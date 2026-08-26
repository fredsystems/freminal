// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Per-pane, per-frame dirty-tracking decision extracted from `show` by Task
//! 122, subtask 122.11.
//!
//! [`evaluate_frame_dirty_state`] decides, for one pane in one frame, which
//! vertex-rebuild path [`FreminalTerminalWidget::show`](super::widget::FreminalTerminalWidget::show)
//! should take: the cheap cursor-only patch, or a re-evaluation of the full
//! rebuild triggers. This is a pure move — relocated here by cleanup entry
//! 122.C3 because leaving it in `widget.rs` (where subtask 122.11 correctly
//! extracted it as a block, but left it in place) grew that file to 5,796
//! lines, overtaking `app_impl.rs` as the largest GUI file and working
//! against Task 122's goal. No logic, control flow, field, signature, or
//! doc-comment text changed; the only edits are visibility (`pub(super)`,
//! needed so `widget.rs` can still call into this sibling module) and `use`
//! statements.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use conv2::ConvUtil;
use egui::Rect;
use freminal_terminal_emulator::snapshot::TerminalSnapshot;

use crate::gui::view_state::{CellCoord, ImageAnimationTick, ViewState};

use super::widget::{
    FoldLayout, PaneRenderCache, RenderState, compute_command_block_hover_rows,
    image_pixels_changed,
};

/// Per-`*_changed` dirty-tracking observations computed once per frame by
/// [`evaluate_frame_dirty_state`].
///
/// Each field is an independent, simultaneously-observable signal — more
/// than one commonly fires together in the same frame (e.g. a fold toggle
/// alongside a selection change) — so per `freminal-state-representation`'s
/// exemption for independent simultaneous signals these stay plain `bool`
/// fields rather than becoming a single enum. Only the *derived*
/// vertex-rebuild decision ([`VertexRebuild`]) is an enum, because that
/// value selects between mutually-exclusive code paths.
// Bools are independent, simultaneously-observable dirty-tracking signals
// (the `freminal-state-representation` exemption for independent
// simultaneous signals) — not a state machine, so an enum would add noise
// without improving clarity.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy)]
pub(super) struct FrameDirtyObservations {
    /// Whether the rendered text content, theme, dimensions, or fold layout
    /// changed since the last full rebuild.
    pub(super) content_changed: bool,
    /// Whether the normalised selection changed since the last full rebuild.
    pub(super) selection_changed: bool,
    /// Whether the search match count or current-match index changed since
    /// the last full rebuild.
    pub(super) search_changed: bool,
    /// Whether the command-block gutter hover-tint range changed since the
    /// last full rebuild.
    pub(super) hover_changed: bool,
    /// Whether any visible animated image advanced to a new frame this tick.
    pub(super) image_frame_changed: bool,
    /// Whether any visible image's selected-frame pixel buffer was mutated
    /// in place since the last full rebuild (e.g. a Kitty `a=c` animation
    /// compose) without any accompanying cell or `run_mode` change.
    pub(super) image_pixels_changed: bool,
    /// Whether text-blink visibility (slow or fast phase) changed since the
    /// last full rebuild.
    pub(super) text_blink_changed: bool,
}

/// Which vertex-rebuild path [`FreminalTerminalWidget::show`] should take
/// this frame, as decided by [`evaluate_frame_dirty_state`].
///
/// This is a switch on *what check runs next*, not a full description of
/// what ultimately happens: `CursorOnly` fully determines the frame (patch
/// the cursor quad, nothing else). `ReevaluateFullRebuild` means the cheap
/// fast path does not apply — the caller must independently check
/// [`FrameDirtyObservations`] (and whether the decoration buffer is empty)
/// to decide between a full vertex rebuild and reusing the previous frame's
/// GPU buffers completely unchanged. Preserving this shape (rather than
/// collapsing to a `CursorOnly`/`Full` binary) matters: not every
/// non-cursor-only frame triggers a full rebuild — the common steady-state
/// frame (nothing changed at all) takes neither branch.
///
/// [`Self::Bounded`] (Task 124.14) means **"full vertex rebuild, bounded
/// damage"**, not a bounded rebuild: the caller still runs the exact same
/// full-rebuild code as [`Self::ReevaluateFullRebuild`] (`upload_verts`
/// stays a whole-buffer write — bounding *that* is Task 125, gated on a
/// fixed-stride vertex relayout that does not exist yet), but it may report
/// [`crate::gui::renderer::PaneFrameDamage::Region`] instead of `Full` for
/// the frame's damage, because the caller already knows content changed
/// only within a known set of rows and no other full-repaint-forcing
/// trigger fired. This variant was named `Rows` until Task 124.14b-i added
/// a second source (`selection_changed`); it was renamed the same commit,
/// since `Rows` would otherwise lie about carrying a damage bound that no
/// longer comes only from rows. This is produced when the content change is
/// attributable *purely* to some combination of [`ChangedRows::Rows`] (Task
/// 124.14a), `selection_changed` (Task 124.14b-i), `hover_changed` (Task
/// 124.14b-ii), and `search_changed` (Task 124.14d) — including
/// [`ChangedRows::None`] with only a selection, hover, and/or search change
/// and no row change at all. `ChangedRows::All`, `theme_changed`,
/// `dims_changed`, `folds_changed`, and every other independently-global
/// trigger (`text_blink_changed`, `image_frame_changed`,
/// `image_pixels_changed`) still yield [`Self::ReevaluateFullRebuild`]
/// (124.21's exhaustive audit, narrowed by one entry each at 124.14b-i,
/// 124.14b-ii, and 124.14d; the remaining one is 124.C5's image-pixels
/// boundary and must stay unbounded until that subtask, if ever, extends
/// it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VertexRebuild {
    /// Patch just the cursor quad in the existing decoration buffer
    /// ([`build_cursor_verts_only`]) — no re-shaping, no full rebuild.
    CursorOnly,
    /// Not the cursor-only fast path; content changed, but the change is
    /// provably bounded — to the window rows named by
    /// [`DirtyTrackingOutcome::changed_rows`] (Task 124.14a), to the
    /// current/previous selection's screen-row span (Task 124.14b-i), to
    /// the current/previous command-block hover span (Task 124.14b-ii), or
    /// any combination of the three — and no other full-repaint trigger
    /// fired. The caller runs the exact same full rebuild as
    /// [`Self::ReevaluateFullRebuild`] but may report a bounded
    /// [`crate::gui::renderer::PaneFrameDamage::Region`] for it instead of
    /// `Full`. Carries no payload of its own — deliberately, so this enum
    /// stays `Copy` — the changed-row list lives on the outcome and the
    /// selection/hover spans live on the render cache, precisely so this
    /// decision and any other reader can see them without a second copy.
    Bounded,
    /// Not the cursor-only fast path; the full-rebuild trigger flags must
    /// still be checked by the caller.
    ReevaluateFullRebuild,
}

/// Which window rows differ from the last full vertex rebuild's rendered
/// content, as computed by [`diff_row_epochs`].
///
/// The asymmetry that governs every branch of this type (and of
/// [`diff_row_epochs`]) is deliberate: **over-reporting costs one needless
/// repaint of a row that did not actually change; under-reporting is
/// silent visual corruption** (a stale glyph left on screen with no event
/// left to correct it). Every conservative branch — recorded here as
/// [`Self::All`] — is chosen on that basis rather than by guessing.
///
/// This is a named type per `freminal-state-representation` rather than a
/// bare `Vec<usize>`: `None` and `All` are not degenerate lists, they are
/// distinct answers to "what changed" that a caller must not confuse with
/// "exactly zero/all rows, coincidentally".
///
/// [`Self::any`] is consumed to fold this into `content_changed`.
/// [`Self::Rows`] (or [`Self::None`] alongside a selection, hover, and/or
/// search change, Tasks 124.14b-i/124.14b-ii/124.14d) is also consumed
/// directly (Task 124.14), via [`DirtyTrackingOutcome::changed_rows`]: when
/// the only content trigger this frame is [`Self::Rows`] and/or
/// `selection_changed` and/or `hover_changed` and/or `search_changed`,
/// [`evaluate_frame_dirty_state`] selects [`VertexRebuild::Bounded`], and
/// the caller reads `changed_rows` off the same outcome to build a bounded
/// [`crate::gui::renderer::PaneFrameDamage::Region`] instead of `Full`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ChangedRows {
    /// No row's rendered content differs from the last full rebuild.
    None,
    /// Exactly these window-row indices differ from the last full rebuild.
    /// Ascending order, no duplicates.
    Rows(Vec<usize>),
    /// Treat the whole pane as changed. Reached when there is no
    /// previous-epoch baseline to diff against (the first-ever rebuild, or
    /// `PaneRenderCache::invalidate_content` cleared it), or when the epoch
    /// vector's length no longer matches this frame's window (the visible
    /// row count changed) — in both cases a per-row diff is not meaningful,
    /// so the conservative whole-pane answer is returned instead of
    /// guessing at a partial one.
    All,
}

impl ChangedRows {
    /// Whether any row changed at all — `true` for a non-empty
    /// [`Self::Rows`] (by construction, see [`diff_row_epochs`]) and for
    /// [`Self::All`].
    pub(super) const fn any(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Compare this frame's per-row content epochs against the epochs recorded
/// at the last full vertex rebuild, producing the minimal set of window
/// rows whose rendered content actually changed.
///
/// See [`ChangedRows`] for the asymmetry that governs the conservative
/// branches below: over-reporting is cheap (one needless repaint),
/// under-reporting is a correctness bug (silent visual corruption), so
/// every branch that cannot prove "these specific rows" falls back to
/// [`ChangedRows::All`] rather than guessing.
fn diff_row_epochs(previous: Option<&Arc<[u64]>>, current: &Arc<[u64]>) -> ChangedRows {
    let Some(previous) = previous else {
        // No baseline recorded yet: the first-ever rebuild, or
        // `invalidate_content` cleared it. There is nothing to diff
        // against, so treat everything as changed rather than assuming
        // "nothing changed".
        return ChangedRows::All;
    };
    if previous.len() != current.len() {
        // The window's row count no longer matches the baseline (a resize
        // or a fold-layout change altered the flattened window height).
        // `dims_changed`/`folds_changed` already force a full rebuild in
        // this case in practice, but this keeps the function correct
        // standalone rather than relying on that coincidence.
        return ChangedRows::All;
    }
    let changed: Vec<usize> = previous
        .iter()
        .zip(current.iter())
        .enumerate()
        .filter_map(|(i, (prev, cur))| (prev != cur).then_some(i))
        .collect();
    if changed.is_empty() {
        ChangedRows::None
    } else {
        ChangedRows::Rows(changed)
    }
}

/// Everything [`evaluate_frame_dirty_state`] computes that the rest of
/// [`FreminalTerminalWidget::show`] needs afterward: the selected
/// vertex-rebuild path, the individual damage observations, and the derived
/// cursor/selection/search/image state shared by the cursor-only and
/// full-rebuild branches (and, for a few fields, by the post-branch
/// animation bookkeeping that runs after both).
pub(super) struct DirtyTrackingOutcome {
    /// Which vertex-rebuild path to take this frame.
    pub(super) rebuild: VertexRebuild,
    /// The individual `*_changed` observations the decision was derived
    /// from.
    pub(super) observations: FrameDirtyObservations,
    /// Exactly which window rows differ from the last full vertex rebuild,
    /// as computed by [`diff_row_epochs`]. `observations.content_changed`
    /// collapses this (and the theme/dims/folds triggers) to a `bool`; this
    /// field keeps the row-level detail so the caller can build
    /// [`crate::gui::renderer::PaneFrameDamage::Region`] from it instead of
    /// forcing a full-pane repaint for a bounded set of changed rows.
    ///
    /// Task 124.14: this is [`ChangedRows::Rows`] or [`ChangedRows::None`]
    /// (the latter when a selection-only and/or hover-only change, Tasks
    /// 124.14b-i/124.14b-ii, is the sole trigger) whenever [`Self::rebuild`]
    /// is [`VertexRebuild::Bounded`] — that variant is selected below
    /// precisely because this field is one of those two shapes and no
    /// other full-repaint trigger fired — so the caller may read it
    /// unconditionally in that branch, alongside the selection's and
    /// hover's screen-row spans, to build the damage union.
    pub(super) changed_rows: ChangedRows,
    /// The normalised selection for this frame (buffer-absolute rows),
    /// after the content-change auto-clear rule has been applied.
    pub(super) current_selection: Option<(CellCoord, CellCoord)>,
    /// The selection translated into snapshot-row space, clamped to the
    /// flattened window, for the renderer.
    pub(super) screen_selection: Option<(usize, usize, usize, usize)>,
    /// Fingerprint of this frame's search-highlight state (cached for next
    /// frame's comparison). See `SearchState::render_epoch`.
    pub(super) search_epoch: u64,
    /// Command-block gutter hover-tint row range this frame.
    ///
    /// Despite the name inherited from `compute_command_block_hover_rows`'s
    /// doc, this is already **screen**-row space, not rendered-row space:
    /// that function's own final step calls `layout.rendered_to_screen`
    /// before returning (124.14b-ii recon), because its result is consumed
    /// directly as an index into the screen-indexed `rendered_shaped_lines`
    /// array at the `widget.rs` call site — the same array `selection`'s
    /// already-screen-space `screen_selection_rendered` indexes. So unlike
    /// selection's `previous_selection` (buffer-absolute, needing a genuine
    /// per-frame translation and therefore a dedicated screen-space
    /// companion field), this value needs no further conversion to be
    /// unioned into [`build_bounded_damage`]'s row set, and needs no second
    /// field either.
    pub(super) command_block_hover_rows: Option<(usize, usize)>,
    /// This frame's cursor screen row, or `None` when the cursor is hidden.
    /// Used with the prior rendered row to bound cursor damage during a full
    /// rebuild without repeating the snapshot-to-screen translation.
    pub(super) cursor_screen_row: Option<usize>,
    /// Whether blink, position, visibility, color, or trail state changed.
    /// The bounded-damage caller uses this same signal rather than duplicating
    /// the cursor-change predicate.
    pub(super) cursor_state_changed: bool,
    /// Whether the cursor should actually be drawn this frame (DECTCEM,
    /// echo-off, active-pane, and fold-visibility all folded in).
    pub(super) effective_show_cursor: bool,
    /// Pixel position of the (possibly trail-animated) visual cursor.
    pub(super) cursor_pixel_pos: (f32, f32),
    /// Horizontal scale factor for the cursor quad (`2.0` on a
    /// DECDWL/DECDHL row, `1.0` otherwise).
    pub(super) cursor_x_scale: f32,
    /// Whether the cursor-trail animation is still interpolating; drives a
    /// repaint request after the branch below runs.
    pub(super) cursor_animating: bool,
    /// This frame's Kitty animated-image playback tick result.
    pub(super) image_anim_tick: ImageAnimationTick,
}

/// Compute this frame's dirty-tracking decision: which vertex-rebuild path
/// [`FreminalTerminalWidget::show`] should take, plus every derived
/// observation the two candidate branches (and the post-branch animation
/// bookkeeping) need.
///
/// This is `show`'s largest near-pure block. `cache` is read-only here —
/// nothing is written back into it; the caller does that once the branch it
/// selects has actually run. The only non-freminal touch is locking
/// `render_state` to check whether `deco_verts` is empty.
///
/// Mutates `view_state` in three ways, in this order (preserved exactly,
/// since later reads in this same function depend on the earlier writes):
/// clearing the selection when content genuinely changed (and no selection
/// is in progress or was just committed), resetting the one-frame
/// `selection_committed_this_frame` edge flag, and advancing the cursor
/// trail / image animation clocks.
/// The borrowed per-frame state [`evaluate_frame_dirty_state`] reads.
///
/// Grouped so the function stays under clippy's `too_many_arguments`
/// threshold without a suppression. Every field is `Copy` (shared
/// references and a `u64`), so the struct destructures by value at the top
/// of the function and the body reads exactly as it did inline.
#[derive(Clone, Copy)]
pub(super) struct FrameDirtyContext<'a> {
    /// The snapshot being rendered this frame.
    pub(super) snap: &'a TerminalSnapshot,
    /// Previous-frame values this decision diffs against. Read-only here.
    pub(super) cache: &'a PaneRenderCache,
    /// Locked only to test whether `deco_verts` is empty.
    pub(super) render_state: &'a Arc<Mutex<RenderState>>,
    /// Command-block fold layout for this frame.
    pub(super) layout: &'a FoldLayout,
    /// Fold-layout generation counter, diffed against the cache.
    pub(super) fold_epoch: u64,
    /// Command-block config, for the gutter hover-row test.
    pub(super) command_blocks_config: &'a freminal_common::config::CommandBlocksConfig,
}

/// Pixel geometry [`evaluate_frame_dirty_state`] needs to translate the
/// selection into screen coordinates and place the cursor.
#[derive(Clone, Copy)]
pub(super) struct FrameDirtyGeometry {
    /// The pane's full rect, including the command-block gutter.
    pub(super) pane_rect: Rect,
    /// The terminal band's rect, i.e. `pane_rect` minus the gutter inset.
    pub(super) terminal_rect: Rect,
    /// Total gutter inset in logical points.
    pub(super) gutter_inset: f32,
    /// Logical (not physical) cell height.
    pub(super) logical_cell_h: f32,
    /// Cell width in physical pixels.
    pub(super) cell_w_f: f32,
    /// Row height in physical pixels.
    pub(super) row_h_f: f32,
}

/// Cursor-related inputs for one frame.
///
/// These are three independent simultaneous conditions plus a duration, not
/// a state machine — but they are *parameters*, and a positional bool list
/// is the case `freminal-state-representation` rule 1 forbids outright.
/// Naming them as fields is what makes the call site readable.
#[derive(Clone, Copy)]
pub(super) struct CursorFrameInputs {
    /// Whether the blink phase is currently in its visible half.
    pub(super) blink_on: bool,
    /// Whether the cursor should be drawn at all this frame. Reassigned
    /// inside the function and returned in [`DirtyTrackingOutcome`].
    pub(super) show_cursor: bool,
    /// Whether the cursor-trail animation is enabled by config.
    pub(super) trail_enabled: bool,
    /// How long a cursor-trail animation runs.
    pub(super) trail_duration: Duration,
}

/// Decide, for one pane in one frame, what has changed since the previous
/// frame and therefore what must be rebuilt.
///
/// Extracted from `show` by subtask 122.11. It folds the per-frame dirty
/// observations ([`FrameDirtyObservations`]) against the cached previous-frame
/// values and yields a [`DirtyTrackingOutcome`] saying which vertex buffers
/// need rebuilding ([`VertexRebuild`]), whether the cursor is drawn, and what
/// repaint the pane wants next.
///
/// Pure with respect to egui: it takes already-sampled inputs
/// ([`FrameDirtyContext`], [`FrameDirtyGeometry`], [`CursorFrameInputs`])
/// rather than a `Ui`, which is what makes the decision unit-testable — it was
/// previously an inline block inside `show` and reachable only by rendering a
/// live frame.
///
// `too_many_lines` is genuine: the length comes from the block being
// extracted, which this does not shorten. The argument count was NOT
// inherited -- the block was inline and took no parameters at all, so the
// extraction introduced the signature -- and it is fixed by grouping above
// rather than suppressed.
#[allow(clippy::too_many_lines)]
pub(super) fn evaluate_frame_dirty_state(
    ctx: &FrameDirtyContext<'_>,
    view_state: &mut ViewState,
    geometry: FrameDirtyGeometry,
    cursor: CursorFrameInputs,
) -> DirtyTrackingOutcome {
    let &FrameDirtyContext {
        snap,
        cache,
        render_state,
        layout,
        fold_epoch,
        command_blocks_config,
    } = ctx;
    let FrameDirtyGeometry {
        pane_rect,
        terminal_rect,
        gutter_inset,
        logical_cell_h,
        cell_w_f,
        row_h_f,
    } = geometry;
    let CursorFrameInputs {
        blink_on: cursor_blink_on,
        show_cursor: mut effective_show_cursor,
        trail_enabled: cursor_trail,
        trail_duration: cursor_trail_duration,
    } = cursor;

    let row_map = &layout.row_map;

    // ── History: why this used to be an `Arc::ptr_eq` test ──────────
    //
    // Before Task 124.12, content changes were detected via `Arc::ptr_eq`
    // against `cache.last_rendered_visible` / `cache.last_rendered_line_widths`
    // rather than the snapshot's own `content_changed` flag (issue #439
    // fix #4). The reason was that `content_changed` is a **sticky bool**
    // baked into the published snapshot at build time: it cannot survive
    // the ~14 unrendered snapshots the PTY thread publishes between the GUI
    // frames that actually get drawn. Re-reading the SAME published `Arc`
    // on each of those idle frames read a stale `true` and forced a full
    // vertex rebuild every frame — a ~60fps rebuild for a screen changing a
    // few times/sec.
    //
    // `Arc::ptr_eq` "fixed" the staleness (a settled screen re-observes the
    // same allocation and correctly reports "unchanged") but introduced its
    // own defect, confirmed by Task 123's measurement: `flatten_visible`
    // allocates a fresh `Arc` on *every* re-flatten, including one that
    // reproduces byte-identical content (e.g. a cursor-blink repaint, or a
    // TUI redrawing an unchanged line by idiom). Pointer inequality then
    // reports "changed" for a frame that changed nothing, forcing a full
    // rebuild — roughly 350x the bytes for roughly 1.08x the calls versus a
    // cursor-only frame.
    //
    // The fix is not a better predicate over the same two types (a sticky
    // bool, an allocation pointer) — it is a third type that is neither: a
    // per-row content epoch (`TerminalSnapshot::row_epochs`, Tasks
    // 124.10-124.11), a globally-monotonic stamp bumped only when a row's
    // *merged* content actually differs from what it replaced. It is
    // level-triggered like the pointer test (so it survives unrendered
    // snapshots, unlike the bool) but does not bump on a byte-identical
    // re-flatten (so it does not false-positive on a fresh allocation,
    // unlike the pointer test). `diff_row_epochs` below is that comparison.
    //
    // Also force a full rebuild when the theme palette changes, since
    // foreground/background colors are baked into the vertex buffers.
    let theme_changed = cache
        .previous_theme
        .is_none_or(|prev| !std::ptr::eq(prev, snap.theme));
    // Detect terminal grid resize (cols or rows changed).  The cell
    // background and foreground instance VBOs hold per-cell vertices
    // that encode column indices and pixel positions based on the
    // terminal width at build time; drawing them into a viewport sized
    // for a different column count leaves stale glyph slivers at the
    // right edge.  Force a full rebuild on resize.
    let dims_changed = snap.term_width != cache.previous_term_width
        || snap.term_height != cache.previous_term_height;
    // Force a rebuild when the fold-range set changes (user folded or
    // unfolded a command block): the rendered row layout shifts, so
    // the cached background/foreground vertex buffers are stale even
    // if `visible_chars` is byte-identical.
    let folds_changed = fold_epoch != cache.previous_fold_epoch;
    // The primary content-change signal (Task 124.12): diff this frame's
    // per-row epochs against the epochs recorded at the last full rebuild.
    // `theme_changed`, `dims_changed` and `folds_changed` stay separate ORs
    // because they are genuinely global triggers, not row-level content —
    // deliberately NOT folded into `rows_changed` itself.
    let rows_changed = diff_row_epochs(cache.last_rendered_row_epochs.as_ref(), &snap.row_epochs);
    let content_changed = theme_changed || dims_changed || folds_changed || rows_changed.any();

    // Clear the selection when actual terminal text content changes so
    // stale highlights don't linger over shifted text. We use
    // `rows_changed.any()` here (Task 124.12; previously `snap.content_changed`)
    // as the cheap pre-filter, then confirm against a chars-only comparison
    // before actually discarding anything — see below for why the
    // confirmation step still exists and still deliberately ignores the
    // epoch.
    //
    // We also exclude scroll events (`scroll_changed`) — when the
    // visible window moves (user scrolling OR auto-scroll-to-bottom on
    // new PTY output), the flat content changes but the underlying
    // buffer text at the selected rows has not mutated.  Selection
    // coordinates are buffer-absolute, so they remain valid across
    // scroll offset changes.
    //
    // Edge case: if `enforce_scrollback_limit` evicts rows from the
    // top of the buffer, all row indices shift and the selection may
    // point to different text.  This is a pre-existing limitation
    // shared by all finite-scrollback terminals; the proper fix is to
    // adjust selection coordinates on eviction, not to clear here.
    //
    // We also exclude frames where a selection was just finalized by
    // a mouse release (`selection_committed_this_frame`). Input is
    // processed before this auto-clear runs each frame, so by the
    // time we get here `selection.is_selecting` is already `false`
    // for a just-completed selection. Without this flag, PTY output
    // that arrives on the same frame as the release would set
    // `rows_changed.any()` and immediately wipe the just-committed
    // selection (defect 2, Task 116.2).
    //
    // Before Task 124.12 the pre-filter was the sticky `snap.content_changed`
    // bool, which is edge-triggered per *snapshot build* while the GUI
    // renders only a subset of the snapshots the PTY thread produces. A
    // change that reverted before the next rendered frame -- a prompt
    // clearing and rewriting its own line, say -- therefore arrived as
    // `content_changed = true` on a snapshot whose text was identical to
    // what this pane already drew:
    //
    //     snapshot A: text X                  -> rendered
    //     snapshot B: text Y, changed = true  -> never rendered
    //     snapshot C: text X, changed = true  -> rendered (Y != X at build C)
    //
    // Acting on that wiped selections for no reason, intermittently, whenever
    // a mouse release happened to land across such a flicker (#470). The
    // confirmation comparison below existed to catch exactly this, and it
    // still does — `rows_changed` inherits the same false-positive shape
    // (it is level-triggered against the last *rendered* frame, so it
    // reports "changed" whenever the PTY-side content differs from what
    // this pane drew, even if it later reverts before the *next* rendered
    // frame) and needs the same backstop.
    //
    // The confirmation stays a **chars-only** comparison. The epoch
    // deliberately folds in format tags and `LineWidth`, so switching this
    // to the epoch alone would start clearing the user's selection on an
    // SGR-only repaint -- a program redrawing identical text in a
    // different colour. Selection is about where text *is*, not how it
    // looks. `rows_changed` replaces `snap.content_changed` purely as the
    // cheap pre-filter it always was; the confirmation's job and mechanism
    // are unchanged.
    //
    // Evaluated last so the O(visible_chars) comparison only runs on the rare
    // frame where every cheap condition already passed.
    // `has_selection()` first: with nothing selected the clear is a no-op, so
    // this skips the comparison below on every frame of continuous PTY output.
    // Equivalent, not merely cheaper -- a degenerate selection (`anchor ==
    // end`, or `end == None`) reports `false` here and `clear()` would have
    // done nothing to it either, so a later primary press still starts a
    // fresh drag rather than being consumed dismissing a phantom.
    if view_state.selection.has_selection()
        && rows_changed.any()
        && !snap.scroll_changed
        && !view_state.selection.is_selecting
        && !view_state.selection_committed_this_frame
        && cache
            .last_rendered_visible
            .as_ref()
            .is_none_or(|prev| prev.as_ref() != snap.visible_chars.as_ref())
    {
        view_state.selection.clear();
    }
    // Reset the per-frame edge flag unconditionally so it does not
    // persist into subsequent frames (Task 116.2).
    view_state.selection_committed_this_frame = false;

    // Check whether the selection has changed since the last frame.
    let current_selection = view_state.selection.normalised();
    let selection_changed = current_selection != cache.previous_selection;

    // Check whether search highlight state has changed since last frame.
    // Compares a fingerprint of everything that determines the highlight
    // geometry, not just the match count and focused index -- see
    // `SearchState::render_epoch` and issue #463.
    let search_epoch = view_state.search_state.render_epoch();
    let search_changed = search_epoch != cache.previous_search_epoch;

    // Convert buffer-absolute selection coordinates to snapshot-row
    // space for the renderer.  `win_start` is the flattened window top
    // (it includes the fold extra rows); the snapshot covers `snap_rows`
    // rows.  Selection rows are later mapped snapshot → rendered →
    // screen alongside the shaped lines.
    let win_start = layout.flat_window_start;
    let snap_rows = snap.term_height.saturating_add(snap.window_extra_rows);

    // Compute the command-block hover-row range NOW (before the
    // vertex-rebuild decision) so a hover-only change — which does not
    // touch text content, selection, or search — still forces a full
    // rebuild.  The hover tint is baked into the background instance
    // VBO, so without this a hover change would be invisible until some
    // other event (PTY output, fold) invalidated the cache.
    let command_block_hover_rows_early = compute_command_block_hover_rows(
        snap,
        view_state,
        command_blocks_config,
        layout,
        pane_rect,
        terminal_rect,
        gutter_inset,
        logical_cell_h,
    );
    let hover_changed = command_block_hover_rows_early != cache.previous_command_block_hover_rows;

    let screen_selection = current_selection.and_then(|(s, e)| {
        // Clamp the selection to the flattened window.  If both start
        // and end are outside the window, there is nothing to
        // highlight on screen.
        let win_end = win_start + snap_rows;
        if e.row < win_start || s.row >= win_end {
            return None; // entirely outside visible window
        }
        let s_row = s.row.saturating_sub(win_start);
        let e_row = e
            .row
            .saturating_sub(win_start)
            .min(snap_rows.saturating_sub(1));

        let is_block = view_state.selection.is_block;

        // For linear selections, when the start row is above the
        // visible window the selection begins at column 0 of the first
        // visible row.  Block selections always preserve the original
        // column bounds regardless of row clamping.
        let s_col = if !is_block && s.row < win_start {
            0
        } else {
            s.col
        };
        // Similarly, linear selections that extend below the window
        // run to the last column.  Block selections keep their column.
        let e_col = if !is_block && e.row >= win_end {
            snap.term_width.saturating_sub(1)
        } else {
            e.col
        };
        Some((s_col, s_row, e_col, e_row))
    });

    // ── Cursor trail animation ─────────────────────────────────────
    // Update the animated cursor position.  When trail is enabled, the
    // visual position glides from the previous location to the new one.
    // When disabled, it snaps instantly.
    //
    // The animation target is in **rendered-row** space — when a fold
    // collapses rows above the cursor, the cursor's rendered row index
    // is less than `snap.cursor_pos.y`.  If the cursor's snapshot row
    // is *inside* a folded range (which shouldn't happen normally
    // because the prompt is never folded, but is defensible against
    // races) we suppress the cursor for this frame.
    // The cursor row is reported relative to the *normal* visible
    // window top; shift it into snapshot-row space (the flattened
    // window has `window_extra_rows` extra rows above it), map through
    // the fold collapse, then to the bottom-anchored screen row.
    let cursor_snap_row = snap.cursor_pos.y.saturating_add(snap.window_extra_rows);
    let cursor_screen_row = row_map
        .snapshot_to_rendered(cursor_snap_row)
        .and_then(|rendered| layout.rendered_to_screen(rendered));
    let cursor_visible = cursor_screen_row.is_some();
    // If the cursor's snapshot row is hidden behind a fold (or scrolled
    // off the top), suppress it for this frame.  AND-ing here means the
    // cursor-only fast path and the full rebuild path agree on
    // visibility.
    effective_show_cursor = effective_show_cursor && cursor_visible;
    let target_col = snap.cursor_pos.x.approx_as::<f32>().unwrap_or(0.0);
    let target_row = cursor_screen_row
        .unwrap_or(snap.cursor_pos.y)
        .approx_as::<f32>()
        .unwrap_or(0.0);
    let cursor_animating = view_state.update_cursor_animation(
        target_col,
        target_row,
        cursor_trail,
        cursor_trail_duration,
    );

    // Compute the pixel position from the (possibly animated) visual
    // cursor coordinates.  These are fractional cell coords, so we
    // multiply by cell dimensions in pixels.
    //
    // For double-width / double-height rows (DECDWL / DECDHL), the
    // cursor x-position is scaled by the row's horizontal scale factor
    // so it aligns with the magnified glyphs.
    let cursor_row_lw = snap
        .visible_line_widths
        .get(cursor_snap_row)
        .copied()
        .unwrap_or(freminal_terminal_emulator::LineWidth::Normal);
    let cursor_x_scale = if cursor_row_lw.is_double_width() {
        2.0
    } else {
        1.0
    };
    let cursor_pixel_pos = (
        view_state.cursor_visual_col * cell_w_f * cursor_x_scale,
        view_state.cursor_visual_row * row_h_f,
    );

    // ── Kitty animated image playback (Task 100.2c) ─────────────────
    // Advance the GUI-side wall-clock frame selector for every
    // animated image visible in this snapshot. A frame change forces
    // the full-rebuild path below (via `image_frame_changed`) so the
    // cloned `snap_images` picks up the newly-selected frame's pixels
    // before `sync_image_textures` runs.
    let anim_tick = view_state.tick_image_animations(&snap.images);
    let image_frame_changed = !anim_tick.changed.is_empty();

    // ── Store-level image pixel mutation detection (Task 100.12) ────
    // A Kitty `a=c` animation compose overwrites an existing frame's
    // pixels in place (a new `Arc<Vec<u8>>` for that frame) without
    // touching any cell or `run_mode`, so `content_changed` and
    // `image_frame_changed` both stay false and the full-rebuild path
    // (the only path that refreshes `snap_images` and therefore
    // drives `sync_image_textures`) never runs. Compare the
    // currently-selected-frame pixel pointer for every visible image
    // against what was actually uploaded last frame to catch this
    // case (and any other store-only pixel mutation) directly.
    //
    // This is recomputed unconditionally every frame (cheap — a
    // `HashMap` build over visible images, typically empty) and the
    // cache is refreshed only when a full rebuild actually runs (see
    // below), so the comparison always reflects what the GPU last
    // saw.
    let image_pixels_changed = image_pixels_changed(
        &snap.images,
        |id| view_state.selected_frame(id),
        &cache.last_rendered_image_pixel_ptrs,
    );

    // Determine whether we can take the cursor-only fast path.
    //
    // Cursor-only: content has not changed, the selection has not
    // changed, but the cursor blink state or position has changed
    // since the last frame.  We only need to patch the cursor quad
    // in the background VBO — no re-shaping and no full vertex
    // rebuild required.
    //
    // When cursor trail is animating, we also enter the cursor-only
    // path so the visual position is updated each frame.
    let cursor_state_changed = cursor_blink_on != cache.previous_cursor_blink_on
        || snap.cursor_pos != cache.previous_cursor_pos
        || effective_show_cursor != cache.previous_show_cursor
        || snap.cursor_color_override != cache.previous_cursor_color_override
        || cursor_animating;

    // A text-blink visibility change requires rebuilding the foreground
    // vertex buffer (glyphs are included or excluded per run).  This is
    // a separate trigger from cursor-only so it always goes through the
    // full rebuild path.
    let text_blink_changed = snap.has_blinking_text
        && (view_state.text_blink_slow_visible != cache.previous_text_blink_slow_visible
            || view_state.text_blink_fast_visible != cache.previous_text_blink_fast_visible);

    // A pane whose decoration buffer was never populated has nothing on
    // screen to preserve, so neither bounded path (cursor-only or
    // row-bounded) may claim the rest of the pane is intact. 124.21 lists
    // this among the eight genuinely-global triggers. Read once and shared
    // by both decisions below rather than re-locking.
    let deco_verts_empty = render_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .deco_verts
        .is_empty();

    let cursor_only = !content_changed
        && !selection_changed
        && !text_blink_changed
        && !search_changed
        && !hover_changed
        && !image_frame_changed
        && !image_pixels_changed
        && cursor_state_changed
        && !deco_verts_empty;

    // Task 124.14: a full rebuild whose damage is provably bounded, rather
    // than the whole pane. `content_changed` ORs four independent triggers
    // together (`theme_changed`, `dims_changed`, `folds_changed`,
    // `rows_changed.any()`); this frame's damage can only be named as a
    // known set of rows when `rows_changed` is not `ChangedRows::All` (a
    // `ChangedRows::All` answer -- no baseline, or a window-size change --
    // is not a row list at all, so there is nothing to bound to) and none
    // of the other five full-repaint triggers this decision already checks
    // for `cursor_only` fired either. Those are exactly 124.21's audit
    // boundary, narrowed by one entry each at 124.14b-i, 124.14b-ii, and
    // 124.14d: `selection_changed`, `hover_changed`, and `search_changed`
    // are now folded INTO the bound rather than vetoing it (each one's
    // screen-row extent is known at the decision point -- `hover_changed`'s
    // exactly as 124.14b-i's recon found it BOUNDABLE-NOW, and the
    // gutter-escape hazard that recon also raised was investigated and
    // disproved before implementation: the hover tint is baked into the
    // background instance buffer inside `terminal_rect`, same as selection,
    // not painted over the gutter strip; `search_changed`'s screen-row
    // extent is the union of `PaneRenderCache::search_damage`'s previous
    // highlight rows and this frame's current ones, derived from
    // `search_highlights` at the `widget.rs` call site rather than
    // re-translating `MatchSpan`s a second time -- and the caller unions
    // all three into `changed_rows` rather than treating any of them as
    // full-repaint-forcing). `image_frame_changed` and
    // `image_pixels_changed` are explicitly excluded (124.C5).
    // `text_blink_changed` has no per-row bitmap. Both stay full-repaint
    // triggers here, deliberately.
    //
    // `ChangedRows::None` combined with `selection_changed`, `hover_changed`,
    // and/or `search_changed` alone is a genuine, common case this bound now
    // covers: a selection-only, hover-only, or search-only change (extending,
    // shrinking, or clearing a selection; moving the hover between command
    // blocks; editing a search query with no row-epoch change at all) has no
    // changed row at all, but its damage is still fully bounded to that
    // source's screen-row span -- see `(rows_changed.any() ||
    // selection_changed || hover_changed || search_changed)` below. A
    // search-only change whose old/new highlight-row union is itself empty
    // (no visible matches before or after) is a further special case: see
    // `build_bounded_damage`'s `EmptyBoundedDamage` parameter in `widget.rs`
    // for why that reports `Unchanged`, not `Full`.
    //
    // `deco_verts_empty` is included for the same reason it vetoes
    // `cursor_only`: with no previously-populated decoration buffer there is
    // nothing on screen for a bounded region to leave intact. In the normal
    // flow that case already implies `ChangedRows::All` (a fresh
    // `PaneRenderCache` starts with no epoch baseline), but that is an
    // invariant spanning two independently-owned structures --
    // `RenderState` is per-window, `PaneRenderCache` per-pane -- so it is
    // asserted here rather than relied upon.
    let bounded_change = !matches!(rows_changed, ChangedRows::All)
        && !deco_verts_empty
        && !theme_changed
        && !dims_changed
        && !folds_changed
        && !text_blink_changed
        && !image_frame_changed
        && !image_pixels_changed
        && (rows_changed.any() || selection_changed || hover_changed || search_changed);

    DirtyTrackingOutcome {
        rebuild: if cursor_only {
            VertexRebuild::CursorOnly
        } else if bounded_change {
            VertexRebuild::Bounded
        } else {
            VertexRebuild::ReevaluateFullRebuild
        },
        observations: FrameDirtyObservations {
            content_changed,
            selection_changed,
            search_changed,
            hover_changed,
            image_frame_changed,
            image_pixels_changed,
            text_blink_changed,
        },
        changed_rows: rows_changed,
        current_selection,
        screen_selection,
        search_epoch,
        command_block_hover_rows: command_block_hover_rows_early,
        cursor_screen_row: effective_show_cursor.then_some(cursor_screen_row).flatten(),
        cursor_state_changed,
        effective_show_cursor,
        cursor_pixel_pos,
        cursor_x_scale,
        cursor_animating,
        image_anim_tick: anim_tick,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod evaluate_frame_dirty_state_tests {
    //! Unit tests for [`evaluate_frame_dirty_state`] (Task 122.11), calling
    //! the real decision function rather than re-implementing its boolean
    //! logic inline. The property that matters most — and the one every
    //! test here is built around — is that [`VertexRebuild::CursorOnly`]
    //! must fire *only* when every content/selection/search/hover/image/
    //! text-blink observation is false, the cursor state genuinely changed,
    //! and the decoration buffer is non-empty. Getting any of those terms
    //! wrong is a visible rendering bug (a stale frame or a needless full
    //! rebuild every frame), not just a wrong test assertion.
    use super::super::widget::new_render_state;
    use super::*;
    use crate::gui::renderer::WindowPostRenderer;
    use crate::gui::view_state::SearchState;
    use freminal_common::config::CommandBlocksConfig;
    use freminal_terminal_emulator::snapshot::TerminalSnapshot;
    use freminal_terminal_emulator::{
        AnimationControl, AnimationRunMode, ImageFrame, ImageSizeMode, InlineImage,
    };

    /// A minimal, deterministic snapshot: a 10x5 grid, cursor at the
    /// origin, no command blocks, no blinking text — chosen so the cursor
    /// is always visible (no folds, no scrolled-off rows) and the
    /// command-block hover lookup always short-circuits to `None`.
    ///
    /// `row_epochs` carries one distinct, non-zero stamp per row (rather
    /// than the all-empty default from `TerminalSnapshot::empty()`) so
    /// tests can mutate a single entry to simulate exactly one row
    /// changing, and so a length mismatch (a resize) is distinguishable
    /// from "no epochs recorded at all".
    fn base_snapshot() -> TerminalSnapshot {
        let mut snap = TerminalSnapshot::empty();
        snap.term_width = 10;
        snap.term_height = 5;
        snap.total_rows = 5;
        snap.row_epochs = Arc::from(vec![1_u64, 2, 3, 4, 5]);
        snap
    }

    /// A [`PaneRenderCache`] pre-populated to describe "the last full
    /// rebuild rendered exactly `snap`, with the given cursor-blink phase
    /// and effective cursor visibility" — i.e. a settled frame where
    /// re-observing the identical `snap` unmodified should report no
    /// changes at all.
    fn settled_cache(
        snap: &TerminalSnapshot,
        cursor_blink_on: bool,
        effective_show_cursor: bool,
    ) -> PaneRenderCache {
        let mut cache = PaneRenderCache::new();
        cache.previous_theme = Some(snap.theme);
        cache.previous_term_width = snap.term_width;
        cache.previous_term_height = snap.term_height;
        cache.previous_fold_epoch = 0;
        cache.last_rendered_visible = Some(Arc::clone(&snap.visible_chars));
        cache.last_rendered_row_epochs = Some(Arc::clone(&snap.row_epochs));
        cache.previous_selection = None;
        // Must agree with `previous_selection` (Task 124.14b-i) -- see that
        // field's doc comment. A "settled" cache that disagreed on this
        // pair would be describing a state `show()` itself can never
        // produce (the two are always written together).
        cache.previous_selection_screen_rows = None;
        // These tests drive `ViewState::new()`, whose search state is
        // default-constructed, so a settled cache is one that already agrees
        // with that state's fingerprint.
        cache.previous_search_epoch = SearchState::default().render_epoch();
        cache.previous_command_block_hover_rows = None;
        // Must agree with `previous_command_block_hover_rows` (Task
        // 124.14b-ii) -- see that field's doc comment. A "settled" cache
        // that disagreed on this pair would be describing a state `show()`
        // itself can never produce (the two are always written together).
        cache.previous_cursor_blink_on = cursor_blink_on;
        cache.previous_cursor_pos = snap.cursor_pos;
        cache.previous_show_cursor = effective_show_cursor;
        cache.previous_cursor_color_override = snap.cursor_color_override;
        cache.previous_text_blink_slow_visible = true;
        cache.previous_text_blink_fast_visible = true;
        cache
    }

    /// A fresh, GL-context-free `Arc<Mutex<RenderState>>`, optionally with a
    /// non-empty `deco_verts` (mirroring "a previous full rebuild already
    /// ran and left decoration vertices behind for the cursor-only patch
    /// path to overwrite").
    fn render_state_with_deco_verts(non_empty: bool) -> Arc<Mutex<RenderState>> {
        let rs = new_render_state(Arc::new(Mutex::new(WindowPostRenderer::new())));
        if non_empty {
            rs.lock().unwrap().deco_verts.push(0.0);
        }
        rs
    }

    /// Call `evaluate_frame_dirty_state` with fixed, inert geometry
    /// (irrelevant here because `base_snapshot` has no command blocks, so
    /// the hover lookup short-circuits before touching any of it) and the
    /// given cache/cursor-state inputs.
    fn call(
        snap: &TerminalSnapshot,
        view_state: &mut ViewState,
        cache: &PaneRenderCache,
        render_state: &Arc<Mutex<RenderState>>,
        cursor_blink_on: bool,
        effective_show_cursor: bool,
    ) -> DirtyTrackingOutcome {
        let layout = FoldLayout::new(snap, &view_state.folded_blocks);
        let command_blocks_config = CommandBlocksConfig::default();
        let pane_rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 100.0));
        let terminal_rect = pane_rect;
        evaluate_frame_dirty_state(
            &FrameDirtyContext {
                snap,
                cache,
                render_state,
                layout: &layout,
                fold_epoch: 0,
                command_blocks_config: &command_blocks_config,
            },
            view_state,
            FrameDirtyGeometry {
                pane_rect,
                terminal_rect,
                gutter_inset: 0.0,
                logical_cell_h: 10.0,
                cell_w_f: 8.0,
                row_h_f: 16.0,
            },
            CursorFrameInputs {
                blink_on: cursor_blink_on,
                show_cursor: effective_show_cursor,
                trail_enabled: false,
                trail_duration: Duration::from_millis(120),
            },
        )
    }

    #[test]
    fn settled_frame_with_no_changes_is_not_cursor_only() {
        // Nothing changed at all, not even the cursor: the fast path must
        // NOT fire, because `cursor_state_changed` is false. This is the
        // common steady-state frame that (at the call site) takes neither
        // branch — it re-draws the existing VBO data unchanged.
        let snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);
        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, true, true);

        assert_eq!(outcome.rebuild, VertexRebuild::ReevaluateFullRebuild);
        assert!(!outcome.observations.content_changed);
        assert!(!outcome.observations.selection_changed);
        assert!(!outcome.observations.search_changed);
        assert!(!outcome.observations.hover_changed);
    }

    /// OBLIGATION 1 of `PLAN_123_GL_MEASUREMENT_HARNESS.md` confirmed the
    /// always-new-`Arc` finding that was Task 124.1's premise: a re-flatten
    /// that produces byte-identical content in a freshly-allocated `Arc`
    /// used to be reported as a content change, forcing a full vertex
    /// rebuild. Tasks 124.10-124.12 fixed it, and this test — inverted, not
    /// deleted, per this subtask's mandate — is now the regression guard
    /// for the fix.
    ///
    /// This test isolates the single variable. It starts from a settled
    /// frame, then swaps in a `visible_chars` `Arc` whose *contents* are
    /// equal by value but whose allocation differs, holding every other
    /// input — theme, dimensions, fold epoch, and (crucially) `row_epochs`
    /// — value-identical to the settled cache. That last point is what
    /// "byte-identical" now means under the new mechanism: the merged
    /// bytes did not change, so neither did the per-row epoch stamped from
    /// them. The only thing that changed is the `visible_chars` allocation.
    ///
    /// The upstream cause is unchanged, and still un-fixed by this task
    /// (that is the bandwidth half, Task 125): in `freminal-buffer`,
    /// `rows_as_tchars_and_tags_incremental` returns the cached `Arc`s
    /// unchanged **only** on its no-op path (`reuse_available` and no row
    /// in the window needed rebuilding). Both the incremental fast path and
    /// the full-merge fallback finish with `Arc::new(chars)`, a fresh
    /// allocation, regardless of whether the merged bytes are identical to
    /// the previous merge. So any dirty row anywhere in the window — a
    /// cursor blink repainting its cell, a row rewritten with the same
    /// text — still produces a new `visible_chars` pointer every time.
    ///
    /// What changed is the *consumption* side: the GUI no longer diffs that
    /// pointer at all. It diffs `TerminalSnapshot::row_epochs` instead, a
    /// per-row monotonic stamp that only bumps when the merged content
    /// actually differs, so a fresh allocation with identical bytes now
    /// correctly reports no change. The proof used here is that the
    /// cursor-only fast path — previously vetoed by the pointer-based
    /// `content_changed` on every such frame — is now reachable: changing
    /// only the cursor-blink phase on top of the byte-identical re-flatten
    /// takes [`VertexRebuild::CursorOnly`], which was impossible before
    /// this fix (see [`re_observing_the_same_arc_reports_no_content_change`]
    /// for the same-`Arc` control, and
    /// [`a_changed_row_epoch_is_reported_as_exactly_that_row`] for the
    /// paired control proving this is not "nothing ever changes").
    ///
    /// What this saves, per Task 123's harness: a full rebuild is ~52 GL
    /// calls and ~200 KB of buffer uploads per frame at 80x24, versus a
    /// cursor-only frame's ~48 calls and under a kilobyte. The waste
    /// avoided is overwhelmingly bandwidth, not call count.
    #[test]
    fn byte_identical_reflatten_in_a_new_arc_is_no_longer_a_content_change() {
        let snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);

        // Re-flatten: same bytes, new allocation. `row_epochs` is untouched
        // by the clone (same allocation, same values) — that IS what
        // "byte-identical" means under the epoch-based mechanism.
        let mut reflattened = snap.clone();
        reflattened.visible_chars = Arc::new((*snap.visible_chars).clone());

        assert_eq!(
            reflattened.visible_chars, snap.visible_chars,
            "precondition: the re-flatten must be byte-identical"
        );
        assert!(
            !Arc::ptr_eq(&reflattened.visible_chars, &snap.visible_chars),
            "precondition: the re-flatten must be a different allocation"
        );
        assert_eq!(
            reflattened.row_epochs, snap.row_epochs,
            "precondition: byte-identical content means the per-row epoch \
             did not bump either"
        );

        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);
        // Only the cursor-blink phase differs from the settled cache
        // (`true`); everything else -- including the reflattened, freshly
        // allocated `visible_chars` -- matches.
        let outcome = call(
            &reflattened,
            &mut view_state,
            &cache,
            &render_state,
            false,
            true,
        );

        assert!(
            !outcome.observations.content_changed,
            "FIXED: a byte-identical re-flatten in a fresh `Arc` must not be \
             reported as a content change -- the epoch, not the pointer, is \
             now the signal"
        );
        assert_eq!(
            outcome.rebuild,
            VertexRebuild::CursorOnly,
            "with content genuinely unchanged, a cursor-blink change alone \
             must take the cheap fast path -- previously impossible, since \
             the pointer-based `content_changed` vetoed it on every such \
             frame"
        );
    }

    /// The paired control for the test above: a *genuine* single-row epoch
    /// change (as opposed to a byte-identical re-flatten) is reported as
    /// exactly that row, and does force a content change. Without this,
    /// the fixed behaviour above would be equally consistent with the
    /// degenerate "nothing ever changes" — this proves the epoch diff is
    /// still change-sensitive, not merely permissive.
    #[test]
    fn a_changed_row_epoch_is_reported_as_exactly_that_row() {
        let snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);

        let mut changed = snap;
        let mut epochs = (*changed.row_epochs).to_vec();
        epochs[2] += 1;
        changed.row_epochs = Arc::from(epochs);

        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);
        let outcome = call(&changed, &mut view_state, &cache, &render_state, true, true);

        assert_eq!(
            outcome.changed_rows,
            ChangedRows::Rows(vec![2]),
            "exactly row 2 differs, so the changed-row set must name only it"
        );
        assert!(outcome.observations.content_changed);
    }

    /// [`diff_row_epochs`] with no recorded baseline (first rebuild, or
    /// `invalidate_content`) must conservatively report every row changed
    /// rather than guessing "nothing changed".
    #[test]
    fn no_recorded_epochs_reports_every_row_changed() {
        let current: Arc<[u64]> = Arc::from(vec![1_u64, 2, 3]);
        assert_eq!(diff_row_epochs(None, &current), ChangedRows::All);
    }

    /// [`diff_row_epochs`] with a length mismatch (the window's row count
    /// changed) must conservatively report every row changed, since a
    /// per-row index comparison across different lengths is not meaningful.
    #[test]
    fn an_epoch_vector_length_change_reports_every_row_changed() {
        let previous: Arc<[u64]> = Arc::from(vec![1_u64, 2, 3]);
        let current: Arc<[u64]> = Arc::from(vec![1_u64, 2, 3, 4]);
        assert_eq!(diff_row_epochs(Some(&previous), &current), ChangedRows::All);
    }

    /// [`diff_row_epochs`] with identical epoch vectors reports no change.
    #[test]
    fn identical_epochs_report_no_change() {
        let epochs: Arc<[u64]> = Arc::from(vec![1_u64, 2, 3]);
        assert_eq!(diff_row_epochs(Some(&epochs), &epochs), ChangedRows::None);
    }

    /// The justification for deleting `last_rendered_line_widths` (Task
    /// 124.12): a line-width-only change (e.g. a DECDWL/DECDHL toggle) is
    /// folded into the epoch, so it is still caught even when
    /// `visible_chars` stays pointer-identical to what was last rendered —
    /// no separate pointer test is needed.
    #[test]
    fn a_line_width_change_is_caught_by_the_epoch_without_a_separate_pointer_test() {
        let snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);

        let mut changed = snap.clone();
        let mut epochs = (*changed.row_epochs).to_vec();
        epochs[1] += 1;
        changed.row_epochs = Arc::from(epochs);

        assert!(
            Arc::ptr_eq(&changed.visible_chars, &snap.visible_chars),
            "precondition: visible_chars pointer is unchanged, isolating the \
             epoch as the only differing signal"
        );

        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);
        let outcome = call(&changed, &mut view_state, &cache, &render_state, true, true);

        assert!(
            outcome.observations.content_changed,
            "a line-width-only change must still be caught via the epoch, \
             even though visible_chars is pointer-identical"
        );
    }

    /// Item 4 of Task 124.12's scope: the selection auto-clear's
    /// confirmation stays a chars-only comparison, so an SGR-only repaint
    /// (identical text, different attributes) must not clear a committed
    /// selection even though the epoch — which deliberately folds in
    /// format tags — reports a change.
    #[test]
    fn an_sgr_only_change_does_not_clear_the_selection() {
        let snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);

        let mut changed = snap;
        bump_one_row_epoch(&mut changed);
        changed.scroll_changed = false;

        let mut view_state = ViewState::new();
        with_committed_selection(&mut view_state);
        let render_state = render_state_with_deco_verts(true);

        let _ = call(&changed, &mut view_state, &cache, &render_state, true, true);

        assert!(
            view_state.selection.has_selection(),
            "an epoch change unaccompanied by a visible_chars byte change \
             (an SGR-only repaint) must not clear the selection"
        );
    }

    /// The control for the test above: re-observing the *same* `Arc`
    /// correctly reports no content change.
    ///
    /// Without this, the confirmation above would be consistent with
    /// "`content_changed` is simply always true", which would be a
    /// different and much larger bug. Pairing them shows the detection
    /// works and is specifically pointer-sensitive.
    #[test]
    fn re_observing_the_same_arc_reports_no_content_change() {
        let snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);
        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, true, true);

        assert!(
            !outcome.observations.content_changed,
            "the same allocation must not be reported as changed"
        );
    }

    #[test]
    fn cursor_blink_change_alone_takes_the_fast_path() {
        // Everything else settled; only the cursor blink phase flipped, and
        // the decoration buffer already has data to patch into. This is
        // exactly the fast path's reason to exist.
        let snap = base_snapshot();
        // Cache remembers blink ON; this frame observes blink OFF.
        let cache = settled_cache(&snap, true, true);
        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, false, true);

        assert_eq!(outcome.rebuild, VertexRebuild::CursorOnly);
    }

    #[test]
    fn cursor_change_with_empty_deco_verts_forces_full_rebuild() {
        // Same cursor-blink change as above, but no previous rebuild ever
        // populated `deco_verts` — there is nothing to patch, so the fast
        // path must not be selected even though every other flag agrees.
        let snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);
        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(false);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, false, true);

        assert_eq!(outcome.rebuild, VertexRebuild::ReevaluateFullRebuild);
    }

    /// Pins the call-site gate `widget.rs` reads off
    /// [`DirtyTrackingOutcome::cursor_state_changed`]: a cursor move
    /// combined with a single changed row must still select
    /// `VertexRebuild::Bounded` (the row change bounds the rebuild on its
    /// own) AND report `cursor_state_changed = true`, so the caller knows
    /// to include the cursor's current/previous screen row in
    /// `build_bounded_damage`'s union rather than passing `None` for both.
    #[test]
    fn cursor_state_changed_is_true_for_a_moved_cursor_alongside_a_bounded_row_change() {
        let snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);

        let mut changed = snap;
        bump_one_row_epoch(&mut changed);
        changed.cursor_pos = freminal_common::buffer_states::cursor::CursorPos { x: 3, y: 2 };

        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);
        let outcome = call(&changed, &mut view_state, &cache, &render_state, true, true);

        assert_eq!(
            outcome.rebuild,
            VertexRebuild::Bounded,
            "a changed row plus a cursor move, with nothing else \
             different, must still take the bounded-damage full rebuild"
        );
        assert!(
            outcome.cursor_state_changed,
            "the cursor's position differs from cache.previous_cursor_pos, \
             so this must be true"
        );
    }

    /// The control for the test above: nothing about the cursor (or
    /// anything else) changed, so `cursor_state_changed` must be false --
    /// the call-site gate must not spuriously include the cursor's row in
    /// a bounded rebuild's damage union when the cursor itself did not
    /// change.
    #[test]
    fn cursor_state_changed_is_false_when_settled() {
        let snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);
        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, true, true);

        assert!(
            !outcome.cursor_state_changed,
            "a fully settled frame must report no cursor-state change"
        );
    }

    // ── VertexRebuild::Bounded boundary (Task 124.14a) ────────────────────

    /// The base case `VertexRebuild::Bounded` exists for: exactly one
    /// changed row, nothing else different, must select the bounded-damage
    /// full rebuild rather than `ReevaluateFullRebuild`, and the outcome's
    /// `changed_rows` must still name exactly that row (the caller reads
    /// it unconditionally in the `Bounded` branch to build
    /// `PaneFrameDamage::Region` — see `build_bounded_damage` in
    /// `widget.rs`).
    #[test]
    fn a_single_bounded_row_change_alone_yields_bounded_rebuild() {
        let snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);

        let mut changed = snap;
        bump_one_row_epoch(&mut changed);

        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);
        let outcome = call(&changed, &mut view_state, &cache, &render_state, true, true);

        assert_eq!(
            outcome.rebuild,
            VertexRebuild::Bounded,
            "a lone bounded row change (no other trigger) must take the \
             bounded-damage full rebuild, not the unbounded one"
        );
        assert_eq!(
            outcome.changed_rows,
            ChangedRows::Rows(vec![0]),
            "the caller reads changed_rows unconditionally when rebuild is \
             Bounded, so it must still name exactly the row that changed"
        );
    }

    /// A selection-only change -- no row changed at all, only the
    /// selection -- must also select `VertexRebuild::Bounded` (Task
    /// 124.14b-i), and `changed_rows` legitimately reports `None` in this
    /// case: there is no row list to name, but the frame is still bounded
    /// via the selection's screen-row span alone. Before 124.14b-i this
    /// exact input (no row change, selection changed) fell through to
    /// `ReevaluateFullRebuild` -- see the now-inverted
    /// `selection_change_alongside_a_row_change_now_yields_bounded` and
    /// `selection_change_beats_cursor_change` below for the paired cases
    /// this test's sibling boundary tests still guard.
    #[test]
    fn selection_only_change_alone_yields_bounded_rebuild() {
        let snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);

        let mut view_state = ViewState::new();
        view_state.selection.anchor = Some(CellCoord { col: 0, row: 0 });
        view_state.selection.end = Some(CellCoord { col: 3, row: 0 });
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, true, true);

        assert!(
            outcome.observations.selection_changed,
            "precondition: the selection must actually be observed as changed"
        );
        assert_eq!(
            outcome.changed_rows,
            ChangedRows::None,
            "precondition: no row changed -- this isolates the \
             selection-only case from the row+selection case below"
        );
        assert_eq!(
            outcome.rebuild,
            VertexRebuild::Bounded,
            "124.14b-i: a selection change with no row change must still \
             take the bounded-damage full rebuild -- ChangedRows::None is \
             an acceptable changed_rows value here because the selection's \
             own screen-row span is what bounds the damage"
        );
    }

    /// The clearing case, distinct from the extending/shrinking case above:
    /// a *previously-recorded* selection (`cache.previous_selection` is
    /// `Some`) disappears entirely this frame (`current_selection` is
    /// `None`), with no row change. `selection_changed` still fires
    /// (`current_selection != cache.previous_selection`), so this must
    /// still take `VertexRebuild::Bounded` -- if it silently fell through
    /// to something that skipped the damage bound, the pixels from the
    /// now-gone highlight would never be repainted at all. Matters because
    /// `build_bounded_damage` at the `widget.rs` call site is what actually
    /// discards the stale highlight, by reading
    /// `PaneRenderCache::previous_selection_screen_rows` (still `Some`
    /// going into this frame) even though `current_selection_screen_rows`
    /// comes out `None`.
    #[test]
    fn selection_clear_alone_yields_bounded_rebuild() {
        let snap = base_snapshot();
        let mut cache = settled_cache(&snap, true, true);
        cache.previous_selection =
            Some((CellCoord { col: 0, row: 0 }, CellCoord { col: 3, row: 0 }));

        // `ViewState::new()` starts with no selection at all, so
        // `current_selection` is `None` -- this frame observes the
        // selection having been cleared since the cached baseline.
        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, true, true);

        assert!(
            outcome.observations.selection_changed,
            "precondition: Some -> None must be observed as a selection change"
        );
        assert_eq!(
            outcome.current_selection, None,
            "precondition: this frame's selection is genuinely cleared"
        );
        assert_eq!(
            outcome.changed_rows,
            ChangedRows::None,
            "precondition: no row changed -- isolates the clearing case"
        );
        assert_eq!(
            outcome.rebuild,
            VertexRebuild::Bounded,
            "a selection being cleared (Some -> None) must still take the \
             bounded-damage path so the stale highlight's rows are \
             repainted, not silently reused unchanged"
        );
    }

    /// The hover analogue of `selection_clear_alone_yields_bounded_rebuild`
    /// (Task 124.14b-ii): a previously-recorded hover
    /// (`cache.previous_command_block_hover_rows` is `Some`) disappears
    /// entirely this frame, with no row change and no selection change.
    /// `hover_changed` still fires, so this must still take
    /// `VertexRebuild::Bounded`.
    ///
    /// This is the only hover-only shape `call()`'s fixed geometry
    /// (`gutter_inset: 0.0`) can exercise at this layer: since
    /// `compute_command_block_hover_rows` always observes `None` under that
    /// geometry, only the clearing direction (`Some -> None`) is reachable
    /// here -- the appearing/moving direction (`current_hover_screen_rows`
    /// actually `Some`) is instead covered directly at the
    /// `build_bounded_damage` level in `widget.rs`'s
    /// `hover_only_change_damages_both_current_and_previous_rows`, which
    /// does not need a live geometry to exercise. Together the two tests
    /// cover both directions this subtask's mandate names.
    #[test]
    fn hover_clear_alone_yields_bounded_rebuild() {
        let snap = base_snapshot();
        let mut cache = settled_cache(&snap, true, true);
        // Must agree with `previous_command_block_hover_rows` -- see that
        // field's doc comment (Task 124.14b-ii).
        cache.previous_command_block_hover_rows = Some((0, 0));

        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, true, true);

        assert!(
            outcome.observations.hover_changed,
            "precondition: Some -> None must be observed as a hover change"
        );
        assert!(
            !outcome.observations.selection_changed,
            "precondition: no selection changed -- isolates the hover-only \
             case from 124.14b-i's selection-only case"
        );
        assert_eq!(
            outcome.changed_rows,
            ChangedRows::None,
            "precondition: no row changed -- isolates the clearing case"
        );
        assert_eq!(
            outcome.rebuild,
            VertexRebuild::Bounded,
            "124.14b-ii: a hover being cleared (Some -> None) must still \
             take the bounded-damage path so the stale tint's rows are \
             repainted, not silently reused unchanged"
        );
    }

    /// `ChangedRows::All` (no baseline recorded, or a length mismatch) must
    /// never select `VertexRebuild::Bounded` — `All` is not a row list at
    /// all, so there is nothing to bound the damage to, and `bounded_change`
    /// in `evaluate_frame_dirty_state` explicitly excludes it regardless of
    /// what else fired.
    #[test]
    fn changed_rows_all_does_not_yield_bounded_rebuild() {
        let snap = base_snapshot();
        let mut cache = settled_cache(&snap, true, true);
        // No recorded baseline: `diff_row_epochs` conservatively reports
        // every row changed (see `no_recorded_epochs_reports_every_row_changed`).
        cache.last_rendered_row_epochs = None;

        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);
        let outcome = call(&snap, &mut view_state, &cache, &render_state, true, true);

        assert_eq!(
            outcome.changed_rows,
            ChangedRows::All,
            "precondition: no baseline must report every row changed"
        );
        assert_eq!(
            outcome.rebuild,
            VertexRebuild::ReevaluateFullRebuild,
            "ChangedRows::All must never select VertexRebuild::Bounded -- \
             Bounded requires a nameable bound, and All is not one"
        );
    }

    /// 124.14b-i inverts this boundary from its 124.14a shape: a selection
    /// change alongside a genuine bounded row change now YIELDS
    /// `VertexRebuild::Bounded` rather than veto-ing it, because
    /// `build_bounded_damage` (`widget.rs`) now unions the selection's
    /// current/previous screen-row span into the damage alongside
    /// `changed_rows`, so the highlight is no longer left stale outside the
    /// bound. Renamed from
    /// `selection_change_alongside_a_row_change_still_yields_reevaluate_not_rows`
    /// -- inverted per this subtask's mandate, not merely adjusted, because
    /// 124.14b-i deliberately crosses the boundary that test pinned.
    #[test]
    fn selection_change_alongside_a_row_change_now_yields_bounded() {
        let snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);

        let mut changed = snap;
        bump_one_row_epoch(&mut changed);

        let mut view_state = ViewState::new();
        view_state.selection.anchor = Some(CellCoord { col: 0, row: 0 });
        view_state.selection.end = Some(CellCoord { col: 3, row: 0 });
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&changed, &mut view_state, &cache, &render_state, true, true);

        assert!(
            outcome.observations.selection_changed,
            "precondition: the selection must actually be observed as changed"
        );
        assert!(
            matches!(outcome.changed_rows, ChangedRows::Rows(_)),
            "precondition: the row change must be a genuine bounded one, \
             not All, or this test would not isolate the selection boundary"
        );
        assert_eq!(
            outcome.rebuild,
            VertexRebuild::Bounded,
            "124.14b-i: selection_changed alongside a genuine bounded row \
             change must now take VertexRebuild::Bounded -- \
             build_bounded_damage unions the selection's screen-row span \
             into the damage, so the highlight is no longer left stale"
        );
    }

    /// 124.14b-ii inverts this boundary from its 124.14a/124.14b-i shape: a
    /// hover change alongside a genuine bounded row change now YIELDS
    /// `VertexRebuild::Bounded` rather than veto-ing it, because
    /// `build_bounded_damage` (`widget.rs`) now unions the hover's
    /// current/previous screen-row span into the damage alongside
    /// `changed_rows`, so the tint is no longer left stale outside the
    /// bound. This crosses the boundary 124.14b-i's own recon pinned as
    /// "must NOT be folded in alongside selection" -- that recon's
    /// gutter-escape hazard (a hover-bounded frame would clip away the
    /// gutter's own repaint, per Task 124.18) was investigated further and
    /// disproved before 124.14b-ii was implemented: the hover tint is
    /// baked into the background instance buffer *inside* `terminal_rect`,
    /// the same surface selection paints to, not the gutter strip itself
    /// (see `PLAN_124_RENDER_EFFICIENCY.md`'s 124.14b recon correction).
    /// Renamed from
    /// `hover_change_alongside_a_row_change_still_yields_reevaluate_not_rows`
    /// -- inverted per this subtask's mandate, not merely adjusted, because
    /// 124.14b-ii deliberately crosses the boundary that test pinned.
    #[test]
    fn hover_change_alongside_a_row_change_now_yields_bounded() {
        let snap = base_snapshot();
        let mut cache = settled_cache(&snap, true, true);
        // `call()`'s fixed geometry (`gutter_inset: 0.0`) makes
        // `compute_command_block_hover_rows` always return `None`, so
        // recording a `Some` here as the previous frame's hover range is
        // what makes it differ from this frame's `None`. Both hover fields
        // are set together -- see their doc comments for why they must
        // never disagree about whether a previous hover was recorded.
        cache.previous_command_block_hover_rows = Some((0, 0));

        let mut changed = snap;
        bump_one_row_epoch(&mut changed);

        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);
        let outcome = call(&changed, &mut view_state, &cache, &render_state, true, true);

        assert!(
            outcome.observations.hover_changed,
            "precondition: the hover range must actually be observed as changed"
        );
        assert!(
            matches!(outcome.changed_rows, ChangedRows::Rows(_)),
            "precondition: the row change must be a genuine bounded one, \
             not All, or this test would not isolate the hover boundary"
        );
        assert_eq!(
            outcome.rebuild,
            VertexRebuild::Bounded,
            "124.14b-ii: hover_changed alongside a genuine bounded row \
             change must now take VertexRebuild::Bounded -- \
             build_bounded_damage unions the hover's screen-row span into \
             the damage, so the tint is no longer left stale"
        );
    }

    /// 124.14d inverts this boundary from its 124.14a/124.14b-i/124.14b-ii
    /// shape: a search-highlight change alongside a genuine bounded row
    /// change now YIELDS `VertexRebuild::Bounded` rather than veto-ing it,
    /// because `build_bounded_damage` (`widget.rs`) now unions the search
    /// overlay's current/previous highlight-row set into the damage
    /// alongside `changed_rows`, so search-match tinting is no longer left
    /// stale outside the bound. Renamed from
    /// `search_change_alongside_a_row_change_still_yields_reevaluate_not_rows`
    /// -- inverted per this subtask's mandate, not merely adjusted, because
    /// 124.14d deliberately crosses the boundary that test pinned.
    #[test]
    fn search_change_alongside_a_row_change_now_yields_bounded() {
        let snap = base_snapshot();
        let mut cache = settled_cache(&snap, true, true);
        cache.previous_search_epoch = cache.previous_search_epoch.wrapping_add(1);

        let mut changed = snap;
        bump_one_row_epoch(&mut changed);

        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);
        let outcome = call(&changed, &mut view_state, &cache, &render_state, true, true);

        assert!(
            outcome.observations.search_changed,
            "precondition: the search epoch must actually be observed as changed"
        );
        assert!(
            matches!(outcome.changed_rows, ChangedRows::Rows(_)),
            "precondition: the row change must be a genuine bounded one, \
             not All, or this test would not isolate the search boundary"
        );
        assert_eq!(
            outcome.rebuild,
            VertexRebuild::Bounded,
            "124.14d: search_changed alongside a genuine bounded row change \
             must now take VertexRebuild::Bounded -- build_bounded_damage \
             unions the search overlay's highlight-row set into the \
             damage, so the tint is no longer left stale"
        );
    }

    /// 124.C5's boundary, image-pixels half: a store-level pixel mutation
    /// alongside a genuine bounded row change must still veto
    /// `VertexRebuild::Bounded`.
    ///
    /// 124.C5 made row epochs sound for image *placement* damage (placing
    /// an image over cells now bumps their row's epoch), but it did
    /// **not** make them sound for image *pixel* damage: an animation
    /// frame advancing, or a Kitty `a=c` compose overwriting a frame's
    /// pixels in place, changes no cell, stamps no new placement, and
    /// bumps no epoch. The image quad's geometry is unchanged; only its
    /// texture contents differ. So `changed_rows` can name a completely
    /// unrelated row while the image itself is invisible to it -- images
    /// must not ride on `changed_rows` until their own bound exists, which
    /// this subtask does not build.
    #[test]
    fn image_pixels_change_alongside_a_row_change_still_yields_reevaluate_not_rows() {
        let mut snap = base_snapshot();
        let image = InlineImage {
            id: 1,
            pixels: Arc::new(vec![1u8; 4]),
            width_px: 1,
            height_px: 1,
            display_cols: 1,
            display_rows: 1,
            size_mode: ImageSizeMode::NativePixels,
            frames: Vec::new(),
            root_gap_ms: 0,
            animation: AnimationControl::default(),
        };
        let mut images = std::collections::HashMap::new();
        images.insert(1, image);
        snap.images = Arc::new(images);
        let cache = settled_cache(&snap, true, true);

        let mut changed = snap;
        bump_one_row_epoch(&mut changed);

        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);
        let outcome = call(&changed, &mut view_state, &cache, &render_state, true, true);

        assert!(
            outcome.observations.image_pixels_changed,
            "precondition: the pixel-mutation detector must actually fire"
        );
        assert!(
            matches!(outcome.changed_rows, ChangedRows::Rows(_)),
            "precondition: the row change must be a genuine bounded one, \
             not All, or this test would not isolate the image boundary"
        );
        assert_eq!(
            outcome.rebuild,
            VertexRebuild::ReevaluateFullRebuild,
            "124.C5's boundary: image_pixels_changed must veto \
             VertexRebuild::Bounded even when the row change is otherwise \
             bounded -- pixel-only image damage bumps no row epoch"
        );
    }

    /// 124.C5's boundary, image-frame half: an animated image's frame
    /// advancing alongside a genuine bounded row change must still veto
    /// `VertexRebuild::Bounded`, for the same reason as the pixels test
    /// above -- advancing a frame changes no cell and bumps no row epoch,
    /// so `changed_rows` cannot be trusted to cover it.
    #[test]
    fn image_frame_change_alongside_a_row_change_still_yields_reevaluate_not_rows() {
        let mut snap = base_snapshot();
        let frame2_pixels = Arc::new(vec![2u8; 4]);
        let frame3_pixels = Arc::new(vec![3u8; 4]);
        let image = InlineImage {
            id: 1,
            pixels: Arc::new(vec![1u8; 4]),
            width_px: 1,
            height_px: 1,
            display_cols: 1,
            display_rows: 1,
            size_mode: ImageSizeMode::NativePixels,
            frames: vec![
                ImageFrame {
                    pixels: Arc::clone(&frame2_pixels),
                    gap_ms: 40,
                },
                ImageFrame {
                    pixels: Arc::clone(&frame3_pixels),
                    gap_ms: 40,
                },
            ],
            root_gap_ms: 40,
            animation: AnimationControl {
                run_mode: AnimationRunMode::Running,
                loop_count: 1,
                current_frame: 0,
            },
        };
        let mut images = std::collections::HashMap::new();
        images.insert(1, image);
        snap.images = Arc::new(images);

        let mut cache = settled_cache(&snap, true, true);
        cache
            .last_rendered_image_pixel_ptrs
            .insert(1, Arc::as_ptr(&frame3_pixels).addr());

        let mut changed = snap;
        bump_one_row_epoch(&mut changed);

        let mut view_state = ViewState::new();
        view_state.seed_anim_clock_for_test(1, 1, Duration::from_millis(100), 0);
        let render_state = render_state_with_deco_verts(true);
        let outcome = call(&changed, &mut view_state, &cache, &render_state, true, true);

        assert!(
            outcome.observations.image_frame_changed,
            "precondition: the animation tick must actually advance a frame"
        );
        assert!(
            matches!(outcome.changed_rows, ChangedRows::Rows(_)),
            "precondition: the row change must be a genuine bounded one, \
             not All, or this test would not isolate the image boundary"
        );
        assert_eq!(
            outcome.rebuild,
            VertexRebuild::ReevaluateFullRebuild,
            "124.C5's boundary: image_frame_changed must veto \
             VertexRebuild::Bounded even when the row change is otherwise \
             bounded -- frame-advance image damage bumps no row epoch"
        );
    }

    #[test]
    fn content_change_beats_cursor_change() {
        // A theme change AND a cursor-blink change happen on the same
        // frame: `content_changed` must veto the fast path even though the
        // cursor state also changed.
        let snap = base_snapshot();
        let mut cache = settled_cache(&snap, true, true);
        cache.previous_theme = Some(&freminal_common::themes::DRACULA);
        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, false, true);

        assert_eq!(outcome.rebuild, VertexRebuild::ReevaluateFullRebuild);
        assert!(outcome.observations.content_changed);
    }

    #[test]
    fn dims_change_beats_cursor_change() {
        // A terminal resize AND a cursor-blink change happen on the same
        // frame: the stale-vertex-slivers hazard means `content_changed`
        // (via `dims_changed`) must veto the fast path.
        let snap = base_snapshot();
        let mut cache = settled_cache(&snap, true, true);
        cache.previous_term_width = snap.term_width + 1;
        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, false, true);

        assert_eq!(outcome.rebuild, VertexRebuild::ReevaluateFullRebuild);
        assert!(outcome.observations.content_changed);
    }

    // ── content-changed selection auto-clear (issue #470) ────────────────

    /// Put a committed (not in-progress) selection on `view_state`.
    fn with_committed_selection(view_state: &mut ViewState) {
        view_state.selection.anchor = Some(CellCoord { col: 2, row: 1 });
        view_state.selection.end = Some(CellCoord { col: 8, row: 3 });
        view_state.selection.is_selecting = false;
        view_state.selection_committed_this_frame = false;
    }

    /// Bump exactly one entry of `snap.row_epochs`, simulating "this row's
    /// rendered content differs from whatever baseline the caller already
    /// captured" without touching `visible_chars` at all. Callers capture
    /// the baseline (typically via `settled_cache`) *before* calling this,
    /// so the bump is visible only to the epoch comparison, not to
    /// whichever `last_rendered_visible` the cache already recorded.
    fn bump_one_row_epoch(snap: &mut TerminalSnapshot) {
        let mut epochs = (*snap.row_epochs).to_vec();
        epochs[0] += 1;
        snap.row_epochs = Arc::from(epochs);
    }

    /// The #470 regression, re-expressed against the epoch-based pre-filter
    /// (Task 124.12) that replaced the old sticky `content_changed` bool as
    /// the auto-clear's trigger. `rows_changed` is level-triggered against
    /// the last *rendered* frame, so — like the bool it replaced — a row
    /// epoch that differs does not by itself prove the *rendered* text
    /// moved: a change that reverts before the next rendered frame still
    /// shows up as "changed" here. That must not discard a selection; the
    /// confirmation comparison against `last_rendered_visible` (unchanged
    /// by this subtask, see item 4 of its scope) is what actually decides.
    #[test]
    fn spurious_row_epoch_change_does_not_discard_a_selection() {
        let mut snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);
        // Bump the epoch *after* `settled_cache` captured the baseline, so
        // the pre-filter fires even though `last_rendered_visible` still
        // equals `snap.visible_chars` — the text has demonstrably not
        // moved since.
        bump_one_row_epoch(&mut snap);
        snap.scroll_changed = false;
        let mut view_state = ViewState::new();
        with_committed_selection(&mut view_state);
        let render_state = render_state_with_deco_verts(true);

        let _ = call(&snap, &mut view_state, &cache, &render_state, true, true);

        assert!(
            view_state.selection.has_selection(),
            "a row-epoch change contradicted by identical rendered text must \
             not clear the selection"
        );
    }

    /// The behaviour being preserved: when the text really did move, a stale
    /// highlight would sit over different content, so it is still discarded.
    #[test]
    fn genuine_content_change_still_discards_a_selection() {
        let mut snap = base_snapshot();
        let mut cache = settled_cache(&snap, true, true);
        bump_one_row_epoch(&mut snap);
        snap.scroll_changed = false;
        // Last-rendered text differs from the snapshot's.
        cache.last_rendered_visible = Some(Arc::new(vec![
            freminal_common::buffer_states::tchar::TChar::Ascii(b'z'),
        ]));
        let mut view_state = ViewState::new();
        with_committed_selection(&mut view_state);
        let render_state = render_state_with_deco_verts(true);

        let _ = call(&snap, &mut view_state, &cache, &render_state, true, true);

        assert!(
            !view_state.selection.has_selection(),
            "a real content change must still clear the selection"
        );
    }

    /// A pure scroll never invalidates a selection: coordinates are
    /// buffer-absolute, so the same text is still selected.
    #[test]
    fn scroll_change_does_not_discard_a_selection() {
        let mut snap = base_snapshot();
        let mut cache = settled_cache(&snap, true, true);
        bump_one_row_epoch(&mut snap);
        snap.scroll_changed = true;
        cache.last_rendered_visible = Some(Arc::new(vec![
            freminal_common::buffer_states::tchar::TChar::Ascii(b'z'),
        ]));
        let mut view_state = ViewState::new();
        with_committed_selection(&mut view_state);
        let render_state = render_state_with_deco_verts(true);

        let _ = call(&snap, &mut view_state, &cache, &render_state, true, true);

        assert!(view_state.selection.has_selection());
    }

    /// The Task 116 guarantee: output landing on the same frame as the mouse
    /// release must not wipe the just-committed selection.
    #[test]
    fn selection_committed_this_frame_survives_a_genuine_content_change() {
        let mut snap = base_snapshot();
        let mut cache = settled_cache(&snap, true, true);
        bump_one_row_epoch(&mut snap);
        snap.scroll_changed = false;
        cache.last_rendered_visible = Some(Arc::new(vec![
            freminal_common::buffer_states::tchar::TChar::Ascii(b'z'),
        ]));
        let mut view_state = ViewState::new();
        with_committed_selection(&mut view_state);
        view_state.selection_committed_this_frame = true;
        let render_state = render_state_with_deco_verts(true);

        let _ = call(&snap, &mut view_state, &cache, &render_state, true, true);

        assert!(view_state.selection.has_selection());
    }

    #[test]
    fn search_change_beats_cursor_change() {
        // The search match count changed AND the cursor blinked: the tint
        // is baked into vertices the fast path never touches, so
        // `search_changed` must veto `CursorOnly` -- but, since 124.14d,
        // it now yields `VertexRebuild::Bounded` rather than
        // `ReevaluateFullRebuild`, because the search overlay's own
        // highlight-row union bounds the damage.
        let snap = base_snapshot();
        let mut cache = settled_cache(&snap, true, true);
        // Any value that differs from this frame's epoch stands in for "the
        // search state changed since the last rendered frame".
        cache.previous_search_epoch = cache.previous_search_epoch.wrapping_add(1);
        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, false, true);

        assert_eq!(outcome.rebuild, VertexRebuild::Bounded);
        assert!(outcome.observations.search_changed);
    }

    /// A selection was made AND the cursor blinked on the same frame. The
    /// highlight is baked into vertices the fast path never touches, so
    /// `selection_changed` must veto `CursorOnly` -- that half of this
    /// test's original purpose is unchanged. What DID change (124.14b-i):
    /// `selection_changed` alone no longer forces the *unbounded*
    /// `ReevaluateFullRebuild` -- it now takes `VertexRebuild::Bounded`,
    /// since `build_bounded_damage` can name the selection's screen-row
    /// span. Updated, not renamed: this test's real subject was always
    /// "selection vetoes the cursor-only fast path", and that is still
    /// exactly what it pins -- only the *value* selection now routes to
    /// changed from `ReevaluateFullRebuild` to `Bounded`.
    #[test]
    fn selection_change_beats_cursor_change() {
        let snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);
        let mut view_state = ViewState::new();
        view_state.selection.anchor = Some(CellCoord { col: 0, row: 0 });
        view_state.selection.end = Some(CellCoord { col: 3, row: 0 });
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, false, true);

        assert_eq!(
            outcome.rebuild,
            VertexRebuild::Bounded,
            "selection vetoes the CursorOnly fast path (unchanged), and \
             takes the bounded-damage path (124.14b-i) rather than the \
             unbounded ReevaluateFullRebuild it used to fall back to"
        );
        assert!(outcome.observations.selection_changed);
    }

    #[test]
    fn text_blink_change_beats_cursor_change() {
        // Text-blink visibility flipped AND the cursor blinked on the same
        // frame: a blink-phase change re-includes/excludes glyphs in the
        // foreground vertex buffer, so `text_blink_changed` must veto the
        // fast path even though it is a separate trigger from cursor-only.
        let mut snap = base_snapshot();
        snap.has_blinking_text = true;
        let cache = settled_cache(&snap, true, true);
        let mut view_state = ViewState::new();
        view_state.text_blink_slow_visible = false;
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, false, true);

        assert_eq!(outcome.rebuild, VertexRebuild::ReevaluateFullRebuild);
        assert!(outcome.observations.text_blink_changed);
    }

    #[test]
    fn image_pixels_change_beats_cursor_change() {
        // A still image is visible whose pixel pointer was never recorded
        // in the cache (as if a store-level pixel mutation replaced it
        // without touching any cell or `run_mode`), AND the cursor
        // blinked: the pixel-mutation detector must veto the fast path
        // even though `image_frame_changed` stays false (the image is not
        // animated).
        let mut snap = base_snapshot();
        let image = InlineImage {
            id: 1,
            pixels: Arc::new(vec![1u8; 4]),
            width_px: 1,
            height_px: 1,
            display_cols: 1,
            display_rows: 1,
            size_mode: ImageSizeMode::NativePixels,
            frames: Vec::new(),
            root_gap_ms: 0,
            animation: AnimationControl::default(),
        };
        let mut images = std::collections::HashMap::new();
        images.insert(1, image);
        snap.images = Arc::new(images);
        let cache = settled_cache(&snap, true, true);
        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, false, true);

        assert_eq!(outcome.rebuild, VertexRebuild::ReevaluateFullRebuild);
        assert!(outcome.observations.image_pixels_changed);
        assert!(!outcome.observations.image_frame_changed);
    }

    #[test]
    fn image_frame_change_beats_cursor_change() {
        // An animated image (3 frames, 40ms gap) ticks from frame 1 to
        // frame 3 in 100ms (mirroring `tick_running_advances_by_wall_clock`
        // in `view_state.rs`) AND the cursor blinked on the same frame.
        // `last_rendered_image_pixel_ptrs` is pre-seeded with frame 3's
        // pixel pointer so the pixel-mutation detector agrees with what
        // the tick lands on and only `image_frame_changed` fires.
        let mut snap = base_snapshot();
        let frame2_pixels = Arc::new(vec![2u8; 4]);
        let frame3_pixels = Arc::new(vec![3u8; 4]);
        let image = InlineImage {
            id: 1,
            pixels: Arc::new(vec![1u8; 4]),
            width_px: 1,
            height_px: 1,
            display_cols: 1,
            display_rows: 1,
            size_mode: ImageSizeMode::NativePixels,
            frames: vec![
                ImageFrame {
                    pixels: Arc::clone(&frame2_pixels),
                    gap_ms: 40,
                },
                ImageFrame {
                    pixels: Arc::clone(&frame3_pixels),
                    gap_ms: 40,
                },
            ],
            root_gap_ms: 40,
            animation: AnimationControl {
                run_mode: AnimationRunMode::Running,
                loop_count: 1,
                current_frame: 0,
            },
        };
        let mut images = std::collections::HashMap::new();
        images.insert(1, image);
        snap.images = Arc::new(images);

        let mut cache = settled_cache(&snap, true, true);
        cache
            .last_rendered_image_pixel_ptrs
            .insert(1, Arc::as_ptr(&frame3_pixels).addr());

        let mut view_state = ViewState::new();
        view_state.seed_anim_clock_for_test(1, 1, Duration::from_millis(100), 0);
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, false, true);

        assert_eq!(outcome.rebuild, VertexRebuild::ReevaluateFullRebuild);
        assert!(outcome.observations.image_frame_changed);
        assert!(!outcome.observations.image_pixels_changed);
    }
}
