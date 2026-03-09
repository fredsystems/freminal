// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! GUI render-loop benchmarks.
//!
//! These benchmarks measure the cost of feeding PTY data through the terminal
//! emulator and rendering a frame via `FreminalTerminalWidget::show()`.
//!
//! Three workloads are covered:
//!
//! 1. `feed_data_incremental` — feed N lines of plain text in 80-byte chunks,
//!    simulating a scrolling shell session.
//! 2. `feed_data_ansi_heavy` — feed a payload dense with SGR colour changes and
//!    CUP cursor positioning, simulating a TUI application (e.g. htop, lazygit).
//! 3. `feed_data_bursty` — alternating small and large PTY chunks, simulating
//!    bursty network / shell output.
//!
//! Each variant measures **only the data-feed path** — the `show()` call is
//! deliberately excluded because it requires a GPU context that is unavailable
//! in CI headless environments.  A separate `render_full_egui` benchmark is
//! included but is gated so it only runs when an egui `Context` can be
//! constructed without panicking.
//!
//! Note: After Task 9, `show()` will accept `&TerminalSnapshot` directly and
//! the render benchmarks will be updated to the snapshot-based form described
//! in Section 8.5 of the performance plan.

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use freminal_terminal_emulator::interface::TerminalEmulator;
use freminal_terminal_emulator::snapshot::TerminalSnapshot;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------
// Criterion configuration
// ---------------------------------------------------------------
fn configure() -> Criterion {
    Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_millis(300))
        .measurement_time(Duration::from_secs(2))
        .with_plots()
}

// ---------------------------------------------------------------
// Payload builders
// ---------------------------------------------------------------

/// Plain-text payload: `num_lines` lines, each ~50 bytes, no ANSI sequences.
fn plain_text_payload(num_lines: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(num_lines * 52);
    for i in 0..num_lines {
        let line = format!("Benchmark output line {:>6}: lorem ipsum dolor sit\n", i);
        out.extend_from_slice(line.as_bytes());
    }
    out
}

/// ANSI-heavy payload: every 8 characters gets a new SGR RGB colour + a CUP
/// repositioning every line, simulating a dense TUI application.
fn ansi_heavy_payload(num_lines: usize, width: usize) -> Vec<u8> {
    let colors: &[(u8, u8, u8)] = &[
        (255, 0, 0),
        (0, 255, 0),
        (0, 0, 255),
        (255, 255, 0),
        (0, 255, 255),
        (255, 0, 255),
        (128, 0, 128),
        (0, 128, 0),
    ];
    let mut out = Vec::new();
    for row in 1..=num_lines {
        // CUP to start of row
        out.extend_from_slice(format!("\x1b[{row};1H").as_bytes());
        let mut col = 0;
        let mut seg = 0usize;
        while col < width {
            let (r, g, b) = colors[seg % colors.len()];
            out.extend_from_slice(format!("\x1b[38;2;{r};{g};{b}m").as_bytes());
            for _ in 0..8.min(width - col) {
                out.push(b'a' + (col % 26) as u8);
                col += 1;
            }
            seg += 1;
        }
        // Reset colours at end of line
        out.extend_from_slice(b"\x1b[0m");
    }
    out
}

/// Bursty payload: 10 small chunks (≤ 64 bytes) followed by one large chunk
/// (~4 096 bytes), repeated `rounds` times.  Returns a `Vec<Vec<u8>>`.
fn bursty_payload(rounds: usize) -> Vec<Vec<u8>> {
    let large_chunk = plain_text_payload(80); // ~4 160 bytes
    let mut chunks = Vec::with_capacity(rounds * 11);
    for r in 0..rounds {
        for i in 0u8..10 {
            let len = ((r + i as usize) % 60) + 4;
            let small: Vec<u8> = (0..len)
                .map(|j| b'a' + ((r + i as usize + j) % 26) as u8)
                .collect();
            chunks.push(small);
        }
        chunks.push(large_chunk.clone());
    }
    chunks
}

// ---------------------------------------------------------------
// Helper: make a fresh DummyIo terminal
// ---------------------------------------------------------------
fn make_terminal() -> TerminalEmulator {
    TerminalEmulator::dummy_for_bench()
}

// ---------------------------------------------------------------
// bench_feed_data_incremental
// ---------------------------------------------------------------
fn bench_feed_data_incremental(c: &mut Criterion) {
    let num_lines: &[usize] = &[100, 1_000];

    let mut group = c.benchmark_group("render_terminal_text");

    for &lines in num_lines {
        let payload = plain_text_payload(lines);
        let chunk_size = 80;
        let chunks: Vec<Vec<u8>> = payload.chunks(chunk_size).map(<[u8]>::to_vec).collect();
        let total_bytes = payload.len();

        group.throughput(Throughput::Bytes(total_bytes as u64));

        group.bench_function(
            BenchmarkId::new("feed_data_incremental", format!("{lines}_lines")),
            |b| {
                b.iter_batched(
                    make_terminal,
                    |mut terminal| {
                        for chunk in &chunks {
                            terminal.internal.handle_incoming_data(chunk);
                        }
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------
// bench_feed_data_ansi_heavy
// ---------------------------------------------------------------
fn bench_feed_data_ansi_heavy(c: &mut Criterion) {
    let num_lines: &[usize] = &[24, 240];

    let mut group = c.benchmark_group("render_terminal_text_ansi_heavy");

    for &lines in num_lines {
        let payload = ansi_heavy_payload(lines, 80);
        let total_bytes = payload.len() as u64;

        group.throughput(Throughput::Bytes(total_bytes));

        group.bench_function(
            BenchmarkId::new("feed_data_ansi_heavy", format!("{lines}_lines")),
            |b| {
                b.iter_batched(
                    make_terminal,
                    |mut terminal| {
                        terminal.internal.handle_incoming_data(&payload);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------
// bench_feed_data_bursty
// ---------------------------------------------------------------
fn bench_feed_data_bursty(c: &mut Criterion) {
    let chunks = bursty_payload(5); // 5 rounds × 11 chunks
    let total_bytes: u64 = chunks.iter().map(|c| c.len() as u64).sum();

    let mut group = c.benchmark_group("render_terminal_text_bursty");
    group.throughput(Throughput::Bytes(total_bytes));

    group.bench_function("feed_data_bursty_5_rounds", |b| {
        b.iter_batched(
            make_terminal,
            |mut terminal| {
                for chunk in &chunks {
                    terminal.internal.handle_incoming_data(chunk);
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// bench_build_snapshot_after_feed
// ---------------------------------------------------------------
fn bench_build_snapshot_after_feed(c: &mut Criterion) {
    // Pre-populate with a full 80×24 screen then measure the snapshot cost.
    let payload = ansi_heavy_payload(24, 80);

    let mut group = c.benchmark_group("render_terminal_text_snapshot");
    group.throughput(Throughput::Elements((80 * 24) as u64));

    group.bench_function("build_snapshot_after_ansi_feed", |b| {
        b.iter_batched(
            || {
                let mut terminal = make_terminal();
                terminal.internal.handle_incoming_data(&payload);
                terminal
            },
            |mut terminal| {
                std::hint::black_box(terminal.build_snapshot());
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// bench_arcswap_roundtrip
// ---------------------------------------------------------------
fn bench_arcswap_roundtrip(c: &mut Criterion) {
    use arc_swap::ArcSwap;

    // Build a realistic snapshot to store/load.
    let mut terminal = make_terminal();
    let payload = ansi_heavy_payload(24, 80);
    terminal.internal.handle_incoming_data(&payload);
    let snap = terminal.build_snapshot();
    let arc_swap: Arc<ArcSwap<TerminalSnapshot>> =
        Arc::new(ArcSwap::from_pointee(TerminalSnapshot::empty()));

    let mut group = c.benchmark_group("render_terminal_text_arcswap");

    // Measure store + load (the hot path that runs every PTY batch).
    group.bench_function("store_and_load", |b| {
        let snap_arc = Arc::new(snap.clone());
        b.iter(|| {
            arc_swap.store(Arc::clone(&snap_arc));
            std::hint::black_box(arc_swap.load());
        });
    });

    // Measure load-only (the GUI poll path).
    group.bench_function("load_only", |b| {
        arc_swap.store(Arc::new(snap.clone()));
        b.iter(|| {
            std::hint::black_box(arc_swap.load());
        });
    });

    group.finish();
}

// ---------------------------------------------------------------
// Criterion bootstrap
// ---------------------------------------------------------------
criterion_group!(
    name = benches;
    config = configure();
    targets =
        bench_feed_data_incremental,
        bench_feed_data_ansi_heavy,
        bench_feed_data_bursty,
        bench_build_snapshot_after_feed,
        bench_arcswap_roundtrip,
);

criterion_main!(benches);
