# Design Decisions

Durable architectural decisions and reference data extracted from completed v0.2.0 task plans.
The full plans are available in git history. This document captures only the "why" that cannot
be recovered from reading the current code.

---

## Renderer Architecture (Tasks 1, 34)

### Why rustybuzz + swash (not cosmic-text)

`cosmic-text` bundles layout logic a terminal doesn't need (paragraph reflow, line-breaking) and
had a hard version conflict with `swash` via `skrifa`. The modular `rustybuzz` + `swash` stack
gives direct control over OpenType feature flags (needed for ligatures) without pulling in an
opinionated layout engine. rustybuzz handles shaping; swash handles rasterisation (including color
emoji) and font metrics.

### Why full glow bypass (not egui Shape::mesh)

egui's `Shape::mesh` still goes through egui's tessellator and text layout, adding overhead and
losing pixel-exact control. The custom renderer uses `PaintCallback` / `egui_glow::CallbackFn`
to own its own GL state, shaders, and textures entirely. egui handles only chrome (menu bar,
settings modal). The terminal area is drawn by custom shaders with no egui involvement in
positioning or rendering.

### egui GL state contract

egui's blend state on entry to a `PaintCallback`: `GL_SCISSOR_TEST` enabled, `GL_BLEND` enabled
(premultiplied alpha: `SRC_ALPHA=ONE, DST_ALPHA=ONE_MINUS_SRC_ALPHA`), `GL_DEPTH_TEST` disabled,
`GL_CULL_FACE` disabled, `TEXTURE0` active. Shaders must output premultiplied alpha. The egui
FBO must be restored via `gl.bind_framebuffer(FRAMEBUFFER, painter.intermediate_fbo())` on exit.

### Why instanced rendering

The per-quad vertex approach built ~900K floats per full rebuild for a 200x50 terminal
(36 floats/bg-quad + 54 floats/fg-quad per cell). Instanced rendering uses a single static unit
quad drawn N times via `glDrawArraysInstanced`. Per-cell instance data is 7 floats (bg) or
14 floats (fg), yielding ~210K floats total — a ~4x reduction in CPU-to-GPU data. Background
opacity is a single `u_bg_opacity` uniform in the fragment shader: decorations/cursor remain
opaque, cell backgrounds receive the user's opacity. No CPU-side selective alpha manipulation.

---

## Font Ligatures (Task 5)

### Cell-grid authority

Ligature glyphs are forced to span exactly N x `cell_width` pixels regardless of the font's
reported advance. The cell grid is authoritative over font metrics for positioning. This prevents
sub-pixel drift and ensures cursor-within-ligature positioning is always correct.

### Feature set

`liga` + `calt` enabled (standard + contextual alternates). `dlig` always disabled (too
aggressive for terminal use — produces unexpected substitutions in code). When ligatures are
turned off in config, all three features are _explicitly disabled_ (not just unset), because
some programming fonts enable them by default.

### Color-break policy

When a format change (color, bold, italic) occurs mid-ligature, the run breaks and the ligature
does not form. Policy is "break, not blend" — no partial-color ligatures.

---

## Theming (Task 11)

### Static references, not Arc

Embedded themes are `&'static ThemePalette` references. All themes are `const` values with
`'static` lifetime, making transport zero-cost (pointer-sized). Future custom (user-defined)
themes would use `Box::leak` to achieve the same `'static` lifetime, avoiding `Arc` overhead
in the snapshot transport path.

### Symbolic colors in cells

Cells store `TerminalColor` enum variants (e.g., `DefaultForeground`, `Ansi(3)`), not resolved
RGB. Color resolution happens at render time at the GUI boundary. This means a theme switch
causes all existing buffer content to immediately re-render in the new colors without requiring
a buffer rewrite.

---

## Image Protocols (Task 13)

### Protocol comparison

| Protocol        | Bandwidth                           | Complexity | Adoption                      | Transparency | Animation   |
| --------------- | ----------------------------------- | ---------- | ----------------------------- | ------------ | ----------- |
| Sixel           | Poor (~100% overhead, ASCII bitmap) | Medium     | Broad (legacy)                | No           | No          |
| iTerm2 OSC 1337 | Moderate (~33% base64 overhead)     | Low-Medium | Broad (modern)                | No           | GIF only    |
| Kitty APC `_G`  | Best (shared-mem zero-copy locally) | High       | Kitty/Ghostty/partial WezTerm | Yes          | Frame-based |

### Priority rationale

iTerm2 first (de-facto common denominator for non-Kitty terminals, simplest), Kitty second
(richest feature set, required by tools like yazi), Sixel third (legacy, implemented but lowest
priority).

### Protocol format strings

- Sixel: `ESC P <params> q <sixel data> ESC \`
- iTerm2: `ESC ] 1337 ; File = [args] : <base64 data> BEL`
- Kitty: `ESC _ G <control data> ; <base64 payload> ESC \`

### yazi detection gap

yazi detects terminal image protocol support via environment variables (`$TERM`, `$TERM_PROGRAM`,
`$XDG_SESSION_TYPE`) with a priority order where Kitty Unicode placeholders are tried first and
iTerm2 is tried only for specific known `TERM_PROGRAM` values. Freminal does not currently set
`TERM_PROGRAM` to a yazi-recognized value, nor has an upstream yazi detection PR been merged.
This remains an actionable gap.

---

## DEC Private Modes (Task 20)

### Intentionally omitted

`?1015` (urxvt mouse encoding): The encoding format clashes with DL/SD/window manipulation
sequences. `?1006` (SGR) is the universally preferred replacement. Do not implement.

### Permanently stubbed

`?4` (DECSCLM — smooth scroll): No modern terminal implements this. All rendering is already
smooth at 60+ fps. Left as a no-op stub.

`?2031` (Color Palette Updates): Contour extension for dark/light mode change notifications.
Niche — no action needed.

---

## vttest Compliance (Task 22)

### Pending-wrap model

Freminal encodes pending-wrap state implicitly: `cursor.pos.x == width` (e.g., `x == 80` in an
80-column terminal). There is no explicit `pending_wrap` boolean flag. All cursor operations that
could interact with this state (CUP, CUF, backspace, CR, LF, RI) must be aware of this encoding.

### Menu classification

Every vttest menu was classified for automation potential:

| Code     | Meaning                                                        |
| -------- | -------------------------------------------------------------- |
| `[A]`    | Fully automatable — deterministic sequences, buffer-verifiable |
| `[I]`    | Input automatable, visual verification needed                  |
| `[V]`    | Visual only                                                    |
| `[SKIP]` | Not relevant (hardware, unimplemented features)                |

**Automatable:** Menus 1 (cursor), 2 (screen), 3 (G0 charsets), 6 (reports), 7 (VT52),
8 (insert/delete), 9 (VT100 bugs), 10.1 (RIS), 11 (non-VT100 extensions).

**Skipped:** Menu 3 G1+/NRC/ISO Latin (not implemented), Menu 4 DECDWL/DECDHL (renderer not
implemented), Menu 5 keyboard (requires GUI key input), Menu 9 Bug A (smooth scroll), Menu 9
Bugs C-L (visual-only), Menu 10.2 DECTST (hardware), Menu 11 BCE/mouse/window (not implemented
or requires GUI).

### Bugs fixed during compliance testing

| #   | Description                                           | Root cause                      |
| --- | ----------------------------------------------------- | ------------------------------- |
| 1   | TBC Ps=2 incorrectly clears character tab stop        | Wrong tab-clear variant         |
| 2   | `handle_lf`/`handle_ri` don't clear pending-wrap      | Missing x-clamp                 |
| 4a  | `character_replace` not saved/restored by DECSC/DECRC | Missing field in save           |
| 4b  | `ESC ) B` (designate G1 as US-ASCII) produces Invalid | Unrecognized SCS sequence       |
| 4c  | SI/SO (0x0E/0x0F) not handled as C0 control chars     | Missing C0 dispatch             |
| 5   | Autowrap doesn't respect DECSTBM scroll region        | Scroll check used screen bottom |
| 6   | BS from pending-wrap state lands at wrong column      | Off-by-one in pending-wrap path |
| 7   | VT52 `ESC Y` OOB row clamps col instead of col-only   | Wrong clamping logic            |
| 8   | IRM (Insert/Replace Mode) and LNM not implemented     | Missing mode handlers           |
| 9   | 8-bit C1 controls (S8C1T/S7C1T) not implemented       | Missing parser path             |

---

## Kitty Keyboard Protocol (Task 35)

### Protocol reference

**Sequences received (PTY-to-terminal):**

| Sequence               | Meaning                                                |
| ---------------------- | ------------------------------------------------------ |
| `CSI ? u`              | Query current mode stack top → respond `CSI ? flags u` |
| `CSI > flags u`        | Push flags onto mode stack                             |
| `CSI < number u`       | Pop N entries from mode stack (default 1)              |
| `CSI = flags ; mode u` | Set current flags (mode: 1=replace, 2=OR, 3=AND-NOT)   |

**Flag bitmask:**

| Bit | Dec | Name                              | Implemented                          |
| --- | --- | --------------------------------- | ------------------------------------ |
| 0   | 1   | `DISAMBIGUATE_ESCAPE`             | Yes — encoding active                |
| 1   | 2   | `REPORT_EVENT_TYPES`              | Yes — press/repeat/release           |
| 2   | 4   | `REPORT_ALTERNATE_KEYS`           | Yes — shifted-key, best-effort       |
| 3   | 8   | `REPORT_ALL_KEYS_AS_ESCAPE_CODES` | Yes — encoding active                |
| 4   | 16  | `REPORT_ASSOCIATED_TEXT`          | Yes — IME / multi-char text reported |

Encoding activates whenever any of `flags & (1 | 8)` is set.

### Flag 2 — event types (Task 50)

Press is the default. Repeat is emitted when winit reports `repeat: true` on a
key-down. Release is emitted on key-up. Winit's key-up events do not include
the character for modifier-only keys (e.g. a bare Ctrl release), so release
encoding is best-effort for modifier-only keys and fully correct for keys with
a keycode.

### Flag 4 — alternate (shifted) keys (Task 50)

When the user presses `Shift+A`, the protocol requires reporting both the
base key `a` (65) and the shifted form `A` (97) as `CSI 65:97 ; 2 u`. Winit
exposes the physical key and the logical key; we use both to populate the
base/shifted pair. Keyboard layouts without a distinct shifted form omit the
second field.

### Flag 16 — associated text (Task 50)

IME composition and multi-character key events carry text not reducible to a
single keycode. The associated text is appended after the keycode/modifier
fields separated by `;` and each character separated by `:`, encoded as
decimal Unicode codepoints.

**Key encoding (CSI u format):** `CSI keycode [; modifiers [: event-type]] u`

- `keycode`: Unicode codepoint (always lowercase/unshifted), or PUA code for non-Unicode keys
- `modifiers`: `1 + shift(1) + alt(2) + ctrl(4) + super(8) + hyper(16) + meta(32) + caps(64) + num(128)` — base is 1, not 0
- `event-type`: 1=press (default), 2=repeat, 3=release

**Functional key encoding:** Arrow/Home/End/F1-F12 use legacy `CSI ~ / CSI [letter]` format with
modifiers inserted. Escape/Enter/Tab/Backspace use `CSI u` format with their legacy C0 codes
(27, 13, 9, 127).

### Separate stacks per screen

Main and alternate screens maintain independent keyboard mode stacks. Entering alt screen saves
the main stack and starts fresh; leaving alt screen discards the alternate stack and restores
the main stack.

---

## Tabs and Per-Tab State (Task 36)

Each tab owns an independent `TerminalEmulator`, PTY reader thread, and
`Arc<ArcSwap<TerminalSnapshot>>`. Switching tabs flips which `ArcSwap` the GUI
renders from; inactive tabs continue receiving PTY output and updating their
snapshots. The PTY-processing thread is per-tab, not shared, which preserves
the lock-free architecture (see `agents.md` "Architecture" section) across
tabs.

Tab IDs are `u64` generated by a monotonic counter on the GUI thread. Closing a
tab signals its PTY thread to exit and drops the `ArcSwap` handle.

---

## Keybinding System (Task 37)

All keyboard shortcuts go through `BindingMap` in
`freminal-common/src/keybindings.rs`. Hardcoded keyboard shortcuts outside this
system are forbidden — every shortcut must be discoverable and configurable
through the `[keybindings]` config section and the Settings Modal.

### Convention

Every feature that introduces or modifies a keyboard shortcut must:

1. Add a `KeyAction` variant (with `name()`, `display_label()`, `FromStr`, and
   inclusion in `ALL`).
2. Add a default binding in `BindingMap::default()` via the
   `register_*_bindings()` helpers.
3. Handle the action in `dispatch_binding_action()` in
   `freminal/src/gui/terminal/input.rs` (or at a higher level in
   `gui/mod.rs` for actions requiring full GUI state).
4. Document the default combo in `config_example.toml` under `[keybindings]`.

This convention is mirrored in `agents.md`.

### Menu shortcut labels

Menu items display the bound shortcut using platform-canonical modifier
symbols: `⌘ ⌥ ⇧ ⌃` on macOS, `Ctrl+ Alt+ Shift+` on Linux/Windows. Labels are
looked up from the _current_ `BindingMap` so user customizations are reflected
immediately; defaults are never hardcoded into the menu.

---

## Bell Handling (Task 41)

The bell is raised by the PTY thread via `WindowCommand::Bell` sent over the
PTY→GUI channel. The GUI sets a tab-level bell-pending flag for 200 ms (visible
in the tab bar as a subtle indicator) and optionally emits an audible beep
through the OS. The 200 ms clear interval is arbitrary but chosen to be long
enough to notice and short enough to avoid stale indicators.

The bell intentionally does not use a dedicated thread or timer — the GUI's
existing repaint-after-duration mechanism clears it on the next relevant
frame.

---

## OSC 52 Clipboard Security (Task 43)

OSC 52 allows the terminal application to programmatically set and read the
system clipboard. Clipboard **writes** from OSC 52 are accepted by default
(matching user expectation when piping through tmux or SSH). Clipboard
**reads** are gated behind `allow_clipboard_read`, defaulting to `false`.

Untrusted programs running inside the terminal could otherwise exfiltrate
clipboard contents silently. The default-off read gate is the conservative
choice and can be enabled per-profile once profiles exist.

---

## SGR Underline Encoding (Task 47)

Underline style is encoded in `FontDecorationFlags` using three bits to
represent the five kitty/xterm underline styles: none, straight, double,
curly, dotted, dashed. The three bits cover all existing styles with room for
one future variant. The chosen encoding reuses the existing underline flag
bit as a "present" bit plus two additional bits for the style.

The colon-subparameter form (`CSI 4 : 3 m` for curly) is parsed in addition to
the semicolon form for compatibility with xterm and kitty. Underline color is
stored separately on the format run so it can differ from foreground color.

---

## Background Color Erase (BCE) (Task 48)

All erase operations (ED, EL, ECH, DCH, ICH, IL, DL) must fill erased cells
with the **current** SGR background color, not a fixed default. The current
SGR format is threaded as an explicit parameter into every erase method on
`Buffer` — this makes the dependency visible at every call site and avoids a
hidden read of mutable state during erase.

The old behavior of writing `default_cell()` is incorrect and user-visible:
`tput clear` with a non-default background produces a one-color field in
other terminals but not in pre-Task-48 Freminal.

---

## DECDWL / DECDHL Line Width (Task 49)

DECDWL (double-width lines) and DECDHL (double-height lines) are modeled as a
`LineWidth` enum on each row: `Single`, `Double`, `DoubleHeightTop`,
`DoubleHeightBottom`. The cursor column is **halved** when on a double-width
line — writing at logical column 40 on a double-width line puts the cursor at
physical column 80. Cursor horizontal movement commands (CUF, CUB) scale by
the line's width.

Only the top half of a DECDHL pair is rendered; the bottom half is blank and
the top half is stretched vertically in the render pipeline. This matches VT100
behavior and is the pragmatic choice — real double-height rendering would
require two-pass text layout.

---

## Password Detection and OSC 133 Bracketing (Task 51)

Password entry is detected by polling the PTY's termios with `tcgetattr`. When
`ECHO` is cleared, the pane is in password-input mode. A per-pane
`Arc<AtomicBool>` is flipped to suppress recording of `PtyInput` events (so
passwords never land in FREC recordings) and to paint a subtle lock indicator
in the status area.

Polling is used rather than listening for termios changes because POSIX does
not expose a "termios changed" notification. The poll interval is short enough
to catch `sudo` / `ssh` password prompts in practice.

---

## Adaptive Theming and DECRPM ?2031 (Task 52)

Freminal supports three theming modes: **auto**, **light**, **dark**. In auto
mode, the theme follows the OS dark-mode setting at startup and on change
(macOS and GNOME supported; other desktops fall back to the configured
default).

The DECRPM response to `CSI ? 2031 $p` encodes the mode as follows:

| User Setting | Ps (DECRPM response)                          |
| ------------ | --------------------------------------------- |
| auto         | 1 (dark mode active) or 2 (light mode active) |
| dark         | 4 (permanently set)                           |
| light        | 3 (permanently reset)                         |

Applications that understand `?2031` (the "ColorScheme" query proposed by
contour) can adapt their colors accordingly without probing the background
color.

---

## Multi-Window Architecture (Task 53)

Freminal runs a single process with multiple OS windows — **not** one
process per window. This was chosen over a multi-process architecture
because:

1. Shared resources (glyph atlas, shader programs, font cache, theme
   registry) avoid duplicating ~50 MB per window.
2. Clipboard, selection, and drag-drop between windows are trivial inside
   one process.
3. The GUI thread can coalesce repaints across windows and share the single
   egui context per window without inter-process synchronization.

### Split of per-window vs shared

- **Per-window:** window handle, egui context, `ViewState`, tab list, GL
  context, texture for glyph atlas (each window's GL context has its own
  upload).
- **Shared:** glyph atlas source data, font descriptors, theme registry,
  config, keybindings, image cache.

### `.desktop` integration

On X11 and Wayland, the `StartupWMClass` field must match the application ID
set via `winit::WindowAttributes::with_app_id()`. A mismatch causes the
compositor to fail to associate the window with the `.desktop` entry and
breaks icon/taskbar grouping.

---

## Custom Shader Pipeline (Task 55)

User-supplied GLSL fragment shaders post-process the rendered terminal
image. The pipeline is: render terminal to an offscreen FBO → run the
fragment shader sampling the FBO texture → composite to screen. The shader
sees a complete terminal frame; it cannot see individual cells.

### Uniform contract

Every user shader receives:

- `u_terminal: sampler2D` — the rendered terminal texture
- `u_resolution: vec2` — framebuffer size in pixels
- `u_time: float` — seconds since startup (monotonic)

Additional uniforms can be declared by the shader; Freminal ignores uniforms
it does not know about.

### Failure modes

Shader compilation errors fall back to pass-through rendering and surface a
non-fatal error to the user (logged, shown in settings). A broken shader
never crashes the terminal. Hot-reload (`shader.hot_reload = true`) reloads
the shader file on change; compile failure keeps the previous program
active.

---

## Render Loop Optimization (Task 57)

Before Task 57, moving the mouse over the terminal caused two repaints per
event: one from egui's default behavior, one from our cell-change detection.
The root cause was in egui-winit 0.34.1 `src/lib.rs:333-338`, which
unconditionally sets `repaint: true` for `WindowEvent::CursorMoved` regardless
of whether any widget needed redrawing.

### Fix

The `TerminalSnapshot` was extended with three fields that let the GUI decide
whether a mouse move actually changes the displayed output:

- `has_urls: bool` — true if any OSC 8 hyperlink is visible
- `row_offsets: Vec<u32>` — byte offsets into the shaped text per row
- `url_tag_indices: Vec<u32>` — indices of format runs that carry URLs

If `has_urls` is false, mouse movement over the terminal never changes what
is painted, and the repaint is skipped. If `has_urls` is true, the GUI
computes which URL (if any) is under the cursor and only repaints when the
hover target actually changes.

### Rejected render-loop alternatives

- **Fork egui-winit** to remove the unconditional repaint — rejected because
  maintaining a fork of egui is a long-term burden and the upstream behavior
  is intentional (tooltips rely on it).
- **Move rendering to a worker thread** — measured in a spike: no net gain
  because the repaint cost is dominated by egui's layout pass, which cannot
  be threaded. Benchmark table showed equivalent frame times with added
  complexity.
- **Strip `PointerMoved` events before passing to egui** — breaks tooltip
  hover on menu items and settings controls.

---

## Built-in Multiplexer / PaneTree (Task 58)

Freminal ships with built-in horizontal and vertical split panes inside a
tab. This is local-only multiplexing — no detach/reattach, no remote
sessions, no status bar. Remote mux (tmux-style) is deferred.

### Pane tree data model

```rust
enum PaneNode {
    Leaf(Pane),
    Split {
        direction: SplitDirection, // Horizontal or Vertical
        ratio: f32,                // 0.0..1.0, position of the divider
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

struct Pane {
    id: PaneId,
    emulator_handle: Arc<ArcSwap<TerminalSnapshot>>,
    input_tx: Sender<InputEvent>,
    pty_write_tx: Sender<PtyWrite>,
    view_state: ViewState,
    cols: u32,
    rows: u32,
}
```

### Multiplexer design decisions

1. **Local-only.** No remote multiplexing, no detach/reattach. Those are
   separate features that can be added later without changing the pane
   model.
2. **Binary tree, not grid.** A binary split tree matches user mental models
   (tmux / zellij) and covers all practical layouts. A grid layout is harder
   to navigate, less composable, and rare in practice.
3. **Ratio-based dividers, not pixel-based.** Ratios survive window resize
   trivially. Pixel positions would require recomputing on every resize and
   break when a window becomes smaller than the stored positions.
4. **No status bar.** Freminal is a terminal, not a workspace manager. Status
   bars are a separate concern tied to profiles/themes and belong in a
   future task.
5. **Tree on GUI thread, PTY threads unaware.** The pane tree is GUI state.
   Each pane's PTY thread owns a `TerminalEmulator` and has no knowledge of
   neighbors. This preserves the lock-free architecture.
6. **Reuse `spawn_pty_tab`.** Creating a new pane uses the same spawn helper
   as creating a tab, just with explicit initial dimensions. Tabs and panes
   share the same lifecycle plumbing.

### Input routing

- **Keyboard** routes to the active pane based on the tab's `active_pane_id`.
- **Mouse** hit-tests the pane tree: the deepest `Leaf` containing the
  cursor position receives the event. Inactive-pane clicks that would focus
  the pane are converted to pure focus changes (the underlying app receives
  no mouse event), matching the convention in `widget.rs`.

### Zoom model

One pane per tab can be "zoomed" — rendered full-size, other panes hidden.
Zoomed panes retain their size (no PTY resize on zoom); toggling zoom is a
pure rendering decision so programs do not see SIGWINCH storms.

### Rejected multiplexer alternatives

- **Trait-based `Pane` (WezTerm-style).** Dynamic dispatch adds complexity
  and did not pay off in experiments — every production pane is a normal
  terminal pane. Revisit only if image/overlay panes become a real use case.
- **Grid layout.** See design decision 2.
- **Tree on a separate thread.** Would reintroduce lock contention between
  GUI (renders) and tree owner (structure mutations from keyboard commands).
  Keeping the tree GUI-thread-owned matches the single-owner principle of
  the rest of the GUI.

---

## `freminal-windowing` Crate (Tasks 62–66)

eframe was removed. All window management, event loop ownership, and
egui/glutin integration now live in the `freminal-windowing` crate. This
section is the durable charter for that crate — what it owns, what it does
not, and the contracts it exposes.

### Responsibilities

- Own the winit event loop and dispatch events to per-window handlers.
- Own each window's glutin GL context, GL config, and `egui::Context`.
- Provide a trait-based `App` interface that the `freminal` binary
  implements to render and respond to events.
- Manage window creation, destruction, and focus; hand back `WindowId`s for
  the binary to correlate with tabs.

### What it does NOT own

- Terminal state, PTY threads, tab/pane trees, snapshots, or any
  freminal-specific semantics. The binary owns all of this.
- Fonts, glyph atlases, shader programs. The binary's renderer loads and
  manages these per-window via the `App` trait.
- Keybindings, input dispatch, or any high-level input routing. The
  `App::handle_event()` receives raw winit events.

### Public API sketch

```rust
pub trait App {
    fn handle_event(&mut self, window: WindowId, event: WindowEvent, ctx: &Context);
    fn render(&mut self, window: WindowId, ctx: &Context);
    fn on_window_close(&mut self, window: WindowId);
}

pub struct WindowConfig {
    pub title: String,
    pub size: (u32, u32),
    pub position: Option<(i32, i32)>,
    pub transparent: bool,
    pub app_id: String,
}

pub struct WindowHandle { /* opaque */ }

impl WindowHandle {
    pub fn create_window(&self, cfg: WindowConfig) -> WindowId;
    pub fn close_window(&self, id: WindowId);
    pub fn request_repaint(&self, id: WindowId);
    pub fn focus(&self, id: WindowId);
}

pub fn run(app: impl App + 'static) -> !;
```

### Dependency version floor

| Crate             | Min version |
| ----------------- | ----------- |
| winit             | 0.30        |
| glutin            | 0.32        |
| egui              | 0.34        |
| raw-window-handle | 0.6         |

### freminal-windowing design decisions

1. **Separate crate, not a module.** Isolation lets the binary depend on a
   stable windowing surface while the winit/glutin versions churn below.
2. **winit 0.30 `ApplicationHandler` trait.** Matches upstream winit
   direction. The event loop is owned by winit and we implement
   `ApplicationHandler` rather than running a custom poll loop.
3. **One `egui::Context` per window, not shared.** eframe's shared context
   caused repaint storms across windows (typing in one window triggered
   repaints in all). A per-window context is independent and reflects the
   reality that each window has its own egui widget tree.
4. **GL context per window.** Simpler than a shared context with per-window
   FBOs and avoids platform-specific shared-context bugs. The glyph atlas
   is re-uploaded per window (small cost; atlases are ~1–4 MB).
5. **No root window — all peers.** No window is privileged. Closing any
   window does not close others; closing the last one exits the app. eframe
   had a "main window" concept that broke when the first window closed.
6. **Render order per window:** egui chrome pass first (it sets up GL
   state), then the terminal GL pass draws over it using the egui
   `PaintCallback` mechanism.
7. **EventLoopProxy for PTY→GUI wakeups.** PTY threads signal new output
   via `winit::EventLoopProxy::send_event()`. This was chosen over a
   channel + `ControlFlow::Poll` loop because the proxy directly wakes the
   event loop from sleep without busy-polling.
8. **Wayland present sync.** Each window calls
   `pre_present_notify()` before `swap_buffers()` on Wayland to let the
   compositor schedule the next frame correctly. Skipping this causes frame
   pacing glitches on sway/Hyprland.

### Frame pacing and idle optimization (Task 65)

When no window has dirty state, the event loop uses `ControlFlow::WaitUntil`
targeting the next scheduled repaint (cursor blink, animation, or far
future). Dirty windows use `ControlFlow::Poll`. The "clean frame" check asks
each window whether it needs to redraw; if none do, the frame is skipped
entirely — no GL commands issued, no swap, no CPU beyond the wake.

### Known diagnostic: initial-spawn truncation (Task 67)

Early child windows were created with default 100×100 dimensions before the
compositor delivered a proper size, causing the terminal to compute a tiny
cell grid and then CUP-reflow when the real size arrived. The fix delays
PTY spawn until the first `Resized` event with non-default dimensions.

---

## FREC v2 Recording Rationale (Task 59)

See `FREC_FORMAT.md` for the full format specification. The design decisions
below capture _why_ the format is shaped this way.

1. **No playback in Freminal itself.** Playback of multi-window, multi-pane
   sessions requires re-creating the full topology, re-spawning PTYs,
   running a second virtual time source, and handling input events that
   should or should not be replayed. The complexity outweighs the value for
   a terminal. Recording remains valuable for diagnostics and session
   analysis — external tools (and `sequence_decoder.py`) can analyze and
   partially replay if needed.
2. **No feature flag.** Recording code is always compiled. `--recording-path`
   is a runtime activation. A feature gate added build-matrix complexity
   for no real benefit — recording is cheap when inactive (a single pointer
   check on the hot path).
3. **No FREC v1 backward compatibility.** The format had one real user
   (developer-only). A clean break was cheaper than a forever-supported
   translator.
4. **Single writer thread with bounded channel.** All events funnel through
   one `BufWriter<File>` to guarantee ordering and avoid multi-writer
   synchronization. PTY threads, GUI thread, and input thread all send to
   the channel; the writer drains it.
5. **Mouse move debouncing.** `MouseMove` is coalesced to ~10 Hz with a
   `coalesced_count` field so the information that the mouse was moving is
   preserved without flooding the recording.
6. **Seek index retained despite no playback.** External tooling benefits
   enormously from random access — analyzing a long recording for a
   specific timestamp or topology event would otherwise require a full
   scan. The index is cheap to produce at finalization.
7. **Topology events are first-class.** WindowCreate/TabCreate/PaneSplit etc
   are distinct event types, not embedded in a metadata blob. This lets
   analysis tools filter to topology-only with trivial code.
8. **`winit::window::WindowId` not serialized.** The opaque type is not
   serializable and could change representation across winit versions. A
   monotonic `u32` assigned per recording is stable and small.

---

## Saved Layouts Rationale (Task 61)

See `LAYOUT_FORMAT.md` for the full format specification. The design
decisions below capture _why_ the format is shaped this way.

1. **TOML over JSON.** Consistent with the existing config format. TOML's
   nesting limitations are overcome by the flat node list with parent
   references — more readable than deeply nested inline tables and lets
   the user define nodes in any order.
2. **Flat node list with parent refs for the pane tree.** The two
   alternatives — deeply nested inline tables or a custom DSL — were both
   worse for hand editing. The flat form is how many config systems handle
   trees and each node is self-contained.
3. **Variables for project-independence.** Without variables, layouts
   hardcode paths and become project-specific. `$1` positional args follow
   shell scripting convention; `${name}` and `$ENV{...}` cover the rest.
4. **Save captures topology + geometry + CWD, not running programs.** We
   cannot reliably restart arbitrary programs. Saving CWDs + geometry
   covers 90% of the restore value. Detected foreground processes are
   written as comments, not auto-run commands, to avoid surprising
   behavior.
5. **Subsumes Task 56 entirely.** The previously planned flat
   `[startup.tabs]` design is a strict subset of the layout system. The
   `[startup]` config section is unified under layouts.
6. **Window geometry is best-effort.** Position is stored unconditionally
   (portable file) and restoration degrades silently on Wayland. As Wayland
   adds positioning protocols, Freminal can adopt them without format
   changes.
7. **Multi-window format with single-window shorthand.** `[[windows]]` is
   required for multi-window layouts, optional for single-window ones, so
   simple layouts stay simple.

---

## UI Polish Decisions (Task 69)

1. **ASCII close buttons over Unicode glyphs.** Unicode symbols like ✕
   (U+2715) are not guaranteed to be in egui's default font on all
   platforms and render as squares on systems with limited font coverage.
   A plain `"X"` or egui's built-in close icon is universally reliable.
   Visual polish is not worth broken rendering.
2. **Per-pane search overlay ID.** The scrollback-search overlay uses
   `egui::Id::new(("search_overlay", pane_id))` so each pane has
   independent search state and positioning. A shared Area ID caused egui
   to treat all panes' search bars as the same widget.
3. **Settings as independent OS window.** An in-window modal (a) blocks
   interaction with the terminal behind it, (b) cannot be moved to a
   second monitor, (c) is coupled to the parent window's lifecycle. A
   standalone window solves all three and follows the peer-window model
   established in Task 64.
4. **Platform-native modifier symbols in menus.** Users expect `⌘` on
   macOS and `Ctrl` on Linux/Windows. Menu labels reflect the _current_
   binding (not hardcoded defaults) so user customizations are always
   visible.

---

## Platform Performance Principles (Task 68)

1. **Diagnose first, fix second.** Performance issues on unfamiliar
   platforms (Windows launch spike, macOS idle CPU) are dangerous to fix
   by guessing. Profiling data must precede code changes. The task's
   subtask ordering enforced this: measure with platform-appropriate tools
   (Instruments on macOS, ETW/superluminal on Windows) before intervening.
2. **Target numbers for idle CPU/GPU:**

   | Scenario              | Linux                  | macOS | Windows |
   | --------------------- | ---------------------- | ----- | ------- |
   | Steady cursor, idle   | ~0%                    | ~0%   | ~0%     |
   | Blinking cursor, idle | ~0.1%                  | ~0.1% | ~0.1%   |
   | Active PTY output     | Proportional to output |       |         |

   Any platform exceeding 1% CPU at steady idle is a correctness bug.
