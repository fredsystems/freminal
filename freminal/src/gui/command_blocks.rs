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
//!
//! Kept as a standalone module so the formatter can be unit-tested
//! without an egui or GPU context.

use std::time::Duration;

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
    fn never_contains_whitespace() {
        // Right-alignment over a single rendered row relies on the
        // label being a single non-wrapping token.
        for secs in [1_u64, 30, 59, 60, 61, 599, 3_599, 3_600, 3_601, 86_400] {
            let s = format_command_duration(Duration::from_secs(secs));
            assert!(!s.contains(' '), "duration '{s}' contains whitespace");
        }
    }
}
