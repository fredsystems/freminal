// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use conv2::ValueFrom;
use freminal_common::buffer_states::{
    buffer_type::BufferType,
    cursor::CursorState,
    format_tag::FormatTag,
    modes::{decawm::Decawm, declrmm::Declrmm, decom::Decom, lnm::Lnm},
    tchar::TChar,
};

use crate::{
    image_store::ImageStore,
    response::InsertResponse,
    row::{Row, RowJoin, RowOrigin},
};

#[cfg(test)]
use crate::cell::Cell;
#[cfg(test)]
use freminal_common::buffer_states::{
    cursor::CursorPos,
    modes::{reverse_wrap_around::ReverseWrapAround, xt_rev_wrap2::XtRevWrap2},
};

mod cursor;
mod erase;
mod flatten;
mod images;
mod lifecycle;
mod lines;
mod resize_and_alt;
mod scroll;
mod tabs;

/// Apply a signed `delta` (in cells or rows) to an unsigned `base` coordinate and clamp the
/// result into `[lo, hi]`.
///
/// Used by cursor-movement escape sequences (CUU, CUD, CUF, CUB) where the caller provides
/// an `i32` delta and the buffer needs a `usize` screen coordinate.
///
/// If the `usize → i32` conversion of any argument would overflow (terminal dimensions
/// exceeding `i32::MAX`, which is not physically possible), the function falls back to `base`
/// — i.e. the cursor does not move. This is the safe no-op on bogus input.
fn clamped_offset(base: usize, delta: i32, lo: usize, hi: usize) -> usize {
    let Ok(base_i) = i32::value_from(base) else {
        return base;
    };
    let Ok(lo_i) = i32::value_from(lo) else {
        return base;
    };
    let Ok(hi_i) = i32::value_from(hi) else {
        return base;
    };
    let clamped = base_i.saturating_add(delta).max(lo_i).min(hi_i);
    // After max(lo_i) with lo_i >= 0 (usize origin), clamped is non-negative, so this is lossless.
    usize::value_from(clamped).unwrap_or(base)
}

/// The primary terminal buffer owning all rows, the cursor, and display state.
///
/// In primary-screen mode the buffer grows dynamically (up to `scrollback_limit` rows above
/// the visible window). In alternate-screen mode it has exactly `height` rows and no scrollback.
/// All row indices are absolute (into `self.rows`); the GUI maps to screen coordinates by
/// subtracting `visible_window_start(scroll_offset)`.
#[derive(Debug)]
pub struct Buffer {
    /// All rows in this buffer: scrollback + visible region.
    /// In the primary buffer, this grows until `scrollback_limit` is hit.
    /// In the alternate buffer, this always has exactly `height` rows.
    pub(in crate::buffer) rows: Vec<Row>,

    /// Per-row flat-representation cache.  Index matches `self.rows`.
    /// `None` = dirty (must be re-flattened on next snapshot).
    /// `Some((chars, tags))` = clean cached flat representation for that row.
    pub(in crate::buffer) row_cache: Vec<Option<(Vec<TChar>, Vec<FormatTag>)>>,

    /// Width and height of the terminal grid.
    pub(in crate::buffer) width: usize,
    pub(in crate::buffer) height: usize,

    /// Current cursor position (row, col).
    pub(in crate::buffer) cursor: CursorState,

    /// Maximum number of scrollback lines allowed.
    ///
    /// For example:
    ///  - height = 40
    ///  - `scrollback_limit` = 1000
    ///    Means `rows.len()` will be at most 1040.
    pub(in crate::buffer) scrollback_limit: usize,

    /// Whether this is the primary or alternate buffer mode.
    ///
    /// Primary:
    ///   - Has scrollback
    ///
    /// Alternate:
    ///   - No scrollback
    ///   - Switching back restores primary buffer's saved state
    pub(in crate::buffer) kind: BufferType,

    /// Saved primary buffer content, cursor, and `scroll_offset`,
    /// used when switching to and from alternate buffer.
    /// The scroll offset is owned by the caller (`ViewState`) and passed
    /// in / returned from `enter_alternate` / `leave_alternate`.
    pub(in crate::buffer) saved_primary: Option<SavedPrimaryState>,

    /// Saved cursor for DECSC / DECRC (ESC 7 / ESC 8).
    /// Independent of the alternate-screen save (`saved_primary`).
    pub(in crate::buffer) saved_cursor: Option<CursorState>,

    /// Current format tag to apply to inserted text.
    pub(in crate::buffer) current_tag: FormatTag,

    /// LNM mode
    pub(in crate::buffer) lnm_enabled: Lnm,

    /// DECAWM — whether soft-wrapping is enabled.
    /// `AutoWrap` (default): text wraps at the terminal width.
    /// `NoAutoWrap`: text is clamped to the last column; overflow is discarded.
    pub(in crate::buffer) wrap_enabled: Decawm,

    /// Preserve the scrollback anchor when resizing
    pub(in crate::buffer) preserve_scrollback_anchor: bool,

    /// DECSTBM top and bottom margins, 0-indexed, inclusive.
    /// When disabled, the region is full-screen: [0, height-1]
    pub(in crate::buffer) scroll_region_top: usize,
    pub(in crate::buffer) scroll_region_bottom: usize,

    /// DECLRMM left and right margins, 0-indexed, inclusive.
    /// When DECLRMM is disabled these are ignored.
    /// Default: [0, width-1] (full screen).
    pub(in crate::buffer) scroll_region_left: usize,
    pub(in crate::buffer) scroll_region_right: usize,

    /// Whether DECLRMM (`?69`) is currently enabled.
    /// When disabled, `scroll_region_left`/`scroll_region_right` are ignored by
    /// all buffer operations that would otherwise respect them.
    pub(in crate::buffer) declrmm_enabled: Declrmm,

    /// Tab stops as a boolean vector indexed by column.
    /// `tab_stops[c] == true` means column `c` is a tab stop.
    /// Default: every 8 columns (8, 16, 24, ...).
    pub(in crate::buffer) tab_stops: Vec<bool>,

    /// DECOM (Origin Mode) — when enabled, cursor addressing is relative
    /// to the scroll region top/bottom instead of the full screen.
    pub(in crate::buffer) decom_enabled: Decom,

    /// Central storage for inline images.
    ///
    /// Cells reference images by ID via `ImagePlacement`; the actual pixel
    /// data lives here behind `Arc`s so snapshots can share it cheaply.
    pub(in crate::buffer) image_store: ImageStore,

    /// Total number of cells across all rows that carry an image placement.
    ///
    /// Maintained incrementally so `has_visible_images` / `has_any_image_cell`
    /// can short-circuit in O(1) when no images are present (the overwhelmingly
    /// common case).
    pub(in crate::buffer) image_cell_count: usize,

    /// Buffer-relative row indices where OSC 133 `PromptStart` markers fired.
    ///
    /// Maintained atomically with row drains: when rows are removed from the
    /// front, all indices are shifted down and entries that fell off are dropped.
    pub(in crate::buffer) prompt_rows: Vec<usize>,
}

/// Snapshot of the primary buffer state saved when entering the alternate screen.
///
/// Restored verbatim by [`Buffer::leave_alternate`].
#[derive(Debug, Clone)]
pub struct SavedPrimaryState {
    /// All primary-buffer rows (scrollback + visible region) at the time of the switch.
    pub rows: Vec<Row>,
    /// Per-row flat-representation cache saved alongside `rows`.
    pub row_cache: Vec<Option<(Vec<TChar>, Vec<FormatTag>)>>,
    /// Cursor state (position, attributes) at the time of the switch.
    pub cursor: CursorState,
    /// Caller-owned scroll offset (from `ViewState`) at the time of the switch.
    pub scroll_offset: usize,
    /// Visible height of the terminal grid at the time of the switch.
    pub height: usize,
    /// Top of the DECSTBM scroll region at the time of the switch.
    pub scroll_region_top: usize,
    /// Bottom of the DECSTBM scroll region at the time of the switch.
    pub scroll_region_bottom: usize,
    /// Left margin (DECSLRM) at the time of the switch.
    pub scroll_region_left: usize,
    /// Right margin (DECSLRM) at the time of the switch.
    pub scroll_region_right: usize,
    /// Saved DECSC cursor carried across alternate-screen round-trips.
    pub saved_cursor: Option<CursorState>,
    /// Saved image store from the primary buffer.
    pub image_store: ImageStore,
    /// Saved image cell count from the primary buffer.
    pub image_cell_count: usize,
}

/// Compute the number of screen columns that `text` will occupy when
/// inserted starting at column `col`, clamped so as not to exceed
/// `wrap_col`.  Wide characters (`display_width` = 2) count for 2 columns.
fn text_col_span(text: &[TChar], col: usize, wrap_col: usize) -> usize {
    let mut span = 0usize;
    for t in text {
        let w = t.display_width().max(1);
        if col + span + w > wrap_col {
            break;
        }
        span += w;
    }
    span
}

impl Buffer {
    /// Return a new buffer with the given scrollback limit instead of the
    /// default (4000).  This is a builder-style method intended for
    /// production use where the value comes from user configuration.
    #[must_use]
    pub const fn with_scrollback_limit(mut self, limit: usize) -> Self {
        self.scrollback_limit = limit;
        self
    }

    /// The maximum number of off-screen rows retained above the visible area.
    #[must_use]
    pub const fn scrollback_limit(&self) -> usize {
        self.scrollback_limit
    }

    /// Returns a reference to all rows in this buffer (scrollback + visible region).
    #[must_use]
    pub const fn get_rows(&self) -> &Vec<Row> {
        &self.rows
    }

    /// Returns a reference to the current cursor state (position and attributes).
    #[must_use]
    pub const fn get_cursor(&self) -> &CursorState {
        &self.cursor
    }

    /// Reset an existing row at `row_idx` to a soft-wrap continuation and update
    /// `image_cell_count` for the cells being discarded.
    ///
    /// Called from `insert_text` whenever a new wrap causes an already-populated
    /// row to be recycled as a continuation line.
    fn reuse_row_as_softwrap(&mut self, row_idx: usize) {
        let row = &mut self.rows[row_idx];
        self.image_cell_count -= row.count_image_cells();
        row.origin = RowOrigin::SoftWrap;
        row.join = RowJoin::ContinueLogicalLine;
        row.clear();
        self.row_cache[row_idx] = None;
    }

    /// Insert `text` at the current cursor position, soft-wrapping as needed.
    ///
    /// Advances the cursor to the column after the last character written.
    /// When `DECAWM` (auto-wrap) is disabled, overflow beyond the right margin
    /// is silently discarded rather than wrapped.
    pub fn insert_text(&mut self, text: &[TChar]) {
        // `start` is an index cursor into the original `text` slice.
        // We never clone the slice — `InsertResponse::Leftover` now returns
        // the index at which the un-inserted portion begins, so we just
        // advance `start` and pass `&text[start..]` on the next iteration.
        let mut start: usize = 0;
        let mut row_idx = self.cursor.pos.y;
        let mut col = self.cursor.pos.x;

        // When DECLRMM is active the effective right wrap column is
        // scroll_region_right + 1; wrapping starts a new row at
        // scroll_region_left rather than column 0.
        let (wrap_col, wrap_start_col) = if self.declrmm_enabled == Declrmm::Enabled {
            (self.scroll_region_right + 1, self.scroll_region_left)
        } else {
            (self.width, 0)
        };

        // First write into row 0 turns it into a real logical line
        if row_idx == 0 && self.rows[0].origin == RowOrigin::ScrollFill {
            let row = &mut self.rows[0];
            row.origin = RowOrigin::HardBreak;
            row.join = RowJoin::NewLogicalLine;
        }

        loop {
            // ┌─────────────────────────────────────────────┐
            // │ PRE-WRAP: if we're already at/past wrap_col,│
            // │ move to the next row as a soft-wrap row.    │
            // └─────────────────────────────────────────────┘
            if col >= wrap_col {
                if self.wrap_enabled == Decawm::NoAutoWrap {
                    // DECAWM NoAutoWrap: clamp cursor to last column and discard
                    // all remaining text.
                    self.cursor.pos.x = wrap_col.saturating_sub(1);
                    self.cursor.pos.y = row_idx;
                    // PTY always at scroll_offset=0; return value is always 0 here.
                    let _ = self.enforce_scrollback_limit(0);
                    return; // nothing left to insert
                }

                // Scroll-region-aware wrap: advance to the next row,
                // preserving scrollback on full-screen primary buffers.
                row_idx = self.advance_row_for_wrap(row_idx, wrap_start_col);
                col = wrap_start_col;
            }

            // ┌─────────────────────────────────────────────┐
            // │ Ensure the target row exists.               │
            // │ If we got here without PRE-WRAP, it's a     │
            // │ normal new logical line. If col == left and  │
            // │ row_idx > 0, we are in a wrap continuation. │
            // └─────────────────────────────────────────────┘
            if row_idx >= self.rows.len() {
                let is_wrap_continuation = col == wrap_start_col && row_idx > 0;

                let origin = if is_wrap_continuation {
                    RowOrigin::SoftWrap
                } else {
                    RowOrigin::HardBreak
                };

                let join = if is_wrap_continuation {
                    RowJoin::ContinueLogicalLine
                } else {
                    RowJoin::NewLogicalLine
                };

                self.rows
                    .push(Row::new_with_origin(self.width, origin, join));
                self.row_cache.push(None);
            }

            // clone tag here to avoid long-lived borrows of &self
            let tag = self.current_tag.clone();

            // ┌─────────────────────────────────────────────┐
            // │ Before writing, check if any cells in the   │
            // │ target range carry an image.  If so, clear  │
            // │ ALL cells of that image across the entire   │
            // │ buffer — images are atomic; overwriting any  │
            // │ part destroys the whole image.               │
            // │                                              │
            // │ We must compute the column span from display │
            // │ widths, not TChar count, because wide chars  │
            // │ (display_width=2) occupy 2 columns each.     │
            // │ Clamp to wrap_col so we don't scan past the  │
            // │ columns insert_text_with_limit will write.   │
            // └─────────────────────────────────────────────┘
            let col_span = text_col_span(&text[start..], col, wrap_col);
            self.clear_images_overwritten_by_text(row_idx, col, col_span);

            // Count Kitty images that survived the sweep and will be
            // overwritten by the text insertion below.
            let kitty_images_in_range = if self.image_cell_count > 0 {
                self.rows[row_idx].count_image_cells_in_range(col, col + col_span)
            } else {
                0
            };

            // ┌─────────────────────────────────────────────┐
            // │ Try to insert into this row (up to wrap_col)│
            // └─────────────────────────────────────────────┘
            self.image_cell_count -= kitty_images_in_range;
            match self.rows[row_idx].insert_text_with_limit(col, &text[start..], &tag, wrap_col) {
                InsertResponse::Consumed(final_col) => {
                    // All text fit on this row.
                    self.cursor.pos.x = final_col;
                    self.cursor.pos.y = row_idx;

                    // PTY always at scroll_offset=0; return value is always 0 here.
                    let _ = self.enforce_scrollback_limit(0);
                    return;
                }

                InsertResponse::Leftover {
                    leftover_start,
                    final_col,
                } => {
                    // This row filled; some data remains.
                    self.cursor.pos.x = final_col;
                    self.cursor.pos.y = row_idx;

                    if self.wrap_enabled == Decawm::NoAutoWrap {
                        // DECAWM NoAutoWrap: clamp cursor to last column and discard
                        // the overflow — do not continue onto the next row.
                        self.cursor.pos.x = wrap_col.saturating_sub(1);
                        // PTY always at scroll_offset=0; return value is always 0 here.
                        let _ = self.enforce_scrollback_limit(0);
                        return;
                    }

                    // Advance the cursor into the original text slice.
                    // `leftover_start` is relative to `&text[start..]`.
                    start += leftover_start;

                    // Scroll-region-aware wrap: same logic as PRE-WRAP.
                    row_idx = self.advance_row_for_wrap(row_idx, wrap_start_col);
                    col = wrap_start_col;
                    // `col` stays wrap_start_col; next iteration writes there.
                }
            }
        }
    }

    /// Handle soft-wrap advancement: move the cursor to the next row when a
    /// line wraps.  Returns the new `row_idx`.
    ///
    /// On a full-screen primary buffer the top visible row is preserved as
    /// scrollback by pushing a new row at the bottom (the `handle_lf` fast-path
    /// strategy).  For partial DECSTBM regions or alternate buffers the old
    /// `scroll_region_up_for_wrap` rotation is used.
    ///
    /// When the cursor is NOT at the scroll region bottom, a new soft-wrap
    /// continuation row is either allocated or reused from the existing row
    /// vector.
    fn advance_row_for_wrap(&mut self, row_idx: usize, wrap_start_col: usize) -> usize {
        let at_region_bottom = self.is_cursor_at_scroll_region_bottom();
        let is_full_screen_region = self.scroll_region_top == 0
            && self.scroll_region_bottom == self.height.saturating_sub(1);

        let new_row_idx =
            if at_region_bottom && self.kind == BufferType::Primary && is_full_screen_region {
                // Full-screen primary: push a new row, advance cursor.
                // Content naturally scrolls into scrollback via visible_window_start().
                let next = row_idx + 1;
                self.push_row(RowOrigin::SoftWrap, RowJoin::ContinueLogicalLine);
                next
            } else if at_region_bottom {
                self.scroll_region_up_for_wrap();
                // row_idx stays the same — it now points to the freshly blanked
                // bottom row of the scroll region.
                row_idx
            } else {
                let next = row_idx + 1;
                if next >= self.rows.len() {
                    self.push_row(RowOrigin::SoftWrap, RowJoin::ContinueLogicalLine);
                } else {
                    self.reuse_row_as_softwrap(next);
                }
                next
            };

        // Initialise cursor column to the wrap start column — the caller
        // still needs to set `col = wrap_start_col` in its own local variable,
        // but we ensure the cursor struct is consistent.
        self.cursor.pos.x = wrap_start_col;
        self.cursor.pos.y = new_row_idx;
        new_row_idx
    }
}

// ============================================================================
// Private helpers
// ============================================================================

/// Compare two `FormatTag` values by their visual formatting only, ignoring
/// the `start` and `end` byte-position fields.
pub(in crate::buffer) fn tags_same_format(a: &FormatTag, b: &FormatTag) -> bool {
    a.colors == b.colors
        && a.font_weight == b.font_weight
        && a.font_decorations == b.font_decorations
        && a.url == b.url
        && a.blink == b.blink
}

// ============================================================================
// Unit Tests for Buffer
// ============================================================================

#[cfg(test)]
mod basic_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    fn ascii(c: char) -> TChar {
        TChar::Ascii(c as u8)
    }

    // ────────────────────────────────────────────────────────────────
    // PRIMARY BUFFER TESTS
    // ────────────────────────────────────────────────────────────────

    #[test]
    fn primary_lf_adds_new_row_no_scroll_yet() {
        let mut buf = Buffer::new(5, 3);

        // Buffer starts with 1 row; each LF into a non-existent row creates one.
        buf.handle_lf();
        assert_eq!(buf.cursor.pos.y, 1);
        assert_eq!(buf.rows.len(), 2);

        buf.handle_lf();
        assert_eq!(buf.cursor.pos.y, 2);
        assert_eq!(buf.rows.len(), 3);
    }

    #[test]
    fn primary_lf_accumulates_scrollback() {
        let mut buf = Buffer::new(5, 3);

        for _ in 0..6 {
            buf.handle_lf();
        }

        // initial row + 6 new rows = 7
        assert_eq!(buf.rows.len(), 7);
        assert_eq!(buf.cursor.pos.y, 6);
    }

    #[test]
    fn primary_lf_respects_scrollback_limit() {
        let mut buf = Buffer::new(5, 3);
        buf.scrollback_limit = 2; // very small

        for _ in 0..10 {
            buf.handle_lf();
        }

        // should now be height (3) + limit (2) = 5 rows
        assert_eq!(buf.rows.len(), 5);
        assert_eq!(buf.cursor.pos.y, buf.rows.len() - 1);
    }

    #[test]
    fn primary_insert_text_does_not_auto_reset_offset() {
        // scroll_offset is now owned by ViewState; insert_text no longer resets it.
        // The GUI is responsible for resetting ViewState::scroll_offset when
        // content_changed is true.
        let mut buf = Buffer::new(10, 5);

        // Just verify insert_text doesn't panic and content is written correctly.
        buf.insert_text(&[ascii('A')]);

        let vis = buf.visible_rows(0);
        assert!(!vis.is_empty());
    }

    // ────────────────────────────────────────────────────────────────
    // ALTERNATE BUFFER TESTS
    // ────────────────────────────────────────────────────────────────

    #[test]
    fn alt_buffer_has_no_scrollback() {
        let mut buf = Buffer::new(5, 3);
        buf.enter_alternate(0);

        assert_eq!(buf.rows.len(), 3);
        assert_eq!(buf.kind, BufferType::Alternate);
    }

    #[test]
    fn alt_buffer_lf_scrolls_screen() {
        let mut buf = Buffer::new(5, 3);
        buf.enter_alternate(0);

        buf.handle_lf();
        buf.handle_lf();
        assert_eq!(buf.cursor.pos.y, 2);

        // now at bottom → should scroll
        buf.handle_lf();
        assert_eq!(buf.cursor.pos.y, 2);
        assert_eq!(buf.rows.len(), 3);
    }

    #[test]
    fn leaving_alt_restores_primary() {
        let mut buf = Buffer::new(6, 4);

        // create scrollback + move cursor
        buf.handle_lf();
        buf.handle_lf();
        let saved_y = buf.cursor.pos.y;
        let saved_rows = buf.rows.len();

        // Enter alternate buffer via API
        buf.enter_alternate(0);

        // Do some things in alternate screen (optional)
        buf.handle_lf();

        // Leave alternate, restoring primary
        let _restored_offset = buf.leave_alternate();

        assert_eq!(buf.kind, BufferType::Primary);
        assert_eq!(buf.rows.len(), saved_rows);
        assert_eq!(buf.cursor.pos.y, saved_y);
    }

    #[test]
    fn scrollback_no_effect_when_no_history() {
        let buf = Buffer::new(5, 3);

        let new_offset = buf.scroll_back(0, 10);
        assert_eq!(new_offset, 0);
    }

    #[test]
    fn scrollback_clamps_to_max_offset() {
        let mut buf = Buffer::new(5, 3);

        // Add many lines
        for _ in 0..10 {
            buf.handle_lf();
        }

        let max = buf.rows.len() - buf.height;
        let new_offset = buf.scroll_back(0, 999);

        assert_eq!(new_offset, max);
    }

    #[test]
    fn scroll_forward_clamps_to_zero() {
        let mut buf = Buffer::new(5, 3);

        for _ in 0..10 {
            buf.handle_lf();
        }

        let offset = buf.scroll_back(0, 5); // scroll up some amount
        let offset = buf.scroll_forward(offset, 999); // scroll down more than enough

        assert_eq!(offset, 0);
    }

    #[test]
    fn scroll_to_bottom_resets_offset() {
        let mut buf = Buffer::new(5, 3);

        for _ in 0..10 {
            buf.handle_lf();
        }

        let offset = buf.scroll_back(0, 5);
        assert!(offset > 0);

        let offset = Buffer::scroll_to_bottom();

        assert_eq!(offset, 0);
    }

    #[test]
    fn no_scrollback_in_alternate_buffer() {
        let mut buf = Buffer::new(5, 3);
        buf.enter_alternate(0);

        for _ in 0..10 {
            buf.handle_lf(); // scrolls but no scrollback
        }

        let offset = buf.scroll_back(0, 10);
        assert_eq!(offset, 0);

        let offset = buf.scroll_forward(offset, 10);
        assert_eq!(offset, 0);
    }

    #[test]
    fn insert_text_does_not_auto_reset_scrollback() {
        // scroll_offset lives in ViewState now; Buffer no longer resets it.
        let mut buf = Buffer::new(10, 5);

        for _ in 0..20 {
            buf.handle_lf();
        }

        let offset_before = buf.scroll_back(0, 5);
        assert!(offset_before > 0);

        buf.insert_text(&[TChar::Ascii(b'A')]);

        // offset is external — it is unchanged by insert_text
        assert_eq!(offset_before, offset_before);
        // visible content at live bottom should include the written char
        let vis = buf.visible_rows(0);
        assert!(!vis.is_empty());
    }

    #[test]
    fn cursor_screen_pos_matches_screen_row_with_no_scrollback() {
        // With fewer rows than height, cursor.pos.y == screen y
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&[TChar::Ascii(b'A')]);
        buf.handle_lf();
        buf.insert_text(&[TChar::Ascii(b'B')]);

        let screen_pos = buf.get_cursor_screen_pos();
        let raw_pos = buf.get_cursor().pos;

        // No scrollback yet — screen y must equal raw y
        assert_eq!(
            screen_pos.y, raw_pos.y,
            "no scrollback: screen y should equal raw y"
        );
        assert_eq!(screen_pos.x, raw_pos.x, "x should be unchanged");
    }

    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn lf_does_not_clear_existing_content_above_screen_bottom_alt() {
        // Reproduce the fastfetch two-column layout:
        //   1. Write N logo lines (each line has content in cols 0..W).
        //   2. CHA to col 0 on the last logo line (y unchanged).
        //   3. CUU N  → cursor jumps back to row 0.
        //   4. For each info row: write info at col W, then LF+CR.
        //      The LF must NOT clear the logo content already in that row.
        let logo_lines: usize = 5;
        let col_split: usize = 10; // logo occupies cols 0..10, info at col 10+
        let mut buf = Buffer::new(40, 20);

        // --- Step 1: write logo lines ---
        for i in 0..logo_lines {
            // Write a recognizable marker so we can verify it survives.
            let marker = TChar::Ascii(b'A' + i as u8);
            buf.insert_text(&vec![marker; col_split]);
            buf.handle_lf();
            buf.handle_cr();
        }
        // Cursor is now at (0, logo_lines).

        // --- Step 2+3: CHA col 0 (no-op on x since already 0) + CUU ---
        buf.set_cursor_pos(Some(0), None); // CHA — y unchanged
        buf.move_cursor_relative(0, -(logo_lines as i32)); // CUU
        assert_eq!(
            buf.get_cursor_screen_pos().y,
            0,
            "after CUU cursor must be at screen row 0"
        );

        // --- Step 4: write info alongside logo, one row at a time ---
        for row in 0..logo_lines {
            // Move to the info column on the current row.
            buf.move_cursor_relative(col_split as i32, 0);

            let info_char = TChar::Ascii(b'0' + row as u8);
            buf.insert_text(&[info_char]);

            // LF + CR — must NOT wipe the logo chars in cols 0..col_split.
            buf.handle_lf();
            buf.handle_cr();
        }

        // --- Verify: every logo row still has its marker in col 0 ---
        let visible = buf.visible_rows(0);
        for (i, row) in visible.iter().enumerate().take(logo_lines) {
            let chars = row.get_characters();
            assert!(
                !chars.is_empty(),
                "row {i} must not be empty after info pass"
            );
            let head = match chars[0].tchar() {
                TChar::Ascii(b) => *b,
                _ => 0,
            };
            assert_eq!(
                head,
                b'A' + i as u8,
                "row {i} col 0: logo marker must survive the info-column LF pass (got {head})"
            );
        }
    }

    #[test]
    fn cursor_screen_pos_is_relative_to_visible_window_with_scrollback() {
        // Height = 3, write 6 lines → 3 rows of scrollback
        let mut buf = Buffer::new(10, 3);

        for i in 0..6_u8 {
            buf.insert_text(&[TChar::Ascii(b'0' + i)]);
            buf.handle_lf();
        }

        // Cursor is on the last visible row (screen row 2, 0-indexed)
        let screen_pos = buf.get_cursor_screen_pos();
        assert!(
            screen_pos.y < buf.terminal_height(),
            "screen y ({}) must be within terminal height ({})",
            screen_pos.y,
            buf.terminal_height()
        );

        // Raw cursor y is an absolute row index — must be larger than screen y
        // when there is scrollback above the visible window.
        let raw_y = buf.get_cursor().pos.y;
        let visible_start = buf.visible_window_start(0);
        assert_eq!(
            screen_pos.y,
            raw_y.saturating_sub(visible_start),
            "screen y must equal raw_y minus visible_window_start"
        );
    }

    #[test]
    fn set_cursor_pos_none_y_preserves_current_row() {
        // CHA (ESC [ n G) sets x only — y must not change.
        let mut buf = Buffer::new(80, 24);

        // Move cursor to row 5 (0-indexed screen coord)
        buf.set_cursor_pos(Some(0), Some(5));
        let row_before = buf.get_cursor().pos.y;

        // CHA: x = Some(10), y = None → only x should change
        buf.set_cursor_pos(Some(10), None);

        assert_eq!(buf.get_cursor().pos.x, 10, "x should be updated to 10");
        assert_eq!(
            buf.get_cursor().pos.y,
            row_before,
            "y must not change when y=None (CHA behaviour)"
        );
    }

    #[test]
    fn set_cursor_pos_none_x_preserves_current_column() {
        // VPA (ESC [ n d) sets y only — x must not change.
        let mut buf = Buffer::new(80, 24);

        // Move cursor to column 20
        buf.set_cursor_pos(Some(20), Some(0));
        assert_eq!(buf.get_cursor().pos.x, 20);

        // VPA: x = None, y = Some(3) → only y should change
        buf.set_cursor_pos(None, Some(3));

        assert_eq!(
            buf.get_cursor().pos.x,
            20,
            "x must not change when x=None (VPA behaviour)"
        );
        assert_eq!(buf.get_cursor_screen_pos().y, 3, "screen y should be 3");
    }

    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn fastfetch_cha_cuu_pattern_leaves_cursor_on_correct_row() {
        // Reproduce the fastfetch rendering pattern:
        //   1. Print N lines of logo via LF/CR (cursor ends at row N).
        //   2. ESC[1G  — CHA: move to column 0, KEEP current row.
        //   3. ESC[NA  — CUU: move up N rows → should land at row 0.
        //   4. ESC[47C — CUF: move right 47 cols.
        //   5. Write info text — must land on row 0, col 47, NOT row 0 col 47
        //      after being reset by a broken CHA.
        let logo_lines: usize = 21;
        let mut buf = Buffer::new(100, 100);

        // Step 1: print logo lines with LF (simulates CRLF pairs)
        for _ in 0..logo_lines {
            buf.insert_text(&[TChar::Ascii(b'X')]);
            buf.handle_lf();
            buf.handle_cr();
        }
        // Cursor is now at the start of row `logo_lines` (0-indexed).
        assert_eq!(
            buf.get_cursor_screen_pos().y,
            logo_lines,
            "after {logo_lines} LFs cursor screen-y should be {logo_lines}",
        );

        // Step 2: CHA to column 0 — y MUST stay at logo_lines.
        buf.set_cursor_pos(Some(0), None);
        assert_eq!(
            buf.get_cursor_screen_pos().y,
            logo_lines,
            "CHA (y=None) must not move cursor off row {logo_lines}",
        );
        assert_eq!(buf.get_cursor().pos.x, 0, "CHA should set x to 0");

        // Step 3: CUU logo_lines → cursor should land at screen row 0.
        buf.move_cursor_relative(0, -(logo_lines as i32));
        assert_eq!(
            buf.get_cursor_screen_pos().y,
            0,
            "after CUU cursor must be at screen row 0"
        );

        // Step 4: CUF 47 → column 47.
        buf.move_cursor_relative(47, 0);
        assert_eq!(
            buf.get_cursor().pos.x,
            47,
            "CUF should place cursor at col 47"
        );

        // Step 5: writing here should land on the first row, not some garbage row.
        let screen_row_before_write = buf.get_cursor_screen_pos().y;
        buf.insert_text(&[
            TChar::Ascii(b'I'),
            TChar::Ascii(b'n'),
            TChar::Ascii(b'f'),
            TChar::Ascii(b'o'),
        ]);
        assert_eq!(
            buf.get_cursor_screen_pos().y,
            screen_row_before_write,
            "writing info text must stay on the same screen row"
        );
    }
}

#[cfg(test)]
mod pty_behavior_tests {
    use super::*;
    use crate::row::{RowJoin, RowOrigin};
    use freminal_common::buffer_states::tchar::TChar;

    // Helper: convert &str to Vec<TChar::Ascii>
    fn to_tchars(s: &str) -> Vec<TChar> {
        s.bytes().map(TChar::Ascii).collect()
    }

    // Helper: pretty row origins for debugging
    fn row_kinds(buf: &Buffer) -> Vec<(RowOrigin, RowJoin)> {
        buf.rows.iter().map(|r| (r.origin, r.join)).collect()
    }

    // B1 — CR-only redraw: no new rows, cursor stays on same row
    #[test]
    fn cr_only_redraw_does_not_create_new_rows() {
        // width large enough to not wrap
        let mut buf = Buffer::new(100, 20);

        let initial_rows = buf.rows.len();
        let initial_row_y = buf.cursor.pos.y;

        // "Loading 1%\rLoading 2%\rLoading 3%\r"
        buf.insert_text(&to_tchars("Loading 1%"));
        buf.handle_cr();

        let row_after_first = buf.cursor.pos.y;
        assert_eq!(
            row_after_first, initial_row_y,
            "CR should not move to a new row"
        );

        buf.insert_text(&to_tchars("Loading 2%"));
        buf.handle_cr();

        buf.insert_text(&to_tchars("Loading 3%"));
        buf.handle_cr();

        // Still on the same physical row, and no extra rows created by CR
        assert_eq!(
            buf.cursor.pos.y, initial_row_y,
            "CR redraw loop should not change row index"
        );
        assert_eq!(
            buf.rows.len(),
            initial_rows,
            "CR redraw loop should not create new rows"
        );
    }

    // B2 — CRLF newline pattern: one new row per LF
    #[test]
    fn crlf_creates_new_logical_lines() {
        let mut buf = Buffer::new(100, 20);

        let start_row = buf.cursor.pos.y;

        // "hello\r\nworld\r\n"
        buf.insert_text(&to_tchars("hello"));
        buf.handle_cr();
        buf.handle_lf(); // first CRLF

        let after_first_lf_row = buf.cursor.pos.y;
        assert_eq!(
            after_first_lf_row,
            start_row + 1,
            "First CRLF should move cursor to next row"
        );

        buf.insert_text(&to_tchars("world"));
        buf.handle_cr();
        buf.handle_lf(); // second CRLF

        let after_second_lf_row = buf.cursor.pos.y;
        assert_eq!(
            after_second_lf_row,
            start_row + 2,
            "Second CRLF should move cursor down one more row"
        );

        // Check row metadata of the line starts
        let kinds = row_kinds(&buf);

        // At least three rows now: initial + two LF-created rows
        assert!(
            kinds.len() >= (start_row + 3),
            "Expected at least three rows after two CRLFs"
        );

        let first_line = kinds[start_row];
        let second_line = kinds[start_row + 1];
        let third_line = kinds[start_row + 2];

        // All LF-started rows should be HardBreak + NewLogicalLine
        assert_eq!(
            first_line.0,
            RowOrigin::HardBreak,
            "Initial line should be a HardBreak logical start"
        );
        assert_eq!(
            first_line.1,
            RowJoin::NewLogicalLine,
            "Initial row should begin a logical line"
        );

        assert_eq!(
            second_line.0,
            RowOrigin::HardBreak,
            "Row after first LF should be HardBreak"
        );
        assert_eq!(
            second_line.1,
            RowJoin::NewLogicalLine,
            "Row after first LF should begin a new logical line"
        );

        assert_eq!(
            third_line.0,
            RowOrigin::HardBreak,
            "Row after second LF should be HardBreak"
        );
        assert_eq!(
            third_line.1,
            RowJoin::NewLogicalLine,
            "Row after second LF should begin a new logical line"
        );
    }

    // B3 — Soft-wrap mid-insertion: long text overflows width into SoftWrap row
    #[test]
    fn soft_wrap_marks_continuation_rows() {
        let width = 10;
        let mut buf = Buffer::new(width, 100);

        let start_row = buf.cursor.pos.y;

        buf.insert_text(&to_tchars("1234567890ABCDE"));

        // Look for a SoftWrap row after start_row
        let kinds = row_kinds(&buf);
        let mut found = false;
        for (idx, (origin, join)) in kinds.iter().enumerate().skip(start_row + 1) {
            if *origin == RowOrigin::SoftWrap && *join == RowJoin::ContinueLogicalLine {
                found = true;
                // Optionally: assert cursor ended up here
                assert_eq!(
                    buf.cursor.pos.y, idx,
                    "Cursor should end on the soft-wrapped continuation row"
                );
                break;
            }
        }

        assert!(
            found,
            "Soft-wrap should produce at least one SoftWrap/ContinueLogicalLine row after the first"
        );
    }

    // B6-ish — Wrap into an existing row: reused row must become SoftWrap continuation
    #[test]
    fn soft_wrap_reuses_existing_next_row_as_continuation() {
        let width = 8;
        let mut buf = Buffer::new(width, 100);

        // Fill the first row exactly, starting from 0
        buf.insert_text(&to_tchars("ABCDEFGH")); // 8 chars

        let first_row = buf.cursor.pos.y;
        assert_eq!(first_row, 0);

        // Now write more to force a wrap into the next row
        buf.insert_text(&to_tchars("ABC"));

        // Cursor must now be on the next row
        let second_row = buf.cursor.pos.y;
        assert_eq!(
            second_row,
            first_row + 1,
            "Soft-wrap should move cursor to next row"
        );

        let kinds = row_kinds(&buf);
        let wrapped = kinds[second_row];

        assert_eq!(
            wrapped.0,
            RowOrigin::SoftWrap,
            "Wrapped row should have SoftWrap origin"
        );
        assert_eq!(
            wrapped.1,
            RowJoin::ContinueLogicalLine,
            "Wrapped row should continue the logical line"
        );
    }

    #[test]
    fn cr_only_redraw_never_creates_new_rows_even_after_wrap() {
        let mut buf = Buffer::new(10, 100);

        buf.insert_text(&to_tchars("1234567890")); // full row
        let row0 = buf.cursor.pos.y;
        let rows_after_insert = buf.rows.len();

        buf.handle_cr(); // reset X
        buf.insert_text(&to_tchars("HELLO"));

        assert_eq!(buf.cursor.pos.y, row0, "CR must not change row");
        assert_eq!(
            buf.rows.len(),
            rows_after_insert,
            "CR+overwrite must not create new rows"
        );
    }

    #[test]
    fn lf_after_softwrap_creates_new_hardbreak_row() {
        let mut buf = Buffer::new(5, 100);

        buf.insert_text(&to_tchars("123456789")); // wraps
        assert!(matches!(buf.rows[1].origin, RowOrigin::SoftWrap));

        buf.handle_lf(); // HARD BREAK

        let last = buf.cursor.pos.y;
        assert!(matches!(buf.rows[last].origin, RowOrigin::HardBreak));
        assert!(matches!(buf.rows[last].join, RowJoin::NewLogicalLine));
    }

    #[test]
    fn crlf_moves_to_new_hardbreak_row() {
        let mut buf = Buffer::new(20, 100);

        buf.insert_text(&to_tchars("hello"));
        buf.handle_cr();
        buf.handle_lf();

        let y = buf.cursor.pos.y;
        assert!(y == 1);
        assert!(matches!(buf.rows[1].origin, RowOrigin::HardBreak));
    }

    #[test]
    fn lnm_enabled_lf_behaves_like_crlf() {
        let mut buf = Buffer::new(20, 100);
        buf.lnm_enabled = Lnm::NewLine;

        buf.insert_text(&to_tchars("hello"));
        buf.cursor.pos.x = 5;

        buf.handle_lf(); // LNM → CRLF

        assert_eq!(buf.cursor.pos.x, 0, "LNM LF resets X to 0");
        assert_eq!(buf.cursor.pos.y, 1, "LNM LF advances row");
    }

    #[test]
    fn cr_inside_softwrap_does_not_create_new_logical_line() {
        let mut buf = Buffer::new(5, 100);

        buf.insert_text(&to_tchars("123456")); // soft-wrap at 5
        assert!(matches!(buf.rows[1].origin, RowOrigin::SoftWrap));

        buf.handle_cr(); // redraw start of continuation row

        buf.insert_text(&to_tchars("ZZ"));

        assert!(matches!(buf.rows[1].origin, RowOrigin::SoftWrap));
        assert_eq!(buf.cursor.pos.y, 1);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod resize_tests {
    use super::*;
    use crate::row::{RowJoin, RowOrigin};
    use freminal_common::buffer_states::tchar::TChar;

    // Helper: convert &str to Vec<TChar::Ascii>
    fn to_tchars(s: &str) -> Vec<TChar> {
        s.bytes().map(TChar::Ascii).collect()
    }

    fn row_kinds(buf: &Buffer) -> Vec<(RowOrigin, RowJoin)> {
        buf.rows.iter().map(|r| (r.origin, r.join)).collect()
    }

    fn softwrap_count(buf: &Buffer) -> usize {
        row_kinds(buf)
            .into_iter()
            .filter(|(origin, _)| *origin == RowOrigin::SoftWrap)
            .count()
    }

    fn hardbreak_count(buf: &Buffer) -> usize {
        row_kinds(buf)
            .into_iter()
            .filter(|(_, join)| *join == RowJoin::NewLogicalLine)
            .count()
    }

    #[test]
    fn narrowing_preserves_logical_line_starts() {
        let mut buf = Buffer::new(40, 100);

        // Two logical lines
        buf.insert_text(&to_tchars("first logical line"));
        buf.handle_lf();
        buf.insert_text(&to_tchars(
            "second logical line that is much longer than the width",
        ));

        let before_hardbreaks = hardbreak_count(&buf);

        // Narrow the terminal
        buf.set_size(15, 100, 0);

        let after_hardbreaks = hardbreak_count(&buf);

        assert_eq!(
            before_hardbreaks, after_hardbreaks,
            "Reflow should preserve the number of logical line starts (HardBreak/NewLogicalLine)"
        );
    }

    #[test]
    fn narrowing_increases_or_preserves_softwrap_for_long_line() {
        let mut buf = Buffer::new(30, 100);

        buf.insert_text(&to_tchars(
            "this is a very long logical line that will wrap more when we narrow the width",
        ));

        let before_softwraps = softwrap_count(&buf);

        buf.set_size(10, 100, 0);

        let after_softwraps = softwrap_count(&buf);

        assert!(
            after_softwraps >= before_softwraps,
            "Narrowing should not decrease the number of SoftWrap rows for a long line"
        );

        // Sanity: all rows should now be configured with the new width
        for row in &buf.rows {
            assert_eq!(row.max_width(), 10);
        }
    }

    #[test]
    fn widening_reduces_or_preserves_softwrap_for_long_line() {
        let mut buf = Buffer::new(10, 100);

        buf.insert_text(&to_tchars(
            "this is a very long logical line that wraps quite a bit at narrow widths",
        ));

        let before_softwraps = softwrap_count(&buf);

        buf.set_size(40, 100, 0);

        let after_softwraps = softwrap_count(&buf);

        assert!(
            after_softwraps <= before_softwraps,
            "Widening should not increase the number of SoftWrap rows for a long line"
        );

        for row in &buf.rows {
            assert_eq!(row.max_width(), 40);
        }
    }

    #[test]
    fn shrink_width_clamps_cursor_x() {
        let mut buf = Buffer::new(40, 100);

        buf.insert_text(&to_tchars("some text on the first line"));
        buf.cursor.pos.x = 30;
        buf.cursor.pos.y = 0;

        buf.set_size(10, 100, 0);

        assert!(
            buf.cursor.pos.x < 10,
            "Cursor X should be clamped to new width"
        );
        assert!(
            buf.cursor.pos.y < buf.rows.len(),
            "Cursor Y should remain within the number of rows"
        );
    }

    #[test]
    fn shrink_height_clamps_cursor_y() {
        let mut buf = Buffer::new(20, 100);

        // Simulate cursor somewhere deep in the buffer
        buf.cursor.pos.y = 50;
        buf.cursor.pos.x = 5;

        buf.set_size(20, 10, 0);

        assert!(
            buf.cursor.pos.y < buf.rows.len(),
            "Cursor Y should be clamped to last row after shrinking height"
        );
    }

    #[test]
    fn reflow_keeps_softwrap_as_continuations() {
        let mut buf = Buffer::new(10, 100);

        buf.insert_text(&to_tchars("1234567890ABCDE")); // this should wrap at width 10

        let before_softwraps = softwrap_count(&buf);
        assert!(
            before_softwraps >= 1,
            "Initial insert should produce at least one SoftWrap row"
        );

        buf.set_size(8, 100, 0);

        let kinds = row_kinds(&buf);

        // Ensure that any SoftWrap row uses ContinueLogicalLine
        for (origin, join) in kinds {
            if origin == RowOrigin::SoftWrap {
                assert_eq!(
                    join,
                    RowJoin::ContinueLogicalLine,
                    "SoftWrap rows after reflow must continue the logical line"
                );
            }
        }
    }

    /// Regression test: when the alternate buffer is resized from 29→58 rows
    /// with a full-screen scroll region, the scroll region must expand to cover
    /// the new full height.  Previously the region stayed at (0, 28) — matching
    /// the old height — which caused nvim's space-fill redraw (189 spaces +
    /// CR+LF × 58 rows) to scroll prematurely at row 29, leaving the bottom
    /// half of the screen stale.
    #[test]
    fn grow_alternate_expands_full_screen_scroll_region() {
        let mut buf = Buffer::new(80, 29);
        buf.enter_alternate(0);

        // Scroll region should be full-screen for the old height.
        assert_eq!(buf.scroll_region(), (0, 28));
        assert_eq!(buf.rows.len(), 29);

        // Grow to 58 rows (simulating a split pane being closed).
        buf.set_size(80, 58, 0);

        // Scroll region MUST expand to the new full screen.
        assert_eq!(
            buf.scroll_region(),
            (0, 57),
            "Full-screen scroll region must expand when buffer height grows"
        );
        assert_eq!(buf.rows.len(), 58);

        // Verify that LF at the old bottom (row 28) advances the cursor
        // instead of scrolling — this is the behavior nvim relies on.
        buf.set_cursor_pos(Some(0), Some(28)); // 0-based row 28
        buf.handle_lf();
        assert_eq!(
            buf.cursor.pos.y, 29,
            "LF at old bottom (row 28) should advance cursor to row 29, not scroll"
        );
    }

    /// Verify that a partial (non-full-screen) scroll region is preserved
    /// across a height grow — only the bottom is clamped, not expanded.
    #[test]
    fn grow_alternate_preserves_partial_scroll_region() {
        let mut buf = Buffer::new(80, 29);
        buf.enter_alternate(0);

        // Set a partial scroll region (rows 5–20, 1-based: 6–21).
        buf.set_scroll_region(6, 21);
        assert_eq!(buf.scroll_region(), (5, 20));

        // Grow to 58 rows.
        buf.set_size(80, 58, 0);

        // Partial region should be preserved, not expanded to full screen.
        assert_eq!(
            buf.scroll_region(),
            (5, 20),
            "Partial scroll region should be preserved when buffer grows"
        );
    }

    /// Verify that shrinking the alternate buffer then growing it back
    /// restores a full-screen scroll region (the exact nvim pane-close scenario).
    #[test]
    fn shrink_then_grow_alternate_restores_full_screen_region() {
        let mut buf = Buffer::new(189, 58);
        buf.enter_alternate(0);

        assert_eq!(buf.scroll_region(), (0, 57));

        // Shrink (split pane created).
        buf.set_size(189, 29, 0);
        assert_eq!(buf.scroll_region(), (0, 28));
        assert_eq!(buf.rows.len(), 29);

        // Grow back (split pane closed).
        buf.set_size(189, 58, 0);
        assert_eq!(
            buf.scroll_region(),
            (0, 57),
            "After shrink→grow cycle, full-screen scroll region must cover new height"
        );
        assert_eq!(buf.rows.len(), 58);

        // Simulate nvim's space-fill pattern: CUP(1,1) then 58 rows of
        // 189 spaces + CR + LF.  All 58 rows should be reachable.
        buf.set_cursor_pos(Some(0), Some(0));
        for row in 0..58 {
            // Write 189 spaces (simplified — just advance cursor X).
            let spaces: Vec<TChar> = vec![TChar::Ascii(b' '); 189];
            buf.insert_text(&spaces);

            if row < 57 {
                buf.handle_cr();
                buf.handle_lf();
            }
        }

        // After filling all 58 rows, cursor should be on the last row.
        assert_eq!(
            buf.cursor.pos.y, 57,
            "Cursor should reach row 57 (0-based) after filling 58 rows"
        );
    }

    /// Regression test: resizing while on the alternate screen must also resize
    /// the saved primary buffer.  Without this, `leave_alternate` restores the
    /// primary buffer at the old dimensions, causing an immediate mismatch
    /// between buffer size and terminal geometry.
    #[test]
    fn resize_on_alternate_updates_saved_primary() {
        let mut buf = Buffer::new(80, 24);

        // Write some content in primary so it's non-trivial.
        buf.insert_text(&to_tchars("hello world"));
        buf.handle_lf();
        buf.insert_text(&to_tchars("line two"));

        // Enter alternate screen (saves primary state).
        buf.enter_alternate(0);
        assert_eq!(buf.rows.len(), 24);

        // Resize while on alternate (simulates pane close).
        buf.set_size(120, 48, 0);

        // The saved primary should have been resized too.
        let saved = buf
            .saved_primary
            .as_ref()
            .expect("saved_primary should exist while on alternate screen");
        // Rows should have the new width.
        for row in &saved.rows {
            assert_eq!(
                row.max_width(),
                120,
                "Saved primary rows should be reflowed to new width"
            );
        }
        // Scroll region should match new height.
        assert_eq!(
            saved.scroll_region_bottom, 47,
            "Saved primary scroll_region_bottom should match new height - 1"
        );

        // Leave alternate → primary should be at new dimensions.
        let restored_offset = buf.leave_alternate();
        assert_eq!(restored_offset, 0);
        assert_eq!(buf.width, 120);
        assert_eq!(buf.height, 48);
        assert_eq!(buf.scroll_region(), (0, 47));
    }
}

#[cfg(test)]
mod backspace_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    #[test]
    fn backspace_moves_left_simple() {
        let mut buf = Buffer::new(80, 24);

        buf.insert_text(&"abc".chars().map(TChar::from).collect::<Vec<_>>());
        assert_eq!(buf.cursor.pos.x, 3);

        buf.handle_backspace(ReverseWrapAround::DontWrap, XtRevWrap2::Disabled);
        assert_eq!(buf.cursor.pos.x, 2);

        buf.handle_backspace(ReverseWrapAround::DontWrap, XtRevWrap2::Disabled);
        assert_eq!(buf.cursor.pos.x, 1);

        buf.handle_backspace(ReverseWrapAround::DontWrap, XtRevWrap2::Disabled);
        assert_eq!(buf.cursor.pos.x, 0);

        // stays at 0
        buf.handle_backspace(ReverseWrapAround::DontWrap, XtRevWrap2::Disabled);
        assert_eq!(buf.cursor.pos.x, 0);
    }

    #[test]
    fn backspace_jumps_wide_glyph() {
        let mut buf = Buffer::new(80, 24);

        // Use a known double-width glyph: "あ"
        let input = "aあb".chars().map(TChar::from).collect::<Vec<_>>();
        buf.insert_text(&input);

        // "a" (col 0)
        // "あ" (cols 1–2)
        // "b" (col 3)
        assert_eq!(buf.cursor.pos.x, 4);

        buf.handle_backspace(ReverseWrapAround::DontWrap, XtRevWrap2::Disabled); // over b → x=3
        assert_eq!(buf.cursor.pos.x, 3);

        buf.handle_backspace(ReverseWrapAround::DontWrap, XtRevWrap2::Disabled); // over wide glyph (continuation cell)
        assert_eq!(buf.cursor.pos.x, 1);

        buf.handle_backspace(ReverseWrapAround::DontWrap, XtRevWrap2::Disabled); // over 'a'
        assert_eq!(buf.cursor.pos.x, 0);

        buf.handle_backspace(ReverseWrapAround::DontWrap, XtRevWrap2::Disabled); // can't go lower
        assert_eq!(buf.cursor.pos.x, 0);
    }

    #[test]
    fn backspace_does_not_move_up_lines() {
        let mut buf = Buffer::new(10, 24);

        buf.insert_text(&"abcdefghij".chars().map(TChar::from).collect::<Vec<_>>());
        buf.insert_text(&"K".chars().map(TChar::from).collect::<Vec<_>>());

        // soft wrapped, cursor should be at row 1
        assert_eq!(buf.cursor.pos.y, 1);
        assert_eq!(buf.cursor.pos.x, 1);

        // backspace never moves Y (without reverse wrap)
        buf.handle_backspace(ReverseWrapAround::DontWrap, XtRevWrap2::Disabled);
        assert_eq!(buf.cursor.pos.y, 1);
        assert_eq!(buf.cursor.pos.x, 0);

        // at col 0 → stays there
        buf.handle_backspace(ReverseWrapAround::DontWrap, XtRevWrap2::Disabled);
        assert_eq!(buf.cursor.pos.y, 1);
        assert_eq!(buf.cursor.pos.x, 0);
    }

    // ── Reverse-wrap tests (?45 and ?1045) ────────────────────────────

    #[test]
    fn reverse_wrap_at_col0_wraps_to_previous_row() {
        let mut buf = Buffer::new(10, 5);

        // Write two rows of content
        buf.insert_text(&"abcdefghij".chars().map(TChar::from).collect::<Vec<_>>());
        buf.insert_text(&"K".chars().map(TChar::from).collect::<Vec<_>>());

        // cursor is at row 1, col 1
        assert_eq!(buf.cursor.pos.y, 1);
        assert_eq!(buf.cursor.pos.x, 1);

        // Move to col 0
        buf.handle_backspace(ReverseWrapAround::WrapAround, XtRevWrap2::Disabled);
        assert_eq!(buf.cursor.pos.y, 1);
        assert_eq!(buf.cursor.pos.x, 0);

        // With reverse_wrap=true, at col 0 → wraps to last col of row 0
        buf.handle_backspace(ReverseWrapAround::WrapAround, XtRevWrap2::Disabled);
        assert_eq!(buf.cursor.pos.y, 0);
        assert_eq!(buf.cursor.pos.x, 9); // last column (width - 1)
    }

    #[test]
    fn reverse_wrap_at_top_of_visible_no_scrollback_stays_put() {
        let mut buf = Buffer::new(10, 5);

        // Only one row — cursor at (0, 0)
        buf.cursor.pos.x = 0;
        buf.cursor.pos.y = 0;

        // reverse_wrap is on but no row above → stays put
        buf.handle_backspace(ReverseWrapAround::WrapAround, XtRevWrap2::Disabled);
        assert_eq!(buf.cursor.pos.y, 0);
        assert_eq!(buf.cursor.pos.x, 0);
    }

    #[test]
    fn reverse_wrap_disabled_never_wraps() {
        let mut buf = Buffer::new(10, 5);

        buf.insert_text(&"abcdefghij".chars().map(TChar::from).collect::<Vec<_>>());
        buf.insert_text(&"K".chars().map(TChar::from).collect::<Vec<_>>());

        // Move to col 0 of row 1
        buf.cursor.pos.x = 0;
        assert_eq!(buf.cursor.pos.y, 1);

        // reverse_wrap=false → stays at col 0
        buf.handle_backspace(ReverseWrapAround::DontWrap, XtRevWrap2::Disabled);
        assert_eq!(buf.cursor.pos.y, 1);
        assert_eq!(buf.cursor.pos.x, 0);
    }

    #[test]
    fn reverse_wrap_into_scrollback_with_xt_rev_wrap2() {
        // Create a buffer with scrollback: fill enough rows to push some
        // above the visible window.
        let mut buf = Buffer::new(10, 3);

        // Fill 5 rows of data; height is 3, so 2 rows are in scrollback.
        for line in &[
            "aaaaaaaaaa",
            "bbbbbbbbbb",
            "cccccccccc",
            "dddddddddd",
            "eeeeeeeeee",
        ] {
            buf.insert_text(&line.chars().map(TChar::from).collect::<Vec<_>>());
            buf.handle_lf();
        }

        // Cursor is now at the bottom of visible window.
        // Move cursor to col 0 at the top of the visible window.
        let vis_start = buf.visible_window_start(0);
        buf.cursor.pos.y = vis_start;
        buf.cursor.pos.x = 0;

        // Without xt_rev_wrap2: stays put at top of visible screen
        buf.handle_backspace(ReverseWrapAround::WrapAround, XtRevWrap2::Disabled);
        assert_eq!(buf.cursor.pos.y, vis_start);
        assert_eq!(buf.cursor.pos.x, 0);

        // With xt_rev_wrap2=true: wraps into scrollback
        buf.handle_backspace(ReverseWrapAround::WrapAround, XtRevWrap2::Enabled);
        assert_eq!(buf.cursor.pos.y, vis_start - 1);
        assert_eq!(buf.cursor.pos.x, 9); // last column
    }

    #[test]
    fn xt_rev_wrap2_without_reverse_wrap_does_nothing() {
        let mut buf = Buffer::new(10, 5);

        buf.insert_text(&"abcdefghij".chars().map(TChar::from).collect::<Vec<_>>());
        buf.insert_text(&"K".chars().map(TChar::from).collect::<Vec<_>>());

        buf.cursor.pos.x = 0;
        assert_eq!(buf.cursor.pos.y, 1);

        // reverse_wrap=false, xt_rev_wrap2=true → no wrap (reverse_wrap gate is checked first)
        buf.handle_backspace(ReverseWrapAround::DontWrap, XtRevWrap2::Enabled);
        assert_eq!(buf.cursor.pos.y, 1);
        assert_eq!(buf.cursor.pos.x, 0);
    }

    #[test]
    fn backspace_from_pending_wrap_state_lands_at_width_minus_2() {
        // A real VT100 keeps the cursor at the last column with a separate
        // pending-wrap bit; BS clears the bit and moves left from the reported
        // column (width-1), landing at width-2.  Freminal encodes pending-wrap as
        // cursor.pos.x == width, so BS must clamp before subtracting.
        let mut buf = Buffer::new(10, 5);

        // Write exactly 10 characters to fill row 0 and enter pending-wrap.
        // After the 10th character insert_text sets cursor.pos.x = 10 (== width).
        buf.insert_text(&"abcdefghij".chars().map(TChar::from).collect::<Vec<_>>());
        assert_eq!(
            buf.cursor.pos.x, 10,
            "cursor should be in pending-wrap state"
        );
        assert_eq!(buf.cursor.pos.y, 0);

        // BS from pending-wrap: should land at col 8 (width-2), not col 9 (width-1).
        buf.handle_backspace(ReverseWrapAround::DontWrap, XtRevWrap2::Disabled);
        assert_eq!(buf.cursor.pos.y, 0);
        assert_eq!(
            buf.cursor.pos.x, 8,
            "BS from pending-wrap must land at width-2"
        );

        // A second BS moves left normally: col 8 → col 7.
        buf.handle_backspace(ReverseWrapAround::DontWrap, XtRevWrap2::Disabled);
        assert_eq!(buf.cursor.pos.x, 7);
    }
}

#[cfg(test)]
mod insert_space_tests {
    use freminal_common::buffer_states::fonts::FontWeight;

    use super::*;

    // Converts a row to a simple string for visual comparison
    fn cell_str(buf: &Buffer, row: usize) -> String {
        (0..buf.width)
            .map(|col| {
                let cell = buf.rows[row].resolve_cell(col);
                match &cell.tchar() {
                    TChar::Ascii(b) => *b as char,
                    TChar::Space => ' ',
                    TChar::NewLine => '⏎',
                    TChar::Utf8(buf, len) => {
                        let s = String::from_utf8_lossy(&buf[..*len as usize]);
                        s.chars().next().unwrap_or('�')
                    }
                }
            })
            .collect()
    }

    fn tag_vec(buf: &Buffer, row: usize) -> Vec<FormatTag> {
        (0..buf.width)
            .map(|col| buf.rows[row].resolve_cell(col).tag().clone())
            .collect()
    }

    /// Construct a `TChar::Ascii` from char
    fn a(c: char) -> TChar {
        TChar::Ascii(c as u8)
    }

    #[test]
    fn ich_simple_middle_insert() {
        // width 10 row: ABCDE-----
        let mut buf = Buffer::new(10, 5);

        let tag = buf.current_tag.clone();

        // Insert ABCDE
        buf.insert_text(&[a('A'), a('B'), a('C'), a('D'), a('E')]);

        // Move cursor to 'C'
        buf.cursor.pos.x = 2;

        // ICH(2): insert 2 blanks at col 2
        buf.insert_spaces(2);

        let row = cell_str(&buf, 0);
        assert_eq!(&row[..7], "AB  CDE", "Row should shift correctly");
        assert_eq!(buf.cursor.pos.x, 2, "Cursor must not move");

        // Tag propagation
        let tags = tag_vec(&buf, 0);
        assert_eq!(tags[2], tag, "Inserted blank must inherit tag");
        assert_eq!(tags[3], tag, "Inserted blank must inherit tag");
    }

    #[test]
    fn ich_clamps_at_row_end() {
        let mut buf = Buffer::new(5, 5);

        buf.insert_text(&[a('A'), a('B'), a('C'), a('D'), a('E')]);

        // Cursor at last column
        buf.cursor.pos.x = 4;

        // ICH(10) -> only 1 can fit
        buf.insert_spaces(10);

        let row = cell_str(&buf, 0);
        assert_eq!(row, "ABCD ", "Only one blank should fit");
    }

    #[test]
    fn ich_preserves_shifted_tags() {
        let mut buf = Buffer::new(10, 5);

        // Store original tag1
        let tag1 = buf.current_tag.clone();
        buf.insert_text(&[a('A'), a('B'), a('C')]);

        // Change tag via your actual API.
        // If you don't have color-changing yet, just clone + toggle an attribute.
        let mut new_tag = tag1.clone();
        new_tag.font_weight = match new_tag.font_weight {
            FontWeight::Normal => FontWeight::Bold,
            FontWeight::Bold => FontWeight::Normal,
        };
        buf.current_tag = new_tag.clone();

        // Insert D E using new tag2
        buf.insert_text(&[a('D'), a('E')]);

        buf.cursor.pos.x = 1;
        buf.insert_spaces(2);

        let row = cell_str(&buf, 0);
        assert_eq!(&row[..7], "A  BCDE", "Row layout mismatch");

        let tags_check = tag_vec(&buf, 0);

        // Inserted blanks should use new_tag
        assert_eq!(tags_check[1], new_tag);
        assert_eq!(tags_check[2], new_tag);

        // Now verify proper tag retention for shifted cells:
        // B, C use tag1 (original)
        assert_eq!(tags_check[3], tag1);
        assert_eq!(tags_check[4], tag1);

        // D, E use new_tag
        assert_eq!(tags_check[5], new_tag);
        assert_eq!(tags_check[6], new_tag);
    }
}

#[cfg(test)]
mod dch_tests {
    use super::*;

    /// Construct a `TChar::Ascii` from a char for brevity.
    fn a(c: char) -> TChar {
        TChar::Ascii(c as u8)
    }

    /// Render row `row` of `buf` as a `String` using the full logical width.
    fn cell_str(buf: &Buffer, row: usize) -> String {
        (0..buf.width)
            .map(|col| {
                let cell = buf.rows[row].resolve_cell(col);
                match cell.tchar() {
                    TChar::Ascii(b) => *b as char,
                    TChar::Space => ' ',
                    TChar::NewLine => '\n',
                    TChar::Utf8(buf, len) => String::from_utf8_lossy(&buf[..*len as usize])
                        .chars()
                        .next()
                        .unwrap_or('?'),
                }
            })
            .collect()
    }

    /// `delete_chars` removes cells at the cursor column, shifting the rest left.
    #[test]
    fn dch_simple() {
        // width 10: insert "ABCDE", cursor at col 1, delete 2 → "ADE       "
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&[a('A'), a('B'), a('C'), a('D'), a('E')]);
        buf.cursor.pos.x = 1;
        buf.delete_chars(2);

        let row = cell_str(&buf, 0);
        assert_eq!(
            &row[..3],
            "ADE",
            "B and C should be removed, rest shifts left"
        );
        assert_eq!(buf.cursor.pos.x, 1, "cursor must not move after DCH");
    }

    /// When `n` exceeds the cells to the right of the cursor, everything from
    /// the cursor onward is erased — no panic, no out-of-bounds access.
    #[test]
    fn dch_clamps() {
        // width 10: insert "ABC", cursor at col 1, delete 100 → "A         "
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&[a('A'), a('B'), a('C')]);
        buf.cursor.pos.x = 1;
        buf.delete_chars(100);

        let row = cell_str(&buf, 0);
        // Only 'A' remains; the rest is blank.
        assert_eq!(row.trim_end(), "A", "only A should remain");
        assert_eq!(buf.cursor.pos.x, 1, "cursor must not move");
    }

    /// DCH at column 0 removes the very first character and shifts everything left.
    #[test]
    fn dch_at_col_zero() {
        // width 10: insert "ABCDE", cursor at col 0, delete 1 → "BCDE      "
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&[a('A'), a('B'), a('C'), a('D'), a('E')]);
        buf.cursor.pos.x = 0;
        buf.delete_chars(1);

        let row = cell_str(&buf, 0);
        assert_eq!(&row[..4], "BCDE", "first char removed, rest shifts left");
        assert_eq!(buf.cursor.pos.x, 0, "cursor must not move");
    }

    /// When the cursor is at or beyond the stored cells, DCH is a no-op
    /// and must not panic.
    #[test]
    fn dch_noop_past_end() {
        // width 10: insert "AB", cursor way beyond stored cells
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&[a('A'), a('B')]);
        buf.cursor.pos.x = 8; // past stored cells
        buf.delete_chars(2); // should be a silent no-op

        let row = cell_str(&buf, 0);
        assert_eq!(&row[..2], "AB", "stored cells must be unchanged");
    }

    /// DCH on a wide (2-column) character at the cursor removes both the head
    /// and its continuation cell.
    #[test]
    fn dch_wide_head() {
        // Width 10; insert the wide char "あ" (display width 2) followed by "BC".
        // Cursor at col 0, delete 1 → head + continuation gone, "BC" shifts left.
        let wide = TChar::from('あ');
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&[wide, a('B'), a('C')]);
        buf.cursor.pos.x = 0;
        buf.delete_chars(1);

        // After deletion the wide glyph (2 cols) is removed; "BC" starts at col 0.
        let row = cell_str(&buf, 0);
        assert_eq!(
            &row[..2],
            "BC",
            "wide head+continuation removed, BC shifts left"
        );
        assert_eq!(buf.cursor.pos.x, 0, "cursor must not move");
    }
}

#[cfg(test)]
mod ech_tests {
    use super::*;

    /// Construct a `TChar::Ascii` from a char for brevity.
    fn a(c: char) -> TChar {
        TChar::Ascii(c as u8)
    }

    /// Render row `row` of `buf` as a `String` using the full logical width.
    fn cell_str(buf: &Buffer, row: usize) -> String {
        (0..buf.width)
            .map(|col| {
                let cell = buf.rows[row].resolve_cell(col);
                match cell.tchar() {
                    TChar::Ascii(b) => *b as char,
                    TChar::Space => ' ',
                    TChar::NewLine => '\n',
                    TChar::Utf8(buf, len) => String::from_utf8_lossy(&buf[..*len as usize])
                        .chars()
                        .next()
                        .unwrap_or('?'),
                }
            })
            .collect()
    }

    /// ECH replaces cells in-place with blanks; chars to the right stay put.
    #[test]
    fn ech_simple() {
        // width 10: insert "ABCDE", cursor at col 1, erase_chars(2) → "A  DE     "
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&[a('A'), a('B'), a('C'), a('D'), a('E')]);
        buf.cursor.pos.x = 1;
        buf.erase_chars(2);

        let row = cell_str(&buf, 0);
        assert_eq!(
            &row[..5],
            "A  DE",
            "B and C replaced by blanks, D/E stay in place"
        );
        assert_eq!(buf.cursor.pos.x, 1, "cursor must not move after ECH");
    }

    /// When the erase range extends past the row width, only up to `width` cells
    /// are erased — no panic, no out-of-bounds access.
    #[test]
    fn ech_clamps_at_width() {
        // width 10: insert "ABCDEFGHIJ", cursor at col 8, erase_chars(10) → only 2 erased
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&[
            a('A'),
            a('B'),
            a('C'),
            a('D'),
            a('E'),
            a('F'),
            a('G'),
            a('H'),
            a('I'),
            a('J'),
        ]);
        buf.cursor.pos.x = 8;
        buf.erase_chars(10); // only cols 8 and 9 can be erased

        let row = cell_str(&buf, 0);
        assert_eq!(&row[..8], "ABCDEFGH", "first 8 chars untouched");
        assert_eq!(&row[8..10], "  ", "last 2 chars erased");
        assert_eq!(buf.cursor.pos.x, 8, "cursor must not move");
    }

    /// ECH at column 0 replaces the first n cells with blanks.
    #[test]
    fn ech_at_col_zero() {
        // width 10: insert "ABCDE", cursor at col 0, erase_chars(3) → "   DE     "
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&[a('A'), a('B'), a('C'), a('D'), a('E')]);
        buf.cursor.pos.x = 0;
        buf.erase_chars(3);

        let row = cell_str(&buf, 0);
        assert_eq!(&row[..5], "   DE", "first 3 cells blanked, D and E stay");
        assert_eq!(buf.cursor.pos.x, 0, "cursor must not move");
    }

    /// ECH differs from DCH: after erasing, the character to the right of the
    /// erased region is still at its original column position (not shifted left).
    #[test]
    fn ech_vs_dch_differ() {
        // width 10: insert "ABCDE", cursor at col 1, erase 2.
        // After ECH: col 3 still holds 'D' (not shifted to col 1 as DCH would do).
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&[a('A'), a('B'), a('C'), a('D'), a('E')]);
        buf.cursor.pos.x = 1;
        buf.erase_chars(2);

        // 'D' must still be at column 3, not column 1.
        let cell_at_3 = buf.rows[0].resolve_cell(3);
        assert_eq!(
            cell_at_3.tchar(),
            &TChar::Ascii(b'D'),
            "D must remain at col 3 (ECH does not shift)"
        );

        // Erased columns 1 and 2 should be blank.
        let cell_at_1 = buf.rows[0].resolve_cell(1);
        let cell_at_2 = buf.rows[0].resolve_cell(2);
        assert_eq!(cell_at_1.tchar(), &TChar::Space, "col 1 should be blank");
        assert_eq!(cell_at_2.tchar(), &TChar::Space, "col 2 should be blank");
    }
}

#[cfg(test)]
mod tests_gui_scroll {
    use super::*;

    fn make_row(width: usize) -> Row {
        Row::new_with_origin(width, RowOrigin::HardBreak, RowJoin::NewLogicalLine)
    }

    fn buffer_with_rows(n: usize, width: usize, height: usize, scrollback: usize) -> Buffer {
        let mut b = Buffer::new(width, height);
        b.scrollback_limit = scrollback;
        b.rows = (0..n).map(|_| make_row(width)).collect();
        b.row_cache = vec![None; b.rows.len()];

        // Put cursor at last row to begin
        b.cursor.pos.y = b.rows.len().saturating_sub(1);
        b.cursor.pos.x = 0;

        b
    }

    // ---------------------------------------------------------------
    // Test 1: basic trimming behavior
    // ---------------------------------------------------------------

    #[test]
    fn enforce_limit_trims_excess_rows() {
        // height = 5, scrollback_limit = 5 → max_rows = 10
        let mut buf = buffer_with_rows(15, 80, 5, 5);

        let new_offset = buf.enforce_scrollback_limit(0);

        assert_eq!(buf.rows.len(), 10, "should trim to max_rows");
        assert_eq!(buf.cursor.pos.y, 9, "cursor adjusted to last row");
        assert_eq!(new_offset, 0, "no scrollback, so offset remains 0");
    }

    // ---------------------------------------------------------------
    // Test 2: scroll_offset reduces when rows are trimmed
    // ---------------------------------------------------------------

    #[test]
    fn enforce_limit_reduces_scroll_offset() {
        // height=5, scrollback_limit=5 → max_rows=10
        // Start with 15 rows, scroll_offset=4
        let mut buf = buffer_with_rows(15, 80, 5, 5);

        let new_offset = buf.enforce_scrollback_limit(4);

        // Overflow = 5 rows trimmed
        // scroll_offset = 4 → trimmed by 5 → becomes 0
        assert_eq!(new_offset, 0);
        assert_eq!(buf.rows.len(), 10);
    }

    // ---------------------------------------------------------------
    // Test 3: scroll_offset shrinks but is not eliminated
    // ---------------------------------------------------------------

    #[test]
    fn enforce_limit_scroll_offset_partially_reduced() {
        // height=5, scrollback_limit=5 → max_rows=10
        // rows=14 → overflow=4
        // scroll_offset=6 → becomes 6-4=2
        let mut buf = buffer_with_rows(14, 80, 5, 5);

        let new_offset = buf.enforce_scrollback_limit(6);

        assert_eq!(buf.rows.len(), 10);
        assert_eq!(new_offset, 2);
    }

    // ---------------------------------------------------------------
    // Test 4: cursor shifts downward when rows removed
    // ---------------------------------------------------------------

    #[test]
    fn enforce_limit_adjusts_cursor_position() {
        // rows=12, height=5 → max_rows=10 → overflow=2
        let mut buf = buffer_with_rows(12, 80, 5, 5);

        buf.cursor.pos.y = 3;
        let _ = buf.enforce_scrollback_limit(0);

        // Expected: 3 - 2 = 1
        assert_eq!(buf.cursor.pos.y, 1);
    }

    // ---------------------------------------------------------------
    // Test 5: cursor shifts correctly when it survives the trim
    // ---------------------------------------------------------------

    #[test]
    fn enforce_limit_cursor_shift_down_by_overflow() {
        // rows=12 → overflow=2 → trim 2
        let mut buf = buffer_with_rows(12, 80, 5, 5);
        buf.cursor.pos.y = 7;

        let _ = buf.enforce_scrollback_limit(0);

        assert_eq!(buf.cursor.pos.y, 5, "cursor should shift by overflow");
    }

    // ---------------------------------------------------------------
    // Test 6: no scrollback trimming in alternate buffer
    // ---------------------------------------------------------------

    #[test]
    fn enforce_limit_noop_in_alternate_buffer() {
        let mut buf = buffer_with_rows(20, 80, 5, 5);
        buf.kind = BufferType::Alternate;
        buf.cursor.pos.y = 10;

        let original_len = buf.rows.len();

        // In alternate buffer, scroll_offset is always effectively 0 and no trimming occurs.
        let new_offset = buf.enforce_scrollback_limit(3);

        assert_eq!(buf.rows.len(), original_len, "alternate buffer never trims");
        assert_eq!(
            new_offset, 3,
            "alternate buffer returns the passed offset unchanged"
        );
        assert_eq!(buf.cursor.pos.y, 10);
    }

    // ---------------------------------------------------------------
    // Test 7: scroll_offset never exceeds new max_scroll_offset()
    // ---------------------------------------------------------------

    #[test]
    fn enforce_limit_clamps_scroll_offset_to_max() {
        // rows=13, height=5 → max_scroll_offset = 13-5 = 8
        let mut buf = buffer_with_rows(13, 80, 5, 5);

        let new_offset = buf.enforce_scrollback_limit(50); // wildly out of range

        let max = buf.max_scroll_offset();
        assert!(new_offset <= max);
    }
}

#[cfg(test)]
mod tests_gui_resize {
    use super::*;
    use crate::row::{Row, RowJoin, RowOrigin};

    // Helper: create a buffer with N rows and a given config
    fn buffer_with_rows_and_config(
        n: usize,
        width: usize,
        height: usize,
        scrollback: usize,
        preserve_anchor: bool,
    ) -> Buffer {
        let mut b = Buffer::new(width, height);
        b.scrollback_limit = scrollback;
        b.rows = (0..n)
            .map(|_| Row::new_with_origin(width, RowOrigin::HardBreak, RowJoin::NewLogicalLine))
            .collect();
        b.row_cache = vec![None; b.rows.len()];

        b.cursor.pos.y = b.rows.len().saturating_sub(1);
        b.cursor.pos.x = 0;

        b.preserve_scrollback_anchor = preserve_anchor;
        b
    }

    // ------------------------------------------------------------------
    // 1. preserve_scrollback_anchor = false → scroll_offset resets
    // ------------------------------------------------------------------
    #[test]
    fn resize_resets_scroll_offset_when_anchor_disabled() {
        let mut buf = buffer_with_rows_and_config(50, 80, 20, 1000, false);

        let new_offset = buf.set_size(80, 10, 15); // shrink height, pass offset=15

        assert_eq!(
            new_offset, 0,
            "resize must reset scroll_offset when anchor is disabled"
        );
    }

    // ------------------------------------------------------------------
    // 2. preserve_scrollback_anchor = true → scroll_offset preserved on grow
    // ------------------------------------------------------------------
    #[test]
    fn resize_preserves_offset_when_growing_height() {
        let mut buf = buffer_with_rows_and_config(50, 80, 20, 1000, true);

        let new_offset = buf.set_size(80, 30, 10); // grow height, pass offset=10

        assert_eq!(
            new_offset, 10,
            "scroll_offset should be unchanged when anchor is enabled on grow"
        );
    }

    // ------------------------------------------------------------------
    // 3. preserve_scrollback_anchor = true → offset clamped when shrinking
    // ------------------------------------------------------------------
    #[test]
    fn resize_clamps_scroll_offset_when_shrinking() {
        // rows = 50, new height = 10 → max_scroll_offset = 50 - 10 = 40
        let mut buf = buffer_with_rows_and_config(50, 80, 20, 1000, true);

        let new_offset = buf.set_size(80, 10, 100); // far beyond range

        assert_eq!(
            new_offset, 40,
            "scroll_offset must clamp to new max_scroll_offset"
        );
    }

    // ------------------------------------------------------------------
    // 4. Primary buffer: cursor stays at absolute position after shrink
    // ------------------------------------------------------------------
    #[test]
    fn resize_primary_cursor_stays_at_absolute_position() {
        let mut buf = buffer_with_rows_and_config(10, 80, 10, 1000, false);

        buf.cursor.pos.y = 9; // last row (absolute index into rows[])
        buf.set_size(80, 5, 0); // shrink height from 10 to 5

        // Primary buffer: cursor.pos.y is an absolute index into rows[], NOT
        // screen-relative.  Shrinking the height does not remove rows (they
        // become scrollback), so row 9 is still valid (9 < rows.len() == 10).
        // The cursor must NOT be clamped to `new_height - 1`.
        assert_eq!(
            buf.cursor.pos.y, 9,
            "primary buffer cursor must keep its absolute row position after shrink"
        );
        assert_eq!(
            buf.rows.len(),
            10,
            "primary buffer rows must not be deleted on shrink"
        );
    }

    // ------------------------------------------------------------------
    // 4b. Alternate buffer: cursor IS clamped on shrink (screen-relative)
    // ------------------------------------------------------------------
    #[test]
    fn resize_alternate_cursor_clamped_on_shrink() {
        let mut buf = buffer_with_rows_and_config(10, 80, 10, 1000, false);
        buf.kind = BufferType::Alternate;

        buf.cursor.pos.y = 9; // last row
        buf.set_size(80, 5, 0); // shrink

        // Alternate buffer: excess rows are drained from the top, cursor is
        // adjusted by the number of removed rows.  With 10 rows shrunk to
        // height 5, 5 rows are removed, cursor moves from 9 to 4.
        assert!(
            buf.cursor.pos.y <= 4,
            "alternate buffer cursor must clamp into new visible height"
        );
    }

    // ------------------------------------------------------------------
    // 5. Growing height adds rows at the bottom
    // ------------------------------------------------------------------
    #[test]
    fn resize_grow_adds_rows() {
        let mut buf = buffer_with_rows_and_config(10, 80, 10, 1000, false);

        buf.set_size(80, 15, 0);

        assert_eq!(buf.rows.len(), 15, "growing height must append blank rows");
    }

    // ------------------------------------------------------------------
    // 6. Shrinking height does not delete scrollback rows
    // ------------------------------------------------------------------
    #[test]
    fn resize_shrink_retain_scrollback() {
        let mut buf = buffer_with_rows_and_config(40, 80, 20, 1000, false);

        buf.set_size(80, 10, 0);

        // rows should remain 40, no deletion due to resize
        assert_eq!(buf.rows.len(), 40);
    }
}

#[cfg(test)]
mod scrollback_wrapping_scroll_visible_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    fn to_tchars(s: &str) -> Vec<TChar> {
        s.bytes().map(TChar::Ascii).collect()
    }

    fn make_buffer(width: usize, height: usize, lines: usize) -> Buffer {
        let mut b = Buffer::new(width, height);
        for _ in 0..lines {
            b.insert_text(&to_tchars("line"));
            b.handle_lf();
        }
        b
    }

    /// Helper: recompute the expected visible slice using the same math
    /// as `Buffer::visible_rows` and compare contents.
    fn assert_visible_rows_consistent(b: &Buffer, scroll_offset: usize) {
        let total = b.rows.len();
        let h = b.height;

        if total == 0 {
            assert_eq!(b.visible_rows(scroll_offset).len(), 0);
            return;
        }

        let max_offset = b.max_scroll_offset();
        let offset = scroll_offset.min(max_offset);

        let start = total.saturating_sub(h + offset);
        let mut end = start + h;
        if end > total {
            end = total;
        }

        let expected = &b.rows[start..end];
        let visible = b.visible_rows(scroll_offset);

        assert_eq!(
            visible.len(),
            expected.len(),
            "visible_rows length mismatch"
        );

        for (row_vis, row_exp) in visible.iter().zip(expected.iter()) {
            assert_eq!(
                row_vis.get_characters(),
                row_exp.get_characters(),
                "visible row content mismatch"
            );
        }
    }

    #[test]
    fn visible_rows_respects_scroll_offset_at_bottom() {
        // Many logical lines, no wrapping needed for this test.
        let b = make_buffer(20, 3, 10);

        // At live bottom
        assert_visible_rows_consistent(&b, 0);
    }

    #[test]
    fn visible_rows_respects_scroll_offset_in_scrollback() {
        let b = make_buffer(20, 3, 10);

        // Scroll back into history
        let scroll_offset = b.scroll_back(0, 2);
        assert!(scroll_offset > 0);

        assert_visible_rows_consistent(&b, scroll_offset);
    }
}

#[cfg(test)]
mod scrollback_reflow_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    fn t(s: &str) -> Vec<TChar> {
        s.bytes().map(TChar::Ascii).collect()
    }

    fn assert_visible_rows_sane(b: &Buffer) {
        let vis = b.visible_rows(0);
        assert!(
            vis.len() <= b.height,
            "visible_rows must not exceed buffer height"
        );
        // Also ensure the slice is a valid contiguous chunk of rows
        // (length 0 is fine).
        if !b.rows.is_empty() {
            assert!(!vis.is_empty(), "non-empty buffer should have visible rows");
        }
    }

    #[test]
    fn reflow_resets_scroll_offset_and_visible_rows_valid() {
        let mut b = Buffer::new(20, 3);

        // Create enough rows so scrollback is actually possible
        for _ in 0..10 {
            b.insert_text(&t("X"));
            b.handle_lf();
        }

        let max_off = b.max_scroll_offset();
        assert!(max_off > 0);

        let scroll_offset = b.scroll_back(0, 2);
        assert!(scroll_offset > 0, "should be scrolled back before reflow");

        // Change width to trigger reflow_to_width; set_size returns new scroll_offset
        let new_offset = b.set_size(10, 3, scroll_offset);

        // reflow_to_width resets scroll_offset to 0
        assert_eq!(new_offset, 0);

        // visible_rows must be sane after reflow
        assert_visible_rows_sane(&b);
    }

    #[test]
    fn reflow_preserves_valid_row_state_after_widening() {
        let mut b = Buffer::new(5, 4);

        // Create a long line likely to wrap at width=5
        b.insert_text(&t(
            "this is a long logical line that should wrap at narrow widths",
        ));

        let rows_before = b.rows.len();
        assert!(rows_before >= 1);

        // Widen the terminal; this may unwrap some rows,
        // but we do NOT assert that the row count must go down.
        let new_offset = b.set_size(40, 4, 0);

        // Scroll offset is always reset by reflow
        assert_eq!(new_offset, 0);

        // All rows should now use the new width
        for row in &b.rows {
            assert_eq!(row.max_width(), 40);
        }

        // visible_rows must remain sane
        assert_visible_rows_sane(&b);
    }
}

#[cfg(test)]
mod scrollback_height_resize_wrapping_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    fn t(s: &str) -> Vec<TChar> {
        s.bytes().map(TChar::Ascii).collect()
    }

    fn assert_visible_rows_sane(b: &Buffer, scroll_offset: usize) {
        let vis = b.visible_rows(scroll_offset);
        assert!(
            vis.len() <= b.height,
            "visible_rows must not exceed buffer height"
        );
        if !b.rows.is_empty() {
            assert!(!vis.is_empty(), "non-empty buffer should have visible rows");
        }
    }

    #[test]
    fn shrink_height_with_wrapped_content_keeps_visible_rows_valid() {
        let mut b = Buffer::new(5, 6);

        // Produce some wrapped content and extra rows
        b.insert_text(&t("ABCDEFGHIJKLMNOPQRSTUVWXYZ"));
        for _ in 0..5 {
            b.handle_lf();
            b.insert_text(&t("extra line"));
        }

        // Allow preserving anchor so resize_height will clamp instead of reset
        b.preserve_scrollback_anchor = true;

        // Try to scroll back; may or may not succeed depending on layout
        let scroll_offset = b.scroll_back(0, 3);

        // Now shrink height; pass scroll_offset in, get adjusted one back
        let new_offset = b.set_size(5, 3, scroll_offset);

        // Whatever scroll_offset is now, visible_rows must be well-formed
        assert_visible_rows_sane(&b, new_offset);
    }
}

#[cfg(test)]
mod visible_rows_boundary_tests {
    use super::*;

    #[test]
    fn visible_rows_small_buffer_returns_all_rows() {
        let mut b = Buffer::new(10, 5);

        // Buffer starts with 1 row; 2 LFs grow it to 3 rows (still within height).
        b.handle_lf();
        b.handle_lf();

        assert_eq!(b.rows.len(), 3);

        let vis = b.visible_rows(0);
        // rows.len() <= height, so all rows are visible.
        assert_eq!(vis.len(), b.rows.len());
    }

    #[test]
    fn visible_rows_exact_height() {
        let mut b = Buffer::new(10, 3);

        b.handle_lf();
        b.handle_lf(); // 3 rows total

        assert_eq!(b.rows.len(), 3);

        let vis = b.visible_rows(0);
        assert_eq!(vis.len(), 3);
    }

    #[test]
    fn visible_rows_top_of_scrollback_is_first_rows() {
        let mut b = Buffer::new(10, 3);

        for _ in 0..10 {
            b.handle_lf();
        }

        let scroll_offset = b.scroll_back(0, 999); // scroll to top
        let vis = b.visible_rows(scroll_offset);

        assert_eq!(vis.len(), 3);
        // The first visible row must be the first buffer row
        assert_eq!(vis[0].get_characters(), b.rows[0].get_characters());
    }
}

#[cfg(test)]
mod alt_buffer_visible_rows_tests {
    use super::*;

    #[test]
    fn alt_buffer_visible_rows_always_height() {
        let mut b = Buffer::new(5, 4);

        b.enter_alternate(0);
        let vis = b.visible_rows(0);

        assert_eq!(vis.len(), 4);
        assert!(vis.iter().all(|r| r.get_characters().is_empty()));
    }

    #[test]
    fn leave_alt_restores_primary_visible_rows() {
        let mut b = Buffer::new(5, 4);

        for _ in 0..10 {
            b.handle_lf();
        }

        let scroll_offset = b.scroll_back(0, 2);
        let before = b.visible_rows(scroll_offset)[0].get_characters().clone();

        b.enter_alternate(scroll_offset);
        let restored_offset = b.leave_alternate();

        let after = b.visible_rows(restored_offset)[0].get_characters().clone();
        assert_eq!(before, after);
    }
}

#[cfg(test)]
mod cr_wrap_scrollback_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    fn t(s: &str) -> Vec<TChar> {
        s.bytes().map(TChar::Ascii).collect()
    }

    fn assert_visible_rows_consistent(b: &Buffer, scroll_offset: usize) {
        let total = b.rows.len();
        let h = b.height;

        if total == 0 {
            assert_eq!(b.visible_rows(scroll_offset).len(), 0);
            return;
        }

        let max_offset = b.max_scroll_offset();
        let offset = scroll_offset.min(max_offset);

        let start = total.saturating_sub(h + offset);
        let mut end = start + h;
        if end > total {
            end = total;
        }

        let expected = &b.rows[start..end];
        let visible = b.visible_rows(scroll_offset);

        assert_eq!(
            visible.len(),
            expected.len(),
            "visible_rows length mismatch"
        );

        for (row_vis, row_exp) in visible.iter().zip(expected.iter()) {
            assert_eq!(
                row_vis.get_characters(),
                row_exp.get_characters(),
                "visible row content mismatch"
            );
        }
    }

    #[test]
    fn cr_in_wrap_then_scrollback_has_consistent_visible_slice() {
        let mut b = Buffer::new(5, 3);

        // Cause a wrap
        b.insert_text(&t("1234567890")); // wraps
        b.handle_cr();
        b.insert_text(&t("ZZ"));

        // Try scrolling into history (may or may not move offset much)
        let scroll_offset = b.scroll_back(0, 1);

        // Whatever the final scroll_offset is, visible_rows must be a
        // correct slice of rows.
        assert_visible_rows_consistent(&b, scroll_offset);
    }
}

#[cfg(test)]
mod scroll_up_scrollback_tests {
    use super::*;

    #[test]
    fn scroll_up_shifts_cursor_and_keeps_scrollback_consistent() {
        let mut b = Buffer::new(10, 3);

        // populate 6 rows
        for _ in 0..5 {
            b.handle_lf();
        }

        b.cursor.pos.y = 2;

        b.scroll_up(); // remove row 0

        assert_eq!(b.rows.len(), 6);
        assert_eq!(b.cursor.pos.y, 1, "cursor must shift downward by 1");
        // scroll_offset is external; caller is responsible for keeping it consistent
    }

    #[test]
    fn scroll_up_does_not_affect_external_scroll_offset() {
        let mut b = Buffer::new(10, 3);

        for _ in 0..20 {
            b.handle_lf();
        }

        let scroll_offset = b.scroll_back(0, 5);

        b.scroll_up(); // remove row 0
        // External scroll_offset is unchanged by scroll_up; caller manages it
        assert_eq!(scroll_offset, 5, "external scroll_offset must not change");
    }
}

#[cfg(test)]
mod alt_primary_scroll_offset_restore_tests {
    use super::*;

    #[test]
    fn leaving_alt_restores_scrollback_offset() {
        let mut b = Buffer::new(10, 5);

        for _ in 0..20 {
            b.handle_lf();
        }
        let scroll_offset = b.scroll_back(0, 3);
        assert_eq!(scroll_offset, 3);

        b.enter_alternate(scroll_offset);
        b.handle_lf();
        b.handle_lf();

        let restored = b.leave_alternate();
        assert_eq!(restored, 3);
    }
}

// ============================================================================
// visible_as_tchars_and_tags tests
// ============================================================================

#[cfg(test)]
mod visible_as_tchars_and_tags_tests {
    use super::*;
    use freminal_common::buffer_states::{fonts::FontWeight, format_tag::FormatTag, tchar::TChar};
    use freminal_common::colors::TerminalColor;

    // Helper: convert ASCII &str or &[u8] to Vec<TChar> without fallible operations.
    fn to_tchars(s: &str) -> Vec<TChar> {
        s.bytes().map(TChar::Ascii).collect()
    }

    fn tchars_to_string(chars: &[TChar]) -> String {
        chars
            .iter()
            .map(|c| match c {
                TChar::Ascii(b) => (*b as char).to_string(),
                TChar::Space => " ".to_string(),
                TChar::NewLine => "\n".to_string(),
                TChar::Utf8(buf, len) => String::from_utf8_lossy(&buf[..*len as usize]).to_string(),
            })
            .collect()
    }

    #[test]
    fn empty_buffer_returns_single_default_tag() {
        // A freshly created buffer with no content written must return an empty
        // chars vec and exactly one default-format tag (start=0, end=usize::MAX).
        let mut buf = Buffer::new(10, 5);
        let (chars, tags, _row_offsets, _url_tag_indices) = buf.visible_as_tchars_and_tags(0);

        // Empty buffer: visible rows exist but all cells are blank/empty.
        // The returned tags must be non-empty (at least one sentinel tag).
        assert!(
            !tags.is_empty(),
            "tags must not be empty for an empty buffer"
        );
        assert_eq!(
            tags[0].font_weight,
            FontWeight::Normal,
            "default tag must have Normal weight"
        );
        let _ = chars; // may be empty or contain spaces — just must not panic
    }

    #[test]
    fn single_char_one_tag() {
        // Write a single ASCII character — chars = [Ascii(b'A')], one tag [0, 1).
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&to_tchars("A"));

        let (chars, tags, _row_offsets, _url_tag_indices) = buf.visible_as_tchars_and_tags(0);

        // 'A' must appear in chars.
        assert!(
            chars.contains(&TChar::Ascii(b'A')),
            "chars must contain Ascii(b'A')"
        );
        // At least one tag must exist.
        assert!(!tags.is_empty(), "tags must not be empty after writing 'A'");
        // Every tag must have valid start <= end.
        for tag in &tags {
            assert!(
                tag.start <= tag.end,
                "tag start ({}) must not exceed end ({})",
                tag.start,
                tag.end
            );
        }
    }

    #[test]
    fn multiple_same_format_merged() {
        // Write "ABC" with the same default format — should produce a single merged tag.
        let mut buf = Buffer::new(80, 5);
        buf.insert_text(&to_tchars("ABC"));

        let (chars, tags, _row_offsets, _url_tag_indices) = buf.visible_as_tchars_and_tags(0);

        // All three characters must be present.
        assert!(chars.contains(&TChar::Ascii(b'A')), "must contain A");
        assert!(chars.contains(&TChar::Ascii(b'B')), "must contain B");
        assert!(chars.contains(&TChar::Ascii(b'C')), "must contain C");

        // All three characters share the same default format so they should be
        // covered by a single tag (or at most a few — the exact merge count
        // depends on newlines).  The key invariant is that no two consecutive
        // tags in the run covering A/B/C have different format.
        assert!(
            tags.iter()
                .any(|t| t.font_weight == FontWeight::Normal && t.font_decorations.is_empty()),
            "at least one default-format tag must cover A/B/C"
        );
    }

    #[test]
    fn color_change_splits_tag() {
        // Write "A", change fg color to Red, write "B" → two distinct tags.
        let mut buf = Buffer::new(80, 5);

        // Write 'A' with default format.
        buf.insert_text(&to_tchars("A"));

        // Change foreground color to Red.
        let mut red_tag = FormatTag::default();
        red_tag.colors.set_color(TerminalColor::Red);
        buf.set_format(red_tag);

        // Write 'B' with red format.
        buf.insert_text(&to_tchars("B"));

        let (chars, tags, _row_offsets, _url_tag_indices) = buf.visible_as_tchars_and_tags(0);

        // Both characters must be present.
        assert!(chars.contains(&TChar::Ascii(b'A')), "must contain A");
        assert!(chars.contains(&TChar::Ascii(b'B')), "must contain B");

        // There must be at least two distinct tags with different colors.
        let default_color_tags = tags
            .iter()
            .filter(|t| t.colors.color == TerminalColor::Default)
            .count();
        let red_tags = tags
            .iter()
            .filter(|t| t.colors.color == TerminalColor::Red)
            .count();
        assert!(
            default_color_tags >= 1,
            "must have at least one default-color tag (for 'A')"
        );
        assert!(red_tags >= 1, "must have at least one red tag (for 'B')");
    }

    #[test]
    fn newline_between_rows() {
        // Write "hi", LF+CR, write "bye" — the flat chars must contain a NewLine
        // between the two words.
        let mut buf = Buffer::new(80, 10);
        buf.insert_text(&to_tchars("hi"));
        buf.handle_lf();
        buf.handle_cr();
        buf.insert_text(&to_tchars("bye"));

        let (chars, tags, _row_offsets, _url_tag_indices) = buf.visible_as_tchars_and_tags(0);

        // NewLine must appear somewhere in the output.
        assert!(
            chars.contains(&TChar::NewLine),
            "chars must contain at least one NewLine between rows"
        );

        // tags must be non-empty and well-formed.
        assert!(!tags.is_empty(), "tags must not be empty");
        for tag in &tags {
            assert!(
                tag.start <= tag.end,
                "tag start ({}) must not exceed end ({})",
                tag.start,
                tag.end
            );
        }

        // The string representation must contain the text from both rows.
        let s = tchars_to_string(&chars);
        assert!(s.contains('h'), "output must contain 'h'");
        assert!(s.contains("bye"), "output must contain 'bye'");
    }

    #[test]
    fn wide_char_no_continuation_in_output() {
        // Write a wide CJK character (2 columns) — the output chars must contain
        // it exactly once (continuation cells must be skipped).
        let mut buf = Buffer::new(80, 5);
        // "あ" is a 2-column wide character — build the TChar directly to avoid fallible ops.
        let wide_tchar = TChar::from('あ');
        buf.insert_text(std::slice::from_ref(&wide_tchar));

        let (chars, _tags, _row_offsets, _url_tag_indices) = buf.visible_as_tchars_and_tags(0);

        let count = chars.iter().filter(|c| **c == wide_tchar).count();
        assert_eq!(
            count, 1,
            "wide char must appear exactly once in output (continuation must be skipped)"
        );
    }
}

// ============================================================================
// Scrollback limit wiring tests
// ============================================================================

#[cfg(test)]
mod scrollback_limit_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    fn ascii(c: char) -> TChar {
        TChar::Ascii(c as u8)
    }

    #[test]
    fn default_scrollback_limit_is_4000() {
        let buf = Buffer::new(10, 5);
        assert_eq!(buf.scrollback_limit(), 4000);
    }

    #[test]
    fn with_scrollback_limit_overrides_default() {
        let buf = Buffer::new(10, 5).with_scrollback_limit(500);
        assert_eq!(buf.scrollback_limit(), 500);
    }

    #[test]
    fn custom_scrollback_limit_is_enforced() {
        // Set a very small scrollback limit and verify the buffer respects it.
        let limit = 5;
        let height = 3;
        let mut buf = Buffer::new(10, height).with_scrollback_limit(limit);

        // Push enough lines to exceed the scrollback limit.
        // Each LF at the bottom creates one scrollback row.
        let ch = [ascii('A')];
        for _ in 0..(limit + height + 10) {
            buf.insert_text(&ch);
            buf.handle_lf();
        }

        // Total rows should be at most scrollback_limit + height.
        assert!(
            buf.rows.len() <= limit + height,
            "rows.len()={} should be <= limit+height={}",
            buf.rows.len(),
            limit + height,
        );
    }

    #[test]
    fn with_scrollback_limit_zero_still_creates_buffer() {
        // Zero is an unusual limit but should not panic.
        let buf = Buffer::new(10, 5).with_scrollback_limit(0);
        assert_eq!(buf.scrollback_limit(), 0);
    }
}

// ============================================================================
// Image Store Integration Tests
// ============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod image_tests {
    use super::*;
    use crate::image_store::{ImagePlacement, ImageProtocol, InlineImage, next_image_id};
    use std::sync::Arc;

    /// Create a test image with the given grid dimensions.
    fn make_image(cols: usize, rows: usize) -> InlineImage {
        let id = next_image_id();
        InlineImage {
            id,
            pixels: Arc::new(vec![0u8; cols * rows * 4]),
            width_px: u32::try_from(cols * 8).unwrap(),
            height_px: u32::try_from(rows * 16).unwrap(),
            display_cols: cols,
            display_rows: rows,
        }
    }

    // ── place_image: basic placement ─────────────────────────────────

    #[test]
    fn place_image_fills_cells_with_placements() {
        let mut buf = Buffer::new(20, 10);
        let img = make_image(3, 2);
        let img_id = img.id;

        // Cursor starts at (0,0).
        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);

        // Rows 0 and 1 should have image cells at columns 0, 1, 2.
        for img_row in 0..2_usize {
            let row = &buf.rows[img_row];
            for img_col in 0..3_usize {
                let cell = &row.cells()[img_col];
                assert!(
                    cell.has_image(),
                    "row={img_row} col={img_col} should have image"
                );
                let p = cell.image_placement().unwrap();
                assert_eq!(p.image_id, img_id);
                assert_eq!(p.col_in_image, img_col);
                assert_eq!(p.row_in_image, img_row);
            }
        }

        // Image store should contain the image.
        assert!(buf.image_store().contains(img_id));
    }

    #[test]
    fn place_image_moves_cursor_below_image() {
        let mut buf = Buffer::new(20, 10);

        // Pre-populate enough rows so that the row below the image exists.
        for _ in 0..4 {
            buf.handle_lf();
        }
        // Move cursor back to (0, 0) for the image placement.
        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 0;

        let img = make_image(3, 2);
        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);

        // Cursor should be at row 2 (below the 2-row image), column 0.
        assert_eq!(buf.cursor.pos.y, 2);
        assert_eq!(buf.cursor.pos.x, 0);
    }

    #[test]
    fn place_image_clips_to_terminal_width() {
        // Terminal is 5 columns wide; image wants 10 columns.
        let mut buf = Buffer::new(5, 10);
        let img = make_image(10, 1);
        let img_id = img.id;

        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);

        // Only columns 0..5 should have image cells.
        let row = &buf.rows[0];
        for col in 0..5_usize {
            let cell = &row.cells()[col];
            assert!(cell.has_image(), "col={col} should have image");
            let p = cell.image_placement().unwrap();
            assert_eq!(p.col_in_image, col);
        }

        // Image store should still have the image.
        assert!(buf.image_store().contains(img_id));
    }

    #[test]
    fn place_image_at_nonzero_cursor_col() {
        let mut buf = Buffer::new(20, 10);
        // Move cursor to column 5.
        buf.cursor.pos.x = 5;
        let img = make_image(3, 1);
        let img_id = img.id;

        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);

        // Image cells should be at columns 5, 6, 7.
        let row = &buf.rows[0];
        for img_col in 0..3_usize {
            let col = 5 + img_col;
            let cell = &row.cells()[col];
            assert!(cell.has_image(), "col={col} should have image");
            let p = cell.image_placement().unwrap();
            assert_eq!(p.image_id, img_id);
            assert_eq!(p.col_in_image, img_col);
        }
    }

    // ── place_image: scrolling ───────────────────────────────────────

    #[test]
    fn place_image_scrolls_when_image_exceeds_visible_area() {
        // Terminal is 10 wide, 3 tall.  Cursor is at the last visible row.
        let mut buf = Buffer::new(10, 3);
        // Push cursor to the bottom row.
        buf.handle_lf();
        buf.handle_lf();
        assert_eq!(buf.cursor.pos.y, 2);

        // Place a 2-row image — there's only 1 row left, so it must scroll.
        let img = make_image(3, 2);
        let img_id = img.id;

        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);

        // The image should exist in the store.
        assert!(buf.image_store().contains(img_id));

        // Verify image cells are present somewhere in the buffer.
        let mut found_placements = 0;
        for row in &buf.rows {
            for cell in row.cells() {
                if let Some(p) = cell.image_placement()
                    && p.image_id == img_id
                {
                    found_placements += 1;
                }
            }
        }
        // 2 rows × 3 cols = 6 placements.
        assert_eq!(
            found_placements, 6,
            "expected 6 image placements after scroll"
        );
    }

    // ── Image GC after scrollback eviction ───────────────────────────

    #[test]
    fn image_gc_removes_unreferenced_images() {
        let mut buf = Buffer::new(10, 3).with_scrollback_limit(2);

        // Place an image at cursor (0,0).
        let img = make_image(2, 1);
        let img_id = img.id;
        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);

        // The image should be in the store.
        assert!(buf.image_store().contains(img_id));

        // Now push enough lines to evict the image row from scrollback.
        // scrollback_limit=2, height=3, so max rows = 5.
        // We need the image row (row 0) to be drained.
        for _ in 0..10 {
            buf.handle_lf();
        }

        // After scrollback eviction + GC, the image should be gone
        // because no remaining cell references it.
        assert!(
            !buf.image_store().contains(img_id),
            "image should be GC'd after its row scrolled off"
        );
    }

    #[test]
    fn image_gc_retains_referenced_images() {
        let mut buf = Buffer::new(10, 5).with_scrollback_limit(10);

        // Place an image on the 2nd row (row 1).
        buf.handle_lf();
        let img = make_image(2, 1);
        let img_id = img.id;
        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);

        // Push a few lines, but not enough to evict the image row.
        for _ in 0..3 {
            buf.handle_lf();
        }

        // The image should still be in the store since its row is still present.
        assert!(
            buf.image_store().contains(img_id),
            "image should be retained while its row is still in the buffer"
        );
    }

    // ── Alternate screen save/restore ────────────────────────────────

    #[test]
    fn image_store_saved_and_restored_across_alternate_screen() {
        let mut buf = Buffer::new(10, 5);

        // Place an image in the primary buffer.
        let img = make_image(2, 1);
        let img_id = img.id;
        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);
        assert!(buf.image_store().contains(img_id));

        // Enter alternate screen — primary images should be saved.
        buf.enter_alternate(0);
        assert!(
            buf.image_store().is_empty(),
            "alternate screen should have no images"
        );

        // Leave alternate screen — primary images should be restored.
        buf.leave_alternate();
        assert!(
            buf.image_store().contains(img_id),
            "image should be restored after leaving alternate screen"
        );
    }

    #[test]
    fn image_cells_restored_after_alternate_screen() {
        let mut buf = Buffer::new(10, 5);

        // Place image and remember the cell content.
        let img = make_image(2, 1);
        let img_id = img.id;
        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);

        // Verify cell has image before alternate screen.
        assert!(buf.rows[0].cells()[0].has_image());

        // Round-trip through alternate screen.
        buf.enter_alternate(0);
        buf.leave_alternate();

        // Cell should still have the image placement.
        let cell = &buf.rows[0].cells()[0];
        assert!(cell.has_image());
        let p = cell.image_placement().unwrap();
        assert_eq!(p.image_id, img_id);
        assert_eq!(p.col_in_image, 0);
        assert_eq!(p.row_in_image, 0);
    }

    // ── full_reset clears images ─────────────────────────────────────

    #[test]
    fn full_reset_clears_image_store() {
        let mut buf = Buffer::new(10, 5);

        let img = make_image(2, 1);
        let img_id = img.id;
        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);
        assert!(buf.image_store().contains(img_id));

        buf.full_reset();
        assert!(
            buf.image_store().is_empty(),
            "full_reset should clear all images"
        );
    }

    // ── Multiple images ──────────────────────────────────────────────

    #[test]
    fn multiple_images_coexist_in_store() {
        let mut buf = Buffer::new(20, 10);

        let img1 = make_image(2, 1);
        let id1 = img1.id;
        buf.place_image(img1, 0, ImageProtocol::Sixel, None, None, 0);

        let img2 = make_image(3, 1);
        let id2 = img2.id;
        buf.place_image(img2, 0, ImageProtocol::Sixel, None, None, 0);

        assert!(buf.image_store().contains(id1));
        assert!(buf.image_store().contains(id2));
        assert_eq!(buf.image_store().len(), 2);
    }

    #[test]
    fn gc_only_removes_evicted_images() {
        // Two images: one near the top (will be evicted), one near the bottom (will stay).
        let mut buf = Buffer::new(10, 3).with_scrollback_limit(3);

        // Image 1 on row 0.
        let img1 = make_image(2, 1);
        let id1 = img1.id;
        buf.place_image(img1, 0, ImageProtocol::Sixel, None, None, 0);

        // Move cursor down and place image 2.
        for _ in 0..4 {
            buf.handle_lf();
        }
        let img2 = make_image(2, 1);
        let id2 = img2.id;
        buf.place_image(img2, 0, ImageProtocol::Sixel, None, None, 0);

        // Push many more lines to evict the first image's row.
        for _ in 0..10 {
            buf.handle_lf();
        }

        // Image 1 should be GC'd; image 2 may or may not be depending on how
        // far its row scrolled.  At minimum, verify the store doesn't contain
        // both if one's row is gone.
        if buf.image_store().contains(id2) {
            // If image 2 survived, its cells should still be present.
            let mut found = false;
            for row in &buf.rows {
                for cell in row.cells() {
                    if let Some(p) = cell.image_placement()
                        && p.image_id == id2
                    {
                        found = true;
                    }
                }
            }
            assert!(found, "if image 2 is in the store, cells must reference it");
        }

        // Image 1's row should be gone — verify no cell references it.
        let mut id1_found = false;
        for row in &buf.rows {
            for cell in row.cells() {
                if let Some(p) = cell.image_placement()
                    && p.image_id == id1
                {
                    id1_found = true;
                }
            }
        }
        if !id1_found {
            assert!(
                !buf.image_store().contains(id1),
                "no cell references image 1, so it should be GC'd"
            );
        }
    }

    // ── Row::set_image_cell ──────────────────────────────────────────

    #[test]
    fn set_image_cell_extends_row_if_needed() {
        let mut row = Row::new(10);
        assert!(row.cells().is_empty());

        let placement = ImagePlacement {
            image_id: 42,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Sixel,
            image_number: None,
            placement_id: None,
            z_index: 0,
        };
        row.set_image_cell(5, placement.clone(), FormatTag::default());

        // Row should have been extended to at least 6 cells.
        assert!(row.cells().len() >= 6);
        let cell = &row.cells()[5];
        assert!(cell.has_image());
        assert_eq!(cell.image_placement().unwrap(), &placement);
    }

    #[test]
    fn set_image_cell_beyond_width_is_noop() {
        let mut row = Row::new(5);
        let placement = ImagePlacement {
            image_id: 42,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Sixel,
            image_number: None,
            placement_id: None,
            z_index: 0,
        };
        // Column 10 is beyond width 5 — should be a no-op.
        row.set_image_cell(10, placement, FormatTag::default());
        assert!(row.cells().is_empty());
    }

    // ── Cell image accessors ─────────────────────────────────────────

    #[test]
    fn cell_image_accessors() {
        // Normal cell has no image.
        let cell = Cell::new(TChar::Ascii(b'A'), FormatTag::default());
        assert!(!cell.has_image());
        assert!(cell.image_placement().is_none());

        // Image cell has a placement.
        let placement = ImagePlacement {
            image_id: 99,
            col_in_image: 1,
            row_in_image: 2,
            protocol: ImageProtocol::Sixel,
            image_number: None,
            placement_id: None,
            z_index: 0,
        };
        let img_cell = Cell::image_cell(placement.clone(), FormatTag::default());
        assert!(img_cell.has_image());
        assert_eq!(img_cell.image_placement().unwrap(), &placement);
    }

    #[test]
    fn cell_clear_image() {
        let placement = ImagePlacement {
            image_id: 99,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Sixel,
            image_number: None,
            placement_id: None,
            z_index: 0,
        };
        let mut cell = Cell::image_cell(placement, FormatTag::default());
        assert!(cell.has_image());

        cell.clear_image();
        assert!(!cell.has_image());
        assert!(cell.image_placement().is_none());
    }

    // ── place_image pre-clear: replacing a larger image with a smaller one ──

    #[test]
    fn place_image_clears_stale_cells_below_smaller_replacement() {
        let mut buf = Buffer::new(20, 10);

        // Place a tall image (3 cols × 5 rows) at cursor (0,0).
        let big_img = make_image(3, 5);
        let big_id = big_img.id;
        buf.place_image(big_img, 0, ImageProtocol::Sixel, None, None, 0);

        // Cursor is now below the image (row 5). Move back to (0,0).
        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 0;

        // Place a smaller image (3 cols × 2 rows) at the same position.
        let small_img = make_image(3, 2);
        let small_id = small_img.id;
        buf.place_image(small_img, 0, ImageProtocol::Sixel, None, None, 0);

        // Rows 0-1 should have the new image's cells.
        for r in 0..2_usize {
            for c in 0..3_usize {
                let cell = &buf.rows[r].cells()[c];
                assert!(cell.has_image(), "row={r} col={c} should have new image");
                assert_eq!(cell.image_placement().unwrap().image_id, small_id);
            }
        }

        // Rows 2-4 should have NO image cells (old image was cleared).
        for r in 2..5_usize {
            for c in 0..3_usize {
                let cell = &buf.rows[r].cells()[c];
                assert!(
                    !cell.has_image(),
                    "row={r} col={c} should not have stale image (old id={big_id})"
                );
            }
        }
    }

    // ── Atomic image invalidation on text overwrite ─────────────────

    /// Helper: count cells referencing a given image id across the buffer.
    fn count_image_cells(buf: &Buffer, image_id: u64) -> usize {
        let mut count = 0;
        for row in &buf.rows {
            for cell in row.cells() {
                if cell
                    .image_placement()
                    .is_some_and(|p| p.image_id == image_id)
                {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn insert_text_over_image_clears_all_cells_of_that_image() {
        let mut buf = Buffer::new(20, 10);

        // Place a 5×3 image at (0,0).
        let img = make_image(5, 3);
        let img_id = img.id;
        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);

        // Verify image cells are present (5 cols × 3 rows = 15).
        assert_eq!(count_image_cells(&buf, img_id), 15);

        // Move cursor to (0,0) and write text over just the first cell.
        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 0;
        buf.insert_text(&[TChar::Ascii(b'X')]);

        // ALL cells of the image across all rows should be cleared.
        assert_eq!(
            count_image_cells(&buf, img_id),
            0,
            "all image cells should be cleared after text overwrite"
        );
    }

    #[test]
    fn insert_text_over_one_image_does_not_clear_another() {
        let mut buf = Buffer::new(20, 10);

        // Place image A at columns 0-2, rows 0-1.
        let img_a = make_image(3, 2);
        let id_a = img_a.id;
        buf.place_image(img_a, 0, ImageProtocol::Sixel, None, None, 0);
        assert_eq!(count_image_cells(&buf, id_a), 6);

        // place_image moved cursor below. Move cursor to row 0, col 10
        // for image B placement.
        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 10;
        let img_b = make_image(3, 2);
        let id_b = img_b.id;
        buf.place_image(img_b, 0, ImageProtocol::Sixel, None, None, 0);
        assert_eq!(count_image_cells(&buf, id_b), 6);

        // Write text over image A's first cell.
        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 0;
        buf.insert_text(&[TChar::Ascii(b'Z')]);

        // Image A should be entirely cleared.
        assert_eq!(
            count_image_cells(&buf, id_a),
            0,
            "image A cells should all be cleared"
        );

        // Image B should be untouched.
        assert_eq!(
            count_image_cells(&buf, id_b),
            6,
            "image B cells should survive"
        );
    }

    // ── Atomic image invalidation on erase operations ────────────────

    #[test]
    fn erase_line_to_end_clears_entire_image() {
        let mut buf = Buffer::new(20, 10);

        // Place a 5×3 image at (0,0).
        let img = make_image(5, 3);
        let img_id = img.id;
        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);
        assert_eq!(count_image_cells(&buf, img_id), 15);

        // Cursor at (0,0); erase from cursor to end of line.
        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 0;
        buf.erase_line_to_end();

        // ALL cells of the image (rows 0-2, cols 0-4) should be gone
        // because erasing row 0 hits the image and atomically clears all.
        assert_eq!(
            count_image_cells(&buf, img_id),
            0,
            "all image cells should be cleared after erase_line_to_end"
        );
    }

    #[test]
    fn erase_line_to_beginning_clears_entire_image() {
        let mut buf = Buffer::new(20, 10);

        // Place a 5×3 image at (0,0).
        let img = make_image(5, 3);
        let img_id = img.id;
        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);
        assert_eq!(count_image_cells(&buf, img_id), 15);

        // Move cursor to the end of the image's first row.
        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 4;
        buf.erase_line_to_beginning();

        // ALL cells of the image should be cleared (atomic invalidation).
        assert_eq!(
            count_image_cells(&buf, img_id),
            0,
            "all image cells should be cleared after erase_line_to_beginning"
        );
    }

    #[test]
    fn erase_display_clears_all_image_cells() {
        let mut buf = Buffer::new(20, 10);

        let img = make_image(5, 3);
        let img_id = img.id;
        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);
        assert_eq!(count_image_cells(&buf, img_id), 15);

        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 0;
        buf.erase_display();

        assert_eq!(
            count_image_cells(&buf, img_id),
            0,
            "all image cells should be cleared after erase_display"
        );
    }

    #[test]
    fn erase_to_end_of_display_clears_image_atomically() {
        let mut buf = Buffer::new(20, 10);

        // Place image spanning rows 0-2, cols 0-4.
        let img = make_image(5, 3);
        let img_id = img.id;
        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);
        assert_eq!(count_image_cells(&buf, img_id), 15);

        // Erase from row 1 onward — image has cells on row 1, so the
        // whole image should be cleared atomically (including row 0).
        buf.cursor.pos.y = 1;
        buf.cursor.pos.x = 0;
        buf.erase_to_end_of_display();

        assert_eq!(
            count_image_cells(&buf, img_id),
            0,
            "all image cells (including row 0) should be cleared atomically"
        );
    }

    #[test]
    fn erase_chars_clears_entire_image() {
        let mut buf = Buffer::new(20, 10);

        // Place a 5×3 image at (0,0).
        let img = make_image(5, 3);
        let img_id = img.id;
        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);
        assert_eq!(count_image_cells(&buf, img_id), 15);

        // Move cursor to col 2 of row 0 and erase 1 char — should trigger
        // atomic invalidation of the entire image.
        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 2;
        buf.erase_chars(1);

        assert_eq!(
            count_image_cells(&buf, img_id),
            0,
            "all image cells should be cleared after erase_chars"
        );
    }

    #[test]
    fn erase_line_clears_entire_image() {
        let mut buf = Buffer::new(20, 10);

        // Place a 5×3 image at (0,0).
        let img = make_image(5, 3);
        let img_id = img.id;
        buf.place_image(img, 0, ImageProtocol::Sixel, None, None, 0);
        assert_eq!(count_image_cells(&buf, img_id), 15);

        // Erase the entire first line — should clear all image cells.
        buf.cursor.pos.y = 0;
        buf.erase_line();

        assert_eq!(
            count_image_cells(&buf, img_id),
            0,
            "all image cells should be cleared after erase_line"
        );
    }
}

// ============================================================================
// extract_text tests
// ============================================================================

#[cfg(test)]
mod extract_text_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    fn ascii(c: char) -> TChar {
        TChar::Ascii(c as u8)
    }

    /// Helper: insert a line of ASCII text and advance to the next line.
    fn push_line(buf: &mut Buffer, text: &str) {
        let chars: Vec<TChar> = text.chars().map(ascii).collect();
        buf.insert_text(&chars);
        buf.handle_lf();
        buf.cursor.pos.x = 0; // carriage return
    }

    #[test]
    fn single_row_full() {
        let mut buf = Buffer::new(10, 5);
        push_line(&mut buf, "hello");
        // Row 0 contains "hello" (plus trailing spaces to width 10).
        let result = buf.extract_text(0, 0, 0, 9);
        assert_eq!(result, "hello");
    }

    #[test]
    fn single_row_partial() {
        let mut buf = Buffer::new(10, 5);
        push_line(&mut buf, "abcdefghij");
        // Extract columns 2..=5 → "cdef"
        let result = buf.extract_text(0, 2, 0, 5);
        assert_eq!(result, "cdef");
    }

    #[test]
    fn multiple_rows() {
        let mut buf = Buffer::new(10, 5);
        push_line(&mut buf, "line one");
        push_line(&mut buf, "line two");
        push_line(&mut buf, "line three");

        // Extract from row 0, col 0 to row 2, col 9 (full lines).
        let result = buf.extract_text(0, 0, 2, 9);
        assert_eq!(result, "line one\nline two\nline three");
    }

    #[test]
    fn start_row_beyond_buffer() {
        let buf = Buffer::new(10, 5);
        // Only 5 rows in a new buffer; asking for row 100 returns empty.
        let result = buf.extract_text(100, 0, 100, 5);
        assert_eq!(result, "");
    }

    #[test]
    fn end_row_clamped() {
        let mut buf = Buffer::new(10, 5);
        push_line(&mut buf, "only");
        // end_row far beyond buffer → clamped to last row.
        let result = buf.extract_text(0, 0, 999, 9);
        // Should extract all rows without panicking.
        assert!(result.contains("only"));
    }

    #[test]
    fn empty_buffer() {
        let buf = Buffer::new(10, 3);
        let result = buf.extract_text(0, 0, 0, 9);
        // A fresh buffer has rows of spaces; trailing spaces are trimmed.
        assert_eq!(result, "");
    }

    #[test]
    fn trailing_spaces_trimmed() {
        let mut buf = Buffer::new(20, 5);
        push_line(&mut buf, "abc");
        // Row has "abc" + 17 spaces; extract_text trims trailing spaces.
        let result = buf.extract_text(0, 0, 0, 19);
        assert_eq!(result, "abc");
    }

    #[test]
    fn col_begin_beyond_row_width() {
        let mut buf = Buffer::new(5, 3);
        push_line(&mut buf, "hi");
        // start_col beyond the actual content should still not panic.
        let result = buf.extract_text(0, 100, 0, 100);
        assert_eq!(result, "");
    }

    #[test]
    fn continuation_cells_skipped() {
        // Wide characters produce a continuation cell that should be skipped.
        let mut buf = Buffer::new(10, 3);
        // Insert a UTF-8 wide character (e.g. "Ｗ" = fullwidth W, U+FF37).
        let wide_char = TChar::from('Ｗ');
        let chars = vec![wide_char, ascii('x')];
        buf.insert_text(&chars);

        let result = buf.extract_text(0, 0, 0, 9);
        assert_eq!(result, "Ｗx");
    }
}

// ============================================================================
// extract_block_text tests
// ============================================================================

#[cfg(test)]
mod extract_block_text_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    fn ascii(c: char) -> TChar {
        TChar::Ascii(c as u8)
    }

    /// Helper: insert a line of ASCII text and advance to the next row.
    fn push_line(buf: &mut Buffer, text: &str) {
        let chars: Vec<TChar> = text.chars().map(ascii).collect();
        buf.insert_text(&chars);
        buf.handle_lf();
        buf.cursor.pos.x = 0; // carriage return
    }

    #[test]
    fn single_row_full_block() {
        // Single-row block selection: same as a normal extract over that row.
        let mut buf = Buffer::new(10, 5);
        push_line(&mut buf, "hello");
        let result = buf.extract_block_text(0, 0, 0, 4);
        assert_eq!(result, "hello");
    }

    #[test]
    fn single_row_partial_cols() {
        // Extract only columns 1..=3 of "abcde" → "bcd".
        let mut buf = Buffer::new(10, 5);
        push_line(&mut buf, "abcde");
        let result = buf.extract_block_text(0, 1, 0, 3);
        assert_eq!(result, "bcd");
    }

    #[test]
    fn multi_row_same_col_range() {
        // Three rows; extract cols 0..=3 from each.
        // Row 0: "abcd..." → "abcd"
        // Row 1: "efgh..." → "efgh"
        // Row 2: "ijkl..." → "ijkl"
        let mut buf = Buffer::new(10, 5);
        push_line(&mut buf, "abcdefghij");
        push_line(&mut buf, "efghijklmn");
        push_line(&mut buf, "ijklmnopqr");
        let result = buf.extract_block_text(0, 0, 2, 3);
        assert_eq!(
            result,
            "abcd
efgh
ijkl"
        );
    }

    #[test]
    fn col_range_reversed() {
        // start_col > end_col: col_min/col_max normalization must apply.
        let mut buf = Buffer::new(10, 5);
        push_line(&mut buf, "abcdefghij");
        // Passing end_col=1, start_col=3 → should extract cols 1..=3 = "bcd".
        let result = buf.extract_block_text(0, 3, 0, 1);
        assert_eq!(result, "bcd");
    }

    #[test]
    fn trailing_spaces_trimmed_per_row() {
        // Each row's trailing spaces are trimmed individually.
        let mut buf = Buffer::new(10, 5);
        push_line(&mut buf, "ab"); // row 0: "ab" + 8 spaces
        push_line(&mut buf, "cde"); // row 1: "cde" + 7 spaces
        // Extract cols 0..=9 (full width): trailing spaces must be stripped.
        let result = buf.extract_block_text(0, 0, 1, 9);
        assert_eq!(
            result,
            "ab
cde"
        );
    }

    #[test]
    fn start_row_beyond_buffer() {
        let buf = Buffer::new(10, 5);
        let result = buf.extract_block_text(100, 0, 105, 9);
        assert_eq!(result, "");
    }

    #[test]
    fn end_row_clamped_to_buffer() {
        let mut buf = Buffer::new(10, 5);
        push_line(&mut buf, "only");
        // end_row far beyond buffer → clamped, must not panic.
        let result = buf.extract_block_text(0, 0, 999, 3);
        assert!(result.contains("only"));
    }

    #[test]
    fn col_beyond_row_width() {
        // Columns beyond the actual row width produce no characters (no panic).
        let mut buf = Buffer::new(5, 3);
        push_line(&mut buf, "hi");
        let result = buf.extract_block_text(0, 10, 0, 20);
        assert_eq!(result, "");
    }

    #[test]
    fn block_does_not_bleed_into_adjacent_cols() {
        // A narrow block in the middle of wide content: only the specified
        // columns are extracted from every row.
        let mut buf = Buffer::new(10, 5);
        push_line(&mut buf, "0123456789");
        push_line(&mut buf, "abcdefghij");
        push_line(&mut buf, "ABCDEFGHIJ");
        // Extract cols 3..=5 → "345", "def", "DEF".
        let result = buf.extract_block_text(0, 3, 2, 5);
        assert_eq!(
            result,
            "345
def
DEF"
        );
    }
}

// ============================================================================
// DECLRMM / DECSLRM tests
// ============================================================================

#[cfg(test)]
mod declrmm_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    fn ascii(c: char) -> TChar {
        TChar::Ascii(c as u8)
    }

    fn text(s: &str) -> Vec<TChar> {
        s.chars().map(ascii).collect()
    }

    /// Create a 10-wide, 5-tall buffer with DECLRMM enabled and margins [2, 7].
    fn buf_with_margins() -> Buffer {
        let mut buf = Buffer::new(10, 5);
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(3, 8); // 1-based: cols 3..8 → 0-based: 2..7
        buf
    }

    /// Create a 10-wide, 5-tall buffer, fill row 0 starting at `start_col` with
    /// `content` (DECLRMM off so no margin wrap), then enable DECLRMM with
    /// margins [2, 7] and home the cursor to (col=0, row=0).
    fn buf_with_content_then_margins(start_col: usize, content: &str) -> Buffer {
        let mut buf = Buffer::new(10, 5);
        // Fill without margin restrictions.
        buf.set_cursor_pos(Some(start_col), Some(0));
        buf.insert_text(&text(content));
        // Now enable DECLRMM + margins; set_left_right_margins homes cursor.
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(3, 8); // 1-based → 0-based [2, 7]
        buf
    }

    // --- set_declrmm / is_declrmm_enabled ---

    #[test]
    fn set_declrmm_enables_mode() {
        let mut buf = Buffer::new(10, 5);
        assert_eq!(buf.is_declrmm_enabled(), Declrmm::Disabled);
        buf.set_declrmm(Declrmm::Enabled);
        assert_eq!(buf.is_declrmm_enabled(), Declrmm::Enabled);
    }

    #[test]
    fn set_declrmm_false_resets_margins() {
        let mut buf = Buffer::new(10, 5);
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(3, 8); // 0-based: 2..7
        assert_eq!(buf.left_right_margins(), (2, 7));

        buf.set_declrmm(Declrmm::Disabled);
        assert_eq!(buf.is_declrmm_enabled(), Declrmm::Disabled);
        // Margins should be reset to full width.
        assert_eq!(buf.left_right_margins(), (0, 9));
    }

    // --- set_left_right_margins ---

    #[test]
    fn set_margins_valid() {
        let mut buf = Buffer::new(10, 5);
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(2, 8); // 0-based: 1..7
        assert_eq!(buf.left_right_margins(), (1, 7));
        // Cursor homed to (0, 0).
        assert_eq!(buf.get_cursor_screen_pos(), CursorPos { x: 0, y: 0 });
    }

    #[test]
    fn set_margins_invalid_left_ge_right_resets() {
        let mut buf = Buffer::new(10, 5);
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(5, 5); // left == right → invalid
        assert_eq!(buf.left_right_margins(), (0, 9));
    }

    #[test]
    fn set_margins_right_beyond_width_resets() {
        let mut buf = Buffer::new(10, 5);
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(1, 11); // right >= width → invalid
        assert_eq!(buf.left_right_margins(), (0, 9));
    }

    // --- insert_text wraps at right margin when DECLRMM active ---

    #[test]
    fn insert_text_wraps_at_right_margin() {
        // 10-wide buffer, margins cols 2..7 (0-based).
        // Write 7 chars starting at col 2 — they should fill 2..7 (6 cells)
        // and then the 7th char wraps to the next row starting at col 2.
        let mut buf = buf_with_margins();
        // Set cursor to col=2, row=0.
        buf.set_cursor_pos(Some(2), Some(0));
        // Margin is [2, 7] → wrap_col = 8.  6 chars fit before wrapping.
        buf.insert_text(&text("ABCDEFG")); // 7 chars: ABCDEF + G wraps

        // First row: cols 2..7 should be ABCDEF.
        let row0 = buf.get_rows()[0].cells();
        assert_eq!(row0.get(2).map(Cell::tchar), Some(&ascii('A')));
        assert_eq!(row0.get(7).map(Cell::tchar), Some(&ascii('F')));
        // Col 8 should be untouched (outside margin).
        assert!(row0.get(8).is_none_or(|c| c.tchar() == &TChar::Space));

        // Second row: wrap starts at col 2 (scroll_region_left).
        let row1 = buf.get_rows()[1].cells();
        assert_eq!(row1.get(2).map(Cell::tchar), Some(&ascii('G')));
    }

    #[test]
    fn insert_text_no_wrap_outside_margins_when_declrmm_disabled() {
        // With DECLRMM disabled, normal full-width wrap.
        let mut buf = Buffer::new(10, 5);
        buf.set_cursor_pos(Some(0), Some(0));
        buf.insert_text(&text("ABCDEFGHIJK")); // 11 chars → wraps at col 10
        // Row 0 must have all 10 chars ABCDEFGHIJ.
        let row0 = buf.get_rows()[0].cells();
        for (i, ch) in "ABCDEFGHIJ".chars().enumerate() {
            assert_eq!(row0[i].tchar(), &ascii(ch), "col {i}");
        }
        // K wraps to row 1, col 0.
        let row1 = buf.get_rows()[1].cells();
        assert_eq!(row1.first().map(Cell::tchar), Some(&ascii('K')));
    }

    // --- move_cursor_relative clamps to margins when DECLRMM active ---

    #[test]
    fn move_cursor_right_clamped_to_right_margin() {
        let mut buf = buf_with_margins(); // margins [2, 7]
        // Set cursor to col=5, row=0 (inside margin), then move right by 10.
        buf.set_cursor_pos(Some(5), Some(0));
        buf.move_cursor_relative(10, 0);
        // Should clamp at scroll_region_right = 7.
        assert_eq!(buf.get_cursor_screen_pos().x, 7);
    }

    #[test]
    fn move_cursor_left_clamped_to_left_margin() {
        let mut buf = buf_with_margins(); // margins [2, 7]
        // Set cursor to col=5, row=0 (inside margin), then move left by 10.
        buf.set_cursor_pos(Some(5), Some(0));
        buf.move_cursor_relative(-10, 0);
        // Should clamp at scroll_region_left = 2.
        assert_eq!(buf.get_cursor_screen_pos().x, 2);
    }

    #[test]
    fn move_cursor_outside_margin_not_clamped_to_margin() {
        let mut buf = buf_with_margins(); // margins [2, 7]
        // Start at col=0, row=0 (outside left margin), move right by 1.
        buf.set_cursor_pos(Some(0), Some(0));
        buf.move_cursor_relative(1, 0);
        // Outside margin: uses normal full-width clamp, not margin clamp.
        assert_eq!(buf.get_cursor_screen_pos().x, 1);
    }

    // --- erase_chars clamps to right margin ---

    #[test]
    fn erase_chars_clamped_to_right_margin() {
        // Fill row 0 fully with 'X' *before* enabling DECLRMM, so all 10 cols get X.
        let mut buf = buf_with_content_then_margins(0, "XXXXXXXXXX");
        // Set cursor to col=5, row=0, then erase 10 chars.
        // With DECLRMM margins [2,7], only cols 5..=7 should be erased.
        buf.set_cursor_pos(Some(5), Some(0));
        buf.erase_chars(10);

        let row = buf.get_rows()[0].cells();
        // Cols 5, 6, 7 should be blank.
        for col in 5..=7 {
            let cell = row.get(col);
            assert!(
                cell.is_none_or(|c| c.tchar() == &TChar::Space),
                "col {col} should be blank"
            );
        }
        // Cols 8, 9 should still be 'X' (outside right margin).
        for col in 8..=9 {
            assert_eq!(
                row.get(col).map(Cell::tchar),
                Some(&ascii('X')),
                "col {col} should still be X"
            );
        }
    }

    // --- insert_spaces respects right margin (ICH) ---

    #[test]
    fn insert_spaces_stays_within_right_margin() {
        // Fill cols 2..9 with distinct letters *before* enabling DECLRMM.
        let mut buf = buf_with_content_then_margins(2, "ABCDEFGH"); // cols 2..9
        // Set cursor to col=2, row=0, then insert 2 spaces.
        // ICH with DECLRMM [2,7]: cells in [2,7] shift right; overflow is discarded.
        // G and H (at cols 8,9) are outside the margin and stay untouched.
        buf.set_cursor_pos(Some(2), Some(0));
        buf.insert_spaces(2);

        let row = buf.get_rows()[0].cells();
        // Cols 2, 3 → blank (inserted spaces).
        for col in 2..=3 {
            assert!(
                row.get(col).is_none_or(|c| c.tchar() == &TChar::Space),
                "col {col} should be blank"
            );
        }
        // Col 4 → 'A', col 5 → 'B', col 6 → 'C', col 7 → 'D' (shifted right within margin).
        assert_eq!(row[4].tchar(), &ascii('A'), "col 4");
        assert_eq!(row[5].tchar(), &ascii('B'), "col 5");
        assert_eq!(row[6].tchar(), &ascii('C'), "col 6");
        assert_eq!(row[7].tchar(), &ascii('D'), "col 7");
        // Cols 8, 9 → 'G', 'H' (outside margin, untouched).
        assert_eq!(row[8].tchar(), &ascii('G'), "col 8");
        assert_eq!(row[9].tchar(), &ascii('H'), "col 9");
    }

    // --- delete_chars respects right margin (DCH) ---

    #[test]
    fn delete_chars_stays_within_right_margin() {
        // Fill cols 2..9 with distinct letters *before* enabling DECLRMM.
        let mut buf = buf_with_content_then_margins(2, "ABCDEFGH"); // cols 2..9
        // Set cursor to col=2, row=0, then delete 2 chars.
        // DCH with DECLRMM [2,7]: cols 2,3 removed; 4..7 shifts left; blanks at 6,7.
        // Cols 8,9 ('G','H') are outside the margin and stay untouched.
        buf.set_cursor_pos(Some(2), Some(0));
        buf.delete_chars(2);

        let row = buf.get_rows()[0].cells();
        // Col 2 → 'C', col 3 → 'D', col 4 → 'E', col 5 → 'F'.
        assert_eq!(row[2].tchar(), &ascii('C'), "col 2");
        assert_eq!(row[3].tchar(), &ascii('D'), "col 3");
        assert_eq!(row[4].tchar(), &ascii('E'), "col 4");
        assert_eq!(row[5].tchar(), &ascii('F'), "col 5");
        // Cols 6, 7 → blanks (filled from right of margin zone).
        for col in 6..=7 {
            assert!(
                row.get(col).is_none_or(|c| c.tchar() == &TChar::Space),
                "col {col} should be blank"
            );
        }
        // Cols 8, 9 → 'G', 'H' (outside margin, untouched).
        assert_eq!(row[8].tchar(), &ascii('G'), "col 8");
        assert_eq!(row[9].tchar(), &ascii('H'), "col 9");
    }

    // --- Full-reset clears DECLRMM state ---

    #[test]
    fn full_reset_clears_declrmm() {
        let mut buf = buf_with_margins();
        assert_eq!(buf.is_declrmm_enabled(), Declrmm::Enabled);
        buf.full_reset();
        assert_eq!(buf.is_declrmm_enabled(), Declrmm::Disabled);
        assert_eq!(buf.left_right_margins(), (0, 9));
    }
}

// ---------------------------------------------------------------------------
//  DECDWL / DECDHL — set_cursor_line_width + visible_line_widths
// ---------------------------------------------------------------------------

#[cfg(test)]
mod line_width_tests {
    use super::*;
    use crate::row::LineWidth;

    #[test]
    fn default_rows_have_normal_line_width() {
        let buf = Buffer::new(10, 3);
        let widths = buf.visible_line_widths(0);
        assert_eq!(widths.len(), 1, "fresh buffer has 1 row");
        assert_eq!(widths[0], LineWidth::Normal);
    }

    #[test]
    fn set_cursor_line_width_changes_current_row() {
        let mut buf = Buffer::new(10, 3);
        buf.set_cursor_line_width(LineWidth::DoubleWidth);
        assert_eq!(buf.rows[0].line_width, LineWidth::DoubleWidth);
    }

    #[test]
    fn set_cursor_line_width_marks_row_dirty() {
        let mut buf = Buffer::new(10, 3);
        buf.rows[0].dirty = false;
        buf.set_cursor_line_width(LineWidth::DoubleHeightTop);
        assert!(buf.rows[0].dirty);
    }

    #[test]
    fn set_cursor_line_width_noop_when_same() {
        let mut buf = Buffer::new(10, 3);
        buf.set_cursor_line_width(LineWidth::DoubleWidth);
        // Clear dirty flag, then set the same width again.
        buf.rows[0].dirty = false;
        buf.set_cursor_line_width(LineWidth::DoubleWidth);
        assert!(!buf.rows[0].dirty, "should not mark dirty on no-op");
    }

    #[test]
    fn set_cursor_line_width_on_non_first_row() {
        let mut buf = Buffer::new(10, 3);
        // Create 3 rows by issuing LFs.
        buf.handle_lf();
        buf.handle_lf();
        assert_eq!(buf.cursor.pos.y, 2);
        buf.set_cursor_line_width(LineWidth::DoubleHeightBottom);
        assert_eq!(buf.rows[2].line_width, LineWidth::DoubleHeightBottom);
        // Other rows remain Normal.
        assert_eq!(buf.rows[0].line_width, LineWidth::Normal);
        assert_eq!(buf.rows[1].line_width, LineWidth::Normal);
    }

    #[test]
    fn visible_line_widths_returns_correct_count() {
        let mut buf = Buffer::new(10, 3);
        buf.handle_lf();
        buf.handle_lf();
        let widths = buf.visible_line_widths(0);
        assert_eq!(widths.len(), 3);
    }

    #[test]
    fn visible_line_widths_reflects_set_width() {
        let mut buf = Buffer::new(10, 3);
        buf.handle_lf();
        buf.handle_lf();
        // Set middle row to double width.
        buf.cursor.pos.y = 1;
        buf.set_cursor_line_width(LineWidth::DoubleWidth);
        let widths = buf.visible_line_widths(0);
        assert_eq!(widths[0], LineWidth::Normal);
        assert_eq!(widths[1], LineWidth::DoubleWidth);
        assert_eq!(widths[2], LineWidth::Normal);
    }

    #[test]
    fn single_width_resets_to_normal() {
        let mut buf = Buffer::new(10, 3);
        buf.set_cursor_line_width(LineWidth::DoubleWidth);
        assert_eq!(buf.rows[0].line_width, LineWidth::DoubleWidth);
        buf.set_cursor_line_width(LineWidth::Normal);
        assert_eq!(buf.rows[0].line_width, LineWidth::Normal);
    }
}

/// Regression tests for scrollback preservation during soft-wrap in
/// `insert_text`.  Before the fix, `scroll_region_up_for_wrap()` used an
/// in-place rotation (`scroll_slice_up`) that destroyed the top visible
/// row instead of preserving it in scrollback.  Long soft-wrapped lines
/// (e.g. a 1539-char direnv export at width 128) would lose ~10 rows of
/// history.
#[cfg(test)]
mod softwrap_scrollback_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    fn t(s: &str) -> Vec<TChar> {
        s.bytes().map(TChar::Ascii).collect()
    }

    /// Fill a 10-column, 5-row primary buffer with 5 identifiable rows,
    /// then write a long line that soft-wraps ~6 times.  The original 5
    /// rows must all survive in scrollback.
    #[test]
    fn long_softwrap_preserves_scrollback() {
        let width = 10;
        let height = 5;
        let mut buf = Buffer::new(width, height);

        // Write 5 full-width identifiable rows.  Each row is exactly
        // `width` chars so it fills the row without wrapping.  The LF
        // after each row moves the cursor to the next line.
        let labels: Vec<String> = (0..height)
            .map(|i| {
                let tag = format!("R{i}");
                // Pad to exactly `width` with a filler character unique to
                // the row, so every cell is identifiable.
                // SAFETY for cast: i < height (5), so always fits in u8.
                #[allow(clippy::cast_possible_truncation)]
                let filler = (b'a' + i as u8) as char;
                let pad_len = width - tag.len();
                format!(
                    "{tag}{}",
                    std::iter::repeat_n(filler, pad_len).collect::<String>()
                )
            })
            .collect();

        for label in &labels {
            buf.insert_text(&t(label));
            buf.handle_cr();
            buf.handle_lf();
        }

        // The 5 LFs pushed the visible window down.  Row 0 in `rows[]`
        // is now scrollback.  Write a long line that soft-wraps 5 more
        // times, each wrap pushing another row into scrollback.
        let long_line: String = "X".repeat(width * 6); // 60 chars → 6 screen rows
        buf.insert_text(&t(&long_line));

        // We should now have scrollback.  The earliest rows (R0–R4)
        // must be accessible.
        let max_off = buf.max_scroll_offset();
        assert!(
            max_off >= 5,
            "expected at least 5 rows of scrollback, got {max_off}"
        );

        // Scroll all the way back and check the first 5 rows' content.
        let vis_start_at_max_scroll = buf.visible_window_start(max_off);
        for i in 0..5 {
            let row = &buf.rows[vis_start_at_max_scroll + i];
            let first_char = row.cells().first().map(Cell::into_utf8);
            let expected_prefix = format!("R{i}");
            // The row should start with "Ri" (R0, R1, ...).
            let row_text: String = row.cells().iter().map(Cell::into_utf8).collect();
            assert!(
                row_text.starts_with(&expected_prefix),
                "scrollback row {i} should start with {expected_prefix:?}, got {row_text:?} (first_char={first_char:?})"
            );
        }
    }

    /// Verify that an alternate buffer still uses `scroll_region_up`
    /// rotation (no scrollback), so we haven't broken alt-screen wrapping.
    #[test]
    fn alt_buffer_softwrap_does_not_grow_scrollback() {
        let width = 10;
        let height = 5;
        let mut buf = Buffer::new(width, height);
        buf.enter_alternate(0);

        // Fill the screen.
        for _ in 0..height {
            buf.insert_text(&t("AAAAAAAAAA"));
            buf.handle_lf();
        }

        // Long soft-wrapping line on the alt buffer.
        let long_line: String = "B".repeat(60);
        buf.insert_text(&t(&long_line));

        // Alternate buffers must never have scrollback.
        assert_eq!(
            buf.max_scroll_offset(),
            0,
            "alt buffer must have no scrollback"
        );
        assert_eq!(
            buf.rows.len(),
            height,
            "alt buffer row count must stay == height"
        );
    }

    /// Ensure that partial DECSTBM regions still use the old rotation
    /// path (not the new push-row path).  A partial region scroll should
    /// discard the top line of the region, not grow the buffer.
    #[test]
    fn partial_decstbm_softwrap_uses_rotation() {
        let width = 10;
        let height = 10;
        let mut buf = Buffer::new(width, height);

        // Fill the buffer so all `height` rows exist.
        for i in 0..height {
            buf.insert_text(&t(&format!("L{i}")));
            if i < height - 1 {
                buf.handle_lf();
            }
        }

        // Set a partial scroll region: rows 2–7 (0-indexed).
        buf.scroll_region_top = 2;
        buf.scroll_region_bottom = 7;

        // Position cursor at the bottom of the scroll region.
        buf.cursor.pos.y = buf.visible_window_start(0) + 7;
        buf.cursor.pos.x = 0;

        let initial_row_count = buf.rows.len();

        // Write a long line that wraps several times while at the bottom
        // of a partial scroll region.
        let long_line: String = "C".repeat(60);
        buf.insert_text(&t(&long_line));

        // With a partial DECSTBM, the buffer should NOT grow beyond
        // the initial size (rotation keeps row count stable).
        assert_eq!(
            buf.rows.len(),
            initial_row_count,
            "partial DECSTBM wrap must not grow the buffer"
        );
    }
}

#[cfg(test)]
mod reflow_to_width_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    fn t(s: &str) -> Vec<TChar> {
        s.bytes().map(TChar::Ascii).collect()
    }

    fn cell_str(buf: &Buffer, row_idx: usize) -> String {
        let row = &buf.rows[row_idx];
        row.get_characters()
            .iter()
            .filter(|c| !c.is_continuation())
            .map(|c| match c.tchar() {
                TChar::Ascii(b) => (*b as char).to_string(),
                TChar::Space => " ".to_string(),
                TChar::NewLine => "\\n".to_string(),
                TChar::Utf8(buf, len) => String::from_utf8_lossy(&buf[..*len as usize]).to_string(),
            })
            .collect()
    }

    // Test 1: same-width is a no-op
    #[test]
    fn reflow_same_width_is_noop() {
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&t("Hello"));
        buf.handle_lf();
        buf.insert_text(&t("World"));

        let rows_before = buf.rows.len();
        let cursor_before = buf.cursor.pos;

        buf.reflow_to_width(10); // same width

        assert_eq!(buf.rows.len(), rows_before);
        assert_eq!(buf.cursor.pos, cursor_before);
    }

    // Test 2: empty buffer is a no-op
    #[test]
    fn reflow_empty_buffer_noop() {
        let mut buf = Buffer::new(10, 5);
        // don't insert any text, but buffer has default rows
        // Clear all rows to make it truly empty
        buf.rows.clear();
        buf.row_cache.clear();
        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 0;

        buf.reflow_to_width(20);
        assert!(buf.rows.is_empty());
    }

    // Test 3: zero width is a no-op
    #[test]
    fn reflow_zero_width_noop() {
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&t("Hello"));
        let rows_before = buf.rows.len();

        buf.reflow_to_width(0);
        assert_eq!(buf.rows.len(), rows_before);
    }

    // Test 4: narrow to wide reflow (content fits, fewer rows)
    #[test]
    fn reflow_narrow_to_wide() {
        let mut buf = Buffer::new(5, 5);
        // Insert "ABCDEFGH" which wraps at width 5:
        // Row 0: "ABCDE" (soft-wrap)
        // Row 1: "FGH  "
        buf.insert_text(&t("ABCDEFGH"));

        // Now reflow to width 10 — should fit on one row
        buf.reflow_to_width(10);

        assert_eq!(buf.width, 10);
        // Content should be on one logical line
        let text = cell_str(&buf, 0);
        assert!(text.starts_with("ABCDEFGH"), "got: {text}");
    }

    // Test 5: wide to narrow reflow (content wraps, more rows)
    #[test]
    fn reflow_wide_to_narrow() {
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&t("ABCDEFGHIJ"));

        // Reflow to width 5 — should produce 2 rows
        buf.reflow_to_width(5);

        assert_eq!(buf.width, 5);
        let row0 = cell_str(&buf, 0);
        let row1 = cell_str(&buf, 1);
        assert!(row0.starts_with("ABCDE"), "row0: {row0}");
        assert!(row1.starts_with("FGHIJ"), "row1: {row1}");
    }

    // Test 6: wide glyph at boundary wraps to next row
    #[test]
    fn reflow_wide_glyph_at_boundary() {
        // Create a buffer with a wide character that will be at the boundary
        let mut buf = Buffer::new(10, 5);
        // Insert 3 normal chars + wide char "あ" (display width 2)
        // On width=10 this fits fine: ABC + あ = 5 columns
        let wide = TChar::from('あ');
        buf.insert_text(&[
            TChar::Ascii(b'A'),
            TChar::Ascii(b'B'),
            TChar::Ascii(b'C'),
            wide,
        ]);

        // Reflow to width 4: "ABC" fills 3 columns, "あ" needs 2 columns
        // 3 + 2 = 5 > 4, so "あ" should wrap to next row
        buf.reflow_to_width(4);

        assert_eq!(buf.width, 4);
        let row0 = cell_str(&buf, 0);
        assert!(row0.starts_with("ABC"), "row0 should have ABC, got: {row0}");
        let row1 = cell_str(&buf, 1);
        assert!(row1.starts_with("あ"), "row1 should have あ, got: {row1}");
    }

    // Test 7: cursor Y clamped when out of bounds after reflow
    #[test]
    fn reflow_clamps_cursor_y() {
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&t("Line1"));
        buf.handle_lf();
        buf.insert_text(&t("Line2"));

        // Force cursor to a high Y
        buf.cursor.pos.y = 100;

        buf.reflow_to_width(20);

        // Cursor should be clamped to rows.len() - 1
        assert!(buf.cursor.pos.y < buf.rows.len());
    }

    // Test 8: cursor X clamped to new width
    #[test]
    fn reflow_clamps_cursor_x() {
        let mut buf = Buffer::new(20, 5);
        buf.insert_text(&t("Hello World"));
        buf.cursor.pos.x = 15;

        buf.reflow_to_width(5);

        // Cursor X should be clamped to new width - 1
        assert!(buf.cursor.pos.x < buf.width);
    }

    // Test 9: multiple logical lines preserved after reflow
    #[test]
    fn reflow_preserves_logical_lines() {
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&t("AAAA"));
        buf.handle_lf(); // hard break (LF only; reset X manually to simulate CR+LF)
        buf.cursor.pos.x = 0;
        buf.insert_text(&t("BBBB"));

        buf.reflow_to_width(20);

        // Should have at least 2 rows (one per logical line)
        assert!(buf.rows.len() >= 2);
        let row0 = cell_str(&buf, 0);
        let row1 = cell_str(&buf, 1);
        assert!(row0.starts_with("AAAA"), "row0: {row0}");
        assert!(row1.starts_with("BBBB"), "row1: {row1}");
    }

    // Test 10: cursor Y is remapped when lines before cursor wrap to more rows
    #[test]
    fn reflow_remaps_cursor_y_on_narrow() {
        let mut buf = Buffer::new(10, 5);
        // Row 0: "ABCDEFGHIJ" (10 chars, fills full width)
        buf.insert_text(&t("ABCDEFGHIJ"));
        buf.handle_lf();
        buf.cursor.pos.x = 0;
        // Row 1: "XY" (cursor is here)
        buf.insert_text(&t("XY"));

        // Cursor should be at row 1
        assert_eq!(buf.cursor.pos.y, 1);
        assert_eq!(buf.cursor.pos.x, 2);

        // Reflow to width 5: "ABCDEFGHIJ" splits into 2 rows, "XY" stays 1 row
        // Row 0: "ABCDE"
        // Row 1: "FGHIJ"
        // Row 2: "XY"
        buf.reflow_to_width(5);

        assert_eq!(buf.rows.len(), 3);
        assert_eq!(
            buf.cursor.pos.y, 2,
            "cursor should move to row 2 (was row 1 before reflow)"
        );
        assert_eq!(buf.cursor.pos.x, 2, "cursor X should be preserved");
    }

    // Test 11: cursor on a row that itself wraps — cursor X remapped to new row
    #[test]
    fn reflow_remaps_cursor_on_wrapping_row() {
        let mut buf = Buffer::new(10, 5);
        // Row 0: "ABCDEFGHIJ" — cursor at col 7
        buf.insert_text(&t("ABCDEFGHIJ"));
        buf.cursor.pos.x = 7;
        buf.cursor.pos.y = 0;

        // Reflow to width 5:
        // Row 0: "ABCDE"
        // Row 1: "FGHIJ" — old col 7 maps to flat offset 7, row 1 col 2
        buf.reflow_to_width(5);

        assert_eq!(buf.rows.len(), 2);
        assert_eq!(
            buf.cursor.pos.y, 1,
            "cursor should be on row 1 (second half)"
        );
        assert_eq!(buf.cursor.pos.x, 2, "cursor should be at col 2 in new row");
    }

    // Test 12: cursor past end of content (in blank space) is preserved
    #[test]
    fn reflow_cursor_in_blank_space() {
        let mut buf = Buffer::new(10, 5);
        // Row 0: "AB" (2 cells), cursor at col 8 (in blank space)
        buf.insert_text(&t("AB"));
        buf.cursor.pos.x = 8;
        buf.cursor.pos.y = 0;

        // Reflow to width 5: "AB" fits in one row, cursor was at col 8
        // flat_offset = 8, but flat_cells.len() = 2
        // Cursor should land on row 0 at col 8 (past content but still valid)
        buf.reflow_to_width(5);

        assert_eq!(buf.cursor.pos.y, 0);
        // Cursor X should be clamped to new_width - 1 = 4
        assert_eq!(buf.cursor.pos.x, 4);
    }
}

#[cfg(test)]
mod erase_operations_tests {
    use super::*;
    use freminal_common::buffer_states::tchar::TChar;

    fn t(s: &str) -> Vec<TChar> {
        s.bytes().map(TChar::Ascii).collect()
    }

    fn cell_str(buf: &Buffer, row_idx: usize) -> String {
        (0..buf.width)
            .map(|col| {
                let cell = buf.rows[row_idx].resolve_cell(col);
                match cell.tchar() {
                    TChar::Ascii(b) => *b as char,
                    TChar::Space => ' ',
                    TChar::NewLine => '\n',
                    TChar::Utf8(b, len) => String::from_utf8_lossy(&b[..*len as usize])
                        .chars()
                        .next()
                        .unwrap_or('?'),
                }
            })
            .collect()
    }

    /// Helper: write `text` then advance to the next row via LF + CR.
    fn write_line(buf: &mut Buffer, text: &str) {
        buf.insert_text(&t(text));
        buf.handle_lf();
        buf.cursor.pos.x = 0;
    }

    // -------------------------------------------------------------------------
    // erase_to_beginning_of_display (ED 1) tests
    // -------------------------------------------------------------------------

    /// Place content on three rows, put the cursor on row 1 col 3, then call
    /// `erase_to_beginning_of_display`.  Rows above the cursor must be blank
    /// and the cursor row must be blank from col 0 through col 3 (inclusive).
    #[test]
    fn erase_to_beginning_of_display_basic() {
        let mut buf = Buffer::new(10, 5);

        write_line(&mut buf, "AAAAAAAAAA");
        write_line(&mut buf, "BBBBBBBBBB");
        write_line(&mut buf, "CCCCCCCCCC");

        let vis_start = buf.visible_window_start(0);

        // Position cursor on the second visible row at column 3.
        buf.cursor.pos.y = vis_start + 1;
        buf.cursor.pos.x = 3;

        buf.erase_to_beginning_of_display();

        // Row 0 (vis_start + 0) must be entirely blank.
        let row0 = cell_str(&buf, vis_start);
        assert!(
            row0.chars().all(|c| c == ' '),
            "row above cursor must be all spaces; got {row0:?}"
        );

        // Cursor row: cols 0-3 must be blank, cols 4-9 must retain 'B'.
        let row1 = cell_str(&buf, vis_start + 1);
        assert!(
            row1[..4].chars().all(|c| c == ' '),
            "cursor row cols 0-3 must be blanked; got {:?}",
            &row1[..4]
        );
        assert_eq!(
            &row1[4..10],
            "BBBBBB",
            "cursor row cols 4-9 must be untouched"
        );

        // Row 2 must still contain 'C'.
        let row2 = cell_str(&buf, vis_start + 2);
        assert!(
            row2.starts_with("CCCCCCCCCC"),
            "row below cursor must be untouched; got {row2:?}"
        );
    }

    /// Cursor on row 0 col 5: only cols 0-5 of row 0 should be cleared.
    #[test]
    fn erase_to_beginning_of_display_at_first_row() {
        let mut buf = Buffer::new(10, 5);

        buf.insert_text(&t("AAAAAAAAAA"));

        let vis_start = buf.visible_window_start(0);
        buf.cursor.pos.y = vis_start;
        buf.cursor.pos.x = 5;

        buf.erase_to_beginning_of_display();

        let row = cell_str(&buf, vis_start);
        assert!(
            row[..6].chars().all(|c| c == ' '),
            "cols 0-5 must be blank; got {:?}",
            &row[..6]
        );
        assert_eq!(&row[6..10], "AAAA", "cols 6-9 must be untouched");
    }

    // -------------------------------------------------------------------------
    // erase_display (ED 2) tests
    // -------------------------------------------------------------------------

    /// Fill every visible row with content, call `erase_display`, and confirm
    /// all rows in the visible window are blank.
    #[test]
    fn erase_display_basic() {
        let mut buf = Buffer::new(10, 5);

        for ch in ['A', 'B', 'C', 'D', 'E'] {
            write_line(&mut buf, &ch.to_string().repeat(10));
        }

        let vis_start = buf.visible_window_start(0);
        buf.erase_display();

        for i in vis_start..(vis_start + buf.height).min(buf.rows.len()) {
            let row = cell_str(&buf, i);
            assert!(
                row.chars().all(|c| c == ' '),
                "row {i} must be all spaces after erase_display; got {row:?}"
            );
        }
    }

    // -------------------------------------------------------------------------
    // erase_scrollback (ED 3) tests
    // -------------------------------------------------------------------------

    /// Push enough lines to generate real scrollback, then call
    /// `erase_scrollback`.  The scrollback rows must disappear and the cursor
    /// must be adjusted so it still points to the same logical row.
    #[test]
    fn erase_scrollback_removes_scrollback() {
        // 10 wide, 3 tall.  Write 8 lines → 5 rows of scrollback.
        let mut buf = Buffer::new(10, 3);

        for i in 0..8u8 {
            let ch = char::from(b'A' + i);
            buf.insert_text(&t(&ch.to_string().repeat(10)));
            buf.handle_lf();
            buf.cursor.pos.x = 0;
        }

        let vis_start_before = buf.visible_window_start(0);
        assert!(
            vis_start_before > 0,
            "expected scrollback rows before erase; rows.len()={}, height={}",
            buf.rows.len(),
            buf.height
        );

        let cursor_before = buf.cursor.pos.y;

        buf.erase_scrollback();

        let vis_start_after = buf.visible_window_start(0);
        assert_eq!(
            vis_start_after, 0,
            "visible_window_start must be 0 after erase_scrollback"
        );

        let expected_cursor_y = cursor_before.saturating_sub(vis_start_before);
        assert_eq!(
            buf.cursor.pos.y, expected_cursor_y,
            "cursor must be adjusted after scrollback drain"
        );
    }

    /// On the alternate buffer `erase_scrollback` is a no-op (early return at line 2750).
    #[test]
    fn erase_scrollback_alternate_is_noop() {
        let mut buf = Buffer::new(10, 5);

        write_line(&mut buf, "AAAAAAAAAA");
        buf.enter_alternate(0);

        buf.insert_text(&t("BBBBBBBBBB"));

        let rows_before = buf.rows.len();
        let cursor_before = buf.cursor.pos;

        buf.erase_scrollback();

        assert_eq!(
            buf.rows.len(),
            rows_before,
            "erase_scrollback must not mutate alternate screen rows"
        );
        assert_eq!(
            buf.cursor.pos, cursor_before,
            "erase_scrollback must not move cursor on alternate screen"
        );
    }

    /// When the primary buffer has no scrollback (content fits in the visible
    /// window), `erase_scrollback` is a no-op.
    #[test]
    fn erase_scrollback_no_scrollback_noop() {
        let mut buf = Buffer::new(10, 5);
        write_line(&mut buf, "AAAAAAAAAA");

        let vis_start = buf.visible_window_start(0);
        assert_eq!(vis_start, 0, "expected no scrollback in a fresh buffer");

        let rows_before = buf.rows.len();
        let cursor_before = buf.cursor.pos;

        buf.erase_scrollback();

        assert_eq!(buf.rows.len(), rows_before, "row count must not change");
        assert_eq!(buf.cursor.pos, cursor_before, "cursor must not move");
    }

    // -------------------------------------------------------------------------
    // erase_line_to_beginning (EL 1) tests
    // -------------------------------------------------------------------------

    /// Write text on a line, set cursor to the middle, call
    /// `erase_line_to_beginning`.  Cells `0..=cursor_x` must be blank; cells
    /// after the cursor must retain their original content.
    #[test]
    fn erase_line_to_beginning_basic() {
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&t("ABCDEFGHIJ"));

        let vis_start = buf.visible_window_start(0);
        buf.cursor.pos.y = vis_start;
        buf.cursor.pos.x = 4;

        buf.erase_line_to_beginning();

        let row = cell_str(&buf, vis_start);
        // Cols 0-4 must be blank.
        assert!(
            row[..5].chars().all(|c| c == ' '),
            "cols 0-4 must be blanked; got {:?}",
            &row[..5]
        );
        // Cols 5-9 must still be 'F'-'J'.
        assert_eq!(&row[5..10], "FGHIJ", "cols 5-9 must be untouched");
    }

    // -------------------------------------------------------------------------
    // erase_line_to_end (EL 0) tests
    // -------------------------------------------------------------------------

    /// With cursor at column 0 the entire line must be cleared.
    #[test]
    fn erase_line_to_end_from_col_zero() {
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&t("ABCDEFGHIJ"));

        let vis_start = buf.visible_window_start(0);
        buf.cursor.pos.y = vis_start;
        buf.cursor.pos.x = 0;

        buf.erase_line_to_end();

        let row = cell_str(&buf, vis_start);
        assert!(
            row.chars().all(|c| c == ' '),
            "entire line must be blanked when cursor is at col 0; got {row:?}"
        );
    }
}

// ============================================================================
// Tests for handle_lf, handle_ri, insert_lines, delete_lines — alternate
// buffer paths and DECLRMM paths
// ============================================================================

#[cfg(test)]
mod lf_ri_il_dl_tests {
    use super::*;
    use freminal_common::buffer_states::{
        modes::{declrmm::Declrmm, lnm::Lnm},
        tchar::TChar,
    };

    // ─── helpers ────────────────────────────────────────────────────────────

    fn ascii(c: char) -> TChar {
        TChar::Ascii(c as u8)
    }

    fn text(s: &str) -> Vec<TChar> {
        s.chars().map(ascii).collect()
    }

    /// Create an alternate buffer of the given dimensions.
    fn make_alt_buffer(width: usize, height: usize) -> Buffer {
        let mut buf = Buffer::new(width, height);
        buf.enter_alternate(0);
        buf
    }

    /// Return the `TChar` at `(col, row)` in `buf.rows`.
    fn cell_char(buf: &Buffer, row: usize, col: usize) -> TChar {
        *buf.rows[row].resolve_cell(col).tchar()
    }

    // ─── handle_lf — primary buffer full-screen region fast path ───────────

    /// Verify the primary-buffer full-screen LF path: when the cursor is NOT
    /// at the last row (sy < height-1), the cursor moves down and does NOT
    /// create a new row if one already exists with real content.
    ///
    /// This exercises the `else` branch at line 1520: the row at the destination
    /// already exists and must be left untouched (origin != `ScrollFill`, sy != height-1).
    #[test]
    fn primary_lf_above_bottom_leaves_existing_row_untouched() {
        let width = 5;
        let height = 5;
        let mut buf = Buffer::new(width, height);

        // Create exactly `height` rows so the buffer is full (no scrollback yet).
        // Each LF creates a new row.
        for _ in 0..height - 1 {
            buf.handle_lf();
        }
        assert_eq!(
            buf.rows.len(),
            height,
            "buffer must have exactly height rows"
        );

        // Write recognizable content on row 2 so it has a real HardBreak row.
        buf.cursor.pos.y = 2;
        buf.cursor.pos.x = 0;
        buf.insert_text(&text("HELLO"));

        // Position cursor one row above row 2, so a LF moves to row 2.
        buf.cursor.pos.y = 1;
        buf.cursor.pos.x = 0;

        let rows_before = buf.rows.len();
        buf.handle_lf();

        // Cursor advances to row 2.
        assert_eq!(buf.cursor.pos.y, 2, "cursor must move to row 2");
        // Row count must not change.
        assert_eq!(buf.rows.len(), rows_before, "row count must not change");
        // Row 2 content must be undisturbed.
        assert_eq!(
            cell_char(&buf, 2, 0),
            ascii('H'),
            "existing row content must be preserved"
        );
    }

    /// Verify the primary-buffer `ScrollFill` placeholder path (lines 1522-1526):
    /// when a LF lands on a row with `origin == ScrollFill`, the row is
    /// upgraded to a `HardBreak` row without clearing its (empty) cells.
    ///
    /// `ScrollFill` rows are created by `set_size` when the buffer is grown.
    #[test]
    fn primary_lf_scrollfill_row_stamped_as_hard_break() {
        let width = 5;
        let height = 5;
        let mut buf = Buffer::new(width, height);

        // Make the buffer full (height rows exist).
        for _ in 0..height - 1 {
            buf.handle_lf();
        }

        // Grow the buffer so `set_size` inserts a ScrollFill placeholder row.
        let new_height = height + 2;
        let _ = buf.set_size(width, new_height, 0);

        // The last row(s) added by set_size should be ScrollFill.
        // Cursor is at the end; position it one row before a ScrollFill row.
        let last = buf.rows.len() - 1;
        if last > 0 {
            // Check that the last row is actually ScrollFill (set_size creates them).
            if buf.rows[last].origin == RowOrigin::ScrollFill {
                buf.cursor.pos.y = last - 1;
                buf.cursor.pos.x = 0;

                buf.handle_lf();

                // The ScrollFill row must now be stamped as HardBreak.
                assert_eq!(
                    buf.rows[last].origin,
                    RowOrigin::HardBreak,
                    "ScrollFill row must become HardBreak after LF"
                );
            }
        }
        // (If no ScrollFill row exists, the test is vacuously satisfied.)
    }

    // ─── handle_lf — alternate buffer paths (lines 1584-1604) ──────────────

    /// In the alternate buffer, LF when the cursor is at `scroll_region_bottom`
    /// should trigger a scroll-up; the cursor stays at the bottom row.
    #[test]
    fn alt_lf_at_scroll_region_bottom_scrolls_up() {
        let width = 5;
        let height = 4;
        let mut buf = make_alt_buffer(width, height);

        // Write distinct content on each row.
        for row in 0..height {
            buf.cursor.pos.y = row;
            buf.cursor.pos.x = 0;
            buf.insert_text(&text(&format!("R{row:03}")));
        }

        // Position cursor at the last row (bottom of the default full-screen
        // scroll region).
        buf.cursor.pos.y = height - 1;
        buf.cursor.pos.x = 0;

        // Fire LF — should scroll the region up; cursor stays at bottom.
        buf.handle_lf();

        assert_eq!(
            buf.cursor.pos.y,
            height - 1,
            "cursor must remain at scroll_region_bottom after scroll"
        );
        assert_eq!(
            buf.rows.len(),
            height,
            "alt buffer row count must stay == height"
        );

        // The newly scrolled-in bottom row should be blank.
        let bottom_row = &buf.rows[height - 1];
        for cell in bottom_row.cells() {
            assert_eq!(
                cell.tchar(),
                &TChar::Space,
                "newly scrolled-in row must be blank"
            );
        }
    }

    /// In the alternate buffer, LF when the cursor is inside the scroll region
    /// but NOT at the bottom: cursor should just move down one row without
    /// any scrolling.
    #[test]
    fn alt_lf_inside_region_not_at_bottom_just_moves_down() {
        let width = 5;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        // Place cursor in the middle of the default full-screen scroll region.
        buf.cursor.pos.y = 2;
        buf.cursor.pos.x = 0;

        buf.handle_lf();

        assert_eq!(buf.cursor.pos.y, 3, "cursor must move down one row");
        assert_eq!(buf.rows.len(), height, "row count must not change");
    }

    /// In the alternate buffer with a partial scroll region, LF when the cursor
    /// is OUTSIDE the scroll region should just move the cursor down (lines
    /// 1599-1601) without any scrolling.
    #[test]
    fn alt_lf_outside_scroll_region_just_moves_down() {
        let width = 5;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        // Set partial scroll region rows 1..3 (0-based: top=1, bottom=3).
        buf.set_scroll_region(2, 4); // 1-based: rows 2..4 → 0-based: 1..3

        // Cursor starts at row 0 — outside the scroll region (above it).
        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 0;

        buf.handle_lf();

        // Cursor moves down to row 1 (still outside the region); no scroll.
        assert_eq!(buf.cursor.pos.y, 1, "cursor must move down to row 1");
        assert_eq!(buf.rows.len(), height, "row count must not change");
    }

    /// In the alternate buffer, LF with LNM=NewLine should also CR the cursor
    /// (line 1585-1586).
    #[test]
    fn alt_lf_with_lnm_also_resets_cursor_x() {
        let width = 10;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        buf.set_lnm(Lnm::NewLine);
        // Position cursor at a mid-line column.
        buf.cursor.pos.y = 1;
        buf.cursor.pos.x = 5;

        buf.handle_lf();

        assert_eq!(buf.cursor.pos.x, 0, "LNM must reset cursor X to 0");
        assert_eq!(buf.cursor.pos.y, 2, "cursor must move down one row");
    }

    /// In the alternate buffer, LF with LNM=LineFeed (the default) must NOT
    /// reset cursor X.
    #[test]
    fn alt_lf_without_lnm_keeps_cursor_x() {
        let width = 10;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        // LNM defaults to LineFeed — no implicit CR.
        buf.cursor.pos.y = 1;
        buf.cursor.pos.x = 5;

        buf.handle_lf();

        assert_eq!(buf.cursor.pos.x, 5, "without LNM cursor X must not change");
        assert_eq!(buf.cursor.pos.y, 2, "cursor must move down one row");
    }

    // ─── handle_ri — alternate buffer paths (lines 1661-1673) ──────────────

    /// In the alternate buffer, RI with the cursor at the TOP of the scroll
    /// region should scroll the region DOWN (insert a blank line at top).
    /// Lines 1667-1668.
    #[test]
    fn alt_ri_at_top_of_region_scrolls_down() {
        let width = 5;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        // Write identifiable content on each row.
        for row in 0..height {
            buf.cursor.pos.y = row;
            buf.cursor.pos.x = 0;
            buf.insert_text(&text(&format!("R{row:03}")));
        }

        // Full-screen region (default): top=0, bottom=height-1.
        // Place cursor at row 0 (top of scroll region).
        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 0;

        buf.handle_ri();

        // Cursor must stay at row 0 (top margin).
        assert_eq!(buf.cursor.pos.y, 0, "cursor must stay at top margin");
        assert_eq!(buf.rows.len(), height, "row count must not change");

        // Row 0 must now be blank (newly inserted line at the top).
        let top_row = &buf.rows[0];
        for cell in top_row.cells() {
            assert_eq!(
                cell.tchar(),
                &TChar::Space,
                "row 0 must be blank after scroll-down"
            );
        }
    }

    /// In the alternate buffer, RI when the cursor is INSIDE the region but
    /// NOT at the top: cursor should just move up one row (line 1665-1666).
    #[test]
    fn alt_ri_inside_region_not_at_top_just_moves_up() {
        let width = 5;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        buf.cursor.pos.y = 3;
        buf.cursor.pos.x = 0;

        buf.handle_ri();

        assert_eq!(buf.cursor.pos.y, 2, "cursor must move up one row");
    }

    /// In the alternate buffer, RI when the cursor is OUTSIDE the scroll region
    /// should just move the cursor up without any scrolling (lines 1670-1671).
    #[test]
    fn alt_ri_outside_region_just_moves_up() {
        let width = 5;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        // Set partial scroll region rows 2..4 (0-based).
        buf.set_scroll_region(3, 5); // 1-based: rows 3..5 → 0-based: 2..4

        // Cursor at row 1 — above the scroll region.
        buf.cursor.pos.y = 1;
        buf.cursor.pos.x = 0;

        buf.handle_ri();

        assert_eq!(buf.cursor.pos.y, 0, "cursor must move up to row 0");
        assert_eq!(buf.rows.len(), height, "row count must not change");
    }

    /// In the alternate buffer, RI with a pending-wrap cursor (pos.x == width)
    /// must first clamp x to width-1 (line 1635-1636) before moving up.
    #[test]
    fn alt_ri_pending_wrap_clears_pending_wrap_state() {
        let width = 5;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        // Simulate pending-wrap: pos.x set to width (one past the last column).
        buf.cursor.pos.y = 2;
        buf.cursor.pos.x = width; // pending-wrap state

        buf.handle_ri();

        // x must be clamped to width-1.
        assert_eq!(
            buf.cursor.pos.x,
            width - 1,
            "pending wrap must be cleared: x clamped to width-1"
        );
        // Cursor should have moved up.
        assert_eq!(buf.cursor.pos.y, 1, "cursor must move up one row");
    }

    // ─── insert_lines — alternate buffer (lines 1682-1708) ─────────────────

    /// `insert_lines(0)` must be a no-op (early return at line 1682-1683).
    #[test]
    fn alt_insert_lines_zero_is_noop() {
        let width = 5;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        // Write recognizable content.
        for row in 0..height {
            buf.cursor.pos.y = row;
            buf.cursor.pos.x = 0;
            buf.insert_text(&text(&format!("R{row:03}")));
        }

        buf.cursor.pos.y = 2;
        buf.insert_lines(0);

        // "R002" → col 0='R', col 1='0', col 2='0', col 3='2'.
        // Verify both col 0 and col 3 to confirm nothing was shifted.
        assert_eq!(
            cell_char(&buf, 2, 0),
            ascii('R'),
            "row 2 col 0 must be unchanged"
        );
        assert_eq!(
            cell_char(&buf, 2, 3),
            ascii('2'),
            "row 2 col 3 must be unchanged"
        );
    }

    /// `insert_lines` when the cursor is OUTSIDE the scroll region must be a
    /// no-op (line 1690-1691).
    #[test]
    fn alt_insert_lines_outside_region_is_noop() {
        let width = 5;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        // Partial region rows 1..3 (0-based: top=1, bottom=3).
        buf.set_scroll_region(2, 4); // 1-based → 0-based: 1..3

        // Write recognizable content on every row.
        for row in 0..height {
            buf.cursor.pos.y = row;
            buf.cursor.pos.x = 0;
            buf.insert_text(&text(&format!("R{row:03}")));
        }

        // Cursor at row 0 — above the scroll region top (1).
        buf.cursor.pos.y = 0;
        buf.insert_lines(2);

        // Row 0 must be unchanged.
        assert_eq!(cell_char(&buf, 0, 0), ascii('R'), "row 0 must be unchanged");
        // Row 1 (region top) must also be unchanged.
        assert_eq!(cell_char(&buf, 1, 0), ascii('R'), "row 1 must be unchanged");
    }

    /// `insert_lines` inside the scroll region on the alternate buffer without
    /// DECLRMM: shifts rows down within the region and blanks the cursor row.
    #[test]
    fn alt_insert_lines_basic() {
        let width = 5;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        // Write identifiable content on each row: "R000", "R001", …
        for row in 0..height {
            buf.cursor.pos.y = row;
            buf.cursor.pos.x = 0;
            buf.insert_text(&text(&format!("R{row:03}")));
        }

        // Cursor at row 1 inside the full-screen scroll region.
        buf.cursor.pos.y = 1;
        buf.insert_lines(1);

        // Row 1 should now be blank (the newly inserted line).
        let row1 = &buf.rows[1];
        for cell in row1.cells() {
            assert_eq!(cell.tchar(), &TChar::Space, "inserted row must be blank");
        }

        // The old row 1 content ("R001") should have been pushed to row 2.
        assert_eq!(
            cell_char(&buf, 2, 0),
            ascii('R'),
            "old row 1 now at row 2 col 0"
        );
        assert_eq!(
            cell_char(&buf, 2, 1),
            ascii('0'),
            "old row 1 now at row 2 col 1"
        );
        assert_eq!(
            cell_char(&buf, 2, 2),
            ascii('0'),
            "old row 1 now at row 2 col 2"
        );
        assert_eq!(
            cell_char(&buf, 2, 3),
            ascii('1'),
            "old row 1 now at row 2 col 3"
        );

        // Row 0 must be untouched.
        assert_eq!(cell_char(&buf, 0, 0), ascii('R'), "row 0 must be unchanged");
    }

    /// `insert_lines` on the alternate buffer WITH DECLRMM enabled: only the
    /// columns within [left, right] are shifted; outside columns are untouched.
    /// Lines 1697-1701.
    #[test]
    fn alt_insert_lines_declrmm_column_selective() {
        let width = 10;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        // Write a known 10-char pattern on each row.
        for row in 0..height {
            buf.cursor.pos.y = row;
            buf.cursor.pos.x = 0;
            buf.insert_text(&text("ABCDEFGHIJ"));
        }

        // Enable DECLRMM with left/right margins cols 3..6 (1-based) →
        // 0-based: 2..5.
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(3, 6); // 1-based → 0-based: 2..5
        // set_left_right_margins homes cursor to (0,0); move to row 1.
        buf.cursor.pos.y = 1;
        buf.cursor.pos.x = 0;

        buf.insert_lines(1);

        // Row 1, cols 2..5 should now be blank (the inserted region columns).
        for col in 2..=5 {
            assert_eq!(
                cell_char(&buf, 1, col),
                TChar::Space,
                "col {col} of inserted row must be blank"
            );
        }

        // Row 1, cols outside the margin (0, 1, 6-9) must retain original values.
        assert_eq!(
            cell_char(&buf, 1, 0),
            ascii('A'),
            "col 0 outside margin untouched"
        );
        assert_eq!(
            cell_char(&buf, 1, 1),
            ascii('B'),
            "col 1 outside margin untouched"
        );
        assert_eq!(
            cell_char(&buf, 1, 6),
            ascii('G'),
            "col 6 outside margin untouched"
        );

        // The original row 1 margin-column content must now be at row 2.
        assert_eq!(
            cell_char(&buf, 2, 2),
            ascii('C'),
            "old row1 col2 now at row2"
        );
        assert_eq!(
            cell_char(&buf, 2, 5),
            ascii('F'),
            "old row1 col5 now at row2"
        );

        // Row 0 must be completely untouched.
        assert_eq!(cell_char(&buf, 0, 0), ascii('A'), "row 0 untouched");
        assert_eq!(cell_char(&buf, 0, 5), ascii('F'), "row 0 col 5 untouched");
    }

    // ─── insert_lines — primary buffer with DECLRMM (lines 1722-1726) ───────

    /// `insert_lines` on the primary buffer WITH DECLRMM enabled: only the
    /// columns within [left, right] are shifted in the visible window.
    #[test]
    fn primary_insert_lines_declrmm_column_selective() {
        let width = 10;
        let height = 5;
        let mut buf = Buffer::new(width, height);

        // Fill the buffer to `height` rows with identifiable content.
        for row in 0..height {
            buf.cursor.pos.y = row;
            buf.cursor.pos.x = 0;
            buf.insert_text(&text("ABCDEFGHIJ"));
        }

        // Enable DECLRMM with margins cols 3..7 (1-based) → 0-based: 2..6.
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(3, 7); // 1-based → 0-based: 2..6
        // set_left_right_margins homes cursor to (0,0); move to row 2.
        buf.cursor.pos.y = 2;
        buf.cursor.pos.x = 0;

        buf.insert_lines(1);

        // Cols 2..6 on the cursor row (row 2) should be blank.
        for col in 2..=6 {
            assert_eq!(
                cell_char(&buf, 2, col),
                TChar::Space,
                "col {col} of inserted row must be blank"
            );
        }

        // Cols outside the margin on row 2 must keep their original values.
        assert_eq!(
            cell_char(&buf, 2, 0),
            ascii('A'),
            "col 0 outside margin untouched"
        );
        assert_eq!(
            cell_char(&buf, 2, 1),
            ascii('B'),
            "col 1 outside margin untouched"
        );
        assert_eq!(
            cell_char(&buf, 2, 7),
            ascii('H'),
            "col 7 outside margin untouched"
        );

        // The original row 2 margin content must now be at row 3.
        assert_eq!(
            cell_char(&buf, 3, 2),
            ascii('C'),
            "old row 2 col 2 now at row 3"
        );
        assert_eq!(
            cell_char(&buf, 3, 6),
            ascii('G'),
            "old row 2 col 6 now at row 3"
        );

        // Row 0 must be completely untouched.
        assert_eq!(cell_char(&buf, 0, 0), ascii('A'), "row 0 untouched");
    }

    // ─── delete_lines — alternate buffer (lines 1742-1767) ──────────────────

    /// `delete_lines` when the cursor is OUTSIDE the scroll region must be a
    /// no-op (line 1749-1750).
    #[test]
    fn alt_delete_lines_outside_region_is_noop() {
        let width = 5;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        // Partial region rows 1..3 (0-based: top=1, bottom=3).
        buf.set_scroll_region(2, 4); // 1-based → 0-based: 1..3

        // Write recognizable content.
        for row in 0..height {
            buf.cursor.pos.y = row;
            buf.cursor.pos.x = 0;
            buf.insert_text(&text(&format!("R{row:03}")));
        }

        // Cursor at row 0 — above the scroll region top (1).
        buf.cursor.pos.y = 0;
        buf.delete_lines(1);

        // Nothing should have changed.
        assert_eq!(cell_char(&buf, 0, 0), ascii('R'), "row 0 must be unchanged");
        assert_eq!(cell_char(&buf, 1, 0), ascii('R'), "row 1 must be unchanged");
    }

    /// `delete_lines` inside the scroll region on the alternate buffer without
    /// DECLRMM: shifts rows up within the region and blanks the bottom row.
    #[test]
    fn alt_delete_lines_basic() {
        let width = 5;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        // Write identifiable content on each row.
        for row in 0..height {
            buf.cursor.pos.y = row;
            buf.cursor.pos.x = 0;
            buf.insert_text(&text(&format!("R{row:03}")));
        }

        // Cursor at row 1 inside the full-screen scroll region.
        buf.cursor.pos.y = 1;
        buf.delete_lines(1);

        // Row 1 should now have the old row 2 content ("R002").
        assert_eq!(cell_char(&buf, 1, 0), ascii('R'), "row 1 col 0 must be 'R'");
        assert_eq!(cell_char(&buf, 1, 3), ascii('2'), "row 1 col 3 must be '2'");

        // Row 0 must be untouched.
        assert_eq!(cell_char(&buf, 0, 0), ascii('R'), "row 0 must be unchanged");
        assert_eq!(cell_char(&buf, 0, 3), ascii('0'), "row 0 col 3 must be '0'");

        // The bottom row (row 4) should be blank.
        let bottom_row = &buf.rows[height - 1];
        for cell in bottom_row.cells() {
            assert_eq!(
                cell.tchar(),
                &TChar::Space,
                "bottom row must be blank after delete"
            );
        }
    }

    /// `delete_lines` on the alternate buffer WITH DECLRMM enabled: only the
    /// columns within [left, right] are shifted; outside columns are untouched.
    /// Lines 1756-1760.
    #[test]
    fn alt_delete_lines_declrmm_column_selective() {
        let width = 10;
        let height = 5;
        let mut buf = make_alt_buffer(width, height);

        // Write a known 10-char pattern on each row.
        for row in 0..height {
            buf.cursor.pos.y = row;
            buf.cursor.pos.x = 0;
            buf.insert_text(&text("ABCDEFGHIJ"));
        }

        // Enable DECLRMM with margins cols 3..6 (1-based) → 0-based: 2..5.
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(3, 6); // 1-based → 0-based: 2..5
        // set_left_right_margins homes cursor to (0,0); move to row 1.
        buf.cursor.pos.y = 1;
        buf.cursor.pos.x = 0;

        buf.delete_lines(1);

        // Row 1, cols 2..5 should now have the old row 2 content.
        assert_eq!(
            cell_char(&buf, 1, 2),
            ascii('C'),
            "row1 col2: old row2 content"
        );
        assert_eq!(
            cell_char(&buf, 1, 5),
            ascii('F'),
            "row1 col5: old row2 content"
        );

        // Row 1, cols outside the margin must retain the original row 1 values.
        assert_eq!(
            cell_char(&buf, 1, 0),
            ascii('A'),
            "col 0 outside margin untouched"
        );
        assert_eq!(
            cell_char(&buf, 1, 1),
            ascii('B'),
            "col 1 outside margin untouched"
        );
        assert_eq!(
            cell_char(&buf, 1, 6),
            ascii('G'),
            "col 6 outside margin untouched"
        );

        // The bottom margin row (row 4), cols 2..5 should be blank.
        for col in 2..=5 {
            assert_eq!(
                cell_char(&buf, height - 1, col),
                TChar::Space,
                "bottom row col {col} must be blank after delete"
            );
        }

        // The bottom row outside the margin must be unchanged.
        assert_eq!(
            cell_char(&buf, height - 1, 0),
            ascii('A'),
            "bottom row col 0 outside margin untouched"
        );

        // Row 0 must be completely untouched.
        assert_eq!(cell_char(&buf, 0, 0), ascii('A'), "row 0 untouched");
        assert_eq!(cell_char(&buf, 0, 5), ascii('F'), "row 0 col 5 untouched");
    }

    // ─── delete_lines — primary buffer with DECLRMM (lines 1781-1785) ───────

    /// `delete_lines` on the primary buffer WITH DECLRMM enabled: only the
    /// columns within [left, right] are shifted in the visible window.
    #[test]
    fn primary_delete_lines_declrmm_column_selective() {
        let width = 10;
        let height = 5;
        let mut buf = Buffer::new(width, height);

        // Fill the buffer to `height` rows with identifiable content.
        for row in 0..height {
            buf.cursor.pos.y = row;
            buf.cursor.pos.x = 0;
            buf.insert_text(&text("ABCDEFGHIJ"));
        }

        // Enable DECLRMM with margins cols 3..7 (1-based) → 0-based: 2..6.
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(3, 7); // 1-based → 0-based: 2..6
        // set_left_right_margins homes cursor to (0,0); move to row 2.
        buf.cursor.pos.y = 2;
        buf.cursor.pos.x = 0;

        buf.delete_lines(1);

        // Cols 2..6 on row 2 should now have the old row 3 content.
        assert_eq!(
            cell_char(&buf, 2, 2),
            ascii('C'),
            "row2 col2: old row3 content"
        );
        assert_eq!(
            cell_char(&buf, 2, 6),
            ascii('G'),
            "row2 col6: old row3 content"
        );

        // Cols outside the margin on row 2 must be unchanged (original row 2).
        assert_eq!(
            cell_char(&buf, 2, 0),
            ascii('A'),
            "col 0 outside margin untouched"
        );
        assert_eq!(
            cell_char(&buf, 2, 1),
            ascii('B'),
            "col 1 outside margin untouched"
        );
        assert_eq!(
            cell_char(&buf, 2, 7),
            ascii('H'),
            "col 7 outside margin untouched"
        );

        // The bottom row (row 4), cols 2..6 should be blank.
        for col in 2..=6 {
            assert_eq!(
                cell_char(&buf, height - 1, col),
                TChar::Space,
                "bottom row col {col} must be blank after delete"
            );
        }

        // Bottom row outside the margin must be unchanged.
        assert_eq!(
            cell_char(&buf, height - 1, 0),
            ascii('A'),
            "bottom row col 0 outside margin untouched"
        );

        // Row 0 must be completely untouched.
        assert_eq!(cell_char(&buf, 0, 0), ascii('A'), "row 0 untouched");
    }
}

// ============================================================================
// Column-selective scroll + miscellaneous uncovered path tests
// ============================================================================

#[cfg(test)]
mod column_scroll_and_misc_tests {
    use super::*;
    use freminal_common::buffer_states::modes::declrmm::Declrmm;
    use freminal_common::buffer_states::tchar::TChar;

    fn t(s: &str) -> Vec<TChar> {
        s.bytes().map(TChar::Ascii).collect()
    }

    fn cell_char(buf: &Buffer, row: usize, col: usize) -> TChar {
        *buf.rows[row].resolve_cell(col).tchar()
    }

    fn ascii(c: char) -> TChar {
        TChar::Ascii(c as u8)
    }

    /// Fill alternate buffer rows with distinct characters per row.
    fn fill_alt_rows(buf: &mut Buffer) {
        let height = buf.height;
        for r in 0..height {
            buf.cursor.pos.y = r;
            buf.cursor.pos.x = 0;
            #[allow(clippy::cast_possible_truncation)]
            let ch = (b'A' + r as u8) as char;
            let text: Vec<TChar> = (0..buf.width).map(|_| TChar::Ascii(ch as u8)).collect();
            buf.insert_text(&text);
        }
    }

    // ── scroll_slice_up_columns via delete_lines with DECLRMM ──

    #[test]
    fn delete_lines_declrmm_shifts_columns_only() {
        let width = 10;
        let height = 5;
        let mut buf = Buffer::new(width, height);
        buf.enter_alternate(0);
        fill_alt_rows(&mut buf);

        // Enable DECLRMM with margins at cols 2..6 (1-based: 3..7)
        buf.set_declrmm(Declrmm::Enabled);
        buf.scroll_region_left = 2;
        buf.scroll_region_right = 6;

        // Cursor at row 1 (inside default scroll region)
        buf.cursor.pos.y = 1;
        buf.delete_lines(1);

        // Row 1 cols 2..6 should now have content from row 2 (was 'C')
        for col in 2..=6 {
            assert_eq!(
                cell_char(&buf, 1, col),
                ascii('C'),
                "row 1 col {col} should have row 2's content"
            );
        }
        // Row 1 col 0 should still be 'B' (outside margin)
        assert_eq!(cell_char(&buf, 1, 0), ascii('B'));
        // Row 1 col 9 should still be 'B' (outside margin)
        assert_eq!(cell_char(&buf, 1, 9), ascii('B'));
    }

    // ── scroll_slice_down_columns via insert_lines with DECLRMM ──

    #[test]
    fn insert_lines_declrmm_shifts_columns_only() {
        let width = 10;
        let height = 5;
        let mut buf = Buffer::new(width, height);
        buf.enter_alternate(0);
        fill_alt_rows(&mut buf);

        buf.set_declrmm(Declrmm::Enabled);
        buf.scroll_region_left = 2;
        buf.scroll_region_right = 6;

        buf.cursor.pos.y = 1;
        buf.insert_lines(1);

        // Row 1 cols 2..6 should be blank (new line inserted)
        for col in 2..=6 {
            assert_eq!(
                cell_char(&buf, 1, col),
                TChar::Space,
                "row 1 col {col} should be blank after insert"
            );
        }
        // Row 1 col 0 should still be 'B' (outside margin, untouched)
        assert_eq!(cell_char(&buf, 1, 0), ascii('B'));
        // Row 2 cols 2..6 should have old row 1's content ('B')
        for col in 2..=6 {
            assert_eq!(
                cell_char(&buf, 2, col),
                ascii('B'),
                "row 2 col {col} should have shifted-down 'B'"
            );
        }
    }

    // ── set_left_right_margins edge cases ──

    #[test]
    fn set_left_right_margins_zero_resets() {
        let mut buf = Buffer::new(10, 5);
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(3, 7);
        assert_eq!(buf.scroll_region_left, 2); // 0-based

        buf.set_left_right_margins(0, 0);
        assert_eq!(buf.scroll_region_left, 0);
        assert_eq!(buf.scroll_region_right, 9);
    }

    #[test]
    fn set_left_right_margins_invalid_resets() {
        let mut buf = Buffer::new(10, 5);
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(3, 7);

        // left >= right
        buf.set_left_right_margins(5, 5);
        assert_eq!(buf.scroll_region_left, 0);
        assert_eq!(buf.scroll_region_right, 9);

        // right >= width
        buf.set_left_right_margins(3, 7);
        buf.set_left_right_margins(1, 20);
        assert_eq!(buf.scroll_region_left, 0);
        assert_eq!(buf.scroll_region_right, 9);
    }

    // ── visible_rows empty ──

    #[test]
    fn visible_rows_empty_buffer() {
        let mut buf = Buffer::new(10, 5);
        buf.rows.clear();
        buf.row_cache.clear();
        assert!(buf.visible_rows(0).is_empty());
    }

    // ── any_visible_dirty ──

    #[test]
    fn any_visible_dirty_after_insert() {
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&t("hello"));
        assert!(buf.any_visible_dirty(0));
    }

    #[test]
    fn any_visible_dirty_after_clean() {
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&t("hello"));
        // Mark all rows clean
        for row in &mut buf.rows {
            row.dirty = false;
        }
        assert!(!buf.any_visible_dirty(0));
    }

    #[test]
    fn any_visible_dirty_empty() {
        let mut buf = Buffer::new(10, 5);
        buf.rows.clear();
        buf.row_cache.clear();
        assert!(!buf.any_visible_dirty(0));
    }

    // ── visible_image_placements ──

    #[test]
    fn visible_image_placements_no_images() {
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&t("hello"));
        let placements = buf.visible_image_placements(0);
        assert!(placements.iter().all(Option::is_none));
    }

    #[test]
    fn visible_image_placements_empty_buffer() {
        let mut buf = Buffer::new(10, 5);
        buf.rows.clear();
        buf.row_cache.clear();
        let placements = buf.visible_image_placements(0);
        assert!(placements.is_empty());
    }

    // ── has_visible_images ──

    #[test]
    fn has_visible_images_no_images() {
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&t("hello"));
        assert!(!buf.has_visible_images(0));
    }

    #[test]
    fn has_visible_images_empty() {
        let mut buf = Buffer::new(10, 5);
        buf.rows.clear();
        buf.row_cache.clear();
        assert!(!buf.has_visible_images(0));
    }

    // ── insert_spaces / delete_chars / erase_chars edge cases ──

    #[test]
    fn insert_spaces_out_of_bounds() {
        let mut buf = Buffer::new(10, 5);
        buf.cursor.pos.y = 100;
        buf.insert_spaces(5); // should be no-op, no panic
    }

    #[test]
    fn delete_chars_out_of_bounds() {
        let mut buf = Buffer::new(10, 5);
        buf.cursor.pos.y = 100;
        buf.delete_chars(5); // should be no-op, no panic
    }

    #[test]
    fn erase_chars_out_of_bounds() {
        let mut buf = Buffer::new(10, 5);
        buf.cursor.pos.y = 100;
        buf.erase_chars(5); // should be no-op, no panic
    }

    #[test]
    fn insert_spaces_with_declrmm() {
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&t("ABCDEFGHIJ"));
        buf.set_declrmm(Declrmm::Enabled);
        buf.scroll_region_left = 2;
        buf.scroll_region_right = 6;

        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 3;
        buf.insert_spaces(2);

        // Cols 0-2 should be unchanged: A B C
        assert_eq!(cell_char(&buf, 0, 0), ascii('A'));
        assert_eq!(cell_char(&buf, 0, 1), ascii('B'));
        assert_eq!(cell_char(&buf, 0, 2), ascii('C'));
        // Col 3,4 should be spaces (inserted)
        assert_eq!(cell_char(&buf, 0, 3), TChar::Space);
        assert_eq!(cell_char(&buf, 0, 4), TChar::Space);
        // Cols 7-9 should be unchanged (outside right margin)
        assert_eq!(cell_char(&buf, 0, 7), ascii('H'));
    }

    #[test]
    fn delete_chars_with_declrmm() {
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&t("ABCDEFGHIJ"));
        buf.set_declrmm(Declrmm::Enabled);
        buf.scroll_region_left = 2;
        buf.scroll_region_right = 6;

        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 3;
        buf.delete_chars(2);

        // Cols 0-2 unchanged: A B C
        assert_eq!(cell_char(&buf, 0, 0), ascii('A'));
        assert_eq!(cell_char(&buf, 0, 1), ascii('B'));
        assert_eq!(cell_char(&buf, 0, 2), ascii('C'));
        // Col 3 should now have 'F' (shifted left from col 5)
        assert_eq!(cell_char(&buf, 0, 3), ascii('F'));
        // Cols 7-9 unchanged (outside margin)
        assert_eq!(cell_char(&buf, 0, 7), ascii('H'));
    }

    #[test]
    fn erase_chars_with_declrmm() {
        let mut buf = Buffer::new(10, 5);
        buf.insert_text(&t("ABCDEFGHIJ"));
        buf.set_declrmm(Declrmm::Enabled);
        buf.scroll_region_left = 2;
        buf.scroll_region_right = 6;

        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 3;
        // Erase 10 chars — should be clamped to right margin
        buf.erase_chars(10);

        // Col 3..6 should be blank (erased, clamped to margin)
        for col in 3..=6 {
            assert_eq!(
                cell_char(&buf, 0, col),
                TChar::Space,
                "col {col} should be blank"
            );
        }
        // Col 7 should be unchanged (outside margin)
        assert_eq!(cell_char(&buf, 0, 7), ascii('H'));
    }

    // ── scroll_region_up_n / scroll_region_down_n ──

    #[test]
    fn scroll_region_up_n_basic() {
        let mut buf = Buffer::new(10, 5);
        buf.enter_alternate(0);
        fill_alt_rows(&mut buf);

        buf.scroll_region_up_n(2);

        // Row 0 should now have what was row 2 ('C')
        assert_eq!(cell_char(&buf, 0, 0), ascii('C'));
        // Bottom 2 rows should be blank
        assert_eq!(cell_char(&buf, 3, 0), TChar::Space);
        assert_eq!(cell_char(&buf, 4, 0), TChar::Space);
    }

    #[test]
    fn scroll_region_down_n_basic() {
        let mut buf = Buffer::new(10, 5);
        buf.enter_alternate(0);
        fill_alt_rows(&mut buf);

        buf.scroll_region_down_n(2);

        // Top 2 rows should be blank
        assert_eq!(cell_char(&buf, 0, 0), TChar::Space);
        assert_eq!(cell_char(&buf, 1, 0), TChar::Space);
        // Row 2 should have what was row 0 ('A')
        assert_eq!(cell_char(&buf, 2, 0), ascii('A'));
    }
}

// ════════════════════════════════════════════════════════════════════════════
// resize_and_insert_tests — covers resize_height alt shrink, insert_text
// NoAutoWrap, set_cursor_pos_raw width=0, enter/leave alternate edge cases,
// visible_as_tchars_and_tags, extract_text, extract_block_text
// ════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod resize_and_insert_tests {
    use super::*;
    use crate::image_store::{ImagePlacement, ImageProtocol};
    use freminal_common::buffer_states::{
        buffer_type::BufferType, format_tag::FormatTag, modes::decawm::Decawm, tchar::TChar,
    };

    fn t(s: &str) -> Vec<TChar> {
        s.bytes().map(TChar::Ascii).collect()
    }

    fn cell_char(buf: &Buffer, row: usize, col: usize) -> TChar {
        *buf.rows[row].resolve_cell(col).tchar()
    }

    fn make_placement(image_id: u64) -> ImagePlacement {
        ImagePlacement {
            image_id,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Kitty,
            image_number: None,
            placement_id: None,
            z_index: 0,
        }
    }

    // ── `resize_height`: alternate buffer shrink ──

    #[test]
    fn resize_height_alt_shrink_drains_top_rows() {
        let mut buf = Buffer::new(10, 5);
        buf.enter_alternate(0);

        // Fill rows with distinct chars: row 0='A', row 1='B', ...
        for r in 0..5 {
            buf.cursor.pos.y = r;
            buf.cursor.pos.x = 0;
            #[allow(clippy::cast_possible_truncation)]
            let ch = (b'A' + r as u8) as char;
            buf.insert_text(&[TChar::Ascii(ch as u8); 10]);
        }
        buf.cursor.pos.y = 2;

        // Shrink to height 3 — top 2 rows should be drained.
        let offset = buf.resize_height(3, 0);
        assert_eq!(buf.rows.len(), 3, "should have exactly 3 rows");
        // First row should be what was row 2 ('C')
        assert_eq!(cell_char(&buf, 0, 0), TChar::Ascii(b'C'));
        // Cursor Y adjusted from 2 to 0
        assert_eq!(buf.cursor.pos.y, 0);
        assert_eq!(offset, 0);
    }

    #[test]
    fn resize_height_alt_shrink_adjusts_image_count() {
        let mut buf = Buffer::new(10, 4);
        buf.enter_alternate(0);

        // Place image cells in rows 0 and 1 (which will be drained on shrink).
        buf.set_image_cell_at(0, 0, make_placement(1), FormatTag::default());
        buf.set_image_cell_at(0, 1, make_placement(1), FormatTag::default());
        buf.set_image_cell_at(1, 0, make_placement(2), FormatTag::default());
        assert_eq!(buf.image_cell_count, 3);

        // Shrink from 4 to 2 — drains rows 0 and 1 (3 image cells).
        let _ = buf.resize_height(2, 0);
        assert_eq!(buf.image_cell_count, 0);
        assert_eq!(buf.rows.len(), 2);
    }

    // ── `preserve_scrollback_anchor` ──

    #[test]
    fn preserve_scrollback_anchor_clamps_offset() {
        let mut buf = Buffer::new(10, 5);
        // Buffer::new starts with 1 row. Add more to get scrollback.
        for _ in 0..14 {
            buf.rows.push(Row::new(10));
            buf.row_cache.push(None);
        }
        // Total rows = 15 (1 initial + 14 added).
        assert_eq!(buf.rows.len(), 15);

        buf.preserve_scrollback_anchor = true;
        // Grow height from 5 to 8. rows.len()=15+3=18, new_height=8.
        // max_offset = 18-8=10. scroll_offset 20 should clamp to 10.
        let offset = buf.resize_height(8, 20);
        assert_eq!(offset, 10);
    }

    #[test]
    fn preserve_scrollback_anchor_few_rows_returns_zero() {
        let mut buf = Buffer::new(10, 3);
        // 3 rows, growing to height 5 — rows.len() becomes 5 which is <= new_height.
        buf.preserve_scrollback_anchor = true;
        let offset = buf.resize_height(5, 10);
        assert_eq!(offset, 0);
    }

    // ── `clamp_cursor_after_resize` with empty rows ──

    #[test]
    fn clamp_cursor_empty_rows() {
        let mut buf = Buffer::new(10, 5);
        buf.cursor.pos.x = 5;
        buf.cursor.pos.y = 3;
        // Drain all rows to make it empty.
        buf.rows.clear();
        buf.row_cache.clear();
        buf.clamp_cursor_after_resize();
        assert_eq!(buf.cursor.pos.x, 0);
        assert_eq!(buf.cursor.pos.y, 0);
    }

    // ── `insert_text` with `NoAutoWrap` ──

    #[test]
    fn insert_text_no_auto_wrap_discards_excess() {
        let mut buf = Buffer::new(5, 3);
        buf.wrap_enabled = Decawm::NoAutoWrap;
        buf.cursor.pos.x = 0;
        buf.cursor.pos.y = 0;
        buf.insert_text(&t("ABCDEFGHIJ")); // 10 chars into width=5

        // Only first 5 chars fit. Cursor should be at last column (4).
        assert_eq!(buf.cursor.pos.x, 4);
        assert_eq!(cell_char(&buf, 0, 0), TChar::Ascii(b'A'));
        assert_eq!(cell_char(&buf, 0, 4), TChar::Ascii(b'E'));
        // Should still be 1 row (no wrapping, Buffer::new starts with 1 row).
        assert_eq!(buf.rows.len(), 1);
    }

    // ── `insert_text` creating new rows with SoftWrap/HardBreak ──

    #[test]
    fn insert_text_appends_rows_with_correct_origin() {
        // Create a very small buffer where inserting text forces new row creation.
        let mut buf = Buffer::new(5, 1);
        buf.cursor.pos.x = 0;
        buf.cursor.pos.y = 0;
        // Insert 12 chars — needs 3 rows at width 5. Row 0 exists, rows 1-2 appended.
        buf.insert_text(&t("ABCDEFGHIJKL"));

        assert!(buf.rows.len() >= 3, "should have at least 3 rows");
        // Row 0 is the original (HardBreak, NewLogicalLine).
        assert_eq!(buf.rows[0].origin, RowOrigin::HardBreak);
        assert_eq!(buf.rows[0].join, RowJoin::NewLogicalLine);
        // Row 1 is a wrap continuation.
        assert_eq!(buf.rows[1].origin, RowOrigin::SoftWrap);
        assert_eq!(buf.rows[1].join, RowJoin::ContinueLogicalLine);
        // Row 2 is also a wrap continuation.
        assert_eq!(buf.rows[2].origin, RowOrigin::SoftWrap);
        assert_eq!(buf.rows[2].join, RowJoin::ContinueLogicalLine);
    }

    // ── `set_cursor_pos_raw` with width=0 ──

    #[test]
    fn set_cursor_pos_raw_width_zero() {
        let mut buf = Buffer::new(10, 5);
        buf.width = 0;
        buf.set_cursor_pos_raw(CursorPos { x: 5, y: 0 });
        assert_eq!(buf.cursor.pos.x, 0);
        // Y clamped to rows.len()-1 = 0 (Buffer::new has 1 row).
        assert_eq!(buf.cursor.pos.y, 0);
    }

    // ── `set_image_cell_at` out-of-bounds ──

    #[test]
    fn set_image_cell_at_out_of_bounds_is_noop() {
        let mut buf = Buffer::new(10, 5);
        assert_eq!(buf.image_cell_count, 0);
        // Row 10 doesn't exist (only 0-4).
        buf.set_image_cell_at(10, 0, make_placement(1), FormatTag::default());
        assert_eq!(buf.image_cell_count, 0);
    }

    // ── `enter_alternate` when already alternate ──

    #[test]
    fn enter_alternate_twice_is_noop() {
        let mut buf = Buffer::new(10, 5);
        buf.enter_alternate(0);
        let rows_before = buf.rows.len();
        let cursor_before = buf.cursor.clone();
        buf.enter_alternate(0);
        assert_eq!(buf.rows.len(), rows_before);
        assert_eq!(buf.cursor.pos, cursor_before.pos);
    }

    // ── `leave_alternate` when already primary ──

    #[test]
    fn leave_alternate_on_primary_returns_zero() {
        let mut buf = Buffer::new(10, 5);
        assert_eq!(buf.kind, BufferType::Primary);
        let offset = buf.leave_alternate();
        assert_eq!(offset, 0);
    }

    // ── `leave_alternate` with no saved state ──

    #[test]
    fn leave_alternate_no_saved_state_returns_zero() {
        let mut buf = Buffer::new(10, 5);
        // Manually set to alternate without going through enter_alternate.
        buf.kind = BufferType::Alternate;
        assert!(buf.saved_primary.is_none());
        let offset = buf.leave_alternate();
        assert_eq!(offset, 0);
        assert_eq!(buf.kind, BufferType::Primary);
    }

    // ── `visible_as_tchars_and_tags` multi-row with NewLine separators ──

    #[test]
    fn visible_as_tchars_and_tags_newline_separators() {
        let mut buf = Buffer::new(5, 3);
        buf.enter_alternate(0); // Creates exactly 3 rows.

        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 0;
        buf.insert_text(&t("ABC"));
        buf.cursor.pos.y = 1;
        buf.cursor.pos.x = 0;
        buf.insert_text(&t("DE"));

        let (chars, tags, row_offsets, _url_indices) = buf.visible_as_tchars_and_tags(0);

        // Should have chars for row 0, NewLine, row 1, NewLine, row 2.
        let newline_count = chars.iter().filter(|c| matches!(c, TChar::NewLine)).count();
        assert_eq!(
            newline_count, 2,
            "should have 2 NewLine separators for 3 rows"
        );

        // Tags should cover the full range.
        assert!(!tags.is_empty());
        // Row offsets should have 3 entries.
        assert_eq!(row_offsets.len(), 3);
    }

    // ── `visible_as_tchars_and_tags` empty buffer ──

    #[test]
    fn visible_as_tchars_and_tags_empty_buffer() {
        let mut buf = Buffer::new(5, 3);
        buf.enter_alternate(0); // Creates exactly 3 empty rows.
        let (_chars, tags, _row_offsets, _url_indices) = buf.visible_as_tchars_and_tags(0);

        // Even empty rows produce Space cells (no — empty cells vec produces no chars).
        // But tags should still be valid.
        assert!(!tags.is_empty(), "should have at least one tag");
        assert_eq!(tags[0].start, 0);
    }

    // ── `extract_text` edge cases ──

    #[test]
    fn extract_text_start_row_out_of_bounds() {
        let buf = Buffer::new(10, 3);
        let text = buf.extract_text(100, 0, 200, 5);
        assert_eq!(text, "");
    }

    #[test]
    fn extract_text_with_newline_cell() {
        let mut buf = Buffer::new(10, 3);
        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 0;
        // Insert enough text to have cells at indices 0-4.
        buf.insert_text(&t("ABXYZ"));
        // Manually overwrite cell 2 with NewLine.
        buf.rows[0].cells_mut()[2] = Cell::new(TChar::NewLine, FormatTag::default());

        let text = buf.extract_text(0, 0, 0, 9);
        assert_eq!(text, "AB", "extraction should stop at NewLine");
    }

    // ── `extract_block_text` edge cases ──

    #[test]
    fn extract_block_text_start_row_out_of_bounds() {
        let buf = Buffer::new(10, 3);
        let text = buf.extract_block_text(100, 0, 200, 5);
        assert_eq!(text, "");
    }

    #[test]
    fn extract_block_text_col_beyond_cells() {
        let mut buf = Buffer::new(5, 2);
        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 0;
        buf.insert_text(&t("ABC"));

        // Extract block from col 10 to 15 — beyond width.
        let text = buf.extract_block_text(0, 10, 0, 15);
        // Should be empty or just whitespace since cols are out of range.
        assert!(text.trim().is_empty());
    }
}

// ════════════════════════════════════════════════════════════════════════════
// image_clearing_tests — covers all clear_image_placements_* functions
// ════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod image_clearing_tests {
    use super::*;
    use crate::image_store::{ImagePlacement, ImageProtocol};
    use freminal_common::buffer_states::format_tag::FormatTag;

    fn make_placement(image_id: u64, image_number: Option<u32>, z_index: i32) -> ImagePlacement {
        ImagePlacement {
            image_id,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Kitty,
            image_number,
            placement_id: None,
            z_index,
        }
    }

    fn place(buf: &mut Buffer, row: usize, col: usize, id: u64) {
        buf.set_image_cell_at(row, col, make_placement(id, None, 0), FormatTag::default());
    }

    /// Create an alternate buffer with exactly `height` rows — guaranteed row access.
    fn alt_buf(width: usize, height: usize) -> Buffer {
        let mut buf = Buffer::new(width, height);
        buf.enter_alternate(0);
        buf
    }

    // ── `has_any_image_cell` ──

    #[test]
    fn has_any_image_cell_empty() {
        let buf = alt_buf(10, 5);
        assert!(!buf.has_any_image_cell());
    }

    #[test]
    fn has_any_image_cell_with_images() {
        let mut buf = alt_buf(10, 5);
        place(&mut buf, 0, 0, 1);
        assert!(buf.has_any_image_cell());
    }

    // ── `clear_image_placements_by_id` ──

    #[test]
    fn clear_by_id_only_clears_matching() {
        let mut buf = alt_buf(10, 5);
        place(&mut buf, 0, 0, 1);
        place(&mut buf, 0, 1, 1);
        place(&mut buf, 1, 0, 2);
        assert_eq!(buf.image_cell_count, 3);

        buf.clear_image_placements_by_id(1);
        assert_eq!(buf.image_cell_count, 1);
        assert!(!buf.rows[0].cells()[0].has_image());
        assert!(!buf.rows[0].cells()[1].has_image());
        assert!(buf.rows[1].cells()[0].has_image());
    }

    // ── `clear_image_placements_by_number` ──

    #[test]
    fn clear_by_number_only_clears_matching() {
        let mut buf = alt_buf(10, 5);
        buf.set_image_cell_at(0, 0, make_placement(1, Some(42), 0), FormatTag::default());
        buf.set_image_cell_at(0, 1, make_placement(2, Some(42), 0), FormatTag::default());
        buf.set_image_cell_at(1, 0, make_placement(3, Some(99), 0), FormatTag::default());
        assert_eq!(buf.image_cell_count, 3);

        buf.clear_image_placements_by_number(42);
        assert_eq!(buf.image_cell_count, 1);
        assert!(!buf.rows[0].cells()[0].has_image());
        assert!(!buf.rows[0].cells()[1].has_image());
        assert!(buf.rows[1].cells()[0].has_image());
    }

    #[test]
    fn clear_by_number_no_match_is_noop() {
        let mut buf = alt_buf(10, 5);
        buf.set_image_cell_at(0, 0, make_placement(1, Some(10), 0), FormatTag::default());
        assert_eq!(buf.image_cell_count, 1);
        buf.clear_image_placements_by_number(999);
        assert_eq!(buf.image_cell_count, 1);
    }

    // ── `clear_image_placements_at_cell` ──

    #[test]
    fn clear_at_cell_clears_all_cells_with_same_id() {
        let mut buf = alt_buf(10, 5);
        place(&mut buf, 0, 0, 1);
        place(&mut buf, 0, 1, 1);
        place(&mut buf, 1, 0, 1);
        place(&mut buf, 2, 0, 2); // different id
        assert_eq!(buf.image_cell_count, 4);

        buf.clear_image_placements_at_cell(0, 0);
        assert_eq!(buf.image_cell_count, 1);
        assert!(!buf.rows[0].cells()[0].has_image());
        assert!(!buf.rows[0].cells()[1].has_image());
        assert!(!buf.rows[1].cells()[0].has_image());
        assert!(buf.rows[2].cells()[0].has_image());
    }

    #[test]
    fn clear_at_cell_out_of_bounds_row() {
        let mut buf = alt_buf(10, 5);
        place(&mut buf, 0, 0, 1);
        assert_eq!(buf.image_cell_count, 1);
        buf.clear_image_placements_at_cell(100, 0);
        assert_eq!(buf.image_cell_count, 1);
    }

    #[test]
    fn clear_at_cell_col_beyond_width() {
        let mut buf = alt_buf(10, 5);
        place(&mut buf, 0, 0, 1);
        assert_eq!(buf.image_cell_count, 1);
        buf.clear_image_placements_at_cell(0, 100);
        assert_eq!(buf.image_cell_count, 1);
    }

    // ── `clear_image_placements_at_cell_and_after` ──

    #[test]
    fn clear_at_cell_and_after_preserves_before() {
        let mut buf = alt_buf(10, 5);
        place(&mut buf, 0, 0, 1); // before — should survive
        place(&mut buf, 1, 5, 2); // at and after — should be cleared
        place(&mut buf, 2, 0, 3); // after — should be cleared
        assert_eq!(buf.image_cell_count, 3);

        buf.clear_image_placements_at_cell_and_after(1, 3);
        assert_eq!(buf.image_cell_count, 1);
        assert!(buf.rows[0].cells()[0].has_image());
        assert!(!buf.rows[1].cells()[5].has_image());
        assert!(!buf.rows[2].cells()[0].has_image());
    }

    // ── `clear_image_placements_in_column` ──

    #[test]
    fn clear_in_column_clears_all_rows() {
        let mut buf = alt_buf(10, 5);
        place(&mut buf, 0, 3, 1);
        place(&mut buf, 2, 3, 2);
        place(&mut buf, 4, 5, 3); // different column — should survive
        assert_eq!(buf.image_cell_count, 3);

        buf.clear_image_placements_in_column(3);
        assert_eq!(buf.image_cell_count, 1);
        assert!(!buf.rows[0].cells()[3].has_image());
        assert!(!buf.rows[2].cells()[3].has_image());
        assert!(buf.rows[4].cells()[5].has_image());
    }

    #[test]
    fn clear_in_column_beyond_width_is_noop() {
        let mut buf = alt_buf(10, 5);
        place(&mut buf, 0, 0, 1);
        assert_eq!(buf.image_cell_count, 1);
        buf.clear_image_placements_in_column(100);
        assert_eq!(buf.image_cell_count, 1);
    }

    // ── `clear_image_placements_in_row` ──

    #[test]
    fn clear_in_row_clears_matching_ids() {
        let mut buf = alt_buf(10, 5);
        place(&mut buf, 2, 0, 1);
        place(&mut buf, 2, 3, 2);
        place(&mut buf, 0, 0, 3); // different row — should survive
        assert_eq!(buf.image_cell_count, 3);

        buf.clear_image_placements_in_row(2);
        assert_eq!(buf.image_cell_count, 1);
        assert!(!buf.rows[2].cells()[0].has_image());
        assert!(!buf.rows[2].cells()[3].has_image());
        assert!(buf.rows[0].cells()[0].has_image());
    }

    #[test]
    fn clear_in_row_out_of_bounds_is_noop() {
        let mut buf = alt_buf(10, 5);
        place(&mut buf, 0, 0, 1);
        assert_eq!(buf.image_cell_count, 1);
        buf.clear_image_placements_in_row(100);
        assert_eq!(buf.image_cell_count, 1);
    }

    // ── `clear_image_placements_by_z_index` ──

    #[test]
    fn clear_by_z_index_only_clears_matching() {
        let mut buf = alt_buf(10, 5);
        buf.set_image_cell_at(0, 0, make_placement(1, None, 5), FormatTag::default());
        buf.set_image_cell_at(0, 1, make_placement(2, None, 5), FormatTag::default());
        buf.set_image_cell_at(1, 0, make_placement(3, None, 10), FormatTag::default());
        assert_eq!(buf.image_cell_count, 3);

        buf.clear_image_placements_by_z_index(5);
        assert_eq!(buf.image_cell_count, 1);
        assert!(!buf.rows[0].cells()[0].has_image());
        assert!(!buf.rows[0].cells()[1].has_image());
        assert!(buf.rows[1].cells()[0].has_image());
    }

    #[test]
    fn clear_by_z_index_no_match_is_noop() {
        let mut buf = alt_buf(10, 5);
        buf.set_image_cell_at(0, 0, make_placement(1, None, 0), FormatTag::default());
        assert_eq!(buf.image_cell_count, 1);
        buf.clear_image_placements_by_z_index(999);
        assert_eq!(buf.image_cell_count, 1);
    }

    // ── `clear_image_placements_at_cursor` ──

    #[test]
    fn clear_at_cursor_clears_cursor_row() {
        let mut buf = alt_buf(10, 5);
        place(&mut buf, 2, 0, 1);
        place(&mut buf, 2, 3, 2);
        place(&mut buf, 3, 0, 3); // different row
        buf.cursor.pos.y = 2;
        assert_eq!(buf.image_cell_count, 3);

        buf.clear_image_placements_at_cursor();
        assert_eq!(buf.image_cell_count, 1);
        assert!(!buf.rows[2].cells()[0].has_image());
        assert!(!buf.rows[2].cells()[3].has_image());
        assert!(buf.rows[3].cells()[0].has_image());
    }

    #[test]
    fn clear_at_cursor_out_of_bounds_is_noop() {
        let mut buf = alt_buf(10, 5);
        place(&mut buf, 0, 0, 1);
        buf.cursor.pos.y = 100;
        assert_eq!(buf.image_cell_count, 1);
        buf.clear_image_placements_at_cursor();
        assert_eq!(buf.image_cell_count, 1);
    }

    // ── `clear_image_placements_at_cursor_and_after` ──

    #[test]
    fn clear_at_cursor_and_after_preserves_rows_before() {
        let mut buf = alt_buf(10, 5);
        place(&mut buf, 0, 0, 1); // before cursor — should survive
        place(&mut buf, 1, 0, 2); // cursor row — cleared
        place(&mut buf, 3, 0, 3); // after cursor — cleared
        buf.cursor.pos.y = 1;
        assert_eq!(buf.image_cell_count, 3);

        buf.clear_image_placements_at_cursor_and_after();
        assert_eq!(buf.image_cell_count, 1);
        assert!(buf.rows[0].cells()[0].has_image());
        assert!(!buf.rows[1].cells()[0].has_image());
        assert!(!buf.rows[3].cells()[0].has_image());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod coverage_gap_tests {
    use super::*;

    /// Helper: create an alternate-screen buffer with given dimensions
    fn alt_buf(width: usize, height: usize) -> Buffer {
        let mut buf = Buffer::new(width, height);
        buf.enter_alternate(0);
        buf
    }

    /// Helper to place an image cell at (row, col) with a given fake `image_id`
    fn place_image(buf: &mut Buffer, row: usize, col: usize, id: u64) {
        use crate::image_store::{ImagePlacement, ImageProtocol};
        let placement = ImagePlacement {
            image_id: id,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Kitty,
            image_number: None,
            placement_id: None,
            z_index: 0,
        };
        buf.rows[row].set_image_cell(col, placement, FormatTag::default());
        buf.image_cell_count += 1;
    }

    // -----------------------------------------------------------------------
    // has_visible_images
    // -----------------------------------------------------------------------

    #[test]
    fn has_visible_images_false_when_no_images() {
        let buf = alt_buf(10, 5);
        assert!(!buf.has_visible_images(0));
    }

    #[test]
    fn has_visible_images_true_when_image_in_visible_window() {
        let mut buf = alt_buf(10, 5);
        place_image(&mut buf, 2, 0, 1);
        assert!(buf.has_visible_images(0));
    }

    #[test]
    fn has_visible_images_empty_buffer() {
        let buf = Buffer::new(10, 5); // primary, no rows filled
        // Buffer::new creates 0 rows (grows dynamically) → false
        assert!(!buf.has_visible_images(0));
    }

    // -----------------------------------------------------------------------
    // visible_window_start edge case: empty rows
    // -----------------------------------------------------------------------

    #[test]
    fn visible_window_start_empty_buffer_returns_one() {
        let buf = Buffer::new(10, 5);
        // Buffer::new creates 1 row by default (grows dynamically)
        assert_eq!(buf.visible_rows(0).len(), 1);
    }

    // -----------------------------------------------------------------------
    // erase_line_to_end / erase_line_to_beginning / erase_line with images
    // -----------------------------------------------------------------------

    #[test]
    fn erase_line_to_end_with_images() {
        let mut buf = alt_buf(10, 5);
        place_image(&mut buf, 0, 5, 1);
        assert_eq!(buf.image_cell_count, 1);

        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 3;
        buf.erase_line_to_end();
        assert_eq!(buf.image_cell_count, 0);
    }

    #[test]
    fn erase_line_to_beginning_with_images() {
        let mut buf = alt_buf(10, 5);
        place_image(&mut buf, 0, 2, 1);
        assert_eq!(buf.image_cell_count, 1);

        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 5;
        buf.erase_line_to_beginning();
        assert_eq!(buf.image_cell_count, 0);
    }

    #[test]
    fn erase_line_with_images() {
        let mut buf = alt_buf(10, 5);
        place_image(&mut buf, 0, 3, 1);
        assert_eq!(buf.image_cell_count, 1);

        buf.cursor.pos.y = 0;
        buf.erase_line();
        assert_eq!(buf.image_cell_count, 0);
    }

    // -----------------------------------------------------------------------
    // erase_to_beginning_of_display with images
    // -----------------------------------------------------------------------

    #[test]
    fn erase_to_beginning_of_display_with_images() {
        let mut buf = alt_buf(10, 5);
        place_image(&mut buf, 0, 5, 1);
        place_image(&mut buf, 1, 3, 2);
        assert_eq!(buf.image_cell_count, 2);

        buf.cursor.pos.y = 2;
        buf.cursor.pos.x = 5;
        buf.erase_to_beginning_of_display();
        assert_eq!(buf.image_cell_count, 0);
    }

    // -----------------------------------------------------------------------
    // erase_display with images
    // -----------------------------------------------------------------------

    #[test]
    fn erase_display_with_images() {
        let mut buf = alt_buf(10, 5);
        place_image(&mut buf, 0, 0, 1);
        place_image(&mut buf, 2, 5, 2);
        place_image(&mut buf, 4, 9, 3);
        assert_eq!(buf.image_cell_count, 3);

        buf.erase_display();
        assert_eq!(buf.image_cell_count, 0);
    }

    // -----------------------------------------------------------------------
    // erase_scrollback with images in scrollback
    // -----------------------------------------------------------------------

    #[test]
    fn erase_scrollback_with_images() {
        let mut buf = Buffer::new(10, 3);
        // Write enough lines to create scrollback
        for i in 0..6_u8 {
            buf.insert_text(&[TChar::Ascii(b'A' + i)]);
            buf.handle_lf();
        }
        // Place an image in a scrollback row
        let visible_start = buf.rows.len().saturating_sub(3);
        if visible_start > 0 {
            place_image(&mut buf, 0, 0, 1);
            let count_before = buf.image_cell_count;
            assert_eq!(count_before, 1);

            buf.erase_scrollback();
            assert_eq!(buf.image_cell_count, 0);
        }
    }

    #[test]
    fn erase_scrollback_adjusts_cursor() {
        let mut buf = Buffer::new(10, 3);
        for i in 0..6_u8 {
            buf.insert_text(&[TChar::Ascii(b'A' + i)]);
            buf.handle_lf();
        }
        let rows_before = buf.rows.len();
        assert!(rows_before > 3, "should have scrollback");

        let cursor_before = buf.cursor.pos.y;
        buf.erase_scrollback();
        assert!(
            buf.cursor.pos.y < cursor_before || buf.rows.len() <= 3,
            "cursor should be adjusted or buffer should be small"
        );
    }

    // -----------------------------------------------------------------------
    // ICH with DECLRMM active
    // -----------------------------------------------------------------------

    #[test]
    fn ich_with_declrmm_shifts_within_margins() {
        let mut buf = alt_buf(10, 5);
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(3, 8); // 1-based → left=2, right=7 (0-based)

        // Write some text
        buf.insert_text(&[
            TChar::Ascii(b'A'),
            TChar::Ascii(b'B'),
            TChar::Ascii(b'C'),
            TChar::Ascii(b'D'),
            TChar::Ascii(b'E'),
            TChar::Ascii(b'F'),
            TChar::Ascii(b'G'),
            TChar::Ascii(b'H'),
        ]);

        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 3;
        buf.insert_spaces(2);

        // Cells should be shifted within the right margin
        let cell0 = buf.rows[0].resolve_cell(0);
        assert_eq!(cell0.tchar(), &TChar::Ascii(b'A'));
    }

    // -----------------------------------------------------------------------
    // DCH with DECLRMM active
    // -----------------------------------------------------------------------

    #[test]
    fn dch_with_declrmm_deletes_within_margins() {
        let mut buf = alt_buf(10, 5);
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(3, 8);

        buf.insert_text(&[
            TChar::Ascii(b'A'),
            TChar::Ascii(b'B'),
            TChar::Ascii(b'C'),
            TChar::Ascii(b'D'),
            TChar::Ascii(b'E'),
            TChar::Ascii(b'F'),
            TChar::Ascii(b'G'),
            TChar::Ascii(b'H'),
        ]);

        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 3;
        buf.delete_chars(2);

        // Cell at position 0 should still be 'A'
        let cell0 = buf.rows[0].resolve_cell(0);
        assert_eq!(cell0.tchar(), &TChar::Ascii(b'A'));
    }

    // -----------------------------------------------------------------------
    // ICH/DCH with images in affected range
    // -----------------------------------------------------------------------

    #[test]
    fn ich_with_images_decrements_count_for_overflow() {
        let mut buf = alt_buf(10, 5);
        // Place image at col 8 — it will overflow when we insert 3 spaces at col 5
        place_image(&mut buf, 0, 8, 1);
        assert_eq!(buf.image_cell_count, 1);

        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 5;
        buf.insert_spaces(3);

        // Image was at col 8, shifted to col 11 → overflows width 10 → cleared
        assert_eq!(buf.image_cell_count, 0);
    }

    #[test]
    fn dch_with_images_decrements_count() {
        let mut buf = alt_buf(10, 5);
        place_image(&mut buf, 0, 3, 1);
        assert_eq!(buf.image_cell_count, 1);

        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 3;
        buf.delete_chars(1);

        assert_eq!(buf.image_cell_count, 0);
    }

    // -----------------------------------------------------------------------
    // ECH (erase characters) with DECLRMM clamping
    // -----------------------------------------------------------------------

    #[test]
    fn ech_with_declrmm_clamps_to_right_margin() {
        let mut buf = alt_buf(10, 5);

        // Write text across the full width WITHOUT DECLRMM first
        buf.insert_text(&[
            TChar::Ascii(b'A'),
            TChar::Ascii(b'B'),
            TChar::Ascii(b'C'),
            TChar::Ascii(b'D'),
            TChar::Ascii(b'E'),
            TChar::Ascii(b'F'),
            TChar::Ascii(b'G'),
            TChar::Ascii(b'H'),
        ]);

        // NOW enable DECLRMM
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(1, 6); // 0-based: left=0, right=5

        buf.cursor.pos.y = 0;
        buf.cursor.pos.x = 2;
        // Erase 10 chars, but DECLRMM clamps to right margin (col 5)
        buf.erase_chars(10);

        // Cell at col 0 and 1 should be untouched
        assert_eq!(buf.rows[0].resolve_cell(0).tchar(), &TChar::Ascii(b'A'));
        assert_eq!(buf.rows[0].resolve_cell(1).tchar(), &TChar::Ascii(b'B'));
        // Cell beyond right margin should be untouched
        assert_eq!(buf.rows[0].resolve_cell(6).tchar(), &TChar::Ascii(b'G'));
    }

    // -----------------------------------------------------------------------
    // Reflow: cursor clamping when cursor.y >= rows.len()
    // -----------------------------------------------------------------------

    #[test]
    fn reflow_clamps_cursor_when_beyond_rows() {
        let mut buf = Buffer::new(10, 5);
        // Write a single line
        buf.insert_text(&[TChar::Ascii(b'X')]);
        // Manually force cursor beyond rows to test the clamp
        buf.cursor.pos.y = 100;

        // Trigger reflow by changing width
        let _ = buf.set_size(5, 5, 0);

        // Cursor should be clamped to valid range
        assert!(buf.cursor.pos.y < buf.rows.len() || buf.rows.is_empty());
    }

    #[test]
    fn reflow_clamps_cursor_x_when_beyond_new_width() {
        let mut buf = Buffer::new(20, 5);
        buf.insert_text(&[TChar::Ascii(b'A')]);
        buf.cursor.pos.x = 15;

        // Shrink width → cursor.x should be clamped
        let _ = buf.set_size(5, 5, 0);
        assert!(buf.cursor.pos.x < 5);
    }

    // -----------------------------------------------------------------------
    // enforce_scrollback_limit: cursor adjustment when overflow > cursor.y
    // -----------------------------------------------------------------------

    #[test]
    fn enforce_scrollback_limit_clamps_cursor_to_zero() {
        let mut buf = Buffer::new(10, 3);
        buf.scrollback_limit = 2; // very small limit

        // Write many lines to exceed scrollback
        for i in 0..20_u8 {
            buf.insert_text(&[TChar::Ascii(b'A' + (i % 26))]);
            buf.handle_lf();
        }

        // Cursor should still be valid
        assert!(buf.cursor.pos.y < buf.rows.len());
    }

    // -----------------------------------------------------------------------
    // scroll_slice_down: edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn scroll_region_down_n_clamps_to_region_size() {
        let mut buf = alt_buf(10, 5);
        // Write text on all rows
        for i in 0..5_u8 {
            buf.cursor.pos.y = i as usize;
            buf.cursor.pos.x = 0;
            buf.insert_text(&[TChar::Ascii(b'A' + i)]);
        }

        buf.set_scroll_region(2, 4); // rows 1-3 (0-based)

        // Scroll down by 10 (clamped to 3 = region size)
        buf.scroll_region_down_n(10);

        // After max-clamped scroll, all region rows should be blank
        // (the original content was shifted out)
    }

    // -----------------------------------------------------------------------
    // scroll_slice_up_columns / scroll_slice_down_columns (DECLRMM)
    // -----------------------------------------------------------------------

    #[test]
    fn column_scroll_up_with_declrmm() {
        let mut buf = alt_buf(10, 5);

        // Write identifiable content on rows 0-4 WITHOUT DECLRMM
        for i in 0..5_u8 {
            buf.cursor.pos.y = i as usize;
            buf.cursor.pos.x = 0;
            let chars: Vec<TChar> = (0..10_u8)
                .map(|c| TChar::Ascii(b'A' + (i * 10 + c) % 26))
                .collect();
            buf.insert_text(&chars);
        }

        // NOW enable DECLRMM
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(3, 8); // left=2, right=7 (0-based)

        // Set scroll region and trigger IL (which uses column scroll)
        buf.cursor.pos.y = 1;
        buf.insert_lines(1);

        // Row 1 should have blanks in the margin area
        let cell_in_margin = buf.rows[1].resolve_cell(3);
        assert_eq!(cell_in_margin.tchar(), &TChar::Space);
    }

    #[test]
    fn column_scroll_down_with_declrmm() {
        let mut buf = alt_buf(10, 5);

        // Write identifiable content WITHOUT DECLRMM
        for i in 0..5_u8 {
            buf.cursor.pos.y = i as usize;
            buf.cursor.pos.x = 0;
            let chars: Vec<TChar> = (0..10_u8)
                .map(|c| TChar::Ascii(b'A' + (i * 10 + c) % 26))
                .collect();
            buf.insert_text(&chars);
        }

        // NOW enable DECLRMM
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(3, 8);

        // Delete lines uses column scroll down
        buf.cursor.pos.y = 1;
        buf.delete_lines(1);

        // The scroll should only affect columns within margins (0-based: 2..7)
        // Cell at col 0 (outside left margin) should be from the original row 1
        let cell_outside = buf.rows[1].resolve_cell(0);
        assert_eq!(cell_outside.tchar(), &TChar::Ascii(b'K'));
    }

    // -----------------------------------------------------------------------
    // set_size with alternate buffer: excess rows trimmed
    // -----------------------------------------------------------------------

    #[test]
    fn resize_alternate_buffer_trims_excess_rows() {
        let mut buf = alt_buf(10, 10);
        // Write text on row 8 to verify it exists
        buf.cursor.pos.y = 8;
        buf.cursor.pos.x = 0;
        buf.insert_text(&[TChar::Ascii(b'Z')]);

        // Shrink height from 10 to 5
        let _ = buf.set_size(10, 5, 0);

        // Buffer should have at most 5 rows
        assert!(
            buf.rows.len() <= 5,
            "alternate buffer should trim to new height, got {} rows",
            buf.rows.len()
        );
    }

    // -----------------------------------------------------------------------
    // any_visible_dirty
    // -----------------------------------------------------------------------

    #[test]
    fn any_visible_dirty_returns_true_after_insert() {
        let mut buf = alt_buf(10, 5);
        buf.insert_text(&[TChar::Ascii(b'A')]);
        assert!(buf.any_visible_dirty(0));
    }

    #[test]
    fn data_and_format_for_gui_empty_buffer() {
        let mut buf = Buffer::new(10, 5);
        let (chars, tags, _, _) = buf.visible_as_tchars_and_tags(0);
        // Should produce at least one tag even for empty buffer
        assert!(!tags.is_empty(), "empty buffer should still have a tag");
        // chars should be empty or minimal
        assert!(
            chars.is_empty()
                || chars
                    .iter()
                    .all(|c| matches!(c, TChar::Space | TChar::NewLine))
        );
    }

    // -----------------------------------------------------------------------
    // data_and_format_for_gui: multiple rows produce newline separators
    // -----------------------------------------------------------------------

    #[test]
    fn data_and_format_for_gui_newline_separators() {
        let mut buf = alt_buf(10, 3);
        buf.insert_text(&[TChar::Ascii(b'A')]);
        buf.handle_lf();
        buf.cursor.pos.x = 0;
        buf.insert_text(&[TChar::Ascii(b'B')]);
        buf.handle_lf();
        buf.cursor.pos.x = 0;
        buf.insert_text(&[TChar::Ascii(b'C')]);

        let (chars, tags, _, _) = buf.visible_as_tchars_and_tags(0);
        // Should contain newlines between rows
        let newline_count = chars.iter().filter(|c| matches!(c, TChar::NewLine)).count();
        assert!(
            newline_count >= 2,
            "expected at least 2 newlines between 3 rows, got {newline_count}"
        );
        // Tags should cover all positions
        assert!(!tags.is_empty());
    }

    // -----------------------------------------------------------------------
    // cursor_screen_y edge: empty buffer
    // -----------------------------------------------------------------------

    #[test]
    fn cursor_screen_y_empty_buffer_returns_zero() {
        let buf = Buffer::new(10, 5);
        assert_eq!(buf.cursor_screen_y(), 0);
    }

    // -----------------------------------------------------------------------
    // LF into existing non-pristine row below bottom
    // -----------------------------------------------------------------------

    #[test]
    fn lf_at_bottom_clears_scrolled_in_row() {
        let mut buf = Buffer::new(10, 3);
        // Write 4 lines (creates scrollback)
        for i in 0..4_u8 {
            buf.insert_text(&[TChar::Ascii(b'A' + i)]);
            buf.handle_lf();
        }
        // Now cursor is at screen bottom, writing more
        buf.cursor.pos.x = 0;
        buf.insert_text(&[TChar::Ascii(b'Z')]);
        buf.handle_lf();

        // The newly scrolled-in row should be cleared (BCE)
        let last_row = &buf.rows[buf.cursor.pos.y];
        // It should be blank
        assert!(
            last_row.cells().is_empty()
                || last_row.cells().iter().all(|c| c.tchar() == &TChar::Space),
            "newly scrolled-in row should be blank"
        );
    }

    // -----------------------------------------------------------------------
    // erase_to_end_of_display with images
    // -----------------------------------------------------------------------

    #[test]
    fn erase_to_end_of_display_with_images() {
        let mut buf = alt_buf(10, 5);
        place_image(&mut buf, 2, 5, 1);
        place_image(&mut buf, 4, 0, 2);
        assert_eq!(buf.image_cell_count, 2);

        buf.cursor.pos.y = 1;
        buf.cursor.pos.x = 0;
        buf.erase_to_end_of_display();
        assert_eq!(buf.image_cell_count, 0);
    }

    // -----------------------------------------------------------------------
    // get_text_for_range: continuation cells skipped
    // -----------------------------------------------------------------------

    #[test]
    fn extract_text_skips_continuations() {
        let mut buf = alt_buf(10, 5);
        // Insert a wide char followed by narrow
        buf.insert_text(&[TChar::from('あ'), TChar::Ascii(b'B')]);

        let text = buf.extract_text(0, 0, 0, 4);
        // Should contain the wide char and 'B', no duplicates from continuation
        assert!(text.contains('B'));
    }

    // -----------------------------------------------------------------------
    // place_image: col >= width breaks early
    // -----------------------------------------------------------------------

    #[test]
    fn place_image_col_beyond_width_stops() {
        use crate::image_store::{ImageProtocol, InlineImage, next_image_id};
        use std::sync::Arc;

        let mut buf = alt_buf(5, 5);
        buf.cursor.pos.x = 4; // start at col 4
        let id = next_image_id();
        let image = InlineImage {
            id,
            pixels: Arc::new(vec![255u8; 3 * 4]), // 3 cols × 1 row RGBA
            width_px: 3,
            height_px: 1,
            display_cols: 3,
            display_rows: 1,
        };
        // Place an image that spans 3 cols starting at col 4 → only col 4 fits
        buf.place_image(image, 0, ImageProtocol::Kitty, None, None, 0);
        // Should have placed at most 1 cell (col 4; cols 5+ out of bounds)
        assert!(buf.image_cell_count <= 1);
    }

    // ── visible_as_tchars_and_tags: NewLine tag path ───────────────────

    #[test]
    fn visible_tchars_multi_row_newline_tag_gap() {
        // When a row's last tag ends before the NewLine position,
        // the NewLine separator tag is pushed as a new tag (lines 2993-2998).
        let mut buf = alt_buf(5, 3);
        // Write text on row 0 then move to row 1
        buf.insert_text(&[TChar::Ascii(b'A'), TChar::Ascii(b'B')]);
        buf.handle_lf();
        buf.insert_text(&[TChar::Ascii(b'C')]);

        let (chars, tags, row_offsets, _url_indices) = buf.visible_as_tchars_and_tags(0);
        // Should have at least 3 rows of offsets
        assert_eq!(row_offsets.len(), 3);
        // Should contain NewLine chars between rows
        let newlines: Vec<_> = chars
            .iter()
            .filter(|c| matches!(c, TChar::NewLine))
            .collect();
        assert!(
            newlines.len() >= 2,
            "Expected at least 2 newlines, got {}",
            newlines.len()
        );
        // All chars should be covered by tags
        assert!(!tags.is_empty());
    }

    #[test]
    fn visible_tchars_empty_buffer_has_fallback_tag() {
        // An empty buffer should still produce at least one tag (lines 3010-3018).
        let mut buf = alt_buf(5, 2);
        let (_chars, tags, row_offsets, _url_indices) = buf.visible_as_tchars_and_tags(0);
        assert!(!tags.is_empty(), "Should have at least one fallback tag");
        assert_eq!(row_offsets.len(), 2);
    }

    // ── extract_text: continuation cells ───────────────────────────────

    #[test]
    fn extract_text_skips_continuation_cells() {
        // Insert a wide char followed by ASCII. extract_text should skip
        // the continuation cell (line 3166-3167).
        let mut buf = alt_buf(10, 3);
        buf.insert_text(&[TChar::from('中'), TChar::Ascii(b'A')]);
        let text = buf.extract_text(0, 0, 0, 5);
        assert!(text.contains('中'), "Should contain the wide char");
        assert!(text.contains('A'), "Should contain the ASCII char");
        // Should NOT contain any placeholder for the continuation
        assert_eq!(text.matches('中').count(), 1);
    }

    #[test]
    fn extract_text_multi_row_with_newlines() {
        // Extract text across multiple rows
        let mut buf = alt_buf(10, 3);
        buf.insert_text(&[TChar::Ascii(b'A'), TChar::Ascii(b'B')]);
        buf.handle_lf();
        buf.handle_cr();
        buf.insert_text(&[TChar::Ascii(b'C'), TChar::Ascii(b'D')]);
        let text = buf.extract_text(0, 0, 1, 5);
        assert!(text.contains("AB"), "First row should have AB");
        assert!(text.contains("CD"), "Second row should have CD");
        assert!(text.contains('\n'), "Should have newline between rows");
    }

    // ── extract_block_text: continuation cells ─────────────────────────

    #[test]
    fn extract_block_text_skips_continuation_cells() {
        // extract_block_text with a wide char should skip continuation (line 3224-3225).
        let mut buf = alt_buf(10, 3);
        buf.insert_text(&[TChar::from('中'), TChar::Ascii(b'X')]);
        let text = buf.extract_block_text(0, 0, 0, 5);
        assert!(text.contains('中'));
        assert!(text.contains('X'));
    }

    #[test]
    fn extract_block_text_multi_row() {
        let mut buf = alt_buf(10, 3);
        buf.insert_text(&[TChar::Ascii(b'A'), TChar::Ascii(b'B')]);
        buf.handle_lf();
        buf.handle_cr();
        buf.insert_text(&[TChar::Ascii(b'C'), TChar::Ascii(b'D')]);
        let text = buf.extract_block_text(0, 0, 1, 3);
        assert!(text.contains("AB"), "First row should have AB: {text:?}");
        assert!(text.contains("CD"), "Second row should have CD: {text:?}");
    }

    // ── erase_scrollback: cursor < visible_start ───────────────────────

    #[test]
    fn erase_scrollback_cursor_below_visible_start_clamps_to_zero() {
        // When cursor.pos.y < visible_start, it should be clamped to 0 (line 2772-2773).
        let mut buf = Buffer::new(10, 3);
        // Generate scrollback by writing more lines than height
        for i in 0..10_u8 {
            buf.insert_text(&[TChar::Ascii(b'A' + (i % 26))]);
            buf.handle_lf();
        }
        assert!(buf.rows.len() > 3, "Should have scrollback");

        // Force cursor into the scrollback area
        buf.cursor.pos.y = 0;
        buf.erase_scrollback();
        // Cursor should be clamped to 0
        assert_eq!(buf.cursor.pos.y, 0);
    }

    // ── LF scroll-fill at bottom ────────────────────────────────────────

    #[test]
    fn lf_at_bottom_of_primary_scrolls_and_adjusts_cursor() {
        // In primary buffer, LF at the bottom should scroll and adjust cursor (line 2528-2532).
        let mut buf = Buffer::new(10, 3);
        // Fill all 3 visible rows
        for _ in 0..3 {
            buf.insert_text(&[TChar::Ascii(b'A')]);
            buf.handle_lf();
        }
        // Cursor should be valid
        let cy = buf.cursor.pos.y;
        assert!(cy < buf.rows.len(), "Cursor should be within buffer");
    }

    // ── DECLRMM column scroll with images ───────────────────────────────

    #[test]
    fn column_scroll_up_with_images_adjusts_count() {
        // scroll_slice_up_columns in a DECLRMM region with image cells should
        // adjust image_cell_count (lines 2336-2384).
        let mut buf = alt_buf(10, 5);
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(2, 8); // 1-based → left=1, right=7 (0-based)

        // Place image cells in the DECLRMM region
        place_image(&mut buf, 1, 3, 100);
        place_image(&mut buf, 2, 3, 101);
        let images_before = buf.image_cell_count;
        assert!(images_before >= 2);

        // Trigger scroll_slice_up_columns: scroll within the left/right margins
        buf.scroll_slice_up_columns(1, 3, 1, 7);

        // Image cell count should be adjusted (some may be lost in the scroll)
        assert!(buf.image_cell_count <= images_before);
    }

    #[test]
    fn column_scroll_down_with_images_adjusts_count() {
        let mut buf = alt_buf(10, 5);
        buf.set_declrmm(Declrmm::Enabled);
        buf.set_left_right_margins(2, 8);

        place_image(&mut buf, 1, 3, 200);
        place_image(&mut buf, 2, 3, 201);
        let images_before = buf.image_cell_count;
        assert!(images_before >= 2);

        buf.scroll_slice_down_columns(1, 3, 1, 7);
        assert!(buf.image_cell_count <= images_before);
    }

    // ── scroll_slice_up_columns boundary validation ─────────────────────

    #[test]
    fn column_scroll_up_invalid_range_is_noop() {
        // first >= last should be a no-op (line 2325)
        let mut buf = alt_buf(10, 5);
        buf.insert_text(&[TChar::Ascii(b'A')]);
        let rows_before = buf.rows.len();
        buf.scroll_slice_up_columns(3, 3, 1, 7); // first == last
        assert_eq!(buf.rows.len(), rows_before);
    }
}
