# PLAN_01 — Custom Terminal Renderer

## Overview

Replace the entire egui-based text rendering pipeline with a custom OpenGL renderer that owns
the cell grid, font stack, glyph atlas, and draw calls end-to-end. egui continues to handle
chrome (menu bar, settings modal); the terminal area is rendered directly via glow shaders
through egui's `PaintCallback` mechanism.

This is not an optimisation of the existing approach. It is a replacement. The current renderer
fights egui's layout system at every level — cell sizing, background fills, glyph placement,
wide characters. The new renderer eliminates egui from the terminal rendering path entirely.

**Dependencies:** None
**Dependents:** Task 5 (Font Ligatures)
**Primary crate:** `freminal` (GUI binary)
**Estimated scope:** Large — replaces the entire rendering core, font system, and introduces
new dependencies (rustybuzz, swash, glow shaders)

---

## Problem Statement

### What is broken (not just slow)

The current rendering pipeline has **structural** problems, not just performance problems:

1. **Cell sizing is inconsistent.** `get_char_size()` measures `' '` (space) in the regular
   face. `render_terminal_text()` measures `'W'`. These are the same for MesloLGS Nerd Mono
   but that is an implicit assumption, not an invariant. The cell grid width is a float derived
   from egui's font metrics, which can produce sub-pixel values at non-integer DPI. There is no
   single authoritative definition of "cell size in pixels."

2. **Fighting egui's layout system.** The terminal area is inside `ScrollArea` → `Frame` →
   `allocate_exact_size`. egui's layout engine adds item spacing, padding, and rounding that
   we then fight to undo. `allocate_exact_size` claims a rect but the actual rendering ignores
   it and positions glyphs manually relative to an `origin` point.

3. **Background fill alignment.** Each cell with a non-default background gets a separate
   `painter.rect_filled()` call. At non-integer DPI, float-based cell positions produce
   sub-pixel gaps between adjacent cells — visible as hairline artifacts in colored output
   (ls --color, htop, vim colorschemes).

4. **LayoutJob built but never used.** `process_tags()` builds a complete `egui::LayoutJob`
   with correctly segmented format sections. `render_terminal_text()` then **ignores it** and
   manually loops over `job.sections` calling `painter.text()` per character. The entire
   LayoutJob abstraction is allocated and discarded every frame.

5. **~1920 draw calls per frame.** Each `painter.text()` call allocates a `String`
   (`c.to_string()`), builds a single-glyph `Galley`, tessellates it into a `Mesh`, and
   discards it. For 80×24 that is 1920 String allocations, 1920 Galley allocations, 1920
   tessellation passes, and 1920 GPU draw calls per frame.

6. **No wide character / emoji support.** The render loop advances x by exactly one
   `glyph_width` per character regardless of Unicode width. CJK characters clip. Emoji are
   not rendered from an emoji font. There is no font fallback chain.

7. **No color emoji support.** The font stack is purely egui's `FontDefinitions` — no COLR,
   CBDT, or sbix table support. Emoji are either missing or rendered as monochrome outlines.

8. **`Color32::from_hex()` called at runtime** for every color conversion. Named Catppuccin
   Mocha colors are parsed from hex strings on every invocation instead of being compile-time
   constants.

9. **`get_char_size()` called 2-3 times per frame** with 4-6 `fonts_mut()` Mutex acquisitions
   for a value that only changes when the user changes font settings.

10. **`create_terminal_output_layout_job()` allocates three buffers per frame:** `Vec<u8>` for
    raw bytes, `Vec<usize>` for TChar→byte offset mapping, and `String` for the final UTF-8.
    Plus N `FormatTag` struct clones including inner `Vec<FontDecorations>` and
    `Option<Arc<String>>` URL fields.

### What we need

A renderer where:

- **One definition of cell size** — integer pixels, computed once from font metrics, used
  everywhere. No floats in the grid.
- **We own the entire terminal rect** — no egui layout, no ScrollArea, no Frame. egui gives us
  a rect via `PaintCallback`; we draw everything inside it with our own shaders.
- **Background fills are pixel-perfect** — integer-aligned cell rects with no sub-pixel gaps.
- **Font fallback chain with color emoji** — primary monospace → bold/italic variants → emoji
  font (COLR/CBDT/sbix) → system fallback. All rasterized into a single glyph atlas.
- **Text shaping via rustybuzz** — correct glyph selection, foundation for Task 5 ligatures.
- **Two draw calls maximum** — one for the background grid (colored quads), one for the
  foreground glyphs (textured quads from the atlas). Both via custom GL shaders.

---

## Architecture Design

### Target Architecture

```text
TerminalSnapshot (lock-free, from ArcSwap)
    │
    ▼
Run Segmentation
    │  Input: visible_chars + visible_tags from snapshot
    │  Output: Vec<TextRun> — spans of chars with same format
    │  (cached per-line; only dirty lines re-segmented)
    ▼
Text Shaping (rustybuzz)
    │  Input: TextRun text + font face
    │  Output: ShapedRun — glyph IDs + cluster advances
    │  (cached per-line; only dirty lines re-shaped)
    ▼
Glyph Rasterisation (swash)
    │  Input: glyph IDs from shaping + emoji codepoints
    │  Output: rasterised glyph images (greyscale for text, RGBA for color emoji)
    │  Stored in glyph atlas texture
    ▼
Glyph Atlas (RGBA texture, managed by us)
    │  LRU cache of rasterised glyphs
    │  Uploaded to GPU via glow as a GL texture
    │  Delta uploads: only modified regions per frame
    ▼
Cell Grid Renderer (custom glow shaders via egui PaintCallback)
    │
    ├── Background pass: one draw call
    │   Vertex buffer: one quad per cell (or per run of same-bg cells)
    │   Solid-color shader, no texture needed
    │   Integer pixel positions — no sub-pixel gaps
    │
    └── Foreground pass: one draw call
        Vertex buffer: one textured quad per visible glyph
        Fragment shader samples glyph atlas
        Greyscale glyphs: alpha-blend with foreground color
        Color emoji: pass through RGBA directly
        Integer pixel positions — glyphs snapped to cell grid
    │
    ▼
GPU (2 draw calls total for entire terminal)
```

### How egui Integration Works

egui owns the window, the menu bar, and the settings modal. The terminal area is rendered
via egui's `PaintCallback` mechanism:

```rust
// In show(): allocate the terminal rect
let (rect, response) = ui.allocate_exact_size(terminal_size, Sense::click_and_drag());

// Add a PaintCallback — our GL code runs during egui's paint phase
let gpu_resources = Arc::clone(&self.gpu_resources);
let snapshot = Arc::clone(&current_snapshot);
ui.painter().add(egui::PaintCallback {
    rect,
    callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
        let gl = painter.gl();
        // info.viewport_in_pixels() → glViewport
        // info.clip_rect_in_pixels() → glScissor
        gpu_resources.render(gl, &snapshot, &info);
        // Restore egui's FBO
        unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo()); }
    })),
});
```

egui's paint phase calls our callback at the correct z-order. We get the raw `glow::Context`,
set up our viewport/scissor, bind our shaders and textures, draw the terminal, and restore
egui's framebuffer binding. egui composites our output with its own UI seamlessly.

**GL state contract:** egui sets `GL_SCISSOR_TEST` enabled, `GL_BLEND` enabled (premultiplied
alpha), `GL_DEPTH_TEST` disabled, `GL_CULL_FACE` disabled, `TEXTURE0` active. We must restore
the framebuffer binding when done. Our shaders must output premultiplied alpha to composite
correctly with egui's blend state.

### Key Design Decisions

1. **rustybuzz + swash (modular stack, not cosmic-text)**

   `cosmic-text` bundles shaping, layout, font fallback, and rasterisation into one package.
   However:
   - It uses `harfrust` internally, not `rustybuzz` — we need direct control over OpenType
     feature flags for Task 5 (ligatures).
   - Its layout engine handles line wrapping, paragraph positioning, and cursor placement —
     all things a terminal does itself. We would be paying for layout logic we don't want and
     fighting its opinions about text positioning.
   - There is a hard `skrifa` version conflict between `cosmic-text` and `swash`.

   The modular approach gives us:
   - `rustybuzz`: shaping with exact feature flag control (ligatures off now, on in Task 5)
   - `swash`: font loading, glyph rasterisation including **full color emoji** (COLR, CBDT,
     sbix table support), font metadata and metrics
   - We write the font fallback chain (~200 lines) and atlas management ourselves — these are
     terminal-specific enough that no library would do them right for us

2. **Full glow bypass (not egui Shape::mesh)**

   The previous plan proposed `egui::Shape::mesh` — building a `Mesh` of textured quads and
   letting egui tessellate and draw it. This still goes through egui's rendering pipeline and
   is subject to its texture management, blend state, and coordinate system.

   Full glow bypass via `PaintCallback` gives us:
   - Our own shaders (separate programs for background quads and textured glyph quads)
   - Our own texture management (the glyph atlas is a GL texture we upload directly)
   - Integer pixel coordinates with no float rounding
   - Full control over blend mode (important for color emoji compositing)
   - No dependency on egui's `Mesh` type or tessellator

3. **Integer cell grid**

   Cell size is computed once from `swash` font metrics (not egui's `fonts_mut`):

   ```text
   cell_width  = ceil(max_advance) as u32    // in pixels
   cell_height = ceil(ascent + descent + leading) as u32  // in pixels
   ```

   All cell positions are `(col * cell_width, row * cell_height)` — integer multiplication,
   no floats, no sub-pixel gaps, no rounding errors. The terminal rect size is
   `(cols * cell_width, rows * cell_height)` — always exact.

4. **Font fallback chain with color emoji from the outset**

   ```text
   Primary: MesloLGS Nerd Font Mono (Regular/Bold/Italic/BoldItalic)
       ↓ glyph not found
   Emoji: Noto Color Emoji (or system emoji font)
       ↓ glyph not found
   System: fontdb discovery → swash rasterisation
       ↓ glyph not found
   Tofu: render U+FFFD replacement character
   ```

   `swash` handles all four levels. For the emoji font, `swash` rasterises COLR/CBDT/sbix
   glyphs as full RGBA bitmaps. These go into the same atlas texture as monochrome glyphs
   (the atlas is RGBA regardless). The fragment shader detects whether a glyph quad is
   monochrome (apply foreground color tinting) or color emoji (pass through RGBA directly)
   via a per-vertex attribute flag.

5. **Per-line dirty tracking**

   `TerminalSnapshot::content_changed` already exists. We extend this with per-line granularity:
   the shaping cache stores `(line_content_hash, Vec<ShapedRun>)` per row. When a new snapshot
   arrives, only lines whose content hash changed are re-segmented, re-shaped, and re-uploaded.
   On a static screen (cursor blink only), the per-frame cost is essentially zero: compare
   hashes, skip all shaping, re-render the cursor overlay.

---

## What Gets Deleted

The following code is removed entirely once the new renderer is complete:

| Code                                          | Location                | Replacement                                 |
| --------------------------------------------- | ----------------------- | ------------------------------------------- |
| `render_terminal_text()`                      | `terminal.rs:1039-1167` | Custom glow renderer                        |
| `add_terminal_data_to_ui()`                   | `terminal.rs:1169-1223` | Direct snapshot → shader pipeline           |
| `create_terminal_output_layout_job()`         | `terminal.rs:794-885`   | Run segmentation (no UTF-8 remapping)       |
| `process_tags()`                              | `terminal.rs:958-1037`  | Run segmentation + shaping                  |
| `setup_job()`                                 | `terminal.rs:887-920`   | Gone — no LayoutJob                         |
| `UiJobAction`, `NewJobAction`, `UiData` types | `terminal.rs:922-938`   | Gone — no intermediate text representations |
| `setup_bg_fill()`                             | `terminal.rs:781-792`   | Background quads in shader                  |
| `internal_color_to_egui()` (runtime hex)      | `colors.rs`             | Compile-time const Color32 values           |
| `get_char_size()` (egui fonts_mut)            | `fonts.rs:416-425`      | swash font metrics, computed once           |
| `setup_font_files()` (egui FontDefinitions)   | `fonts.rs:73-98`        | swash font loading                          |
| `ScrollArea` wrapping terminal output         | `terminal.rs` (show)    | Our own scroll handling                     |
| All `fonts_mut()` calls for terminal text     | `terminal.rs`, `mod.rs` | swash metrics + cached cell size            |
| `TerminalFont` egui family handles            | `fonts.rs`              | swash face handles                          |

egui font infrastructure is **retained** for the menu bar, settings modal, and any non-terminal
UI elements. Only the terminal rendering path is replaced.

---

## Subtasks

### 1.1 — Add dependencies and create module skeleton

- **Status:** Complete (2026-03-10)
- **Scope:** `Cargo.toml` (workspace + freminal)
- **Details:**
  - Add to workspace deps: `rustybuzz`, `swash`, `glow` (already transitive via eframe, but
    pin version for direct use)
  - Keep `fontdb` for system font discovery
  - Create empty module files with `pub mod` declarations:
    - `freminal/src/gui/font_manager.rs` — font loading, face management, fallback chain
    - `freminal/src/gui/shaping.rs` — run segmentation and text shaping
    - `freminal/src/gui/atlas.rs` — glyph atlas and rasterisation
    - `freminal/src/gui/renderer.rs` — GL shader programs, vertex buffers, draw calls
  - Ensure `cargo build --all` compiles, `cargo-machete` clean (deps are used in stubs)
- **Acceptance criteria:** All deps compile, module skeleton exists, no warnings
- **Tests:** None yet (empty modules)
- **Completion notes:** Added `glow = "0.16.0"`, `rustybuzz = "0.20.1"`, `swash = "0.2.6"` to
  workspace deps. Created four stub modules with doc comments, basic structs, and tests. Each
  stub references its target dependency to satisfy `cargo-machete`. `glow`, `rustybuzz`, and
  `swash` added to machete ignore list since they are only used in test-cfg stubs for now.
  All clippy pedantic/nursery lints pass clean.

### 1.2 — Font manager: loading, metrics, and fallback chain

- **Status:** Complete (2026-03-10)
- **Scope:** `freminal/src/gui/font_manager.rs`
- **Details:**

  #### Core struct

  ```rust
  pub struct FontManager {
      /// Primary face stack (regular, bold, italic, bold-italic).
      /// If a user font is configured, it is loaded as primary and the bundled
      /// MesloLGS faces become the first fallback tier.
      primary: PrimaryFaces,

      /// System emoji face (Noto Color Emoji, Apple Color Emoji, etc.)
      emoji_face: Option<LoadedFace>,

      /// Lazily-discovered system fallback faces, keyed by codepoint range.
      system_fallback_cache: HashMap<char, Option<LoadedFace>>,

      /// Resolved glyph cache: (codepoint, style) → (FaceId, GlyphId).
      glyph_cache: HashMap<(char, GlyphStyle), (FaceId, GlyphId)>,

      /// Authoritative cell size in integer pixels. Recomputed on font change.
      cell_width: u32,
      cell_height: u32,

      /// Font size in points. Drives rasterisation size in the atlas.
      font_size_pt: f32,

      /// fontdb database for system font discovery.
      font_db: fontdb::Database,
  }
  ```

  #### Font loading

- Load bundled MesloLGS Nerd Font Mono TTF files (4 variants: Regular, Bold, Italic,
  BoldItalic) via `swash`. These are the default primary faces and are always available.
- Parse font metrics from the primary regular face: ascent, descent, leading, max advance
  width.
- Compute authoritative cell size: `cell_width = ceil(max_advance)`,
  `cell_height = ceil(ascent + descent + leading)`. Integer pixels. One definition. Used
  everywhere.

  #### User font override

  The existing config system has two `FontConfig` types:
  - `freminal_common::config::FontConfig` — TOML-serializable: `{ family: Option<String>, size: f32 }`
  - `gui::fonts::FontConfig` — egui-specific rendering config (will be replaced by `FontManager`)

  When `config.font.family` is `Some(path_or_name)`:
  1. **File path:** If the string is an existing file path, load the font file via
     `swash::FontRef::from_index(&data, 0)`. Use it as the primary regular face.
  2. **System font name:** Search `fontdb::Database` for a face whose family name matches
     (case-insensitive, substring match — same behaviour as current `find_system_font_by_name`).
     Load the matched file via `swash`.
  3. **Bold/italic variants:** When a user font is loaded, search the same family in fontdb
     for bold, italic, and bold-italic variants by matching `fontdb::Style` and `fontdb::Weight`.
     If variants are not found, fall back to the regular face for those styles (same as current
     behaviour where user font is inserted into all four family slots).
  4. **Bundled faces become first fallback:** When a user font is active, the bundled MesloLGS
     faces are demoted to the first fallback tier (between primary and emoji). This ensures
     Nerd Font symbols still render even if the user font lacks them.
  5. **Failure handling:** If the user font cannot be loaded (file missing, parse error, etc.),
     log a warning and fall back to the bundled MesloLGS faces as primary. Do not panic.

  Cell size is always computed from whichever font is primary (user or bundled). When the user
  font changes, cell size is recomputed.

  #### Font fallback chain

  The full chain, in resolution order:
  1. **Primary face** (user font if configured, else bundled MesloLGS) — regular/bold/italic/
     bold-italic selected by `FontWeight` + `FontDecorations` from the format tag.
  2. **Bundled fallback** (MesloLGS) — only present as a separate tier when a user font is
     primary. Covers Nerd Font symbols the user font may lack.
  3. **Emoji face:** Search system fonts for known emoji families: Noto Color Emoji, Apple Color
     Emoji, Segoe UI Emoji, Twemoji, Emoji One, OpenMoji, Symbola (same candidate list as
     current `emoji_fonts::CANDIDATES`). Load via `swash`. Done once at construction.
  4. **System fallback:** For any glyph not found above, query `fontdb` for a font covering
     that codepoint. Load via `swash`. Cache discovered faces per codepoint.
  5. **Tofu:** If no font has the glyph, return the `.notdef` glyph from the primary face
     (rendered as a box or U+FFFD).

  #### Core lookup function

  `resolve_glyph(codepoint: char, style: GlyphStyle) -> (FaceId, GlyphId)` — tries each
  fallback tier in order. Caches results per `(codepoint, style)` pair. Second call for the
  same key is a `HashMap` lookup (~10ns).

  #### Hot reload

  `FontManager` exposes a `rebuild(&mut self, config: &Config)` method that:
  1. Compares `config.font.family` and `config.font.size` against current values.
  2. If family changed: reloads the primary face stack (user font or bundled), recomputes
     cell size, clears the glyph cache and the glyph atlas (atlas invalidation is signalled
     to the caller via a return value).
  3. If only size changed: recomputes cell size, clears the glyph cache and atlas (glyphs
     are rasterised at a specific pixel size so all atlas entries are invalidated).
  4. If nothing changed: returns early with no work.

  This replaces the current `apply_config_changes` font path, which calls `setup_font_files(ctx,
&new_font_config)` to rebuild egui's `FontDefinitions`. The new path does not touch egui's
  font system at all.

  `apply_config_changes` in `FreminalTerminalWidget` will call `self.font_manager.rebuild(new_config)`
  and, if it returns `FontChanged`, additionally:
  - Invalidate the glyph atlas (signal to `atlas.rs` to clear all entries)
  - Invalidate the shaping cache (signal to `shaping.rs` to clear all cached shaped runs)
  - Recompute terminal dimensions from the new cell size
  - Send `InputEvent::Resize` if terminal dimensions changed

  #### Bridging the two FontConfig types

  The `gui::fonts::FontConfig` struct and its associated functions (`setup_font_files`,
  `get_char_size`, `TerminalFont`) are **retained** during the transition for egui chrome
  (menu bar, settings modal). `FontManager` does not use them. Eventually (subtask 1.9),
  `gui::fonts::FontConfig` and `TerminalFont` can be simplified since they only serve
  non-terminal UI.

  #### Providing face references for shaping

  `FontManager` exposes `rustybuzz::Face` references for each loaded face. These are needed
  by the shaping pipeline (subtask 1.3). The `Face` is created from the same font data bytes
  that `swash` uses, so no duplication of font loading is needed — both libraries read from
  the same `&[u8]` buffer.

- **Acceptance criteria:**
  - All 4 bundled faces load and produce valid metrics
  - Cell size is integer and consistent across all faces
  - Emoji face discovered on system (or graceful fallback if not installed)
  - `resolve_glyph` returns correct face for ASCII, CJK, and emoji codepoints
  - User font override works: file path loads the font as primary
  - User font override works: system font name resolves via fontdb
  - User font fallback: if user font fails to load, bundled font is used
  - Bold/italic variant discovery: when user font family has variants, they are used
  - `rebuild()` with changed family reloads faces and recomputes cell size
  - `rebuild()` with changed size recomputes cell size without reloading faces
  - `rebuild()` with no changes returns early
- **Tests required:**
  - Bundled font loading produces non-zero metrics
  - Cell size computation matches expected values for MesloLGS
  - Fallback chain: ASCII → primary, emoji codepoint → emoji face, unknown → tofu
  - `resolve_glyph` caching: second call returns same result without re-scanning
  - User font from file path: loads successfully, becomes primary
  - User font from system name: resolves via fontdb (test with a known system font)
  - User font failure: graceful fallback to bundled
  - `rebuild()` with font change: glyph cache is cleared, cell size recomputed
  - `rebuild()` with size change: cell size recomputed, glyph cache cleared
  - `rebuild()` no-op: no state changes when config is identical

### 1.3 — Text shaping pipeline

- **Status:** Complete (2026-03-10)
- **Scope:** `freminal/src/gui/shaping.rs`
- **Details:**
  - **Run segmentation:** Walk `visible_chars` + `visible_tags` from the snapshot. Split into
    `TextRun` spans wherever the format changes (color, weight, italic, underline) OR the font
    face changes (primary vs emoji vs fallback — determined by `resolve_glyph` from 1.2).
    Each `TextRun` is a contiguous sequence of characters that can be shaped in a single
    `rustybuzz::shape()` call.
  - **Shaping:** For each `TextRun`, call `rustybuzz::shape()` with the appropriate face.
    Output: `ShapedRun` containing glyph IDs, x-advances, y-offsets, and cluster→character
    mapping.
  - **Cell grid snapping:** After shaping, snap each glyph's x-position to the cell grid.
    For monospace text this is trivial (glyph N goes at column N × cell_width). For emoji
    and wide characters, the glyph spans 2 cells. The shaping output's advance is used to
    validate this, not to position — the cell grid is authoritative.
  - **Ligature control:** Initially shape with `kern` and `cmap` features only. Ligature
    features (`liga`, `calt`, `dlig`) are disabled. Task 5 enables them.
  - **Per-line caching:** Store `(content_hash, Vec<ShapedRun>)` per row. On each snapshot,
    only re-shape rows whose content hash changed. Hash is computed from the TChar sequence
    and FormatTag data for that row.
- **Acceptance criteria:**
  - ASCII text shapes to expected glyph IDs and advances
  - Bold/italic selects correct face variant
  - Wide characters (CJK U+4E2D etc.) produce 2-cell advances
  - Emoji codepoints route to emoji face and shape correctly
  - Mixed runs (ASCII + emoji + CJK) segment and shape correctly
  - Per-line cache avoids re-shaping unchanged lines
- **Tests required:**
  - ASCII run shaping: glyph count matches char count, advances are uniform
  - CJK run shaping: 2-cell advance per character
  - Emoji run shaping: routes to emoji face, produces valid glyph IDs
  - Run segmentation splits on format boundaries
  - Run segmentation splits on font-face boundaries (ASCII→emoji transition)
  - Cache hit: re-shaping identical content returns cached result
  - Cache miss: changed content produces new shaped runs

### 1.4 — Glyph atlas with color emoji

- **Status:** Complete (2026-03-10)
- **Scope:** `freminal/src/gui/atlas.rs`
- **Details:**
  - **Atlas texture:** Single RGBA texture, initial size 1024×1024 pixels. Grows by doubling
    when full (max 4096×4096). Managed as a GL texture via `glow`.
  - **Glyph key:** `(glyph_id: u16, face_id: u8, size_px: u16)` — uniquely identifies a
    rasterised glyph in the atlas.
  - **Rasterisation via swash:**
    - Monochrome glyphs (ASCII, CJK): `swash` rasterises to alpha-only bitmap. Stored in
      atlas as `(R=255, G=255, B=255, A=coverage)`. The fragment shader multiplies by
      foreground color.
    - Color emoji: `swash` rasterises COLR/CBDT/sbix to full RGBA bitmap. Stored as-is in
      the atlas. The fragment shader passes through RGBA directly.
    - Per-glyph flag: `is_color: bool` stored in the atlas entry, propagated to the vertex
      buffer as a per-vertex attribute so the shader knows which blending path to use.
  - **Bin packing:** Simple shelf-based packing (rows of glyphs packed left-to-right, new
    shelf started when current shelf is full). Sufficient for terminal glyph sets. More
    sophisticated packing (Skyline, Guillotine) is unnecessary given LRU eviction.
  - **LRU eviction:** When the atlas is full and a new glyph needs space, evict the
    least-recently-used shelf. Mark evicted glyph entries as invalid so they are
    re-rasterised on next use.
  - **Delta uploads:** Track which regions of the atlas texture were modified this frame.
    Upload only those regions via `gl.tex_sub_image_2d()`. Avoid re-uploading the entire
    texture.
  - **Atlas entry:** `{ uv_rect: [f32; 4], bearing_x: i16, bearing_y: i16, width: u16,
height: u16, is_color: bool }`.
- **Acceptance criteria:**
  - ASCII glyphs rasterise to non-empty bitmaps with correct dimensions
  - Color emoji rasterise to RGBA bitmaps (not monochrome)
  - Cache hit rate > 99% for steady-state terminal output
  - Atlas growth works without visual artifacts
  - LRU eviction frees space for new glyphs
  - Delta upload only touches modified regions
- **Tests required:**
  - Monochrome glyph rasterisation: non-zero alpha, correct dimensions
  - Color emoji rasterisation: non-zero RGB, correct dimensions, `is_color = true`
  - Cache lookup: same key returns same UV rect
  - Eviction: least-recently-used entry is evicted first
  - Growth: atlas doubles when full, existing entries remain valid
  - Bin packing: glyphs do not overlap in the atlas
- **Benchmarks required:**
  - Cache hit lookup latency (target: <100ns)
  - Cache miss + rasterise latency
  - Delta upload throughput (bytes/frame)

### 1.5 — GL renderer: shaders, vertex buffers, draw calls

- **Status:** Not Started
- **Scope:** `freminal/src/gui/renderer.rs`, modifications to `terminal.rs`
- **Details:**
  - **GPU resource struct:** `TerminalRenderer` holds all GL state:
    - Two shader programs (background, foreground)
    - Vertex array objects (VAOs) and vertex buffer objects (VBOs)
    - The atlas texture handle
    - Uniform locations (viewport size, atlas texture unit)
  - **Background shader:**
    - Vertex format: `(x: f32, y: f32, r: f32, g: f32, b: f32, a: f32)` — position + color.
    - One quad per cell with a non-default background (or one quad per horizontal run of
      same-background cells — merge adjacent cells for fewer vertices).
    - Simple passthrough vertex shader; fragment shader outputs the interpolated color.
    - Integer cell positions: `x = col * cell_width`, `y = row * cell_height`. No floats in
      the grid coordinate system. The vertex shader converts to NDC.
  - **Foreground shader:** - Vertex format: `(x: f32, y: f32, u: f32, v: f32, r: f32, g: f32, b: f32, a: f32,
is_color: f32)` — position, UV, color tint, color-emoji flag. - One textured quad per visible glyph. Positioned at cell grid coordinates plus glyph
    bearing offsets. - Fragment shader: `if is_color > 0.5 { output = texture(atlas, uv); } else { output =
vec4(tint_color.rgb, tint_color.a * texture(atlas, uv).a); }` — color emoji pass
    through; monochrome glyphs are tinted by foreground color.
  - **Cursor overlay:** Rendered as part of the background pass (a colored quad at the cursor
    cell position). Blink state computed from `ui.input(|i| i.time)` before the callback.
  - **Underline / strikethrough:** Rendered as thin quads in the background pass at the
    appropriate y-offset within the cell.
  - **Integration with egui via `PaintCallback`:**
    - `show()` allocates the terminal rect via `ui.allocate_exact_size()`
    - Adds an `egui::PaintCallback` with an `egui_glow::CallbackFn` closure
    - The closure captures `Arc<TerminalRenderer>` + snapshot data
    - Inside the callback: set viewport/scissor from `PaintCallbackInfo`, bind shaders,
      upload vertex data, draw background pass, draw foreground pass, restore egui's FBO
  - **Vertex buffer management:** Double-buffered VBOs (write to one while GPU reads the
    other). Orphan-then-write pattern (`gl.buffer_data` with null then `gl.buffer_sub_data`)
    to avoid sync stalls.
  - **Premultiplied alpha:** All shader output must be premultiplied alpha to composite
    correctly with egui's blend state (`SRC_ALPHA=ONE, DST_ALPHA=ONE_MINUS_SRC_ALPHA`).
- **Acceptance criteria:**
  - Terminal text renders correctly (ASCII, bold, italic, colors)
  - Background fills are pixel-perfect — no sub-pixel gaps between cells
  - Color emoji render in full color at correct cell positions
  - CJK/wide characters span 2 cells correctly
  - Cursor renders at correct position with correct style (block/underline/bar)
  - Underline and strikethrough render at correct positions
  - Frame time < 1ms for 80×50 terminal (target: <0.5ms)
  - No visual artifacts at non-integer DPI scaling
- **Tests required:**
  - Shader compilation succeeds on GL 3.3+ context
  - Vertex buffer generation: correct vertex count for known terminal content
  - Background quad merging: adjacent same-color cells produce one quad
  - Cursor position maps correctly from `CursorPos` to pixel coordinates
- **Benchmarks required:**
  - Full render cycle: snapshot → vertex build → draw (must be < 1ms)
  - Vertex buffer upload throughput
  - Compare to current pipeline using `render_loop_bench.rs`

### 1.6 — Compile-time color constants

- **Status:** Complete (2026-03-10)
- **Scope:** `freminal/src/gui/colors.rs`
- **Details:**
  - Replace all `Color32::from_hex("...")` calls with `const` color values
  - Catppuccin Mocha palette as compile-time constants:

    ```rust
    pub const ROSEWATER: Color32 = Color32::from_rgb(245, 224, 220);
    pub const FLAMINGO: Color32 = Color32::from_rgb(242, 205, 205);
    // ... etc
    ```

  - `internal_color_to_egui()` becomes a simple match returning const references
  - Also provide `[f32; 4]` RGBA versions for direct use in GL vertex attributes
    (avoids `Color32` → float conversion per vertex)
  - Zero heap allocations in the color conversion path

- **Acceptance criteria:**
  - No runtime hex parsing
  - Both `Color32` and `[f32; 4]` versions available for each color
  - All existing color mappings preserved exactly
- **Tests required:**
  - Each named color produces the expected RGB values
  - `Color32` and `[f32; 4]` versions are consistent

### 1.7 — Wire it all together: show() rewrite

- **Status:** Not Started
- **Scope:** `freminal/src/gui/terminal.rs`, `freminal/src/gui/mod.rs`
- **Details:**

  #### `FreminalTerminalWidget` ownership

  `FreminalTerminalWidget` gains ownership of the new rendering subsystems:

  ```rust
  pub struct FreminalTerminalWidget {
      font_manager: FontManager,       // from 1.2
      shaping_cache: ShapingCache,     // from 1.3
      atlas: GlyphAtlas,               // from 1.4
      renderer: TerminalRenderer,      // from 1.5
      // ... existing fields retained for input handling ...
  }
  ```

  The old fields (`font_defs: FontConfig`, `terminal_fonts: TerminalFont`, `character_size`,
  `previous_font_size`, `previous_pass: TerminalOutputRenderResponse`, `max_line_width`) are
  removed. Cell size comes from `font_manager.cell_size()`.

  #### `show()` rewrite

  Rewrite `FreminalTerminalWidget::show()` to:
  1. Load snapshot from `ArcSwap`
  2. Compute terminal rect: `(snap.term_width * cell_width, snap.term_height * cell_height)`
  3. `ui.allocate_exact_size()` for the rect
  4. Check if snapshot changed (pointer comparison or `content_changed` flag)
  5. If changed: run segmentation → shaping → atlas lookup → build vertex buffers
  6. If unchanged: reuse previous frame's vertex buffers (only update cursor blink)
  7. Add `PaintCallback` that calls `TerminalRenderer::draw()`
  8. Handle input events (keyboard, mouse, scroll) as before — send through channels
  - Remove `ScrollArea` wrapping — scrollback is handled by adjusting which rows from the
    snapshot are rendered, not by egui's scroll widget
  - Remove all `fonts_mut()` calls from the terminal rendering path
  - Cell size comes from `FontManager` (1.2), computed once at startup and on font change
  - Resize detection: when the available rect size changes, recompute `(cols, rows)` from
    `(rect_width / cell_width, rect_height / cell_height)` and send `InputEvent::Resize`

  #### Hot-reload via `apply_config_changes`

  The existing `apply_config_changes(&mut self, ctx, old_config, new_config)` method is
  rewritten to route through the new pipeline:

  ```rust
  pub fn apply_config_changes(
      &mut self,
      ctx: &egui::Context,
      old_config: &Config,
      new_config: &Config,
      input_tx: &Sender<InputEvent>,
  ) {
      // Font changes route through FontManager::rebuild()
      let font_result = self.font_manager.rebuild(new_config);

      if font_result.font_changed() {
          // Atlas: all entries are invalid (font or size changed)
          self.atlas.clear();
          // Shaping cache: all entries are invalid
          self.shaping_cache.clear();
          // Check if terminal dimensions changed due to new cell size
          // (cell_width or cell_height changed → cols/rows may differ)
          let (new_w, new_h) = self.compute_terminal_dimensions();
          input_tx.send(InputEvent::Resize(new_w, new_h, ...));
      }

      // Cursor and theme changes take effect on next render via config/snapshot.
      // No additional state update needed.
  }
  ```

  Note the new `input_tx` parameter — `apply_config_changes` needs to send a resize event
  when font changes alter the cell size. The call site in `FreminalGui::update()` (mod.rs:514)
  must be updated to pass `&self.input_tx`.

  The egui-side `setup_font_files(ctx, &new_font_config)` call is **retained** during this
  subtask for chrome fonts (menu bar, settings modal). It is removed in subtask 1.9 if chrome
  fonts are also migrated, or kept if egui continues to handle its own font loading for
  non-terminal UI.

- **Acceptance criteria:**
  - Terminal renders correctly end-to-end via the new pipeline
  - Input handling (keyboard, mouse, scroll, paste) works as before
  - Resize works correctly
  - `skip_draw` (synchronized output) skips the shaping/render path
  - Font change from settings modal: atlas cleared, shaping cache cleared, cell size
    recomputed, terminal re-rendered with new font
  - Font size change from settings modal: same pipeline, only size differs
  - User font → no user font: falls back to bundled MesloLGS correctly
  - No user font → user font: loads and renders the user font
- **Tests required:**
  - End-to-end integration test: feed ANSI data → snapshot → render (vertex buffer validation)
  - Resize detection produces correct (cols, rows) for known pixel sizes
  - `skip_draw` flag suppresses rendering
  - Font hot-reload: `apply_config_changes` with font change triggers atlas + cache clear

### 1.8 — Per-line dirty tracking and caching

- **Status:** Not Started
- **Scope:** `freminal/src/gui/shaping.rs`, `freminal/src/gui/renderer.rs`
- **Details:**
  - Shaping cache: `HashMap<usize, (u64, Vec<ShapedRun>)>` — row index → (content hash,
    shaped runs). On each frame, for each visible row:
    - Compute hash of that row's TChar slice + FormatTag data
    - If hash matches cached entry: reuse shaped runs
    - If hash differs: re-segment, re-shape, update cache
  - Vertex buffer partial update: when only a few lines changed, only rebuild the vertex
    data for those lines. Use `gl.buffer_sub_data()` to update just the affected regions
    of the VBO.
  - Cursor-only frames: when no content changed and only the cursor blink state toggled,
    update only the cursor quad in the background VBO. Cost: ~0.
  - Skip `request_repaint` entirely when nothing changed and cursor is steady (no blink).
    Only request repaint when: new snapshot arrives, cursor blinks, or user input occurs.
- **Acceptance criteria:**
  - Idle terminal with steady cursor uses near-zero CPU
  - Typing a single character only re-shapes one line
  - Full-screen TUI redraw (vim, htop) re-shapes all lines but still completes in < 1ms
  - Scrollback navigation only shapes newly-visible lines
- **Tests required:**
  - Cache hit: identical row content returns cached shaped runs
  - Cache miss: changed row content produces new shaped runs
  - Partial VBO update: only expected byte range is modified
- **Benchmarks required:**
  - Idle frame cost (target: < 0.1ms with no shaping work)
  - Single-line update cost
  - Full-screen update cost
  - Scrollback scroll cost (rendering previously unseen lines)

### 1.9 — Delete old renderer and clean up

- **Status:** Not Started
- **Scope:** All modified files
- **Details:**
  - Remove all code listed in the "What Gets Deleted" section above
  - Remove egui font infrastructure from the terminal rendering path (keep for chrome)
  - Remove `LayoutJob` construction, `process_tags`, `setup_job`, `UiJobAction`, etc.
  - Remove `ScrollArea` usage for terminal content
  - Clean up `Cargo.toml`: remove any egui font-related features that are no longer needed
    for the terminal path
  - Run `cargo-machete` to verify no unused dependencies
  - Run full verification suite
  - Update benchmark baselines in this document
  - Verify the application runs correctly end-to-end
- **Acceptance criteria:**
  - No dead code from the old rendering pipeline
  - `cargo test --all` passes
  - `cargo clippy --all-targets --all-features -- -D warnings` clean
  - `cargo-machete` clean
  - Benchmark baselines recorded below
  - Application renders correctly: ASCII, bold/italic, colors, CJK, emoji, cursor, scrollback

---

## Affected Files

| File                                         | Change Type                                    |
| -------------------------------------------- | ---------------------------------------------- |
| `Cargo.toml` (workspace)                     | Add rustybuzz, swash dependencies              |
| `freminal/Cargo.toml`                        | Add rustybuzz, swash, glow (direct)            |
| `freminal/src/gui/font_manager.rs`           | NEW — font loading, metrics, fallback chain    |
| `freminal/src/gui/shaping.rs`                | NEW — run segmentation, text shaping           |
| `freminal/src/gui/atlas.rs`                  | NEW — glyph atlas, rasterisation, bin packing  |
| `freminal/src/gui/renderer.rs`               | NEW — GL shaders, vertex buffers, draw calls   |
| `freminal/src/gui/terminal.rs`               | Major rewrite: show(), render pipeline         |
| `freminal/src/gui/colors.rs`                 | Refactor to const colors + f32 variants        |
| `freminal/src/gui/fonts.rs`                  | Retain for egui chrome; bridge to font_manager |
| `freminal/src/gui/mod.rs`                    | Update frame loop, remove fonts_mut calls      |
| `freminal/benches/render_loop_bench.rs`      | Update/add benchmarks for new pipeline         |
| `freminal-terminal-emulator/src/snapshot.rs` | Possibly add per-row content hashes            |

---

## Risk Assessment

| Risk                                          | Likelihood | Impact | Mitigation                                          |
| --------------------------------------------- | ---------- | ------ | --------------------------------------------------- |
| Visual regression (glyph rendering differs)   | High       | High   | Side-by-side comparison, gradual transition         |
| GL state conflicts with egui                  | Medium     | High   | Careful save/restore of GL state in callback        |
| swash color emoji on all platforms            | Medium     | Medium | Test on Linux/macOS/Windows; fallback to monochrome |
| Atlas memory pressure (many unique glyphs)    | Low        | Medium | LRU eviction, configurable max atlas size           |
| DPI scaling edge cases                        | Medium     | Medium | Test at 1x, 1.25x, 1.5x, 2x, 3x scaling             |
| Shader compilation failures on old GL drivers | Low        | High   | Target GL 3.3 (very widely supported); fallback     |
| Font metric differences (swash vs egui)       | Medium     | Medium | Validate cell size against known font measurements  |
| Performance regression during transition      | Low        | High   | Benchmark every subtask, old renderer available     |

---

## Execution Order and Dependencies

```text
1.1 (deps + skeleton) ─┬─► 1.2 (font manager) ─┬─► 1.3 (shaping) ──► 1.7 (wire together)
                        │                        │                          │
                        └─► 1.6 (const colors)   └─► 1.4 (atlas) ──────────┤
                                                                            │
                                                  1.5 (GL renderer) ────────┤
                                                                            │
                                                                            ▼
                                                                    1.8 (dirty tracking)
                                                                            │
                                                                            ▼
                                                                    1.9 (cleanup)
```

Parallelism opportunities:

- 1.2 and 1.6 can run in parallel (font manager + const colors)
- 1.3 and 1.4 can run in parallel once 1.2 is done (shaping + atlas both need font manager)
- 1.5 can start as soon as the atlas API shape is known (does not need shaping to be complete)
- 1.7 requires 1.3, 1.4, 1.5, and 1.6 to be complete
- 1.8 requires 1.7 to be complete (need the full pipeline running)
- 1.9 requires everything else

---

## Benchmark Baselines

Record before/after numbers here as subtasks complete:

| Metric                              | Before (current) | After | Subtask |
| ----------------------------------- | ---------------- | ----- | ------- |
| Full frame time (80×50)             | —                | —     | 1.7     |
| Full frame time (200×50)            | —                | —     | 1.7     |
| Idle frame time (no content change) | —                | —     | 1.8     |
| Single-line update time             | —                | —     | 1.8     |
| Glyph atlas cache hit latency       | N/A              | —     | 1.4     |
| Color emoji rasterise latency       | N/A              | —     | 1.4     |
| Vertex buffer build time (80×50)    | N/A              | —     | 1.5     |
| Color conversion (per call)         | —                | —     | 1.6     |
| `cat 10000_lines.txt` end-to-end    | ~10s+            | —     | 1.7     |
