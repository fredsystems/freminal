// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, Mutex};

use super::renderer::WindowPostRenderer;
use tracing::error;

impl super::FreminalGui {
    /// Push a `pending_shader` change to every secondary window's
    /// `WindowPostRenderer`.
    ///
    /// Called from hot-reload and settings-apply so that secondary windows
    /// recompile the same shader as the root.
    ///
    /// `shader` is `None` to clear the shader, or `Some(source)` to set it.
    pub(super) fn propagate_shader_to_secondary_windows(&self, shader: Option<&String>) {
        let pending = shader.cloned();
        for (_vid, state) in &self.secondary_windows {
            if let Ok(win) = state.try_lock()
                && let Ok(mut wpr) = win.window_post.lock()
            {
                wpr.pending_shader = Some(pending.clone());
            }
        }
    }

    /// Push a background image change to every pane in every secondary window.
    ///
    /// Called from settings-apply so secondary windows render the same
    /// background image as the root.
    pub(super) fn propagate_bg_image_to_secondary_windows(
        &self,
        path: Option<&std::path::PathBuf>,
    ) {
        let owned = path.cloned();
        for (_vid, state) in &self.secondary_windows {
            if let Ok(win) = state.try_lock() {
                for tab in win.tabs.iter() {
                    if let Ok(panes) = tab.pane_tree.iter_panes() {
                        for pane in panes {
                            if let Ok(mut rs) = pane.render_state.lock() {
                                rs.set_pending_bg_image(owned.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    /// Copy the root window's active/pending shader into a new
    /// `WindowPostRenderer` so it compiles its own copy on the next frame.
    ///
    /// If the root has a pending shader change, that pending value is copied.
    /// Otherwise, if the root already has an active (compiled) shader, the
    /// source is re-read from the config path.
    pub(super) fn copy_root_shader_to(&self, target: &Arc<Mutex<WindowPostRenderer>>) {
        let root_guard = self
            .window_post
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let shader_src: Option<Option<String>> = root_guard.pending_shader.as_ref().map_or_else(
            || {
                if root_guard.is_active() {
                    self.config.shader.path.as_ref().and_then(|p| {
                        std::fs::read_to_string(p)
                            .map_err(|e| {
                                error!(
                                    "Failed to read shader for new window from '{}': {e}",
                                    p.display()
                                );
                            })
                            .ok()
                            .map(Some)
                    })
                } else {
                    None
                }
            },
            |pending| Some(pending.clone()),
        );
        drop(root_guard);
        if let Some(src) = shader_src {
            target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pending_shader = Some(src);
        }
    }
}
