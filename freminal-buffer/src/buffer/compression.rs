// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Scrollback block compression (Task 119 — Scrollback Compression).
//!
//! Deep-cold scrollback rows (already Task-118 [`Row::is_compact`]) can be
//! moved out of `Buffer::rows` entirely into an LZ4-compressed
//! [`CompressedBlock`], via the explicit, test-driven
//! [`Buffer::compress_scrollback_block`]. There is no idle-driven *policy*
//! deciding when to call it yet — that is Task 119.5. Compressed content is
//! transparently restored at the flatten/read boundary via
//! [`Buffer::ensure_decompressed`], so no caller outside `crate::buffer`
//! ever observes a row being compressed.
//!
//! ## Single residency
//!
//! A row is always in exactly one of three states: `Live`, Task-118
//! `Compact` (in `self.rows[i]`, `row_block_map[i] == None`), or compressed
//! (`row_block_map[i] == Some(_)`, real content lives only in
//! `self.blocks`). [`Buffer::ensure_decompressed`] restores a touched block
//! back to `Compact` (not `Live` — preserving the Task-118 memory win) and
//! removes it from `self.blocks`, so a block is never both compressed and
//! live at the same time.

use std::collections::HashSet;
use std::ops::Range;

use conv2::ValueFrom;

use crate::cell::Cell;
use crate::compact_row::CompactRow;
use crate::compressed_block::CompressedBlock;

use super::{BlockId, BlockRowRef, Buffer};

impl Buffer {
    /// Pad `row_block_map` up to `self.rows.len()` with `None`, healing any
    /// lag introduced by the handful of row-append call sites outside this
    /// module (see the field doc on `Buffer::row_block_map`). A cheap no-op
    /// once `row_block_map` has already caught up, which is the common case
    /// — every append site this module (and `resize_and_alt.rs`/`scroll.rs`)
    /// owns keeps it eagerly in lockstep.
    pub(in crate::buffer) fn sync_row_block_map_len(&mut self) {
        if self.row_block_map.len() < self.rows.len() {
            self.row_block_map.resize(self.rows.len(), None);
        }
    }

    /// Drop every entry in `self.blocks` that is no longer referenced by any
    /// entry in `self.row_block_map`.
    ///
    /// A drain (`Buffer::enforce_scrollback_limit`'s front-of-buffer
    /// eviction, `Buffer::erase_scrollback`, `Buffer::scroll_up`) can remove
    /// every row that referenced a given compressed block without ever
    /// calling `Buffer::ensure_decompressed` on it — the block's bytes would
    /// otherwise sit in `self.blocks` forever, unreferenced and
    /// unreachable, leaking exactly the memory compression exists to save.
    /// Called after every such drain to keep `self.blocks` containing only
    /// live, referenced blocks.
    pub(in crate::buffer) fn gc_unreferenced_blocks(&mut self) {
        if self.blocks.is_empty() {
            return;
        }
        let referenced: HashSet<BlockId> = self
            .row_block_map
            .iter()
            .filter_map(|entry| entry.map(BlockRowRef::block_id))
            .collect();
        self.blocks.retain(|id, _| referenced.contains(id));
    }

    /// Compress rows `[start, start + count)` into a single new
    /// LZ4-compressed block, evicting their real content out of
    /// `self.rows` and into `self.blocks`.
    ///
    /// This is the explicit, test-driven entry point for Task 119.4 — there
    /// is no automatic policy yet deciding *when* to call this (Task 119.5).
    ///
    /// Every row in the range must currently be Task-118 [`Row::is_compact`]
    /// and not already evicted, and the range must lie entirely below the
    /// visible window (`Buffer::visible_window_start(0)`): the live/visible
    /// region and any not-yet-compacted `Live` scrollback row are never
    /// compressed. Returns `false` (no-op — no partial mutation) if
    /// `count == 0`, the range is out of bounds, the range reaches into the
    /// visible window, or any row in the range fails the
    /// compact-and-not-evicted precondition.
    #[must_use]
    pub fn compress_scrollback_block(&mut self, start: usize, count: usize) -> bool {
        if count == 0 {
            return false;
        }
        self.sync_row_block_map_len();

        let Some(end) = start.checked_add(count) else {
            return false;
        };
        if end > self.rows.len() {
            return false;
        }
        // Never compress the visible region, nor any row at/after it.
        let visible_start = self.visible_window_start(0);
        if end > visible_start {
            return false;
        }

        // Validate every row up front and collect its `CompactRow` (cloned,
        // not decompacted) before mutating anything, so a precondition
        // failure partway through never leaves the buffer half-compressed.
        let mut compact_rows: Vec<CompactRow> = Vec::with_capacity(count);
        for row in &self.rows[start..end] {
            if row.is_evicted() {
                return false;
            }
            let Some(compact) = row.as_compact() else {
                return false;
            };
            compact_rows.push(compact.clone());
        }

        let block = CompressedBlock::from_rows(&compact_rows);
        let block_id = BlockId::new(self.next_block_id);
        // Practically unreachable (would require ~4 billion compressions in
        // one buffer's lifetime); saturate rather than wrap so an id is
        // never silently reused — see the field doc on
        // `Buffer::next_block_id`.
        self.next_block_id = self.next_block_id.saturating_add(1);

        for (i, row_idx) in (start..end).enumerate() {
            // `count` is bounded by a single compression call's row span
            // (never remotely close to `u32::MAX`); degrade to `u32::MAX`
            // rather than panicking in the unreachable overflow case,
            // mirroring `CompressedBlock::from_rows`'s own row-count
            // conversion.
            let offset_in_block = u32::value_from(i).unwrap_or(u32::MAX);
            self.rows[row_idx].evict_to_block();
            self.row_block_map[row_idx] = Some(BlockRowRef::new(block_id, offset_in_block));
            if row_idx < self.row_cache.len() {
                self.row_cache[row_idx] = None;
            }
        }

        self.blocks.insert(block_id, block);

        self.debug_assert_invariants();
        true
    }

    /// Ensure every row in `range` has real, readable content: decompress
    /// (once) every distinct compressed block referenced by
    /// `self.row_block_map[range]`, restoring every row across the **whole
    /// buffer** that references it back to Task-118 `Compact` storage — not
    /// just the rows inside `range`.
    ///
    /// A block is all-or-nothing (its bytes are one LZ4 blob), so
    /// decompressing it restores every row it holds, wherever those rows
    /// currently sit in `self.rows`. Walking the whole buffer for matching
    /// block ids (rather than tracking a per-block row-index list) is the
    /// Task 119.4 design choice: eviction is rare and buffers are bounded by
    /// `scrollback_limit`, so this is cheap relative to the decompression it
    /// guards.
    ///
    /// After this call, no row in `range` is evicted
    /// (`Row::is_evicted() == false`) and every block referenced from
    /// `range` has been removed from `self.blocks` (single residency).
    ///
    /// This is the correctness-over-speed decompress-on-read seam Task 119.4
    /// mandates. Callers include the scrollback flatten path
    /// (`Buffer::scrollback_as_tchars_and_tags`) and `Buffer::reflow_to_width`
    /// (called there over the *entire* buffer — deliberately unoptimized;
    /// Task 120 makes that fast, this subtask only needs it correct).
    pub(in crate::buffer) fn ensure_decompressed(&mut self, range: Range<usize>) {
        self.sync_row_block_map_len();

        let end = range.end.min(self.row_block_map.len());
        let start = range.start.min(end);

        let mut block_ids: HashSet<BlockId> = HashSet::new();
        for r in self.row_block_map[start..end].iter().flatten() {
            block_ids.insert(r.block_id());
        }

        for block_id in block_ids {
            let Some(block) = self.blocks.remove(&block_id) else {
                // Already restored by an earlier iteration (can't happen
                // with a `HashSet` of distinct ids, but `self.blocks` may
                // simply have no entry for a dangling reference — treat
                // that identically to "nothing to do" rather than panicking).
                continue;
            };

            match block.decompress_into(&mut self.decompress_scratch) {
                Some(rows) => {
                    for i in 0..self.rows.len() {
                        let Some(Some(r)) = self.row_block_map.get(i).copied() else {
                            continue;
                        };
                        if r.block_id() != block_id {
                            continue;
                        }
                        let offset = usize::value_from(r.offset_in_block()).unwrap_or(usize::MAX);
                        if let Some(compact) = rows.get(offset).cloned() {
                            self.rows[i].restore_from_compact(compact);
                        } else {
                            // Corrupt/impossible: the offset baked into
                            // `row_block_map` doesn't exist in the
                            // decompressed row list. Best-effort recovery
                            // (see `Row::abandon_eviction`) rather than a
                            // panic: leave the row blank but readable.
                            self.rows[i].abandon_eviction();
                        }
                        self.row_block_map[i] = None;
                    }
                }
                None => {
                    // Decompression failed (corrupt block — should be
                    // impossible per `CompressedBlock`'s own internal
                    // consistency checks). Best-effort: drop the mapping
                    // and the eviction marker for every row that referenced
                    // it, so future reads return blank content instead of
                    // asserting/panicking forever on a row nothing can ever
                    // restore.
                    for i in 0..self.rows.len() {
                        let Some(Some(r)) = self.row_block_map.get(i).copied() else {
                            continue;
                        };
                        if r.block_id() != block_id {
                            continue;
                        }
                        self.rows[i].abandon_eviction();
                        self.row_block_map[i] = None;
                    }
                }
            }
        }
    }

    /// Read-only, non-mutating resolution of row `row_idx`'s cells, for the
    /// `&self` text-extraction paths (`Buffer::extract_text` /
    /// `Buffer::extract_block_text`) that must not observe row eviction but
    /// also cannot call `Buffer::ensure_decompressed` (which needs
    /// `&mut self` to restore rows and cache the decompressed block).
    ///
    /// A non-evicted row (`Live` or Task-118 `Compact`) is served directly
    /// via `Row::characters()`, which already self-decompacts a `Compact`
    /// row transparently — no clone, no allocation.
    ///
    /// An evicted row's block is decompressed into a *local, transient*
    /// scratch buffer — never `self.decompress_scratch`, and the block is
    /// never removed from `self.blocks` or cached back onto the row. This
    /// is deliberately not the same "restore to `Compact` and cache" path
    /// `ensure_decompressed` uses: it is a one-off peek that leaves
    /// `Buffer` state completely unchanged, at the cost of re-decompressing
    /// the same block on every call — acceptable here because
    /// `extract_text`/`extract_block_text` are user-selection-driven, not a
    /// per-frame hot path.
    pub(in crate::buffer) fn row_cells_for_read(
        &self,
        row_idx: usize,
    ) -> std::borrow::Cow<'_, [Cell]> {
        if let Some(Some(block_ref)) = self.row_block_map.get(row_idx).copied()
            && let Some(block) = self.blocks.get(&block_ref.block_id())
        {
            let mut scratch = Vec::new();
            let cells = block
                .decompress_into(&mut scratch)
                .and_then(|rows| {
                    let offset = usize::value_from(block_ref.offset_in_block()).ok()?;
                    rows.into_iter().nth(offset)
                })
                .map(|compact| compact.to_row().cells().to_vec())
                .unwrap_or_default();
            return std::borrow::Cow::Owned(cells);
        }
        std::borrow::Cow::Borrowed(self.rows[row_idx].characters().as_slice())
    }
}

impl BlockId {
    /// Construct a `BlockId` from a raw counter value. Restricted to
    /// `crate::buffer` — outside this module a `BlockId` is an opaque
    /// handle obtained only from `Buffer::compress_scrollback_block`'s own
    /// bookkeeping.
    pub(in crate::buffer) const fn new(id: u32) -> Self {
        Self(id)
    }
}

impl BlockRowRef {
    /// Construct a `BlockRowRef` from its parts. Restricted to
    /// `crate::buffer` — see `BlockId::new`.
    pub(in crate::buffer) const fn new(block_id: BlockId, offset_in_block: u32) -> Self {
        Self {
            block_id,
            offset_in_block,
        }
    }

    /// The block this row's content lives in.
    pub(in crate::buffer) const fn block_id(self) -> BlockId {
        self.block_id
    }

    /// This row's block-relative position within `block_id`'s block.
    pub(in crate::buffer) const fn offset_in_block(self) -> u32 {
        self.offset_in_block
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use freminal_common::buffer_states::tchar::TChar;

    use crate::row::Row;

    use super::*;

    fn ascii(c: char) -> TChar {
        TChar::from(c)
    }

    fn text(s: &str) -> Vec<TChar> {
        s.chars().map(ascii).collect()
    }

    /// Push `n` numbered lines (each terminated by LF+CR, matching real PTY
    /// output). Mirrors `scrollback_compaction_tests::push_numbered_lines`
    /// in `buffer/mod.rs`.
    fn push_numbered_lines(buf: &mut Buffer, n: usize) {
        for i in 0..n {
            buf.insert_text(&text(&format!("line{i:04}content")));
            buf.handle_lf();
            buf.handle_cr();
        }
    }

    /// Build a buffer with `n` numbered scrollback lines, all compacted
    /// (Task 118), ready for `compress_scrollback_block`.
    fn buffer_with_compact_scrollback(n: usize) -> Buffer {
        let mut buf = Buffer::new(20, 3).with_scrollback_limit(200);
        push_numbered_lines(&mut buf, n);
        let _ = buf.compact_idle_scrollback(usize::MAX);
        buf
    }

    #[test]
    fn compress_scrollback_block_rejects_the_visible_region() {
        let mut buf = buffer_with_compact_scrollback(10);
        let visible_start = buf.visible_window_start(0);

        // A range reaching into (or starting at) the visible window must
        // be rejected outright.
        assert!(!buf.compress_scrollback_block(visible_start, 1));
        assert!(!buf.compress_scrollback_block(0, buf.rows.len() + 1));
    }

    #[test]
    fn compress_scrollback_block_rejects_non_compact_rows() {
        let mut buf = Buffer::new(20, 3).with_scrollback_limit(200);
        push_numbered_lines(&mut buf, 10);
        // Deliberately do NOT call compact_idle_scrollback: rows are Live.
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start > 0, "test needs scrollback");
        assert!(!buf.compress_scrollback_block(0, visible_start));
    }

    #[test]
    fn compress_scrollback_block_zero_count_is_noop() {
        let mut buf = buffer_with_compact_scrollback(10);
        assert!(!buf.compress_scrollback_block(0, 0));
    }

    #[test]
    fn compressing_evicts_rows_and_populates_blocks() {
        let mut buf = buffer_with_compact_scrollback(10);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 2, "test needs scrollback");

        assert!(buf.compress_scrollback_block(0, visible_start));

        assert_eq!(buf.blocks.len(), 1);
        for i in 0..visible_start {
            assert!(buf.rows[i].is_evicted(), "row {i} should be evicted");
            assert!(
                buf.row_block_map[i].is_some(),
                "row_block_map[{i}] should reference the new block"
            );
        }
        for i in visible_start..buf.rows.len() {
            assert!(
                !buf.rows[i].is_evicted(),
                "visible row {i} must be untouched"
            );
            assert!(buf.row_block_map[i].is_none());
        }
    }

    #[test]
    fn flatten_identical_before_and_after_compression() {
        let mut buf = buffer_with_compact_scrollback(20);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 2, "test needs scrollback");

        let (chars_before, tags_before, offsets_before, urls_before) =
            buf.scrollback_as_tchars_and_tags(0);

        assert!(buf.compress_scrollback_block(0, visible_start));

        let (chars_after, tags_after, offsets_after, urls_after) =
            buf.scrollback_as_tchars_and_tags(0);

        assert_eq!(
            chars_before, chars_after,
            "flattened characters must be identical before/after compression"
        );
        assert_eq!(tags_before, tags_after, "format tags must be identical");
        assert_eq!(
            offsets_before, offsets_after,
            "row offsets must be identical"
        );
        assert_eq!(urls_before, urls_after, "url tag indices must be identical");

        // The flatten above must have transparently decompressed and
        // restored every row (single residency).
        assert!(buf.blocks.is_empty(), "block must be removed after a read");
        for i in 0..visible_start {
            assert!(!buf.rows[i].is_evicted());
            assert!(buf.row_block_map[i].is_none());
        }
    }

    #[test]
    fn scroll_into_compressed_block_decompresses_and_clears_mapping() {
        let mut buf = buffer_with_compact_scrollback(20);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 2);

        assert!(buf.compress_scrollback_block(0, visible_start));
        assert_eq!(buf.blocks.len(), 1);

        // Simulate "scrolling into" a compressed row by reading it directly
        // via ensure_decompressed (the seam every read path uses).
        buf.ensure_decompressed(0..visible_start);

        assert!(buf.blocks.is_empty());
        for i in 0..visible_start {
            assert!(!buf.rows[i].is_evicted());
            assert!(buf.row_block_map[i].is_none());
            assert!(
                buf.rows[i].is_compact(),
                "row {i} should restore to Compact, not Live"
            );
        }
    }

    #[test]
    fn extract_text_and_block_text_identical_over_compressed_scrollback() {
        let mut buf = buffer_with_compact_scrollback(10);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 2, "test needs at least 2 scrollback rows");

        let before_text = buf.extract_text(0, 0, 1, 14);
        let before_block = buf.extract_block_text(0, 0, 1, 6);

        assert!(buf.compress_scrollback_block(0, visible_start));

        let after_text = buf.extract_text(0, 0, 1, 14);
        let after_block = buf.extract_block_text(0, 0, 1, 6);

        assert_eq!(before_text, after_text);
        assert_eq!(before_block, after_block);
        assert!(after_text.contains("line0000content"));
        assert!(after_text.contains("line0001content"));

        // extract_text/extract_block_text take `&self` and must not mutate
        // Buffer state: the block must still be resident afterward.
        assert_eq!(
            buf.blocks.len(),
            1,
            "extract_text must not decompress-and-cache"
        );
        assert!(buf.rows[0].is_evicted());
    }

    #[test]
    fn drain_bisecting_a_compressed_block_survives() {
        // Two otherwise-identical buffers, diverging only in whether the
        // first batch of scrollback was compressed before the second batch
        // pushes enough further output that `enforce_scrollback_limit`
        // drains some (but not all) of the original block's rows from the
        // front — bisecting it. Both must end up byte-identical.
        fn build(compress: bool) -> Buffer {
            let mut buf = Buffer::new(20, 3).with_scrollback_limit(5);
            // Push enough lines that the scrollback limit has already
            // engaged and stabilized at its cap
            // (`height + scrollback_limit == 8`), giving a comfortably
            // bisectable `visible_start` of 5.
            push_numbered_lines(&mut buf, 20);
            let _ = buf.compact_idle_scrollback(usize::MAX);

            if compress {
                let visible_start = buf.visible_window_start(0);
                assert!(visible_start >= 4, "test needs enough scrollback to bisect");
                assert!(buf.compress_scrollback_block(0, visible_start));
                assert_eq!(buf.blocks.len(), 1);
            }

            // Push enough further output that enforce_scrollback_limit
            // drains some (but not all) of the original block's rows from
            // the front.
            push_numbered_lines(&mut buf, 30);
            buf
        }

        let mut compressed = build(true);
        let mut plain = build(false);

        let max_rows = compressed.height + compressed.scrollback_limit();
        assert!(compressed.rows.len() <= max_rows);
        assert_eq!(compressed.rows.len(), plain.rows.len());
        assert_eq!(
            compressed.row_block_map.len(),
            compressed.rows.len(),
            "row_block_map must stay index-parallel to rows after a drain"
        );

        let (chars_c, tags_c, offsets_c, urls_c) = compressed.scrollback_as_tchars_and_tags(0);
        let (chars_p, tags_p, offsets_p, urls_p) = plain.scrollback_as_tchars_and_tags(0);
        assert_eq!(
            chars_c, chars_p,
            "surviving scrollback content must match exactly"
        );
        assert_eq!(tags_c, tags_p);
        assert_eq!(offsets_c, offsets_p);
        assert_eq!(urls_c, urls_p);

        let (vis_chars_c, ..) = compressed.visible_as_tchars_and_tags(0);
        let (vis_chars_p, ..) = plain.visible_as_tchars_and_tags(0);
        assert_eq!(
            vis_chars_c, vis_chars_p,
            "visible window must also match exactly"
        );
    }

    #[test]
    fn reflow_over_compressed_scrollback_matches_uncompressed_reflow() {
        fn build_and_reflow(compress: bool) -> Vec<Row> {
            let mut buf = Buffer::new(20, 3).with_scrollback_limit(200);
            push_numbered_lines(&mut buf, 20);
            let _ = buf.compact_idle_scrollback(usize::MAX);
            if compress {
                let visible_start = buf.visible_window_start(0);
                assert!(buf.compress_scrollback_block(0, visible_start));
            }
            buf.set_size(8, 3, 0);
            buf.rows.clone()
        }

        let compressed = build_and_reflow(true);
        let plain = build_and_reflow(false);

        assert_eq!(compressed.len(), plain.len());
        for (a, b) in compressed.iter().zip(plain.iter()) {
            assert_eq!(a.cells(), b.cells());
            assert_eq!(a.max_width(), b.max_width());
            assert_eq!(a.origin, b.origin);
            assert_eq!(a.join, b.join);
        }
    }

    #[test]
    fn alt_screen_round_trip_preserves_compressed_primary_scrollback() {
        let mut buf = buffer_with_compact_scrollback(20);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 2);

        assert!(buf.compress_scrollback_block(0, visible_start));
        assert_eq!(buf.blocks.len(), 1);

        let (chars_before, tags_before, ..) = buf.scrollback_as_tchars_and_tags(0);
        // The read above transparently decompressed everything back to
        // Compact; re-compress so the round trip actually exercises a
        // non-empty `blocks` map across the switch.
        assert!(buf.compress_scrollback_block(0, visible_start));
        assert_eq!(buf.blocks.len(), 1);

        buf.enter_alternate(0);
        assert!(
            buf.blocks.is_empty(),
            "alt screen must start with no blocks"
        );
        assert_eq!(buf.row_block_map.iter().filter(|e| e.is_some()).count(), 0);

        let _ = buf.leave_alternate();

        assert_eq!(
            buf.blocks.len(),
            1,
            "compressed block must survive the round trip"
        );
        for i in 0..visible_start {
            assert!(buf.rows[i].is_evicted());
        }

        let (chars_after, tags_after, ..) = buf.scrollback_as_tchars_and_tags(0);
        assert_eq!(chars_before, chars_after);
        assert_eq!(tags_before, tags_after);
    }

    #[test]
    fn ensure_decompressed_clears_eviction_flag_so_reads_no_longer_assert() {
        let mut buf = buffer_with_compact_scrollback(10);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 1);

        assert!(buf.compress_scrollback_block(0, visible_start));
        assert!(buf.rows[0].is_evicted());

        buf.ensure_decompressed(0..visible_start);

        assert!(!buf.rows[0].is_evicted());
        // A direct read must now succeed without tripping the diagnostic
        // debug_assert in `Row::cells_ref`.
        let _ = buf.rows[0].cells();
    }

    #[test]
    fn erase_scrollback_drops_compressed_blocks() {
        let mut buf = buffer_with_compact_scrollback(20);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 2);

        assert!(buf.compress_scrollback_block(0, visible_start));
        assert_eq!(buf.blocks.len(), 1);

        buf.erase_scrollback();

        assert!(buf.blocks.is_empty());
        assert_eq!(buf.row_block_map.len(), buf.rows.len());
        assert!(buf.row_block_map.iter().all(Option::is_none));
    }

    /// Regression (119.4 code review, CRITICAL-1): scrolling back into a
    /// compressed region via the *visible-window* flatten path
    /// (`visible_as_tchars_and_tags_extended` with a nonzero scroll offset)
    /// must decompress the evicted rows first, not trip `cells_ref`'s
    /// eviction `debug_assert` (debug) / render blank (release). This path is
    /// the one the GUI actually drives when the user scrolls up.
    #[test]
    fn scrolled_visible_window_flatten_decompresses_compressed_rows() {
        let mut buf = buffer_with_compact_scrollback(20);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 2, "test needs scrollback");

        // Baseline: flatten the fully-scrolled-back view (offset large
        // enough to pin the window at the very top of the buffer) BEFORE
        // compressing, so we have a known-good comparison.
        let max_offset = buf.max_scroll_offset();
        assert!(max_offset > 0, "test needs a scrollable buffer");
        let (chars_before, tags_before, ..) = buf.visible_as_tchars_and_tags(max_offset);

        // Compress the entire scrollback region, then flatten the same
        // scrolled-back view again. Must be byte-identical and must not
        // panic on an evicted placeholder.
        assert!(buf.compress_scrollback_block(0, visible_start));
        assert!(!buf.blocks.is_empty(), "scrollback should be compressed");

        let (chars_after, tags_after, ..) = buf.visible_as_tchars_and_tags(max_offset);
        assert_eq!(
            chars_before, chars_after,
            "scrolled-back flatten must be identical before/after compression"
        );
        assert_eq!(tags_before, tags_after);
    }

    /// Regression (119.4 code review, CRITICAL-2): the whole-buffer
    /// image-clearing sweeps in `images.rs` iterate every row (including
    /// deep scrollback) and read cells; they must skip evicted/compact rows
    /// rather than trip the eviction `debug_assert`. An evicted row provably
    /// holds no images, so clearing is a no-op there anyway.
    #[test]
    fn whole_buffer_image_clear_over_compressed_scrollback_does_not_panic() {
        let mut buf = buffer_with_compact_scrollback(20);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 2, "test needs scrollback");

        assert!(buf.compress_scrollback_block(0, visible_start));
        assert!(!buf.blocks.is_empty());

        // Each of these walks the entire buffer, including the compressed
        // scrollback rows. None may panic; all are no-ops over evicted rows
        // (which carry no images), so the compressed block stays resident.
        buf.clear_all_image_placements();
        buf.clear_image_placements_by_id(42);
        buf.clear_image_placements_by_number(1);
        buf.clear_image_placements_by_z_index(0);
        buf.clear_image_placements_in_column(0);

        assert_eq!(
            buf.blocks.len(),
            1,
            "image sweeps must not decompress/evict compressed rows"
        );
        for i in 0..visible_start {
            assert!(buf.rows[i].is_evicted(), "row {i} must stay evicted");
        }
    }
}
