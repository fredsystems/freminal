// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, Mutex, OnceLock};

use conv2::ConvUtil as _;
use freminal_common::pty_write::FreminalTerminalSize;
use freminal_common::terminal_size::{DEFAULT_HEIGHT, DEFAULT_WIDTH};
use freminal_terminal_emulator::io::InputEvent;
use freminal_windowing::{RepaintProxy, WindowId};
use tracing::{error, warn};

use super::window::PerWindowState;
use super::{FreminalGui, panes, pty, renderer, tabs, terminal, view_state};

impl FreminalGui {
    /// Spawn a new PTY-backed tab and add it to the tab manager.
    ///
    /// Uses the stored `Args` and `Config` to configure the new terminal.
    /// Logs an error and does nothing if the PTY fails to start.
    pub(super) fn spawn_new_tab(&self, win: &mut PerWindowState) {
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
            .map_or((0, 0), |p| p.view_state.last_sent_size);
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
            .and_then(|p| p.arc_swap.load().cwd.clone());
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
                    render_state: terminal::new_render_state(Arc::clone(&win.window_post)),
                    render_cache: terminal::PaneRenderCache::new(),
                };
                let tab = tabs::Tab::new(id, pane);
                // Inform the new tab of the current theme mode so DECRPM
                // ?2031 queries return the correct locked/dynamic status.
                if let Some(active) = tab.active_pane() {
                    if let Err(e) = active.input_tx.send(InputEvent::ThemeModeUpdate(
                        self.config.theme.mode,
                        win.os_dark_mode,
                    )) {
                        error!("Failed to send ThemeModeUpdate to new tab: {e}");
                    }
                } else {
                    warn!("new tab has no active pane when sending ThemeModeUpdate");
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
    pub(super) fn initial_size_for_split(
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
            .map_or((0, 0), |p| p.view_state.last_sent_size);
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
    pub(super) fn spawn_split_pane(
        &self,
        win: &mut PerWindowState,
        direction: panes::SplitDirection,
    ) {
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
            .and_then(|p| p.arc_swap.load().cwd.clone());
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
                render_state: terminal::new_render_state(Arc::clone(&win.window_post)),
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
    pub(super) fn spawn_new_window(&self, handle: &freminal_windowing::WindowHandle<'_>) {
        handle.create_window(freminal_windowing::WindowConfig {
            title: "Freminal".to_owned(),
            inner_size: None,
            position: None,
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
    pub(super) fn spawn_pane_from_leaf(
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
}
