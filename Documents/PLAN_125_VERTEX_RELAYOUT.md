# PLAN_125_VERTEX_RELAYOUT.md — Task 125 "Fixed-Stride Vertex Relayout"

> **STATUS: ENRICHED STUB — deliberately not decomposed.** Every durable
> design decision made at Task 124's activation (2026-08-23) is recorded
> below. Subtask decomposition happens in a dedicated session when this task
> is activated, against the code as it then exists, per `plan-decomposition`'s
> just-in-time rule.
>
> **Version: unassigned.** This task is gated on Task 124's measurements and
> its roadmap position is a maintainer decision that has not been taken. It
> is deliberately **not** placed in v0.12.0, which is already carrying
> Tasks 118–124.

---

## Goal

Make a per-row dirty set translate into a per-row GPU upload.

Task 124 gives freminal the ability to say "these rows changed". This task
gives it the ability to *act* on that at the vertex layer, by making row N's
instance range a pure function of `row_idx` so that `glBufferSubData` at a
non-zero offset becomes possible.

---

## Relationship to Task 124

The division, set at 124's activation and not open for reinterpretation:

- **Task 124 stops doing work that produces no pixels.** It is the
  frame-count and present win.
- **Task 125 makes the work that does produce pixels cheaper.** It is the
  bandwidth win.

Task 123 measured the size of each. A needless full rebuild is roughly
**350x the bytes for roughly 1.08x the calls** — so 124's prize is counted in
frames avoided and 125's is counted in bytes not moved. Roughly 200 KB per
full-rebuild frame against under a kilobyte for a cursor-only frame
(`PLAN_123_GL_MEASUREMENT_HARNESS.md`, "Per-workload GL cost, 80x24").

### The gate

**This task is not activated until Task 124 has landed and measured the
residual.** 124.12 and 124.14 both carry mandatory before/after captures.
If, after 124, the remaining per-frame upload volume on realistic workloads
is small — because most frames are now `None` or `Region` and never rebuild
at all — then a vertex format relayout is not worth its risk and this task
should be closed rather than executed.

That gate is not ceremonial. It is the direct lesson of Task 121, which
closed with four of six candidate items refuted by their own verification
step, and of Task 123's Group G precedent where three code-reading
hypotheses were falsified in sequence before measurement found the real
cause.

---

## The problem, as established by recon (2026-08-23)

`upload_verts` (`freminal/src/gui/renderer/gpu.rs:1653-1670`) orphans the
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

| Buffer | Emission rule | Why the count varies |
| ------ | ------------- | -------------------- |
| `bg_instances` | one 6-float instance per **non-default-background** cell (`vertex.rs:390-431`) | `DefaultBackground` cells are skipped entirely (`vertex.rs:406-415`); count depends on content |
| `fg_instances` | one 13-float instance per **glyph** (`vertex.rs:722-784`) | ligature clusters, wide characters, and blink-hidden runs contributing zero |
| `deco_verts` | per-row underline and strikethrough quads, **interleaved** with non-row-scoped artifacts | search highlights, command-block hover tint, selection spans, and the cursor quad always appended last |

So row N's byte range differs in **length** between frames. A one-row change
is an insert-and-delete-and-shift for every row after it, not an overwrite in
place. `deco_verts` is worse than variable-length: a single row is not even
guaranteed to be *contiguous*, since a row can receive an underline quad in
the first pass and then be touched again by a selection quad appended much
later in the buffer.

`row_offsets` — the one per-row boundary index the buffer layer already
computes and threads onto the snapshot — describes offsets into the
**character** stream, which has a fixed relationship to row index. It has no
analogue for the instance buffers.

---

## Durable design decisions

These were settled at Task 124's activation and should not be re-derived.

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
slots for cells that emit nothing, so the byte volume of a *full* rebuild
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
painting every cell the clear colour *including its alpha* does not work as
a substitute for the clear either. With blending enabled an alpha-zero quad
writes nothing and therefore does not erase stale pixels; disabling blending
to force a replace breaks the image and shader compositing layers.

### 3. The clear stays

`GlState::clear` (`freminal-windowing/src/gl_context.rs:350-357`) is two
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

- Does 124's residual justify this at all? See "The gate" above. Closing this
  task is a legitimate outcome and should be reported as a result, not a
  failure.
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

---

## References

- `Documents/PLAN_124_RENDER_EFFICIENCY.md` — the damage-model task that
  produces the per-row dirty signal this task consumes, and the gate this
  task waits behind.
- `Documents/PLAN_123_GL_MEASUREMENT_HARNESS.md` — the measurement harnesses
  and the per-workload cost table quoted throughout.
- `Documents/PLAN_121_PERF_REMEDIATION.md` — closed; the source of the
  measure-before-fixing discipline this task's gate enforces.
- `Documents/PROFILING.md` — profiling methodology.
- Issue #432 — the silent visual corruption bug class this task shares.
- Issue #435 — partial present, the mechanism a per-row upload complements.
- Issue #440 — the missing pixel harness, closed by Task 123 Phase 2.
