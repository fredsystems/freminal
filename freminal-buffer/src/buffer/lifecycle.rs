// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Buffer construction, reset, prompt-row tracking, and internal invariant
//! checking for [`Buffer`].

use freminal_common::buffer_states::{
    buffer_type::BufferType,
    cursor::CursorState,
    format_tag::FormatTag,
    modes::{decawm::Decawm, declrmm::Declrmm, decom::Decom, lnm::Lnm},
};

use crate::{
    image_store::ImageStore,
    row::{Row, RowJoin, RowOrigin},
};

use crate::buffer::Buffer;

impl Buffer {
    /// Generate default tab stops at every 8 columns for the given width.
    pub(in crate::buffer) fn default_tab_stops(width: usize) -> Vec<bool> {
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
            prompt_rows: Vec::new(),
        }
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
        self.prompt_rows.clear();
    }

    /// Record the current cursor row as a prompt-start marker.
    ///
    /// Called by `TerminalHandler` when an OSC 133 `PromptStart` fires.
    pub fn mark_prompt_row(&mut self) {
        self.prompt_rows.push(self.cursor.pos.y);
    }

    /// Buffer-relative row indices of all recorded prompt-start markers.
    #[must_use]
    pub fn prompt_rows(&self) -> &[usize] {
        &self.prompt_rows
    }

    /// Shift all prompt-row markers down by `removed` and drop any that
    /// fell below zero.  Called after draining rows from the front.
    pub(in crate::buffer) fn adjust_prompt_rows(&mut self, removed: usize) {
        self.prompt_rows.retain_mut(|r| {
            r.checked_sub(removed).is_some_and(|adjusted| {
                *r = adjusted;
                true
            })
        });
    }

    /// Internal consistency checks for debug builds.
    ///
    /// This is called from most mutating entry points. In release builds
    /// it compiles down to a no-op.
    #[cfg(debug_assertions)]
    pub(in crate::buffer) fn debug_assert_invariants(&self) {
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
    pub(in crate::buffer) fn debug_assert_invariants(&self) {}

    pub(in crate::buffer) fn push_row(&mut self, origin: RowOrigin, join: RowJoin) {
        let row = Row::new_with_origin(self.width, origin, join);
        // New rows created by scrolling (LF at bottom, auto-wrap at bottom-right)
        // use default background — NOT the current SGR background.  BCE
        // (back_color_erase) only applies to explicit erase operations (ED, EL).
        // Filling with current_tag here causes visible artifacts when programs
        // output long lines with colored backgrounds that wrap at the right margin:
        // the trailing blank cells on the wrapped continuation row retain the
        // non-default background instead of being transparent.
        self.rows.push(row);
        self.row_cache.push(None);
    }
}
