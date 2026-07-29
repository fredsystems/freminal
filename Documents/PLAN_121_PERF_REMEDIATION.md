# PLAN_121_PERF_REMEDIATION.md — Task 121 "Performance Remediation"

> **STATUS: IN PROGRESS.** Eleven subtasks have merged to `main` across five pull
> requests (#458, #460, #461, #464, #465). The remainder — five known bugs, one
> unifying improvement, seven unactioned issue #459 items, and four pieces of
> measurement debt — are outstanding and unscheduled.

Task 121 is carried by v0.12.0. The version-level summary lives in
`PLAN_VERSION_120.md` ("Task 121 — Performance Remediation"); this document is the
full breakdown.

---

## Goal

Reduce freminal's real-workload CPU cost — idle, pointer motion, typing, and
full-screen TUI redraw — to the level set by wezterm and ghostty on the same
hardware, and close the tracking gap left by an investigation that ran for five
merged pull requests under a task number that existed only in branch names.

Task 121 is the umbrella for **all** performance remediation arising from GitHub
issue #459 (real-workload CPU profiling findings, still open):

1. the work that has already landed,
2. the bugs that work surfaced but did not fix, and
3. the issue #459 candidate items nobody has actioned.

It is deliberately an umbrella, not a single deliverable. Subtasks are independent
and are scheduled individually.

---

## Relationship to `DECOUPLING_FRAMEWORK.md`

These are two different documents doing two different jobs, and conflating them has
already caused confusion.

- **`Documents/DECOUPLING_FRAMEWORK.md`** is the **decision record** for the
  question "should freminal stop using egui for the main window?", plus the
  rewrite-if-chosen plan (its Phases 1–5). Its status is **reopened, leaning
  against the rewrite, explicitly undecided** — Phase 0 measurement found three
  cheap fixes inside egui that recovered most of the benefit the rewrite was meant
  to deliver, which weakened rather than strengthened the case. It is **not** a
  `PLAN_VERSION_*.md`, and its Phases 1–5 are **not** tasks in `MASTER_PLAN.md`.
- **This document** is the **work tracker** for the performance remediation itself:
  what landed, what broke, what is left. It exists independently of whichever way
  the rewrite decision eventually falls.

`DECOUPLING_FRAMEWORK.md` §2A ("Phase 0 results — why the direction reopened") is
the **source of truth** for the measurements, the three findings, and the known
gaps in the Finding 3 spike. Do not re-derive those numbers here and do not
contradict them. This document records the numbers only where they are needed to
identify a subtask, and points at §2A for the reasoning.

One overlap is real and intentional: `DECOUPLING_FRAMEWORK.md` §8 Phase 0 is the
measurement phase of that decision, and several of its loose ends are tracked here
as Task 121 subtasks (121.27 is Phase 0.5 verbatim; 121.25 is Phase 0.2's
outstanding half).

**Phase 1 (orchestration extraction) is now Task 122**, tracked in `MASTER_PLAN.md`
with `DECOUPLING_FRAMEWORK.md` §8 Phase 1 as its plan content. It is required under
any outcome of the rewrite decision, so it is a task in its own right rather than a
rewrite prerequisite. It is **not** a blocker for most of Task 121: Groups D and E
live in `shaping.rs`, `vertex.rs`, `atlas.rs`, the GL layer and `freminal-windowing`,
which Task 122 does not touch, and 121.12–121.15 sit in already-extracted predicates.
The one real dependency is **121.17**, which needs the orchestration seam Task 122
creates — see that subtask.

---

## Subtask summary

| Group                                   | Subtasks      | Status      |
| --------------------------------------- | ------------- | ----------- |
| A — Completed work (merged to `main`)   | 121.1–121.11  | Complete    |
| B — Bugs found but unfixed              | 121.12–121.15 | Not started |
| B — Withdrawn                           | 121.16        | Withdrawn   |
| C — Unifying improvement                | 121.17        | Not started |
| D — Unactioned issue #459 items         | 121.18–121.24 | Not started |
| E — Measurement debt                    | 121.25–121.28 | Not started |

Subtask numbers are stable once assigned. A withdrawn or dissolved subtask keeps its
number and records why (the convention Task 118 used for 118.10), so the decision is
not re-litigated by someone reading the source document that prompted it.

---

## Group A — Completed work

All eleven subtasks below are merged to `main`. Every SHA and PR number here was
verified against the repository. Do not redo any of this work.

| #      | Subtask                                                  | Landed as        |
| ------ | -------------------------------------------------------- | ---------------- |
| 121.1  | Batched LF+flatten benchmark at scrollback capacity      | `76dd67f9`       |
| 121.2  | `merge_cache` invalidation on scrollback-capacity drain  | PR #458          |
| 121.3  | `FaceId` font caches to `FxHashMap`                      | PR #460          |
| 121.4  | Command-block gutter hover repaint over-firing           | PR #461          |
| 121.5  | Region-gate pointer-move full-present; skip no-op PTY repaint | PR #464     |
| 121.6  | Frame-profiling harness                                  | `0620cc60`       |
| 121.7  | Replay gate-blocker + per-signal instrumentation         | `436a54f1`       |
| 121.8  | `RedrawRequested` disqualified `ChromeMode::Replay`      | `7d483998`       |
| 121.9  | egui repaint-cause instrumentation                       | `ab88d0f5`       |
| 121.10 | Pointer-frame suppression + repaint-delay override       | `19780e16`       |
| 121.11 | CodeRabbit remediation on PR #465                        | `f762ca02`       |

Subtasks 121.6 through 121.11 all shipped in PR #465.

### 121.1 — Batched LF+flatten benchmark at scrollback capacity

Commit `76dd67f9` (merged directly, no PR). Added
`bench_lf_batch_then_flatten_at_capacity` to
`freminal-buffer/benches/buffer_row_bench.rs`, measuring K line feeds followed by
one flatten — the shape `build_snapshot` is actually driven with (one publish per
PTY-read batch, not one per line).

This is the measurement that **pivoted the whole investigation**: it showed the
`merge_cache` rotation cost amortizes to roughly 1.5% of the realistic per-batch
total at K=512, which is what redirected effort from issue #457's synthetic
structural fix to profiling the running application (issue #459).

### 121.2 — `merge_cache` invalidation on scrollback-capacity drain

PR #458, commit `b1a904dd`. A correctness bug (issue #405) found while building the
benchmark above: `merge_cache` was not invalidated when the scrollback drained at
capacity. Fixed on a `task-121/` branch, hence its membership here.

### 121.3 — `FaceId` font caches to `FxHashMap`

PR #460, commit `f4cd9747`. Issue #459 item 1. The `FaceId`-keyed `FontManager`
caches used the default SipHash hasher on a hot internal cache key; the glyph atlas
already used `FxBuildHasher` for the same class of map. Per `DECOUPLING_FRAMEWORK.md`
§2A this was the largest single CPU win prior to the PR #465 work.

### 121.4 — Command-block gutter hover repaint over-firing

PR #461, commit `555a8b5b`. Merged under the name of issue #459 item 2. It did
**not** in fact fix item 2 — 121.8 did. Record this honestly: the first fix attempt
was caught in adversarial review as BLOCKING because it depended on
`Response::hovered()` lagging `PointerState::latest_pos()` by one frame, an egui
internal. See `DECOUPLING_FRAMEWORK.md` §1 and §12 — PR #461 is the canonical
example in this repo of validation deferred and never performed.

### 121.5 — Region-gate pointer-move full-present; skip no-op PTY repaint

PR #464, commits `a9fb1210` and `cb1d5155`. Two fixes:

1. Replaced the unconditional `pointer_moving` term in `app_impl.rs`'s `force_full`
   with a region-aware test. Roughly 15% fewer samples under continuous pointer
   motion over terminal content. It did **not** move the btop idle-mouse floor —
   that floor is set by the frame-scheduling rate, not per-frame cost.
2. Gated the PTY consumer thread's unconditional `request_repaint_after(16ms)`
   behind a pure, exhaustive, unit-tested `input_event_needs_repaint` classifier.
   `InputEvent::Key` / `FocusChange` only write bytes to the child fd and mutate no
   emulator state, so they were producing a second, redundant GUI wake per
   keystroke. This is the fix that moved CPU during typing on the laptop.

### 121.6 — Frame-profiling harness

Commit `0620cc60`. A feature-gated (`frame-profiling`, non-default) harness
instrumenting the drawn frame path: `run_frame` wraps `run_ui` wraps `App::update`
wraps `central_body` wraps per-pane `show()`. Produced the §2 idle and active
breakdowns in `DECOUPLING_FRAMEWORK.md`.

### 121.7 — Replay gate-blocker + per-signal instrumentation

Commit `436a54f1`. Instrumented which signal was disqualifying `ChromeMode::Replay`
on each frame, rather than guessing. This is how Finding 1 was isolated — and it is
also how the *previous* hypothesis (the two `request_repaint_after` call sites in
`terminal/widget.rs`) was proven wrong at zero cost. See `DECOUPLING_FRAMEWORK.md`
§8 subtask 0.1.

### 121.8 — `RedrawRequested` permanently disqualified `ChromeMode::Replay`

Commit `7d483998`. **The highest-value fix of the whole investigation.** Every frame
is driven by `WindowEvent::RedrawRequested`; `egui-winit` returns `repaint: true`
for it; the chrome-input gate consumed that as evidence of real input; and the same
event's handler read the flag back via `std::mem::take` about 110 lines later in the
same call. The event that drove the frame disqualified the frame — unconditionally,
not as a race. The entire issue #436 chrome-cache subsystem had therefore been inert
since the day it landed.

A one-line carve-out plus regression tests fixed it. Steady-state idle:

| Metric               | Before  | After   | Change |
| -------------------- | ------- | ------- | ------ |
| `Replay` duty cycle  | 0%      | 100%    | —      |
| chrome construction  | 69 us   | 10 us   | -86%   |
| freminal's own       | 96 us   | 42 us   | -56%   |
| total per idle frame | 434 us  | 376 us  | -13.4% |
| partial present      | 115/120 | 120/120 | —      |

This closes issue #459 item 2, which PR #461 (121.4) was merged under but never
validated.

### 121.9 — egui repaint-cause instrumentation

Commit `ab88d0f5`. Named the culprit behind Finding 2 exactly:
`egui-0.35.0/src/context.rs` `begin_pass` calling
`InputState::wants_repaint_after()`, which returns `Duration::ZERO` whenever
`!self.events.is_empty()`. Zero freminal call sites appeared in the causes, which
excluded the gutter, scrollbar and cursor-trail hypotheses by measurement rather
than by argument.

### 121.10 — Pointer-frame suppression + repaint-delay override (spike)

Commit `19780e16`. Explicitly labelled a **spike**. Suppressing the input side alone
achieves nothing (measured: 99.99% of pointer events suppressed, frame rate
unchanged at 61fps) because suppressed events still reach `on_window_event` and
egui re-arms from inside the frame. Overriding `frame_output.repaint_delay` when the
only thing since the last frame was suppressed pointer motion breaks that loop:
61fps to 2.05fps, matching the 2 Hz blink rate.

**Wording discipline for this result — do not upgrade it.** The original bench run
was confounded (the tester accidentally clicked and dragged, and left the window
partway). It was **subsequently corroborated by an independent A/B on different
hardware** — a laptop, different observer, no accidental input. The mechanism is
real and the magnitude is corroborated; a clean re-run of the original test is still
outstanding (121.25). The wezterm comparison in that A/B is **not**
apples-to-apples: wezterm was not blinking a cursor at 2 Hz (121.26). Full wording
in `DECOUPLING_FRAMEWORK.md` §2A Finding 3 — mirror it, do not strengthen it.

This subtask's known gaps are subtasks 121.12 through 121.17.

### 121.11 — CodeRabbit remediation on PR #465

Commit `f762ca02`. One real bug plus cleanups, from automated review of PR #465.

---

## Group B — Bugs found but unfixed

These were surfaced by Group A and are known-broken or known-coarse today. None is
scheduled.

### 121.12 — The 250 ms fallback makes blink-off slower than blink-on

`SUPPRESSED_POINTER_FALLBACK_DELAY` in `freminal-windowing/src/event_loop.rs` is
250 ms, used by `effective_repaint_delay` when pointer motion was suppressed, egui
asked for an immediate repaint purely because of those suppressed events, and the
app itself requested no delay at all. It is deliberately bounded rather than
unbounded so the window cannot stall when there is no blink schedule to honour (for
example `DECTCEM` has hidden the cursor under btop or vim).

The perverse consequence: with the cursor blinking, the app requests 500 ms and the
suppressed floor is 2 fps. With blinking **off**, the app requests nothing, the
250 ms fallback applies, and the floor becomes **4 fps** — turning the blink off
makes freminal schedule twice as many frames.

**Fix:** three call sites bypass `App::update`'s `shortest_repaint_delay`
aggregation and call `ui.ctx().request_repaint()` directly, so the delay they need
never reaches `app_requested_delay`:

- `freminal/src/gui/terminal/widget.rs:1936` — gutter hover needs-repaint
- `freminal/src/gui/terminal/widget.rs:1942` — gutter clearing frame
- `freminal/src/gui/terminal/widget.rs:3228` — scrollbar damage decision

Route all three through `shortest_repaint_delay` (as `app_impl.rs:3023` and
`app_impl.rs:3436` already do for their own sources). Once every real repaint need
is represented in `app_requested_delay`, `SUPPRESSED_POINTER_FALLBACK_DELAY` can
become `Duration::MAX` and the liveness argument for the bounded fallback goes away.

Note these are **not** the two call sites `DECOUPLING_FRAMEWORK.md` §8 subtask 0.1
ruled benign — 0.1 was about those sites forcing `not_settled` at idle, which they
do not do after warm-up. This is a different defect on the same lines.

**Also unblocks 121.26**: the honest apples-to-apples comparison against wezterm
requires `cursor.blink = false`, which today lands on the worse of the two floors.

### 121.13 — Chrome cache is disabled during pointer motion

`egui_integration.rs` stashes `self.prev_repaint_delay = repaint_delay` from the raw
`ViewportOutput::repaint_delay`, **not** from `effective_repaint_delay`'s
substituted value. `chrome_repaint_settled(prev_repaint_delay, ...)` then reads
egui's raw `0` on every suppressed-pointer frame and concludes the chrome has not
settled. Consequence: `ChromeMode::Replay` sits at roughly 0.5% duty cycle while the
mouse is moving, against 100% at idle. The 121.8 win is silently switched off for
exactly as long as the pointer moves.

### 121.14 — `animation_in_flight` tests presence, not motion

`app_impl.rs:818`: `animation_in_flight` is
`win.resize_overlay.is_some() || !toasts.is_empty()`. A toast only requests 16 ms
while actually fading; during its steady hold it requests 250 ms, and the resize HUD
is fully opaque for 650 ms of its 900 ms life. So any visible toast or HUD disables
suppression for its whole 1–3 s life rather than just its animating portion. A
superset of correct, therefore safe, but wasteful.

**Fix:** surface `toast.rs`'s existing `any_animating` local (`toast.rs:1585`,
returned from `measure_inputs` at `toast.rs:1662`) as a real signal instead of
testing for presence.

### 121.15 — `has_urls` and `scroll_offset > 0` are pane-wide vetoes

Any pane containing a hyperlink, or scrolled back at all, reverts to full-rate
scheduling for pointer motion anywhere in it. Conservative direction: it costs
benefit, not correctness. Subsumed by 121.17, which is the preferred fix.

### 121.16 — Config kill switch for the suppression (WITHDRAWN)

> **WITHDRAWN by maintainer decision. Do not re-propose.** This subtask previously
> read: 121.10 is default-on for every build, the `frame-profiling` feature gates
> only the diagnostics, so add a config toggle before the spike is relied upon.

Rejected. A config toggle for scheduling behaviour ships two code paths and tests
neither, and it converts a bug into a supported configuration. If the suppression
misbehaves it is a **bug**, and the remedy is to fix it or revert the commit — not
to let users route around it. freminal is beta software; revert-and-fix is the
correct failure mode at this stage.

The underlying fact remains true and is **not** a defect: 121.10 changes scheduling
for every build with no runtime opt-out. `DECOUPLING_FRAMEWORK.md` §2A lists that
under "Finding 3's known gaps" and recommends considering a toggle. **That
recommendation is overruled by this entry** — §2A's statement of the gap is
accurate, its suggested remedy is not the one being taken.

---

## Group C — Unifying improvement

### 121.17 — Cell-granular pointer suppression

Nearly all of the terminal's interactive state changes at **cell** granularity, not
pixel granularity: URL hover, gutter hover, selection extent, and mouse-tracking
reports are all per-cell. Pointer motion within a single cell therefore cannot
change any of them.

Cache the pane's terminal-rect origin and logical cell size during `update()`, track
the pointer's cell rather than its position, and suppress any `CursorMoved` that
does not cross a cell boundary. That one mechanism:

- removes the `has_urls` and `scroll_offset` pane-wide vetoes (121.15) — a pane full
  of hyperlinks still suppresses, because only a hovered-cell change wakes it,
- lets **selection drags suppress too**, which they cannot today,
- subsumes the gutter carve-out (the gutter is per-row), and
- is correct for mouse-tracking mode, whose reports are per-cell.

**The scrollbar must stay excluded** — thumb dragging is genuinely pixel-granular.

Per `DECOUPLING_FRAMEWORK.md` §2A this is the highest-value follow-up on the
scheduling axis, and it is strictly freminal-owned logic with no egui dependence.

**Depends on Task 122 (orchestration extraction).** This is the one Task 121 subtask
that does. `pointer_motion_needs_repaint` runs **outside** a frame, so the per-pane
terminal-rect origin and cell size must be captured during `update()` and read from
the event layer — per-pane render-time geometry threaded to the event path, which is
exactly what Task 122 builds a home for. §2A records that the suppression predicate
"already needed four rounds"; adding a fifth to its current shape is how the
maintainability argument for the egui rewrite gets stronger for no good reason. Do
Task 122 first.

---

## Group D — Unactioned issue #459 items

Issue #459 is still open. Its candidate list items 1 and 2 are done (121.3 and
121.8); items 3 through 8 have never been actioned. Item 9 was added in a comment
thread and is covered by 121.5.

**Maintainer priority ordering** (from `DECOUPLING_FRAMEWORK.md` §2A "Beyond
scheduling: per-frame cost"): the font and text pipeline first — unicode width,
rustybuzz shaping (121.19), and `build_foreground_instances` (121.18) — then the
remainder. Note the savings are not idle-only: the same per-frame work is paid on
the active path.

### 121.18 — Non-incremental vertex-instance build (#459 item 3)

`build_background_instances` and `build_foreground_instances` clear and walk every
visible row unconditionally whenever anything is dirty; a single-row change pays the
same cost as a full-screen rebuild. Measured at 4.80% self under btop. The existing
`instanced_bg_partial_dirty` / `instanced_fg_partial_dirty` counters already
quantify the recoverable headroom. Also listed as issue #405 Part B's own
suggested-next-step 4.

### 121.19 — ASCII / simple-text shaping fast path (#459 item 4)

`<char as UnicodeGeneralCategory>::general_category` was 8.56% self under btop,
inside `unicode-properties`, a dependency of `rustybuzz` itself — not callable from
freminal code and not cacheable by us. `ShapingCache` already avoids re-shaping
unchanged rows, so this is genuine reshape cost. The lever is a fast path that skips
full rustybuzz shaping for runs that cannot need ligatures or complex script
shaping. **Confirm no such path exists today before scoping.**

### 121.20 — GPU buffer-orphaning for `deco_verts` (#459 item 5)

The `glBufferData(NULL)`-orphan then `glBufferSubData` pattern in `upload_verts`
pays a Mesa slab-allocator round trip on every blink tick for a small, fixed-size
payload. Roughly 10% combined at idle. Investigate whether orphaning is necessary at
this payload size.

### 121.21 — Compute-shader-dispatched buffer clear (#459 item 6)

4.57% self at idle in `si_fast_clear` to `si_compute_clear_copy_buffer`. Confirm
whether the clear is scoped to the damage rect or the full framebuffer, and why a
compute dispatch is used rather than fixed-function.

### 121.22 — `wayland_client_handle` call frequency (#459 item 7)

7.83% self at idle for what should be an O(1) `OnceCell` fetch. Almost certainly a
call-frequency problem rather than a lookup-cost problem. Confirm before fixing.

### 121.23 — Profiling methodology document (#459 item 8)

`Cargo.toml`'s `[profile.profiling]` comment points at "CONTRIBUTING / the profiling
notes for the btop idle-CPU investigation (issue #405)". No such file or notes exist
anywhere in the repo. Either write that reference document — capturing the
`perf record --call-graph dwarf,65528` and `--no-inline` invocation, without which
the DWARF unwinder silently truncates freminal's deep stacks and produces a
false-negative flamegraph — or fix the comment to stop pointing at nothing.

Note that `DECOUPLING_FRAMEWORK.md` §8 subtask 0.3 records that the in-app harness
(121.6) proved sufficient to root-cause all three Phase 0 findings, so the `perf`
cross-validation was never needed. That does not retire this item: the methodology
is still the fallback if a finding is ever disputed, and the dangling `Cargo.toml`
reference is a defect regardless. Creating a new document requires maintainer
approval per `no-summary-documents`.

### 121.24 — Two heap allocations per `CursorMoved` — measure before fixing

`pane_tree.layout(central_rect)` and `iter_panes()` each allocate a `Vec` inside
`pointer_motion_needs_repaint`, which runs at the mouse's full report rate —
measured at 425–478 events/s. At roughly 1000 small allocations/s that could be a
material fraction of the roughly 0.077% of a core the suppression leaves behind.

**Measure first.** Add a counter or profile the predicate specifically, then choose
between a `layout_into(&mut buf)` variant and a scratch buffer on `PerWindowState`.
Do not build the buffers speculatively: every Phase 0 hypothesis acted on without
measurement turned out to be wrong.

---

## Group E — Measurement debt

### 121.25 — Typing and btop workloads, and a clean Finding 3 re-run

Only genuine idle and pointer-motion-over-static-content were captured. **Typing**
and **btop** (hidden-cursor / `DECTCEM`) are unmeasured, and the clean, unconfounded
before/after run of Finding 3 has not been done. All need a human at the machine.
This is `DECOUPLING_FRAMEWORK.md` §8 subtasks 0.2 (outstanding half) and 0.6.

### 121.26 — Blink-off comparison against wezterm

The wezterm A/B in `DECOUPLING_FRAMEWORK.md` §2A Finding 3 is not apples-to-apples:
wezterm is not blinking a cursor at 2 Hz, and freminal's floor is roughly 2 fps of
blink frames by construction. The honest test is freminal with
`cursor.blink = false`, which is likely to close most of the remaining gap. Blocked
on 121.12 — today, blink-off lands on the 4 fps fallback and would measure worse
than blink-on.

### 121.27 — `DESIGN_DECISIONS.md` entry for Phase 0

`DECOUPLING_FRAMEWORK.md` §8 subtask 0.5, still outstanding. Must record the
direction **and** the inconvenient numbers, including that Phase 0 weakened the case
for the egui rewrite rather than strengthening it.

### 121.28 — Pixel / headless-GL test harness (issue #440)

No headless-GL or pixel-readback harness exists; the "436.9 pixel harness" never
landed. Everything in Groups A through D is validated by counters, `perf` samples
and human observation — never by pixels. Any regression in the suppression or
chrome-cache paths that changes what is drawn rather than how often is currently
undetectable in CI.

---

## Verification

Standard for every subtask, per `agents.md`:

1. `cargo test --all`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo machete`
4. `cargo xtask check-windows` before any PR (`freminal-windows-crosscheck`)

Scheduling and per-frame-cost subtasks additionally require a before/after capture
per `performance-benchmarks` and `freminal-bench-table`. Group D subtasks touch the
render and shaping hot paths and are benchmark-mandated.

---

## References

- `Documents/DECOUPLING_FRAMEWORK.md` — §2A is the source of truth for the Phase 0
  measurements, findings and gaps; §12 is the annotated pointer list.
- `Documents/PLAN_VERSION_120.md` — the v0.12.0 summary section for this task.
- `Documents/EGUI_UPGRADE_ASSUMPTIONS.md` — assumptions A1–A13; A6 and A13 are
  flagged untested by their own authors.
- Issue #405 — the earlier idle-CPU investigation this pivoted from.
- Issue #435, issue #436 — partial present and chrome caching, both closed; 121.8
  is the fix that made #436 actually engage.
- Issue #440 — the missing pixel / headless-GL harness (121.28).
- Issue #457 — `merge_cache` structural shift, still open, deprioritized by 121.1.
- Issue #459 — the profiling findings and the candidate list Group D drains.
