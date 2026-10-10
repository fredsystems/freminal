// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Buffer construction, reset, prompt-row tracking, and internal invariant
//! checking for [`Buffer`].

use std::collections::{HashMap, VecDeque};
use std::time::SystemTime;

use freminal_common::buffer_states::{
    buffer_type::BufferType,
    command_block::{CommandBlock, CommandBlockId},
    cursor::CursorState,
    format_tag::FormatTag,
    modes::{decawm::Decawm, declrmm::Declrmm, decom::Decom, lnm::Lnm},
    row_number::RowNumber,
};

use crate::{
    image_store::ImageStore,
    row::{Row, RowJoin, RowOrigin},
};

use super::command_block_log::CommandBlockLog;
use crate::buffer::{Buffer, CommandBlocksGeneration, RowStore};

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
        let rows = std::iter::once(Row::new(width)).collect();

        Self {
            rows,
            merge_cache: None,
            row_epoch_counter: 0,
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
            parked_primary: None,
            parked_alternate: None,
            next_alt_base: RowNumber::ALTERNATE_BASE,
            pending_reflow_remap: None,
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
            command_blocks: CommandBlockLog::new(),
            blocks: HashMap::new(),
            next_block_id: 0,
            decompress_scratch: Vec::new(),
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
    ///
    /// Row numbers are never reset: the fresh screen is numbered from past the
    /// last row number issued on the screen being discarded, so a stale
    /// [`RowNumber`] can never alias a post-reset row (Task 125.14).
    pub fn full_reset(&mut self) {
        // The primary namespace continues past the primary content being
        // discarded (which, on the alternate screen, is parked in
        // `parked_primary`); the alternate namespace continues past whichever
        // alternate store is being discarded, the live one or the parked one,
        // so alternate numbering stays monotonic across the reset.
        //
        // The alternate screen is only ever entered through
        // `switch_to_alternate`, which parks the primary store, so on the
        // alternate screen it is always present. If that invariant were ever
        // broken the primary numbering is unrecoverable; the fallback
        // restarts it at zero, which can alias only numbers held outside the
        // buffer, and RIS clears every such holder (the marks below, the
        // handler's kitty placements).
        self.debug_assert_screen_parking();
        let primary_next = match self.kind {
            BufferType::Alternate => self
                .parked_primary
                .as_ref()
                .map_or(RowNumber::ZERO, |parked| parked.rows.next_number()),
            BufferType::Primary => self.rows.next_number(),
        };
        let discarded_alternate_next = match self.kind {
            BufferType::Alternate => Some(self.rows.next_number()),
            BufferType::Primary => self
                .parked_alternate
                .as_ref()
                .map(|parked| parked.rows.next_number()),
        };
        if let Some(next) = discarded_alternate_next {
            self.next_alt_base = self.next_alt_base.max(next);
        }
        self.rows = RowStore::from_rows_at(primary_next, [Row::new(self.width)]);
        // Task 121 Part C: the row cache above was just replaced wholesale
        // with fresh, unrelated content — a stale `merge_cache` (even one
        // whose `fp` coincidentally still matches, e.g. an unchanged
        // width/height reset) must not be reused against it.
        self.merge_cache = None;
        // Task 119: discard every compressed block and reset the per-row
        // id counter alongside the row reset above (which already reset the
        // per-row block map) — a full
        // reset wipes all scrollback, so no compressed content survives it.
        self.blocks.clear();
        self.next_block_id = 0;
        self.cursor = CursorState::default();
        self.current_tag = FormatTag::default();
        self.kind = BufferType::Primary;
        self.parked_primary = None;
        self.parked_alternate = None;
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
        self.pending_reflow_remap = None;
    }

    /// Record the current cursor row as a prompt-start marker.
    ///
    /// Called by `TerminalHandler` when an OSC 133 `PromptStart` fires.
    pub fn mark_prompt_row(&mut self) {
        self.prompt_rows.push(self.cursor_row_number());
    }

    /// Logical row numbers of all recorded prompt-start markers.
    ///
    /// Stable across scrollback eviction: a number is never rewritten, it
    /// simply falls below [`Buffer::row_base`] once its row is evicted
    /// (convert with [`Buffer::row_index_of`], which yields `None` for such a
    /// number). Order is the order the markers fired, which is not guaranteed
    /// to be ascending.
    #[must_use]
    pub fn prompt_rows(&self) -> &[RowNumber] {
        &self.prompt_rows
    }

    /// Drop the prompt marks and command blocks whose rows have been evicted.
    ///
    /// Called after every front eviction. Row numbers are stable, so nothing
    /// is rewritten: this only trims the leading run of marks that now lie
    /// below [`Buffer::row_base`] (O(1) when there is nothing to trim). A
    /// block is dropped when its *prompt* row is evicted, as before.
    ///
    /// Marks are appended in the order they fire, which is usually but not
    /// always ascending, so a stale mark behind a retained one stays until the
    /// retained one is itself evicted; consumers must filter with
    /// [`Buffer::row_index_of`] / `RowNumber::rows_after`.
    ///
    /// Only marks in the same namespace as the active store are considered:
    /// primary marks held while the alternate screen is up are not "below"
    /// the alternate base, they are simply another screen's.
    pub(in crate::buffer) fn prune_evicted_marks(&mut self) {
        self.prune_marks_below(self.rows.base());
    }

    /// [`Self::prune_evicted_marks`] against an explicit `base`, for the one
    /// case where rows are evicted from a store other than the active one: a
    /// resize on the alternate screen shrinks the parked primary store.
    pub(in crate::buffer) fn prune_marks_below(&mut self, base: RowNumber) {
        let evicted = |row: RowNumber| row.is_alternate() == base.is_alternate() && row < base;

        let stale = self
            .prompt_rows
            .iter()
            .take_while(|&&row| evicted(row))
            .count();
        if stale > 0 {
            self.prompt_rows.drain(..stale);
        }

        while self
            .command_blocks
            .front()
            .is_some_and(|block| evicted(block.prompt_start_row))
        {
            self.command_blocks.pop_front();
        }
    }

    /// Forget every mark at or past `next` in the active screen's namespace:
    /// rows that no longer exist and whose numbers are about to be issued
    /// again.
    ///
    /// Called after trailing blank padding rows are popped
    /// ([`RowStore::pop`] re-issues a popped row's number). A prompt mark on a
    /// popped row is dropped, as is a command block that started there; a
    /// later boundary of a surviving block that fell on a popped row is
    /// clamped to the last row that still exists, the way an erased range is
    /// (`drop_command_blocks_in_visible_window`).
    pub(in crate::buffer) fn prune_marks_from(&mut self, next: RowNumber) {
        let gone = |row: RowNumber| row.is_alternate() == next.is_alternate() && row >= next;

        self.prompt_rows.retain(|&row| !gone(row));

        // There is always a row below `next` (padding is only popped down to
        // the cursor row), but `base` is a safe floor regardless.
        let last_surviving = next.offset(-1).max(self.rows.base());
        self.command_blocks.retain_mut(|block| {
            if gone(block.prompt_start_row) {
                return false;
            }
            let clamp = |row: RowNumber| if gone(row) { last_surviving } else { row };
            block.command_start_row = block.command_start_row.map(clamp);
            block.output_start_row = block.output_start_row.map(clamp);
            block.end_row = block.end_row.map(clamp);
            true
        });
    }

    /// Forget every mark that belongs to the alternate screen: the alternate
    /// screen's rows are discarded when it is left, so a mark in that
    /// namespace has nothing left to point at.
    ///
    /// A block that started on the primary screen but recorded a later
    /// boundary on the alternate screen keeps its primary fields and loses the
    /// alternate ones.
    ///
    /// Note the interplay for a block finished while on the alternate screen:
    /// [`Buffer::finish_command_block`] stamps `finished_at` and `exit_code`
    /// and sets `end_row` to an alternate number, so clearing `end_row` here
    /// leaves a block that is finished (`finished_at.is_some()`, so its status
    /// is not `Running`) but has `end_row == None`. Consumers keyed on
    /// `end_row.is_none()` therefore see it as having no row extent (not
    /// foldable, gutter extends to the running extent), and the close guard,
    /// which reads `status()` plus `output_start_row.is_some()`, does not
    /// treat it as running because `status()` comes from `finished_at`. A
    /// cleared `output_start_row` likewise means "no recorded output".
    /// `finish_command_block` selects open blocks by `end_row.is_none()`, so a
    /// later `D` carrying the same `fid` could match such a block again.
    pub(in crate::buffer) fn drop_alternate_marks(&mut self) {
        self.prompt_rows.retain(|row| !row.is_alternate());
        self.command_blocks.retain_mut(|block| {
            if block.prompt_start_row.is_alternate() {
                return false;
            }
            for field in [
                &mut block.command_start_row,
                &mut block.output_start_row,
                &mut block.end_row,
            ] {
                if field.is_some_and(RowNumber::is_alternate) {
                    *field = None;
                }
            }
            true
        });
    }

    /// Drop prompt-row markers and command blocks whose `prompt_start_row`
    /// falls within the retained-index range `[visible_start, visible_end)`
    /// of the active screen, and clamp surviving blocks whose later row
    /// fields fell inside the erased range.
    ///
    /// The range is converted to logical row numbers first, so only marks on
    /// the active screen can match: erasing the alternate screen (ED 2) never
    /// touches a primary-screen block.
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
        let start = self.rows.number_of(visible_start);
        let end = self.rows.number_of(visible_end);
        let in_erased = |row: RowNumber| row >= start && row < end;

        self.prompt_rows.retain(|r| !in_erased(*r));

        // The last row above the erased range; the oldest row itself when the
        // range starts there (there is nothing above it to clamp to).
        let last_surviving = if start > self.rows.base() {
            start.offset(-1)
        } else {
            start
        };
        self.command_blocks.retain_mut(|b| {
            if in_erased(b.prompt_start_row) {
                // Block was started on a row that just got blanked.
                return false;
            }
            // Block survives (prompt is in scrollback).  Clamp later fields
            // that pointed into the erased range so the block's row span
            // does not include now-blank rows.
            let clamp =
                |r: RowNumber| -> RowNumber { if in_erased(r) { last_surviving } else { r } };
            b.command_start_row = b.command_start_row.map(clamp);
            b.output_start_row = b.output_start_row.map(clamp);
            b.end_row = b.end_row.map(clamp);
            true
        });
    }

    // ── OSC 133 command-block API ────────────────────────────────────────────

    /// Append a fresh [`CommandBlock`] to the end of `command_blocks`, with
    /// `prompt_start_row` set to the cursor's row number, the given `cwd`, and the given
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
        let block = CommandBlock::new_running(self.cursor_row_number(), cwd, fid);
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
        let row = self.cursor_row_number();
        for block in self.command_blocks.iter_mut().rev() {
            if block.fid == fid {
                if block.command_start_row.is_none() {
                    block.command_start_row = Some(row);
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
        let row = self.cursor_row_number();
        for block in self.command_blocks.iter_mut().rev() {
            if block.fid == fid {
                if block.output_start_row.is_none() {
                    block.output_start_row = Some(row);
                    block.executed_at = Some(SystemTime::now());
                }
                return;
            }
        }
        // No matching block — silently no-op.
    }

    /// Finish the block whose `fid` matches and whose `end_row` is `None`,
    /// by setting `end_row` to the cursor's row number, `exit_code`, and
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
        let row = self.cursor_row_number();
        for block in self.command_blocks.iter_mut().rev() {
            if block.fid == fid && block.end_row.is_none() {
                block.end_row = Some(row);
                block.exit_code = exit_code;
                block.finished_at = Some(SystemTime::now());
                return Some(block.clone());
            }
        }
        None
    }

    /// Read-only view of all stored command blocks, oldest first.
    #[must_use]
    pub fn command_blocks(&self) -> &VecDeque<CommandBlock> {
        &self.command_blocks
    }

    /// Identifies the current contents of [`Self::command_blocks`]: it changes
    /// whenever a block is added, removed or edited, and only then. A caller
    /// that derives something expensive from the blocks (the snapshot's
    /// `Arc<[CommandBlock]>`) can keep it until this value moves.
    #[must_use]
    pub const fn command_blocks_generation(&self) -> CommandBlocksGeneration {
        self.command_blocks.generation()
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

        self.debug_assert_parked_screens();

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

        // The flatten cache and the compressed-block map are index-parallel
        // to `rows`. `RowStore` keeps them so by construction (every
        // structural edit moves all three together), so this is a
        // belt-and-braces check on that guarantee.
        debug_assert_eq!(
            self.rows.cache().len(),
            self.rows.len(),
            "row cache length {} != rows length {}",
            self.rows.cache().len(),
            self.rows.len()
        );
        debug_assert_eq!(
            self.rows.block_map().len(),
            self.rows.len(),
            "row block map length {} != rows length {}",
            self.rows.block_map().len(),
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

        self.debug_assert_block_live_rows();
        self.debug_assert_image_horizons();
    }

    /// The active screen is never also parked, and each parked store is
    /// internally consistent and the size of the screen it will become.
    ///
    /// Checked per parked screen: the flatten cache and block map are
    /// index-parallel to the rows, `image_cell_count` matches the image cells
    /// actually present, and the alternate store holds exactly `height` rows.
    /// (That a parked primary is *present* while the alternate is active is
    /// checked by [`Self::debug_assert_screen_parking`] at the switch points;
    /// the throwaway buffers `set_size` builds to resize a parked screen are
    /// alternate-kind with nothing parked, so it cannot live here.)
    #[cfg(debug_assertions)]
    fn debug_assert_parked_screens(&self) {
        match self.kind {
            BufferType::Primary => debug_assert!(
                self.parked_primary.is_none(),
                "primary screen is active but also parked"
            ),
            BufferType::Alternate => debug_assert!(
                self.parked_alternate.is_none(),
                "alternate screen is active but also parked"
            ),
        }

        for (name, parked) in [
            ("primary", self.parked_primary.as_ref()),
            ("alternate", self.parked_alternate.as_ref()),
        ] {
            let Some(parked) = parked else { continue };
            debug_assert_eq!(
                parked.rows.cache().len(),
                parked.rows.len(),
                "parked {name} row cache length != rows length"
            );
            debug_assert_eq!(
                parked.rows.block_map().len(),
                parked.rows.len(),
                "parked {name} row block map length != rows length"
            );
            let actual_image_cells: usize = parked.rows.iter().map(Row::count_image_cells).sum();
            debug_assert_eq!(
                parked.image_cell_count, actual_image_cells,
                "parked {name} image_cell_count != actual image cells"
            );
        }
        if let Some(parked) = &self.parked_alternate {
            debug_assert_eq!(
                parked.rows.len(),
                self.height,
                "parked alternate store must have exactly `height` rows"
            );
        }
    }

    /// The alternate screen is active exactly when the primary screen is
    /// parked. Called where a screen is switched or discarded.
    #[cfg(debug_assertions)]
    pub(in crate::buffer) fn debug_assert_screen_parking(&self) {
        debug_assert_eq!(
            self.kind == BufferType::Alternate,
            self.parked_primary.is_some(),
            "the primary screen must be parked exactly while the alternate is active"
        );
    }

    /// Every compressed block's `live_rows` equals the number of block-map
    /// entries naming it, and every block-map entry names a stored block
    /// (Task 125.15). O(rows).
    #[cfg(debug_assertions)]
    fn debug_assert_block_live_rows(&self) {
        let mut referenced: HashMap<crate::buffer::BlockId, u32> = HashMap::new();
        for block_ref in self.rows.block_map().iter().flatten() {
            *referenced.entry(block_ref.block_id()).or_insert(0) += 1;
        }
        debug_assert_eq!(
            referenced.len(),
            self.blocks.len(),
            "{} blocks are referenced by rows but {} are stored",
            referenced.len(),
            self.blocks.len()
        );
        for (id, slot) in &self.blocks {
            debug_assert_eq!(
                referenced.get(id).copied(),
                Some(slot.live_rows),
                "block {id:?} live_rows {} != rows referencing it {:?}",
                slot.live_rows,
                referenced.get(id)
            );
        }
    }

    /// Every image cell lies on a row at or below its image's stamp horizon
    /// (Task 125.15), which is what lets eviction free an image without
    /// scanning cells. Images absent from the store are skipped (their cells
    /// have nothing to keep alive), as are protocol-retained Kitty images (see
    /// the exemption below). O(rows x cols).
    #[cfg(debug_assertions)]
    fn debug_assert_image_horizons(&self) {
        if self.image_cell_count == 0 {
            return;
        }
        for (row_idx, row) in self.rows.iter().enumerate() {
            let number = self.rows.number_of(row_idx);
            for cell in row.cells_for_image_scan() {
                let Some(placement) = cell.image_placement() else {
                    continue;
                };
                // A Kitty image is exempt: its cells can be stamped before the
                // image is transmitted (placeholders) or outlive a removal and
                // re-transmission of the same id, so a Kitty image may
                // legitimately have no horizon covering them. Its data is
                // protocol-retained, so eviction never frees it and the
                // horizon is not needed for correctness.
                if !self.image_store.contains(placement.image_id)
                    || self.image_store.is_protocol_retained(placement.image_id)
                {
                    continue;
                }
                debug_assert!(
                    self.image_store
                        .horizon_of(placement.image_id)
                        .is_some_and(|horizon| number <= horizon),
                    "image {} has a cell on row {number} above its stamp horizon {:?}",
                    placement.image_id,
                    self.image_store.horizon_of(placement.image_id)
                );
            }
        }
    }

    // In release builds this is a no-op, so we can call it freely.
    #[cfg(not(debug_assertions))]
    #[inline]
    pub(in crate::buffer) fn debug_assert_invariants(&self) {}

    #[cfg(not(debug_assertions))]
    #[inline]
    pub(in crate::buffer) fn debug_assert_screen_parking(&self) {}

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
        assert_eq!(block.prompt_start_row, RowNumber::new(5));
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
        assert_eq!(block.command_start_row, Some(RowNumber::new(3)));
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
        assert_eq!(block.output_start_row, Some(RowNumber::new(6)));
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

        assert_eq!(finished.end_row, Some(RowNumber::new(5)));
        assert_eq!(finished.exit_code, Some(0));
        assert!(finished.finished_at.is_some());
        assert_eq!(finished.status(), CommandStatus::Success);

        // The block in the deque must also be updated.
        let stored = buf.command_blocks.front().unwrap();
        assert_eq!(stored.end_row, Some(RowNumber::new(5)));
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
        assert_eq!(finished.prompt_start_row, RowNumber::new(2));
        assert_eq!(finished.end_row, Some(RowNumber::new(4)));

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

    // ── 10: eviction pruning removes fully-scrolled-out blocks ───────────

    #[test]
    fn prune_evicted_marks_removes_scrolled_out_blocks() {
        let mut buf = make_buf();
        while buf.rows.len() < 25 {
            buf.rows.push(crate::row::Row::new(buf.width));
        }
        buf.cursor.pos.y = 5;
        buf.mark_prompt_row();
        let _id = buf.start_command_block(None, "fid1".to_owned());
        buf.cursor.pos.y = 10;
        let _finished = buf.finish_command_block(Some(0), "fid1");

        // Evict 20 rows from the front — block at rows 5..10 is gone.
        let _ = buf.evict_front_rows(20);
        buf.prune_evicted_marks();

        assert!(
            buf.command_blocks.is_empty(),
            "block should be evicted when its prompt row scrolls out"
        );
        assert!(
            buf.prompt_rows.is_empty(),
            "prompt mark should be dropped when its row scrolls out"
        );
    }

    // ── 11: eviction never rewrites surviving blocks ──────────────────────

    #[test]
    fn prune_evicted_marks_leaves_surviving_blocks_untouched() {
        let mut buf = make_buf();
        while buf.rows.len() < 45 {
            buf.rows.push(crate::row::Row::new(buf.width));
        }
        buf.cursor.pos.y = 30;
        buf.mark_prompt_row();
        let _id = buf.start_command_block(None, "fid1".to_owned());
        buf.cursor.pos.y = 35;
        buf.mark_command_start_row("fid1");
        buf.cursor.pos.y = 40;
        let _finished = buf.finish_command_block(Some(0), "fid1");

        // Evict 10 rows — the block survives and its numbers do NOT change:
        // only the row *index* of each number moves.
        let _ = buf.evict_front_rows(10);
        buf.prune_evicted_marks();

        assert_eq!(buf.command_blocks.len(), 1);
        let block = buf.command_blocks.front().unwrap();
        assert_eq!(block.prompt_start_row, RowNumber::new(30));
        assert_eq!(block.command_start_row, Some(RowNumber::new(35)));
        assert_eq!(block.end_row, Some(RowNumber::new(40)));
        assert_eq!(buf.prompt_rows(), &[RowNumber::new(30)]);
        // The retained indices moved down by the 10 evicted rows.
        assert_eq!(buf.row_index_of(block.prompt_start_row), Some(20));
        assert_eq!(buf.row_index_of(RowNumber::new(35)), Some(25));
        assert_eq!(buf.row_index_of(RowNumber::new(40)), Some(30));
        // An evicted number resolves to nothing.
        assert_eq!(buf.row_index_of(RowNumber::new(9)), None);
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
            prompt_start_row: buf.row_number_at(visible_start - 1),
            command_start_row: Some(buf.row_number_at(visible_start)),
            output_start_row: Some(buf.row_number_at(visible_start + 1)),
            end_row: Some(buf.row_number_at(visible_start + 3)),
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
        assert_eq!(b.prompt_start_row, buf.row_number_at(visible_start - 1));
        assert_eq!(
            b.end_row,
            Some(buf.row_number_at(visible_start - 1)),
            "end_row inside erased range must clamp to last surviving row"
        );
    }
}
