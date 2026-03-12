// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::borrow::Cow;
use std::sync::Arc;

/// Cached flat representation of the visible window stored between snapshots.
///
/// Two separate `Arc<Vec<T>>` fields match the types in `TerminalSnapshot`
/// directly, so the clean path (no dirty rows) is a pair of refcount bumps
/// with no `Vec` allocation.
type VisibleSnap = Option<(Arc<Vec<TChar>>, Arc<Vec<FormatTag>>)>;

use crate::io::FreminalPtyInputOutput;
use crate::io::{FreminalTerminalSize, PtyRead, PtyWrite};
use crate::snapshot::TerminalSnapshot;
use crate::state::{data::TerminalSections, internal::TerminalState};
use anyhow::Result;
use crossbeam_channel::{Receiver, unbounded};

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
    #[allow(clippy::too_many_lines)]
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
    /// Kept alive for RAII (holds the terminfo `TempDir`).
    /// `None` in headless/benchmark mode where no PTY is started.
    _io: Option<FreminalPtyInputOutput>,
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
            _io: None,
            write_tx,
            previous_visible_snap: None,
            previous_was_alternate: false,
            gui_scroll_offset: 0,
            previous_scroll_offset: 0,
        }
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
            internal: TerminalState::new(write_tx.clone(), scrollback_limit),
            _io: Some(io),
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

    #[must_use]
    pub fn get_cursor_visual_style(&self) -> CursorVisualStyle {
        self.internal.get_cursor_visual_style()
    }

    pub const fn is_mouse_hovered_on_url(&mut self, mouse_position: &CursorPos) -> Option<String> {
        self.internal.is_mouse_hovered_on_url(mouse_position)
    }

    #[must_use]
    pub fn skip_draw_always(&self) -> bool {
        self.internal.skip_draw_always()
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
        self.internal.set_win_size(width_chars, height_chars);

        if old_width != width_chars || old_height != height_chars {
            self.write_tx.send(PtyWrite::Resize(FreminalTerminalSize {
                width: width_chars,
                height: height_chars,
                pixel_width: font_pixel_width,
                pixel_height: font_pixel_height,
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
        self.internal.set_win_size(width_chars, height_chars);

        if let Err(e) = self.write_tx.send(PtyWrite::Resize(FreminalTerminalSize {
            width: width_chars,
            height: height_chars,
            pixel_width: font_pixel_width,
            pixel_height: font_pixel_height,
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
        if scroll_offset != self.previous_scroll_offset {
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
        let cursor_pos = self.internal.cursor_pos();
        // Hide the cursor when the user is scrolled back into history —
        // the live cursor line is not visible on screen.
        let show_cursor = self.internal.show_cursor() && scroll_offset == 0;
        let cursor_visual_style = self.internal.get_cursor_visual_style();
        let is_normal_display = self.internal.is_normal_display();

        let bracketed_paste = self.internal.modes.bracketed_paste.clone();
        let mouse_tracking = self.internal.modes.mouse_tracking.clone();
        let repeat_keys = self.internal.should_repeat_keys();
        let cursor_key_app_mode = {
            use freminal_common::buffer_states::modes::decckm::Decckm;
            self.internal.get_cursor_key_mode() == Decckm::Application
        };
        let skip_draw = self.internal.skip_draw_always();
        let cwd = self
            .internal
            .handler
            .current_working_directory()
            .map(String::from);

        let ftcs_state = self.internal.handler.ftcs_state();
        let last_exit_code = self.internal.handler.last_exit_code();

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
            content_changed,
            bracketed_paste,
            mouse_tracking,
            repeat_keys,
            cursor_key_app_mode,
            skip_draw,
            cwd,
            ftcs_state,
            last_exit_code,
        }
    }
}
