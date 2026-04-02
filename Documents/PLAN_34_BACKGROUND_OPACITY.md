# PLAN 34 — Instanced Renderer + Background Opacity

## Status: Pending

---

## Overview

Replace the current per-quad vertex-building renderer with an instanced rendering architecture,
and add configurable background opacity as a shader uniform. The instanced renderer eliminates
all CPU-side vertex construction for the background and foreground passes — the GPU computes cell
positions, looks up colors from a palette texture, and applies background opacity per-fragment.
This is the single largest rendering performance improvement possible.

### Why Instanced Rendering

The current renderer builds explicit vertex data on the CPU for every visible cell every frame:

- **Background pass:** 6 vertices x 6 floats = 36 floats per cell background quad, plus quad
  merging logic that walks every run. For a 200x50 terminal: up to 10,000 cells processed.
- **Foreground pass:** 6 vertices x 9 floats = 54 floats per glyph quad. For a 200x50 terminal
  with every cell filled: 540K floats.
- **Total CPU work per full rebuild:** ~900K floats constructed, uploaded to GPU, then discarded.

With instanced rendering:

- **One static unit quad** (6 vertices, uploaded once).
- **Per-cell instance data:** `(col, row, bg_color, fg_glyph_atlas_uv, flags)` — packed into
  ~8-12 floats per cell instance.
- **GPU computes position** from `(col, row)` x `(cell_width, cell_height)` uniforms.
- **GPU applies opacity** via `uniform float u_bg_opacity` — zero CPU cost, no vertex rebuild
  when the user drags the opacity slider.
- **Total CPU work per full rebuild:** ~80-120K floats (instance data only), no position math,
  no quad merging, no per-vertex color resolution.

The instanced approach also makes features like background opacity trivially correct: a single
uniform controls it in the fragment shader, with no CPU-side selective alpha manipulation needed.

### Background Opacity

All cell backgrounds become semi-transparent when `background_opacity < 1.0`:

- Default background cells, colored background cells (ls --color, vim status bars), and
  selection highlights all receive the opacity alpha via `uniform float u_bg_opacity`.
- Foreground elements (text glyphs, cursor, underlines, strikethrough, images) remain fully
  opaque — they use a different shader program with no opacity uniform.
- The menu bar background also receives opacity via egui panel fills with alpha.

This matches Ghostty and WezTerm behavior.

**Dependencies:** None (Tasks 2 and 3 are complete)
**Dependents:** None
**Primary crates:** `freminal-common` (config), `freminal` (GUI rendering)
**Estimated scope:** High (12 subtasks)

---

## Current Renderer Architecture

### Shader Programs (renderer.rs)

Three shader programs, all `#version 330 core`:

1. **Background** (`BG_VERT_SRC` / `BG_FRAG_SRC`, lines 38-61):
   - Vertex layout: `vec2 a_pos, vec4 a_color` (stride = 6 x f32)
   - Uniforms: `u_viewport_size` only
   - Fragment: premultiplied alpha output `vec4(v_color.rgb * v_color.a, v_color.a)`
   - Used for: cell backgrounds, underlines, strikethrough, cursor, selection highlights

2. **Foreground** (`FG_VERT_SRC` / `FG_FRAG_SRC`, lines 68-108):
   - Vertex layout: `vec2 a_pos, vec2 a_uv, vec4 a_color, float a_is_color` (stride = 9 x f32)
   - Uniforms: `u_viewport_size`, `u_atlas` (glyph atlas texture)
   - Fragment: monochrome glyph tinting or color emoji pass-through
   - Used for: text glyphs

3. **Image** (`IMG_VERT_SRC` / `IMG_FRAG_SRC`, lines 114-140):
   - Vertex layout: `vec2 a_pos, vec2 a_uv` (stride = 4 x f32)
   - Uniforms: `u_viewport_size`, `u_image` (per-image texture)
   - Used for: inline images (Kitty/iTerm2 protocol)

### GPU Resources (TerminalRenderer struct, lines 166-201)

- 3 shader programs, 3 VAOs, 3 x [2] VBOs (double-buffered), 5 uniform locations
- 1 atlas texture (glyph cache)
- HashMap of per-image textures

### Vertex Building Functions

- `build_background_verts()` (line 1212): Walks `ShapedLine` runs, resolves colors via
  `internal_color_to_gl()`, merges adjacent same-color cells into single quads, skips
  DefaultBackground cells, appends underline/strikethrough/selection/cursor quads.
- `build_foreground_verts()` (line 1534): Walks shaped runs, resolves per-glyph atlas UVs via
  `emit_glyph_quad()`, handles blink visibility, selection foreground color override.
- `build_image_verts()` (line 1632): Builds one textured quad per visible inline image.

### Draw Pipeline (per frame)

1. `terminal.rs` calls `shaping_cache.shape_visible()` — shapes text into `ShapedLine`s
2. `terminal.rs` calls `build_background_verts()`, `build_foreground_verts()`,
   `build_image_verts()` — all CPU-side
3. Pre-built `Vec<f32>` vertex data stashed in `RenderState` behind `Arc<Mutex<...>>`
4. egui `PaintCallback` closure: `renderer.draw_with_verts()` uploads VBOs, sets uniforms,
   draws background → foreground → images, restores egui FBO

### Cursor-Only Fast Path

When only the cursor state changed (blink toggle, cursor move), `build_cursor_verts_only()`
patches just the cursor region of the BG VBO via `glBufferSubData` — no full rebuild.

### Shaping Cache

`ShapingCache` (shaping.rs:109) caches `(content_hash, ShapedLine)` per row. Only re-shapes
rows whose content hash changed. This is preserved — instanced rendering doesn't change when
shaping happens, only what happens with the results.

---

## Target Architecture

### Instance-Based Cell Rendering

Replace per-quad vertex construction with per-cell instance data. A single static unit quad
(6 vertices) is drawn N times via `glDrawArraysInstanced(TRIANGLES, 0, 6, instance_count)`.

#### Background Instance Buffer

Per-cell background instance:

```glsl
// Instance attributes (per cell, divisor = 1):
layout(location = 1) in vec2  a_cell_pos;    // (col, row) as integers
layout(location = 2) in vec4  a_bg_color;    // resolved RGBA (alpha=1.0 for now)
layout(location = 3) in float a_cell_flags;  // bit flags: is_decoration, is_cursor, is_selected
```

Instance data per cell: `col(f32), row(f32), r, g, b, a, flags` = **7 floats**.
For a 200x50 terminal: 10,000 cells x 7 = 70K floats (vs ~360K for current BG pass).

The vertex shader computes pixel position:

```glsl
uniform vec2  u_viewport_size;
uniform float u_cell_width;
uniform float u_cell_height;
uniform float u_bg_opacity;    // ← background opacity, applied in fragment shader

void main() {
    // Unit quad vertex: a_pos is one of (0,0), (1,0), (0,1), (1,1), etc.
    vec2 cell_origin = a_cell_pos * vec2(u_cell_width, u_cell_height);
    vec2 pixel_pos = cell_origin + a_pos * vec2(u_cell_width, u_cell_height);
    vec2 ndc = (pixel_pos / u_viewport_size) * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    v_bg_color = a_bg_color;
    v_flags = a_cell_flags;
}
```

The fragment shader applies opacity:

```glsl
uniform float u_bg_opacity;

void main() {
    float is_bg = step(v_flags, 0.5);  // 1.0 if cell background, 0.0 if decoration/cursor
    float alpha = v_bg_color.a * mix(1.0, u_bg_opacity, is_bg);
    frag_color = vec4(v_bg_color.rgb * alpha, alpha);
}
```

This means:

- Cell backgrounds: alpha = `v_bg_color.a * u_bg_opacity` (user-controlled)
- Decorations/cursor: alpha = `v_bg_color.a * 1.0` (always fully opaque)
- No CPU-side selective alpha manipulation needed. Change slider → change one uniform.

#### Foreground Instance Buffer

Per-glyph foreground instance:

```glsl
layout(location = 1) in vec2  a_cell_pos;    // (col, row)
layout(location = 2) in vec4  a_uv_rect;     // atlas UV (u0, v0, u1, v1)
layout(location = 3) in vec4  a_fg_color;    // resolved RGBA
layout(location = 4) in float a_is_color;    // 1.0 for color emoji
layout(location = 5) in vec2  a_glyph_offset; // sub-pixel offset from cell origin
layout(location = 6) in vec2  a_glyph_size;  // pixel size of the glyph rect
```

Instance data: 14 floats per glyph. More than the current 9 per-vertex, but **6x fewer
instances** (1 instance per glyph vs 6 vertices per glyph). Net data: 14 x 10K = 140K floats
vs 54 x 10K = 540K floats for the current approach.

The vertex shader maps the unit quad to the glyph rect and computes UVs.

#### What Stays the Same

- **Image pass:** Images are rare and per-image (not per-cell). The current approach of one
  textured quad per image is already efficient. No change needed.
- **Shaping pipeline:** `ShapingCache`, `ShapedLine`, `ShapedRun` — unchanged. The instance
  buffer builder reads from shaped data just like the current vertex builders do.
- **Glyph atlas:** `GlyphAtlas` — unchanged. Atlas UV lookup is the same; it just goes into
  instance data instead of per-vertex data.
- **Cursor-only fast path:** Still possible — patch one instance in the BG instance buffer
  (7 floats via `glBufferSubData`) instead of one quad (36 floats).
- **Double buffering:** Keep the `[2]` VBO pattern for instance buffers.

---

## Config & UI (unchanged from original plan)

### Config System

- Add `background_opacity: f32` to `UiConfig` with default `1.0` and validation `[0.0, 1.0]`.
- Update `config_example.toml` with documentation.

### Settings Modal

- Add opacity slider to `show_ui_tab()`.

### Platform Transparency

- Wayland: native support, works out of the box.
- macOS: Core Animation layers support transparent backgrounds.
- Windows: DWM supports transparent windows.
- X11: requires a running compositor (picom, compton, xcompmgr). Without one, transparent
  areas render as black. Documented in config and settings help text.

---

## Subtasks

---

### 34.1 — Add `background_opacity` to `UiConfig` and validation

**Status:** Complete
**Priority:** 1 — High
**Scope:** `freminal-common/src/config.rs`

**Details:**

1. Add `background_opacity: f32` to `UiConfig`:

   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   #[serde(default)]
   pub struct UiConfig {
       pub hide_menu_bar: bool,
       /// Background opacity (0.0 = fully transparent, 1.0 = fully opaque).
       /// Only affects the terminal and menu bar backgrounds; text and content
       /// remain fully opaque.
       pub background_opacity: f32,
   }
   ```

2. Update `UiConfig`'s `Default` impl (currently derived) to an explicit impl that sets
   `background_opacity: 1.0` (fully opaque — no visual change for existing users).

3. Add validation in `Config::validate()`:

   ```rust
   if !(0.0..=1.0).contains(&self.ui.background_opacity) {
       return Err(ConfigError::Validation(format!(
           "ui.background_opacity={} out of allowed range (0.0–1.0)",
           self.ui.background_opacity
       )));
   }
   ```

4. `ConfigPartial` already wraps `UiConfig` as `Option<UiConfig>`, so no change is needed
   to the partial merge machinery — the entire `UiConfig` section is replaced when present.

**Acceptance criteria:**

- `UiConfig::default().background_opacity` is `1.0`.
- `Config::validate()` rejects values outside `[0.0, 1.0]`.
- Existing configs without `background_opacity` deserialize correctly (default to `1.0`).
- `cargo test --all` passes.
- `cargo clippy --all-targets --all-features -- -D warnings` passes.

**Tests required:**

- Default config has `background_opacity == 1.0`.
- Validation accepts `0.0`, `0.5`, `1.0`.
- Validation rejects `-0.1`, `1.1`, `2.0`.
- Round-trip: serialize then deserialize preserves the value.
- Missing field in TOML defaults to `1.0` (backward compatibility).

---

### 34.2 — Update `config_example.toml`

**Status:** Complete
**Priority:** 2 — Medium
**Scope:** `config_example.toml`

**Details:**

Add the `background_opacity` field to the `[ui]` section with documentation:

```toml
# Background opacity (0.0 = fully transparent, 1.0 = fully opaque).
# All cell backgrounds are affected (default, colored, selection highlights).
# Text, images, cursor, underlines, and strikethrough remain fully opaque.
#
# Note: On X11, transparency requires a running compositor (e.g. picom).
# Without a compositor, transparent areas will render as black.
#
# background_opacity = 1.0
```

Place it after the `hide_menu_bar` entry.

**Acceptance criteria:**

- The example documents the field, its range, default, what it affects, and the X11 caveat.
- The field is commented out (default value) matching the convention of other optional fields.

**Tests required:** None (documentation only).

---

### 34.3 — Add opacity slider to the Settings Modal UI tab

**Status:** Complete
**Priority:** 2 — Medium
**Scope:** `freminal/src/gui/settings.rs`

**Details:**

In `show_ui_tab()`, add a slider for `background_opacity` after the `hide_menu_bar` checkbox:

```rust
fn show_ui_tab(&mut self, ui: &mut Ui) {
    ui.checkbox(&mut self.draft.ui.hide_menu_bar, "Hide Menu Bar");
    // ... existing help text ...

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    ui.label("Background Opacity:");
    ui.add(Slider::new(&mut self.draft.ui.background_opacity, 0.0..=1.0).step_by(0.05));
    ui.add_space(4.0);
    ui.colored_label(
        egui::Color32::GRAY,
        "Only affects backgrounds. Text and content remain fully opaque.",
    );
    ui.colored_label(
        egui::Color32::GRAY,
        "On X11, requires a running compositor (e.g. picom).",
    );
}
```

The `Slider` import already exists in `settings.rs` (line 6).

**Acceptance criteria:**

- The UI tab shows a slider with range `[0.0, 1.0]` and step `0.05`.
- The slider value is stored in `draft.ui.background_opacity`.
- Help text explains the effect and the X11 caveat.
- Clicking Apply persists the value to `config.toml`.

**Tests required:**

- Existing settings modal tests pass unchanged.
- No new tests needed (slider behavior is an egui widget; config persistence is covered by
  existing `try_apply` / `save_config` tests).

---

### 34.4 — Wire opacity into eframe viewport and egui panel fills

**Status:** Pending
**Priority:** 1 — High
**Scope:** `freminal/src/gui/mod.rs`, `freminal/src/gui/colors.rs`

**Details:**

This subtask handles everything outside the OpenGL renderer — the windowing system
transparency and the egui theme fills. The renderer changes come in later subtasks.

**A. Enable viewport transparency when opacity < 1.0.**

In the `run()` function (`gui/mod.rs`), after constructing `NativeOptions`, conditionally
enable transparency:

```rust
if config.ui.background_opacity < 1.0 {
    native_options.viewport.transparent = Some(true);
}
```

**B. Set panel fills with alpha.**

Add a helper function to `colors.rs`:

```rust
/// Map a `TerminalColor` to an egui `Color32` with an explicit alpha channel.
#[must_use]
pub fn internal_color_to_egui_with_alpha(
    color: TerminalColor,
    make_faint: bool,
    theme: &ThemePalette,
    alpha: f32,
) -> Color32 {
    let base = internal_color_to_egui(color, make_faint, theme);
    let [r, g, b, _] = base.to_array();
    let a = (alpha * 255.0) as u8;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}
```

Use it in `set_egui_options()` and `update_egui_theme()` to set `window_fill` and
`panel_fill` with the opacity alpha.

**Acceptance criteria:**

- When `background_opacity = 1.0`: no visual change.
- When `background_opacity < 1.0`: the egui panel/window fills are semi-transparent.
- `cargo test --all` passes.

**Tests required:**

- Unit test for `internal_color_to_egui_with_alpha`: verify alpha channel is set correctly.
- `internal_color_to_egui_with_alpha` with `alpha = 1.0` produces the same RGB as
  `internal_color_to_egui` (regression guard).

---

### 34.5 — Instanced background shader and instance buffer builder

**Status:** Pending
**Priority:** 1 — High
**Scope:** `freminal/src/gui/renderer.rs`

**Details:**

Replace the background shader program and vertex-building function with an instanced
approach.

**A. New background shaders.**

Vertex shader:

```glsl
#version 330 core

// Static unit-quad vertex (one of 6 triangle vertices for a quad).
layout(location = 0) in vec2 a_pos;          // (0,0), (1,0), (0,1), (1,1), ...

// Per-instance attributes (divisor = 1):
layout(location = 1) in vec2  a_cell_pos;    // (col, row) — integer grid position
layout(location = 2) in vec4  a_bg_color;    // resolved RGBA
layout(location = 3) in float a_cell_flags;  // 0.0 = cell bg, 1.0 = decoration/cursor

uniform vec2  u_viewport_size;
uniform float u_cell_width;
uniform float u_cell_height;

out vec4  v_bg_color;
out float v_flags;

void main() {
    vec2 cell_origin = a_cell_pos * vec2(u_cell_width, u_cell_height);
    vec2 pixel_pos = cell_origin + a_pos * vec2(u_cell_width, u_cell_height);
    vec2 ndc = (pixel_pos / u_viewport_size) * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    v_bg_color = a_bg_color;
    v_flags = a_cell_flags;
}
```

Fragment shader:

```glsl
#version 330 core

in vec4  v_bg_color;
in float v_flags;

out vec4 frag_color;

uniform float u_bg_opacity;

void main() {
    // flags > 0.5 means decoration/cursor — no opacity applied.
    float is_bg = step(v_flags + 0.5, 1.0);  // 1.0 when flags < 0.5 (cell bg)
    float opacity = mix(1.0, u_bg_opacity, is_bg);
    float alpha = v_bg_color.a * opacity;
    frag_color = vec4(v_bg_color.rgb * alpha, alpha);
}
```

**B. Static unit quad VBO.**

Create a static VBO containing the 6-vertex unit quad (two triangles):

```rust
const UNIT_QUAD: [f32; 12] = [
    0.0, 0.0,  1.0, 0.0,  0.0, 1.0,  // triangle 1
    1.0, 0.0,  1.0, 1.0,  0.0, 1.0,  // triangle 2
];
```

Uploaded once during `init()`. Never changes.

**C. Instance buffer builder.**

Replace `build_background_verts()` with `build_background_instances()`:

```rust
/// Per-instance data for one background element.
/// Layout: col, row, r, g, b, a, flags  (7 floats)
const BG_INSTANCE_FLOATS: usize = 7;

pub fn build_background_instances(
    shaped_lines: &[ShapedLine],
    cell_width: u32,   // still needed for decoration sub-cell positioning
    cell_height: u32,
    underline_offset: f32,
    strikeout_offset: f32,
    stroke_size: f32,
    show_cursor: bool,
    cursor_blink_on: bool,
    cursor_pos: CursorPos,
    cursor_visual_style: &CursorVisualStyle,
    selection: Option<(usize, usize, usize, usize)>,
    theme: &ThemePalette,
    cursor_color_override: Option<(u8, u8, u8)>,
) -> Vec<f32> {
    // For each cell with a non-default background:
    //   push (col, row, r, g, b, 1.0, 0.0)  // flags=0 → cell background
    //
    // For underline/strikethrough:
    //   push (x_start_px / cell_width, y_px / cell_height, r, g, b, a, 1.0)
    //   These need sub-cell positioning, so they use pixel-based coords
    //   divided by cell dimensions. The vertex shader handles it the same way.
    //
    // For cursor:
    //   push (cursor_col, cursor_row, r, g, b, 1.0, 1.0)  // flags=1 → no opacity
}
```

**Note on decorations and cursor:** Underlines, strikethrough, and non-block cursors
(underline cursor, vertical bar cursor) don't occupy full cells — they are sub-cell
rectangles. Two approaches:

1. **Keep decorations/cursor as explicit quads in a separate mini-VBO** drawn with the old
   non-instanced BG shader. This is the simplest — decorations are few (O(runs with
   underline), not O(cells)) and the cursor is 1 quad. The instanced pass handles the
   10,000 cell backgrounds; a tiny non-instanced pass handles ~50 decoration quads.

2. **Extend the instance format** with width/height overrides for sub-cell rectangles.

Option 1 is recommended — it keeps the instanced shader simple, avoids complicating the
instance format, and the decoration/cursor quad count is negligible.

**D. Draw call changes.**

In `draw_background()`:

```rust
// 1. Instanced cell backgrounds:
gl.use_program(Some(bg_instanced_program));
gl.uniform_2_f32(bg_u_viewport, vp_w, vp_h);
gl.uniform_1_f32(bg_u_cell_width, cell_w);
gl.uniform_1_f32(bg_u_cell_height, cell_h);
gl.uniform_1_f32(bg_u_bg_opacity, bg_opacity);
gl.bind_vertex_array(Some(bg_vao));
// ... bind unit quad VBO for location 0, instance VBO for locations 1-3 ...
gl.draw_arrays_instanced(TRIANGLES, 0, 6, instance_count);

// 2. Decoration/cursor quads (old non-instanced shader, tiny VBO):
gl.use_program(Some(decoration_program));  // original BG shader
// ... draw decorations ...
```

**E. Update `TerminalRenderer` struct.**

Add new fields for the instanced BG pass:

- `bg_instanced_program`, `bg_unit_quad_vbo`, `bg_instance_vbo: [Option<Buffer>; 2]`
- Uniform locations for `u_cell_width`, `u_cell_height`, `u_bg_opacity`

Keep the old BG program for decorations (renamed to `decoration_program`).

**Acceptance criteria:**

- Cell backgrounds render identically to the current renderer when `bg_opacity = 1.0`.
- `u_bg_opacity < 1.0` makes cell backgrounds semi-transparent.
- Decorations and cursor remain fully opaque.
- `cargo test --all` passes (existing `build_background_verts` tests adapted).

**Tests required:**

- Unit tests for `build_background_instances()`: verify instance count matches expected cells.
- Verify decoration/cursor instances have `flags = 1.0`.
- Verify cell background instances have `flags = 0.0`.
- Benchmark: instance buffer construction vs old `build_background_verts` — expect significant
  reduction in float count and construction time.

---

### 34.6 — Instanced foreground shader and instance buffer builder

**Status:** Pending
**Priority:** 1 — High
**Scope:** `freminal/src/gui/renderer.rs`

**Details:**

Replace the foreground shader and `build_foreground_verts()` with an instanced approach.

**A. New foreground shaders.**

Vertex shader:

```glsl
#version 330 core

layout(location = 0) in vec2  a_pos;          // unit quad vertex

// Per-glyph instance attributes (divisor = 1):
layout(location = 1) in vec2  a_glyph_origin;  // pixel position (top-left of glyph rect)
layout(location = 2) in vec2  a_glyph_size;    // pixel size of glyph rect
layout(location = 3) in vec4  a_uv_rect;       // (u0, v0, u1, v1) in atlas
layout(location = 4) in vec4  a_fg_color;      // resolved RGBA
layout(location = 5) in float a_is_color;      // 1.0 for color emoji

uniform vec2 u_viewport_size;

out vec2  v_uv;
out vec4  v_color;
out float v_is_color;

void main() {
    vec2 pixel_pos = a_glyph_origin + a_pos * a_glyph_size;
    vec2 ndc = (pixel_pos / u_viewport_size) * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);

    // Interpolate UV across the quad.
    v_uv = mix(a_uv_rect.xy, a_uv_rect.zw, a_pos);
    v_color = a_fg_color;
    v_is_color = a_is_color;
}
```

Fragment shader: identical to current `FG_FRAG_SRC` (no change needed — the glyph atlas
sampling and premultiplied alpha logic are the same).

**B. Instance buffer builder.**

Replace `build_foreground_verts()` with `build_foreground_instances()`:

Per-glyph instance: `glyph_x, glyph_y, glyph_w, glyph_h, u0, v0, u1, v1, r, g, b, a,
is_color` = **13 floats** per glyph.

For 10,000 glyphs: 130K floats (vs 540K for current approach). **4x reduction.**

**C. Draw call.**

```rust
gl.draw_arrays_instanced(TRIANGLES, 0, 6, glyph_instance_count);
```

**Acceptance criteria:**

- Text renders identically to current renderer.
- Color emoji renders correctly.
- Blink visibility still works (instances omitted for hidden blink).
- Selection foreground color override works.
- `cargo test --all` passes.

**Tests required:**

- Unit tests for `build_foreground_instances()`.
- Verify instance count, UV coordinates, color values.
- Benchmark: instance buffer vs old vertex builder.

---

### 34.7 — Update cursor-only fast path for instanced buffers

**Status:** Pending
**Priority:** 2 — Medium
**Scope:** `freminal/src/gui/renderer.rs`, `freminal/src/gui/terminal.rs`

**Details:**

The current cursor-only path patches a fixed region of the BG VBO via `glBufferSubData`.
Update this to patch the cursor instance in the decoration VBO (if the cursor is a
decoration quad) or the instanced BG buffer (if using a block cursor with a flags field).

Since decorations/cursor are in a separate mini-VBO (from subtask 34.5), the cursor-only
path remains simple: overwrite the cursor instance data (7 floats for decoration approach)
or zero it out when hidden.

**Acceptance criteria:**

- Cursor blink works without triggering a full rebuild.
- Cursor movement works without full rebuild.
- `draw_with_cursor_only_update` uses `glBufferSubData` on the correct buffer.

**Tests required:**

- Existing cursor-only tests adapted for new buffer layout.

---

### 34.8 — Remove old per-quad vertex builders

**Status:** Pending
**Priority:** 2 — Medium
**Scope:** `freminal/src/gui/renderer.rs`

**Details:**

Delete the old functions and constants that are now superseded:

- `build_background_verts()` — replaced by `build_background_instances()`
- `build_foreground_verts()` — replaced by `build_foreground_instances()`
- `push_quad()` — no longer used for cell backgrounds or glyphs
- `BG_VERTEX_FLOATS` (6), `FG_VERTEX_FLOATS` (9) — replaced by instance stride constants
- `CURSOR_QUAD_FLOATS` — replaced by new cursor instance size
- Old `draw()` method (the one that builds verts internally) — only `draw_with_verts` variant
  remains, now using instance buffers

Keep `push_quad()` if it's still used for the decoration mini-VBO.

**Acceptance criteria:**

- No dead code.
- `cargo clippy --all-targets --all-features -- -D warnings` passes.
- `cargo-machete` passes.

**Tests required:**

- All existing renderer tests updated to use new APIs.
- No test depends on deleted functions.

---

### 34.9 — Handle opacity changes on Apply (hot-reload)

**Status:** Pending
**Priority:** 2 — Medium
**Scope:** `freminal/src/gui/mod.rs`

**Details:**

When the user changes `background_opacity` in the Settings Modal and clicks Apply:

1. **Opacity changed while transparency was already enabled** (opacity was < 1.0 before):
   Update `u_bg_opacity` uniform on next frame. Panel fills updated via
   `update_egui_theme()`. Takes effect immediately.

2. **Opacity changed from 1.0 to < 1.0** (transparency was not enabled at startup):
   The viewport was created without `transparent = true`. Display a status message:
   "Restart required for transparency to take effect." Config saved; applies on next launch.

3. **Opacity changed from < 1.0 to 1.0** (disabling transparency):
   Update uniform and panel fills. Takes effect immediately.

The `u_bg_opacity` uniform is set every frame from the live config, so no special
uniform-update path is needed — the value automatically picks up the new config on the
next draw call.

**Acceptance criteria:**

- Changing opacity from 0.7 to 0.5: immediate.
- Changing opacity from 1.0 to 0.5: config saved, restart message shown.
- Changing opacity from 0.5 to 1.0: immediate.

**Tests required:**

- Manual verification only (requires running GUI context).

---

### 34.10 — Benchmark the instanced renderer

**Status:** Pending
**Priority:** 1 — High
**Scope:** `freminal/benches/render_loop_bench.rs`

**Details:**

Add benchmarks comparing the old vertex-building approach (captured baseline) with the new
instanced approach:

| Benchmark                     | What It Measures                                 |
| ----------------------------- | ------------------------------------------------ |
| `bench_bg_instances_80x24`    | `build_background_instances` for 80x24 terminal  |
| `bench_bg_instances_200x50`   | `build_background_instances` for 200x50 terminal |
| `bench_fg_instances_80x24`    | `build_foreground_instances` for 80x24 terminal  |
| `bench_fg_instances_200x50`   | `build_foreground_instances` for 200x50 terminal |
| `bench_instance_vs_vertex_bg` | Side-by-side comparison group                    |
| `bench_instance_vs_vertex_fg` | Side-by-side comparison group                    |

Expected results:

- BG instance construction: significantly faster (no quad merging, no per-vertex position math)
- FG instance construction: ~4x less data produced
- GPU draw: single instanced draw call vs thousands of vertices

**Acceptance criteria:**

- All benchmarks compile and run.
- Instance buffer construction is measurably faster than vertex construction.
- Results recorded in `PERFORMANCE_PLAN.md` Section 8.2.

**Tests required:** None (benchmarks only).

---

### 34.11 — Dead code cleanup and final verification

**Status:** Pending
**Priority:** 2 — Medium
**Scope:** Entire `freminal` crate

**Details:**

- Remove any remaining dead code from the renderer transition.
- Verify all `#[allow(...)]` attributes are still justified.
- Run full verification suite.
- Run benchmarks and record before/after numbers.

**Acceptance criteria:**

- `cargo test --all` passes.
- `cargo clippy --all-targets --all-features -- -D warnings` passes.
- `cargo-machete` passes.
- No dead code warnings.

**Tests required:** Full suite passes.

---

### 34.12 — Update plan documents and MASTER_PLAN

**Status:** Pending
**Priority:** 2 — Medium
**Scope:** `Documents/PLAN_34_BACKGROUND_OPACITY.md`, `Documents/MASTER_PLAN.md`

**Details:**

- Mark all subtasks complete with completion dates.
- Update MASTER_PLAN Task 34 status, completion tracking table, and "Complete:" line.
- Record benchmark results in PERFORMANCE_PLAN.md Section 8.2.

**Acceptance criteria:**

- All plan documents reflect actual completion state.
- MASTER_PLAN is consistent.

---

## Implementation Notes

### Subtask Ordering

```text
34.1 (config field) ──┬── 34.2 (example config)
                      ├── 34.3 (settings slider)
                      └── 34.4 (viewport + panel fills)

34.5 (instanced BG) ── 34.6 (instanced FG) ── 34.7 (cursor fast path)
                                               ── 34.8 (delete old builders)

34.4 + 34.5 ── 34.9 (hot-reload)

34.8 ── 34.10 (benchmark) ── 34.11 (cleanup) ── 34.12 (docs)
```

Subtasks 34.1-34.4 and 34.5-34.6 can proceed in parallel since they touch different files.
However, 34.5 and 34.6 should be sequential (both modify `renderer.rs`).

**Recommended linear order:** 34.1 → 34.2 → 34.3 → 34.4 → 34.5 → 34.6 → 34.7 → 34.8 →
34.9 → 34.10 → 34.11 → 34.12

### Risk Assessment

- **Medium risk overall.** The instanced renderer is a significant architectural change to the
  rendering pipeline. However:
  - The shaping pipeline is unchanged.
  - The snapshot/PTY architecture is unchanged.
  - The image pass is unchanged.
  - Each subtask leaves `cargo test --all` passing.
  - The old renderer code is not deleted until 34.8, so rollback is straightforward.

- **OpenGL 3.3 compatibility.** Instanced rendering is core in GL 3.3 (which we already
  require via `#version 330 core`). `glDrawArraysInstanced` and `glVertexAttribDivisor` are
  available in glow 0.17.0. No compatibility risk.

- **Background opacity risk: low.** A single `uniform float u_bg_opacity` with shader-side
  multiply is the simplest possible implementation. When `bg_opacity = 1.0`, the multiply is
  a no-op and rendering is identical.

- **Platform variability.** X11 without a compositor shows black for transparent areas.
  Documented. Not a bug.

### Interaction with Other Tasks

- **PERFORMANCE_PLAN.md Section 10:** This plan implements the "render path investigation"
  described there — replacing per-element draw calls with batched/instanced rendering. It
  does not address the `request_repaint()` rate capping (Part 2 of Section 10), which remains
  an independent optimization.
- **Task 1 (Custom Renderer):** This is a further evolution of the Task 1 renderer. All Task 1
  infrastructure (glow shaders, glyph atlas, font manager, shaping cache) is preserved.
- **Task 11 (Theming):** Theme palette colors are resolved CPU-side into instance data (same
  as current vertex data). Theme changes trigger a full rebuild (same as now).

### Verification

Each subtask must pass before proceeding:

- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo-machete`

---

## References

- `freminal-common/src/config.rs` — Config structs, `UiConfig`, validation
- `freminal/src/gui/mod.rs` — `set_egui_options()`, `update_egui_theme()`, `NativeOptions`
- `freminal/src/gui/colors.rs` — `internal_color_to_egui()`, `internal_color_to_gl()`, `rgb_to_f32()`
- `freminal/src/gui/renderer.rs` — `TerminalRenderer`, all shaders, vertex builders, draw calls
- `freminal/src/gui/terminal.rs` — `render_terminal_output()`, `RenderState`, paint callback
- `freminal/src/gui/shaping.rs` — `ShapingCache`, `ShapedLine`, `ShapedRun`
- `freminal/src/gui/settings.rs` — Settings modal, `show_ui_tab()`
- `config_example.toml` — Current config documentation
- `Documents/PERFORMANCE_PLAN.md` — Section 10 (render path investigation)
- glow 0.17.0 API: `draw_arrays_instanced`, `vertex_attrib_divisor`
