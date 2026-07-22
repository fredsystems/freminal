# egui Stack Upgrade Assumptions

**Status:** Authoritative reference. Update this file whenever an assumption
below is added, removed, or changed — it is a living checklist, not a
work log.

## Why this file exists

The chrome-caching work (issues #435 and #436) makes the GUI skip re-recording,
re-tessellating, and re-painting the "chrome" (menu bar, tab bar, borders,
overlays) on frames where the chrome provably did not change, while always
freshly rebuilding the terminal band. To do this correctly it relies on a
number of **undocumented, internal behaviours** of the egui stack
(`egui`, `epaint`, `egui_glow`, `egui-winit`) at version **0.35.0**.

These behaviours are not part of any crate's public API contract. A version
bump — even a patch bump — could silently change one of them. The failure mode
is nasty: **no compile error and no headless test failure** (the repo has no
headless-GL harness), just wrong pixels, ghosted chrome, or stale overlays at
runtime.

To contain that risk:

- `egui`, `egui_glow`, and `egui-winit` are **exact-pinned** (`=0.35.0`) in the
  workspace `Cargo.toml`, as a matched set. This is deliberate — do not "clean
  it up" to a caret range.
- Renovate is configured so the "egui + windowing stack" group **never
  auto-merges** and requires explicit Dependency-Dashboard approval before a
  bump PR is even opened (see `renovate.json`). Dependabot cargo updates are
  disabled (`open-pull-requests-limit: 0`).
- **Before bumping any crate in the egui stack, walk every row of the table
  below** against the new version's source, and run the pixel-level
  verification (the 436.9 pixel harness, once it exists — until then, a manual
  visual smoke of the scenarios in the "Symptom if broken" column is
  mandatory).

If any assumption no longer holds in the new version, the chrome-cache code
must be adapted (and this file updated) before the bump can land.

## How to use this on a bump

1. Read the new version's source for each `Upstream (0.35.0)` location below
   and confirm the behaviour still holds. Line numbers will drift between
   versions; find the equivalent code, do not trust the line number blindly.
2. For any assumption that changed, fix the corresponding `Our code` site and
   update this table.
3. Run `cargo test --all` (catches the headless-verifiable subset — callback
   ordering, atlas-growth detection logic, the FULL/REPLAY decision, the
   damage composition).
4. Run the pixel-level verification for every "Symptom if broken" scenario. A
   green `cargo test` is **not** sufficient — the load-bearing failures are
   pixel-only.
5. Only then update the `=0.35.0` pins.

## The assumptions

Each row: what we rely on, where our code depends on it, the upstream source
that proves it in 0.35.0, and what breaks (visibly) if a bump invalidates it.

### A1 — `paint_primitives` fully re-establishes GL state per call (except FBO)

We split one frame's paint into three sequential `paint_primitives` calls
(head, band, tail). This is only equivalent to one call because
`prepare_painting` re-establishes every piece of managed GL state
(scissor, blend, viewport, program, VAO, EBO, texture unit) at the _start_ of
every call, and again after every callback. The one thing it does **not** reset
is the bound framebuffer — the terminal band's own callbacks are responsible
for restoring that.

- **Our code:** `freminal-windowing/src/egui_integration.rs` (the three
  `paint_primitives` calls in `run_frame`, head/band/tail).
- **Upstream (0.35.0):** `egui_glow/src/painter.rs` — `prepare_painting`
  (`~300-349`), called at `paint_primitives` entry (`~405`) and after each
  callback (`~450`). No `bind_framebuffer` anywhere in `prepare_painting`.
- **Symptom if broken:** garbled / mis-clipped / wrongly-blended chrome or
  terminal content; state from the band leaking into chrome paint (or vice
  versa). Multi-pane with an active post-process shader is the sharpest test.

### A2 — `paint_and_update_textures` is set-all then paint then free-all

We do not call `paint_and_update_textures`; we hand-inline its three phases
(upload every `textures_delta.set`, then our three `paint_primitives` calls,
then free every `textures_delta.free`) so the band can be painted separately.
This is only correct if that is exactly what the upstream method does.

- **Our code:** `freminal-windowing/src/egui_integration.rs` (the
  `set_texture` loop, three paints, `free_texture` loop in `run_frame`).
- **Upstream (0.35.0):** `egui_glow/src/painter.rs` —
  `paint_and_update_textures` (`~356-374`).
- **Symptom if broken:** a texture referenced by a primitive is freed too
  early or uploaded too late — missing glyphs, blank text, or a use-after-free
  crash in the GL layer.

### A3 — the background layer drains first and contiguously into `FullOutput.shapes`

The terminal band is captured as a contiguous index range within
`LayerId::background()`'s `PaintList`. We then slice `full_output.shapes` by
that same range. This is valid only because `Order::Background` is drained
first and whole into `full_output.shapes`, AND because freminal never creates a
second `Order::Background` layer (an application-level invariant, not enforced
by egui).

- **Our code:** `freminal/src/gui/app_impl.rs` (`band_shape_start` /
  `band_shape_end` capture); `freminal-windowing/src/egui_integration.rs`
  (the head/band/tail slice of `full_output.shapes`).
- **Upstream (0.35.0):** `egui/src/layers.rs` — `enum Order` (`Background`
  first), `Order::ALL`, `GraphicLayers::drain` (`~213-260`, iterates
  `Order::ALL`, appends into one `Vec`).
- **Symptom if broken:** the band paints at the wrong z-position (under chrome,
  or over overlays), or the sliced range picks up chrome/overlay shapes.
  Guarded partly by the `dedicated_background_layer_hides_contained_widget...`
  regression test.

### A4 — the tessellator never merges callbacks and preserves their order

Each `Shape::Callback` becomes its own `Primitive::Callback`, never fused into
an adjacent mesh, in input order. This keeps the band's pre-clear /
per-pane / post-shader GL callbacks contiguous and correctly ordered across the
split. The parallel (rayon) tessellation path both excludes callbacks and is
not compiled (rayon is not in the dependency graph — only `criterion`'s dev
dependency pulls a `rayon` we never build against).

- **Our code:** `freminal/src/gui/app_impl.rs` (pre-clear, post-shader
  callbacks), `freminal/src/gui/terminal/widget.rs` (per-pane callback); test
  `band_gl_callbacks_stay_contiguous_and_ordered_across_the_split` in
  `freminal-windowing/src/egui_integration.rs`.
- **Upstream (0.35.0):** `epaint/src/tessellator.rs` —
  `tessellate_clipped_shape` callback branch (`~1375-1394`), the
  `Primitive::Callback(_) => true` merge-break, the sequential
  `tessellate_shapes` loop (`~2230`), and the rayon `should_parallelize`
  callback exclusion (`~2287`, `#[cfg(feature = "rayon")]`).
- **Symptom if broken:** the offscreen-FBO round-trip (pre-clear then panes
  then post-shader) is reordered or split — shader post-processing renders
  wrong, or panes draw into the wrong target.

### A5 — glyph UVs are normalized by the live atlas size at tessellate time

`ctx.tessellate` normalizes each glyph's UV by the font atlas size current at
the moment of the call, and bakes that into the mesh vertex. A mesh tessellated
before the atlas grows therefore holds stale UVs. This is the entire premise of
the atlas-resize self-heal (A6).

- **Our code:** `freminal-windowing/src/egui_integration.rs` (the REPLAY
  atlas-grow self-heal re-tessellating cached chrome shapes); test
  `atlas_growth_invalidates_cached_text_uvs_and_retessellation_fixes_it`.
- **Upstream (0.35.0):** `egui/src/context.rs` — `tessellate` (`~2757-2795`,
  reads `texture_atlas.size()` fresh, builds a new `Tessellator`);
  `epaint/src/tessellator.rs` — `tessellate_text` UV normalization by
  `font_tex_size` (`~2029-2030`).
- **Symptom if broken:** cached chrome text shows garbled / wrong glyphs after
  a new glyph or larger font grows the atlas mid-session.

### A6 — atlas growth always yields a whole-upload delta on the font atlas

Detecting "cached chrome UVs may be stale" is done by watching for a
whole-texture upload (`ImageDelta::is_whole()` / `pos: None`) on
`TextureId::default()`. This is sound _and complete_ only because the atlas
never relocates already-allocated glyphs and only marks the whole image dirty
on a resize/recreate — so there is no way for existing UVs to become stale
without a whole-upload delta.

- **Our code:** `freminal-windowing/src/egui_integration.rs` — the
  `atlas_grew` helper.
- **Upstream (0.35.0):** `epaint/src/texture_atlas.rs` — `allocate`
  (`~220-263`, forward-only cursor, never repositions), `resize_to_min_height`
  (sets `dirty = Rectu::EVERYTHING`), `take_delta` (turns `EVERYTHING` into
  `ImageDelta::full`); `epaint/src/image.rs` — `ImageDelta::is_whole`
  (`~493-495`), `ImageDelta::full` sets `pos: None` (`~474-480`).
- **Symptom if broken:** if a future atlas repacks/relocates glyphs with a
  _partial_ delta, `atlas_grew` misses it and cached chrome text garbles with
  no self-heal. This is the most dangerous assumption to lose — re-read
  `allocate` carefully on any bump.

### A7 — `TextureId::default()` is the font atlas for the Context's lifetime

`TextureId::default() == TextureId::Managed(0)` is the font atlas, allocated
first and stable for the whole `Context` lifetime. A6's detector keys on it.

- **Our code:** `freminal-windowing/src/egui_integration.rs` — `atlas_grew`.
- **Upstream (0.35.0):** `egui/src/context.rs` —
  `WrappedTextureManager::default` (`~73-91`, allocates the font texture first
  with an `assert_eq!(font_id, TextureId::default())`);
  `epaint/src/textures.rs` — `TextureManager::alloc` doc (`~24-28`).
- **Symptom if broken:** `atlas_grew` watches the wrong texture; either never
  self-heals (garbled chrome text) or self-heals spuriously (wasted work).

### A8 — `ctx.graphics()` is readable mid-frame and reflects the live layers

We read the background layer's shape count mid-`update` (inside the `run_ui`
closure) to bracket the band range. This works because `ctx.graphics()` /
`graphics_mut()` operate on the live per-viewport `GraphicLayers` that
`end_pass` later drains.

- **Our code:** `freminal/src/gui/app_impl.rs` — `band_shape_start` /
  `band_shape_end` captures via `ctx.graphics(...)`.
- **Upstream (0.35.0):** `egui/src/context.rs` — `graphics` / `graphics_mut`
  (`~971-981`), and `end_pass` draining `viewport.graphics` (`~2617-2619`).
- **Symptom if broken:** the band range is captured against the wrong or an
  empty layer set — the band is mis-sliced (blank terminal or duplicated
  chrome).

### A9 — child Uis inherit the parent painter's layer; top-level Ui defaults to background

The REPLAY path builds the band's `Ui` directly at a cached rect; the FULL path
uses the `CentralPanel`'s child `Ui`. Both must land in `LayerId::background()`.
A top-level `Ui::new` with no explicit layer defaults to `background()`, and
`new_child` / `scope_builder` inherit the parent painter's layer unless
overridden.

- **Our code:** `freminal/src/gui/app_impl.rs` — the root Ui construction and
  the REPLAY `band_ui` construction (explicit `.layer_id(background())`).
- **Upstream (0.35.0):** `egui/src/ui.rs` — `Ui::new`
  (`layer_id.unwrap_or_else(LayerId::background)`, `~124`), `Ui::new_child`
  (`~224-231`, clones parent painter, only overrides layer if the builder
  supplies one).
- **Symptom if broken:** REPLAY-frame band shapes land in a different layer
  than FULL-frame band shapes — z-order / hit-test divergence between the two
  frame kinds.

### A10 — `repaint_delay` is one shared per-viewport field

The FULL/REPLAY decision cannot use a literal `repaint_delay == Duration::MAX`
gate, because `ctx.request_repaint_after` (which freminal calls every frame for
cursor blink) writes the _same_ per-viewport `repaint_delay` field that
egui-internal animations do. So we gate on `repaint_delay >=
terminal_requested_delay` (our own request) instead — i.e. "nothing other than
our own blink/content scheduling wants an earlier wake."

- **Our code:** `freminal-windowing/src/egui_integration.rs` —
  `chrome_repaint_settled`; `freminal/src/gui/app_impl.rs` —
  `shortest_repaint_delay` / `request_repaint_after` /
  `take_terminal_requested_delay`.
- **Upstream (0.35.0):** `egui/src/context.rs` — `request_repaint_after`
  effect writes `viewport.repaint.repaint_delay` (`~158-159`); the same field
  is read into `ViewportOutput.repaint_delay` (`~2719`); egui-internal
  scheduling writes it too (`~111`, `~113`).
- **Symptom if broken:** either REPLAY never fires while the cursor blinks
  (the optimization does nothing on the headline idle case), or REPLAY fires
  when an egui chrome animation still wants to run (stale chrome). If the field
  semantics change, revisit the whole `chrome_repaint_settled` gate.

### A11 — cross-layer hit-test "hidden" rule forces the band into the background layer

egui hides a widget from hover/click/drag if a _later_ widget on a
_different layer_ fully contains its rect. This is exactly why the terminal band
must stay in the shared background layer rather than a dedicated one: the
`CentralPanel`'s content-area widget (background layer) contains every band
widget, and a dedicated band layer would let hash-ordered layer tie-breaking
hide the gutter-hover / interaction widgets.

- **Our code:** `freminal/src/gui/app_impl.rs` — the 436.2a index-range
  approach (band stays in background layer); regression tests
  `same_layer_widget_is_not_hidden_by_containing_widget` and
  `dedicated_background_layer_hides_contained_widget_cross_layer`.
- **Upstream (0.35.0):** `egui/src/hit_test.rs` — the hidden-rule loop
  (`~143-151`: `contains_rect(...) && current.layer_id != next.layer_id =>
  hidden.insert(...)`).
- **Symptom if broken:** if the rule is removed or changed, the "band must stay
  in background layer" constraint may relax — but do not rely on that; the
  regression tests pin the current behaviour. If they fail on a bump, the
  band-layer strategy needs re-evaluation.

### A12 — egui-winit flags a repaint for scale-factor (and similar) events

The FULL/REPLAY input gate forces FULL on chrome-affecting input. It relies in
part on `egui-winit` returning `EventResponse { repaint: true }` for events like
`ScaleFactorChanged`, so a DPI change forces FULL via the `response.repaint`
branch even though `is_potential_chrome_input` does not enumerate every such
event.

- **Our code:** `freminal-windowing/src/event_loop.rs` —
  `is_potential_chrome_input` plus the `|| response.repaint` OR at the general
  event arm.
- **Upstream (0.35.0):** `egui-winit/src/lib.rs` —
  `WindowEvent::ScaleFactorChanged { .. }` arm returning `EventResponse {
  repaint: true, .. }` (`~311-324`).
- **Symptom if broken:** a DPI / scale-factor change is not forced FULL — the
  chrome renders at the wrong scale on a REPLAY frame until an unrelated FULL
  frame fixes it.

## What is NOT covered here

Behaviours that _are_ part of a stable public API (e.g. `Context::run_ui`,
`Context::tessellate` signatures, `PaintCallback` shape) are not listed — a
version bump that changes those is a compile error and needs no special
checklist. This file is only for the silent, undocumented, internal behaviours.
