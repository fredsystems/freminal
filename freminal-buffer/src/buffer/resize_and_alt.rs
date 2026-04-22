// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Buffer resize, reflow, and alternate-screen management.
//!
//! This module contains methods for resizing the terminal buffer (width and
//! height), reflowing soft-wrapped lines to a new width, enforcing the
//! scrollback row limit, and switching between the primary and alternate
//! screen buffers.

use freminal_common::buffer_states::{
    buffer_type::BufferType,
    cursor::CursorState,
    format_tag::FormatTag,
    modes::{decawm::Decawm, declrmm::Declrmm, decom::Decom, lnm::Lnm},
};

use crate::row::{Row, RowJoin, RowOrigin};

use super::{Buffer, SavedPrimaryState};

impl Buffer {
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
            //
            // `self.height` is still the OLD height here (updated at line 785).
            //
            // If the scroll region covered the entire old screen (which is the
            // default after DECSTBM reset or after `enter_alternate`), expand it
            // to cover the entire NEW screen.  Without this, growing the buffer
            // (e.g. from 29→58 rows when a split pane is closed) leaves the
            // scroll region at the old height.  Full-screen TUIs like nvim that
            // redraw via space-fill + LF rather than ED hit the old
            // scroll_region_bottom and start scrolling prematurely, causing the
            // lower half of the screen to never be written.
            let old_max_bottom = self.height.saturating_sub(1);
            let new_max_bottom = new_height.saturating_sub(1);

            let was_full_screen =
                self.scroll_region_top == 0 && self.scroll_region_bottom == old_max_bottom;

            if was_full_screen {
                // Region was full-screen → keep it full-screen at the new size.
                self.scroll_region_top = 0;
                self.scroll_region_bottom = new_max_bottom;
            } else if self.scroll_region_bottom >= new_height
                || self.scroll_region_top >= new_height
                || self.scroll_region_top >= self.scroll_region_bottom
            {
                // Region is now invalid → reset to full screen.
                self.scroll_region_top = 0;
                self.scroll_region_bottom = new_max_bottom;
            } else {
                // Partial region still valid → just clamp bottom.
                self.scroll_region_bottom = self.scroll_region_bottom.min(new_max_bottom);
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

        // Ensure every row's max_width matches the new buffer width.
        // For alternate buffers (which skip reflow), the row-level cache
        // entries still contain flattened data at the old width — we must
        // invalidate them so the next `flatten_visible` re-flattens every
        // row at the new width.  Without this, nvim (and any full-screen
        // TUI on the alternate screen) renders with stale row data after a
        // resize, causing gaps, uncolored cells, and mispositioned content.
        if width_changed {
            for (i, row) in self.rows.iter_mut().enumerate() {
                row.set_max_width(new_width);
                // On the alternate screen (which skips reflow) a shrink leaves
                // stale cells beyond the new width in row.cells.  flatten_row
                // iterates row.cells directly, so those stale cells would leak
                // into the snapshot and render as a strip of old content at
                // the right edge of the viewport.  `truncate_cells_to_width`
                // is a no-op when cells.len() <= new_width, so it is safe to
                // call unconditionally (including on grow).
                row.truncate_cells_to_width(new_width);
                row.dirty = true;
                self.row_cache[i] = None;
            }
        }

        // Always clamp cursor after size change
        self.clamp_cursor_after_resize();

        // Enforce scrollback limit after resize (reflow may have created extra rows)
        let final_offset = self.enforce_scrollback_limit(after_resize);

        // When on the alternate screen, also resize the saved primary buffer
        // so that `leave_alternate` restores a buffer that matches the current
        // terminal dimensions.  Without this, exiting a full-screen TUI (e.g.
        // nvim) after a pane resize restores the primary buffer at the old
        // dimensions, causing immediate rendering artifacts.
        if self.kind == BufferType::Alternate
            && let Some(saved) = self.saved_primary.take()
        {
            let saved = Self::resize_saved_primary(saved, new_width, new_height);
            self.saved_primary = Some(saved);
        }

        self.debug_assert_invariants();

        final_offset
    }

    /// Resize a saved primary buffer to new dimensions.
    ///
    /// Builds a temporary primary `Buffer`, applies `set_size`, and extracts
    /// the updated state back into a `SavedPrimaryState`.  This reuses all
    /// the existing resize logic (reflow, height adjust, scroll region
    /// validation, cursor clamping, scrollback limit enforcement) instead of
    /// duplicating it.
    fn resize_saved_primary(
        saved: SavedPrimaryState,
        new_width: usize,
        new_height: usize,
    ) -> SavedPrimaryState {
        // Reconstruct a temporary primary Buffer from the saved state.
        let old_width = saved.rows.first().map_or(new_width, Row::max_width);
        let old_height = saved.height;

        let mut tmp = Self {
            rows: saved.rows,
            row_cache: saved.row_cache,
            width: old_width,
            height: old_height,
            cursor: saved.cursor,
            current_tag: FormatTag::default(),
            scrollback_limit: 4000,
            kind: BufferType::Primary,
            saved_primary: None,
            saved_cursor: saved.saved_cursor,
            lnm_enabled: Lnm::LineFeed,
            wrap_enabled: Decawm::AutoWrap,
            preserve_scrollback_anchor: false,
            scroll_region_top: saved.scroll_region_top,
            scroll_region_bottom: saved.scroll_region_bottom,
            scroll_region_left: saved.scroll_region_left,
            scroll_region_right: saved.scroll_region_right,
            declrmm_enabled: Declrmm::Disabled,
            tab_stops: Self::default_tab_stops(old_width),
            decom_enabled: Decom::NormalCursor,
            image_store: saved.image_store,
            image_cell_count: saved.image_cell_count,
            prompt_rows: Vec::new(),
        };

        let new_offset = tmp.set_size(new_width, new_height, saved.scroll_offset);

        SavedPrimaryState {
            rows: tmp.rows,
            row_cache: tmp.row_cache,
            cursor: tmp.cursor,
            scroll_offset: new_offset,
            height: new_height,
            scroll_region_top: tmp.scroll_region_top,
            scroll_region_bottom: tmp.scroll_region_bottom,
            scroll_region_left: tmp.scroll_region_left,
            scroll_region_right: tmp.scroll_region_right,
            saved_cursor: tmp.saved_cursor,
            image_store: tmp.image_store,
            image_cell_count: tmp.image_cell_count,
        }
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

        let old_cursor_y = self.cursor.pos.y;
        let old_cursor_x = self.cursor.pos.x;

        // Take ownership of the old rows
        let old_rows = std::mem::take(&mut self.rows);

        // 1) Group rows into logical lines based on RowJoin.
        //    While grouping, identify which logical line contains the cursor
        //    and compute the cursor's flat cell offset within that line.
        let mut logical_lines: Vec<Vec<Row>> = Vec::new();
        let mut current_line: Vec<Row> = Vec::new();

        let mut cursor_logical_line: Option<usize> = None;
        let mut cursor_flat_offset: usize = 0;

        for (old_row_idx, row) in old_rows.into_iter().enumerate() {
            if row.join == RowJoin::NewLogicalLine && !current_line.is_empty() {
                logical_lines.push(current_line);
                current_line = Vec::new();
            }

            if old_row_idx == old_cursor_y {
                cursor_logical_line = Some(logical_lines.len());
                // Flat offset = cells from preceding rows in this logical line + cursor X.
                cursor_flat_offset = current_line
                    .iter()
                    .map(|r| r.characters().len())
                    .sum::<usize>()
                    + old_cursor_x;
            }

            current_line.push(row);
        }
        if !current_line.is_empty() {
            logical_lines.push(current_line);
        }

        // 2) For each logical line, flatten its cells and re-wrap
        let mut new_rows: Vec<Row> = Vec::new();
        let mut new_cursor_y: Option<usize> = None;
        let mut new_cursor_x: Option<usize> = None;

        for (line_idx, line) in logical_lines.into_iter().enumerate() {
            // Determine origin for the first row of this logical line.
            let first_origin = line.first().map_or(RowOrigin::HardBreak, |r| r.origin);
            let is_cursor_line = cursor_logical_line == Some(line_idx);

            // Flatten all rows in this logical line into a single Vec<Cell>
            let mut flat_cells: Vec<crate::cell::Cell> = Vec::new();
            for row in &line {
                flat_cells.extend(row.characters().iter().cloned());
            }

            // Record where this logical line's new rows start (for cursor mapping).
            let line_start_idx = new_rows.len();

            if flat_cells.is_empty() {
                // Empty logical line → keep a single empty row
                new_rows.push(Row::new_with_origin(
                    new_width,
                    first_origin,
                    RowJoin::NewLogicalLine,
                ));
                if is_cursor_line {
                    new_cursor_y = Some(new_rows.len() - 1);
                    new_cursor_x = Some(old_cursor_x.min(new_width.saturating_sub(1)));
                }
                continue;
            }

            let mut idx = 0;
            let mut col = 0;
            let mut cur_cells: Vec<crate::cell::Cell> = Vec::new();
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

            // If this logical line contains the cursor, map the old cursor
            // position to the correct new row and column.
            if is_cursor_line {
                let mut flat_col: usize = 0;
                let mut found = false;
                for (i, new_row) in new_rows[line_start_idx..].iter().enumerate() {
                    let row_cells = new_row.characters().len();
                    if flat_col + row_cells > cursor_flat_offset {
                        new_cursor_y = Some(line_start_idx + i);
                        new_cursor_x = Some(cursor_flat_offset - flat_col);
                        found = true;
                        break;
                    }
                    flat_col += row_cells;
                }
                if !found {
                    // Cursor is past the end of content (in blank space).
                    // Place it on the last row of this logical line.
                    let last_row_idx = new_rows.len() - 1;
                    let last_row_len = new_rows[last_row_idx].characters().len();
                    new_cursor_y = Some(last_row_idx);
                    new_cursor_x = Some(cursor_flat_offset.saturating_sub(flat_col) + last_row_len);
                }
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

        // 4) Remap cursor position based on reflow tracking.
        if let (Some(cy), Some(cx)) = (new_cursor_y, new_cursor_x) {
            self.cursor.pos.y = cy.min(self.rows.len().saturating_sub(1));
            self.cursor.pos.x = cx.min(self.width.saturating_sub(1));
        } else if self.cursor.pos.y >= self.rows.len() {
            if self.rows.is_empty() {
                self.cursor.pos.y = 0;
            } else {
                self.cursor.pos.y = self.rows.len() - 1;
            }
            self.cursor.pos.x = 0;
        } else if self.cursor.pos.x >= self.width {
            self.cursor.pos.x = self.width.saturating_sub(1);
        }
    }

    /// Adjust the buffer rows for a new height and return the adjusted `scroll_offset`.
    pub(in crate::buffer) fn resize_height(
        &mut self,
        new_height: usize,
        scroll_offset: usize,
    ) -> usize {
        let old_height = self.height;

        if new_height > old_height {
            // Grow: add blank rows at the bottom.
            let grow = new_height - old_height;
            for _ in 0..grow {
                self.rows.push(Row::new(self.width));
                self.row_cache.push(None);
            }
            // Mark all pre-existing rows dirty so the row-level cache is
            // invalidated.  The visible window is now taller and the old
            // snapshot was built for a different height — returning stale
            // cached rows causes gaps and mispositioned content in
            // full-screen TUIs (e.g. nvim) that rely on absolute cursor
            // positioning after SIGWINCH.
            for i in 0..old_height.min(self.rows.len()) {
                self.rows[i].dirty = true;
                self.row_cache[i] = None;
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
                    self.adjust_prompt_rows(excess);
                    // Adjust cursor Y for the removed rows.
                    self.cursor.pos.y = self.cursor.pos.y.saturating_sub(excess);
                }
            } else {
                // Primary buffer: extra rows become scrollback (handled by
                // enforce_scrollback_limit later).  The cursor's absolute row
                // index is left unchanged — `clamp_cursor_after_resize()` (below)
                // ensures it stays within `rows.len()`, and `visible_window_start(0)`
                // already anchors the visible window to the bottom of the buffer.
                //
                // Previous code clamped `cursor.pos.y` to `new_height - 1`, which
                // was wrong: `cursor.pos.y` is an absolute index into `self.rows`,
                // not a screen-relative position.  That clamp moved the cursor from
                // its real row (e.g. the shell prompt) into the middle of whatever
                // content happened to be at row `new_height - 1`, causing the
                // SIGWINCH-triggered shell redraw to overwrite that content.
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

    pub(in crate::buffer) const fn clamp_cursor_after_resize(&mut self) {
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
    pub(in crate::buffer) fn enforce_scrollback_limit(&mut self, scroll_offset: usize) -> usize {
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
        self.adjust_prompt_rows(overflow);

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

    /// Set the current format tag for subsequent text insertions
    pub fn set_format(&mut self, tag: FormatTag) {
        self.current_tag = tag;
    }

    /// Get the current format tag
    #[must_use]
    pub const fn format(&self) -> &FormatTag {
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
            height: self.height,
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
}
