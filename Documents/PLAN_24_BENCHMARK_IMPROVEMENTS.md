# PLAN_24 — Benchmark Improvements and CI Integration

## Status: In Progress

---

## Overview

Freminal has 26 benchmarks across 3 crates covering the primary hot paths (buffer operations,
parser, emulator, snapshot building). However, there are coverage gaps, fragile benchmarks, no
CI integration for benchmark compilation, and no systematic regression detection workflow.

This task addresses all four areas: adds missing benchmarks, fixes fragile ones, integrates
benchmarks into CI, and updates `agents.md` with benchmark-related rules.

**Dependencies:** None (independent)
**Dependents:** None
**Primary crates:** `freminal`, `freminal-buffer`, `freminal-terminal-emulator`, `xtask`
**Estimated scope:** Medium (7 subtasks)

---

## Current State

### Benchmark Inventory (26 total)

**`freminal-buffer/benches/buffer_row_bench.rs` (10 benchmarks):**

| Benchmark                         | What It Measures                            |
| --------------------------------- | ------------------------------------------- |
| `buffer_insert_large_line`        | Single large insert into buffer             |
| `buffer_insert_chunks`            | Chunked inserts (realistic pattern)         |
| `buffer_resize/reflow_width`      | Width change triggers reflow                |
| `buffer_resize/shrink_height`     | Height reduction                            |
| `softwrap_heavy`                  | Long line soft-wrap                         |
| `bench_visible_flatten`           | `visible_as_tchars_and_tags()` on 200x50    |
| `bench_scrollback_flatten`        | `scrollback_as_tchars_and_tags()` 1024 rows |
| `bench_insert_with_color_changes` | Insert with frequent SGR changes            |
| `bench_cursor_ops`                | CUP + data interleaved                      |
| `bench_lf_heavy`                  | 4100 LF operations                          |
| `bench_erase_display`             | ED (erase to end)                           |

**`freminal-terminal-emulator/benches/buffer_benches.rs` (7 benchmarks):**

| Benchmark                       | What It Measures                      |
| ------------------------------- | ------------------------------------- |
| `bench_parse_plain_text`        | Parser on plain ASCII                 |
| `bench_parse_sgr_heavy`         | Parser on SGR-dense input             |
| `bench_parse_cup_writes`        | Parser on CUP + data (TUI pattern)    |
| `bench_parse_bursty`            | Bursty PTY output pattern             |
| `bench_handle_incoming_data`    | Full `handle_incoming_data()` path    |
| `bench_data_and_format_for_gui` | Flatten for GUI                       |
| `bench_build_snapshot`          | Snapshot building (dirty/clean paths) |

**`freminal/benches/render_loop_bench.rs` (9 benchmarks):**

| Benchmark                                | What It Measures               |
| ---------------------------------------- | ------------------------------ |
| `feed_data_incremental` (100/1000 lines) | Incremental data feed          |
| `feed_data_ansi_heavy` (24/240 lines)    | ANSI-heavy data feed           |
| `feed_data_bursty`                       | Bursty output pattern          |
| `build_snapshot_after_ansi_feed`         | Snapshot after ANSI processing |
| `store_and_load`                         | ArcSwap round-trip             |
| `load_only`                              | ArcSwap load cost              |

### Gaps Identified

| Gap                                    | Impact | Notes                                                        |
| -------------------------------------- | ------ | ------------------------------------------------------------ |
| No scrollback rendering benchmark      | Medium | GUI applies scroll offset to select display window from rows |
| No shaping cache-miss benchmark        | Medium | First render of new glyphs is expensive                      |
| No alternate screen switch benchmark   | Low    | `enter_alternate`/`leave_alternate` performance unknown      |
| No image rendering benchmark           | Low    | Image protocol (iTerm2/Kitty/Sixel) GPU path not measured    |
| No full-screen erase variant benchmark | Low    | ED Ps=2 on full buffer vs ED Ps=0                            |

### Fragile Benchmarks

| Benchmark                              | Problem                                                       |
| -------------------------------------- | ------------------------------------------------------------- |
| `bench_resize`                         | Includes insert time in measurement — resize cost conflated   |
| `bench_shaping_ligatures` (if present) | Rebuilds `FontManager` per sample — measures init not shaping |

### CI Integration

- `.github/workflows/ci.yml` does NOT run any benchmark commands.
- `xtask ci` does NOT include `cargo bench --no-run`.
- A benchmark compilation failure would go undetected until the next manual bench run.

---

## Subtasks

---

### 24.1 — Add Missing Benchmarks

- **Status:** Complete
- **Priority:** 1 — High
- **Scope:** `freminal-buffer/benches/buffer_row_bench.rs`,
  `freminal-terminal-emulator/benches/buffer_benches.rs`,
  `freminal/benches/render_loop_bench.rs`
- **Details:**
  Add the following benchmarks:

  **`freminal-buffer`:**
  - `bench_scrollback_render`: Pre-populate buffer with 5000 rows of scrollback. Measure
    time to extract visible window at various scroll offsets (0, 1000, 4000).
  - `bench_alternate_screen_switch`: Fill primary buffer, enter alternate, fill alternate,
    leave alternate. Measure the switch time.
  - `bench_erase_display_full`: ED Ps=2 on a fully populated 200x50 buffer. Compare against
    existing `bench_erase_display` (Ps=0).

  **`freminal-terminal-emulator`:**
  - `bench_build_snapshot_with_scrollback`: Build snapshot on a terminal with 10000 rows of
    scrollback. Measure cost of `Arc<Vec<Row>>` wrapping.

  **`freminal`:**
  - `bench_shaping_cache_hit`: Shape the same text 1000 times (cache should be hot). Measure
    steady-state shaping cost. (Only if shaping infrastructure allows benchmark construction
    without a GPU context — otherwise defer.)

- **Acceptance criteria:**
  - All new benchmarks compile and produce stable results.
  - Each benchmark has `Throughput::Elements()` or `Throughput::Bytes()` where meaningful.
  - `cargo bench --all` runs without errors.
- **Tests required:** Benchmarks are self-verifying (Criterion reports).

---

### 24.2 — Fix Fragile Benchmarks

- **Status:** Complete
- **Priority:** 2 — Medium
- **Scope:** `freminal-buffer/benches/buffer_row_bench.rs`
- **Details:**
  Fix `bench_resize` to separate setup from measurement:

  **Current (fragile):**

  ```rust
  // Setup AND resize happen inside the timed section
  b.iter(|| {
      let mut buffer = Buffer::new(80, 24);
      // ... insert data ...
      buffer.set_size(40, 24, 0);
  });
  ```

  **Fixed:**

  ```rust
  b.iter_batched(
      || {
          let mut buffer = Buffer::new(80, 24);
          // ... insert data ...
          buffer
      },
      |mut buffer| buffer.set_size(40, 24, 0),
      BatchSize::SmallInput,
  );
  ```

  This isolates the resize cost from the data population cost.

- **Acceptance criteria:**
  - `bench_resize` measures only resize time, not setup + resize.
  - Benchmark numbers reflect actual resize performance.
  - No regression in other benchmarks.
- **Tests required:** Run `cargo bench` and verify the resize numbers are lower (setup excluded).

---

### 24.3 — Add `cargo bench --no-run` to CI

- **Status:** Complete
- **Priority:** 1 — High
- **Scope:** `xtask/src/main.rs`, `.github/workflows/ci.yml`
- **Details:**
  1. Add `cargo bench --no-run --all` to `xtask ci` after the test step. This compiles all
     benchmarks without running them, catching compilation failures early.

  2. In `.github/workflows/ci.yml`, ensure the CI job runs `cargo xtask ci` (which now
     includes bench compilation). No separate bench step needed — compilation check is
     sufficient for CI.

  Note: Do NOT run benchmarks in CI. Benchmark numbers on shared CI runners are meaningless
  due to noisy neighbors. The goal is compilation verification only.

- **Acceptance criteria:**
  - `cargo xtask ci` includes benchmark compilation.
  - A benchmark that fails to compile causes CI to fail.
  - No benchmark execution in CI (only `--no-run`).
- **Tests required:**
  - Verify `cargo xtask ci` succeeds locally.
  - Intentionally break a benchmark, verify CI catches it.

---

### 24.4 — Weekly Benchmark Regression Workflow

- **Status:** Pending
- **Priority:** 3 — Low
- **Scope:** `.github/workflows/bench.yml` (new)
- **Details:**
  Create a GitHub Actions workflow that runs benchmarks weekly on a consistent runner:

  ```yaml
  name: Benchmark Regression Check
  on:
    schedule:
      - cron: "0 6 * * 1" # Monday 6am UTC
    workflow_dispatch: {} # Manual trigger
  ```

  The workflow:
  1. Checks out the repo.
  2. Runs `cargo bench --all -- --output-format=bencher` (or Criterion JSON).
  3. Uploads benchmark results as an artifact.
  4. Compares against the previous week's results (stored as artifact).
  5. If any benchmark regresses by > 20%, creates a GitHub issue.

  This is a detection mechanism, not a gate. Regressions are flagged for human review, not
  auto-blocked. The 20% threshold accounts for CI runner noise.

- **Acceptance criteria:**
  - Weekly workflow runs benchmarks on schedule.
  - Results are stored as artifacts for comparison.
  - Regressions > 20% are flagged (issue or PR comment).
- **Tests required:**
  - Manually trigger the workflow and verify it completes.

---

### 24.5 — Update `agents.md` Benchmark Rules

- **Status:** Complete
- **Priority:** 2 — Medium
- **Scope:** `agents.md`
- **Details:**
  Add the following to `agents.md`:
  1. **Benchmark lookup table:** Add a table listing all benchmark files and what they cover,
     so agents know which benchmarks to run for a given change area.

     | Change Area        | Benchmark File                                         | Key Benchmarks                                            |
     | ------------------ | ------------------------------------------------------ | --------------------------------------------------------- |
     | Buffer operations  | `freminal-buffer/benches/buffer_row_bench.rs`          | `bench_insert_*`, `bench_resize`, `bench_visible_flatten` |
     | Parser / Emulator  | `freminal-terminal-emulator/benches/buffer_benches.rs` | `bench_parse_*`, `bench_handle_incoming_data`             |
     | Snapshot building  | `freminal-terminal-emulator/benches/buffer_benches.rs` | `bench_build_snapshot`                                    |
     | Rendering pipeline | `freminal/benches/render_loop_bench.rs`                | `feed_data_*`, `build_snapshot_*`                         |

  2. **Recording rule:** Agents modifying performance-sensitive code must record before/after
     benchmark numbers in their completion report. The format should be:

     ```text
     | Benchmark | Before | After | Change |
     | --- | --- | --- | --- |
     ```

  3. **Regression threshold:** Any regression > 15% on a relevant benchmark must be justified
     in the completion report or the change must be revised.

  4. **Verification suite update:** Add `cargo bench --no-run --all` to the verification
     commands table (after `cargo-machete`).

- **Acceptance criteria:**
  - `agents.md` has a benchmark lookup table.
  - Recording and regression rules are documented.
  - Verification suite includes bench compilation.
- **Tests required:** None (documentation only).

---

### 24.6 — Baseline Recording and Documentation

- **Status:** Complete
- **Priority:** 2 — Medium
- **Scope:** `Documents/PERFORMANCE_PLAN.md`
- **Details:**
  After all new benchmarks are added (24.1) and fragile ones fixed (24.2):
  1. Run `cargo bench --all -- --save-baseline current`.
  2. Record all benchmark numbers in `PERFORMANCE_PLAN.md` Section 8.2, updating the existing
     tables with the new benchmarks.
  3. Note the hardware/environment used for the baseline.

  This establishes a new baseline that includes all the benchmarks from this task.

- **Acceptance criteria:**
  - All benchmarks (existing + new) have recorded numbers.
  - Numbers are in `PERFORMANCE_PLAN.md` Section 8.2.
  - Baseline saved via Criterion.
- **Tests required:** None (measurement only).

---

### 24.7 — Flamegraph Profiling and Hot-Path Analysis

- **Status:** Complete
- **Priority:** 1 — High
- **Scope:** All crates (profiling), `Documents/PLAN_24_BENCHMARK_IMPROVEMENTS.md` (results)
- **Details:**
  Run `cargo flamegraph` against realistic workloads to identify CPU hot paths that benchmarks
  alone cannot surface (e.g., hidden allocation storms, lock contention remnants, redundant
  computation). The procedure:
  1. **Build a release profile with debug symbols:**

     ```bash
     cargo build --release
     ```

     The workspace `Cargo.toml` already sets `[profile.release] debug = true` (or if not,
     temporarily add `debug = 1` to get symbol names without full debuginfo overhead).

  2. **Generate flamegraphs for key workloads:**

     Run the application under `cargo flamegraph` with representative terminal workloads.
     At minimum, capture flamegraphs for:
     - **Large `cat`:** `cargo flamegraph -- -e 'cat /dev/urandom | head -c 10000000'` or
       equivalent (exercises PTY read → parse → buffer insert → snapshot → render).
     - **TUI application:** Run `htop` or `vim` inside the terminal for 10 seconds
       (exercises cursor movement, partial screen updates, SGR changes).
     - **Idle terminal:** Terminal sitting at a shell prompt for 5 seconds (exercises the
       clean-snapshot fast path and repaint throttling).

     If the binary cannot be launched under `cargo flamegraph` directly due to PTY/tty
     requirements, use `perf record -g` + `perf script | inferno-flamegraph` as a fallback.

  3. **Analyze the flamegraphs:**

     For each flamegraph, identify:
     - Functions consuming > 5% of total CPU time.
     - Unexpected allocation sites (look for `alloc::`, `Vec::push`, `String::from` in hot
       paths).
     - Any function appearing in the flamegraph that should not be on the hot path (e.g.,
       `reflow_to_width` firing during normal output, `fonts_mut` being called per-character).
     - Differences between the large-cat (throughput-bound) and TUI (latency-bound) profiles.

  4. **Document findings and propose fixes:**

     Record the top 5–10 hot functions from each workload in a results table:

     ```text
     | Workload | Function | % CPU | Category | Proposed Fix |
     | --- | --- | --- | --- | --- |
     ```

     Categories: `expected` (inherently hot — parser, buffer insert), `fixable` (hot due to
     inefficiency — unnecessary clone, redundant computation), `deferred` (requires architectural
     change — e.g., shader-based rendering).

     For each `fixable` item, write a brief proposal (1–3 sentences) describing the fix.
     These proposals become input for future subtasks or new plan documents.

  5. **Save flamegraph SVGs:**

     Save the generated `.svg` files as artifacts (do NOT commit them to the repo). Note the
     commit hash and hardware used for reproducibility.

- **Acceptance criteria:**
  - At least two flamegraph workloads captured (large-cat + TUI or idle).
  - Top hot functions documented in the results table below.
  - Each `fixable` item has a concrete fix proposal.
  - No code changes in this subtask — analysis and proposals only.
- **Tests required:** None (profiling only).
- **Results:**

  **Profiling environment:**
  - Commit: `f50933ef8e5c0410073c5008e46a6f5e2305e6f1` (main, pre-task-24)
  - Hardware: AMD Ryzen 9 9950X 16-Core, 32 threads, Linux 6.19.9 NixOS
  - Tool: `cargo flamegraph 0.6.11` via `perf record -g --call-graph dwarf`
  - Workloads: All three benchmark binaries (`buffer_row_bench`, `buffer_benches`,
    `render_loop_bench`) with `--profile-time 5` (5s per benchmark)
  - Flamegraph SVGs saved to `/tmp/flamegraph_{buffer,emulator,render}.svg` (not committed)

  **Workload 1 — Buffer Benchmarks (213K samples, 265 billion cycles):**

  | Function                                      | % CPU       | Category | Proposed Fix                                                                                                                                                                                                                     |
  | --------------------------------------------- | ----------- | -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
  | `Cell::clone`                                 | 31.7%       | fixable  | `TChar::Utf8(Vec<u8>)` clones a heap `Vec` per character. Consider `TChar::Utf8(SmallVec<[u8; 4]>)` or `TChar::Utf8([u8; 4], u8)` inline representation — UTF-8 chars are at most 4 bytes. Eliminates ~90% of clone allocations. |
  | `Row::insert_text_with_limit`                 | 20.2%       | expected | Core insert path — inherently hot. Minor improvement: `tag.clone()` is called once per cell; should be called once per format run.                                                                                               |
  | `memmove` (libc)                              | 13.0%       | expected | Driven by `Vec::resize` and `Vec::clone`. Reducing `Cell::clone` heap allocs will reduce this proportionally.                                                                                                                    |
  | `drop_in_place<(Vec<TChar>, Vec<FormatTag>)>` | 9.0%        | fixable  | Benchmark teardown cost. In production, the snapshot row cache (`rows_as_tchars_and_tags_cached`) avoids this by reusing cached vectors. Consider `Arc`-wrapping cached results to eliminate drop/clone in the snapshot path.    |
  | `Cloned<I>::fold` (iter adapter)              | 7.2% + 1.3% | fixable  | Flatten path clones cells via `.cloned()` iterator. If `TChar` becomes `Copy` (inline repr), this becomes a memcpy instead of per-element clone.                                                                                 |
  | `scrollback_as_tchars_and_tags`               | 3.2%        | expected | Proportional to scrollback size — correct.                                                                                                                                                                                       |
  | `reflow_to_width`                             | 2.5%        | expected | Only fires on resize — correct.                                                                                                                                                                                                  |
  | `scroll_slice_up`                             | 1.7%        | expected | Core scroll path — correct.                                                                                                                                                                                                      |

  **Workload 2 — Emulator Benchmarks (160K samples, 215 billion cycles):**

  | Function                                      | % CPU | Category | Proposed Fix                                                                                                                                                                                                                                |
  | --------------------------------------------- | ----- | -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
  | `Row::insert_text_with_limit`                 | 12.5% | expected | Core path. Same `tag.clone()` per-cell issue as above.                                                                                                                                                                                      |
  | `Cell::clone`                                 | 11.5% | fixable  | Same issue — inline `TChar` representation would eliminate.                                                                                                                                                                                 |
  | `TerminalHandler::has_visible_images`         | 11.2% | fixable  | **O(W×H) scan every snapshot** even when no images exist. Add a `has_images: bool` dirty flag to `Buffer` that is set when an image cell is inserted and cleared on erase. Reduces to O(1) in the common case.                              |
  | `memmove` (libc)                              | 9.1%  | expected | Same as buffer — driven by clone/resize.                                                                                                                                                                                                    |
  | `Cloned<I>::fold`                             | 7.5%  | fixable  | Same flatten clone issue.                                                                                                                                                                                                                   |
  | `FreminalAnsiParser::ansi_parser_inner_empty` | 6.2%  | expected | Parser fast path for non-escape bytes.                                                                                                                                                                                                      |
  | `Graphemes::next` (unicode_segmentation)      | 5.4%  | fixable  | `from_vec` calls `.graphemes(true)` on every input chunk. For pure ASCII (the common case), grapheme segmentation is unnecessary — each byte is one grapheme. Add a fast-path: if all bytes are `< 0x80`, skip grapheme iteration entirely. |
  | `_int_malloc` (libc)                          | 3.7%  | fixable  | Heap allocation pressure from `Cell::clone`, `FormatTag::clone`, `Vec` growth. Reducing clone allocations via inline `TChar` will reduce this.                                                                                              |
  | `FreminalAnsiParser::push`                    | 3.5%  | expected | Parser dispatch — inherently hot.                                                                                                                                                                                                           |
  | `AnsiCsiParser::ansiparser_inner_csi`         | 3.2%  | expected | CSI sequence parsing — inherently hot.                                                                                                                                                                                                      |
  | `rows_as_tchars_and_tags_cached`              | 2.4%  | fixable  | Merge pass clones `FormatTag` (with `Vec<FontDecorations>` + `Option<Url>`) for every tag of every row on every call, even when rows are clean. Should rebased tags by reference or cache the merged result.                                |

  **Workload 3 — Render Benchmarks (297K samples, 365 billion cycles):**

  | Function                                           | % CPU | Category | Proposed Fix                                                                                                                                                                                                                                                          |
  | -------------------------------------------------- | ----- | -------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
  | `Cell::clone`                                      | 15.9% | fixable  | Same inline `TChar` fix.                                                                                                                                                                                                                                              |
  | `build_background_instances`                       | 14.1% | expected | GPU instance buffer construction — inherently hot. Minor opt: pre-size `Vec` with `with_capacity`.                                                                                                                                                                    |
  | `skrifa::outline::glyf::hint::engine::Engine::run` | 6.3%  | expected | TrueType hinting during glyph rasterization. Only fires on cache-miss (new glyphs).                                                                                                                                                                                   |
  | `arc_swap::debt::Debt::pay_all`                    | 5.7%  | expected | ArcSwap debt repayment — O(1) amortized. Normal cost of the lock-free snapshot model.                                                                                                                                                                                 |
  | `memmove` (libc)                                   | 5.1%  | expected | Same pattern.                                                                                                                                                                                                                                                         |
  | `arc_swap::debt::list::LocalNode::with`            | 4.6%  | expected | ArcSwap thread-local debt management. Combined with `pay_all`, total ArcSwap overhead is ~10.3% — acceptable for lock-free reads.                                                                                                                                     |
  | `Row::insert_text_with_limit`                      | 3.9%  | expected | Data feed benchmarks exercise the full pipeline.                                                                                                                                                                                                                      |
  | `AnsiCsiParser::ansiparser_inner_csi`              | 2.9%  | expected | Parser — same as emulator profile.                                                                                                                                                                                                                                    |
  | `memset` (libc) / `GlyphAtlas::new`                | 2.7%  | expected | Atlas initialization — one-time cost per benchmark sample.                                                                                                                                                                                                            |
  | `Graphemes::next`                                  | 2.1%  | fixable  | Same ASCII fast-path proposal.                                                                                                                                                                                                                                        |
  | `ShapingCache::shape_visible`                      | 2.0%  | fixable  | Two issues: (1) cache key uses `format!("{:?}", ...)` per tag field per line — allocates a `String` per field even on cache hits. Replace with a proper `Hash` impl on the key type. (2) `ShapedLine::clone()` on every cache hit — return `Arc<ShapedLine>` instead. |
  | `Hasher<S>::write` (SipHash)                       | 1.1%  | expected | Hashing cost in `HashMap` lookups (shaping cache). If `format!`-based key construction is eliminated, the hash itself is fine.                                                                                                                                        |

  **Summary — Top 7 Fixable Issues (ranked by estimated impact):**

  | #   | Issue                                                             | Affected Profiles                  | Est. Impact | Fix Complexity                                     |
  | --- | ----------------------------------------------------------------- | ---------------------------------- | ----------- | -------------------------------------------------- |
  | F1  | `Cell::clone` heap allocation via `TChar::Utf8(Vec<u8>)`          | All three (15–32%)                 | Very High   | Medium — change `TChar` to inline `[u8; 4]` repr   |
  | F2  | `has_visible_images` O(W×H) scan per snapshot                     | Emulator (11.2%)                   | High        | Low — add `image_count: usize` to `Buffer`         |
  | F3  | `FormatTag::clone` in `rows_as_tchars_and_tags_cached` merge pass | Emulator (2.4%), Buffer (indirect) | Medium      | Medium — cache merged tags or rebased by reference |
  | F4  | `Graphemes::next` on ASCII-only input                             | Emulator (5.4%), Render (2.1%)     | Medium      | Low — ASCII fast-path bypass                       |
  | F5  | `shape_visible` `format!` key allocation + `ShapedLine::clone`    | Render (2.0%)                      | Medium      | Medium — proper `Hash` impl + `Arc` return         |
  | F6  | `tag.clone()` per cell in `insert_text_with_limit`                | All three (indirect)               | Medium      | Low — clone once per format run                    |
  | F7  | `Cloned<I>::fold` in flatten path                                 | Buffer (8.5%), Emulator (7.5%)     | Medium      | Low — becomes free if F1 makes `TChar` `Copy`      |

---

## Implementation Notes

### Subtask Ordering

24.1 (new benchmarks) and 24.2 (fix fragile) are independent and can run in parallel.
24.3 (CI integration) is independent and can run at any time.
24.4 (weekly workflow) depends on 24.3.
24.5 (agents.md update) is independent.
24.6 (baseline) must run last, after 24.1 and 24.2 are complete.
24.7 (flamegraph) is independent and can run at any time; recommended early to inform priorities.

**Recommended order:** 24.7 → 24.1 + 24.2 (parallel) → 24.3 → 24.5 → 24.6 → 24.4

### Verification

Each subtask must pass before proceeding:

- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo-machete`
- `cargo bench --no-run --all` (after 24.3)

---

## References

- `Documents/PERFORMANCE_PLAN.md` Section 8 — existing benchmark plan and numbers
- `freminal-buffer/benches/buffer_row_bench.rs` — buffer benchmarks
- `freminal-terminal-emulator/benches/buffer_benches.rs` — emulator benchmarks
- `freminal/benches/render_loop_bench.rs` — render benchmarks
- `xtask/src/main.rs` — CI orchestration
- `.github/workflows/ci.yml` — CI pipeline
- `agents.md` — agent rules (verification suite section)
