// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The cursor's per-frame blink clock and focus/appearance state for a pane.
//!
//! One concept: *when* a pane's cursor is in the visible half of its blink
//! cycle, and the combined per-frame derivation ([`frame_cursor_state`]) that
//! folds the blink clock together with the pane/window focus and the resolved
//! [`CursorAppearance`] that `show()` needs each frame. The *what* of the
//! cursor (how it is drawn for a given focus) is owned by
//! [`super::cursor_appearance`]; this module owns the clock and the per-frame
//! composition.
//!
//! Extracted verbatim from `widget.rs`; no behaviour change.

use super::cursor_appearance::{
    CursorAppearance, CursorAppearanceInputs, CursorFocus, CursorVisibility, EchoState,
    cursor_focus, resolve_cursor_appearance,
};
use super::input::PaneFocus;
use crate::gui::frame_drain::WindowFocus;
use crate::gui::renderer::CursorBlinkPhase;
use conv2::{ApproxFrom, RoundToZero};
use freminal_common::config::UnfocusedCursorStyle;
use tracing::error;

/// Compute the cursor blink phase (`true` = cursor visible) at `time`.
///
/// The phase toggles every `tick_seconds`. When `anchor` is `Some`, the phase
/// is measured relative to that activation time, so the first `tick_seconds`
/// after activation are always in the visible ("on") half — this makes a
/// freshly-activated pane's cursor appear immediately instead of inheriting
/// whichever half of the global cycle happens to be current. When `anchor` is
/// `None`, the global wall-clock phase is used.
///
/// A conversion failure (absurd `time`) is treated as "visible", matching the
/// pre-existing fallback: a shown cursor is always the safe default.
#[must_use]
fn cursor_blink_phase(time: f64, anchor: Option<f64>, tick_seconds: f64) -> bool {
    let blink_time = anchor.map_or(time, |a| time - a);
    match <i64 as ApproxFrom<f64, RoundToZero>>::approx_from((blink_time / tick_seconds).floor()) {
        Ok(ticks) => ticks % 2 == 0,
        Err(e) => {
            error!("Failed to convert blink ticks to i64: {e}");
            true
        }
    }
}

/// Whether the blink clock must be re-anchored because the cursor's focus
/// changed between frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlinkAnchorAction {
    /// The cursor regained focus: re-anchor so the blink starts in its
    /// visible half.
    Reanchor,
    /// No focus-driven re-anchor.
    Keep,
}

/// Decide whether a focus change requires re-anchoring the blink clock.
///
/// While a cursor is not [`CursorFocus::Focused`] it is drawn hollow/steady
/// with the blink phase pinned `On`, but the blink clock itself keeps
/// running. When focus returns (window refocus, or the pane becoming the
/// active one) the cursor turns solid and starts honouring the clock, which
/// may well be in its off half -- leaving no visible cursor for up to a blink
/// tick. Re-anchoring on the non-`Focused` -> `Focused` edge makes the first
/// focused frame blink-on, exactly as the activation reset does for a
/// newly-activated pane.
#[must_use]
const fn blink_anchor_action(previous: CursorFocus, now: CursorFocus) -> BlinkAnchorAction {
    match (previous, now) {
        (CursorFocus::InactivePane | CursorFocus::UnfocusedWindow, CursorFocus::Focused) => {
            BlinkAnchorAction::Reanchor
        }
        _ => BlinkAnchorAction::Keep,
    }
}

/// Half-period of the cursor blink cycle, in seconds.
const BLINK_TICK_SECONDS: f64 = 0.50;

/// Whether the GUI flagged this pane's blink clock for an activation
/// re-anchor (`ViewState::cursor_blink_reset_pending`).
///
/// A named enum rather than a `bool` so [`CursorStateInputs`] has no bare
/// flag; converted from the field at the `show()` boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActivationBlinkReset {
    /// The pane was just activated or revealed; re-anchor the blink clock.
    Pending,
    /// No activation re-anchor requested.
    NotPending,
}

impl ActivationBlinkReset {
    /// Build from the raw `ViewState::cursor_blink_reset_pending` flag.
    pub(super) const fn from_bool(pending: bool) -> Self {
        if pending {
            Self::Pending
        } else {
            Self::NotPending
        }
    }
}

/// The plain values `show()` has in hand that determine one frame's cursor
/// focus, appearance and blink phase.
pub(super) struct CursorStateInputs<'a> {
    /// Whether this pane is the active one.
    pub(super) pane_focus: PaneFocus,
    /// Whether the window has input focus.
    pub(super) window_focus: WindowFocus,
    /// The focus the cursor was last drawn with (`PaneRenderCache`).
    pub(super) previous_focus: CursorFocus,
    /// Whether an activation re-anchor is pending.
    pub(super) activation_reset: ActivationBlinkReset,
    /// Whether the application wants the cursor shown (`DECTCEM`).
    pub(super) snapshot_visible: CursorVisibility,
    /// Whether the pane is in echo-off mode.
    pub(super) echo: EchoState,
    /// The application's DECSCUSR style.
    pub(super) style: &'a freminal_common::cursor::CursorVisualStyle,
    /// The user's preference for unfocused cursors.
    pub(super) unfocused_style: UnfocusedCursorStyle,
    /// The egui input clock, in seconds.
    pub(super) time: f64,
    /// The pane's current blink anchor, if any.
    pub(super) anchor: Option<f64>,
}

/// One frame's cursor focus, appearance and blink clock, as derived by
/// [`frame_cursor_state`].
#[derive(Debug, Clone, PartialEq)]
pub(super) struct CursorStateOutcome {
    /// The combined pane/window focus the cursor is drawn with.
    pub(super) focus: CursorFocus,
    /// How the cursor is drawn, before fold/off-screen visibility is applied.
    pub(super) appearance: CursorAppearance,
    /// The RAW blink-clock phase. Pinning it for a cursor that does not blink
    /// is done by `evaluate_frame_dirty_state`, which returns the effective
    /// phase.
    pub(super) raw_blink: CursorBlinkPhase,
    /// The blink anchor to store back on the `ViewState`: `Some(time)` when
    /// this frame re-anchored, otherwise the input anchor unchanged.
    pub(super) anchor: Option<f64>,
}

/// Derive one frame's cursor focus, appearance and blink clock.
///
/// The blink anchor is re-set to `time` when either the GUI flagged an
/// activation reset or the cursor just regained focus
/// ([`blink_anchor_action`]); a pending activation reset is thereby consumed,
/// and the caller must clear its flag. The raw phase is then measured from the
/// (possibly new) anchor, so a freshly (re)focused or (re)activated pane's
/// first frame is blink-on.
///
/// Lives beside [`blink_anchor_action`] and [`cursor_blink_phase`] because
/// those own the *when* of the cursor (the blink clock); the *what* is
/// delegated to `cursor_appearance`'s [`cursor_focus`] and
/// [`resolve_cursor_appearance`], which stay the single definitions of the
/// focus and appearance rules.
#[must_use]
pub(super) fn frame_cursor_state(inputs: &CursorStateInputs<'_>) -> CursorStateOutcome {
    let focus = cursor_focus(inputs.pane_focus, inputs.window_focus);
    let anchor = if inputs.activation_reset == ActivationBlinkReset::Pending
        || blink_anchor_action(inputs.previous_focus, focus) == BlinkAnchorAction::Reanchor
    {
        Some(inputs.time)
    } else {
        inputs.anchor
    };
    let appearance = resolve_cursor_appearance(&CursorAppearanceInputs {
        snapshot_visible: inputs.snapshot_visible,
        echo: inputs.echo,
        focus,
        style: inputs.style.clone(),
        unfocused_style: inputs.unfocused_style,
    });
    let raw_blink = CursorBlinkPhase::from_blink_on(cursor_blink_phase(
        inputs.time,
        anchor,
        BLINK_TICK_SECONDS,
    ));
    CursorStateOutcome {
        focus,
        appearance,
        raw_blink,
        anchor,
    }
}

#[cfg(test)]
mod cursor_blink_phase_tests {
    use super::{BlinkAnchorAction, blink_anchor_action, cursor_blink_phase};
    use crate::gui::renderer::CursorBlinkPhase;
    use crate::gui::terminal::PaneRenderCache;
    use crate::gui::terminal::cursor_appearance::{CursorAppearance, CursorFocus};
    use crate::gui::terminal::frame_dirty::{CursorFrame, DrawnCursor};
    use freminal_common::buffer_states::cursor::CursorPos;

    const TICK: f64 = 0.50;

    #[test]
    fn global_phase_toggles_every_tick() {
        // No anchor -> global wall-clock phase; on for [0,0.5), off for
        // [0.5,1.0), on for [1.0,1.5), ...
        assert!(cursor_blink_phase(0.0, None, TICK), "t=0 on");
        assert!(cursor_blink_phase(0.25, None, TICK), "t=0.25 on");
        assert!(!cursor_blink_phase(0.5, None, TICK), "t=0.5 off");
        assert!(!cursor_blink_phase(0.75, None, TICK), "t=0.75 off");
        assert!(cursor_blink_phase(1.0, None, TICK), "t=1.0 on");
    }

    #[test]
    fn anchor_makes_cursor_visible_immediately_on_activation() {
        // The bug: activating a pane at a "global-off" moment (t=0.7) would
        // leave its cursor hidden until the global phase flipped. With an
        // anchor at the activation time, the phase re-bases so the first
        // half-cycle after activation is visible regardless of global phase.
        let activation = 0.7; // global phase here is "off"
        assert!(
            !cursor_blink_phase(activation, None, TICK),
            "global off at 0.7"
        );
        // Anchored: measured from activation, so t-anchor in [0,0.5) -> on.
        assert!(
            cursor_blink_phase(activation, Some(activation), TICK),
            "anchored on at activation"
        );
        assert!(
            cursor_blink_phase(activation + 0.4, Some(activation), TICK),
            "anchored still on 0.4s after activation"
        );
    }

    #[test]
    fn anchored_phase_toggles_relative_to_activation() {
        let anchor = 0.7;
        // 0.5s after activation -> first "off" half.
        assert!(!cursor_blink_phase(anchor + 0.5, Some(anchor), TICK));
        // 1.0s after activation -> "on" again.
        assert!(cursor_blink_phase(anchor + 1.0, Some(anchor), TICK));
    }

    /// Regaining focus (window refocus, or the pane becoming active) from
    /// either non-focused state re-anchors the blink clock.
    #[test]
    fn focus_regain_reanchors_the_blink() {
        for previous in [CursorFocus::InactivePane, CursorFocus::UnfocusedWindow] {
            assert_eq!(
                blink_anchor_action(previous, CursorFocus::Focused),
                BlinkAnchorAction::Reanchor,
                "{previous:?} -> Focused"
            );
        }
    }

    /// Every other transition leaves the anchor alone: staying focused must
    /// not restart the blink each frame, and losing focus has no phase to
    /// restore (the phase is pinned on while not focused).
    #[test]
    fn other_focus_transitions_keep_the_anchor() {
        let all = [
            CursorFocus::Focused,
            CursorFocus::InactivePane,
            CursorFocus::UnfocusedWindow,
        ];
        for previous in all {
            for now in all {
                if now == CursorFocus::Focused && previous != CursorFocus::Focused {
                    continue;
                }
                assert_eq!(
                    blink_anchor_action(previous, now),
                    BlinkAnchorAction::Keep,
                    "{previous:?} -> {now:?}"
                );
            }
        }
    }

    /// The point of the re-anchor: refocusing while the global clock is in
    /// its off half would otherwise show no cursor for up to a tick; anchored
    /// at the regain time the first focused frame is blink-on.
    #[test]
    fn reanchoring_on_regain_makes_the_first_focused_frame_visible() {
        let regain_time = 0.7; // global phase here is "off"
        assert!(!cursor_blink_phase(regain_time, None, TICK));
        assert_eq!(
            blink_anchor_action(CursorFocus::UnfocusedWindow, CursorFocus::Focused),
            BlinkAnchorAction::Reanchor
        );
        assert!(cursor_blink_phase(regain_time, Some(regain_time), TICK));
    }

    /// Task 127.C2: the focus baseline is held back on a skipped frame, so a
    /// regain that happened while skipped is still seen as a non-`Focused` ->
    /// `Focused` edge on the first drawn frame, and the blink still
    /// re-anchors there.
    #[test]
    fn regain_during_a_skipped_frame_still_reanchors_on_the_first_drawn_frame() {
        let mut cache = PaneRenderCache::new();
        // The last drawn frame showed the cursor unfocused.
        cache.record_cursor_frame(CursorFrame::Drawn(DrawnCursor {
            blink_phase: CursorBlinkPhase::On,
            focus: CursorFocus::UnfocusedWindow,
            pos: CursorPos::default(),
            screen_row: Some(0),
            appearance: CursorAppearance::Hollow,
            color_override: None,
        }));

        // Focus returns during a skipped frame, which re-anchors on its own
        // (the cursor is not visible yet) but does not advance the baseline.
        assert_eq!(
            blink_anchor_action(cache.previous_cursor_focus, CursorFocus::Focused),
            BlinkAnchorAction::Reanchor
        );
        cache.record_cursor_frame(CursorFrame::Skipped);

        // The first drawn focused frame must therefore still see the regain.
        assert_eq!(
            blink_anchor_action(cache.previous_cursor_focus, CursorFocus::Focused),
            BlinkAnchorAction::Reanchor
        );
    }
}

#[cfg(test)]
mod frame_cursor_state_tests {
    //! Tests for [`frame_cursor_state`], the pure per-frame cursor derivation
    //! `show()` calls: focus, appearance, blink anchor and raw blink phase.

    use super::{ActivationBlinkReset, CursorStateInputs, CursorStateOutcome, frame_cursor_state};
    use crate::gui::frame_drain::WindowFocus;
    use crate::gui::renderer::CursorBlinkPhase;
    use crate::gui::terminal::cursor_appearance::{
        CursorAppearance, CursorFocus, CursorVisibility, EchoState,
    };
    use crate::gui::terminal::input::PaneFocus;
    use freminal_common::config::UnfocusedCursorStyle;
    use freminal_common::cursor::CursorVisualStyle;

    /// A time at which the un-anchored global blink clock is in its OFF half
    /// (`0.7 / 0.5 = 1.4 -> tick 1 -> off`).
    const OFF_HALF_TIME: f64 = 0.7;

    const BLINK: CursorVisualStyle = CursorVisualStyle::BlockCursorBlink;

    /// A focused, visible, normal-echo frame at [`OFF_HALF_TIME`] with no
    /// anchor and no pending reset; each test overrides what it exercises.
    fn base(style: &CursorVisualStyle) -> CursorStateInputs<'_> {
        CursorStateInputs {
            pane_focus: PaneFocus::Active,
            window_focus: WindowFocus::Focused,
            previous_focus: CursorFocus::Focused,
            activation_reset: ActivationBlinkReset::NotPending,
            snapshot_visible: CursorVisibility::Shown,
            echo: EchoState::Normal,
            style,
            unfocused_style: UnfocusedCursorStyle::Hollow,
            time: OFF_HALF_TIME,
            anchor: None,
        }
    }

    fn state(inputs: &CursorStateInputs<'_>) -> CursorStateOutcome {
        frame_cursor_state(inputs)
    }

    /// Control: with no re-anchor trigger the un-anchored clock is in its off
    /// half, so this scenario really does depend on the re-anchor below.
    #[test]
    fn without_a_trigger_the_global_clock_is_in_its_off_half() {
        let got = state(&base(&BLINK));
        assert_eq!(got.anchor, None);
        assert_eq!(got.raw_blink, CursorBlinkPhase::Off);
        assert_eq!(got.focus, CursorFocus::Focused);
    }

    #[test]
    fn regaining_window_focus_reanchors_so_the_first_focused_frame_is_on() {
        let got = state(&CursorStateInputs {
            previous_focus: CursorFocus::UnfocusedWindow,
            ..base(&BLINK)
        });
        assert_eq!(got.anchor, Some(OFF_HALF_TIME));
        assert_eq!(got.raw_blink, CursorBlinkPhase::On);
        assert_eq!(got.appearance, CursorAppearance::Solid(BLINK));
    }

    #[test]
    fn becoming_the_active_pane_reanchors() {
        let got = state(&CursorStateInputs {
            previous_focus: CursorFocus::InactivePane,
            ..base(&BLINK)
        });
        assert_eq!(got.anchor, Some(OFF_HALF_TIME));
        assert_eq!(got.raw_blink, CursorBlinkPhase::On);
    }

    #[test]
    fn a_pending_activation_reset_reanchors_even_when_focus_is_unchanged() {
        let got = state(&CursorStateInputs {
            activation_reset: ActivationBlinkReset::Pending,
            anchor: Some(0.0),
            ..base(&BLINK)
        });
        assert_eq!(got.anchor, Some(OFF_HALF_TIME));
        assert_eq!(got.raw_blink, CursorBlinkPhase::On);
    }

    /// Staying focused must not restart the blink every frame: the existing
    /// anchor is kept and the phase is measured from it.
    #[test]
    fn staying_focused_keeps_the_anchor_and_follows_its_clock() {
        // Anchored at 0.0, 0.7s in is the second (off) half-period.
        let got = state(&CursorStateInputs {
            anchor: Some(0.0),
            ..base(&BLINK)
        });
        assert_eq!(got.anchor, Some(0.0));
        assert_eq!(got.raw_blink, CursorBlinkPhase::Off);

        // And 1.1s in it is on again.
        let got = state(&CursorStateInputs {
            anchor: Some(0.0),
            time: 1.1,
            ..base(&BLINK)
        });
        assert_eq!(got.anchor, Some(0.0));
        assert_eq!(got.raw_blink, CursorBlinkPhase::On);
    }

    /// Losing focus never re-anchors (there is no phase to restore: the
    /// cursor is steady while not focused).
    #[test]
    fn losing_focus_keeps_the_anchor() {
        let got = state(&CursorStateInputs {
            window_focus: WindowFocus::Unfocused,
            previous_focus: CursorFocus::Focused,
            anchor: Some(0.1),
            ..base(&BLINK)
        });
        assert_eq!(got.anchor, Some(0.1));
        assert_eq!(got.focus, CursorFocus::UnfocusedWindow);
    }

    #[test]
    fn inactive_pane_resolves_to_a_hollow_cursor() {
        let got = state(&CursorStateInputs {
            pane_focus: PaneFocus::Inactive,
            previous_focus: CursorFocus::InactivePane,
            ..base(&BLINK)
        });
        assert_eq!(got.focus, CursorFocus::InactivePane);
        assert_eq!(got.appearance, CursorAppearance::Hollow);
    }

    #[test]
    fn unfocused_window_resolves_to_a_hollow_cursor_even_for_the_active_pane() {
        let got = state(&CursorStateInputs {
            window_focus: WindowFocus::Unfocused,
            previous_focus: CursorFocus::UnfocusedWindow,
            ..base(&BLINK)
        });
        assert_eq!(got.focus, CursorFocus::UnfocusedWindow);
        assert_eq!(got.appearance, CursorAppearance::Hollow);
    }

    /// The user's unfocused preference reaches the resolution: `Unchanged`
    /// keeps the shape but steady, `Hidden` draws nothing.
    #[test]
    fn unfocused_preference_is_applied_to_a_non_focused_cursor() {
        let unchanged = state(&CursorStateInputs {
            pane_focus: PaneFocus::Inactive,
            previous_focus: CursorFocus::InactivePane,
            unfocused_style: UnfocusedCursorStyle::Unchanged,
            ..base(&BLINK)
        });
        assert_eq!(
            unchanged.appearance,
            CursorAppearance::Solid(CursorVisualStyle::BlockCursorSteady)
        );
        let hidden = state(&CursorStateInputs {
            pane_focus: PaneFocus::Inactive,
            previous_focus: CursorFocus::InactivePane,
            unfocused_style: UnfocusedCursorStyle::Hidden,
            ..base(&BLINK)
        });
        assert_eq!(hidden.appearance, CursorAppearance::Hidden);
    }

    #[test]
    fn dectcem_hidden_is_hidden_in_every_focus_state() {
        for (pane_focus, window_focus, previous_focus) in [
            (
                PaneFocus::Active,
                WindowFocus::Focused,
                CursorFocus::Focused,
            ),
            (
                PaneFocus::Inactive,
                WindowFocus::Focused,
                CursorFocus::InactivePane,
            ),
            (
                PaneFocus::Active,
                WindowFocus::Unfocused,
                CursorFocus::UnfocusedWindow,
            ),
        ] {
            let got = state(&CursorStateInputs {
                pane_focus,
                window_focus,
                previous_focus,
                snapshot_visible: CursorVisibility::Hidden,
                ..base(&BLINK)
            });
            assert_eq!(
                got.appearance,
                CursorAppearance::Hidden,
                "{pane_focus:?} / {window_focus:?}"
            );
        }
    }

    #[test]
    fn echo_off_is_hidden() {
        let got = state(&CursorStateInputs {
            echo: EchoState::EchoOff,
            ..base(&BLINK)
        });
        assert_eq!(got.appearance, CursorAppearance::Hidden);
    }

    /// The returned phase is the RAW clock: it is not pinned for a non-focused
    /// cursor here (that is `evaluate_frame_dirty_state`'s job).
    #[test]
    fn the_blink_phase_is_the_raw_clock_even_when_not_focused() {
        let got = state(&CursorStateInputs {
            pane_focus: PaneFocus::Inactive,
            previous_focus: CursorFocus::InactivePane,
            ..base(&BLINK)
        });
        assert_eq!(got.raw_blink, CursorBlinkPhase::Off);
    }
}
