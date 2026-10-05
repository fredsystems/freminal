// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! [`ReflowRemap`] — how a width-changing reflow moved every row (Task 125.14).
//!
//! Reflow re-wraps logical lines, so the row count and the row boundaries
//! change. Every stored [`RowNumber`] that referred to a pre-reflow row (prompt
//! marks, command-block boundaries, kitty placements)
//! has to be translated to the row that now holds the same content. Reflow
//! installs its rows at fresh numbers (`base = old.next_number()`), so a number
//! that is *not* translated falls below the new base and is detectably invalid
//! instead of silently aliasing some other row.
//!
//! The remap is built from the bookkeeping reflow already does (which logical
//! line each old row belonged to, its flat cell offset within that line, and
//! where each logical line's new rows begin); it adds no pass over the rows.
//!
//! Two anchors are provided, because the two kinds of stored row reference
//! anchor to different ends of an old row that re-wraps into several rows:
//!
//! - [`ReflowRemap::map_start`]: anchors to the row's *first* cell. Used for
//!   "start" fields (prompt / command / output start, placement origin).
//! - [`ReflowRemap::map_end`]: anchors to the row's *last* cell. Used for the
//!   inclusive end of a region (`end_row`), so a row that re-wraps into several
//!   narrower rows does not shrink the region to its first piece.

use freminal_common::buffer_states::row_number::RowNumber;

/// Where one pre-reflow row sat within its logical line.
#[derive(Debug, Clone, Copy)]
pub(in crate::buffer) struct OldRowMeta {
    /// Index of the logical line the old row belonged to.
    pub line: usize,
    /// Flat cell offset of the old row's first cell within that line.
    pub flat_start: usize,
    /// Number of cells the old row stored.
    pub cells: usize,
    /// The post-reflow row index this old row became, when reflow emitted it
    /// verbatim: an image-bearing logical line is not re-wrapped, it is carried
    /// over one new row per old row (clipped to the new width), so the mapping
    /// is exact and one-to-one. `flat_start` / `cells` count the *unclipped*
    /// old cells while the new rows hold clipped ones, so a flat-offset
    /// translation would land on the wrong row; this override bypasses it.
    /// `None` for a re-wrapped line, which is translated by flat offset.
    pub new_row: Option<usize>,
}

/// One reflow's row translation. See the module docs.
#[derive(Debug, Clone)]
struct ReflowStage {
    /// Number of the first pre-reflow row.
    old_base: RowNumber,
    /// Number of the first post-reflow row.
    new_base: RowNumber,
    /// Per pre-reflow row, in order.
    old_rows: Vec<OldRowMeta>,
    /// Per logical line: index of the first post-reflow row it produced.
    line_new_starts: Vec<usize>,
    /// Per post-reflow row: the number of cells it stores.
    new_row_cells: Vec<usize>,
}

impl ReflowStage {
    /// Index of the pre-reflow row numbered `row`, if this stage covers it.
    ///
    /// `None` for a row in another namespace, already evicted before the
    /// reflow, or past the end of the pre-reflow rows.
    fn old_index(&self, row: RowNumber) -> Option<usize> {
        // `rows_after` is `None` across namespaces.
        row.rows_after(self.old_base)
            .filter(|&index| index < self.old_rows.len())
    }

    /// Translate a flat offset within logical line `line` to the post-reflow
    /// row index whose cell span contains it.
    fn offset_to_new_row(&self, line: usize, flat_offset: usize) -> Option<usize> {
        let line_start = *self.line_new_starts.get(line)?;
        // The line's new rows span [line_start, line_end).
        let line_end = self
            .line_new_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.new_row_cells.len());

        let mut acc = 0usize;
        for new_idx in line_start..line_end {
            let cells = self.new_row_cells.get(new_idx).copied().unwrap_or(0);
            // A zero-width row (empty logical line) still anchors offset 0.
            if flat_offset < acc + cells.max(1) {
                return Some(new_idx);
            }
            acc += cells;
        }
        // Offset past the end of the line's content: clamp to the line's last
        // new row (the cursor remap uses the same fallback).
        Some(line_end.saturating_sub(1).max(line_start))
    }

    fn map_start(&self, row: RowNumber) -> Option<RowNumber> {
        let meta = self.old_rows.get(self.old_index(row)?)?;
        let new_idx = match meta.new_row {
            Some(verbatim) => verbatim,
            None => self.offset_to_new_row(meta.line, meta.flat_start)?,
        };
        Some(self.new_base.saturating_add(new_idx))
    }

    fn map_end(&self, row: RowNumber) -> Option<RowNumber> {
        let meta = self.old_rows.get(self.old_index(row)?)?;
        // A verbatim row is one row before and after: both anchors agree.
        let new_idx = if let Some(verbatim) = meta.new_row {
            verbatim
        } else {
            let last_cell = meta.flat_start + meta.cells.saturating_sub(1);
            self.offset_to_new_row(meta.line, last_cell)?
        };
        Some(self.new_base.saturating_add(new_idx))
    }

    /// `true` if `row` is in the namespace this stage renumbers.
    const fn covers_namespace(&self, row: RowNumber) -> bool {
        row.is_alternate() == self.old_base.is_alternate()
    }
}

/// The translation produced by one or more reflows, applied oldest first.
///
/// [`Buffer::take_reflow_remap`](super::Buffer::take_reflow_remap) hands the
/// accumulated remap to the one consumer that keeps row numbers outside the
/// buffer (the emulator's kitty placement table). Several reflows between two
/// takes simply chain: each stage translates the output of the previous one.
///
/// Only rows in the namespace a stage renumbers are touched: a primary-screen
/// reflow leaves alternate-screen numbers alone and vice versa.
#[derive(Debug, Clone, Default)]
pub struct ReflowRemap {
    stages: Vec<ReflowStage>,
}

impl ReflowRemap {
    /// A remap of a single reflow.
    pub(in crate::buffer) fn single(
        old_base: RowNumber,
        new_base: RowNumber,
        old_rows: Vec<OldRowMeta>,
        line_new_starts: Vec<usize>,
        new_row_cells: Vec<usize>,
    ) -> Self {
        Self {
            stages: vec![ReflowStage {
                old_base,
                new_base,
                old_rows,
                line_new_starts,
                new_row_cells,
            }],
        }
    }

    /// Append `later`'s stages, to run after this remap's.
    pub(in crate::buffer) fn chain(&mut self, later: Self) {
        self.stages.extend(later.stages);
    }

    /// Translate a "start" row reference (anchored to the first cell of the
    /// old row) to its post-reflow row number.
    ///
    /// `None` when the row is not covered by a stage that renumbers its
    /// namespace: it was already evicted, or lay past the end of the rows. A
    /// row in a namespace no stage renumbers is returned unchanged.
    #[must_use]
    pub fn map_start(&self, row: RowNumber) -> Option<RowNumber> {
        self.fold(row, ReflowStage::map_start)
    }

    /// Translate an "end" row reference (anchored to the last cell of the old
    /// row). See [`Self::map_start`].
    #[must_use]
    pub fn map_end(&self, row: RowNumber) -> Option<RowNumber> {
        self.fold(row, ReflowStage::map_end)
    }

    fn fold(
        &self,
        row: RowNumber,
        map: impl Fn(&ReflowStage, RowNumber) -> Option<RowNumber>,
    ) -> Option<RowNumber> {
        let mut current = row;
        for stage in &self.stages {
            if !stage.covers_namespace(current) {
                continue;
            }
            current = map(stage, current)?;
        }
        Some(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stage modelling three old rows, `[0]` and `[1]` forming one logical
    /// line of 4 + 4 cells that re-wraps to 3 + 3 + 2, and `[2]` its own line
    /// of 2 cells that stays one row. New rows therefore are
    /// `[3, 3, 2]` (line 0) and `[2]` (line 1).
    fn stage() -> ReflowRemap {
        ReflowRemap::single(
            RowNumber::new(100),
            RowNumber::new(200),
            vec![
                OldRowMeta {
                    line: 0,
                    flat_start: 0,
                    cells: 4,
                    new_row: None,
                },
                OldRowMeta {
                    line: 0,
                    flat_start: 4,
                    cells: 4,
                    new_row: None,
                },
                OldRowMeta {
                    line: 1,
                    flat_start: 0,
                    cells: 2,
                    new_row: None,
                },
            ],
            vec![0, 3],
            vec![3, 3, 2, 2],
        )
    }

    #[test]
    fn default_remap_maps_every_row_to_itself() {
        let remap = ReflowRemap::default();
        assert_eq!(remap.map_start(RowNumber::new(5)), Some(RowNumber::new(5)));
        assert_eq!(remap.map_end(RowNumber::new(5)), Some(RowNumber::new(5)));
    }

    #[test]
    fn start_anchors_to_the_first_cell_of_the_old_row() {
        let remap = stage();
        // Old row 0 starts at flat offset 0 -> new row 0.
        assert_eq!(
            remap.map_start(RowNumber::new(100)),
            Some(RowNumber::new(200))
        );
        // Old row 1 starts at flat offset 4 -> new row 1 (cells 3..6).
        assert_eq!(
            remap.map_start(RowNumber::new(101)),
            Some(RowNumber::new(201))
        );
        // Old row 2 is its own line -> new row 3.
        assert_eq!(
            remap.map_start(RowNumber::new(102)),
            Some(RowNumber::new(203))
        );
    }

    #[test]
    fn end_anchors_to_the_last_cell_of_the_old_row() {
        let remap = stage();
        // Old row 0's last cell is flat offset 3 -> new row 1 (cells 3..6).
        assert_eq!(
            remap.map_end(RowNumber::new(100)),
            Some(RowNumber::new(201))
        );
        // Old row 1's last cell is flat offset 7 -> new row 2 (cells 6..8).
        assert_eq!(
            remap.map_end(RowNumber::new(101)),
            Some(RowNumber::new(202))
        );
        assert_eq!(
            remap.map_end(RowNumber::new(102)),
            Some(RowNumber::new(203))
        );
    }

    #[test]
    fn rows_outside_the_old_range_are_unmappable() {
        let remap = stage();
        // Below the old base (already evicted) and past the old end.
        assert_eq!(remap.map_start(RowNumber::new(99)), None);
        assert_eq!(remap.map_start(RowNumber::new(103)), None);
        assert_eq!(remap.map_end(RowNumber::new(99)), None);
        assert_eq!(remap.map_end(RowNumber::new(103)), None);
    }

    #[test]
    fn other_namespace_is_untouched() {
        let remap = stage();
        let alt = RowNumber::ALTERNATE_BASE.saturating_add(7);
        assert_eq!(remap.map_start(alt), Some(alt));
        assert_eq!(remap.map_end(alt), Some(alt));
    }

    #[test]
    fn stages_chain_oldest_first() {
        let mut remap = stage();
        // Second reflow: new rows 200..204 -> everything collapses to one row
        // per old row, renumbered from 300.
        let second = ReflowRemap::single(
            RowNumber::new(200),
            RowNumber::new(300),
            (0..4)
                .map(|i| OldRowMeta {
                    line: i,
                    flat_start: 0,
                    cells: 1,
                    new_row: None,
                })
                .collect(),
            vec![0, 1, 2, 3],
            vec![1, 1, 1, 1],
        );
        remap.chain(second);
        // 100 -> 200 (first stage) -> 300 (second stage).
        assert_eq!(
            remap.map_start(RowNumber::new(100)),
            Some(RowNumber::new(300))
        );
        // 102 -> 203 -> 303.
        assert_eq!(
            remap.map_start(RowNumber::new(102)),
            Some(RowNumber::new(303))
        );
    }

    #[test]
    fn offset_past_line_content_clamps_to_the_last_row_of_the_line() {
        // Old row claims 10 cells but the line only produced 2 new rows of
        // 1 cell: the end anchor lands past the content and clamps.
        let remap = ReflowRemap::single(
            RowNumber::new(0),
            RowNumber::new(50),
            vec![OldRowMeta {
                line: 0,
                flat_start: 0,
                cells: 10,
                new_row: None,
            }],
            vec![0],
            vec![1, 1],
        );
        assert_eq!(remap.map_end(RowNumber::new(0)), Some(RowNumber::new(51)));
    }

    #[test]
    fn empty_row_anchors_to_offset_zero() {
        let remap = ReflowRemap::single(
            RowNumber::new(0),
            RowNumber::new(10),
            vec![OldRowMeta {
                line: 0,
                flat_start: 0,
                cells: 0,
                new_row: None,
            }],
            vec![0],
            vec![0],
        );
        assert_eq!(remap.map_start(RowNumber::new(0)), Some(RowNumber::new(10)));
        assert_eq!(remap.map_end(RowNumber::new(0)), Some(RowNumber::new(10)));
    }

    #[test]
    fn verbatim_rows_map_one_to_one_despite_clipped_cell_counts() {
        // One soft-wrapped logical line of three 10-cell old rows, emitted
        // verbatim and clipped to 3 cells each. A flat-offset translation
        // would see old starts 0/10/20 against new cells 3/3/3 and map the
        // later rows past the line's content (clamping them all to the last
        // row); the explicit override keeps them one-to-one.
        let meta = |flat_start: usize, new_row: usize| OldRowMeta {
            line: 0,
            flat_start,
            cells: 10,
            new_row: Some(new_row),
        };
        let remap = ReflowRemap::single(
            RowNumber::new(0),
            RowNumber::new(50),
            vec![meta(0, 0), meta(10, 1), meta(20, 2)],
            vec![0],
            vec![3, 3, 3],
        );
        for (old, new) in [(0, 50), (1, 51), (2, 52)] {
            assert_eq!(
                remap.map_start(RowNumber::new(old)),
                Some(RowNumber::new(new))
            );
            assert_eq!(
                remap.map_end(RowNumber::new(old)),
                Some(RowNumber::new(new))
            );
        }
    }
}
