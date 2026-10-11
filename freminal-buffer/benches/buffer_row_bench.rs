// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use criterion::measurement::WallTime;
use criterion::{
    BatchSize, BenchmarkGroup, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main,
};

use freminal_buffer::buffer::Buffer;
use freminal_buffer::compact_row::CompactRow;
use freminal_buffer::compressed_block::CompressedBlock;
use freminal_buffer::image_store::{
    AnimationControl, ImageProtocol, ImageSizeMode, ImageStore, InlineImage,
};
use freminal_buffer::row::Row;
use freminal_common::buffer_states::{
    cursor::StateColors,
    fonts::{FontDecorationFlags, FontWeight},
    format_tag::FormatTag,
    tchar::TChar,
};
use freminal_common::colors::TerminalColor;

use std::time::Duration;

// ---------------------------------------------------------------
// Criterion configuration: FAST RUNS
// ---------------------------------------------------------------
fn configure() -> Criterion {
    Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_millis(300))
        .measurement_time(Duration::from_secs(2))
        .with_plots()
}

// ---------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------

/// Generate `n` ASCII TChar values cycling through printable characters.
fn gen_ascii_tchars(n: usize) -> Vec<TChar> {
    (0..n)
        .map(|i| TChar::Ascii(b'a' + (i % 26) as u8))
        .collect()
}

/// Generate `n` TChar values with a newline inserted every `line_len` chars,
/// simulating a file with many short lines.
#[allow(dead_code)]
fn gen_line_tchars(n: usize, line_len: usize) -> Vec<TChar> {
    (0..n)
        .map(|i| {
            if i % line_len == line_len - 1 {
                TChar::NewLine
            } else {
                TChar::Ascii(b'a' + (i % 26) as u8)
            }
        })
        .collect()
}

/// Load benchmark data from the external fixture file when the `bench_fixtures`
/// feature is enabled. Falls back to inline generated data otherwise.
fn load_tchars_for_large_bench() -> Vec<TChar> {
    #[cfg(feature = "bench_fixtures")]
    {
        use std::fs::File;
        use std::io::Read;
        let mut file =
            File::open("../speed_tests/10000_lines.txt").expect("bench_fixtures file missing");
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).expect("read failed");
        buf.into_iter().map(TChar::from).collect()
    }
    #[cfg(not(feature = "bench_fixtures"))]
    {
        // ~500 KB inline substitute: 10 000 lines of 49 chars + newline
        gen_line_tchars(500_000, 50)
    }
}

// ---------------------------------------------------------------
// Benchmark: inserting a large Vec<TChar> in one go
// ---------------------------------------------------------------
fn bench_insert_large_line(c: &mut Criterion) {
    let data = load_tchars_for_large_bench();

    let mut group = c.benchmark_group("buffer_insert_large_line");
    group.throughput(Throughput::Elements(data.len() as u64));

    group.bench_function(BenchmarkId::new("insert_full", data.len()), |b| {
        b.iter(|| {
            let mut buf = Buffer::new(100, 80);
            buf.insert_text(&data);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: inserting in chunks
// ---------------------------------------------------------------
fn bench_insert_chunks(c: &mut Criterion) {
    let data = load_tchars_for_large_bench();
    let chunks: Vec<Vec<TChar>> = data.chunks(1000).map(<[TChar]>::to_vec).collect();

    let mut group = c.benchmark_group("buffer_insert_chunks");
    group.throughput(Throughput::Elements(data.len() as u64));

    group.bench_function(BenchmarkId::new("insert_chunks_1000", chunks.len()), |b| {
        b.iter(|| {
            let mut buf = Buffer::new(100, 80);
            for chunk in &chunks {
                buf.insert_text(chunk);
            }
        });
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: resizing (24.2 — uses iter_batched to separate setup from measurement)
// ---------------------------------------------------------------
fn bench_resize(c: &mut Criterion) {
    let data = load_tchars_for_large_bench();

    let mut group = c.benchmark_group("buffer_resize");

    group.bench_with_input(BenchmarkId::new("reflow_width", 40), &data, |b, data| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new(100, 80);
                buf.insert_text(data);
                buf
            },
            |mut buf| {
                std::hint::black_box(buf.set_size(40, 80, 0));
            },
            BatchSize::LargeInput,
        );
    });

    group.bench_with_input(BenchmarkId::new("shrink_height", 20), &data, |b, data| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new(100, 200);
                buf.insert_text(data);
                buf
            },
            |mut buf| {
                std::hint::black_box(buf.set_size(100, 20, 0));
            },
            BatchSize::LargeInput,
        );
    });

    // Height grow — the Task 113.1 path. Exercises the primary-buffer grow
    // branch, which now reclaims trailing blank screen-padding below the live
    // cursor instead of appending an unreclaimable blank tail.
    group.bench_with_input(BenchmarkId::new("grow_height", 200), &data, |b, data| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new(100, 80);
                buf.insert_text(data);
                buf
            },
            |mut buf| {
                std::hint::black_box(buf.set_size(100, 200, 0));
            },
            BatchSize::LargeInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: extreme softwrap behavior
// ---------------------------------------------------------------
fn bench_softwrap_heavy(c: &mut Criterion) {
    let long_line = "a".repeat(5000);
    let data: Vec<TChar> = long_line.chars().map(TChar::from).collect();

    let mut group = c.benchmark_group("softwrap_heavy");

    group.bench_function("wrap_long_line_to_width_10", |b| {
        b.iter(|| {
            let mut buf = Buffer::new(100, 80);
            buf.insert_text(&data);
            buf.set_size(10, 80, 0);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmarks: line-feed eviction at the real default scrollback capacity
// (Task 125.12).
//
// Task 125.10 found that once scrollback is full, every line feed runs
// `Buffer::enforce_scrollback_limit`, which front-drains `rows`, `row_cache`
// and `row_block_map` (a memmove of ~10 000 entries per line feed) and then
// scans the whole block map / every live cell. These benches measure that
// steady-state eviction path at the DEFAULT 10 000-row limit (see
// `Buffer::new` in `buffer/lifecycle.rs`), so the Task 125 `RowStore`
// remediation (125.13-125.16) can be compared before/after under the named
// Criterion baseline `before_125_rowstore`.
//
// They use only public `Buffer` methods (`insert_text`, `handle_cr`,
// `handle_lf`, the prompt / command-block marks, `place_image`, and the idle
// compaction / compression entry points) and never touch `rows`, `row_cache`
// or `row_block_map`, so they compile unchanged across the RowStore refactor.
//
// Geometry is the 124x31 grid used by the Task 125.10 matched-protocol
// capture. The timed unit is `EVICTION_LF_BURST` (200) lines, each a short
// text insert followed by CR + LF (a real PTY's `\r\n` after ONLCR) — the
// size of one `seq 1 200` style burst. Every LF in the burst evicts exactly
// one row because the buffer already sits at `height + limit` rows.
//
// Harness choice: every scenario builds a FRESH buffer per iteration in the
// untimed setup (`iter_batched_ref` + `BatchSize::PerIteration`) and the
// buffer is dropped outside the timed span. A single reused buffer would stay
// at capacity indefinitely, but it would not stay in the scenario's state: the
// compressed blocks, the image and the prompt marks all scroll off the top
// after enough bursts (each burst evicts 200 rows), silently degenerating
// every scenario into `plain`. Rebuilding per iteration keeps all four
// scenarios and the scaling sweep on one identical methodology, at the cost of
// wall-clock time spent in (untimed) setup.
// ---------------------------------------------------------------

/// Width of the matched-protocol Freminal grid (Task 125.10).
const EVICTION_WIDTH: usize = 124;

/// Height of the matched-protocol Freminal grid (Task 125.10).
const EVICTION_HEIGHT: usize = 31;

/// Compiled-in default scrollback limit — mirrors `Buffer::new`
/// (`lifecycle.rs`) and `ScrollbackConfig::default`.
const EVICTION_DEFAULT_LIMIT: usize = 10_000;

/// Lines per timed iteration (one `seq 1 200`-sized burst).
const EVICTION_LF_BURST: usize = 200;

/// A prompt mark / command block is recorded every this many filled lines in
/// the `prompts` scenario (~500 marks across a 10 000-row scrollback).
const EVICTION_PROMPT_INTERVAL: usize = 20;

/// Which retained-state the eviction scenario carries in its scrollback.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EvictionScenario {
    /// Plain rows only.
    Plain,
    /// Whole scrollback compacted (Task 118) and compressed (Task 119).
    Compressed,
    /// OSC 133-style prompt marks and command blocks every
    /// `EVICTION_PROMPT_INTERVAL` lines.
    Prompts,
    /// One inline image placed mid-scrollback.
    Image,
}

/// Record one step of a prompt / command-block cycle for filled line `line`:
/// prompt + command start on line 0 of the interval, output start on line 1,
/// finish on the last line.
fn drive_prompt_cycle(buf: &mut Buffer, line: usize) {
    let phase = line % EVICTION_PROMPT_INTERVAL;
    if phase > 1 && phase != EVICTION_PROMPT_INTERVAL - 1 {
        return;
    }
    let fid = format!("bench-fid-{}", line / EVICTION_PROMPT_INTERVAL);
    if phase == 0 {
        buf.mark_prompt_row();
        let _id = buf.start_command_block(None, fid.clone());
        buf.mark_command_start_row(&fid);
    } else if phase == 1 {
        buf.mark_output_start_row(&fid);
    } else {
        let _ = buf.finish_command_block(Some(0), &fid);
    }
}

/// Build a primary buffer sitting exactly at capacity (`height + limit` rows)
/// in the requested scenario. Untimed setup.
fn build_eviction_buffer(scenario: EvictionScenario, limit: usize) -> Buffer {
    let mut buf = Buffer::new(EVICTION_WIDTH, EVICTION_HEIGHT).with_scrollback_limit(limit);

    // Fill past capacity so the scrollback is at its steady-state cap before
    // any timed work runs.
    let total_lines = EVICTION_HEIGHT + limit + 8;
    let image_line = total_lines / 2;
    for i in 0..total_lines {
        if scenario == EvictionScenario::Prompts {
            drive_prompt_cycle(&mut buf, i);
        }
        if scenario == EvictionScenario::Image && i == image_line {
            let _ = buf.place_image(
                make_bench_image(1),
                0,
                ImageProtocol::Kitty,
                None,
                None,
                0,
                None,
                1,
                None,
            );
        }
        let text: Vec<TChar> = format!("line{i:06}")
            .bytes()
            .cycle()
            .take(EVICTION_WIDTH)
            .map(TChar::Ascii)
            .collect();
        buf.insert_text(&text);
        buf.handle_cr();
        buf.handle_lf();
    }

    if scenario == EvictionScenario::Compressed {
        // The two entry points the PTY idle tick drives, run to completion
        // (usize::MAX budget) so the whole scrollback is compressed and the
        // timed evictions hit compressed blocks.
        let _ = buf.compact_idle_scrollback(usize::MAX);
        let _ = buf.compress_idle_scrollback(usize::MAX);
    }

    buf
}

/// Verify (once, outside any timing) that a scenario's buffer really is at
/// capacity and really carries the state the scenario claims to measure.
fn assert_eviction_scenario(scenario: EvictionScenario, limit: usize) {
    let buf = build_eviction_buffer(scenario, limit);
    let heap = buf.heap_bytes();
    assert_eq!(
        heap.total_rows,
        EVICTION_HEIGHT + limit,
        "scenario buffer must sit exactly at capacity"
    );
    match scenario {
        EvictionScenario::Plain => {}
        EvictionScenario::Compressed => assert!(
            heap.blocks_bytes > 0,
            "compressed scenario must hold compressed blocks"
        ),
        EvictionScenario::Prompts => {
            assert!(
                buf.prompt_rows().len() >= limit / EVICTION_PROMPT_INTERVAL - 2,
                "prompts scenario must retain ~one mark per interval"
            );
            assert!(buf.command_blocks().len() >= limit / EVICTION_PROMPT_INTERVAL - 2);
        }
        EvictionScenario::Image => assert!(
            buf.has_any_image_cell(),
            "image scenario must retain image cells"
        ),
    }
}

/// The timed unit: one burst of `EVICTION_LF_BURST` short lines.
fn eviction_burst(buf: &mut Buffer, payload: &[TChar]) {
    for _ in 0..EVICTION_LF_BURST {
        buf.insert_text(payload);
        buf.handle_cr();
        buf.handle_lf();
    }
}

fn eviction_payload() -> Vec<TChar> {
    b"sustained output burst line 12345"
        .iter()
        .copied()
        .map(TChar::Ascii)
        .collect()
}

fn run_eviction_scenario(
    group: &mut BenchmarkGroup<'_, WallTime>,
    id: BenchmarkId,
    scenario: EvictionScenario,
    limit: usize,
    payload: &[TChar],
) {
    assert_eviction_scenario(scenario, limit);
    group.bench_function(id, |b| {
        b.iter_batched_ref(
            || build_eviction_buffer(scenario, limit),
            |buf| eviction_burst(buf, payload),
            BatchSize::PerIteration,
        );
    });
}

// IDs: plain | compressed | prompts | image. Each times
// `EVICTION_LF_BURST` (200) line feeds, each with a short text insert, on a
// 124x31 buffer already at the default 10 000-row capacity.
fn bench_lf_eviction_at_capacity(c: &mut Criterion) {
    let payload = eviction_payload();
    let mut group = c.benchmark_group("bench_lf_eviction_at_capacity");
    group.throughput(Throughput::Elements(EVICTION_LF_BURST as u64));
    // Per-group override of the file-wide 2 s: a single `image` burst takes
    // ~370 ms (every LF rescans all live cells), so 10 samples need ~4.6 s.
    // Without this Criterion warns it cannot finish in the default time.
    group.measurement_time(Duration::from_secs(5));

    for (name, scenario) in [
        ("plain", EvictionScenario::Plain),
        ("compressed", EvictionScenario::Compressed),
        ("prompts", EvictionScenario::Prompts),
        ("image", EvictionScenario::Image),
    ] {
        run_eviction_scenario(
            &mut group,
            BenchmarkId::from_parameter(name),
            scenario,
            EVICTION_DEFAULT_LIMIT,
            &payload,
        );
    }

    group.finish();
}

// Retained-row-count sweep: the `plain` scenario at scrollback limits 1 000,
// 10 000 and 50 000, with identical timed work (one 200-line burst). If
// eviction is O(retained rows) the per-burst time scales with the limit; if it
// is O(evicted rows) the three IDs are flat.
fn bench_lf_eviction_scaling(c: &mut Criterion) {
    let payload = eviction_payload();
    let mut group = c.benchmark_group("bench_lf_eviction_scaling");
    group.throughput(Throughput::Elements(EVICTION_LF_BURST as u64));
    // Per-group override of the file-wide 2 s: the 50 000-row burst takes
    // ~20 ms and 10 samples exceed the default budget.
    group.measurement_time(Duration::from_secs(3));

    for limit in [1_000usize, 10_000, 50_000] {
        run_eviction_scenario(
            &mut group,
            BenchmarkId::new("plain", limit),
            EvictionScenario::Plain,
            limit,
            &payload,
        );
    }

    group.finish();
}

// Long-run eviction (Task 125 review S2). The at-capacity benches above time ONE
// 200-line burst on a freshly built buffer, so the moving head never gets as
// far as the compaction threshold (`max(live / 2, 64)` dead slots, ~5 000
// evictions at the default limit) and `RowStore::compact` is never inside a
// measurement. This bench times one buffer across at least TWO live-buffer
// lengths of line feeds, so every iteration includes several compactions, and
// the Criterion mean is the amortised cost per line feed with compaction.
//
// The mean hides a stall: compaction moves every live row at once, inside one
// line feed. `report_burst_latency` is an untimed-by-Criterion probe that
// times every 200-line burst of the same run and prints the mean, p99 and the
// worst burst. It is a bench helper, not a test: it asserts nothing, because a
// latency ceiling is a judgement on a particular machine.

/// Line feeds per long-run iteration: two full live-buffer lengths, plus slack
/// so the final partial burst cannot leave the second compaction out.
fn long_run_line_feeds(limit: usize) -> usize {
    2 * (limit + EVICTION_HEIGHT) + 2 * EVICTION_LF_BURST
}

/// Print per-burst latency over `long_run_line_feeds(limit)` line feeds,
/// repeated `passes` times on one at-capacity buffer.
fn report_burst_latency(limit: usize, passes: usize) {
    let payload = eviction_payload();
    let mut buf = build_eviction_buffer(EvictionScenario::Plain, limit);
    let bursts_per_pass = long_run_line_feeds(limit) / EVICTION_LF_BURST;
    let mut samples: Vec<Duration> = Vec::with_capacity(bursts_per_pass * passes);
    for _ in 0..passes {
        for _ in 0..bursts_per_pass {
            let start = std::time::Instant::now();
            eviction_burst(&mut buf, &payload);
            samples.push(start.elapsed());
        }
    }
    samples.sort_unstable();
    let total: Duration = samples.iter().sum();
    let count = u32::try_from(samples.len()).unwrap_or(u32::MAX).max(1);
    let mean = total / count;
    let at = |q: f64| {
        let idx = ((samples.len() as f64) * q) as usize;
        samples[idx.min(samples.len() - 1)]
    };
    // A burst that contains a compaction is far above the median; count the
    // bursts more than 3x the median as compaction bursts.
    let median = at(0.5);
    let slow = samples.iter().filter(|d| **d > median * 3).count();
    eprintln!(
        "[burst-latency limit={limit}] {} bursts of {EVICTION_LF_BURST} LF: mean {mean:?}, \
         median {median:?}, p99 {:?}, max {:?}; {slow} burst(s) > 3x median \
         (compaction); worst-burst overhead {:?} over the median",
        samples.len(),
        at(0.99),
        samples[samples.len() - 1],
        samples[samples.len() - 1].saturating_sub(median),
    );
}

// IDs: plain/<limit>. Times `long_run_line_feeds(limit)` line feeds (with a
// short text insert each) on one at-capacity buffer, compactions included.
fn bench_lf_eviction_long_run(c: &mut Criterion) {
    let payload = eviction_payload();
    // The probe runs at registration time, so honour a Criterion name filter
    // (any positional argument) rather than adding it to every unrelated run.
    let filters: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    if filters.is_empty() || filters.iter().any(|f| f.contains("long_run")) {
        for limit in [EVICTION_DEFAULT_LIMIT, 50_000] {
            report_burst_latency(limit, 3);
        }
    }

    let mut group = c.benchmark_group("bench_lf_eviction_long_run");
    group.measurement_time(Duration::from_secs(5));
    let limit = EVICTION_DEFAULT_LIMIT;
    let line_feeds = long_run_line_feeds(limit);
    group.throughput(Throughput::Elements(line_feeds as u64));
    assert_eviction_scenario(EvictionScenario::Plain, limit);
    group.bench_function(BenchmarkId::new("plain", limit), |b| {
        b.iter_batched_ref(
            || build_eviction_buffer(EvictionScenario::Plain, limit),
            |buf| {
                for _ in 0..line_feeds / EVICTION_LF_BURST {
                    eviction_burst(buf, &payload);
                }
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

// ---------------------------------------------------------------
// Criterion bootstrap
// ---------------------------------------------------------------
fn bench_visible_flatten(c: &mut Criterion) {
    // Pre-populate a 200×50 buffer with content so all visible rows are non-empty.
    let data: Vec<TChar> = gen_ascii_tchars(200 * 50);
    let mut buf = Buffer::new(200, 50);
    buf.insert_text(&data);

    let mut group = c.benchmark_group("bench_visible_flatten");
    group.throughput(Throughput::Elements((200 * 50) as u64));

    group.bench_function("visible_200x50", |b| {
        b.iter(|| {
            std::hint::black_box(buf.visible_as_tchars_and_tags(0));
        });
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: flatten scrollback rows
// ---------------------------------------------------------------
fn bench_scrollback_flatten(c: &mut Criterion) {
    // Fill enough lines to create ~1000 scrollback rows.
    // With width=80 and height=24, each line is 80 chars + LF.
    // 1024 extra lines above the visible window.
    let lines = 1024 + 24;
    let mut data = Vec::with_capacity(lines * 81);
    for _ in 0..lines {
        for _ in 0..80 {
            data.push(TChar::Ascii(b'x'));
        }
        data.push(TChar::NewLine);
    }
    let mut buf = Buffer::new(80, 24);
    buf.insert_text(&data);

    let mut group = c.benchmark_group("bench_scrollback_flatten");
    group.throughput(Throughput::Elements(1024 * 80));

    group.bench_function("scrollback_1024_rows", |b| {
        b.iter(|| {
            std::hint::black_box(buf.scrollback_as_tchars_and_tags(0));
        });
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: insert with frequent color-tag changes
// ---------------------------------------------------------------
fn bench_insert_with_color_changes(c: &mut Criterion) {
    // Build a sequence where the format tag changes every 8 characters,
    // alternating between two different foreground colors.
    const SEGMENT: usize = 8;
    const TOTAL: usize = 4_000;
    let colors = [
        TerminalColor::Custom(255, 0, 0),
        TerminalColor::Custom(0, 255, 0),
        TerminalColor::Custom(0, 0, 255),
        TerminalColor::Custom(255, 255, 0),
    ];

    let mut group = c.benchmark_group("bench_insert_with_color_changes");
    group.throughput(Throughput::Elements(TOTAL as u64));

    group.bench_function("color_change_every_8_chars", |b| {
        b.iter_batched(
            // Setup: build (tag, chars) pairs outside the timed section.
            || {
                (0..TOTAL / SEGMENT)
                    .map(|i| {
                        let color = colors[i % colors.len()];
                        let tag = FormatTag {
                            start: 0,
                            end: usize::MAX,
                            colors: StateColors {
                                color,
                                ..StateColors::default()
                            },
                            font_weight: FontWeight::Normal,
                            font_decorations: FontDecorationFlags::empty(),
                            url: None,
                            blink: freminal_common::buffer_states::fonts::BlinkState::None,
                        };
                        let chars: Vec<TChar> =
                            (0..SEGMENT).map(|j| TChar::Ascii(b'a' + j as u8)).collect();
                        (tag, chars)
                    })
                    .collect::<Vec<_>>()
            },
            // Timed section: insert each segment with its own format tag.
            |segments| {
                let mut buf = Buffer::new(80, 50);
                for (tag, chars) in segments {
                    buf.set_format(tag);
                    buf.insert_text(&chars);
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: cursor ops (CUP + data) — TUI screen redraw pattern
// ---------------------------------------------------------------
fn bench_cursor_ops(c: &mut Criterion) {
    // Simulate a TUI app: for each of 24 rows, position the cursor then write
    // a full line of 80 characters.
    const ROWS: usize = 24;
    const COLS: usize = 80;

    let mut group = c.benchmark_group("bench_cursor_ops");
    group.throughput(Throughput::Elements((ROWS * COLS) as u64));

    group.bench_function("cup_then_data_24x80", |b| {
        b.iter_batched(
            || Buffer::new(80, 24),
            |mut buf| {
                for row in 0..ROWS {
                    buf.set_cursor_pos(Some(0), Some(row));
                    let line: Vec<TChar> = (0..COLS)
                        .map(|i| TChar::Ascii(b'a' + (i % 26) as u8))
                        .collect();
                    buf.insert_text(&line);
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: LF until scrollback limit — stress handle_lf + limit enforcement
// ---------------------------------------------------------------
fn bench_lf_heavy(c: &mut Criterion) {
    // Push 4 100 LFs through `handle_lf` on a default `Buffer::new(80, 24)`.
    //
    // The compiled-in default scrollback limit is now 10 000 rows (Task 118
    // raised it from 4 000), so 4 100 LFs no longer reach capacity: this
    // bench measures buffer growth (`push_row` below the limit), NOT the
    // `enforce_scrollback_limit` front-drain. The ID and workload are kept
    // unchanged so historical baselines stay comparable. Eviction at the real
    // default capacity is covered by `bench_lf_eviction_at_capacity` and
    // `bench_lf_eviction_scaling` (Task 125.12).
    const LF_COUNT: usize = 4_100;

    let mut group = c.benchmark_group("bench_lf_heavy");
    group.throughput(Throughput::Elements(LF_COUNT as u64));

    group.bench_function("lf_4100_times", |b| {
        b.iter_batched(
            || Buffer::new(80, 24),
            |mut buf| {
                for i in 0..LF_COUNT {
                    // Write one character per line so rows are not empty.
                    buf.insert_text(&[TChar::Ascii(b'a' + (i % 26) as u8)]);
                    buf.handle_lf();
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: steady-state LF at scrollback capacity — `merge_cache`
// rotation invalidation cost (Task #405).
//
// At scrollback capacity every line feed evicts a row, so the visible
// window's first logical row number advances and the merge-cache
// fingerprint (keyed by that number since Task 125.16) misses (see
// `Buffer::merge_cache`'s field doc and the
// `incremental_merge_matches_oracle_after_scrollback_capacity_rotation`
// regression test in `flatten.rs`). The very next
// `visible_as_tchars_and_tags` flatten therefore cannot take the
// incremental fast path and must fully re-merge the whole visible window. This benchmark isolates that worst case: a
// buffer already sitting at its scrollback cap, then one LF (which always
// rotates) immediately followed by one flatten, repeated every iteration
// so the flatten never gets to reuse a warm cache. Two window heights are
// measured to show the cost scales with `height` (a full-window re-merge),
// not with `scrollback_limit`.
// ---------------------------------------------------------------
fn build_buffer_at_scrollback_capacity(
    width: usize,
    height: usize,
    scrollback_limit: usize,
) -> Buffer {
    let mut buf = Buffer::new(width, height).with_scrollback_limit(scrollback_limit);
    // Fill well past capacity so scrollback is already at its steady-state
    // cap before any timed iteration runs (pre-fill happens outside the
    // timed closure via `iter_batched`'s setup callback).
    let total_lines = height + scrollback_limit + 8;
    for i in 0..total_lines {
        let text: Vec<TChar> = format!("line{i:06}")
            .bytes()
            .cycle()
            .take(width)
            .map(TChar::Ascii)
            .collect();
        buf.insert_text(&text);
        buf.handle_lf();
        buf.handle_cr();
    }
    buf
}

fn bench_lf_flatten_at_capacity(c: &mut Criterion) {
    const WIDTH: usize = 200;
    const SCROLLBACK_LIMIT: usize = 4_000;

    let mut group = c.benchmark_group("bench_lf_flatten_at_capacity");

    for height in [24usize, 100usize] {
        group.bench_with_input(
            BenchmarkId::new("lf_then_flatten_steady_state", height),
            &height,
            |b, &height| {
                // `iter_batched_ref` (not `iter_batched`): the routine takes
                // `&mut Buffer`, so the ~`height + SCROLLBACK_LIMIT`-row
                // buffer's destructor runs when the batch's input `Vec` is
                // dropped — OUTSIDE the timed span — instead of inside the
                // routine on every iteration. With by-value `iter_batched`
                // the per-iteration buffer drop dominated the measurement
                // (thousands of `Row` frees), swamping the single LF+flatten
                // this bench isolates and making the result track
                // `SCROLLBACK_LIMIT` rather than `height`. The buffer stays at
                // capacity across the reused iterations (each LF is a
                // push+drain that nets `rows.len()` unchanged), so every timed
                // iteration remains in the exact steady state under test.
                b.iter_batched_ref(
                    || {
                        let mut buf =
                            build_buffer_at_scrollback_capacity(WIDTH, height, SCROLLBACK_LIMIT);
                        // Warm the merge cache once, outside the timed
                        // section, so the timed rotation+flatten pair below
                        // starts from a populated (not merely absent) cache
                        // — matching the real steady-state PTY loop, where
                        // the previous frame already warmed it.
                        let _ = buf.visible_as_tchars_and_tags(0);
                        buf
                    },
                    |buf| {
                        buf.insert_text(&[TChar::Ascii(b'x')]);
                        buf.handle_lf();
                        buf.handle_cr();
                        std::hint::black_box(buf.visible_as_tchars_and_tags(0));
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: batched LF then flatten at scrollback capacity — amortized
// re-merge cost under realistic PTY-read batching (issue #457 follow-up).
//
// `bench_lf_flatten_at_capacity` above measures the worst case: exactly one
// LF (which always rotates scrollback and nulls `merge_cache` once already
// at capacity) immediately followed by one flatten, so the O(window-height)
// full re-merge is paid on every single line. That is not how
// `visible_as_tchars_and_tags` (via `build_snapshot`) is actually driven in
// production: the GUI/PTY thread batches up to 64 read chunks (up to
// ~256 KiB) before publishing a single snapshot, so many LFs typically
// happen between two flatten calls. This benchmark isolates that amortized
// case instead: for a buffer already at scrollback capacity, perform `K`
// LFs back-to-back, then exactly ONE flatten, as a single timed iteration —
// for `K` in `[1, 8, 32, 128, 512]`. If PTY-read batching already mitigates
// the re-merge cost identified by issue #457 in realistic usage, the
// per-LF amortized cost (total time / K) should fall sharply as `K` grows,
// since the single full re-merge's cost is spread across all K lines
// instead of being paid once per line as in the worst-case benchmark above.
//
// Only height=100 is measured (per-task scope): it is the more realistic,
// interesting window size, and crossing every K with both heights adds
// little beyond what `bench_lf_flatten_at_capacity` already shows about the
// height dependency.
// ---------------------------------------------------------------
fn bench_lf_batch_then_flatten_at_capacity(c: &mut Criterion) {
    const WIDTH: usize = 200;
    const SCROLLBACK_LIMIT: usize = 4_000;
    const HEIGHT: usize = 100;

    let mut group = c.benchmark_group("bench_lf_batch_then_flatten_at_capacity");

    for k in [1usize, 8, 32, 128, 512] {
        group.bench_with_input(
            BenchmarkId::new("lf_batch_then_flatten_steady_state", k),
            &k,
            |b, &k| {
                // Same `iter_batched_ref` care as `bench_lf_flatten_at_capacity`:
                // the routine takes `&mut Buffer`, so the ~`HEIGHT +
                // SCROLLBACK_LIMIT`-row buffer's destructor runs when the
                // batch's input `Vec` is dropped — OUTSIDE the timed span —
                // instead of inside the routine on every iteration. The
                // buffer stays at capacity across all K LFs within one
                // iteration: each LF's push+drain nets `rows.len()`
                // unchanged, so this holds for any K, and every timed
                // iteration remains in the exact steady state under test.
                b.iter_batched_ref(
                    || {
                        let mut buf =
                            build_buffer_at_scrollback_capacity(WIDTH, HEIGHT, SCROLLBACK_LIMIT);
                        // Warm the merge cache once, outside the timed
                        // section, matching the real steady-state PTY loop
                        // where the previous frame already warmed it.
                        let _ = buf.visible_as_tchars_and_tags(0);
                        buf
                    },
                    |buf| {
                        for _ in 0..k {
                            buf.insert_text(&[TChar::Ascii(b'x')]);
                            buf.handle_lf();
                            buf.handle_cr();
                        }
                        std::hint::black_box(buf.visible_as_tchars_and_tags(0));
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: erase display (ED) on a full buffer
// ---------------------------------------------------------------
fn bench_erase_display(c: &mut Criterion) {
    // Fill a 80×24 buffer then erase it. Measure only the erase.
    let data = gen_ascii_tchars(80 * 24);

    let mut group = c.benchmark_group("bench_erase_display");

    group.bench_function("erase_to_end_of_display_80x24", |b| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new(80, 24);
                buf.insert_text(&data);
                buf
            },
            |mut buf| {
                buf.erase_to_end_of_display();
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: scrollback rendering at various offsets (24.1)
// ---------------------------------------------------------------
fn bench_scrollback_render(c: &mut Criterion) {
    // Pre-populate a buffer with ~5000 rows of scrollback.
    // width=80, height=24 → 5000+24 lines needed for 5000 scrollback rows.
    let total_lines = 5024;
    let mut data = Vec::with_capacity(total_lines * 81);
    for i in 0..total_lines {
        for j in 0..80 {
            data.push(TChar::Ascii(b'a' + ((i + j) % 26) as u8));
        }
        data.push(TChar::NewLine);
    }
    let mut buf = Buffer::new(80, 24);
    buf.insert_text(&data);

    let mut group = c.benchmark_group("bench_scrollback_render");
    group.throughput(Throughput::Elements((80 * 24) as u64));

    for offset in [0, 1000, 4000] {
        group.bench_with_input(
            BenchmarkId::new("visible_at_offset", offset),
            &offset,
            |b, &offset| {
                b.iter(|| {
                    std::hint::black_box(buf.visible_as_tchars_and_tags(offset));
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: alternate screen switch (24.1)
// ---------------------------------------------------------------
fn bench_alternate_screen_switch(c: &mut Criterion) {
    // Measure the cost of entering and leaving the alternate screen on a
    // populated buffer, in the shape `?1049h` / `?1049l` drive: DECSC, switch,
    // clear on the way in; switch, DECRC on the way out. The IDs are the
    // pre-131.5 ones so the before/after comparison lines up. Every routine
    // returns the buffer so criterion drops it outside the timed region: the
    // measurement is the switch, not the deallocation of two screens.
    let primary_data = gen_ascii_tchars(80 * 100); // 100 lines in primary
    let alt_data = gen_ascii_tchars(80 * 24); // full alternate screen

    let mut group = c.benchmark_group("bench_alternate_screen_switch");

    group.bench_function("enter_alternate", |b| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new(80, 24);
                buf.insert_text(&primary_data);
                buf
            },
            |mut buf| {
                buf.save_cursor();
                buf.switch_to_alternate();
                buf.clear_alternate_screen();
                // Returned, so the buffer is dropped outside the timed region.
                buf
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("leave_alternate", |b| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new(80, 24);
                buf.insert_text(&primary_data);
                buf.save_cursor();
                buf.switch_to_alternate();
                buf.clear_alternate_screen();
                buf.insert_text(&alt_data);
                buf
            },
            |mut buf| {
                buf.switch_to_primary();
                buf.restore_cursor();
                buf
            },
            BatchSize::SmallInput,
        );
    });

    // Re-entry onto an alternate screen that is already parked with content:
    // the persisted path, a pure move of the parked store with no allocation
    // of a fresh blank screen.
    group.bench_function("alternate_reenter", |b| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new(80, 24);
                buf.insert_text(&primary_data);
                buf.switch_to_alternate();
                buf.clear_alternate_screen();
                buf.insert_text(&alt_data);
                buf.switch_to_primary();
                buf
            },
            |mut buf| {
                buf.switch_to_alternate();
                buf
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: erase entire display — ED Ps=2 (24.1)
// ---------------------------------------------------------------
fn bench_erase_display_full(c: &mut Criterion) {
    // Fill a 200×50 buffer then erase entire display.
    let data = gen_ascii_tchars(200 * 50);

    let mut group = c.benchmark_group("bench_erase_display_full");

    group.bench_function("erase_display_200x50", |b| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new(200, 50);
                buf.insert_text(&data);
                buf
            },
            |mut buf| {
                buf.erase_display();
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: LF-heavy scroll with BCE (non-default background)
// ---------------------------------------------------------------
fn bench_lf_heavy_bce(c: &mut Criterion) {
    // Same workload as bench_lf_heavy but with a non-default background
    // color set, exercising the BCE fill path in push_row / handle_lf. As
    // with `bench_lf_heavy`, 4 100 LFs stay below the default 10 000-row
    // scrollback limit, so no eviction occurs (see the note there).
    const LF_COUNT: usize = 4_100;

    let bce_tag = FormatTag {
        colors: StateColors::default().with_background_color(TerminalColor::Blue),
        ..FormatTag::default()
    };

    let mut group = c.benchmark_group("bench_lf_heavy_bce");
    group.throughput(Throughput::Elements(LF_COUNT as u64));

    group.bench_function("lf_4100_times_bce", |b| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new(80, 24);
                buf.set_format(bce_tag.clone());
                buf
            },
            |mut buf| {
                for i in 0..LF_COUNT {
                    buf.insert_text(&[TChar::Ascii(b'a' + (i % 26) as u8)]);
                    buf.handle_lf();
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: erase display with BCE (non-default background)
// ---------------------------------------------------------------
fn bench_erase_display_bce(c: &mut Criterion) {
    // Fill a 80×24 buffer then erase it with a non-default background,
    // exercising the BCE path in row clearing.
    let data = gen_ascii_tchars(80 * 24);

    let bce_tag = FormatTag {
        colors: StateColors::default().with_background_color(TerminalColor::Red),
        ..FormatTag::default()
    };

    let mut group = c.benchmark_group("bench_erase_display_bce");

    group.bench_function("erase_display_80x24_bce", |b| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new(80, 24);
                buf.set_format(bce_tag.clone());
                buf.insert_text(&data);
                buf
            },
            |mut buf| {
                buf.erase_display();
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: move_cursor_relative — stress the CUU/CUD/CUF/CUB hot path.
// This is the path that clamped_offset() runs on; it exercises the
// usize<->i32 conversions used by relative cursor motion.
// ---------------------------------------------------------------
fn bench_move_cursor_relative(c: &mut Criterion) {
    const ITERS: usize = 10_000;

    let mut group = c.benchmark_group("bench_move_cursor_relative");
    group.throughput(Throughput::Elements(ITERS as u64));

    group.bench_function("alternating_dx_dy_80x24", |b| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new(80, 24);
                buf.set_cursor_pos(Some(40), Some(12));
                buf
            },
            |mut buf| {
                // Alternate between +1/-1 in x and y; clamping kicks in at edges.
                for i in 0..ITERS {
                    let dx = if i % 2 == 0 { 1 } else { -1 };
                    let dy = if i % 4 < 2 { 1 } else { -1 };
                    buf.move_cursor_relative(dx, dy);
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: flatten a buffer whose visible rows each contain a plain URL.
//
// Measures the cost of the single-pass URL detection + tag splicing added
// in Task 71.7b.  Each iteration builds a fresh buffer so the flatten cache
// starts cold; this captures the full work of byte-mirror construction,
// regex scanning, and tag splicing across the full visible window.
// ---------------------------------------------------------------
fn bench_flatten_url_heavy(c: &mut Criterion) {
    let width = 80usize;
    let height = 50usize;
    let url = b"https://example.com/path?q=1&r=2";
    let prefix = b"see ";
    let suffix_template = b" for more info xyz";

    // One row's content. A real hard break is driven per row via
    // `handle_lf`/`handle_cr` below (matching how the PTY thread actually
    // terminates a line) rather than embedding a `TChar::NewLine` cell in the
    // inserted text: `TChar::NewLine` is an ordinary printable cell to
    // `insert_text`, not a line-break instruction, so embedding it does not
    // produce a hard break and would make every row after the first a DECAWM
    // soft-wrap continuation of one giant logical line — defeating the
    // point of this benchmark (one complete, self-contained URL per row).
    let mut row_data: Vec<TChar> = Vec::with_capacity(width);
    let mut row_len = 0usize;
    for &b in prefix {
        row_data.push(TChar::Ascii(b));
        row_len += 1;
    }
    for &b in url {
        row_data.push(TChar::Ascii(b));
        row_len += 1;
    }
    for &b in suffix_template {
        if row_len >= width {
            break;
        }
        row_data.push(TChar::Ascii(b));
        row_len += 1;
    }
    while row_len < width {
        row_data.push(TChar::Ascii(b' '));
        row_len += 1;
    }

    let mut group = c.benchmark_group("bench_flatten_url_heavy");
    group.throughput(Throughput::Elements((width * height) as u64));

    group.bench_function("visible_80x50_with_urls", |b| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new(width, height);
                for i in 0..height {
                    buf.insert_text(&row_data);
                    if i + 1 < height {
                        buf.handle_lf();
                        buf.handle_cr();
                    }
                }
                buf
            },
            |mut buf| {
                std::hint::black_box(buf.visible_as_tchars_and_tags(0));
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: URL auto-detection when it wraps across rows (Task 418)
//
// Unlike `bench_flatten_url_heavy` (one complete URL per row, hard-broken by
// a real newline), this benchmark fills every row edge-to-edge with URL
// content that DECAWM soft-wraps across many rows, so the group-level
// redetect path added for GitHub issue #418 (URLs wrapping across rows must
// be detected in full) is actually exercised on every flatten.
// ---------------------------------------------------------------
fn bench_flatten_wrapped_url_heavy(c: &mut Criterion) {
    let width = 80usize;
    let height = 50usize;

    // One continuous URL, longer than the whole screen, so it soft-wraps
    // across every row with no hard breaks at all.
    let mut url = String::from("https://example.com/");
    while url.len() < width * height {
        url.push_str("a/very/long/path/segment/");
    }
    let data: Vec<TChar> = url.chars().map(TChar::from).collect();

    let mut group = c.benchmark_group("bench_flatten_wrapped_url_heavy");
    group.throughput(Throughput::Elements((width * height) as u64));

    group.bench_function("visible_80x50_one_wrapped_url", |b| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new(width, height);
                buf.insert_text(&data);
                buf
            },
            |mut buf| {
                std::hint::black_box(buf.visible_as_tchars_and_tags(0));
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: command-block record/finish cycle (72.2)
//
// Measures the cost of recording 10,000 start/finish command-block cycles.
// Captures the new VecDeque<CommandBlock> machinery added in 72.2.
// ---------------------------------------------------------------
fn bench_command_block_record(c: &mut Criterion) {
    c.bench_function("command_block_record_10k", |b| {
        b.iter_batched(
            || Buffer::new(80, 24),
            |mut buffer| {
                for i in 0..10_000u32 {
                    let fid = format!("bench-{i}");
                    let _id = buffer.start_command_block(None, fid.clone());
                    let _ = buffer.finish_command_block(Some(0), &fid);
                }
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

// ---------------------------------------------------------------
// Benchmark: ImageStore::insert per-insert quota-scan cost (100.5)
//
// Every insert() now scans all stored images to sum base/anim pool byte
// totals via enforce_quota(). This measures that scan cost at a realistic
// store size (256 stored images) without allocating anywhere near the real
// 320 MB quota — the scan cost scales with image COUNT, not pixel size, so
// images are kept intentionally small (4 KB) to keep setup fast.
// ---------------------------------------------------------------
fn make_bench_image(id: u64) -> InlineImage {
    InlineImage {
        id,
        pixels: std::sync::Arc::new(vec![0u8; 4096]),
        width_px: 32,
        height_px: 32,
        display_cols: 4,
        display_rows: 2,
        size_mode: ImageSizeMode::NativePixels,
        frames: Vec::new(),
        root_gap_ms: 0,
        animation: AnimationControl::default(),
    }
}

fn bench_image_store_insert_at_quota(c: &mut Criterion) {
    const PRELOAD_COUNT: u64 = 256;

    c.bench_function("image_store_insert_at_quota", |b| {
        b.iter_batched(
            || {
                let mut store = ImageStore::new();
                for id in 0..PRELOAD_COUNT {
                    store.insert(make_bench_image(id));
                }
                store
            },
            |mut store| {
                store.insert(make_bench_image(PRELOAD_COUNT));
                std::hint::black_box(&store);
            },
            BatchSize::SmallInput,
        );
    });
}

// ---------------------------------------------------------------
// Benchmark: CompressedBlock compress/decompress round trip (Task 119.6)
//
// Builds a representative 256-row, ~120-col block of colored content (the
// "shell session" style bracket from the Task 119 feasibility spike — many
// rows sharing structure with a handful of color changes) and separately
// measures CompressedBlock::from_rows (compress) and decompress_into
// (decompress). Used to justify the IDLE_COMPRESSION_BUDGET tuning in
// freminal/src/gui/pty.rs: per-block decompress cost must stay well under
// one 16.6ms frame (plan target: ~34µs/256-line block at LZ4 speed).
// ---------------------------------------------------------------
fn build_representative_compact_block(rows: usize, width: usize) -> Vec<CompactRow> {
    let colors = [
        TerminalColor::Custom(0, 200, 0),
        TerminalColor::Custom(200, 200, 0),
        TerminalColor::Default,
    ];

    (0..rows)
        .map(|i| {
            let mut row = Row::new(width);
            let color = colors[i % colors.len()];
            let tag = FormatTag {
                start: 0,
                end: usize::MAX,
                colors: StateColors {
                    color,
                    ..StateColors::default()
                },
                ..FormatTag::default()
            };
            let text = format!("scrollback line {i:06} of representative shell output data");
            let chars: Vec<TChar> = text.bytes().cycle().take(width).map(TChar::Ascii).collect();
            row.insert_text(0, &chars, &tag);
            CompactRow::from_row(&row).expect("row should be compactable")
        })
        .collect()
}

fn bench_compressed_block_round_trip(c: &mut Criterion) {
    const ROWS: usize = 256;
    const WIDTH: usize = 120;

    let compact_rows = build_representative_compact_block(ROWS, WIDTH);

    let mut group = c.benchmark_group("bench_compressed_block_round_trip");
    group.throughput(Throughput::Elements(ROWS as u64));

    group.bench_function(BenchmarkId::new("compress", ROWS), |b| {
        b.iter(|| {
            std::hint::black_box(CompressedBlock::from_rows(&compact_rows));
        });
    });

    let block = CompressedBlock::from_rows(&compact_rows);
    let mut scratch = Vec::new();
    group.bench_function(BenchmarkId::new("decompress", ROWS), |b| {
        b.iter(|| {
            std::hint::black_box(block.decompress_into(&mut scratch));
        });
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: scrolling into a compressed scrollback region (Task 119.6)
//
// Builds a buffer whose entire scrollback has been Task-118-compacted and
// Task-119-compressed (mirroring the idle-tick's settled state), then
// measures the real decompress-on-scroll cost paid by a flatten that
// touches the compressed region. Each iteration rebuilds the compressed
// buffer from scratch (`iter_batched` + `BatchSize::LargeInput`) because
// decompression mutates state (single residency — Buffer::ensure_decompressed
// restores rows to Compact and empties `self.blocks`), so a stale
// already-decompressed buffer would not measure the cold path a second time.
// ---------------------------------------------------------------
fn build_compressed_scrollback_buffer() -> Buffer {
    let total_lines = 1024 + 24;
    let mut data = Vec::with_capacity(total_lines * 81);
    for _ in 0..total_lines {
        for _ in 0..80 {
            data.push(TChar::Ascii(b'x'));
        }
        data.push(TChar::NewLine);
    }
    let mut buf = Buffer::new(80, 24);
    buf.insert_text(&data);
    let _ = buf.compact_idle_scrollback(usize::MAX);
    let _ = buf.compress_idle_scrollback(usize::MAX);
    buf
}

fn bench_scroll_into_compressed_region(c: &mut Criterion) {
    let mut group = c.benchmark_group("bench_scroll_into_compressed_region");
    group.throughput(Throughput::Elements(1024 * 80));

    group.bench_function("scrollback_flatten_1024_compressed_rows", |b| {
        b.iter_batched(
            build_compressed_scrollback_buffer,
            |mut buf| {
                std::hint::black_box(buf.scrollback_as_tchars_and_tags(0));
            },
            BatchSize::LargeInput,
        );
    });

    // Also measure the GUI's actual scroll path: a scrolled-back visible
    // window flatten (the path `scrolled_visible_window_flatten_decompresses_compressed_rows`
    // in `buffer/compression.rs` regression-tests for correctness) reaching
    // all the way into the compressed region.
    group.bench_function("visible_flatten_scrolled_into_compressed", |b| {
        b.iter_batched(
            build_compressed_scrollback_buffer,
            |mut buf| {
                let max_offset = buf.max_scroll_offset();
                std::hint::black_box(buf.visible_as_tchars_and_tags(max_offset));
            },
            BatchSize::LargeInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// Benchmark: absolute per-idle-tick cost of the budgeted background work
// (Task 119 follow-up tuning).
//
// These measure the CPU cost of ONE real idle tick's worth of work, in
// isolation, at the SAME per-tick budgets production uses in
// `freminal/src/gui/pty.rs`. The point is an ABSOLUTE time figure (µs to
// process one tick's rows), not a wall-clock CPU% on the dev box: a machine
// Nx slower simply multiplies the measured µs by N, and the justification for
// the budgets is that even Nx that figure stays far below the 100ms idle-tick
// interval, so a full-scrollback catch-up burst never stalls the PTY tick loop
// or pegs a core on modest hardware.
//
// The two budgets differ (matching production): compaction runs 1024 rows/tick,
// compression 4096 rows/tick. Each bench uses its own production budget so its
// result is directly "one real tick" — no mental scaling needed.
//
// Each iteration rebuilds fresh state (`iter_batched` + `BatchSize::LargeInput`)
// because both operations mutate the buffer (compaction converts Live -> Compact;
// compression evicts Compact -> block with single residency), so a second call
// on the same buffer would find less/no work.
// ---------------------------------------------------------------

/// Matches `IDLE_COMPACTION_BUDGET` in `freminal/src/gui/pty.rs`, so
/// `bench_idle_compaction_tick` measures exactly one production compaction
/// tick's worth of work.
const IDLE_COMPACTION_BUDGET_BENCH: usize = 1024;

/// Matches `IDLE_COMPRESSION_BUDGET` in `freminal/src/gui/pty.rs`, so
/// `bench_idle_compression_tick` measures exactly one production compression
/// tick's worth of work (16 blocks of 256 rows).
const IDLE_COMPRESSION_BUDGET_BENCH: usize = 4096;

/// Build a buffer with `scrollback_rows` rows of representative ~120-col
/// content, all still `Live` (not compacted) — the worst case a compaction
/// tick faces.
fn build_live_scrollback_buffer(scrollback_rows: usize) -> Buffer {
    const WIDTH: usize = 120;
    // + a screen's worth so the visible window sits below the scrollback we
    // want compacted.
    let total_lines = scrollback_rows + 24;
    let mut data = Vec::with_capacity(total_lines * (WIDTH + 1));
    for i in 0..total_lines {
        let text = format!("scrollback line {i:06} of representative shell output data");
        for b in text.bytes().cycle().take(WIDTH) {
            data.push(TChar::Ascii(b));
        }
        data.push(TChar::NewLine);
    }
    let mut buf = Buffer::new(WIDTH, 24);
    buf.insert_text(&data);
    buf
}

fn bench_idle_compaction_tick(c: &mut Criterion) {
    // Twice the budget of all-Live scrollback rows so a full budget's worth of
    // work is available for the tick under test.
    let scrollback_rows = 2 * IDLE_COMPACTION_BUDGET_BENCH;

    let mut group = c.benchmark_group("bench_idle_compaction_tick");
    group.throughput(Throughput::Elements(IDLE_COMPACTION_BUDGET_BENCH as u64));
    group.bench_function(
        BenchmarkId::new("compact", IDLE_COMPACTION_BUDGET_BENCH),
        |b| {
            b.iter_batched(
                || build_live_scrollback_buffer(scrollback_rows),
                |mut buf| {
                    std::hint::black_box(buf.compact_idle_scrollback(IDLE_COMPACTION_BUDGET_BENCH));
                },
                BatchSize::LargeInput,
            );
        },
    );
    group.finish();
}

fn bench_idle_compression_tick(c: &mut Criterion) {
    // Twice the budget of fully-compacted (but uncompressed) scrollback rows so
    // a full compression tick's worth of work (16 blocks of 256 rows) exists.
    let scrollback_rows = 2 * IDLE_COMPRESSION_BUDGET_BENCH;

    let mut group = c.benchmark_group("bench_idle_compression_tick");
    group.throughput(Throughput::Elements(IDLE_COMPRESSION_BUDGET_BENCH as u64));
    group.bench_function(
        BenchmarkId::new("compress", IDLE_COMPRESSION_BUDGET_BENCH),
        |b| {
            b.iter_batched(
                || {
                    let mut buf = build_live_scrollback_buffer(scrollback_rows);
                    // Fully compact first: compression only touches already-compact
                    // rows, so the tick under test starts from the settled-compact
                    // state the real idle loop reaches before compressing.
                    let _ = buf.compact_idle_scrollback(usize::MAX);
                    buf
                },
                |mut buf| {
                    std::hint::black_box(
                        buf.compress_idle_scrollback(IDLE_COMPRESSION_BUDGET_BENCH),
                    );
                },
                BatchSize::LargeInput,
            );
        },
    );
    group.finish();
}

// ---------------------------------------------------------------
// Benchmarks: full-depth width-change reflow (Task 120.1).
//
// `buffer_resize` and `softwrap_heavy` reflow small buffers and do not show
// the cost a real pane pays: a width change reflows the WHOLE scrollback
// (`Buffer::reflow_to_width`), so its cost is proportional to scrollback
// depth, not to the width delta. These groups fill the default 10,000-row
// scrollback with realistic shell-like content -- ~70-char lines carrying a
// few colour runs, ~15% of lines long enough to soft-wrap -- bring it to one
// of the three storage states the idle tick produces, and time ONE
// `set_size` width change.
//
// Storage states (via the public idle-tick entry points, run to completion):
// - `live`: rows exactly as written (the state right after output);
// - `compacted`: `compact_idle_scrollback(usize::MAX)` (Task 118);
// - `compressed`: compacted, then `compress_idle_scrollback(usize::MAX)`
//   (Task 119), so reflow must first decompress every LZ4 block.
//
// Width changes from the 100-column fill width: `widen` (100 -> 160),
// `narrow` (100 -> 60) and `one_col` (100 -> 99). The 1-column case is the
// one a drag resize produces on almost every frame.
//
// Each iteration rebuilds the buffer in untimed setup (`iter_batched` +
// `BatchSize::LargeInput`): reflow replaces every row and leaves them `Live`,
// so a reused buffer would no longer be in the stated storage state. The
// reflowed buffer is returned from the routine so its drop is not timed.
//
// The 100,000-row variant (`reflow_full_depth_100k`, `one_col` only) costs
// over a second per iteration plus setup, so it is opt-in: it runs only when
// the Criterion filter names `100k`, e.g.
// `cargo bench -p freminal-buffer --bench buffer_row_bench -- reflow_full_depth_100k`.
// ---------------------------------------------------------------

/// Fill width of the reflow benches (columns).
const REFLOW_WIDTH: usize = 100;

/// Screen height of the reflow benches (rows).
const REFLOW_HEIGHT: usize = 40;

/// Storage state of the scrollback when the timed reflow runs.
#[derive(Clone, Copy)]
enum ReflowStorage {
    Live,
    Compacted,
    Compressed,
}

impl ReflowStorage {
    const ALL: [Self; 3] = [Self::Live, Self::Compacted, Self::Compressed];

    const fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Compacted => "compacted",
            Self::Compressed => "compressed",
        }
    }
}

/// The width changes timed, as `(label, new_width)`.
const REFLOW_WIDTH_CHANGES: [(&str, usize); 3] = [
    ("widen", 160),
    ("narrow", 60),
    ("one_col", REFLOW_WIDTH - 1),
];

/// A foreground-only format tag in `color`.
fn reflow_tag(color: TerminalColor) -> FormatTag {
    FormatTag {
        colors: StateColors {
            color,
            ..StateColors::default()
        },
        ..FormatTag::default()
    }
}

/// Write one shell-like line `i`: alternating default and coloured runs,
/// ~70 chars in total. Every 7th line (~15%) is extended past `REFLOW_WIDTH`
/// so it soft-wraps onto a continuation row.
fn write_reflow_line(buf: &mut Buffer, i: usize, tags: &[FormatTag; 4]) {
    let wraps = i.is_multiple_of(7);
    let total = if wraps { 130 + (i % 40) } else { 60 + (i % 21) };
    let words = format!("line {i:06} drwxr-xr-x fred users 4096 Oct 08 src/buffer/resize.rs ");
    let mut written = 0usize;
    let mut run = 0usize;
    while written < total {
        // Alternate default and coloured runs of 8..=19 chars.
        let run_len = (8 + (i + run * 5) % 12).min(total - written);
        let tag = if run.is_multiple_of(2) {
            &tags[0]
        } else {
            &tags[1 + (run / 2) % 3]
        };
        buf.set_format(tag.clone());
        let text: Vec<TChar> = words
            .bytes()
            .cycle()
            .skip(written)
            .take(run_len)
            .map(TChar::Ascii)
            .collect();
        buf.insert_text(&text);
        written += run_len;
        run += 1;
    }
    buf.set_format(tags[0].clone());
    buf.handle_cr();
    buf.handle_lf();
}

/// Build a `REFLOW_WIDTH` x `REFLOW_HEIGHT` buffer whose scrollback is full at
/// `limit` rows, in the given storage state.
fn build_reflow_buffer(limit: usize, storage: ReflowStorage) -> Buffer {
    let tags = [
        FormatTag::default(),
        reflow_tag(TerminalColor::Custom(220, 80, 80)),
        reflow_tag(TerminalColor::Custom(80, 200, 120)),
        reflow_tag(TerminalColor::Custom(90, 140, 230)),
    ];
    let mut buf = Buffer::new(REFLOW_WIDTH, REFLOW_HEIGHT).with_scrollback_limit(limit);
    // Logical lines, not rows: ~15% of lines take two rows, so this
    // overfills the row budget and the buffer settles at capacity.
    for i in 0..limit + REFLOW_HEIGHT {
        write_reflow_line(&mut buf, i, &tags);
    }
    match storage {
        ReflowStorage::Live => {}
        ReflowStorage::Compacted => {
            let _ = buf.compact_idle_scrollback(usize::MAX);
        }
        ReflowStorage::Compressed => {
            let _ = buf.compact_idle_scrollback(usize::MAX);
            let _ = buf.compress_idle_scrollback(usize::MAX);
        }
    }
    buf
}

/// Verify (once, outside timing) that a reflow buffer is at capacity and in
/// the storage state its label claims.
fn assert_reflow_buffer(limit: usize, storage: ReflowStorage, live_rows_bytes: usize) {
    let buf = build_reflow_buffer(limit, storage);
    let heap = buf.heap_bytes();
    assert_eq!(
        heap.total_rows,
        REFLOW_HEIGHT + limit,
        "reflow buffer must sit exactly at capacity"
    );
    match storage {
        ReflowStorage::Live => assert_eq!(heap.blocks_bytes, 0),
        ReflowStorage::Compacted => {
            assert_eq!(heap.blocks_bytes, 0, "compacted state must not compress");
            assert!(
                heap.rows_bytes < live_rows_bytes / 2,
                "compacted state must shrink row storage"
            );
        }
        ReflowStorage::Compressed => assert!(
            heap.blocks_bytes > 0,
            "compressed state must hold compressed blocks"
        ),
    }
}

fn run_reflow_group(
    group: &mut BenchmarkGroup<'_, WallTime>,
    limit: usize,
    changes: &[(&str, usize)],
) {
    let live_rows_bytes = build_reflow_buffer(limit, ReflowStorage::Live)
        .heap_bytes()
        .rows_bytes;
    for storage in ReflowStorage::ALL {
        assert_reflow_buffer(limit, storage, live_rows_bytes);
        for &(change, new_width) in changes {
            group.bench_function(BenchmarkId::new(storage.label(), change), |b| {
                b.iter_batched(
                    || build_reflow_buffer(limit, storage),
                    |mut buf| {
                        std::hint::black_box(buf.set_size(new_width, REFLOW_HEIGHT, 0));
                        buf
                    },
                    BatchSize::LargeInput,
                );
            });
        }
    }
}

fn bench_reflow_full_depth(c: &mut Criterion) {
    let mut group = c.benchmark_group("reflow_full_depth");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(12));
    run_reflow_group(&mut group, 10_000, &REFLOW_WIDTH_CHANGES);
    group.finish();
}

fn bench_reflow_full_depth_100k(c: &mut Criterion) {
    // Opt-in: see the section comment. Runs only when a filter names `100k`.
    let wanted = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .any(|f| f.contains("100k"));
    if !wanted {
        return;
    }
    let mut group = c.benchmark_group("reflow_full_depth_100k");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(20));
    run_reflow_group(&mut group, 100_000, &REFLOW_WIDTH_CHANGES[2..]);
    group.finish();
}

// ---------------------------------------------------------------
// Criterion bootstrap
// ---------------------------------------------------------------
criterion_group!(
    name = benches;
    config = configure();
    targets =
        bench_insert_large_line,
        bench_insert_chunks,
        bench_resize,
        bench_softwrap_heavy,
        bench_visible_flatten,
        bench_scrollback_flatten,
        bench_flatten_url_heavy,
        bench_flatten_wrapped_url_heavy,
        bench_insert_with_color_changes,
        bench_cursor_ops,
        bench_move_cursor_relative,
        bench_lf_heavy,
        bench_lf_flatten_at_capacity,
        bench_lf_batch_then_flatten_at_capacity,
        bench_erase_display,
        bench_scrollback_render,
        bench_alternate_screen_switch,
        bench_erase_display_full,
        bench_lf_heavy_bce,
        bench_erase_display_bce,
        bench_command_block_record,
        bench_image_store_insert_at_quota,
        bench_compressed_block_round_trip,
        bench_scroll_into_compressed_region,
        bench_idle_compaction_tick,
        bench_idle_compression_tick,
        bench_lf_eviction_at_capacity,
        bench_lf_eviction_scaling,
        bench_lf_eviction_long_run,
        bench_reflow_full_depth,
        bench_reflow_full_depth_100k,
);

criterion_main!(benches);
