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
//! GUI Thread (eframe update() — pure render, no mutation)
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
pub mod playback;

use freminal_common::{args::Args, config, config::load_config, themes};

use clap::Parser;

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

    let is_playback = args.playback.is_some();

    let res = if let Some(ref playback_path) = args.playback {
        // ── Playback mode ───────────────────────────────────────────
        let file_data = match std::fs::read(playback_path) {
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
            TerminalEmulator::new_for_playback(Some(cfg.scrollback.limit));

        // Apply the configured theme.
        let theme = themes::by_slug(&cfg.theme.name).unwrap_or(&themes::CATPPUCCIN_MOCHA);
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

        let egui_ctx: Arc<OnceLock<eframe::egui::Context>> = Arc::new(OnceLock::new());
        let egui_ctx_playback = Arc::clone(&egui_ctx);

        std::thread::spawn(move || {
            playback::run_playback_thread(
                terminal,
                frames,
                input_rx,
                window_cmd_tx,
                arc_swap,
                egui_ctx_playback,
                clipboard_tx,
            );
        });

        gui::run(
            arc_swap_gui,
            cfg,
            args.config,
            input_tx,
            pty_write_tx,
            window_cmd_rx,
            clipboard_rx,
            egui_ctx,
            is_playback,
        )
    } else {
        // ── Normal mode ─────────────────────────────────────────────
        match TerminalEmulator::new(&args, Some(cfg.scrollback.limit)) {
            Ok((mut terminal, pty_read_rx)) => {
                // Apply the configured theme to the emulator so all snapshots carry
                // the correct palette from the start.
                let theme = themes::by_slug(&cfg.theme.name).unwrap_or(&themes::CATPPUCCIN_MOCHA);
                terminal.internal.handler.set_theme(theme);

                // Shared snapshot published by the PTY thread, consumed lock-free by the GUI.
                let arc_swap: Arc<ArcSwap<TerminalSnapshot>> =
                    Arc::new(ArcSwap::from_pointee(TerminalSnapshot::empty()));
                let arc_swap_gui = Arc::clone(&arc_swap);

                // Clone the PTY write sender before the emulator is moved into the
                // consumer thread.  The GUI uses it to send Report* responses back
                // to the PTY without going through the emulator.
                let pty_write_tx = terminal.clone_write_tx();

                // Extract the child-exit receiver before the emulator is moved.
                // On Windows the ConPTY keeps the read pipe open after the child
                // exits, so the reader thread blocks indefinitely.  This channel
                // provides a reliable cross-platform shutdown signal.
                let child_exit_rx = terminal.child_exit_rx();

                // Channel for GUI → PTY-consumer thread events (resize, key, focus).
                let (input_tx, input_rx) = unbounded::<InputEvent>();

                // Channel for PTY-consumer thread → GUI (window manipulation commands).
                let (window_cmd_tx, window_cmd_rx) = unbounded::<WindowCommand>();

                // Channel for clipboard text extraction responses (PTY → GUI).
                // Bounded(1) acts as a oneshot: GUI sends ExtractSelection via
                // input_tx, PTY thread sends the extracted text back here.
                let (clipboard_tx, clipboard_rx) = crossbeam_channel::bounded::<String>(1);

                // Shared egui context handle so the PTY consumer thread can request
                // repaints after publishing new snapshots.  The GUI sets it during
                // `FreminalGui::new()`; the PTY thread reads it after each store.
                let egui_ctx: Arc<OnceLock<eframe::egui::Context>> = Arc::new(OnceLock::new());
                let egui_ctx_pty = Arc::clone(&egui_ctx);

                // The TerminalEmulator is fully owned by the PTY consumer thread.
                // No FairMutex. No shared lock.
                std::thread::spawn(move || {
                    let mut emulator = terminal;

                    // If the PTY I/O layer provided a child-exit receiver, use
                    // it; otherwise use a channel that never fires.  On Unix
                    // the PTY reader thread detects exit via read() == 0, but
                    // on Windows the ConPTY keeps the pipe open after the child
                    // exits, so this is the only reliable shutdown signal.
                    let child_exit = child_exit_rx.unwrap_or_else(crossbeam_channel::never::<()>);

                    // Helper closure: drain window commands, publish snapshot,
                    // request repaint.  Called after every event in both loops.
                    let post_event =
                        |emulator: &mut TerminalEmulator,
                         window_cmd_tx: &crossbeam_channel::Sender<WindowCommand>,
                         arc_swap: &ArcSwap<TerminalSnapshot>,
                         egui_ctx_pty: &OnceLock<eframe::egui::Context>| {
                            let cmds: Vec<_> =
                                emulator.internal.window_commands.drain(..).collect();
                            for cmd in cmds {
                                use freminal_common::buffer_states::window_manipulation::WindowManipulation;
                                let wc = match &cmd {
                                    WindowManipulation::ReportWindowState
                                    | WindowManipulation::ReportWindowPositionWholeWindow
                                    | WindowManipulation::ReportWindowPositionTextArea
                                    | WindowManipulation::ReportWindowSizeInPixels
                                    | WindowManipulation::ReportWindowTextAreaSizeInPixels
                                    | WindowManipulation::ReportRootWindowSizeInPixels
                                    | WindowManipulation::ReportIconLabel
                                    | WindowManipulation::ReportTitle
                                    | WindowManipulation::QueryClipboard(_) => {
                                        WindowCommand::Report(cmd)
                                    }
                                    _ => WindowCommand::Viewport(cmd),
                                };
                                if let Err(e) = window_cmd_tx.send(wc) {
                                    error!("Failed to send window command to GUI: {e}");
                                }
                            }

                            let snap = emulator.build_snapshot();
                            arc_swap.store(Arc::new(snap));

                            if let Some(ctx) = egui_ctx_pty.get() {
                                ctx.request_repaint_after(std::time::Duration::from_millis(8));
                            }
                        };

                    // Helper closure: process a single InputEvent.  Returns
                    // `false` if the input channel has closed (GUI exited).
                    let handle_input = |emulator: &mut TerminalEmulator,
                                        msg: Result<InputEvent, crossbeam_channel::RecvError>,
                                        clipboard_tx: &crossbeam_channel::Sender<String>|
                     -> bool {
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
                            Ok(InputEvent::ThemeChange(theme)) => {
                                emulator.internal.handler.set_theme(theme);
                            }
                            Ok(InputEvent::ExtractSelection {
                                start_row,
                                start_col,
                                end_row,
                                end_col,
                            }) => {
                                let text = emulator
                                    .extract_selection_text(start_row, start_col, end_row, end_col);
                                let _ = clipboard_tx.send(text);
                            }
                            Ok(InputEvent::PlaybackControl(_)) => {
                                // Playback commands are handled by the dedicated playback
                                // consumer thread, not the normal PTY consumer.  Ignore.
                            }
                            Err(_) => {
                                info!("Input channel closed; consumer thread exiting");
                                return false;
                            }
                        }
                        true
                    };

                    // Primary loop: service PTY reads, GUI input events, and
                    // child-exit signals.
                    loop {
                        crossbeam_channel::select! {
                            recv(pty_read_rx) -> msg => {
                                if let Ok(read) = msg {
                                    emulator.handle_incoming_data(
                                        &read.buf[0..read.read_amount],
                                    );
                                } else {
                                    // PTY read channel closed — the shell exited
                                    // and the reader thread returned (dropping its
                                    // Sender).  Signal the GUI to close cleanly.
                                    info!("PTY read channel closed; requesting GUI close");

                                    // Publish one final snapshot so the GUI shows
                                    // the complete output.
                                    post_event(&mut emulator, &window_cmd_tx, &arc_swap, &egui_ctx_pty);

                                    if let Some(ctx) = egui_ctx_pty.get() {
                                        ctx.send_viewport_cmd(
                                            eframe::egui::ViewportCommand::Close,
                                        );
                                    }
                                    return;
                                }
                            }
                            recv(input_rx) -> msg => {
                                if !handle_input(&mut emulator, msg, &clipboard_tx) {
                                    return;
                                }
                            }
                            recv(child_exit) -> _ => {
                                // The child process exited.  On Unix the PTY
                                // reader thread may still have buffered data
                                // that hasn't been sent yet (waitpid can return
                                // before the pipe is fully drained).  Drain any
                                // remaining PTY output with a short timeout so
                                // the final chunks are not lost.
                                //
                                // On Windows, ConPTY does not close the read
                                // pipe after child exit, so the timeout ensures
                                // we don't block forever.
                                info!("Child process exited; draining remaining PTY output");

                                let drain_deadline = std::time::Duration::from_millis(200);
                                while let Ok(read) = pty_read_rx.recv_timeout(drain_deadline) {
                                    emulator.handle_incoming_data(
                                        &read.buf[0..read.read_amount],
                                    );
                                }

                                info!("PTY drain complete; requesting GUI close");
                                post_event(&mut emulator, &window_cmd_tx, &arc_swap, &egui_ctx_pty);

                                if let Some(ctx) = egui_ctx_pty.get() {
                                    ctx.send_viewport_cmd(
                                        eframe::egui::ViewportCommand::Close,
                                    );
                                }
                                return;
                            }
                        }

                        post_event(&mut emulator, &window_cmd_tx, &arc_swap, &egui_ctx_pty);
                    }
                });

                gui::run(
                    arc_swap_gui,
                    cfg,
                    args.config,
                    input_tx,
                    pty_write_tx,
                    window_cmd_rx,
                    clipboard_rx,
                    egui_ctx,
                    is_playback,
                )
            }
            Err(e) => {
                error!("Failed to create terminal emulator: {}", e);
                return;
            }
        }
    };

    if let Err(e) = res {
        error!("Failed to run terminal emulator: {}", e);
    }

    info!("Shutting down freminal");
}
