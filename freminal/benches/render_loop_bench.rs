// Copyright (C) 2024–2026 Fred Clausen
// Use of this source code is governed by an MIT-style license
// that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Benchmarks GUI rendering performance in two stages:
//! 1. "logic_only" – tests terminal logic without running the egui pipeline.
//! 2. "full_egui"  – runs the full FreminalTerminalWidget::show() under a headless egui::Context.

use criterion::{Criterion, criterion_group, criterion_main};
use egui::{Context, Ui};
use freminal::gui::terminal::FreminalTerminalWidget;
use freminal_terminal_emulator::{interface::TerminalEmulator, io::DummyIo};
use std::io::Write;

/// Builds a dummy terminal once, ready to accept input.
fn make_empty_terminal() -> TerminalEmulator<DummyIo> {
    TerminalEmulator::dummy_for_bench()
}

pub fn render_loop_benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_terminal_text");
    let num_lines = 1_000;

    // Prepare shared objects reused across all benches
    let mut terminal = make_empty_terminal();
    let ctx = Context::default();
    let mut widget = FreminalTerminalWidget::new(&ctx, &freminal_common::config::Config::default());

    // ---------- 1. Feed Data (incremental) ----------
    //
    // Measures feeding the same 10 k lines in small chunks,
    // simulating incremental PTY output.
    let mut buf = Vec::new();
    for i in 0..num_lines {
        let _ = writeln!(buf, "Benchmark line {}\n", i);
    }

    // Split into ~80-byte chunks
    let chunk_size = 80;
    let chunks: Vec<&[u8]> = buf.chunks(chunk_size).collect();

    group.bench_function(
        format!("feed_data_incremental/{}k_lines", num_lines / 1000),
        |b| {
            b.iter(|| {
                for chunk in &chunks {
                    terminal.internal.handle_incoming_data(chunk);
                }
            });
        },
    );

    // ---------- 2. Logic Only ----------
    //
    // Simulates the fast logic pass without rendering.
    group.bench_function(format!("logic_only/{}k_lines", num_lines / 1000), |b| {
        b.iter(|| {
            let _ = terminal.needs_redraw();
        });
    });

    // ---------- 3. Full egui Render ----------
    //
    // Runs the complete FreminalTerminalWidget::show() path inside a headless egui frame.
    group.bench_function(format!("full_egui/{}k_lines", num_lines / 1000), |b| {
        b.iter(|| {
            let _ = ctx.run(Default::default(), |egui_ctx| {
                let _ = egui::CentralPanel::default().show(egui_ctx, |ui: &mut Ui| {
                    widget.show(ui, &mut terminal);
                });
            });
        });
    });

    group.finish();
}

criterion_group!(benches, render_loop_benchmarks);
criterion_main!(benches);
