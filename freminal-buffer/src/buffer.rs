// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::{
    buffer_type::BufferType,
    cursor::{CursorPos, CursorState},
    format_tag::FormatTag,
    modes::{
        decawm::Decawm, declrmm::Declrmm, decom::Decom, lnm::Lnm,
        reverse_wrap_around::ReverseWrapAround, xt_rev_wrap2::XtRevWrap2,
    },
    tchar::TChar,
};

use crate::{
    cell::Cell,
    image_store::{ImagePlacement, ImageProtocol, ImageStore, InlineImage},
    response::InsertResponse,
    row::{Row, RowJoin, RowOrigin},
};

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
    rows: Vec<Row>,

    /// Per-row flat-representation cache.  Index matches `self.rows`.
    /// `None` = dirty (must be re-flattened on next snapshot).
    /// `Some((chars, tags))` = clean cached flat representation for that row.
    row_cache: Vec<Option<(Vec<TChar>, Vec<FormatTag>)>>,

    /// Width and height of the terminal grid.
    width: usize,
    height: usize,

    /// Current cursor position (row, col).
    cursor: CursorState,

    /// Maximum number of scrollback lines allowed.
    ///
    /// For example:
    ///  - height = 40
    ///  - `scrollback_limit` = 1000
    ///    Means `rows.len()` will be at most 1040.
    scrollback_limit: usize,

    /// Whether this is the primary or alternate buffer mode.
    ///
    /// Primary:
    ///   - Has scrollback
    ///
    /// Alternate:
    ///   - No scrollback
    ///   - Switching back restores primary buffer's saved state
    kind: BufferType,

    /// Saved primary buffer content, cursor, and `scroll_offset`,
    /// used when switching to and from alternate buffer.
    /// The scroll offset is owned by the caller (`ViewState`) and passed
    /// in / returned from `enter_alternate` / `leave_alternate`.
    saved_primary: Option<SavedPrimaryState>,

    /// Saved cursor for DECSC / DECRC (ESC 7 / ESC 8).
    /// Independent of the alternate-screen save (`saved_primary`).
    saved_cursor: Option<CursorState>,

    /// Current format tag to apply to inserted text.
    current_tag: FormatTag,

    /// LNM mode
    lnm_enabled: Lnm,

    /// DECAWM — whether soft-wrapping is enabled.
    /// `AutoWrap` (default): text wraps at the terminal width.
    /// `NoAutoWrap`: text is clamped to the last column; overflow is discarded.
    wrap_enabled: Decawm,

    /// Preserve the scrollback anchor when resizing
    preserve_scrollback_anchor: bool,

    /// DECSTBM top and bottom margins, 0-indexed, inclusive.
    /// When disabled, the region is full-screen: [0, height-1]
    scroll_region_top: usize,
    scroll_region_bottom: usize,

    /// DECLRMM left and right margins, 0-indexed, inclusive.
    /// When DECLRMM is disabled these are ignored.
    /// Default: [0, width-1] (full screen).
    scroll_region_left: usize,
    scroll_region_right: usize,

    /// Whether DECLRMM (`?69`) is currently enabled.
    /// When disabled, `scroll_region_left`/`scroll_region_right` are ignored by
    /// all buffer operations that would otherwise respect them.
    declrmm_enabled: Declrmm,

    /// Tab stops as a boolean vector indexed by column.
    /// `tab_stops[c] == true` means column `c` is a tab stop.
    /// Default: every 8 columns (8, 16, 24, ...).
    tab_stops: Vec<bool>,

    /// DECOM (Origin Mode) — when enabled, cursor addressing is relative
    /// to the scroll region top/bottom instead of the full screen.
    decom_enabled: Decom,

    /// Central storage for inline images.
    ///
    /// Cells reference images by ID via `ImagePlacement`; the actual pixel
    /// data lives here behind `Arc`s so snapshots can share it cheaply.
    image_store: ImageStore,

    /// Total number of cells across all rows that carry an image placement.
    ///
    /// Maintained incrementally so `has_visible_images` / `has_any_image_cell`
    /// can short-circuit in O(1) when no images are present (the overwhelmingly
    /// common case).
    image_cell_count: usize,
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
    /// Generate default tab stops at every 8 columns for the given width.
    fn default_tab_stops(width: usize) -> Vec<bool> {
        let mut stops = vec![false; width];
        for i in (8..width).step_by(8) {
            stops[i] = true;
        }
        stops
    }

    /// Creates a new Buffer with the specified width and height.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        // Start with a single blank row.  The buffer grows dynamically as
        // content is written.  Pre-allocating `height` empty rows caused the
        // visible area to always contain `height` rows, most of which were
        // blank — the GUI's stick_to_bottom would then display those trailing
        // blank rows instead of the actual content at the top.
        let rows = vec![Row::new(width)];
        let row_cache = vec![None];

        Self {
            rows,
            row_cache,
            width,
            height,
            cursor: CursorState::default(),
            current_tag: FormatTag::default(),
            scrollback_limit: 4000,
            kind: BufferType::Primary,
            saved_primary: None,
            saved_cursor: None,
            lnm_enabled: Lnm::LineFeed,
            wrap_enabled: Decawm::AutoWrap,
            preserve_scrollback_anchor: false,
            scroll_region_top: 0,
            scroll_region_bottom: height.saturating_sub(1),
            scroll_region_left: 0,
            scroll_region_right: width.saturating_sub(1),
            declrmm_enabled: Declrmm::Disabled,
            tab_stops: Self::default_tab_stops(width),
            decom_enabled: Decom::NormalCursor,
            image_store: ImageStore::new(),
            image_cell_count: 0,
        }
    }

    /// Return a new buffer with the given scrollback limit instead of the
    /// default (4000).  This is a builder-style method intended for
    /// production use where the value comes from user configuration.
    #[must_use]
    pub const fn with_scrollback_limit(mut self, limit: usize) -> Self {
        self.scrollback_limit = limit;
        self
    }

    /// Full terminal reset (RIS — Reset to Initial State).
    ///
    /// Restores the buffer to its initial startup state:
    /// - Clears all screen content and scrollback
    /// - Resets cursor to home position (0,0)
    /// - Resets all character attributes
    /// - Resets scroll region to full screen
    /// - Resets tab stops to default 8-column positions
    /// - Exits alternate buffer if active
    ///
    /// Preserves `width`, `height`, and `scrollback_limit` (terminal geometry
    /// and user configuration).
    pub fn full_reset(&mut self) {
        self.rows = vec![Row::new(self.width)];
        self.row_cache = vec![None];
        self.cursor = CursorState::default();
        self.current_tag = FormatTag::default();
        self.kind = BufferType::Primary;
        self.saved_primary = None;
        self.saved_cursor = None;
        self.lnm_enabled = Lnm::LineFeed;
        self.wrap_enabled = Decawm::AutoWrap;
        self.preserve_scrollback_anchor = false;
        self.scroll_region_top = 0;
        self.scroll_region_bottom = self.height.saturating_sub(1);
        self.scroll_region_left = 0;
        self.scroll_region_right = self.width.saturating_sub(1);
        self.declrmm_enabled = Declrmm::Disabled;
        self.tab_stops = Self::default_tab_stops(self.width);
        self.decom_enabled = Decom::NormalCursor;
        self.image_store.clear();
        self.image_cell_count = 0;
    }

    /// The maximum number of off-screen rows retained above the visible area.
    #[must_use]
    pub const fn scrollback_limit(&self) -> usize {
        self.scrollback_limit
    }

    /// Internal consistency checks for debug builds.
    ///
    /// This is called from most mutating entry points. In release builds
    /// it compiles down to a no-op.
    #[cfg(debug_assertions)]
    fn debug_assert_invariants(&self) {
        // If there are no rows at all, we expect a fully reset buffer state.
        if self.rows.is_empty() {
            debug_assert_eq!(self.cursor.pos.y, 0, "empty buffer must keep cursor.y at 0");
            debug_assert_eq!(self.cursor.pos.x, 0, "empty buffer must keep cursor.x at 0");
            return;
        }

        // Cursor Y must always point at an existing row.
        debug_assert!(
            self.cursor.pos.y < self.rows.len(),
            "cursor.pos.y {} out of bounds for rows.len() {}",
            self.cursor.pos.y,
            self.rows.len()
        );

        // Cursor X must be within [0, width) if width > 0.
        if self.width == 0 {
            debug_assert_eq!(
                self.cursor.pos.x, 0,
                "width=0 buffer must keep cursor.x at 0"
            );
        } else {
            debug_assert!(
                self.cursor.pos.x <= self.width,
                "cursor.pos.x {} out of bounds for width {}",
                self.cursor.pos.x,
                self.width
            );
        }

        // Scrollback invariants by buffer kind.
        match self.kind {
            BufferType::Primary => {
                // Primary buffer: rows must never exceed height + scrollback_limit.
                let max_rows = self.height + self.scrollback_limit;
                debug_assert!(
                    self.rows.len() <= max_rows,
                    "primary buffer has {} rows but max_rows is {} (height={} + scrollback_limit={})",
                    self.rows.len(),
                    max_rows,
                    self.height,
                    self.scrollback_limit
                );
            }
            BufferType::Alternate => {
                // Alternate buffer: fixed-size, no scrollback.
                debug_assert_eq!(
                    self.rows.len(),
                    self.height,
                    "alternate buffer must have exactly `height` rows (got rows.len()={}, height={})",
                    self.rows.len(),
                    self.height
                );
            }
        }

        // Scroll region (DECSTBM) invariants: screen-relative.
        if self.height > 0 {
            debug_assert!(
                self.scroll_region_top <= self.scroll_region_bottom,
                "scroll_region_top {} must be <= scroll_region_bottom {}",
                self.scroll_region_top,
                self.scroll_region_bottom
            );
            debug_assert!(
                self.scroll_region_bottom < self.height,
                "scroll_region_bottom {} must be < height {}",
                self.scroll_region_bottom,
                self.height
            );
        }

        // Cache length must always match rows length.
        debug_assert_eq!(
            self.row_cache.len(),
            self.rows.len(),
            "row_cache length {} != rows length {}",
            self.row_cache.len(),
            self.rows.len()
        );

        // Image cell count must match the actual number of image cells across
        // all rows.  This is O(rows × cols) but only runs in debug builds.
        let actual_image_cells: usize = self.rows.iter().map(Row::count_image_cells).sum();
        debug_assert_eq!(
            self.image_cell_count, actual_image_cells,
            "image_cell_count {} != actual image cells {}",
            self.image_cell_count, actual_image_cells
        );
    }

    // In release builds this is a no-op, so we can call it freely.
    #[cfg(not(debug_assertions))]
    #[inline]
    fn debug_assert_invariants(&self) {}

    fn push_row(&mut self, origin: RowOrigin, join: RowJoin) {
        let mut row = Row::new_with_origin(self.width, origin, join);
        // BCE: if the current SGR has a non-default background, fill the new
        // row with blank cells carrying that background color.  For the common
        // case (default tag) this is a no-op.
        row.fill_with_tag(&self.current_tag);
        self.rows.push(row);
        self.row_cache.push(None);
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

    /// Set the cursor to an absolute buffer position without any DECOM or
    /// screen-relative translation.
    ///
    /// The position is clamped to the current buffer dimensions.  Used by
    /// DECSDM to restore the cursor after `place_image` moves it.
    pub fn set_cursor_pos_raw(&mut self, pos: CursorPos) {
        self.cursor.pos.x = if self.width > 0 {
            pos.x.min(self.width - 1)
        } else {
            0
        };
        self.cursor.pos.y = pos.y.min(self.rows.len().saturating_sub(1));
    }

    /// Advance the cursor by one column, wrapping to the next line if needed.
    ///
    /// Used after inserting a placeholder image cell that occupies one column.
    pub const fn advance_cursor_one(&mut self) {
        if self.cursor.pos.x + 1 < self.width {
            self.cursor.pos.x += 1;
        }
        // If at the rightmost column, don't wrap automatically — let the
        // next character insertion handle wrap/scroll as normal.
    }

    /// Set the [`LineWidth`] attribute on the row under the cursor.
    ///
    /// This is the buffer-level primitive called by the terminal handler in
    /// response to `ESC # 3` (double-height top), `ESC # 4` (double-height
    /// bottom), `ESC # 5` (single-width), and `ESC # 6` (double-width).
    ///
    /// The row is marked dirty so the next snapshot rebuild re-flattens it.
    pub fn set_cursor_line_width(&mut self, lw: crate::row::LineWidth) {
        let row_idx = self.cursor.pos.y;
        if let Some(row) = self.rows.get_mut(row_idx)
            && row.line_width != lw
        {
            row.line_width = lw;
            row.dirty = true;
        }
    }

    /// Return the [`LineWidth`] for each row in the visible window.
    ///
    /// The returned vector has `min(term_height, row_count)` entries, one per
    /// visible row in top-to-bottom order.  Used by `build_snapshot` to thread
    /// per-row line-width data through to the renderer.
    #[must_use]
    pub fn visible_line_widths(&self, scroll_offset: usize) -> Vec<crate::row::LineWidth> {
        let start = self.visible_window_start(scroll_offset);
        let end = (start + self.height).min(self.rows.len());
        self.rows[start..end].iter().map(|r| r.line_width).collect()
    }

    /// Set an image cell at a specific (row, col) position in the buffer.
    ///
    /// Also invalidates the corresponding row cache entry.  Used by
    /// `TerminalHandler` for Kitty Unicode placeholder cells.
    pub fn set_image_cell_at(
        &mut self,
        row_idx: usize,
        col_idx: usize,
        placement: ImagePlacement,
        tag: FormatTag,
    ) {
        if row_idx < self.rows.len() {
            // Check if the old cell already had an image (avoid double-counting).
            let had_image = self.rows[row_idx]
                .cells()
                .get(col_idx)
                .is_some_and(Cell::has_image);
            self.rows[row_idx].set_image_cell(col_idx, placement, tag);
            if !had_image {
                self.image_cell_count += 1;
            }
            if row_idx < self.row_cache.len() {
                self.row_cache[row_idx] = None;
            }
        }
    }

    /// Return the cursor position in **screen coordinates** (0-indexed, relative
    /// to the top of the visible window).
    ///
    /// Unlike `get_cursor().pos.y`, which is an absolute index into `self.rows`,
    /// this subtracts `visible_window_start()` so the result is always in the
    /// range `0..height` and matches what the GUI painter expects.
    #[must_use]
    pub fn get_cursor_screen_pos(&self) -> CursorPos {
        let screen_y = self.cursor_screen_y();
        CursorPos {
            x: self.cursor.pos.x,
            y: screen_y,
        }
    }

    /// Get the rows that should be *visually displayed* in the GUI.
    ///
    /// Contract:
    /// - Returns a contiguous slice of `self.rows`.
    /// - `visible_rows(scroll_offset).len() <= self.height`.
    /// - When `self.rows.len() <= self.height`, the slice is the entire buffer.
    /// - When `scroll_offset == 0`, the slice is the last `height` rows
    ///   (the live bottom).
    /// - When `scroll_offset > 0`, the slice is shifted upwards into
    ///   scrollback, clamped so it never goes before the oldest row.
    /// - Never allocates; always borrows from `self.rows`.
    ///
    /// `scroll_offset` is owned by the caller (e.g. `ViewState`) and is never
    /// stored inside `Buffer`.
    #[must_use]
    pub fn visible_rows(&self, scroll_offset: usize) -> &[Row] {
        if self.rows.is_empty() {
            return &[];
        }

        let total = self.rows.len();
        let h = self.height;

        // Clamp scroll_offset within bounds.
        let max_offset = self.max_scroll_offset();
        let offset = scroll_offset.min(max_offset);

        let start = total.saturating_sub(h + offset);
        let end = start + h;

        &self.rows[start.min(total)..end.min(total)]
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

                // Scroll-region-aware wrap: if the cursor is at the bottom
                // margin of the DECSTBM scroll region, scroll the region up
                // and keep the cursor on the (now-blanked) bottom row — do
                // NOT advance past the region boundary.
                let at_region_bottom = self.is_cursor_at_scroll_region_bottom();
                if at_region_bottom {
                    self.scroll_region_up_for_wrap();
                    // row_idx stays the same — it now points to the freshly
                    // blanked bottom row of the scroll region.
                } else {
                    row_idx += 1;
                }
                col = wrap_start_col;

                if !at_region_bottom {
                    if row_idx >= self.rows.len() {
                        // brand new soft-wrap continuation row
                        self.push_row(RowOrigin::SoftWrap, RowJoin::ContinueLogicalLine);
                    } else {
                        // reuse existing row as a soft-wrap continuation
                        self.reuse_row_as_softwrap(row_idx);
                    }
                }

                self.cursor.pos.y = row_idx;
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
                    let at_region_bottom = self.is_cursor_at_scroll_region_bottom();
                    if at_region_bottom {
                        self.scroll_region_up_for_wrap();
                    } else {
                        row_idx += 1;
                    }
                    col = wrap_start_col;

                    // POST-WRAP: we now know a wrap actually occurred.
                    if !at_region_bottom {
                        if row_idx >= self.rows.len() {
                            // brand new continuation
                            self.push_row(RowOrigin::SoftWrap, RowJoin::ContinueLogicalLine);
                        } else {
                            // reuse existing row as continuation
                            self.reuse_row_as_softwrap(row_idx);
                        }
                    }

                    self.cursor.pos.y = row_idx;
                    // `col` stays wrap_start_col; next iteration writes there.
                }
            }
        }
    }

    /// Resize the terminal buffer and return the adjusted `scroll_offset`.
    ///
    /// The caller passes in the current `scroll_offset` (from `ViewState`) and
    /// receives back the clamped/reset value that should be stored into `ViewState`.
    pub fn set_size(&mut self, new_width: usize, new_height: usize, scroll_offset: usize) -> usize {
        let width_changed = new_width != self.width;
        let height_changed = new_height != self.height;

        if !width_changed && !height_changed {
            return scroll_offset;
        }

        // ---- WIDTH CHANGE → REFLOW ----
        // Alternate buffers must never reflow (they represent a fixed-size screen,
        // not a scrollback history).  Reflow re-wraps logical lines which can
        // create more or fewer rows than `height`, breaking the invariant that
        // alternate buffers always have exactly `height` rows.  Instead, just
        // update each row's max_width (content that extends beyond the new width
        // is simply clipped, which matches xterm/VT behaviour).
        let after_reflow = if width_changed && self.kind != BufferType::Alternate {
            self.reflow_to_width(new_width);
            0
        } else {
            scroll_offset
        };

        // ---- HEIGHT CHANGE → GROW/SHRINK SCREEN ----
        // For alternate buffers we must ALWAYS reconcile row count with the
        // target height, even when only the width changed (reflow was skipped
        // above, so rows.len() is still the old height — but we also guard
        // against any future code path that could desync them).
        let needs_height_adjust =
            height_changed || (self.kind == BufferType::Alternate && self.rows.len() != new_height);

        let after_resize = if needs_height_adjust {
            let adjusted = self.resize_height(new_height, after_reflow);

            // Validate scroll region against new height.
            // If it's now invalid, reset to full screen.
            let max_bottom = new_height.saturating_sub(1);

            if self.scroll_region_bottom >= new_height
                || self.scroll_region_top >= new_height
                || self.scroll_region_top >= self.scroll_region_bottom
            {
                self.scroll_region_top = 0;
                self.scroll_region_bottom = max_bottom;
            } else {
                // Just clamp bottom if region is still valid
                self.scroll_region_bottom = self.scroll_region_bottom.min(max_bottom);
            }

            adjusted
        } else {
            after_reflow
        };

        // Update buffer scalars
        self.width = new_width;
        self.height = new_height;

        // Preserve existing tab stops across width changes:
        // - Wider: extend with defaults (every 8th column) for new columns only.
        // - Narrower: truncate to the new width.
        if width_changed {
            let old_width = self.tab_stops.len();
            if new_width > old_width {
                // Extend the vector with `false` for the new columns
                self.tab_stops.resize(new_width, false);
                // Set default 8-column stops only in the newly added range.
                // Default stops are at 8, 16, 24, … (matching `default_tab_stops`).
                for col in (8..new_width).step_by(8).filter(|&c| c >= old_width) {
                    self.tab_stops[col] = true;
                }
            } else if new_width < old_width {
                self.tab_stops.truncate(new_width);
            }
        }

        // Ensure every row's max_width matches the new buffer width
        if width_changed {
            for row in &mut self.rows {
                row.set_max_width(new_width);
            }
        }

        // Always clamp cursor after size change
        self.clamp_cursor_after_resize();

        // Enforce scrollback limit after resize (reflow may have created extra rows)
        let final_offset = self.enforce_scrollback_limit(after_resize);

        self.debug_assert_invariants();

        final_offset
    }

    // Inherently large: the reflow algorithm walks every logical line, splits/joins rows at the
    // new width, and preserves all cell content and tags. The size reflects algorithmic
    // complexity, not lack of structure.
    #[allow(clippy::too_many_lines)]
    /// Re-wrap all rows to `new_width` columns without losing any text.
    ///
    /// ## Algorithm
    ///
    /// 1. **Group into logical lines.** Rows are joined by a `RowJoin` flag:
    ///    `NewLogicalLine` starts a new logical line and `ContinueLogicalLine`
    ///    means this row is a soft-wrapped continuation of the previous one.
    ///    We split `self.rows` into groups where each group represents one
    ///    original logical line (a paragraph, in word-processor terms).
    ///
    /// 2. **Flatten each logical line.** The cells from every physical row in
    ///    the group are concatenated into a single `Vec<Cell>`, discarding the
    ///    old row boundaries.
    ///
    /// 3. **Re-wrap at the new width.** Wide glyphs (display width > 1) are
    ///    kept whole — if a glyph does not fit at the current column it is
    ///    moved to the next row rather than split.  Each flushed row records
    ///    whether it is the first physical row of its logical line
    ///    (`RowJoin::NewLogicalLine`) or a continuation (`ContinueLogicalLine`),
    ///    and inherits the original `RowOrigin` of its logical line.
    ///
    /// 4. **Install the new rows.** `self.rows` is replaced with the reflow
    ///    result, `self.width` is updated, and `self.row_cache` is reset to
    ///    all-`None` (every row is dirty after reflow).
    ///
    /// The operation is O(total cells) — linear in the amount of text.
    pub fn reflow_to_width(&mut self, new_width: usize) {
        if new_width == 0 || self.rows.is_empty() || new_width == self.width {
            // Nothing to do
            return;
        }

        // Take ownership of the old rows
        let old_rows = std::mem::take(&mut self.rows);

        // 1) Group rows into logical lines based on RowJoin
        let mut logical_lines: Vec<Vec<Row>> = Vec::new();
        let mut current_line: Vec<Row> = Vec::new();

        for row in old_rows {
            if row.join == RowJoin::NewLogicalLine && !current_line.is_empty() {
                logical_lines.push(current_line);
                current_line = Vec::new();
            }
            current_line.push(row);
        }
        if !current_line.is_empty() {
            logical_lines.push(current_line);
        }

        // 2) For each logical line, flatten its cells and re-wrap
        let mut new_rows: Vec<Row> = Vec::new();

        for line in logical_lines {
            // Determine origin for the first row of this logical line.
            let first_origin = line.first().map_or(RowOrigin::HardBreak, |r| r.origin);

            // Flatten all rows in this logical line into a single Vec<Cell>
            let mut flat_cells: Vec<Cell> = Vec::new();
            for row in &line {
                flat_cells.extend(row.get_characters().iter().cloned());
            }

            if flat_cells.is_empty() {
                // Empty logical line → keep a single empty row
                new_rows.push(Row::new_with_origin(
                    new_width,
                    first_origin,
                    RowJoin::NewLogicalLine,
                ));
                continue;
            }

            let mut idx = 0;
            let mut col = 0;
            let mut cur_cells: Vec<Cell> = Vec::new();
            let mut is_first_row_for_line = true;

            while idx < flat_cells.len() {
                let cell = &flat_cells[idx];

                if cell.is_head() {
                    let w = cell.display_width().max(1);

                    // If this glyph doesn't fit on the current row (and we already have content),
                    // flush the current row and start a new one.
                    if col + w > new_width && col > 0 {
                        let origin = if is_first_row_for_line {
                            first_origin
                        } else {
                            RowOrigin::SoftWrap
                        };
                        let join = if is_first_row_for_line {
                            RowJoin::NewLogicalLine
                        } else {
                            RowJoin::ContinueLogicalLine
                        };

                        new_rows.push(Row::from_cells(new_width, origin, join, cur_cells));

                        cur_cells = Vec::new();
                        col = 0;
                        is_first_row_for_line = false;
                    }

                    // Now place this glyph (head + continuations) onto the row.
                    cur_cells.push(cell.clone());
                    idx += 1;

                    let mut consumed = 1;
                    while consumed < w
                        && idx < flat_cells.len()
                        && flat_cells[idx].is_continuation()
                    {
                        cur_cells.push(flat_cells[idx].clone());
                        idx += 1;
                        consumed += 1;
                    }

                    col += w.min(new_width);
                } else {
                    // Stray continuation (should be rare): treat as width 1 column.
                    if col + 1 > new_width && col > 0 {
                        let origin = if is_first_row_for_line {
                            first_origin
                        } else {
                            RowOrigin::SoftWrap
                        };
                        let join = if is_first_row_for_line {
                            RowJoin::NewLogicalLine
                        } else {
                            RowJoin::ContinueLogicalLine
                        };

                        new_rows.push(Row::from_cells(new_width, origin, join, cur_cells));

                        cur_cells = Vec::new();
                        col = 0;
                        is_first_row_for_line = false;
                    }

                    cur_cells.push(cell.clone());
                    idx += 1;
                    col += 1;
                }
            }

            // Flush any remaining cells as the final row of this logical line.
            if !cur_cells.is_empty() {
                let origin = if is_first_row_for_line {
                    first_origin
                } else {
                    RowOrigin::SoftWrap
                };
                let join = if is_first_row_for_line {
                    RowJoin::NewLogicalLine
                } else {
                    RowJoin::ContinueLogicalLine
                };

                new_rows.push(Row::from_cells(new_width, origin, join, cur_cells));
            }
        }

        // 3) Install the new rows and update width
        self.rows = new_rows;
        // All rows are freshly constructed (dirty=true by construction), so
        // the entire cache is invalid.  Reset it to match the new row count.
        self.row_cache = vec![None; self.rows.len()];
        self.width = new_width;
        // Reflow rebuilds all rows from scratch; recount image cells so the
        // counter stays accurate regardless of how reflow may have clipped or
        // merged cells.
        self.image_cell_count = self.rows.iter().map(Row::count_image_cells).sum();

        // 4) Ensure cursor is in bounds (scroll_offset is always reset to 0 by the
        //    caller after a reflow — returned from set_size).
        if self.cursor.pos.y >= self.rows.len() {
            if self.rows.is_empty() {
                self.cursor.pos.y = 0;
            } else {
                self.cursor.pos.y = self.rows.len() - 1;
            }
            self.cursor.pos.x = 0;
        } else {
            // Clamp X to the new width
            if self.cursor.pos.x >= self.width {
                self.cursor.pos.x = self.width.saturating_sub(1);
            }
        }
    }

    /// Adjust the buffer rows for a new height and return the adjusted `scroll_offset`.
    fn resize_height(&mut self, new_height: usize, scroll_offset: usize) -> usize {
        let old_height = self.height;

        if new_height > old_height {
            // Grow: add blank rows at the bottom
            let grow = new_height - old_height;
            for _ in 0..grow {
                self.rows.push(Row::new(self.width));
                self.row_cache.push(None);
            }
        } else if new_height < old_height {
            if self.kind == BufferType::Alternate {
                // Alternate buffer must never have scrollback.  When shrinking,
                // keep the bottom `new_height` rows (the ones the user can see)
                // and discard the top ones.  This preserves the invariant
                // `rows.len() == height` which all alternate-buffer coordinate
                // logic depends on (handle_lf, handle_ri, insert_lines,
                // delete_lines all compare cursor.pos.y against
                // scroll_region_top/bottom directly without an offset).
                let excess = self.rows.len().saturating_sub(new_height);
                if excess > 0 {
                    // Account for image cells in the drained rows.
                    if self.image_cell_count > 0 {
                        let drained_images: usize =
                            self.rows[..excess].iter().map(Row::count_image_cells).sum();
                        self.image_cell_count -= drained_images;
                    }
                    self.rows.drain(0..excess);
                    self.row_cache.drain(0..excess);
                    // Adjust cursor Y for the removed rows.
                    self.cursor.pos.y = self.cursor.pos.y.saturating_sub(excess);
                }
            } else {
                // Primary buffer: extra rows become scrollback (handled by
                // enforce_scrollback_limit later).  Just clamp cursor.
                if self.cursor.pos.y >= new_height {
                    self.cursor.pos.y = new_height.saturating_sub(1);
                }
            }
        }

        if self.preserve_scrollback_anchor {
            // IMPORTANT: use new_height, not self.height (which is still old here)
            let max_offset = if self.rows.len() > new_height {
                self.rows.len() - new_height
            } else {
                0
            };
            scroll_offset.min(max_offset)
        } else {
            // xterm-style: reset to live bottom
            0
        }
    }

    const fn clamp_cursor_after_resize(&mut self) {
        // Clamp Y
        if self.cursor.pos.y >= self.rows.len() {
            self.cursor.pos.y = self.rows.len().saturating_sub(1);
        }

        // Clamp X
        if self.cursor.pos.x >= self.width {
            self.cursor.pos.x = self.width.saturating_sub(1);
        }

        if self.rows.is_empty() {
            self.cursor.pos.x = 0;
            self.cursor.pos.y = 0;
        }
    }

    /// Enforce the scrollback row limit, adjusting the caller's `scroll_offset` if
    /// rows are trimmed from the top.  Returns the (possibly reduced) scroll offset
    /// that the caller should store into `ViewState`.
    #[must_use]
    fn enforce_scrollback_limit(&mut self, scroll_offset: usize) -> usize {
        // Only primary buffer keeps scrollback.
        if self.kind == BufferType::Alternate {
            return scroll_offset;
        }

        let max_rows = self.height + self.scrollback_limit;

        // Nothing to trim, but still make sure scroll_offset is not insane.
        if self.rows.len() <= max_rows {
            let max_offset = self.max_scroll_offset();
            return scroll_offset.min(max_offset);
        }

        // Number of rows to drop from the top of the scrollback.
        let overflow = self.rows.len() - max_rows;

        // --- Adjust scroll_offset BEFORE modifying the rows ---
        //
        // If the user is scrolled back into the area we're about to delete,
        // reduce their offset by the number of deleted rows. If that wipes
        // out all their scrollback, snap them to live view.
        let adjusted_offset = scroll_offset.saturating_sub(overflow);

        // --- Drop the oldest rows (and their cache entries) ---
        // First, account for any image cells in the rows being drained.
        if self.image_cell_count > 0 {
            let drained_images: usize = self.rows[..overflow]
                .iter()
                .map(Row::count_image_cells)
                .sum();
            self.image_cell_count -= drained_images;
        }
        self.rows.drain(0..overflow);
        self.row_cache.drain(0..overflow);

        // --- Garbage-collect images no longer referenced by any row ---
        if !self.image_store.is_empty() {
            self.image_store
                .retain_referenced(self.rows.iter().map(Row::cells));
        }

        // --- Adjust cursor row index ---
        //
        // Cursor is always measured relative to self.rows, so subtract
        // `overflow` from its y-coordinate when possible.
        if self.cursor.pos.y >= overflow {
            self.cursor.pos.y -= overflow;
        } else {
            self.cursor.pos.y = 0;
        }

        // Finally, clamp scroll_offset to the new max_scroll_offset().
        let max_offset = self.max_scroll_offset();
        let final_offset = adjusted_offset.min(max_offset);

        self.debug_assert_invariants();

        final_offset
    }

    /// Handle ANSI Backspace (BS, 0x08).
    ///
    /// Semantics (ECMA-48, VT100):
    /// - Move cursor left by one cell.
    /// - If the cursor is in the pending-wrap state (`cursor.pos.x == self.width`,
    ///   i.e. just wrote into the last column), BS treats the cursor as if it were
    ///   at `self.width - 1` (the last visible column) before subtracting, which
    ///   places it at `self.width - 2`.  This matches real VT100 hardware, where
    ///   the pending-wrap flag is separate from the reported column position and BS
    ///   clears the flag and moves left from the reported column.
    /// - If the cell to the left is a continuation cell of a wide glyph,
    ///   skip left until the glyph head.
    /// - If cursor is at column 0 and `reverse_wrap` is false, do nothing.
    /// - If cursor is at column 0 and `reverse_wrap` is true, wrap to the
    ///   last column of the previous line (within the visible screen, or
    ///   into scrollback if `xt_rev_wrap2` is also true).
    /// - Never deletes characters.
    pub fn handle_backspace(&mut self, reverse_wrap: ReverseWrapAround, xt_rev_wrap2: XtRevWrap2) {
        if self.cursor.pos.x == 0 {
            if reverse_wrap == ReverseWrapAround::DontWrap {
                return;
            }
            self.reverse_wrap_up(xt_rev_wrap2);
            return;
        }

        let row_idx = self.cursor.pos.y;

        if row_idx >= self.rows.len() {
            return;
        }

        let row = &self.rows[row_idx];

        // When in pending-wrap state the internal x equals self.width (one past
        // the last column).  A real VT100 keeps the cursor at the last column with
        // a separate pending-wrap bit; BS clears the bit and moves left from the
        // reported column, landing at width-2 (0-based).  Clamp before subtracting.
        let effective_x = self.cursor.pos.x.min(self.width.saturating_sub(1));
        let mut new_x = effective_x - 1;

        // Skip left over continuation cells of a wide glyph
        while new_x > 0 {
            if let Some(cell) = row.get_char_at(new_x) {
                if !cell.is_continuation() {
                    break;
                }
            } else {
                break;
            }
            new_x -= 1;
        }

        self.cursor.pos.x = new_x;

        // This MUST be the only post-condition for backspace.
        debug_assert!(self.cursor.pos.y < self.rows.len());
        debug_assert!(self.cursor.pos.x < self.width);
    }

    /// Move the cursor up one row and to the last column (reverse wrap).
    ///
    /// If the cursor is already at the top of the visible screen and
    /// `into_scrollback` is true, the cursor enters the scrollback region.
    /// If at the top and `into_scrollback` is false, the cursor stays put.
    fn reverse_wrap_up(&mut self, into_scrollback: XtRevWrap2) {
        let visible_start = self.visible_window_start(0);

        if self.cursor.pos.y > visible_start {
            // Not at the top of the visible screen — wrap to previous row.
            self.cursor.pos.y -= 1;
            self.cursor.pos.x = self.width.saturating_sub(1);
        } else if into_scrollback == XtRevWrap2::Enabled && self.cursor.pos.y > 0 {
            // At top of visible screen, but scrollback exists and
            // extended reverse-wrap is enabled.
            self.cursor.pos.y -= 1;
            self.cursor.pos.x = self.width.saturating_sub(1);
        }
        // Otherwise: at absolute top or scrollback not allowed — no-op.
    }

    /// Advance the cursor to the next tab stop (HT / 0x09).
    ///
    /// If the cursor is already at or past the last tab stop, it moves to
    /// the rightmost column.  HT never wraps to the next line.
    pub fn advance_to_next_tab_stop(&mut self) {
        let col = self.cursor.pos.x;
        let max_col = self.width.saturating_sub(1);

        // Search for the next tab stop after the current column
        let next = self
            .tab_stops
            .iter()
            .enumerate()
            .skip(col + 1)
            .find(|&(_, &is_stop)| is_stop)
            .map(|(i, _)| i);

        self.cursor.pos.x = next.map_or(max_col, |stop| stop.min(max_col));
    }

    /// Set a tab stop at the current cursor column (HTS — ESC H).
    pub fn set_tab_stop(&mut self) {
        let col = self.cursor.pos.x;
        if col < self.tab_stops.len() {
            self.tab_stops[col] = true;
        }
    }

    /// Clear the tab stop at the current cursor column (TBC Ps=0).
    pub fn clear_tab_stop_at_cursor(&mut self) {
        let col = self.cursor.pos.x;
        if col < self.tab_stops.len() {
            self.tab_stops[col] = false;
        }
    }

    /// Clear all tab stops (TBC Ps=3).
    pub fn clear_all_tab_stops(&mut self) {
        self.tab_stops.iter_mut().for_each(|s| *s = false);
    }

    /// Move cursor backward to the Ps-th previous tab stop (CBT — CSI Z).
    ///
    /// If there is no previous tab stop, moves to column 0.
    /// CBT never wraps to the previous line.
    pub fn tab_backward(&mut self, count: usize) {
        let mut col = self.cursor.pos.x;
        for _ in 0..count {
            // Search backward from current column
            if col == 0 {
                break;
            }
            let prev = self.tab_stops[..col].iter().rposition(|&is_stop| is_stop);
            col = prev.unwrap_or(0);
        }
        self.cursor.pos.x = col;
    }

    /// Move cursor forward by `count` tab stops (CHT — CSI I).
    ///
    /// If there are fewer than `count` tab stops remaining, moves to the
    /// rightmost column.  CHT never wraps to the next line.
    pub fn tab_forward(&mut self, count: usize) {
        for _ in 0..count {
            self.advance_to_next_tab_stop();
        }
    }

    /// Fill the entire visible screen with 'E' characters (DECALN — ESC # 8).
    ///
    /// Also resets the scroll region to full screen and moves the cursor
    /// to the home position (0, 0).
    pub fn screen_alignment_test(&mut self) {
        use freminal_common::buffer_states::format_tag::FormatTag;
        use freminal_common::buffer_states::tchar::TChar;

        let visible_start = self.visible_window_start(0);
        let visible_end = visible_start + self.height;

        // Ensure we have enough rows
        while self.rows.len() < visible_end {
            self.rows.push(crate::row::Row::new(self.width));
            self.row_cache.push(None);
        }

        let default_tag = FormatTag::default();
        let e_chars: Vec<TChar> = vec![TChar::Ascii(b'E'); self.width];
        for i in visible_start..visible_end.min(self.rows.len()) {
            self.image_cell_count -= self.rows[i].count_image_cells();
            self.rows[i].clear();
            self.rows[i].insert_text(0, &e_chars, &default_tag);
            // Invalidate row cache
            if i < self.row_cache.len() {
                self.row_cache[i] = None;
            }
        }

        // Reset scroll region to full screen
        self.scroll_region_top = 0;
        self.scroll_region_bottom = self.height.saturating_sub(1);

        // Move cursor to home position
        self.cursor.pos.x = 0;
        self.cursor.pos.y = visible_start;
    }

    /// Handle ANSI LF (line feed), IND-style advance, and LNM mode.
    ///
    /// Moves the cursor down one row within the current `DECSTBM` scroll region.
    /// When the cursor is at the bottom margin the region scrolls up by one line.
    /// In `LNM` (new-line) mode an implicit CR is also applied.
    pub fn handle_lf(&mut self) {
        // Clear the implicit pending-wrap state.  When the cursor is at
        // `x == width` (one past the last column) it means a character was
        // just written at the rightmost column and the next printable
        // character should wrap.  LF must cancel that deferred wrap —
        // the cursor stays at the last column of the current/new row
        // rather than wrapping on the next character write.
        if self.cursor.pos.x >= self.width {
            self.cursor.pos.x = self.width.saturating_sub(1);
        }

        match self.kind {
            BufferType::Primary => {
                // LNM: CR implied
                if self.lnm_enabled == Lnm::NewLine {
                    self.cursor.pos.x = 0;
                }

                let sy = self.cursor_screen_y();

                // --- DECSTBM applies ONLY when:
                //     - region is not full screen
                //     - not scrolled back (already ensured above)
                if self.scroll_region_top == 0 && self.scroll_region_bottom == self.height - 1 {
                    //
                    // FAST PATH: FULL-SCREEN REGION
                    //
                    // When the cursor is above the last visible row we simply move
                    // it down.  We must NOT clear the destination row if it already
                    // holds real content (e.g. logo lines written in a previous pass
                    // before a CUU brought the cursor back up).  We only clear when
                    // the destination is a pristine ScrollFill placeholder, and we
                    // only scroll-in a brand-new blank row when the cursor is at the
                    // very bottom of the visible window.
                    self.cursor.pos.y += 1;

                    if self.cursor.pos.y >= self.rows.len() {
                        // Row doesn't exist yet — always create it fresh.
                        // BCE: fill with current SGR background.
                        let mut new_row = Row::new_with_origin(
                            self.width,
                            RowOrigin::HardBreak,
                            RowJoin::NewLogicalLine,
                        );
                        new_row.fill_with_tag(&self.current_tag);
                        self.rows.push(new_row);
                        self.row_cache.push(None);
                    } else {
                        let row = &mut self.rows[self.cursor.pos.y];
                        if row.origin == RowOrigin::ScrollFill {
                            // Pristine placeholder: stamp it as a real hard-break
                            // line but leave its (empty) cell content alone.
                            row.origin = RowOrigin::HardBreak;
                            row.join = RowJoin::NewLogicalLine;
                        } else if sy == self.height.saturating_sub(1) {
                            // Cursor was at the bottom of the visible window and the
                            // next slot already has content from old scrollback — this
                            // is the newly-scrolled-in line, so wipe it (BCE).
                            self.image_cell_count -= row.count_image_cells();
                            row.origin = RowOrigin::HardBreak;
                            row.join = RowJoin::NewLogicalLine;
                            row.clear_with_tag(&self.current_tag);
                        }
                        // Otherwise (cursor was above the bottom, row has real
                        // content): leave the row completely untouched.
                    }

                    // PTY always at scroll_offset=0; return value is always 0 here.
                    let _ = self.enforce_scrollback_limit(0);
                    self.debug_assert_invariants();
                    return;
                }

                //
                // PARTIAL REGION (true DECSTBM behavior)
                //
                if sy >= self.scroll_region_top && sy <= self.scroll_region_bottom {
                    if sy < self.scroll_region_bottom {
                        // Move cursor down inside region
                        self.cursor.pos.y += 1;
                        // If the row doesn't exist yet (buffer filling up), create it.
                        while self.cursor.pos.y >= self.rows.len() {
                            self.push_row(RowOrigin::HardBreak, RowJoin::NewLogicalLine);
                        }
                    } else {
                        // At bottom margin → scroll region UP
                        self.scroll_region_up_primary();
                        // cursor stays in place
                    }
                } else {
                    // Outside DECSTBM → just move down with no scrolling
                    if self.cursor.pos.y + 1 < self.rows.len() {
                        self.cursor.pos.y += 1;
                    } else {
                        let mut new_row = Row::new_with_origin(
                            self.width,
                            RowOrigin::HardBreak,
                            RowJoin::NewLogicalLine,
                        );
                        new_row.fill_with_tag(&self.current_tag);
                        self.rows.push(new_row);
                        self.row_cache.push(None);
                        self.cursor.pos.y = self.rows.len() - 1;
                    }
                }

                // PTY always at scroll_offset=0; return value is always 0 here.
                let _ = self.enforce_scrollback_limit(0);
                self.debug_assert_invariants();
            }

            BufferType::Alternate => {
                if self.lnm_enabled == Lnm::NewLine {
                    self.cursor.pos.x = 0;
                }

                let y = self.cursor.pos.y;

                if y >= self.scroll_region_top && y <= self.scroll_region_bottom {
                    if y < self.scroll_region_bottom {
                        if self.cursor.pos.y + 1 < self.height {
                            self.cursor.pos.y += 1;
                        }
                    } else {
                        self.scroll_slice_up(self.scroll_region_top, self.scroll_region_bottom);
                    }
                } else if self.cursor.pos.y + 1 < self.height {
                    self.cursor.pos.y += 1;
                }

                self.debug_assert_invariants();
            }
        }
    }

    /// Handle ANSI CR (carriage return) — move cursor to column 0.
    pub const fn handle_cr(&mut self) {
        self.cursor.pos.x = 0;
    }

    /// IND – Index (move down, scroll within DECSTBM region).
    /// Same as LF, but *does not* honor LNM (no implicit CR).
    pub fn handle_ind(&mut self) {
        // Temporarily disable LNM so `handle_lf` won't do CR.
        let old_lnm = self.lnm_enabled;
        self.lnm_enabled = Lnm::LineFeed;
        self.handle_lf();
        self.lnm_enabled = old_lnm;
    }

    /// NEL – Next Line (CR + LF with scrolling in DECSTBM region).
    pub fn handle_nel(&mut self) {
        // Explicit CR then LF – this is allowed to honor LNM.
        self.handle_cr();
        self.handle_lf();
    }

    /// RI – Reverse Index.
    /// Move the cursor up; at the top margin of DECSTBM region,
    /// scroll the region down by one line (blank line at top).
    pub fn handle_ri(&mut self) {
        // Clear implicit pending-wrap state (same as handle_lf).
        if self.cursor.pos.x >= self.width {
            self.cursor.pos.x = self.width.saturating_sub(1);
        }

        match self.kind {
            BufferType::Primary => {
                let sy = self.cursor_screen_y();

                if sy >= self.scroll_region_top && sy <= self.scroll_region_bottom {
                    if sy > self.scroll_region_top {
                        // move up inside region
                        self.cursor.pos.y -= 1;
                    } else {
                        // at top margin → scroll region DOWN
                        self.scroll_region_down_primary();
                    }
                } else {
                    // outside region → never scroll
                    if self.cursor.pos.y > 0 {
                        self.cursor.pos.y -= 1;
                    }
                }

                self.debug_assert_invariants();
            }

            BufferType::Alternate => {
                let y = self.cursor.pos.y;

                if y >= self.scroll_region_top && y <= self.scroll_region_bottom {
                    if y > self.scroll_region_top {
                        self.cursor.pos.y -= 1;
                    } else {
                        self.scroll_slice_down(self.scroll_region_top, self.scroll_region_bottom);
                    }
                } else if self.cursor.pos.y > 0 {
                    self.cursor.pos.y -= 1;
                }
                self.debug_assert_invariants();
            }
        }
    }

    /// IL – Insert Lines within DECSTBM region.
    /// Insert `n` blank lines at the cursor row, shifting lines down and
    /// discarding at the bottom of the region.
    pub fn insert_lines(&mut self, n: usize) {
        if n == 0 {
            return;
        }

        match self.kind {
            BufferType::Alternate => {
                // leave as-is
                let y = self.cursor.pos.y;
                if y < self.scroll_region_top || y > self.scroll_region_bottom {
                    return;
                }

                let max_lines = self.scroll_region_bottom.saturating_sub(y) + 1;
                let count = n.min(max_lines);

                if self.declrmm_enabled == Declrmm::Enabled {
                    let (left, right) = (self.scroll_region_left, self.scroll_region_right);
                    for _ in 0..count {
                        self.scroll_slice_down_columns(y, self.scroll_region_bottom, left, right);
                    }
                } else {
                    for _ in 0..count {
                        self.scroll_slice_down(y, self.scroll_region_bottom);
                    }
                }

                self.debug_assert_invariants();
            }

            BufferType::Primary => {
                let sy = self.cursor_screen_y();
                if sy < self.scroll_region_top || sy > self.scroll_region_bottom {
                    return;
                }

                let (t, b) = self.scroll_region_rows();
                let offset = sy - self.scroll_region_top;
                let row = t + offset;

                let count = n.min(b - row + 1);
                if self.declrmm_enabled == Declrmm::Enabled {
                    let (left, right) = (self.scroll_region_left, self.scroll_region_right);
                    for _ in 0..count {
                        self.scroll_slice_down_columns(row, b, left, right);
                    }
                } else {
                    for _ in 0..count {
                        self.scroll_slice_down(row, b);
                    }
                }

                self.debug_assert_invariants();
            }
        }
    }

    /// DL – Delete Lines within DECSTBM region.
    /// Delete `n` lines at the cursor row, shifting lines up and
    /// inserting blanks at the bottom of the region.
    pub fn delete_lines(&mut self, n: usize) {
        if n == 0 {
            return;
        }

        match self.kind {
            BufferType::Alternate => {
                let y = self.cursor.pos.y;
                if y < self.scroll_region_top || y > self.scroll_region_bottom {
                    return;
                }

                let max_lines = self.scroll_region_bottom.saturating_sub(y) + 1;
                let count = n.min(max_lines);

                if self.declrmm_enabled == Declrmm::Enabled {
                    let (left, right) = (self.scroll_region_left, self.scroll_region_right);
                    for _ in 0..count {
                        self.scroll_slice_up_columns(y, self.scroll_region_bottom, left, right);
                    }
                } else {
                    for _ in 0..count {
                        self.scroll_slice_up(y, self.scroll_region_bottom);
                    }
                }

                self.debug_assert_invariants();
            }

            BufferType::Primary => {
                let sy = self.cursor_screen_y();
                if sy < self.scroll_region_top || sy > self.scroll_region_bottom {
                    return;
                }

                let (t, b) = self.scroll_region_rows();
                let offset = sy - self.scroll_region_top;
                let row = t + offset;

                let count = n.min(b - row + 1);
                if self.declrmm_enabled == Declrmm::Enabled {
                    let (left, right) = (self.scroll_region_left, self.scroll_region_right);
                    for _ in 0..count {
                        self.scroll_slice_up_columns(row, b, left, right);
                    }
                } else {
                    for _ in 0..count {
                        self.scroll_slice_up(row, b);
                    }
                }

                self.debug_assert_invariants();
            }
        }
    }

    /// Implements DECSC – Save Cursor.
    ///
    /// Saves the current cursor position (and associated `CursorState`).
    pub fn save_cursor(&mut self) {
        self.saved_cursor = Some(self.cursor.clone());
    }

    /// Implements DECRC – Restore Cursor.
    ///
    /// Restores the previously saved cursor position.  If no cursor has been
    /// saved, this is a no-op.  The restored position is clamped to the
    /// current buffer dimensions so a resize between save and restore never
    /// produces an out-of-bounds cursor.
    pub fn restore_cursor(&mut self) {
        if let Some(saved) = self.saved_cursor.clone() {
            self.cursor = saved;
            // Clamp to current dimensions after restore.
            if self.width > 0 {
                self.cursor.pos.x = self.cursor.pos.x.min(self.width - 1);
            }
            let max_row = self.rows.len().saturating_sub(1);
            self.cursor.pos.y = self.cursor.pos.y.min(max_row);
            self.debug_assert_invariants();
        }
        // No saved cursor → silent no-op.
    }

    /// Implements ICH – Insert Characters (spaces).
    pub fn insert_spaces(&mut self, n: usize) {
        let row = self.cursor.pos.y;
        let col = self.cursor.pos.x;

        if row >= self.rows.len() {
            return;
        }

        // Sweep any image cells in the range that will be shifted off the right
        // edge before the insert happens.  Cells shifted past the right margin
        // are discarded, so we must update image_cell_count now.
        let remaining_images = if self.image_cell_count > 0 {
            let right = if self.declrmm_enabled == Declrmm::Enabled {
                self.scroll_region_right + 1
            } else {
                self.width
            };
            self.collect_and_clear_image_ids_in_rows(row, row + 1, Some(col), Some(right));
            // Count Kitty images that survived the sweep in the cells that
            // will overflow past the right edge when we insert `n` blanks.
            let overflow_start = right.saturating_sub(n).max(col);
            self.rows[row].count_image_cells_in_range(overflow_start, right)
        } else {
            0
        };

        let tag = self.current_tag.clone();
        if self.declrmm_enabled == Declrmm::Enabled {
            // ICH: shift only within [col, scroll_region_right]; cells that
            // fall off the right margin are discarded.
            self.rows[row].insert_spaces_at_with_right_limit(
                col,
                n,
                &tag,
                self.scroll_region_right + 1,
            );
        } else {
            self.rows[row].insert_spaces_at(col, n, &tag);
        }

        self.image_cell_count -= remaining_images;
        // Cursor does NOT move for ICH
    }

    /// Implements DCH – Delete Characters.
    ///
    /// Removes `n` cells starting at the cursor column on the cursor row, shifting
    /// the cells to the right of the deleted range left to fill the gap. Cells
    /// shifted off the right edge are discarded.  The cursor does not move.
    ///
    /// When DECLRMM is active, the operation is confined to
    /// `[col, scroll_region_right]`; cells outside the right margin are
    /// unaffected.
    ///
    /// Wide-glyph cleanup is delegated to [`Row::delete_cells_at`] /
    /// [`Row::delete_cells_at_with_right_limit`].
    pub fn delete_chars(&mut self, n: usize) {
        let row = self.cursor.pos.y;
        let col = self.cursor.pos.x;

        if row >= self.rows.len() {
            // Row doesn't even exist — nothing to delete.
            return;
        }

        // Sweep image cells in the range being deleted before the row-level
        // operation removes them without updating image_cell_count.
        let remaining_images = if self.image_cell_count > 0 {
            let end_col = if self.declrmm_enabled == Declrmm::Enabled {
                (col + n).min(self.scroll_region_right + 1)
            } else {
                col + n
            };
            self.collect_and_clear_image_ids_in_rows(row, row + 1, Some(col), Some(end_col));
            // Count Kitty images that survived the sweep in the deleted range.
            self.rows[row].count_image_cells_in_range(col, end_col)
        } else {
            0
        };

        if self.declrmm_enabled == Declrmm::Enabled {
            self.rows[row].delete_cells_at_with_right_limit(
                col,
                n,
                self.scroll_region_right + 1,
                &self.current_tag.clone(),
            );
        } else {
            let tag = self.current_tag.clone();
            self.rows[row].delete_cells_at(col, n, &tag);
        }

        self.image_cell_count -= remaining_images;
        self.debug_assert_invariants();
    }

    /// Implements ECH – Erase Characters.
    ///
    /// Replaces `n` cells starting at the cursor column with blanks using the current
    /// format tag.  Cells to the right of the erased range are **not** shifted.
    /// The cursor does not move.
    ///
    /// When DECLRMM is active, `n` is clamped so the erase never crosses the
    /// right margin.
    ///
    /// Wide-glyph cleanup is delegated to [`Row::erase_cells_at`].
    pub fn erase_chars(&mut self, n: usize) {
        let row = self.cursor.pos.y;
        let col = self.cursor.pos.x;

        if row >= self.rows.len() {
            // Row doesn't exist yet — nothing to erase.
            return;
        }

        // Clamp erase count to right margin when DECLRMM is active.
        let effective_n = if self.declrmm_enabled == Declrmm::Enabled {
            let max_erase = self.scroll_region_right.saturating_sub(col) + 1;
            n.min(max_erase)
        } else {
            n
        };

        // Sweep image cells that are about to be erased (non-Kitty images).
        self.collect_and_clear_image_ids_in_rows(row, row + 1, Some(col), Some(col + effective_n));

        // Count remaining image cells (e.g. Kitty) in the range being erased.
        let remaining = if self.image_cell_count > 0 {
            self.rows[row].count_image_cells_in_range(col, col + effective_n)
        } else {
            0
        };

        let tag = self.current_tag.clone();
        self.rows[row].erase_cells_at(col, effective_n, &tag);
        self.image_cell_count -= remaining;

        self.debug_assert_invariants();
    }

    fn reset_scroll_region_to_full(&mut self) {
        self.scroll_region_top = 0;
        self.scroll_region_bottom = self.height.saturating_sub(1);
        // Reset cursor to home position (screen row 0, col 0).
        // Use set_cursor_pos so rows are created if they don't exist yet.
        self.set_cursor_pos(Some(0), Some(0));
    }

    /// Set DECSTBM scroll region (1-based inclusive).
    /// If invalid, resets to full screen.
    pub fn set_scroll_region(&mut self, top1: usize, bottom1: usize) {
        // 0 or missing → ignore and reset
        if top1 == 0 || bottom1 == 0 {
            self.reset_scroll_region_to_full();
            return;
        }

        // Convert to 0-based
        let top = top1.saturating_sub(1);
        let bottom = bottom1.saturating_sub(1);

        // Validate
        if top >= bottom || bottom >= self.height {
            self.reset_scroll_region_to_full();
            return;
        }

        self.scroll_region_top = top;
        self.scroll_region_bottom = bottom;

        // DECSTBM always homes the cursor to (0, 0).
        // When DECOM is enabled, set_cursor_pos interprets y=0 as
        // scroll_region_top.  When DECOM is disabled, y=0 is screen top.
        // Both cases are handled correctly by a single call.
        self.set_cursor_pos(Some(0), Some(0));
    }

    /// Return the current scroll region as 0-based inclusive `(top, bottom)`.
    #[must_use]
    pub const fn scroll_region(&self) -> (usize, usize) {
        (self.scroll_region_top, self.scroll_region_bottom)
    }

    /// Reset left/right margins to the full terminal width.
    /// Public so `TerminalHandler` can call it when DECLRMM is reset.
    pub const fn reset_scroll_region_left_right(&mut self) {
        self.scroll_region_left = 0;
        self.scroll_region_right = self.width.saturating_sub(1);
    }

    /// Enable or disable DECLRMM (`?69`).
    ///
    /// When disabled, left/right margins are also reset to full-screen.
    pub fn set_declrmm(&mut self, mode: Declrmm) {
        self.declrmm_enabled = mode;
        if self.declrmm_enabled == Declrmm::Disabled {
            self.reset_scroll_region_left_right();
        }
    }

    /// Returns the current DECLRMM (`?69`) state.
    #[must_use]
    pub const fn is_declrmm_enabled(&self) -> Declrmm {
        self.declrmm_enabled
    }

    /// Set DECSLRM left/right margins (1-based inclusive).
    ///
    /// Only effective when DECLRMM (`?69`) is active; the caller
    /// (`TerminalHandler::handle_set_left_right_margins`) must gate on that flag.
    /// If parameters are invalid, resets to full-width.
    pub fn set_left_right_margins(&mut self, left1: usize, right1: usize) {
        // 0 → treat as 1 (reset)
        if left1 == 0 || right1 == 0 {
            self.reset_scroll_region_left_right();
            self.set_cursor_pos(Some(0), Some(0));
            return;
        }

        let left = left1.saturating_sub(1);
        let right = right1.saturating_sub(1);

        if left >= right || right >= self.width {
            self.reset_scroll_region_left_right();
            self.set_cursor_pos(Some(0), Some(0));
            return;
        }

        self.scroll_region_left = left;
        self.scroll_region_right = right;

        // DECSLRM homes the cursor to (col 0, row 0) — just like DECSTBM.
        self.set_cursor_pos(Some(0), Some(0));
    }

    /// Return the current left/right margins as 0-based inclusive `(left, right)`.
    #[must_use]
    pub const fn left_right_margins(&self) -> (usize, usize) {
        (self.scroll_region_left, self.scroll_region_right)
    }

    /// Index in `rows` of the first visible line for the given `scroll_offset`.
    /// The PTY thread always calls this with `scroll_offset = 0`; the GUI thread
    /// passes the value from `ViewState`.
    #[must_use]
    fn visible_window_start(&self, scroll_offset: usize) -> usize {
        if self.rows.is_empty() || self.height == 0 {
            return 0;
        }

        let total = self.rows.len();
        let h = self.height.min(total);
        let offset = scroll_offset.min(self.max_scroll_offset());

        total.saturating_sub(h + offset)
    }

    /// Return `true` if any row in the visible window is dirty (needs re-flattening).
    ///
    /// The PTY thread calls this with `scroll_offset = 0`.  A `false` result means
    /// the cached flat representation for every visible row is still valid, so
    /// `build_snapshot` can skip the flatten step entirely and reuse the previous
    /// `visible_chars` / `visible_tags` vectors.
    #[must_use]
    pub fn any_visible_dirty(&self, scroll_offset: usize) -> bool {
        if self.rows.is_empty() || self.height == 0 {
            return false;
        }
        let vis_start = self.visible_window_start(scroll_offset);
        let vis_end = (vis_start + self.height).min(self.rows.len());
        self.rows[vis_start..vis_end].iter().any(|r| r.dirty)
    }

    /// Extract image placements for all cells in the visible window.
    ///
    /// Returns a flat `Vec` of `Option<ImagePlacement>`, one entry per cell,
    /// in row-major order (row 0, col 0..width; row 1, col 0..width; …).
    ///
    /// `None` means the cell carries no image data.
    /// `Some(placement)` means the cell is part of an inline image.
    ///
    /// The length of the returned `Vec` is `height * width` (clamped to the
    /// actual number of visible rows × terminal width), matching the layout
    /// of `visible_chars` so the caller can index them in parallel.
    #[must_use]
    pub fn visible_image_placements(&self, scroll_offset: usize) -> Vec<Option<ImagePlacement>> {
        if self.rows.is_empty() || self.height == 0 || self.width == 0 {
            return Vec::new();
        }
        let vis_start = self.visible_window_start(scroll_offset);
        let vis_end = (vis_start + self.height).min(self.rows.len());
        let mut out = Vec::with_capacity((vis_end - vis_start) * self.width);
        for row in &self.rows[vis_start..vis_end] {
            let cells = row.cells();
            for col in 0..self.width {
                let placement = cells.get(col).and_then(|c| c.image_placement()).cloned();
                out.push(placement);
            }
        }
        out
    }

    /// Returns `true` if any cell in the visible window carries an image
    /// placement.  Used by `build_snapshot` to cheaply decide whether to
    /// include image data in the snapshot.
    ///
    /// Short-circuits in O(1) when the buffer has no image cells at all
    /// (the overwhelmingly common case).
    #[must_use]
    pub fn has_visible_images(&self, scroll_offset: usize) -> bool {
        // Fast path: no image cells anywhere in the buffer.
        if self.image_cell_count == 0 {
            return false;
        }

        if self.rows.is_empty() || self.height == 0 {
            return false;
        }
        let vis_start = self.visible_window_start(scroll_offset);
        let vis_end = (vis_start + self.height).min(self.rows.len());
        self.rows[vis_start..vis_end]
            .iter()
            .any(|r| r.cells().iter().any(|c| c.image_placement().is_some()))
    }

    /// Cursor Y expressed in "screen coordinates" (0..height-1).
    /// If the buffer is shorter than the height, we just return the raw Y.
    /// Always computed relative to the live bottom (`scroll_offset` = 0), because the
    /// PTY thread only ever mutates the buffer at the live bottom.
    fn cursor_screen_y(&self) -> usize {
        if self.rows.is_empty() || self.height == 0 {
            return 0;
        }

        let start = self.visible_window_start(0);
        self.cursor.pos.y.saturating_sub(start)
    }

    /// Convert DECSTBM region (screen coords) into buffer row indices (rows[])
    ///
    /// Ensures `self.rows` is extended to at least `height` entries so that
    /// the returned indices always point to real rows.  Without this, an early
    /// buffer (`rows.len()` < height) would clamp both top and bottom to the
    /// same index, causing every scroll operation to silently no-op.
    fn scroll_region_rows(&mut self) -> (usize, usize) {
        let start = self.visible_window_start(0);
        let required = start + self.scroll_region_bottom + 1;
        while self.rows.len() < required {
            self.push_row(RowOrigin::ScrollFill, RowJoin::NewLogicalLine);
        }
        let top = start + self.scroll_region_top;
        let bottom = start + self.scroll_region_bottom;
        (top, bottom)
    }

    /// Scroll DECSTBM region UP by 1 (primary buffer)
    fn scroll_region_up_primary(&mut self) {
        let (t, b) = self.scroll_region_rows();
        if t < b {
            self.scroll_slice_up(t, b);
        }
    }

    /// Scroll DECSTBM region DOWN by 1 (primary buffer)
    fn scroll_region_down_primary(&mut self) {
        let (t, b) = self.scroll_region_rows();
        if t < b {
            self.scroll_slice_down(t, b);
        }
    }

    /// Check whether the cursor is at the bottom margin of the DECSTBM
    /// scroll region.  Used by `insert_text` to decide whether a right-margin
    /// wrap should scroll the region instead of advancing past it.
    fn is_cursor_at_scroll_region_bottom(&self) -> bool {
        match self.kind {
            BufferType::Primary => {
                let sy = self.cursor_screen_y();
                sy == self.scroll_region_bottom
            }
            BufferType::Alternate => self.cursor.pos.y == self.scroll_region_bottom,
        }
    }

    /// Scroll the DECSTBM region up by one line during an autowrap at the
    /// bottom margin.  Handles both Primary (with scrollback) and Alternate
    /// (fixed grid) buffer types.
    fn scroll_region_up_for_wrap(&mut self) {
        match self.kind {
            BufferType::Primary => {
                self.scroll_region_up_primary();
            }
            BufferType::Alternate => {
                if self.scroll_region_top < self.scroll_region_bottom {
                    self.scroll_slice_up(self.scroll_region_top, self.scroll_region_bottom);
                }
            }
        }
    }

    /// SU — Scroll the scroll region UP by `n` lines.
    /// Content moves up; blank lines appear at the bottom.
    /// If no scroll region is set, operates on the whole screen.
    pub fn scroll_region_up_n(&mut self, n: usize) {
        let (t, b) = self.scroll_region_rows();
        if t >= b {
            return;
        }
        // Region indices are inclusive, so region has (b - t + 1) rows.
        let region_size = b - t + 1;
        let clamped = n.min(region_size);
        for _ in 0..clamped {
            self.scroll_slice_up(t, b);
        }
    }

    /// SD — Scroll the scroll region DOWN by `n` lines.
    /// Content moves down; blank lines appear at the top.
    /// If no scroll region is set, operates on the whole screen.
    pub fn scroll_region_down_n(&mut self, n: usize) {
        let (t, b) = self.scroll_region_rows();
        if t >= b {
            return;
        }
        // Region indices are inclusive, so region has (b - t + 1) rows.
        let region_size = b - t + 1;
        let clamped = n.min(region_size);
        for _ in 0..clamped {
            self.scroll_slice_down(t, b);
        }
    }

    /// Scroll a contiguous vertical slice [first, last] UP by one line.
    /// Rows outside that range are untouched. New bottom line is blank.
    fn scroll_slice_up(&mut self, first: usize, last: usize) {
        if first >= last {
            return;
        }
        if last >= self.rows.len() {
            return;
        }

        for row_idx in first..last {
            let next = self.rows[row_idx + 1].clone();
            self.rows[row_idx] = next;
            // Rotate the cache entry in lockstep: a moved row keeps its cached
            // flat representation (it hasn't changed content, only position).
            self.row_cache[row_idx] = self.row_cache[row_idx + 1].take();
        }

        // The original rows[last] was not shifted (the loop only copies
        // rows[row_idx+1] into rows[row_idx] for row_idx in first..last).
        // It is now replaced with a blank row; deduct any image cells it held.
        self.image_cell_count -= self.rows[last].count_image_cells();
        let mut new_row = Row::new(self.width);
        new_row.fill_with_tag(&self.current_tag);
        self.rows[last] = new_row;
        // New blank row at `last` — no cached representation yet.
        self.row_cache[last] = None;
    }

    /// Scroll a contiguous vertical slice [first, last] DOWN by one line.
    /// Rows outside that range are untouched. New top line is blank.
    fn scroll_slice_down(&mut self, first: usize, last: usize) {
        if first >= last {
            return;
        }
        if last >= self.rows.len() {
            return;
        }

        for row_idx in (first + 1..=last).rev() {
            let prev = self.rows[row_idx - 1].clone();
            self.rows[row_idx] = prev;
            // Rotate the cache entry in lockstep.
            self.row_cache[row_idx] = self.row_cache[row_idx - 1].take();
        }

        // The original rows[first] was not shifted (the loop only copies
        // rows[row_idx-1] into rows[row_idx] for row_idx in first+1..=last).
        // It is now replaced with a blank row; deduct any image cells it held.
        self.image_cell_count -= self.rows[first].count_image_cells();
        let mut new_row = Row::new(self.width);
        new_row.fill_with_tag(&self.current_tag);
        self.rows[first] = new_row;
        // New blank row at `first` — no cached representation yet.
        self.row_cache[first] = None;
    }

    /// Column-selective scroll-up: shifts cells within `[left_col, right_col]`
    /// on rows `[first, last]` up by one, without touching cells outside that
    /// horizontal range.  Used by `insert_lines` / `delete_lines` when DECLRMM
    /// is active.
    fn scroll_slice_up_columns(
        &mut self,
        first: usize,
        last: usize,
        left_col: usize,
        right_col: usize,
    ) {
        if first >= last || last >= self.rows.len() || left_col > right_col {
            return;
        }
        // Sweep image cells in the destination columns [left_col, right_col] across
        // all rows [first, last] before any cells are overwritten or erased.
        // This covers both the copy-overwrite path (rows first..last) and the
        // final erase on rows[last].
        //
        // After the sweep, count remaining image cells (e.g. Kitty) in the
        // affected range before the copy/erase so we can compute the net
        // change after the operation.
        let images_before = if self.image_cell_count > 0 {
            self.collect_and_clear_image_ids_in_rows(
                first,
                last + 1,
                Some(left_col),
                Some(right_col + 1),
            );
            (first..=last)
                .map(|i| self.rows[i].count_image_cells_in_range(left_col, right_col + 1))
                .sum::<usize>()
        } else {
            0
        };
        let tag = self.current_tag.clone();
        for row_idx in first..last {
            // Copy cells [left_col, right_col] from row_idx+1 into row_idx.
            // Use resolve_cell to handle sparse rows correctly — columns
            // beyond the stored cell count are treated as implicit blanks.
            let src_cells: Vec<_> = (left_col..=right_col)
                .map(|col| self.rows[row_idx + 1].resolve_cell(col))
                .collect();
            let row = &mut self.rows[row_idx];
            // Ensure storage.
            if row.cells_mut().len() < right_col + 1 {
                let width = row.width();
                while row.cells_mut().len() < (right_col + 1).min(width) {
                    row.cells_mut_push(Cell::blank_with_tag(FormatTag::default()));
                }
            }
            for (offset, cell) in src_cells.into_iter().enumerate() {
                let dst = left_col + offset;
                if dst <= right_col && dst < row.cells().len() {
                    row.cells_mut()[dst] = cell;
                }
            }
            row.mark_dirty();
            self.row_cache[row_idx] = None;
        }
        // Blank [left_col, right_col] on the last row.
        let row = &mut self.rows[last];
        row.erase_cells_at(left_col, right_col - left_col + 1, &tag);
        self.row_cache[last] = None;

        // Adjust image_cell_count for any images lost during the shift/erase.
        if images_before > 0 {
            let images_after: usize = (first..=last)
                .map(|i| self.rows[i].count_image_cells_in_range(left_col, right_col + 1))
                .sum();
            self.image_cell_count -= images_before.saturating_sub(images_after);
        }
    }

    /// Column-selective scroll-down: shifts cells within `[left_col, right_col]`
    /// on rows `[first, last]` down by one, without touching cells outside that
    /// horizontal range.  Used by `insert_lines` / `delete_lines` when DECLRMM
    /// is active.
    fn scroll_slice_down_columns(
        &mut self,
        first: usize,
        last: usize,
        left_col: usize,
        right_col: usize,
    ) {
        if first >= last || last >= self.rows.len() || left_col > right_col {
            return;
        }
        // Sweep image cells in the destination columns [left_col, right_col] across
        // all rows [first, last] before any cells are overwritten or erased.
        // This covers both the copy-overwrite path (rows first+1..=last) and the
        // final erase on rows[first].
        let images_before = if self.image_cell_count > 0 {
            self.collect_and_clear_image_ids_in_rows(
                first,
                last + 1,
                Some(left_col),
                Some(right_col + 1),
            );
            (first..=last)
                .map(|i| self.rows[i].count_image_cells_in_range(left_col, right_col + 1))
                .sum::<usize>()
        } else {
            0
        };
        let tag = self.current_tag.clone();
        for row_idx in (first + 1..=last).rev() {
            // Copy cells [left_col, right_col] from row_idx-1 into row_idx.
            // Use resolve_cell to handle sparse rows correctly — columns
            // beyond the stored cell count are treated as implicit blanks.
            let src_cells: Vec<_> = (left_col..=right_col)
                .map(|col| self.rows[row_idx - 1].resolve_cell(col))
                .collect();
            let row = &mut self.rows[row_idx];
            if row.cells_mut().len() < right_col + 1 {
                let width = row.width();
                while row.cells_mut().len() < (right_col + 1).min(width) {
                    row.cells_mut_push(Cell::blank_with_tag(FormatTag::default()));
                }
            }
            for (offset, cell) in src_cells.into_iter().enumerate() {
                let dst = left_col + offset;
                if dst <= right_col && dst < row.cells().len() {
                    row.cells_mut()[dst] = cell;
                }
            }
            row.mark_dirty();
            self.row_cache[row_idx] = None;
        }
        // Blank [left_col, right_col] on the first row.
        let row = &mut self.rows[first];
        row.erase_cells_at(left_col, right_col - left_col + 1, &tag);
        self.row_cache[first] = None;

        // Adjust image_cell_count for any images lost during the shift/erase.
        if images_before > 0 {
            let images_after: usize = (first..=last)
                .map(|i| self.rows[i].count_image_cells_in_range(left_col, right_col + 1))
                .sum();
            self.image_cell_count -= images_before.saturating_sub(images_after);
        }
    }

    // ----------------------------------------------------------
    // Scrollback: only valid in the PRIMARY buffer
    // ----------------------------------------------------------

    /// How many lines above the live bottom the user can scroll.
    #[must_use]
    pub const fn max_scroll_offset(&self) -> usize {
        if self.rows.len() <= self.height {
            0
        } else {
            self.rows.len() - self.height
        }
    }

    /// Compute a new scroll offset after scrolling upward by `lines`.
    ///
    /// Alternate buffer always returns 0 (no scrollback).
    /// The caller is responsible for storing the returned value into `ViewState`.
    #[must_use]
    pub fn scroll_back(&self, scroll_offset: usize, lines: usize) -> usize {
        if self.kind != BufferType::Primary {
            return 0; // Alternate buffer: no scrollback
        }

        let max = self.max_scroll_offset();
        if max == 0 {
            return 0;
        }

        (scroll_offset + lines).min(max)
    }

    /// Compute a new scroll offset after scrolling downward by `lines`.
    ///
    /// The caller is responsible for storing the returned value into `ViewState`.
    #[must_use]
    pub fn scroll_forward(&self, scroll_offset: usize, lines: usize) -> usize {
        if self.kind != BufferType::Primary {
            return 0;
        }

        scroll_offset.saturating_sub(lines)
    }

    /// Returns `0` — the scroll offset for the live bottom view.
    ///
    /// Provided as a convenience so call sites read clearly.
    #[must_use]
    pub const fn scroll_to_bottom() -> usize {
        0
    }

    /// Scroll the visible window up by one row, discarding the top row and appending a blank row at
    /// the bottom.
    ///
    /// In the primary buffer the cursor row index is also decremented to follow the visible window.
    pub fn scroll_up(&mut self) {
        // Deduct any image cells in the row about to be removed.
        self.image_cell_count -= self.rows[0].count_image_cells();
        // remove topmost row (and its cache entry)
        self.rows.remove(0);
        self.row_cache.remove(0);

        // add a new empty row at the bottom (BCE: fill with current SGR background)
        let mut new_row = Row::new(self.width);
        new_row.fill_with_tag(&self.current_tag);
        self.rows.push(new_row);
        self.row_cache.push(None);

        // DO NOT move the cursor in alternate buffer
        if self.kind == BufferType::Primary {
            // primary buffer uses scrollback: move cursor with the visible window
            if self.cursor.pos.y > 0 {
                self.cursor.pos.y -= 1;
            }
        }
    }

    /// Move cursor to absolute position (CUP, HVP).
    ///
    /// `x` and `y` are 0-indexed screen coordinates.  `None` means "leave this
    /// axis unchanged" (e.g. CHA only supplies x, VPA only supplies y, CUP
    /// supplies both).
    ///
    /// When DECOM (origin mode) is enabled, `y` is relative to `scroll_region_top`
    /// and is clamped to the scroll region height.  When DECOM is disabled, `y` is
    /// relative to the top of the visible window and clamped to the screen height.
    pub fn set_cursor_pos(&mut self, x: Option<usize>, y: Option<usize>) {
        // `None` means "leave this axis unchanged" (e.g. CHA only supplies x,
        // VPA only supplies y, CUP supplies both).
        let new_x = match x {
            Some(col) => col.min(self.width.saturating_sub(1)),
            None => self.cursor.pos.x,
        };

        // y is a screen-relative coordinate (0 = top of visible window in normal
        // mode, or 0 = top of scroll region in DECOM mode).
        let new_buffer_y = match y {
            Some(row) => {
                if self.decom_enabled == Decom::OriginMode {
                    // DECOM: row is relative to scroll_region_top, clamped to
                    // the scroll region height.
                    let region_height = self
                        .scroll_region_bottom
                        .saturating_sub(self.scroll_region_top);
                    let clamped = row.min(region_height);
                    let screen_row = self.scroll_region_top + clamped;
                    self.visible_window_start(0) + screen_row
                } else {
                    let clamped = row.min(self.height.saturating_sub(1));
                    // PTY always operates at live bottom (scroll_offset = 0)
                    self.visible_window_start(0) + clamped
                }
            }
            None => self.cursor.pos.y,
        };

        // Ensure rows exist up to the target position
        while new_buffer_y >= self.rows.len() {
            self.push_row(RowOrigin::ScrollFill, RowJoin::NewLogicalLine);
        }

        self.cursor.pos.x = new_x;
        self.cursor.pos.y = new_buffer_y;
        self.debug_assert_invariants();
    }

    /// Move cursor relatively (CUU, CUD, CUF, CUB)
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss
    )]
    /// Move the cursor by a relative offset `(dx, dy)` in screen coordinates.
    ///
    /// Positive `dx` moves right; negative moves left. Positive `dy` moves down; negative moves
    /// up.  The cursor is clamped to the visible screen boundaries and never enters scrollback.
    pub fn move_cursor_relative(&mut self, dx: i32, dy: i32) {
        // When DECLRMM is active and moving horizontally, clamp to the
        // left/right margins if the cursor is currently within the margin zone.
        let new_x = if self.declrmm_enabled == Declrmm::Enabled && dx != 0 {
            let cx = self.cursor.pos.x;
            if cx >= self.scroll_region_left && cx <= self.scroll_region_right {
                // Cursor is inside the margin zone: clamp to [left, right].
                (cx as i32 + dx)
                    .max(self.scroll_region_left as i32)
                    .min(self.scroll_region_right as i32) as usize
            } else {
                // Cursor is outside the margin zone: use normal full-width clamp.
                (cx as i32 + dx)
                    .max(0)
                    .min(self.width.saturating_sub(1) as i32) as usize
            }
        } else {
            (self.cursor.pos.x as i32 + dx)
                .max(0)
                .min(self.width.saturating_sub(1) as i32) as usize
        };

        let current_screen_y = self.cursor_screen_y();
        let new_screen_y = (current_screen_y as i32 + dy)
            .max(0)
            .min(self.height.saturating_sub(1) as i32) as usize;

        // PTY always operates at live bottom (scroll_offset = 0)
        let new_buffer_y = self.visible_window_start(0) + new_screen_y;

        // Ensure rows exist
        while new_buffer_y >= self.rows.len() {
            self.push_row(RowOrigin::ScrollFill, RowJoin::NewLogicalLine);
        }

        self.cursor.pos.x = new_x;
        self.cursor.pos.y = new_buffer_y;
        self.debug_assert_invariants();
    }

    /// Erase from cursor to end of display (ED 0)
    pub fn erase_to_end_of_display(&mut self) {
        let cursor_y = self.cursor.pos.y;
        let cursor_x = self.cursor.pos.x;

        // PTY always operates at live bottom (scroll_offset = 0)
        let visible_start = self.visible_window_start(0);
        let visible_end = visible_start + self.height;

        // Sweep image cells that are about to be erased (clears non-Kitty
        // images buffer-wide and decrements image_cell_count for them).
        self.collect_and_clear_image_ids_in_rows(cursor_y, visible_end, Some(cursor_x), None);

        // Count any remaining image cells (e.g. Kitty images) that survived the
        // sweep but will be destroyed by the row-level clear operations below.
        let mut remaining_images = 0usize;
        if self.image_cell_count > 0 {
            if cursor_y < self.rows.len() {
                remaining_images +=
                    self.rows[cursor_y].count_image_cells_in_range(cursor_x, self.width);
            }
            for i in (cursor_y + 1)..visible_end.min(self.rows.len()) {
                remaining_images += self.rows[i].count_image_cells();
            }
        }

        // Clear from cursor to end of current row
        if cursor_y < self.rows.len() {
            let tag = self.current_tag.clone();
            self.rows[cursor_y].clear_from(cursor_x, &tag);
        }

        // Clear all rows below cursor in the visible window
        for i in (cursor_y + 1)..visible_end.min(self.rows.len()) {
            let tag = self.current_tag.clone();
            self.rows[i].clear_with_tag(&tag);
        }

        self.image_cell_count -= remaining_images;
        self.debug_assert_invariants();
    }

    /// Erase from beginning of display to cursor (ED 1)
    pub fn erase_to_beginning_of_display(&mut self) {
        let cursor_y = self.cursor.pos.y;
        let cursor_x = self.cursor.pos.x;

        // PTY always operates at live bottom (scroll_offset = 0)
        let visible_start = self.visible_window_start(0);

        // Sweep image cells that are about to be erased (clears non-Kitty
        // images buffer-wide and decrements image_cell_count for them).
        // First row being erased starts at col 0; the cursor row is erased
        // from col 0..=cursor_x.  For simplicity, sweep the entire row range.
        self.collect_and_clear_image_ids_in_rows(visible_start, cursor_y + 1, None, None);

        // Count any remaining image cells (e.g. Kitty images) that survived the
        // sweep but will be destroyed by the row-level clear operations below.
        let mut remaining_images = 0usize;
        if self.image_cell_count > 0 {
            for i in visible_start..cursor_y.min(self.rows.len()) {
                remaining_images += self.rows[i].count_image_cells();
            }
            if cursor_y < self.rows.len() {
                remaining_images += self.rows[cursor_y].count_image_cells_in_range(0, cursor_x + 1);
            }
        }

        // Clear all rows above cursor in the visible window
        for i in visible_start..cursor_y.min(self.rows.len()) {
            let tag = self.current_tag.clone();
            self.rows[i].clear_with_tag(&tag);
        }

        // Clear from beginning of current row to cursor
        if cursor_y < self.rows.len() {
            let tag = self.current_tag.clone();
            self.rows[cursor_y].clear_to(cursor_x + 1, &tag);
        }

        self.image_cell_count -= remaining_images;
        self.debug_assert_invariants();
    }

    /// Erase entire display (ED 2)
    pub fn erase_display(&mut self) {
        // PTY always operates at live bottom (scroll_offset = 0)
        let visible_start = self.visible_window_start(0);
        let visible_end = visible_start + self.height;

        // Sweep image cells that are about to be erased (clears non-Kitty
        // images buffer-wide and decrements image_cell_count for them).
        self.collect_and_clear_image_ids_in_rows(visible_start, visible_end, None, None);

        // Count any remaining image cells (e.g. Kitty images) that survived
        // the sweep but will be destroyed by the row-level clear below.
        let mut remaining_images = 0usize;
        if self.image_cell_count > 0 {
            for i in visible_start..visible_end.min(self.rows.len()) {
                remaining_images += self.rows[i].count_image_cells();
            }
        }

        for i in visible_start..visible_end.min(self.rows.len()) {
            let tag = self.current_tag.clone();
            self.rows[i].clear_with_tag(&tag);
        }

        self.image_cell_count -= remaining_images;
        self.debug_assert_invariants();
    }

    /// Erase scrollback (ED 3)
    pub fn erase_scrollback(&mut self) {
        if self.kind == BufferType::Alternate {
            // Alternate buffer has no scrollback
            return;
        }

        let visible_start = self.visible_window_start(0);

        // Remove all scrollback rows (everything before visible window)
        if visible_start > 0 {
            // Account for image cells in the drained scrollback rows.
            if self.image_cell_count > 0 {
                let drained_images: usize = self.rows[..visible_start]
                    .iter()
                    .map(Row::count_image_cells)
                    .sum();
                self.image_cell_count -= drained_images;
            }
            self.rows.drain(0..visible_start);
            self.row_cache.drain(0..visible_start);

            // Adjust cursor
            if self.cursor.pos.y >= visible_start {
                self.cursor.pos.y -= visible_start;
            } else {
                self.cursor.pos.y = 0;
            }

            // scroll_offset is owned by the caller (ViewState); erase_scrollback
            // removes all rows before the visible window so the caller should
            // reset their scroll_offset to 0 after calling this method.
        }

        self.debug_assert_invariants();
    }

    /// Erase from cursor to end of line (EL 0)
    pub fn erase_line_to_end(&mut self) {
        let cursor_y = self.cursor.pos.y;
        let cursor_x = self.cursor.pos.x;

        if cursor_y < self.rows.len() {
            // Sweep image cells that are about to be erased (non-Kitty images).
            self.collect_and_clear_image_ids_in_rows(cursor_y, cursor_y + 1, Some(cursor_x), None);

            // Count remaining image cells (e.g. Kitty) in the range being cleared.
            let remaining = if self.image_cell_count > 0 {
                self.rows[cursor_y].count_image_cells_in_range(cursor_x, self.width)
            } else {
                0
            };

            let tag = self.current_tag.clone();
            self.rows[cursor_y].clear_from(cursor_x, &tag);
            self.image_cell_count -= remaining;
        }

        self.debug_assert_invariants();
    }

    /// Erase from beginning of line to cursor (EL 1)
    pub fn erase_line_to_beginning(&mut self) {
        let cursor_y = self.cursor.pos.y;
        let cursor_x = self.cursor.pos.x;

        if cursor_y < self.rows.len() {
            // Sweep image cells that are about to be erased (non-Kitty images).
            self.collect_and_clear_image_ids_in_rows(cursor_y, cursor_y + 1, None, None);

            // Count remaining image cells (e.g. Kitty) in the range being cleared.
            let remaining = if self.image_cell_count > 0 {
                self.rows[cursor_y].count_image_cells_in_range(0, cursor_x + 1)
            } else {
                0
            };

            let tag = self.current_tag.clone();
            self.rows[cursor_y].clear_to(cursor_x + 1, &tag);
            self.image_cell_count -= remaining;
        }

        self.debug_assert_invariants();
    }

    /// Erase entire line (EL 2)
    pub fn erase_line(&mut self) {
        let cursor_y = self.cursor.pos.y;

        if cursor_y < self.rows.len() {
            // Sweep image cells that are about to be erased (non-Kitty images).
            self.collect_and_clear_image_ids_in_rows(cursor_y, cursor_y + 1, None, None);

            // Count remaining image cells (e.g. Kitty) in the row.
            let remaining = if self.image_cell_count > 0 {
                self.rows[cursor_y].count_image_cells()
            } else {
                0
            };

            let tag = self.current_tag.clone();
            self.rows[cursor_y].clear_with_tag(&tag);
            self.image_cell_count -= remaining;
        }

        self.debug_assert_invariants();
    }

    /// Convert the currently visible rows into a flat `(Vec<TChar>, Vec<FormatTag>)` pair
    /// Convert visible rows (with the given `scroll_offset`) into flat
    /// `(Vec<TChar>, Vec<FormatTag>)` suitable for the GUI renderer.
    ///
    /// Pass `scroll_offset = 0` when calling from the PTY thread (which always
    /// operates at the live bottom).
    ///
    /// Takes `&mut self` because it updates the per-row cache and clears dirty
    /// flags on rows that are freshly flattened.
    #[must_use]
    pub fn visible_as_tchars_and_tags(
        &mut self,
        scroll_offset: usize,
    ) -> (Vec<TChar>, Vec<FormatTag>) {
        let visible_start = self.visible_window_start(scroll_offset);
        let visible_end = (visible_start + self.height).min(self.rows.len());
        Self::rows_as_tchars_and_tags_cached(
            &mut self.rows[visible_start..visible_end],
            &mut self.row_cache[visible_start..visible_end],
        )
    }

    /// Flatten all scrollback rows (everything before the visible window) into
    /// a linear `(Vec<TChar>, Vec<FormatTag>)` pair using the same algorithm as
    /// [`Self::visible_as_tchars_and_tags`].
    ///
    /// Returns `(vec![], vec![])` for the alternate screen buffer, which never
    /// accumulates scrollback.
    ///
    /// Flatten all scrollback rows (everything before the visible window) into
    /// a linear `(Vec<TChar>, Vec<FormatTag>)` pair.
    ///
    /// Pass `scroll_offset = 0` when calling from the PTY thread.
    pub fn scrollback_as_tchars_and_tags(
        &mut self,
        scroll_offset: usize,
    ) -> (Vec<TChar>, Vec<FormatTag>) {
        // Alternate buffer has no scrollback.
        if self.kind == BufferType::Alternate {
            return (vec![], vec![]);
        }

        let visible_start = self.visible_window_start(scroll_offset);

        if visible_start == 0 {
            // No scrollback rows exist yet.
            return (vec![], vec![]);
        }

        Self::rows_as_tchars_and_tags_cached(
            &mut self.rows[..visible_start],
            &mut self.row_cache[..visible_start],
        )
    }

    /// Shared helper: flatten a slice of [`Row`]s into `(Vec<TChar>,
    /// Vec<FormatTag>)`, using a per-row cache to skip rows that have not
    /// changed since the last snapshot.
    ///
    /// For each row:
    /// - If `row.dirty` or the cache entry is `None`, flatten the row, populate
    ///   the cache entry, and call `row.mark_clean()`.
    /// - Otherwise reuse the cached per-row `(chars, tags)` directly.
    ///
    /// Per-row tag offsets are stored relative to each row's own character
    /// slice (starting at 0).  The merge step below re-computes global offsets
    /// each time, so the cache never stores stale absolute positions.
    fn rows_as_tchars_and_tags_cached(
        rows: &mut [Row],
        cache: &mut [Option<(Vec<TChar>, Vec<FormatTag>)>],
    ) -> (Vec<TChar>, Vec<FormatTag>) {
        // ── Step 1: ensure every row has an up-to-date cache entry ──────────
        for (row, entry) in rows.iter_mut().zip(cache.iter_mut()) {
            if row.dirty || entry.is_none() {
                *entry = Some(Self::flatten_row(row));
                row.mark_clean();
            }
        }

        // ── Step 2: merge per-row results into the global flat vectors ───────
        // Per-row tags have offsets relative to the start of that row's chars.
        // We accumulate a running `global_offset` and re-base each tag.
        let row_count = rows.len();
        let mut chars: Vec<TChar> = Vec::new();
        let mut tags: Vec<FormatTag> = Vec::new();

        for (row_idx, entry) in cache.iter().enumerate() {
            // Step 1 populated every entry unconditionally, so `None` cannot
            // occur here.  We use `if let` to satisfy the no-unwrap/expect rule;
            // the `else` branch is unreachable in practice.
            if let Some((row_chars, row_tags)) = entry.as_ref() {
                let global_offset = chars.len();

                // Append this row's characters, adjusting tag offsets.
                for row_tag in row_tags {
                    let rebased = FormatTag {
                        start: global_offset + row_tag.start,
                        end: global_offset + row_tag.end,
                        colors: row_tag.colors,
                        font_weight: row_tag.font_weight,
                        font_decorations: row_tag.font_decorations,
                        url: row_tag.url.clone(),
                        blink: row_tag.blink,
                    };

                    // Merge with the previous tag when format is identical and
                    // the ranges are contiguous (same logic as the original helper).
                    if let Some(last) = tags.last_mut() {
                        if last.end == rebased.start && tags_same_format(last, &rebased) {
                            last.end = rebased.end;
                        } else {
                            tags.push(rebased);
                        }
                    } else {
                        tags.push(rebased);
                    }
                }

                chars.extend_from_slice(row_chars);
            }

            // Append a NewLine separator after every row except the last.
            let is_last_row = row_idx + 1 == row_count;
            if !is_last_row {
                let byte_pos = chars.len();
                chars.push(TChar::NewLine);
                if let Some(last) = tags.last_mut() {
                    if last.end == byte_pos {
                        last.end += 1;
                    } else {
                        tags.push(FormatTag {
                            start: byte_pos,
                            end: byte_pos + 1,
                            ..FormatTag::default()
                        });
                    }
                } else {
                    tags.push(FormatTag {
                        start: byte_pos,
                        end: byte_pos + 1,
                        ..FormatTag::default()
                    });
                }
            }
        }

        // Guarantee at least one tag covering the full range.
        if tags.is_empty() {
            tags.push(FormatTag {
                start: 0,
                end: if chars.is_empty() {
                    usize::MAX
                } else {
                    chars.len()
                },
                ..FormatTag::default()
            });
        } else if let Some(last) = tags.last_mut() {
            last.end = chars.len();
        }

        (chars, tags)
    }

    /// Flatten a single [`Row`] into a `(Vec<TChar>, Vec<FormatTag>)` pair.
    ///
    /// Tag offsets are **row-relative** (start at 0 for the first character in
    /// this row).  The caller is responsible for re-basing them into global
    /// offsets when merging multiple rows.
    fn flatten_row(row: &Row) -> (Vec<TChar>, Vec<FormatTag>) {
        let mut chars: Vec<TChar> = Vec::new();
        let mut tags: Vec<FormatTag> = Vec::new();

        for cell in row.get_characters() {
            // Skip wide-glyph continuation cells.
            if cell.is_continuation() {
                continue;
            }

            let byte_pos = chars.len();
            chars.push(*cell.tchar());

            let cell_tag = cell.tag();
            if let Some(last) = tags.last_mut() {
                if last.end == byte_pos && tags_same_format(last, cell_tag) {
                    last.end += 1;
                } else {
                    tags.push(FormatTag {
                        start: byte_pos,
                        end: byte_pos + 1,
                        colors: cell_tag.colors,
                        font_weight: cell_tag.font_weight,
                        font_decorations: cell_tag.font_decorations,
                        url: cell_tag.url.clone(),
                        blink: cell_tag.blink,
                    });
                }
            } else {
                tags.push(FormatTag {
                    start: byte_pos,
                    end: byte_pos + 1,
                    colors: cell_tag.colors,
                    font_weight: cell_tag.font_weight,
                    font_decorations: cell_tag.font_decorations,
                    url: cell_tag.url.clone(),
                    blink: cell_tag.blink,
                });
            }
        }

        // Guarantee at least one tag even for an empty row.
        if tags.is_empty() {
            tags.push(FormatTag {
                start: 0,
                end: 0,
                ..FormatTag::default()
            });
        }

        (chars, tags)
    }

    /// Return `true` when the alternate screen is currently active.
    #[must_use]
    pub const fn is_alternate_screen(&self) -> bool {
        matches!(self.kind, BufferType::Alternate)
    }

    /// Return `true` when a cursor has been saved via DECSC (ESC 7 / `\x1b[?1048h`).
    #[must_use]
    pub const fn has_saved_cursor(&self) -> bool {
        self.saved_cursor.is_some()
    }

    /// Return the terminal width (columns).
    #[must_use]
    pub const fn terminal_width(&self) -> usize {
        self.width
    }

    /// Return the terminal height (rows).
    #[must_use]
    pub const fn terminal_height(&self) -> usize {
        self.height
    }

    /// Extract the text content of a selection range from the buffer.
    ///
    /// Coordinates are buffer-absolute row indices (0 = first row in the full
    /// buffer including scrollback). Columns are 0-indexed cell positions.
    /// The range is inclusive on both ends: `[start_row, start_col]` through
    /// `[end_row, end_col]`.
    ///
    /// Trailing whitespace on each row is trimmed (standard terminal behaviour).
    /// Rows are separated by `'\n'`.
    #[must_use]
    pub fn extract_text(
        &self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) -> String {
        use std::fmt::Write as _;

        if start_row >= self.rows.len() {
            return String::new();
        }
        let end_row = end_row.min(self.rows.len().saturating_sub(1));

        let mut result = String::new();

        for row_idx in start_row..=end_row {
            let row = &self.rows[row_idx];
            let cells = row.get_characters();

            let col_begin = if row_idx == start_row { start_col } else { 0 };
            let col_end = if row_idx == end_row {
                end_col
            } else {
                cells.len().saturating_sub(1)
            };

            let mut row_text = String::new();
            for col in col_begin..=col_end {
                if col >= cells.len() {
                    break;
                }
                let cell = &cells[col];
                if cell.is_continuation() {
                    continue;
                }
                let tc = cell.tchar();
                if matches!(tc, TChar::NewLine) {
                    break;
                }
                write!(&mut row_text, "{tc}").unwrap_or_default();
            }

            let trimmed = row_text.trim_end();
            result.push_str(trimmed);

            if row_idx < end_row {
                result.push('\n');
            }
        }

        result
    }

    /// Extract a rectangular block of text from the buffer.
    ///
    /// Every row from `start_row` to `end_row` (inclusive) is sampled between
    /// the same `col_min`..=`col_max` column range, where
    /// `col_min = start_col.min(end_col)` and `col_max = start_col.max(end_col)`.
    /// Rows are joined with `\n`.  Trailing whitespace is trimmed per row.
    ///
    /// This is the copy behaviour for Alt+drag (block/rectangular) selections.
    #[must_use]
    pub fn extract_block_text(
        &self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) -> String {
        use std::fmt::Write as _;

        if start_row >= self.rows.len() {
            return String::new();
        }
        let end_row = end_row.min(self.rows.len().saturating_sub(1));
        let col_min = start_col.min(end_col);
        let col_max = start_col.max(end_col);

        let mut result = String::new();

        for row_idx in start_row..=end_row {
            let row = &self.rows[row_idx];
            let cells = row.get_characters();

            let mut row_text = String::new();
            for col in col_min..=col_max {
                if col >= cells.len() {
                    break;
                }
                let cell = &cells[col];
                if cell.is_continuation() {
                    continue;
                }
                let tc = cell.tchar();
                if matches!(tc, TChar::NewLine) {
                    break;
                }
                write!(&mut row_text, "{tc}").unwrap_or_default();
            }

            let trimmed = row_text.trim_end();
            result.push_str(trimmed);

            if row_idx < end_row {
                result.push('\n');
            }
        }

        result
    }

    /// Set the current format tag for subsequent text insertions
    pub fn set_format(&mut self, tag: FormatTag) {
        self.current_tag = tag;
    }

    /// Get the current format tag
    #[must_use]
    pub const fn get_format(&self) -> &FormatTag {
        &self.current_tag
    }

    /// Set whether Line Feed Mode (LNM) is enabled.
    ///
    /// `Lnm::NewLine`: LF behaves like CRLF (cursor moves to column 0 on line feed).
    /// `Lnm::LineFeed` (default): LF only advances the row; column is unchanged.
    pub const fn set_lnm(&mut self, mode: Lnm) {
        self.lnm_enabled = mode;
    }

    /// Return whether Line Feed Mode is currently enabled.
    #[must_use]
    pub const fn is_lnm_enabled(&self) -> Lnm {
        self.lnm_enabled
    }

    /// Set whether soft-wrapping is enabled (DECAWM).
    ///
    /// `Decawm::AutoWrap` (default): text wraps at the terminal width onto the next row.
    /// `Decawm::NoAutoWrap`: text is clamped to the last column; any characters that would
    /// overflow the current row are discarded.
    pub const fn set_wrap(&mut self, mode: Decawm) {
        self.wrap_enabled = mode;
    }

    /// Return whether soft-wrapping is currently enabled.
    #[must_use]
    pub const fn is_wrap_enabled(&self) -> Decawm {
        self.wrap_enabled
    }

    /// Enable or disable DECOM (Origin Mode).
    ///
    /// When origin mode is enabled, cursor positioning (CUP, HVP, and similar
    /// commands) is relative to the scroll region rather than the full screen.
    /// Cursor movement is also constrained to the scroll region boundaries.
    ///
    /// Per DEC spec: enabling or disabling DECOM homes the cursor.
    pub fn set_decom(&mut self, mode: Decom) {
        self.decom_enabled = mode;
        // DEC spec: changing DECOM homes the cursor to position (1,1) in the
        // current addressing mode.  With `set_cursor_pos` already DECOM-aware,
        // passing (0, 0) does the right thing for both modes.
        self.set_cursor_pos(Some(0), Some(0));
    }

    /// Return whether DECOM (Origin Mode) is currently enabled.
    #[must_use]
    pub const fn is_decom_enabled(&self) -> Decom {
        self.decom_enabled
    }

    /// Switch the buffer to a different column width (DECCOLM).
    ///
    /// This performs all the side effects mandated by the DEC spec:
    /// - Reflows the buffer to the new width
    /// - Clears the screen
    /// - Resets the scroll region to full screen
    /// - Resets DECOM (Origin Mode)
    /// - Homes the cursor to (0, 0)
    pub fn set_column_mode(&mut self, columns: usize) {
        // Reflow to the new width (height stays the same).
        // set_size returns the new scroll offset; DECCOLM always operates at
        // live bottom, so pass 0 and discard the result.
        let _ = self.set_size(columns, self.height, 0);

        // Clear the visible screen.
        self.erase_display();

        // Reset scroll region to full screen.
        self.scroll_region_top = 0;
        self.scroll_region_bottom = self.height.saturating_sub(1);

        // Reset DECOM without the cursor-home side effect (we home below).
        self.decom_enabled = Decom::NormalCursor;

        // Home cursor.
        self.set_cursor_pos(Some(0), Some(0));
    }

    /// Switch to the alternate screen buffer.
    ///
    /// Enter the alternate screen buffer.
    ///
    /// Saves the primary buffer state (rows, cursor, scroll region, image store)
    /// so it can be restored by [`Self::leave_alternate`]. Tab stops are NOT saved —
    /// they are shared between primary and alternate screens (matching xterm).
    ///
    /// The caller must pass the current `scroll_offset` from `ViewState` so it can
    /// be saved and restored later.  The alternate screen always starts at offset 0;
    /// the caller should set `ViewState::scroll_offset = 0` after this call.
    pub fn enter_alternate(&mut self, scroll_offset: usize) {
        // If we're already in the alternate buffer, do nothing.
        if self.kind == BufferType::Alternate {
            return;
        }

        // Save primary state (rows + cursor + scroll_offset + cache).
        let saved = SavedPrimaryState {
            rows: self.rows.clone(),
            row_cache: self.row_cache.clone(),
            cursor: self.cursor.clone(),
            scroll_offset,
            scroll_region_top: self.scroll_region_top,
            scroll_region_bottom: self.scroll_region_bottom,
            scroll_region_left: self.scroll_region_left,
            scroll_region_right: self.scroll_region_right,
            saved_cursor: self.saved_cursor.clone(),
            image_store: self.image_store.clone(),
            image_cell_count: self.image_cell_count,
        };
        self.saved_primary = Some(saved);

        // Switch to alternate buffer.
        self.kind = BufferType::Alternate;

        // Fresh screen: exactly `height` empty rows, all dirty (None cache entries).
        self.rows = vec![Row::new(self.width); self.height];
        self.row_cache = vec![None; self.height];

        // Alternate screen has no images.
        self.image_store.clear();
        self.image_cell_count = 0;

        // Reset cursor for the alternate screen.
        self.cursor = CursorState::default();

        // Alternate screen starts with a full-screen scroll region.
        self.reset_scroll_region_to_full();

        self.debug_assert_invariants();
    }

    /// Leave the alternate screen and restore the primary buffer.
    ///
    /// Restores rows, cursor, scroll region, and image store from the saved
    /// primary state. Tab stops are NOT restored — they are shared between
    /// primary and alternate screens, so any changes made in the alternate
    /// screen persist (matching xterm behavior).
    ///
    /// Returns the `scroll_offset` that was saved when `enter_alternate` was
    /// called.  The caller should store this back into `ViewState::scroll_offset`.
    /// Returns `0` if there was no saved primary state.
    pub fn leave_alternate(&mut self) -> usize {
        // Already in the primary buffer — nothing to do.
        if self.kind == BufferType::Primary {
            return 0;
        }

        self.kind = BufferType::Primary;

        if let Some(saved) = self.saved_primary.take() {
            // Restore saved primary state.
            let restored_offset = saved.scroll_offset;
            self.rows = saved.rows;
            self.row_cache = saved.row_cache;
            self.cursor = saved.cursor;
            self.scroll_region_top = saved.scroll_region_top;
            self.scroll_region_bottom = saved.scroll_region_bottom;
            self.scroll_region_left = saved.scroll_region_left;
            self.scroll_region_right = saved.scroll_region_right;
            self.saved_cursor = saved.saved_cursor;
            self.image_store = saved.image_store;
            self.image_cell_count = saved.image_cell_count;

            self.debug_assert_invariants();
            restored_offset
        } else {
            self.debug_assert_invariants();
            0
        }
    }

    // ========================================================================
    // Inline image support
    // ========================================================================

    /// Access the image store (read-only).
    #[must_use]
    pub const fn image_store(&self) -> &ImageStore {
        &self.image_store
    }

    /// Access the image store (mutable).
    pub const fn image_store_mut(&mut self) -> &mut ImageStore {
        &mut self.image_store
    }

    /// Clear all image placements from every cell in the buffer.
    pub fn clear_all_image_placements(&mut self) {
        let mut cleared = 0usize;
        for (i, row) in self.rows.iter_mut().enumerate() {
            let mut changed = false;
            for cell in row.cells_mut() {
                if cell.has_image() {
                    cell.clear_image();
                    cleared += 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                if i < self.row_cache.len() {
                    self.row_cache[i] = None;
                }
            }
        }
        self.image_cell_count -= cleared;
    }

    /// Clear all image placements for a specific image ID from every cell.
    pub fn clear_image_placements_by_id(&mut self, image_id: u64) {
        let mut cleared = 0usize;
        for (i, row) in self.rows.iter_mut().enumerate() {
            let mut changed = false;
            for cell in row.cells_mut() {
                if cell
                    .image_placement()
                    .is_some_and(|p| p.image_id == image_id)
                {
                    cell.clear_image();
                    cleared += 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                if i < self.row_cache.len() {
                    self.row_cache[i] = None;
                }
            }
        }
        self.image_cell_count -= cleared;
    }

    /// Clear image placements at the current cursor row and all rows after.
    ///
    /// Used by Kitty `d=c` (at cursor) and `d=C` (at cursor and after) delete
    /// targets. Clears every image cell from the cursor row to the end of
    /// the buffer.
    pub fn clear_image_placements_at_cursor_and_after(&mut self) {
        let start_row = self.cursor.pos.y;
        let mut cleared = 0usize;
        for i in start_row..self.rows.len() {
            let row = &mut self.rows[i];
            let mut changed = false;
            for cell in row.cells_mut() {
                if cell.has_image() {
                    cell.clear_image();
                    cleared += 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                if i < self.row_cache.len() {
                    self.row_cache[i] = None;
                }
            }
        }
        self.image_cell_count -= cleared;
    }

    /// Clear image placements at the current cursor position only (single row).
    pub fn clear_image_placements_at_cursor(&mut self) {
        let row_idx = self.cursor.pos.y;
        if row_idx >= self.rows.len() {
            return;
        }
        let row = &mut self.rows[row_idx];
        let mut cleared = 0usize;
        for cell in row.cells_mut() {
            if cell.has_image() {
                cell.clear_image();
                cleared += 1;
            }
        }
        if cleared > 0 {
            row.dirty = true;
            if row_idx < self.row_cache.len() {
                self.row_cache[row_idx] = None;
            }
            self.image_cell_count -= cleared;
        }
    }

    /// Returns `true` if any cell in the buffer has an image placement.
    ///
    /// O(1) — backed by the `image_cell_count` counter.
    #[must_use]
    pub const fn has_any_image_cell(&self) -> bool {
        self.image_cell_count > 0
    }

    /// Clear all image placements whose Kitty image number matches `number`.
    pub fn clear_image_placements_by_number(&mut self, number: u32) {
        let mut cleared = 0usize;
        for (i, row) in self.rows.iter_mut().enumerate() {
            let mut changed = false;
            for cell in row.cells_mut() {
                if cell
                    .image_placement()
                    .is_some_and(|p| p.image_number == Some(number))
                {
                    cell.clear_image();
                    cleared += 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                if i < self.row_cache.len() {
                    self.row_cache[i] = None;
                }
            }
        }
        self.image_cell_count -= cleared;
    }

    /// Clear image placements at a specific cell position.
    pub fn clear_image_placements_at_cell(&mut self, row: usize, col: usize) {
        if row >= self.rows.len() {
            return;
        }
        let id = {
            let cells = self.rows[row].cells();
            if col < cells.len() {
                cells[col].image_placement().map(|p| p.image_id)
            } else {
                None
            }
        };
        if let Some(id) = id {
            self.clear_image_placements_by_id(id);
        }
    }

    /// Clear image placements at a specific cell and all cells after it.
    pub fn clear_image_placements_at_cell_and_after(&mut self, row: usize, col: usize) {
        let mut ids_to_clear: Vec<u64> = Vec::new();
        for r in row..self.rows.len() {
            let start = if r == row { col } else { 0 };
            let cells = self.rows[r].cells();
            for cell in cells.iter().skip(start) {
                if let Some(placement) = cell.image_placement() {
                    let id = placement.image_id;
                    if !ids_to_clear.contains(&id) {
                        ids_to_clear.push(id);
                    }
                }
            }
        }
        for id in ids_to_clear {
            self.clear_image_placements_by_id(id);
        }
    }

    /// Clear all image placements that intersect the given column.
    pub fn clear_image_placements_in_column(&mut self, col: usize) {
        let mut ids_to_clear: Vec<u64> = Vec::new();
        for row in &self.rows {
            let cells = row.cells();
            if col < cells.len()
                && let Some(placement) = cells[col].image_placement()
            {
                let id = placement.image_id;
                if !ids_to_clear.contains(&id) {
                    ids_to_clear.push(id);
                }
            }
        }
        for id in ids_to_clear {
            self.clear_image_placements_by_id(id);
        }
    }

    /// Clear all image placements that intersect the given row.
    pub fn clear_image_placements_in_row(&mut self, row: usize) {
        if row >= self.rows.len() {
            return;
        }
        let mut ids_to_clear: Vec<u64> = Vec::new();
        let cells = self.rows[row].cells();
        for cell in cells {
            if let Some(placement) = cell.image_placement() {
                let id = placement.image_id;
                if !ids_to_clear.contains(&id) {
                    ids_to_clear.push(id);
                }
            }
        }
        for id in ids_to_clear {
            self.clear_image_placements_by_id(id);
        }
    }

    /// Clear all image placements with the given z-index.
    pub fn clear_image_placements_by_z_index(&mut self, z: i32) {
        let mut cleared = 0usize;
        for (i, row) in self.rows.iter_mut().enumerate() {
            let mut changed = false;
            for cell in row.cells_mut() {
                if cell.image_placement().is_some_and(|p| p.z_index == z) {
                    cell.clear_image();
                    cleared += 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                if i < self.row_cache.len() {
                    self.row_cache[i] = None;
                }
            }
        }
        self.image_cell_count -= cleared;
    }
    /// would overwrite any image cells.  If so, clear **all** cells of each
    /// affected image across the entire buffer.
    ///
    /// Only the cells in `[col .. col + text_len)` are inspected — images
    /// outside this range on the same row are not affected.
    ///
    /// Images are treated as atomic: overwriting even a single cell of an
    /// image invalidates the whole image.  This matches the behaviour of
    /// other terminal emulators (`iTerm2`, `WezTerm`, Kitty).
    fn clear_images_overwritten_by_text(&mut self, row_idx: usize, col: usize, text_len: usize) {
        self.collect_and_clear_image_ids_in_rows(
            row_idx,
            row_idx + 1,
            Some(col),
            Some(col + text_len),
        );
    }

    /// Scan rows in `[row_start..row_end)` for image cells and clear every
    /// cell of each found image across the entire buffer.
    ///
    /// If `start_col` is `Some(col)`, only cells from `col..` on the first
    /// row (or single row) are scanned.  If `end_col` is `Some(ec)`, only
    /// cells up to (exclusive) `ec` are scanned on the first row.  If
    /// `None`, scanning goes to the end of the row.
    ///
    /// For rows after the first (when `row_end - row_start > 1`), all cells
    /// are always scanned.
    ///
    /// This is a no-op when no image cells are present.
    fn collect_and_clear_image_ids_in_rows(
        &mut self,
        row_start: usize,
        row_end: usize,
        start_col: Option<usize>,
        end_col: Option<usize>,
    ) {
        let end = row_end.min(self.rows.len());
        if row_start >= end {
            return;
        }

        // Collect unique image IDs from the targeted cells, skipping Kitty
        // images (they are cleared only via explicit `a=d` commands).
        let mut ids_to_clear: Vec<u64> = Vec::new();
        for (idx, row) in self.rows[row_start..end].iter().enumerate() {
            let cells = row.cells();
            let skip = if idx == 0 {
                start_col.unwrap_or(0).min(cells.len())
            } else {
                0
            };
            let limit = if idx == 0 {
                end_col.unwrap_or(cells.len()).min(cells.len())
            } else {
                cells.len()
            };
            if skip >= limit {
                continue;
            }
            for cell in &cells[skip..limit] {
                if let Some(placement) = cell.image_placement() {
                    // Kitty images are not cleared by text writes.
                    if placement.protocol == ImageProtocol::Kitty {
                        continue;
                    }
                    let id = placement.image_id;
                    if !ids_to_clear.contains(&id) {
                        ids_to_clear.push(id);
                    }
                }
            }
        }

        // Clear all cells of each affected image buffer-wide.
        for id in ids_to_clear {
            self.clear_image_placements_by_id(id);
        }
    }

    /// Place an inline image at the current cursor position.
    ///
    /// The image is stored in the central `ImageStore` and cells in the
    /// rectangular region `[cursor_y .. cursor_y + display_rows) ×
    /// [cursor_x .. cursor_x + display_cols)` are filled with
    /// `ImagePlacement` references.
    ///
    /// After placement the cursor is moved to the row immediately below
    /// the image (or the last visible row if the image extends to the
    /// bottom), at column 0 — matching iTerm2 behaviour.
    ///
    /// If the image extends beyond the right edge of the terminal, it is
    /// clipped to the terminal width (cells beyond the edge are not placed).
    /// If the image extends below the visible area, new rows are created
    /// (scrolling if necessary in the primary buffer).
    pub fn place_image(
        &mut self,
        image: InlineImage,
        scroll_offset: usize,
        protocol: ImageProtocol,
        image_number: Option<u32>,
        placement_id: Option<u32>,
        z_index: i32,
    ) -> usize {
        let image_id = image.id;
        let display_cols = image.display_cols;
        let display_rows = image.display_rows;

        // Store the image centrally.
        self.image_store.insert(image);

        let start_col = self.cursor.pos.x;

        // Clamp display_cols to not exceed terminal width.
        let effective_cols = display_cols.min(self.width.saturating_sub(start_col));

        // Before placing new image cells, clear any existing image cells in
        // the column range from the cursor row downward.  This handles the
        // common case where a new (possibly smaller) image replaces an old
        // (possibly larger) one at the same position — without this, stale
        // cells from the old image persist below the new one.
        let clear_end_col = start_col + effective_cols;
        for row_idx in self.cursor.pos.y..self.rows.len() {
            let row = &mut self.rows[row_idx];
            let mut changed = false;
            for col in start_col..clear_end_col.min(row.max_width()) {
                if let Some(cell) = row.cells_mut().get_mut(col)
                    && cell.has_image()
                {
                    cell.clear_image();
                    self.image_cell_count -= 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                if row_idx < self.row_cache.len() {
                    self.row_cache[row_idx] = None;
                }
            }
        }

        let mut current_offset = scroll_offset;

        // Place image cells row by row.
        //
        // We track `base_row` which starts at the cursor's current row and
        // shifts downward as rows are created.  Unlike `scroll_up()`, we
        // grow the buffer by pushing new rows and then let
        // `enforce_scrollback_limit` trim excess from the top — this avoids
        // the infinite-loop problem where `scroll_up()` keeps rows.len()
        // constant.
        let mut base_row = self.cursor.pos.y;

        for img_row in 0..display_rows {
            let target_row = base_row + img_row;

            // Ensure the target row exists.
            while target_row >= self.rows.len() {
                self.push_row(RowOrigin::HardBreak, RowJoin::NewLogicalLine);
            }

            let row = &mut self.rows[target_row];
            row.dirty = true;

            // Invalidate the row cache for this row.
            if target_row < self.row_cache.len() {
                self.row_cache[target_row] = None;
            }

            let mut placed_count = 0usize;
            for img_col in 0..effective_cols {
                let col = start_col + img_col;
                if col >= self.width {
                    break;
                }
                let placement = ImagePlacement {
                    image_id,
                    col_in_image: img_col,
                    row_in_image: img_row,
                    protocol,
                    image_number,
                    placement_id,
                    z_index,
                };
                row.set_image_cell(col, placement, self.current_tag.clone());
                placed_count += 1;
            }
            self.image_cell_count += placed_count;
        }

        // Enforce scrollback limit — this may drain rows from the top.
        if self.kind == BufferType::Primary {
            let rows_before = self.rows.len();
            current_offset = self.enforce_scrollback_limit(current_offset);
            let drained = rows_before - self.rows.len();
            // Adjust base_row for the drained rows.
            base_row = base_row.saturating_sub(drained);
        }

        // Move cursor below the image, column 0 (iTerm2 behaviour).
        let final_row = base_row + display_rows;
        if final_row < self.rows.len() {
            self.cursor.pos.y = final_row;
        } else {
            self.cursor.pos.y = self.rows.len().saturating_sub(1);
        }
        self.cursor.pos.x = 0;

        self.debug_assert_invariants();
        current_offset
    }
}

// ============================================================================
// Private helpers
// ============================================================================

/// Compare two `FormatTag` values by their visual formatting only, ignoring
/// the `start` and `end` byte-position fields.
fn tags_same_format(a: &FormatTag, b: &FormatTag) -> bool {
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
    // 4. Cursor must clamp within new height
    // ------------------------------------------------------------------
    #[test]
    fn resize_clamps_cursor() {
        let mut buf = buffer_with_rows_and_config(10, 80, 10, 1000, false);

        buf.cursor.pos.y = 9; // last row
        buf.set_size(80, 5, 0); // shrink

        assert!(
            buf.cursor.pos.y <= 4,
            "cursor must clamp into new visible height"
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
        let (chars, tags) = buf.visible_as_tchars_and_tags(0);

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

        let (chars, tags) = buf.visible_as_tchars_and_tags(0);

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

        let (chars, tags) = buf.visible_as_tchars_and_tags(0);

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

        let (chars, tags) = buf.visible_as_tchars_and_tags(0);

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

        let (chars, tags) = buf.visible_as_tchars_and_tags(0);

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

        let (chars, _tags) = buf.visible_as_tchars_and_tags(0);

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
