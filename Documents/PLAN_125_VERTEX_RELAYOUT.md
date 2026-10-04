# PLAN_125_VERTEX_RELAYOUT.md — Task 125 "Performance Parity and Residual Remediation"

> **STATUS: IN PROGRESS — measurement phase complete (125.1–125.10);
> remediation phase (125.11–125.18) activated 2026-10-04.** The measurement phase is
> decomposed below against the post-Task-124 codebase. No remediation is
> selected or decomposed yet. Fixed-stride relayout remains
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
  `0-unstable-2026-09-17` at
  `/nix/store/mmvpz8sgpp4gg1lwsjv68knvkzqmsdwk-wezterm-0-unstable-2026-09-17/bin/wezterm`
  and Ghostty `1.3.1` at
  `/nix/store/i5zqr903i6yb642h9amwh5174n5bmfc4-ghostty-1.3.1/bin/ghostty`.
  Record the Freminal commit and active GL renderer with every capture.
  Re-pinned by maintainer decision on 2026-10-04, before any screening
  capture: the originally pinned WezTerm `0-unstable-2026-08-12`
  (`fjd3yyncgw5w…`) and Ghostty `1.3.1` (`ij9fvnhfj710…`) store paths had been
  garbage-collected after a system update.
- **CPU parity:** use a staged protocol. Screen every non-pointer workload with
  three 20-second steady samples after a 5-second warm-up, interleaving terminal
  order; run pointer screening separately with explicit maintainer interaction.
  Then run seven 60-second samples after a 10-second warm-up only for workloads
  whose screen shows a meaningful/noisy gap or whose result controls a
  remediation gate. A confirmation gap is material only when the deterministic
  10,000-resample bootstrap 95% confidence interval for the median paired
  task-clock delta excludes zero **and** the median delta is at least 0.5 ms
  task-clock per wall-second (0.05% of one core). Smaller statistically
  separable residuals must still be explained.
- **GPU parity:** use cumulative per-process AMD DRM fdinfo engine time for
  matched cross-terminal comparison, with `amdgpu_top` for device/process
  discovery and asynchronous OpenGL timestamp queries inside Freminal for
  attribution. If the fdinfo fields are absent or too coarse to distinguish
  the workloads, the cross-terminal GPU verdict is `INCONCLUSIVE`, never
  inferred from llvmpipe or CPU time.
- **Controlled geometry/config:** the same stable Hyprland tiled allocation,
  with each actual PTY grid recorded, CaskaydiaCove Nerd Font at 12 pt, opaque
  background, no background image, no user shader, no cursor trail, the same
  clean interactive shell/prompt, and isolated config/state directories.
  Backend integer-pixel font metrics prevent an exact common grid without
  unequal font sizes; synthetic workloads stay within the common 124x31 region,
  while `btop` is explicitly product-level. Ligatures stay enabled for all
  three terminals.
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
each run; rejects software renderers; supports the three-repeat/20-second
screening pass and selective seven-repeat/60-second confirmation pass described
above; and writes machine-readable raw output outside the repository. It must
print the expected window count/duration and interaction requirements before
spawning anything. `perf stat` must collect `task-clock`,
`task-clock:u`, `task-clock:k`, `cycles`, `instructions`,
`context-switches`, and exact wakeups. This host already has
`sched:sched_wakeup`; tracefs is mounted `root:root` mode `0700`, so the driver
must run only the system-wide, terminal-thread-filtered scheduler tracepoint collector via
`sudo` after an explicit `sudo -v` preflight. Do not remount tracefs or weaken
its permissions. Count `sched:sched_wakeup` events whose target TID belongs
to the mapped terminal GUI process, and keep the ordinary per-process
`perf stat` counters unprivileged. In parallel, use
`amdgpu_top --json --process --no-pc` to identify the discrete Navi 31 device
and the measured GUI process's DRM clients, then difference cumulative
`drm-engine-gfx` time from that process's `/proc/<pid>/fdinfo/*` records over
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
8. physical pointer motion over inert terminal content, timed for 20 seconds in
   screening and 60 seconds in confirmation, run separately from unattended
   workloads and reported with Freminal's observed event rate;
9. four-tab/2x2-pane idle chrome, once with the active cursor blinking and once
   steady; and
10. scrolling streaming output (added by 125.C4): one new line every 20 ms, so
    each line scrolls the view by exactly one row. The typed command is:

    ```sh
    n=0; while :; do printf 'Task125 stream line %08d\n' "$((++n))"; sleep 0.02; done
    ```

Deliverable: hermetic configs, runnable scripts, deterministic summary output,
and an updated `PROFILING.md` command/reference section. A dry run must prove
all three terminals report valid grids in the same tiled bounds and use the
intended discrete GPU;
that wakeups are counted for all terminal GUI threads, excluding workload
descendants; and that each
terminal exposes cumulative GFX engine time through DRM fdinfo. If fdinfo is
absent or remains below its measurable resolution under the active control,
the preflight records cross-terminal GPU parity as unavailable rather than
substituting device-wide utilization.

Verification: run `bash -n` on `workloads.sh` and `run-matrix.sh`; run
`python3 -m py_compile
assets/profiling/task125/summarize.py`; run each driver's preflight and one
10-second smoke sample per terminal; `cargo test --all`; `cargo clippy
--all-targets --all-features -- -D warnings`; `cargo machete`; markdownlint
on `Documents/PROFILING.md`.

Prohibitions: do not read the user's normal terminal or `btop` config; do not
compare unlike cursor defaults; do not treat `btop`'s CPU display as a metric;
do not treat llvmpipe as performance evidence; do not silently substitute a
wakeup proxy; do not remount or chmod tracefs; do not attribute device-wide GPU
utilization to one terminal process; do not commit raw machine captures.

Stop: report preflight/smoke results and any host prerequisite. Do not start
screening or confirmation yet.

**Complete.** The fixtures use the NixOS system `bash-interactive` under an
isolated HOME/XDG environment, pre-seed only Freminal's onboarding-complete
state, and track mapped GUI and separately-reparented shell PIDs by PID plus
start time. No pattern-based process cleanup remains. Hyprland's stable tiled
slot is the geometry control; observed smoke grids were Freminal 124x31,
WezTerm 138x31, and Ghostty 140x33. Exact common-grid calibration was rejected
because Ghostty's integer-pixel steps skip the target and unequal font sizes
would be a worse confound. Synthetic workloads stay within 124x31; `btop` is
product-level.

The staged protocol replaces the original four-hour exhaustive matrix: a
roughly 34-minute non-pointer screen (roughly 38 minutes after 125.C4 added a
tenth workload), a separate roughly four-minute physical-pointer screen, then
seven 60-second confirmations only for selected workloads.
Scheduler wakeups use the existing root-only `sched:sched_wakeup` tracepoint
with a terminal-thread filter; workload descendants are excluded from terminal
CPU accounting. Per-process AMD DRM fdinfo was available for all three smoke
runs. Ten-second collector controls recorded complete perf, wakeup, CPU tick,
GPU engine-time, and grid rows for Freminal, WezTerm, and Ghostty. These smoke
numbers validate plumbing only and are not parity findings.

### 125.C1 — Capture isolated feature-gated profiling logs

Scope: `assets/profiling/task125/run-matrix.sh` and `Documents/PROFILING.md`.

Surface point: discovered during the 125.6 live smoke after commit `06d1118f`.
The fixture's `env -i` launch discarded `RUST_LOG`, while its config fixed file
logging at `info`, so the required `debug`-level Task-125 profile summaries
could neither be emitted nor associated with an individual raw sample.

What: pass a narrow Freminal-only `RUST_LOG` filter inside the isolated launch,
redirect Freminal stdout/stderr into each external sample directory, and add a
one-window sustained-output smoke that forces a 120-observation summary.
WezTerm and Ghostty retain their existing clean environments. Verify with
ShellCheck and the smoke's checks for the real AMD renderer plus at least one
Task-125 live-render summary. No raw capture is committed.

**Complete.** The runner now captures Task 121 frame summaries and Task 125
live-render summaries in each Freminal sample's external raw directory without
enabling Rust logging for either peer terminal. `profile-smoke` uses the
existing sustained-output fixture and validates renderer and summary identity.

### 125.C2 — TerminalRenderer GL resources are never destroyed

Scope: `freminal/src/gui/renderer/gpu.rs` (`TerminalRenderer::destroy`) and
wherever a pane's `RenderState` is dropped across the GUI binary; the exact
fix scope is not yet determined and must be investigated, not assumed.

Surface point: found during the 125.8 review, pre-existing and predating
Task 125.

What: `TerminalRenderer::destroy` has no production caller -- a repo-wide
search finds no `.destroy(` call reaching it; the only production
`.destroy()` call in the GL-facing code is `egui_integration.rs`'s own
painter teardown. Closing a pane, tab, or window therefore never deletes
that pane's programs, VAOs, VBOs, atlas/image textures, or (under
`gpu-profiling`) pending GPU timer queries; every one of those GL objects is
reclaimed only when the whole GL context tears down. `toast_pass` and
`toast_text_pass` may share the same gap and must be checked, not assumed
clear.

Impact: a GL object leak proportional to the number of panes opened over a
session. It also makes 125.8's `PaneGpuTimingProfile::shutdown` path
unreachable in a live run -- the cleanup it performs (destroying pending
query handles before GL teardown) currently only runs in this module's own
tests, never in production.

Scope of fix: wherever a pane's `RenderState` is dropped needs a
GL-context-current destroy path scheduled for it. `Drop` itself has no GL
context available, so this likely needs a deferred-destroy queue drained
from inside a paint callback rather than a direct call from `Drop`. Whether
an existing teardown pattern can be reused is unverified.

Scheduling: not part of the Task 125 measurement phase; must not block
125.9 or 125.10. Status: open, not complete.

### 125.C3 — Capture the 125.8 GPU timing log target

Scope: `assets/profiling/task125/run-matrix.sh` and
`Documents/PROFILING.md`.

Surface point: found during 125.8 smoke preparation, following the same
class of gap 125.C1 fixed for the task 125.5/125.6 summary.

What: the runner's `TASK125_FREMINAL_RUST_LOG` filter starts from `none`
and allowlists only specific targets; it omitted
`freminal::task_125::gpu_timing`, so the task 125.8 GPU timing flush this
subtask's log line is emitted under could never reach `freminal.stdout.log`
even with `gpu-profiling` enabled. `profile-smoke` also did not assert the
flush's presence, so the gap was silent. Fixed by adding
`freminal::task_125::gpu_timing=debug` to the filter, adding a third
`profile-smoke` assertion for the literal text `Task 125.8 terminal GPU
timing flush`, and noting in both the preflight failure message and
`profile-smoke`'s own pre-run notice that `FREMINAL_BIN` must be built with
`--features frame-profiling,gpu-profiling` for these checks to pass.

**Complete.** The 125.8 real-AMD `profile-smoke` run on 2026-10-04 validated
all three assertions (renderer identity, 125.5/125.6 summary, 125.8 GPU
timing flush).

### 125.C4 — Add a scrolling streaming-output workload

Scope: `assets/profiling/task125/workloads.sh`,
`assets/profiling/task125/run-matrix.sh`, `Documents/PROFILING.md`, and this
document.

Surface point: maintainer review of the 125.8 smoke on 2026-10-04 observed that
`sustained-output` prints each 200-line burst at once, so every burst replaces
the whole 31-row view.

What: workload 7 (`sustained-output`) is a dense full-redraw throughput test,
and workload 4 (`sparse-row`) updates one row in place without scrolling.
Neither covers the most common real-world pattern: one new line at a time
scrolling the view by one row (`tail -f`, `cargo build`, `journalctl -f`).
A one-line scroll changes the content of every visible row although only one
line is new, so this workload tests whether Freminal's damage model treats a
one-line scroll as all-rows-changed -- an expected but unverified hypothesis;
measurement decides -- and gates a possible scroll-aware row-reuse remediation
(shifting or reusing existing row vertex data, or a GPU region copy, instead of
a full rebuild). The change adds a `streaming-output` workload to
`workloads.sh` (one fixed-width ASCII line every 20 ms with a monotonically
increasing counter, narrower than the common 124-column grid), classifies it in
`run-matrix.sh` as a non-pointer unattended workload started before warm-up
beside `sustained-output`, includes it in the non-pointer `screen`, and
documents it and the new screen duration (90 windows, roughly 38 minutes) in
`PROFILING.md`. No raw capture is committed.

**Complete.** A 15-second real-AMD smoke on 2026-10-04 ran the exact command
directly in an isolated, onboarding-seeded Freminal with `frame-profiling` and
`gpu-profiling`. It rendered cleanly and produced live profile and GPU timing
flushes. Smoke indication only, not a finding: of 840 observations, all 317
`Bounded` frames fell in the 17-32 changed-row bucket and none in the one-row
bucket, and 435 resolved `Full`, consistent with the all-rows-changed
hypothesis. 125.10 decides it under the matched protocol.

### 125.C5 — Allow thread exits during a capture

Scope: `assets/profiling/task125/run-matrix.sh`.

Surface point: the first 125.10 screen on 2026-10-04 aborted on Ghostty's first
sample: 29 startup worker threads exited during capture and none were created,
and the runner rejected any thread-set change.

What: exited threads were in the wakeup filter from the start and their CPU
time folds into the process, so exits no longer invalidate a sample; the exit
count is written to `exited-tids`. A thread created during capture would be
missing from the filter and remains a hard failure.

**Complete.** ShellCheck clean; validated by the restarted screen.

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

**Complete.** The steady-state foreground benchmark now reuses a prewarmed
atlas and measures 151.90 us for all 50 rows against 3.0308 us for one middle
row, a 50.1x construction ceiling. The separated cold-atlas group measures
977.31 us for the same full-screen corpus. Background construction measures
134.86 ns / 14.239 ns for all-default, 6.6333 us / 141.26 ns for 10%-sparse,
and 18.239 us / 361.78 ns for dense all-row / one-row cases. The preserved
all-default baselines changed by +0.25% and +1.46%, within Criterion's noise
threshold; the foreground changes of -83.13% and -99.59% are the intended
removal of cold-atlas work rather than production speedups.

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

**Complete.** Added the feature-gated `LiveRenderProfile` state machine with
monotonic pane-frame tokens, a bounded 120-record queue, cumulative resolved-
class, changed-row-bucket, and upload-byte totals, and the shared 120-
observation flush cadence. Tests cover painted and superseded-unpainted frames
plus late, duplicate, out-of-order, queue-eviction, and bucket-boundary cases.
The module remains entirely absent from default-feature builds and has no live
renderer or widget wiring yet.

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

**Complete.** The raw `VertexRebuild` decision and resolved reuse, cursor-only,
bounded, or full class are captured once per renderable pane `show()` and
finalized by the matching paint callback. Pending observations retain their
classification, so a callback suppressed by `FrameDamage::None` is finalized
on the next token as zero-upload while still contributing to raw/resolved
counts and the changed-row histogram. A one-shot flush signal preserves every
120-observation boundary whether completion occurs in `start()` or the paint
callback. Feature-gated tests cover every mapping, bounded zero-row handling,
painted/unpainted token flow, and the superseded-token flush boundary.

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

**Complete.** Actual GL upload sites now return only transferred payload bytes,
excluding orphan-allocation calls, and the pane callback attributes background,
foreground, decoration, image-vertex, image-texture, full-atlas, and atlas-
subrectangle bytes to its matching resolved work class. Recording-facade tests
pin every category, skip/error paths, cursor-only behavior, and the orphaning
double-count trap. The 125.C1 real-AMD sustained-output smoke produced periodic
reconcilable logs; at 840 observations it reported 7,438,540 total bytes:
3,183,596 foreground, 51,408 decoration, 4,194,304 full-atlas, and 9,232 atlas-
subrectangle bytes, with all other categories zero for that corpus.

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

**Complete.** Added the feature-gated, GL-independent `GpuQueryRing<Q>` with
pure desktop/GLES capability detection, bounded asynchronous issue/poll
transitions, delayed availability checks, explicit pending-handle destruction,
and saturating drop accounting. Same-frame polls never consult the query
source, results are read only after end-query availability succeeds, and a
full-ring rejection returns both query handles to the caller for destruction.
Twenty-two fake-handle tests cover capability parsing, delayed completion,
slot reuse, drops, rejected ownership, and cleanup; default and Windows builds
remain clean.

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

**Complete.** The GL facade grew from 49 to 56 methods (query create/delete,
`query_counter` with `GL_TIMESTAMP`, availability and 64-bit result reads,
and `GL_VERSION`/extension capability probes), with recording-arm support
and tests, and `PLAN_123` updated to the 56-entry surface.
`PaneGpuTimingProfile` (`freminal/src/gui/renderer/gpu_profiling.rs`) adapts
125.7's ring per pane: timestamp pairs bracket the upload commands and the
draw commands in both `draw_with_verts` and `draw_with_cursor_only_update`,
with the upload-end and draw-start timestamps issued back to back at the
boundary; no `TIME_ELAPSED`, no same-frame or blocking read. Capability is
detected once per GL-resource lifetime in `TerminalRenderer::init`; under
`gpu-profiling` that adds one `GL_VERSION` read, so
`headless_workloads::init_dominates_a_single_frame` pins 262 calls with the
feature and 261 without. Interpretation recorded: "aggregate all panes" is
met by profiling every pane uniformly under one log target with `pane_id`
attached, aggregated post hoc, because 125.7's phase type cannot carry a
pane id. Query latency is counted in that pane's own drawn frames, not
window frames, and must not be compared across panes. Smokes on 2026-10-04
(both release builds with `frame-profiling,gpu-profiling`): real AMD
(Radeon RX 7900 XTX, radeonsi) via `run-matrix.sh profile-smoke`: 12
flushes, 720 upload and 720 draw samples, `last_latency_frames=1`
throughout, zero dropped samples, alongside a 125.5/125.6 summary; llvmpipe
(LLVM 21.1.8) under the `gl-pixel` shell, with the runner's isolated config
and seeded onboarding state: capability `Available`, 15 flushes, latency 1,
zero drops — a correctness check only, no performance claim. `TerminalRenderer::destroy` (and therefore the profile's `shutdown`)
has no production caller; recorded as 125.C2. The runner log-target gap is
125.C3.

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

**Complete.** `freminal-windowing/src/gpu_profiling.rs` gains a GL-free
`FrameGpuTiming<Q>` state machine (ring capacity 64, four samples per painted
frame: `chrome_head`, `terminal_band`, `chrome_tail`, `frame_total`), a
`GpuTimestampSource` trait, a named `FramePhaseBoundary` enum, and a thin
`glow` source; one instance per `EguiState`, capability detected once in
`EguiState::new`, and pending queries destroyed in `destroy_painter`, which has
a real production caller on every window close. `paint_frame_impl` takes a
feature-gated boundary marker; `paint_frame` keeps its signature for the
frame-paint harness. Boundaries are issued only when the frame paints:
`TotalStart` before the clear, `HeadStart` after texture uploads, back-to-back
pairs between head/band and band/tail, `TailEnd`, and `TotalEnd` after the
texture frees, so the swap is untimed and `FrameDamage::None` issues and polls
nothing. No paint, clear, or texture call moved; `run_ui_pass` was extracted
only to keep `paint_frame_impl` under the line limit. Flushes log under
`freminal_windowing::task_125::gpu_timing` every 60 completed total samples;
`run-matrix.sh` allowlists that target and `profile-smoke` asserts it. Scope
was widened by the orchestrator to the runner, the stale `lib.rs` module doc,
and the `gpu-profiling` comment in `freminal-windowing/Cargo.toml`. Fourteen
pure and four offscreen tests cover order, delayed availability, drops,
unavailable capability, shutdown, and painted/None sequences; the windowing
offscreen suite (144) and the Task 123 pixel harness (11) pass unchanged on
llvmpipe as correctness only. The 2026-10-04 real-AMD `profile-smoke` produced
12 flushes each from 125.8 and 125.9 over 720 painted frames, latency 1, zero
drops, capability `Available`; the band interval (1.11 ms total) sits just
above 125.8's terminal upload plus draw (1.01 ms), as a cross-check expects.

### 125.10 — Execute the parity matrix and close the remediation gate

Scope: `Documents/PLAN_125_VERTEX_RELAYOUT.md` only. Raw captures remain
outside the repository.

What: run the three-repeat/20-second screening matrix from 125.2 against the
three pinned binaries, with terminal order interleaved. Run pointer screening
as a separate maintainer-interactive session. Select confirmation workloads
from the screen, record why each was selected or closed, then run seven
60-second samples only for the selected workloads. For Freminal run both
`frame-profiling` and `gpu-profiling`; verify the active
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
- scroll-aware row reuse (shift/reuse existing row vertex data or GPU region
  copy on one-line scrolls);
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
- **Scroll reuse:** open when `streaming-output` shows a material paired CPU
  or GPU gap and the live render-work profile shows one-line scrolls resolving
  to full rebuilds or whole-buffer uploads. Preserve the `DefaultBackground`
  no-fragment and cursor-last decoration invariants.
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

## Findings (125.10, 2026-10-04)

Identity: Freminal `d704366e` release build with `frame-profiling` and
`gpu-profiling`; WezTerm `0-unstable-2026-09-17` and Ghostty `1.3.1` at the
re-pinned store paths; AMD Radeon RX 7900 XTX (radeonsi, navi31); Hyprland
0.56.2; control grids Freminal 124x31, WezTerm 138x31, Ghostty 140x33 (chrome
panes 61-69x14-15). Raw captures are under `/tmp/opencode/t125-screen/` and
`/tmp/opencode/t125-confirm/`, not committed.

### Screen (3 x 20 s, all 90 samples valid)

Medians per wall-second: CPU task-clock ms, AMD fdinfo GFX ms, wakeups.

| Workload         | Freminal           | WezTerm            | Ghostty            |
| ---------------- | ------------------ | ------------------ | ------------------ |
| idle-blink       | 1.1 / 0.35 / 26    | 4.0 / 3.20 / 139   | 1.5 / 0.35 / 33    |
| idle-steady      | 0.1 / 0.00 / 2     | 0.1 / 0.00 / 2     | 1.5 / 0.35 / 32    |
| typing           | 20.2 / 11.0 / 1014 | 30.0 / 16.5 / 1028 | 27.7 / 9.1 / 894   |
| sparse-row       | 10.5 / 5.9 / 430   | 12.2 / 6.7 / 355   | 13.1 / 4.3 / 310   |
| btop             | 2.6 / 0.45 / 35    | 10.5 / 7.0 / 164   | 3.7 / 1.05 / 89    |
| scrollback       | 6.7 / 0.32 / 93    | 34.1 / 4.3 / 215   | 9.4 / 1.85 / 119   |
| sustained-output | 433 / 11.2 / 810   | 19.7 / 13.2 / 649  | 37.5 / 18.7 / 1136 |
| streaming-output | 18.2 / 11.8 / 812  | 27.2 / 15.8 / 639  | 34.5 / 14.3 / 772  |
| chrome-blink     | 1.3 / 0.32 / 26    | 5.3 / 3.4 / 141    | 1.5 / 0.34 / 31    |
| chrome-steady    | 0.1 / 0.00 / 2     | 0.1 / 0.00 / 2     | 1.4 / 0.35 / 32    |

Only `sustained-output` screened material against the slower peer; every other
workload's paired delta was negative (Freminal cheaper) with a confidence
interval excluding zero, so none was selected for confirmation.

### Confirmation: sustained-output (7 x 60 s, all 21 samples valid)

Freminal 428.9 ms/s (425.7 user, 2.8 kernel), IPC 1.14; WezTerm 19.9; Ghostty
37.1. Paired delta against Ghostty +390.4 ms/s, bootstrap 95% CI
[381.8, 405.6]: **material**. GPU time is the lowest of the three (11.3 ms/s).

Attribution: windowing and app frame profiles put the GUI thread at about
180 us per frame (run_ui 72, tessellate 7, paint 32, swap about 65), roughly
11 ms/s at 60 fps. A 15-second `perf record` attributes 97.7% of samples to the
`freminal-pty-consumer` thread and 96.5% of all samples to `memmove`, under
`Vec::drain` of `Row` and `Option<RowCacheEntry>` in
`Buffer::enforce_scrollback_limit` (`freminal-buffer/src/buffer/resize_and_alt.rs`).
Once scrollback is full, every line feed drains the overflow row from the front
of `rows`, `row_cache`, and the row-block map, shifting about 10,000 entries per
line. At about 10,000 lines/s this is the whole gap. It is a buffer
data-structure cost, not rendering.

### Gate verdicts

- **Idle/chrome bypass, cursor-blink decoupling:** REFUTED. Blinking idle and
  four-tab chrome are at or below both peers.
- **Retained chrome output:** REFUTED, same evidence.
- **Fixed-stride per-row uploads:** REFUTED. No workload shows a GPU or upload
  gap; Freminal's GPU time is lowest or within the peer range everywhere.
- **Incremental CPU row construction:** REFUTED. GUI-thread cost is about
  11 ms/s even under sustained output, and typing/sparse/streaming are cheaper
  than both peers.
- **Persistent GPU buffers:** REFUTED, no GPU or driver gap.
- **Scheduling / wakeups:** REFUTED. Wakeups are within the peer range; the
  sparse-row excess (430 vs 355/310) carries lower CPU than both peers.
- **Presentation/compositor:** REFUTED, no kernel-time or GPU gap.
- **Scroll-aware row reuse (125.C4):** REFUTED. `streaming-output` is the
  cheapest of the three despite one-line scrolls resolving to dense rebuilds.
- **Scrollback eviction (new, outside the original list):** CONFIRMED. The
  sole material gap. Candidate remediations: (a) evict in batches, letting
  scrollback overshoot by a fixed chunk before one drain, which amortises the
  shift by the chunk size and is a small change with a bounded memory cost;
  (b) make `rows` and its parallel structures ring buffers (`VecDeque` or an
  offset index), removing the shift entirely at the cost of touching every
  row-index consumer. Both need buffer tests for row-index, prompt/block, and
  image accounting across eviction, the `buffer_benches` scrollback-push
  benchmark, and a matched `sustained-output` re-capture.
- **Pointer workload:** INCONCLUSIVE. The maintainer-interactive pointer screen
  has not been run.
- **Accept residual:** not applicable while the eviction gap is open.

Maintainer decisions (2026-10-04): the pointer screen is skipped and the
pointer gate stays `INCONCLUSIVE`; remediation (b) is selected, in its full
form below. Batched eviction (a) is rejected as a partial fix: it keeps
eviction O(retained) per chunk, loosens the exact scrollback limit, and leaves
the absolute-index drift bugs in place.

## Remediation phase: stable-row-number `RowStore`

Goal: Freminal's `sustained-output` CPU at or below WezTerm's under the matched
protocol, with eviction cost proportional to rows evicted rather than rows
retained, and no row index anywhere that silently drifts on eviction.

The defect is structural. Rows are addressed by physical position in three
manually synchronised `Vec`s (`rows`, `row_cache`, `row_block_map`), so a
front eviction shifts the storage and forces every holder of a row index to be
rewritten (`adjust_prompt_rows`) or, where nobody rewrites it, to drift: GUI
selection (`frame_dirty.rs` documents it), DECSC saved cursor, and kitty
`RealPlacement.origin_row`. Per-eviction scans compound it:
`gc_unreferenced_blocks` (whole-map `HashSet`), `image_store.retain_referenced`
(all live cells), and `merge_cache = None` (full window re-merge).

### Fixed design direction

1. **One `RowStore`** owns rows, flatten cache entries, and block references
   together; the three parallel `Vec`s cease to exist as separately mutable
   fields.
2. **Stable logical row numbers.** Each row has a number that never changes for
   its lifetime: physical position plus a monotonic evicted-row base. Stored
   row references (prompts, command blocks, saved cursor, image placements,
   selection) hold logical numbers and are never rewritten on eviction.
3. **Eviction is O(evicted).** No whole-store shift, no whole-store scan.
   Compressed-block reclamation and image reachability become incremental
   counts maintained on push and evict.
4. **The scrollback limit stays exact.** No overshoot visible to any caller,
   snapshot, or test.
5. **Contiguous range access is preserved** for the flatten hot path, or its
   replacement is proven no slower by benchmark.
6. Task 120 (windowed reflow) builds on `RowStore`; this phase does not
   implement Task 120 but must not foreclose it.

### Remediation execution model

```text
125.11 design -> 125.12 benchmarks/baseline -> 125.13 RowStore (no behaviour
change) -> 125.14 logical row numbers -> 125.15 O(evicted) eviction ->
125.16 merge cache across eviction -> 125.17 snapshot + GUI coordinates ->
125.18 matched re-capture and closure
```

Strictly sequential, one active editor. Each subtask leaves `cargo test --all`
green and runs the buffer and snapshot benchmarks named in 125.12 before and
after.

### 125.11 — `RowStore` design

Scope: this document only (a design section appended to this subtask).

What: settle, against the current code: the storage mechanism (moving-head
contiguous store with half-capacity compaction versus `VecDeque` with
two-slice handling) with its flatten implications; the logical row-number type
and its name; which coordinates become logical (cursor and scroll offset are
screen-relative and may stay physical; decide and justify each); how
`BlockRowRef`, compression, decompression, alternate-screen save/restore,
resize reflow, `erase_scrollback`, and height-shrink drains map onto it; how
reflow renumbers or preserves logical rows and what that does to stored
references; the incremental block live-row count and image reference count
designs; the snapshot contract for logical numbers; and the public API
(`rows()`, `visible_rows()`, `SavedPrimaryState`) replacements. Enumerate every
stored row index found in the workspace and its disposition.

Deliverable: a decision record precise enough that 125.13-125.17 need no
further design choices. Any choice that changes user-visible behaviour is
flagged for the maintainer.

### 125.12 — Capacity benchmarks and baseline

Scope: `freminal-buffer/benches/buffer_row_bench.rs`,
`freminal-terminal-emulator/benches/buffer_benches.rs`,
`.opencode/skills/freminal-bench-table/SKILL.md`.

What: benchmarks that run at the real 10,000-row capacity: line-feed eviction
steady state with (i) plain rows, (ii) compressed blocks present, (iii) OSC 133
prompts and command blocks present, and (iv) an inline image present; plus an
emulator-level sustained-output ingest benchmark (`seq 1 200` bursts through
`handle_incoming_data` at capacity). Correct the stale 4,000/4,100 comments.
Capture a named Criterion baseline `before_125_rowstore`.

### 125.13 — Introduce `RowStore` with identical behaviour

Scope: `freminal-buffer` only.

What: move `rows`, `row_cache`, and `row_block_map` behind `RowStore` with the
API decided in 125.11, still Vec-backed and still front-draining, so every
existing test and benchmark is unchanged in behaviour. Mechanical call-site
migration; no semantic change.

### 125.14 — Logical row numbers

Scope: `freminal-buffer`, and the emulator call sites that pass row indices.

What: introduce the logical base and convert stored row references inside the
buffer and emulator (prompts, command blocks, saved cursor, image placements)
to logical numbers; delete the eviction-time rewrite in `adjust_prompt_rows`.
Regression tests prove each previously drifting reference stays attached to
its row across eviction.

### 125.15 — O(evicted) eviction

Scope: `freminal-buffer`.

What: switch `RowStore` eviction to the 125.11 mechanism; replace
`gc_unreferenced_blocks` and `image_store.retain_referenced` on the eviction
path with incremental counts. Benchmarks from 125.12 must show eviction cost
independent of retained-row count.

### 125.16 — Merge cache across eviction

Scope: `freminal-buffer/src/buffer/flatten.rs` and its tests.

What: key the visible-window merge cache by logical row so eviction no longer
forces a full re-merge; oracle tests at capacity rotation must still match.

### 125.17 — Snapshot and GUI coordinates

Scope: `freminal-terminal-emulator` snapshot, `freminal` GUI selection and fold
state.

What: export logical numbers plus base in `TerminalSnapshot`; move GUI
selection and fold ranges to logical numbers; remove the documented selection
drift workaround. Windows cross-check required.

### 125.18 — Matched re-capture and closure

Scope: this document, `assets/profiling/task125/` (one new workload).

What: add a `sustained-output` variant with shell integration prompts active
and an idle gap that engages compression; run the seven-repeat confirmation for
`sustained-output` and the new variant against both peers; record verdicts.
Task 125 closes only if Freminal is at or below WezTerm on `sustained-output`
with a CI excluding a regression, and no screened workload regressed.

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
