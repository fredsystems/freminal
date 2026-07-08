# PLAN_VERSION_111.md — v0.11.1 "Correctness Fixes"

## Goal

A focused patch release closing three correctness bugs surfaced by a
senior-engineer investigation during the v0.11.0 → v0.11.1 window. None add
features; each fixes observably-wrong behaviour and ships with regression tests.

- **Task 115 — DECSCNM per-pane, per-cell reverse video.** DECSCNM (`CSI ?5h`)
  currently repaints the whole window's egui chrome white based on the _active_
  pane, instead of inverting _that pane's_ terminal cells. Fix: apply DECSCNM as
  a per-pane, per-cell foreground/background swap at render time, composing with
  per-cell SGR-7 via XOR; remove the window-global chrome coupling.
- **Task 116 — Text selection release/stuck bug.** A click-drag selection
  sometimes clears on mouse-release and the selection state machine gets stuck
  until a copy keystroke resets it. Three distinct defects: a release-time
  coordinate recompute racing snapshot swaps, a same-frame `content_changed`
  auto-clear, and `is_selecting` orphaning under input suppression / scrollbar
  drag / pane-boundary drags.
- **Task 117 — DECDWL/DECDHL/DECSLRM buffer-model completeness.** Rendering is
  correct; two buffer-model gaps remain: the auto-wrap column is not halved on
  double-width/height rows, and explicit scroll (SU/SD) plus margin-triggered
  IND/RI auto-scroll are not confined to DECSLRM left/right margins the way
  IL/DL already are.

Depends on v0.11.0 (merged). This version is **decomposed** (per the
`freminal-version-activation` skill): it is the active version and the subtasks
below were confirmed against the current code seams during a **2026-07-08
activation recon** (post PR #382 merge, workspace at `0.11.0`). Re-confirm the
seams if execution is deferred — the codebase may move.

---

## Task Summary

| #   | Feature                                         | Scope        | Status  | Depends On |
| --- | ----------------------------------------------- | ------------ | ------- | ---------- |
| 115 | DECSCNM per-pane per-cell reverse video         | Medium       | Planned | Task 58    |
| 116 | Text selection release/stuck fix                | Medium       | Planned | None       |
| 117 | DECDWL/DECDHL/DECSLRM buffer-model completeness | Small-Medium | Planned | None       |

---

## Execution model (READ THIS FIRST)

The three tasks are **logically independent** — Task 115 is GUI-renderer, Task
116 is GUI-input state, Task 117 is the buffer crate — and touch disjoint files.
**They must NOT be executed by concurrent cargo-running sub-agents on the same
checkout.** All agents share one `target/` directory and one workspace; a
transient compilation error introduced by one agent's in-progress edit causes
another agent's `cargo test` / `cargo clippy` to fail spuriously, and the agents
thrash trying to "fix" damage that isn't theirs.

Permitted execution strategies:

1. **Serial (default).** One task branch at a time, fully verified and merged
   (or at least green and committed) before the next begins. Recommended order:
   **117 → 116 → 115** (lowest-risk/self-contained first; renderer change with a
   design decision last).
2. **Parallel via isolated worktrees.** If parallelism is desired, each task
   runs in its **own `git worktree` with its own `target/`** so no two agents
   share a build directory. Only then may the three run concurrently.

Within a task, subtasks are strictly sequential (each leaves `cargo test --all`
green before the next starts), per `agents.md`'s multi-step protocol.

Each subtask carries the five-part contract (scope / what / deliverable /
verification / prohibitions / stop) from the `freminal-orchestrator-protocol`
skill. Verification for every implementation subtask is:

```text
cargo test --all
cargo clippy --all-targets --all-features -- -D warnings
```

plus any subtask-specific test named below.

---

## Task 115 — DECSCNM per-pane, per-cell reverse video

### 115 — Problem

`snap.is_normal_display` is consumed in exactly one place today:
`chrome_style::build_visuals` (`freminal/src/gui/chrome_style.rs:204-240`), whose
`else` arm (lines 230-239) forces `window_fill`/`panel_fill` to solid white when
reverse-video is on. It is driven per-frame from a **single** snapshot via
`ctx.global_style_mut` in `app_impl.rs:936-963` (the style-cache key at
`window.rs:50-60` carries one `bool`). Because the window's `CentralPanel` hosts
the entire split-pane tree, one pane's DECSCNM repaints every pane and the
chrome. The actual cell renderer (`vertex.rs`) never reads `is_normal_display` at
all, so DECSCNM does not even invert cell content.

`rendering.rs::apply_chrome_visuals` (line 69) hardcodes `normal_display = true`
and is only the window-creation / theme-change baseline — the live per-frame path
is `app_impl.rs`, not `rendering.rs`.

### 115 — Correct behaviour

DECSCNM is a per-terminal (per-pane) mode. It swaps the resolved foreground and
background of **every cell in that pane's grid**, at render time, composing with
per-cell SGR-7 reverse video by **XOR** (a cell that is both SGR-7 and DECSCNM
renders un-swapped — the DEC/xterm-correct semantic, confirmed with the
maintainer). Chrome (menu bar, tab bar, inter-pane panel background) must NOT
change based on any pane's terminal mode.

### 115 — Durable design decisions

- **XOR compose.** `effective_reverse = cell_sgr7_reverse XOR pane_decscnm`. The
  existing per-cell swap lives in `StateColors::color()` / `background_color()`
  (`freminal-common/src/buffer_states/cursor.rs:120-135`), keyed on
  `StateColors.reverse_video: ReverseVideo`. DECSCNM is a screen-wide flag XORed
  against that per-cell flag at the point colours are resolved in the vertex
  builders.
- **Scope the flag to the pane's own snapshot.** `snap.is_normal_display` is
  already reachable at the per-pane vertex-builder call sites
  (`widget.rs:2337-2387`, `snap` in scope). Thread it into the builder option
  structs; do not reintroduce any window-global state.
- **Chrome is decoupled entirely.** `build_visuals` loses its `normal_display`
  parameter; no chrome fill depends on DECSCNM any more.

### 115 — Subtasks

#### 115.1 — Decouple DECSCNM from egui chrome

Scope: `freminal/src/gui/chrome_style.rs`, `freminal/src/gui/app_impl.rs`,
`freminal/src/gui/window.rs`, `freminal/src/gui/rendering.rs`.

What: Remove the `normal_display: bool` parameter from `build_visuals`
(chrome_style.rs:204-209) and delete the white-fill `else` arm (230-239) so the
palette-derived fills always apply. Update both call sites: `app_impl.rs:949-962`
(stop reading `snap.is_normal_display`; drop it from the `build_visuals` call and
from the `style_cache` key tuple) and `rendering.rs:63-74` (drop the hardcoded
`true` argument). Update the `style_cache` tuple type in `window.rs:55-60` to
drop the leading `bool`, and its doc comment at line 50. Remove the now-stale
"keeps visuals in sync with the active pane's display mode" comments in
`rendering.rs:29-32,61-62`. Delete the chrome tests
`reverse_video_forces_white_window_fill` (chrome_style.rs:510) and
`reverse_video_panel_fill_alpha_reflects_opacity` (chrome_style.rs:653); keep and
adjust `normal_display_window_fill_is_opaque_and_palette_derived` (line 524) so
it no longer passes the removed parameter.

Deliverable: chrome no longer reacts to DECSCNM; `is_normal_display` is no longer
read anywhere in the GUI chrome path. Existing non-reverse-video chrome tests
still pass.

Prohibitions: do NOT yet touch the cell renderer; do NOT delete the
`is_normal_display` field from `TerminalSnapshot` (115.2 consumes it); do NOT
proceed to 115.2.

Stop: report files changed + verification results; await review.

#### 115.2 — Apply per-cell DECSCNM XOR swap in the vertex builders

Scope: `freminal/src/gui/renderer/vertex.rs`, `freminal/src/gui/terminal/widget.rs`.

What: Add a `reverse_screen: bool` field to `BackgroundFrame`
(vertex.rs:225-252) and `FgRenderOptions` (vertex.rs:104-117). Thread
`snap.is_normal_display` (inverted to `reverse_screen = !snap.is_normal_display`)
into both from the call sites in `widget.rs:2337-2387`. In `build_background_instances`
(bg colour at vertex.rs:307; decoration/underline-fallback colour paths at
vertex.rs:352,357,384) and `build_foreground_instances` (fg colour at
vertex.rs:626), compute the effective reverse state by XORing `reverse_screen`
against the per-run SGR-7 state before resolving the colour. Prefer a small
helper on the resolved colours rather than mutating `StateColors`: when
`reverse_screen` is set, swap the fg/bg that `run.colors.color()` /
`run.colors.background_color()` would return (equivalently, call the _opposite_
accessor). Ensure underline colour (vertex.rs:352) follows the same effective
state so underlines remain visible against the swapped background.

Deliverable: DECSCNM inverts only the emitting pane's cells; SGR-7 cells inside a
DECSCNM screen render un-swapped (XOR); sibling panes and chrome are unaffected.

Prohibitions: do NOT change `StateColors` public API in `freminal-common` unless
strictly necessary (prefer the render-side swap); do NOT reintroduce any
window-global flag; do NOT proceed to 115.3.

Stop: report files changed + verification results; await review.

#### 115.3 — Tests

Scope: `freminal/src/gui/renderer/vertex.rs` (test module).

What: Add unit tests asserting: (a) with `reverse_screen = true` and a cell whose
SGR-7 is Off, the emitted background instance uses the cell's foreground colour
and vice-versa; (b) with `reverse_screen = true` and SGR-7 On, colours render
un-swapped (XOR); (c) `reverse_screen = false` reproduces today's output exactly.
Reuse the existing vertex-builder test harness in the file.

Deliverable: green tests covering normal / DECSCNM / SGR-7×DECSCNM compose.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT proceed to 115.4.

Stop: report files changed + verification results; await review.

#### 115.4 — Escape-sequence dual-doc update

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`.

What: Per the `freminal-escape-sequence-docs` skill, upgrade DECSCNM from 🚧 to
✅ in COVERAGE (note: per-pane per-cell fg/bg swap, XOR with SGR-7) and remove the
"DECSCNM cell-level fg/bg swap (panel-fill swap exists)" renderer-gap line from
GAPS. Update both "Last updated" headers.

Deliverable: docs match the implementation; markdownlint clean.

Prohibitions: do NOT change source code; do NOT proceed to another task.

Stop: report files changed + markdownlint result; await review.

---

## Task 116 — Text selection release/stuck fix

### 116 — Problem

Three independent defects (all confirmed at current file:line):

1. **Release-time coordinate race (primary).** The release branch
   (`input.rs:2328-2368`) recomputes the end coordinate via `release_end_col`
   (input.rs:1246-1280) → `screen_row_to_buffer_row` (input.rs:82-107) →
   `visible_window_start` (coords.rs:25-44), which is
   `total_rows - term_height - scroll_offset`. New PTY output between the last
   `PointerMoved` and the release changes `total_rows`, so the release-computed
   buffer row can collide with the anchor, triggering the
   `anchor == end_coord → clear()` path (input.rs:2365-2367) and wiping a real
   selection.
2. **Same-frame auto-clear.** `widget.rs:1924` clears when
   `snap.content_changed && !snap.scroll_changed && !is_selecting`, and runs
   _after_ input processing in the frame where the release flipped
   `is_selecting = false`. A release coinciding with new output wipes the
   just-committed selection.
3. **Orphaning.** `widget.rs:1647-1656` forces `is_selecting = false` (without
   clearing `anchor`/`end`) whenever `suppress_input` / a menu / search / command
   history / `scrollbar_dragging` is active. It leaves the state machine stuck;
   the next primary press sees a stale `has_selection()` and takes the
   "clear existing selection, don't start a new drag" branch (input.rs:2267-2271),
   so it _feels_ stuck until the copy handler's unconditional `clear()`
   (widget.rs:1715) resets everything. A split-pane drag that crosses into
   another pane's rect is the same failure: the origin pane never receives the
   mouse-up.

### 116 — Durable design decisions

- **Finalize from tracked state, not a re-derivation.** Mouse-up must commit the
  selection from the `selection.end` already maintained by `PointerMoved`, so
  press / drag / release share one coordinate space and are immune to
  intervening snapshot `total_rows` shifts. The "click without drag collapses to
  no-selection" behaviour must be preserved, but decided from the tracked end,
  not a freshly recomputed release coordinate.
- **The auto-clear must not fire on a just-committed frame.** Introduce an
  explicit "selection committed this frame" signal distinct from `is_selecting`.
- **Interrupted drags finalize deterministically.** When `is_selecting` is
  force-cleared by suppression / scrollbar / a pane-boundary exit, the selection
  must be finalized (or cleared) through the _same_ path a normal release uses —
  never left with a stranded `anchor`/`end`.

### 116 — Subtasks

#### 116.1 — Finalize selection from tracked end (fix defect 1)

Scope: `freminal/src/gui/terminal/input.rs`.

What: In the primary-release branch (input.rs:2328-2368), stop recomputing the
end coordinate via `release_end_col`/`screen_row_to_buffer_row`. Use the
`selection.end` already set by the last `PointerMoved` (input.rs:2116-2162).
Preserve the click-count snapping (word/line) that `release_end_col` performed by
applying it to the tracked end rather than to a re-derived coordinate. Keep the
"anchor == end ⇒ collapse to no selection" behaviour, evaluated against the
tracked end. If `release_end_col` is unused afterward, remove it.

Deliverable: dragging a selection while output streams no longer clears it on
release.

Prohibitions: do NOT touch `widget.rs` in this subtask; do NOT proceed to 116.2.

Stop: report files changed + verification results; await review.

#### 116.2 — Guard the content-changed auto-clear (fix defect 2)

Scope: `freminal/src/gui/terminal/widget.rs`, `freminal/src/gui/view_state.rs`.

What: Add a per-frame "selection just committed" flag (e.g. a `bool` on the
selection-processing path or a field on `ViewState`/the render cache) set when a
release finalizes a non-empty selection this frame. Gate the auto-clear at
widget.rs:1924 so it cannot fire on that frame. Clear the flag at frame end.

Deliverable: a release coinciding with new PTY output keeps the selection.

Prohibitions: do NOT weaken the existing `snap.content_changed`
(vs `Arc::ptr_eq`) distinction documented at widget.rs:1903-1923; do NOT proceed
to 116.3.

Stop: report files changed + verification results; await review.

#### 116.3 — Finalize interrupted drags deterministically (fix defect 3)

Scope: `freminal/src/gui/terminal/widget.rs`, `freminal/src/gui/terminal/input.rs`.

What: Where `is_selecting` is force-cleared (widget.rs:1647-1656), route through
the same finalize/collapse logic a normal release uses instead of only setting
`is_selecting = false` — so `anchor`/`end` never strand. For the split-pane
boundary case, ensure a drag whose release lands outside the origin pane's rect
still finalizes the origin pane's selection (e.g. finalize on `is_selecting`
while the button is released regardless of position, or clamp the end to the
pane rect). Confirm the next primary press then starts a fresh drag rather than
hitting the clear-only branch (input.rs:2267-2271).

Deliverable: opening a menu / dragging the scrollbar / crossing a pane boundary
mid-drag leaves a well-defined selection state; no "stuck" state requiring copy
to reset.

Prohibitions: do NOT change copy behaviour (widget.rs:1708-1716); do NOT proceed
to 116.4.

Stop: report files changed + verification results; await review.

#### 116.4 — Regression tests

Scope: `freminal/src/gui/terminal/input.rs` and/or
`freminal/src/gui/view_state.rs` (test modules).

What: Deterministic tests (no `Instant`/real timing) that: (a) simulate
press→drag→release across two `TerminalSnapshot`s with different `total_rows`
and assert the selection is not spuriously cleared; (b) toggle
`content_changed`/`scroll_changed` on the release frame and assert the selection
survives; (c) force the suppress-input/scrollbar path mid-drag and assert
`anchor`/`end`/`is_selecting` end in a consistent state and the next press starts
a new drag. Build synthetic snapshots and events with the existing test helpers.

Deliverable: green regression tests for all three defects.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT proceed to another task.

Stop: report files changed + verification results; await review.

> No escape-sequence doc impact for Task 116 (GUI-only).

---

## Task 117 — DECDWL/DECDHL/DECSLRM buffer-model completeness

### 117 — Problem

Rendering is correct and the docs are accurate. Two buffer-model gaps remain:

1. **Half-width wrap not applied.** `insert_text` (`buffer/mod.rs:329+`) computes
   `(wrap_col, wrap_start_col)` at lines 341-345 from DECLRMM margins or full
   width — it already respects DECSLRM but never consults the row's `LineWidth`
   (row.rs:38-59, documented "rendering-only"). On a `DoubleWidth` /
   `DoubleHeightTop` / `DoubleHeightBottom` row a real VT100 wraps at half the
   column count.
2. **DECSLRM scroll confinement missing for SU/SD and IND/RI.** IL/DL branch on
   `declrmm_enabled` and use the column-confined `scroll_slice_{up,down}_columns`
   (lines.rs:371,396,430,455). But `scroll_region_up_n`/`scroll_region_down_n`
   (SU/SD, scroll.rs:318-345) and the IND/RI path — `handle_ind`
   (lines.rs:289-295) / `handle_ri` (lines.rs:307-350), both inheriting the
   unconditional full-row `scroll_slice_{up,down}` via `handle_lf` and
   `scroll_region_{up,down}_primary` (scroll.rs:271-284) — have **no**
   `declrmm_enabled` branch, so content outside the left/right margins is shifted
   along with content inside them.

### 117 — Durable design decisions

- **Half-width wrap.** On a row whose `LineWidth::is_double_width()` is true
  (row.rs:56-59), the effective usable column count is halved. Apply this to
  `wrap_col`/`wrap_start_col` in `insert_text`. `LineWidth` stays a per-row render
  attribute in every other respect (the column _count_ stored is unchanged); only
  the wrap boundary computation consults it. Cursor-column addressing semantics on
  double-width rows are decided in 117.1 (default: leave stored column count
  unchanged; only the wrap boundary halves — matching the existing
  "rendering-only" invariant while fixing observable wrap position).
- **Confinement mirrors IL/DL.** SU/SD and the IND/RI-triggered scroll must, when
  `declrmm_enabled == Enabled`, use the column-confined
  `scroll_slice_{up,down}_columns` with `[scroll_region_left, scroll_region_right]`,
  exactly as IL/DL do. The cleanest fix point for IND/RI is the shared
  `scroll_region_{up,down}_primary` helpers (scroll.rs:271-284), since fixing
  there covers LF/IND/RI/NEL at once — confirm no unintended callers during 117.2.

### 117 — Subtasks

#### 117.1 — Halve the auto-wrap column on double-width/height rows

Scope: `freminal-buffer/src/buffer/mod.rs`.

What: In `insert_text` (wrap computation at lines 341-345), after computing
`(wrap_col, wrap_start_col)` from DECLRMM/full width, consult the current row's
`LineWidth`; when `is_double_width()` is true, halve the effective usable width
(clamp so `wrap_col > wrap_start_col`). Preserve DECSLRM interaction (halving
applies to the margin-derived width too). Do not change the stored column count.

Deliverable: text on a DECDWL/DECDHL row wraps at half width.

Prohibitions: do NOT touch scroll.rs/lines.rs in this subtask; do NOT change
`LineWidth`'s render-side usage; do NOT proceed to 117.2.

Stop: report files changed + verification results; await review.

#### 117.2 — Confine SU/SD and IND/RI scroll to DECSLRM margins

Scope: `freminal-buffer/src/buffer/scroll.rs`, `freminal-buffer/src/buffer/lines.rs`.

What: In `scroll_region_up_n`/`scroll_region_down_n` (scroll.rs:318-345) and in
the shared `scroll_region_up_primary`/`scroll_region_down_primary`
(scroll.rs:271-284) used by LF/IND/RI/NEL, branch on `declrmm_enabled == Enabled`
and call `scroll_slice_{up,down}_columns` with
`[scroll_region_left, scroll_region_right]` when active, mirroring IL/DL
(lines.rs:371,396,430,455). Verify all callers of the modified helpers to ensure
the confinement is correct for every path (LF, IND, RI, NEL, SU, SD) and does not
regress the non-DECLRMM (full-row) behaviour.

Deliverable: with DECLRMM active, SU/SD and IND/RI scroll only within the
left/right margins.

Prohibitions: do NOT alter DECSTBM top/bottom region logic; do NOT proceed to
117.3.

Stop: report files changed + verification results; await review.

#### 117.3 — Tests

Scope: `freminal-buffer/src/buffer/mod.rs` (test modules; extend `line_width_tests`
and `declrmm_tests`).

What: Add buffer-level tests: (a) inserting past half width on a `DoubleWidth`
row wraps at the halved column; same for `DoubleHeightTop`/`Bottom`; (b) with
DECLRMM enabled and a `[left,right]` margin, SU/SD and IND/RI leave columns
outside the margin untouched and shift only columns inside it; (c) with DECLRMM
disabled, SU/SD/IND/RI still perform a full-row scroll (no regression).

Deliverable: green tests for both gaps.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT proceed to 117.4.

Stop: report files changed + verification results; await review.

#### 117.4 — Escape-sequence dual-doc update

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`.

What: Per the `freminal-escape-sequence-docs` skill: in COVERAGE, upgrade SU
(`CSI Ps S`) and SD (`CSI Ps T`) from 🚧 to ✅ and update the DECSLRM /
"CSI Scroll (SU/SD)" summary-matrix rows to reflect full margin confinement;
note the double-width/height half-wrap is now honoured on the DECDWL/DECDHL row.
In GAPS, remove the two "Buffer Semantics Gaps" entries (half-width wrap;
SU/SD + IND/RI confinement) once implemented. Update both "Last updated" headers.

Deliverable: docs match the implementation; markdownlint clean; a sequence marked
✅ in COVERAGE no longer appears as an open gap in GAPS.

Prohibitions: do NOT change source code; do NOT proceed to another task.

Stop: report files changed + markdownlint result; await review.

#### 117.5 — Confine Alternate-buffer LF/IND/RI/NEL scroll to DECSLRM margins (cleanup)

Surface point: subtask 117.2 (task-117 branch, `scroll.rs` confinement).

Bug impact: 117.2's durable design decision assumed the shared
`scroll_region_{up,down}_primary` helpers (scroll.rs:271-284) cover LF/IND/RI/NEL
for all buffer types. They do not — they are `Primary`-only. The
`BufferType::Alternate` arms of `handle_lf` (lines.rs:271) and `handle_ri`
(lines.rs:342) call the unconfined full-row `scroll_slice_{up,down}` directly,
bypassing those helpers. So with DECLRMM active on the alternate screen, LF /
IND / RI / NEL still shift content outside the left/right margins. This is
inconsistent with IL/DL, whose alternate-buffer arms (lines.rs:371, 430) already
branch on DECLRMM correctly. SU/SD are unaffected — `scroll_region_{up,down}_n`
are buffer-type-agnostic and were fully confined in 117.2.

Scope of fix: `freminal-buffer/src/buffer/lines.rs` — the `BufferType::Alternate`
scroll branches of `handle_lf` (line 271) and `handle_ri` (line 342) only.

Suggested approach: mirror the exact IL/DL alternate-arm pattern (branch on
`declrmm_enabled == Declrmm::Enabled`; call `scroll_slice_{up,down}_columns` with
`(scroll_region_left, scroll_region_right)` when active, else the unconfined
variant). Do NOT alter DECSTBM top/bottom logic.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D
warnings`. Extend 117.3's tests to cover the alternate buffer under DECLRMM.

Scheduling: fixed within Task 117 immediately after 117.2 (same branch), before
117.3 so the tests cover both buffer types.

Stop: report files changed + verification results; await review.

---

## Benchmarks

Task 115 touches the vertex builders (`vertex.rs`), a benchmarked hot path per
the `freminal-bench-table` skill. Per `performance-benchmarks`, capture a
before/after for the render-loop / vertex-build benchmarks around 115.2 and
confirm no >15% regression. Tasks 116 and 117 are not on a benchmarked hot path
(input-event handling and buffer edits under test); no benchmark gate required
unless 117.2 shows up in buffer-scroll benches.

---

## Definition of done (whole version)

- All subtasks of Tasks 115, 116, 117 complete and reviewed.
- `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`,
  and `cargo machete` green.
- Escape-sequence docs updated (115.4, 117.4) and internally consistent.
- No >15% render benchmark regression from Task 115.
- `MASTER_PLAN.md` updated per the `freminal-plan-status-lifecycle` skill: on
  merge of each task's PR, its status and the v0.11.1 version row advance to
  `Complete` in both tables.
