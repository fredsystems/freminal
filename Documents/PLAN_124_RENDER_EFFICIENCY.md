# PLAN_124_RENDER_EFFICIENCY.md — Task 124 "Render Efficiency Remediation"

> **STATUS: STUB — awaiting Task 123's findings for most subtasks.** Several
> entries below are already well-specified and can be written out fully
> because their premise was established during Task 121's investigation
> rather than left as an open question. No subtask is implemented from this
> document until stated otherwise — see "The governing rule" below for the
> one exception.

Task 124 is carried by v0.12.0. See `Documents/PLAN_VERSION_120.md` for the
version summary and `Documents/MASTER_PLAN.md` for roadmap position.

---

## Relationship to Task 123

**Task 124 depends on Task 123** (the GL measurement harness — see
`Documents/PLAN_123_GL_MEASUREMENT_HARNESS.md`). 123 measures; 124 fixes.

### The governing rule

**No subtask in this document is implemented until Task 123 has quantified
the thing it claims to fix.** This is inherited directly from Task 121's
outcome: that task closed with four of its six issue #459 candidate items
**refuted by their own verification step** (121.18, 121.19, 121.21, 121.22
— see `PLAN_121_PERF_REMEDIATION.md`). A plausible finding derived from
static reading or a code comment is a hypothesis, not a work item, in this
codebase specifically — `DECOUPLING_FRAMEWORK.md` §12 records the same
lesson from PR #461, and Task 121's Group G (121.32) records it a second
time for the chrome cache, where three code-reading hypotheses were
falsified in sequence before the actual cause was found by measurement.
Task 124 exists to avoid repeating that pattern rather than to skip past it.

**One subtask is explicitly exempt: 124.4.** It is a readability and
state-representation fix with no expected performance effect, so there is
nothing for Task 123 to quantify before it lands.

### The headline problem statement

The maintainer's observation, which this task exists to close: on a slow
laptop, wezterm and ghostty essentially never rise above 0.0% CPU, while
freminal measurably does **when the mouse is moving**. Two suspected
classes:

- full-frame re-render on pointer motion, and
- draw calls issued more expensively than necessary.

Keystrokes are the lesser concern — they do not arrive faster than the
60 Hz render target — but should not be wasteful either.

---

## Subtask summary

| Subtask | Title                                                      | Gated on 123 |
| ------- | ----------------------------------------------------------| ------------ |
| 124.1   | Dirty-row `Arc` churn forces a full rebuild on byte-identical content | Yes |
| 124.2   | Every frame is a full present during pointer motion        | Yes (diagnosis) |
| 124.3   | Cell-granular pointer suppression                          | Yes (re-measurement) |
| 124.4   | Replace the six positional bools in the pointer-motion predicate | No |
| 124.5   | Decide and execute the chrome cache's fate                 | Yes |
| 124.6   | Shaping-path levers                                         | Yes |
| 124.7   | GPU buffer-orphaning for small payloads                     | Yes (Phase 2 harness) |
| 124.8   | `DESIGN_DECISIONS.md` entry for the Phase 0 / Task 121 outcome | No |
| 124.9   | `sync_atlas` re-uploads glyphs a full atlas upload already covered | No (already measured) |

Subtask numbers are stable once assigned, matching the convention
`PLAN_121_PERF_REMEDIATION.md` established: a withdrawn or absorbed
predecessor subtask keeps its number and records why, rather than being
silently deleted from the historical record.

---

## Subtasks

### 124.1 — Dirty-row `Arc` churn forces a full rebuild on byte-identical content

**THE leading candidate.**

`rows_as_tchars_and_tags_incremental`'s incremental path
(`freminal-buffer/src/buffer/flatten.rs:530-533`) and its full-merge
fallback (`:569-572`) both wrap their result in a **brand-new `Arc`
whenever any row is dirty — even when the merged bytes are byte-identical
to the previous merge**. Only the true no-op path (`:454-479`, taken when
`boundary.is_none()`) returns the same `Arc`.

`evaluate_frame_dirty_state` (`freminal/src/gui/terminal/frame_dirty.rs:301-311`)
derives `content_changed` from `Arc::ptr_eq` over `visible_chars` /
`visible_line_widths`, so a byte-identical re-flatten reads as changed. That
forces `VertexRebuild::ReevaluateFullRebuild` (`frame_dirty.rs:574-586`),
which sets `PaneFrameDamage::Full` (`freminal/src/gui/terminal/widget.rs:2585-2602`),
which `decide_frame_damage` (`freminal/src/gui/frame_damage.rs:78-118`) turns
into `FrameDamage::Full`. The module doc at `frame_dirty.rs:270-281` already
acknowledges the cursor-blink instance of this pattern.

**Consequence:** any workload that dirties rows every tick pays a full
vertex rebuild and a full present every tick, whether or not a pixel
changed. Full-screen TUIs redraw everything by idiom, so they hit it
constantly.

**This is workload-correlated, not alt-screen-keyed.** A verified recon
(2026-08-20) found no alt-screen-specific bypass anywhere; every branch on
`is_alternate_screen` in the render path *suppresses* work instead (the
gutter and command-block features at `freminal/src/gui/pointer_motion.rs:143-146`
and `widget.rs:2022,2072-2079,2120,3216,3287`). `watch`, or `htop` run
without an alternate screen, on the primary screen would behave identically.

**Candidate fix directions — do not pick one; 123 measures first:**

- content-hash the merged bytes before allocating a new `Arc`,
- return the previous `Arc` when the merge is byte-identical, or
- propagate real per-row dirty information across the snapshot boundary
  instead of one global bit.

Task 121's closed subtask 121.18 independently arrived at "better dirty
granularity at the snapshot level" as the cheaper alternative to its own
proposed redesign of the vertex-instance layer — that recommendation lands
here.

### 124.2 — Every frame is a full present during pointer motion

*Migrated from 121.31.*

Observed `frame_damage_full=120, frame_damage_partial=0` during pointer
motion versus `120/120` *partial* at idle; `swap_mean_us=210` was 29% of the
clean run's 729 µs frame. Never diagnosed.

`pointer_forces_full_present` (`freminal/src/gui/app_impl.rs:117-123`) is
`pointer_moving && (pointer_over_chrome || border_drag_active)` and should
not fire for motion over terminal content.

**Confound recorded at the time:** `toast_active=48` fired in every run (a
startup toast), and `toast_active` is a separate short-circuit in
`decide_frame_damage` (`frame_damage.rs:86-88`).

**Diagnosis belongs to Task 123; the fix belongs here.** Note this is the
issue #435 partial-present mechanism, entirely independent of the
issue #436 chrome cache — do not conflate the two when investigating.

### 124.3 — Cell-granular pointer suppression

*Migrated from 121.15 + 121.17.*

**The highest-value scheduling item.** Nearly all interactive terminal
state changes at **cell** granularity — URL hover, gutter hover, selection
extent, mouse-tracking reports — so pointer motion within one cell cannot
change any of it. Caching the pane terminal-rect origin and logical cell
size and suppressing any `CursorMoved` that does not cross a cell boundary
would:

- remove the pane-wide `has_urls` and `scroll_offset` vetoes (121.15),
- let selection drags suppress,
- subsume the gutter carve-out, and
- be correct for mouse-tracking mode.

**The scrollbar must stay excluded** — thumb dragging is genuinely
pixel-granular.

**Measured stakes (harness, 2026-07-29) — stale, must be re-taken by 123:**

| Scenario                 | checks | suppressed | veto firing                     |
| ------------------------ | ------ | ---------- | -------------------------------- |
| Clean pane                | 15,265 | 99.16%     | `overlay_open` 126 (0.8%)        |
| One OSC 8 URL on screen  | 792    | 1.68%      | `has_urls` 792 (100%)            |
| btop                      | 217    | 0%         | `mouse_tracking_active` 216 (99.5%) |

A single hyperlink takes suppression from 99.16% to 1.68% — total defeat —
sustaining 61.4 fps at 185 µs/frame, about 1.1% of a core against ~0.06%
for the clean pane. Roughly 20x. **This is very likely the direct cause of
the maintainer's mouse-movement symptom.**

A CPU meter cannot see it: a ~1.1% burst over a few hundred ms averages to
0.1-0.2% over a typical sampling window — this is the same reporting-blind
spot `PROFILING.md` warns about elsewhere in the codebase.

The Task 122 seam it once waited on exists: subtask 122.15 publishes
`pane_terminal_origin(pane_id)`, and the reader currently carries an
`#[allow(dead_code)]` with a TODO naming this work — **remove that allow
when landing**.

The old numbers above predate the chrome cache being disabled (121.32) and
must be re-measured by Task 123 before any design is committed to.

### 124.4 — Replace the six positional bools in the pointer-motion predicate

*Migrated from 121.29's surviving residue.*

`pointer_motion_needs_repaint_decision` (`freminal/src/gui/pointer_motion.rs:232-247`)
takes `focus_change_pending`, `chrome_interactive`, `any_pane_selecting`,
`overlay_open` and `pointer_pane_unresolved` as positional bools, plus
`pane_signals: Option<PointerMotionPaneSignals>` bundling two more.

PR #496 flagged this in both the PR body and commit `b17c5709`'s message
and deferred it deliberately: "a real hazard... the signature wants a
named-field input struct... it is Task 121/122's own surface and does not
belong in a bug fix." Per `freminal-state-representation`, bool
*parameters* are forbidden outright.

**Scope:** a named-field input struct.

Note this is a readability/safety fix with no expected performance effect,
so it is **not** gated on Task 123.

**Record clearly that 121.29's actual proposal — an unbounded
suppressed-pointer fallback driven by `Context::repaint_causes()` — is NOT
migrated.** It was investigated and rejected in Task 121 on the grounds
that it depends on five egui internals, two of which are present-day
holes, for a measured prize of ~0.075% of a core. Do not re-derive it.

### 124.5 — Decide and execute the chrome cache's fate

*Migrated from 121.34, absorbing 121.30, 121.33, 121.35, 121.36.*

The #436 chrome cache is **disabled by default** since 121.32
(`chrome_cache_enabled()` in `egui_integration.rs`; `FREMINAL_CHROME_CACHE=1`
re-enables it) because it is structurally unsound: `ChromeMode::Replay`
skips *constructing* chrome widgets, and egui resolves hit-testing and
click validity against the **previous frame's** widget set, so unbuilt
widgets are uninteractable. That shipped as a tab-click and
pane-border-drag regression in 0.12.0-beta.7.

The maintainer's current position is that Group F was "mostly predicated
on the chrome cache being useful", and deletion is actively under
consideration. Only two sound designs exist:

- cache the *output* while still constructing the widgets, or
- delete the machinery.

**Recommend deletion unless Task 123 shows a material, concentrated
saving.**

If deleted, the following go with it: `ChromeCache`, `ChromeGatePredicates`,
`evaluate_chrome_gate`, the `gate_blocked_*` counters, the reverted 121.13,
121.14's chrome half, and subtasks 121.30, 121.33, 121.35 and 121.36 all
resolve as moot.

Note 121.35's live waste as the case for urgency: while disabled, the
`Full` arm still populates the cache every frame — six vector clones per
frame to fill a cache nothing reads.

### 124.6 — Shaping-path levers

*Migrated from 121.19's surviving alternatives.*

121.19's ASCII fast path was closed because ASCII does not imply "cannot
ligate" (`->`, `=>`, `!=` are exactly what ligature substitution targets),
so the only safe gate is `ligatures == false` — and `FontConfig::default`
sets `ligatures: true` (`freminal-common/src/config.rs:122`, pinned by a
test at `:2206`), making it dead code for default-config users.

Two surviving levers:

- a content-addressed **run-level** shaping cache keyed on `(face_id,
  ligatures, run text)`. Today's `ShapingCache` (`freminal/src/gui/shaping.rs:127`)
  is keyed by **line index**, so it cannot hit across a scroll, and one
  changed character re-shapes every run on the row.
- per-run allocation reduction in `build_shaped_glyphs` (`shaping.rs:701-802`),
  which builds four `Vec`s per run per cache miss.

Gated on Task 123 producing shaping cache hit/miss instrumentation and a
benchmark modelling full-screen TUI redraw — neither exists today.

### 124.7 — GPU buffer-orphaning for small payloads

*Migrated from 121.20.*

`upload_verts` (`freminal/src/gui/renderer/gpu.rs:1665-1682`) orphans
unconditionally with no size gate; the idle `deco_verts` floor is the
cursor quad alone, `CURSOR_QUAD_FLOATS = 36` (`vertex.rs:149`) = 144 bytes.

Task 121 left this open because it needed a pixel harness to be safe.
**Task 123 Phase 2 is that harness, so 124.7 is gated on it.**

Carry over the corrected #432 analysis: commit `c76ae8d1`'s primary fix
was CPU-side offset bookkeeping, and the orphan arrived as explicitly
secondary hardening ("Also hardens the cursor-only GPU fast path found
while investigating") — so the risk is smaller than `gpu.rs`'s own comment
implies, but the double-buffer-without-orphan counterfactual was never
isolated and the failure mode is silent visual corruption.

### 124.8 — `DESIGN_DECISIONS.md` entry for the Phase 0 / Task 121 outcome

*Migrated from 121.27.*

Must record the direction **and** the inconvenient numbers, including that
Phase 0 weakened rather than strengthened the case for the egui rewrite,
and that Task 121 closed with four of six candidate items refuted.

### 124.9 — `sync_atlas` re-uploads glyphs a full atlas upload already covered

*Surfaced by Task 123 subtask 123.8 on `task-123/gl-measurement-harness`.
Unlike the rest of this document, this subtask is **not gated on further
measurement** — 123.8 already measured it, and the test that measured it is
committed.*

**The defect.** `TerminalRenderer::sync_atlas`
(`freminal/src/gui/renderer/gpu.rs`) branches on
`GlyphAtlas::needs_full_reupload()`. The full-upload arm issues one
`tex_image_2d` covering the entire atlas — but it never clears
`GlyphAtlas::dirty_rects`. Only the delta arm consumes them, via
`take_dirty_rects()`. Every glyph rasterised *before* a full upload
therefore stays queued, and the next frame re-uploads each one
individually with `tex_sub_image_2d`, despite the full upload having
already contained all of them.

**Impact, measured.** On an 80x24 first paint, frame 2 issues **30 upload
calls against a steady-state 4**, roughly doubling that frame's total GL
call count (104 versus ~52). It recurs on every event that sets
`full_reupload`: atlas growth, a font or font-size change, and
`RenderState::clear_atlas`. It is a first-paint and
post-font-change cost, not a steady-state one, which is precisely why
eyeball profiling never caught it.

**Scope of fix.** One line: consume the queued rects in the full-upload
arm, e.g. `drop(atlas.take_dirty_rects());` immediately after the
`tex_image_2d` call. The rects are redundant by construction at that point
— the full upload wrote the whole texture, including every region they
describe.

**Verification.**
`freminal/src/gui/renderer/headless_workloads.rs::a_full_atlas_upload_leaves_stale_dirty_rects_to_re_upload`
currently asserts the **buggy** behaviour deliberately, so the defect stays
pinned and measurable until it is fixed. That test must be **inverted, not
deleted**, when this subtask lands: after the fix, frame 2's upload count
must be comparable to steady state rather than several times it. Deleting
it would discard the only regression guard for this behaviour.

**Why it is a Task 124 entry and not a Task 123 fix.** Task 123 measures;
Task 124 fixes. The maintainer confirmed this split explicitly when the
defect surfaced.

---

## What did not migrate from Task 121, and why

- **121.29's `repaint_causes()` proposal** — rejected. Depends on five
  egui internals, two of which are present-day holes, for a measured prize
  of ~0.075% of a core. See 124.4's closing note.
- **121.30** — dies with the chrome cache. Its residual risk (chrome
  widgets not constructed on `Replay`, so an egui-internal chrome animation
  would freeze rather than degrade) is moot once the mechanism it concerns
  is deleted or redesigned per 124.5.
- **121.18 as originally framed** — a redesign of the CPU-side
  vertex-instance representation, not a subtask; its useful residue (better
  dirty granularity at the snapshot level) is 124.1.
- **121.19's ASCII gate** — inert at default config, since
  `FontConfig::default` sets `ligatures: true`. Its two surviving levers
  are 124.6.
- **121.21 and 121.22** — no freminal-side lever. 121.21's compute-shader
  clear is Mesa radeonsi's own internal fast-clear path for a `glClear`
  freminal already issues in the most basic fixed-function form possible;
  121.22's `wayland_client_handle` call site is already maximally hoisted
  and freminal owns no repeated fetch to eliminate.
- **121.24** — measured and refuted. The two heap allocations in
  `pointer_motion_needs_repaint` (`iter_panes()` and `pane_tree.layout()`)
  were benchmarked at ~1.4% of the residual they were proposed to explain;
  building a scratch buffer for them is not warranted.

---

## Verification

Standard for every subtask, per `agents.md`:

1. `cargo test --all`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo machete`
4. `cargo xtask check-windows` before any PR, per `freminal-windows-crosscheck`

Every performance subtask (124.1, 124.2, 124.3, 124.5, 124.6, 124.7)
additionally requires a before/after capture per `performance-benchmarks`
and the freminal-specific catalog in `freminal-bench-table`. 124.4 and
124.8 are documentation/readability-only and carry no performance capture
requirement.

---

## References

- `Documents/PLAN_123_GL_MEASUREMENT_HARNESS.md` — the measurement harness
  this task depends on.
- `Documents/PLAN_121_PERF_REMEDIATION.md` — the full breakdown this
  document migrates surviving work out of; carries the CONFIRMED/REFUTED
  verdicts and dated corrections referenced throughout.
- `Documents/DECOUPLING_FRAMEWORK.md` — the decision record for whether
  freminal should stop using egui for the main window; §2A is the source
  of truth for the Phase 0 measurements this task's 124.8 records.
- `Documents/PROFILING.md` — the profiling methodology reference,
  including the frame-rate-plus-per-frame-cost reporting discipline.
- Issue #405 — the earlier idle-CPU investigation Task 121 pivoted from.
- Issue #432 — the silent visual corruption bug class 124.7 shares.
- Issue #435, issue #436 — partial present and chrome caching, both
  closed; relevant to 124.2 and 124.5 respectively.
- Issue #440 — the missing pixel / headless-GL harness, the gap Task 123
  closes.
- Issue #457 — `merge_cache` structural shift, still open, deprioritized
  by Task 121's 121.1.
- Issue #459 — the profiling findings and the candidate list Task 121's
  Group D drained.
