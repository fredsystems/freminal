# PLAN_VERSION_060.md — v0.6.0 "Foundation"

## Goal

Replace eframe with a direct winit + glutin + egui integration, giving Freminal full control
over the event loop, GL context lifecycle, frame pacing, and multi-window management. This
eliminates five documented eframe workarounds, enables true event-driven rendering (zero CPU
at idle), and removes the root-viewport coupling that forces the "Close All Windows?"
confirmation dialog. The new windowing layer lives in a dedicated workspace crate
(`freminal-windowing`) that encapsulates all platform windowing, GL context management, and
egui integration — keeping the `freminal` binary crate focused on terminal-specific GUI logic.

---

## Task Summary

| #   | Feature                               | Scope  | Status   | Dependencies |
| --- | ------------------------------------- | ------ | -------- | ------------ |
| 62  | freminal-windowing crate + event loop | Large  | Complete | None         |
| 63  | Single-window migration               | Large  | Pending  | Task 62      |
| 64  | Multi-window parity                   | Large  | Pending  | Task 63      |
| 65  | Frame pacing + idle optimization      | Medium | Pending  | Task 63      |
| 66  | Cleanup + eframe removal              | Medium | Pending  | Task 64      |

---

## Crate: freminal-windowing

A new workspace member that owns the platform windowing abstraction. Everything below the
`freminal` crate's terminal GUI logic and above the OS lives here.

### Responsibilities

- winit `EventLoop` creation and `ApplicationHandler` implementation
- GL context creation and management via glutin + glutin-winit
- egui input translation via egui-winit
- egui rendering via egui_glow (chrome: menus, modals, dialogs)
- Window lifecycle: create, destroy, resize, focus, minimize, close
- Frame pacing: render only on demand, proper Wayland `pre_present_notify()`
- Multi-window management: peer windows with independent GL contexts

### What It Does NOT Own

- Terminal emulation (freminal-terminal-emulator)
- Buffer model (freminal-buffer)
- Custom terminal renderer (stays in freminal — the GL shaders, glyph atlas, vertex builder)
- Application-level state (tabs, panes, config, keybindings)
- PTY I/O

### Public API Surface (Sketch)

```rust
/// The application trait that `freminal` implements instead of `eframe::App`.
pub trait App {
    /// Called once per window per frame, only when a redraw is needed.
    /// `gl` is the window's GL context; `ctx` is the egui context for this window.
    fn update(&mut self, window_id: WindowId, ctx: &egui::Context, gl: &glow::Context);

    /// Called when a window is created.  Returns initial egui configuration.
    fn on_window_created(&mut self, window_id: WindowId, ctx: &egui::Context);

    /// Called when a window close is requested.  Return `false` to cancel.
    fn on_close_requested(&mut self, window_id: WindowId) -> bool;

    /// GL clear color for the given window (supports transparency).
    fn clear_color(&self, window_id: WindowId) -> [f32; 4];
}

/// Opaque window identifier (wraps winit::window::WindowId).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(winit::window::WindowId);

/// Configuration for creating a new window.
pub struct WindowConfig {
    pub title: String,
    pub inner_size: Option<(u32, u32)>,
    pub transparent: bool,
    pub icon: Option<egui::IconData>,
    pub app_id: Option<String>,
}

/// Handle passed to the App for requesting window operations.
pub struct WindowHandle<'a> { /* ... */ }

impl<'a> WindowHandle<'a> {
    pub fn create_window(&self, config: WindowConfig);
    pub fn close_window(&self, id: WindowId);
    pub fn request_repaint(&self, id: WindowId);
    pub fn request_repaint_after(&self, id: WindowId, delay: Duration);
    pub fn set_title(&self, id: WindowId, title: &str);
    pub fn set_visible(&self, id: WindowId, visible: bool);
    pub fn set_minimized(&self, id: WindowId, minimized: bool);
}

/// Entry point — replaces `eframe::run_native()`.
pub fn run(config: WindowConfig, app: impl App + 'static) -> Result<(), Error>;
```

This is a sketch, not a contract. The API will be refined during implementation. The key
design principle: `freminal-windowing` owns the event loop and GL contexts; the `freminal`
crate owns everything terminal-specific.

### Dependencies

The implementing agent MUST use the most recent stable versions of all crates at the time of
implementation. The versions listed below are the latest stable as of 2026-04-15 and serve as
a **minimum floor**, not a pin:

| Crate               | Minimum Version | Purpose                                    |
| ------------------- | --------------- | ------------------------------------------ |
| `winit`             | 0.30.13         | Window creation, event loop                |
| `glutin`            | 0.32.3          | OpenGL context creation (EGL/WGL/GLX/CGL)  |
| `glutin-winit`      | 0.5.0           | glutin-to-winit integration bridge         |
| `egui`              | 0.34.1          | Immediate-mode UI (menus, modals, dialogs) |
| `egui-winit`        | 0.34.1          | winit event to egui RawInput translation   |
| `egui_glow`         | 0.34.1          | egui rendering via glow (chrome only)      |
| `glow`              | 0.17.0          | OpenGL function loader and bindings        |
| `raw-window-handle` | 0.6.2           | Window handle interop (glutin / winit)     |
| `tracing`           | (workspace)     | Structured logging                         |
| `thiserror`         | (workspace)     | Error types                                |
| `conv2`             | (workspace)     | Numeric conversions (per agents.md)        |

**IMPORTANT:** At implementation time, the agent MUST run `cargo search <crate>` for each
dependency to verify the latest stable version and use that version, not the floor listed here.
If a new major version of `winit` (0.31+) or `egui` (0.35+) has been released stable, the
agent must evaluate compatibility and use the latest stable if feasible.

**The `eframe` dependency is NOT removed until Task 66.** Tasks 62–65 add the new crate
alongside eframe so that the migration can be verified incrementally.

---

## Task 62 — freminal-windowing Crate + Event Loop

### 62 Overview

Create the `freminal-windowing` workspace crate with a working event loop, GL context, and
egui integration for a single window. This is the foundation — no terminal rendering, no
migration of `freminal` code. The deliverable is a crate that can open a window, run an egui
frame, and paint it via glow, verified by a minimal example binary.

### 62 Subtasks

1. **62.1 — Scaffold the crate**
   Create `freminal-windowing/Cargo.toml` with workspace dependencies. Create `src/lib.rs`
   with the public API types (`App` trait, `WindowId`, `WindowConfig`, `run()`). Add the
   crate to the workspace `members` list in the root `Cargo.toml`. Ensure `cargo check --all`
   passes with the new empty crate.

2. **62.2 — winit event loop**
   Implement the winit 0.30 `ApplicationHandler` trait. Create an `EventLoop`, open a single
   window via `ActiveEventLoop::create_window()`. Handle `Resumed`, `Suspended`,
   `WindowEvent::RedrawRequested`, `WindowEvent::CloseRequested`, `WindowEvent::Resized`,
   and `AboutToWait`. The `ApplicationHandler` holds application state and dispatches to the
   `App` trait.

3. **62.3 — GL context via glutin**
   On `Resumed`, create a glutin `Display` (EGL on Linux/Wayland, WGL on Windows, CGL on
   macOS) via `glutin-winit::DisplayBuilder`. Select an OpenGL config with RGBA8 + alpha
   (for transparency support). Create a `Surface` and `PossiblyCurrentContext`. Create a
   `glow::Context` from the GL loader function. Handle `Suspended` by destroying the surface
   (required on Android, good practice elsewhere).

4. **62.4 — egui integration**
   Create an `egui::Context`. Create an `egui_winit::State` for input translation. Create an
   `egui_glow::Painter` for rendering. On each `RedrawRequested`:
   - `state.take_egui_input(&window)` → `RawInput`
   - `ctx.run(raw_input, |ctx| app.update(...))` → `FullOutput`
   - `state.handle_platform_output(&window, full_output.platform_output)`
   - `painter.paint_and_update_textures(...)` with the shapes and textures delta
   - `window.pre_present_notify()` then `surface.swap_buffers(&context)`

5. **62.5 — Frame pacing**
   Implement demand-driven rendering: only call `window.request_redraw()` when egui reports
   `needs_repaint`. Expose `request_repaint()` and `request_repaint_after(delay)` through the
   `WindowHandle` API. Use a timer (winit `ControlFlow::WaitUntil`) to wake the event loop
   for delayed repaints. Verify: with no UI interaction, CPU usage is zero.

6. **62.6 — Transparency support**
   When `WindowConfig::transparent` is true, configure the glutin surface with alpha, set
   `window_attributes.with_transparent(true)`, and clear with `[0, 0, 0, 0]` before each
   frame. Verify: window background is see-through on a supported compositor.

7. **62.7 — Error handling**
   Define a `freminal_windowing::Error` enum covering: event loop creation failure, GL context
   creation failure, surface creation failure, window creation failure. All public APIs return
   `Result<T, Error>`. No panics in production code.

8. **62.8 — Example binary + tests**
   Create `freminal-windowing/examples/hello.rs` — a minimal app that opens a window, shows
   an egui label "Hello from freminal-windowing", and exits on close. Unit tests for
   `WindowId`, `WindowConfig`, error types. The example is not a production artifact — it
   exists for verification and can be removed later.

### 62 Primary Files

- `freminal-windowing/Cargo.toml` (new crate manifest)
- `freminal-windowing/src/lib.rs` (public API, re-exports)
- `freminal-windowing/src/event_loop.rs` (ApplicationHandler, event dispatch)
- `freminal-windowing/src/gl_context.rs` (glutin setup, surface management)
- `freminal-windowing/src/egui_integration.rs` (egui_winit + egui_glow wiring)
- `freminal-windowing/src/error.rs` (error types)
- `freminal-windowing/examples/hello.rs` (verification example)
- `Cargo.toml` (workspace member addition)

### 62 Design Decisions

1. **Separate crate, not inline in `freminal`.** The windowing layer is a reusable abstraction
   that encapsulates platform-specific complexity. Keeping it in a separate crate enforces a
   clean API boundary, makes it testable in isolation, and follows the existing workspace
   pattern (`freminal-buffer`, `freminal-common`, `freminal-terminal-emulator`).

2. **winit 0.30 `ApplicationHandler` trait, not the deprecated `EventLoop::run()` closure.**
   winit 0.30 deprecated the closure-based event loop in favor of the trait-based
   `ApplicationHandler`. The trait-based approach is cleaner (proper struct methods instead of
   a giant closure) and is the only supported path forward in winit 0.31+.

3. **One `egui::Context` per window (not shared).** eframe shares one `Context` across all
   viewports. This causes the root-viewport coupling problem. With independent contexts, each
   window is a peer with its own egui state. The cost is that egui `Memory` (widget state like
   scroll offsets, text cursor positions) is per-window, which is correct behavior anyway.

4. **GL context per window.** Each window gets its own glutin `Surface` +
   `PossiblyCurrentContext`. This avoids the need for context switching (`make_current`) on
   every frame and enables future per-window vsync.

---

## Task 63 — Single-Window Migration

### 63 Overview

Migrate the `freminal` binary from eframe to `freminal-windowing` for the single-window case.
After this task, Freminal opens and runs using the new event loop with full feature parity
for a single window. eframe remains as a dependency (not yet removed) but is no longer called.

### 63 Subtasks

1. **63.1 — Implement the `App` trait in `freminal`**
   Create a new `impl freminal_windowing::App for FreminalApp` (or adapt `FreminalGui`).
   The `update()` method should contain the same logic as the current `eframe::App::ui()`
   body. The `on_close_requested()` method replaces the `close_requested` + `CancelClose`
   intercept.

2. **63.2 — Replace `eframe::run_native()` with `freminal_windowing::run()`**
   In `main.rs`, replace the eframe launch code with the new entry point. Translate
   `NativeOptions` to `WindowConfig`. Remove the `raw_input_hook` override (no longer needed).
   Remove the `clear_color` hook (controlled directly via the `App` trait method).

3. **63.3 — Migrate PaintCallback to direct GL calls**
   The current custom renderer is wrapped in `egui_glow::CallbackFn` closures. With
   `freminal-windowing`, the `App::update()` method receives the `glow::Context` directly.
   Restructure the render pipeline: egui paints chrome first (via `egui_glow::Painter`), then
   the custom terminal renderer draws directly to the framebuffer. This eliminates the
   `PaintCallback` indirection and the `painter.intermediate_fbo()` restore requirement.

4. **63.4 — Migrate repaint requests**
   Replace `ui.ctx().request_repaint()` and `ui.ctx().request_repaint_after(delay)` with
   `WindowHandle::request_repaint()` and `WindowHandle::request_repaint_after()`. Update the
   PTY consumer thread to use the new repaint mechanism (it currently holds an
   `Arc<OnceLock<egui::Context>>` and calls `ctx.request_repaint_after`).

5. **63.5 — Migrate ViewportCommand calls**
   Replace all `ui.ctx().send_viewport_cmd(ViewportCommand::*)` calls with direct
   `WindowHandle` method calls (`set_title`, `close_window`, `set_minimized`, etc.). Remove
   the `last_window_title` caching workaround (no longer needed — direct winit calls don't
   trigger repaint loops).

6. **63.6 — Migrate `eframe::egui::*` imports**
   Mechanical find-replace: all `eframe::egui::*` imports become `egui::*`. All
   `eframe::glow::*` imports become `glow::*`. All `eframe::egui_glow::*` imports become
   `egui_glow::*`. This is a large but trivial diff.

7. **63.7 — Verification + benchmarks**
   Run the full verification suite. Run render loop benchmarks before and after. Verify:
   single window opens, terminal works, tabs work, panes work, settings modal works, menu bar
   works, search works, background images work, custom shaders work, background opacity works.
   Verify on Wayland: no hang on hidden workspace (the vsync bug is gone).

### 63 Primary Files

- `freminal/src/main.rs` (launch code replacement)
- `freminal/src/gui/mod.rs` (App trait impl, removal of eframe hooks)
- `freminal/src/gui/terminal/widget.rs` (PaintCallback removal)
- `freminal/src/gui/window.rs` (secondary window code — deferred to Task 64)
- `freminal/src/gui/rendering.rs` (direct GL rendering)
- `freminal/src/gui/pty.rs` (repaint mechanism)
- `freminal/Cargo.toml` (add `freminal-windowing` dep)

### 63 Design Decisions

1. **Render order: egui chrome first, then terminal GL.** Currently the terminal renderer
   runs inside a `PaintCallback` embedded in egui's paint list. With direct control, we paint
   egui's chrome (menu bar, settings modal, tab bar) first via `egui_glow::Painter`, then
   render the terminal content directly via our custom shaders. The terminal occupies a known
   pixel rect (from egui layout), so the custom renderer clips to that rect. This eliminates
   the intermediate FBO restore dance.

2. **Keep eframe as a dependency during migration.** Tasks 62–65 are incremental. eframe is
   not removed until Task 66. This allows bisecting if something breaks.

---

## Task 64 — Multi-Window Parity

### 64 Overview

Extend `freminal-windowing` to support multiple windows and migrate the multi-window code from
the current eframe deferred-viewport approach. After this task, `Ctrl+Shift+N` opens a new
peer window with its own GL context, and closing any window closes only that window.

### 64 Subtasks

1. **64.1 — Multi-window support in freminal-windowing**
   Extend the `ApplicationHandler` to manage a `HashMap<WindowId, WindowState>` where
   `WindowState` holds the `Window`, `Surface`, `PossiblyCurrentContext`, `egui::Context`,
   `egui_winit::State`, and `egui_glow::Painter` for each window. `WindowHandle::create_window()`
   inserts a new entry. Events are dispatched to the correct window by `winit::window::WindowId`.

2. **64.2 — Peer window model**
   All windows are equal peers. There is no root window. Closing any window closes only that
   window and its resources. Closing the last window exits the process. The `App` trait's
   `on_close_requested()` receives the `WindowId` and the app decides whether to allow it
   (e.g., confirm if the window has running processes). This eliminates the confirmation
   dialog for root-window close.

3. **64.3 — Migrate secondary window code**
   Replace the `show_viewport_deferred` approach in `gui/window.rs` with
   `WindowHandle::create_window()`. Remove `SecondaryWindowState`, the per-frame
   re-registration loop, the pruning loop, and the `closing_all` flag. Each window calls
   `App::update()` independently with its own `WindowId`.

4. **64.4 — Shared resources**
   Resources that are shared across windows (`Config`, `FontManager`, `PaneIdGenerator`)
   continue to use `Arc<Mutex<>>`. Per-window resources (`TabManager`, `PaneTree`, channels,
   `WindowPostRenderer`) move into a per-window state struct owned by the `App`.

5. **64.5 — Tests**
   Unit tests for the multi-window manager: create window, close window, close last window
   exits, event dispatch to correct window. Integration: open two windows, verify independent
   tab/pane management, close one, verify the other continues.

### 64 Primary Files

- `freminal-windowing/src/event_loop.rs` (multi-window dispatch)
- `freminal-windowing/src/lib.rs` (WindowHandle API extensions)
- `freminal/src/gui/mod.rs` (per-window state management)
- `freminal/src/gui/window.rs` (replacement of deferred viewport code)
- `freminal/src/gui/actions.rs` (close_or_hide_root → close_window)

### 64 Design Decisions

1. **No root window.** The fundamental architectural improvement. Every window is a peer.
   Closing the last window triggers `ControlFlow::Exit`. This eliminates the entire class of
   bugs around root-viewport coupling.

2. **One `egui::Context` per window.** Each window has its own egui state. This means the
   settings modal is per-window (not shared). If the user opens settings in Window A, Window B
   is unaffected. This is simpler and more correct than the shared `Arc<AtomicBool>` mutual
   exclusion approach.

### 64 Known Bug: Child Window Initial Spawn Truncation

**Must verify fixed / fix during this task.** Under eframe, child windows spawned via
`show_viewport_deferred` start their PTY at the hardcoded 100×100 default size. The shell
and any startup programs (e.g. fastfetch) format output for 100 columns using CUP (absolute
cursor positioning) during the gap before the first deferred-viewport frame fires a resize.
When the real resize arrives (e.g. 61×34 on a tiling WM), reflow garbles the CUP-positioned
layout. The root window doesn't have this issue because eframe's synchronous initialization
fires the first frame/resize before the shell does significant work.

The fix in v0.6.0 is architectural: with peer windows and `on_window_created()`, the window
size is known before PTY creation. `spawn_pty_tab()` should accept initial dimensions from
the actual window geometry instead of using hardcoded defaults. **Additionally**, there may
be a deeper buffer/reflow bug — verify that CUP-positioned content survives resize correctly
even when the initial size is correct.

---

## Task 65 — Frame Pacing + Idle Optimization

### 65 Overview

With the custom event loop in place, implement true event-driven rendering. The goal: zero CPU
usage when the terminal is idle with a steady cursor, and minimal wakeups when only the cursor
blink timer is active.

### 65 Subtasks

1. **65.1 — Demand-driven rendering**
   Each window tracks whether it needs a repaint via a dirty flag. The PTY consumer thread
   sets the flag and calls `request_repaint()`. Keyboard/mouse input sets the flag. The cursor
   blink timer calls `request_repaint_after(500ms)`. If the flag is not set when
   `RedrawRequested` fires, skip the frame entirely (just return from the handler without
   calling `ctx.run()` or `swap_buffers()`).

2. **65.2 — Proper Wayland frame pacing**
   Call `window.pre_present_notify()` before `surface.swap_buffers()`. This activates winit's
   Wayland frame-callback pacing, preventing the compositor hang on hidden workspaces. Remove
   the `vsync = false` workaround. Test: move Freminal to a hidden workspace, verify no hang
   and no CPU spin.

3. **65.3 — Per-window repaint timers**
   Each window maintains its own next-repaint-at timestamp. The event loop's `ControlFlow` is
   set to `WaitUntil(earliest_deadline)` across all windows. Only the window whose deadline
   has passed receives a `request_redraw()`. Other windows sleep.

4. **65.4 — Idle power measurement**
   Measure and document idle CPU usage in three scenarios:
   - Terminal idle, steady cursor (expect ~0%)
   - Terminal idle, blinking cursor (expect ~0.1%, waking every 500ms)
   - Active PTY output (expect proportional to output rate, capped at ~120 FPS per window)
     Compare against the eframe baseline. Include in the completion report.

### 65 Primary Files

- `freminal-windowing/src/event_loop.rs` (frame pacing, dirty tracking, timer management)
- `freminal/src/gui/pty.rs` (repaint request path)
- `freminal/src/gui/mod.rs` (dirty flag integration)

### 65 Design Decisions

1. **Skip-frame on clean.** If nothing has changed, don't call `ctx.run()`. This is the
   single biggest idle power win. eframe always calls `update()` on `RedrawRequested` even
   if the frame will be identical.

2. **`WaitUntil` instead of `Poll`.** winit's `ControlFlow::WaitUntil` suspends the thread
   until the deadline or an OS event, whichever comes first. This is strictly better than
   `Poll` (which spins) or `Wait` (which doesn't support timers).

---

## Task 66 — Cleanup + eframe Removal

### 66 Overview

Remove the `eframe` dependency entirely. Clean up any remaining eframe artifacts, dead code,
and transitional scaffolding.

### 66 Subtasks

1. **66.1 — Remove eframe from Cargo.toml**
   Remove `eframe` from the workspace `[dependencies]` and from `freminal/Cargo.toml`. Add
   `egui`, `egui-winit`, `egui_glow`, `glow`, `winit`, `glutin`, `glutin-winit` as direct
   workspace dependencies if not already present (they may already be there from Task 62).

2. **66.2 — Remove eframe re-export paths**
   Any remaining `eframe::egui::*`, `eframe::glow::*`, or `eframe::egui_glow::*` import
   paths are replaced with direct crate imports. This should already be done by Task 63.6
   but verify completeness.

3. **66.3 — Remove dead workarounds**
   Remove the following code that exists solely to work around eframe limitations:
   - `raw_input_hook` method (no longer exists after Task 63)
   - `clear_color` hook (no longer exists after Task 63)
   - `last_window_title` caching (no longer needed after Task 63.5)
   - `closing_all` flag and confirmation dialog (no longer needed after Task 64.2)
   - `show_close_confirmation` flag
   - `vsync = false` workaround and associated comments
   - `predicted_dt = 0.0` override and associated comments

4. **66.4 — Update documentation**
   Update `agents.md` architecture section to reflect the new windowing layer. Update
   `DESIGN_DECISIONS.md` with the eframe removal rationale. Update `config_example.toml` if
   any config keys changed.

5. **66.5 — Final verification**
   Full verification suite. Render loop benchmarks before and after (expect improvement from
   eliminated callback indirection). Manual testing on Linux/Wayland, Linux/X11. Verify all
   features work: tabs, panes, split, multiple windows, search, background images, custom
   shaders, background opacity, settings modal, theming, clipboard, drag-and-drop, bell,
   cursor trail, blinking text, Kitty keyboard protocol.

### 66 Primary Files

- `Cargo.toml` (workspace dependency changes)
- `freminal/Cargo.toml` (eframe removal)
- `freminal/src/gui/mod.rs` (dead workaround removal)
- `agents.md` (architecture update)
- `Documents/DESIGN_DECISIONS.md` (new decision entry)

---

## Dependency Graph

```text
Task 62 (freminal-windowing crate)
  │
  ▼
Task 63 (Single-window migration)
  │
  ├──────────────────┐
  ▼                  ▼
Task 64            Task 65
(Multi-window)     (Frame pacing)
  │
  ▼
Task 66 (Cleanup + eframe removal)
```

**Recommended order:** 62 → 63 → 64 ∥ 65 → 66

Tasks 64 and 65 can run in parallel after Task 63 — they touch different code (multi-window
lifecycle vs. frame pacing/timers). Task 66 must be last.

---

## Cross-Cutting Concerns

### Crate Dependency Boundaries

`freminal-windowing` must NOT depend on any other freminal crate. It is a general-purpose
windowing layer. The dependency flows one way:

```text
freminal ──► freminal-windowing
freminal ──► freminal-terminal-emulator ──► freminal-buffer ──► freminal-common
```

`freminal-windowing` depends only on external crates (winit, glutin, egui, glow, etc.) and
standard library types.

### Custom Renderer Integration

The terminal renderer (`freminal/src/gui/renderer/`) currently uses `egui_glow::CallbackFn`
to inject GL calls into egui's paint pipeline. After migration:

- The renderer receives `&glow::Context` directly from the `App::update()` signature
- No more `PaintCallback` wrapping
- No more `painter.intermediate_fbo()` restore
- The render order is explicit: egui chrome → terminal content (or vice versa, with proper
  depth/stencil management)

The renderer itself (`gpu.rs`, `vertex.rs`, shader code) is unchanged — only the call site
changes.

### PTY Thread Repaint Path

Currently, PTY consumer threads hold `Arc<OnceLock<egui::Context>>` and call
`ctx.request_repaint_after(Duration::from_millis(8))`. After migration, they need a
thread-safe repaint handle from `freminal-windowing`. Options:

1. **Channel-based:** PTY thread sends a "repaint window X" message to the event loop thread
   via `crossbeam-channel`. The event loop calls `window.request_redraw()`.
2. **`EventLoopProxy`:** winit provides `EventLoopProxy::send_event()` for cross-thread
   wakeups. The PTY thread holds a clone of the proxy and sends a custom user event.

Option 2 is preferred — `EventLoopProxy` is the canonical winit mechanism for cross-thread
wakeups and avoids adding a channel.

### Config Schema

No config schema changes are expected. The windowing layer is transparent to the user.
`background_opacity`, `vsync` (if exposed), and window-related settings continue to work
through the same config paths.

### Platform Testing

The migration is platform-sensitive. The following must be tested:

- **Linux/Wayland:** Primary target. Verify no compositor hang, proper frame pacing,
  transparency, multi-window.
- **Linux/X11:** Verify GL context creation (GLX or EGL), transparency, multi-window.
- **macOS:** Verify CGL context, Retina scaling, transparency. (If no macOS CI, document
  as untested and request manual verification.)
- **Windows:** Verify WGL context, DPI scaling, transparency. (Same caveat.)

### Benchmark Impact

The migration should improve render loop benchmarks by eliminating:

- `PaintCallback` closure allocation and dispatch
- egui intermediate FBO management
- `predicted_dt` scheduling overhead

Agents must capture before/after numbers for all render loop benchmarks:

- `freminal/benches/render_loop_bench.rs` — all benchmarks

---

## Risk Assessment

| Risk                                                  | Likelihood | Impact | Mitigation                                                                 |
| ----------------------------------------------------- | ---------- | ------ | -------------------------------------------------------------------------- |
| GL context creation fails on exotic drivers           | Medium     | High   | glutin handles driver fallbacks; test on Mesa, NVIDIA proprietary, and AMD |
| winit 0.31 goes stable mid-task, breaking 0.30 API    | Low        | Medium | Pin to 0.30.x during migration; upgrade to 0.31 as a follow-up             |
| egui 0.35 releases mid-task with breaking changes     | Low        | Medium | Pin to 0.34.x during migration; upgrade as a follow-up                     |
| PaintCallback removal breaks custom shader pipeline   | Medium     | Medium | Task 63.3 is dedicated to this; thorough testing of all shader features    |
| Wayland compositor differences (GNOME vs Sway vs KDE) | Medium     | Low    | Test on at least two compositors                                           |

---

## Completion Criteria

Per `agents.md`, each task is complete when:

1. All subtasks marked complete
2. `cargo test --all` passes
3. `cargo clippy --all-targets --all-features -- -D warnings` passes
4. `cargo-machete` passes
5. Benchmarks show no unexplained regressions for render changes
6. Plan document updated with completion status and notes
