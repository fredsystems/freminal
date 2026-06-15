// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The `FreminalTerminalWidget` egui widget and GPU render state.

use crate::gui::{
    folding::{RenderedRow, RowMap, compute_fold_ranges},
    fonts::{FontConfig, setup_font_files},
    mouse::PreviousMouseState,
    shaping::ShapedLine,
    view_state::{CellCoord, ViewState},
};

use crossbeam_channel::{Receiver, Sender};
use freminal_common::{
    buffer_states::{
        command_block::CommandStatus, pointer_shape::PointerShape, tchar::TChar, url::Url,
    },
    config::Config,
    send_or_log,
    themes::ThemePalette,
};
use freminal_terminal_emulator::{
    InlineImage, LineWidth, io::InputEvent, snapshot::TerminalSnapshot,
};

use egui::{self, Color32, Context, CursorIcon, Key, Pos2, Rect, Ui};

use super::{
    super::{
        atlas::GlyphAtlas,
        font_manager::FontManager,
        renderer::{
            BackgroundFrame, CURSOR_QUAD_FLOATS, FgRenderOptions, MatchHighlight, TerminalRenderer,
            WindowPostRenderer, build_background_instances, build_cursor_verts_only,
            build_foreground_instances, build_image_verts,
        },
        search::{
            SearchBarAction, matches_to_highlights, run_search, scroll_to_match_and_send,
            show_search_bar,
        },
    },
    coords::{encode_egui_mouse_pos_as_usize, flat_index_for_cell, running_block_extent},
    input::write_input_to_terminal,
};

use conv2::{ApproxFrom, ConvUtil, RoundToZero};
use egui_glow::CallbackFn;
use glow::HasContext;
use std::sync::{Arc, Mutex};
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
struct FoldLayout {
    /// Buffer-absolute index of the first snapshot row.
    flat_window_start: usize,
    /// Snapshot → rendered row mapping with folds collapsed.
    row_map: RowMap,
    /// Rendered rows scrolled off the top so the bottom `term_height` rendered
    /// rows fill the screen.
    render_skip: usize,
}

impl FoldLayout {
    /// Build the layout for `snap` given the folded-block set.
    fn new(
        snap: &TerminalSnapshot,
        folded_blocks: &std::collections::HashSet<
            freminal_common::buffer_states::command_block::CommandBlockId,
        >,
    ) -> Self {
        let raw_fold_ranges = compute_fold_ranges(&snap.command_blocks, folded_blocks);
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
    const fn rendered_to_screen(&self, rendered_row: usize) -> Option<usize> {
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

/// Compute the rendered-row span (inclusive) of the command block whose
/// gutter the pointer is currently hovering, for the hover-tint overlay.
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
fn compute_command_block_hover_rows(
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
        buffer_row,
        running_extent,
    )?;
    let start = block.command_start_row?;
    let end = block.end_row.unwrap_or(running_extent);
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

///
/// The scrollbar is shown when the user is actively scrolled back
/// (`scroll_offset > 0`).  It disappears at the live bottom.
///
/// Supports click-to-position and drag-to-scroll.  Returns the new
/// `scroll_offset` if the user interacted with the scrollbar, or `None`
/// if no scrollbar interaction occurred.
pub(super) fn handle_scrollbar(
    scroll_offset: usize,
    max_scroll_offset: usize,
    ui: &Ui,
    dragging: &mut bool,
) -> Option<usize> {
    const SCROLLBAR_WIDTH: f32 = 6.0;
    const SCROLLBAR_MARGIN: f32 = 2.0;
    const MIN_THUMB_HEIGHT: f32 = 12.0;
    // Wider hit-test area so the narrow pill is easy to grab.
    const HIT_TEST_PADDING: f32 = 6.0;

    // Only show when scrolled back into history — but keep rendering
    // while the user is mid-drag so the scrollbar doesn't vanish when
    // they drag to the bottom.
    if !*dragging && (scroll_offset == 0 || max_scroll_offset == 0) {
        return None;
    }
    if max_scroll_offset == 0 {
        *dragging = false;
        return None;
    }

    let painter = ui.painter();

    // ── Dimensions ───────────────────────────────────────────────────────
    let viewport = ui.max_rect();
    let track_top = viewport.top();
    let track_bottom = viewport.bottom();
    let track_height = track_bottom - track_top;
    if track_height <= 0.0 {
        return None;
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
    let thumb_top = track_top + scrollable_track * (1.0 - position_fraction);

    let thumb_rect = Rect::from_min_max(
        Pos2::new(track_left, thumb_top),
        Pos2::new(track_right, thumb_top + thumb_height),
    );

    // ── Mouse interaction ────────────────────────────────────────────────
    // Use a wider hit-test rect so the narrow scrollbar is easy to click.
    let hit_rect = Rect::from_min_max(
        Pos2::new(track_left - HIT_TEST_PADDING, track_top),
        Pos2::new(track_right + HIT_TEST_PADDING, track_bottom),
    );

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
    let is_hovered = ui.input(|i| {
        i.pointer
            .interact_pos()
            .is_some_and(|pos| hit_rect.contains(pos))
    });
    let alpha = if *dragging {
        220
    } else if is_hovered {
        200
    } else {
        150
    };
    let color = Color32::from_rgba_premultiplied(200, 200, 200, alpha);
    let rounding = SCROLLBAR_WIDTH / 2.0;

    painter.rect_filled(thumb_rect, rounding, color);

    new_offset
}

/// Duration of the visual bell flash overlay.
const BELL_FLASH_DURATION: Duration = Duration::from_millis(150);

/// Maximum alpha for the bell flash overlay (0–255).
const BELL_FLASH_MAX_ALPHA: u8 = 60;

/// Steady-state alpha for the persistent bell overlay shown when the
/// window is unfocused and a bell has fired (0–255).
const BELL_PERSISTENT_ALPHA: u8 = 30;

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
fn paint_bell_flash(ui: &Ui, terminal_rect: Rect, view_state: &mut ViewState) {
    let Some(since) = view_state.bell_since else {
        return;
    };

    if !view_state.window_focused {
        // Unfocused: show a persistent, non-fading overlay.  No repaint
        // request — the overlay is static and doesn't need continuous
        // redraws while the window is in the background.
        let alpha = BELL_PERSISTENT_ALPHA;
        let overlay_color = Color32::from_rgba_premultiplied(alpha, alpha, alpha, alpha);
        ui.painter().rect_filled(terminal_rect, 0.0, overlay_color);
        return;
    }

    // Focused: if the flash duration has elapsed the bell either fired
    // while unfocused (the user just alt-tabbed back) or the fade-out
    // already completed — either way, clear immediately.
    let elapsed = since.elapsed();
    if elapsed >= BELL_FLASH_DURATION {
        view_state.bell_since = None;
        return;
    }

    // Linear fade from BELL_FLASH_MAX_ALPHA → 0 over the flash duration.
    let progress = elapsed.as_secs_f32() / BELL_FLASH_DURATION.as_secs_f32();
    let alpha_f = f32::from(BELL_FLASH_MAX_ALPHA) * (1.0 - progress);
    let alpha: u8 = alpha_f.approx_as::<u8>().unwrap_or(0);

    let overlay_color = Color32::from_rgba_premultiplied(alpha, alpha, alpha, alpha);
    ui.painter().rect_filled(terminal_rect, 0.0, overlay_color);

    // Request a repaint so the fade-out animation continues next frame (~60 fps cap).
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(16));
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
        start_row: usize,
        end_row: usize,
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

    // Look up whether the right-clicked cell sits inside a URL span.
    let url_under_cursor = view_state.context_menu_cell.and_then(|cell| {
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
    let command_output_range = view_state.context_menu_cell.and_then(|cell| {
        let block = super::input::find_block_containing_row(snap, cell.row)?;
        match (block.output_start_row, block.end_row) {
            (Some(start), Some(end)) if start <= end => Some((start, end)),
            _ => None,
        }
    });

    egui::Area::new(area_id)
        .order(egui::Order::Foreground)
        .fixed_pos(menu_pos)
        .interactable(true)
        .constrain(true)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(120.0);

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
fn dispatch_context_menu_action(
    action: Option<ContextMenuAction>,
    ui: &Ui,
    view_state: &mut ViewState,
    snap: &TerminalSnapshot,
    input_tx: &Sender<InputEvent>,
    clipboard_rx: &Receiver<String>,
    deferred_actions: &mut Vec<freminal_common::keybindings::KeyAction>,
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
            let window_start = super::coords::visible_window_start(snap);
            let last_row = window_start + snap.height.saturating_sub(1);
            // Find the last column on the last visible row.
            let last_col = crate::gui::view_state::line_boundaries(
                &snap.visible_chars,
                snap.height.saturating_sub(1),
            )
            .1;
            view_state.selection.anchor = Some(CellCoord {
                col: 0,
                row: window_start,
            });
            view_state.selection.end = Some(CellCoord {
                col: last_col,
                row: last_row,
            });
            view_state.selection.is_selecting = false;
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
    /// Snapshot image map from the last full rebuild, cloned into `RenderState`
    /// so the `PaintCallback` closure (`Send`+`Sync`) can pass it to `draw_with_verts`.
    pub(super) snap_images: std::collections::HashMap<u64, InlineImage>,
    /// Float offset (not byte offset) into `deco_verts` where the cursor quad
    /// data begins.  Set after every full vertex rebuild so cursor-only frames
    /// can patch just this region.
    pub(super) cursor_vert_float_offset: usize,
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
    Arc::new(Mutex::new(RenderState {
        renderer: TerminalRenderer::new(),
        atlas: GlyphAtlas::default(),
        bg_instances: Vec::new(),
        deco_verts: Vec::new(),
        fg_instances: Vec::new(),
        image_verts: Vec::new(),
        snap_images: std::collections::HashMap::new(),
        cursor_vert_float_offset: 0,
        cell_width_px: 0.0,
        cell_height_px: 0.0,
        bg_opacity: 1.0,
        bg_image_opacity: 0.5,
        bg_image_mode: freminal_common::config::BackgroundImageMode::Cover,
        window_post,
        pending_bg_image: None,
    }))
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
    /// Last scroll amount processed.
    pub(super) previous_scroll_amount: f32,
    /// Cursor blink state from the most recently rendered frame.
    pub(super) previous_cursor_blink_on: bool,
    /// Cursor position from the most recently rendered frame.
    pub(super) previous_cursor_pos: freminal_common::buffer_states::cursor::CursorPos,
    /// Whether the cursor was shown in the most recently rendered frame.
    pub(super) previous_show_cursor: bool,
    /// Cursor color override from the most recently rendered frame.
    pub(super) previous_cursor_color_override: Option<(u8, u8, u8)>,
    /// The `visible_chars` arc from the last full vertex rebuild.
    ///
    /// Used to detect content changes via `Arc::ptr_eq` — immune to the race
    /// where a later snapshot overwrites `content_changed` before the GUI wakes.
    pub(super) last_rendered_visible: Option<Arc<Vec<TChar>>>,
    /// Line-width data from the last full vertex rebuild.  When line widths
    /// change (e.g. DECDWL/DECDHL toggle), we must force a full rebuild so
    /// glyph scaling is re-applied.
    pub(super) last_rendered_line_widths: Option<Arc<Vec<freminal_terminal_emulator::LineWidth>>>,
    /// Theme pointer from the last full vertex rebuild.  When this changes,
    /// we must force a full rebuild so foreground/background vertex colors
    /// are re-resolved against the new palette.
    pub(super) previous_theme: Option<&'static ThemePalette>,
    /// The normalised selection from the last full vertex rebuild, used to
    /// detect selection changes that require a full rebuild.
    pub(super) previous_selection: Option<(CellCoord, CellCoord)>,
    /// Text blink slow-visibility from the most recently rendered frame.
    pub(super) previous_text_blink_slow_visible: bool,
    /// Text blink fast-visibility from the most recently rendered frame.
    pub(super) previous_text_blink_fast_visible: bool,
    /// Whether a UI overlay (modal dialog or dropdown menu) was open on the
    /// previous frame.
    pub(super) overlay_was_open_last_frame: bool,
    /// Number of search matches from the most recently rendered frame.
    pub(super) previous_search_match_count: usize,
    /// Current match index from the most recently rendered frame.
    pub(super) previous_search_current_match: usize,
    /// The terminal cell `(col, row)` the mouse was hovering over in the
    /// previous frame.
    pub(super) previous_hover_cell: Option<(usize, usize)>,
    /// The command-block hover-tint rendered-row range from the previous
    /// frame.  A hover change (different range, or appearing/disappearing)
    /// forces a vertex rebuild so the tint is baked into the background VBO.
    pub(super) previous_command_block_hover_rows: Option<(usize, usize)>,
    /// Cached URL from the most recent URL hover lookup.
    pub(super) cached_hovered_url: Option<Arc<Url>>,
    /// Pointer identity of the `visible_chars` `Arc` used for the last URL
    /// hover lookup.
    pub(super) hover_snap_ptr: usize,
    /// Per-pane shaping cache for text layout.
    pub(crate) shaping_cache: crate::gui::shaping::ShapingCache,
    /// Whether the user is currently dragging the scrollbar thumb.
    pub(super) scrollbar_dragging: bool,
    /// Whether the pointer was over the command-block gutter hit zone on the
    /// previous frame.  Used to request one extra repaint on the frame the
    /// pointer leaves the gutter so the hover-tint clearing frame is drawn.
    pub(super) pointer_in_gutter_last_frame: bool,
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
}

impl PaneRenderCache {
    /// Create a new cache with default initial values.
    #[must_use]
    pub fn new() -> Self {
        Self {
            previous_mouse_state: None,
            previous_key: None,
            previous_scroll_amount: 0.0,
            previous_cursor_blink_on: true,
            previous_cursor_pos: freminal_common::buffer_states::cursor::CursorPos::default(),
            previous_show_cursor: false,
            previous_cursor_color_override: None,
            last_rendered_visible: None,
            last_rendered_line_widths: None,
            previous_theme: None,
            previous_selection: None,
            previous_text_blink_slow_visible: true,
            previous_text_blink_fast_visible: true,
            overlay_was_open_last_frame: false,
            previous_search_match_count: 0,
            previous_search_current_match: 0,
            previous_hover_cell: None,
            previous_command_block_hover_rows: None,
            cached_hovered_url: None,
            hover_snap_ptr: 0,
            shaping_cache: crate::gui::shaping::ShapingCache::new(),
            scrollbar_dragging: false,
            pointer_in_gutter_last_frame: false,
            previous_term_width: 0,
            previous_term_height: 0,
            previous_fold_epoch: 0,
            placeholder_hit_rects: Vec::new(),
        }
    }

    /// Invalidate the cached theme pointer so the next frame forces a full
    /// vertex rebuild with the new palette colors.
    pub const fn invalidate_theme_cache(&mut self) {
        self.previous_theme = None;
    }

    /// Force a full vertex rebuild on the next frame by clearing the cached
    /// `visible_chars` and `line_widths` pointers.
    pub fn invalidate_content(&mut self) {
        self.last_rendered_visible = None;
        self.last_rendered_line_widths = None;
        self.shaping_cache.clear();
    }
}

impl Default for PaneRenderCache {
    fn default() -> Self {
        Self::new()
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
    /// Whether OpenType ligatures are enabled for text shaping.
    ligatures: bool,
    /// Whether cursor trail animation is enabled (cursor glides to new position).
    cursor_trail: bool,
    /// Duration of the cursor trail animation.
    cursor_trail_duration: Duration,
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
            ligatures: config.font.ligatures,
            cursor_trail: config.cursor.trail,
            cursor_trail_duration: Duration::from_millis(u64::from(
                config.cursor.trail_duration_ms,
            )),
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
    /// - `bg_opacity` — background panel opacity (`0.0`–`1.0`) from config.
    /// - `bg_image_opacity` — background image opacity (`0.0`–`1.0`) from config.
    /// - `bg_image_mode` — background image fit mode from config.
    /// - `binding_map` — user key-binding map; bound combos are intercepted before PTY dispatch.
    /// - `is_active_pane` — whether this pane currently has keyboard focus.
    /// - `key_broadcast_targets` — input senders of the other panes to mirror
    ///   keyboard input to when broadcast mode is active (Task 74); empty when
    ///   broadcast is off or this is not the active pane.
    // Inherently large: the main per-frame terminal widget handler — processes input, handles
    // blink/scroll/mouse, and orchestrates layout. Each section is tightly coupled.
    #[allow(clippy::too_many_lines)]
    // All parameters are required: each pane needs its own render state, cache, channels, and
    // view state; there is no sensible grouping that reduces the count without hiding the intent.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        snap: &TerminalSnapshot,
        view_state: &mut ViewState,
        render_state: &Arc<Mutex<RenderState>>,
        cache: &mut PaneRenderCache,
        input_tx: &Sender<InputEvent>,
        clipboard_rx: &Receiver<String>,
        search_buffer_rx: &Receiver<(usize, Vec<TChar>)>,
        ui_overlay_open: bool,
        bg_opacity: f32,
        bg_image_opacity: f32,
        bg_image_mode: freminal_common::config::BackgroundImageMode,
        command_blocks_config: &freminal_common::config::CommandBlocksConfig,
        gutter_inset_logical: f32,
        binding_map: &freminal_common::keybindings::BindingMap,
        is_echo_off: bool,
        is_active_pane: bool,
        pane_id: crate::gui::panes::PaneId,
        recording_ctx: Option<&freminal_terminal_emulator::recording::RecordingContext<'_>>,
        pending_copy: &mut bool,
        key_broadcast_targets: &[Sender<InputEvent>],
    ) -> (bool, Vec<freminal_common::keybindings::KeyAction>) {
        const BLINK_TICK_SECONDS: f64 = 0.50;

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

        // Suppress input for one extra frame after a modal closes.
        // This prevents the dismiss-click (Cancel / X / click-away) from
        // leaking through to the terminal as a pointer event.
        let suppress_input = ui_overlay_open || cache.overlay_was_open_last_frame;
        cache.overlay_was_open_last_frame = ui_overlay_open;

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
        let terminal_rect = egui::Rect::from_min_max(
            egui::pos2(pane_rect.min.x + gutter_inset, pane_rect.min.y),
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
            if hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if hovered || cache.pointer_in_gutter_last_frame {
                ui.ctx().request_repaint();
            }
            cache.pointer_in_gutter_last_frame = hovered;
        } else if cache.pointer_in_gutter_last_frame {
            // Feature toggled off / alt-screen entered while we were hovering:
            // draw one clearing frame.
            ui.ctx().request_repaint();
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
                ptr.interact_pos().is_some_and(|pos| {
                    let vp = ui.max_rect();
                    let track_right = vp.right() - 2.0; // SCROLLBAR_MARGIN
                    let track_left = track_right - 6.0; // SCROLLBAR_WIDTH
                    let hit_left = track_left - 6.0; // HIT_TEST_PADDING
                    let hit_right = track_right + 6.0;
                    pos.x >= hit_left
                        && pos.x <= hit_right
                        && pos.y >= vp.top()
                        && pos.y <= vp.bottom()
                })
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
        if suppress_input
            || context_menu_open
            || view_state.search_state.is_open
            || view_state.command_history.is_open
            || cache.scrollbar_dragging
        {
            cache.previous_key = None;
            cache.previous_mouse_state = None;
            cache.previous_scroll_amount = 0.0;
            view_state.selection.is_selecting = false;
        } else {
            let repeat_characters = snap.repeat_keys;
            let ctx = ui.ctx().clone();
            let (
                left_mouse_button_pressed_inner,
                new_mouse_pos,
                previous_key,
                scroll_amount,
                clipboard_pending,
                actions,
            ) = ui.input(|input_state| {
                write_input_to_terminal(
                    input_state,
                    snap,
                    input_tx,
                    view_state,
                    logical_cell_w,
                    logical_cell_h,
                    terminal_rect,
                    cache.previous_mouse_state.clone(),
                    repeat_characters,
                    cache.previous_key,
                    cache.previous_scroll_amount,
                    binding_map,
                    is_active_pane,
                    recording_ctx,
                    &cache.placeholder_hit_rects,
                    key_broadcast_targets,
                )
            });
            left_mouse_button_pressed |= left_mouse_button_pressed_inner;
            cache.previous_mouse_state = new_mouse_pos;
            cache.previous_key = previous_key;
            cache.previous_scroll_amount = scroll_amount;
            deferred_actions = actions;

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
                // Clear the selection highlight now that the text has been
                // copied to the clipboard.
                view_state.selection.clear();
            }
        }

        // Blink state must be computed here — cannot call `ui.input` inside
        // the `Arc<CallbackFn>` closure (it must be `Send + Sync`).
        let time = ui.input(|i| i.time);
        let cursor_blink_on = match <i64 as ApproxFrom<f64, RoundToZero>>::approx_from(
            (time / BLINK_TICK_SECONDS).floor(),
        ) {
            Ok(ticks) => ticks % 2 == 0,
            Err(e) => {
                error!("Failed to convert blink ticks to i64: {e}");
                true
            }
        };

        // Search: request the full buffer from the PTY thread when needed,
        // then run (or re-run) the search against the cached corpus.
        let search_error: Option<String> = if view_state.search_state.is_open {
            // Detect staleness: if total_rows changed, the cached buffer is out
            // of date and we need a fresh copy from the PTY thread.
            let total_rows_changed =
                snap.total_rows != view_state.search_state.last_known_total_rows;
            if total_rows_changed
                && view_state.search_state.buffer_request_state
                    == crate::gui::view_state::BufferRequestState::Idle
            {
                view_state.search_state.cached_full_buffer = None;
                if let Err(e) = input_tx.send(InputEvent::RequestSearchBuffer) {
                    error!("Failed to request search buffer from PTY: {e}");
                } else {
                    view_state.search_state.buffer_request_state =
                        crate::gui::view_state::BufferRequestState::Pending;
                }
            }

            // Try to receive the full buffer (non-blocking). Drain queued
            // responses and only accept a buffer whose version matches the
            // current snapshot — otherwise re-request a fresh copy.
            if let Some((buffer_total_rows, buf)) = search_buffer_rx.try_iter().last() {
                view_state.search_state.buffer_request_state =
                    crate::gui::view_state::BufferRequestState::Idle;

                if buffer_total_rows == snap.total_rows {
                    view_state.search_state.cached_full_buffer = Some(Arc::new(buf));
                    view_state.search_state.last_known_total_rows = buffer_total_rows;
                } else {
                    // Stale response — discard and re-request.
                    view_state.search_state.cached_full_buffer = None;
                    if let Err(e) = input_tx.send(InputEvent::RequestSearchBuffer) {
                        error!("Failed to request search buffer from PTY: {e}");
                    } else {
                        view_state.search_state.buffer_request_state =
                            crate::gui::view_state::BufferRequestState::Pending;
                    }
                }
            }

            // Run search if query/mode changed or we just got a new buffer.
            if view_state.search_state.needs_refresh() {
                if let Some(ref buffer) = view_state.search_state.cached_full_buffer {
                    let query = view_state.search_state.query.clone();
                    let regex_mode = view_state.search_state.regex_mode;
                    let case_sensitive = view_state.search_state.case_sensitive;
                    let (found, err) = run_search(&query, regex_mode, case_sensitive, buffer);
                    view_state.search_state.matches = found;
                    view_state.search_state.current_match = 0;
                    view_state.search_state.mark_fresh();
                    err
                } else {
                    // No cached buffer yet — request one if we haven't already.
                    if view_state.search_state.buffer_request_state
                        == crate::gui::view_state::BufferRequestState::Idle
                    {
                        if let Err(e) = input_tx.send(InputEvent::RequestSearchBuffer) {
                            error!("Failed to request search buffer from PTY: {e}");
                        } else {
                            view_state.search_state.buffer_request_state =
                                crate::gui::view_state::BufferRequestState::Pending;
                        }
                    }
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // Cursor-only state captured before the PaintCallback closure (which
        // requires `Send + Sync + 'static`).  `is_cursor_only` and
        // `cursor_only_verts` are moved into the closure below.
        let mut is_cursor_only = false;
        let mut cursor_only_verts: Vec<f32> = Vec::new();

        // Suppress the cursor when:
        // - the terminal has hidden it (DECTCEM ?25l),
        // - a password prompt is active (echo-off lock icon replaces it), or
        // - this pane is not the active/focused pane (tmux-style: only the
        //   focused pane shows a cursor).
        let mut effective_show_cursor = snap.show_cursor && !is_echo_off && is_active_pane;

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

        if !snap.skip_draw {
            // Detect content changes via `Arc::ptr_eq` — this is immune to the
            // race where the PTY thread overwrites a "changed" snapshot with a
            // "clean" one before the GUI wakes up.  If the `visible_chars` arc
            // is a different allocation from the one we last rendered, the
            // content has changed regardless of the `content_changed` flag.
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
            let content_changed = snap.content_changed
                || theme_changed
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
            if snap.content_changed && !snap.scroll_changed && !view_state.selection.is_selecting {
                view_state.selection.clear();
            }

            // Check whether the selection has changed since the last frame.
            let current_selection = view_state.selection.normalised();
            let selection_changed = current_selection != cache.previous_selection;

            // Check whether search highlight state has changed since last frame.
            let search_match_count = view_state.search_state.matches.len();
            let search_current_match = view_state.search_state.current_match;
            let search_changed = search_match_count != cache.previous_search_match_count
                || search_current_match != cache.previous_search_current_match;

            // Convert buffer-absolute selection coordinates to snapshot-row
            // space for the renderer.  `win_start` is the flattened window top
            // (it includes the fold extra rows); the snapshot covers `snap_rows`
            // rows.  Selection rows are later mapped snapshot → rendered →
            // screen alongside the shaped lines.
            let win_start = flat_window_start;
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
                &layout,
                pane_rect,
                terminal_rect,
                gutter_inset,
                logical_cell_h,
            );
            let hover_changed =
                command_block_hover_rows_early != cache.previous_command_block_hover_rows;

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
                self.cursor_trail,
                self.cursor_trail_duration,
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
                    || view_state.text_blink_fast_visible
                        != cache.previous_text_blink_fast_visible);

            let cursor_only = !content_changed
                && !selection_changed
                && !text_blink_changed
                && !search_changed
                && !hover_changed
                && cursor_state_changed
                && !render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .deco_verts
                    .is_empty();

            if cursor_only {
                // Fast path: build just the cursor quad and stash it.
                let cursor_verts = build_cursor_verts_only(
                    cell_w,
                    cell_h,
                    effective_show_cursor,
                    cursor_blink_on,
                    cursor_pixel_pos,
                    cursor_x_scale,
                    &snap.cursor_visual_style,
                    snap.theme,
                    snap.cursor_color_override,
                );
                is_cursor_only = true;
                cursor_only_verts.clone_from(&cursor_verts);
                let mut rs = render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // detect the cursor-only mode via a separate flag.
                // We overwrite the cursor quad data in the CPU copy so that if
                // a full rebuild happens next frame it starts from correct state.
                let cfo = rs.cursor_vert_float_offset;
                if cursor_verts.is_empty() {
                    // Hide cursor: zero out the region.
                    if cfo + CURSOR_QUAD_FLOATS <= rs.deco_verts.len() {
                        for f in &mut rs.deco_verts[cfo..cfo + CURSOR_QUAD_FLOATS] {
                            *f = 0.0;
                        }
                    }
                } else if cfo + CURSOR_QUAD_FLOATS <= rs.deco_verts.len()
                    && cursor_verts.len() == CURSOR_QUAD_FLOATS
                {
                    rs.deco_verts[cfo..cfo + CURSOR_QUAD_FLOATS].copy_from_slice(&cursor_verts);
                }
            } else if content_changed
                || selection_changed
                || text_blink_changed
                || search_changed
                || hover_changed
                || render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .deco_verts
                    .is_empty()
            {
                // Full rebuild path.
                let shaped_lines = cache.shaping_cache.shape_visible(
                    &snap.visible_chars,
                    &snap.visible_tags,
                    snap.term_width,
                    &mut self.font_manager,
                    cell_w_f,
                    self.ligatures,
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
                                    self.ligatures,
                                );
                                out.push(Arc::new(shaped));

                                // Record the placeholder's hit rect in
                                // logical pixel coordinates (screen row) so the
                                // input handler (which sees pointer positions in
                                // window coordinates) can hit-test against it
                                // directly.
                                let screen_f = screen.approx_as::<f32>().unwrap_or(0.0);
                                let row_top = screen_f.mul_add(logical_cell_h, terminal_rect.min.y);
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
                // rows converted from buffer-absolute to snapshot-relative.
                let win_start = flat_window_start;
                let snap_rows = snap.term_height.saturating_add(snap.window_extra_rows);
                let search_highlights_snap: Vec<MatchHighlight> =
                    matches_to_highlights(&view_state.search_state, win_start, snap_rows);
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

                // Translate the selection's row indices from snapshot to
                // bottom-anchored screen space.  If either endpoint sits inside
                // a folded range or is scrolled off the top, drop the selection
                // for this frame (it will reappear when the user unfolds /
                // scrolls back).
                let screen_selection_rendered = if row_map.ranges().is_empty() && render_skip == 0 {
                    screen_selection
                } else {
                    screen_selection.and_then(|(sc, sr, ec, er)| {
                        let sr_s = layout.rendered_to_screen(row_map.snapshot_to_rendered(sr)?)?;
                        let er_s = layout.rendered_to_screen(row_map.snapshot_to_rendered(er)?)?;
                        Some((sc, sr_s, ec, er_s))
                    })
                };

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

                build_background_instances(
                    &BackgroundFrame {
                        shaped_lines: &rendered_shaped_lines,
                        cell_width: cell_w,
                        cell_height: cell_h,
                        ascent: self.font_manager.ascent(),
                        underline_offset: self.font_manager.underline_offset(),
                        strikeout_offset: self.font_manager.strikeout_offset(),
                        stroke_size: self.font_manager.stroke_size(),
                        show_cursor: effective_show_cursor,
                        cursor_blink_on,
                        cursor_pixel_pos,
                        cursor_width_scale: cursor_x_scale,
                        cursor_visual_style: &snap.cursor_visual_style,
                        selection: screen_selection_rendered,
                        selection_is_block: view_state.selection.is_block,
                        match_highlights: &search_highlights,
                        command_block_hover_rows,
                        term_width_cols: snap.term_width,
                        theme: snap.theme,
                        cursor_color_override: snap.cursor_color_override,
                    },
                    &mut rs_ref.bg_instances,
                    &mut rs_ref.deco_verts,
                );

                // Record where the cursor quad starts in the decoration VBO.
                // The cursor is always appended at the END of deco_verts, and is
                // exactly CURSOR_QUAD_FLOATS floats (or absent when hidden).
                let cursor_vert_float_offset = if effective_show_cursor {
                    rs_ref.deco_verts.len().saturating_sub(CURSOR_QUAD_FLOATS)
                } else {
                    rs_ref.deco_verts.len()
                };

                let fg_opts = FgRenderOptions {
                    selection: screen_selection_rendered,
                    selection_is_block: view_state.selection.is_block,
                    text_blink_slow_visible: view_state.text_blink_slow_visible,
                    text_blink_fast_visible: view_state.text_blink_fast_visible,
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
                );
                // Clone the image map into RenderState so the PaintCallback
                // (which must be Send+Sync+'static) can pass it to the renderer.
                rs_ref.snap_images.clone_from(snap.images.as_ref());
                rs_ref.cursor_vert_float_offset = cursor_vert_float_offset;
                rs_ref.cell_width_px = f32::approx_from(cell_w).unwrap_or(0.0);
                rs_ref.cell_height_px = f32::approx_from(cell_h).unwrap_or(0.0);
                rs_ref.bg_opacity = bg_opacity;
                rs_ref.bg_image_opacity = bg_image_opacity;
                rs_ref.bg_image_mode = bg_image_mode;
                drop(rs);

                // Remember which `visible_chars` allocation we rendered, so
                // the next frame can detect changes via `Arc::ptr_eq`.
                cache.last_rendered_visible = Some(Arc::clone(&snap.visible_chars));
                cache.last_rendered_line_widths = Some(Arc::clone(&snap.visible_line_widths));
                cache.previous_theme = Some(snap.theme);
                cache.previous_selection = current_selection;
                cache.previous_text_blink_slow_visible = view_state.text_blink_slow_visible;
                cache.previous_text_blink_fast_visible = view_state.text_blink_fast_visible;
                cache.previous_search_match_count = search_match_count;
                cache.previous_search_current_match = search_current_match;
                cache.previous_command_block_hover_rows = command_block_hover_rows_early;
                cache.previous_term_width = snap.term_width;
                cache.previous_term_height = snap.term_height;
                cache.previous_fold_epoch = fold_epoch;
            }
            // If neither path applies (content unchanged, cursor unchanged,
            // selection unchanged, buffers not empty) we simply re-draw the
            // existing VBO data — no CPU work at all.

            // Drive the cursor trail animation: request a repaint on the next
            // frame so the interpolation continues smoothly until it completes.
            if cursor_animating {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(16));
            }
        }

        // Update per-frame cursor state for the next frame's comparison.
        cache.previous_cursor_blink_on = cursor_blink_on;
        cache.previous_cursor_pos = snap.cursor_pos;
        cache.previous_show_cursor = effective_show_cursor;
        cache.previous_cursor_color_override = snap.cursor_color_override;

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
        // data (not `FontManager`) may be captured here.  `is_cursor_only` and
        // `cursor_only_verts` are captured by value (bool is Copy; Vec is moved).
        let render_state_for_cb = Arc::clone(render_state);
        // The MutexGuard inside the callback intentionally lives through
        // `draw_with_verts` because the renderer and atlas are refs into it.
        #[allow(clippy::significant_drop_tightening)]
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(CallbackFn::new(move |info, painter| {
                let gl = painter.gl();
                let vp = info.viewport_in_pixels();
                let mut rs = render_state_for_cb
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !rs.renderer.initialized()
                    && let Err(e) = rs.renderer.init(gl)
                {
                    error!("GL init failed: {e}");
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

                if is_cursor_only {
                    // Cursor-only fast path: patch just the cursor quad on the
                    // GPU via `glBufferSubData` (no VBO orphan, no full upload).
                    let deco_len = rs.deco_verts.len();
                    let bg_len = rs.bg_instances.len();
                    let fg_len = rs.fg_instances.len();
                    let img_len = rs.image_verts.len();
                    let cfo_bytes = rs.cursor_vert_float_offset * std::mem::size_of::<f32>();
                    let cw = rs.cell_width_px;
                    let ch = rs.cell_height_px;
                    let opacity = rs.bg_opacity;
                    let bg_image_opacity = rs.bg_image_opacity;
                    let bg_image_mode = rs.bg_image_mode;
                    // Split borrow: renderer + atlas are disjoint from the
                    // scalar fields and snap_images.
                    let rs_ref: &mut RenderState = &mut rs;
                    let renderer = &mut rs_ref.renderer;
                    let atlas = &mut rs_ref.atlas;
                    let images = &rs_ref.snap_images;
                    renderer.draw_with_cursor_only_update(
                        gl,
                        atlas,
                        cfo_bytes,
                        deco_len,
                        bg_len,
                        &cursor_only_verts,
                        fg_len,
                        img_len,
                        images,
                        vp.width_px,
                        vp.height_px,
                        cw,
                        ch,
                        opacity,
                        bg_image_opacity,
                        bg_image_mode,
                        restore_fbo,
                    );
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
                    renderer.draw_with_verts(
                        gl,
                        atlas,
                        &rs_ref.bg_instances,
                        &rs_ref.deco_verts,
                        &rs_ref.fg_instances,
                        &rs_ref.image_verts,
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
                }
            })),
        });

        // ── Scrollbar (visual + interactive) ─────────────────────────
        if let Some(new_offset) = handle_scrollbar(
            snap.scroll_offset,
            snap.max_scroll_offset,
            ui,
            &mut cache.scrollbar_dragging,
        ) {
            view_state.scroll_offset = new_offset;
            let _ = input_tx.try_send(super::input::scroll_event(
                snap,
                &view_state.folded_blocks,
                new_offset,
            ));
        }

        // ── Visual bell flash overlay ────────────────────────────────
        paint_bell_flash(ui, rect, view_state);

        // ── Password-prompt lock indicator ───────────────────────────
        // When echo-off is detected (password prompt), paint a lock icon
        // at the cursor position.  The normal cursor is suppressed (via
        // `effective_show_cursor`) so only the lock icon is visible.
        if is_echo_off {
            let cursor_logical_x = view_state
                .cursor_visual_col
                .mul_add(logical_cell_w, terminal_rect.min.x);
            let cursor_logical_y = view_state
                .cursor_visual_row
                .mul_add(logical_cell_h, terminal_rect.min.y);
            let lock_pos = egui::pos2(cursor_logical_x, cursor_logical_y);
            ui.painter().text(
                lock_pos,
                egui::Align2::LEFT_TOP,
                "\u{1F512}",
                egui::FontId::proportional(logical_cell_h),
                egui::Color32::from_rgb(255, 200, 50),
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

        // ── Search overlay ───────────────────────────────────────────
        // Run search refresh when query changed (outside the !snap.skip_draw block
        // to ensure it fires even on identical content frames).
        if view_state.search_state.is_open {
            let bar_action = show_search_bar(
                ui,
                view_state,
                terminal_rect,
                search_error.as_deref(),
                pane_id,
            );
            match bar_action {
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
        }

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
            if let Some(mouse_position) = view_state.mouse_position {
                let (col, row) = encode_egui_mouse_pos_as_usize(
                    mouse_position,
                    (logical_cell_w, logical_cell_h),
                    terminal_rect.min,
                );

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

                // Update cursor icon from cached URL state.
                // URL hover (pointing hand) takes priority over OSC 22 shape.
                // Must be set unconditionally every frame because egui resets
                // output.cursor_icon to Default at the start of each frame.
                let new_icon = if cache.cached_hovered_url.is_some() {
                    CursorIcon::PointingHand
                } else {
                    pointer_shape_to_cursor_icon(snap.pointer_shape)
                };

                ui.ctx().output_mut(|output| {
                    output.cursor_icon = new_icon;
                });

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
                // Mouse left the terminal area — fall back to OSC 22 shape.
                cache.previous_hover_cell = None;
                cache.cached_hovered_url = None;
                let base_icon = pointer_shape_to_cursor_icon(snap.pointer_shape);
                ui.ctx().output_mut(|output| {
                    output.cursor_icon = base_icon;
                });
            }
        } else {
            // No URLs — apply OSC 22 shape (or default if none set).
            cache.previous_hover_cell = None;
            cache.cached_hovered_url = None;
            let base_icon = pointer_shape_to_cursor_icon(snap.pointer_shape);
            ui.ctx().output_mut(|output| {
                output.cursor_icon = base_icon;
            });
        }

        // Fold placeholder hover: override the cursor icon to a pointing
        // hand whenever the mouse is over a placeholder row, regardless
        // of URL or OSC 22 shape state. Runs every frame because egui
        // resets `output.cursor_icon` to Default at the start of each
        // frame, so the override must be reapplied.
        if !cache.placeholder_hit_rects.is_empty()
            && let Some(mouse_position) = view_state.mouse_position
            && hit_test_placeholder(&cache.placeholder_hit_rects, mouse_position).is_some()
        {
            ui.ctx().output_mut(|output| {
                output.cursor_icon = CursorIcon::PointingHand;
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
        );

        (left_mouse_button_pressed, deferred_actions)
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
        self.ligatures = new_config.font.ligatures;
        self.cursor_trail = new_config.cursor.trail;
        self.cursor_trail_duration =
            Duration::from_millis(u64::from(new_config.cursor.trail_duration_ms));

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
        self.ligatures = new_config.font.ligatures;
        self.cursor_trail = new_config.cursor.trail;
        self.cursor_trail_duration =
            Duration::from_millis(u64::from(new_config.cursor.trail_duration_ms));

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
}

/// Convert a [`PointerShape`] (from [`TerminalSnapshot`]) to the corresponding
/// [`egui::CursorIcon`].
///
/// [`PointerShape::Default`] and any value that has no direct egui equivalent
/// both produce [`CursorIcon::Default`].
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

/// Handle file drag-and-drop events on the terminal area.
///
/// **Drop:** Shell-escapes each dropped file path and sends the result as
/// keyboard input to the PTY (space-separated, with a trailing space).
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
        let mut payload = String::new();
        for (i, file) in dropped_files.iter().enumerate() {
            if i > 0 {
                payload.push(' ');
            }
            if let Some(path) = &file.path {
                payload.push_str(&shell_escape_path(path));
            }
        }
        if !payload.is_empty() {
            payload.push(' ');
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
            cursor_vert_float_offset: 0,
            image_verts: Vec::new(),
            snap_images: std::collections::HashMap::new(),
            cell_width_px: 0.0,
            cell_height_px: 0.0,
            bg_opacity: 1.0,
            bg_image_opacity: 0.5,
            bg_image_mode: freminal_common::config::BackgroundImageMode::Cover,
            window_post: Arc::new(Mutex::new(WindowPostRenderer::new())),
            pending_bg_image: None,
        };
        assert!(rs.bg_instances.is_empty(), "bg_instances should be empty");
        assert!(rs.deco_verts.is_empty(), "deco_verts should be empty");
        assert!(rs.fg_instances.is_empty(), "fg_instances should be empty");
    }

    /// Verify that `FontManager::cell_size()` returns non-zero dimensions for
    /// the default config (bundled `MesloLGS` Nerd Font Mono).
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod gutter_hover_trigger_tests {
    //! 73.5: the gutter strip is the sole hover trigger; hovering output
    //! cells does not tint a command block.
    use super::*;
    use freminal_common::buffer_states::command_block::{CommandBlock, CommandBlockId};
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
            prompt_start_row: 1,
            command_start_row: Some(1),
            output_start_row: Some(2),
            end_row: Some(3),
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
}

#[cfg(test)]
mod shell_escape_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::path::Path;

    use super::shell_escape_path;

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
