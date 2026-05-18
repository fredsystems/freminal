// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Buffer construction, reset, prompt-row tracking, and internal invariant
//! checking for [`Buffer`].

use std::collections::VecDeque;
use std::time::SystemTime;

use freminal_common::buffer_states::{
    buffer_type::BufferType,
    command_block::{CommandBlock, CommandBlockId},
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
            auto_detect_urls: true,
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
            command_blocks: VecDeque::new(),
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
        self.command_blocks.clear();
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
    ///
    /// Also adjusts `command_blocks`: blocks whose `prompt_start_row` is less
    /// than `removed` have fully scrolled out and are removed.  All row
    /// indices on surviving blocks are shifted down by `removed`.
    pub(in crate::buffer) fn adjust_prompt_rows(&mut self, removed: usize) {
        self.prompt_rows.retain_mut(|r| {
            r.checked_sub(removed).is_some_and(|adjusted| {
                *r = adjusted;
                true
            })
        });

        self.command_blocks.retain_mut(|b| {
            if b.prompt_start_row < removed {
                // Block has fully scrolled out of the buffer.
                return false;
            }
            b.prompt_start_row = b.prompt_start_row.saturating_sub(removed);
            b.command_start_row = b.command_start_row.map(|r| r.saturating_sub(removed));
            b.output_start_row = b.output_start_row.map(|r| r.saturating_sub(removed));
            b.end_row = b.end_row.map(|r| r.saturating_sub(removed));
            true
        });
    }

    // ── OSC 133 command-block API ────────────────────────────────────────────

    /// Append a fresh [`CommandBlock`] to the end of `command_blocks`, with
    /// `prompt_start_row = cursor.pos.y` and the given `cwd`.  Allocates a
    /// new [`CommandBlockId`] via [`CommandBlockId::next`].
    ///
    /// When the deque has already reached the scrollback cap, the oldest
    /// block is evicted (`pop_front`) before the new one is pushed.
    ///
    /// Returns the id of the new block so callers can correlate events (e.g.
    /// for emitting `WindowCommand::CommandFinished`).
    pub fn start_command_block(&mut self, cwd: Option<String>) -> CommandBlockId {
        // Cap command_blocks at the scrollback limit to bound memory.  We use
        // scrollback_limit as the cap because it already governs how many rows
        // (and therefore how many past prompts) the user can scroll back to see.
        // One block per prompt is a natural pairing: evicting blocks at the same
        // rate as rows prevents unbounded growth without a separate constant.
        let cap = self.scrollback_limit;
        if self.command_blocks.len() >= cap {
            self.command_blocks.pop_front();
        }
        let block = CommandBlock::new_running(self.cursor.pos.y, cwd);
        let id = block.id;
        self.command_blocks.push_back(block);
        id
    }

    /// Set `command_start_row` to the current cursor row on the most recent
    /// block whose `command_start_row` is `None`.  No-op if no such block
    /// exists (e.g. `B` arrived before `A`).
    pub fn mark_command_start_row(&mut self) {
        for block in self.command_blocks.iter_mut().rev() {
            if block.command_start_row.is_none() {
                block.command_start_row = Some(self.cursor.pos.y);
                return;
            }
        }
    }

    /// Set `output_start_row` to the current cursor row on the most recent
    /// block whose `output_start_row` is `None`.  No-op if no such block
    /// exists.
    pub fn mark_output_start_row(&mut self) {
        for block in self.command_blocks.iter_mut().rev() {
            if block.output_start_row.is_none() {
                block.output_start_row = Some(self.cursor.pos.y);
                return;
            }
        }
    }

    /// Finish the most recent open block (one whose `end_row` is `None`) by
    /// setting `end_row = cursor.pos.y`, `exit_code = exit_code`, and
    /// `finished_at = Some(SystemTime::now())`.  No-op if no open block
    /// exists.
    ///
    /// Returns a clone of the finished block (so the handler can forward it
    /// via `WindowCommand::CommandFinished`), or `None` if no-op.
    #[must_use]
    pub fn finish_command_block(&mut self, exit_code: Option<i32>) -> Option<CommandBlock> {
        for block in self.command_blocks.iter_mut().rev() {
            if block.end_row.is_none() {
                block.end_row = Some(self.cursor.pos.y);
                block.exit_code = exit_code;
                block.finished_at = Some(SystemTime::now());
                return Some(block.clone());
            }
        }
        None
    }

    /// Read-only view of all stored command blocks, oldest first.
    #[must_use]
    pub const fn command_blocks(&self) -> &VecDeque<CommandBlock> {
        &self.command_blocks
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

// ============================================================================
// Unit tests for command-block lifecycle methods
// ============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod command_block_tests {
    use super::*;
    use freminal_common::buffer_states::command_block::CommandStatus;

    /// Create a fresh buffer with a known scrollback limit.
    fn make_buf() -> Buffer {
        Buffer::new(80, 24)
    }

    // ── 1: start_command_block initializes correctly ─────────────────────

    #[test]
    fn start_command_block_initializes_correctly() {
        let mut buf = make_buf();
        // Place cursor at row 5.
        buf.cursor.pos.y = 5;

        let _id = buf.start_command_block(Some("/x".to_string()));

        assert_eq!(buf.command_blocks.len(), 1);
        let block = buf.command_blocks.front().unwrap();
        assert_eq!(block.status(), CommandStatus::Running);
        assert_eq!(block.prompt_start_row, 5);
        assert_eq!(block.cwd.as_deref(), Some("/x"));
        assert!(block.command_start_row.is_none());
        assert!(block.output_start_row.is_none());
        assert!(block.end_row.is_none());
    }

    // ── 2: start_command_block returns a monotonically increasing id ─────

    #[test]
    fn start_command_block_returns_increasing_ids() {
        let mut buf = make_buf();
        let id1 = buf.start_command_block(None);
        let id2 = buf.start_command_block(None);
        let id3 = buf.start_command_block(None);
        assert!(id1 < id2, "ids must be strictly increasing");
        assert!(id2 < id3, "ids must be strictly increasing");
    }

    // ── 3: mark_command_start_row sets field on newest open block ────────

    #[test]
    fn mark_command_start_row_sets_field() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 2;
        let _id = buf.start_command_block(None);

        buf.cursor.pos.y = 3;
        buf.mark_command_start_row();

        let block = buf.command_blocks.front().unwrap();
        assert_eq!(block.command_start_row, Some(3));
    }

    // ── 4: mark_command_start_row no-op when no open block ───────────────

    #[test]
    fn mark_command_start_row_noop_when_empty() {
        let mut buf = make_buf();
        // No blocks at all — must not panic.
        buf.mark_command_start_row();
        assert!(buf.command_blocks.is_empty());
    }

    // ── 5: mark_output_start_row analogous to test 3 ─────────────────────

    #[test]
    fn mark_output_start_row_sets_field() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 4;
        let _id = buf.start_command_block(None);

        buf.cursor.pos.y = 6;
        buf.mark_output_start_row();

        let block = buf.command_blocks.front().unwrap();
        assert_eq!(block.output_start_row, Some(6));
    }

    // ── 6: finish_command_block full A→B→C→D cycle ───────────────────────

    #[test]
    fn finish_command_block_full_lifecycle() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 0;
        let _id = buf.start_command_block(None); // A

        buf.cursor.pos.y = 1;
        buf.mark_command_start_row(); // B

        buf.cursor.pos.y = 2;
        buf.mark_output_start_row(); // C

        buf.cursor.pos.y = 5;
        let finished = buf.finish_command_block(Some(0)).unwrap(); // D

        assert_eq!(finished.end_row, Some(5));
        assert_eq!(finished.exit_code, Some(0));
        assert!(finished.finished_at.is_some());
        assert_eq!(finished.status(), CommandStatus::Success);

        // The block in the deque must also be updated.
        let stored = buf.command_blocks.front().unwrap();
        assert_eq!(stored.end_row, Some(5));
        assert_eq!(stored.exit_code, Some(0));
        assert_eq!(stored.status(), CommandStatus::Success);
    }

    // ── 7: finish_command_block no-op when no open block ─────────────────

    #[test]
    fn finish_command_block_noop_when_empty() {
        let mut buf = make_buf();
        let result = buf.finish_command_block(Some(0));
        assert!(result.is_none());
        assert!(buf.command_blocks.is_empty());
    }

    // ── 8: finish_command_block finishes only the most recent open block ─

    #[test]
    fn finish_command_block_finishes_most_recent() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 0;
        let _id1 = buf.start_command_block(None); // first A — never finished

        buf.cursor.pos.y = 2;
        let _id2 = buf.start_command_block(None); // second A

        buf.cursor.pos.y = 4;
        let finished = buf.finish_command_block(Some(0)).unwrap();

        // The returned block must be the second one.
        assert_eq!(finished.prompt_start_row, 2);
        assert_eq!(finished.end_row, Some(4));

        // The first block must still be Running.
        let first = buf.command_blocks.front().unwrap();
        assert_eq!(first.prompt_start_row, 0);
        assert_eq!(first.status(), CommandStatus::Running);
    }

    // ── 9: command_blocks() returns deque in insertion order ─────────────

    #[test]
    fn command_blocks_returns_insertion_order() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 0;
        let id1 = buf.start_command_block(None);
        buf.cursor.pos.y = 5;
        let id2 = buf.start_command_block(None);
        buf.cursor.pos.y = 10;
        let id3 = buf.start_command_block(None);

        let blocks: Vec<_> = buf.command_blocks().iter().map(|b| b.id).collect();
        assert_eq!(blocks, vec![id1, id2, id3]);
    }

    // ── 10: adjust_prompt_rows removes fully-scrolled-out blocks ─────────

    #[test]
    fn adjust_prompt_rows_removes_scrolled_out_blocks() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 5;
        let _id = buf.start_command_block(None);
        buf.cursor.pos.y = 10;
        let _finished = buf.finish_command_block(Some(0));

        // Remove 20 rows from the front — block at rows 5..10 is gone.
        buf.adjust_prompt_rows(20);

        assert!(
            buf.command_blocks.is_empty(),
            "block should be evicted when its prompt row scrolls out"
        );
    }

    // ── 11: adjust_prompt_rows shifts surviving blocks ────────────────────

    #[test]
    fn adjust_prompt_rows_shifts_surviving_blocks() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 30;
        let _id = buf.start_command_block(None);
        buf.cursor.pos.y = 35;
        buf.mark_command_start_row();
        buf.cursor.pos.y = 40;
        let _finished = buf.finish_command_block(Some(0));

        // Remove 10 rows — block should survive and shift down by 10.
        buf.adjust_prompt_rows(10);

        assert_eq!(buf.command_blocks.len(), 1);
        let block = buf.command_blocks.front().unwrap();
        assert_eq!(block.prompt_start_row, 20);
        assert_eq!(block.command_start_row, Some(25));
        assert_eq!(block.end_row, Some(30));
    }

    // ── 12: clear() empties command_blocks ───────────────────────────────

    #[test]
    fn full_reset_clears_command_blocks() {
        let mut buf = make_buf();
        buf.start_command_block(None);
        buf.start_command_block(None);
        assert!(!buf.command_blocks.is_empty());

        buf.full_reset();

        assert!(
            buf.command_blocks.is_empty(),
            "full_reset must clear command_blocks"
        );
    }

    // ── 13: scrollback cap enforced ───────────────────────────────────────

    #[test]
    fn scrollback_cap_enforced() {
        let mut buf = make_buf();
        let cap = buf.scrollback_limit;

        // Insert cap + 5 blocks without finishing them.
        for _ in 0..cap + 5 {
            buf.start_command_block(None);
        }

        assert_eq!(
            buf.command_blocks.len(),
            cap,
            "deque length must not exceed scrollback_limit"
        );
    }
}
