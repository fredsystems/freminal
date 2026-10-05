// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! OSC 9;4 (ConEmu-style) progress-state types (issue #507).
//!
//! `OSC 9 ; 4 ; s [ ; v ] ST` reports a program's progress (e.g. a build or
//! copy) so the terminal can surface it outside the scrollback — typically
//! as a taskbar/dock indicator. Per
//! <https://ghostty.org/docs/vt/osc/conemu>:
//!
//! | `s` | New state      | New value                          |
//! |-----|----------------|-------------------------------------|
//! | 0   | Inactive       | 0                                     |
//! | 1   | In progress    | `v`                                   |
//! | 2   | Error          | `v` when specified, otherwise unchanged |
//! | 3   | Indeterminate  | unchanged                             |
//! | 4   | Paused         | `v` when specified, otherwise unchanged |
//!
//! [`ProgressUpdate`] is the wire form produced by the parser — it carries
//! `Option<u8>` for the states whose value is "unchanged when omitted".
//! [`ProgressReport`] is the resolved, stateful result of applying successive
//! updates, which is what gets carried in `TerminalSnapshot`.

/// Which progress state a pane is in (OSC 9;4 `s`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProgressState {
    /// No progress is being reported (`s=0`, or never set).
    #[default]
    Inactive,
    /// A program is reporting active progress (`s=1`).
    InProgress,
    /// The last-reported operation errored (`s=2`).
    Error,
    /// Progress is happening but the percentage is unknown (`s=3`).
    Indeterminate,
    /// Progress is temporarily paused (`s=4`).
    Paused,
}

/// A parsed OSC 9;4 update, before it is applied to the pane's current state.
///
/// This is the wire form: it is what the parser produces, and deliberately
/// carries `Option<u8>` for the states whose value is "unchanged when
/// omitted" per the `ConEmu` spec table (see the module doc comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressUpdate {
    /// `s=0` — clear progress; value resets to 0.
    Clear,
    /// `s=1` — in progress, with a new percentage value.
    ///
    /// The spec does not define what an omitted `v` means for this state
    /// (unlike states 2 and 4, which explicitly say "unchanged"). Freminal's
    /// reading of this underspecified case: the parser maps an omitted `v`
    /// to `0` before constructing this variant, so `InProgress` always
    /// carries a concrete value here.
    InProgress(u8),
    /// `s=2` — error. `Some(v)` sets the value; `None` leaves it unchanged.
    Error(Option<u8>),
    /// `s=3` — indeterminate progress; value is always left unchanged.
    Indeterminate,
    /// `s=4` — paused. `Some(v)` sets the value; `None` leaves it unchanged.
    Paused(Option<u8>),
}

/// A pane's current progress, as resolved by applying successive
/// [`ProgressUpdate`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProgressReport {
    state: ProgressState,
    percent: u8,
}

impl ProgressReport {
    /// The current progress state.
    #[must_use]
    pub const fn state(&self) -> ProgressState {
        self.state
    }

    /// The current progress percentage (0-100).
    ///
    /// Meaningful only when `state()` is not [`ProgressState::Inactive`] or
    /// [`ProgressState::Indeterminate`], but always holds the
    /// last-resolved value regardless of state (see [`Self::apply`]'s
    /// value-retention rules).
    #[must_use]
    pub const fn percent(&self) -> u8 {
        self.percent
    }

    /// Apply a parsed [`ProgressUpdate`], resolving the "unchanged when
    /// omitted" rows of the OSC 9;4 spec table against the current value.
    pub fn apply(&mut self, update: ProgressUpdate) {
        match update {
            ProgressUpdate::Clear => {
                self.state = ProgressState::Inactive;
                self.percent = 0;
            }
            ProgressUpdate::InProgress(value) => {
                self.state = ProgressState::InProgress;
                self.percent = value.min(100);
            }
            ProgressUpdate::Error(value) => {
                self.state = ProgressState::Error;
                if let Some(value) = value {
                    self.percent = value.min(100);
                }
            }
            ProgressUpdate::Indeterminate => {
                self.state = ProgressState::Indeterminate;
                // Value is always left unchanged for this state.
            }
            ProgressUpdate::Paused(value) => {
                self.state = ProgressState::Paused;
                if let Some(value) = value {
                    self.percent = value.min(100);
                }
            }
        }
    }

    /// True when the pane is reporting progress at all (any state other
    /// than [`ProgressState::Inactive`]).
    #[must_use]
    pub const fn is_active(&self) -> bool {
        !matches!(self.state, ProgressState::Inactive)
    }
}

#[cfg(test)]
mod tests {
    use super::{ProgressReport, ProgressState, ProgressUpdate};

    #[test]
    fn default_is_inactive_zero() {
        let report = ProgressReport::default();
        assert_eq!(report.state(), ProgressState::Inactive);
        assert_eq!(report.percent(), 0);
        assert!(!report.is_active());
    }

    #[test]
    fn in_progress_sets_state_and_value() {
        let mut report = ProgressReport::default();
        report.apply(ProgressUpdate::InProgress(50));
        assert_eq!(report.state(), ProgressState::InProgress);
        assert_eq!(report.percent(), 50);
        assert!(report.is_active());
    }

    #[test]
    fn in_progress_clamps_above_100() {
        let mut report = ProgressReport::default();
        report.apply(ProgressUpdate::InProgress(255));
        assert_eq!(report.percent(), 100);
    }

    #[test]
    fn value_retention_across_state_transitions() {
        let mut report = ProgressReport::default();

        // Set 50% in progress.
        report.apply(ProgressUpdate::InProgress(50));
        assert_eq!(report.state(), ProgressState::InProgress);
        assert_eq!(report.percent(), 50);

        // Error(None) keeps the value, changes only the state.
        report.apply(ProgressUpdate::Error(None));
        assert_eq!(report.state(), ProgressState::Error);
        assert_eq!(report.percent(), 50);

        // Indeterminate keeps the value too.
        report.apply(ProgressUpdate::Indeterminate);
        assert_eq!(report.state(), ProgressState::Indeterminate);
        assert_eq!(report.percent(), 50);

        // Paused(None) keeps the value.
        report.apply(ProgressUpdate::Paused(None));
        assert_eq!(report.state(), ProgressState::Paused);
        assert_eq!(report.percent(), 50);

        // Clear resets to inactive/0.
        report.apply(ProgressUpdate::Clear);
        assert_eq!(report.state(), ProgressState::Inactive);
        assert_eq!(report.percent(), 0);
        assert!(!report.is_active());
    }

    #[test]
    fn error_with_explicit_value_sets_it() {
        let mut report = ProgressReport::default();
        report.apply(ProgressUpdate::InProgress(50));
        report.apply(ProgressUpdate::Error(Some(10)));
        assert_eq!(report.state(), ProgressState::Error);
        assert_eq!(report.percent(), 10);
    }

    #[test]
    fn error_clamps_explicit_value_above_100() {
        let mut report = ProgressReport::default();
        report.apply(ProgressUpdate::Error(Some(255)));
        assert_eq!(report.percent(), 100);
    }

    #[test]
    fn paused_with_explicit_value_sets_it() {
        let mut report = ProgressReport::default();
        report.apply(ProgressUpdate::InProgress(20));
        report.apply(ProgressUpdate::Paused(Some(75)));
        assert_eq!(report.state(), ProgressState::Paused);
        assert_eq!(report.percent(), 75);
    }

    #[test]
    fn paused_clamps_explicit_value_above_100() {
        let mut report = ProgressReport::default();
        report.apply(ProgressUpdate::Paused(Some(255)));
        assert_eq!(report.percent(), 100);
    }

    #[test]
    fn indeterminate_from_default_stays_at_zero() {
        let mut report = ProgressReport::default();
        report.apply(ProgressUpdate::Indeterminate);
        assert_eq!(report.state(), ProgressState::Indeterminate);
        assert_eq!(report.percent(), 0);
        assert!(report.is_active());
    }

    #[test]
    fn is_active_true_for_every_non_inactive_state() {
        for update in [
            ProgressUpdate::InProgress(0),
            ProgressUpdate::Error(None),
            ProgressUpdate::Indeterminate,
            ProgressUpdate::Paused(None),
        ] {
            let mut report = ProgressReport::default();
            report.apply(update);
            assert!(report.is_active(), "update {update:?} should be active");
        }
    }

    #[test]
    fn clear_from_any_state_returns_to_inactive() {
        for update in [
            ProgressUpdate::InProgress(10),
            ProgressUpdate::Error(Some(20)),
            ProgressUpdate::Indeterminate,
            ProgressUpdate::Paused(Some(30)),
        ] {
            let mut report = ProgressReport::default();
            report.apply(update);
            report.apply(ProgressUpdate::Clear);
            assert_eq!(report.state(), ProgressState::Inactive);
            assert_eq!(report.percent(), 0);
            assert!(!report.is_active());
        }
    }

    #[test]
    fn default_trait_matches_manual_default() {
        assert_eq!(
            ProgressReport::default(),
            ProgressReport {
                state: ProgressState::Inactive,
                percent: 0,
            }
        );
    }

    #[test]
    fn progress_state_default_is_inactive() {
        assert_eq!(ProgressState::default(), ProgressState::Inactive);
    }
}
