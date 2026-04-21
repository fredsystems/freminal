// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, Mutex, OnceLock};

use freminal_windowing::{RepaintProxy, WindowId};

use super::{
    PaneBorderDrag, renderer::WindowPostRenderer, tabs::TabManager,
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
}
