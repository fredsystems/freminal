// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::borrow::Cow;

use crate::ansi_components::modes::dectcem::Dectcem;
use crate::io::DummyIo;
use crate::io::FreminalPtyInputOutput;
use crate::io::{FreminalTermInputOutput, FreminalTerminalSize, PtyRead, PtyWrite};
use crate::state::{data::TerminalSections, internal::TerminalState};
use anyhow::Result;
use crossbeam_channel::{unbounded, Receiver};
use eframe::egui;

use freminal_common::buffer_states::cursor::CursorPos;
use freminal_common::buffer_states::format_tag::FormatTag;
use freminal_common::{
    args::Args, buffer_states::tchar::TChar, cursor::CursorVisualStyle,
    terminal_size::DEFAULT_HEIGHT, terminal_size::DEFAULT_WIDTH,
};

const fn char_to_ctrl_code(c: u8) -> u8 {
    // https://catern.com/posts/terminal_quirks.html
    // man ascii
    c & 0b0001_1111
}

#[must_use]
pub fn collect_text(text: &String) -> Cow<'static, [TerminalInput]> {
    text.as_bytes()
        .iter()
        .map(|c| TerminalInput::Ascii(*c))
        .collect::<Vec<_>>()
        .into()
}

#[must_use]
pub fn raw_ascii_bytes_to_terminal_input(buf: &[u8]) -> Cow<'static, [TerminalInput]> {
    buf.iter()
        .map(|c| TerminalInput::Ascii(*c))
        .collect::<Vec<_>>()
        .into()
}

#[derive(Eq, PartialEq, Debug)]
pub enum TerminalInputPayload {
    Single(u8),
    Many(&'static [u8]),
}

#[derive(Clone, Debug)]
pub enum TerminalInput {
    // Normal keypress
    Ascii(u8),
    // Normal keypress with ctrl
    Ctrl(u8),
    Enter,
    LineFeed,
    Backspace,
    ArrowRight,
    ArrowLeft,
    ArrowUp,
    ArrowDown,
    Home,
    End,
    Delete,
    Insert,
    PageUp,
    PageDown,
    Tab,
    Escape,
    InFocus,
    LostFocus,
    KeyPad(u8),
}

impl TerminalInput {
    #[must_use]
    pub fn to_payload(&self, decckm_mode: bool, keypad_mode: bool) -> TerminalInputPayload {
        match self {
            Self::Ascii(c) => TerminalInputPayload::Single(*c),
            Self::Ctrl(c) => TerminalInputPayload::Single(char_to_ctrl_code(*c)),
            // I have NO idea why this is the case, but just sending a '\n' fucks up some things
            // For instance nvim and lazygit will not response to an enter key press with \n
            // The shell itself is fine with \n. So who knows.
            // TODO: really fix this out one.
            Self::Enter => TerminalInputPayload::Single(char_to_ctrl_code(b'm')),
            Self::LineFeed => TerminalInputPayload::Single(b'\n'),
            // Hard to tie back, but check default VERASE in terminfo definition
            Self::Backspace => TerminalInputPayload::Single(char_to_ctrl_code(b'H')),
            Self::Escape => TerminalInputPayload::Single(0x1b),
            // https://vt100.net/docs/vt100-ug/chapter3.html
            // Table 3-6
            Self::ArrowRight => {
                if decckm_mode {
                    TerminalInputPayload::Many(b"\x1bOC")
                } else {
                    TerminalInputPayload::Many(b"\x1b[C")
                }
            }
            Self::ArrowLeft => {
                if decckm_mode {
                    TerminalInputPayload::Many(b"\x1bOD")
                } else {
                    TerminalInputPayload::Many(b"\x1b[D")
                }
            }
            Self::ArrowUp => {
                if decckm_mode {
                    TerminalInputPayload::Many(b"\x1bOA")
                } else {
                    TerminalInputPayload::Many(b"\x1b[A")
                }
            }
            Self::ArrowDown => {
                if decckm_mode {
                    TerminalInputPayload::Many(b"\x1bOB")
                } else {
                    TerminalInputPayload::Many(b"\x1b[B")
                }
            }
            Self::Home => {
                if decckm_mode {
                    TerminalInputPayload::Many(b"\x1bOH")
                } else {
                    TerminalInputPayload::Many(b"\x1b[H")
                }
            }
            Self::End => {
                if decckm_mode {
                    TerminalInputPayload::Many(b"\x1bOF")
                } else {
                    TerminalInputPayload::Many(b"\x1b[F")
                }
            }
            Self::KeyPad(c) => {
                if keypad_mode {
                    TerminalInputPayload::Single(*c)
                } else {
                    match c {
                        0 => TerminalInputPayload::Many(b"\x1b[Op"),
                        1 => TerminalInputPayload::Many(b"\x1b[Oq"),
                        2 => TerminalInputPayload::Many(b"\x1b[Or"),
                        3 => TerminalInputPayload::Many(b"\x1b[Os"),
                        4 => TerminalInputPayload::Many(b"\x1b[Ot"),
                        5 => TerminalInputPayload::Many(b"\x1b[Ou"),
                        6 => TerminalInputPayload::Many(b"\x1b[Ov"),
                        7 => TerminalInputPayload::Many(b"\x1b[Ow"),
                        8 => TerminalInputPayload::Many(b"\x1b[Ox"),
                        9 => TerminalInputPayload::Many(b"\x1b[Oy"),
                        b'-' => TerminalInputPayload::Many(b"\x1b[Om"),
                        b',' => TerminalInputPayload::Many(b"\x1b[Ol"),
                        b'.' => TerminalInputPayload::Many(b"\x1b[On"),
                        b'\n' => TerminalInputPayload::Many(b"\x1b[OM"),
                        _ => {
                            warn!("Unknown keypad key: {c}");
                            TerminalInputPayload::Single(*c)
                        }
                    }
                }
            }
            Self::Tab => TerminalInputPayload::Single(char_to_ctrl_code(b'i')),
            // Why \e[3~? It seems like we are emulating the vt510. Other terminals do it, so we
            // can too
            // https://web.archive.org/web/20160304024035/http://www.vt100.net/docs/vt510-rm/chapter8
            // https://en.wikipedia.org/wiki/Delete_character
            Self::Delete => TerminalInputPayload::Many(b"\x1b[3~"),
            Self::Insert => TerminalInputPayload::Many(b"\x1b[2~"),
            Self::PageUp => TerminalInputPayload::Many(b"\x1b[5~"),
            Self::PageDown => TerminalInputPayload::Many(b"\x1b[6~"),
            Self::LostFocus => TerminalInputPayload::Many(b"\x1b[O"),
            Self::InFocus => TerminalInputPayload::Many(b"\x1b[I"),
        }
    }
}

#[must_use]
pub fn split_format_data_for_scrollback(
    tags: Vec<FormatTag>,
    scrollback_split: usize,
    visible_end: usize,
    include_scrollback: bool,
) -> TerminalSections<Vec<FormatTag>> {
    let scrollback_tags = if include_scrollback {
        tags.iter()
            .filter(|tag| tag.start < scrollback_split)
            .cloned()
            .map(|mut tag| {
                tag.end = tag.end.min(scrollback_split);
                tag
            })
            .collect()
    } else {
        Vec::new()
    };

    let canvas_tags: Vec<FormatTag> = tags
        .into_iter()
        .filter(|tag| tag.end > scrollback_split && tag.end <= visible_end)
        .map(|mut tag| {
            tag.start = tag.start.saturating_sub(scrollback_split);
            if tag.end != usize::MAX {
                tag.end -= scrollback_split;
            }
            tag
        })
        .collect();

    TerminalSections {
        scrollback: scrollback_tags,
        visible: canvas_tags,
    }
}

pub struct TerminalEmulator<Io: FreminalTermInputOutput> {
    pub internal: TerminalState,
    _io: Io,
    write_tx: crossbeam_channel::Sender<PtyWrite>,
    ctx: Option<egui::Context>,
    previous_pass_valid: bool,
}

impl TerminalEmulator<DummyIo> {
    /// Creates a dummy terminal emulator for headless benchmarks or UI tests.
    ///
    /// This version skips PTY setup and I/O threads, initializing only the
    /// fields required for GUI rendering.
    #[must_use]
    pub fn dummy_for_bench() -> Self {
        use crossbeam_channel::unbounded;

        let (write_tx, _write_rx) = unbounded();

        Self {
            internal: TerminalState::default(),
            _io: DummyIo,
            write_tx,
            ctx: None,
            previous_pass_valid: false,
        }
    }
}

impl TerminalEmulator<FreminalPtyInputOutput> {
    /// Create a new terminal emulator
    ///
    /// # Errors
    ///
    pub fn new(args: &Args) -> Result<(Self, Receiver<PtyRead>)> {
        let (write_tx, read_rx) = unbounded();
        let (pty_tx, pty_rx) = unbounded();

        let io = FreminalPtyInputOutput::new(
            read_rx,
            pty_tx,
            args.recording.clone(),
            args.shell.clone(),
        )?;

        if let Err(e) = write_tx.send(PtyWrite::Resize(FreminalTerminalSize {
            width: DEFAULT_WIDTH as usize,
            height: DEFAULT_HEIGHT as usize,
            pixel_width: 0,
            pixel_height: 0,
        })) {
            error!("Failed to send resize to pty: {e}");
        }

        let ret = Self {
            internal: TerminalState::new(write_tx.clone()),
            _io: io,
            write_tx,
            ctx: None,
            previous_pass_valid: false,
        };
        Ok((ret, pty_rx))
    }
}

impl<Io: FreminalTermInputOutput> TerminalEmulator<Io> {
    pub fn get_cursor_visual_style(&self) -> CursorVisualStyle {
        self.internal.get_cursor_visual_style()
    }

    pub const fn set_mouse_position_from_move_event(&mut self, pos: &egui::Pos2) {
        self.internal.mouse_position = Some(*pos);
    }

    pub fn set_mouse_position(&mut self, pos: &Option<egui::Vec2>) {
        // info!("Setting mouse position: {pos:?}");
        self.internal.mouse_position = pos.map(|pos| egui::Pos2 {
            x: pos[0],
            y: pos[1],
        });
    }

    pub const fn get_mouse_position(&self) -> Option<egui::Pos2> {
        self.internal.mouse_position
    }

    pub fn is_mouse_hovered_on_url(&mut self, mouse_position: &CursorPos) -> Option<String> {
        self.internal.is_mouse_hovered_on_url(mouse_position)
    }

    pub fn set_window_focused(&mut self, focused: bool) {
        self.internal.set_window_focused(focused);

        if !focused {
            self.internal.mouse_position = None;
        }
    }

    pub fn set_egui_ctx_if_missing(&mut self, ctx: egui::Context) {
        if self.ctx.is_none() {
            self.ctx = Some(ctx.clone());
            self.internal.set_ctx(ctx);
        }
    }

    pub fn request_redraw(&mut self) {
        debug!("Terminal Emulator: Requesting redraw");
        self.previous_pass_valid = false;
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    pub const fn set_previous_pass_invalid(&mut self) {
        self.previous_pass_valid = false;
    }
    pub const fn set_previous_pass_valid(&mut self) {
        self.previous_pass_valid = true;
    }

    pub fn skip_draw_always(&self) -> bool {
        self.internal.skip_draw_always()
    }

    pub fn needs_redraw(&mut self) -> bool {
        let internal = if self.internal.is_changed() {
            self.internal.clear_changed();
            true
        } else {
            false
        };

        !self.previous_pass_valid || internal
    }

    pub const fn get_win_size(&mut self) -> (usize, usize) {
        self.internal.get_win_size()
    }

    /// Set the window title
    ///
    /// # Errors
    /// Will error if the terminal cannot be locked
    pub fn set_win_size(
        &mut self,
        width_chars: usize,
        height_chars: usize,
        font_pixel_width: usize,
        font_pixel_height: usize,
    ) -> Result<()> {
        let response = self.internal.set_win_size(width_chars, height_chars);

        if response.changed {
            self.write_tx.send(PtyWrite::Resize(FreminalTerminalSize {
                width: width_chars,
                height: height_chars,
                pixel_width: font_pixel_width,
                pixel_height: font_pixel_height,
            }))?;

            self.request_redraw();
        }

        Ok(())
    }

    /// Write to the terminal
    ///
    /// # Errors
    /// Will error if the terminal cannot be locked
    pub fn write(&self, to_write: &TerminalInput) -> Result<()> {
        self.internal.write(to_write)
    }

    pub fn data(&mut self, include_scrollback: bool) -> TerminalSections<Vec<TChar>> {
        self.internal.data(include_scrollback)
    }

    pub fn data_and_format_data_for_gui(
        &mut self,
    ) -> (
        TerminalSections<Vec<TChar>>,
        TerminalSections<Vec<FormatTag>>,
    ) {
        self.internal.data_and_format_data_for_gui()
    }

    pub const fn cursor_pos(&mut self) -> CursorPos {
        self.internal.cursor_pos()
    }

    pub fn show_cursor(&mut self) -> bool {
        self.internal.get_current_buffer().show_cursor == Dectcem::Show
            && self.internal.show_cursor()
    }
}
