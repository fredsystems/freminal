# PLAN_125_VERTEX_RELAYOUT.md — Task 125 "Performance Parity and Residual Remediation"

> **STATUS: ENRICHED STUB — broadened after activation recon, deliberately
> not decomposed.** The 2026-08-26 recon established that fixed-stride vertex
> relayout cannot affect the observed idle CPU floor and that Task 124 did not
> collect the live changed-row, upload-byte, or GPU-time measurements needed
> to justify it for active workloads. Task 125 therefore starts with a
> measurement phase. Remediation subtasks are decomposed only after those
> measurements identify the remaining cost, per `plan-decomposition`'s
> just-in-time rule.
>
> **Version: unassigned.** Task 124 is complete and merged. Task 125 is
> deliberately **not** placed in v0.12.0. Its roadmap position remains a
> maintainer decision, informed by the measurement phase rather than assumed
> from the former relayout proposal.

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

## Open questions for activation

- What exact Freminal configuration and competitor versions produce the
  reported 0.1% versus 0.0% observation? Cursor blink, dimensions, font,
  opacity, shell, tabs, panes, and workload must match.
- Is the residual CPU work, GPU work, driver blocking, compositor/present
  cost, or wake frequency? Existing wall-clock phase timers cannot answer all
  five.
- How often does each `VertexRebuild` outcome occur in real workloads, and
  what is the changed-row-count distribution within `Bounded` frames?
- What are live per-buffer upload bytes per frame, rather than synthetic
  workload totals?
- Does the fixed-stride candidate's active-workload benefit justify it? Closing
  that branch is a legitimate result, not a failure.
- What is the padding's measured cost on a full rebuild, and at what ratio of
  partial-to-full frames does the relayout break even?
- Is `fg_instances` relayoutable in practice, given that glyph count per cell
  is not one — ligature clusters emit fewer, and wide characters and
  fallback-font runs complicate the mapping? A per-cell stride may need a
  maximum-glyphs-per-cell bound, and that bound needs justifying against real
  content rather than assumed.
- Does the instanced draw call need changing, or only the upload? 123 found
  **GL call count is independent of grid size** — an 8x2 and an 80x24 grid
  record byte-identical call sequences, only instance counts and byte volume
  scale — so a relayout that changed the call structure would be a
  regression against a property that is now pinned by
  `call_count_is_independent_of_grid_size`.
- Which version carries this task.

## Measurement-first activation scope

The activation session decomposes these in order. Remediation subtasks are not
invented until the findings gate closes.

### 1. Controlled parity protocol

Define reproducible Freminal, WezTerm, and Ghostty runs with matched window
dimensions, font, shell, cursor mode, opacity, tab/pane topology, and workload.
Record exact binary versions and renderer strings. Capture both blinking and
steady cursor cases rather than comparing unlike defaults.

The workload matrix must include:

- true idle with a visible blinking cursor;
- true idle with a steady cursor;
- scripted typing and a single-row update;
- a hidden-cursor TUI such as `btop`;
- scrollback scrolling;
- continuous PTY output;
- pointer motion over inert terminal content; and
- multiple tabs and panes with representative chrome.

Use an external process-level instrument such as `perf stat` for comparable
task-clock, user/system time, cycles, instructions, context switches, and
wakeups. `btop` remains a product-level smoke indicator, not the attribution
instrument. Always report frame rate and per-frame cost together.

### 2. Live rebuild and upload attribution

Extend the feature-gated profiling path to report:

- `VertexRebuild::{CursorOnly, Bounded, ReevaluateFullRebuild}` counts;
- the count of frames that reuse all prior vertex buffers;
- a changed-row-count histogram for `Bounded` frames;
- bytes uploaded per frame and per background, foreground, decoration,
  image, and atlas buffer; and
- frame outcome alongside upload volume, so `None`, cursor-only, sparse
  bounded, dense bounded, and full work cannot be conflated.

Default builds must retain zero instrumentation overhead. The counters observe
already-computed values and do not alter damage or rendering decisions.

### 3. CPU benchmark repair

Replace the current foreground all-rows-versus-one-row comparison with a
steady-state measurement whose glyph atlas is already populated, and measure
atlas rasterisation separately. Add background corpora that quantify sparse,
dense, and default-background-heavy emission so fixed-stride padding cost is
visible. Preserve the existing benchmark IDs where their meaning remains
accurate; use new IDs where it does not.

### 4. Asynchronous GPU timing

Add real-hardware GPU timing that separates, at minimum, buffer upload,
terminal draw, chrome draw, and total GPU execution. OpenGL timer-query results
must be polled on later frames; reading a query in the frame that issued it
would introduce the stall being measured. Unsupported contexts report the
capability as unavailable rather than changing behavior or carrying a second
renderer path.

The Task 123 recording harness continues to own deterministic call and byte
accounting. The pixel harness continues to own output correctness. Neither is
a substitute for real-GPU timing.

### 5. Findings and remediation gate

Record the parity matrix and attribute each material gap. For every candidate,
report expected benefit, measured ceiling, implementation complexity,
correctness risk, portability limits, and verification method. Then choose and
decompose only the supported remediation branches.

Candidates explicitly on the table include:

- bypassing most of the egui/chrome frame path for cursor-only redraws;
- caching retained chrome output while preserving current-frame hit testing;
- decoupling cursor blink rendering from full UI reconstruction;
- fixed-stride per-row uploads;
- CPU-side incremental row vertex construction;
- mapped or persistent GPU buffers with capability-based fallbacks;
- further scheduling and unnecessary-wakeup elimination; and
- presentation/compositor changes where the platform APIs expose a real lever.

Changing the default cursor from blinking to steady may alter the headline
idle number, but it is a product-default decision, not a performance
remediation, and may not be used to conceal blinking-cursor cost.

## Decision gates

- **Idle/chrome branch:** proceed only if matched blinking-cursor captures
  attribute a material competitor gap to Freminal's UI pass or presentation
  path.
- **Fixed-stride branch:** proceed only if live sparse `Bounded` frames are
  common enough that saved CPU work and upload bytes exceed the measured
  padding cost on dense/full rebuilds.
- **Persistent-buffer branch:** proceed only if GPU timing attributes material
  cost to upload/driver synchronization and a safe capability fallback is
  available on supported platforms.
- **Accept residual:** allowed only when the gap is measured, explained, and
  either shared by peers under matched conditions or smaller than every safe
  remediation's demonstrated cost/risk.

---

## Verification, when activated

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
  on the target laptop for every remediation claiming parity benefit.
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
