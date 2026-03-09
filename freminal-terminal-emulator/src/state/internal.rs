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
        mode::TerminalModes,
        modes::{
            decarm::Decarm, decckm::Decckm, sync_updates::SynchronizedUpdates, xtmsewin::XtMseWin,
        },
        tchar::TChar,
        window_manipulation::WindowManipulation,
    },
    cursor::CursorVisualStyle,
    terminal_size::{DEFAULT_HEIGHT, DEFAULT_WIDTH},
};

use std::time::Instant;

use crate::{
    ansi::FreminalAnsiParser,
    interface::{TerminalInput, TerminalInputPayload, collect_text},
    io::PtyWrite,
};

use freminal_buffer::terminal_handler::TerminalHandler as NewHandler;

use super::data::TerminalSections;

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
        Self::new(crossbeam_channel::unbounded().0)
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
    #[must_use]
    pub fn new(write_tx: crossbeam_channel::Sender<PtyWrite>) -> Self {
        let handler = {
            let mut h = NewHandler::new(DEFAULT_WIDTH as usize, DEFAULT_HEIGHT as usize);
            // Bridge: the new handler writes raw bytes; forward them to the
            // emulator's PtyWrite::Write channel so the PTY receives them.
            let (bytes_tx, bytes_rx) = crossbeam_channel::unbounded::<Vec<u8>>();
            h.set_write_tx(bytes_tx);
            let fwd_tx = write_tx.clone();
            std::thread::spawn(move || {
                while let Ok(bytes) = bytes_rx.recv() {
                    if fwd_tx.send(PtyWrite::Write(bytes)).is_err() {
                        break;
                    }
                }
            });
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
    ) -> (
        TerminalSections<Vec<TChar>>,
        TerminalSections<Vec<FormatTag>>,
    ) {
        self.handler.data_and_format_data_for_gui()
    }

    #[must_use]
    pub fn cursor_pos(&mut self) -> CursorPos {
        self.handler.cursor_pos()
    }

    pub fn set_win_size(&mut self, width: usize, height: usize) {
        self.handler.handle_resize(width, height);
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
        let mut leftover_bytes = vec![];
        while let Err(_e) = String::from_utf8(incoming.clone()) {
            let Some(p) = incoming.pop() else { break };
            leftover_bytes.insert(0, p);
        }
        if !leftover_bytes.is_empty() {
            match self.leftover_data {
                Some(ref mut self_leftover) => {
                    self_leftover.splice(0..0, leftover_bytes);
                }
                None => self.leftover_data = Some(leftover_bytes),
            }
        }

        let parsed = self.parser.push(&incoming);

        self.handler.process_outputs(&parsed);
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
        match to_write.to_payload(decckm, decckm) {
            TerminalInputPayload::Single(c) => {
                self.write_tx.send(PtyWrite::Write(vec![c]))?;
            }
            TerminalInputPayload::Many(to_write) => {
                self.write_tx.send(PtyWrite::Write(to_write.to_vec()))?;
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
                TerminalInput::ArrowDown
            } else {
                TerminalInput::ArrowUp
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

    pub fn report_device_name_and_version(&mut self) {
        let version = format!(
            "{}-{}",
            env!("CARGO_PKG_VERSION"),
            env!("VERGEN_BUILD_TIMESTAMP")
        );
        let output = collect_text(&format!("\x1bP>|Freminal {version}\x1b\\"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write device name and version: {e}");
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
}
