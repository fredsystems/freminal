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
| 124.2 | `FrameDamage::None` — a frame that changed nothing presents nothing | Ready — its `buffer_age` crux is answered by 124.18 |
| 124.3 | Cell-granular pointer suppression | Ready — 124.13 confirms the case |
| 124.4 | Named-field struct for the pointer-motion predicate | Complete |
| 124.5 | Decide and execute the chrome cache's fate | Complete — deleted |
| 124.6 | Shaping-path levers | Ready — 124.16 supports lever 1 |
| 124.7 | GPU buffer-orphaning for small payloads | Complete |
| 124.8 | `DESIGN_DECISIONS.md` entry for the Phase 0 / Task 121 outcome | Complete |
| 124.9 | `sync_atlas` re-uploads glyphs a full upload already covered | Complete |
| 124.10 | Per-row content epoch in `freminal-buffer` | Complete |
| 124.11 | `row_epochs` on `TerminalSnapshot`; delete `content_changed` | Complete — field landed here, deletion landed in 124.12b |
| 124.12 | GUI consumes epochs; delete the `Arc::ptr_eq` content test | Complete |
| 124.13 | Re-measure pointer-motion suppression rates | Complete |
| 124.14 | `PaneFrameDamage::Region` and `VertexRebuild::Rows` | **Ready — and its prize is no longer zero, see 124.18** |
| 124.15 | Measure chrome's per-frame cost | Complete |
| 124.16 | Shaping cache instrumentation and a TUI-redraw benchmark | Complete |
| 124.C1 | `decide_frame_damage`'s doc comment describes a removed term | Complete |
| 124.C2 | `sync_toast_atlas` carries the same defect as 124.9 | Complete |
| 124.17 | Does the skip-clear + partial-present path ever actually fire? | Complete — re-taken on GPU; answer was **no** |
| 124.18 | Make partial present actually work (gate + clipping) | **Complete** |
| 124.19 | Phase 3: an egui-level offscreen pixel harness | **Complete** — 124.19a extraction, 124.19b harness |
| 124.20 | Scissor the clear to the redraw region, don't skip it | **Complete** |
| 124.C3 | `merge_cache` has no per-buffer stash, so alt-screen round trips over-report | Planned — maintainer has not decided whether to execute |
| 124.C4 | A pixel golden must not be compared across rasterisers | **Complete** |
| 124.21 | Exhaustive audit of every full-repaint-forcing trigger | **Complete** — 52 triggers, 8 genuinely global |
| 124.22 | `freminal-damage-model` agent skill | Planned — write after 124.14 |
| 124.C5 | Inline image placement is invisible to the row epoch | Planned — **constrains 124.14a** |
| 124.C6 | `search_corpus` + open fold desyncs `merge_cache` permanently | Planned |

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

### 124.3 — Cell-granular pointer suppression

*Migrated from 121.15 + 121.17. Gated on 124.13.*

Nearly all interactive terminal state changes at **cell** granularity — URL
hover, gutter hover, selection extent, mouse-tracking reports — so pointer
motion within one cell cannot change any of it. Caching the pane
terminal-rect origin and logical cell size and suppressing any `CursorMoved`
that does not cross a cell boundary would remove the pane-wide `has_urls`
and `scroll_offset` vetoes, let selection drags suppress, subsume the gutter
carve-out, and remain correct for mouse-tracking mode.

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

### 124.14 — `PaneFrameDamage::Region` and `VertexRebuild::Rows`

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

That still wins, because the existing cursor-only path already proves the
mechanism: `widget.rs:3038-3067` scissors the draw on the authoritative
`present_is_partial` flag, and `egui_integration` restricts the EGL present.
Extending that from one cursor rect to an arbitrary row-range rect set is the
change.

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

### 124.22 — `freminal-damage-model` agent skill

**Planned.** *Requested by the maintainer during 124.21.*

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

Should be written after 124.14 lands, so it codifies the shipped model
rather than the intended one.

### 124.C5 — Inline image placement is invisible to the row epoch

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

**Binding constraint on 124.14a:** `changed_rows` is a sound, tested,
non-under-reporting signal for text/tag/line-width content — proceed on that
basis. It is **not** sound for image damage. Image vertex rebuilding must
stay on its existing coarser whole-pane trigger set
(`image_frame_changed || image_pixels_changed || ...`) and must **not** be
folded into the per-row `changed_rows` decision until this entry is closed.
Doing so would take a currently-quiet bug and make it load-bearing for a
correctness property 124.14a is supposed to provide.

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
