// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use anyhow::Result;
use freminal_common::{
    buffer_states::{
        cursor::CursorPos,
        mode::SetMode,
        mode::{Mode, TerminalModes},
        modes::{
            MouseModeNumber, ReportMode,
            alternate_scroll::AlternateScroll,
            decanm::Decanm,
            decarm::Decarm,
            decbkm::Decbkm,
            decckm::Decckm,
            decnkm::Decnkm,
            keypad::KeypadMode,
            mouse::{MouseEncoding, MouseTrack},
            reverse_wrap_around::ReverseWrapAround,
            s8c1t::S8c1t,
            sync_updates::SynchronizedUpdates,
            theme::Theming,
            xtmsewin::XtMseWin,
        },
        terminal_output::TerminalOutput,
        window_manipulation::WindowManipulation,
    },
    config::ThemeMode,
    cursor::CursorVisualStyle,
    terminal_size::{DEFAULT_HEIGHT, DEFAULT_WIDTH},
};

use std::{fmt::Write as _, time::Instant};

use crate::{
    ansi::FreminalAnsiParser,
    input::{KeyEventMeta, TerminalInput, TerminalInputPayload},
    io::PtyWrite,
};

use freminal_buffer::terminal_handler::TerminalHandler as NewHandler;

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

#[derive(Debug)]
pub struct TerminalState {
    pub parser: FreminalAnsiParser,
    pub modes: TerminalModes,
    pub write_tx: crossbeam_channel::Sender<PtyWrite>,
    pub leftover_data: Option<Vec<u8>>,
    pub window_commands: Vec<WindowManipulation>,
    pub cursor_visual_style: CursorVisualStyle,

    /// The `freminal-buffer` implementation — the sole source of truth for
    /// terminal content, cursor position, and format state.
    pub handler: NewHandler,
}

impl Default for TerminalState {
    /// Creates a `TerminalState` with a disconnected (dropped-receiver) PTY write channel.
    ///
    /// This is provided solely so that the test suite can construct a `TerminalState` without
    /// a live PTY.  In production the emulator is always constructed via `TerminalState::new`
    /// with a real `Sender<PtyWrite>` wired to the PTY consumer thread.
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
            cursor_visual_style: CursorVisualStyle::default(),
            handler,
        }
    }

    #[must_use]
    pub(crate) fn get_cursor_visual_style(&self) -> CursorVisualStyle {
        self.handler.cursor_visual_style()
    }

    #[must_use]
    pub(crate) const fn is_normal_display(&self) -> bool {
        self.modes.invert_screen.is_normal_display()
    }

    #[must_use]
    pub fn should_repeat_keys(&self) -> bool {
        self.modes.repeat_keys == Decarm::RepeatKey
    }

    #[must_use]
    pub(crate) const fn show_cursor(&self) -> bool {
        self.handler.show_cursor()
    }

    #[must_use]
    pub(crate) fn skip_draw_always(&self) -> bool {
        self.modes.synchronized_updates == SynchronizedUpdates::DontDraw
    }

    #[must_use]
    pub const fn get_win_size(&mut self) -> (usize, usize) {
        self.handler.get_win_size()
    }

    #[must_use]
    pub(crate) fn cursor_pos(&self) -> CursorPos {
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
    pub(crate) const fn get_cursor_key_mode(&self) -> Decckm {
        self.modes.cursor_key
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

    /// Sync a single `TerminalOutput` into `self.modes` when it carries a
    /// mode flag that lives in `TerminalState` rather than `TerminalHandler`.
    ///
    /// ## Mode ownership split
    ///
    /// Terminal modes are split across two layers:
    ///
    /// - **`TerminalHandler` (buffer layer)** owns modes that directly affect
    ///   buffer mutations: DECAWM, DECOM, DECLRMM, DECCOLM, alternate screen
    ///   (`XtExtscrn` / `AltScreen47`), DEC special graphics, etc.  The
    ///   handler processes these inside `process_outputs()` and updates its own
    ///   internal state.  `sync_mode_flags` receives the same output list but
    ///   hits the catch-all `_ => {}` branch for these variants.
    ///
    /// - **`TerminalState.modes` (`TerminalModes`)** owns modes that affect
    ///   input encoding or GUI-visible behaviour: `cursor_key` (DECCKM),
    ///   `bracketed_paste`, `mouse_tracking`, `mouse_encoding`, `focus_reporting`
    ///   (`XtMseWin`), `repeat_keys` (DECARM), `keypad_mode` (DECPAM/DECNKM),
    ///   `invert_screen` (DECSCNM), `synchronized_updates`, `line_feed_mode`,
    ///   `backarrow_key_mode` (DECBKM), `alternate_scroll`, and `reverse_wrap_around`.
    ///
    /// - **`FreminalAnsiParser.vt52_mode` (parser layer)** — DECANM must also
    ///   be mirrored into the parser so it routes ESC bytes to the correct
    ///   VT52 or ANSI state machine.  This is the only mode flag that needs
    ///   to be copied to a third layer.
    ///
    /// Query variants are intercepted here: the current state is looked up
    /// and a DECRPM response is sent.
    fn sync_mode_flags(&mut self, output: &TerminalOutput) {
        match output {
            TerminalOutput::Mode(mode) => self.sync_mode(mode),
            // DECPAM (ESC =) / DECPNM (ESC >)
            TerminalOutput::ApplicationKeypadMode => {
                self.modes.keypad_mode = KeypadMode::Application;
            }
            TerminalOutput::NormalKeypadMode => {
                self.modes.keypad_mode = KeypadMode::Numeric;
            }
            // S8C1T (ESC SP G) / S7C1T (ESC SP F) — toggle 8-bit C1 control
            // recognition in the parser and response encoding in the handler.
            TerminalOutput::EightBitControl => {
                self.parser.s8c1t_mode = S8c1t::EightBit;
                self.handler.set_s8c1t_mode(S8c1t::EightBit);
            }
            TerminalOutput::SevenBitControl => {
                self.parser.s8c1t_mode = S8c1t::SevenBit;
                self.handler.set_s8c1t_mode(S8c1t::SevenBit);
            }
            _ => {}
        }
    }

    fn sync_mode(&mut self, mode: &Mode) {
        match mode {
            // ── Query variants — respond with DECRPM ──────────
            Mode::Decckm(Decckm::Query)
            | Mode::BracketedPaste(
                freminal_common::buffer_states::modes::rl_bracket::RlBracket::Query,
            )
            | Mode::MouseMode(MouseTrack::Query(_))
            | Mode::XtMseWin(XtMseWin::Query)
            | Mode::Decscnm(freminal_common::buffer_states::modes::decscnm::Decscnm::Query)
            | Mode::Decarm(Decarm::Query)
            | Mode::SynchronizedUpdates(SynchronizedUpdates::Query)
            | Mode::Decnkm(Decnkm::Query)
            | Mode::Decbkm(Decbkm::Query)
            | Mode::AlternateScroll(AlternateScroll::Query)
            | Mode::Theming(Theming::Query) => {
                self.handle_mode_query(mode);
            }
            // ── Set/Reset variants — sync into self.modes ─────
            Mode::Decckm(v) => self.modes.cursor_key = *v,
            Mode::BracketedPaste(v) => self.modes.bracketed_paste = v.clone(),
            Mode::MouseMode(v) => self.modes.mouse_tracking = v.clone(),
            Mode::MouseEncodingMode(v) => self.modes.mouse_encoding = v.clone(),
            Mode::XtMseWin(v) => self.modes.focus_reporting = v.clone(),
            Mode::Decscnm(v) => self.modes.invert_screen = v.clone(),
            Mode::Decarm(v) => self.modes.repeat_keys = *v,
            // ?45 set/reset: sync into TerminalModes for backwards compat.
            // Query is answered by the handler; ignore it here.
            Mode::ReverseWrapAround(
                v @ (ReverseWrapAround::WrapAround | ReverseWrapAround::DontWrap),
            ) => self.modes.reverse_wrap_around = *v,
            Mode::SynchronizedUpdates(v) => self.modes.synchronized_updates = v.clone(),
            Mode::LineFeedMode(v) => self.modes.line_feed_mode = *v,
            Mode::Decnkm(Decnkm::Application) => {
                self.modes.keypad_mode = KeypadMode::Application;
            }
            Mode::Decnkm(Decnkm::Numeric) => {
                self.modes.keypad_mode = KeypadMode::Numeric;
            }
            Mode::Decbkm(v) => self.modes.backarrow_key_mode = *v,
            Mode::AlternateScroll(v) => self.modes.alternate_scroll = *v,
            // ── Modes handled entirely by TerminalHandler ──────
            Mode::XtExtscrn(_)
            | Mode::AltScreen47(_)
            | Mode::SaveCursor1048(_)
            | Mode::Decawm(_)
            | Mode::Dectem(_)
            | Mode::XtCBlink(_)
            | Mode::Decom(_)
            | Mode::Deccolm(_)
            | Mode::Declrmm(_)
            | Mode::AllowColumnModeSwitch(_)
            | Mode::AllowAltScreen(_)
            | Mode::UnknownQuery(_)
            | Mode::ApplicationEscapeKey(_)
            | Mode::InBandResizeMode(_)
            | Mode::GraphemeClustering(_)
            | Mode::Decsdm(_)
            | Mode::Decnrcm(_)
            | Mode::Irm(_)
            | Mode::PrivateColorRegisters(_)
            | Mode::ReverseWrapAround(_)
            | Mode::XtRevWrap2(_)
            | Mode::Decanm(Decanm::Query) => {}
            // DECANM — toggle the parser between VT52 and ANSI modes.
            // The handler owns the authoritative `vt52_mode` flag, but
            // the parser also needs to know so it routes ESC bytes to
            // the correct state machine.
            Mode::Decanm(Decanm::Vt52) => {
                self.parser.vt52_mode = Decanm::Vt52;
            }
            Mode::Decanm(Decanm::Ansi) => {
                self.parser.vt52_mode = Decanm::Ansi;
            }
            // ── ?2031 Theming Set/Reset — only honoured when mode is Auto ──
            //
            // When `theme_mode` is `Dark` or `Light`, the mode is locked and
            // application DECSET/DECRST requests are silently ignored.  When
            // `theme_mode` is `Auto`, the Theming state is updated to reflect
            // the application's requested preference.
            Mode::Theming(v) => {
                if self.modes.theme_mode == ThemeMode::Auto {
                    self.modes.theming = v.clone();
                } else {
                    debug!(
                        "?2031 Theming mode change ignored: theme_mode={:?} is locked",
                        self.modes.theme_mode
                    );
                }
            }
            // ── Modes parsed but not yet acted on ─────────────
            Mode::NoOp | Mode::Decsclm(_) | Mode::Unknown(_) => {
                debug!("Mode not acted on by either layer: {mode}");
            }
        }
    }

    /// Handle DECRQM query variants — respond with the appropriate DECRPM.
    fn handle_mode_query(&self, mode: &Mode) {
        match mode {
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
                let ps = match *report_mode {
                    1005 | 1006 | 1016 => {
                        let active_enc_num = self.modes.mouse_encoding.mouse_mode_number();
                        if active_enc_num == *report_mode
                            && self.modes.mouse_encoding != MouseEncoding::X11
                        {
                            1 // set
                        } else {
                            2 // reset
                        }
                    }
                    _ => {
                        let active_num = self.modes.mouse_tracking.mouse_mode_number();
                        if active_num == *report_mode
                            && self.modes.mouse_tracking != MouseTrack::NoTracking
                        {
                            1 // set
                        } else {
                            2 // reset
                        }
                    }
                };
                let resp = format!("\x1b[?{report_mode};{ps}$y");
                self.send_decrpm(&resp);
            }
            Mode::XtMseWin(XtMseWin::Query) => {
                let resp = self.modes.focus_reporting.report(None);
                self.send_decrpm(&resp);
            }
            Mode::Decscnm(freminal_common::buffer_states::modes::decscnm::Decscnm::Query) => {
                let resp = self.modes.invert_screen.report(None);
                self.send_decrpm(&resp);
            }
            Mode::Decarm(Decarm::Query) => {
                let resp = self.modes.repeat_keys.report(None);
                self.send_decrpm(&resp);
            }
            Mode::SynchronizedUpdates(SynchronizedUpdates::Query) => {
                let resp = self.modes.synchronized_updates.report(None);
                self.send_decrpm(&resp);
            }
            Mode::Decnkm(Decnkm::Query) => {
                let override_mode = match self.modes.keypad_mode {
                    KeypadMode::Application => SetMode::DecSet,
                    KeypadMode::Numeric => SetMode::DecRst,
                };
                self.send_decrpm(&Decnkm::Application.report(Some(override_mode)));
            }
            Mode::Decbkm(Decbkm::Query) => {
                let resp = self.modes.backarrow_key_mode.report(None);
                self.send_decrpm(&resp);
            }
            Mode::AlternateScroll(AlternateScroll::Query) => {
                let resp = self.modes.alternate_scroll.report(None);
                self.send_decrpm(&resp);
            }
            // ── ?2031 Theming query ─────────────────────────────────────────
            //
            // Response codes defined in the xterm spec:
            //   Ps=1 → permanently set (light mode active, locked)
            //   Ps=2 → permanently reset (dark mode active, locked)
            //   Ps=3 → temporarily set (was: light mode, but can be changed)
            //   Ps=4 → temporarily reset (was: dark mode, but can be changed)
            //
            // Freminal usage:
            //   `ThemeMode::Light` → Ps=1 (light locked — permanently set)
            //   `ThemeMode::Dark`  → Ps=2 (dark locked — permanently reset)
            //   `ThemeMode::Auto`  → Ps=3 or Ps=4 based on current Theming state
            //                        (dynamically follows OS preference; app can override)
            Mode::Theming(Theming::Query) => {
                let resp = match self.modes.theme_mode {
                    ThemeMode::Light => String::from("\x1b[?2031;1$y"),
                    ThemeMode::Dark => String::from("\x1b[?2031;2$y"),
                    ThemeMode::Auto => match self.modes.theming {
                        Theming::Light => String::from("\x1b[?2031;3$y"),
                        Theming::Dark | Theming::Query => String::from("\x1b[?2031;4$y"),
                    },
                };
                self.send_decrpm(&resp);
            }
            _ => {}
        }
    }

    /// Drain the tmux reparse queue, parsing and processing any queued
    /// raw bytes (CSI/OSC sequences from DCS tmux passthrough).
    fn drain_tmux_reparse_queue(&mut self) {
        loop {
            let reparse = self.handler.take_tmux_reparse_queue();
            if reparse.is_empty() {
                break;
            }
            for raw in reparse {
                let reparsed = self.parser.push(&raw);
                for output in &reparsed {
                    trace!(%output, "reparsed tmux passthrough output");
                }
                self.handler.process_outputs(&reparsed);
                for output in &reparsed {
                    self.sync_mode_flags(output);
                }
            }
        }
    }

    /// Process one chunk of raw PTY bytes through the full terminal pipeline.
    ///
    /// ## Pipeline stages
    ///
    /// 1. **UTF-8 reassembly** — prepends any bytes saved from the previous
    ///    call (`leftover_data`) that form an incomplete multi-byte sequence.
    ///
    /// 2. **Tail scan** — scans at most the last 3 bytes of the combined buffer
    ///    looking for a UTF-8 leading byte whose declared sequence length extends
    ///    past the end of the buffer.  If found, those bytes are split off and
    ///    stored in `leftover_data` for the next call.  This is O(1) — no
    ///    full-buffer clone is required.
    ///
    /// 3. **Parser** — feeds the complete (non-trailing) bytes to
    ///    `FreminalAnsiParser::push()`, which produces a `Vec<TerminalOutput>`.
    ///
    /// 4. **Buffer mutations** — `TerminalHandler::process_outputs()` applies
    ///    every `TerminalOutput` item to the buffer: text insertion, cursor
    ///    movement, erase operations, mode changes, etc.
    ///
    /// 5. **Mode sync** — iterates the same output list a second time to update
    ///    the mode flags that live in `TerminalState` rather than the handler
    ///    (mouse tracking, bracketed paste, focus reporting, DECANM, etc.).
    ///
    /// 6. **RIS reset** — if any item is `ResetDevice` (ESC c), resets all
    ///    `TerminalState`-owned mode fields and clears window commands.
    ///
    /// 7. **Window commands** — drains the handler's `window_commands` queue
    ///    into `self.window_commands` so the GUI's `handle_window_manipulation`
    ///    drain loop can pick them up on the next frame.
    ///
    /// 8. **tmux reparse** — drains any raw bytes queued by the DCS tmux
    ///    passthrough handler, re-runs them through the parser and handler,
    ///    and loops until the queue is empty.
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
        for output in &parsed {
            self.sync_mode_flags(output);
        }

        // ── RIS (ESC c) — full terminal reset ──────────────────────────
        //
        // If the parsed output contains a ResetDevice, the handler has already
        // reset all buffer-level state.  We also need to reset the state that
        // lives in TerminalState: modes, parser, leftover data, and cursor
        // visual style.  `write_tx` is preserved (user configuration).
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

        // ── tmux passthrough reparse queue ─────────────────────────────
        //
        // tmux DCS passthrough can contain inner CSI or OSC sequences that
        // the handler cannot parse (the ANSI parser lives here, not in the
        // handler).  After process_outputs() returns, we drain any queued
        // raw bytes, feed them through the parser, and process the resulting
        // TerminalOutput items.  This loop runs until the queue is empty
        // (inner sequences are unlikely to produce more reparse items, but
        // we handle it for correctness).
        self.drain_tmux_reparse_queue();

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
        let decckm = self.get_cursor_key_mode();
        let keypad_app = self.modes.keypad_mode;
        let modify_other_keys = self.handler.modify_other_keys_level();
        let application_escape_key = self.handler.application_escape_key();
        let backarrow_sends_bs = self.modes.backarrow_key_mode;
        let line_feed_mode = self.modes.line_feed_mode;
        let kitty_keyboard_flags = self.handler.kitty_keyboard_flags();
        match to_write.to_payload(
            decckm,
            keypad_app,
            modify_other_keys,
            application_escape_key,
            backarrow_sends_bs,
            line_feed_mode,
            kitty_keyboard_flags,
            &KeyEventMeta::PRESS,
        ) {
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
