# PLAN_VERSION_130.md — v0.13.0 "Kitty: Text Sizing"

## Goal

Implement the kitty text-sizing protocol (OSC 66): integer-scaled and fractionally-scaled
multi-cell text. This is the **highest-risk rendering item** in the entire kitty surface —
it reworks the cell-grid model to support multicell blocks, fractional font scaling within
cells, a custom character-width algorithm, and complex overwrite/wrap semantics. It is
isolated in its own version precisely so that risk cannot drag anything else off schedule.

Depends on Task 13 (the custom OpenGL renderer / glyph atlas / shaping pipeline this work
extends).

**Decomposed** per the `freminal-version-activation` skill (next-up, stable spec). The
spec is stable; the _risk_ is implementation, not protocol churn. Re-confirm the renderer
seams at activation — they are the ones most likely to have moved.

---

## Task Summary

| #   | Feature                    | Scope     | Status  | Depends On |
| --- | -------------------------- | --------- | ------- | ---------- |
| 104 | Kitty Text Sizing (OSC 66) | Very high | Planned | Task 13    |

---

## Reference spec

- Text sizing — <https://sw.kovidgoyal.net/kitty/text-sizing-protocol/>

Escape-sequence change triggers the dual-document update
(`ESCAPE_SEQUENCE_COVERAGE.md` + `ESCAPE_SEQUENCE_GAPS.md`) per
`freminal-escape-sequence-docs`.

---

## The OSC 66 collision (read before scoping anything)

**freminal already uses OSC 66.** Per `ESCAPE_SEQUENCE_COVERAGE.md`, OSC 66 is currently
treated as the Contour "ColorScheme Notification" — recognised and silently consumed,
with DECRPM `?2031` as the functional adaptive-theme path. Kitty's text-sizing protocol
_also_ uses OSC 66. These are two different protocols claiming the same OSC number.

**This ambiguity MUST be resolved before any text-sizing implementation.** The first
subtask is a code audit (104.1) that confirms what freminal's OSC 66 handler actually
does today, determines whether the Contour interpretation is load-bearing or vestigial,
and decides how OSC 66 is disambiguated (by metadata shape, or by dropping the dormant
Contour interpretation). No implementation proceeds until that decision is made and
recorded in the escape-sequence docs.

---

## Current-state map (from activation recon)

- **OSC 66 today:** recognised/silently consumed (Contour ColorScheme). Find the handler
  via `OscTarget` (`freminal-common/src/buffer_states/osc.rs`) and
  `dispatch_osc_target()` (`freminal-terminal-emulator/src/ansi_components/osc.rs`).
- **Cell grid / shaping / render (the risk surface):**
  - Shaping: `freminal/src/gui/shaping.rs` `ShapingCache::shape_visible()` →
    `ShapedLine` / `TextRun` / `ShapedGlyph`. `TextRun` already carries `char_widths`
    (1 normal, 2 wide) and a `col_start`/`col_count` model for wide glyphs.
  - Atlas: `freminal/src/gui/atlas.rs` `GlyphAtlas::get_or_insert()` (swash raster).
  - Vertices: `freminal/src/gui/renderer/vertex.rs` `build_foreground_instances()` /
    `build_background_instances()`. The renderer uses **integer cell-grid coordinates
    throughout** — fractional/sub-cell positioning requires moving to float-pixel
    coordinates in both `shaping.rs` and `vertex.rs`. Localized but real.
  - Buffer cell model: `freminal-buffer` — multicell blocks need an ownership concept so
    erase/overwrite (ICH/DCH/ECH/ED/EL/IL/DL) behave per the 7 spec rules.

---

## Task 104 — Kitty Text Sizing (OSC 66)

### 104 Summary

`ESC ] 66 ; <metadata> ; <text> ST` renders `text` at an integer scale (`s=1..7`,
occupying `s*w` columns × `s` rows) and/or a fractional scale (`n/d` with vertical/
horizontal alignment). The terminal must maintain multicell blocks, track their cell
ownership for correct overwrite semantics, apply fractional font scaling within cells,
and implement the kitty character-width algorithm (Unicode 16 grapheme segmentation,
variation selectors). Fire-and-forget (no reverse path beyond the support handshake).

### 104 Subtasks

#### 104.1 — READ-ONLY audit: resolve the OSC 66 collision

Scope: read-only across the OSC 66 handler (`OscTarget`, `dispatch_osc_target`, any
`osc_*` module handling it), `DECRPM ?2031` adaptive-theme path, and the escape-sequence
docs.

What: determine exactly what freminal does with OSC 66 today; whether the Contour
ColorScheme interpretation is reachable/used or vestigial; and how kitty text-sizing OSC
66 can be disambiguated from it (metadata shape differs — kitty uses `s=`/`w=`/`n=`/`d=`
keys; Contour's payload differs). Produce the disambiguation decision and the plan for
the parser to branch correctly (or to drop the Contour path if dormant).

Deliverable: findings + disambiguation decision (chat / task notes). No code.

Verification: none (read-only).

Prohibitions: do NOT edit files; do NOT begin implementation; do NOT proceed without
maintainer sign-off on the disambiguation decision.

Stop: report; await explicit decision before 104.2.

#### 104.2 — OSC 66 parser + disambiguation; escape-sequence docs for the resolution

Scope: the OSC 66 handler module + `freminal-common` types; `OscTarget` /
`dispatch_osc_target` if branching changes; `Documents/ESCAPE_SEQUENCE_COVERAGE.md` /
`ESCAPE_SEQUENCE_GAPS.md`.

What: implement the disambiguation from 104.1 and parse the text-sizing metadata
(`s`,`w`,`n`,`d`,`v`,`h`) into a typed `TextSizingSpec`. Record the OSC 66 resolution in
the escape-sequence docs (this is a behaviour change worth documenting even before the
renderer work).

Deliverable: parser + disambiguation + tests (text-sizing forms, Contour form routed
correctly or removed); dual-doc update of the resolution.

Verification: `cargo test --all`; clippy; markdownlint clean.

Prohibitions: no buffer/render work yet; do NOT proceed.

Stop: report + await review.

#### 104.3 — Character-width algorithm (kitty Unicode-16 grapheme model)

Scope: `freminal-common` or `freminal-buffer` (wherever character width is currently
computed), tests.

What: implement the kitty "splitting text into cells" algorithm: Unicode 16 grapheme
segmentation and variation-selector handling (U+FE0E/U+FE0F changing width). This is the
foundation the multicell model and the rest of the protocol depend on; keep it pure and
exhaustively tested against the spec's examples.

Deliverable: width algorithm + a large table-driven test suite (graphemes, VS15/VS16,
the spec's worked examples).

Verification: `cargo test --all`; clippy.

Prohibitions: no rendering yet; do NOT proceed.

Stop: report + await review.

#### 104.4 — Buffer: multicell block model + ownership + overwrite rules

Scope: `freminal-buffer` (cell model, the mutation ops ICH/DCH/ECH/ED/EL/IL/DL).

What: add the multicell-block concept (a scaled block owns `s*w` columns × `s` rows);
track ownership so the 7 spec overwrite/erase rules behave correctly; ensure all
mutations return the existing structured change description (`freminal-buffer` contract).
No rendering.

Deliverable: multicell model + tests for each of the 7 overwrite/erase interactions.

Verification: `cargo test --all`; clippy.

Prohibitions: no shaping/render changes; do NOT proceed.

Stop: report + await review.

#### 104.5 — Wrapping & overwriting behaviour at the line level

Scope: `freminal-buffer` / `freminal-terminal-emulator` line-handling for scaled blocks.

What: implement the spec's wrapping rules for multicell text (a block that doesn't fit
the line, cursor movement across blocks, editing controls interacting with blocks).

Deliverable: wrap/edit behaviour + tests (block at line end, cursor over a block, edits
spanning blocks).

Verification: `cargo test --all`; clippy.

Prohibitions: no render changes; do NOT proceed.

Stop: report + await review.

#### 104.6 — Renderer: integer→float cell coordinates + scaled-glyph rendering

Scope: `freminal/src/gui/shaping.rs`, `freminal/src/gui/renderer/vertex.rs`,
`freminal/src/gui/atlas.rs` (scaled raster keys if needed).

What: move the affected vertex path from integer cell-grid to float-pixel coordinates so
integer- and fractionally-scaled blocks render at the right size and alignment
(`v=`/`h=`). Render scaled glyphs (raster at `s × base` or scale at draw). Preserve the
existing integer path for normal text (no regression for unscaled content). Follow
`freminal-architecture` (snapshot transport; renderer is a pure read).

Deliverable: scaled rendering + tests (vertex geometry for s=2, fractional n/d with each
alignment); a visual/integration assertion at the achievable level.

Verification: `cargo test --all`; clippy; **mandatory** before/after benchmark capture on
the shaping + `build_snapshot` + render hot paths per `performance-benchmarks` +
`freminal-bench-table` (this is the most performance-sensitive change in the kitty work;
the 15% regression threshold applies).

Prohibitions: do NOT regress the unscaled-text fast path; do NOT proceed.

Stop: report + await review (include the benchmark table).

#### 104.7 — Support-detection handshake

Scope: the OSC 66 handler + reverse-write helper.

What: implement the detection sequence (the spec's CPR-based width/scale probe): respond
so a client probing with `w=2` and `s=2` between CPR queries sees the correct cursor
advancement that signals width-only vs full-scale support — truthfully reflecting what is
implemented.

Deliverable: handshake support + tests asserting the probe responses.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT advertise full scale if only width is implemented; do NOT proceed.

Stop: report + await review.

#### 104.8 — Final escape-sequence docs

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`.

What: mark OSC 66 text sizing implemented with the supported-capability summary (integer
scale, fractional, alignments); refresh the "Last updated" header; ensure the
Contour-vs-kitty resolution from 104.2 is reflected in the final state.

Deliverable: dual-doc update.

Verification: markdownlint clean.

Prohibitions: none beyond scope.

Stop: report + await review.

### 104 Open questions (resolve at activation)

- The integer→float renderer coordinate change (104.6): a localized branch for scaled
  blocks only, or a broader coordinate-system change? The recon says localized; confirm
  against the then-current `vertex.rs` before committing to scope.
- Scaled-glyph rasterisation: raster at the scaled size (atlas key includes scale) vs
  raster once and scale the quad. Quality vs atlas-size tradeoff — decide at activation.
- If 104.4/104.5 reveal the multicell model is larger than estimated, stop and re-scope
  with the maintainer (this version is intentionally a single task so it can slip alone).

---

## Design Decisions (provisional, confirm at activation)

- **Text sizing is isolated by design.** It is the single highest-risk rendering item in
  the kitty surface; giving it its own version means if it slips, nothing else slips with
  it. Do not add unrelated work to v0.13.0.
- **The OSC 66 collision is resolved before implementation.** The Contour ColorScheme
  interpretation and kitty text-sizing share OSC 66; 104.1 resolves the ambiguity and the
  resolution is documented before the renderer work begins. Never silently assume one
  wins.
- **The unscaled fast path must not regress.** Normal (scale-1) text keeps the existing
  integer-cell path; the float-coordinate work is additive for scaled blocks. The
  mandatory benchmark capture in 104.6 guards this.
- **Character width is foundational and pure.** The Unicode-16 grapheme/width algorithm
  (104.3) is built and tested in isolation before anything depends on it.
