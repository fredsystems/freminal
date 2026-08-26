# PLAN_124_RENDER_EFFICIENCY.md — Task 124 "Damage Model Remediation"

> **STATUS: ACTIVATED 2026-08-23.** Task 123 merged (PR #497), its Findings
> section discharged both diagnostic obligations, and the activation recon
> below re-scoped this task from a list of point fixes into a single
> architectural change plus a set of independent leaves.

Task 124 is carried by v0.12.0. See `Documents/PLAN_VERSION_120.md` for the
version summary and `Documents/MASTER_PLAN.md` for roadmap position.

---

## What this task is now, and why it changed name

The stub carried the title "Render Efficiency Remediation" and a list of
nine loosely-related fixes. Activation recon found that three of them —
124.1, 124.2 and 124.3 — are not three problems. They are three symptoms of
**one missing type**: freminal has no way to say "these specific rows
changed", and no way to say "nothing changed".

The task is renamed accordingly. It is a damage-model change with a few
unrelated leaves attached, not an optimisation list.

**The governing principle for this task, set by the maintainer at
activation:** we are doing the hard work to fix the architecture, not
landing a band-aid that improves the number while leaving the shape wrong.
Where a cheaper local fix and a correct structural fix both exist, this
document specifies the structural one.

### The defect, stated once

The dirty signal is destroyed four times on its way from the buffer to the
GPU, each collapse coarser than the last:

| Where | Signal that exists | What survives |
| ----- | ------------------ | ------------- |
| `flatten.rs:1091` | `rebuilt_this_row`, per row | `min()` into one scalar `boundary`, then discarded |
| `scroll.rs:183` | the per-row `Row::dirty` bitset | `.any()` into one `bool` |
| `interface.rs:957` | a true byte-level diff of the whole window | one `bool`, `content_changed` |
| `frame_dirty.rs:301` | — | that `bool` is **discarded** and replaced by `Arc::ptr_eq` |
| `shaping.rs:204` | per-line cache hit/miss | not returned; only `Vec<Arc<ShapedLine>>` |

The fourth row is the sharp one. `TerminalEmulator::flatten_visible`
already computes exactly the right answer — an `O(visible_chars)` byte
comparison that correctly reports "identical" for a row rewritten with the
same content — and the GUI throws it away in favour of a coarser pointer
test. The reason is documented at `frame_dirty.rs:262-281`: `content_changed`
is a **sticky bool** that goes stale across the roughly fourteen unrendered
snapshots published between rendered GUI frames, so the GUI cannot trust it.

That is a type error, not a bug in any predicate. A boolean cannot survive a
many-producers-to-one-consumer publish relationship. A monotonic counter
can.

### The fix, stated once

Replace the boolean with a **per-row content epoch**, bumped in
`freminal-buffer` when a rebuilt row cache entry differs in content from the
one it replaces — not when `Row::dirty` is set. `Row::dirty` means "was
written to"; the epoch means "actually changed". The distinction is the
whole point: full-screen TUIs rewrite unchanged bytes by idiom, which is
precisely the workload that currently pays a full rebuild every tick.

This subsumes and then deletes the emulator's whole-window diff, so it
removes work rather than adding it.

The consumers then gain the two states the type system is missing:

- `FrameDamage::None` — nothing changed, present nothing.
- `PaneFrameDamage::Region(rects)` — these cells changed, present only them.

123's findings argue the second is the larger of the two: the first saves
frames where nothing happened, the second saves every frame where something
small happened. Today the cursor is the only thing that ever got bounded
damage, so selection, hover, search and a single keystroke are all forced to
`Full` because the type cannot express anything else.

### The wall this task deliberately stops at

Having per-row dirty information does **not** by itself let the renderer
upload only the changed rows. `upload_verts` (`gpu.rs:1653`) orphans and
rewrites the whole buffer at offset zero every time, and it has to, because
none of the three instance buffers has a fixed stride per row:

- `bg_instances` emits one instance per **non-default-background** cell, so
  a row's instance count varies with its content.
- `fg_instances` emits one per glyph, varying with ligatures, wide
  characters and blink visibility.
- `deco_verts` interleaves per-row underline and strikethrough quads with
  multi-row selection, search and hover spans, and always appends the cursor
  quad last.

So row N's byte range differs in **length** between frames. One changed row
is an insert-and-shift for every row after it, not an overwrite in place.
Real per-row `glBufferSubData` needs a fixed-stride relayout of the vertex
emission format.

**That relayout is Task 125, not this task**, and it is gated on this task's
measurements showing the residual bandwidth is worth a format change. See
`Documents/PLAN_125_VERTEX_RELAYOUT.md`.

The division is: **Task 124 stops doing work that produces no pixels.
Task 125 makes the work that does produce pixels cheaper.** 123 measured the
prize for each — a needless full rebuild is roughly 350x the bytes for
roughly 1.08x the calls, so 124 is the frame-count and present win and 125
is the bandwidth win.

### A hard constraint on both tasks

**`DefaultBackground` is not a colour. It is "leave these pixels
untouched."**

`vertex.rs:406-415` deliberately emits no quad for such cells, and the
per-frame `GlState::clear` writes the base state they show through —
`(0, 0, 0, 0)` when `background_opacity < 1.0`. Window transparency,
background images and the window post-processing shader FBO all depend on
it. 123's Phase 2 measured 70.2% of a dense-text frame at `alpha == 0` and
pinned it with
`pixel_harness.rs::default_background_cells_are_left_untouched`.

Any change to damage tracking, presents or vertex layout **must preserve
this property**, and no call-count test will catch a violation. The pixel
harness will; use it.

Corollary, recorded because it was asked at activation and will be asked
again: **the clear is not removable and is not the problem.** It is two
zero-byte calls on a GPU fast-clear path, already skipped on partial frames
behind a genuine `EGL_EXT_buffer_age` query
(`egui_integration.rs:1195-1208`), and the present is a real
`eglSwapBuffersWithDamageKHR`. Painting every cell an explicit opaque
background to make the clear redundant would delete Task 34. Painting every
cell the clear colour *including its alpha* would not work either: with
blending enabled an alpha-zero quad writes nothing and so does not erase
stale pixels, and disabling blending to force a replace breaks the image and
shader compositing layers. The clear fires too often today; that is fixed by
`FrameDamage::None`, not by removing it.

---

## Subtask summary

Numbers are stable. 124.1, 124.2, 124.3, 124.5 and 124.6 keep their original
numbers with re-pointed scope; new work takes new numbers from 124.10.

| Subtask | Title | Status |
| ------- | ----- | ------ |
| 124.1 | Dirty-row `Arc` churn (umbrella) | Resolved by 124.10–124.12 |
| 124.2 | `FrameDamage::None` — a frame that changed nothing presents nothing | **Complete** — `03b8082a` |
| 124.3 | Cell-granular pointer suppression, and correct `?1016` delivery | **Complete** — `4644a8f9`, `8f518987`, `e9b33ec0`; physical capture complete |
| 124.3a | Immediate-report foundation + correct `?1016` encoding | Complete — `4644a8f9` |
| 124.3b | Cell-boundary repaint decision (post-124.3a) | Complete — `8f518987`, `e9b33ec0`; live measurement complete |
| 124.4 | Named-field struct for the pointer-motion predicate | Complete |
| 124.5 | Decide and execute the chrome cache's fate | Complete — deleted |
| 124.6 | Shaping-path levers | **Complete** — `9dbaaf47`; lever 1 only, lever 2 unsized |
| 124.7 | GPU buffer-orphaning for small payloads | Complete |
| 124.8 | `DESIGN_DECISIONS.md` entry for the Phase 0 / Task 121 outcome | Complete |
| 124.9 | `sync_atlas` re-uploads glyphs a full upload already covered | Complete |
| 124.10 | Per-row content epoch in `freminal-buffer` | Complete |
| 124.11 | `row_epochs` on `TerminalSnapshot`; delete `content_changed` | Complete — field landed here, deletion landed in 124.12b |
| 124.12 | GUI consumes epochs; delete the `Arc::ptr_eq` content test | Complete |
| 124.13 | Re-measure pointer-motion suppression rates | Complete |
| 124.14 | `PaneFrameDamage::Region` and `VertexRebuild::Bounded` | **Complete** — a/b/c/d landed; search expanded into unified surface damage |
| 124.14a | Bound row-only damage | **Complete** — `eae76d1b` |
| 124.15 | Measure chrome's per-frame cost | Complete |
| 124.16 | Shaping cache instrumentation and a TUI-redraw benchmark | Complete |
| 124.C1 | `decide_frame_damage`'s doc comment describes a removed term | Complete |
| 124.C2 | `sync_toast_atlas` carries the same defect as 124.9 | Complete |
| 124.17 | Does the skip-clear + partial-present path ever actually fire? | Complete — re-taken on GPU; answer was **no** |
| 124.18 | Make partial present actually work (gate + clipping) | **Complete** |
| 124.19 | Phase 3: an egui-level offscreen pixel harness | **Complete** — 124.19a extraction, 124.19b harness |
| 124.20 | Scissor the clear to the redraw region, don't skip it | **Complete** |
| 124.C3 | `merge_cache` has no per-buffer stash, so alt-screen round trips over-report | Closed unexecuted — maintainer decision; risk exceeds one-rebuild benefit |
| 124.C4 | A pixel golden must not be compared across rasterisers | **Complete** |
| 124.21 | Exhaustive audit of every full-repaint-forcing trigger | **Complete** — 52 triggers, 8 genuinely global |
| 124.22 | `freminal-damage-model` agent skill | **Complete** — `3de34651` |
| 124.C5 | Inline image placement is invisible to the row epoch | **Complete** — gate on 124.14a lifted for placement, see below |
| 124.C7 | `CursorDamage` is a misnomer once `Region` carries it | **Complete** — renamed to `PaneDamageRect`, `7e63bb8c` |
| 124.23 | The full-draw paint arm ignores the published present region | **Complete** — 124.14 unblocked; two residual gaps recorded |
| 124.14b | Bound `selection_changed` (b-i) and `hover_changed` (b-ii) | **Complete** — `edf9e017`, `284ce253`; gutter hazard disproved |
| 124.14c | Stop a busy pane forcing full damage on unchanged siblings | **Complete** — `058c2627` |
| 124.14d | Bound search highlights and the search overlay | **Complete** — `ab275052` |
| 124.C6 | `search_corpus` + open fold desyncs `merge_cache` permanently | **Complete** — isolated search flatten, `c77047bd` |
| 124.C8 | Run-cache key omits per-character widths | **Complete** — `d94eefdc` |
| 124.C9 | Bounded rebuild omits previous/current cursor rows | **Complete** — `d94eefdc` |
| 124.C10 | Review hardening for measurement infrastructure | **Complete** — `a8875ab0` |
| 124.C11 | Search-overlay safety composition is duplicated | **Complete** — `670a5288` |
| 124.C12 | Review-flagged documentation drift | **Complete** — `046d28f1` |

### Execution model

```text
independent leaves (may run at any time, in parallel):
  124.4   124.7   124.9   124.8   124.C1

the epoch chain (strictly sequential, the spine of this task):
  124.10 -> 124.11 -> 124.12 -> [124.17] -> [124.18] -> 124.14 -> 124.2

measurement-then-decision pairs (each pair sequential; pairs parallel):
  124.13 -> 124.3
  124.15 -> 124.5
  124.16 -> 124.6
  124.17 -> 124.18 -> 124.14, 124.2   (see 124.18)
```

**124.18 was inserted into the spine on 2026-08-23, after 124.17's GPU
re-take.** 124.17 was written to ask whether the skip-clear +
partial-present path ever fires. Re-measured on real hardware, the answer is
**no — not once in 8,160 frames** — because the gate requires
`buffer_age() == 1` and a double-buffered surface reports 2. Forcing it open
by hand then corrupted the display, confirming a second, independent defect:
a partial frame still paints unclipped opaque chrome over the whole surface.
124.14 and 124.2 both spend this path, so neither may be built until it
works. See 124.18.

The epoch chain edits shared types in `freminal-buffer`,
`freminal-terminal-emulator` and the GUI in sequence, so per
`parallel-work-isolation` it is one workstream with one active editor. The
leaves touch disjoint files and are safe to run concurrently with it. The
three measurement pairs may run concurrently with each other but 124.15 and
124.3 both read frame-path state, so land them one at a time.

**Ordering caution carried from 123.** If the chrome cache is deleted
(124.5) *and* `FrameDamage::None` lands (124.2), chrome becomes the thing
forcing `Full` on frames the grid would otherwise skip. That is why 124.15
measures chrome's per-frame cost **before** 124.5 decides, and why 124.5
should not land before 124.2 is at least designed.

---

## Subtasks

### 124.1 — Dirty-row `Arc` churn forces a full rebuild on byte-identical content

**Status: resolved by 124.10, 124.11 and 124.12. Retained for the record;
do not implement from this entry.**

Premise **CONFIRMED** by Task 123, pinned by
`frame_dirty.rs::byte_identical_reflatten_in_a_new_arc_still_forces_a_full_rebuild`.

The stub offered three candidate directions — content-hash the merged bytes,
return the previous `Arc` when identical, or propagate real per-row dirty
information across the snapshot boundary. The maintainer chose the third at
activation, on the explicit grounds that the first two improve the number
while leaving the architecture wrong.

Recorded for the next reader so it is not re-litigated: returning the
previous `Arc` on a byte-identical merge is a **correct but insufficient**
fix. It repairs `Arc::ptr_eq`'s accuracy without repairing its *granularity*
— one changed row still invalidates the whole pane — so it would have to be
undone to build 124.14 on top. Content-hashing has the same ceiling and adds
a hash of the whole window to a path the per-row design does not need.

123 also corrected the cost model: the waste is **overwhelmingly bandwidth,
not call count**, roughly 350x the bytes for roughly 1.08x the calls. Do not
justify or measure this work on call counts.

### 124.2 — `FrameDamage::None`: a frame that changed nothing presents nothing

*Re-pointed. 123 refuted this entry's original diagnosis and confirmed its
symptom.*

`pointer_forces_full_present` is **not** implicated and must not be
"fixed" — 123 pinned that with
`frame_damage.rs::pointer_motion_over_content_is_partial_once_the_toast_confound_is_removed`,
plus a companion proving motion over chrome does still force `Full`, so the
predicate is neither dead nor guilty. The original
`frame_damage_full=120, frame_damage_partial=0` observation is fully
explained by the `toast_active=48` confound recorded alongside it.

The real mechanism, pinned by
`frame_damage.rs::pointer_motion_over_inert_content_is_full_via_the_unchanged_fallback`:
when every pane reports `PaneFrameDamage::Unchanged`, no damage rect is ever
pushed, `decide_frame_damage` reaches `if rects.is_empty()` and returns
`FrameDamage::Full`. A frame in which nothing whatsoever changed is
presented as a full clear plus a full present. 123's Phase 2 then proved the
work is not merely redundant-looking but **provably without effect**:
rendering identical state twice yields **zero differing pixels at a channel
bound of zero**, pinned by
`pixel_harness.rs::repainting_unchanged_state_produces_identical_pixels`.

Scope: `freminal-windowing/src/lib.rs`, `freminal-windowing/src/egui_integration.rs`,
`freminal/src/gui/frame_damage.rs`, and the `decide_frame_damage` call site.

What: add a third variant to `FrameDamage`. `Full` remains `#[default]` —
the safe fallback must stay the one that repaints. `decide_frame_damage`
returns `None` when no pane pushed a rect **and** no force-full or
toast short-circuit fired; today that case falls through to `Full`.
`egui_integration` skips the clear, the paint and the swap entirely on
`None`.

The `buffer_age()` interaction is the correctness crux and must be handled
explicitly, not assumed: skipping a swap means the next frame's buffer age
is **not** 1, so a subsequent `Partial` frame's existing guard
(`egui_integration.rs:1195-1204`) will correctly decline the partial path
and fall back to a full clear. Verify that this is what happens rather than
relying on it; a wrong answer here is silent visual corruption, which is the
issue #432 class.

Also fold in **124.C1** if not already landed, since it edits the same doc
comment.

Deliverable: the variant, the branch, the `egui_integration` skip, unit
tests in `frame_damage.rs` covering the new `None` case and confirming the
existing short-circuits still take precedence, and a pixel-harness test
proving a skipped frame leaves the surface identical.

Verification: `cargo test --all`;
`cargo clippy --all-targets --all-features -- -D warnings`; `cargo machete`;
the Phase 2 pixel harness; `cargo xtask check-windows` before the PR.

Prohibitions: do NOT modify `pointer_forces_full_present`. Do NOT change
`FrameDamage`'s default. Do NOT touch the vertex layer.

#### 124.2 implementation notes (commit `03b8082a`)

**Landed.** `FrameDamage` gains a `None` variant; `Full` remains the
`#[default]` fallback, unchanged from the entry above.
`decide_frame_damage`'s `Unchanged`-or-no-rect case now resolves to `None`
rather than falling through to `Full`, **unless an earlier `Full`
short-circuit has already fired** — those short-circuits keep precedence
exactly as specified. `ChromeDamage::Changed` upgrades a `None` decision to
`Full`, so chrome activity is never silently dropped by the new state.

A `None` frame still runs egui's UI pass and still computes damage and
platform output, and still performs texture-delta bookkeeping — it is not a
full early-return.
What it skips is the work downstream of "there is nothing to paint": shape
partitioning, tessellation, the framebuffer clear, primitive paint, the
pre-present notification, and the swap itself. No `DamageHistory` entry is
recorded for a `None` frame, because there is no swap to record one against.

The presentation outcome is carried as a named type, `FramePresentation`,
distinguishing `None`, `Full` and `Partial` rather than a bare bool or an
overloaded reuse of `FrameDamage`. Profiling instrumentation separates four
buckets rather than collapsing them: `None` decided before composition,
`None` surviving to the final decision, `Full`, and `Partial` — so a `None`
that gets upgraded by chrome activity is visible as a distinct count, not
folded into either terminal state. Coverage spans both GL paths and is
verified at exact-pixel granularity, not merely by call count.

#### Post-124.2 GPU measurement (2026-08-25), AUTHORITATIVE

Measured on this workstation — Hyprland/Wayland, AMD GPU
(`LIBGL_ALWAYS_SOFTWARE` printed `<unset>`; the live process's llvmpipe
thread count was 0) — release build with `--features frame-profiling`. The
floating window was pinned to 1264x681 at `(-2400, 200)`; the pointer was
held at `(-1800, 500)`. The continuous PTY workload was
`sh -c 'while :; do seq 1 200; sleep 0.02; done'`. Steady state is frame 3720
through frame 6120 — 2,400 frames over 39.838837s.

| Metric | Value |
| ---------------------------------------- | --------- |
| fps | 60.24 |
| total us/frame | 301.38 |
| run_ui us/frame | 118.20 |
| tessellate us/frame | 10.04 |
| paint us/frame | 14.06 |
| swap us/frame | 141.12 |
| final `None` count | 281 |
| `Partial` taken count | 2104 |
| `Full` count | 15 |
| buffer-age-blocked count | 0 |
| `buffer_age_histogram` delta `[0,1,2,3+]` | `[0,0,2104,0]` |
| pointer events | 537 |

No visual corruption was observed.

**Comparison against the post-124.14 output-only controls (runs 2 and 3 of
that measurement, 329.02 and 406.57 us/frame total), recorded as
observation, not causation:**

- Total per-frame cost is lower with 124.2 landed: 301.38 us/frame here
  against 329.02-406.57 us/frame in the post-124.14 output-only controls.
- The zero-change component that the post-124.14 measurement reported as
  `Full` (273-280 frames per run, in that measurement's terms) is reported
  here as `FrameDamage::None` (281 frames) rather than `Full`.
- `Partial` remains the dominant bounded path in this run, as it did in the
  post-124.14 measurement.
- `buffer_age()` again resolves to exactly 2 in every steady-state query
  (`[0,0,2104,0]`), and every requested `Partial` frame is taken
  (buffer-age-blocked count 0) — the same pattern the post-124.14
  measurement recorded.

These are the figures as measured; no stronger causal claim is drawn from a
single run than what is stated above.

### 124.3 — Cell-granular pointer suppression, and correct `?1016` delivery

*Migrated from 121.15 + 121.17. Gated on 124.13. **Expanded 2026-08-25**
after the mouse-report-delivery recon below. The original scope is kept as
the historical premise rather than deleted, but two of its claims did not
survive recon and are corrected in place — marked, not silently rewritten —
so the next reader can see what changed and why.*

Nearly all interactive terminal state changes at **cell** granularity — URL
hover, gutter hover, selection extent. **Corrected 2026-08-25:** the
original text also listed "mouse-tracking reports" in that set, and said
suppression would "remain correct for mouse-tracking mode." Neither survives
recon — see the recon block below. `?1016` (`SgrPixels`) reports sub-cell
pixel motion by design, so a mouse-tracking report is not always
cell-granular, and today's implementation does not honor that regardless of
what this task does. Pointer motion within one cell genuinely cannot change
any of the cell-granular state, so caching the pane terminal-rect origin and
logical cell size and suppressing any `CursorMoved` that does not cross a
cell boundary would still remove the pane-wide `has_urls` and
`scroll_offset` vetoes, let selection drags suppress, and subsume the
gutter carve-out. **It must not, by itself, also suppress delivery of a
mouse-tracking report** — that is a distinct problem, addressed by
124.3a/124.3b below, not solved by cell-boundary suppression alone.

**The scrollbar must stay excluded** — thumb dragging is genuinely
pixel-granular.

The Task 122 seam exists: 122.15 publishes `pane_terminal_origin(pane_id)`,
and the reader currently carries an `#[allow(dead_code)]` with a TODO naming
this work. **Remove that allow when landing.**

123 sharpened the case rather than weakening it: a whole pointer-motion
decision costs 33–423 ns, so **the predicate's own cost is irrelevant** and
what costs money is whether the event causes a repaint at all — roughly five
orders of magnitude apart in bytes moved. Suppression is the lever.

Note the interaction with 124.2: `FrameDamage::None` makes an unsuppressed
motion frame nearly free at the *present* layer, but it still costs the
whole GUI frame walk. Suppression is still worth having; its measured prize
just changes, which is what 124.13 is for.

The old 2026-07-29 numbers in the stub (99.16% suppression clean, 1.68% with
one OSC 8 URL on screen) **predate the chrome cache being disabled by
121.32** and must not be reused. 123 could not re-take them — pointer motion
is not a renderer workload — so 124.13 does.

**Corrected 2026-08-25, superseding the conclusion (not the measurement) of
124.13's "Not a target for 124.3: `mouse_tracking_active`" note.** That
note's measurement block is left exactly as written below — this corrects
what follows from it, in this entry, not there. 124.13 found
`mouse_tracking_active` a pane-wide veto that "defeats suppression... for
the same pane-wide reason, but correctly," on the reasoning that an
application receiving mouse reports must be sent every motion event, full
stop. That much is true — but it is a claim about *whether* a report is
sent, and says nothing about *what* the report contains once sent, which is
not always cell-granular. `mouse_tracking_active` **is** a target for
124.3: not the repaint-suppression target 124.13's percentages describe,
but the target of the delivery-path and pixel-encoding fix in 124.3a below.

#### Recon: how a mouse-tracking report actually reaches the PTY (2026-08-25, read-only)

- `WindowEvent::CursorMoved` is forwarded into egui via
  `state.egui.on_window_event(...)` (`freminal-windowing/src/event_loop.rs`,
  the fast-path block around line 611) unconditionally; only *repaint
  scheduling* is gated after that. `App::pointer_motion_needs_repaint`
  (`freminal/src/gui/app_impl.rs:983`) decides that scheduling, delegating
  to `pointer_motion_needs_repaint_decision`
  (`freminal/src/gui/pointer_motion.rs:236`), whose pane-signal term is
  `mouse_tracking_active || hover_region_risk` (line 246): today
  `mouse_tracking_active` alone forces that decision to `true`. This is a
  *scheduling* forcing term, not a delivery guarantee, and the two are
  currently conflated only because nothing else guarantees delivery.
- The queued `egui::Event::PointerMoved` is consumed — and the PTY report
  actually encoded and written — only inside `write_input_to_terminal`'s
  `Event::PointerMoved` arm (`freminal/src/gui/terminal/input.rs:2297`,
  inside `write_input_to_terminal` at line 1590), which runs only as part
  of a frame. If `pointer_motion_needs_repaint` returns `false` for a given
  motion and nothing else requests a redraw, that frame does not run: the
  event sits queued in egui-winit's input buffer until some later frame
  runs for an unrelated reason, at which point it is delivered late. Today
  this is masked only by `mouse_tracking_active` forcing scheduling to
  `true` — remove that forcing term (as cell-boundary suppression in this
  entry's original scope would, unmodified) without first giving report
  delivery its own path, and reports would be delayed exactly as this
  paragraph describes.
- Coordinate conversion loses pixel information *before* encoding, not
  inside it. `encode_egui_mouse_pos_as_usize`
  (`freminal/src/gui/terminal/coords.rs:152`) floor-divides the raw pointer
  position by cell size into `FreminalMousePosition`'s character
  column/row before `freminal/src/gui/mouse.rs` ever sees a position.
  `mouse.rs`'s `encode_x11_mouse_button` / `encode_x11_mouse_wheel`
  (lines 293 and 341) then branch only on
  `*encoding == MouseEncoding::X11`; every other encoding — `Sgr` (`?1006`)
  and `SgrPixels` (`?1016`) alike — takes the same one-based
  cell-coordinate `else` branch. `MouseEncoding::SgrPixels` exists and is
  advertised (`freminal-common/src/buffer_states/modes/mouse.rs`, mode
  number 1016, confirmed in `freminal-common/tests/mouse_mode_tests.rs`)
  but nothing downstream of coordinate conversion distinguishes it from
  `Sgr`. This is a pre-existing correctness gap, not new breakage, and it
  falsifies the "reports are per-cell" premise this entry's original text
  relied on.
- Immediate delivery also needs, at the point a `CursorMoved` is observed
  outside the frame path: the currently-held button state for `?1002`
  (button-motion tracking) and `?1003` (any-motion tracking), current
  modifiers, which pane the pointer resolves to (active-pane routing must
  not change shape from today), current scrollback offset (a scrolled-back
  pane must not emit reports for content that is not live), and the same
  input suppressors `write_input_to_terminal` already gates on via
  `InputSuppressors` (`modal_or_drag`, `context_menu`, `search_overlay`,
  `command_history`, `scrollbar_drag`) — none of which have a home
  outside a frame today.
- `PublishedFrameState::pane_terminal_origin`
  (`freminal/src/gui/published_frame_state.rs:264`) was built by 122.15 for
  exactly this purpose and is already published once per frame per pane.
  It carries origin only — logical cell size, pixels-per-point, and
  per-pane input-suppression state have no published home yet.

#### Maintainer decision (2026-08-25)

Expand 124.3 and fix `?1016` now, rather than preserve the incorrect
per-cell premise or defer the fix to a later version. The cell-boundary
repaint suppression this entry originally scoped is still correct and
still lands, but not until report delivery has its own path independent of
repaint scheduling.

#### Settled design

1. Add generic, PTY-semantics-ignorant synchronous pointer-motion and
   pointer-button-held-state observation hooks at the
   `freminal-windowing::App` boundary, parallel to the existing
   `pointer_motion_needs_repaint` / `is_chrome_interactive_at` hooks.
   Windowing remains ignorant of mouse-tracking modes or PTY encoding. The
   motion hook runs independently of repaint scheduling — it is not gated
   by, and does not gate, `pointer_motion_needs_repaint`'s return value.
   The button-held-state hook observes held state synchronously for the
   motion hook's use; button press/release PTY emission itself stays
   exactly where it is today, in the existing frame path.
2. Give immediate PTY motion-report state (last-sent position, held
   buttons relevant to `?1002`/`?1003`, etc.) a GUI-owned per-pane home. It
   is read and written outside a frame, but `ViewState` (scroll, mouse,
   focus) is already GUI-thread-owned per-pane state that persists outside
   a frame, so it is not ruled out on that basis. In fact `ViewState`
   already declares a same-named, currently-unused
   `previous_mouse_state: Option<PreviousMouseState>` field
   (`freminal/src/gui/view_state.rs:503`) that is a candidate ownership
   seam — unlike the field of the same name actually in live use today,
   `PaneRenderCache::previous_mouse_state`
   (`freminal/src/gui/terminal/widget.rs:1327`), which is frame-scoped
   render-cache state and must not gain out-of-frame readers. `PreviousMouseState`
   itself (`freminal/src/gui/mouse.rs:20`) carries only a cell-granular
   `FreminalMousePosition` and is insufficient for `SgrPixels`'s
   pixel-granular position, so implementation must choose or define a
   named report-state representation with the pixel precision `?1016`
   needs — reusing `ViewState`'s ownership seam if it fits, but not
   reusing `PreviousMouseState`'s type, and not adding report state to
   `PaneRenderCache` merely to get out-of-frame access. Per
   `freminal-state-representation`, this is named event/state types, never
   a bool parameter and never a bare `true`/`false` threaded through the
   new hooks. No new crate — this is `freminal` GUI state, same tier as
   `ViewState` today.
3. Publish one per-pane, last-completed-frame snapshot of exactly the
   geometry and suppressors report delivery needs outside a frame:
   terminal bounds/origin (already published — extend, do not duplicate,
   `pane_terminal_origin`), logical cell size, pixels-per-point, and the
   exact `InputSuppressors` fields (`modal_or_drag`, `context_menu`,
   `search_overlay`, `command_history`, `scrollbar_drag`). Unknown or
   stale/unavailable published data is conservative on both axes: no
   immediate report is sent, and repaint scheduling is left unaffected
   (never suppressed on the strength of missing data).
4. Move — do not duplicate — PTY motion-report encoding and sending out of
   `write_input_to_terminal`'s `Event::PointerMoved` arm into the new
   synchronous hook path. Selection-extent updates, hover-state updates,
   recording's `EventPayload::MouseMove` emission, and any other
   terminal-owned visual-state processing that arm also performs stay
   exactly where they are, in the queued egui `PointerMoved` path — only
   the PTY report itself moves. A single physical motion event must
   produce at most one PTY report; the two paths must never both fire for
   the same motion.
5. Implement `?1016` correctly: the same SGR framing as `?1006`
   (`\x1b[<{cb};{x};{y}M`/`m`), but `x`/`y` are one-based *physical pixel*
   coordinates relative to the terminal content area's top-left, not cell
   column/row. Ordinary X11 and `?1006` SGR keep exactly their current
   one-based cell coordinates — this is additive, not a behavior change to
   the existing encodings. A `SgrPixels` motion report must reflect
   within-cell pixel movement even on a frame where the cell-boundary
   repaint decision (point 6) declines to schedule a repaint.
6. Repaint scheduling stays a separate axis from report delivery. Remove
   `mouse_tracking_active` as a forcing term in
   `pointer_motion_needs_repaint_decision` **only after** 124.3a's tests
   prove immediate report delivery works without relying on it. The
   cell-boundary decision uses the previous and current pointer position
   together with the published pane origin/cell size, so motion that stays
   within one cell can suppress repaint for cell-granular terminal-owned
   effects (hover, selection extent, URL span) while `SgrPixels` still gets
   its pixel-granular report via the point-1 hook regardless. Scrollbar
   dragging and any other genuinely pixel-granular/chrome/overlay condition
   remain repaint-forcing exactly as today.
7. Preserve current active-pane routing and the existing one-frame
   focus-follow transition semantics unchanged (issue #495's
   `focus_change_pending` term). Do not opportunistically change which
   pane receives a report as a side effect of this work.

#### Implementation decomposition (124.3a / 124.3b — no new roadmap task numbers)

- **124.3a** — Immediate-report foundation: the synchronous motion and
  held-button observation hooks, the per-pane GUI-owned immediate-report
  state (point 2), the published per-pane geometry/suppressor snapshot
  (point 3), the move of PTY motion emission out of
  `write_input_to_terminal` (point 4), and correct `?1016` encoding
  (point 5). Repaint scheduling is **unchanged** by this subtask —
  `mouse_tracking_active` keeps forcing `pointer_motion_needs_repaint` to
  `true` until 124.3b lands. Ordinary X11 and `?1006` SGR reports are
  preserved except for earlier delivery timing (a report that was
  previously delayed until an unrelated repaint now arrives on its own
  motion event); `?1016` intentionally changes observable behavior, from
  today's incorrect cell coordinates to correct pixel coordinates — that
  is the fix, not a regression.
- **124.3b** — Cell-boundary repaint decision (point 6): once 124.3a's
  tests prove reports are delivered correctly without relying on the
  forced-`true` scheduling path, remove `mouse_tracking_active` from
  `pointer_motion_needs_repaint_decision`. The pane-wide `has_urls` veto
  can be replaced by the cell-boundary-crossing test, subsuming the gutter
  carve-out as originally scoped, because URL span, selection extent, and
  gutter row are all cell-granular. The pane-wide `scroll_offset` veto is
  a separate case: it exists because scrollbar hover/drag is genuinely
  pixel-granular, not cell-granular, so it cannot be replaced by the same
  cell-boundary test. The pane-wide `scroll_offset` veto can be removed
  only once a precise, positionally-scoped scrollbar-hover/drag forcing
  region exists to take over what it was actually protecting; until then
  it stays.

#### Tests / deliverables

- Immediate report is emitted on a `CursorMoved` even when
  `pointer_motion_needs_repaint` returns `false` for that same event —
  proving the two axes are actually decoupled, not merely documented as
  decoupled.
- No duplicate report: a single physical motion produces exactly one PTY
  write, verified against both the old frame-path site (now removed) and
  the new hook path.
- `?1002` held-button and `?1003` any-motion tracking behavior, active-pane
  routing, scrollback-offset suppression, and every existing
  `InputSuppressors` field (`modal_or_drag`, `context_menu`,
  `search_overlay`, `command_history`, `scrollbar_drag`) all still hold
  from the new path.
- Exact-byte tests: ordinary SGR (`?1006`/X11) cell-coordinate output is
  unchanged; `?1016` pixel-coordinate output is correct, including a
  two-moves-within-one-cell case that must produce **two** distinct pixel
  reports but (once 124.3b lands) **no second repaint**.
- Conservative-fallback tests: no immediate report, and repaint scheduling
  unaffected, when geometry has not yet been published or pane resolution
  fails.
- Regression coverage proving existing focus-follow-mouse and
  scrollbar-drag behavior is unchanged by both subtasks.

#### Documentation

`freminal-escape-sequence-docs`'s mandatory dual-document update — to
`Documents/ESCAPE_SEQUENCE_COVERAGE.md` and
`Documents/SUPPORTED_CONTROL_CODES.md` — applies **when `?1016` behavior
actually lands** in 124.3a's implementation pass. This entry is a
documentation-only recon-and-design edit to the plan document; it does not
change escape-sequence behavior, so the dual-doc update is explicitly
deferred to that implementation pass and is not done here.

#### Performance verification

Use 124.13's table as the authoritative before-measurement; do not retake
it here. 124.3a's and 124.3b's own verification must each report, per
`PROFILING.md`: the post-change suppression rate paired with the observed
event rate (as 124.13 established the pairing requirement), plus frame
rate and per-frame cost. A suppression-percentage change presented without
the paired event rate is not an acceptable performance result, and 124.13's
own findings block is not to be edited to produce one.

#### Implementation notes (2026-08-25)

**124.3 status: `Complete`.** Both 124.3a and 124.3b are implemented,
reviewed and merged to the working branch; the live-pointer measurement
this section's own Performance Verification clause requires was
subsequently captured with a physical pointer device — see
"Post-124.3 physical-pointer measurement (2026-08-25), AUTHORITATIVE"
below. The synthetic-capture attempt that follows immediately is kept
verbatim as the historical record of why a compositor-driven cursor could
not exercise this path; it is superseded, not deleted.

**124.3a — implementation complete, commit `4644a8f9`.**

- Added synchronous PTY-agnostic pointer motion/button/presence hooks in
  windowing; motion delivery runs independently of repaint scheduling.
- Immediate PTY motion reports route only to the active pane, gated by the
  terminal rect, GUI scrollback offset, the exact five input suppressors,
  and published geometry.
- Held-button state is window-owned; per-pane report-position history is
  `ViewState`-owned; pointer/focus loss resets both relevant state halves.
- Frame-time `PointerMoved` no longer sends PTY motion bytes, preventing
  duplicates; terminal-owned selection/hover/recording processing remains
  in the queued path.
- `?1016` now emits one-based terminal-relative physical pixel coordinates
  for motion/button/wheel; ordinary X11/`?1006` output is unchanged; `?1005`
  behavior is intentionally unchanged.
- The mandatory dual escape-sequence docs (`ESCAPE_SEQUENCE_COVERAGE.md`,
  `SUPPORTED_CONTROL_CODES.md`) were updated for the `?1016` fix.

**124.3b — implementation complete, commit `8f518987`; instrumentation
wording fix `e9b33ec0`. Live measurement pending.**

- Added previous/current pointer history at the `App` repaint-decision
  boundary while preserving the unconditional report-hook ordering and the
  existing scheduling edge latch.
- Removed `mouse_tracking_active`, the pane-wide `has_urls` veto, and the
  pane-wide `scroll_offset` veto as repaint-forcing terms.
- Added exact per-pane classification of a pointer position: Content cell,
  Gutter row, scrollbar pane/hit rect, Outside, Unknown. URL/selection/
  gutter classification compares cells/rows; scrollbar classification
  compares against the pane-aware hit boundary; a drag in progress remains
  unconditionally repaint-forcing; unknown/layout/lookup failures force
  conservatively rather than suppress.
- A pointer crossing between two quiet panes — neither showing a
  cell-granular visual effect — does not force a repaint on its own;
  multiple simultaneously-selecting panes still force conservatively.
- The scrollbar hit rect and the current drag state are published after
  scrollbar processing runs; one shared helper owns the hit-rect geometry
  so the published value and the input-handling value cannot drift apart.
- Frame-profiling instrumentation now reports the ten actual
  repaint-forcing conditions this design introduces, replacing stale
  observations from before 124.3b.
- Two motion events that both land within one cell but at different
  `SgrPixels` sub-cell positions are pinned as two distinct PTY reports,
  while the second of the two suppresses its repaint — the decoupling this
  entry's design mandated.

**Review corrections worth retaining because they protect correctness:**

- Per-pane held-button state was rejected: a focus or tab change between
  press and release could strand the press in a pane that never sees the
  release.
- A bare scrollbar-presence bool was rejected: a transition from
  scrollbar-A to scrollbar-B is `true` to `true` under a bool but still
  requires a repaint, so the classification must be positional, not
  boolean.
- Lookup or layout failure, and any case of missing published geometry,
  resolves to `Unknown` and forces a repaint — never to a quiet `Outside`
  classification, which would suppress silently on missing data.
- The gutter's classification boundary is the exact half-open row range
  production hover logic already uses, not a re-derived approximation.
- The pane split layout is computed once per pointer event and shared by
  both the previous-position and current-position classification, so the
  two resolutions cannot see different geometry within the same event.

**Verification independently run and green for both commits:**

- `cargo fmt --all -- --check`
- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo machete`
- `cargo xtask check-windows`
- pre-commit hooks passed for both implementation commits

**Measurement blocker, dated 2026-08-25.** `LIBGL_ALWAYS_SOFTWARE` was
unset; a release frame-profiling binary was built; the environment is an
AMD GPU under Hyprland. The target window was successfully floated, moved
and resized to 1264x681 at `(-2400,200)`, and the cursor was visibly moved
to `(-1800,500)`. `hyprctl dispatch movecursor` (`hl.dsp.cursor.move`)
changed the compositor's cursor position but produced zero application
`CursorMoved` events — `pointer_repaint_checks_total` and the pointer
scheduled/suppressed counters all stayed at zero. This reproduces the
earlier finding that synthetic compositor-driven cursor motion cannot
exercise this path. Therefore **no post-124.3 event rate, suppression
percentage, or frame-rate/cost comparison is claimed here.** A physical
pointer-device capture, paired per `PROFILING.md`, is still required before
124.3 can be marked Complete. No visual corruption was observed during the
output-only workload exercised in this attempt, but that is not a
substitute for the missing pointer-path validation, and the unrelated
output-only frame costs captured during this failed attempt are not
recorded as a 124.3 measurement. This blocker is retained verbatim as the
historical record of the failed synthetic attempt; it was superseded by a
physical pointer-device capture — see "Post-124.3 physical-pointer
measurement (2026-08-25), AUTHORITATIVE" immediately below.

#### Post-124.3 physical-pointer measurement (2026-08-25), AUTHORITATIVE

Captured with a **physical** pointer device, discharging the blocker
above. Release build with `--features frame-profiling`;
`LIBGL_ALWAYS_SOFTWARE` unset; the live process's llvmpipe thread count
was 0 — Hyprland/Wayland, AMD GPU, matching the environment used for the
post-124.2 GPU measurement. The floating focused window was 1264x681 at
`(-2400, 200)`. The PTY workload was `btop` launched directly — a
mouse-reporting, full-screen TUI — and the maintainer moved the physical
mouse continuously over the terminal grid for the duration of the
capture.

The steady interval is differenced from the 120-frame profiling flush at
`23:03:56.051360Z` to the 360-frame flush at `23:04:44.982403Z` — 240
frames over 48.931043s.

| Metric | Value |
| ---------------------------------------- | ------------------- |
| fps | 4.90486 (240 frames / 48.931043s) |
| real `CursorMoved` checks | 45,027 (920.213 events/s) |
| pointer scheduled (delta) | 51 |
| pointer suppressed (delta) | 44,976 |
| suppression | 99.8867% |
| total us/frame | 491.4125 |
| run_ui us/frame | 322.5542 |
| tessellate us/frame | 8.1167 |
| paint us/frame | 43.2875 |
| swap us/frame | 97.8292 |
| app_update us/frame | 271.55 |
| panes us/frame | 190.9583 |
| orchestration us/frame | 11.775 |
| final presentation: `None` (delta) | 110 |
| final presentation: `Full` (delta) | 6 |
| final presentation: `Partial` (delta) | 124 |
| `Partial` taken (delta) | 124 |
| buffer-age-blocked (delta) | 0 |
| `buffer_age_histogram` delta `[0,1,2,3+]` | `[0,0,124,0]` |

The three presentation-outcome deltas (110 + 6 + 124) sum to the full 240
frames, as expected.

Forcing-condition windows over this interval: `overlay_open` 16,
`first_motion` 2, `chrome_interactive` 29. These sum to 47; the remaining
four of the 51 scheduled events are consistent with the pre-existing
previous-needed/current-needed transition latch. This is recorded as
attribution of the already-existing scheduled events, not as a new
forcing behavior introduced by this capture.

**Interpretation, not overstated:**

- This directly validates the maintainer's qualitative result: `btop` no
  longer sustains a frame per mouse report. At roughly 920 physical
  pointer events per second, 99.8867% are suppressed and the UI draws at
  roughly 4.90 fps — a rate driven mostly by `btop`'s own TUI/normal
  repaint cadence, not by every pointer event forcing a frame.
- Compare carefully to pre-work symptoms: earlier mouse-reporting
  captures had `mouse_tracking_active` defeat suppression entirely, with
  mouse movement associated with roughly 60 fps.
- Do **not** compare the 491.4125 us/frame figure here directly against
  the post-124.2 `seq`-output measurement's 301.38 us/frame as equivalent
  workloads — `btop` is a different, and heavier, TUI than the `seq`
  synthetic workload, so the two totals are not a like-for-like
  before/after pair.
- This completes 124.3's own target as scoped above. It does **not**
  claim parity with WezTerm. The maintainer still observes roughly 0.1%,
  occasionally 0.2% on a slower laptop, of continued cost against this
  workload, where WezTerm and other emulators show no comparable load.
  That residual is recorded here as outstanding performance evidence, not
  as an in-scope fix or a cleanup task for this subtask — any further
  attribution of that residual must be separately scoped after Task 124
  rather than silently expanding 124.3.
- No visual corruption was explicitly recorded for this capture; absence
  of a report is not itself a claim of a checked, corruption-free status.

### 124.4 — Named-field struct for the pointer-motion predicate

*Migrated from 121.29's surviving residue. Ungated.*

`pointer_motion_needs_repaint_decision` (`freminal/src/gui/pointer_motion.rs:232-247`)
takes `focus_change_pending`, `chrome_interactive`, `any_pane_selecting`,
`overlay_open` and `pointer_pane_unresolved` as positional bools, plus
`pane_signals: Option<PointerMotionPaneSignals>` bundling two more. Per
`freminal-state-representation`, bool *parameters* are forbidden outright.

PR #496 flagged it in both the PR body and commit `b17c5709` and deferred it
deliberately: "a real hazard... the signature wants a named-field input
struct... it is Task 121/122's own surface and does not belong in a bug fix."

Scope: `freminal/src/gui/pointer_motion.rs` and its call sites. A
named-field input struct.

123 confirmed this carries **no expected performance effect** and it must
not be presented as one. It is a readability and safety fix, and it is the
one subtask in this document that was never gated.

**121.29's actual proposal — an unbounded suppressed-pointer fallback driven
by `Context::repaint_causes()` — is NOT migrated.** It was investigated and
rejected in Task 121: it depends on five egui internals, two of which are
present-day holes, for a measured prize of roughly 0.075% of a core. Do not
re-derive it.

### 124.5 — Decide and execute the chrome cache's fate

*Migrated from 121.34, absorbing 121.30, 121.33, 121.35, 121.36. Gated on
124.15.*

The #436 chrome cache is disabled by default since 121.32
(`chrome_cache_enabled()` in `egui_integration.rs`; `FREMINAL_CHROME_CACHE=1`
re-enables it) because it is structurally unsound: `ChromeMode::Replay`
skips *constructing* chrome widgets, and egui resolves hit-testing and click
validity against the **previous frame's** widget set, so unbuilt widgets are
uninteractable. That shipped as a tab-click and pane-border-drag regression
in 0.12.0-beta.7.

121.8 further found that `RedrawRequested` had permanently disqualified
`ChromeMode::Replay`, leaving the subsystem inert since it landed. So the
live question is not "is it beneficial" but "has it ever done anything", and
**deletion is the null hypothesis**. The deeper argument, recorded by the
maintainer during Task 123, is that caching an immediate-mode framework's
output fights egui's design intent and each workaround compounds.

Only two sound designs exist: cache the *output* while still constructing
the widgets, or delete the machinery. **Recommend deletion unless 124.15
shows a material, concentrated saving.**

If deleted, the following go with it: `ChromeCache`, `ChromeGatePredicates`,
`evaluate_chrome_gate`, the `gate_blocked_*` counters, the reverted 121.13,
121.14's chrome half, and subtasks 121.30, 121.33, 121.35 and 121.36 all
resolve as moot. 121.35's live waste is the case for urgency: while
disabled, the `Full` arm still populates the cache every frame — six vector
clones per frame to fill a cache nothing reads.

### 124.6 — Shaping-path levers

*Migrated from 121.19's surviving alternatives. Gated on 124.16.*

121.19's ASCII fast path was closed because ASCII does not imply "cannot
ligate" (`->`, `=>`, `!=` are exactly what ligature substitution targets),
so the only safe gate is `ligatures == false` — and `FontConfig::default`
sets `ligatures: true` (`freminal-common/src/config.rs:122`, pinned by a
test at `:2206`), making it dead code for default-config users.

Two surviving levers:

- a content-addressed **run-level** shaping cache keyed on
  `(face_id, ligatures, run text)`. Today's `ShapingCache`
  (`freminal/src/gui/shaping.rs:127`) is keyed by **line index**, so it
  cannot hit across a scroll, and one changed character re-shapes every run
  on the row.
- per-run allocation reduction in `build_shaped_glyphs`
  (`shaping.rs:701-802`), which builds four `Vec`s per run per cache miss.

Recon note for whoever takes 124.16: `shape_visible` already computes a
per-line content hash and reuses `Arc<ShapedLine>` on a hit
(`shaping.rs:204-211`), but returns only `Vec<Arc<ShapedLine>>` — the
hit/miss outcome is a fifth collapse point, discarded at the vertex-prep
boundary. Surfacing it is most of 124.16.

#### 124.6 findings (2026-08-25)

**Lever 1 landed on commit `9dbaaf47`. Lever 2 was not attempted and stays
unsized — do not read its absence as an oversight.**

124.16 measured and justified exactly one thing: scrolling defeats the
line-indexed cache because the index, not the content, is the key. That is
the case 124.6 fixes. Full-screen TUI redraw was already at 100% line-cache
hits before this subtask and gains nothing from it; any apparent
improvement to that workload below is reported because it was measured, not
because lever 1 caused it.

**What landed.** A content-addressed run-level cache, keyed by exact
`(FaceId, named ligature mode, run text)` equality — not a hash used as a
proxy for equality, so there is no collision risk from truncating that key
to a digest. The cache holds two generations: the current
`shape_visible` call's miss-bearing runs, and the immediately previous
miss-bearing call's runs. Rotation is driven by content, not frame count —
a call that hits on every line (the identical-redraw case) does not bear a
miss and does not rotate the generations, so the previous generation
survives quiet frames rather than being evicted by them. There is no
arbitrary capacity bound; the cache is bounded by the visible run content
of at most two miss-bearing calls.

Each entry stores an `Arc<[ShapedGlyph]>` canonical template shaped at
`col_start == 0` and unit cell width. On a hit, the current call allocates
its own output buffer, rebases the template's glyph positions to the
run's actual `col_start`. Glyph cell and cluster widths are text-derived
and already live in the canonical template; the metadata attached fresh on
every hit is `ShapedRun`-level and comes from the current run, not the
template: style, font weight, font decorations, colors, URL, blink, and
`col_start`/position. This reuses the *shaping* result, not the
*positioned* result — it intentionally does not implement lever 2's
allocation reduction in `build_shaped_glyphs`; the per-run `Vec`
allocations there are unchanged.

`ShapingCache::clear()` clears both the line-index cache and both
run-cache generations in one call, because a font rebuild can reuse a
`FaceId` for entirely different font data and stale run-cache entries keyed
on that `FaceId` would serve templates shaped against the old font. There
is no separate ligature-mode cache to clear: the ligature mode is part of
each `RunCacheKey`'s exact-equality key (`LigatureShaping`, alongside
`face_id` and run text), not a distinct cache or a distinct invalidation
event. Resetting `ShapingCacheStats` preserves the entries themselves — a
stats reset is an observability action, not a cache invalidation.

**Test treatment.** 124.16's `a_scroll_by_one_line_hits_nothing` asserted a
defect deliberately and said explicitly that 124.6 must invert it, not
delete it. It is now renamed and inverted: the same one-line scroll still
misses all 24 line-index slots (that part of the defect is real and
lever 1 does not touch the line cache), but now resolves to exactly 23 run
cache hits and 1 run cache miss, matching the 23 byte-identical rows the
scroll shifted into new slots. New differential exact-equality tests cover
plain ASCII, a nonzero `col_start`, nonunit cell width, ligature
substitution, wide (double-width) Unicode glyphs, and that per-run metadata
does not leak between two calls sharing a template — each asserts the
rebased output is bit-for-bit what a fresh shape of the same run would
produce, not merely "close enough".

**Benchmark methodology.** `cargo bench -p freminal --bench
render_loop_bench -- shaping_crossframe --save-baseline before-124-6`
before the change; `--baseline before-124-6` (same filter) after.

| Workload | Before (midpoint) | After (midpoint) | Criterion change (midpoint, CI) |
| -------- | ------------------ | ----------------- | -------------------------------- |
| Persistent half-screen change | 953.56 us | 673.56 us | -29.502% (-30.362% to -28.614%) |
| Identical full-screen redraw | 17.013 us | 15.176 us | -10.651% (-12.110% to -9.0677%) |
| Scroll by one line | 975.74 us | 198.44 us | -79.637% (-79.938% to -79.366%) |

The scroll/identical-redraw midpoint ratio — 124.16's 59x proxy for "how
much worse is the defect than the workload that already hits" — falls from
roughly 57.35x before this change to roughly 13.08x after. A substantial
residual gap is expected, not a sign the fix is incomplete: lever 1 adds a
content-addressed lookup and avoids rustybuzz shaping on run-cache hits, but
per-line segmentation into runs, the per-hit template-rebase allocation, and
the per-run output allocation in `build_shaped_glyphs` are all still paid on
every call, hit or miss. That residual is lever 2's territory, and this
subtask produced no measurement that sizes lever 2 — none should be inferred
from it.

The identical-redraw benchmark does not exercise the run cache at all (it
was and remains 100% line-cache hits, 0 run-cache hits and 0 run-cache
misses); its measured -10.651% is reported for completeness because it was
captured in the same run, not attributed to lever 1 causally.

**Why lever 2 is out of scope here, stated plainly.** 124.6 exists because
124.16 measured and justified the scrolling cross-line-index miss; that is
the evidence base this subtask acted on. 124.16 separately recorded that
the per-run allocation reduction in `build_shaped_glyphs` "has no
measurement here and should not be assumed to matter" — nothing in this
subtask's work changed that. Lever 2 remains unsized and is a separate
maintainer decision, not something to fold in silently because lever 1
landed cleanly.

**Verification.** `cargo fmt --all -- --check`, `cargo test --all`,
`cargo clippy --all-targets --all-features -- -D warnings`,
`cargo machete`, and `cargo xtask check-windows` all passed; pre-commit
hooks passed on the commit above.

### 124.7 — GPU buffer-orphaning for small payloads

*Migrated from 121.20. Ungated — Task 123 Phase 2 built the harness this
needed.*

`upload_verts` (`freminal/src/gui/renderer/gpu.rs:1653-1670`) orphans
unconditionally with no size gate; the idle `deco_verts` floor is the cursor
quad alone, `CURSOR_QUAD_FLOATS = 36` (`vertex.rs:149`) = 144 bytes.

Carry over the corrected #432 analysis: commit `c76ae8d1`'s primary fix was
CPU-side offset bookkeeping, and the orphan arrived as explicitly secondary
hardening ("Also hardens the cursor-only GPU fast path found while
investigating") — so the risk is smaller than `gpu.rs`'s own comment
implies, but the double-buffer-without-orphan counterfactual was never
isolated and the failure mode is **silent visual corruption**. That is
precisely why this waited for a pixel harness. Use it.

Note the boundary with Task 125: this subtask gates the *orphan*, it does
not change the *layout*. Do not begin a relayout here.

#### 124.7 findings (2026-08-23)

**Landed, and the prize is smaller than the subtask implied. Recorded
plainly so nobody re-derives a larger one.**

Measured on the Task 123 Phase 1 recording harness, 80x24, before the fix:

| Workload | Orphan calls / 3 frames | Orphan payload bytes |
| -------- | ----------------------- | -------------------- |
| Steady state | 6 | `144, 99840` per frame |
| Cursor-only | 3 | `144` per frame |

After gating the decoration buffer: steady 6 -> 5, cursor-only 3 -> 1.
Total GL calls over three cursor-only frames, 144 -> 142.

So the win is **one zero-byte GL call per gated upload, and no bytes at
all** — roughly 2% of a cursor-only frame's ~48 calls. Per 123.14's
correction the cost model is bandwidth, not call count, and by that measure
this subtask scores **zero**. The real prize is driver-side allocator churn
(a 144-byte store retired and reallocated every idle frame), which neither
Phase 1 nor Phase 2 can observe. It is landed on the argument that it is
nearly free and strictly less work, not on a measured improvement.

Two things narrowed the risk from what the stub assumed:

- `glBufferSubData` is **synchronous with respect to prior GL commands**, so
  skipping the orphan cannot produce a stale or torn read. The worst case is
  a pipeline stall — and `deco_vbo`'s independent double-buffer index
  already means the slot being written was last drawn from two frames ago.
- Issue #432's corruption was CPU-side offset bookkeeping (commit
  `c76ae8d1`), not an unsynchronized GPU write. The orphan was secondary
  hardening in that commit, as the stub already suspected.

Verified against Phase 2 regardless, because the failure mode would have
been silent:
`pixel_harness.rs::reusing_a_decoration_allocation_changes_no_pixels`
compares a cursor-only frame reached through allocation reuse against the
same state reached through orphaning, and requires zero differing pixels at
a channel bound of zero. This needed a new harness entry point,
`capture_after_cursor_only_frames` — `capture` draws exactly one frame into
a fresh renderer, so every upload in it orphans and it structurally cannot
see a reuse defect.

**Interaction, recorded because it looks like a loosened tolerance and is
not.** 124.9's guard asserted frame 2's upload count equals steady state.
Gating the orphan makes frame 3 exactly one call cheaper than frame 2, since
`deco_vbo` is double-buffered and frames 1 and 2 each pay a one-off sizing
orphan for their own slot. The assertion is now
`frame_two_uploads == frame_three_uploads + 1`: still exact, still
falsifiable, with the one named. The defect it guards was 30 versus 4.

The 4 KiB threshold (`SMALL_UPLOAD_ORPHAN_THRESHOLD_BYTES`) is **not a
measured optimum**. It is a bound comfortably above the decoration buffer's
idle and light-decoration sizes and far below the bulk uploads where
orphaning is unambiguously right. Only `deco_vbo` is gated; `bg_inst_vbo`,
`fg_vbo` and `img_vbo` are not re-uploaded at all on a cursor-only frame,
and their payloads are bulk.

### 124.8 — `DESIGN_DECISIONS.md` entry for the Phase 0 / Task 121 outcome

*Migrated from 121.27. Documentation only.*

Must record the direction **and** the inconvenient numbers, including that
Phase 0 weakened rather than strengthened the case for the egui rewrite, and
that Task 121 closed with four of six candidate items refuted.

Add, from Task 123: that Obligation 2's first verdict was itself wrong and
was corrected on 2026-08-21 after the maintainer caught that its two tests
had assumed away the mechanism they were testing. An entry that records only
the successful refutations would misrepresent the method.

Scope: `Documents/DESIGN_DECISIONS.md` only. No code.

### 124.9 — `sync_atlas` re-uploads glyphs a full atlas upload already covered

*Surfaced and measured by Task 123 subtask 123.8. Ungated.*

`TerminalRenderer::sync_atlas` (`freminal/src/gui/renderer/gpu.rs`) branches
on `GlyphAtlas::needs_full_reupload()`. The full-upload arm issues one
`tex_image_2d` covering the entire atlas — but never clears
`GlyphAtlas::dirty_rects`. Only the delta arm consumes them, via
`take_dirty_rects()`. Every glyph rasterised *before* a full upload
therefore stays queued, and the next frame re-uploads each one individually
with `tex_sub_image_2d`, despite the full upload having already contained
all of them.

Measured: on an 80x24 first paint, frame 2 issues **30 upload calls against
a steady-state 4**, roughly doubling that frame's total GL call count (104
versus ~52). It recurs on every event that sets `full_reupload`: atlas
growth, a font or font-size change, and `RenderState::clear_atlas`. A
first-paint and post-font-change cost, not a steady-state one, which is
exactly why eyeball profiling never caught it.

Scope of fix: one line — consume the queued rects in the full-upload arm,
e.g. `drop(atlas.take_dirty_rects());` immediately after the `tex_image_2d`
call. The rects are redundant by construction at that point.

**Verification requires inverting a test, not deleting it.**
`freminal/src/gui/renderer/headless_workloads.rs::a_full_atlas_upload_leaves_stale_dirty_rects_to_re_upload`
currently asserts the **buggy** behaviour deliberately, so the defect stays
pinned until fixed. After the fix, frame 2's upload count must be comparable
to steady state rather than several times it. Deleting the test would
discard the only regression guard for this behaviour.

### 124.10 — Per-row content epoch in `freminal-buffer`

**Foundation. Nothing consumes it yet; that is deliberate.**

> **REVISED 2026-08-23 (maintainer-approved).** The original entry put the
> epoch on `RowCacheEntry`. Recon found that unsound in two ways — see the
> recon block below, findings (c) and (d) — and the agreed resolution is to
> move the epoch to the **merge output**. The recon block is retained
> because it is the evidence for the change, but **this section, not that
> one, is what to implement.**

Scope: `freminal-buffer/src/buffer/flatten.rs`,
`freminal-buffer/src/buffer/scroll.rs`, `freminal-buffer/src/buffer/mod.rs`,
and their tests. Do not touch `freminal-terminal-emulator` or the GUI in
this subtask.

#### Why the merge output and not `RowCacheEntry`

`RowCacheEntry` is an **intermediate**. What is actually drawn is the merge
output — `chars` plus `tags` *after* URL splicing — and `MergeCache`
(`flatten.rs:210-224`) already stores exactly that, with `row_offsets[r]`
giving each window row's boundary into it.

Keying the epoch to the intermediate is what produced both soundness
findings. Refinement is applied at merge time and never written back to the
entries, so a clean row's rendered URL tags can change without its entry
changing (finding (c)); and per-row counters over an absolute-indexed cache
alias when the window slides (finding (d)).

Comparing the merge output dissolves both **by construction** rather than
working around them:

- Refinement is already applied by the time the comparison happens, so
  there is nothing to propagate and no group logic to get wrong. This also
  covers any *other* merge-time splicing, which a group-propagation fix
  would not.
- The comparison is "did the rendered content **at this screen position**
  change", which is precisely the question the consumer is asking. It never
  compares counters across a slid window, so aliasing cannot occur.

It is also **cheaper than what it replaces.** `interface.rs`'s current
`content_changed` diff is O(all visible chars) on every dirty frame; this is
O(re-merged rows only), because the reused prefix carries its stamps
forward untouched. The plan's "removes work rather than adding it" claim
holds only in this form.

#### What to build

Add a single globally-monotonic `u64` counter to `Buffer` and a
`row_epochs: Arc<Vec<u64>>` to `MergeCache`, populated on every merge path:

- **No-op path** (`reuse_available && boundary.is_none()`): return the
  cached epochs verbatim. Nothing changed, by definition.
- **Incremental path**: rows `[0, boundary)` carry their previous stamps
  unchanged — `build_reused_prefix` copies their merged bytes verbatim, so
  their content is *provably* identical, not merely assumed to be. Rows
  `[boundary, ..)` are compared against the previous merge's slice at the
  same position; equal carries the old stamp, different takes a fresh one.
- **Full-merge path**: compare against the previous merge's slice at the
  same position where one exists, otherwise assign fresh stamps.

Compare **regardless of whether the window fingerprint matches**. On an `fp`
mismatch position *r* holds a different absolute row, but the question being
answered is still "do these pixels need repainting", and identical rendered
content at a position genuinely does not. This is what makes a buffer-slide
correct without a special case.

The per-row comparison basis is the row's merged `chars` slice
(`row_offsets[r]..row_offsets[r+1]`, last row to `chars.len()`), the `tags`
sub-slice intersecting that range with `start`/`end` clamped to it — the
same clamping `build_reused_prefix` already does at the cut — and the row's
`line_width`.

`line_width` therefore **does** get captured onto `RowCacheEntry` at flatten
time (it is available in `flatten_row`, and it is genuinely part of what a
row renders as), but it is *compared* at the merge output with everything
else. `merge_rows_range` holds `cache`, so it is in reach. This is what lets
124.12 delete both `Arc::ptr_eq` terms instead of one.

Also add `Buffer::visible_row_epochs(scroll_offset, extra_rows) -> Vec<u64>`
alongside `visible_line_widths_extended` in `scroll.rs`.

#### A monotonic stamp, deliberately not a content hash

Each changed row takes a fresh, never-reused value from the `Buffer`
counter. The stamp is only a **transport token**; all the meaning lives in
the content comparison that decides whether to issue a new one.

A 64-bit content hash would additionally handle the A -> B -> A case
optimally (the monotonic stamp reports "changed" there, and repaints
needlessly). **Reject it anyway.** A hash collision is an *under-report*,
and an under-report of this signal is silent visual corruption. Monotonic
stamps can only ever over-report. Today's `Arc::ptr_eq` also only
over-reports, so that is the bar any replacement has to clear, and it is not
negotiable for a performance win.

#### The distinction that must survive

**`Row::dirty` means "was written to"; the epoch means "actually
changed".** Full-screen TUIs rewrite unchanged bytes by idiom, and that is
the workload this whole task exists to stop charging for.

Deliverable: the counter, the `MergeCache` field, the three population
paths, the `line_width` capture, the accessor, and unit tests covering: a
row rewritten with identical bytes does **not** bump; a row rewritten with
different bytes does; an SGR-only change does; a `line_width` change does; a
clean row is untouched; and a row whose *neighbour* changed such that
wrapped-URL refinement alters this row's rendered tags **does** bump — that
last one is finding (c) and is the test the `RowCacheEntry` design would
have failed.

**Only under-reporting is dangerous, so test it adversarially.** A test
suite that only checks "unchanged content does not bump" can be passed by a
stamp that never changes. Every case above needs its paired control.

The debug oracle (`debug_verify_against_oracle`, `flatten.rs:602-623`)
compares only the four merged output vectors and gives a new field **no**
coverage. Extend it to compare epochs too, or state explicitly why not —
the incremental path is a hand-maintained shortcut and the epoch is now part
of what it must get right.

Verification: `cargo test --all`;
`cargo clippy --all-targets --all-features -- -D warnings`; `cargo machete`.

Prohibitions: do NOT add consumers. Do NOT touch `Row::dirty`'s existing
semantics or call sites. Do NOT substitute a content hash for the monotonic
stamp.

#### 124.10 implementation notes (2026-08-23)

**Landed as specified in the revised section above.** Four things the entry
did not say, recorded so the next reader is not surprised by the diff.

**`MergeCache` needed a second new field, `row_line_widths`.** The entry
specifies capturing `line_width` onto `RowCacheEntry` (done) and comparing it
at the merge output (done) — but comparing needs the *previous* merge's value
and nothing stored one. `RowCacheEntry` holds only the current value, and
`Buffer::visible_line_widths_extended` reads live `Row`s. So `MergeCache`
carries a plain `Vec<LineWidth>` alongside `row_epochs`. Plain, not `Arc`: it
is never handed out.

**`visible_row_epochs` lives in `flatten.rs`, not `scroll.rs`.** The entry
says "alongside `visible_line_widths_extended` in `scroll.rs`". That neighbour
reads live `Row`s; this accessor reads `MergeCache`, whose fields — including
`MergeWindowFp` — are private to `flatten.rs`. Placing it in `scroll.rs` would
mean widening that visibility to serve one caller, which `module-cohesion`
explicitly declines. The window-bounds computation it shares with its intended
neighbour is one line.

**The accessor is `&mut self`.** When no cached merge covers the requested
window — nothing flattened yet, or an explicit `merge_cache = None`
invalidation site fired — there is no per-row answer, and the only safe one is
"every row changed", expressed by issuing every row a fresh stamp. That
advances the counter, hence `&mut`. On the snapshot path the fallback is a
safety net: `build_snapshot` calls this immediately after flattening the same
window.

**The debug oracle is a new, separate mechanism, not an extension of
`debug_verify_against_oracle`.** The entry offered "extend it, or state
explicitly why not"; this is the why-not. That oracle recomputes a full merge
*from scratch*, and epochs are history-dependent by construction, so a
from-scratch recomputation has no epoch values to compare against — it would
have to be given the previous merge, at which point it is a different check.
`debug_verify_epochs` is that check: for every row whose stamp was carried
forward, assert the row really does render identically to the previous merge.
It deliberately tests **only the carrying direction**, because only an
under-report is dangerous. Its load-bearing case is the incremental fast
path's reused prefix, where the stamp is carried on the *assumption* that
`build_reused_prefix` reproduced those rows byte-for-byte and that nothing
outside the merge moved under them.

Cost, since the entry claims this removes work rather than adding it: the
no-op path is a refcount bump on `Arc<Vec<u64>>` and compares nothing; the
incremental path walks the window once but does content comparison only for
rows at or after `boundary`; the full path compares every row. That is
O(re-merged rows), against the O(all visible chars) whole-window diff in
`interface.rs` that 124.11 deletes.

Tests: `flatten.rs::row_epoch_tests`, 11 tests. Every "does not bump" case is
paired with a "does bump" control, because a stamp that never advances passes
the negative cases trivially. The load-bearing one is
`wrapped_url_refinement_bumps_a_clean_rows_epoch` — finding (c), the test the
`RowCacheEntry`-keyed design would have failed — and it asserts its own setup
really did change the clean row's rendered URL tags before asserting the
epoch bumped.

#### 124.10 recon (2026-08-23) — BLOCKED, four corrections needed

**Do not implement this subtask as written.** Read-only recon against
`flatten.rs`, `scroll.rs`, `row.rs` and `interface.rs` found four places
where the entry above does not match the code. Two are bookkeeping; two are
soundness, and implementing the literal text would make the damage signal
**under-report**, which is a visual-corruption regression rather than a
missed optimisation. Today's `Arc::ptr_eq` can only over-report, so any
replacement must clear that bar.

**(a) `line_width` is not on `RowCacheEntry`, so the stated comparison basis
cannot be written.** The entry's six fields are `chars`, `tags`, `bytes`,
`byte_to_char`, `auto_urls`, `tail_could_be_wrapped_scheme`
(`flatten.rs:76-104`). `line_width` is a `Row` field (`row.rs:135`) that
reaches the snapshot by a wholly separate path,
`Buffer::visible_line_widths_extended` (`scroll.rs:46-53`), which reads
`r.line_width` straight off each `Row` and never consults the cache. The GUI
compares it with its own second `Arc::ptr_eq`
(`frame_dirty.rs:309-311`). Either the entry gains a `line_width` field
captured at flatten time, or `line_width` stays a separate whole-pane term.
`set_cursor_line_width` does set `row.dirty` (`cursor.rs:44-52`), so the
rebuild will fire; the question is only where the value is compared.

**(b) `content_changed` compares `chars` only — not the whole window.** The
defect table at the top of this document calls it "a true byte-level diff of
the whole window". It is `prev_chars.as_ref() != vc.as_ref()`
(`interface.rs:955-958`); `tags`, `row_offsets` and `url_tag_indices` are
stored but never compared. An SGR-only change is invisible to it. This makes
the epoch *more* informative than what it replaces, which is fine — but the
"subsumes and then deletes" framing should not be read as parity.

**(c) Cross-row URL refinement can change a clean row's rendered output.**
`redetect_urls_for_group` (`flatten.rs:1527`) recomputes URL ranges over a
whole `RowJoin::ContinueLogicalLine` group and returns them in a separate
`refined_auto_urls` vector that is **applied at merge time and never written
back into the cache entries**. So row R's rendered URL tags can change
because a *neighbouring* row changed, while R itself is clean and its own
entry is byte-identical. A per-row epoch keyed solely on R's rebuilt entry
misses it.

The cheapest sound fix is **group propagation**: if any row in a logical-line
group bumps, bump every row in that group. It is conservative, cheap, and
the group boundaries are already computed in the same loop. This needs
deciding before implementation, not during.

**(d) Per-row counters alias across a sliding window.** The entry says
epochs are "window-relative by design" and leans on `scroll_changed` and
`dims_changed` to cover the shift. They do not cover the common case: when
new output pushes rows into scrollback the visible window slides while
`scroll_offset` stays 0, so `scroll_changed` is **false**. `row_cache` is
indexed by **absolute** row (`buffer/mod.rs:126`), so window position *i*
then holds a different row's epoch — and with independent per-row counters
those two values can coincide, reporting "unchanged" for changed content.

Fix: make the epoch a **single globally-monotonic stamp** on `Buffer`, with
each changed row stamped with a fresh never-reused value. Two distinct rows
can then never share a stamp, so a slid window always reports changed
(conservative, correct) and a genuinely untouched row still reports
unchanged. Cost is one `u64` field. This still satisfies everything the
entry asks for; it is just not the literal reading of "bump the epoch".

Also note for whoever implements: `RowCacheEntry` derives only
`Debug, Clone` (`flatten.rs:75`) — the comparison needs `PartialEq` on it or
a hand-written field-wise compare. And the debug oracle
(`debug_verify_against_oracle`, `flatten.rs:602-623`) compares only the four
*merged* output vectors, never `RowCacheEntry` values, so it gives a new
entry field **no coverage**. The entry's instruction to "prove the new field
consistent with that oracle" therefore needs its own mechanism.

### 124.C2 — `sync_toast_atlas` carries the same defect as 124.9

*Surfaced by 124.9. Cleanup entry per `agent-orchestration-protocol`.*

`toast_text_pass.rs::sync_toast_atlas` (`:641-656`) is a documented
standalone mirror of `TerminalRenderer::sync_atlas` — reproduced rather than
reused because the method is private to `TerminalRenderer` and bound to its
own `atlas_texture`. It has the identical bug 124.9 fixed: the full-upload
arm never consumes `GlyphAtlas::dirty_rects`, so every glyph rasterised
before a full upload is re-uploaded individually on the next frame.

Not fixed inline with 124.9, whose stated scope is `gpu.rs`. Impact is
smaller — the toast atlas is only synced while a toast is on screen, and
123's measurement shows a toast frame already costs 121 calls against a
steady 52 — but leaving the two copies divergent is worse than either state.

**Fix by deduplication, not by fixing the bug twice** (maintainer decision,
2026-08-23). Applying 124.9's one-liner here a second time would leave
intact the thing that caused the second bug: two hand-maintained copies of
the same routine, which drifted once and would drift again. Extract a single
free function taking `(gl, texture, atlas)` — the only reason the copy exists
is that `sync_atlas` is a private method bound to `TerminalRenderer`'s own
`atlas_texture` field, and a free function parameterised on the texture
removes that reason entirely. `TerminalRenderer::sync_atlas` becomes a thin
wrapper that supplies its own texture.

Verification: the toast-present workload in `headless_workloads.rs`
(`a_toast_more_than_doubles_frame_cost`) asserts a bound the fix moves, so
extend it rather than merely keeping it passing.

### 124.11 — `row_epochs` on `TerminalSnapshot`; delete `content_changed`

Scope: `freminal-terminal-emulator/src/snapshot.rs`,
`freminal-terminal-emulator/src/interface.rs`, and their tests.

What: add `row_epochs: Arc<[u64]>` to `TerminalSnapshot`, populated in
`build_snapshot` from 124.10's accessor. Delete the `content_changed: bool`
field and the `O(visible_chars)` full-vector comparison that computes it at
`interface.rs:957`. The per-row epoch is strictly more informative and is
computed where the data is already being touched, so this subtask **removes**
a whole-window comparison rather than adding one.

`Arc<[u64]>` matches the existing convention for `prompt_rows`
(`snapshot.rs:294`) and `command_blocks` (`:306`).

Note that `scroll_changed` stays. It answers a different question and its
consumers are unaffected.

**Correction carried from 124.10's recon (finding (b)).** The wording above,
and the defect table at the top of this document, describe
`content_changed` as "a true byte-level diff of the whole window". It is
not: `interface.rs:955-958` compares `prev_chars` only, so `tags`,
`row_offsets` and `url_tag_indices` are stored and never compared, and an
SGR-only change is invisible to it. The epoch is therefore **strictly more
informative** than what it replaces, not equivalent to it. Do not read
"subsumes and then deletes" as parity — deleting `content_changed` removes a
weaker signal, which is why no consumer needs a compatibility shim.

Deliverable: the field, the population, the deletions, and the consequent
updates to every `content_changed` reader in the emulator crate and its
tests. Any GUI reader is 124.12's problem and this subtask may leave a
compile break there only if 124.11 and 124.12 land as one PR; if they land
separately, keep `content_changed` until 124.12 removes its last reader.

Verification: `cargo test --all`;
`cargo clippy --all-targets --all-features -- -D warnings`; `cargo machete`.

Prohibitions: do NOT change `frame_dirty.rs`. Do NOT change the snapshot's
other fields.

#### 124.11 implementation notes (2026-08-23)

**`row_epochs` landed; `content_changed` did not go.** This entry's own
fallback applies — *"if they land separately, keep `content_changed` until
124.12 removes its last reader"* — because every commit on this branch has to
leave `cargo test --all` passing, so a deliberate GUI compile break is not
available. The deletion moves into 124.12.

**The entry under-counts the readers.** It says "Any GUI reader is 124.12's
problem", with 124.12's scope listed as `frame_dirty.rs` plus the
`PaneRenderCache` fields in `widget.rs`. There is a second reader outside that
scope: `app_impl.rs`'s `content_wants_repaint = is_new_snapshot &&
pane_snap.content_changed`, which drives *repaint scheduling*, not vertex
rebuilds. 124.12's scope therefore has to include `app_impl.rs`, or
`content_changed` cannot be deleted at all.

Note also that `frame_dirty.rs`'s surviving `snap.content_changed` reader is
the **selection auto-clear**, which deliberately does *not* use the
`Arc::ptr_eq`-augmented signal and already backstops itself with an
`O(visible_chars)` comparison against `last_rendered_visible` (the #470 fix).
The epoch diff answers that question directly and more cheaply, so 124.12
subsumes it rather than having to preserve it.

**One hazard worth knowing before 124.12 relies on this.** `Buffer` holds
exactly one `merge_cache`, keyed to one window, and `visible_row_epochs` falls
back to re-stamping every row when it does not match. Any *other* caller of
`Buffer::visible_as_tchars_and_tags*` therefore evicts it. Audited: the only
non-test, non-bench caller is `TerminalHandler::search_corpus` (Ctrl-F, via
`gui/pty.rs`), which is occasional and costs one spurious full repaint.
`TerminalHandler::data_and_format_data_for_gui` also calls it but has **no
production callers at all** — benches only. Recorded rather than fixed: it is
a Task 31 dead-code question, not this task's.

Tests: seven in `snapshot_build.rs`. The one that matters is
`a_change_survives_the_snapshots_the_gui_never_renders`, which asserts *both*
halves of the contrast in a single test — that `content_changed` is `true` on
the snapshot right after a change and has gone stale (`false`) three
unrendered snapshots later, while the epoch still differs from the pre-change
baseline. Verified empirically, not assumed: the stale-bool half reproduces.
That test is the justification for the whole type change and must not be
reduced to its epoch half.

### 124.12 — GUI consumes epochs; delete the `Arc::ptr_eq` content test

**This is where 124.1's confirmed defect is actually fixed.**

Scope: `freminal/src/gui/terminal/frame_dirty.rs` and the `PaneRenderCache`
fields it reads in `freminal/src/gui/terminal/widget.rs:1300-1308`.

What: `PaneRenderCache` stores the last-rendered `row_epochs`.
`evaluate_frame_dirty_state` derives content change by diffing the snapshot's
epoch vector against it, producing a **changed-row set** rather than a bool.
Delete the `!Arc::ptr_eq(last_rendered_visible, snap.visible_chars)` and
`!Arc::ptr_eq(last_rendered_line_widths, snap.visible_line_widths)` terms at
`frame_dirty.rs:301-311`.

`theme_changed`, `dims_changed` and `folds_changed` keep forcing a
whole-pane change and are unaffected — they are genuinely global.

**Both** `Arc::ptr_eq` terms go, not just the `visible_chars` one. That is a
consequence of 124.10's revision: `line_width` is captured onto
`RowCacheEntry` and folded into the merge-output comparison, so the epoch
vector already covers a double-width/double-height change and the separate
`visible_line_widths` pointer test has nothing left to catch.
`snap.visible_line_widths` itself stays — the renderer still needs it as
*data* — it just stops being a change-detection input.

The changed-row set is the input 124.14 consumes. In this subtask it may
still collapse to a bool at the `VertexRebuild` boundary; the point of
landing it separately is that the epoch chain's correctness is provable on
its own, before the damage-model change is layered on it.

`freminal-state-representation` applies: the changed-row set is a named
type, not a bare `Vec<usize>` passed positionally.

**Invert, do not delete,**
`frame_dirty.rs::byte_identical_reflatten_in_a_new_arc_still_forces_a_full_rebuild`
— Task 123 wrote it to pin the defect deliberately. After this subtask a
byte-identical re-flatten in a fresh `Arc` must report **no** content change,
and its paired control (re-observing the same `Arc` reports no change) must
still pass, so the new assertion is not the degenerate "nothing ever
changes".

Also revisit the doc comment at `frame_dirty.rs:262-281`, which explains why
`snap.content_changed` was distrusted. That explanation is now history and
should say so rather than describing a field that no longer exists.

Deliverable: the epoch diff, the deletions, the inverted test, a test that a
single changed row is reported as exactly that one row, and a before/after
capture per `performance-benchmarks` and `freminal-bench-table`. Measure on
**bandwidth**, per 123's correction; a typing workload should move off the
"Full redraw, steady" row of 123's table toward the cursor-only row, and that
migration is this subtask's success criterion.

Verification: `cargo test --all`;
`cargo clippy --all-targets --all-features -- -D warnings`; `cargo machete`;
the Phase 1 recording harness; `cargo xtask check-windows` before the PR.

Prohibitions: do NOT add `PaneFrameDamage::Region` here — that is 124.14. Do
NOT touch `decide_frame_damage`.

#### 124.12 implementation notes (2026-08-23)

**Landed in two commits, 124.12a and 124.12b.** 124.12a built the epoch diff
and switched the rebuild decision onto it; 124.12b deleted
`TerminalSnapshot::content_changed` and its last three readers. The split is
not cosmetic: 124.11's entry required `content_changed` to survive until its
last reader went, because every commit on this branch must leave
`cargo test --all` passing, and 124.12a's scope (`frame_dirty.rs` plus
`widget.rs`) does not reach the third reader.

**The entry's scope was short by one file, exactly as 124.11's notes
predicted.** `app_impl.rs`'s repaint scheduler held
`content_wants_repaint = is_new_snapshot && pane_snap.content_changed`, so
the deletion could not land without it. `observe_visible_snapshot` became
`observe_row_epochs` (an `Arc::ptr_eq` on `visible_chars` becoming a value
comparison on `row_epochs`), and the `&& pane_snap.content_changed` conjunct
went with it.

**That conjunct's removal is an equivalence, and it was checked case by case
rather than asserted.** Its stated purpose (issue #439 fix #4) was to
suppress the case where `is_new_snapshot` was `true` merely because
`flatten_visible` had allocated a fresh `Arc` holding identical bytes. The
epoch-based `is_new_snapshot` does not report that case at all, so the
conjunct has nothing left to veto. Enumerated:

| Case | Old `is_new && content_changed` | New `is_new` |
| ---- | ------------------------------- | ------------ |
| Nothing changed (clean path, same `Arc`) | false | false |
| Real content change | true | true |
| Byte-identical re-flatten, fresh `Arc` | false (the conjunct's job) | false (the epoch's job) |
| SGR-only change | **false** | **true** |
| Alt-screen round trip, identical content | false | **true** |

The two divergences both go the safe way — an extra scheduled repaint, never
a lost one — and the SGR-only row is arguably a fix: `content_changed`
compared `chars` only (124.10 recon finding (b)), so an SGR-only change was
invisible to it, and the frame still rebuilt via the pointer test but armed
no follow-up repaint. There is no case where the old expression was `true`
and the new one is `false`; that direction would require the epoch to
under-report, which is the failure mode 124.10 was designed and tested
against.

**`DeferredChangeFlags` lost its content half and now defers `scroll` only.**
`resolve_for_frame` drops two parameters and returns a `bool` instead of a
`(bool, bool)` tuple. The scroll arm is unchanged line for line. This is not
a regression of issue #490's content case: the content case was fixed by
deleting the bool that needed deferring. A monotonic per-row stamp compared
against a baseline the *consumer* holds cannot be lost by a frame the
consumer skipped, so there is nothing to accumulate. The GUI half of that
argument is structural and was verified rather than assumed —
`evaluate_frame_dirty_state` and the whole rebuild path sit inside
`if !snap.skip_draw` (`widget.rs:2479`), so `last_rendered_row_epochs` only
ever advances on a frame that was actually drawn.

**Five tests in `snapshot_build.rs` were deleted, not rewritten.** Each was
checked individually against the 124.11 epoch test claimed to subsume it,
because a deletion justified by a subsumption that does not hold is lost
coverage:

| Deleted | Subsumed by | Verdict |
| ------- | ----------- | ------- |
| `first_snapshot_reports_content_changed_true` | `the_first_snapshot_stamps_every_row_distinctly` | Covered, with a caveat below |
| `second_snapshot_with_no_new_data_reports_content_changed_false` | `a_snapshot_with_no_new_data_carries_identical_row_epochs` | Stronger (all rows, not one bool) |
| `new_data_after_snapshot_causes_content_changed_true` | `new_pty_data_bumps_only_the_affected_rows_epoch` | Strictly stronger |
| `cursor_only_move_does_not_set_content_changed` | `a_cursor_only_move_bumps_no_row_epoch` | Same test, epoch-valued |
| `alt_screen_enter_invalidates_cache` | `entering_the_alternate_screen_changes_row_epochs` | Same test, epoch-valued |

The caveat on the first row: "the first snapshot is a change" has no
emulator-side analogue, because with no baseline there is nothing to
compare. Its epoch-side content — that the first snapshot stamps every row
freshly rather than leaving zeros — is what the replacement asserts, and the
consumer-side half ("no baseline means treat everything as changed") is
pinned separately by `diff_row_epochs`'s `no_recorded_epochs_reports_every_row_changed`
and by `observe_row_epochs_reports_new_only_on_genuine_change`'s first
assertion.

**`a_change_survives_the_snapshots_the_gui_never_renders` kept only its epoch
half, and that is a real loss, recorded rather than papered over.** The test
asserted a two-sided contrast: the bool goes stale by snapshot E, the epoch
does not. The bool half is not restatable once the field is gone. What
survives still pins something falsifiable — that four snapshots after a
change, the changed row's stamp still differs from the pre-change baseline,
which fails immediately if anything re-introduces edge-triggered semantics —
but it no longer demonstrates *why* the epoch exists. The comment in the
test says so explicitly rather than leaving a reader to assume the weaker
assertion was always the point.

**The #490 tests in `interface_tests.rs` were re-pointed at a
consumer-held baseline.** Each now captures `settled.row_epochs` before the
synchronized block and asserts the post-`?2026l` snapshot differs from *that*,
which is the shape the GUI actually uses. Three tests beyond the two the
brief named were touched for the same reason
(`..._does_not_imply_scroll_change`, `..._without_change_reports_none`,
`..._survives_timeout_resume`); the scroll-deferral test itself is unchanged
except for a stale doc line.

#### 124.12 measurement debt — a Phase 1 capture is not feasible, and why

The Verification section requires a before/after capture for 124.12 measured
on bandwidth. **It cannot be taken on the Task 123 Phase 1 harness, and this
is a property of the harness rather than an excuse.** `headless.rs` drives
`HeadlessRenderer::draw_frame` / `draw_cursor_only` directly from a
`SyntheticFrame` description. It never constructs a `TerminalSnapshot`, never
calls `evaluate_frame_dirty_state`, and the choice between the full-rebuild
and cursor-only paths is made *by the test*, not by the damage decision.
124.12 changes nothing about what either path costs; it changes **which path
a given frame takes**. The harness is blind to exactly that variable.

The capture is therefore composed from two measurements that do exist, both
already pinned by tests:

- **The path change**, by `frame_dirty.rs::byte_identical_reflatten_in_a_new_arc_is_no_longer_a_content_change`,
  which asserts `VertexRebuild::CursorOnly` on a frame that previously took
  the full rebuild, with `a_changed_row_epoch_is_reported_as_exactly_that_row`
  as the control proving the diff is still change-sensitive.
- **The per-path cost**, by 123.14's per-workload table: ~200,000 bytes and
  52 calls for "Full redraw, steady" against ~576 bytes and ~48 calls for
  "Cursor-only, steady" at 80x24 — roughly 350x the bytes for roughly 1.08x
  the calls.

So the win, stated in the units 123 mandated, is **~199,400 bytes per
migrated frame**, on the frames where the emulator re-flattens and the
rendered content is unchanged.

**A correction to 123.14's own framing, since it named the criterion.** 123
wrote that "typing should move to something nearer the cursor-only row, and
that migration is the measurable success criterion for 124.1". Typing does
**not** migrate and must not: a keystroke genuinely changes a row's rendered
content, its epoch bumps, and a full rebuild is the correct answer. What
migrates is the byte-identical re-flatten — a full-screen TUI rewriting
unchanged bytes by idiom, or any site that sets `Row::dirty` without changing
content. That is the workload this task's opening paragraph names, and it is
the one 123's own Obligation 1 test was built from.

**What an end-to-end capture would need, recorded so 124.14 inherits a
decision rather than re-deriving it.** A harness that builds a real
`TerminalSnapshot` (via `TerminalEmulator::new_headless`), drives
`evaluate_frame_dirty_state` against a `PaneRenderCache`, and feeds the
resulting `VertexRebuild` into the headless renderer to select the draw path.
That is a new harness — effectively a Phase 3 — and building it inside a
subtask whose scope is `frame_dirty.rs` plus three readers would be scope
creep. 124.14 carries the same capture requirement and the same obstacle;
it should either commission that harness as its own subtask or compose its
number the same way and say so.

### 124.13 — Re-measure pointer-motion suppression rates

**Measurement only. Changes no behaviour.**

Scope: the pointer-motion instrumentation counters and whatever harness or
scratch build is needed to read them. No production behaviour change.

What: re-take the 2026-07-29 suppression table. Those numbers predate the
chrome cache being disabled (121.32) and Task 123 could not refresh them,
because pointer motion is not a renderer workload and the Phase 1 harness
drives the renderer directly.

Report, per scenario, the check count, the suppression rate, and which veto
fired: a clean pane, a pane with one OSC 8 hyperlink on screen, a
mouse-tracking application, an active selection drag, and a pane with
scrollback offset non-zero. The 2026-07-29 finding this is testing is that a
single hyperlink took suppression from 99.16% to 1.68% via the pane-wide
`has_urls` veto — roughly 20x — which was the leading candidate for the
maintainer's mouse-movement symptom.

Per `PROFILING.md`, report frame rate and per-frame cost **as a pair**, and
state the observed pointer event rate alongside any CPU figure. 123 declined
to guess an event rate and this subtask should not guess either — measure it
or say it was not measured.

Deliverable: a findings block appended to this document, feeding 124.3.

Prohibitions: do NOT change any suppression logic. Do NOT implement 124.3.

#### 124.13 findings (2026-08-23)

**The 2026-07-29 table's headline is confirmed, and the mechanism turns out
to be structural rather than statistical — which makes 124.3's case stronger
than a percentage would.**

Measured by driving the existing pure chain (`resolve_pane_under_pointer` ->
`PointerMotionInputs` -> `pointer_motion_needs_repaint_decision`) over a
deterministic 64x40 lattice of pointer positions across a window whose
central terminal area is 1200x700 at (40, 60). Pinned as tests in
`gui::pointer_motion::suppression_rates` so these numbers cannot go stale
unnoticed the way the 2026-07-29 table did.

| Scenario | Suppression | Veto that fired |
| -------- | ----------- | --------------- |
| Clean pane | 100.00% | none |
| One OSC 8 hyperlink | 17.97% | `has_urls` (pane-wide) |
| Scrollback offset > 0 | 17.97% | `scroll_offset_nonzero` (pane-wide) |
| Mouse-tracking application | 17.97% | `mouse_tracking_active` (pane-wide) |
| Active selection drag | 0.00% | `any_pane_selecting` (window-wide) |
| Gutter active, blocks present | 98.63% | `gutter_active` (**positional**) |

**Read the 17.97% correctly.** It is *not* a residual benefit — it is
exactly the 460 of 2560 lattice positions that fall **outside** the pane and
so resolve to no pane at all. **For motion inside the pane, suppression is
exactly 0%.** The precise figure therefore depends on the lattice's geometry
and should not be quoted as a session number; the structural fact behind it
does not depend on anything and should be.

That is the whole point. `has_urls` and `scroll_offset > 0` are
**position-independent** vetoes. `pane_hover_region_risk` approximates
"motion might enter or leave a URL span" as "motion anywhere in this pane",
because the precise cell-to-hyperlink hit test is render-pass-only state.
No amount of knowing where the pointer actually is can rescue a check once
either is set — which is why the 2026-07-29 session measurement saw
99.16% collapse to 1.68% from a single hyperlink, and why re-measuring it
produces a cliff rather than a curve.

**The gutter is the counter-example, and the template.** It is the one veto
that already carries a positional term (the Task 121 fix), and it costs
98.63% -> only the strip, not the pane. 124.3 proposes giving the other two
the same shape. This table is the before-picture for that.

**Not a target for 124.3:** `mouse_tracking_active`. It defeats suppression
for the same pane-wide reason, but correctly — an application receiving
mouse reports must be sent every motion event. `any_pane_selecting` is worth
noting separately: it is window-level with no positional term at all, so it
suppresses *nothing anywhere*, including over chrome.

**Not measured, stated rather than guessed:** the pointer **event rate**. A
suppression rate is a fraction of checks; converting it to a CPU figure
needs an events-per-second the compositor determines and no harness here can
observe. Task 123 declined to guess it and so does this. Per
`PROFILING.md`, any CPU claim derived from these numbers must carry the rate
it assumed.

**Interaction with 124.2, carried forward for 124.3.** `FrameDamage::None`
will make an unsuppressed motion frame nearly free at the *present* layer,
but it still costs a whole GUI frame walk. 124.15 measured that walk's
chrome portion at 43.2 us of construct-plus-tessellate. Suppression remains
worth having; its prize is now that figure rather than a full present.

### 124.14 — `PaneFrameDamage::Region` and `VertexRebuild::Bounded`

**The larger half of the damage-model fix.**

Scope: `freminal/src/gui/renderer/mod.rs` (the `PaneFrameDamage` enum),
`freminal/src/gui/terminal/frame_dirty.rs` (the `VertexRebuild` enum),
`freminal/src/gui/terminal/widget.rs` (the rebuild match and damage
production), `freminal/src/gui/frame_damage.rs` (aggregation).

What: today `PaneFrameDamage` is `{Full, CursorOnly(Option<CursorDamage>),
Unchanged}` and `VertexRebuild` is `{CursorOnly, ReevaluateFullRebuild}`. The
cursor got a bounded-damage special case and nothing else did, so a single
keystroke, a selection extension, a hover change and a search-highlight
change are all forced to `Full` because the type cannot express "these
cells".

Add `PaneFrameDamage::Region(Vec<DamageRect>)` and a `VertexRebuild` variant
carrying 124.12's changed-row set.

> **CORRECTION (124.21, 2026-08-24).** This entry previously said the
> selection, hover and search extents "are already known at
> `widget.rs:2585-2596` as the booleans that currently force `Full`". That
> citation is **wrong and was already wrong when written**. That line range
> is the *cursor-only* damage assignment (`CursorDamage::from_cursor_cells`
> -> `PaneFrameDamage::CursorOnly`) and contains **zero** references to
> selection, hover or search. Those flags live in `frame_dirty.rs`, which
> Task 122's cleanup entry 122.C3 split out on 2026-08-03 — three weeks
> before this task was activated.
>
> The claim was also **overstated**. Of the three, only selection and hover
> have a real extent at the decision point (`screen_selection`,
> `command_block_hover_rows_early`). `search_changed` is a **hash**
> (`search_epoch`), not a location — bounding it needs new work, not
> plumbing. See 124.21's census.

`decide_frame_damage` aggregates `Region` rects exactly as it already
aggregates `CursorOnly(Some(rect))`.

**Scope boundary, stated because it will be tempting to cross it:** this
subtask bounds the **present**, not the **upload**. The vertex rebuild stays
a full rebuild and `upload_verts` stays a whole-buffer write, because the
instance buffers have no fixed per-row stride. Bounding the upload is
Task 125 and requires a vertex format relayout. Do not start one here.

> **CORRECTION (124.14a recon, 2026-08-24).** This paragraph previously
> read: *"That still wins, because the existing cursor-only path already
> proves the mechanism: `widget.rs:3038-3067` scissors the draw on the
> authoritative `present_is_partial` flag, and `egui_integration` restricts
> the EGL present. Extending that from one cursor rect to an arbitrary
> row-range rect set is the change."*
>
> Three things in that were wrong. `present_is_partial` no longer exists —
> 124.18 replaced it with `freminal_windowing::PresentRegion`
> (`freminal-windowing/src/lib.rs:99-111`). The line range is stale; the
> scissor is now at `widget.rs:3082-3116`. And, most importantly, **the
> mechanism is not already proven for the path a `Region` frame would
> take** — see the recon block below. "Extending that ... is the change"
> understates the work by the whole of the full-draw arm.

The correctness argument that must be verified, not assumed: on a `Region`
frame every pixel outside the rects must be byte-identical to the previous
frame. Redrawing unchanged rows produces identical pixels — 123's Phase 2
measured exactly that, zero differing pixels at bound zero — but the
**untouched-background constraint** at the top of this document applies with
full force, and only the pixel harness can catch a violation.

Deliverable: both variants, the rect computation, the aggregation, unit
tests for the new aggregation cases, and a pixel-harness test that a
`Region` frame and a `Full` frame of the same state produce identical
pixels. Plus a before/after capture per `performance-benchmarks`.

Verification: `cargo test --all`;
`cargo clippy --all-targets --all-features -- -D warnings`; `cargo machete`;
both Task 123 harnesses; `cargo xtask check-windows` before the PR.

Prohibitions: do NOT change `upload_verts` or the vertex emission format. Do
NOT remove `CursorOnly` — it is a legitimate special case with a stable
patch offset and deleting it is a separate decision.

#### 124.14a activation recon (2026-08-24) — READ-ONLY, changed no code

Commissioned because this entry has already been caught once citing a line
range that was wrong when written, so its remaining citations were not
trusted. Every claim below was re-derived from the source and then
independently spot-checked against the file. **124.C5, the hard gate on this
subtask, closed first** (commits `1339208c`, `74e94576`).

**The blocking finding: the full-draw paint arm applies no scissor at all.**
The paint callback splits on `is_cursor_only` (`widget.rs:3027`). The
`CursorOnly` arm reads `PresentRegion` and scissors to it
(`widget.rs:3082-3116`) — deliberately to the *windowing-published* region
rather than to the pane's own cursor rect, because on a stale back buffer
the published region is a union covering more than this frame's damage
(comment at `widget.rs:3058-3071`). The `else` arm
(`widget.rs:3117-3147`) calls `draw_with_verts` and **scissors nothing**.

That matters because a `Region` frame takes the **full vertex rebuild**, by
this entry's own scope boundary, and therefore lands in the unscissored arm.
Combined with 124.20's now-scissored clear the result is a genuine
silent-corruption hazard rather than merely wasted work: outside the region
the clear does not run but the draw does, so semi-transparent content blends
against whatever an unrelated earlier frame left in the back buffer
(`a*fill + (1-a)*stale`). Those pixels are not presented this frame, but
124.18's damage-history union means a later frame may treat that buffer as
valid. This is the issue #432 class, and it is exactly what 124.20's
**one-region-for-clip-clear-and-present** invariant exists to prevent.

**So 124.14a's first obligation is to extend that invariant to the full-draw
arm**, not to add enum variants. The enum work is the easy half.

**What is already in place** (verified, with citations):

| Fact | Where |
| ---- | ----- |
| `PaneFrameDamage` is `{Full, CursorOnly(Option<CursorDamage>), Unchanged}` | `renderer/mod.rs:107-116` |
| `VertexRebuild` is `{CursorOnly, ReevaluateFullRebuild}`, one construction site | `frame_dirty.rs:87-95`, `:715-720` |
| `ChangedRows` is `{None, Rows(Vec<usize>), All}`, already computed and tested | `frame_dirty.rs:118-133` |
| `CursorDamage` and `DamageRect` are the same shape; conversion is a field copy | `renderer/mod.rs:129-139`, `windowing/lib.rs:73-82`, `frame_damage.rs:110-116` |
| Both are **physical pixels, bottom-left origin** (the `glScissor` convention) | `renderer/mod.rs:118-122` |
| `from_cursor_cells` consumes **top-left**-origin viewport-relative cells and does the Y-flip itself | `renderer/mod.rs:157-220`, flip at `:212` |
| `decide_frame_damage` aggregates `CursorOnly(Some)` rects, and `bell` / `CursorOnly(None)` / `Full` each do `rects.clear(); break;` | `frame_damage.rs:90-130` |

**Three coordinate spaces are in play and they are not the same.**
`ChangedRows::Rows` holds **snapshot/flattened-window** row indices
(`snapshot.rs:152-171`). `screen_selection` is also snapshot-row space
(`frame_dirty.rs:552-585`). But `command_block_hover_rows_early` is
**rendered-row** space, post-fold-collapse (`widget.rs:316`, `:328-337`), and
`cache.previous_selection` is **buffer-absolute** (`widget.rs:1329`). Going
from a snapshot row to pixels needs `row_map.snapshot_to_rendered` then
`layout.rendered_to_screen`, which is the two-step the cursor path already
does (`frame_dirty.rs:603-605`). **124.14b must not assume its two extents
share a space with `changed_rows`;** that is a live bug waiting to be
written, not a hypothetical.

**`vp_left_px`, `vp_top_px` and `fb_height_px` are match-arm-local**
(`widget.rs:2545-2553`) — computed inside the `CursorOnly` arm only, so they
are **not** in scope in the `ReevaluateFullRebuild` arm and must be hoisted
rather than assumed available. `cell_w_f` / `row_h_f` (`widget.rs:1928-1929`)
and `terminal_rect` (`:2019`) are in scope for both.

**The `#[expect(dead_code)]` removal is load-bearing, not cleanup.** The
attribute (`frame_dirty.rs:211-217`) is `cfg_attr(not(test), expect(...))`.
`expect` was chosen over `allow` precisely so that it fails the build the
moment a real reader appears: once 124.14a adds a non-test reader,
`dead_code` stops firing, the expectation becomes unfulfilled, and
`unfulfilled_lint_expectations` errors under `-D warnings`. Leaving it in
place is a build failure, not a warning.

**No test asserts "a row change forces `Full`", so nothing needs inverting
here.** Every test in both files was read.
`a_changed_row_epoch_is_reported_as_exactly_that_row`
(`frame_dirty.rs:999-1018`) asserts `changed_rows` but deliberately does not
assert `outcome.rebuild`. The seven `*_beats_cursor_change` tests pin that a
trigger *vetoes* the cursor-only fast path, which stays true. This is
recorded because the absence is a finding: a subtask that expects to invert a
pin and cannot find one usually has the wrong model, and here it does not.

**Image damage is untouched by all of this, as 124.C5 requires.**
`image_frame_changed` and `image_pixels_changed` remain whole-pane booleans
with no per-row extent, gating only the full-rebuild-vs-reuse `if` at
`widget.rs:2601-2612`. Nothing in the current code folds them into
`changed_rows`, and 124.14a must not start.

**One stale citation left uncorrected, deliberately.** 124.17's evidence
table cites `widget.rs:3092-3100` and the retired `present_is_partial` name.
That block is a historical findings record of what was believed at the time;
rewriting its citations would falsify the record. Noted here instead.

### 124.15 — Measure chrome's per-frame cost

**Measurement only. Changes no behaviour. Gates 124.5.**

Scope: a test or bench using the Task 123 Phase 1 recording harness and, if
useful, the Phase 2 pixel harness.

What: quantify what constructing and painting freminal's chrome costs per
frame — the menu bar, tab strip, pane borders and the `CentralPanel` fill —
separated from the terminal band. 123's per-workload table has a "Toast
present" row at 121 calls against a steady frame's 52, but no chrome-only
figure.

This exists because of 123's recorded ordering caution: if the chrome cache
is deleted **and** `FrameDamage::None` lands, chrome becomes the thing
forcing `Full` on frames the grid would otherwise skip, and without a
baseline that trade is invisible.

Report also whether `ChromeMode::Replay` is reachable at all today, given
121.8's finding that `RedrawRequested` had permanently disqualified it. If it
is not reachable, say so plainly — that is the strongest possible input to
124.5 and it should not be buried.

Deliverable: a findings block appended to this document.

Prohibitions: do NOT delete or re-enable the chrome cache. That is 124.5.

#### 124.15 findings (2026-08-23)

**The headline reframes 124.5 rather than answering it on cost.**

**Is `ChromeMode::Replay` reachable today?** Mechanically yes; in a shipped
build, no. 121.8's fix (commit `7d483998`) genuinely worked — it removed the
`RedrawRequested` self-disqualification and took the idle `Replay` duty
cycle from 0/360 frames to 100%, chrome construction 69 us -> 10 us, total
frame 434 us -> 376 us (-13.4%). 121.32 (commit `c3daa1be`) then turned the
whole subsystem off **again**, behind `chrome_cache_enabled()`, which reads
`FREMINAL_CHROME_CACHE` once via a `OnceLock` and defaults false. Nothing in
the repo — no config key, CLI flag, `flake.nix` or CI job — ever sets that
variable.

So the honest answer to the subtask's question is sharper than "unreachable":
**the cache is disabled for correctness, not for cost.** `ChromeMode::Replay`
skips *constructing* chrome widgets, and egui resolves hit-testing against
the previous pass's widget set, so unbuilt widgets are uninteractable — that
shipped as a tab-click and pane-border-drag regression in 0.12.0-beta.7.

**This means no cost measurement can authorise re-enabling it.** A faster
build that drops tab clicks is not an option at any price. Measurement can
only inform a different question: is the ceiling high enough to justify a
*redesign* — caching the tessellated output while still constructing the
widgets?

**The numbers** (`freminal/benches/chrome_cost_bench.rs`, headless
`egui::Context`, 1280x800, 4 tabs, 3 pane borders):

| Measurement | Per frame |
| ----------- | --------- |
| Chrome construction (what `Replay` skips) | 32.9 us |
| Chrome tessellation | 10.3 us |
| Cache shape-vector clone | 0.65 us |
| Cache primitive-vector clone | 0.64 us |

Reported per frame with no implied rate, per `PROFILING.md`. Against 121.8's
recorded 434 us total frame, chrome construction is roughly 8%.

**121.35's live-waste claim is verified and is small.** With the cache
disabled, `chrome_mode` is forced to `Full`, so that arm runs every frame and
populates a `ChromeCache` no reachable code reads. Four of its six clones
disappear with the cache (the two `to_vec()` slice copies feeding
`tessellate` do not — it takes an owned `Vec`), so deletion returns roughly
**2.6 us per frame, about 0.6% of a frame.** Real, and worth having, but not
an argument on its own.

**Recommendation to 124.5: delete.** Not because the cache is slow — the
ceiling is a genuine 8% — but because the shipped design cannot be made
correct without becoming a different design, deletion returns 0.6% and about
600 lines immediately, and the 8% stays available to anyone who later wants
to build the sound version (cache the output, keep constructing the widgets).
Keeping ~600 lines of disabled, structurally-unsound machinery to preserve
optionality on a redesign that shares none of its code is the worst of both.

**Ordering still holds.** Per the caution carried from 123, chrome becomes
the thing forcing `Full` on frames the grid would otherwise skip once
`FrameDamage::None` (124.2) lands. These numbers are that baseline: 43.2 us
of construct-plus-tessellate is what a `None` frame would still have to pay
if chrome is not itself gated.

### 124.16 — Shaping cache instrumentation and a TUI-redraw benchmark

**Measurement infrastructure. Gates 124.6.**

Scope: `freminal/src/gui/shaping.rs` and the benches directory.

What: two things 124.6 was gated on and Task 123 did not build.

First, surface per-line shaping cache hit/miss. `shape_visible`
(`shaping.rs:161`) already computes a per-line content hash and reuses
`Arc<ShapedLine>` on a hit (`:204-211`), but returns only
`Vec<Arc<ShapedLine>>`, discarding the outcome. Surface it as counters.

Second, a benchmark modelling full-screen TUI redraw — the workload that
rewrites unchanged bytes every tick and is the reason this whole task
exists. Per `freminal-bench-table`, place it with the existing shaping
benches.

Report the hit rate for: steady typing, a full-screen redraw of identical
content, a scroll by one line (which today cannot hit at all, because the
cache is keyed by **line index**), and a single-character edit (which today
re-shapes every run on the row).

Deliverable: the counters, the benchmark, and a findings block feeding
124.6.

Prohibitions: do NOT re-key the shaping cache. Do NOT implement either of
124.6's levers.

#### 124.16 findings (2026-08-23)

**One lever is strongly supported, and the workload this task is named for
turns out not to be a shaping problem at all.**

`ShapingCacheStats` now surfaces the per-line hit/miss outcome
`shape_visible` was computing and discarding. The counters are always
compiled, not feature-gated, so a measurement cannot drift from the code
path it describes.

**Hit rates** (80x24, pinned as tests in
`gui::shaping::shaping_cache_hit_rate` so they cannot rot the way the
2026-07-29 pointer-motion table did):

| Workload | Hit rate | Rows re-shaped |
| -------- | -------- | -------------- |
| Identical full-screen redraw | 100% | 0 of 24 |
| Single-character edit | 96% | 1 of 24 |
| Steady typing | 96% | 1 per keystroke |
| **Scroll by one line** | **0%** | **24 of 24** |

**Cost** (200x50, `shaping_crossframe` group):

| Benchmark | Time |
| --------- | ---- |
| `shape_visible_identical_redraw` | 46.8 us |
| `shape_visible_scroll_by_one_line` | 2.76 ms |
| `shape_visible_persistent_fm_cache` (half the rows changed) | 2.86 ms |

**A one-line scroll costs 59x an identical redraw, and 96% of what
rewriting half the screen costs** — despite 23 of its 24 rows being
byte-identical to rows shaped on the immediately preceding frame. The cache
is keyed by **line index**, so scrolling shifts every line into a different
slot and invalidates all of them. That is the measured case for 124.6's
first lever (a content-addressed run-level cache keyed on
`(face_id, ligatures, run text)`), and it is a strong one.

`a_scroll_by_one_line_hits_nothing` asserts this **defect** deliberately, in
the same idiom Task 123 used for 124.9 and 124.1. **124.6 must invert it,
not delete it** — it is the only regression guard the behaviour has.

**The inconvenient half, recorded because it cuts against 124.6's framing.**
This document's opening argues that "full-screen TUIs rewrite unchanged
bytes by idiom, which is precisely the workload that currently pays a full
rebuild every tick". True at the *damage* layer — that is what the epoch
chain fixes — but **not at the shaping layer**, which already handles it at
a 100% hit rate for 46.8 us. Shaping is not that workload's bottleneck, and
124.6 must not be justified on it. The scroll case, not the redraw case, is
124.6's argument.

**124.6's second lever is untouched by this and stays unsized.** "One
changed character re-shapes every run on the row" is confirmed as
*behaviour* by the single-character-edit test (1 row, not 1 run), but the
per-run allocation reduction in `build_shaped_glyphs` has no measurement
here and should not be assumed to matter.

### 124.C1 — `decide_frame_damage`'s doc comment describes a removed term

*Surfaced by Task 123's 123.8 while discharging Obligation 2. Cleanup entry
per `agent-orchestration-protocol`.*

`decide_frame_damage`'s doc comment (`freminal/src/gui/frame_damage.rs`, the
`force_full` description around lines 54-56) still describes a
`pointer_moving` term removed by issue #459 item 9. It states precisely the
hypothesis Obligation 2 was testing, so a reader checking the docs would get
a **false confirmation** of a refuted diagnosis.

Scope: the doc comment only. Documentation change; no test needed beyond the
standard suite. May be folded into 124.2, which edits the same function.

### 124.17 — Does the skip-clear + partial-present path ever actually fire?

**Measurement only. Changes no behaviour. Gates 124.14 and 124.2.**

*Added 2026-08-23 during 124.14's activation recon, which hit a hard stop.*

#### Why this exists

124.14's entry asserts that extending bounded damage from the cursor rect to
a row-range rect set is safe because "the existing cursor-only path already
proves the mechanism". **Recon could not verify that, and reading the code
produces a contradiction.** Each link below is confirmed in the source:

| Fact | Where |
| ---- | ----- |
| On a `Partial` frame the GL clear is skipped **entirely**, not scissored | `egui_integration.rs:1205` |
| The head pass is "chrome painted before the terminal band — e.g. the `CentralPanel` background fill" | `egui_integration.rs:1046-1058` |
| Head, band and tail are painted **unconditionally**, every frame, `Partial` or not | `egui_integration.rs:1258-1262` |
| `panel_fill` is the palette background at `bg_opacity` alpha | `chrome_style.rs:214` |
| `bg_opacity` defaults to **1.0**, i.e. fully opaque | `config.rs:359` |
| The pane's scissor is applied **only** when `present_is_partial` — exactly when the clear was skipped | `widget.rs:3092-3100` |

Composed, a `CursorOnly` partial frame at default config should: skip the
clear, then paint an **opaque** `panel_fill` over the whole central area
(erasing the previous frame's terminal content), then redraw the grid
**clipped to the cursor rect**. Everything outside that rect would be left
flat background. The screen would blank on every cursor blink.

That is not observed. So either the model above is wrong somewhere not
visible in the source, or **the partial path is effectively never taken**.
Nobody has measured which, and the existing counters cannot tell us —
`frame_stats.frame_damage_full` / `frame_damage_partial`
(`app_impl.rs:3858-3868`) count the **app's request**, i.e.
`win.pending_frame_damage`. The windowing layer then applies a *second*,
entirely separate gate (`supports_partial_present() && buffer_age() == 1`)
that decides whether the clear is actually skipped. 121.31's
`frame_damage_partial=0` is an app-side number and says nothing about it.

`buffer_age() == 1` is the suspicious term. It means "the back buffer holds
the **immediately previous** frame". On a conventionally double-buffered
surface the buffer about to be drawn into holds the frame from *two* frames
ago, i.e. age 2, so the gate would never pass. Whether that is what happens
here is unknown and is the single most valuable thing this subtask reports.

#### Scope

`freminal-windowing/src/egui_integration.rs` and
`freminal-windowing/src/gl_context.rs` (one log-message correction only).
No other crate.

#### What to build for 124.17

Extract the gate at `egui_integration.rs:1195-1203` into a pure, named,
unit-testable function and attribute every frame to exactly one outcome.
Two new types, per `state-representation` — no bool parameters, no bare
bools:

```rust
/// Whether the surface can present a damaged sub-region at all. A static
/// per-surface capability, probed once at surface creation.
pub(crate) enum PartialPresentSupport { Supported, Unsupported }

/// Why a frame did or did not take the skip-clear + partial-present path.
/// Exactly one variant per frame, so the derived counters sum to the frame
/// count.
pub(crate) enum PartialPresentDecision {
    /// The app reported `FrameDamage::Full`.
    NotRequested,
    /// The app reported `Partial` with an empty rect list.
    RequestedWithNoRects,
    /// The app reported `Partial`, but the surface cannot present a
    /// sub-region (damage extension absent, non-EGL backend, Apple).
    BlockedBySurface,
    /// The app reported `Partial` and the surface supports it, but the
    /// back buffer does not hold the previous frame's contents.
    BlockedByBufferAge { age: u32 },
    /// Taken: the clear is skipped and only the damaged rects present.
    Taken,
}
```

The decision function takes the buffer age **lazily**:

```rust
fn decide_partial_present(
    frame_damage: &crate::FrameDamage,
    support: PartialPresentSupport,
    buffer_age: impl FnOnce() -> u32,
) -> PartialPresentDecision
```

Laziness is not a style preference: today's `&&` chain short-circuits, so
`buffer_age()` — an EGL query — is never issued on a `Full` frame. Taking it
eagerly would add a per-frame driver round trip to every frame in the
program, which is a behaviour change inside the very path being measured.

#### Counters

On the existing `FrameProfile`, feature-gated `frame-profiling`, mirroring
the established `gate_blocked_*` pattern and flushed on the existing
windowing `frame_profiling` tracing line:

- `present_partial_not_requested`
- `present_partial_no_rects`
- `present_partial_blocked_surface`
- `present_partial_blocked_buffer_age`
- `present_partial_taken`
- `buffer_age_histogram: [u64; 4]`, bucketed by `min(age, 3)` so index 3
  means "3 or more".

The histogram is sampled **only where the age was actually queried** (app
requested `Partial` *and* the surface supports it), so its total equals
`present_partial_blocked_buffer_age + present_partial_taken` and **not** the
frame count. Say so in the field's doc comment; a histogram that silently
covers a different denominator than its neighbours is how a measurement
becomes a lie.

#### One honesty fix, in scope

`gl_context.rs:301-305` logs, at `info`, that "cursor-only frames **will**
skip the full clear and present only the changed region". That is precisely
the unverified premise, stated to the user as fact, and it is what a reader
checking the logs would take as confirmation. Amend it to report the
capability only — the extension is present and the fast path is *available*
— not that it will be used. Do not touch the `else` arm or the Apple arm.

#### Deliverable

The two types, the extracted function, the counters, the log correction, and
unit tests: one per decision variant; one proving the age closure is **not**
called when the app reported `Full` (pass a closure that panics); and one
proving `Taken` requires all three conditions together.

Then **run it** and append a findings block reporting, from a real
interactive session with `--features frame-profiling`: the five counts, the
age histogram, the present-path line from startup, and — if
`present_partial_taken > 0` — whether any visual corruption is observable.
A live Wayland/X session is available, so this is measurable here rather
than something to hand over.

#### The three outcomes, and what each means for 124.14 and 124.2

- **`present_partial_taken == 0`.** The subsystem is inert, exactly as the
  chrome cache was before 121.8 found it. 124.14's prize is zero until
  whatever blocks it is fixed, and 124.2's `FrameDamage::None` — which also
  spends this path — needs re-deriving. Both entries get rewritten, not
  implemented.
- **`present_partial_taken > 0` and the display is correct.** The model
  above is wrong somewhere; find where before building on it. The most
  likely candidates, in order: the head pass does not in fact cover the
  band's pixels; `panel_fill` is not opaque in the composited result; or
  `buffer_age()` semantics differ from the doc comment.
- **`present_partial_taken > 0` and corruption is observable.** A shipped
  bug, filed and fixed before 124.14 adds a second consumer of the path.

#### Prohibitions

Do NOT change what `partial` evaluates to — for every input the computed
value must be bit-identical to today's expression; this subtask is
observation only. Do NOT touch the clear, the paint order, the scissor,
`decide_frame_damage`, `PaneFrameDamage`, or anything in the `freminal`
crate. Do NOT implement 124.14 or 124.2. Do NOT "fix" anything the counters
reveal — report it.

#### 124.17 findings — GPU re-take (2026-08-23), AUTHORITATIVE

**On real hardware the partial-present path is never taken. Not once in
8,160 frames across three runs.** This inverts the llvmpipe result below on
its single most important term, and it resolves — rather than deepens — the
contradiction that blocked 124.14.

Measured on this workstation, Hyprland/Wayland, **AMD GPU, no llvmpipe**
(`llvmpipe_threads=0` asserted at the start of every run), release build with
`--features frame-profiling`, continuous PTY output plus pointer motion over
the grid, 1264x680:

| Counter | Run 1 | Run 2 | Run 3 |
| ------- | ----- | ----- | ----- |
| `frame_counter` | 3120 | 2520 | 2520 |
| `present_partial_not_requested` | 3100 | 2503 | 2501 |
| `present_partial_no_rects` | 0 | 0 | 0 |
| `present_partial_blocked_surface` | 0 | 0 | 0 |
| `present_partial_blocked_buffer_age` | **20** | **17** | **19** |
| `present_partial_taken` | **0** | **0** | **0** |
| `buffer_age_histogram` `[0,1,2,3+]` | `[0,0,20,0]` | `[0,0,17,0]` | `[0,0,19,0]` |

Two things changed against the llvmpipe run, and they are the same thing:

- **`buffer_age()` returns 2, on every single query, in every run.** Never 1.
- **Therefore `buffer_age() == 1` blocks 100% of requested partial frames**,
  and `present_partial_taken` is exactly zero.

**The original run's central refutation was a software-rendering artefact.**
This subtask's premise recorded a suspicion in as many words: *"On a
conventionally double-buffered surface the buffer about to be drawn into
holds the frame from two frames ago, i.e. age 2, so the gate would never
pass."* The llvmpipe session reported age 1 on all 44 queries and recorded
that suspicion as **REFUTED**. On the GPU the suspicion is **CONFIRMED** —
the surface is conventionally double-buffered and behaves exactly as
predicted. llvmpipe's swapchain is not the hardware's.

**This resolves the blocking question, and the answer is the reassuring
one.** 124.14's recon predicted that a taken partial present *should* visibly
corrupt the display (clear skipped, opaque `panel_fill` painted over the
central area by the head pass, band redrawn scissored to the cursor rect) and
observed that it manifestly does not. Three candidate explanations were
offered; two were eliminated, leaving the head-pass premise in doubt. The
GPU numbers supply a fourth and much simpler explanation that the llvmpipe
data had excluded:

> **The display does not corrupt because the partial path never runs.**

The recon model was most likely correct all along. It was never exercised on
real hardware, so it was never falsified by one.

**Consequence: this is outcome 1 of the three this subtask enumerated.** Per
that enumeration — *"`present_partial_taken == 0`. The subsystem is inert,
exactly as the chrome cache was before 121.8 found it. 124.14's prize is zero
until whatever blocks it is fixed, and 124.2's `FrameDamage::None` — which
also spends this path — needs re-deriving. Both entries get rewritten, not
implemented."*

That is now the operative instruction for both. Specifically:

- **124.14's prize on this hardware is currently zero.** Converting frames
  from `Full` to `Region` converts them into *requests* that the
  `buffer_age() == 1` gate then rejects. The vertex-rebuild savings survive;
  the present savings do not.
- **124.2 is affected differently and less.** `FrameDamage::None` skips the
  clear, the paint and the swap *in the app*, upstream of this gate, so its
  saving does not depend on the partial-present path. Its `buffer_age()`
  interaction — already flagged in its entry as "the correctness crux" —
  now has a concrete answer: age is 2 in steady state, so a subsequent
  `Partial` frame is declined and falls back to a full clear, which is the
  safe direction.
- **The `buffer_age() == 1` condition itself is now the highest-value open
  question in this task.** Whether it is correct to require age 1, or whether
  the damage-rect set should be unioned across `buffer_age()` frames (the
  conventional `EGL_EXT_buffer_age` idiom, and what the extension exists
  for), is a real design question that 124.17 was not scoped to answer.
  Requiring age 1 on a double-buffered surface is equivalent to disabling
  the feature.

**Do not re-derive the llvmpipe numbers as a cross-check.** They describe
Mesa's software swapchain and have no bearing on shipped behaviour. They are
retained below only because the contrast is what identified the defect.

#### 124.17 findings (2026-08-23) — SUPERSEDED, llvmpipe only

> **INVALIDATED 2026-08-23 by 123.C2.** The session below was run from the
> `default` dev shell, which at the time exported `LIBGL_ALWAYS_SOFTWARE=1`
> (see `PLAN_123_GL_MEASUREMENT_HARNESS.md`, 123.C2). It therefore measured
> **Mesa llvmpipe's** EGL implementation, not the GPU's — and this subtask's
> two central results are `supports_partial_present()` and `buffer_age()`,
> which are precisely the properties most likely to differ between a
> software surface and a real one. The numbers below are retained as the
> llvmpipe data point; they must not be used to unblock 124.14 or 124.2.
> The GPU re-take above is authoritative.

**The path fires, it fires constantly, and the windowing-side gate is wide
open. The bottleneck is entirely app-side.** That is the opposite of the
hypothesis this subtask was written to test, and it makes 124.14 and 124.2
*more* valuable and *more* dangerous at the same time.

Measured on this workstation — Hyprland/Wayland, Mesa, EGL — over a 60 s
interactive session with continuous PTY output, `--features
frame-profiling`, 2280 drawn frames:

| Counter | Value | Share |
| ------- | ----- | ----- |
| `frame_counter` | 2280 | — |
| `present_partial_not_requested` (app said `Full`) | 2236 | 98.1% |
| `present_partial_no_rects` | 0 | 0% |
| `present_partial_blocked_surface` | 0 | 0% |
| `present_partial_blocked_buffer_age` | 0 | 0% |
| `present_partial_taken` | 44 | 1.9% |
| `buffer_age_histogram` (`[age0, age1, age2, age3+]`) | `[0, 44, 0, 0]` | — |

Read the two zeros in the middle of that table, because they are the
finding. **Neither windowing-side condition has ever blocked a single
frame.** `supports_partial_present()` is `true` (startup logs the
damage-aware present path), and `buffer_age()` returned **1 on every one of
the 44 occasions it was queried** — never 0, never 2. The
double-buffering suspicion recorded in this subtask's premise is
**REFUTED**: on this surface the back buffer does hold the immediately
previous frame.

So of the three conditions gating the skip-clear + partial-present path,
two are permanently satisfied and the third — the app deciding `Partial` —
fails 98.1% of the time. **Every frame 124.14 or 124.2 converts from `Full`
to bounded damage converts directly into a taken partial present.** There
is no second gate to absorb it.

The 44 are also spread evenly across the session (1–3 per 120-frame flush
window, from the first window to the last), not clustered at startup. This
is live, steady-state behaviour, not a warm-up artefact.

**What this does NOT settle, and what is now the blocking question.**
124.14's recon (recorded above) predicted that a taken partial present
should visibly corrupt the display: the clear is skipped, the head pass then
paints an opaque `CentralPanel` `panel_fill` over the whole central area,
and the band pass redraws the grid *scissored to the cursor rect*, which
should leave everything outside that rect flat background. At 1–3 partial
frames per two seconds that would be an unmissable strobe. It does not
happen. **So the recon model is wrong somewhere, and 124.14 must not be
built until it is known where** — its safety argument depends on the real
mechanism, not on the current code happening to work.

Two of the three candidate explanations offered by the recon are now
eliminated:

- **`buffer_age()` semantics differ from the doc comment** — refuted by the
  histogram above.
- **egui renders into an intermediate FBO, so the default framebuffer's
  contents and the clear are not what the recon assumed** — refuted by
  reading `egui_glow` 0.36.1: `Painter::intermediate_fbo` returns `None`
  unconditionally, with the source comment "We don't currently ever render
  to an offscreen buffer".

That leaves the head-pass premise: either the `CentralPanel` fill is not
painted, not opaque, or does not cover the band's pixels on these frames.
Resolving it needs an observation below egui and above the renderer, which
is precisely the gap between Task 123's two harnesses — Phase 1 sees no
pixels, Phase 2 never runs egui.

**A screenshot test was attempted and is reported as inconclusive rather
than as evidence.** Forty full-screen `grim` captures at 4 Hz over an idle
session showed no blanked terminal band — but an idle session drew fewer
than 120 frames in 40 s (under 3 fps, the idle optimisation working as
designed), so the flush threshold was never reached and there is no counter
proving any of those frames took the partial path. At a 1.9% rate the
sample may simply have missed every one. It is recorded so nobody repeats
it expecting a result.

**Recommendation.** 124.14 and 124.2 both stay blocked, but the reason has
changed: not "the path may be inert" (it is not) but "the path is live,
about to be used far more heavily, and its correctness under a skipped
clear is unexplained". The cheapest honest next step is a Phase 3 app-level
capture — one that runs egui's head/band/tail split against a real
framebuffer and reads back pixels — which is the same instrument 124.14's
own deliverable already requires for its "`Region` frame and `Full` frame of
the same state produce identical pixels" test. Building it once serves
124.2, 124.14 and this question together.

### 124.18 — Make partial present actually work

**Blocks 124.14 and 124.2. Both spend this path; today it is inert, and the
one time it was forced open by hand it corrupted the display.**

*Added 2026-08-23, after 124.17's GPU re-take. This is squarely in this
task's remit — "Task 124 stops doing work that produces no pixels; it is the
frame-count and present win" — because without it there is no present win at
all, on any machine.*

#### The two defects, and why neither can land alone

**(a) The gate demands a swapchain depth almost nothing has.**
`decide_partial_present` takes the path only when `buffer_age() == 1`.
`EGL_EXT_buffer_age` returns *how many frames ago the back buffer's contents
were defined*: `0` means undefined (glutin documents this — "you must redraw
the entire buffer"), and `n` means the contents are those of the buffer `n`
frames ago. A conventionally double-buffered surface therefore reports **2**
in steady state, and 124.17's GPU re-take measured exactly that on every one
of ~250 queries across 21 flush windows — never 1, never 3+. llvmpipe reports
1, which is why the pre-123.C2 measurement saw the path firing constantly and
concluded the gate was wide open.

The extension exists precisely so a buffer that is `n` frames stale can be
reused: keep a short damage history and redraw the union of the last `n`
frames' damage. Requiring `age == 1` accepts only the degenerate case.
`gl_context.rs`'s doc comment — "values `> 1` mean the buffer holds an older
frame and is likewise unsafe to treat as 'last frame'" — is true as stated
but draws the wrong conclusion: stale is not unusable, it is *repairable*.

**(b) A partial frame still paints unclipped chrome over the whole surface.**
On a taken partial present the clear is skipped, but head, band and tail are
each painted unconditionally (`egui_integration.rs`, the three
`paint_primitives` calls). The **head slice contains the `CentralPanel`
background fill**, which is opaque at the default `bg_opacity = 1.0` and
covers the entire central area. Only the band is clipped, and only because
the app's paint callback sets its own scissor to the cursor rect. Everything
else is erased.

**(b) is confirmed empirically, by the maintainer, 2026-08-23.** Forcing the
`Taken` branch at `age == 2` corrupted the display: the surface cleared and
stayed blank until an event forced a `Full` frame (notably on focus loss,
recovering on pointer re-entry), with possible flicker during use. That
symptom is diagnostic — a missing damage union (defect (a)'s other half)
would produce *small* artifacts such as cursor trails or stale glyph
fragments, not a blank surface. A blank surface is the unclipped opaque fill.

This also closes 124.14's long-open contradiction. The recon model was
right; it was simply never exercised, because on real hardware the gate
never opened. Both prior candidate explanations stay refuted, and the
head-pass premise is now **CONFIRMED** rather than merely surviving.

**Neither defect may be fixed alone.** Repairing the gate without the
clipping turns an inert subsystem into visible corruption on most frames —
which is precisely the experiment that was just run. Repairing the clipping
without the gate changes nothing observable, because the path still never
fires.

#### The design

The contract between the layers becomes:

- **app -> windowing:** "these rects changed this frame" (`FrameDamage`).
- **windowing -> app:** "redraw at least this region" — the damage union with
  swapchain staleness already applied.

Windowing owns the staleness arithmetic because windowing owns the
swapchain. This is what keeps 124.14 simple: it emits rects and never learns
what `buffer_age` is, carries no damage history, and reasons about no
swapchain depth.

1. **Damage history.** `EguiState` retains the damage rects of the last few
   presented frames. On `buffer_age() == n`, the region that must be redrawn
   is the union of the current frame's damage with the previous `n - 1`
   frames'. `age == 0`, or `age` deeper than the retained history, falls back
   to `Full` — unchanged, and still the always-correct path.

2. **Clip everything, not just the band.** A partial present means *only
   these pixels may change*, so every primitive must obey it. Each
   `egui::ClippedPrimitive` carries its own `clip_rect`; intersect all three
   slices' clip rects with the redraw region before painting. Verified
   against `egui_glow` 0.36.1: `set_clip_rect` clamps `max` to `>= min`, so a
   fully-clipped primitive scissors to zero area and draws nothing. The
   opaque `CentralPanel` fill then cannot escape the region.

3. **Hand the region to the band callback.** The band is a
   `Primitive::Callback` and the app's callback sets its own scissor,
   overriding egui's. So the published `present_is_partial` `AtomicBool` must
   become a published *region*: the callback scissors to what windowing says
   must be redrawn, rather than to its own cursor rect. This deletes the
   current arrangement where band clipping is correct by coincidence.

4. **Present the same region** via `swap_buffers_with_damage`. Whatever was
   redrawn is what is declared damaged; the two must not diverge.

**A bounding box is sufficient for v1, and is not a shortcut.** Redrawing
more than the union is always safe — 123's Phase 2 proved redundant redraw of
unchanged state yields zero differing pixels at a channel bound of zero — and
one rect is exactly what the app's draw path already supports. Scattered
multi-row damage collapsing to a large bbox is a *measurable performance*
question, not a correctness one. Multi-rect scissoring, with one draw pass
per rect, is a later optimisation to be justified by measurement.

#### The verification problem, stated plainly

**The Task 123 Phase 2 pixel harness cannot regression-test the shipped
path.** It runs on llvmpipe, which reports `age == 1`, so it structurally
cannot reproduce the `age == 2` case this subtask exists to fix. The
offscreen pbuffer surface reports `0`.

What each instrument can still do:

- **Phase 2 / llvmpipe** *can* verify defect (b): it is the one environment
  where a partial present is actually taken today, so the clipping fix is
  testable there, and the untouched-background property (`DefaultBackground`
  means "leave these pixels alone") can be pinned exactly as 123 pinned it.
- **Unit tests** cover the staleness arithmetic completely — the union over
  `n` frames of history is pure and needs no GL at all. Cover `age` of 0, 1,
  2, history-depth, and deeper-than-history.
- **Hardware observation plus the 124.17 counters** are the only check on the
  age-2 path end to end. After the fix, `present_partial_taken` should become
  the large majority of `Partial` requests and
  `present_partial_blocked_buffer_age` should fall to near zero.

That residual risk is real and belongs to a silent-corruption class of bug
(issue #432's class). **Recorded so the choice is explicit: if that is judged
insufficient, the defensible alternative is to leave partial present off.**
It has never once worked in a shipped build, so disabling it deliberately
costs nothing that is currently being had, and would be an honest outcome
rather than a defeat.

#### Scope for 124.18

`freminal-windowing/src/egui_integration.rs` (the gate, the history, the clip
intersection), `freminal-windowing/src/gl_context.rs` (the `buffer_age` doc
comment, which currently asserts the wrong conclusion),
`freminal-windowing/src/lib.rs` (the published-region type replacing the
`AtomicBool`), and the app-side callback in
`freminal/src/gui/terminal/widget.rs` that consumes it.

Per `state-representation`, the published region is a named type, not a bare
`Option<Vec<DamageRect>>` threaded positionally.

#### Prohibitions for 124.18

Do NOT change `decide_frame_damage` or `PaneFrameDamage` — that is 124.14 and
124.2. Do NOT begin multi-rect scissoring. Do NOT touch the vertex layout.
Do NOT weaken the `age == 0` fallback.

### 124.C3 — `merge_cache` has no per-buffer stash, so alt-screen round trips over-report

*Surfaced by 124.12. Cleanup entry per `agent-orchestration-protocol`.
**Not** in scope for 124.14 — see the boundary below.*

`Buffer` holds exactly one `merge_cache`, keyed to one window. Anything that
replaces `row_cache` wholesale clears it, and `visible_row_epochs` then takes
its "no cached merge covers this window" fallback and issues every row a
fresh stamp. Two sites reach that today:

- **Alt-screen round trips.** `enter_alternate`
  (`resize_and_alt.rs:1215`) and `leave_alternate` (`:1255`) both set
  `merge_cache = None` unconditionally, and `SavedPrimary` (`:220`) has no
  `merge_cache` field to stash one in. So `row_epochs` re-stamps every row on
  every round trip **even when the restored content is byte-identical and
  `visible_chars` is provably reused**. This partially undoes issue #405's
  "alt/primary one-frame tax" optimisation, which fixed exactly this shape
  for the emulator's own `previous_visible_snap` by giving it a per-buffer
  stash.
- **`TerminalHandler::search_corpus`** (Ctrl-F, via `gui/pty.rs`), already
  recorded in 124.11's notes: any other caller of
  `Buffer::visible_as_tchars_and_tags*` evicts the frame path's cache.

Confirmed empirically, not assumed:
`interface.rs::build_snapshot_return_to_primary_unchanged_content_reuses_visible_chars_arc`
now asserts both halves — the `visible_chars` `Arc` **is** reused and the
`row_epochs` **do** re-stamp — so the over-report is pinned as known
behaviour rather than latent.

**Cost is one full rebuild per round trip**, on a user-initiated and
infrequent event (entering or leaving `vim`, `less`, a TUI). It is an
over-report, which is the safe direction: the bar 124.10 set is that a
replacement may only ever over-report, and this clears it.

**Why this is a separate subtask and not a rider on 124.14.** The fix lives
in `freminal-buffer`'s alt-screen save/restore, which is a different crate
and a different concept from 124.14's GUI damage model, and it is not a
one-liner: stashing a `merge_cache` per buffer means the restored cache must
be provably valid against the restored `row_cache` and the current window, and
getting that wrong is an **under**-report, i.e. silent visual corruption. It
needs its own tests, including the adversarial pairing 124.10 used. Folding it
into 124.14 would put a correctness-critical buffer change inside a subtask
whose verification is aimed at the renderer.

Scope when taken: `freminal-buffer/src/buffer/resize_and_alt.rs`
(`SavedPrimary`, `enter_alternate`, `leave_alternate`) and
`freminal-buffer/src/buffer/flatten.rs`. Deliverable: the per-buffer stash,
plus a test that an alt-screen round trip over byte-identical primary content
carries **unchanged** row epochs, paired with a control proving a round trip
over genuinely changed content still re-stamps. The three
`interface.rs` tests that currently assert the over-report must be inverted,
not deleted.

**Undecided and left to the maintainer:** whether this is worth doing at all.
One repaint per TUI entry and exit is close to unmeasurable, and the fix
carries real corruption risk. Closing this entry unexecuted is a legitimate
outcome; it is recorded so the behaviour is known rather than rediscovered as
a bug.

**Disposition (2026-08-25): closed unexecuted, maintainer decision.** The
cost is one full rebuild per user-initiated alt-screen round trip (entering
or leaving `vim`, `less`, a TUI) — near-unmeasurable in practice. The fix
requires a per-buffer `merge_cache` stash, and an incorrect stash is an
**under**-report, i.e. silent visual corruption, not merely a missed
optimisation. The known behaviour today — re-stamping every row on a round
trip — is safe over-reporting, the direction 124.10 established as
tolerable. Risk exceeds benefit, so this entry is closed unexecuted.
Reopening requires new evidence that the benefit justifies the risk.

---

## Verification

Standard for every subtask, per `agents.md`:

1. `cargo test --all`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo machete`
4. `cargo xtask check-windows` before any PR, per `freminal-windows-crosscheck`

Every performance subtask (124.2, 124.3, 124.5, 124.6, 124.7, 124.12,
124.14) additionally requires a before/after capture per
`performance-benchmarks` and the freminal-specific catalog in
`freminal-bench-table`. Per 123's correction, capture and justify on
**bandwidth**, not call count.

Every subtask that changes what is drawn or presented (124.2, 124.7, 124.14)
must additionally run the Task 123 Phase 2 pixel harness, because the
untouched-background property is invisible to call-count tests.

124.4, 124.8, 124.9 and 124.C1 are documentation, readability or one-line
fixes and carry no performance-capture requirement — though 124.9 has its own
mandatory test inversion.

---

## References

- `Documents/PLAN_123_GL_MEASUREMENT_HARNESS.md` — the measurement harness
  this task depends on, and the Findings section (123.14) that re-scoped it.
- `Documents/PLAN_125_VERTEX_RELAYOUT.md` — the vertex format relayout this
  task deliberately stops short of, gated on 124's measurements.
- `Documents/PLAN_121_PERF_REMEDIATION.md` — the closed umbrella this
  document migrates surviving work out of; carries the CONFIRMED/REFUTED
  verdicts and dated corrections referenced throughout.
- `Documents/DECOUPLING_FRAMEWORK.md` — the decision record for whether
  freminal should stop using egui for the main window; §2A is the source of
  truth for the Phase 0 measurements 124.8 records.
- `Documents/PROFILING.md` — profiling methodology, including the
  frame-rate-plus-per-frame-cost reporting discipline.
- Issue #405 — the earlier idle-CPU investigation Task 121 pivoted from.
- Issue #432 — the silent visual corruption bug class 124.7 and 124.2 share.
- Issue #435, issue #436 — partial present and chrome caching, both closed;
  relevant to 124.2/124.14 and 124.5 respectively.
- Issue #440 — the missing pixel harness, closed by Task 123 Phase 2.
- Issue #457 — `merge_cache` structural shift, still open, deprioritized by
  Task 121's 121.1.
- Issue #459 — the profiling findings and the candidate list Task 121's
  Group D drained.

### 124.19 — Phase 3: an egui-level offscreen pixel harness

**Complete (2026-08-24). Two commits: 124.19a `58ff02b9`, 124.19b `aff02068`.**

*Added because 124.18's verification plan rested on a false premise.*

124.18 recorded that "Phase 2 / llvmpipe *can* verify defect (b)". It cannot,
for two independent reasons, and the residual risk the maintainer was asked
to accept was therefore materially larger than what was described:

- `pixel_harness.rs` drives `HeadlessRenderer` against an offscreen pbuffer
  and **never constructs an `egui::Context`**. Defect (b) is about the
  `CentralPanel` fill in the head slice, which that harness never paints.
- `EguiState::run_frame` had exactly one caller — `event_loop.rs`, inside
  the live event loop — so nothing could drive the paint path from a test
  at all.

The llvmpipe `age == 1` observation came from an *interactive* freminal run
in a pre-123.C2 shell, not from the pbuffer harness, which reports age `0`.
124.18's own text says so two lines earlier; the two statements contradict.

**124.19a** extracted `frame_paint::paint_frame` — run_ui, the
partial-present decision, the conditional clear, the head/band/tail
tessellate-and-paint, texture housekeeping — leaving `run_frame` as the
window-bound shell. Pure refactor, zero behaviour change.

**124.19b** built the harness on it: a `FrameSurface` over an offscreen
pbuffer with a **caller-chosen buffer age**, which is the load-bearing part.
It can put the surface in the `age == 1` state a real double-buffered
window never reports. Painting successive frames into one pbuffer with no
intervening swap *is* "the back buffer holds the previous frame", so this
models the shipped situation rather than faking it.

It runs in the existing `gl-pixel` CI job. **No new CI job and no
`flake.nix` change were needed** — `OffscreenGl` reaches EGL directly and
needs no winit, so there is no once-per-process `EventLoop` limit.

Measured, first automated reproduction of the defect: 512/4096 pixels
differed, decomposing exactly into the 256px marker (erased — the defect)
plus 256px of legitimately-new damage content. After 124.18: 256/4096.

### 124.20 — Scissor the clear to the redraw region, don't skip it

**Complete (2026-08-24), commit `7edfb480`.**

*Surfaced during review of 124.18. A third defect neither 124.17 nor 124.18
identified.*

124.18 kept 124.17's behaviour of skipping the GL clear **entirely** on a
`Taken` frame. That is wrong *inside* the redraw region. `DefaultBackground`
cells deliberately emit no quad, and a `background_opacity < 1.0` chrome
fill is semi-transparent — both need the clear colour underneath. With the
clear skipped they blend against whatever an unrelated previous frame left:
`a*fill + (1-a)*stale` rather than `a*fill + (1-a)*clear`.

Confirmed empirically before fixing. Two points inside the **same** damage
region, receiving the **same** paint, differed purely by what preceded them:

| Sample | Pixel |
| ------ | ----- |
| Over a stale opaque marker | `[100, 100, 0, 255]` |
| Over cleared background | `[0, 100, 0, 128]` |

Exact blend arithmetic for `[200,0,0,255]` under `[0,200,0,128]` at alpha
0.5, so stale content was provably bleeding through.

**Bound on the defect, recorded so it is not overstated:** at the default
`background_opacity == 1.0` the fill is opaque and covers the redraw region
completely, so stale pixels are fully overwritten and nothing is visible.
This bites transparency (Task 34) and anything else painting non-opaquely
over the band.

Neither extreme is right — a full clear on a `Taken` frame erases content
outside the region (the bug 124.18 fixed); no clear leaves the region
blending against stale pixels. The clear is now confined to exactly the
region already used for clipping and for the present. **One region for all
three** is the invariant that stops them diverging.

Note none of the four pre-existing harness tests shifted. That is
informative rather than reassuring: they paint fully opaque content across
the whole clipped region, so a skipped clear and a scissored clear are
indistinguishable to them. That is precisely why none caught this.

### 124.C4 — A pixel golden must not be compared across rasterisers

**Complete (2026-08-24), commit `26847dc0`.** *Cleanup entry.*

`golden_round_trips_for_a_reference_frame` failed on any machine with a real
GPU (9648/80000 pixels against a Radeon 7900 XTX), and since the pre-commit
hook runs `cargo test --all-features`, **it blocked every commit on such a
machine.**

Fallout from 123.C2, and older than the symptom: before it, the `default`
shell exported `LIBGL_ALWAYS_SOFTWARE=1`, so every local run was llvmpipe
and matched the golden by accident. Confining that variable to the
`gl-pixel` shell was correct, and left this test comparing an llvmpipe
golden against whatever GPU the developer has.

The test already *detected* the mismatch, printed "this is a Mesa change,
NOT a regression", and then failed anyway. That was the bug. **Not a widened
tolerance** — the comparison is still exact at zero differing pixels; the
guard is on whether the comparison means anything. Under `FREMINAL_REQUIRE_GL`
(the pinned-llvmpipe `gl-pixel` job) a mismatch still fails loudly, because
there it means the golden was regenerated under the wrong rasteriser.

### What can actually produce a `Partial` frame today

Recorded because it is the honest measure of what 124.18 bought, and the
direct case for 124.14.

A pane reports `CursorOnly` only if **all** of these hold
(`frame_dirty.rs`):

```text
!content_changed && !selection_changed && !text_blink_changed
  && !search_changed && !hover_changed && !image_frame_changed
  && !image_pixels_changed && cursor_state_changed && !deco_verts.is_empty()
```

| Produces `Partial` | Forces `Full` |
| ------------------ | ------------- |
| Cursor blink toggle | Text selection / highlighting |
| Cursor move | Any content change (typing, output) |
| Cursor show/hide | URL hover underline |
| Cursor colour override | Search highlight |
| Cursor trail animation | Blinking text; image frame/pixel changes |

Plus window-level vetoes that each force `Full`: `ui_overlay_open`,
`shader_recomposites`, `active_pane_changed`, `pointer_forces_full_present`,
`toast_active`, `bell_active`, and `ChromeDamage::Changed`.

**And a multi-pane killer:** `decide_frame_damage` walks every pane, and a
single pane reporting `Full` does `rects.clear(); break;`. In a split with
anything running in the other pane, partial present never fires at all.

So 124.18 + 124.20 delivered the **mechanism**, not yet the **benefit**. Its
only consumer today is a blinking cursor in an otherwise idle single pane.
124.14 is what makes it pay.

### 124.21 — Exhaustive audit of every full-repaint-forcing trigger

**Complete (2026-08-24). READ-ONLY audit; changed no code.**

*Commissioned because a four-item summary of the full-forcing surface was
offered and was badly incomplete. The maintainer's requirement is 100%
coverage: a missed path that 124.14 bounds anyway is silent visual
corruption, and a path left unbounded is exactly the wasted work this task
exists to eliminate.*

**The surface is 52 distinct triggers, not four.** Only **eight** are
genuinely global. Everything else is bounded work currently being done at
full-surface cost.

#### Genuinely GLOBAL — these must always force a full repaint

| Trigger | Why no bounded region suffices |
| ------- | ------------------------------ |
| `theme_changed` | colours are baked per-vertex; every drawn cell is stale |
| `dims_changed` | a width change reflows and re-shapes every row |
| `ChangedRows::All` | no epoch baseline exists to diff against |
| `deco_verts.is_empty()` | no previous rebuild ever populated the buffer |
| `CursorDamage` `None` | cursor rect degenerated after clamping |
| `shader_recomposites` | a post-process shader rewrites the whole framebuffer |
| `style_changed`, `size_changed`, `ppp_changed` | chrome-layer equivalents |
| `buffer_age() == 0`, `Unsupported` | contents unknown / no capability |

#### The rest, by how much work bounding them needs

- **BOUNDABLE-NOW** (extent already computed at the decision point, just
  discarded): `rows_changed` (`changed_rows`), `selection_changed`
  (`screen_selection` + `cache.previous_selection`), `hover_changed`
  (`command_block_hover_rows_early` + its cached previous).
- **BOUNDABLE-WITH-WORK** (region knowable, not currently computed or not
  reachable from the decision point): `folds_changed`, `search_changed`
  (only a hash today), `image_frame_changed`, `image_pixels_changed`,
  `text_blink_changed` (`has_blinking_text` is one whole-snapshot bool with
  no per-row bitmap), scrollbar visibility and hover-alpha, bell flash,
  `active_pane_changed`, all eight `ui_overlay_open` sub-conditions, both
  `pointer_forces_full_present` disjuncts, both `toast_active`
  sub-conditions, `tab_set_changed`, `tab_title_changed`,
  `pane_layout_changed`, `broadcast_state_changed`, `focus_changed`,
  `foreground_overlay_open`, `dismissible_presence_transitioned`,
  `DamageHistory::MAX_DEPTH` overflow.

#### Findings that are not merely classification

1. **`changed_rows` is already computed, tested, and deliberately unread.**
   `frame_dirty.rs` carries
   `#[expect(dead_code, reason = "computed and tested by 124.12; the first
   production reader arrives in 124.14")]`. 124.14 must remove that
   `expect`.
2. **The multi-pane fan-out is confirmed.** In `decide_frame_damage`'s loop,
   one pane reporting `Full` — or a bell in *any* pane — does
   `rects.clear(); break;`, discarding rects already collected from every
   other pane. **In a split, one busy pane forces a full clear + present of
   every provably-`Unchanged` sibling and all chrome.** This is arguably a
   larger prize than the per-pane bounding itself.
3. **Precedence is monotonic toward `Full` and finalised outside
   `central_body`.** `compose_with_chrome_damage` runs *after*
   `stage_frame_damage` and can upgrade a `Partial` to `Full`, never the
   reverse. So the true frame damage is not knowable from
   `stage_frame_damage`'s return value alone.
4. **`PartialPresentDecision::RequestedWithNoRects` is dead.**
   `decide_frame_damage` can never return an empty `Partial` (it collapses
   to `Full` first), and `event_loop.rs` is the only production constructor
   of `FrameSignals`. Same shape as the chrome cache's inertness — flagged
   deliberately.
5. **`unresolved_pane` reachability is UNKNOWN.** Not proven dead; the audit
   named the experiment (a counter plus a stress session with concurrent
   tab-close/split) rather than guessing.
6. **Chrome has 15 independently-sufficient signals**, each pinned by
   `chrome_damage.rs::each_signal_field_alone_forces_changed`. Any one alone
   forces the whole window `Full`.

#### What the audit did NOT verify, stated rather than glossed

`freminal-buffer`'s epoch machinery was taken on trust from 124.10-124.12's
completion, not re-derived. **If that guarantee is unsound, every
`BOUNDABLE-NOW` classification needs re-examination.** Image and text-blink
boundability rests on those structures not being reachable from
`evaluate_frame_dirty_state`'s current inputs, not on an exhaustive search of
the emulator crate. No experiment was run — the audit was read-only.

### 124.23 — The full-draw paint arm ignores the published present region

**Complete (2026-08-24), commit `fab22611`. 124.14a, b, c and d are now
unblocked.** *Added
2026-08-24 by 124.14a's activation recon. Maintainer chose design (a),
scissor the draw, on 2026-08-24.*

#### What is wrong

124.20 established the invariant that **one region governs the clip, the
clear and the present**. That invariant is enforced in the windowing layer
and in the app's *cursor-only* paint arm. It is not enforced in the app's
**full-draw** arm, which scissors nothing (`widget.rs:3117-3147`).

Today the cursor-only arm reads `PresentRegion` and scissors to it
(`widget.rs:3082-3116`) — deliberately to the windowing-published region
rather than the pane's own cursor rect, because on a stale back buffer the
published region is a union covering more than this frame's damage.

#### This is reachable today, not merely a hazard 124.14 would create

Established by reading, and stated as the reason this entry is a **fix**
rather than preparation:

- The paint-callback registration sits **outside** the
  `if !snap.skip_draw` block (`widget.rs:2465` opens it; registration is at
  `:2960`), so every pane registers a callback every frame regardless of
  whether it changed.
- `is_cursor_only` is a per-widget local initialised `false`
  (`widget.rs:2410`) and set `true` only inside the cursor-only rebuild arm
  (`:2532`). A pane reporting `Unchanged` therefore takes the **full-draw**
  arm.
- `decide_frame_damage` does **not** clear rects for an `Unchanged` pane —
  only `Full`, `CursorOnly(None)` and `bell_active` do
  (`frame_damage.rs:104-121`).

Compose those: in a split where pane A has a blinking cursor and pane B is
genuinely idle, the frame is `Partial` on A's cursor rect, the clear is
scissored to that rect, and **pane B redraws its whole viewport with no
scissor over pixels the clear deliberately skipped.**

#### The bound, stated so it is not overstated

At the default `background_opacity == 1.0` pane B's own quads are opaque and
fully overwrite the stale pixels, so nothing is visible — the same bound
124.20 recorded for its own defect, and the reason this has never been seen.
It bites `background_opacity < 1.0` (Task 34 transparency), where the idle
pane's quads blend against already-composited pixels from an older frame
(`a*fill + (1-a)*stale`) instead of against the clear colour, compounding
across frames.

Note it is **not** a defect that pane B's *untouched* pixels are stale. That
is what partial present is for, and 124.18's damage-history union covers a
pane that changed within the staleness window. The defect is specifically
the unscissored **draw** over an unscissored-away **clear**.

#### The fix — design (a), chosen by the maintainer

Both arms read the same `PresentRegion` and both scissor to it. Design (b) —
leave the draw unbounded and widen the published region to cover every pane
doing a full draw — was considered and declined: it cannot corrupt and is
less code, but it forfeits the fill saving and makes damage pane-granular,
and having two paint arms disagree about whether the region binds them is
the shape of bug this task keeps finding.

Scope: `freminal/src/gui/terminal/widget.rs` only. Lift the `PresentRegion`
read out of the `is_cursor_only` branch so both arms share one read, and
apply the same enable/scissor/disable discipline around `draw_with_verts`
that the cursor arm already applies around `draw_with_cursor_only_update`.
Leave GL scissor state as egui expects on both paths.

#### Why this can land ahead of 124.14a, and must

**It is behaviour-neutral for every frame that exists today** in the
single-pane case, because a pane that produces a `Region` takes the cursor
arm, so the full-draw arm only ever sees `PresentRegion::Full`, on which the
scissor is a no-op. That is what makes it independently verifiable: a `Full`
frame must be byte-identical before and after. The multi-pane case above is
the one behaviour change, and it is the bug fix.

Landing it inside 124.14a instead would put a correctness fix for a shipped
defect inside a subtask whose verification is aimed at a new enum, and would
leave no frame at which the no-op property could be demonstrated.

#### Verification for 124.23

The windowing-level harness in `frame_paint.rs` is the **wrong instrument**
and must not be used for this: it drives synthetic egui `FrameFn` closures,
and a GL `Primitive::Callback` is precisely the thing that bypasses egui's
scissor, so that harness structurally cannot reproduce the defect. The right
instrument is `freminal`'s own pixel harness
(`cargo test -p freminal --features gl-pixel`), which drives the renderer
directly.

Pin, fix, then invert, per the idiom used by 124.9, 124.16 and 124.C5:

1. a pixel test reproducing the blend-against-stale case at
   `background_opacity < 1.0` through the full-draw path, asserting the
   defective pixel values;
2. the fix;
3. the inversion, plus a test proving a `PresentRegion::Full` frame is
   byte-identical before and after (the no-op property).

Then `cargo test --all`;
`cargo clippy --all-targets --all-features -- -D warnings`;
`cargo clippy --workspace --all-targets`; `cargo machete`; both Task 123
harnesses; `cargo xtask check-windows` before the PR.

#### Prohibitions for 124.23

Do NOT add `PaneFrameDamage::Region` or touch `VertexRebuild` — that is
124.14a and this entry exists so it can be built safely. Do NOT change
`decide_frame_damage`. Do NOT touch the vertex layout or `upload_verts`. Do
NOT widen the published region (that is design (b), declined).

#### 124.23 implementation notes (2026-08-24)

Landed as specified. Both arms share one `PresentRegion` read and one
`draw_scissored_to_present_region` helper, so they cannot drift apart again
— which was the actual failure mode, not the missing scissor per se.

**Two residual gaps, recorded because neither is closed and both bear on
124.14a's risk.**

**(1) The blend-against-stale reproduction was not written, and cannot be
in this harness.** Verified rather than assumed: neither `draw_with_verts`
nor `HeadlessRenderer` issues a `glClear` anywhere, because the clear the
defect skips belongs to the windowing layer — so there is no
clear-happened-versus-clear-skipped distinction the app-level pixel harness
can even pose. Independently, `headless.rs:334` passes a hardcoded `1.0`
for `bg_opacity`, and the reproduction needs `< 1.0`. Both files were out
of scope. So the *live* defect this entry fixes is pinned by reading and by
the mechanism test, not by a reproduction. Closing that gap needs an
instrument that spans egui, the app callback and the windowing clear at
once — which is the same Phase 3-shaped gap 124.19 opened and did not
fully close.

**(2) The two pixel tests pin the mechanism, not the wiring.** They prove a
scissored draw writes nothing outside its rect, and that the rect
convention (physical pixels, bottom-left origin) is right. Nothing here
drives the app's paint callback — the Phase 3 harness lives in
`freminal-windowing` and cannot reach it — so "the full-draw arm actually
scissors" rests on the structure being obviously right (one read, one
helper, two call sites) rather than on a test. Stated so the coverage is
not later read as larger than it was.

**Why the `Full` no-op property has no direct test.** `PresentRegion::Full`
makes the helper touch no GL state at all, so there is nothing observable
to compare. The surviving test
(`a_full_viewport_scissor_box_clips_nothing`) pins the *convention*
instead, and its doc comment says so explicitly — an earlier draft
justified it as the `Full` arm's no-op property, which it is not.

#### 124.14a design decisions (2026-08-24, pre-implementation)

Settled before delegating so the next session inherits them rather than
re-deriving them. Both are choices the entry left open.

**`Region(Vec<CursorDamage>)`, and `PaneFrameDamage` loses `Copy`.** A
single bounding box per pane would keep `Copy` and is equivalent *today*,
because windowing's `DamageHistory::redraw_region` bboxes anyway. Reject it:
it is lossy exactly in this subtask's case — rows 3 and 40 changed would
present the 37 rows between them — and 124.18 names multi-rect scissoring as
a later optimisation to be justified by measurement. Collapsing at the
source would have to be undone to get that back, which is the "correct but
insufficient" trap 124.1 records for the `Arc`-reuse fix. The cost is a
handful of `.clone()`s at `app_impl.rs:343` and `:347` and wherever else
`last_frame_cursor_damage` is read by value; per pane, once per frame.

**`CursorDamage` is reused as the rect type and becomes a misnomer.**
Renaming it is mechanical but would land inside a behaviour diff and make
that diff harder to review, which is where the risk actually is. Deferred to
124.C7.

**`VertexRebuild::Rows` means "full vertex rebuild, bounded damage".** It is
not a bounded rebuild. The rebuild stays full and `upload_verts` stays a
whole-buffer write; bounding the upload is Task 125 and needs the
fixed-stride relayout. This is the boundary most likely to be crossed by
accident.

**Bound `rows_changed` only.** If `selection_changed`, `hover_changed`,
`search_changed`, `image_frame_changed` or `image_pixels_changed` fired, the
frame falls back to `ReevaluateFullRebuild`. Those are 124.14b, 124.14d and
124.C5's constraint respectively, and each boundary should be pinned by a
test so a later subtask cannot cross it silently.

**An empty rect set reports `Full`, not an empty `Region`.** Every changed
row can collapse into a fold. An empty `Region` would be a third way of
spelling "nothing", and `decide_frame_damage` already carries one dead
`RequestedWithNoRects` variant from the last time that happened (124.21
finding 4).

**Reuse `CursorDamage::from_cursor_cells` for the coordinate transform.** It
already consumes top-left-origin viewport-relative pixels and performs the
Y-flip, outward rounding, 1px pad and framebuffer clamp. A second hand-rolled
transform is how these go wrong.

### 124.C7 — `CursorDamage` is a misnomer once `Region` carries it

*Deferred from 124.14a's design, 2026-08-24. Cleanup entry.*

**Complete (2026-08-25), commit `7e63bb8c`.** `CursorDamage` is now
`PaneDamageRect` across the renderer, frame aggregation, widget, search-overlay
damage state, tests, and the damage-model skill. The existing
`from_cursor_cells` coordinate transform and all rectangle semantics are
unchanged; this was an isolated nominal refactor.

Before this cleanup, `PaneFrameDamage::Region` reused `CursorDamage` as its
rect type, so the name described only one of its users. The current call-site
inventory also includes search-overlay damage and app-level tests added after
this entry was written; all were renamed mechanically in the same commit.

It was held back deliberately rather than folded into 124.14a: a rename
inflates a behaviour diff and the review risk in 124.14a is concentrated in
the coordinate transform and the damage aggregation, which a rename would
bury. It therefore landed as its own commit after 124.14a.

#### 124.14b recon (2026-08-24) — the subtask splits in two

**Selection and hover are not the same shape of problem, and pairing them
in one subtask hides a real hazard in the smaller half.** Split into
**124.14b-i (selection)** and **124.14b-ii (hover)**.

> **CORRECTED 2026-08-24, before implementation. The hazard below is
> WRONG.** The gutter strip's fill is `snap.theme.gutter_color_for(status)`
> — the block's *status* colour, plus a desaturate flag for fold
> placeholders — and its paint block (`widget.rs:3535-3608`) contains **no
> hover term at all**. The hover tint is baked into the background instance
> buffer *inside* `terminal_rect` (`widget.rs:2303`: "the hover tint is
> baked into the background instance buffer"), so hover damage does **not**
> escape the terminal rect and **no gutter widening is needed**.
>
> The error was misreading `compute_command_block_hover_rows`' doc
> (`widget.rs:360`): *"the gutter strip is the sole hover **trigger**"*
> describes which surface the pointer must be over to trigger a hover, not
> which surface gets painted. Trigger surface was read as paint surface.
>
> The split into b-i and b-ii is **kept anyway** — two smaller commits with
> independent evidence is still better than one — but b-ii is the same
> shape as b-i, not a harder problem. The URL-hover tooltip is a separate
> signal (`hover_tooltip_active()` = `cached_hovered_url.is_some()`) which
> already forces `Full` via `foreground_overlay_open`, and the command-block
> duration label is not hover-dependent. Both checked.
>
> Retained rather than deleted because a wrong hazard that was investigated
> and disproved is worth more to the next reader than silence: it records
> that the question was asked and answers it.

**The hazard, found by reading: hover damage escapes `terminal_rect`.**
`terminal_rect` deliberately starts at `pane_rect.min.x + gutter_inset`
(`widget.rs:1933`) — the command-block gutter strip is **outside** it, and
is painted separately by egui, not by the GL callback. `row_run_damage`
builds rects from the viewport origin rightward, so a hover-bounded rect
covers the terminal rows and **not** the gutter.

`hover_changed` is `command_block_hover_rows_early !=
cache.previous_command_block_hover_rows` (`frame_dirty.rs:567`) — it fires
when the hovered *block* changes, which moves the gutter's tinted segment.
The gutter's own signal, `gutter_hover_repaint_decision`
(`widget.rs:643-650`), is a pure boolean over "is the pointer in the gutter
at all", so moving between two blocks does **not** flip it. Nothing else
would mark the gutter damaged.

Since 124.18 clips every egui primitive to the redraw region, a
hover-bounded frame would clip the gutter repaint away and leave a **stale
tint on the previously-hovered block**. That is an under-report — silent
visual corruption, the issue #432 class — not a missed optimisation.

**Consequence for 124.14b-ii:** a hover rect must be widened leftward over
the gutter strip (`gutter_inset * ppp`), so clip, clear and present still
agree on one region per 124.20's invariant. Recorded rather than
implemented, because it deserves its own commit and its own pixel evidence.

**124.14b-i (selection) has no equivalent escape.** The selection highlight
is drawn as decoration/background vertices inside `terminal_rect` only.

#### 124.14b-i design decisions (2026-08-24)

**Cache the previous frame's selection in *screen*-row space, not
buffer-absolute.** `cache.previous_selection` is buffer-absolute
(`frame_dirty.rs:534`) and `screen_selection` is snapshot-row space
(`:569`), so a naive union would compare two different spaces — the trap
recorded in 124.14a's recon. Translating the old selection with *this*
frame's `win_start` is also wrong whenever the window moved between frames
(a scroll, or new output pushing rows into scrollback while `scroll_offset`
stays 0, which does not set `scroll_changed`).

The question being answered is **"where on screen was the old highlight
painted"**, which is inherently a screen-space question, so the answer is
stored in screen space and needs no translation and no window-movement
guard. Added as a `PaneRenderCache` field updated in lockstep with
`previous_selection` (`widget.rs:3106`), inside the rebuild body, so it only
advances on a frame that actually drew.

**`VertexRebuild::Rows` is renamed `VertexRebuild::Bounded`.** Once
selection contributes, "Rows" names one of several sources. The variant is
one commit old, so the rename costs nothing and prevents a name that lies.

**The damage extent is the union of every bounded source, merged into runs
once**, rather than each source emitting its own overlapping rects.

#### 124.14d recon (2026-08-24) — BLOCKED, terminal damage is erased by chrome

**Do not implement 124.14d from the original entry. Its premise does not
match the current frame path.** The requested search extent is buildable —
`SearchState::matches` carries real `MatchSpan` row locations — but consuming
it in `VertexRebuild::Bounded` would have zero present effect while search is
open:

1. `stage_frame_damage` sets `foreground_overlay_open` when any pane has
   `pane.view_state.search_state.is_open` (`app_impl.rs:338-341`).
2. `foreground_overlay_open` is independently sufficient for
   `ChromeDamage::Changed` (`chrome_damage.rs:136-156`), pinned by
   `foreground_overlay_open_alone_forces_changed` (`:440-448`).
3. `compose_with_chrome_damage` upgrades any terminal `FrameDamage` to
   `FrameDamage::Full` whenever chrome reports `Changed`
   (`frame_damage.rs:167-174`).

So a bounded search-highlight region would be computed, transported and then
discarded on every open-search frame. That is the same inert-optimisation
shape 124.5 found in the chrome cache and 124.17 found in partial present;
landing it anyway would improve an intermediate enum while changing no clear,
draw or present.

The original 124.14 entry did not account for this independent chrome path.
Its statement that search merely needs "a real extent built" is therefore
insufficient: 124.14d also needs a design for bounding the search overlay's
chrome damage, or an explicit decision that the overlay remains globally
damaging and search highlights stay unbounded with it. That is a maintainer
decision because it changes 124.14d's scope from terminal damage into the
chrome damage model. Per the task's stop rule, no workaround or inert partial
implementation was started.

> **UNBLOCKED 2026-08-24 by maintainer decision: expand 124.14d into the
> chrome damage model.** The final framebuffer is one surface; the split
> between terminal damage and chrome damage is an implementation boundary,
> not a reason to discard known geometry. Search changes two independently
> knowable regions and both join the final `FrameDamage::Partial`:
>
> 1. old/new search-highlight rows in the terminal band;
> 2. old/new floating search-bar paint bounds, including the popup shadow.
>
> Search is then removed from the binary `foreground_overlay_open` chrome
> escalation. Context menus, command history, URL tooltips and every other
> foreground overlay remain globally damaging.

#### 124.14d expanded design (2026-08-24)

**The search bar reports its actual paint bounds.** `show_search_bar` returns
a named output containing its `SearchBarAction`, the `egui::Area` response
rect, and whether a tooltip-bearing control is hovered. The paint rect is the
area rect expanded by `Frame::popup(ui.style()).shadow.margin()` — egui's own
shadow-bound calculation (`epaint::Shadow::margin`), not a guessed constant.
The old and new popup rects are both damaged so opening, resizing and closing
erase the previous pixels correctly.

**Button tooltips retain the safe `Full` fallback.** Prev, next, close and
the match-case control use `on_hover_text`; their tooltip can paint outside
the popup rect. While any such response is hovered, search remains an
unbounded foreground overlay for that frame. This may over-report during the
tooltip delay, which is safe; assuming a tooltip bound we do not have would
under-report. Normal typing, navigation, caret blinking and highlight changes
stay bounded.

**Search highlights use old/new visible screen-row sets.** `MatchSpan` rows
are buffer-absolute; the existing `matches_to_highlights` plus fold/layout
translation already produces the exact current screen rows drawn by the
renderer. Cache the previous drawn screen-row set and union it with the
current one. A broad search costs at most one entry per visible row, not one
per full-buffer match. Old rows erase highlights that disappeared; new rows
draw highlights that appeared or changed.

**Search-only changes with no visible highlight rows are valid bounded
frames.** The popup still supplies damage, so the terminal-band contribution
may be `PaneFrameDamage::Unchanged`; it must not fall back to `Full` merely
because no match is visible. Selection/hover/row sources keep their existing
`Full` fallback when no bound can be established.

**Damage ownership gets a named home.** Add
`terminal/search_damage.rs::SearchDamageState`, owned by `PaneRenderCache`,
instead of adding three unrelated fields to `widget.rs`. Its invariant:
previous highlight rows and previous popup bounds describe what was actually
drawn on the last relevant frame; per-frame popup damage is the deduplicated
union of old/current bounds; and the tooltip safety state is reset and
recomputed every frame. `PaneRenderCache` exposes only the current frame's
popup damage and safety classification to `stage_frame_damage`.

**The coarse chrome signal remains binary for everything else.** Do not add
a `Region` variant to `ChromeDamage`: search geometry is composed into
`FrameDamage` before the existing binary chrome composition. Rename/comment
the local signal as the *unbounded* foreground-overlay set so excluding
search is explicit rather than silently changing what "any foreground
overlay" means.

#### 124.14 implementation notes (complete 2026-08-25)

Task 124.14 landed in four behaviour commits plus two prerequisite fixes:

| Subtask | Commit | Result |
| ------- | ------ | ------ |
| 124.C5 | `74e94576` | Image placement joined the row-epoch render basis; the hard gate on row damage closed |
| 124.23 | `fab22611` | Both paint arms obey one windowing-published region |
| 124.14a | `eae76d1b` | Row-only changes report bounded pane damage |
| 124.14b-i | `edf9e017` | Selection damage unions old/new screen rows |
| 124.14b-ii | `284ce253` | Command-block hover damage unions old/new screen rows |
| 124.14c | `058c2627` | A busy pane reports its own rect instead of forcing every sibling full |
| 124.14d | `ab275052` | Search highlights and floating popup chrome join one bounded surface-damage decision |

**The upload boundary held.** Every `VertexRebuild::Bounded` frame still
runs the full vertex rebuild and `upload_verts` still writes whole buffers.
Only clear/draw/present damage became bounded. Task 125 remains the fixed-
stride upload-relayout question.

**Old and new extents are always unioned.** Rows, selection, command-block
hover, search highlights and the floating search popup all include the pixels
drawn last frame as well as this frame. That is what erases shrinking or
moving decoration instead of leaving stale pixels behind.

**Search required crossing the old terminal/chrome split.** A first attempt
would have built highlight geometry only to have
`foreground_overlay_open -> ChromeDamage::Changed` erase it. The maintainer
expanded 124.14d: the final framebuffer is one surface, so old/new highlight
rows and old/new popup bounds join `FrameDamage` directly. Binary
`ChromeDamage` remains unchanged for context menus, command history, URL
tooltips and every other unbounded chrome source.

**Search tooltip safety has a one-frame settle.** Prev/Next/Close/Aa tooltips
can paint outside the popup rect. A hovered control forces `Full`, and the
first frame after hover ends also forces `Full` to erase the old tooltip;
then the state returns to bounded. Resetting immediately was caught during
orchestrator review before commit.

**Other review corrections:** the implementation's production lint
suppression was removed rather than justified; tooltip safety uses one named
domain enum end to end instead of private helper booleans; and the
`zero_change_presented` profiler now excludes frames carrying search-popup
damage, so the planned 124.17 re-measurement is not poisoned by the new
damage source.

**Verification:** the full 106-group workspace suite, both clippy commands,
`cargo machete`, all warning-sensitive build configurations, the nine
`gl-recording` headless workloads, the five-test windowing frame-paint
harness and the 1,353-test `gl-pixel` run all passed. Mutation checks proved
the search bounded-source decision, tooltip escape fallback and popup-only
damage aggregation are each load-bearing.

#### Post-124.14 GPU re-measurement (2026-08-25), AUTHORITATIVE

**On real hardware, with 124.18's history union and 124.14's bounded sources
both landed, the partial-present path is now taken on the overwhelming
majority of frames.** This is the direct inverse of the pre-124.18 124.17
GPU take, which recorded `present_partial_taken == 0` across 8,160 frames
because `buffer_age()` was 2 and the gate required exactly 1.

Measured on this workstation, Hyprland/Wayland, AMD GPU (`LIBGL_ALWAYS_SOFTWARE`
printed `<unset>`; the live process's llvmpipe thread count was 0 in every
run), release build with `--features frame-profiling`. The floating window
was pinned to 1264x680 at `(-2400, 200)`; the pointer was verified inside the
window before each run. The continuous PTY workload was
`sh -c 'while :; do seq 1 200; sleep 0.02; done'`. Steady state is the
difference between frame 120 and frame 2520 in each run — 2,400 frames per
run.

Run 1 drove real pointer motion (144.16 events/s observed). Runs 2 and 3 are
**output-only repeat controls**, not pointer-motion runs: compositor-driven
synthetic cursor moves produced no application `CursorMoved` events, so no
pointer-rate figure is reported for them rather than claiming motion that did
not reach the app.

| Metric | Run 1 | Run 2 | Run 3 |
| ------------------------------------ | -------------- | --------------- | --------------- |
| fps | 60.28 | 60.21 | 59.97 |
| total us/frame | 331.55 | 329.02 | 406.57 |
| run_ui us/frame | 123.72 | 122.87 | 153.62 |
| tessellate us/frame | 11.84 | 12.03 | 13.92 |
| paint us/frame | 15.94 | 16.22 | 20.36 |
| swap us/frame | 156.10 | 154.34 | 189.44 |
| Partial taken / requested | 2104/2400 | 2063/2400 | 2079/2400 |
| Partial taken (%) | 87.67% | 85.96% | 86.62% |
| Full count | 296 | 337 | 321 |
| buffer-age-blocked count | 0 | 0 | 0 |
| buffer_age_histogram delta `[0,1,2,3+]` | `[0,0,2104,0]` | `[0,0,2063,0]` | `[0,0,2079,0]` |
| zero-change `Full` count | 289 | 280 | 273 |
| other documented `Full` cause | `focus_changed` 7 | `toast_active` 57 | `toast_active` 48 |
| pointer events/s | 144.16 | n/a (output-only control) | n/a (output-only control) |

**Interpretation:**

- This directly inverts pre-124.18's GPU 124.17 result on its central term:
  `buffer_age()` remains exactly 2 in steady state in every query across all
  three runs — the swapchain's staleness has not changed — but with the
  124.18 damage-history union in place, every `Partial` request that reaches
  the gate is now taken rather than blocked. `buffer-age-blocked` is 0 in
  all three runs.
- 124.14 moved changing-content frames from `Full` to `Partial` exactly as
  its design intended: `Partial` is taken on 85.96%-87.67% of the 2,400
  steady-state frames per run.
- **Do not characterise this as "almost no `Full` frames."** Every observed
  `Full` frame is fully and exactly accounted for: run 1 is 289 + 7 = 296,
  run 2 is 280 + 57 = 337, run 3 is 273 + 48 = 321. The zero-change component
  in each run is the `rects.is_empty() -> Full` fallback that 124.2 owns, not
  a defect in 124.14. `focus_changed` and `toast_active` are documented
  genuinely-global/chrome causes from the 124.21 audit, not unexplained
  residue.
- Frame rate and per-frame stage costs are reported together above rather
  than in isolation, per the measurement discipline this task follows
  throughout: all three runs hold ~60 fps with `run_ui`/`tessellate`/`paint`/
  `swap` in the low hundreds of microseconds. Run 3 is the high-cost repeat
  of the three (406.57 us/frame total against 331.55 and 329.02 for runs 1
  and 2); the measured values are reported as-is and no causal conclusion is
  drawn from a single repeat.
- **No visual corruption was observed.** A live screenshot was captured and
  inspected under the same continuous-output workload; terminal content was
  intact.

**Conclusion: 124.2 is unblocked.** `FrameDamage::None`'s `buffer_age()`
interaction — flagged as "the correctness crux" when 124.2 was written, and
resolved by 124.17's GPU take as "age is 2 in steady state, so a subsequent
`Partial` frame is declined and falls back to a full clear" — now resolves
further: with 124.18's history union, that fallback is no longer the
common case, and 124.2's `rects.is_empty()` zero-change path is the
best-characterised remaining `Full` source in the measurements above. 124.2
is the next maintainer-set task.

### 124.22 — `freminal-damage-model` agent skill

**Complete (2026-08-25). `3de34651`.** *Requested by the maintainer during
124.21.*

Codify the rule that a full-surface repaint is a last resort, so future work
does not silently re-add unbounded damage. The skill fires when adding or
changing anything that writes `PaneFrameDamage`, `FrameDamage`,
`ChromeDamage`, or any `*_changed` flag feeding them.

It must carry: the eight genuinely-global triggers and why each is global;
the requirement that any new trigger states its classification
(GLOBAL / BOUNDABLE-NOW / BOUNDABLE-WITH-WORK) and justifies GLOBAL rather
than defaulting to it; the monotonic-toward-`Full` precedence rule and that
damage is only final after `compose_with_chrome_damage`; the multi-pane
`rects.clear(); break;` fan-out; the one-region-for-clip-clear-and-present
invariant from 124.20; and the rule that bounding a trigger whose extent is
not provably complete is silent visual corruption, so `GLOBAL` is the safe
default *only* when the extent genuinely cannot be established.

Written after 124.14 landed, so it codifies the shipped model rather than
the intended one. `.opencode/skills/freminal-damage-model/SKILL.md` landed,
and `agents.md`'s skill table was updated to register it.

#### 124.22 implementation notes (2026-08-25)

The skill's ten sections carry the durable content, not a restatement of
124's narrative:

- **Section 1, the classification rule.** Every new or changed trigger must
  state GLOBAL / BOUNDABLE-NOW / BOUNDABLE-WITH-WORK, and a current `Full`
  fallback is explicitly *not* proof of GLOBAL — several audited triggers
  are `BOUNDABLE-WITH-WORK` and only unbounded because the geometry was
  never built, so the skill forbids canonising that debt as GLOBAL.
- **Section 2, the eight genuinely-global categories** from the 124.21
  audit (theme change, resize, `ChangedRows::All`, empty prior
  decoration/vertex state, degenerate cursor damage, shader recomposites,
  chrome style/size/ppp change, and unrepresentable windowing history),
  each with why no bounded region suffices, plus the explicit warning that
  this is a category list, not license to leave every other `BOUNDABLE`
  trigger unbounded forever.
- **Section 3, the shipped flow**: `PaneFrameDamage` (per pane) ->
  `decide_frame_damage` -> `FrameDamage` -> `compose_with_chrome_damage` ->
  windowing's `DamageHistory`/`PresentRegion`. Carries the monotonic-toward-
  `Full` precedence rule, that damage is not final until after chrome
  composition, and that `FrameDamage::None` is never pushed into
  `DamageHistory`.
- **Section 4, multi-pane fan-out**: any pane reporting `Full`,
  `CursorOnly(None)`, or an active bell flash discards every rect already
  collected from sibling panes for that frame; 124.14c's fix (a boundable
  busy pane reports its own region rather than forcing `Full`) must not be
  regressed.
- **Section 5, the one-region invariant** from 124.20/124.23: the exact
  `PresentRegion` windowing publishes governs clip, clear, draw, *and*
  present together — never derive a second, app-local scissor from a
  frame's own declared damage, since `DamageHistory`'s buffer-age union can
  make the published region wider than this frame's own damage.
- **Section 6, the complete-bound rule**: bounded damage must cover the
  union of old and new extents, not just the new one; under-reporting is
  silent corruption while over-reporting is only wasted work, so `Full` is
  the safe fallback when completeness can't be proven.
- **Section 7, coordinate/representation constraints**: damage rects are
  physical framebuffer pixels in bottom-left-origin `glScissor` convention,
  reuse `CursorDamage::from_cursor_cells` rather than hand-rolling a second
  transform, damage state must be a named domain enum per
  `freminal-state-representation`, and `DefaultBackground` means "leave
  these pixels untouched" — a constraint a call-count test cannot verify,
  only the pixel harness can.
- **Section 8, the Task 125 boundary**: `Region`/`Bounded` bound the
  present (clear/draw/present), not the vertex upload — a `Bounded` frame
  still runs a full vertex rebuild and writes the whole instance buffer,
  and the skill forbids touching `upload_verts` or the vertex emission
  format from a damage change.
- **Section 9, the review checklist**, and **section 10, stop triggers**
  (classifying GLOBAL without justification, a bound crossing the
  terminal/chrome ownership split without prior authorization, hand-rolling
  a coordinate transform, shipping an unproven bound, weakening a `Full`
  fallback, touching Task 125's territory, a pixel harness that can't reach
  the change, and terminal-semantics ambiguity per `agents.md`).

**Verification performed:** the new skill file was run through
`markdownlint-cli2` and produced zero local findings. Adding the new row
to `agents.md`'s skill table did not introduce any new MD060
(table-column-width) issue — the repository's markdownlint config already
reports seven pre-existing local findings in `agents.md` unrelated to this
change, and that count did not increase. The actual pre-commit
`markdownlint` hook passed on the commit, as did the rest of the
pre-commit hook suite; the skill's frontmatter `name` field matches its
containing folder name (`freminal-damage-model`), satisfying the shared
skill-authoring convention.

**At 124.22's completion, 124.C3, 124.C6, and 124.C7 remained
maintainer-approval-gated and were not touched by that entry.** The maintainer
subsequently closed C3 unexecuted and approved C6 and C7; both implementations
are now complete.

### 124.C5 — Inline image placement is invisible to the row epoch

**Complete (2026-08-24). Two commits: the pin `1339208c`, the fix
`74e94576`.**

*Surfaced 2026-08-24 by the read-only verification of 124.10-124.12's
no-under-report guarantee, commissioned before 124.14a was decomposed.
Cleanup entry. **Pre-existing and already shipped** — 124.14 does not
introduce it, but must not build on top of it.*

**The guarantee holds for text.** Row epochs do not under-report changes to
`chars`, `tags` (after URL refinement) or `line_width`. That was re-derived
by reading, and is backed by an always-on `debug_assertions` oracle
(`debug_verify_epochs`) plus a `proptest` fuzzer, both of which run in every
`cargo test`.

**It does not cover inline image placement.** `flatten_row` never reads
`cell.image_placement()` — only `cell.tchar()` and `cell.tag()`. `Cell::image_cell`
stamps the cell's value as `TChar::Space` under whatever `FormatTag` was
active. So placing an image over cells that already held plain spaces under
an identical tag leaves `chars`, `tags` and `line_width` byte-identical, the
epoch is carried forward, and the row is reported unchanged **while its
rendered pixels have changed.**

Reproduced empirically against `freminal-buffer`, not merely reasoned about:

```text
row_epochs(before) = [1, 2, 3, 4, 5]
place_image(new id) over row 0's three literal-space cells, same default tag
row_epochs(after)  = [1, 2, 3, 4, 5]      <- row 0 did NOT bump
cell(0,0) now shows image id 1
```

**Why it was previously harmless, and why that matters.** `place_image` sets
`row.dirty = true`, which forced a rebuilt `RowCacheEntry` and therefore a
**fresh `Arc`** regardless of byte content — and `Arc::ptr_eq` treats any new
allocation as changed. That accidental safety net is exactly what
124.10-124.12 deliberately removed, since a byte-identical re-flatten must
not look changed. Image placement fell into the blind spot that created.

`image_pixels_changed` catches some sub-cases coincidentally (a brand-new
image id changes the pixel-map's key set), but **not** an already-known image
moved onto content-identical cells. `snap.visible_image_placements` has no
independent diff anywhere in the GUI.

**Binding constraint on 124.14a — now partially lifted, and the remaining
half is the part that matters.** `changed_rows` is a sound, tested,
non-under-reporting signal for text, tag, line-width **and image-placement**
content, and 124.14a may proceed on that basis for the **present region**.
It is still **not** a complete signal for image *pixel* damage: an animation
frame advancing, or an image's transmitted bytes being replaced, changes no
placement and so bumps no epoch. Image vertex rebuilding must therefore stay
on its existing coarser whole-pane trigger set (`image_frame_changed ||
image_pixels_changed || ...`) and must **not** be folded into the per-row
`changed_rows` decision. What C5 bought is that a row whose image *placement*
changed can no longer be excluded from a bounded present region — which was
the gate — not that `changed_rows` has become a complete image damage signal.

#### 124.C5 implementation notes (2026-08-24)

**Fixed in 124.10's `line_width` idiom, deliberately, because it is the same
shape of problem**: something a row renders as that never appears in
`chars`/`tags`. `RowCacheEntry` gains `images: Vec<RowImageCell>` captured in
`flatten_row`, `MergeCache` gains `row_images` as the previous-frame half,
and `RowRenderBasis::row_renders_identically` compares them alongside
everything else. That predicate is the single choke point —
`row_epochs_for_merge`, the incremental path's reused-prefix carry and the
`debug_verify_epochs` oracle all route through it — so all three gained image
coverage without being touched. Two `RowRenderBasis` construction sites and
one `MergeCache` construction site exist; all were audited, not assumed.

**The whole `ImagePlacement` is compared, not just `image_id`.** A field that
turns out not to affect rendering costs a spurious repaint; a field omitted
from the comparison is silent visual corruption. Only the first is
recoverable. For the same reason the placements are compared literally rather
than hashed — 124.10 rejected content hashing on the record because a
collision is an under-report.

**Keying on the post-continuation-skip `char_idx` is exact, not lossy.**
`Cell::image_cell` sets `is_wide_continuation: false`, and `Cell::from_parts`
— the only other constructor that can set that flag — sets `image: None`. So
no cell can be simultaneously a wide continuation and hold a placement, and
`flatten_row`'s `continue` can never skip an image cell. Verified in
`cell.rs` rather than inferred from the happy path.

**Cost:** rows with no images produce a `Vec::new()`, which does not
allocate, so the common case is one `Option::is_some` check per cell. Only
re-merged rows clone.

**Tests: the pin was inverted, not deleted**, keeping both of its evidence
assertions (that the placement really landed, and that the merged `chars`
really are byte-identical) — without those the inverted assertion could pass
for the wrong reason. Four more were added: image *removal* (a distinct code
path, `clear_image` rather than `image_cell`); an already-known `image_id`
moved onto a different content-identical cell, which is the sub-case this
entry records as caught by no other signal; a `z_index`-only change, pinning
that the whole placement is compared; and a degenerate guard proving an
unchanged image still does **not** bump. That last one is load-bearing: a
comparison that always reported "different" would satisfy every other test
here.

**Verified by mutation rather than by assertion count.** With the four-line
`images` comparison removed from `row_renders_identically`, all four "does
bump" tests fail and the degenerate guard still passes. The tests are
therefore pinned to the production change and not to the fixture.

**Not covered, stated rather than glossed:** the `proptest` fuzzer in this
file (`incremental_merge_matches_full_merge`) lives in
`incremental_merge_tests`, generates only `set_cursor_pos` + `insert_text`
with ASCII and tag changes, and asserts merged-output equality against an
oracle. It does not touch `visible_row_epochs` at all, so it gives the epoch
mechanism **zero** coverage — image-related or otherwise. The
`debug_assertions` oracle `debug_verify_epochs` is the epoch's coverage, and
that one *did* gain the image term automatically.

### 124.C6 — `search_corpus` over an open fold desyncs `merge_cache` permanently

*Surfaced by the same verification. Cleanup entry. Performance, not
correctness — the direction is over-reporting, which is safe.*

`interface.rs` records that a `search_corpus` call "costs one spurious full
repaint". **That characterisation is empirically wrong.** When Ctrl-F runs
while a fold has extended the window (`extra_rows > 0`), `search_corpus`
overwrites `merge_cache` with a different-sized window, and every subsequent
**idle** `build_snapshot` then takes `visible_row_epochs`' no-matching-cache
fallback and re-stamps every row — indefinitely, with zero PTY activity,
until some row is genuinely dirtied and resyncs the cache:

```text
idle snap 0 epochs = [12, 13, 14, 15, 16, 17]
idle snap 1 epochs = [18, 19, 20, 21, 22, 23]
idle snap 2 epochs = [24, 25, 26, 27, 28, 29]
```

Control: with matching windows the sequence stays `[1,2,3,4,5]` forever.

This is the sticky-full-rebuild-every-frame class that 124.10-124.12 exists
to eliminate, reintroduced by one specific interaction. Related to 124.C3
(both are `merge_cache` eviction), but distinct: C3 is alt-screen round
trips costing one rebuild each; this recurs every frame until broken.

#### 124.C6 implementation notes (2026-08-25)

**Complete, commit `c77047bd`.** Search now flattens the normal visible window
through `Buffer::visible_as_tchars_and_tags_full_merge`, which reuses the
existing per-row caches but neither reads nor replaces the snapshot path's
`merge_cache` and does not mint row epochs. No additional persistent cache or
state was added.

The regression test establishes a fold-extended snapshot window, requests a
search corpus, and proves repeated idle snapshots retain identical row epochs.
It then edits one visible row without changing the window and proves exactly
that row's epoch changes. A buffer-level test independently proves the search
flatten leaves the extended-window merge cache allocation and fingerprint
untouched while producing the same visible content as the ordinary flatten.

### 124.C8-C12 — PR #503 review remediation

**Complete (2026-08-26).** CodeRabbit's full review, including all four
nitpick comments, surfaced two correctness gaps and three cleanup groups:

| Entry | Finding | Resolution |
| ----- | ------- | ---------- |
| 124.C8 | Run-cache keys could alias equal text with different grapheme widths | `char_widths` joined the structural key; a regression test pins the miss |
| 124.C9 | A bounded content rebuild could omit old/new cursor rows | Cursor screen rows now join damage only when cursor state changed |
| 124.C10 | Renderer verification and measurement helpers had unsafe assumptions or duplication | Log `GL_RENDERER`, harden assertions/texture disposal, share pixel setup |
| 124.C11 | Search safety composition had two implementations | One truth-table-tested `combine` now owns the rule |
| 124.C12 | Skill/status/comment prose had drifted from behavior | Corrected without changing runtime behavior |

The shaping benchmark's cache-hit path changed by +2.9% and the partial-dirty
path by +7.9%, both below the 15% threshold and accepted for the correctness
fix. Full workspace tests, strict clippy, machete, the GL pixel tests, benchmark
compilation, and the Windows cross-check cover the combined remediation.
