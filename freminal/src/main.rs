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
use std::sync::Arc;
use tracing::Level;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{
    EnvFilter,
    filter::Directive,
    fmt::{self, layer},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

pub mod gui;

use freminal_common::{args::Args, config::load_config};

use clap::Parser;

#[allow(clippy::too_many_lines)]
fn main() {
    // use env for filtering
    // example
    // RUST_LOG=none,freminal=debug cargo run

    let args = Args::parse();

    // ── 1. Load config and apply CLI overrides ──────────────────────────
    // Config must be loaded before logging setup so that the merged
    // `write_logs_to_file` value (CLI > TOML > default) can control
    // whether log files are created.
    let mut cfg = match load_config(args.config.as_deref()) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Error: failed to load config: {err:#}");
            std::process::exit(1);
        }
    };

    cfg.apply_cli_overrides(args.shell.as_deref(), args.write_logs_to_file);

    // ── 2. Set up logging ───────────────────────────────────────────────
    let env_filter = if args.show_all_debug {
        EnvFilter::builder()
            .with_default_directive(Level::INFO.into())
            .from_env_lossy()
    } else {
        let winit_directive: Directive = match "winit=off".parse() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Failed to parse directive: {e}");
                std::process::exit(1);
            }
        };

        let wgpu_directive: Directive = match "wgpu=off".parse() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Failed to parse directive: {e}");
                std::process::exit(1);
            }
        };

        let eframe_directive: Directive = match "eframe=off".parse() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Failed to parse directive: {e}");
                std::process::exit(1);
            }
        };

        let egui_directive: Directive = match "egui=off".parse() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Failed to parse directive: {e}");
                std::process::exit(1);
            }
        };

        EnvFilter::builder()
            .with_default_directive(Level::INFO.into())
            .from_env_lossy()
            .add_directive(winit_directive)
            .add_directive(wgpu_directive)
            .add_directive(eframe_directive)
            .add_directive(egui_directive)
    };

    let subscriber = tracing_subscriber::registry().with(env_filter);

    if cfg.write_logs_to_file() {
        let std_out_layer = layer()
            .with_line_number(true)
            .with_span_events(fmt::format::FmtSpan::ACTIVE)
            .compact();

        let file_appender = match RollingFileAppender::builder()
            .rotation(Rotation::HOURLY)
            .max_log_files(2)
            .filename_prefix("freminal")
            .filename_suffix("log")
            .build("./")
        {
            Ok(appender) => appender,
            Err(e) => {
                eprintln!("Failed to create file appender: {e}");
                return;
            }
        };
        subscriber
            .with(layer().with_ansi(false).with_writer(file_appender))
            .with(std_out_layer)
            .init();
    } else {
        let std_out_layer = layer()
            .with_line_number(true)
            .with_span_events(fmt::format::FmtSpan::ACTIVE)
            .compact();

        subscriber.with(std_out_layer).init();
    }

    info!("Starting freminal");
    debug!("Loaded config: {:#?}", cfg);

    // Propagate the merged shell path back into args so that
    // TerminalEmulator::new (which reads args.shell) gets the effective
    // value from the CLI > TOML > default precedence chain.
    let mut args = args;
    if args.shell.is_none() {
        args.shell = cfg.shell_path().map(String::from);
    }

    let res = match TerminalEmulator::new(&args) {
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
                            | WindowManipulation::ReportTitle => WindowCommand::Report(cmd),
                            _ => WindowCommand::Viewport(cmd),
                        };
                        if let Err(e) = window_cmd_tx.send(wc) {
                            error!("Failed to send window command to GUI: {e}");
                        }
                    }

                    // Publish a fresh snapshot for the GUI to load lock-free.
                    let snap = emulator.build_snapshot();
                    arc_swap.store(Arc::new(snap));
                }
            });

            gui::run(arc_swap_gui, cfg, input_tx, pty_write_tx, window_cmd_rx)
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
