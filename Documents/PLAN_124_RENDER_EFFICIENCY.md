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
| 124.2 | `FrameDamage::None` — a frame that changed nothing presents nothing | Planned |
| 124.3 | Cell-granular pointer suppression | Planned, after 124.13 |
| 124.4 | Named-field struct for the pointer-motion predicate | Planned, ungated |
| 124.5 | Decide and execute the chrome cache's fate | Planned, after 124.15 |
| 124.6 | Shaping-path levers | Planned, after 124.16 |
| 124.7 | GPU buffer-orphaning for small payloads | Planned, ungated |
| 124.8 | `DESIGN_DECISIONS.md` entry for the Phase 0 / Task 121 outcome | Planned |
| 124.9 | `sync_atlas` re-uploads glyphs a full upload already covered | Planned, ungated |
| 124.10 | Per-row content epoch in `freminal-buffer` | Planned |
| 124.11 | `row_epochs` on `TerminalSnapshot`; delete `content_changed` | Planned |
| 124.12 | GUI consumes epochs; delete the `Arc::ptr_eq` content test | Planned |
| 124.13 | Re-measure pointer-motion suppression rates | Planned |
| 124.14 | `PaneFrameDamage::Region` and `VertexRebuild::Rows` | Planned |
| 124.15 | Measure chrome's per-frame cost | Planned |
| 124.16 | Shaping cache instrumentation and a TUI-redraw benchmark | Planned |
| 124.C1 | `decide_frame_damage`'s doc comment describes a removed term | Planned |

### Execution model

```text
independent leaves (may run at any time, in parallel):
  124.4   124.7   124.9   124.8   124.C1

the epoch chain (strictly sequential, the spine of this task):
  124.10 -> 124.11 -> 124.12 -> 124.14 -> 124.2

measurement-then-decision pairs (each pair sequential; pairs parallel):
  124.13 -> 124.3
  124.15 -> 124.5
  124.16 -> 124.6
```

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

Scope: `freminal-buffer/src/buffer/flatten.rs`,
`freminal-buffer/src/buffer/scroll.rs`, and their tests. Do not touch
`freminal-terminal-emulator` or the GUI in this subtask.

What: add a `u64` content epoch to `RowCacheEntry` (`flatten.rs:76-104`). In
`refresh_row_cache_and_refine_wrapped_urls` (`flatten.rs:1072-1166`), where a
row cache entry is rebuilt because `row.dirty || cache[i].is_none()`, compare
the newly-built entry's content against the entry it replaces and bump the
epoch **only if it differs**. Add
`Buffer::visible_row_epochs(scroll_offset, extra_rows) -> Vec<u64>`
alongside the existing `visible_line_widths_extended` in `scroll.rs`.

The distinction is the entire point and must be preserved by whoever
implements this: **`Row::dirty` means "was written to"; the epoch means
"actually changed".** Full-screen TUIs rewrite unchanged bytes by idiom, and
that is the workload this task exists to stop charging for.

The comparison basis is the `RowCacheEntry`'s rendered content — `chars`,
`tags` and `line_width` — not the source `Row`. Anything that changes what
is drawn must bump; anything that does not, must not. Note that `tags` are
row-relative in the cache entry, so this comparison is position-independent
by construction.

Epochs are **window-relative** by design. On a scroll or a resize every
visible row genuinely does show different content, and that is correctly a
full-screen change; `scroll_changed` and `dims_changed` already force a full
rebuild for exactly that reason. Do not attempt to key epochs to row
identity across scroll — that is a strictly larger design and it buys
nothing here.

Deliverable: the epoch field, the bump-on-difference logic, the accessor,
and unit tests covering: a row rewritten with identical bytes does **not**
bump; a row rewritten with different bytes does; an SGR-only change does; a
`line_width` change does; and a clean row is untouched.

Watch the debug-mode oracle cross-check (`flatten.rs:458-469, 515-521`),
which asserts the fast path's output byte-equals a from-scratch full merge.
Any new field on the fast path must be proven consistent with that oracle.

Verification: `cargo test --all`;
`cargo clippy --all-targets --all-features -- -D warnings`; `cargo machete`.

Prohibitions: do NOT change the `Arc` allocation behaviour of
`rows_as_tchars_and_tags_incremental` — that is 124.12's business and doing
it here would confound the measurement. Do NOT add consumers. Do NOT touch
`Row::dirty`'s existing semantics or call sites.

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

Deliverable: the field, the population, the deletions, and the consequent
updates to every `content_changed` reader in the emulator crate and its
tests. Any GUI reader is 124.12's problem and this subtask may leave a
compile break there only if 124.11 and 124.12 land as one PR; if they land
separately, keep `content_changed` until 124.12 removes its last reader.

Verification: `cargo test --all`;
`cargo clippy --all-targets --all-features -- -D warnings`; `cargo machete`.

Prohibitions: do NOT change `frame_dirty.rs`. Do NOT change the snapshot's
other fields.

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
carrying 124.12's changed-row set. Compute the damage rects from the changed
rows plus the existing selection, hover and search extents, which are already
known at `widget.rs:2585-2596` as the booleans that currently force `Full`.
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
