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

use crossbeam_channel::unbounded;
use freminal_terminal_emulator::interface::TerminalEmulator;
use freminal_terminal_emulator::io::InputEvent;
use parking_lot::FairMutex;
use std::{process, sync::Arc};
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

#[allow(clippy::too_many_lines)]
fn main() {
    // use env for filtering
    // example
    // RUST_LOG=none,freminal=debug cargo run

    let args = Args::parse(std::env::args()).unwrap_or_else(|_| {
        process::exit(1);
    });

    let env_filter = if args.show_all_debug {
        EnvFilter::builder()
            .with_default_directive(Level::INFO.into())
            .from_env_lossy()
    } else {
        let winit_directive: Directive = match "winit=off".parse() {
            Ok(d) => d,
            Err(e) => {
                error!("Failed to parse directive: {}", e);
                std::process::exit(1);
            }
        };

        let wgpu_directive: Directive = match "wgpu=off".parse() {
            Ok(d) => d,
            Err(e) => {
                error!("Failed to parse directive: {}", e);
                std::process::exit(1);
            }
        };

        let eframe_directive: Directive = match "eframe=off".parse() {
            Ok(d) => d,
            Err(e) => {
                error!("Failed to parse directive: {}", e);
                std::process::exit(1);
            }
        };

        let egui_directive: Directive = match "egui=off".parse() {
            Ok(d) => d,
            Err(e) => {
                error!("Failed to parse directive: {}", e);
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

    if args.write_logs_to_file {
        let std_out_layer = layer()
            .with_line_number(true)
            .with_span_events(fmt::format::FmtSpan::ACTIVE)
            .compact();

        //let file_appender = tracing_appender::rolling::daily("./", "freminal.log");
        let file_appender = match RollingFileAppender::builder()
            .rotation(Rotation::HOURLY) // rotate log files once every hour
            .max_log_files(2)
            .filename_prefix("freminal") // log file names will be prefixed with `myapp.`
            .filename_suffix("log") // log file names will be suffixed with `.log`
            .build("./") // try to build an appender that stores log files in `/var/log`
             {
            Ok(appender) => appender,
            Err(e) => {
                error!("Failed to create file appender: {}", e);
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

    let cfg = match load_config(None) {
        Ok(cfg) => cfg,
        Err(err) => {
            error!("Failed to load config: {:#}", err);
            std::process::exit(1);
        }
    };

    debug!("Loaded config: {:#?}", cfg);

    let res = match TerminalEmulator::new(&args) {
        Ok((terminal, rx)) => {
            let terminal = Arc::new(FairMutex::new(terminal));
            let terminal_clone = Arc::clone(&terminal);

            // Channel for GUI → PTY-consumer thread events (resize, key, focus).
            // The GUI holds the Sender; the consumer thread holds the Receiver.
            let (input_tx, input_rx) = unbounded::<InputEvent>();

            std::thread::spawn(move || {
                loop {
                    // Drain any pending InputEvents first (non-blocking), then
                    // block on the next PTY read.  This keeps resize handling
                    // off the GUI lock while still being processed promptly.
                    while let Ok(event) = input_rx.try_recv() {
                        match event {
                            InputEvent::Resize(w, h, pw, ph) => {
                                terminal.lock().handle_resize_event(w, h, pw, ph);
                            }
                            InputEvent::Key(_) | InputEvent::FocusChange(_) => {
                                // Key and FocusChange are handled elsewhere for now;
                                // reserved for Task 8.
                            }
                        }
                    }

                    if let Ok(read) = rx.recv() {
                        terminal
                            .lock()
                            .handle_incoming_data(&read.buf[0..read.read_amount]);
                    }
                }
            });

            gui::run(terminal_clone, cfg, input_tx)
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
