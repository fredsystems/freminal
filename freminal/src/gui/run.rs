// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Result;
use egui;
use freminal_common::args::Args;
use freminal_common::config::Config;
use freminal_windowing::{RepaintProxy, WindowId};
use renderer::WindowPostRenderer;
use tabs::Tab;

use super::{FreminalGui, renderer, tabs};

/// Launch the Freminal GUI application.
///
/// # Errors
///
/// Returns an error if the window icon cannot be loaded from the embedded PNG
/// bytes, or if the windowing event loop fails to start.
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

    // Seed the initial window's geometry from window_state.toml if
    // available.  Setting this at creation time (rather than via a later
    // viewport command) is essential on Wayland: xdg-shell ignores
    // resize requests that arrive after the initial surface configure in
    // many compositors.
    let (initial_size, initial_position) = freminal_common::window_state::window_state_path()
        .as_deref()
        .map(freminal_common::window_state::WindowState::load_or_default)
        .and_then(|state| state.main_windows.into_iter().next())
        .map_or((None, None), |geom| {
            (
                geom.size.map(<[u32; 2]>::into),
                geom.position.map(<[i32; 2]>::into),
            )
        });

    let window_config = freminal_windowing::WindowConfig {
        title: "Freminal".to_owned(),
        inner_size: initial_size,
        position: initial_position,
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
