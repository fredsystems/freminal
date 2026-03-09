# PLAN_01 — Glyph Atlas + Custom Painter + Text Shaping Infrastructure

## Overview

Replace the current per-character `painter.text()` rendering with a glyph atlas, custom painter,
and text shaping pipeline. This task also introduces the rustybuzz shaping infrastructure that
Task 5 (Font Ligatures) will build on.

**Dependencies:** None
**Dependents:** Task 5 (Font Ligatures)
**Primary crate:** `freminal` (GUI binary)
**Estimated scope:** Large — touches rendering core, font system, and introduces new dependencies

---

## Problem Statement

The current rendering pipeline in `render_terminal_text()` (`freminal/src/gui/terminal.rs:904-1032`)
calls `painter.text()` **per character**, resulting in ~4000 draw calls per frame for an 80×50
terminal. Each call:

- Allocates a `String` via `c.to_string()` (line 1019)
- Passes through egui's full text layout pipeline
- Acquires `fonts_mut()` mutex (multiple times per frame)

Additional problems:

- `LayoutJob` is constructed but NOT used for egui galley rendering — code manually paints char-by-char
- `internal_color_to_egui()` calls `Color32::from_hex()` for named colors on every invocation
- No text shaping — no harfbuzz/rustybuzz, no OpenType feature processing
- Wide character rendering is broken — buffer models 2-column cells but renderer forces 1-column
- Continuous 16ms repaint even when nothing changed

---

## Architecture Design

### Target Architecture

```text
TerminalSnapshot
    │
    ▼
Text Shaping Layer (rustybuzz)
    │  Input: runs of TChars with consistent formatting
    │  Output: shaped glyph IDs + positions
    ▼
Glyph Atlas (texture cache)
    │  Input: glyph IDs from shaping
    │  Output: UV coordinates in atlas texture
    │  Cache: LRU eviction, dynamic growth
    ▼
Custom Painter (egui Shape::mesh)
    │  Input: glyph positions + UV coords + colors
    │  Output: single batched mesh for entire terminal
    ▼
GPU (single draw call for all terminal text)
```

### Key Design Decisions

1. **rustybuzz over harfbuzz-sys** — Pure Rust, no C dependencies, easier cross-compilation,
   sufficient for terminal use case (monospace shaping + ligatures).

2. **Glyph atlas with LRU eviction** — Dynamic texture atlas that rasterizes glyphs on demand,
   caches them, and evicts least-recently-used entries when full. Sized for typical terminal
   character sets (~500-1000 unique glyphs).

3. **Custom painter via `egui::Shape::mesh`** — Bypass egui's text rendering entirely. Build a
   single `Mesh` with textured quads for all visible glyphs. Single draw call replaces ~4000.

4. **Run-based shaping** — Group consecutive characters with identical formatting into "runs",
   shape each run through rustybuzz. This is the foundation Task 5 needs for ligatures.

5. **Dirty-region tracking** — Only re-shape and re-upload regions that changed between snapshots.
   Most frames in a terminal only change a few lines.

---

## Subtasks

### 1.1 — Add rustybuzz and font dependencies

- **Status:** Not Started
- **Scope:** `Cargo.toml` workspace and `freminal/Cargo.toml`
- **Details:**
  - Add `rustybuzz` to workspace dependencies
  - Add `ab_glyph_rasterizer` (or `swash`) for glyph rasterization independent of egui
  - Keep `fontdb` for font discovery
- **Acceptance criteria:** Dependencies compile, no unused dep warnings from machete

### 1.2 — Create font parsing and face management module

- **Status:** Not Started
- **Scope:** New module `freminal/src/gui/font_face.rs`
- **Details:**
  - Parse bundled font files into `rustybuzz::Face` objects
  - Support all 4 variants (regular, bold, italic, bold-italic)
  - Manage face lifecycle (load once, reference by style)
  - Bridge with existing `TerminalFont` / `FontConfig` system
  - Handle system font fallback for emoji and missing glyphs
- **Acceptance criteria:** All 4 bundled font faces parse successfully, fallback chain works
- **Tests required:** Unit tests for face loading, variant selection, fallback resolution

### 1.3 — Implement text shaping pipeline

- **Status:** Not Started
- **Scope:** New module `freminal/src/gui/shaping.rs`
- **Details:**
  - Define `ShapedRun` struct: glyph IDs, positions, advance widths, cluster mapping
  - Define `TextRun` input: character slice, style (bold/italic/color), font face reference
  - Implement `shape_run(face, text, features) -> ShapedRun`
  - Handle run segmentation: split TChar sequences by formatting changes
  - Initially disable ligature features (Task 5 enables them)
  - Correctly handle wide characters (2-cell advances)
- **Acceptance criteria:** Shaped output matches expected glyph positions for test strings
- **Tests required:**
  - ASCII text shaping produces correct advances
  - Bold/italic selects correct face
  - Wide characters produce correct 2-cell advances
  - Mixed ASCII + wide character runs shape correctly
  - Run segmentation splits on format boundaries

### 1.4 — Implement glyph atlas

- **Status:** Not Started
- **Scope:** New module `freminal/src/gui/atlas.rs`
- **Details:**
  - Atlas texture: RGBA, initial size 1024×1024 (grow dynamically)
  - `GlyphKey`: (glyph_id, font_face_id, size_px) — uniquely identifies a rasterized glyph
  - `AtlasEntry`: UV rect, metrics (bearing, advance)
  - Rasterize glyphs on demand using `ab_glyph_rasterizer` or `swash`
  - LRU eviction when atlas is full
  - Upload texture to GPU via egui's `TextureManager`
  - Delta uploads: only upload modified atlas regions per frame
- **Acceptance criteria:**
  - Glyphs are correctly rasterized and cached
  - Cache hit rate > 99% for steady-state terminal output
  - Atlas grows and evicts without visual artifacts
- **Tests required:**
  - Glyph rasterization produces non-empty bitmaps
  - Cache returns same UV for repeated glyph lookups
  - Eviction removes LRU entries
  - Atlas growth allocates new texture correctly
- **Benchmarks required:**
  - Glyph lookup latency (cache hit vs miss)
  - Atlas upload throughput

### 1.5 — Implement custom painter

- **Status:** Not Started
- **Scope:** Modify `freminal/src/gui/terminal.rs` — replace `render_terminal_text()`
- **Details:**
  - Build background quad mesh: one colored rect per cell (or per run of same-background cells)
  - Build foreground glyph mesh: one textured quad per visible glyph
  - Use `egui::Shape::mesh` with atlas texture for foreground
  - Use `egui::Shape::rect_filled` batched into a mesh for backgrounds
  - Single `painter.add(Shape::mesh(...))` call replaces all `painter.text()` calls
  - Correctly handle cursor overlay, selection highlighting
  - Correctly position wide characters (2-cell width)
- **Acceptance criteria:**
  - Visual output matches current rendering (pixel comparison)
  - Frame time reduced from ~4ms to <1ms for 80×50 terminal
  - Wide characters display at correct width
  - Cursor and selection render correctly
- **Tests required:**
  - Mesh generation produces correct vertex count for known terminal content
  - Background colors are correctly assigned per cell
  - Wide characters span correct number of cells in mesh
- **Benchmarks required:**
  - Full render cycle (snapshot → mesh → paint) compared to current pipeline
  - Must capture before/after numbers using `render_loop_bench.rs`

### 1.6 — Fix color caching

- **Status:** Not Started
- **Scope:** `freminal/src/gui/colors.rs`
- **Details:**
  - Replace runtime `Color32::from_hex()` calls with `const` color values
  - Named colors (Catppuccin Mocha palette) should be compile-time constants
  - Eliminate per-frame allocation from color conversion
- **Acceptance criteria:** Zero heap allocations in color conversion path
- **Tests required:** Color mapping produces correct Color32 values for all named colors

### 1.7 — Add dirty-region tracking

- **Status:** Not Started
- **Scope:** `freminal/src/gui/terminal.rs`, potentially `freminal-terminal-emulator/src/snapshot.rs`
- **Details:**
  - Compare consecutive snapshots to identify changed lines
  - Only re-shape changed lines (cache shaped runs for unchanged lines)
  - Only re-build mesh regions for changed lines
  - Skip repaint entirely if snapshot hasn't changed (remove unconditional 16ms repaint)
- **Acceptance criteria:**
  - Idle terminal uses near-zero CPU (no repaints when nothing changes)
  - Typing single characters only re-shapes the affected line
- **Tests required:**
  - Dirty detection correctly identifies changed lines
  - Unchanged lines are not re-shaped
- **Benchmarks required:**
  - Idle CPU usage (before/after)
  - Single-line update cost vs full-screen update cost

### 1.8 — Fix wide character rendering

- **Status:** Not Started
- **Scope:** `freminal/src/gui/terminal.rs` (custom painter), shaping pipeline
- **Details:**
  - Wide characters (CJK, emoji) must render at 2× cell width
  - Shaping pipeline already handles this (1.3), but painter must correctly position
  - Placeholder cells (second column of wide char) must not render a glyph
  - Test with CJK text and emoji
- **Acceptance criteria:** Wide characters display correctly at 2× cell width, no overlap
- **Tests required:** Visual regression tests with CJK/emoji content

### 1.9 — Integration and cleanup

- **Status:** Not Started
- **Scope:** All modified files
- **Details:**
  - Remove old `create_terminal_output_layout_job()` and `process_tags()` functions
  - Remove unused LayoutJob construction code
  - Clean up font_mut() acquisition (should be once per frame at most)
  - Update benchmarks with new baselines
  - Run full verification suite
- **Acceptance criteria:**
  - No dead code from old rendering pipeline
  - All tests pass
  - All clippy warnings resolved
  - Benchmark baselines recorded in this document

---

## Affected Files

| File                                         | Change Type                          |
| -------------------------------------------- | ------------------------------------ |
| `Cargo.toml` (workspace)                     | Add dependencies                     |
| `freminal/Cargo.toml`                        | Add dependencies                     |
| `freminal/src/gui/terminal.rs`               | Major rewrite of rendering           |
| `freminal/src/gui/colors.rs`                 | Refactor to const colors             |
| `freminal/src/gui/fonts.rs`                  | Bridge to new face management        |
| `freminal/src/gui/mod.rs`                    | Update frame loop for dirty tracking |
| `freminal/src/gui/font_face.rs`              | NEW — font face management           |
| `freminal/src/gui/shaping.rs`                | NEW — text shaping pipeline          |
| `freminal/src/gui/atlas.rs`                  | NEW — glyph atlas                    |
| `freminal/benches/render_loop_bench.rs`      | Update/add benchmarks                |
| `freminal-terminal-emulator/src/snapshot.rs` | Potentially add dirty flags          |

---

## Risk Assessment

| Risk                                          | Likelihood | Impact | Mitigation                                   |
| --------------------------------------------- | ---------- | ------ | -------------------------------------------- |
| Visual regression (glyphs render differently) | Medium     | High   | Pixel-comparison tests, gradual rollout      |
| Atlas memory pressure                         | Low        | Medium | LRU eviction, configurable atlas size        |
| rustybuzz shaping differences from egui       | Medium     | Medium | Extensive test coverage with known strings   |
| Wide char edge cases                          | Medium     | Medium | Test matrix with CJK, emoji, combining marks |
| Performance regression during transition      | Low        | High   | Benchmark every subtask, can revert          |

---

## Benchmark Baselines

Record before/after numbers here as subtasks complete:

| Metric                       | Before (current) | After | Subtask |
| ---------------------------- | ---------------- | ----- | ------- |
| render_terminal_text (80×50) | —                | —     | 1.5     |
| Frame time (idle)            | —                | —     | 1.7     |
| Frame time (full redraw)     | —                | —     | 1.5     |
| Glyph cache hit rate         | N/A              | —     | 1.4     |
| Color conversion (per call)  | —                | —     | 1.6     |
