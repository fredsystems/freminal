// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, Mutex, OnceLock};

use freminal_terminal_emulator::io::InputEvent;
use freminal_windowing::{RepaintProxy, WindowId};
use tracing::{debug, error};

use super::window::PerWindowState;
use super::{FreminalGui, panes, renderer, rendering, tabs, terminal};

// ── Layout helpers ────────────────────────────────────────────────────────────

/// Convert a `LayoutSplitDirection` to a `panes::SplitDirection`.
pub(super) const fn layout_dir_to_pane_dir(
    dir: freminal_common::layout::LayoutSplitDirection,
) -> panes::SplitDirection {
    match dir {
        // LayoutSplitDirection::Horizontal = top/bottom (horizontal divider)
        // panes::SplitDirection::Vertical  = top/bottom (horizontal divider)
        freminal_common::layout::LayoutSplitDirection::Horizontal => {
            panes::SplitDirection::Vertical
        }
        // LayoutSplitDirection::Vertical = left/right (vertical divider)
        // panes::SplitDirection::Horizontal = left/right (vertical divider)
        freminal_common::layout::LayoutSplitDirection::Vertical => {
            panes::SplitDirection::Horizontal
        }
    }
}

/// Extract the root leaf from a `ResolvedNode` tree.
///
/// If the root is a `Leaf`, returns `(Some(leaf), None)`.
/// If the root is a `Split`, returns `(Some(first_leaf), Some(root_split))` — the
/// `first_leaf` is the leftmost/topmost leaf, suitable for constructing the initial
/// `Tab` pane, and the `root_split` is the full tree (used to build the rest).
pub(super) fn extract_root_leaf(
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

impl FreminalGui {
    /// Insert all panes from `node` as the `second` child of a split on
    /// `target_id`.  Returns the ID of the deepest leaf inserted.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn insert_subtree_as_split(
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
    pub(super) fn build_tab_from_resolved(
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
    pub(super) fn inject_layout_commands(&self, commands: &[(panes::PaneId, String)]) {
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
    pub(super) fn read_cwd_for_pane_with_extra(
        &self,
        pane_id: panes::PaneId,
        extra_win: Option<&PerWindowState>,
    ) -> Option<String> {
        // Search extra_win first (current window removed from self.windows during update()).
        let child_pid = extra_win
            .and_then(|win| {
                win.tabs.iter().find_map(|tab| {
                    tab.pane_tree.iter_panes().ok().and_then(|ps| {
                        ps.into_iter()
                            .find(|p| p.id == pane_id)
                            .and_then(|p| p.child_pid)
                    })
                })
            })
            .or_else(|| {
                // Find the pane across all other windows and tabs.
                self.windows.values().find_map(|win| {
                    win.tabs.iter().find_map(|tab| {
                        tab.pane_tree.iter_panes().ok().and_then(|ps| {
                            ps.into_iter()
                                .find(|p| p.id == pane_id)
                                .and_then(|p| p.child_pid)
                        })
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
    /// `extra_win` should be `Some(win)` when called during `update()`, because
    /// the current window has been removed from `self.windows` for the duration
    /// of the frame.  Pass `None` when called outside `update()` (e.g. from
    /// `auto_save_session` in `on_close_requested`) where all windows are still
    /// in `self.windows`.
    ///
    /// `name` is the human-readable name for the layout; if empty the path
    /// file stem is used as a fallback.
    ///
    /// # Errors
    ///
    /// Returns an error if the layout cannot be serialised or the file cannot be written.
    pub fn save_layout(
        &self,
        path: &std::path::Path,
        name: &str,
        extra_win: Option<&PerWindowState>,
    ) -> anyhow::Result<()> {
        use freminal_common::layout::{Layout, LayoutMeta, LayoutTab, LayoutWindow};

        let mut windows: Vec<LayoutWindow> = Vec::new();

        // Helper closure: build a `LayoutWindow` from any `PerWindowState`.
        // `win_extra` is the window that was removed from self.windows for the
        // current frame — used so CWD lookups can search it when it is the same
        // window being serialised.
        let build_window = |win: &PerWindowState, win_extra: Option<&PerWindowState>| {
            let active_tab_idx = win.tabs.active_index();
            let mut tabs: Vec<LayoutTab> = Vec::new();
            for (tab_idx, tab) in win.tabs.iter().enumerate() {
                let active_pane_id = if tab_idx == active_tab_idx {
                    Some(tab.active_pane)
                } else {
                    None
                };
                let panes = tab.pane_tree.to_layout_panes(active_pane_id, |pane_id| {
                    self.read_cwd_for_pane_with_extra(pane_id, win_extra)
                });
                tabs.push(LayoutTab {
                    title: None,
                    active: tab_idx == active_tab_idx,
                    panes,
                });
            }
            LayoutWindow {
                size: win.last_known_size,
                position: win.last_known_position,
                monitor: None,
                tabs,
            }
        };

        // If a current window was extracted from self.windows, include it first.
        if let Some(win) = extra_win {
            windows.push(build_window(win, Some(win)));
        }

        // Then all other windows (or all windows when extra_win is None).
        for win in self.windows.values() {
            windows.push(build_window(win, extra_win));
        }

        let layout_name = if name.is_empty() {
            path.file_stem().and_then(|s| s.to_str()).map(str::to_owned)
        } else {
            Some(name.to_owned())
        };

        let layout = Layout {
            layout: LayoutMeta {
                name: layout_name,
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
        window_id: freminal_windowing::WindowId,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) -> Vec<(panes::PaneId, String)> {
        use freminal_common::terminal_size::{DEFAULT_HEIGHT, DEFAULT_WIDTH};

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
                // Schedule geometry restoration for this window — applied on the
                // next frame via ctx.send_viewport_cmd in update().
                if first_window.size.is_some() || first_window.position.is_some() {
                    win.pending_geometry = Some((first_window.size, first_window.position));
                }
            }
        }

        // Queue remaining windows for creation.
        for extra_window in windows {
            self.pending_layout_windows.push_back(extra_window.clone());
            handle.create_window(freminal_windowing::WindowConfig {
                title: "Freminal".to_owned(),
                inner_size: extra_window.size.map(<[u32; 2]>::into),
                position: extra_window.position.map(<[i32; 2]>::into),
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
    pub(super) fn build_tabs_for_window(
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
            if let Some((mut tab, active_pane)) = self.build_tab_from_resolved(
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
                // Apply the active pane from the layout if one was marked.
                if let Some(id) = active_pane {
                    tab.active_pane = id;
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
    pub(super) fn build_window_from_pending_layout(
        &mut self,
        window_id: freminal_windowing::WindowId,
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

        let terminal_widget = terminal::FreminalTerminalWidget::new(ctx, &self.config)
            .unwrap_or_else(|e| {
                tracing::error!("fatal: failed to initialise terminal widget (font manager): {e}");
                std::process::exit(1);
            });
        let (cell_w, cell_h) = terminal_widget.cell_size();
        let initial_size = Self::compute_initial_size(inner_size.0, inner_size.1, cell_w, cell_h);

        let (tab_mgr_opt, commands) = self.build_tabs_for_window(
            &resolved_window,
            &repaint_handle,
            &window_post,
            &initial_size,
        );
        let tab_mgr = tab_mgr_opt?;

        let win = super::window::PerWindowState {
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
            pending_geometry: None,
            last_known_size: None,
            last_known_position: None,
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
}
