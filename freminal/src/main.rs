// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Binary entry point and PTY threading model for the Freminal terminal emulator.
//!
//! # Threading model
//!
//! ```text
//! OS PTY fd
//!   └─ reader thread: reads chunks, sends PtyRead over channel
//!
//! PTY Processing Thread (owns TerminalEmulator exclusively)
//!   ├─ Receives PtyRead from OS PTY reader thread
//!   ├─ Receives InputEvent from GUI (keyboard, resize, focus)
//!   ├─ After each batch: publishes Arc<TerminalSnapshot> via ArcSwap
//!   └─ Sends WindowCommand to GUI for Report*/Viewport handling
//!
//! GUI Thread (update() — pure render, no mutation)
//!   ├─ Loads TerminalSnapshot from ArcSwap (atomic, lock-free)
//!   ├─ Sends InputEvent through crossbeam channel
//!   ├─ Sends PtyWrite directly for Report* responses
//!   └─ Owns ViewState (scroll offset, mouse, focus — never shared)
//! ```

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
#![deny(
    clippy::pedantic,
    clippy::cargo,
    clippy::nursery,
    clippy::style,
    clippy::correctness,
    clippy::all,
    clippy::suspicious,
    clippy::complexity,
    clippy::perf,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(clippy::multiple_crate_versions)] // Allow multiple versions from transitive dependencies
#![allow(clippy::cargo_common_metadata)] // Metadata is inherited from workspace

#[macro_use]
extern crate tracing;

use std::sync::{Arc, Mutex, OnceLock};

#[cfg(feature = "playback")]
use arc_swap::ArcSwap;
#[cfg(feature = "playback")]
use crossbeam_channel::unbounded;
#[cfg(feature = "playback")]
use freminal_terminal_emulator::interface::TerminalEmulator;
#[cfg(feature = "playback")]
use freminal_terminal_emulator::io::{InputEvent, WindowCommand};
#[cfg(feature = "playback")]
use freminal_terminal_emulator::snapshot::TerminalSnapshot;
use tracing::Level;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{
    EnvFilter, Layer,
    filter::Directive,
    fmt::{self, layer},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

pub mod gui;
#[cfg(feature = "playback")]
pub mod playback;

use anyhow::Result;
use freminal_common::{args::Args, config, config::load_config, themes};
use gui::pty::spawn_pty_tab;

use clap::Parser;

/// Run the normal (non-playback) PTY terminal path.
///
/// Spawns a PTY-backed terminal tab via [`spawn_pty_tab`] and starts the
/// GUI event loop.
fn normal_run(args: Args, cfg: freminal_common::config::Config) -> Result<()> {
    // Select the initial theme.  The OS dark/light preference is not yet
    // available (no egui context), so `active_slug(false)` assumes light mode
    // for `ThemeMode::Auto`.  The GUI constructor will detect the real OS
    // preference and send a corrective `ThemeChange` if needed.
    let theme = themes::by_slug(cfg.theme.active_slug(false)).unwrap_or(&themes::CATPPUCCIN_MOCHA);

    // Shared egui context handle so the PTY consumer thread can request
    // repaints after publishing new snapshots.
    let repaint_handle: Arc<
        OnceLock<(
            freminal_windowing::RepaintProxy,
            freminal_windowing::WindowId,
        )>,
    > = Arc::new(OnceLock::new());

    let channels = spawn_pty_tab(&args, cfg.scrollback.limit, theme, &repaint_handle)?;

    let config_path = args.config.clone();

    let window_post = Arc::new(Mutex::new(gui::renderer::WindowPostRenderer::new()));

    let initial_pane = gui::panes::Pane {
        id: gui::panes::PaneId::first(),
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
        view_state: gui::view_state::ViewState::new(),
        echo_off: channels.echo_off,
        render_state: gui::terminal::new_render_state(Arc::clone(&window_post)),
        render_cache: gui::terminal::PaneRenderCache::new(),
    };
    let initial_tab = gui::tabs::Tab::new(gui::tabs::TabId::first(), initial_pane);

    gui::run(
        initial_tab,
        cfg,
        args,
        config_path,
        repaint_handle,
        window_post,
        #[cfg(feature = "playback")]
        false,
    )
}

// Inherently large: application entry point that wires all subsystems (PTY reader, PTY
// consumer thread, GUI). Each section is necessary; splitting would produce artificial helpers.
#[allow(clippy::too_many_lines)]
fn main() {
    // use env for filtering
    // example
    // RUST_LOG=none,freminal=debug cargo run

    let args = Args::parse();

    // Collect warnings that occur before the tracing subscriber is
    // initialised.  They are replayed as `warn!()` once logging is ready.
    // On Windows (with windows_subsystem = "windows") there is no console
    // attached, so eprintln!() output would be silently lost.
    let mut early_warnings: Vec<String> = Vec::new();

    // ── 1. Load config and apply CLI overrides ──────────────────────────
    let mut cfg = match load_config(args.config.as_deref()) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Error: failed to load config: {err:#}");
            std::process::exit(1);
        }
    };

    cfg.apply_cli_overrides(
        args.shell.as_deref(),
        args.write_logs_to_file,
        args.hide_menu_bar,
    );

    // Print deprecation notice if --write-logs-to-file was used.
    if args.write_logs_to_file.is_some() {
        early_warnings.push(
            "--write-logs-to-file is deprecated and ignored. \
             File logging is now managed automatically. Freminal will \
             attempt to write logs to the platform log directory; if no \
             suitable log directory is available or log files cannot be \
             created, logs will only be written to the console."
                .to_string(),
        );
    }

    // ── 2. Set up logging ───────────────────────────────────────────────
    //
    // Two layers:
    //   - Stdout layer: INFO by default (or RUST_LOG override)
    //   - File layer:   config-specified level (default DEBUG), always on
    //
    // Both layers share framework silencers (winit, wgpu, egui = off)
    // unless --show-all-debug is set.

    // Stdout filter: INFO default, RUST_LOG override, framework silencers.
    let stdout_filter = if args.show_all_debug {
        EnvFilter::builder()
            .with_default_directive(Level::INFO.into())
            .from_env_lossy()
    } else {
        let mut filter = EnvFilter::builder()
            .with_default_directive(Level::INFO.into())
            .from_env_lossy();
        for spec in &["winit=off", "wgpu=off", "egui=off"] {
            match spec.parse::<Directive>() {
                Ok(d) => filter = filter.add_directive(d),
                Err(e) => {
                    eprintln!("Failed to parse directive {spec}: {e}");
                    std::process::exit(1);
                }
            }
        }
        filter
    };

    // File filter: config level (default INFO), framework silencers.
    let file_log_level = cfg.file_log_level();
    let file_default_directive: Directive = file_log_level.parse().unwrap_or_else(|_| {
        early_warnings.push(format!(
            "invalid log level \"{file_log_level}\" in config; falling back to info"
        ));
        Level::INFO.into()
    });

    let file_filter = if args.show_all_debug {
        EnvFilter::builder()
            .with_default_directive(file_default_directive)
            .from_env_lossy()
    } else {
        let mut filter = EnvFilter::builder()
            .with_default_directive(file_default_directive)
            .from_env_lossy();
        for spec in &["winit=off", "wgpu=off", "egui=off"] {
            match spec.parse::<Directive>() {
                Ok(d) => filter = filter.add_directive(d),
                Err(e) => {
                    eprintln!("Failed to parse directive {spec}: {e}");
                    std::process::exit(1);
                }
            }
        }
        filter
    };

    let std_out_layer = layer()
        .with_line_number(true)
        .with_span_events(fmt::format::FmtSpan::ACTIVE)
        .compact()
        .with_filter(stdout_filter);

    // Always-on file appender targeting the platform log directory.
    let log_dir_path = config::log_dir();
    let file_layer = log_dir_path.as_ref().and_then(|dir| {
        match RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .max_log_files(7)
            .filename_prefix("freminal")
            .filename_suffix("log")
            .build(dir)
        {
            Ok(appender) => Some(
                layer()
                    .with_ansi(false)
                    .with_writer(appender)
                    .with_filter(file_filter),
            ),
            Err(e) => {
                early_warnings.push(format!("Failed to create file appender: {e}"));
                None
            }
        }
    });

    // `Option<Layer>` implements `Layer` (None = no-op), so both branches
    // produce the same subscriber type.
    tracing_subscriber::registry()
        .with(file_layer)
        .with(std_out_layer)
        .init();

    // Replay any warnings that were collected before tracing was ready.
    for msg in &early_warnings {
        warn!("{msg}");
    }

    info!("Starting freminal");
    if let Some(ref dir) = log_dir_path {
        info!("Log directory: {}", dir.display());
    }
    debug!("Loaded config: {:#?}", cfg);

    // Warn if both a positional command and --shell are specified.
    // The positional command takes precedence (handled in TerminalEmulator::new).
    let mut args = args;
    if !args.command.is_empty() && args.shell.is_some() {
        warn!(
            "Both --shell and a positional command were specified; \
             the positional command takes precedence and --shell is ignored"
        );
    }

    // Propagate the merged shell path back into args so that
    // TerminalEmulator::new (which reads args.shell) gets the effective
    // value from the CLI > TOML > default precedence chain.
    if args.shell.is_none() {
        args.shell = cfg.shell_path().map(String::from);
    }

    // ── 3. Create the emulator and data source ───────────────────────
    //
    // Normal mode: spawn a PTY, feed its output through a channel.
    // Playback mode: parse a FREC recording file into frames, hand them
    //                to a dedicated playback consumer thread.

    #[cfg(feature = "playback")]
    let is_playback = args.playback.is_some();

    #[cfg(feature = "playback")]
    let res = if let Some(playback_path) = args.playback.clone() {
        // ── Playback mode ───────────────────────────────────────────
        let file_data = match std::fs::read(&playback_path) {
            Ok(d) => d,
            Err(e) => {
                error!(
                    "Failed to read playback file {}: {e}",
                    playback_path.display()
                );
                return;
            }
        };

        let frames = match freminal_terminal_emulator::recording::parse_recording(&file_data) {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to parse recording file: {e}");
                return;
            }
        };

        info!(
            "Loaded {} playback frames from {}",
            frames.len(),
            playback_path.display()
        );

        let (mut terminal, _pty_write_rx) =
            TerminalEmulator::new_headless(Some(cfg.scrollback.limit));

        // Apply the configured theme.
        //
        // In playback mode there is no egui context yet, so we cannot detect
        // the OS dark/light preference.  `active_slug(false)` assumes light
        // mode for `ThemeMode::Auto`.  The GUI constructor will correct this
        // by sending a `ThemeChange` once the real OS preference is known.
        let theme =
            themes::by_slug(cfg.theme.active_slug(false)).unwrap_or(&themes::CATPPUCCIN_MOCHA);
        terminal.internal.handler.set_theme(theme);

        // Shared snapshot published by the playback thread.
        let arc_swap: Arc<ArcSwap<TerminalSnapshot>> =
            Arc::new(ArcSwap::from_pointee(TerminalSnapshot::empty()));
        let arc_swap_gui = Arc::clone(&arc_swap);

        // The playback emulator has no real PTY, but the GUI still needs a
        // pty_write_tx for Report* responses from handle_window_manipulation.
        // Create a throwaway channel — responses are silently dropped.
        let (pty_write_tx, _pty_write_sink) =
            crossbeam_channel::unbounded::<freminal_common::pty_write::PtyWrite>();

        let (input_tx, input_rx) = unbounded::<InputEvent>();
        let (window_cmd_tx, window_cmd_rx) = unbounded::<WindowCommand>();
        let (clipboard_tx, clipboard_rx) = crossbeam_channel::bounded::<String>(1);
        let (search_buffer_tx, search_buffer_rx) = crossbeam_channel::bounded::<(
            usize,
            Vec<freminal_common::buffer_states::tchar::TChar>,
        )>(1);

        let repaint_handle: Arc<
            OnceLock<(
                freminal_windowing::RepaintProxy,
                freminal_windowing::WindowId,
            )>,
        > = Arc::new(OnceLock::new());
        let repaint_handle_playback = Arc::clone(&repaint_handle);

        std::thread::spawn(move || {
            playback::run_playback_thread(
                terminal,
                frames,
                input_rx,
                window_cmd_tx,
                arc_swap,
                repaint_handle_playback,
                clipboard_tx,
                search_buffer_tx,
            );
        });

        let config_path = args.config.clone();
        let (_pty_dead_tx, pty_dead_rx) = crossbeam_channel::bounded::<()>(1);
        let window_post = Arc::new(Mutex::new(gui::renderer::WindowPostRenderer::new()));
        let playback_pane = gui::panes::Pane {
            id: gui::panes::PaneId::first(),
            arc_swap: arc_swap_gui,
            input_tx,
            pty_write_tx,
            window_cmd_rx,
            clipboard_rx,
            search_buffer_rx,
            pty_dead_rx,
            title: "Playback".to_owned(),
            bell_active: false,
            title_stack: Vec::new(),
            view_state: gui::view_state::ViewState::new(),
            echo_off: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            render_state: gui::terminal::new_render_state(Arc::clone(&window_post)),
            render_cache: gui::terminal::PaneRenderCache::new(),
        };
        gui::run(
            gui::tabs::Tab::new(gui::tabs::TabId::first(), playback_pane),
            cfg,
            args,
            config_path,
            repaint_handle,
            window_post,
            is_playback,
        )
    } else {
        // ── Normal mode (playback feature enabled, but no playback path) ──
        normal_run(args, cfg)
    };

    #[cfg(not(feature = "playback"))]
    let res = normal_run(args, cfg);

    if let Err(e) = res {
        error!("Failed to run terminal emulator: {}", e);
    }

    info!("Shutting down freminal");
}
