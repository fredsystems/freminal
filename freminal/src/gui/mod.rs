// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::gui::colors::internal_color_to_egui_with_alpha;
use anyhow::Result;
use conv2::{ApproxFrom, ConvUtil, ValueFrom};
use egui::{self, CentralPanel, Panel, ViewportCommand};
use egui_glow::CallbackFn;
use freminal_common::args::Args;
use freminal_common::config::{Config, TabBarPosition, ThemeMode};
use freminal_common::pty_write::FreminalTerminalSize;
use freminal_common::terminal_size::{DEFAULT_HEIGHT, DEFAULT_WIDTH};
use freminal_terminal_emulator::io::InputEvent;
use freminal_windowing::{RepaintProxy, WindowId};
use glow::HasContext;
use renderer::WindowPostRenderer;
use settings::{SettingsAction, SettingsModal};
use tabs::{Tab, TabManager};
use terminal::{FreminalTerminalWidget, new_render_state};
use window::PerWindowState;

pub mod atlas;
pub mod colors;
pub mod font_manager;
pub mod fonts;
pub mod mouse;
pub mod panes;
pub mod pty;
pub mod renderer;
pub mod search;
pub mod settings;
pub mod shaping;
pub mod tabs;
pub mod terminal;
pub mod view_state;

mod actions;
mod hot_reload;
mod menu;
mod rendering;
pub(crate) mod window;

use tracing::{debug, error, trace};

// ── Layout helpers ────────────────────────────────────────────────────────────

/// Convert a `LayoutSplitDirection` to a `panes::SplitDirection`.
const fn layout_dir_to_pane_dir(
    dir: freminal_common::layout::LayoutSplitDirection,
) -> panes::SplitDirection {
    match dir {
        freminal_common::layout::LayoutSplitDirection::Horizontal => {
            panes::SplitDirection::Horizontal
        }
        freminal_common::layout::LayoutSplitDirection::Vertical => panes::SplitDirection::Vertical,
    }
}

/// Extract the root leaf from a `ResolvedNode` tree.
///
/// If the root is a `Leaf`, returns `(Some(leaf), None)`.
/// If the root is a `Split`, returns `(Some(first_leaf), Some(root_split))` — the
/// `first_leaf` is the leftmost/topmost leaf, suitable for constructing the initial
/// `Tab` pane, and the `root_split` is the full tree (used to build the rest).
fn extract_root_leaf(
    node: &freminal_common::layout::ResolvedNode,
) -> (
    Option<&freminal_common::layout::ResolvedLeaf>,
    Option<&freminal_common::layout::ResolvedNode>,
) {
    use freminal_common::layout::ResolvedNode;
    match node {
        ResolvedNode::Leaf(leaf) => (Some(leaf), None),
        split @ ResolvedNode::Split { first, .. } => {
            let (leaf, _) = extract_root_leaf(first);
            (leaf, Some(split))
        }
    }
}

/// Action requested by the tab bar UI.
///
/// Returned by `show_tab_bar()` and consumed by the main `ui()` method
/// after the panel finishes rendering.
#[derive(Clone, Copy)]
enum TabBarAction {
    /// No tab bar interaction this frame.
    None,
    /// User clicked the "+" button — spawn a new tab.
    NewTab,
    /// User clicked a tab label — switch to tab at `index`.
    SwitchTo(usize),
    /// User clicked the "x" close button — close tab at `index`.
    Close(usize),
}

/// Tracks an in-progress mouse drag on a pane split border.
///
/// Created when the user starts dragging a border sensor rect and
/// cleared when the drag ends. While active, mouse movement deltas
/// are converted to ratio deltas and fed to [`panes::PaneTree::resize_split`].
#[derive(Debug, Clone, Copy)]
struct PaneBorderDrag {
    /// A pane id in the first child of the split being resized.
    /// Used as `target_id` for `resize_split()`.
    target_pane: panes::PaneId,

    /// The direction of the split being resized.
    direction: panes::SplitDirection,

    /// The extent of the parent split node along the split axis,
    /// used to accurately convert pixel drag distance into a ratio delta.
    parent_extent: f32,
}

/// Initial per-window state consumed by `on_window_created()` for the first
/// window.  Subsequent windows spawn their own PTY tabs.
struct InitialWindowState {
    tab: Tab,
    window_post: Arc<Mutex<WindowPostRenderer>>,
    repaint_handle: Arc<OnceLock<(RepaintProxy, WindowId)>>,
}

struct FreminalGui {
    /// Per-window state keyed by OS window id.
    ///
    /// All windows are peers — there is no root/secondary distinction.
    windows: HashMap<WindowId, PerWindowState>,

    config: Config,

    /// CLI arguments needed for spawning new PTY tabs.
    args: Args,

    /// Settings modal state (open/close, draft config, tabs).
    settings_modal: SettingsModal,

    /// Compiled key-binding map from config. Rebuilt when the user applies
    /// new settings. Passed into the terminal widget on every frame so that
    /// bound key combos are intercepted before PTY dispatch.
    binding_map: freminal_common::keybindings::BindingMap,

    /// Monotonic generator for `PaneId` values.
    ///
    /// All panes across all tabs and all windows draw from this single generator
    /// so that pane ids are globally unique within the process lifetime.
    /// Wrapped in `Arc<Mutex<>>` so all windows can share it.
    pane_id_gen: Arc<Mutex<panes::PaneIdGenerator>>,

    /// State consumed by the first `on_window_created()` call.
    /// `None` after the initial window is created.
    initial_state: Option<InitialWindowState>,

    /// Window icon shared across all windows.
    icon: Option<egui::IconData>,

    /// Which window currently owns the settings modal, if any.
    /// `None` when the modal is closed.
    settings_owner: Option<WindowId>,

    /// The OS window used for the standalone settings dialog.
    /// `None` if no settings window is currently open.
    settings_window_id: Option<WindowId>,

    /// Set to `true` when a settings window creation has been requested
    /// but `on_window_created()` has not yet been called for it.
    pending_settings_window: bool,

    /// Set to `true` when the existing settings window should be focused.
    pending_focus_settings: bool,

    /// Optional recording handle for FREC v2 session recording.
    /// When `Some`, topology and window events are emitted.
    recording_handle: Option<freminal_terminal_emulator::recording::RecordingHandle>,

    /// Maps OS `WindowId` to recording-local u32 identifiers.
    recording_window_ids: HashMap<WindowId, u32>,

    /// Counter for assigning monotonic recording window IDs.
    next_recording_window_id: u32,

    /// Queue of resolved windows waiting to be instantiated as OS windows.
    ///
    /// Populated by `apply_layout`.  Each call to `on_window_created` (for a
    /// non-settings, non-initial window) pops one entry from this queue and
    /// uses it instead of spawning a default single-pane window.
    pending_layout_windows: std::collections::VecDeque<freminal_common::layout::ResolvedWindow>,

    /// Cached list of layouts discovered in the layout library directory.
    ///
    /// Populated at startup from `layout_library_dir()` and refreshed after
    /// `SaveLayout` writes a new file.  Used to populate the Layouts menu.
    discovered_layouts: Vec<freminal_common::layout::LayoutSummary>,

    /// A layout that has been selected from the menu and is waiting to be
    /// applied once `update()` has access to `WindowHandle`.
    ///
    /// `None` when no layout application is pending.
    pending_load_layout: Option<freminal_common::layout::ResolvedLayout>,
}

impl FreminalGui {
    #[allow(clippy::too_many_arguments)] // Constructor naturally needs all initialization params.
    fn new(
        initial_tab: Tab,
        config: Config,
        args: Args,
        repaint_handle: Arc<OnceLock<(RepaintProxy, WindowId)>>,
        config_path: Option<std::path::PathBuf>,
        window_post: Arc<Mutex<WindowPostRenderer>>,
        recording_handle: Option<freminal_terminal_emulator::recording::RecordingHandle>,
    ) -> Self {
        // Inform the initial tab about the configured theme mode and current OS
        // dark/light preference so DECRPM ?2031 responses are correct from the start.
        // OS dark mode is not yet known (no egui context). Assume light mode initially.
        let os_dark_mode = false;
        if let Err(e) = initial_tab
            .active_pane()
            .input_tx
            .send(InputEvent::ThemeModeUpdate(config.theme.mode, os_dark_mode))
        {
            error!("Failed to send initial ThemeModeUpdate to tab: {e}");
        }

        // Apply initial background image from config (if set).
        let initial_bg_path = config.ui.background_image.clone();
        if initial_bg_path.is_some()
            && let Ok(panes) = initial_tab.pane_tree.iter_panes()
        {
            for pane in panes {
                if let Ok(mut rs) = pane.render_state.lock() {
                    rs.set_pending_bg_image(initial_bg_path.clone());
                }
            }
        }
        // Push pending shader to the shared WindowPostRenderer.
        let initial_shader_src: Option<String> = config.shader.path.as_ref().and_then(|p| {
            std::fs::read_to_string(p)
                .map_err(|e| {
                    error!("Failed to read initial shader file '{}': {e}", p.display());
                })
                .ok()
        });
        if let Some(src) = initial_shader_src
            && let Ok(mut wpr) = window_post.lock()
        {
            wpr.pending_shader = Some(Some(src));
        }

        let binding_map = config.build_binding_map().unwrap_or_else(|e| {
            error!("Failed to build binding map from config: {e}. Using defaults.");
            freminal_common::keybindings::BindingMap::default()
        });

        Self {
            windows: HashMap::new(),
            binding_map,
            config,
            args,
            settings_modal: SettingsModal::new(config_path),
            pane_id_gen: Arc::new(Mutex::new(panes::PaneIdGenerator::new(1))),
            initial_state: Some(InitialWindowState {
                tab: initial_tab,
                window_post,
                repaint_handle,
            }),
            icon: None,
            settings_owner: None,
            settings_window_id: None,
            pending_settings_window: false,
            pending_focus_settings: false,
            recording_handle,
            recording_window_ids: HashMap::new(),
            next_recording_window_id: 0,
            pending_layout_windows: std::collections::VecDeque::new(),
            discovered_layouts: freminal_common::config::layout_library_dir()
                .map(|dir| freminal_common::layout::discover_layouts(&dir))
                .unwrap_or_default(),
            pending_load_layout: None,
        }
    }

    /// Get or assign a recording-local u32 ID for the given OS `WindowId`.
    fn recording_window_id(&mut self, wid: WindowId) -> u32 {
        *self.recording_window_ids.entry(wid).or_insert_with(|| {
            let id = self.next_recording_window_id;
            self.next_recording_window_id += 1;
            id
        })
    }

    /// Compute the initial PTY terminal size from pixel dimensions and cell size.
    ///
    /// Falls back to [`DEFAULT_WIDTH`]x[`DEFAULT_HEIGHT`] if the cell size is zero
    /// (font not yet measured) or the pixel dimensions are zero.
    fn compute_initial_size(
        pixel_width: u32,
        pixel_height: u32,
        cell_width: u32,
        cell_height: u32,
    ) -> FreminalTerminalSize {
        let pw = pixel_width.value_as::<usize>().unwrap_or(0);
        let ph = pixel_height.value_as::<usize>().unwrap_or(0);
        let cw = cell_width.value_as::<usize>().unwrap_or(0);
        let ch = cell_height.value_as::<usize>().unwrap_or(0);

        if cw == 0 || ch == 0 || pw == 0 || ph == 0 {
            return FreminalTerminalSize {
                width: usize::from(DEFAULT_WIDTH),
                height: usize::from(DEFAULT_HEIGHT),
                pixel_width: pw,
                pixel_height: ph,
            };
        }
        FreminalTerminalSize {
            width: (pw / cw).max(1),
            height: (ph / ch).max(1),
            pixel_width: pw,
            pixel_height: ph,
        }
    }

    /// Spawn a new PTY-backed tab and add it to the tab manager.
    ///
    /// Uses the stored `Args` and `Config` to configure the new terminal.
    /// Logs an error and does nothing if the PTY fails to start.
    fn spawn_new_tab(&self, win: &mut PerWindowState) {
        let theme =
            freminal_common::themes::by_slug(self.config.theme.active_slug(win.os_dark_mode))
                .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);

        let (cell_w, cell_h) = win.terminal_widget.cell_size();
        let cw = cell_w.value_as::<usize>().unwrap_or(0);
        let ch = cell_h.value_as::<usize>().unwrap_or(0);
        let (last_cols, last_rows) = win
            .tabs
            .active_tab()
            .active_pane()
            .view_state
            .last_sent_size;
        let initial_size = if last_cols > 0 && last_rows > 0 {
            FreminalTerminalSize {
                width: last_cols,
                height: last_rows,
                pixel_width: cw * last_cols,
                pixel_height: ch * last_rows,
            }
        } else {
            FreminalTerminalSize {
                width: usize::from(DEFAULT_WIDTH),
                height: usize::from(DEFAULT_HEIGHT),
                pixel_width: 0,
                pixel_height: 0,
            }
        };

        let pane_id = self
            .pane_id_gen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .next_id();

        let pane_cwd = win
            .tabs
            .active_tab()
            .active_pane()
            .arc_swap
            .load()
            .cwd
            .clone();
        let cwd_path = pane_cwd.as_deref().map(std::path::Path::new);

        match pty::spawn_pty_tab(
            &self.args,
            self.config.scrollback.limit,
            theme,
            &win.repaint_handle,
            initial_size,
            pty::PtyTabConfig {
                cwd: cwd_path,
                shell_override: None,
                extra_env: None,
                recording_handle: self.recording_handle.clone(),
                recording_pane_id: pane_id.raw().try_into().unwrap_or(u32::MAX),
            },
        ) {
            Ok(channels) => {
                let id = win.tabs.next_tab_id();
                let pane = panes::Pane {
                    id: pane_id,
                    arc_swap: channels.arc_swap,
                    input_tx: channels.input_tx,
                    pty_write_tx: channels.pty_write_tx,
                    window_cmd_rx: channels.window_cmd_rx,
                    clipboard_rx: channels.clipboard_rx,
                    search_buffer_rx: channels.search_buffer_rx,
                    pty_dead_rx: channels.pty_dead_rx,
                    title: "Terminal".to_owned(),
                    bell_active: false,
                    title_stack: Vec::new(),
                    view_state: view_state::ViewState::new(),
                    echo_off: channels.echo_off,
                    child_pid: channels.child_pid,
                    render_state: new_render_state(Arc::clone(&win.window_post)),
                    render_cache: terminal::PaneRenderCache::new(),
                };
                let tab = Tab::new(id, pane);
                // Inform the new tab of the current theme mode so DECRPM
                // ?2031 queries return the correct locked/dynamic status.
                if let Err(e) = tab.active_pane().input_tx.send(InputEvent::ThemeModeUpdate(
                    self.config.theme.mode,
                    win.os_dark_mode,
                )) {
                    error!("Failed to send ThemeModeUpdate to new tab: {e}");
                }
                win.tabs.add_tab(tab);
            }
            Err(e) => {
                error!("Failed to spawn new tab: {e}");
            }
        }
    }

    /// Compute the initial PTY size for a new split pane, halving along the
    /// split axis based on the active pane's current dimensions.
    fn initial_size_for_split(
        win: &PerWindowState,
        direction: panes::SplitDirection,
    ) -> FreminalTerminalSize {
        let (cell_w, cell_h) = win.terminal_widget.cell_size();
        let cw = cell_w.value_as::<usize>().unwrap_or(0);
        let ch = cell_h.value_as::<usize>().unwrap_or(0);
        let (last_cols, last_rows) = win
            .tabs
            .active_tab()
            .active_pane()
            .view_state
            .last_sent_size;
        if last_cols > 0 && last_rows > 0 {
            let cols = match direction {
                panes::SplitDirection::Horizontal => last_cols / 2,
                panes::SplitDirection::Vertical => last_cols,
            };
            let rows = match direction {
                panes::SplitDirection::Horizontal => last_rows,
                panes::SplitDirection::Vertical => last_rows / 2,
            };
            FreminalTerminalSize {
                width: cols.max(1),
                height: rows.max(1),
                pixel_width: cw * cols.max(1),
                pixel_height: ch * rows.max(1),
            }
        } else {
            FreminalTerminalSize {
                width: usize::from(DEFAULT_WIDTH),
                height: usize::from(DEFAULT_HEIGHT),
                pixel_width: 0,
                pixel_height: 0,
            }
        }
    }

    /// Spawn a new PTY-backed pane and insert it into the active tab's pane tree,
    /// splitting the currently focused pane.
    ///
    /// The focused pane becomes the `first` child of the new split; the new pane
    /// becomes the `second` child. Focus is transferred to the new pane after
    /// insertion. The split ratio starts at 0.5 (equal halves).
    ///
    /// Does nothing in playback mode (no PTY to spawn).
    // The mutex guard for `pane_id_gen` must stay alive across the `split` call
    // because `id_gen` borrows from it. Clippy cannot see through the borrow and
    // suggests an impossible inline form; suppressed here with justification.
    #[allow(clippy::significant_drop_tightening)]
    fn spawn_split_pane(&self, win: &mut PerWindowState, direction: panes::SplitDirection) {
        let theme =
            freminal_common::themes::by_slug(self.config.theme.active_slug(win.os_dark_mode))
                .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);

        let initial_size = Self::initial_size_for_split(win, direction);

        // Pre-allocate pane id so it can be threaded into recording.
        let new_pane_id = self
            .pane_id_gen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .next_id();

        // Read CWD from the focused pane so the new split inherits it.
        let pane_cwd = win
            .tabs
            .active_tab()
            .active_pane()
            .arc_swap
            .load()
            .cwd
            .clone();
        let cwd_path = pane_cwd.as_deref().map(std::path::Path::new);

        // Spawn the new PTY before touching `win.tabs` so there is no borrow conflict.
        let channels = match pty::spawn_pty_tab(
            &self.args,
            self.config.scrollback.limit,
            theme,
            &win.repaint_handle,
            initial_size,
            pty::PtyTabConfig {
                cwd: cwd_path,
                shell_override: None,
                extra_env: None,
                recording_handle: self.recording_handle.clone(),
                recording_pane_id: new_pane_id.raw().try_into().unwrap_or(u32::MAX),
            },
        ) {
            Ok(ch) => ch,
            Err(e) => {
                error!("Failed to spawn split pane: {e}");
                return;
            }
        };

        // Read the focused pane id before mutably borrowing the tab.
        let target_id = win.tabs.active_tab().active_pane;

        // Insert the new pane into the tree.
        let new_pane_id = {
            let tab = win.tabs.active_tab_mut();
            let new_pane = panes::Pane {
                id: new_pane_id,
                arc_swap: channels.arc_swap,
                input_tx: channels.input_tx,
                pty_write_tx: channels.pty_write_tx,
                window_cmd_rx: channels.window_cmd_rx,
                clipboard_rx: channels.clipboard_rx,
                search_buffer_rx: channels.search_buffer_rx,
                pty_dead_rx: channels.pty_dead_rx,
                title: "Terminal".to_owned(),
                bell_active: false,
                title_stack: Vec::new(),
                view_state: view_state::ViewState::new(),
                echo_off: channels.echo_off,
                child_pid: channels.child_pid,
                render_state: new_render_state(Arc::clone(&win.window_post)),
                render_cache: terminal::PaneRenderCache::new(),
            };
            match tab.pane_tree.split_with_id(target_id, direction, new_pane) {
                Ok(id) => id,
                Err(e) => {
                    error!("Failed to insert split pane into tree: {e}");
                    return;
                }
            }
        };
        let tab = win.tabs.active_tab_mut();

        // Transfer terminal focus from the old pane to the new one so
        // applications that track focus (DEC mode 1004) see the transition.
        if let Some(old_pane) = tab.pane_tree.find(target_id)
            && let Err(e) = old_pane.input_tx.send(InputEvent::FocusChange(false))
        {
            error!("Failed to send FocusChange(false) to previous pane {target_id}: {e}");
        }

        // Move keyboard focus to the newly created pane.
        tab.active_pane = new_pane_id;

        if let Some(new_pane) = tab.pane_tree.find(new_pane_id) {
            if let Err(e) = new_pane.input_tx.send(InputEvent::FocusChange(true)) {
                error!("Failed to send FocusChange(true) to new pane {new_pane_id}: {e}");
            }

            // Notify the new pane of the current theme mode so DECRPM ?2031
            // responses are correct from the start.
            if let Err(e) = new_pane.input_tx.send(InputEvent::ThemeModeUpdate(
                self.config.theme.mode,
                win.os_dark_mode,
            )) {
                error!("Failed to send ThemeModeUpdate to split pane: {e}");
            }

            // Propagate any active background image to the new pane.
            let new_bg_path = self.config.ui.background_image.clone();
            if new_bg_path.is_some()
                && let Ok(mut rs) = new_pane.render_state.lock()
            {
                rs.set_pending_bg_image(new_bg_path);
            }
        }
    }

    /// Spawn a new OS window with its own PTY tab.
    ///
    /// Called when the `NewWindow` key action fires or the "Window → New Window"
    /// menu is clicked.  The actual window creation is deferred to the windowing
    /// crate; `on_window_created()` will set up the `PerWindowState` when the
    /// window is ready.
    fn spawn_new_window(&self, handle: &freminal_windowing::WindowHandle<'_>) {
        handle.create_window(freminal_windowing::WindowConfig {
            title: "Freminal".to_owned(),
            inner_size: None,
            transparent: true,
            icon: self.icon.clone(),
            app_id: Some("freminal".into()),
        });
    }

    // ── Layout application (Task 61.2) ───────────────────────────────────────

    /// Spawn a PTY-backed `Pane` for a resolved layout leaf.
    ///
    /// Returns `None` and logs an error if the PTY cannot be spawned.
    #[allow(clippy::significant_drop_tightening)]
    fn spawn_pane_from_leaf(
        &self,
        leaf: &freminal_common::layout::ResolvedLeaf,
        repaint_handle: &Arc<OnceLock<(RepaintProxy, WindowId)>>,
        window_post: &Arc<Mutex<renderer::WindowPostRenderer>>,
        initial_size: freminal_common::pty_write::FreminalTerminalSize,
    ) -> Option<panes::Pane> {
        let theme = freminal_common::themes::by_slug(self.config.theme.active_slug(false))
            .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);

        let pane_id = self
            .pane_id_gen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .next_id();

        let cwd = leaf.directory.as_deref().map(std::path::Path::new);
        let shell_override = leaf.shell.as_deref();
        let extra_env = if leaf.env.is_empty() {
            None
        } else {
            Some(&leaf.env)
        };

        let channels = match pty::spawn_pty_tab(
            &self.args,
            self.config.scrollback.limit,
            theme,
            repaint_handle,
            initial_size,
            pty::PtyTabConfig {
                cwd,
                shell_override,
                extra_env,
                recording_handle: self.recording_handle.clone(),
                recording_pane_id: pane_id.raw().try_into().unwrap_or(u32::MAX),
            },
        ) {
            Ok(ch) => ch,
            Err(e) => {
                error!("layout: failed to spawn pane '{}': {e}", leaf.id);
                return None;
            }
        };

        Some(panes::Pane {
            id: pane_id,
            arc_swap: channels.arc_swap,
            input_tx: channels.input_tx,
            pty_write_tx: channels.pty_write_tx,
            window_cmd_rx: channels.window_cmd_rx,
            clipboard_rx: channels.clipboard_rx,
            search_buffer_rx: channels.search_buffer_rx,
            pty_dead_rx: channels.pty_dead_rx,
            title: leaf.title.clone().unwrap_or_else(|| "Terminal".to_owned()),
            bell_active: false,
            title_stack: Vec::new(),
            view_state: view_state::ViewState::new(),
            echo_off: channels.echo_off,
            child_pid: channels.child_pid,
            render_state: terminal::new_render_state(Arc::clone(window_post)),
            render_cache: terminal::PaneRenderCache::new(),
        })
    }

    /// Insert all panes from `node` as the `second` child of a split on
    /// `target_id`.  Returns the ID of the deepest leaf inserted.
    #[allow(clippy::too_many_arguments)]
    fn insert_subtree_as_split(
        &self,
        node: &freminal_common::layout::ResolvedNode,
        tab: &mut tabs::Tab,
        target_id: panes::PaneId,
        direction: panes::SplitDirection,
        ratio: f32,
        repaint_handle: &Arc<OnceLock<(RepaintProxy, WindowId)>>,
        window_post: &Arc<Mutex<renderer::WindowPostRenderer>>,
        initial_size: &freminal_common::pty_write::FreminalTerminalSize,
        commands: &mut Vec<(panes::PaneId, String)>,
        active_pane: &mut Option<panes::PaneId>,
    ) -> Option<panes::PaneId> {
        use freminal_common::layout::ResolvedNode;
        match node {
            ResolvedNode::Leaf(leaf) => {
                let pane = self.spawn_pane_from_leaf(
                    leaf,
                    repaint_handle,
                    window_post,
                    initial_size.clone(),
                )?;
                let id = pane.id;
                if leaf.active {
                    *active_pane = Some(id);
                }
                if let Some(ref cmd) = leaf.command {
                    commands.push((id, cmd.clone()));
                }
                if let Err(e) = pane
                    .input_tx
                    .send(InputEvent::ThemeModeUpdate(self.config.theme.mode, false))
                {
                    error!("layout: failed to send ThemeModeUpdate: {e}");
                }
                if let Err(e) = tab.pane_tree.split_with_id(target_id, direction, pane) {
                    error!("layout: failed to split pane: {e}");
                    return None;
                }
                // Adjust ratio away from the default 0.5.
                if (ratio - 0.5_f32).abs() > f32::EPSILON
                    && let Err(e) = tab.pane_tree.set_split_ratio(target_id, direction, ratio)
                {
                    debug!("layout: could not set split ratio: {e}");
                }
                Some(id)
            }
            ResolvedNode::Split {
                direction: sub_dir,
                ratio: sub_ratio,
                first,
                second,
            } => {
                // Insert first sub-node as the split, then split it further.
                let first_id = self.insert_subtree_as_split(
                    first,
                    tab,
                    target_id,
                    direction,
                    ratio,
                    repaint_handle,
                    window_post,
                    initial_size,
                    commands,
                    active_pane,
                )?;
                let sub_dir_pane = layout_dir_to_pane_dir(*sub_dir);
                self.insert_subtree_as_split(
                    second,
                    tab,
                    first_id,
                    sub_dir_pane,
                    *sub_ratio,
                    repaint_handle,
                    window_post,
                    initial_size,
                    commands,
                    active_pane,
                )
            }
        }
    }

    /// Build a tab from a `ResolvedTab`, returning the tab and a list of
    /// `(pane_id, command)` pairs for deferred command injection.
    ///
    /// Returns `None` if the tab has no panes or all pane spawns fail.
    fn build_tab_from_resolved(
        &self,
        resolved_tab: &freminal_common::layout::ResolvedTab,
        tab_id: tabs::TabId,
        repaint_handle: &Arc<OnceLock<(RepaintProxy, WindowId)>>,
        window_post: &Arc<Mutex<renderer::WindowPostRenderer>>,
        initial_size: &freminal_common::pty_write::FreminalTerminalSize,
        commands: &mut Vec<(panes::PaneId, String)>,
    ) -> Option<(tabs::Tab, Option<panes::PaneId>)> {
        let root_node = resolved_tab.root.as_ref()?;

        // Spawn root leaf or first leaf of the tree as the initial pane.
        let (root_pane, root_node_rest) = extract_root_leaf(root_node);
        let root_leaf = root_pane?;
        let root_spawned = self.spawn_pane_from_leaf(
            root_leaf,
            repaint_handle,
            window_post,
            initial_size.clone(),
        )?;

        let root_id = root_spawned.id;
        let mut active_pane: Option<panes::PaneId> = if root_leaf.active {
            Some(root_id)
        } else {
            None
        };
        if let Some(ref cmd) = root_leaf.command {
            commands.push((root_id, cmd.clone()));
        }
        if let Err(e) = root_spawned
            .input_tx
            .send(InputEvent::ThemeModeUpdate(self.config.theme.mode, false))
        {
            error!("layout: failed to send ThemeModeUpdate: {e}");
        }

        // Build the tab with the root pane.
        let mut tab = tabs::Tab::new(tab_id, root_spawned);
        if let Some(title) = resolved_tab.title.as_deref() {
            // Title will be overridden by PTY title changes but set it now.
            if let Ok(panes_mut) = tab.pane_tree.iter_panes_mut() {
                for p in panes_mut {
                    title.clone_into(&mut p.title);
                }
            }
        }

        // If there's a rest subtree (Split), insert it.
        if let Some(rest) = root_node_rest {
            use freminal_common::layout::ResolvedNode;
            if let ResolvedNode::Split {
                direction,
                ratio,
                second,
                ..
            } = rest
            {
                let dir = layout_dir_to_pane_dir(*direction);
                self.insert_subtree_as_split(
                    second,
                    &mut tab,
                    root_id,
                    dir,
                    *ratio,
                    repaint_handle,
                    window_post,
                    initial_size,
                    commands,
                    &mut active_pane,
                );
            }
        }

        Some((tab, active_pane))
    }

    /// Inject startup commands into panes after layout application.
    ///
    /// Each `(pane_id, command)` pair was collected during layout application;
    /// the command is sent to the pane's PTY immediately followed by a newline.
    /// The shell receives the text as if the user typed it.
    fn inject_layout_commands(&self, commands: &[(panes::PaneId, String)]) {
        if commands.is_empty() {
            return;
        }
        // Build a flat map of pane_id → pty_write_tx across all windows.
        for (pane_id, command) in commands {
            let found = self.windows.values().find_map(|win| {
                win.tabs.iter().find_map(|tab| {
                    tab.pane_tree.iter_panes().ok().and_then(|panes| {
                        panes
                            .into_iter()
                            .find(|p| p.id == *pane_id)
                            .map(|p| p.pty_write_tx.clone())
                    })
                })
            });
            if let Some(tx) = found {
                let mut payload = command.as_bytes().to_owned();
                payload.push(b'\n');
                if let Err(e) = tx.send(freminal_common::pty_write::PtyWrite::Write(payload)) {
                    error!(
                        "layout: failed to inject command into pane {:?}: {e}",
                        pane_id
                    );
                }
            } else {
                debug!("layout: pane {:?} not found for command injection", pane_id);
            }
        }
    }

    /// Read the current working directory of the shell in the given pane.
    ///
    /// On Linux this resolves `/proc/<pid>/cwd`.  Returns `None` on non-Linux
    /// platforms or when the child PID is unknown.
    fn read_cwd_for_pane(&self, pane_id: panes::PaneId) -> Option<String> {
        // Find the pane across all windows and tabs.
        let child_pid = self.windows.values().find_map(|win| {
            win.tabs.iter().find_map(|tab| {
                tab.pane_tree.iter_panes().ok().and_then(|ps| {
                    ps.into_iter()
                        .find(|p| p.id == pane_id)
                        .and_then(|p| p.child_pid)
                })
            })
        })?;

        #[cfg(target_os = "linux")]
        {
            let link = format!("/proc/{child_pid}/cwd");
            std::fs::read_link(&link)
                .ok()
                .and_then(|p| p.into_os_string().into_string().ok())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = child_pid;
            None
        }
    }

    /// Serialise the current window/tab/pane topology as a [`freminal_common::layout::Layout`]
    /// and write it to `path` in TOML format.
    ///
    /// # Errors
    ///
    /// Returns an error if the layout cannot be serialised or the file cannot be written.
    pub fn save_layout(&self, path: &std::path::Path) -> anyhow::Result<()> {
        use freminal_common::layout::{Layout, LayoutMeta, LayoutTab, LayoutWindow};

        let mut windows: Vec<LayoutWindow> = Vec::new();

        for win in self.windows.values() {
            let mut tabs: Vec<LayoutTab> = Vec::new();

            for (tab_idx, tab) in win.tabs.iter().enumerate() {
                let panes = tab
                    .pane_tree
                    .to_layout_panes(|pane_id| self.read_cwd_for_pane(pane_id));
                tabs.push(LayoutTab {
                    title: None,
                    active: tab_idx == 0,
                    panes,
                });
            }

            windows.push(LayoutWindow {
                size: None,
                position: None,
                monitor: None,
                tabs,
            });
        }

        let name = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned);

        let layout = Layout {
            layout: LayoutMeta {
                name,
                description: None,
                variables: std::collections::HashMap::new(),
            },
            windows,
            tabs: Vec::new(),
        };

        let toml_str = layout.to_toml_string()?;
        std::fs::write(path, toml_str)?;
        Ok(())
    }

    /// Apply a resolved layout to the current frontmost window and spawn any
    /// additional windows.
    ///
    /// - The first window in the layout is applied to `window_id` (replacing
    ///   existing tabs).
    /// - Additional windows are queued in `pending_layout_windows` and created
    ///   via deferred `handle.create_window()` calls.
    ///
    /// Returns a list of `(pane_id, command)` pairs for the caller to inject
    /// after shell startup.
    pub fn apply_layout(
        &mut self,
        resolved: &freminal_common::layout::ResolvedLayout,
        window_id: WindowId,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) -> Vec<(panes::PaneId, String)> {
        let mut all_commands: Vec<(panes::PaneId, String)> = Vec::new();

        let mut windows = resolved.windows.iter();

        // Apply first window to current window.
        // Extract the Arc handles before any &self method calls to avoid borrow conflicts.
        if let Some(first_window) = windows.next()
            && let Some(win) = self.windows.get(&window_id)
        {
            let repaint_handle = Arc::clone(&win.repaint_handle);
            let window_post = Arc::clone(&win.window_post);
            // `win` borrow ends here; we can now call &self methods.

            let initial_size = freminal_common::pty_write::FreminalTerminalSize {
                width: usize::from(DEFAULT_WIDTH),
                height: usize::from(DEFAULT_HEIGHT),
                pixel_width: 0,
                pixel_height: 0,
            };

            let (new_tabs_opt, cmds) = self.build_tabs_for_window(
                first_window,
                &repaint_handle,
                &window_post,
                &initial_size,
            );
            all_commands.extend(cmds);

            if let Some(new_tabs) = new_tabs_opt
                && let Some(win) = self.windows.get_mut(&window_id)
            {
                win.tabs = new_tabs;
            }
        }

        // Queue remaining windows for creation.
        for extra_window in windows {
            self.pending_layout_windows.push_back(extra_window.clone());
            handle.create_window(freminal_windowing::WindowConfig {
                title: "Freminal".to_owned(),
                inner_size: extra_window.size.map(<[u32; 2]>::into),
                transparent: true,
                icon: self.icon.clone(),
                app_id: Some("freminal".into()),
            });
        }

        all_commands
    }

    /// Build a `TabManager` from all tabs in a `ResolvedWindow`.
    ///
    /// Returns `(Some(TabManager), commands)` on success, `(None, commands)` if no
    /// tabs could be built.
    fn build_tabs_for_window(
        &self,
        resolved_window: &freminal_common::layout::ResolvedWindow,
        repaint_handle: &Arc<OnceLock<(RepaintProxy, WindowId)>>,
        window_post: &Arc<Mutex<renderer::WindowPostRenderer>>,
        initial_size: &freminal_common::pty_write::FreminalTerminalSize,
    ) -> (Option<tabs::TabManager>, Vec<(panes::PaneId, String)>) {
        let mut commands: Vec<(panes::PaneId, String)> = Vec::new();

        if resolved_window.tabs.is_empty() {
            return (None, commands);
        }

        let mut built_tabs: Vec<tabs::Tab> = Vec::new();
        let mut active_tab_idx: Option<usize> = None;

        for (i, resolved_tab) in resolved_window.tabs.iter().enumerate() {
            let tab_id = if i == 0 {
                tabs::TabId::first()
            } else {
                tabs::TabId::offset(u64::try_from(i).unwrap_or(u64::MAX))
            };
            if let Some((tab, _active_pane)) = self.build_tab_from_resolved(
                resolved_tab,
                tab_id,
                repaint_handle,
                window_post,
                initial_size,
                &mut commands,
            ) {
                if resolved_tab.active || active_tab_idx.is_none() {
                    active_tab_idx = Some(built_tabs.len());
                }
                built_tabs.push(tab);
            }
        }

        if built_tabs.is_empty() {
            return (None, commands);
        }

        let first = built_tabs.remove(0);
        let mut tab_mgr = tabs::TabManager::new(first);
        for extra in built_tabs {
            tab_mgr.add_tab(extra);
        }
        if let Some(idx) = active_tab_idx
            && let Err(e) = tab_mgr.switch_to(idx)
        {
            debug!("layout: could not switch to active tab {idx}: {e}");
        }

        (Some(tab_mgr), commands)
    }

    /// Consume the next pending layout window and build a `PerWindowState` for it.
    ///
    /// Called from `on_window_created` when `pending_layout_windows` is non-empty.
    fn build_window_from_pending_layout(
        &mut self,
        window_id: WindowId,
        ctx: &egui::Context,
        handle: &freminal_windowing::WindowHandle<'_>,
        inner_size: (u32, u32),
        os_dark_mode: bool,
    ) -> Option<Vec<(panes::PaneId, String)>> {
        let resolved_window = self.pending_layout_windows.pop_front()?;

        let theme = freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
            .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
        rendering::set_egui_options(ctx, theme, self.config.ui.background_opacity);

        let repaint_handle = Arc::new(OnceLock::new());
        let proxy = handle.event_loop_proxy();
        let _ = repaint_handle.set((proxy, window_id));
        let window_post = Arc::new(Mutex::new(renderer::WindowPostRenderer::new()));

        let terminal_widget = terminal::FreminalTerminalWidget::new(ctx, &self.config);
        let (cell_w, cell_h) = terminal_widget.cell_size();
        let initial_size = Self::compute_initial_size(inner_size.0, inner_size.1, cell_w, cell_h);

        let (tab_mgr_opt, commands) = self.build_tabs_for_window(
            &resolved_window,
            &repaint_handle,
            &window_post,
            &initial_size,
        );
        let tab_mgr = tab_mgr_opt?;

        let win = window::PerWindowState {
            tabs: tab_mgr,
            terminal_widget,
            last_window_title: String::from("Freminal"),
            os_dark_mode,
            style_cache: None,
            pending_close_pane: false,
            pending_focus_direction: None,
            border_drag: None,
            shader_last_mtime: None,
            window_post,
            repaint_handle,
            pending_new_window: false,
        };
        self.windows.insert(window_id, win);

        // Emit WindowCreate recording event.
        let rec_wid = self.recording_window_id(window_id);
        if let Some(ref h) = self.recording_handle {
            h.emit(
                freminal_terminal_emulator::recording::EventPayload::WindowCreate {
                    window_id: rec_wid,
                    width_px: inner_size.0,
                    height_px: inner_size.1,
                    x: 0,
                    y: 0,
                },
            );
        }

        Some(commands)
    }

    /// Handle a `SettingsAction` from the standalone settings window.
    ///
    /// Unlike the inline modal path (which operates on a single `win`), this
    /// applies changes across ALL terminal windows in `self.windows`.
    #[allow(clippy::too_many_lines)]
    fn handle_settings_action(
        &mut self,
        action: &SettingsAction,
        handle: &freminal_windowing::WindowHandle<'_>,
        _settings_window_id: WindowId,
    ) {
        match action {
            SettingsAction::Applied => {
                let new_cfg = self.settings_modal.applied_config().clone();

                // Apply theme change to all windows.
                for win in self.windows.values_mut() {
                    if new_cfg.theme.active_slug(win.os_dark_mode)
                        != self.config.theme.active_slug(win.os_dark_mode)
                        && let Some(theme) = freminal_common::themes::by_slug(
                            new_cfg.theme.active_slug(win.os_dark_mode),
                        )
                    {
                        for tab in win.tabs.iter() {
                            if let Ok(panes) = tab.pane_tree.iter_panes() {
                                for pane in panes {
                                    if let Err(e) =
                                        pane.input_tx.send(InputEvent::ThemeChange(theme))
                                    {
                                        error!("Failed to send ThemeChange to PTY thread: {e}");
                                    }
                                }
                            }
                        }
                        for tab in win.tabs.iter_mut() {
                            if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                                for pane in panes {
                                    pane.render_cache.invalidate_theme_cache();
                                }
                            }
                        }
                    }
                }

                // Apply font changes to all windows.
                for win in self.windows.values_mut() {
                    let font_changed = win
                        .terminal_widget
                        .apply_config_changes_no_ctx(&self.config, &new_cfg);
                    if font_changed {
                        win.invalidate_all_pane_atlases();
                    }
                }

                self.binding_map = new_cfg.build_binding_map().unwrap_or_else(|e| {
                    error!(
                        "Failed to rebuild binding map after settings apply: {e}. Using defaults."
                    );
                    freminal_common::keybindings::BindingMap::default()
                });
                self.config = new_cfg;

                // Apply background image to all panes in all windows.
                let new_bg_path = self.config.ui.background_image.clone();
                for win in self.windows.values() {
                    for tab in win.tabs.iter() {
                        if let Ok(panes) = tab.pane_tree.iter_panes() {
                            for pane in panes {
                                if let Ok(mut rs) = pane.render_state.lock() {
                                    rs.set_pending_bg_image(new_bg_path.clone());
                                }
                            }
                        }
                    }
                }

                // Apply shader changes to all windows.
                let has_shader_path = self.config.shader.path.is_some();
                if !has_shader_path {
                    for win in self.windows.values() {
                        if let Ok(mut wpr) = win.window_post.lock() {
                            wpr.pending_shader = Some(None);
                        }
                    }
                } else if let Some(ref p) = self.config.shader.path {
                    match std::fs::read_to_string(p) {
                        Ok(src) => {
                            for win in self.windows.values() {
                                if let Ok(mut wpr) = win.window_post.lock() {
                                    wpr.pending_shader = Some(Some(src.clone()));
                                }
                            }
                        }
                        Err(e) => {
                            error!(
                                "Failed to read shader file '{}': {e}; keeping current shader",
                                p.display()
                            );
                        }
                    }
                }

                // Notify all panes of theme mode update.
                for win in self.windows.values() {
                    for tab in win.tabs.iter() {
                        if let Ok(panes) = tab.pane_tree.iter_panes() {
                            for pane in panes {
                                if let Err(e) = pane.input_tx.send(InputEvent::ThemeModeUpdate(
                                    self.config.theme.mode,
                                    win.os_dark_mode,
                                )) {
                                    error!(
                                        "Failed to send ThemeModeUpdate after settings apply: {e}"
                                    );
                                }
                            }
                        }
                    }
                }

                // Request repaint on all terminal windows so changes are visible.
                for &wid in self.windows.keys() {
                    handle.request_repaint(wid);
                }
            }
            SettingsAction::PreviewOpacity(opacity) | SettingsAction::RevertOpacity(opacity) => {
                self.config.ui.background_opacity = *opacity;
                for &wid in self.windows.keys() {
                    handle.request_repaint(wid);
                }
            }
            SettingsAction::PreviewTheme(slug)
                if let Some(theme) = freminal_common::themes::by_slug(slug) =>
            {
                // Send theme preview to all panes in all windows.
                for win in self.windows.values() {
                    for tab in win.tabs.iter() {
                        if let Ok(panes) = tab.pane_tree.iter_panes() {
                            for pane in panes {
                                if let Err(e) = pane.input_tx.send(InputEvent::ThemeChange(theme)) {
                                    error!("Failed to send theme preview to PTY thread: {e}");
                                }
                            }
                        }
                    }
                }
                for &wid in self.windows.keys() {
                    handle.request_repaint(wid);
                }
            }
            SettingsAction::RevertTheme(slug, original_opacity)
                if let Some(theme) = freminal_common::themes::by_slug(slug) =>
            {
                for win in self.windows.values() {
                    for tab in win.tabs.iter() {
                        if let Ok(panes) = tab.pane_tree.iter_panes() {
                            for pane in panes {
                                if let Err(e) = pane.input_tx.send(InputEvent::ThemeChange(theme)) {
                                    error!("Failed to send theme revert to PTY thread: {e}");
                                }
                            }
                        }
                    }
                }
                self.config.ui.background_opacity = *original_opacity;
                for &wid in self.windows.keys() {
                    handle.request_repaint(wid);
                }
            }
            SettingsAction::RevertTheme(_, _)
            | SettingsAction::PreviewTheme(_)
            | SettingsAction::None => {}
        }
    }

    /// Return the path used for auto-save/restore of the last session.
    fn last_session_path() -> Option<std::path::PathBuf> {
        freminal_common::config::layout_library_dir().map(|d| d.join("last_session.toml"))
    }

    /// Save the current session to `last_session.toml` in the layout library.
    ///
    /// Called automatically when the last terminal window closes and
    /// `restore_last_session` is enabled.  Failures are logged but not fatal.
    fn auto_save_session(&self) {
        let Some(path) = Self::last_session_path() else {
            error!("auto_save_session: cannot determine layout library path");
            return;
        };
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            error!("auto_save_session: cannot create layout library dir: {e}");
            return;
        }
        match self.save_layout(&path) {
            Ok(()) => {
                tracing::info!("Session auto-saved to {}", path.display());
            }
            Err(e) => {
                error!("auto_save_session: failed: {e}");
            }
        }
    }

    /// Apply the last-session layout if `restore_last_session` is enabled and
    /// the file exists.  Called once after the first window is ready.
    ///
    /// Only called when no `--layout` CLI flag was provided.
    fn maybe_restore_last_session(
        &mut self,
        window_id: WindowId,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        if !self.config.startup.restore_last_session {
            return;
        }
        let Some(path) = Self::last_session_path() else {
            return;
        };
        if !path.exists() {
            return;
        }
        match freminal_common::layout::Layout::from_file(&path).and_then(|l| {
            l.validate()?;
            l.resolve()
        }) {
            Ok(resolved) => {
                let commands = self.apply_layout(&resolved, window_id, handle);
                self.inject_layout_commands(&commands);
            }
            Err(e) => {
                error!(
                    "restore_last_session: failed to apply {}: {e}",
                    path.display()
                );
            }
        }
    }
}

impl freminal_windowing::App for FreminalGui {
    /// Called when a window is created.
    ///
    /// For the first window, consumes `initial_state` to get the pre-spawned
    /// tab and widget.  For subsequent windows, spawns a fresh PTY tab.
    // Window creation handles two distinct paths (first window with pre-spawned state vs
    // subsequent windows with fresh PTY) that share no logic — splitting would not reduce
    // coupling and would obscure the flow.
    #[allow(clippy::too_many_lines)]
    fn on_window_created(
        &mut self,
        window_id: WindowId,
        ctx: &egui::Context,
        handle: &freminal_windowing::WindowHandle<'_>,
        inner_size: (u32, u32),
    ) {
        // ── Settings window ──────────────────────────────────────────────────
        if self.pending_settings_window {
            self.pending_settings_window = false;
            self.settings_window_id = Some(window_id);
            self.settings_owner = Some(window_id);
            // Don't create a PerWindowState — the settings window renders
            // only the settings UI via show_standalone().
            return;
        }

        let os_dark_mode = ctx.global_style().visuals.dark_mode;

        if let Some(initial) = self.initial_state.take() {
            // First window — use the pre-spawned tab and widget.
            let proxy = handle.event_loop_proxy();
            let _ = initial.repaint_handle.set((proxy, window_id));

            let initial_theme =
                freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
                    .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
            rendering::set_egui_options(ctx, initial_theme, self.config.ui.background_opacity);

            // Re-create terminal widget with real egui context for correct
            // font registration and DPI scaling.
            let terminal_widget = FreminalTerminalWidget::new(ctx, &self.config);

            // Send an immediate resize to the PTY so the shell starts at the
            // correct dimensions instead of the pre-spawn defaults (100x100).
            let (cell_w, cell_h) = terminal_widget.cell_size();
            let computed_size =
                Self::compute_initial_size(inner_size.0, inner_size.1, cell_w, cell_h);
            let cell_pixel_w = cell_w.value_as::<usize>().unwrap_or(0);
            let cell_pixel_h = cell_h.value_as::<usize>().unwrap_or(0);
            if let Ok(panes) = initial.tab.pane_tree.iter_panes() {
                for pane in panes {
                    if let Err(e) = pane.input_tx.send(InputEvent::Resize(
                        computed_size.width,
                        computed_size.height,
                        cell_pixel_w,
                        cell_pixel_h,
                    )) {
                        error!("Failed to send initial resize to pre-spawned pane: {e}");
                    }
                }
            }

            // Correct the theme for auto mode if the real OS preference differs
            // from the assumed-light default used during construction.
            if os_dark_mode
                && let Some(theme) =
                    freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
                && let Ok(panes) = initial.tab.pane_tree.iter_panes()
            {
                for pane in panes {
                    if let Err(e) = pane.input_tx.send(InputEvent::ThemeChange(theme)) {
                        error!("Failed to send corrective ThemeChange: {e}");
                    }
                    if let Err(e) = pane.input_tx.send(InputEvent::ThemeModeUpdate(
                        self.config.theme.mode,
                        os_dark_mode,
                    )) {
                        error!("Failed to send ThemeModeUpdate: {e}");
                    }
                }
            }

            let win = PerWindowState {
                tabs: TabManager::new(initial.tab),
                terminal_widget,
                last_window_title: String::from("Freminal"),
                os_dark_mode,
                style_cache: None,
                pending_close_pane: false,
                pending_focus_direction: None,
                border_drag: None,
                shader_last_mtime: None,
                window_post: initial.window_post,
                repaint_handle: initial.repaint_handle,
                pending_new_window: false,
            };
            self.windows.insert(window_id, win);

            // If --layout was given on the CLI, apply it to this first window.
            if let Some(ref layout_path) = self.args.layout.clone() {
                match freminal_common::layout::Layout::from_file(std::path::Path::new(layout_path))
                {
                    Ok(layout) => match layout.resolve() {
                        Ok(resolved) => {
                            let cmds = self.apply_layout(&resolved, window_id, handle);
                            self.inject_layout_commands(&cmds);
                        }
                        Err(e) => {
                            error!("Failed to resolve layout '{layout_path}': {e}");
                        }
                    },
                    Err(e) => {
                        error!("Failed to load layout '{layout_path}': {e}");
                    }
                }
            } else {
                // No --layout CLI flag — try to restore the last session if configured.
                self.maybe_restore_last_session(window_id, handle);
            }

            // Emit WindowCreate recording event.
            let rec_wid = self.recording_window_id(window_id);
            if let Some(ref h) = self.recording_handle {
                h.emit(
                    freminal_terminal_emulator::recording::EventPayload::WindowCreate {
                        window_id: rec_wid,
                        width_px: inner_size.0,
                        height_px: inner_size.1,
                        x: 0,
                        y: 0,
                    },
                );
            }
        } else {
            // Subsequent window — check if a layout window is waiting, otherwise
            // spawn a default single-pane PTY tab.
            if !self.pending_layout_windows.is_empty() {
                if let Some(cmds) = self.build_window_from_pending_layout(
                    window_id,
                    ctx,
                    handle,
                    inner_size,
                    os_dark_mode,
                ) {
                    self.inject_layout_commands(&cmds);
                }
                return;
            }

            // Subsequent window — spawn a new PTY tab.
            let theme =
                freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
                    .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
            rendering::set_egui_options(ctx, theme, self.config.ui.background_opacity);

            let repaint_handle = Arc::new(OnceLock::new());
            let proxy = handle.event_loop_proxy();
            let _ = repaint_handle.set((proxy, window_id));

            let window_post = Arc::new(Mutex::new(WindowPostRenderer::new()));

            let terminal_widget = FreminalTerminalWidget::new(ctx, &self.config);
            let (cell_w, cell_h) = terminal_widget.cell_size();
            let initial_size =
                Self::compute_initial_size(inner_size.0, inner_size.1, cell_w, cell_h);

            let pane_id = self
                .pane_id_gen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .next_id();

            match pty::spawn_pty_tab(
                &self.args,
                self.config.scrollback.limit,
                theme,
                &repaint_handle,
                initial_size,
                pty::PtyTabConfig {
                    cwd: None,
                    shell_override: None,
                    extra_env: None,
                    recording_handle: self.recording_handle.clone(),
                    recording_pane_id: pane_id.raw().try_into().unwrap_or(u32::MAX),
                },
            ) {
                Ok(channels) => {
                    let pane = panes::Pane {
                        id: pane_id,
                        arc_swap: channels.arc_swap,
                        input_tx: channels.input_tx,
                        pty_write_tx: channels.pty_write_tx,
                        window_cmd_rx: channels.window_cmd_rx,
                        clipboard_rx: channels.clipboard_rx,
                        search_buffer_rx: channels.search_buffer_rx,
                        pty_dead_rx: channels.pty_dead_rx,
                        title: "Terminal".to_owned(),
                        bell_active: false,
                        title_stack: Vec::new(),
                        view_state: view_state::ViewState::new(),
                        echo_off: channels.echo_off,
                        child_pid: channels.child_pid,
                        render_state: new_render_state(Arc::clone(&window_post)),
                        render_cache: terminal::PaneRenderCache::new(),
                    };
                    let tab_id = tabs::TabId::first();
                    let tab = Tab::new(tab_id, pane);

                    if let Err(e) = tab.active_pane().input_tx.send(InputEvent::ThemeModeUpdate(
                        self.config.theme.mode,
                        os_dark_mode,
                    )) {
                        error!("Failed to send ThemeModeUpdate to new window tab: {e}");
                    }

                    // Copy shader from config if present.
                    let shader_src = self
                        .config
                        .shader
                        .path
                        .as_ref()
                        .and_then(|p| std::fs::read_to_string(p).ok());
                    if let Some(src) = shader_src
                        && let Ok(mut wpr) = window_post.lock()
                    {
                        wpr.pending_shader = Some(Some(src));
                    }

                    // Copy bg image if present.
                    let bg_path = self.config.ui.background_image.clone();
                    if bg_path.is_some()
                        && let Ok(panes_list) = tab.pane_tree.iter_panes()
                    {
                        for p in panes_list {
                            if let Ok(mut rs) = p.render_state.lock() {
                                rs.set_pending_bg_image(bg_path.clone());
                            }
                        }
                    }

                    let win = PerWindowState {
                        tabs: TabManager::new(tab),
                        terminal_widget,
                        last_window_title: String::from("Freminal"),
                        os_dark_mode,
                        style_cache: None,
                        pending_close_pane: false,
                        pending_focus_direction: None,
                        border_drag: None,
                        shader_last_mtime: None,
                        window_post,
                        repaint_handle,
                        pending_new_window: false,
                    };
                    self.windows.insert(window_id, win);

                    // Emit WindowCreate recording event.
                    let rec_wid = self.recording_window_id(window_id);
                    if let Some(ref h) = self.recording_handle {
                        h.emit(
                            freminal_terminal_emulator::recording::EventPayload::WindowCreate {
                                window_id: rec_wid,
                                width_px: inner_size.0,
                                height_px: inner_size.1,
                                x: 0,
                                y: 0,
                            },
                        );
                    }
                }
                Err(e) => {
                    error!("Failed to spawn PTY for new window: {e}");
                }
            }
        }
    }

    /// Called when a window close is requested.
    ///
    /// Removes the window's state — its PTY threads will be dropped when
    /// the channels close.  Always returns `true` to allow the close.
    fn on_close_requested(&mut self, window_id: WindowId) -> bool {
        // Settings window closed (via OS close button).
        if self.settings_window_id == Some(window_id) {
            self.settings_modal.is_open = false;
            self.settings_window_id = None;
            self.settings_owner = None;
            return true;
        }
        // If this window owns the settings modal, close it.
        if self.settings_owner == Some(window_id) {
            self.settings_modal.is_open = false;
            self.settings_owner = None;
        }

        // Auto-save session before the last terminal window is removed.
        // We check *before* remove so we still have access to the window's tabs.
        let remaining_terminal_windows = self
            .windows
            .keys()
            .filter(|&&wid| Some(wid) != self.settings_window_id)
            .count();
        if remaining_terminal_windows == 1 && self.config.startup.restore_last_session {
            self.auto_save_session();
        }

        self.windows.remove(&window_id);

        // Emit WindowClose recording event (only for known windows), and clean up the mapping.
        if let Some(rec_wid) = self.recording_window_ids.remove(&window_id)
            && let Some(ref h) = self.recording_handle
        {
            h.emit(
                freminal_terminal_emulator::recording::EventPayload::WindowClose {
                    window_id: rec_wid,
                },
            );
        }

        true
    }

    /// Override the GL framebuffer clear color.
    ///
    /// When `background_opacity < 1.0` the viewport was created with
    /// `transparent = true`, so the compositor can show the desktop through.
    /// For that to work the clear color must have alpha = 0; otherwise the
    /// opaque clear overwrites the transparent framebuffer before egui
    /// paints anything.
    ///
    /// When opacity is 1.0 the clear color matches `panel_fill` (fully
    /// opaque) — there is no visible difference from the default.
    fn clear_color(&self, window_id: WindowId) -> [f32; 4] {
        // Settings window: use a neutral opaque background.
        if self.settings_window_id == Some(window_id) {
            return [0.2, 0.2, 0.2, 1.0];
        }
        if self.config.ui.background_opacity < 1.0 {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            // Fully opaque: use the terminal background color from the theme.
            let os_dark_mode = self.windows.get(&window_id).is_some_and(|w| w.os_dark_mode);
            let theme =
                freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
                    .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
            let (r, g, b) = theme.background;
            let color = egui::Color32::from_rgb(r, g, b);
            color.to_normalized_gamma_f32()
        }
    }

    // Inherently large: the main per-frame UI function handles menu bar, settings modal, window
    // manipulation drain, terminal widget layout, and resize detection — all in one pass over
    // the shared snapshot. Artificial sub-functions would not reduce the coupling.
    #[allow(clippy::too_many_lines)]
    fn update(
        &mut self,
        window_id: WindowId,
        ctx: &egui::Context,
        _gl: &glow::Context,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        trace!("Starting new frame");
        let now = std::time::Instant::now();

        // ── Settings window rendering ────────────────────────────────────────
        // If this update is for the settings window, render settings directly
        // and return — no terminal state to process.
        if self.settings_window_id == Some(window_id) {
            let os_dark = ctx.global_style().visuals.dark_mode;
            let settings_action = self.settings_modal.show_standalone(ctx, os_dark);
            self.handle_settings_action(&settings_action, handle, window_id);

            // If the modal closed (Cancel or Apply), close the OS window.
            if !self.settings_modal.is_open {
                self.settings_window_id = None;
                self.settings_owner = None;
                handle.close_window(window_id);
            }
            return;
        }

        // ── Focus or create settings window (deferred from menu/keybind) ─────
        if self.pending_focus_settings {
            self.pending_focus_settings = false;
            if let Some(sid) = self.settings_window_id {
                handle.focus_window(sid);
            }
        }
        if self.pending_settings_window && self.settings_window_id.is_none() {
            // Don't clear pending_settings_window here — cleared in on_window_created.
            handle.create_window(freminal_windowing::WindowConfig {
                title: "Freminal Settings".to_owned(),
                inner_size: Some((600, 500)),
                transparent: false,
                icon: self.icon.clone(),
                app_id: Some("freminal-settings".into()),
            });
        }

        // Remove per-window state for the duration of this frame.
        // All other windows remain in the map, so shader/bg propagation
        // to "other windows" simply iterates self.windows.
        let Some(mut win) = self.windows.remove(&window_id) else {
            return;
        };

        // ── Spawn new window ─────────────────────────────────────────────────
        if win.pending_new_window {
            win.pending_new_window = false;
            self.spawn_new_window(handle);
        }

        // ── Deferred egui font update from standalone settings window ────────
        win.terminal_widget
            .flush_egui_fonts_if_dirty(ctx, &self.config);

        // ── Detect OS dark/light preference changes ───────────────────────────
        let current_os_dark = ctx.global_style().visuals.dark_mode;
        if current_os_dark != win.os_dark_mode {
            win.os_dark_mode = current_os_dark;

            // Always propagate the updated OS preference so DECRPM ?2031
            // reflects the new dark/light state, regardless of ThemeMode.
            for tab in win.tabs.iter() {
                if let Ok(panes) = tab.pane_tree.iter_panes() {
                    for pane in panes {
                        if let Err(e) = pane.input_tx.send(InputEvent::ThemeModeUpdate(
                            self.config.theme.mode,
                            win.os_dark_mode,
                        )) {
                            error!("Failed to send ThemeModeUpdate on OS change to pane: {e}");
                        }
                    }
                }
            }

            if self.config.theme.mode == ThemeMode::Auto {
                let slug = self.config.theme.active_slug(win.os_dark_mode);
                if let Some(theme) = freminal_common::themes::by_slug(slug) {
                    // Notify every pane in every tab so all PTY threads get the new palette.
                    for tab in win.tabs.iter() {
                        if let Ok(panes) = tab.pane_tree.iter_panes() {
                            for pane in panes {
                                if let Err(e) = pane.input_tx.send(
                                    freminal_terminal_emulator::io::InputEvent::ThemeChange(theme),
                                ) {
                                    error!("Failed to send auto ThemeChange to pane: {e}");
                                }
                            }
                        }
                    }
                    rendering::update_egui_theme(ctx, theme, self.config.ui.background_opacity);
                    // Invalidate theme cache on all panes in all tabs so the
                    // next frame forces a full vertex rebuild with the new palette.
                    for tab in win.tabs.iter_mut() {
                        if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                            for pane in panes {
                                pane.render_cache.invalidate_theme_cache();
                            }
                        }
                    }
                }
            }
        }

        // ── Shader hot-reload ─────────────────────────────────────────────────
        // When hot_reload is enabled and a shader file is configured, check the
        // file's mtime each frame and push a recompile to all panes if it changed.
        if self.config.shader.hot_reload
            && let Some(ref shader_path) = self.config.shader.path.clone()
        {
            let new_mtime = std::fs::metadata(shader_path)
                .and_then(|m| m.modified())
                .ok();
            let changed = match (new_mtime, win.shader_last_mtime) {
                (Some(new), Some(prev)) => new != prev,
                (Some(_), None) => true,
                _ => false,
            };
            if changed {
                win.shader_last_mtime = new_mtime;
                match std::fs::read_to_string(shader_path) {
                    Ok(src) => {
                        if let Ok(mut wpr) = win.window_post.lock() {
                            wpr.pending_shader = Some(Some(src.clone()));
                        }
                        // Propagate to all other windows (win is removed from map).
                        for other_win in self.windows.values() {
                            if let Ok(mut wpr) = other_win.window_post.lock() {
                                wpr.pending_shader = Some(Some(src.clone()));
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            "Shader hot-reload: failed to read '{}': {e}",
                            shader_path.display()
                        );
                    }
                }
            }
        }

        // ── Poll all tabs for PTY death signals ───────────────────────────────
        // When a pane's PTY dies, close that pane.  If it was the last pane in
        // the tab, close the tab.  If it was the last tab, close the window.
        //
        // Collect (tab_index, pane_id) pairs for dead panes, then process
        // them in reverse order to avoid index shifting issues.
        let mut dead_panes: Vec<(usize, panes::PaneId)> = Vec::new();
        for (tab_idx, tab) in win.tabs.iter().enumerate() {
            if let Ok(panes) = tab.pane_tree.iter_panes() {
                for pane in panes {
                    if pane.pty_dead_rx.try_recv().is_ok() {
                        dead_panes.push((tab_idx, pane.id));
                    }
                }
            }
        }

        for (tab_idx, pane_id) in dead_panes.into_iter().rev() {
            // Try to close just the dead pane within its tab.
            let is_active_tab = tab_idx == win.tabs.active_index();

            // Switch to the dead pane's tab temporarily if needed so we can
            // operate on it.
            if !is_active_tab && let Err(e) = win.tabs.switch_to(tab_idx) {
                error!("Failed to switch to tab {tab_idx} for dead pane cleanup: {e}");
                continue;
            }

            let tab = win.tabs.active_tab_mut();
            // If the dead pane was the zoomed pane, un-zoom first.
            if tab.zoomed_pane == Some(pane_id) {
                tab.zoomed_pane = None;
            }

            match tab.pane_tree.close(pane_id) {
                Ok(_closed) => {
                    // Emit PaneClose recording event.
                    if let Some(ref h) = self.recording_handle {
                        #[allow(clippy::cast_possible_truncation)]
                        h.emit(
                            freminal_terminal_emulator::recording::EventPayload::PaneClose {
                                pane_id: pane_id.raw() as u32,
                            },
                        );
                    }

                    // Reset last_sent_size on all surviving panes so the
                    // next frame's resize check fires with the new layout.
                    let tab = win.tabs.active_tab_mut();
                    if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                        for pane in panes {
                            pane.view_state.last_sent_size = (0, 0);
                        }
                    }
                    // If the active pane was the one that died, pick a new active pane
                    // and notify it that it gained focus.
                    let tab = win.tabs.active_tab_mut();
                    if tab.active_pane == pane_id
                        && let Ok(panes) = tab.pane_tree.iter_panes()
                        && let Some(first) = panes.first()
                    {
                        let new_id = first.id;
                        if let Err(e) = first.input_tx.send(InputEvent::FocusChange(true)) {
                            error!("Failed to send FocusChange(true) to pane {new_id}: {e}");
                        }
                        tab.active_pane = new_id;
                    }
                }
                Err(panes::PaneError::CannotCloseLastPane) => {
                    // Last pane in tab — close the entire tab.
                    if win.tabs.tab_count() <= 1 {
                        // Last tab in this window — close the window.
                        self.windows.insert(window_id, win);
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                        return;
                    }
                    win.close_tab(tab_idx);
                }
                Err(e) => {
                    error!("Failed to close dead pane {pane_id}: {e}");
                }
            }

            // Restore the original active tab if we switched away.
            if !is_active_tab {
                // The tab we were on may have been removed, so saturate.
                let restore_idx = tab_idx.min(win.tabs.tab_count().saturating_sub(1));
                let _ = win.tabs.switch_to(restore_idx);
            }
        }

        // Load the latest snapshot from the PTY thread — no lock, single atomic load.
        let snap = win.tabs.active_tab().active_pane().arc_swap.load();

        // Sync the GUI's scroll offset from the snapshot.  When new PTY output
        // arrives the PTY thread resets its offset to 0, so the snapshot will
        // carry scroll_offset = 0 even if the GUI previously sent a non-zero
        // value.  Adopting the snapshot's value keeps ViewState in sync.
        if win.tabs.active_tab().active_pane().view_state.scroll_offset != snap.scroll_offset {
            win.tabs
                .active_tab_mut()
                .active_pane_mut()
                .view_state
                .scroll_offset = snap.scroll_offset;
        }

        // Create a root Ui covering the full available area.  Panels reserve
        // space from this Ui via `show_inside` (the non-deprecated API).
        let mut root_ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::new("freminal_root"),
            egui::UiBuilder::default(),
        );

        // Menu bar at the top of the window.
        let mut any_menu_open = false;
        if !self.config.ui.hide_menu_bar {
            let (menu_action, menu_open) = Panel::top("menu_bar")
                .show_inside(&mut root_ui, |ui| {
                    self.show_menu_bar(ui, &mut win, window_id)
                })
                .inner;
            any_menu_open = menu_open;
            self.dispatch_tab_bar_action(menu_action, &mut win);
        }

        // Tab bar: shown when multiple tabs are open, or when the config
        // option `tabs.show_single_tab` is enabled.
        let show_tab_bar = win.tabs.tab_count() > 1 || self.config.tabs.show_single_tab;

        if show_tab_bar {
            let panel = match self.config.tabs.position {
                TabBarPosition::Top => Panel::top("tab_bar"),
                TabBarPosition::Bottom => Panel::bottom("tab_bar"),
            };
            let tab_action = panel
                .show_inside(&mut root_ui, |ui| self.show_tab_bar(&win, ui))
                .inner;
            self.dispatch_tab_bar_action(tab_action, &mut win);
        }

        let _panel_response = CentralPanel::default().show_inside(&mut root_ui, |ui| {
            // Synchronise font metrics with the current display scale *before*
            // reading `cell_size()`.  Without this, the first frame after a DPI
            // change would use stale pixel metrics for the resize calculation.
            let ppp = ctx.pixels_per_point();
            let ppp_changed = win.terminal_widget.sync_pixels_per_point(ppp);

            // Synchronise font zoom for the active tab.  Each tab has its own
            // zoom_delta and the font manager only knows one size at a time.
            // This check fires on every frame but is a single float comparison
            // when no change is needed.
            let effective = win
                .tabs
                .active_tab()
                .active_pane()
                .view_state
                .effective_font_size(self.config.font.size);
            let zoom_changed = win.terminal_widget.apply_font_zoom(effective);

            // When pixels-per-point or font zoom changes, every pane's GL
            // atlas and cached content must be invalidated so glyphs are
            // re-rasterised at the new size.
            if ppp_changed || zoom_changed {
                win.invalidate_all_pane_atlases();
            }

            // Compute char size once — shared across all panes since all panes
            // use the same font at the same size.
            // `cell_size()` returns integer pixel dimensions (physical) from swash
            // font metrics.  egui's coordinate system uses logical points, so we
            // convert with `pixels_per_point` when doing layout math.
            let (cell_w_u, cell_height_u) = win.terminal_widget.cell_size();
            let font_width = usize::value_from(cell_w_u).unwrap_or(0);
            let font_height = usize::value_from(cell_height_u).unwrap_or(0);
            let logical_char_w = f32::approx_from(cell_w_u).unwrap_or(0.0) / ppp;
            let logical_char_h = f32::approx_from(cell_height_u).unwrap_or(0.0) / ppp;

            let window_width = ui.input(|i: &egui::InputState| i.content_rect());

            // Drain window commands for ALL tabs and ALL panes within each tab.
            // The active tab's active pane gets full handling (viewport commands,
            // reports, title updates, clipboard). All other panes get reports
            // answered, titles updated, and clipboard handled — only
            // viewport-mutating commands (resize, move, minimize, fullscreen)
            // are discarded since a non-active pane must not alter the shared
            // window geometry.
            let active_idx = win.tabs.active_index();
            let active_pane_id_for_drain = win.tabs.active_tab().active_pane;
            let window_focused = win
                .tabs
                .active_tab()
                .active_pane()
                .view_state
                .window_focused;
            for (idx, tab) in win.tabs.iter_mut().enumerate() {
                let is_active_tab = idx == active_idx;
                let is_only_pane = match tab.pane_tree.pane_count() {
                    Ok(count) => count == 1,
                    Err(e) => {
                        trace!("pane_count error (treating as split): {e}");
                        false
                    }
                };
                if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                    for pane in panes {
                        let is_fully_active = is_active_tab && pane.id == active_pane_id_for_drain;
                        rendering::handle_window_manipulation(
                            ui,
                            &pane.window_cmd_rx,
                            &pane.pty_write_tx,
                            font_width,
                            font_height,
                            window_width,
                            &mut pane.title_stack,
                            &mut pane.title,
                            &mut pane.bell_active,
                            &mut pane.view_state.bell_since,
                            self.config.bell.mode,
                            &rendering::WindowManipFlags {
                                allow_clipboard_read: self.config.security.allow_clipboard_read,
                                is_active: is_fully_active,
                                window_focused,
                                is_only_pane,
                            },
                        );
                    }
                }
            }

            // Update background color based on the active pane's display mode.
            //
            // Gated: only call `global_style_mut` when the inputs have
            // changed.  `global_style_mut` triggers `Arc::make_mut` on
            // the egui `Style`, which clones every frame unless skipped.
            let bg_opacity = self.config.ui.background_opacity;
            let style_key = (snap.is_normal_display, snap.theme, bg_opacity);
            let style_changed = match win.style_cache {
                Some((prev_display, prev_theme, prev_opacity)) => {
                    prev_display != style_key.0
                        || !std::ptr::eq(prev_theme, style_key.1)
                        || prev_opacity.to_bits() != bg_opacity.to_bits()
                }
                None => true,
            };
            if style_changed {
                if snap.is_normal_display {
                    ctx.global_style_mut(|style| {
                        // window_fill: always opaque (menus, settings, chrome).
                        style.visuals.window_fill = internal_color_to_egui_with_alpha(
                            freminal_common::colors::TerminalColor::DefaultBackground,
                            false,
                            snap.theme,
                            1.0,
                        );
                        // panel_fill: respects background_opacity (terminal area only).
                        style.visuals.panel_fill = internal_color_to_egui_with_alpha(
                            freminal_common::colors::TerminalColor::DefaultBackground,
                            false,
                            snap.theme,
                            bg_opacity,
                        );
                    });
                } else {
                    ctx.global_style_mut(|style| {
                        // window_fill: always opaque (menus, settings, chrome).
                        style.visuals.window_fill =
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 255);
                        // panel_fill: respects background_opacity (terminal area only).
                        let alpha = (bg_opacity * 255.0)
                            .round()
                            .approx_as::<u8>()
                            .unwrap_or(255);
                        style.visuals.panel_fill =
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
                    });
                }
                win.style_cache = Some(style_key);
            }

            // ── Multi-pane rendering loop ────────────────────────────
            //
            // Compute layout rects for every leaf pane in the active tab's
            // pane tree, then render each one into its allocated rect.
            // Collect deferred key actions from all panes for dispatch after
            // the loop.

            let available_rect = ui.available_rect_before_wrap();
            let active_pane_id = win.tabs.active_tab().active_pane;
            let zoomed_pane = win.tabs.active_tab().zoomed_pane;
            let has_multiple_panes = win.tabs.active_tab().pane_tree.pane_count().unwrap_or(1) > 1;

            // When a pane is zoomed, render only that pane at full size.
            // Borders are hidden during zoom since there is only one visible pane.
            let (pane_layout, border_width) = if let Some(zoomed_id) = zoomed_pane {
                (vec![(zoomed_id, available_rect)], 0.0)
            } else {
                // Width of the border drawn between adjacent panes (logical pixels).
                let bw: f32 = if has_multiple_panes { 1.0 } else { 0.0 };
                let layout = win
                    .tabs
                    .active_tab()
                    .pane_tree
                    .layout(available_rect)
                    .unwrap_or_default();
                (layout, bw)
            };

            let mut all_deferred_actions = Vec::new();

            // Track repaint needs across all panes.
            let mut shortest_repaint_delay: Option<std::time::Duration> = None;

            let ui_overlay_open = any_menu_open;

            // ── Pane border drag-to-resize ───────────────────────────
            //
            // Before rendering panes, place invisible drag sensors on each
            // split border. This must happen before the per-pane
            // `scope_builder` calls so that pointer events on the border
            // are consumed here instead of reaching the terminal widgets.
            if has_multiple_panes && zoomed_pane.is_none() && !ui_overlay_open {
                let borders = win
                    .tabs
                    .active_tab()
                    .pane_tree
                    .split_borders(available_rect, active_pane_id)
                    .unwrap_or_default();

                // Half-width of the invisible drag sensor zone (pixels
                // on each side of the 1px border line).
                let sensor_half: f32 = 3.0;

                for (border_idx, border) in borders.iter().enumerate() {
                    // Expand the thin 1px border rect into a wider sensor rect.
                    let sensor_rect = match border.direction {
                        panes::SplitDirection::Horizontal => {
                            // Vertical divider — expand horizontally.
                            let cx = border.rect.center().x;
                            egui::Rect::from_min_max(
                                egui::pos2(cx - sensor_half, border.rect.min.y),
                                egui::pos2(cx + sensor_half, border.rect.max.y),
                            )
                        }
                        panes::SplitDirection::Vertical => {
                            // Horizontal divider — expand vertically.
                            let cy = border.rect.center().y;
                            egui::Rect::from_min_max(
                                egui::pos2(border.rect.min.x, cy - sensor_half),
                                egui::pos2(border.rect.max.x, cy + sensor_half),
                            )
                        }
                    };

                    let sensor_id = ui.id().with("pane_border_sensor").with(border_idx);
                    let response =
                        ui.interact(sensor_rect, sensor_id, egui::Sense::click_and_drag());

                    // Change cursor when hovering or dragging a border.
                    if response.hovered() || response.dragged() {
                        let cursor = match border.direction {
                            panes::SplitDirection::Horizontal => egui::CursorIcon::ResizeHorizontal,
                            panes::SplitDirection::Vertical => egui::CursorIcon::ResizeVertical,
                        };
                        ctx.set_cursor_icon(cursor);
                    }

                    // On drag start, record which border we're resizing.
                    if response.drag_started() {
                        win.border_drag = Some(PaneBorderDrag {
                            target_pane: border.first_child_pane,
                            direction: border.direction,
                            parent_extent: border.parent_extent,
                        });
                    }

                    // While dragging, convert pixel delta to ratio delta.
                    if response.dragged()
                        && let Some(drag) = &win.border_drag
                    {
                        let delta_px = match drag.direction {
                            panes::SplitDirection::Horizontal => response.drag_delta().x,
                            panes::SplitDirection::Vertical => response.drag_delta().y,
                        };

                        // Convert pixel delta to ratio delta based on
                        // the dragged split parent's extent along the split axis.
                        let total_px = drag.parent_extent;

                        if total_px > 0.0 {
                            let delta_ratio = delta_px / total_px;
                            if let Err(e) = win.tabs.active_tab_mut().pane_tree.resize_split(
                                drag.target_pane,
                                drag.direction,
                                delta_ratio,
                            ) {
                                debug!("Border resize failed: {e}");
                            }
                        }
                    }

                    // Clear drag state when drag ends.
                    if response.drag_stopped() {
                        win.border_drag = None;
                    }
                }
            }

            // ── Pre-clear the window post-processing FBO ──────────
            //
            // When a user GLSL shader is active (or about to become active),
            // all panes render into a shared window FBO.  We clear it once
            // per frame here, before any pane draws into it, so stale content
            // from the previous frame does not bleed through.
            //
            // We also schedule the pre-clear when `pending_shader` is set so
            // that the very first frame after a shader is enabled already has
            // the FBO ready for pane callbacks.  The `ensure_fbo` call inside
            // the callback creates the FBO on-demand if it doesn't exist yet.
            {
                let wpr_guard = win
                    .window_post
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let wpr_active = wpr_guard.is_active();
                let shader_activation_pending = wpr_guard.pending_shader.is_some();
                drop(wpr_guard);

                if wpr_active || shader_activation_pending {
                    let wpr_for_clear = Arc::clone(&win.window_post);
                    ui.painter().add(egui::PaintCallback {
                        rect: available_rect,
                        callback: Arc::new(CallbackFn::new(move |info, painter| {
                            let gl = painter.gl();
                            let vp = info.viewport_in_pixels();
                            let mut wpr = wpr_for_clear
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            wpr.ensure_fbo(gl, vp.width_px, vp.height_px);
                            if let Some(fbo) = wpr.fbo() {
                                unsafe {
                                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                                    gl.clear_color(0.0, 0.0, 0.0, 0.0);
                                    gl.clear(glow::COLOR_BUFFER_BIT);
                                    // Restore egui's FBO.
                                    gl.bind_framebuffer(
                                        glow::FRAMEBUFFER,
                                        painter.intermediate_fbo(),
                                    );
                                }
                            }
                        })),
                    });
                }
            }

            for (pane_id, pane_rect) in &pane_layout {
                // Shrink the pane rect slightly to leave room for borders.
                // Each pane edge that is interior (shared with another pane)
                // gives up half the border width so the total gap equals
                // `border_width`.
                let content_rect = if has_multiple_panes {
                    let half = border_width / 2.0;
                    let shrink_left = if pane_rect.min.x > available_rect.min.x {
                        half
                    } else {
                        0.0
                    };
                    let shrink_right = if pane_rect.max.x < available_rect.max.x {
                        half
                    } else {
                        0.0
                    };
                    let shrink_top = if pane_rect.min.y > available_rect.min.y {
                        half
                    } else {
                        0.0
                    };
                    let shrink_bottom = if pane_rect.max.y < available_rect.max.y {
                        half
                    } else {
                        0.0
                    };
                    egui::Rect::from_min_max(
                        egui::pos2(pane_rect.min.x + shrink_left, pane_rect.min.y + shrink_top),
                        egui::pos2(
                            pane_rect.max.x - shrink_right,
                            pane_rect.max.y - shrink_bottom,
                        ),
                    )
                } else {
                    *pane_rect
                };

                // Per-pane character dimensions from this pane's content rect.
                let pane_width_chars = (content_rect.width() / logical_char_w)
                    .floor()
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to calculate pane width chars: {e}");
                        10
                    });
                let pane_height_chars = (content_rect.height() / logical_char_h)
                    .floor()
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to calculate pane height chars: {e}");
                        10
                    })
                    .max(1);

                // Look up the pane mutably for resize + render.
                let pane_id = *pane_id;
                let tab = win.tabs.active_tab_mut();
                let Some(pane) = tab.pane_tree.find_mut(pane_id) else {
                    // Should never happen — layout returned this id.
                    error!("Pane {pane_id} not found in tree during render");
                    continue;
                };

                // Debounced resize: only send when char dims changed.
                let new_size = (pane_width_chars, pane_height_chars);
                if new_size != pane.view_state.last_sent_size {
                    if let Err(e) = pane.input_tx.send(InputEvent::Resize(
                        pane_width_chars,
                        pane_height_chars,
                        font_width,
                        font_height,
                    )) {
                        error!("Failed to send resize event for {pane_id}: {e}");
                    } else {
                        pane.view_state.last_sent_size = new_size;
                    }
                }

                // Load this pane's snapshot and sync scroll offset.
                let pane_snap = pane.arc_swap.load();
                if pane.view_state.scroll_offset != pane_snap.scroll_offset {
                    pane.view_state.scroll_offset = pane_snap.scroll_offset;
                }

                let is_echo_off = self.config.security.password_indicator
                    && pane.echo_off.load(std::sync::atomic::Ordering::Relaxed);
                let is_active = pane_id == active_pane_id;

                // Build a RecordingContext for this pane if recording is active.
                let rec_window_id = self.recording_window_id(window_id);
                let rec_ctx = self.recording_handle.as_ref().map(|h| {
                    freminal_terminal_emulator::recording::RecordingContext {
                        handle: h,
                        window_id: rec_window_id,
                        #[allow(clippy::cast_possible_truncation)]
                        pane_id: pane_id.raw() as u32,
                    }
                });

                // Render this pane into a child UI scoped to its content rect.
                // show() returns (left_clicked, deferred_key_actions).
                // left_clicked is true when a primary left-click was pressed inside
                // this pane's rect — used below for click-to-focus.
                let show_result =
                    ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |pane_ui| {
                        win.terminal_widget.show(
                            pane_ui,
                            &pane_snap,
                            &mut pane.view_state,
                            &pane.render_state,
                            &mut pane.render_cache,
                            &pane.input_tx,
                            &pane.clipboard_rx,
                            &pane.search_buffer_rx,
                            ui_overlay_open,
                            bg_opacity,
                            self.config.ui.background_image_opacity,
                            self.config.ui.background_image_mode,
                            &self.binding_map,
                            is_echo_off,
                            is_active,
                            pane_id,
                            rec_ctx.as_ref(),
                        )
                    });
                let (left_clicked, deferred_actions) = show_result.inner;
                all_deferred_actions.extend(deferred_actions);

                // Click-to-focus: if a non-active pane was left-clicked, transfer
                // keyboard focus to it and send FocusChange events to both panes.
                if left_clicked && !is_active {
                    let tab = win.tabs.active_tab_mut();
                    let old_active = tab.active_pane;
                    // Notify the previously-active pane that it lost focus.
                    if let Some(old_pane) = tab.pane_tree.find(old_active)
                        && let Err(e) = old_pane.input_tx.send(InputEvent::FocusChange(false))
                    {
                        error!("Failed to send FocusChange(false) to pane {old_active}: {e}");
                    }
                    // Switch focus.
                    tab.active_pane = pane_id;
                    // Notify the newly-active pane that it gained focus.
                    if let Some(new_pane) = tab.pane_tree.find(pane_id)
                        && let Err(e) = new_pane.input_tx.send(InputEvent::FocusChange(true))
                    {
                        error!("Failed to send FocusChange(true) to pane {pane_id}: {e}");
                    }
                }

                // Advance text blink cycle for this pane if it has blinking text.
                if pane_snap.has_blinking_text {
                    // Re-borrow after the allocate_new_ui closure.
                    let tab = win.tabs.active_tab_mut();
                    if let Some(p) = tab.pane_tree.find_mut(pane_id) {
                        p.view_state.tick_text_blink();
                    }
                }

                // Determine repaint delay for this pane.
                let cursor_is_blinking = matches!(
                    pane_snap.cursor_visual_style,
                    freminal_common::cursor::CursorVisualStyle::BlockCursorBlink
                        | freminal_common::cursor::CursorVisualStyle::UnderlineCursorBlink
                        | freminal_common::cursor::CursorVisualStyle::VerticalLineCursorBlink,
                );
                if pane_snap.content_changed || cursor_is_blinking || pane_snap.has_blinking_text {
                    let delay = if pane_snap.content_changed {
                        std::time::Duration::from_millis(16)
                    } else if pane_snap.has_blinking_text {
                        view_state::TEXT_BLINK_TICK_DURATION
                    } else {
                        std::time::Duration::from_millis(500)
                    };
                    shortest_repaint_delay =
                        Some(shortest_repaint_delay.map_or(delay, |prev| prev.min(delay)));
                }
            }

            // ── Window-level post-processing pass ────────────────────
            //
            // When a user GLSL shader is active, the window FBO now contains
            // the composited terminal content from all panes.  We draw it
            // through the user shader back to egui's framebuffer.
            //
            // This callback is registered BEFORE pane borders so the borders
            // are painted on top of the shader output.
            {
                let wpr_check = win
                    .window_post
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let shader_active = wpr_check.is_active();
                let pending = wpr_check.pending_shader.is_some();
                drop(wpr_check);

                if shader_active || pending {
                    let frame_dt = ui.input(|i| i.stable_dt);
                    let wpr_for_post = Arc::clone(&win.window_post);
                    ui.painter().add(egui::PaintCallback {
                        rect: available_rect,
                        callback: Arc::new(CallbackFn::new(move |info, painter| {
                            let gl = painter.gl();
                            let vp = info.viewport_in_pixels();
                            let mut wpr = wpr_for_post
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);

                            // Lazy-init GPU resources.
                            if !wpr.initialized()
                                && let Err(e) = wpr.init(gl)
                            {
                                error!("WindowPostRenderer init failed: {e}");
                                return;
                            }

                            // Process any pending shader change.
                            if let Some(pending_shader) = wpr.pending_shader.take() {
                                match pending_shader {
                                    Some(src)
                                        if let Err(e) = wpr.update_shader(
                                            gl,
                                            &src,
                                            vp.width_px,
                                            vp.height_px,
                                        ) =>
                                    {
                                        error!("Shader compilation failed: {e}");
                                    }
                                    Some(_) => {}
                                    None => wpr.clear_shader(gl),
                                }
                            }

                            // Apply the post-processing pass if the shader is active.
                            if wpr.is_active() {
                                wpr.ensure_fbo(gl, vp.width_px, vp.height_px);
                                // Bind egui's framebuffer as the render target.
                                unsafe {
                                    gl.bind_framebuffer(
                                        glow::FRAMEBUFFER,
                                        painter.intermediate_fbo(),
                                    );
                                }

                                let vp_w = vp.width_px.approx_as::<f32>().unwrap_or(0.0);
                                let vp_h = vp.height_px.approx_as::<f32>().unwrap_or(0.0);
                                wpr.draw_post_pass(gl, vp_w, vp_h, frame_dt);
                            }
                        })),
                    });

                    // When the shader is active, request continuous repaints so
                    // the `u_time` uniform advances smoothly (~60 fps).
                    if shader_active {
                        let anim_delay = std::time::Duration::from_millis(16);
                        shortest_repaint_delay = Some(
                            shortest_repaint_delay.map_or(anim_delay, |prev| prev.min(anim_delay)),
                        );
                    }
                }
            }

            // ── Pane borders ─────────────────────────────────────────
            //
            // Draw tmux-style half-highlighted borders: each split border is
            // divided at the midpoint along its length. The half adjacent to
            // the active pane's subtree is drawn in the active color; the
            // other half gets the inactive color. This makes it visually
            // clear which pane owns each shared edge.
            if has_multiple_panes && zoomed_pane.is_none() {
                let painter = ui.painter();
                let inactive_color = egui::Color32::from_gray(80);
                let active_color = egui::Color32::from_rgb(100, 160, 255);

                let border_rects = win
                    .tabs
                    .active_tab()
                    .pane_tree
                    .split_borders(available_rect, active_pane_id)
                    .unwrap_or_default();

                for border in &border_rects {
                    let r = border.rect;

                    // Determine which halves are active/inactive.
                    // active_in_first == Some(true)  → first half active
                    // active_in_first == Some(false) → second half active
                    // active_in_first == None        → both inactive
                    let (first_color, second_color) = match border.active_in_first {
                        Some(true) => (active_color, inactive_color),
                        Some(false) => (inactive_color, active_color),
                        None => (inactive_color, inactive_color),
                    };

                    match border.direction {
                        panes::SplitDirection::Horizontal => {
                            // Vertical dividing line — split top/bottom.
                            // First child is left → "first half" = top.
                            let mid_y = f32::midpoint(r.min.y, r.max.y);
                            let top = egui::Rect::from_min_max(r.min, egui::pos2(r.max.x, mid_y));
                            let bot = egui::Rect::from_min_max(egui::pos2(r.min.x, mid_y), r.max);

                            painter.line_segment(
                                [top.left_top(), top.left_bottom()],
                                egui::Stroke::new(border_width, first_color),
                            );
                            painter.line_segment(
                                [bot.left_top(), bot.left_bottom()],
                                egui::Stroke::new(border_width, second_color),
                            );
                        }
                        panes::SplitDirection::Vertical => {
                            // Horizontal dividing line — split left/right.
                            // First child is top → "first half" = left.
                            let mid_x = f32::midpoint(r.min.x, r.max.x);
                            let left = egui::Rect::from_min_max(r.min, egui::pos2(mid_x, r.max.y));
                            let right = egui::Rect::from_min_max(egui::pos2(mid_x, r.min.y), r.max);

                            painter.line_segment(
                                [left.left_top(), left.right_top()],
                                egui::Stroke::new(border_width, first_color),
                            );
                            painter.line_segment(
                                [right.left_top(), right.right_top()],
                                egui::Stroke::new(border_width, second_color),
                            );
                        }
                    }
                }
            }

            // Handle key actions that couldn't be dispatched at the input
            // layer because they require full GUI state.
            for action in all_deferred_actions {
                self.dispatch_deferred_action(action, &mut win, window_id);
            }

            // Handle deferred close-pane (needs `ui` for ViewportCommand::Close).
            if win.pending_close_pane {
                win.pending_close_pane = false;
                Self::close_focused_pane(ui, &mut win);
            }

            // Handle deferred directional focus (needs layout rects).
            if let Some(dir) = win.pending_focus_direction.take() {
                Self::focus_pane_in_direction(dir, available_rect, &mut win);
            }

            // Keep the window title bar in sync with the active tab's title.
            // This handles tab switches, OSC 0/2 title changes, and restore
            // from the title stack — all in one place.
            //
            // Only issue the viewport command when the title actually changed;
            // calling `send_viewport_cmd` unconditionally every frame triggers
            // an infinite repaint loop (~3 % idle CPU).
            let active_title = &win.tabs.active_tab().active_pane().title;
            let window_title = if active_title.is_empty() {
                "Freminal"
            } else {
                active_title.as_str()
            };
            if window_title != win.last_window_title {
                window_title.clone_into(&mut win.last_window_title);
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(win.last_window_title.clone()));
            }

            // Schedule a repaint at the shortest interval needed by any pane.
            if let Some(delay) = shortest_repaint_delay {
                ctx.request_repaint_after(delay);
            }
        });

        let elapsed = now.elapsed();
        let frame_time = if elapsed.as_millis() > 0 {
            format!("Frame time={}ms", elapsed.as_millis())
        } else {
            format!("Frame time={}μs", elapsed.as_micros())
        };

        trace!("{}", frame_time);

        // Reinsert per-window state before returning.
        self.windows.insert(window_id, win);

        // Apply a pending layout (set from the Layouts menu).
        if let Some(resolved) = self.pending_load_layout.take() {
            let commands = self.apply_layout(&resolved, window_id, handle);
            self.inject_layout_commands(&commands);
        }
    }

    fn raw_input_hook(&mut self, _window_id: WindowId, raw_input: &mut egui::RawInput) {
        // Override egui's predicted frame time to zero.
        //
        // egui's `request_repaint_after(delay)` subtracts `predicted_dt`
        // (~16.7 ms at the default 1/60) from the requested delay to avoid
        // "overshooting" into the next frame.  With vsync disabled (see the
        // `native_options.vsync = false` below), this subtraction collapses
        // any delay ≤ 16.7 ms to zero — turning every repaint request into
        // an immediate repaint and driving the frame rate to hundreds of FPS
        // during active PTY output.
        //
        // Setting `predicted_dt = 0` disables the subtraction, so our delays
        // are honoured exactly:
        //   - 8 ms  (PTY thread after each batch)  → ~120 FPS cap
        //   - 16 ms (GUI on content_changed)        → ~60 FPS cap
        //   - 500 ms (cursor blink)                 → ~2 FPS
        //   - no request (true idle, steady cursor)  → 0 FPS
        raw_input.predicted_dt = 0.0;
    }
}

/// Run the GUI
///
/// # Errors
/// Will return an error if the GUI fails to run
pub fn run(
    initial_tab: Tab,
    config: Config,
    args: Args,
    config_path: Option<std::path::PathBuf>,
    repaint_handle: Arc<OnceLock<(RepaintProxy, WindowId)>>,
    window_post: Arc<Mutex<WindowPostRenderer>>,
    recording_handle: Option<freminal_terminal_emulator::recording::RecordingHandle>,
) -> Result<()> {
    let icon_bytes = include_bytes!("../../../assets/icon.png");
    let image = image::load_from_memory(icon_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to load window icon: {e}"))?;
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let icon = egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    };

    let window_config = freminal_windowing::WindowConfig {
        title: "Freminal".to_owned(),
        inner_size: None,
        transparent: true,
        icon: Some(icon.clone()),
        app_id: Some("freminal".into()),
    };

    let mut app = FreminalGui::new(
        initial_tab,
        config,
        args,
        repaint_handle,
        config_path,
        window_post,
        recording_handle,
    );
    app.icon = Some(icon);

    freminal_windowing::run(window_config, app).map_err(|e| anyhow::anyhow!(e.to_string()))
}

#[cfg(test)]
mod multi_window_tests {
    use freminal_common::keybindings::{
        BindingKey, BindingMap, BindingModifiers, KeyAction, KeyCombo,
    };

    // ── NewWindow keybinding ────────────────────────────────────────────────

    /// `KeyAction::NewWindow` must appear in `KeyAction::ALL` so that the
    /// settings modal and key-binding serialisation can discover it.
    #[test]
    fn new_window_action_is_in_all() {
        assert!(
            KeyAction::ALL.contains(&KeyAction::NewWindow),
            "KeyAction::NewWindow missing from KeyAction::ALL"
        );
    }

    /// The `name()` method must return the canonical TOML key used in
    /// `config_example.toml` and written by the settings modal.
    #[test]
    fn new_window_action_name() {
        assert_eq!(KeyAction::NewWindow.name(), "new_window");
    }

    /// The `display_label()` must be a human-readable string for the UI.
    #[test]
    fn new_window_action_display_label() {
        assert_eq!(KeyAction::NewWindow.display_label(), "New Window");
    }

    /// `FromStr` round-trip: parsing the canonical name must recover the
    /// `NewWindow` variant.
    #[test]
    fn new_window_action_from_str_round_trips() {
        use std::str::FromStr;
        let Ok(parsed) = KeyAction::from_str("new_window") else {
            panic!("parse failed")
        };
        assert_eq!(parsed, KeyAction::NewWindow);
    }

    // ── Default binding ─────────────────────────────────────────────────────

    /// `BindingMap::default()` must bind `Ctrl+Shift+N` to `NewWindow`.
    /// This is the advertised default in `config_example.toml`.
    #[test]
    fn default_binding_map_contains_new_window() {
        let map = BindingMap::default();
        let combo = KeyCombo::new(BindingKey::N, BindingModifiers::CTRL_SHIFT);
        let action = map.lookup(&combo);
        assert_eq!(
            action,
            Some(KeyAction::NewWindow),
            "Ctrl+Shift+N should be bound to NewWindow in the default map"
        );
    }

    /// `NewWindow` must be discoverable by action — the reverse lookup from
    /// action to combo must return a non-empty list so the settings modal can
    /// display the current binding.
    #[test]
    fn default_binding_map_new_window_is_discoverable() {
        let map = BindingMap::default();
        let combos = map.all_combos_for(KeyAction::NewWindow);
        assert!(
            !combos.is_empty(),
            "NewWindow must have at least one combo in the default binding map"
        );
    }

    // ── Args Clone ──────────────────────────────────────────────────────────

    /// `Args` must implement `Clone` so that each window can hold an
    /// independent copy for spawning new PTY tabs.  This test is a
    /// compile-time check disguised as a runtime assertion.
    #[test]
    fn args_implements_clone() {
        use clap::Parser;
        use freminal_common::args::Args;
        // Parse from an empty argv (just the program name) to get a default Args.
        let args = Args::parse_from(["freminal"]);
        // Clone into a separate binding to verify the trait is implemented.
        // The clone is used so the compiler doesn't elide it.
        let cloned = args.clone();
        assert_eq!(cloned.show_all_debug, args.show_all_debug);
        // If Args does not derive Clone this file will not compile.
    }
}
