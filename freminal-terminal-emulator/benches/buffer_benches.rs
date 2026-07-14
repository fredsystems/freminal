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
// Real-world colored scrollback corpora (Task 118 memory bench)
//
// These generate authentic ANSI byte streams that are fed through the REAL
// parser (`handle_incoming_data`), so the resulting `FormatTag`s are produced
// exactly as they would be from a live shell — not hand-constructed. They
// model realistic developer output where formatting is *sparse-to-moderate*
// (a few SGR runs per line), which is the common case scrollback compaction
// targets, as opposed to the synthetic `high_entropy_colored` worst case in
// the freminal-buffer harness.
// ---------------------------------------------------------------

/// SGR reset.
const SGR_RESET: &str = "\x1b[0m";

/// Colored build/compiler-style output (cargo/gcc flavour): mostly-default
/// lines with sparse SGR bursts — bold paths, red `error`, yellow `warning`,
/// green `ok`/`Compiling`, dim notes. A realistic dev scrollback: the majority
/// of lines carry only one or two format runs.
fn build_output_payload(lines: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(lines * 80);
    for i in 0..lines {
        let line = match i % 8 {
            0 => format!("\x1b[1;32m   Compiling\x1b[0m freminal-buffer v0.11.2 (crate {i})"),
            1 => format!("\x1b[1;32m   Compiling\x1b[0m freminal-common v0.11.2 (crate {i})"),
            2 => format!("\x1b[1;33mwarning\x1b[0m: unused variable: \x1b[1m`tmp_{i}`\x1b[0m"),
            3 => format!(
                "  \x1b[1;34m-->\x1b[0m src/buffer/mod.rs:{}:{}",
                i % 900,
                i % 80
            ),
            4 => format!("\x1b[1;31merror[E0308]\x1b[0m: mismatched types in expr {i}"),
            5 => "   |".to_string(),
            6 => format!("   = \x1b[1mnote\x1b[0m: expected `usize`, found `u32` (site {i})"),
            _ => format!("    Finished dev [unoptimized + debuginfo] target(s) in {i}.0s"),
        };
        out.extend_from_slice(line.as_bytes());
        out.extend_from_slice(SGR_RESET.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out
}

/// `ls --color` / `eza`-style colored file listing: each filename colored by
/// type (blue dirs, green executables, cyan symlinks, default regular files),
/// a few runs per line. Common interactive output that lands in scrollback.
fn ls_color_payload(lines: usize) -> Vec<u8> {
    // (sgr, name-suffix) by "file type"
    let kinds: &[(&str, &str)] = &[
        ("\x1b[1;34m", "/"), // directory (bold blue)
        ("\x1b[1;32m", "*"), // executable (bold green)
        ("\x1b[1;36m", "@"), // symlink (bold cyan)
        ("\x1b[0m", ""),     // regular file (default)
        ("\x1b[0m", ""),     // regular file (default) — weight toward plain
    ];
    let mut out = Vec::with_capacity(lines * 80);
    for i in 0..lines {
        // 4 colored entries per line, like a wide `ls` listing.
        for col in 0..4 {
            let (sgr, suffix) = kinds[(i + col) % kinds.len()];
            let entry = format!("{sgr}entry_{i}_{col}{suffix}\x1b[0m  ");
            out.extend_from_slice(entry.as_bytes());
        }
        out.extend_from_slice(b"\r\n");
    }
    out
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
                state.set_win_size(80, 24, 8, 16);
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
    state.set_win_size(80, 24, 8, 16);
    let payload = cup_writes_payload(80, 24);
    let parsed = state.parser.push(&payload);
    state.handler.process_outputs(&parsed);

    let mut group = c.benchmark_group("bench_data_and_format_for_gui");
    group.throughput(Throughput::Elements((80 * 24) as u64));

    group.bench_function("flatten_80x24", |b| {
        b.iter(|| {
            std::hint::black_box(state.handler.data_and_format_data_for_gui(0));
        });
    });

    group.finish();
}

// ---------------------------------------------------------------
// bench_build_snapshot — snapshot production cost on pre-populated emulator
//
// Two sub-benchmarks:
//
//   build_snapshot_80x24_dirty
//     Measures the *dirty path*: every iteration re-marks all visible rows
//     as dirty (by re-processing the fill payload) before calling
//     `build_snapshot`.  This reflects the worst-case cost — a full screen
//     repaint after every PTY data batch.
//
//   build_snapshot_80x24_clean
//     Measures the *clean path*: `build_snapshot` is called on an emulator
//     whose rows are all clean (no mutation since the last snapshot).  This
//     reflects the typical cost when the PTY is idle and the GUI polls for
//     a fresh snapshot — the cached vectors are simply cloned.
// ---------------------------------------------------------------
fn bench_build_snapshot(c: &mut Criterion) {
    let mut group = c.benchmark_group("bench_build_snapshot");
    group.throughput(Throughput::Elements((80 * 24) as u64));

    // ── Dirty path ──────────────────────────────────────────────────────────
    // Re-feed the fill payload before every snapshot call so every visible
    // row is dirty.  Uses iter_batched to set up state per-sample.
    group.bench_function("build_snapshot_80x24_dirty", |b| {
        b.iter_batched(
            || {
                let mut emulator = TerminalEmulator::dummy_for_bench();
                emulator.internal.set_win_size(80, 24, 8, 16);
                let payload = cup_writes_payload(80, 24);
                let parsed = emulator.internal.parser.push(&payload);
                emulator.internal.handler.process_outputs(&parsed);
                emulator
            },
            |mut emulator| {
                std::hint::black_box(emulator.build_snapshot());
            },
            criterion::BatchSize::SmallInput,
        );
    });

    // ── Clean path ──────────────────────────────────────────────────────────
    // Build one snapshot to warm the cache (marking all rows clean), then
    // measure repeated calls where nothing has changed.
    {
        let mut emulator = TerminalEmulator::dummy_for_bench();
        emulator.internal.set_win_size(80, 24, 8, 16);
        let payload = cup_writes_payload(80, 24);
        let parsed = emulator.internal.parser.push(&payload);
        emulator.internal.handler.process_outputs(&parsed);
        // Warm the cache: first build_snapshot marks all rows clean.
        let _ = emulator.build_snapshot();

        group.bench_function("build_snapshot_80x24_clean", |b| {
            b.iter(|| {
                std::hint::black_box(emulator.build_snapshot());
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------
// bench_build_snapshot_with_scrollback — snapshot cost with large scrollback
//
// Pre-populates 10,000 rows of scrollback, then measures snapshot building.
// The visible window is 80×24 at the bottom, but Arc<Vec<Row>> wraps all rows.
// ---------------------------------------------------------------
fn bench_build_snapshot_with_scrollback(c: &mut Criterion) {
    let mut group = c.benchmark_group("bench_build_snapshot_with_scrollback");
    group.throughput(Throughput::Elements((80 * 24) as u64));

    // Build a payload that produces ~10,000 scrollback rows.
    // Each line: 80 chars + \n = ~81 bytes. 10024 lines → 10000 scrollback + 24 visible.
    let mut payload = Vec::with_capacity(10_024 * 81);
    for row in 0..10_024 {
        for col in 0..80 {
            payload.push(b'a' + ((row + col) % 26) as u8);
        }
        payload.push(b'\n');
    }

    // ── Dirty path (first snapshot after full data feed) ────────────────────
    group.bench_function("snapshot_10k_scrollback_dirty", |b| {
        b.iter_batched(
            || {
                let mut emulator = TerminalEmulator::dummy_for_bench();
                emulator.internal.set_win_size(80, 24, 8, 16);
                emulator.internal.handle_incoming_data(&payload);
                emulator
            },
            |mut emulator| {
                std::hint::black_box(emulator.build_snapshot());
            },
            criterion::BatchSize::LargeInput,
        );
    });

    // ── Clean path (snapshot already built, no changes) ─────────────────────
    {
        let mut emulator = TerminalEmulator::dummy_for_bench();
        emulator.internal.set_win_size(80, 24, 8, 16);
        emulator.internal.handle_incoming_data(&payload);
        // Warm cache
        let _ = emulator.build_snapshot();

        group.bench_function("snapshot_10k_scrollback_clean", |b| {
            b.iter(|| {
                std::hint::black_box(emulator.build_snapshot());
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------
// bench_scrollback_memory_realworld — resident memory of a large scrollback
// filled with REAL colored output, fed through the real ANSI parser.
//
// Reports bytes-per-scrollback-line in two labelled states:
//   * steady-state   : what build_snapshot does (visible flatten only) — the
//                       state the terminal is in ~always.
//   * post-search     : after a full-scrollback flatten (the Ctrl-F /
//                       RequestSearchBuffer path) — the transient worst case.
//
// This is a *reporting* bench (prints B/line); it also registers a trivial
// timing loop so it participates in `cargo bench` normally.
// ---------------------------------------------------------------
fn bench_scrollback_memory_realworld(c: &mut Criterion) {
    const LINES: usize = 5_000;

    type CorpusGen = fn(usize) -> Vec<u8>;
    let corpora: &[(&str, CorpusGen)] = &[
        ("build_output", build_output_payload),
        ("ls_color", ls_color_payload),
    ];

    for (label, generate) in corpora {
        let payload = generate(LINES);

        // Build a fresh emulator, feed the corpus through the REAL parser in
        // CHUNKS with a `build_snapshot` between chunks. This models real
        // streaming output: the GUI renders every frame while data arrives, so
        // `build_snapshot` (→ `visible_as_tchars_and_tags`) runs repeatedly as
        // rows scroll through the visible window, leaving each row with a
        // populated `RowCacheEntry` that is NOT cleared on scroll-out. Feeding
        // the whole payload in one shot with a single final snapshot never
        // reproduces that per-row scrollback-cache accumulation (the ~180 MB
        // stale cache a 100k-line scrollback showed in the live app), giving
        // falsely optimistic numbers. ~4 KB per chunk approximates a PTY read
        // burst.
        const CHUNK_BYTES: usize = 4096;
        let build = || {
            let mut emulator = TerminalEmulator::dummy_for_bench();
            emulator.internal.set_win_size(80, 24, 8, 16);
            for chunk in payload.chunks(CHUNK_BYTES) {
                emulator.internal.handle_incoming_data(chunk);
                // Model a rendered frame after each read burst.
                let _ = emulator.build_snapshot();
            }
            emulator
        };

        // (1) Fresh fill, BEFORE idle compaction — compaction is deferred off
        //     the hot path (Task 118.9), so nothing is compact yet. This is the
        //     transient state for ~250ms right after a burst of output.
        let mut fresh = build();
        let _ = fresh.build_snapshot();
        let fresh_bpl = report_emulator_memory(label, "fresh fill, pre-idle (transient)", &fresh);

        // (2) SETTLED steady state — idle compaction has run to completion (as
        //     the PTY idle tick would ~250ms after output stops), then the
        //     render path warms the visible window. THIS is the real win.
        let mut settled = build();
        let _ = settled
            .internal
            .handler
            .buffer_mut()
            .compact_idle_scrollback(usize::MAX);
        let _ = settled.build_snapshot();
        let settled_bpl = report_emulator_memory(
            label,
            "settled steady-state (post-idle-compaction)",
            &settled,
        );

        // (3) SETTLED + post-search: Ctrl-F full-scrollback flatten on the
        //     settled buffer; Task 118.4 eviction reclaims the transient
        //     copies, returning to ~the settled number.
        let mut searched = build();
        let _ = searched
            .internal
            .handler
            .buffer_mut()
            .compact_idle_scrollback(usize::MAX);
        let _ = searched.build_snapshot();
        let _ = searched.internal.handler.data_and_format_data_for_gui(0);
        let search_bpl = report_emulator_memory(label, "settled + post-search (Ctrl-F)", &searched);

        println!(
            " -> {label}: fresh(pre-idle) {fresh_bpl} B/line, settled {settled_bpl} B/line, \
             settled+search {search_bpl} B/line\n"
        );

        let measured = build();
        c.bench_function(&format!("scrollback_memory_realworld_{label}"), |b| {
            b.iter(|| {
                std::hint::black_box(measured.internal.handler.buffer().heap_bytes());
            });
        });
    }
}

/// Print one labelled memory report block for an emulator's buffer and return
/// bytes-per-scrollback-line.
fn report_emulator_memory(label: &str, state: &str, emulator: &TerminalEmulator) -> usize {
    let breakdown = emulator.internal.handler.buffer().heap_bytes();
    let total = breakdown.rows_bytes + breakdown.row_cache_bytes + breakdown.url_bytes;
    let bpl = total.checked_div(breakdown.scrollback_lines).unwrap_or(0);

    println!("==================================================================");
    println!(" Realworld scrollback memory: {label}  [{state}]");
    println!("------------------------------------------------------------------");
    println!(" scrollback_lines    : {}", breakdown.scrollback_lines);
    println!(" rows_bytes          : {}", breakdown.rows_bytes);
    println!(" row_cache_bytes     : {}", breakdown.row_cache_bytes);
    println!(" url_bytes           : {}", breakdown.url_bytes);
    println!(" total_bytes         : {total}");
    println!(" bytes/scrollback_ln : {bpl}");
    println!("==================================================================");

    bpl
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
        bench_build_snapshot_with_scrollback,
        bench_scrollback_memory_realworld,
);

criterion_main!(benches);
