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
//! Storage is currently three plain `Vec`s and eviction from the front drains
//! all three (O(retained rows)). The public surface is deliberately shaped so
//! a later change to the eviction mechanism changes this file's internals
//! only.

use std::ops::{Deref, DerefMut};

use crate::row::Row;

use super::{BlockRowRef, RowCacheEntry};

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
#[derive(Debug, Clone, Default)]
pub(in crate::buffer) struct RowStore {
    rows: Vec<Row>,
    cache: Vec<Option<RowCacheEntry>>,
    blocks: Vec<Option<BlockRowRef>>,
}

impl RowStore {
    // ----------------------------------------------------------------
    // Read access
    // ----------------------------------------------------------------

    /// All rows as a slice. `const`, unlike going through `Deref`.
    pub(in crate::buffer) const fn as_slice(&self) -> &[Row] {
        self.rows.as_slice()
    }

    /// Number of rows. Also available through `Deref`, but that path is not
    /// `const`, and `const fn` buffer methods need this.
    pub(in crate::buffer) const fn len(&self) -> usize {
        self.rows.len()
    }

    /// `true` if there are no rows. See [`Self::len`] for why this is inherent.
    pub(in crate::buffer) const fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Capacity of the row allocation, in rows. Used by memory accounting.
    pub(in crate::buffer) const fn capacity(&self) -> usize {
        self.rows.capacity()
    }

    /// Capacity of the flatten-cache allocation, in entries. Used by memory
    /// accounting, which reports the cache allocation separately from rows.
    pub(in crate::buffer) const fn cache_capacity(&self) -> usize {
        self.cache.capacity()
    }

    /// Per-row flatten-cache entries, index-parallel to the rows.
    pub(in crate::buffer) const fn cache(&self) -> &[Option<RowCacheEntry>] {
        self.cache.as_slice()
    }

    /// Mutable per-row flatten-cache entries, index-parallel to the rows.
    pub(in crate::buffer) const fn cache_mut(&mut self) -> &mut [Option<RowCacheEntry>] {
        self.cache.as_mut_slice()
    }

    /// Per-row compressed-block references, index-parallel to the rows.
    pub(in crate::buffer) const fn block_map(&self) -> &[Option<BlockRowRef>] {
        self.blocks.as_slice()
    }

    /// Mutable per-row compressed-block references, index-parallel to the rows.
    pub(in crate::buffer) const fn block_map_mut(&mut self) -> &mut [Option<BlockRowRef>] {
        self.blocks.as_mut_slice()
    }

    /// Borrow rows, cache entries and block references mutably *at once*.
    ///
    /// The three slices are disjoint (they are different allocations) and all
    /// have the same length. This exists for the sites that must hold a
    /// mutable window over the rows and the matching window over the cache
    /// simultaneously, which separate `&mut self` accessors cannot express.
    pub(in crate::buffer) const fn split_mut(
        &mut self,
    ) -> (
        &mut [Row],
        &mut [Option<RowCacheEntry>],
        &mut [Option<BlockRowRef>],
    ) {
        (
            self.rows.as_mut_slice(),
            self.cache.as_mut_slice(),
            self.blocks.as_mut_slice(),
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
        if let Some(entry) = self.cache.get_mut(index) {
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
    pub(in crate::buffer) fn pop(&mut self) -> Option<Row> {
        let row = self.rows.pop()?;
        self.cache.pop();
        self.blocks.pop();
        Some(row)
    }

    /// Remove the first `n` rows (clamped to the number stored) along with
    /// their cache entries and block references.
    ///
    /// `BlockRowRef::offset_in_block` is block-relative, so removing the front
    /// of the block map needs no remapping of the surviving references. The
    /// caller remains responsible for any bookkeeping keyed on the removed
    /// rows (image cell counts, compressed-block reclamation, prompt rows).
    pub(in crate::buffer) fn evict_front(&mut self, n: usize) -> EvictionReport {
        let n = n.min(self.rows.len());
        self.rows.drain(..n);
        self.cache.drain(..n);
        self.blocks.drain(..n);
        EvictionReport { rows: n }
    }

    /// Replace every row with `rows`, resetting the cache and block map to
    /// match: every entry `None`.
    pub(in crate::buffer) fn replace_all(&mut self, rows: Vec<Row>) {
        *self = rows.into_iter().collect();
    }

    /// Remove and return every row, leaving the store empty (all three tables
    /// empty, so still in lockstep). The cache entries and block references
    /// are discarded with the store's old contents.
    pub(in crate::buffer) fn take_rows(&mut self) -> Vec<Row> {
        std::mem::take(self).rows
    }

    /// Remove every row, cache entry and block reference.
    #[cfg(test)]
    pub(in crate::buffer) fn clear_all(&mut self) {
        self.rows.clear();
        self.cache.clear();
        self.blocks.clear();
    }
}

impl Deref for RowStore {
    type Target = [Row];

    fn deref(&self) -> &[Row] {
        &self.rows
    }
}

impl DerefMut for RowStore {
    /// Mutable access to row *content*. The slice cannot change the number of
    /// rows, so the side tables stay parallel. Reordering through the slice
    /// (`swap`, `rotate_*`, ...) would **not** move the side tables; callers
    /// that relocate rows are responsible for relocating the matching cache
    /// entries themselves, exactly as before this type existed.
    fn deref_mut(&mut self) -> &mut [Row] {
        &mut self.rows
    }
}

impl<'a> IntoIterator for &'a RowStore {
    type Item = &'a Row;
    type IntoIter = std::slice::Iter<'a, Row>;

    fn into_iter(self) -> Self::IntoIter {
        self.rows.iter()
    }
}

impl<'a> IntoIterator for &'a mut RowStore {
    type Item = &'a mut Row;
    type IntoIter = std::slice::IterMut<'a, Row>;

    fn into_iter(self) -> Self::IntoIter {
        self.rows.iter_mut()
    }
}

impl FromIterator<Row> for RowStore {
    /// Build a store from rows, giving each an empty cache entry and no block
    /// reference.
    fn from_iter<I: IntoIterator<Item = Row>>(iter: I) -> Self {
        let rows: Vec<Row> = iter.into_iter().collect();
        let len = rows.len();
        Self {
            rows,
            cache: vec![None; len],
            blocks: vec![None; len],
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
        let popped = store.pop().unwrap();
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
        let report = store.evict_front(2);
        assert_eq!(report, EvictionReport { rows: 2 });
        assert_markers(&store, &[2, 3, 4, 5]);
    }

    #[test]
    fn evict_front_zero_is_a_no_op() {
        let mut store = full_store(3);
        let report = store.evict_front(0);
        assert_eq!(report.rows, 0);
        assert_markers(&store, &[0, 1, 2]);
    }

    #[test]
    fn evict_front_clamps_to_len_and_reports_actual() {
        let mut store = full_store(3);
        let report = store.evict_front(10);
        assert_eq!(report.rows, 3);
        assert!(store.is_empty());
        assert_lockstep(&store);
    }

    #[test]
    fn evict_front_all_rows_exactly() {
        let mut store = full_store(3);
        let report = store.evict_front(3);
        assert_eq!(report.rows, 3);
        assert!(store.is_empty());
        assert_lockstep(&store);
    }

    #[test]
    fn evict_front_then_push_keeps_lockstep() {
        let mut store = full_store(4);
        let _ = store.evict_front(1);
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
        let _ = store.evict_front(2);
        store.push(marked_row(40));
        store.push(marked_row(41));
        let _ = store.pop();
        let _ = store.evict_front(1);
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
}
