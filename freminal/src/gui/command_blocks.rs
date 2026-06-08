// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Helpers for OSC 133 command-block GUI overlays.
//!
//! Currently contains:
//! - [`format_command_duration`] — compact human-readable duration
//!   formatter used by the duration-label overlay drawn at the end of a
//!   finished block's first row.
//! - [`command_block_overlays_visible`] — gate deciding whether any
//!   command-block visual affordance (hover tint, duration label) may be
//!   drawn for the current frame.
//! - [`gutter_status_for_row`] — maps a buffer-absolute row to the
//!   [`CommandStatus`] of the command block that contains it, if any
//!   (the gutter color decision, factored out for unit testing).
//!
//! Kept as a standalone module so the formatter can be unit-tested
//! without an egui or GPU context.

use freminal_common::buffer_states::command_block::{CommandBlock, CommandStatus};
use std::time::Duration;

/// Find the command block containing the given buffer-absolute `row`, for
/// gutter coloring, hover, and click hit-testing.
///
/// A block spans from its `prompt_start_row` to its `end_row` (inclusive).
/// A still-running block (no `end_row`) is treated as extending to
/// `running_extent` — the last visible buffer row — so its gutter bar
/// fills down to the live prompt.  Returns `None` for rows not covered by
/// any block.
///
/// When blocks overlap (which should not happen for well-formed OSC 133
/// streams, but is not structurally prevented), the **last** matching
/// block in iteration order wins, matching the most-recently-emitted
/// command.
#[must_use]
pub fn gutter_block_for_row(
    blocks: &[CommandBlock],
    row: usize,
    running_extent: usize,
) -> Option<&CommandBlock> {
    let mut found = None;
    for block in blocks {
        let start = block.prompt_start_row;
        let end = block.end_row.unwrap_or(running_extent);
        if row >= start && row <= end {
            found = Some(block);
        }
    }
    found
}

/// Determine the [`CommandStatus`] of the command block containing the
/// given buffer-absolute `row`, for gutter coloring.
///
/// Thin wrapper over [`gutter_block_for_row`]; see that function for the
/// row-span and overlap semantics.
#[must_use]
pub fn gutter_status_for_row(
    blocks: &[CommandBlock],
    row: usize,
    running_extent: usize,
) -> Option<CommandStatus> {
    gutter_block_for_row(blocks, row, running_extent).map(CommandBlock::status)
}

/// Whether a command block can be folded.
///
/// Only finished blocks (those with an `end_row`) are foldable; a running
/// block has no defined output range to collapse, so a gutter click on it
/// is a no-op (it still focuses the pane).  This mirrors the
/// `command_start_row.is_some() && end_row.is_some()` guard used by the
/// `FoldPreviousCommand` keybinding.
#[must_use]
pub const fn block_is_foldable(block: &CommandBlock) -> bool {
    block.end_row.is_some()
}

/// Decide whether command-block visual overlays (hover-row tint and the
/// duration label) may be drawn this frame.
///
/// Overlays are suppressed when:
///
/// - the command-block feature is disabled (`enabled == false`), or
/// - the alternate screen is active — the stored [`CommandBlock`]s
///   describe primary-screen buffer rows, so painting hover tints or
///   duration labels over a full-screen TUI (vim, htop, less, …) would
///   highlight unrelated rows, or
/// - there are no command blocks to draw.
///
/// [`CommandBlock`]: freminal_common::buffer_states::command_block::CommandBlock
#[must_use]
pub const fn command_block_overlays_visible(
    feature_enabled: bool,
    is_alternate_screen: bool,
    has_blocks: bool,
) -> bool {
    feature_enabled && !is_alternate_screen && has_blocks
}

/// Format a finished command's wall-clock duration as a compact label
/// such as `"3s"`, `"2m15s"`, or `"1h3m"`.
///
/// Rules:
///
/// - Sub-second durations always round up to `"1s"` (the threshold gate
///   in the caller filters these out before we reach the formatter, so
///   they should not normally occur; rounding up is a safer default
///   than emitting `"0s"`).
/// - `< 60 s` → `"Ns"` (whole seconds, truncated).
/// - `< 1 h` → `"NmSs"` (e.g. `"2m15s"`). The seconds component is
///   suppressed when zero (`"5m"`).
/// - `≥ 1 h` → `"HhMm"` (e.g. `"1h3m"`). The minutes component is
///   suppressed when zero (`"2h"`).
///
/// The output never contains internal whitespace so it can be drawn as
/// a single right-aligned label in a fixed-width slot without word
/// wrapping concerns.
#[must_use]
pub fn format_command_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    if total_secs < 1 {
        return "1s".to_string();
    }
    if total_secs < 60 {
        return format!("{total_secs}s");
    }
    if total_secs < 3600 {
        let m = total_secs / 60;
        let s = total_secs % 60;
        if s == 0 {
            return format!("{m}m");
        }
        return format!("{m}m{s}s");
    }
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    if m == 0 {
        format!("{h}h")
    } else {
        format!("{h}h{m}m")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn sub_second_rounds_up_to_one() {
        assert_eq!(format_command_duration(Duration::from_millis(0)), "1s");
        assert_eq!(format_command_duration(Duration::from_millis(450)), "1s");
        assert_eq!(format_command_duration(Duration::from_millis(999)), "1s");
    }

    #[test]
    fn whole_seconds_under_a_minute() {
        assert_eq!(format_command_duration(Duration::from_secs(1)), "1s");
        assert_eq!(format_command_duration(Duration::from_secs(3)), "3s");
        assert_eq!(format_command_duration(Duration::from_secs(59)), "59s");
    }

    #[test]
    fn fractional_seconds_truncate_not_round() {
        // 2.9s should display as "2s", not "3s" — we truncate so the
        // displayed value never overstates how long the command took.
        assert_eq!(format_command_duration(Duration::from_millis(2_900)), "2s");
    }

    #[test]
    fn minutes_with_seconds() {
        assert_eq!(format_command_duration(Duration::from_mins(1)), "1m");
        assert_eq!(format_command_duration(Duration::from_secs(75)), "1m15s");
        assert_eq!(format_command_duration(Duration::from_secs(135)), "2m15s");
        assert_eq!(
            format_command_duration(Duration::from_secs(3_599)),
            "59m59s"
        );
    }

    #[test]
    fn whole_minutes_suppress_seconds() {
        assert_eq!(format_command_duration(Duration::from_mins(2)), "2m");
        assert_eq!(format_command_duration(Duration::from_mins(10)), "10m");
    }

    #[test]
    fn hours_with_minutes() {
        assert_eq!(format_command_duration(Duration::from_hours(1)), "1h");
        assert_eq!(format_command_duration(Duration::from_mins(63)), "1h3m");
        assert_eq!(format_command_duration(Duration::from_hours(2)), "2h");
        assert_eq!(
            format_command_duration(Duration::from_mins(150) + Duration::from_secs(15)),
            "2h30m"
        );
    }

    #[test]
    fn whole_hours_suppress_minutes() {
        assert_eq!(format_command_duration(Duration::from_hours(1)), "1h");
        assert_eq!(format_command_duration(Duration::from_hours(24)), "24h");
    }

    #[test]
    fn boundary_at_one_minute_no_extra_seconds() {
        // 60s exactly should render as "1m" not "1m0s".
        assert_eq!(format_command_duration(Duration::from_mins(1)), "1m");
    }

    #[test]
    fn boundary_at_one_hour_no_extra_minutes() {
        // 3600s exactly should render as "1h" not "1h0m".
        assert_eq!(format_command_duration(Duration::from_hours(1)), "1h");
    }

    #[test]
    fn overlays_visible_only_when_enabled_present_and_primary_screen() {
        // All preconditions met → visible.
        assert!(command_block_overlays_visible(true, false, true));
    }

    #[test]
    fn overlays_hidden_when_feature_disabled() {
        assert!(!command_block_overlays_visible(false, false, true));
    }

    #[test]
    fn overlays_hidden_on_alternate_screen() {
        // The regression this gate fixes: a full-screen TUI on the
        // alternate screen must not show stale primary-screen command
        // affordances, even though blocks are present and the feature
        // is on.
        assert!(!command_block_overlays_visible(true, true, true));
    }

    #[test]
    fn overlays_hidden_when_no_blocks() {
        assert!(!command_block_overlays_visible(true, false, false));
    }

    #[test]
    fn alternate_screen_dominates_every_other_precondition() {
        // No matter how the other two flags are set, the alternate
        // screen always suppresses overlays.
        for enabled in [false, true] {
            for has_blocks in [false, true] {
                assert!(
                    !command_block_overlays_visible(enabled, true, has_blocks),
                    "alt-screen should suppress overlays (enabled={enabled}, has_blocks={has_blocks})"
                );
            }
        }
    }

    #[test]
    fn never_contains_whitespace() {
        // Right-alignment over a single rendered row relies on the
        // label being a single non-wrapping token.
        for secs in [1_u64, 30, 59, 60, 61, 599, 3_599, 3_600, 3_601, 86_400] {
            let s = format_command_duration(Duration::from_secs(secs));
            assert!(!s.contains(' '), "duration '{s}' contains whitespace");
        }
    }

    // ── gutter_status_for_row ────────────────────────────────────────────

    use freminal_common::buffer_states::command_block::CommandBlockId;
    use std::time::SystemTime;

    /// Build a finished block spanning `[prompt_start, end]` with the given
    /// exit code.
    fn finished_block(prompt_start: usize, end: usize, exit: Option<i32>) -> CommandBlock {
        let started = SystemTime::UNIX_EPOCH;
        CommandBlock {
            id: CommandBlockId::next(),
            fid: "t".to_owned(),
            prompt_start_row: prompt_start,
            command_start_row: Some(prompt_start),
            output_start_row: Some(prompt_start + 1),
            end_row: Some(end),
            exit_code: exit,
            cwd: None,
            started_at: started,
            executed_at: Some(started),
            finished_at: Some(started + Duration::from_secs(1)),
        }
    }

    /// Build a still-running block starting at `prompt_start` (no end row).
    fn running_block(prompt_start: usize) -> CommandBlock {
        CommandBlock {
            id: CommandBlockId::next(),
            fid: "t".to_owned(),
            prompt_start_row: prompt_start,
            command_start_row: Some(prompt_start),
            output_start_row: Some(prompt_start + 1),
            end_row: None,
            exit_code: None,
            cwd: None,
            started_at: SystemTime::UNIX_EPOCH,
            executed_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn gutter_status_none_when_no_block_contains_row() {
        let blocks = [finished_block(2, 5, Some(0))];
        assert_eq!(gutter_status_for_row(&blocks, 0, 100), None);
        assert_eq!(gutter_status_for_row(&blocks, 1, 100), None);
        assert_eq!(gutter_status_for_row(&blocks, 6, 100), None);
    }

    #[test]
    fn gutter_status_inclusive_of_both_endpoints() {
        let blocks = [finished_block(2, 5, Some(0))];
        // prompt_start_row (2) and end_row (5) are both inside the block.
        assert_eq!(
            gutter_status_for_row(&blocks, 2, 100),
            Some(CommandStatus::Success)
        );
        assert_eq!(
            gutter_status_for_row(&blocks, 5, 100),
            Some(CommandStatus::Success)
        );
        assert_eq!(
            gutter_status_for_row(&blocks, 3, 100),
            Some(CommandStatus::Success)
        );
    }

    #[test]
    fn gutter_status_reflects_exit_code() {
        let success = [finished_block(0, 3, Some(0))];
        let failure = [finished_block(0, 3, Some(127))];
        let unknown = [finished_block(0, 3, None)];
        assert_eq!(
            gutter_status_for_row(&success, 1, 100),
            Some(CommandStatus::Success)
        );
        assert_eq!(
            gutter_status_for_row(&failure, 1, 100),
            Some(CommandStatus::Failure(127))
        );
        assert_eq!(
            gutter_status_for_row(&unknown, 1, 100),
            Some(CommandStatus::Unknown)
        );
    }

    #[test]
    fn gutter_status_running_block_extends_to_running_extent() {
        let blocks = [running_block(4)];
        // Inside [4, running_extent=10].
        assert_eq!(
            gutter_status_for_row(&blocks, 4, 10),
            Some(CommandStatus::Running)
        );
        assert_eq!(
            gutter_status_for_row(&blocks, 10, 10),
            Some(CommandStatus::Running)
        );
        // Above prompt start — not in the block.
        assert_eq!(gutter_status_for_row(&blocks, 3, 10), None);
        // Past the running extent — not painted.
        assert_eq!(gutter_status_for_row(&blocks, 11, 10), None);
    }

    #[test]
    fn gutter_status_last_matching_block_wins_on_overlap() {
        // Two blocks both claiming row 5; the later one (in iteration
        // order) wins, matching the most-recently-emitted command.
        let blocks = [
            finished_block(0, 8, Some(0)), // success, spans 0..=8
            finished_block(5, 9, Some(1)), // failure, spans 5..=9
        ];
        assert_eq!(
            gutter_status_for_row(&blocks, 5, 100),
            Some(CommandStatus::Failure(1))
        );
        // Row only in the first block stays success.
        assert_eq!(
            gutter_status_for_row(&blocks, 1, 100),
            Some(CommandStatus::Success)
        );
    }

    #[test]
    fn gutter_block_for_row_returns_the_block_not_just_status() {
        let blocks = [finished_block(2, 5, Some(7))];
        let hit = gutter_block_for_row(&blocks, 3, 100).expect("row 3 is inside the block");
        assert_eq!(hit.prompt_start_row, 2);
        assert_eq!(hit.exit_code, Some(7));
        assert!(gutter_block_for_row(&blocks, 6, 100).is_none());
    }

    // ── block_is_foldable ────────────────────────────────────────────────

    #[test]
    fn finished_block_is_foldable() {
        assert!(block_is_foldable(&finished_block(0, 3, Some(0))));
        assert!(block_is_foldable(&finished_block(0, 3, Some(1))));
        assert!(block_is_foldable(&finished_block(0, 3, None)));
    }

    #[test]
    fn running_block_is_not_foldable() {
        // A running command has no end_row; a gutter click on it is a
        // no-op fold (it only focuses the pane).
        assert!(!block_is_foldable(&running_block(4)));
    }
}
