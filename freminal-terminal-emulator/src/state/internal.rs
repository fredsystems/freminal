// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use anyhow::Result;
use conv2::ConvUtil;
use freminal_common::{
    buffer_states::{
        cursor::CursorPos,
        format_tag::FormatTag,
        mode::SetMode,
        mode::{Mode, TerminalModes},
        modes::{
            MouseModeNumber, ReportMode, decarm::Decarm, decckm::Decckm, keypad::KeypadMode,
            mouse::MouseTrack, reverse_wrap_around::ReverseWrapAround,
            sync_updates::SynchronizedUpdates, xtmsewin::XtMseWin,
        },
        tchar::TChar,
        terminal_output::TerminalOutput,
        window_manipulation::WindowManipulation,
    },
    cursor::CursorVisualStyle,
    terminal_size::{DEFAULT_HEIGHT, DEFAULT_WIDTH},
};

use std::{fmt::Write as _, time::Instant};

use crate::{
    ansi::FreminalAnsiParser,
    interface::{TerminalInput, TerminalInputPayload, collect_text},
    io::PtyWrite,
};

use freminal_buffer::terminal_handler::TerminalHandler as NewHandler;

use super::data::TerminalSections;

/// Format the first `max_bytes` of `data` as a hex string for trace logging.
///
/// Returns a `String` like `"48 65 6c 6c 6f"`. If `data` is longer than
/// `max_bytes`, the output is truncated and `"..."` is appended.
fn hex_preview(data: &[u8], max_bytes: usize) -> String {
    let truncated = data.len() > max_bytes;
    let slice = if truncated { &data[..max_bytes] } else { data };

    // Each byte is "XX " (3 chars) — pre-allocate.
    let mut out = String::with_capacity(slice.len() * 3 + if truncated { 3 } else { 0 });
    for (i, b) in slice.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        // write! to a String is infallible; ignore the result.
        let _: std::fmt::Result = write!(out, "{b:02x}");
    }
    if truncated {
        out.push_str("...");
    }
    out
}

#[derive(Debug, Default)]
pub enum Theme {
    Light,
    #[default]
    Dark,
}

impl From<bool> for Theme {
    fn from(dark_mode: bool) -> Self {
        if dark_mode { Self::Dark } else { Self::Light }
    }
}

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct TerminalState {
    pub parser: FreminalAnsiParser,
    pub modes: TerminalModes,
    pub write_tx: crossbeam_channel::Sender<PtyWrite>,
    pub leftover_data: Option<Vec<u8>>,
    pub window_commands: Vec<WindowManipulation>,
    pub theme: Theme,
    pub cursor_visual_style: CursorVisualStyle,

    /// The `freminal-buffer` implementation — the sole source of truth for
    /// terminal content, cursor position, and format state.
    pub handler: NewHandler,
}

impl Default for TerminalState {
    /// This method should never really be used. It was added to allow the test suite to pass.
    /// The problem here is that you most likely really really want a rx channel to go with the tx channel.
    fn default() -> Self {
        Self::new(crossbeam_channel::unbounded().0, None)
    }
}

impl PartialEq for TerminalState {
    fn eq(&self, other: &Self) -> bool {
        self.parser == other.parser
            && self.modes == other.modes
            && self.leftover_data == other.leftover_data
    }
}

impl TerminalState {
    /// Create a new `TerminalState`.
    ///
    /// `scrollback_limit` overrides the default scrollback history size when
    /// `Some(n)` is provided.  `None` keeps the compiled-in default (4000).
    #[must_use]
    pub fn new(
        write_tx: crossbeam_channel::Sender<PtyWrite>,
        scrollback_limit: Option<usize>,
    ) -> Self {
        let handler = {
            let mut h = NewHandler::new(DEFAULT_WIDTH as usize, DEFAULT_HEIGHT as usize);
            if let Some(limit) = scrollback_limit {
                h = h.with_scrollback_limit(limit);
            }
            // Pass the PtyWrite sender directly so the handler can write
            // escape-sequence responses (DA, CPR, etc.) straight to the PTY
            // without an intermediate forwarding thread.
            h.set_write_tx(write_tx.clone());
            h
        };

        Self {
            parser: FreminalAnsiParser::new(),
            modes: TerminalModes::default(),
            write_tx,
            leftover_data: None,
            window_commands: Vec::new(),
            theme: Theme::default(),
            cursor_visual_style: CursorVisualStyle::default(),
            handler,
        }
    }

    #[must_use]
    pub fn get_cursor_visual_style(&self) -> CursorVisualStyle {
        self.handler.cursor_visual_style()
    }

    /// Return the cursor color.
    /// The cursor color is not yet tracked by the new handler, so we return the terminal default.
    #[must_use]
    pub const fn cursor_color(&self) -> freminal_common::colors::TerminalColor {
        freminal_common::colors::TerminalColor::DefaultCursorColor
    }

    pub const fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    #[must_use]
    pub const fn is_normal_display(&self) -> bool {
        self.modes.invert_screen.is_normal_display()
    }

    #[must_use]
    pub fn should_repeat_keys(&self) -> bool {
        self.modes.repeat_keys == Decarm::RepeatKey
    }

    #[must_use]
    pub const fn show_cursor(&mut self) -> bool {
        self.handler.show_cursor()
    }

    #[must_use]
    pub fn skip_draw_always(&self) -> bool {
        self.modes.synchronized_updates == SynchronizedUpdates::DontDraw
    }

    #[must_use]
    pub const fn get_win_size(&mut self) -> (usize, usize) {
        self.handler.get_win_size()
    }

    #[allow(clippy::needless_pass_by_ref_mut)]
    pub const fn is_mouse_hovered_on_url(&mut self, pos: &CursorPos) -> Option<String> {
        // URL hover detection is not yet ported to the new handler.
        let _ = pos;
        None
    }

    #[allow(clippy::missing_const_for_fn)]
    #[allow(clippy::needless_pass_by_ref_mut)]
    pub(crate) fn data_and_format_data_for_gui(
        &mut self,
        scroll_offset: usize,
    ) -> (
        TerminalSections<Vec<TChar>>,
        TerminalSections<Vec<FormatTag>>,
    ) {
        self.handler.data_and_format_data_for_gui(scroll_offset)
    }

    #[must_use]
    pub fn cursor_pos(&mut self) -> CursorPos {
        self.handler.cursor_pos()
    }

    pub fn set_win_size(
        &mut self,
        width: usize,
        height: usize,
        cell_pixel_width: u32,
        cell_pixel_height: u32,
    ) {
        self.handler
            .handle_resize(width, height, cell_pixel_width, cell_pixel_height);
    }

    #[must_use]
    pub fn get_cursor_key_mode(&self) -> Decckm {
        self.modes.cursor_key.clone()
    }

    /// Send the focus-change escape sequence to the PTY if focus reporting is enabled.
    ///
    /// This no longer touches `window_focused`; the GUI owns that field on `ViewState`.
    pub fn send_focus_event(&mut self, focused: bool) {
        if self.modes.focus_reporting == XtMseWin::Disabled {
            return;
        }

        let to_write = if focused {
            TerminalInput::InFocus
        } else {
            TerminalInput::LostFocus
        };

        if let Err(e) = self.write(&to_write) {
            error!("Failed to write focus change: {e}");
        }

        debug!("Reported focus change to terminal");
    }

    #[allow(clippy::too_many_lines)]
    pub fn handle_incoming_data(&mut self, incoming: &[u8]) {
        debug!("Handling Incoming Data");
        trace!(
            bytes = incoming.len(),
            hex = %hex_preview(incoming, 512),
            "PTY data received"
        );
        let now = Instant::now();

        // Reassemble with any leftover bytes from a split UTF-8 sequence.
        let mut incoming = self.leftover_data.take().map_or_else(
            || incoming.to_vec(),
            |leftover_data| {
                debug!("We have leftover data: {:?}", leftover_data);
                let mut new_data = Vec::with_capacity(leftover_data.len() + incoming.len());
                new_data.extend_from_slice(&leftover_data);
                new_data.extend_from_slice(incoming);
                trace!("Reassembled buffer: {:?}", new_data);
                self.leftover_data = None;
                new_data
            },
        );

        // Strip any trailing incomplete UTF-8 sequence and save it for next time.
        //
        // A UTF-8 sequence is at most 4 bytes, so any split can leave at most
        // 3 trailing bytes that are part of an incomplete sequence.  We scan
        // only the tail — no full-buffer clone required.
        //
        // The algorithm:
        //   1. Walk backwards over the last 3 bytes (or fewer if the buffer is
        //      shorter) looking for a non-continuation byte (i.e. a leading byte
        //      of a multi-byte sequence: 0xC0–0xFF) that starts a sequence whose
        //      declared length extends past the end of the buffer.
        //   2. If we find such a byte, everything from that position onwards is
        //      the incomplete tail; split it off.
        //   3. If every byte in the tail is a valid ASCII byte or a complete
        //      sequence we leave the buffer unchanged — no allocation at all.
        let split_at: Option<usize> = {
            let len = incoming.len();
            // Scan at most the last 3 bytes (max continuation bytes in UTF-8).
            let scan_start = len.saturating_sub(3);
            let mut found = None;
            for i in (scan_start..len).rev() {
                let b = incoming[i];
                // Leading byte of a 2-byte sequence: 110x xxxx
                // Leading byte of a 3-byte sequence: 1110 xxxx
                // Leading byte of a 4-byte sequence: 1111 0xxx
                let seq_len = if b & 0b1110_0000 == 0b1100_0000 {
                    2
                } else if b & 0b1111_0000 == 0b1110_0000 {
                    3
                } else if b & 0b1111_1000 == 0b1111_0000 {
                    4
                } else {
                    // ASCII or continuation byte — not a leading byte, keep scanning.
                    continue;
                };
                // If the declared sequence extends past the end of the buffer,
                // this leading byte begins an incomplete sequence.
                if i + seq_len > len {
                    found = Some(i);
                }
                // Whether or not it's incomplete we stop scanning: a leading byte
                // can only appear once per sequence.
                break;
            }
            found
        };

        if let Some(split) = split_at {
            let leftover_bytes = incoming.split_off(split);
            match self.leftover_data {
                Some(ref mut self_leftover) => {
                    self_leftover.splice(0..0, leftover_bytes);
                }
                None => self.leftover_data = Some(leftover_bytes),
            }
        }

        let parsed = self.parser.push(&incoming);

        for output in &parsed {
            trace!(%output, "parsed terminal output");
        }

        self.handler.process_outputs(&parsed);

        // ── Sync mode flags that the handler doesn't own ─────────────
        //
        // The handler processes buffer-level modes (auto-wrap, alt screen,
        // cursor visibility, cursor blink, LNM).  Every other mode flag
        // lives in `self.modes` and is read by the snapshot / GUI layer.
        // We iterate the parsed output and update `self.modes` for each
        // relevant Mode variant.  Query variants are intercepted here:
        // the current state is looked up and a DECRPM response is sent.
        for output in &parsed {
            match output {
                TerminalOutput::Mode(mode) => match mode {
                    // ── Query variants — respond with DECRPM ──────────
                    //
                    // We compute the response string first (immutable borrow
                    // of self.modes), then send it (shared borrow of
                    // self.write_tx).  This avoids a simultaneous &mut self
                    // + &self.modes borrow conflict.
                    Mode::Decckm(Decckm::Query) => {
                        let resp = self.modes.cursor_key.report(None);
                        self.send_decrpm(&resp);
                    }
                    Mode::BracketedPaste(
                        freminal_common::buffer_states::modes::rl_bracket::RlBracket::Query,
                    ) => {
                        let resp = self.modes.bracketed_paste.report(None);
                        self.send_decrpm(&resp);
                    }
                    Mode::MouseMode(MouseTrack::Query(report_mode)) => {
                        // Determine whether the queried mode number is the currently
                        // active mouse tracking mode, then report via DECRPM using
                        // the queried mode number as Ps.
                        let active_num = self.modes.mouse_tracking.mouse_mode_number();
                        let override_mode = if active_num == *report_mode
                            && self.modes.mouse_tracking != MouseTrack::NoTracking
                        {
                            SetMode::DecSet
                        } else {
                            SetMode::DecRst
                        };
                        let resp = MouseTrack::Query(*report_mode).report(Some(override_mode));
                        self.send_decrpm(&resp);
                    }
                    Mode::XtMseWin(XtMseWin::Query) => {
                        let resp = self.modes.focus_reporting.report(None);
                        self.send_decrpm(&resp);
                    }
                    Mode::Decscnm(
                        freminal_common::buffer_states::modes::decscnm::Decscnm::Query,
                    ) => {
                        let resp = self.modes.invert_screen.report(None);
                        self.send_decrpm(&resp);
                    }
                    Mode::Decarm(Decarm::Query) => {
                        let resp = self.modes.repeat_keys.report(None);
                        self.send_decrpm(&resp);
                    }
                    Mode::ReverseWrapAround(ReverseWrapAround::Query) => {
                        let resp = self.modes.reverse_wrap_around.report(None);
                        self.send_decrpm(&resp);
                    }
                    Mode::SynchronizedUpdates(SynchronizedUpdates::Query) => {
                        let resp = self.modes.synchronized_updates.report(None);
                        self.send_decrpm(&resp);
                    }
                    // ── Set/Reset variants — sync into self.modes ─────
                    // 7.7  — DECCKM (?1)
                    Mode::Decckm(v) => self.modes.cursor_key = v.clone(),
                    // 7.8  — Bracketed paste (?2004)
                    Mode::BracketedPaste(v) => self.modes.bracketed_paste = v.clone(),
                    // 7.9  — Mouse tracking (?9/?1000/?1002/?1003/?1005/?1006/?1016)
                    Mode::MouseMode(v) => self.modes.mouse_tracking = v.clone(),
                    // 7.10 — Focus events (?1004)
                    Mode::XtMseWin(v) => self.modes.focus_reporting = v.clone(),
                    // 7.15 — DECSCNM (?5) screen inversion
                    Mode::Decscnm(v) => self.modes.invert_screen = v.clone(),
                    // 7.15 — DECARM (?8) repeat keys
                    Mode::Decarm(v) => self.modes.repeat_keys = v.clone(),
                    // Reverse wrap around (?45)
                    Mode::ReverseWrapAround(v) => self.modes.reverse_wrap_around = v.clone(),
                    // Synchronized updates (?2026)
                    Mode::SynchronizedUpdates(v) => self.modes.synchronized_updates = v.clone(),
                    // LNM (20) — handler handles buffer-level, keep modes in sync
                    Mode::LineFeedMode(v) => self.modes.line_feed_mode = v.clone(),
                    // All other modes are either:
                    // - Handled entirely by the handler (XtExtscrn, Decawm,
                    //   Dectcem, XtCBlink, UnknownQuery)
                    // - Parsed but not yet acted on (Decom, Deccolm, etc.)
                    other => {
                        debug!("Mode not tracked in TerminalState mode-sync: {other}");
                    }
                },
                // 7.14 — DECPAM (ESC =) / DECPNM (ESC >)
                TerminalOutput::ApplicationKeypadMode => {
                    self.modes.keypad_mode = KeypadMode::Application;
                }
                TerminalOutput::NormalKeypadMode => {
                    self.modes.keypad_mode = KeypadMode::Numeric;
                }
                _ => {}
            }
        }

        // ── RIS (ESC c) — full terminal reset ──────────────────────────
        //
        // If the parsed output contains a ResetDevice, the handler has already
        // reset all buffer-level state.  We also need to reset the state that
        // lives in TerminalState: modes, parser, leftover data, and cursor
        // visual style.  Theme and write_tx are preserved (user configuration).
        if parsed
            .iter()
            .any(|o| matches!(o, TerminalOutput::ResetDevice))
        {
            self.modes = TerminalModes::default();
            self.parser = FreminalAnsiParser::new();
            self.leftover_data = None;
            self.cursor_visual_style = CursorVisualStyle::default();
            self.window_commands.clear();
        }

        // Drain window commands queued by the new handler into the shared vec
        // so that the GUI's existing drain loop in handle_window_manipulation
        // can consume them.
        self.window_commands
            .extend(self.handler.take_window_commands());

        let elapsed = now.elapsed();
        if elapsed.as_millis() > 0 {
            debug!("Data processing time: {}ms", elapsed.as_millis());
        } else {
            debug!("Data processing time: {}μs", elapsed.as_micros());
        }

        debug!("Finished handling incoming data");
    }

    /// Write data to the terminal
    ///
    /// # Errors
    /// Will return an error if the write fails
    pub fn write(&self, to_write: &TerminalInput) -> Result<()> {
        let decckm = self.get_cursor_key_mode() == Decckm::Application;
        let keypad_app = self.modes.keypad_mode == KeypadMode::Application;
        match to_write.to_payload(decckm, keypad_app) {
            TerminalInputPayload::Single(c) => {
                self.write_tx.send(PtyWrite::Write(vec![c]))?;
            }
            TerminalInputPayload::Many(to_write) => {
                self.write_tx.send(PtyWrite::Write(to_write.to_vec()))?;
            }
            TerminalInputPayload::Owned(bytes) => {
                self.write_tx.send(PtyWrite::Write(bytes))?;
            }
        }

        Ok(())
    }

    pub fn scroll(&mut self, scroll: f32) {
        // In alternate screen, route scrolling as arrow-key presses.
        // In primary screen, use the new handler's scroll helpers.
        let in_alternate = self.handler.is_alternate_screen();

        if in_alternate {
            let key = if scroll < 0.0 {
                TerminalInput::ArrowDown(crate::interface::KeyModifiers::NONE)
            } else {
                TerminalInput::ArrowUp(crate::interface::KeyModifiers::NONE)
            };
            match self.write(&key) {
                Ok(()) => (),
                Err(e) => error!("Failed to scroll: {e}"),
            }
            return;
        }

        let mut scroll = scroll.round();
        if scroll < 0.0 {
            scroll *= -1.0;
            let n = scroll.max(1.0).approx_as::<usize>().unwrap_or(1);
            // scroll_offset lives in ViewState (Task 4); pass 0 temporarily.
            // The returned new offset is discarded until ViewState is wired (Task 7/8).
            let _new_offset = self.handler.handle_scroll_back(0, n);
        } else {
            let n = scroll.max(1.0).approx_as::<usize>().unwrap_or(1);
            // scroll_offset lives in ViewState (Task 4); pass 0 temporarily.
            let _new_offset = self.handler.handle_scroll_forward(0, n);
        }
    }

    pub fn report_window_state(&mut self, minimized: bool) {
        let output = if minimized {
            collect_text(&String::from("\x1b[2t"))
        } else {
            collect_text(&String::from("\x1b[1t"))
        };
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write window state: {e}");
                }
            }
        }
    }

    pub fn report_window_position(&mut self, x: usize, y: usize) {
        let output = collect_text(&format!("\x1b[3;{x};{y}t"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write window position: {e}");
                }
            }
        }
    }

    pub fn report_window_size(&mut self, width: usize, height: usize) {
        let output = collect_text(&format!("\x1b[4;{height};{width}t"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write window size: {e}");
                }
            }
        }
    }

    pub fn report_root_window_size(&mut self, width: usize, height: usize) {
        let output = collect_text(&format!("\x1b[5;{height};{width}t"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write window size: {e}");
                }
            }
        }
    }

    pub fn report_character_size(&mut self, width: usize, height: usize) {
        let output = collect_text(&format!("\x1b[6;{height};{width}t"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write character size: {e}");
                }
            }
        }
    }

    pub fn report_terminal_size_in_characters(&mut self, width: usize, height: usize) {
        let output = collect_text(&format!("\x1b[8;{height};{width}t"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write terminal size in characters: {e}");
                }
            }
        }
    }

    pub fn report_root_terminal_size_in_characters(&mut self, width: usize, height: usize) {
        let output = collect_text(&format!("\x1b[9;{height};{width}t"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write terminal size in characters: {e}");
                }
            }
        }
    }

    pub fn report_icon_label(&mut self, title: &str) {
        let output = collect_text(&format!("\x1b]L{title}\x1b\\"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write title: {e}");
                }
            }
        }
    }

    pub fn report_title(&mut self, title: &str) {
        let output = collect_text(&format!("\x1b]l{title}\x1b\\"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write title: {e}");
                }
            }
        }
    }

    pub fn report_mode(&mut self, report: &String) {
        let report = collect_text(report);
        for input in report.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write mode report: {e}");
                }
            }
        }
    }

    /// Send a DECRPM response string directly to the PTY.
    ///
    /// This bypasses the `TerminalInput` encoding path — the response is an
    /// escape sequence that must be sent verbatim.
    fn send_decrpm(&self, response: &str) {
        if let Err(e) = self
            .write_tx
            .send(PtyWrite::Write(response.as_bytes().to_vec()))
        {
            error!("Failed to send DECRPM response: {e}");
        }
    }
}
