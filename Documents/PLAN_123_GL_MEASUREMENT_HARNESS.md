# PLAN_123_GL_MEASUREMENT_HARNESS.md — Task 123 "GL Pipeline Measurement Harness"

> **STATUS: PHASE 1 COMPLETE, PHASE 2 BLOCKED ON `nix develop`** on
> `task-123/gl-measurement-harness`. Subtasks 123.1-123.10 are committed;
> 123.14's Phase 1 half is written below under Findings. 123.11-123.13 and
> 123.6b wait on the maintainer running `nix develop` after 123.10's
> `flake.nix` change, per `flake-dev-shell-discipline`. This document
> is the full subtask breakdown, written at activation time against the code
> as it stands on `main`. Subtask 123.1's audit corrected several factual
> claims made at activation time — see "Audit corrections (123.1)" below.
> Those corrections are folded into the prose that follows them.

Task 123 is carried by v0.12.0. See `Documents/PLAN_VERSION_120.md` for the
version summary and `Documents/MASTER_PLAN.md` for roadmap position.

---

## Relationship to other documents

Task 123 **supersedes** Task 121 subtask 121.28 (the pixel / headless-GL
harness that was scoped but never built) and **absorbs** 121.25's outstanding
measurement debt (the typing and sustained-motion workloads that were never
captured cleanly). Both subtasks stay recorded in
`Documents/PLAN_121_PERF_REMEDIATION.md` with a pointer here rather than being
deleted, per the numbering convention that document itself establishes.

**Task 124 depends on Task 123.** `Documents/PLAN_124_RENDER_EFFICIENCY.md`
is a stub for exactly this reason: per its own governing rule, no subtask
there is implemented until this task has quantified the thing it claims to
fix. 123 measures; 124 fixes. Concretely, 124.1's `Arc`-churn hypothesis and
124.2's diagnosis obligation (migrated from 121.31) are both restated below
as the two diagnostic obligations this task must discharge, because they are
Task 124's first inputs.

**Task 123 changes no rendering behaviour.** Every subtask here is
instrumentation, harness construction, or reporting. Any behaviour change
belongs to Task 124.

### The lesson this task exists to avoid repeating

Task 121 closed with four of its six issue #459 candidate items **refuted by
their own verification step** (121.18, 121.19, 121.21, 121.22 — see
`PLAN_121_PERF_REMEDIATION.md`). `DECOUPLING_FRAMEWORK.md` §12 records the
same lesson from PR #461, and Task 121's Group G records it a second time for
the chrome cache, where three code-reading hypotheses were falsified in
sequence before the actual cause was found by measurement. A plausible
finding derived from static reading is a hypothesis, not a work item, in this
codebase specifically. Task 123 builds the instrument so that the next round
of findings does not repeat the pattern.

---

## Goal

Quantify freminal's rendering cost — draw calls, state changes, GPU uploads,
and eventually pixels — so that Task 124's remediation is defined by
measurement rather than hypothesis. The work splits into two independent
phases with very different infrastructure requirements:

- **Phase 1 — call-recording harness.** No GPU, no display server, runs in
  the existing CI matrix unmodified.
- **Phase 2 — pixel/readback harness.** Linux-only, needs new Nix and CI
  infrastructure, and closes the gap `PROFILING.md` names as the single
  biggest hole in the methodology: "there is no headless-GL or
  pixel-readback harness... a regression that changes what is drawn, rather
  than how often, is undetectable in CI."

---

## Phase 1 — call-recording harness

### Why this is tractable with no new infrastructure

- `RenderState` (`freminal/src/gui/terminal/widget.rs:1165-1215`) is
  constructible with no GL context — via the free function
  `new_render_state` (`widget.rs:1245`), not a `RenderState::new`.
- `GlyphAtlas::new` (`freminal/src/gui/atlas.rs:186`) takes no `gl`.
- Fonts are embedded via `include_bytes!`
  (`freminal/src/gui/font_manager.rs:36-44`), and
  `FontManager::new` already runs headlessly at 7 call sites in
  `freminal/benches/render_loop_bench.rs`.
- GL objects are created lazily via `renderer.init(gl)`, gated on
  `renderer.initialized()` (`widget.rs:2960-2965`).

So a test can build `RenderState`, call `init` and the `draw_*` family
against a recording `Gl`, and assert on the resulting call log — no window,
no context, no driver.

### The blocking fact: `glow::HasContext` is sealed

`glow::HasContext` is **sealed**: `glow-0.17.0/src/lib.rs:142` declares
`pub trait HasContext: __private::Sealed`, and `__private::Sealed`
(`lib.rs:4845-4849`) is implemented only for glow's own `native::Context`
(`native.rs:198`). Implementing it on a wrapper is a **hard compile error**,
not a design tradeoff to weigh. State this plainly in code comments and
commit messages so nobody retries the trait-implementation approach.

The trait has 396 methods and 12 associated types; freminal uses only **49
distinct entry points** (enumerated below, and frozen in code at
`freminal/src/gui/renderer/gl_facade/surface.rs`). freminal is monomorphic
over a concrete `&glow::Context` throughout — call-site counts are
`freminal/src/gui/renderer/gpu.rs` 268, `toast_pass.rs` 37,
`toast_text_pass.rs` 37, `widget.rs` 4, `app_impl.rs` 5 — with 52 parameters
carrying that type and **zero generic bounds to rewire**. There are two
capture points in the GUI, not one: `let gl = painter.gl();` at
`freminal/src/gui/terminal/widget.rs:2955` and the equivalent inside the
`PaintCallback`s in `freminal/src/gui/app_impl.rs` (around `:2409` and
`:3200`). The context itself is stored as `Arc<glow::Context>`
(`freminal-windowing/src/gl_context.rs:191`) and created at `:284-286`.

Handle types have **public tuple fields**, so a recording backend can
fabricate its own handles without touching the real driver:
`pub struct NativeBuffer(pub NonZeroU32)` (`glow-0.17.0/src/native.rs:169`);
also `NativeShader` (`:163`), `NativeProgram` (`:166`),
`NativeVertexArray` (`:172`), `NativeTexture` (`:175`),
`NativeFramebuffer` (`:184`), and `NativeUniformLocation(pub GLuint)`
(`:193`).

### The design — specified, not open for re-litigation

The maintainer has already chosen the shape. Subtasks implement it; they do
not re-open it. A concrete facade struct, **not** a trait and **not**
generics:

```rust
pub struct Gl<'a> {
    inner: GlTarget<'a>,
}

enum GlTarget<'a> {
    Real(&'a glow::Context),
    Recording(RecordingState),
}
```

Every renderer signature changes from `gl: &glow::Context` to
`gl: &Gl<'_>`. All 47 methods live on `Gl` as thin dispatch: the `Real` arm
delegates directly to `glow::Context`; the `Recording` arm appends a
call-record entry and returns a fabricated handle where one is expected.

**Rationale to record in the code and in this document:** a trait would
force generics at 52 parameter sites across five files, widening every
one of those signatures. The enum keeps codegen monomorphic in the `Real`
arm and makes the migration mechanical — a search-and-replace of the
parameter type, not a redesign of call sites.

**Recording must be feature-gated**, mirroring the existing
`frame-profiling` precedent (`freminal/Cargo.toml`,
`freminal-windowing/frame-profiling`), so that a production build compiles
`Gl` down to direct delegation with no per-call branch. Unlike
`frame-profiling`, which is instrumentation-only and never asserted against
in a test, this facade's zero-cost claim for the default build must be
**verified by a benchmark**, not merely asserted in a doc comment — see
123.6.

### The 49 entry points

For sizing the facade and driving 123.1's guard. This list is frozen in code
as `GL_CALL_SURFACE` in `freminal/src/gui/renderer/gl_facade/surface.rs`;
that array is the source of truth and this block mirrors it.

```text
active_texture, attach_shader, bind_buffer, bind_framebuffer, bind_texture,
bind_vertex_array, buffer_data_size, buffer_data_u8_slice,
buffer_sub_data_u8_slice, check_framebuffer_status, clear, clear_color,
compile_shader, create_buffer, create_framebuffer, create_program,
create_shader, create_texture, create_vertex_array, delete_buffer,
delete_framebuffer, delete_program, delete_shader, delete_texture,
delete_vertex_array, disable, draw_arrays, draw_arrays_instanced, enable,
enable_vertex_attrib_array, framebuffer_texture_2d, get_program_info_log,
get_program_link_status, get_shader_compile_status, get_shader_info_log,
get_uniform_location, link_program, pixel_store_i32, scissor, shader_source,
tex_image_2d, tex_parameter_i32, tex_sub_image_2d, uniform_1_f32,
uniform_1_i32, uniform_2_f32, use_program, vertex_attrib_divisor,
vertex_attrib_pointer_f32
```

Derived metric groups the recording log must be able to answer without a
second pass:

- **draw calls** = `draw_arrays` + `draw_arrays_instanced`
- **state changes** = the `bind_*` family plus `use_program` / `enable` /
  `disable` / `scissor`
- **uploads** = `buffer_data_*` / `buffer_sub_data_u8_slice` /
  `tex_image_2d` / `tex_sub_image_2d`

`active_texture` is counted in the state-change group. The plan's original
wording did not name it, but it is a texture-unit selector — a bind-family
state change — and omitting it would undercount. All three groups are frozen
in code as `DRAW_CALL_METHODS`, `STATE_CHANGE_METHODS` and `UPLOAD_METHODS`
alongside `GL_CALL_SURFACE`.

### Audit corrections (123.1)

123.1's prohibition required that any discrepancy found during the audit be
reported and this document corrected rather than silently absorbed. Five
were found. Each is already folded into the prose above; they are itemised
here so the correction is on the record and the original claim is not
quietly rewritten out of history.

| # | Claim at activation | Audited reality |
| - | ------------------- | --------------- |
| 1 | 47 distinct `HasContext` entry points | **49.** `create_program` (`gpu.rs:1614`) and `create_shader` (`gpu.rs:1643`) are written as multi-line method chains, so the activation-time grep missed them. Without them the facade cannot create a shader or a program at all. |
| 2 | `gpu.rs` has 266 call sites | **268** — exactly the two missed multi-line chains. |
| 3 | All GL calls live in `gpu.rs`, `toast_pass.rs`, `toast_text_pass.rs` | **Nine further production call sites** exist: `widget.rs` (`bind_framebuffer`, `enable`, `scissor`, `disable`) and `app_impl.rs` (`bind_framebuffer` ×3, `clear_color`, `clear`). See the scope note below. |
| 4 | "roughly 40" `&glow::Context` parameters | **52** workspace-wide; 46 within the three originally-named files. |
| 5 | `atlas.rs` / `font_manager.rs` live under `src/gui/renderer/` | They live at `freminal/src/gui/atlas.rs` and `freminal/src/gui/font_manager.rs`. The cited line numbers were correct; only the paths were wrong. |

**Scope decision arising from correction 3 (maintainer, 2026-08-21).** The
nine calls in `widget.rs` and `app_impl.rs` are **in scope** for the
migration, and 123.1's guard therefore polices the **whole `freminal`
crate**, not just `src/gui/renderer/` as 123.1 originally specified. Two
reasons. First, both files already capture `let gl = painter.gl();` and must
construct the facade regardless in order to call the migrated `gpu.rs`.
Second, and decisively, `widget.rs`'s `enable`/`scissor`/`disable` calls are
interleaved *between* calls into `gpu.rs` inside a single `PaintCallback` —
leaving them raw would make the recording log's state-change metric silently
undercount, which is precisely the measurement this task exists to produce.

---

## Phase 2 — pixel/readback harness

### Why this needs new infrastructure

- `GlState::new` (`freminal-windowing/src/gl_context.rs:200-325`) is
  hard-wired to a winit `Window` and `WindowSurface`:
  `compatible_with_native_window` (`:210-214`) and
  `SurfaceAttributesBuilder::<WindowSurface>` (`:258-262`). glutin 0.32.3
  does provide `PbufferSurface` (`glutin-0.32.3/src/surface.rs:228`), so an
  offscreen path is possible, but it is new code, not a config flag.
- `flake.nix` has **no Mesa driver**. `pkgs.libGL` resolves to
  `libglvnd-1.7.0`, a vendor-neutral **dispatcher with no rendering
  backend**. `pkgs.mesa` (which ships `llvmpipe` and `softpipe`) is absent.
  `pkgs.xorg.xvfb` is absent. winit 0.30.13 has no headless backend and
  needs a real X11 or Wayland connection to construct a window at all.
- Additions required: `pkgs.mesa`, `pkgs.mesa.llvmpipeHook` (sets
  `LIBGL_ALWAYS_SOFTWARE`, `LIBGL_DRIVERS_PATH`, and the EGL vendor JSON),
  and `pkgs.xorg.xvfb`. **Per `flake-dev-shell-discipline`, the subtask that
  edits `flake.nix` must stop and wait for the maintainer to run
  `nix develop`** before any later Phase 2 subtask can build against it.
  This is an explicit stop condition on 123.10, not a suggestion.
- **CI shape matters, and it is not what a casual read of `ci.yml` suggests.**
  `.github/workflows/ci.yml`'s `test` job runs real test execution on
  `[ubuntu-latest, windows-latest, macos-latest, ubuntu-24.04-arm]` via
  `dtolnay/rust-toolchain`, **not Nix**, so it inherits nothing from
  `flake.nix`. Only `build-and-check` (the pre-commit job) and
  `nightly.yml`'s `ci` job invoke `nix develop`. Phase 1 therefore lands in
  the existing matrix with no CI change; Phase 2 needs a **new Nix-based
  job**, because llvmpipe and Xvfb only exist inside the flake's dev shells.
- **Linux-gating precedent already exists in this repo.**
  `Documents/PROFILING.md` Tier 2 (`perf`) is Linux-only because `perf` is
  `stdenv.isLinux`-gated in `flake.nix` (the `pkgs.perf` entry under the
  Linux-only package list, alongside the Windows cross-check toolchain).
  Phase 2 follows the same precedent rather than inventing a new one.
- **Golden-file precedent exists, but only for the convention, not the
  mechanism.** The vttest suite
  (`freminal-terminal-emulator/tests/vttest_*.rs` plus
  `freminal-terminal-emulator/tests/golden/`) uses `UPDATE_GOLDEN=1` to
  regenerate golden files, documented at
  `freminal-terminal-emulator/tests/vttest_common.rs:14, 183-208`. That
  suite compares text/buffer state, not pixels, so it supplies the
  regeneration convention (an env var, an explicit opt-in, a clear failure
  message pointing at it) — Phase 2 must design its own comparison and
  tolerance mechanism from scratch.

### Flakiness is a first-class risk, not an afterthought

llvmpipe's rasterization output can vary across Mesa versions, and
`flaky-tests-are-bugs` forbids papering over that with retries, `#[ignore]`,
or loosened tolerances discovered after the fact. Phase 2's tolerance policy
must be decided and justified **before** any golden image is captured
(123.11), and the new CI job must **not gate PRs** until it has demonstrated
stability over an observation period — see 123.12's stop condition.

---

## Subtask summary

| Subtask | Phase | Title                                                              |
| ------- | ----- | ------------------------------------------------------------------ |
| 123.1   | 1     | Enumerate and freeze the GL call surface, with a compile-time guard |
| 123.2   | 1     | Define the `Gl` facade and `RecordingState`                        |
| 123.3   | 1     | Handle fabrication for the recording backend                        |
| 123.4   | 1     | Migrate `gpu.rs` to the `Gl` facade                                  |
| 123.5   | 1     | Migrate `toast_pass.rs` and `toast_text_pass.rs`                    |
| 123.6   | 1     | Verify zero production overhead (static proof; see re-scope note)   |
| 123.6b  | 2     | Benchmark the `Real` arm against a direct call (gated on 123.11)    |
| 123.7   | 1     | Headless render-path driver                                          |
| 123.8   | 1     | Workload assertion tests against the recording log                  |
| 123.9   | 1     | Wire Phase 1 into the existing CI matrix                             |
| 123.10  | 2     | `flake.nix`: Mesa, llvmpipe, Xvfb (STOP for `nix develop`)           |
| 123.11  | 2     | Offscreen pbuffer GL context                                         |
| 123.12  | 2     | Readback, golden storage, comparison, and tolerance policy           |
| 123.13  | 2     | New Nix-based CI job for Phase 2                                     |
| 123.14  | both  | Quantified findings report                                          |

Ordering: 123.1 through 123.9 are Phase 1 and largely sequential (each
migrates or depends on the previous). 123.10 through 123.13 are Phase 2 and
depend on nothing in Phase 1 completing first, but in practice should follow
Phase 1 so the facade Phase 2 measures through already exists. 123.14 depends
on both phases having landed at least their Phase 1 half; Phase 2's
contribution to 123.14 (pixel-level findings) may follow later without
blocking Phase 1's contribution (call-count findings).

---

## Subtasks

### 123.1 — Enumerate and freeze the GL call surface, with a compile-time guard

**Status: complete.** `freminal/src/gui/renderer/gl_facade/`
(`mod.rs`, `surface.rs`) plus the module declaration in
`freminal/src/gui/renderer/mod.rs`.

Scope: a new small module documenting the entry points (candidate
location: alongside the future `Gl` facade module under
`freminal/src/gui/renderer/`), plus a lint or test that fails if a raw
`gl.` call (on `glow::Context` directly) appears anywhere in the `freminal`
crate outside the facade module itself once 123.4 and 123.5 land.

Deliverable: the frozen list (reproduced in this document above), and a
guard — a `grep`-based test or a clippy-level check acceptable to
`rust-best-practices` — that catches a new raw call being added later
without going through `Gl`.

Verification: `cargo test --all`. The guard must be demonstrated to fail
against a deliberately-introduced raw call in a scratch commit, then
reverted before landing.

Prohibitions: do not migrate any call site yet — that is 123.4 and 123.5.
Do not add methods to the list beyond those enumerated; if a further call
site is found during the audit, report it and update this document rather
than silently including it.

Stop: report the frozen list and the guard's mechanism; await review before
123.2.

**As built.** The list was audited to 49, not 47 (see "Audit corrections"
above). The guard is import-based rather than method-name-based: calling any
`HasContext` method requires the trait in scope, so
`has_context_use_sites_match_allowlist` walks every `.rs` file under
`freminal/src/`, flags each non-comment line mentioning `HasContext`, and
asserts the resulting file set equals a `NOT_YET_MIGRATED` allowlist **in
both directions** — a new offender fails as unexpected, and a migrated file
left behind fails as stale, so the allowlist cannot rot. The `gl_facade/`
subtree is excluded at the walk level, because it is the migration target
and its `Real` arm must reference the trait. The allowlist currently holds
the five pre-migration files and shrinks to empty at 123.5. Two further
tests pin the frozen list's sortedness/uniqueness and the metric groups'
membership in it.

### 123.2 — Define the `Gl` facade and `RecordingState`

Scope: new module, `freminal/src/gui/renderer/gl_facade.rs` (or an
equivalent name consistent with `freminal-module-cohesion` — one concept,
"the GL call boundary", per module).

What: the `Gl<'a>` struct and `GlTarget<'a>` enum exactly as specified
above, with all 47 methods implemented as thin dispatch. `RecordingState`
holds an append-only log of typed call records (one variant per method, or a
single record type carrying an opcode plus fabricated-handle bookkeeping —
either is acceptable; do not add fields the workload assertions in 123.8 do
not need). Gate `GlTarget::Recording` and `RecordingState` behind a Cargo
feature (name it consistently with the `frame-profiling` precedent, e.g.
`gl-recording`).

Deliverable: the facade compiling with no call sites migrated yet (dead code
is expected and acceptable at this point since the module is not yet wired
in — allow it locally with a dated TODO referencing 123.4, not a bare
`#[allow(dead_code)]`, per the repo's no-dead-code-without-TODO rule).

Verification: `cargo test --all`, `cargo test --all --features
gl-recording`, `cargo clippy --all-targets --all-features -- -D warnings`,
`cargo machete`.

Prohibitions: do not touch `gpu.rs`, `toast_pass.rs`, or
`toast_text_pass.rs` yet. Do not use raw `as` casts in the dispatch bodies —
`conv2` per `freminal-numeric-conversions`.

Stop: report the module's public API; await review before 123.3.

### 123.3 — Handle fabrication for the recording backend

Scope: `freminal/src/gui/renderer/gl_facade.rs`.

What: for each of the seven handle types (`NativeBuffer`, `NativeShader`,
`NativeProgram`, `NativeVertexArray`, `NativeTexture`, `NativeFramebuffer`,
`NativeUniformLocation`), give `RecordingState` a monotonic counter and
construct a fabricated handle from it using the public tuple field. Ensure
fabricated handles are distinguishable from real ones only by convention
(the `Recording` arm never sees a real handle, so no collision is possible
within one `Gl` instance).

Deliverable: `create_buffer`, `create_shader` (via `compile_shader`'s
internals if applicable), `create_program`, `create_vertex_array`,
`create_texture`, `create_framebuffer`, and `get_uniform_location` all
functional against `RecordingState` and returning fabricated handles that
round-trip through `delete_*` and `bind_*` calls without panicking.

Verification: unit tests exercising create/bind/delete sequences against
`RecordingState` directly (no `RenderState`, no `RenderData` needed yet).
Standard suite.

Prohibitions: do not depend on `glow::Context` inside the `Recording` arm
for anything — it must be fully independent of a real driver.

Stop: report handle-fabrication test results; await review before 123.4.

**Boundary shift, recorded at execution time.** As written, 123.2 and 123.3
cannot be separated: the facade does not compile with the seven
handle-returning methods' `Recording` arms absent, and each commit must
leave `cargo test --all` green. Handle fabrication therefore landed in
123.2, and **123.3 became the behavioural verification subtask** — the
`recording_tests` module that drives the facade end to end and proves the
`Recording` arm is a trustworthy driver stand-in. The split is still
create-then-prove; only the line between "defined" and "demonstrated"
moved. 123.3's substance is unchanged: the create/bind/delete round trips
the original wording asked for are there, plus the assertion that every one
of the 49 methods records itself under its *own* name (the copy-paste error
the 123.2 structural check cannot see), and hand-derived expected values for
each metric group, which is what 123.8's workload numbers rest on.

### 123.4 — Migrate `gpu.rs` to the `Gl` facade

Scope: `freminal/src/gui/renderer/gpu.rs`.

What: change all 268 call sites from `gl: &glow::Context` to `gl: &Gl<'_>`,
and every `gl.<method>(...)` call to go through the facade. This is
mechanical per the design's own rationale (monomorphic, no generics to
rewire) but is the largest single-file diff in the task, so keep it its own
commit separate from 123.5.

Deliverable: `gpu.rs` compiling and passing existing tests unchanged in
intent, with zero raw `glow::Context` calls remaining (verified by 123.1's
guard).

Verification: standard suite. Existing render-path tests and benches
(`freminal/benches/render_loop_bench.rs`) must build and pass with no
tolerance changes — a needed tolerance change here is a red flag per
`agents.md`, not a fix.

Prohibitions: do not change any GL call's arguments, ordering, or the
sequence of state changes — this subtask changes the type signature only,
never behaviour. Do not migrate `toast_pass.rs` or `toast_text_pass.rs` in
this commit.

Stop: report the diff size and that no test tolerance changed; await review
before 123.5.

### 123.5 — Migrate `toast_pass.rs` and `toast_text_pass.rs`

Scope: `freminal/src/gui/renderer/toast_pass.rs`,
`freminal/src/gui/renderer/toast_text_pass.rs`,
`freminal/src/gui/terminal/widget.rs`, `freminal/src/gui/app_impl.rs`.

What: the same mechanical migration as 123.4, applied to the remaining 37 +
37 call sites in the two toast passes, and to the nine call sites in
`widget.rs` and `app_impl.rs` that correction 3 above added to the task's
scope. The latter two are also the **capture points**: each must construct
the facade around the `&glow::Context` it gets from `painter.gl()` and pass
`&Gl<'_>` onward into the migrated renderer, rather than passing the raw
context.

Deliverable: all four files compiling and passing existing tests, zero raw
`glow::Context` calls remaining anywhere in the `freminal` crate outside
`gl_facade/` per 123.1's guard — empty the guard's `NOT_YET_MIGRATED`
allowlist at the end of this subtask.

Verification: standard suite, plus running 123.1's guard and confirming it
now passes clean (no raw calls left) and still fails against a scratch
reintroduction.

Prohibitions: same as 123.4 — no behaviour change, no argument or ordering
changes.

Stop: report that the guard is now enforced repo-wide; await review before
123.6.

**123.4 and 123.5 landed as one commit, and could not have landed as two.**
The plan separated them on diff size, assuming `gpu.rs` and the two toast
passes were independent. They are not: `toast_pass.rs` and
`toast_text_pass.rs` import `compile_program`, `upload_verts`,
`setup_fg_inst_attribs`, `gl_i32`, `gl_f32_i32` and `gl_i32_u32` **from**
`gpu.rs`, so changing those helpers' `gl` parameter breaks both toast files
in the same edit. `agents.md` requires every commit to leave
`cargo test --all` green, and no ordering of the two subtasks satisfies both
constraints without inventing transitional double-signature shims — which
would be more code, and more risk, than the migration itself.

The same coupling forced `widget.rs` and `app_impl.rs` (callers of
`gpu.rs`) and `toast.rs` (caller of the toast passes) into the same commit.
The resulting diff is nonetheless small — seven files, ~70 changed lines —
because the facade's methods mirror `glow::HasContext`'s signatures exactly,
so **not one call expression changed**. Only the 46 parameter types, six
imports, and three `Gl::real(painter.gl())` construction sites did. Some
multi-line signatures were reflowed onto one line by `rustfmt` purely
because `&Gl<'_>` is shorter than `&glow::Context`.

### 123.6 — Verify zero production overhead with a benchmark

Scope: a new or extended Criterion bench under `freminal/benches/`
(coordinate the exact file with `freminal-bench-table`; this subtask is
also the point at which that skill's catalog should gain an entry for the
new facade, since it now sits on every render call).

What: benchmark the `Real` arm of `Gl` against calling `glow::Context`
directly (pre-123.4 baseline captured from the same commit range, or from
`main` if 123.4/123.5 have already merged) to substantiate the "no per-call
branch in production" claim rather than asserting it in a doc comment.

Deliverable: a benchmark ID demonstrating the delegation path has no
measurable per-call cost versus the direct call, per
`performance-benchmarks`'s before/after procedure and 15% threshold.

Verification: `cargo bench --no-run --all` compiles; the benchmark run
itself is reported per `performance-benchmarks` (not gated in CI, per the
existing weekly-schedule precedent for `bench.yml`).

Prohibitions: do not use this benchmark to justify skipping the facade
entirely if it does show overhead — report the number and let the
maintainer decide; do not silently redesign the dispatch to hide a cost.

Stop: report the benchmark result; await review before 123.7.

**Re-scoped at execution time (maintainer decision, 2026-08-21): the static
half lands here, the dynamic half becomes 123.6b in Phase 2.** As written,
123.6 is not implementable in Phase 1 and the plan did not notice. The
`Real` arm delegates to live GL function pointers, so benchmarking it needs
a real `glow::Context` — which needs a display server and a driver, exactly
the infrastructure 123.10 and 123.11 exist to build. A `Context` built from
a stub loader holds null function pointers and segfaults on first call, so
there is no headless shortcut.

What is provable now is **narrower than the benchmark would have been, not
stronger** — an earlier revision of this note claimed otherwise and
overstated it. In a default build `GlTarget` has exactly one variant, so
Rust lays it out identically to `&glow::Context` with no discriminant, and
the `match` in all 49 methods is irrefutable: there is nothing to
discriminate, so there is no condition to test. That is an argument from
the language's semantics, and it is good evidence — but it is an argument,
not a measurement. **No test here reads generated code, so "no per-call
branch in the emitted machine code" stays unverified until 123.6b.** What
the two tests in `facade.rs` actually pin is memory layout: `size_of::<Gl<'_>>()` equals `size_of::<&glow::Context>()`
in a default build (measured: 8 and 8, with matching alignment), and is
strictly greater under `gl-recording` (measured: 72), which is what keeps
the first assertion from being vacuous. Unlike a Criterion benchmark, these
run in the ordinary `cargo test` matrix on all four CI platforms and cannot
go quiet the way an unwired benchmark can.

### 123.6b — benchmark the `Real` arm against a direct `glow::Context` call

Phase 2. **Gated on 123.11** (offscreen pbuffer context). Carries 123.6's
original wording and its original prohibition: if the delegation does show
measurable per-call cost, report the number and let the maintainer decide —
do not silently redesign the dispatch to hide it, and do not use the result
to argue the facade away. Coordinate the bench file with
`freminal-bench-table` and follow `performance-benchmarks`'s before/after
procedure and 15% threshold.

### 123.7 — Headless render-path driver

Scope: new test-support module (candidate:
`freminal/src/gui/renderer/test_support.rs` or a `tests/` helper,
consistent with `freminal-module-cohesion`), reusing the existing headless
pattern already proven at the 7 `FontManager::new` call sites in
`freminal/benches/render_loop_bench.rs`.

What: a driver that constructs `RenderState`, a `GlyphAtlas`, and a
`FontManager` with no GL context, then calls `renderer.init(gl)` and the
`draw_*` family against a `Gl` in `Recording` mode, producing a call log for
a given synthetic frame (a fixed set of cells, cursor state, and optional
toast).

Deliverable: the driver function(s), callable from tests with a small,
explicit synthetic workload description as input.

Verification: standard suite. A smoke test confirming the driver produces a
non-empty, well-formed call log for at least one trivial workload (e.g. a
single-cell clear-and-cursor-draw).

Prohibitions: do not attempt to reproduce full `App::update` orchestration
here — this drives the renderer directly, below the GUI event layer. Do not
add a GPU or window dependency; if one turns out to be unavoidable, stop and
report rather than reaching for Phase 2's infrastructure prematurely.

Stop: report the driver's API and the smoke test result; await review
before 123.8.

### 123.8 — Workload assertion tests against the recording log

Scope: new test module(s) alongside 123.7's driver.

What: for each of the workloads named in 123.14 that can be exercised
without a real window (idle, pointer motion over inert content, pointer
motion with a URL on screen, typing, full-screen TUI redraw, alt screen),
construct the synthetic input and assert on the derived metrics — draw
call count, state-change count, upload count and byte volume — using the
groupings defined in the "Derived metric groups" section above.

Deliverable: one test per workload, each asserting concrete numbers (not
just "did not panic"), so a future regression that changes call counts is
caught in CI without a GPU.

Verification: standard suite. Document, next to each assertion, which real
GUI code path it stands in for, so a future reader can tell the difference
between "this is exactly what production does" and "this is a
representative approximation" — do not overstate the fidelity of a
synthetic driver.

Prohibitions: do not assert on pixel content — that is Phase 2's job, not
this subtask's. Do not invent numbers for workloads this driver cannot
represent; if a workload genuinely needs Phase 2, say so explicitly rather
than approximating it here.

Stop: report per-workload assertion results; await review before 123.9.

### 123.9 — Wire Phase 1 into the existing CI matrix

Scope: `.github/workflows/ci.yml` (the `test` job only — no new job needed,
per this task's own finding that Phase 1 requires no GPU or Nix).

What: confirm the new tests run under the existing matrix
(`[ubuntu-latest, windows-latest, macos-latest, ubuntu-24.04-arm]`) with no
platform-specific gating, since Phase 1 is pure Rust with no GL driver
dependency. If any platform fails for a reason unrelated to the facade
itself (e.g. a `conv2` cast difference), fix it in this subtask; if the
failure implicates the facade design, stop and report rather than adding a
platform-specific `#[cfg]` to route around it.

Deliverable: green CI on all four platforms with Phase 1's tests included.

Verification: the CI run itself; `cargo xtask check-windows` locally per
`freminal-windows-crosscheck` before any PR, since this subtask touches CI
configuration.

Prohibitions: do not touch `nightly.yml` or `build-and-check` — those are
Phase 2's concern (123.13). Do not weaken the existing matrix.

Stop: report the green CI run; await review before starting Phase 2.

**Outcome: no CI change was required, and the reason is worth recording**
because it is not obvious and it is the thing that would silently break
this. Every Phase 1 test added by 123.3, 123.7 and 123.8 is gated behind
`#[cfg(feature = "gl-recording")]`, so a plain `cargo test --all` does not
run any of them. They run in CI only because the `test` job invokes
`cargo xtask test`, and `xtask`'s `test_libs`/`test_docs` pass
`--all-features` — which picks up `gl-recording` along with everything
else. The tests therefore execute on all four matrix platforms
(`ubuntu-latest`, `windows-latest`, `macos-latest`, `ubuntu-24.04-arm`)
with no platform-specific gating and no workflow edit.

The fragility this creates should be understood by anyone touching
`xtask`: **if `--all-features` is ever dropped from `test_libs`, the entire
Phase 1 harness stops running in CI and nothing fails.** The suite would
still be green; it would simply no longer be testing this. `xtask`'s
`test_default_features` pass is the complement and correctly runs *without*
the feature, which is what proves the default build is unaffected.

Verified locally: `cargo xtask test` (the exact CI invocation) passes, and
`cargo xtask check-windows` is clean, per `freminal-windows-crosscheck`.

### 123.10 — `flake.nix`: Mesa, llvmpipe, Xvfb

Scope: `flake.nix` only.

What: add `pkgs.mesa`, `pkgs.mesa.llvmpipeHook`, and `pkgs.xorg.xvfb` to the
Linux-only package set, following the existing
`pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ ... ]` pattern already
used for `pkgs.perf` and the Windows cross-check toolchain. Wire
`llvmpipeHook`'s environment (`LIBGL_ALWAYS_SOFTWARE`,
`LIBGL_DRIVERS_PATH`, EGL vendor JSON) into the relevant dev shell(s).

Deliverable: the `flake.nix` diff only.

**Stop condition, mandatory per `flake-dev-shell-discipline`: this subtask
stops here.** Do not proceed to 123.11 until the maintainer has run
`nix develop` (or `direnv allow`) and confirmed the new tools are present.
Do not work around the missing tools by installing them out-of-band or by
changing application logic to avoid needing them.

Verification: `nix flake check` if available in the environment; otherwise
report the diff and wait.

Prohibitions: do not add any non-Linux equivalent — Phase 2 is Linux-only by
design, matching the `perf` precedent. Do not touch any `.rs` file in this
subtask.

Stop: report the diff; wait for `nix develop` confirmation before any
further Phase 2 work.

**As built, with two deviations, both recorded rather than taken silently.**

1. **`pkgs.xorg.xvfb` -> `pkgs.xvfb`.** The attribute this document named is
   deprecated in the pinned nixpkgs, which emits
   `the xorg package set has been deprecated, 'xorg.xvfb' has been renamed
   to 'xvfb'` on evaluation. `pkgs.xvfb-run` is added alongside it, since
   the wrapper is what a CI job and a local run actually invoke.

2. **`pkgs.mesa.llvmpipeHook` was not used.** The three variables it would
   set are set explicitly in a new `glPixelEnv` attrset instead. The hook's
   contents cannot be inspected without building it, and a setup hook that
   silently overrides an explicitly-set variable is action-at-a-distance
   this file otherwise avoids. The explicit form also matches the
   `windowsCheckEnv` idiom immediately above it. Both interpolated paths
   (`${pkgs.mesa}/lib/dri` and
   `${pkgs.mesa}/share/glvnd/egl_vendor.d/50_mesa.json`) were verified to
   exist in the `mesa` derivation before being written.

`glPixelEnv` is merged into the **`default`** shell only, never `ci` —
forcing software GL in the CI shell would silently slow every unrelated
check. 123.13's job must therefore use the `default` shell (or a dedicated
one), not `ci`.

Verified without building: `nix eval .#devShells.x86_64-linux.default.drvPath`
succeeds, and `nixfmt` / `statix check` / `deadnix --fail` are all clean.
Actual availability of the tools is what the maintainer's `nix develop`
confirms — that is the stop condition, and it has not been bypassed.

### 123.11 — Offscreen pbuffer GL context

Scope: new module in `freminal-windowing/src/` (candidate:
`gl_context_offscreen.rs`, named for the one concept — an offscreen context
construction path — distinct from the windowed path in `gl_context.rs`).

What: using glutin 0.32.3's `PbufferSurface`
(`glutin-0.32.3/src/surface.rs:228`), build an offscreen GL context that
does not require a winit `Window`, but does require Xvfb (for the X11
connection glutin still needs to enumerate configs on Linux) and the
llvmpipe software rasterizer from 123.10.

Deliverable: a function that returns a working GL context and an offscreen
framebuffer of a given pixel size, runnable under `xvfb-run` in the `default`
Nix dev shell.

Verification: a manual smoke test (clear to a known color, read back,
assert the color) run locally under `nix develop` with `xvfb-run`. This
subtask cannot be verified by `cargo test --all` alone until 123.13 wires
CI, so report the manual run's output explicitly.

Prohibitions: do not modify `gl_context.rs`'s windowed path — this is an
additive, parallel construction path. Do not attempt to make this work on
macOS or Windows; Phase 2 is Linux-only per the `perf` precedent, stated
above.

Stop: report the smoke-test readback result; await review before 123.12.

### 123.12 — Readback, golden storage, comparison, and tolerance policy

Scope: new test-support module for Phase 2 (candidate:
`freminal/tests/` or a dedicated `freminal-windowing` test helper), plus a
`Documents/` reference for the tolerance policy — this may be a section
appended to `Documents/PROFILING.md` rather than a new file, since
`PROFILING.md` already owns the profiling-methodology reference role; if a
new file is judged necessary, stop and propose it rather than creating it
unilaterally, per `no-summary-documents`.

What: capture the offscreen framebuffer to an image, store golden images
under a new `freminal/tests/golden_pixels/` (or equivalent) directory
mirroring the `UPDATE_GOLDEN=1` convention from
`freminal-terminal-emulator/tests/vttest_common.rs:14, 183-208`, and define
a **decided-up-front** comparison tolerance (e.g. per-pixel channel
difference bound plus an allowed-mismatched-pixel-count ceiling). The
tolerance number and its justification must be written down before the
first golden image is captured — not derived after a flaky run, which
`flaky-tests-are-bugs` forbids.

Deliverable: the capture/compare/regenerate mechanism, the documented
tolerance policy with its rationale, and at least one golden image for a
trivial synthetic scene (a single filled cell) as a proof of the mechanism.

Verification: run the comparison twice in a row against the same golden
image under `nix develop` and confirm bit-for-bit or within-tolerance
stability across runs on the same machine, before considering
cross-machine or cross-Mesa-version stability at all.

Prohibitions: do not gate any existing CI job on this yet — that is 123.13,
and only after a stability period. Do not choose a tolerance by running
the test until it passes; the tolerance must be justified independently of
any specific observed diff.

Stop: report the mechanism, the tolerance and its rationale, and the
stability-run results; await review before 123.13.

### 123.13 — New Nix-based CI job for Phase 2

Scope: a new job in `.github/workflows/nightly.yml` (following the existing
`ci` job's `nix develop --impure .#ci` pattern) or a new dedicated workflow
file if the maintainer prefers isolation from the nightly schedule — propose
both options rather than picking silently, since this is a new recurring CI
cost.

What: wire 123.10 through 123.12 into a job that runs under `xvfb-run`
inside the Nix `default` (or a new `gl-pixel`) dev shell, on the
`nightly`/manual-dispatch cadence that `bench.yml` already established for
similarly noise-sensitive checks, **not** on every push or PR.

Deliverable: the workflow diff, plus a documented decision on whether this
job blocks merges (default: it should not, until 123.12's stability period
has run for long enough to be trusted — the same caution `bench.yml` applies
to Criterion regressions).

Verification: at least one green run of the new job in CI, observed over
several runs before recommending it as a merge gate.

Prohibitions: do not add this job to the `test` matrix in `ci.yml` — that
matrix has no Nix and cannot host it (see the CI-shape finding above). Do
not make this job a required check without the stability period.

Stop: report the workflow diff and the observed run stability; await review
before 123.14.

### 123.14 — Quantified findings report

Scope: this document (a new "Findings" section appended once the work below
is done) and, if the maintainer requests it, an issue or discussion post —
no other file changes.

What: this is the task's actual product. Point the harness (Phase 1 for
call-count and state-change metrics; Phase 2 for pixel-level checks, once
it exists) at each of:

- genuine idle,
- pointer motion over inert content,
- pointer motion with a URL on screen,
- typing,
- full-screen TUI redraw, and
- the alt screen.

Report frame rate and per-frame cost **as a pair** for every workload, per
`PROFILING.md`'s reporting discipline — never a single CPU number, since
total CPU is the product of the two and a single figure cannot distinguish
"fewer frames" from "cheaper frames."

**This subtask's output is what defines Task 124's subtask list.** Note
explicitly, for the record, that Phase 1's metrics need no human sitting at
the machine reproducing a gesture, because they are deterministic given a
synthetic workload description — this is the harness's main advantage over
the manual method Task 121 used throughout, and is why the two diagnostic
obligations below should be the first things run once 123.8 lands, well
before Phase 2 is complete.

Deliverable: the Findings section, structured per workload with the metric
pairs, and an explicit verdict (CONFIRMED / REFUTED / INCONCLUSIVE, per
`PLAN_121_PERF_REMEDIATION.md`'s own verdict discipline) on each of the two
diagnostic obligations below.

Verification: no code change in the reporting half; any code fixes the
report's obligations require belong to Task 124, not here.

Prohibitions: do not report a number this task did not itself measure — no
borrowing pre-123 informal observations as if they were harness output. Do
not upgrade a finding beyond what was measured (e.g. "likely the cause" when
the harness only showed correlation).

Stop: report the Findings section in full; this is the task's closing
subtask.

---

## Findings (123.14, Phase 1)

> **Phase 1 only.** Every number here comes from the call-recording
> harness (123.7) or from Criterion benchmarks, on `task-123/gl-measurement-harness`.
> Phase 2's pixel-level contribution follows later and does not block this.
> Reference platform for byte volumes and timings: x86_64 Linux, dev shell,
> bundled CaskaydiaCove font at `pixels_per_point = 1.0`.

### How to read these numbers, and what is deliberately missing

`PROFILING.md` requires frame rate and per-frame cost to be reported **as a
pair**, because total CPU is their product and a single figure cannot
distinguish "fewer frames" from "cheaper frames".

Phase 1 measures **per-frame cost only**. The harness drives the renderer
directly, below the GUI event layer, so it has no opinion about how often a
frame is drawn. Frame rate is therefore reported here as **not measured**,
and no number below should be multiplied out into a "CPU during X" figure.
That is a limitation of the instrument, stated rather than papered over.

What Phase 1 does give, which the manual method Task 121 used throughout
could not, is **determinism**: given a synthetic frame description the call
log is byte-identical on every run, so a regression in per-frame cost is
caught in CI with nobody sitting at the machine reproducing a gesture.

### Per-workload GL cost, 80x24

| Workload | Calls | Draws | State changes | Uploads | Bytes |
| -------- | ----- | ----- | ------------- | ------- | ----- |
| One-time init | 261 | 0 | 30 | 2 | 96 |
| Full redraw, frame 1 | 53 | 2 | 21 | 5 | 4,394,272 |
| Full redraw, frame 2 | 104 | 2 | 21 | 30 | ~209,000 |
| Full redraw, steady | 52 | 2 | 21 | 4 | ~200,000 |
| Cursor-only, steady | ~48 | 2 | ~19 | ~2 | ~576 |
| Cursor hidden | 38 | 1 | 14 | 3 | 4,393,984 |
| Toast present | 121 | 4 | 41 | 10 | 4,658,448 |

Frame 1's 4.4 MB is the initial full glyph-atlas upload (a 1024x1024 RGBA
texture). Frame 2 is inflated by defect 124.9, below. "Steady" is frame 3
onward.

Workloads **not** in this table, because the harness cannot represent them
honestly:

- **Pointer motion** (both variants). Not a renderer workload — see the
  dedicated section below.
- **Alt screen.** The buffer layer produces different content; the renderer
  draws it identically. There is no renderer-level difference to report,
  and inventing one would be fabrication.

Two workloads from 123.14's list **are** covered, but by rows that do not
carry their name, so the mapping is stated explicitly rather than left to
be inferred:

- **Typing** maps to the "Full redraw, steady" row, and that is itself
  Obligation 1's finding: because a one-cell edit produces a new
  `visible_chars` `Arc`, a keystroke currently costs a *full* rebuild, not
  an incremental one. There is no cheaper typing row to report because the
  renderer has no cheaper typing path. If 124.1 lands, typing should move
  to something nearer the cursor-only row, and that migration is the
  measurable success criterion for 124.1.
- **Genuine idle** is, at the renderer, *no row at all* — an idle terminal
  draws no frame. The nearest thing with a cost is cursor blink, which is
  the "Cursor-only, steady" row at roughly two frames per second. Reporting
  idle as if it had a per-frame cost would misrepresent it; its cost is
  a frame *rate* question, and rate is not measured here.

### A disclosed gap in this accounting

The table above covers the `freminal` crate only, and that is a real
boundary rather than a complete picture. `freminal-windowing`'s
`GlState::clear` (`freminal-windowing/src/gl_context.rs:350-357`) issues a
raw `clear_color` + `clear` pair directly on its `glow::Context`, and
`egui_integration.rs` calls it on **every full-damage frame**. Those two
calls are in a different crate, so they are outside both 123.1's guard
(which walks the `freminal` crate) and the recording harness (which drives
the renderer, not the windowing layer).

So every real `FrameDamage::Full` frame in production issues **two more
calls — one more state change and one clear — than any row above shows**.
The omission does not move the headline findings: `clear`/`clear_color`
carry no bytes, so Obligation 1's bandwidth ratio is unaffected, and two
calls against a 52-call frame is under 4%. But a document whose purpose is
to be a trustworthy accounting instrument should not have an undisclosed
boundary, and 123.5's "zero raw `glow::Context` calls remaining anywhere in
the `freminal` crate" is true only because the crate boundary excludes this
site.

Extending the facade across the `freminal-windowing` boundary is a real
piece of work — it means either exporting `Gl` from a lower crate or
duplicating it — and is deliberately **not** done here, since Task 123
changes no behaviour and this is a scope question for the maintainer.
Recorded as an open question rather than silently absorbed.

### Verdicts on the two diagnostic obligations

#### Obligation 1 — the always-new-`Arc` finding: **CONFIRMED**

A re-flatten that produces byte-identical content in a freshly-allocated
`Arc` is reported as a content change and forces a full vertex rebuild.
Pinned by
`frame_dirty.rs::byte_identical_reflatten_in_a_new_arc_still_forces_a_full_rebuild`,
which isolates the single variable — same bytes, new allocation, with
`visible_line_widths`, theme, dimensions and fold epoch all held
pointer-identical. A paired control confirms that re-observing the *same*
`Arc` correctly reports no change, so the confirmation is not the
degenerate "`content_changed` is always true".

Upstream cause: `rows_as_tchars_and_tags_incremental`
(`freminal-buffer/src/buffer/flatten.rs`) returns the cached `Arc`s only on
its no-op path (`reuse_available` and no row in the window rebuilt). Both
the incremental fast path and the full-merge fallback end in
`Arc::new(...)` regardless of whether the merged bytes changed. One dirty
row anywhere in the window is enough.

**Cost of being wrong, quantified:** a needless full rebuild is ~52 GL
calls and ~200 KB of buffer uploads, against a cursor-only frame's ~48
calls and under a kilobyte. **The waste is overwhelmingly bandwidth, not
call count** — roughly 350x the bytes for roughly 1.08x the calls. Task
124.1 should be justified and measured on bandwidth, not on call counts.

This is a correction to the intuition in the obligation as written, which
framed the cost as "a full vertex rebuild and a full present every tick"
without distinguishing the two.

#### Obligation 2 — the pointer-motion full-present anomaly: **REFUTED** (as stated), explained by the toast

`pointer_forces_full_present` is **not** implicated. With the toast
confound removed, pointer motion over terminal content yields
`FrameDamage::Partial`; with a toast present and everything else identical,
`Full`. The toast alone accounts for the original `frame_damage_full=120,
frame_damage_partial=0` observation, which was recorded at the time with
`toast_active=48` firing in every run.

Pinned by
`frame_damage.rs::pointer_motion_over_content_is_partial_once_the_toast_confound_is_removed`,
with a companion test confirming motion **over chrome** does still force
`Full`, so the predicate is not dead code and the refutation is not an
artefact of it never firing.

This is the experiment the original observation needed and could not
perform: in a live session the startup toast expires on a timer while the
gesture is still in progress. As a function parameter it is trivially held
fixed. That is the clearest single demonstration of why deterministic
construction was worth building.

**Task 124.2 should be re-scoped or closed.** Its premise — that every
frame is a full present during pointer motion, caused by the pointer
predicate — does not survive measurement. What remains true is narrower:
*a visible toast* forces a full present on every frame for its lifetime,
which is correct behaviour for an animating overlay but worth knowing is
unconditional.

### Pointer motion, treated honestly

Pointer motion is freminal's worst interactive workload, and the reason is
**event rate**, not per-event cost. macOS delivers pointer motion even to
unfocused windows; Wayland is comparably chatty. Total CPU is per-event
cost multiplied by that rate.

The harness can measure the first term. It cannot observe the second, and
**the rate is left here as an explicit, unmeasured multiplier** rather than
folded into a single figure.

Per-event decision-path cost, from `pane_resolution_bench`:

| Stage | 1 pane | 4 panes | 16 panes | 16 deep (chain) |
| ----- | ------ | ------- | -------- | --------------- |
| `PaneTree::layout` | 33 ns | 85 ns | 362 ns | 423 ns |
| `pane_at_pos` (worst case) | 4 ns | 7 ns | 17 ns | — |

**The finding is that this is negligible, and that is itself the result.**
A whole pointer-motion decision costs tens to a few hundred nanoseconds.
Even at an aggressive 1 kHz event rate, 380 ns/event is under 0.04% of one
core. Optimising the predicate would buy nothing measurable.

Two things follow, and they matter for Task 124's shape:

1. **What costs money is whether a motion event causes a repaint**, not
   what the event handler computes. A repaint is ~52 GL calls and ~200 KB;
   the decision that avoids it is ~380 ns. That is a ratio of roughly five
   orders of magnitude in bytes moved. **Task 124.3 (cell-granular pointer
   suppression) is therefore the right lever, and 124.4 (the bool-to-struct
   readability fix) should not be expected to show any performance effect
   at all** — consistent with how 124.4 is already scoped.
2. **A minor, quantified inefficiency worth noting but not prioritising:**
   `FreminalGui::pointer_motion_needs_repaint` (`app_impl.rs:1072-1078`)
   calls `pane_tree.layout(central_rect)` on **every** pointer-motion event,
   which walks the tree and heap-allocates a fresh
   `Vec<(PaneId, Rect)>` each time. It dominates the per-event cost (10-25x
   `pane_at_pos`) and the layout only changes on resize or a split/close.
   Caching it is straightforward. At the measured magnitudes this is worth
   perhaps 0.03% of a core at high event rates, so it is recorded for
   completeness and explicitly **not** recommended as a priority.

Anyone reporting a pointer-motion CPU figure should state the observed
event rate alongside it. This document does not have one, and does not
guess.

### Other findings

- **GL call count is independent of terminal grid size.** An 8x2 grid and
  an 80x24 grid record byte-identical call sequences (53 calls, 2 draws, 21
  state changes, 5 uploads). Only instance count (17 vs 1921) and byte
  volume scale. This is the instanced renderer working as intended, and
  `call_count_is_independent_of_grid_size` now fails if it ever stops being
  true.
- **The cursor-only "fast path" is a bandwidth optimisation, not a
  call-count one.** ~48 calls against a full rebuild's ~52, the same two
  draw calls — but under a kilobyte against ~200 KB. Reasoning about it as
  "the cheap path" in call-count terms is mistaken, and any Task 124 work
  that trades calls for bytes should be evaluated on that basis.
- **One-time init is ~5x a steady frame** (261 calls vs 52). This is why
  `record_steady_state` discards init, and why per-frame figures measured
  without that separation would have been mostly setup.
- **Defect 124.9 (new).** `sync_atlas`'s full-upload branch never clears
  `GlyphAtlas::dirty_rects`, so every glyph rasterised before a full upload
  is redundantly re-uploaded one `tex_sub_image_2d` at a time on the next
  frame — 30 uploads against a steady-state 4, roughly doubling frame 2 of
  a first paint. Recurs on atlas growth, font change and `clear_atlas`.
  One-line fix, recorded in `PLAN_124_RENDER_EFFICIENCY.md`. Being a
  first-paint cost is exactly why eyeball profiling never found it.
- **Cleanup 123.C1 (new).** `decide_frame_damage`'s doc comment still
  describes a `force_full` term (`pointer_moving`) removed by #459 item 9.
  It states precisely the hypothesis Obligation 2 was testing, so a reader
  checking the docs would get a false confirmation.

### What this means for Task 124

123.14's stated purpose is to define Task 124's subtask list. Concretely:

- **124.1** — premise CONFIRMED, proceed. Justify and measure it on
  **bandwidth**, not call count.
- **124.2** — premise REFUTED. Re-scope to "a visible toast unconditionally
  forces a full present" or close it.
- **124.3** — unchanged, and now better supported: suppressing repaints is
  where the money is.
- **124.4** — unchanged; confirmed to have no expected performance effect.
- **124.9** — new, measured, one-line fix, not gated on further work.
- **123.C1** — new, documentation only.
- **Open question for the maintainer** — whether to extend the `Gl` facade
  across the `freminal-windowing` boundary so the two per-full-frame
  `clear`/`clear_color` calls are recorded too. Not a Task 124 subtask
  until that is decided; see the disclosed-gap note above.

Nothing above is upgraded beyond what was measured, and no pre-123 informal
observation is reported here as harness output.

---

## Two diagnostic obligations

These are restated here, in the language of what the harness must answer,
because they are Task 124's first two inputs and 123.14 must not close
without addressing them.

### Obligation 1 — confirm or refute the always-new-`Arc` finding (Task 124.1's premise)

`rows_as_tchars_and_tags_incremental` (`freminal-buffer/src/buffer/flatten.rs:530-533`
and `:569-572`) wraps the merge result in a **brand-new `Arc` whenever any
row is dirty, even when the merged bytes are byte-identical**; only the true
no-op path (`:454-479`) returns the same `Arc`.
`evaluate_frame_dirty_state` (`freminal/src/gui/terminal/frame_dirty.rs:301-311`)
then tests `Arc::ptr_eq`, so a byte-identical re-flatten sets
`content_changed`, forcing `ReevaluateFullRebuild`, then
`PaneFrameDamage::Full`, then `FrameDamage::Full`. The module doc at
`frame_dirty.rs:270-281` already acknowledges this for cursor blink.

**Hypothesis to test with the Phase 1 harness:** full-screen-redraw
workloads pay a full vertex rebuild and a full present every tick,
regardless of whether anything visibly changed. 123.8's full-screen-TUI
workload assertion is the direct test; 123.14 must report the draw-call and
state-change counts for that workload against a control workload where the
content is genuinely static, and state a verdict.

### Obligation 2 — diagnose the 121.31 full-present-during-pointer-motion anomaly

Migrated here from `PLAN_121_PERF_REMEDIATION.md` subtask 121.31 as a
measurement obligation rather than a code fix (the fix, once diagnosed,
belongs to Task 124.2). `frame_damage_full=120, frame_damage_partial=0` was
observed during pointer motion versus `120/120` *partial* at idle, and was
never diagnosed. `pointer_forces_full_present`
(`freminal/src/gui/app_impl.rs:117-123`) is
`pointer_moving && (pointer_over_chrome || border_drag_active)` and should
not fire for motion over terminal content.

**Confound recorded at the time of the original observation:**
`toast_active=48` fired in every run (a startup toast present at launch),
and `toast_active` is a separate short-circuit in `decide_frame_damage`
(`freminal/src/gui/frame_damage.rs:86-88`) — so the 120/0 split may have
been explained entirely by the toast, not by pointer motion at all.

**What 123 must do:** re-run the equivalent workload with no startup toast
present, using the Phase 1 harness's deterministic workload construction so
the toast variable can be held fixed across runs rather than relying on
timing. Report the full/partial split with and without a toast present, and
state a verdict on whether `pointer_forces_full_present` itself is
implicated once the toast confound is removed.

---

## Cleanup entries surfaced during Task 123

Per `agent-orchestration-protocol`, a pre-existing bug found mid-task
becomes a numbered entry rather than an inline fix or a chat message.

### 123.C1 — `decide_frame_damage`'s doc comment describes a `force_full` term that no longer exists

Surfaced by 123.8 while discharging Obligation 2.

`freminal/src/gui/frame_damage.rs`'s doc comment on `decide_frame_damage`
states that the caller computes `force_full` as
`ui_overlay_open || shader_recomposites || active_pane_changed ||
pointer_moving`. The bare `pointer_moving` term has not been there since
issue #459 item 9: `freminal/src/gui/app_impl.rs:285-292` computes it as
`ui_overlay_open || shader_recomposites || active_pane_changed ||
pointer_forces_full_present(pointer_moving, pointer_over_chrome,
border_drag_active)`.

This matters more than a typo would. The stale comment states exactly the
behaviour Obligation 2 set out to investigate — "pointer motion forces a
full present" — so a reader checking the hypothesis against the docs would
have it confirmed by a comment that is wrong, and the code refutes it.

Scope of fix: correct the doc comment to name
`pointer_forces_full_present` and its three inputs. Documentation only, no
behaviour change, no test change. Verification: `cargo test --all` plus the
standard suite; the two Obligation 2 tests added in 123.8 already pin the
real behaviour.

Not fixed in Task 123 because Task 123 changes no behaviour and touches no
file it is not measuring; this is a one-line docs correction better carried
with Task 124's `frame_damage.rs` work (124.2/124.4 both touch this area).

---

## Also record

- Task 123 changes no rendering behaviour — instrumentation and measurement
  only, end to end.
- Task 121 left subtask 121.20 (GPU buffer-orphaning for small payloads)
  open specifically because it needed a pixel harness to be exercised
  safely. **Phase 2 of this task is that harness**, so 123 unblocks 121.20,
  and the fix itself is carried forward as Task 124.7.

---

## Verification

Standard for every subtask, per `agents.md`:

1. `cargo test --all`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo machete`

Additionally, per the pre-commit hook divergence recorded in
`PLAN_122_ORCHESTRATION_EXTRACTION.md`, also run
`cargo clippy --workspace --all-targets` and treat it as the primary gate,
since it is what pre-commit actually enforces.

Every Phase 1 subtask that touches the recording feature (123.2 through
123.5) must also pass `cargo test --all --features gl-recording`.

`cargo xtask check-windows` runs once before any PR touching `flake.nix` or
CI workflow files, per `freminal-windows-crosscheck`.

123.6 additionally requires a before/after capture per
`performance-benchmarks`, coordinated with `freminal-bench-table`.

---

## References

- `Documents/PLAN_124_RENDER_EFFICIENCY.md` — the remediation task this
  harness unblocks. 123 measures; 124 fixes.
- `Documents/PLAN_121_PERF_REMEDIATION.md` — carries 121.20 (blocked on
  Phase 2), 121.25 (measurement debt absorbed here), 121.28 (the harness
  this task supersedes), and 121.31 (migrated here as Obligation 2), plus
  the CONFIRMED/REFUTED verdict discipline this task's 123.14 follows.
- `Documents/PLAN_122_ORCHESTRATION_EXTRACTION.md` — style template for
  this document's subtask structure, and the source of the
  documented-clippy-versus-pre-commit-hook divergence noted in
  "Verification" above.
- `Documents/PROFILING.md` — the frame-rate-plus-per-frame-cost reporting
  discipline 123.14 follows, and the source of the "no headless-GL or
  pixel-readback harness exists" gap this task closes.
- `Documents/DECOUPLING_FRAMEWORK.md` — the decision record for whether
  freminal should stop using egui for the main window; unaffected by this
  task, but the source of the profiling-methodology precedent
  `PROFILING.md` itself cites.
- `agents.md` — `freminal-bench-table`, `freminal-numeric-conversions`,
  `freminal-module-cohesion`, `flake-dev-shell-discipline`, and
  `flaky-tests-are-bugs`, all directly load-bearing for this task's
  subtasks.
- Issue #440 — the missing pixel / headless-GL harness this task closes.
- Issue #459 — the profiling findings whose refutation rate motivates this
  task's existence.
</content>
