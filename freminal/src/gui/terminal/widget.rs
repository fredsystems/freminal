// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The `FreminalTerminalWidget` egui widget and GPU render state.

use crate::gui::{
    folding::{RenderedRow, RowMap, compute_fold_ranges},
    fonts::{FontConfig, setup_font_files},
    icons::ChromeIcon,
    mouse::PreviousMouseState,
    published_frame_state::PanePointerReportInputs,
    shaping::ShapedLine,
    view_state::{LogicalCell, ViewState},
};

use crossbeam_channel::{Receiver, Sender};
use freminal_common::{
    buffer_states::{
        command_block::CommandStatus, pointer_shape::PointerShape, progress::ProgressState,
        row_number::RowNumber, tchar::TChar, url::Url,
    },
    config::{Config, UnfocusedCursorStyle},
    send_or_log,
    themes::ThemePalette,
};
use freminal_terminal_emulator::{
    InlineImage, LineWidth,
    io::{InputEvent, SearchCorpus},
    snapshot::TerminalSnapshot,
};

use egui::{self, Color32, Context, CursorIcon, Key, Pos2, Rect, Ui};

use super::{
    super::{
        atlas::GlyphAtlas,
        colors::cursor_f,
        font_manager::FontManager,
        frame_drain::WindowFocus,
        renderer::{
            BackgroundFrame, CursorBlinkPhase, CursorDrawParams, CursorVertRange, FgRenderOptions,
            GlRetireQueue, ImageDrawEntry, MatchHighlight, TerminalRenderer, WindowPostRenderer,
            build_background_instances, build_cursor_verts_only, build_foreground_instances,
            build_image_verts, gl_facade::Gl,
        },
        search::{
            SearchBarAction, matches_to_highlights, run_search, scroll_to_match_and_send,
            show_search_bar,
        },
    },
    coords::{encode_egui_mouse_pos_as_usize, flat_index_for_cell, running_block_extent},
    cursor_appearance::{CursorAppearance, CursorFocus, CursorVisibility, EchoState},
    cursor_blink::{ActivationBlinkReset, CursorStateInputs, frame_cursor_state},
    frame_dirty::{
        CursorFrame, CursorFrameInputs, DrawnCursor, FrameDirtyContext, FrameDirtyGeometry,
        VertexRebuild, evaluate_frame_dirty_state,
    },
    input::{
        InputCarryState, PaneFocus, WriteInputParams, scroll_overlay_passthrough,
        write_input_to_terminal,
    },
};

use conv2::{ApproxFrom, ConvUtil};
use egui_glow::CallbackFn;
use std::sync::{Arc, Mutex};

// Task 125.5: live render-work profiling is entirely absent from a default
// build -- no field, no branch -- see `RenderState::live_profile` and its
// call sites below, all gated on the same feature.
#[cfg(feature = "frame-profiling")]
use super::super::renderer::profiling::{
    LiveRenderProfile, PaneFrameToken, RawRebuildDecision, ReevaluatedRebuild, UploadByteCounts,
    resolve_render_work_class,
};
// Task 125.8: asynchronous terminal GPU upload/draw timing is entirely
// absent from a default build -- see `gui::renderer::gpu_profiling`'s doc.
#[cfg(feature = "gpu-profiling")]
use super::super::renderer::gpu_profiling::LOG_TARGET as GPU_TIMING_LOG_TARGET;
use std::time::Duration;
use tracing::error;

// ─── Fold-placeholder helpers (Task 72.10b-3) ────────────────────────────

/// Format the placeholder text shown on a collapsed fold-row.
///
/// Examples (assuming `width_cols` is generous):
///
/// - `format_placeholder_text(1, 80)` → `"▶ 1 line hidden — click to unfold"`
/// - `format_placeholder_text(7, 80)` → `"▶ 7 lines hidden — click to unfold"`
///
/// When `width_cols` cannot fit the full string, the result is truncated
/// to `width_cols.saturating_sub(1)` characters and an ellipsis (`…`) is
/// appended.  When `width_cols` is too small to fit even the minimal
/// `"▶ N lines…"` form, the helper falls back to `"▶…"` (or `""` if the
/// width is zero).
/// Patch the cursor's geometry into `deco_verts` for a cursor-only frame
/// (content, selection, and everything else unchanged; only the cursor's
/// appearance, blink state, position, or color changed since the last frame).
///
/// `range` is the [`CursorVertRange`] recorded by the most recent full rebuild
/// (or the previous cursor-only patch): the exact float range the cursor
/// occupies in `deco_verts`. [`build_background_instances`] always emits the
/// cursor as the **final** region of the decoration buffer, so everything
/// before `range.start` is non-cursor decoration (underlines, selection,
/// search and hover quads) that this function must never touch.
///
/// `cursor_verts` is the freshly-built cursor geometry for *this* frame from
/// [`build_cursor_verts_only`]. Its length is not fixed: empty when nothing
/// should be drawn (hidden, or blink-off), `CURSOR_QUAD_FLOATS` for a solid
/// cursor, or four times that for a hollow one.
///
/// The patch truncates `deco_verts` to `range.start` and appends
/// `cursor_verts`. Hiding the cursor is therefore just a truncate, and a
/// cursor whose length changed between frames (solid -> hollow, hidden ->
/// visible) needs no special case. Making the buffer's length vary is safe
/// because the GPU upload path always uploads `deco_verts` at its current
/// length and draws that many vertices rather than assuming a fixed-size
/// reserved tail.
///
/// Returns the new [`CursorVertRange`], which the caller must store: it is
/// what the next patch truncates to. Deriving it from anything but the
/// geometry actually written is the issue #432 corruption class (a stale
/// offset overwrites the bottom-row selection quad).
///
/// `range` is `Option` because the render state stores it that way
/// (`RenderState::cursor_vert_range`: `None` until the first rebuild). A
/// cursor-only frame is never selected for a never-built render state
/// (`evaluate_frame_dirty_state` vetoes it), so `None` cannot reach here; it
/// is handled defensively by appending after whatever is present. The
/// caller stores the returned range as `Some`.
///
/// A `range.start` beyond the end of `deco_verts` cannot be produced by the
/// bookkeeping; it is clamped to the buffer's length rather than panicking,
/// so the cursor is appended after whatever is present.
#[must_use]
fn patch_cursor_only_deco_verts(
    deco_verts: &mut Vec<f32>,
    range: Option<CursorVertRange>,
    cursor_verts: &[f32],
) -> CursorVertRange {
    let start = range.map_or(deco_verts.len(), |r| r.start.min(deco_verts.len()));
    deco_verts.truncate(start);
    deco_verts.extend_from_slice(cursor_verts);
    CursorVertRange {
        start,
        len: cursor_verts.len(),
    }
}

/// Scissor a GPU draw to the windowing-published present region, run
/// `draw`, then restore GL scissor state to what egui expects (disabled)
/// before returning (124.23).
///
/// Both of the paint callback's arms — cursor-only and full-draw — call
/// this with the *same* `region` read, rather than each reading (or
/// ignoring) `PresentRegion` independently. See the call sites in `show`'s
/// paint callback for why the region scissored to is the windowing-published
/// one, not either arm's own damage rect: on a stale back buffer the
/// published region can be a union covering more than either arm's own
/// declared damage, and scissoring to a narrower rect would silently skip
/// repainting pixels the union says still need it.
///
/// [`freminal_windowing::PresentRegion::Full`] means the windowing layer
/// could not prove a smaller region was safe (or the surface doesn't
/// support partial present at all) — the whole grid must be redrawn, so
/// `draw` runs with no scissor applied.
fn draw_scissored_to_present_region<R>(
    gl: &Gl<'_>,
    region: freminal_windowing::PresentRegion,
    draw: impl FnOnce() -> R,
) -> R {
    let applied_scissor = match region {
        freminal_windowing::PresentRegion::Region(d) => {
            unsafe {
                gl.enable(glow::SCISSOR_TEST);
                gl.scissor(d.x, d.y, d.width, d.height);
            }
            true
        }
        freminal_windowing::PresentRegion::Full => false,
    };
    let result = draw();
    if applied_scissor {
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
        }
    }
    result
}

/// Format the fold-placeholder text for a collapsed command block.
///
/// See the render path (`show`) for how this is used.
#[must_use]
pub fn format_placeholder_text(hidden_rows: usize, width_cols: usize) -> String {
    let suffix = if hidden_rows == 1 { "line" } else { "lines" };
    let full = format!("▶ {hidden_rows} {suffix} hidden — click to unfold");

    if width_cols == 0 {
        return String::new();
    }

    // Count *characters* (not bytes) to compare against terminal columns.
    // This is a rough match: wide chars actually take 2 cols, but the
    // placeholder string is overwhelmingly ASCII so the over-approximation
    // is acceptable for truncation purposes.
    if full.chars().count() <= width_cols {
        return full;
    }

    if width_cols < 2 {
        return "▶".to_string();
    }

    // Take `width_cols - 1` chars, then append the ellipsis.
    let kept: String = full.chars().take(width_cols.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Hit-test a pointer position against a list of fold-placeholder rects.
///
/// Returns the `CommandBlockId` of the first rect that contains `pos`, or
/// `None` if the pointer is not over any placeholder.  Rects are checked
/// in insertion order; placeholder rows do not overlap by construction
/// (each occupies one rendered row), so order does not matter for
/// correctness — but it is well-defined for testability.
#[must_use]
pub fn hit_test_placeholder(
    rects: &[(
        Rect,
        freminal_common::buffer_states::command_block::CommandBlockId,
    )],
    pos: Pos2,
) -> Option<freminal_common::buffer_states::command_block::CommandBlockId> {
    rects
        .iter()
        .find(|(rect, _)| rect.contains(pos))
        .map(|(_, id)| *id)
}

/// Fold-aware window layout for one frame.
///
/// Centralises the coordinate math shared by the renderer and every overlay
/// that maps between buffer rows, snapshot rows, rendered rows, and on-screen
/// rows. Computed once from a snapshot plus the GUI-local folded-block set so
/// the renderer, gutter, duration labels, hover, and hit-tests all agree.
///
/// Coordinate spaces:
/// - **buffer row**: absolute index into the scrollback buffer.
/// - **snapshot row** `[0, snap_rows)`: index into `visible_chars` etc. The
///   window covers `term_height + window_extra_rows` rows starting at
///   `flat_window_start`.
/// - **rendered row** `[0, rendered_row_count)`: snapshot rows with folded
///   ranges collapsed to placeholders.
/// - **screen row** `[0, term_height)`: rendered rows with the top
///   `render_skip` rows scrolled off (bottom-anchored so the live bottom is
///   pinned).
pub(super) struct FoldLayout {
    /// Buffer-absolute index of the first snapshot row.
    pub(super) flat_window_start: usize,
    /// Snapshot → rendered row mapping with folds collapsed.
    pub(super) row_map: RowMap,
    /// Rendered rows scrolled off the top so the bottom `term_height` rendered
    /// rows fill the screen.
    render_skip: usize,
}

impl FoldLayout {
    /// Build the layout for `snap` given the folded-block set.
    pub(super) fn new(
        snap: &TerminalSnapshot,
        folded_blocks: &std::collections::HashSet<
            freminal_common::buffer_states::command_block::CommandBlockId,
        >,
    ) -> Self {
        let raw_fold_ranges =
            compute_fold_ranges(&snap.command_blocks, snap.row_base, folded_blocks);
        let flat_window_start =
            super::coords::visible_window_start(snap).saturating_sub(snap.window_extra_rows);
        let snap_rows = snap.term_height.saturating_add(snap.window_extra_rows);
        let fold_ranges =
            crate::gui::folding::translate_ranges_to_snapshot(&raw_fold_ranges, flat_window_start);
        let row_map = RowMap::new(snap_rows, &fold_ranges);
        let render_skip = row_map
            .rendered_row_count()
            .saturating_sub(snap.term_height);
        Self {
            flat_window_start,
            row_map,
            render_skip,
        }
    }

    /// Map a rendered row to an on-screen row, or `None` if it is scrolled off
    /// the top of the screen (above the bottom-anchored window).
    pub(super) const fn rendered_to_screen(&self, rendered_row: usize) -> Option<usize> {
        rendered_row.checked_sub(self.render_skip)
    }

    /// Map an on-screen row to its rendered row.
    const fn screen_to_rendered(&self, screen_row: usize) -> usize {
        screen_row.saturating_add(self.render_skip)
    }
}

/// Resolve a pointer position in the command-block gutter to the
/// `CommandBlockId` of the block whose rendered row range the pointer is
/// over, accounting for folds.
///
/// `pos` is a logical-point position; only its `y` is used (the gutter
/// spans the full pane height to the left of `terminal_rect`).  Returns
/// `None` when the row maps to a fold placeholder for no block, or to a
/// row not covered by any block.  Mirrors the fold-aware row mapping the
/// renderer uses so the hit-test agrees with what is painted.
fn gutter_block_id_at_pos(
    pos: Pos2,
    snap: &TerminalSnapshot,
    view_state: &ViewState,
    terminal_rect: Rect,
    logical_cell_h: f32,
) -> Option<freminal_common::buffer_states::command_block::CommandBlockId> {
    if logical_cell_h <= 0.0 {
        return None;
    }
    // Screen row under the pointer (relative to the terminal area top), then
    // its rendered row in the bottom-anchored layout.
    let screen_row = ((pos.y - terminal_rect.min.y) / logical_cell_h)
        .floor()
        .approx_as::<usize>()
        .ok()?;

    // Build the same fold-aware layout the renderer uses this frame.
    let layout = FoldLayout::new(snap, &view_state.folded_blocks);
    let rendered_row = layout.screen_to_rendered(screen_row);
    // Running blocks extend only to the cursor's row (last output line so far),
    // not the bottom of the pane (106.2b).
    let running_extent = super::coords::running_block_extent(snap);

    match layout.row_map.rendered_to_snapshot(rendered_row) {
        // A live snapshot row → containment hit-test against the blocks.
        Some(RenderedRow::Snapshot(snap_row)) => {
            let buffer_row = layout.flat_window_start + snap_row;
            crate::gui::command_blocks::gutter_block_for_row(
                &snap.command_blocks,
                snap.row_base,
                buffer_row,
                running_extent,
            )
            .map(|b| b.id)
        }
        // A fold placeholder → the folded block itself.
        Some(RenderedRow::Placeholder(range)) => Some(range.command_block_id),
        None => None,
    }
}

/// Compute the **screen**-row span (inclusive) of the command block whose
/// gutter the pointer is currently hovering, for the hover-tint overlay.
///
/// Despite the name, the returned span is screen-row space, not
/// rendered-row space: the final step below converts through
/// `layout.rendered_to_screen` before returning, because the caller
/// consumes the result directly as an index into the screen-indexed
/// `rendered_shaped_lines` array at the `widget.rs` call site (124.14b-ii
/// recon) -- the same array `screen_selection_rendered` indexes.
///
/// The **gutter strip is the sole hover trigger** (73.5): hovering a cell in
/// the terminal output area does nothing block-related, so the tint no longer
/// fires during text selection, mouse-tracking apps, or passive cursor motion.
/// Returns `None` when the feature is off, the gutter is disabled
/// (`gutter_inset == 0`), the alternate screen is active, there are no blocks,
/// the pointer is not over the gutter, or the hovered block is entirely inside
/// a fold.  The result must be recomputed before the vertex-rebuild decision so
/// a hover-only change can invalidate the cached background instances.
#[allow(clippy::too_many_arguments)]
pub(super) fn compute_command_block_hover_rows(
    snap: &TerminalSnapshot,
    view_state: &ViewState,
    command_blocks_config: &freminal_common::config::CommandBlocksConfig,
    layout: &FoldLayout,
    pane_rect: Rect,
    terminal_rect: Rect,
    gutter_inset: f32,
    logical_cell_h: f32,
) -> Option<(usize, usize)> {
    if !crate::gui::command_blocks::command_block_overlays_visible(
        command_blocks_config.enabled,
        snap.is_alternate_screen,
        !snap.command_blocks.is_empty(),
    ) {
        return None;
    }
    // No gutter (feature off / `gutter = "off"`) means no hover trigger.
    if gutter_inset <= 0.0 || logical_cell_h <= 0.0 {
        return None;
    }

    // The gutter strip is the only hover surface: the pointer must be in the
    // reserved inset, left of the terminal rect.
    let mouse_position = view_state.mouse_position?;
    if mouse_position.x < pane_rect.min.x
        || mouse_position.x >= terminal_rect.min.x
        || mouse_position.y < terminal_rect.min.y
        || mouse_position.y >= terminal_rect.max.y
    {
        return None;
    }

    let win_start = layout.flat_window_start;
    let snap_rows = snap.term_height.saturating_add(snap.window_extra_rows);

    // Map y → screen row → rendered row → buffer row (live rows only; a
    // placeholder resolves to the folded block via its first row).
    let screen_row = ((mouse_position.y - terminal_rect.min.y) / logical_cell_h)
        .floor()
        .approx_as::<usize>()
        .ok()?;
    let rendered_row = layout.screen_to_rendered(screen_row);
    let buffer_row = match layout.row_map.rendered_to_snapshot(rendered_row) {
        Some(RenderedRow::Snapshot(r)) => win_start + r,
        Some(RenderedRow::Placeholder(range)) => win_start + range.start_row,
        None => return None,
    };
    // Find the block containing this absolute row.  A running block (no
    // `end_row`) extends to the live bottom so its gutter is hoverable.
    let running_extent =
        super::coords::visible_window_start(snap) + snap.term_height.saturating_sub(1);
    let block = crate::gui::command_blocks::gutter_block_for_row(
        &snap.command_blocks,
        snap.row_base,
        buffer_row,
        running_extent,
    )?;
    // The block stores logical row numbers; resolve them to buffer indices.
    let block_rows = crate::gui::command_blocks::BlockRows::resolve(block, snap.row_base)?;
    let start = block_rows.command_start?;
    let end = block_rows.end.unwrap_or(running_extent);
    // Clip [start, end] to the flattened window, then convert each endpoint
    // into screen-row space.  If the entire block sits inside a fold or is
    // scrolled off the top, None.
    let win_end = win_start + snap_rows;
    if end < win_start || start >= win_end {
        return None;
    }
    let s_snap = start.saturating_sub(win_start);
    let e_snap = end
        .saturating_sub(win_start)
        .min(snap_rows.saturating_sub(1));
    let s_screen = layout.rendered_to_screen(layout.row_map.snapshot_to_rendered(s_snap)?)?;
    let e_screen = layout.rendered_to_screen(layout.row_map.snapshot_to_rendered(e_snap)?)?;
    Some((s_screen.min(e_screen), s_screen.max(e_screen)))
}

/// Resolve the (column, row) cell under the pointer for URL-hover purposes,
/// or `None` when the pointer is not actually over this pane.
///
/// Every pane in a split runs `write_input_to_terminal` against the same
/// window-wide event queue, so `view_state.mouse_position` can hold a
/// position that belongs to a sibling pane rather than this one (issue
/// #510: a URL on a pane's top row got "stuck" hovered when the pointer
/// moved into the pane stacked above it). Requiring containment here --
/// not just a `Some`/`None` check on `mouse_position` -- is what stops a
/// stale out-of-pane position from resolving, via
/// `encode_egui_mouse_pos_as_usize`'s edge clamp, to this pane's row/column
/// 0 and reviving a stale cached URL. Mirrors the equivalent guard in
/// `compute_command_block_hover_rows` for the sibling gutter-hover feature.
pub(super) fn cell_under_pointer_for_url_hover(
    mouse_position: Option<Pos2>,
    terminal_rect: Rect,
    character_size: (f32, f32),
) -> Option<(usize, usize)> {
    let pos = mouse_position?;
    if !terminal_rect.contains(pos) {
        return None;
    }
    Some(encode_egui_mouse_pos_as_usize(
        pos,
        character_size,
        terminal_rect.min,
    ))
}

/// Outcome of a scrollbar render+interaction pass. `new_offset` is the
/// scroll offset the user dragged to (if any). `rendered` is whether the
/// thumb was actually painted this frame. `hovered` is the
/// window-exit-corrected hover state (`latest_pos().is_some() &&
/// interact_pos() over the hit rect`) — the SAME signal the thumb's painted
/// alpha uses, so the paint and the damage decision can never drift a frame
/// apart on window-exit.
pub(super) struct ScrollbarOutcome {
    pub(super) new_offset: Option<usize>,
    pub(super) rendered: bool,
    pub(super) hovered: bool,
    /// The scrollbar's exact hit rect this frame ([`scrollbar_hit_rect`]),
    /// or `None` when the thumb was not rendered at all — mirrors
    /// `rendered`, since there is nothing to hover/drag when the scrollbar
    /// is not shown. Task 124.3b: published verbatim into
    /// `PanePointerReportInputs::scrollbar_hit_rect` so the out-of-frame
    /// pointer-motion predicate can compare it across the previous and
    /// current pointer position without recomputing scrollbar geometry.
    pub(super) hit_rect: Option<egui::Rect>,
}

impl ScrollbarOutcome {
    /// The outcome for a frame where the thumb was not rendered at all
    /// (scrolled to the live bottom, or a degenerate zero-height viewport).
    const fn not_rendered() -> Self {
        Self {
            new_offset: None,
            rendered: false,
            hovered: false,
            hit_rect: None,
        }
    }
}

/// The scrollbar's exact clickable/hoverable hit rect for a pane, computed
/// from the pane's viewport rect alone.
///
/// Task 124.3b: single source of truth for this geometry, replacing the
/// constants (`SCROLLBAR_WIDTH`/`SCROLLBAR_MARGIN`/`HIT_TEST_PADDING`) that
/// were previously hand-copied between `show()`'s scrollbar pre-check and
/// [`handle_scrollbar`] itself — the two could not drift apart by
/// construction before this extraction only because nobody had yet changed
/// one without the other.
pub(super) fn scrollbar_hit_rect(viewport: egui::Rect) -> egui::Rect {
    const SCROLLBAR_WIDTH: f32 = 6.0;
    const SCROLLBAR_MARGIN: f32 = 2.0;
    // Wider hit-test area so the narrow pill is easy to grab.
    const HIT_TEST_PADDING: f32 = 6.0;

    let track_right = viewport.right() - SCROLLBAR_MARGIN;
    let track_left = track_right - SCROLLBAR_WIDTH;

    egui::Rect::from_min_max(
        egui::pos2(track_left - HIT_TEST_PADDING, viewport.top()),
        egui::pos2(track_right + HIT_TEST_PADDING, viewport.bottom()),
    )
}
///
/// The scrollbar is shown when the user is actively scrolled back
/// (`scroll_offset > 0`).  It disappears at the live bottom.
///
/// Supports click-to-position and drag-to-scroll.  Returns a
/// [`ScrollbarOutcome`] describing the new `scroll_offset` (if the user
/// interacted with the scrollbar), whether the thumb was rendered this
/// frame, and whether it was hovered.
pub(super) fn handle_scrollbar(
    scroll_offset: usize,
    max_scroll_offset: usize,
    ui: &Ui,
    dragging: &mut bool,
) -> ScrollbarOutcome {
    const SCROLLBAR_WIDTH: f32 = 6.0;
    const SCROLLBAR_MARGIN: f32 = 2.0;
    const MIN_THUMB_HEIGHT: f32 = 12.0;

    // Only show when scrolled back into history — but keep rendering
    // while the user is mid-drag so the scrollbar doesn't vanish when
    // they drag to the bottom.
    if !*dragging && (scroll_offset == 0 || max_scroll_offset == 0) {
        return ScrollbarOutcome::not_rendered();
    }
    if max_scroll_offset == 0 {
        *dragging = false;
        return ScrollbarOutcome::not_rendered();
    }

    let painter = ui.painter();

    // ── Dimensions ───────────────────────────────────────────────────────
    let viewport = ui.max_rect();
    let track_top = viewport.top();
    let track_bottom = viewport.bottom();
    let track_height = track_bottom - track_top;
    if track_height <= 0.0 {
        return ScrollbarOutcome::not_rendered();
    }

    let track_right = viewport.right() - SCROLLBAR_MARGIN;
    let track_left = track_right - SCROLLBAR_WIDTH;

    // ── Thumb geometry ───────────────────────────────────────────────────
    let max_f = max_scroll_offset.approx_as::<f32>().unwrap_or(0.0);
    let total = max_f + track_height;
    let thumb_fraction = (track_height / total).clamp(0.05, 1.0);
    let thumb_height = (track_height * thumb_fraction)
        .max(MIN_THUMB_HEIGHT)
        .min(track_height);

    // Position: scroll_offset 0 = bottom, max = top.
    let scrollable_track = track_height - thumb_height;
    let position_fraction = scroll_offset.approx_as::<f32>().unwrap_or(0.0) / max_f;
    let thumb_top = scrollable_track.mul_add(1.0 - position_fraction, track_top);

    let thumb_rect = Rect::from_min_max(
        Pos2::new(track_left, thumb_top),
        Pos2::new(track_right, thumb_top + thumb_height),
    );

    // ── Mouse interaction ────────────────────────────────────────────────
    // Single source of truth for the hit-test rect (Task 124.3b) — see
    // `scrollbar_hit_rect`'s doc.
    let hit_rect = scrollbar_hit_rect(viewport);

    let new_offset = ui.input(|i| {
        let ptr = &i.pointer;
        let primary_down = ptr.primary_down();
        let ptr_pos = ptr.interact_pos();

        if !primary_down {
            *dragging = false;
            return None;
        }

        if let Some(pos) = ptr_pos {
            // Start drag if clicking within the hit-test area.
            if !*dragging && ptr.primary_pressed() && hit_rect.contains(pos) {
                *dragging = true;
            }

            if *dragging {
                // Map pointer Y to scroll_offset.
                // Centre the thumb on the pointer position.
                let thumb_centre_y = pos.y;
                let thumb_top_y = thumb_centre_y - thumb_height / 2.0;
                let clamped_top = thumb_top_y.clamp(track_top, track_top + scrollable_track);
                let frac = if scrollable_track > 0.0 {
                    1.0 - (clamped_top - track_top) / scrollable_track
                } else {
                    0.0
                };
                let new_off = (frac * max_f).round();
                // Clamp to valid range.
                let clamped = new_off
                    .approx_as::<usize>()
                    .unwrap_or(0)
                    .min(max_scroll_offset);
                return Some(clamped);
            }
        }

        None
    });

    // ── Appearance ───────────────────────────────────────────────────────
    // `interact_pos()` lags `latest_pos()` by one frame on window-exit
    // (egui's documented `Event::PointerGone` behavior — `latest_pos` clears
    // immediately, `interact_pos` not until the next frame). Fold in
    // `pointer_in_window` (`latest_pos().is_some()`) so the PAINTED alpha and
    // the returned hover state both use the SAME same-frame-corrected signal.
    // Sourcing the paint and the damage decision (see the call site's
    // `scrollbar_damage_decision`) from one consistent signal is what keeps
    // the thumb's hover alpha from getting stuck one frame on window-exit —
    // mirroring how the command-block gutter (#461) drives both its tint and
    // its damage from the single non-lagged `view_state.mouse_position`.
    let effectively_hovered = ui.input(|i| {
        i.pointer.latest_pos().is_some()
            && i.pointer
                .interact_pos()
                .is_some_and(|pos| hit_rect.contains(pos))
    });
    let alpha = if *dragging {
        220
    } else if effectively_hovered {
        200
    } else {
        150
    };
    let color = Color32::from_rgba_premultiplied(200, 200, 200, alpha);
    let rounding = SCROLLBAR_WIDTH / 2.0;

    painter.rect_filled(thumb_rect, rounding, color);

    ScrollbarOutcome {
        new_offset,
        rendered: true,
        hovered: effectively_hovered,
        hit_rect: Some(hit_rect),
    }
}

/// Decide whether the command-block gutter's hover-tint state changed
/// enough to need a repaint, and what the new cached hover state should be.
///
/// This is the pure decision logic factored out of the gutter hover block
/// so it is unit-testable without a live `egui::Ui`/`Context`.
///
/// `hovered` is the raw `Response::hovered()` for this frame's gutter
/// interact region. `pointer_in_window` is whether the pointer currently
/// has *any* position in this window
/// (`PointerState::latest_pos().is_some()`). `was_hovering` is the cached
/// state from the previous frame (`ViewState::pointer_in_gutter_last_frame`
/// or equivalent).
///
/// `hovered()` is keyed off egui's `interact_pos()`, which lags
/// `latest_pos()` by one frame specifically when the pointer leaves the OS
/// window (egui's own documented behavior around `Event::PointerGone`:
/// `latest_pos` clears to `None` immediately, but `interact_pos` is not
/// cleared until the next frame). A naive `hovered != was_hovering` edge
/// check would therefore read stale-`true` on the exact frame the pointer
/// leaves the window, evaluate `true != true` = no change, and fail to
/// schedule the very repaint needed to observe the real (cleared) state on
/// a later frame -- since nothing else is guaranteed to wake a frame,
/// the hover tint could get stuck indefinitely. Folding in
/// `pointer_in_window` (cleared same-frame on window-exit) closes that gap:
/// a pointer that has left the window is treated as "not hovering"
/// immediately, without waiting for `interact_pos()`'s one-frame lag to
/// catch up.
///
/// Returns `(needs_repaint, new_cached_state)`.
const fn gutter_hover_repaint_decision(
    hovered: bool,
    pointer_in_window: bool,
    was_hovering: bool,
) -> (bool, bool) {
    let effectively_hovered = hovered && pointer_in_window;
    (effectively_hovered != was_hovering, effectively_hovered)
}

/// Snapshot of the scrollbar's render-visibility + effective-hover state for
/// a single frame, used by [`scrollbar_damage_decision`].
///
/// Bundled into a named struct (rather than passed as loose bool
/// parameters) both reads more clearly at the two call sites (this frame's
/// observed state vs. the previous frame's cached state) and keeps
/// `scrollbar_damage_decision` under clippy's bool-parameter-count limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScrollbarDamageState {
    /// Whether the thumb was rendered (shown) this frame.
    pub(super) rendered: bool,
    /// Whether the thumb was hovered this frame. MUST already fold in
    /// `pointer_in_window` (`hovered && latest_pos().is_some()`) to avoid
    /// the `interact_pos()` one-frame window-exit lag that PR #461
    /// documents for the gutter.
    pub(super) effectively_hovered: bool,
}

/// Decide whether the scrollbar's damage state changed enough to need a
/// repaint + a forced Full present this frame, given this frame's observed
/// [`ScrollbarDamageState`] and the previous frame's cached state.
///
/// A rendered->not-rendered transition erases the previously-painted thumb
/// (force Full to clear it). A hover-alpha change while rendered repaints the
/// thumb at a new alpha (force Full — the thumb is on the plain painter,
/// outside per-pane VBO damage). Either also needs a repaint scheduled since
/// nothing else is guaranteed to wake a frame. Returns whether a repaint +
/// forced Full present is needed — the single bool drives both, matching
/// the gutter's `request_repaint` + Full pattern.
const fn scrollbar_damage_decision(
    current: ScrollbarDamageState,
    previous: ScrollbarDamageState,
) -> bool {
    let visibility_changed = current.rendered != previous.rendered;
    let hover_changed =
        current.rendered && (current.effectively_hovered != previous.effectively_hovered);
    visibility_changed || hover_changed
}

/// Duration of the visual bell flash overlay.
const BELL_FLASH_DURATION: Duration = Duration::from_millis(150);

/// Maximum alpha for the bell flash overlay (0–255).
const BELL_FLASH_MAX_ALPHA: u8 = 60;

/// Steady-state alpha for the persistent bell overlay shown when the
/// window is unfocused and a bell has fired (0–255).
const BELL_PERSISTENT_ALPHA: u8 = 30;

/// Outcome of evaluating whether a visual bell overlay should still be
/// shown, computed by the pure [`bell_flash_outcome`] decision function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BellFlashOutcome {
    /// Window is unfocused: paint a persistent, non-fading overlay at the
    /// given alpha and keep `bell_since` set. Self-heals (re-evaluated as
    /// `Fading`/`Cleared`) the moment the window is next found focused.
    Persistent { alpha: u8 },
    /// Window is focused and the flash duration has not yet elapsed: paint
    /// a fading overlay at the given alpha and keep `bell_since` set so the
    /// fade continues next frame.
    Fading { alpha: u8 },
    /// Window is focused and the flash duration has elapsed: clear
    /// `bell_since`, nothing more to paint.
    Cleared,
}

/// Pure mapping from a [`BellFlashOutcome`] to the repaint delay
/// [`paint_bell_flash`] needs (subtask 121.12), factored out so the
/// delay-selection logic is unit-testable without a live `egui::Ui` — see
/// [`paint_bell_flash`]'s doc for why the delay is returned rather than
/// requested on the `Context` directly.
const fn bell_flash_repaint_delay(outcome: BellFlashOutcome) -> Option<std::time::Duration> {
    match outcome {
        BellFlashOutcome::Fading { .. } => Some(std::time::Duration::from_millis(16)),
        BellFlashOutcome::Persistent { .. } | BellFlashOutcome::Cleared => None,
    }
}

/// Pure decision logic for [`paint_bell_flash`], factored out so it is
/// unit-testable without a live `egui::Ui`/`Context`.
///
/// `window_focused` must be the OS window's *current* focus state — see the
/// caller-discipline note on [`paint_bell_flash`] for why this must never be
/// a per-pane cached flag.
fn bell_flash_outcome(window_focused: bool, elapsed: Duration) -> BellFlashOutcome {
    if !window_focused {
        return BellFlashOutcome::Persistent {
            alpha: BELL_PERSISTENT_ALPHA,
        };
    }

    // Focused: if the flash duration has elapsed the bell either fired
    // while unfocused (the user just alt-tabbed back) or the fade-out
    // already completed — either way, clear immediately.
    if elapsed >= BELL_FLASH_DURATION {
        return BellFlashOutcome::Cleared;
    }

    // Linear fade from BELL_FLASH_MAX_ALPHA → 0 over the flash duration.
    let progress = elapsed.as_secs_f32() / BELL_FLASH_DURATION.as_secs_f32();
    let alpha_f = f32::from(BELL_FLASH_MAX_ALPHA) * (1.0 - progress);
    let alpha: u8 = alpha_f.approx_as::<u8>().unwrap_or(0);
    BellFlashOutcome::Fading { alpha }
}

/// Paint a semi-transparent white overlay for the visual bell.
///
/// **Focused window:** a brief flash that fades from [`BELL_FLASH_MAX_ALPHA`]
/// to 0 over [`BELL_FLASH_DURATION`] milliseconds. Once elapsed,
/// `view_state.bell_since` is cleared.
///
/// **Unfocused window:** a persistent subtle overlay at
/// [`BELL_PERSISTENT_ALPHA`] that remains until the window regains focus.
/// When focus returns the flash duration will have long since elapsed, so
/// `bell_since` is cleared on the first focused frame (no fade).
///
/// Focus is read live from `ui.ctx().input(|i| i.focused)` every frame
/// rather than from any per-pane cached flag. This function runs for every
/// rendered pane (active or not, in the active tab or a background one), so
/// the focus check must reflect the OS window's *current* focus state
/// unconditionally -- a value that is only updated while a given pane
/// happens to be the active one would go stale for every other pane and
/// get permanently stuck on the "unfocused, non-fading, no-repaint" branch
/// below (regression fixed here: a bell firing in a background/inactive
/// pane, or a newly created split/tab that never itself received a real
/// focus transition, would flash once and then never clear).
///
/// Returns the repaint delay this call needs (subtask 121.12): `Some(16ms)`
/// while a fade is in progress (so the fade continues smoothly next frame),
/// `None` for the static persistent overlay and the cleared case. THE
/// CALLER MUST FOLD THE RETURNED DELAY INTO THE PANE'S [`PaneRenderCache`]
/// via `PaneRenderCache::request_repaint_after` — this function deliberately
/// does NOT call `ui.ctx().request_repaint_after()` itself, because that
/// would be invisible to `effective_repaint_delay`'s suppressed-pointer
/// substitution (in `freminal-windowing`) and would be silently downgraded
/// to the much longer fallback interval while the mouse is moving over
/// terminal content.
fn paint_bell_flash(
    ui: &Ui,
    terminal_rect: Rect,
    view_state: &mut ViewState,
) -> Option<std::time::Duration> {
    let since = view_state.bell_since?;

    let window_focused = ui.ctx().input(|i| i.focused);
    let outcome = bell_flash_outcome(window_focused, since.elapsed());
    match outcome {
        BellFlashOutcome::Persistent { alpha } => {
            // No repaint request — the overlay is static and doesn't need
            // continuous redraws while the window is in the background.
            let overlay_color = Color32::from_rgba_premultiplied(alpha, alpha, alpha, alpha);
            ui.painter().rect_filled(terminal_rect, 0.0, overlay_color);
        }
        BellFlashOutcome::Fading { alpha } => {
            let overlay_color = Color32::from_rgba_premultiplied(alpha, alpha, alpha, alpha);
            ui.painter().rect_filled(terminal_rect, 0.0, overlay_color);
        }
        BellFlashOutcome::Cleared => {
            view_state.bell_since = None;
        }
    }
    bell_flash_repaint_delay(outcome)
}

/// Context menu action produced by the right-click popup.
///
/// These actions are dispatched after `render_context_menu` returns because
/// some (e.g. Copy) need clipboard channel access that is threaded through
/// the caller.
enum ContextMenuAction {
    Copy,
    Paste,
    SelectAll,
    OpenUrl(String),
    /// Copy the URL string to the clipboard. Distinct from `Copy` (which
    /// copies the current selection) and from `OpenUrl` (which launches the
    /// browser). Surfaced only when the right-click cell is inside an
    /// OSC 8 hyperlink.
    CopyUrl(String),
    NewTerminal,
    /// Copy the output range `[start_row, end_row]` of the command block
    /// the right-click occurred inside, full-width per row.
    CopyCommandOutput {
        start_row: RowNumber,
        end_row: RowNumber,
    },
}

/// Render the right-click context menu when `view_state.context_menu_pos`
/// is `Some`.
///
/// The menu is drawn as an `egui::Area` at the pixel position captured when
/// the right-click occurred. Items are:
///
/// - **Copy** (enabled only when a selection exists)
/// - **Paste**
/// - **Select All**
/// - **New Terminal** (opens a new tab)
/// - **Open URL** (shown only when the right-clicked cell is inside a URL span)
///
/// When the user clicks outside the popup or picks an item, the menu closes
/// and the relevant `ViewState` fields are cleared.
///
/// Actions that require full GUI state (e.g. spawning a new tab) are pushed
/// onto `deferred_actions` so the caller can dispatch them after this returns.
fn render_context_menu(
    ui: &Ui,
    snap: &TerminalSnapshot,
    view_state: &mut ViewState,
    input_tx: &Sender<InputEvent>,
    clipboard_rx: &Receiver<String>,
    deferred_actions: &mut Vec<freminal_common::keybindings::KeyAction>,
    copied: &mut bool,
) {
    let Some(menu_pos) = view_state.context_menu_pos else {
        return;
    };

    let mut action: Option<ContextMenuAction> = None;
    let mut close = false;

    let area_id = ui.id().with("terminal_context_menu");

    // Always render the Area so that egui tracks its bounds and interaction
    // state. The `InnerResponse.response` gives us `clicked_elsewhere()`
    // which uses egui's own layer-aware hit testing — far more reliable
    // than manually checking `area_rect` from memory.
    let area_response = render_context_menu_area(
        ui,
        snap,
        view_state,
        menu_pos,
        area_id,
        &mut action,
        &mut close,
    );

    // Use egui's built-in `clicked_elsewhere()` for dismiss detection.
    // This checks `any_click` (fires on pointer *release*, not press),
    // so the opening right-click press does not cause a false dismissal
    // on the same frame.
    if area_response.response.clicked_elsewhere() {
        close = true;
    }

    dispatch_context_menu_action(
        action,
        ui,
        view_state,
        snap,
        input_tx,
        clipboard_rx,
        deferred_actions,
        copied,
    );

    if close {
        view_state.context_menu_cell = None;
        view_state.context_menu_pos = None;
    }
}

/// Draw the popup area with menu buttons.
///
/// Returns the outer `InnerResponse` from `Area::show()` so the caller can
/// use `response.clicked_elsewhere()` for dismiss detection.
///
/// Separated from [`render_context_menu`] to stay within the 100-line
/// function limit.
fn render_context_menu_area(
    ui: &Ui,
    snap: &TerminalSnapshot,
    view_state: &ViewState,
    menu_pos: Pos2,
    area_id: egui::Id,
    action: &mut Option<ContextMenuAction>,
    close: &mut bool,
) -> egui::InnerResponse<()> {
    let has_selection = view_state.selection.has_selection();

    // The right-clicked cell is stored as a logical row, so it keeps pointing
    // at the clicked text even if the menu stays open while rows are evicted.
    // Resolve it against this frame's snapshot; a cell whose row has since
    // been evicted resolves to nothing and offers no URL / command entries.
    let context_cell = view_state
        .context_menu_cell
        .and_then(|cell| cell.resolve(snap));

    // Look up whether the right-clicked cell sits inside a URL span.
    let url_under_cursor = context_cell.and_then(|cell| {
        super::coords::url_at_cell(
            cell.row,
            cell.col,
            &snap.visible_chars,
            &snap.visible_tags,
            super::coords::visible_window_start(snap),
            &snap.row_offsets,
        )
    });

    // Look up whether the right-clicked cell sits inside a completed
    // OSC 133 command block.  Returns `(start_row, end_row)` of the
    // block's output region if the click was inside a block with a
    // captured C marker and a recorded D marker.
    let command_output_range = context_cell.and_then(|cell| {
        let block = super::input::find_block_containing_row(snap, cell.row)?;
        let (start, end) = super::input::block_output_range(block, snap.row_base)?;
        // The menu action carries stable row numbers, not indices.
        Some((snap.row_number_at(start), snap.row_number_at(end)))
    });

    egui::Area::new(area_id)
        .order(egui::Order::Foreground)
        .fixed_pos(menu_pos)
        .interactable(true)
        .constrain(true)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(120.0);

                // Apply egui's menu styling so items render as borderless
                // rows (transparent until hovered) rather than boxed buttons,
                // matching the menu-bar dropdowns. egui applies this
                // automatically inside `menu_button`/`context_menu`, but this
                // popup is hand-rolled via `Area` + `Frame::popup`, so it must
                // be applied explicitly here.
                egui::containers::menu::menu_style(ui.style_mut());

                // Copy — disabled when no text is selected.
                if ui
                    .add_enabled(has_selection, egui::Button::new("Copy"))
                    .clicked()
                {
                    *action = Some(ContextMenuAction::Copy);
                    *close = true;
                }

                if ui.button("Paste").clicked() {
                    *action = Some(ContextMenuAction::Paste);
                    *close = true;
                }

                ui.separator();

                if ui.button("Select All").clicked() {
                    *action = Some(ContextMenuAction::SelectAll);
                    *close = true;
                }

                ui.separator();

                if ui.button("New Terminal").clicked() {
                    *action = Some(ContextMenuAction::NewTerminal);
                    *close = true;
                }

                // "Open URL" — only shown when the clicked cell is a URL.
                if let Some(ref url) = url_under_cursor {
                    ui.separator();
                    let label = format!("Open {}", truncate_url(url, 40));
                    if ui.button(label).clicked() {
                        *action = Some(ContextMenuAction::OpenUrl(url.clone()));
                        *close = true;
                    }
                    if ui.button("Copy URL").clicked() {
                        *action = Some(ContextMenuAction::CopyUrl(url.clone()));
                        *close = true;
                    }
                }

                // "Copy Command Output" — only shown when the clicked
                // cell is inside a completed OSC 133 command block
                // (`OutputStart` and `CommandFinished` markers both
                // recorded).  Running and incomplete blocks suppress
                // the entry entirely.
                if let Some((start_row, end_row)) = command_output_range {
                    ui.separator();
                    if ui.button("Copy Command Output").clicked() {
                        *action = Some(ContextMenuAction::CopyCommandOutput { start_row, end_row });
                        *close = true;
                    }
                }
            });
        })
}

/// Truncate a URL for display in the context menu, keeping at most `max_len`
/// characters and appending an ellipsis if truncated.
///
/// Uses `char_indices` to find a safe byte boundary so multi-byte UTF-8
/// URLs are never split mid-character.
fn truncate_url(url: &str, max_len: usize) -> String {
    if url.chars().count() <= max_len {
        url.to_string()
    } else {
        let byte_end = url
            .char_indices()
            .nth(max_len)
            .map_or(url.len(), |(idx, _)| idx);
        let mut s = url[..byte_end].to_string();
        s.push('…');
        s
    }
}

/// Execute the action chosen from the context menu.
///
/// Separated from [`render_context_menu`] to stay within the 100-line
/// function limit.
///
/// Actions that require full GUI state (e.g. `NewTerminal`) are pushed onto
/// `deferred_actions` rather than executed directly, because this function
/// does not have access to `FreminalGui` or `TabManager`.
// All eight parameters are cohesive context-menu-dispatch state (mirrors the
// existing `too_many_arguments` allowance on `compute_command_block_hover_rows`
// above); splitting them into a struct would not meaningfully improve clarity.
#[allow(clippy::too_many_arguments)]
fn dispatch_context_menu_action(
    action: Option<ContextMenuAction>,
    ui: &Ui,
    view_state: &mut ViewState,
    snap: &TerminalSnapshot,
    input_tx: &Sender<InputEvent>,
    clipboard_rx: &Receiver<String>,
    deferred_actions: &mut Vec<freminal_common::keybindings::KeyAction>,
    copied: &mut bool,
) {
    let Some(action) = action else {
        return;
    };

    match action {
        ContextMenuAction::Copy if let Some((start, end)) = view_state.selection.normalised() => {
            if let Err(e) = input_tx.send(InputEvent::ExtractSelection {
                start_row: start.row,
                start_col: start.col,
                end_row: end.row,
                end_col: end.col,
                is_block: view_state.selection.is_block,
            }) {
                error!("Context menu: failed to send ExtractSelection: {e}");
            } else if let Ok(text) =
                clipboard_rx.recv_timeout(std::time::Duration::from_millis(100))
                && !text.is_empty()
            {
                ui.ctx().copy_text(text);
                view_state.selection.clear();
                *copied = true;
            }
        }
        ContextMenuAction::Copy => {}
        ContextMenuAction::Paste => {
            // Ask the platform to inject an Event::Paste on the next frame.
            // egui-winit reads the system clipboard internally and delivers
            // the content as Event::Paste, which our existing handler in
            // input.rs already processes (including bracketed paste mode).
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        }
        ContextMenuAction::SelectAll => {
            // Select from the first visible cell to the last visible cell.
            view_state.selection.select_all(snap);
        }
        ContextMenuAction::OpenUrl(url) => {
            let url_str = url;
            if let Err(e) = std::thread::Builder::new()
                .name("freminal-open-url".to_string())
                .spawn(move || {
                    if let Err(e) = open::that(&url_str) {
                        error!("Failed to open URL {url_str}: {e}");
                    }
                })
            {
                error!("Failed to spawn URL-open thread: {e}");
            }
        }
        ContextMenuAction::CopyUrl(url) => {
            ui.ctx().copy_text(url);
            *copied = true;
        }
        ContextMenuAction::NewTerminal => {
            deferred_actions.push(freminal_common::keybindings::KeyAction::NewTab);
        }
        ContextMenuAction::CopyCommandOutput { start_row, end_row } => {
            // Full-width per-row extraction.  `extract_text` clamps per
            // row to the actual cell count, so passing
            // `term_width - 1` as `end_col` gives us "to end of row"
            // without spurious trailing whitespace.
            let end_col = snap.term_width.saturating_sub(1);
            if let Err(e) = input_tx.send(InputEvent::ExtractSelection {
                start_row,
                start_col: 0,
                end_row,
                end_col,
                is_block: false,
            }) {
                error!("Context menu Copy Command Output: failed to send ExtractSelection: {e}");
            } else if let Ok(text) =
                clipboard_rx.recv_timeout(std::time::Duration::from_millis(100))
                && !text.is_empty()
            {
                ui.ctx().copy_text(text);
                *copied = true;
            }
        }
    }
}

/// Represents a pending GPU-side resource update that must be applied inside a
/// `PaintCallback` (which has access to the GL context).
///
/// - [`PendingGpuOp::Load`] — load or replace the resource with the given value.
/// - [`PendingGpuOp::Clear`] — destroy / reset the resource.
///
/// The outer `Option<PendingGpuOp<T>>` on the field indicates *whether* a change
/// is pending at all (`None` = no pending change this frame).
#[derive(Debug, Clone)]
pub(super) enum PendingGpuOp<T> {
    /// Load or replace the resource with this value.
    Load(T),
    /// Destroy / reset the resource.
    Clear,
}

/// GPU resources shared between the main thread (vertex building) and the
/// egui `PaintCallback` closure (draw calls).
///
/// ## Threading invariant
///
/// Despite the `Arc<Mutex<…>>` wrapper, `RenderState` is **GUI-thread-only**.
/// It is never accessed from the PTY processing thread, the OS PTY reader
/// thread, or any other background thread. The `Mutex` is not here to
/// coordinate between threads — it exists purely for **interior mutability**:
///
/// - egui's `PaintCallback` requires captures to be `Send + Sync + 'static`,
///   which forces ownership via `Arc`.
/// - The vertex-building code (before the callback fires) and the draw code
///   (inside the callback) both need `&mut` access to the same buffers.
/// - Rust cannot prove the two accesses are disjoint through an `Arc`, so
///   the `Mutex` provides the runtime `&mut` path.
///
/// In practice the lock is always uncontended: both the vertex builder and
/// the paint callback run sequentially on the GUI thread within a single
/// frame. If a second thread ever tries to lock this `Mutex`, that is a bug.
pub struct RenderState {
    pub(super) renderer: TerminalRenderer,
    pub(super) atlas: GlyphAtlas,
    /// Per-cell instanced background data (col, row, r, g, b, a per cell).
    pub(super) bg_instances: Vec<f32>,
    /// Decoration vertex data (underlines, strikethrough, cursor, selection).
    pub(super) deco_verts: Vec<f32>,
    pub(super) fg_instances: Vec<f32>,
    /// Pre-built image vertex data (one quad per unique inline image).
    pub(super) image_verts: Vec<f32>,
    /// Authoritative `(z_index, id)` draw order for the quads in
    /// `image_verts`, computed by [`build_image_verts`] (Task 100.7b).
    /// `draw_images` iterates this SAME list so the vertex slab order and the
    /// draw order can never drift apart. Retained (not recomputed) on the
    /// cursor-only fast path, alongside `image_verts` and `snap_images`.
    pub(super) image_draw_order: Vec<ImageDrawEntry>,
    /// Snapshot image map from the last full rebuild, cloned into `RenderState`
    /// so the `PaintCallback` closure (`Send`+`Sync`) can pass it to `draw_with_verts`.
    pub(super) snap_images: std::collections::HashMap<u64, InlineImage>,
    /// The exact float range (not byte range) the cursor occupies in
    /// `deco_verts`; always the buffer's final region.  Set after every full
    /// vertex rebuild from [`build_background_instances`]'s return value, and
    /// updated by every cursor-only patch, so cursor-only frames can replace
    /// just this region whatever its length (0, solid, or hollow).
    ///
    /// `None` means no rebuild has ever populated this render state: the
    /// "never built" signal that 124.21 lists among the eight genuinely
    /// global damage triggers. It is deliberately NOT inferred from
    /// `deco_verts.is_empty()`: a plain-shell pane has no underline,
    /// selection, or search quads, so a blink-off (or DECTCEM-hidden)
    /// cursor legitimately leaves `deco_verts` empty on a built pane, and
    /// reading that as "never built" turns every blink-on frame into a full
    /// rebuild and a `Full` present.
    pub(super) cursor_vert_range: Option<CursorVertRange>,
    /// Cell dimensions in physical pixels, for the instanced background shader.
    pub(super) cell_width_px: f32,
    pub(super) cell_height_px: f32,
    /// Background opacity (0.0–1.0), for the instanced background shader.
    pub(super) bg_opacity: f32,
    /// Background image opacity (0.0–1.0).
    pub(super) bg_image_opacity: f32,
    /// Background image fit mode.
    pub(super) bg_image_mode: freminal_common::config::BackgroundImageMode,
    /// Shared window-level post-processing renderer.
    ///
    /// All panes in the session share one `WindowPostRenderer` (via `Arc<Mutex<…>>`).
    /// When a user GLSL shader is active, this pane's `PaintCallback` renders its
    /// terminal content into the window FBO.  A window-level `PaintCallback` registered
    /// after the pane loop applies the post pass to egui's framebuffer.
    ///
    /// As with [`RenderState`], the `Arc<Mutex<…>>` here provides interior
    /// mutability for `PaintCallback` captures — not cross-thread
    /// synchronisation. `WindowPostRenderer` is only ever touched on the
    /// GUI thread.
    pub(super) window_post: Arc<Mutex<WindowPostRenderer>>,
    /// Pending background image load/clear to apply on the next `PaintCallback`.
    ///
    /// `Some(PendingGpuOp::Load(path))` → load the image at `path`.
    /// `Some(PendingGpuOp::Clear)` → clear the current image.
    /// `None` → no pending change this frame.
    pub(super) pending_bg_image: Option<PendingGpuOp<std::path::PathBuf>>,
    /// Live, feature-gated render-work profile for this pane (Task 125.4's
    /// state machine; Task 125.5 wires `show()` and this pane's
    /// `PaintCallback` into it). Both need to reach the SAME instance --
    /// `show()` computes the raw/resolved classification and starts a
    /// fresh token every call; the paint callback finalizes it -- and
    /// `RenderState` is already the `Arc<Mutex<...>>` both share (see
    /// this struct's own threading-invariant doc above). Entirely absent
    /// from a default build.
    #[cfg(feature = "frame-profiling")]
    pub(super) live_profile: LiveRenderProfile,
    /// The window's retired-renderer queue (Task 125.C2), cloned from
    /// `window_post` at construction.
    ///
    /// `Drop` has no GL context, so [`Drop for RenderState`](#impl-Drop-for-RenderState)
    /// moves `renderer` into this queue instead of destroying it; the window
    /// drains the queue where its context is current. See
    /// `gui::renderer::retire` for the design and invariants.
    retire_queue: GlRetireQueue,
}

/// Retire this pane's GL objects when the pane's render state is discarded.
///
/// Runs when the **last** `Arc<Mutex<RenderState>>` clone is dropped. The
/// pane's paint callback captures a clone, so a renderer an in-flight paint
/// callback is still drawing through is never retired early; it is retired
/// only after the frame that used it has been painted and its callbacks
/// released. Touches no GL (see [`GlRetireQueue::retire`]).
impl Drop for RenderState {
    fn drop(&mut self) {
        self.retire_queue.retire(std::mem::take(&mut self.renderer));
    }
}

impl RenderState {
    /// Clear the glyph atlas, forcing all glyphs to be re-rasterised on
    /// the next frame.
    ///
    /// Called when font metrics change (font size, DPI, ligature toggle) so
    /// that stale glyph textures are discarded.
    pub fn clear_atlas(&mut self) {
        self.atlas.clear();
    }

    /// Schedule a background image load on the next `PaintCallback`.
    ///
    /// `path = Some(p)` → load the image at `p`.
    /// `path = None` → clear the current image.
    pub fn set_pending_bg_image(&mut self, path: Option<std::path::PathBuf>) {
        self.pending_bg_image = Some(path.map_or(PendingGpuOp::Clear, PendingGpuOp::Load));
    }
}

/// Create a new [`RenderState`] with default (empty) values.
///
/// Used when constructing new panes — each pane needs its own GPU render
/// state since `PaintCallback` closures capture the `Arc<Mutex<RenderState>>`
/// and execute asynchronously during egui's paint phase.
///
/// `window_post` is the shared window-level post-processing renderer.
/// All panes in the same session share one instance.
#[must_use]
pub fn new_render_state(window_post: Arc<Mutex<WindowPostRenderer>>) -> Arc<Mutex<RenderState>> {
    // Take the window's retire queue handle once, here, so `Drop` never has to
    // lock `window_post` (which could be held, or the pane dropped from inside
    // a lock scope).
    let retire_queue = window_post
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retire_queue();
    Arc::new(Mutex::new(RenderState {
        renderer: TerminalRenderer::new(),
        atlas: GlyphAtlas::default(),
        bg_instances: Vec::new(),
        deco_verts: Vec::new(),
        fg_instances: Vec::new(),
        image_verts: Vec::new(),
        image_draw_order: Vec::new(),
        snap_images: std::collections::HashMap::new(),
        cursor_vert_range: None,
        cell_width_px: 0.0,
        cell_height_px: 0.0,
        bg_opacity: 1.0,
        bg_image_opacity: 0.5,
        bg_image_mode: freminal_common::config::BackgroundImageMode::Cover,
        window_post,
        pending_bg_image: None,
        #[cfg(feature = "frame-profiling")]
        live_profile: LiveRenderProfile::new(),
        retire_queue,
    }))
}

/// Emit the Task 125.5/125.6 live render-work profile's periodic flush
/// summary, if (and only if) `profile` currently has one pending.
///
/// Two call sites in [`FreminalTerminalWidget::show`] invoke this: right
/// after `show()`'s own [`LiveRenderProfile::start`] call (which may have
/// just finalized a stale, superseded token as unpainted and crossed a
/// [`LiveRenderProfile::FLUSH_EVERY`] boundary doing so), and inside the
/// pane's `PaintCallback` right after [`LiveRenderProfile::complete`]
/// (the normal case, where the boundary is crossed by a painted
/// completion). [`LiveRenderProfile::take_flush_signal`] is the single
/// stateful, one-shot source of truth both share, so whichever call site
/// happens to run first after a crossing consumes the signal and actually
/// logs; the other observes nothing pending and logs nothing -- this
/// helper exists precisely so that "check the signal, and log if it's
/// set" is written once rather than duplicated at both call sites (and so
/// the two can never drift into logging the same boundary twice or
/// checking it with different field sets).
///
/// Task 125.6 extends the line with real upload-byte totals: the overall
/// total, the per-buffer-category breakdown [`LiveRenderProfile::upload_totals`]
/// owns, and the per-resolved-class byte distribution
/// [`LiveRenderProfile::class_upload_totals`] owns -- dividing the latter
/// by the matching `resolved_*` count above gives an average
/// bytes-per-frame figure per class, which is the reconciliation this
/// pair of fields exists to make possible.
#[cfg(feature = "frame-profiling")]
fn maybe_log_live_render_profile_flush(
    profile: &mut LiveRenderProfile,
    pane_id: crate::gui::panes::PaneId,
) {
    if !profile.take_flush_signal() {
        return;
    }
    let uploads = profile.upload_totals();
    let class_bytes = profile.class_upload_totals();
    tracing::debug!(
        target: LiveRenderProfile::LOG_TARGET,
        pane_id = ?pane_id,
        painted = profile.painted_count(),
        unpainted = profile.unpainted_count(),
        observations = profile.observation_count(),
        raw_cursor_only = profile.raw_counts().cursor_only,
        raw_bounded = profile.raw_counts().bounded,
        raw_reevaluate_full_rebuild = profile.raw_counts().reevaluate_full_rebuild,
        resolved_reuse = profile.class_counts().reuse,
        resolved_cursor_only = profile.class_counts().cursor_only,
        resolved_bounded = profile.class_counts().bounded,
        resolved_full = profile.class_counts().full,
        changed_row_histogram = ?profile.row_bucket_counts(),
        upload_bytes_total = uploads.total(),
        upload_bytes_background_instance_vbo = uploads.background_instance_vbo_bytes,
        upload_bytes_foreground_instance_vbo = uploads.foreground_instance_vbo_bytes,
        upload_bytes_decoration_vbo = uploads.decoration_vbo_bytes,
        upload_bytes_image_vertex_vbo = uploads.image_vertex_vbo_bytes,
        upload_bytes_image_texture = uploads.image_texture_bytes,
        upload_bytes_atlas_full = uploads.atlas_full_bytes,
        upload_bytes_atlas_subrect = uploads.atlas_subrect_bytes,
        upload_bytes_by_class_reuse = class_bytes.reuse,
        upload_bytes_by_class_cursor_only = class_bytes.cursor_only,
        upload_bytes_by_class_bounded = class_bytes.bounded,
        upload_bytes_by_class_full = class_bytes.full,
        "live render-work profile (task 125.5/125.6): cumulative raw \
         VertexRebuild decisions vs. resolved render-work class, the \
         changed-row histogram, and real per-buffer-category and \
         per-resolved-class upload-byte totals, for this pane since its \
         RenderState was created -- flushed every \
         LiveRenderProfile::FLUSH_EVERY finalized observations, whichever \
         of `start`'s stale-token sweep or the paint callback's \
         `complete` happened to cross the boundary"
    );
}

/// Per-pane dirty-tracking cache for the terminal render pipeline.
///
/// Each pane needs its own set of "previous frame" state to support
/// incremental rendering optimisations (cursor-only fast path, content
/// change detection via `Arc::ptr_eq`, theme/selection/blink tracking).
///
/// This struct is stored on [`Pane`](super::super::panes::Pane) alongside
/// the per-pane `Arc<Mutex<RenderState>>`.
// Bools are inherently boolean dirty-tracking flags (cursor blink on/off,
// cursor shown/hidden, text blink visible, overlay open) — enums would add
// noise without improving clarity.
#[allow(clippy::struct_excessive_bools)]
pub struct PaneRenderCache {
    /// Mouse state from the most recently rendered frame.
    pub(super) previous_mouse_state: Option<PreviousMouseState>,
    /// Last key processed by input handling.
    pub(super) previous_key: Option<Key>,
    /// Physical Super/Command key hold-state from the most recently
    /// processed frame (Task 101.2), tracked per side. egui exposes no
    /// `Modifiers` bit for the physical Super/Windows key on Linux/Windows,
    /// so this is tracked across frames via discrete `SuperLeft`/`SuperRight`
    /// press/release events observed in `write_input_to_terminal`. Tracking
    /// each side independently avoids losing state when both are held and one
    /// is released.
    pub(super) super_state: super::input::SuperKeyState,
    /// Last scroll amount processed.
    pub(super) previous_scroll_amount: f32,
    /// The effective cursor blink phase from the most recently rendered
    /// frame (already pinned to `On` for a non-focused cursor, as
    /// `evaluate_frame_dirty_state` returns it).
    pub(super) previous_cursor_blink_phase: CursorBlinkPhase,
    /// The cursor's pane/window focus from the most recently rendered frame.
    /// A non-`Focused` -> `Focused` transition re-anchors the blink clock so
    /// the regained cursor starts in its visible half (see
    /// [`blink_anchor_action`](super::cursor_blink)). Initialised to `Focused`
    /// so the first frame never counts as a regain: a freshly-created pane is anchored by the
    /// activation reset (`cursor_blink_reset_pending`) instead.
    pub(super) previous_cursor_focus: CursorFocus,
    /// Cursor position from the most recently rendered frame.
    pub(super) previous_cursor_pos: freminal_common::buffer_states::cursor::CursorPos,
    /// Cursor screen row from the most recently drawn frame. This advances
    /// only after damage has consumed the old row and never on `skip_draw`.
    pub(super) previous_cursor_screen_row: Option<usize>,
    /// How the cursor was drawn in the most recently rendered frame
    /// (visibility and focus-driven shape together: a hidden-to-hollow or
    /// solid-to-hollow switch is a change even when the cursor did not move).
    pub(super) previous_cursor_appearance: CursorAppearance,
    /// Cursor color override from the most recently rendered frame.
    pub(super) previous_cursor_color_override: Option<(u8, u8, u8)>,
    /// The `visible_chars` arc from the last full vertex rebuild.
    ///
    /// As of Task 124.12 this is **not** the primary content-change signal
    /// any more — that is [`Self::last_rendered_row_epochs`]. Its one
    /// surviving use is the selection auto-clear's confirmation comparison
    /// in `frame_dirty.rs`: a byte-value comparison against
    /// `snap.visible_chars` that deliberately stays chars-only (not
    /// epoch-based), so an SGR-only repaint (a program redrawing identical
    /// text in a different colour) does not clear the user's selection. See
    /// that call site's comment for the full rationale.
    pub(super) last_rendered_visible: Option<Arc<Vec<TChar>>>,
    /// Per-row content epochs from the last full vertex rebuild, parallel to
    /// [`freminal_terminal_emulator::snapshot::TerminalSnapshot::row_epochs`].
    ///
    /// This is the baseline `diff_row_epochs` (`frame_dirty.rs`) diffs the
    /// current frame's epochs against to compute
    /// [`super::frame_dirty::ChangedRows`] — the primary content-change
    /// signal since Task 124.12. It replaces the two `Arc::ptr_eq` tests
    /// that used to run against `last_rendered_visible` and the
    /// now-deleted `last_rendered_line_widths`: the epoch already folds in
    /// merged characters, merged format tags, and each row's `LineWidth`,
    /// so a separate line-width pointer test has nothing left to catch.
    pub(super) last_rendered_row_epochs: Option<Arc<[u64]>>,
    /// Theme pointer from the last full vertex rebuild.  When this changes,
    /// we must force a full rebuild so foreground/background vertex colors
    /// are re-resolved against the new palette.
    pub(super) previous_theme: Option<&'static ThemePalette>,
    /// The normalised selection from the last full vertex rebuild, used to
    /// detect selection changes that require a full rebuild. Logical row
    /// numbers (Task 125.17), so it is stable across eviction and scrolling.
    pub(super) previous_selection: Option<(LogicalCell, LogicalCell)>,
    /// The selection in snapshot-row space at the last full vertex rebuild:
    /// `(start_col, start_row, end_col, end_row)` relative to the flattened
    /// window, as `DirtyTrackingOutcome::screen_selection` reports it
    /// (Task 125.17).
    ///
    /// [`Self::previous_selection`] is stable under eviction, so it cannot
    /// notice a selection that stayed attached to its text while the window
    /// slid beneath it and the highlight therefore moved on screen. Comparing
    /// this window-relative form can. Updated in lockstep with
    /// [`Self::previous_selection`] and reset everywhere it is.
    pub(super) previous_screen_selection: Option<(usize, usize, usize, usize)>,
    /// The screen-row span (inclusive, `(min, max)`) the selection occupied
    /// at the last full vertex rebuild, in **screen**-row space, not
    /// logical-row space (Task 124.14b-i).
    ///
    /// [`Self::previous_selection`] holds logical row numbers and
    /// `DirtyTrackingOutcome::screen_selection` is snapshot-row space, so a
    /// bounded-damage union built by naively comparing the two would be
    /// comparing coordinates from two different spaces. Translating the old
    /// selection with *this* frame's `win_start` is also wrong whenever the
    /// window moved between frames — new output pushing rows into
    /// scrollback moves the window without setting `scroll_changed`, so
    /// there is no reliable signal that the translation would even need
    /// correcting.  The question a bounded-damage rebuild must answer is
    /// "where on screen was the old highlight painted", which is inherently
    /// a screen-space question — so the answer is stored in screen space
    /// once, here, and needs no translation and no window-movement guard
    /// when read back.
    ///
    /// Updated in lockstep with [`Self::previous_selection`] — see that
    /// field's call site in `show()` — so it only advances on a frame that
    /// actually ran the full rebuild body; a frame that reused the previous
    /// frame's vertices unchanged leaves both fields alone. Reset to `None`
    /// everywhere [`Self::previous_selection`] is reset to `None`, so the
    /// two fields can never disagree about whether a previous selection was
    /// recorded at all.
    pub(super) previous_selection_screen_rows: Option<(usize, usize)>,
    /// Text blink slow-visibility from the most recently rendered frame.
    pub(super) previous_text_blink_slow_visible: bool,
    /// Text blink fast-visibility from the most recently rendered frame.
    pub(super) previous_text_blink_fast_visible: bool,
    /// Whether a UI overlay (modal dialog or dropdown menu) was open on the
    /// previous frame.
    pub(super) overlay_was_open_last_frame: bool,
    /// Whether a pane-border drag-to-resize was active on the previous
    /// frame. Mirrors `overlay_was_open_last_frame`'s one-frame release tail
    /// but tracks a distinct cause: the release-click that ends a divider
    /// drag must not leak through to the terminal on the same frame
    /// `border_drag_active` goes false (issue #453 review). Kept separate
    /// from `overlay_was_open_last_frame` so each latch tracks only its own
    /// cause.
    pub(super) border_drag_was_active_last_frame: bool,
    /// Fingerprint of the search-highlight state from the most recently
    /// rendered frame (see `SearchState::render_epoch`).
    pub(super) previous_search_epoch: u64,
    /// The terminal cell `(col, row)` the mouse was hovering over in the
    /// previous frame.
    pub(super) previous_hover_cell: Option<(usize, usize)>,
    /// The command-block hover-tint row range from the previous frame,
    /// compared against this frame's [`super::frame_dirty::DirtyTrackingOutcome::command_block_hover_rows`]
    /// to detect a hover change (different range, or appearing/disappearing),
    /// which forces a vertex rebuild so the tint is baked into the
    /// background VBO.
    ///
    /// Despite its name, this is already **screen**-row space:
    /// [`super::frame_dirty::compute_command_block_hover_rows`]'s final step
    /// calls `FoldLayout::rendered_to_screen` before returning. So unlike
    /// selection -- whose [`Self::previous_selection`] is in logical-row space and
    /// needed a dedicated screen-space companion -- this field is unioned
    /// straight into [`build_bounded_damage`] with no conversion and no
    /// second field (Task 124.14b-ii).
    pub(super) previous_command_block_hover_rows: Option<(usize, usize)>,
    /// Cached URL from the most recent URL hover lookup.
    pub(super) cached_hovered_url: Option<Arc<Url>>,
    /// Pointer identity of the `visible_chars` `Arc` used for the last URL
    /// hover lookup.
    pub(super) hover_snap_ptr: usize,
    /// The `row_epochs` `Arc` this pane observed on the previous frame
    /// (issue #439 fix #4).
    ///
    /// The PTY thread only publishes a new `Arc<TerminalSnapshot>` when real
    /// output arrives (~a few times/sec under a settled full-screen TUI), but
    /// the GUI reads the currently-published snapshot on *every* frame
    /// (~60fps). Comparing the current `row_epochs` `Arc` against this field
    /// tells us whether *this* frame is the first observation of a
    /// genuinely-new snapshot. It gates the content-driven 16ms repaint
    /// scheduling in `app_impl` so an already-drawn snapshot does not
    /// perpetually re-arm a 60fps wake for pixels that are not changing.
    ///
    /// This used to compare `visible_chars` `Arc` pointers instead. Task
    /// 124.12 switched it to `row_epochs` because the pointer test reported
    /// "new" for a byte-identical re-flatten in a fresh `Arc` — a cursor-blink
    /// repaint, for example, allocates a fresh `Arc<Vec<TChar>>` with
    /// unchanged bytes — which re-armed a 16ms wake for pixels that were not
    /// changing. Comparing epochs suppresses that too: two `Arc`s with
    /// content-equal `row_epochs` are treated as the same observation even
    /// when they are different allocations. This is the same direction issue
    /// #439 fix #4 was already going, taken further.
    ///
    /// A skipped (`skip_draw`) frame does not corrupt this baseline: repaint
    /// *scheduling* is not the vertex-rebuild trigger. The rebuild decision
    /// uses [`Self::last_rendered_row_epochs`], which only advances on an
    /// actual full rebuild, so a skipped frame updating this field cannot
    /// cause a change to be lost — every `build_snapshot` is independently
    /// paired with its own `request_repaint_after` (see the comment at this
    /// field's call site in `app_impl.rs`).
    ///
    /// Distinct from [`Self::last_rendered_row_epochs`], which tracks the last
    /// *full rebuild* (conditionally updated inside the render path); this
    /// tracks the last *observation* (updated unconditionally every frame,
    /// before the widget draws). An owned `Arc` (not a bare address) is stored
    /// to avoid the ABA hazard of a freed allocation reusing an old address.
    pub(super) last_observed_row_epochs: Option<Arc<[u64]>>,
    /// Per-pane shaping cache for text layout.
    pub(crate) shaping_cache: crate::gui::shaping::ShapingCache,
    /// Whether the user is currently dragging the scrollbar thumb.
    pub(super) scrollbar_dragging: bool,
    /// Whether the pointer was over the command-block gutter hit zone on the
    /// previous frame.  Used to request one extra repaint on the frame the
    /// pointer leaves the gutter so the hover-tint clearing frame is drawn.
    pub(super) pointer_in_gutter_last_frame: bool,
    /// Whether the scrollbar thumb was hovered on the previous frame (using
    /// the #461-proven `hovered && pointer_in_window` shape to avoid the
    /// `interact_pos()` one-frame window-exit lag). Drives a one-frame
    /// hover-alpha clearing repaint + Full damage, since the thumb is painted
    /// on the plain egui painter outside the per-pane VBO damage tracking.
    pub(super) scrollbar_was_hovered_last_frame: bool,
    /// Whether the scrollbar thumb was rendered at all on the previous frame.
    /// A rendered->not-rendered transition (e.g. scrolled to bottom) must
    /// force one Full clear to erase the previously-painted thumb pixels; a
    /// hover-only latch misses the common visible-but-unhovered vanish case.
    pub(super) scrollbar_was_rendered_last_frame: bool,
    /// Terminal width (columns) from the last full vertex rebuild.  When this
    /// changes (window resize), the cell-instance VBOs still contain vertices
    /// for the old column count; drawing them into a smaller viewport leaves
    /// stale glyph slivers in the right-edge slop region.  We force a full
    /// rebuild whenever the dimensions change.
    pub(super) previous_term_width: usize,
    /// Terminal height (rows) from the last full vertex rebuild.  See
    /// `previous_term_width` for rationale.
    pub(super) previous_term_height: usize,
    /// Hash of the sorted fold-range list from the last full vertex rebuild.
    ///
    /// When the user folds or unfolds a command block, the rendered row
    /// layout shifts (folded ranges collapse to a single placeholder row).
    /// The cached vertex buffers still encode the *previous* layout, so we
    /// must force a full rebuild when this epoch changes.
    pub(super) previous_fold_epoch: u64,
    /// Per-frame list of fold-placeholder click targets in window/logical
    /// pixel coordinates, paired with the `CommandBlockId` to unfold when
    /// the user clicks them.
    ///
    /// Rebuilt every frame inside the render path (cheap — at most one
    /// entry per folded block) and consumed by [`super::input::write_input_to_terminal`]
    /// to convert clicks on placeholder rows into `view_state.unfold()`
    /// calls.  Empty when no folds are active.
    pub(super) placeholder_hit_rects: Vec<(
        Rect,
        freminal_common::buffer_states::command_block::CommandBlockId,
    )>,
    /// Pointer identity of each visible image's *selected-frame* pixel
    /// buffer, as of the last full vertex rebuild.
    ///
    /// Maps image id -> `Arc::as_ptr(..).addr()` of whichever `Arc<Vec<u8>>`
    /// was uploaded to the GPU (root `pixels` for still images, the
    /// GUI-selected animation frame for animated images — see
    /// `build_image_pixel_ptrs`). A store-only pixel mutation that changes no
    /// cell and no `run_mode` (e.g. a Kitty `a=c` animation compose, Task
    /// 100.12) still swaps in a new `Arc` for the affected frame, so
    /// comparing this map against the current snapshot's pixel pointers
    /// detects the change and forces a full rebuild + texture re-upload even
    /// though `content_changed`/`image_frame_changed` stay false.
    pub(super) last_rendered_image_pixel_ptrs: std::collections::HashMap<u64, usize>,
    /// Damage report for the frame just rendered (#435).
    ///
    /// Records which render path this pane took, so the per-window
    /// aggregation in `app_impl` can decide whether the whole frame was a
    /// pure cursor-only update (skip-clear + partial present) or must clear
    /// and present fully. See [`PaneFrameDamage`] for the three cases. Any
    /// pane with a visible cursor -- the active pane's solid one or an
    /// inactive pane's hollow one -- can produce a cursor rect when only its
    /// cursor changed; a pane whose content and cursor are both unchanged
    /// reports [`PaneFrameDamage::Unchanged`] and never forces a full frame.
    ///
    /// Set every frame by [`FreminalTerminalWidget::show`].
    ///
    /// [`PaneFrameDamage`]: crate::gui::renderer::PaneFrameDamage
    /// [`PaneFrameDamage::Unchanged`]: crate::gui::renderer::PaneFrameDamage::Unchanged
    pub(crate) last_frame_cursor_damage: crate::gui::renderer::PaneFrameDamage,
    /// Repaint delay this pane's `show()` needs, folded to the shortest across
    /// every in-frame requester. Drained by `central_body` after `show()`
    /// returns and folded into `shortest_repaint_delay`, so the need is visible
    /// in `App::take_terminal_requested_delay` (subtask 121.12). Requesting a
    /// repaint directly on the `Context` here would be invisible to
    /// `effective_repaint_delay`'s suppressed-pointer substitution and would be
    /// silently downgraded to the fallback interval.
    pub(crate) pending_repaint_delay: Option<std::time::Duration>,
    /// This frame's geometry and input-suppression snapshot for the
    /// out-of-frame immediate PTY mouse-report path (Task 124.3a). Set
    /// unconditionally, every frame `show()` runs, immediately after
    /// `terminal_rect` and `InputSuppressors` are computed — this is the
    /// *only* place `terminal_rect` (and therefore the pane's terminal-rect
    /// origin, `terminal_rect.min`) is computed; nothing else may re-derive
    /// it (subtask 122.15). `app_impl` lifts this into
    /// [`crate::gui::published_frame_state::PublishedFrameState`] right
    /// after `show()` returns, the same way it drains
    /// [`Self::pending_repaint_delay`], so subtask 121.17/124.3b can read a
    /// pane's terminal-rect origin from outside a frame via
    /// `PublishedFrameState::pane_terminal_origin`, which derives it from
    /// this same published snapshot rather than a second, parallel field —
    /// an earlier revision of this seam kept a dedicated
    /// `terminal_rect_origin` field here too, which Task 124.3a's review
    /// removed as a redundant, driftable copy of `terminal_rect.min`.
    /// Holds a stale (previous frame's) value before the first `show()`
    /// call and between frames; callers needing "is this fresh" semantics
    /// must go through the published type's own per-frame rebuild
    /// discipline rather than this field directly.
    pub(in crate::gui) pointer_report_inputs: PanePointerReportInputs,
    /// Cross-frame search-overlay damage state (Task 124.14d): the
    /// highlight rows and popup paint bounds the search overlay actually
    /// drew, so a bounded rebuild can union the old/new extents into its
    /// damage instead of forcing `Full` while search is open. See
    /// [`super::search_damage::SearchDamageState`] for the full invariant.
    pub(super) search_damage: super::search_damage::SearchDamageState,
    /// The OSC 9;4 progress bar's `(state, percent)` fingerprint as of the
    /// most recently rendered frame, or `None` when the bar was not drawn
    /// (progress inactive, or `progress_enabled` off) -- issue #507 phase
    /// B2. Compared against the current frame's fingerprint in `show()` to
    /// decide whether the bar's fixed-geometry damage rect needs merging
    /// into [`Self::last_frame_cursor_damage`]; see `merge_progress_damage`.
    pub(super) previous_progress: Option<(ProgressState, u8)>,
}

impl PaneRenderCache {
    /// Create a new cache with default initial values.
    #[must_use]
    pub fn new() -> Self {
        Self {
            previous_mouse_state: None,
            previous_key: None,
            super_state: super::input::SuperKeyState::default(),
            previous_scroll_amount: 0.0,
            previous_cursor_blink_phase: CursorBlinkPhase::On,
            previous_cursor_focus: CursorFocus::Focused,
            previous_cursor_pos: freminal_common::buffer_states::cursor::CursorPos::default(),
            previous_cursor_screen_row: None,
            previous_cursor_appearance: CursorAppearance::Hidden,
            previous_cursor_color_override: None,
            last_rendered_visible: None,
            last_rendered_row_epochs: None,
            previous_theme: None,
            previous_selection: None,
            previous_screen_selection: None,
            previous_selection_screen_rows: None,
            previous_text_blink_slow_visible: true,
            previous_text_blink_fast_visible: true,
            overlay_was_open_last_frame: false,
            border_drag_was_active_last_frame: false,
            previous_search_epoch: 0,
            previous_hover_cell: None,
            previous_command_block_hover_rows: None,
            cached_hovered_url: None,
            hover_snap_ptr: 0,
            last_observed_row_epochs: None,
            shaping_cache: crate::gui::shaping::ShapingCache::new(),
            scrollbar_dragging: false,
            pointer_in_gutter_last_frame: false,
            scrollbar_was_hovered_last_frame: false,
            scrollbar_was_rendered_last_frame: false,
            previous_term_width: 0,
            previous_term_height: 0,
            previous_fold_epoch: 0,
            placeholder_hit_rects: Vec::new(),
            last_rendered_image_pixel_ptrs: std::collections::HashMap::new(),
            last_frame_cursor_damage: crate::gui::renderer::PaneFrameDamage::Unchanged,
            pending_repaint_delay: None,
            pointer_report_inputs: PanePointerReportInputs::default(),
            search_damage: super::search_damage::SearchDamageState::new(),
            previous_progress: None,
        }
    }

    /// Physical Super/Command key hold-state as of the most recently
    /// rendered frame (Task 114.7).
    ///
    /// `super_pressed` is `pub(super)` (visible only within `gui::terminal`)
    /// because it is render-pipeline-internal state; this narrow accessor
    /// lets `app_impl.rs`'s raw-key drain (Task 114.7) read the true
    /// physical Super state for the active pane without widening the
    /// field's visibility.
    #[must_use]
    pub(crate) const fn super_pressed(&self) -> bool {
        self.super_state.any()
    }

    /// Whether a URL-hover tooltip is currently displayed for this pane.
    /// `cached_hovered_url` is `pub(super)` (render-pipeline internal); this
    /// narrow accessor lets `app_impl.rs`'s chrome-damage aggregation know
    /// the `Order::Tooltip` URL tooltip is on screen — TAIL chrome that must
    /// force `ChromeDamage::Changed`, which composes into
    /// `FrameDamage::Full` (see `compose_with_chrome_damage`), so a frame
    /// with the tooltip visible is never presented `Partial`. Mirrors
    /// [`Self::super_pressed`]'s pattern rather than widening the field's
    /// visibility.
    #[must_use]
    pub(crate) const fn hover_tooltip_active(&self) -> bool {
        self.cached_hovered_url.is_some()
    }

    /// This pane's search-overlay safety classification this frame (Task
    /// 124.14d) -- see [`crate::gui::search::SearchOverlaySafety`]. Mirrors
    /// [`Self::hover_tooltip_active`]'s narrow-accessor pattern:
    /// `search_damage` stays `pub(super)` (render-pipeline internal), and
    /// `app_impl.rs`'s chrome-damage aggregation reads only this
    /// classification, not the state.
    #[must_use]
    pub(crate) const fn search_overlay_safety(&self) -> crate::gui::search::SearchOverlaySafety {
        self.search_damage.safety()
    }

    /// This pane's search-overlay popup damage rects this frame (Task
    /// 124.14d), for `frame_damage::PaneDamageInput::search_overlay_rects`.
    #[must_use]
    pub(crate) fn search_overlay_damage_rects(&self) -> &[crate::gui::renderer::PaneDamageRect] {
        self.search_damage.overlay_damage_rects()
    }

    /// Record that this pane observed `row_epochs` this frame and report
    /// whether it is the first observation of a genuinely-new snapshot
    /// (issue #439 fix #4).
    ///
    /// Returns `true` only when `row_epochs` differs (element-for-element)
    /// from the one observed on the previous call — i.e. the PTY thread
    /// published a snapshot with genuinely different content since last
    /// frame. Returns `false` when the same published snapshot is being
    /// re-read (the ~14 idle frames between real updates under a settled
    /// full-screen TUI) AND when a fresh `Arc` carries content-identical
    /// `row_epochs` (e.g. a cursor-blink repaint's byte-identical re-flatten;
    /// see this field's doc comment).
    ///
    /// Always updates the stored `Arc` (a cheap refcount bump), so it must be
    /// called exactly once per pane per frame, before the content-driven
    /// repaint decision. `last_observed_row_epochs` is `pub(super)`
    /// (render-pipeline internal); this narrow accessor lets `app_impl.rs`'s
    /// repaint scheduler consult it without widening the field's visibility,
    /// mirroring [`Self::super_pressed`] / [`Self::hover_tooltip_active`].
    pub(crate) fn observe_row_epochs(&mut self, row_epochs: &Arc<[u64]>) -> bool {
        let is_new = self
            .last_observed_row_epochs
            .as_ref()
            .is_none_or(|prev| *prev != *row_epochs);
        self.last_observed_row_epochs = Some(Arc::clone(row_epochs));
        is_new
    }

    /// Invalidate the cached theme pointer so the next frame forces a full
    /// vertex rebuild with the new palette colors.
    pub const fn invalidate_theme_cache(&mut self) {
        self.previous_theme = None;
    }

    /// Force a full vertex rebuild on the next frame by clearing the cached
    /// `visible_chars` pointer and the recorded row epochs.
    pub fn invalidate_content(&mut self) {
        self.last_rendered_visible = None;
        self.last_rendered_row_epochs = None;
        self.shaping_cache.clear();
        self.last_rendered_image_pixel_ptrs.clear();
    }

    /// Record that some in-frame animation (bell flash, cursor trail,
    /// animated image, gutter hover clear, scrollbar damage) needs another
    /// repaint after `delay`, folding to the shortest delay requested so far
    /// this frame (subtask 121.12).
    ///
    /// Every in-frame repaint need must be routed through this method rather
    /// than calling `ui.ctx().request_repaint_after()` directly — see the
    /// doc comment on [`Self::pending_repaint_delay`] for why a direct
    /// `Context` call is invisible to `effective_repaint_delay`'s
    /// suppressed-pointer substitution.
    pub(crate) fn request_repaint_after(&mut self, delay: std::time::Duration) {
        self.pending_repaint_delay = Some(
            self.pending_repaint_delay
                .map_or(delay, |prev| prev.min(delay)),
        );
    }

    /// Drain and return the repaint delay accumulated this frame via
    /// [`Self::request_repaint_after`], if any. Called once per frame by
    /// `app_impl`'s `central_body` after `show()` returns, so a stale delay
    /// from a previous frame is never re-folded into the next frame's
    /// aggregate.
    pub(crate) const fn take_pending_repaint_delay(&mut self) -> Option<std::time::Duration> {
        self.pending_repaint_delay.take()
    }
}

impl Default for PaneRenderCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a map of image id -> pointer address of the currently-*selected*
/// frame's pixel buffer, for every image in `images`.
///
/// This mirrors exactly which `Arc<Vec<u8>>` the full-rebuild path uploads to
/// the GPU (see the `rs_ref.snap_images` frame-swap loop in `show()`): for an
/// animated image, the selected frame (per `selected`, the GUI-side wall-clock
/// playback cursor) via [`InlineImage::frame_pixels`], falling back to the
/// root `pixels` if that frame no longer exists; for a still image, the root
/// `pixels` directly.
///
/// Used to detect store-level pixel mutations (e.g. a Kitty `a=c` animation
/// compose, Task 100.12) that change no cell and no `run_mode` — so none of
/// the other full-rebuild triggers (`content_changed`, `image_frame_changed`,
/// ...) would otherwise fire, and the GPU texture would go stale forever.
fn build_image_pixel_ptrs(
    images: &std::collections::HashMap<u64, InlineImage>,
    selected: impl Fn(u64) -> u32,
) -> std::collections::HashMap<u64, usize> {
    images
        .iter()
        .map(|(id, img)| {
            let px = if img.is_animated() {
                img.frame_pixels(selected(*id)).unwrap_or(&img.pixels)
            } else {
                &img.pixels
            };
            (*id, Arc::as_ptr(px).addr())
        })
        .collect()
}

/// Returns `true` if the selected-frame pixel `Arc` pointer for any image in
/// `images` differs from what is recorded in `prev` (including images that
/// appeared or disappeared since `prev` was captured).
///
/// See [`build_image_pixel_ptrs`] for exactly which pixel buffer is compared
/// per image.
pub(super) fn image_pixels_changed(
    images: &std::collections::HashMap<u64, InlineImage>,
    selected: impl Fn(u64) -> u32,
    prev: &std::collections::HashMap<u64, usize>,
) -> bool {
    build_image_pixel_ptrs(images, selected) != *prev
}

/// Pixel geometry shared by every row-range rect a [`build_bounded_damage`]
/// call may emit. Grouped into one `Copy` struct so that function (and its
/// per-run helper) stay under clippy's `too_many_arguments` threshold —
/// mirrors `frame_dirty::FrameDirtyGeometry`'s precedent for the same reason.
#[derive(Clone, Copy)]
struct RowDamageGeometry {
    /// Row height in physical pixels (matches `row_h_f` at the `show` call
    /// site).
    row_h_f: f32,
    /// Terminal viewport width in physical pixels (`terminal_rect.width() *
    /// ppp`) — a row-range rect always spans the full width, since a content
    /// change on a row is not itself localised to a column range.
    viewport_width_px: f32,
    /// The terminal viewport's top-left corner, physical pixels, top-left
    /// origin — the same `vp_left_px`/`vp_top_px` the cursor-only arm
    /// derives (hoisted to the `show` call site by Task 124.14a so both
    /// arms share one computation).
    vp_left_px: f32,
    /// See [`Self::vp_left_px`].
    vp_top_px: f32,
    /// The full framebuffer height in physical pixels, needed for the
    /// top-left-to-bottom-left origin flip (see
    /// [`crate::gui::renderer::PaneDamageRect::from_cursor_cells`]).
    fb_height_px: i32,
}

/// Build the [`crate::gui::renderer::PaneDamageRect`] rect for one contiguous
/// run of on-screen rows `[run_start, run_end]` (inclusive), or `None` if the
/// run degenerates to nothing under `from_cursor_cells`' clamping.
///
/// Reuses [`crate::gui::renderer::PaneDamageRect::from_cursor_cells`] for the
/// coordinate transform (Task 124.14a's design decision) rather than
/// hand-rolling a second Y-flip / outward-rounding / framebuffer-clamp —
/// that function already consumes top-left-origin, viewport-relative
/// physical pixels and does exactly this conversion for the cursor's own
/// rect.
fn row_run_damage(
    run_start: usize,
    run_end: usize,
    geometry: RowDamageGeometry,
) -> Option<crate::gui::renderer::PaneDamageRect> {
    let start_f = run_start.approx_as::<f32>().unwrap_or(0.0);
    let run_len = run_end.saturating_sub(run_start).saturating_add(1);
    let len_f = run_len.approx_as::<f32>().unwrap_or(0.0);
    let cell = (
        0.0,
        start_f * geometry.row_h_f,
        geometry.viewport_width_px,
        len_f * geometry.row_h_f,
    );
    crate::gui::renderer::PaneDamageRect::from_cursor_cells(
        geometry.vp_left_px,
        geometry.vp_top_px,
        geometry.fb_height_px,
        &[cell],
    )
}

/// Build the [`crate::gui::renderer::PaneDamageRect`] rect for a pane's OWN
/// full rebuild (Task 124.21 finding 2: the multi-pane fan-out).
/// `decide_frame_damage` does `rects.clear(); break;` the moment any pane
/// reports [`crate::gui::renderer::PaneFrameDamage::Full`], discarding rects
/// already collected from every other pane -- so in a split, one pane
/// needing a full rebuild was forcing a full clear + present of every
/// provably-`Unchanged` sibling and all chrome. Reporting this pane's own
/// bounds instead of escalating the whole frame to `Full` stops the fan-out
/// at the source; `decide_frame_damage` itself is unchanged.
///
/// Returns `None` -- which the caller turns into
/// [`crate::gui::renderer::PaneFrameDamage::Full`] -- when the pane's own
/// bounds are degenerate: zero/negative extent, or a rect that clamps away
/// entirely against the framebuffer. A pane whose own bounds cannot be
/// established genuinely cannot bound its damage, and `Full` is the correct,
/// safe fallback there.
///
/// `pane_rect`, not `terminal_rect`, is the pane's own bounds: a full pane
/// rebuild also repaints the command-block gutter strip and the scrollbar,
/// both of which live inside `pane_rect` but outside `terminal_rect`.
///
/// Reuses [`crate::gui::renderer::PaneDamageRect::from_cursor_cells`] rather
/// than a second hand-rolled transform (124.14a's design decision). This is
/// the first caller whose rect can extend LEFT of the viewport origin
/// `vp_left_px`/`vp_top_px` measure from -- the cell offset below is
/// negative whenever the gutter is enabled -- but `from_cursor_cells`
/// already adds the offset before clamping left/top to 0, so no second
/// Y-flip transform is needed.
fn full_pane_rebuild_damage_rect(
    pane_rect: egui::Rect,
    terminal_rect: egui::Rect,
    ppp: f32,
    vp_left_px: f32,
    vp_top_px: f32,
    fb_height_px: i32,
) -> Option<crate::gui::renderer::PaneDamageRect> {
    rect_damage_relative_to_terminal(
        pane_rect,
        terminal_rect,
        ppp,
        vp_left_px,
        vp_top_px,
        fb_height_px,
    )
}

/// Shared coordinate transform behind [`full_pane_rebuild_damage_rect`] and
/// the search-overlay popup-rect conversion (Task 124.14d, see `show`'s
/// search-overlay block): convert a rect in the same logical-point space as
/// `terminal_rect` -- which may extend outside it, as both a pane's own
/// gutter-inclusive bounds and a floating popup anchored elsewhere in the
/// pane do -- into a [`crate::gui::renderer::PaneDamageRect`] rect, relative
/// to the viewport origin `vp_left_px`/`vp_top_px` already measure from.
/// Reusing one function for both callers is what stops them silently
/// drifting apart on the exact conversion (124.14a's design decision, the
/// same reasoning behind hoisting `vp_left_px`/`vp_top_px`/`fb_height_px`
/// themselves into [`viewport_framebuffer_geometry`]).
///
/// Returns `None` for a degenerate (zero/negative extent) or unconvertible
/// (fully clamped away) rect -- checked explicitly ahead of
/// `PaneDamageRect::from_cursor_cells`, which pads every cell outward by 1px
/// on every side before clamping, so a genuinely zero-width or zero-height
/// cell would NOT naturally come out `None` there (it would come out a
/// spurious ~2px-wide sliver instead). Only a fully out-of-framebuffer rect
/// degenerates inside `from_cursor_cells` itself. Catching zero/negative
/// extent here is what actually delivers "zero or negative width/height
/// falls back to the caller's unbounded case" rather than relying on
/// padding to do it by accident.
fn rect_damage_relative_to_terminal(
    rect: egui::Rect,
    terminal_rect: egui::Rect,
    ppp: f32,
    vp_left_px: f32,
    vp_top_px: f32,
    fb_height_px: i32,
) -> Option<crate::gui::renderer::PaneDamageRect> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let cell = (
        (rect.min.x - terminal_rect.min.x) * ppp,
        (rect.min.y - terminal_rect.min.y) * ppp,
        rect.width() * ppp,
        rect.height() * ppp,
    );
    crate::gui::renderer::PaneDamageRect::from_cursor_cells(
        vp_left_px,
        vp_top_px,
        fb_height_px,
        &[cell],
    )
}

/// Height, in physical pixels, of the OSC 9;4 progress indicator bar
/// painted across each pane's top edge (issue #507, phase B2). Thin enough
/// not to compete visually with the first row of terminal content, while
/// still reading clearly at typical DPI scales.
const PROGRESS_BAR_HEIGHT_PX: f32 = 3.0;

/// The OSC 9;4 progress bar's full-width track rect for a pane, in logical
/// points, anchored to `pane_rect`'s top edge.
///
/// Spans the pane's ENTIRE width -- including the command-block gutter
/// strip, if any -- not just the cell grid (`terminal_rect`), matching
/// Ghostty's "a progress bar on top of each split" placement (the
/// reference behavior this `ConEmu`-style OSC 9;4 extension is modelled
/// on; see <https://ghostty.org/docs/vt/osc/conemu>).
fn progress_bar_track_rect(pane_rect: egui::Rect, height_logical: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        pane_rect.min,
        egui::pos2(pane_rect.max.x, pane_rect.min.y + height_logical),
    )
}

/// The filled (determinate) portion of `track_rect` for `percent` (0-100),
/// left-anchored.
///
/// Pure geometry, extracted so the percent -> pixel-width conversion --
/// the obvious `as`-cast trap here, per `freminal-numeric-conversions` --
/// is directly unit-tested rather than only reachable through a render
/// pass. `percent` is clamped and converted via the lossless `f32::from`
/// (not a cast) before the width multiplication.
fn progress_fill_rect(track_rect: egui::Rect, percent: u8) -> egui::Rect {
    let percent_f = f32::from(percent.min(100)) / 100.0;
    let fill_width = track_rect.width() * percent_f;
    egui::Rect::from_min_max(
        track_rect.min,
        egui::pos2(track_rect.min.x + fill_width, track_rect.max.y),
    )
}

/// Merge the OSC 9;4 progress bar's damage into this frame's already-decided
/// `PaneFrameDamage` (issue #507, phase B2). Called only when the bar's
/// active/state/percent fingerprint changed since the last frame -- see the
/// call site in `show()`.
///
/// **Classification (`freminal-damage-model` skill, section 1):
/// BOUNDABLE-NOW.** Unlike terminal content, the bar's rect is a FIXED,
/// pane-relative extent that depends only on `pane_rect` -- never on
/// scroll position, folds, or the bar's own state/percent. So the instant
/// a fingerprint change is detected, the complete old-and-new extent is
/// already known: old and new are literally the same rect (only the fill
/// color/width *within* it changes, never the rect's own bounds), so no
/// separate erase-the-old-extent step is needed the way selection/hover/
/// search damage need one.
///
/// This safety is a **cross-file coupling**, not something this function
/// (or `dims_changed`) enforces on its own: `dims_changed` is defined in
/// CELL counts, so a pane-border drag that moves `pane_rect` by less than
/// one cell's worth of pixels changes the bar's on-screen rect without
/// flipping `dims_changed`. What actually keeps that case safe today is
/// three other mechanisms: (1) interactive pane-border dragging forces
/// `PaneFrameDamage::Full` via the pointer/border-drag `force_full` path in
/// `app_impl.rs`; (2) window resize and pixels-per-point changes force
/// `Full` via the chrome `size_changed` / `ppp_changed` signals; and (3)
/// theme changes are covered by the separate `theme_changed` global damage
/// category. If any of those three were changed or removed, a stale or
/// ghosted progress bar could reappear here with no test catching it.
///
/// `bar_rect` is `None` only when the viewport-relative geometry
/// conversion itself degenerated (e.g. a zero-height viewport). Per the
/// damage model's complete-bound rule (section 6), that is an
/// unprovable-bound case, not a "nothing to report" one, so it escalates
/// to `Full` rather than silently reporting no damage for a bar that DID
/// change.
fn merge_progress_damage(
    damage: crate::gui::renderer::PaneFrameDamage,
    bar_rect: Option<crate::gui::renderer::PaneDamageRect>,
) -> crate::gui::renderer::PaneFrameDamage {
    use crate::gui::renderer::PaneFrameDamage;
    let Some(bar_rect) = bar_rect else {
        return PaneFrameDamage::Full;
    };
    match damage {
        PaneFrameDamage::Full | PaneFrameDamage::CursorOnly(None) => damage,
        PaneFrameDamage::Unchanged => PaneFrameDamage::Region(vec![bar_rect]),
        PaneFrameDamage::CursorOnly(Some(r)) => PaneFrameDamage::Region(vec![r, bar_rect]),
        PaneFrameDamage::Region(mut rects) => {
            rects.push(bar_rect);
            PaneFrameDamage::Region(rects)
        }
    }
}

/// Compute the terminal viewport's top-left corner in physical framebuffer
/// pixels, plus the framebuffer height, from egui's own screen rect and
/// `ppp`. Shared by the cursor-only/bounded-damage full-rebuild arms
/// (hoisted at Task 124.14a) and the search-overlay popup-rect conversion
/// (Task 124.14d, computed again here because that block runs outside the
/// `!snap.skip_draw` guard the first computation lives inside) so a second
/// hand-rolled version cannot drift from the first.
fn viewport_framebuffer_geometry(ui: &Ui, terminal_rect: egui::Rect, ppp: f32) -> (f32, f32, i32) {
    let vp_left_px = terminal_rect.min.x * ppp;
    let vp_top_px = terminal_rect.min.y * ppp;
    let screen_h_logical = ui
        .ctx()
        .input(|i| i.raw.screen_rect.map_or(0.0, |r| r.max.y));
    let fb_height_px: i32 = (screen_h_logical * ppp)
        .ceil()
        .approx_as_by::<i32, conv2::RoundToNearest>()
        .unwrap_or(0);
    (vp_left_px, vp_top_px, fb_height_px)
}

/// Whether -- and how -- a frame's full vertex rebuild can report bounded
/// damage (Task 124.14).
///
/// Decided before the rebuild body runs, because the body is byte-for-byte
/// the same either way: [`VertexRebuild::Bounded`] means "full rebuild,
/// bounded *damage*", never a bounded rebuild. Bounding the vertex upload
/// itself is Task 125, gated on a fixed-stride relayout that does not exist
/// yet.
enum FullRebuildDamage {
    /// Content changed only within known bounds -- the frame's
    /// `changed_rows` row list (Task 124.14a), the current/previous
    /// selection's screen-row span (Task 124.14b-i), the current/previous
    /// command-block hover span (Task 124.14b-ii), or any combination of
    /// the three -- and no other full-repaint trigger fired. Build
    /// [`crate::gui::renderer::PaneFrameDamage::Region`] from it once the
    /// rebuild completes (or fall back to `Full` if every contributing row
    /// maps to nothing, e.g. collapsed inside a fold).
    Bounded,
    /// No bound is available; report
    /// [`crate::gui::renderer::PaneFrameDamage::Full`].
    Full,
}

/// The current/previous screen-row span for each of [`build_bounded_damage`]'s
/// selection and hover union sources. Every field is already screen-row
/// space -- see the two Task 124.14b-ii fields' doc comments for why no
/// further conversion happens inside [`build_bounded_damage`] itself.
///
/// The search fields (Task 124.14d) differ in shape from selection/hover:
/// search matches are not necessarily contiguous, so they are borrowed
/// slices of individual rows rather than a single `(start, end)` span. They
/// are still already screen-row space, sorted, and deduplicated --
/// `PaneRenderCache::search_damage` derives them from `search_highlights`
/// at the `show` call site (already snapshot -> rendered -> screen
/// translated) rather than re-translating `MatchSpan`s a second time.
///
/// Grouped into one `Copy` struct (mirroring `RowDamageGeometry`'s
/// precedent) so adding the hover pair at Task 124.14b-ii, and the search
/// pair at Task 124.14d, did not push [`build_bounded_damage`] over
/// clippy's `too_many_arguments` threshold. Slice references are `Copy`, so
/// this stays `Copy` despite the added lifetime.
#[derive(Clone, Copy)]
struct BoundedDamageSpans<'a> {
    /// This frame's selection highlight (derived from
    /// `screen_selection_rendered` at the call site, Task 124.14b-i).
    current_selection: Option<(usize, usize)>,
    /// Last frame's selection highlight
    /// ([`PaneRenderCache::previous_selection_screen_rows`], Task
    /// 124.14b-i).
    previous_selection: Option<(usize, usize)>,
    /// This frame's command-block hover tint (`command_block_hover_rows_early`
    /// -- see [`super::frame_dirty::compute_command_block_hover_rows`]'s doc
    /// for why no further conversion is needed here, unlike the selection
    /// field above, Task 124.14b-ii).
    current_hover: Option<(usize, usize)>,
    /// Last frame's command-block hover tint
    /// ([`PaneRenderCache::previous_command_block_hover_rows`], already
    /// screen-space -- Task 124.14b-ii).
    previous_hover: Option<(usize, usize)>,
    /// This frame's search-highlight screen rows (Task 124.14d), sorted and
    /// deduplicated.
    current_search_rows: &'a [usize],
    /// The search-highlight screen rows from the last full rebuild
    /// ([`super::search_damage::SearchDamageState::replace_highlight_rows`]'s
    /// return value, Task 124.14d), sorted and deduplicated.
    previous_search_rows: &'a [usize],
    /// This frame's cursor row when cursor state changed.
    current_cursor_row: Option<usize>,
    /// Last drawn cursor row when cursor state changed.
    previous_cursor_row: Option<usize>,
}

/// Whether an empty bounded-damage row union (Task 124.14d) should fall
/// back to [`crate::gui::renderer::PaneFrameDamage::Full`] or
/// [`crate::gui::renderer::PaneFrameDamage::Unchanged`].
///
/// [`Self::Unchanged`] is correct only when search is the *sole* bounded
/// source this frame -- `selection_changed` and `hover_changed` are false
/// and `changed_rows` is [`super::frame_dirty::ChangedRows::None`] -- and
/// its own old/current highlight-row union came out empty too: a query
/// with no visible matches before or after still changes the floating
/// search-bar popup, which the caller unions in separately (via
/// `frame_damage::PaneDamageInput::search_overlay_rects`), so the terminal
/// band genuinely contributed nothing this frame and `Unchanged` is the
/// accurate answer, not a fallback.
///
/// Selection, hover, and row sources keep the existing [`Self::Full`]
/// fallback: an empty union there means their own extent collapsed
/// entirely behind a fold (see [`build_bounded_damage`]'s doc), which is a
/// genuine "cannot bound this" case, not "nothing changed" -- reporting
/// `Unchanged` there would be silent visual corruption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmptyBoundedDamage {
    /// Fall back to `PaneFrameDamage::Full`.
    Full,
    /// Fall back to `PaneFrameDamage::Unchanged` -- correct only when
    /// search is the sole bounded source and its own row union is empty.
    Unchanged,
}

/// Build a [`crate::gui::renderer::PaneFrameDamage::Region`] from every
/// bounded damage source a [`VertexRebuild::Bounded`] outcome may carry,
/// falling back to `empty_fallback` when no bound can be established (Task
/// 124.14, extended by 124.14d).
///
/// Nine sources are unioned into one row set before any rect is built:
///
/// - `changed_rows` (Task 124.14a) -- the window rows named by
///   [`super::frame_dirty::ChangedRows::Rows`], translated snapshot ->
///   rendered -> screen.
/// - `spans.current_selection` / `spans.previous_selection` (Task
///   124.14b-i) -- the selection highlight's current and previous frame
///   extent, both already in screen-row space. Erasing the *old* highlight
///   matters as much as drawing the *new* one: a selection that shrank or
///   was cleared still left pixels on screen that only these rows will
///   repaint.
/// - `spans.current_hover` / `spans.previous_hover` (Task 124.14b-ii) --
///   the command-block hover tint's current and previous frame extent,
///   also both already in screen-row space, for the same
///   erase-the-old/draw-the-new reason as selection: moving the hover
///   between two blocks (or off a block entirely) must repaint the
///   previously-tinted rows or the stale tint lingers with no later event
///   to correct it.
/// - `spans.current_search_rows` / `spans.previous_search_rows` (Task
///   124.14d) -- the search-match tint's current and previous frame rows,
///   for the same erase-the-old/draw-the-new reason as selection and
///   hover.
/// - `spans.current_cursor_row` / `spans.previous_cursor_row` -- the cursor's
///   current and previous screen rows when cursor state changed. Both are
///   needed to draw the new cursor and erase the old one.
///
/// `evaluate_frame_dirty_state` selects [`VertexRebuild::Bounded`] only when
/// `changed_rows` is [`super::frame_dirty::ChangedRows::Rows`] or
/// [`super::frame_dirty::ChangedRows::None`] — never
/// [`super::frame_dirty::ChangedRows::All`] — but the `All` arm below falls
/// back to `Full` (unconditionally, not `empty_fallback` -- an unbounded
/// input is not the same shape as an empty one) rather than panicking on
/// that unreachable-in-practice shape, per the panic-free-production rule.
///
/// Snapshot rows that map to nothing (collapsed inside a fold) contribute no
/// rect via `row_map.snapshot_to_rendered` / `layout.rendered_to_screen`
/// returning `None`. If every source's row does — the whole change is
/// hidden behind folds — the result is `empty_fallback`, not an empty
/// `Region`: an empty region would be a third way of spelling "nothing
/// changed", and `decide_frame_damage` already carries one dead
/// `RequestedWithNoRects` variant from the last time that shape existed
/// (124.21 finding 4). This is the ONLY place `empty_fallback` is read —
/// once `screen_rows` (below) is non-empty, every subsequent fallback (rows
/// that survived the union but produced no convertible rect) stays `Full`
/// unconditionally, per that field's own doc comment.
///
/// All nine sources are merged into one sorted, deduplicated row set
/// **before** run-merging runs, so overlapping sources (e.g. a changed row
/// that sits inside the selection, or a hover span that overlaps either)
/// collapse into one rect rather than several overlapping ones. Contiguous
/// on-screen rows are merged into a single rect (a run) rather than
/// emitted one rect per row, so a block of adjacent rows costs one
/// [`crate::gui::renderer::PaneDamageRect`] rather than many — but
/// non-contiguous rows (e.g. rows 3 and 40) stay separate rects rather
/// than collapsing to their bounding box, which is the entire reason
/// `PaneFrameDamage::Region` carries a `Vec` instead of one rect (124.14a's
/// design decision: a single bbox would present the rows between them
/// too).
fn build_bounded_damage(
    changed_rows: &super::frame_dirty::ChangedRows,
    spans: BoundedDamageSpans<'_>,
    row_map: &RowMap,
    layout: &FoldLayout,
    geometry: RowDamageGeometry,
    empty_fallback: EmptyBoundedDamage,
) -> crate::gui::renderer::PaneFrameDamage {
    let mut screen_rows: Vec<usize> = match changed_rows {
        super::frame_dirty::ChangedRows::Rows(rows) => rows
            .iter()
            .filter_map(|&snap_row| {
                let rendered = row_map.snapshot_to_rendered(snap_row)?;
                layout.rendered_to_screen(rendered)
            })
            .collect(),
        super::frame_dirty::ChangedRows::None => Vec::new(),
        super::frame_dirty::ChangedRows::All => {
            return crate::gui::renderer::PaneFrameDamage::Full;
        }
    };

    if let Some((start, end)) = spans.current_selection {
        screen_rows.extend(start..=end);
    }
    if let Some((start, end)) = spans.previous_selection {
        screen_rows.extend(start..=end);
    }
    if let Some((start, end)) = spans.current_hover {
        screen_rows.extend(start..=end);
    }
    if let Some((start, end)) = spans.previous_hover {
        screen_rows.extend(start..=end);
    }
    screen_rows.extend_from_slice(spans.current_search_rows);
    screen_rows.extend_from_slice(spans.previous_search_rows);
    if let Some(row) = spans.current_cursor_row {
        screen_rows.push(row);
    }
    if let Some(row) = spans.previous_cursor_row {
        screen_rows.push(row);
    }

    // Merge all nine sources into one sorted, deduplicated set BEFORE
    // run-merging, so a row contributed by more than one source (e.g. a
    // changed row inside the selection, or a hover span overlapping either)
    // collapses to a single entry rather than emitting overlapping rects
    // for it.
    screen_rows.sort_unstable();
    screen_rows.dedup();

    let Some(&first) = screen_rows.first() else {
        return match empty_fallback {
            EmptyBoundedDamage::Full => crate::gui::renderer::PaneFrameDamage::Full,
            EmptyBoundedDamage::Unchanged => crate::gui::renderer::PaneFrameDamage::Unchanged,
        };
    };

    let mut rects = Vec::new();
    let mut run_start = first;
    let mut run_end = first;
    for &row in &screen_rows[1..] {
        // A gap closes the run in progress and starts a new one; a
        // contiguous row simply extends it. Either way `row` becomes the
        // run's new end.
        if row != run_end + 1 {
            rects.extend(row_run_damage(run_start, run_end, geometry));
            run_start = row;
        }
        run_end = row;
    }
    rects.extend(row_run_damage(run_start, run_end, geometry));

    if rects.is_empty() {
        // Deliberately unconditional `Full`, not `empty_fallback`: reaching
        // here means `screen_rows` named real rows (the check above already
        // returned for the truly-empty case) that then ALL collapsed inside
        // a fold. That is "cannot bound this", the same shape `empty_fallback
        // == Full` documents for selection/hover/row sources, not "search
        // changed nothing" -- so it must not be read as `Unchanged` even
        // when `empty_fallback` is `Unchanged`.
        crate::gui::renderer::PaneFrameDamage::Full
    } else {
        crate::gui::renderer::PaneFrameDamage::Region(rects)
    }
}

/// The egui widget that owns and drives the terminal render pipeline.
///
/// `FreminalTerminalWidget` holds shared resources that are common across all
/// panes: the [`FontManager`] (font metrics, shaping config) and global
/// config state (ligatures, cursor trail).
///
/// Per-pane GPU state (`RenderState`) and render cache state
/// (`PaneRenderCache`, including dirty tracking and per-line shaped glyph
/// runs) live on each [`Pane`](super::super::panes::Pane) instance. On each
/// call to [`show`](Self::show), the widget:
///
/// 1. Detects content changes via `Arc` pointer comparison (per-pane cache).
/// 2. Re-shapes only dirty lines using the pane's shaping cache.
/// 3. Rebuilds GPU vertex buffers in the pane's `RenderState`.
/// 4. Submits a `PaintCallback` to egui that executes the GL draw calls.
/// 5. Processes keyboard, mouse, scroll, and focus input and forwards them
///    to the PTY thread via `input_tx`.
pub struct FreminalTerminalWidget {
    /// Shared font manager — metrics, rasterisation, fallback chain.
    pub(super) font_manager: FontManager,
    /// Hot-reloadable single-bool feature toggles cached from config,
    /// grouped into their own type to keep this struct's own field count
    /// under clippy's `struct_excessive_bools` threshold (a fourth
    /// independent bool -- `progress_enabled`, issue #507 phase B2 -- would
    /// otherwise trip it). See [`WidgetDisplayToggles`]'s own doc for why
    /// each field is still correctly a `bool`, not an enum.
    toggles: WidgetDisplayToggles,
    /// Duration of the cursor trail animation.
    cursor_trail_duration: Duration,
    /// How a cursor is drawn when its pane or window is not focused (config
    /// `[cursor] unfocused_style`). Cached like `cursor_trail_duration` so
    /// `show()` reads it each frame without touching the config.
    unfocused_cursor_style: UnfocusedCursorStyle,
    /// The base egui `FontDefinitions` (without any preview font registered).
    /// Captured at construction and updated on `apply_config_changes`. Used by
    /// the settings modal to register a temporary preview font without losing
    /// the original font set.
    base_font_defs: egui::FontDefinitions,
    /// Set by `apply_config_changes_no_ctx` when the font family or size
    /// changed but no egui context was available to register the new fonts.
    /// Cleared on the next frame when the terminal window calls
    /// `flush_egui_fonts_if_dirty`.
    egui_fonts_dirty: bool,
}

/// Hot-reloadable single-bool feature toggles [`FreminalTerminalWidget`]
/// caches from config and reads every frame in `show()`.
///
/// Each field is independently a TOML config toggle -- one of the three
/// cases `freminal-state-representation` names as still correctly a
/// `bool` rather than an enum -- so grouping them here is purely to keep
/// [`FreminalTerminalWidget`]'s own field count under clippy's
/// `struct_excessive_bools` limit; it carries no combined meaning of its
/// own (unlike, say, `PaneRenderCache`'s bools, which are genuinely
/// per-pane dirty-tracking flags and are grouped under an explicit
/// `#[allow(clippy::struct_excessive_bools)]` instead).
#[derive(Debug, Clone, Copy)]
struct WidgetDisplayToggles {
    /// Whether OpenType ligatures are enabled for text shaping.
    ligatures: bool,
    /// Whether cursor trail animation is enabled (cursor glides to new position).
    cursor_trail: bool,
    /// Whether the per-pane OSC 9;4 progress indicator is drawn
    /// (`config.progress.enabled`, issue #507 phase B2). Gates display
    /// only -- recognition/parsing of OSC 9;4 always happens regardless of
    /// this flag (see [`freminal_common::config::ProgressConfig`]'s doc).
    progress_enabled: bool,
}

/// Compute a pane's terminal-rect origin: the top-left corner of the cell
/// grid, after the command-block gutter (if any) shifts it right by
/// `gutter_inset` logical points off `pane_rect`'s left edge.
///
/// This is a small pure extraction of the first component of the
/// `terminal_rect` computation inside [`FreminalTerminalWidget::show`] (see
/// the call site there), done **solely** so a unit test can exercise the
/// exact computation `show` uses without driving a full `show()` call
/// (which needs a live `Ui`, GPU-backed `RenderState`, and PTY channels —
/// not practical to construct in a unit test). `show` and the test below
/// are the only two callers; subtask 122.15 forbids any *third* call site
/// re-deriving this from `cached_central_rect` + a gutter inset
/// independently — see `PublishedFrameState::pane_terminal_origin`'s doc.
fn terminal_rect_origin(
    pane_rect: egui::Rect,
    gutter_inset: f32,
) -> freminal_common::geometry::Point {
    freminal_common::geometry::point(pane_rect.min.x + gutter_inset, pane_rect.min.y)
}

impl FreminalTerminalWidget {
    /// Create a new `FreminalTerminalWidget`, loading fonts and initialising
    /// shared rendering resources from the provided config.
    ///
    /// # Errors
    ///
    /// Propagates any [`crate::gui::font_manager::FontManagerError`] from
    /// [`FontManager::new`].  Such errors indicate build-time packaging or
    /// memory corruption issues and should be treated as fatal by the binary
    /// (e.g. log and exit from `main()`).
    pub fn new(
        ctx: &Context,
        config: &Config,
    ) -> Result<Self, crate::gui::font_manager::FontManagerError> {
        let font_config = FontConfig {
            size: config.font.size,
            user_font: config.font.family.clone(),
            ..FontConfig::default()
        };
        let base_font_defs = setup_font_files(ctx, &font_config);

        let pixels_per_point = ctx.pixels_per_point();

        Ok(Self {
            font_manager: FontManager::new(config, pixels_per_point)?,
            toggles: WidgetDisplayToggles {
                ligatures: config.font.ligatures,
                cursor_trail: config.cursor.trail,
                progress_enabled: config.progress.enabled,
            },
            cursor_trail_duration: Duration::from_millis(u64::from(
                config.cursor.trail_duration_ms,
            )),
            unfocused_cursor_style: config.cursor.unfocused_style,
            base_font_defs,
            egui_fonts_dirty: false,
        })
    }

    /// Returns the authoritative cell size in integer pixels `(width, height)`.
    ///
    /// Computed once from swash font metrics and updated on font change.
    #[must_use]
    pub const fn cell_size(&self) -> (u32, u32) {
        self.font_manager.cell_size()
    }

    /// Return a sorted, deduplicated list of all monospaced font family names
    /// installed on the system.  Delegates to [`FontManager::enumerate_monospace_families`].
    #[must_use]
    pub fn monospace_families(&self) -> Vec<String> {
        self.font_manager.enumerate_monospace_families()
    }

    /// Load the raw font file bytes for a system font family name.
    /// Delegates to [`FontManager::load_font_bytes_for_family`].
    #[must_use]
    pub fn load_font_bytes(&self, family: &str) -> Option<Vec<u8>> {
        self.font_manager.load_font_bytes_for_family(family)
    }

    /// Mutable access to this window's shared `FontManager`, for main-thread
    /// shaping/measuring outside the terminal grid (e.g. the toast overlay,
    /// issue #433). Shaping must never happen inside a `PaintCallback`
    /// (`FontManager` is `!Sync`); this is called on the GUI thread.
    pub(crate) const fn font_manager_mut(&mut self) -> &mut FontManager {
        &mut self.font_manager
    }

    /// If the egui chrome fonts were marked dirty by a no-ctx config change,
    /// re-register them now with the provided context and clear the flag.
    pub fn flush_egui_fonts_if_dirty(&mut self, ctx: &egui::Context, config: &Config) {
        if self.egui_fonts_dirty {
            self.egui_fonts_dirty = false;
            let new_font_config = FontConfig {
                size: config.font.size,
                user_font: config.font.family.clone(),
                ..FontConfig::default()
            };
            self.base_font_defs = setup_font_files(ctx, &new_font_config);
        }
    }

    /// Return a reference to the base egui `FontDefinitions` (without any
    /// preview font). Used by the settings modal to register a temporary
    /// preview font.
    #[must_use]
    pub const fn base_font_defs(&self) -> &egui::FontDefinitions {
        &self.base_font_defs
    }

    /// Synchronise the font manager's `pixels_per_point` with the current
    /// display scale factor.  If the value changed (e.g. the window moved to a
    /// monitor with a different DPI), cell metrics are recomputed and the
    /// shared shaping cache is invalidated.
    ///
    /// Returns `true` if the scale factor changed. When this returns `true`
    /// the caller must clear each pane's `RenderState::atlas` and
    /// `PaneRenderCache::invalidate_content()` so that all panes force a
    /// full vertex rebuild on the next frame.
    ///
    /// **Must be called before [`Self::cell_size`] each frame** so that resize
    /// calculations in `FreminalGui::ui()` use up-to-date metrics.
    pub fn sync_pixels_per_point(&mut self, ppp: f32) -> bool {
        self.font_manager
            .update_pixels_per_point(ppp)
            .unwrap_or_else(|e| {
                error!("fatal: font manager could not recompute metrics for pixels_per_point change: {e}");
                std::process::exit(1);
            })
    }

    /// Render the terminal for one egui frame and process all pending input.
    ///
    /// - `snap` — the latest terminal snapshot from the PTY thread (lock-free).
    /// - `view_state` — GUI-local scroll, selection, blink, and focus state.
    /// - `render_state` — per-pane GPU resources (renderer, atlas, vertex buffers).
    /// - `cache` — per-pane dirty-tracking cache for incremental rendering.
    /// - `input_tx` — channel to send keyboard/resize/focus events to the PTY.
    /// - `clipboard_rx` — receives clipboard content from the PTY write-back.
    /// - `search_buffer_rx` — receives full-buffer search content from the PTY thread.
    /// - `ui_overlay_open` — suppresses terminal input while a modal or menu dropdown is visible.
    /// - `border_drag_active` — suppresses terminal input and clears any
    ///   phantom selection while a pane-border drag-to-resize is in
    ///   progress. The invisible drag-sensor rect geometrically overlaps
    ///   the adjacent pane's `terminal_rect`, so without this the same
    ///   press+drag would also start/extend a real text selection there
    ///   (issue #453).
    /// - `bg_opacity` — background panel opacity (`0.0`–`1.0`) from config.
    /// - `bg_image_opacity` — background image opacity (`0.0`–`1.0`) from config.
    /// - `bg_image_mode` — background image fit mode from config.
    /// - `binding_map` — user key-binding map; bound combos are intercepted before PTY dispatch.
    /// - `is_active_pane` — whether this pane is the active pane of its tab.
    /// - `window_focus` — whether the OS window has input focus; together
    ///   with `is_active_pane` it decides whether the cursor is drawn as a
    ///   focused (solid) or an unfocused (config-styled) cursor.
    /// - `key_broadcast_targets` — input senders of the other panes to mirror
    ///   keyboard input to when broadcast mode is active (Task 74); empty when
    ///   broadcast is off or this is not the active pane.
    ///
    /// Returns `(left_mouse_button_pressed, copied_to_clipboard, deferred_actions)`.
    /// The second `bool`, `copied_to_clipboard`, is `true` iff a non-empty
    /// local selection was copied to the system clipboard this frame.
    // Inherently large: the main per-frame terminal widget handler — processes input, handles
    // blink/scroll/mouse, and orchestrates layout. Each section is tightly coupled.
    #[allow(clippy::too_many_lines)]
    // All parameters are required: each pane needs its own render state, cache, channels, and
    // view state; there is no sensible grouping that reduces the count without hiding the intent.
    #[allow(clippy::too_many_arguments)]
    // Each bool gates an independent, unrelated suppression/state concern
    // (overlay-open, border-drag-active, echo-off, active-pane); bundling
    // them into an enum would not express any real combined state and
    // would only obscure the call site.
    #[allow(clippy::fn_params_excessive_bools)]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        snap: &TerminalSnapshot,
        view_state: &mut ViewState,
        render_state: &Arc<Mutex<RenderState>>,
        cache: &mut PaneRenderCache,
        input_tx: &Sender<InputEvent>,
        clipboard_rx: &Receiver<String>,
        search_buffer_rx: &Receiver<SearchCorpus>,
        ui_overlay_open: bool,
        border_drag_active: bool,
        bg_opacity: f32,
        bg_image_opacity: f32,
        bg_image_mode: freminal_common::config::BackgroundImageMode,
        command_blocks_config: &freminal_common::config::CommandBlocksConfig,
        gutter_inset_logical: f32,
        binding_map: &freminal_common::keybindings::BindingMap,
        is_echo_off: bool,
        is_active_pane: bool,
        window_focus: WindowFocus,
        pane_id: crate::gui::panes::PaneId,
        recording_ctx: Option<&freminal_terminal_emulator::recording::RecordingContext<'_>>,
        pending_copy: &mut bool,
        key_broadcast_targets: &[Sender<InputEvent>],
        present_region: &Arc<Mutex<freminal_windowing::PresentRegion>>,
        split_border_hover: SplitBorderHover,
    ) -> (bool, bool, Vec<freminal_common::keybindings::KeyAction>) {
        // `sync_pixels_per_point()` has already been called by
        // `FreminalGui::ui()` before this method, so font metrics are
        // up-to-date.  We just read `ppp` for logical-pixel conversions.
        let ppp = ui.ctx().pixels_per_point();

        let (cell_w, cell_h) = self.font_manager.cell_size();
        // Physical pixel dimensions (for vertex building / OpenGL renderer).
        let cell_w_f = f32::approx_from(cell_w).unwrap_or(0.0);
        let row_h_f = f32::approx_from(cell_h).unwrap_or(0.0);

        // Logical point dimensions (for egui layout, mouse hit-testing, scroll).
        let logical_cell_w = cell_w_f / ppp;
        let logical_cell_h = row_h_f / ppp;

        // Suppress input for one extra frame after a modal closes, or after
        // a pane-border drag-to-resize ends. This prevents the dismiss-click
        // (Cancel / X / click-away) or the drag-release-click from leaking
        // through to the terminal as a pointer event (issue #453 review).
        let suppress_input = ui_overlay_open
            || border_drag_active
            || cache.overlay_was_open_last_frame
            || cache.border_drag_was_active_last_frame;
        cache.overlay_was_open_last_frame = ui_overlay_open;
        cache.border_drag_was_active_last_frame = border_drag_active;

        // Claim the full available space.
        let available = ui.available_size();
        ui.set_min_size(available);

        // Claim keyboard focus for the terminal area so egui does not use
        // Tab / arrow keys for its own widget-focus cycling.  This is a
        // terminal emulator — ALL keyboard input belongs to the PTY.
        //
        // When the settings modal is open (or was open last frame) we
        // release focus so that Tab and arrow keys work normally inside the
        // modal's egui widgets, and so the dismiss-click is not forwarded.
        //
        // Also release focus when the right-click context menu or the search
        // overlay is open so that egui can deliver events to those widgets.
        let context_menu_open = view_state.context_menu_pos.is_some();
        let search_open = view_state.search_state.is_open;
        let command_history_open = view_state.command_history.is_open;
        if !suppress_input
            && !context_menu_open
            && !search_open
            && !command_history_open
            && is_active_pane
        {
            let terminal_id = ui.id().with("terminal_focus");
            let focus_rect = ui.available_rect_before_wrap();
            let response = ui.interact(
                focus_rect,
                terminal_id,
                egui::Sense::focusable_noninteractive(),
            );
            if !response.has_focus() {
                response.request_focus();
            }
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    terminal_id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
        }

        // Compute the terminal area rect BEFORE processing input events.
        // Pointer events from `input.raw.events` are in window coordinates,
        // so `encode_egui_mouse_pos_as_usize` must subtract the rect's min
        // corner to get terminal-grid-relative coordinates.  The full rect
        // is also used to reject pointer events outside the terminal area
        // (e.g. clicks on the tab bar).
        //
        // The command-block gutter (if enabled) reserves `gutter_inset_logical`
        // points on the LEFT edge.  That total inset is the painted strip
        // width PLUS a padding gap, so `terminal_rect` is shifted right by the
        // whole inset (keeping the cell grid, mouse hit-testing — which
        // subtracts `terminal_rect.min` — and the PTY column count in agreement;
        // `app_impl` computes the column count from the identical inset).  The
        // painted `gutter_rect` is only the strip width; the remaining padding
        // is left blank so glyphs are not flush against the status bar.
        let pane_rect = ui.available_rect_before_wrap();
        let gutter_inset = gutter_inset_logical.max(0.0);
        let gutter_strip_w = if gutter_inset > 0.0 {
            command_blocks_config.gutter.width_px() / ppp
        } else {
            0.0
        };
        let gutter_rect = egui::Rect::from_min_max(
            pane_rect.min,
            egui::pos2(pane_rect.min.x + gutter_strip_w, pane_rect.max.y),
        );
        let terminal_origin = terminal_rect_origin(pane_rect, gutter_inset);
        let terminal_rect = egui::Rect::from_min_max(
            crate::gui::geometry_interop::point_to_egui(terminal_origin),
            pane_rect.max,
        );

        // Keep the gutter hover-tint live.  This works together with the
        // `hover_changed` cache invalidation below; both are required:
        //
        //   1. WAKING A FRAME on cursor motion.  The windowing layer's
        //      cursor-move fast path (Task 65/68 idle-CPU optimization) only
        //      schedules a repaint when egui itself reports `repaint` — i.e.
        //      when an egui-tracked interactive region's hover state changes.
        //      Registering the gutter as a `Sense::click()` region makes egui
        //      report that on enter/leave, so the frame runs and the hover
        //      recompute happens.
        //   2. REBUILDING THE VBO (the `hover_changed` term, further below):
        //      the hover tint is baked into the background instance buffer,
        //      which is otherwise only rebuilt on content/selection/search
        //      changes.  Without `hover_changed` the woken frame would reuse
        //      stale vertices and show nothing.
        //
        // The click itself is still handled by the pre-check that follows; this
        // response is only used for the repaint wake-up and the hand cursor.
        // We also force one repaint on the frame the pointer leaves so the
        // clearing frame is guaranteed.
        let mut gutter_hovered = false;
        if gutter_inset > 0.0 && command_blocks_config.enabled && !snap.is_alternate_screen {
            let gutter_hit_rect = egui::Rect::from_min_max(
                pane_rect.min,
                egui::pos2(terminal_rect.min.x, pane_rect.max.y),
            );
            let gutter_response = ui.interact(
                gutter_hit_rect,
                ui.id().with(("command_block_gutter", pane_id)),
                egui::Sense::click(),
            );
            let hovered = gutter_response.hovered();
            // See `gutter_hover_repaint_decision` for why `latest_pos()` is
            // folded in alongside `hovered()` here: it detects a
            // pointer-left-the-window exit in the same frame it happens,
            // instead of one frame late (egui's documented `interact_pos()`
            // lag around `Event::PointerGone`).
            let pointer_in_window = ui.ctx().input(|i| i.pointer.latest_pos().is_some());
            let (needs_repaint, effectively_hovered) = gutter_hover_repaint_decision(
                hovered,
                pointer_in_window,
                cache.pointer_in_gutter_last_frame,
            );
            // Recorded rather than applied here: every cursor-icon write in
            // this frame is resolved once, at the end of `show`, by
            // `PointerHover`. Setting it at this point had no visible effect
            // at all, because the URL / OSC-22 block below unconditionally
            // overwrote `output.cursor_icon` a few hundred lines later
            // (issue #462).
            gutter_hovered = effectively_hovered;
            if needs_repaint {
                // 16ms, not `Duration::ZERO` (subtask 121.12): scheduling is
                // unchanged either way (`clamp_repaint_delay` already floors
                // any delay, including a bare `request_repaint()`, at
                // `MIN_REPAINT_INTERVAL` = 16ms), so this value is
                // scheduling-equivalent to zero.
                cache.request_repaint_after(std::time::Duration::from_millis(16));
            }
            cache.pointer_in_gutter_last_frame = effectively_hovered;
        } else if cache.pointer_in_gutter_last_frame {
            // Feature toggled off / alt-screen entered while we were hovering:
            // draw one clearing frame. See the comment above for why 16ms,
            // not zero, is used here (settle-check discriminating power, not
            // this frame's outcome).
            cache.request_repaint_after(std::time::Duration::from_millis(16));
            cache.pointer_in_gutter_last_frame = false;
        }

        // ── Scrollbar pre-check ──────────────────────────────────────────
        // Detect if the user is clicking or starting a drag on the scrollbar
        // BEFORE processing terminal input, so the click is not forwarded
        // to the PTY as a terminal mouse event.
        {
            let scrollbar_hit = ui.input(|i| {
                let ptr = &i.pointer;
                if !ptr.primary_pressed() {
                    return false;
                }
                // Task 124.3b: the shared `scrollbar_hit_rect` helper is now
                // the single source of truth for this geometry — no more
                // hand-copied constants to keep in sync with
                // `handle_scrollbar`.
                ptr.interact_pos()
                    .is_some_and(|pos| scrollbar_hit_rect(ui.max_rect()).contains(pos))
            });
            if scrollbar_hit && snap.scroll_offset > 0 {
                cache.scrollbar_dragging = true;
            }
        }

        // ── Command-block gutter pre-check ────────────────────────────────
        // Intercept primary clicks that land in the reserved gutter inset
        // (left of `terminal_rect`) BEFORE they reach `write_input_to_terminal`
        // — gutter positions are outside `terminal_rect`, so they would
        // otherwise be dropped entirely (no fold, no focus).  A click on a
        // FINISHED block toggles its fold; a click on a RUNNING block is a
        // no-op fold but still focuses the pane.  Hovering the gutter is
        // handled later (it feeds the same hover-tint overlay as the cell
        // grid).  Suppressed on the alternate screen.
        let mut left_mouse_button_pressed_gutter = false;
        if gutter_inset > 0.0
            && command_blocks_config.enabled
            && !snap.is_alternate_screen
            && !snap.command_blocks.is_empty()
            && !suppress_input
            && !context_menu_open
            && !view_state.search_state.is_open
            && !view_state.command_history.is_open
        {
            let gutter_press_pos = ui.input(|i| {
                let ptr = &i.pointer;
                if ptr.primary_pressed() {
                    ptr.interact_pos()
                } else {
                    None
                }
            });
            // The hit zone is the whole inset region [pane left, terminal
            // left), i.e. the painted strip plus the padding gap — a more
            // forgiving target than the 4px strip alone.
            if let Some(pos) = gutter_press_pos
                && pos.x >= pane_rect.min.x
                && pos.x < terminal_rect.min.x
                && pos.y >= terminal_rect.min.y
                && pos.y < terminal_rect.max.y
                && let Some(block_id) =
                    gutter_block_id_at_pos(pos, snap, view_state, terminal_rect, logical_cell_h)
            {
                // Focus the pane regardless of fold outcome.
                left_mouse_button_pressed_gutter = true;
                // Only finished blocks can fold (running blocks have no
                // `end_row`).
                if let Some(block) = snap.command_blocks.iter().find(|b| b.id == block_id)
                    && crate::gui::command_blocks::block_is_foldable(block)
                {
                    view_state.toggle_fold(block_id);
                    super::input::resend_scroll_window(snap, view_state, input_tx);
                }
            }
        }

        // When a modal dialog (e.g. the settings window) or the right-click
        // context menu is open — or the modal was open last frame — do NOT
        // forward keyboard/mouse events to the PTY.  For modals, the one-frame
        // delay prevents the dismiss-click from leaking through as a pointer
        // event.  For the context menu, suppression ensures that clicking a
        // menu button (e.g. Copy) is delivered to egui's Area widget instead
        // of being consumed by `write_input_to_terminal` as a terminal click.
        let mut deferred_actions = Vec::new();
        // A gutter click never reaches `write_input_to_terminal` (it is outside
        // `terminal_rect`), so its click-to-focus intent is carried here.
        let mut left_mouse_button_pressed = left_mouse_button_pressed_gutter;
        // Set to `true` below iff a non-empty local selection is actually
        // copied to the system clipboard this frame (Subtask D3).
        let mut copied_to_clipboard = false;
        let pane_focus_now = if is_active_pane {
            PaneFocus::Active
        } else {
            PaneFocus::Inactive
        };
        let suppressors = InputSuppressors {
            modal_or_drag: suppress_input,
            context_menu: context_menu_open,
            search_overlay: view_state.search_state.is_open,
            command_history: view_state.command_history.is_open,
            scrollbar_drag: cache.scrollbar_dragging,
        };

        if suppressors.any() {
            let request_scroll_repaint = if suppressors.scroll_passes_through(pane_focus_now) {
                let result = ui.input(|input_state| {
                    scroll_overlay_passthrough(
                        input_state,
                        snap,
                        input_tx,
                        view_state,
                        logical_cell_h,
                        cache.previous_scroll_amount,
                    )
                });
                cache.previous_scroll_amount = result.carry;
                result.scrolled
            } else {
                cache.previous_scroll_amount = 0.0;
                false
            };
            // Must be outside the `ui.input` closure above, which holds a read
            // lock on the egui context.
            if request_scroll_repaint {
                ui.ctx().request_repaint();
            }

            cache.previous_key = None;
            cache.previous_mouse_state = None;
            if border_drag_active {
                // A pane-border drag geometrically overlaps the adjacent
                // pane's `terminal_rect`; the same press+drag would
                // otherwise start and extend a phantom text selection
                // here (issue #453). Fully clear it rather than
                // finalize-and-keep, since the gesture was a resize, not
                // a selection.
                view_state.selection.clear();
            } else {
                // Task 116.3 (defect 3): route through `finalize_interrupted_drag`
                // instead of bluntly clearing `is_selecting`. An in-progress drag
                // interrupted here (modal/menu/search/command-history opening,
                // scrollbar-drag starting, or a split-pane-boundary release that
                // never reaches this pane's `write_input_to_terminal` call) would
                // otherwise strand `anchor`/`end` with `is_selecting = false`,
                // making the next primary press see a stale `has_selection()`
                // and clear instead of starting a new drag. Finalizing collapses
                // a not-yet-dragged point selection (clears everything) or keeps
                // a real range as a completed selection, matching what a normal
                // mouse-release would have done.
                view_state.selection.finalize_interrupted_drag();
            }
        } else {
            let repeat_characters = snap.repeat_keys;
            let ctx = ui.ctx().clone();
            let result = ui.input(|input_state| {
                write_input_to_terminal(WriteInputParams {
                    input: input_state,
                    snap,
                    input_tx,
                    view_state,
                    character_size_x: logical_cell_w,
                    character_size_y: logical_cell_h,
                    pixels_per_point: ppp,
                    terminal_rect,
                    repeat_characters,
                    binding_map,
                    pane_focus: pane_focus_now,
                    recording_ctx,
                    placeholder_rects: &cache.placeholder_hit_rects,
                    key_broadcast_targets,
                    carry: InputCarryState {
                        last_reported_mouse_pos: cache.previous_mouse_state.clone(),
                        previous_key: cache.previous_key,
                        scroll_amount: cache.previous_scroll_amount,
                        super_state: cache.super_state,
                    },
                })
            });
            left_mouse_button_pressed |= result.left_mouse_button_pressed;
            cache.previous_mouse_state = result.carry.last_reported_mouse_pos;
            cache.previous_key = result.carry.previous_key;
            cache.previous_scroll_amount = result.carry.scroll_amount;
            cache.super_state = result.carry.super_state;
            let clipboard_pending = result.clipboard_pending;
            deferred_actions = result.deferred_actions;

            // Perform the clipboard copy OUTSIDE the ui.input() closure.
            // copy_text() calls ctx.output_mut() which needs a write lock on
            // the Context, but ui.input() holds a read lock — calling
            // copy_text() inside the closure would deadlock.
            //
            // If we sent an ExtractSelection request, wait briefly for the
            // PTY thread to respond with the extracted text.  Either the
            // in-widget keybinding path (`clipboard_pending`) or an external
            // trigger such as the Edit menu (`pending_copy`) can request
            // this round-trip.
            let copy_requested = clipboard_pending || *pending_copy;
            *pending_copy = false;
            if copy_requested
                && let Ok(text) = clipboard_rx.recv_timeout(std::time::Duration::from_millis(100))
                && !text.is_empty()
            {
                ctx.copy_text(text);
                copied_to_clipboard = true;
                // Clear the selection highlight now that the text has been
                // copied to the clipboard.
                view_state.selection.clear();
            }
        }

        // Blink state must be computed here — cannot call `ui.input` inside
        // the `Arc<CallbackFn>` closure (it must be `Send + Sync`).
        let time = ui.input(|i| i.time);

        // The blink phase is derived relative to the activation anchor (if
        // set), else the global wall clock. The anchor is (re)set by the GUI
        // at the single point where the active pane or active tab changes (see
        // `reset_blink_anchor_on_activation` in `app_impl`), so a
        // freshly-activated OR freshly-revealed pane starts in the visible
        // ("on") half regardless of the global cycle — no cursor-appear lag on
        // pane switch or tab switch. The anchor is captured lazily on the
        // first render after activation, when a valid `time` is available.
        //
        // A focus regain re-anchors the blink exactly as an activation reset
        // does, so the regained (now solid) cursor starts visible. The whole
        // derivation (focus, appearance, anchor, raw phase) is
        // `frame_cursor_state`, which is unit-tested.
        let frame_cursor = frame_cursor_state(&CursorStateInputs {
            pane_focus: pane_focus_now,
            window_focus,
            previous_focus: cache.previous_cursor_focus,
            activation_reset: ActivationBlinkReset::from_bool(
                view_state.cursor_blink_reset_pending,
            ),
            snapshot_visible: CursorVisibility::from_bool(snap.show_cursor),
            echo: EchoState::from_echo_off(is_echo_off),
            style: &snap.cursor_visual_style,
            unfocused_style: self.unfocused_cursor_style,
            time,
            anchor: view_state.cursor_blink_anchor,
        });
        view_state.cursor_blink_anchor = frame_cursor.anchor;
        // `frame_cursor_state` consumes any pending activation reset.
        view_state.cursor_blink_reset_pending = false;
        let cursor_focus_now = frame_cursor.focus;
        let raw_cursor_blink = frame_cursor.raw_blink;

        // Search: request the full buffer from the PTY thread when needed,
        // then run (or re-run) the search against the cached corpus.
        let search_error: Option<String> = if view_state.search_state.is_open {
            // Corpus exchange. `step_corpus` accepts whatever the PTY thread
            // has sent (matches are stable row numbers, so a slightly old
            // corpus is correct for the rows it holds) and asks for a fresh one
            // only when the buffer extent has moved on and the last request is
            // old enough. At scrollback capacity the extent advances with every
            // evicted row, so a reply that must equal the live extent is never
            // accepted (Task 125 review S1).
            let reply = search_buffer_rx.try_iter().last();
            if view_state
                .search_state
                .step_corpus(reply, snap.extent(), time)
                == crate::gui::view_state::CorpusRequest::Send
                && let Err(e) = input_tx.send(InputEvent::RequestSearchBuffer)
            {
                error!("Failed to request search buffer from PTY: {e}");
                view_state.search_state.corpus_request_failed();
            }

            // Run search if query/mode changed or we just got a new buffer.
            if view_state.search_state.needs_refresh() {
                if let (Some(buffer), Some(extent)) = (
                    view_state.search_state.cached_full_buffer.as_ref(),
                    view_state.search_state.last_known_extent,
                ) {
                    let query = view_state.search_state.query.clone();
                    let regex_mode = view_state.search_state.regex_mode;
                    let case_sensitive = view_state.search_state.case_sensitive;
                    // Corpus row `i` is buffer row `extent.row_base + i`.
                    let (found, err) =
                        run_search(&query, regex_mode, case_sensitive, buffer, extent.row_base);
                    view_state.search_state.replace_matches(found);
                    err
                } else {
                    // No corpus yet: `step_corpus` has already requested one.
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // Cursor-only state captured before the PaintCallback closure (which
        // requires `Send + Sync + 'static`).  `is_cursor_only` is moved into
        // the closure below. The decoration data itself (including the
        // patched-in cursor quad) lives in `RenderState::deco_verts` and is
        // read fresh by the closure — it is not captured here, so it always
        // reflects whatever was last written to it.
        let mut is_cursor_only = false;

        // How the cursor is drawn. Hidden when the terminal has hidden it
        // (DECTCEM ?25l) or a password prompt is active (the echo-off lock
        // icon replaces it); otherwise solid when focused, and styled by
        // `[cursor] unfocused_style` when the pane or window is not focused.
        let mut cursor_appearance = frame_cursor.appearance;

        // ── Command-block folding (Task 72.10b) ─────────────────────────────
        //
        // Compute the per-frame fold-range list from the snapshot's
        // `command_blocks` and the GUI-local `folded_blocks` set, then build
        // a `RowMap` that translates between snapshot-row space (what the
        // PTY/buffer produced) and rendered-row space (what we actually paint,
        // with folded ranges collapsed to single placeholder rows).
        //
        // For 72.10b-2, a folded range collapses to a *blank* row at its
        // placeholder slot — the placeholder visual (line count, triangle
        // glyph) and click-to-unfold land in 72.10b-3.
        // `compute_fold_ranges` produces ranges in **buffer-absolute** row
        // space (because `CommandBlock` row fields are buffer-absolute).
        // `RowMap` works in **snapshot-row** space `[0, term_height)`.
        // Translate before constructing the map; otherwise ranges with
        // `start_row >= term_height` are silently dropped and the fold
        // becomes a visual no-op.
        // Fold-aware window layout for this frame: with command-block folds in
        // view the PTY flattens `window_extra_rows` extra rows ABOVE the normal
        // visible window so the screen can be filled after collapsing folds
        // (see `TerminalSnapshot::window_extra_rows`).  `FoldLayout` centralises
        // the buffer/snapshot/rendered/screen row mapping; the renderer paints
        // the bottom `term_height` rendered rows so the live bottom is pinned.
        // When no fold is in view (`window_extra_rows == 0`) `render_skip == 0`
        // and rendering is identical to the unfolded path.
        let layout = FoldLayout::new(snap, &view_state.folded_blocks);
        let flat_window_start = layout.flat_window_start;
        let render_skip = layout.render_skip;
        let row_map = &layout.row_map;
        // Per-frame epoch: a stable hash of the sorted, non-overlapping ranges
        // list (plus the bottom-anchor skip).  When the user folds or unfolds a
        // block — or scrolls such that the visible fold span changes — this
        // changes, and we use it below to invalidate the vertex cache (the
        // rendered row layout has shifted).
        let fold_epoch: u64 = {
            use std::hash::{Hash, Hasher};
            let mut h = rustc_hash::FxHasher::default();
            for r in row_map.ranges() {
                r.command_block_id.hash(&mut h);
                r.start_row.hash(&mut h);
                r.end_row.hash(&mut h);
            }
            render_skip.hash(&mut h);
            h.finish()
        };

        // Task 125.5: one live-profiling observation begins per `show()`
        // call, once the raw/resolved rebuild outcome for THIS frame is
        // known below. `LiveRenderProfile::start` is given that raw/resolved
        // classification directly and retains it internally, so only the
        // resulting token needs to be carried forward to the paint
        // callback -- see `LiveRenderProfile::complete`'s doc for why it
        // takes back only the token and uploads, not the classification a
        // second time.
        //
        // Stays `None` on a `skip_draw` frame and permanently on a
        // non-`frame-profiling` build. `skip_draw` is deliberately NOT
        // classified into an observation of its own: `evaluate_frame_dirty_state`
        // never runs on such a frame, so there is no `VertexRebuild`
        // decision to mirror into a `RawRebuildDecision` -- the pane's
        // `PaintCallback` (still registered unconditionally below) simply
        // redraws whatever GPU buffers already exist, which is not CPU
        // render work this profiler measures. This is the "explicitly
        // justified" alternative to starting a token for every `show()`
        // call: forcing a classification here would misrepresent a frame
        // where no rebuild decision was made at all. Whenever a token IS
        // started below, the very same `show()` call unconditionally
        // registers the matching `PaintCallback` a token could complete
        // (see the registration further down) -- so a started token is
        // never orphaned by construction; the only way it goes unpainted
        // is the genuine `FrameDamage::None` case `LiveRenderProfile`'s
        // module doc describes, which this state machine already handles.
        #[cfg(feature = "frame-profiling")]
        let mut profiling_token: Option<PaneFrameToken> = None;

        // The cursor as drawn this frame, recorded into the cache's baselines
        // once at the end of `show()`. `Skipped` on a `skip_draw` frame, where
        // no dirty evaluation (and no draw) ran and so every cursor baseline is
        // left describing the last frame that did draw.
        let cursor_frame = if snap.skip_draw {
            CursorFrame::Skipped
        } else {
            // See `evaluate_frame_dirty_state`'s doc for the full rationale
            // behind every flag and translation computed here; this call
            // site only destructures the result back into the same local
            // names the two branches below (and the post-branch animation
            // bookkeeping) already expect.
            let dirty = evaluate_frame_dirty_state(
                &FrameDirtyContext {
                    snap,
                    cache,
                    render_state,
                    layout: &layout,
                    fold_epoch,
                    command_blocks_config,
                },
                view_state,
                FrameDirtyGeometry {
                    pane_rect,
                    terminal_rect,
                    gutter_inset,
                    logical_cell_h,
                },
                CursorFrameInputs {
                    blink: raw_cursor_blink,
                    appearance: cursor_appearance.clone(),
                    focus: cursor_focus_now,
                    trail_enabled: self.toggles.cursor_trail,
                    trail_duration: self.cursor_trail_duration,
                },
            );
            let content_changed = dirty.observations.content_changed;
            let selection_changed = dirty.observations.selection_changed;
            let search_changed = dirty.observations.search_changed;
            let hover_changed = dirty.observations.hover_changed;
            let image_frame_changed = dirty.observations.image_frame_changed;
            let image_pixels_changed = dirty.observations.image_pixels_changed;
            let text_blink_changed = dirty.observations.text_blink_changed;
            let current_selection = dirty.current_selection;
            let screen_selection = dirty.screen_selection;
            let search_epoch = dirty.search_epoch;
            let command_block_hover_rows_early = dirty.command_block_hover_rows;
            let cursor_state_changed = dirty.cursor_state_changed;
            cursor_appearance = dirty.cursor_appearance;
            let cursor_x_scale = dirty.cursor_x_scale;
            // Draw params shared by the cursor-only and full-rebuild paths.
            // `col`/`row` are the trail-animated visual coordinates, and the
            // cursor's pixel origin (for the damage rect below) is derived
            // from these same params via `pixel_origin`. `appearance` is the
            // focus-resolved appearance with fold/off-screen visibility
            // already applied, and `blink_on` is the focus-pinned phase
            // `evaluate_frame_dirty_state` derived from the raw one.
            let cursor_draw = CursorDrawParams {
                appearance: cursor_appearance.clone(),
                col: view_state.cursor_visual_col,
                row: view_state.cursor_visual_row,
                color: cursor_f(snap.theme, snap.cursor_color_override),
                x_scale: cursor_x_scale,
                blink_on: dirty.cursor_blink_phase,
            };
            let cursor_animating = dirty.cursor_animating;
            let anim_tick = dirty.image_anim_tick;

            // Default: this pane rendered no change (the no-op reuse branch).
            // The cursor-only and full-rebuild branches below overwrite this
            // with the appropriate `PaneFrameDamage` (#435).
            cache.last_frame_cursor_damage = crate::gui::renderer::PaneFrameDamage::Unchanged;

            // Hoisted out of the cursor-only arm (124.14a activation recon):
            // the bounded-damage full-rebuild arm below needs the same
            // viewport-to-framebuffer-pixel conversion to build its
            // `PaneFrameDamage::Region` rects, and computing it twice risked
            // the two arms silently drifting apart on the exact conversion.
            // `viewport_framebuffer_geometry` is also reused, recomputed,
            // by the search-overlay popup-rect conversion further below
            // (Task 124.14d), which runs outside this `!snap.skip_draw`
            // block and so cannot see this local binding.
            let (vp_left_px, vp_top_px, fb_height_px) =
                viewport_framebuffer_geometry(ui, terminal_rect, ppp);

            // Task 124.21 finding 2 (the multi-pane fan-out): this pane's own
            // rect, so a full rebuild below can report bounded damage
            // instead of escalating the whole frame to `Full`. See
            // `full_pane_rebuild_damage_rect`'s doc comment for why.
            let pane_rect_damage = full_pane_rebuild_damage_rect(
                pane_rect,
                terminal_rect,
                ppp,
                vp_left_px,
                vp_top_px,
                fb_height_px,
            );

            // Whether -- and how -- this frame's full rebuild (if any) can
            // report bounded damage, decided BEFORE the (identical either
            // way) rebuild body runs below. `VertexRebuild::Bounded` means
            // "full rebuild, bounded *damage*" (Task 124.14): the rebuild
            // itself never changes shape or becomes partial -- bounding the
            // vertex upload itself is Task 125, gated on a fixed-stride
            // relayout that does not exist yet.
            let full_rebuild = match dirty.rebuild {
                VertexRebuild::CursorOnly => None,
                VertexRebuild::Bounded => Some(FullRebuildDamage::Bounded),
                VertexRebuild::ReevaluateFullRebuild => {
                    // The last clause is the "never built" signal (124.21's
                    // empty-prior-state category): no rebuild has ever stored
                    // a cursor range. NOT `deco_verts.is_empty()` -- see
                    // `RenderState::cursor_vert_range`.
                    if content_changed
                        || selection_changed
                        || text_blink_changed
                        || search_changed
                        || hover_changed
                        || image_frame_changed
                        || image_pixels_changed
                        || render_state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .cursor_vert_range
                            .is_none()
                    {
                        Some(FullRebuildDamage::Full)
                    } else {
                        None
                    }
                }
            };

            // Task 125.5: the raw `VertexRebuild` decision and its
            // resolved render-work class are both known now -- before
            // either candidate branch below runs -- so record them into a
            // fresh profile token here. `ChangedRows::bounded_row_count`
            // supplies `VertexRebuild::Bounded`'s row count (including the
            // `ChangedRows::None` zero case); `full_rebuild`'s `Some`/`None`
            // resolution supplies `ReevaluateFullRebuild`'s outcome.
            // Profiling reads these already-computed decisions; it does
            // not recompute or influence them.
            #[cfg(feature = "frame-profiling")]
            {
                let raw = match dirty.rebuild {
                    VertexRebuild::CursorOnly => RawRebuildDecision::CursorOnly,
                    VertexRebuild::Bounded => RawRebuildDecision::Bounded {
                        changed_row_count: dirty.changed_rows.bounded_row_count(),
                    },
                    VertexRebuild::ReevaluateFullRebuild => {
                        RawRebuildDecision::ReevaluateFullRebuild {
                            resolved: if matches!(full_rebuild, Some(FullRebuildDamage::Full)) {
                                ReevaluatedRebuild::Full
                            } else {
                                ReevaluatedRebuild::Reuse
                            },
                        }
                    }
                };
                let class = resolve_render_work_class(raw);
                let mut guard = render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let token = guard.live_profile.start(raw, class);
                // The stale-token sweep inside `start` above may have just
                // finalized a PREVIOUS token as unpainted and crossed a
                // `FLUSH_EVERY` boundary doing so -- check here, not only
                // from the paint callback's `complete` call site below, or
                // that boundary is lost forever (Task 125.5 regression
                // fix; see `LiveRenderProfile::take_flush_signal`'s doc).
                maybe_log_live_render_profile_flush(&mut guard.live_profile, pane_id);
                drop(guard);
                profiling_token = Some(token);
            }

            if matches!(dirty.rebuild, VertexRebuild::CursorOnly) {
                // Fast path: build just the cursor quad and stash it.
                let cursor_verts = build_cursor_verts_only(cell_w, cell_h, &cursor_draw);
                is_cursor_only = true;

                // Compute the frame-damage rect (#435): the region that
                // actually changed this frame, so the windowing layer can
                // skip the full clear and present only this rect. The changed
                // region is the union of the cursor's *previous* cell (whose
                // glyph is revealed when the cursor moves or blinks off) and
                // its *current* cell. Coordinates are physical framebuffer
                // pixels; `PaneDamageRect` handles the Y-flip to GL origin.
                let cell_w_px = cell_w_f * cursor_x_scale;
                // Current cursor cell, relative to the viewport top-left.
                let (cur_x, cur_y) = cursor_draw.pixel_origin(cell_w, cell_h);
                let mut damage_cells: Vec<(f32, f32, f32, f32)> =
                    vec![(cur_x, cur_y, cell_w_px, row_h_f)];
                // If the cursor moved since last frame, also damage the old
                // cell so the present covers the revealed glyph there.
                let prev = cache.previous_cursor_pos;
                if prev != snap.cursor_pos {
                    // The vacated cell's horizontal scale is the PREVIOUS
                    // row's line width, not the current row's — they can
                    // differ (DECDWL/DECDHL), and using the current row's
                    // scale would under-cover a revealed double-width glyph.
                    let prev_x_scale = snap
                        .visible_line_widths
                        .get(prev.y)
                        .copied()
                        .unwrap_or(freminal_terminal_emulator::LineWidth::Normal);
                    let prev_scale = if prev_x_scale.is_double_width() {
                        2.0
                    } else {
                        1.0
                    };
                    let prev_x = prev.x.approx_as::<f32>().unwrap_or(0.0) * cell_w_f * prev_scale;
                    let prev_y = prev.y.approx_as::<f32>().unwrap_or(0.0) * row_h_f;
                    let prev_cell_w = cell_w_f * prev_scale;
                    damage_cells.push((prev_x, prev_y, prev_cell_w, row_h_f));
                }
                let cursor_damage = crate::gui::renderer::PaneDamageRect::from_cursor_cells(
                    vp_left_px,
                    vp_top_px,
                    fb_height_px,
                    &damage_cells,
                );
                cache.last_frame_cursor_damage =
                    crate::gui::renderer::PaneFrameDamage::CursorOnly(cursor_damage);

                let mut rs = render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // We replace the cursor geometry in the CPU copy so that the
                // GPU upload sees the current cursor, and record its new
                // range for the next cursor-only patch.
                // (`None` cannot reach here -- see
                // `patch_cursor_only_deco_verts`.)
                let range = rs.cursor_vert_range;
                rs.cursor_vert_range = Some(patch_cursor_only_deco_verts(
                    &mut rs.deco_verts,
                    range,
                    &cursor_verts,
                ));
            } else if let Some(full_rebuild_damage) = full_rebuild {
                // Full rebuild path: the whole pane changed, so the frame
                // must clear + present fully -- or, when the change is
                // bounded to known rows (124.14), a smaller region (#435).
                // `cache.last_frame_cursor_damage` is assigned once, at the
                // end of this branch, once `full_rebuild_damage` (decided
                // above, before this identical-either-way rebuild ran) can
                // be turned into the actual `PaneFrameDamage`.
                {
                    let shaped_lines = cache.shaping_cache.shape_visible(
                        &snap.visible_chars,
                        &snap.visible_tags,
                        snap.term_width,
                        &mut self.font_manager,
                        cell_w_f,
                        self.toggles.ligatures,
                        &snap.visible_line_widths,
                    );

                    // ── Apply folding to shaped_lines ─────────────────────────
                    //
                    // The renderer iterates `shaped_lines` by enumerated index
                    // and treats that index as the screen row.  When folds are
                    // active, the rendered row layout differs from the snapshot
                    // row layout: each folded range collapses to a single
                    // *placeholder* row.  Build a new Vec sized to
                    // `rendered_row_count`, mapping each rendered row index back
                    // to its snapshot row (or to a blank placeholder).
                    //
                    // 72.10b-3: each placeholder row carries a shaped line of
                    // `"▶ {N} lines hidden — click to unfold"` rendered in a
                    // dim foreground colour (BrightBlack from the active
                    // palette).  Per-placeholder hit rects are recorded into
                    // `cache.placeholder_hit_rects` so the input handler can
                    // turn primary clicks on those rows into `view_state.unfold()`.
                    cache.placeholder_hit_rects.clear();
                    let rendered_shaped_lines: Vec<Arc<ShapedLine>> = if row_map.ranges().is_empty()
                        && render_skip == 0
                        && snap.window_extra_rows == 0
                    {
                        // No folds and no extra rows: snapshot rows == screen rows.
                        shaped_lines
                    } else {
                        let empty_placeholder = Arc::new(ShapedLine {
                            runs: Vec::new(),
                            line_width: LineWidth::Normal,
                        });
                        let dim_fg = freminal_common::colors::TerminalColor::BrightBlack;
                        // Paint exactly the bottom `term_height` rendered rows
                        // (screen rows). `render_skip` rendered rows are scrolled
                        // off the top so the live bottom stays pinned.
                        let mut out: Vec<Arc<ShapedLine>> = Vec::with_capacity(snap.term_height);
                        for screen in 0..snap.term_height {
                            let rendered = layout.screen_to_rendered(screen);
                            match row_map.rendered_to_snapshot(rendered) {
                                Some(RenderedRow::Snapshot(snap_row)) => {
                                    out.push(
                                        shaped_lines
                                            .get(snap_row)
                                            .cloned()
                                            .unwrap_or_else(|| Arc::clone(&empty_placeholder)),
                                    );
                                }
                                Some(RenderedRow::Placeholder(range)) => {
                                    let text = format_placeholder_text(
                                        range.block_total_rows,
                                        snap.term_width,
                                    );
                                    let shaped = crate::gui::shaping::shape_placeholder_line(
                                        &text,
                                        dim_fg,
                                        &mut self.font_manager,
                                        cell_w_f,
                                        self.toggles.ligatures,
                                    );
                                    out.push(Arc::new(shaped));

                                    // Record the placeholder's hit rect in
                                    // logical pixel coordinates (screen row) so the
                                    // input handler (which sees pointer positions in
                                    // window coordinates) can hit-test against it
                                    // directly.
                                    let screen_f = screen.approx_as::<f32>().unwrap_or(0.0);
                                    let row_top =
                                        screen_f.mul_add(logical_cell_h, terminal_rect.min.y);
                                    let rect = Rect::from_min_size(
                                        egui::pos2(terminal_rect.min.x, row_top),
                                        egui::vec2(terminal_rect.width(), logical_cell_h),
                                    );
                                    cache
                                        .placeholder_hit_rects
                                        .push((rect, range.command_block_id));
                                }
                                None => {
                                    out.push(Arc::clone(&empty_placeholder));
                                }
                            }
                        }
                        out
                    };

                    // Build search match highlights from the current search state.
                    // Only matches within the flattened window are included, with
                    // rows converted from logical row numbers to snapshot-relative.
                    let snap_rows = snap.term_height.saturating_add(snap.window_extra_rows);
                    let search_highlights_snap: Vec<MatchHighlight> = matches_to_highlights(
                        &view_state.search_state,
                        snap.row_number_at(flat_window_start),
                        snap_rows,
                    );
                    // Translate from snapshot-row space to screen-row space and
                    // drop highlights inside folded ranges or scrolled off the top.
                    let search_highlights: Vec<MatchHighlight> =
                        if row_map.ranges().is_empty() && render_skip == 0 {
                            search_highlights_snap
                        } else {
                            search_highlights_snap
                                .into_iter()
                                .filter_map(|h| {
                                    let rendered = row_map.snapshot_to_rendered(h.row)?;
                                    let screen = layout.rendered_to_screen(rendered)?;
                                    Some(MatchHighlight { row: screen, ..h })
                                })
                                .collect()
                        };

                    // This frame's search-highlight screen rows (Task
                    // 124.14d), for `build_bounded_damage`'s union with
                    // `changed_rows`/selection/hover. Derived from
                    // `search_highlights` above -- already snapshot ->
                    // rendered -> screen translated -- rather than
                    // re-translating `MatchSpan`s a second time. Sorted and
                    // deduplicated so a broad search costs at most one entry
                    // per visible row, regardless of full-buffer match count.
                    let mut current_search_screen_rows: Vec<usize> =
                        search_highlights.iter().map(|h| h.row).collect();
                    current_search_screen_rows.sort_unstable();
                    current_search_screen_rows.dedup();

                    // Translate the selection's row indices from snapshot to
                    // bottom-anchored screen space.  If either endpoint sits inside
                    // a folded range or is scrolled off the top, drop the selection
                    // for this frame (it will reappear when the user unfolds /
                    // scrolls back).
                    let screen_selection_rendered =
                        if row_map.ranges().is_empty() && render_skip == 0 {
                            screen_selection
                        } else {
                            screen_selection.and_then(|(sc, sr, ec, er)| {
                                let sr_s =
                                    layout.rendered_to_screen(row_map.snapshot_to_rendered(sr)?)?;
                                let er_s =
                                    layout.rendered_to_screen(row_map.snapshot_to_rendered(er)?)?;
                                Some((sc, sr_s, ec, er_s))
                            })
                        };

                    // This frame's selection screen-row span (inclusive),
                    // for the union-with-selection extension of
                    // `build_bounded_damage` (Task 124.14b-i). Derived from
                    // `screen_selection_rendered` above rather than
                    // re-translating `screen_selection` a second time --
                    // that value already performed the identical snapshot
                    // -> rendered -> screen translation, and a second
                    // hand-rolled translation is how these go wrong.
                    let current_selection_screen_rows = screen_selection_rendered
                        .map(|(_, sr_s, _, er_s)| (sr_s.min(er_s), sr_s.max(er_s)));

                    // ── Command-block hover-row range (current frame) ──
                    //
                    // Determine which OSC 133 block (if any) the mouse is
                    // hovering over and compute its rendered-row span.  The
                    // result is passed into `BackgroundFrame` so the tint
                    // is drawn alongside selection / search highlights in
                    // the same vertex batch.  Disabled when the feature is
                    // off, when the alternate screen is active (command
                    // blocks describe primary-screen rows and must not tint
                    // a full-screen TUI), or when no blocks exist.
                    //
                    // Two trigger surfaces feed this: hovering a cell inside
                    // the terminal area (72.12), and hovering the command-block
                    // gutter strip (73.3).  73.5 will retire the cell trigger,
                    // leaving the gutter as the sole affordance.
                    // `command_block_hover_rows` was computed earlier (before the
                    // vertex-rebuild decision) so a hover-only change can force a
                    // rebuild; reuse it here.
                    let command_block_hover_rows = command_block_hover_rows_early;

                    // Acquire the lock early so all vertex builders can write
                    // directly into the persistent `RenderState` Vecs, reusing
                    // their heap allocations (clear+extend pattern) instead of
                    // allocating fresh Vecs every frame.
                    let mut rs = render_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    // Reborrow through `&mut *rs` so the borrow checker can see
                    // disjoint field accesses (MutexGuard's DerefMut is opaque).
                    let rs_ref: &mut RenderState = &mut rs;

                    // `cursor_range` is the authoritative answer for where the
                    // cursor sits in `deco_verts` (always the final region). It
                    // MUST be stored as returned rather than re-derived from
                    // `cursor_appearance`, which ignores the blink phase and
                    // the cursor's variable length (issue #432).
                    let cursor_range = build_background_instances(
                        &BackgroundFrame {
                            shaped_lines: &rendered_shaped_lines,
                            cell_width: cell_w,
                            cell_height: cell_h,
                            ascent: self.font_manager.ascent(),
                            underline_offset: self.font_manager.underline_offset(),
                            strikeout_offset: self.font_manager.strikeout_offset(),
                            stroke_size: self.font_manager.stroke_size(),
                            cursor: &cursor_draw,
                            selection: screen_selection_rendered,
                            selection_is_block: view_state.selection.is_block,
                            match_highlights: &search_highlights,
                            command_block_hover_rows,
                            term_width_cols: snap.term_width,
                            theme: snap.theme,
                            // Task 115.2: DECSCNM (whole-screen reverse video)
                            // composes with per-cell SGR-7 by XOR inside the
                            // vertex builders via `effective_fg`/`effective_bg`.
                            // `is_normal_display` is `true` for normal display,
                            // so DECSCNM-active is its negation.
                            reverse_screen: !snap.is_normal_display,
                        },
                        &mut rs_ref.bg_instances,
                        &mut rs_ref.deco_verts,
                    );

                    let fg_opts = FgRenderOptions {
                        selection: screen_selection_rendered,
                        selection_is_block: view_state.selection.is_block,
                        text_blink_slow_visible: view_state.text_blink_slow_visible,
                        text_blink_fast_visible: view_state.text_blink_fast_visible,
                        // Task 115.2: see the matching `BackgroundFrame`
                        // construction above for the XOR-compose rationale.
                        reverse_screen: !snap.is_normal_display,
                    };
                    build_foreground_instances(
                        &rendered_shaped_lines,
                        &mut rs_ref.atlas,
                        &self.font_manager,
                        cell_h,
                        self.font_manager.ascent(),
                        &fg_opts,
                        snap.theme,
                        &mut rs_ref.fg_instances,
                    );
                    build_image_verts(
                        &snap.visible_image_placements,
                        &snap.images,
                        snap.term_width,
                        cell_w,
                        cell_h,
                        &mut rs_ref.image_verts,
                        &mut rs_ref.image_draw_order,
                    );
                    // Clone the image map into RenderState so the PaintCallback
                    // (which must be Send+Sync+'static) can pass it to the renderer.
                    rs_ref.snap_images.clone_from(snap.images.as_ref());
                    // Overwrite each animated image's pixels with the frame
                    // currently selected by the GUI-side wall-clock playback
                    // clock (Task 100.2c). `build_image_verts` above only reads
                    // frame-invariant display dims, so only the texture-upload
                    // path (which reads `img.pixels`) needs the swapped frame.
                    for (id, img) in &mut rs_ref.snap_images {
                        if img.is_animated()
                            && let Some(px) = img.frame_pixels(view_state.selected_frame(*id))
                        {
                            img.pixels = Arc::clone(px);
                        }
                    }
                    rs_ref.cursor_vert_range = Some(cursor_range);
                    rs_ref.cell_width_px = f32::approx_from(cell_w).unwrap_or(0.0);
                    rs_ref.cell_height_px = f32::approx_from(cell_h).unwrap_or(0.0);
                    rs_ref.bg_opacity = bg_opacity;
                    rs_ref.bg_image_opacity = bg_image_opacity;
                    rs_ref.bg_image_mode = bg_image_mode;
                    drop(rs);

                    // Remember what we just rendered: `visible_chars` for the
                    // selection auto-clear's chars-only confirmation comparison
                    // (`frame_dirty.rs`), and `row_epochs` as the baseline the
                    // next frame's `diff_row_epochs` diffs against -- the
                    // primary content-change signal since Task 124.12.
                    cache.last_rendered_visible = Some(Arc::clone(&snap.visible_chars));
                    cache.last_rendered_row_epochs = Some(Arc::clone(&snap.row_epochs));
                    cache.previous_theme = Some(snap.theme);
                    // Captured BEFORE the overwrite below so the damage
                    // build a few lines down can still see where THIS
                    // frame's selection was painted last time (Task
                    // 124.14b-i) -- `previous_selection` and
                    // `previous_selection_screen_rows` are updated in
                    // lockstep here, inside the rebuild body, so both only
                    // advance on a frame that actually drew.
                    let previous_selection_screen_rows = cache.previous_selection_screen_rows;
                    cache.previous_selection = current_selection;
                    cache.previous_screen_selection = screen_selection;
                    cache.previous_selection_screen_rows = current_selection_screen_rows;
                    cache.previous_text_blink_slow_visible = view_state.text_blink_slow_visible;
                    cache.previous_text_blink_fast_visible = view_state.text_blink_fast_visible;
                    cache.previous_search_epoch = search_epoch;
                    // Same capture-before-overwrite shape as selection above
                    // (Task 124.14b-ii). No screen-space companion field is
                    // needed here: this value is already screen-space, so
                    // there is nothing to keep in lockstep and no pair that
                    // could drift apart.
                    let previous_hover_screen_rows = cache.previous_command_block_hover_rows;
                    cache.previous_command_block_hover_rows = command_block_hover_rows_early;
                    // Same capture-before-overwrite shape as selection/hover
                    // above (Task 124.14d): only advances the search-overlay
                    // highlight-row baseline on a frame that actually ran
                    // this rebuild body, so a skipped frame cannot advance
                    // it. `replace_highlight_rows` sorts/dedups internally,
                    // so `previous_search_screen_rows` is guaranteed sorted
                    // and deduplicated even though `current_search_screen_rows`
                    // (above) already was.
                    let previous_search_screen_rows = cache
                        .search_damage
                        .replace_highlight_rows(current_search_screen_rows.clone());
                    cache.previous_term_width = snap.term_width;
                    cache.previous_term_height = snap.term_height;
                    cache.previous_fold_epoch = fold_epoch;
                    // Record exactly which selected-frame pixel buffers were just
                    // uploaded (Task 100.12), so the next frame's
                    // `image_pixels_changed` comparison is against fresh state —
                    // otherwise a one-off pixel mutation would pin the pane in
                    // the full-rebuild path forever.
                    cache.last_rendered_image_pixel_ptrs =
                        build_image_pixel_ptrs(&snap.images, |id| view_state.selected_frame(id));

                    cache.last_frame_cursor_damage = match full_rebuild_damage {
                        // Task 124.21 finding 2: report this pane's OWN rect
                        // rather than escalating the whole frame to `Full`,
                        // so a busy pane in a split no longer forces a full
                        // clear + present of every sibling. `pane_rect_damage`
                        // is `None` only when the pane's own bounds are
                        // degenerate, in which case `Full` is the correct,
                        // safe fallback.
                        FullRebuildDamage::Full => pane_rect_damage
                            .map_or(crate::gui::renderer::PaneFrameDamage::Full, |d| {
                                crate::gui::renderer::PaneFrameDamage::Region(vec![d])
                            }),
                        FullRebuildDamage::Bounded => {
                            // Task 124.14d: `Unchanged` is the accurate
                            // terminal-band contribution -- not a fallback --
                            // only when search is the SOLE bounded source
                            // this frame and its own highlight-row union
                            // comes out empty too (no visible matches before
                            // or after). The floating popup's own damage is
                            // unioned in separately, at the per-pane-frame
                            // level (see the search-overlay block below).
                            // Selection/hover/row sources keep the existing
                            // `Full` fallback -- see `EmptyBoundedDamage`'s
                            // doc comment.
                            let empty_fallback = if search_changed
                                && !selection_changed
                                && !hover_changed
                                && !dirty.changed_rows.any()
                            {
                                EmptyBoundedDamage::Unchanged
                            } else {
                                EmptyBoundedDamage::Full
                            };
                            build_bounded_damage(
                                &dirty.changed_rows,
                                BoundedDamageSpans {
                                    current_selection: current_selection_screen_rows,
                                    previous_selection: previous_selection_screen_rows,
                                    current_hover: command_block_hover_rows_early,
                                    previous_hover: previous_hover_screen_rows,
                                    current_search_rows: &current_search_screen_rows,
                                    previous_search_rows: &previous_search_screen_rows,
                                    // Cursor rows contribute only when cursor
                                    // state changed; otherwise search-only
                                    // empty damage must remain `Unchanged`.
                                    current_cursor_row: cursor_state_changed
                                        .then_some(dirty.cursor_screen_row)
                                        .flatten(),
                                    previous_cursor_row: cursor_state_changed
                                        .then_some(cache.previous_cursor_screen_row)
                                        .flatten(),
                                },
                                row_map,
                                &layout,
                                RowDamageGeometry {
                                    row_h_f,
                                    viewport_width_px: terminal_rect.width() * ppp,
                                    vp_left_px,
                                    vp_top_px,
                                    fb_height_px,
                                },
                                empty_fallback,
                            )
                        }
                    };
                }
            }
            // else: neither path applies (content unchanged, cursor
            // unchanged, selection unchanged, buffers not empty) -- simply
            // re-draw the existing VBO data, no CPU work at all.

            // Drive the cursor trail animation: request a repaint on the next
            // frame so the interpolation continues smoothly until it completes.
            // Folded into `cache` (subtask 121.12), not requested on the
            // `Context` directly — see `PaneRenderCache::request_repaint_after`.
            if cursor_animating {
                cache.request_repaint_after(std::time::Duration::from_millis(16));
            }

            // Drive animated image playback: request a repaint when the next
            // frame is due so animations keep advancing while otherwise idle.
            if let Some(due) = anim_tick.next_due {
                cache.request_repaint_after(due);
            }

            // Everything above has consumed the prior baselines (damage reads
            // the previous cursor row), so it is now safe to replace them with
            // what this frame drew.
            CursorFrame::Drawn(DrawnCursor {
                blink_phase: dirty.cursor_blink_phase,
                focus: cursor_focus_now,
                pos: snap.cursor_pos,
                screen_row: dirty.cursor_screen_row,
                appearance: cursor_appearance,
                color_override: snap.cursor_color_override,
            })
        };

        // Update per-frame cursor state for the next frame's comparison. A
        // skipped frame drew nothing, so it leaves every baseline untouched.
        cache.record_cursor_frame(cursor_frame);

        // Allocate the exact terminal rect (in logical points for egui).
        let desired_size = egui::Vec2::new(
            snap.term_width.approx_as::<f32>().unwrap_or(0.0) * logical_cell_w,
            snap.height.approx_as::<f32>().unwrap_or(0.0) * logical_cell_h,
        );
        let (_rect, _response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
        // Use the terminal area (the full pane minus the command-block gutter
        // strip on the left) as the PaintCallback rect.  The cell-content
        // vertex coordinates are computed relative to (0,0) in physical pixels,
        // so the GL viewport origin must be the terminal rect's left edge —
        // otherwise column 0 would render under the gutter strip.  The right
        // and bottom edges are unchanged, so the post-process shader still
        // covers the full cell area (any sub-cell padding at the right/bottom).
        // The gutter slice itself is painted separately by egui below.
        let rect = terminal_rect;

        // Hand off the draw call to egui's paint phase via PaintCallback.
        // The closure must be `Send + Sync + 'static`, so only `Arc<Mutex<…>>`
        // data (not `FontManager`) may be captured here.  `is_cursor_only` is
        // captured by value (bool is Copy). The decoration vertex data itself
        // is read from `RenderState::deco_verts` inside the closure, not
        // captured separately.
        let render_state_for_cb = Arc::clone(render_state);
        // Authoritative present region (124.18, formerly a
        // `present_is_partial: AtomicBool`): the windowing layer publishes
        // this just before this callback runs. The cursor-only scissor
        // reads it directly, rather than scissoring to this pane's own
        // narrower cursor-damage rect — see the scissor call site below for
        // why that distinction matters.
        let present_region_cb = Arc::clone(present_region);
        // The MutexGuard inside the callback intentionally lives through
        // `draw_with_verts` because the renderer and atlas are refs into it.
        #[allow(clippy::significant_drop_tightening)]
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(CallbackFn::new(move |info, painter| {
                let gl = &Gl::real(painter.gl());
                let vp = info.viewport_in_pixels();
                let mut rs = render_state_for_cb
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // A failed init is latched (`should_attempt_init` turns false):
                // it is reported once, through the window's `last_error`
                // channel, rather than retried and re-logged every frame.
                if rs.renderer.should_attempt_init()
                    && let Err(e) = rs.renderer.init(gl)
                {
                    error!("GL init failed: {e}");
                    rs.window_post
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .last_error = Some(format!("Terminal renderer init failed: {e}"));
                }
                if !rs.renderer.initialized() {
                    return;
                }

                // Apply any pending background image changes that arrived from
                // the config-apply path (these need a GL context).
                if let Some(pending) = rs.pending_bg_image.take() {
                    match pending {
                        PendingGpuOp::Load(ref path)
                            if let Err(e) = rs.renderer.update_background_image(gl, path) =>
                        {
                            error!("Failed to load background image: {e}");
                        }
                        PendingGpuOp::Load(_) => {}
                        PendingGpuOp::Clear => rs.renderer.clear_background_image(gl),
                    }
                }

                // Determine the render target framebuffer.
                //
                // When a window-level post-processing shader is active, each pane
                // renders into the shared window FBO (so the shader can composite the
                // full window).  When inactive, panes render directly to egui's FBO.
                let wpr_fbo = {
                    let wpr = rs
                        .window_post
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if wpr.is_active() { wpr.fbo() } else { None }
                };

                // If the window FBO is active, explicitly bind it now.
                // egui has already set viewport/scissor for this pane's sub-rect,
                // which persists across FBO binds.  After drawing, draw_with_verts
                // restores the binding to `restore_fbo` (egui's FBO) so egui
                // state is clean after the callback.
                if wpr_fbo.is_some() {
                    unsafe {
                        gl.bind_framebuffer(glow::FRAMEBUFFER, wpr_fbo);
                    }
                }
                // The restore-FBO is always egui's intermediate FBO, regardless
                // of which FBO we rendered into.
                let restore_fbo = painter.intermediate_fbo();

                // Scissor the GPU redraw to what the windowing layer
                // published as this frame's present region (124.18), NOT to
                // this pane's own damage rect (the earlier `#435` design for
                // the cursor-only arm). The rest of the framebuffer already
                // holds the previous frame's contents, but on a stale
                // (`buffer_age() > 1`) back buffer the windowing layer's
                // region can be a union covering MORE than just this arm's
                // own declared damage (e.g. a previous frame's cursor
                // position this buffer never received) — scissoring to a
                // narrower rect would silently skip repainting pixels the
                // union says still need it. `PresentRegion::Region` is
                // physical framebuffer pixels, bottom-left origin — the same
                // convention as `glScissor`.
                //
                // Both the cursor-only and full-draw arms below read this
                // *same* value (124.23): a full draw that scissored nothing
                // could redraw pixels the clear deliberately skipped over,
                // which then blend against stale (rather than cleared)
                // content when `background_opacity < 1.0`. One region now
                // governs the clip, the clear, and the present for either
                // arm — see `draw_scissored_to_present_region`.
                //
                // egui disables `SCISSOR_TEST` after painting all
                // primitives, and this callback is self-contained, so
                // `draw_scissored_to_present_region` enables it before the
                // draw and disables it again afterwards, leaving GL scissor
                // state as egui expects.
                //
                // `PresentRegion::Full` means the windowing layer could not
                // prove a smaller region was safe (or the surface doesn't
                // support partial present at all) — the whole grid must be
                // redrawn, so neither arm scissors in that case.
                let region = *present_region_cb
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);

                // Task 125.6: exactly the uploads the draw call below
                // issues, assigned exactly once inside whichever arm runs.
                // Declared here (rather than a default-initialized `let
                // mut`) so a future arm added without an assignment fails
                // to compile instead of silently finalizing with a stale
                // value. Entirely absent from a default build -- see
                // `DrawUploadCounts`'s doc in `gui::renderer::gpu`.
                #[cfg(feature = "frame-profiling")]
                let frame_uploads: UploadByteCounts;

                if is_cursor_only {
                    // Cursor-only fast path: bg/fg/image are unchanged and
                    // simply redrawn from the last full rebuild's slot.
                    // `deco_verts` (underline/strike/search/hover/selection/
                    // cursor quads) DOES change every such frame (the cursor
                    // moved/blinked), so it is always fully re-uploaded via a
                    // safe orphan-then-write into its own double-buffer slot
                    // — never patched live with `glBufferSubData` into a
                    // buffer a pending GPU read may still be using (#432).
                    let bg_len = rs.bg_instances.len();
                    let fg_len = rs.fg_instances.len();
                    let img_len = rs.image_verts.len();
                    let cw = rs.cell_width_px;
                    let ch = rs.cell_height_px;
                    let opacity = rs.bg_opacity;
                    let bg_image_opacity = rs.bg_image_opacity;
                    let bg_image_mode = rs.bg_image_mode;
                    // Split borrow: renderer + atlas are disjoint from the
                    // scalar fields, deco_verts, and image_draw_order.
                    let rs_ref: &mut RenderState = &mut rs;
                    let renderer = &mut rs_ref.renderer;
                    let atlas = &mut rs_ref.atlas;
                    // The full, current decoration data — already patched
                    // in place with the cursor quad above.
                    let deco_verts = &rs_ref.deco_verts;
                    // Reuse the draw order retained from the last full
                    // rebuild — the cursor-only path does not recompute
                    // image state, so this is the same list `draw_images`
                    // used to emit `rs_ref.image_verts` last time.
                    let draw_order = &rs_ref.image_draw_order;

                    // Task 125.6: the `frame-profiling` arm captures
                    // `draw_with_cursor_only_update`'s returned upload-byte
                    // measurement (dropping the closure body's trailing
                    // semicolon makes it the closure's — and so
                    // `draw_scissored_to_present_region`'s — return
                    // value); the default arm issues the identical call
                    // and discards its (unconditionally computed, but
                    // here-unused) return, exactly as before this
                    // subtask.
                    #[cfg(feature = "frame-profiling")]
                    {
                        frame_uploads = draw_scissored_to_present_region(gl, region, || {
                            renderer.draw_with_cursor_only_update(
                                gl,
                                atlas,
                                deco_verts,
                                bg_len,
                                fg_len,
                                img_len,
                                draw_order,
                                vp.width_px,
                                vp.height_px,
                                cw,
                                ch,
                                opacity,
                                bg_image_opacity,
                                bg_image_mode,
                                restore_fbo,
                            )
                        });
                    }
                    #[cfg(not(feature = "frame-profiling"))]
                    draw_scissored_to_present_region(gl, region, || {
                        renderer.draw_with_cursor_only_update(
                            gl,
                            atlas,
                            deco_verts,
                            bg_len,
                            fg_len,
                            img_len,
                            draw_order,
                            vp.width_px,
                            vp.height_px,
                            cw,
                            ch,
                            opacity,
                            bg_image_opacity,
                            bg_image_mode,
                            restore_fbo,
                        );
                    });
                } else {
                    // Full draw path: split-borrow RenderState to pass
                    // vertex slices by reference (no cloning) alongside
                    // the mutable renderer and atlas.
                    let cw = rs.cell_width_px;
                    let ch = rs.cell_height_px;
                    let opacity = rs.bg_opacity;
                    let bg_image_opacity = rs.bg_image_opacity;
                    let bg_image_mode = rs.bg_image_mode;
                    let rs_ref: &mut RenderState = &mut rs;
                    let renderer = &mut rs_ref.renderer;
                    let atlas = &mut rs_ref.atlas;
                    // Task 125.6: see the `is_cursor_only` arm above for
                    // why the two cfg arms differ only in whether the
                    // closure's return value is bound.
                    #[cfg(feature = "frame-profiling")]
                    {
                        frame_uploads = draw_scissored_to_present_region(gl, region, || {
                            renderer.draw_with_verts(
                                gl,
                                atlas,
                                &rs_ref.bg_instances,
                                &rs_ref.deco_verts,
                                &rs_ref.fg_instances,
                                &rs_ref.image_verts,
                                &rs_ref.image_draw_order,
                                &rs_ref.snap_images,
                                vp.width_px,
                                vp.height_px,
                                cw,
                                ch,
                                opacity,
                                bg_image_opacity,
                                bg_image_mode,
                                restore_fbo,
                            )
                        });
                    }
                    #[cfg(not(feature = "frame-profiling"))]
                    draw_scissored_to_present_region(gl, region, || {
                        renderer.draw_with_verts(
                            gl,
                            atlas,
                            &rs_ref.bg_instances,
                            &rs_ref.deco_verts,
                            &rs_ref.fg_instances,
                            &rs_ref.image_verts,
                            &rs_ref.image_draw_order,
                            &rs_ref.snap_images,
                            vp.width_px,
                            vp.height_px,
                            cw,
                            ch,
                            opacity,
                            bg_image_opacity,
                            bg_image_mode,
                            restore_fbo,
                        );
                    });
                }

                // Task 125.5/125.6: finalize this frame's live-profiling
                // observation now that the actual GL draw above has run.
                // `complete` takes back only the token and the upload
                // counts -- the raw/resolved classification was already
                // recorded by `start` above, against this exact token, so
                // it cannot be reported differently here. `frame_uploads`
                // is exactly what the draw call issued (Task 125.6): real
                // per-buffer/per-texture byte counts, not an estimate from
                // any buffer's length. `profiling_token` is `None` on a
                // `skip_draw` frame (no rebuild decision was made, so
                // there is nothing to finalize) and permanently on a
                // non-`frame-profiling` build; a callback that is never
                // invoked at all (this closure's body never runs) leaves
                // its token pending, and the NEXT `show()` call's
                // `LiveRenderProfile::start` finalizes it as unpainted --
                // see that type's module doc.
                #[cfg(feature = "frame-profiling")]
                if let Some(token) = profiling_token {
                    rs.live_profile.complete(token, frame_uploads);
                    // Also checked right after `start()` above -- see that
                    // call site's comment for why a boundary crossed by
                    // the stale-token sweep inside `start` would otherwise
                    // never be logged. `maybe_log_live_render_profile_flush`
                    // itself only actually logs once per crossed boundary
                    // (`LiveRenderProfile::take_flush_signal` consumes the
                    // signal), so calling it from both sites cannot
                    // produce a duplicate log line.
                    maybe_log_live_render_profile_flush(&mut rs.live_profile, pane_id);
                }

                // Task 125.8: log this pane's asynchronous GPU upload/draw
                // timing flush, if one is due. Independent of
                // `frame-profiling`/`profiling_token` -- GPU timing samples
                // are issued and polled unconditionally by the draw call
                // above whenever `gpu-profiling` is enabled, regardless of
                // whether this frame also carried a live-render-work
                // observation.
                #[cfg(feature = "gpu-profiling")]
                if let Some(report) = rs.renderer.take_gpu_timing_flush() {
                    tracing::debug!(
                        target: GPU_TIMING_LOG_TARGET,
                        pane_id = ?pane_id,
                        renderer = %report.renderer_string,
                        capability = ?report.capability,
                        upload_ns_total = report.upload_ns_total,
                        upload_sample_count = report.upload_sample_count,
                        draw_ns_total = report.draw_ns_total,
                        draw_sample_count = report.draw_sample_count,
                        last_latency_frames = report.last_latency_frames,
                        dropped_sample_count = report.dropped_sample_count,
                        unavailable_sample_count = report.unavailable_sample_count,
                        "Task 125.8 terminal GPU timing flush"
                    );
                }
            })),
        });

        // ── Scrollbar (visual + interactive) ─────────────────────────
        let scrollbar_outcome = handle_scrollbar(
            snap.scroll_offset,
            snap.max_scroll_offset,
            ui,
            &mut cache.scrollbar_dragging,
        );
        if let Some(new_offset) = scrollbar_outcome.new_offset {
            view_state.scroll_offset = new_offset;
            let _ = input_tx.try_send(super::input::scroll_event(
                snap,
                &view_state.folded_blocks,
                new_offset,
            ));
        }

        // Task 124.3a/124.3b: publish this frame's geometry + suppressor
        // snapshot for the out-of-frame immediate PTY mouse-report path AND
        // the out-of-frame pointer-motion repaint predicate. `app_impl`
        // lifts this into `PublishedFrameState` immediately after `show()`
        // returns.
        //
        // Deliberately published HERE, after `handle_scrollbar` runs, not
        // back where `suppressors` was built (124.3a's original site,
        // before terminal-input processing): `cache.scrollbar_dragging` is
        // mutated BY `handle_scrollbar` (release detection, drag-continue),
        // so reading it before that call published a stale snapshot — a
        // drag that ended earlier this same frame would still publish
        // `scrollbar_drag: true`. Reading it here, after the mutation, is
        // this frame's true value. `scrollbar_hit_rect` is `None` unless
        // the thumb actually rendered this frame (`ScrollbarOutcome`'s own
        // doc). Every other field (`terminal_rect`/`cell_size`/`ppp`/the
        // four non-scrollbar suppressors) is unchanged by anything between
        // the old and new publish sites, so moving this later changes
        // nothing about them.
        cache.pointer_report_inputs = PanePointerReportInputs {
            terminal_rect,
            cell_size: egui::vec2(logical_cell_w, logical_cell_h),
            pixels_per_point: ppp,
            modal_or_drag: suppressors.modal_or_drag,
            context_menu: suppressors.context_menu,
            search_overlay: suppressors.search_overlay,
            command_history: suppressors.command_history,
            scrollbar_drag: cache.scrollbar_dragging,
            scrollbar_hit_rect: scrollbar_outcome.hit_rect,
        };

        // #459 item 9 / mirrors #461 gutter: the scrollbar thumb is painted on
        // the plain egui painter (alpha varies with hover), outside per-pane VBO
        // damage. A hover-alpha change or a rendered->not-rendered vanish must
        // force one Full clear or a Partial present (driven by an unrelated
        // pane's cursor blink) would leave a stale/ghosted thumb.
        // `scrollbar_outcome.hovered` is already the window-exit-corrected
        // signal (the same one the thumb's alpha was painted with), so the
        // paint and this damage decision can never drift a frame apart.
        let current_scrollbar_state = ScrollbarDamageState {
            rendered: scrollbar_outcome.rendered,
            effectively_hovered: scrollbar_outcome.hovered,
        };
        let previous_scrollbar_state = ScrollbarDamageState {
            rendered: cache.scrollbar_was_rendered_last_frame,
            effectively_hovered: cache.scrollbar_was_hovered_last_frame,
        };
        if scrollbar_damage_decision(current_scrollbar_state, previous_scrollbar_state) {
            // 16ms, not `Duration::ZERO` (subtask 121.12) — see the
            // gutter-hover comment above: scheduling is unchanged (already
            // floored at 16ms by `clamp_repaint_delay`), so this value is
            // scheduling-equivalent to zero.
            cache.request_repaint_after(std::time::Duration::from_millis(16));
            cache.last_frame_cursor_damage = crate::gui::renderer::PaneFrameDamage::Full;
        }
        cache.scrollbar_was_rendered_last_frame = scrollbar_outcome.rendered;
        cache.scrollbar_was_hovered_last_frame = scrollbar_outcome.hovered;

        // ── Visual bell flash overlay ────────────────────────────────
        // Fold the returned delay into `cache` (subtask 121.12) — see
        // `paint_bell_flash`'s doc comment for why it does not request the
        // repaint on the `Context` directly.
        if let Some(delay) = paint_bell_flash(ui, rect, view_state) {
            cache.request_repaint_after(delay);
        }

        // ── Password-prompt lock indicator ───────────────────────────
        // When echo-off is detected (password prompt), paint a lock icon
        // at the cursor position.  The normal cursor is suppressed (it
        // resolves to `CursorAppearance::Hidden`) so only the lock icon is
        // visible.
        if is_echo_off {
            let cursor_logical_x = view_state
                .cursor_visual_col
                .mul_add(logical_cell_w, terminal_rect.min.x);
            let cursor_logical_y = view_state
                .cursor_visual_row
                .mul_add(logical_cell_h, terminal_rect.min.y);
            let lock_pos = egui::pos2(cursor_logical_x, cursor_logical_y);
            // Bundled lock glyph (monospace family → resolves from the bundled
            // Nerd Font, not the fallible system emoji font), tinted to the
            // palette warning color rather than a hard-coded amber.
            let lock_color = ui.visuals().warn_fg_color;
            ui.painter().text(
                lock_pos,
                egui::Align2::LEFT_TOP,
                ChromeIcon::Lock.glyph(),
                egui::FontId::monospace(logical_cell_h),
                lock_color,
            );
        }

        // ── Command-block status gutter ──────────────────────────────
        // Fill the reserved left strip (`gutter_rect`) with a per-row
        // status color: each visible rendered row maps back to a buffer
        // row; if a command block contains that row, the gutter cell is
        // painted with the block's status color (green = success,
        // red = failure, yellow = running, white = unknown).  Rows in no
        // block render the terminal background (empty gutter).
        //
        // Suppressed on the alternate screen for the same reason the
        // overlays are: the stored blocks describe primary-screen rows.
        // The 4px strip is OUTSIDE the cell grid (`terminal_rect` was
        // shifted right by the inset), so it never overlaps glyph cells.
        if gutter_inset > 0.0
            && command_blocks_config.enabled
            && !snap.is_alternate_screen
            && !snap.command_blocks.is_empty()
        {
            let win_start = flat_window_start;
            // Running blocks extend only to the cursor's row (the last line of
            // output produced so far), not the bottom of the pane (106.2b).
            let running_extent = running_block_extent(snap);
            // Iterate on-screen rows (bottom-anchored); map each back through
            // rendered → snapshot → buffer.
            for screen_row_idx in 0..snap.term_height {
                let rendered_row = layout.screen_to_rendered(screen_row_idx);
                // Resolve each rendered row to a status color.  Snapshot
                // rows map back to a buffer row and use row containment;
                // fold placeholders are colored (desaturated) by the
                // folded block's own status, looked up by id.
                let resolved: Option<(CommandStatus, bool)> =
                    match row_map.rendered_to_snapshot(rendered_row) {
                        Some(RenderedRow::Snapshot(snap_row)) => {
                            let buffer_row = win_start + snap_row;
                            crate::gui::command_blocks::gutter_status_for_row(
                                &snap.command_blocks,
                                snap.row_base,
                                buffer_row,
                                running_extent,
                            )
                            .map(|s| (s, false))
                        }
                        Some(RenderedRow::Placeholder(range)) => snap
                            .command_blocks
                            .iter()
                            .find(|b| b.id == range.command_block_id)
                            .map(|b| (b.status(), true)),
                        None => None,
                    };
                let Some((status, desaturate)) = resolved else {
                    continue;
                };
                let (cr, cg, cb) = snap.theme.gutter_color_for(status);
                let color = if desaturate {
                    // Half-alpha for folded placeholder rows so a collapsed
                    // block still shows its status, muted.
                    egui::Color32::from_rgba_unmultiplied(cr, cg, cb, 128)
                } else {
                    egui::Color32::from_rgb(cr, cg, cb)
                };
                let screen_f = screen_row_idx.approx_as::<f32>().unwrap_or(0.0);
                let y0 = screen_f.mul_add(logical_cell_h, terminal_rect.min.y);
                let row_rect = egui::Rect::from_min_max(
                    egui::pos2(gutter_rect.min.x, y0),
                    egui::pos2(gutter_rect.max.x, y0 + logical_cell_h),
                );
                ui.painter().rect_filled(row_rect, 0.0, color);
            }
        }

        // ── Command-block duration label (gutter-anchored, 73.6) ─────
        // For each finished block whose duration meets the configured
        // threshold, paint a compact duration label as a floating layer
        // immediately RIGHT of the gutter strip, anchored to the block's
        // LAST visible rendered row.  Anchoring to the last on-screen row
        // (rather than the first, as 72.12 did) keeps the label visible:
        // the first row scrolls off almost immediately for any command
        // that produces output, whereas the gutter follows the block.
        //
        // Requires the gutter to be present (`gutter_inset > 0`) — the
        // label is positioned against it.  Running blocks (no duration)
        // are skipped.  Suppressed on the alternate screen for the usual
        // reason (stored blocks describe primary-screen rows).
        if command_blocks_config.show_duration
            && gutter_inset > 0.0
            && crate::gui::command_blocks::command_block_overlays_visible(
                command_blocks_config.enabled,
                snap.is_alternate_screen,
                !snap.command_blocks.is_empty(),
            )
        {
            let threshold =
                Duration::from_secs_f32(command_blocks_config.duration_threshold_secs.max(0.0));
            let win_start = flat_window_start;
            let win_end = win_start + snap.term_height.saturating_add(snap.window_extra_rows);
            let running_extent = running_block_extent(snap);
            let (fg_r, fg_g, fg_b) = snap.theme.foreground;
            // Muted: ~60% alpha so the label reads without overpowering
            // the underlying cell content.
            let label_color = egui::Color32::from_rgba_unmultiplied(fg_r, fg_g, fg_b, 153);
            let font_id = egui::FontId::monospace(logical_cell_h * 0.75);
            // Floating layer (option a): anchored just inside the cell grid,
            // immediately right of the gutter inset, on the block's last
            // visible row.  It overlays the first cells of that row — a small
            // muted label on the block's bottom line, which follows the block
            // as it scrolls (unlike the old first-row placement).
            let label_x = terminal_rect.min.x + 2.0;
            for block in snap.command_blocks.iter() {
                // `duration()` measures from command-execution start
                // (`executed_at`/OSC 133 C), excluding the user's typing time
                // at the prompt — so instant commands no longer report
                // multi-second durations.  `None` while the block is running.
                let Some(elapsed) = block.duration() else {
                    continue;
                };
                if elapsed < threshold {
                    continue;
                }
                // Anchor on the block's LAST visible row so the label follows
                // the block as it scrolls (see `duration_label_anchor_row`).
                let Some(last_visible_buffer_row) =
                    crate::gui::command_blocks::duration_label_anchor_row(
                        block,
                        snap.row_base,
                        win_start,
                        win_end,
                        running_extent,
                    )
                else {
                    continue; // block entirely outside the viewport
                };
                let snap_row = last_visible_buffer_row.saturating_sub(win_start);
                let Some(screen_row) = row_map
                    .snapshot_to_rendered(snap_row)
                    .and_then(|rendered| layout.rendered_to_screen(rendered))
                else {
                    continue; // last row hidden inside a fold or scrolled off
                };
                let screen_f = screen_row.approx_as::<f32>().unwrap_or(0.0);
                let y = screen_f.mul_add(logical_cell_h, terminal_rect.min.y);
                let pos = egui::pos2(label_x, y);
                let label = crate::gui::command_blocks::format_command_duration(elapsed);
                ui.painter().text(
                    pos,
                    egui::Align2::LEFT_TOP,
                    label,
                    font_id.clone(),
                    label_color,
                );
            }
        }

        // ── OSC 9;4 progress indicator (issue #507, phase B2) ─────────
        // Ghostty-style: a thin bar across the pane's TOP edge, drawn only
        // when the config gate is on and the snapshot reports active
        // progress. Unlike the gutter/duration-label overlays above, this
        // is not tied to command blocks or gated on the primary screen --
        // Ghostty renders it "on top of each split" unconditionally.
        let progress_state = snap.progress.state();
        let progress_active =
            self.toggles.progress_enabled && progress_state != ProgressState::Inactive;
        if progress_active {
            let bar_height_logical = PROGRESS_BAR_HEIGHT_PX / ppp;
            let track_rect = progress_bar_track_rect(pane_rect, bar_height_logical);
            let (r, g, b) = snap.theme.progress_color_for(progress_state);
            let fill_color = egui::Color32::from_rgb(r, g, b);
            if matches!(progress_state, ProgressState::Indeterminate) {
                // Indeterminate: paint a full-width bar with NO marching/
                // marquee animation. A real animated sweep would need a
                // continuously re-armed repaint request and its own
                // moving-extent damage handling -- `merge_progress_damage`
                // below relies on the bar's rect being fixed across frames,
                // which a moving sweep would break. Deliberately out of
                // scope for this phase; see the phase B2 plan.
                ui.painter().rect_filled(track_rect, 0.0, fill_color);
            } else {
                // Determinate states (InProgress/Error/Paused): a dim
                // translucent track plus an opaque fill sized to
                // `percent()`, left-anchored.
                let track_color = egui::Color32::from_rgba_unmultiplied(r, g, b, 70);
                ui.painter().rect_filled(track_rect, 0.0, track_color);
                let fill_rect = progress_fill_rect(track_rect, snap.progress.percent());
                ui.painter().rect_filled(fill_rect, 0.0, fill_color);
            }
        }

        // Damage (issue #507 phase B2; BOUNDABLE-NOW, see
        // `merge_progress_damage`'s doc): only touch
        // `cache.last_frame_cursor_damage` when the bar's active/state/
        // percent fingerprint actually changed since the last frame. An
        // unchanged fingerprint means the previously-presented frame
        // already has the correct pixels here, so there is nothing new to
        // bound -- the same changed-vs-static split the gutter-hover and
        // scrollbar decisions above already use for their own
        // plain-painter overlays.
        let current_progress_fingerprint =
            progress_active.then_some((progress_state, snap.progress.percent()));
        if current_progress_fingerprint != cache.previous_progress {
            let bar_height_logical = PROGRESS_BAR_HEIGHT_PX / ppp;
            let track_rect = progress_bar_track_rect(pane_rect, bar_height_logical);
            let (vp_left_px, vp_top_px, fb_height_px) =
                viewport_framebuffer_geometry(ui, terminal_rect, ppp);
            let bar_damage_rect = rect_damage_relative_to_terminal(
                track_rect,
                terminal_rect,
                ppp,
                vp_left_px,
                vp_top_px,
                fb_height_px,
            );
            let damage = std::mem::take(&mut cache.last_frame_cursor_damage);
            cache.last_frame_cursor_damage = merge_progress_damage(damage, bar_damage_rect);
        }
        cache.previous_progress = current_progress_fingerprint;

        // ── Search overlay ───────────────────────────────────────────
        // Run search refresh when query changed (outside the !snap.skip_draw block
        // to ensure it fires even on identical content frames). Also update
        // the cross-frame search-overlay damage state (Task 124.14d)
        // unconditionally, every frame the widget runs -- not gated on
        // whether a full rebuild happened -- because the bar's own
        // caret/hover/text content can change independently of any
        // terminal-content rebuild.
        let mut search_popup_rect: Option<crate::gui::renderer::PaneDamageRect> = None;
        let mut search_popup_safety = crate::gui::search::SearchOverlaySafety::Bounded;
        if view_state.search_state.is_open {
            let bar_frame = show_search_bar(
                ui,
                view_state,
                terminal_rect,
                search_error.as_deref(),
                pane_id,
            );
            match bar_frame.action {
                SearchBarAction::Next => {
                    view_state.search_state.next_match();
                    scroll_to_match_and_send(view_state, snap, input_tx);
                }
                SearchBarAction::Prev => {
                    view_state.search_state.prev_match();
                    scroll_to_match_and_send(view_state, snap, input_tx);
                }
                SearchBarAction::Close => {
                    view_state.search_state.close();
                }
                SearchBarAction::None => {}
            }

            // Convert the bar's shadow-expanded logical paint rect into a
            // `PaneDamageRect`, relative to `terminal_rect` -- the search
            // block runs outside the `!snap.skip_draw` guard that
            // `vp_left_px`/`vp_top_px`/`fb_height_px` were originally
            // computed inside, so they are recomputed here via the shared
            // `viewport_framebuffer_geometry` helper rather than a second
            // hand-rolled version.
            let (search_vp_left_px, search_vp_top_px, search_fb_height_px) =
                viewport_framebuffer_geometry(ui, terminal_rect, ppp);
            match rect_damage_relative_to_terminal(
                bar_frame.paint_rect,
                terminal_rect,
                ppp,
                search_vp_left_px,
                search_vp_top_px,
                search_fb_height_px,
            ) {
                Some(rect) => {
                    search_popup_rect = Some(rect);
                    search_popup_safety = bar_frame.safety;
                }
                None => {
                    // A degenerate/unconvertible current popup rect is an
                    // unbounded safety case for this frame (Task 124.14d),
                    // not a silently-dropped one.
                    search_popup_safety = crate::gui::search::SearchOverlaySafety::TooltipMayEscape;
                }
            }
        }
        cache
            .search_damage
            .finish_overlay_frame(search_popup_rect, search_popup_safety);

        // ── URL hover detection ───────────────────────────────────────
        //
        // Four gates to minimise work:
        //   1. has_urls — skip everything when no URLs exist (common case).
        //   2. Cell-or-content change — skip URL lookup when the mouse is
        //      still over the same terminal cell AND the snapshot content has
        //      not changed (i.e. the underlying text is identical).
        //   3. Icon-change — skip `output_mut(cursor_icon)` when the icon
        //      has not changed.
        //   4. Click detection always runs against the cached URL so that
        //      Ctrl+click works even when the mouse has not moved.
        if snap.has_urls {
            if let Some((col, row)) = cell_under_pointer_for_url_hover(
                view_state.mouse_position,
                terminal_rect,
                (logical_cell_w, logical_cell_h),
            ) {
                let cell = (col, row);
                let cell_changed = cache.previous_hover_cell != Some(cell);
                // Pointer identity comparison for the snapshot's char buffer.
                // `.addr()` is the explicit, non-`as`-cast form for extracting
                // the pointer's address as a `usize` (stable since Rust 1.84).
                let snap_ptr = Arc::as_ptr(&snap.visible_chars).addr();
                let content_changed_under_mouse = snap_ptr != cache.hover_snap_ptr;
                cache.previous_hover_cell = Some(cell);
                cache.hover_snap_ptr = snap_ptr;

                if cell_changed || content_changed_under_mouse {
                    // Translate the mouse's rendered row to a snapshot row
                    // (folding-aware).  When the mouse hovers over a fold
                    // placeholder row, there is no underlying text to match
                    // against a URL — clear the cache.  When `row` is past
                    // the bottom of the rendered viewport, `rendered_to_snapshot`
                    // returns None and we likewise clear.
                    let snap_row =
                        match row_map.rendered_to_snapshot(layout.screen_to_rendered(row)) {
                            Some(RenderedRow::Snapshot(r)) => Some(r),
                            Some(RenderedRow::Placeholder(_)) | None => None,
                        };
                    cache.cached_hovered_url = snap_row.and_then(|snap_row| {
                        // Recompute the hovered URL: convert the mouse's
                        // display-column position to a flat index into
                        // `visible_chars`, using the O(1) row-offset table.
                        let flat_idx = flat_index_for_cell(
                            &snap.visible_chars,
                            snap_row,
                            col,
                            &snap.row_offsets,
                        );

                        flat_idx.and_then(|idx| {
                            snap.url_tag_indices
                                .iter()
                                .filter_map(|&ti| snap.visible_tags.get(ti))
                                .find(|tag| tag.start <= idx && idx < tag.end)
                                .and_then(|tag| tag.url.clone())
                        })
                    });
                }

                // Tooltip: show the target URL at the pointer so the user
                // can verify before Ctrl+clicking. Suppressed while the
                // user is actively dragging out a selection so it does
                // not visually fight the selection rectangle.
                if !view_state.selection.is_selecting
                    && let Some(url) = &cache.cached_hovered_url
                {
                    let url_text = url.url.clone();
                    egui::Tooltip::always_open(
                        ui.ctx().clone(),
                        ui.layer_id(),
                        egui::Id::new("freminal_url_hover_tooltip"),
                        egui::PopupAnchor::Pointer,
                    )
                    .show(|ui| {
                        ui.label(&url_text);
                        ui.weak(if cfg!(target_os = "macos") {
                            "Cmd+click to open"
                        } else {
                            "Ctrl+click to open"
                        });
                    });
                }

                // Ctrl+click (Cmd+click on macOS) opens the URL.
                if let Some(url) = &cache.cached_hovered_url {
                    let clicked = ui.input(|i| {
                        i.pointer.button_clicked(egui::PointerButton::Primary)
                            && (i.modifiers.ctrl || i.modifiers.mac_cmd)
                    });
                    if clicked {
                        let url_str = url.url.clone();
                        if let Err(e) = std::thread::Builder::new()
                            .name("freminal-open-url".to_string())
                            .spawn(move || {
                                if let Err(e) = open::that(&url_str) {
                                    error!("Failed to open URL {url_str}: {e}");
                                }
                            })
                        {
                            error!("Failed to spawn URL-open thread: {e}");
                        }
                    }
                }
            } else {
                // Mouse left the terminal area, or (issue #510) is over a
                // sibling pane and never actually entered this one. Only the
                // URL-hover cache is cleared here; the icon itself is
                // resolved once below.
                cache.previous_hover_cell = None;
                cache.cached_hovered_url = None;
            }
        } else {
            // No URLs in the visible window, so nothing can be URL-hovered.
            cache.previous_hover_cell = None;
            cache.cached_hovered_url = None;
        }

        // ── Cursor icon ──────────────────────────────────────────────
        //
        // Every source that wants a say in the pointer shape is gathered here
        // and resolved by one explicit precedence rule, then written exactly
        // once. This replaces four independent unconditional writes whose
        // relative outcome was decided purely by which one happened to run
        // last in `show` -- which silently discarded the command-block
        // gutter's pointing-hand entirely (issue #462).
        //
        // The write is unconditional because egui resets
        // `output.cursor_icon` to `Default` at the start of every frame.
        let placeholder_hovered = !cache.placeholder_hit_rects.is_empty()
            && view_state.mouse_position.is_some_and(|pos| {
                hit_test_placeholder(&cache.placeholder_hit_rects, pos).is_some()
            });

        let pointer_hover = PointerHover {
            command_block_gutter: gutter_hovered,
            fold_placeholder: placeholder_hovered,
            url: cache.cached_hovered_url.is_some(),
        };

        // Only the pane the pointer is actually over may set the icon.
        //
        // `output.cursor_icon` is a single window-wide field, and every pane
        // runs this code every frame. Writing unconditionally therefore means
        // the last pane to render decides the cursor for the entire window,
        // clobbering whatever the pane under the pointer resolved. That made
        // gutter and URL hover appear to work only in whichever pane happened
        // to render last (the bottom of a vertical split), and it also
        // overwrote the cursors egui sets for its own chrome -- the I-beam
        // over a text field, resize arrows over a splitter -- because a pane
        // would stamp its own icon over them after they were set.
        //
        // `rect_contains_pointer` respects layer and clip rect, so a modal
        // drawn above the pane correctly keeps its own cursor. Split-border
        // sensors are not a separate layer -- they overlap the pane
        // geometrically -- so they are excluded explicitly.
        if ui.rect_contains_pointer(pane_rect) && split_border_hover == SplitBorderHover::Clear {
            let resolved_icon = cursor_icon_for(pointer_hover.resolve(), snap.pointer_shape);
            ui.ctx().output_mut(|output| {
                output.cursor_icon = resolved_icon;
            });
        }

        // ── Drag-and-drop ────────────────────────────────────────────
        handle_file_drop(ui, terminal_rect, input_tx);

        // ── Right-click context menu ─────────────────────────────────
        render_context_menu(
            ui,
            snap,
            view_state,
            input_tx,
            clipboard_rx,
            &mut deferred_actions,
            &mut copied_to_clipboard,
        );

        (
            left_mouse_button_pressed,
            copied_to_clipboard,
            deferred_actions,
        )
    }

    /// Apply config changes that can be hot-reloaded at runtime.
    ///
    /// Called when the user clicks "Apply" in the settings modal. Compares the
    /// old and new configs and updates font/cursor/theme state as needed.
    /// Returns `true` if the font or ligature config changed, meaning the
    /// caller must clear each pane's `RenderState::atlas` and
    /// `PaneRenderCache::invalidate_content()`.
    ///
    /// Note: this does NOT send a Resize event. When the font changes, the cell
    /// size changes too, and the normal resize detection in `FreminalGui::ui()`
    /// will detect the mismatch between `available_pixels / new_cell_size` and
    /// `view_state.last_sent_size` on the very next frame and send the correct
    /// `InputEvent::Resize` with proper character dimensions.
    pub fn apply_config_changes(
        &mut self,
        ctx: &egui::Context,
        old_config: &Config,
        new_config: &Config,
    ) -> bool {
        let pixels_per_point = ctx.pixels_per_point();
        let rebuild_result = self
            .font_manager
            .rebuild(new_config, pixels_per_point)
            .unwrap_or_else(|e| {
                error!("fatal: font manager rebuild failed during config apply: {e}");
                std::process::exit(1);
            });
        let ligatures_changed = old_config.font.ligatures != new_config.font.ligatures;
        let needs_pane_atlas_clear = rebuild_result.font_changed() || ligatures_changed;
        self.toggles = WidgetDisplayToggles {
            ligatures: new_config.font.ligatures,
            cursor_trail: new_config.cursor.trail,
            progress_enabled: new_config.progress.enabled,
        };
        self.cursor_trail_duration =
            Duration::from_millis(u64::from(new_config.cursor.trail_duration_ms));
        self.unfocused_cursor_style = new_config.cursor.unfocused_style;

        // Keep egui font infrastructure updated for chrome (menu bar, settings
        // modal).  This is retained from the old pipeline; it will be cleaned
        // up in subtask 1.9 once chrome fonts are fully migrated.
        let font_changed = old_config.font.family != new_config.font.family
            || (old_config.font.size - new_config.font.size).abs() > f32::EPSILON;
        if font_changed {
            let new_font_config = FontConfig {
                size: new_config.font.size,
                user_font: new_config.font.family.clone(),
                ..FontConfig::default()
            };
            self.base_font_defs = setup_font_files(ctx, &new_font_config);
        }
        needs_pane_atlas_clear
    }

    /// Apply config changes without an egui context.
    ///
    /// Used when the standalone settings window applies changes — the settings
    /// window's egui context is separate from terminal windows, so we cannot
    /// register chrome fonts here.  The font manager rebuild uses the
    /// last-known `pixels_per_point`.  Each terminal window will pick up the
    /// egui chrome font update on its next frame via `flush_egui_fonts_if_dirty`.
    pub fn apply_config_changes_no_ctx(
        &mut self,
        old_config: &Config,
        new_config: &Config,
    ) -> bool {
        let pixels_per_point = self.font_manager.pixels_per_point();
        let rebuild_result = self
            .font_manager
            .rebuild(new_config, pixels_per_point)
            .unwrap_or_else(|e| {
                error!("fatal: font manager rebuild failed during config apply (no-ctx): {e}");
                std::process::exit(1);
            });
        let ligatures_changed = old_config.font.ligatures != new_config.font.ligatures;
        let needs_pane_atlas_clear = rebuild_result.font_changed() || ligatures_changed;
        self.toggles = WidgetDisplayToggles {
            ligatures: new_config.font.ligatures,
            cursor_trail: new_config.cursor.trail,
            progress_enabled: new_config.progress.enabled,
        };
        self.cursor_trail_duration =
            Duration::from_millis(u64::from(new_config.cursor.trail_duration_ms));
        self.unfocused_cursor_style = new_config.cursor.unfocused_style;

        // Mark egui chrome fonts as needing update — will be applied on the
        // next frame when this window's update() runs with a real ctx.
        let font_changed = old_config.font.family != new_config.font.family
            || (old_config.font.size - new_config.font.size).abs() > f32::EPSILON;
        if font_changed {
            self.egui_fonts_dirty = true;
        }
        needs_pane_atlas_clear
    }

    /// Apply a font zoom by setting the font manager to `effective_size`.
    ///
    /// Clears the shared shaping cache if the size actually changed.
    /// Returns `true` if the font size changed. When this returns `true`,
    /// the caller must clear each pane's `RenderState::atlas` and
    /// `PaneRenderCache::invalidate_content()` so that all panes force a
    /// full vertex rebuild on the next frame.
    ///
    /// The resize event to the PTY is handled automatically by the existing
    /// resize-detection logic in the render loop (it compares
    /// `available_pixels / cell_size` against `view_state.last_sent_size`).
    pub fn apply_font_zoom(&mut self, effective_size: f32) -> bool {
        self.font_manager
            .set_font_size(effective_size)
            .unwrap_or_else(|e| {
                error!("fatal: font manager could not apply font zoom: {e}");
                std::process::exit(1);
            })
    }

    /// Live-preview an expensive font change (family and/or line height) via
    /// a full [`FontManager::rebuild`] (issue #452 phase C).
    ///
    /// Callers reserve this for changes that actually touch family or line
    /// height -- a size-only change should go through the cheaper
    /// [`Self::apply_font_zoom`] instead, exactly as it does for Ctrl+Scroll
    /// zoom. `FontManager::rebuild` reads only `config.font.family` /
    /// `config.font.size` / `config.font.line_height`, so it is safe to
    /// build a throwaway [`Config`] carrying just those three values rather
    /// than threading the caller's real (uncommitted) config through here.
    ///
    /// Returns `true` if any font-related state was invalidated, in which
    /// case the caller must clear each pane's `RenderState::atlas` and
    /// `PaneRenderCache::invalidate_content()`. As with
    /// [`Self::apply_config_changes`], no resize event is sent here -- the
    /// normal resize-detection logic notices the character-dimension
    /// mismatch on the next frame and sends the correct
    /// `InputEvent::Resize` on its own.
    pub fn apply_font_preview_rebuild(
        &mut self,
        family: Option<&str>,
        size: f32,
        line_height: f32,
    ) -> bool {
        let pixels_per_point = self.font_manager.pixels_per_point();
        let mut synthetic_config = Config::default();
        synthetic_config.font.family = family.map(str::to_owned);
        synthetic_config.font.size = size;
        synthetic_config.font.line_height = line_height;
        self.font_manager
            .rebuild(&synthetic_config, pixels_per_point)
            .unwrap_or_else(|e| {
                error!("fatal: font manager rebuild failed during font preview: {e}");
                std::process::exit(1);
            })
            .font_changed()
    }

    /// Live-preview the OpenType ligature toggle (issue #452 phase B)
    /// without a full [`FontManager::rebuild`].
    ///
    /// Ligatures only change which shaping features `rustybuzz` applies --
    /// no new fonts are loaded and no metrics change -- so calling the full
    /// config-apply path (which reparses font files) would be wasted work on
    /// every preview edit. This only flips the cached toggle `show()` reads
    /// each frame. Returns `true` when the value actually changed, so the
    /// caller knows whether to invalidate the pane atlases: a ligature
    /// change alters shaping output, so previously-shaped lines must
    /// re-shape to be visible.
    pub const fn set_ligatures_preview(&mut self, enabled: bool) -> bool {
        if self.toggles.ligatures == enabled {
            return false;
        }
        self.toggles.ligatures = enabled;
        true
    }

    /// Schedule the egui chrome fonts to be rebuilt from the GUI's config
    /// on this window's next frame (see [`Self::flush_egui_fonts_if_dirty`]).
    ///
    /// Used by the Settings font preview, which writes the previewed font
    /// into the GUI's config directly; without this the chrome would keep
    /// the old font, and a later Apply would see no font change to refresh.
    pub const fn mark_egui_fonts_dirty(&mut self) {
        self.egui_fonts_dirty = true;
    }

    /// Live-preview the OSC 9;4 progress-bar display toggle (issue #507).
    ///
    /// Only flips the cached toggle `show()` reads each frame. No atlas
    /// invalidation is needed: the bar is a plain painter overlay, and
    /// flipping the toggle while a bar is active changes the bar's damage
    /// fingerprint, so `show()` reports a bounded damage region for it on
    /// the next frame. Returns `true` when the value actually changed.
    pub const fn set_progress_enabled_preview(&mut self, enabled: bool) -> bool {
        if self.toggles.progress_enabled == enabled {
            return false;
        }
        self.toggles.progress_enabled = enabled;
        true
    }

    /// Live-preview the cursor trail toggle and its duration (issue #452
    /// phase B).
    ///
    /// Both are GUI-side render parameters `show()` reads each frame
    /// (`CursorFrameInputs::trail_enabled` / `CursorFrameInputs::trail_duration`)
    /// -- there is no PTY round trip and no glyph shaping is affected, so no
    /// atlas invalidation is needed either.
    pub fn set_cursor_trail_preview(&mut self, trail: bool, duration_ms: u32) {
        self.toggles.cursor_trail = trail;
        self.cursor_trail_duration = Duration::from_millis(u64::from(duration_ms));
    }

    /// Live-preview the unfocused-cursor style (Task 127.7).
    ///
    /// A GUI-side render parameter `show()` reads each frame (it resolves
    /// the inactive-pane / unfocused-window cursor appearance from this
    /// cached value) -- no PTY round trip, no shaping, no atlas
    /// invalidation.
    pub const fn set_unfocused_cursor_style_preview(&mut self, style: UnfocusedCursorStyle) {
        self.unfocused_cursor_style = style;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod visual_preview_tests {
    //! Tests for the cheap, GUI-side preview setters used by
    //! `apply_visual_preview` (issue #452 phase B): `set_ligatures_preview`
    //! and `set_cursor_trail_preview`. Both avoid the full
    //! `apply_config_changes[_no_ctx]` path, so they need their own
    //! coverage that the cached toggles actually change (or correctly
    //! report no change).
    //!
    //! Also covers `apply_font_preview_rebuild` (issue #452 phase C), the
    //! expensive counterpart applied only once the debounce in
    //! `visual_preview::DebouncedPreview` settles.

    use super::FreminalTerminalWidget;
    use freminal_common::config::Config;
    use std::time::Duration;

    fn widget() -> FreminalTerminalWidget {
        let ctx = egui::Context::default();
        FreminalTerminalWidget::new(&ctx, &Config::default())
            .expect("widget construction must succeed with the bundled default font")
    }

    #[test]
    fn set_ligatures_preview_updates_toggle_and_reports_change() {
        let mut widget = widget();
        assert!(widget.toggles.ligatures, "default config enables ligatures");

        assert!(
            !widget.set_ligatures_preview(true),
            "setting to the already-active value must report no change"
        );
        assert!(widget.toggles.ligatures);

        assert!(
            widget.set_ligatures_preview(false),
            "setting to a different value must report a change"
        );
        assert!(!widget.toggles.ligatures);
    }

    #[test]
    fn set_progress_enabled_preview_updates_toggle_and_reports_change() {
        let mut widget = widget();
        assert!(
            widget.toggles.progress_enabled,
            "default config enables the progress bar"
        );

        assert!(
            !widget.set_progress_enabled_preview(true),
            "setting to the already-active value must report no change"
        );
        assert!(widget.toggles.progress_enabled);

        assert!(
            widget.set_progress_enabled_preview(false),
            "setting to a different value must report a change"
        );
        assert!(!widget.toggles.progress_enabled);
    }

    #[test]
    fn set_cursor_trail_preview_updates_toggle_and_duration() {
        let mut widget = widget();

        widget.set_cursor_trail_preview(true, 250);
        assert!(widget.toggles.cursor_trail);
        assert_eq!(widget.cursor_trail_duration, Duration::from_millis(250));

        widget.set_cursor_trail_preview(false, 400);
        assert!(!widget.toggles.cursor_trail);
        assert_eq!(widget.cursor_trail_duration, Duration::from_millis(400));
    }

    #[test]
    fn set_unfocused_cursor_style_preview_updates_cached_style() {
        use freminal_common::config::UnfocusedCursorStyle;

        let mut widget = widget();
        assert_eq!(
            widget.unfocused_cursor_style,
            Config::default().cursor.unfocused_style
        );

        widget.set_unfocused_cursor_style_preview(UnfocusedCursorStyle::Hidden);
        assert_eq!(widget.unfocused_cursor_style, UnfocusedCursorStyle::Hidden);

        widget.set_unfocused_cursor_style_preview(UnfocusedCursorStyle::Unchanged);
        assert_eq!(
            widget.unfocused_cursor_style,
            UnfocusedCursorStyle::Unchanged
        );

        widget.set_unfocused_cursor_style_preview(UnfocusedCursorStyle::Hollow);
        assert_eq!(widget.unfocused_cursor_style, UnfocusedCursorStyle::Hollow);
    }

    #[test]
    fn apply_font_preview_rebuild_reports_change_on_size_only() {
        let mut widget = widget();
        let default_size = Config::default().font.size;

        assert!(
            !widget.apply_font_preview_rebuild(
                None,
                default_size,
                Config::default().font.line_height
            ),
            "re-applying the already-active values must report no change"
        );

        assert!(
            widget.apply_font_preview_rebuild(
                None,
                default_size + 4.0,
                Config::default().font.line_height
            ),
            "a genuine size change must report a change"
        );
    }

    #[test]
    fn apply_font_preview_rebuild_reports_change_on_line_height_only() {
        let mut widget = widget();
        let default = Config::default().font;

        assert!(
            widget.apply_font_preview_rebuild(None, default.size, default.line_height + 0.2),
            "a genuine line-height change must report a change"
        );
    }
}

/// Convert a [`PointerShape`] (from [`TerminalSnapshot`]) to the corresponding
/// [`egui::CursorIcon`].
///
/// [`PointerShape::Default`] and any value that has no direct egui equivalent
/// both produce [`CursorIcon::Default`].
/// Whether the pointer is over a pane-split drag sensor this frame.
///
/// The sensor rects are built and hit-tested in `app_impl`, which sets the
/// resize cursor before any pane renders. They are deliberately wider than
/// the 1px border line they straddle, which means the pointer sits
/// *geometrically inside* one of the two adjacent panes while *logically*
/// over chrome. A pane must therefore abstain from writing the cursor icon
/// here, or it overwrites the resize arrow for all but the hairline sliver
/// that falls between the two pane rects (issue #462).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitBorderHover {
    /// Pointer is over a split-border drag sensor; that chrome owns the icon.
    Over,
    /// Pointer is not over any split border.
    Clear,
}

/// What the mouse pointer is over, for the purpose of choosing a cursor icon.
///
/// Variants are listed in **descending precedence**: when several apply at
/// once, the earliest wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PointerTarget {
    /// The command-block gutter strip down the left edge of the pane. A
    /// clickable chrome affordance that sits outside the terminal grid, so it
    /// outranks anything the application asks for.
    CommandBlockGutter,
    /// A collapsed command-block fold placeholder row. Also chrome, also
    /// clickable, and drawn over the grid.
    FoldPlaceholder,
    /// A hyperlink in the terminal grid.
    Url,
    /// Ordinary terminal content -- the application's OSC 22 pointer shape
    /// applies, which is `Default` when it has not set one.
    TerminalContent,
}

/// Which cursor-icon sources are active this frame.
///
/// These are independent simultaneous observations -- the pointer can be over
/// a fold placeholder that happens to contain a URL -- so a set of bools is
/// the right representation. The *precedence* between them, which is the part
/// that was previously implicit and wrong, lives in [`Self::resolve`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct PointerHover {
    /// Pointer is over the command-block gutter strip.
    pub(super) command_block_gutter: bool,
    /// Pointer is over a collapsed fold placeholder row.
    pub(super) fold_placeholder: bool,
    /// Pointer is over a hyperlink.
    pub(super) url: bool,
}

impl PointerHover {
    /// Reduce the active sources to the single highest-precedence target.
    pub(super) const fn resolve(self) -> PointerTarget {
        if self.command_block_gutter {
            PointerTarget::CommandBlockGutter
        } else if self.fold_placeholder {
            PointerTarget::FoldPlaceholder
        } else if self.url {
            PointerTarget::Url
        } else {
            PointerTarget::TerminalContent
        }
    }
}

/// The cursor icon for a resolved [`PointerTarget`].
///
/// Chrome affordances all use the pointing hand; only ordinary terminal
/// content defers to the application's OSC 22 shape.
const fn cursor_icon_for(target: PointerTarget, osc22_shape: PointerShape) -> CursorIcon {
    match target {
        PointerTarget::CommandBlockGutter | PointerTarget::FoldPlaceholder | PointerTarget::Url => {
            CursorIcon::PointingHand
        }
        PointerTarget::TerminalContent => pointer_shape_to_cursor_icon(osc22_shape),
    }
}

const fn pointer_shape_to_cursor_icon(shape: PointerShape) -> CursorIcon {
    match shape {
        PointerShape::Default => CursorIcon::Default,
        PointerShape::None => CursorIcon::None,
        PointerShape::Text => CursorIcon::Text,
        PointerShape::VerticalText => CursorIcon::VerticalText,
        PointerShape::Pointer => CursorIcon::PointingHand,
        PointerShape::ContextMenu => CursorIcon::ContextMenu,
        PointerShape::Help => CursorIcon::Help,
        PointerShape::Progress => CursorIcon::Progress,
        PointerShape::Wait => CursorIcon::Wait,
        PointerShape::Cell => CursorIcon::Cell,
        PointerShape::Crosshair => CursorIcon::Crosshair,
        PointerShape::Move => CursorIcon::Move,
        PointerShape::NoDrop => CursorIcon::NoDrop,
        PointerShape::NotAllowed => CursorIcon::NotAllowed,
        PointerShape::Grab => CursorIcon::Grab,
        PointerShape::Grabbing => CursorIcon::Grabbing,
        PointerShape::Alias => CursorIcon::Alias,
        PointerShape::Copy => CursorIcon::Copy,
        PointerShape::AllScroll => CursorIcon::AllScroll,
        PointerShape::ResizeHorizontal => CursorIcon::ResizeHorizontal,
        PointerShape::ResizeVertical => CursorIcon::ResizeVertical,
        PointerShape::ResizeNeSw => CursorIcon::ResizeNeSw,
        PointerShape::ResizeNwSe => CursorIcon::ResizeNwSe,
        PointerShape::ResizeEast => CursorIcon::ResizeEast,
        PointerShape::ResizeSouthEast => CursorIcon::ResizeSouthEast,
        PointerShape::ResizeSouth => CursorIcon::ResizeSouth,
        PointerShape::ResizeSouthWest => CursorIcon::ResizeSouthWest,
        PointerShape::ResizeWest => CursorIcon::ResizeWest,
        PointerShape::ResizeNorthWest => CursorIcon::ResizeNorthWest,
        PointerShape::ResizeNorth => CursorIcon::ResizeNorth,
        PointerShape::ResizeNorthEast => CursorIcon::ResizeNorthEast,
        PointerShape::ZoomIn => CursorIcon::ZoomIn,
        PointerShape::ZoomOut => CursorIcon::ZoomOut,
    }
}

/// POSIX shell-escape a file path for safe pasting into a terminal.
///
/// Wraps the path in single quotes and escapes any embedded single quotes
/// with the `'\''` idiom.  The result is safe to paste into `sh`, `bash`,
/// `zsh`, and `fish`.
fn shell_escape_path(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Build the PTY payload for a set of dropped file paths: each path
/// shell-escaped, space-separated, with a trailing space so the shell treats
/// the insertion as a finished argument list the user can keep typing after.
///
/// Returns an empty string when there is nothing to send, which the caller
/// uses as the "don't write to the PTY at all" signal.
///
/// **Empty paths are skipped.** egui 0.36 changed `dropped_files` to a
/// `Vec<Arc<dyn DroppedFile>>` whose `path()` returns a plain `&Path`, where
/// the pre-0.36 API had an `Option<PathBuf>` that this code gated on. On native
/// that `Option` was always `Some` (`egui-winit` fills it straight from the
/// winit event), but skipping empties preserves the old gate's behaviour for
/// any source that cannot supply a real filesystem path — without it such an
/// entry would shell-escape to a literal `''` and inject an empty argument into
/// the user's command line.
///
/// Split out of [`handle_file_drop`] because that function needs a live
/// `egui::Ui` and so cannot be called from a test, while this assembly step
/// carries all the behaviour worth pinning (escaping, separator placement, the
/// trailing space, and the empty-path skip).
fn dropped_files_payload<'a>(paths: impl IntoIterator<Item = &'a std::path::Path>) -> String {
    let mut payload = String::new();
    for path in paths {
        if path.as_os_str().is_empty() {
            continue;
        }
        // Guarded on `payload`, not on the loop index: with empty paths
        // skipped, an index-based check would emit a leading separator
        // whenever the first entry was the one skipped.
        if !payload.is_empty() {
            payload.push(' ');
        }
        payload.push_str(&shell_escape_path(path));
    }
    if !payload.is_empty() {
        payload.push(' ');
    }
    payload
}

/// Handle file drag-and-drop events on the terminal area.
///
/// **Drop:** Shell-escapes each dropped file path and sends the result as
/// keyboard input to the PTY (space-separated, with a trailing space). The
/// payload assembly itself lives in [`dropped_files_payload`], which is where
/// its behaviour is tested.
///
/// **Hover:** Draws a semi-transparent overlay with a "Drop files here" label
/// while files are being dragged over the terminal area.
fn handle_file_drop(ui: &Ui, terminal_rect: Rect, input_tx: &Sender<InputEvent>) {
    // Only handle drops/hovers when the pointer is over the terminal area.
    // `raw.dropped_files` / `raw.hovered_files` are window-global, so without
    // this gate a drop on the menu bar or settings modal would inject paths.
    let pointer_over_terminal = ui.ctx().input(|i| {
        i.pointer
            .hover_pos()
            .is_some_and(|p| terminal_rect.contains(p))
    });

    // ── Drop handling ────────────────────────────────────────────────
    let dropped_files = ui.ctx().input(|i| i.raw.dropped_files.clone());
    if pointer_over_terminal && !dropped_files.is_empty() {
        let payload = dropped_files_payload(dropped_files.iter().map(|file| file.path()));
        if !payload.is_empty() {
            send_or_log!(
                input_tx,
                InputEvent::Key(payload.into_bytes()),
                "Failed to send dropped file paths to PTY"
            );
        }
    }

    // ── Hover overlay ────────────────────────────────────────────────
    let hovered_files = ui.ctx().input(|i| i.raw.hovered_files.clone());
    if pointer_over_terminal && !hovered_files.is_empty() {
        let overlay_color = Color32::from_rgba_premultiplied(0, 0, 0, 160);
        ui.painter().rect_filled(terminal_rect, 0.0, overlay_color);
        ui.painter().text(
            terminal_rect.center(),
            egui::Align2::CENTER_CENTER,
            "Drop files here",
            egui::FontId::proportional(20.0),
            Color32::WHITE,
        );
    }
}

#[cfg(test)]
mod gutter_hover_repaint_decision_tests {
    //! Tests for [`gutter_hover_repaint_decision`], the pure decision
    //! function behind the command-block gutter's hover-tint repaint
    //! logic. Covers the steady-state / enter / leave-within-window cases
    //! plus the critical regression case: a same-frame pointer-left-window
    //! exit must be detected immediately via `pointer_in_window`, without
    //! waiting for `hovered()`'s one-frame `interact_pos()` lag to catch up
    //! (see the function doc comment for the full egui behavior citation).

    use super::gutter_hover_repaint_decision;

    #[test]
    fn steady_hover_no_change() {
        assert_eq!(
            gutter_hover_repaint_decision(true, true, true),
            (false, true)
        );
    }

    #[test]
    fn steady_not_hovering_no_change() {
        assert_eq!(
            gutter_hover_repaint_decision(false, true, false),
            (false, false)
        );
    }

    #[test]
    fn enter_transition_fires_and_now_hovering() {
        assert_eq!(
            gutter_hover_repaint_decision(true, true, false),
            (true, true)
        );
    }

    #[test]
    fn leave_within_window_transition_fires_and_now_not_hovering() {
        // Pointer is still inside the window, but has moved off the
        // gutter's interact rect.
        assert_eq!(
            gutter_hover_repaint_decision(false, true, true),
            (true, false)
        );
    }

    #[test]
    fn pointer_left_window_fires_immediately_despite_stale_hovered() {
        // Regression guard: `hovered` is stale-`true` here (simulating
        // egui's one-frame `interact_pos()` lag on `Event::PointerGone`),
        // but `pointer_in_window` correctly reports `false` in the same
        // frame the cursor left the OS window. The decision must fire the
        // repaint immediately and clear the cached hover state -- it must
        // NOT require a follow-up frame to observe the real state, since
        // nothing else is guaranteed to wake one.
        assert_eq!(
            gutter_hover_repaint_decision(true, false, true),
            (true, false)
        );
    }

    #[test]
    fn steady_state_after_window_exit_settled_no_more_repaints() {
        assert_eq!(
            gutter_hover_repaint_decision(false, false, false),
            (false, false)
        );
    }
}

#[cfg(test)]
mod scrollbar_damage_decision_tests {
    //! Tests for [`scrollbar_damage_decision`], the pure decision function
    //! behind the scrollbar thumb's hover/visibility repaint + Full-present
    //! forcing (#459 item 9, mirroring #461's gutter fix). Covers the
    //! rendered<->not-rendered vanish/appear transitions, the hover-alpha
    //! change while rendered, and the steady-state no-op cases — including
    //! the case where hover *would* differ but the thumb isn't rendered at
    //! all, which must be ignored (governed only by visibility).

    use super::{ScrollbarDamageState, scrollbar_damage_decision};

    /// Test-only shorthand for building a [`ScrollbarDamageState`].
    const fn state(rendered: bool, effectively_hovered: bool) -> ScrollbarDamageState {
        ScrollbarDamageState {
            rendered,
            effectively_hovered,
        }
    }

    #[test]
    fn rendered_to_not_rendered_forces_damage() {
        // Scrolled to bottom: thumb was visible last frame, gone this frame.
        assert!(scrollbar_damage_decision(
            state(false, false),
            state(true, false)
        ));
    }

    #[test]
    fn not_rendered_to_rendered_forces_damage() {
        // Scrolled back into history: thumb appears this frame.
        assert!(scrollbar_damage_decision(
            state(true, false),
            state(false, false)
        ));
    }

    #[test]
    fn hover_enter_while_rendered_forces_damage() {
        assert!(scrollbar_damage_decision(
            state(true, true),
            state(true, false)
        ));
    }

    #[test]
    fn hover_leave_while_rendered_forces_damage() {
        assert!(scrollbar_damage_decision(
            state(true, false),
            state(true, true)
        ));
    }

    #[test]
    fn steady_visible_unhovered_no_damage() {
        assert!(!scrollbar_damage_decision(
            state(true, false),
            state(true, false)
        ));
    }

    #[test]
    fn steady_visible_hovered_no_damage() {
        assert!(!scrollbar_damage_decision(
            state(true, true),
            state(true, true)
        ));
    }

    #[test]
    fn steady_hidden_no_damage() {
        assert!(!scrollbar_damage_decision(
            state(false, false),
            state(false, false)
        ));
    }

    #[test]
    fn hover_change_while_not_rendered_is_ignored() {
        // Not rendered both frames, "hover" bit differs -- irrelevant since
        // visibility governs when the thumb isn't rendered at all.
        assert!(!scrollbar_damage_decision(
            state(false, true),
            state(false, false)
        ));
        assert!(!scrollbar_damage_decision(
            state(false, false),
            state(false, true)
        ));
    }
}

#[cfg(test)]
mod bell_flash_tests {
    //! Tests for [`bell_flash_outcome`], the pure decision function behind
    //! [`paint_bell_flash`]. Covers the fade/clear/persistent boundaries and
    //! guards the regression where a bell overlay got stuck forever: the
    //! overlay must clear once focused and the flash duration has elapsed,
    //! *regardless* of how the caller learned `window_focused` — the fix
    //! removed the only source of staleness (a per-pane cached flag) by
    //! deleting `ViewState::window_focused` entirely and requiring callers
    //! to pass a live-queried value instead.

    use super::{
        BELL_FLASH_DURATION, BELL_FLASH_MAX_ALPHA, BELL_PERSISTENT_ALPHA, BellFlashOutcome,
        bell_flash_outcome, bell_flash_repaint_delay,
    };
    use std::time::Duration;

    #[test]
    fn focused_fresh_bell_fades_from_max_alpha() {
        let outcome = bell_flash_outcome(true, Duration::from_millis(0));
        assert_eq!(
            outcome,
            BellFlashOutcome::Fading {
                alpha: BELL_FLASH_MAX_ALPHA
            }
        );
    }

    #[test]
    fn focused_partway_through_fade_has_reduced_alpha() {
        let half = BELL_FLASH_DURATION / 2;
        let BellFlashOutcome::Fading { alpha } = bell_flash_outcome(true, half) else {
            panic!("expected Fading at the halfway point");
        };
        assert!(
            alpha > 0 && alpha < BELL_FLASH_MAX_ALPHA,
            "alpha {alpha} should be strictly between 0 and max at the halfway point"
        );
    }

    #[test]
    fn focused_exactly_at_duration_clears() {
        // Regression guard: this is the boundary that must actually clear.
        // A stuck bell would show as this never returning `Cleared`.
        assert_eq!(
            bell_flash_outcome(true, BELL_FLASH_DURATION),
            BellFlashOutcome::Cleared
        );
    }

    #[test]
    fn focused_past_duration_clears() {
        assert_eq!(
            bell_flash_outcome(true, BELL_FLASH_DURATION + Duration::from_secs(1)),
            BellFlashOutcome::Cleared
        );
        // Also true for a bell that has been "stuck" for a long time (e.g.
        // the pre-fix scenario of an entire session) — once the caller
        // passes the correct live `window_focused = true`, it clears
        // immediately rather than requiring a fresh focus *transition*.
        assert_eq!(
            bell_flash_outcome(true, Duration::from_hours(1)),
            BellFlashOutcome::Cleared
        );
    }

    #[test]
    fn unfocused_is_persistent_regardless_of_elapsed() {
        assert_eq!(
            bell_flash_outcome(false, Duration::from_millis(0)),
            BellFlashOutcome::Persistent {
                alpha: BELL_PERSISTENT_ALPHA
            }
        );
        assert_eq!(
            bell_flash_outcome(false, Duration::from_hours(1)),
            BellFlashOutcome::Persistent {
                alpha: BELL_PERSISTENT_ALPHA
            }
        );
    }

    // ── Subtask 121.12: `bell_flash_repaint_delay` ────────────────────────
    // `paint_bell_flash` itself needs a live `egui::Ui` and cannot be driven
    // headlessly, so its delay-selection logic is factored into this pure
    // function and pinned directly here.

    #[test]
    fn fading_outcome_wants_a_16ms_repaint() {
        assert_eq!(
            bell_flash_repaint_delay(BellFlashOutcome::Fading {
                alpha: BELL_FLASH_MAX_ALPHA
            }),
            Some(Duration::from_millis(16))
        );
    }

    #[test]
    fn persistent_outcome_wants_no_repaint() {
        assert_eq!(
            bell_flash_repaint_delay(BellFlashOutcome::Persistent {
                alpha: BELL_PERSISTENT_ALPHA
            }),
            None
        );
    }

    #[test]
    fn cleared_outcome_wants_no_repaint() {
        assert_eq!(bell_flash_repaint_delay(BellFlashOutcome::Cleared), None);
    }
}

#[cfg(test)]
mod patch_cursor_only_deco_verts_tests {
    //! Tests for [`patch_cursor_only_deco_verts`], the cursor-only
    //! decoration-buffer patch, and the [`CursorVertRange`] bookkeeping that
    //! drives it.
    //!
    //! The cursor's length is not fixed (0 floats hidden / blink-off,
    //! `CURSOR_QUAD_FLOATS` solid, `4 * CURSOR_QUAD_FLOATS` hollow), so the
    //! patch truncates to the recorded cursor start and appends. The
    //! integration-style tests here use the real vertex builders as the
    //! reference: after every patch, `deco_verts` must be bit-identical to a
    //! from-scratch full rebuild with the same cursor params. That is the
    //! invariant whose violation was issue #432 (stale offset bookkeeping
    //! overwriting the bottom row's selection quad).

    use super::{CursorVertRange, patch_cursor_only_deco_verts};
    use crate::gui::renderer::{
        BackgroundFrame, CURSOR_QUAD_FLOATS, CursorBlinkPhase, CursorDrawParams,
        build_background_instances, build_cursor_verts_only,
    };
    use crate::gui::terminal::cursor_appearance::CursorAppearance;
    use crate::gui::{colors::cursor_f, shaping::ShapedLine};
    use freminal_common::{cursor::CursorVisualStyle, themes::CATPPUCCIN_MOCHA};
    use freminal_terminal_emulator::LineWidth;
    use std::sync::Arc;

    const CELL_W: u32 = 8;
    const CELL_H: u32 = 16;

    fn params(appearance: CursorAppearance, blink_on: CursorBlinkPhase) -> CursorDrawParams {
        CursorDrawParams {
            appearance,
            col: 2.0,
            row: 1.0,
            color: cursor_f(&CATPPUCCIN_MOCHA, None),
            x_scale: 1.0,
            blink_on,
        }
    }

    fn solid() -> CursorDrawParams {
        params(
            CursorAppearance::Solid(CursorVisualStyle::BlockCursorSteady),
            CursorBlinkPhase::On,
        )
    }

    fn hollow() -> CursorDrawParams {
        params(CursorAppearance::Hollow, CursorBlinkPhase::On)
    }

    fn hidden() -> CursorDrawParams {
        params(CursorAppearance::Hidden, CursorBlinkPhase::On)
    }

    /// A blinking cursor caught in its off phase: nothing is drawn.
    fn blink_off() -> CursorDrawParams {
        params(
            CursorAppearance::Solid(CursorVisualStyle::BlockCursorBlink),
            CursorBlinkPhase::Off,
        )
    }

    /// A from-scratch full rebuild over a two-row screen whose rows are both
    /// selected, so `deco` has non-cursor quads (the bottom row's selection
    /// quad is the last one before the cursor, the quad #432 corrupted).
    fn full_rebuild(cursor: &CursorDrawParams) -> (Vec<f32>, CursorVertRange) {
        full_rebuild_with_selection(cursor, Some((0, 0, 2, 1)))
    }

    /// A from-scratch full rebuild of a plain-shell screen: no selection,
    /// underline, or search quads, so the cursor is the ONLY content of
    /// `deco_verts` (empty whenever it is blink-off or hidden).
    fn plain_shell_rebuild(cursor: &CursorDrawParams) -> (Vec<f32>, CursorVertRange) {
        full_rebuild_with_selection(cursor, None)
    }

    fn full_rebuild_with_selection(
        cursor: &CursorDrawParams,
        selection: Option<(usize, usize, usize, usize)>,
    ) -> (Vec<f32>, CursorVertRange) {
        let line = Arc::new(ShapedLine {
            runs: Vec::new(),
            line_width: LineWidth::Normal,
        });
        let lines = [Arc::clone(&line), line];
        let mut instances = Vec::new();
        let mut deco = Vec::new();
        let range = build_background_instances(
            &BackgroundFrame {
                shaped_lines: &lines,
                cell_width: CELL_W,
                cell_height: CELL_H,
                ascent: 14.0,
                underline_offset: 13.0,
                strikeout_offset: 8.0,
                stroke_size: 1.0,
                cursor,
                selection,
                selection_is_block: false,
                match_highlights: &[],
                command_block_hover_rows: None,
                term_width_cols: 0,
                theme: &CATPPUCCIN_MOCHA,
                reverse_screen: false,
            },
            &mut instances,
            &mut deco,
        );
        (deco, range)
    }

    fn bits(v: &[f32]) -> Vec<u32> {
        v.iter().map(|f| f.to_bits()).collect()
    }

    /// Apply a cursor-only frame the way the widget does and return the new
    /// stored range.
    fn cursor_only_frame(
        deco: &mut Vec<f32>,
        range: CursorVertRange,
        cursor: &CursorDrawParams,
    ) -> CursorVertRange {
        let verts = build_cursor_verts_only(CELL_W, CELL_H, cursor);
        patch_cursor_only_deco_verts(deco, Some(range), &verts)
    }

    /// Assert the patched state is bit-identical to a full rebuild, that the
    /// recorded range matches the rebuild's, and that the cursor is the final
    /// region of the buffer.
    fn assert_matches_full_rebuild(
        deco: &[f32],
        range: CursorVertRange,
        cursor: &CursorDrawParams,
        label: &str,
    ) {
        let (expected, expected_range) = full_rebuild(cursor);
        assert_eq!(bits(deco), bits(&expected), "{label}: deco_verts differ");
        assert_eq!(range, expected_range, "{label}: stored range differs");
        assert_eq!(
            range.start + range.len,
            deco.len(),
            "{label}: the cursor must be the final region of deco_verts"
        );
    }

    /// Replaces the old `hides_cursor_by_zeroing_a_reserved_region`: hiding
    /// is now a truncate. Nothing is zeroed in place and the non-cursor prefix
    /// is untouched.
    #[test]
    fn hiding_the_cursor_truncates_to_the_range_start() {
        let mut deco = vec![1.0; CURSOR_QUAD_FLOATS]; // selection quad, say
        deco.extend(vec![9.0; CURSOR_QUAD_FLOATS]); // solid cursor
        let range = CursorVertRange {
            start: CURSOR_QUAD_FLOATS,
            len: CURSOR_QUAD_FLOATS,
        };

        let new_range = patch_cursor_only_deco_verts(&mut deco, Some(range), &[]);

        assert_eq!(
            new_range,
            CursorVertRange {
                start: CURSOR_QUAD_FLOATS,
                len: 0
            }
        );
        assert_eq!(deco, vec![1.0; CURSOR_QUAD_FLOATS], "cursor must be gone");
    }

    /// Replaces the old `overwrites_a_reserved_region_in_place` and
    /// `appends_cursor_quad_when_no_tail_was_reserved`: one path now covers
    /// both, so the same-length replace and the zero-length append are
    /// asserted together, and so is a length-changing replace (solid -> hollow)
    /// the old in-place overwrite could not express.
    #[test]
    fn replaces_the_cursor_region_whatever_its_old_and_new_length() {
        let prefix = vec![1.0; CURSOR_QUAD_FLOATS];
        let solid_quad = vec![9.0; CURSOR_QUAD_FLOATS];
        let hollow_quads = vec![7.0; 4 * CURSOR_QUAD_FLOATS];
        let start = prefix.len();

        // (old cursor floats, new cursor floats)
        let cases: [(&[f32], &[f32]); 4] = [
            (&solid_quad, &solid_quad),   // same length, new content below
            (&[], &solid_quad),           // append (old: no reserved tail)
            (&solid_quad, &hollow_quads), // grow
            (&hollow_quads, &solid_quad), // shrink
        ];
        for (old, new) in cases {
            let mut deco = prefix.clone();
            deco.extend_from_slice(old);
            let range = CursorVertRange {
                start,
                len: old.len(),
            };

            let new_range = patch_cursor_only_deco_verts(&mut deco, Some(range), new);

            assert_eq!(
                new_range,
                CursorVertRange {
                    start,
                    len: new.len()
                }
            );
            assert_eq!(deco[..start], prefix[..], "prefix must be untouched");
            assert_eq!(deco[start..], *new, "cursor region must be the new floats");
        }
    }

    /// Replaces the old `no_reserved_tail_and_cursor_hidden_is_a_no_op`:
    /// hiding an already-hidden cursor changes nothing and keeps the range.
    #[test]
    fn hiding_an_already_hidden_cursor_is_a_no_op() {
        let mut deco = vec![1.0; CURSOR_QUAD_FLOATS];
        let original = deco.clone();
        let range = CursorVertRange {
            start: deco.len(),
            len: 0,
        };

        let new_range = patch_cursor_only_deco_verts(&mut deco, Some(range), &[]);

        assert_eq!(deco, original);
        assert_eq!(new_range, range);
    }

    /// A start past the end cannot come from the bookkeeping; it is clamped
    /// to the buffer length instead of panicking or leaving a gap.
    #[test]
    fn out_of_range_start_is_clamped_to_the_buffer_length() {
        let mut deco = vec![1.0; CURSOR_QUAD_FLOATS];
        let range = CursorVertRange {
            start: CURSOR_QUAD_FLOATS * 10,
            len: CURSOR_QUAD_FLOATS,
        };
        let cursor = vec![9.0; CURSOR_QUAD_FLOATS];

        let new_range = patch_cursor_only_deco_verts(&mut deco, Some(range), &cursor);

        assert_eq!(
            new_range,
            CursorVertRange {
                start: CURSOR_QUAD_FLOATS,
                len: CURSOR_QUAD_FLOATS
            }
        );
        assert_eq!(deco.len(), CURSOR_QUAD_FLOATS * 2);
        assert_eq!(deco[CURSOR_QUAD_FLOATS..], cursor[..]);
    }

    /// Consecutive cursor-only frames of changing length (solid -> hollow ->
    /// hidden -> solid -> blink-off -> hollow) leave `deco_verts` identical to
    /// a full rebuild after every step, with the cursor always last.
    #[test]
    fn consecutive_cursor_only_frames_match_a_full_rebuild_each_time() {
        let (mut deco, mut range) = full_rebuild(&solid());
        assert_matches_full_rebuild(&deco, range, &solid(), "initial solid rebuild");
        assert_eq!(range.len, CURSOR_QUAD_FLOATS);

        let steps = [
            ("solid -> hollow", hollow(), 4 * CURSOR_QUAD_FLOATS),
            ("hollow -> hidden", hidden(), 0),
            ("hidden -> solid", solid(), CURSOR_QUAD_FLOATS),
            ("solid -> blink-off", blink_off(), 0),
            ("blink-off -> hollow", hollow(), 4 * CURSOR_QUAD_FLOATS),
            ("hollow -> solid", solid(), CURSOR_QUAD_FLOATS),
        ];
        for (label, cursor, expected_len) in steps {
            range = cursor_only_frame(&mut deco, range, &cursor);
            assert_eq!(range.len, expected_len, "{label}: cursor length");
            assert_matches_full_rebuild(&deco, range, &cursor, label);
        }
    }

    /// The same chain, but the cursor moves between frames too: position is
    /// part of the cursor floats, not of the range.
    #[test]
    fn cursor_only_frames_track_a_moving_cursor() {
        let (mut deco, mut range) = full_rebuild(&solid());
        let mut moved = hollow();
        moved.col = 5.0;
        moved.row = 0.0;

        range = cursor_only_frame(&mut deco, range, &moved);

        assert_matches_full_rebuild(&deco, range, &moved, "moved hollow");
    }

    /// Issue #432-style regression: a full rebuild lands between cursor-only
    /// frames of differing lengths. The rebuild's blink-off cursor has length
    /// 0, so the stored range must be `{ start: len, len: 0 }`; a later
    /// cursor-only patch must append after it and never overwrite the bottom
    /// row's selection quad (the last non-cursor quad).
    #[test]
    fn full_rebuild_between_cursor_only_frames_of_different_lengths() {
        let (prefix, _) = full_rebuild(&hidden());
        assert_eq!(
            prefix.len(),
            2 * CURSOR_QUAD_FLOATS,
            "fixture: one selection quad per row"
        );
        let bottom_row_quad = prefix[CURSOR_QUAD_FLOATS..].to_vec();

        // Full rebuild with a visible cursor, then a hollow cursor-only patch
        // (4x the length of the rebuild's cursor).
        let (mut deco, mut range) = full_rebuild(&solid());
        range = cursor_only_frame(&mut deco, range, &hollow());
        assert_matches_full_rebuild(&deco, range, &hollow(), "hollow cursor-only");

        // A full rebuild now runs on the cursor's blink-off phase: the cursor
        // has length 0 and the tail of deco is the bottom-row selection quad.
        let (rebuilt, rebuilt_range) = full_rebuild(&blink_off());
        deco = rebuilt;
        range = rebuilt_range;
        assert_eq!(
            range,
            CursorVertRange {
                start: deco.len(),
                len: 0
            }
        );
        assert_eq!(deco[CURSOR_QUAD_FLOATS..], bottom_row_quad[..]);

        // Cursor-only frame with the solid cursor visible again.
        range = cursor_only_frame(&mut deco, range, &solid());

        assert_eq!(
            deco[..prefix.len()],
            prefix[..],
            "non-cursor quads must be untouched"
        );
        assert_eq!(
            deco[CURSOR_QUAD_FLOATS..prefix.len()],
            bottom_row_quad[..],
            "the bottom-row selection quad must survive"
        );
        assert_matches_full_rebuild(&deco, range, &solid(), "solid after blink-off rebuild");

        // And back to hollow, then hidden, off the same bookkeeping.
        range = cursor_only_frame(&mut deco, range, &hollow());
        assert_matches_full_rebuild(&deco, range, &hollow(), "hollow after solid");
        range = cursor_only_frame(&mut deco, range, &hidden());
        assert_matches_full_rebuild(&deco, range, &hidden(), "hidden after hollow");
    }

    /// Regression for the "empty prefix" defect: on a plain-shell pane the
    /// cursor is the only decoration, so a blink-off patch leaves `deco_verts`
    /// EMPTY. The stored range must stay `Some` (the render state is still
    /// built -- only the buffer is empty), and each blink-on / blink-off
    /// patch must equal a from-scratch rebuild, however many times it
    /// repeats.
    #[test]
    fn blink_cycles_on_an_empty_prefix_match_a_full_rebuild_each_time() {
        let (mut deco, range) = plain_shell_rebuild(&solid());
        let mut stored: Option<CursorVertRange> = Some(range);
        assert_eq!(range.start, 0, "fixture: the cursor is the whole buffer");

        for cycle in 0..3 {
            for (label, cursor) in [("blink-off", blink_off()), ("blink-on", solid())] {
                let verts = build_cursor_verts_only(CELL_W, CELL_H, &cursor);
                stored = Some(patch_cursor_only_deco_verts(&mut deco, stored, &verts));

                let (expected, expected_range) = plain_shell_rebuild(&cursor);
                let label = format!("cycle {cycle} {label}");
                assert_eq!(bits(&deco), bits(&expected), "{label}: deco_verts differ");
                assert_eq!(
                    stored,
                    Some(expected_range),
                    "{label}: stored range must stay Some and match the rebuild"
                );
            }
        }
        // The blink-off half really did empty the buffer, so this exercises
        // the case a `deco_verts.is_empty()` "never built" test misreads.
        let verts = build_cursor_verts_only(CELL_W, CELL_H, &blink_off());
        stored = Some(patch_cursor_only_deco_verts(&mut deco, stored, &verts));
        assert_eq!(deco, Vec::<f32>::new());
        assert_eq!(stored, Some(CursorVertRange { start: 0, len: 0 }));
    }

    /// Defensive `None` handling (never selected in practice): the cursor is
    /// appended after whatever is present and the returned range describes it.
    #[test]
    fn patching_with_no_recorded_range_appends_after_existing_data() {
        let mut deco = vec![1.0; CURSOR_QUAD_FLOATS];
        let cursor = vec![9.0; CURSOR_QUAD_FLOATS];

        let range = patch_cursor_only_deco_verts(&mut deco, None, &cursor);

        assert_eq!(
            range,
            CursorVertRange {
                start: CURSOR_QUAD_FLOATS,
                len: CURSOR_QUAD_FLOATS
            }
        );
        assert_eq!(deco.len(), 2 * CURSOR_QUAD_FLOATS);
    }
}

#[cfg(test)]
mod subtask_1_7_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// Verify that an empty `RenderState` has empty vertex buffers.
    ///
    /// This confirms that `skip_draw` leaves the existing (initially empty)
    /// vertex buffers untouched rather than calling the vertex-build path.
    #[test]
    fn skip_draw_leaves_verts_empty() {
        let rs = RenderState {
            renderer: TerminalRenderer::new(),
            atlas: GlyphAtlas::default(),
            bg_instances: Vec::new(),
            deco_verts: Vec::new(),
            fg_instances: Vec::new(),
            cursor_vert_range: None,
            image_verts: Vec::new(),
            image_draw_order: Vec::new(),
            snap_images: std::collections::HashMap::new(),
            cell_width_px: 0.0,
            cell_height_px: 0.0,
            bg_opacity: 1.0,
            bg_image_opacity: 0.5,
            bg_image_mode: freminal_common::config::BackgroundImageMode::Cover,
            window_post: Arc::new(Mutex::new(WindowPostRenderer::new())),
            pending_bg_image: None,
            #[cfg(feature = "frame-profiling")]
            live_profile: LiveRenderProfile::new(),
            retire_queue: GlRetireQueue::new(),
        };
        assert!(rs.bg_instances.is_empty(), "bg_instances should be empty");
        assert!(rs.deco_verts.is_empty(), "deco_verts should be empty");
        assert!(rs.fg_instances.is_empty(), "fg_instances should be empty");
    }

    /// Verify that `FontManager::cell_size()` returns non-zero dimensions for
    /// the default config (bundled `CaskaydiaCove` Nerd Font).
    #[test]
    fn cell_size_from_font_manager_is_nonzero() {
        let config = freminal_common::config::Config::default();
        let fm = FontManager::new(&config, 1.0).unwrap();
        let (w, h) = fm.cell_size();
        assert!(w > 0, "cell_width must be non-zero, got {w}");
        assert!(h > 0, "cell_height must be non-zero, got {h}");
    }

    #[test]
    fn truncate_url_no_truncation_when_short() {
        let url = "https://example.com";
        let result = super::truncate_url(url, 40);
        assert_eq!(result, url);
    }

    #[test]
    fn truncate_url_truncates_long_ascii() {
        let url = "https://example.com/very/long/path/that/exceeds/the/limit";
        let result = super::truncate_url(url, 20);
        assert_eq!(result.chars().count(), 21); // 20 chars + ellipsis
        assert!(result.ends_with('…'));
        assert!(result.starts_with("https://example.com/"));
    }

    #[test]
    fn truncate_url_safe_with_multibyte_utf8() {
        // Each char here is multi-byte in UTF-8 (3 bytes each for CJK).
        let url = "https://例え.jp/パス/テスト";
        // Should not panic when truncation falls on a multi-byte boundary.
        let result = super::truncate_url(url, 12);
        assert!(result.ends_with('…'));
        assert_eq!(result.chars().count(), 13); // 12 chars + ellipsis
    }

    #[test]
    fn truncate_url_exact_boundary() {
        let url = "abcde";
        // Exactly at the limit — no truncation.
        assert_eq!(super::truncate_url(url, 5), "abcde");
        // One over — truncates.
        assert_eq!(super::truncate_url(url, 4), "abcd…");
    }

    #[test]
    fn observe_row_epochs_reports_new_only_on_genuine_change() {
        // Issue #439 fix #4 (Task 124.12 revision): the repaint scheduler
        // gates the content-driven 16ms wake on this returning `true` only
        // when a genuinely-new snapshot is observed. Re-observing the SAME
        // `Arc` (the idle frames between real PTY updates) must return
        // `false` so the wake is not re-armed.
        let mut cache = PaneRenderCache::new();
        let epochs_a: Arc<[u64]> = Arc::from([1_u64, 2]);
        let epochs_b: Arc<[u64]> = Arc::from([3_u64, 4]);

        // First ever observation of any snapshot is new.
        assert!(
            cache.observe_row_epochs(&epochs_a),
            "first observation of a snapshot must report new"
        );
        // Re-observing the SAME Arc is NOT new (the idle re-read case).
        assert!(
            !cache.observe_row_epochs(&epochs_a),
            "re-observing the same Arc must report not-new"
        );
        assert!(
            !cache.observe_row_epochs(&epochs_a),
            "still not-new on a third re-read of the same Arc"
        );
        // A different allocation with different epoch values is new again.
        assert!(
            cache.observe_row_epochs(&epochs_b),
            "observing row_epochs with different values must report new"
        );
        assert!(
            !cache.observe_row_epochs(&epochs_b),
            "re-observing the new row_epochs must then report not-new"
        );
        // A value-identical but distinct allocation is NOT "new" — this is
        // the fix over the old `Arc::ptr_eq`-based `visible_chars` test:
        // comparing epoch *values* (not pointer identity) suppresses the
        // spurious repaint a byte-identical re-flatten in a fresh Arc used
        // to cause (e.g. a cursor-blink repaint).
        let epochs_b_clone_values: Arc<[u64]> = Arc::from([3_u64, 4]);
        assert!(
            !cache.observe_row_epochs(&epochs_b_clone_values),
            "a distinct allocation with identical epoch values is NOT a new \
             observation"
        );
    }
}

/// Task 125.5: `RenderState::live_profile`'s token lifecycle through the
/// exact `Arc<Mutex<RenderState>>` API `show()` and the pane `PaintCallback`
/// share — a fresh, GL-context-free `RenderState` locked twice (once per
/// side), never a real rendered frame.
#[cfg(all(test, feature = "frame-profiling"))]
mod live_profiling_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::gui::renderer::profiling::{PaneFrameOutcome, RenderWorkClass};

    /// A freshly constructed pane's `live_profile` starts with no pending
    /// token and zero observations — mirrors a pane's very first `show()`
    /// call.
    #[test]
    fn fresh_render_state_has_no_pending_profiling_token() {
        let render_state = new_render_state(Arc::new(Mutex::new(WindowPostRenderer::new())));
        let rs = render_state.lock().unwrap();
        assert_eq!(rs.live_profile.pending(), None);
        assert_eq!(rs.live_profile.observation_count(), 0);
        drop(rs);
    }

    /// End-to-end token lifecycle through the SAME `Arc<Mutex<RenderState>>`
    /// `show()` and the paint callback share: `show()`'s side resolves the
    /// raw decision into a class via [`resolve_render_work_class`] (the
    /// same mapping the `widget.rs` call site uses) and starts a token;
    /// the paint callback's side re-locks later and finalizes that SAME
    /// token with all-zero uploads -- this test is exercising the token
    /// lifecycle itself, not upload-byte attribution, so a zero
    /// [`UploadByteCounts`] is simply the input, not a placeholder; see
    /// `render_state_level_wiring_of_a_real_upload_byte_counts_value`
    /// below for the Task 125.6 case that pins non-zero attribution
    /// flowing through the same API.
    #[test]
    fn token_lifecycle_through_the_render_state_shared_by_show_and_the_paint_callback() {
        let render_state = new_render_state(Arc::new(Mutex::new(WindowPostRenderer::new())));

        // `show()`'s side: the raw decision and resolved class are known
        // up front and supplied directly to `start`.
        let raw = RawRebuildDecision::Bounded {
            changed_row_count: 5,
        };
        let class = resolve_render_work_class(raw);
        let token = render_state.lock().unwrap().live_profile.start(raw, class);

        // The paint callback's side: a later, separate lock, finalizing
        // the token `show()` started above with only the token and the
        // upload counts -- the classification is not supplied again (it
        // cannot disagree with what `start` already recorded).
        let finalized = render_state
            .lock()
            .unwrap()
            .live_profile
            .complete(token, UploadByteCounts::default());
        assert!(finalized);

        let rs = render_state.lock().unwrap();
        assert_eq!(rs.live_profile.pending(), None);
        assert_eq!(rs.live_profile.painted_count(), 1);
        assert_eq!(rs.live_profile.raw_counts().bounded, 1);
        assert_eq!(rs.live_profile.class_counts().bounded, 1);
        assert_eq!(rs.live_profile.row_bucket_counts().five_to_eight, 1);
        assert_eq!(rs.live_profile.upload_totals().total(), 0);
        drop(rs);
    }

    /// A pane whose paint callback never runs this frame (the
    /// `FrameDamage::None` case `LiveRenderProfile`'s module doc
    /// describes) leaves its token pending until the SAME pane's next
    /// `show()` call starts a new one — exactly what a skipped paint
    /// callback looks like from `RenderState`'s side, without needing an
    /// actual skipped GL frame. The superseded token's classification
    /// (Task 125.5 regression fix) must survive into the finalized
    /// unpainted record and the cumulative raw/resolved totals.
    #[test]
    fn an_uninvoked_callback_leaves_its_token_finalized_as_unpainted_by_the_next_start() {
        let render_state = new_render_state(Arc::new(Mutex::new(WindowPostRenderer::new())));

        let raw = RawRebuildDecision::ReevaluateFullRebuild {
            resolved: ReevaluatedRebuild::Reuse,
        };
        let class = RenderWorkClass::Reuse;
        let first_token = render_state.lock().unwrap().live_profile.start(raw, class);
        // The pane's next `show()` call begins a new observation before
        // the first one's paint callback ever completed it.
        let _second_token = render_state
            .lock()
            .unwrap()
            .live_profile
            .start(RawRebuildDecision::CursorOnly, RenderWorkClass::CursorOnly);

        let rs = render_state.lock().unwrap();
        assert_eq!(rs.live_profile.unpainted_count(), 1);
        assert_eq!(rs.live_profile.raw_counts().reevaluate_full_rebuild, 1);
        assert_eq!(rs.live_profile.class_counts().reuse, 1);
        assert_eq!(rs.live_profile.upload_totals().total(), 0);
        let records: Vec<_> = rs.live_profile.records().copied().collect();
        drop(rs);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].token, first_token);
        assert_eq!(
            records[0].outcome,
            PaneFrameOutcome::Unpainted { raw, class }
        );
    }

    /// Regression test (Task 125.5 review): the flush signal a boundary
    /// crossing latches is observable through the SAME
    /// `Arc<Mutex<RenderState>>` API `show()` and the paint callback
    /// share, regardless of which side's finalization crosses it --
    /// mirrors `maybe_log_live_render_profile_flush`'s two call sites in
    /// `show()` without needing a real rendered frame.
    #[test]
    fn flush_signal_crossed_by_a_stale_token_sweep_is_observable_through_render_state() {
        let render_state = new_render_state(Arc::new(Mutex::new(WindowPostRenderer::new())));

        for _ in 0..(LiveRenderProfile::FLUSH_EVERY - 1) {
            let token = render_state
                .lock()
                .unwrap()
                .live_profile
                .start(RawRebuildDecision::CursorOnly, RenderWorkClass::CursorOnly);
            render_state
                .lock()
                .unwrap()
                .live_profile
                .complete(token, UploadByteCounts::default());
        }
        assert!(
            !render_state
                .lock()
                .unwrap()
                .live_profile
                .take_flush_signal()
        );

        // The 120th observation is finalized by a superseding `start()`
        // call (`show()`'s side), not by `complete()` (the paint
        // callback's side).
        let _stale_120th = render_state
            .lock()
            .unwrap()
            .live_profile
            .start(RawRebuildDecision::CursorOnly, RenderWorkClass::CursorOnly);
        let _next_token = render_state
            .lock()
            .unwrap()
            .live_profile
            .start(RawRebuildDecision::CursorOnly, RenderWorkClass::CursorOnly);

        assert!(
            render_state
                .lock()
                .unwrap()
                .live_profile
                .take_flush_signal(),
            "the boundary crossed by the stale-token sweep inside `start` \
             must be observable through the same RenderState the paint \
             callback would otherwise check"
        );
        assert!(
            !render_state
                .lock()
                .unwrap()
                .live_profile
                .take_flush_signal()
        );
    }

    /// Task 125.6: a real, non-zero, multi-category [`UploadByteCounts`]
    /// completed through the SAME `Arc<Mutex<RenderState>>` API `show()`
    /// and the pane's paint callback share must reach both
    /// `upload_totals()` (the per-buffer-category breakdown) and
    /// `class_upload_totals()` (the per-resolved-class breakdown) intact
    /// -- proving the `RenderState`-level plumbing this subtask's
    /// production call site (`FreminalTerminalWidget::show`'s paint
    /// callback) relies on, independent of any real GL draw.
    #[test]
    fn render_state_level_wiring_of_a_real_upload_byte_counts_value() {
        let render_state = new_render_state(Arc::new(Mutex::new(WindowPostRenderer::new())));

        let raw = RawRebuildDecision::ReevaluateFullRebuild {
            resolved: ReevaluatedRebuild::Full,
        };
        let class = resolve_render_work_class(raw);
        assert_eq!(class, RenderWorkClass::Full);
        let token = render_state.lock().unwrap().live_profile.start(raw, class);

        let uploads = UploadByteCounts {
            background_instance_vbo_bytes: 100,
            foreground_instance_vbo_bytes: 200,
            decoration_vbo_bytes: 24,
            image_vertex_vbo_bytes: 96,
            image_texture_bytes: 64,
            atlas_full_bytes: 4096,
            atlas_subrect_bytes: 0,
        };
        let finalized = render_state
            .lock()
            .unwrap()
            .live_profile
            .complete(token, uploads);
        assert!(finalized);

        let rs = render_state.lock().unwrap();
        assert_eq!(rs.live_profile.upload_totals(), uploads);
        assert_eq!(rs.live_profile.upload_totals().total(), uploads.total());
        assert_eq!(rs.live_profile.class_upload_totals().full, uploads.total());
        assert_eq!(rs.live_profile.class_upload_totals().reuse, 0);
        assert_eq!(rs.live_profile.class_upload_totals().bounded, 0);
        assert_eq!(rs.live_profile.class_upload_totals().cursor_only, 0);
        drop(rs);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod image_pixels_changed_tests {
    //! Regression coverage for Task 100.12: a Kitty `a=c` animation compose
    //! overwrites an existing frame's pixels in place (a new
    //! `Arc<Vec<u8>>`) without changing any cell or `run_mode`, so neither
    //! `content_changed` nor `image_frame_changed` fires and the full-rebuild
    //! path (the only path that refreshes `snap_images` and drives
    //! `sync_image_textures`) never runs. `image_pixels_changed` /
    //! `build_image_pixel_ptrs` are the pure, GUI-free predicate that catches
    //! this by comparing selected-frame pixel `Arc` pointers across frames.
    use super::*;
    use freminal_terminal_emulator::{
        AnimationControl, AnimationRunMode, ImageFrame, ImageSizeMode,
    };

    /// Build a 1x1-pixel `InlineImage`. `frame_pixels` (frames 2..N) share
    /// `id`'s allocation unless overridden by the caller after construction.
    fn still_image(id: u64) -> InlineImage {
        InlineImage {
            id,
            pixels: Arc::new(vec![0u8; 4]),
            width_px: 1,
            height_px: 1,
            display_cols: 1,
            display_rows: 1,
            size_mode: ImageSizeMode::NativePixels,
            frames: Vec::new(),
            root_gap_ms: 0,
            animation: AnimationControl {
                run_mode: AnimationRunMode::Running,
                loop_count: 1,
                current_frame: 0,
            },
        }
    }

    /// Build a 2-frame animated `InlineImage` (root frame 1 + one extra
    /// frame 2), each frame getting its own fresh `Arc` allocation.
    fn animated_image(id: u64) -> InlineImage {
        InlineImage {
            frames: vec![ImageFrame {
                pixels: Arc::new(vec![0u8; 4]),
                gap_ms: 40,
            }],
            ..still_image(id)
        }
    }

    #[test]
    fn unchanged_still_image_is_not_flagged() {
        let mut images = std::collections::HashMap::new();
        images.insert(1, still_image(1));

        let prev = build_image_pixel_ptrs(&images, |_| 1);

        assert!(
            !image_pixels_changed(&images, |_| 1, &prev),
            "identical pixel pointers across frames must not trigger a rebuild"
        );
    }

    #[test]
    fn still_image_pixel_replacement_is_flagged() {
        let mut images = std::collections::HashMap::new();
        images.insert(1, still_image(1));
        let prev = build_image_pixel_ptrs(&images, |_| 1);

        // Simulate a store-level mutation that replaces the root pixel
        // buffer with a new allocation (same id, new `Arc`).
        images.insert(1, still_image(1));

        assert!(
            image_pixels_changed(&images, |_| 1, &prev),
            "a new pixel Arc for the same image id must trigger a rebuild"
        );
    }

    #[test]
    fn animation_compose_on_non_root_frame_is_flagged() {
        // Frame 2 is the currently-selected/displayed frame (mirrors an
        // animation whose playback clock has advanced to frame 2).
        let mut images = std::collections::HashMap::new();
        images.insert(1, animated_image(1));
        let prev = build_image_pixel_ptrs(&images, |_| 2);

        // `a=c` compose: overwrite frame 2's pixels with a new `Arc`,
        // in place, without touching any cell or `run_mode`.
        {
            let img = images.get_mut(&1).unwrap();
            img.frames[0].pixels = Arc::new(vec![255u8; 4]);
        }

        assert!(
            image_pixels_changed(&images, |_| 2, &prev),
            "compose replacing the selected frame's pixels must trigger a rebuild"
        );
    }

    #[test]
    fn animation_compose_on_unselected_frame_is_not_flagged() {
        // Frame 1 (root) is currently selected/displayed; the compose below
        // targets frame 2, which is not currently visible.
        let mut images = std::collections::HashMap::new();
        images.insert(1, animated_image(1));
        let prev = build_image_pixel_ptrs(&images, |_| 1);

        {
            let img = images.get_mut(&1).unwrap();
            img.frames[0].pixels = Arc::new(vec![255u8; 4]);
        }

        assert!(
            !image_pixels_changed(&images, |_| 1, &prev),
            "a mutation to a frame that isn't currently selected must not force a rebuild"
        );
    }

    #[test]
    fn new_and_removed_image_ids_are_flagged() {
        let mut images = std::collections::HashMap::new();
        images.insert(1, still_image(1));
        let prev = build_image_pixel_ptrs(&images, |_| 1);

        // A new image id appears.
        images.insert(2, still_image(2));
        assert!(
            image_pixels_changed(&images, |_| 1, &prev),
            "an added image id must trigger a rebuild"
        );

        // The original id disappears, leaving only the new one.
        images.remove(&1);
        assert!(
            image_pixels_changed(&images, |_| 1, &prev),
            "a removed image id must trigger a rebuild"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod build_bounded_damage_tests {
    //! Task 124.14b-i introduced [`build_bounded_damage`]'s union of the
    //! changed-row list (124.14a) with the selection's current/previous
    //! screen-row span; Task 124.14b-ii adds the command-block hover's
    //! current/previous screen-row span as a fourth and fifth source. These
    //! tests exercise that union directly, at the pure-function level,
    //! rather than through the full `show()` pipeline (which needs a GL
    //! context). A no-fold, no-scroll-skip `FoldLayout`/`RowMap` pair is
    //! used throughout so snapshot row == rendered row == screen row,
    //! isolating the union/merge logic from the row-translation logic
    //! 124.14a's own tests already cover.
    use super::*;
    use crate::gui::renderer::{PaneDamageRect, PaneFrameDamage};
    use crate::gui::terminal::frame_dirty::ChangedRows;

    /// A `RowMap`/`FoldLayout` pair with no folds and no scroll-skip, so
    /// `row_map.snapshot_to_rendered` and `layout.rendered_to_screen`
    /// together act as the identity function over `[0, row_count)`.
    fn identity_layout(row_count: usize) -> FoldLayout {
        FoldLayout {
            flat_window_start: 0,
            row_map: RowMap::new(row_count, &[]),
            render_skip: 0,
        }
    }

    /// [`BoundedDamageSpans`] with every source empty/`None`, so each test
    /// below only needs to name the one or two sources it actually
    /// exercises rather than repeating every empty source at every call
    /// site.
    const fn no_spans() -> BoundedDamageSpans<'static> {
        BoundedDamageSpans {
            current_selection: None,
            previous_selection: None,
            current_hover: None,
            previous_hover: None,
            current_search_rows: &[],
            previous_search_rows: &[],
            current_cursor_row: None,
            previous_cursor_row: None,
        }
    }

    /// Fixed geometry shared by every test: 10px-tall rows, a 100px-wide
    /// viewport, and a 200px-tall framebuffer (20 rows) with the viewport
    /// pinned to the framebuffer's top-left origin. Rows used by the tests
    /// below are chosen away from row 0 and the last row so the 1px safety
    /// pad in `PaneDamageRect::from_cursor_cells` never gets clamped away --
    /// that clamping is `PaneDamageRect`'s own concern (covered by its own
    /// tests / the pixel harness), not this union logic's.
    fn geometry() -> RowDamageGeometry {
        RowDamageGeometry {
            row_h_f: 10.0,
            viewport_width_px: 100.0,
            vp_left_px: 0.0,
            vp_top_px: 0.0,
            fb_height_px: 200,
        }
    }

    /// The exact `PaneDamageRect` `row_run_damage` produces for on-screen rows
    /// `[start, end]` inclusive under [`geometry`] -- worked out once here
    /// and reused by every assertion below, rather than re-deriving the
    /// same arithmetic at each call site.
    fn expected_run(start: usize, end: usize) -> PaneDamageRect {
        let top: i32 = (start * 10).approx_as().unwrap_or(0);
        let bottom: i32 = ((end + 1) * 10).approx_as().unwrap_or(0);
        PaneDamageRect {
            x: 0,
            y: 200 - (bottom + 1),
            width: 101,
            height: (bottom + 1) - (top - 1),
        }
    }

    /// A selection-only change (no row change) must still produce bounded
    /// damage, and that damage must cover BOTH the current selection's rows
    /// (the new highlight) and the previous selection's rows (erasing the
    /// old one) -- the old-rows half is what a shrinking or moving
    /// selection needs to avoid leaving a stale highlight on screen. Uses
    /// two non-adjacent single-row spans so the result is provably two
    /// separate rects, not a bounding box spanning the rows between them
    /// (124.14a's design decision, still load-bearing here).
    #[test]
    fn selection_only_change_damages_both_current_and_previous_rows() {
        let layout = identity_layout(20);

        let damage = build_bounded_damage(
            &ChangedRows::None,
            BoundedDamageSpans {
                current_selection: Some((2, 2)),
                previous_selection: Some((5, 5)),
                ..no_spans()
            },
            &layout.row_map,
            &layout,
            geometry(),
            EmptyBoundedDamage::Full,
        );

        assert_eq!(
            damage,
            PaneFrameDamage::Region(vec![expected_run(2, 2), expected_run(5, 5)]),
            "damage must name both the new selection's row (2) and the old \
             selection's row (5) as two separate, non-contiguous rects"
        );
    }

    /// Task 125.17: a selection that stays attached to its text while the
    /// window slides beneath it (eviction at scrollback capacity) is painted
    /// on different screen rows each frame. The bounded damage must cover
    /// both extents -- the rows the highlight left AND the rows it moved to --
    /// even though no row changed and the selection itself did not. Here the
    /// highlight moves from rows 3..=4 to rows 2..=3; the union is one merged
    /// run over rows 2..=4, so the old bottom row (4) is repainted too.
    #[test]
    fn a_selection_slid_by_eviction_damages_the_old_and_the_new_rows() {
        let layout = identity_layout(20);

        let damage = build_bounded_damage(
            &ChangedRows::None,
            BoundedDamageSpans {
                current_selection: Some((2, 3)),
                previous_selection: Some((3, 4)),
                ..no_spans()
            },
            &layout.row_map,
            &layout,
            geometry(),
            EmptyBoundedDamage::Full,
        );

        assert_eq!(
            damage,
            PaneFrameDamage::Region(vec![expected_run(2, 4)]),
            "the vacated row 4 and the newly covered row 2 must both be damaged"
        );
    }

    /// The clearing case: the current selection is `None` (nothing to
    /// draw this frame) but a previous selection was recorded. The union
    /// must still damage the previous selection's rows -- if it did not,
    /// the just-cleared highlight's pixels would never be repainted and
    /// would linger on screen forever (nothing else would ever touch
    /// those rows again).
    #[test]
    fn cleared_selection_still_damages_the_previous_rows() {
        let layout = identity_layout(20);

        let damage = build_bounded_damage(
            &ChangedRows::None,
            BoundedDamageSpans {
                previous_selection: Some((1, 3)),
                ..no_spans()
            },
            &layout.row_map,
            &layout,
            geometry(),
            EmptyBoundedDamage::Full,
        );

        assert_eq!(
            damage,
            PaneFrameDamage::Region(vec![expected_run(1, 3)]),
            "clearing the selection (current None, previous Some) must \
             still damage the rows the now-erased highlight occupied"
        );
    }

    /// Overlapping sources -- a changed row that falls inside the current
    /// selection's span -- must merge into ONE rect, not two overlapping
    /// ones. This is the reason the three sources are combined into a
    /// single sorted, deduplicated row set before any run-merging runs:
    /// merging per-source first (row list -> one set of rects, selection ->
    /// another) would double-present the overlap, which is wasted work at
    /// best and, if a future change made a rect's bounds sensitive to which
    /// source produced it, a duplicate-rect correctness hazard at worst.
    #[test]
    fn a_changed_row_inside_the_selection_merges_into_one_rect() {
        let layout = identity_layout(20);

        let damage = build_bounded_damage(
            &ChangedRows::Rows(vec![3]),
            BoundedDamageSpans {
                current_selection: Some((2, 4)),
                ..no_spans()
            },
            &layout.row_map,
            &layout,
            geometry(),
            EmptyBoundedDamage::Full,
        );

        assert_eq!(
            damage,
            PaneFrameDamage::Region(vec![expected_run(2, 4)]),
            "row 3 sits inside the selection's [2, 4] span, so the union \
             must merge to exactly one rect spanning [2, 4] -- two \
             separate (and here, overlapping) rects would be a bug"
        );
    }

    // ── Task 124.14b-ii: command-block hover spans ─────────────────────────

    /// A hover-only change (no row change, no selection change) must still
    /// produce bounded damage, and that damage must cover BOTH the current
    /// hover's rows (drawing the new tint) and the previous hover's rows
    /// (erasing the old one) -- the old-rows half is what moving the hover
    /// from one command block to another needs to avoid leaving a stale
    /// tint on the block the pointer just left. Uses two non-adjacent
    /// single-row spans so the result is provably two separate rects, not
    /// a bounding box spanning the rows between them (124.14a's design
    /// decision, still load-bearing here).
    #[test]
    fn hover_only_change_damages_both_current_and_previous_rows() {
        let layout = identity_layout(20);

        let damage = build_bounded_damage(
            &ChangedRows::None,
            BoundedDamageSpans {
                current_hover: Some((2, 2)),
                previous_hover: Some((5, 5)),
                ..no_spans()
            },
            &layout.row_map,
            &layout,
            geometry(),
            EmptyBoundedDamage::Full,
        );

        assert_eq!(
            damage,
            PaneFrameDamage::Region(vec![expected_run(2, 2), expected_run(5, 5)]),
            "damage must name both the new hover's row (2) and the old \
             hover's row (5) as two separate, non-contiguous rects"
        );
    }

    /// The clearing case: the current hover is `None` (the pointer left the
    /// gutter, or left the block entirely) but a previous hover was
    /// recorded. The union must still damage the previous hover's rows --
    /// if it did not, the just-cleared tint's pixels would never be
    /// repainted and would linger on screen forever (nothing else would
    /// ever touch those rows again).
    #[test]
    fn cleared_hover_still_damages_the_previous_rows() {
        let layout = identity_layout(20);

        let damage = build_bounded_damage(
            &ChangedRows::None,
            BoundedDamageSpans {
                previous_hover: Some((1, 3)),
                ..no_spans()
            },
            &layout.row_map,
            &layout,
            geometry(),
            EmptyBoundedDamage::Full,
        );

        assert_eq!(
            damage,
            PaneFrameDamage::Region(vec![expected_run(1, 3)]),
            "clearing the hover (current None, previous Some) must still \
             damage the rows the now-erased tint occupied"
        );
    }

    /// Overlapping sources -- a changed row that falls inside the current
    /// hover's span -- must merge into ONE rect, not two overlapping ones.
    /// Same reasoning as the selection/changed-row overlap above: merging
    /// per-source first would double-present the overlap.
    #[test]
    fn a_changed_row_inside_the_hover_merges_into_one_rect() {
        let layout = identity_layout(20);

        let damage = build_bounded_damage(
            &ChangedRows::Rows(vec![3]),
            BoundedDamageSpans {
                current_hover: Some((2, 4)),
                ..no_spans()
            },
            &layout.row_map,
            &layout,
            geometry(),
            EmptyBoundedDamage::Full,
        );

        assert_eq!(
            damage,
            PaneFrameDamage::Region(vec![expected_run(2, 4)]),
            "row 3 sits inside the hover's [2, 4] span, so the union must \
             merge to exactly one rect spanning [2, 4] -- two separate \
             (and here, overlapping) rects would be a bug"
        );
    }

    /// Overlapping sources across two *different* sources -- a hover span
    /// that overlaps the current selection's span -- must also merge into
    /// ONE rect. This is the case 124.14b-ii adds on top of 124.14b-i:
    /// selection and hover are independent sources that can legitimately
    /// highlight the same rows at once (selecting text inside the block
    /// the pointer is hovering), and the union must not double-present
    /// that overlap either.
    #[test]
    fn a_hover_span_overlapping_the_selection_merges_into_one_rect() {
        let layout = identity_layout(20);

        let damage = build_bounded_damage(
            &ChangedRows::None,
            BoundedDamageSpans {
                current_selection: Some((2, 4)),
                current_hover: Some((3, 6)),
                ..no_spans()
            },
            &layout.row_map,
            &layout,
            geometry(),
            EmptyBoundedDamage::Full,
        );

        assert_eq!(
            damage,
            PaneFrameDamage::Region(vec![expected_run(2, 6)]),
            "the selection's [2, 4] and the hover's [3, 6] overlap on rows \
             3-4, so the union must merge to exactly one rect spanning \
             [2, 6] -- two separate (and here, overlapping) rects would be \
             a bug"
        );
    }

    // ── Task 124.14d: search-highlight rows ─────────────────────────────

    /// A search-only change (no row change, no selection, no hover) must
    /// still produce bounded damage, covering BOTH the current highlight
    /// rows (drawing the new tint) and the previous ones (erasing the old
    /// one) -- mirrors `selection_only_change_damages_both_current_and_previous_rows`
    /// and `hover_only_change_damages_both_current_and_previous_rows`.
    #[test]
    fn search_only_change_damages_both_current_and_previous_rows() {
        let layout = identity_layout(20);

        let damage = build_bounded_damage(
            &ChangedRows::None,
            BoundedDamageSpans {
                current_search_rows: &[2],
                previous_search_rows: &[5],
                ..no_spans()
            },
            &layout.row_map,
            &layout,
            geometry(),
            EmptyBoundedDamage::Full,
        );

        assert_eq!(
            damage,
            PaneFrameDamage::Region(vec![expected_run(2, 2), expected_run(5, 5)]),
            "damage must name both the new search highlight's row (2) and \
             the old highlight's row (5) as two separate, non-contiguous \
             rects"
        );
    }

    /// Overlapping sources -- a search row that falls inside a changed
    /// row's run -- must merge into ONE rect, same reasoning as the
    /// selection/hover overlap tests above.
    #[test]
    fn a_search_row_inside_a_changed_run_merges_into_one_rect() {
        let layout = identity_layout(20);

        let damage = build_bounded_damage(
            &ChangedRows::Rows(vec![2, 3, 4]),
            BoundedDamageSpans {
                current_search_rows: &[3],
                ..no_spans()
            },
            &layout.row_map,
            &layout,
            geometry(),
            EmptyBoundedDamage::Full,
        );

        assert_eq!(
            damage,
            PaneFrameDamage::Region(vec![expected_run(2, 4)]),
            "search row 3 sits inside the changed run [2, 4], so the union \
             must merge to exactly one rect"
        );
    }

    /// The load-bearing case for Task 124.14d's `EmptyBoundedDamage`
    /// parameter: search is the sole bounded source, and its own
    /// current/previous highlight-row union is empty (no visible matches
    /// before or after -- e.g. a query edit that still matches nothing).
    /// The terminal-band contribution must be `Unchanged`, not `Full`: the
    /// popup's own damage is unioned in separately at the per-pane-frame
    /// level, so escalating the terminal band to `Full` here would be
    /// pure waste, not correctness.
    #[test]
    fn search_only_change_with_no_visible_matches_reports_unchanged() {
        let layout = identity_layout(20);

        let damage = build_bounded_damage(
            &ChangedRows::None,
            no_spans(),
            &layout.row_map,
            &layout,
            geometry(),
            EmptyBoundedDamage::Unchanged,
        );

        assert_eq!(
            damage,
            PaneFrameDamage::Unchanged,
            "a search-only frame with no visible matches before or after \
             must report Unchanged, not Full"
        );
    }

    /// The control for the test above: with nothing at all bound (the
    /// shape selection/hover/row sources hit when their own extent
    /// collapses behind a fold), `EmptyBoundedDamage::Full` must still
    /// yield `Full` -- proving the enum threads through correctly in both
    /// directions, not just the new one.
    #[test]
    fn nothing_bound_with_full_empty_fallback_reports_full() {
        let layout = identity_layout(20);

        let damage = build_bounded_damage(
            &ChangedRows::None,
            no_spans(),
            &layout.row_map,
            &layout,
            geometry(),
            EmptyBoundedDamage::Full,
        );

        assert_eq!(damage, PaneFrameDamage::Full);
    }

    // ── Cursor screen-row extents ────────────────────────────────────────

    /// A frame with a changed content row (8) AND a cursor that moved from
    /// screen row 15 (last frame) to screen row 2 (this frame) must damage
    /// all three rows -- the changed content row, the cursor's new row,
    /// and the cursor's old row -- as three separate, non-contiguous rects,
    /// sorted and deduplicated. Mirrors the selection/hover/search
    /// "current and previous" pattern: erasing the OLD cursor row matters
    /// exactly as much as damaging the NEW one, since a bounded rebuild's
    /// presented damage must not leave stale cursor pixels behind on the
    /// row the cursor just vacated.
    #[test]
    fn changed_row_and_cursor_move_damages_all_three_rows() {
        let layout = identity_layout(20);

        let damage = build_bounded_damage(
            &ChangedRows::Rows(vec![8]),
            BoundedDamageSpans {
                current_cursor_row: Some(2),
                previous_cursor_row: Some(15),
                ..no_spans()
            },
            &layout.row_map,
            &layout,
            geometry(),
            EmptyBoundedDamage::Full,
        );

        assert_eq!(
            damage,
            PaneFrameDamage::Region(vec![
                expected_run(2, 2),
                expected_run(8, 8),
                expected_run(15, 15)
            ]),
            "damage must name the cursor's new row (2), the changed \
             content row (8), and the cursor's old row (15) as three \
             separate, sorted, deduplicated rects"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod full_pane_rebuild_damage_rect_tests {
    //! Task 124.21 finding 2's fix: [`full_pane_rebuild_damage_rect`] builds
    //! the damage rect a full pane rebuild reports instead of escalating to
    //! `PaneFrameDamage::Full`. These tests exercise it directly at the
    //! pure-function level rather than through the full `show()` pipeline
    //! (which needs a GL context).
    use super::*;

    /// Physical scale factor shared by every test below.
    const PPP: f32 = 2.0;
    /// A framebuffer tall enough that a 50-logical-point-tall pane (100
    /// physical pixels) plus `from_cursor_cells`' 1px outward safety pad
    /// never clips against the bottom edge, so the tests exercising
    /// ordinary (non-clamped) geometry see un-clamped values.
    const TALL_FB_HEIGHT_PX: i32 = 300;
    /// A deliberately short framebuffer, used only by the framebuffer-clamp
    /// test below to put a pane rect entirely below the bottom edge.
    const SHORT_FB_HEIGHT_PX: i32 = 100;

    /// The pane-rect damage must be built from the WHOLE pane rect, not
    /// `terminal_rect` -- i.e. it must extend left of the terminal area
    /// when a gutter inset is present. This is the whole reason
    /// `full_pane_rebuild_damage_rect` takes `pane_rect` as a distinct
    /// parameter from `terminal_rect` rather than reusing `terminal_rect`
    /// alone (a full pane rebuild also repaints the command-block gutter
    /// strip and the scrollbar, both of which live inside `pane_rect` but
    /// outside `terminal_rect`). If this regressed to using `terminal_rect`'s
    /// bounds instead, the emitted rect's left edge would sit at the
    /// gutter's right edge instead of the pane's own left edge, and this
    /// assertion would fail.
    #[test]
    fn pane_rect_damage_extends_left_of_terminal_rect_when_a_gutter_inset_is_present() {
        // A 20-logical-point gutter inset: `terminal_rect` starts 20 points
        // right of `pane_rect`, matching `terminal_rect_origin`'s
        // `pane_rect.min.x + gutter_inset` convention.
        let pane_rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 50.0));
        let terminal_rect =
            egui::Rect::from_min_max(egui::pos2(20.0, 0.0), egui::pos2(100.0, 50.0));
        // `vp_left_px`/`vp_top_px` are `terminal_rect.min * ppp`, exactly as
        // computed at the `show` call site.
        let vp_left_px = terminal_rect.min.x * PPP;
        let vp_top_px = terminal_rect.min.y * PPP;

        let damage = full_pane_rebuild_damage_rect(
            pane_rect,
            terminal_rect,
            PPP,
            vp_left_px,
            vp_top_px,
            TALL_FB_HEIGHT_PX,
        )
        .expect("a well-formed pane rect must yield damage");

        // The pane's left edge is at physical x=0 (pane_rect.min.x=0 * ppp),
        // strictly left of the terminal viewport's own left edge at
        // physical x=40 (20.0 * 2.0) -- i.e. the gutter strip's width is
        // included.
        assert_eq!(
            damage.x, 0,
            "the damage rect must start at the PANE's left edge (physical \
             x=0), not the terminal viewport's left edge (physical x=40) -- \
             otherwise the gutter strip would never be repainted by a full \
             pane rebuild"
        );
        // Full pane width/height in physical pixels (100 * 2.0 = 200, 50 *
        // 2.0 = 100), plus the 1px safety pad `from_cursor_cells` always
        // applies outward on the left/top edge only (there is no separate
        // clamp on the right/bottom edges beyond the framebuffer's own
        // bounds, which `TALL_FB_HEIGHT_PX` is chosen not to hit here).
        assert_eq!(damage.width, 201);
        assert_eq!(damage.height, 101);
    }

    /// The ordinary case with no gutter (`pane_rect == terminal_rect`):
    /// behaves exactly like damaging the terminal viewport's own full
    /// bounds, with no leftward extension.
    #[test]
    fn pane_rect_damage_matches_terminal_rect_when_no_gutter_is_present() {
        let rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 50.0));
        let vp_left_px = rect.min.x * PPP;
        let vp_top_px = rect.min.y * PPP;

        let damage = full_pane_rebuild_damage_rect(
            rect,
            rect,
            PPP,
            vp_left_px,
            vp_top_px,
            TALL_FB_HEIGHT_PX,
        )
        .expect("a well-formed pane rect must yield damage");

        assert_eq!(damage.x, 0);
        assert_eq!(damage.width, 201);
        assert_eq!(damage.height, 101);
    }

    /// A degenerate pane rect (zero width) falls back to `None` -- which
    /// the `show` call site turns into `PaneFrameDamage::Full` -- rather
    /// than emitting a zero-area `Region`. A pane whose own bounds cannot
    /// be established genuinely cannot bound its damage.
    #[test]
    fn degenerate_zero_width_pane_rect_yields_none() {
        let pane_rect = egui::Rect::from_min_max(egui::pos2(10.0, 0.0), egui::pos2(10.0, 50.0));
        let vp_left_px = pane_rect.min.x * PPP;
        let vp_top_px = pane_rect.min.y * PPP;

        let damage = full_pane_rebuild_damage_rect(
            pane_rect,
            pane_rect,
            PPP,
            vp_left_px,
            vp_top_px,
            TALL_FB_HEIGHT_PX,
        );

        assert_eq!(
            damage, None,
            "a zero-width pane rect must yield None (-> Full at the call \
             site), never a zero-area Region"
        );
    }

    /// A degenerate pane rect (negative height, e.g. an inverted rect from
    /// an upstream bug) also falls back to `None`.
    #[test]
    fn degenerate_negative_extent_pane_rect_yields_none() {
        // `max.y < min.y` makes `height()` negative.
        let pane_rect = egui::Rect::from_min_max(egui::pos2(0.0, 50.0), egui::pos2(100.0, 0.0));
        let vp_left_px = pane_rect.min.x * PPP;
        let vp_top_px = 0.0;

        let damage = full_pane_rebuild_damage_rect(
            pane_rect,
            pane_rect,
            PPP,
            vp_left_px,
            vp_top_px,
            TALL_FB_HEIGHT_PX,
        );

        assert_eq!(
            damage, None,
            "a negative-extent pane rect must yield None (-> Full at the \
             call site)"
        );
    }

    /// A pane rect that lies entirely off the bottom of the framebuffer
    /// clamps away to nothing and also yields `None` -- the framebuffer
    /// clamp is the other degenerate case named in the design
    /// (`from_cursor_cells`' own bounds check), distinct from a
    /// zero/negative logical extent.
    #[test]
    fn pane_rect_clamped_entirely_off_the_framebuffer_yields_none() {
        let pane_rect = egui::Rect::from_min_max(egui::pos2(0.0, 200.0), egui::pos2(100.0, 250.0));
        let vp_left_px = pane_rect.min.x * PPP;
        let vp_top_px = pane_rect.min.y * PPP;

        let damage = full_pane_rebuild_damage_rect(
            pane_rect,
            pane_rect,
            PPP,
            vp_left_px,
            vp_top_px,
            SHORT_FB_HEIGHT_PX,
        );

        assert_eq!(
            damage, None,
            "a pane rect entirely below the framebuffer must clamp away to \
             nothing (-> Full at the call site), not emit a degenerate rect"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod progress_bar_geometry_tests {
    //! Issue #507 phase B2: [`progress_bar_track_rect`], [`progress_fill_rect`],
    //! and [`merge_progress_damage`] exercised directly at the pure-function
    //! level, mirroring `full_pane_rebuild_damage_rect_tests` above.
    use super::*;
    use crate::gui::renderer::{PaneDamageRect, PaneFrameDamage};

    // ── progress_bar_track_rect ──────────────────────────────────────────

    /// The track spans the pane's FULL width (including any gutter strip
    /// to its left), anchored to the pane's top edge, with the given
    /// height -- not the terminal cell grid's width, and not centered.
    #[test]
    fn track_rect_spans_full_pane_width_at_the_top_edge() {
        let pane_rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(200.0, 100.0));

        let track = progress_bar_track_rect(pane_rect, 3.0);

        assert_eq!(track.min, egui::pos2(0.0, 0.0));
        assert_eq!(track.max, egui::pos2(200.0, 3.0));
    }

    /// A gutter-shifted pane (nonzero `pane_rect.min.x`) still has the
    /// track start at the pane's own left edge, not the terminal viewport's
    /// -- mirroring `full_pane_rebuild_damage_rect`'s use of `pane_rect`
    /// rather than `terminal_rect` for the same reason (the bar spans the
    /// gutter strip too).
    #[test]
    fn track_rect_starts_at_the_panes_own_left_edge() {
        let pane_rect = egui::Rect::from_min_max(egui::pos2(50.0, 10.0), egui::pos2(250.0, 110.0));

        let track = progress_bar_track_rect(pane_rect, 4.0);

        assert_eq!(track.min, egui::pos2(50.0, 10.0));
        assert_eq!(track.max, egui::pos2(250.0, 14.0));
    }

    // ── progress_fill_rect ───────────────────────────────────────────────

    /// 0% fills nothing: the fill rect collapses to a zero-width sliver at
    /// the track's left edge.
    #[test]
    fn fill_rect_at_zero_percent_has_zero_width() {
        let track = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(200.0, 3.0));

        let fill = progress_fill_rect(track, 0);

        assert_eq!(fill.min, track.min);
        assert!(fill.width().abs() < f32::EPSILON);
        assert!((fill.height() - track.height()).abs() < f32::EPSILON);
    }

    /// 100% fills the entire track width.
    #[test]
    fn fill_rect_at_100_percent_matches_the_full_track_width() {
        let track = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(200.0, 3.0));

        let fill = progress_fill_rect(track, 100);

        assert_eq!(fill.min, track.min);
        assert!((fill.max.x - track.max.x).abs() < f32::EPSILON);
        assert!((fill.height() - track.height()).abs() < f32::EPSILON);
    }

    /// An intermediate percentage fills exactly that fraction of the
    /// track's width, left-anchored -- the core percent -> pixel-width
    /// conversion this function exists to isolate for direct testing.
    #[test]
    fn fill_rect_at_50_percent_is_half_the_track_width() {
        let track = egui::Rect::from_min_max(egui::pos2(10.0, 0.0), egui::pos2(210.0, 3.0));

        let fill = progress_fill_rect(track, 50);

        assert!((fill.min.x - 10.0).abs() < f32::EPSILON);
        assert!((fill.width() - 100.0).abs() < f32::EPSILON);
    }

    /// A `percent` above 100 (should never occur -- `ProgressReport` already
    /// clamps at parse time -- but this is a pure function with no way to
    /// enforce that at the type level) is clamped rather than overflowing
    /// the track's own bounds.
    #[test]
    fn fill_rect_clamps_percent_above_100() {
        let track = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(200.0, 3.0));

        let fill = progress_fill_rect(track, 255);

        assert!((fill.max.x - track.max.x).abs() < f32::EPSILON);
    }

    // ── merge_progress_damage ────────────────────────────────────────────

    fn dr(x: i32) -> PaneDamageRect {
        PaneDamageRect {
            x,
            y: 0,
            width: 10,
            height: 3,
        }
    }

    /// Merging into `Unchanged` (the common case: nothing else changed
    /// this frame besides the progress bar) produces a `Region` containing
    /// exactly the bar's rect.
    #[test]
    fn merge_into_unchanged_yields_region_with_only_the_bar_rect() {
        let result = merge_progress_damage(PaneFrameDamage::Unchanged, Some(dr(1)));
        assert_eq!(result, PaneFrameDamage::Region(vec![dr(1)]));
    }

    /// Merging into an existing `CursorOnly(Some(rect))` (e.g. the cursor
    /// also moved this frame) upgrades to `Region` carrying BOTH rects, in
    /// order -- neither is dropped.
    #[test]
    fn merge_into_cursor_only_preserves_both_rects() {
        let result = merge_progress_damage(PaneFrameDamage::CursorOnly(Some(dr(5))), Some(dr(1)));
        assert_eq!(result, PaneFrameDamage::Region(vec![dr(5), dr(1)]));
    }

    /// Merging into an existing `Region(rects)` (e.g. a bounded content
    /// rebuild ran this same frame) appends the bar's rect rather than
    /// replacing the existing ones.
    #[test]
    fn merge_into_region_appends_the_bar_rect() {
        let result =
            merge_progress_damage(PaneFrameDamage::Region(vec![dr(3), dr(7)]), Some(dr(1)));
        assert_eq!(result, PaneFrameDamage::Region(vec![dr(3), dr(7), dr(1)]));
    }

    /// `Full` is left untouched -- it already covers the bar's rect, and
    /// downgrading it would be a correctness regression, not an
    /// optimization.
    #[test]
    fn merge_into_full_stays_full() {
        let result = merge_progress_damage(PaneFrameDamage::Full, Some(dr(1)));
        assert_eq!(result, PaneFrameDamage::Full);
    }

    /// A degenerate `CursorOnly(None)` (already Full-equivalent once
    /// aggregated) is left as-is rather than being "fixed up" into
    /// something boundable -- it already forces `Full` at aggregation
    /// regardless of what this function does with it.
    #[test]
    fn merge_into_cursor_only_none_is_left_unchanged() {
        let result = merge_progress_damage(PaneFrameDamage::CursorOnly(None), Some(dr(1)));
        assert_eq!(result, PaneFrameDamage::CursorOnly(None));
    }

    /// A `None` bar rect (the geometry conversion itself degenerated) means
    /// the bar's extent cannot be bounded, so the whole pane escalates to
    /// `Full` regardless of what damage was already decided -- the
    /// complete-bound rule (`freminal-damage-model` section 6): an
    /// unprovable bound must over-report, never silently drop the change.
    #[test]
    fn merge_with_no_bar_rect_escalates_to_full() {
        assert_eq!(
            merge_progress_damage(PaneFrameDamage::Unchanged, None),
            PaneFrameDamage::Full
        );
        assert_eq!(
            merge_progress_damage(PaneFrameDamage::Region(vec![dr(3)]), None),
            PaneFrameDamage::Full
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod gutter_hover_trigger_tests {
    //! 73.5: the gutter strip is the sole hover trigger; hovering output
    //! cells does not tint a command block.
    use super::*;
    use freminal_common::buffer_states::command_block::{CommandBlock, CommandBlockId};
    use freminal_common::buffer_states::row_number::RowNumber;
    use freminal_common::config::{CommandBlocksConfig, GutterPosition};
    use freminal_terminal_emulator::snapshot::TerminalSnapshot;
    use std::time::SystemTime;

    /// A snapshot with a single finished block spanning screen rows 1..=3,
    /// `term_height` rows tall, scrolled to the live bottom (so
    /// `win_start == 0`).
    fn snapshot_with_block(term_height: usize) -> TerminalSnapshot {
        let mut snap = TerminalSnapshot::empty();
        snap.term_width = 80;
        snap.term_height = term_height;
        snap.total_rows = term_height; // win_start = total - height - 0 = 0
        snap.scroll_offset = 0;
        let block = CommandBlock {
            id: CommandBlockId::next(),
            fid: "t".to_owned(),
            prompt_start_row: RowNumber::new(1),
            command_start_row: Some(RowNumber::new(1)),
            output_start_row: Some(RowNumber::new(2)),
            end_row: Some(RowNumber::new(3)),
            exit_code: Some(0),
            cwd: None,
            started_at: SystemTime::UNIX_EPOCH,
            executed_at: Some(SystemTime::UNIX_EPOCH),
            finished_at: Some(SystemTime::UNIX_EPOCH),
        };
        snap.command_blocks = std::sync::Arc::from(vec![block]);
        snap
    }

    /// Geometry: 10px logical cells, gutter inset 8px, pane top-left at (0,0).
    /// Terminal rect therefore starts at x=8.  Row 2 spans y in [20,30).
    fn geometry() -> (Rect, Rect, f32, f32) {
        let cell_h = 10.0_f32;
        let inset = 8.0_f32;
        let pane_rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(808.0, 500.0));
        let terminal_rect = Rect::from_min_max(egui::pos2(inset, 0.0), egui::pos2(808.0, 500.0));
        (pane_rect, terminal_rect, inset, cell_h)
    }

    #[test]
    fn hovering_gutter_row_tints_the_block() {
        let snap = snapshot_with_block(24);
        let (pane_rect, terminal_rect, inset, cell_h) = geometry();
        let cfg = CommandBlocksConfig::default();
        let mut vs = ViewState::new();
        // Pointer in the gutter (x=4, inside [0,8)), at row 2 (y=25 -> row 2).
        vs.mouse_position = Some(egui::pos2(4.0, 25.0));
        let layout = FoldLayout::new(&snap, &vs.folded_blocks);

        let rows = compute_command_block_hover_rows(
            &snap,
            &vs,
            &cfg,
            &layout,
            pane_rect,
            terminal_rect,
            inset,
            cell_h,
        );
        // Block spans command_start_row..=end_row = rows 1..=3.
        assert_eq!(rows, Some((1, 3)), "gutter hover must tint the block");
    }

    #[test]
    fn hovering_output_cell_does_not_tint() {
        let snap = snapshot_with_block(24);
        let (pane_rect, terminal_rect, inset, cell_h) = geometry();
        let cfg = CommandBlocksConfig::default();
        let mut vs = ViewState::new();
        // Pointer over a terminal cell well inside the block's rows (x=100,
        // which is >= terminal_rect.min.x=8), row 2.
        vs.mouse_position = Some(egui::pos2(100.0, 25.0));
        let layout = FoldLayout::new(&snap, &vs.folded_blocks);

        let rows = compute_command_block_hover_rows(
            &snap,
            &vs,
            &cfg,
            &layout,
            pane_rect,
            terminal_rect,
            inset,
            cell_h,
        );
        assert_eq!(rows, None, "hovering output cells must not tint a block");
    }

    #[test]
    fn gutter_off_disables_hover() {
        let snap = snapshot_with_block(24);
        let (pane_rect, terminal_rect, _inset, cell_h) = geometry();
        let cfg = CommandBlocksConfig {
            gutter: GutterPosition::Off,
            ..CommandBlocksConfig::default()
        };
        let mut vs = ViewState::new();
        vs.mouse_position = Some(egui::pos2(4.0, 25.0));
        let layout = FoldLayout::new(&snap, &vs.folded_blocks);

        // gutter_inset == 0 when the gutter is off.
        let rows = compute_command_block_hover_rows(
            &snap,
            &vs,
            &cfg,
            &layout,
            pane_rect,
            terminal_rect,
            0.0,
            cell_h,
        );
        assert_eq!(rows, None, "gutter = off disables the hover trigger");
    }

    #[test]
    fn no_pointer_no_tint() {
        let snap = snapshot_with_block(24);
        let (pane_rect, terminal_rect, inset, cell_h) = geometry();
        let cfg = CommandBlocksConfig::default();
        let vs = ViewState::new(); // mouse_position == None
        let layout = FoldLayout::new(&snap, &vs.folded_blocks);

        let rows = compute_command_block_hover_rows(
            &snap,
            &vs,
            &cfg,
            &layout,
            pane_rect,
            terminal_rect,
            inset,
            cell_h,
        );
        assert_eq!(rows, None);
    }
}

/// The reasons pane input can be suppressed on a given frame.
///
/// Several can hold simultaneously (a modal open *and* a scrollbar drag in
/// flight), and no combination is illegal, so this is the documented case
/// where a set of independent bool signals is the correct representation
/// rather than a single enum (`freminal-state-representation`).
///
/// Grouping them gives the suppression rule one place to live. It was
/// previously spelled out twice -- once to decide whether to suppress, once
/// to decide what may still get through -- which meant the two lists had to
/// be kept in sync by hand.
// Each field is a separate, independently-observed condition, and every
// combination of them is legal and meaningful -- which is exactly the case
// `freminal-state-representation` names as the correct use of bools rather
// than an enum. Collapsing them into a state machine would assert an ordering
// and mutual exclusion that does not exist (a modal can be open while a
// scrollbar drag is in flight). `SearchState` carries the same allow for the
// same reason.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy)]
pub(super) struct InputSuppressors {
    /// A modal/menu overlay or pane-border drag, including the deliberate
    /// one-frame release tail that stops a dismiss-click leaking through.
    pub(super) modal_or_drag: bool,
    /// The right-click context menu is open.
    pub(super) context_menu: bool,
    /// The find-in-scrollback overlay is open.
    pub(super) search_overlay: bool,
    /// The command-history palette is open.
    pub(super) command_history: bool,
    /// A scrollbar drag is in progress.
    pub(super) scrollbar_drag: bool,
}

impl InputSuppressors {
    /// Whether anything at all is suppressing pane input this frame.
    pub(super) const fn any(self) -> bool {
        self.modal_or_drag
            || self.context_menu
            || self.search_overlay
            || self.command_history
            || self.scrollbar_drag
    }

    /// Whether mouse-wheel events should still reach the pane despite
    /// suppression.
    ///
    /// True only when the search overlay is the *sole* reason input is
    /// suppressed, and only for the active pane. Search finds matches in this
    /// pane's scrollback, so swallowing the wheel leaves those matches
    /// unreachable -- the user cannot look at what they just found. Every
    /// other suppressor keeps the wheel blocked: a context menu or palette has
    /// its own scrollable content, a scrollbar drag is already driving the
    /// offset, and a modal's dismiss-click tail must not move the view.
    ///
    /// Note this governs *whether* wheel events are read at all; what they are
    /// then allowed to do is further restricted in
    /// [`super::input::scroll_overlay_passthrough`] (primary screen only, no
    /// PTY writes, no mouse-tracking reports).
    pub(super) const fn scroll_passes_through(self, pane_focus: PaneFocus) -> bool {
        self.search_overlay
            && matches!(pane_focus, PaneFocus::Active)
            && !self.modal_or_drag
            && !self.context_menu
            && !self.command_history
            && !self.scrollbar_drag
    }
}

#[cfg(test)]
mod url_hover_pane_bounds_tests {
    //! Issue #510: a URL on a pane's top row must not stay "hovered" once
    //! the pointer moves into a sibling pane stacked above it. Every pane
    //! runs against the same window-wide `PointerMoved` queue, so
    //! `view_state.mouse_position` alone cannot be trusted to mean "over
    //! this pane" -- containment must be checked explicitly.
    use super::*;

    /// 10px logical cells, pane/terminal rect both `(0,0)-(100,100)` (no
    /// gutter inset, unlike the command-block tests above).
    fn terminal_rect() -> Rect {
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 100.0))
    }

    #[test]
    fn pointer_inside_rect_resolves_to_a_cell() {
        let rect = terminal_rect();
        // (25, 35) -> column 2, row 3 at 10px cells.
        let pos = egui::pos2(25.0, 35.0);
        assert_eq!(
            cell_under_pointer_for_url_hover(Some(pos), rect, (10.0, 10.0)),
            Some((2, 3))
        );
    }

    #[test]
    fn pointer_above_the_pane_is_rejected_not_clamped_to_row_zero() {
        let rect = terminal_rect();
        // A position belonging to a sibling pane stacked above this one:
        // above `rect.min.y`. Before the #510 fix this clamped to row 0
        // instead of being rejected outright.
        let pos = egui::pos2(25.0, -40.0);
        assert_eq!(
            cell_under_pointer_for_url_hover(Some(pos), rect, (10.0, 10.0)),
            None
        );
    }

    #[test]
    fn pointer_left_of_the_pane_is_rejected_not_clamped_to_column_zero() {
        let rect = terminal_rect();
        let pos = egui::pos2(-40.0, 25.0);
        assert_eq!(
            cell_under_pointer_for_url_hover(Some(pos), rect, (10.0, 10.0)),
            None
        );
    }

    #[test]
    fn pointer_below_or_right_of_the_pane_is_rejected() {
        let rect = terminal_rect();
        assert_eq!(
            cell_under_pointer_for_url_hover(Some(egui::pos2(25.0, 500.0)), rect, (10.0, 10.0)),
            None
        );
        assert_eq!(
            cell_under_pointer_for_url_hover(Some(egui::pos2(500.0, 25.0)), rect, (10.0, 10.0)),
            None
        );
    }

    #[test]
    fn no_mouse_position_is_rejected() {
        let rect = terminal_rect();
        assert_eq!(
            cell_under_pointer_for_url_hover(None, rect, (10.0, 10.0)),
            None
        );
    }
}

#[cfg(test)]
mod pointer_target_tests {
    use super::{
        CursorIcon, PointerHover, PointerShape, PointerTarget, cursor_icon_for,
        pointer_shape_to_cursor_icon,
    };

    /// Nothing hovered.
    const NONE: PointerHover = PointerHover {
        command_block_gutter: false,
        fold_placeholder: false,
        url: false,
    };

    #[test]
    fn nothing_hovered_defers_to_the_application_shape() {
        assert_eq!(NONE.resolve(), PointerTarget::TerminalContent);
        assert_eq!(
            cursor_icon_for(NONE.resolve(), PointerShape::Crosshair),
            CursorIcon::Crosshair,
            "ordinary terminal content must honour the OSC 22 shape"
        );
    }

    /// The regression: hovering the gutter must actually produce a pointing
    /// hand, even though the application has set its own OSC 22 shape. This
    /// previously lost to the URL/OSC-22 write that ran later in `show`.
    #[test]
    fn gutter_hover_beats_the_application_shape() {
        let hover = PointerHover {
            command_block_gutter: true,
            ..NONE
        };
        assert_eq!(hover.resolve(), PointerTarget::CommandBlockGutter);
        assert_eq!(
            cursor_icon_for(hover.resolve(), PointerShape::Text),
            CursorIcon::PointingHand
        );
    }

    /// Precedence is total and deterministic, not last-writer-wins.
    #[test]
    fn precedence_is_gutter_then_placeholder_then_url() {
        let all = PointerHover {
            command_block_gutter: true,
            fold_placeholder: true,
            url: true,
        };
        assert_eq!(all.resolve(), PointerTarget::CommandBlockGutter);

        let no_gutter = PointerHover {
            command_block_gutter: false,
            ..all
        };
        assert_eq!(no_gutter.resolve(), PointerTarget::FoldPlaceholder);

        let url_only = PointerHover { url: true, ..NONE };
        assert_eq!(url_only.resolve(), PointerTarget::Url);
    }

    /// Every chrome affordance uses the same icon, so which one wins is not
    /// visually observable -- but the rule must still be defined.
    #[test]
    fn all_chrome_targets_use_the_pointing_hand() {
        for target in [
            PointerTarget::CommandBlockGutter,
            PointerTarget::FoldPlaceholder,
            PointerTarget::Url,
        ] {
            assert_eq!(
                cursor_icon_for(target, PointerShape::Wait),
                CursorIcon::PointingHand,
                "{target:?} must not defer to the application shape"
            );
        }
    }

    /// Terminal content maps straight through the OSC 22 table.
    #[test]
    fn terminal_content_matches_the_osc22_table() {
        for shape in [
            PointerShape::Default,
            PointerShape::Text,
            PointerShape::Wait,
            PointerShape::Grab,
            PointerShape::None,
        ] {
            assert_eq!(
                cursor_icon_for(PointerTarget::TerminalContent, shape),
                pointer_shape_to_cursor_icon(shape)
            );
        }
    }
}

#[cfg(test)]
mod input_suppressors_tests {
    use super::{InputSuppressors, PaneFocus};

    /// Nothing suppressing.
    const CLEAR: InputSuppressors = InputSuppressors {
        modal_or_drag: false,
        context_menu: false,
        search_overlay: false,
        command_history: false,
        scrollbar_drag: false,
    };

    #[test]
    fn any_is_false_only_when_every_suppressor_is_clear() {
        assert!(!CLEAR.any());
        assert!(
            InputSuppressors {
                modal_or_drag: true,
                ..CLEAR
            }
            .any()
        );
        assert!(
            InputSuppressors {
                context_menu: true,
                ..CLEAR
            }
            .any()
        );
        assert!(
            InputSuppressors {
                search_overlay: true,
                ..CLEAR
            }
            .any()
        );
        assert!(
            InputSuppressors {
                command_history: true,
                ..CLEAR
            }
            .any()
        );
        assert!(
            InputSuppressors {
                scrollbar_drag: true,
                ..CLEAR
            }
            .any()
        );
    }

    /// The reported case: with only the search overlay open, the wheel must
    /// still reach the active pane so the user can look at the matches.
    #[test]
    fn scroll_passes_through_for_search_overlay_on_the_active_pane() {
        let s = InputSuppressors {
            search_overlay: true,
            ..CLEAR
        };
        assert!(s.any(), "search must still suppress everything else");
        assert!(s.scroll_passes_through(PaneFocus::Active));
    }

    /// Scroll targets the active pane only, matching `write_input_to_terminal`.
    #[test]
    fn scroll_does_not_pass_through_on_an_inactive_pane() {
        let s = InputSuppressors {
            search_overlay: true,
            ..CLEAR
        };
        assert!(!s.scroll_passes_through(PaneFocus::Inactive));
    }

    /// Any other suppressor present alongside search keeps the wheel blocked.
    #[test]
    fn any_other_suppressor_blocks_scroll_even_with_search_open() {
        for s in [
            InputSuppressors {
                search_overlay: true,
                modal_or_drag: true,
                ..CLEAR
            },
            InputSuppressors {
                search_overlay: true,
                context_menu: true,
                ..CLEAR
            },
            InputSuppressors {
                search_overlay: true,
                command_history: true,
                ..CLEAR
            },
            InputSuppressors {
                search_overlay: true,
                scrollbar_drag: true,
                ..CLEAR
            },
        ] {
            assert!(
                !s.scroll_passes_through(PaneFocus::Active),
                "search must not re-enable scroll while {s:?} also suppresses"
            );
        }
    }

    /// Overlays other than search keep the old behaviour: fully blocked.
    #[test]
    fn non_search_suppressors_never_pass_scroll_through() {
        for s in [
            InputSuppressors {
                modal_or_drag: true,
                ..CLEAR
            },
            InputSuppressors {
                context_menu: true,
                ..CLEAR
            },
            InputSuppressors {
                command_history: true,
                ..CLEAR
            },
            InputSuppressors {
                scrollbar_drag: true,
                ..CLEAR
            },
        ] {
            assert!(!s.scroll_passes_through(PaneFocus::Active));
        }
    }
}

#[cfg(test)]
mod overlay_suppress_input_tests {
    /// Test the one-frame suppression state machine for overlay dismiss.
    ///
    /// The `suppress_input` flag is computed as:
    ///   `ui_overlay_open || self.overlay_was_open_last_frame`
    /// and `overlay_was_open_last_frame` is then set to `ui_overlay_open`.
    ///
    /// This test verifies the state machine transitions without requiring a
    /// full egui context by exercising the boolean logic directly.
    #[test]
    fn suppress_input_state_machine() {
        // Simulates `overlay_was_open_last_frame` field on the widget.
        let mut overlay_was_open_last_frame = false;

        // Helper: compute suppress_input for one "frame" and update the
        // tracking field.  Returns the suppress_input value for that frame.
        let mut frame = |overlay_is_open: bool| -> bool {
            let suppress = overlay_is_open || overlay_was_open_last_frame;
            overlay_was_open_last_frame = overlay_is_open;
            suppress
        };

        // Frame 1: overlay not open, never was → input NOT suppressed.
        assert!(!frame(false), "frame 1: no overlay → no suppression");

        // Frame 2: overlay opens → input suppressed.
        assert!(frame(true), "frame 2: overlay open → suppressed");

        // Frame 3: overlay still open → input suppressed.
        assert!(frame(true), "frame 3: overlay still open → suppressed");

        // Frame 4: overlay closes (dismiss click) → input STILL suppressed
        // because overlay_was_open_last_frame is true.
        assert!(frame(false), "frame 4: dismiss frame → still suppressed");

        // Frame 5: overlay closed, was closed last frame → input allowed.
        assert!(!frame(false), "frame 5: fully closed → input allowed");

        // Frame 6: verify stable — stays unsuppressed.
        assert!(!frame(false), "frame 6: stable → input allowed");
    }

    /// Verify that `overlay_was_open_last_frame` starts `false` on a fresh
    /// widget, matching the initializer in `FreminalTerminalWidget::new()`.
    #[test]
    fn initial_state_does_not_suppress() {
        // Simulates the initial state of the field after construction.
        let overlay_was_open_last_frame = false;
        let overlay_is_open = false;
        let suppress = overlay_is_open || overlay_was_open_last_frame;
        assert!(!suppress, "fresh widget should not suppress input");
    }

    /// Issue #453: a pane-border drag-to-resize must suppress terminal
    /// input the same way a modal/menu overlay does. The `suppress_input`
    /// flag is computed as:
    ///   `ui_overlay_open || border_drag_active || self.overlay_was_open_last_frame`
    /// Verify that `border_drag_active` alone (no overlay open, no
    /// one-frame latch active) is sufficient to suppress input.
    #[test]
    fn border_drag_active_suppresses_input() {
        let ui_overlay_open = false;
        let overlay_was_open_last_frame = false;

        let border_drag_active = true;
        let suppress = ui_overlay_open || border_drag_active || overlay_was_open_last_frame;
        assert!(
            suppress,
            "border_drag_active alone must suppress input, matching the \
             ui_overlay_open || border_drag_active || overlay_was_open_last_frame \
             computation in FreminalTerminalWidget::show"
        );

        let border_drag_active = false;
        let suppress = ui_overlay_open || border_drag_active || overlay_was_open_last_frame;
        assert!(
            !suppress,
            "with everything else false, no border drag must NOT suppress input"
        );
    }

    /// Issue #453 root cause: a pane-border drag sensor sits geometrically
    /// inside the adjacent pane's `terminal_rect`, so the same press+drag
    /// also starts/extends a phantom text selection in that pane.
    ///
    /// This locks in that the border-drag suppression branch fully CLEARS
    /// the phantom selection (`SelectionState::clear`) rather than
    /// finalizing-and-keeping it (`SelectionState::finalize_interrupted_drag`,
    /// used for every OTHER suppression cause). Finalizing would keep a
    /// real anchor != end range as a "completed" selection — which is
    /// exactly the bug: the phantom selection painted during a divider
    /// drag would survive the drag ending instead of disappearing.
    #[test]
    fn border_drag_clears_phantom_selection() {
        use crate::gui::view_state::{LogicalCell, SelectionState};
        use freminal_common::buffer_states::row_number::RowNumber;

        // Build a phantom in-progress selection exactly as described in
        // the root-cause diagnostic: one endpoint pinned at the mouse-down
        // anchor, the other tracking the drag, `is_selecting = true`.
        let make_phantom = || SelectionState {
            anchor: Some(LogicalCell {
                col: 2,
                row: RowNumber::new(3),
            }),
            end: Some(LogicalCell {
                col: 9,
                row: RowNumber::new(3),
            }),
            is_selecting: true,
            ..SelectionState::default()
        };

        // Contrast case: on every OTHER suppression cause, the existing
        // `finalize_interrupted_drag()` KEEPS a real range as a completed
        // selection (only `is_selecting` is cleared).
        let mut finalized = make_phantom();
        finalized.finalize_interrupted_drag();
        assert!(
            finalized.has_selection(),
            "finalize_interrupted_drag must KEEP a real anchor != end range"
        );
        assert!(!finalized.is_selecting, "drag flag must be cleared");
        assert_eq!(
            finalized.anchor,
            Some(LogicalCell {
                col: 2,
                row: RowNumber::new(3)
            })
        );
        assert_eq!(
            finalized.end,
            Some(LogicalCell {
                col: 9,
                row: RowNumber::new(3)
            })
        );

        // Fix under test: when the suppression cause is a border drag, the
        // widget must call `clear()` instead, fully discarding the phantom
        // rather than keeping it as a "completed" selection.
        let mut cleared = make_phantom();
        cleared.clear();
        assert!(
            !cleared.has_selection(),
            "border-drag suppression must fully clear the phantom selection"
        );
        assert!(!cleared.is_selecting);
        assert_eq!(cleared.anchor, None);
        assert_eq!(cleared.end, None);
    }

    /// Issue #453 review: the release-click that ends a pane-border drag
    /// must not leak through to the terminal on the same frame
    /// `border_drag_active` goes false, mirroring the existing overlay
    /// one-frame release tail. `suppress_input` is computed as:
    ///   `ui_overlay_open || border_drag_active
    ///       || overlay_was_open_last_frame || border_drag_was_active_last_frame`
    /// and `border_drag_was_active_last_frame` is then set to
    /// `border_drag_active`, kept as its own latch separate from
    /// `overlay_was_open_last_frame`.
    #[test]
    fn border_drag_one_frame_release_tail() {
        let mut overlay_was_open_last_frame = false;
        let mut border_drag_was_active_last_frame = false;

        let mut frame = |ui_overlay_open: bool, border_drag_active: bool| -> bool {
            let suppress = ui_overlay_open
                || border_drag_active
                || overlay_was_open_last_frame
                || border_drag_was_active_last_frame;
            overlay_was_open_last_frame = ui_overlay_open;
            border_drag_was_active_last_frame = border_drag_active;
            suppress
        };

        // Frame 1: no overlay, no drag → input NOT suppressed.
        assert!(!frame(false, false), "frame 1: idle → no suppression");

        // Frame 2: drag starts → input suppressed.
        assert!(frame(false, true), "frame 2: drag active → suppressed");

        // Frame 3: drag still in progress → input suppressed.
        assert!(
            frame(false, true),
            "frame 3: drag still active → suppressed"
        );

        // Frame 4: drag ends (release click lands this frame) → input
        // STILL suppressed because `border_drag_was_active_last_frame` is
        // true, exactly mirroring the overlay dismiss-click tail.
        assert!(
            frame(false, false),
            "frame 4: drag-release frame → still suppressed"
        );

        // Frame 5: drag ended last frame, no overlay → input allowed again.
        assert!(
            !frame(false, false),
            "frame 5: fully settled → input allowed"
        );

        // Frame 6: verify stable — stays unsuppressed.
        assert!(!frame(false, false), "frame 6: stable → input allowed");
    }
}

#[cfg(test)]
mod shell_escape_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::path::Path;

    use super::{dropped_files_payload, shell_escape_path};

    #[test]
    fn simple_path() {
        let result = shell_escape_path(Path::new("/home/user/file.txt"));
        assert_eq!(result, "'/home/user/file.txt'");
    }

    #[test]
    fn path_with_spaces() {
        let result = shell_escape_path(Path::new("/home/user/my file.txt"));
        assert_eq!(result, "'/home/user/my file.txt'");
    }

    #[test]
    fn path_with_single_quote() {
        let result = shell_escape_path(Path::new("/home/user/it's a file"));
        assert_eq!(result, "'/home/user/it'\\''s a file'");
    }

    #[test]
    fn path_with_multiple_single_quotes() {
        let result = shell_escape_path(Path::new("a'b'c"));
        assert_eq!(result, "'a'\\''b'\\''c'");
    }

    #[test]
    fn path_with_special_chars() {
        let result = shell_escape_path(Path::new("/home/user/$var & (parens)"));
        assert_eq!(result, "'/home/user/$var & (parens)'");
    }

    #[test]
    fn empty_path() {
        let result = shell_escape_path(Path::new(""));
        assert_eq!(result, "''");
    }

    // ── `dropped_files_payload` ──────────────────────────────────────

    fn payload_of(paths: &[&str]) -> String {
        dropped_files_payload(paths.iter().map(Path::new))
    }

    #[test]
    fn payload_for_no_files_is_empty() {
        assert_eq!(payload_of(&[]), "");
    }

    /// The trailing space is deliberate: it leaves the shell's argument
    /// finished so the user can keep typing after the drop.
    #[test]
    fn payload_for_one_file_is_escaped_with_a_trailing_space() {
        assert_eq!(payload_of(&["/tmp/a.txt"]), "'/tmp/a.txt' ");
    }

    #[test]
    fn payload_separates_multiple_files_with_single_spaces() {
        assert_eq!(
            payload_of(&["/tmp/a.txt", "/tmp/b c.txt"]),
            "'/tmp/a.txt' '/tmp/b c.txt' "
        );
    }

    #[test]
    fn payload_escapes_each_path_independently() {
        assert_eq!(
            payload_of(&["it's", "$plain"]),
            "'it'\\''s' '$plain' ",
            "quote escaping must apply per path, not to the joined string"
        );
    }

    /// egui 0.36 replaced the `Option<PathBuf>` this code used to gate on with
    /// a plain `&Path`. An empty path must be skipped, not escaped to `''`,
    /// which would inject an empty argument into the user's command line.
    #[test]
    fn payload_skips_empty_paths() {
        assert_eq!(payload_of(&[""]), "", "a lone empty path sends nothing");
        assert_eq!(payload_of(&["", ""]), "");
    }

    /// Regression guard for the separator placement: the space is emitted
    /// based on what has already been written, so a skipped *first* entry
    /// must not leave a leading space.
    #[test]
    fn payload_does_not_emit_a_leading_space_when_the_first_path_is_skipped() {
        assert_eq!(payload_of(&["", "/tmp/a.txt"]), "'/tmp/a.txt' ");
    }

    #[test]
    fn payload_skips_an_empty_path_between_two_real_ones() {
        assert_eq!(
            payload_of(&["/tmp/a.txt", "", "/tmp/b.txt"]),
            "'/tmp/a.txt' '/tmp/b.txt' "
        );
    }
}

#[cfg(test)]
mod placeholder_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::{format_placeholder_text, hit_test_placeholder};
    use egui::{Pos2, Rect, pos2, vec2};
    use freminal_common::buffer_states::command_block::CommandBlockId;

    #[test]
    fn format_singular() {
        assert_eq!(
            format_placeholder_text(1, 80),
            "▶ 1 line hidden — click to unfold"
        );
    }

    #[test]
    fn format_plural() {
        assert_eq!(
            format_placeholder_text(7, 80),
            "▶ 7 lines hidden — click to unfold"
        );
    }

    #[test]
    fn format_zero_is_plural() {
        assert_eq!(
            format_placeholder_text(0, 80),
            "▶ 0 lines hidden — click to unfold"
        );
    }

    #[test]
    fn format_truncates_when_narrow() {
        let result = format_placeholder_text(123, 10);
        // 10 chars total, last is the ellipsis
        assert_eq!(result.chars().count(), 10);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn format_falls_back_when_very_narrow() {
        assert_eq!(format_placeholder_text(5, 1), "▶");
    }

    #[test]
    fn format_empty_when_zero_width() {
        assert_eq!(format_placeholder_text(5, 0), "");
    }

    #[test]
    fn hit_test_inside() {
        let id = CommandBlockId(42);
        let rects = vec![(Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 20.0)), id)];
        assert_eq!(hit_test_placeholder(&rects, pos2(50.0, 10.0)), Some(id));
    }

    #[test]
    fn hit_test_outside() {
        let id = CommandBlockId(42);
        let rects = vec![(Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 20.0)), id)];
        assert_eq!(hit_test_placeholder(&rects, pos2(200.0, 200.0)), None);
    }

    #[test]
    fn hit_test_empty_list() {
        assert_eq!(hit_test_placeholder(&[], Pos2::new(10.0, 10.0)), None);
    }

    #[test]
    fn hit_test_multiple_rects_returns_first_containing() {
        let id_a = CommandBlockId(1);
        let id_b = CommandBlockId(2);
        let rects = vec![
            (Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 20.0)), id_a),
            (
                Rect::from_min_size(pos2(0.0, 40.0), vec2(100.0, 20.0)),
                id_b,
            ),
        ];
        assert_eq!(hit_test_placeholder(&rects, pos2(50.0, 50.0)), Some(id_b));
        assert_eq!(hit_test_placeholder(&rects, pos2(50.0, 10.0)), Some(id_a));
    }
}

#[cfg(test)]
mod pane_render_cache_repaint_delay_tests {
    //! Subtask 121.12: [`PaneRenderCache::request_repaint_after`] /
    //! [`PaneRenderCache::take_pending_repaint_delay`] — the per-pane
    //! aggregation seam every in-frame repaint requester now folds through,
    //! instead of calling `ui.ctx().request_repaint_after()` directly.

    use super::PaneRenderCache;
    use std::time::Duration;

    #[test]
    fn no_request_drains_to_none() {
        let mut cache = PaneRenderCache::new();
        assert_eq!(cache.take_pending_repaint_delay(), None);
    }

    #[test]
    fn single_request_is_returned_unchanged() {
        let mut cache = PaneRenderCache::new();
        cache.request_repaint_after(Duration::from_millis(16));
        assert_eq!(
            cache.take_pending_repaint_delay(),
            Some(Duration::from_millis(16))
        );
    }

    #[test]
    fn repeated_requests_fold_to_the_minimum_regardless_of_order() {
        let mut cache = PaneRenderCache::new();
        cache.request_repaint_after(Duration::from_millis(250));
        cache.request_repaint_after(Duration::from_millis(16));
        cache.request_repaint_after(Duration::from_millis(100));
        assert_eq!(
            cache.take_pending_repaint_delay(),
            Some(Duration::from_millis(16))
        );

        // Order must not matter — smallest-wins either way.
        let mut cache = PaneRenderCache::new();
        cache.request_repaint_after(Duration::from_millis(16));
        cache.request_repaint_after(Duration::from_millis(250));
        assert_eq!(
            cache.take_pending_repaint_delay(),
            Some(Duration::from_millis(16))
        );
    }

    #[test]
    fn take_drains_the_cache_so_a_stale_delay_never_survives_into_the_next_frame() {
        let mut cache = PaneRenderCache::new();
        cache.request_repaint_after(Duration::from_millis(16));
        assert_eq!(
            cache.take_pending_repaint_delay(),
            Some(Duration::from_millis(16))
        );
        // Second drain (as if a new frame started with no new requests):
        // must be `None`, not the previous frame's value.
        assert_eq!(cache.take_pending_repaint_delay(), None);
    }
}

#[cfg(test)]
mod terminal_rect_origin_tests {
    //! Subtask 122.15, adapted by Task 124.3a's review: proves
    //! `PublishedFrameState::pane_terminal_origin` — which now derives its
    //! answer from `PanePointerReportInputs.terminal_rect.min` rather than
    //! a second, parallel `pane_terminal_origins` map (removed as a
    //! redundant, driftable copy once `PanePointerReportInputs` already
    //! carried the identical `terminal_rect`) — agrees with what
    //! `FreminalTerminalWidget::show` itself computes for `terminal_rect`,
    //! for the *same* frame's geometry.
    //!
    //! `show()` cannot be driven directly in a unit test — it needs a live
    //! `Ui`, a GL-context-backed `RenderState`, and PTY channels. So this
    //! pins the invariant one level down, at `terminal_rect_origin`: the
    //! pure helper `show()` calls to build `terminal_rect.min` (see the
    //! call site a few hundred lines above `impl FreminalTerminalWidget`).
    //! The test below reconstructs `terminal_rect` the *exact* way `show()`
    //! does — `egui::Rect::from_min_max(point_to_egui(terminal_rect_origin(..)), pane_rect.max)`
    //! — publishes it as `PanePointerReportInputs` the same way `app_impl`
    //! lifts `cache.pointer_report_inputs`, and asserts
    //! `pane_terminal_origin` returns exactly that rect's `.min` corner. A
    //! test that instead re-derived the origin independently (e.g. straight
    //! from `pane_rect.min.x + gutter_inset` inline, without going through
    //! `terminal_rect_origin`) would pass even if `show()`'s real
    //! `terminal_rect` construction silently drifted from this helper — see
    //! the prohibition on that in the helper's own doc comment.

    use super::terminal_rect_origin;
    use crate::gui::geometry_interop::{point_from_egui, point_to_egui};
    use crate::gui::panes::PaneIdGenerator;
    use crate::gui::published_frame_state::{PanePointerReportInputs, PublishedFrameState};

    /// Fixed geometry mirroring the crate doc's example: a pane whose
    /// top-left is not at the window origin (200, 50), an 808x500 available
    /// area, and a non-zero command-block gutter inset (18 logical points —
    /// larger than a single gutter test's 8px, to make a copy-paste-only
    /// "origin == `pane_rect.min`" bug visible).
    fn fixture() -> (egui::Rect, f32) {
        let pane_rect =
            egui::Rect::from_min_max(egui::pos2(200.0, 50.0), egui::pos2(1008.0, 550.0));
        let gutter_inset = 18.0_f32;
        (pane_rect, gutter_inset)
    }

    /// The `PanePointerReportInputs` `show()` would publish for a given
    /// `terminal_rect` — only `terminal_rect` matters for this test; the
    /// rest are placeholders matching `PanePointerReportInputs::default`'s
    /// "no suppressors, unity scale" shape.
    fn report_inputs_for(terminal_rect: egui::Rect) -> PanePointerReportInputs {
        PanePointerReportInputs {
            terminal_rect,
            ..PanePointerReportInputs::default()
        }
    }

    /// The published origin must equal `terminal_rect.min` as `show()`
    /// constructs it — reconstructed here verbatim from the same helper.
    #[test]
    fn published_origin_matches_terminal_rect_min_as_show_constructs_it() {
        let (pane_rect, gutter_inset) = fixture();

        // Exactly what `show()` does: compute the origin via the shared
        // helper, then build `terminal_rect` from it.
        let terminal_origin = terminal_rect_origin(pane_rect, gutter_inset);
        let terminal_rect = egui::Rect::from_min_max(point_to_egui(terminal_origin), pane_rect.max);

        // Exactly what `app_impl` does right after `show()` returns: lift
        // the pane's `PanePointerReportInputs` (carrying that same
        // `terminal_rect`) into `PublishedFrameState`.
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_id = id_gen.next_id();
        let mut published = PublishedFrameState::new();
        published.publish_pane_pointer_report_inputs(pane_id, report_inputs_for(terminal_rect));

        assert_eq!(
            published.pane_terminal_origin(pane_id),
            Some(point_from_egui(terminal_rect.min)),
            "the origin pane_terminal_origin returns must equal the min corner of the \
             exact terminal_rect show() builds this frame"
        );
        // Sanity: the gutter inset must actually have moved the origin off
        // the pane's left edge, so this test could not pass by accident if
        // the inset were silently dropped.
        assert!((terminal_rect.min.x - pane_rect.min.x - gutter_inset).abs() < f32::EPSILON);
        assert!((terminal_rect.min.y - pane_rect.min.y).abs() < f32::EPSILON);
    }

    /// A zero gutter inset (no command-block gutter, or an alt-screen
    /// frame) leaves the origin at the pane's own top-left corner.
    #[test]
    fn zero_gutter_inset_origin_is_pane_top_left() {
        let (pane_rect, _) = fixture();
        let origin = terminal_rect_origin(pane_rect, 0.0);
        assert_eq!(point_to_egui(origin), pane_rect.min);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod pane_cursor_focus_geometry_tests {
    //! Task 127.5: what each pane's cursor looks like once focus is wired in.
    //!
    //! `FreminalTerminalWidget::show` needs a live egui `Ui` and a GL context,
    //! so it cannot be driven from a unit test. These tests instead drive the
    //! narrowest seam that still runs the real code on both sides of the wiring
    //! `show()` performs: [`frame_cursor_state`] (the very function `show()`
    //! calls to derive focus and appearance) produces the appearance that
    //! becomes the [`CursorDrawParams`] [`build_cursor_verts_only`] -- the
    //! builder the cursor-only path uses, and which emits identical geometry to
    //! the full-rebuild path for identical params -- turns into quads. The
    //! focus pinning of the blink phase is the one piece of `show()` that is
    //! not reachable from here.

    use crate::gui::colors::cursor_f;
    use crate::gui::frame_drain::WindowFocus;
    use crate::gui::renderer::{
        CURSOR_QUAD_FLOATS, CursorBlinkPhase, CursorDrawParams, build_cursor_verts_only,
    };
    use crate::gui::terminal::cursor_appearance::{CursorFocus, CursorVisibility, EchoState};
    use crate::gui::terminal::input::PaneFocus;
    use freminal_common::config::{Config, UnfocusedCursorStyle};
    use freminal_common::cursor::CursorVisualStyle;
    use freminal_common::themes::CATPPUCCIN_MOCHA;

    use super::FreminalTerminalWidget;
    use crate::gui::terminal::cursor_blink::{
        ActivationBlinkReset, CursorStateInputs, frame_cursor_state,
    };

    /// The cursor floats a pane would emit, resolved exactly as `show()`
    /// resolves them from its pane focus, window focus, and DECTCEM state.
    fn cursor_floats(
        pane: PaneFocus,
        window: WindowFocus,
        visibility: CursorVisibility,
        unfocused_style: UnfocusedCursorStyle,
    ) -> Vec<f32> {
        let style = CursorVisualStyle::BlockCursorSteady;
        let state = frame_cursor_state(&CursorStateInputs {
            pane_focus: pane,
            window_focus: window,
            previous_focus: CursorFocus::Focused,
            activation_reset: ActivationBlinkReset::NotPending,
            snapshot_visible: visibility,
            echo: EchoState::Normal,
            style: &style,
            unfocused_style,
            time: 0.0,
            anchor: None,
        });
        let params = CursorDrawParams {
            appearance: state.appearance,
            col: 3.0,
            row: 1.0,
            color: cursor_f(&CATPPUCCIN_MOCHA, None),
            x_scale: 1.0,
            blink_on: CursorBlinkPhase::On,
        };
        build_cursor_verts_only(8, 16, &params)
    }

    #[test]
    fn active_pane_in_a_focused_window_emits_one_solid_quad() {
        let floats = cursor_floats(
            PaneFocus::Active,
            WindowFocus::Focused,
            CursorVisibility::Shown,
            UnfocusedCursorStyle::Hollow,
        );
        assert_eq!(floats.len(), CURSOR_QUAD_FLOATS);
    }

    #[test]
    fn inactive_pane_emits_four_hollow_border_quads() {
        let floats = cursor_floats(
            PaneFocus::Inactive,
            WindowFocus::Focused,
            CursorVisibility::Shown,
            UnfocusedCursorStyle::Hollow,
        );
        assert_eq!(floats.len(), 4 * CURSOR_QUAD_FLOATS);
    }

    #[test]
    fn active_pane_in_an_unfocused_window_emits_hollow_quads() {
        let floats = cursor_floats(
            PaneFocus::Active,
            WindowFocus::Unfocused,
            CursorVisibility::Shown,
            UnfocusedCursorStyle::Hollow,
        );
        assert_eq!(floats.len(), 4 * CURSOR_QUAD_FLOATS);
    }

    #[test]
    fn unchanged_style_keeps_a_single_quad_for_an_inactive_pane() {
        let floats = cursor_floats(
            PaneFocus::Inactive,
            WindowFocus::Focused,
            CursorVisibility::Shown,
            UnfocusedCursorStyle::Unchanged,
        );
        assert_eq!(floats.len(), CURSOR_QUAD_FLOATS);
    }

    #[test]
    fn hidden_style_emits_nothing_for_an_inactive_pane() {
        let floats = cursor_floats(
            PaneFocus::Inactive,
            WindowFocus::Focused,
            CursorVisibility::Shown,
            UnfocusedCursorStyle::Hidden,
        );
        assert_eq!(floats, Vec::<f32>::new());
    }

    /// DECTCEM-off wins over every focus state: a hidden cursor is never
    /// resurrected as a hollow one by the pane losing focus.
    #[test]
    fn dectcem_off_emits_nothing_in_any_focus_state() {
        for (pane, window) in [
            (PaneFocus::Active, WindowFocus::Focused),
            (PaneFocus::Inactive, WindowFocus::Focused),
            (PaneFocus::Active, WindowFocus::Unfocused),
            (PaneFocus::Inactive, WindowFocus::Unfocused),
        ] {
            let floats = cursor_floats(
                pane,
                window,
                CursorVisibility::Hidden,
                UnfocusedCursorStyle::Hollow,
            );
            assert_eq!(floats, Vec::<f32>::new(), "{pane:?} / {window:?}");
        }
    }

    #[test]
    fn unfocused_cursor_style_is_cached_at_construction_and_on_config_apply() {
        let ctx = egui::Context::default();
        let mut widget = FreminalTerminalWidget::new(&ctx, &Config::default())
            .expect("widget construction must succeed with the bundled default font");
        assert_eq!(
            widget.unfocused_cursor_style,
            Config::default().cursor.unfocused_style,
            "construction must cache the configured style"
        );

        let old_config = Config::default();
        let mut new_config = Config::default();
        new_config.cursor.unfocused_style = UnfocusedCursorStyle::Hidden;
        widget.apply_config_changes_no_ctx(&old_config, &new_config);

        assert_eq!(widget.unfocused_cursor_style, UnfocusedCursorStyle::Hidden);
    }
}
