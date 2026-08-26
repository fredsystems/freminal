---
name: freminal-damage-model
description: Use ONLY when working in the freminal repository AND adding or changing anything that writes or derives `PaneFrameDamage`, `FrameDamage`, `ChromeDamage`, any `*_changed` signal feeding them, `PresentRegion`, `DamageHistory`, or clear/clip/present-region/full-repaint logic. Codifies full-surface repaint as a last resort, the mandatory GLOBAL / BOUNDABLE-NOW / BOUNDABLE-WITH-WORK trigger classification, the eight genuinely-global categories from the 124.21 audit, the monotonic-toward-`Full` precedence chain, the multi-pane fan-out hazard, and the one-region-for-clip-clear-and-present invariant.
---

# Freminal: the damage model -- full repaint is a last resort

Task 124 replaced a boolean dirty signal with a typed damage model
(`PaneFrameDamage` -> `FrameDamage` -> `ChromeDamage` composition ->
windowing's `PresentRegion`/`DamageHistory`). This skill exists so later
work does not silently re-add unbounded damage, or worse, add a *bounded*
path whose extent is not actually complete -- which is silent visual
corruption, not a missed optimisation.

## 1. Core rule

Full-surface repaint is a last resort, not a default. Before writing a new
damage trigger, or changing what an existing one reports, state one
classification:

- **GLOBAL** -- pixels may change anywhere on the surface and no complete
  smaller extent exists.
- **BOUNDABLE-NOW** -- the complete old and new extents already exist at the
  decision point; bounding costs only wiring.
- **BOUNDABLE-WITH-WORK** -- the extent is knowable in principle but is not
  yet computed, or is not reachable from the decision point today.

**A current `Full` fallback is not proof a trigger is GLOBAL.** GLOBAL is
the safe *runtime* fallback when complete bounds cannot be proven; it is
not a license to skip design review. Several triggers that currently force
`Full` are `BOUNDABLE-WITH-WORK` and are only unbounded because nobody has
built the geometry yet (124.21's audit lists them explicitly). Call a
trigger `BOUNDABLE-WITH-WORK` when that is true, rather than canonising
today's technical debt as `GLOBAL`.

## 2. The eight genuinely-global categories (124.21 audit)

124.21 exhaustively audited 52 full-repaint-forcing triggers. Only these
eight are genuinely global -- every other trigger is bounded work currently
paid at full-surface cost:

| Category | Why no bounded region suffices |
| -------- | ------------------------------- |
| Terminal `theme_changed` | colours are baked per-vertex; every drawn cell is stale |
| `dims_changed` | a resize reflows and re-shapes every row |
| `ChangedRows::All` | no per-row epoch baseline exists to diff against |
| Empty prior decoration/vertex state (`deco_verts.is_empty()`) | no previous rebuild ever populated a reusable buffer |
| Degenerate/unresolvable cursor damage (`CursorOnly(None)`) | the cursor rect did not resolve to a valid bound after clamping |
| `shader_recomposites` | a post-process shader rewrites the whole framebuffer |
| Chrome `style_changed` / `size_changed` / `ppp_changed` (one grouped category) | chrome-layer geometry/style invalidates the whole chrome surface |
| Windowing unsupported partial present, or `buffer_age() == 0` / unreconstructable history | back-buffer contents are unknown |

These are **categories**, not an exhaustive list of every trigger that is
currently `Full`. Do not read this table as "everything else must also be
`Full`". Bell flashes, toasts, foreground overlays, fold changes, image
frame/pixel changes, and text-blink are examples the audit classified
`BOUNDABLE-WITH-WORK` -- they force `Full` today only because nobody has
built or wired the bounding geometry, not because their extent is
inherently unknowable. Do not cite this table to justify leaving one of
those unbounded forever.

## 3. Shipped type flow and precedence

```text
per pane:        PaneFrameDamage::{Unchanged, CursorOnly(Option<rect>), Region(rects), Full}
                            |
terminal-wide:    decide_frame_damage()  ->  FrameDamage::{None, Partial(rects), Full}
                            |
chrome compose:   compose_with_chrome_damage()  ->  final FrameDamage
                            |
windowing:        DamageHistory / buffer_age()  ->  PresentRegion::{Full, Region(rect)}
```

- **Per-pane** (`freminal/src/gui/renderer/mod.rs`): `PaneFrameDamage`.
  `Region(Vec<PaneDamageRect>)` is never empty by construction -- an empty
  rect set is reported as `Full` instead, so `Region` and "nothing
  changed" never collide.
- **Terminal aggregation** (`freminal/src/gui/frame_damage.rs`,
  `decide_frame_damage`): produces `FrameDamage`. If the loop over panes
  finishes with **no rects collected at all** -- every pane was
  `Unchanged` and no pane contributed search-overlay rects -- the result
  is `FrameDamage::None`, not `Full`. There is no rect-based short-circuit
  to `None`; it is purely the empty case.
- **`compose_with_chrome_damage` runs after that**, and is not optional:
  `ChromeDamage::Changed` unconditionally upgrades any `None` or `Partial`
  decision to `Full`. Damage is **not final** before this call runs --
  never read a terminal-side `FrameDamage` as the frame's true damage.
- **Precedence is monotonic toward `Full`.** Every stage may only make
  damage coarser than what it received; no later stage may downgrade
  `Full` back to `Partial` or `None`.
- **Windowing** (`freminal-windowing/src/frame_paint.rs`) then applies
  `DamageHistory` against `buffer_age()`: an age-`n` partial redraw is the
  union of this frame's damage with the previous `n - 1` presented
  frames' damage, and is published as `PresentRegion` -- a single bounding
  box today (multi-rect scissoring is a measured-later optimisation, not
  yet built).
- **`FrameDamage::None` is never inserted into `DamageHistory`.** A `None`
  frame does not swap, so there is nothing to reconstruct history from --
  do not add a `push` call for it.

## 4. Multi-pane fan-out

Inside `decide_frame_damage`'s per-pane loop, **any** of the following
short-circuits and discards every rect already collected from other
panes, forcing the whole window `Full`:

- a pane reporting `Full`,
- a pane reporting `CursorOnly(None)` (degenerate cursor),
- a bell flash active in any pane.

An unresolved pane (one present in the layout but not found in the pane
tree) is **not** represented as an item inside this loop. The caller does
the pane-tree lookup, and an unresolved pane there means the caller never
builds a full `per_pane_damage` list for this frame at all -- it instead
calls `decide_frame_damage` with `force_full = true`, which short-circuits
before the loop even runs. This is a lossless simplification of the call
site, not a behavior change: an unresolved pane forces `Full` either way.

124.14c fixed the common case where a busy-but-boundable pane forced every
idle sibling to a full clear + present: that pane now reports its own
`Region`/`CursorOnly` rect instead of `Full`. **Do not reintroduce
`PaneFrameDamage::Full` for a pane-local change that has a provable
bound** -- that regresses 124.14c.

## 5. One-region invariant (124.20 / 124.23)

The exact `PresentRegion` windowing publishes governs **all** of: the
scissored clear, the clip, the draw, and `swap_buffers_with_damage`. Both
the cursor-only paint arm and the full-draw paint arm read the same
`PresentRegion` and scissor to it (124.23 closed the gap where only the
cursor-only arm did).

**Never derive a second, app-local scissor from this frame's own declared
damage.** The published region can be *wider* than this frame's damage --
`DamageHistory`'s buffer-age union means a stale back buffer requires
redrawing more than what changed just now -- so a callback that scissors to
its own narrower rect can leave pixels the windowing layer already decided
must change. If clip, clear, draw, and present ever disagree about the
region, that is the issue #432 silent-corruption class, not a performance
bug.

## 6. Complete-bound rule

Bounded damage must cover the union of **old and new** extents -- the
pixels a decoration occupied last frame **and** the pixels it occupies
this frame. This applies to selection, hover, search highlights, the
search popup, the cursor, and any moving or shrinking overlay. Reporting
only the new extent leaves the old pixels stale on screen.

If you cannot prove the extent is complete, use the `Full` runtime
fallback and classify the trigger `BOUNDABLE-WITH-WORK` in review rather
than shipping a partial bound. **Under-reporting is silent corruption;
over-reporting is only wasted work.** The safe direction when in doubt is
always to report more, never less.

## 7. Coordinate and representation constraints

- Damage rects (`DamageRect`, `PaneDamageRect`) are **physical framebuffer
  pixels, bottom-left origin** -- the `glScissor` / `eglSwapBuffersWithDamage`
  convention. Reuse the sanctioned transform
  (`PaneDamageRect::from_cursor_cells`, which already does the Y-flip,
  outward rounding, safety pad, and framebuffer clamp) rather than
  hand-rolling a second one. `renderer/mod.rs` and `frame_dirty.rs`
  document at least three *other* coordinate spaces in play
  (snapshot/flattened-window row space, rendered-row space post-fold, and
  buffer-absolute row space) -- verify which space an input is in before
  unioning it with `changed_rows`.
- Follow `freminal-state-representation`: damage state is a named domain
  enum (`PaneFrameDamage`, `FrameDamage`, `PresentRegion`), never a bare
  bool or an `Option<Vec<DamageRect>>` threaded positionally.
- `DefaultBackground` means "leave these pixels untouched", not "draw the
  clear colour". Any partial-present path must preserve the scissored
  clear underneath transparent or default-background content -- a
  call-count test cannot catch a violation of this; only the pixel
  harness can.

## 8. Boundary with Task 125 -- do not cross it here

`PaneFrameDamage::Region` / `VertexRebuild::Bounded` bound the **present**
(clear, draw, present), not the **upload**. A `Bounded` frame still runs a
full vertex rebuild and `upload_verts` still writes the whole instance
buffer, because none of `bg_instances`, `fg_instances`, or `deco_verts`
has a fixed stride per row. Do not claim per-row GPU upload, do not change
the vertex emission format, and do not touch `upload_verts` from a damage
change -- that is Task 125, gated on this task's own measurements.

## 9. Review checklist

Before landing a damage-model change, confirm:

- [ ] The new/changed trigger states its classification (GLOBAL /
      BOUNDABLE-NOW / BOUNDABLE-WITH-WORK) and, if GLOBAL, says why no
      smaller complete extent exists.
- [ ] Bounded extents include both old and new pixels.
- [ ] Every coordinate space involved has been identified and no space is
      assumed without checking.
- [ ] Multi-pane behaviour is correct: a boundable pane-local change does
      not force siblings to `Full`, and a `Full`/degenerate/bell pane
      still forces the whole window per section 4.
- [ ] Chrome composition is accounted for: the change is verified against
      the *composed* `FrameDamage`, not the pre-`compose_with_chrome_damage`
      value.
- [ ] Windowing's `DamageHistory` / buffer-age interaction is considered if
      the change touches presentation, not just decision.
- [ ] Clip, clear, draw, and present all resolve to the exact same region.
- [ ] `DefaultBackground` / transparency is preserved under the change.
- [ ] `FrameDamage::None` semantics are respected: no swap, no
      `DamageHistory` entry.
- [ ] Unit tests cover the new decision/aggregation case; a pixel-harness
      test covers anything that changes what is drawn or presented (per
      `performance-benchmarks` and `freminal-bench-table` for anything
      perf-relevant).

## 10. When to stop and ask

- You are about to classify a new trigger GLOBAL without being able to
  state why no smaller complete extent exists.
- The bound you need crosses the terminal/chrome ownership split (as
  search did in 124.14d) and the plan you are working from did not
  already authorize that.
- You find yourself writing a second coordinate transform instead of
  reusing `PaneDamageRect::from_cursor_cells`.
- You cannot prove the old extent is complete, but you're tempted to ship
  the bound anyway "since it's probably fine".
- You are about to weaken the `Full` fallback (e.g. remove a short-circuit
  in `decide_frame_damage`, or the `age == 0` fallback in windowing).
- You are about to touch Task 125's territory: `upload_verts`, the vertex
  emission format, or per-row GPU upload.
- The pixel harness cannot reach the presentation change you are making
  (as 124.23 recorded for its own full-draw-arm fix) -- say so rather than
  claiming coverage you don't have.
- Any ambiguity about what an escape sequence or terminal mode "should"
  do reaches this work -- that is a terminal-semantics stop per `agents.md`,
  not a damage-model question.
