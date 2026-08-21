# PLAN_VERSION_120.md — v0.12.0 "Scrollback Memory & Performance"

## Goal

**v0.12.0 is a bug-fix, performance and structural-cleanup release.** It ships no new
protocol support and no new user-facing features. Three themes:

**Theme 1 — the entire scrollback-memory effort**, deliberately pulled forward and
completed in one place rather than spread across later point releases (a conscious bending
of the one-theme-per-version convention — the memory work is cohesive and the context is
hot, so it ships together):

- **Task 118 — Compact Cell Representation** (complete): a buffer-layer memory optimisation
  that shrinks stored scrollback rows ~8–12× by sharing formatting across runs and dropping
  the always-null image pointer, plus idle-driven compaction off the hot path.
- **Task 119 — Scrollback Compression (LZ4)** (complete): an incremental memory multiplier
  layered on the Task-118 compact form — block-granular LZ4 compression of idle scrollback,
  decompress-on-scroll with an LRU block cache, driven by the same idle tick Task 118
  established. LZ4-only (no zstd tier).
- **Task 120 — Compression-Aware Windowed Reflow** (enriched stub): once a very large
  scrollback is affordable, synchronous full-scrollback reflow becomes the new latency wall.
  This absorbs the former 118.10 lazy-reflow stub and the reflow half of the old Task 119,
  because band-decompression and lazy reflow are one control flow. Decomposed at its own
  activation session, not now.

**Theme 2 — CPU performance remediation:**

- **Task 121 — Performance Remediation** (closed 2026-08-20): the umbrella for all work
  arising from issue #459's real-workload CPU profiling. Thirteen subtasks merged (121.13 was
  reverted), plus 121.23, 121.26, 121.32 and the Group D close-out. **Closed as an umbrella
  rather than finished** — it had grown to 36 subtasks across seven groups, much of Groups F
  and G predicated on a chrome cache now disabled by default, and had stopped working as a
  tracker. `PLAN_121_PERF_REMEDIATION.md` is now a historical record carrying the migration
  map. Summarised below.
- **Task 123 — GL Pipeline Measurement Harness** (planned): the instrument Task 121 never
  had. Phase 1 is a 47-method facade over `&glow::Context` with a recording backend —
  deterministic, no GPU, no display server, runs in the existing CI matrix on all four
  platforms. Phase 2 is the pixel/readback harness issue #440 has wanted since #436, which is
  Linux-only and needs Mesa/llvmpipe/Xvfb added to `flake.nix` plus a new Nix CI job. It
  changes no rendering behaviour. Broken down in
  `Documents/PLAN_123_GL_MEASUREMENT_HARNESS.md`.
- **Task 124 — Render Efficiency Remediation** (stub, gated on 123): the fixes. Carries the
  surviving work from Task 121 — cell-granular pointer suppression, the full-present-on-motion
  anomaly, the chrome-cache keep/delete decision, the shaping levers, GPU buffer orphaning —
  plus its own leading hypothesis, that dirty-row `Arc` churn forces a full rebuild on
  byte-identical content. **No subtask is implemented before 123 quantifies it**, except
  124.4 (bool-to-struct), which has no expected performance effect. Broken down in
  `Documents/PLAN_124_RENDER_EFFICIENCY.md`.

**Theme 3 — structural cleanup:**

- **Task 122 — Orchestration Extraction** (complete, merged 2026-08-03 via PR #472): decompose
  the GUI binary's god functions and give orchestration logic (event triage, view window, input
  encoding, frame decisions) a home. A no-behaviour-change refactor. Required whichever way
  the egui decision falls. Broken down in
  `Documents/PLAN_122_ORCHESTRATION_EXTRACTION.md`; summarised in the Task 122 section below.

The memory tasks and Task 121 touch different layers (`freminal-buffer` versus the GUI and
windowing frame path) and are independent and parallelizable. Task 122 overlaps Task 121
in one place only: subtask 121.17 depends on it, so 122 precedes 121.17.

**Tasks 102 (Kitty File Transfer) and 103 (Multiple Cursors) moved out of this version**
to `PLAN_VERSION_130.md` when v0.12.0 was redefined as bug-fix-and-performance-only. Their
content moved unchanged.

Depends on the existing lock-free architecture.

**Decomposed** per the `freminal-version-activation` skill, except Task 120, which stays an
enriched stub per the just-in-time planning policy. Re-confirm the seams at activation
before executing.

---

## Task Summary

| #   | Feature                           | Scope     | Status      | Depends On     |
| --- | --------------------------------- | --------- | ----------- | -------------- |
| 118 | Compact Cell Representation       | Medium    | Complete    | None           |
| 119 | Scrollback Compression (LZ4)      | Large     | Complete    | Task 118       |
| 120 | Compression-Aware Windowed Reflow | Large     | Stub        | Tasks 118, 119 |
| 121 | Performance Remediation           | Large     | Complete    | None           |
| 122 | Orchestration Extraction          | Large     | Complete    | None           |
| 123 | GL Pipeline Measurement Harness   | Large     | Planned     | Task 122       |
| 124 | Render Efficiency Remediation     | Large     | Stub        | Task 123       |

---

## Design Decisions (provisional, confirm at activation)

- **v0.12.0 ships no new features.** It was redefined as a bug-fix, performance and
  structural-cleanup release after the issue #459 profiling work surfaced more than it
  fixed. Tasks 102 and 103 moved to v0.13.0 rather than being descoped; nothing about
  their design changed. "Structural cleanup" is in the charter specifically to admit
  Task 122, which is neither a fix nor an optimisation.
- **The entire scrollback-memory effort lands in this version.** Tasks 118 (compact), 119
  (LZ4 compression), and 120 (compression-aware windowed reflow) were originally spread
  across v0.12.0 and a later v0.13.1 (`PLAN_VERSION_131.md`, now deleted). They are pulled
  together here deliberately — the work is cohesive, the infrastructure Task 118 built (compact
  form, idle driver, decompact-on-read seam, RSS reclaim) is exactly what 119 and 120 reuse,
  and doing it in one place while that context is fresh is worth bending the
  one-theme-per-version convention for.
- **Task 120 is an enriched stub; 119 is fully decomposed.** Per `freminal-version-activation`,
  the large, subtle reflow task is decomposed at its own activation, not now; the compression
  core (119) is decomposed because its prerequisites (Task 118) are already merged.
- **Task 121 was an umbrella, not a single deliverable, and was closed as one.** Its
  subtasks were scheduled individually; the survivors migrated to Tasks 123 and 124 on
  2026-08-20 rather than outliving the version inside a tracker that no longer worked.
  Tasks 123 and 124 replace it, and the version does not gate on Task 124
  reaching Complete; it gates on the completed subtasks being merged and the outstanding
  ones being tracked.
- **Task 121 is not the egui-decoupling decision.** `Documents/DECOUPLING_FRAMEWORK.md` is
  the decision record for "should freminal stop using egui for the main window", and its
  Phases 1–5 are the rewrite-if-chosen plan. That question is reopened and leaning against
  the rewrite. Task 121 is the performance work, and it stands regardless of how that
  decision falls.

---

## Task 118 — Compact Cell Representation

### 118 Summary

Reduce the resident-memory cost of scrollback so a much larger default scrollback becomes
affordable, by shrinking the in-memory footprint of stored rows. This is **phase one** of the
three-phase scrollback-memory effort that all lands in this version. Phase one is pure
representation/serialization — **no compression codec, no decompression-on-scroll, no reflow
complexity, no new dependency** — and captures the large majority of the achievable win.
Phase two (Task 119) adds LZ4 idle compression as an incremental multiplier on top; phase
three (Task 120) makes reflow of the resulting very-large scrollback affordable.

The measured motivation (feasibility spike, 100k-line corpora with realistic
"stable-structure + unique-content" data):

| Corpus                    | In-memory today | Flat compact repr | Reduction |
| ------------------------- | --------------- | ----------------- | --------- |
| Shell session (typical)   | ~4160 B/line    | ~345 B/line       | **~12×**  |
| Source / logs             | ~3739 B/line    | ~310 B/line       | **~12×**  |
| High-entropy colored (WC) | ~5800 B/line    | ~732 B/line       | **~8×**   |

The in-memory `Cell` is **72 bytes** (18-byte `TChar` + 40-byte `FormatTag` +
2 bools + 8-byte `Option<Box<ImagePlacement>>`, padded to 72). The 40-byte `FormatTag`
is duplicated in full on **every** cell even though runs of adjacent cells almost always
share identical formatting, and the 8-byte image pointer is `None` for essentially every
text cell. A compact representation that (a) shares formatting across runs instead of
per-cell and (b) drops the always-null image slot from the common-case storage recovers
~8–12× with zero runtime decompression cost.

### 118 Design decisions (durable)

- **Phase one is representation only; no codec.** The ~8–12× win comes entirely from
  removing per-cell `FormatTag` duplication and the always-`None` image pointer from stored
  scrollback rows. This is guaranteed regardless of content and adds **zero** read-path
  latency (it is a compact layout, not a compressed blob). Compression (Task 119) is layered
  on later and is explicitly out of scope here.
- **Format-run sharing, not per-cell tags.** Store a row's formatting as a small run list
  (`(FormatTag, run_length)` or a per-row interned tag table + per-cell index), reflecting
  that adjacent cells overwhelmingly share a `FormatTag`. This mirrors the existing
  `FormatTag { start, end, … }` range model already used at the flatten boundary
  (`freminal-common/src/buffer_states/format_tag.rs`) and in `RowCacheEntry.tags`
  (`freminal-buffer/src/buffer/flatten.rs`), so the run model is not a new concept in the
  codebase.
- **Scope the compaction to scrollback rows, not the active region.** The visible/active
  region (`Buffer.rows` tail of length `height`) is mutated constantly and must stay in the
  fast random-access `Vec<Cell>` form. Only rows that have scrolled into history
  (`rows[0 .. rows.len()-height]`) are candidates for the compact form. The boundary is
  crossed when a row scrolls out of the visible window.
- **The `row_cache` duplicate is part of the prize.** `Buffer.row_cache:
  Vec<Option<RowCacheEntry>>` (`buffer/mod.rs:84`) holds a *second*, fully-flattened copy of
  every row (`chars`, `tags`, `bytes`, `byte_to_char`, `auto_urls`). For scrollback rows this
  is pure duplication of data that is rarely read. Evicting / not-populating the cache entry
  for compacted scrollback rows is a first-class part of this task's memory win, separate
  from the cell compaction itself.
- **Correctness is preserved exactly.** The compact form must round-trip losslessly to the
  current `Row`/`Cell` (same `TChar`, same `FormatTag`, same wide-head/continuation flags,
  same image placement when present). Inline-image scrollback rows (rare) may opt out of
  compaction and stay in the `Vec<Cell>` form rather than complicate the compact encoding.
- **No public snapshot/API change if avoidable.** `build_snapshot()` and the flatten path
  consume rows via the existing accessors; the compact form should be internal to
  `freminal-buffer` and decompacted on read at the flatten boundary, so the terminal-emulator
  and GUI layers are unaffected. This respects the crate dependency boundaries in
  `freminal-architecture`.
- **Raising the default scrollback is a deliberate outcome, decided with data.** Once the
  per-line cost drops ~8–12×, the default `ScrollbackConfig.limit` (currently 4000, range
  1..=100_000, `freminal-common/src/config.rs`) can be raised substantially at net-lower
  memory. The exact new default is chosen in 118.5 against the measured post-compaction
  per-line cost, not guessed here.
- **Compaction lives inside `Row`, not `Buffer` (chosen at 118.3 activation, with recon).**
  Two designs were weighed against measured blast radius: (A) change `Buffer.rows` to
  `Vec<StoredRow>` where `StoredRow = enum { Live(Row), Compact(CompactRow) }`, and (B) keep
  `Buffer.rows: Vec<Row>` and give `Row` an internal storage enum
  (`{ Live(Vec<Cell>), Compact(CompactRow) }`) so compaction is transparent behind `Row`'s
  existing accessors. Recon showed Design A touches ~228 `self.rows[...]` sites plus ~28 bare
  `pub`-field accesses (`origin`/`join`/`dirty`/`line_width`) across all 10 `buffer/` files —
  every one a match-arm or new accessor, a large correctness-risk surface. Design B confines
  the storage change to `row.rs`; `Buffer`'s call sites are untouched. **Design B chosen.** The
  performance rationale: the change adds **zero** cost to the hot paths (frame render / flatten,
  PTY-ingest `insert_text`, scroll) under either design, so the deciding factor is
  correctness-risk, and B's is far smaller.
- **Compact rows are never mutated in place — decompact-all-on-resize (118.3 recon finding).**
  The initial assumption that scrollback rows are read-only once they leave the visible window
  is **false**. Recon confirmed three in-place scrollback-touch paths: (1) the `set_size`
  width-change pass (`resize_and_alt.rs:132-146`) force-dirties and re-widths every row incl.
  scrollback after reflow; (2) the `resize_height` grow pass (`resize_and_alt.rs:730-733`)
  dirties `0..old_height` (scrollback indices — see cleanup note below); (3) the whole-buffer
  image-placement-clear family (`images.rs`), reachable from everyday `insert_text`/erase when
  a partially-visible image is overwritten, and from kitty `a=d`. Path (3) is neutralised for
  free: **image rows opt out of compaction**, so an image-clear only ever touches `Live` rows.
  Paths (1)/(2) are handled by **decompacting every compact scrollback row back to `Live` at
  the start of any resize**, letting the existing (delicate) reflow/dirty passes run unchanged,
  then re-compacting out-of-window rows afterward. This is chosen for performance where it
  matters: resize is a rare, human-timescale event already doing O(all rows) work (reflow does
  `mem::take` + full rebuild), so the extra decompact pass is negligible against reflow's own
  cost, adds **zero** cost to every hot path, and keeps the fragile resize logic untouched.
  Making the resize passes compact-aware was rejected: it saves nothing measurable (the saving
  is invisible against reflow, and resize is not hot) at real correctness risk to the most
  delicate code in the buffer.
- **Compaction is a deferred, idle-driven, budgeted background task — NEVER synchronous on a hot
  path (revised after CPU benchmarking).** The original 118.3 design compacted scrollback rows
  synchronously inside `enforce_scrollback_limit` (reached from `insert_text`/`handle_lf`/resize).
  Benchmarking showed this put `CompactRow::from_row` cost directly on hot paths: the worst case
  was `softwrap_heavy` (+45% → +23% after the O(n²) reflow-offset fix), where reflowing one
  5000-char line to width 10 creates ~420 scrollback rows that were all compacted *inside the
  timed resize*. Resize is a hot loop and always will be; a giant line can hit the buffer and
  immediately scroll into history. The durable principle: **having the most recent snapshot
  available to the user, at the expense of slightly delaying the memory saving, is more valuable
  than getting the memory win immediately.** Therefore compaction is moved entirely off the hot
  paths:
  - `enforce_scrollback_limit` (and every other hot path) no longer compacts. Rows that scroll
    into history simply stay `Live` until idle compaction runs.
  - The PTY consumer thread's `select!` loop (`freminal/src/gui/pty.rs`) gains a genuine idle
    tick: a `recv(crossbeam_channel::after(~250ms))` arm that fires only when neither PTY data
    nor GUI input has arrived (the loop previously blocked indefinitely with no timeout arm).
    On that idle tick the thread compacts scrollback. This respects the lock-free architecture:
    the PTY thread still owns `TerminalEmulator` exclusively; no separate timer thread touches
    the buffer.
  - Each idle tick compacts at most a **bounded budget** of rows (e.g. 512) so even a large
    backlog never causes a single long stall; the remainder compacts on subsequent ticks. When
    no compaction work remains, the tick need not re-arm (avoid waking a fully-idle terminal
    forever — battery).
  - Entry point: a new `pub` `Buffer::compact_idle_scrollback(budget) -> usize` (rows compacted),
    passed through `TerminalHandler`/`TerminalEmulator`, callable from the PTY loop via
    `emulator.internal.handler.buffer_mut()`.
  This makes the memory win **eventually-consistent** rather than immediate, which is the correct
  tradeoff for a memory optimisation: the user never pays compaction latency during typing,
  scrolling, or resizing; they get the full snapshot immediately and the memory is reclaimed a
  few hundred ms later once the terminal goes quiet.
- **`row_cache` decompaction seam is via `Row`'s accessors, memoized (118.3).** The 3
  borrow-returning accessors (`cells()`, `characters()`, `cells_mut()`) are the seam: a
  `Compact` `Row` materialises back to `Live` on first cell access and stays `Live` for the
  duration of the read burst, so repeated accesses within one flatten/extract are zero-cost.
  Since scrollback is read-mostly, this one-time cost per read burst is acceptable per 118.5's
  "cold decompact-on-read may slow down; visible-region path must not regress" rule.

### 118 Cleanup entries (surfaced during recon)

- **118.8 — `compact_newly_scrolled_rows` early-stop can miss a decompacted mid-scrollback
  row. RESOLVED / OBSOLETE by 118.9.** This concerned the incremental backward-scan-with-early-
  stop in `compact_newly_scrolled_rows` (the resize-regression fix): an image-clear could
  decompact a mid-scrollback row in place, and the backward scan would break early and never
  re-compact it. 118.9 (deferred idle compaction) **deleted `compact_newly_scrolled_rows`
  entirely** — compaction no longer runs on any hot path, and `compact_idle_scrollback` uses a
  simple forward scan over `0..visible_window_start(0)` that skips already-compact rows and
  compacts any `Live` compactable row it finds, with no early-stop invariant to violate. A row
  an image-clear left `Live` is therefore re-compacted on the next idle tick automatically. No
  further action needed.
- **118.7 — `resize_height` grow-pass dirties scrollback rows, not the new visible window.
  RESOLVED (folded into the 118.5 pass).** The height-grow branch marked rows `0..old_height`
  dirty, but the visible window is bottom-anchored, so when scrollback existed those were the
  OLDEST scrollback rows at the top of the buffer — the wrong rows: it over-invalidated cold
  scrollback (wasting cache rebuilds, and needlessly touching compact rows) while leaving the
  genuinely-newly-visible rows stale. Fixed to invalidate the new bottom-anchored visible window
  (`rows.len()-new_height..rows.len()`).   Regression test added
  (`height_grow_invalidates_new_visible_window_not_top_scrollback`) asserting the new visible
  window is invalidated and top-of-scrollback cache entries are retained. Impact was benign
  (over-invalidation, not corruption), consistent with the original assessment.
- **118.11 — `resize_saved_primary` reflows the saved primary against the compiled-in default
  scrollback limit, not the user's configured limit. OPEN (pre-existing, out of scope for 118).**
  `Buffer::resize_saved_primary` (`buffer/resize_and_alt.rs:229`) reconstructs a throwaway
  primary `Buffer` to reuse the resize/reflow logic, but `SavedPrimaryState` does not carry the
  real configured `scrollback_limit`, so the temp buffer hardcodes `10_000` (previously `4000`;
  bumped with the 118.5 default raise). For any pane whose configured limit differs from the
  default, an alt-screen resize therefore enforces the wrong scrollback limit on the saved
  primary. This is a pre-existing gap (the value was already hardcoded before Task 118 — only the
  constant changed) and is disclosed in an in-code `NOTE`. The fix is to thread the true limit
  through `SavedPrimaryState` and use it here instead of the constant; deferred as a standalone
  cleanup so this PR does not alter alt-screen resize behavior.

### 118 Current-state map (from recon)

- **`Cell`** — `freminal-buffer/src/cell.rs:15` (`value: TChar`, `format: FormatTag`,
  `is_wide_head`, `is_wide_continuation`, `image: Option<Box<ImagePlacement>>`). Fields are
  private with accessors; construction via `Cell::new` / `blank_with_tag` /
  `wide_continuation`.
- **`Row`** — `freminal-buffer/src/row.rs:68` (`cells: Vec<Cell>`, `width`, `origin`, `join`,
  `dirty`, `line_width`). Rows are already **sparse** (trailing default-blank cells trimmed,
  e.g. `row.rs:570`).
- **`Buffer.rows: Vec<Row>`** — `buffer/mod.rs:78`; scrollback = indices
  `0..rows.len()-height`, visible = last `height`. **`Buffer.row_cache:
  Vec<Option<RowCacheEntry>>`** — `buffer/mod.rs:84`, index-parallel to `rows`.
- **`FormatTag`** — `freminal-common/src/buffer_states/format_tag.rs:22`; 40 bytes; the only
  heap field is `url: Option<Arc<Url>>` (cloning bumps a refcount, never deep-copies).
  `is_visually_default()` (`format_tag.rs:60`) is the cheap default check.
- **Flatten boundary** — `Buffer::flatten_row` / `rows_as_tchars_and_tags_cached`
  (`buffer/flatten.rs`) is where rows become `RowCacheEntry`; a compact scrollback row must
  decompact correctly through this path.
- **Benchmarks** — `freminal-buffer/benches/buffer_row_bench.rs`
  (`bench_scrollback_flatten`, `bench_scrollback_render`, `buffer_resize`, `softwrap_heavy`)
  and `freminal-terminal-emulator/benches/buffer_benches.rs`
  (`bench_build_snapshot_with_scrollback`) cover the hot paths this task touches.

### 118 Subtasks

#### 118.1 — READ-ONLY audit + compact-representation design

Scope: read-only across `freminal-buffer/src/cell.rs`, `row.rs`, `buffer/mod.rs`,
`buffer/flatten.rs`, `freminal-common/src/buffer_states/format_tag.rs`, and the buffer
benches.

What: produce the concrete design for the compact scrollback-row representation. Decide:
the exact compact type (e.g. `CompactRow { chars: Vec<TChar>, tag_runs: Vec<(FormatTag,
u32)>, flags: …, line_width, origin, join }` or an interned-tag-table variant); the
compaction trigger (row scrolls out of the visible window) and decompaction trigger (row
re-enters visible window / is read for flatten); how inline-image rows are handled (opt out
vs encode); how `row_cache` eviction for compacted rows is wired; and the exact accessor/
flatten seam where decompaction happens so no higher layer sees the compact form. Confirm
the sparse-row invariant interaction. Name every file each later subtask touches.

Deliverable: design report with the chosen type definitions and the file-scoping for
118.2–118.6. No code.

Verification: none (read-only).

Prohibitions: do NOT edit files; do NOT introduce a compression codec (that is Task 119);
do NOT begin implementation; do NOT proceed without maintainer review of the design.

Stop: report design; await explicit sign-off before 118.2.

#### 118.2 — Compact row type + lossless round-trip (pure, in `freminal-buffer`)

Scope: new module `freminal-buffer/src/compact_row.rs` (or as named in 118.1);
`freminal-buffer/src/lib.rs` (module decl); unit tests in the new module.

What: implement the compact row type chosen in 118.1 and the two conversions
`Row -> CompactRow` and `CompactRow -> Row`, exactly lossless for `TChar`, `FormatTag`
(including `Arc<Url>` sharing), wide-head/continuation flags, `line_width`, `origin`,
`join`, and inline-image placement (or the documented opt-out). Pure data transform; no
Buffer integration yet.

Deliverable: the type + conversions + exhaustive round-trip tests (plain rows, colored
runs, mixed tags, wide chars, URL tags, blank/sparse rows, and — per the 118.1 decision —
image rows or the opt-out path). A `size_of`/heap-size assertion demonstrating the
per-row reduction on a representative row.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT touch `Buffer`; do NOT change `Cell`/`Row` public API; do NOT add a
codec; do NOT proceed.

Stop: report + await review.

#### 118.3 — Buffer integration: compact scrollback rows on scroll-out

Scope: `freminal-buffer/src/buffer/mod.rs` (row storage + the scroll-out path), the
scroll/enforce-scrollback sites (`enforce_scrollback_limit`, the scroll path around
`buffer/mod.rs:333`), and `buffer/flatten.rs` (decompact-on-read seam).

What: store scrollback rows in the compact form, decompacting at the flatten/read boundary
so no higher layer observes the change. Compact a row when it scrolls out of the visible
window; keep the visible `height` rows in the existing `Vec<Cell>` form. Preserve every
existing `Buffer` behaviour (visible_rows, resize, alt-screen switch, prompt_rows/
command_blocks index shifting on drain).

Deliverable: integration + tests proving identical observable output (flatten, visible_rows,
snapshot content) before/after compaction across scroll, and that scrollback eviction still
shifts dependent indices correctly.

Verification: `cargo test --all`; clippy. Existing buffer tests must pass unchanged.

Prohibitions: do NOT alter visible-region storage; do NOT change snapshot/public API; do
NOT add a codec; do NOT proceed.

Stop: report + await review.

#### 118.4 — `row_cache` eviction for compacted scrollback rows

Scope: `freminal-buffer/src/buffer/mod.rs` (`row_cache` population/invalidation),
`buffer/flatten.rs` (cache lookup).

What: stop populating / actively evict `RowCacheEntry` for rows that are in the compact
scrollback form, so the second flattened copy is not held for cold history. Re-populate on
demand when such a row is read (decompacted) for flatten. Ensure the cache-index parallelism
with `rows` is maintained through drains and resizes.

Deliverable: cache-eviction logic + tests asserting compacted scrollback rows hold no cache
entry, that reading one repopulates correctly, and that URL auto-detection still works on
re-read.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT evict cache for visible rows; do NOT proceed.

Stop: report + await review.

#### 118.5 — Raise default scrollback + benchmark before/after

Scope: `freminal-common/src/config.rs` (`ScrollbackConfig::default`), `config_example.toml`,
the buffer benches (`freminal-buffer/benches/buffer_row_bench.rs`,
`freminal-terminal-emulator/benches/buffer_benches.rs`).

What: capture before/after memory + throughput per `performance-benchmarks` +
`freminal-bench-table` for `bench_scrollback_flatten`, `bench_scrollback_render`,
`bench_build_snapshot_with_scrollback`, `buffer_resize`, and `softwrap_heavy`. Confirm no
>15% regression on the read/flatten hot paths (some slowdown on the cold decompact-on-read
path is acceptable and expected; the visible-region path must not regress). Using the
measured post-compaction per-line cost, raise `ScrollbackConfig`'s default `limit` to a value
that is net-lower-or-equal memory versus today's 4000-line default (proposed target decided
here with data, not guessed), and update `config_example.toml` and the field doc/comment.

Deliverable: benchmark record (before/after) + the new default + config doc update.

Verification: `cargo test --all`; clippy; `cargo bench --no-run --all`; markdownlint clean
for any doc edits.

Prohibitions: do NOT raise the default without the benchmark justifying it; do NOT regress
the visible-region path >15%; do NOT proceed.

Stop: report + await review.

**DONE.** Default raised **4000 → 10000** (`ScrollbackConfig::default` and the buffer's
compiled-in fallback in `lifecycle.rs`, kept in sync; `config_example.toml` updated). Chosen
with data: measured settled per-line cost after compaction is ~1.0–1.7 KB/line for realistic
colored scrollback (worst realistic ~1.7 KB/line), so 10000 lines ≈ 17 MB resident ≈ the old
4000-line default's ~16.6 MB — 2.5× the history at net-neutral steady-state memory. Config
default-assertion tests updated across `freminal-common`, `freminal-buffer`,
`freminal-terminal-emulator`. Also folded in cleanup 118.7 (below).

#### 118.6 — Windows cross-check + final verification

Scope: no new logic; verification only (plus any trivial fix the cross-check surfaces).

What: run `cargo xtask check-windows` (per `freminal-windows-crosscheck`) since this touches
buffer storage/threading-adjacent code, and the full verification suite. Fix any
Windows-only issue surfaced.

Deliverable: green verification across the suite + Windows cross-check.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`;
`cargo machete`; `cargo fmt --all -- --check`; `cargo xtask check-windows`.

Prohibitions: do NOT add features here; do NOT proceed past a failing check.

Stop: report results.

#### 118.9 — Deferred idle-driven compaction (move compaction off hot paths)

Scope: `freminal-buffer/src/buffer/` (remove hot-path compaction, add
`compact_idle_scrollback(budget)`), `freminal-terminal-emulator` (passthrough on
`TerminalHandler`/`TerminalEmulator`), `freminal/src/gui/pty.rs` (PTY-loop idle tick).

What: implement the "compaction is a deferred, budgeted, idle-driven background task, never
synchronous on a hot path" decision recorded in the durable-decisions section. Two halves:
(a) **buffer layer** — remove every synchronous compaction call from `enforce_scrollback_limit`
and any other hot path; add `pub fn Buffer::compact_idle_scrollback(&mut self, budget: usize) ->
usize`; adapt tests to call it explicitly before asserting compaction. (b) **PTY-loop wiring** —
add a real idle-tick arm (`recv(crossbeam_channel::after(~250ms))`) to the `select!` in
`spawn_pty_consumer_thread`; on idle, call the budgeted compaction through
`emulator.internal.handler.buffer_mut().compact_idle_scrollback(BUDGET)`; re-arm the tick while
the return value is `> 0`, and let it lapse (no re-arm) when there is nothing left to compact so a
fully-idle terminal is not woken forever. Must respect the lock-free architecture: PTY thread owns
`TerminalEmulator` exclusively; no separate timer thread touches the buffer.

Deliverable: hot paths compaction-free (proven by a test that a scrollback fill leaves rows `Live`
until the idle call runs); idle tick wired; before/after CPU benches showing hot paths
(insert/lf/resize/softwrap) no longer pay compaction cost; before/after memory benches confirming
compaction still happens (just deferred).

Verification: `cargo test --all`; clippy; `cargo bench --bench buffer_row_bench -- --baseline
before_118_3` (softwrap_heavy back to ~0%); memory benches; `cargo xtask check-windows` (touches
the PTY thread / crossbeam select).

Prohibitions: do NOT compact on any hot path; do NOT spawn a separate thread that touches the
buffer; do NOT let the idle tick busy-wake a fully-idle terminal.

Stop: report + benches + await review.

#### 118.10 — Windowed / lazy reflow (DISSOLVED into Task 120)

Status: **dissolved.** This subtask was promoted to a first-class task (**Task 120 —
Compression-Aware Windowed Reflow**, later in this document) and merged with the reflow half
of the original Task 119, because band-decompression-on-reflow and lazy reflow are the same
control flow (the band you decompress is the band you reflow, and the async tail is shared).
Building them separately would mean constructing the lazy-reflow band machinery twice. The
durable design principle it captured is preserved in the Task 120 section; nothing is lost.

### 118 Open questions (resolve at activation)

- Compact encoding shape: format-run list vs per-row interned tag table + indices. (Lean:
  run list, since `FormatTag` runs are already the mental model and runs are typically very
  few per row. Decide in 118.1 with a quick size comparison on representative rows.)
- Inline-image scrollback rows: encode into the compact form, or opt out and keep as
  `Vec<Cell>`? (Lean: opt out — image rows are rare and the `Box<ImagePlacement>` complicates
  the flat encoding for negligible gain. Decide in 118.1.)
- New default scrollback value: decided in 118.5 from the measured per-line cost. Candidate
  framing: pick the largest round number whose post-compaction memory ≤ today's 4000-line
  uncompacted memory (likely in the tens of thousands).

---

## Task 119 — Scrollback Compression (LZ4)

### 119 Summary

Compress **blocks** of idle scrollback — already in the Task-118 flat compact form — with
LZ4, decompress on demand when scrolled into view, keep decompressed while visible, and
recompress/evict when the region scrolls back out. This is **phase two** of the
scrollback-memory effort: an incremental multiplier layered on the guaranteed Task-118 win,
targeting the aggregate-memory case that actually hurts — **many tabs/panes open at once**,
where the sum across buffers, not any single buffer, is the pressure.

Scope is deliberately the **compression core only**. Reflow interaction
(band-decompression) is explicitly **out of scope** and lives in Task 120, because it is the
same control flow as lazy reflow and must be built once, together. Task 119 must therefore
leave the existing (synchronous, full-scrollback) reflow path working correctly by
decompressing whatever it needs — slow on a huge scrollback, but correct; Task 120 makes it
fast.

### 119 What Task 118 already provides (reuse, do not rebuild)

Task 118 shipped the infrastructure that made compression the *smaller* half of the effort:

- **The flat, pointer-free compact form** (`CompactRow`, `freminal-buffer/src/compact_row.rs`)
  is the only thing safe to byte-compress — raw `Cell` holds `Arc`/`Box` pointers. LZ4
  operates on the serialized compact bytes, never on `Cell` directly.
- **The idle-driven background driver** (`freminal/src/gui/pty.rs` `select!` idle-tick arm:
  `crossbeam_channel::after(...)`, budgeted work, re-arm-while-work-remains, `never()` disarm
  when caught up) is exactly the mechanism idle *compression* needs. Compression is another
  kind of budgeted work the same tick drives — **do not add a second timer/thread.**
- **The decompact-on-read seam** via `Row`'s memoized accessors (`cells()`/`characters()`/
  `cells_mut()`) is architecturally the same seam decompress-on-scroll needs; extend it, don't
  invent a parallel one.
- **RSS-reclaim discipline** (`malloc_trim(0)` after the backlog drains, glibc-only) is
  already coded; compression's eviction path should honour the same lesson.

### 119 Design decisions (durable)

- **LZ4 is the only codec (no zstd tier).** The `fast-over-ratio` preference plus the
  on-the-fly profile (frequent, small, hot-path block reads) point at LZ4's low per-call
  overhead and ~2,600 MB/s decompress. A zstd "max savings" tier was explicitly dropped: it
  pulls a C dependency and a Windows-cross-check burden for a ratio gain that does not justify
  the complexity here. It may be revisited as a future refinement, not in this version.
- **Compress in blocks, never per line.** A ~40-byte line gives a terrible ratio and pays
  fixed per-call overhead on every access. Block granularity ≈128–256 logical scrollback rows
  (tuned in 119.5). Reading one line decompresses its whole block — a 256-line block ≈ ~88 KB
  flat ≈ ~34 µs at LZ4 speed, well under one 16.6 ms frame.
- **Never compress the active/visible region, nor the compact-but-uncompressed rows near the
  viewport.** Only scrollback idle past a threshold is a compression candidate. The visible
  `height` rows stay `Live`; the Task-118 compact rows near the viewport stay directly
  readable; only cold, deep blocks compress.
- **LRU cache of decompressed blocks + a reusable scratch buffer.** The jank in naive designs
  is allocation churn, not the codec. Decompress once on scroll-into-view, keep live while
  visible, recompress/evict on scroll-out. Steady-state scrolling within a cached region does
  **zero** decompression.
- **New dependency (`lz4_flex`), pure-Rust, added via `flake.nix` + `Cargo.toml`** per
  `flake-dev-shell-discipline` (add to flake, STOP, wait for `nix develop`). Pure-Rust LZ4,
  no C toolchain, Windows-clean. `freminal-buffer` currently has zero
  serialization/compression dependencies; this is the first.
- **Lives in `freminal-buffer`, below the snapshot line.** Compression is internal to the
  buffer; `build_snapshot()`, the terminal-emulator, and the GUI are unaffected — they read
  decompressed/decompacted rows through the existing flatten accessors. Respects the crate
  dependency boundaries in `freminal-architecture`.
- **Compressed blocks hold no `row_cache` entry.** Task 118 already evicts cache for compact
  scrollback rows; a compressed block is even colder and likewise carries no second flattened
  copy. Repopulate on decompress-for-read.
- **Correctness over ratio.** Every block round-trips losslessly to the Task-118 compact form
  and thence to `Row`/`Cell`. A wrong scrollback line is worse than a larger one.

### 119 Measured motivation (feasibility spike)

Ratios are **on top of** the Task-118 flat compact form (100k-line corpora, realistic
"stable-structure + unique-content" data + a pessimistic high-entropy bracket):

| Corpus                    | Flat (Task 118) | flat + LZ4  | Total vs. 72-byte cell |
| ------------------------- | --------------- | ----------- | ---------------------- |
| Shell session (typical)   | ~345 B/line     | ~106 B/line | ~39×                   |
| Source / logs             | ~310 B/line     | ~120 B/line | ~31×                   |
| High-entropy colored (WC) | ~732 B/line     | ~625 B/line | ~9× (worst case)       |

LZ4 decompress ~2,600 MB/s — far above any plausible scroll rate. The bulk throughput number
is **not** what governs on-the-fly cost; per-call overhead, block granularity, and allocation
churn are (addressed by the block + LRU + scratch-buffer decisions above).

### 119 Current-state map (confirm at activation)

- **`CompactRow`** — `freminal-buffer/src/compact_row.rs` (Task 118). Needs a stable byte
  serialization for LZ4 input; confirm whether the in-memory `CompactRow` is already
  contiguous-serializable or needs an explicit encode step.
- **Row storage enum** — `freminal-buffer/src/row.rs` (Task 118 Design B: `Row` holds
  `{ Live(Vec<Cell>), Compact(CompactRow) }`). A third state — a *reference into a compressed
  block* — is the shape to weigh in 119.1 (a `Row` whose storage is `Compressed(block_id,
  offset)` decompressed on access), vs. a separate block store indexed alongside `Buffer.rows`.
- **Idle driver** — `freminal/src/gui/pty.rs` idle-tick arm + `Buffer::compact_idle_scrollback`
  passthrough (`TerminalHandler`/`TerminalEmulator`). Compression reuses this entry point
  pattern (`compress_idle_scrollback` alongside/after compaction).
- **Flatten/read seam** — `Row` accessors + `buffer/flatten.rs`; decompress-on-read extends
  the Task-118 decompact-on-read.
- **Benchmarks** — `freminal-buffer/benches/buffer_row_bench.rs` + the memory benches Task 118
  hardened; add compression-specific block round-trip and scroll-into-compressed benches.

### 119 Subtasks

#### 119.1 — READ-ONLY design audit: block model + storage state + cache/driver seams

Scope: read-only across `compact_row.rs`, `row.rs`, `buffer/mod.rs`, `buffer/flatten.rs`, the
idle driver in `freminal/src/gui/pty.rs`, and the buffer benches.

What: produce the concrete design for: the compressed-block type and where blocks are stored
(a `Row` `Compressed` storage variant vs. a separate block store keyed alongside
`Buffer.rows`); the byte serialization of `CompactRow` fed to LZ4; block size and how logical
scrollback rows map to blocks (and how block boundaries survive scrollback eviction/drain
index-shifting); the LRU cache + scratch-buffer shape; the decompress-on-read seam
(extending Task 118's) and the compress-on-idle entry point (`Buffer::compress_idle_scrollback`
reusing the existing idle tick); `row_cache` interaction; and how the existing synchronous
reflow path decompresses what it needs (correct-but-slow, since fast reflow is Task 120).
Name every file each later subtask touches.

Deliverable: design report with the chosen types and file-scoping for 119.2–119.6. No code.

Verification: none (read-only).

Prohibitions: do NOT edit files; do NOT touch reflow performance (that is Task 120); do NOT
add the dependency yet; do NOT begin implementation; do NOT proceed without maintainer review.

Stop: report design; await explicit sign-off before 119.2.

#### 119.2 — Add `lz4_flex` dependency (flake + Cargo)

Scope: `flake.nix`, `freminal-buffer/Cargo.toml`, workspace `Cargo.toml` if versions are
pinned there.

What: add the pure-Rust `lz4_flex` crate per `flake-dev-shell-discipline` and the
dependency-hygiene rules in `rust-best-practices` (alphabetical sort, full semver pin). Per
the flake discipline: add to `flake.nix`, then **STOP and tell the maintainer to run
`nix develop` / `direnv allow`**, and wait for confirmation before writing code against it.

Deliverable: dependency added + confirmed available in the dev shell.

Verification: `cargo build` (the crate resolves); `cargo machete` (not flagged unused once
119.3 uses it — sequence accordingly).

Prohibitions: do NOT vendor a C-based codec; do NOT add zstd; do NOT proceed past the
STOP-and-wait until the shell is confirmed.

Stop: report; await confirmation the dev shell has the dep.

#### 119.3 — Compressed block type + lossless block round-trip (pure, in `freminal-buffer`)

Scope: new module `freminal-buffer/src/compressed_block.rs` (or as named in 119.1);
`freminal-buffer/src/lib.rs` (module decl); unit tests in the new module.

What: implement the block type chosen in 119.1: serialize a run of `CompactRow`s to bytes,
LZ4-compress to a block, and decompress back to the exact `CompactRow`s (and thence `Row`s).
Reusable scratch buffer for the decompress output. Pure data transform; no `Buffer`
integration yet.

Deliverable: block type + compress/decompress + exhaustive round-trip tests (plain, colored
runs, wide chars, URL tags, blank/sparse rows, block-boundary rows, a high-entropy block) and
a size assertion demonstrating the on-top-of-compact reduction on a representative block.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT touch `Buffer`; do NOT change `Cell`/`Row`/`CompactRow` public API; do
NOT wire the idle driver; do NOT proceed.

Stop: report + await review.

#### 119.4 — Buffer integration: compress cold blocks, decompress-on-read

Scope: `freminal-buffer/src/buffer/mod.rs` (block storage + the compress/decompress paths),
`row.rs` (storage state if a `Compressed` variant is chosen), `buffer/flatten.rs`
(decompress-on-read seam), the LRU cache.

What: store deep-cold scrollback as compressed blocks; decompress at the flatten/read boundary
(extending Task 118's decompact-on-read) so no higher layer observes the change; LRU-cache
decompressed blocks, keep live while visible, recompress/evict on scroll-out. Preserve every
existing `Buffer` behaviour (visible_rows, scrollback eviction index-shifting for
`prompt_rows`/`command_blocks`, alt-screen switch). The **existing synchronous reflow must
still work** by decompressing what it needs (slow on huge scrollback — Task 120 fixes speed).

Deliverable: integration + tests proving identical observable output (flatten, visible_rows,
snapshot content) before/after compression across scroll; scroll-into-a-compressed-block
decompresses and caches; scroll-out recompresses/evicts; eviction still shifts dependent
indices correctly.

Verification: `cargo test --all`; clippy. Existing buffer + Task-118 tests pass unchanged.

Prohibitions: do NOT compress the visible region; do NOT change snapshot/public API; do NOT
optimise reflow (Task 120); do NOT proceed.

Stop: report + await review.

#### 119.5 — Idle-driven compression via the existing tick + block-size tuning

Scope: `Buffer::compress_idle_scrollback(budget)` (`freminal-buffer`), passthrough on
`TerminalHandler`/`TerminalEmulator`, `freminal/src/gui/pty.rs` (extend the existing idle-tick
arm — no new timer/thread).

What: implement compression as budgeted idle work on the **existing** PTY-thread idle tick,
after compaction has caught up (compact first, then compress the now-cold compact blocks).
Re-arm while either compaction or compression has work; disarm (`never()`) when both are
caught up so a quiescent pane is not woken. Honour the `malloc_trim` RSS-reclaim discipline
Task 118 established. Tune block size (128 vs 256) and the idle-past threshold against measured
behaviour.

Deliverable: idle compression wired into the one tick; a test that a scrollback fill stays
`Live` → compacts → compresses across successive idle calls; block-size decision recorded with
the measurement.

Verification: `cargo test --all`; clippy; `cargo xtask check-windows` (touches the PTY thread
/ crossbeam select).

Prohibitions: do NOT add a second timer or thread; do NOT compress on any hot path; do NOT let
the tick busy-wake a fully-idle terminal; do NOT proceed.

Stop: report + await review.

#### 119.6 — Benchmarks, config, escape-sequence-doc check, Windows cross-check

Scope: buffer + emulator benches; `freminal-common/src/config.rs` (any new
`[scrollback]`/compression key, full `freminal-config-options` wiring if added);
`config_example.toml`; verification suite.

What: before/after memory + throughput per `performance-benchmarks` + `freminal-bench-table`
for the scrollback flatten/render/build_snapshot benches plus new
block-round-trip / scroll-into-compressed benches. Confirm no >15% regression on the
read/flatten hot paths (cold decompress-on-read may slow; the visible-region path must not
regress). If a config toggle or capacity knob is added, wire it fully (no `apply_partial`
omission). No escape-sequence surface changes are expected — confirm and note. Run the full
suite + `cargo xtask check-windows`.

Deliverable: benchmark record (before/after) + any config wiring + green suite + Windows
cross-check.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`;
`cargo machete`; `cargo fmt --all -- --check`; `cargo bench --no-run --all`;
`cargo xtask check-windows`; markdownlint clean for any doc edits.

Prohibitions: do NOT skip config wiring if a key is added; do NOT regress the visible-region
path >15%; do NOT proceed past a failing check.

Stop: report results.

### 119 Open questions (resolve at activation)

- Block storage: a `Row` `Compressed(block_id, offset)` storage variant vs. a separate block
  store indexed alongside `Buffer.rows`. (Lean: separate block store — a block spans many rows,
  so per-row storage variants fragment the mental model. Decide in 119.1.)
- Block size (128 vs 256 lines) and idle-past threshold — tune in 119.5 against measured
  scroll behaviour.
- LRU sizing (how many decompressed blocks kept live) and eviction policy — decide in 119.4/119.5.
- Compress ordering vs. compaction on the shared idle tick: strictly compact-then-compress, or
  interleaved under one budget? (Lean: compact-then-compress; simpler invariants. Decide in 119.5.)

---

## Task 120 — Compression-Aware Windowed Reflow

> **STATUS: ENRICHED STUB.** Durable design decisions are captured below; per-subtask
> decomposition happens at activation in a dedicated session, against the code as it then
> exists (see the `freminal-version-activation` skill). Do not invent subtasks early.

### 120 Summary

Make width-resize reflow of a very large scrollback affordable. Once Task 118 (compact) and
Task 119 (LZ4 compression) make tens-of-thousands-to-100k-line scrollback the norm,
**synchronous full-scrollback reflow becomes the new latency wall** — and Task 119
deliberately left the existing reflow correct-but-slow (it decompresses everything it needs).
This task fixes reflow speed with the same recency-first, eventually-consistent philosophy the
memory tasks apply to compaction and compression.

This task **absorbs two previously-separate pieces** that turned out to be one control flow:

1. The former **118.10** lazy/windowed-reflow stub.
2. The **reflow half of the original Task 119** (band-decompression on resize).

They are unified because *the band you decompress is the band you reflow, and the async tail
that finishes decompression is the async tail that finishes reflow.* Building them separately
would construct the lazy-reflow band machinery twice.

### 120 Design principle (durable)

On a width resize:

1. Reflow only the **visible region plus a small scroll-headroom margin** synchronously —
   band-decompressing only the blocks that band needs — producing a correct snapshot for the
   current viewport essentially instantly.
2. **Publish that snapshot immediately**; the user sees the resized view with no perceptible
   delay.
3. Reflow (and re-decompress as needed) the remaining scrollback **lazily/incrementally in the
   background** — reusing the Task-118/119 idle-tick driver — and/or **on-demand as the user
   scrolls up** into not-yet-reflowed history. Recompaction and recompression of reflowed rows
   then follow the normal deferred path.

Reflow cost becomes proportional to what is *visible*, not to total scrollback depth.

### 120 Why this is a stub, not decomposed now

Lazy, compression-aware reflow is substantially larger and subtler than the 118/119 memory
work. It touches logical-line reconstruction, cursor remapping, band-decompression, and — the
hard part — the **`command_blocks` / `prompt_rows` absolute-index remapping** (Task 113 "Bug
R") across a buffer that is only *partially* reflowed to the current width **and** partially
compressed. The buffer must track which scrollback regions are reflowed-to-current-width vs
stale, handle a scroll into a stale and/or compressed region (reflow-and-decompress-on-read),
and keep the absolute-index remaps correct while regions carry mixed widths and mixed
compression states.

Open design questions to resolve at activation:

- How to represent "target width" per row/region, and whether stale regions store their
  pre-resize width for on-read reflow.
- How scroll-offset maps onto a mixed-width, mixed-compression buffer.
- How `visible_window_start` and snapshot bounds behave mid-reflow.
- How the single idle driver sequences three kinds of deferred work — compaction (118),
  compression (119), and reflow-tail (120): shared budget? strict ordering? priority?
- Which thread performs the deferred full reflow, and how a partially-reflowed snapshot is
  represented without violating the lock-free snapshot model (`freminal-architecture`).

Depends on Task 118 (compact representation + idle driver) and Task 119 (block compression +
band-decompression primitive). Decompose in a dedicated session against the code as it then
exists, per `freminal-version-activation`.

---

## Task 121 — Performance Remediation

> **STATUS: COMPLETE — closed 2026-08-20 as an umbrella, not finished.** Summary only. The
> full per-subtask breakdown and the migration map live in
> `Documents/PLAN_121_PERF_REMEDIATION.md`, which is now a **historical record**. Surviving
> work moved to **Task 123** (`PLAN_123_GL_MEASUREMENT_HARNESS.md`) and **Task 124**
> (`PLAN_124_RENDER_EFFICIENCY.md`). Do not resume work from the Task 121 document.

### 121 Summary

Task 121 is the umbrella for **all** performance remediation arising from GitHub issue #459
(real-workload CPU profiling findings, still open): the work that has already landed, the
bugs that work surfaced but did not fix, and the issue #459 candidate items nobody has
actioned.

It ran for five merged pull requests (#458, #460, #461, #464, #465) under a task number that
existed only in branch names — there was no Task 121 in `MASTER_PLAN.md` at all. Creating it
here closes that tracking gap.

### 121 Goal

Reduce freminal's real-workload CPU cost — idle, pointer motion, typing, and full-screen TUI
redraw — to the level set by wezterm and ghostty on the same hardware.

### 121 Subtask summary

| Group                                 | Subtasks              | Status      | Covers                                                                                           |
| ------------------------------------- | --------------------- | ----------- | ------------------------------------------------------------------------------------------------ |
| A — Completed work                    | 121.1–121.11          | Complete    | PRs #458, #460, #461, #464, #465                                                                 |
| B — Bugs found and fixed              | 121.12–121.14         | Complete    | blink-off fallback; animation signal. NB 121.13 was reverted 2026-08-02 — see 121.32             |
| B — Bug blocked behind Task 122       | 121.15                | Unblocked   | pane-wide `has_urls` / `scroll_offset` vetoes; left to 121.17                                    |
| B — Withdrawn                         | 121.16                | Withdrawn   | config kill switch — rejected; revert-and-fix is the remedy                                      |
| C — Unifying improvement              | 121.17                | Not started | cell-granular pointer suppression; Task 122 seam landed, chrome-cache numbers stale (re-measure) |
| D — Reconned, premise does not hold   | 121.18, 121.19, 121.21, 121.22 | Closed | items 3, 4, 6, 7 — all four closed as not actionable as framed                          |
| D — Reconned, needs maintainer gate   | 121.20                | Blocked     | item 5 — premise confirmed; needs 121.28 or an agreed manual-QA gate                             |
| D — Measured and refuted              | 121.24                | Complete    | per-`CursorMoved` allocations are ~1.4% of the residual; no fix warranted                        |
| D — Profiling methodology             | 121.23                | Complete    | `Documents/PROFILING.md`; fixed a `Cargo.toml` ref to a nonexistent file                         |
| E — Measurement debt                  | 121.27–121.28         | Not started | `DESIGN_DECISIONS.md` entry, issue #440 pixel harness                                            |
| E — Measurement debt (partly done)    | 121.25                | In progress | clean Finding 3 re-run done; typing and btop outstanding                                         |
| E — Blink-off comparison              | 121.26                | Complete    | blink-off ≈ blink-on ≈ 0.0–0.1% at idle; resolution-limited                                      |
| F — Surfaced by the Group B work      | 121.29–121.31         | Not started | `repaint_causes()`; chrome not built on `Replay`; full present on motion                         |
| G — beta.7 interaction regression     | 121.32                | Complete    | chrome cache disabled by default; tab-click / border-drag regression fixed                       |
| G — Surfaced by 121.32                | 121.33                | Not started | `Full`/`Replay` `Ui` id divergence churns pane-border drag state                                 |
| G — Chrome-cache decision gate        | 121.34                | Not started | measure always-`Full` cost; decides keep/delete/confine (121.32)                                 |
| G — Chrome-cache waste while disabled | 121.35                | Deferred    | stop populating cache while disabled; Task 122 takes priority                                    |
| G — Confine Replay to non-chrome      | 121.36                | Conditional | confine `Replay` to pointer-not-over-chrome frames; conditional on 121.34, blocked on 121.33     |

### 121 Headline result

The single highest-value change was `7d483998` (subtask 121.8): `WindowEvent::RedrawRequested`
had been permanently disqualifying `ChromeMode::Replay`, so the entire issue #436 chrome-cache
subsystem had been inert since the day it landed. A one-line carve-out took steady-state idle
`Replay` duty cycle from 0% to 100%, chrome construction from 69 us to 10 us per frame (-86%),
and total idle frame cost from 434 us to 376 us (-13.4%).

The pointer-frame suppression in `19780e16` (subtask 121.10) is a **spike**, default-on with
no kill switch — and it stays that way; a config toggle was proposed and rejected (121.16,
withdrawn), because it would ship two code paths and test neither. If the suppression
misbehaves it is a bug, and the remedy is a fix or a revert. Its headline number — 61fps to
2.05fps under pointer motion
over static content — came from a confounded bench run but was **subsequently corroborated by
an independent A/B on different hardware**. The wezterm comparison in that A/B is not
apples-to-apples (wezterm is not blinking a cursor at 2 Hz). Do not restate these results more
strongly than `DECOUPLING_FRAMEWORK.md` §2A does.

### 121 Relationship to `DECOUPLING_FRAMEWORK.md`

`Documents/DECOUPLING_FRAMEWORK.md` is the **decision record** for "should freminal stop
using egui for the main window?", plus the rewrite-if-chosen plan (its Phases 1–5). Its
status is **reopened, leaning against the rewrite, explicitly undecided** — Phase 0
measurement found three cheap fixes inside egui that recovered most of the benefit the
rewrite was meant to deliver. It is not a `PLAN_VERSION_*.md` and its phases are not tasks in
`MASTER_PLAN.md`.

Task 121 is the **performance remediation work itself**, and it stands regardless of how that
decision falls. `DECOUPLING_FRAMEWORK.md` §2A is the source of truth for the Phase 0
measurements, the three findings, and the known gaps in the Finding 3 spike; do not re-derive
those numbers and do not contradict them.

---

## Task 122 — Orchestration Extraction

> **STATUS: COMPLETE.** Merged to `main` on 2026-08-03 via PR #472 (merge commit
> `e533ed00`). The full per-subtask breakdown is
> `Documents/PLAN_122_ORCHESTRATION_EXTRACTION.md` (17 subtasks in five groups). That
> document **supersedes** `Documents/DECOUPLING_FRAMEWORK.md` §8 Phase 1, whose subtasks
> 1.1–1.6 and line counts are stale. This section is a summary only.

### 122 Summary

Decompose the GUI binary's god functions and give orchestration logic — event triage, view
window, input encoding, frame decisions — a home. No behaviour change; `cargo test --all`
must be green at every step and the app usable throughout.

Scope, per §8 Phase 1: `App::update` and the `central_body` closure, `terminal/widget.rs::show`,
`terminal/input.rs::write_input_to_terminal`, toolkit-neutral `Rect` / `Point` in
`freminal-common` to get `panes/mod.rs` layout and hit-test math off `egui::Rect` / `Pos2`,
the `gui_scroll_offset` / `gui_extra_rows` naming leak, and designing the layer as if it were
a crate while landing it as a module first.

### 122 Why it is on the roadmap at all

It is the **one** phase of `DECOUPLING_FRAMEWORK.md` that is required whichever way the egui
rewrite decision falls, so it is a roadmap task rather than a rewrite phase. Phase 0
measurement showed the rewrite case is a maintainability judgement, not a performance
necessity — which reframes this work: it is the deliverable that makes that judgement
answerable, by separating "the damage-tracking machinery is inherently ugly" from "the
machinery has nowhere to live". Those are different findings with different verdicts, and
today they cannot be told apart.

Task 121's own work kept producing evidence for it. `DECOUPLING_FRAMEWORK.md` §12 describes
PR #464's `post_event` classifier as "a clean example of orchestration logic wanting a home",
and §2A records that the pointer-suppression predicate "already needed four rounds".

**What it does not buy:** Task 122 retires **none** of the 13 assumptions in
`EGUI_UPGRADE_ASSUMPTIONS.md`. Per §3 those only die when chrome leaves the main window's
`Context`, which is Phase 3 of the rewrite. Task 122 addresses the "ugliness" argument and
part of the "edge cases" argument, and nothing of the undocumented-internals argument. It is
not a substitute for the rewrite on the maintainability axis; it is what lets the rewrite be
priced accurately.

### 122 Sequencing within the version

Independent of Tasks 118–120 entirely (different crates). Against Task 121 it was a blocker
for exactly one subtask, now discharged:

- **121.17 (cell-granular suppression) depended on Task 122** for per-pane render-time
  geometry captured during `update()` and read from the event layer — the seam this task
  built. Adding a fifth round to the suppression predicate in its current shape would have
  made the maintainability argument for the rewrite stronger for no good reason. Task 122
  merged (PR #472, 2026-08-03) and subtask 122.15 publishes that seam, so 121.17 is now
  **unblocked** — see its entry in `PLAN_121_PERF_REMEDIATION.md` for the re-check-your-
  assumptions caveat left by the chrome-cache changes.
- **Everything else in Task 121 was independent.** Groups D and E live in `shaping.rs`,
  `vertex.rs`, `atlas.rs`, the GL layer and `freminal-windowing`, none of which Task 122
  touched; 121.12–121.15 sit in already-extracted predicates.

### 122 Activation outcome (2026-07-30)

The activation pass is **done**; see `Documents/PLAN_122_ORCHESTRATION_EXTRACTION.md` for
the breakdown. Re-measured on `main` at `f8ebd17a`: `App::update` **3,132** lines (§8 says
2,743; 3,051 at this task's creation), the `central_body` closure **2,033** (§8 says 1,859),
`terminal/widget.rs::show` **1,882**, `write_input_to_terminal` **1,226** (static across
three measurements), and `panes/mod.rs` **58** `Rect` / `Pos2` occurrences (static).

Three findings changed the shape, and are recorded in full in the plan document:

- **The drift is entirely in the frame path.** Two of §8's five targets
  (`write_input_to_terminal`, `panes/mod.rs`) have not moved at all, so §8's flat
  equal-priority list is the wrong weighting. The breakdown demotes both to cleanup.
- **The growth has a nameable cause**: render-time state written during a frame purely so
  an out-of-frame consumer can read it, with no name, type or enforced invariant — the
  ~7 `PerWindowState` fields that `pointer_motion_needs_repaint` and
  `is_chrome_interactive_at` read from `freminal-windowing`'s `CursorMoved` fast path.
  That seam is the deliverable; god-function decomposition is the mechanism, not the goal.
- **`write_input_to_terminal` has 17 parameters, not 16** (this document and §8 both said
  16; the 17th is `super_state`, from 101.2). It is **not** decomposed by Task 122 — its
  concerns are interleaved rather than separable by line range, and the apparent
  de-duplication win is a semantic trap (see cleanup entry 122.C1).

---

## Task 123 — GL Pipeline Measurement Harness

> **STATUS: PLANNED.** Summary only. The full per-subtask breakdown (123.1–123.14) lives in
> `Documents/PLAN_123_GL_MEASUREMENT_HARNESS.md` — edit that document, not this section,
> when subtask status changes.

### 123 Summary

The instrument Task 121 never had. **Task 123 changes no rendering behaviour**: it builds a
measurement harness and reports numbers, and every fix it justifies belongs to Task 124.
It supersedes 121.28 (the pixel harness that never landed) and absorbs 121.25's outstanding
measurement debt.

**Phase 1 — call recording, no GPU, no new infrastructure.** `glow::HasContext` is sealed
(`glow-0.17.0/src/lib.rs:142`, `__private::Sealed` at `:4845-4849`), so it cannot be
implemented on a wrapper — that is a hard compile error, not a design tradeoff. It does not
matter: freminal is monomorphic over the concrete `&glow::Context` at roughly 40 parameters
with no generic bounds to rewire, and uses only **47** of the trait's 396 entry points. A
concrete facade struct with a real backend and a recording backend therefore covers the whole
surface, and glow's handle types have public tuple fields
(`pub struct NativeBuffer(pub NonZeroU32)`, `native.rs:169`) so the recording backend can
fabricate them. `RenderState`, `GlyphAtlas` and `FontManager` already construct headlessly —
`render_loop_bench.rs` does it at seven call sites — so the whole render path can be driven
with no window, no context and no driver, in the existing `cargo test` matrix on all four CI
platforms.

**Phase 2 — pixel readback, Linux-only, new infrastructure.** `GlState::new` is hard-wired to
a winit `Window` and `WindowSurface`; glutin 0.32.3 does expose `PbufferSurface`, so an
offscreen path is possible but is new code. `flake.nix` currently has **no Mesa driver** —
`pkgs.libGL` is `libglvnd`, a dispatcher with no rendering backend — and no Xvfb, so
`pkgs.mesa`, `mesa.llvmpipeHook` and `pkgs.xorg.xvfb` must be added, which per
`flake-dev-shell-discipline` is a stop-and-wait-for-`nix develop` subtask. It also needs a
**new Nix-based CI job**, because `ci.yml`'s existing test matrix runs on
`dtolnay/rust-toolchain` and inherits nothing from the flake. llvmpipe output varies across
Mesa versions, so the tolerance policy is decided up front and Phase 2 does not gate PRs until
it has demonstrated stability — `flaky-tests-are-bugs` forbids retrofitting a tolerance to
make a flaky golden pass.

**Two diagnostic obligations** ride along: confirm or refute the dirty-row `Arc` churn
hypothesis that is Task 124.1's premise, and diagnose the 121.31 anomaly
(`frame_damage_full=120, frame_damage_partial=0` during pointer motion, versus 120/120
partial at idle, never explained; `toast_active=48` from a startup toast is the recorded
confound).

---

## Task 124 — Render Efficiency Remediation

> **STATUS: STUB, gated on Task 123.** Summary only. The full breakdown (124.1–124.8) lives
> in `Documents/PLAN_124_RENDER_EFFICIENCY.md`.

### 124 Summary

The fixes. **No subtask is implemented before Task 123 has quantified what it claims to
fix** — the direct lesson of Task 121's Group D, where four of six issue #459 candidate items
were refuted by their own verification step. The single exception is 124.4, a
`state-representation` fix with no expected performance effect.

It carries the surviving work from Task 121 — cell-granular pointer suppression (121.15 and
121.17, whose measured prize was a 99.16% to 1.68% collapse in suppression from a single
on-screen hyperlink), the full-present-on-pointer-motion anomaly (121.31's fix half), the
chrome-cache keep-or-delete decision (121.34, with 121.30, 121.33, 121.35 and 121.36 folded
in), the two surviving shaping levers (121.19), GPU buffer orphaning (121.20, unblocked by
123's Phase 2), and the `DESIGN_DECISIONS.md` entry (121.27).

Its own leading hypothesis is **124.1**: `rows_as_tchars_and_tags_incremental`
(`freminal-buffer/src/buffer/flatten.rs:530-533`) mints a fresh `Arc` whenever any row is
dirty, even when the merged bytes are byte-identical, so `frame_dirty.rs`'s `Arc::ptr_eq`
test reports `content_changed` and forces a full vertex rebuild and a full present. Any
workload that touches rows every tick pays that, whether or not a pixel changed. Recon on
2026-08-20 established this is **workload-correlated, not alt-screen-specific** — every
branch on `is_alternate_screen` in the render path *suppresses* work, and a primary-screen
`watch` would behave identically.
