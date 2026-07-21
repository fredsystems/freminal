// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use freminal_buffer::buffer::Buffer;
use freminal_buffer::compact_row::CompactRow;
use freminal_buffer::compressed_block::CompressedBlock;
use freminal_buffer::image_store::{AnimationControl, ImageSizeMode, ImageStore, InlineImage};
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
// Benchmark: LF-heavy scroll with BCE (non-default background)
// ---------------------------------------------------------------
fn bench_lf_heavy_bce(c: &mut Criterion) {
    // Same workload as bench_lf_heavy but with a non-default background
    // color set, exercising the BCE fill path in push_row / handle_lf.
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
);

criterion_main!(benches);
