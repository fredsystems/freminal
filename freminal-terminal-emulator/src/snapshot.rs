// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `TerminalSnapshot` — the lock-free data contract between the PTY processing
//! thread and the GUI thread.
//!
//! The PTY thread produces a fresh snapshot after every batch of processed data
//! and publishes it atomically via an `ArcSwap<TerminalSnapshot>`.  The GUI
//! thread loads the snapshot with a single atomic pointer load — no lock, no
//! blocking.

use std::sync::Arc;

use freminal_common::{
    buffer_states::{
        cursor::CursorPos,
        format_tag::FormatTag,
        ftcs::FtcsState,
        modes::{mouse::MouseTrack, rl_bracket::RlBracket},
        tchar::TChar,
    },
    cursor::CursorVisualStyle,
    themes::ThemePalette,
};

/// A point-in-time snapshot of the terminal state, ready for the GUI to render.
///
/// All expensive work (flattening rows → `Vec<TChar>` / `Vec<FormatTag>`) is
/// performed on the PTY thread so the GUI render path is allocation-free.
///
/// The snapshot is always immutable once constructed.  The GUI must never
/// mutate any field.
///
/// `visible_chars` and `visible_tags` are wrapped in `Arc` so that cloning a
/// snapshot (or handing the same content to a second snapshot on the clean
/// path) is a cheap atomic refcount increment rather than a full `Vec` copy.
#[allow(clippy::struct_excessive_bools)] // Four independent semantic flags; enums would add noise
#[derive(Debug, Clone)]
pub struct TerminalSnapshot {
    /// Flattened visible character content, already converted from `Row`/`Cell`.
    ///
    /// Produced once on the PTY side; the GUI reads it directly.
    ///
    /// Wrapped in `Arc` so passing the same content to a new snapshot (clean
    /// path — no visible rows changed) is a refcount bump, not a Vec copy.
    pub visible_chars: Arc<Vec<TChar>>,

    /// Format tags corresponding to `visible_chars`.
    ///
    /// Wrapped in `Arc` for the same reason as `visible_chars`.
    pub visible_tags: Arc<Vec<FormatTag>>,

    /// Current scroll offset (rows from the bottom, 0 = live view).
    ///
    /// The GUI reads this to stay in sync with the PTY thread's view.  When
    /// the PTY thread auto-scrolls to bottom on new output, this will be 0
    /// even if the GUI previously sent a non-zero offset.
    pub scroll_offset: usize,

    /// Maximum valid scroll offset (total scrollback rows above the visible
    /// window).  Used by the GUI to compute the scrollbar thumb position and
    /// size.  When `max_scroll_offset == 0` there is no scrollback history.
    pub max_scroll_offset: usize,

    /// Height of the visible window in rows.
    pub height: usize,

    /// Cursor position in screen coordinates (0-indexed, relative to the top
    /// of the visible window).
    pub cursor_pos: CursorPos,

    /// Whether the cursor should be painted.
    pub show_cursor: bool,

    /// Current cursor shape / blink style.
    pub cursor_visual_style: CursorVisualStyle,

    /// `true` when the alternate screen buffer is currently active.
    pub is_alternate_screen: bool,

    /// `true` when the display is in normal (non-inverted) mode.
    pub is_normal_display: bool,

    /// Terminal width in character columns.
    pub term_width: usize,

    /// Terminal height in character rows.
    pub term_height: usize,

    /// Set to `true` when the visible content changed since the previous
    /// snapshot.
    ///
    /// The GUI uses this flag to reset `ViewState::scroll_offset` to 0 when
    /// the user is scrolled back and new output arrives.
    pub content_changed: bool,

    /// Current bracketed-paste mode setting.
    ///
    /// Carried in the snapshot so the GUI can wrap pasted text in the correct
    /// escape sequences without holding the emulator lock.
    pub bracketed_paste: RlBracket,

    /// Current mouse-tracking mode setting.
    ///
    /// Carried in the snapshot so the GUI can decide which mouse events to
    /// encode and send to the PTY without holding the emulator lock.
    pub mouse_tracking: MouseTrack,

    /// Whether the terminal should repeat key-press events while a key is held.
    pub repeat_keys: bool,

    /// Whether the cursor key is in application mode (`DECCKM`).
    ///
    /// Needed by the GUI to encode arrow / home / end keys correctly without
    /// consulting the emulator.
    pub cursor_key_app_mode: bool,

    /// Whether the terminal has requested that rendering be suppressed
    /// (Synchronized Output / `DEC 2026`).
    ///
    /// When `true` the GUI skips the render pass entirely for this frame.
    pub skip_draw: bool,

    /// Current working directory reported by the shell via OSC 7, if any.
    ///
    /// The GUI can use this for tab titles, file-open dialogs, or spawning
    /// new terminals in the same directory.
    pub cwd: Option<String>,

    /// Current FTCS (OSC 133) shell integration state.
    ///
    /// Indicates whether the terminal is currently inside a prompt, command
    /// input, or command output region.
    pub ftcs_state: FtcsState,

    /// Exit code from the most recent `OSC 133 ; D` marker, if any.
    ///
    /// The GUI can use this to display command success/failure indicators.
    pub last_exit_code: Option<i32>,

    /// The active color theme palette.
    ///
    /// Carried in the snapshot so the GUI can render with the user's chosen
    /// theme without holding any lock.
    pub theme: &'static ThemePalette,
}

impl TerminalSnapshot {
    /// Construct a blank snapshot suitable as the initial value for an
    /// `ArcSwap<TerminalSnapshot>` before the PTY thread has produced any
    /// real data.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            visible_chars: Arc::new(Vec::new()),
            visible_tags: Arc::new(Vec::new()),
            scroll_offset: 0,
            max_scroll_offset: 0,
            height: 0,
            cursor_pos: CursorPos { x: 0, y: 0 },
            show_cursor: false,
            cursor_visual_style: CursorVisualStyle::default(),
            is_alternate_screen: false,
            is_normal_display: true,
            term_width: 0,
            term_height: 0,
            content_changed: false,
            bracketed_paste: RlBracket::default(),
            mouse_tracking: MouseTrack::default(),
            repeat_keys: true,
            cursor_key_app_mode: false,
            skip_draw: false,
            cwd: None,
            ftcs_state: FtcsState::default(),
            last_exit_code: None,
            theme: &freminal_common::themes::CATPPUCCIN_MOCHA,
        }
    }
}
