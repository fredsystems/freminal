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
/// This is a two-way switch on *what check runs next*, not a description of
/// what ultimately happens: `CursorOnly` fully determines the frame (patch
/// the cursor quad, nothing else). `ReevaluateFullRebuild` means the cheap
/// fast path does not apply — the caller must independently check
/// [`FrameDirtyObservations`] (and whether the decoration buffer is empty)
/// to decide between a full vertex rebuild and reusing the previous frame's
/// GPU buffers completely unchanged. Preserving this shape (rather than
/// collapsing to a `CursorOnly`/`Full` binary) matters: not every
/// non-cursor-only frame triggers a full rebuild — the common steady-state
/// frame (nothing changed at all) takes neither branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VertexRebuild {
    /// Patch just the cursor quad in the existing decoration buffer
    /// ([`build_cursor_verts_only`]) — no re-shaping, no full rebuild.
    CursorOnly,
    /// Not the cursor-only fast path; the full-rebuild trigger flags must
    /// still be checked by the caller.
    ReevaluateFullRebuild,
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
    /// The normalised selection for this frame (buffer-absolute rows),
    /// after the content-change auto-clear rule has been applied.
    pub(super) current_selection: Option<(CellCoord, CellCoord)>,
    /// The selection translated into snapshot-row space, clamped to the
    /// flattened window, for the renderer.
    pub(super) screen_selection: Option<(usize, usize, usize, usize)>,
    /// Fingerprint of this frame's search-highlight state (cached for next
    /// frame's comparison). See `SearchState::render_epoch`.
    pub(super) search_epoch: u64,
    /// Command-block gutter hover-tint rendered-row range this frame.
    pub(super) command_block_hover_rows: Option<(usize, usize)>,
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

    // Detect content changes via `Arc::ptr_eq` — this is immune to the
    // race where the PTY thread overwrites a "changed" snapshot with a
    // "clean" one before the GUI wakes up.  If the `visible_chars` arc
    // is a different allocation from the one we last rendered, the
    // content has changed regardless of the `content_changed` flag.
    //
    // We deliberately do NOT OR in the snapshot's `content_changed`
    // flag here (issue #439 fix #4). That flag is baked into the
    // published snapshot at build time, so when the GUI re-reads the
    // SAME `Arc` on the ~14 frames between real PTY updates it reads a
    // stale `true` and forces a full vertex rebuild every frame — a
    // ~60fps rebuild for a screen changing a few times/sec. The
    // `Arc::ptr_eq` check below already detects every genuine change:
    // whenever real content changes, `flatten_visible` allocates a NEW
    // `visible_chars` `Arc` (so ptr_eq fails and we rebuild), and a
    // cursor-blink re-flatten that produces a byte-identical-but-new
    // `Arc` also fails ptr_eq and rebuilds. Re-observing the same `Arc`
    // correctly reports "unchanged". So the raw flag is redundant here
    // and, worse, sticky — dropping it is what lets an idle screen fall
    // through to the cheap cursor-only / no-op path.
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
    let content_changed = theme_changed
        || dims_changed
        || folds_changed
        || cache
            .last_rendered_visible
            .as_ref()
            .is_none_or(|prev| !Arc::ptr_eq(prev, &snap.visible_chars))
        || cache
            .last_rendered_line_widths
            .as_ref()
            .is_none_or(|prev| !Arc::ptr_eq(prev, &snap.visible_line_widths));

    // Clear the selection when actual terminal text content changes so
    // stale highlights don't linger over shifted text.  We use
    // `snap.content_changed` here (NOT the `Arc::ptr_eq`-augmented
    // `content_changed`) because the PTY thread may re-flatten and
    // allocate a new Arc for cursor-blink dirty rows even when the
    // visible text is byte-identical.  Using the broader check would
    // clear the selection within ~500 ms of mouse release (on every
    // cursor blink), making copy impossible.
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
    // `snap.content_changed` and immediately wipe the
    // just-committed selection (defect 2, Task 116.2).
    // `snap.content_changed` is edge-triggered per *snapshot build*, but the
    // GUI renders only a subset of the snapshots the PTY thread produces. A
    // change that reverts before the next rendered frame -- a prompt clearing
    // and rewriting its own line, say -- therefore arrives as
    // `content_changed = true` on a snapshot whose text is identical to what
    // this pane already drew:
    //
    //     snapshot A: text X                  -> rendered
    //     snapshot B: text Y, changed = true  -> never rendered
    //     snapshot C: text X, changed = true  -> rendered (Y != X at build C)
    //
    // Acting on that wiped selections for no reason, intermittently, whenever
    // a mouse release happened to land across such a flicker (#470).
    //
    // So confirm against what was actually last rendered before discarding
    // anything. This is a content comparison, deliberately not the
    // `Arc::ptr_eq` check used for `content_changed` above: the PTY thread
    // allocates a fresh Arc for cursor-blink dirty rows even when the text is
    // byte-identical, so pointer identity would re-introduce the ~500ms
    // clear-on-blink bug that `snap.content_changed` was chosen to avoid.
    //
    // Evaluated last so the O(visible_chars) comparison only runs on the rare
    // frame where every cheap condition already passed.
    if snap.content_changed
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

    let cursor_only = !content_changed
        && !selection_changed
        && !text_blink_changed
        && !search_changed
        && !hover_changed
        && !image_frame_changed
        && !image_pixels_changed
        && cursor_state_changed
        && !render_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .deco_verts
            .is_empty();

    DirtyTrackingOutcome {
        rebuild: if cursor_only {
            VertexRebuild::CursorOnly
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
        current_selection,
        screen_selection,
        search_epoch,
        command_block_hover_rows: command_block_hover_rows_early,
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
    fn base_snapshot() -> TerminalSnapshot {
        let mut snap = TerminalSnapshot::empty();
        snap.term_width = 10;
        snap.term_height = 5;
        snap.total_rows = 5;
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
        cache.last_rendered_line_widths = Some(Arc::clone(&snap.visible_line_widths));
        cache.previous_selection = None;
        // These tests drive `ViewState::new()`, whose search state is
        // default-constructed, so a settled cache is one that already agrees
        // with that state's fingerprint.
        cache.previous_search_epoch = SearchState::default().render_epoch();
        cache.previous_command_block_hover_rows = None;
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

    /// The #470 regression. `content_changed` is edge-triggered per snapshot
    /// build and the GUI renders only some snapshots, so a change that reverts
    /// between rendered frames arrives as `content_changed = true` on a
    /// snapshot identical to what was already drawn. That must not discard a
    /// selection.
    #[test]
    fn spurious_content_changed_does_not_discard_a_selection() {
        let mut snap = base_snapshot();
        snap.content_changed = true;
        snap.scroll_changed = false;
        let cache = settled_cache(&snap, true, true);
        // `settled_cache` records this exact buffer as last-rendered, so the
        // text has demonstrably not moved since.
        let mut view_state = ViewState::new();
        with_committed_selection(&mut view_state);
        let render_state = render_state_with_deco_verts(true);

        let _ = call(&snap, &mut view_state, &cache, &render_state, true, true);

        assert!(
            view_state.selection.has_selection(),
            "a content_changed flag contradicted by the rendered text must not \
             clear the selection"
        );
    }

    /// The behaviour being preserved: when the text really did move, a stale
    /// highlight would sit over different content, so it is still discarded.
    #[test]
    fn genuine_content_change_still_discards_a_selection() {
        let mut snap = base_snapshot();
        snap.content_changed = true;
        snap.scroll_changed = false;
        let mut cache = settled_cache(&snap, true, true);
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
        snap.content_changed = true;
        snap.scroll_changed = true;
        let mut cache = settled_cache(&snap, true, true);
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
        snap.content_changed = true;
        snap.scroll_changed = false;
        let mut cache = settled_cache(&snap, true, true);
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
        // `search_changed` must veto it.
        let snap = base_snapshot();
        let mut cache = settled_cache(&snap, true, true);
        // Any value that differs from this frame's epoch stands in for "the
        // search state changed since the last rendered frame".
        cache.previous_search_epoch = cache.previous_search_epoch.wrapping_add(1);
        let mut view_state = ViewState::new();
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, false, true);

        assert_eq!(outcome.rebuild, VertexRebuild::ReevaluateFullRebuild);
        assert!(outcome.observations.search_changed);
    }

    #[test]
    fn selection_change_beats_cursor_change() {
        // A selection was made AND the cursor blinked on the same frame:
        // the highlight is baked into vertices the fast path never
        // touches, so `selection_changed` must veto it.
        let snap = base_snapshot();
        let cache = settled_cache(&snap, true, true);
        let mut view_state = ViewState::new();
        view_state.selection.anchor = Some(CellCoord { col: 0, row: 0 });
        view_state.selection.end = Some(CellCoord { col: 3, row: 0 });
        let render_state = render_state_with_deco_verts(true);

        let outcome = call(&snap, &mut view_state, &cache, &render_state, false, true);

        assert_eq!(outcome.rebuild, VertexRebuild::ReevaluateFullRebuild);
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
