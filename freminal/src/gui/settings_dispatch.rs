// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_terminal_emulator::io::InputEvent;
use tracing::error;

use super::FreminalGui;
use super::settings::SettingsAction;

impl FreminalGui {
    /// Handle a `SettingsAction` from the standalone settings window.
    ///
    /// Unlike the inline modal path (which operates on a single `win`), this
    /// applies changes across ALL terminal windows in `self.windows`.
    #[allow(clippy::too_many_lines)]
    pub(super) fn handle_settings_action(
        &mut self,
        action: &SettingsAction,
        handle: &freminal_windowing::WindowHandle<'_>,
        _settings_window_id: freminal_windowing::WindowId,
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
            SettingsAction::DeleteLayout(path) => {
                if let Err(e) = std::fs::remove_file(path) {
                    error!("Failed to delete layout file '{}': {e}", path.display());
                }
                // Refresh the layout list regardless (file may already be gone).
                self.discovered_layouts = freminal_common::config::layout_library_dir()
                    .map(|dir| freminal_common::layout::discover_layouts(&dir))
                    .unwrap_or_default();
                self.settings_modal.discovered_layouts = self.discovered_layouts.clone();
            }
        }
    }
}
