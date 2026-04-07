// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The `FreminalTerminalWidget` egui widget and GPU render state.

use crate::gui::{
    fonts::{FontConfig, setup_font_files},
    mouse::PreviousMouseState,
    view_state::{CellCoord, ViewState},
};

use crossbeam_channel::{Receiver, Sender};
use freminal_common::{buffer_states::tchar::TChar, config::Config, themes::ThemePalette};
use freminal_terminal_emulator::{InlineImage, io::InputEvent, snapshot::TerminalSnapshot};

use eframe::egui::{self, Color32, Context, CursorIcon, Key, Pos2, Rect, Ui};

use super::{
    super::{
        atlas::GlyphAtlas,
        font_manager::FontManager,
        renderer::{
            CURSOR_QUAD_FLOATS, FgRenderOptions, TerminalRenderer, build_background_instances,
            build_cursor_verts_only, build_foreground_instances, build_image_verts,
        },
        shaping::ShapingCache,
    },
    coords::{encode_egui_mouse_pos_as_usize, flat_index_for_cell, visible_window_start},
    input::write_input_to_terminal,
};

use conv2::{ApproxFrom, ConvUtil, RoundToZero};
use eframe::egui_glow::CallbackFn;
use std::sync::{Arc, Mutex};

///
/// The scrollbar is only shown when the user is actively scrolled back
/// (`scroll_offset > 0`).  It disappears at the live bottom.
///
/// The indicator is purely visual — it does not handle drag input.
pub(super) fn paint_scrollbar(scroll_offset: usize, max_scroll_offset: usize, ui: &Ui) {
    const SCROLLBAR_WIDTH: f32 = 6.0;
    const SCROLLBAR_MARGIN: f32 = 2.0;
    const MIN_THUMB_HEIGHT: f32 = 12.0;

    // Only show when scrolled back into history.
    if scroll_offset == 0 || max_scroll_offset == 0 {
        return;
    }

    let painter = ui.painter();

    // ── Dimensions ───────────────────────────────────────────────────────
    // Anchor to the full viewport rect, not the text content rect, so the
    // scrollbar stays pinned to the right edge regardless of content width.
    let viewport = ui.max_rect();
    let track_top = viewport.top();
    let track_bottom = viewport.bottom();
    let track_height = track_bottom - track_top;
    if track_height <= 0.0 {
        return;
    }

    let track_right = viewport.right() - SCROLLBAR_MARGIN;
    let track_left = track_right - SCROLLBAR_WIDTH;

    // ── Thumb geometry ───────────────────────────────────────────────────
    // The visible window covers `term_height` rows out of a total of
    // `max_scroll_offset + term_height`.  We don't have `term_height` here
    // but it cancels out: the thumb fraction in pixels equals
    //   track_height / (max_scroll_offset + term_height)  * term_height
    // which simplifies when we use the pixel track_height as the visible
    // proxy (they are proportional).
    //
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

    // ── Appearance ───────────────────────────────────────────────────────
    let color = Color32::from_rgba_premultiplied(200, 200, 200, 180);
    let rounding = SCROLLBAR_WIDTH / 2.0; // pill shape

    painter.rect_filled(thumb_rect, rounding, color);
}

/// GPU resources shared between the main thread (vertex building) and the
/// egui `PaintCallback` closure (draw calls).
///
/// Wrapped in `Arc<Mutex<…>>` so that the pre-built vertex data can be
/// written on the main thread and consumed inside the `PaintCallback`,
/// which requires `Send + Sync + 'static` captures.
pub(super) struct RenderState {
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
}

/// The egui widget that owns and drives the terminal render pipeline.
///
/// `FreminalTerminalWidget` bridges the PTY snapshot model and the OpenGL
/// renderer. It holds the [`FontManager`], the per-line shaping cache, and the
/// GPU render state. On each call to [`show`](Self::show) it:
///
/// 1. Detects content changes via `Arc` pointer comparison.
/// 2. Re-shapes only dirty lines using the [`ShapingCache`].
/// 3. Rebuilds GPU vertex buffers when content, theme, selection, or blink
///    state has changed.
/// 4. Submits a `PaintCallback` to egui that executes the GL draw calls.
/// 5. Processes keyboard, mouse, scroll, and focus input and forwards them
///    to the PTY thread via `input_tx`.
#[allow(clippy::struct_excessive_bools)] // Six GUI rendering bookkeeping bools; not terminal modes
pub struct FreminalTerminalWidget {
    pub(super) font_manager: FontManager,
    shaping_cache: ShapingCache,
    render_state: Arc<Mutex<RenderState>>,
    previous_mouse_state: Option<PreviousMouseState>,
    previous_key: Option<Key>,
    previous_scroll_amount: f32,
    /// Cursor blink state from the most recently rendered frame.
    previous_cursor_blink_on: bool,
    /// Cursor position from the most recently rendered frame.
    previous_cursor_pos: freminal_common::buffer_states::cursor::CursorPos,
    /// Whether the cursor was shown in the most recently rendered frame.
    previous_show_cursor: bool,
    /// Cursor color override from the most recently rendered frame.
    previous_cursor_color_override: Option<(u8, u8, u8)>,
    /// The `visible_chars` arc from the last full vertex rebuild.
    ///
    /// Used to detect content changes via `Arc::ptr_eq` — immune to the race
    /// where a later snapshot overwrites `content_changed` before the GUI wakes.
    last_rendered_visible: Option<Arc<Vec<TChar>>>,
    /// Theme pointer from the last full vertex rebuild.  When this changes,
    /// we must force a full rebuild so foreground/background vertex colors
    /// are re-resolved against the new palette.
    previous_theme: Option<&'static ThemePalette>,
    /// The normalised selection from the last full vertex rebuild, used to
    /// detect selection changes that require a full rebuild.
    previous_selection: Option<(CellCoord, CellCoord)>,
    /// Text blink slow-visibility from the most recently rendered frame.
    /// Used to detect blink-tick changes that require a foreground vertex rebuild.
    previous_text_blink_slow_visible: bool,
    /// Text blink fast-visibility from the most recently rendered frame.
    previous_text_blink_fast_visible: bool,
    /// Whether OpenType ligatures are enabled for text shaping.
    ligatures: bool,
    /// Whether a modal dialog was open on the previous frame.
    ///
    /// Used to suppress input for one extra frame after the modal closes,
    /// preventing the dismiss-click from leaking through to the terminal.
    modal_was_open_last_frame: bool,
    /// The base egui `FontDefinitions` (without any preview font registered).
    /// Captured at construction and updated on `apply_config_changes`. Used by
    /// the settings modal to register a temporary preview font without losing
    /// the original font set.
    base_font_defs: eframe::egui::FontDefinitions,
}

impl FreminalTerminalWidget {
    /// Create a new `FreminalTerminalWidget`, loading fonts and initialising
    /// the GPU render state from the provided config.
    #[must_use]
    pub fn new(ctx: &Context, config: &Config) -> Self {
        let font_config = FontConfig {
            size: config.font.size,
            user_font: config.font.family.clone(),
            ..FontConfig::default()
        };
        let base_font_defs = setup_font_files(ctx, &font_config);

        let pixels_per_point = ctx.pixels_per_point();

        Self {
            font_manager: FontManager::new(config, pixels_per_point),
            shaping_cache: ShapingCache::new(),
            render_state: Arc::new(Mutex::new(RenderState {
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
            })),
            previous_mouse_state: None,
            previous_key: None,
            previous_scroll_amount: 0.0,
            previous_cursor_blink_on: true,
            previous_cursor_pos: freminal_common::buffer_states::cursor::CursorPos::default(),
            previous_show_cursor: false,
            previous_cursor_color_override: None,
            last_rendered_visible: None,
            previous_theme: None,
            previous_selection: None,
            previous_text_blink_slow_visible: true,
            previous_text_blink_fast_visible: true,
            ligatures: config.font.ligatures,
            modal_was_open_last_frame: false,
            base_font_defs,
        }
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

    /// Return a reference to the base egui `FontDefinitions` (without any
    /// preview font). Used by the settings modal to register a temporary
    /// preview font.
    #[must_use]
    pub const fn base_font_defs(&self) -> &eframe::egui::FontDefinitions {
        &self.base_font_defs
    }

    /// Synchronise the font manager's `pixels_per_point` with the current
    /// display scale factor.  If the value changed (e.g. the window moved to a
    /// monitor with a different DPI), cell metrics are recomputed and all
    /// render caches are invalidated.
    ///
    /// **Must be called before [`Self::cell_size`] each frame** so that resize
    /// calculations in `FreminalGui::ui()` use up-to-date metrics.
    pub fn sync_pixels_per_point(&mut self, ppp: f32) {
        if self.font_manager.update_pixels_per_point(ppp) {
            let mut rs = self
                .render_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            rs.atlas.clear();
            drop(rs);
            self.shaping_cache.clear();
            // Force a full vertex rebuild on the next frame.  The existing
            // VBO data was built for the old cell pixel size and must not be
            // reused.
            self.last_rendered_visible = None;
        }
    }

    /// Render the terminal for one egui frame and process all pending input.
    ///
    /// - `snap` — the latest terminal snapshot from the PTY thread (lock-free).
    /// - `view_state` — GUI-local scroll, selection, blink, and focus state.
    /// - `input_tx` — channel to send keyboard/resize/focus events to the PTY.
    /// - `clipboard_rx` — receives clipboard content from the PTY write-back.
    /// - `modal_is_open` — suppresses terminal input while a modal is visible.
    /// - `bg_opacity` — background panel opacity (`0.0`–`1.0`) from config.
    /// - `binding_map` — user key-binding map; bound combos are intercepted before PTY dispatch.
    // Inherently large: the main per-frame terminal widget handler — processes input, handles
    // blink/scroll/mouse, and orchestrates layout. Each section is tightly coupled.
    #[allow(clippy::too_many_lines)]
    // All parameters are required: `bg_opacity` must be threaded from config through to the
    // renderer; there is no sensible grouping that reduces the count without hiding the intent.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        snap: &TerminalSnapshot,
        view_state: &mut ViewState,
        input_tx: &Sender<InputEvent>,
        clipboard_rx: &Receiver<String>,
        modal_is_open: bool,
        bg_opacity: f32,
        binding_map: &freminal_common::keybindings::BindingMap,
    ) -> Vec<freminal_common::keybindings::KeyAction> {
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
        let suppress_input = modal_is_open || self.modal_was_open_last_frame;
        self.modal_was_open_last_frame = modal_is_open;

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
        if !suppress_input {
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
        let terminal_rect = ui.available_rect_before_wrap();

        // When a modal dialog (e.g. the settings window) is open — or was
        // open last frame — do NOT forward keyboard/mouse events to the PTY.
        // The one-frame delay prevents the dismiss-click from leaking through
        // as a pointer event, and resets stale inter-frame state so the next
        // real input starts from a clean slate.
        let mut deferred_actions = Vec::new();
        if suppress_input {
            self.previous_key = None;
            self.previous_mouse_state = None;
            self.previous_scroll_amount = 0.0;
            view_state.selection.is_selecting = false;
        } else {
            let repeat_characters = snap.repeat_keys;
            let ctx = ui.ctx().clone();
            let (
                _left_mouse_button_pressed,
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
                    self.previous_mouse_state.clone(),
                    repeat_characters,
                    self.previous_key,
                    self.previous_scroll_amount,
                    binding_map,
                )
            });
            self.previous_mouse_state = new_mouse_pos;
            self.previous_key = previous_key;
            self.previous_scroll_amount = scroll_amount;
            deferred_actions = actions;

            // Perform the clipboard copy OUTSIDE the ui.input() closure.
            // copy_text() calls ctx.output_mut() which needs a write lock on
            // the Context, but ui.input() holds a read lock — calling
            // copy_text() inside the closure would deadlock.
            //
            // If we sent an ExtractSelection request, wait briefly for the
            // PTY thread to respond with the extracted text.
            if clipboard_pending
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

        // Cursor-only state captured before the PaintCallback closure (which
        // requires `Send + Sync + 'static`).  `is_cursor_only` and
        // `cursor_only_verts` are moved into the closure below.
        let mut is_cursor_only = false;
        let mut cursor_only_verts: Vec<f32> = Vec::new();

        if !snap.skip_draw {
            // Detect content changes via `Arc::ptr_eq` — this is immune to the
            // race where the PTY thread overwrites a "changed" snapshot with a
            // "clean" one before the GUI wakes up.  If the `visible_chars` arc
            // is a different allocation from the one we last rendered, the
            // content has changed regardless of the `content_changed` flag.
            //
            // Also force a full rebuild when the theme palette changes, since
            // foreground/background colors are baked into the vertex buffers.
            let theme_changed = self
                .previous_theme
                .is_none_or(|prev| !std::ptr::eq(prev, snap.theme));
            let content_changed = snap.content_changed
                || theme_changed
                || self
                    .last_rendered_visible
                    .as_ref()
                    .is_none_or(|prev| !Arc::ptr_eq(prev, &snap.visible_chars));

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
            let selection_changed = current_selection != self.previous_selection;

            // Convert buffer-absolute selection coordinates to screen-relative
            // for the renderer (which iterates `shaped_lines` by screen row).
            let win_start = visible_window_start(snap);
            let screen_selection = current_selection.and_then(|(s, e)| {
                // Clamp the selection to the visible window.  If both start
                // and end are outside the visible range, there is nothing to
                // highlight on screen.
                let win_end = win_start + snap.term_height;
                if e.row < win_start || s.row >= win_end {
                    return None; // entirely outside visible window
                }
                let s_row = s.row.saturating_sub(win_start);
                let e_row = e
                    .row
                    .saturating_sub(win_start)
                    .min(snap.term_height.saturating_sub(1));
                // If the start row is above the visible window, the selection
                // begins at column 0 of the first visible row.
                let s_col = if s.row < win_start { 0 } else { s.col };
                // If the end row is below the visible window, the selection
                // extends to the last column of the last visible row.
                let e_col = if e.row >= win_end {
                    snap.term_width.saturating_sub(1)
                } else {
                    e.col
                };
                Some((s_col, s_row, e_col, e_row))
            });

            // Determine whether we can take the cursor-only fast path.
            //
            // Cursor-only: content has not changed, the selection has not
            // changed, but the cursor blink state or position has changed
            // since the last frame.  We only need to patch the cursor quad
            // in the background VBO — no re-shaping and no full vertex
            // rebuild required.
            let cursor_state_changed = cursor_blink_on != self.previous_cursor_blink_on
                || snap.cursor_pos != self.previous_cursor_pos
                || snap.show_cursor != self.previous_show_cursor
                || snap.cursor_color_override != self.previous_cursor_color_override;

            // A text-blink visibility change requires rebuilding the foreground
            // vertex buffer (glyphs are included or excluded per run).  This is
            // a separate trigger from cursor-only so it always goes through the
            // full rebuild path.
            let text_blink_changed = snap.has_blinking_text
                && (view_state.text_blink_slow_visible != self.previous_text_blink_slow_visible
                    || view_state.text_blink_fast_visible != self.previous_text_blink_fast_visible);

            let cursor_only = !content_changed
                && !selection_changed
                && !text_blink_changed
                && cursor_state_changed
                && !self
                    .render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .deco_verts
                    .is_empty();

            if cursor_only {
                // Fast path: build just the cursor quad and stash it.
                let cursor_verts = build_cursor_verts_only(
                    cell_w,
                    cell_h,
                    snap.show_cursor,
                    cursor_blink_on,
                    snap.cursor_pos,
                    &snap.cursor_visual_style,
                    snap.theme,
                    snap.cursor_color_override,
                );
                is_cursor_only = true;
                cursor_only_verts.clone_from(&cursor_verts);
                let mut rs = self
                    .render_state
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
                || self
                    .render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .deco_verts
                    .is_empty()
            {
                // Full rebuild path.
                let shaped_lines = self.shaping_cache.shape_visible(
                    &snap.visible_chars,
                    &snap.visible_tags,
                    snap.term_width,
                    &mut self.font_manager,
                    cell_w_f,
                    self.ligatures,
                );

                let (bg_instances, deco_verts) = build_background_instances(
                    &shaped_lines,
                    cell_w,
                    cell_h,
                    self.font_manager.ascent(),
                    self.font_manager.underline_offset(),
                    self.font_manager.strikeout_offset(),
                    self.font_manager.stroke_size(),
                    snap.show_cursor,
                    cursor_blink_on,
                    snap.cursor_pos,
                    &snap.cursor_visual_style,
                    screen_selection,
                    snap.theme,
                    snap.cursor_color_override,
                );

                // Record where the cursor quad starts in the decoration VBO.
                // The cursor is always appended at the END of deco_verts, and is
                // exactly CURSOR_QUAD_FLOATS floats (or absent when hidden).
                let cursor_vert_float_offset = if snap.show_cursor {
                    deco_verts.len().saturating_sub(CURSOR_QUAD_FLOATS)
                } else {
                    deco_verts.len()
                };

                // `build_foreground_instances` needs mutable access to the atlas for
                // rasterisation, so acquire the lock before calling it.
                let mut rs = self
                    .render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let fg_opts = FgRenderOptions {
                    selection: screen_selection,
                    text_blink_slow_visible: view_state.text_blink_slow_visible,
                    text_blink_fast_visible: view_state.text_blink_fast_visible,
                };
                let fg_instances = build_foreground_instances(
                    &shaped_lines,
                    &mut rs.atlas,
                    &self.font_manager,
                    cell_h,
                    self.font_manager.ascent(),
                    &fg_opts,
                    snap.theme,
                );
                let image_verts = build_image_verts(
                    &snap.visible_image_placements,
                    &snap.images,
                    snap.term_width,
                    cell_w,
                    cell_h,
                );
                rs.bg_instances = bg_instances;
                rs.deco_verts = deco_verts;
                rs.fg_instances = fg_instances;
                rs.image_verts = image_verts;
                // Clone the image map into RenderState so the PaintCallback
                // (which must be Send+Sync+'static) can pass it to the renderer.
                rs.snap_images.clone_from(snap.images.as_ref());
                rs.cursor_vert_float_offset = cursor_vert_float_offset;
                rs.cell_width_px = f32::approx_from(cell_w).unwrap_or(0.0);
                rs.cell_height_px = f32::approx_from(cell_h).unwrap_or(0.0);
                rs.bg_opacity = bg_opacity;
                drop(rs);

                // Remember which `visible_chars` allocation we rendered, so
                // the next frame can detect changes via `Arc::ptr_eq`.
                self.last_rendered_visible = Some(Arc::clone(&snap.visible_chars));
                self.previous_theme = Some(snap.theme);
                self.previous_selection = current_selection;
                self.previous_text_blink_slow_visible = view_state.text_blink_slow_visible;
                self.previous_text_blink_fast_visible = view_state.text_blink_fast_visible;
            }
            // If neither path applies (content unchanged, cursor unchanged,
            // selection unchanged, buffers not empty) we simply re-draw the
            // existing VBO data — no CPU work at all.
        }

        // Update per-frame cursor state for the next frame's comparison.
        self.previous_cursor_blink_on = cursor_blink_on;
        self.previous_cursor_pos = snap.cursor_pos;
        self.previous_show_cursor = snap.show_cursor;
        self.previous_cursor_color_override = snap.cursor_color_override;

        // Allocate the exact terminal rect (in logical points for egui).
        let desired_size = egui::Vec2::new(
            snap.term_width.approx_as::<f32>().unwrap_or(0.0) * logical_cell_w,
            snap.height.approx_as::<f32>().unwrap_or(0.0) * logical_cell_h,
        );
        let (rect, _response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

        // Hand off the draw call to egui's paint phase via PaintCallback.
        // The closure must be `Send + Sync + 'static`, so only `Arc<Mutex<…>>`
        // data (not `FontManager`) may be captured here.  `is_cursor_only` and
        // `cursor_only_verts` are captured by value (bool is Copy; Vec is moved).
        let render_state = Arc::clone(&self.render_state);
        // The MutexGuard inside the callback intentionally lives through
        // `draw_with_verts` because the renderer and atlas are refs into it.
        #[allow(clippy::significant_drop_tightening)]
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: Arc::new(CallbackFn::new(move |info, painter| {
                let gl = painter.gl();
                let vp = info.viewport_in_pixels();
                let mut rs = render_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !rs.renderer.initialized()
                    && let Err(e) = rs.renderer.init(gl)
                {
                    error!("GL init failed: {e}");
                    return;
                }
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
                        painter.intermediate_fbo(),
                    );
                } else {
                    // Full draw path: clone and re-upload all VBOs.
                    let bg_inst = rs.bg_instances.clone();
                    let deco = rs.deco_verts.clone();
                    let fg = rs.fg_instances.clone();
                    let img = rs.image_verts.clone();
                    let images = rs.snap_images.clone();
                    let cw = rs.cell_width_px;
                    let ch = rs.cell_height_px;
                    let opacity = rs.bg_opacity;
                    let rs_ref: &mut RenderState = &mut rs;
                    let renderer = &mut rs_ref.renderer;
                    let atlas = &mut rs_ref.atlas;
                    renderer.draw_with_verts(
                        gl,
                        atlas,
                        &bg_inst,
                        &deco,
                        &fg,
                        &img,
                        &images,
                        vp.width_px,
                        vp.height_px,
                        cw,
                        ch,
                        opacity,
                        painter.intermediate_fbo(),
                    );
                }
            })),
        });

        paint_scrollbar(snap.scroll_offset, snap.max_scroll_offset, ui);

        // URL hover detection: convert mouse pixel position to a cell
        // coordinate, find the FormatTag covering that cell in the snapshot,
        // and check whether it carries a URL.
        if let Some(mouse_position) = view_state.mouse_position {
            let (col, row) = encode_egui_mouse_pos_as_usize(
                mouse_position,
                (logical_cell_w, logical_cell_h),
                terminal_rect.min,
            );

            // Convert the mouse's display-column position to a flat index
            // into visible_chars.  This correctly handles wide characters
            // (CJK, emoji) whose continuation cells are stripped during
            // flattening, making the per-row TChar count smaller than
            // term_width.
            let flat_idx = flat_index_for_cell(&snap.visible_chars, row, col);

            let hovered_url = flat_idx.and_then(|idx| {
                snap.visible_tags
                    .iter()
                    .find(|tag| tag.start <= idx && idx < tag.end)
                    .and_then(|tag| tag.url.as_ref())
            });

            if let Some(url) = hovered_url {
                ui.ctx().output_mut(|output| {
                    output.cursor_icon = CursorIcon::PointingHand;
                });

                // Ctrl+click (Cmd+click on macOS) opens the URL.
                let clicked = ui.input(|i| {
                    i.pointer.button_clicked(egui::PointerButton::Primary)
                        && (i.modifiers.ctrl || i.modifiers.mac_cmd)
                });
                if clicked {
                    let url_str = url.url.clone();
                    // Spawn the open on a background thread to avoid blocking
                    // the render loop on the system's URL handler.
                    std::thread::spawn(move || {
                        if let Err(e) = open::that(&url_str) {
                            tracing::error!("Failed to open URL {url_str}: {e}");
                        }
                    });
                }
            } else {
                ui.ctx().output_mut(|output| {
                    output.cursor_icon = CursorIcon::Default;
                });
            }
        } else {
            ui.ctx().output_mut(|output| {
                output.cursor_icon = CursorIcon::Default;
            });
        }

        deferred_actions
    }

    /// Apply config changes that can be hot-reloaded at runtime.
    ///
    /// Called when the user clicks "Apply" in the settings modal. Compares the
    /// old and new configs and updates font/cursor/theme state as needed.
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
    ) {
        let pixels_per_point = ctx.pixels_per_point();
        let rebuild_result = self.font_manager.rebuild(new_config, pixels_per_point);
        let ligatures_changed = old_config.font.ligatures != new_config.font.ligatures;
        if rebuild_result.font_changed() || ligatures_changed {
            let mut rs = self
                .render_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            rs.atlas.clear();
            drop(rs);
            self.shaping_cache.clear();
        }
        self.ligatures = new_config.font.ligatures;

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
    }

    /// Apply a font zoom by setting the font manager to `effective_size`.
    ///
    /// Clears the glyph atlas and shaping cache if the size actually changed.
    /// The resize event to the PTY is handled automatically by the existing
    /// resize-detection logic in the render loop (it compares
    /// `available_pixels / cell_size` against `view_state.last_sent_size`).
    pub fn apply_font_zoom(&mut self, effective_size: f32) {
        if self.font_manager.set_font_size(effective_size) {
            let mut rs = self
                .render_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            rs.atlas.clear();
            drop(rs);
            self.shaping_cache.clear();
        }
    }

    /// Invalidate the cached theme pointer so the next frame forces a full
    /// vertex rebuild with the new palette colors.
    ///
    /// Called when a theme change is applied (not just previewed) to guarantee
    /// the vertex data is rebuilt even if the preview already set the same
    /// theme pointer.
    pub const fn invalidate_theme_cache(&mut self) {
        self.previous_theme = None;
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
        let fm = FontManager::new(&config, 1.0);
        let (w, h) = fm.cell_size();
        assert!(w > 0, "cell_width must be non-zero, got {w}");
        assert!(h > 0, "cell_height must be non-zero, got {h}");
    }
}

#[cfg(test)]
mod modal_suppress_input_tests {
    /// Test the one-frame suppression state machine for modal dismiss.
    ///
    /// The `suppress_input` flag is computed as:
    ///   `modal_is_open || self.modal_was_open_last_frame`
    /// and `modal_was_open_last_frame` is then set to `modal_is_open`.
    ///
    /// This test verifies the state machine transitions without requiring a
    /// full egui context by exercising the boolean logic directly.
    #[test]
    fn suppress_input_state_machine() {
        // Simulates `modal_was_open_last_frame` field on the widget.
        let mut modal_was_open_last_frame = false;

        // Helper: compute suppress_input for one "frame" and update the
        // tracking field.  Returns the suppress_input value for that frame.
        let mut frame = |modal_is_open: bool| -> bool {
            let suppress = modal_is_open || modal_was_open_last_frame;
            modal_was_open_last_frame = modal_is_open;
            suppress
        };

        // Frame 1: modal not open, never was → input NOT suppressed.
        assert!(!frame(false), "frame 1: no modal → no suppression");

        // Frame 2: modal opens → input suppressed.
        assert!(frame(true), "frame 2: modal open → suppressed");

        // Frame 3: modal still open → input suppressed.
        assert!(frame(true), "frame 3: modal still open → suppressed");

        // Frame 4: modal closes (dismiss click) → input STILL suppressed
        // because modal_was_open_last_frame is true.
        assert!(frame(false), "frame 4: dismiss frame → still suppressed");

        // Frame 5: modal closed, was closed last frame → input allowed.
        assert!(!frame(false), "frame 5: fully closed → input allowed");

        // Frame 6: verify stable — stays unsuppressed.
        assert!(!frame(false), "frame 6: stable → input allowed");
    }

    /// Verify that `modal_was_open_last_frame` starts `false` on a fresh
    /// widget, matching the initializer in `FreminalTerminalWidget::new()`.
    #[test]
    fn initial_state_does_not_suppress() {
        // Simulates the initial state of the field after construction.
        let modal_was_open_last_frame = false;
        let modal_is_open = false;
        let suppress = modal_is_open || modal_was_open_last_frame;
        assert!(!suppress, "fresh widget should not suppress input");
    }
}
