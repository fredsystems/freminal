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
            // Compiled-in fallback used when no config value is supplied; kept
            // in sync with `ScrollbackConfig::default` (Task 118 raised both
            // from 4000 to 10000 — see that impl for the data-backed rationale).
            scrollback_limit: 10_000,
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

    /// Drop prompt-row markers and command blocks whose `prompt_start_row`
    /// falls within `[visible_start, visible_end)`, and clamp surviving
    /// blocks whose later row fields fell inside the erased range.
    ///
    /// Called from [`Buffer::erase_display`] (CSI 2J) so that the duration
    /// overlay and command-block gutters do not continue to point at rows
    /// that the user just blanked with `clear`.
    ///
    /// Blocks anchored entirely in scrollback (`prompt_start_row <
    /// visible_start`) survive untouched.  Blocks anchored on screen are
    /// dropped wholesale; partial-scrollback / partial-visible blocks have
    /// their `command_start_row`, `output_start_row`, and `end_row` clamped
    /// back to the last surviving row when those fields land inside the
    /// erased range.
    pub(in crate::buffer) fn drop_command_blocks_in_visible_window(
        &mut self,
        visible_start: usize,
        visible_end: usize,
    ) {
        self.prompt_rows
            .retain(|r| *r < visible_start || *r >= visible_end);

        let last_surviving = visible_start.saturating_sub(1);
        self.command_blocks.retain_mut(|b| {
            if b.prompt_start_row >= visible_start && b.prompt_start_row < visible_end {
                // Block was started on a row that just got blanked.
                return false;
            }
            // Block survives (prompt is in scrollback).  Clamp later fields
            // that pointed into the erased range so the block's row span
            // does not include now-blank rows.
            let clamp = |r: usize| -> usize {
                if r >= visible_start && r < visible_end {
                    last_surviving
                } else {
                    r
                }
            };
            b.command_start_row = b.command_start_row.map(clamp);
            b.output_start_row = b.output_start_row.map(clamp);
            b.end_row = b.end_row.map(clamp);
            true
        });
    }

    // ── OSC 133 command-block API ────────────────────────────────────────────

    /// Append a fresh [`CommandBlock`] to the end of `command_blocks`, with
    /// `prompt_start_row = cursor.pos.y`, the given `cwd`, and the given
    /// freminal correlation `fid`.  Allocates a new [`CommandBlockId`] via
    /// [`CommandBlockId::next`].
    ///
    /// When the deque has already reached the scrollback cap, the oldest
    /// block is evicted (`pop_front`) before the new one is pushed.
    ///
    /// Returns the id of the new block so callers can correlate events (e.g.
    /// for emitting `WindowCommand::CommandFinished`).
    pub fn start_command_block(&mut self, cwd: Option<String>, fid: String) -> CommandBlockId {
        // Cap command_blocks at the scrollback limit to bound memory.  We use
        // scrollback_limit as the cap because it already governs how many rows
        // (and therefore how many past prompts) the user can scroll back to see.
        // One block per prompt is a natural pairing: evicting blocks at the same
        // rate as rows prevents unbounded growth without a separate constant.
        let cap = self.scrollback_limit;
        if self.command_blocks.len() >= cap {
            self.command_blocks.pop_front();
        }
        let block = CommandBlock::new_running(self.cursor.pos.y, cwd, fid);
        let id = block.id;
        self.command_blocks.push_back(block);
        id
    }

    /// Set `command_start_row` to the current cursor row on the block whose
    /// `fid` matches and whose `command_start_row` is `None`.  Searches
    /// newest-to-oldest so that the most recent matching block is updated.
    /// No-op if no matching block exists (e.g. `B` arrived before `A` from us,
    /// or a foreign `B` marker slipped through).
    pub fn mark_command_start_row(&mut self, fid: &str) {
        for block in self.command_blocks.iter_mut().rev() {
            if block.fid == fid {
                if block.command_start_row.is_none() {
                    block.command_start_row = Some(self.cursor.pos.y);
                }
                return;
            }
        }
        // No matching block — silently no-op.
    }

    /// Set `output_start_row` to the current cursor row on the block whose
    /// `fid` matches and whose `output_start_row` is `None`.  Searches
    /// newest-to-oldest.  No-op if no matching block exists.
    ///
    /// Also stamps `executed_at = SystemTime::now()` — this is the moment the
    /// command begins executing (`OSC 133 C`), which anchors the command's
    /// duration (see [`CommandBlock::duration`]).  The user's typing time at
    /// the prompt (`started_at` -> `executed_at`) is thereby excluded.
    pub fn mark_output_start_row(&mut self, fid: &str) {
        for block in self.command_blocks.iter_mut().rev() {
            if block.fid == fid {
                if block.output_start_row.is_none() {
                    block.output_start_row = Some(self.cursor.pos.y);
                    block.executed_at = Some(SystemTime::now());
                }
                return;
            }
        }
        // No matching block — silently no-op.
    }

    /// Finish the block whose `fid` matches and whose `end_row` is `None`,
    /// by setting `end_row = cursor.pos.y`, `exit_code`, and
    /// `finished_at = Some(SystemTime::now())`.  Searches newest-to-oldest.
    /// No-op if no matching open block exists.
    ///
    /// Returns a clone of the finished block (so the handler can forward it
    /// via `WindowCommand::CommandFinished`), or `None` if no-op.
    #[must_use]
    pub fn finish_command_block(
        &mut self,
        exit_code: Option<i32>,
        fid: &str,
    ) -> Option<CommandBlock> {
        for block in self.command_blocks.iter_mut().rev() {
            if block.fid == fid && block.end_row.is_none() {
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

        let _id = buf.start_command_block(Some("/x".to_string()), "fid1".to_owned());

        assert_eq!(buf.command_blocks.len(), 1);
        let block = buf.command_blocks.front().unwrap();
        assert_eq!(block.status(), CommandStatus::Running);
        assert_eq!(block.prompt_start_row, 5);
        assert_eq!(block.cwd.as_deref(), Some("/x"));
        assert_eq!(block.fid, "fid1");
        assert!(block.command_start_row.is_none());
        assert!(block.output_start_row.is_none());
        assert!(block.end_row.is_none());
    }

    // ── 2: start_command_block returns a monotonically increasing id ─────

    #[test]
    fn start_command_block_returns_increasing_ids() {
        let mut buf = make_buf();
        let id1 = buf.start_command_block(None, "fid1".to_owned());
        let id2 = buf.start_command_block(None, "fid2".to_owned());
        let id3 = buf.start_command_block(None, "fid3".to_owned());
        assert!(id1 < id2, "ids must be strictly increasing");
        assert!(id2 < id3, "ids must be strictly increasing");
    }

    // ── 3: mark_command_start_row sets field on matching block ───────────

    #[test]
    fn mark_command_start_row_sets_field() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 2;
        let _id = buf.start_command_block(None, "fid1".to_owned());

        buf.cursor.pos.y = 3;
        buf.mark_command_start_row("fid1");

        let block = buf.command_blocks.front().unwrap();
        assert_eq!(block.command_start_row, Some(3));
    }

    // ── 4: mark_command_start_row no-op when no matching block ───────────

    #[test]
    fn mark_command_start_row_noop_when_empty() {
        let mut buf = make_buf();
        // No blocks at all — must not panic.
        buf.mark_command_start_row("any-fid");
        assert!(buf.command_blocks.is_empty());
    }

    // ── 5: mark_output_start_row analogous to test 3 ─────────────────────

    #[test]
    fn mark_output_start_row_sets_field() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 4;
        let _id = buf.start_command_block(None, "fid1".to_owned());

        buf.cursor.pos.y = 6;
        buf.mark_output_start_row("fid1");

        let block = buf.command_blocks.front().unwrap();
        assert_eq!(block.output_start_row, Some(6));
    }

    // ── 73.7: OSC 133 C stamps executed_at so duration excludes prompt-wait
    #[test]
    fn mark_output_start_row_stamps_executed_at() {
        let mut buf = make_buf();
        let _id = buf.start_command_block(None, "fid1".to_owned());
        let block = buf.command_blocks.front().unwrap();
        let started_at = block.started_at;
        assert!(
            block.executed_at.is_none(),
            "executed_at must be None before OSC 133 C"
        );

        buf.mark_output_start_row("fid1");

        let block = buf.command_blocks.front().unwrap();
        let executed_at = block
            .executed_at
            .expect("executed_at must be stamped at OSC 133 C");
        assert!(
            executed_at >= started_at,
            "executed_at must be at or after started_at"
        );
    }

    // ── 6: finish_command_block full A→B→C→D cycle ───────────────────────

    #[test]
    fn finish_command_block_full_lifecycle() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 0;
        let _id = buf.start_command_block(None, "fid1".to_owned()); // A

        buf.cursor.pos.y = 1;
        buf.mark_command_start_row("fid1"); // B

        buf.cursor.pos.y = 2;
        buf.mark_output_start_row("fid1"); // C

        buf.cursor.pos.y = 5;
        let finished = buf.finish_command_block(Some(0), "fid1").unwrap(); // D

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
        let result = buf.finish_command_block(Some(0), "fid1");
        assert!(result.is_none());
        assert!(buf.command_blocks.is_empty());
    }

    // ── 8: finish_command_block matches by fid, not most-recent ──────────

    #[test]
    fn finish_command_block_matches_by_fid() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 0;
        let _id1 = buf.start_command_block(None, "fid-a".to_owned()); // first A

        buf.cursor.pos.y = 2;
        let _id2 = buf.start_command_block(None, "fid-b".to_owned()); // second A

        buf.cursor.pos.y = 4;
        // Finish by fid "fid-b" — the second block, not the first.
        let finished = buf.finish_command_block(Some(0), "fid-b").unwrap();

        // The returned block must be the second one (fid-b).
        assert_eq!(finished.fid, "fid-b");
        assert_eq!(finished.prompt_start_row, 2);
        assert_eq!(finished.end_row, Some(4));

        // The first block must still be Running.
        let first = buf.command_blocks.front().unwrap();
        assert_eq!(first.fid, "fid-a");
        assert_eq!(first.status(), CommandStatus::Running);
    }

    // ── 9: command_blocks() returns deque in insertion order ─────────────

    #[test]
    fn command_blocks_returns_insertion_order() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 0;
        let id1 = buf.start_command_block(None, "fid1".to_owned());
        buf.cursor.pos.y = 5;
        let id2 = buf.start_command_block(None, "fid2".to_owned());
        buf.cursor.pos.y = 10;
        let id3 = buf.start_command_block(None, "fid3".to_owned());

        let blocks: Vec<_> = buf.command_blocks().iter().map(|b| b.id).collect();
        assert_eq!(blocks, vec![id1, id2, id3]);
    }

    // ── 10: adjust_prompt_rows removes fully-scrolled-out blocks ─────────

    #[test]
    fn adjust_prompt_rows_removes_scrolled_out_blocks() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 5;
        let _id = buf.start_command_block(None, "fid1".to_owned());
        buf.cursor.pos.y = 10;
        let _finished = buf.finish_command_block(Some(0), "fid1");

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
        let _id = buf.start_command_block(None, "fid1".to_owned());
        buf.cursor.pos.y = 35;
        buf.mark_command_start_row("fid1");
        buf.cursor.pos.y = 40;
        let _finished = buf.finish_command_block(Some(0), "fid1");

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
        buf.start_command_block(None, "fid1".to_owned());
        buf.start_command_block(None, "fid2".to_owned());
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
        for i in 0..cap + 5 {
            buf.start_command_block(None, format!("fid-{i}"));
        }

        assert_eq!(
            buf.command_blocks.len(),
            cap,
            "deque length must not exceed scrollback_limit"
        );
    }

    // ── 14: finish_by_fid_matches_correct_block ───────────────────────────

    #[test]
    fn finish_by_fid_matches_correct_block() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 0;
        let _id_a = buf.start_command_block(None, "block-a".to_owned());
        buf.cursor.pos.y = 5;
        let _id_b = buf.start_command_block(None, "block-b".to_owned());

        buf.cursor.pos.y = 8;
        // Finish "block-b" explicitly — "block-a" must remain Running.
        let finished = buf.finish_command_block(Some(0), "block-b").unwrap();
        assert_eq!(finished.fid, "block-b");
        assert_eq!(
            buf.command_blocks[0].status(),
            CommandStatus::Running,
            "block-a must still be Running"
        );
        assert_eq!(
            buf.command_blocks[1].status(),
            CommandStatus::Success,
            "block-b must be Success"
        );
    }

    // ── 15: finish_with_unknown_fid_is_noop ──────────────────────────────

    #[test]
    fn finish_with_unknown_fid_is_noop() {
        let mut buf = make_buf();
        buf.start_command_block(None, "fid-a".to_owned());

        let result = buf.finish_command_block(Some(0), "fid-z");
        assert!(
            result.is_none(),
            "finishing with an unknown fid must return None"
        );
        assert_eq!(
            buf.command_blocks[0].status(),
            CommandStatus::Running,
            "block must remain Running"
        );
    }

    // ── 16: mark_command_start_row_with_unknown_fid_is_noop ───────────────

    #[test]
    fn mark_command_start_row_with_unknown_fid_is_noop() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 2;
        buf.start_command_block(None, "fid-a".to_owned());

        buf.cursor.pos.y = 5;
        buf.mark_command_start_row("fid-z");

        // command_start_row must remain None — the call was a no-op.
        assert!(
            buf.command_blocks[0].command_start_row.is_none(),
            "command_start_row must remain None for an unmatched fid"
        );
    }

    // ── 17: mark_output_start_row_with_unknown_fid_is_noop ────────────────

    #[test]
    fn mark_output_start_row_with_unknown_fid_is_noop() {
        let mut buf = make_buf();
        buf.cursor.pos.y = 2;
        buf.start_command_block(None, "fid-a".to_owned());

        buf.cursor.pos.y = 5;
        buf.mark_output_start_row("fid-z");

        assert!(
            buf.command_blocks[0].output_start_row.is_none(),
            "output_start_row must remain None for an unmatched fid"
        );
    }

    // ── 14: erase_display drops command_blocks on visible rows ───────────

    #[test]
    fn erase_display_drops_command_blocks_anchored_on_screen() {
        // Simulate a finished command block whose prompt was on the visible
        // screen.  After `clear` (ED 2), the block should be evicted so the
        // duration overlay does not paint on the now-blank rows.
        let mut buf = make_buf();
        // Pre-grow rows so cursor positions are addressable.
        while buf.rows.len() < buf.height {
            buf.rows.push(crate::row::Row::new(buf.width));
            buf.row_cache.push(None);
        }
        buf.cursor.pos.y = 3;
        let _id = buf.start_command_block(None, "fid-clear".to_owned());
        buf.cursor.pos.y = 3;
        buf.mark_command_start_row("fid-clear");
        buf.cursor.pos.y = 4;
        buf.mark_output_start_row("fid-clear");
        buf.cursor.pos.y = 6;
        let _finished = buf.finish_command_block(Some(0), "fid-clear");
        assert_eq!(buf.command_blocks.len(), 1);

        buf.erase_display();

        assert!(
            buf.command_blocks.is_empty(),
            "erase_display must drop blocks anchored on the visible window"
        );
        assert!(
            buf.prompt_rows.is_empty(),
            "erase_display must drop prompt_rows on the visible window"
        );
    }

    #[test]
    fn erase_display_preserves_blocks_anchored_in_scrollback() {
        // A block whose prompt_start_row sits in scrollback (below
        // visible_start) must survive ED 2.  Its end_row, if it lands
        // inside the now-erased visible window, must be clamped to the
        // last surviving row.
        let mut buf = make_buf();
        // Grow the buffer so there is at least one row of scrollback above
        // the visible window.  `visible_window_start = total - height`, so
        // we need total > height to produce a non-zero scrollback.
        let target_rows = buf.height + 3;
        while buf.rows.len() < target_rows {
            buf.rows.push(crate::row::Row::new(buf.width));
            buf.row_cache.push(None);
        }
        let visible_start = buf.visible_window_start(0);
        assert!(
            visible_start > 0,
            "test prerequisite: need a non-empty scrollback"
        );

        // Manually construct a block straddling scrollback and visible.
        let block = CommandBlock {
            id: CommandBlockId::next(),
            fid: "straddle".to_owned(),
            prompt_start_row: visible_start - 1,
            command_start_row: Some(visible_start),
            output_start_row: Some(visible_start + 1),
            end_row: Some(visible_start + 3),
            started_at: SystemTime::now(),
            executed_at: Some(SystemTime::now()),
            finished_at: Some(SystemTime::now()),
            cwd: None,
            exit_code: Some(0),
        };
        buf.command_blocks.push_back(block);

        buf.erase_display();

        assert_eq!(buf.command_blocks.len(), 1, "scrollback block must survive");
        let b = &buf.command_blocks[0];
        assert_eq!(b.prompt_start_row, visible_start - 1);
        assert_eq!(
            b.end_row,
            Some(visible_start - 1),
            "end_row inside erased range must clamp to last surviving row"
        );
    }
}
