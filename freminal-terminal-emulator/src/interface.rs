// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

/// Cached flat representation of the visible window stored between snapshots.
///
/// Two separate `Arc<Vec<T>>` fields match the types in `TerminalSnapshot`
/// directly, so the clean path (no dirty rows) is a pair of refcount bumps
/// with no `Vec` allocation.
type VisibleSnap = Option<(Arc<Vec<TChar>>, Arc<Vec<FormatTag>>)>;

/// Image data collected from the visible window for a snapshot.
///
/// First element: map of referenced images (keyed by ID).
/// Second element: per-cell placement vector (parallel to `visible_chars`).
type VisibleImages = (
    Arc<HashMap<u64, InlineImage>>,
    Arc<Vec<Option<ImagePlacement>>>,
);

use crate::io::FreminalPtyInputOutput;
use crate::io::{FreminalTerminalSize, PtyRead, PtyWrite};
use crate::snapshot::TerminalSnapshot;
use crate::state::{data::TerminalSections, internal::TerminalState};
use anyhow::Result;
use crossbeam_channel::{Receiver, unbounded};
use freminal_buffer::image_store::{ImagePlacement, InlineImage};

use freminal_common::buffer_states::cursor::CursorPos;
use freminal_common::buffer_states::format_tag::FormatTag;
use freminal_common::buffer_states::modes::{
    mouse::MouseEncoding, mouse::MouseTrack, rl_bracket::RlBracket,
};

use freminal_common::{
    args::Args, buffer_states::tchar::TChar, cursor::CursorVisualStyle,
    terminal_size::DEFAULT_HEIGHT, terminal_size::DEFAULT_WIDTH,
};

/// Mode-related fields extracted from the emulator state for a snapshot.
///
/// Factored out so `build_snapshot` stays within Clippy's 100-line limit.
#[allow(clippy::struct_excessive_bools)]
struct SnapshotModeFields {
    bracketed_paste: RlBracket,
    mouse_tracking: MouseTrack,
    mouse_encoding: MouseEncoding,
    repeat_keys: bool,
    cursor_key_app_mode: bool,
    keypad_app_mode: bool,
    skip_draw: bool,
    modify_other_keys: u8,
    application_escape_key: bool,
    backarrow_sends_bs: bool,
}

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

/// Modifier key state for xterm-style modified key encoding.
///
/// When any modifier is set, special keys (arrows, Home/End, function keys,
/// Insert/Delete/PageUp/PageDown) produce the xterm `CSI 1 ; Nm <final>`
/// form where N encodes the modifier combination:
///
/// | N | Modifiers       |
/// |---|-----------------|
/// | 2 | Shift           |
/// | 3 | Alt             |
/// | 4 | Shift+Alt       |
/// | 5 | Ctrl            |
/// | 6 | Ctrl+Shift      |
/// | 7 | Ctrl+Alt        |
/// | 8 | Ctrl+Alt+Shift  |
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl KeyModifiers {
    /// No modifiers held.
    pub const NONE: Self = Self {
        shift: false,
        ctrl: false,
        alt: false,
    };

    /// Returns `true` when no modifier is held.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        !self.shift && !self.ctrl && !self.alt
    }

    /// Compute the xterm modifier parameter (2–8), or `None` if no modifier
    /// is held.
    ///
    /// Encoding: `1 + (shift ? 1 : 0) + (alt ? 2 : 0) + (ctrl ? 4 : 0)`
    #[must_use]
    pub const fn modifier_param(self) -> Option<u8> {
        if self.is_empty() {
            return None;
        }
        let mut n: u8 = 1;
        if self.shift {
            n += 1;
        }
        if self.alt {
            n += 2;
        }
        if self.ctrl {
            n += 4;
        }
        Some(n)
    }
}

#[derive(Eq, PartialEq, Debug)]
pub enum TerminalInputPayload {
    Single(u8),
    Many(&'static [u8]),
    /// Variable-length payload for modified key sequences that cannot be
    /// represented as static byte slices.
    Owned(Vec<u8>),
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
    ArrowRight(KeyModifiers),
    ArrowLeft(KeyModifiers),
    ArrowUp(KeyModifiers),
    ArrowDown(KeyModifiers),
    Home(KeyModifiers),
    End(KeyModifiers),
    Delete(KeyModifiers),
    Insert(KeyModifiers),
    PageUp(KeyModifiers),
    PageDown(KeyModifiers),
    Tab,
    Escape,
    InFocus,
    LostFocus,
    KeyPad(u8),
    // Function keys F1–F12
    FunctionKey(u8, KeyModifiers),
}

/// Build an xterm-style modified key sequence: `ESC [ 1 ; <mod> <final>`.
///
/// Used for arrow keys and Home/End when a modifier is held.
fn modified_csi_final(modifier: u8, final_byte: u8) -> TerminalInputPayload {
    TerminalInputPayload::Owned(format!("\x1b[1;{modifier}{}", final_byte as char).into_bytes())
}

/// Build an xterm-style modified tilde key sequence: `ESC [ <code> ; <mod> ~`.
///
/// Used for Insert/Delete/PageUp/PageDown and F5–F12 when a modifier is held.
fn modified_csi_tilde(code: u8, modifier: u8) -> TerminalInputPayload {
    TerminalInputPayload::Owned(format!("\x1b[{code};{modifier}~").into_bytes())
}

impl TerminalInput {
    #[must_use]
    #[allow(clippy::too_many_lines, clippy::fn_params_excessive_bools)]
    pub fn to_payload(
        &self,
        decckm_mode: bool,
        keypad_mode: bool,
        modify_other_keys: u8,
        application_escape_key: bool,
        backarrow_sends_bs: bool,
    ) -> TerminalInputPayload {
        match self {
            Self::Ascii(c) => TerminalInputPayload::Single(*c),
            Self::Ctrl(c) => {
                // modifyOtherKeys level 2: encode Ctrl+<letter> as
                // CSI 27 ; 5 ; <ASCII code of letter> ~
                // instead of the traditional C0 control code.
                if modify_other_keys >= 2 {
                    let code = u32::from(c.to_ascii_uppercase());
                    TerminalInputPayload::Owned(format!("\x1b[27;5;{code}~").into_bytes())
                } else {
                    TerminalInputPayload::Single(char_to_ctrl_code(*c))
                }
            }
            // Sending bare '\n' causes misbehaviour in full-screen TUI programs (nvim,
            // lazygit) which expect CR (0x0d) for the Enter key.  Interactive shells
            // handle '\n' fine, but the POSIX tty layer translates CR→NL on input when
            // ICRNL is set, so sending CR is correct for both cases.
            // TODO: investigate further — the tty driver should be handling this.
            Self::Enter => TerminalInputPayload::Single(char_to_ctrl_code(b'm')),
            Self::LineFeed => TerminalInputPayload::Single(b'\n'),
            // DECBKM (?67): set → BS (0x08), reset → DEL (0x7F).
            Self::Backspace => {
                if backarrow_sends_bs {
                    TerminalInputPayload::Single(char_to_ctrl_code(b'H'))
                } else {
                    TerminalInputPayload::Single(0x7F)
                }
            }
            Self::Escape => {
                // Mode 7727 (Application Escape Key): send CSI 27 ; 1 ; 27 ~
                // instead of bare ESC so tmux can instantly distinguish the
                // Escape key from the start of an escape sequence.
                if application_escape_key {
                    TerminalInputPayload::Owned(b"\x1b[27;1;27~".to_vec())
                } else {
                    TerminalInputPayload::Single(0x1b)
                }
            }
            // https://vt100.net/docs/vt100-ug/chapter3.html
            // Table 3-6
            //
            // When modifiers are held, always use CSI form (not SS3) even in
            // DECCKM mode — xterm convention.
            Self::ArrowRight(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'C'),
                None if decckm_mode => TerminalInputPayload::Many(b"\x1bOC"),
                None => TerminalInputPayload::Many(b"\x1b[C"),
            },
            Self::ArrowLeft(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'D'),
                None if decckm_mode => TerminalInputPayload::Many(b"\x1bOD"),
                None => TerminalInputPayload::Many(b"\x1b[D"),
            },
            Self::ArrowUp(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'A'),
                None if decckm_mode => TerminalInputPayload::Many(b"\x1bOA"),
                None => TerminalInputPayload::Many(b"\x1b[A"),
            },
            Self::ArrowDown(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'B'),
                None if decckm_mode => TerminalInputPayload::Many(b"\x1bOB"),
                None => TerminalInputPayload::Many(b"\x1b[B"),
            },
            Self::Home(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'H'),
                None if decckm_mode => TerminalInputPayload::Many(b"\x1bOH"),
                None => TerminalInputPayload::Many(b"\x1b[H"),
            },
            Self::End(mods) => match mods.modifier_param() {
                Some(m) => modified_csi_final(m, b'F'),
                None if decckm_mode => TerminalInputPayload::Many(b"\x1bOF"),
                None => TerminalInputPayload::Many(b"\x1b[F"),
            },
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
            Self::Delete(mods) => mods
                .modifier_param()
                .map_or(TerminalInputPayload::Many(b"\x1b[3~"), |m| {
                    modified_csi_tilde(3, m)
                }),
            Self::Insert(mods) => mods
                .modifier_param()
                .map_or(TerminalInputPayload::Many(b"\x1b[2~"), |m| {
                    modified_csi_tilde(2, m)
                }),
            Self::PageUp(mods) => mods
                .modifier_param()
                .map_or(TerminalInputPayload::Many(b"\x1b[5~"), |m| {
                    modified_csi_tilde(5, m)
                }),
            Self::PageDown(mods) => mods
                .modifier_param()
                .map_or(TerminalInputPayload::Many(b"\x1b[6~"), |m| {
                    modified_csi_tilde(6, m)
                }),
            Self::LostFocus => TerminalInputPayload::Many(b"\x1b[O"),
            Self::InFocus => TerminalInputPayload::Many(b"\x1b[I"),
            // https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-PC-Style-Function-Keys
            //
            // F1–F4 use SS3 form without modifiers, CSI form with modifiers.
            // F5–F12 use CSI tilde form, with modifier inserted before `~`.
            Self::FunctionKey(n, mods) => {
                let mod_param = mods.modifier_param();
                match (n, mod_param) {
                    // F1–F4 with modifiers: CSI 1;Nm P/Q/R/S
                    (1, Some(m)) => modified_csi_final(m, b'P'),
                    (2, Some(m)) => modified_csi_final(m, b'Q'),
                    (3, Some(m)) => modified_csi_final(m, b'R'),
                    (4, Some(m)) => modified_csi_final(m, b'S'),
                    // F1–F4 without modifiers: SS3 P/Q/R/S
                    (1, None) => TerminalInputPayload::Many(b"\x1bOP"),
                    (2, None) => TerminalInputPayload::Many(b"\x1bOQ"),
                    (3, None) => TerminalInputPayload::Many(b"\x1bOR"),
                    (4, None) => TerminalInputPayload::Many(b"\x1bOS"),
                    // F5–F12 with modifiers: CSI code;Nm ~
                    (5, Some(m)) => modified_csi_tilde(15, m),
                    (6, Some(m)) => modified_csi_tilde(17, m),
                    (7, Some(m)) => modified_csi_tilde(18, m),
                    (8, Some(m)) => modified_csi_tilde(19, m),
                    (9, Some(m)) => modified_csi_tilde(20, m),
                    (10, Some(m)) => modified_csi_tilde(21, m),
                    (11, Some(m)) => modified_csi_tilde(23, m),
                    (12, Some(m)) => modified_csi_tilde(24, m),
                    // F5–F12 without modifiers
                    (5, None) => TerminalInputPayload::Many(b"\x1b[15~"),
                    (6, None) => TerminalInputPayload::Many(b"\x1b[17~"),
                    (7, None) => TerminalInputPayload::Many(b"\x1b[18~"),
                    (8, None) => TerminalInputPayload::Many(b"\x1b[19~"),
                    (9, None) => TerminalInputPayload::Many(b"\x1b[20~"),
                    (10, None) => TerminalInputPayload::Many(b"\x1b[21~"),
                    (11, None) => TerminalInputPayload::Many(b"\x1b[23~"),
                    (12, None) => TerminalInputPayload::Many(b"\x1b[24~"),
                    _ => {
                        warn!("Unhandled function key: F{n}");
                        TerminalInputPayload::Many(b"")
                    }
                }
            }
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

pub struct TerminalEmulator {
    pub internal: TerminalState,
    /// PTY I/O layer (holds the terminfo `TempDir` and child-exit receiver).
    /// `None` in headless/benchmark mode where no PTY is started.
    pty_io: Option<FreminalPtyInputOutput>,
    write_tx: crossbeam_channel::Sender<PtyWrite>,
    /// Cached flat representation of the visible window from the last
    /// `build_snapshot` call.  `None` until the first snapshot is built.
    ///
    /// Stored as two separate `Arc<Vec<T>>` matching the types in
    /// `TerminalSnapshot`, so the clean path (no dirty rows) hands them
    /// directly into the snapshot with a refcount bump — no Vec allocation.
    previous_visible_snap: VisibleSnap,
    /// Whether the previous snapshot was taken while in the alternate screen
    /// buffer.  Used to detect primary↔alternate transitions and invalidate
    /// `previous_visible_snap` so stale content is never reused across a
    /// buffer switch.
    previous_was_alternate: bool,
    /// Scroll offset requested by the GUI (rows from the bottom, 0 = live).
    ///
    /// Updated when an `InputEvent::ScrollOffset(n)` is received.  Reset to 0
    /// when new PTY output arrives (auto-scroll to bottom).
    gui_scroll_offset: usize,
    /// The scroll offset used for the previous snapshot.  When this differs
    /// from the current `gui_scroll_offset`, the visible window has moved and
    /// the cached snapshot must be invalidated.
    previous_scroll_offset: usize,
}

impl TerminalEmulator {
    /// Creates a headless terminal emulator for benchmarks or tests.
    ///
    /// This version skips PTY setup and I/O threads, initializing only the
    /// fields required for data processing and snapshot building.
    #[must_use]
    pub fn dummy_for_bench() -> Self {
        use crossbeam_channel::unbounded;

        let (write_tx, _write_rx) = unbounded();

        Self {
            internal: TerminalState::default(),
            pty_io: None,
            write_tx,
            previous_visible_snap: None,
            previous_was_alternate: false,
            gui_scroll_offset: 0,
            previous_scroll_offset: 0,
        }
    }

    /// Creates a headless terminal emulator for playback mode.
    ///
    /// No PTY is spawned.  The returned `Receiver<PtyWrite>` drains any
    /// escape-sequence responses that the emulator's handler sends (DA, CPR,
    /// etc.) so channels never block.  The caller feeds recorded data via
    /// `handle_incoming_data`.
    #[must_use]
    pub fn new_for_playback(scrollback_limit: Option<usize>) -> (Self, Receiver<PtyWrite>) {
        use crossbeam_channel::unbounded;

        let (write_tx, write_rx) = unbounded();

        let emulator = Self {
            internal: TerminalState::new(write_tx.clone(), scrollback_limit),
            pty_io: None,
            write_tx,
            previous_visible_snap: None,
            previous_was_alternate: false,
            gui_scroll_offset: 0,
            previous_scroll_offset: 0,
        };
        (emulator, write_rx)
    }

    /// Create a new terminal emulator
    ///
    /// `scrollback_limit` overrides the default scrollback history size when
    /// `Some(n)` is provided.  `None` keeps the compiled-in default (4000).
    ///
    /// # Errors
    ///
    pub fn new(args: &Args, scrollback_limit: Option<usize>) -> Result<(Self, Receiver<PtyRead>)> {
        let (write_tx, read_rx) = unbounded();
        let (pty_tx, pty_rx) = unbounded();

        // Derive the command tuple from the positional `command` arg.
        // If `command` is non-empty, it takes precedence over `--shell`.
        let command = if args.command.is_empty() {
            None
        } else {
            let mut iter = args.command.iter().cloned();
            // SAFETY: we just checked `is_empty()` above; first element exists.
            let prog = iter.next().unwrap_or_default();
            Some((prog, iter.collect()))
        };

        // When a positional command is specified, shell is ignored.
        let shell = if command.is_some() {
            None
        } else {
            args.shell.clone()
        };

        let io =
            FreminalPtyInputOutput::new(read_rx, pty_tx, args.recording.clone(), command, shell)?;

        if let Err(e) = write_tx.send(PtyWrite::Resize(FreminalTerminalSize {
            width: DEFAULT_WIDTH as usize,
            height: DEFAULT_HEIGHT as usize,
            pixel_width: 0,
            pixel_height: 0,
        })) {
            error!("Failed to send resize to pty: {e}");
        }

        let ret = Self {
            internal: TerminalState::new(write_tx.clone(), scrollback_limit),
            pty_io: Some(io),
            write_tx,
            previous_visible_snap: None,
            previous_was_alternate: false,
            gui_scroll_offset: 0,
            previous_scroll_offset: 0,
        };
        Ok((ret, pty_rx))
    }

    /// Return a clone of the PTY write sender.
    ///
    /// Used by `main.rs` to pass the real write channel to the GUI before the
    /// emulator is moved into the PTY consumer thread.  The GUI uses it to
    /// send `PtyWrite::Write` responses for Report* window manipulation
    /// commands without going through the emulator lock.
    #[must_use]
    pub fn clone_write_tx(&self) -> crossbeam_channel::Sender<PtyWrite> {
        self.write_tx.clone()
    }

    /// Return the child-exit receiver from the PTY I/O layer.
    ///
    /// Returns `Some(Receiver<()>)` in normal mode (where a real PTY child
    /// process exists) or `None` in headless/benchmark/playback mode.
    ///
    /// Used by `main.rs` to add a third arm to the `select!` loop so the
    /// consumer thread can detect child exit on platforms (Windows) where the
    /// PTY read pipe does not close when the child exits.
    #[must_use]
    pub fn child_exit_rx(&self) -> Option<crossbeam_channel::Receiver<()>> {
        self.pty_io.as_ref().map(|io| io.child_exit_rx.clone())
    }

    #[must_use]
    pub fn get_cursor_visual_style(&self) -> CursorVisualStyle {
        self.internal.get_cursor_visual_style()
    }

    #[must_use]
    pub fn skip_draw_always(&self) -> bool {
        self.internal.skip_draw_always()
    }

    /// Extract text from the full buffer for a selection range.
    ///
    /// Coordinates are buffer-absolute row indices and 0-indexed columns.
    /// Delegates to `Buffer::extract_text`.
    #[must_use]
    pub fn extract_selection_text(
        &self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) -> String {
        self.internal
            .handler
            .buffer()
            .extract_text(start_row, start_col, end_row, end_col)
    }

    /// Process a chunk of raw PTY bytes.
    ///
    /// This wraps `TerminalState::handle_incoming_data` for the consumer thread.
    /// When the user is scrolled back (`gui_scroll_offset > 0`), new output
    /// auto-scrolls to the bottom by resetting the offset to 0.
    pub fn handle_incoming_data(&mut self, incoming: &[u8]) {
        self.internal.handle_incoming_data(incoming);
        // Auto-scroll to bottom on new output, matching standard terminal
        // behavior.  The next snapshot will carry scroll_offset = 0 so the
        // GUI's ViewState is synced automatically.
        if self.gui_scroll_offset > 0 {
            self.gui_scroll_offset = 0;
        }
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
        let (old_width, old_height) = self.internal.get_win_size();
        #[allow(clippy::cast_possible_truncation)]
        self.internal.set_win_size(
            width_chars,
            height_chars,
            font_pixel_width as u32,
            font_pixel_height as u32,
        );

        if old_width != width_chars || old_height != height_chars {
            // TIOCGWINSZ expects total window pixel dimensions, not per-cell.
            self.write_tx.send(PtyWrite::Resize(FreminalTerminalSize {
                width: width_chars,
                height: height_chars,
                pixel_width: font_pixel_width.saturating_mul(width_chars),
                pixel_height: font_pixel_height.saturating_mul(height_chars),
            }))?;
        }

        Ok(())
    }

    /// Handle a resize event delivered via the `InputEvent` channel.
    ///
    /// This is called by the PTY consumer thread (or, currently, the inline
    /// `input_rx` receiver in `main.rs`) when the GUI detects a change in
    /// terminal dimensions.  It updates the emulator's internal size and
    /// forwards a `PtyWrite::Resize` to the PTY writer so the kernel's tty
    /// layer sees the new window size.
    ///
    /// Unlike `set_win_size`, this method does not need to return `Result`
    /// because send failures are logged rather than propagated — the caller
    /// is on the consumer thread which has no caller to propagate to.
    pub fn handle_resize_event(
        &mut self,
        width_chars: usize,
        height_chars: usize,
        font_pixel_width: usize,
        font_pixel_height: usize,
    ) {
        #[allow(clippy::cast_possible_truncation)]
        self.internal.set_win_size(
            width_chars,
            height_chars,
            font_pixel_width as u32,
            font_pixel_height as u32,
        );

        // The PTY's TIOCGWINSZ expects the *total* window pixel dimensions
        // (ws_xpixel, ws_ypixel), not per-cell sizes.  Applications like nvim
        // compute cell size as ws_xpixel/ws_col, so passing per-cell values here
        // would give them a near-zero cell width.
        if let Err(e) = self.write_tx.send(PtyWrite::Resize(FreminalTerminalSize {
            width: width_chars,
            height: height_chars,
            pixel_width: font_pixel_width.saturating_mul(width_chars),
            pixel_height: font_pixel_height.saturating_mul(height_chars),
        })) {
            error!("Failed to send resize to PTY: {e}");
        }
    }

    /// Update the GUI-requested scroll offset.
    ///
    /// Called by the PTY consumer thread when it receives
    /// `InputEvent::ScrollOffset(n)`.  The value is clamped to
    /// `max_scroll_offset()` during the next `build_snapshot()` call.
    pub const fn set_gui_scroll_offset(&mut self, offset: usize) {
        self.gui_scroll_offset = offset;
    }

    /// Reset the scroll offset to 0 (live bottom).
    ///
    /// Called when new PTY data arrives while the user is scrolled back.
    pub const fn reset_scroll_offset(&mut self) {
        self.gui_scroll_offset = 0;
    }

    /// Write to the terminal
    ///
    /// # Errors
    /// Will error if the terminal cannot be locked
    pub fn write(&self, to_write: &TerminalInput) -> Result<()> {
        self.internal.write(to_write)
    }

    /// Write raw bytes directly to the PTY write channel.
    ///
    /// Used by the PTY consumer thread to forward keyboard input bytes that
    /// arrived via `InputEvent::Key(bytes)` without re-encoding them through
    /// `TerminalInput`.
    ///
    /// # Errors
    /// Returns an error if the send to the PTY write channel fails.
    pub fn write_raw_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.write_tx
            .send(PtyWrite::Write(bytes.to_vec()))
            .map_err(|e| anyhow::anyhow!("Failed to send raw bytes to PTY: {e}"))
    }

    pub fn data(&mut self, include_scrollback: bool) -> TerminalSections<Vec<TChar>> {
        let (chars, _tags) = self.internal.handler.data_and_format_data_for_gui(0);
        if include_scrollback {
            chars
        } else {
            TerminalSections {
                scrollback: vec![],
                visible: chars.visible,
            }
        }
    }

    pub fn data_and_format_data_for_gui(
        &mut self,
    ) -> (
        TerminalSections<Vec<TChar>>,
        TerminalSections<Vec<FormatTag>>,
    ) {
        self.internal.data_and_format_data_for_gui(0)
    }

    pub fn cursor_pos(&mut self) -> CursorPos {
        self.internal.cursor_pos()
    }

    pub const fn show_cursor(&mut self) -> bool {
        self.internal.show_cursor()
    }

    /// Build a point-in-time snapshot of the terminal state.
    ///
    /// This is cheap to call: the visible content is flattened here on the
    /// PTY thread so the GUI render path never has to do it.
    ///
    /// `content_changed` is `true` only when the visible flat content differs
    /// from the previous snapshot.  Cursor-only moves do not set it because
    /// cursor position is carried separately in the snapshot struct.
    #[must_use]
    pub fn build_snapshot(&mut self) -> TerminalSnapshot {
        // ── Cheap immutable reads (no &mut borrow of handler needed) ────────
        let (term_width, term_height) = self.internal.handler.get_win_size();
        let is_alternate_screen = self.internal.handler.is_alternate_screen();

        // On the alternate screen scrollback is meaningless — clamp to 0.
        let (scroll_offset, max_scroll_offset) = if is_alternate_screen {
            (0, 0)
        } else {
            // Clamp to the maximum scrollback offset so an out-of-range value
            // (e.g. from a previous buffer state) doesn't panic.
            let max = self.internal.handler.buffer().max_scroll_offset();
            (self.gui_scroll_offset.min(max), max)
        };

        // ── Invalidate the snap cache on primary ↔ alternate screen switch ───
        //
        // When the buffer type changes, the previous visible_snap belongs to
        // the other buffer and must never be reused for the new one.
        if is_alternate_screen != self.previous_was_alternate {
            self.previous_visible_snap = None;
            self.previous_was_alternate = is_alternate_screen;
        }

        // ── Invalidate the snap cache when scroll offset changes ─────────
        //
        // The visible window moved — the cached flat content is from a
        // different set of rows and must not be reused.
        let scroll_changed = scroll_offset != self.previous_scroll_offset;
        if scroll_changed {
            self.previous_visible_snap = None;
            self.previous_scroll_offset = scroll_offset;
        }

        // ── Determine whether any visible row changed since last snapshot ────
        let any_dirty = self.internal.handler.any_visible_dirty(scroll_offset);

        // ── Produce (visible_chars, visible_tags, content_changed) ───────
        let (visible_chars, visible_tags, content_changed) = if any_dirty {
            // At least one visible row is dirty — re-flatten via the cache.
            // `data_and_format_data_for_gui` calls `visible_as_tchars_and_tags`
            // which updates the per-row cache and clears dirty flags in one pass.
            let (chars, tags) = self.internal.data_and_format_data_for_gui(scroll_offset);
            let vc = Arc::new(chars.visible);
            let vt = Arc::new(tags.visible);

            // `content_changed` is true when the flat content actually differs
            // from the previous snapshot (guards against spurious redraws from
            // dirty flags set on rows that were ultimately written with the same
            // bytes, e.g. cursor-blink redraws).
            let changed = self
                .previous_visible_snap
                .as_ref()
                .is_none_or(|(prev_chars, _)| prev_chars.as_ref() != vc.as_ref());

            self.previous_visible_snap = Some((Arc::clone(&vc), Arc::clone(&vt)));
            (vc, vt, changed)
        } else if let Some((prev_chars, prev_tags)) = &self.previous_visible_snap {
            // No visible row is dirty — reuse cached Arcs.
            // This is a refcount bump only: no Vec allocation, no memcpy.
            (Arc::clone(prev_chars), Arc::clone(prev_tags), false)
        } else {
            // First-ever snapshot and nothing is marked dirty yet (e.g. the
            // buffer was just created).  Flatten once to populate the cache.
            let (chars, tags) = self.internal.data_and_format_data_for_gui(scroll_offset);
            let vc = Arc::new(chars.visible);
            let vt = Arc::new(tags.visible);
            self.previous_visible_snap = Some((Arc::clone(&vc), Arc::clone(&vt)));
            (vc, vt, true)
        };

        // ── Remaining cheap reads ────────────────────────────────────────────
        let mode_fields = self.collect_mode_fields();
        let cursor_pos = self.internal.cursor_pos();
        // Hide the cursor when the user is scrolled back into history —
        // the live cursor line is not visible on screen.
        let show_cursor = self.internal.show_cursor() && scroll_offset == 0;
        let cursor_visual_style = self.internal.get_cursor_visual_style();
        let is_normal_display = self.internal.is_normal_display();

        let cwd = self
            .internal
            .handler
            .current_working_directory()
            .map(String::from);

        let ftcs_state = self.internal.handler.ftcs_state();
        let last_exit_code = self.internal.handler.last_exit_code();
        let theme = self.internal.handler.theme();

        // ── Inline image data ────────────────────────────────────────────────
        let (images, visible_image_placements) = self.collect_visible_images(scroll_offset);

        let total_rows = self.internal.handler.buffer().get_rows().len();

        TerminalSnapshot {
            visible_chars,
            visible_tags,
            scroll_offset,
            max_scroll_offset,
            height: term_height,
            cursor_pos,
            show_cursor,
            cursor_visual_style,
            is_alternate_screen,
            is_normal_display,
            term_width,
            term_height,
            total_rows,
            content_changed,
            scroll_changed,
            bracketed_paste: mode_fields.bracketed_paste,
            mouse_tracking: mode_fields.mouse_tracking,
            mouse_encoding: mode_fields.mouse_encoding,
            repeat_keys: mode_fields.repeat_keys,
            cursor_key_app_mode: mode_fields.cursor_key_app_mode,
            keypad_app_mode: mode_fields.keypad_app_mode,
            skip_draw: mode_fields.skip_draw,
            modify_other_keys: mode_fields.modify_other_keys,
            application_escape_key: mode_fields.application_escape_key,
            backarrow_sends_bs: mode_fields.backarrow_sends_bs,
            cwd,
            ftcs_state,
            last_exit_code,
            theme,
            images,
            visible_image_placements,
            playback_info: None,
            cursor_color_override: self.internal.handler.cursor_color_override(),
        }
    }

    /// Collect all mode flags needed by the snapshot in a single pass.
    fn collect_mode_fields(&self) -> SnapshotModeFields {
        use freminal_common::buffer_states::modes::{
            decbkm::Decbkm, decckm::Decckm, keypad::KeypadMode,
        };

        SnapshotModeFields {
            bracketed_paste: self.internal.modes.bracketed_paste.clone(),
            mouse_tracking: self.internal.modes.mouse_tracking.clone(),
            mouse_encoding: self.internal.modes.mouse_encoding.clone(),
            repeat_keys: self.internal.should_repeat_keys(),
            cursor_key_app_mode: self.internal.get_cursor_key_mode() == Decckm::Application,
            keypad_app_mode: self.internal.modes.keypad_mode == KeypadMode::Application,
            skip_draw: self.internal.skip_draw_always(),
            modify_other_keys: self.internal.handler.modify_other_keys_level(),
            application_escape_key: self.internal.handler.application_escape_key(),
            backarrow_sends_bs: self.internal.modes.backarrow_key_mode == Decbkm::BackarrowSendsBs,
        }
    }

    /// Build the image map and placement vector for the visible window.
    ///
    /// Returns `(images, placements)` — both wrapped in `Arc` for cheap
    /// snapshot cloning.  The common case (no images) returns empty containers
    /// with zero allocation.
    fn collect_visible_images(&self, scroll_offset: usize) -> VisibleImages {
        if !self.internal.handler.has_visible_images(scroll_offset) {
            return (Arc::new(HashMap::new()), Arc::new(Vec::new()));
        }

        let placements = self
            .internal
            .handler
            .visible_image_placements(scroll_offset);

        // Collect only the images actually referenced by a visible cell so the
        // snapshot doesn't grow without bound.
        let mut img_map: HashMap<u64, InlineImage> = HashMap::new();
        for placement in placements.iter().flatten() {
            let id = placement.image_id;
            if let std::collections::hash_map::Entry::Vacant(entry) = img_map.entry(id)
                && let Some(img) = self.internal.handler.buffer().image_store().get(id)
            {
                entry.insert(img.clone());
            }
        }

        (Arc::new(img_map), Arc::new(placements))
    }
}
