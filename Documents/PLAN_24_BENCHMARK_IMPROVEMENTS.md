# PLAN_24 — Benchmark Improvements and CI Integration

## Status: Pending

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
**Estimated scope:** Medium (6 subtasks)

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

- **Status:** Pending
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

- **Status:** Pending
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

- **Status:** Pending
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

- **Status:** Pending
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

- **Status:** Pending
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

## Implementation Notes

### Subtask Ordering

24.1 (new benchmarks) and 24.2 (fix fragile) are independent and can run in parallel.
24.3 (CI integration) is independent and can run at any time.
24.4 (weekly workflow) depends on 24.3.
24.5 (agents.md update) is independent.
24.6 (baseline) must run last, after 24.1 and 24.2 are complete.

**Recommended order:** 24.1 + 24.2 (parallel) → 24.3 → 24.5 → 24.6 → 24.4

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
