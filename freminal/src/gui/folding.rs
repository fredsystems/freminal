// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Command-block folding helpers (OSC 133, Task 72.10b).
//!
//! This module is a pure helper layer: it turns the
//! `(command_blocks, folded_blocks)` pair carried on a terminal snapshot
//! plus the GUI-local view state into:
//!
//! 1. A sorted, non-overlapping list of [`FoldRange`] values describing
//!    which snapshot rows should be collapsed.
//! 2. A [`RowMap`] that maps between **snapshot-row** space (the row indices
//!    the buffer/terminal think in) and **rendered-row** space (the row
//!    indices the widget actually paints, where each folded range has been
//!    collapsed to a single placeholder row).
//!
//! Both data structures are immutable and trivially testable in isolation.
//! Widget integration (the actual collapse-and-render of folded rows and
//! placeholder line) lands in subtask 72.10b-2.
//!
//! ## Invariants
//!
//! - A [`FoldRange`] is only emitted for command blocks where `end_row`
//!   is `Some` (i.e. completed) AND at least one of `output_start_row`
//!   (preferred) or `command_start_row` is `Some` AND whose
//!   `CommandBlockId` is currently in `folded_blocks`.
//! - The fold range starts at `output_start_row` when present so the
//!   prompt and command line stay visible above the placeholder.
//!   Shells that don't emit OSC 133 C fall back to `command_start_row`
//!   (the pre-fix behaviour: command line is folded with the output).
//! - Running blocks (`end_row.is_none()`) cannot be folded, matching the
//!   spec in `PLAN_VERSION_090.md` §72.10.
//! - Degenerate ranges where `start > end_row` are dropped.
//!   They should never occur in well-formed OSC 133 streams; we treat them
//!   as data corruption and silently ignore them rather than panicking.
//! - The returned `Vec<FoldRange>` is sorted ascending by `start_row` and
//!   contains no overlapping entries.
//!
//! ## Overlap policy
//!
//! If two folded blocks somehow describe overlapping snapshot-row ranges
//! (which OSC 133 markers should never produce — `B`/`D` pairs nest only
//! through the `fid` correlation, not row arithmetic), the **later** range
//! (by start row) is dropped. This preserves the earlier, presumably
//! outer, fold and avoids the alternative of merging ranges (which would
//! create a `FoldRange` whose `command_block_id` no longer corresponds to
//! a single block — confusing for placeholder rendering and click-to-
//! unfold). The policy is intentionally pessimistic: corrupt input yields
//! a strict subset of the requested folds, never more.

use std::collections::HashSet;
use std::hash::BuildHasher;

use freminal_common::buffer_states::command_block::{CommandBlock, CommandBlockId};

/// A contiguous range of snapshot rows that should be collapsed into a
/// single placeholder row in the rendered view.
///
/// Both `start_row` and `end_row` are **inclusive** indices in
/// snapshot-row space. `start_row` is the block's `output_start_row`
/// (the row where output begins, immediately after the command line);
/// `end_row` is the block's `end_row` (the row of the `OSC 133 D`
/// marker). The prompt and command line are **not** part of the fold —
/// folding preserves them so the user can still see what was run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldRange {
    /// Stable id of the command block this fold belongs to.
    pub command_block_id: CommandBlockId,
    /// First folded snapshot row, inclusive.
    pub start_row: usize,
    /// Last folded snapshot row, inclusive.
    pub end_row: usize,
    /// Total number of rows in the underlying command block, **before**
    /// any clipping for partial visibility or scrollback eviction.
    ///
    /// `start_row` / `end_row` describe the *visible* rows currently
    /// collapsed (which may be a subset of the block when scrolling has
    /// brought only part of it on-screen). This field is preserved
    /// unchanged through [`translate_ranges_to_snapshot`] and
    /// [`RowMap::new`] so the renderer can display a stable
    /// "*N* lines hidden" placeholder regardless of scroll position.
    pub block_total_rows: usize,
}

impl FoldRange {
    /// Number of snapshot rows this range hides.
    #[must_use]
    pub const fn len(&self) -> usize {
        // start_row <= end_row is an invariant of construction in
        // `compute_fold_ranges`. We saturate defensively.
        self.end_row.saturating_sub(self.start_row) + 1
    }

    /// `true` if the range is empty. Never emitted by
    /// [`compute_fold_ranges`]; included for `clippy::len_without_is_empty`.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        // `len()` is always >= 1 by construction.
        false
    }

    /// `true` if `snap_row` falls inside this range (inclusive).
    #[must_use]
    pub const fn contains(&self, snap_row: usize) -> bool {
        snap_row >= self.start_row && snap_row <= self.end_row
    }
}

/// Compute the set of fold ranges for a frame from the snapshot's
/// `command_blocks` list and the view-local `folded_blocks` set.
///
/// A block contributes a [`FoldRange`] iff:
///
/// - its `CommandBlockId` is present in `folded_blocks`, AND
/// - `end_row` is `Some`, AND
/// - at least one of `output_start_row` (preferred) or
///   `command_start_row` is `Some`, AND
/// - the resolved start row `<= end_row`.
///
/// IDs in `folded_blocks` that do not correspond to any block in
/// `command_blocks` (e.g. because the block has scrolled out of
/// scrollback) are silently ignored — the [`super::view_state::ViewState`]
/// keeps stale ids in its set harmlessly until a fold/unfold action
/// prunes them.
///
/// The returned `Vec` is sorted ascending by `start_row` and contains no
/// overlapping ranges. See the module-level documentation for the
/// overlap policy (the later range is dropped).
#[must_use]
pub fn compute_fold_ranges<S: BuildHasher>(
    command_blocks: &[CommandBlock],
    folded_blocks: &HashSet<CommandBlockId, S>,
) -> Vec<FoldRange> {
    if folded_blocks.is_empty() || command_blocks.is_empty() {
        return Vec::new();
    }

    let mut ranges: Vec<FoldRange> = command_blocks
        .iter()
        .filter(|b| folded_blocks.contains(&b.id))
        .filter_map(|b| {
            // Fold the *output* of a block, leaving the prompt and command
            // line visible. Prefer `output_start_row` (OSC 133 C) so the
            // collapsed range starts on the first output row. Fall back to
            // `command_start_row` for shell integrations that don't emit
            // OSC 133 C; in that case the command line is folded with the
            // output (the pre-fix behaviour) rather than refusing to fold.
            let start = b.output_start_row.or(b.command_start_row)?;
            let end = b.end_row?;
            if start > end {
                return None;
            }
            Some(FoldRange {
                command_block_id: b.id,
                start_row: start,
                end_row: end,
                block_total_rows: end.saturating_sub(start).saturating_add(1),
            })
        })
        .collect();

    ranges.sort_by_key(|r| r.start_row);

    // Drop any range that overlaps the previous (kept) range. With the
    // ranges sorted by `start_row`, overlap iff `cur.start_row <=
    // prev.end_row`. We keep the earlier one (which is structurally the
    // "outer" fold under any reasonable interpretation of OSC 133 input)
    // and discard the later overlapping fold.
    let mut deduped: Vec<FoldRange> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match deduped.last() {
            Some(prev) if r.start_row <= prev.end_row => {
                // Overlap with previous kept range — drop `r`.
            }
            _ => deduped.push(r),
        }
    }
    deduped
}

/// Compute how many extra rows to flatten **above** the visible window.
///
/// After collapsing every fold that overlaps the window, the extra rows let a
/// full screen of rendered rows still be painted with the live bottom pinned.
///
/// `ranges` are the **buffer-absolute** fold ranges (as produced by
/// [`compute_fold_ranges`]). `visible_window_start` is the buffer-absolute
/// index of the topmost normally-visible row, and `term_height` the visible
/// window height in rows.
///
/// For each fold overlapping the window `[visible_window_start,
/// visible_window_start + term_height)`, collapsing it frees `overlap_len - 1`
/// rows of screen space (the fold's visible rows become a single placeholder).
/// The renderer must pull in that many real rows from above the window to keep
/// the screen full. The total is the sum across all overlapping folds, capped
/// by the rows actually available above the window (`visible_window_start`).
///
/// Returns `0` when no fold overlaps the window. The result is a stable
/// function of `visible_window_start` (it does not depend on the extra-row
/// count itself), so feeding it back through the scroll request does not
/// oscillate.
#[must_use]
pub fn compute_extra_rows(
    ranges: &[FoldRange],
    visible_window_start: usize,
    term_height: usize,
) -> usize {
    if term_height == 0 {
        return 0;
    }
    let win_end = visible_window_start.saturating_add(term_height); // exclusive
    let mut freed: usize = 0;
    for r in ranges {
        // Overlap of [start_row, end_row] (inclusive) with
        // [visible_window_start, win_end) (half-open).
        let ov_start = r.start_row.max(visible_window_start);
        let ov_end_incl = r.end_row.min(win_end.saturating_sub(1));
        if ov_start > ov_end_incl {
            continue; // no overlap with the visible window
        }
        let overlap_len = ov_end_incl - ov_start + 1;
        // Collapsing an overlapping fold frees `overlap_len - 1` rows.
        freed = freed.saturating_add(overlap_len.saturating_sub(1));
    }
    // Cannot pull in more rows than exist above the window.
    freed.min(visible_window_start)
}

/// Direction of a scroll step in [`apply_rendered_scroll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDir {
    /// Scroll up, into history (increases the raw scroll offset).
    Up,
    /// Scroll down, toward the live bottom (decreases the raw scroll offset).
    Down,
}

/// Convert a rendered-row scroll request into a new raw `scroll_offset`.
///
/// Steps are measured in **rendered rows** (visible lines); rows hidden inside
/// collapsed folds are skipped so each step moves the view by exactly one
/// visible line.
///
/// Without this, the raw scroll offset steps one buffer row at a time; while
/// traversing a collapsed fold's hidden rows the visible content does not move
/// (the rows are not painted), so scrolling feels stuck and the live bottom
/// fails to track the input. With it, one rendered step that lands on a
/// collapsed fold consumes the whole hidden span in a single move.
///
/// Parameters:
/// - `ranges`: buffer-absolute fold ranges (from [`compute_fold_ranges`]).
/// - `total_rows`, `term_height`: buffer / window geometry.
/// - `max_scroll_offset`: clamp bound (raw rows of scrollback).
/// - `raw_offset`: the current raw scroll offset.
/// - `dir`, `steps`: scroll direction and the number of *rendered* rows.
///
/// Returns the new raw `scroll_offset`, clamped to `[0, max_scroll_offset]`.
/// When `ranges` is empty this is exactly `raw_offset ± steps` (1:1 with the
/// unfolded behaviour).
#[must_use]
pub fn apply_rendered_scroll(
    ranges: &[FoldRange],
    total_rows: usize,
    term_height: usize,
    max_scroll_offset: usize,
    raw_offset: usize,
    dir: ScrollDir,
    steps: usize,
) -> usize {
    if ranges.is_empty() || term_height == 0 {
        // No folds: rendered rows == raw rows.
        return match dir {
            ScrollDir::Up => raw_offset.saturating_add(steps).min(max_scroll_offset),
            ScrollDir::Down => raw_offset.saturating_sub(steps),
        };
    }

    // The first normally-visible buffer row at a given raw offset.
    let top_of =
        |raw: usize| -> usize { total_rows.saturating_sub(term_height).saturating_sub(raw) };
    // Is buffer row `row` strictly inside a collapsed fold (i.e. hidden, not
    // the placeholder anchor)? The placeholder occupies the fold's first row.
    let hidden_in_fold = |row: usize| -> Option<&FoldRange> {
        ranges
            .iter()
            .find(|r| row > r.start_row && row <= r.end_row)
    };

    let mut raw = raw_offset;
    match dir {
        ScrollDir::Up => {
            for _ in 0..steps {
                if raw >= max_scroll_offset {
                    break;
                }
                let top = top_of(raw);
                if top == 0 {
                    break;
                }
                let revealed = top - 1; // buffer row revealed by one raw step up
                // If the revealed row is hidden inside a fold, jump up to the
                // fold's first (placeholder) row so the single placeholder is
                // what appears, consuming the whole hidden span in one step.
                // Otherwise reveal exactly one more row (raw + 1).
                let next_raw = hidden_in_fold(revealed)
                    .map_or(raw + 1, |fold| raw + (revealed - fold.start_row) + 1);
                raw = next_raw.min(max_scroll_offset);
            }
        }
        ScrollDir::Down => {
            for _ in 0..steps {
                if raw == 0 {
                    break;
                }
                let top = top_of(raw);
                // Scrolling down hides the current top row. If the row that
                // would become the new top (`top + 1` after one raw step) is
                // hidden inside a fold, jump down past the whole hidden span so
                // one rendered row disappears, not a hidden one.
                let new_top = top + 1;
                // If the row that would become the new top is hidden inside a
                // fold, jump down past the whole hidden span so one rendered
                // row disappears, not a hidden one. Otherwise hide one row.
                let next_raw = hidden_in_fold(new_top).map_or(raw - 1, |fold| {
                    let target_top = fold.end_row + 1;
                    raw.saturating_sub(target_top - top)
                });
                raw = next_raw;
            }
        }
    }
    raw.min(max_scroll_offset)
}

/// Translate a slice of [`FoldRange`] values from **buffer-absolute** row
/// space into **snapshot-row** space (the row space [`RowMap`] consumes).
///
/// `CommandBlock` rows are stored in buffer-absolute coordinates (the
/// total scrollback-buffer row indices), while the renderer's snapshot
/// row space is `[0, term_height)` indexed from the top of the *visible*
/// window. The two spaces differ by `visible_window_start =
/// total_rows.saturating_sub(term_height).saturating_sub(scroll_offset)`.
///
/// Behaviour:
///
/// - Ranges entirely in scrollback (`end_row < visible_window_start`)
///   are dropped — they have no visible placeholder. The fold persists
///   in `view_state.folded_blocks` and will re-emerge when the user
///   scrolls back into them.
/// - Ranges whose `start_row < visible_window_start` (block straddles
///   scrollback / visible boundary) are clamped to start at snapshot
///   row 0 via saturating subtraction.
/// - `end_row` is translated unchanged (saturating sub); [`RowMap::new`]
///   subsequently clamps it to `snapshot_row_count - 1`.
///
/// Input is expected to be sorted and non-overlapping (as produced by
/// [`compute_fold_ranges`]); the output preserves both invariants.
#[must_use]
pub fn translate_ranges_to_snapshot(
    ranges: &[FoldRange],
    visible_window_start: usize,
) -> Vec<FoldRange> {
    ranges
        .iter()
        .filter_map(|r| {
            if r.end_row < visible_window_start {
                return None;
            }
            Some(FoldRange {
                command_block_id: r.command_block_id,
                start_row: r.start_row.saturating_sub(visible_window_start),
                end_row: r.end_row.saturating_sub(visible_window_start),
                block_total_rows: r.block_total_rows,
            })
        })
        .collect()
}

/// A single row in rendered-row space, as produced by
/// [`RowMap::rendered_to_snapshot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderedRow {
    /// Rendered row corresponds 1:1 to this snapshot row.
    Snapshot(usize),
    /// Rendered row is the placeholder line for a folded range.
    Placeholder(FoldRange),
}

/// Bidirectional mapping between snapshot-row space and rendered-row space.
///
/// Construct via [`RowMap::new`] with a snapshot row count and the
/// (sorted, non-overlapping) ranges from [`compute_fold_ranges`].
///
/// The mapping is built lazily-by-walk: `RowMap` itself just stores the
/// inputs plus a precomputed `rendered_row_count`. Both translation
/// methods perform a linear walk over the (typically small) ranges
/// list. There is no per-row table; this keeps construction O(R) where
/// R is the number of folded ranges, regardless of the buffer size.
#[derive(Debug, Clone)]
pub struct RowMap {
    snapshot_row_count: usize,
    /// Ranges sorted ascending by `start_row`, non-overlapping. Owned
    /// rather than borrowed so the map is usable independently of the
    /// snapshot lifetime.
    ranges: Vec<FoldRange>,
    rendered_row_count: usize,
}

impl RowMap {
    /// Build a [`RowMap`] from `snapshot_row_count` and the fold `ranges`.
    ///
    /// `ranges` is expected to be the output of [`compute_fold_ranges`]:
    /// sorted ascending by `start_row` and non-overlapping. Ranges whose
    /// `start_row >= snapshot_row_count` are dropped (they would describe
    /// folds against rows the snapshot doesn't have); ranges whose
    /// `end_row >= snapshot_row_count` are clamped to the last snapshot
    /// row. Both situations are defensive: callers should not normally
    /// produce them, but the snapshot row count can shrink due to
    /// scrollback eviction between fold registration and the next frame.
    #[must_use]
    pub fn new(snapshot_row_count: usize, ranges: &[FoldRange]) -> Self {
        let mut clamped: Vec<FoldRange> = Vec::with_capacity(ranges.len());
        let mut folded_hidden_rows: usize = 0;
        let mut placeholder_rows: usize = 0;

        for r in ranges {
            if r.start_row >= snapshot_row_count {
                // Entire range is beyond the snapshot — skip.
                continue;
            }
            let end = r.end_row.min(snapshot_row_count.saturating_sub(1));
            if r.start_row > end {
                // Should not happen given the input invariant, but stay safe.
                continue;
            }
            let clamped_range = FoldRange {
                command_block_id: r.command_block_id,
                start_row: r.start_row,
                end_row: end,
                block_total_rows: r.block_total_rows,
            };
            folded_hidden_rows = folded_hidden_rows.saturating_add(clamped_range.len());
            placeholder_rows = placeholder_rows.saturating_add(1);
            clamped.push(clamped_range);
        }

        // Each folded range contributes `len` hidden rows but 1 placeholder,
        // so rendered = snapshot - hidden + placeholders.
        let rendered_row_count = snapshot_row_count
            .saturating_sub(folded_hidden_rows)
            .saturating_add(placeholder_rows);

        Self {
            snapshot_row_count,
            ranges: clamped,
            rendered_row_count,
        }
    }

    /// Total number of rows in rendered-row space.
    ///
    /// Equals `snapshot_row_count - sum(range.len()) + ranges.len()`.
    /// When there are no folds, equals the snapshot row count exactly.
    #[must_use]
    pub const fn rendered_row_count(&self) -> usize {
        self.rendered_row_count
    }

    /// The snapshot row count this map was built against.
    #[must_use]
    pub const fn snapshot_row_count(&self) -> usize {
        self.snapshot_row_count
    }

    /// The sorted, non-overlapping fold ranges this map was built from.
    #[must_use]
    pub fn ranges(&self) -> &[FoldRange] {
        &self.ranges
    }

    /// Map a snapshot row to its rendered-row index.
    ///
    /// Returns `None` if `snap_row` lies strictly inside a folded range
    /// (i.e. is hidden by the fold and is not the placeholder row
    /// itself). Returns `None` if `snap_row >= snapshot_row_count`.
    #[must_use]
    pub fn snapshot_to_rendered(&self, snap_row: usize) -> Option<usize> {
        if snap_row >= self.snapshot_row_count {
            return None;
        }

        // Walk ranges in order. Snapshot rows before `range.start_row`
        // map 1:1 (with all previously-collapsed ranges accounted for).
        // Rows inside a range are hidden (`None`). Rows after a range
        // shift down by `range.len() - 1` (we lose `len` rows and gain
        // 1 placeholder).
        let mut rendered = snap_row;
        for range in &self.ranges {
            if snap_row < range.start_row {
                // No further range can affect us.
                break;
            }
            if range.contains(snap_row) {
                return None;
            }
            // snap_row > range.end_row — collapse this range.
            // `range.len()` is >= 1; subtract len rows, add 1 placeholder.
            rendered = rendered.saturating_sub(range.len()).saturating_add(1);
        }
        Some(rendered)
    }

    /// Map a rendered row to either a snapshot row or a placeholder.
    ///
    /// Returns `None` if `rendered_row >= rendered_row_count()`.
    ///
    /// The placeholder row for a fold is positioned at the same rendered
    /// index as the fold's `start_row` would have been (i.e. immediately
    /// after the prompt line). Subsequent ranges then shift accordingly.
    #[must_use]
    pub fn rendered_to_snapshot(&self, rendered_row: usize) -> Option<RenderedRow> {
        if rendered_row >= self.rendered_row_count {
            return None;
        }

        // Walk ranges and translate rendered → snapshot incrementally.
        // `rendered_cursor` tracks the rendered index at which the next
        // range's *placeholder* would appear; `snap_cursor` tracks the
        // corresponding snapshot row. Rendered rows in the gap
        // `[rendered_cursor_prev, rendered_cursor_for_this_range)` map
        // 1:1 to snapshot rows in the gap before this range's start.
        let mut snap_cursor: usize = 0;
        let mut rendered_cursor: usize = 0;

        for range in &self.ranges {
            // Gap before this range: rendered rows
            //   [rendered_cursor .. rendered_cursor + (range.start_row - snap_cursor))
            // map to snapshot rows [snap_cursor .. range.start_row).
            let gap_len = range.start_row.saturating_sub(snap_cursor);
            let placeholder_rendered = rendered_cursor.saturating_add(gap_len);

            if rendered_row < placeholder_rendered {
                let offset = rendered_row.saturating_sub(rendered_cursor);
                return Some(RenderedRow::Snapshot(snap_cursor.saturating_add(offset)));
            }
            if rendered_row == placeholder_rendered {
                return Some(RenderedRow::Placeholder(*range));
            }

            // Advance past this range.
            snap_cursor = range.end_row.saturating_add(1);
            rendered_cursor = placeholder_rendered.saturating_add(1);
        }

        // After the last range, the tail maps 1:1.
        let offset = rendered_row.saturating_sub(rendered_cursor);
        Some(RenderedRow::Snapshot(snap_cursor.saturating_add(offset)))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::command_block::CommandBlock;

    fn make_block(
        id: u64,
        prompt: usize,
        cmd_start: Option<usize>,
        end: Option<usize>,
    ) -> CommandBlock {
        let mut b = CommandBlock::new_running(prompt, None, String::new());
        b.id = CommandBlockId(id);
        b.command_start_row = cmd_start;
        b.end_row = end;
        b
    }

    /// Builder used by tests that exercise `output_start_row` precedence.
    fn make_block_with_output(
        id: u64,
        prompt: usize,
        cmd_start: Option<usize>,
        output_start: Option<usize>,
        end: Option<usize>,
    ) -> CommandBlock {
        let mut b = make_block(id, prompt, cmd_start, end);
        b.output_start_row = output_start;
        b
    }

    fn set(ids: &[u64]) -> HashSet<CommandBlockId> {
        ids.iter().copied().map(CommandBlockId).collect()
    }

    // ── compute_fold_ranges ──────────────────────────────────────────────

    #[test]
    fn compute_no_folds_when_set_empty() {
        let blocks = [make_block(1, 0, Some(1), Some(5))];
        let folded = HashSet::new();
        assert!(compute_fold_ranges(&blocks, &folded).is_empty());
    }

    #[test]
    fn compute_no_folds_when_blocks_empty() {
        let folded = set(&[1, 2, 3]);
        assert!(compute_fold_ranges(&[], &folded).is_empty());
    }

    #[test]
    fn compute_skips_running_blocks() {
        let blocks = [
            make_block(1, 0, Some(1), None), // running — end_row is None
        ];
        let folded = set(&[1]);
        assert!(
            compute_fold_ranges(&blocks, &folded).is_empty(),
            "running blocks must never be folded"
        );
    }

    #[test]
    fn compute_skips_blocks_without_command_start_row() {
        let blocks = [
            // Prompt sent but B never received; command_start_row is None.
            make_block(1, 0, None, Some(5)),
        ];
        let folded = set(&[1]);
        assert!(
            compute_fold_ranges(&blocks, &folded).is_empty(),
            "blocks without command_start_row must be skipped"
        );
    }

    #[test]
    fn compute_skips_unknown_ids() {
        // Folded set references id 99, which is not in the blocks list
        // (e.g. block has scrolled out of scrollback).
        let blocks = [make_block(1, 0, Some(1), Some(3))];
        let folded = set(&[99]);
        assert!(compute_fold_ranges(&blocks, &folded).is_empty());
    }

    #[test]
    fn compute_emits_range_for_completed_folded_block() {
        let blocks = [make_block(1, 0, Some(1), Some(5))];
        let folded = set(&[1]);
        let ranges = compute_fold_ranges(&blocks, &folded);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].command_block_id, CommandBlockId(1));
        assert_eq!(ranges[0].start_row, 1);
        assert_eq!(ranges[0].end_row, 5);
    }

    #[test]
    fn compute_prefers_output_start_row_over_command_start_row() {
        // A typical block: prompt + command on row 1 (command_start_row=1),
        // output starts on row 2 (output_start_row=2), ends on row 5.
        // Folding should hide rows 2..=5, leaving the prompt+command on
        // row 1 visible above the placeholder.
        let blocks = [make_block_with_output(1, 1, Some(1), Some(2), Some(5))];
        let folded = set(&[1]);
        let ranges = compute_fold_ranges(&blocks, &folded);
        assert_eq!(ranges.len(), 1);
        assert_eq!(
            ranges[0].start_row, 2,
            "fold must start at output_start_row, not command_start_row"
        );
        assert_eq!(ranges[0].end_row, 5);
        // block_total_rows reflects the (output-only) fold size: 4 rows.
        assert_eq!(ranges[0].block_total_rows, 4);
    }

    #[test]
    fn compute_falls_back_to_command_start_row_when_output_unset() {
        // Legacy shells that don't emit OSC 133 C leave output_start_row
        // as None. We must still allow folding — falling back to
        // command_start_row (the pre-fix behaviour) is the documented
        // contract.
        let blocks = [make_block_with_output(1, 0, Some(1), None, Some(5))];
        let folded = set(&[1]);
        let ranges = compute_fold_ranges(&blocks, &folded);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start_row, 1);
        assert_eq!(ranges[0].end_row, 5);
    }

    #[test]
    fn compute_sorts_by_start_row() {
        // Provide blocks out of order; expect sorted output.
        let blocks = [
            make_block(2, 10, Some(11), Some(15)),
            make_block(1, 0, Some(1), Some(5)),
        ];
        let folded = set(&[1, 2]);
        let ranges = compute_fold_ranges(&blocks, &folded);
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].start_row, 1);
        assert_eq!(ranges[1].start_row, 11);
    }

    #[test]
    fn compute_drops_degenerate_ranges() {
        // command_start_row > end_row should not happen but must be dropped.
        let blocks = [make_block(1, 0, Some(10), Some(5))];
        let folded = set(&[1]);
        assert!(compute_fold_ranges(&blocks, &folded).is_empty());
    }

    #[test]
    fn compute_drops_overlapping_later_range() {
        // Two blocks whose ranges overlap. The later one (by start_row)
        // is dropped per the documented policy.
        let blocks = [
            make_block(1, 0, Some(1), Some(10)), // 1..=10
            make_block(2, 5, Some(6), Some(15)), // 6..=15 overlaps with 1
        ];
        let folded = set(&[1, 2]);
        let ranges = compute_fold_ranges(&blocks, &folded);
        assert_eq!(ranges.len(), 1, "overlapping later range must be dropped");
        assert_eq!(ranges[0].command_block_id, CommandBlockId(1));
    }

    #[test]
    fn compute_keeps_adjacent_non_overlapping_ranges() {
        // Back-to-back ranges: prev.end_row + 1 == cur.start_row. NOT an
        // overlap — both must be kept.
        let blocks = [
            make_block(1, 0, Some(1), Some(5)),  // 1..=5
            make_block(2, 0, Some(6), Some(10)), // 6..=10  (adjacent)
        ];
        let folded = set(&[1, 2]);
        let ranges = compute_fold_ranges(&blocks, &folded);
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].end_row, 5);
        assert_eq!(ranges[1].start_row, 6);
    }

    // ── RowMap: identity (no folds) ─────────────────────────────────────

    #[test]
    fn rowmap_no_folds_is_identity() {
        let map = RowMap::new(20, &[]);
        assert_eq!(map.rendered_row_count(), 20);
        for snap in 0..20 {
            assert_eq!(map.snapshot_to_rendered(snap), Some(snap));
            assert_eq!(
                map.rendered_to_snapshot(snap),
                Some(RenderedRow::Snapshot(snap))
            );
        }
        assert_eq!(map.snapshot_to_rendered(20), None);
        assert_eq!(map.rendered_to_snapshot(20), None);
    }

    // ── RowMap: single fold in the middle ───────────────────────────────

    fn fold(id: u64, start: usize, end: usize) -> FoldRange {
        FoldRange {
            command_block_id: CommandBlockId(id),
            start_row: start,
            end_row: end,
            block_total_rows: end.saturating_sub(start).saturating_add(1),
        }
    }

    #[test]
    fn rowmap_single_fold_in_middle() {
        // 10 snapshot rows; fold rows 3..=6 (4 rows hidden → 1 placeholder).
        // Rendered count = 10 - 4 + 1 = 7.
        let ranges = [fold(1, 3, 6)];
        let map = RowMap::new(10, &ranges);
        assert_eq!(map.rendered_row_count(), 7);

        // Rows 0,1,2 map 1:1.
        for snap in 0..3 {
            assert_eq!(map.snapshot_to_rendered(snap), Some(snap));
        }
        // Rows 3,4,5,6 are inside the fold → None.
        for snap in 3..=6 {
            assert_eq!(map.snapshot_to_rendered(snap), None);
        }
        // Rows 7..=9 map to rendered 4..=6 (shifted by 4 hidden - 1 placeholder = 3).
        assert_eq!(map.snapshot_to_rendered(7), Some(4));
        assert_eq!(map.snapshot_to_rendered(8), Some(5));
        assert_eq!(map.snapshot_to_rendered(9), Some(6));

        // Reverse: rendered 0,1,2 → snap 0,1,2; rendered 3 → placeholder;
        // rendered 4,5,6 → snap 7,8,9.
        assert_eq!(map.rendered_to_snapshot(0), Some(RenderedRow::Snapshot(0)));
        assert_eq!(map.rendered_to_snapshot(2), Some(RenderedRow::Snapshot(2)));
        assert_eq!(
            map.rendered_to_snapshot(3),
            Some(RenderedRow::Placeholder(ranges[0]))
        );
        assert_eq!(map.rendered_to_snapshot(4), Some(RenderedRow::Snapshot(7)));
        assert_eq!(map.rendered_to_snapshot(6), Some(RenderedRow::Snapshot(9)));
        assert_eq!(map.rendered_to_snapshot(7), None);
    }

    // ── RowMap: fold at start of buffer ─────────────────────────────────

    #[test]
    fn rowmap_fold_at_start() {
        // Fold rows 0..=2 of a 10-row buffer.
        // Rendered count = 10 - 3 + 1 = 8.
        let ranges = [fold(1, 0, 2)];
        let map = RowMap::new(10, &ranges);
        assert_eq!(map.rendered_row_count(), 8);

        // Snapshot rows inside the fold are hidden.
        assert_eq!(map.snapshot_to_rendered(0), None);
        assert_eq!(map.snapshot_to_rendered(1), None);
        assert_eq!(map.snapshot_to_rendered(2), None);
        // Rows after the fold map to rendered 1..=7.
        assert_eq!(map.snapshot_to_rendered(3), Some(1));
        assert_eq!(map.snapshot_to_rendered(9), Some(7));

        // Rendered row 0 is the placeholder for the leading fold.
        assert_eq!(
            map.rendered_to_snapshot(0),
            Some(RenderedRow::Placeholder(ranges[0]))
        );
        assert_eq!(map.rendered_to_snapshot(1), Some(RenderedRow::Snapshot(3)));
    }

    // ── RowMap: fold at end of buffer ───────────────────────────────────

    #[test]
    fn rowmap_fold_at_end() {
        // Fold rows 7..=9 of a 10-row buffer.
        // Rendered count = 10 - 3 + 1 = 8.
        let ranges = [fold(1, 7, 9)];
        let map = RowMap::new(10, &ranges);
        assert_eq!(map.rendered_row_count(), 8);

        // Rows 0..=6 map 1:1.
        for snap in 0..=6 {
            assert_eq!(map.snapshot_to_rendered(snap), Some(snap));
        }
        // Rows 7,8,9 are hidden.
        for snap in 7..=9 {
            assert_eq!(map.snapshot_to_rendered(snap), None);
        }

        // Rendered row 7 is the trailing placeholder.
        assert_eq!(
            map.rendered_to_snapshot(7),
            Some(RenderedRow::Placeholder(ranges[0]))
        );
        assert_eq!(map.rendered_to_snapshot(8), None);
    }

    // ── RowMap: two non-adjacent folds ──────────────────────────────────

    #[test]
    fn rowmap_two_non_adjacent_folds() {
        // 20 rows. Fold [3..=5] (3 rows) and [10..=14] (5 rows).
        // Hidden: 8 rows, placeholders: 2. Rendered = 20 - 8 + 2 = 14.
        let ranges = [fold(1, 3, 5), fold(2, 10, 14)];
        let map = RowMap::new(20, &ranges);
        assert_eq!(map.rendered_row_count(), 14);

        // 0..=2 identity.
        for snap in 0..=2 {
            assert_eq!(map.snapshot_to_rendered(snap), Some(snap));
        }
        // 3..=5 hidden.
        for snap in 3..=5 {
            assert_eq!(map.snapshot_to_rendered(snap), None);
        }
        // 6..=9 shift down by (3 - 1) = 2 → rendered 4..=7.
        assert_eq!(map.snapshot_to_rendered(6), Some(4));
        assert_eq!(map.snapshot_to_rendered(9), Some(7));
        // 10..=14 hidden.
        for snap in 10..=14 {
            assert_eq!(map.snapshot_to_rendered(snap), None);
        }
        // 15..=19 shift down by (3 - 1) + (5 - 1) = 6 → rendered 9..=13.
        assert_eq!(map.snapshot_to_rendered(15), Some(9));
        assert_eq!(map.snapshot_to_rendered(19), Some(13));

        // Placeholders at rendered rows 3 and 8.
        assert_eq!(
            map.rendered_to_snapshot(3),
            Some(RenderedRow::Placeholder(ranges[0]))
        );
        assert_eq!(
            map.rendered_to_snapshot(8),
            Some(RenderedRow::Placeholder(ranges[1]))
        );
    }

    // ── RowMap: two adjacent folds ──────────────────────────────────────

    #[test]
    fn rowmap_two_adjacent_folds() {
        // 10 rows. Fold [1..=3] (3 rows) and [4..=6] (3 rows) — adjacent
        // (4 == 3+1, no overlap).
        // Hidden: 6 rows, placeholders: 2. Rendered = 10 - 6 + 2 = 6.
        let ranges = [fold(1, 1, 3), fold(2, 4, 6)];
        let map = RowMap::new(10, &ranges);
        assert_eq!(map.rendered_row_count(), 6);

        // Row 0 maps 1:1.
        assert_eq!(map.snapshot_to_rendered(0), Some(0));
        // Rows 1..=6 are hidden by the two adjacent folds.
        for snap in 1..=6 {
            assert_eq!(map.snapshot_to_rendered(snap), None);
        }
        // Rows 7..=9 follow placeholder 1 (rendered 1), placeholder 2
        // (rendered 2), so snap 7 → rendered 3, ..., snap 9 → rendered 5.
        assert_eq!(map.snapshot_to_rendered(7), Some(3));
        assert_eq!(map.snapshot_to_rendered(8), Some(4));
        assert_eq!(map.snapshot_to_rendered(9), Some(5));

        // Rendered: 0 → snap 0; 1 → placeholder(range 0);
        // 2 → placeholder(range 1); 3,4,5 → snap 7,8,9.
        assert_eq!(map.rendered_to_snapshot(0), Some(RenderedRow::Snapshot(0)));
        assert_eq!(
            map.rendered_to_snapshot(1),
            Some(RenderedRow::Placeholder(ranges[0]))
        );
        assert_eq!(
            map.rendered_to_snapshot(2),
            Some(RenderedRow::Placeholder(ranges[1]))
        );
        assert_eq!(map.rendered_to_snapshot(3), Some(RenderedRow::Snapshot(7)));
        assert_eq!(map.rendered_to_snapshot(5), Some(RenderedRow::Snapshot(9)));
        assert_eq!(map.rendered_to_snapshot(6), None);
    }

    // ── RowMap: round-trip identity for visible rows ────────────────────

    #[test]
    fn rowmap_round_trip_visible_rows_no_folds() {
        let map = RowMap::new(50, &[]);
        for snap in 0..50 {
            let rendered = map.snapshot_to_rendered(snap).unwrap();
            assert_eq!(
                map.rendered_to_snapshot(rendered),
                Some(RenderedRow::Snapshot(snap))
            );
        }
    }

    #[test]
    fn rowmap_round_trip_visible_rows_with_folds() {
        let ranges = [fold(1, 3, 5), fold(2, 10, 14), fold(3, 18, 19)];
        let map = RowMap::new(25, &ranges);
        for snap in 0..25 {
            if let Some(rendered) = map.snapshot_to_rendered(snap) {
                // Visible row → must round-trip exactly.
                assert_eq!(
                    map.rendered_to_snapshot(rendered),
                    Some(RenderedRow::Snapshot(snap)),
                    "round-trip failed for snapshot row {snap}"
                );
            }
        }
    }

    // ── RowMap: placeholder lookup carries the FoldRange ────────────────

    #[test]
    fn rowmap_placeholder_lookup_carries_range() {
        let r = fold(42, 5, 9);
        let map = RowMap::new(15, &[r]);
        // Rendered index 5 is the placeholder (rows 0..=4 are 1:1).
        match map.rendered_to_snapshot(5) {
            Some(RenderedRow::Placeholder(p)) => {
                assert_eq!(p, r);
                assert_eq!(p.command_block_id, CommandBlockId(42));
                assert_eq!(p.start_row, 5);
                assert_eq!(p.end_row, 9);
            }
            other => panic!("expected placeholder, got {other:?}"),
        }
    }

    // ── RowMap: defensive clamping ──────────────────────────────────────

    #[test]
    fn rowmap_clamps_range_ending_past_buffer() {
        // Range claims rows 5..=20 but buffer only has 10 rows.
        // The range should clamp to 5..=9.
        let ranges = [fold(1, 5, 20)];
        let map = RowMap::new(10, &ranges);
        // 5 rows hidden, 1 placeholder → 10 - 5 + 1 = 6.
        assert_eq!(map.rendered_row_count(), 6);
        for snap in 5..=9 {
            assert_eq!(map.snapshot_to_rendered(snap), None);
        }
    }

    #[test]
    fn rowmap_drops_range_starting_past_buffer() {
        // Range starts at row 50 in a 10-row buffer → dropped entirely.
        let ranges = [fold(1, 50, 60)];
        let map = RowMap::new(10, &ranges);
        assert_eq!(map.rendered_row_count(), 10);
        for snap in 0..10 {
            assert_eq!(map.snapshot_to_rendered(snap), Some(snap));
        }
    }

    // ── FoldRange helpers ───────────────────────────────────────────────

    #[test]
    fn foldrange_len_and_contains() {
        let r = fold(1, 3, 7);
        assert_eq!(r.len(), 5);
        assert!(!r.is_empty());
        assert!(!r.contains(2));
        assert!(r.contains(3));
        assert!(r.contains(5));
        assert!(r.contains(7));
        assert!(!r.contains(8));
    }

    #[test]
    fn foldrange_len_single_row() {
        let r = fold(1, 4, 4);
        assert_eq!(r.len(), 1);
        assert!(r.contains(4));
    }

    // ── compute_extra_rows ──────────────────────────────────────────────

    #[test]
    fn extra_rows_zero_when_no_folds() {
        assert_eq!(compute_extra_rows(&[], 100, 24), 0);
    }

    #[test]
    fn extra_rows_zero_when_term_height_zero() {
        let ranges = [fold(1, 100, 110)];
        assert_eq!(compute_extra_rows(&ranges, 100, 0), 0);
    }

    #[test]
    fn extra_rows_fold_fully_in_window() {
        // Window [100, 124). Fold buffer rows 105..=110 (6 rows) is fully
        // inside; collapsing frees 6 - 1 = 5 rows.
        let ranges = [fold(1, 105, 110)];
        assert_eq!(compute_extra_rows(&ranges, 100, 24), 5);
    }

    #[test]
    fn extra_rows_fold_entirely_in_scrollback_above() {
        // Window [100, 124). Fold 80..=90 is wholly above the window → no
        // freed space.
        let ranges = [fold(1, 80, 90)];
        assert_eq!(compute_extra_rows(&ranges, 100, 24), 0);
    }

    #[test]
    fn extra_rows_fold_straddles_top_of_window() {
        // Window [100, 124). Fold 95..=110 overlaps rows 100..=110 (11 rows
        // visible) → frees 11 - 1 = 10.
        let ranges = [fold(1, 95, 110)];
        assert_eq!(compute_extra_rows(&ranges, 100, 24), 10);
    }

    #[test]
    fn extra_rows_capped_by_available_scrollback() {
        // Window starts at row 3, so only 3 rows exist above it. A fold that
        // would free 10 rows is capped at 3.
        let ranges = [fold(1, 3, 20)];
        assert_eq!(compute_extra_rows(&ranges, 3, 24), 3);
    }

    #[test]
    fn extra_rows_sums_multiple_folds() {
        // Window [100, 130). Fold A 102..=105 (4 rows → frees 3), fold B
        // 110..=120 (11 rows → frees 10). Total freed 13, capped by 100.
        let ranges = [fold(1, 102, 105), fold(2, 110, 120)];
        assert_eq!(compute_extra_rows(&ranges, 100, 30), 13);
    }

    #[test]
    fn extra_rows_single_row_fold_frees_nothing() {
        // A 1-row fold collapses to a 1-row placeholder: no net space freed.
        let ranges = [fold(1, 105, 105)];
        assert_eq!(compute_extra_rows(&ranges, 100, 24), 0);
    }

    // ── apply_rendered_scroll ───────────────────────────────────────────

    #[test]
    fn rendered_scroll_no_folds_is_one_to_one_up() {
        // total 100, height 24, max 76. Up 3 from offset 0 → 3.
        let n = apply_rendered_scroll(&[], 100, 24, 76, 0, ScrollDir::Up, 3);
        assert_eq!(n, 3);
    }

    #[test]
    fn rendered_scroll_no_folds_is_one_to_one_down() {
        let n = apply_rendered_scroll(&[], 100, 24, 76, 10, ScrollDir::Down, 3);
        assert_eq!(n, 7);
    }

    #[test]
    fn rendered_scroll_clamps_up_at_max() {
        let n = apply_rendered_scroll(&[], 100, 24, 76, 75, ScrollDir::Up, 10);
        assert_eq!(n, 76);
    }

    #[test]
    fn rendered_scroll_clamps_down_at_zero() {
        let n = apply_rendered_scroll(&[], 100, 24, 76, 2, ScrollDir::Down, 10);
        assert_eq!(n, 0);
    }

    #[test]
    fn rendered_scroll_up_skips_collapsed_fold_in_one_step() {
        // total 100, height 24, max 76. Window top at offset 0 is row 76.
        // A fold collapses buffer rows 70..=79 (start 70, end 79). The fold's
        // first row (70) is the placeholder; rows 71..=79 are hidden.
        //
        // At a raw offset that places the top just below the fold's hidden
        // span, scrolling up one rendered row must skip the entire hidden span
        // in a single move and land the placeholder row at the top.
        let ranges = [fold(1, 70, 79)];
        // Offset 6 → top = 100 - 24 - 6 = 70 (the placeholder row already at
        // top). One step up reveals row 69 (not folded) → raw 7.
        let n = apply_rendered_scroll(&ranges, 100, 24, 76, 6, ScrollDir::Up, 1);
        assert_eq!(n, 7, "row above the fold is normal → +1 raw");

        // Offset 0 → top = 76, inside the fold's hidden span (71..=79).
        // Revealed row would be 75 (hidden). One rendered step up must jump to
        // the placeholder row 70: raw such that top == 70 → raw = 6.
        let n2 = apply_rendered_scroll(&ranges, 100, 24, 76, 0, ScrollDir::Up, 1);
        assert_eq!(
            n2, 6,
            "one rendered step from inside a fold jumps to the placeholder"
        );
    }

    #[test]
    fn rendered_scroll_down_skips_collapsed_fold() {
        // Mirror of the up case. Fold 70..=79. From offset 7 (top = 69),
        // scrolling down one rendered row: new top would be 70 (placeholder) —
        // not hidden, so raw - 1 = 6.
        let ranges = [fold(1, 70, 79)];
        let n = apply_rendered_scroll(&ranges, 100, 24, 76, 7, ScrollDir::Down, 1);
        assert_eq!(n, 6);

        // From offset 6 (top = 70, the placeholder). Scrolling down, the new
        // top (71) is hidden inside the fold → jump past the whole span so the
        // new top is row 80 (after fold end): raw 6 - (80 - 70) saturates to 0.
        let n2 = apply_rendered_scroll(&ranges, 100, 24, 76, 6, ScrollDir::Down, 1);
        assert_eq!(n2, 0, "down past a fold skips the hidden span");
    }

    // ── translate_ranges_to_snapshot ────────────────────────────────────

    #[test]
    fn translate_identity_when_win_start_zero() {
        let ranges = [fold(1, 3, 6), fold(2, 10, 14)];
        let out = translate_ranges_to_snapshot(&ranges, 0);
        assert_eq!(out, ranges);
    }

    #[test]
    fn translate_drops_range_entirely_in_scrollback() {
        // win_start=65, range 46..=60 is wholly in scrollback (end < win_start).
        let ranges = [fold(1, 46, 60)];
        let out = translate_ranges_to_snapshot(&ranges, 65);
        assert!(out.is_empty(), "wholly-scrollback range must be dropped");
    }

    #[test]
    fn translate_clamps_straddling_range_to_zero() {
        // The bug-trigger case from the field: block 1 spans 46..=79,
        // visible_window_start=65, term_height=17 (visible rows 65..=81).
        // The visible portion is rows 65..=79 → snapshot rows 0..=14.
        let ranges = [fold(1, 46, 79)];
        let out = translate_ranges_to_snapshot(&ranges, 65);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].start_row, 0);
        assert_eq!(out[0].end_row, 14);
        assert_eq!(out[0].command_block_id, CommandBlockId(1));
    }

    #[test]
    fn translate_wholly_visible_range_shifts_uniformly() {
        // Range 70..=75 with win_start=65 → snapshot rows 5..=10.
        let ranges = [fold(1, 70, 75)];
        let out = translate_ranges_to_snapshot(&ranges, 65);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].start_row, 5);
        assert_eq!(out[0].end_row, 10);
    }

    #[test]
    fn translate_then_rowmap_collapses_straddling_block() {
        // Full pipeline regression for the field bug.
        let raw = [fold(1, 46, 79)];
        let translated = translate_ranges_to_snapshot(&raw, 65);
        let map = RowMap::new(17, &translated);
        // 15 visible folded rows → 1 placeholder; rendered = 17 - 15 + 1 = 3.
        assert_eq!(map.rendered_row_count(), 3);
        assert_eq!(
            map.rendered_to_snapshot(0),
            Some(RenderedRow::Placeholder(translated[0]))
        );
        assert_eq!(map.rendered_to_snapshot(1), Some(RenderedRow::Snapshot(15)));
    }

    #[test]
    fn block_total_rows_is_stable_across_scroll() {
        // Block 1 is 34 rows of buffer-absolute output (46..=79). As the
        // user scrolls, the *visible portion* of the fold shrinks/grows,
        // but the placeholder text must always report the full 34 lines
        // so the user is not misled about how much content is hidden.
        let raw = compute_fold_ranges(&[make_block(1, 45, Some(46), Some(79))], &set(&[1]));
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].block_total_rows, 34);

        // No scroll (block partially in scrollback, partially visible).
        let t0 = translate_ranges_to_snapshot(&raw, 65);
        assert_eq!(t0[0].block_total_rows, 34);
        let m0 = RowMap::new(17, &t0);
        assert_eq!(m0.ranges()[0].block_total_rows, 34);

        // Scrolled up so the block fully covers the visible window.
        let t1 = translate_ranges_to_snapshot(&raw, 55);
        assert_eq!(t1[0].block_total_rows, 34);
        let m1 = RowMap::new(17, &t1);
        assert_eq!(m1.ranges()[0].block_total_rows, 34);

        // Scrolled up so only the top of the block is visible.
        let t2 = translate_ranges_to_snapshot(&raw, 40);
        assert_eq!(t2[0].block_total_rows, 34);

        // Scrolled past the block entirely (visible window above it).
        let t3 = translate_ranges_to_snapshot(&raw, 5);
        assert_eq!(t3[0].block_total_rows, 34);
        // RowMap drops the range because its translated start is past
        // term_height — no placeholder rendered, pre-command scrollback
        // is fully visible.
        let m3 = RowMap::new(17, &t3);
        assert_eq!(m3.rendered_row_count(), 17);
        assert!(m3.ranges().is_empty());
    }

    // ── end-to-end: compute_fold_ranges feeding RowMap ──────────────────

    #[test]
    fn end_to_end_compute_then_map() {
        let blocks = [
            make_block(1, 0, Some(1), Some(3)),    // foldable
            make_block(2, 4, Some(5), None),       // running — skipped
            make_block(3, 10, Some(11), Some(14)), // foldable
        ];
        let folded = set(&[1, 2, 3]);
        let ranges = compute_fold_ranges(&blocks, &folded);
        assert_eq!(ranges.len(), 2, "running block id 2 must be skipped");

        let map = RowMap::new(20, &ranges);
        // Hidden: (3-1+1) + (14-11+1) = 3 + 4 = 7. Placeholders: 2.
        // Rendered: 20 - 7 + 2 = 15.
        assert_eq!(map.rendered_row_count(), 15);
        assert_eq!(map.snapshot_to_rendered(0), Some(0));
        assert_eq!(map.snapshot_to_rendered(1), None);
        assert_eq!(map.snapshot_to_rendered(4), Some(2));
    }
}
