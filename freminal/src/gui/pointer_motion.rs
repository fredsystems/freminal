// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The out-of-frame pointer-motion repaint decision (Task 122, subtask
//! 122.5a).
//!
//! `freminal-windowing`'s pointer fast path calls
//! `App::pointer_motion_needs_repaint` on every `CursorMoved` event,
//! **outside any egui frame**, to decide whether that motion requires a
//! repaint at all. This module holds the pure decision chain behind that
//! call — pane resolution, hover-region risk, and the composed
//! true/false answer — factored out of `app_impl.rs` (subtask 122.5) and
//! then out of that file entirely (subtask 122.5a) because it is one
//! coherent concept distinct from the `App` trait impl: it is pure, it
//! runs before any frame exists, and it is headlessly unit-testable
//! without constructing a `FreminalGui`/`PerWindowState`/`PaneTree`/`Pane`/
//! `TerminalSnapshot`. Per `freminal-module-cohesion`, a concept this
//! self-contained gets a module whose path names it, rather than staying
//! folded into the file it had already outgrown.
//!
//! `App::pointer_motion_needs_repaint` and `App::is_chrome_interactive_at`
//! themselves stay in `app_impl.rs` — they are trait-impl methods needing
//! `&self` — and call into this module.
use freminal_common::geometry::{Point, Rect};

use super::panes;

/// Task 121 pointer-motion repaint-gate spike: per-pane signals feeding
/// [`pointer_motion_needs_repaint_decision`], for whichever pane (if any) is
/// under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PointerMotionPaneSignals {
    /// The pane's terminal application has requested mouse reporting
    /// (`TerminalSnapshot::mouse_tracking != MouseTrack::NoTracking`) — if
    /// it is receiving mouse reports, motion must always be forwarded, so a
    /// repaint is always needed.
    mouse_tracking_active: bool,
    /// See [`pane_hover_region_risk`]'s doc: a coarse, pane-level (not
    /// pixel-precise) approximation of "this pane may have a hover-sensitive
    /// band region (URL, command-block gutter, or scrollbar) this frame".
    hover_region_risk: bool,
}

/// Task 121 spike (residual-risk approximation — see the doc on
/// `App::pointer_motion_needs_repaint`'s freminal-side implementation for
/// the full writeup): may this pane have a hover-sensitive band region that
/// pointer motion anywhere in the pane could affect?
///
/// This is deliberately **pane-level, not pixel/cell-precise**: the exact
/// strip/thumb geometry (gutter width, scrollbar track rect, per-cell
/// hyperlink spans) is computed inside the per-frame render pass using
/// `pixels_per_point` and the live `terminal_rect`/`gutter_rect` split
/// (`terminal/widget.rs`), neither of which is available at the
/// `pointer_motion_needs_repaint` call site (outside a frame, before
/// `pixels_per_point` for a possible new frame is known). Rather than
/// guess at that geometry, this function treats each of the three
/// hover-sensitive band regions as risking the WHOLE pane whenever its
/// cheap, always-available precondition holds:
///   - `has_urls`: the pane's visible content contains at least one
///     hyperlink anywhere (`TerminalSnapshot::has_urls`) — approximates
///     "pointer motion might enter/leave a URL span" as "pointer motion
///     anywhere in this pane", since the precise cell-to-hyperlink hit test
///     (`flat_index_for_cell` + `url_tag_indices` lookup in
///     `terminal/widget.rs`) is render-pass-only state.
///   - `scroll_offset > 0`: the scrollbar is only rendered/interactive when
///     scrolled back (`handle_scrollbar`'s own gate) — approximates "over
///     the scrollbar track" as "anywhere in this pane while scrolled back".
///   - `gutter_config_active && !is_alternate_screen && command_blocks_non_empty
///     && pointer_in_gutter_strip`:
///     the command-block gutter strip is only meaningful when the feature is
///     on, the pane has at least one block, and the alternate screen (which
///     never shows the gutter) is not active — AND, unlike the other two
///     terms, this one IS pixel-precise: `pointer_in_gutter_strip` is a real
///     `pos.x` test against the pane's left-edge gutter strip (see the doc
///     on `App::pointer_motion_needs_repaint`'s `gutter_width_upper_bound_logical`
///     for how that test is computed without needing `pixels_per_point` at
///     this call site). This term was measured (Task 121 spike) to fire on
///     100% of pointer-motion checks in any session with auto-detected
///     command blocks before the positional test was added — the whole pane
///     was being treated as gutter-hover-sensitive.
///
/// Pure, so directly unit-testable without constructing a `Pane`/snapshot.
// Each bool is an independent, unrelated precondition for one of the three
// hover-sensitive band regions (URL, scrollbar, gutter) -- not a state
// machine; bundling them into an enum would not express any real combined
// state and would only obscure the call site (mirrors the existing allow on
// `terminal/widget.rs`'s `show`).
#[allow(clippy::fn_params_excessive_bools)]
const fn pane_hover_region_risk(
    has_urls: bool,
    scroll_offset: usize,
    is_alternate_screen: bool,
    command_blocks_non_empty: bool,
    gutter_config_active: bool,
    pointer_in_gutter_strip: bool,
) -> bool {
    let (has_urls, scroll_offset_nonzero, gutter_active) = pane_hover_region_terms(
        has_urls,
        scroll_offset,
        is_alternate_screen,
        command_blocks_non_empty,
        gutter_config_active,
        pointer_in_gutter_strip,
    );
    has_urls || scroll_offset_nonzero || gutter_active
}

/// Task 121 diagnostic: the three independent preconditions
/// [`pane_hover_region_risk`] ORs together, exposed separately so a caller
/// can count which one(s) actually fired rather than only the aggregate
/// (see `FreminalGui::pointer_motion_needs_repaint`'s
/// `#[cfg(feature = "frame-profiling")]` recording block). `pane_hover_region_risk`
/// itself calls this and ORs the three results, so the two functions can
/// never disagree about what "hover region risk" means — this is a pure
/// refactor of that function's body, not a second, independently-maintained
/// copy of its logic.
///
/// Returns `(has_urls, scroll_offset_nonzero, gutter_active)`. See
/// `pane_hover_region_risk`'s doc for what each term approximates. Pure, so
/// directly unit-testable.
///
/// `pointer_in_gutter_strip` is the positional term (Task 121 fix): `true`
/// when the pointer's x-coordinate falls within the gutter's left-edge
/// strip for this pane. Passing `true` unconditionally reproduces the
/// pre-fix pane-wide approximation; real callers pass a real rect test —
/// see `App::pointer_motion_needs_repaint`.
// Same rationale as `pane_hover_region_risk`'s matching allow: six
// independent, unrelated preconditions, not a state machine.
#[allow(clippy::fn_params_excessive_bools)]
const fn pane_hover_region_terms(
    has_urls: bool,
    scroll_offset: usize,
    is_alternate_screen: bool,
    command_blocks_non_empty: bool,
    gutter_config_active: bool,
    pointer_in_gutter_strip: bool,
) -> (bool, bool, bool) {
    (
        has_urls,
        scroll_offset > 0,
        gutter_config_active
            && !is_alternate_screen
            && command_blocks_non_empty
            && pointer_in_gutter_strip,
    )
}

/// Task 121 fix: is `pos_x` within the pane's left-edge gutter strip?
///
/// The strip runs from `pane_rect_min_x` (the pane's left edge) to
/// `pane_rect_min_x + gutter_width_upper_bound_logical`, using `<` (NOT
/// `<=`) at the far edge: the boundary point itself counts as OUTSIDE.
///
/// `gutter_width_upper_bound_logical` is the gutter's *total inset* in
/// logical points (`PublishedFrameState::cached_gutter_inset_logical`),
/// which is strictly wider than the painted strip because the inset includes the
/// padding gap. The real strip's right edge therefore always falls strictly
/// inside this bound, making the `<` vs `<=` choice at the outer boundary
/// immaterial. Being a genuine logical measurement, it holds at any
/// `pixels_per_point` — including fractional scale below 1.0, which broke
/// the earlier physical-pixels-as-logical approximation.
///
/// Also `false` when `pos_x` is left of the pane entirely (`pos_x <
/// pane_rect_min_x`) — relevant for multi-pane layouts where a pane's left
/// edge is not at window x=0.
///
/// Pure, so directly unit-testable without a live pane/rect.
const fn pointer_in_gutter_strip(
    pos_x: f32,
    pane_rect_min_x: f32,
    gutter_width_upper_bound_logical: f32,
) -> bool {
    pos_x >= pane_rect_min_x && pos_x < pane_rect_min_x + gutter_width_upper_bound_logical
}

/// Subtask 121.14: pure composition of the "some animation is in flight
/// somewhere in this window, independent of pointer position" term used by
/// `pointer_motion_needs_repaint`. Extracted so it is unit-testable without
/// a live `FreminalGui`/`PerWindowState` (see
/// `pointer_motion_needs_repaint_decision`'s doc for why the wrapping
/// method cannot be constructed headlessly). Trivial (an OR of two already-
/// computed booleans), but named and tested on its own so the composition
/// itself — as distinct from how each term is computed — is pinned.
pub(super) const fn animation_in_flight_composed(
    resize_overlay_animating: bool,
    toast_animating: bool,
) -> bool {
    resize_overlay_animating || toast_animating
}

/// Task 121 pointer-motion repaint-gate spike: the composed decision behind
/// `App::pointer_motion_needs_repaint`'s freminal-side implementation.
/// Extracted as a pure function over already-computed signals so it is
/// unit-testable without a live `FreminalGui`/windowing stack (a full
/// `FreminalGui`/`PerWindowState` cannot be constructed headlessly —
/// `freminal_windowing::WindowId` has no public constructor outside the real
/// winit event loop).
///
/// Returns `true` (a repaint is needed) if ANY of:
///   - `focus_change_pending`: focus-follows-mouse is enabled and the pointer
///     is over a pane that is not the active one, so this motion has a focus
///     switch to apply. Without this term the switch is not applied until
///     some unrelated event schedules the next frame -- in an otherwise idle
///     terminal that is the ~500ms cursor-blink wake, which is what made
///     hover-to-focus feel badly lagged. Suppression is preserved for motion
///     within the already-active pane, which is the common case the gate
///     exists to make cheap.
///   - `chrome_interactive`: `App::is_chrome_interactive_at` said so (menu
///     bar, tab bar, split-border drag sensor).
///   - `any_pane_selecting`: some pane in the active tab has an
///     in-progress selection drag (`ViewState::selection.is_selecting`).
///   - `overlay_open`: some UI overlay/popup/tooltip/context menu is open
///     this window.
///   - `pointer_pane_unresolved`: the pane under the pointer could not be
///     determined at all (no FULL frame has rendered yet, so there is no
///     cached pane layout to hit-test against) — conservative "unknown".
///   - `pane_signals` resolved to `Some` (a specific pane is under the
///     pointer) AND that pane's `mouse_tracking_active` or
///     `hover_region_risk` (see [`pane_hover_region_risk`]) is `true`.
///
/// When `pane_signals` is `None` because the pointer is simply not over any
/// pane (e.g. over inter-pane padding not covered by a chrome-border
/// sensor), that is NOT `pointer_pane_unresolved` — it is a legitimate "no
/// pane, so no pane-specific signal applies" case, contributing `false`.
///
/// Subtask 124.4: the five window-level terms arrive as the named fields of
/// [`PointerMotionInputs`] rather than as five positional `bool` parameters.
/// `freminal-state-representation` forbids bool *parameters* outright, and
/// PR #496 flagged the old signature in both its body and commit `b17c5709`
/// as "a real hazard" — five same-typed positional arguments whose order is
/// unenforceable, called from one site and eleven tests. There is **no
/// expected performance effect**; this is a readability and safety fix and
/// must not be presented as anything else.
pub(super) const fn pointer_motion_needs_repaint_decision(inputs: PointerMotionInputs) -> bool {
    if inputs.focus_change_pending
        || inputs.chrome_interactive
        || inputs.any_pane_selecting
        || inputs.overlay_open
        || inputs.pointer_pane_unresolved
    {
        return true;
    }
    match inputs.pane_signals {
        Some(s) => s.mouse_tracking_active || s.hover_region_risk,
        None => false,
    }
}

/// Subtask 124.4: the window-level inputs to
/// [`pointer_motion_needs_repaint_decision`].
///
/// See that function's doc comment for what each term means and why it
/// forces a repaint. This type exists so those terms are named at the call
/// site; it carries no logic of its own.
///
/// [`Default`] is `all-clear` — every term `false`, no pane resolved — which
/// is the "plain motion over the already-active pane's terminal content"
/// case the gate exists to suppress. Tests construct the interesting cases
/// with struct-update syntax off it, so each test names only the one term it
/// is about.
// struct_excessive_bools: each field is an INDEPENDENT forcing condition
// that can be true simultaneously with any other (chrome interactivity,
// selection drag, overlay presence, pane-resolution failure, pending focus
// switch) -- the "independent simultaneous signals" case in
// `state-representation`, not a state machine masquerading as bools. This
// mirrors `PaneSnapshotInputs` and `window.rs`'s
// `PointerMotionConditionFlags`, which carry the allow for the same reason.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct PointerMotionInputs {
    /// Focus-follows-mouse is enabled and the pointer is over a pane that is
    /// not the active one, so this motion has a focus switch to apply.
    pub(super) focus_change_pending: bool,
    /// `App::is_chrome_interactive_at` said so (menu bar, tab bar,
    /// split-border drag sensor).
    pub(super) chrome_interactive: bool,
    /// Some pane in the active tab has an in-progress selection drag.
    pub(super) any_pane_selecting: bool,
    /// Some UI overlay/popup/tooltip/context menu is open this window.
    pub(super) overlay_open: bool,
    /// The pane under the pointer could not be determined at all —
    /// conservative "unknown", distinct from "no pane here".
    pub(super) pointer_pane_unresolved: bool,
    /// The resolved pane's signals, or `None` when the pointer is over no
    /// pane at all. `None` is NOT `pointer_pane_unresolved`.
    pub(super) pane_signals: Option<PointerMotionPaneSignals>,
}

/// Subtask 122.5: the per-pane values [`resolve_pane_under_pointer`] needs
/// for whichever pane (if any) resolves under the pointer — a stand-in for
/// the fields read off `pane.arc_swap.load()`'s `TerminalSnapshot`, passed
/// through a lookup closure so the resolution chain itself needs no
/// `&PaneTree`/`&Pane` and is headlessly unit-testable.
// struct_excessive_bools: each field is an independent observation read
// straight off a `TerminalSnapshot` (mirrors the same rationale as
// `window.rs`'s `PointerMotionConditionFlags`) — not a state machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PaneSnapshotInputs {
    /// `TerminalSnapshot::mouse_tracking != MouseTrack::NoTracking`.
    pub(super) mouse_tracking_active: bool,
    /// `TerminalSnapshot::has_urls`.
    pub(super) has_urls: bool,
    /// `TerminalSnapshot::scroll_offset`.
    pub(super) scroll_offset: usize,
    /// `TerminalSnapshot::is_alternate_screen`.
    pub(super) is_alternate_screen: bool,
    /// `!TerminalSnapshot::command_blocks.is_empty()`.
    pub(super) command_blocks_non_empty: bool,
}

/// The result of [`resolve_pane_under_pointer`]: the resolved pane's Task
/// 121 repaint-gate signals, plus (subtask 122.5) the four diagnostic term
/// bools those signals are composed from.
///
/// The four term bools are computed **unconditionally** — not only under
/// `#[cfg(feature = "frame-profiling")]` — specifically so the diagnostic
/// recording at the call site (`FreminalGui::pointer_motion_needs_repaint`)
/// can never drift from the real computation: before this subtask the
/// terms were computed a second time inside a `#[cfg(...)]` block
/// interleaved with the real decision, relying on a comment to keep the two
/// copies in sync. Recomputing the diagnostic in the caller from this
/// struct's fields would reintroduce exactly that drift risk, so callers
/// MUST read them from here rather than recompute them.
// struct_excessive_bools: each of the four bool fields is an independent
// yes/no observation (same rationale as `window.rs`'s
// `PointerMotionConditionFlags`, which these terms feed) — not a state
// machine.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PaneResolution {
    /// `Some` when a pane resolved under the pointer AND `pane_inputs`
    /// returned a value for it; `None` when no pane resolved (the pointer
    /// is over inter-pane padding, or a zoomed/split layout simply has no
    /// pane there) or the lookup returned `None` (the resolved id no
    /// longer exists in the tree).
    pub(super) signals: Option<PointerMotionPaneSignals>,
    /// See [`pane_hover_region_terms`]'s doc for what this approximates.
    /// `false` when `signals` is `None`.
    pub(super) mouse_tracking_active: bool,
    /// See [`pane_hover_region_terms`]'s doc. `false` when `signals` is
    /// `None`.
    pub(super) has_urls: bool,
    /// See [`pane_hover_region_terms`]'s doc. `false` when `signals` is
    /// `None`.
    pub(super) scroll_offset_nonzero: bool,
    /// See [`pane_hover_region_terms`]'s doc. `false` when `signals` is
    /// `None`.
    pub(super) gutter_active: bool,
    /// Which pane the pointer resolved to, when one did.
    ///
    /// Needed by the focus-follows-mouse term: motion only matters for focus
    /// if it lands on a pane that is not already the active one.
    pub(super) resolved_pane: Option<panes::PaneId>,
}

impl PaneResolution {
    /// The "no pane resolved" result: every field `false`/`None`, matching
    /// what the pre-extraction diagnostic recorded via
    /// `pane_diag_terms.unwrap_or_default()` when `pane_signals` was
    /// `None`.
    pub(super) const fn unresolved() -> Self {
        Self {
            signals: None,
            mouse_tracking_active: false,
            has_urls: false,
            scroll_offset_nonzero: false,
            gutter_active: false,
            resolved_pane: None,
        }
    }
}

/// Subtask 122.5: the pane-resolution chain behind
/// `FreminalGui::pointer_motion_needs_repaint` — layout -> hit-test ->
/// snapshot lookup -> signal computation — extracted as a pure function so
/// it is headlessly unit-testable without a live
/// `FreminalGui`/`PerWindowState`/`PaneTree`/`Pane`/`TerminalSnapshot` (see
/// that method's doc for why none of those can be constructed outside the
/// real winit event loop).
///
/// # Zoomed vs split — mirrors `update()`'s own choice, do not get this wrong
///
/// When `zoomed_pane` is `Some`, that pane alone is treated as filling
/// `central_rect` and `split_layout` is ignored entirely. Otherwise
/// `split_layout` — the tree's ordinary split layout, exactly what
/// `PaneTree::layout` returns, supplied as data so this function needs no
/// `&PaneTree` — is hit-tested. This mirrors `update()`'s own
/// zoomed-vs-split rendering choice (`central_body`); getting it wrong
/// would silently hit-test the pointer against panes that are not actually
/// the one rendered full-size this frame.
///
/// Either way `pos` must still fall inside the candidate rect to resolve:
/// the zoomed branch is NOT an unconditional hit on `zoomed_pane` — a
/// pointer outside `central_rect` entirely (e.g. still over chrome) resolves
/// to no pane even while zoomed, exactly as the pre-extraction code did by
/// hit-testing a single-entry `vec![(zoomed_id, central_rect)]` through the
/// same `.contains(pos)` check the split branch uses.
///
/// `pane_inputs` stands in for `active_tab.pane_tree.find(pane_id)` +
/// `pane.arc_swap.load()`: a lookup from a resolved [`panes::PaneId`] to
/// that pane's current [`PaneSnapshotInputs`]. Returning `None` models
/// `PaneTree::find` returning `None` (the resolved id no longer exists),
/// which the real caller also treats as "no pane" rather than a bug.
pub(super) fn resolve_pane_under_pointer(
    pos: Point,
    central_rect: Rect,
    zoomed_pane: Option<panes::PaneId>,
    split_layout: &[(panes::PaneId, Rect)],
    gutter_config_active: bool,
    gutter_width_upper_bound_logical: f32,
    pane_inputs: impl Fn(panes::PaneId) -> Option<PaneSnapshotInputs>,
) -> PaneResolution {
    let hit = zoomed_pane.map_or_else(
        || {
            split_layout
                .iter()
                .find(|(_, rect)| rect.contains(pos))
                .copied()
        },
        |zoomed_id| {
            central_rect
                .contains(pos)
                .then_some((zoomed_id, central_rect))
        },
    );
    let Some((pane_id, pane_rect)) = hit else {
        return PaneResolution::unresolved();
    };

    let Some(inputs) = pane_inputs(pane_id) else {
        return PaneResolution::unresolved();
    };

    let pointer_in_gutter_strip_now =
        pointer_in_gutter_strip(pos.x, pane_rect.min.x, gutter_width_upper_bound_logical);

    let hover_region_risk = pane_hover_region_risk(
        inputs.has_urls,
        inputs.scroll_offset,
        inputs.is_alternate_screen,
        inputs.command_blocks_non_empty,
        gutter_config_active,
        pointer_in_gutter_strip_now,
    );

    let (has_urls, scroll_offset_nonzero, gutter_active) = pane_hover_region_terms(
        inputs.has_urls,
        inputs.scroll_offset,
        inputs.is_alternate_screen,
        inputs.command_blocks_non_empty,
        gutter_config_active,
        pointer_in_gutter_strip_now,
    );

    PaneResolution {
        signals: Some(PointerMotionPaneSignals {
            mouse_tracking_active: inputs.mouse_tracking_active,
            hover_region_risk,
        }),
        mouse_tracking_active: inputs.mouse_tracking_active,
        has_urls,
        scroll_offset_nonzero,
        gutter_active,
        resolved_pane: Some(pane_id),
    }
}

#[cfg(test)]
mod tests {
    use freminal_common::geometry::{Rect, point};

    use super::{
        PaneResolution, PaneSnapshotInputs, PointerMotionInputs, PointerMotionPaneSignals,
        animation_in_flight_composed, pane_hover_region_risk, pane_hover_region_terms,
        pointer_in_gutter_strip, pointer_motion_needs_repaint_decision, resolve_pane_under_pointer,
    };
    use crate::gui::panes::PaneIdGenerator;

    // ── Subtask 121.14: `animation_in_flight_composed` ───────────────────

    #[test]
    fn animation_in_flight_composed_both_false_is_false() {
        assert!(!animation_in_flight_composed(false, false));
    }

    #[test]
    fn animation_in_flight_composed_resize_overlay_alone_forces_true() {
        assert!(animation_in_flight_composed(true, false));
    }

    #[test]
    fn animation_in_flight_composed_toast_alone_forces_true() {
        assert!(animation_in_flight_composed(false, true));
    }

    #[test]
    fn animation_in_flight_composed_both_true_is_true() {
        assert!(animation_in_flight_composed(true, true));
    }

    // ── Task 121 pointer-motion repaint-gate spike ───────────────────────

    #[test]
    fn pane_hover_region_risk_all_clear_is_false() {
        assert!(!pane_hover_region_risk(false, 0, false, false, true, true));
    }

    #[test]
    fn pane_hover_region_risk_has_urls_is_true_regardless_of_everything_else() {
        assert!(pane_hover_region_risk(true, 0, true, false, false, false));
    }

    #[test]
    fn pane_hover_region_risk_scrolled_back_is_true() {
        assert!(pane_hover_region_risk(false, 1, false, false, false, false));
    }

    #[test]
    fn pane_hover_region_risk_at_live_bottom_is_not_risky_from_scroll_alone() {
        assert!(!pane_hover_region_risk(
            false, 0, false, false, false, false
        ));
    }

    #[test]
    fn pane_hover_region_risk_gutter_needs_all_four_conditions() {
        // Feature enabled + blocks present + not alt screen + pointer in the
        // strip -> risky.
        assert!(pane_hover_region_risk(false, 0, false, true, true, true));
        // Any one of the four missing -> not risky (from gutter alone).
        assert!(!pane_hover_region_risk(false, 0, false, true, false, true)); // feature off
        assert!(!pane_hover_region_risk(false, 0, false, false, true, true)); // no blocks
        assert!(!pane_hover_region_risk(false, 0, true, true, true, true)); // alt screen
        assert!(!pane_hover_region_risk(false, 0, false, true, true, false)); // pointer outside strip
    }

    // ── Task 121 fix: gutter positional term (pane_hover_region_terms) ───
    //
    // These exercise `pane_hover_region_terms`'s `gutter_active` output
    // directly (rather than through `pane_hover_region_risk`'s aggregate
    // OR) so the positional term's truth table is visible on its own,
    // matching the diagnostic counter that reads this same tuple.

    #[test]
    fn pane_hover_region_terms_gutter_active_true_when_pointer_in_strip() {
        let (_, _, gutter_active) = pane_hover_region_terms(false, 0, false, true, true, true);
        assert!(gutter_active);
    }

    #[test]
    fn pane_hover_region_terms_gutter_active_false_when_pointer_right_of_strip() {
        // The headline case this fix addresses: previously the pane-wide
        // approximation made this `true` unconditionally whenever blocks
        // were present; it must now be `false` once the pointer is past the
        // strip.
        let (_, _, gutter_active) = pane_hover_region_terms(false, 0, false, true, true, false);
        assert!(!gutter_active);
    }

    #[test]
    fn pane_hover_region_terms_gutter_active_false_when_disabled_or_off_or_alt_or_no_blocks() {
        // Pointer inside the strip in all four cases -- only the named
        // precondition is what makes each `false`.
        let (_, _, disabled) = pane_hover_region_terms(false, 0, false, true, false, true);
        assert!(!disabled, "feature disabled must suppress gutter_active");

        let (_, _, alt_screen) = pane_hover_region_terms(false, 0, true, true, true, true);
        assert!(!alt_screen, "alt screen must suppress gutter_active");

        let (_, _, no_blocks) = pane_hover_region_terms(false, 0, false, false, true, true);
        assert!(!no_blocks, "no command blocks must suppress gutter_active");
    }

    // ── Task 121 fix: `pointer_in_gutter_strip` rect test ────────────────

    #[test]
    fn pointer_in_gutter_strip_true_inside_the_strip() {
        // Pane's left edge at x=10, strip width 4 -> strip is [10, 14).
        assert!(pointer_in_gutter_strip(11.0, 10.0, 4.0));
    }

    #[test]
    fn pointer_in_gutter_strip_false_right_of_the_strip() {
        // The headline case: pointer well past the strip.
        assert!(!pointer_in_gutter_strip(50.0, 10.0, 4.0));
    }

    #[test]
    fn pointer_in_gutter_strip_boundary_at_exact_far_edge_is_false() {
        // Chosen convention: the far edge (`pane_rect_min_x + width`) is
        // exclusive, matching a half-open `[min, min+width)` strip -- see
        // this function's doc for why the `ppp == 1.0` edge case is the
        // only one where this choice is observable at all.
        assert!(!pointer_in_gutter_strip(14.0, 10.0, 4.0));
        // Just inside is still true.
        assert!(pointer_in_gutter_strip(13.999, 10.0, 4.0));
    }

    #[test]
    fn pointer_in_gutter_strip_false_left_of_pane_rect() {
        // Multi-pane layouts can place a pane's left edge away from x=0;
        // a pointer left of THIS pane's left edge is not in THIS pane's
        // gutter strip (it is presumably over a different pane or a
        // border/padding region).
        assert!(!pointer_in_gutter_strip(5.0, 10.0, 4.0));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_all_clear_is_false() {
        assert!(!pointer_motion_needs_repaint_decision(
            PointerMotionInputs::default()
        ));
    }

    /// Issue #495. With focus-follows-mouse on, motion onto a non-active pane
    /// carries a pending focus switch, so the frame that applies it must not
    /// be suppressed -- even over plain terminal content with every other
    /// signal clear.
    #[test]
    fn pointer_motion_needs_repaint_decision_pending_focus_change_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            focus_change_pending: true,
            ..PointerMotionInputs::default()
        }));
    }

    /// And the suppression this gate exists for is preserved: with no focus
    /// switch pending (pointer inside the already-active pane) plain motion
    /// still schedules nothing.
    #[test]
    fn pointer_motion_needs_repaint_decision_no_pending_focus_change_still_suppresses() {
        assert!(!pointer_motion_needs_repaint_decision(
            PointerMotionInputs {
                pane_signals: Some(PointerMotionPaneSignals {
                    mouse_tracking_active: false,
                    hover_region_risk: false,
                }),
                ..PointerMotionInputs::default()
            }
        ));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_chrome_interactive_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            chrome_interactive: true,
            ..PointerMotionInputs::default()
        }));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_any_pane_selecting_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            any_pane_selecting: true,
            ..PointerMotionInputs::default()
        }));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_overlay_open_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            overlay_open: true,
            ..PointerMotionInputs::default()
        }));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_unresolved_pane_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            pointer_pane_unresolved: true,
            ..PointerMotionInputs::default()
        }));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_no_pane_under_pointer_is_false() {
        // Pointer resolved to "no pane here" (e.g. inter-pane padding) --
        // NOT the same as unresolved; contributes false on its own.
        assert!(!pointer_motion_needs_repaint_decision(
            PointerMotionInputs {
                pane_signals: None,
                ..PointerMotionInputs::default()
            }
        ));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_mouse_tracking_active_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            pane_signals: Some(PointerMotionPaneSignals {
                mouse_tracking_active: true,
                hover_region_risk: false,
            }),
            ..PointerMotionInputs::default()
        }));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_hover_region_risk_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            pane_signals: Some(PointerMotionPaneSignals {
                mouse_tracking_active: false,
                hover_region_risk: true,
            }),
            ..PointerMotionInputs::default()
        }));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_pane_signals_both_false_is_false() {
        assert!(!pointer_motion_needs_repaint_decision(
            PointerMotionInputs {
                pane_signals: Some(PointerMotionPaneSignals {
                    mouse_tracking_active: false,
                    hover_region_risk: false,
                }),
                ..PointerMotionInputs::default()
            }
        ));
    }

    /// 124.4's own guard: the struct's `Default` must be the all-clear case,
    /// since every test above builds its scenario with struct-update syntax
    /// off it. If a future field defaulted to `true`, those tests would all
    /// silently stop testing what they name.
    #[test]
    fn pointer_motion_inputs_default_is_all_clear() {
        let d = PointerMotionInputs::default();
        assert!(!d.focus_change_pending);
        assert!(!d.chrome_interactive);
        assert!(!d.any_pane_selecting);
        assert!(!d.overlay_open);
        assert!(!d.pointer_pane_unresolved);
        assert!(d.pane_signals.is_none());
    }

    // ── resolve_pane_under_pointer (subtask 122.5) ──────────────────
    //
    // These construct NO `FreminalGui`, `PerWindowState`, `PaneTree`,
    // `Pane`, or `TerminalSnapshot` -- only `PaneId` (via the public
    // `PaneIdGenerator`), the toolkit-neutral `Rect`/`Point`, and a stub
    // `pane_inputs` closure over a hand-built `PaneSnapshotInputs`. This is
    // the success criterion this subtask exists to satisfy.

    /// A `PaneSnapshotInputs` with every field at its "quiet" (non-forcing)
    /// value: not mouse-tracking, no URLs, not scrolled back, primary
    /// screen, no command blocks.
    fn quiet_inputs() -> PaneSnapshotInputs {
        PaneSnapshotInputs {
            mouse_tracking_active: false,
            has_urls: false,
            scroll_offset: 0,
            is_alternate_screen: false,
            command_blocks_non_empty: false,
        }
    }

    #[test]
    fn resolve_pane_under_pointer_zoomed_hit_tests_the_zoomed_pane_not_the_split_layout() {
        let mut id_gen = PaneIdGenerator::new(0);
        let split_pane = id_gen.next_id();
        let zoomed_pane = id_gen.next_id();

        let central_rect = Rect::from_min_max(point(0.0, 0.0), point(100.0, 100.0));
        // The split layout's rect for `split_pane` ALSO contains the test
        // point -- if the zoomed branch incorrectly consulted
        // `split_layout` instead of ignoring it, the lookup closure below
        // would be called with `split_pane` and the assertion inside it
        // would fail.
        let split_layout = [(split_pane, central_rect)];
        let pos = point(50.0, 50.0);

        let resolution = resolve_pane_under_pointer(
            pos,
            central_rect,
            Some(zoomed_pane),
            &split_layout,
            false,
            0.0,
            |id| {
                assert_eq!(
                    id, zoomed_pane,
                    "zoomed branch must resolve the zoomed pane, not a pane from split_layout"
                );
                Some(quiet_inputs())
            },
        );

        assert!(resolution.signals.is_some());
    }

    #[test]
    fn resolve_pane_under_pointer_split_hit_tests_the_supplied_tree_layout() {
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let pane_b = id_gen.next_id();

        let rect_a = Rect::from_min_max(point(0.0, 0.0), point(50.0, 100.0));
        let rect_b = Rect::from_min_max(point(50.0, 0.0), point(100.0, 100.0));
        let split_layout = [(pane_a, rect_a), (pane_b, rect_b)];
        let central_rect = Rect::from_min_max(point(0.0, 0.0), point(100.0, 100.0));

        // Inside rect_b only.
        let pos = point(75.0, 50.0);

        let resolution =
            resolve_pane_under_pointer(pos, central_rect, None, &split_layout, false, 0.0, |id| {
                assert_eq!(id, pane_b, "must resolve the pane whose rect contains pos");
                Some(quiet_inputs())
            });

        assert!(resolution.signals.is_some());
    }

    #[test]
    fn resolve_pane_under_pointer_zoomed_pointer_outside_central_rect_resolves_to_no_pane() {
        // Pins that the zoomed branch is NOT an unconditional hit on
        // `zoomed_pane`: `pos` must still fall inside `central_rect`,
        // mirroring the pre-extraction code's shared `.contains(pos)` check
        // (both branches fed the same hit-test, over a one-entry
        // `vec![(zoomed_id, central_rect)]` in the zoomed case).
        let mut id_gen = PaneIdGenerator::new(0);
        let zoomed_pane = id_gen.next_id();
        let central_rect = Rect::from_min_max(point(0.0, 0.0), point(100.0, 100.0));
        let pos = point(500.0, 500.0);

        let resolution = resolve_pane_under_pointer(
            pos,
            central_rect,
            Some(zoomed_pane),
            &[],
            false,
            0.0,
            |_| unreachable!("lookup must not run when no pane resolves"),
        );

        assert_eq!(resolution, PaneResolution::unresolved());
    }

    #[test]
    fn resolve_pane_under_pointer_outside_every_pane_is_legitimately_no_pane() {
        // Pointer over inter-pane padding: no pane resolves, contributing
        // `false` for every term. This is deliberately NOT the same case as
        // "pane resolution altogether unavailable"
        // (`pointer_pane_unresolved`, computed by the caller from
        // `cached_central_rect().is_none()` BEFORE this function is even
        // invoked) -- it is a legitimate "no pane here" outcome.
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let rect_a = Rect::from_min_max(point(0.0, 0.0), point(50.0, 100.0));
        let split_layout = [(pane_a, rect_a)];
        let central_rect = Rect::from_min_max(point(0.0, 0.0), point(100.0, 100.0));

        // Right of rect_a, not covered by any pane in the layout.
        let pos = point(75.0, 50.0);

        let resolution =
            resolve_pane_under_pointer(pos, central_rect, None, &split_layout, false, 0.0, |_| {
                unreachable!("lookup must not run when no pane resolves")
            });

        assert_eq!(resolution, PaneResolution::unresolved());
    }

    #[test]
    fn resolve_pane_under_pointer_gutter_strip_fires_inside_not_just_outside() {
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let rect_a = Rect::from_min_max(point(0.0, 0.0), point(100.0, 100.0));
        let split_layout = [(pane_a, rect_a)];
        let central_rect = rect_a;
        let gutter_config_active = true;
        let gutter_width = 10.0;
        let inputs_with_a_block = PaneSnapshotInputs {
            command_blocks_non_empty: true,
            ..quiet_inputs()
        };

        // Inside the strip: pane's left edge (0.0) + gutter width (10.0).
        let inside = point(5.0, 50.0);
        let resolution = resolve_pane_under_pointer(
            inside,
            central_rect,
            None,
            &split_layout,
            gutter_config_active,
            gutter_width,
            |_| Some(inputs_with_a_block),
        );
        assert!(
            resolution.gutter_active,
            "pointer inside the gutter strip must fire the gutter term"
        );
        match resolution.signals {
            Some(sig) => assert!(sig.hover_region_risk),
            None => panic!("a pane must have resolved under the pointer"),
        }

        // Just outside the strip.
        let outside = point(15.0, 50.0);
        let resolution = resolve_pane_under_pointer(
            outside,
            central_rect,
            None,
            &split_layout,
            gutter_config_active,
            gutter_width,
            |_| Some(inputs_with_a_block),
        );
        assert!(
            !resolution.gutter_active,
            "pointer just outside the gutter strip must not fire the gutter term"
        );
        match resolution.signals {
            Some(sig) => assert!(!sig.hover_region_risk),
            None => panic!("a pane must have resolved under the pointer"),
        }
    }

    #[test]
    fn resolve_pane_under_pointer_broken_split_layout_resolves_to_no_pane_not_forced_true() {
        // Pins the conservative direction for a `PaneError::InvalidState`
        // (empty/broken tree) at the ONE place that error can reach this
        // function: the real caller's
        // `active_tab.pane_tree.layout(...).unwrap_or_default()`, which
        // turns a `PaneError::Err` into an empty `split_layout` -- exactly
        // what `&[]` simulates here. This function then legitimately
        // resolves to no pane (`PaneResolution::unresolved()`), which is
        // NOT itself a forced-`true` outcome.
        //
        // The overall conservative-true guarantee for a genuinely broken
        // tree does not come from here: it comes from the SEPARATE
        // `any_selecting` term the real caller computes independently via
        // `active_tab.pane_tree.iter_panes().map_or(true, ...)` (see
        // `pointer_motion_needs_repaint`'s body) -- untouched by this
        // subtask's extraction, and already exercised by
        // `pointer_motion_needs_repaint_decision_any_pane_selecting_forces_true`
        // above. This test pins that THIS function's own contribution on a
        // broken/empty layout is a plain "no pane", not a second,
        // independently-forced `true`.
        let central_rect = Rect::from_min_max(point(0.0, 0.0), point(100.0, 100.0));
        let pos = point(50.0, 50.0);

        let resolution =
            resolve_pane_under_pointer(pos, central_rect, None, &[], false, 0.0, |_| {
                unreachable!("lookup must not run when no pane resolves")
            });

        assert_eq!(resolution, PaneResolution::unresolved());
    }

    #[test]
    fn resolve_pane_under_pointer_diagnostic_terms_match_pre_extraction_computation() {
        // Pins that `PaneResolution`'s four term bools equal exactly what
        // the pre-extraction `#[cfg(feature = "frame-profiling")]` block
        // used to compute inline, for the same inputs.
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let rect_a = Rect::from_min_max(point(0.0, 0.0), point(100.0, 100.0));
        let split_layout = [(pane_a, rect_a)];
        let central_rect = rect_a;
        let gutter_config_active = true;
        let gutter_width = 10.0;
        // Inside the gutter strip.
        let pos = point(5.0, 50.0);

        let inputs = PaneSnapshotInputs {
            mouse_tracking_active: true,
            has_urls: true,
            scroll_offset: 3,
            is_alternate_screen: false,
            command_blocks_non_empty: true,
        };

        let resolution = resolve_pane_under_pointer(
            pos,
            central_rect,
            None,
            &split_layout,
            gutter_config_active,
            gutter_width,
            |_| Some(inputs),
        );

        let pointer_in_gutter_strip_now =
            pointer_in_gutter_strip(pos.x, rect_a.min.x, gutter_width);
        let (expected_has_urls, expected_scroll_offset_nonzero, expected_gutter_active) =
            pane_hover_region_terms(
                inputs.has_urls,
                inputs.scroll_offset,
                inputs.is_alternate_screen,
                inputs.command_blocks_non_empty,
                gutter_config_active,
                pointer_in_gutter_strip_now,
            );
        let expected_hover_region_risk = pane_hover_region_risk(
            inputs.has_urls,
            inputs.scroll_offset,
            inputs.is_alternate_screen,
            inputs.command_blocks_non_empty,
            gutter_config_active,
            pointer_in_gutter_strip_now,
        );

        assert_eq!(
            resolution.mouse_tracking_active,
            inputs.mouse_tracking_active
        );
        assert_eq!(resolution.has_urls, expected_has_urls);
        assert_eq!(
            resolution.scroll_offset_nonzero,
            expected_scroll_offset_nonzero
        );
        assert_eq!(resolution.gutter_active, expected_gutter_active);
        assert_eq!(
            resolution.signals,
            Some(PointerMotionPaneSignals {
                mouse_tracking_active: inputs.mouse_tracking_active,
                hover_region_risk: expected_hover_region_risk,
            })
        );
    }

    #[test]
    fn resolve_pane_under_pointer_lookup_miss_resolves_to_no_pane() {
        // `pane_inputs` returning `None` models `PaneTree::find` returning
        // `None` for an id that hit-tested successfully but no longer
        // exists in the tree -- treated as "no pane", not a panic.
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let rect_a = Rect::from_min_max(point(0.0, 0.0), point(100.0, 100.0));
        let split_layout = [(pane_a, rect_a)];
        let central_rect = rect_a;
        let pos = point(50.0, 50.0);

        let resolution =
            resolve_pane_under_pointer(pos, central_rect, None, &split_layout, false, 0.0, |_| {
                None
            });

        assert_eq!(resolution, PaneResolution::unresolved());
    }
}

/// Subtask 124.13: the pointer-motion gate's suppression rate, per scenario.
///
/// **Measurement, not behaviour.** Nothing here changes a decision; it
/// drives the existing pure chain (`resolve_pane_under_pointer` ->
/// `PointerMotionInputs` -> `pointer_motion_needs_repaint_decision`) over a
/// deterministic sweep of pointer positions and counts what comes out.
///
/// It replaces the 2026-07-29 table, which predates the chrome cache being
/// disabled (121.32) and could not be refreshed by Task 123 — pointer motion
/// is not a renderer workload, and the Phase 1 harness drives the renderer
/// directly. Kept as tests rather than prose specifically so these numbers
/// cannot go stale unnoticed the way that table did.
///
/// # What is NOT measured here, stated rather than guessed
///
/// The **pointer event rate**. A suppression rate is a fraction of checks;
/// turning it into a CPU figure needs an events-per-second the compositor
/// determines and this harness cannot observe. Task 123 declined to guess
/// it and so does this. Per `PROFILING.md`, any CPU claim downstream of
/// these numbers must carry the rate it assumed.
///
/// Full results are in 124.13's findings block in
/// `Documents/PLAN_124_RENDER_EFFICIENCY.md`.
#[cfg(test)]
mod suppression_rates {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use freminal_common::geometry::{Rect, point};

    use super::{
        PaneResolution, PaneSnapshotInputs, PointerMotionInputs,
        pointer_motion_needs_repaint_decision, resolve_pane_under_pointer,
    };
    use crate::gui::panes::PaneIdGenerator;

    /// A window whose central (terminal) area is 1200x700 at (40, 60) —
    /// leaving a chrome band above it, so the sweep below covers both
    /// over-pane and over-chrome positions the way a real session does.
    const CENTRAL: Rect = Rect {
        min: point(40.0, 60.0),
        max: point(1240.0, 760.0),
    };

    /// The gutter's total inset in logical points, matching
    /// `PublishedFrameState::cached_gutter_inset_logical`'s order of
    /// magnitude.
    const GUTTER_INSET: f32 = 18.0;

    /// One scenario's outcome.
    struct Outcome {
        checks: u32,
        suppressed: u32,
    }

    impl Outcome {
        fn rate(&self) -> f64 {
            f64::from(self.suppressed) / f64::from(self.checks)
        }
    }

    /// Sweep a deterministic lattice of pointer positions over the whole
    /// window and count how many motion checks the gate suppresses.
    ///
    /// The lattice spans beyond `CENTRAL` on all sides, so positions over
    /// chrome (which resolve to no pane) are included at their natural
    /// proportion rather than being excluded to flatter the number.
    fn sweep(
        inputs: PaneSnapshotInputs,
        gutter_config_active: bool,
        any_pane_selecting: bool,
    ) -> Outcome {
        let mut ids = PaneIdGenerator::default();
        let pane_id = ids.next_id();
        let layout = [(pane_id, CENTRAL)];

        let mut checks = 0_u32;
        let mut suppressed = 0_u32;

        // 64 x 40 lattice over a region wider and taller than CENTRAL.
        for iy in 0..40 {
            for ix in 0..64 {
                let x = f64::from(ix) * 20.0;
                let y = f64::from(iy) * 20.0;
                #[allow(clippy::cast_possible_truncation)]
                let pos = point(x as f32, y as f32);

                let resolution: PaneResolution = resolve_pane_under_pointer(
                    pos,
                    CENTRAL,
                    None,
                    &layout,
                    gutter_config_active,
                    GUTTER_INSET,
                    |_| Some(inputs),
                );

                let needs_repaint = pointer_motion_needs_repaint_decision(PointerMotionInputs {
                    focus_change_pending: false,
                    chrome_interactive: false,
                    any_pane_selecting,
                    overlay_open: false,
                    pointer_pane_unresolved: false,
                    pane_signals: resolution.signals,
                });

                checks += 1;
                if !needs_repaint {
                    suppressed += 1;
                }
            }
        }

        Outcome { checks, suppressed }
    }

    const CLEAN: PaneSnapshotInputs = PaneSnapshotInputs {
        mouse_tracking_active: false,
        has_urls: false,
        scroll_offset: 0,
        is_alternate_screen: false,
        command_blocks_non_empty: false,
    };

    /// Baseline: nothing hover-sensitive on screen. Every check suppresses.
    #[test]
    fn a_clean_pane_suppresses_everything() {
        let out = sweep(CLEAN, false, false);
        assert_eq!(
            out.suppressed, out.checks,
            "with no veto active, motion anywhere must suppress"
        );
    }

    /// **The finding 124.3 rests on.** One hyperlink anywhere on screen
    /// takes suppression to exactly zero for every position inside the pane.
    ///
    /// `has_urls` is a **pane-wide, position-independent** veto: the precise
    /// cell-to-hyperlink hit test is render-pass-only state, so
    /// `pane_hover_region_risk` approximates "motion might enter or leave a
    /// URL span" as "motion anywhere in this pane". Nothing about where the
    /// pointer actually is can rescue it.
    ///
    /// The residual suppression is entirely positions **outside** the pane,
    /// which resolve to no pane at all.
    #[test]
    fn one_hyperlink_defeats_suppression_everywhere_inside_the_pane() {
        let inputs = PaneSnapshotInputs {
            has_urls: true,
            ..CLEAN
        };
        let clean = sweep(CLEAN, false, false);
        let urls = sweep(inputs, false, false);

        assert!(
            urls.rate() < 0.25,
            "one hyperlink should collapse suppression, got {:.2}%",
            urls.rate() * 100.0
        );
        assert!(
            clean.rate() > urls.rate() * 3.0,
            "clean {:.2}% versus one-hyperlink {:.2}%",
            clean.rate() * 100.0,
            urls.rate() * 100.0
        );
    }

    /// Scrollback offset is the second pane-wide veto, for the same reason:
    /// the scrollbar is interactive whenever scrolled back, and its track
    /// rect is render-pass-only state.
    #[test]
    fn a_nonzero_scroll_offset_is_also_a_pane_wide_veto() {
        let inputs = PaneSnapshotInputs {
            scroll_offset: 1,
            ..CLEAN
        };
        let scrolled = sweep(inputs, false, false);
        let urls = sweep(
            PaneSnapshotInputs {
                has_urls: true,
                ..CLEAN
            },
            false,
            false,
        );
        assert_eq!(
            scrolled.suppressed, urls.suppressed,
            "both vetoes are pane-wide, so they suppress identically"
        );
    }

    /// Mouse tracking defeats suppression too — and **correctly**. A
    /// terminal application receiving mouse reports must be sent every
    /// motion, so this one is not a target for 124.3.
    #[test]
    fn mouse_tracking_defeats_suppression_and_should() {
        let inputs = PaneSnapshotInputs {
            mouse_tracking_active: true,
            ..CLEAN
        };
        let out = sweep(inputs, false, false);
        let clean = sweep(CLEAN, false, false);
        assert!(out.suppressed < clean.suppressed);
    }

    /// An active selection drag suppresses nothing anywhere — the veto is
    /// window-level, not per-pane, so even positions over chrome lose.
    #[test]
    fn an_active_selection_drag_suppresses_nothing_at_all() {
        let out = sweep(CLEAN, false, true);
        assert_eq!(
            out.suppressed, 0,
            "any_pane_selecting is a window-level veto with no positional term"
        );
    }

    /// The gutter is the one veto that is already positional (the Task 121
    /// fix), so it costs only the strip itself rather than the whole pane.
    /// This is the shape 124.3 proposes to give the other two.
    #[test]
    fn the_gutter_veto_is_positional_and_therefore_cheap() {
        let inputs = PaneSnapshotInputs {
            command_blocks_non_empty: true,
            ..CLEAN
        };
        let out = sweep(inputs, true, false);
        let clean = sweep(CLEAN, false, false);

        assert!(
            out.suppressed < clean.suppressed,
            "the strip does cost some"
        );
        assert!(
            out.rate() > 0.9,
            "but only the strip: expected >90% still suppressed, got {:.2}%",
            out.rate() * 100.0
        );
    }
}
