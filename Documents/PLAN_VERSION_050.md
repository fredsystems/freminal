# PLAN_VERSION_050.md — v0.5.0 "Multi-Instance & Visual"

## Goal

Deliver differentiation features: multiple OS windows from a single instance, background
images, user-provided custom shaders for post-processing effects, and session restore /
startup commands.

---

## Task Summary

| #   | Feature                        | Scope  | Status   |
| --- | ------------------------------ | ------ | -------- |
| 53  | Multiple Windows               | Large  | Pending  |
| 54  | Background Images              | Medium | Pending  |
| 55  | Custom Shaders                 | Medium | Pending  |
| 56  | Session Restore / Startup Cmds | Medium | Pending  |
| 57  | Render Loop Optimization       | Medium | Complete |
| 58  | Built-in Multiplexer           | Large  | Pending  |

---

## Task 53 — Multiple Windows

### 53 Overview

Allow opening multiple OS windows from a single Freminal process, sharing configuration,
theme state, and font resources. Currently each Freminal invocation is fully independent.

**Known issue:** Some application launchers (e.g., vicinae) refuse to spawn a second
Freminal instance when one is already running. This is because the `.desktop` entry does
not signal that multiple instances are expected. The desktop entry in `flake.nix` needs
`StartupWMClass` and potentially other flags. Additionally, invocations like
`freminal yazi --no-menu-bar` from compositor keybindings must spawn fully isolated
instances that are not coalesced with existing windows. This task must address both the
multi-window-from-one-process model (Option A) and the multi-process launch model (fixing
the `.desktop` entry so independent instances always work).

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

1. **53.1 — Fix `.desktop` entry for multi-instance launch**
   Update the desktop entry in `flake.nix` to ensure application launchers always spawn a
   new process. Add `StartupWMClass=freminal` and investigate whether `SingleMainWindow=false`
   or other freedesktop keys are needed. Verify that `freminal yazi --no-menu-bar` launched
   from a compositor keybinding creates a fully isolated instance regardless of whether
   another Freminal window is already open. This subtask can be completed independently of
   the multi-viewport work below and provides immediate relief.

2. **53.2 — Research egui multi-viewport API**
   Investigate eframe/egui's viewport API. Determine how to create additional OS windows,
   share GL contexts (or create separate ones), and manage per-window state. Document findings
   and confirm feasibility of Option A.

3. **53.3 — Application lifecycle refactor**
   Refactor the application model to support multiple windows. Create a `WindowManager` that
   owns a collection of windows, each with its own `TabManager`. The main eframe app becomes
   a coordinator.

4. **53.4 — Per-window state**
   Each window owns: `TabManager`, `ViewState` per tab, window position/size. Shared across
   windows: `Config`, `FontManager`, glyph atlas texture.

5. **53.5 — Window creation and destruction**
   Implement "New Window" (creates a new viewport with an initial tab). Implement window close
   (closes all tabs in that window). Last window close exits the app.

6. **53.6 — Keyboard shortcut and menu integration**
   Add `KeyAction::NewWindow` to keybindings. Add "New Window" to the menu bar's "Window" menu.

7. **53.7 — Tests**
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

1. **57.1 — `has_urls` snapshot flag** ✅ Complete.
   During `rows_as_tchars_and_tags_cached` in `buffer.rs`, track whether any tag in the
   visible window has `url.is_some()`. Store as `has_urls: bool` on the snapshot. Zero-cost
   check (piggybacked on the existing tag iteration that runs only when content is dirty).
   Add to `TerminalSnapshot`, `build_snapshot()`, and relevant tests.

2. **57.2 — `row_offsets` index table** ✅ Complete.
   During `rows_as_tchars_and_tags_cached`, record the flat index where each row begins in
   `visible_chars`. Store as `row_offsets: Arc<Vec<usize>>` on the snapshot. Modify
   `flat_index_for_cell` to accept the row offsets and skip the O(visible_chars) row scan.
   Update all callers. Add benchmarks comparing before/after for `flat_index_for_cell`.

3. **57.3 — `url_tag_indices` lookup array** ✅ Complete.
   During `rows_as_tchars_and_tags_cached`, collect indices of tags with `url.is_some()`.
   Store as `url_tag_indices: Arc<Vec<usize>>` on the snapshot. Modify URL hover detection
   in `widget.rs` to iterate only URL-bearing tags instead of all tags.

4. **57.4 — Cell-change gating for URL hover** ✅ Complete.
   Add `previous_hover_cell: (usize, usize)` and `previous_cursor_icon: CursorIcon` to
   `FreminalTerminalWidget`. Only run URL detection when the hovered cell changes. Only call
   `output_mut(cursor_icon)` when the icon actually changes. When `snap.has_urls` is false,
   skip the entire block (including `encode_egui_mouse_pos_as_usize`).

5. **57.5 — Gate `global_style_mut`** ✅ Complete.
   Track previous `(is_normal_display, theme pointer, bg_opacity)` in `FreminalGui`. Only
   call `global_style_mut` when any of those change. Eliminates per-frame `Arc::make_mut`
   clone of the egui Style.

6. **57.6 — PaintCallback caching feasibility** ✅ Closed: not feasible.
   egui rebuilds its shape list from scratch every frame — `ui.painter().add()` is the only
   way to submit draw commands, and there is no "reuse last frame's shapes" API. The
   `CallbackFn` closure captures per-frame state (`is_cursor_only`, `cursor_only_verts`)
   that changes each frame, so the closure itself cannot be reused. Even if we restructured
   captures via `Arc<Mutex<…>>`, we would still allocate a `PaintCallback` shape each frame.
   The cost of `Arc::new(CallbackFn::new(…))` is a single heap allocation per frame
   (~microseconds) — negligible compared to actual GL draw calls. No code change needed.

7. **57.7 — Benchmarks and verification** ✅ Complete.
   Full verification suite passes: `cargo test --all`, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo-machete`. Benchmark results (post-Task 57):

   | Benchmark                                        | Time    | Throughput   |
   | ------------------------------------------------ | ------- | ------------ |
   | `bench_visible_flatten/visible_200x50`           | 4.59 µs | 2.18 Gelem/s |
   | `bench_scrollback_flatten/scrollback_1024_rows`  | 5.02 ns | 16.3 Gelem/s |
   | `bench_scrollback_render/offset/0`               | 1.30 µs | 1.48 Gelem/s |
   | `bench_scrollback_render/offset/1000`            | 1.27 µs | 1.51 Gelem/s |
   | `bench_scrollback_render/offset/4000`            | 1.24 µs | 1.55 Gelem/s |
   | `bench_data_and_format_for_gui/flatten_80x24`    | 1.18 µs | 1.63 Gelem/s |
   | `bench_build_snapshot/dirty_80x24`               | 41.8 µs | 45.9 Melem/s |
   | `bench_build_snapshot/clean_80x24`               | 150 ns  | 12.8 Gelem/s |
   | `bench_build_snapshot_scrollback/dirty_10k`      | 1.04 ms | 1.85 Melem/s |
   | `bench_build_snapshot_scrollback/clean_10k`      | 157 ns  | 12.2 Gelem/s |
   | `render_terminal_text_snapshot/build_after_ansi` | 50.9 µs | 37.8 Melem/s |

   No Criterion regressions reported. The primary CPU savings come from gating (skipping
   work on mouse-movement frames), which is not captured by dirty-path benchmarks. Manual
   testing with mouse movement is needed to verify the idle CPU reduction.

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

## Task 58 — Built-in Multiplexer (Split Panes)

### 58 Overview

Add built-in terminal multiplexing: horizontal and vertical split panes within a tab, with
keyboard-driven navigation, resize, close, and zoom. This replaces the need for tmux/zellij
for local workflows and gives Freminal native ownership of every pane's scrollback — meaning
search, selection, and copy mode work natively across all panes without fighting an external
multiplexer's alt-screen buffer.

**Subsumes:** A.2 (Split Panes) from `FUTURE_PLANS.md`. That item is now tracked here.

**Explicitly out of scope for this task:**

- Remote mux / SSH domains / detach-reattach (B.1 in `FUTURE_PLANS.md` — remains deferred)
- Status bar (deferred to a future task)
- Session save/restore of pane layouts (handled by Task 56's stretch goals)
- Multiple OS windows (Task 53 — separate, coexists)

### 58 Motivation

The user relies on tmux daily. tmux works inside Freminal, but because tmux manages its own
scrollback in the alternate screen buffer, Freminal has no access to pane scrollback content.
Built-in muxing means Freminal owns every pane's `TerminalEmulator` and `Buffer` natively —
search, selection, copy mode, and scroll all Just Work without fighting an external
multiplexer. The goal is "90% of tmux" for local use: split, navigate, resize, close, zoom.

### 58 Architecture

#### Current Model (one emulator per tab)

```text
TabManager
  └── Tab
        ├── Arc<ArcSwap<TerminalSnapshot>>   (one snapshot)
        ├── Sender<InputEvent>                (one input channel)
        ├── Sender<PtyWrite>                  (one write channel)
        ├── Receiver<WindowCommand>           (one cmd channel)
        ├── Receiver<()>                      (pty death)
        ├── ViewState                         (one view state)
        └── …metadata (title, bell, echo_off)
```

#### New Model (pane tree per tab)

```text
TabManager
  └── Tab
        ├── PaneTree                          (binary tree of panes)
        │     ├── Leaf: Pane { channels, view_state, title, … }
        │     └── Split { direction, ratio, left, right }
        ├── active_pane: PaneId               (which pane has focus)
        └── zoomed_pane: Option<PaneId>       (if zoomed, only this pane renders)
```

Each `Pane` holds the fields currently on `Tab`:

```rust
pub struct Pane {
    pub id: PaneId,
    pub arc_swap: Arc<ArcSwap<TerminalSnapshot>>,
    pub input_tx: Sender<InputEvent>,
    pub pty_write_tx: Sender<PtyWrite>,
    pub window_cmd_rx: Receiver<WindowCommand>,
    pub clipboard_rx: Receiver<String>,
    pub search_buffer_rx: Receiver<(usize, Vec<TChar>)>,
    pub pty_dead_rx: Receiver<()>,
    pub title: String,
    pub bell_active: bool,
    pub title_stack: Vec<String>,
    pub view_state: ViewState,
    pub echo_off: Arc<AtomicBool>,
}
```

`Tab` becomes a thin wrapper around `PaneTree` + focus tracking. A tab with no splits is
simply a `PaneTree` with a single leaf — zero overhead compared to today.

#### PaneTree (binary tree)

The pane layout is a binary tree. Leaves are `Pane` instances. Internal nodes are splits:

```rust
pub enum PaneTree {
    Leaf(Pane),
    Split {
        direction: SplitDirection,  // Horizontal | Vertical
        ratio: f32,                 // 0.0–1.0, position of the divider
        first: Box<PaneTree>,       // left/top child
        second: Box<PaneTree>,      // right/bottom child
    },
}

pub enum SplitDirection {
    Horizontal,  // divider is horizontal → panes stack top/bottom
    Vertical,    // divider is vertical → panes stack left/right
}
```

This is the same approach used by WezTerm (`Tree<Arc<dyn Pane>, SplitDirectionAndSize>`)
but simplified: no trait objects, no remote pane abstraction, no Arc indirection. The tree
is owned entirely by the GUI thread (pane channels are the only shared state).

**Why a binary tree?** It naturally represents nested splits of any depth. Splitting a pane
replaces its leaf node with an internal Split node whose children are the original pane and
the new pane. Closing a pane removes it and collapses the parent Split back to the sibling.
No grid layout math, no constraint solver — just recursive subdivision.

#### Rendering

Each pane is rendered into its allocated `egui::Rect` within the `CentralPanel`. The layout
algorithm recursively subdivides the available rect according to split direction and ratio:

```text
fn layout(tree: &PaneTree, rect: Rect) -> Vec<(PaneId, Rect)> {
    match tree {
        Leaf(pane) => vec![(pane.id, rect)],
        Split { direction, ratio, first, second } => {
            let (r1, r2) = split_rect(rect, direction, ratio);
            [layout(first, r1), layout(second, r2)].concat()
        }
    }
}
```

Pane borders are rendered as thin lines (1-2px) between adjacent panes. The focused pane's
border is highlighted with the theme's accent color.

**Zoom mode:** When a pane is zoomed, only that pane renders (using the full `CentralPanel`
rect). All other panes continue receiving PTY output and draining channels but are not drawn.
A visual indicator (e.g., `[Z]` in the tab title or a subtle overlay badge) signals that
zoom is active.

#### Input Routing

- **Keyboard input** goes to the active pane's `input_tx` only.
- **Mouse input** goes to whichever pane the mouse cursor is over (hit-test against pane
  rects). Clicking in a pane also sets it as the active pane.
- **Resize events** are per-pane: when the window resizes or a split ratio changes, each
  pane receives its own `InputEvent::Resize` with its new dimensions.

#### Pane Lifecycle

- **Split:** Takes the focused pane, replaces it with a Split node. The original pane
  becomes `first`, a new pane (new PTY via `spawn_pty_tab`) becomes `second`.
- **Close:** Removes the pane from the tree. Its parent Split collapses — the sibling
  becomes the parent's replacement. If it was the last pane in the tab, the tab closes.
- **PTY death:** Same as today — when `pty_dead_rx` fires, the pane is closed (tree
  collapse). If it was the last pane, the tab closes.

### 58 Design Decisions

1. **Local-only muxing.** No Domain trait, no remote protocol, no SSH integration, no
   detach/reattach. This keeps the implementation scope manageable and focused on the
   primary use case (replacing tmux for local splits). Remote mux remains B.1.

2. **Binary tree, not grid.** A binary tree is simpler to implement, handles arbitrary
   nesting, and matches WezTerm's proven approach. Grid layouts can be simulated by
   nesting horizontal splits inside vertical splits (or vice versa).

3. **Ratio-based dividers.** Each split stores a `ratio: f32` (0.0–1.0) rather than
   absolute pixel sizes. This makes resize propagation trivial — ratios are preserved
   when the window resizes.

4. **No status bar.** Deferred. Pane focus is indicated by border highlighting. Pane
   information (title, working directory) can be shown in the tab bar or a future status
   bar.

5. **Pane tree lives on the GUI thread.** The tree structure is only needed for layout
   and rendering. PTY threads don't know about the tree — they just own their
   `TerminalEmulator` and publish snapshots as before. This preserves the lock-free
   architecture.

6. **`spawn_pty_tab` reused for pane creation.** The existing function creates a
   `TerminalEmulator` and returns `TabChannels`. A new pane calls this same function
   and wraps the result in a `Pane` struct. No new PTY spawning code needed.

### 58 Keybindings

All multiplexer actions go through the `BindingMap` system (per `agents.md` keybinding
convention). Default bindings use `Ctrl+Shift+` prefix to avoid conflict with shell and
application shortcuts:

| Action            | Default Binding | `KeyAction` variant |
| ----------------- | --------------- | ------------------- |
| Split vertical    | Ctrl+Shift+Pipe | `SplitVertical`     |
| Split horizontal  | Ctrl+Shift+\_   | `SplitHorizontal`   |
| Close pane        | Ctrl+Shift+W    | `ClosePane`         |
| Navigate left     | Ctrl+Shift+H    | `FocusPaneLeft`     |
| Navigate down     | Ctrl+Shift+J    | `FocusPaneDown`     |
| Navigate up       | Ctrl+Shift+K    | `FocusPaneUp`       |
| Navigate right    | Ctrl+Shift+L    | `FocusPaneRight`    |
| Resize grow left  | Ctrl+Alt+H      | `ResizePaneLeft`    |
| Resize grow down  | Ctrl+Alt+J      | `ResizePaneDown`    |
| Resize grow up    | Ctrl+Alt+K      | `ResizePaneUp`      |
| Resize grow right | Ctrl+Alt+L      | `ResizePaneRight`   |
| Zoom/unzoom pane  | Ctrl+Shift+Z    | `ZoomPane`          |

Note: `Ctrl+Shift+W` currently closes a tab. With muxing, it closes the focused pane. If
the pane is the last in its tab, the tab closes (same end result). This is consistent with
how tmux's `prefix x` works.

### 58 Subtasks

1. **58.1 — `PaneId` and `Pane` struct** — **COMPLETE** (2026-04-09)
   Define `PaneId` (monotonic newtype, like `TabId`). Extract per-terminal fields from `Tab`
   into a new `Pane` struct in `freminal/src/gui/panes.rs`. `Pane` holds all the channel
   endpoints, `ViewState`, title, bell state, and `echo_off` that currently live on `Tab`.
   Add unit tests for `PaneId` generation.
   _Commit: `8f8fc06` — `PaneId`, `PaneIdGenerator`, `Pane` struct, custom `Debug` impl,
   11 unit tests._

2. **58.2 — `PaneTree` data structure** — **COMPLETE** (2026-04-09)
   Implement `PaneTree` enum (`Leaf`/`Split`) in `freminal/src/gui/panes/mod.rs`. Core
   operations:
   - `layout(rect) -> Vec<(PaneId, Rect)>` — recursive layout computation
   - `find(id) -> Option<&Pane>` / `find_mut(id) -> Option<&mut Pane>`
   - `iter_panes()` / `iter_panes_mut()` — iterate all leaves
   - `pane_count() -> usize`
   - `split(target_id, direction) -> Result<PaneId>` — split a leaf, returns new pane's id
   - `close(target_id) -> Result<ClosedPaneResult>` — remove a leaf, collapse parent
   - `resize_split(target_id, direction, delta)` — adjust the split ratio of the nearest
     ancestor split in the given direction
     Thorough unit tests: split, close, layout math, nested trees, edge cases (close last
     pane, deep nesting, unbalanced trees).
     _Commit: `333dcb4` — PaneTree with PaneNode (Leaf/Split), SplitDirection, PaneError,
     ClosedPaneResult, split_rect helper, 35+ unit tests. Pane module converted to directory._

3. **58.3 — Refactor `Tab` to use `PaneTree`** ✅ _Complete._
   Replace `Tab`'s direct channel/view-state fields with a `PaneTree` and `active_pane:
PaneId`. Add `zoomed_pane: Option<PaneId>`. The single-pane case (no splits) is a
   `PaneTree::Leaf` — functionally identical to today. Migrate all call sites in
   `mod.rs` that access `tab.arc_swap`, `tab.input_tx`, etc. to go through the active
   pane: `tab.active_pane().arc_swap`, etc. Ensure all existing tab functionality works
   unchanged. Run the full test suite to verify no regressions.

   _Completion note: `Tab` struct replaced with `{id, pane_tree, active_pane, zoomed_pane}`.
   `Tab::new()` constructor and `active_pane()` / `active_pane_mut()` accessors added. All
   27 call-site groups in `mod.rs` migrated — including the window command drain loop
   (heaviest site), terminal widget show, theme broadcasting (all-pane iteration), PTY death
   polling, resize debounce, scroll offset sync, font zoom, and settings theme changes.
   `main.rs` tab construction updated (both normal and playback modes). `tabs.rs` test module
   rewritten: `dummy_tab()` now creates a `Pane` + `Tab::new()`, all field accesses go
   through `active_pane()` / `active_pane_mut()`, Debug test updated for new output format.
   All 335 tests pass, clippy clean, no unused deps._

4. **58.4 — Pane layout rendering**
   Modify `FreminalGui::ui()` to compute pane rects via `PaneTree::layout()` and render
   each visible pane into its allocated rect. The `FreminalTerminalWidget::show()` call
   needs to accept a rect parameter (or use `ui.allocate_rect()`). Render pane borders
   between adjacent panes. Highlight the focused pane's border.

5. **58.5 — Input routing**
   Route keyboard input to the active pane. Route mouse input to the pane under the cursor
   (hit-test against pane rects from the layout pass). Clicking in a pane sets it as active.
   Per-pane resize: when the window resizes or a split ratio changes, compute each pane's
   new `(width_chars, height_chars)` and send `InputEvent::Resize` to each affected pane.

6. **58.6 — Split operations**
   Implement split-vertical and split-horizontal: create a new pane (via `spawn_pty_tab`),
   insert it into the tree at the focused pane's location. The focused pane stays in `first`,
   the new pane goes in `second`. Focus moves to the new pane. Wire up the keybindings.
   **CWD inheritance:** New panes inherit the parent pane's current working directory by
   default (read via `/proc/<pid>/cwd` on Linux, or the `cwd` field on the snapshot). A
   config option (`[panes] inherit_cwd = true`) controls this — when false, new panes use
   the user's default shell CWD (or a configured `default_directory`).

7. **58.7 — Close pane**
   Implement pane close: remove the pane from the tree, collapse the parent split. Focus
   moves to the sibling. If the closed pane was the last in the tab, close the tab.
   Handle PTY death: when `pty_dead_rx` fires for a pane, trigger the same close logic.
   Wire up the keybinding.

8. **58.8 — Directional navigation**
   Implement `FocusPaneLeft/Down/Up/Right`: from the focused pane's rect, find the nearest
   pane in the given direction (by comparing rect centers/edges). Move focus to it. Wrap
   behavior: no-op at edges (do not wrap around). Wire up keybindings.

9. **58.9 — Pane resize**
   Implement `ResizePaneLeft/Down/Up/Right`: find the nearest split divider in the given
   direction from the focused pane and adjust its ratio by a fixed step (e.g., 0.05).
   Clamp ratio to `[0.1, 0.9]` to prevent zero-size panes. Each resize triggers
   `InputEvent::Resize` to affected panes. Wire up keybindings. Also implement mouse-drag
   resize: clicking and dragging a pane border adjusts the split ratio interactively.

10. **58.10 — Zoom pane**
    Implement zoom toggle: when zoomed, only the zoomed pane renders (using the full
    available rect). All other panes continue receiving PTY output and draining channels
    but are not drawn. The tab title or a subtle badge indicates zoom is active. Pressing
    the zoom key again unzooms (restores the pane tree layout). Wire up the keybinding.

11. **58.11 — Window command and PTY death drain for all panes**
    Extend the per-frame drain loop in `FreminalGui::ui()` to iterate all panes in all
    tabs (not just the active tab's channels). Each pane's `window_cmd_rx` is drained.
    Each pane's `pty_dead_rx` is polled.

12. **58.12 — Menu bar and config integration**
    Add split/close/zoom/navigate actions to the menu bar under a "Pane" menu. Add
    default keybindings to `BindingMap::default()` and `config_example.toml`. Update the
    Settings Modal keybindings tab to show the new bindings. Document in `config_example.toml`.

13. **58.13 — Tests and performance verification**
    - Unit tests: `PaneTree` operations (split, close, layout, navigation, resize, zoom)
    - Unit tests: `Tab` with pane tree (single pane regression, multi-pane operations)
    - Integration tests: verify multiple panes render concurrently, input goes to correct
      pane, resize propagates correctly
    - Benchmarks: if pane layout computation is performance-sensitive, add a benchmark for
      `PaneTree::layout()` with various tree depths
    - **Pre-merge flamegraph:** Before merging, run `cargo flamegraph` and compare against
      the known baseline. Zero performance regressions are acceptable except for the
      inherent cost of additional PTY/emulator instances (which should be near-zero since
      the PTY and emulator layers are not a major bottleneck). Any regression in the render
      loop, layout, or snapshot path is a blocker.

### 58 Primary Files

- `freminal/src/gui/panes.rs` (new — `Pane`, `PaneId`, `PaneTree`, `SplitDirection`)
- `freminal/src/gui/tabs.rs` (`Tab` refactored to use `PaneTree`)
- `freminal/src/gui/mod.rs` (multi-pane rendering, input routing, drain loops)
- `freminal/src/gui/terminal/widget.rs` (accept pane rect, per-pane rendering)
- `freminal/src/gui/terminal/input.rs` (dispatch new `KeyAction` variants)
- `freminal-common/src/keybindings.rs` (new `KeyAction` variants)
- `config_example.toml` (new keybinding defaults)

### 58 Rejected Alternatives

**Trait-based `Pane` abstraction (WezTerm style):** WezTerm defines `Pane` as a trait with
~40 methods to support local, remote, and tmux CC panes. Freminal's muxing is local-only,
so a concrete struct is simpler, avoids dynamic dispatch, and avoids `Arc<dyn Pane>`
indirection. If remote mux is added later (B.1), the Pane struct can be promoted to a trait
at that time.

**Grid layout (Windows Terminal style):** Windows Terminal uses a constraint-based grid.
More flexible for certain layouts but significantly harder to implement and maintain. A
binary tree handles all practical split arrangements and is the approach used by WezTerm,
tmux, and zellij.

**Pane tree on a separate thread:** No benefit — the tree is only needed for layout and
rendering (GUI-thread concerns). PTY threads already communicate via channels and ArcSwap.
Adding thread ownership of the tree would require synchronization with no performance gain.

---

## Dependency Graph

```text
Task 53 (Multiple Windows) ── builds on tabs (Task 36, v0.3.0)
Task 54 (Background Images) ── extends background_opacity (Task 34, complete)
Task 55 (Custom Shaders) ── extends the GL renderer (Task 1, complete)
Task 56 (Session Restore) ── depends on tabs (Task 36, v0.3.0)
Task 57 (Render Loop Opt) ── independent; complete
Task 58 (Built-in Muxing) ── builds on tabs (Task 36, v0.3.0); subsumes A.2

Tasks 54 and 55 are independent of each other.
Tasks 53, 56, and 58 all depend on tabs but are independent of each other.
Task 57 is independent of all other v0.5.0 tasks (complete).
Task 58 is independent of Task 53 (multiple windows) — both can coexist.
Task 58 subsumes A.2 (Split Panes) from FUTURE_PLANS.md.
```

**Recommended order:** Task 57 is complete. Tasks 54 and 55 can start immediately (GL
renderer work). Tasks 53, 56, and 58 require tabs from v0.3.0 (complete and stable).
Task 58 (muxing) is the highest-value remaining feature — recommend prioritising it
after or alongside Task 53.

---

## Completion Criteria

Per `agents.md`, each task is complete when:

1. All subtasks marked complete
2. `cargo test --all` passes
3. `cargo clippy --all-targets --all-features -- -D warnings` passes
4. `cargo-machete` passes
5. Benchmarks show no unexplained regressions for render/buffer changes
6. Config schema additions propagated to config.rs, config_example.toml, home-manager, settings
