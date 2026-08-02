# PLAN_122_ORCHESTRATION_EXTRACTION.md — Task 122 "Orchestration Extraction"

> **STATUS: ACTIVATED, AWAITING SIGN-OFF.** This document is the output of the
> Task 122 activation pass (2026-07-30, on `main` at `f8ebd17a`). It replaces
> `Documents/DECOUPLING_FRAMEWORK.md` §8 Phase 1 as Task 122's plan content.
> Per `freminal-version-activation`, decomposition and execution are separate
> sessions: the subtasks below need maintainer sign-off before any is spawned.
>
> **Exception: 122.0 is done.** The maintainer moved the agent-skill change to
> the front and approved it on 2026-07-30, on the grounds that running it last
> would mean every other subtask executes under the skills that caused the
> drift. No production code has been touched.

Task 122 is carried by v0.12.0. See `Documents/PLAN_VERSION_120.md` for the
version summary and `Documents/MASTER_PLAN.md` for roadmap position.

---

## Goal

Give orchestration logic — event triage, view window, input encoding, frame
decisions — a home, so that the GUI binary's god functions stop being the
default destination for every new cross-cutting decision.

The maintainer's framing, which this plan is written to serve:

> "The app should just literally display what it's told to display. Instead the
> main app logic is involved in decisions that rely on things it doesn't need to
> know."

**No behaviour change.** `cargo test --all` green at every step, app usable
throughout, one atomic commit per subtask.

**What this task does NOT buy.** Task 122 retires **none** of the 13 assumptions
in `Documents/EGUI_UPGRADE_ASSUMPTIONS.md`. Per `DECOUPLING_FRAMEWORK.md` §3
those die only when chrome leaves the main window's `Context`, which is Phase 3
of the rewrite. Task 122 addresses the "ugliness" argument and part of the "edge
cases" argument, and nothing of the undocumented-internals argument. Do not
claim otherwise in commit messages or the PR.

---

## Re-measurement (2026-07-30, `main` at `f8ebd17a`)

| Target                                          | §8 figure | At 122's creation | **Now**   |
| ----------------------------------------------- | --------- | ----------------- | --------- |
| `App::update` (`app_impl.rs:1081-4212`)         | 2,743     | 3,051             | **3,132** |
| `central_body` closure (`app_impl.rs:1826-3858`) | 1,859     | 1,989             | **2,033** |
| `widget.rs::show` (`widget.rs:1818-3699`)       | 1,851     | 1,851             | **1,882** |
| `write_input_to_terminal` (`input.rs:1379-2604`) | 1,226     | 1,226             | **1,226** |
| `panes/mod.rs` `Rect` / `Pos2` occurrences      | 44        | 58                | **58**    |

The drift is **entirely in the frame path**, and +81 of the latest `App::update`
growth came from PR #467's Group B bug fixes. `write_input_to_terminal` and the
`panes/mod.rs` count have not moved across three measurements.

This continued drift is what the stub anticipated and told the activation pass
to re-measure; it is confirmation, not a new discovery. The new findings are
below.

### Corrections to §8's stated facts

- **`write_input_to_terminal` has 17 parameters, not 16.** §8 and
  `PLAN_VERSION_120.md` both say 16. The 17th is
  `super_state: SuperKeyState` (`input.rs:1396`), added by subtask 101.2. Its
  return is a 7-tuple behind the alias `WriteInputResult` (`input.rs:1252-1260`).
- **`show` has 23 parameters plus `&mut self`** (`widget.rs:1818-1843`), guarded
  by `#[allow(clippy::too_many_arguments)]` at `widget.rs:1812`. §8 never
  counted it.
- **`freminal-windowing` has zero freminal dependencies.** Its `Cargo.toml`
  deps are all external (`egui`, `winit`, `glutin`, `conv2`, …). This matters
  for the trait-boundary decision below.

---

## Why §8's shape is wrong, and what replaces it

§8 Phase 1 is a flat list of five "decompose this big function" items plus a
rename. Re-measurement says that weighting is wrong: two of the five targets are
**static**, and the growth is concentrated in one specific, nameable mechanism.

**The mechanism:** render-time state written during a frame purely so a consumer
that runs **outside** any frame can read it, with no name, no type, and no
enforced invariant. It accumulates on `PerWindowState` because that is the only
long-lived thing the `central_body` closure captures.

PR #467 is the case study. `pointer_motion_needs_repaint` — an event-layer
predicate that runs outside any frame — now needs toast animation phase, the
resize-HUD fade schedule, toast pill geometry, pane layout, and gutter strip
widths. All render-time knowledge with no home, so it gets cached ad hoc
(`chrome_toast_rects` was added doing exactly that). Subtask 121.17 wants to add
two more.

### It is three lifetime classes, not one

An adversarial review of the first draft of this plan correctly refuted the
"one coherent pattern" claim. The fields differ in lifetime, consumer, and risk,
and only one class is genuinely unowned:

- **Type A — intra-frame drain.** `take_frame_damage` / `take_chrome_damage` /
  `take_terminal_band_range` / `take_terminal_requested_delay`
  (`app_impl.rs:1027-1078`), called by `freminal-windowing` synchronously
  immediately after `update()` returns, same tick. These **already have a
  contract** (reset-on-read, documented per field). Low risk.
  **Task 122 does not restructure Type A.**
- **Type B — true cross-event-boundary reads.** `is_chrome_interactive_at`
  (`app_impl.rs:751-769`) and `pointer_motion_needs_repaint`
  (`app_impl.rs:823-1025`), called from `freminal-windowing`'s `CursorMoved`
  fast path (`event_loop.rs:815, 828-831, 868, 885`), which fires **between**
  `update()` calls on a separate control path. **This is the real hazard and
  Group A's target.**
- **Type C — cross-module next-frame caches** on `PaneRenderCache`
  (`widget.rs:1271-1445`). Only the cross-module half qualifies;
  `last_observed_visible` is `widget.rs`-internal (`widget.rs:1362, 1474,
  1527-1536`) and is **excluded** — it is indistinguishable from the ~26 other
  `previous_*` diffing fields. Type C is noted in the inventory but is not
  restructured by Task 122.

### Published-state inventory

Type B — read from outside any frame. **This is Group A's scope.**

| Field                         | Def (`window.rs`) | Type                       | Written (`app_impl.rs`)          | Read out-of-frame |
| ----------------------------- | ----------------- | -------------------------- | -------------------------------- | ----------------- |
| `cached_central_rect`         | 491               | `Option<egui::Rect>`       | 2114 (in `central_body`)         | 879, 906          |
| `cached_gutter_inset_logical` | 509               | `f32`                      | 1895 (in `central_body`)         | 903               |
| `chrome_head_rects`           | 514               | `Option<Vec<egui::Rect>>`  | 1753 (`Full` only)               | 755               |
| `chrome_border_rects`         | 518               | `Vec<egui::Rect>`          | 2461, cleared 2467               | 756               |
| `chrome_toast_rects`          | 581               | `Vec<egui::Rect>`          | 3970, cleared 3915               | 766               |
| `pending_chrome_signals`      | 422               | `ChromeSignals` (14 bools) | 3374-3390                        | 866-867           |
| `resize_overlay`              | 204               | `Option<ResizeOverlayState>` | 3113 set, 3663 cleared; 524 init | 849-856         |

Type A — drained same tick, already contracted. **Not restructured.**

| Field                             | Def | Written                          | Drained   |
| --------------------------------- | --- | -------------------------------- | --------- |
| `pending_frame_damage`            | 359 | 3240 pre-comp, **4041 post-comp** | 1027-1039 |
| `pending_chrome_damage`           | 411 | 4025-4029                        | 1050-1062 |
| `pending_terminal_band_range`     | 380 | 3758                             | 1041-1048 |
| `pending_terminal_requested_delay` | 477 | 3830, 3984-3987                  | 1064-1075 |

Type C — cross-module, on `PaneRenderCache`. **Noted, not restructured.**

| Field                      | Def (`widget.rs`) | Consumer                                |
| -------------------------- | ----------------- | --------------------------------------- |
| `pending_repaint_delay`    | 1444              | `app_impl.rs:2861` after `show()`       |
| `last_frame_cursor_damage` | 1436              | `app_impl.rs:3188, 3233, 3237`          |
| `placeholder_hit_rects`    | 1406              | `input.rs` `write_input_to_terminal`    |

### 122.1 audit result (2026-07-30, `task-122/orchestration-extraction`)

**All 14 rows of all three tables verified line-for-line against current code.
Nothing moved.** Every definition line, write site, drain site and read site
above is exact, as are `is_chrome_interactive_at` (`app_impl.rs:751-769`),
`pointer_motion_needs_repaint` (`823-1025`), the `CursorMoved` fast-path call
sites (`event_loop.rs:815, 828-831, 868, 885`), and the same-tick Type A drain
(`event_loop.rs:1182-1185`, immediately after `app.update()` at `1180`). The
`resize_overlay` row's "feature state" gloss has been replaced with its real
write sites. Field types are added because 122.4 needs them.

`last_observed_visible` (`widget.rs:1362`, init `1474`, used `1527-1536`) was
re-checked and is still `widget.rs`-internal — correctly excluded from Type C.

#### The Type B set is complete — with two exclusions now stated explicitly

Every `win.<field>` read inside the two out-of-frame predicates was enumerated.
The read set is the seven fields above plus `win.tabs` (`app_impl.rs:870`) and
`win.frame_stats` (`1004`). Neither belongs in 122.4's type:

- **`win.tabs`** is live structural domain state (the pane tree), mutated
  synchronously by input handling. It reflects current truth at any instant —
  it is not a render-time snapshot cached for later reuse, which is the whole
  category 122.4 exists to name.
- **`win.frame_stats`** is a write-only diagnostic accumulator behind
  `#[cfg(feature = "frame-profiling")]`, using `Cell` so it can be mutated
  through `&self`. `app_impl.rs:998` already documents that it does not
  influence the predicate's return value.

**Correction to 122.1's own brief.** It asked to "confirm there are no others"
beyond `is_chrome_interactive_at`, `pointer_motion_needs_repaint` and the four
`take_*` drains. As literally worded that is not satisfiable: the `App` trait
has 13 methods (`freminal-windowing/src/lib.rs:194-372`) and six more are
called from outside `update()` — `on_window_created` (`event_loop.rs:659`),
`on_close_requested` (`1074, 1311`), `on_raw_key_event` (`1011-1012`),
`clear_color` (`1143`), `raw_input_hook` (`1159`), `present_partial_flag`
(`1165`). Walking each body, only two touch `PerWindowState` at all, and
**neither is Type B**:

- **`clear_color` reads `win.os_dark_mode`** (`app_impl.rs:731`), a field
  `update()` writes (`1323`). Structurally the same shape as a Type B read, but
  a materially different timing discipline: it is called exactly once per
  frame, deterministically immediately *before* `update()` for that same window
  in the same `RedrawRequested` pass (`event_loop.rs:1143` vs `1180`). It cannot
  accumulate multi-frame staleness the way a `CursorMoved`-driven read can.
- **`present_partial_flag` reads `win.present_is_partial`**
  (`app_impl.rs:748`) but clones an `Arc<AtomicBool>` rather than copying a
  value. It is a shared-mutation handle, exempt by construction.

**Both stay out of 122.4's type**, deliberately: folding them in would put two
different timing disciplines under one name and weaken the invariant the type
exists to carry. `on_window_created`, `on_close_requested`, `on_raw_key_event`
and `raw_input_hook` read no `PerWindowState` field at all (`on_raw_key_event`
only *writes*, pushing to `win.pending_raw_keys` at `4254`).

#### Early-return behaviour

All four claims in 122.1's brief **confirmed**: `app_impl.rs:1179` (settings
window, branch at `1106`) has no `PerWindowState` at all; `1225`
(`windows.remove` returned `None`) has nothing to reinsert; `1591` and `1626`
are byte-identical in effect, both `insert(window_id, win); return;`; and
`window.rs:540-580` already documents the discipline and names the `1626`
bail-out.

The decisive fact for 122.4: **both `1591` and `1626` occur before line 1704**,
which opens the `Full`-vs-`Replay` branch containing the earliest Type B write
(`chrome_head_rects` at `1753`). Every other Type B write site is later still
(`1895`, `2114`, `2461`/`2467`, `3113`/`3663`, `3374-3390`, `3915`/`3970`). So
on both paths `win` is reinserted untouched and **all seven fields retain the
value left by the last fully-completing `update()`** — an early-return
staleness layered on top of the ordinary one-frame staleness. `1179` and `1225`
are vacuous (no `PerWindowState` instance in play).

Minor pre-existing doc gap, recorded not fixed: the comment at
`app_impl.rs:1096-1098` enumerates *three* early-return paths, but there are
four return sites — `1591` ("last tab closes the whole window") and `1626`
("no active pane") are distinct scenarios the comment merges. Behaviourally the
plan groups them correctly; only the code comment undercounts.

#### The invariant 122.4 must preserve

Each of the seven fields is written **at most once per successfully-completing
`App::update`**, at a fixed point in that function — `chrome_head_rects` only
on a `Full` frame (`1753`), the other six unconditionally once reached — and
from nowhere else. None of the four early-return paths writes any of them, so
each holds whatever the last fully-reaching `update()` left. Reads happen
exclusively from `is_chrome_interactive_at` and `pointer_motion_needs_repaint`,
called on `freminal-windowing`'s pointer fast path
(`event_loop.rs:815, 828-831, 868, 885`), a control path fully decoupled from
`update()`: a read may therefore observe the same snapshot across arbitrarily
many pointer events between two frames, and is **one frame stale by
construction even in the best case**. Write *ordering* among the three rect
fields is itself load-bearing and must not be collapsed: `chrome_head_rects`
early and `Full`-only (`1753`), `chrome_border_rects` inside `central_body`
(`2461`, cleared `2467` when no border sensors were built), `chrome_toast_rects`
after `central_body` returns (`3970`, pre-cleared `3915`). A wrapper that let a
caller read `chrome_toast_rects` mid-`central_body` would surface a value the
current frame has not yet had the chance to overwrite — reintroducing exactly
the staleness class `window.rs:540-580` was written to bound.

#### State-representation verdict (per `freminal-state-representation`)

**No bool-to-enum conversion is warranted among the seven Type B fields.** Six
are not bool-related (rects, an `f32`, and two `Option<T>`s — note
`resize_overlay` already uses `Option<T>` rather than a bool-plus-value pair,
which is the correct presence encoding). The seventh,
`pending_chrome_signals: ChromeSignals`, is 14 independent simultaneous signals
and is one of the four types the skill **names explicitly as a legitimate bool
bag that must not be converted**. No field admits an illegal state: no
`Option<bool>`, and no pair among the seven that cannot both be set.

One shape decision 122.4 must make, flagged here rather than left to be
discovered: **`chrome_head_rects` is `Option<Vec<Rect>>` while its two
structural siblings are plain `Vec<Rect>`** (empty-vec-as-absent). The three
disagree today. 122.4 wraps all three and will have to either preserve the
disagreement or unify it — and unifying is a **semantic** change for
`chrome_head_rects`, whose `None` means "no `Full` frame has rendered yet",
which is distinguishable from "a `Full` frame rendered and produced no head
rects". Preserving the types as-is is the default per 122.4's "do NOT change
any field's type" prohibition; this note exists so that prohibition is
understood as deliberate rather than an oversight.

---

## Design decisions (durable)

1. **The layer lands as a module, designed as if it were a crate.** Carried
   forward from §8 subtask 1.6 unchanged. Crate extraction is the
   highest-friction refactor to undo; it is not part of Task 122.
2. **The `App` trait keeps `pos: egui::Pos2`.** The app-side impl converts to
   the neutral `Point` on entry. Rationale: `freminal-windowing` has no freminal
   dependency today, and its charter in `DESIGN_DECISIONS.md` explicitly
   disowns "any freminal-specific semantics". Adding a
   `freminal-windowing` → `freminal-common` edge would be legal per
   `freminal-architecture` but would widen Task 122 from a no-behaviour-change
   refactor into trait API design that the rewrite decision has not been made
   for. The trait boundary's toolkit is Phase 3's business.
3. **The window-level `chrome_*_rects` and `cached_central_rect` stay
   `egui::Rect`.** They are compared against an `egui::Pos2` arriving from a
   trait boundary that decision 2 keeps egui-typed, and their consumer
   `point_in_chrome_rects` lives in `chrome_damage.rs` (1,119 lines, own test
   suite). Re-typing them buys Task 122 nothing and drags in a third file.
   **Consequence: 122.2/122.3 (neutral geometry, `panes/mod.rs` only) and
   122.4-122.6 (the named type) are independent and may run in parallel.**
   The first draft of this plan wrongly sequenced them.
4. **Toolkit-neutral geometry is scoped to `panes/mod.rs`'s own math**, as §8
   subtask 1.4 originally intended — not to the whole binary.
5. **`write_input_to_terminal` is not decomposed in Task 122.** See 122.12.
6. **121.17's unblock is the last implementation subtask** (122.15), by
   maintainer decision. Publishing the terminal rect earlier would mean adding
   a fifteenth ad-hoc cached field before the type that should own it exists.

### The measurable success criterion

Not line counts. **Task 122 succeeds when the out-of-frame predicates'
pane-resolution chain is headlessly testable.**

The pure decision cores are **already well covered** — this plan's first draft
claimed "zero tests" and was wrong:

| Function                                | Tests | Location                |
| --------------------------------------- | ----- | ----------------------- |
| `pointer_motion_needs_repaint_decision` | 9     | `app_impl.rs:4932-5015` |
| `pane_hover_region_risk`                | 5     | `app_impl.rs:4827-4849` |
| `animation_in_flight_composed`          | 4     | `app_impl.rs:4805-4821` |
| `pointer_in_gutter_strip`               | 4     | `app_impl.rs:4900-4923` |
| `pane_hover_region_terms`               | 3     | `app_impl.rs:4868-4884` |

**Count corrected 2026-07-30 (122.14 adversarial review).** The activation
pass recorded 8 for `pointer_motion_needs_repaint_decision` and an end line of
5011; the real figures are **9** tests ending at **5015** — the ninth is
`pointer_motion_needs_repaint_decision_pane_signals_both_false_is_false`
(`app_impl.rs:5004`), whose body extends past the previously-cited range. The
four excluded predicate cores therefore carry **22** tests in total, not 21.

What is genuinely untested is the **glue**:

- the pane-resolution chain (`app_impl.rs:905-990`): layout → hit-test →
  snapshot load → signal computation, an `and_then` chain that mirrors
  `update()`'s own zoomed-vs-split layout choice and would silently
  mis-hit-test if it drifted;
- the `self.windows.get(&window_id)` + `is_none_or` dispatch;
- the real call from `event_loop.rs:828-831` against a live app.
  `DummyApp` (`freminal-windowing/src/lib.rs:619-648`) does not override either
  predicate, so only the conservative trait defaults (`lib.rs:332, 355`) are
  ever exercised.

Two more symptoms of "no home", worth recording because they are what the
maintainer is describing:

- `show` has 67 tests, **none of them on `show`**. `overlay_suppress_input_tests`
  (`widget.rs:4761-4947`) **re-implements `show`'s inline boolean logic inside
  the test** rather than calling anything.
- `write_input_to_terminal` has 50 tests in its file, none on itself. The
  blocker is precisely its first parameter: `&InputState` has no bare
  constructor. `egui::Key` / `Modifiers` / `PointerButton` are plain enums and
  are already used freely in those tests.

---

## Subtasks

**17 subtasks in five groups.** Ordering constraints are stated per group; where
none is stated, subtasks within a group are independent.

**Ordering across groups:** **122.0 runs before everything else** — it is the
skill change that gives later subtasks the mandate they need, and running it last
would mean every other subtask executes under the skills that caused the drift.
Then 122.14 (benchmark), the before-capture for the rest — scoped to the code as
it stands, since the pane-resolution chain it would most like to measure does not
become callable until 122.5, which therefore carries the requirement to extend
that benchmark. Group A and Group C are independent of each other. Group B's
122.9 collides with Group A's 122.4 (both touch `pending_frame_damage` /
`pending_chrome_signals`) and must land after it. 122.15 is the last
implementation subtask, then 122.16 closes.

Every subtask's verification includes, at minimum:

```text
cargo test --all
cargo clippy --all-targets --all-features -- -D warnings
cargo machete
```

and, additionally, because 28 `#[cfg(feature = "frame-profiling")]` sites in
`window.rs` and 26 in `app_impl.rs` are interleaved through exactly the code
being restructured, and `cargo test --all` never compiles them:

```text
cargo test --all --features frame-profiling
```

`cargo xtask check-windows` runs once before the PR, per
`freminal-windows-crosscheck`. None of the five primary files contains
`cfg(windows)` / `cfg(target_os)` code, so the risk is low, but the gate still
applies.

### The documented clippy command does not match the pre-commit hook

**Found during 122.6, 2026-07-30.** `agents.md`'s verification suite says:

```text
cargo clippy --all-targets --all-features -- -D warnings
```

The pre-commit hook runs something different:

```text
cargo clippy --workspace --all-targets
```

These are **not equivalent**, and 122.6 hit a case where the documented command
passes and the hook fails. A `pub(crate) struct DummyApp` inside a private
`mod tests` triggers `clippy::redundant_pub_crate` (denied via
`clippy::nursery`) under the hook's invocation, but **not** under
`--all-features`. Enabling all features changes the compiled configuration
enough to suppress it.

Consequence for this task and any other: **running only the `agents.md` command
is not sufficient to predict a clean commit.** Every subtask in Task 122 should
run both, and a green `--all-features` run should not be reported as "clippy
clean" on its own. Note also that a passing run can come from cache: after a
failure, re-running the *documented* command appeared clean because that
feature set's fingerprint was already cached as successful. Prefer the hook's
invocation as the primary gate.

This is a repo-wide documentation gap rather than a Task 122 issue. It is
recorded here because it was found here; whether `agents.md` should be
corrected is a maintainer decision, noted for 122.16.

---

### Group Z — mandate (runs first)

#### 122.0 — New skill: scope to propose a new home rather than extend in place

Scope: `.opencode/skills/freminal-extend-or-extract/SKILL.md` (new),
`.opencode/skills/freminal-architecture/SKILL.md`, `agents.md`.

**Runs before every other subtask**, by maintainer decision (2026-07-30). This
was originally one third of 122.16 and was moved forward: the whole point is to
give later subtasks a mandate they currently lack, and running it last would mean
all sixteen others execute under exactly the skills that produced the drift.

What: nothing in the skill set gave agents scope to propose a new module or
crate, so the default answer was "extend in place" by omission. That is the
maintainer's own diagnosis of how `App::update` reached 3,132 lines and how
render-time state ended up cached ad hoc across ~10 `PerWindowState` fields.

**This is a dedicated skill, deliberately not a section inside
`freminal-architecture`.** The reason is trigger matching, and it matters:
`freminal-architecture`'s description fires on "the GUI/PTY split, the ArcSwap
snapshot transport, the channel-based input system, crate dependency boundaries,
or `TerminalEmulator` / `TerminalSnapshot` / `ViewState`". An agent about to add
one more cached rect to `PerWindowState` would not match any of those — adding a
field to a GUI struct does not read as architecture work. **Guidance buried in
that skill would not load at the moment it is needed**, which is precisely how
the drift happened. The new skill's description is therefore written in the
language of the moment: adding a field only an outside reader needs, extending an
already-long function, adding a parameter to an already-wide signature, relying
on a `too_many_lines` / `too_many_arguments` allow, copying an unreachable
computation, or writing a test that re-implements production logic to pin it.

`freminal-architecture` keeps a short pointer to it and retains ownership of the
*constraints* on any new home (dependency graph, crate responsibilities); the new
skill owns the *whether to create one at all* decision. `agents.md`'s skill table
gains a row.

Deliverable: the new skill, the pointer, the `agents.md` row.

Verification: markdown lint via pre-commit. No code change, so `cargo test --all`
is unaffected. `opencode.json` already globs `.opencode/skills`, so no config
change is needed.

Prohibitions: do NOT edit the shared skills under
`~/.config/opencode/skills/shared/` — they are nix-store read-only, sourced from
`~/GitHub/nixos`, and by maintainer decision (2026-07-30) this rule stays
**freminal-local rather than shared**. Do NOT weaken any existing invariant. Do
NOT duplicate the guidance in both skills — `freminal-architecture` gets a
pointer only. Do NOT proceed to 122.14.

Stop: report the skill added; await review.

---

### Group A — the seam (Type B)

#### 122.1 — Published-state inventory and classification

Scope: `Documents/PLAN_122_ORCHESTRATION_EXTRACTION.md` only.

What: READ-ONLY audit confirming the three inventory tables above against code
at implementation time, and recording anything that moved since 2026-07-30. Also
records, for each Type B field, its behaviour on `App::update`'s early returns.

The early-return picture is **already resolved and is smaller than the first
draft claimed** — record it, do not re-litigate it:

- `app_impl.rs:1179` — settings window. Has **no `PerWindowState` at all**
  (branch at `1106`). Not a staleness case; a different window kind.
- `app_impl.rs:1225` — `windows.remove` returned `None`. Nothing to reinsert;
  vacuously correct.
- `app_impl.rs:1591` and `app_impl.rs:1626` — **behave identically**, both
  `insert(window_id, win); return;`.
- The staleness discipline is **already documented** at `window.rs:540-580`,
  which names the `1626` bail-out explicitly and reasons about why it is
  tolerable. The code's own comment at `app_impl.rs:1096-1098` names three
  early-return paths.

Deliverable: the three tables in this document, corrected and dated, plus a
one-paragraph statement of the invariant 122.4 must preserve.

Verification: no code change; `cargo test --all` unchanged. Markdown lint via
pre-commit.

Prohibitions: do NOT edit any `.rs` file. Do NOT introduce a "staleness matrix"
subtask — that work is transcription of `window.rs:540-580` and belongs here.
Do NOT proceed to 122.2.

Stop: report the corrected tables; await review.

**DONE.** See "122.1 audit result" above. All 14 rows of all three tables
verified line-for-line; nothing had moved. Field types added for 122.4's
benefit, and `resize_overlay`'s "feature state" gloss replaced with real write
sites. The Type B set is confirmed complete, with `win.tabs` and
`win.frame_stats` excluded for stated reasons; the brief's "confirm there are no
others" was corrected (six further `App` methods run outside `update()`, of
which only `clear_color` and `present_partial_flag` touch `PerWindowState`, and
neither is Type B). Early-return behaviour recorded per field — the decisive
fact being that both `1591` and `1626` precede the earliest Type B write at
`1753`. The invariant statement 122.4 must preserve is recorded. Per the
`freminal-state-representation` skill, no bool-to-enum conversion is warranted
among the seven; the one bool-bearing field is an explicitly exempt signal bag.
One shape decision (`Option<Vec<Rect>>` vs `Vec<Rect>`) is flagged for 122.4.

#### 122.2 — Toolkit-neutral `Rect` / `Point` in `freminal-common`

Scope: new module in `freminal-common/src/`, plus `freminal-common/src/lib.rs`.

What: add a toolkit-neutral geometry module. The required API is **exactly**
this, measured exhaustively from `panes/mod.rs`:

| Item                             | Uses in `panes/mod.rs` |
| -------------------------------- | ---------------------- |
| `Rect::from_min_max(min, max)`   | 34                     |
| `Point` constructor (`pos2`-equivalent) | 68              |
| `.min` / `.max` fields (each `.x`, `.y`) | 28 / 25         |
| `.contains(point) -> bool`       | 15                     |
| `.width() -> f32`                | 10                     |
| `.height() -> f32`               | 7                      |
| `.center() -> Point`             | 4                      |

Plus derives `PartialEq` (relied on by `assert_eq!` at `panes/mod.rs:1871`),
`Copy`, `Clone`, `Debug`.

**Counts corrected 2026-07-30 (122.2 adversarial review).** The **API set above
is right and complete** — re-verified against production code (`panes/mod.rs`
lines 1-1572; the test module starts at 1573) and confirmed sufficient for
122.3. The **occurrence counts are not**, in two ways: they include the test
module, and the `contains` figure conflates three unrelated methods. There is
exactly **one** production `Rect::contains` call (`pane_at_pos`,
`panes/mod.rs:1567`); the other hits are `PaneNode::contains(PaneId)`, a
recursive tree search, and `String::contains` in test assertions. Treat the
table as "what the API must provide", not as a measurement.

**Deliberately NOT provided** (verified absent from `panes/mod.rs`): `shrink`,
`expand`, `translate`, `intersect`, `union`, `from_min_size`,
`split_left_right_at_x`, `left`/`right`/`top`/`bottom`. Do not add them
speculatively.

Precedent to follow: `freminal-common`'s `gui_theme` module is already described
in `lib.rs` as "Toolkit-agnostic GUI styling geometry … no egui dependency".
`freminal-common` has no egui dependency and must not gain one. It already
depends on `conv2`.

Deliverable: the module plus unit tests covering `contains` at boundaries (the
existing `pointer_in_gutter_strip_boundary_at_exact_far_edge_is_false` test at
`app_impl.rs:4911` shows boundary semantics are load-bearing here), `center`
rounding, and zero/negative-extent rects.

Verification: standard suite. New tests must pin `contains` boundary behaviour
to match `egui::Rect::contains` exactly.

Prohibitions: do NOT add an egui dependency to `freminal-common`. Do NOT add
API beyond the table. Do NOT touch `panes/mod.rs` (that is 122.3). Do NOT use
raw `as` casts — `conv2` per `freminal-numeric-conversions`.

Stop: report the module's public API and test results; await review.

**DONE.** `freminal-common/src/geometry.rs` — `Point` (with a free `point(x, y)`
constructor mirroring `egui::pos2`) and `Rect`, all six items `const fn`, no
egui dependency added, `freminal-common/Cargo.toml` untouched. 12 tests.

Float semantics verified operation-by-operation against `emath` 0.35.0 on disk,
including operand order: `contains` inclusive on all four bounds
(`rect.rs:274-276`); `width`/`height` as plain subtraction, negative permitted;
`center` as `(min + max) / 2.0` per component, matching `fast_midpoint`
(`lib.rs:122-128`) rather than `min + (max - min) / 2.0`, which differs by one
ULP for values the test pins; `from_min_max` stores verbatim without
normalising.

`Rect::center` carries `#[allow(clippy::manual_midpoint)]`. The lint wants
`f32::midpoint`, which uses an `f64` intermediate to avoid overflow and so
diverges from emath's naive form — but only near `f32::MAX` (`a = b = f32::MAX`
gives `inf` vs `f32::MAX`), which screen geometry never reaches. The allow is
kept for parity **by construction**, and the code comment now says exactly
that rather than implying the divergence is reachable or tested.

The adversarial review found two false claims in the first draft's doc
comments, both corrected: `Rect::contains` cited
`pointer_in_gutter_strip_boundary_at_exact_far_edge_is_false` as evidence for
inclusive boundaries, but that predicate is scalar arithmetic using a
**half-open** interval and never touches `Rect`; and `Rect::center` claimed
`active_highlight_segment` uses a 0.5 epsilon when `edge_epsilon` is `1.0`
(`app_impl.rs:3556`). The real justification for inclusivity — now documented —
is that `split_rect` gives both halves of a split the *same* boundary
coordinate, so adjacent pane rects **share** an edge; a pointer on that edge is
contained by both and `pane_at_pos` takes the first match. Half-open would make
every split boundary a one-pixel dead stripe.

#### 122.3 — Move `panes/mod.rs` production geometry onto the neutral types

Scope: `freminal/src/gui/panes/mod.rs`, plus
`freminal/benches/pane_resolution_bench.rs`.

**Scope widened 2026-07-30 (122.14 adversarial review).** It was
`panes/mod.rs` only, which was unsatisfiable: 122.14 added
`freminal/benches/pane_resolution_bench.rs`, which benchmarks
`PaneTree::layout`, `PaneTree::split_borders`, `pane_at_pos` and
`active_highlight_segment` through their current `egui::Rect` / `Pos2`
signatures. Re-typing those four onto the neutral geometry breaks that file's
compilation, so `cargo bench --no-run --all` would fail and 122.3 could not
reach a green verification suite without touching it. Note the bench builds
rects with `egui::Rect::from_min_size` and `egui::vec2`, and the 122.2 neutral
type deliberately provides **neither** — the bench's constructions convert to
`from_min_max` form. That is an adaptation of bench fixtures only; **do not add
`from_min_size` to the neutral type** to avoid it.

What: migrate the 58 `Rect` / `Pos2` occurrences. Every **production** use in
this file is pure geometry — `from_min_max`, field reads, `width`/`height`/
`center`/`contains` — with **zero** `egui::Response`, `ui.interact`, painter or
layout calls. Affected items:

- `SplitBorder.rect` (`panes/mod.rs:460`)
- `active_highlight_segment` (`507-555`) — signature is `Rect` in and out
- `PaneNode::layout` (`664-681`), `PaneTree::layout` (`1079-1092`)
- `PaneNode::split_borders` (`693-754`), `PaneTree::split_borders` (`1094-1119`)
- `split_rect` (`1513-1529`) — note `mul_add` and `.round()`
- `pane_at_pos` (`1553-1569`) — the single `.contains(pos)`

Callers outside this file convert at the boundary; that conversion is part of
this subtask's scope only where it is a one-line adaptation at the call site.
There are **nine** such sites, in two files: `gui/actions.rs:153` and
`gui/app_impl.rs:918, 2170, 2344, 2433, 3552, 3575, 3611, 3938`.

**A conversion seam is required, and the plan did not anticipate it (added
2026-07-30, before implementation).** Those call sites pass `egui::Rect` in
(from `ui.available_rect_before_wrap()`) and paint with what comes out, so both
directions are needed. Neither direction can be a `From` impl: `freminal-common`
must not depend on egui, and in the `freminal` binary both `egui::Rect` and
`geometry::Rect` are foreign types, so the orphan rule forbids it. The seam is
therefore **free functions in a new small module,
`freminal/src/gui/geometry_interop.rs`**, named for exactly that one concept
per `freminal-module-cohesion`: converting between egui's geometry types and
the neutral ones. It is placed in the `freminal` binary because that is the
only crate that legally sees both. Do **not** put these helpers on
`panes/mod.rs` — the conversion is not a pane concept, and two of the nine call
sites are not pane-tree calls.

Deliverable: migrated file, existing tests (`panes/mod.rs:1573`+) passing
unchanged in intent.

Verification: standard suite. **Float behaviour must be identical** — `split_rect`
uses `mul_add` and `.round()`, and the existing tests assert exact widths and
heights. Any test needing a tolerance change is a red flag, not a fix: stop and
report.

Additionally, **re-run the 122.14 benchmark** — it covers exactly the functions
this subtask re-types. Per the measured noise floor recorded under the 122.14
baseline, check it for **algorithmic-shape change within a run**, not for a
per-ID 15% wall-clock delta, which this hardware cannot resolve.

**DONE.** `freminal-common::geometry::{Rect, Point, point}` now carries
`SplitBorder.rect`, `active_highlight_segment`, `PaneNode`/`PaneTree::layout`,
`PaneNode`/`PaneTree::split_borders`, `split_rect` and `pane_at_pos`. The
`panes/mod.rs` diff is mechanical — `egui::pos2(` to `point(`, one import — and
`split_rect`'s `mul_add(...).round()` expressions are byte-identical, so the
float behaviour is unchanged by construction. `split_rect` additionally became
`const fn`, forced by `clippy::missing_const_for_fn` now that the neutral
type's methods are `const` where egui's are not; that is compile-time only.

Conversion seam added as planned: `freminal/src/gui/geometry_interop.rs`, four
`pub const fn` free functions. Nine boundary call sites converted in
`app_impl.rs` (8) and `actions.rs` (1). No test assertion or tolerance was
altered.

**On performance:** the change is neutral by construction, and that is the
claim being made — `geometry::Rect` has the same layout as `egui::Rect` (two
pairs of `f32`), performs the same arithmetic in the same operand order, and
the only delta is compile-time `const`-ness. The benchmark corroborates: the
algorithmic shape is unchanged across pane counts in every group. Per-ID
wall-clock deltas were **not** used to support this, because the same-code
noise floor measured here (12 of 39 IDs above 15%) exceeds them.

Prohibitions: do NOT change any layout, resize or hit-test **semantics**. Do NOT
alter rounding. Do NOT touch `window.rs`'s `chrome_*_rects` or
`cached_central_rect` — decision 3 keeps those `egui::Rect`. Do NOT proceed to
122.4.

Stop: report files changed and that no test tolerance was altered; await review.

#### 122.3a — `SplitBorder::active_in_first` becomes a named enum

Scope: `freminal/src/gui/panes/mod.rs`, `freminal/src/gui/app_impl.rs` (the
single match at `3575`), `freminal/benches/pane_resolution_bench.rs` (fixture).

**Added 2026-07-30**, at maintainer instruction to take the
`freminal-state-representation` opportunities that this task's extractions
surface, rather than only moving code.

What: `SplitBorder.active_in_first` is `Option<bool>` (`panes/mod.rs:474`)
carrying **three** meanings its own doc comment has to spell out in prose —
`Some(true)` = the active pane is in the first (top/left) subtree,
`Some(false)` = it is in the second, `None` = it is in neither. That is a
named-domain-enum case under `freminal-state-representation`: the value crosses
a module boundary on a `pub` struct, and `Some(false)` is exactly the kind of
double-negative a reader has to decode at the far end. Its only consumer is a
three-arm `match` at `app_impl.rs:3575` choosing which half of a divider gets
the active colour, so the conversion is mechanical.

Replace it with a named enum in the domain's own vocabulary — e.g.
`ActiveSubtree { First, Second, Neither }` — and drop the `Option`. Name the
enum for the concept, not generically; do not introduce a shared
two-state/three-state helper type (that is the anti-pattern the skill names).

Deliverable: the enum, the migrated producer in `PaneNode::split_borders`, the
migrated `match`, and the bench fixture updated.

Verification: standard suite, plus `--features frame-profiling`. Existing
`active_highlight_segment` and `split_borders` tests must pass unchanged —
note `active_highlight_segment` does not read this field at all, so its tests
should be entirely unaffected.

Prohibitions: do NOT change which half of a divider is highlighted for any
input. Do NOT fold this into 122.3 — the geometry migration is the riskiest
diff in Group A and must stay independently reviewable. Do NOT convert any
other bool in this file; `SplitDirection` is already an enum and nothing else
here qualifies.

Stop: report the enum and that the highlight behaviour is unchanged; await
review.

#### 122.4 — Introduce the named published-state type

Scope: new module under `freminal/src/gui/`, plus `freminal/src/gui/window.rs`,
`freminal/src/gui/app_impl.rs` and `freminal/src/gui/layout_ops.rs`.

**Scope corrected 2026-07-30, before implementation.** The stated scope missed a
`PerWindowState` construction site. There are **three**, all of which initialise
all seven fields and so must be migrated together:
`freminal/src/gui/layout_ops.rs:706-741`, `app_impl.rs:~524-559`, and
`app_impl.rs:~4499-4534`. Two further files mention the fields only in prose —
`toast.rs:472, 1571` and `chrome_damage.rs:974-975` (a commented-out example) —
and should have those references retargeted at the new type, but need no code
change. `settings.rs:1887`'s `show_resize_overlay` is an unrelated config field;
do not touch it. `window.rs`'s `resize_overlay_alpha` /
`resize_overlay_is_animating` / `resize_overlay_repaint_delay` helpers take
`Duration` arguments rather than the field, so they and their ~30 tests are
unaffected.

What: introduce a named type owning the **seven Type B fields** from the
inventory, designed as if it were a crate (decision 1). It is a **wrapper**: the
field types are unchanged (decision 3 — `chrome_*_rects` stay `egui::Rect`).
`PerWindowState` holds one instance instead of seven loose fields.

The type carries an explicit publish/read discipline in its doc comment,
consolidating what is today spread across `window.rs:540-580` and six separate
field docs: written during a frame, read from outside one, one frame stale by
construction, and unchanged across an early return.

Deliverable: the module, the migration, and tests pinning the publish discipline
(including that a frame which early-returns leaves the previous frame's values
intact).

Verification: standard suite, plus `--features frame-profiling`.

Prohibitions: do NOT change any field's type. Do NOT include Type A or Type C
fields — Type A already has a reset-on-read contract and restructuring it is out
of scope. Do NOT touch `chrome_damage.rs`. Do NOT change write sites' **ordering**
relative to each other: `chrome_head_rects` is written `Full`-only at
`app_impl.rs:1753`, `chrome_border_rects` inside `central_body` at `2461`, and
`chrome_toast_rects` after `central_body` at `3970` — the one-frame-stale
semantics documented at `window.rs:565-569` depend on exactly this ordering.
Do NOT proceed to 122.5.

Stop: report the type's API, the publish discipline doc, and test results; await
review.

#### 122.5 — Predicates read only from the named type; extract the pane-resolution chain

Scope: `freminal/src/gui/app_impl.rs` only.

What: two changes.

1. `is_chrome_interactive_at` (`751-769`) and `pointer_motion_needs_repaint`
   (`823-1025`) read Type B state **only** through the 122.4 type.
2. Extract the pane-resolution chain (`905-990`) — layout → hit-test → snapshot
   load → signal computation — as a **pure function**, so it is headlessly
   testable. Convert `egui::Pos2` to the neutral `Point` on entry per decision 2.

This is the success criterion from the top of this document. The chain must keep
mirroring `update()`'s zoomed-vs-split layout choice; the existing comment at
`app_impl.rs:908-913` explains why getting that wrong mis-hit-tests silently.

**The return contract, decided here so the implementer does not have to.** The
naive reading — "pure function returning `Option<PointerMotionPaneSignals>`" —
collides with the feature-gated diagnostic, because `pane_diag_terms`
(`app_impl.rs:893-894`, set inside the `.map()` closure at `968-984`) is computed
*within* the chain being extracted. A pure function cannot also write it, and
recomputing it in the caller is exactly the drift the comment at `964-967`
warns against. Therefore:

- The extracted function returns a struct — call it `PaneResolution` — carrying
  **both** `PointerMotionPaneSignals` **and** the four term bools
  (`mouse_tracking_active`, `has_urls`, `scroll_offset_nonzero`,
  `gutter_active`).
- Those four terms are computed **unconditionally**, not under
  `#[cfg(feature = "frame-profiling")]`. They are four bools derived from values
  already in hand; there is no cost worth a cfg for, and computing them always
  is what makes drift structurally impossible rather than impossible-by-comment.
- Only the **recording** stays feature-gated, in the caller: the
  `win.frame_stats.record_pointer_motion_check(...)` call at `1000-1016`.
- Net effect: the extracted function contains **no `#[cfg]` at all**, and
  `pane_hover_region_terms` is no longer a diagnostic-only helper.

This is a strict improvement on the status quo, which keeps the diagnostic
honest by interleaving it and relying on a comment to keep it that way.

Deliverable: the extraction plus unit tests for the chain — zoomed vs split,
pointer outside every pane, pointer in the gutter strip, and the
`PaneError::InvalidState` conservative-`true` path (`app_impl.rs:871-877`).
Also **extend 122.14's benchmark** to cover the newly-callable chain, and record
the number against that baseline; 122.14 could not do this because the chain was
not extractable when it ran.

Verification: standard suite, plus `--features frame-profiling` — the recording
call must still fire with the same values for the same inputs as before, and
`FrameStats::record_pointer_motion_check` still mutates through a `Cell` under
an immutable borrow (`window.rs:868`, fields `701-730`, all feature-gated). Add
a test that the four terms returned by `PaneResolution` match what the
pre-extraction diagnostic would have recorded.

Prohibitions: do NOT change any predicate's return value for any input. Do NOT
put a `#[cfg]` inside the extracted function — that is the thing this contract
exists to remove. Do NOT recompute the diagnostic terms in the caller. Do NOT
remove the `#[allow(clippy::too_many_lines)]` at `app_impl.rs:822` unless the
function genuinely fits without it. Do NOT alter the conservative directions
(unknown window → `true`, `try_borrow` failure → `true`,
`PaneError` → `true`). Do NOT proceed to 122.6.

Stop: report the extracted signature, the benchmark number, and test results;
await review.

#### 122.5a — Give the pointer-motion decision its own module

Scope: new `freminal/src/gui/pointer_motion.rs`, `freminal/src/gui/mod.rs`,
`freminal/src/gui/app_impl.rs`.

**Added 2026-07-30, after 122.5 landed.** 122.5 made the pane-resolution chain
pure and testable but left it in `app_impl.rs`, which **grew** to 5,784 lines
as a result — the extraction added ~450 lines of tests to the god file it was
meant to relieve. Measured after 122.5, one coherent concept accounts for
~840 of those lines:

| Part                                                                | Lines |
| ------------------------------------------------------------------- | ----- |
| `PointerMotionPaneSignals`, `PaneSnapshotInputs`, `PaneResolution`  | ~387  |
| `pane_hover_region_risk`, `pane_hover_region_terms`,                | (in   |
| `pointer_in_gutter_strip`, `animation_in_flight_composed`,          | the   |
| `pointer_motion_needs_repaint_decision`, `resolve_pane_under_pointer` | above) |
| their 35 test functions                                             | ~450  |

That is one concept — **the out-of-frame pointer-motion repaint decision** —
and `freminal-module-cohesion` says it should be a module whose path names it.
This is also the concrete case the `freminal-extend-or-extract` skill (122.0)
was written for: the alternative is leaving it in the file it was already too
big for.

What: move those six functions, the three types, and their tests into
`freminal/src/gui/pointer_motion.rs`. `App::pointer_motion_needs_repaint` and
`is_chrome_interactive_at` stay in `app_impl.rs` — they are trait-impl methods
needing `&self` — and call into the new module.

**Keep the module `pub(super)`, not `pub`.** It is tempting to make it `pub`
so 122.5's skipped benchmark becomes possible, but that repeats the trade the
122.14 amendment rejected: widening visibility to serve a benchmark. Cohesion
is the justification here; benchmarking is not. If a later subtask wants the
benchmark badly enough, it can make that case on its own.

Deliverable: the module, the moved code and tests, and a line-count delta on
`app_impl.rs`.

Verification: standard suite, plus `--features frame-profiling`. This is a
**pure move** — no test assertion may change, and no function body may change
except for import paths and visibility keywords.

Prohibitions: do NOT change any function body's logic. Do NOT widen visibility
beyond `pub(super)`. Do NOT move `pointer_motion_needs_repaint` or
`is_chrome_interactive_at` themselves. Do NOT move `PublishedFrameState` (it
has its own module). Do NOT rename anything.

Stop: report the line-count delta and that no assertion changed; await review.

#### 122.6 — `DummyApp` override so the dispatch path is testable

Scope: `freminal-windowing/src/lib.rs` and `freminal-windowing/src/event_loop.rs`
test modules only.

What: `DummyApp` (`lib.rs:619-648`) does not override
`pointer_motion_needs_repaint` or `is_chrome_interactive_at`, so the only
behaviour ever exercised is the conservative trait defaults (`lib.rs:332, 355`).
Give it configurable overrides and add tests covering the `CursorMoved` dispatch
path at `event_loop.rs:828-831` — including its interaction with
`should_schedule_cursor_moved` (`event_loop.rs:448-454`), which is tested today
only with hand-supplied booleans.

Deliverable: the harness plus tests proving a suppressing app suppresses and a
conservative app does not.

Verification: standard suite. `winit::window::WindowId::dummy()` is already used
for this purpose at `lib.rs:653, 671, 688`.

Prohibitions: do NOT add a freminal dependency to `freminal-windowing`
(decision 2). Do NOT change production code in this crate. Do NOT change the
trait's default bodies. Do NOT proceed to Group B.

Stop: report tests added and results; await review.

---

### Group B — frame-path decomposition

Where the drift actually is. Each subtask extracts one block; 122.9 must land
after 122.4.

#### 122.7 — Extract `App::update`'s two zero-egui blocks

Scope: `freminal/src/gui/app_impl.rs` only.

What: extract two blocks that contain no egui calls at all:

- dead-pane cleanup (`1527-1611`, ~102 lines) — the only egui touch is
  `ctx.send_viewport_cmd(ViewportCommand::Close)` in the close-whole-window arm
  at `1587-1591`, which **is an early return that reinserts `win`** and must
  stay observably identical;
- command-finished drain and notification routing (`1411-1508`, ~98 lines) —
  one `ctx.input(|i| i.focused)` call.

Deliverable: two extracted functions plus tests for whichever parts become pure.

Verification: standard suite, plus `--features frame-profiling`.

Prohibitions: do NOT merge the two blocks. Do NOT change the ordering of the
notification routing relative to the pane loop — the routing sits after the loop
deliberately, to avoid a borrow conflict (`app_impl.rs:1496-1508`). Do NOT
touch `central_body`. Do NOT proceed to 122.8.

Stop: report line-count delta on `App::update` and test results; await review.

#### 122.8 — Extract `central_body`'s window-command drain and OSC routing

Scope: `freminal/src/gui/app_impl.rs` only.

What: extract `1899-2098` (~200 lines): the per-tab, per-pane
`handle_window_manipulation` drain and the OSC 9/777, OSC 52 and OSC 99 routing
plus OSC 99 control answering.

**This is more than "OSC routing".** The comment at `app_impl.rs:1899-1904`
records that the drain covers all tabs and all panes with **different discard
rules for active versus non-active panes** (viewport commands versus reports,
titles and clipboard). That branching is the risk in this subtask; preserve it
exactly.

Deliverable: the extraction plus tests for the active/non-active discard rules.

Verification: standard suite, plus `--features frame-profiling`.

Prohibitions: do NOT change any discard rule. Do NOT fold the four routing
blocks (`1986-2000`, `2002-2014`, `2016-2048`, `2050-2098`) into one — they
route to different destinations and one writes PTY responses via `send_or_log!`.
Do NOT proceed to 122.9.

Stop: report the extraction and test results; await review.

#### 122.9 — Extract `central_body`'s frame-damage aggregation

Scope: `freminal/src/gui/app_impl.rs` only. **Must land after 122.4.**

What: extract `3119-3295` (~177 lines): `shader_recomposites`, `pointer_moving`,
`pointer_over_chrome`, `force_full`, `toast_active`, the per-pane damage scan,
and the `decide_frame_damage` call.

**The landmine.** `pending_frame_damage` is written **twice**: pre-composition
inside `central_body` at `3240`, then overwritten post-composition at
`4041` by `compose_with_chrome_damage`. The comment at `4056-4063` explains the
second write exists because the first is not final. An extraction that publishes
once, at the wrong point, silently reintroduces the bug the second write
prevents.

Also note `app_impl.rs:3155-3158`: `self.is_chrome_interactive_at` is
**unusable** inside `central_body` because `win` was removed from `self.windows`
for the frame, so the block hit-tests the local `win` rects directly via
`point_in_chrome_rects`. Preserve that; do not "simplify" it into a call to the
method.

Deliverable: the extraction plus a test pinning that the pre-composition and
post-composition values remain distinct.

Verification: standard suite, plus `--features frame-profiling` — the duty-cycle
counters at `3246-3295` are feature-gated.

Prohibitions: do NOT collapse the double write. Do NOT replace the direct
`point_in_chrome_rects` call with `is_chrome_interactive_at`. Do NOT change
`decide_frame_damage`'s inputs. Do NOT proceed to 122.10.

Stop: report the extraction, the double-write test, and results; await review.

#### 122.10 — Extract `central_body`'s chrome-signal staging

Scope: `freminal/src/gui/app_impl.rs` only.

What: extract `3338-3410` (~73 lines): the `ChromeTabSnapshot` build, its diff
against `prev_chrome_tab_snapshot`, and the `ChromeSignals` assembly written at
`3374-3390`.

Deliverable: the extraction plus tests for the snapshot diff.

Verification: standard suite, plus `--features frame-profiling` (per-signal
counters are gated).

Prohibitions: do NOT change which signals force `ChromeDamage::Changed` —
`toast_active` and `any_overlay_open` each force it every frame they hold, and
that is what keeps a genuinely changed chrome from being replayed. Do NOT
proceed to 122.11.

Stop: report the extraction and results; await review.

#### 122.11 — Extract `show`'s dirty-tracking decision block

Scope: `freminal/src/gui/terminal/widget.rs` only.

What: extract `2367-2660` (~294 lines): `theme_changed`, `dims_changed`,
`folds_changed`, `content_changed`, the selection-clear rule,
`selection_changed`, `search_changed`, `hover_changed`, `screen_selection`
translation, cursor-trail animation, `image_pixels_changed`, and the final
`cursor_only` boolean.

This is `show`'s largest near-pure block; its only non-freminal touch is locking
`render_state` to check `deco_verts.is_empty()`.

Deliverable: the extraction plus tests. This is also the opportunity to make
`overlay_suppress_input_tests` (`widget.rs:4761-4947`) call real code instead of
re-implementing `show`'s inline booleans — if the extracted surface reaches
them, convert those tests; if not, say so and leave them.

Verification: standard suite, plus `--features frame-profiling`.

Prohibitions: do NOT change the `cursor_only` fast-path decision for any input —
it selects between `build_cursor_verts_only` (`2666-2745`) and the full rebuild
(`2746-3044`) and a wrong answer is a visible rendering bug. Do NOT touch either
vertex-building branch. Do NOT proceed to Group C.

Stop: report the extraction, whether the suppress-input tests were converted,
and results; await review.

---

### Group C — cleanup (static targets, demoted)

#### 122.11a — Consolidate the per-frame pane drains into one module

Scope: new `freminal/src/gui/frame_drain.rs`, `freminal/src/gui/mod.rs`,
`freminal/src/gui/app_impl.rs`.

**Added 2026-07-30, after 122.8.** Group B is shrinking the god *functions* as
intended — `update()`'s inline body and `central_body` are both materially
smaller — but `app_impl.rs` itself has **grown**, because each subtask's doc
comments and tests land in the same file:

| After  | `app_impl.rs` |
| ------ | ------------- |
| 122.5a | 4,878         |
| 122.7  | 5,252         |
| 122.8  | 5,545         |

The plan's success criterion is explicitly not line counts, and the goal
statement is about god *functions*, so this is not a failure. But finishing
Group B with the file larger than it started would be a poor outcome for a task
whose headline is decomposition, and there is now an obvious home.

Two independent sub-agents, asked separately, proposed the same concept:
**per-frame pane-event draining** — everything that walks `win.tabs` once per
frame to drain a channel and stage results for later handling. That is a real
concept, it names itself, and it currently accounts for roughly 700 lines
spread across `app_impl.rs`.

What: move into `frame_drain.rs` the three drains and their supporting types
and tests — `drain_command_finished_events`, `process_dead_panes`,
`DeadPaneOutcome`, `drain_window_manipulation_commands`,
`WindowManipulationEvents`, and `route_window_manipulation_events`. `WindowFocus`
goes too if nothing outside the group uses it by then; check.

Two known wrinkles: `route_window_manipulation_events` takes `&self` on
`FreminalGui`, so it either becomes a free function taking explicit references
(consistent with the other two, and preferred) or stays behind; and
`process_dead_panes` takes `&mut PerWindowState`, which is fine for a sibling
module but confirm no visibility has to widen past `pub(super)`.

**Run this after Group B's extractions are complete**, not between them, so the
move happens once rather than churning the same code repeatedly. It is a pure
move: no logic change, no test-assertion change, identical test count.

Deliverable: the module, the move, and the line-count delta on `app_impl.rs`.

Verification: standard suite plus `--features frame-profiling`, and **both**
clippy invocations.

Prohibitions: do NOT change any logic. Do NOT widen visibility past
`pub(super)`. Do NOT fold the three drains into one. Do NOT move anything that
is not a per-frame drain merely because it is nearby.

Stop: report the delta and that no assertion changed; await review.

#### 122.12 — Name `write_input_to_terminal`'s parameters and result

Scope: `freminal/src/gui/terminal/input.rs` and
`freminal/src/gui/terminal/widget.rs` (call site only).

What: replace the 17 positional parameters with a named params struct and the
7-tuple `WriteInputResult` (`input.rs:1252-1260`) with a named struct. This is
a **mechanical, zero-semantics** change that addresses §8 subtask 1.3's actual
complaint — the orchestration smell — without touching the interleaved body.

**Task 122 does not decompose this function**, and that is a deliberate
reversal of §8 subtask 1.3. Two reasons:

1. Its concerns are **thoroughly interleaved**, not separable by line range.
   There is no contiguous range that is single-concern for more than ~15-20
   lines; most `match` arms mix egui event parsing, VT encoding, `ViewState`
   mutation and a `continue` decision within a handful of lines.
2. The obvious-looking win is a trap. The inline press-path `match`
   (`1620-2016`) appears to duplicate the already-pure
   `egui_key_to_terminal_input` (`989-1055`, called from exactly one site,
   `1513`, on the KKP release path). **They are not equivalent.** See the
   cleanup entry 122.C1 below.

Deliverable: the two structs, the migrated call site, and a note in this
document confirming byte-for-byte identical behaviour.

Verification: standard suite. The 50 existing tests in `input.rs` must pass
unchanged.

Prohibitions: do NOT touch the body's logic. Do NOT attempt the
`control_key` / `egui_key_to_terminal_input` reconciliation — that is 122.C1,
deliberately out of scope. Do NOT change any encoding. Do NOT remove the
`#[allow(clippy::too_many_arguments)]` on `show` — that is a different function.

Stop: report the structs and that no encoding changed; await review.

#### 122.13 — Rename `gui_scroll_offset` / `gui_extra_rows`

Scope: `freminal-terminal-emulator/src/interface.rs` only, plus call sites the
rename touches.

What: the names leak the consumer's identity into a
`freminal-terminal-emulator` type. Nothing about `TerminalEmulator` should
encode that the GUI is the thing asking. Surface:

- fields `gui_scroll_offset` (`176`), `gui_extra_rows` (`186`),
  `previous_scroll_offset` (`190`), `previous_extra_rows` (`194`) — all private
- methods `set_gui_scroll_offset` (`506-516`), `set_gui_scroll_window`
  (`518-529`), `reset_scroll_offset` (`531-538`)
- reads in `handle_incoming_data` (`418`) and `build_snapshot` (`629`, `648`)
- 27 occurrences of `gui_scroll_offset`, 16 of `gui_extra_rows`, **all confined
  to this one file**; tests poke the private fields directly (`1058`, `1062`,
  `1070`, `1072`, `1084`, `1086`, `1097`, `1098`, `1180`, `1212-1215`, `1534`)

Semantically these are the **requested scrollback viewport position** and the
**number of extra rows to flatten above the visible window** (command-block fold
support). The existing doc comments already describe them accurately without the
`gui_` prefix. Opus's chosen names, to be used verbatim:
`requested_scroll_offset`, `extra_flatten_rows`, `previous_requested_scroll_offset`,
`previous_extra_flatten_rows`, and `set_requested_scroll_offset` /
`set_requested_scroll_window` (`reset_scroll_offset` keeps its name).

Deliverable: the rename, existing tests passing.

Verification: standard suite. Confirm no propagation into `freminal-buffer`.

Prohibitions: do NOT change any semantics or any value. Do NOT change method
visibility. Do NOT touch `freminal-buffer`. Do NOT rename `ViewState`'s
`scroll_offset` — different field, different crate.

Stop: report the rename and confirm `freminal-buffer` untouched; await review.

---

### Group D — gates and close-out

#### 122.14 — Benchmark the frame path and the pointer-motion predicate

Scope: `freminal/benches/` (new or extended bench file), `freminal/Cargo.toml`
if a new bench target is needed.

**Runs first.** This is the before-capture for the whole task.

What: **no existing benchmark covers this code.** The suite is
`freminal-buffer/benches/buffer_memory_bench.rs`,
`freminal-buffer/benches/buffer_row_bench.rs`,
`freminal/benches/paste_guard_bench.rs`,
`freminal/benches/render_loop_bench.rs`, and
`freminal-terminal-emulator/benches/buffer_benches.rs` — none of them touches
`App::update`, `central_body`, or the `CursorMoved` predicate path. Per
`performance-benchmarks`, a change to a measured hot path with no benchmark
requires adding one first.

**AMENDED 2026-07-30 (maintainer decision), before any code was written.** The
original text asked for a benchmark of the four pure predicate cores
(`pointer_motion_needs_repaint_decision`, `pane_hover_region_risk`,
`animation_in_flight_composed`, `pointer_in_gutter_strip`) *and* prohibited
changing production code. Recon established those two requirements are mutually
exclusive, and that satisfying the first would produce a benchmark with no
regression-detection power:

- All four are **private** module-level `const fn`s, and
  `freminal/src/gui/mod.rs:39` is `mod app_impl;` — not `pub`. A Criterion bench
  compiles as an **external crate** against the `freminal` lib, so reaching them
  requires making the 5,319-line `app_impl` module `pub`, plus `pub` on five
  functions, plus making `PointerMotionPaneSignals` `pub` with `pub` fields.
  That is a production-code change, and one that widens visibility on the
  binary's largest internal module purely to serve a bench — which
  `freminal-module-cohesion` says to decline.
- All four are **O(1) boolean compositions over already-computed inputs**
  (`animation_in_flight_composed` is `a || b`). Criterion would report
  sub-nanosecond figures dominated by its own harness overhead. There is no
  regression such a benchmark can detect, and they already carry 22 unit tests
  — 9 + 5 + 4 + 4 (`app_impl.rs:4805-5015`; see the corrected count above).
  Benchmarking them would be a gate in name only.

**What is benchmarked instead: the pane-resolution chain's constituents, which
are already `pub` and require no production-code change at all.** The four
predicates are the cheap tail of the `CursorMoved` path. Reading
`app_impl.rs:905-990`, the dominant per-event cost is:

| Step | Call                                          | Work                                    |
| ---- | --------------------------------------------- | --------------------------------------- |
| 1    | `PaneTree::layout` (`panes/mod.rs:1087`)      | recursive tree walk + `Vec` alloc, O(n) |
| 2    | inline rect-containment `find` (`930-933`)    | linear scan; same work as `pane_at_pos` |
| 3    | `PaneTree::find` (`panes/mod.rs:1042`)        | tree search, O(n)                       |
| 4    | `pane.arc_swap.load()` (`942`)                | `ArcSwap` guard acquire                 |
| 5    | the four predicates                           | O(1) boolean composition                |

Steps 1-4 are all `pub`, and `PaneTree` is constructible headlessly through the
public `Pane::from_channels` (`panes/mod.rs:301`) plus a hand-assembled
`pty::TabChannels` (`pty.rs:248`, all fields `pub`) plus
`WindowPostRenderer::new()` (`renderer/gpu.rs:1866`, a `pub const fn`
documented as creating GPU resources lazily on first `init`). No window, no GL
context, no PTY process.

Benchmark, parameterised over 1/2/4/8/16 panes where pane count is an input:

- `PaneTree::layout`
- `PaneTree::split_borders`
- `PaneTree::find`
- `panes::pane_at_pos` (over a synthetic `Vec<(PaneId, Rect)>` — no `PaneTree`
  needed)
- `panes::active_highlight_segment` (`panes/mod.rs:507`)

This framing also **gives 122.3 a performance gate it otherwise lacks**:
`PaneTree::layout`, `split_borders`, `pane_at_pos` and
`active_highlight_segment` are precisely the functions 122.3 re-types onto the
neutral geometry, and 122.3's verification was otherwise only "no test
tolerance changed".

**122.14 still does not benchmark the extracted pane-resolution chain itself**,
because 122.5 is what makes that chain callable as a unit. Extending the
benchmark to cover it remains a requirement *of 122.5* — but 122.5 now extends
a benchmark whose constituent parts are already measured, rather than starting
from nothing.

Record the baseline in this document.

Deliverable: the benchmark plus a recorded baseline.

Verification: `cargo bench --no-run --all` compiles; standard suite unaffected.

Prohibitions: do NOT attempt to benchmark `App::update` end to end — it needs a
live window and that harness does not exist (see 121.28 / issue #440). Do NOT
change production code — in particular do NOT widen the visibility of
`mod app_impl`, `mod chrome_damage` or `mod frame_damage`, and do NOT add `pub`
to any function or field to make it benchmarkable. If a candidate is not
reachable today, it is out of scope. Do NOT benchmark the four O(1) predicate
cores. Do NOT benchmark the extracted pane-resolution chain — it is not
extractable yet; that belongs to 122.5. Do NOT proceed to any other subtask.

Stop: report the benchmark IDs and baseline numbers; await review.

##### 122.14 recorded baseline

Bench file: `freminal/benches/pane_resolution_bench.rs`. Captured 2026-07-30 on
`task-122/orchestration-extraction`, Criterion `sample_size(50)`,
`measurement_time(2s)`. Figures are the **median** of Criterion's
`[low median high]` triple. 34 benchmark IDs.

`PaneTree::layout` — the pointer-motion chain's step 1, and the frame path's
layout call. `chain/16` is the degenerate right-leaning shape.

| Bench ID            | Median    |
| ------------------- | --------- |
| `layout/balanced/1`  | 32.816 ns |
| `layout/balanced/2`  | 45.409 ns |
| `layout/balanced/4`  | 80.883 ns |
| `layout/balanced/8`  | 167.15 ns |
| `layout/balanced/16` | 348.70 ns |
| `layout/chain/16`    | 412.52 ns |

`PaneTree::split_borders` — frame-path divider geometry. `active_first` is the
cheapest possible input (pane 0 sits on the all-`first` spine, so every
ancestor's `first.contains` hits immediately); `active_last` is the
representative case (a `second` child, so at least one ancestor pays a failed
exhaustive subtree scan). At 16 panes the difference is ~43%, which is why both
are recorded — measuring only `active_first` would have understated the
baseline.

| Bench ID                       | Median    |
| ------------------------------ | --------- |
| `split_borders/active_first/1`  | 15.393 ns |
| `split_borders/active_last/1`   | 15.559 ns |
| `split_borders/active_first/2`  | 67.040 ns |
| `split_borders/active_last/2`   | 51.677 ns |
| `split_borders/active_first/4`  | 116.45 ns |
| `split_borders/active_last/4`   | 101.00 ns |
| `split_borders/active_first/8`  | 392.74 ns |
| `split_borders/active_last/8`   | 352.62 ns |
| `split_borders/active_first/16` | 718.11 ns |
| `split_borders/active_last/16`  | 1.0302 µs |

At 2, 4 and 8 panes `active_last` measured marginally *faster* than
`active_first`; the run-to-run spread at those sizes (e.g. `active_first/8`
spans 362-425 ns) exceeds the gap, so those pairs are within noise. Only the
16-pane pair separates cleanly.

`PaneTree::find` — the pointer-motion chain's step 3, worst case (last-inserted
id). Balanced and chain land within noise of each other, as predicted: a full
depth-first search visits the same node count under either shape.

| Bench ID          | Median    |
| ----------------- | --------- |
| `find/balanced/1`  | 4.4778 ns |
| `find/balanced/2`  | 6.5536 ns |
| `find/balanced/4`  | 14.703 ns |
| `find/balanced/8`  | 26.036 ns |
| `find/balanced/16` | 58.346 ns |
| `find/chain/16`    | 57.853 ns |

`pane_at_pos` — the same linear rect-containment scan the chain inlines at
`app_impl.rs:930-933`. `first_hit` is flat in pane count (matches immediately);
`last_hit` and `miss` scale, as a linear scan must.

| Bench ID                    | Median    |
| --------------------------- | --------- |
| `pane_at_pos/first_hit/1`    | 4.9338 ns |
| `pane_at_pos/last_hit/1`     | 4.5014 ns |
| `pane_at_pos/miss/1`         | 4.1744 ns |
| `pane_at_pos/first_hit/2`    | 6.1305 ns |
| `pane_at_pos/last_hit/2`     | 7.6166 ns |
| `pane_at_pos/miss/2`         | 5.8324 ns |
| `pane_at_pos/first_hit/4`    | 6.2849 ns |
| `pane_at_pos/last_hit/4`     | 6.1088 ns |
| `pane_at_pos/miss/4`         | 6.3946 ns |
| `pane_at_pos/first_hit/8`    | 5.2452 ns |
| `pane_at_pos/last_hit/8`     | 12.538 ns |
| `pane_at_pos/miss/8`         | 7.5946 ns |
| `pane_at_pos/first_hit/16`   | 5.5787 ns |
| `pane_at_pos/last_hit/16`    | 24.230 ns |
| `pane_at_pos/miss/16`        | 11.306 ns |

`active_highlight_segment` — frame-path divider highlight, not pane-count
parameterised.

| Bench ID                              | Median    |
| ------------------------------------- | --------- |
| `active_highlight_segment/bordering`     | 6.8619 ns |
| `active_highlight_segment/non_bordering` | 5.4369 ns |

#### Measured noise floor — this is NOT a 15% wall-clock gate

**Revised 2026-07-30 during 122.3, with measurement.** The baseline above was
recorded expecting the standard 15% threshold from `performance-benchmarks` to
apply per ID. **It cannot, on this hardware.** The development machine is a
laptop; `opencode` alone holds a core at ~90% and freminal itself takes ~12%,
so a quiet machine is not achievable and should not be waited for.

Two full runs of the **identical binary**, pinned (`taskset -c 12,13,14,15
nice -n -5`), give this run-to-run spread:

| Spread on identical code | IDs | Examples                                        |
| ------------------------ | --- | ----------------------------------------------- |
| > 15%                    | 12  | `split_borders/active_first/4` **54%**, `/16` 22% |
| 5-15%                    | 12  | `layout/balanced/16` 5.5%, `find/balanced/8` 7.8% |
| < 5%                     | 15  | `pane_at_pos/miss/16` 0.1%, `layout/balanced/4` 1.1% |

Unpinned it is far worse — up to 71% on a single ID. Pinning is worth doing and
roughly halves the spread, but **12 of 39 IDs still exceed 15% with no code
change at all**, and the worst offender is not one of the sub-10 ns ones. A
per-ID 15% gate would therefore fire constantly on noise and prove nothing.

**What this benchmark is actually good for, and how to use it:**

1. **Algorithmic shape, compared *within* a single run.** This is reliable and
   is the real value. In every run, `layout` roughly doubles per pane-count
   doubling; `pane_at_pos/first_hit` stays flat (~3.5 ns) at every pane count
   while `last_hit` and `miss` grow with it; `find` grows with depth. If a
   change makes `layout` superlinear, or makes `first_hit` start scaling, that
   shows up unmistakably and is exactly the class of regression worth catching.
2. **Order-of-magnitude changes**, which survive the noise trivially.
3. It is **not** a detector of a 15% wall-clock change, and no subtask should
   claim it verified one.

When re-running, pin the process, and report the *shape* across pane counts
plus any order-of-magnitude move — not per-ID percentage deltas, which are
noise at this resolution. Capture two runs of the unchanged code first if a
per-ID number is ever genuinely needed, so the claim is made against that day's
measured noise floor rather than against this table.

#### 122.15 — Publish per-pane terminal-rect origin (unblocks 121.17)

Scope: `freminal/src/gui/terminal/widget.rs`, `freminal/src/gui/app_impl.rs`,
and the 122.4 module.

**Last implementation subtask**, by maintainer decision.

What: subtask 121.17 needs the per-pane terminal-rect origin and logical cell
size readable from outside a frame. The activation pass found this blocker is
**narrower than 121.17's own text assumes**:

- **Cell size is already reachable out-of-frame.**
  `FreminalTerminalWidget::cell_size()` is a `pub const fn`
  (`widget.rs:1706-1712`) backed by `FontManager`, not by frame state, and
  `font_manager.pixels_per_point()` is already used outside a frame at
  `widget.rs:3763`. Logical cell size is one division away.
- **The terminal-rect origin is the actual gap.** `terminal_rect`
  (`widget.rs:1943-1946`) is derived from `pane_rect`
  (`widget.rs:1932` = `ui.available_rect_before_wrap()`) plus the gutter inset,
  and is **persisted nowhere** — not in `PaneRenderCache`, not in `RenderState`,
  not in `ViewState`.

So this subtask publishes one value through the 122.4 type: the per-pane
terminal-rect origin. It does **not** implement 121.17.

Deliverable: the published value plus a test proving it matches what `show`
computed for the same frame.

Verification: standard suite, plus `--features frame-profiling`.

Prohibitions: do NOT implement 121.17's cell-granular suppression — this
subtask only builds the seam. Do NOT add a loose field to `PerWindowState`;
it goes through the 122.4 type. Do NOT change any suppression behaviour.

Stop: report the published value and its test; await review. Note in the report
that 121.17 is now unblocked.

#### 122.16 — Reconcile the documents

Scope: `Documents/DECOUPLING_FRAMEWORK.md`, `Documents/PLAN_VERSION_120.md`,
`Documents/MASTER_PLAN.md`, `Documents/PLAN_121_PERF_REMEDIATION.md`.

The agent-skill half of this subtask **moved to 122.0** and runs first.

What: two things.

1. Edit `DECOUPLING_FRAMEWORK.md` §8 Phase 1 itself to mark it superseded by
   this document, and correct its stale line counts and its "16 parameters"
   claim (it is 17). The activation commit updated `PLAN_VERSION_120.md` and
   `MASTER_PLAN.md` but deliberately left §8's own text untouched.
2. Advance Task 122's status per `freminal-plan-status-lifecycle` (two tables
   must agree: the Task Summary Status column and the Completion Tracking
   dates), and mark 121.15 / 121.17 as unblocked in
   `PLAN_121_PERF_REMEDIATION.md`.

Deliverable: the document edits.

Verification: markdown lint via pre-commit; `cargo test --all` unaffected.

Prohibitions: do NOT claim Task 122 retired any `EGUI_UPGRADE_ASSUMPTIONS.md`
assumption — it retires none. Do NOT add Phases 2-5 to `MASTER_PLAN.md`; they
remain unsequenced by maintainer instruction. Do NOT re-sequence other versions.

Stop: report the edits; await review.

---

## Cleanup entries surfaced during activation

Per `freminal-orchestrator-protocol`, bugs found outside a subtask's scope
become numbered entries here rather than TODO comments or informal
known-issues sections.

### 122.C1 — `control_key` and `egui_key_to_terminal_input` diverge

Surface point: activation pass, 2026-07-30, while scoping 122.12.

The two key-encoding functions in `input.rs` look like duplicates and are not:

- `control_key` (`930-978`) maps A-Z to `Ctrl(byte)` unconditionally (the caller
  has already established Ctrl), **plus a full C0 punctuation and digit table**:
  `OpenBracket`, `CloseBracket`, `Backslash`, `Space` → `Ctrl(b' ')`,
  `Minus | Slash | Num7` → `0x1F`, `Num2` → `0x00`, `Num3` → `0x1B`,
  `Num4` → `0x1C`, `Num5` → `0x1D`, `Num6` → `0x1E`, `Num8` → `0x7F`.
- `egui_key_to_terminal_input` (`989-1055`) branches A-Z on ctrl/command/shift,
  maps `Key::Space` to `Ascii(b' ')` — **the opposite of `control_key`** — and
  has no C0 table at all.

The press-path `match` dispatches to `control_key` for the wildcard Ctrl+letter
arm (`1994-2006`); `egui_key_to_terminal_input` is called from exactly one site
(`1513`), the KKP flag-2 release path.

Impact: none known — each is correct for its own call site. But any future
"de-duplicate these" change is a semantic reconciliation requiring a test that
pins both call sites' current output first.

Scope of fix: `input.rs`. Approach: characterisation tests over both functions
across the full `egui::Key` range before touching either.

Scheduling: **not** part of Task 122. Deliberately excluded from 122.12.

### 122.C2 — `assert_eq!` production panic path in `control_key`

Surface point: activation pass, 2026-07-30, while reading `control_key`.

`input.rs:933` is `assert_eq!(name.len(), 1);` in production code, inside the
A-Z branch of `control_key`. `agents.md` forbids panics in production; the
`unwrap_used` / `expect_used` lints do not catch `assert_eq!`, so this slipped
through.

Impact: latent. It holds for every `egui::Key` in `A..=Z` today, so it is not
reachable — but it is a panic in a hot input path guarded by an assumption about
a third-party enum's `name()`.

Scope of fix: `input.rs:930-978`. Approach: return `None` rather than assert.

Scheduling: independent; may be done any time. Not part of Task 122.

---

## Verification

Standard for every subtask, per `agents.md`:

1. `cargo test --all`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo machete`
4. `cargo test --all --features frame-profiling` — **task-specific addition**,
   because 28 feature-gated sites in `window.rs` and 26 in `app_impl.rs` are
   interleaved through the restructured code and step 1 never compiles them
5. `cargo xtask check-windows` before the PR
   (`freminal-windows-crosscheck`)

Plus the 122.14 baseline for any subtask touching the frame path, per
`performance-benchmarks` and `freminal-bench-table`.

---

## References

- `Documents/DECOUPLING_FRAMEWORK.md` — §8 Phase 1 is this task's predecessor
  plan content, superseded by this document; §2A is the Phase 0 measurement
  record; §10 lists the invariants a refactor may not break.
- `Documents/PLAN_121_PERF_REMEDIATION.md` — Task 121; 121.17 is the one subtask
  blocked on this task, 121.15 is subsumed by 121.17.
- `Documents/PLAN_VERSION_120.md` — v0.12.0 summary.
- `Documents/EGUI_UPGRADE_ASSUMPTIONS.md` — assumptions A1-A13. Task 122 retires
  none of them.
- `Documents/DESIGN_DECISIONS.md` — the `freminal-windowing` crate charter,
  which decision 2 rests on.
- `Documents/PROFILING.md` — profiling methodology (121.23).
- Issue #440 — the missing pixel / headless-GL harness, which is why 122.14
  cannot benchmark `App::update` end to end.
- Issue #459 — the profiling findings behind Task 121.
- PR #467 — Group B fixes; the case study for why the seam needs a name.
