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
use freminal::gui::atlas::GlyphAtlas;
use freminal::gui::font_manager::FontManager;
use freminal::gui::renderer::{
    BackgroundFrame, FgRenderOptions, build_background_instances, build_foreground_instances,
};
use freminal::gui::shaping::ShapingCache;
use freminal_common::config::Config;
use freminal_common::cursor::CursorVisualStyle;
use freminal_common::themes::CATPPUCCIN_MOCHA;
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
// bench_shaping_ligatures
// ---------------------------------------------------------------

/// Build a ligature-heavy payload: lines containing `->`, `=>`, `!=`, `==`,
/// `<=`, `>=`, `::`, `//`, `&&`, `||` interspersed with regular text.
fn ligature_heavy_visible_chars(
    width: usize,
    height: usize,
) -> (
    Vec<freminal_common::buffer_states::tchar::TChar>,
    Vec<freminal_common::buffer_states::format_tag::FormatTag>,
) {
    use freminal_common::buffer_states::format_tag::FormatTag;
    use freminal_common::buffer_states::tchar::TChar;

    let ligature_sequences = ["->", "=>", "!=", "==", "<=", ">=", "::", "//", "&&", "||"];
    let mut chars = Vec::with_capacity(width * height + height);
    let mut col = 0;
    let mut line = 0;

    while line < height {
        if col + 4 <= width {
            // Insert "a" + ligature_seq + "b" pattern
            let seq = ligature_sequences[(col / 4 + line) % ligature_sequences.len()];
            chars.push(TChar::Ascii(b'a'));
            for b in seq.bytes() {
                chars.push(TChar::Ascii(b));
            }
            chars.push(TChar::Ascii(b'b'));
            col += 2 + seq.len();
        } else {
            // Fill remaining with ASCII
            while col < width {
                chars.push(TChar::Ascii(b'x'));
                col += 1;
            }
        }

        if col >= width {
            chars.push(TChar::NewLine);
            col = 0;
            line += 1;
        }
    }

    let tags = vec![FormatTag {
        start: 0,
        end: chars.len(),
        ..FormatTag::default()
    }];

    (chars, tags)
}

fn bench_shaping_ligatures(c: &mut Criterion) {
    let width = 80;
    let height = 50;
    let (chars, tags) = ligature_heavy_visible_chars(width, height);
    let total_chars = (width * height) as u64;

    let mut group = c.benchmark_group("shaping_ligatures");
    group.throughput(Throughput::Elements(total_chars));

    for ligatures in [false, true] {
        let label = if ligatures {
            "ligatures_on"
        } else {
            "ligatures_off"
        };
        group.bench_function(BenchmarkId::new("shape_visible", label), |b| {
            b.iter_batched(
                || {
                    let fm = FontManager::new(&Config::default(), 1.0).unwrap();
                    let cache = ShapingCache::new();
                    (fm, cache)
                },
                |(mut fm, mut cache)| {
                    #[allow(clippy::cast_precision_loss)]
                    let cell_w = fm.cell_width() as f32;
                    std::hint::black_box(cache.shape_visible(
                        &chars,
                        &tags,
                        width,
                        &mut fm,
                        cell_w,
                        ligatures,
                        &[],
                    ));
                },
                BatchSize::SmallInput,
            );
        });
    }

    // Also benchmark cache hit path (second call with same data).
    group.bench_function("shape_visible_cache_hit", |b| {
        let mut fm = FontManager::new(&Config::default(), 1.0).unwrap();
        let mut cache = ShapingCache::new();
        #[allow(clippy::cast_precision_loss)]
        let cell_w = fm.cell_width() as f32;
        // Prime the cache.
        let _ = cache.shape_visible(&chars, &tags, width, &mut fm, cell_w, false, &[]);

        b.iter(|| {
            std::hint::black_box(cache.shape_visible(
                &chars,
                &tags,
                width,
                &mut fm,
                cell_w,
                false,
                &[],
            ));
        });
    });

    // Partial-dirty path (issue #405 Part B): prime the cache, then change a
    // single visible row's content each sample and re-shape. `ShapingCache` is
    // per-row content-hashed, so only the one changed row re-shapes (rustybuzz)
    // while every other row is an `Arc::clone` hit. This isolates the shaping
    // stage's genuinely-incremental behaviour — in contrast to the downstream
    // vertex builders (`build_bg_instances`/`build_fg_instances`), which are
    // NOT per-row incremental and rebuild all instances regardless (see
    // `instanced_bg`/`instanced_fg`). Comparing this against
    // `shape_visible_cache_hit` (0 changed) and the cold shape (all changed)
    // quantifies the per-row shaping win that the deferred vertex-incremental
    // work would need to preserve end-to-end.
    {
        let width = 200usize;
        let height = 50usize;
        let (base_chars, base_tags) = ligature_heavy_visible_chars(width, height);
        group.bench_function("shape_visible_partial_dirty_200x50", |b| {
            b.iter_batched(
                || {
                    let mut fm = FontManager::new(&Config::default(), 1.0).unwrap();
                    let mut cache = ShapingCache::new();
                    #[allow(clippy::cast_precision_loss)]
                    let cell_w = fm.cell_width() as f32;
                    // Prime the cache with the base content.
                    let _ = cache.shape_visible(
                        &base_chars,
                        &base_tags,
                        width,
                        &mut fm,
                        cell_w,
                        false,
                        &[],
                    );
                    // Produce a variant with exactly one row changed. Rows are
                    // NewLine-delimited; flip one character on the ~middle row.
                    let mut edited = base_chars.clone();
                    let newline_positions: Vec<usize> = edited
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| {
                            matches!(c, freminal_common::buffer_states::tchar::TChar::NewLine)
                        })
                        .map(|(i, _)| i)
                        .collect();
                    // First char of the middle row (after the preceding newline).
                    let mid = newline_positions.len() / 2;
                    let target = if mid == 0 {
                        0
                    } else {
                        newline_positions[mid - 1] + 1
                    };
                    if let Some(slot) = edited.get_mut(target) {
                        *slot = freminal_common::buffer_states::tchar::TChar::Ascii(b'Q');
                    }
                    (fm, cache, cell_w, edited)
                },
                |(mut fm, mut cache, cell_w, edited)| {
                    std::hint::black_box(cache.shape_visible(
                        &edited,
                        &base_tags,
                        width,
                        &mut fm,
                        cell_w,
                        false,
                        &[],
                    ));
                },
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

// ---------------------------------------------------------------
// Helper: build shaped lines for a given terminal size
// ---------------------------------------------------------------

/// Build shaped lines from an ANSI-heavy payload at the given terminal size.
///
/// Returns `(shaped_lines, font_manager)` — the `FontManager` is needed by the
/// foreground instance builder.
fn build_shaped_lines_for_size(
    width: usize,
    height: usize,
) -> (
    Vec<std::sync::Arc<freminal::gui::shaping::ShapedLine>>,
    FontManager,
) {
    let (chars, tags) = ligature_heavy_visible_chars(width, height);
    let mut fm = FontManager::new(&Config::default(), 1.0).unwrap();
    let mut cache = ShapingCache::new();
    #[allow(clippy::cast_precision_loss)]
    let cell_w = fm.cell_width() as f32;
    let lines = cache.shape_visible(&chars, &tags, width, &mut fm, cell_w, false, &[]);
    (lines, fm)
}

// ---------------------------------------------------------------
// bench_bg_instances — instanced background buffer builder
// ---------------------------------------------------------------
fn bench_bg_instances(c: &mut Criterion) {
    let sizes: &[(usize, usize)] = &[(80, 24), (200, 50)];

    let mut group = c.benchmark_group("instanced_bg");

    for &(width, height) in sizes {
        let (lines, fm) = build_shaped_lines_for_size(width, height);
        let total_cells = (width * height) as u64;

        group.throughput(Throughput::Elements(total_cells));

        let cell_width = fm.cell_width();
        let cell_height = fm.cell_height();
        let ascent = fm.ascent();
        let underline_offset = fm.underline_offset();
        let strikeout_offset = fm.strikeout_offset();
        let stroke_size = fm.stroke_size();

        let cursor_pixel_pos = (0.0_f32, 0.0_f32);
        let cursor_style = CursorVisualStyle::BlockCursorSteady;

        group.bench_function(
            BenchmarkId::new("build_bg_instances", format!("{width}x{height}")),
            |b| {
                let mut instances = Vec::new();
                let mut deco = Vec::new();
                b.iter(|| {
                    build_background_instances(
                        &BackgroundFrame {
                            shaped_lines: &lines,
                            cell_width,
                            cell_height,
                            ascent,
                            underline_offset,
                            strikeout_offset,
                            stroke_size,
                            show_cursor: true,
                            cursor_blink_on: true,
                            cursor_pixel_pos,
                            cursor_width_scale: 1.0,
                            cursor_visual_style: &cursor_style,
                            selection: None,
                            selection_is_block: false,
                            match_highlights: &[],
                            command_block_hover_rows: None,
                            term_width_cols: 0,
                            theme: &CATPPUCCIN_MOCHA,
                            cursor_color_override: None,
                            reverse_screen: false,
                        },
                        &mut instances,
                        &mut deco,
                    );
                    std::hint::black_box(instances.len());
                    std::hint::black_box(deco.len());
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------
// bench_fg_instances — instanced foreground buffer builder
// ---------------------------------------------------------------
fn bench_fg_instances(c: &mut Criterion) {
    let sizes: &[(usize, usize)] = &[(80, 24), (200, 50)];

    let mut group = c.benchmark_group("instanced_fg");

    for &(width, height) in sizes {
        let (lines, fm) = build_shaped_lines_for_size(width, height);
        let total_cells = (width * height) as u64;

        group.throughput(Throughput::Elements(total_cells));

        let cell_height = fm.cell_height();
        let ascent = fm.ascent();
        let opts = FgRenderOptions::all_visible(None);

        group.bench_function(
            BenchmarkId::new("build_fg_instances", format!("{width}x{height}")),
            |b| {
                b.iter_batched(
                    || (GlyphAtlas::default(), Vec::new()),
                    |(mut atlas, mut instances)| {
                        build_foreground_instances(
                            &lines,
                            &mut atlas,
                            &fm,
                            cell_height,
                            ascent,
                            &opts,
                            &CATPPUCCIN_MOCHA,
                            &mut instances,
                        );
                        std::hint::black_box(instances.len());
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------
// bench_bg_instances_partial_dirty — all-rows-vs-one-row headroom
// (issue #405 Part C item 2) for `build_background_instances`.
// ---------------------------------------------------------------
//
// `build_background_instances` `clear()`s its output buffers and walks every
// visible row unconditionally — there is no per-row incremental path today.
// This benchmark measures the theoretical recoverable cost: building
// instances for the FULL 200x50 `shaped_lines` slice vs. building instances
// for a slice of exactly ONE row. The delta between the two is the headroom
// a future per-row incremental build could recover; it is not itself an
// incremental build (none exists yet).
fn bench_bg_instances_partial_dirty(c: &mut Criterion) {
    let width = 200;
    let height = 50;
    let (lines, fm) = build_shaped_lines_for_size(width, height);

    let mut group = c.benchmark_group("instanced_bg_partial_dirty");

    let cell_width = fm.cell_width();
    let cell_height = fm.cell_height();
    let ascent = fm.ascent();
    let underline_offset = fm.underline_offset();
    let strikeout_offset = fm.strikeout_offset();
    let stroke_size = fm.stroke_size();

    let cursor_pixel_pos = (0.0_f32, 0.0_f32);
    let cursor_style = CursorVisualStyle::BlockCursorSteady;

    group.throughput(Throughput::Elements((width * height) as u64));
    group.bench_function(
        BenchmarkId::new("build_bg_instances_all_rows", "200x50"),
        |b| {
            let mut instances = Vec::new();
            let mut deco = Vec::new();
            b.iter(|| {
                build_background_instances(
                    &BackgroundFrame {
                        shaped_lines: &lines,
                        cell_width,
                        cell_height,
                        ascent,
                        underline_offset,
                        strikeout_offset,
                        stroke_size,
                        show_cursor: true,
                        cursor_blink_on: true,
                        cursor_pixel_pos,
                        cursor_width_scale: 1.0,
                        cursor_visual_style: &cursor_style,
                        selection: None,
                        selection_is_block: false,
                        match_highlights: &[],
                        command_block_hover_rows: None,
                        term_width_cols: 0,
                        theme: &CATPPUCCIN_MOCHA,
                        cursor_color_override: None,
                        reverse_screen: false,
                    },
                    &mut instances,
                    &mut deco,
                );
                std::hint::black_box(instances.len());
                std::hint::black_box(deco.len());
            });
        },
    );

    group.throughput(Throughput::Elements(width as u64));
    let single_row = &lines[24..25];
    group.bench_function(
        BenchmarkId::new("build_bg_instances_one_row", "200x50"),
        |b| {
            let mut instances = Vec::new();
            let mut deco = Vec::new();
            b.iter(|| {
                build_background_instances(
                    &BackgroundFrame {
                        shaped_lines: single_row,
                        cell_width,
                        cell_height,
                        ascent,
                        underline_offset,
                        strikeout_offset,
                        stroke_size,
                        show_cursor: true,
                        cursor_blink_on: true,
                        cursor_pixel_pos,
                        cursor_width_scale: 1.0,
                        cursor_visual_style: &cursor_style,
                        selection: None,
                        selection_is_block: false,
                        match_highlights: &[],
                        command_block_hover_rows: None,
                        term_width_cols: 0,
                        theme: &CATPPUCCIN_MOCHA,
                        cursor_color_override: None,
                        reverse_screen: false,
                    },
                    &mut instances,
                    &mut deco,
                );
                std::hint::black_box(instances.len());
                std::hint::black_box(deco.len());
            });
        },
    );

    group.finish();
}

// ---------------------------------------------------------------
// bench_fg_instances_partial_dirty — all-rows-vs-one-row headroom
// (issue #405 Part C item 2) for `build_foreground_instances`.
// ---------------------------------------------------------------
//
// Same rationale as `bench_bg_instances_partial_dirty`, applied to
// `build_foreground_instances`: builds instances for the full 200x50
// `shaped_lines` slice vs. a slice of exactly one row, quantifying the
// per-row headroom a future incremental foreground build could recover.
fn bench_fg_instances_partial_dirty(c: &mut Criterion) {
    let width = 200;
    let height = 50;
    let (lines, fm) = build_shaped_lines_for_size(width, height);

    let mut group = c.benchmark_group("instanced_fg_partial_dirty");

    let cell_height = fm.cell_height();
    let ascent = fm.ascent();
    let opts = FgRenderOptions::all_visible(None);

    group.throughput(Throughput::Elements((width * height) as u64));
    group.bench_function(
        BenchmarkId::new("build_fg_instances_all_rows", "200x50"),
        |b| {
            b.iter_batched(
                || (GlyphAtlas::default(), Vec::new()),
                |(mut atlas, mut instances)| {
                    build_foreground_instances(
                        &lines,
                        &mut atlas,
                        &fm,
                        cell_height,
                        ascent,
                        &opts,
                        &CATPPUCCIN_MOCHA,
                        &mut instances,
                    );
                    std::hint::black_box(instances.len());
                },
                BatchSize::SmallInput,
            );
        },
    );

    group.throughput(Throughput::Elements(width as u64));
    let single_row = &lines[24..25];
    group.bench_function(
        BenchmarkId::new("build_fg_instances_one_row", "200x50"),
        |b| {
            b.iter_batched(
                || (GlyphAtlas::default(), Vec::new()),
                |(mut atlas, mut instances)| {
                    build_foreground_instances(
                        single_row,
                        &mut atlas,
                        &fm,
                        cell_height,
                        ascent,
                        &opts,
                        &CATPPUCCIN_MOCHA,
                        &mut instances,
                    );
                    std::hint::black_box(instances.len());
                },
                BatchSize::SmallInput,
            );
        },
    );

    group.finish();
}

// ---------------------------------------------------------------
// bench_shape_placeholder_line — fold-placeholder shaping
// ---------------------------------------------------------------
//
// Measures the cost of shaping a single fold-placeholder row, which is
// invoked once per folded command block on the full-rebuild path. Folded
// workspaces with many command blocks (Task 72.10) call this for each
// collapsed range; the per-line cost must stay well below the per-row
// cost of `shape_visible` for it to remain free.
fn bench_shape_placeholder_line(c: &mut Criterion) {
    use freminal::gui::shaping::shape_placeholder_line;
    use freminal_common::colors::TerminalColor;

    let mut group = c.benchmark_group("shape_placeholder_line");
    // One element per call — each invocation shapes one row.
    group.throughput(Throughput::Elements(1));

    // Typical case: width 80, 7 lines hidden.
    group.bench_function("typical_w80", |b| {
        b.iter_batched(
            || FontManager::new(&Config::default(), 1.0).unwrap(),
            |mut fm| {
                #[allow(clippy::cast_precision_loss)]
                let cell_w = fm.cell_width() as f32;
                let text = "▶ 7 lines hidden — click to unfold".to_string();
                std::hint::black_box(shape_placeholder_line(
                    &text,
                    TerminalColor::BrightBlack,
                    &mut fm,
                    cell_w,
                    false,
                ));
            },
            BatchSize::SmallInput,
        );
    });

    // Wide terminal — still one row, same text.
    group.bench_function("wide_w200", |b| {
        b.iter_batched(
            || FontManager::new(&Config::default(), 1.0).unwrap(),
            |mut fm| {
                #[allow(clippy::cast_precision_loss)]
                let cell_w = fm.cell_width() as f32;
                let text = "▶ 1234 lines hidden — click to unfold".to_string();
                std::hint::black_box(shape_placeholder_line(
                    &text,
                    TerminalColor::BrightBlack,
                    &mut fm,
                    cell_w,
                    false,
                ));
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------
// Chrome style: build_visuals (Task 112.4 / 112.5)
// ---------------------------------------------------------------
//
// `build_visuals` maps a `(GuiTheme, ThemePalette)` pair to a fully-specified
// egui `Visuals`.  It is invoked by the per-frame style hook in
// `app_impl::update`, but **only when the style cache key changes** (theme /
// profile / opacity).  On the steady-state path the cache short-circuits the
// call entirely, so this measures the worst case: the cost paid on a
// theme/profile switch, and the cost that *would* be paid every frame if the
// cache regressed.  It must be far below a frame budget so that even a
// per-frame call (a cache regression) would be invisible.
fn bench_build_visuals(c: &mut Criterion) {
    use freminal::gui::chrome_style::build_visuals;
    use freminal_common::gui_theme::StyleProfile;

    let mut group = c.benchmark_group("build_visuals");
    group.throughput(Throughput::Elements(1));

    for (label, profile) in [
        ("modern", StyleProfile::Modern),
        ("retro", StyleProfile::Retro),
    ] {
        let gui_theme = profile.defaults();
        group.bench_function(BenchmarkId::new("normal_display", label), |b| {
            b.iter(|| {
                std::hint::black_box(build_visuals(
                    std::hint::black_box(&gui_theme),
                    std::hint::black_box(&CATPPUCCIN_MOCHA),
                    std::hint::black_box(1.0),
                ));
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------
// bench_image_animation_tick — GUI wall-clock animation frame selector
// (Task 100.2c). Measures the per-frame CPU cost of
// `ViewState::tick_image_animations` against a representative set of
// concurrently-animating kitty images (e.g. multiple animated GIFs/spinners
// visible at once).
// ---------------------------------------------------------------
fn bench_image_animation_tick(c: &mut Criterion) {
    use freminal::gui::view_state::ViewState;
    use freminal_terminal_emulator::{
        AnimationControl, AnimationRunMode, ImageFrame, ImageSizeMode, InlineImage,
    };
    use std::collections::HashMap;

    const NUM_IMAGES: u64 = 8;
    const EXTRA_FRAMES: usize = 9; // frame_count() == 10
    const GAP_MS: u32 = 40;

    let mut images: HashMap<u64, InlineImage> = HashMap::new();
    for id in 1..=NUM_IMAGES {
        let frames = (0..EXTRA_FRAMES)
            .map(|_| ImageFrame {
                pixels: Arc::new(vec![0u8; 4]),
                gap_ms: GAP_MS,
            })
            .collect();
        images.insert(
            id,
            InlineImage {
                id,
                pixels: Arc::new(vec![0u8; 4]),
                width_px: 1,
                height_px: 1,
                display_cols: 1,
                display_rows: 1,
                size_mode: ImageSizeMode::NativePixels,
                frames,
                root_gap_ms: GAP_MS,
                animation: AnimationControl {
                    run_mode: AnimationRunMode::Running,
                    loop_count: 1,
                    current_frame: 0,
                },
            },
        );
    }

    let mut group = c.benchmark_group("image_animation_tick");
    group.throughput(Throughput::Elements(NUM_IMAGES));
    group.bench_function("tick_8_images_10_frames", |b| {
        b.iter_batched(
            ViewState::default,
            |mut view_state| {
                std::hint::black_box(view_state.tick_image_animations(&images));
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

// ---------------------------------------------------------------
// bench_build_image_verts — image-quad vertex generation (Task 100.17b).
// Measures the per-frame CPU cost of `build_image_verts` (which calls
// `compute_image_quad` per image, including the Task 100.17b size-mode
// branch) against a representative set of concurrently-displayed images
// stamped across the cell grid.
// ---------------------------------------------------------------
fn bench_build_image_verts(c: &mut Criterion) {
    use freminal::gui::renderer::build_image_verts;
    use freminal_terminal_emulator::{
        AnimationControl, ImagePlacement, ImageProtocol, ImageSizeMode, InlineImage,
    };
    use std::collections::HashMap;

    const NUM_IMAGES: u64 = 8;
    const IMG_COLS: usize = 4;
    const IMG_ROWS: usize = 4;
    const TERM_WIDTH: usize = 80;
    const TERM_ROWS: usize = IMG_ROWS;
    const CELL_WIDTH: u32 = 8;
    const CELL_HEIGHT: u32 = 16;

    let mut images: HashMap<u64, InlineImage> = HashMap::new();
    for id in 1..=NUM_IMAGES {
        images.insert(
            id,
            InlineImage {
                id,
                pixels: Arc::new(vec![0u8; 4]),
                width_px: 20,
                height_px: 40,
                display_cols: IMG_COLS,
                display_rows: IMG_ROWS,
                size_mode: ImageSizeMode::NativePixels,
                frames: Vec::new(),
                root_gap_ms: 0,
                animation: AnimationControl::default(),
            },
        );
    }

    // Stamp each image into its own non-overlapping IMG_COLSxIMG_ROWS block
    // of the cell grid: image `id` (1-indexed) occupies columns
    // `(id-1)*IMG_COLS .. id*IMG_COLS`, all `TERM_ROWS` rows.
    let mut placements: Vec<Option<ImagePlacement>> = vec![None; TERM_WIDTH * TERM_ROWS];
    for id in 1..=NUM_IMAGES {
        let col_base = usize::try_from(id - 1).unwrap_or(0) * IMG_COLS;
        for row in 0..IMG_ROWS {
            for col_in_image in 0..IMG_COLS {
                let col = col_base + col_in_image;
                let cell_idx = row * TERM_WIDTH + col;
                placements[cell_idx] = Some(ImagePlacement {
                    image_id: id,
                    col_in_image,
                    row_in_image: row,
                    protocol: ImageProtocol::Kitty,
                    image_number: None,
                    placement_id: None,
                    z_index: 0,
                    source_crop: None,
                    // Distinct per-image instance id (Task 100.18) — each
                    // of the 8 images is its own separate on-screen
                    // placement, so bucketing must keep them as 8 separate
                    // draw_order entries, matching the pre-100.18 (keyed by
                    // image_id) benchmark workload shape.
                    placement_instance: id,
                    subcell_offset: None,
                });
            }
        }
    }

    let mut group = c.benchmark_group("build_image_verts");
    group.throughput(Throughput::Elements(NUM_IMAGES));
    group.bench_function("8_images_4x4_cells", |b| {
        b.iter_batched(
            || (Vec::new(), Vec::new()),
            |(mut verts, mut draw_order)| {
                build_image_verts(
                    std::hint::black_box(&placements),
                    std::hint::black_box(&images),
                    TERM_WIDTH,
                    CELL_WIDTH,
                    CELL_HEIGHT,
                    &mut verts,
                    &mut draw_order,
                );
                std::hint::black_box((verts, draw_order));
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
        bench_feed_data_incremental,
        bench_feed_data_ansi_heavy,
        bench_feed_data_bursty,
        bench_build_snapshot_after_feed,
        bench_arcswap_roundtrip,
        bench_shaping_ligatures,
        bench_bg_instances,
        bench_fg_instances,
        bench_bg_instances_partial_dirty,
        bench_fg_instances_partial_dirty,
        bench_shape_placeholder_line,
        bench_build_visuals,
        bench_image_animation_tick,
        bench_build_image_verts,
);

criterion_main!(benches);
