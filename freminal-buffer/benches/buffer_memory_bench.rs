// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 118 phase-one memory measurement harness.
//!
//! This bench is not primarily a *timing* benchmark: its main deliverable is
//! the printed per-corpus heap-bytes-per-scrollback-line report, captured as
//! the "before" baseline ahead of the upcoming compact-cell-representation
//! change (Task 118). A trivial Criterion `bench_function` per corpus is
//! still registered so the file participates in `cargo bench` normally.
//!
//! Three synthetic corpora are built, matching the plan's three categories:
//!
//! - `shell_session`: mostly-default-formatting lines resembling shell
//!   prompts and command output (no SGR changes).
//! - `source_logs`: plain ASCII source/log-like lines of varying length,
//!   still mostly default formatting.
//! - `high_entropy_colored`: worst case — the format tag is changed every
//!   few characters via `Buffer::set_format`, producing many `FormatTag`
//!   runs per line.

use criterion::{Criterion, criterion_group, criterion_main};

use freminal_buffer::buffer::Buffer;
use freminal_common::buffer_states::{cursor::StateColors, format_tag::FormatTag, tchar::TChar};
use freminal_common::colors::TerminalColor;

const LINE_COUNT: usize = 5_000;
const WIDTH: usize = 80;
const HEIGHT: usize = 24;

/// Insert `lines` into a fresh `WIDTH`x`HEIGHT` buffer, one logical row per
/// string, using explicit `handle_lf` + `handle_cr` between lines.
///
/// NOTE: embedding `TChar::NewLine` inside a single `insert_text` call does
/// **not** create a new row — `Buffer::insert_text` only wraps at `width`,
/// and the flatten pass (`visible_as_tchars_and_tags` /
/// `scrollback_as_tchars_and_tags`) inserts `TChar::NewLine` as an *output*
/// row separator, not as an input row-break signal. Real row breaks require
/// `handle_lf`/`handle_cr`, exactly as the PTY-driving code does for every
/// actual newline byte.
fn build_buffer_from_lines(lines: impl Iterator<Item = String>) -> Buffer {
    let mut buf = Buffer::new(WIDTH, HEIGHT);
    for line in lines {
        let chars: Vec<TChar> = line.bytes().map(TChar::Ascii).collect();
        buf.insert_text(&chars);
        buf.handle_lf();
        buf.handle_cr();
    }
    buf
}

/// Build the `shell_session` corpus: ~5000 lines of plain-ASCII shell
/// prompts and output, one `FormatTag` run per line (no SGR changes).
fn build_shell_session_buffer() -> Buffer {
    build_buffer_from_lines((0..LINE_COUNT).map(|i| {
        if i % 3 == 0 {
            format!("user@host:~/project$ ls -la target/{i}")
        } else if i % 3 == 1 {
            format!("-rw-r--r-- 1 user user {i:>6} Jul 13 12:00 file_{i}.rs")
        } else {
            format!("total {} files, {} bytes used", i % 50, i * 128)
        }
    }))
}

/// Build the `source_logs` corpus: ~5000 lines of source-code / log-like
/// ASCII text with varying line lengths (some short, some ~80 cols), still
/// mostly default formatting.
fn build_source_logs_buffer() -> Buffer {
    build_buffer_from_lines((0..LINE_COUNT).map(|i| match i % 5 {
        0 => format!("fn handler_{i}() {{"),
        1 => format!("    let result = process_request(request_{i}, &ctx)?; // step {i}"),
        2 => "}".to_string(),
        3 => format!(
            "[{i:>08}] INFO  connection accepted from 10.0.{}.{} on port 84{}",
            i % 256,
            (i * 7) % 256,
            i % 10
        ),
        _ => String::new(),
    }))
}

/// Build the `high_entropy_colored` corpus by inserting text in small
/// batches, calling `Buffer::set_format` between batches to vary the
/// foreground color every few characters. This is the worst case for the
/// per-cell `FormatTag` representation: many short `FormatTag` runs per row.
fn build_high_entropy_colored_buffer() -> Buffer {
    const SEGMENT: usize = 4;
    let colors = [
        TerminalColor::Custom(255, 0, 0),
        TerminalColor::Custom(0, 255, 0),
        TerminalColor::Custom(0, 0, 255),
        TerminalColor::Custom(255, 255, 0),
        TerminalColor::Custom(0, 255, 255),
        TerminalColor::Custom(255, 0, 255),
    ];

    let mut buf = Buffer::new(WIDTH, HEIGHT);
    let mut color_idx = 0usize;

    for i in 0..LINE_COUNT {
        let mut col = 0usize;
        while col < WIDTH {
            let color = colors[color_idx % colors.len()];
            color_idx += 1;
            let tag = FormatTag {
                start: 0,
                end: usize::MAX,
                colors: StateColors {
                    color,
                    ..StateColors::default()
                },
                ..FormatTag::default()
            };
            buf.set_format(tag);

            let seg_len = SEGMENT.min(WIDTH - col);
            let chars: Vec<TChar> = (0..seg_len)
                .map(|j| TChar::Ascii(b'a' + ((i + j) % 26) as u8))
                .collect();
            buf.insert_text(&chars);
            col += seg_len;
        }
        buf.handle_lf();
        buf.handle_cr();
    }

    buf
}

/// Warm the row-flatten cache the same way steady-state GUI rendering would:
/// one full scrollback flatten and one full visible flatten.
fn warm_cache(buf: &mut Buffer) {
    let _ = buf.scrollback_as_tchars_and_tags(0);
    let _ = buf.visible_as_tchars_and_tags(0);
}

/// Print a readable memory report for one corpus.
fn print_report(label: &str, buf: &mut Buffer) {
    warm_cache(buf);
    let breakdown = buf.heap_bytes();

    let total_bytes = breakdown.rows_bytes + breakdown.row_cache_bytes + breakdown.url_bytes;
    let bytes_per_scrollback_line = total_bytes
        .checked_div(breakdown.scrollback_lines)
        .unwrap_or(0);

    println!("==================================================================");
    println!(" Buffer memory report: {label}");
    println!("------------------------------------------------------------------");
    println!(" total_rows          : {}", breakdown.total_rows);
    println!(" scrollback_lines    : {}", breakdown.scrollback_lines);
    println!(" rows_bytes          : {}", breakdown.rows_bytes);
    println!(" row_cache_bytes     : {}", breakdown.row_cache_bytes);
    println!(" url_bytes           : {}", breakdown.url_bytes);
    println!(" total_bytes         : {total_bytes}");
    println!(" bytes/scrollback_ln : {bytes_per_scrollback_line}");
    println!("==================================================================");
}

fn bench_memory_shell_session(c: &mut Criterion) {
    let mut buf = build_shell_session_buffer();

    print_report("shell_session", &mut buf);

    c.bench_function("memory_report_shell_session", |b| {
        b.iter(|| {
            std::hint::black_box(buf.heap_bytes());
        });
    });
}

fn bench_memory_source_logs(c: &mut Criterion) {
    let mut buf = build_source_logs_buffer();

    print_report("source_logs", &mut buf);

    c.bench_function("memory_report_source_logs", |b| {
        b.iter(|| {
            std::hint::black_box(buf.heap_bytes());
        });
    });
}

fn bench_memory_high_entropy_colored(c: &mut Criterion) {
    let mut buf = build_high_entropy_colored_buffer();

    print_report("high_entropy_colored", &mut buf);

    c.bench_function("memory_report_high_entropy_colored", |b| {
        b.iter(|| {
            std::hint::black_box(buf.heap_bytes());
        });
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default().sample_size(10);
    targets =
        bench_memory_shell_session,
        bench_memory_source_logs,
        bench_memory_high_entropy_colored,
);

criterion_main!(benches);
