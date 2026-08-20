# PLAN_121_PERF_REMEDIATION.md — Task 121 "Performance Remediation"

> **STATUS: IN PROGRESS.** Fourteen subtasks have merged to `main` across six pull
> requests (#458, #460, #461, #464, #465, #467) — though **121.13 was subsequently
> reverted**, so thirteen stand — plus 121.23, 121.26 and 121.32 landed directly.
> The remainder — one bug now routed through 121.17 rather than blocked, one
> unifying improvement (121.17, whose Task 122 dependency was discharged on
> 2026-08-03 but whose measured numbers are stale), two-and-a-half pieces of
> measurement debt (121.25 is partly captured), three items surfaced by the Group B
> work, and Group G's four open chrome-cache follow-ups (121.33–121.36) — are
> outstanding and unscheduled.
>
> **Group D is drained (2026-08-20).** All six unactioned issue #459 items have now
> been reconned. Four are **closed as not actionable as framed** — 121.18 and 121.19
> (2026-08-16), 121.21 and 121.22 (2026-08-20). 121.24 is **complete and refuted by
> measurement**, which also corrects 121.25's attribution of the pointer-motion
> residual. Only 121.20 remains live, and it is **not** a scheduling matter but a
> maintainer decision: its premise is confirmed, but the fix lands in issue #432's
> silent-corruption bug class with no harness to catch a recurrence. Group D produced
> no production-code change, which is the correct outcome for a group whose every
> entry was gated on "measure/confirm before fixing".

Task 121 is carried by v0.12.0. The version-level summary lives in
`PLAN_VERSION_120.md` ("Task 121 — Performance Remediation"); this document is the
full breakdown.

> **Citation re-verification (2026-08-16).** The egui stack moved 0.35.0 → 0.36.1 in
> `06701ce8`; that bump re-verified `EGUI_UPGRADE_ASSUMPTIONS.md` but left this
> document's internal-line citations pointing at the old source tree. All
> `egui-0.35.0` citations below (121.9, 121.29, 121.30, 121.32) have been re-checked
> against 0.36.1 and updated where the line moved. Two citations turned out to be
> wrong in **both** versions — a four-line offset in 121.29 item 2, and an incorrect
> claim in 121.32 that `potential_drag_id` is cleared the same way as
> `potential_click_id` (it is not; egui deliberately leaves it alone). Both are
> pre-existing errors, not bump-induced ones, and are fixed in place below. No
> subtask's status or verdict changes as a result.
>
> **Dated harness captures are deliberately NOT re-pointed.** Where this document
> quotes what `repaint_cause_top8` actually logged (121.29 and 121.30, both
> "harness, 2026-07-29"), the `egui-0.35.0` paths are left verbatim, because that
> is the version the run observed and a log is not a citation. So the same
> mechanism legitimately appears twice at two versions: `context.rs:525` inside
> the 0.35.0 capture, and `context.rs:536-537` as the live 0.36.1 reference. Do
> not "fix" the captures to match.

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

| Group                                   | Subtasks      | Status        |
| --------------------------------------- | ------------- | ------------- |
| A — Completed work (merged to `main`)   | 121.1–121.11  | Complete      |
| B — Bugs found and fixed                | 121.12–121.14 | Complete      |
| B — Bug blocked behind Task 122         | 121.15        | Unblocked     |
| B — Withdrawn                           | 121.16        | Withdrawn     |
| C — Unifying improvement                | 121.17        | Not started   |
| D — Reconned, premise does not hold     | 121.18, 121.19, 121.21, 121.22 | Closed — not as framed |
| D — Reconned, needs a maintainer gate   | 121.20        | Blocked on 121.28 or a QA gate |
| D — Measured and refuted                | 121.24        | Complete      |
| D — Profiling methodology               | 121.23        | Complete      |
| E — Measurement debt                    | 121.27–121.28 | Not started   |
| E — Measurement debt (partly captured)  | 121.25        | In progress   |
| E — Blink-off comparison                | 121.26        | Complete      |
| F — Surfaced by the Group B work        | 121.29–121.31 | Not started   |
| G — beta.7 interaction regression       | 121.32        | Complete      |
| G — Surfaced by 121.32                  | 121.33        | Not started   |
| G — Chrome-cache decision gate          | 121.34        | Not started   |
| G — Chrome-cache waste while disabled   | 121.35        | Deferred      |
| G — Confine Replay to non-chrome        | 121.36        | Conditional   |

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
`egui-0.36.1/src/context.rs` `begin_pass` calling
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

## Group B — Bugs found by Group A

These were surfaced by Group A. 121.12, 121.13 and 121.14 were **merged to `main`** via
PR #467 (merge commit `f7dac216`, one atomic commit per subtask on `task-121/group-b`).
**121.13 was subsequently reverted (2026-08-02) — it shipped a user-visible interaction
regression in 0.12.0-beta.7; see 121.32.** 121.12 and 121.14 stand. 121.15 remains
unfixed and is deliberately left to 121.17, whose Task 122 dependency was discharged
when that task merged on 2026-08-03. 121.16 is
withdrawn.

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

Note these are **not** the two call sites `DECOUPLING_FRAMEWORK.md` §8 subtask 0.1
ruled benign — 0.1 was about those sites forcing `not_settled` at idle, which they
do not do after warm-up. This is a different defect on the same lines.

**Also unblocks 121.26**: the honest apples-to-apples comparison against wezterm
requires `cursor.blink = false`, which previously landed on the worse of the two
floors.

### 121.12 outcome (DONE — merged, PR #467)

**This entry originally named three bypassing call sites. That was wrong — there are
eight**, and the miscount changed what the fix buys. Recon found, in addition to the
three gutter/scrollbar sites: `widget.rs`'s bell-flash fade, cursor-trail animation
and animated-image tick; `app_impl.rs`'s resize-overlay HUD (inside `central_body`,
with the aggregate local in scope); and `toast.rs`'s toast cadence (outside
`central_body`, after the aggregate is published).

That miscount concealed a **live visual bug this entry never described**: because
those animations' 16ms requests never reached `app_requested_delay`, and egui's raw
delay is zero during pointer motion regardless, the substitution fired, found
`None`, and returned the fallback. The bell flash, cursor trail and animated images
were animating at **4fps instead of 60fps whenever the mouse was moving** over
terminal content. Routing them is a correctness fix in its own right, independent of
the fallback constant.

All eight now fold into the aggregate: six via a new
`PaneRenderCache::pending_repaint_delay` drained after `show()` returns
(`paint_bell_flash` returns `Option<Duration>` rather than requesting a repaint
itself), one directly, and `ToastStack::show` returning its delay for a second
aggregation point. The three formerly-bare `request_repaint()` sites fold **16ms,
not `Duration::ZERO`** — scheduling is identical because `clamp_repaint_delay`
already floored at `MIN_REPAINT_INTERVAL`, but a zero app-side ask would make
`chrome_repaint_settled`'s `repaint_delay >= app_delay` test permanently vacuous.

**`Duration::MAX` was considered and rejected.** This entry's closing claim — that
once every real repaint need is represented the liveness argument goes away — does
not hold: `app_requested_delay` can only ever represent *freminal's* needs, and
**egui's own chrome animations are unrepresentable in it** and are masked by egui's
events-driven zero. Unbounded would freeze such an animation at partial alpha until
an unrelated event arrived. `SUPPRESSED_POINTER_FALLBACK_DELAY` is therefore
**250ms → 500ms**, chosen to equal the cursor-blink period so blink-off can never
schedule more frames than blink-on, which is the perversity this entry is about. A
regression test pins `>= 500ms` so it cannot be reintroduced.

A mechanism *does* exist to make unbounded safe — `Context::repaint_causes()` — but
consuming it costs five egui-internals dependencies, two of which are present-day
holes. See **121.29**, which records the full analysis so it is not re-litigated.
The measured prize over 500ms is ~0.075% of a core.

### 121.13 — Chrome cache is disabled during pointer motion

`egui_integration.rs` stashes `self.prev_repaint_delay = repaint_delay` from the raw
`ViewportOutput::repaint_delay`, **not** from `effective_repaint_delay`'s
substituted value. `chrome_repaint_settled(prev_repaint_delay, ...)` then reads
egui's raw `0` on every suppressed-pointer frame and concludes the chrome has not
settled. Consequence: `ChromeMode::Replay` sits at roughly 0.5% duty cycle while the
mouse is moving, against 100% at idle. The 121.8 win is silently switched off for
exactly as long as the pointer moves.

### 121.13 outcome (REVERTED 2026-08-02 — but NOT the cause of the beta.7 bug)

> **READ THIS BEFORE RE-LITIGATING 121.13. It was investigated at length on
> 2026-08-02 and the conclusions below are measured, not argued.**
>
> **Status: reverted on `main`.** The scheduling analysis below is **correct** and was
> never disproved: `run_frame` really did stash the raw
> `ViewportOutput::repaint_delay`; that value really is always zero while pointer
> events sit in egui's queue; `ChromeMode::Replay` really was pinned near 0% duty
> cycle while the mouse moved. Substituting the effective delay really did fix that.
>
> **121.13 was NOT the proximate cause of the beta.7 tab-click / border-drag
> regression.** That was the initial hypothesis, it drove the revert, and **reverting
> it did not fix the symptom** — verified by the maintainer against a system build.
> Do not re-derive that hypothesis; it is closed.
>
> What 121.13 *did* do is raise `Replay`'s duty cycle, which increases exposure to a
> **structural unsoundness in the chrome cache that is older than 121.13 and
> independent of it** (121.32). 121.14 raises that duty cycle too, and the
> unsoundness is reachable without either.
>
> **The revert is retained for a different reason than it was made:** with the chrome
> cache now disabled by default (121.32), 121.13's substitution only affects *when*
> `Replay` would be chosen, and `Replay` is never chosen. It is dead code with a
> subtle double-write contract, so it stays reverted until and unless the cache is
> re-enabled soundly.
>
> **If you are re-enabling the chrome cache, 121.13's reasoning is worth restoring —
> but only after 121.32's structural constraint is satisfied. Restoring 121.13 alone
> re-creates the exposure without fixing anything.**

Fixed with a narrow `EguiState::stash_effective_repaint_delay` called from the
`RedrawRequested` arm once `effective_delay` is known. `run_frame` still performs
the raw write, so the field is never left unwritten on a path that does not reach
the override; the deliberate double-write is documented at both ends.

**The stashed value is not the scheduled one, and that distinction is the fix.** One
value cannot answer both "what do we schedule?" (bounded — we cannot prove nothing
needs drawing) and "did anything actually *want* a repaint?" (no — the absence of an
app request is itself the proof). Stashing the scheduled value would have left the
`app_requested_delay == None` case exactly as broken: the synthetic 500ms liveness
poll would be stashed, `chrome_repaint_settled`'s `None` arm would compare it
against `Duration::MAX`, and the gate would still decide `Full`. **That case is
precisely the btop / `DECTCEM`-hidden-cursor workload named in 121.25.** Scheduling
therefore uses `effective_repaint_delay`; the gate is fed a sibling,
`effective_chrome_gate_delay`, differing in exactly one branch (`None` yields
`Duration::MAX`).

Replay stays independently gated on `cache_matches`, `damage_unchanged` and
`no_chrome_input`, and both `toast_active` and `any_overlay_open` force
`ChromeDamage::Changed` every frame they hold, so a genuinely changed chrome still
cannot be replayed.

Residual, documented in code and tracked as **121.30**: chrome widgets are not
constructed at all on `Replay`, so an egui-internal chrome animation would not
merely be scheduled less often — its advancing logic would not run. Latent, not
live. The wiring itself has no automated coverage (see 121.28).

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

### 121.14 outcome (DONE — merged, PR #467)

**Both halves fixed**, not just the toast half this entry's Fix bullet named — the
resize-HUD half is cited in the bug text above and was trivially fixable.

Resize HUD: a pure `resize_overlay_is_animating` replaces `is_some()`. It reports
`true` past `linger` as well as during the fade, because the overlay is only cleared
by a rendered frame; going false there would strand the HUD on screen at partial
alpha. **Narrowing the predicate alone would have achieved nothing** — after 121.12
the HUD folds a delay into the aggregate every frame it is alive, so
`app_requested_delay` would have stayed 16ms and scheduling would have stayed at
60fps. It now requests the time remaining until the fade begins while opaque. A
property test pins the coupling between the two functions (the originally proposed
`is_animating == (delay <= 16ms)` invariant is **false** — in the opaque phase the
countdown is transiently `<= 16ms` while `is_animating` is still false; the real
coupling is: `animating` implies exactly 16ms, and `!animating` implies exactly
`fade_start - elapsed`).

Toasts: cached on `ToastStack`, read via `is_animating()`, because the predicate runs
outside any frame. `push()` sets the flag eagerly — exact rather than merely
conservative, since a new toast is always mid-entry-animation — closing the priming
gap between a push and the next render. `try_borrow`-fails-means-`true` preserved.

**A hover regression had to be fixed to make the toast half safe at all.**
`is_chrome_interactive_at` tested only head and border rects; toast rects were not in
the pointer-interactive set, and it was toast *presence* that had been keeping hover
alive. Suppressing during the steady hold would have left the close-button highlight
and hover-to-pause resolving only at the 250ms cadence. `ToastStack::show` now also
returns its laid-out rects, cached in a dedicated `chrome_toast_rects` and tested by
a widened `point_in_chrome_rects`. This also correctly forces `ChromeMode::Full`
while the pointer is over a toast.

Accepted residuals, documented in code: the cached rects are one frame stale, so a
hover can be missed for a frame while the stack reflows; and `chrome_toast_rects`
goes stale on `App::update`'s `CLEANUP-436-A` early return, symmetrically with
`chrome_head_rects` and `chrome_border_rects` (pre-existing, a documented
should-never-happen branch). The toast's 16ms/250ms cadence is deliberately
unchanged — unlike the HUD it has hover-extension and expiry logic driven per frame.
The presence-based `toast_active` chrome-damage signal is untouched; it is
presence-based by design.

### 121.15 — `has_urls` and `scroll_offset > 0` are pane-wide vetoes

Any pane containing a hyperlink, or scrolled back at all, reverts to full-rate
scheduling for pointer motion anywhere in it. Conservative direction: it costs
benefit, not correctness. Subsumed by 121.17, which is the preferred fix.

**Measured 2026-07-29: `has_urls` alone vetoes 100% of pointer-motion checks** (792 of
792), taking suppression from 99.16% to 1.68% and the motion path to ~20× the CPU. See
121.17's measured-prize table. "It costs benefit, not correctness" remains true, but
the benefit it costs is nearly all of it.

**Deliberately excluded from the Group B fix (maintainer decision).** 121.12–121.14
shipped without it. An interim narrowing here would mean adding a fifth round to the
suppression predicate in its current shape — exactly what 121.17 warns is how the
maintainability argument for the egui rewrite gets stronger for no good reason. It
remains routed through 121.17 — the Task 122 dependency that once blocked both was
discharged when that task merged on 2026-08-03, so what defers 121.15 now is the
decision to fix it via 121.17 rather than separately, not a blocker.

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

> **UNBLOCKED on the Task 122 side (2026-08-02), but RE-CHECK YOUR ASSUMPTIONS
> BEFORE STARTING.** Two things changed under this subtask on the same day:
>
> 1. **The seam it was waiting for exists.** Subtask 122.15 publishes the per-pane
>    terminal-rect origin through `PublishedFrameState`
>    (`pane_terminal_origin(pane_id) -> Option<Point>`), and logical cell size was
>    already reachable out-of-frame via `cell_size()`. The reader currently carries
>    `#[allow(dead_code)]` with a TODO naming this subtask — **remove that allow**
>    as part of landing 121.17.
> 2. **The chrome cache is disabled (121.32), and may be deleted outright.** This
>    subtask's measured prize and its "also un-gates 121.13" finding below were
>    both computed in a world where `ChromeMode::Replay` was live. 121.13 is
>    reverted, `Replay` is never chosen, and 121.34 may remove the machinery
>    entirely. **The numbers below are stale and the un-gating argument no longer
>    holds as written.** Re-measure before committing to a design, and do not
>    resurrect the 121.13 interaction as a justification.
>
> The Task 122 dependency is discharged. The chrome-cache dependency is new.

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

### 121.17 measured prize (harness, 2026-07-29) — STALE, see caveat above

Refining the caveat at the top of this entry: these numbers predate 121.32
disabling the chrome cache. The suppression
percentages and veto counts are still indicative — they concern
`pointer_motion_needs_repaint`, which 121.32 did not touch — but the `Replay %`
column is now meaningless, because `ChromeMode::Replay` is never chosen. The
whole table must be re-captured before it is used to justify a design.

No longer an argument. Captured with `--features frame-profiling`, wiggling the
pointer over terminal content in three scenarios, flushes differenced:

| Scenario | checks | suppressed | veto firing | `Replay` % (void)² | µs/frame |
| --- | --- | --- | --- | --- | --- |
| Clean pane | 15,265 | **99.16%** | `overlay_open` 126 (0.8%) | 58.3% | 729¹ |
| One OSC 8 URL on screen | 792 | **1.68%** | `has_urls` **792 (100%)** | 5.8% | 185 |
| btop | 217 | **0%** | `mouse_tracking_active` **216 (99.5%)** | 40.4% | 521 |

¹ single flush, so warm-up is included; the others are differenced.
² retained only as a record of what was observed while the chrome cache was
live; `Replay` is never chosen since 121.32, so this column has no bearing on
current behaviour.

**A single hyperlink on screen takes suppression from 99.16% to 1.68%.** `has_urls`
fired on 792 of 792 checks — total defeat. btop confirms `mouse_tracking_active` does
the same (216 of 217, zero suppressed), so any mouse-reporting TUI is affected.

Cost of the vetoed path: the URL run's flushes are 1.955 s apart for 120 frames =
**61.4 fps sustained**, at 185 µs/frame = **~1.1% of a core**, against ~0.06% for the
clean pane at its 2–3 fps blink floor. **Roughly 20×.**

**A CPU meter cannot see this** — a ~1.1% burst lasting a few hundred ms averages to
0.1–0.2% over a typical sampling window, which is why informal observation (121.25)
read the vetoed and unvetoed paths as identical. Use the counters, not a meter.

### 121.17 also un-gates 121.13 (WITHDRAWN - the premise no longer exists)

> **WITHDRAWN 2026-08-02. Do not use this as a justification for 121.17.**
> 121.13 is reverted and the #436 chrome cache is disabled by default (121.32),
> so `ChromeMode::Replay` is never chosen. There is no 121.13 win left to
> un-gate, and 121.34 may delete the machinery entirely. The measurement below
> is retained only as a record of what was observed while the cache was live;
> **every number in it is stale** and none of it supports 121.17 today.
>
> 121.17 must stand on its own measured prize, re-taken against current code.
> If the chrome cache is ever re-enabled soundly, this interaction can be
> re-measured then - but it would need re-measuring, not restoring.

Historical record, measured 2026-07-29 while 121.13 was live and `Replay` was
engaging:

`Replay` duty cycle collapsed 58.3% -> 5.8% in the vetoed case, with
`gate_blocked_not_settled` accounting for essentially every `Full` frame
(`settle_repaint_delay = 0us`, `settle_terminal_requested_delay = 500000us`).
The reading at the time was that `effective_chrome_gate_delay` substitutes only
when `suppressed_only`, so 121.13's win was gated on suppression actually
engaging, and 121.17 would retroactively switch it on for the vetoed path.

That compounding argument is void: both halves of it depend on a `Replay` path
that no longer runs.

---

## Group D — Unactioned issue #459 items

Issue #459 is still open. Its candidate list items 1 and 2 are done (121.3 and
121.8). Item 9 was added in a comment thread and is covered by 121.5.

**Group D is now fully reconned (2026-08-20) and the group heading has outlived its
accuracy** — "unactioned" was true when it was written and is not any more. Every
entry in this group carried a "measure/confirm before fixing" instruction, and every
one has now had it honoured. The result:

| Subtask | #459 item | Outcome                                                       |
| ------- | --------- | ------------------------------------------------------------- |
| 121.18  | 3         | Closed — a redesign, not a subtask (2026-08-16)               |
| 121.19  | 4         | Closed — the ASCII gate is inert at default config (2026-08-16) |
| 121.20  | 5         | Live, but needs a maintainer gate decision (2026-08-20)       |
| 121.21  | 6         | Closed — not a freminal-side problem (2026-08-20)             |
| 121.22  | 7         | Closed — freminal owns no lever (2026-08-20)                  |
| 121.23  | 8         | Complete — `Documents/PROFILING.md`                           |
| 121.24  | —         | Complete — measured and refuted (2026-08-20)                  |

**Four of the six candidate items did not survive contact with their own
verification step.** That is worth stating plainly rather than burying, because it
is the same pattern `DECOUPLING_FRAMEWORK.md` §12 already records for PR #461: a
plausible finding derived from a profile is a hypothesis, not a work item. The
standing "measure first" instruction in these entries is what stopped four of them
becoming speculative refactors. It is also why the maintainer priority ordering
below is now moot in its specifics.

**Maintainer priority ordering** (from `DECOUPLING_FRAMEWORK.md` §2A "Beyond
scheduling: per-frame cost"): the font and text pipeline first — unicode width,
rustybuzz shaping (121.19), and `build_foreground_instances` (121.18) — then the
remainder. Note the savings are not idle-only: the same per-frame work is paid on
the active path. **Superseded in its specifics (2026-08-20):** both named
front-of-queue items are now closed, so this ordering no longer selects any live
work. The principle it encodes — that per-frame cost in the font and text pipeline
is where the remaining headroom is — is unaffected, and 121.19's recon names two
surviving levers (a run-level shaping cache, and per-run allocation reduction) that
inherit it.

### 121.18 — Non-incremental vertex-instance build (#459 item 3)

`build_background_instances` and `build_foreground_instances` clear and walk every
visible row unconditionally whenever anything is dirty; a single-row change pays the
same cost as a full-screen rebuild. Measured at 4.80% self under btop. The existing
`instanced_bg_partial_dirty` / `instanced_fg_partial_dirty` counters already
quantify the recoverable headroom. Also listed as issue #405 Part B's own
suggested-next-step 4.

### 121.18 recon finding (2026-08-16): this is a redesign, not a subtask

The premise is confirmed: both builders `clear()` and walk every visible row
unconditionally. `build_background_instances` is
`freminal/src/gui/renderer/vertex.rs:361-646`, `build_foreground_instances` is
`vertex.rs:722-784`. Called only from `freminal/src/gui/terminal/widget.rs:2770`
(bg) and `:2828` (fg), plus benches.

> **Citation correction (2026-08-20).** This line previously cited `widget.rs:2740`
> and `:2798`. Both were wrong at the time they were written, and neither drifted:
> `:2740` lands inside an unrelated comment block, and `:2798` is the closing `);`
> of the *background* call's argument list, not the foreground call. The real call
> sites are `:2770` and `:2828`, corrected above. Found by adversarial review of the
> Group D close-out; a pre-existing error, not one introduced by it. Nothing in
> 121.18's assessment depends on the line numbers, so no verdict changes.

**Blocker 1 — the instance buffers are variable-length per row, not
fixed-stride.** Background skips `TerminalColor::DefaultBackground` runs
entirely (`vertex.rs:408-415`, a `continue`); foreground skips zero-size glyphs
i.e. spaces (`vertex.rs:1436-1439`). So a row's instance count is a function of
its *content*. A single cell changing default-background→coloured changes that
row's count and shifts every subsequent row's offset in the flat buffer. There
is no row-start-offset index anywhere, so locating "row N's data" costs a full
re-walk — the same cost as the rebuild it is trying to avoid.

**Blocker 2 — `deco_verts` is not row-major past its first section.** The
per-row underline/strikethrough pass runs in the row loop (`vertex.rs:434-503`),
but search-match highlights (`vertex.rs:509-531`), the command-block hover tint
(`546-566`) and selection highlights (`569-617`) are each appended afterwards as
bulk global blocks spanning arbitrary row ranges, with the cursor quad always
last (`619-643`). One row's decoration output can therefore live in up to three
non-adjacent regions plus a shared tail.

**Blocker 3 — no per-row dirty signal survives to the call site.**
`evaluate_frame_dirty_state` in `freminal/src/gui/terminal/frame_dirty.rs:265-314`
derives `content_changed` from `Arc::ptr_eq` over the whole `visible_chars` /
`visible_line_widths` arrays, so one changed cell flips a single global bit for
the entire screen. `freminal-buffer` does track per-row dirty bits internally,
but they are consumed inside `rows_as_tchars_and_tags_cached` and never cross
the snapshot boundary. `ShapingCache` (`freminal/src/gui/shaping.rs:196-228`)
*does* compute per-row change via hash comparison, but discards which indices
changed the moment it returns its `Vec<Arc<ShapedLine>>`.

**Blocker 4 — the GL upload pattern forbids a partial write.** `upload_verts`
(`freminal/src/gui/renderer/gpu.rs:1665-1682`) orphans the buffer
(`buffer_data_size` + `STREAM_DRAW`) before every write specifically to avoid a
sync stall, which leaves prior GPU-side contents undefined. A genuine partial
`glBufferSubData` is therefore impossible without abandoning that pattern, and
there is a double-buffered VBO discipline (`gpu.rs:587-623`) that issue #432
depends on for correctness.

**Assessment.** Making this incremental requires, in order: propagating
per-row dirtiness out of `ShapingCache`; giving `bg_instances`/`fg_instances`
either a fixed stride (padding blank cells, a real GPU-side cost needing its
own benchmark) or a maintained per-row offset index (bookkeeping that silently
corrupts unrelated rows when wrong — exactly the bug class issue #432 already
produced once for the far simpler cursor-quad offset); deciding that
`deco_verts` stays a full rebuild; and only then changing the upload strategy.
That is a redesign of the CPU-side vertex representation plus a new dirty
channel plus a GL-upload change, to recover a measured 4.80% self time.

**Recommendation:** do not attempt 121.18 in its current framing. Either
re-scope it explicitly as that redesign (and price it accordingly), or pursue
the cheaper alternative of reducing how often the full rebuild is triggered at
all — better dirty granularity at the snapshot level — which touches no buffer
layout. Note the existing `instanced_bg_partial_dirty` /
`instanced_fg_partial_dirty` benches quantify the headroom but are explicitly
*not* an incremental implementation.

### 121.19 — ASCII / simple-text shaping fast path (#459 item 4)

`<char as UnicodeGeneralCategory>::general_category` was 8.56% self under btop,
inside `unicode-properties`, a dependency of `rustybuzz` itself — not callable from
freminal code and not cacheable by us. `ShapingCache` already avoids re-shaping
unchanged rows, so this is genuine reshape cost. The lever is a fast path that skips
full rustybuzz shaping for runs that cannot need ligatures or complex script
shaping. **Confirm no such path exists today before scoping.**

### 121.19 recon finding (2026-08-16): the ASCII gate is dead on arrival at default config

The entry asked to "confirm no such path exists today". **Confirmed: none
exists.** Every `TextRun` reaches `rustybuzz` via `shape_single_run` →
`FontManager::shape_cached` → `rustybuzz::shape_with_plan`
(`freminal/src/gui/shaping.rs:661`, `freminal/src/gui/font_manager.rs:787`)
with no complexity-based bypass. The only ASCII fast path in the tree is
`TChar::from_string`'s grapheme-segmentation skip in
`freminal-common/src/buffer_states/tchar.rs:143-146`, which is upstream in the
buffer layer and unrelated to shaping.

The cost attribution is confirmed too: `general_category` is called per
character from inside rustybuzz's own `GlyphInfo::init_unicode_props`, during
`shape_with_plan`. It is not reachable or cacheable from freminal code, so the
only way to avoid it is to not call rustybuzz for that run.

**The blocker: ASCII does not imply "cannot ligate".** `->`, `=>`, `!=` are
pure ASCII and are precisely what ligature substitution targets.
`shaping_features` (`shaping.rs:494-508`) enables `liga` and `calt` under the
config flag, always enables `kern`, and always disables `dlig`. So the only
formulation that is obviously safe is to gate the fast path on
`ligatures == false` — and `FontConfig::default` sets `ligatures: true`
(`freminal-common/src/config.rs:122`, with a test pinning it at
`config.rs:2206`). The fast path would therefore be dead code for every user on
default config. There is no telemetry on how many users set `ligatures = false`,
so no claim is made about the size of that population — but an optimisation that
is inert unless a non-default option is set is a poor use of the effort either
way.

**One risk is smaller than it looks.** Glyph positions are snapped to the cell
grid (`shaping.rs:774-777`, `x_px = col * cell_width`), so rustybuzz's
positional output is discarded for placement. `kern` is a GPOS pair adjustment
and is positional-only, never substitutive, so kerning cannot change a glyph
id. That narrows the substantive risk to the substitutive features `liga` /
`calt` alone.

**One risk is larger than it looks.** A fast path would have to source glyph
ids from `FontManager::resolve_glyph` (`font_manager.rs:690-698`), which
returns a swash **charmap** lookup. That is a different provenance from
rustybuzz's shaped output, and nothing in the tree currently proves the two
agree even for plain ASCII. The existing identity test
(`shape_with_plan_matches_old_shape_for_mixed_content`) compares `shape_cached`
against the old `rustybuzz::shape()` path — it does not compare charmap ids
against shaped ids. Any fast path needs that proof first.

**No coupling risk to selection/search/URLs.** `ShapedGlyph` / `ShapedRun` /
`ShapedLine` are consumed only by `freminal/src/gui/renderer/vertex.rs` and
`widget.rs`; selection, search and URL hit-testing work off the raw `TChar`
grid and their own byte-offset maps, so a fast path's only obligation is
producing identical rendered output.

**Two alternative levers, both compatible with `ligatures = true`:**

1. A content-addressed **run-level** shaping cache keyed on `(face_id,
   ligatures, run text)` storing the raw shaped `(glyph_id, cluster)` pairs.
   Today's `ShapingCache` (`shaping.rs:127`) is keyed by **line index**, so it
   cannot hit across a scroll, and one changed character re-shapes every run
   on that row. A run cache is provably behaviour-preserving (shaping is
   deterministic given face + features + text) and positions are re-derived
   from `col_start`/`cell_width`, which stay out of the key. Needs bounding
   for memory.
2. Per-run allocation reduction in `build_shaped_glyphs`
   (`shaping.rs:701-802`), which builds four `Vec`s per run per cache miss
   (`byte_to_char`, `run_chars`, `cum_cols`, and the output).

**Do not implement either speculatively.** There is no shaping cache hit/miss
instrumentation, and no benchmark models the full-screen TUI redraw workload
that produced the 8.56% figure — the existing `shaping_ligatures` group
benches a cold cache and a fully-warm cache, not a realistic
partial-invalidation stream. Per this document's own standing instruction in
121.24, measure first.

### 121.20 — GPU buffer-orphaning for `deco_verts` (#459 item 5)

The `glBufferData(NULL)`-orphan then `glBufferSubData` pattern in `upload_verts`
pays a Mesa slab-allocator round trip on every blink tick for a small, fixed-size
payload. Roughly 10% combined at idle. Investigate whether orphaning is necessary at
this payload size.

### 121.20 recon finding (2026-08-20): premise confirmed; the only live Group D item, pending a maintainer gate

The premise holds. `upload_verts`
(`freminal/src/gui/renderer/gpu.rs:1665-1682`) orphans unconditionally —
`buffer_data_size(..., STREAM_DRAW)` then `buffer_sub_data_u8_slice` — with **no
size threshold anywhere**, in `upload_verts` or in any of its six callers
(`upload_deco_verts` `gpu.rs:796`, `upload_bg_instances` `:804`,
`upload_fg_instances` `:812`, `upload_img_verts` `:820`, plus
`toast_text_pass.rs:562` and `toast_pass.rs:211`). At genuine idle the
`deco_verts` payload floor is the cursor quad alone: `CURSOR_QUAD_FLOATS = 36`
(`vertex.rs:149`) = **144 bytes**. A 144-byte payload against a slab-allocator
round trip is a real mismatch of scale, so the item is not a phantom.

**The blocker is the orphan's entanglement with commit `c76ae8d1` — but be precise
about what that commit actually fixed, because the repo tells two different stories
and the weaker one is the true one.** `gpu.rs`'s own doc comment asserts that the
unsynchronized in-place `glBufferSubData` "was the confirmed root cause of
issue #432". **The commit message says otherwise, and it is the primary source.** The
root cause it describes is a pure CPU-side bookkeeping bug: the cursor tail-quad
offset was derived from `show_cursor` alone, while `build_background_instances` only
appends a cursor quad when `show_cursor` **and** the blink phase says visible, so a
full rebuild landing on a blink-off instant left the offset pointing at the
bottom-most selection quad, which the next blink-on frame then overwrote. The fix
was to have `build_background_instances` return whether it actually appended a
cursor quad. The GPU-side change is explicitly secondary — the message introduces it
with "**Also** hardens the cursor-only GPU fast path found while investigating".

That distinction cuts **in favour of 121.20**, and it is easy to get backwards —
reading only the code comment yields the opposite conclusion, which is why it is
spelled out here. The orphan is not the surviving half of the #432 fix; it is part
of an opportunistic hardening bundled into the same commit. What
that hardening actually introduced was `deco_vbo`'s own double-buffer index
(`deco_vbo_index`, `gpu.rs:206`, separate from the `vbo_index` shared by
bg/fg/img), described as ensuring the per-blink re-upload "always orphans into a
slot the GPU isn't currently reading". So the two mechanisms were bundled, and the
counterfactual that matters — **double-buffering alone, without re-orphaning** —
was never isolated or tested. There is no `glFenceSync` / `glClientWaitSync`
anywhere in `renderer/*.rs` (zero grep hits), so what remains is not a proven
synchronisation bound but an untested pairing.

**Do not resolve this by reading the code comment.** Reconcile the two narratives
first; a risk assessment that cites the comment while the commit message contradicts
it is exactly the "confident wrong answer from static reading" failure 121.32
records as this subsystem's signature.

Two further constraints on any fix. `buffer_sub_data` never resizes, so a
persistent buffer must be pre-sized for the worst case (selection plus hover tint
plus search highlights plus cursor, across several full-width rows) with an explicit
resize fallback, or it corrupts adjacent VBO regions. And the failure mode of
getting it wrong is **silent visual corruption**, not a crash — the same bug class
as #432.

**Assessment.** Unlike 121.18, 121.19 and 121.22, this is *not* "do not attempt".
It is a narrow, low-line-count change whose regression risk is **smaller than the
`gpu.rs` comment implies** — the offset bug that actually caused #432 is fixed
independently and is unaffected by the orphan — but which still lands in a bug class
this repo has shipped once, and whose failure mode is silent visual corruption
rather than a crash. There is no automated way to catch a recurrence:
`freminal/benches/` contains no GL benchmark, no bench holds a GL context, and
121.28 independently confirms no pixel or headless-GL harness exists.

**Recommendation — maintainer decision required, do not proceed without it.**
Either sequence 121.20 behind 121.28, or accept a documented manual-QA gate
reproducing #432's exact repro (sustained cursor blink concurrent with an active
selection highlight, watching the bottom-most selected row). Note also that the
"roughly 10% combined at idle" figure shares the AMD-radeonsi caveat recorded under
121.21 below, and that per 121.21 the clear — and therefore much of the idle frame —
is skipped entirely on partial-present frames, so the idle denominator this 10% was
measured against may no longer exist.

### 121.21 — Compute-shader-dispatched buffer clear (#459 item 6)

4.57% self at idle in `si_fast_clear` to `si_compute_clear_copy_buffer`. Confirm
whether the clear is scoped to the damage rect or the full framebuffer, and why a
compute dispatch is used rather than fixed-function.

### 121.21 recon finding (2026-08-20): both questions have answers that dissolve the item

**"Scoped to the damage rect or the full framebuffer?" — neither. There is no
scoped clear, because on a partial-present frame there is no clear at all.**
`freminal-windowing/src/egui_integration.rs:1195-1208` is the only production clear
path:

```rust
let partial = match frame_damage {
    crate::FrameDamage::Partial(rects)
        if !rects.is_empty()
            && gl_state.supports_partial_present()
            && gl_state.buffer_age() == 1 => Some(rects),
    _ => None,
};

if partial.is_none() {
    gl_state.clear(clear_color);
}
```

`GlState::clear` (`freminal-windowing/src/gl_context.rs:351-357`) is a plain
`clear_color` + `clear(COLOR_BUFFER_BIT)` with **no scissor** — `gl_context.rs` has
zero scissor references. Scissoring exists in the tree
(`freminal/src/gui/terminal/widget.rs:3062-3067`) but is mutually exclusive with
clearing by construction: its `cursor_only_scissor` gate is set only when the
windowing layer already skipped the clear (comment at `widget.rs:3055-3059`). A
blinking cursor with nothing else changing is exactly the case `decide_frame_damage`
(`freminal/src/gui/frame_damage.rs:78-118`) routes to `Partial`.

**"Why a compute dispatch rather than fixed-function?" — freminal never chose
compute.** Grep for `dispatch_compute` / `GL_COMPUTE_SHADER` / `glow::COMPUTE*`
across `freminal/src` and `freminal-windowing/src` returns zero GL hits. freminal
issues the most basic GL-1.0-era fixed-function clear that exists.
`si_compute_clear_copy_buffer` is Mesa radeonsi translating that request into its
own internal compute fast-clear path for DCC/CMask-compressed surfaces. There is no
GL entry point freminal calls that could select otherwise, so **there is no
freminal-side lever here at all**.

**And the clear cannot simply be deleted — it is load-bearing.**
`build_background_instances` skips emitting a quad for any cell whose effective
background is `TerminalColor::DefaultBackground` (`vertex.rs:408-415`, a `continue`),
precisely because the clear paints those cells. `clear_color`
(`freminal/src/gui/app_impl.rs:872-902`) returns the theme background at the default
`background_opacity == 1.0` (`freminal-common/src/config.rs:359`) and transparent
below it. Removing the clear would leave stale pixels in every default-background
cell unless the `DefaultBackground` skip were also removed — which reinstates
exactly the per-cell quad cost that skip exists to avoid.

**Driver-specific.** The whole `si_fast_clear` / `si_execute_clears` /
`si_compute_clear_copy_buffer` / `si_launch_grid` chain is Mesa radeonsi symbol
namespace (`si_*` = GCN/Southern Islands); the adjacent `amdgpu_bo_create` confirms
an AMD GPU. Intel `iris`, NVIDIA proprietary and `llvmpipe` would very plausibly
execute the identical `glClear` with no compute dispatch and no comparable cost.
This finding does not generalise past the capture machine.

**Recommendation: close 121.21 as not actionable as framed.** The only remaining
freminal-owned question is not a code change but a measurement — *how often does the
partial-present skip actually engage at idle?* Both the `frame_damage_full` /
`frame_damage_partial` counters (`freminal/src/gui/window.rs:526,530`) and 121.8's
`120/120 partial` idle figure postdate issue #459's capture (#459 filed
2026-07-27; the counters landed in `0620cc60` on 2026-07-28), so the original
capture cannot tell us whether the skip was engaging. If a re-measurement shows the
skip near 100% at idle, the residual is Mesa-internal and freminal has nothing left
to fix. Fold that measurement into 121.25 rather than treating it as a code subtask.

### 121.22 — `wayland_client_handle` call frequency (#459 item 7)

7.83% self at idle for what should be an O(1) `OnceCell` fetch. Almost certainly a
call-frequency problem rather than a lookup-cost problem. Confirm before fixing.

### 121.22 recon finding (2026-08-20): not actionable as framed — freminal owns no lever here

The entry said "confirm before fixing". Confirmed, and the answer is that there is
nothing on freminal's side to fix.

**Two claims are in play and only one of them is refuted outright.** The narrow
claim — that freminal repeatedly fetches a window/display handle somewhere it could
hoist — is **refuted** by the two-hit grep below. The broad claim — that the 7.83%
is real and attributable to call frequency — is **not refuted**; it is left open and
un-actionable, because settling it needs measurement freminal cannot act on either
way. This entry therefore reads "not actionable as framed", matching 121.18, 121.19
and 121.21, rather than a flat "refuted".

**The function is what it claims to be.**
`wayland-sys-0.31.11/src/client.rs:113-117` is a `once_cell::sync::Lazy` fetch;
every call after the first is an atomic load. The *first* call per process is not
O(1) — it `dlopen`s `libwayland-client.so` and resolves ~40 symbols through the
`external_library!` macro (`client.rs:20-84`). The `dlopen` feature is confirmed
active: winit enables `wayland-dlopen` in its default feature set
(`winit-0.30.13/Cargo.toml:77,117,331`) and freminal does not disable winit's
defaults.

**freminal never calls it, directly or transitively, from any hot path.** The two
callers both live in dependencies: `calloop-wayland-source-0.3.0/src/lib.rs`'s
lifecycle hooks (`before_sleep`, `before_handle_events`, `process_events`), which
fire per **event-loop wake**, and winit's `request_frame_callback`
(`winit-0.30.13/src/platform_impl/linux/wayland/window/state.rs:250-260`), which is
**debounced** by `FrameCallbackState::Requested` and fires at most once per drawn
frame. The sole freminal call that reaches the second path is
`window.pre_present_notify()` (`freminal-windowing/src/egui_integration.rs:1274`),
once per drawn frame. `window.request_redraw()` does *not* reach wayland FFI — it is
an atomic `compare_exchange` plus a calloop `Ping`. freminal runs
`ControlFlow::Wait` / `WaitUntil`, never `Poll` (`event_loop.rs:707-709`), so the
loop genuinely blocks between wakes.

**The one hypothesis worth testing — a repeated handle fetch freminal could hoist —
is refuted outright.** Grepping `freminal-windowing/src` and `freminal/src` for
`window_handle()`, `display_handle()`, `raw_window_handle`, `raw_display_handle`,
`HasWindowHandle` and `HasDisplayHandle` yields **two hits, both the same call**:
the `use` at `freminal-windowing/src/gl_context.rs:20` and the call at
`gl_context.rs:206`, inside `GlState::new()`. That runs once per window at
GL-context creation, and its result is already reused across all four subsequent
`.as_raw()` uses in the same function body. It is maximally hoisted. In any case
`WindowHandle<'a>` is a borrowed type (`raw-window-handle-0.6.2/src/borrowed.rs:211-222`)
with no owned variant in 0.6.x, so wider caching is not possible even in principle.

**The figure itself is suspect.** 7.83% is `perf report`'s share of *on-CPU
samples*, not of wall-clock or of frame budget, over a 60 s capture of a process
whose own idle baseline #459 records as ~0.1–0.5% CPU — so the absolute sample count
behind that percentage is small and the noise floor correspondingly high. #459's own
methodology section records that `perf script --inline` was "degrading (not
eliminating) inline-frame peeling for some samples" in that environment.
`wayland_client_handle` is a thin wrapper over two nested `Lazy`s whose init closure
does the `dlopen`, which makes **one-time startup cost misattributed onto the
shallow symbol** a live alternative explanation. The capture also predates every
Phase 0 fix and `PROFILING.md`'s frame-rate-plus-per-frame-cost reporting rule
(121.23), which it does not satisfy.

**Recommendation: close 121.22 as not actionable as framed** — the third Group D
item whose premise does not survive recon, after 121.18 and 121.19. Any real fix
would have to land in winit, wayland-backend or calloop-wayland-source, none of
which show a runaway call pattern under static reading. Should anyone want to
pursue it anyway, it needs measurement and not more reading: a fresh Tier-2 capture
per `PROFILING.md` on a post-Phase-0 build; then, if it still reproduces, a second
capture started several seconds after launch to separate the one-time `dlopen`
window from steady state; then `perf probe` for an actual calls/sec figure.

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

### 121.23 outcome (DONE)

`Documents/PROFILING.md` written (maintainer-approved) and the `Cargo.toml` comment
repointed at it. Both halves of the claim above were verified before writing: there
is no `CONTRIBUTING.md` anywhere in the repo, and no profiling notes existed outside
`DECOUPLING_FRAMEWORK.md` §8 0.3 and this entry.

**This entry's own `perf` invocation is wrong, and the document corrects it.**
`perf record` does **not** accept `--no-inline` — only `perf report` and `perf script`
do, verified against `perf --help`. As written here it would fail. It is two commands,
and the two flags do different jobs:

- `perf record --call-graph dwarf,65528` — the stack-snapshot size. The default 8192
  bytes is too small for freminal's call depth and `perf` truncates **silently**,
  which is the false-negative flamegraph this entry warns about.
- `perf report --no-inline` — inline-frame resolution against a full-debuginfo binary
  this size is pathologically slow; without it `perf report` appears to hang. Nothing
  to do with truncation.

Also settled while writing it: `perf`, `cargo-flamegraph` and `cargo-profiler` are
already in the **`default`** dev shell, so no `flake.nix` change is needed (`hotspot`
is absent if a GUI is ever wanted). `perf` is Linux-gated, so Tier 2 is Linux-only.

The document additionally records the **reporting discipline** that motivated doing
this subtask first: always report frame rate and per-frame cost as a pair, never a
single CPU figure, because total CPU is their product and one number cannot separate
"fewer frames" from "cheaper frames". Work on per-frame cost can otherwise mask a
scheduling regression, and vice versa.

### 121.24 — Two heap allocations per `CursorMoved` — measure before fixing

`pane_tree.layout(central_rect)` and `iter_panes()` each allocate a `Vec` inside
`pointer_motion_needs_repaint`, which runs at the mouse's full report rate —
measured at 425–478 events/s. At roughly 1000 small allocations/s that could be a
material fraction of the roughly 0.077% of a core the suppression leaves behind.

**Measure first.** Add a counter or profile the predicate specifically, then choose
between a `layout_into(&mut buf)` variant and a scratch buffer on `PerWindowState`.
Do not build the buffers speculatively: every Phase 0 hypothesis acted on without
measurement turned out to be wrong.

### 121.24 outcome (DONE — measured, and the hypothesis is REFUTED)

Measured, per this entry's own instruction. **The two allocations are ~1.4% of the
residual they were proposed to explain. Do not build the scratch buffer.**

**Where the code went.** Task 122 moved the predicate: `pointer_motion_needs_repaint`
is now at `freminal/src/gui/app_impl.rs:981-1152`, and its pure decision chain was
extracted to `freminal/src/gui/pointer_motion.rs` by subtask 122.5a.
`should_schedule_cursor_moved` stayed at `freminal-windowing/src/event_loop.rs:398`.
PR #495 added a `focus_change_pending` term (`app_impl.rs:1139-1142`) but touched
neither allocation site.

**The allocation inventory is exactly two, and only in the non-zoomed case.**
`iter_panes()` (`panes/mod.rs:1070-1075`, `Vec::with_capacity` of `&Pane`) is called
unconditionally at `app_impl.rs:1034`. `layout()` (`panes/mod.rs:1100-1105`,
`Vec::with_capacity` of `(PaneId, Rect)`) is called at `app_impl.rs:1077` **only
when `zoomed_pane.is_none()`**; the zoomed branch uses `Vec::new()`, which does not
allocate. Nothing else on the path allocates — no `String`, no `Box`, no allocating
`Arc::clone`, no `HashMap`, no `collect()`. `arc_swap.load()` returns a `Guard` with
an atomic-only fast path. Neither `iter_panes` nor `layout` has a single-pane fast
path: `Vec::with_capacity(1)` on a non-ZST always allocates, so a brand-new
single-pane window pays both.

**How it was measured.** `bench_iter_panes` added to the existing
`freminal/benches/pane_resolution_bench.rs`, mirroring `bench_layout`'s shape,
pane counts, chain case, black-box discipline and Criterion configuration (both
share `configure()` and `PANE_COUNTS`, so the settings are identical by
construction). `layout` was already benched there by subtask 122.14; `iter_panes`
was the missing half. Criterion medians, both benches captured in the same sitting.
**One asymmetry, recorded so the table is not read as tighter than it is:**
`iter_panes` takes no argument besides `&self`, so where `bench_layout` black-boxes
its `rect` argument, `bench_iter_panes` must black-box the receiver
(`black_box(&tree).iter_panes()`) to stop LLVM hoisting a loop-invariant call out of
`b.iter()`. That costs one extra opaque barrier per iteration that `bench_layout`
does not pay. It is sub-nanosecond, and it biases `iter_panes` *upward* — i.e.
against the conclusion drawn below — so it is conservative, but the two columns are
not comparable to each other at sub-nanosecond resolution. They are used here only
as an order-of-magnitude bound, which is all the conclusion needs.

| Case        | `layout` (ns) | `iter_panes` (ns) | Sum (ns) |
| ----------- | ------------- | ----------------- | -------- |
| balanced/1  | 11.528        | 11.309            | 22.84    |
| balanced/2  | 13.009        | 11.521            | 24.53    |
| balanced/4  | 27.956        | 12.165            | 40.12    |
| balanced/8  | 58.946        | 19.943            | 78.89    |
| balanced/16 | 123.45        | 36.464            | 159.91   |
| chain/16    | 143.96        | 33.890            | 177.85   |

**The arithmetic, at this entry's own measured 425–478 events/s.** At the modal
single-pane configuration, 478 × 22.84 ns = **~0.0011% of a core** — against the
~0.077% residual this entry proposed the allocations might be a material fraction
of, that is **1.4%**. Even the degenerate 16-pane chain, an unusual configuration,
reaches only 478 × 177.85 ns = 0.0085%, or **11%** of the residual.

**The conclusion is insensitive to which baseline you use, which is why it can be
trusted.** These `layout` figures are roughly 2.8× faster than 122.14's recorded
2026-07-30 baseline (32.816 ns at `balanced/1`). That is a **different sitting on
possibly different hardware and the two are not comparable** — per
`performance-benchmarks`, no speedup is claimed and none should be read here.
It does not matter: scaling both columns up by that 2.847 ratio still gives only
~0.003% of a core, about 4% of the residual. Immaterial either way. (That scaling
assumes `iter_panes` would have moved by the same ratio as `layout` between the two
sittings — an unstated extrapolation, since `iter_panes` has no 122.14 baseline to
substitute. The margin is wide enough that the assumption does not carry the
conclusion: even the *unscaled worst case* in the table, a 16-pane chain, reaches
only 11%.)

**Secondary finding, which is the more useful one: 121.25's attribution of the
motion residual to this predicate is not supported.** For
`pointer_motion_needs_repaint` to account for 0.077% of a core at 478 events/s it
would have to cost ~1611 ns per call. The two allocations are 22.8 ns of that
budget. The rest of the predicate is O(1) boolean composition over already-resolved
flags plus a per-pane atomic `arc_swap.load()` — nowhere near the remaining
~1588 ns. So either the residual lives somewhere else on the per-event path
(winit event decode, egui event translation, `on_window_event` — none of which
anyone has measured), or the 0.077% figure is itself an artefact of the CPU-meter
reading 121.25 already warns must not be used to discriminate. Recorded against
121.25 as well.

**What was deliberately not done.** No counter was added to production code. This
entry offered "a counter **or** profile the predicate specifically"; the counter is
the more invasive of the two and would have measured the wrong thing anyway. The
full predicate is **not** benchable — it needs `&self` on `FreminalGui` plus a real
`WindowId`, which has no public constructor outside the winit event loop (122.14
recorded the same obstacle) — but the two allocating calls are pure, headlessly
constructible, and are the only non-O(1) work in it. Benching them bounds the whole
question, and the bound came out two orders of magnitude below the threshold that
would have justified acting.

---

## Group E — Measurement debt

### 121.25 — Typing and btop workloads, and a clean Finding 3 re-run

Only genuine idle and pointer-motion-over-static-content were captured. **Typing**
and **btop** (hidden-cursor / `DECTCEM`) are unmeasured, and the clean, unconfounded
before/after run of Finding 3 has not been done. All need a human at the machine.
This is `DECOUPLING_FRAMEWORK.md` §8 subtasks 0.2 (outstanding half) and 0.6.

### 121.25 partial capture (IN PROGRESS)

The **clean Finding 3 re-run is done.** Re-measured post-Group-B on second hardware
(a laptop, slower than the original machine), no accidental input, which is what this
entry asked for. Informal tooling readings, not instrumented captures:

| Scenario | freminal |
| --- | --- |
| Genuine idle, `cursor.blink = false` | 0.0%, occasionally 0.1% |
| Genuine idle, blink on | same |
| Pointer motion over static, unvetoed content | 0.1–0.2% |

Consistent with the arithmetic: a 2 fps floor at ~400–600 µs/frame is 0.08–0.12% of a
core, at or below the reporting resolution. Roughly level with the pre-Group-B
reading, which is **expected** — none of 121.12–121.14 is on that path. 121.12's
routing needs an animation live; its 250→500 ms change only bites when the app
requests nothing (blink off); 121.13's `Replay` win is ~59 µs on 2 frames/s ≈ 0.012%;
121.14 needs a toast or resize HUD present.

Note the residual 0.1–0.2% under motion is **not frames** — at a 2 fps floor, drawing
at pointer rate would cost ~2–3% of a core. It is per-event work outside the frame
path: `pointer_motion_needs_repaint` running at 425–478 events/s doing `iter_panes()`,
`pane_tree.layout()` and an `arc_swap.load()` per event. That brackets 121.24's own
0.077% estimate and is corroborating evidence for it.

> **Correction (2026-08-20, from 121.24). The named mechanism is wrong.** The
> paragraph above attributes the motion residual to `iter_panes()` and
> `pane_tree.layout()`. Those two calls have now been benchmarked and together cost
> **22.84 ns** per event at the modal single-pane configuration — 1.4% of the
> 0.077% they were said to explain, and ~1.4% of the ~1611 ns/call the predicate
> would need to cost for that attribution to hold. The remainder of the predicate is
> O(1) boolean composition plus an atomic `arc_swap.load()` and cannot plausibly
> make up the difference. **The residual is therefore unattributed.** The two live
> candidates are per-event plumbing upstream of the predicate (winit event decode,
> egui event translation, `on_window_event`), which nobody has measured, and the
> possibility that the 0.1–0.2% meter reading is itself below the tooling's
> discriminating power — which this very entry warns about two paragraphs down. Do
> not cite the predicate as the cause without a new measurement. See 121.24.

**The vetoed path has since been measured with the harness** — see 121.17's
measured-prize table. It reads 0.1–0.2% on a CPU meter, identical to the unvetoed
path, while actually sustaining 61.4 fps at ~1.1% of a core. **The meter readings above
therefore do not discriminate between the two paths and must not be cited as evidence
that the vetoed path is cheap.** That is the concrete case for `PROFILING.md`'s
reporting discipline: use `pointer_frames_scheduled` / `pointer_frames_suppressed` and
`pointer_repaint_conditions_fired`, not an averaged percentage.

**Still outstanding: typing.** btop is now covered for the veto mechanism
(`mouse_tracking_active` 216 of 217) but **not** for sustained cost — that run logged
217 checks over 26.9 s, i.e. ~8 events/s, so the pointer was barely moving. A
sustained-motion btop capture is still worth having.

### 121.26 — Blink-off comparison against wezterm

The wezterm A/B in `DECOUPLING_FRAMEWORK.md` §2A Finding 3 is not apples-to-apples:
wezterm is not blinking a cursor at 2 Hz, and freminal's floor is roughly 2 fps of
blink frames by construction. The honest test is freminal with
`cursor.blink = false`, which is likely to close most of the remaining gap. Blocked
on 121.12 — today, blink-off lands on the 4 fps fallback and would measure worse
than blink-on.

### 121.26 outcome (DONE — but not as specified)

Measured against wezterm on second hardware. At genuine idle, freminal with
`cursor.blink = false` reads **0.0%, occasionally 0.1%**, level with blink on. The gap
the original A/B implied is closed.

**The lever this entry is built on no longer works, and did not need to.** 121.12 set
`SUPPRESSED_POINTER_FALLBACK_DELAY` to 500 ms precisely so blink-off could never be
worse than blink-on — which means blink-off now falls back to the *same* 500 ms and
**does not remove the 2 Hz floor** this entry wanted to eliminate. Had 121.12 gone
unbounded (see 121.29) blink-off would have dropped to zero scheduled frames and the
comparison would have been clean by construction. That trade was not flagged when
121.12 landed.

It does not reopen the decision; it dissolves the question. The floor this entry set
out to control for measures ~0.08% of a core. A confound below reporting resolution
does not need removing.

**Wording discipline — do not upgrade this.** The equality is
**resolution-limited, not measured**: both figures sit at or under the tooling's one
decimal place. Do not restate it as "freminal matches wezterm exactly", and mirror
`DECOUPLING_FRAMEWORK.md` §2A's caution rather than exceeding it.

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

Group B added two concrete instances. (1) 121.13's tests exercise the pure
`chrome_repaint_settled` / `evaluate_chrome_gate` functions, which that subtask did
not modify — they pin the reasoning, not the wiring, and would pass against
pre-121.13 code. The wiring (that `event_loop.rs` calls the stash at the right point
with the right value on every reachable path) needs a live winit window and GL
context and has none. (2) 121.29 cannot be attempted safely without this harness.

---

## Group F — Surfaced by the Group B work

### 121.29 — Unbounded suppressed-pointer fallback via `repaint_causes()`

`SUPPRESSED_POINTER_FALLBACK_DELAY` is bounded (500ms after 121.12) because
`app_requested_delay` represents only freminal's repaint needs. egui's own chrome
animations are unrepresentable in it and are masked by egui's events-driven zero, so
`Duration::MAX` would freeze such an animation at partial alpha until an unrelated
event arrived.

A mechanism exists to discriminate. `egui-0.36.1/src/context.rs:536-537` pushes the
events-driven zero as a `RepaintCause`, and `ContextImpl::request_repaint_after`
pushes `causes` **unconditionally**, before the `delay < repaint_delay` early-out, so
the list survives even though the delay value is flattened to zero.
`Context::repaint_causes()` exposes it. If the only cause is the `begin_pass` events
cause, nothing but suppressed pointer motion wants a frame and unbounded is safe.

**This was investigated and rejected for now.** Consuming it depends on five egui
internals, two of which are present-day holes rather than future-bump risks:

1. The events-driven zero is recorded as a cause at all, identifiable by `file`+`line`
   — making an egui *source line number* load-bearing runtime data, in a repo whose
   own upgrade checklist says line numbers drift and must not be trusted.
2. `causes.push` happening unconditionally. **Correction (2026-08-16): the original
   citation here, `context.rs:157-159`, was off by four lines in both 0.35.0 and
   0.36.1 — a pre-existing error, not something the version bump introduced.** The
   actual push, `viewport.repaint.causes.push(cause);`, is at `context.rs:153`; the
   `if delay < viewport.repaint.repaint_delay {` early-out it precedes is at
   `context.rs:158`. The substance of the claim is unaffected: the push genuinely
   happens unconditionally, before that early-out.
3. `repaint_causes()` returning `prev_causes` — exactly one pass stale
   (`context.rs:102-105`).
4. **`outstanding`-driven repaints push no cause at all** (`context.rs:110-118`).
   Every zero-delay `request_repaint()` sets `outstanding = 1` ("Each request results
   in two repaints, just to give some things time to settle"); the following pass is
   forced to `ZERO` down a path that never touches `causes`. A cause-based test is
   structurally blind to egui's own settling mechanism.
5. **`run_dyn` is a multi-pass loop** (`context.rs:835-875` in egui-0.36.1 — the loop
   itself opens at `835` and closes at `875`; the enclosing `fn run_dyn` begins at
   `823`); `request_discard` reruns `begin_pass`/`end_pass`, swapping `causes` again,
   so after a discarded pass `repaint_causes()` is the second-to-last pass's causes.

`set_request_repaint_callback` looked like the documented escape hatch but is not: it
fires only when `delay < repaint_delay`, so once the events-zero lands at
`begin_pass` every later request is silent.

The correctness argument therefore requires reasoning about three interacting
internal mechanisms and is unfalsifiable without **121.28**. The measured prize over
the 500ms fallback is ~**0.075% of a core** (2 wakes/s × the 376µs idle frame cost
from 121.8), which 121.24 independently corroborates. Blocked on 121.28; would also
need a new `EGUI_UPGRADE_ASSUMPTIONS.md` entry. **Do not re-derive this analysis.**

**Confirmed live (harness, 2026-07-29):** `repaint_cause_top8` shows
`95x .../egui-0.35.0/src/context.rs:525` during pointer motion — that is exactly the
events-driven `begin_pass` cause this analysis rests on, so the mechanism is real and
observable. It also means the discriminating test is *implementable*; the objection
remains the five internals it depends on, not whether the signal exists.

### 121.30 — Chrome widgets are not constructed at all on `Replay`

`SUPPRESSED_POINTER_FALLBACK_DELAY`'s original doc framed the residual risk purely as
scheduling cadence. There is a second, distinct mechanism: freminal's chrome widgets
(menu bar, tab bar) are not constructed on `ChromeMode::Replay`, so while continuous
pointer motion keeps the gate settled, an egui-internal chrome animation would not
merely be scheduled less often — its own advancing logic would not run, and it would
freeze rather than degrade.

**Latent, not live** in freminal's own code: it uses no `ctx.animate_bool` /
`ctx.animate_value` anywhere in chrome (verified by search), and an open menu forces
`Full` through `any_overlay_open` via an unrelated gate. 121.13 widened the latent
window to the "app requested nothing" case as a deliberate, accepted trade.

**But egui itself raises repaint causes we do not control (harness, 2026-07-29).**
`repaint_cause_top8` logged `12x .../egui-0.35.0/src/containers/area.rs:640`, plus
`:552` and `:684`, during a pointer-motion run. The earlier "no `ctx.animate_*`"
check covered **freminal's** code, not egui's `Area`. That does not make this live —
those frames were not suppressed — but it removes the comfort that egui is quiescent,
and it is independent support for keeping 121.12's fallback bounded rather than
unbounded (121.29).

The trigger to action this is the introduction of any `ctx.animate_*`-driven chrome
widget, or evidence of an `area.rs` cause arriving on a frame that *was* suppressed.
Both mechanisms are documented at the constant.

> **Correction (2026-08-02, from 121.32).** "Latent, not live" was wrong. This entry
> reasoned about non-construction on `Replay` purely as an **animation** problem and
> cleared it because freminal uses no `ctx.animate_*` in chrome. The reachable
> consequence was **input**: egui resolves hit-testing and click/drag validity against
> the previous frame's widget set, so widgets that are not built are not merely
> unanimated, they are uninteractable — and in beta.7 that broke tab clicks and
> pane-border drags badly enough to require reverting 121.13. The transferable lesson:
> "the widgets are not built" is a statement about the widget **set**, and egui uses
> that set for interaction as well as painting. Enumerating one consumer of it
> under-scoped the risk.
>
> **Refined 2026-08-16.** "That broke tab clicks and pane-border drags" is too
> broad: non-construction accounts for the **tab clicks only**. Pane-border drag
> sensors are built on both paths (121.33), so they are never absent and this
> mechanism cannot be what breaks them. The lesson above still stands unchanged —
> the widget set is used for interaction, not just painting — but see 121.32's
> 2026-08-16 correction for the click/drag split.

### 121.31 — Every frame is a full present during pointer motion

Observed while capturing 121.17's numbers, not yet diagnosed.
`frame_damage_full=120, frame_damage_partial=0` in both the clean-pane and URL runs —
i.e. **every** frame during pointer motion was a full present, where 121.8 recorded
120/120 *partial* at idle. `swap_mean_us=210` is 29% of the clean run's 729 µs frame.

Subtask 121.5 region-gated `pointer_forces_full_present` specifically so motion over
terminal content would not force a full present. Either that gate is over-firing, or
something else is forcing `Full` — note `toast_active=48` fired in every run, and
`chrome_signals_fired` also shows `warming_up=3`, so a startup toast is a plausible
confound rather than a bug.

**Diagnose before fixing**, and re-run without the startup toast present. Second-order
against 121.17 (which cuts the frame count on the path where this costs most), so
schedule it after. Cheap read-only recon; do not change the damage logic speculatively.

### 121.32 — The chrome cache is structurally unsound; disabled by default

**Live regression shipped in 0.12.0-beta.7. Resolved 2026-08-02 by disabling the #436
chrome cache by default (`FREMINAL_CHROME_CACHE=1` re-enables it). Confirmed fixed by
the maintainer under real daily use.**

Symptoms: clicking a tab mostly did nothing; pane-border drag-to-resize was inconsistent
to unusable. Markedly worse while a TUI (`btop` etc.) was running in a pane. Hover
highlighting on tabs appeared to work throughout.

#### What was tried and FALSIFIED — do not re-derive these

Recorded so future work does not repeat the sequence. Each was a plausible,
code-reading-derived hypothesis; each was disproved by a maintainer build test.

| # | Hypothesis                                                      | Result                                                                                   |
| - | --------------------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| 1 | Half-wired #436.8 drag latch: OR the latch into the frame drain | **Falsified.** Reduced tab-click failures, made border drags *worse*. Withdrawn.          |
| 2 | 121.13 raised `Replay` duty cycle and is the cause; revert it   | **Falsified.** Revert verified present in the build; symptom persisted.                   |
| 3 | Dependency change in the beta.6→beta.7 window                   | **Ruled out** statically: only `toml 1.1.3→1.1.4`. egui stack pinned and untouched.      |

Hypothesis 1 was wrong because it protects the span *after* a press registers, and the
decisive frame is the one *before* it (see below). Hypothesis 2 was wrong because
121.13 only changes *how often* `Replay` is chosen — 121.14 also raises it, and the
underlying defect does not need either commit to be reachable.

**Methodological note, which is the durable lesson.** All three hypotheses came from
reading code, and two survived adversarial sub-agent review before being disproved by a
single click test. This bug is timing-, GL- and window-dependent; static reasoning
repeatedly produced confident wrong answers. 121.13 shipped the same way — its own test
module states its wiring has no automated coverage. **Do not accept a code-reading
argument about this subsystem. Measure it** (see "How to measure" below).

**Established fact (verified against `egui-0.35.0` source at the time; re-verified
against `egui-0.36.1` on 2026-08-16 — unchanged in substance, only line numbers
moved).** egui resolves interaction entirely against the **previous** frame's widget
set:

- `context.rs:487` — `hit_test(&viewport.prev_pass.widgets, …)`
- `context.rs:500` — `interact(…, &viewport.prev_pass.widgets, …)`
- `interaction.rs:109-123` — if `potential_click_id` is not in that set it is cleared
  (*"The widget we were interested in clicking is gone"*). **`potential_drag_id` is
  handled differently, and this entry previously stated it wrong.** When absent from
  the set, egui deliberately leaves it alone — the comment at that site reads "this
  could be drag-and-drop, and the widget being dragged is now 'in the air' and thus
  not registered in the new frame." Corrected 2026-08-16; see the consequences below
  for what that changes.

Consequences:

1. A press on frame N can only hit a widget that was **built on frame N-1**. This
   holds for **both** clicks and drags — it is what breaks a gesture from *starting*
   at all, regardless of which kind of gesture it is.
2. A **single** `Replay` frame anywhere in a gesture discards the in-flight **click**
   (`potential_click_id` is cleared). This does **not** hold for drags: per the
   corrected fact above, an in-flight drag survives a `Replay` frame by design.

**Correction (2026-08-16), and what it does NOT say.** These two consequences were
previously stated as one, covering "click/drag" together. They diverge: consequence 2
is click-only.

Apply that split to the two observed symptoms separately, because they have different
mechanisms and conflating them is what this correction is fixing:

- **Tab clicks** are fully explained by consequence 1. The tab strip is built only on
  `ChromeMode::Full`, so on a `Replay` frame it is genuinely **absent** from
  `prev_pass.widgets` — and consequence 2 then discards any click that was already in
  flight. Both apply.
- **Pane-border drags are explained by NEITHER.** Consequence 1 does not apply,
  because per 121.33 those sensors are built on **both** paths and so are never
  absent. Consequence 2 does not apply, because an in-flight drag is deliberately
  preserved. This correction therefore **eliminates the last mechanism in this entry
  that could have accounted for the drag half of the symptom**, leaving 121.33's
  `Ui` id churn as the explanation — which is exactly what 121.33 already says ("they
  fail by id churn rather than by absence"). Before this correction, consequence 2's
  "discards the in-flight click/drag" wording made it look as though 121.32 alone
  covered drags too. It never did.

The net effect is to **promote 121.33 from "probably an active participant" to the
only remaining candidate mechanism for the drag symptom**, and to make it a hard
prerequisite for any re-enabling work rather than a nice-to-have.

**This does not reopen the decision below.** Consequence 1 is sufficient on its own to
condemn the cache on the tab-click evidence, the drag symptom is still explained (by
121.33), and the maintainer confirmed the fix under real daily use. Nothing here
weakens that.

**Forward-looking note (2026-08-16).** 0.36.1 adds a filter step —
`.filter(|layer_id| self.memory.areas().is_interactable(*layer_id))` — when building
the candidate layer list passed into `hit_test` (new `Areas::is_interactable` at
`memory/mod.rs:1214`, wired around `context.rs:475-481`). Non-interactable layers are
now excluded from hit-testing before it runs. This is **orthogonal** to the
`prev_pass` unsoundness above and changes none of this subtask's conclusions — it did
not exist when the original analysis was written, so any future reasoning about which
layers get hit-tested must account for it.

Since freminal builds the tab strip only on `ChromeMode::Full`, any `Replay` frame
adjacent to a click is fatal to it. **Hover is not evidence against this**: on a `Replay`
frame the cached chrome texture is replayed *including the highlight drawn on the last
`Full` frame*, so hover looks live while the widget set backing it is absent. That
appearance misled the first two diagnosis attempts.

Why a TUI made it worse: with the pointer at rest, `CursorMoved` stops firing, so nothing
sets `chrome_input_pending`. A TUI supplies a continuous stream of PTY-driven frames,
every one of which is then eligible for `Replay`. With no TUI, an idle app produces no
such frames, the last `Full` frame persists, and the click lands.

#### Resolution: the cache is off by default

`chrome_cache_enabled()` in `egui_integration.rs` gates the whole mechanism and
**defaults to disabled**; `chrome_mode` is then unconditionally `Full`. Set
`FREMINAL_CHROME_CACHE=1` to re-enable, which exists so both states can be A/B'd in one
binary against the `frame-profiling` counters without a rebuild between samples.

This is not a workaround pending a "real" fix. The unsoundness is structural: `Replay`
skips *constructing* the widgets, and egui needs the widget set for **interaction**, not
only painting. There is no scheduling policy that repairs that, because the frame whose
mode decides whether a click can land is the frame *before* an event that has not
happened yet. The only sound designs are:

1. Make `Replay` still construct the chrome widgets and skip only tessellation/paint —
   i.e. cache the *output*, not the *pass*. This preserves the widget set and is the
   only variant that keeps the optimisation.
2. Delete the chrome cache. **Actively under consideration by the maintainer**
   (2026-08-02): the measured win is small and process memory has grown substantially
   since #436 introduced it. If this is chosen, 121.13, 121.14's chrome half, and the
   whole `ChromeGatePredicates` apparatus go with it.

Do not re-enable the cache without picking (1) or (2) explicitly.

**Two further contributing defects were identified. Neither is fixed, and neither is
sufficient on its own to explain the symptom.**

- **The #436.8 drag latch is only half-wired.** `chrome_input_pending` is
  edge-triggered (set only by an arriving input event) and drained per frame;
  `chrome_drag_pressed_count` is level-triggered but is consulted only at the four
  pointer-event sites, never at the `RedrawRequested` drain. A frame driven by anything
  other than a pointer event therefore ignores a held chrome drag. This is genuinely
  wrong independent of 121.13.
- **The `Full`/`Replay` `Ui` id divergence (121.33) is probably an active participant,
  not the latent risk it was filed as.** See that entry.

**How to measure (use this instead of arguing).** The `frame-profiling` counters
already exist and answer the question directly:

```text
cargo build --release --features frame-profiling
RUST_LOG=none,freminal_windowing=debug ./target/release/freminal
```

A line flushes every 120 frames carrying `chrome_mode_full`, `chrome_mode_replay`,
`chrome_replay_duty_cycle_pct` and the four `gate_blocked_*` breakdowns; deltas between
consecutive lines give per-interval behaviour. The acceptance test for any re-enabled
cache is two-condition and falsifiable: hovering over chrome must produce **zero**
`Replay` frames, and hovering over the terminal must produce **many**.

**Verification status: CONFIRMED** (maintainer, 2026-08-02, system build under real
daily use). Disabling the cache resolved both the tab-click and pane-border-drag
failures. The bug had survived three earlier code-reading fixes, so this was held at
"provisional" until it had been exercised as the daily-driver terminal rather than in a
test session.

**A corroborating symptom, recorded because it is the recognisable signature of this
bug class.** The maintainer noted that even when a tab click *did* register, it was
"subtly delayed — hard to put into words", and that the delay is gone with the cache
off. That is the same defect expressed as latency rather than loss: a click landing on
a `Replay` frame could not be serviced, so it waited until the gate happened to pick
`Full`. Whether an interaction is *lost* or merely *late* depends only on how soon the
next `Full` frame arrives. **If a future change reintroduces the cache, watch for input
latency as well as dropped clicks** — the latency appears first and at a much higher
rate, and is the earlier warning.

The A/B against `FREMINAL_CHROME_CACHE=1` was not run; it was not needed once the
default-off build held up in real use, and the escape hatch remains available if anyone
wants to pin the correlation formally later.

### 121.33 — `Full` / `Replay` `Ui` id divergence

Surfaced by 121.32. `central_body`'s `Ui` is allocated by `CentralPanel::show` on `Full`
(an unsalted `new_child`, id auto-derived from the root id plus a per-frame child-index
counter) and by a bare `egui::Ui::new(ctx, Id::new("freminal_root"), …)` on `Replay`.
Any widget inside it keying persistent state off its `Ui`-derived id churns that state
across a mode toggle.

Pane-border drag sensors are exactly such a widget, and unlike the tab strip they **are**
built on both paths — so they fail by id churn rather than by absence.

The in-code comment calls this "inert" because "real user interaction with such a widget
forces `ChromeMode::Full` on the same frame". **That premise is false** — it is precisely
what 121.32 disproved — and the comment should be corrected whether or not the
divergence is fixed. Forcing `Full` on the same frame is also insufficient on its own,
per 121.32's point 1.

Scope of fix: give both paths the same explicit, stable id salt so neither depends on
egui's child-index counter. Approach: pin the current `Full`-path id with a test first,
then make `Replay` match. Prerequisite for any re-landing of 121.13.

### 121.34 — Measure what always-`Full` actually costs (DECISION GATE)

**This subtask gates the keep/delete/confine decision for the chrome cache. Do not
choose between the three designs in 121.32 without it.**

121.32 disabled the cache and restored a usable terminal. What that costs is currently
**unmeasured**, and the original #436 justification is no longer trustworthy as a
number: it was taken before 121.8 made the cache actually engage, before 121.12–121.14
changed the scheduling, and before the cache was found to be unsound. Re-derive it.

**Separate the two mechanisms before measuring — they are routinely conflated.**

| Mechanism            | What it decides                                                       | Status after 121.32  |
| -------------------- | --------------------------------------------------------------------- | -------------------- |
| Frame suppression    | Whether a `CursorMoved` schedules a repaint **at all** (`should_schedule_cursor_moved` / `pointer_motion_needs_repaint`, `event_loop.rs`) | **Intact, untouched** |
| Chrome cache         | Given a frame happens, rebuild chrome widgets or replay a cached texture (`ChromeMode`) | **Disabled globally** |

The "don't render when the pointer is just moving over terminal content" win is
mechanism 1 and still works. 121.32 disabled mechanism 2, and disabled it on **every
rendered frame** — including PTY-driven frames with the pointer over the terminal or
outside the window entirely. The cost is therefore global, not confined to chrome
interaction, which is what makes 121.36 worth considering.

**What to measure.** Per `performance-benchmarks`, before/after on the same machine in
one sitting, cache off vs `FREMINAL_CHROME_CACHE=1`:

```text
cargo build --release --features frame-profiling
RUST_LOG=none,freminal_windowing=debug ./target/release/freminal
```

Capture, for each arm, over the same workload:

- `chrome_mode_full` / `chrome_mode_replay` / `chrome_replay_duty_cycle_pct` — with the
  cache off the duty cycle is 0 by construction; the ON arm establishes how often
  `Replay` *would* have been taken, which is the upper bound on any possible saving.
- `phase_total_total` / `phase_total_max`, and the tessellation phase specifically —
  chrome re-tessellation is the actual work being avoided.
- Process CPU at idle, and with a TUI (`btop`) running in a pane.
- RSS, sampled over several minutes in each arm (see 121.35 — the current build still
  *populates* the cache it never reads, so an honest RSS comparison needs 121.35 landed
  first or the ON/OFF arms are not comparable).

**Workloads, at minimum:** idle with a visible cursor; idle with the cursor hidden
(DECTCEM — the btop case); pointer moving over terminal content; pointer at rest over
chrome; TUI redrawing continuously.

**Decision rule, fixed in advance so the result cannot be rationalised:**

- Saving below roughly 5% of frame time and no material RSS difference → **delete the
  cache** (121.32 design 2). Take the machinery out: `ChromeCache`,
  `ChromeGatePredicates`, `evaluate_chrome_gate`, the `gate_blocked_*` counters, the
  reverted 121.13, and 121.14's chrome half.
- Saving material **and** concentrated in the pointer-over-terminal path → **confine**
  (121.36).
- Saving material and spread across all frames → the only sound option is 121.32
  design 1 (cache the output, construct the widgets), which is a larger job and should
  get its own subtask rather than being smuggled into 121.36.

**Prohibitions:** do not re-enable the cache by default as part of this subtask. Do not
change any production logic — this is measurement only. Do not report a single
aggregate number; the whole point is which workload the cost lands in.

### 121.35 — Stop populating the chrome cache while it is disabled

**Deferred by maintainer decision (2026-08-02): written up now, not scheduled.
Task 122 takes priority.**

> **The stated reason has lapsed (2026-08-16).** Task 122 merged on 2026-08-03,
> so "Task 122 takes priority" no longer defers anything. This entry is left
> `Deferred` rather than re-scheduled, because that was a maintainer call and
> only the *reason* expired, not the decision. Note the sequencing constraint
> below still binds: it should land before 121.34's RSS arm, or the cache-on and
> cache-off arms are not comparable on memory.

121.32 bypassed the cache **read** but not the **write**. `chrome_mode` is forced to
`Full`, and the `Full` arm still populates `ChromeCache` on every single frame:

```text
let head_shapes = shapes[..start].to_vec();                      // clone
let tail_shapes = shapes[end..].to_vec();                        // clone
let head_primitives = self.ctx.tessellate(head_shapes.clone(), ppp);  // clone
let tail_primitives = self.ctx.tessellate(tail_shapes.clone(), ppp);  // clone
self.chrome_cache = Some(ChromeCache {
    head_shapes, tail_shapes,
    head_primitives: head_primitives.clone(),                    // clone
    tail_primitives: tail_primitives.clone(),                    // clone
    ppp, size,
});
```

Six vector clones per frame to fill a cache nothing reads. Because every frame is now
`Full`, the allocation churn is *higher* than before the cache was disabled.

Steady-state retention is bounded — one `ChromeCache`, overwritten each frame — so this
is **not** unbounded growth, and this entry should not be cited as one. But sustained
churn of large shape and primitive vectors is a plausible contributor to the RSS growth
observed since #436, and freminal already carries `malloc_trim` discipline (Task 118)
precisely because allocator behaviour under churn matters here.

Scope of fix: skip the `ChromeCache` construction (and the two extra `clone()`s feeding
`tessellate`) when `chrome_cache_enabled()` is false. Keep the tessellation itself — the
frame still needs its primitives.

**Sequencing note: this should land BEFORE 121.34's RSS arm**, or the cache-on and
cache-off arms are not comparable on memory. It is independent of the keep/delete/confine
decision and worth doing either way — if the cache is deleted, this code goes with it.

### 121.36 — Confine `Replay` to frames where the pointer is not over chrome

**Conditional on 121.34.** Only worth doing if measurement says the saving is real and
concentrated in the pointer-over-terminal path. **Blocked on 121.33.**

The insight 121.32 arrived at only after the fact: the cost of always-`Full` is paid on
every frame, but the *soundness* requirement only binds where the user can interact with
chrome. Confine the loss to that region instead of paying it globally.

Design: permit `Replay` only when the pointer is **provably not** over a
chrome-interactive region and no chrome drag is latched — evaluated **every frame**
(level-triggered), not on pointer-event arrival (edge-triggered).

**Why this is sound where 121.13, 121.14 and the withdrawn drag-latch fix were not.** All
three tried to predict, at event time, that a frame would need to be `Full`. That cannot
work: egui hit-tests a press against the **previous** frame's widget set, so the
deciding frame precedes an event that has not happened yet. A level-triggered
position test does not predict anything — while the pointer is over chrome, every frame
is `Full`, including the one before any click. `last_cursor_pos` and
`is_chrome_interactive_at` already exist and already cover tab strip, menu bar, pane
borders and toasts.

**Two things that must be settled first, or this reintroduces the bug in a new shape:**

1. **121.33 is a hard prerequisite.** Every `Full`↔`Replay` transition churns the
   `Ui`-derived id of anything built in `central_body` on both paths — the pane-border
   drag sensors. Confining `Replay` makes those transitions *more* frequent at the chrome
   boundary, which is exactly where border drags start.
2. **`last_cursor_pos == None` must be handled deliberately.** At pointer-event time the
   existing convention treats unknown position as "over chrome" (conservative, force
   `Full`). At *frame* time `None` means the pointer is not in the window, where forcing
   `Full` forever would defeat the whole subtask. The two conventions differ and the
   frame-time one must be written down where it is used.

**Acceptance test, using the counters already in `egui_integration.rs` — two conditions,
both falsifiable, no code-reading argument accepted:**

- Pointer at rest over the tab strip with a TUI running → `chrome_mode_replay` delta of
  **exactly zero** across several flush intervals.
- Pointer over terminal content with a TUI running → `chrome_mode_replay` delta **high**.

Plus manual confirmation of tab clicks and pane-border drags with a TUI running, since
that is the workload that reproduced the original bug and no automated coverage of this
wiring exists (issue #440 / 121.28).

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
