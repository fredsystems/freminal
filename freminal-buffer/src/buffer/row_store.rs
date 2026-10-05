// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The buffer's row storage: rows plus the two per-row side tables that must
//! stay index-parallel with them (Task 125.13).
//!
//! [`RowStore`] owns, as one unit:
//!
//! - `rows` — the [`Row`]s themselves (scrollback + visible region);
//! - `cache` — each row's flat-representation cache entry
//!   ([`RowCacheEntry`]); `None` means "dirty, re-flatten on next snapshot";
//! - `blocks` — each row's reference into a compressed scrollback block
//!   ([`BlockRowRef`]); `None` means "not currently compressed".
//!
//! Before this type existed those were three `Vec`s on [`Buffer`](super::Buffer)
//! that every structural edit had to update by hand, with one documented
//! exception (the block map was allowed to lag the rows). Every *structural*
//! mutation — anything that changes the number or order of rows — now goes
//! through a [`RowStore`] method that updates all three together, so the three
//! lengths are equal by construction and the "block map may lag" relaxation no
//! longer exists.
//!
//! Row *content* is still edited in place through `DerefMut<Target = [Row]>`;
//! that never changes any length. The two side tables are reached through
//! [`RowStore::cache`] / [`RowStore::block_map`] and their `_mut` forms, or
//! all at once, as disjoint borrows, through [`RowStore::split_mut`].
//!
//! # Logical row numbers (Task 125.14)
//!
//! The store also owns a `base`: the [`RowNumber`] of the row at index `0`.
//! Row `i` is numbered `base + i`. Front eviction advances `base` by the number
//! of rows removed, so a surviving row's number never changes; anything that
//! must keep pointing at a row across eviction stores its [`RowNumber`] and
//! converts back with [`RowStore::index_of`]. A number is never re-issued after
//! front eviction (a trailing [`RowStore::pop`] does re-issue the popped row's
//! number: that row was never durable content, only blank screen padding).
//!
//! # Moving-head storage and O(evicted) eviction (Task 125.15)
//!
//! Storage is three plain `Vec`s sharing one `head`. The *live* region is
//! `[head..]`; `[..head]` holds *dead slots*: the zero-width placeholder row,
//! `None` cache entry and `None` block reference that
//! [`RowStore::evict_front`] leaves behind. Evicting from the front therefore
//! costs O(rows evicted): each evicted slot's payload (cell storage, flatten
//! cache bytes) is dropped immediately, `head` and `base` advance, and no
//! surviving row moves.
//!
//! Every read accessor (`Deref`, `len`, iteration, `cache`, `block_map`,
//! [`RowStore::split_mut`]) exposes only the live region, so the dead slots
//! are invisible: indices are always live-relative and the logical length is
//! exact at every moment. The dead slots are reclaimed in bulk by
//! *compaction* (`drain(..head)` on all three `Vec`s) once
//! `head >= max(live / COMPACT_LIVE_DIVISOR, MIN_COMPACT_DEAD_SLOTS)`, which
//! keeps the amortised cost of compaction at O(1) moved rows per eviction
//! while bounding the dead-slot overhead to a fixed fraction of the live rows.
//! Compaction changes no row number and no live index.
//!
//! [`RowStore::capacity`] and [`RowStore::cache_capacity`] report the full
//! allocations, dead slots included, so memory accounting stays honest.

use std::ops::{Deref, DerefMut};

use freminal_common::buffer_states::row_number::RowNumber;

use crate::row::Row;

use super::{BlockId, BlockRowRef, RowCacheEntry};

/// Compaction never runs while fewer than this many dead slots exist, so a
/// small store (the alternate screen, a short test buffer) is not re-packed on
/// every few evictions.
const MIN_COMPACT_DEAD_SLOTS: usize = 64;

/// Compaction runs once the dead slots reach `live / COMPACT_LIVE_DIVISOR`
/// (and at least [`MIN_COMPACT_DEAD_SLOTS`]).
///
/// With the divisor `2`, compaction moves the live rows once per `live / 2`
/// evictions: the amortised cost is at most two row moves per evicted row,
/// and the dead slots never exceed half the live rows (so the slot arrays
/// stay within 1.5x the live size). A larger divisor compacts sooner and
/// wastes less memory at the price of more frequent moves; a smaller one is
/// the reverse. Valid range per the 125.11 record: `live / 4 ..= live`.
const COMPACT_LIVE_DIVISOR: usize = 2;

/// The inert row left in a dead slot. Zero-width and unallocated, so keeping
/// a dead slot costs only the `Row` struct itself.
const fn dead_row() -> Row {
    Row::new(0)
}

/// The live region `[head..]` of one of the store's tables.
///
/// `head <= table.len()` is a `RowStore` invariant; a violation degrades to an
/// empty slice rather than panicking.
const fn live<T>(table: &[T], head: usize) -> &[T] {
    match table.split_at_checked(head) {
        Some((_, live)) => live,
        None => &[],
    }
}

/// Mutable form of [`live`].
const fn live_mut<T>(table: &mut [T], head: usize) -> &mut [T] {
    match table.split_at_mut_checked(head) {
        Some((_, live)) => live,
        None => &mut [],
    }
}

/// What a [`RowStore::evict_front`] call did.
///
/// Deliberately minimal: it carries only what every current caller needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::buffer) struct EvictionReport {
    /// Number of rows actually removed from the front. Equals the requested
    /// count unless that exceeded the number of stored rows, in which case it
    /// is clamped to the row count.
    pub rows: usize,
}

/// Rows, their flatten-cache entries, and their compressed-block references,
/// kept index-parallel by construction. See the module docs.
///
/// Invariants: the three tables have equal length; `head <= rows.len()`; every
/// slot in `[..head]` is a dead slot (placeholder row, `None`, `None`).
#[derive(Debug, Clone)]
pub(in crate::buffer) struct RowStore {
    rows: Vec<Row>,
    cache: Vec<Option<RowCacheEntry>>,
    blocks: Vec<Option<BlockRowRef>>,
    /// Number of dead slots at the front of each table; the live region is
    /// `[head..]`. See the module docs.
    head: usize,
    /// Logical number of the live row at index `0`. See the module docs.
    base: RowNumber,
}

impl RowStore {
    // ----------------------------------------------------------------
    // Logical row numbers
    // ----------------------------------------------------------------

    /// Logical number of the oldest retained row (index `0`).
    pub(in crate::buffer) const fn base(&self) -> RowNumber {
        self.base
    }

    /// Logical number the next pushed row would receive: `base + len`.
    pub(in crate::buffer) fn next_number(&self) -> RowNumber {
        self.base.saturating_add(self.len())
    }

    /// Logical number of the row at `index`.
    ///
    /// Defined for any index, including one past the end (the number the next
    /// pushed row will get); it is plain arithmetic, not a bounds check.
    pub(in crate::buffer) fn number_of(&self, index: usize) -> RowNumber {
        self.base.saturating_add(index)
    }

    /// Retained index of the row numbered `number`, or `None` when no such row
    /// is stored: evicted (below `base`), not yet created (at or past
    /// `next_number`), or in a different namespace.
    pub(in crate::buffer) fn index_of(&self, number: RowNumber) -> Option<usize> {
        number
            .rows_after(self.base)
            .filter(|&index| index < self.len())
    }

    /// A store of `rows` whose first row is numbered `base`; every cache entry
    /// and block reference is `None`.
    pub(in crate::buffer) fn from_rows_at(
        base: RowNumber,
        rows: impl IntoIterator<Item = Row>,
    ) -> Self {
        let mut store: Self = rows.into_iter().collect();
        store.base = base;
        store
    }

    // ----------------------------------------------------------------
    // Read access (live region only)
    // ----------------------------------------------------------------

    /// All live rows as a slice. `const`, unlike going through `Deref`.
    pub(in crate::buffer) const fn as_slice(&self) -> &[Row] {
        live(self.rows.as_slice(), self.head)
    }

    /// Number of live rows. Also available through `Deref`, but that path is
    /// not `const`, and `const fn` buffer methods need this.
    pub(in crate::buffer) const fn len(&self) -> usize {
        self.rows.len().saturating_sub(self.head)
    }

    /// `true` if there are no live rows. See [`Self::len`] for why this is
    /// inherent.
    pub(in crate::buffer) const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Capacity of the row allocation, in rows, **dead slots included**. Used
    /// by memory accounting.
    pub(in crate::buffer) const fn capacity(&self) -> usize {
        self.rows.capacity()
    }

    /// Capacity of the flatten-cache allocation, in entries, **dead slots
    /// included**. Used by memory accounting, which reports the cache
    /// allocation separately from rows.
    pub(in crate::buffer) const fn cache_capacity(&self) -> usize {
        self.cache.capacity()
    }

    /// Per-row flatten-cache entries, index-parallel to the live rows.
    pub(in crate::buffer) const fn cache(&self) -> &[Option<RowCacheEntry>] {
        live(self.cache.as_slice(), self.head)
    }

    /// Mutable per-row flatten-cache entries, index-parallel to the live rows.
    pub(in crate::buffer) const fn cache_mut(&mut self) -> &mut [Option<RowCacheEntry>] {
        live_mut(self.cache.as_mut_slice(), self.head)
    }

    /// Per-row compressed-block references, index-parallel to the live rows.
    pub(in crate::buffer) const fn block_map(&self) -> &[Option<BlockRowRef>] {
        live(self.blocks.as_slice(), self.head)
    }

    /// Mutable per-row compressed-block references, index-parallel to the live
    /// rows.
    pub(in crate::buffer) const fn block_map_mut(&mut self) -> &mut [Option<BlockRowRef>] {
        live_mut(self.blocks.as_mut_slice(), self.head)
    }

    /// Borrow rows, cache entries and block references mutably *at once*.
    ///
    /// The three slices are disjoint (they are different allocations), cover
    /// the live region only, and all have the same length. This exists for the
    /// sites that must hold a mutable window over the rows and the matching
    /// window over the cache simultaneously, which separate `&mut self`
    /// accessors cannot express.
    pub(in crate::buffer) const fn split_mut(
        &mut self,
    ) -> (
        &mut [Row],
        &mut [Option<RowCacheEntry>],
        &mut [Option<BlockRowRef>],
    ) {
        (
            live_mut(self.rows.as_mut_slice(), self.head),
            live_mut(self.cache.as_mut_slice(), self.head),
            live_mut(self.blocks.as_mut_slice(), self.head),
        )
    }

    // ----------------------------------------------------------------
    // Cache invalidation
    // ----------------------------------------------------------------

    /// Drop row `index`'s flatten-cache entry so the next flatten rebuilds it.
    ///
    /// Out-of-range indices are ignored. This is the one tolerant accessor:
    /// it exists for the call sites that were already written as
    /// `if i < cache.len() { cache[i] = None }`. It does **not** touch the
    /// row's own `dirty` flag.
    pub(in crate::buffer) fn invalidate(&mut self, index: usize) {
        if let Some(entry) = self.cache_mut().get_mut(index) {
            *entry = None;
        }
    }

    // ----------------------------------------------------------------
    // Structural mutators: all three tables move together
    // ----------------------------------------------------------------

    /// Append `row` at the bottom with an empty cache entry and no block
    /// reference.
    pub(in crate::buffer) fn push(&mut self, row: Row) {
        self.rows.push(row);
        self.cache.push(None);
        self.blocks.push(None);
    }

    /// Remove and return the bottom row together with its cache entry and
    /// block reference (both discarded).
    ///
    /// The caller is responsible for any bookkeeping keyed on a discarded
    /// block reference; the rows popped today are blank padding that is never
    /// compressed.
    pub(in crate::buffer) fn pop(&mut self) -> Option<Row> {
        if self.is_empty() {
            return None;
        }
        let row = self.rows.pop()?;
        self.cache.pop();
        let block = self.blocks.pop();
        debug_assert!(
            block.is_none_or(|slot| slot.is_none()),
            "pop discarded a compressed-block reference; the caller owns its live-row count"
        );
        self.reset_if_drained();
        Some(row)
    }

    /// Remove the first `n` rows (clamped to the number stored) along with
    /// their cache entries and block references, in O(`n`).
    ///
    /// Each evicted slot becomes a dead slot (see the module docs): its row,
    /// cache entry and block reference are dropped immediately, so no payload
    /// outlives the eviction, and `head` / [`Self::base`] advance. No
    /// surviving row moves, so every surviving row keeps its index-relative
    /// order and its logical number. Compaction of the dead slots is deferred
    /// until enough have accumulated.
    ///
    /// `BlockRowRef::offset_in_block` is block-relative, so the surviving
    /// references need no remapping. For every evicted row that referenced a
    /// compressed block, `release_block_rows(block, count)` is called with the
    /// number of consecutive evicted rows of that block (a run), so the
    /// caller can maintain per-block live-row counts. The caller remains
    /// responsible for any other bookkeeping keyed on the removed rows (image
    /// cell counts, image reachability, marks).
    pub(in crate::buffer) fn evict_front(
        &mut self,
        n: usize,
        mut release_block_rows: impl FnMut(BlockId, u32),
    ) -> EvictionReport {
        let n = n.min(self.len());
        if n == 0 {
            return EvictionReport { rows: 0 };
        }
        let evicted = self.head..self.head + n;

        // A run of consecutive evicted rows that share one block, flushed to
        // `release_block_rows` when the block changes or the range ends.
        let mut run: Option<(BlockId, u32)> = None;
        for ((row, entry), block) in self.rows[evicted.clone()]
            .iter_mut()
            .zip(self.cache[evicted.clone()].iter_mut())
            .zip(self.blocks[evicted].iter_mut())
        {
            // Dropping the old values frees their payloads right here.
            drop(std::mem::replace(row, dead_row()));
            drop(entry.take());
            if let Some(block_ref) = block.take() {
                let id = block_ref.block_id();
                run = match run {
                    Some((run_id, count)) if run_id == id => {
                        Some((run_id, count.saturating_add(1)))
                    }
                    Some((run_id, count)) => {
                        release_block_rows(run_id, count);
                        Some((id, 1))
                    }
                    None => Some((id, 1)),
                };
            }
        }
        if let Some((run_id, count)) = run {
            release_block_rows(run_id, count);
        }

        self.head += n;
        self.base = self.base.saturating_add(n);
        if !self.reset_if_drained() && self.head >= self.compaction_threshold() {
            self.compact();
        }
        EvictionReport { rows: n }
    }

    /// Replace every row with `rows`, resetting the cache and block map to
    /// match: every entry `None`.
    ///
    /// The new rows are numbered from the *old* [`Self::next_number`], so no
    /// row number is ever reused: anything still holding a number from the
    /// replaced content falls below the new `base` and is detectably invalid
    /// rather than aliasing a new row.
    pub(in crate::buffer) fn replace_all(&mut self, rows: Vec<Row>) {
        let base = self.next_number();
        *self = Self::from_rows_at(base, rows);
    }

    /// Remove and return every live row, leaving the store empty (all three
    /// tables empty, so still in lockstep). The cache entries and block
    /// references are discarded with the store's old contents. [`Self::base`]
    /// advances past the removed rows so numbers are not reused.
    pub(in crate::buffer) fn take_rows(&mut self) -> Vec<Row> {
        let next = self.next_number();
        let mut taken = std::mem::take(self);
        self.base = next;
        taken.rows.drain(..taken.head);
        taken.rows
    }

    /// Remove every row, cache entry and block reference. [`Self::base`]
    /// advances past the removed rows.
    #[cfg(test)]
    pub(in crate::buffer) fn clear_all(&mut self) {
        self.base = self.next_number();
        self.rows.clear();
        self.cache.clear();
        self.blocks.clear();
        self.head = 0;
    }

    // ----------------------------------------------------------------
    // Dead-slot management
    // ----------------------------------------------------------------

    /// Number of dead slots currently held at the front of the tables.
    #[cfg(test)]
    pub(in crate::buffer) const fn dead_slots(&self) -> usize {
        self.head
    }

    /// Dead-slot count at which [`Self::evict_front`] compacts:
    /// `max(live / COMPACT_LIVE_DIVISOR, MIN_COMPACT_DEAD_SLOTS)`.
    const fn compaction_threshold(&self) -> usize {
        let proportional = self.len() / COMPACT_LIVE_DIVISOR;
        if proportional > MIN_COMPACT_DEAD_SLOTS {
            proportional
        } else {
            MIN_COMPACT_DEAD_SLOTS
        }
    }

    /// Drop the dead slots from all three tables. O(live rows); changes no
    /// live index and no row number.
    fn compact(&mut self) {
        self.rows.drain(..self.head);
        self.cache.drain(..self.head);
        self.blocks.drain(..self.head);
        self.head = 0;
    }

    /// If no live row remains, discard the (all-dead) tables outright instead
    /// of waiting for the compaction threshold. Returns whether it did.
    fn reset_if_drained(&mut self) -> bool {
        if self.rows.len() != self.head {
            return false;
        }
        self.rows.clear();
        self.cache.clear();
        self.blocks.clear();
        self.head = 0;
        true
    }
}

impl Default for RowStore {
    /// An empty store numbered from [`RowNumber::ZERO`].
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            cache: Vec::new(),
            blocks: Vec::new(),
            head: 0,
            base: RowNumber::ZERO,
        }
    }
}

impl Deref for RowStore {
    type Target = [Row];

    fn deref(&self) -> &[Row] {
        self.as_slice()
    }
}

impl DerefMut for RowStore {
    /// Mutable access to row *content* (live rows only). The slice cannot
    /// change the number of rows, so the side tables stay parallel. Reordering
    /// through the slice (`swap`, `rotate_*`, ...) would **not** move the side
    /// tables; callers that relocate rows are responsible for relocating the
    /// matching cache entries themselves, exactly as before this type existed.
    fn deref_mut(&mut self) -> &mut [Row] {
        live_mut(self.rows.as_mut_slice(), self.head)
    }
}

impl<'a> IntoIterator for &'a RowStore {
    type Item = &'a Row;
    type IntoIter = std::slice::Iter<'a, Row>;

    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

impl<'a> IntoIterator for &'a mut RowStore {
    type Item = &'a mut Row;
    type IntoIter = std::slice::IterMut<'a, Row>;

    fn into_iter(self) -> Self::IntoIter {
        live_mut(self.rows.as_mut_slice(), self.head).iter_mut()
    }
}

impl FromIterator<Row> for RowStore {
    /// Build a store from rows, giving each an empty cache entry and no block
    /// reference. Numbering starts at [`RowNumber::ZERO`]; use
    /// [`RowStore::from_rows_at`] for another base.
    fn from_iter<I: IntoIterator<Item = Row>>(iter: I) -> Self {
        let rows: Vec<Row> = iter.into_iter().collect();
        let len = rows.len();
        Self {
            rows,
            cache: vec![None; len],
            blocks: vec![None; len],
            head: 0,
            base: RowNumber::ZERO,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::buffer::BlockId;

    /// A row identifiable by its width.
    fn marked_row(marker: usize) -> Row {
        Row::new(marker)
    }

    fn marker_of(row: &Row) -> usize {
        row.max_width()
    }

    /// A block reference identifiable by its block id (`BlockId` is opaque
    /// outside `crate::buffer`, so compare through `block_id()`).
    fn marked_ref(marker: u32) -> BlockRowRef {
        BlockRowRef::new(BlockId::new(marker), 0)
    }

    fn ref_marker(r: Option<BlockRowRef>) -> Option<BlockId> {
        r.map(BlockRowRef::block_id)
    }

    /// A store of `n` rows marked `0..n`, where every cache entry is `Some`
    /// and every block reference is `Some`, so a lockstep failure in any
    /// mutator shows up as a mismatched marker rather than a silent `None`.
    fn full_store(n: usize) -> RowStore {
        let mut store: RowStore = (0..n).map(marked_row).collect();
        for (i, entry) in store.cache_mut().iter_mut().enumerate() {
            let mut e = RowCacheEntry::empty();
            e.bytes = vec![0; i + 1];
            *entry = Some(e);
        }
        for (i, r) in store.block_map_mut().iter_mut().enumerate() {
            *r = Some(marked_ref(u32::try_from(i).unwrap()));
        }
        store
    }

    /// The three tables have equal length.
    fn assert_lockstep(store: &RowStore) {
        assert_eq!(store.cache().len(), store.len(), "cache length");
        assert_eq!(store.block_map().len(), store.len(), "block map length");
    }

    /// Row `i`, its cache entry and its block reference all still carry the
    /// marker the store was built with, `expected[i]`.
    fn assert_markers(store: &RowStore, expected: &[usize]) {
        assert_lockstep(store);
        assert_eq!(store.len(), expected.len());
        for (i, &m) in expected.iter().enumerate() {
            assert_eq!(marker_of(&store[i]), m, "row {i}");
            assert_eq!(
                store.cache()[i].as_ref().map(|e| e.bytes.len()),
                Some(m + 1),
                "cache {i}"
            );
            assert_eq!(
                ref_marker(store.block_map()[i]),
                Some(BlockId::new(u32::try_from(m).unwrap())),
                "block ref {i}"
            );
        }
    }

    #[test]
    fn default_store_is_empty_and_in_lockstep() {
        let store = RowStore::default();
        assert!(store.is_empty());
        assert_lockstep(&store);
        assert_eq!(store.as_slice().len(), 0);
    }

    #[test]
    fn from_iter_creates_matching_none_entries() {
        let store: RowStore = (0..5).map(marked_row).collect();
        assert_eq!(store.len(), 5);
        assert_lockstep(&store);
        assert!(store.cache().iter().all(Option::is_none));
        assert!(store.block_map().iter().all(Option::is_none));
        for (i, row) in store.iter().enumerate() {
            assert_eq!(marker_of(row), i);
        }
    }

    #[test]
    fn from_iter_of_nothing_is_empty() {
        let store: RowStore = std::iter::empty().collect();
        assert!(store.is_empty());
        assert_lockstep(&store);
    }

    #[test]
    fn push_extends_all_three_with_none() {
        let mut store = full_store(3);
        store.push(marked_row(99));
        assert_eq!(store.len(), 4);
        assert_lockstep(&store);
        assert_eq!(marker_of(&store[3]), 99);
        assert!(store.cache()[3].is_none());
        assert!(store.block_map()[3].is_none());
        // Existing entries are untouched.
        assert_markers_prefix(&store, 3);
    }

    /// The first `n` entries of a `full_store` are still `0..n`.
    fn assert_markers_prefix(store: &RowStore, n: usize) {
        for i in 0..n {
            assert_eq!(marker_of(&store[i]), i);
            assert!(store.cache()[i].is_some());
            assert!(store.block_map()[i].is_some());
        }
    }

    #[test]
    fn push_onto_empty_store() {
        let mut store = RowStore::default();
        store.push(marked_row(7));
        assert_eq!(store.len(), 1);
        assert_lockstep(&store);
    }

    #[test]
    fn pop_shrinks_all_three_and_returns_the_row() {
        let mut store = full_store(4);
        let popped = pop_clean(&mut store).unwrap();
        assert_eq!(marker_of(&popped), 3);
        assert_markers(&store, &[0, 1, 2]);
    }

    #[test]
    fn pop_on_empty_store_is_none_and_keeps_lockstep() {
        let mut store = RowStore::default();
        assert!(store.pop().is_none());
        assert_lockstep(&store);
    }

    #[test]
    fn evict_front_drops_front_of_all_three() {
        let mut store = full_store(6);
        let report = store.evict_front(2, |_, _| {});
        assert_eq!(report, EvictionReport { rows: 2 });
        assert_markers(&store, &[2, 3, 4, 5]);
    }

    #[test]
    fn evict_front_zero_is_a_no_op() {
        let mut store = full_store(3);
        let report = store.evict_front(0, |_, _| {});
        assert_eq!(report.rows, 0);
        assert_markers(&store, &[0, 1, 2]);
    }

    #[test]
    fn evict_front_clamps_to_len_and_reports_actual() {
        let mut store = full_store(3);
        let report = store.evict_front(10, |_, _| {});
        assert_eq!(report.rows, 3);
        assert!(store.is_empty());
        assert_lockstep(&store);
    }

    #[test]
    fn evict_front_all_rows_exactly() {
        let mut store = full_store(3);
        let report = store.evict_front(3, |_, _| {});
        assert_eq!(report.rows, 3);
        assert!(store.is_empty());
        assert_lockstep(&store);
    }

    #[test]
    fn evict_front_then_push_keeps_lockstep() {
        let mut store = full_store(4);
        let _ = store.evict_front(1, |_, _| {});
        store.push(marked_row(50));
        assert_lockstep(&store);
        assert_eq!(store.len(), 4);
        assert_eq!(marker_of(&store[0]), 1);
        assert_eq!(marker_of(&store[3]), 50);
        assert!(store.cache()[3].is_none());
        assert!(store.block_map()[3].is_none());
    }

    #[test]
    fn replace_all_resets_side_tables_to_none() {
        let mut store = full_store(4);
        store.replace_all(vec![marked_row(10), marked_row(11)]);
        assert_eq!(store.len(), 2);
        assert_lockstep(&store);
        assert_eq!(marker_of(&store[0]), 10);
        assert!(store.cache().iter().all(Option::is_none));
        assert!(store.block_map().iter().all(Option::is_none));
    }

    #[test]
    fn replace_all_with_nothing_empties_the_store() {
        let mut store = full_store(4);
        store.replace_all(Vec::new());
        assert!(store.is_empty());
        assert_lockstep(&store);
    }

    #[test]
    fn take_rows_returns_the_rows_and_leaves_an_empty_lockstep_store() {
        let mut store = full_store(3);
        let rows = store.take_rows();
        let markers: Vec<usize> = rows.iter().map(marker_of).collect();
        assert_eq!(markers, vec![0, 1, 2]);
        assert!(store.is_empty());
        assert_lockstep(&store);
        // The store is reusable afterwards.
        store.push(marked_row(9));
        assert_eq!(store.len(), 1);
        assert_lockstep(&store);
    }

    #[test]
    fn clear_all_empties_all_three() {
        let mut store = full_store(4);
        store.clear_all();
        assert!(store.is_empty());
        assert_lockstep(&store);
    }

    #[test]
    fn invalidate_clears_only_that_cache_entry() {
        let mut store = full_store(3);
        store.invalidate(1);
        assert!(store.cache()[0].is_some());
        assert!(store.cache()[1].is_none());
        assert!(store.cache()[2].is_some());
        // Block references are untouched by cache invalidation.
        assert!(store.block_map().iter().all(Option::is_some));
        assert_lockstep(&store);
    }

    #[test]
    fn invalidate_out_of_range_is_ignored() {
        let mut store = full_store(2);
        store.invalidate(2);
        store.invalidate(usize::MAX);
        assert_markers(&store, &[0, 1]);
    }

    #[test]
    fn split_mut_slices_are_equal_length_and_independently_mutable() {
        let mut store = full_store(4);
        {
            let (rows, cache, blocks) = store.split_mut();
            assert_eq!(rows.len(), 4);
            assert_eq!(cache.len(), 4);
            assert_eq!(blocks.len(), 4);
            // All three held mutably at the same time.
            rows[0].dirty = true;
            cache[1] = None;
            blocks[2] = None;
        }
        assert!(store[0].dirty);
        assert!(store.cache()[1].is_none());
        assert!(store.block_map()[2].is_none());
        // Nothing else moved.
        assert!(store.cache()[0].is_some());
        assert!(store.cache()[2].is_some());
        assert!(store.block_map()[1].is_some());
        assert_lockstep(&store);
    }

    #[test]
    fn split_mut_windows_can_be_taken_over_a_sub_range() {
        let mut store = full_store(5);
        // Freshly built rows start dirty; start from all-clean so the
        // assertions below can see exactly which window was touched.
        for row in &mut store {
            row.dirty = false;
        }
        let (rows, cache, _) = store.split_mut();
        let (rows, cache) = (&mut rows[1..4], &mut cache[1..4]);
        for (row, entry) in rows.iter_mut().zip(cache.iter_mut()) {
            row.dirty = true;
            *entry = None;
        }
        assert!(!store[0].dirty);
        assert!(store[1].dirty && store[2].dirty && store[3].dirty);
        assert!(!store[4].dirty);
        assert!(store.cache()[0].is_some());
        assert!(store.cache()[1..4].iter().all(Option::is_none));
        assert!(store.cache()[4].is_some());
    }

    #[test]
    fn deref_mut_edits_row_content_without_changing_lengths() {
        let mut store = full_store(3);
        store[1].dirty = true;
        for row in &mut store {
            row.dirty = true;
        }
        assert!(store.iter().all(|r| r.dirty));
        assert_markers(&store, &[0, 1, 2]);
    }

    #[test]
    fn into_iterator_for_shared_ref_yields_rows_in_order() {
        let store = full_store(3);
        let markers: Vec<usize> = (&store).into_iter().map(marker_of).collect();
        assert_eq!(markers, vec![0, 1, 2]);
    }

    #[test]
    fn clone_copies_all_three_tables() {
        let store = full_store(3);
        let copy = store.clone();
        assert_markers(&copy, &[0, 1, 2]);
        assert_markers(&store, &[0, 1, 2]);
    }

    #[test]
    fn capacities_cover_the_stored_rows() {
        let store = full_store(5);
        assert!(store.capacity() >= 5);
        assert!(store.cache_capacity() >= 5);
    }

    #[test]
    fn mixed_mutation_sequence_keeps_lockstep() {
        let mut store = full_store(5);
        let _ = store.evict_front(2, |_, _| {});
        store.push(marked_row(40));
        store.push(marked_row(41));
        let _ = pop_clean(&mut store);
        let _ = store.evict_front(1, |_, _| {});
        store.push(marked_row(42));
        assert_lockstep(&store);
        let markers: Vec<usize> = store.iter().map(marker_of).collect();
        assert_eq!(markers, vec![3, 4, 40, 42]);
        // The two surviving original rows kept their own cache/block entries.
        assert_eq!(store.cache()[0].as_ref().map(|e| e.bytes.len()), Some(4));
        assert_eq!(store.cache()[1].as_ref().map(|e| e.bytes.len()), Some(5));
        assert!(store.cache()[2..].iter().all(Option::is_none));
        assert!(store.block_map()[2..].iter().all(Option::is_none));
    }

    // ── Logical row numbers (Task 125.14) ───────────────────────────────

    #[test]
    fn default_and_from_iter_stores_are_numbered_from_zero() {
        assert_eq!(RowStore::default().base(), RowNumber::ZERO);
        let store = full_store(3);
        assert_eq!(store.base(), RowNumber::ZERO);
        assert_eq!(store.next_number(), RowNumber::new(3));
    }

    #[test]
    fn from_rows_at_numbers_the_first_row_at_the_given_base() {
        let store = RowStore::from_rows_at(RowNumber::new(40), (0..3).map(marked_row));
        assert_eq!(store.base(), RowNumber::new(40));
        assert_eq!(store.next_number(), RowNumber::new(43));
        assert_eq!(store.number_of(2), RowNumber::new(42));
        assert_lockstep(&store);
    }

    #[test]
    fn number_of_and_index_of_are_inverse_within_the_stored_rows() {
        let store = RowStore::from_rows_at(RowNumber::new(10), (0..4).map(marked_row));
        for i in 0..4 {
            assert_eq!(store.index_of(store.number_of(i)), Some(i));
        }
    }

    #[test]
    fn index_of_rejects_numbers_outside_the_stored_rows() {
        let store = RowStore::from_rows_at(RowNumber::new(10), (0..4).map(marked_row));
        assert_eq!(store.index_of(RowNumber::new(9)), None, "below the base");
        assert_eq!(store.index_of(RowNumber::new(14)), None, "past the end");
        assert_eq!(store.index_of(RowNumber::ZERO), None);
        assert_eq!(
            store.index_of(RowNumber::ALTERNATE_BASE),
            None,
            "other namespace"
        );
    }

    #[test]
    fn number_of_past_the_end_is_the_next_number() {
        let store = full_store(3);
        assert_eq!(store.number_of(3), store.next_number());
    }

    #[test]
    fn evict_front_advances_the_base_and_keeps_surviving_numbers() {
        let mut store = full_store(6);
        let survivor = store.number_of(4);
        let _ = store.evict_front(2, |_, _| {});
        assert_eq!(store.base(), RowNumber::new(2));
        assert_eq!(store.index_of(survivor), Some(2));
        assert_eq!(marker_of(&store[2]), 4, "the number still names row 4");
        assert_eq!(store.index_of(RowNumber::new(1)), None, "evicted");
    }

    #[test]
    fn evict_front_advances_the_base_by_the_clamped_count_only() {
        let mut store = full_store(3);
        let _ = store.evict_front(10, |_, _| {});
        assert_eq!(store.base(), RowNumber::new(3));
        assert_eq!(store.next_number(), RowNumber::new(3));
        let _ = store.evict_front(0, |_, _| {});
        assert_eq!(store.base(), RowNumber::new(3));
    }

    #[test]
    fn numbers_are_not_reissued_after_front_eviction() {
        let mut store = full_store(4);
        let _ = store.evict_front(2, |_, _| {});
        store.push(marked_row(50));
        store.push(marked_row(51));
        // 4 rows existed (numbers 0..4); 2 evicted; 2 pushed: next numbers 4, 5.
        assert_eq!(store.number_of(2), RowNumber::new(4));
        assert_eq!(store.next_number(), RowNumber::new(6));
        assert_eq!(store.index_of(RowNumber::new(0)), None);
        assert_eq!(store.index_of(RowNumber::new(1)), None);
    }

    #[test]
    fn pop_reissues_only_the_trailing_rows_number() {
        let mut store = full_store(3);
        let _ = pop_clean(&mut store);
        assert_eq!(store.base(), RowNumber::ZERO, "pop never moves the base");
        assert_eq!(store.next_number(), RowNumber::new(2));
        store.push(marked_row(9));
        assert_eq!(
            store.number_of(2),
            RowNumber::new(2),
            "tail number re-issued"
        );
    }

    #[test]
    fn replace_all_numbers_the_new_rows_past_the_old_ones() {
        let mut store = full_store(4);
        let old_row = store.number_of(1);
        store.replace_all(vec![marked_row(10), marked_row(11)]);
        assert_eq!(
            store.base(),
            RowNumber::new(4),
            "numbered from old next_number"
        );
        assert_eq!(store.index_of(old_row), None, "old numbers do not alias");
        assert_eq!(store.index_of(RowNumber::new(5)), Some(1));
    }

    #[test]
    fn take_rows_advances_the_base_past_the_removed_rows() {
        let mut store = full_store(3);
        let _ = store.take_rows();
        assert_eq!(store.base(), RowNumber::new(3));
        assert!(store.is_empty());
        // A following replace_all keeps counting forward.
        store.replace_all(vec![marked_row(1)]);
        assert_eq!(store.base(), RowNumber::new(3));
        assert_eq!(store.number_of(0), RowNumber::new(3));
    }

    #[test]
    fn clear_all_advances_the_base_past_the_removed_rows() {
        let mut store = full_store(4);
        store.clear_all();
        assert_eq!(store.base(), RowNumber::new(4));
        assert_eq!(store.next_number(), RowNumber::new(4));
    }

    #[test]
    fn clone_preserves_the_base() {
        let mut store = full_store(5);
        let _ = store.evict_front(3, |_, _| {});
        assert_eq!(store.clone().base(), RowNumber::new(3));
    }

    // ── Moving head (Task 125.15) ───────────────────────────────────────

    /// A big enough store that its compaction threshold is the proportional
    /// one, not the fixed minimum.
    const BIG: usize = 400;

    /// Pop the bottom row after dropping its block reference, which `pop`
    /// requires to be `None` (nothing owns the live-row count of a block
    /// reference it discards).
    fn pop_clean(store: &mut RowStore) -> Option<Row> {
        if let Some(last) = store.block_map_mut().last_mut() {
            *last = None;
        }
        store.pop()
    }

    fn evict(store: &mut RowStore, n: usize) -> EvictionReport {
        store.evict_front(n, |_, _| {})
    }

    #[test]
    fn evict_front_leaves_dead_slots_and_moves_nothing() {
        let mut store = full_store(10);
        let _ = evict(&mut store, 3);
        assert_eq!(store.dead_slots(), 3);
        assert_eq!(store.len(), 7);
        assert_markers(&store, &[3, 4, 5, 6, 7, 8, 9]);
        // The physical tables still hold the dead prefix.
        assert_eq!(store.rows.len(), 10);
        assert_eq!(store.cache.len(), 10);
        assert_eq!(store.blocks.len(), 10);
    }

    #[test]
    fn evicted_payloads_are_dropped_immediately() {
        let mut store = full_store(6);
        let _ = evict(&mut store, 4);
        for dead in 0..4 {
            assert_eq!(store.rows[dead].max_width(), 0, "dead row {dead}");
            assert!(store.cache[dead].is_none(), "dead cache {dead}");
            assert!(store.blocks[dead].is_none(), "dead block ref {dead}");
        }
    }

    #[test]
    fn dead_slots_are_invisible_to_every_accessor() {
        let mut store = full_store(8);
        let _ = evict(&mut store, 5);
        assert_eq!(store.len(), 3);
        assert_eq!(store.as_slice().len(), 3);
        assert_eq!(store.iter().count(), 3);
        assert_eq!((&store).into_iter().count(), 3);
        assert_eq!((&mut store).into_iter().count(), 3);
        assert_eq!(store.cache().len(), 3);
        assert_eq!(store.cache_mut().len(), 3);
        assert_eq!(store.block_map().len(), 3);
        assert_eq!(store.block_map_mut().len(), 3);
        assert_eq!(marker_of(&store[0]), 5);
        assert_eq!(marker_of(store.last().unwrap()), 7);
        assert_eq!(store.first().map(marker_of), Some(5));
    }

    #[test]
    fn logical_length_is_exact_through_a_run_of_evict_and_push() {
        let mut store = full_store(BIG);
        for step in 0..3 * BIG {
            let _ = evict(&mut store, 1);
            store.push(marked_row(1000 + step));
            assert_eq!(store.len(), BIG, "step {step}");
            assert_lockstep(&store);
        }
    }

    #[test]
    fn small_stores_do_not_compact_below_the_minimum() {
        let mut store = full_store(MIN_COMPACT_DEAD_SLOTS + 10);
        let _ = evict(&mut store, MIN_COMPACT_DEAD_SLOTS - 1);
        assert_eq!(
            store.dead_slots(),
            MIN_COMPACT_DEAD_SLOTS - 1,
            "below the minimum nothing compacts"
        );
    }

    #[test]
    fn compaction_runs_exactly_when_the_dead_slots_reach_the_threshold() {
        // After evicting `k` rows one at a time, `k` slots are dead and
        // `BIG - k` are live, so compaction fires at the first `k` with
        // `k >= max((BIG - k) / divisor, minimum)`.
        let expected = (1..BIG)
            .find(|&k| k >= ((BIG - k) / COMPACT_LIVE_DIVISOR).max(MIN_COMPACT_DEAD_SLOTS))
            .unwrap();

        let mut store = full_store(BIG);
        for k in 1..expected {
            let _ = evict(&mut store, 1);
            assert_eq!(store.dead_slots(), k, "no compaction after {k} evictions");
        }
        let _ = evict(&mut store, 1);
        assert_eq!(
            store.dead_slots(),
            0,
            "compacted after {expected} evictions"
        );
        assert_eq!(store.rows.len(), store.len(), "tables are packed again");
        assert_eq!(store.len(), BIG - expected);
        assert_lockstep(&store);
    }

    #[test]
    fn compaction_threshold_is_half_the_live_rows_but_never_below_the_minimum() {
        let big = full_store(BIG);
        assert_eq!(big.compaction_threshold(), BIG / COMPACT_LIVE_DIVISOR);
        let small = full_store(10);
        assert_eq!(small.compaction_threshold(), MIN_COMPACT_DEAD_SLOTS);
    }

    #[test]
    fn numbers_and_indices_are_stable_across_compaction() {
        let mut store = full_store(BIG);
        let survivor_number = store.number_of(BIG - 1);
        let marker_before = marker_of(&store[BIG - 1]);

        // Evict enough, one batch, to force a compaction.
        let batch = store.compaction_threshold();
        let _ = evict(&mut store, batch);
        assert_eq!(store.dead_slots(), 0, "the batch compacted");

        assert_eq!(store.base(), RowNumber::new(u64::try_from(batch).unwrap()));
        let index = store.index_of(survivor_number).unwrap();
        assert_eq!(index, BIG - 1 - batch);
        assert_eq!(marker_of(&store[index]), marker_before);
        // The survivor's side tables moved with it.
        assert_eq!(
            store.cache()[index].as_ref().map(|e| e.bytes.len()),
            Some(marker_before + 1)
        );
        assert_eq!(
            ref_marker(store.block_map()[index]),
            Some(BlockId::new(u32::try_from(marker_before).unwrap()))
        );
    }

    #[test]
    fn numbers_stay_stable_over_many_compactions() {
        let mut store = full_store(BIG);
        let mut expected_base = 0u64;
        for step in 0..4 * BIG {
            let _ = evict(&mut store, 1);
            expected_base += 1;
            store.push(marked_row(BIG + step));
            assert_eq!(store.base(), RowNumber::new(expected_base), "step {step}");
            // The row at the top of the store is always the one numbered
            // `base`, whatever the physical layout is.
            assert_eq!(store.index_of(store.base()), Some(0));
            assert_eq!(
                marker_of(&store[0]),
                usize::try_from(expected_base).unwrap(),
                "step {step}"
            );
        }
    }

    #[test]
    fn evicting_everything_resets_the_dead_prefix() {
        let mut store = full_store(10);
        let _ = evict(&mut store, 4);
        let report = evict(&mut store, 100);
        assert_eq!(report.rows, 6);
        assert!(store.is_empty());
        assert_eq!(store.dead_slots(), 0);
        assert_eq!(store.rows.len(), 0, "an empty store holds no dead slots");
        assert_eq!(store.base(), RowNumber::new(10));
        // Reusable, and numbering continues.
        store.push(marked_row(1));
        assert_eq!(store.number_of(0), RowNumber::new(10));
        assert_lockstep(&store);
    }

    #[test]
    fn pop_never_reaches_into_the_dead_prefix() {
        let mut store = full_store(4);
        let _ = evict(&mut store, 3);
        assert_eq!(store.len(), 1);
        let popped = pop_clean(&mut store).unwrap();
        assert_eq!(marker_of(&popped), 3);
        assert!(store.is_empty());
        assert!(store.pop().is_none(), "a dead slot is never popped");
        assert_lockstep(&store);
    }

    #[test]
    fn push_after_evict_appends_after_the_live_rows() {
        let mut store = full_store(5);
        let _ = evict(&mut store, 2);
        store.push(marked_row(77));
        assert_eq!(store.len(), 4);
        assert_eq!(marker_of(&store[3]), 77);
        assert!(store.cache()[3].is_none());
        assert!(store.block_map()[3].is_none());
        assert_eq!(store.next_number(), RowNumber::new(6));
    }

    #[test]
    fn invalidate_addresses_live_indices_not_physical_ones() {
        let mut store = full_store(5);
        let _ = evict(&mut store, 2);
        store.invalidate(0);
        assert!(store.cache()[0].is_none());
        assert!(store.cache()[1].is_some());
        // Live index 2 is the last row; 3 is out of range and ignored (it is
        // not a dead slot.
        store.invalidate(2);
        store.invalidate(3);
        assert!(store.cache()[1].is_some());
        assert!(store.cache()[2].is_none());
        assert_eq!(store.cache().len(), 3);
    }

    #[test]
    fn split_mut_windows_are_live_and_disjoint_after_the_head_moves() {
        let mut store = full_store(8);
        let _ = evict(&mut store, 3);
        {
            let (rows, cache, blocks) = store.split_mut();
            assert_eq!((rows.len(), cache.len(), blocks.len()), (5, 5, 5));
            // All three are held mutably at once, over the live region.
            assert_eq!(marker_of(&rows[0]), 3);
            rows[0].dirty = false;
            cache[1] = None;
            blocks[2] = None;
        }
        assert!(!store[0].dirty);
        assert!(store.cache()[1].is_none());
        assert!(store.block_map()[2].is_none());
        // Neighbours are untouched.
        assert!(store.cache()[0].is_some());
        assert!(store.cache()[2].is_some());
        assert!(store.block_map()[1].is_some());
    }

    #[test]
    fn split_mut_sub_windows_work_over_a_moved_head() {
        let mut store = full_store(8);
        let _ = evict(&mut store, 2);
        for row in &mut store {
            row.dirty = false;
        }
        let (rows, cache, _) = store.split_mut();
        for (row, entry) in rows[1..3].iter_mut().zip(cache[1..3].iter_mut()) {
            row.dirty = true;
            *entry = None;
        }
        assert!(!store[0].dirty && store[1].dirty && store[2].dirty && !store[3].dirty);
        assert!(store.cache()[1].is_none() && store.cache()[2].is_none());
        assert!(store.cache()[0].is_some() && store.cache()[3].is_some());
    }

    #[test]
    fn replace_all_after_eviction_starts_a_fresh_packed_store() {
        let mut store = full_store(10);
        let _ = evict(&mut store, 4);
        store.replace_all(vec![marked_row(50), marked_row(51)]);
        assert_eq!(store.dead_slots(), 0);
        assert_eq!(store.len(), 2);
        assert_eq!(store.rows.len(), 2);
        assert_eq!(store.base(), RowNumber::new(10), "numbered from old next");
        assert_eq!(marker_of(&store[0]), 50);
        assert!(store.cache().iter().all(Option::is_none));
        assert_lockstep(&store);
    }

    #[test]
    fn take_rows_after_eviction_returns_only_the_live_rows() {
        let mut store = full_store(6);
        let _ = evict(&mut store, 2);
        let rows = store.take_rows();
        let markers: Vec<usize> = rows.iter().map(marker_of).collect();
        assert_eq!(markers, vec![2, 3, 4, 5], "no dead placeholder returned");
        assert!(store.is_empty());
        assert_eq!(store.dead_slots(), 0);
        assert_eq!(store.base(), RowNumber::new(6));
        assert_lockstep(&store);
    }

    #[test]
    fn from_iter_and_from_rows_at_start_without_dead_slots() {
        let store: RowStore = (0..5).map(marked_row).collect();
        assert_eq!(store.dead_slots(), 0);
        let store = RowStore::from_rows_at(RowNumber::new(9), (0..5).map(marked_row));
        assert_eq!(store.dead_slots(), 0);
        assert_eq!(store.rows.len(), 5);
    }

    #[test]
    fn clear_all_resets_the_dead_prefix() {
        let mut store = full_store(6);
        let _ = evict(&mut store, 2);
        store.clear_all();
        assert_eq!(store.dead_slots(), 0);
        assert_eq!(store.rows.len(), 0);
        assert_eq!(store.base(), RowNumber::new(6));
    }

    #[test]
    fn capacity_includes_the_dead_slots() {
        let mut store = full_store(10);
        let capacity = store.capacity();
        let cache_capacity = store.cache_capacity();
        let _ = evict(&mut store, 4);
        assert_eq!(store.capacity(), capacity, "eviction frees no allocation");
        assert_eq!(store.cache_capacity(), cache_capacity);
        assert!(store.capacity() >= store.len() + store.dead_slots());
    }

    #[test]
    fn clone_preserves_the_dead_prefix_and_the_live_view() {
        let mut store = full_store(6);
        let _ = evict(&mut store, 2);
        let copy = store.clone();
        assert_markers(&copy, &[2, 3, 4, 5]);
        assert_eq!(copy.base(), store.base());
        assert_eq!(copy.dead_slots(), store.dead_slots());
    }

    #[test]
    fn release_callback_receives_one_run_per_block() {
        let mut store = full_store(8);
        // Rows 0-2 reference block 7, rows 3-4 block 9, rows 5-7 none.
        for (i, r) in store.block_map_mut().iter_mut().enumerate() {
            *r = match i {
                0..=2 => Some(marked_ref(7)),
                3..=4 => Some(marked_ref(9)),
                _ => None,
            };
        }
        let mut releases: Vec<(BlockId, u32)> = Vec::new();
        let report = store.evict_front(6, |id, count| releases.push((id, count)));
        assert_eq!(report.rows, 6);
        assert_eq!(
            releases,
            vec![(BlockId::new(7), 3), (BlockId::new(9), 2)],
            "one release per run of consecutive rows of the same block"
        );
    }

    #[test]
    fn release_callback_covers_a_block_that_reappears_after_a_gap() {
        let mut store = full_store(6);
        for (i, r) in store.block_map_mut().iter_mut().enumerate() {
            *r = match i {
                0 | 2 => Some(marked_ref(1)),
                _ => None,
            };
        }
        let mut releases: Vec<(BlockId, u32)> = Vec::new();
        let _ = store.evict_front(4, |id, count| releases.push((id, count)));
        let total: u32 = releases.iter().map(|&(_, c)| c).sum();
        assert_eq!(total, 2, "every evicted reference is released exactly once");
        assert!(releases.iter().all(|&(id, _)| id == BlockId::new(1)));
    }

    #[test]
    fn release_callback_is_not_called_when_nothing_is_compressed() {
        let mut store: RowStore = (0..6).map(marked_row).collect();
        let mut calls = 0;
        let _ = store.evict_front(4, |_, _| calls += 1);
        assert_eq!(calls, 0);
    }

    #[test]
    fn surviving_references_are_not_released() {
        let mut store = full_store(6);
        let mut released: Vec<BlockId> = Vec::new();
        let _ = store.evict_front(2, |id, _| released.push(id));
        assert_eq!(released, vec![BlockId::new(0), BlockId::new(1)]);
        // Survivors 2..6 keep their references.
        assert!(store.block_map().iter().all(Option::is_some));
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "compressed-block reference")]
    fn pop_of_a_row_with_a_block_reference_trips_the_debug_assertion() {
        let mut store = full_store(2);
        let _ = store.pop();
    }
}
