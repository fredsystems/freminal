# Design Decisions

Durable architectural decisions and reference data extracted from completed v0.2.0 task plans.
The full plans are available in git history. This document captures only the "why" that cannot
be recovered from reading the current code.

---

## Renderer Architecture (Tasks 1, 34)

### Why harfrust + swash (not cosmic-text)

`cosmic-text` bundles layout logic a terminal doesn't need (paragraph reflow, line-breaking) and
had a hard version conflict with `swash` via `skrifa`. The modular `harfrust` + `swash` stack
gives direct control over OpenType feature flags (needed for ligatures) without pulling in an
opinionated layout engine. harfrust handles shaping; swash handles rasterisation (including color
emoji) and font metrics.

harfrust (the HarfBuzz project's Rust port, built on the fontations `read-fonts` crate) replaced
`rustybuzz` in #530, after rustybuzz was archived and left unmaintained (RUSTSEC-2026-0206). It is
held to the 0.12 line that `epaint` declares, so that `read-fonts` / `font-types` are shared with
egui's text layout and with swash's `skrifa` rather than duplicated. It is therefore bumped
together with the egui stack; `renovate.json` enforces `<0.13`.

`FontManager` caches the owned `harfrust::ShaperData` per face and rebuilds the cheap
`FontRef` / `Shaper` borrow views on every shape call, which avoids self-referential storage.
Compiled shape plans are cached under `(FaceId, LigatureShaping, Script, Direction)`.
`LigatureShaping` owns the full OpenType feature list for each mode, so the key fully determines
the plan's inputs and one caller (for example toast text) can never decide the plan another
caller (the terminal grid) gets. `shape_cached` takes the text and builds the harfrust buffer
itself (no language set; script and direction guessed), so the plan key captures every input to
the plan. `FontManager::rebuild` clears both caches; a font size or DPI change evicts only the
`FaceId::System` entries (system face ids are reassigned), because shaping data and plans are
size-independent and every other entry stays valid.

Toast text deliberately shapes with ligatures disabled (`TOAST_LIGATURE_SHAPING`, one constant
shared by measurement and drawing), so the shared plan cache never mixes feature sets. Until the
fix in #530 the plan key ignored the feature list, so a toast could switch ligatures on in the
terminal.

Two pinned golden tests in `freminal/src/gui/shaping.rs` are the output-identity guard across the
shaping-library change, both captured from rustybuzz 0.20.1 before the migration:
`golden_bundled_shaping_output_is_pinned` covers the regular face with a Latin and ligature
corpus, and `golden_extended_bundled_shaping_matches_rustybuzz` covers every bundled face (bold,
italic and bold-italic CaskaydiaCove; Noto Color Emoji including ZWJ, VS16 and flags) plus CJK,
RTL and Indic text on the regular face.

### Why winit and egui-winit build without default features

`winit` and `egui-winit` are both declared with `default-features = false` in the workspace
`Cargo.toml`, each with an explicit feature list. winit's list is its own default set with
`wayland-csd-adwaita` swapped for `wayland-csd-adwaita-notitle`: the default feature draws
client-side decorations with `ab_glyph`, which pulls `ttf-parser` (unmaintained,
RUSTSEC-2026-0192; #530), while the `-notitle` variant keeps the decorations but drops the title
font dependency. egui-winit's list is its own default set minus `winit/default`, because
enabling its defaults would turn winit's defaults back on.

The tradeoff: on Wayland compositors that require client-side decorations (GNOME), the window
title text is not drawn; the border, buttons and drag area remain. winit 0.31's
`wayland-csd-adwaita-skrifa` restores titles using `skrifa`, which is already in the tree, and
becomes available once `egui-winit` moves to winit 0.31. Re-enabling the default features on
either crate (egui-winit forwards `winit/default`) reintroduces `ttf-parser`, so both must stay
opted out together.

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

---

## Shell Integration Architecture (Task 72.8b / 72.8c)

### Why Ghostty-style spawn-time injection (not PTY-side scripting, not on-system install)

freminal provides command-aware features (gutters, navigation, notifications,
command-block fold/collapse) by parsing OSC 133 (FTCS) prompt/command
markers emitted by the user's shell. To do this reliably, freminal needs
those markers to actually be emitted — which means the shell must source
some integration script before drawing its first prompt.

Three options were considered:

1. **User manually sources a script in their rc file.** Standard practice for
   bash/zsh users. Rejected because:
   - Users must take action; if they don't, command-aware features silently
     don't work.
   - Re-launching freminal from inside another terminal (or freminal-in-
     freminal) doesn't help — the user's rc was loaded once at session
     start; the injection happens at that moment, not at PTY spawn.

2. **PTY-side injection: write the script bytes to the master side of the
   PTY after the shell starts.** Investigated empirically (validation in
   `/tmp/nix-shell.noQLDq/opencode/freminal-shell-injection/` on
   2026-05-18). **Rejected because it is fundamentally visible** to the
   user. Both bash and zsh echo all stdin via their line editor; heredoc
   blocks render their body verbatim before evaluation. Bracketed-paste
   wrapping makes it worse (reverse-video flash of the pasted code).
   There is no way to inject through the PTY's input side that doesn't
   show on screen.

3. **Ghostty-style spawn-time env injection.** freminal sets shell-specific
   env vars (`ENV` for bash, `ZDOTDIR` for zsh, `XDG_DATA_DIRS` for fish
   and nushell) on the child process before spawn. The shell's standard
   startup-file discovery finds our scripts and sources them silently
   alongside the user's normal rc files. **This is the architecture
   adopted in 72.8b.**

The empirical confirmation that Ghostty does exactly this and that the
result is invisible to the user came from reading their source
documentation ([Ghostty Shell Integration](https://ghostty.org/docs/features/shell-integration)
and `src/shell-integration/README.md`, retrieved 2026-05-18). iTerm2 3.5+
has the same feature ("Load shell integration automatically"); WezTerm
distributes `wezterm.sh` via `/etc/profile.d/` on packaged installs and
relies on users sourcing it manually otherwise — a less ambitious model
than Ghostty's.

### The `freminal=1; fid=<id>` marker convention

When sourced inside a freminal session, our shell scripts emit OSC 133
markers tagged with two parameters that no other emitter uses:

- `freminal=1` — sentinel saying "this marker was emitted by freminal's
  integration." Presence is the gate that distinguishes our markers from
  Starship's, WezTerm's, iTerm2's, etc.
- `fid=<id>` — a freminal-specific block ID (matching A and D pairs).
  Format is shell-side opaque (currently `<pid>-<counter>`). Required for
  correlation when multiple emitters interleave A and D markers within
  the same prompt cycle.

Example:

```text
OSC 133 ; A ; freminal=1 ; fid=12345-1 ST
OSC 133 ; B ; freminal=1 ; fid=12345-1 ST
OSC 133 ; C ; freminal=1 ; fid=12345-1 ST
OSC 133 ; D ; 0 ; freminal=1 ; fid=12345-1 ST
```

The parser (`parse_ftcs_params` in `freminal-common/src/buffer_states/ftcs.rs`)
requires `freminal=1` for A/B/C/D markers. Markers without it return `None`
and never reach the buffer. WezTerm/Starship/iTerm2 emissions are silently
dropped at the parser layer. The shell-side emitters continue to run (we
do not attempt to disable them), but their output is microseconds-per-
prompt of harmless noise. P (prompt property) does not require the
`freminal=1` gate because it is informational-only and has no effect on
buffer state.

`Buffer::start_command_block(cwd, fid)` and `finish_command_block(exit_code, fid)`
take the `fid` and use it for explicit A/D correlation, falling back to
the previous "most-recent-open" heuristic only when `fid` is `None`
(which our scripts never emit, but defensively preserves behaviour for
historical FTCS markers in FREC recordings).

### Why freminal owns the on-disk script files

The scripts live on disk at runtime (see "File layout" below). They are
shipped as `include_bytes!()` constants in the binary so the install does
not depend on packaging shipping the scripts as separate files.

**freminal owns the on-disk files. User edits are not preserved.** On
every launch, `sync_to_disk` (in `freminal/src/shell_integration.rs`)
compares the on-disk content of each script against the embedded bytes
and overwrites any file that differs.

Rationale: the scripts are tightly coupled to freminal's parser. A
user-modified script may emit markers in an incompatible format,
producing parser errors that the user cannot easily debug. Treating the
scripts as a managed resource (like a vendored binary) keeps the
parser/script contract closed and testable. Users who genuinely need to
customise the integration are expected to fork freminal and modify the
embedded scripts.

The Settings Modal does NOT expose a manual sync trigger or path-copy
button. The auto-sync runs on every launch silently. This was a
deliberate UI simplification: removing the buttons removed the
`SettingsAction::ReinstallShellScripts` and `CopyShellIntegrationPath`
variants and the corresponding `pending_shell_action` field on
`SettingsModal`.

### Version-marker hygiene (developer-facing, not runtime-load-bearing)

Each shipped script starts with a `# freminal-shell-integration v<N>`
comment (or `<!-- ... -->` for README.md). A single Rust constant
`FREMINAL_SHELL_INTEGRATION_VERSION: u32` lives in `shell_integration.rs`.
A `cargo test` invariant asserts the constant matches the marker in every
shipped script. Forgetting to bump the constant after editing a script
fails CI.

The runtime install path (`sync_to_disk`) does NOT read the version
marker. It compares bytes directly. The version is purely a development-
discipline marker — it gives us a stable, greppable identifier for
support diagnostics ("what version of the integration is this user
running?") and ensures the team cannot ship asymmetric script/parser
changes.

### File layout on disk

```text
$FREMINAL_RESOURCES_DIR/shell-integration/    (or user fallback)
├── bash/
│   └── freminal-init.bash       — sourced via $ENV in POSIX bash mode
├── zsh/
│   ├── .zshenv                  — read by zsh when $ZDOTDIR points here
│   └── freminal-integration     — sourced from .zshenv after user's rc
├── fish/
│   └── vendor_conf.d/
│       └── freminal.fish        — auto-loaded via $XDG_DATA_DIRS
└── README.md
```

The bash, zsh, and fish entries use the shell-specific subdirectory
structures required by each shell's standard startup-file discovery:

- **bash**'s `$ENV` env var (set when `--posix` is active) points to a
  single file. Our `freminal-init.bash` first runs `set +o posix` to
  return to bash mode, then sources `~/.bashrc` (or `~/.bash_profile`
  for login shells), then installs our hooks.
- **zsh** treats `$ZDOTDIR` as a directory and looks for `.zshenv`,
  `.zshrc`, etc. inside it. Our `.zshenv` does the Ghostty-derived dance:
  restore the user's original `$ZDOTDIR` from `$__FREMINAL_ZSH_ZDOTDIR`
  (a sentinel env var we set before spawn), source the user's real
  `.zshenv`, then source our `freminal-integration` script. The
  `+X` parameter expansion is used to distinguish "user had `$ZDOTDIR`
  set even if empty" from "user did not have `$ZDOTDIR` set at all".
- **fish** auto-loads files matching `<XDG_DATA_DIR>/fish/vendor_conf.d/*.fish`
  on startup. We prepend our resources dir to `$XDG_DATA_DIRS`, and fish
  finds `freminal.fish` via standard discovery. No `$ZDOTDIR`-style
  backup-restore needed because fish's `XDG_DATA_DIRS` discovery
  composes naturally with the user's existing paths.

### Search order for the resources dir

At runtime, `freminal_common::config::shell_integration_dir()` returns
the first existing directory among:

1. `$FREMINAL_RESOURCES_DIR/shell-integration/` (packaging override —
   intended for distros that ship our scripts in a system location like
   `/usr/share/freminal/shell-integration/`).
2. Entries in `$XDG_DATA_DIRS` containing `freminal/shell-integration/`
   (alternate system-wide install path on Linux).
3. `~/.config/freminal/shell-integration/` (user-local fallback,
   populated by `sync_to_disk` on launch).

The fallback location is the only one freminal itself writes to.
Packaging overrides (1, 2) are read-only and managed by the packager;
freminal's `sync_to_disk` does not write to them. If `$FREMINAL_RESOURCES_DIR`
points at a directory that lacks `shell-integration/`, freminal falls
through to the user-local fallback and treats the packaging override
as missing.

### Fail-graceful semantics

The whole shell-integration pipeline is best-effort. Any failure mode
results in "no command-aware features for this session" with no error
surface beyond a `tracing::warn!` log line. Failure paths:

- Resources dir cannot be resolved (e.g. `BaseDirs::new()` returns None) —
  PTY spawns normally without env injection. Other terminal features
  unaffected.
- `sync_to_disk` cannot write (read-only filesystem, missing parent dir,
  permission denied) — PTY spawns normally without env injection. Toast
  is NOT surfaced (this happens at startup before the toast system
  exists); only logged.
- Shell binary is unknown (not bash/zsh/fish/nushell) — no env injection.
  Shell starts as usual, no integration.
- User runs an explicit `command` instead of a shell (e.g. `freminal vim`)
  — no env injection. The command runs as itself; command-aware features
  are obviously not relevant.
- Scripts are present but malformed (should never happen at runtime
  because the byte content is compiled in; would happen only if a
  packaging override has corrupted scripts) — the shell itself surfaces
  a syntax error to stderr; our parser sees no markers; user sees a
  brief error message in their shell. Recovery is to remove the
  packaging override.

### Why we do NOT attempt to detect and disable other FTCS emitters

The shell user's environment may include WezTerm's `wezterm.sh`,
Starship's `starship init`, iTerm2's `iterm2_shell_integration.zsh`,
Kitty's integration, etc. — any of these distributed by packaging or
sourced manually by the user. They emit OSC 133 markers that lack our
`freminal=1` parameter.

Earlier designs considered detecting these (via env vars like
`$WEZTERM_PANE`, `$ITERM_SHELL_INTEGRATION_INSTALLED`) and disabling
them. This was rejected because:

- Detection signals are unreliable. A NixOS environment where WezTerm's
  shell integration is sourced via home-manager has no `$WEZTERM_PANE`
  (no live WezTerm parent process) but the integration runs anyway.
- Disabling other emitters means modifying the user's shell environment
  in ways that affect their experience outside freminal (e.g. unsetting
  Starship's OSC 133 emission would also affect non-freminal terminals
  they later launch).
- Parser-side filtering on `freminal=1` is correct and complete: other
  emitters' markers are silently dropped without touching the user's
  shell configuration.

The cost is microseconds-per-prompt of dropped marker traffic, which is
not measurable in practice.

---

## Render Performance: the Phase 0 / Task 121 outcome (Task 124.8)

### Why this entry exists at all

`Documents/DECOUPLING_FRAMEWORK.md` §2A is the source of truth for the
Phase 0 measurements, and `PLAN_121_PERF_REMEDIATION.md` is the closed
record of the work. This entry exists because both are documents a future
reader has to know to go looking for, and because the durable lesson is not
in either one's conclusions — it is in **how often the conclusions were
wrong**.

### The direction, stated plainly

**Phase 0 weakened rather than strengthened the case for replacing egui in
the main window.** It was undertaken partly expecting to justify a rewrite.
It did the opposite.

The strongest maintenance argument on the table had been "we carry 13
undocumented assumptions about egui internals for an optimisation that never
fires". Finding 1 established that the optimisation never firing was a
**one-line bug** — `WindowEvent::RedrawRequested` drove every frame, egui-winit
reported it as input, and the chrome-input gate consumed that as evidence of
input, so the event that drove the frame disqualified that same frame. Fixed,
`ChromeMode::Replay` went from 0% to 100% duty cycle at idle, chrome
construction 69 us -> 10 us, total frame 434 us -> 376 us.

The performance case is spent. Idle was already 0.073% of a core; pointer
motion went from about 2% to about 0.08%. **Nothing measured justifies a
multi-version rewrite on CPU grounds.** The maintainer position at the end of
Phase 0 was _leaning against the rewrite, explicitly undecided_, and it rests
now on maintainability and undocumented-internals reliance, not speed.

### The inconvenient numbers, recorded deliberately

An entry that listed only the wins would misrepresent the method, so:

- **Task 121 closed with four of six issue #459 candidate items refuted by
  their own verification step.** The refutations were the deliverable, not a
  failure of it. A profiling task whose candidate list survives contact
  intact has probably not been checking.
- **Task 121 itself was closed as an umbrella**, not completed. It reached
  36 subtasks across seven groups, executed out of order, with much of
  Groups F and G predicated on a chrome cache that had since been disabled
  by default. It stopped working as a tracker and was retired to a
  historical record.
- **The chrome cache was re-disabled after being fixed.** Finding 1 made it
  engage and deliver; 121.32 then turned it off by default anyway, because
  `ChromeMode::Replay` skips _constructing_ chrome widgets and egui resolves
  hit-testing against the previous pass's widget set — so unbuilt widgets are
  uninteractable. That shipped as a tab-click and pane-border-drag regression
  in 0.12.0-beta.7. A measured, real speed-up was reverted for correctness,
  and that is the right trade every time.
- **Task 123's Obligation 2 verdict was itself wrong, and was corrected on
  2026-08-21** after the maintainer noticed that its two tests had assumed
  away the very mechanism they were meant to test. The corrected verdict was
  _partially refuted_: the symptom was real, but the accused predicate
  (`pointer_forces_full_present`) was innocent; the actual cause was the
  `Unchanged -> Full` fallback in `decide_frame_damage`.

That last item is the reason this entry is worded the way it is. **The
verification step caught a wrong diagnosis; then a human caught a wrong
verification.** Both layers were necessary, and a record that shows only the
successful refutations would teach the wrong lesson about how much to trust
a test that agrees with you.

### Durable principles

1. **Measure before optimising, and treat refutation as success.** Four of
   six candidates were wrong. The cost of checking was far below the cost of
   having "fixed" them.
2. **A test that confirms your hypothesis deserves more scrutiny than one
   that refutes it.** Obligation 2's first pass produced a comfortable
   confirmation by assuming away the mechanism.
3. **Correctness outranks a measured speed-up.** The chrome cache was faster
   and was still disabled.
4. **Justify render work on the right unit.** Task 123 found the waste is
   overwhelmingly **bandwidth** — roughly 350x the bytes for roughly 1.08x
   the calls. Call counts are the intuitive metric and were the wrong one.
5. **Docs describing a removed mechanism are worse than no docs.**
   `decide_frame_damage`'s comment still named a `pointer_moving` term that
   issue #459 had deleted, and it stated precisely the hypothesis under
   investigation — a reader checking the docs would have taken a false
   confirmation of a refuted diagnosis (fixed as 124.C1).

---

## Desktop Notification Backends (Task 150)

### What `freminal-notify` owns, and what stays a caller

"Show this notification on the operating system" has one owner: the
`freminal-notify` crate (`freminal-notify/src/`). It contains the OS backends (the Linux/BSD
D-Bus service in `linux/`, the macOS/Windows plain worker and the macOS/Windows
OSC 99 untracked worker), the icon pipeline (`icons/`: decode, raster, temp
files, lookup), `presentation.rs`, body-markup escaping (`markup.rs`), the
daemon capability types and the backend interface types.

These stay outside it, as callers:

- `gui/osc99/`: the OSC 99 registry, filter, pane state, effects, glue, toast
  spec and the `g=` icon cache. They are protocol state, not delivery.
- `gui/notifications.rs`: routing, and the mapping from a `NotificationRequest`
  to a `PlainNotification`.
- `gui/host_capabilities.rs`: capability policy, including the decision whether
  the service is wanted (`plain_notification_demand`).
- `gui/notify_wake.rs`: the repaint wake. It uses `freminal_windowing`, so it is
  the GUI's adapter. The service only takes a `Box<dyn Fn() + Send>` waker.

### Two request kinds

- **Plain**: fire and forget. OSC 9, OSC 777, OSC 133 `D` command-finished and the
  Settings "Test Notification" button carry a summary, a body and an urgency
  (read on Linux/BSD only). A caller holds a `PlainSink`, a cloneable value with
  one `show(PlainNotification)` method on every platform. A notification that
  cannot be shown is dropped.
- **Tracked**: OSC 99, where freminal observes the notification afterwards. The
  registry hands a `NotificationInstanceId` and a `&Notification99Data` to a
  `TrackedBackend` (the Linux/BSD service), which queues the request and reports
  `Shown` / `ShowFailed` / `Closed` / `Activated` / capability events back as
  `SystemNotifyEvent`s. macOS and Windows cannot observe a notification, so their
  system leg is an `UntrackedBackend`: shown and forgotten, with the same
  `Notification99Data` payload and no events.

`SYSTEM_LEG` (`Tracked` on Linux/BSD, `Untracked` elsewhere) is a `const` of a
two-variant enum defined with `cfg!`, so every platform compiles both arms and
the OSC 99 router picks the registry's leg from it without a `#[cfg]`.

### The one-surface rule

Outside code names only what `freminal-notify/src/lib.rs` re-exports, plus
`freminal_notify::test_support` (behind the `test-utils` feature: the
recording backends, the fake-daemon rig and the test assets). No path such as
`freminal_notify::linux::state::...` appears outside the crate, and nothing
inside it imports the GUI, egui, `freminal_windowing` or
`freminal_terminal_emulator`. The crate depends only on `freminal_common` and
external crates.

### `DesktopNotifier` and `ServiceDemand`

`DesktopNotifier` is the single `FreminalGui` field that owns the backends: the
service (started on first demand, kept for the process) and the untracked
backend. Its methods exist on every platform (`ensure_started`, `plain_sink`,
`with_backends`); `is_started` and `drain_events` exist on Linux/BSD only, where
the events do. Callers never name a platform.

Whether the service is wanted is policy, and the policy stays with the caller:
it passes a `ServiceDemand { Wanted, NotWanted }` and the notifier only obeys.
A cell that is borrowed elsewhere never panics. The notifier falls back to a
sink that drops, or to the failing `NoTrackedBackend`, because showing a
notification calls nothing back into the GUI, so a busy cell is transient.
Dropping the notifier drops the service first and the untracked backend second.

### Injected assets

`NotifyAssets { app_icon_png, icon_font }` is built once by the GUI
(`app_icon::notify_assets()`) and passed to every backend constructor. The
`include_bytes!` calls stay in the GUI, so the backend never reaches for the
application icon or the font manager. The eight standard icon names are one
table, `StandardIcon`, which carries the kitty name, the freedesktop name and the
glyph codepoint; the GUI's `ChromeIcon` converts from it.

### Two macOS/Windows worker lifecycles, deliberately not merged

| Worker                    | Thread                    | Lifecycle                                                                                                                         |
| ------------------------- | ------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| Plain (`plain_worker.rs`) | `freminal-notify`         | Process-wide static. Started on first use, restarted at most every 5 s if it exits, never shut down. Queue of 16.                 |
| OSC 99 (`untracked.rs`)   | `freminal-notify-deliver` | Owned by the untracked backend, spawned on the first notification, never restarted, joined (bounded at 500 ms) on drop. Queue 16. |

Both are bounded queues that drop on full, so neither can stall the frame
thread. They were kept apart because each lifecycle is load-bearing: the OSC 99
worker owns the icon temp-file store and must remove it on exit, and a slow
macOS delivery for one kind must not stall the other. Only the non-blocking
offer to a bounded queue is shared (`queue.rs`). The two outcome types differ
(the plain one reports a suppressed respawn and hands a disconnected item back
for retry; the untracked one logs inline), so merging them would have changed
behaviour. On Linux/BSD both kinds go through the one D-Bus service, where plain
notifications have their own budget of 16 inside the service's shared command
queue, so a flood of one kind cannot starve the other.

Threads are named `freminal-notify-dbus`, `-owner` and `-signals` (the D-Bus
service), `freminal-notify-icons` (the icon-theme scan) and the two workers
above.

### The icon directory keeps its old name

Icon files live in `<temp dir>/freminal-osc99-icons-<pid>`. The constant was
renamed (`ICON_DIR_PREFIX`) but its value was not: a process that is killed
leaves its directory behind, and the startup sweep removes siblings by that
prefix. Renaming the value would orphan every directory left by an earlier build
for good. The probe file prefix (`freminal-osc99-probe-`) is unchanged for the
same reason.

### Tests never reach the OS

Only GUI production code constructs a sink that reaches the OS:
`PlainSink::platform_worker` is called only from `DesktopNotifier::plain_sink`,
and `PlainSink::through_service` needs a running service's sender. A test uses
`PlainSink::nowhere` or the `PlainSink::recording` recorder, or runs a service
against the fake-daemon rig on a private peer-to-peer bus. This is a correctness
requirement, not tidiness: on macOS `notify-rust`'s `show()` throws an uncatchable
Objective-C exception from a binary that is not an `.app` bundle, as a
`cargo test` binary is, and the process aborts during teardown.

### The crate

`freminal-notify` is its own crate (Task 150.10) and depends only on
`freminal-common`. That is why zbus, `tauri-winrt-notification` and `notify-rust`
and their platform `cfg`s live in it rather than in `freminal`. Its submodules
are private; the crate's public API is what `lib.rs` re-exports.

`test_support` is compiled for the crate's own tests and for any build that
enables the `test-utils` feature. `freminal` enables it in its
`[dev-dependencies]` only, so a production build never contains the recording
backends, `PlainSink::recording` or the fake daemon. The feature also turns on
`zbus`'s `p2p` feature, which the fake daemon's socket-pair connection needs.

An exported item of a library is never dead code, so the crate carries no
dead-code exceptions: the `NotifyAssets` fields and the `StandardIcon` methods
are plain `pub` items with no `cfg` gate or `expect`.

---

## Workspace Lint Configuration (issue 552)

### Where the lints live

The clippy lint set is declared once, in `[workspace.lints.clippy]` in the root
`Cargo.toml`, and inherited with `[lints] workspace = true` by every first-party
crate: `freminal`, `freminal-buffer`, `freminal-common`, `freminal-notify`,
`freminal-terminal-emulator`, `freminal-windowing` and `xtask`. The crate-root
`#![deny(...)]` blocks that used to repeat the set in every `lib.rs` / `main.rs`
are gone. `xtask` previously lacked the `cargo`, `unwrap_used` and `expect_used`
lints and now gets the full set. The vendored `portable-pty` crate stays opted
out (it does not inherit workspace lints), because it is third-party code that
we do not hold to our bar.

### Why `[workspace.lints]` and not crate-root attributes

Workspace lints are passed to every target of the crate (tests, integration
tests, benches, `build.rs`). A crate-root `#![deny]` attribute only reaches the
crate it is written in, so test and bench targets were silently held to a lower
bar. Moving to the manifest tightened them, and the findings that surfaced in
tests, benches and `build.rs` were fixed in the same change.

### Test code and `unwrap` / `expect`

`unwrap_used` and `expect_used` are denied for production code. `clippy.toml`
sets `allow-unwrap-in-tests` and `allow-expect-in-tests`, which exempts
`#[test]` functions and `#[cfg(test)]` code, matching the repository rule that
`unwrap()` / `expect()` are forbidden outside tests. That exemption does not
cover non-test helper functions inside integration-test files (`tests/`) or
bench files (`benches/`), so those files carry a file-level `#![allow(...)]`
with a comment saying why. Production code never does.

### `multiple_crate_versions` is allowed, and where duplicates are tracked

`clippy::multiple_crate_versions` (part of the `cargo` group) is explicitly
allowed. Every duplicate crate version is **transitive** and cannot be fixed by
changing one of our direct dependencies: they are dominated by the exact-pinned
egui 0.36 / winit 0.30 windowing stack (objc2 0.5/0.6, smithay-client-toolkit
0.19/0.20, calloop, rustix 0.38/1, windows-sys 0.52-0.61), plus `thiserror` 1
via `conv2` and `getrandom` 0.3 via `ahash`. A lint that can only be satisfied
by moving that stack would be permanently red or permanently suppressed.
Duplicate tracking therefore moved to `deny.toml` (`[bans] multiple-versions =
"warn"`), where `cargo deny check` lists them without failing the build.
**Revisit when the windowing stack moves**: if the duplicates shrink
substantially, tighten `deny.toml` to `deny` and drop the clippy allow.

---

## Security Tooling: Dependency Advisories and Fuzzing (issue 553)

Procedures live in `fuzz/README.md`; this section records only the decisions
and the reasons.

### Dependency advisories: cargo-deny, not cargo-audit

`cargo deny --locked check` runs on every pull request as the `deny` job and is
part of the required `CI Success` gate, so a new advisory, banned crate,
disallowed licence or unknown registry blocks the merge. Advisories also appear
between PRs, so `security-audit.yml` runs `cargo deny --locked check
advisories` daily against `main`. On failure it opens (or comments on) one
tracking issue, "Security audit: cargo-deny advisories failing on main"; when a
later run passes it comments and **closes that issue itself**, so there is at
most one open issue and it reflects the current state.

Every `ignore` entry in `deny.toml` must carry a reason that references a
tracking issue, and is removed when the advisory is resolved. There are none at
the time of writing; #567, for the zbus 5 crates merged into zbus 6, was the
last (resolved by Task 149). `deny.toml` is the authoritative list.

`cargo-audit` is deliberately **not** used. It reads the same RustSec advisory
database as cargo-deny, so running both would report the same findings twice
and add a second ignore list to keep in sync.

### Fuzzing design

- **Standalone crate, separate shell.** `fuzz/` is excluded from the workspace
  and has its own `Cargo.lock`, because it needs a nightly compiler and
  sanitizer flags that must never reach a normal build. Its toolchain is the
  `fuzz` dev shell with a nightly pinned by date (`fuzzNightlyDate` in
  `flake.nix`), so `nix flake update` cannot move it silently.
- **Three kinds of input, kept apart.** `seeds/` is curated, small, committed
  and never written by a run. `regressions/` holds minimized reproducers of
  fixed bugs, committed with the fix. `corpus/` and `artifacts/` are local and
  gitignored. Keeping them apart means the committed trees are deterministic
  and a run can never rewrite them; seeds and regressions are also byte-exact,
  so they are excluded from the pre-commit text fixers and marked `binary` in
  `.gitattributes`.
- **Three CI tiers.** `fuzz-replay` is **required**: it runs every seed and
  regression file once, as an individual input (libFuzzer only names the
  failing file in that mode), so a failure is a real regression
  and never flaky. `fuzz-smoke` is **advisory** (`continue-on-error`): a short
  time-boxed run is stochastic, and gating merges on a random search would make
  CI flaky. `fuzz-nightly.yml` is the real fuzzing: it runs on a self-hosted
  runner with `--on-crash keep-going` and a **persistent corpus** (pulled from
  and pushed to the corpus repository, below), because coverage accumulates
  over days and a fresh run starts from nothing otherwise. It is triggered only
  by `schedule` and `workflow_dispatch`, never `pull_request`, since a
  self-hosted runner on a public repository must not execute fork-controlled
  code.
- **The persistent corpus lives in a separate private git repository**
  (`fredsystems/freminal-fuzz-corpus`, layout `<target>/<sha1>`), synced by
  `cargo xtask fuzz-corpus pull|push`. Alternatives rejected:
  - _In this repository:_ every clone and every CI checkout would fetch the
    corpus and its ever-growing history, for data almost nobody needs.
  - _Actions cache or artifacts:_ both are evicted or expire, which loses
    exactly the accumulated coverage the corpus exists to keep.
  - _The runner's disk:_ ties the corpus to one machine and one wipe of its
    work directory. It now holds only build cache.

  The corpus is small: after a 90 s run plus `fuzz-cmin`, `ansi_parser` held
  3,286 files / 868 KB, `frec_parse` 3,284 files / 2.8 MB and `terminal_state`
  3,262 files / 2.7 MB; individual files are hundreds of bytes to a few KB.
  libFuzzer names a corpus file by the SHA-1 of its content, so names are
  content-addressed: machines that find the same input add the same file and a
  rebase never conflicts, which is what lets `push` simply rebase and retry.
  The repository is **private** because corpus inputs sit close to the inputs
  that crash the code, including crashes behind advisories that are still
  open. Its history exists only to make concurrent pushes mergeable and may be
  squashed whenever it grows. CI authenticates with the `PAT` repository
  secret, handed to git through an inline credential helper so the token is
  never on a command line or on disk.

### Sandbox decision

`terminal_state` can drive kitty graphics `t=f` / `t=t` / `t=s` transmission,
which reads arbitrary paths and deletes kitty-marked files under `/tmp` and
`/dev/shm`. Filtering that in the harness would mean fuzzing something other
than the production code path, so the process is isolated instead: locally,
`xtask` runs every fuzz binary under bubblewrap by default (read-only root,
private `/tmp` and `/dev/shm`, no network). In CI, bubblewrap cannot create user
namespaces on hosted runners or under the self-hosted runner's hardened systemd
unit, so CI passes `--sandbox none` and relies on isolation that already exists:
a disposable VM for hosted jobs, the hardened unit for the self-hosted runner
(the nixpkgs `services.github-runners` defaults: `DynamicUser`, `PrivateTmp`,
`PrivateDevices`, `ProtectSystem=strict`, `ProtectHome`; the runner itself is
provisioned in the separate NixOS configuration repository).
The runner's hardening was **deliberately not relaxed** to make bubblewrap work;
weakening the host's isolation to enable a second layer would be a net loss.

### Keep-going exit-code classification

In libFuzzer fork mode the parent exits with the **last child's** exit code. A
naive "non-zero means failure" check therefore misreports a completed keep-going
run as broken whenever the final worker happened to hit a finding, and hides
real infrastructure failures among the findings. `xtask` classifies the codes
instead: 70 (timeout), 71 (out of memory) and 77 (crash) are findings (fatal in
stop mode, a warning pointing at the artifacts in keep-going mode); 72
(interrupt), death by signal and any other code are infrastructure failures in
both modes. AddressSanitizer exits `1` by default, which collides with
libFuzzer usage errors, so `ASAN_OPTIONS` gets `exitcode=77` appended and every
sanitizer finding reports as a crash. The exit code is never the finding
signal in keep-going mode, in either direction: after the time budget stops the
workers, the last one usually exits `0` even when earlier ones found crashes, so
`xtask` counts new artifact files and reports them, and the nightly workflow
counts them too. A non-zero exit from a keep-going run means infrastructure
trouble.

### Miri: deferred

Miri is **not** adopted for now. `freminal-buffer` and `freminal-common`, the
crates whose logic is pure Rust, contain no `unsafe` in production code (the
only `unsafe` in their `src` is `std::env::set_var` / `std::env::remove_var`
in test modules), so Miri would have nothing to check there. The `unsafe` that
remains is GL, mmap and other OS FFI, which Miri cannot execute. **Revisit if
pure-Rust `unsafe` is introduced** (for example in a hot-path data structure).

### AddressSanitizer is already covered

cargo-fuzz builds fuzz targets with AddressSanitizer by default, so memory
errors in any `unsafe` or FFI code **reachable from a fuzz target** are already
detected. Code that no fuzz target reaches is not covered by this.

### The fuzz crate's own dependencies

`fuzz/` is outside the workspace, so the `deny` job and `security-audit.yml`
do not see `fuzz/Cargo.lock`. That is accepted: the fuzz crate never ships, and
its only direct dependencies beyond the workspace crates are `libfuzzer-sys`
and `toml` (`arbitrary` is pulled in transitively by `libfuzzer-sys`).
