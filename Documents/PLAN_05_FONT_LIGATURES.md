# PLAN_05 — Font Ligatures (OpenType)

## Overview

Add OpenType ligature support, building on the text shaping infrastructure introduced by Task 1
(Glyph Atlas + Custom Painter). This task enables ligature features in the rustybuzz shaping
calls and handles the terminal-specific challenges of ligature rendering.

**Dependencies:** Task 1 (Glyph Atlas + Custom Painter) — requires shaping infrastructure
**Dependents:** None
**Primary crate:** `freminal` (GUI binary)
**Estimated scope:** Medium

---

## Problem Statement

Programming fonts like JetBrains Mono, Fira Code, Cascadia Code, and the bundled MesloLGS Nerd
Font contain OpenType ligature features that combine sequences of characters (e.g., `->`, `=>`,
`!=`, `>=`, `<=`, `===`) into single visually connected glyphs. These ligatures are valued by
developers for improved readability.

Freminal currently has no text shaping at all — each character is rendered independently. Task 1
introduces rustybuzz for text shaping but initially disables ligature features. This task enables
them with proper terminal semantics.

### Terminal-Specific Ligature Challenges

Ligatures in terminals differ from general text rendering:

1. **Cell-grid alignment** — Ligature glyphs must span exactly N cells where N is the number of
   source characters. A `->` ligature must span exactly 2 cells.

2. **Cursor positioning** — The cursor must be able to sit on any character within a ligature.
   Moving the cursor to the `-` in `->` must work correctly.

3. **Partial selection** — Users can select part of a ligature. The `>` in `->` might be selected
   while `-` is not.

4. **Mid-ligature color changes** — If `-` is red and `>` is blue in `->`, some terminals break
   the ligature, others render it in the first color. We should break the ligature (correctness
   over aesthetics).

5. **Line boundaries** — Ligatures must not span across line breaks or wrapped lines.

6. **Config toggle** — Users should be able to disable ligatures (some prefer them off).

---

## Architecture Design

### Shaping with Ligatures (Building on Task 1)

Task 1 introduces `shape_run()` in `freminal/src/gui/shaping.rs`. This task modifies the shaping
calls to enable ligature features:

```text
shape_run(face, text, features=[]) → ShapedRun  (Task 1: no ligatures)
    ↓
shape_run(face, text, features=["liga", "clig"]) → ShapedRun  (Task 5: with ligatures)
```

### Ligature-Aware Run Segmentation

Before shaping, runs must be segmented to respect ligature boundaries:

```text
Input: "fn main() -> Result<(), Error>"
         ▲         ▲▲  ▲
         │         ││  └── different color? break run
         │         │└── ligature candidate: ->
         │         └── ligature candidate: ()
         └── same format, same line = one run

Runs:
  1. "fn main() -> Result<(), Error>"  (if all same format)
  OR
  1. "fn main() "   (regular)
  2. "->"            (ligature candidate, same color)
  3. " Result<(), Error>"  (regular)
```

Key insight: We don't need to identify ligature candidates ourselves — rustybuzz handles that.
We just need to:

1. Not break runs in the middle of potential ligatures (same format = same run)
2. Break runs when format changes (color, bold, italic)
3. Break runs at line boundaries

### Ligature Glyph Rendering

When rustybuzz produces a ligature glyph that spans N input characters:

- The ligature glyph is rasterized at N× cell width in the atlas
- It occupies N cells in the mesh
- The N-1 "continuation" cells get no glyph (similar to wide character handling)

---

## Subtasks

### 5.1 — Add ligature configuration option

- **Status:** Complete (commit 7b5527e)
- **Scope:** `freminal-common/src/config.rs`, `config_example.toml`
- **Details:**
  - Add to FontConfig: `ligatures: bool` (default: `true`)
  - Add to `config_example.toml`:

    ```toml
    [font]
    family = "CaskaydiaCove Nerd Font"
    size = 12.0
    ligatures = true   # Enable OpenType ligatures (liga, clig)
    ```

  - Update Nix home-manager module option (if Task 4 is complete)

- **Acceptance criteria:**
  - Config option loads and defaults to `true`
  - Setting to `false` disables ligatures
- **Tests required:**
  - Config deserialization with ligatures = true/false
  - Default value is true
  - Missing field defaults to true (backward compat)

### 5.2 — Enable ligature OpenType features in shaping

- **Status:** Complete (commit 2cc3fd4)
- **Scope:** `freminal/src/gui/shaping.rs`
- **Details:**
  - When ligatures are enabled, pass OpenType features to rustybuzz:
    - `liga` — Standard ligatures
    - `clig` — Contextual ligatures
  - When disabled, explicitly disable these features (some fonts enable them by default)
  - The `shape_run()` function from Task 1 already accepts a features parameter
  - Feature tags: use `rustybuzz::Feature` with tag bytes
- **Acceptance criteria:**
  - Shaping with ligatures enabled produces ligature glyphs for known sequences
  - Shaping with ligatures disabled produces individual glyphs
- **Tests required:**
  - Shape `->` with ligatures enabled → produces single glyph (if font supports it)
  - Shape `->` with ligatures disabled → produces two glyphs
  - Shape `!=` with ligatures enabled → produces single glyph
  - Font without ligatures → no change regardless of setting

### 5.3 — Handle ligature glyph metrics and positioning

- **Status:** Complete (commit d0e3c4e)
- **Scope:** `freminal/src/gui/shaping.rs`, `freminal/src/gui/atlas.rs`
- **Details:**
  - When rustybuzz produces a ligature glyph spanning N characters:
    - `ShapedRun` must record that the glyph spans N cells
    - Atlas must rasterize the glyph at N× cell width
    - `GlyphKey` must distinguish ligature glyphs from regular glyphs (different size)
  - Cluster mapping: rustybuzz cluster values indicate which input characters map to which glyphs.
    Use this to determine ligature spans.
  - Handle advance widths: ligature glyph advance should equal N × cell_width
- **Acceptance criteria:**
  - Ligature glyphs render at correct width (spanning N cells)
  - No overlap with adjacent characters
  - Atlas correctly caches different-width variants
- **Tests required:**
  - 2-char ligature spans exactly 2 cells
  - 3-char ligature spans exactly 3 cells
  - Ligature advance width equals N × cell_width
  - Atlas caches ligature glyphs separately from regular glyphs

### 5.4 — Handle cursor within ligatures

- **Status:** Complete — no changes needed (commit d0e3c4e)
- **Scope:** `freminal/src/gui/terminal.rs` (cursor rendering)
- **Details:**
  - Cursor must be drawable at any character position within a ligature
  - For block cursor: highlight the single cell under the cursor, not the entire ligature
  - For bar cursor: draw bar at the leading edge of the cell under the cursor
  - For underline cursor: draw underline under the single cell
  - The ligature glyph is still rendered whole — cursor overlays on top
- **Acceptance criteria:**
  - Cursor displays correctly at any position within a ligature
  - Block cursor covers exactly one cell
  - Moving cursor through a ligature updates position correctly
- **Tests required:**
  - Cursor position calculation within ligature
  - All cursor shapes render correctly within ligature

### 5.5 — Handle selection within ligatures

- **Status:** Complete — no changes needed (commit d0e3c4e)
- **Scope:** `freminal/src/gui/terminal.rs` (selection rendering)
- **Details:**
  - Selection highlighting must work at character granularity within ligatures
  - Selected characters within a ligature get selection background color
  - The ligature glyph is rendered whole, with per-cell background colors
  - This naturally falls out of the background mesh being per-cell (from Task 1)
- **Acceptance criteria:**
  - Partial selection within a ligature displays correctly
  - Copy of partial selection produces correct characters
- **Tests required:**
  - Selection of subset of ligature characters

### 5.6 — Handle ligature-breaking conditions

- **Status:** Complete (commit d0e3c4e)
- **Scope:** `freminal/src/gui/shaping.rs`
- **Details:**
  - Ligatures must NOT form across:
    - Line boundaries (including soft wraps)
    - Color changes (different foreground color)
    - Style changes (bold/italic/underline transitions)
    - Different background colors (debatable — but safer to break)
  - Implementation: run segmentation already breaks on format changes (Task 1).
    Verify that this correctly prevents cross-boundary ligatures.
  - Add explicit line-boundary breaks in run segmentation
- **Acceptance criteria:**
  - `->` with `-` red and `>` blue renders as two separate glyphs
  - `->` at end of line with `>` wrapped to next line renders as two separate glyphs
  - `->` with same formatting renders as ligature
- **Tests required:**
  - Color change mid-ligature breaks it
  - Style change mid-ligature breaks it
  - Line boundary mid-ligature breaks it
  - Same-format sequence forms ligature

### 5.7 — Performance optimization

- **Status:** Complete (commit d0e3c4e)
- **Scope:** `freminal/src/gui/shaping.rs`, `freminal/src/gui/atlas.rs`
- **Details:**
  - Ligature shaping should not significantly impact frame time
  - Cache shaped runs aggressively — most terminal lines don't change between frames
  - Ligature glyphs in atlas should use same LRU as regular glyphs
  - Profile and benchmark with ligature-heavy content (e.g., Rust source code)
- **Acceptance criteria:**
  - Frame time with ligatures enabled is within 10% of ligatures disabled
  - No noticeable lag when scrolling through ligature-heavy content
- **Benchmarks required:**
  - Shaping throughput with ligatures enabled vs disabled
  - Full render cycle with ligature-heavy content
  - Atlas cache behavior with ligature glyphs

### 5.8 — Integration testing and cleanup

- **Status:** Complete (commit d0e3c4e)
- **Scope:** All modified files
- **Details:**
  - Test with multiple ligature-supporting fonts:
    - JetBrains Mono
    - Fira Code
    - Cascadia Code
    - MesloLGS Nerd Font Mono (bundled)
  - Test with fonts that don't support ligatures (should work fine, no ligatures formed)
  - Test config toggle: enable/disable ligatures at runtime (if hot-reload from Task 3 is available)
  - Run full verification suite
- **Acceptance criteria:**
  - Ligatures render correctly with all tested fonts
  - Non-ligature fonts work without errors
  - Config toggle works
  - All tests pass, clippy clean

---

## Affected Files

| File                                    | Change Type                                      |
| --------------------------------------- | ------------------------------------------------ |
| `freminal-common/src/config.rs`         | Add ligatures config option                      |
| `freminal/src/gui/shaping.rs`           | Enable ligature features, handle ligature glyphs |
| `freminal/src/gui/atlas.rs`             | Support variable-width ligature glyph caching    |
| `freminal/src/gui/terminal.rs`          | Cursor/selection within ligatures                |
| `freminal/benches/render_loop_bench.rs` | Add ligature benchmarks                          |
| `config_example.toml`                   | Add ligatures option                             |

---

## Common Ligature Sequences to Test

| Sequence | Unicode Points       | Description      |
| -------- | -------------------- | ---------------- |
| `->`     | U+002D U+003E        | Arrow            |
| `=>`     | U+003D U+003E        | Fat arrow        |
| `<-`     | U+003C U+002D        | Left arrow       |
| `!=`     | U+0021 U+003D        | Not equal        |
| `==`     | U+003D U+003D        | Equal            |
| `===`    | U+003D U+003D U+003D | Strict equal     |
| `>=`     | U+003E U+003D        | Greater or equal |
| `<=`     | U+003C U+003D        | Less or equal    |
| `::`     | U+003A U+003A        | Scope resolution |
| `/*`     | U+002F U+002A        | Comment open     |
| `*/`     | U+002A U+002F        | Comment close    |
| `//`     | U+002F U+002F        | Line comment     |
| `&&`     | U+0026 U+0026        | Logical and      |
| `\|\|`   | U+007C U+007C        | Logical or       |
| `\|>`    | U+007C U+003E        | Pipe             |
| `<\|`    | U+003C U+007C        | Reverse pipe     |

---

## Risk Assessment

| Risk                                     | Likelihood | Impact | Mitigation                           |
| ---------------------------------------- | ---------- | ------ | ------------------------------------ |
| Bundled font has no ligatures            | Medium     | Low    | Test with font, fall back gracefully |
| Ligature width doesn't match cell grid   | Medium     | High   | Explicit N× cell_width enforcement   |
| Performance regression from shaping      | Low        | Medium | Benchmark, cache aggressively        |
| Visual artifacts at ligature boundaries  | Medium     | Medium | Extensive visual testing             |
| Cursor positioning bugs within ligatures | Medium     | High   | Thorough cursor position tests       |

---

## Benchmark Baselines

Record before/after numbers here as subtasks complete:

| Metric                        | Without Ligatures | With Ligatures | Subtask |
| ----------------------------- | ----------------- | -------------- | ------- |
| Shaping throughput (runs/sec) | ~3.6 Melem/s      | ~3.3 Melem/s   | 5.7     |
| Shaping cache hit             | ~146 Melem/s      | —              | 5.7     |
| Shaping time (80×50)          | ~1.12 ms          | ~1.20 ms       | 5.7     |

Ligatures on is ~7% slower than off (expected — more OpenType feature lookups).
Cache hits are ~43× faster than cold shaping.
