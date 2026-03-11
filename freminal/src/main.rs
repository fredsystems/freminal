// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

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

// #![warn(missing_docs)]

#[macro_use]
extern crate tracing;

use arc_swap::ArcSwap;
use crossbeam_channel::unbounded;
use freminal_terminal_emulator::interface::TerminalEmulator;
use freminal_terminal_emulator::io::{InputEvent, WindowCommand};
use freminal_terminal_emulator::snapshot::TerminalSnapshot;
use std::sync::{Arc, OnceLock};
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

use freminal_common::{args::Args, config, config::load_config};

use clap::Parser;

#[allow(clippy::too_many_lines)]
fn main() {
    // use env for filtering
    // example
    // RUST_LOG=none,freminal=debug cargo run

    let args = Args::parse();

    // ── 1. Load config and apply CLI overrides ──────────────────────────
    let mut cfg = match load_config(args.config.as_deref()) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Error: failed to load config: {err:#}");
            std::process::exit(1);
        }
    };

    cfg.apply_cli_overrides(args.shell.as_deref(), args.write_logs_to_file);

    // Print deprecation notice if --write-logs-to-file was used.
    if args.write_logs_to_file.is_some() {
        eprintln!(
            "WARNING: --write-logs-to-file is deprecated and ignored. \
             File logging is now always on. Logs are written to the \
             platform log directory."
        );
    }

    // ── 2. Set up logging ───────────────────────────────────────────────
    //
    // Two layers:
    //   - Stdout layer: INFO by default (or RUST_LOG override)
    //   - File layer:   config-specified level (default DEBUG), always on
    //
    // Both layers share framework silencers (winit, wgpu, eframe, egui = off)
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
        for spec in &["winit=off", "wgpu=off", "eframe=off", "egui=off"] {
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

    // File filter: config level (default DEBUG), framework silencers.
    let file_log_level = cfg.file_log_level();
    let file_default_directive: Directive = file_log_level.parse().unwrap_or_else(|_| {
        eprintln!(
            "WARNING: invalid log level \"{file_log_level}\" in config; falling back to debug"
        );
        Level::DEBUG.into()
    });

    let file_filter = if args.show_all_debug {
        EnvFilter::builder()
            .with_default_directive(file_default_directive)
            .from_env_lossy()
    } else {
        let mut filter = EnvFilter::builder()
            .with_default_directive(file_default_directive)
            .from_env_lossy();
        for spec in &["winit=off", "wgpu=off", "eframe=off", "egui=off"] {
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
                eprintln!("Failed to create file appender: {e}");
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

    info!("Starting freminal");
    if let Some(ref dir) = log_dir_path {
        info!("Log directory: {}", dir.display());
    }
    debug!("Loaded config: {:#?}", cfg);

    // Propagate the merged shell path back into args so that
    // TerminalEmulator::new (which reads args.shell) gets the effective
    // value from the CLI > TOML > default precedence chain.
    let mut args = args;
    if args.shell.is_none() {
        args.shell = cfg.shell_path().map(String::from);
    }

    let res = match TerminalEmulator::new(&args, Some(cfg.scrollback.limit)) {
        Ok((terminal, pty_read_rx)) => {
            // Shared snapshot published by the PTY thread, consumed lock-free by the GUI.
            let arc_swap: Arc<ArcSwap<TerminalSnapshot>> =
                Arc::new(ArcSwap::from_pointee(TerminalSnapshot::empty()));
            let arc_swap_gui = Arc::clone(&arc_swap);

            // Clone the PTY write sender before the emulator is moved into the
            // consumer thread.  The GUI uses it to send Report* responses back
            // to the PTY without going through the emulator.
            let pty_write_tx = terminal.clone_write_tx();

            // Channel for GUI → PTY-consumer thread events (resize, key, focus).
            let (input_tx, input_rx) = unbounded::<InputEvent>();

            // Channel for PTY-consumer thread → GUI (window manipulation commands).
            let (window_cmd_tx, window_cmd_rx) = unbounded::<WindowCommand>();

            // Shared egui context handle so the PTY consumer thread can request
            // repaints after publishing new snapshots.  The GUI sets it during
            // `FreminalGui::new()`; the PTY thread reads it after each store.
            let egui_ctx: Arc<OnceLock<eframe::egui::Context>> = Arc::new(OnceLock::new());
            let egui_ctx_pty = Arc::clone(&egui_ctx);

            // The TerminalEmulator is fully owned by the PTY consumer thread.
            // No FairMutex. No shared lock.
            std::thread::spawn(move || {
                let mut emulator = terminal;

                loop {
                    // Use crossbeam select! to wait on either a PTY read or an
                    // InputEvent from the GUI without spinning.
                    crossbeam_channel::select! {
                        recv(pty_read_rx) -> msg => {
                            if let Ok(read) = msg {
                                emulator.handle_incoming_data(
                                    &read.buf[0..read.read_amount],
                                );
                            } else {
                                // PTY read channel closed — shell exited.
                                info!("PTY read channel closed; consumer thread exiting");
                                break;
                            }
                        }
                        recv(input_rx) -> msg => {
                            match msg {
                                Ok(InputEvent::Resize(w, h, pw, ph)) => {
                                    emulator.handle_resize_event(w, h, pw, ph);
                                }
                                Ok(InputEvent::Key(bytes)) => {
                                    if let Err(e) = emulator.write_raw_bytes(&bytes) {
                                        error!("Failed to forward key bytes to PTY: {e}");
                                    }
                                }
                                Ok(InputEvent::FocusChange(focused)) => {
                                    emulator.internal.send_focus_event(focused);
                                }
                                Ok(InputEvent::ScrollOffset(offset)) => {
                                    emulator.set_gui_scroll_offset(offset);
                                }
                                Err(_) => {
                                    // GUI closed the sender — time to stop.
                                    info!("Input channel closed; consumer thread exiting");
                                    break;
                                }
                            }
                        }
                    }

                    // After processing each event, drain any window manipulation
                    // commands the emulator accumulated and forward them to the GUI.
                    let cmds: Vec<_> = emulator.internal.window_commands.drain(..).collect();
                    for cmd in cmds {
                        use freminal_common::buffer_states::window_manipulation::WindowManipulation;
                        let wc = match &cmd {
                            WindowManipulation::ReportWindowState
                            | WindowManipulation::ReportWindowPositionWholeWindow
                            | WindowManipulation::ReportWindowPositionTextArea
                            | WindowManipulation::ReportWindowSizeInPixels
                            | WindowManipulation::ReportWindowTextAreaSizeInPixels
                            | WindowManipulation::ReportRootWindowSizeInPixels
                            | WindowManipulation::ReportCharacterSizeInPixels
                            | WindowManipulation::ReportTerminalSizeInCharacters
                            | WindowManipulation::ReportRootWindowSizeInCharacters
                            | WindowManipulation::ReportIconLabel
                            | WindowManipulation::ReportTitle
                            | WindowManipulation::QueryClipboard(_) => WindowCommand::Report(cmd),
                            _ => WindowCommand::Viewport(cmd),
                        };
                        if let Err(e) = window_cmd_tx.send(wc) {
                            error!("Failed to send window command to GUI: {e}");
                        }
                    }

                    // Publish a fresh snapshot for the GUI to load lock-free.
                    let snap = emulator.build_snapshot();
                    arc_swap.store(Arc::new(snap));

                    // Notify egui that new content is available so it wakes up
                    // and renders the updated snapshot.  Cap the rate at ~120 fps
                    // to avoid flooding egui during heavy PTY output (e.g. `cat`
                    // of a large file).
                    if let Some(ctx) = egui_ctx_pty.get() {
                        ctx.request_repaint_after(std::time::Duration::from_millis(8));
                    }
                }
            });

            gui::run(
                arc_swap_gui,
                cfg,
                args.config,
                input_tx,
                pty_write_tx,
                window_cmd_rx,
                egui_ctx,
            )
        }
        Err(e) => {
            error!("Failed to create terminal emulator: {}", e);
            return;
        }
    };

    if let Err(e) = res {
        error!("Failed to run terminal emulator: {}", e);
    }

    info!("Shutting down freminal");
}
