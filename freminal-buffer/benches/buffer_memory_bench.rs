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

/// Current process resident set size (RSS) in bytes, or `None` if unavailable.
///
/// Reads `/proc/self/statm` (field 2 = resident pages) on Linux. This is the
/// number a process monitor like `btop` reports, and — crucially — it can
/// diverge sharply from the buffer's own `heap_bytes()` accounting: freeing a
/// `Vec` in Rust returns the memory to the allocator, but glibc retains those
/// pages in its arenas rather than returning them to the OS, so RSS stays high
/// while the internal accounting drops. That divergence is exactly the class
/// of bug the accounting-only harness missed, so this measures the real thing.
///
/// Returns `None` on non-Linux (no portable `/proc/self/statm`); callers skip
/// the RSS line there.
#[cfg(target_os = "linux")]
fn process_rss_bytes() -> Option<usize> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: usize = statm.split_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf(_SC_PAGESIZE) is a pure query with no preconditions.
    let page_size = unsafe { libc_sysconf_pagesize() };
    Some(resident_pages * page_size)
}

#[cfg(not(target_os = "linux"))]
fn process_rss_bytes() -> Option<usize> {
    None
}

/// Page size via `sysconf(_SC_PAGESIZE)`. Declared inline to avoid a `libc`
/// dev-dependency just for one constant.
#[cfg(target_os = "linux")]
unsafe fn libc_sysconf_pagesize() -> usize {
    unsafe extern "C" {
        fn sysconf(name: core::ffi::c_int) -> core::ffi::c_long;
    }
    // _SC_PAGESIZE == 30 on Linux/glibc.
    const SC_PAGESIZE: core::ffi::c_int = 30;
    let v = unsafe { sysconf(SC_PAGESIZE) };
    if v > 0 { v as usize } else { 4096 }
}

/// Ask the allocator to return free pages to the OS (glibc `malloc_trim`), so
/// an RSS reading reflects live memory rather than allocator-retained free
/// pages. Mirrors the live app's post-idle-compaction trim. No-op off glibc.
fn trim_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: `malloc_trim` only releases already-free heap; it cannot
        // affect live allocations.
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> core::ffi::c_int;
        }
        unsafe {
            let _ = malloc_trim(0);
        }
    }
}

/// How often, in logical lines, to flatten the visible window during a fill.
///
/// This is the critical realism knob (see `build_buffer_from_lines`). A real
/// terminal renders every frame while output streams, so `build_snapshot` —
/// and thus `visible_as_tchars_and_tags` — runs repeatedly as rows scroll
/// through the visible window, leaving each row with a populated
/// `RowCacheEntry` that is NOT cleared when the row scrolls into scrollback.
/// A bench that only fills and then flattens once at the end never reproduces
/// that per-row scrollback-cache accumulation (the ~180 MB stale-cache leak a
/// 100k-line scrollback exhibited in the live app), giving falsely optimistic
/// memory numbers. Flattening every few lines during the fill reproduces it.
///
/// ~4 lines per flatten approximates a fast `cat` (many lines per frame)
/// without flattening on literally every line (which no real frame cadence
/// does and which would dominate the fill cost).
const FLATTEN_EVERY_LINES: usize = 4;

/// Insert `lines` into a fresh `WIDTH`x`HEIGHT` buffer, one logical row per
/// string, using explicit `handle_lf` + `handle_cr` between lines, and
/// flattening the visible window every [`FLATTEN_EVERY_LINES`] lines to model
/// real per-frame rendering (see that constant for why this matters).
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
    for (i, line) in lines.enumerate() {
        let chars: Vec<TChar> = line.bytes().map(TChar::Ascii).collect();
        buf.insert_text(&chars);
        buf.handle_lf();
        buf.handle_cr();
        // Model per-frame rendering: flatten the visible window periodically so
        // rows accrue (and then retain) a `RowCacheEntry` as they scroll past,
        // exactly as the live render path does.
        if i % FLATTEN_EVERY_LINES == 0 {
            let _ = buf.visible_as_tchars_and_tags(0);
        }
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
        // Model per-frame rendering during the fill so scrollback rows retain
        // a `RowCacheEntry` (see `FLATTEN_EVERY_LINES`).
        if i % FLATTEN_EVERY_LINES == 0 {
            let _ = buf.visible_as_tchars_and_tags(0);
        }
    }

    buf
}

/// Warm the flatten cache the way *steady-state GUI rendering* actually does.
///
/// This is the important correction over the original harness. The per-frame
/// render path (`TerminalEmulator::build_snapshot`) only ever flattens the
/// **visible window** (`visible_as_tchars_and_tags_extended`); it never reads
/// scrollback cell content. A full-scrollback flatten happens in exactly one
/// place in the whole application: the `RequestSearchBuffer` handler
/// (`data_and_format_data_for_gui`), i.e. only when the user presses Ctrl-F to
/// search scrollback.
///
/// So the realistic resting state is: visible window flattened (cached), all
/// scrollback rows *never touched* since they scrolled off. That is what this
/// models — one visible flatten, no scrollback flatten.
fn warm_steady_state(buf: &mut Buffer) {
    let _ = buf.visible_as_tchars_and_tags(0);
}

/// Warm the caches the way a *Ctrl-F scrollback search* does: flatten the
/// entire scrollback (populating `row_cache`, and — with the compact
/// representation — memoizing every compacted row's decompacted cells too).
///
/// This is the transient worst case, reported separately so it is never
/// conflated with the steady-state number. It is exactly the state Task 118.4
/// (`row_cache` eviction for compacted rows) exists to reclaim.
fn warm_post_search(buf: &mut Buffer) {
    let _ = buf.scrollback_as_tchars_and_tags(0);
    let _ = buf.visible_as_tchars_and_tags(0);
}

/// Run idle scrollback compaction to completion, modelling the state a real
/// terminal settles into ~250ms after output stops.
///
/// Compaction is now a DEFERRED, idle-driven background task (Task 118.9): it
/// no longer runs on any hot path. In the live application the PTY thread's
/// idle tick calls `compact_idle_scrollback` in bounded budgets until the
/// scrollback is fully compacted. A benchmark has no PTY thread, so we invoke
/// it directly with an unbounded budget to reach the same settled state. This
/// is the realistic resting memory of an idle terminal — the number that
/// represents the actual memory win.
fn settle_idle_compaction(buf: &mut Buffer) {
    // One unbounded pass compacts everything the idle tick eventually would.
    let _ = buf.compact_idle_scrollback(usize::MAX);
}

/// Run idle scrollback compaction AND compression to completion, modelling
/// the fully-settled state a real terminal reaches once the PTY thread's
/// idle tick has both compacted (Task 118) and then compressed (Task 119)
/// every eligible cold scrollback row into LZ4 blocks.
///
/// Mirrors the ordering the live idle tick enforces
/// (`freminal/src/gui/pty.rs`): compaction must catch up before compression
/// has anything eligible to compress, so `compact_idle_scrollback` is always
/// called first. This is the number the Task 119 "flat + LZ4" plan table
/// (`Documents/PLAN_VERSION_120.md` ~851-855) is measured against.
fn settle_idle_compression(buf: &mut Buffer) {
    let _ = buf.compact_idle_scrollback(usize::MAX);
    let _ = buf.compress_idle_scrollback(usize::MAX);
}

/// Print one labelled memory report block for a buffer in whatever cache state
/// the caller has already established. Returns bytes/scrollback-line.
///
/// Also reports actual process RSS, trimmed first so the figure reflects live
/// memory rather than allocator-retained free pages. NOTE: RSS is a
/// whole-process number and is only meaningful in *relative* terms here (this
/// bench builds many buffers in one process, so absolute RSS accumulates); the
/// internal `total_bytes` accounting is the precise per-buffer figure. RSS is
/// printed so a large divergence between it and the accounting — the
/// allocator-retention class of bug — is visible rather than hidden.
fn report_block(label: &str, state: &str, buf: &Buffer) -> usize {
    let breakdown = buf.heap_bytes();
    // `blocks_bytes` (Task 119 — Scrollback Compression) is the resident
    // cost of LZ4-compressed blocks evicted out of `rows_bytes`; including it
    // here is what makes this report reflect the compression win rather than
    // just showing `rows_bytes` drop to ~0 for evicted rows with their real
    // content now invisible to the accounting.
    let total_bytes = breakdown.rows_bytes
        + breakdown.row_cache_bytes
        + breakdown.url_bytes
        + breakdown.blocks_bytes;
    let bytes_per_scrollback_line = total_bytes
        .checked_div(breakdown.scrollback_lines)
        .unwrap_or(0);

    trim_allocator();
    let rss = process_rss_bytes();

    println!("==================================================================");
    println!(" Buffer memory report: {label}  [{state}]");
    println!("------------------------------------------------------------------");
    println!(" total_rows          : {}", breakdown.total_rows);
    println!(" scrollback_lines    : {}", breakdown.scrollback_lines);
    println!(" rows_bytes          : {}", breakdown.rows_bytes);
    println!(" row_cache_bytes     : {}", breakdown.row_cache_bytes);
    println!(" url_bytes           : {}", breakdown.url_bytes);
    println!(" blocks_bytes        : {}", breakdown.blocks_bytes);
    println!(" total_bytes         : {total_bytes}");
    println!(" bytes/scrollback_ln : {bytes_per_scrollback_line}");
    match rss {
        Some(bytes) => println!(" process_rss (trimmed): {bytes} ({} MB)", bytes / 1_000_000),
        None => println!(" process_rss (trimmed): n/a (non-Linux)"),
    }
    println!("==================================================================");

    bytes_per_scrollback_line
}

/// Print the four-state memory report for one corpus.
///
/// Each phase builds its OWN buffer and drops it before the next, so buffers
/// never coexist — otherwise their allocations would accumulate and confound
/// the RSS reading (RSS is whole-process). The per-corpus RSS *delta* (idle
/// baseline → settled, both trimmed) isolates what the settled buffer actually
/// costs the OS, which is the figure that diverged from the internal
/// accounting in the live app (allocator retention). Absolute RSS across
/// corpora is still not comparable and is reported only as an advisory.
fn print_report(label: &str, build: impl Fn() -> Buffer) {
    // Trimmed idle RSS baseline before this corpus builds anything.
    trim_allocator();
    let rss_before = process_rss_bytes();

    // (1) Freshly filled, BEFORE idle compaction — the transient uncompacted
    //     state (~250ms right after a burst, PTY thread still busy). Now that
    //     the fill flattens the visible window periodically (see
    //     `FLATTEN_EVERY_LINES`), scrollback rows carry the stale
    //     `RowCacheEntry` they retain in the live app.
    let fresh_bpl = {
        let mut fresh = build();
        warm_steady_state(&mut fresh);
        report_block(label, "fresh fill, pre-idle (transient)", &fresh)
    };

    // (2) SETTLED steady state — idle compaction has run to completion (as the
    //     PTY idle tick would ~250ms after output stops), then the render path
    //     warms the visible window. THIS is the real resident-memory win.
    let (settled_bpl, rss_settled) = {
        let mut settled = build();
        settle_idle_compaction(&mut settled);
        warm_steady_state(&mut settled);
        let bpl = report_block(
            label,
            "settled steady-state (post-idle-compaction)",
            &settled,
        );
        // Read RSS while the settled buffer is still alive, after trimming.
        trim_allocator();
        (bpl, process_rss_bytes())
    };

    // (3) FULLY COMPRESSED — idle compaction AND idle compression (Task 119)
    //     have both run to completion, then the render path warms the visible
    //     window (scrollback stays compressed; only the visible window is
    //     touched, mirroring steady-state rendering). This is the "flat + LZ4"
    //     number from the Task 119 feasibility-spike table
    //     (`Documents/PLAN_VERSION_120.md` ~851-855) made measurable.
    let (compressed_bpl, rss_compressed) = {
        let mut compressed = build();
        settle_idle_compression(&mut compressed);
        warm_steady_state(&mut compressed);
        let bpl = report_block(
            label,
            "settled + compressed (post-idle-compression)",
            &compressed,
        );
        trim_allocator();
        (bpl, process_rss_bytes())
    };

    // (4) SETTLED + post-search — Ctrl-F full-scrollback flatten on the settled
    //     (compaction-only) buffer; eviction (Task 118.4) reclaims the
    //     transient copies, so this returns to the settled number.
    let search_bpl = {
        let mut searched = build();
        settle_idle_compaction(&mut searched);
        warm_post_search(&mut searched);
        report_block(label, "settled + post-search (Ctrl-F)", &searched)
    };

    let rss_delta = match (rss_before, rss_settled) {
        (Some(before), Some(settled)) => {
            format!("{} MB", settled.saturating_sub(before) / 1_000_000)
        }
        _ => "n/a".to_string(),
    };
    let rss_delta_compressed = match (rss_before, rss_compressed) {
        (Some(before), Some(compressed)) => {
            format!("{} MB", compressed.saturating_sub(before) / 1_000_000)
        }
        _ => "n/a".to_string(),
    };

    println!(
        " -> {label}: fresh(pre-idle) {fresh_bpl} B/line, settled {settled_bpl} B/line, \
         settled+compressed {compressed_bpl} B/line, settled+search {search_bpl} B/line, \
         settled RSS delta {rss_delta}, settled+compressed RSS delta {rss_delta_compressed}\n"
    );
}

fn bench_memory_shell_session(c: &mut Criterion) {
    print_report("shell_session", build_shell_session_buffer);

    let mut buf = build_shell_session_buffer();
    warm_steady_state(&mut buf);
    c.bench_function("memory_report_shell_session", |b| {
        b.iter(|| {
            std::hint::black_box(buf.heap_bytes());
        });
    });
}

fn bench_memory_source_logs(c: &mut Criterion) {
    print_report("source_logs", build_source_logs_buffer);

    let mut buf = build_source_logs_buffer();
    warm_steady_state(&mut buf);
    c.bench_function("memory_report_source_logs", |b| {
        b.iter(|| {
            std::hint::black_box(buf.heap_bytes());
        });
    });
}

fn bench_memory_high_entropy_colored(c: &mut Criterion) {
    print_report("high_entropy_colored", build_high_entropy_colored_buffer);

    let mut buf = build_high_entropy_colored_buffer();
    warm_steady_state(&mut buf);
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
