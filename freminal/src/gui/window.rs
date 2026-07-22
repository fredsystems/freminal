// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, Mutex, OnceLock};

use egui;
use freminal_windowing::{RepaintProxy, WindowId};

use super::{
    PaneBorderDrag, renderer::WindowPostRenderer, tabs::TabId, tabs::TabManager,
    terminal::FreminalTerminalWidget,
};

/// Pending window geometry from layout engine: `(size_px, position_px)`.
///
/// Each component is independent — either or both may be `Some`.
/// Position is typically `None` on Wayland.
pub(super) type PendingGeometry = (Option<[u32; 2]>, Option<[i32; 2]>);

/// Per-window state for a single OS window.
///
/// Each window (whether it was the first or spawned later via `Ctrl+Shift+N`)
/// owns one of these. All windows are peers — there is no root/secondary
/// distinction. Shared state (config, args, binding map, settings modal)
/// lives on [`super::FreminalGui`].
// Each bool is an independent, short-lived per-frame UI intent flag
// (pending close-pane / new-window / force-close, etc.) drained at the end of
// the frame. They are unrelated and combining them into a state machine would
// couple distinct intents and obscure meaning -- same rationale as the
// `FreminalGui` aggregator's allow.
#[allow(clippy::struct_excessive_bools)]
pub(super) struct PerWindowState {
    /// All open terminal tabs for this window.
    pub(super) tabs: TabManager,

    /// Terminal widget: owns font manager, shaping cache, glyph atlas metadata.
    /// Created lazily on the first frame when the egui context is available.
    pub(super) terminal_widget: FreminalTerminalWidget,

    /// Last title string sent to the OS window via `ViewportCommand::Title`.
    pub(super) last_window_title: String,

    /// Cached OS dark/light preference for this window.
    pub(super) os_dark_mode: bool,

    /// Cached egui style inputs — prevents redundant `global_style_mut` calls.
    ///
    /// Key tuple: `(&'static ThemePalette, background_opacity, GuiTheme)`.  A
    /// change in any element invalidates the cache and forces a full
    /// `build_visuals` rebuild.  `GuiTheme` is compared by value (it is
    /// `PartialEq` but not `Eq` because of its `f32` fields), so the comparison
    /// is done manually in `app_impl::update`.
    pub(super) style_cache: Option<(
        &'static freminal_common::themes::ThemePalette,
        f32,
        freminal_common::gui_theme::GuiTheme,
    )>,

    /// Set to `true` by the `ClosePane` key action dispatch; consumed at the
    /// end of the frame.
    pub(super) pending_close_pane: bool,

    /// Pending directional focus change; consumed at the end of the frame.
    pub(super) pending_focus_direction: Option<freminal_common::keybindings::KeyAction>,

    /// Active pane border drag state (mouse drag-to-resize).
    pub(super) border_drag: Option<PaneBorderDrag>,

    /// Last modified time of the shader file, used for hot-reload detection.
    /// `None` when no shader is configured or hot-reload is disabled.
    pub(super) shader_last_mtime: Option<std::time::SystemTime>,

    /// Per-window post-processing renderer (FBO + custom shader).
    ///
    /// Each window owns its own `WindowPostRenderer` so that pane
    /// `PaintCallback`s write into this window's FBO — not another window's.
    ///
    /// The `Arc<Mutex<…>>` wrapper is for GUI-thread interior mutability
    /// inside `PaintCallback` captures, not cross-thread synchronisation —
    /// this is only accessed on the GUI thread. See [`RenderState`]
    /// (in `gui::terminal::widget`) for the full rationale.
    pub(super) window_post: Arc<Mutex<WindowPostRenderer>>,

    /// Shared repaint handle for this window's PTY threads.
    ///
    /// Each window gets its own `Arc<OnceLock<(RepaintProxy, WindowId)>>`
    /// so PTY threads repaint the correct window.
    pub(super) repaint_handle: Arc<OnceLock<(RepaintProxy, WindowId)>>,

    /// Set to `true` by the `NewWindow` key action or menu; consumed in
    /// `update()` where `WindowHandle` is available.
    pub(super) pending_new_window: bool,

    /// If set, send resize + reposition viewport commands on the next frame.
    ///
    /// Populated by the layout engine when applying a layout to an existing
    /// window.  Consumed in `update()` via `ctx.send_viewport_cmd`.
    /// Each component is independent — either or both may be `Some`.
    pub(super) pending_geometry: Option<PendingGeometry>,

    /// Last known inner size (width, height) in physical pixels.
    ///
    /// Updated every frame from `ctx.input(|i| i.screen_rect())`.  Used by
    /// `save_layout` to persist window geometry without needing `ctx`.
    pub(super) last_known_size: Option<[u32; 2]>,

    /// Last known outer position in physical pixels.
    ///
    /// Updated every frame from `ViewportInfo::outer_rect` when available.
    /// `None` on Wayland (position is not reported) or before the first frame.
    pub(super) last_known_position: Option<[i32; 2]>,

    /// Tab currently being renamed via an inline text editor.
    ///
    /// Set by `KeyAction::RenameTab` (renames the active tab) or a
    /// double-click on a tab label.  While `Some`, the tab bar renders
    /// a `TextEdit` widget in place of the label for this tab.
    ///
    /// Cleared when the user commits (Enter) or cancels (Escape) the
    /// rename, or when the target tab is closed.
    pub(super) renaming_tab: Option<TabId>,

    /// Scratch buffer for the in-progress rename.
    ///
    /// Initialised from the target tab's current display name when
    /// `renaming_tab` is set, mutated by the `TextEdit`, and consumed on
    /// commit.  Cleared when rename ends.
    pub(super) rename_buffer: String,

    /// Index of the tab currently being dragged, if any.
    ///
    /// Set when a mouse drag starts on a tab label, cleared when the drag
    /// ends (at which point a `TabBarAction::Reorder` is emitted if the
    /// pointer was released over a different tab).
    pub(super) dragging_tab: Option<usize>,

    /// Tab rects indexed by original tab position, used to compute the
    /// drop slot during a drag.
    ///
    /// Captured at the end of each frame's `show_tab_bar`, but **frozen**
    /// for the duration of a drag: once `dragging_tab` is set, rects are
    /// NOT refreshed until the drag ends. Freezing is essential because
    /// the rendered (preview) rects shift as the ghost moves between
    /// slots — if we used those shifted rects to decide the next frame's
    /// slot, differently-sized tabs would oscillate between orderings
    /// (pointer crosses center → swap → new rect moves under pointer →
    /// swap back). Freezing rects to the natural pre-drag layout gives
    /// stable decision boundaries for the entire drag.
    pub(super) last_tab_rects: Vec<egui::Rect>,

    /// `KeyAction`s triggered from menu items (Edit, Help, etc.) that must
    /// run against the active pane's `ViewState` + PTY `input_tx`.
    ///
    /// The menu bar itself cannot dispatch these directly because it runs
    /// with `&mut PerWindowState` (no pane view-state access), so it pushes
    /// here.  Drained and dispatched at the top of the active pane's input
    /// processing each frame.
    pub(super) pending_menu_actions: Vec<freminal_common::keybindings::KeyAction>,

    /// Smart paste guard confirmation dialog for this window (Task 77).
    ///
    /// Opened by `guarded_paste` when the analyzer flags a payload, rendered
    /// every frame while open, and resolved when the user confirms or cancels.
    pub(super) paste_dialog: super::paste_guard::PasteDialog,

    /// Broadcast-input confirmation dialog for this window (Task 74).
    ///
    /// Opened by the `ToggleBroadcastInput` dispatch when
    /// `[tabs] confirm_broadcast` is set and broadcast is being turned on.
    pub(super) broadcast_dialog: super::broadcast_guard::BroadcastConfirmDialog,

    /// Close-on-running-command confirmation dialog for this window (Task 98).
    ///
    /// Opened by a guarded pane / tab / window close when the affected scope
    /// contains a running foreground command, rendered every frame while open,
    /// and resolved to Cancel or Force Close.
    pub(super) close_dialog: super::close_guard::CloseGuardDialog,

    /// Set by the `ForceClose` key action; consumed in `update()` where the
    /// close dialog is resolved.  Resolves an open close-guard dialog as
    /// "Force Close" without the user reaching for the mouse or Ctrl+Enter.
    pub(super) pending_force_close: bool,

    /// Raw key events for the egui-blocked key set (Task 114.5/114.7:
    /// keypad operators/directional, media, print/pause/menu keys), queued
    /// by `App::on_raw_key_event` at winit-event time.
    ///
    /// Encoding cannot happen inside `on_raw_key_event` itself — that
    /// callback fires outside the render/`update()` path, where the active
    /// pane, its snapshot, and the true per-pane `super_pressed` state are
    /// not in scope (and `super_pressed` is only updated during render, so
    /// encoding at event time risks a stale-super hazard for chorded keys).
    /// Instead, events are pushed here and drained once per frame on the
    /// render path — mirroring the `pending_menu_actions` /
    /// `pending_close_pane` deferred-queue precedent on this struct — at the
    /// point where the active pane's fresh `super_pressed` state is
    /// available.
    pub(super) pending_raw_keys: Vec<(
        freminal_windowing::RawKeyEvent,
        freminal_windowing::RawKeyMods,
    )>,

    /// Frame-damage report for the most recent `update()` of this window
    /// (#435), drained by `App::take_frame_damage`.
    ///
    /// Set at the end of each `update()`: [`FrameDamage::Partial`] with the
    /// cursor damage rect(s) when the frame was a pure cursor-only update
    /// (every rendered pane took the cursor-only fast path and nothing else in
    /// the window changed), otherwise [`FrameDamage::Full`]. Defaults to
    /// `Full` so a window that has not yet rendered, or any frame the
    /// aggregation does not positively prove cursor-only, presents fully.
    ///
    /// [`FrameDamage::Partial`]: freminal_windowing::FrameDamage::Partial
    /// [`FrameDamage::Full`]: freminal_windowing::FrameDamage::Full
    pub(super) pending_frame_damage: freminal_windowing::FrameDamage,

    /// Egui shapes for the "terminal band" (the pre-clear FBO callback, the
    /// per-pane render loop, the post-shader composite callback, pane border
    /// lines, and the broadcast label) painted during the most recent
    /// `update()` of this window (#436.2a), drained by
    /// `App::take_terminal_band_shapes`.
    ///
    /// Set at the end of each `update()` by cloning the shape-index range
    /// `[band_shape_start, band_shape_end)` out of
    /// `LayerId::background()`'s `PaintList` via `ctx.graphics(..)` — see the
    /// extraction comment at the `band_shapes` binding in `update()`. The
    /// band paints into the SAME background layer chrome uses (not a
    /// dedicated layer: routing it into a second `Order::Background` layer
    /// trips egui's cross-layer hit-test "hidden" rule and suppresses band
    /// widget interaction — see the capture-point comments in `update()`).
    /// This subtask (436.2a) only establishes the extraction seam: the
    /// shapes are cloned (not removed from the background layer), so they
    /// still drain into `FullOutput.shapes` and render exactly as before.
    /// Separate tessellation/painting of the band — which requires
    /// neutralizing those shapes so they are not double-painted — is 436.4.
    /// Defaults to empty before the first frame.
    pub(super) pending_terminal_band_shapes: Vec<egui::epaint::ClippedShape>,

    /// The `(active tab, active pane)` shown on the previous frame.
    ///
    /// Compared each frame to detect when the active pane changes — whether by
    /// a pane switch within a tab or by a tab switch (which changes the active
    /// pane too). On a change, the newly-active pane's cursor blink phase is
    /// re-anchored so its cursor appears immediately rather than inheriting the
    /// global blink cycle's current half. `None` before the first frame.
    pub(super) previous_active_pane_key: Option<(TabId, crate::gui::panes::PaneId)>,

    /// Authoritative partial-present flag for this window (#435).
    ///
    /// The windowing layer stores into this each frame — `true` when it
    /// skipped the full clear and is presenting only the damage region,
    /// `false` for a normal full clear + present — **before** the pane paint
    /// callbacks run. The callbacks read it (a clone is captured into each)
    /// to gate their scissor optimization, so a pane only scissors its redraw
    /// when the clear was actually skipped. Shared via `Arc` because the pane
    /// `PaintCallback` closures require `'static` captures; only ever touched
    /// on the GUI thread, so `Relaxed` ordering suffices.
    pub(super) present_is_partial: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
