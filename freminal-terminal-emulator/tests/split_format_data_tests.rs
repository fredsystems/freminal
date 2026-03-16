// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Unit tests for `split_format_data_for_scrollback`.
//!
//! This is a pure function that partitions a `Vec<FormatTag>` at a split point
//! into scrollback and visible sections.  Each tag in the scrollback section
//! has its `end` clamped to the split point.  Each tag in the visible section
//! has its offsets rebased so that `start = 0` corresponds to the first
//! visible character.
//!
//! **Note:** As of this writing, `split_format_data_for_scrollback` has zero
//! production callers — it was written for the scrollback rendering path but
//! is not yet wired up.  These tests document its intended contract so that
//! future integration can rely on tested behaviour.

use freminal_common::buffer_states::format_tag::FormatTag;
use freminal_terminal_emulator::interface::split_format_data_for_scrollback;

// ─── helpers ────────────────────────────────────────────────────────────────

/// Create a `FormatTag` with the given `start` and `end`, all other fields
/// default (default colors, normal weight, no decorations, no URL).
fn tag(start: usize, end: usize) -> FormatTag {
    FormatTag {
        start,
        end,
        ..FormatTag::default()
    }
}

// ─── Empty input ─────────────────────────────────────────────────────────────

#[test]
fn empty_tags_returns_both_empty() {
    let result = split_format_data_for_scrollback(vec![], 10, 20, true);
    assert!(result.scrollback.is_empty(), "scrollback must be empty");
    assert!(result.visible.is_empty(), "visible must be empty");
}

#[test]
fn empty_tags_with_include_scrollback_false() {
    let result = split_format_data_for_scrollback(vec![], 10, 20, false);
    assert!(result.scrollback.is_empty());
    assert!(result.visible.is_empty());
}

// ─── include_scrollback = false ──────────────────────────────────────────────

#[test]
fn include_scrollback_false_always_produces_empty_scrollback() {
    let tags = vec![tag(0, 5), tag(5, 15), tag(15, 20)];
    let result = split_format_data_for_scrollback(tags, 10, 20, false);
    assert!(
        result.scrollback.is_empty(),
        "scrollback must be empty when include_scrollback = false"
    );
    // Visible section is still populated normally.
    assert!(
        !result.visible.is_empty(),
        "visible section must still contain tags that fall in the visible range"
    );
}

// ─── Tag entirely before the split point ─────────────────────────────────────

#[test]
fn tag_entirely_before_split_appears_in_scrollback_only() {
    let tags = vec![tag(0, 5)];
    let result = split_format_data_for_scrollback(tags, 10, 20, true);

    assert_eq!(result.scrollback.len(), 1);
    assert_eq!(result.scrollback[0].start, 0);
    assert_eq!(
        result.scrollback[0].end, 5,
        "end must be unchanged (already < split)"
    );

    assert!(
        result.visible.is_empty(),
        "tag before split must not appear in visible"
    );
}

// ─── Tag entirely after the split (within visible_end) ───────────────────────

#[test]
fn tag_entirely_after_split_appears_in_visible_only_rebased() {
    // split = 10, visible_end = 30.  Tag [15..20] is entirely in the visible range.
    let tags = vec![tag(15, 20)];
    let result = split_format_data_for_scrollback(tags, 10, 30, true);

    assert!(
        result.scrollback.is_empty(),
        "tag entirely after split must not appear in scrollback"
    );

    assert_eq!(result.visible.len(), 1);
    assert_eq!(
        result.visible[0].start, 5,
        "start must be rebased: 15 - 10 = 5"
    );
    assert_eq!(
        result.visible[0].end, 10,
        "end must be rebased: 20 - 10 = 10"
    );
}

// ─── Tag spanning the split boundary ─────────────────────────────────────────

#[test]
fn spanning_tag_appears_in_both_sections() {
    // Tag [5..15] with split at 10: scrollback gets [5..10], visible gets [0..5].
    let tags = vec![tag(5, 15)];
    let result = split_format_data_for_scrollback(tags, 10, 20, true);

    // Scrollback: start < 10 → included, end clamped to 10.
    assert_eq!(result.scrollback.len(), 1);
    assert_eq!(result.scrollback[0].start, 5);
    assert_eq!(
        result.scrollback[0].end, 10,
        "scrollback end must be clamped to split"
    );

    // Visible: end > 10 && end <= 20 → included, rebased.
    assert_eq!(result.visible.len(), 1);
    assert_eq!(
        result.visible[0].start, 0,
        "visible start must be rebased: max(5, 10) - 10 = 0 via saturating_sub"
    );
    assert_eq!(
        result.visible[0].end, 5,
        "visible end must be rebased: 15 - 10 = 5"
    );
}

// ─── Tag at exact split boundary ─────────────────────────────────────────────

#[test]
fn tag_ending_at_split_goes_to_scrollback_only() {
    // Tag [3..10], split at 10.
    // Scrollback: start < 10 → included, end = min(10, 10) = 10.
    // Visible: end > 10 → false (end == 10).  Not included.
    let tags = vec![tag(3, 10)];
    let result = split_format_data_for_scrollback(tags, 10, 20, true);

    assert_eq!(result.scrollback.len(), 1);
    assert_eq!(result.scrollback[0].start, 3);
    assert_eq!(result.scrollback[0].end, 10);

    assert!(
        result.visible.is_empty(),
        "tag ending exactly at split must not appear in visible (filter is end > split)"
    );
}

#[test]
fn tag_starting_at_split_goes_to_visible_only() {
    // Tag [10..15], split at 10.
    // Scrollback: start < 10 → false (start == 10).  Not included.
    // Visible: end > 10 && end <= 20 → true.  Rebased: start = 0, end = 5.
    let tags = vec![tag(10, 15)];
    let result = split_format_data_for_scrollback(tags, 10, 20, true);

    assert!(
        result.scrollback.is_empty(),
        "tag starting at split must not be in scrollback"
    );

    assert_eq!(result.visible.len(), 1);
    assert_eq!(
        result.visible[0].start, 0,
        "start must be rebased: 10 - 10 = 0"
    );
    assert_eq!(result.visible[0].end, 5, "end must be rebased: 15 - 10 = 5");
}

// ─── Tag past visible_end is dropped from visible ────────────────────────────

#[test]
fn tag_past_visible_end_is_dropped_from_visible() {
    // Tag [12..25], split = 10, visible_end = 20.
    // Scrollback: start < 10 → false (12 >= 10).
    // Visible: end > 10 → true, but end <= 20 → false (25 > 20).  Dropped.
    let tags = vec![tag(12, 25)];
    let result = split_format_data_for_scrollback(tags, 10, 20, true);

    assert!(result.scrollback.is_empty());
    assert!(
        result.visible.is_empty(),
        "tag with end > visible_end must be dropped from visible"
    );
}

// ─── Sentinel end (usize::MAX) ───────────────────────────────────────────────

#[test]
fn sentinel_end_tag_in_scrollback_is_clamped() {
    // A default tag with end = usize::MAX, start = 0, split = 10.
    // Scrollback: start < 10 → true.  end = min(usize::MAX, 10) = 10.
    let tags = vec![tag(0, usize::MAX)];
    let result = split_format_data_for_scrollback(tags, 10, 20, true);

    assert_eq!(result.scrollback.len(), 1);
    assert_eq!(
        result.scrollback[0].end, 10,
        "sentinel end must be clamped to split"
    );
}

#[test]
fn sentinel_end_tag_is_dropped_from_visible() {
    // Tag with end = usize::MAX.
    // Visible filter: end > split (true) && end <= visible_end (usize::MAX <= 20 → false).
    // The tag is dropped from the visible section.
    let tags = vec![tag(0, usize::MAX)];
    let result = split_format_data_for_scrollback(tags, 10, 20, true);

    assert!(
        result.visible.is_empty(),
        "sentinel-end tag (usize::MAX) must be dropped from visible (fails end <= visible_end)"
    );
}

#[test]
fn sentinel_end_tag_passes_visible_when_visible_end_is_max() {
    // When visible_end is also usize::MAX, the sentinel tag passes the filter.
    let tags = vec![tag(5, usize::MAX)];
    let result = split_format_data_for_scrollback(tags, 10, usize::MAX, true);

    assert_eq!(result.visible.len(), 1);
    // start rebased: saturating_sub(10) = 0 (5 < 10 → 0)
    assert_eq!(result.visible[0].start, 0);
    // end stays usize::MAX (the `if tag.end != usize::MAX` guard prevents subtraction)
    assert_eq!(result.visible[0].end, usize::MAX);
}

// ─── Multiple tags, mixed positions ──────────────────────────────────────────

#[test]
fn multiple_tags_sorted_correctly() {
    // Split = 10, visible_end = 30.
    let tags = vec![
        tag(0, 5),   // entirely in scrollback
        tag(3, 12),  // spans split
        tag(10, 20), // entirely in visible
        tag(20, 35), // past visible_end → dropped from visible
        tag(25, 30), // entirely in visible
    ];
    let result = split_format_data_for_scrollback(tags, 10, 30, true);

    // Scrollback: tags with start < 10: [0..5] and [3..12].
    assert_eq!(result.scrollback.len(), 2);
    assert_eq!(result.scrollback[0].start, 0);
    assert_eq!(result.scrollback[0].end, 5);
    assert_eq!(result.scrollback[1].start, 3);
    assert_eq!(
        result.scrollback[1].end, 10,
        "spanning tag end clamped to 10"
    );

    // Visible: tags with end > 10 && end <= 30: [3..12] and [10..20] and [25..30].
    // [20..35] is dropped (35 > 30).
    assert_eq!(result.visible.len(), 3);

    // [3..12] → rebased [0..2]
    assert_eq!(result.visible[0].start, 0);
    assert_eq!(result.visible[0].end, 2);

    // [10..20] → rebased [0..10]
    assert_eq!(result.visible[1].start, 0);
    assert_eq!(result.visible[1].end, 10);

    // [25..30] → rebased [15..20]
    assert_eq!(result.visible[2].start, 15);
    assert_eq!(result.visible[2].end, 20);
}

// ─── Zero split point ────────────────────────────────────────────────────────

#[test]
fn zero_split_puts_everything_in_visible() {
    let tags = vec![tag(0, 10), tag(10, 20)];
    let result = split_format_data_for_scrollback(tags, 0, 30, true);

    // No tag has start < 0, so scrollback is empty.
    assert!(result.scrollback.is_empty());

    // Both tags have end > 0 && end <= 30.
    assert_eq!(result.visible.len(), 2);
    // Rebasing with split = 0 is a no-op.
    assert_eq!(result.visible[0].start, 0);
    assert_eq!(result.visible[0].end, 10);
    assert_eq!(result.visible[1].start, 10);
    assert_eq!(result.visible[1].end, 20);
}

// ─── Single tag covering the entire range ────────────────────────────────────

#[test]
fn single_tag_covering_full_range_splits_into_both() {
    // Tag [0..20], split = 10, visible_end = 20.
    let tags = vec![tag(0, 20)];
    let result = split_format_data_for_scrollback(tags, 10, 20, true);

    // Scrollback: [0..10]
    assert_eq!(result.scrollback.len(), 1);
    assert_eq!(result.scrollback[0].start, 0);
    assert_eq!(result.scrollback[0].end, 10);

    // Visible: rebased [0..10]
    assert_eq!(result.visible.len(), 1);
    assert_eq!(result.visible[0].start, 0);
    assert_eq!(result.visible[0].end, 10);
}

// ─── Tag start == end (zero-width) ───────────────────────────────────────────

#[test]
fn zero_width_tag_before_split_is_in_scrollback() {
    // Zero-width tag at position 5, split = 10.
    // start < 10 → included in scrollback.  end > 10 → false → not in visible.
    let tags = vec![tag(5, 5)];
    let result = split_format_data_for_scrollback(tags, 10, 20, true);

    assert_eq!(result.scrollback.len(), 1);
    assert_eq!(result.scrollback[0].start, 5);
    assert_eq!(result.scrollback[0].end, 5);
    assert!(result.visible.is_empty());
}

#[test]
fn zero_width_tag_after_split_is_in_neither() {
    // Zero-width tag at position 15, split = 10, visible_end = 20.
    // start < 10 → false → not in scrollback.
    // end > 10 → true, end <= 20 → true → would be in visible,
    // but it's zero-width with start == end == 15.
    let tags = vec![tag(15, 15)];
    let result = split_format_data_for_scrollback(tags, 10, 20, true);

    // start >= 10, not in scrollback.
    assert!(result.scrollback.is_empty());
    // end > 10 && end <= 20 → passes filter; rebased start = 5, end = 5.
    assert_eq!(result.visible.len(), 1);
    assert_eq!(result.visible[0].start, 5);
    assert_eq!(result.visible[0].end, 5);
}

// ─── Visible end equals split (empty visible range) ──────────────────────────

#[test]
fn visible_end_equals_split_drops_all_from_visible() {
    // When visible_end == split, no tag can satisfy end > split && end <= split.
    let tags = vec![tag(0, 5), tag(3, 12)];
    let result = split_format_data_for_scrollback(tags, 10, 10, true);

    // Scrollback still gets its tags.
    assert_eq!(result.scrollback.len(), 2);
    // Visible is empty.
    assert!(result.visible.is_empty());
}
