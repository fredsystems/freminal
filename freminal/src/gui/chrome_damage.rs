// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Chrome-damage decision (#436.3), extracted for reuse and unit testing.
//!
//! This module produces [`freminal_windowing::ChromeDamage`] — the #436
//! decision input for whether static chrome (menu bar, tab bar, pane
//! borders, broadcast label, and every overlay: modals/toasts/tooltips/
//! popups) changed on the frame just rendered. It implements two of the
//! three signal families from the design (`gh issue view 436`):
//!
//! - **§3.3 "app-level chrome-change signals"**: [`ChromeSignals`] bundles
//!   the individual per-frame booleans the app already computes (or can
//!   cheaply compute) for each row of the §3.3 table. [`ChromeSignals::any_fired`]
//!   ORs them together.
//! - **§3.5 "self-dismissal settle rule"** (adversarial finding 1): a
//!   dismissible chrome element (toast, About, Welcome, paste/broadcast/
//!   close-guard dialogs, save-layout prompt) can vanish *during its own
//!   render pass* (the proven toast hazard: it draws itself, then
//!   `retain`-removes itself as expired, then requests no further repaint
//!   because the stack is now empty). Comparing this frame's post-render
//!   state to *last frame's* post-render state shows no delta and misses
//!   the transition entirely. [`DismissiblePresence`] + [`dismissible_presence_transitioned`]
//!   implement the fix: presence is sampled *before* any dismissible
//!   element's `.show(ctx)` call this frame and *after* all of them, and
//!   the two samples are diffed — catching the transition even when the
//!   cross-frame (last-frame-vs-this-frame) comparison would not.
//!
//! **NOT covered here** (explicitly deferred to their own subtasks, per the
//! design): §3.1 (egui's own `repaint_delay`) and §3.2 (chrome-affecting
//! input events, including the region-aware pointer-motion gate) are
//! `run_frame`-side signals owned by 436.4/436.9. The §5.2 font-atlas-resize
//! retroactive-FULL fallback is also 436.4/436.6 — it can only be detected
//! *after* the terminal band tessellates, which does not exist as a
//! separate pass yet. This module produces app-level signals only; it does
//! not yet feed into `run_frame` (that wiring is 436.4).
//!
//! Like [`super::frame_damage`], every function here is pure — no `self`,
//! no `egui` context, no window state — so it is directly unit-testable.

use crate::gui::panes::PaneId;
use crate::gui::tabs::TabId;

/// Number of frames after window creation during which chrome is always
/// forced `Changed` (#436 §7 warm-up). The font atlas, layout, and
/// `PanelState` id-maps settle over the first 1-2 frames; giving a small
/// margin (3) is cheap insurance against a REPLAY being permitted before
/// steady state.
pub const WARMUP_FRAMES: u32 = 3;

/// Everything the app measured this frame that bears on whether static
/// chrome changed (#436 §3.3). Each field maps to a row of the §3.3 table
/// that the app can compute without any `run_frame`-side state; see the
/// module doc for what is deliberately NOT here (§3.1/§3.2 egui-side
/// signals, §5.2 font-atlas resize).
///
/// All fields independently force `Changed` when true — see
/// [`ChromeSignals::any_fired`]. Each is an unrelated, independently-sourced
/// per-frame observation (not a state machine), which is why this is a flat
/// bag of bools rather than an enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct ChromeSignals {
    /// Any menu/dropdown, modal/dialog, or tab-rename editor open
    /// (`ui_overlay_open` in `app_impl.rs`, itself `any_menu_open ||
    /// pending_save_layout.is_some() || about_window_open ||
    /// welcome.is_open() || renaming_tab.is_some() || paste_dialog.is_open()
    /// || broadcast_dialog.is_open() || close_dialog.is_open()`).
    pub any_overlay_open: bool,
    /// Theme, profile, or background-opacity change (`style_cache` miss).
    pub style_changed: bool,
    /// The active pane changed (pane switch within a tab, or a tab switch)
    /// — moves the active-pane border highlight.
    pub active_pane_changed: bool,
    /// Tab count, tab-id-list, or active-tab id changed vs. last frame.
    pub tab_set_changed: bool,
    /// Any tab's resolved display name changed vs. last frame (rename, OSC
    /// title, or a `TabTitlePolicy` recombination of the two).
    pub tab_title_changed: bool,
    /// The active tab's rendered pane set (split/close) or zoom state
    /// changed vs. last frame — pane borders move/appear/disappear.
    pub pane_layout_changed: bool,
    /// The active tab's broadcast-input flag changed vs. last frame — the
    /// broadcast label and border tint appear/disappear/retint.
    pub broadcast_state_changed: bool,
    /// A window-post-process shader is active or has a pending
    /// (re)compile (`shader_recomposites` in `app_impl.rs`) — it
    /// recomposites the whole window every frame, redrawing chrome on top.
    pub shader_active: bool,
    /// Any rendered pane has an active bell flash.
    pub bell_active: bool,
    /// Any toast is currently on the stack. Unlike the other rows, this
    /// does not by itself prove a *transition* — it is the "stays forced
    /// while visible" half; the *transition* half (added/expired) is
    /// covered separately by [`DismissiblePresence`]/§3.5, mirroring how
    /// `toast_active` also forces `FrameDamage::Full` every frame it is up
    /// in `decide_frame_damage` (#435) for the same reason (a toast's
    /// hover/dismiss hit-test needs live input testing, not stale cache).
    pub toast_active: bool,
    /// The window's inner size (physical pixels) changed vs. last frame.
    pub size_changed: bool,
    /// `pixels_per_point` (DPI/scale-factor) changed vs. last frame.
    pub ppp_changed: bool,
    /// The window gained or lost OS focus this frame — affects the active
    /// pane's border/cursor style.
    pub focus_changed: bool,
    /// Still within the first [`WARMUP_FRAMES`] frames since window
    /// creation (#436 §7) — force `Changed` unconditionally until steady
    /// state.
    pub warming_up: bool,
    /// Any rendered pane has an open overlay that paints ABOVE the terminal
    /// band: the `Order::Foreground` right-click context menu, in-terminal
    /// search bar, or command-history palette, OR the `Order::Tooltip`
    /// URL-hover tooltip. These all paint outside the captured terminal-band
    /// range (TAIL chrome), so a REPLAY frame would otherwise discard their
    /// freshly-built shapes and repaint the stale cached tail from before the
    /// overlay opened — making the open overlay vanish or ghost. Issue #436 §1
    /// names context-menu/command-history and the URL tooltip as chrome that
    /// must be covered. (The name predates the tooltip addition; it covers all
    /// above-band per-pane terminal overlays, not only `Order::Foreground`.)
    pub foreground_overlay_open: bool,
}

impl ChromeSignals {
    /// Whether any individual §3.3 signal fired this frame. A `true` from
    /// any field forces the frame `Changed`; only when *every* field is
    /// `false` does this contribute nothing to the decision.
    const fn any_fired(&self) -> bool {
        self.any_overlay_open
            || self.style_changed
            || self.active_pane_changed
            || self.tab_set_changed
            || self.tab_title_changed
            || self.pane_layout_changed
            || self.broadcast_state_changed
            || self.shader_active
            || self.bell_active
            || self.toast_active
            || self.size_changed
            || self.ppp_changed
            || self.focus_changed
            || self.warming_up
            || self.foreground_overlay_open
    }
}

/// Presence of each dismissible chrome element, sampled once per frame.
///
/// Field order/identity is stable frame-to-frame (it is a fixed struct, not
/// a dynamic collection), which is what makes a plain `PartialEq` diff
/// ([`dismissible_presence_transitioned`]) a correct membership-change test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct DismissiblePresence {
    /// The "About Freminal" dialog (`FreminalGui::about_window_open`).
    pub about: bool,
    /// The first-run welcome overlay (`FreminalGui::welcome`).
    pub welcome: bool,
    /// The smart-paste-guard confirmation dialog (`PerWindowState::paste_dialog`).
    pub paste_dialog: bool,
    /// The broadcast-input confirmation dialog (`PerWindowState::broadcast_dialog`).
    pub broadcast_dialog: bool,
    /// The close-on-running-command guard dialog (`PerWindowState::close_dialog`).
    pub close_dialog: bool,
    /// The floating "Save Layout" name-entry prompt (`FreminalGui::pending_save_layout`).
    pub save_layout_prompt: bool,
    /// Whether the shared toast stack is non-empty (`FreminalGui::toasts`).
    pub any_toast: bool,
}

/// #436 §3.5 self-dismissal settle rule (adversarial finding 1): a
/// dismissible element transitioning present<->absent must force THIS frame
/// AND the next frame FULL, so the cache is never rebuilt from the frame that
/// still contains a vanishing element. Returns true if `prev != current`
/// (any element changed presence this frame).
///
/// The caller is responsible for calling this twice per frame and OR-ing the
/// results — once for the before-`.show()`-vs-after-`.show()` comparison
/// (which is the one that catches the toast intra-frame self-dismissal
/// hazard) and once for the after-this-frame-vs-after-last-frame comparison
/// (which catches an element opened/closed by something other than its own
/// `.show()`, e.g. a menu action). See the finding-1 test below for why the
/// intra-frame comparison is load-bearing and the cross-frame one alone is
/// not sufficient.
pub fn dismissible_presence_transitioned(
    prev: DismissiblePresence,
    current: DismissiblePresence,
) -> bool {
    prev != current
}

/// Per-tab/pane snapshot used to detect the §3.3 "tab set changed" / "tab
/// title changed" / "pane layout changed" / "broadcast state changed" rows
/// between frames. Captured once per frame from data the app already has;
/// comparing two snapshots (via [`diff_tab_snapshots`]) yields the four
/// independent booleans those rows need.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChromeTabSnapshot {
    /// Tab ids in display order — captures add/remove/reorder.
    pub tab_ids: Vec<TabId>,
    /// The active tab's id — captures a tab switch even when the id list
    /// and order are otherwise unchanged.
    pub active_tab_id: Option<TabId>,
    /// Each tab's resolved display name (under the active `TabTitlePolicy`),
    /// in the same order as `tab_ids` — captures rename / OSC-title changes.
    pub tab_titles: Vec<String>,
    /// Leaf pane ids actually laid out this frame, in layout order — the
    /// same set/order `pane_layout` produces. Captures split/close.
    pub pane_ids: Vec<PaneId>,
    /// The active tab's zoomed pane, if any — captures zoom enter/exit.
    pub zoomed_pane: Option<PaneId>,
    /// The active tab's broadcast-input flag.
    pub broadcast_input: bool,
}

/// Result of comparing two [`ChromeTabSnapshot`]s: one boolean per §3.3 row
/// this snapshot covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
// Four independent §3.3 table rows, each an unrelated per-frame observation
// (not a state machine) -- same rationale as `ChromeSignals`/`DismissiblePresence`.
#[allow(clippy::struct_excessive_bools)]
pub struct ChromeTabSnapshotDiff {
    pub tab_set_changed: bool,
    pub tab_title_changed: bool,
    pub pane_layout_changed: bool,
    pub broadcast_state_changed: bool,
}

/// Pure diff of two [`ChromeTabSnapshot`]s into the four §3.3 booleans it
/// covers. See [`ChromeTabSnapshotDiff`] for the field-to-row mapping.
pub fn diff_tab_snapshots(
    prev: &ChromeTabSnapshot,
    current: &ChromeTabSnapshot,
) -> ChromeTabSnapshotDiff {
    ChromeTabSnapshotDiff {
        tab_set_changed: prev.tab_ids != current.tab_ids
            || prev.active_tab_id != current.active_tab_id,
        tab_title_changed: prev.tab_titles != current.tab_titles,
        pane_layout_changed: prev.pane_ids != current.pane_ids
            || prev.zoomed_pane != current.zoomed_pane,
        broadcast_state_changed: prev.broadcast_input != current.broadcast_input,
    }
}

/// Decide this frame's [`freminal_windowing::ChromeDamage`] (#436 §3.3 +
/// §3.5). Returns [`freminal_windowing::ChromeDamage::Changed`] if ANY §3.3
/// signal fired ([`ChromeSignals::any_fired`]) OR a dismissible element
/// transitioned this frame OR the previous frame flagged a pending settle
/// frame; [`freminal_windowing::ChromeDamage::Unchanged`] only when none of
/// those fired.
///
/// `presence_transitioned` is the OR of both [`dismissible_presence_transitioned`]
/// comparisons the caller is expected to make (intra-frame before/after, and
/// cross-frame after-vs-last-frame) — see that function's doc.
///
/// `settle_frame_pending` is the **other** half of the §3.5 settle rule: a
/// transition forces BOTH this frame and the NEXT frame `Changed`. The
/// caller carries this by remembering "a transition happened last frame"
/// (e.g. a `chrome_settle_pending: bool` field on its per-window state,
/// simply reassigned to this frame's `presence_transitioned` value at the
/// end of every frame — no separate reset step is needed since the flag is
/// always freshly recomputed, never accumulated) and feeding it in here as
/// `settle_frame_pending` on the *next* frame.
///
/// Pure: no `self`/`egui`/window state, so directly unit-testable.
pub const fn decide_chrome_damage(
    signals: &ChromeSignals,
    presence_transitioned: bool,
    settle_frame_pending: bool,
) -> freminal_windowing::ChromeDamage {
    if signals.any_fired() || presence_transitioned || settle_frame_pending {
        freminal_windowing::ChromeDamage::Changed
    } else {
        freminal_windowing::ChromeDamage::Unchanged
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChromeSignals, ChromeTabSnapshot, DismissiblePresence, decide_chrome_damage,
        diff_tab_snapshots, dismissible_presence_transitioned,
    };
    use crate::gui::panes::{PaneId, PaneIdGenerator};
    use crate::gui::tabs::TabId;
    use freminal_windowing::ChromeDamage;

    /// A `PaneId` distinct from [`PaneId::first`], for tests that need two
    /// different pane identities. `PaneId`'s inner field is private outside
    /// its module, so a second id must come from a generator.
    fn second_pane_id() -> PaneId {
        PaneIdGenerator::new(1).next_id()
    }

    #[test]
    fn no_signals_no_transition_no_pending_is_unchanged() {
        let signals = ChromeSignals::default();
        assert_eq!(
            decide_chrome_damage(&signals, false, false),
            ChromeDamage::Unchanged
        );
    }

    /// Table test: every individual `ChromeSignals` field, set alone (all
    /// others false), forces `Changed`. Exercises every §3.3 row this module
    /// covers.
    #[test]
    fn each_signal_field_alone_forces_changed() {
        type Setter = fn(&mut ChromeSignals);
        let setters: [(&str, Setter); 15] = [
            ("any_overlay_open", |s| s.any_overlay_open = true),
            ("style_changed", |s| s.style_changed = true),
            ("active_pane_changed", |s| s.active_pane_changed = true),
            ("tab_set_changed", |s| s.tab_set_changed = true),
            ("tab_title_changed", |s| s.tab_title_changed = true),
            ("pane_layout_changed", |s| s.pane_layout_changed = true),
            ("broadcast_state_changed", |s| {
                s.broadcast_state_changed = true;
            }),
            ("shader_active", |s| s.shader_active = true),
            ("bell_active", |s| s.bell_active = true),
            ("toast_active", |s| s.toast_active = true),
            ("size_changed", |s| s.size_changed = true),
            ("ppp_changed", |s| s.ppp_changed = true),
            ("focus_changed", |s| s.focus_changed = true),
            ("warming_up", |s| s.warming_up = true),
            ("foreground_overlay_open", |s| {
                s.foreground_overlay_open = true;
            }),
        ];

        for (name, set) in setters {
            let mut signals = ChromeSignals::default();
            set(&mut signals);
            assert_eq!(
                decide_chrome_damage(&signals, false, false),
                ChromeDamage::Changed,
                "expected field `{name}` alone to force Changed"
            );
        }
    }

    /// Regression test for the #436.4b defect: an open per-pane
    /// `Order::Foreground` overlay (context menu, in-terminal search bar, or
    /// command-history palette) must alone force `Changed`, so a REPLAY
    /// frame never discards its freshly-built shapes in favor of the stale
    /// cached tail chrome from before the overlay opened. The table test
    /// above also covers this row; this test documents the fix intent
    /// explicitly since it is a targeted regression fix rather than a
    /// generic table entry.
    #[test]
    fn foreground_overlay_open_alone_forces_changed() {
        let signals = ChromeSignals {
            foreground_overlay_open: true,
            ..ChromeSignals::default()
        };
        assert_eq!(
            decide_chrome_damage(&signals, false, false),
            ChromeDamage::Changed,
            "an open context-menu/search-bar/command-history overlay must force Changed"
        );
    }

    #[test]
    fn presence_transitioned_forces_changed_with_no_other_signals() {
        let signals = ChromeSignals::default();
        assert_eq!(
            decide_chrome_damage(&signals, true, false),
            ChromeDamage::Changed
        );
    }

    #[test]
    fn settle_frame_pending_forces_changed_with_no_other_signals() {
        // This is the "+ next frame" half of the §3.5 settle rule: even
        // with nothing else happening this frame, a pending settle from a
        // transition on the PREVIOUS frame still forces Changed.
        let signals = ChromeSignals::default();
        assert_eq!(
            decide_chrome_damage(&signals, false, true),
            ChromeDamage::Changed
        );
    }

    #[test]
    fn dismissible_presence_equal_is_not_transitioned() {
        let a = DismissiblePresence::default();
        let b = DismissiblePresence::default();
        assert!(!dismissible_presence_transitioned(a, b));
    }

    /// Table test: each individual `DismissiblePresence` field, differing
    /// alone between `prev` and `current`, is detected as a transition.
    #[test]
    fn each_dismissible_field_differing_alone_is_transitioned() {
        let base = DismissiblePresence::default();

        let variants: Vec<(&str, DismissiblePresence)> = vec![
            (
                "about",
                DismissiblePresence {
                    about: true,
                    ..base
                },
            ),
            (
                "welcome",
                DismissiblePresence {
                    welcome: true,
                    ..base
                },
            ),
            (
                "paste_dialog",
                DismissiblePresence {
                    paste_dialog: true,
                    ..base
                },
            ),
            (
                "broadcast_dialog",
                DismissiblePresence {
                    broadcast_dialog: true,
                    ..base
                },
            ),
            (
                "close_dialog",
                DismissiblePresence {
                    close_dialog: true,
                    ..base
                },
            ),
            (
                "save_layout_prompt",
                DismissiblePresence {
                    save_layout_prompt: true,
                    ..base
                },
            ),
            (
                "any_toast",
                DismissiblePresence {
                    any_toast: true,
                    ..base
                },
            ),
        ];

        for (name, changed) in variants {
            assert!(
                dismissible_presence_transitioned(base, changed),
                "expected field `{name}` differing alone to be a transition"
            );
        }
    }

    /// Models the app-side §3.5 state machine described in
    /// `decide_chrome_damage`'s doc: `chrome_settle_pending` is simply
    /// reassigned to this frame's `presence_transitioned` at the end of
    /// every frame (no separate "reset" call). Verifies the full sequence:
    /// a transition on frame N forces N AND N+1 `Changed`, then N+2 (with
    /// nothing else happening) is `Unchanged`.
    #[test]
    fn settle_sequence_forces_two_full_frames_after_a_transition() {
        let no_signals = ChromeSignals::default();
        let mut settle_pending = false;

        // Frame N: a presence transition happens (e.g. a dialog closes).
        let transitioned_n = true;
        let decision_n = decide_chrome_damage(&no_signals, transitioned_n, settle_pending);
        assert_eq!(decision_n, ChromeDamage::Changed, "frame N must be Changed");
        // App-side: carry this frame's transition into the next frame's
        // pending flag.
        settle_pending = transitioned_n;

        // Frame N+1: no new transition, but the settle-pending flag from N
        // alone must force Changed.
        let transitioned_n1 = false;
        let decision_n1 = decide_chrome_damage(&no_signals, transitioned_n1, settle_pending);
        assert_eq!(
            decision_n1,
            ChromeDamage::Changed,
            "frame N+1 must be Changed (settle frame)"
        );
        settle_pending = transitioned_n1;

        // Frame N+2: nothing happening at all -> Unchanged.
        let transitioned_n2 = false;
        let decision_n2 = decide_chrome_damage(&no_signals, transitioned_n2, settle_pending);
        assert_eq!(
            decision_n2,
            ChromeDamage::Unchanged,
            "frame N+2 must be Unchanged once the settle frame has passed"
        );
    }

    /// **Mandatory finding-1 test.** Reproduces the exact toast hazard
    /// verbatim from the design: a toast is present when its `.show(ctx)`
    /// begins on frame N, expires and `retain`-removes itself DURING that
    /// same call, and — because the stack is now empty — the toast stack
    /// requests no further repaint. The PREVIOUS frame's post-`.show()`
    /// state was *already* toast-absent (nothing distinguishes frame N-1's
    /// end state from frame N's end state), so a naive comparison against
    /// only last frame's post-mutation state sees NO delta and would wrongly
    /// permit a REPLAY that keeps painting the (now-ghost) toast forever.
    ///
    /// This test asserts that the naive cross-frame comparison indeed misses
    /// it (documenting the exact bug this subtask exists to prevent) and
    /// that the mandatory before/after-WITHIN-FRAME comparison catches it —
    /// i.e. if the before/after-within-frame diff were removed and only the
    /// cross-frame diff were fed into `decide_chrome_damage`, frame N would
    /// wrongly compute `Unchanged` instead of `Changed`.
    #[test]
    fn finding_1_toast_self_dismissal_ghost_is_caught_by_before_after_diff() {
        // Frame N: the toast is present right before its `.show(ctx)` call.
        let before = DismissiblePresence {
            any_toast: true,
            ..DismissiblePresence::default()
        };
        // ...but it expired and removed itself DURING that same `.show()`
        // call (toast.rs: draws, `retain`-removes the expired entry, then
        // only `request_repaint_after`s if the stack is still non-empty).
        let after = DismissiblePresence {
            any_toast: false,
            ..DismissiblePresence::default()
        };
        // Last frame's post-`.show()` state was ALSO toast-absent (the toast
        // was created and fully expired within frame N itself, or simply
        // frame N-1 ended with no toast visible either way).
        let prev_frame_after = DismissiblePresence {
            any_toast: false,
            ..DismissiblePresence::default()
        };

        // The BUG this subtask exists to prevent: the naive cross-frame
        // comparison (this frame's `after` vs last frame's `after`) sees NO
        // change.
        let naive_cross_frame_transitioned =
            dismissible_presence_transitioned(prev_frame_after, after);
        assert!(
            !naive_cross_frame_transitioned,
            "naive cross-frame comparison must (wrongly) show no change here -- \
             this is exactly the ghost hazard the before/after-within-frame diff exists to catch"
        );

        // The MANDATORY before/after-within-frame diff DOES catch it.
        let intra_frame_transitioned = dismissible_presence_transitioned(before, after);
        assert!(
            intra_frame_transitioned,
            "before/after-within-frame diff must detect the toast's self-dismissal"
        );

        // Feeding the correct (intra-frame) result into the decision forces
        // THIS frame Changed. If the before/after-within-frame diff were
        // dropped and only `naive_cross_frame_transitioned` were used here,
        // this would (wrongly) compute Unchanged -- proving the intra-frame
        // diff is load-bearing, not redundant.
        let signals = ChromeSignals::default();
        let decision_n = decide_chrome_damage(&signals, intra_frame_transitioned, false);
        assert_eq!(
            decision_n,
            ChromeDamage::Changed,
            "frame N (the toast's self-dismissal frame) must be Changed"
        );

        // ...and the §3.5 settle rule forces the NEXT frame Changed too,
        // even though nothing else changed and the naive cross-frame check
        // would (again) see no difference on that frame either.
        let settle_pending_next = intra_frame_transitioned;
        let decision_n_plus_1 = decide_chrome_damage(&signals, false, settle_pending_next);
        assert_eq!(
            decision_n_plus_1,
            ChromeDamage::Changed,
            "frame N+1 (the settle frame) must also be Changed"
        );
    }

    #[test]
    fn tab_snapshot_diff_all_unchanged_is_all_false() {
        let snap = ChromeTabSnapshot {
            tab_ids: vec![TabId::first()],
            active_tab_id: Some(TabId::first()),
            tab_titles: vec!["shell".to_owned()],
            pane_ids: vec![PaneId::first()],
            zoomed_pane: None,
            broadcast_input: false,
        };
        let diff = diff_tab_snapshots(&snap, &snap.clone());
        assert!(!diff.tab_set_changed);
        assert!(!diff.tab_title_changed);
        assert!(!diff.pane_layout_changed);
        assert!(!diff.broadcast_state_changed);
    }

    #[test]
    fn tab_snapshot_diff_detects_tab_set_change() {
        let prev = ChromeTabSnapshot {
            tab_ids: vec![TabId::first()],
            active_tab_id: Some(TabId::first()),
            ..ChromeTabSnapshot::default()
        };
        let current = ChromeTabSnapshot {
            tab_ids: vec![TabId::first(), TabId::offset(1)],
            active_tab_id: Some(TabId::first()),
            ..ChromeTabSnapshot::default()
        };
        let diff = diff_tab_snapshots(&prev, &current);
        assert!(diff.tab_set_changed, "tab id list grew");
        assert!(!diff.tab_title_changed);
        assert!(!diff.pane_layout_changed);
        assert!(!diff.broadcast_state_changed);
    }

    #[test]
    fn tab_snapshot_diff_detects_active_tab_change_with_same_id_list() {
        let prev = ChromeTabSnapshot {
            tab_ids: vec![TabId::first(), TabId::offset(1)],
            active_tab_id: Some(TabId::first()),
            ..ChromeTabSnapshot::default()
        };
        let current = ChromeTabSnapshot {
            tab_ids: vec![TabId::first(), TabId::offset(1)],
            active_tab_id: Some(TabId::offset(1)),
            ..ChromeTabSnapshot::default()
        };
        let diff = diff_tab_snapshots(&prev, &current);
        assert!(diff.tab_set_changed, "active tab id changed");
    }

    #[test]
    fn tab_snapshot_diff_detects_tab_title_change() {
        let prev = ChromeTabSnapshot {
            tab_titles: vec!["old".to_owned()],
            ..ChromeTabSnapshot::default()
        };
        let current = ChromeTabSnapshot {
            tab_titles: vec!["new".to_owned()],
            ..ChromeTabSnapshot::default()
        };
        let diff = diff_tab_snapshots(&prev, &current);
        assert!(diff.tab_title_changed);
        assert!(!diff.tab_set_changed);
    }

    #[test]
    fn tab_snapshot_diff_detects_pane_layout_change_via_pane_ids() {
        let prev = ChromeTabSnapshot {
            pane_ids: vec![PaneId::first()],
            ..ChromeTabSnapshot::default()
        };
        let current = ChromeTabSnapshot {
            pane_ids: vec![PaneId::first(), second_pane_id()],
            ..ChromeTabSnapshot::default()
        };
        let diff = diff_tab_snapshots(&prev, &current);
        assert!(diff.pane_layout_changed, "a split added a pane");
        assert!(!diff.tab_set_changed);
    }

    #[test]
    fn tab_snapshot_diff_detects_pane_layout_change_via_zoom() {
        let prev = ChromeTabSnapshot {
            pane_ids: vec![PaneId::first()],
            zoomed_pane: None,
            ..ChromeTabSnapshot::default()
        };
        let current = ChromeTabSnapshot {
            pane_ids: vec![PaneId::first()],
            zoomed_pane: Some(PaneId::first()),
            ..ChromeTabSnapshot::default()
        };
        let diff = diff_tab_snapshots(&prev, &current);
        assert!(diff.pane_layout_changed, "zoom entered");
    }

    #[test]
    fn tab_snapshot_diff_detects_broadcast_state_change() {
        let prev = ChromeTabSnapshot {
            broadcast_input: false,
            ..ChromeTabSnapshot::default()
        };
        let current = ChromeTabSnapshot {
            broadcast_input: true,
            ..ChromeTabSnapshot::default()
        };
        let diff = diff_tab_snapshots(&prev, &current);
        assert!(diff.broadcast_state_changed);
        assert!(!diff.tab_set_changed);
        assert!(!diff.tab_title_changed);
        assert!(!diff.pane_layout_changed);
    }
}
