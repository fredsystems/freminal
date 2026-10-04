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
//! `Compact` (in `self.rows[i]`, `block_map()[i] == None`), or compressed
//! (`block_map()[i] == Some(_)`, real content lives only in
//! `self.blocks`). [`Buffer::ensure_decompressed`] restores a touched block
//! back to `Compact` (not `Live` — preserving the Task-118 memory win) and
//! removes it from `self.blocks`, so a block is never both compressed and
//! live at the same time.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use conv2::ValueFrom;

use freminal_common::buffer_states::buffer_type::BufferType;

use crate::cell::Cell;
use crate::compact_row::CompactRow;
use crate::compressed_block::CompressedBlock;

use super::{BlockId, BlockRowRef, Buffer};

/// Maximum number of contiguous eligible rows grouped into a single
/// `compress_scrollback_block` call by `Buffer::compress_idle_scrollback`.
///
/// The plan's target range is ~128-256 logical scrollback rows: large
/// enough to amortize LZ4's fixed per-call overhead, small enough that
/// scrolling into a block (which decompresses the whole thing) stays well
/// under one frame budget. `256` is chosen here as the upper end of that
/// range; whether `128` yields a meaningfully better ratio/latency
/// trade-off is validated against measured behavior in Task 119.6's
/// benches, not here.
const BLOCK_SIZE: usize = 256;

/// One compressed block together with the number of buffer rows that still
/// reference it (Task 125.15).
///
/// `live_rows` is the count of `Some(BlockRowRef)` entries in the row store's
/// block map that name this block. [`Buffer::compress_scrollback_block`] sets
/// it to the number of rows it evicted into the block; front eviction
/// ([`release_block_rows`]) decrements it per evicted row and frees the block
/// when it reaches zero, so reclaiming a fully-drained block is O(rows
/// evicted) instead of a whole-buffer reachability scan. The counts are
/// verified against the block map by `Buffer::debug_assert_invariants`.
#[derive(Debug, Clone)]
pub(in crate::buffer) struct BlockSlot {
    /// The compressed rows.
    pub(in crate::buffer) block: CompressedBlock,
    /// How many live rows reference this block.
    pub(in crate::buffer) live_rows: u32,
}

/// Account for `rows` rows that referenced block `id` having been evicted from
/// the front of the buffer, freeing the block once nothing references it.
///
/// A free function over the map (not a `Buffer` method) so the eviction
/// closure can borrow `Buffer::blocks` while `Buffer::rows` is mutably
/// borrowed. An unknown `id` is ignored: the block was already removed (for
/// example by [`Buffer::ensure_decompressed`], which also clears the
/// references).
pub(in crate::buffer) fn release_block_rows(
    blocks: &mut HashMap<BlockId, BlockSlot>,
    id: BlockId,
    rows: u32,
) {
    let Some(slot) = blocks.get_mut(&id) else {
        return;
    };
    slot.live_rows = slot.live_rows.saturating_sub(rows);
    if slot.live_rows == 0 {
        blocks.remove(&id);
    }
}

impl Buffer {
    /// Compress up to `budget` rows of already-Task-118-compact, cold
    /// scrollback into LZ4 blocks, returning the number of rows *newly
    /// compressed*.
    ///
    /// Intended to be called from the PTY thread's idle tick immediately
    /// AFTER `compact_idle_scrollback` has caught up for this same tick
    /// (see the idle-tick arm in `freminal/src/gui/pty.rs`): a row must be
    /// Task-118-compacted before it is a compression candidate, so calling
    /// this while a compaction backlog is still draining would find
    /// nothing to do — compact first, compress the now-cold result second.
    ///
    /// Only contiguous runs of scrollback rows (below the visible window,
    /// `0..visible_window_start(0)`) that are ALL currently
    /// [`Row::is_compact`] and not already evicted into a block are
    /// eligible. A `Live` (not-yet-compacted) row, an already-compressed
    /// row, or an image row (never compact) breaks the current run —
    /// those rows become eligible on a later tick once compaction (or a
    /// prior compression call) has processed them. A no-op (returns `0`)
    /// on the alternate screen and when there is no scrollback, no budget,
    /// or nothing left to compress.
    ///
    /// Each eligible run is grouped into chunks of at most `BLOCK_SIZE`
    /// rows and compressed via one `compress_scrollback_block` call per
    /// chunk. There is deliberately no minimum chunk length: even a
    /// single-row trailing run is still compressed, trading some
    /// compression ratio (a one-row block still pays LZ4's fixed per-call
    /// overhead) for algorithmic simplicity and testability — such a short
    /// run is rare in practice (only at scrollback's tail, near the visible
    /// window, or where an image row bisects a run).
    ///
    /// `budget` bounds the number of rows *actually compressed* — i.e.
    /// rows inside chunks for which `compress_scrollback_block` returned
    /// `true` — mirroring `compact_idle_scrollback`'s "budget counts real
    /// work, not rows scanned" discipline: a chunk that fails to compress
    /// (should not happen given the eligibility scan performed here, but
    /// mirrors `compress_scrollback_block`'s own preconditions defensively)
    /// does not consume budget. A chunk that would exceed the remaining
    /// budget is shrunk to fit exactly, so every call makes forward
    /// progress up to `budget` rather than skipping a run entirely because
    /// its full size doesn't fit.
    #[must_use]
    pub fn compress_idle_scrollback(&mut self, budget: usize) -> usize {
        if self.kind == BufferType::Alternate || budget == 0 {
            return 0;
        }

        let visible_start = self.visible_window_start(0);
        if visible_start == 0 {
            return 0;
        }

        let mut compressed = 0usize;
        let mut i = 0usize;
        while i < visible_start && compressed < budget {
            if !self.row_is_compression_candidate(i) {
                i += 1;
                continue;
            }

            // Extend the run while rows stay eligible, bounded by the
            // visible window.
            let mut run_end = i + 1;
            while run_end < visible_start && self.row_is_compression_candidate(run_end) {
                run_end += 1;
            }

            // Compress the run in `BLOCK_SIZE` chunks, each further capped
            // by the remaining budget so `budget` is always respected
            // exactly.
            let mut chunk_start = i;
            while chunk_start < run_end && compressed < budget {
                let remaining_budget = budget - compressed;
                let chunk_len = (run_end - chunk_start)
                    .min(BLOCK_SIZE)
                    .min(remaining_budget);
                if chunk_len == 0 {
                    break;
                }
                if self.compress_scrollback_block(chunk_start, chunk_len) {
                    compressed += chunk_len;
                }
                chunk_start += chunk_len;
            }

            i = run_end;
        }

        self.debug_assert_invariants();
        compressed
    }

    /// `true` if scrollback row `idx` is eligible to be grouped into a
    /// `compress_idle_scrollback` run: currently Task-118 [`Row::is_compact`]
    /// and not already evicted into a compressed block. Used only by the
    /// run scan in `compress_idle_scrollback`.
    fn row_is_compression_candidate(&self, idx: usize) -> bool {
        let Some(row) = self.rows.get(idx) else {
            return false;
        };
        row.is_compact()
            && !row.is_evicted()
            && self.rows.block_map().get(idx).copied().flatten().is_none()
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

        let mut live_rows: u32 = 0;
        for (i, row_idx) in (start..end).enumerate() {
            // `count` is bounded by a single compression call's row span
            // (never remotely close to `u32::MAX`); degrade to `u32::MAX`
            // rather than panicking in the unreachable overflow case,
            // mirroring `CompressedBlock::from_rows`'s own row-count
            // conversion.
            let offset_in_block = u32::value_from(i).unwrap_or(u32::MAX);
            self.rows[row_idx].evict_to_block();
            self.rows.block_map_mut()[row_idx] = Some(BlockRowRef::new(block_id, offset_in_block));
            self.rows.invalidate(row_idx);
            live_rows = live_rows.saturating_add(1);
        }

        self.blocks.insert(block_id, BlockSlot { block, live_rows });

        self.debug_assert_invariants();
        true
    }

    /// Ensure every row in `range` has real, readable content: decompress
    /// (once) every distinct compressed block referenced by
    /// the row store's block map over `range`, restoring every row across the **whole
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
        let end = range.end.min(self.rows.block_map().len());
        let start = range.start.min(end);

        let mut block_ids: HashSet<BlockId> = HashSet::new();
        for r in self.rows.block_map()[start..end].iter().flatten() {
            block_ids.insert(r.block_id());
        }

        for block_id in block_ids {
            let Some(BlockSlot { block, .. }) = self.blocks.remove(&block_id) else {
                // Already restored by an earlier iteration (can't happen
                // with a `HashSet` of distinct ids, but `self.blocks` may
                // simply have no entry for a dangling reference — treat
                // that identically to "nothing to do" rather than panicking).
                continue;
            };

            match block.decompress_into(&mut self.decompress_scratch) {
                Some(rows) => {
                    for i in 0..self.rows.len() {
                        let Some(Some(r)) = self.rows.block_map().get(i).copied() else {
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
                            // the block map doesn't exist in the
                            // decompressed row list. Best-effort recovery
                            // (see `Row::abandon_eviction`) rather than a
                            // panic: leave the row blank but readable.
                            self.rows[i].abandon_eviction();
                        }
                        self.rows.block_map_mut()[i] = None;
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
                        let Some(Some(r)) = self.rows.block_map().get(i).copied() else {
                            continue;
                        };
                        if r.block_id() != block_id {
                            continue;
                        }
                        self.rows[i].abandon_eviction();
                        self.rows.block_map_mut()[i] = None;
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
        if let Some(Some(block_ref)) = self.rows.block_map().get(row_idx).copied()
            && let Some(slot) = self.blocks.get(&block_ref.block_id())
        {
            let mut scratch = Vec::new();
            let cells = slot
                .block
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
    use freminal_common::buffer_states::{row_number::RowNumber, tchar::TChar};

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
                buf.rows.block_map()[i].is_some(),
                "block_map()[{i}] should reference the new block"
            );
        }
        for i in visible_start..buf.rows.len() {
            assert!(
                !buf.rows[i].is_evicted(),
                "visible row {i} must be untouched"
            );
            assert!(buf.rows.block_map()[i].is_none());
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
            assert!(buf.rows.block_map()[i].is_none());
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
            assert!(buf.rows.block_map()[i].is_none());
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

        let before_text = buf.extract_text(RowNumber::new(0), 0, RowNumber::new(1), 14);
        let before_block = buf.extract_block_text(RowNumber::new(0), 0, RowNumber::new(1), 6);

        assert!(buf.compress_scrollback_block(0, visible_start));

        let after_text = buf.extract_text(RowNumber::new(0), 0, RowNumber::new(1), 14);
        let after_block = buf.extract_block_text(RowNumber::new(0), 0, RowNumber::new(1), 6);

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
            compressed.rows.block_map().len(),
            compressed.rows.len(),
            "block map must stay index-parallel to rows after a drain"
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
            buf.rows.to_vec()
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
        assert_eq!(
            buf.rows.block_map().iter().filter(|e| e.is_some()).count(),
            0
        );

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
        assert_eq!(buf.rows.block_map().len(), buf.rows.len());
        assert!(buf.rows.block_map().iter().all(Option::is_none));
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

    // -------------------------------------------------------------------
    // compress_idle_scrollback (Task 119.5)
    // -------------------------------------------------------------------

    #[test]
    fn compress_idle_scrollback_compresses_already_compacted_rows() {
        let mut buf = Buffer::new(20, 3).with_scrollback_limit(200);
        push_numbered_lines(&mut buf, 20);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 2, "test needs scrollback");

        let (chars_before, tags_before, offsets_before, urls_before) =
            buf.scrollback_as_tchars_and_tags(0);

        // Compact first (Task 118), then compress (Task 119.5) — mirrors
        // the idle-tick sequencing in `freminal/src/gui/pty.rs`.
        let compacted = buf.compact_idle_scrollback(usize::MAX);
        assert!(compacted > 0, "test needs rows to compact");

        let compressed = buf.compress_idle_scrollback(usize::MAX);
        assert!(compressed > 0, "expected some rows to be compressed");
        assert!(!buf.blocks.is_empty(), "expected at least one block");

        for i in 0..visible_start {
            assert!(buf.rows[i].is_evicted(), "row {i} should be evicted");
            assert!(buf.rows.block_map()[i].is_some());
        }
        for i in visible_start..buf.rows.len() {
            assert!(!buf.rows[i].is_evicted(), "visible row {i} untouched");
        }

        let (chars_after, tags_after, offsets_after, urls_after) =
            buf.scrollback_as_tchars_and_tags(0);
        assert_eq!(
            chars_before, chars_after,
            "flattened characters must be identical before/after idle compression"
        );
        assert_eq!(tags_before, tags_after);
        assert_eq!(offsets_before, offsets_after);
        assert_eq!(urls_before, urls_after);
    }

    #[test]
    fn compress_idle_scrollback_on_live_rows_compresses_nothing() {
        let mut buf = Buffer::new(20, 3).with_scrollback_limit(200);
        push_numbered_lines(&mut buf, 20);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 2, "test needs scrollback");

        // Deliberately do NOT compact: every scrollback row is still Live,
        // so compression must find nothing eligible.
        let compressed = buf.compress_idle_scrollback(usize::MAX);

        assert_eq!(compressed, 0);
        assert!(buf.blocks.is_empty());
        for i in 0..visible_start {
            assert!(!buf.rows[i].is_evicted());
            assert!(buf.rows.block_map().get(i).copied().flatten().is_none());
        }
    }

    #[test]
    fn compress_idle_scrollback_respects_budget_and_drains_over_multiple_calls() {
        let mut buf = buffer_with_compact_scrollback(2000);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 100, "test needs a large scrollback");

        let budget = 100usize;
        let mut total_compressed = 0usize;
        let mut iterations = 0usize;
        loop {
            let compressed = buf.compress_idle_scrollback(budget);
            assert!(
                compressed <= budget,
                "a single call must never compress more than its budget"
            );
            if compressed == 0 {
                break;
            }
            total_compressed += compressed;
            iterations += 1;
            assert!(
                iterations < 1_000,
                "compression should fully drain well within this many calls"
            );
        }

        assert!(
            iterations > 1,
            "a large scrollback should take more than one budgeted call to fully compress"
        );
        assert_eq!(total_compressed, visible_start);
        for i in 0..visible_start {
            assert!(buf.rows[i].is_evicted(), "row {i} should be evicted");
        }
    }

    #[test]
    fn compress_idle_scrollback_alternate_screen_is_noop() {
        let mut buf = buffer_with_compact_scrollback(20);
        buf.enter_alternate(0);
        assert_eq!(buf.kind, BufferType::Alternate);

        let compressed = buf.compress_idle_scrollback(usize::MAX);

        assert_eq!(compressed, 0);
        assert!(buf.blocks.is_empty());
    }

    #[test]
    fn compress_idle_scrollback_never_touches_the_visible_region() {
        let mut buf = buffer_with_compact_scrollback(20);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 2, "test needs scrollback");

        let _ = buf.compress_idle_scrollback(usize::MAX);

        for i in visible_start..buf.rows.len() {
            assert!(
                !buf.rows[i].is_evicted(),
                "visible row {i} must never be compressed"
            );
            assert!(buf.rows.block_map().get(i).copied().flatten().is_none());
        }
    }

    #[test]
    fn compress_idle_scrollback_is_idempotent_once_fully_compressed() {
        let mut buf = buffer_with_compact_scrollback(20);

        let first_pass = buf.compress_idle_scrollback(usize::MAX);
        assert!(first_pass > 0, "test needs something to compress");

        // Fully compressed already: further calls must do no busy-work.
        assert_eq!(buf.compress_idle_scrollback(usize::MAX), 0);
        assert_eq!(buf.compress_idle_scrollback(usize::MAX), 0);
    }

    // ── Live-row counts (Task 125.15) ───────────────────────────────────

    /// Evict `n` rows from the front and keep the cursor on its row, as every
    /// production caller of `evict_front_rows` does for itself.
    fn evict(buf: &mut Buffer, n: usize) -> usize {
        let evicted = buf.evict_front_rows(n);
        buf.cursor.pos.y = buf.cursor.pos.y.saturating_sub(evicted);
        evicted
    }

    /// Number of block-map entries across the live rows that name a block.
    fn rows_referencing_blocks(buf: &Buffer) -> usize {
        buf.rows.block_map().iter().flatten().count()
    }

    /// Sum of every stored block's `live_rows`.
    fn total_live_rows(buf: &Buffer) -> usize {
        buf.blocks
            .values()
            .map(|slot| usize::value_from(slot.live_rows).unwrap())
            .sum()
    }

    /// The single block's id, asserting exactly one is stored.
    fn only_block_id(buf: &Buffer) -> BlockId {
        assert_eq!(buf.blocks.len(), 1);
        *buf.blocks.keys().next().unwrap()
    }

    #[test]
    fn compression_sets_live_rows_to_the_rows_it_evicted() {
        let mut buf = buffer_with_compact_scrollback(12);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 6, "test needs scrollback");

        assert!(buf.compress_scrollback_block(0, 4));
        assert!(buf.compress_scrollback_block(4, 2));

        assert_eq!(buf.blocks.len(), 2);
        let mut counts: Vec<u32> = buf.blocks.values().map(|s| s.live_rows).collect();
        counts.sort_unstable();
        assert_eq!(counts, vec![2, 4]);
        assert_eq!(total_live_rows(&buf), rows_referencing_blocks(&buf));
    }

    #[test]
    fn evicting_every_row_of_a_block_frees_it() {
        let mut buf = buffer_with_compact_scrollback(12);
        assert!(buf.compress_scrollback_block(0, 6));
        assert_eq!(only_block_id_live_rows(&buf), 6);

        assert_eq!(evict(&mut buf, 6), 6);

        assert!(buf.blocks.is_empty(), "a fully evicted block must be freed");
        assert_eq!(rows_referencing_blocks(&buf), 0);
        buf.debug_assert_invariants();
    }

    fn only_block_id_live_rows(buf: &Buffer) -> u32 {
        buf.blocks[&only_block_id(buf)].live_rows
    }

    #[test]
    fn evicting_part_of_a_block_keeps_it_with_a_reduced_count() {
        let mut buf = buffer_with_compact_scrollback(12);
        assert!(buf.compress_scrollback_block(0, 6));
        let id = only_block_id(&buf);

        assert_eq!(evict(&mut buf, 2), 2);

        assert_eq!(buf.blocks[&id].live_rows, 4);
        assert_eq!(rows_referencing_blocks(&buf), 4);
        buf.debug_assert_invariants();

        // The survivors still decompress to the right content: offsets are
        // block-relative, so evicting the front never remapped them.
        let (chars, ..) = buf.scrollback_as_tchars_and_tags(0);
        let text: String = chars
            .iter()
            .map(|c| match c {
                TChar::Ascii(b) => char::from(*b),
                _ => '?',
            })
            .collect();
        assert!(text.contains("line0002"), "row 2 must survive: {text}");
        assert!(!text.contains("line0001"), "row 1 was evicted: {text}");
    }

    #[test]
    fn a_bisected_block_is_freed_only_when_its_last_row_goes() {
        let mut buf = buffer_with_compact_scrollback(14);
        assert!(buf.compress_scrollback_block(0, 4));
        assert!(buf.compress_scrollback_block(4, 4));
        assert_eq!(buf.blocks.len(), 2);

        // 6 rows: all of block 0 and the first two rows of block 1.
        assert_eq!(evict(&mut buf, 6), 6);
        assert_eq!(buf.blocks.len(), 1, "block 0 freed, block 1 bisected");
        assert_eq!(only_block_id_live_rows(&buf), 2);
        buf.debug_assert_invariants();

        assert_eq!(evict(&mut buf, 2), 2);
        assert!(buf.blocks.is_empty());
        buf.debug_assert_invariants();
    }

    #[test]
    fn a_run_of_rows_from_one_block_is_released_in_one_call() {
        // The store coalesces consecutive evicted rows of one block into a
        // single release; the buffer-level count must still come out exact.
        let mut buf = buffer_with_compact_scrollback(20);
        assert!(buf.compress_scrollback_block(0, 8));
        assert!(buf.compress_scrollback_block(8, 8));
        assert_eq!(evict(&mut buf, 12), 12);
        assert_eq!(only_block_id_live_rows(&buf), 4);
        assert_eq!(total_live_rows(&buf), rows_referencing_blocks(&buf));
    }

    #[test]
    fn decompress_then_recompress_gets_a_new_block_id_and_count() {
        let mut buf = buffer_with_compact_scrollback(12);
        assert!(buf.compress_scrollback_block(0, 6));
        let first = only_block_id(&buf);

        buf.ensure_decompressed(0..6);
        assert!(buf.blocks.is_empty(), "decompression removes the slot");
        assert_eq!(rows_referencing_blocks(&buf), 0);

        assert!(buf.compress_scrollback_block(0, 6));
        let second = only_block_id(&buf);
        assert_ne!(first, second, "recompression mints a new id");
        assert_eq!(buf.blocks[&second].live_rows, 6);

        // Evicting the rows frees the new block; the old id is long gone.
        assert_eq!(evict(&mut buf, 6), 6);
        assert!(buf.blocks.is_empty());
        buf.debug_assert_invariants();
    }

    #[test]
    fn decompressing_through_a_partly_evicted_block_clears_every_reference() {
        let mut buf = buffer_with_compact_scrollback(12);
        assert!(buf.compress_scrollback_block(0, 6));
        assert_eq!(evict(&mut buf, 2), 2);

        buf.ensure_decompressed(0..buf.rows.len());

        assert!(buf.blocks.is_empty());
        assert_eq!(rows_referencing_blocks(&buf), 0);
        buf.debug_assert_invariants();
    }

    #[test]
    fn erase_scrollback_leaves_no_block_and_no_reference() {
        let mut buf = buffer_with_compact_scrollback(20);
        let visible_start = buf.visible_window_start(0);
        assert!(buf.compress_scrollback_block(0, visible_start / 2));
        assert!(
            buf.compress_scrollback_block(visible_start / 2, visible_start - visible_start / 2)
        );
        assert_eq!(buf.blocks.len(), 2);

        buf.erase_scrollback();

        assert!(buf.blocks.is_empty());
        assert_eq!(rows_referencing_blocks(&buf), 0);
        buf.debug_assert_invariants();
    }

    /// Eviction through the real hot path (`enforce_scrollback_limit`), with
    /// compressed rows at the front, keeps every block's count equal to the
    /// rows that reference it; the debug invariant asserts this on every line.
    #[test]
    fn counts_stay_exact_through_scrollback_limit_eviction() {
        let mut buf = Buffer::new(20, 3).with_scrollback_limit(40);
        push_numbered_lines(&mut buf, 38);
        let _ = buf.compact_idle_scrollback(usize::MAX);
        let _ = buf.compress_idle_scrollback(usize::MAX);
        let initial_blocks = buf.blocks.len();
        assert!(initial_blocks > 0, "the test must exercise blocks");

        let mut freed = false;
        for round in 0..10 {
            push_numbered_lines(&mut buf, 6);
            assert_eq!(
                total_live_rows(&buf),
                rows_referencing_blocks(&buf),
                "round {round}"
            );
            freed |= buf.blocks.len() < initial_blocks;
        }
        assert!(freed, "eviction must have released at least one block");
        assert_eq!(buf.rows.len(), 3 + 40, "limit stays exact");
    }

    /// The count survives an alternate-screen round trip: the blocks are
    /// parked with the primary rows and restored with them.
    #[test]
    fn counts_survive_an_alternate_screen_round_trip() {
        let mut buf = buffer_with_compact_scrollback(12);
        assert!(buf.compress_scrollback_block(0, 6));
        buf.enter_alternate(0);
        let _ = buf.leave_alternate();
        assert_eq!(only_block_id_live_rows(&buf), 6);
        buf.debug_assert_invariants();
    }

    /// ED 3 must not discard a block that still has rows in the visible
    /// window. Growing the window upward over compressed rows leaves them
    /// compressed until something reads them, so the block straddles the
    /// scrollback boundary when `erase_scrollback` evicts everything above it.
    #[test]
    fn erase_scrollback_keeps_a_block_that_straddles_the_visible_window() {
        let mut buf = buffer_with_compact_scrollback(14);
        let visible_start = buf.visible_window_start(0);
        assert!(visible_start >= 8, "test needs scrollback");
        assert!(buf.compress_scrollback_block(0, visible_start));
        let id = only_block_id(&buf);

        // Grow the window over the last compressed rows.
        let _ = buf.set_size(20, 3 + 4, 0);
        let new_start = buf.visible_window_start(0);
        assert!(
            new_start > 0 && new_start < visible_start,
            "the window must now reach into the block: {new_start} vs {visible_start}"
        );

        buf.erase_scrollback();

        // The rows still in the window keep their compressed content.
        let straddling = visible_start - new_start;
        assert_eq!(buf.blocks.len(), 1, "the block must survive ED 3");
        assert_eq!(
            buf.blocks[&id].live_rows,
            u32::value_from(straddling).unwrap()
        );
        assert_eq!(rows_referencing_blocks(&buf), straddling);
        buf.debug_assert_invariants();

        // And they still read back correctly.
        let (chars, ..) = buf.visible_as_tchars_and_tags(0);
        let text: String = chars
            .iter()
            .map(|c| match c {
                TChar::Ascii(b) => char::from(*b),
                _ => '?',
            })
            .collect();
        assert!(
            text.contains(&format!("line{new_start:04}")),
            "the first surviving row must be intact: {text}"
        );
        assert!(buf.blocks.is_empty(), "reading restored the block");
    }
}
