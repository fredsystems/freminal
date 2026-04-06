// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use freminal_buffer::buffer::Buffer;
use freminal_common::buffer_states::{
    cursor::StateColors, fonts::FontWeight, format_tag::FormatTag, tchar::TChar,
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
// Benchmark: flatten visible rows → (Vec<TChar>, Vec<FormatTag>)
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
                            font_decorations: Vec::new(),
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
    // Push 4 100 LFs (just past the default 4 000-line scrollback limit) to
    // exercise `handle_lf` and `enforce_scrollback_limit` together.
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
    // Measure the cost of enter_alternate and leave_alternate on a populated buffer.
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
                buf.enter_alternate(0);
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("leave_alternate", |b| {
        b.iter_batched(
            || {
                let mut buf = Buffer::new(80, 24);
                buf.insert_text(&primary_data);
                buf.enter_alternate(0);
                buf.insert_text(&alt_data);
                buf
            },
            |mut buf| {
                std::hint::black_box(buf.leave_alternate());
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
        bench_insert_with_color_changes,
        bench_cursor_ops,
        bench_lf_heavy,
        bench_erase_display,
        bench_scrollback_render,
        bench_alternate_screen_switch,
        bench_erase_display_full,
);

criterion_main!(benches);
