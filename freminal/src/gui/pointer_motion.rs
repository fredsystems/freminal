// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The out-of-frame pointer-motion repaint decision (Task 122, subtask
//! 122.5a; rewritten to cell-granular positional suppression by Task
//! 124.3b).
//!
//! `freminal-windowing`'s pointer fast path calls
//! `App::pointer_motion_needs_repaint` on every `CursorMoved` event,
//! **outside any egui frame**, to decide whether that motion requires a
//! repaint at all. This module holds the pure decision chain behind that
//! call — pane resolution, cell-granular position classification, and the
//! composed true/false answer — factored out of `app_impl.rs` (subtask
//! 122.5) and then out of that file entirely (subtask 122.5a) because it is
//! one coherent concept distinct from the `App` trait impl: it is pure, it
//! runs before any frame exists, and it is headlessly unit-testable
//! without constructing a `FreminalGui`/`PerWindowState`/`PaneTree`/`Pane`/
//! `TerminalSnapshot`. Per `freminal-module-cohesion`, a concept this
//! self-contained gets a module whose path names it, rather than staying
//! folded into the file it had already outgrown.
//!
//! # Task 124.3b: from pane-wide vetoes to cell-granular positions
//!
//! Before this subtask, three of the predicate's terms were pane-WIDE
//! approximations: `has_urls`, `scroll_offset > 0`, and `any_pane_selecting`
//! each forced a repaint for ANY motion anywhere in (or, for selection,
//! anywhere in the whole window) the affected pane, because the exact
//! cell/pixel geometry a precise test would need was not available outside
//! a frame. Task 122.15/124.3a published that geometry
//! (`PanePointerReportInputs`: `terminal_rect`, `cell_size`, and now
//! `scrollbar_hit_rect`), and `App::pointer_motion_needs_repaint` itself now
//! carries the PREVIOUS pointer position alongside the current one
//! (`freminal_windowing::PointerMotionPositions`), so this module can ask
//! the much narrower question each of those vetoes actually needs answered:
//! did the motion cross a cell (or, for the scrollbar, a hit-rect) boundary
//! that matters?
//!
//! [`PointerRegion`] classifies one position within one pane's published
//! geometry; [`resolve_pane_under_pointer`] resolves a position to a pane
//! and a region in one pass (reusing the existing hit-test); and the
//! `*_positional_force` functions each answer one narrow question by
//! comparing the previous and current [`PointerObservation`]. The pane-wide
//! `has_urls`/`scroll_offset > 0` vetoes are gone entirely; the gutter's
//! already-positional test is generalized rather than duplicated.
//!
//! `App::pointer_motion_needs_repaint` and `App::is_chrome_interactive_at`
//! themselves stay in `app_impl.rs` — they are trait-impl methods needing
//! `&self` — and call into this module.
use conv2::ConvUtil;
use freminal_common::geometry::{Point, Rect};

use super::panes;

/// Where one pointer position falls relative to a pane's published
/// geometry (Task 124.3b).
///
/// `Content`/`Gutter` carry cell coordinates using the SAME floor semantics
/// as the live input path (`terminal/coords.rs`'s
/// `encode_egui_mouse_pos_as_usize`): `col`/`row` are
/// `floor((pos - origin) / cell_size)`, zero-based. `Outside` and `Unknown`
/// are deliberately distinct: `Outside` means the position is somewhere
/// real (inside the pane's rect, or simply not over any pane at all) that
/// is neither the terminal content grid nor the gutter strip — a
/// legitimate, precisely-known "nowhere interesting". `Unknown` means the
/// position could NOT be classified at all (no geometry has been published
/// for this pane yet, or the published cell size is non-finite/
/// non-positive) — see the `*_positional_force` functions' docs for why
/// `Unknown` may only ever cause a term to OVER-report, never suppress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PointerRegion {
    /// Inside the terminal content grid, at this zero-based display cell.
    Content {
        /// Zero-based display column.
        col: usize,
        /// Zero-based display row.
        row: usize,
    },
    /// Inside the command-block gutter's left-edge strip, at this zero-based
    /// row (using the SAME vertical origin/cell height as `Content` — the
    /// gutter and the terminal content grid share one row grid, only the
    /// horizontal strip differs).
    Gutter {
        /// Zero-based row, aligned with `Content`'s `row`.
        row: usize,
    },
    /// A real, precisely-known position that is neither `Content` nor
    /// `Gutter` — e.g. the padding gap between the gutter strip and the
    /// terminal content rect, or simply not over any pane at all.
    Outside,
    /// The position could not be classified: no geometry has been published
    /// for the candidate pane yet, or the published cell size is invalid.
    Unknown,
}

/// One pane's published position-classification geometry, sourced verbatim
/// from [`super::published_frame_state::PanePointerReportInputs`] — never
/// recomputed, so classification can never silently disagree with what the
/// pane actually rendered this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PanePositionGeometry {
    /// The pane's terminal content rect (post-gutter-inset), egui logical
    /// points — verbatim from `PanePointerReportInputs::terminal_rect`.
    pub(super) terminal_rect: Rect,
    /// One character cell's logical width, egui points — verbatim from
    /// `PanePointerReportInputs::cell_size.x`.
    pub(super) cell_width: f32,
    /// One character cell's logical height, egui points — verbatim from
    /// `PanePointerReportInputs::cell_size.y`.
    pub(super) cell_height: f32,
    /// The scrollbar's exact hit rect this frame, or `None` when the thumb
    /// did not render — verbatim from
    /// `PanePointerReportInputs::scrollbar_hit_rect`.
    pub(super) scrollbar_hit_rect: Option<Rect>,
}

/// Is `v` usable as a cell width/height for position classification?
///
/// Finite and strictly positive — the two properties floor-division needs
/// to produce a meaningful, non-negative, non-infinite cell index.
const fn is_valid_cell_dimension(v: f32) -> bool {
    v.is_finite() && v > 0.0
}

/// Classify `pos` (already known to belong to the pane described by
/// `pane_rect`) against that pane's published `geometry`.
///
/// `pane_rect` is the pane's full rect (from the split/zoomed layout);
/// `geometry` is `None` when the pane has never published
/// `PanePointerReportInputs` (e.g. it has not rendered a single frame yet).
///
/// A position outside `pane_rect` entirely is [`PointerRegion::Outside`]
/// (relevant when `resolve_pane_under_pointer` classifies the "no pane
/// resolved" case, or when a test constructs `pos` outside the rect
/// directly). Pure, so directly unit-testable.
///
/// # Review fix: exact geometry, not an upper-bound approximation
///
/// `Content` requires `geometry.terminal_rect.contains(pos)` EXACTLY —
/// previously this only tested `pos.x >= terminal_rect.min.x`, which
/// classified any position past the terminal rect's left edge as `Content`
/// regardless of its `y` (or how far past `terminal_rect.max.x`/`max.y` it
/// fell) as long as it was still inside `pane_rect`. `Rect::contains` is
/// inclusive on all four edges (see its doc), so a position exactly on the
/// `terminal_rect`/gutter-strip shared boundary resolves to `Content`
/// (tested first), matching the pre-review boundary behavior of "the
/// shared edge belongs to content, not gutter".
///
/// # Review fix (item 3): gutter classification mirrors `compute_command_block_hover_rows` exactly
///
/// `Gutter` does NOT use `Rect::contains` (inclusive on all four edges) —
/// it uses the SAME half-open test
/// `terminal/widget.rs::compute_command_block_hover_rows` applies to the
/// live hover trigger: `pane_rect.min.x <= x < terminal_rect.min.x` (the
/// pane's left edge through the terminal rect's left edge, exclusive) AND
/// `terminal_rect.min.y <= y < terminal_rect.max.y` (the terminal rect's
/// own vertical span, exclusive on the bottom). A position on the gutter
/// strip's own bottom (max-Y) edge is therefore `Outside`, not `Gutter`
/// (unless some other classification claims it first) — an inclusive
/// `contains` would wrongly classify the pixel row immediately below the
/// strip (which `compute_command_block_hover_rows` already treats as
/// no-hover) as still inside it. The strip's right edge — the x
/// coordinate it shares with `terminal_rect`'s left edge — belongs to
/// `Content` instead (checked first, above): that boundary was never in
/// dispute between the two functions.
/// Not the
/// `PublishedFrameState::cached_gutter_inset_logical` upper-bound
/// approximation an earlier version used (that value, and the
/// `pointer_in_gutter_strip` helper it fed, are gone: the exact
/// `terminal_rect` this function already receives makes the approximation
/// unnecessary).
pub(super) fn classify_pointer_position(
    pos: Point,
    pane_rect: Rect,
    geometry: Option<PanePositionGeometry>,
) -> PointerRegion {
    if !pane_rect.contains(pos) {
        return PointerRegion::Outside;
    }
    let Some(geometry) = geometry else {
        return PointerRegion::Unknown;
    };
    if !is_valid_cell_dimension(geometry.cell_width)
        || !is_valid_cell_dimension(geometry.cell_height)
    {
        return PointerRegion::Unknown;
    }

    let terminal_rect = geometry.terminal_rect;
    if terminal_rect.contains(pos) {
        let col = ((pos.x - terminal_rect.min.x) / geometry.cell_width).floor();
        let row = ((pos.y - terminal_rect.min.y) / geometry.cell_height).floor();
        return match (col.approx_as::<usize>(), row.approx_as::<usize>()) {
            (Ok(col), Ok(row)) => PointerRegion::Content { col, row },
            _ => PointerRegion::Unknown,
        };
    }

    // Half-open, NOT `Rect::contains` — see this function's doc (review fix,
    // item 3). Must stay byte-for-byte equivalent to
    // `compute_command_block_hover_rows`'s own bounds check.
    let in_gutter_strip = pos.x >= pane_rect.min.x
        && pos.x < terminal_rect.min.x
        && pos.y >= terminal_rect.min.y
        && pos.y < terminal_rect.max.y;
    if in_gutter_strip {
        let row = ((pos.y - terminal_rect.min.y) / geometry.cell_height).floor();
        return row
            .approx_as::<usize>()
            .map_or(PointerRegion::Unknown, |row| PointerRegion::Gutter { row });
    }

    PointerRegion::Outside
}

/// Subtask 121.14: pure composition of the "some animation is in flight
/// somewhere in this window, independent of pointer position" term used by
/// `pointer_motion_needs_repaint`. Extracted so it is unit-testable without
/// a live `FreminalGui`/`PerWindowState`. Trivial (an OR of two already-
/// computed booleans), but named and tested on its own so the composition
/// itself — as distinct from how each term is computed — is pinned.
pub(super) const fn animation_in_flight_composed(
    resize_overlay_animating: bool,
    toast_animating: bool,
) -> bool {
    resize_overlay_animating || toast_animating
}

/// The per-pane values [`resolve_pane_under_pointer`] needs for whichever
/// pane (if any) resolves under a pointer position — a stand-in for the
/// fields read off `pane.arc_swap.load()`'s `TerminalSnapshot` plus that
/// pane's published [`PanePositionGeometry`], passed through a lookup
/// closure so the resolution chain itself needs no `&PaneTree`/`&Pane` and
/// is headlessly unit-testable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PaneSnapshotInputs {
    /// `TerminalSnapshot::has_urls`.
    pub(super) has_urls: bool,
    /// Whether the command-block gutter is eligible for this pane THIS
    /// frame: `gutter_config_active && !is_alternate_screen &&
    /// !command_blocks.is_empty()`.
    ///
    /// Computed by the caller at the app boundary (`app_impl.rs`'s
    /// `resolve_pointer_observation_for`, which has `self.config` in
    /// scope) as an independent named field here, rather than as a `bool`
    /// PARAMETER on [`resolve_pane_under_pointer`] — review fix:
    /// `freminal-state-representation` forbids bool parameters outright,
    /// and the three source facts this ANDs together
    /// (`gutter_config_active`/`is_alternate_screen`/`command_blocks_non_empty`)
    /// have no other consumer, so folding them into one field the resolver
    /// reads verbatim is the correct shape rather than threading three
    /// separate values (or the pre-fold `bool` parameter) through the pure
    /// resolution chain.
    pub(super) gutter_eligible: bool,
    /// This pane's published position-classification geometry, or `None`
    /// if it has never published one.
    pub(super) geometry: Option<PanePositionGeometry>,
}

/// One resolved pointer position: which pane (if any) it falls over, and
/// where within that pane's geometry it classifies to, plus the per-pane
/// facts the `*_positional_force` functions need to answer their one
/// question each.
///
/// Built by [`resolve_pane_under_pointer`] for the previous and current
/// pointer position independently (against the SAME frame's layout), then
/// compared pairwise by the `*_positional_force` functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PointerObservation {
    /// Which pane the position resolved to, or `None` when it is not over
    /// any pane at all (inter-pane padding, or no layout could be
    /// hit-tested).
    pub(super) pane: Option<panes::PaneId>,
    /// Where the position classifies to within that pane's geometry (or
    /// [`PointerRegion::Outside`] when `pane` is `None`).
    pub(super) region: PointerRegion,
    /// The resolved pane's `TerminalSnapshot::has_urls`, or `false` when no
    /// pane resolved.
    pub(super) has_urls: bool,
    /// Whether the command-block gutter is eligible for the resolved pane
    /// THIS frame (`PaneSnapshotInputs::gutter_eligible`, verbatim), or
    /// `false` when no pane resolved.
    pub(super) gutter_eligible: bool,
    /// The pane whose published `scrollbar_hit_rect` this position falls
    /// inside, or `None` when it falls inside no pane's scrollbar hit rect
    /// at all (including when no pane resolved, or the resolved pane has
    /// no scrollbar currently rendered).
    ///
    /// Review fix: carries PANE IDENTITY, not just a bare `bool` — a plain
    /// "am I inside *a* scrollbar hit rect" flag cannot distinguish moving
    /// from pane A's scrollbar into pane B's scrollbar (both `true`, so a
    /// bare-`bool` comparison would wrongly suppress) from staying inside
    /// the SAME pane's scrollbar the whole time (also both `true`, and the
    /// case that must actually suppress). See
    /// [`scrollbar_boundary_force`]'s doc for the exact comparison this
    /// field exists to make correct.
    pub(super) scrollbar_pane: Option<panes::PaneId>,
}

impl PointerObservation {
    /// The "no pane resolved" result: legitimately [`PointerRegion::Outside`]
    /// (not `Unknown` — the position is not over any pane at all, which is
    /// precisely known, not undetermined), every other field at its
    /// quiet/non-forcing value.
    pub(super) const fn no_pane() -> Self {
        Self {
            pane: None,
            region: PointerRegion::Outside,
            has_urls: false,
            gutter_eligible: false,
            scrollbar_pane: None,
        }
    }

    /// The "could not resolve at all" result: [`PointerRegion::Unknown`],
    /// every other field at its quiet value.
    ///
    /// Review fix: used where the pre-review code returned [`Self::no_pane`]
    /// for a case that is NOT a legitimate "nowhere interesting" — the
    /// resolution chain found something real but could not read it. Two
    /// distinct call sites need exactly this:
    ///
    /// - `resolve_pane_under_pointer`'s pane-lookup miss: the hit-tested
    ///   rect resolved to a real `pane_id` from THIS SAME frame's layout,
    ///   but looking that id up (`PaneTree::find`) returned `None` — the
    ///   tree changed between building the layout and this lookup, a bug
    ///   state, not "the pointer is over nothing".
    /// - `app_impl.rs`'s `pointer_motion_needs_repaint`, when
    ///   `PaneTree::layout` itself returns `Err` (an empty tree, also a bug
    ///   state) — rather than silently substituting an empty layout (which
    ///   would resolve every position to [`Self::no_pane`] and could
    ///   therefore suppress), both endpoints become `unresolved()`, and
    ///   [`unknown_geometry_force`] turns that into an unconditional force.
    pub(super) const fn unresolved() -> Self {
        Self {
            pane: None,
            region: PointerRegion::Unknown,
            has_urls: false,
            gutter_eligible: false,
            scrollbar_pane: None,
        }
    }
}

/// The pane-resolution chain behind `FreminalGui::pointer_motion_needs_repaint`
/// — layout -> hit-test -> snapshot lookup -> classification — extracted as
/// a pure function so it is headlessly unit-testable without a live
/// `FreminalGui`/`PerWindowState`/`PaneTree`/`Pane`/`TerminalSnapshot`.
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
/// pointer outside `central_rect` entirely (e.g. still over chrome)
/// resolves to no pane even while zoomed, exactly as the split branch's
/// `.contains(pos)` check would.
///
/// `pane_inputs` stands in for `active_tab.pane_tree.find(pane_id)` +
/// `pane.arc_swap.load()` + the pane's published `PanePointerReportInputs`:
/// a lookup from a resolved [`panes::PaneId`] to that pane's current
/// [`PaneSnapshotInputs`]. Returning `None` here means [`PaneTree::find`]
/// returned `None` for an id THIS SAME call's own hit-test just produced
/// from `split_layout`/`zoomed_pane` — the tree changed out from under the
/// layout, a bug state — so this resolves to [`PointerObservation::unresolved`]
/// (review fix: previously `no_pane`, which is reserved for the legitimately
/// "nowhere interesting" case; see that method's doc for why the distinction
/// matters).
///
/// Review fix: no longer takes a `gutter_config_active: bool` /
/// `gutter_width_upper_bound_logical: f32` pair — gutter eligibility is now
/// [`PaneSnapshotInputs::gutter_eligible`], a field computed by the caller
/// at the app boundary, and gutter geometry classification uses the exact
/// published `terminal_rect`/`pane_rect` (see [`classify_pointer_position`]'s
/// doc) rather than an upper-bound approximation. This function therefore
/// takes no `bool` parameters at all (`freminal-state-representation`).
///
/// Task 124.3b: this is called TWICE per `pointer_motion_needs_repaint`
/// call — once for the previous pointer position, once for the current —
/// against the SAME frame's `split_layout`/`zoomed_pane` (review fix: now
/// computed exactly ONCE by the caller and borrowed for both calls, rather
/// than each call recomputing `PaneTree::layout` independently), so the two
/// resulting [`PointerObservation`]s are directly comparable by the
/// `*_positional_force` functions.
pub(super) fn resolve_pane_under_pointer(
    pos: Point,
    central_rect: Rect,
    zoomed_pane: Option<panes::PaneId>,
    split_layout: &[(panes::PaneId, Rect)],
    pane_inputs: impl Fn(panes::PaneId) -> Option<PaneSnapshotInputs>,
) -> PointerObservation {
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
        return PointerObservation::no_pane();
    };

    let Some(inputs) = pane_inputs(pane_id) else {
        return PointerObservation::unresolved();
    };

    let region = classify_pointer_position(pos, pane_rect, inputs.geometry);
    let scrollbar_pane = inputs
        .geometry
        .and_then(|g| g.scrollbar_hit_rect)
        .is_some_and(|r| r.contains(pos))
        .then_some(pane_id);

    PointerObservation {
        pane: Some(pane_id),
        region,
        has_urls: inputs.has_urls,
        gutter_eligible: inputs.gutter_eligible,
        scrollbar_pane,
    }
}

/// The previous and current [`PointerObservation`] for one `CursorMoved`
/// call, resolved against the SAME frame's layout by
/// `app_impl.rs`'s `resolve_pointer_observations`.
///
/// # Review fix: named fields, not a same-typed tuple
///
/// `resolve_pointer_observations` previously returned
/// `(PointerObservation, PointerObservation)` — a bare tuple of two values
/// of the IDENTICAL type, with no way for the compiler to catch a
/// transposed `(current, previous)` vs `(previous, current)` at either the
/// return site or the call site. This type gives each position a name;
/// composing it (`PointerObservations { previous, current }`) and reading
/// it back (`observations.previous`/`observations.current`) makes a swap a
/// visible field-name mismatch instead of a silent type-checked transpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PointerObservations {
    /// The pointer's PREVIOUS resolved position, or a copy of `current` when
    /// there was no previous position at all (first motion — see
    /// `freminal_windowing::PointerMotionPositions`'s doc).
    pub(super) previous: PointerObservation,
    /// The pointer's CURRENT resolved position.
    pub(super) current: PointerObservation,
}

/// How many panes in the active tab currently have an in-progress selection
/// drag (`ViewState::selection.is_selecting`) — named domain enum
/// (`freminal-state-representation`) rather than a raw count, since
/// [`selection_positional_force`] only ever needs to distinguish these
/// three cases, and `One` needs to carry which pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectingPanes {
    /// No pane is selecting.
    None,
    /// Exactly one pane is selecting.
    One(panes::PaneId),
    /// More than one pane is selecting — a bug state (a single pointer
    /// cannot legitimately drive two simultaneous selection drags), treated
    /// conservatively.
    Multiple,
}

/// Do `a` and `b` resolve to the SAME pane, both classified as the SAME
/// [`PointerRegion::Content`] cell?
///
/// The shared "did motion stay within one content cell of one pane"
/// predicate behind both [`url_positional_force`] and
/// [`selection_positional_force`]. `false` whenever either side is not
/// `Content` (including `Outside`/`Unknown`/`Gutter`) or the two panes
/// differ — there is no partial credit.
fn endpoints_share_content_cell(a: PointerObservation, b: PointerObservation) -> bool {
    match (a.pane, b.pane) {
        (Some(pane_a), Some(pane_b)) if pane_a == pane_b => matches!(
            (a.region, b.region),
            (
                PointerRegion::Content { col: c1, row: r1 },
                PointerRegion::Content { col: c2, row: r2 },
            ) if c1 == c2 && r1 == r2
        ),
        _ => false,
    }
}

/// Do `a` and `b` resolve to the SAME pane, both classified as the SAME
/// [`PointerRegion::Gutter`] row? The gutter analogue of
/// [`endpoints_share_content_cell`], behind [`gutter_positional_force`].
fn endpoints_share_gutter_row(a: PointerObservation, b: PointerObservation) -> bool {
    match (a.pane, b.pane) {
        (Some(pane_a), Some(pane_b)) if pane_a == pane_b => matches!(
            (a.region, b.region),
            (PointerRegion::Gutter { row: r1 }, PointerRegion::Gutter { row: r2 }) if r1 == r2
        ),
        _ => false,
    }
}

/// Task 124.3b: does URL hover state need a repaint for this motion?
///
/// Replaces the old pane-wide `has_urls` veto (which forced a repaint for
/// ANY motion anywhere in a pane containing at least one hyperlink) with
/// the cell-granular test the URL hit test itself actually needs: hover
/// only changes when the pointer crosses into, out of, or between
/// hyperlink-relevant cells.
///
/// `false` unless at least one endpoint's resolved pane currently has URLs
/// (`PointerObservation::has_urls`) — a pane with no hyperlinks at all can
/// never have its URL-hover state change, regardless of position. When
/// that precondition holds: `false` (suppress) only when both endpoints
/// resolve to the SAME pane at the SAME `Content` cell
/// ([`endpoints_share_content_cell`]); `true` (force) for a changed cell,
/// entering/leaving `Content` (including via `Outside`/`Unknown`), or
/// crossing away to a different pane.
pub(super) fn url_positional_force(prev: PointerObservation, curr: PointerObservation) -> bool {
    (prev.has_urls || curr.has_urls) && !endpoints_share_content_cell(prev, curr)
}

/// Task 124.3b: does the command-block gutter's hover state need a repaint
/// for this motion?
///
/// Generalizes the Task 121 fix's already-positional single-position gutter
/// test to the previous/current pair. Relevant only when at least one
/// endpoint is classified [`PointerRegion::Gutter`] AND that endpoint's own
/// pane is gutter-eligible this frame
/// (`PointerObservation::gutter_eligible`: config on, not the alternate
/// screen, at least one command block) — motion confined to `Content` in an
/// eligible pane never touches this term at all. When relevant: `false`
/// only when both endpoints resolve to the SAME pane at the SAME `Gutter`
/// row ([`endpoints_share_gutter_row`]); `true` for a row change,
/// entering/leaving the gutter, or a pane change.
pub(super) fn gutter_positional_force(prev: PointerObservation, curr: PointerObservation) -> bool {
    let relevant = (matches!(prev.region, PointerRegion::Gutter { .. }) && prev.gutter_eligible)
        || (matches!(curr.region, PointerRegion::Gutter { .. }) && curr.gutter_eligible);
    relevant && !endpoints_share_gutter_row(prev, curr)
}

/// Task 124.3b: does the scrollbar's hover state need a repaint for this
/// motion?
///
/// Replaces the old pane-wide `scroll_offset > 0` veto with an exact
/// boundary-crossing test against `PointerObservation::scrollbar_pane` —
/// WHICH pane's `scrollbar_hit_rect` (if any) each endpoint falls inside,
/// not merely whether it falls inside *some* pane's. `false` (suppress)
/// only when both endpoints agree exactly: the SAME `Some(pane)`, or both
/// `None`. Every other transition forces — entering a scrollbar from
/// outside any (`None` -> `Some`), leaving one (`Some` -> `None`), AND
/// crossing directly from one pane's scrollbar into a different pane's
/// (`Some(a)` -> `Some(b)`, `a != b`).
///
/// # Review fix: pane identity, not a bare bool
///
/// The pre-review version compared a bare `bool` ("is this endpoint inside
/// *a* scrollbar hit rect"), which cannot distinguish "moved from pane A's
/// scrollbar into pane B's scrollbar" (both `true` under the bare-bool
/// test, so it would have wrongly suppressed) from "stayed inside pane A's
/// scrollbar the whole time" (also both `true`, and the case that
/// genuinely should suppress). `Option<PaneId>` equality catches both
/// correctly: `Some(a) != Some(b)` when `a != b`, but `Some(a) == Some(a)`.
///
/// This does NOT cover an in-progress scrollbar DRAG, which is
/// unconditional and computed separately (see
/// `App::pointer_motion_needs_repaint`'s composition of
/// `PointerMotionInputs::scrollbar_drag_forced`) because a drag can
/// continue moving the pointer outside the dragging pane's own rect.
pub(super) fn scrollbar_boundary_force(prev: PointerObservation, curr: PointerObservation) -> bool {
    prev.scrollbar_pane != curr.scrollbar_pane
}

/// Task 124.3b: does an in-progress selection drag need a repaint for this
/// motion?
///
/// Replaces the old window-level `any_pane_selecting` veto (which forced a
/// repaint for ANY motion anywhere in the window, including over chrome,
/// whenever any pane was selecting) with a per-drag positional test.
///
/// `SelectingPanes::None` never forces. `SelectingPanes::Multiple` — more
/// than one pane simultaneously selecting, a bug state a single pointer
/// cannot legitimately produce — forces unconditionally (conservative).
/// `SelectingPanes::One(pane)`: forces UNLESS both endpoints resolve to
/// `pane` at the SAME `Content` cell ([`endpoints_share_content_cell`],
/// which already requires equal panes). This deliberately also forces when
/// either endpoint is NOT `pane` at all (a stale `is_selecting` left set on
/// a background tab's pane by an interrupted drag is exactly the case the
/// old window-level veto's conservatism existed to cover, and the
/// positional test preserves that by forcing rather than guessing at a
/// cell comparison that cannot mean anything for an unrelated pane).
pub(super) fn selection_positional_force(
    prev: PointerObservation,
    curr: PointerObservation,
    selecting: SelectingPanes,
) -> bool {
    match selecting {
        SelectingPanes::None => false,
        SelectingPanes::Multiple => true,
        SelectingPanes::One(pane) => {
            let both_at_selecting_pane = prev.pane == Some(pane) && curr.pane == Some(pane);
            !(both_at_selecting_pane && endpoints_share_content_cell(prev, curr))
        }
    }
}

/// Task 124.3b: `Unknown` geometry may only ever cause a repaint, never
/// suppress one.
///
/// `true` iff either endpoint classified to [`PointerRegion::Unknown`]
/// (missing/invalid published geometry for its resolved pane). This is a
/// deliberate backstop layered on top of the other `*_positional_force`
/// functions rather than relied upon to fall out of them implicitly: an
/// `Unknown` region can never equal a `Content`/`Gutter` region in the
/// `endpoints_share_*` helpers (so URL/gutter/selection already force
/// whenever their OWN precondition — `has_urls`/gutter-eligible/a matching
/// selecting pane — happens to hold), but a motion with NONE of those
/// preconditions active would otherwise suppress on `Unknown` alone. This
/// term closes that gap explicitly.
pub(super) const fn unknown_geometry_force(
    prev: PointerObservation,
    curr: PointerObservation,
) -> bool {
    matches!(prev.region, PointerRegion::Unknown) || matches!(curr.region, PointerRegion::Unknown)
}

/// The composed inputs to [`pointer_motion_needs_repaint_decision`].
///
/// See that function's doc for what each term means and why it forces a
/// repaint. This type exists so those terms are named at the call site; it
/// carries no logic of its own.
///
/// [`Default`] is `all-clear` — every term `false` — which is the "plain
/// motion within one cell of the already-active pane's terminal content"
/// case the gate exists to suppress. Tests construct the interesting cases
/// with struct-update syntax off it, so each test names only the one term
/// it is about.
// struct_excessive_bools: each field is an INDEPENDENT forcing condition
// that can be true simultaneously with any other -- the "independent
// simultaneous signals" case in `state-representation`, not a state machine
// masquerading as bools. Mirrors `window.rs`'s `PointerMotionConditionFlags`,
// which carries the allow for the same reason.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct PointerMotionInputs {
    /// This is the first `CursorMoved` event since window creation, or
    /// since the pointer last left the window (`previous` position
    /// unknown) — conservative: cannot compare against a prior position at
    /// all.
    pub(super) first_motion: bool,
    /// Focus-follows-mouse is enabled and the pointer is over a pane that is
    /// not the active one, so this motion has a focus switch to apply.
    pub(super) focus_change_pending: bool,
    /// `App::is_chrome_interactive_at` said so (menu bar, tab bar,
    /// split-border drag sensor).
    pub(super) chrome_interactive: bool,
    /// Some UI overlay/popup/tooltip/context menu is open this window, or
    /// an animation (toast fade, resize-overlay HUD) is in flight.
    pub(super) overlay_open: bool,
    /// The pane under the pointer could not be determined at all —
    /// conservative "unknown", distinct from "no pane here".
    pub(super) pointer_pane_unresolved: bool,
    /// Either endpoint's geometry could not be classified at all
    /// ([`unknown_geometry_force`]).
    pub(super) unknown_geometry: bool,
    /// [`url_positional_force`]'s answer.
    pub(super) url_forced: bool,
    /// [`gutter_positional_force`]'s answer.
    pub(super) gutter_forced: bool,
    /// [`scrollbar_boundary_force`]'s answer.
    pub(super) scrollbar_boundary_forced: bool,
    /// A scrollbar drag is in progress for at least one pane in this
    /// window — unconditional, regardless of where the pointer currently
    /// resolves (see `scrollbar_boundary_force`'s doc for why a drag cannot
    /// be tied to the resolved pane alone).
    pub(super) scrollbar_drag_forced: bool,
    /// [`selection_positional_force`]'s answer.
    pub(super) selection_forced: bool,
}

/// The composed decision behind `App::pointer_motion_needs_repaint`'s
/// freminal-side implementation (Task 124.3b). Extracted as a pure function
/// over already-computed signals so it is unit-testable without a live
/// `FreminalGui`/windowing stack.
///
/// Returns `true` (a repaint is needed) if ANY [`PointerMotionInputs`] field
/// is `true`. Every field is already named and documented on that type;
/// this function is a plain disjunction, kept deliberately free of any
/// logic of its own so the composition itself cannot silently diverge from
/// the field list.
pub(super) const fn pointer_motion_needs_repaint_decision(inputs: PointerMotionInputs) -> bool {
    inputs.first_motion
        || inputs.focus_change_pending
        || inputs.chrome_interactive
        || inputs.overlay_open
        || inputs.pointer_pane_unresolved
        || inputs.unknown_geometry
        || inputs.url_forced
        || inputs.gutter_forced
        || inputs.scrollbar_boundary_forced
        || inputs.scrollbar_drag_forced
        || inputs.selection_forced
}

#[cfg(test)]
mod tests {
    use freminal_common::geometry::{Rect, point};

    use super::{
        PanePositionGeometry, PaneSnapshotInputs, PointerMotionInputs, PointerObservation,
        PointerRegion, SelectingPanes, animation_in_flight_composed, classify_pointer_position,
        gutter_positional_force, pointer_motion_needs_repaint_decision, resolve_pane_under_pointer,
        scrollbar_boundary_force, selection_positional_force, unknown_geometry_force,
        url_positional_force,
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

    // ── `classify_pointer_position` ───────────────────────────────────────

    fn geometry() -> PanePositionGeometry {
        PanePositionGeometry {
            terminal_rect: Rect::from_min_max(point(20.0, 0.0), point(120.0, 100.0)),
            cell_width: 10.0,
            cell_height: 20.0,
            scrollbar_hit_rect: None,
        }
    }

    fn pane_rect() -> Rect {
        Rect::from_min_max(point(0.0, 0.0), point(120.0, 100.0))
    }

    #[test]
    fn classify_content_cell_at_origin() {
        let region = classify_pointer_position(point(20.0, 0.0), pane_rect(), Some(geometry()));
        assert_eq!(region, PointerRegion::Content { col: 0, row: 0 });
    }

    #[test]
    fn classify_content_floors_within_a_cell() {
        // Cell width 10, height 20: (25, 15) relative to terminal origin
        // (20, 0) is (5, 15) -> floor(5/10)=0, floor(15/20)=0.
        let region = classify_pointer_position(point(25.0, 15.0), pane_rect(), Some(geometry()));
        assert_eq!(region, PointerRegion::Content { col: 0, row: 0 });

        // (31, 15) relative is (11, 15) -> col 1, row 0.
        let region = classify_pointer_position(point(31.0, 15.0), pane_rect(), Some(geometry()));
        assert_eq!(region, PointerRegion::Content { col: 1, row: 0 });
    }

    #[test]
    fn classify_gutter_row_left_of_terminal_rect() {
        // Gutter strip is [pane_rect.min.x=0, terminal_rect.min.x=20).
        let region = classify_pointer_position(point(5.0, 25.0), pane_rect(), Some(geometry()));
        assert_eq!(region, PointerRegion::Gutter { row: 1 });
    }

    #[test]
    fn classify_content_wins_the_shared_boundary_with_gutter() {
        // Review fix: `Content` is tested (and matched) FIRST, so a
        // position exactly on the terminal_rect/gutter_rect shared edge
        // (x == terminal_rect.min.x == 20) resolves to Content, not
        // Gutter -- `Rect::contains` is inclusive on all four edges, so
        // without this ordering the shared boundary would be ambiguous.
        let region = classify_pointer_position(point(20.0, 25.0), pane_rect(), Some(geometry()));
        assert_eq!(region, PointerRegion::Content { col: 0, row: 1 });
    }

    #[test]
    fn classify_gutter_just_left_of_the_shared_boundary() {
        let region = classify_pointer_position(point(19.999, 25.0), pane_rect(), Some(geometry()));
        assert_eq!(region, PointerRegion::Gutter { row: 1 });
    }

    #[test]
    fn classify_gutter_inclusive_at_pane_rects_left_edge() {
        let region = classify_pointer_position(point(0.0, 5.0), pane_rect(), Some(geometry()));
        assert_eq!(region, PointerRegion::Gutter { row: 0 });
    }

    #[test]
    fn classify_gutter_bottom_edge_is_outside_not_gutter() {
        // Review fix (item 3): the gutter strip is half-open on `y`, exactly
        // like `compute_command_block_hover_rows`'s own
        // `y >= terminal_rect.max.y` bound -- a position exactly on
        // `terminal_rect.max.y` (100, this fixture), even though it is
        // within the gutter's x range [0, 20), is Outside, not Gutter.
        // `Rect::contains` (inclusive on all four edges) would have put
        // this in Gutter; this function deliberately does not use it here.
        let region = classify_pointer_position(point(5.0, 100.0), pane_rect(), Some(geometry()));
        assert_eq!(region, PointerRegion::Outside);
    }

    #[test]
    fn classify_outside_when_pos_is_outside_pane_rect() {
        let region = classify_pointer_position(point(500.0, 500.0), pane_rect(), Some(geometry()));
        assert_eq!(region, PointerRegion::Outside);
    }

    #[test]
    fn classify_unknown_when_geometry_is_missing() {
        let region = classify_pointer_position(point(30.0, 10.0), pane_rect(), None);
        assert_eq!(region, PointerRegion::Unknown);
    }

    #[test]
    fn classify_unknown_when_cell_width_is_zero() {
        let bad = PanePositionGeometry {
            cell_width: 0.0,
            ..geometry()
        };
        let region = classify_pointer_position(point(30.0, 10.0), pane_rect(), Some(bad));
        assert_eq!(region, PointerRegion::Unknown);
    }

    #[test]
    fn classify_unknown_when_cell_height_is_non_finite() {
        let bad = PanePositionGeometry {
            cell_height: f32::NAN,
            ..geometry()
        };
        let region = classify_pointer_position(point(30.0, 10.0), pane_rect(), Some(bad));
        assert_eq!(region, PointerRegion::Unknown);
    }

    #[test]
    fn classify_unknown_when_cell_width_is_negative() {
        let bad = PanePositionGeometry {
            cell_width: -10.0,
            ..geometry()
        };
        let region = classify_pointer_position(point(30.0, 10.0), pane_rect(), Some(bad));
        assert_eq!(region, PointerRegion::Unknown);
    }

    // ── Review fix: exact `terminal_rect` containment, not an upper bound ──
    //
    // A pane where the terminal content rect leaves real padding on the
    // pane's right and bottom edges too (not just the left/gutter side), so
    // a position past `terminal_rect.max.x`/`max.y` but still inside
    // `pane_rect` is representable at all.

    fn padded_geometry() -> PanePositionGeometry {
        PanePositionGeometry {
            terminal_rect: Rect::from_min_max(point(20.0, 0.0), point(100.0, 80.0)),
            cell_width: 10.0,
            cell_height: 20.0,
            scrollbar_hit_rect: None,
        }
    }

    fn padded_pane_rect() -> Rect {
        Rect::from_min_max(point(0.0, 0.0), point(120.0, 100.0))
    }

    #[test]
    fn classify_outside_beyond_terminal_rect_max_x_even_though_inside_pane_rect() {
        // Before the review fix, `Content` only tested `pos.x >=
        // terminal_rect.min.x`, so x=110 (past terminal_rect.max.x=100 but
        // still inside pane_rect's max.x=120) was wrongly classified as
        // Content. The fix requires exact `terminal_rect.contains(pos)`.
        let region = classify_pointer_position(
            point(110.0, 10.0),
            padded_pane_rect(),
            Some(padded_geometry()),
        );
        assert_eq!(region, PointerRegion::Outside);
    }

    #[test]
    fn classify_outside_beyond_terminal_rect_max_y_even_though_inside_pane_rect() {
        let region = classify_pointer_position(
            point(30.0, 90.0),
            padded_pane_rect(),
            Some(padded_geometry()),
        );
        assert_eq!(region, PointerRegion::Outside);
    }

    #[test]
    fn classify_content_inclusive_at_terminal_rects_far_corner() {
        // (100, 80) is exactly `terminal_rect.max` -- inclusive, per
        // `Rect::contains`'s doc.
        let region = classify_pointer_position(
            point(100.0, 80.0),
            padded_pane_rect(),
            Some(padded_geometry()),
        );
        // floor((100-20)/10) = 8, floor((80-0)/20) = 4.
        assert_eq!(region, PointerRegion::Content { col: 8, row: 4 });
    }

    #[test]
    fn classify_gutter_exact_bottom_edge_is_outside_with_padding() {
        // x=5 is within the gutter strip [0, 20); y=80 is exactly
        // `terminal_rect.max.y` (padded fixture) -- half-open (item 3), so
        // this is the boundary case itself, not merely "further past" it
        // like the sibling test below.
        let region = classify_pointer_position(
            point(5.0, 80.0),
            padded_pane_rect(),
            Some(padded_geometry()),
        );
        assert_eq!(region, PointerRegion::Outside);
    }

    #[test]
    fn classify_outside_gutter_x_range_but_beyond_terminal_rects_y_bounds() {
        // x=5 is within the gutter strip [0, 20); y=90 is past
        // terminal_rect.max.y=80 but still inside pane_rect (max.y=100).
        // Review fix: the gutter strip shares terminal_rect's Y bounds
        // exactly, so this is Outside, not Gutter.
        let region = classify_pointer_position(
            point(5.0, 90.0),
            padded_pane_rect(),
            Some(padded_geometry()),
        );
        assert_eq!(region, PointerRegion::Outside);
    }

    // ── `resolve_pane_under_pointer` ───────────────────────────────────

    /// A `PaneSnapshotInputs` with every field at its "quiet" (non-forcing)
    /// value.
    fn quiet_inputs() -> PaneSnapshotInputs {
        PaneSnapshotInputs {
            has_urls: false,
            gutter_eligible: false,
            geometry: Some(geometry()),
        }
    }

    #[test]
    fn resolve_pane_under_pointer_zoomed_hit_tests_the_zoomed_pane_not_the_split_layout() {
        let mut id_gen = PaneIdGenerator::new(0);
        let split_pane = id_gen.next_id();
        let zoomed_pane = id_gen.next_id();

        let central_rect = Rect::from_min_max(point(0.0, 0.0), point(100.0, 100.0));
        let split_layout = [(split_pane, central_rect)];
        let pos = point(50.0, 50.0);

        let observation =
            resolve_pane_under_pointer(pos, central_rect, Some(zoomed_pane), &split_layout, |id| {
                assert_eq!(
                    id, zoomed_pane,
                    "zoomed branch must resolve the zoomed pane, not a pane from split_layout"
                );
                Some(quiet_inputs())
            });

        assert_eq!(observation.pane, Some(zoomed_pane));
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

        let pos = point(75.0, 50.0);

        let observation =
            resolve_pane_under_pointer(pos, central_rect, None, &split_layout, |id| {
                assert_eq!(id, pane_b, "must resolve the pane whose rect contains pos");
                Some(quiet_inputs())
            });

        assert_eq!(observation.pane, Some(pane_b));
    }

    #[test]
    fn resolve_pane_under_pointer_zoomed_pointer_outside_central_rect_resolves_to_no_pane() {
        let mut id_gen = PaneIdGenerator::new(0);
        let zoomed_pane = id_gen.next_id();
        let central_rect = Rect::from_min_max(point(0.0, 0.0), point(100.0, 100.0));
        let pos = point(500.0, 500.0);

        let observation =
            resolve_pane_under_pointer(pos, central_rect, Some(zoomed_pane), &[], |_| {
                unreachable!("lookup must not run when no pane resolves")
            });

        assert_eq!(observation.pane, None);
        assert_eq!(observation.region, PointerRegion::Outside);
    }

    #[test]
    fn resolve_pane_under_pointer_outside_every_pane_is_legitimately_no_pane() {
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let rect_a = Rect::from_min_max(point(0.0, 0.0), point(50.0, 100.0));
        let split_layout = [(pane_a, rect_a)];
        let central_rect = Rect::from_min_max(point(0.0, 0.0), point(100.0, 100.0));

        let pos = point(75.0, 50.0);

        let observation =
            resolve_pane_under_pointer(pos, central_rect, None, &split_layout, |_| {
                unreachable!("lookup must not run when no pane resolves")
            });

        assert_eq!(observation.pane, None);
        assert_eq!(
            observation.region,
            PointerRegion::Outside,
            "not over any pane at all is a legitimate Outside, not Unknown"
        );
    }

    #[test]
    fn resolve_pane_under_pointer_lookup_miss_resolves_to_unresolved_unknown_not_quiet_outside() {
        // Review fix: a hit-tested pane whose id came from THIS SAME call's
        // own layout, but whose data cannot be looked up, is a bug state
        // (the tree changed out from under the layout) -- Unknown, not the
        // legitimately-quiet `no_pane`/Outside case.
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let rect_a = Rect::from_min_max(point(0.0, 0.0), point(100.0, 100.0));
        let split_layout = [(pane_a, rect_a)];
        let central_rect = rect_a;
        let pos = point(50.0, 50.0);

        let observation =
            resolve_pane_under_pointer(pos, central_rect, None, &split_layout, |_| None);

        assert_eq!(observation.pane, None);
        assert_eq!(observation.region, PointerRegion::Unknown);
    }

    #[test]
    fn resolve_pane_under_pointer_classifies_using_the_resolved_panes_geometry() {
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let rect_a = Rect::from_min_max(point(0.0, 0.0), point(120.0, 100.0));
        let split_layout = [(pane_a, rect_a)];
        let central_rect = rect_a;
        let pos = point(30.0, 10.0); // inside terminal_rect (min.x=20) -> content

        let observation =
            resolve_pane_under_pointer(pos, central_rect, None, &split_layout, |_| {
                Some(quiet_inputs())
            });

        assert_eq!(
            observation.region,
            PointerRegion::Content { col: 1, row: 0 }
        );
    }

    #[test]
    fn resolve_pane_under_pointer_gutter_eligible_passes_through_pane_snapshot_inputs_verbatim() {
        // Review fix: gutter eligibility is no longer computed inside
        // `resolve_pane_under_pointer` from a `gutter_config_active: bool`
        // parameter (removed) plus pane-state fields -- it is now a single
        // named `PaneSnapshotInputs::gutter_eligible` field the caller
        // (app boundary) computes, and this function passes it through
        // unchanged.
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let rect_a = Rect::from_min_max(point(0.0, 0.0), point(120.0, 100.0));
        let split_layout = [(pane_a, rect_a)];
        let central_rect = rect_a;
        let pos = point(30.0, 10.0);

        let eligible = resolve_pane_under_pointer(pos, central_rect, None, &split_layout, |_| {
            Some(PaneSnapshotInputs {
                gutter_eligible: true,
                ..quiet_inputs()
            })
        });
        assert!(eligible.gutter_eligible);

        let ineligible = resolve_pane_under_pointer(pos, central_rect, None, &split_layout, |_| {
            Some(quiet_inputs())
        });
        assert!(!ineligible.gutter_eligible);
    }

    #[test]
    fn resolve_pane_under_pointer_scrollbar_pane_set_only_when_inside_that_panes_hit_rect() {
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let rect_a = Rect::from_min_max(point(0.0, 0.0), point(120.0, 100.0));
        let split_layout = [(pane_a, rect_a)];
        let central_rect = rect_a;
        let hit_rect = Rect::from_min_max(point(90.0, 0.0), point(120.0, 100.0));
        let inputs_with_scrollbar = PaneSnapshotInputs {
            geometry: Some(PanePositionGeometry {
                scrollbar_hit_rect: Some(hit_rect),
                ..geometry()
            }),
            ..quiet_inputs()
        };

        let inside = resolve_pane_under_pointer(
            point(100.0, 50.0),
            central_rect,
            None,
            &split_layout,
            |_| Some(inputs_with_scrollbar),
        );
        assert_eq!(inside.scrollbar_pane, Some(pane_a));

        let outside = resolve_pane_under_pointer(
            point(30.0, 50.0),
            central_rect,
            None,
            &split_layout,
            |_| Some(inputs_with_scrollbar),
        );
        assert_eq!(outside.scrollbar_pane, None);
    }

    // ── `url_positional_force` ────────────────────────────────────────────

    fn content_observation(
        pane: crate::gui::panes::PaneId,
        col: usize,
        row: usize,
    ) -> PointerObservation {
        PointerObservation {
            pane: Some(pane),
            region: PointerRegion::Content { col, row },
            has_urls: false,
            gutter_eligible: false,
            scrollbar_pane: None,
        }
    }

    #[test]
    fn url_positional_force_no_urls_anywhere_never_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = content_observation(pane, 0, 0);
        let curr = content_observation(pane, 5, 5);
        assert!(!url_positional_force(prev, curr));
    }

    #[test]
    fn url_positional_force_same_pane_same_cell_suppresses() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = PointerObservation {
            has_urls: true,
            ..content_observation(pane, 3, 4)
        };
        let curr = PointerObservation {
            has_urls: true,
            ..content_observation(pane, 3, 4)
        };
        assert!(!url_positional_force(prev, curr));
    }

    #[test]
    fn url_positional_force_same_pane_different_cell_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = PointerObservation {
            has_urls: true,
            ..content_observation(pane, 3, 4)
        };
        let curr = PointerObservation {
            has_urls: true,
            ..content_observation(pane, 3, 5)
        };
        assert!(url_positional_force(prev, curr));
    }

    #[test]
    fn url_positional_force_entering_content_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = PointerObservation {
            has_urls: true,
            region: PointerRegion::Outside,
            ..content_observation(pane, 3, 4)
        };
        let curr = PointerObservation {
            has_urls: true,
            ..content_observation(pane, 3, 4)
        };
        assert!(url_positional_force(prev, curr));
    }

    #[test]
    fn url_positional_force_crossing_away_from_url_bearing_pane_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane_a = ids.next_id();
        let pane_b = ids.next_id();
        let prev = PointerObservation {
            has_urls: true,
            ..content_observation(pane_a, 3, 4)
        };
        let curr = content_observation(pane_b, 3, 4);
        assert!(url_positional_force(prev, curr));
    }

    #[test]
    fn url_positional_force_either_endpoint_having_urls_is_sufficient() {
        let mut ids = PaneIdGenerator::new(0);
        let pane_a = ids.next_id();
        let pane_b = ids.next_id();
        // prev has no urls but curr's pane does; different panes -> forces.
        let prev = content_observation(pane_a, 0, 0);
        let curr = PointerObservation {
            has_urls: true,
            ..content_observation(pane_b, 0, 0)
        };
        assert!(url_positional_force(prev, curr));
    }

    // ── `gutter_positional_force` ─────────────────────────────────────────

    /// A [`PointerObservation`] classified [`PointerRegion::Gutter`] at
    /// `row`, for a pane where the gutter IS eligible this frame.
    ///
    /// Two explicit constructors (this one and
    /// [`gutter_observation_ineligible`]) rather than one taking an
    /// `eligible: bool` parameter — `freminal-state-representation`
    /// forbids bool parameters, including in test helpers.
    fn gutter_observation_eligible(
        pane: crate::gui::panes::PaneId,
        row: usize,
    ) -> PointerObservation {
        PointerObservation {
            pane: Some(pane),
            region: PointerRegion::Gutter { row },
            has_urls: false,
            gutter_eligible: true,
            scrollbar_pane: None,
        }
    }

    /// The [`gutter_observation_eligible`] counterpart for a pane where the
    /// gutter is NOT eligible this frame.
    fn gutter_observation_ineligible(
        pane: crate::gui::panes::PaneId,
        row: usize,
    ) -> PointerObservation {
        PointerObservation {
            gutter_eligible: false,
            ..gutter_observation_eligible(pane, row)
        }
    }

    #[test]
    fn gutter_positional_force_content_to_content_never_relevant() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = content_observation(pane, 0, 0);
        let curr = content_observation(pane, 1, 1);
        assert!(!gutter_positional_force(prev, curr));
    }

    #[test]
    fn gutter_positional_force_same_pane_same_row_suppresses() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = gutter_observation_eligible(pane, 2);
        let curr = gutter_observation_eligible(pane, 2);
        assert!(!gutter_positional_force(prev, curr));
    }

    #[test]
    fn gutter_positional_force_row_change_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = gutter_observation_eligible(pane, 2);
        let curr = gutter_observation_eligible(pane, 3);
        assert!(gutter_positional_force(prev, curr));
    }

    #[test]
    fn gutter_positional_force_leaving_the_gutter_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = gutter_observation_eligible(pane, 2);
        let curr = content_observation(pane, 0, 2);
        assert!(gutter_positional_force(prev, curr));
    }

    #[test]
    fn gutter_positional_force_ineligible_gutter_region_is_not_relevant() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = gutter_observation_ineligible(pane, 2);
        let curr = gutter_observation_ineligible(pane, 3);
        assert!(!gutter_positional_force(prev, curr));
    }

    #[test]
    fn gutter_positional_force_pane_change_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane_a = ids.next_id();
        let pane_b = ids.next_id();
        let prev = gutter_observation_eligible(pane_a, 2);
        let curr = gutter_observation_eligible(pane_b, 2);
        assert!(gutter_positional_force(prev, curr));
    }

    // ── `scrollbar_boundary_force` ────────────────────────────────────────

    #[test]
    fn scrollbar_boundary_force_both_inside_the_same_pane_does_not_force() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = PointerObservation {
            scrollbar_pane: Some(pane),
            ..content_observation(pane, 0, 0)
        };
        let curr = PointerObservation {
            scrollbar_pane: Some(pane),
            ..content_observation(pane, 0, 1)
        };
        assert!(!scrollbar_boundary_force(prev, curr));
    }

    #[test]
    fn scrollbar_boundary_force_both_outside_any_scrollbar_does_not_force() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = content_observation(pane, 0, 0);
        let curr = content_observation(pane, 0, 1);
        assert!(!scrollbar_boundary_force(prev, curr));
    }

    #[test]
    fn scrollbar_boundary_force_crossing_in_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = content_observation(pane, 0, 0);
        let curr = PointerObservation {
            scrollbar_pane: Some(pane),
            ..content_observation(pane, 0, 1)
        };
        assert!(scrollbar_boundary_force(prev, curr));
    }

    #[test]
    fn scrollbar_boundary_force_crossing_out_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = PointerObservation {
            scrollbar_pane: Some(pane),
            ..content_observation(pane, 0, 0)
        };
        let curr = content_observation(pane, 0, 1);
        assert!(scrollbar_boundary_force(prev, curr));
    }

    #[test]
    fn scrollbar_boundary_force_crossing_directly_from_one_panes_scrollbar_to_anothers_forces() {
        // Review fix: this is the case a bare `bool` comparison gets wrong
        // -- both endpoints are "inside a scrollbar hit rect" (so a
        // bare-bool `!=` would see no change and suppress), but they are
        // DIFFERENT panes' scrollbars, which must force exactly like any
        // other cross-pane transition.
        let mut ids = PaneIdGenerator::new(0);
        let pane_a = ids.next_id();
        let pane_b = ids.next_id();
        let prev = PointerObservation {
            scrollbar_pane: Some(pane_a),
            ..content_observation(pane_a, 0, 0)
        };
        let curr = PointerObservation {
            scrollbar_pane: Some(pane_b),
            ..content_observation(pane_b, 0, 0)
        };
        assert!(scrollbar_boundary_force(prev, curr));
    }

    // ── `selection_positional_force` ──────────────────────────────────────

    #[test]
    fn selection_positional_force_none_selecting_never_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = content_observation(pane, 0, 0);
        let curr = content_observation(pane, 5, 5);
        assert!(!selection_positional_force(
            prev,
            curr,
            SelectingPanes::None
        ));
    }

    #[test]
    fn selection_positional_force_multiple_selecting_always_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = content_observation(pane, 0, 0);
        let curr = content_observation(pane, 0, 0);
        assert!(selection_positional_force(
            prev,
            curr,
            SelectingPanes::Multiple
        ));
    }

    #[test]
    fn selection_positional_force_one_selecting_same_cell_suppresses() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = content_observation(pane, 3, 3);
        let curr = content_observation(pane, 3, 3);
        assert!(!selection_positional_force(
            prev,
            curr,
            SelectingPanes::One(pane)
        ));
    }

    #[test]
    fn selection_positional_force_one_selecting_different_cell_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = content_observation(pane, 3, 3);
        let curr = content_observation(pane, 4, 3);
        assert!(selection_positional_force(
            prev,
            curr,
            SelectingPanes::One(pane)
        ));
    }

    #[test]
    fn selection_positional_force_one_selecting_outside_endpoint_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = content_observation(pane, 3, 3);
        let curr = PointerObservation {
            region: PointerRegion::Outside,
            ..content_observation(pane, 3, 3)
        };
        assert!(selection_positional_force(
            prev,
            curr,
            SelectingPanes::One(pane)
        ));
    }

    #[test]
    fn selection_positional_force_one_selecting_different_pane_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let selecting_pane = ids.next_id();
        let other_pane = ids.next_id();
        let prev = content_observation(other_pane, 0, 0);
        let curr = content_observation(other_pane, 0, 0);
        assert!(selection_positional_force(
            prev,
            curr,
            SelectingPanes::One(selecting_pane)
        ));
    }

    // ── `unknown_geometry_force` ──────────────────────────────────────────

    #[test]
    fn unknown_geometry_force_neither_unknown_is_false() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = content_observation(pane, 0, 0);
        let curr = content_observation(pane, 1, 1);
        assert!(!unknown_geometry_force(prev, curr));
    }

    #[test]
    fn unknown_geometry_force_previous_unknown_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = PointerObservation {
            region: PointerRegion::Unknown,
            ..content_observation(pane, 0, 0)
        };
        let curr = content_observation(pane, 1, 1);
        assert!(unknown_geometry_force(prev, curr));
    }

    #[test]
    fn unknown_geometry_force_current_unknown_forces() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        let prev = content_observation(pane, 0, 0);
        let curr = PointerObservation {
            region: PointerRegion::Unknown,
            ..content_observation(pane, 1, 1)
        };
        assert!(unknown_geometry_force(prev, curr));
    }

    // ── `pointer_motion_needs_repaint_decision` ───────────────────────────

    #[test]
    fn pointer_motion_needs_repaint_decision_all_clear_is_false() {
        assert!(!pointer_motion_needs_repaint_decision(
            PointerMotionInputs::default()
        ));
    }

    #[test]
    fn pointer_motion_inputs_default_is_all_clear() {
        let d = PointerMotionInputs::default();
        assert!(!d.first_motion);
        assert!(!d.focus_change_pending);
        assert!(!d.chrome_interactive);
        assert!(!d.overlay_open);
        assert!(!d.pointer_pane_unresolved);
        assert!(!d.unknown_geometry);
        assert!(!d.url_forced);
        assert!(!d.gutter_forced);
        assert!(!d.scrollbar_boundary_forced);
        assert!(!d.scrollbar_drag_forced);
        assert!(!d.selection_forced);
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_first_motion_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            first_motion: true,
            ..PointerMotionInputs::default()
        }));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_pending_focus_change_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            focus_change_pending: true,
            ..PointerMotionInputs::default()
        }));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_chrome_interactive_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            chrome_interactive: true,
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
    fn pointer_motion_needs_repaint_decision_unknown_geometry_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            unknown_geometry: true,
            ..PointerMotionInputs::default()
        }));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_url_forced_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            url_forced: true,
            ..PointerMotionInputs::default()
        }));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_gutter_forced_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            gutter_forced: true,
            ..PointerMotionInputs::default()
        }));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_scrollbar_boundary_forced_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            scrollbar_boundary_forced: true,
            ..PointerMotionInputs::default()
        }));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_scrollbar_drag_forced_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            scrollbar_drag_forced: true,
            ..PointerMotionInputs::default()
        }));
    }

    #[test]
    fn pointer_motion_needs_repaint_decision_selection_forced_forces_true() {
        assert!(pointer_motion_needs_repaint_decision(PointerMotionInputs {
            selection_forced: true,
            ..PointerMotionInputs::default()
        }));
    }

    // ── Quiet pane-crossing must not force merely because pane ids differ ──

    #[test]
    fn quiet_crossing_between_clean_panes_does_not_force() {
        let mut ids = PaneIdGenerator::new(0);
        let pane_a = ids.next_id();
        let pane_b = ids.next_id();
        let prev = content_observation(pane_a, 5, 5);
        let curr = content_observation(pane_b, 2, 2);

        assert!(!url_positional_force(prev, curr));
        assert!(!gutter_positional_force(prev, curr));
        assert!(!scrollbar_boundary_force(prev, curr));
        assert!(!selection_positional_force(
            prev,
            curr,
            SelectingPanes::None
        ));
        assert!(!unknown_geometry_force(prev, curr));

        assert!(!pointer_motion_needs_repaint_decision(
            PointerMotionInputs {
                url_forced: url_positional_force(prev, curr),
                gutter_forced: gutter_positional_force(prev, curr),
                scrollbar_boundary_forced: scrollbar_boundary_force(prev, curr),
                selection_forced: selection_positional_force(prev, curr, SelectingPanes::None),
                unknown_geometry: unknown_geometry_force(prev, curr),
                ..PointerMotionInputs::default()
            }
        ));
    }

    // ── Composed invariant with `pty_mouse_report.rs` (Task 124.3b) ───────
    //
    // `pty_mouse_report.rs`'s
    // `sgr_pixels_two_sub_cell_moves_produce_two_distinct_reports_and_no_third_repeat`
    // proves the DELIVERY half: two motions within one 8x16 cell
    // ((10.0, 10.0) then (11.0, 10.0), both floor to `Content { col: 1, row: 0 }`)
    // each produce a distinct `?1016` PTY report, because pixel-granular
    // report encoding (124.3a) is independent of this module's
    // cell-granular repaint gate. This test proves the other half of the
    // SAME composed invariant: those same two positions, resolved through
    // THIS module's classifier against THIS module's decision chain,
    // suppress the repaint that would otherwise redraw the second motion —
    // "two PTY reports, one repaint" is the intended shape, not a
    // coincidence of two independently-passing tests.

    #[test]
    fn two_within_cell_sgr_pixels_motions_report_twice_but_repaint_once() {
        let mut ids = PaneIdGenerator::new(0);
        let pane = ids.next_id();
        // Mirrors `pty_mouse_report.rs::clean_report_inputs`'s geometry
        // exactly: terminal_rect at the window origin, 8x16 cells.
        let geom = PanePositionGeometry {
            terminal_rect: Rect::from_min_max(point(0.0, 0.0), point(100.0, 100.0)),
            cell_width: 8.0,
            cell_height: 16.0,
            scrollbar_hit_rect: None,
        };
        let pane_rect = geom.terminal_rect;

        // Same two positions `pty_mouse_report.rs`'s test feeds
        // `maybe_send_immediate_motion_report`: same cell (1, 0), distinct
        // sub-cell pixels.
        let first_pos = point(10.0, 10.0);
        let second_pos = point(11.0, 10.0);
        let first_region = classify_pointer_position(first_pos, pane_rect, Some(geom));
        let second_region = classify_pointer_position(second_pos, pane_rect, Some(geom));
        assert_eq!(
            first_region,
            PointerRegion::Content { col: 1, row: 0 },
            "fixture sanity: both positions must floor to the same content cell"
        );
        assert_eq!(first_region, second_region);

        let prev = PointerObservation {
            pane: Some(pane),
            region: first_region,
            has_urls: false,
            gutter_eligible: false,
            scrollbar_pane: None,
        };
        let curr = PointerObservation {
            pane: Some(pane),
            region: second_region,
            has_urls: false,
            gutter_eligible: false,
            scrollbar_pane: None,
        };

        // The repaint gate: same pane, same cell, no other forcing
        // condition active -> suppressed. `pty_mouse_report.rs`'s sibling
        // test independently proves the PTY write path still emits two
        // distinct byte sequences for these same two positions — this
        // repaint suppression does not, and must not, affect that.
        assert!(!pointer_motion_needs_repaint_decision(
            PointerMotionInputs {
                url_forced: url_positional_force(prev, curr),
                gutter_forced: gutter_positional_force(prev, curr),
                scrollbar_boundary_forced: scrollbar_boundary_force(prev, curr),
                selection_forced: selection_positional_force(prev, curr, SelectingPanes::None),
                unknown_geometry: unknown_geometry_force(prev, curr),
                ..PointerMotionInputs::default()
            }
        ));
    }
}

/// Subtask 124.13, updated by Task 124.3b: the pointer-motion gate's
/// suppression rate, per scenario — now driven by sequential
/// previous/current position pairs through the positional chain
/// (`resolve_pane_under_pointer` -> the `*_positional_force` functions ->
/// `pointer_motion_needs_repaint_decision`) instead of the pre-124.3b
/// pane-wide vetoes the original 124.13 table measured.
///
/// **Measurement, not behaviour.** Nothing here changes a decision; it
/// drives the real positional chain over a deterministic sweep of
/// sequential pointer-position pairs and counts what comes out.
///
/// # What changed from the original 124.13 table
///
/// The original table's two headline findings — "one hyperlink anywhere
/// collapses suppression to near zero" and "a nonzero scroll offset is an
/// identical pane-wide veto" — described the OLD `has_urls`/
/// `scroll_offset > 0` pane-wide vetoes this subtask removed. Both
/// scenarios below now demonstrate the replacement: a hyperlink-bearing (or
/// scrolled-back) pane suppresses steady motion within one cell/scrollbar
/// state exactly as well as a clean pane, and only forces on an actual cell
/// crossing (URL) or scrollbar hit-rect boundary crossing (scrollbar) — the
/// positional cost the pane-wide veto's replacement was designed to pay
/// instead of the whole pane.
#[cfg(test)]
mod suppression_rates {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use conv2::ConvUtil;
    use freminal_common::geometry::{Rect, point};

    use super::{
        PanePositionGeometry, PaneSnapshotInputs, PointerMotionInputs, PointerObservation,
        PointerRegion, SelectingPanes, endpoints_share_content_cell, gutter_positional_force,
        pointer_motion_needs_repaint_decision, resolve_pane_under_pointer,
        scrollbar_boundary_force, selection_positional_force, unknown_geometry_force,
        url_positional_force,
    };
    use crate::gui::panes::PaneIdGenerator;

    /// A window whose central (terminal) area is 1200x700 at (40, 60) —
    /// leaving a chrome band above it, so the sweep below covers both
    /// over-pane and over-chrome positions the way a real session does.
    const CENTRAL: Rect = Rect {
        min: point(40.0, 60.0),
        max: point(1240.0, 760.0),
    };

    /// The gutter's total inset in logical points — matches
    /// `GutterPosition::total_inset_px() / ppp`'s real-world order of
    /// magnitude, used here to build a `terminal_rect` with a realistic
    /// left-edge offset from `CENTRAL`.
    const GUTTER_INSET: f32 = 18.0;

    /// One scenario's outcome.
    ///
    /// `same_content_cell_pairs`/`same_content_cell_suppressed` (review fix,
    /// item 5) isolate the specific sub-population the hyperlink scenario's
    /// headline claim is actually about: checked pairs that resolved to the
    /// SAME pane at the SAME [`PointerRegion::Content`] cell (no cell
    /// crossing at all). `checks`/`suppressed` alone cannot distinguish "a
    /// same-cell pair suppressed" from "an unrelated Outside/Outside pair
    /// suppressed" — both count identically toward the aggregate — so a
    /// claim about same-cell behaviour needs its own counters, not a
    /// generic rate over a denominator that also contains chrome/Outside
    /// pairs that prove nothing about cell-crossing.
    struct Outcome {
        checks: u32,
        suppressed: u32,
        same_content_cell_pairs: u32,
        same_content_cell_suppressed: u32,
    }

    impl Outcome {
        fn rate(&self) -> f64 {
            f64::from(self.suppressed) / f64::from(self.checks)
        }
    }

    /// Sweep a deterministic lattice of SEQUENTIAL pointer-position pairs
    /// (each lattice point as `curr`, the immediately preceding lattice
    /// point as `prev`) over the whole window and count how many motion
    /// checks the gate suppresses. Every pane resolved against the SAME
    /// `layout`/`inputs` (a single steady pane, matching how a real
    /// wandering-pointer sweep observes one pane's state).
    ///
    /// # Review fix (item 5): explicit sub-cell companion moves
    ///
    /// The coarse 20-point lattice stride is larger than the 8x16 fixture
    /// cell, so on its own the lattice crosses a content cell on almost
    /// every step and rarely exercises the "moved, but stayed in the same
    /// cell" case a hyperlink-hover repaint gate exists to suppress. Whenever
    /// a lattice point resolves inside the terminal content grid, this
    /// function immediately follows it with one more checked pair against
    /// the DEAD CENTER of that same `Content` cell — guaranteed same-cell by
    /// construction (the center of a cell can never floor to a different
    /// cell than the cell it is the center of), not an arbitrary nearby
    /// offset that could straddle a boundary depending on the lattice
    /// stride's phase. This makes the whole-window sweep itself, not only a
    /// separate one-off assertion, genuinely exercise inside-pane
    /// same-content-cell pairs for every scenario that calls `sweep`.
    fn sweep(inputs: PaneSnapshotInputs, selecting: SelectingPanes) -> Outcome {
        let mut ids = PaneIdGenerator::default();
        let pane_id = ids.next_id();
        let layout = [(pane_id, CENTRAL)];

        let mut checks = 0_u32;
        let mut suppressed = 0_u32;
        let mut same_content_cell_pairs = 0_u32;
        let mut same_content_cell_suppressed = 0_u32;
        let mut previous: Option<PointerObservation> = None;

        let mut check_pair = |prev: PointerObservation, current: PointerObservation| {
            let needs_repaint = pointer_motion_needs_repaint_decision(PointerMotionInputs {
                url_forced: url_positional_force(prev, current),
                gutter_forced: gutter_positional_force(prev, current),
                scrollbar_boundary_forced: scrollbar_boundary_force(prev, current),
                selection_forced: selection_positional_force(prev, current, selecting),
                unknown_geometry: unknown_geometry_force(prev, current),
                ..PointerMotionInputs::default()
            });

            checks += 1;
            if !needs_repaint {
                suppressed += 1;
            }
            if endpoints_share_content_cell(prev, current) {
                same_content_cell_pairs += 1;
                if !needs_repaint {
                    same_content_cell_suppressed += 1;
                }
            }
        };

        // 64 x 40 lattice over a region wider and taller than CENTRAL.
        for iy in 0..40 {
            for ix in 0..64 {
                let x = f64::from(ix) * 20.0;
                let y = f64::from(iy) * 20.0;
                #[allow(clippy::cast_possible_truncation)]
                let pos = point(x as f32, y as f32);

                let current =
                    resolve_pane_under_pointer(pos, CENTRAL, None, &layout, |_| Some(inputs));

                let Some(prev) = previous else {
                    // First motion: always forced, and not counted as a
                    // "check" the positional chain could have suppressed —
                    // mirrors `PointerMotionInputs::first_motion` being a
                    // separate top-level term, not a positional one.
                    previous = Some(current);
                    continue;
                };

                check_pair(prev, current);

                // Review fix (item 5): a guaranteed-same-cell companion
                // move, dead center of the cell `current` just resolved to
                // (skipped for Gutter/Outside/Unknown, which have no
                // "center" this fixture geometry can compute) — see this
                // function's doc.
                let sub_current = match (current.region, inputs.geometry) {
                    (PointerRegion::Content { col, row }, Some(geometry)) => {
                        let col_f = col.approx_as::<f32>().unwrap_or(0.0);
                        let row_f = row.approx_as::<f32>().unwrap_or(0.0);
                        let sub_pos = point(
                            (col_f + 0.5)
                                .mul_add(geometry.cell_width, geometry.terminal_rect.min.x),
                            (row_f + 0.5)
                                .mul_add(geometry.cell_height, geometry.terminal_rect.min.y),
                        );
                        let sub_current =
                            resolve_pane_under_pointer(sub_pos, CENTRAL, None, &layout, |_| {
                                Some(inputs)
                            });
                        check_pair(current, sub_current);
                        sub_current
                    }
                    _ => current,
                };
                previous = Some(sub_current);
            }
        }

        Outcome {
            checks,
            suppressed,
            same_content_cell_pairs,
            same_content_cell_suppressed,
        }
    }

    const CLEAN: PaneSnapshotInputs = PaneSnapshotInputs {
        has_urls: false,
        gutter_eligible: false,
        geometry: None,
    };

    fn clean_with_geometry() -> PaneSnapshotInputs {
        PaneSnapshotInputs {
            geometry: Some(PanePositionGeometry {
                terminal_rect: Rect::from_min_max(
                    point(CENTRAL.min.x + GUTTER_INSET, CENTRAL.min.y),
                    CENTRAL.max,
                ),
                cell_width: 8.0,
                cell_height: 16.0,
                scrollbar_hit_rect: None,
            }),
            ..CLEAN
        }
    }

    /// Baseline: nothing hover-sensitive on screen, real geometry published.
    ///
    /// # Review fix (item 5): truthful expectation, not "a high fraction"
    ///
    /// A clean pane has no positional visual source at all: `has_urls`,
    /// `gutter_eligible`, and `scrollbar_hit_rect` are all off/`None`, and
    /// `selecting` is `SelectingPanes::None`, so every one of
    /// `url_positional_force`/`gutter_positional_force`/
    /// `scrollbar_boundary_force`/`selection_positional_force` is
    /// unconditionally `false` regardless of where the pointer moved —
    /// their preconditions, not cell crossing, gate them. Whether the
    /// lattice's 20-point stride happens to cross the fixture's 8x16 cell
    /// boundary on a given step is therefore irrelevant to this scenario:
    /// EVERY checked pair suppresses, by construction, not merely "a high
    /// fraction" of them. The previous version of this test asserted only
    /// `suppressed > 0` and its doc claimed the lattice stride made 100%
    /// unlikely — both were wrong; a clean pane suppressing anything less
    /// than everything would itself be the bug.
    #[test]
    fn a_clean_pane_with_geometry_suppresses_everything() {
        let out = sweep(clean_with_geometry(), SelectingPanes::None);
        assert_eq!(
            out.suppressed, out.checks,
            "a pane with no positional visual source (no urls, no gutter, no scrollbar, not \
             selecting) must suppress EVERY checked pair regardless of cell crossing -- got {} \
             suppressed of {} checks",
            out.suppressed, out.checks
        );
    }

    /// **The replacement for 124.13's headline finding.** The removed
    /// pane-wide `has_urls` veto forced EVERY motion check for a
    /// hyperlink-bearing pane, regardless of position — a suppression rate
    /// of exactly zero. `url_positional_force` instead only forces on an
    /// actual cell crossing, so same-cell steps in the sweep now suppress.
    ///
    /// # Review fix (item 5): exact same-cell counters, not a loose rate
    ///
    /// `urls.suppressed > 0` alone can ALSO be satisfied by lattice pairs
    /// that land entirely OUTSIDE the pane (both endpoints `no_pane`/
    /// `Outside` suppress too, but prove nothing about cell-granular URL
    /// suppression specifically) — a loose-tolerance assertion that would
    /// pass even if the sweep never exercised a genuine inside-pane
    /// same-content-cell pair at all. `sweep`'s sub-cell companion moves
    /// (see its doc) guarantee the whole-window sweep itself now contains
    /// such pairs — `same_content_cell_pairs` — for every scenario, so the
    /// exact counters below assert the real claim directly: EVERY
    /// same-content-cell pair suppressed (no cell crossing occurred, so
    /// nothing could have forced), and at least one such pair was actually
    /// exercised.
    #[test]
    fn a_hyperlink_bearing_pane_suppresses_every_same_content_cell_pair() {
        let inputs = PaneSnapshotInputs {
            has_urls: true,
            ..clean_with_geometry()
        };
        let urls = sweep(inputs, SelectingPanes::None);

        assert!(
            urls.same_content_cell_pairs > 0,
            "the whole-window sweep must exercise at least one inside-pane same-content-cell \
             pair for the hyperlink scenario (via `sweep`'s sub-cell companion moves) -- got 0 \
             out of {} checks",
            urls.checks
        );
        assert_eq!(
            urls.same_content_cell_suppressed, urls.same_content_cell_pairs,
            "every inside-pane same-content-cell pair must suppress a hyperlink-bearing pane's \
             URL-hover repaint (no cell crossing occurred, so nothing could have forced) -- got \
             {} suppressed of {} such pairs",
            urls.same_content_cell_suppressed, urls.same_content_cell_pairs
        );

        // In addition to `sweep`'s own same-cell counters above, pin the
        // underlying mechanism directly: two sequential positions strictly
        // INSIDE the pane's terminal content, well under one 8x16 cell
        // apart, that resolve to the SAME pane at the SAME content cell.
        let mut ids = PaneIdGenerator::default();
        let pane_id = ids.next_id();
        let layout = [(pane_id, CENTRAL)];
        let inside_a = point(CENTRAL.min.x + GUTTER_INSET + 10.0, CENTRAL.min.y + 10.0);
        // +3 logical points on each axis -- well inside the 8x16 cell
        // `inside_a` floors to, so both positions must classify to the
        // SAME `Content` cell.
        let inside_b = point(inside_a.x + 3.0, inside_a.y + 3.0);

        let prev = resolve_pane_under_pointer(inside_a, CENTRAL, None, &layout, |_| Some(inputs));
        let curr = resolve_pane_under_pointer(inside_b, CENTRAL, None, &layout, |_| Some(inputs));

        assert_eq!(
            prev.pane,
            Some(pane_id),
            "fixture sanity: must resolve inside the pane"
        );
        assert_eq!(
            curr.pane,
            Some(pane_id),
            "fixture sanity: must resolve inside the pane"
        );
        assert!(
            matches!(prev.region, PointerRegion::Content { .. }),
            "fixture sanity: must land in the terminal content grid, not the gutter -- got {:?}",
            prev.region
        );
        assert_eq!(
            prev.region, curr.region,
            "fixture sanity: both sub-cell positions must floor to the SAME content cell"
        );
        assert!(
            !url_positional_force(prev, curr),
            "two sub-cell moves within the SAME pane's SAME content cell must suppress \
             URL-hover repaint even though the pane has URLs -- got prev={prev:?} curr={curr:?}"
        );
    }

    /// An active selection drag with no matching cell/pane suppresses
    /// nothing at all — `selection_positional_force`'s conservative
    /// "different pane" branch, which every lattice position here hits
    /// (the selecting pane never resolves under the sweep's own pane).
    #[test]
    fn a_foreign_selecting_pane_suppresses_nothing() {
        let mut ids = PaneIdGenerator::new(100);
        let foreign_pane = ids.next_id();
        let out = sweep(clean_with_geometry(), SelectingPanes::One(foreign_pane));
        assert_eq!(
            out.suppressed, 0,
            "a selection drag on a pane that never resolves under the sweep must force every check"
        );
    }

    /// The gutter strip remains cheap: enabling it (with command blocks
    /// present) only costs the strip itself, not the whole pane, exactly as
    /// it did before 124.3b (this term was already positional — 124.3b
    /// generalizes it to the previous/current pair rather than changing its
    /// shape).
    #[test]
    fn the_gutter_veto_remains_positional_and_therefore_cheap() {
        let clean = sweep(clean_with_geometry(), SelectingPanes::None);
        let inputs = PaneSnapshotInputs {
            gutter_eligible: true,
            ..clean_with_geometry()
        };
        let out = sweep(inputs, SelectingPanes::None);

        assert!(
            out.rate() > clean.rate() * 0.5,
            "the gutter strip is a narrow fraction of the pane, not a pane-wide veto: \
             clean {:.2}% vs gutter-enabled {:.2}%",
            clean.rate() * 100.0,
            out.rate() * 100.0
        );
    }
}
