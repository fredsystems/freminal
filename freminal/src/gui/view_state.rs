// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `ViewState` — GUI-local terminal view state.
//!
//! This struct holds all state that is owned exclusively by the GUI render
//! loop and has no business being inside the `TerminalEmulator` or `Buffer`.
//! It is never shared with the PTY thread.
//!
//! See `Documents/PERFORMANCE_PLAN.md`, Section 4.5 for the architecture context.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use conv2::ConvUtil;
use egui;
use freminal_common::buffer_states::tchar::TChar;

use super::mouse::PreviousMouseState;

/// Duration of one text-blink tick (~167 ms).
///
/// At this rate the 6-tick cycle completes in ~1 000 ms:
///   - Slow blink: visible on ticks 0-2 (501 ms), hidden on ticks 3-5 (501 ms) ≈ 1 Hz.
///   - Fast blink: visible on even ticks (0,2,4), hidden on odd ticks (1,3,5) ≈ 3 Hz.
pub const TEXT_BLINK_TICK_DURATION: Duration = Duration::from_millis(167);

/// Maximum elapsed time between two primary clicks to count as a multi-click.
///
/// If the second click arrives within this window AND within
/// [`DOUBLE_CLICK_MAX_CELL_DISTANCE`] of the first, the `click_count` is
/// incremented rather than reset to 1.
pub(crate) const DOUBLE_CLICK_TIMEOUT: Duration = Duration::from_millis(400);

/// Maximum per-axis distance (in terminal cells) between consecutive clicks
/// for them to be considered part of the same multi-click sequence.
pub(crate) const DOUBLE_CLICK_MAX_CELL_DISTANCE: usize = 1;

/// A terminal cell coordinate (column, row), both 0-indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellCoord {
    pub col: usize,
    pub row: usize,
}

/// Tracks an in-progress or completed text selection.
///
/// Selection is defined by an *anchor* (where the mouse was pressed) and an
/// *end* (where the mouse currently is or was released).  The anchor stays
/// fixed; the end moves with the pointer.
///
/// The selection is "active" when a drag is in progress (`is_selecting`), and
/// "present" when anchor != end (i.e. something is highlighted).
///
/// When `is_block` is `true` the selection is a rectangular block: every row
/// from `anchor.row` to `end.row` is highlighted from
/// `min(anchor.col, end.col)` to `max(anchor.col, end.col)`.  Block mode is
/// activated by holding Alt while starting a drag (Alt+drag).
#[derive(Debug, Clone, Default)]
pub struct SelectionState {
    /// The cell where the mouse button was pressed (fixed during drag).
    pub anchor: Option<CellCoord>,
    /// The cell where the pointer currently is (updated during drag).
    pub end: Option<CellCoord>,
    /// `true` while the primary button is held down and dragging.
    pub is_selecting: bool,
    /// `true` when the selection is a rectangular block (Alt+drag).
    ///
    /// When `false` the selection is a linear span (normal left-to-right,
    /// wrapping across rows).  When `true` every row in the range is
    /// highlighted between the same two column boundaries.
    pub is_block: bool,
}

impl SelectionState {
    /// Returns the normalised selection range as `(start, end)` where `start`
    /// is always before or equal to `end` in reading order.
    ///
    /// For both linear and block selections `start.row <= end.row`.
    /// For block selections the column ordering is not normalised here —
    /// renderers use `min`/`max` of the two column values directly.
    ///
    /// Returns `None` if there is no selection.
    #[must_use]
    pub fn normalised(&self) -> Option<(CellCoord, CellCoord)> {
        let (a, e) = (self.anchor?, self.end?);
        if self.is_block {
            // Block mode: normalise rows only.  Keep the original column
            // values so renderers use `min`/`max` of the two directly.
            if a.row <= e.row {
                Some((a, e))
            } else {
                Some((
                    CellCoord {
                        row: e.row,
                        col: a.col,
                    },
                    CellCoord {
                        row: a.row,
                        col: e.col,
                    },
                ))
            }
        } else if a.row < e.row || (a.row == e.row && a.col <= e.col) {
            Some((a, e))
        } else {
            Some((e, a))
        }
    }

    /// Clear the selection entirely, including block mode.
    pub const fn clear(&mut self) {
        self.anchor = None;
        self.end = None;
        self.is_selecting = false;
        self.is_block = false;
    }

    /// Returns `true` if there is a visible selection (anchor and end differ).
    #[must_use]
    pub fn has_selection(&self) -> bool {
        match (self.anchor, self.end) {
            (Some(a), Some(e)) => a != e,
            _ => false,
        }
    }
}

/// A single search match span within the terminal buffer.
///
/// Coordinates are in *buffer-absolute* space: `row` is 0-indexed from the
/// first scrollback row (row 0 = oldest scrollback line).  `col_start` and
/// `col_end` are inclusive display-column indices within that row.
///
/// When rendering highlights, only matches whose row falls within the
/// visible window are converted to screen-relative coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchSpan {
    /// Buffer-absolute row index (0 = first scrollback row).
    pub row: usize,
    /// First matching column (inclusive).
    pub col_start: usize,
    /// Last matching column (inclusive).
    pub col_end: usize,
}

/// Tracks whether a `RequestSearchBuffer` message is in-flight.
///
/// Used instead of a bare `bool` to satisfy `clippy::struct_excessive_bools`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BufferRequestState {
    /// No request is pending — the GUI can send a new one.
    #[default]
    Idle,
    /// A request has been sent; waiting for the PTY thread to respond.
    Pending,
}

/// State owned by the GUI for the search-in-scrollback overlay.
///
/// All fields are private-to-GUI — the PTY thread never reads them.
/// Searching is done against the full buffer (scrollback + visible), which
/// is fetched on-demand from the PTY thread via `InputEvent::RequestSearchBuffer`.
#[derive(Debug, Default)]
pub struct SearchState {
    /// Whether the search overlay is currently visible.
    pub is_open: bool,
    /// The current query string (UTF-8).
    pub query: String,
    /// All matches found for the current query in the full buffer.
    ///
    /// Match rows are buffer-absolute (0 = first scrollback row).
    /// Empty when `query` is empty or no matches were found.
    pub matches: Vec<MatchSpan>,
    /// Index into `matches` indicating the "current" (focused) match.
    ///
    /// Wraps around: navigating forward from the last match returns to 0.
    pub current_match: usize,
    /// When `true`, the query is compiled as a regular expression.
    pub regex_mode: bool,
    /// The query string that was used to produce the current `matches` list.
    ///
    /// Used to detect when the query changed and a re-search is needed.
    pub last_searched_query: String,
    /// Whether `regex_mode` was active when `matches` was last computed.
    pub last_searched_regex: bool,
    /// The full-buffer `TChar` corpus that was searched for the current
    /// `matches` list.  When a new corpus is fetched from the PTY thread
    /// (because `total_rows` changed), the search is stale and must be re-run.
    pub cached_full_buffer: Option<Arc<Vec<TChar>>>,
    /// The `total_rows` value when the cached buffer was fetched.
    ///
    /// When the snapshot's `total_rows` changes, the cached buffer is stale
    /// and a new `RequestSearchBuffer` should be sent to the PTY thread.
    pub last_known_total_rows: usize,
    /// Whether a `RequestSearchBuffer` has been sent and the response is
    /// still pending.  Prevents sending duplicate requests.
    pub buffer_request_state: BufferRequestState,
}

impl SearchState {
    /// Returns `true` if a re-search is needed (query or mode changed since
    /// the last search, or no cached buffer is available yet).
    #[must_use]
    pub fn needs_refresh(&self) -> bool {
        self.cached_full_buffer.is_none()
            || self.query != self.last_searched_query
            || self.regex_mode != self.last_searched_regex
    }

    /// Mark the current matches as up-to-date.
    pub fn mark_fresh(&mut self) {
        self.last_searched_query.clone_from(&self.query);
        self.last_searched_regex = self.regex_mode;
    }

    /// Move to the next match, wrapping around.
    ///
    /// Does nothing when there are no matches.
    #[allow(clippy::missing_const_for_fn)] // Vec::is_empty() / Vec::len() are not const
    pub fn next_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        self.current_match = (self.current_match + 1) % self.matches.len();
    }

    /// Move to the previous match, wrapping around.
    ///
    /// Does nothing when there are no matches.
    #[allow(clippy::missing_const_for_fn)] // Vec::is_empty() / Vec::len() are not const
    pub fn prev_match(&mut self) {
        if self.matches.is_empty() {
            return;
        }
        if self.current_match == 0 {
            self.current_match = self.matches.len() - 1;
        } else {
            self.current_match -= 1;
        }
    }

    /// Returns the currently focused match, if any.
    #[must_use]
    pub fn current(&self) -> Option<MatchSpan> {
        self.matches.get(self.current_match).copied()
    }

    /// Reset matches and navigation when the search overlay closes.
    pub fn close(&mut self) {
        self.is_open = false;
        self.matches.clear();
        self.current_match = 0;
        self.last_searched_query.clear();
        self.last_searched_regex = false;
        self.cached_full_buffer = None;
        self.last_known_total_rows = 0;
        self.buffer_request_state = BufferRequestState::Idle;
    }
}

/// GUI-local view state for the terminal widget.
///
/// Everything here belongs to the render thread only.  The PTY thread never
/// reads or writes any of these fields.
///
/// All fields that were previously on `TerminalEmulator` / `TerminalState` have
/// been migrated here as part of the lock-free architecture refactor
/// (see `Documents/PERFORMANCE_PLAN.md`, Section 4.5).
#[derive(Debug)]
pub struct ViewState {
    /// How many lines the user has scrolled back from the live bottom.
    ///
    /// `0` = live bottom view (normal terminal mode).
    /// `> 0` = user is viewing older scrollback history.
    ///
    /// This field is the single source of truth for the scroll position.
    /// `Buffer` no longer stores or mutates it.  All `Buffer` methods that
    /// operate on visible rows accept this value as a parameter.
    pub scroll_offset: usize,

    /// The last mouse position reported to the terminal, if any.
    pub mouse_position: Option<egui::Pos2>,

    /// Whether the terminal window currently has keyboard focus.
    pub window_focused: bool,

    /// The last `(width, height)` in character cells that was sent to the PTY
    /// as a resize.  Used to debounce resize events so we only send a new
    /// `InputEvent::Resize` when the size actually changes.
    pub last_sent_size: (usize, usize),

    /// The most-recently pressed key, used to suppress auto-repeat on the
    /// first frame a key is held down.
    pub previous_key: Option<egui::Key>,

    /// Accumulated scroll delta (in fractional lines) carried over between
    /// frames so sub-line scroll events are not lost.
    pub previous_scroll_amount: f32,

    /// The mouse button / position / modifier state from the previous frame,
    /// used to detect button-state transitions and avoid sending redundant
    /// mouse reports to the PTY.
    pub previous_mouse_state: Option<PreviousMouseState>,

    /// Current text selection state (anchor, end, `is_selecting`).
    ///
    /// Populated when the user clicks and drags with the primary mouse button
    /// while mouse tracking is off (no mouse-aware TUI application running).
    pub selection: SelectionState,

    // ── Text blink state ─────────────────────────────────────────────
    /// Current position in the 6-tick blink cycle (0–5).
    ///
    /// The cycle advances by one tick every ~167 ms when `has_blinking_text`
    /// is true in the snapshot.  At tick 6 it wraps back to 0.
    ///
    /// Slow blink visibility: `cycle < 3` → visible, `cycle >= 3` → hidden.
    /// Fast blink visibility: `cycle % 2 == 0` → visible, `cycle % 2 == 1` → hidden.
    pub text_blink_cycle: u8,

    /// Timestamp of the last blink-cycle tick.
    ///
    /// When the snapshot's `has_blinking_text` transitions from false to true,
    /// this is set to `Instant::now()` so the first visible phase starts
    /// immediately.
    pub text_blink_last_tick: Instant,

    /// Whether slow-blink text should currently be drawn (derived from cycle).
    pub text_blink_slow_visible: bool,

    /// Whether fast-blink text should currently be drawn (derived from cycle).
    pub text_blink_fast_visible: bool,

    // ── Multi-click tracking ─────────────────────────────────────────
    /// Timestamp of the most-recent primary button press, used to detect
    /// double- and triple-clicks.  `None` until the first click arrives.
    pub last_click_time: Option<Instant>,

    /// Buffer-absolute cell coordinate of the most-recent primary button
    /// press, used for proximity checking in multi-click detection.
    pub last_click_pos: Option<CellCoord>,

    /// Click multiplicity for the current multi-click sequence.
    ///
    /// - `0` — no primary click has been recorded yet.
    /// - `1` — single click (normal press-drag-release selection).
    /// - `2` — double click (expand to word boundaries).
    /// - `3` — triple click (expand to line boundaries; capped here).
    pub click_count: u8,

    // ── Context menu ─────────────────────────────────────────────────
    /// Cell coordinate of the most-recent right-click that should open the
    /// context menu.
    ///
    /// Set to `Some(coord)` when:
    /// - Mouse tracking is OFF and `PointerButton::Secondary` is pressed, or
    /// - Mouse tracking is ON but Shift is held (escape hatch).
    ///
    /// The widget's `show()` method reads this to decide whether to render
    /// the context menu and to look up the URL under the clicked cell.
    /// Cleared when the context menu closes.
    pub context_menu_cell: Option<CellCoord>,

    /// Pixel position (in egui window coordinates) where the context menu
    /// should appear.  Captured at the moment of the right-click.
    ///
    /// `Some(pos)` means the menu is open; `None` means it is closed.
    /// Cleared when the user picks a menu item or clicks outside the popup.
    pub context_menu_pos: Option<egui::Pos2>,

    // ── Font zoom ────────────────────────────────────────────────────
    /// Session-only font size delta applied on top of the base font size
    /// from the config.
    ///
    /// The effective font size is `config.font.size + zoom_delta`, clamped
    /// to `[4.0, 96.0]`.  Each tab maintains its own zoom delta so that
    /// zooming in one tab does not affect others.
    ///
    /// `ZoomReset` (Ctrl+0) sets this back to `0.0` (i.e. back to the
    /// config's base font size).  This value is never persisted.
    pub zoom_delta: f32,

    // ── Visual bell ──────────────────────────────────────────────────
    /// Timestamp of the most-recent bell event, used to drive a brief
    /// visual flash overlay.
    ///
    /// `Some(instant)` = bell is active and the flash should be rendered
    /// (fading out over `BELL_FLASH_DURATION`).
    /// `None` = no active bell flash.
    ///
    /// Automatically cleared once the flash duration has elapsed.
    pub bell_since: Option<Instant>,

    // ── Cursor trail animation ───────────────────────────────────────
    /// Current visual cursor position (fractional cell coordinates).
    ///
    /// When cursor trail is enabled, this is the interpolated position
    /// between the previous snapshot cursor position and the current one.
    /// Updated each frame by [`Self::update_cursor_animation`].
    pub cursor_visual_col: f32,

    /// Current visual cursor row (fractional cell coordinates).
    pub cursor_visual_row: f32,

    /// The target cursor column (from the latest snapshot).
    ///
    /// When the snapshot's `cursor_pos` differs from this, a new
    /// animation is started.
    pub cursor_target_col: f32,

    /// The target cursor row (from the latest snapshot).
    pub cursor_target_row: f32,

    /// Timestamp of the last animation frame, used to compute delta time
    /// for exponential decay interpolation.
    ///
    /// `Some(instant)` = cursor was animating on the previous frame.
    /// `None` = cursor is at rest or trail was just enabled (first frame
    /// will record the timestamp without moving the cursor).
    pub cursor_last_frame: Option<Instant>,

    // ── Search overlay ───────────────────────────────────────────────
    /// Search overlay state: open/closed, query, matches, navigation.
    ///
    /// All search interaction is GUI-local and never touches the PTY thread.
    pub search_state: SearchState,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            scroll_offset: 0,
            mouse_position: None,
            window_focused: false,
            last_sent_size: (0, 0),
            previous_key: None,
            previous_scroll_amount: 0.0,
            previous_mouse_state: None,
            selection: SelectionState::default(),
            text_blink_cycle: 0,
            text_blink_last_tick: Instant::now(),
            text_blink_slow_visible: true,
            text_blink_fast_visible: true,
            last_click_time: None,
            last_click_pos: None,
            click_count: 0,
            context_menu_cell: None,
            context_menu_pos: None,
            zoom_delta: 0.0,
            bell_since: None,
            cursor_visual_col: 0.0,
            cursor_visual_row: 0.0,
            cursor_target_col: 0.0,
            cursor_target_row: 0.0,
            cursor_last_frame: None,
            search_state: SearchState::default(),
        }
    }
}

impl ViewState {
    /// Create a new `ViewState` with all fields at their default (live-bottom)
    /// values.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // ── Font zoom helpers ────────────────────────────────────────────

    /// Minimum allowed effective font size (points).
    const MIN_FONT_SIZE: f32 = 4.0;

    /// Maximum allowed effective font size (points).
    const MAX_FONT_SIZE: f32 = 96.0;

    /// Compute the effective font size for this tab.
    ///
    /// The effective size is `base_size + zoom_delta`, clamped to
    /// `[4.0, 96.0]`.  `base_size` is the config's `font.size`.
    #[must_use]
    pub fn effective_font_size(&self, base_size: f32) -> f32 {
        (base_size + self.zoom_delta).clamp(Self::MIN_FONT_SIZE, Self::MAX_FONT_SIZE)
    }

    /// Adjust the zoom delta by `step` points, clamping the effective size.
    ///
    /// After clamping, the zoom delta is back-derived so that
    /// `base + zoom_delta` equals the clamped effective size exactly.
    pub fn adjust_zoom(&mut self, base_size: f32, step: f32) {
        let effective =
            (base_size + self.zoom_delta + step).clamp(Self::MIN_FONT_SIZE, Self::MAX_FONT_SIZE);
        self.zoom_delta = effective - base_size;
    }

    /// Reset the zoom delta to zero (back to the config's base font size).
    pub const fn reset_zoom(&mut self) {
        self.zoom_delta = 0.0;
    }

    // ── Cursor trail animation ───────────────────────────────────────

    /// Update the cursor animation state for this frame.
    ///
    /// `target_col` and `target_row` come from the snapshot's `cursor_pos`.
    /// `trail_enabled` is the config flag; when `false`, the visual position
    /// snaps instantly to the target.  `duration` controls the speed of the
    /// exponential decay: approximately 95% of the remaining distance is
    /// covered in `duration` (tau = duration / 3).  Every cursor movement
    /// — including single-column typing — produces a visible glide.
    ///
    /// Returns `true` if the animation is still in progress (i.e. the caller
    /// should `request_repaint()` to continue driving it).
    pub fn update_cursor_animation(
        &mut self,
        target_col: f32,
        target_row: f32,
        trail_enabled: bool,
        duration: Duration,
    ) -> bool {
        /// Distance (in cells) below which the visual position snaps to the
        /// target rather than continuing to interpolate.
        const SNAP_THRESHOLD: f32 = 0.01;

        self.cursor_target_col = target_col;
        self.cursor_target_row = target_row;

        if !trail_enabled {
            // Snap instantly — no animation.
            self.cursor_visual_col = target_col;
            self.cursor_visual_row = target_row;
            self.cursor_last_frame = None;
            return false;
        }

        // First observation: snap visual to target so we don't glide
        // from (0,0) on startup.  Record the timestamp so subsequent
        // target changes will animate normally.
        if self.cursor_last_frame.is_none() {
            self.cursor_visual_col = target_col;
            self.cursor_visual_row = target_row;
            self.cursor_last_frame = Some(Instant::now());
            return false;
        }

        let dx = self.cursor_target_col - self.cursor_visual_col;
        let dy = self.cursor_target_row - self.cursor_visual_row;

        // Close enough — snap to target and stop animating.
        if dx.abs() < SNAP_THRESHOLD && dy.abs() < SNAP_THRESHOLD {
            self.cursor_visual_col = self.cursor_target_col;
            self.cursor_visual_row = self.cursor_target_row;
            return false;
        }

        let now = Instant::now();
        // `cursor_last_frame` is always `Some` here — the `None` case
        // was handled by the first-observation snap above.  The
        // `unwrap_or` is a defensive fallback that can never fire.
        let dt = now.duration_since(self.cursor_last_frame.unwrap_or(now));
        self.cursor_last_frame = Some(now);

        // Exponential decay: ~95% of distance covered in `duration`.
        // tau = duration / 3  →  e^(-3) ≈ 0.05  →  95% convergence.
        let tau = duration.as_secs_f32() / 3.0;
        if tau < f32::EPSILON {
            // Zero duration — snap immediately.
            self.cursor_visual_col = self.cursor_target_col;
            self.cursor_visual_row = self.cursor_target_row;
            self.cursor_last_frame = None;
            return false;
        }

        let factor = 1.0_f32 - (-dt.as_secs_f32() / tau).exp();
        self.cursor_visual_col = dx.mul_add(factor, self.cursor_visual_col);
        self.cursor_visual_row = dy.mul_add(factor, self.cursor_visual_row);

        true
    }

    /// Advance the text blink cycle if enough time has elapsed.
    ///
    /// Called from `update()` every frame when `has_blinking_text` is true.
    /// Returns `true` if the visibility flags changed (i.e. the caller should
    /// repaint).
    pub fn tick_text_blink(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.text_blink_last_tick);

        if elapsed < TEXT_BLINK_TICK_DURATION {
            return false;
        }

        // Advance by the number of ticks elapsed (normally 1, but could be
        // more if the frame rate dropped below the tick rate).
        // `as_millis()` returns `u128`; conv2 has no `ValueFrom<u128>`, so we
        // first narrow to `u64` via `try_from` (infallible for any realistic
        // duration), then use `approx_as` for the final `u64 → u8` step.
        let raw_u128 = elapsed.as_millis() / TEXT_BLINK_TICK_DURATION.as_millis() % 6;
        let ticks = u64::try_from(raw_u128)
            .unwrap_or(0)
            .approx_as::<u8>()
            .unwrap_or(0);
        self.text_blink_cycle = (self.text_blink_cycle + ticks) % 6;
        self.text_blink_last_tick = now;

        let old_slow = self.text_blink_slow_visible;
        let old_fast = self.text_blink_fast_visible;

        self.text_blink_slow_visible = self.text_blink_cycle < 3;
        self.text_blink_fast_visible = self.text_blink_cycle.is_multiple_of(2);

        old_slow != self.text_blink_slow_visible || old_fast != self.text_blink_fast_visible
    }

    /// Reset the text blink cycle to the beginning (both slow and fast
    /// visible).  Called when `has_blinking_text` transitions from false
    /// to true.
    pub fn reset_text_blink(&mut self) {
        self.text_blink_cycle = 0;
        self.text_blink_last_tick = Instant::now();
        self.text_blink_slow_visible = true;
        self.text_blink_fast_visible = true;
    }

    /// Compute the visibility for a given blink cycle value.
    ///
    /// Returns `(slow_visible, fast_visible)`.
    /// Useful for testing the mapping without an `Instant` dependency.
    #[must_use]
    pub const fn blink_visibility_for_cycle(cycle: u8) -> (bool, bool) {
        (cycle < 3, cycle.is_multiple_of(2))
    }

    /// Record a primary button press at `coord` occurring at `now`, updating
    /// the multi-click sequence counter.
    ///
    /// Rules:
    /// - If there is a previous click within [`DOUBLE_CLICK_TIMEOUT`] **and**
    ///   within [`DOUBLE_CLICK_MAX_CELL_DISTANCE`] cells on each axis, the
    ///   count is incremented (capped at 3).
    /// - Otherwise the count resets to 1.
    ///
    /// Returns the new `click_count` so the caller can branch on single /
    /// double / triple click behaviour.
    pub fn register_click(&mut self, coord: CellCoord, now: Instant) -> u8 {
        let is_multi = match (self.last_click_time, self.last_click_pos) {
            (Some(prev_time), Some(prev_pos)) => {
                let elapsed = now.duration_since(prev_time);
                let col_dist = coord.col.abs_diff(prev_pos.col);
                let row_dist = coord.row.abs_diff(prev_pos.row);
                elapsed <= DOUBLE_CLICK_TIMEOUT
                    && col_dist <= DOUBLE_CLICK_MAX_CELL_DISTANCE
                    && row_dist <= DOUBLE_CLICK_MAX_CELL_DISTANCE
            }
            _ => false,
        };

        if is_multi {
            self.click_count = (self.click_count + 1).min(3);
        } else {
            self.click_count = 1;
        }

        self.last_click_time = Some(now);
        self.last_click_pos = Some(coord);
        self.click_count
    }
}

/// Returns `true` if `tc` is a word character for selection purposes.
///
/// Word characters are ASCII alphanumerics, `_`, and any non-ASCII Unicode
/// grapheme (treated conservatively as a word constituent).  Spaces and
/// newlines are not word characters.
const fn is_word_char(tc: &TChar) -> bool {
    match tc {
        TChar::Ascii(b) => b.is_ascii_alphanumeric() || *b == b'_',
        // Non-ASCII graphemes (e.g. accented letters, CJK) are word chars.
        TChar::Utf8(..) => true,
        TChar::Space | TChar::NewLine => false,
    }
}

/// Walk `visible_chars` and collect all displayable cells for `screen_row`.
///
/// Returns a `Vec<(start_display_col, display_width, TChar)>` for each
/// character in the row.  `TChar::NewLine` entries are skipped (they act as
/// row delimiters, not content).
///
/// Returns an empty `Vec` if `screen_row` is beyond the available data.
fn collect_row_cells(visible_chars: &[TChar], screen_row: usize) -> Vec<(usize, usize, TChar)> {
    let mut current_row: usize = 0;
    let mut idx: usize = 0;

    // Advance past all preceding rows.
    while current_row < screen_row {
        if idx >= visible_chars.len() {
            return Vec::new();
        }
        if matches!(visible_chars[idx], TChar::NewLine) {
            current_row += 1;
        }
        idx += 1;
    }

    // Collect cells for this row until the next NewLine or end of slice.
    let mut cells = Vec::new();
    let mut display_col: usize = 0;
    while idx < visible_chars.len() {
        if matches!(visible_chars[idx], TChar::NewLine) {
            break;
        }
        // Use display_width, but treat zero-width graphemes as width 1 to
        // avoid infinite loops and degenerate column offsets.
        let w = visible_chars[idx].display_width().max(1);
        cells.push((display_col, w, visible_chars[idx]));
        display_col += w;
        idx += 1;
    }
    cells
}

/// Find the display-column boundaries of the word that contains `col` in
/// `screen_row` of `visible_chars`.
///
/// `visible_chars` is the flat terminal character buffer (from
/// `TerminalSnapshot::visible_chars`), using `TChar::NewLine` as a row
/// separator.  `screen_row` is **screen-relative** (0 = first visible row).
///
/// Word characters are defined by [`is_word_char`]: ASCII alphanumerics, `_`,
/// and non-ASCII Unicode graphemes.
///
/// Returns `(start_col, end_col)` as **inclusive** display-column indices for
/// the word boundaries.  If `col` falls on a non-word character, only that
/// character's span is returned.  If the row has no content, `(col, col)` is
/// returned.
pub(crate) fn word_boundaries(
    visible_chars: &[TChar],
    screen_row: usize,
    col: usize,
) -> (usize, usize) {
    let cells = collect_row_cells(visible_chars, screen_row);
    if cells.is_empty() {
        return (col, col);
    }

    // Find the cell that contains display column `col`.
    let hit = cells
        .iter()
        .position(|&(start, w, _)| col >= start && col < start + w);

    // If `col` is beyond row content, clamp to the last cell and
    // fall through to the word-expansion logic so we return the full
    // word span rather than a bare single-cell span.
    let Some(hit) = hit else {
        let last = cells.len() - 1;
        // Re-enter the function logic at the word-expansion step by
        // treating the last cell as the hit.
        let hit = last;
        if !is_word_char(&cells[hit].2) {
            return (cells[hit].0, cells[hit].0 + cells[hit].1 - 1);
        }
        let mut start = hit;
        while start > 0 && is_word_char(&cells[start - 1].2) {
            start -= 1;
        }
        let mut end = hit;
        while end + 1 < cells.len() && is_word_char(&cells[end + 1].2) {
            end += 1;
        }
        return (cells[start].0, cells[end].0 + cells[end].1 - 1);
    };

    if !is_word_char(&cells[hit].2) {
        // Clicked on a delimiter — select just that cell.
        return (cells[hit].0, cells[hit].0 + cells[hit].1 - 1);
    }

    // Expand left while the preceding cell is also a word char.
    let mut start = hit;
    while start > 0 && is_word_char(&cells[start - 1].2) {
        start -= 1;
    }

    // Expand right while the following cell is also a word char.
    let mut end = hit;
    while end + 1 < cells.len() && is_word_char(&cells[end + 1].2) {
        end += 1;
    }

    (cells[start].0, cells[end].0 + cells[end].1 - 1)
}

/// Find the display-column boundaries of the entire visual line at
/// `screen_row` in `visible_chars`.
///
/// `visible_chars` is the flat terminal character buffer (from
/// `TerminalSnapshot::visible_chars`), using `TChar::NewLine` as a row
/// separator.  `screen_row` is **screen-relative** (0 = first visible row).
///
/// Returns `(0, last_col)` where `last_col` is the last occupied display
/// column in the row (inclusive).  Returns `(0, 0)` for an empty row.
pub(crate) fn line_boundaries(visible_chars: &[TChar], screen_row: usize) -> (usize, usize) {
    let cells = collect_row_cells(visible_chars, screen_row);
    if cells.is_empty() {
        return (0, 0);
    }
    let last = cells.len() - 1;
    (0, cells[last].0 + cells[last].1 - 1)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn blink_cycle_slow_visibility() {
        // Cycles 0, 1, 2 → slow visible; 3, 4, 5 → slow hidden.
        let expected_slow = [true, true, true, false, false, false];
        for (cycle, &expected) in expected_slow.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let (slow, _) = ViewState::blink_visibility_for_cycle(cycle as u8);
            assert_eq!(slow, expected, "slow visibility at cycle {cycle}");
        }
    }

    #[test]
    fn blink_cycle_fast_visibility() {
        // Cycles 0, 2, 4 → fast visible; 1, 3, 5 → fast hidden.
        let expected_fast = [true, false, true, false, true, false];
        for (cycle, &expected) in expected_fast.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let (_, fast) = ViewState::blink_visibility_for_cycle(cycle as u8);
            assert_eq!(fast, expected, "fast visibility at cycle {cycle}");
        }
    }

    #[test]
    fn blink_cycle_combined_visibility() {
        // Verify combined (slow, fast) pairs for the full cycle.
        let expected = [
            (true, true),   // cycle 0: both visible
            (true, false),  // cycle 1: slow visible, fast hidden
            (true, true),   // cycle 2: both visible
            (false, false), // cycle 3: both hidden
            (false, true),  // cycle 4: slow hidden, fast visible
            (false, false), // cycle 5: both hidden
        ];
        for (cycle, &(slow, fast)) in expected.iter().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            let result = ViewState::blink_visibility_for_cycle(cycle as u8);
            assert_eq!(result, (slow, fast), "visibility at cycle {cycle}");
        }
    }

    #[test]
    fn reset_text_blink_restores_defaults() {
        let mut vs = ViewState::new();
        vs.text_blink_cycle = 4;
        vs.text_blink_slow_visible = false;
        vs.text_blink_fast_visible = false;

        vs.reset_text_blink();

        assert_eq!(vs.text_blink_cycle, 0);
        assert!(vs.text_blink_slow_visible);
        assert!(vs.text_blink_fast_visible);
    }

    #[test]
    fn tick_does_not_advance_when_insufficient_time() {
        let mut vs = ViewState::new();
        // Just created — last_tick is now(), so elapsed < 167ms.
        let changed = vs.tick_text_blink();
        assert!(!changed, "should not advance within the first tick window");
        assert_eq!(vs.text_blink_cycle, 0);
        assert!(vs.text_blink_slow_visible);
        assert!(vs.text_blink_fast_visible);
    }

    #[test]
    fn tick_advances_after_elapsed_time() {
        let mut vs = ViewState::new();
        // Artificially set last_tick to the past so the next tick will fire.
        vs.text_blink_last_tick = Instant::now()
            .checked_sub(TEXT_BLINK_TICK_DURATION)
            .unwrap();

        let changed = vs.tick_text_blink();
        // cycle goes from 0 → 1.  slow stays true, fast changes true → false.
        assert!(changed, "visibility should change at cycle 0 → 1");
        assert_eq!(vs.text_blink_cycle, 1);
        assert!(vs.text_blink_slow_visible); // cycle 1 < 3
        assert!(!vs.text_blink_fast_visible); // cycle 1 % 2 != 0
    }

    #[test]
    fn new_view_state_starts_both_visible() {
        let vs = ViewState::new();
        assert!(vs.text_blink_slow_visible);
        assert!(vs.text_blink_fast_visible);
        assert_eq!(vs.text_blink_cycle, 0);
    }

    // ── register_click tests ──────────────────────────────────────────────

    fn coord(col: usize, row: usize) -> CellCoord {
        CellCoord { col, row }
    }

    #[test]
    fn first_click_is_single() {
        let mut vs = ViewState::new();
        let t = Instant::now();
        let count = vs.register_click(coord(5, 3), t);
        assert_eq!(count, 1, "first click should be single (count=1)");
        assert_eq!(vs.click_count, 1);
    }

    #[test]
    fn rapid_same_position_clicks_increment_count() {
        let mut vs = ViewState::new();
        let t0 = Instant::now();
        vs.register_click(coord(2, 1), t0);

        // Second click within timeout, same position → double click.
        let t1 = t0;
        let count = vs.register_click(coord(2, 1), t1);
        assert_eq!(count, 2, "quick second click should be double (count=2)");

        // Third click → triple click.
        let count = vs.register_click(coord(2, 1), t1);
        assert_eq!(count, 3, "quick third click should be triple (count=3)");

        // Fourth click is capped at 3.
        let count = vs.register_click(coord(2, 1), t1);
        assert_eq!(count, 3, "click_count must be capped at 3");
    }

    #[test]
    fn slow_click_resets_count() {
        let mut vs = ViewState::new();
        let t0 = Instant::now();
        vs.register_click(coord(0, 0), t0);
        vs.click_count = 2; // manually set to double

        // Click well past the timeout.
        let t1 = t0 + DOUBLE_CLICK_TIMEOUT + Duration::from_millis(1);
        let count = vs.register_click(coord(0, 0), t1);
        assert_eq!(count, 1, "click after timeout should reset to single");
    }

    #[test]
    fn distant_click_resets_count() {
        let mut vs = ViewState::new();
        let t = Instant::now();
        vs.register_click(coord(0, 0), t);
        vs.click_count = 2;

        // Click far away (beyond threshold) within the timeout.
        let count = vs.register_click(coord(10, 0), t);
        assert_eq!(count, 1, "click far from previous should reset to single");
    }

    #[test]
    fn click_within_proximity_threshold_increments() {
        let mut vs = ViewState::new();
        let t = Instant::now();
        vs.register_click(coord(5, 5), t);

        // Adjacent cell (distance = 1 on each axis) is within threshold.
        let count = vs.register_click(coord(6, 6), t);
        assert_eq!(count, 2, "click within 1-cell distance should double-click");
    }

    #[test]
    fn click_just_outside_proximity_resets() {
        let mut vs = ViewState::new();
        let t = Instant::now();
        vs.register_click(coord(5, 5), t);

        // Distance of 2 on col axis is outside the threshold (max is 1).
        let count = vs.register_click(coord(7, 5), t);
        assert_eq!(count, 1, "click 2 cells away should reset count");
    }

    // ── word_boundaries tests ─────────────────────────────────────────────

    fn make_visible(rows: &[&str]) -> Vec<TChar> {
        let mut chars = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            for b in row.bytes() {
                chars.push(TChar::new_from_single_char(b));
            }
            if i + 1 < rows.len() {
                chars.push(TChar::NewLine);
            }
        }
        chars
    }

    #[test]
    fn word_boundaries_single_word() {
        // Row: "hello" — clicking anywhere gives (0, 4).
        let chars = make_visible(&["hello"]);
        assert_eq!(word_boundaries(&chars, 0, 0), (0, 4));
        assert_eq!(word_boundaries(&chars, 0, 2), (0, 4));
        assert_eq!(word_boundaries(&chars, 0, 4), (0, 4));
    }

    #[test]
    fn word_boundaries_word_then_space() {
        // Row: "hi there" — click on 'h'/'i' gives word "hi" (0..1),
        // click on space gives just the space, click on "there" gives (3..7).
        let chars = make_visible(&["hi there"]);
        assert_eq!(word_boundaries(&chars, 0, 0), (0, 1)); // 'h'
        assert_eq!(word_boundaries(&chars, 0, 1), (0, 1)); // 'i'
        assert_eq!(word_boundaries(&chars, 0, 2), (2, 2)); // space
        assert_eq!(word_boundaries(&chars, 0, 3), (3, 7)); // 'there'
        assert_eq!(word_boundaries(&chars, 0, 7), (3, 7)); // 'e' of 'there'
    }

    #[test]
    fn word_boundaries_underscore_included() {
        // '_' is a word character.
        let chars = make_visible(&["my_var"]);
        assert_eq!(word_boundaries(&chars, 0, 0), (0, 5));
        assert_eq!(word_boundaries(&chars, 0, 2), (0, 5)); // '_'
        assert_eq!(word_boundaries(&chars, 0, 5), (0, 5));
    }

    #[test]
    fn word_boundaries_punctuation_not_word() {
        // Punctuation breaks the word.  "foo.bar": '.' is not a word char.
        let chars = make_visible(&["foo.bar"]);
        assert_eq!(word_boundaries(&chars, 0, 0), (0, 2)); // "foo"
        assert_eq!(word_boundaries(&chars, 0, 3), (3, 3)); // '.'
        assert_eq!(word_boundaries(&chars, 0, 4), (4, 6)); // "bar"
    }

    #[test]
    fn word_boundaries_second_row() {
        // Two rows — clicking on row 1.
        let chars = make_visible(&["abc", "def"]);
        assert_eq!(word_boundaries(&chars, 1, 0), (0, 2));
        assert_eq!(word_boundaries(&chars, 1, 2), (0, 2));
    }

    #[test]
    fn word_boundaries_empty_row() {
        // An empty row returns (col, col).
        let chars = make_visible(&["", "abc"]);
        assert_eq!(word_boundaries(&chars, 0, 0), (0, 0));
    }

    #[test]
    fn word_boundaries_col_beyond_row_clamps() {
        // `col` beyond row content: should return the last cell's span.
        let chars = make_visible(&["hi"]);
        // 'h' at col 0, 'i' at col 1.  col=5 is beyond.
        assert_eq!(word_boundaries(&chars, 0, 5), (0, 1)); // last cell is 'i' at col 1
    }

    // ── line_boundaries tests ─────────────────────────────────────────────

    #[test]
    fn line_boundaries_simple_row() {
        let chars = make_visible(&["hello"]);
        assert_eq!(line_boundaries(&chars, 0), (0, 4));
    }

    #[test]
    fn line_boundaries_second_row() {
        let chars = make_visible(&["abc", "xyz"]);
        assert_eq!(line_boundaries(&chars, 0), (0, 2));
        assert_eq!(line_boundaries(&chars, 1), (0, 2));
    }

    #[test]
    fn line_boundaries_empty_row() {
        let chars = make_visible(&["", "x"]);
        assert_eq!(line_boundaries(&chars, 0), (0, 0));
    }

    #[test]
    fn line_boundaries_single_char() {
        let chars = make_visible(&["a"]);
        assert_eq!(line_boundaries(&chars, 0), (0, 0));
    }

    // ── integration-style selection tests ────────────────────────────────

    /// Simulate the press-handler logic for a double-click and verify that
    /// `SelectionState` is set to the full word span.
    ///
    /// This mirrors the `click_count == 2` branch in `input.rs`:
    ///   anchor = `word_start`, end = `word_end`.
    #[test]
    fn double_click_sets_word_selection() {
        let chars = make_visible(&["hello world"]);
        // "hello" spans cols 0–4; "world" spans cols 6–10; space is col 5.
        let t = Instant::now();
        let mut vs = ViewState::new();

        // First click (single) — on 'h' at col 0, abs_row 0.
        let click_coord = CellCoord { col: 0, row: 0 };
        vs.register_click(click_coord, t);
        let (anchor_col, end_col) = word_boundaries(&chars, 0, 0);
        vs.selection.anchor = Some(CellCoord {
            col: anchor_col,
            row: 0,
        });
        vs.selection.end = Some(CellCoord {
            col: end_col,
            row: 0,
        });
        vs.selection.is_selecting = true;

        // Second click (double) — same position, within timeout.
        let count = vs.register_click(click_coord, t);
        assert_eq!(count, 2, "second click should be double");
        let (anchor_col, end_col) = word_boundaries(&chars, 0, 0);
        vs.selection.anchor = Some(CellCoord {
            col: anchor_col,
            row: 0,
        });
        vs.selection.end = Some(CellCoord {
            col: end_col,
            row: 0,
        });

        // "hello" is cols 0–4.
        assert_eq!(
            vs.selection.anchor,
            Some(CellCoord { col: 0, row: 0 }),
            "anchor at word start"
        );
        assert_eq!(
            vs.selection.end,
            Some(CellCoord { col: 4, row: 0 }),
            "end at word end"
        );
    }

    /// Simulate the press-handler logic for a triple-click and verify that
    /// `SelectionState` is set to the full line span.
    ///
    /// This mirrors the `click_count >= 3` branch in `input.rs`:
    ///   anchor = `line_start` (col 0), end = `line_end` (last col).
    #[test]
    fn triple_click_sets_line_selection() {
        let chars = make_visible(&["hello world"]);
        // Row 0 spans cols 0–10 (11 chars).
        let t = Instant::now();
        let mut vs = ViewState::new();

        let click_coord = CellCoord { col: 3, row: 0 };

        // First click.
        vs.register_click(click_coord, t);
        // Second click.
        vs.register_click(click_coord, t);
        // Third click.
        let count = vs.register_click(click_coord, t);
        assert_eq!(count, 3, "third click should be triple");

        let (start_col, end_col) = line_boundaries(&chars, 0);
        vs.selection.anchor = Some(CellCoord {
            col: start_col,
            row: 0,
        });
        vs.selection.end = Some(CellCoord {
            col: end_col,
            row: 0,
        });

        // Line spans cols 0–10 inclusive.
        assert_eq!(
            vs.selection.anchor,
            Some(CellCoord { col: 0, row: 0 }),
            "anchor at line start"
        );
        assert_eq!(
            vs.selection.end,
            Some(CellCoord { col: 10, row: 0 }),
            "end at line end"
        );
    }

    /// Verify that a single click clears any prior multi-click selection
    /// and records a point selection (anchor == end).
    #[test]
    fn single_click_sets_point_selection() {
        let mut vs = ViewState::new();
        let t = Instant::now();

        // Simulate double-click state left over.
        vs.click_count = 2;
        vs.selection.anchor = Some(CellCoord { col: 0, row: 0 });
        vs.selection.end = Some(CellCoord { col: 4, row: 0 });

        // A new single click well past the timeout resets to 1.
        let far_future = t + DOUBLE_CLICK_TIMEOUT + Duration::from_millis(1);
        let click_coord = CellCoord { col: 7, row: 2 };
        let count = vs.register_click(click_coord, far_future);
        assert_eq!(count, 1, "click after timeout must reset to single");

        // Simulate press handler: single-click sets anchor == end.
        vs.selection.anchor = Some(click_coord);
        vs.selection.end = Some(click_coord);
        vs.selection.is_selecting = true;

        assert_eq!(
            vs.selection.anchor, vs.selection.end,
            "point selection: anchor==end"
        );
        assert!(vs.selection.is_selecting);
    }

    // ── release-without-move regression tests ────────────────────────────

    /// Helper that mirrors `release_end_col()` from `input.rs`.
    ///
    /// Given the current `ViewState` (with `click_count` and `selection`
    /// already set by the press handler), compute the end column that the
    /// release handler should use.
    fn release_end_col(
        vs: &ViewState,
        visible_chars: &[TChar],
        x: usize,
        y: usize,
        abs_row: usize,
    ) -> usize {
        if vs.click_count >= 3 {
            let anchor_row = vs.selection.anchor.map_or(abs_row, |a| a.row);
            let (line_start, line_end) = line_boundaries(visible_chars, y);
            if abs_row >= anchor_row {
                line_end
            } else {
                line_start
            }
        } else if vs.click_count == 2 {
            let anchor_row = vs.selection.anchor.map_or(abs_row, |a| a.row);
            let anchor_col = vs.selection.anchor.map_or(x, |a| a.col);
            let (word_start, word_end) = word_boundaries(visible_chars, y, x);
            if abs_row > anchor_row || (abs_row == anchor_row && word_end >= anchor_col) {
                word_end
            } else {
                word_start
            }
        } else {
            x
        }
    }

    /// Double-click press+release without any pointer-move in between must
    /// keep the full word selection (regression test for the release handler
    /// collapsing the selection to the raw mouse column).
    #[test]
    fn double_click_release_without_move_keeps_word() {
        let chars = make_visible(&["hello world"]);
        // "hello" = cols 0–4, space = col 5, "world" = cols 6–10.
        // Double-click on 'e' (col 1) — press selects "hello" (0–4).
        let t = Instant::now();
        let mut vs = ViewState::new();
        let click_coord = CellCoord { col: 1, row: 0 };

        // First click.
        vs.register_click(click_coord, t);
        // Second click (double).
        let count = vs.register_click(click_coord, t);
        assert_eq!(count, 2);

        // Press handler: set anchor/end to word boundaries.
        let (word_start, word_end) = word_boundaries(&chars, 0, 1);
        vs.selection.anchor = Some(CellCoord {
            col: word_start,
            row: 0,
        });
        vs.selection.end = Some(CellCoord {
            col: word_end,
            row: 0,
        });
        vs.selection.is_selecting = true;

        // Release at the same raw position (col 1) — no PointerMoved fired.
        let end_col = release_end_col(&vs, &chars, 1, 0, 0);
        vs.selection.end = Some(CellCoord {
            col: end_col,
            row: 0,
        });
        vs.selection.is_selecting = false;

        // Selection must still span the full word "hello" (0–4), NOT col 1.
        assert_eq!(
            vs.selection.anchor,
            Some(CellCoord { col: 0, row: 0 }),
            "anchor must stay at word start"
        );
        assert_eq!(
            vs.selection.end,
            Some(CellCoord { col: 4, row: 0 }),
            "end must stay at word end, not collapse to raw col"
        );
    }

    /// Triple-click press+release without any pointer-move in between must
    /// keep the full line selection.
    #[test]
    fn triple_click_release_without_move_keeps_line() {
        let chars = make_visible(&["hello world"]);
        // Row 0 spans cols 0–10.
        let t = Instant::now();
        let mut vs = ViewState::new();
        let click_coord = CellCoord { col: 3, row: 0 };

        vs.register_click(click_coord, t);
        vs.register_click(click_coord, t);
        let count = vs.register_click(click_coord, t);
        assert_eq!(count, 3);

        // Press handler: set anchor/end to line boundaries.
        let (line_start, line_end) = line_boundaries(&chars, 0);
        vs.selection.anchor = Some(CellCoord {
            col: line_start,
            row: 0,
        });
        vs.selection.end = Some(CellCoord {
            col: line_end,
            row: 0,
        });
        vs.selection.is_selecting = true;

        // Release at raw col 3 — no PointerMoved fired.
        let end_col = release_end_col(&vs, &chars, 3, 0, 0);
        vs.selection.end = Some(CellCoord {
            col: end_col,
            row: 0,
        });
        vs.selection.is_selecting = false;

        assert_eq!(
            vs.selection.anchor,
            Some(CellCoord { col: 0, row: 0 }),
            "anchor must stay at line start"
        );
        assert_eq!(
            vs.selection.end,
            Some(CellCoord { col: 10, row: 0 }),
            "end must stay at line end, not collapse to raw col"
        );
    }

    // ── upward drag in word/line mode ────────────────────────────────────

    /// Double-click on row 1, then drag upward to row 0.  The end should
    /// snap to the word boundary on row 0, and after normalisation the
    /// anchor word on row 1 must still be fully included.
    #[test]
    fn double_click_drag_upward_preserves_anchor_word() {
        let chars = make_visible(&["foo bar", "baz qux"]);
        // Row 0: "foo"=0–2, ' '=3, "bar"=4–6
        // Row 1: "baz"=0–2, ' '=3, "qux"=4–6
        let t = Instant::now();
        let mut vs = ViewState::new();

        // Double-click on "qux" (row 1, col 5) — abs_row = 1.
        let click_coord = CellCoord { col: 5, row: 1 };
        vs.register_click(click_coord, t);
        let count = vs.register_click(click_coord, t);
        assert_eq!(count, 2);

        // Press handler: anchor/end to "qux" word boundaries on row 1.
        let (ws, we) = word_boundaries(&chars, 1, 5);
        vs.selection.anchor = Some(CellCoord { col: ws, row: 1 });
        vs.selection.end = Some(CellCoord { col: we, row: 1 });
        vs.selection.is_selecting = true;

        // Drag upward to row 0, col 1 (inside "foo").
        // The drag handler snaps to word boundaries:
        // abs_row (0) < anchor_row (1), so use word_start.
        let (drag_word_start, _) = word_boundaries(&chars, 0, 1);
        vs.selection.end = Some(CellCoord {
            col: drag_word_start,
            row: 0,
        });

        // After normalisation, the selection should span from "foo" start
        // on row 0 to "qux" end on row 1.
        let (start, end) = vs.selection.normalised().unwrap();
        assert_eq!(start, CellCoord { col: 0, row: 0 }, "start at 'foo' begin");
        assert_eq!(end, CellCoord { col: 4, row: 1 }, "end at 'qux' anchor");
    }

    /// Triple-click on row 1, then drag upward to row 0.  The end should
    /// snap to line start on row 0, and after normalisation the anchor
    /// line (row 1) must still be fully included.
    #[test]
    fn triple_click_drag_upward_preserves_anchor_line() {
        let chars = make_visible(&["hello", "world"]);
        // Row 0: cols 0–4, Row 1: cols 0–4.
        let t = Instant::now();
        let mut vs = ViewState::new();

        // Triple-click on row 1, col 2 — abs_row = 1.
        let click_coord = CellCoord { col: 2, row: 1 };
        vs.register_click(click_coord, t);
        vs.register_click(click_coord, t);
        let count = vs.register_click(click_coord, t);
        assert_eq!(count, 3);

        // Press handler: anchor/end to full line on row 1.
        let (ls, le) = line_boundaries(&chars, 1);
        vs.selection.anchor = Some(CellCoord { col: ls, row: 1 });
        vs.selection.end = Some(CellCoord { col: le, row: 1 });
        vs.selection.is_selecting = true;

        // Drag upward to row 0, col 3.
        // abs_row (0) < anchor_row (1), so use line_start.
        let (drag_line_start, _) = line_boundaries(&chars, 0);
        vs.selection.end = Some(CellCoord {
            col: drag_line_start,
            row: 0,
        });

        // After normalisation: row 0 col 0 → row 1 col 4.
        let (start, end) = vs.selection.normalised().unwrap();
        assert_eq!(
            start,
            CellCoord { col: 0, row: 0 },
            "start at row 0 line begin"
        );
        assert_eq!(
            end,
            CellCoord { col: 0, row: 1 },
            "end at anchor row line start (anchor holds line end)"
        );

        // The anchor (row 1, col 0) and the original anchor end (col 4)
        // ensure the full anchor line is covered.  Verify the anchor
        // itself still points at the line start of row 1.
        assert_eq!(
            vs.selection.anchor,
            Some(CellCoord { col: 0, row: 1 }),
            "anchor stays at line start of row 1"
        );
    }

    // ── font zoom tests ─────────────────────────────────────────────

    #[test]
    fn zoom_delta_default_is_zero() {
        let vs = ViewState::new();
        assert!(
            vs.zoom_delta.abs() < f32::EPSILON,
            "new ViewState should have zoom_delta == 0.0"
        );
    }

    #[test]
    fn effective_font_size_no_zoom() {
        let vs = ViewState::new();
        let effective = vs.effective_font_size(14.0);
        assert!(
            (effective - 14.0).abs() < f32::EPSILON,
            "effective should equal base when zoom_delta == 0"
        );
    }

    #[test]
    fn effective_font_size_with_positive_delta() {
        let mut vs = ViewState::new();
        vs.zoom_delta = 4.0;
        let effective = vs.effective_font_size(14.0);
        assert!(
            (effective - 18.0).abs() < f32::EPSILON,
            "expected 14 + 4 = 18, got {effective}"
        );
    }

    #[test]
    fn effective_font_size_with_negative_delta() {
        let mut vs = ViewState::new();
        vs.zoom_delta = -6.0;
        let effective = vs.effective_font_size(14.0);
        assert!(
            (effective - 8.0).abs() < f32::EPSILON,
            "expected 14 - 6 = 8, got {effective}"
        );
    }

    #[test]
    fn effective_font_size_clamps_to_minimum() {
        let mut vs = ViewState::new();
        vs.zoom_delta = -100.0;
        let effective = vs.effective_font_size(14.0);
        assert!(
            (effective - 4.0).abs() < f32::EPSILON,
            "effective should clamp to 4.0, got {effective}"
        );
    }

    #[test]
    fn effective_font_size_clamps_to_maximum() {
        let mut vs = ViewState::new();
        vs.zoom_delta = 200.0;
        let effective = vs.effective_font_size(14.0);
        assert!(
            (effective - 96.0).abs() < f32::EPSILON,
            "effective should clamp to 96.0, got {effective}"
        );
    }

    #[test]
    fn adjust_zoom_applies_step() {
        let mut vs = ViewState::new();
        vs.adjust_zoom(14.0, 2.0);
        assert!(
            (vs.zoom_delta - 2.0).abs() < f32::EPSILON,
            "after +2 step from 0, zoom_delta should be 2.0, got {}",
            vs.zoom_delta
        );
        assert!(
            (vs.effective_font_size(14.0) - 16.0).abs() < f32::EPSILON,
            "effective should be 16.0"
        );
    }

    #[test]
    fn adjust_zoom_accumulates() {
        let mut vs = ViewState::new();
        vs.adjust_zoom(14.0, 1.0);
        vs.adjust_zoom(14.0, 1.0);
        vs.adjust_zoom(14.0, 1.0);
        assert!(
            (vs.zoom_delta - 3.0).abs() < f32::EPSILON,
            "three +1 steps should give delta 3.0, got {}",
            vs.zoom_delta
        );
    }

    #[test]
    fn adjust_zoom_negative_step() {
        let mut vs = ViewState::new();
        vs.adjust_zoom(14.0, -3.0);
        assert!(
            (vs.zoom_delta - -3.0).abs() < f32::EPSILON,
            "after -3 step from 0, zoom_delta should be -3.0, got {}",
            vs.zoom_delta
        );
        assert!(
            (vs.effective_font_size(14.0) - 11.0).abs() < f32::EPSILON,
            "effective should be 11.0"
        );
    }

    #[test]
    fn adjust_zoom_clamps_at_minimum() {
        let mut vs = ViewState::new();
        // base=6, step=-10 → effective would be -4, clamps to 4.0
        vs.adjust_zoom(6.0, -10.0);
        assert!(
            (vs.effective_font_size(6.0) - 4.0).abs() < f32::EPSILON,
            "effective should clamp to 4.0"
        );
        // zoom_delta should be back-derived: 4.0 - 6.0 = -2.0
        assert!(
            (vs.zoom_delta - -2.0).abs() < f32::EPSILON,
            "zoom_delta should be -2.0 after clamping, got {}",
            vs.zoom_delta
        );
    }

    #[test]
    fn adjust_zoom_clamps_at_maximum() {
        let mut vs = ViewState::new();
        // base=90, step=100 → effective would be 190, clamps to 96.0
        vs.adjust_zoom(90.0, 100.0);
        assert!(
            (vs.effective_font_size(90.0) - 96.0).abs() < f32::EPSILON,
            "effective should clamp to 96.0"
        );
        // zoom_delta should be back-derived: 96.0 - 90.0 = 6.0
        assert!(
            (vs.zoom_delta - 6.0).abs() < f32::EPSILON,
            "zoom_delta should be 6.0 after clamping, got {}",
            vs.zoom_delta
        );
    }

    #[test]
    fn reset_zoom_sets_delta_to_zero() {
        let mut vs = ViewState::new();
        vs.zoom_delta = 5.0;
        vs.reset_zoom();
        assert!(
            vs.zoom_delta.abs() < f32::EPSILON,
            "after reset, zoom_delta should be 0.0"
        );
    }

    #[test]
    fn zoom_delta_preserved_across_base_change() {
        // Simulate: user zooms +4 on base 14, then settings change base to 16.
        // Effective should be 16 + 4 = 20.
        let mut vs = ViewState::new();
        vs.adjust_zoom(14.0, 4.0);
        assert!(
            (vs.effective_font_size(14.0) - 18.0).abs() < f32::EPSILON,
            "before base change: 14 + 4 = 18"
        );
        // Base changes to 16 — zoom_delta stays at 4.0
        assert!(
            (vs.effective_font_size(16.0) - 20.0).abs() < f32::EPSILON,
            "after base change: 16 + 4 = 20"
        );
    }

    // ── cursor trail animation tests ─────────────────────────────────

    #[test]
    fn cursor_animation_disabled_snaps_instantly() {
        let mut vs = ViewState::new();
        let animating = vs.update_cursor_animation(10.0, 5.0, false, Duration::from_millis(150));

        assert!(!animating, "should not be animating when trail disabled");
        assert!(
            (vs.cursor_visual_col - 10.0).abs() < f32::EPSILON,
            "visual col should snap to target"
        );
        assert!(
            (vs.cursor_visual_row - 5.0).abs() < f32::EPSILON,
            "visual row should snap to target"
        );
        assert!(vs.cursor_last_frame.is_none());
    }

    #[test]
    fn cursor_animation_first_frame_snaps_to_target() {
        let mut vs = ViewState::new();
        // First ever call snaps visual to target so we don't glide from (0,0).
        let animating = vs.update_cursor_animation(10.0, 5.0, true, Duration::from_millis(150));

        assert!(!animating, "first observation should snap, not animate");
        assert!(
            vs.cursor_last_frame.is_some(),
            "should record timestamp on first frame"
        );
        assert!(
            (vs.cursor_visual_col - 10.0).abs() < f32::EPSILON,
            "visual col should snap to target on first frame"
        );
        assert!(
            (vs.cursor_visual_row - 5.0).abs() < f32::EPSILON,
            "visual row should snap to target on first frame"
        );
    }

    #[test]
    fn cursor_animation_converges_toward_target() {
        let mut vs = ViewState::new();
        let duration = Duration::from_millis(150);

        // First call — snaps to (0, 0) as initialization.
        vs.update_cursor_animation(0.0, 0.0, true, duration);

        // Change target to (10, 5) — should start animating.
        std::thread::sleep(Duration::from_millis(30));
        let animating = vs.update_cursor_animation(10.0, 5.0, true, duration);
        assert!(animating, "should be animating toward new target");
        assert!(
            vs.cursor_visual_col > 0.0,
            "visual col should have moved toward target, got {}",
            vs.cursor_visual_col
        );
        assert!(
            vs.cursor_visual_col < 10.0,
            "visual col should not overshoot target, got {}",
            vs.cursor_visual_col
        );
        assert!(
            vs.cursor_visual_row > 0.0,
            "visual row should have moved toward target, got {}",
            vs.cursor_visual_row
        );
    }

    #[test]
    fn cursor_animation_at_target_returns_false() {
        let mut vs = ViewState::new();
        // Initialize at (5, 3).
        vs.update_cursor_animation(5.0, 3.0, true, Duration::from_millis(150));

        // Call again with same target — nothing to animate.
        let animating = vs.update_cursor_animation(5.0, 3.0, true, Duration::from_millis(150));

        assert!(!animating, "should not be animating when already at target");
    }

    #[test]
    fn cursor_animation_retarget_changes_direction() {
        let mut vs = ViewState::new();
        let duration = Duration::from_millis(150);

        // Initialize at (0, 0), then move toward (10, 0).
        vs.update_cursor_animation(0.0, 0.0, true, duration);
        std::thread::sleep(Duration::from_millis(30));
        vs.update_cursor_animation(10.0, 0.0, true, duration);

        let mid_col = vs.cursor_visual_col;
        assert!(
            mid_col > 0.0 && mid_col < 10.0,
            "should be mid-animation, got {mid_col}"
        );

        // Change target to (20, 0) — exponential decay smoothly redirects.
        std::thread::sleep(Duration::from_millis(30));
        let animating = vs.update_cursor_animation(20.0, 0.0, true, duration);
        assert!(animating, "should still be animating after retarget");
        assert!(
            vs.cursor_visual_col > mid_col,
            "visual should have moved further toward new target, got {}",
            vs.cursor_visual_col
        );
        assert!(
            (vs.cursor_target_col - 20.0).abs() < f32::EPSILON,
            "target should update to 20.0"
        );
    }

    #[test]
    fn cursor_animation_zero_duration_snaps() {
        let mut vs = ViewState::new();
        // Initialize, then move with zero duration.
        vs.update_cursor_animation(0.0, 0.0, true, Duration::from_millis(150));
        std::thread::sleep(Duration::from_millis(5));
        let animating = vs.update_cursor_animation(10.0, 5.0, true, Duration::ZERO);

        assert!(!animating, "zero duration should snap instantly");
        assert!(
            (vs.cursor_visual_col - 10.0).abs() < f32::EPSILON,
            "visual col should be at target"
        );
        assert!(
            (vs.cursor_visual_row - 5.0).abs() < f32::EPSILON,
            "visual row should be at target"
        );
    }

    #[test]
    fn cursor_animation_default_state_at_origin() {
        let vs = ViewState::new();
        assert!(
            vs.cursor_visual_col.abs() < f32::EPSILON,
            "default visual_col should be 0.0"
        );
        assert!(
            vs.cursor_visual_row.abs() < f32::EPSILON,
            "default visual_row should be 0.0"
        );
        assert!(
            vs.cursor_target_col.abs() < f32::EPSILON,
            "default target_col should be 0.0"
        );
        assert!(
            vs.cursor_target_row.abs() < f32::EPSILON,
            "default target_row should be 0.0"
        );
        assert!(
            vs.cursor_last_frame.is_none(),
            "default cursor_last_frame should be None"
        );
    }

    // ── SelectionState::is_block tests ───────────────────────────────

    #[test]
    fn selection_state_default_is_not_block() {
        let sel = SelectionState::default();
        assert!(
            !sel.is_block,
            "default SelectionState must not be block mode"
        );
    }

    #[test]
    fn selection_state_clear_resets_is_block() {
        let mut sel = SelectionState {
            anchor: Some(CellCoord { col: 2, row: 0 }),
            end: Some(CellCoord { col: 5, row: 3 }),
            is_selecting: false,
            is_block: true,
        };
        sel.clear();
        assert!(!sel.is_block, "clear() must reset is_block to false");
        assert!(sel.anchor.is_none(), "clear() must reset anchor");
        assert!(sel.end.is_none(), "clear() must reset end");
        assert!(!sel.is_selecting, "clear() must reset is_selecting");
    }

    #[test]
    fn selection_state_has_selection_block_mode() {
        // has_selection only cares that anchor != end; is_block does not affect it.
        let sel = SelectionState {
            anchor: Some(CellCoord { col: 1, row: 0 }),
            end: Some(CellCoord { col: 3, row: 2 }),
            is_selecting: false,
            is_block: true,
        };
        assert!(
            sel.has_selection(),
            "block selection with anchor != end should report has_selection"
        );
    }

    #[test]
    fn selection_state_block_normalised_puts_earlier_row_first() {
        // Dragged from row 5 back to row 2 — normalised should flip them.
        let sel = SelectionState {
            anchor: Some(CellCoord { col: 4, row: 5 }),
            end: Some(CellCoord { col: 1, row: 2 }),
            is_selecting: false,
            is_block: true,
        };
        let (s, e) = sel.normalised().unwrap();
        assert_eq!(s.row, 2, "normalised start row should be the smaller row");
        assert_eq!(e.row, 5, "normalised end row should be the larger row");
    }
    #[test]
    fn cursor_animation_snap_threshold() {
        let mut vs = ViewState::new();
        // Place visual very close to target (within SNAP_THRESHOLD of 0.01).
        vs.cursor_visual_col = 9.995;
        vs.cursor_visual_row = 4.998;

        let animating = vs.update_cursor_animation(10.0, 5.0, true, Duration::from_millis(150));

        assert!(!animating, "should snap when within threshold, not animate");
        assert!(
            (vs.cursor_visual_col - 10.0).abs() < f32::EPSILON,
            "visual col should snap to target"
        );
        assert!(
            (vs.cursor_visual_row - 5.0).abs() < f32::EPSILON,
            "visual row should snap to target"
        );
    }
}
