# PLAN_VERSION_050.md — v0.5.0 "Multi-Instance & Visual"

## Goal

Deliver differentiation features: multiple OS windows from a single instance, background
images, user-provided custom shaders for post-processing effects, and session restore /
startup commands.

---

## Task Summary

| #   | Feature                        | Scope  | Status  |
| --- | ------------------------------ | ------ | ------- |
| 53  | Multiple Windows               | Large  | Pending |
| 54  | Background Images              | Medium | Pending |
| 55  | Custom Shaders                 | Medium | Pending |
| 56  | Session Restore / Startup Cmds | Medium | Pending |
| 57  | Render Loop Optimization       | Medium | Pending |

---

## Task 53 — Multiple Windows

### 53 Overview

Allow opening multiple OS windows from a single Freminal process, sharing configuration,
theme state, and font resources. Currently each Freminal invocation is fully independent.

### 53 Design

**Application Model:** The current model is:

```text
main() → TerminalEmulator → PTY thread → gui::run() → eframe::run_native() → FreminalGui
```

`eframe::run_native()` creates a single window and blocks until it closes. For multiple
windows, the architecture needs to change to either:

**Option A — eframe viewports (egui 0.29+):** egui supports multiple viewports (windows)
from a single eframe application. Each viewport is a separate OS window but shares the same
`egui::Context`. This is the lightest-weight approach:

- Main window: the primary FreminalGui with its tab bar and terminal
- Additional windows: spawned via `ctx.show_viewport_immediate()` or deferred viewports
- Each window owns its own `TabManager` (set of tabs)
- Shared resources: `Config`, `FontManager`, glyph atlas, theme

**Option B — Multi-process with IPC:** Spawn a new OS process for each window but share
state via IPC (Unix socket or similar). Heavier, but more robust to individual window crashes.

Recommend **Option A** for simplicity and resource sharing.

**Window Management:**

- Menu bar: "Window" menu with "New Window" action
- Keyboard shortcut: Ctrl+Shift+N (new window)
- Each window is independent: its own tabs, selection, scroll state
- Closing the last window exits the application
- Window position/size can optionally be remembered per window

### 53 Subtasks

1. **53.1 — Research egui multi-viewport API**
   Investigate eframe/egui's viewport API. Determine how to create additional OS windows,
   share GL contexts (or create separate ones), and manage per-window state. Document findings
   and confirm feasibility of Option A.

2. **53.2 — Application lifecycle refactor**
   Refactor the application model to support multiple windows. Create a `WindowManager` that
   owns a collection of windows, each with its own `TabManager`. The main eframe app becomes
   a coordinator.

3. **53.3 — Per-window state**
   Each window owns: `TabManager`, `ViewState` per tab, window position/size. Shared across
   windows: `Config`, `FontManager`, glyph atlas texture.

4. **53.4 — Window creation and destruction**
   Implement "New Window" (creates a new viewport with an initial tab). Implement window close
   (closes all tabs in that window). Last window close exits the app.

5. **53.5 — Keyboard shortcut and menu integration**
   Add `KeyAction::NewWindow` to keybindings. Add "New Window" to the menu bar's "Window" menu.

6. **53.6 — Tests**
   Unit tests: window manager operations (create, close, last-window-exit). Integration:
   verify multiple windows can exist concurrently without resource conflicts.

### 53 Primary Files

- `freminal/src/gui/mod.rs` (application lifecycle)
- `freminal/src/gui/windows.rs` (new — window manager)
- `freminal/src/gui/tabs.rs` (per-window tab ownership)
- `freminal/src/main.rs` (startup changes)

---

## Task 54 — Background Images

### 54 Overview

Configurable background image behind the terminal grid, with opacity and optional blur.
Background opacity (Task 34) is already implemented — background images are the natural
extension.

### 54 Design

**Config:**

```toml
[ui]
# Path to a background image. Supports PNG, JPEG, WebP.
# background_image = "/path/to/image.png"

# How to fit the image: "fill" (stretch), "fit" (contain), "cover" (crop), "tile" (repeat).
# background_image_mode = "cover"

# Opacity of the background image (0.0–1.0). Layered under the terminal background.
# The existing background_opacity applies on top of this.
# background_image_opacity = 0.5
```

**Rendering:** The custom GL renderer (Task 1) already has a full shader pipeline. Adding a
background image requires:

1. Load the image and upload it as a GL texture.
2. Before rendering the terminal grid, draw a full-viewport quad textured with the background
   image, applying the configured fit mode and opacity.
3. The terminal's `background_opacity` then layers on top, creating a composited result.

**Image Loading:** Use the `image` crate (already a dependency for iTerm2/Kitty inline images).
Load on startup and when the config changes. Hot-reload on config file change.

### 54 Subtasks

1. **54.1 — Config: background image options**
   Add `background_image`, `background_image_mode`, `background_image_opacity` to `UiConfig`.
   Validation: file exists, opacity in range, mode is valid enum. Update config_example.toml,
   home-manager module.

2. **54.2 — Image loading and GL texture upload**
   Load the image from the configured path. Convert to RGBA. Upload as a GL texture via glow.
   Handle errors gracefully (missing file, unsupported format → log warning, no background).

3. **54.3 — Background quad rendering**
   Add a background pass to the renderer that draws a textured quad before the terminal grid.
   Implement fit modes: fill (stretch to viewport), fit (contain within viewport, letterboxed),
   cover (fill viewport, crop excess), tile (repeat).

4. **54.4 — Opacity compositing**
   Apply `background_image_opacity` to the background quad. Ensure it composes correctly
   with the existing `background_opacity` setting.

5. **54.5 — Hot-reload**
   When the config changes (settings modal save or file watcher), reload the background image.
   Handle image path changes and removal.

6. **54.6 — Tests**
   Unit tests: config parsing, fit mode calculations. Integration: verify image renders behind
   terminal text.

### 54 Primary Files

- `freminal-common/src/config.rs` (`UiConfig` extension)
- `freminal/src/gui/renderer/gpu.rs` (background quad rendering)
- `freminal/src/gui/renderer/shaders.rs` (background shader if needed)
- `config_example.toml`
- `nix/home-manager-module.nix`

---

## Task 55 — Custom Shaders

### 55 Overview

User-provided GLSL fragment shaders for post-processing effects (CRT scanlines, bloom,
color grading, etc.). Include a few example shaders for testing and demonstration.

### 55 Design

**Render Pipeline Change:** Currently the terminal is rendered directly to the screen. With
custom shaders, the pipeline becomes:

```text
1. Render terminal to an offscreen framebuffer (FBO / texture)
2. Apply the user's fragment shader as a fullscreen post-processing pass
3. Output the result to the screen
```

**Shader Interface:** The post-processing fragment shader receives:

- `uniform sampler2D u_terminal` — the terminal framebuffer as a texture
- `uniform vec2 u_resolution` — viewport size in pixels
- `uniform float u_time` — elapsed time in seconds (for animated effects)
- Standard `gl_FragCoord` / texture coordinates

The user writes only the fragment shader. The vertex shader for the fullscreen quad is
provided by Freminal.

**Config:**

```toml
[shader]
# Path to a custom GLSL fragment shader for post-processing.
# When set, the terminal is rendered to a framebuffer and the shader
# is applied as a fullscreen pass.
# path = "/path/to/shader.frag"

# Enable hot-reload: recompile the shader when the file changes.
# hot_reload = true
```

**Bundled Examples:** Include 3-4 example shaders in a `shaders/` directory:

- `crt.frag` — CRT scanlines + slight barrel distortion
- `bloom.frag` — Soft glow around bright text
- `grayscale.frag` — Convert output to grayscale
- `retro_amber.frag` — Amber phosphor monochrome look

**Error Handling:** If the shader fails to compile, log the error, show a warning in the
terminal (brief overlay or title bar message), and fall back to direct rendering.

### 55 Subtasks

1. **55.1 — Offscreen framebuffer setup**
   Create an FBO with a color texture attachment. Render the terminal to this FBO instead
   of directly to the screen.

2. **55.2 — Post-processing pass**
   Draw a fullscreen quad sampling the FBO texture through the user's fragment shader.
   Pass `u_terminal`, `u_resolution`, `u_time` uniforms.

3. **55.3 — Shader loading and compilation**
   Load the fragment shader from the configured path. Compile and link with the fullscreen
   vertex shader. Handle compilation errors gracefully.

4. **55.4 — Hot-reload**
   Watch the shader file for changes. Recompile on change. If compilation fails, keep the
   previous working shader active and log the error.

5. **55.5 — Config: shader options**
   Add `ShaderConfig` to config. Update config_example.toml, home-manager module.

6. **55.6 — Bundled example shaders**
   Write and include `crt.frag`, `bloom.frag`, `grayscale.frag`, `retro_amber.frag` in
   a `shaders/examples/` directory. Document each shader's effect.

7. **55.7 — Bypass when no shader configured**
   When `shader.path` is not set, render directly to the screen (current behavior). The FBO
   overhead is only incurred when a custom shader is active.

8. **55.8 — Tests**
   Unit tests: config parsing, shader uniform setup. Integration: verify the FBO pipeline
   does not regress rendering quality when using an identity (passthrough) shader.

### 55 Primary Files

- `freminal/src/gui/renderer/gpu.rs` (FBO, post-processing pass)
- `freminal/src/gui/renderer/shaders.rs` (shader loading, compilation)
- `freminal-common/src/config.rs` (`ShaderConfig`)
- `shaders/examples/` (new — bundled example shaders)
- `config_example.toml`
- `nix/home-manager-module.nix`

---

## Task 56 — Session Restore / Startup Commands

### 56 Overview

Configurable startup commands per tab and session layouts that restore a multi-tab arrangement.
Depends on tabs (Task 36, v0.3.0).

### 56 Design

**Startup Commands:** Each tab can have a startup command that runs after the shell launches:

```toml
[[startup.tabs]]
title = "Server"
command = "ssh user@server"

[[startup.tabs]]
title = "Logs"
command = "tail -f /var/log/syslog"
directory = "/var/log"

[[startup.tabs]]
title = "Dev"
directory = "~/projects/myapp"
```

On application launch, if `startup.tabs` is configured, create one tab per entry (instead of
the default single tab). Each tab's shell is started in the specified `directory` (if set) and
the `command` (if set) is sent as input after the shell is ready.

**Session Save/Restore (stretch goal):** Save the current tab layout (titles, working
directories, active commands) to a session file. Restore from a session file on startup.
This is a stretch goal — the minimum viable feature is the TOML startup configuration.

**Config:**

```toml
[startup]
# restore_last_session = false

[[startup.tabs]]
title = "Main"
# command = ""
# directory = ""
# shell = ""  # Override the default shell for this tab
```

### 56 Subtasks

1. **56.1 — Config: `[startup]` section**
   Add `StartupConfig` with a `tabs: Vec<StartupTabConfig>` field. Each `StartupTabConfig`
   has optional `title`, `command`, `directory`, `shell`. Update config_example.toml,
   home-manager module.

2. **56.2 — Startup tab creation**
   On application launch, if `startup.tabs` is non-empty, create tabs according to the
   configuration instead of a single default tab. Pass `directory` and `shell` to the PTY
   creation function.

3. **56.3 — Startup command injection**
   After a tab's shell is ready (small delay or detect prompt), send the `command` as
   keyboard input to that tab's PTY.

4. **56.4 — Tab title from config**
   If `title` is specified in the startup config, set it as the tab's initial title (before
   any OSC 0/2 from the shell overrides it).

5. **56.5 — Session save (stretch)**
   Add a "Save Session" menu item that captures the current tab layout to a TOML file.

6. **56.6 — Session restore (stretch)**
   Add a `--session` CLI flag and `startup.session_file` config option to restore from a
   saved session file.

7. **56.7 — Tests**
   Unit tests: config parsing, startup tab creation. Integration: launch with startup config,
   verify tabs are created with correct titles.

### 56 Primary Files

- `freminal-common/src/config.rs` (`StartupConfig`, `StartupTabConfig`)
- `freminal/src/gui/tabs.rs` (startup tab creation)
- `freminal/src/main.rs` (startup flow)
- `config_example.toml`
- `nix/home-manager-module.nix`

---

## Task 57 — Render Loop Optimization

### 57 Overview

Eliminate unnecessary CPU work during mouse movement and other input events that do not
change terminal content. Currently, `egui-winit` unconditionally returns `repaint: true`
for every `WindowEvent::CursorMoved`, triggering a full `update()` call — including snapshot
load, input processing, blink calculations, URL hover detection, PaintCallback allocation,
and OpenGL draw — even when nothing visible has changed. At 125 Hz mouse polling (standard)
this is 250 wasted frames/second; at 1000 Hz (gaming mice) it is 2000.

The same applies to keyboard input: key presses dispatch to the PTY and the result arrives
back as a snapshot change. The key press itself should never drive a repaint.

**Guiding principle:** Input events dispatch to the PTY but never drive repaints. Repaints
are driven only by observable state changes: new PTY content, cursor blink, text blink,
selection cell changes during active drag, and animations (cursor trail, bell flash).

### 57 Root Cause Analysis

**Hard constraint:** `egui-winit` 0.34.1 (`src/lib.rs:333-338`) hardcodes
`EventResponse { repaint: true }` for `WindowEvent::CursorMoved`. Additionally, a zero-delay
repaint sets `outstanding = 1` in egui's context (`context.rs:140`), meaning each
`CursorMoved` causes **two** frames. This is upstream behaviour that cannot be changed without
forking `egui-winit`.

**Consequence:** We cannot prevent `update()` from being called on every mouse movement. But
we can make those frames cost near-zero by short-circuiting all expensive work when nothing
meaningful has changed.

### 57 Design

The design has four independent sub-optimisations, ordered from highest to lowest impact:

#### A. URL hover fast-path (highest impact)

Current cost: **O(visible_chars) + O(tags)** per mouse-move pixel.

`flat_index_for_cell()` (`coords.rs:43-81`) linearly scans the flat `visible_chars` vec
looking for `NewLine` separators to find the target row, then walks within the row. For a
50×220 terminal this is up to ~11,000 iterations. Then `visible_tags.iter().find()` does a
linear scan of all tags. This runs on every single pixel of mouse movement even when there
are zero URLs in the entire visible content.

**New data structures** (built during `rows_as_tchars_and_tags_cached`, zero extra cost
since we already iterate tags):

1. **`has_urls: bool`** on `TerminalSnapshot` — set `true` if any tag in the visible
   window has `url.is_some()`. When `false`, the entire URL hover code path is skipped.
   This is the fast path for ~99% of terminal usage.

2. **`row_offsets: Arc<Vec<usize>>`** on `TerminalSnapshot` — one entry per visible row,
   `row_offsets[r]` = flat index in `visible_chars` where row `r` begins. Replaces the
   O(visible_chars) scan in `flat_index_for_cell` with O(1) row lookup + O(row_width)
   column walk. Benefits all callers (URL hover, selection hit-testing).

3. **`url_tag_indices: Arc<Vec<usize>>`** on `TerminalSnapshot` — indices into
   `visible_tags` of tags with `url.is_some()`. Replaces O(all_tags) linear scan with
   O(url_tags) — typically O(0) or O(1).

**Cell-change gating:** Track the previous hovered cell `(col, row)`. Only run URL
detection when the cell changes. Only call `output_mut(cursor_icon)` when the icon
actually needs to change (track previous icon).

| Scenario              | Before                               | After                      |
| --------------------- | ------------------------------------ | -------------------------- |
| No URLs, mouse moving | O(visible_chars) + O(tags) per pixel | O(0) — `has_urls` skip     |
| URLs exist, same cell | O(visible_chars) + O(tags) per pixel | O(0) — cell-change gate    |
| URLs exist, new cell  | O(visible_chars) + O(tags)           | O(row_width) + O(url_tags) |
| Cursor icon unchanged | `output_mut` every frame             | Skipped                    |

#### B. Gate `global_style_mut` on actual changes (minor)

`global_style_mut` calls `Arc::make_mut` on the egui `Style` every frame, cloning the Arc
even when the background color and theme haven't changed. Track the previous
`(is_normal_display, theme_ptr, bg_opacity)` tuple and only call `global_style_mut` when
any of those change.

#### C. Cache PaintCallback allocation (minor)

Every frame allocates `Arc::new(CallbackFn::new(...))` even when the GPU data is identical
to the previous frame. Investigate whether the PaintCallback can be cached and reused when
no vertex data changed. This may require changes to the egui PaintCallback API compatibility
(the closure captures change each frame for `is_cursor_only` / `cursor_only_verts`), so this
subtask begins with a feasibility check.

#### D. Worker thread for vertex generation (rejected)

Considered and rejected. Benchmark data shows:

| Operation                    | 80×24   | 200×50   |
| ---------------------------- | ------- | -------- |
| `build_bg_instances`         | ~140 ns | ~276 ns  |
| `build_fg_instances`         | ~941 µs | ~1.50 ms |
| `shape_visible` (cache miss) | —       | ~1.82 ms |
| `shape_visible` (cache hit)  | —       | ~14.2 µs |

The full rebuild path is ~3.3 ms worst case, fitting comfortably within the PTY thread's
8 ms repaint cadence. The shaping cache reduces repeat frames to ~1.5 ms. The "nothing
changed" path (which is what mouse-move frames hit) does zero vertex generation — just
re-renders cached VBOs.

A worker thread would add latency (one extra frame delay between content arrival and
display), complicate ownership (selection and cursor state live on the GUI thread), and
solve a problem that content-change gating already handles. The complexity is not justified.

### 57 Subtasks

1. **57.1 — `has_urls` snapshot flag**
   During `rows_as_tchars_and_tags_cached` in `buffer.rs`, track whether any tag in the
   visible window has `url.is_some()`. Store as `has_urls: bool` on the snapshot. Zero-cost
   check (piggybacked on the existing tag iteration that runs only when content is dirty).
   Add to `TerminalSnapshot`, `build_snapshot()`, and relevant tests.

2. **57.2 — `row_offsets` index table**
   During `rows_as_tchars_and_tags_cached`, record the flat index where each row begins in
   `visible_chars`. Store as `row_offsets: Arc<Vec<usize>>` on the snapshot. Modify
   `flat_index_for_cell` to accept the row offsets and skip the O(visible_chars) row scan.
   Update all callers. Add benchmarks comparing before/after for `flat_index_for_cell`.

3. **57.3 — `url_tag_indices` lookup array**
   During `rows_as_tchars_and_tags_cached`, collect indices of tags with `url.is_some()`.
   Store as `url_tag_indices: Arc<Vec<usize>>` on the snapshot. Modify URL hover detection
   in `widget.rs` to iterate only URL-bearing tags instead of all tags.

4. **57.4 — Cell-change gating for URL hover**
   Add `previous_hover_cell: (usize, usize)` and `previous_cursor_icon: CursorIcon` to
   `FreminalTerminalWidget`. Only run URL detection when the hovered cell changes. Only call
   `output_mut(cursor_icon)` when the icon actually changes. When `snap.has_urls` is false,
   skip the entire block (including `encode_egui_mouse_pos_as_usize`).

5. **57.5 — Gate `global_style_mut`**
   Track previous `(is_normal_display, theme pointer, bg_opacity)` in `FreminalGui`. Only
   call `global_style_mut` when any of those change. Eliminates per-frame `Arc::make_mut`
   clone of the egui Style.

6. **57.6 — PaintCallback caching feasibility**
   Investigate whether the `egui::PaintCallback` (and its `Arc<CallbackFn>`) can be cached
   and reused across frames when no vertex data changed. If feasible, implement. If the egui
   API requires a new callback each frame, document the finding and close this subtask as
   not-feasible.

7. **57.7 — Benchmarks and verification**
   Measure before/after CPU usage during sustained mouse movement (no content change, no
   URLs, sitting at prompt). Record frame time under mouse movement with `TRACE`-level frame
   logging. Run the full verification suite. Capture benchmark numbers for any
   buffer/renderer benchmarks affected by the new snapshot fields.

### 57 Primary Files

- `freminal-buffer/src/buffer.rs` (`rows_as_tchars_and_tags_cached` — build new indices)
- `freminal-terminal-emulator/src/snapshot.rs` (new snapshot fields)
- `freminal-terminal-emulator/src/interface.rs` (`build_snapshot`, `flatten_visible`)
- `freminal/src/gui/terminal/widget.rs` (URL hover gating, PaintCallback caching)
- `freminal/src/gui/terminal/coords.rs` (`flat_index_for_cell` optimisation)
- `freminal/src/gui/mod.rs` (`global_style_mut` gating)

### 57 Rejected Alternatives

**Fork egui-winit:** Would allow returning `repaint: false` for `CursorMoved`, eliminating
wasted frames entirely. Rejected due to maintenance burden of carrying a fork across egui
version upgrades. The "make wasted frames near-zero cost" approach achieves the same CPU
savings without a fork.

**Worker thread for vertex generation:** See section D above. Rejected due to added latency,
ownership complexity, and the fact that content-change gating already prevents vertex
generation on input-only frames.

**Strip `PointerMoved` from `raw_input_hook`:** Would prevent egui from knowing the mouse
moved, but also breaks selection drag, URL hover, context menus, and cursor shape changes.
Requires re-implementing mouse tracking outside egui, duplicating work. Rejected.

---

## Dependency Graph

```text
Task 53 (Multiple Windows) ── builds on tabs (Task 36, v0.3.0)
Task 54 (Background Images) ── extends background_opacity (Task 34, complete)
Task 55 (Custom Shaders) ── extends the GL renderer (Task 1, complete)
Task 56 (Session Restore) ── depends on tabs (Task 36, v0.3.0)
Task 57 (Render Loop Opt) ── independent; touches buffer, snapshot, and GUI

Tasks 54 and 55 are independent of each other.
Tasks 53 and 56 both depend on tabs but are independent of each other.
Task 57 is independent of all other v0.5.0 tasks.
```

**Recommended order:** Task 57 can start immediately — it is independent and reduces CPU
overhead for all subsequent development and testing. 54 and 55 can also start immediately
(GL renderer work). 53 and 56 require tabs from v0.3.0 to be complete and stable.

---

## Completion Criteria

Per `agents.md`, each task is complete when:

1. All subtasks marked complete
2. `cargo test --all` passes
3. `cargo clippy --all-targets --all-features -- -D warnings` passes
4. `cargo-machete` passes
5. Benchmarks show no unexplained regressions for render/buffer changes
6. Config schema additions propagated to config.rs, config_example.toml, home-manager, settings
