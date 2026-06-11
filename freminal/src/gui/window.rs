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
    pub(super) style_cache: Option<(bool, &'static freminal_common::themes::ThemePalette, f32)>,

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
}
