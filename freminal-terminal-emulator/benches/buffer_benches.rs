// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Benchmarks for the terminal emulator's parser and processing pipeline.
//!
//! Covers:
//! - `FreminalAnsiParser::push()` in isolation (plain text and SGR-heavy)
//! - Parser + handler together (CUP-writes, bursty PTY output)
//! - Full `handle_incoming_data()` (includes UTF-8 reassembly overhead)
//! - `data_and_format_data_for_gui()` on a pre-populated handler
//! - `build_snapshot()` on a pre-populated emulator

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use freminal_terminal_emulator::{
    ansi::FreminalAnsiParser, interface::TerminalEmulator, state::internal::TerminalState,
};
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
// Payloads
// ---------------------------------------------------------------

/// 4 096 bytes of plain ASCII text with no escape sequences.
fn plain_text_payload() -> Vec<u8> {
    let line = b"Hello, world! This is a plain ASCII line with no escapes.\n";
    let mut out = Vec::with_capacity(4096);
    while out.len() < 4096 {
        let remaining = 4096 - out.len();
        out.extend_from_slice(&line[..line.len().min(remaining)]);
    }
    out.truncate(4096);
    out
}

/// ~4 KB payload dense with SGR color-change escape sequences.
/// Each segment is: ESC[38;2;R;G;Bm + 8 ASCII chars.
fn sgr_heavy_payload() -> Vec<u8> {
    let mut out = Vec::with_capacity(4096);
    let colors: &[(u8, u8, u8)] = &[
        (255, 0, 0),
        (0, 255, 0),
        (0, 0, 255),
        (255, 255, 0),
        (0, 255, 255),
        (255, 0, 255),
        (128, 128, 0),
        (0, 128, 128),
    ];
    let mut i = 0usize;
    while out.len() < 4096 {
        let (r, g, b) = colors[i % colors.len()];
        let esc = format!("\x1b[38;2;{r};{g};{b}m");
        out.extend_from_slice(esc.as_bytes());
        out.extend_from_slice(b"abcdefgh");
        i += 1;
    }
    out
}

/// ~4 KB payload simulating a TUI screen draw: ESC[row;colH + line data.
fn cup_writes_payload(width: usize, height: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for row in 1..=height {
        let esc = format!("\x1b[{row};1H");
        out.extend_from_slice(esc.as_bytes());
        let line: Vec<u8> = (0..width).map(|i| b'a' + (i % 26) as u8).collect();
        out.extend_from_slice(&line);
    }
    out
}

/// Bursty payload: 10 small chunks (< 100 bytes) followed by one 4 096-byte chunk.
fn bursty_payload() -> Vec<Vec<u8>> {
    let mut chunks = Vec::new();
    // 10 small chunks
    for i in 0u8..10 {
        let small: Vec<u8> = (0..((i % 7) + 3) as usize)
            .map(|j| b'a' + ((i as usize + j) % 26) as u8)
            .collect();
        chunks.push(small);
    }
    // one large chunk
    chunks.push(plain_text_payload());
    chunks
}

// ---------------------------------------------------------------
// bench_parse_plain_text
// ---------------------------------------------------------------
fn bench_parse_plain_text(c: &mut Criterion) {
    let payload = plain_text_payload();

    let mut group = c.benchmark_group("bench_parse_plain_text");
    group.throughput(Throughput::Bytes(payload.len() as u64));

    group.bench_function(BenchmarkId::new("parser_push", payload.len()), |b| {
        b.iter_batched(
            FreminalAnsiParser::new,
            |mut parser| {
                std::hint::black_box(parser.push(&payload));
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// bench_parse_sgr_heavy
// ---------------------------------------------------------------
fn bench_parse_sgr_heavy(c: &mut Criterion) {
    let payload = sgr_heavy_payload();

    let mut group = c.benchmark_group("bench_parse_sgr_heavy");
    group.throughput(Throughput::Bytes(payload.len() as u64));

    group.bench_function(BenchmarkId::new("parser_push_sgr", payload.len()), |b| {
        b.iter_batched(
            FreminalAnsiParser::new,
            |mut parser| {
                std::hint::black_box(parser.push(&payload));
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// bench_parse_cup_writes — parser + handler together
// ---------------------------------------------------------------
fn bench_parse_cup_writes(c: &mut Criterion) {
    let payload = cup_writes_payload(80, 24);

    let mut group = c.benchmark_group("bench_parse_cup_writes");
    group.throughput(Throughput::Bytes(payload.len() as u64));

    group.bench_function("parse_and_handle_80x24", |b| {
        b.iter_batched(
            || {
                let mut state = TerminalState::default();
                // Pre-resize to 80×24 so the handler dimensions match the payload.
                state.set_win_size(80, 24);
                state
            },
            |mut state| {
                let parsed = state.parser.push(&payload);
                state.handler.process_outputs(&parsed);
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// bench_parse_bursty — bursty PTY output pattern
// ---------------------------------------------------------------
fn bench_parse_bursty(c: &mut Criterion) {
    let chunks = bursty_payload();
    let total_bytes: usize = chunks.iter().map(Vec::len).sum();

    let mut group = c.benchmark_group("bench_parse_bursty");
    group.throughput(Throughput::Bytes(total_bytes as u64));

    group.bench_function("bursty_10_small_plus_1_large", |b| {
        b.iter_batched(
            TerminalState::default,
            |mut state| {
                for chunk in &chunks {
                    let parsed = state.parser.push(chunk);
                    state.handler.process_outputs(&parsed);
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// bench_handle_incoming_data — full path including UTF-8 reassembly
// ---------------------------------------------------------------
fn bench_handle_incoming_data(c: &mut Criterion) {
    let payload = plain_text_payload();

    let mut group = c.benchmark_group("bench_handle_incoming_data");
    group.throughput(Throughput::Bytes(payload.len() as u64));

    group.bench_function("handle_incoming_data_4096", |b| {
        b.iter_batched(
            TerminalState::default,
            |mut state| {
                state.handle_incoming_data(&payload);
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// bench_data_and_format_for_gui — flatten cost on pre-populated handler
// ---------------------------------------------------------------
fn bench_data_and_format_for_gui(c: &mut Criterion) {
    // Pre-populate with a full 80×24 screen of content.
    let mut state = TerminalState::default();
    state.set_win_size(80, 24);
    let payload = cup_writes_payload(80, 24);
    let parsed = state.parser.push(&payload);
    state.handler.process_outputs(&parsed);

    let mut group = c.benchmark_group("bench_data_and_format_for_gui");
    group.throughput(Throughput::Elements((80 * 24) as u64));

    group.bench_function("flatten_80x24", |b| {
        b.iter(|| {
            std::hint::black_box(state.handler.data_and_format_data_for_gui());
        });
    });

    group.finish();
}

// ---------------------------------------------------------------
// bench_build_snapshot — snapshot production cost on pre-populated emulator
// ---------------------------------------------------------------
fn bench_build_snapshot(c: &mut Criterion) {
    // Build a dummy emulator and fill it with a full 80×24 screen.
    let mut emulator = TerminalEmulator::dummy_for_bench();
    emulator.internal.set_win_size(80, 24);
    let payload = cup_writes_payload(80, 24);
    let parsed = emulator.internal.parser.push(&payload);
    emulator.internal.handler.process_outputs(&parsed);

    let mut group = c.benchmark_group("bench_build_snapshot");
    group.throughput(Throughput::Elements((80 * 24) as u64));

    group.bench_function("build_snapshot_80x24", |b| {
        b.iter(|| {
            std::hint::black_box(emulator.build_snapshot());
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
        bench_parse_plain_text,
        bench_parse_sgr_heavy,
        bench_parse_cup_writes,
        bench_parse_bursty,
        bench_handle_incoming_data,
        bench_data_and_format_for_gui,
        bench_build_snapshot,
);

criterion_main!(benches);
