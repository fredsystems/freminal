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

use freminal_buffer::row::Row;
use freminal_common::{
    buffer_states::{cursor::CursorPos, format_tag::FormatTag, tchar::TChar},
    cursor::CursorVisualStyle,
};

/// A point-in-time snapshot of the terminal state, ready for the GUI to render.
///
/// All expensive work (flattening rows → `Vec<TChar>` / `Vec<FormatTag>`) is
/// performed on the PTY thread so the GUI render path is allocation-free.
///
/// The snapshot is always immutable once constructed.  The GUI must never
/// mutate any field.
#[allow(clippy::struct_excessive_bools)] // Four independent semantic flags; enums would add noise
#[derive(Debug, Clone)]
pub struct TerminalSnapshot {
    /// Flattened visible character content, already converted from `Row`/`Cell`.
    ///
    /// Produced once on the PTY side; the GUI reads it directly.
    pub visible_chars: Vec<TChar>,

    /// Format tags corresponding to `visible_chars`.
    pub visible_tags: Vec<FormatTag>,

    /// Raw rows wrapped in an `Arc` so the GUI can perform its own
    /// `scroll_offset`-based slicing without copying all row data.
    ///
    /// This is only needed once scrollback rendering is active.  Until then
    /// the value is an empty `Arc<Vec<Row>>`.
    pub rows: Arc<Vec<Row>>,

    /// Total number of rows (scrollback + visible) so the GUI can compute the
    /// maximum scroll offset without inspecting `rows` directly in most cases.
    pub total_rows: usize,

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
}
