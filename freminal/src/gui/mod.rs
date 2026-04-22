// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use conv2::ConvUtil;
use egui;
use freminal_common::args::Args;
use freminal_common::config::Config;
use freminal_common::pty_write::FreminalTerminalSize;
use freminal_common::send_or_log;
use freminal_common::terminal_size::{DEFAULT_HEIGHT, DEFAULT_WIDTH};
use freminal_terminal_emulator::io::InputEvent;
use freminal_windowing::{RepaintProxy, WindowId};
use renderer::WindowPostRenderer;
use settings::SettingsModal;
use tabs::Tab;
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
mod app_impl;
mod hot_reload;
mod layout_ops;
mod menu;
mod rendering;
mod run;
mod session;
mod settings_dispatch;
mod tab_spawning;
pub(crate) mod window;

use tracing::{error, warn};

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

    /// Persisted ephemeral UI window geometry for the Settings window
    /// and each main terminal window.  Loaded from `window_state.toml`
    /// at startup: the settings entry is consulted when the settings
    /// window opens, and `main_windows` is consulted at app launch to
    /// seed the initial window's `WindowConfig`.  Updated continuously
    /// while windows are moving/resizing and persisted on window close
    /// so the next launch restores the user's layout.
    window_state: freminal_common::window_state::WindowState,

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

    /// When `Some`, the Layouts menu is showing an inline name-entry prompt.
    /// The string holds the name being typed; an empty string is a valid
    /// initial state (user hasn't typed yet).
    pending_save_layout: Option<String>,
    /// True only on the first frame after the save-layout prompt opens.
    /// Used to focus the text field exactly once instead of every frame.
    save_layout_prompt_just_opened: bool,
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
        if let Some(pane) = initial_tab.active_pane() {
            send_or_log!(
                pane.input_tx,
                InputEvent::ThemeModeUpdate(config.theme.mode, os_dark_mode),
                "Failed to send initial ThemeModeUpdate to tab"
            );
        } else {
            warn!("initial tab has no active pane when sending ThemeModeUpdate");
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
            window_state: freminal_common::window_state::window_state_path()
                .as_deref()
                .map(freminal_common::window_state::WindowState::load_or_default)
                .unwrap_or_default(),
            recording_handle,
            recording_window_ids: HashMap::new(),
            next_recording_window_id: 0,
            pending_layout_windows: std::collections::VecDeque::new(),
            discovered_layouts: freminal_common::config::layout_library_dir()
                .map(|dir| freminal_common::layout::discover_layouts(&dir))
                .unwrap_or_default(),
            pending_load_layout: None,
            pending_save_layout: None,
            save_layout_prompt_just_opened: false,
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
}

pub use run::run;

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
