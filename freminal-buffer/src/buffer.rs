// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::{
    buffer_states::{
        buffer_type::BufferType, cursor::CursorState, format_tag::FormatTag, tchar::TChar,
    },
    config::FontConfig,
};

use crate::{
    cell::Cell,
    response::InsertResponse,
    row::{Row, RowJoin, RowOrigin},
};

pub struct Buffer {
    /// All rows in this buffer: scrollback + visible region.
    /// In the primary buffer, this grows until `scrollback_limit` is hit.
    /// In the alternate buffer, this always has exactly `height` rows.
    rows: Vec<Row>,

    /// Width and height of the terminal grid.
    width: usize,
    height: usize,

    /// Current cursor position (row, col).
    cursor: CursorState,

    /// How far the user has scrolled back.
    ///
    /// 0 = bottom (normal live terminal mode)
    /// >0 = viewing older content
    scroll_offset: usize,

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
    ///   - Writing while scrolled back resets `scroll_offset`
    ///
    /// Alternate:
    ///   - No scrollback
    ///   - Switching back restores primary buffer's saved state
    kind: BufferType,

    /// Saved primary buffer content, cursor, `scroll_offset`,
    /// used when switching to and from alternate buffer.
    saved_primary: Option<SavedPrimaryState>,

    /// Current format tag to apply to inserted text.
    current_tag: FormatTag,

    /// LMN mode
    lnm_enabled: bool,

    /// Preserve the scrollback anchor when resizing
    preserve_scrollback_anchor: bool,

    /// DECSTBM top and bottom margins, 0-indexed, inclusive.
    /// When disabled, the region is full-screen: [0, height-1]
    scroll_region_top: usize,
    scroll_region_bottom: usize,
}

/// Everything we need to restore when leaving alternate buffer.
#[derive(Debug, Clone)]
pub struct SavedPrimaryState {
    pub rows: Vec<Row>,
    pub cursor: CursorState,
    pub scroll_offset: usize,
    pub scroll_region_top: usize,
    pub scroll_region_bottom: usize,
}

impl Buffer {
    /// Creates a new Buffer with the specified width and height.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        let rows = vec![Row::new(width)];

        Self {
            rows,
            width,
            height,
            cursor: CursorState::default(),
            current_tag: FormatTag::default(),
            scroll_offset: 0,
            scrollback_limit: 4000,
            kind: BufferType::Primary,
            saved_primary: None,
            lnm_enabled: false,
            preserve_scrollback_anchor: false,
            scroll_region_top: 0,
            scroll_region_bottom: height - 1,
        }
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
            debug_assert_eq!(
                self.scroll_offset, 0,
                "empty buffer must have zero scroll_offset"
            );
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
                debug_assert_eq!(
                    self.scroll_offset, 0,
                    "alternate buffer must never have a scroll_offset (got {})",
                    self.scroll_offset
                );
            }
        }

        // Scroll offset must always be within [0, max_scroll_offset].
        let max_off = self.max_scroll_offset();
        debug_assert!(
            self.scroll_offset <= max_off,
            "scroll_offset {} exceeds max_scroll_offset {}",
            self.scroll_offset,
            max_off
        );

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
    }

    // In release builds this is a no-op, so we can call it freely.
    #[cfg(not(debug_assertions))]
    #[inline]
    fn debug_assert_invariants(&self) {}

    fn push_row(&mut self, origin: RowOrigin, join: RowJoin) {
        let row = Row::new_with_origin(self.width, origin, join);
        self.rows.push(row);
    }

    fn push_row_with_kind(&mut self, origin: RowOrigin, join: RowJoin) {
        self.rows
            .push(Row::new_with_origin(self.width, origin, join));
    }

    #[must_use]
    pub const fn get_rows(&self) -> &Vec<Row> {
        &self.rows
    }

    #[must_use]
    pub const fn get_cursor(&self) -> &CursorState {
        &self.cursor
    }

    /// Get the rows that should be *visually displayed* in the GUI.
    ///
    /// Contract:
    /// - Returns a contiguous slice of `self.rows`.
    /// - `visible_rows().len() <= self.height`.
    /// - When `self.rows.len() <= self.height`, the slice is the entire buffer.
    /// - When `scroll_offset == 0`, the slice is the last `height` rows
    ///   (the live bottom).
    /// - When `scroll_offset > 0`, the slice is shifted upwards into
    ///   scrollback, clamped so it never goes before the oldest row.
    /// - Never allocates; always borrows from `self.rows`.
    #[must_use]
    pub fn visible_rows(&self) -> &[Row] {
        if self.rows.is_empty() {
            return &[];
        }

        let total = self.rows.len();
        let h = self.height;

        // Clamp scroll_offset within bounds.
        let max_offset = self.max_scroll_offset();
        let offset = self.scroll_offset.min(max_offset);

        let start = total.saturating_sub(h + offset);
        let end = start + h;

        &self.rows[start.min(total)..end.min(total)]
    }

    pub fn insert_text(&mut self, text: &[TChar]) {
        // If we're in the primary buffer and the user has scrolled back,
        // jump back to the live bottom view when new output arrives.
        if self.kind == BufferType::Primary && self.scroll_offset > 0 {
            self.scroll_offset = 0;
        }

        let mut remaining = text.to_vec();
        let mut row_idx = self.cursor.pos.y;
        let mut col = self.cursor.pos.x;

        // FIX #3: first write into row 0 turns it into a real logical line
        if row_idx == 0 && self.rows[0].origin == RowOrigin::ScrollFill {
            let row = &mut self.rows[0];
            row.origin = RowOrigin::HardBreak;
            row.join = RowJoin::NewLogicalLine;
        }

        loop {
            // ┌─────────────────────────────────────────────┐
            // │ PRE-WRAP: if we're already at/past width,   │
            // │ move to the next row as a soft-wrap row.    │
            // └─────────────────────────────────────────────┘
            if col >= self.width {
                row_idx += 1;
                col = 0;

                if row_idx >= self.rows.len() {
                    // brand new soft-wrap continuation row
                    self.push_row(RowOrigin::SoftWrap, RowJoin::ContinueLogicalLine);
                } else {
                    // reuse existing row as a soft-wrap continuation
                    let row = &mut self.rows[row_idx];
                    row.origin = RowOrigin::SoftWrap;
                    row.join = RowJoin::ContinueLogicalLine;
                    row.clear();
                }

                self.cursor.pos.y = row_idx;
            }

            // ┌─────────────────────────────────────────────┐
            // │ Ensure the target row exists.               │
            // │ If we got here without PRE-WRAP, it's a     │
            // │ normal new logical line. If col == 0 and    │
            // │ row_idx > 0, we are in a wrap continuation. │
            // └─────────────────────────────────────────────┘
            if row_idx >= self.rows.len() {
                let is_wrap_continuation = col == 0 && row_idx > 0;

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
            }

            // clone tag here to avoid long-lived borrows of &self
            let tag = self.current_tag.clone();

            // ┌─────────────────────────────────────────────┐
            // │ Try to insert into this row.                │
            // └─────────────────────────────────────────────┘
            match self.rows[row_idx].insert_text(col, &remaining, &tag) {
                InsertResponse::Consumed(final_col) => {
                    // All text fit on this row.
                    self.cursor.pos.x = final_col;
                    self.cursor.pos.y = row_idx;

                    self.enforce_scrollback_limit();
                    return;
                }

                InsertResponse::Leftover { data, final_col } => {
                    // This row filled; some data remains.
                    self.cursor.pos.x = final_col;
                    self.cursor.pos.y = row_idx;

                    remaining = data;

                    // Move to next row for continuation.
                    row_idx += 1;
                    col = 0;

                    // POST-WRAP: we now know a wrap actually occurred.
                    if row_idx >= self.rows.len() {
                        // brand new continuation
                        self.push_row(RowOrigin::SoftWrap, RowJoin::ContinueLogicalLine);
                    } else {
                        // reuse existing row as continuation
                        let row = &mut self.rows[row_idx];
                        row.origin = RowOrigin::SoftWrap;
                        row.join = RowJoin::ContinueLogicalLine;
                        row.clear();
                    }

                    self.cursor.pos.y = row_idx;
                    // `col` stays 0; next loop iteration writes at start of continuation row.
                }
            }
        }
    }

    /// Resize the terminal buffer.
    /// Reflows lines when width changes.
    /// Adjusts scrollback when height changes.
    pub fn set_size(&mut self, new_width: usize, new_height: usize) {
        let width_changed = new_width != self.width;
        let height_changed = new_height != self.height;

        if !width_changed && !height_changed {
            return;
        }

        // ---- WIDTH CHANGE → REFLOW ----
        if width_changed {
            self.reflow_to_width(new_width);
        }

        // ---- HEIGHT CHANGE → GROW/SHRINK SCREEN ----
        if height_changed {
            self.resize_height(new_height);

            // Clamp scroll region to new size (screen-relative).
            self.scroll_region_bottom = self.scroll_region_bottom.min(new_height.saturating_sub(1));

            if self.scroll_region_top >= self.scroll_region_bottom {
                self.reset_scroll_region_to_full();
            }
        }

        // Update buffer scalars
        self.width = new_width;
        self.height = new_height;

        // Ensure every row's max_width matches the new buffer width
        if width_changed {
            for row in &mut self.rows {
                row.set_max_width(new_width);
            }
        }

        // Always clamp cursor after size change
        self.clamp_cursor_after_resize();
        self.debug_assert_invariants();
    }

    #[allow(clippy::too_many_lines)]
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
        self.width = new_width;

        // 4) Reset scroll offset; ensure cursor is in bounds.
        self.scroll_offset = 0;

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

    fn resize_height(&mut self, new_height: usize) {
        let old_height = self.height;

        if new_height > old_height {
            // Grow: add blank rows at the bottom
            let grow = new_height - old_height;
            for _ in 0..grow {
                self.rows.push(Row::new(self.width));
            }
        } else if new_height < old_height {
            // If cursor is above the bottom of the new visible window, clamp it.
            if self.cursor.pos.y >= new_height {
                self.cursor.pos.y = new_height.saturating_sub(1);
            }
        }

        if self.preserve_scrollback_anchor {
            // IMPORTANT: use new_height, not self.height (which is still old here)
            let max_offset = if self.rows.len() > new_height {
                self.rows.len() - new_height
            } else {
                0
            };
            self.scroll_offset = self.scroll_offset.min(max_offset);
        } else {
            // xterm-style
            self.scroll_offset = 0;
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

    fn enforce_scrollback_limit(&mut self) {
        // Only primary buffer keeps scrollback.
        if self.kind == BufferType::Alternate {
            return;
        }

        let max_rows = self.height + self.scrollback_limit;

        // Nothing to trim, but still make sure scroll_offset is not insane
        if self.rows.len() <= max_rows {
            let max_offset = self.max_scroll_offset();
            if self.scroll_offset > max_offset {
                self.scroll_offset = max_offset;
            }
            return;
        }

        // Number of rows to drop from the top of the scrollback.
        let overflow = self.rows.len() - max_rows;

        // --- Adjust scroll_offset BEFORE modifying the rows ---
        //
        // If the user is scrolled back into the area we're about to delete,
        // reduce their offset by the number of deleted rows. If that wipes
        // out all their scrollback, snap them to live view.
        if self.scroll_offset > 0 {
            if self.scroll_offset > overflow {
                self.scroll_offset -= overflow;
            } else {
                self.scroll_offset = 0;
            }
        }

        // --- Drop the oldest rows ---
        self.rows.drain(0..overflow);

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
        if self.scroll_offset > max_offset {
            self.scroll_offset = max_offset;
        }

        self.debug_assert_invariants();
    }

    /// Handle ANSI Backspace (BS, 0x08).
    ///
    /// Semantics (ECMA-48, VT100):
    /// - Move cursor left by one cell.
    /// - If the cell to the left is a continuation cell of a wide glyph,
    ///   skip left until the glyph head.
    /// - If cursor is at column 0, do nothing.
    /// - Never moves vertically and never deletes characters.
    pub fn handle_backspace(&mut self) {
        // Backspace is a purely local operation. Do NOT perform scrollback
        // enforcement or invariants after this function.

        if self.cursor.pos.x == 0 {
            return;
        }

        let row_idx = self.cursor.pos.y;

        if row_idx >= self.rows.len() {
            return;
        }

        let row = &self.rows[row_idx];

        let mut new_x = self.cursor.pos.x - 1;

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

    pub fn handle_lf(&mut self) {
        match self.kind {
            BufferType::Primary => {
                // Reset scrollback if output arrives while scrolled back
                if self.scroll_offset > 0 {
                    self.scroll_offset = 0;
                }

                // LNM: CR implied
                if self.lnm_enabled {
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
                    self.cursor.pos.y += 1;

                    if self.cursor.pos.y >= self.rows.len() {
                        self.rows.push(Row::new_with_origin(
                            self.width,
                            RowOrigin::HardBreak,
                            RowJoin::NewLogicalLine,
                        ));
                    } else {
                        let row = &mut self.rows[self.cursor.pos.y];
                        row.origin = RowOrigin::HardBreak;
                        row.join = RowJoin::NewLogicalLine;
                        row.clear();
                    }

                    self.enforce_scrollback_limit();
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
                        self.rows.push(Row::new_with_origin(
                            self.width,
                            RowOrigin::HardBreak,
                            RowJoin::NewLogicalLine,
                        ));
                        self.cursor.pos.y = self.rows.len() - 1;
                    }
                }

                self.enforce_scrollback_limit();
                self.debug_assert_invariants();
            }

            BufferType::Alternate => {
                // (keep your existing Alternate LF unchanged)
                if self.lnm_enabled {
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

    pub const fn handle_cr(&mut self) {
        self.cursor.pos.x = 0;
    }

    /// IND – Index (move down, scroll within DECSTBM region).
    /// Same as LF, but *does not* honor LNM (no implicit CR).
    pub fn handle_ind(&mut self) {
        // Temporarily disable LNM so `handle_lf` won't do CR.
        let old_lnm = self.lnm_enabled;
        self.lnm_enabled = false;
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
        match self.kind {
            BufferType::Primary => {
                let sy = self.cursor_screen_y();

                // FULL-SCREEN REGION → RI never scrolls
                if self.scroll_region_top == 0 && self.scroll_region_bottom == self.height - 1 {
                    if self.cursor.pos.y > 0 {
                        self.cursor.pos.y -= 1;
                    }
                    self.debug_assert_invariants();
                    return;
                }

                // PARTIAL DECSTBM
                if sy >= self.scroll_region_top && sy <= self.scroll_region_bottom {
                    if sy > self.scroll_region_top {
                        // move up inside region
                        self.cursor.pos.y -= 1;
                    } else {
                        // at top → scroll region DOWN
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
                // (unchanged)
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

                for _ in 0..count {
                    self.scroll_slice_down(y, self.scroll_region_bottom);
                }

                self.debug_assert_invariants();
            }

            BufferType::Primary => {
                if self.scroll_offset > 0 {
                    return;
                }

                let sy = self.cursor_screen_y();
                if sy < self.scroll_region_top || sy > self.scroll_region_bottom {
                    return;
                }

                let (t, b) = self.scroll_region_rows();
                let offset = sy - self.scroll_region_top;
                let row = t + offset;

                let count = n.min(b - row + 1);
                for _ in 0..count {
                    self.scroll_slice_down(row, b);
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

                for _ in 0..count {
                    self.scroll_slice_up(y, self.scroll_region_bottom);
                }

                self.debug_assert_invariants();
            }

            BufferType::Primary => {
                if self.scroll_offset > 0 {
                    return;
                }

                let sy = self.cursor_screen_y();
                if sy < self.scroll_region_top || sy > self.scroll_region_bottom {
                    return;
                }

                let (t, b) = self.scroll_region_rows();
                let offset = sy - self.scroll_region_top;
                let row = t + offset;

                let count = n.min(b - row + 1);
                for _ in 0..count {
                    self.scroll_slice_up(row, b);
                }

                self.debug_assert_invariants();
            }
        }
    }

    /// Implements ICH – Insert Characters (spaces).
    pub fn insert_spaces(&mut self, n: usize) {
        let row = self.cursor.pos.y;
        let col = self.cursor.pos.x;

        if row >= self.rows.len() {
            return;
        }

        let tag = self.current_tag.clone();
        self.rows[row].insert_spaces_at(col, n, &tag);

        // Cursor does NOT move for ICH
    }

    const fn reset_scroll_region_to_full(&mut self) {
        self.scroll_region_top = 0;
        self.scroll_region_bottom = self.height.saturating_sub(1);
    }

    /// Set DECSTBM scroll region (1-based inclusive).
    /// If invalid, resets to full screen.
    pub const fn set_scroll_region(&mut self, top1: usize, bottom1: usize) {
        // the terminal is passing in 1 based values. Clamp
        let top1 = top1.saturating_sub(1);
        let bottom1 = bottom1.saturating_sub(1);

        // 0 or missing → ignore and reset
        if top1 == 0 || bottom1 == 0 {
            self.reset_scroll_region_to_full();
            return;
        }

        // Convert to 0-based
        let top = top1 - 1;
        let bottom = bottom1 - 1;

        // Validate
        if top >= bottom || bottom >= self.height {
            self.reset_scroll_region_to_full();
            return;
        }

        self.scroll_region_top = top;
        self.scroll_region_bottom = bottom;

        // xterm behavior: move cursor to row 0 of region
        self.cursor.pos.y = self.scroll_region_top;
        self.cursor.pos.x = 0;
    }

    /// Index in `rows` of the first visible line.
    /// This is the same start index used by `visible_rows()`.
    fn visible_window_start(&self) -> usize {
        if self.rows.is_empty() || self.height == 0 {
            return 0;
        }

        let total = self.rows.len();
        let h = self.height.min(total);
        let offset = self.scroll_offset.min(self.max_scroll_offset());

        total.saturating_sub(h + offset)
    }

    /// Cursor Y expressed in "screen coordinates" (0..height-1).
    /// If the buffer is shorter than the height, we just return the raw Y.
    fn cursor_screen_y(&self) -> usize {
        if self.rows.is_empty() || self.height == 0 {
            return 0;
        }

        let start = self.visible_window_start();
        self.cursor.pos.y.saturating_sub(start)
    }

    #[inline]
    fn at_scroll_region_bottom(&self) -> bool {
        if self.height == 0 {
            return false;
        }
        self.cursor_screen_y() == self.scroll_region_bottom
    }

    #[inline]
    fn at_scroll_region_top(&self) -> bool {
        if self.height == 0 {
            return false;
        }
        self.cursor_screen_y() == self.scroll_region_top
    }

    /// Convert DECSTBM region (screen coords) into buffer row indices (rows[])
    fn scroll_region_rows(&self) -> (usize, usize) {
        let start = self.visible_window_start();
        let top = start + self.scroll_region_top;
        let bottom = start + self.scroll_region_bottom;
        let max = self.rows.len().saturating_sub(1);
        (top.min(max), bottom.min(max))
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
        }

        self.rows[last] = Row::new(self.width);
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
        }

        self.rows[first] = Row::new(self.width);
    }

    // ----------------------------------------------------------
    // Scrollback: only valid in the PRIMARY buffer
    // ----------------------------------------------------------

    /// How many lines above the live bottom we can scroll.
    const fn max_scroll_offset(&self) -> usize {
        if self.rows.len() <= self.height {
            0
        } else {
            self.rows.len() - self.height
        }
    }

    /// Scroll upward (`lines > 0`) in the primary buffer.
    pub fn scroll_back(&mut self, lines: usize) {
        if self.kind != BufferType::Primary {
            return; // Alternate buffer: no scrollback
        }

        let max = self.max_scroll_offset();
        if max == 0 {
            return;
        }

        self.scroll_offset = (self.scroll_offset + lines).min(max);

        self.debug_assert_invariants();
    }

    /// Scroll downward (`lines > 0`) toward the live bottom.
    pub fn scroll_forward(&mut self, lines: usize) {
        if self.kind != BufferType::Primary {
            return;
        }

        if self.scroll_offset == 0 {
            return;
        }

        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.debug_assert_invariants();
    }

    /// Jump back to the live view (row = last row).
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.debug_assert_invariants();
    }

    pub fn scroll_up(&mut self) {
        // remove topmost row
        self.rows.remove(0);

        // add a new empty row at the bottom
        self.rows.push(Row::new(self.width));

        // DO NOT move the cursor in alternate buffer
        if self.kind == BufferType::Primary {
            // primary buffer uses scrollback: move cursor with the visible window
            if self.cursor.pos.y > 0 {
                self.cursor.pos.y -= 1;
            }
        }
    }

    /// Switch from the primary buffer to the alternate screen.
    ///
    /// - Saves current rows, cursor, and `scroll_offset`.
    /// - Replaces contents with a fresh empty screen (height rows).
    /// - Disables scrollback semantics for the alternate screen.
    pub fn enter_alternate(&mut self) {
        // If we're already in the alternate buffer, do nothing.
        if self.kind == BufferType::Alternate {
            return;
        }

        // Save primary state (rows + cursor + scroll_offset).
        let saved = SavedPrimaryState {
            rows: self.rows.clone(),
            cursor: self.cursor.clone(),
            scroll_offset: self.scroll_offset,
            scroll_region_top: self.scroll_region_top,
            scroll_region_bottom: self.scroll_region_bottom,
        };
        self.saved_primary = Some(saved);

        // Switch to alternate buffer.
        self.kind = BufferType::Alternate;

        // Fresh screen: exactly `height` empty rows.
        self.rows = vec![Row::new(self.width); self.height];

        // Reset cursor and scroll offset for the alternate screen.
        self.cursor = CursorState::default();
        self.scroll_offset = 0;

        // Alternate screen starts with a full-screen scroll region.
        self.reset_scroll_region_to_full();

        self.debug_assert_invariants();
    }

    /// Leave the alternate screen and restore the primary buffer, if any was saved.
    pub fn leave_alternate(&mut self) {
        // If we're not in alternate, nothing to do.
        if self.kind != BufferType::Alternate {
            return;
        }

        if let Some(saved) = self.saved_primary.take() {
            // Restore saved primary state.
            self.rows = saved.rows;
            self.cursor = saved.cursor;
            self.scroll_offset = saved.scroll_offset;
            self.scroll_region_top = saved.scroll_region_top;
            self.scroll_region_bottom = saved.scroll_region_bottom;
        }

        self.kind = BufferType::Primary;

        self.debug_assert_invariants();
    }
}

// tests

// ============================================================================
// Unit Tests for Buffer
// ============================================================================

#[cfg(test)]
mod basic_tests {
    use super::*;
    use crate::row::Row;
    use freminal_common::buffer_states::buffer_type::BufferType;
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

        buf.handle_lf();
        assert_eq!(buf.cursor.pos.y, 1);
        assert_eq!(buf.rows.len(), 2);
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
    fn primary_insert_text_resets_scroll_offset() {
        let mut buf = Buffer::new(10, 5);
        buf.scroll_offset = 3; // simulate user scrollback

        buf.insert_text(&[ascii('A')]);

        assert_eq!(buf.scroll_offset, 0);
    }

    // ────────────────────────────────────────────────────────────────
    // ALTERNATE BUFFER TESTS
    // ────────────────────────────────────────────────────────────────

    #[test]
    fn alt_buffer_has_no_scrollback() {
        let mut buf = Buffer::new(5, 3);
        buf.enter_alternate();

        assert_eq!(buf.rows.len(), 3);
        assert_eq!(buf.kind, BufferType::Alternate);
    }

    #[test]
    fn alt_buffer_lf_scrolls_screen() {
        let mut buf = Buffer::new(5, 3);
        buf.enter_alternate();

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
        buf.enter_alternate();

        // Do some things in alternate screen (optional)
        buf.handle_lf();

        // Leave alternate, restoring primary
        buf.leave_alternate();

        assert_eq!(buf.kind, BufferType::Primary);
        assert_eq!(buf.rows.len(), saved_rows);
        assert_eq!(buf.cursor.pos.y, saved_y);
    }

    #[test]
    fn scrollback_no_effect_when_no_history() {
        let mut buf = Buffer::new(5, 3);

        buf.scroll_back(10);
        assert_eq!(buf.scroll_offset, 0);
    }

    #[test]
    fn scrollback_clamps_to_max_offset() {
        let mut buf = Buffer::new(5, 3);

        // Add many lines
        for _ in 0..10 {
            buf.handle_lf();
        }

        let max = buf.rows.len() - buf.height;
        buf.scroll_back(999);

        assert_eq!(buf.scroll_offset, max);
    }

    #[test]
    fn scroll_forward_clamps_to_zero() {
        let mut buf = Buffer::new(5, 3);

        for _ in 0..10 {
            buf.handle_lf();
        }

        buf.scroll_back(5); // scroll up some amount
        buf.scroll_forward(999); // scroll down more than enough

        assert_eq!(buf.scroll_offset, 0);
    }

    #[test]
    fn scroll_to_bottom_resets_offset() {
        let mut buf = Buffer::new(5, 3);

        for _ in 0..10 {
            buf.handle_lf();
        }

        buf.scroll_back(5);
        assert!(buf.scroll_offset > 0);

        buf.scroll_to_bottom();

        assert_eq!(buf.scroll_offset, 0);
    }

    #[test]
    fn no_scrollback_in_alternate_buffer() {
        let mut buf = Buffer::new(5, 3);
        buf.enter_alternate();

        for _ in 0..10 {
            buf.handle_lf(); // scrolls but no scrollback
        }

        buf.scroll_back(10);
        assert_eq!(buf.scroll_offset, 0);

        buf.scroll_forward(10);
        assert_eq!(buf.scroll_offset, 0);
    }

    #[test]
    fn insert_text_resets_scrollback() {
        let mut buf = Buffer::new(10, 5);

        for _ in 0..20 {
            buf.handle_lf();
        }

        buf.scroll_back(5);
        assert!(buf.scroll_offset > 0);

        buf.insert_text(&[TChar::Ascii(b'A')]);

        assert_eq!(buf.scroll_offset, 0);
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

        assert!(found,"Soft-wrap should produce at least one SoftWrap/ContinueLogicalLine row after the first");
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

        buf.handle_cr(); // reset X
        buf.insert_text(&to_tchars("HELLO"));

        assert_eq!(buf.cursor.pos.y, row0, "CR must not change row");
        assert_eq!(buf.rows.len(), 1, "No new row must be created");
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
        buf.lnm_enabled = true;

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
        buf.set_size(15, 100);

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

        buf.set_size(10, 100);

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

        buf.set_size(40, 100);

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

        buf.set_size(10, 100);

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

        buf.set_size(20, 10);

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

        buf.set_size(8, 100);

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

        buf.handle_backspace();
        assert_eq!(buf.cursor.pos.x, 2);

        buf.handle_backspace();
        assert_eq!(buf.cursor.pos.x, 1);

        buf.handle_backspace();
        assert_eq!(buf.cursor.pos.x, 0);

        // stays at 0
        buf.handle_backspace();
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

        buf.handle_backspace(); // over b → x=3
        assert_eq!(buf.cursor.pos.x, 3);

        buf.handle_backspace(); // over wide glyph (continuation cell)
        assert_eq!(buf.cursor.pos.x, 1);

        buf.handle_backspace(); // over 'a'
        assert_eq!(buf.cursor.pos.x, 0);

        buf.handle_backspace(); // can't go lower
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

        // backspace never moves Y
        buf.handle_backspace();
        assert_eq!(buf.cursor.pos.y, 1);
        assert_eq!(buf.cursor.pos.x, 0);

        // at col 0 → stays there
        buf.handle_backspace();
        assert_eq!(buf.cursor.pos.y, 1);
        assert_eq!(buf.cursor.pos.x, 0);
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
                    TChar::Utf8(v) => {
                        let s = String::from_utf8_lossy(&v[..]);
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
mod tests_gui_scroll {
    use super::*;

    fn make_row(width: usize) -> Row {
        Row::new_with_origin(width, RowOrigin::HardBreak, RowJoin::NewLogicalLine)
    }

    fn buffer_with_rows(n: usize, width: usize, height: usize, scrollback: usize) -> Buffer {
        let mut b = Buffer::new(width, height);
        b.scrollback_limit = scrollback;
        b.rows = (0..n).map(|_| make_row(width)).collect();

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

        buf.enforce_scrollback_limit();

        assert_eq!(buf.rows.len(), 10, "should trim to max_rows");
        assert_eq!(buf.cursor.pos.y, 9, "cursor adjusted to last row");
        assert_eq!(buf.scroll_offset, 0, "no scrollback, so offset remains 0");
    }

    // ---------------------------------------------------------------
    // Test 2: scroll_offset reduces when rows are trimmed
    // ---------------------------------------------------------------

    #[test]
    fn enforce_limit_reduces_scroll_offset() {
        // height=5, scrollback_limit=5 → max_rows=10
        // Start with 15 rows, scroll_offset=4
        let mut buf = buffer_with_rows(15, 80, 5, 5);
        buf.scroll_offset = 4;

        buf.enforce_scrollback_limit();

        // Overflow = 5 rows trimmed
        // scroll_offset = 4 → trimmed by 5 → becomes 0
        assert_eq!(buf.scroll_offset, 0);
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
        buf.scroll_offset = 6;

        buf.enforce_scrollback_limit();

        assert_eq!(buf.rows.len(), 10);
        assert_eq!(buf.scroll_offset, 2);
    }

    // ---------------------------------------------------------------
    // Test 4: cursor shifts downward when rows removed
    // ---------------------------------------------------------------

    #[test]
    fn enforce_limit_adjusts_cursor_position() {
        // rows=12, height=5 → max_rows=10 → overflow=2
        let mut buf = buffer_with_rows(12, 80, 5, 5);

        buf.cursor.pos.y = 3;
        buf.enforce_scrollback_limit();

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

        buf.enforce_scrollback_limit();

        assert_eq!(buf.cursor.pos.y, 5, "cursor should shift by overflow");
    }

    // ---------------------------------------------------------------
    // Test 6: no scrollback trimming in alternate buffer
    // ---------------------------------------------------------------

    #[test]
    fn enforce_limit_noop_in_alternate_buffer() {
        let mut buf = buffer_with_rows(20, 80, 5, 5);
        buf.kind = BufferType::Alternate;
        buf.scroll_offset = 3;
        buf.cursor.pos.y = 10;

        let original_len = buf.rows.len();

        buf.enforce_scrollback_limit();

        assert_eq!(buf.rows.len(), original_len, "alternate buffer never trims");
        assert_eq!(buf.scroll_offset, 3);
        assert_eq!(buf.cursor.pos.y, 10);
    }

    // ---------------------------------------------------------------
    // Test 7: scroll_offset never exceeds new max_scroll_offset()
    // ---------------------------------------------------------------

    #[test]
    fn enforce_limit_clamps_scroll_offset_to_max() {
        // rows=13, height=5 → max_scroll_offset = 13-5 = 8
        let mut buf = buffer_with_rows(13, 80, 5, 5);
        buf.scroll_offset = 50; // wildly out of range

        buf.enforce_scrollback_limit();

        let max = buf.max_scroll_offset();
        assert!(buf.scroll_offset <= max);
    }
}

#[cfg(test)]
mod tests_gui_resize {
    use super::*;
    use crate::buffer::{Buffer, BufferType};
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

        buf.scroll_offset = 15;
        buf.set_size(80, 10); // shrink height

        assert_eq!(
            buf.scroll_offset, 0,
            "resize must reset scroll_offset when anchor is disabled"
        );
    }

    // ------------------------------------------------------------------
    // 2. preserve_scrollback_anchor = true → scroll_offset preserved on grow
    // ------------------------------------------------------------------
    #[test]
    fn resize_preserves_offset_when_growing_height() {
        let mut buf = buffer_with_rows_and_config(50, 80, 20, 1000, true);

        buf.scroll_offset = 10;
        buf.set_size(80, 30); // grow height

        assert_eq!(
            buf.scroll_offset, 10,
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

        buf.scroll_offset = 100; // far beyond range
        buf.set_size(80, 10);

        assert_eq!(
            buf.scroll_offset, 40,
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
        buf.set_size(80, 5); // shrink

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

        buf.set_size(80, 15);

        assert_eq!(buf.rows.len(), 15, "growing height must append blank rows");
    }

    // ------------------------------------------------------------------
    // 6. Shrinking height does not delete scrollback rows
    // ------------------------------------------------------------------
    #[test]
    fn resize_shrink_retain_scrollback() {
        let mut buf = buffer_with_rows_and_config(40, 80, 20, 1000, false);

        buf.set_size(80, 10);

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
    fn assert_visible_rows_consistent(b: &Buffer) {
        let total = b.rows.len();
        let h = b.height;

        if total == 0 {
            assert_eq!(b.visible_rows().len(), 0);
            return;
        }

        let max_offset = b.max_scroll_offset();
        let offset = b.scroll_offset.min(max_offset);

        let start = total.saturating_sub(h + offset);
        let mut end = start + h;
        if end > total {
            end = total;
        }

        let expected = &b.rows[start..end];
        let visible = b.visible_rows();

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
        let mut b = make_buffer(20, 3, 10);

        // At live bottom
        b.scroll_offset = 0;
        assert_visible_rows_consistent(&b);
    }

    #[test]
    fn visible_rows_respects_scroll_offset_in_scrollback() {
        let mut b = make_buffer(20, 3, 10);

        // Scroll back into history
        b.scroll_back(2);
        assert!(b.scroll_offset > 0);

        assert_visible_rows_consistent(&b);
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
        let vis = b.visible_rows();
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

        b.scroll_back(2);
        assert!(b.scroll_offset > 0, "should be scrolled back before reflow");

        // Change width to trigger reflow_to_width
        b.set_size(10, 3);

        // reflow_to_width resets scroll_offset
        assert_eq!(b.scroll_offset, 0);

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
        b.set_size(40, 4);

        // Scroll offset is always reset by reflow
        assert_eq!(b.scroll_offset, 0);

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

    fn assert_visible_rows_sane(b: &Buffer) {
        let vis = b.visible_rows();
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
        b.scroll_back(3);

        // Now shrink height
        b.set_size(5, 3);

        // Whatever scroll_offset is now, visible_rows must be well-formed
        assert_visible_rows_sane(&b);
    }
}

#[cfg(test)]
mod visible_rows_boundary_tests {
    use super::*;

    #[test]
    fn visible_rows_small_buffer_returns_all_rows() {
        let mut b = Buffer::new(10, 5);

        // initial row + 2 LFs → 3 rows total
        b.handle_lf();
        b.handle_lf();

        assert_eq!(b.rows.len(), 3);

        let vis = b.visible_rows();
        // Since rows.len() < height, we should get all rows.
        assert_eq!(vis.len(), b.rows.len());
    }

    #[test]
    fn visible_rows_exact_height() {
        let mut b = Buffer::new(10, 3);

        b.handle_lf();
        b.handle_lf(); // 3 rows total

        assert_eq!(b.rows.len(), 3);

        let vis = b.visible_rows();
        assert_eq!(vis.len(), 3);
    }

    #[test]
    fn visible_rows_top_of_scrollback_is_first_rows() {
        let mut b = Buffer::new(10, 3);

        for _ in 0..10 {
            b.handle_lf();
        }

        b.scroll_back(999); // scroll to top
        let vis = b.visible_rows();

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

        b.enter_alternate();
        let vis = b.visible_rows();

        assert_eq!(vis.len(), 4);
        assert!(vis.iter().all(|r| r.get_characters().is_empty()));
    }

    #[test]
    fn leave_alt_restores_primary_visible_rows() {
        let mut b = Buffer::new(5, 4);

        for _ in 0..10 {
            b.handle_lf();
        }

        b.scroll_back(2);
        let before = b.visible_rows()[0].get_characters().clone();

        b.enter_alternate();
        b.leave_alternate();

        let after = b.visible_rows()[0].get_characters().clone();
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

    fn assert_visible_rows_consistent(b: &Buffer) {
        let total = b.rows.len();
        let h = b.height;

        if total == 0 {
            assert_eq!(b.visible_rows().len(), 0);
            return;
        }

        let max_offset = b.max_scroll_offset();
        let offset = b.scroll_offset.min(max_offset);

        let start = total.saturating_sub(h + offset);
        let mut end = start + h;
        if end > total {
            end = total;
        }

        let expected = &b.rows[start..end];
        let visible = b.visible_rows();

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
        b.scroll_back(1);

        // Whatever the final scroll_offset is, visible_rows must be a
        // correct slice of rows.
        assert_visible_rows_consistent(&b);
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
        assert_eq!(b.scroll_offset, 0);
    }

    #[test]
    fn scroll_up_does_not_break_scrollback_offset() {
        let mut b = Buffer::new(10, 3);

        for _ in 0..20 {
            b.handle_lf();
        }

        b.scroll_back(5);

        b.scroll_up(); // remove row 0
        assert_eq!(b.scroll_offset, 5, "scroll_offset must not change");
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
        b.scroll_back(3);
        assert_eq!(b.scroll_offset, 3);

        b.enter_alternate();
        b.handle_lf();
        b.handle_lf();

        b.leave_alternate();
        assert_eq!(b.scroll_offset, 3);
    }
}
