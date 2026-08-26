# PLAN_125_VERTEX_RELAYOUT.md — Task 125 "Performance Parity and Residual Remediation"

> **STATUS: PLANNED — measurement phase activated 2026-08-26.** The
> measurement phase is decomposed below against the post-Task-124 codebase.
> No remediation is selected or decomposed yet. Fixed-stride relayout remains
> one conditional branch, not the task goal, and cannot affect cursor-only
> idle frames.
>
> **Version: v0.12.0.** Task 124 is complete and merged. The maintainer assigned
> Task 125 to v0.12.0 on 2026-08-26; the release now gates on the measurement
> findings and whichever remediation branches those findings select.

---

## Goal

Under controlled equivalent workloads, bring Freminal's CPU and GPU cost into
parity with WezTerm and Ghostty closely enough that no persistent overhead
remains unexplained.

The target is product-level resource use, not a particular implementation.
Fixed-stride per-row GPU upload remains a candidate for sparse active
workloads, but it is neither the task's governing goal nor a foregone
conclusion. CPU scheduling, egui/chrome construction, vertex construction,
driver uploads, GPU execution, presentation, and compositor interaction are
all in scope when measurement attributes meaningful residual cost to them.

Difficulty or a small expected return is not grounds for omitting a candidate.
Every credible remediation is recorded with honest benefit, complexity,
correctness risk, and portability cost. The final decision may still be to
accept a measured residual, but only after it is explained.

## Activation decision from 2026-08-26 recon

**Do not activate the old fixed-stride implementation plan as written.** Its
gate was not discharged:

- Task 124's authoritative sustained-output capture recorded 2,104 `Partial`,
  281 `None`, and 15 `Full` outcomes across 2,400 frames at 60.24 fps, but did
  not distinguish `VertexRebuild::Bounded` from other partial sources, record
  changed-row counts, or record live upload bytes.
- Task 123 measures deterministic GL calls and synthetic upload volume. It
  does not measure actual GPU execution time, driver stalls, GPU utilisation,
  power, or compositor cost.
- A cursor-only blink frame already avoids background and foreground uploads;
  Task 125's former relayout cannot improve true idle. A steady-cursor idle
  terminal draws no frame at all.
- Every redraw still runs egui's UI pass. Historical genuine-idle profiling
  measured 434 us/frame at 1.95 fps: 96 us in Freminal, 89 us in egui, 226 us
  in present, and 23 us unmeasured. This is the leading idle-parity surface,
  not vertex bandwidth.
- The existing foreground one-row benchmark creates a fresh glyph atlas per
  timed iteration. The 2026-08-26 recon measured 906.29 us for all 50 rows
  against 751.24 us for one row at 200 columns, showing that atlas setup and
  rasterisation dominate the result. It does not isolate incremental vertex
  construction. The background counterpart measured 133.97 ns for all rows
  against 14.41 ns for one row, but its corpus does not establish the
  fixed-stride padding cost.

The first activated work is therefore measurement infrastructure and a
controlled parity capture. Remediation is selected afterward.

---

## Relationship to Tasks 123 and 124

The original division remains useful but is no longer the complete task:

- **Task 124 stops doing work that produces no pixels.** It is the
  frame-count and present win.
- **Task 125 explains and remediates what remains.** Per-row upload is one
  possible bandwidth win; idle and chrome costs live elsewhere.

Task 123 measured the size of each. A needless full rebuild is roughly
**350x the bytes for roughly 1.08x the calls** — so 124's prize is counted in
frames avoided and 125's is counted in bytes not moved. Roughly 200 KB per
full-rebuild frame against under a kilobyte for a cursor-only frame
(`PLAN_123_GL_MEASUREMENT_HARNESS.md`, "Per-workload GL cost, 80x24").

### The old relayout gate

Task 124 has landed, but it did not measure the live residual in the units the
relayout decision needs. A `Region` frame bounds clear, draw, and present; it
still performs a full vertex rebuild and whole-buffer upload. The missing
gate is the distribution of `VertexRebuild` outcomes, changed-row counts, and
bytes uploaded per real frame.

If realistic sparse-update workloads rarely reach `Bounded`, or usually
change nearly every visible row, the fixed-stride branch closes unexecuted.
That result does not close Task 125: measurements may instead select an idle,
chrome, scheduling, CPU-build, driver, or presentation remediation.

That gate is not ceremonial. It is the direct lesson of Task 121, which
closed with four of six candidate items refuted by their own verification
step, and of Task 123's Group G precedent where three code-reading
hypotheses were falsified in sequence before measurement found the real
cause.

---

## Fixed-stride candidate, as established by recon (2026-08-23)

`upload_verts` (`freminal/src/gui/renderer/gpu.rs:1749-1763`) orphans the
whole buffer and rewrites it from offset zero on every upload:

```rust
gl.buffer_data_size(glow::ARRAY_BUFFER, gl_i32(bytes.len()), glow::STREAM_DRAW);
gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytes);
```

Despite the `buffer_sub_data` name this is a whole-buffer replace, not a
partial update. All four call sites in `gpu.rs` and the two in
`toast_pass.rs` / `toast_text_pass.rs` pass offset zero and the entire
slice. No non-zero offset exists anywhere in the codebase.

It has to work this way today, because **none of the three instance buffers
has a fixed stride per row**:

| Buffer         | Emission rule                                                                            | Why the count varies                                                                                   |
| -------------- | ---------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| `bg_instances` | one 6-float instance per **non-default-background** cell (`vertex.rs:361-431`)           | `DefaultBackground` cells are skipped entirely (`vertex.rs:405-415`); count depends on content         |
| `fg_instances` | one 13-float instance per **glyph** (`vertex.rs:722-784`)                                | ligature clusters, wide characters, and blink-hidden runs contributing zero                            |
| `deco_verts`   | per-row underline and strikethrough quads, **interleaved** with non-row-scoped artifacts | search highlights, command-block hover tint, selection spans, and the cursor quad always appended last |

So row N's byte range differs in **length** between frames. A one-row change
is an insert-and-delete-and-shift for every row after it, not an overwrite in
place. `deco_verts` is worse than variable-length: a single row is not even
guaranteed to be _contiguous_, since a row can receive an underline quad in
the first pass and then be touched again by a selection quad appended much
later in the buffer.

`row_offsets` — the one per-row boundary index the buffer layer already
computes and threads onto the snapshot — describes offsets into the
**character** stream, which has a fixed relationship to row index. It has no
analogue for the instance buffers.

---

## Conditional fixed-stride design decisions

These apply only if the measurement phase selects fixed-stride relayout. They
were settled at Task 124's activation and should not be re-derived without new
evidence.

### 1. The mechanism is a fixed-stride relayout, not an offset table

Two designs were considered:

- **Fixed stride.** Emit exactly `term_width` background slots per row, using
  a degenerate or zero-coverage instance for default-background cells, so
  row N's start offset is `row_idx * term_width * stride`.
- **Maintained offset table.** Keep variable-length emission and maintain a
  persistent per-row offset-and-length table incrementally.

**Fixed stride is the chosen direction.** The offset table must be correctly
invalidated on every possible per-row content, format, or blink-visibility
change — including `fg_instances` count changes caused by blink visibility
toggling with no content change at all — which is strictly more bookkeeping
than the flat rebuild it replaces. It trades a cheap uniform cost for an
expensive correctness obligation.

Activation must nonetheless re-examine this against the code as it then
stands, and must quantify the padding's cost: fixed stride means uploading
slots for cells that emit nothing, so the byte volume of a _full_ rebuild
goes **up**. The design only pays if partial uploads are common enough to
cover that. Measure both.

### 2. The padded slots must emit no fragments

**This is the hard constraint and it is inherited from Task 34.**

`DefaultBackground` is not a colour. It is "leave these pixels untouched".
`background_opacity < 1.0`, background images, and the window
post-processing shader FBO all depend on those pixels showing the base state
written by `GlState::clear`. Task 123's Phase 2 measured **70.2% of a dense
text frame at `alpha == 0`** and pinned it with
`pixel_harness.rs::default_background_cells_are_left_untouched`.

The padding introduced by a fixed-stride layout exists for **addressing, not
painting**. A padded slot must be degenerate — zero area, or discarded — so
that nothing is drawn that is not drawn today. An implementation that
"simplifies" by painting every cell an explicit opaque background deletes
window transparency, and **no call-count test will catch it**. Use the pixel
harness.

Recorded because it was raised at 124's activation and is a natural idea:
painting every cell the clear colour _including its alpha_ does not work as
a substitute for the clear either. With blending enabled an alpha-zero quad
writes nothing and therefore does not erase stale pixels; disabling blending
to force a replace breaks the image and shader compositing layers.

### 3. The clear stays

`GlState::clear` (`freminal-windowing/src/gl_context.rs:363-369`) is two
zero-byte calls on a GPU fast-clear path, already skipped on partial frames
behind a genuine `EGL_EXT_buffer_age` query
(`egui_integration.rs:1195-1208`), with a real `eglSwapBuffersWithDamageKHR`
present. It is load-bearing for the property in decision 2. It fires more
often than it should today, and that is fixed by Task 124.2's
`FrameDamage::None`, not by removing it.

### 4. `deco_verts` may not be relayoutable and that is an acceptable outcome

The background and foreground instance buffers are per-cell and per-glyph and
map naturally onto a per-row stride. `deco_verts` mixes per-row decorations
with multi-row spans and a singleton cursor quad, and its ordering is
load-bearing — the cursor quad's always-last invariant is what makes the
existing cursor-only patch offset stable (`vertex.rs:340-341, 619`;
`widget.rs:2812-2816`).

Activation may legitimately conclude that `bg_instances` and `fg_instances`
are relayed out and `deco_verts` is left whole-buffer. It is the smallest of
the three and the cursor-only fast path already handles its common case.
**Do not break the cursor-quad-last invariant** to achieve uniformity.

### 5. Scope explicitly excludes the orphan decision

Whether `upload_verts` should orphan at all for small payloads is
**Task 124.7**, not this task. 124.7 gates the orphan; 125 changes the
layout. If 124.7 has landed by the time this activates, build on its result;
do not redo it.

---

## Activation decisions

The maintainer resolved the workload and parity semantics on 2026-08-26:

- **Authority machine:** the current AMD/Hyprland workstation is authoritative.
  The slower laptop is a conditional confirmation target only if the
  workstation does not reproduce the reported gap or a selected lever is
  driver/presentation-sensitive.
- **Peer binaries:** use the installed binaries and record both their displayed
  versions and immutable Nix store paths: WezTerm
  `0-unstable-2026-08-12` at
  `/nix/store/fjd3yyncgw5wj0vv8wkvlibp5z4wqqzg-wezterm-0-unstable-2026-08-12/bin/wezterm`
  and Ghostty `1.3.1` at
  `/nix/store/ij9fvnhfj710aafmlav1psl434cw5wqc-ghostty-1.3.1/bin/ghostty`.
  Record the Freminal commit and active GL renderer with every capture.
- **CPU parity:** seven 60-second steady samples after a 10-second warm-up,
  interleaving terminal order between repeats. A gap is material only when the
  deterministic 10,000-resample bootstrap 95% confidence interval for the
  median paired task-clock delta excludes zero **and** the median delta is at
  least 0.5 ms task-clock per wall-second (0.05% of one core). Smaller
  statistically separable residuals must still be explained.
- **GPU parity:** use cumulative per-process AMD DRM fdinfo engine time for
  matched cross-terminal comparison, with `amdgpu_top` for device/process
  discovery and asynchronous OpenGL timestamp queries inside Freminal for
  attribution. If the fdinfo fields are absent or too coarse to distinguish
  the workloads, the cross-terminal GPU verdict is `INCONCLUSIVE`, never
  inferred from llvmpipe or CPU time.
- **Controlled geometry/config:** 120x40 terminal cells, CaskaydiaCove Nerd
  Font at 12 pt, opaque background, no background image, no user shader, no
  cursor trail, the same clean interactive shell/prompt, and isolated
  config/state directories. Ligatures stay enabled for all three terminals.
- **Chrome topology:** four tabs, with the active tab containing a 2x2 pane
  layout. The other tabs and all inactive panes are idle.
- **Pointer workload:** a timed physical-device capture, because compositor
  synthetic motion is known not to produce application `CursorMoved` events
  on this host. Report the observed Freminal event rate with the result; do not
  present unmatched hand motion as exact input equivalence.
- **Cursor default:** both visible blinking and steady cursor idle are measured.
  Changing the product default to steady is not a remediation and cannot close
  the blinking-cursor gate.
- **Version assignment:** v0.12.0, by maintainer decision on 2026-08-26.

## Current-code map

- `freminal/src/gui/terminal/frame_dirty.rs` owns
  `VertexRebuild::{CursorOnly, Bounded, ReevaluateFullRebuild}` and
  `ChangedRows::{None, Rows, All}`. `Bounded` still means a full CPU rebuild
  with bounded presentation damage.
- `freminal/src/gui/terminal/widget.rs` resolves
  `ReevaluateFullRebuild` into a real rebuild or buffer reuse, constructs all
  four CPU vertex buffers, and registers the pane paint callback. It is the
  only point that can pair the raw decision, resolved work, and changed-row
  count without recomputing them.
- `freminal/src/gui/renderer/gpu.rs` owns every terminal VBO and texture upload.
  `upload_verts` still orphan-writes whole buffers; cursor-only frames upload
  decorations only. Atlas and image-texture uploads also occur here.
- `freminal/src/gui/renderer/gl_facade/` freezes the production renderer's 49
  GL methods. Timer-query calls are new measurement calls and must extend that
  facade rather than bypass it from renderer code.
- `freminal-windowing/src/frame_paint.rs` owns the head/band/tail paint split.
  Head and tail are chrome; the band contains terminal callbacks. This is the
  only existing seam that can time chrome and whole-frame GPU work without
  guessing from CPU wall time.
- `freminal/src/gui/window.rs` and
  `freminal-windowing/src/egui_integration.rs` own the existing feature-gated
  CPU frame accumulators and 120-frame log cadence. New measurement state gets
  its own cohesive modules rather than adding unrelated fields to either
  already-wide accumulator.
- `freminal/benches/render_loop_bench.rs` recreates a fresh foreground glyph
  atlas inside each timed partial-dirty iteration. Its one-row result therefore
  measures atlas creation/rasterisation more than row vertex construction.
- `freminal/benches/chrome_cost_bench.rs` is a representative headless chrome
  stand-in, not literal production chrome. Live external and frame/GPU
  profiling remain authoritative for product parity.

## Execution model

```text
tooling and protocol:
  125.1 -> STOP for nix develop -> 125.2

independent CPU measurement work after 125.2:
  125.3
  125.4 -> 125.5 -> 125.6

GPU timing spine after 125.4:
  125.7 -> 125.8 -> 125.9

measurement and gate:
  125.2 + 125.3 + 125.6 + 125.9 -> 125.10 -> maintainer review
```

125.3 may run in parallel with 125.4. The profiling foundation in 125.4 lands
before either live CPU/upload wiring or GPU integration adds shared types.
125.5, 125.6, 125.8 and 125.9 each edit renderer/frame-path state and therefore
land sequentially with one active editor. No remediation work starts from the
same branch or session as 125.10's findings.

## Measurement-phase subtasks

### 125.1 — Add reproducible measurement tools to the default dev shell

Scope: `flake.nix` only.

What: add Linux-only `pkgs.wtype` and `pkgs.amdgpu_top` to
`devOnlyTools`. `wtype` drives deterministic keyboard input under Wayland;
`amdgpu_top --json --process --no-pc` supplies AMD fdinfo/process samples
without enabling GRBM performance-counter polling that can itself change GPU
power behavior. `perf` already exists and must not be duplicated.

Deliverable: both tools declared in the default shell, with no tool added to
the `ci` or `gl-pixel` shells.

Verification: `nixfmt flake.nix`; `statix check`; `deadnix --fail`; `nix eval
.#devShells.x86_64-linux.default.drvPath`.

Prohibitions: do not install either tool out of band; do not add GPU tools for
hardware not present on the authority machine; do not edit Rust or scripts.

Stop: per `flake-dev-shell-discipline`, stop after the flake edit and ask the
maintainer to run `nix develop` or reload direnv. Do not proceed to 125.2 until
both commands are confirmed on `PATH`.

### 125.2 — Matched parity fixtures, workload driver, and capture preflight

Scope: new files under `assets/profiling/task125/` only:
`freminal.toml`, `wezterm.lua`, `ghostty.conf`, `btop.conf`, `shell.rc`,
`workloads.sh`, `run-matrix.sh`, and `summarize.py`; plus
`Documents/PROFILING.md`.

What: encode the activation decisions above as isolated competitor configs and
one driver. The driver records binary/store versions, Freminal commit, CPU/GPU,
kernel, compositor, display refresh, terminal grid, and renderer string before
each run; rejects software renderers; performs a 10-second warm-up; captures
seven interleaved 60-second samples; and writes machine-readable raw output
outside the repository. `perf stat` must collect `task-clock`,
`task-clock:u`, `task-clock:k`, `cycles`, `instructions`,
`context-switches`, and exact wakeups. This host already has
`sched:sched_wakeup`; tracefs is mounted `root:root` mode `0700`, so the driver
must run only the system-wide, PID-filtered scheduler tracepoint collector via
`sudo` after an explicit `sudo -v` preflight. Do not remount tracefs or weaken
its permissions. Count `sched:sched_wakeup` events whose target PID/TID belongs
to the measured terminal process tree, and keep the ordinary per-process
`perf stat` counters unprivileged. In parallel, use
`amdgpu_top --json --process --no-pc` to identify the discrete Navi 31 device
and the measured process's DRM clients, then difference cumulative
`drm-engine-gfx` time from those clients' `/proc/<pid>/fdinfo/*` records over
the same steady interval. Record a monitor-only control to quantify collector
overhead. `summarize.py` performs the deterministic bootstrap rule above with
seed 125 and 10,000 resamples.

The exact workload matrix is:

1. visible blinking-cursor idle;
2. visible steady-cursor idle;
3. scripted typing at a clean prompt, using `wtype` to type and erase a fixed
   ASCII payload without executing it;
4. sparse one-row PTY updates using a fixed carriage-return/erase-line script
   at 20 updates/s;
5. hidden-cursor `btop` with the repository fixture config and 1,000 ms update
   interval;
6. scrollback scrolling after preloading 10,000 numbered lines, alternating
   PageUp/PageDown through `wtype` at a fixed cadence;
7. sustained PTY output using the Task-124 control
   `while :; do seq 1 200; sleep 0.02; done`;
8. physical pointer motion over inert terminal content, timed for 60 seconds
   and reported with Freminal's observed event rate; and
9. four-tab/2x2-pane idle chrome, once with the active cursor blinking and once
   steady.

Deliverable: hermetic configs, runnable scripts, deterministic summary output,
and an updated `PROFILING.md` command/reference section. A dry run must prove
all three terminals resolve to 120x40 cells and the intended discrete GPU;
that wakeups are counted for the complete terminal process tree; and that each
terminal exposes cumulative GFX engine time through DRM fdinfo. If fdinfo is
absent or remains below its measurable resolution under the active control,
the preflight records cross-terminal GPU parity as unavailable rather than
substituting device-wide utilization.

Verification: run `bash -n` on `workloads.sh` and `run-matrix.sh`; run
`python -m py_compile
assets/profiling/task125/summarize.py`; run each driver's preflight and one
10-second smoke sample per terminal; `cargo test --all`; `cargo clippy
--all-targets --all-features -- -D warnings`; `cargo machete`; markdownlint
on `Documents/PROFILING.md`.

Prohibitions: do not read the user's normal terminal or `btop` config; do not
compare unlike cursor defaults; do not treat `btop`'s CPU display as a metric;
do not treat llvmpipe as performance evidence; do not silently substitute a
wakeup proxy; do not remount or chmod tracefs; do not attribute device-wide GPU
utilization to one terminal process; do not commit raw machine captures.

Stop: report preflight/smoke results and any host prerequisite. Do not start the
seven-repeat matrix yet.

### 125.3 — Repair the incremental vertex-construction benchmarks

Scope: `freminal/benches/render_loop_bench.rs` and
`.opencode/skills/freminal-bench-table/SKILL.md` only.

What: keep the existing `instanced_bg_partial_dirty` and
`instanced_fg_partial_dirty` group IDs, but make their timed regions isolate
CPU vertex construction. Prepopulate one persistent foreground atlas before
timing both all-row and one-row cases; add a separate
`instanced_fg_atlas_rasterization` group for cold-atlas rasterisation. Expand
the background group with three named corpora at 200x50: all-default
background, 10% sparse colored backgrounds, and 100% dense colored
backgrounds. For each corpus measure all 50 rows and one middle row with
preallocated output vectors. Preserve the existing all-row/one-row benchmark
names only where their meaning remains accurate; suffix new corpus cases
explicitly. These remain headroom measurements, not a claim that an
incremental production builder exists.

Deliverable: steady-state foreground construction, separately measured atlas
rasterisation, background padding-cost corpora, and corrected benchmark-catalog
descriptions.

Verification: capture a Criterion baseline before editing; run
`cargo bench --bench render_loop_bench instanced_bg_partial_dirty -- --baseline
before_125_3`, `cargo bench --bench render_loop_bench
instanced_fg_partial_dirty -- --baseline before_125_3`, and the new atlas
group; `cargo bench --no-run --all`; `cargo test --all`;
`cargo clippy --all-targets --all-features -- -D warnings`; `cargo machete`.

Prohibitions: do not implement incremental production vertices; do not include
fresh atlas allocation/rasterisation in the steady-state foreground timed
region; do not use these synthetic numbers as live upload evidence.

Stop: report before/after tables and measured one-row/all-row ceilings; await
review before 125.10 consumes them.

### 125.4 — Feature-gated live render-work profiling foundation

Scope: new `freminal/src/gui/renderer/profiling.rs`;
`freminal/src/gui/renderer/mod.rs`; and `freminal/Cargo.toml` only.

What: add the cohesive profiling types used by 125.5, 125.6 and 125.8 behind
the existing `frame-profiling` feature: `RenderWorkClass::{Reuse, CursorOnly,
Bounded, Full}`, `ChangedRowBucket::{Zero, One, TwoToFour, FiveToEight,
NineToSixteen, SeventeenToThirtyTwo, ThirtyThreeToSixtyFour, MoreThanSixtyFour}`,
`UploadByteCounts`, and `LiveRenderProfile`. `LiveRenderProfile` owns a
monotonic pane-frame token, a bounded queue of per-frame records, cumulative
class/histogram/upload totals, and the 120-observation flush cadence. Starting
a new token finalizes an older unpainted token as zero-upload; a later paint
callback may finalize only its matching token. This explicitly handles
`FrameDamage::None`, where egui never invokes the registered callback.

Deliverable: pure state transitions and exhaustive unit tests for painted,
unpainted, late, duplicate, and out-of-order token completion. Default builds
contain none of these fields or increments.

Verification: `cargo test --all --all-features`; `cargo test --all`;
`cargo clippy --all-targets --all-features -- -D warnings`; `cargo machete`.

Prohibitions: do not wire renderer/widget call sites; do not add fields to
`FrameStats` or `FrameProfile`; do not transport profiling state across the
PTY/snapshot boundary; do not alter damage decisions.

Stop: report the tested state-machine API; await review before 125.5.

### 125.5 — Live rebuild outcomes and changed-row histogram

Scope: `freminal/src/gui/renderer/profiling.rs`,
`freminal/src/gui/terminal/frame_dirty.rs`, and
`freminal/src/gui/terminal/widget.rs` only.

What: record both the raw `VertexRebuild` decision and its resolved work:
`CursorOnly`; `Bounded` with the `ChangedRows::Rows` count bucket;
`ReevaluateFullRebuild` resolving to `Full`; and
`ReevaluateFullRebuild` resolving to `Reuse`. Begin one profile token per pane
`show()` call and capture that token in the pane's paint callback. Log raw and
resolved counts, changed-row histogram, and pane id every 120 observations.
The profiling reads the already-computed `dirty.rebuild`, `changed_rows`, and
`full_rebuild`; it must not recalculate or influence them.

Deliverable: live counts that distinguish reuse, cursor-only, sparse bounded,
dense bounded and full rebuilds, with tests pinning every mapping and proving
feature-disabled builds retain no profiling branch.

Verification: targeted renderer/frame-dirty/widget tests with and without
`frame-profiling`; `cargo test --all`; `cargo clippy --all-targets
--all-features -- -D warnings`; `cargo machete`.

Prohibitions: do not change `VertexRebuild`, `ChangedRows`,
`PaneFrameDamage`, or `FrameDamage` semantics; do not call the profiler from
the PTY thread; do not infer upload bytes yet.

Stop: report a synthetic log covering all resolved classes; await review
before 125.6.

### 125.6 — Actual live per-buffer upload-byte attribution

Scope: `freminal/src/gui/renderer/profiling.rs`,
`freminal/src/gui/renderer/gpu.rs`,
`freminal/src/gui/terminal/widget.rs`, and
`freminal/src/gui/atlas.rs` only.

What: count bytes at the actual GL upload call sites, not from synthetic
buffer lengths elsewhere. Attribute background-instance VBO, foreground-
instance VBO, decoration VBO, image-vertex VBO, image textures, full atlas
uploads, and atlas sub-rectangle uploads separately. The pane paint callback
finalizes its 125.5 token with exactly the uploads issued for that callback;
an uninvoked callback remains zero-upload. Log totals and per-class byte
distributions, including zero-byte reuse/`None` observations. Counter updates
remain feature-gated and observe existing slice/rectangle sizes only.

Deliverable: live upload-byte totals paired with resolved render-work class,
plus recording-facade and unit tests proving every upload category counts the
exact payload once and orphaning's zero-byte allocation call is not double-
counted as transferred bytes.

Verification: `cargo test --all --all-features`; Task 123 recording workload
tests; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo
machete`; `cargo xtask check-windows`.

Prohibitions: do not change upload offsets, orphaning policy, VBO layout,
damage, or draw order; do not count `buffer_data_size` allocation bytes as a
second transfer; do not include toast buffers in terminal-buffer categories.

Stop: report synthetic exact-byte checks and one live smoke log; await review.

### 125.7 — Asynchronous GPU timestamp-query foundation

Scope: new `freminal-windowing/src/gpu_profiling.rs`;
`freminal-windowing/src/lib.rs`; `freminal-windowing/Cargo.toml`; and
`freminal/Cargo.toml` only.

What: add a separate `gpu-profiling` feature and a reusable, bounded
`GpuQueryRing<Q>` lifecycle state machine. A query sample contains start/end
timestamp handles, issue frame, and a named phase supplied by its owner.
Polling is forbidden on the issue frame; later frames first test
`QUERY_RESULT_AVAILABLE` and read results only when ready. If the bounded ring
is full, drop the new sample and count the drop rather than block. Capability
is available only for desktop OpenGL 3.3+ or `GL_ARB_timer_query`; GLES and
unsupported contexts report `Unavailable`. Query destruction is explicit.

Deliverable: the pure ring/state machine with fake-handle tests for delayed
availability, wraparound, dropped samples, unsupported capability, and cleanup;
the `freminal` feature forwards to `freminal-windowing/gpu-profiling`.

Verification: `cargo test --all --all-features`; `cargo test --all`;
`cargo clippy --all-targets --all-features -- -D warnings`; `cargo machete`;
`cargo xtask check-windows`.

Prohibitions: do not integrate any paint path; do not call `glFinish`,
`glFlush`, or blocking `QUERY_RESULT`; do not claim llvmpipe timing as
performance evidence; do not add a second renderer path.

Stop: report the lifecycle API and zero same-frame-read tests; await review.

### 125.8 — Terminal upload and draw GPU timing

Scope: new `freminal/src/gui/renderer/gpu_profiling.rs`;
`freminal/src/gui/renderer/mod.rs`;
`freminal/src/gui/renderer/gl_facade/{facade.rs,recording.rs,recording_tests.rs,surface.rs}`;
`freminal/src/gui/renderer/gpu.rs`;
`freminal/src/gui/terminal/widget.rs`;
`Documents/PLAN_123_GL_MEASUREMENT_HARNESS.md`; and this document.

What: extend the GL facade with query creation/deletion, timestamp issue,
availability, and 64-bit result reads; update the frozen call surface and its
recording tests. Build a renderer-side adapter over 125.7's ring. Issue
non-overlapping timestamp pairs around actual terminal upload commands and
terminal draw commands, aggregate all panes, and poll only on later callbacks.
Report upload GPU time, terminal draw GPU time excluding upload, unavailable/
dropped sample counts, pane id, renderer string, and query latency in frames.

Deliverable: asynchronous terminal GPU attribution on real GL, deterministic
recording behavior for the expanded facade, and corrected Task-123 references
to the former 49-method surface.

Verification: `cargo test --all --all-features`; Task 123 recording tests;
`cargo clippy --all-targets --all-features -- -D warnings`; `cargo machete`;
`cargo xtask check-windows`; one llvmpipe/offscreen correctness smoke that
claims no performance result; one real-AMD smoke proving results arrive on a
later frame with no same-frame query read; markdownlint on both plan documents.

Prohibitions: do not alter production call order or payloads; do not nest
`TIME_ELAPSED` queries; do not read a result until availability succeeds; do
not weaken the raw-GL guard to hide new renderer calls.

Stop: report query latency/drop counts and real-GPU smoke timing; await review.

### 125.9 — Chrome and total-frame GPU timing

Scope: `freminal-windowing/src/gpu_profiling.rs`,
`freminal-windowing/src/frame_paint.rs`,
`freminal-windowing/src/egui_integration.rs`, and
`Documents/PROFILING.md` only.

What: use asynchronous timestamp pairs around head chrome paint, terminal-band
paint, tail chrome paint, and the total clear/textures/paint interval. Poll on
later frames through 125.7's ring and log chrome draw (head + tail), terminal
band, total GPU work, query latency, dropped samples, capability, window id,
and renderer. Keep terminal renderer upload/draw timings from 125.8 separate;
the band value is a cross-check, not a subtraction-based substitute. A
`FrameDamage::None` frame issues no GPU sample because it submits no GPU work.

Deliverable: real-hardware asynchronous chrome/terminal/total GPU attribution
documented in `PROFILING.md`, with phase-boundary tests and an unsupported-
context path that changes no rendering behavior.

Verification: `cargo test --all --all-features`; frame-paint harness; Task 123
pixel harness for unchanged pixels; `cargo clippy --all-targets --all-features
-- -D warnings`; `cargo machete`; `cargo xtask check-windows`; real-AMD smoke
with no same-frame read or query-ring drops at the protocol workload rate;
markdownlint on `Documents/PROFILING.md`.

Prohibitions: do not move, merge, or reorder head/band/tail painting; do not
time swap/compositor latency as GPU execution; do not make unsupported query
capability fatal to normal rendering; do not use llvmpipe numbers in findings.

Stop: report phase timings and capability; await review before 125.10.

### 125.10 — Execute the parity matrix and close the remediation gate

Scope: `Documents/PLAN_125_VERTEX_RELAYOUT.md` only. Raw captures remain
outside the repository.

What: run all workloads from 125.2 against the three pinned binaries, seven
60-second steady samples each after warm-up, with terminal order interleaved.
For Freminal run both `frame-profiling` and `gpu-profiling`; verify the active
renderer is the discrete Navi 31 and `LIBGL_ALWAYS_SOFTWARE` is unset. Record
external task-clock, user/kernel time, cycles, instructions, context switches,
wakeups, external per-process GPU samples, frame rate plus CPU cost/frame,
live render-work outcomes, changed-row histogram, upload bytes by buffer and
work class, and asynchronous GPU upload/terminal/chrome/total timings.

Append a Findings section with raw-version/environment identity, per-workload
median and confidence interval tables, and a `CONFIRMED`, `REFUTED`, or
`INCONCLUSIVE` verdict for every candidate below. For every credible
remediation report measured ceiling, expected benefit, complexity, correctness
risk, portability cost, and required verification. Then stop for maintainer
selection; remediation subtasks are written in a later activation session.

Deliverable: the complete matched parity matrix and explicit gates for:

- idle/chrome bypass or cursor-blink decoupling;
- retained chrome output that preserves current-frame hit testing;
- fixed-stride per-row uploads;
- incremental CPU row vertex construction;
- mapped/persistent GPU buffers with capability fallback;
- scheduling and unnecessary-wakeup elimination;
- presentation/compositor changes; and
- accepting an explained residual.

Verification: the runner's preflight and statistical checks; all metric totals
reconcile to their class/frame counts; frame rate and per-frame cost are paired;
the full repository suite (`cargo fmt --all -- --check`, `cargo test --all`,
`cargo clippy --all-targets --all-features -- -D warnings`, `cargo machete`,
`cargo bench --no-run --all`, `cargo xtask check-windows`); markdownlint on
this document.

Prohibitions: do not implement or decompose a remediation; do not average away
terminal/order effects; do not report rounded `btop` CPU as evidence; do not
claim cross-terminal GPU parity if external process counters are unavailable;
do not use difficulty or low expected ROI to omit a credible hard option; do
not accept a residual that is unmeasured or unexplained.

Stop: present findings and remaining maintainer choices. Await explicit review
before any remediation activation.

## Findings gates

- **Idle/chrome:** open when blinking-cursor idle has a material paired CPU
  gap and Freminal's frame/UI/presentation measurements account for it. Steady
  idle is the control and must remain no-frame/no-wake after settling.
- **Fixed stride:** open only when live sparse `Bounded` frames are frequent
  enough that measured saved upload bytes exceed dense/full padding bytes at
  the observed class distribution, and the predicted CPU or GPU saving reaches
  the material floor. Preserve `DefaultBackground` no-fragment semantics and
  the cursor-last decoration invariant.
- **Incremental CPU construction:** open when repaired row benchmarks and live
  changed-row histograms predict a material task-clock saving independent of
  upload bandwidth. It may open even if fixed stride does not.
- **Persistent buffers:** open when asynchronous timing attributes material
  time to upload/driver synchronization after accounting for byte volume, and
  supported platforms have a safe capability fallback. Difficulty and
  portability cost affect the decision, not whether the option is reported.
- **Scheduling:** open when Freminal's wakeup or frame rate exceeds the slower
  peer materially while cost per frame is already comparable. The proposed
  lever must name the wake source; a generic longer timeout is not a fix.
- **Presentation:** open when user/kernel time, swap wall time, external GPU
  data, and internal GPU completion show a material gap after draw work is
  accounted for, and a real platform API lever exists.
- **Accept residual:** allowed only when every material gap is either shared by
  the slower peer, attributed to an unavoidable platform difference, or below
  every safe remediation's measured ceiling. Every smaller statistically
  separable residual remains documented even when it does not force code.

---

## Verification for the measurement phase

Standard, per `agents.md`:

1. `cargo test --all`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo machete`
4. `cargo xtask check-windows` before any PR

Additionally mandatory for this task specifically:

- **The Task 123 Phase 2 pixel harness on every subtask that changes
  emission.** The untouched-background property in decision 2 is invisible to
  call-count tests, and the failure mode is silent visual corruption — the
  issue #432 class.
- A before/after capture per `performance-benchmarks` and
  `freminal-bench-table`, reported in **bytes** as well as calls, per Task
  123's correction to the cost model.
- Matched external process-level captures for Freminal, WezTerm, and Ghostty
  on the authority machine for every remediation claiming parity benefit.
- Real-GPU timing on supported hardware for changes justified by GPU or driver
  cost. llvmpipe remains a correctness harness and must not be reported as
  hardware performance evidence.

---

## References

- `Documents/PLAN_124_RENDER_EFFICIENCY.md` — the completed damage-model task
  that produces the per-row dirty signal this task can consume.
- `Documents/PLAN_123_GL_MEASUREMENT_HARNESS.md` — the measurement harnesses
  and the per-workload cost table quoted throughout.
- `Documents/PLAN_121_PERF_REMEDIATION.md` — closed; the source of the
  measure-before-fixing discipline this task's gate enforces.
- `Documents/PROFILING.md` — profiling methodology.
- Issue #432 — the silent visual corruption bug class this task shares.
- Issue #435 — partial present, the mechanism a per-row upload complements.
- Issue #440 — the missing pixel harness, closed by Task 123 Phase 2.
