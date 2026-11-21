# Design Decisions

Durable architectural decisions and reference data extracted from completed task plans.
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

**Automatable:** Menus 1 (cursor), 2 (screen), 3 (G0 to G3 charsets), 6 (reports), 7 (VT52),
8 (insert/delete), 9 (VT100 bugs), 10.1 (RIS), 11 (non-VT100 extensions).

**Skipped:** Menu 3 NRC sets other than UK and ISO Latin (not implemented; G0 to G3,
SO/SI and the single shifts are, since Task 147.20), Menu 4 DECDWL/DECDHL (renderer not
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

## Background Color Erase (BCE) (Task 48, refined by Task 140)

Cells blanked by an erase take the **current** SGR background color, not a
fixed default. The old behavior of writing `default_cell()` was incorrect and
user-visible: `tput clear` with a non-default background produces a one-color
field in other terminals but not in pre-Task-48 Freminal.

Which operations, and which attributes, are decided as follows (Task 140,
2026-11-19):

- **Erased cells take only the background.** ED, EL, ECH, ICH, DCH, the
  column-confined fills used while DECLRMM is enabled (these also cover IL, DL,
  SU, SD and margin-triggered scrolls) and the alternate-screen clear all blank
  with one tag, built by `Buffer::erase_tag()`: the stored background colour of
  the current format and nothing else. No foreground, weight, underline,
  reverse, blink or OSC 8 hyperlink is copied, so an open hyperlink no longer
  leaks into erased cells. The stored background is used as it is, so SGR 7 does
  not swap it (xterm's `cur_background`). With a default background the tag is
  visually default and rows erased with it stay sparse. Background-only is the
  tie-break the project applies when the reference terminals disagree: xterm
  and Ghostty blank with the background only, kitty (`line_apply_cursor`) and
  WezTerm copy the whole pen, and freminal follows xterm and Ghostty, consistent
  with the Task 131 alternate-screen clear. It is a deliberate difference from
  kitty.
- **DCH fills the cells it vacates at the right edge** with the same tag, as
  xterm and kitty do; with a default background the row stays sparse.
- **Whole-row IL, DL, SU, SD and line-feed scrolling fill with default
  attributes, not BCE.** The rows they create are `Row::new` with no format
  (kitty parity), which keeps scrollback rows sparse. Earlier text here, and
  the escape-sequence coverage table, said IL and DL were BCE-aware; that was
  wrong and was corrected in Task 140. Only the DECLRMM column-confined variants
  take the background, because they blank cells inside an existing row rather
  than creating one.
- **The erase methods read the format from `Buffer`**, through `erase_tag()`,
  rather than taking it as a parameter. Task 48's text described the current
  format as an explicit parameter of every erase method, but the code read the
  buffer's current format; one helper that every blanking site uses is what
  keeps the rule above uniform.

---

## DECDWL / DECDHL Line Width (Task 49)

DECDWL (double-width lines) and DECDHL (double-height lines) are modeled as a
`LineWidth` enum on each row: `Normal`, `DoubleWidth`, `DoubleHeightTop`,
`DoubleHeightBottom`. The cursor column is **halved** when on a double-width
line — writing at logical column 40 on a double-width line puts the cursor at
physical column 80. Cursor horizontal movement commands (CUF, CUB) scale by
the line's width.

Both halves of a DECDHL pair are rendered. Each row draws its glyphs scaled to
twice the cell height from the pair's top (the bottom half is shifted up one
cell), so each row shows its own half of the doubled glyph. Underline and
strikethrough are laid out in the same doubled box and clipped to the row
(Task 140). (Earlier text here said only the top half was rendered; that no
longer describes the code.)

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
   _Superseded by Task 112: chrome glyphs come from `ChromeIcon` in the
   bundled font; see "UI Chrome Styling and Bundled Assets"._
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
   env vars (`ENV` for bash, `ZDOTDIR` for zsh, `XDG_DATA_DIRS` for
   fish) on the child process before spawn. The shell's standard
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
take the `fid` and use it for explicit A/D correlation. There is no
"most-recent-open" fallback: a D whose `fid` matches no open block is a
no-op.

`[shell_integration] set_term_program` gates both `TERM_PROGRAM` and the
shell injection, because the two are coupled: a user who opts out of one
is opting out of freminal's shell hooks.

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
A `cargo test` invariant asserts that the marker line in every shipped
script agrees with the constant. It does not detect an edited script whose
marker and constant were both left unchanged, so bumping the version after
editing a script is a discipline, not a CI gate.

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
  `.zshrc`, etc. inside it. Our `.zshenv` (written independently; the
  same restore-then-chain sequence Ghostty and kitty use) restores the
  user's original `$ZDOTDIR` from `$__FREMINAL_ZSH_ZDOTDIR` (a sentinel
  env var we set before spawn), sources the user's real `.zshenv`, then
  sources our `freminal-integration` script. zsh's `${+name}` test
  distinguishes "user had `$ZDOTDIR` set even if empty" from "user did
  not have `$ZDOTDIR` set at all".
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
- Shell binary is unknown (not bash/zsh/fish) — no env injection.
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

## Command Blocks, Gutters and Fold (Tasks 72, 73)

### Block lifecycle and coordinates

A `CommandBlock` is created by OSC 133 A, annotated by B and C, and closed by D, all correlated by
`fid` (see "Shell Integration Architecture"). Blocks live on the primary screen only: alternate
screen entry and exit neither clear nor extend them, and the GUI suppresses the gutter, hover and
duration label while the alternate screen is active. A block's row fields are buffer row numbers,
never snapshot rows. Anything that draws or hits a block must translate to the visible window first
(`translate_ranges_to_snapshot`); feeding buffer rows to a snapshot-space structure silently drops
every block that is not in the first screenful.

Fold state is view state (`ViewState`, keyed by `CommandBlockId`), never buffer state. When a block
is evicted its fold is dropped silently, and folds are not persisted across sessions or layouts.

### Duration is C to D, and an unexecuted block has none

`duration()` measures `executed_at` (OSC 133 C) to `finished_at` (D). The first version measured A to
D, which includes the time the user spent reading and typing, so an instant `ls` after a 30 s pause
reported 30 s. A block that never saw C (Ctrl-C at an idle prompt) has no duration and never raises a
command-finished notification: `executed()` is the shared predicate, and the notification path checks
it explicitly so a zero threshold cannot produce a false notification. A still-running block extends
to the cursor row, not the pane bottom.

### Fold anchor and `FoldPreviousCommand`

The fold range starts at `output_start_row` (C) and falls back to `command_start_row` (B) only for a
shell that never emits C; starting at B hides the prompt and the typed command. `FoldPreviousCommand`
folds the most recent completed block, not the block under the cursor: the PTY cursor is always on the
live prompt, after every completed block, so a cursor-containment rule never fires. The placeholder's
"N lines hidden" uses the block's full height, not the clipped on-screen range.

### Transport: a dedicated channel, not a `WindowCommand` variant

Finished blocks reach the GUI over a per-pane `Sender<CommandFinishedEvent>`, drained by the PTY
consumer thread. `WindowCommand` only wraps viewport and report manipulations; a `CommandFinished`
variant would have pulled the GUI enum into the handler crate or restructured the PTY loop.
`Pane::recent_commands` is a ring capped at 64, so a long-lived shell cannot grow GUI memory without
bound.

### The gutter

- **One inset, two consumers.** The left inset is computed once per frame and used for both the column
  count sent to the PTY and the terminal rect the renderer draws. Deriving either independently
  desynchronises wrapping and cursor addressing.
- **Hit zone.** The whole inset is the click and hover target. Gutter pixels lie outside
  `terminal_rect`, so DEC mouse modes, selection and URL hover never see them.
- **The gutter is the only hover trigger.** Tinting a block when the pointer was over any of its cells
  fired during text selection and with mouse-tracking applications, so the cell trigger was removed.
- **The duration label floats** just inside the cell grid, anchored to the block's last visible row,
  instead of widening the gutter or sitting on the prompt row, which scrolls away first.
- **Live hover needs two things.** The gutter must be a `Sense::click()` egui region, or a bare
  pointer move never wakes a frame; and a hover-only change must rebuild the background instance
  buffer, because the tint is baked into it. Either alone shows nothing.

### Command-history palette (Task 72.15)

The palette merges the shell-history seed with this session's live commands, kept parallel
(`history_seed` beside `recent_commands`) and merged at render time. Extending `CommandBlock` with
command text was rejected: every buffer-row consumer would have to handle a variant with no rows. The
merge is newest-first with a global text dedup, because the rendered list is capped after filtering;
an oldest-first merge pushed this session's commands past the cap. The binding is `Ctrl+Shift+M`:
`Ctrl+R` is the shell's reverse search, `Ctrl+Shift+R` recording, `Ctrl+Shift+P` the command palette.
Selecting an entry types it without a trailing newline, so the user reviews and presses Enter.

OSC 1338 exists because `HISTFILE` is often a shell variable set in `.zshrc`, invisible to the
environment freminal spawned the shell with. The shell reports its evaluated value after its rc files
have run, and a sequence-tagged seed lets that report win regardless of arrival order.

## Paste, Broadcast and Close Guards (Tasks 74, 77, 98)

### Paste guard

- **Where it runs.** The `input.rs` paste sites have no `Config`, no compiled-regex cache and no
  per-window dialog state, so paste is deferred to `update()`, which has all three. `PasteGuard`
  (compiled regexes) lives on the GUI, not on the serde config value. Bad regexes are reported, not
  fatal at load (`freminal-common` has no UI to toast from), and are skipped at match time.
- **The clipboard is read once, by the windowing layer.** On Ctrl/Cmd+V the event loop reads the
  clipboard through the cross-window egui path (Wayland's per-window clipboard breaks `arboard`) and
  injects `Event::Paste`; menu "Paste" arrives as `ViewportCommand::RequestPaste` and becomes the same
  injection. Re-reading the clipboard in the GUI fails on the common path.
- **Bypass.** `PasteUnsafe` defaults to `Ctrl+Shift+Alt+V`, because `Ctrl+Shift+V` is `Paste`. The
  interceptor converts any `command + v` into `Event::Paste`, so the bypass is recognised by the Alt
  modifier on that event, not by the `BindingMap`.
- **Target is captured at open.** The dialog records the destination `(TabId, PaneId)` when it opens.
  With focus-follows-mouse, moving onto the dialog's buttons changes the active pane, and resolving
  against the current active pane pasted into the wrong pane. A closed target drops the paste.
- **Triggers.** Multi-line, control characters (C0/C1 except `\n`, `\r`, `\t`) and an opt-in pattern
  list. A bad regex never blocks paste.

### Broadcast

Broadcast is per tab and keyboard-only. Mouse, scroll-derived arrow keys, mouse-tracking reports,
resize, focus and selection are pane-local. A key or paste is broadcast as the active pane's encoded
bytes (bracketed-paste wrapping decided by the active pane), not re-encoded per destination; Task 137
owns encoding per destination.

### Close guard

A pane is "running" only when its latest block has seen C and not D; an idle prompt, or a prompt after
Ctrl-C with no C, never blocks close. Detection is OSC 133 only: `/proc` or process-tree introspection
was rejected as platform-specific and unnecessary now that shell integration is injected by default. A
pane that never saw a prompt marker is "unknown" and blocks only if `unknown_blocks` is set, which keeps
`vim`, `cat` and shells without integration closeable. Window close goes through `on_close_requested`;
a confirmed force close is remembered per window so the `ViewportCommand::Close` round trip is not
re-prompted. The dialog shows the pane title, because `CommandBlock` carries no command text.

## Tab Titles, Focus and the Active-Pane Highlight (Tasks 94, 109, 110)

- **Tab title precedence.** The default policy is `prefix` (`"{custom}: {osc}"`). The old behaviour
  cleared a custom name whenever the shell set an OSC 0/1/2 title, which made renaming pointless under
  any prompt framework. The OSC title always stays in the snapshot; the policy decides what is shown,
  and `OscWins` is the only policy where an OSC title clears `custom_name`. Persisted names are
  `LayoutTab::custom_name`, distinct from the author-supplied `title` seed (`LAYOUT_FORMAT.md`).
- **Focus follows the mouse** (default on): hover changes only the keyboard target and never retargets
  in-flight mouse input. Tab switching stays click-to-focus.
- **Active-pane highlight.** With three or more panes, every interior divider edge of the active pane
  is highlighted over its full length; outer window edges never are. With exactly two panes they share
  one divider, so surrounding either lights the same line; that divider is half-filled on the active
  side (classic tmux). The trigger is whole-tab pane count: a per-divider rule wrongly split the shared
  bar in the three-pane case.

## Startup Failure and Teardown (Tasks 106, 107)

- **A window with no `PerWindowState` renders nothing**, so a failed only-shell spawn used to leave a
  blank, unrecoverable window. The app-level `fatal_error` panel renders for that window and offers
  Exit. A failed sub-pane with other shells alive is only a toast plus a skipped pane.
- **Bash injection must keep the shell as `args[0]`.** On a default-program builder the first pushed
  argument becomes the executable, so `cmd.arg("--posix")` made bash unspawnable. Use
  `replace_default_prog([shell, "--posix"])`.
- **Reader shutdown is classified, not silenced.** The PTY consumer thread exits first when the GUI
  drops the input channel, so the reader may legitimately fail one last send. A flag set by the consumer
  before it exits distinguishes expected teardown (`debug`) from a receiver that vanished early
  (`error`).
- **Every window destroys its egui-glow painter** with the GL context current (`close_window()` and
  `exiting()`; `destroy()` is idempotent). The standalone Settings window is its own winit window.
- **Version embedding had three independent `unknown` paths:** a local build (no `rerun-if-changed` on
  git `HEAD`/refs), CI (shallow, tagless checkout; build jobs use `fetch-depth: 0` and
  `fetch-tags: true`), and Nix (`cleanSource` strips `.git`, so the flake passes `v<version>-<rev>`
  through an env override `build.rs` honours verbatim). Fixing one leaves the other two broken.

## Notification Defaults and Detection (Task 76)

Delivery is in "Desktop Notification Backends (Task 150)". Notifications are opt-in
(`[notifications] enabled = false`) because they are noisy and platform-dependent, while the paste guard
is on by default because it prevents real foot-guns. OSC 9 and OSC 777 have no capability query, so
clients detect freminal through `TERM_PROGRAM=freminal`. The Settings "Test Notification" button
bypasses the `enabled` master switch (but not per-category routing).

## Layout Environment Scope (Task 75)

Only per-pane `env` is supported. A layout-wide env table, per-layout theme or font, and profile
binding are deferred to Profiles (Task 78); per-pane env covers the realistic cases.

---

## Auto-Detected URLs (Task 71.7b)

Plain URLs (`http`, `https`, `file`, `ftp`, `mailto`) in output from programs that do not
emit OSC 8 are made clickable by the same hover/click machinery as OSC 8.

- **Detection runs inside the per-row flatten cache, not on the PTY read path.** The
  rejected design scanned in `TerminalHandler` after each PTY batch and stamped matches onto
  cells, adding allocations per batch on the hot path under bursty output (`cat bigfile`,
  build logs). Auto-detected URLs therefore never touch cells and never enter cell-level
  OSC 8 storage. The cost is paid once per dirty row (`RowCacheEntry` carries `bytes`,
  `byte_to_char` and `auto_urls`) and later frames reuse it. No allocation or per-batch work
  may be added to `handle_incoming_data` / `process_outputs` for this.
- **OSC 8 wins.** Detected ranges are spliced into the tag sequence at merge time, and a tag
  that already carries a URL is never overlaid.
- **`url_detect` lives in `freminal-buffer`**, its only caller; the emulator crate would have
  needed a dependency inversion.
- **Syntactic only.** Byte-based matching (`regex::bytes`) with GFM-style termination:
  whitespace or control bytes end the match, trailing `.,;:!?` is stripped, and closers are
  stripped only when unbalanced, so `.../Foo_(bar)` survives. No validation, no unescaping,
  and no bare-path detection (`/tmp/foo`): users write `file://`, matching WezTerm, iTerm2
  and kitty.
- Toggling `ui.auto_detect_urls` invalidates every cache entry.

## Persisted Runtime State vs Config (Task 71)

`config.toml` is declarative user intent. State the program writes on the user's behalf
(today only `first_run_complete`) lives in a separate, always-writable `state.toml`
(`freminal_common::app_state`; paths are in the module header). The trigger was a NixOS
home-manager user whose `config.toml` is a read-only symlink: mutating it failed with
`EROFS`, raised an error toast, and re-opened the welcome overlay on every launch.
`OnboardingConfig` stays only for back-compat parsing, and a `true` value is migrated
forward. Anything the program mutates on its own (dismissed prompts, tips) goes in
`state.toml`, never in `config.toml`.

## Surfacing Failures: Toasts, Not Modals (Task 71)

Recoverable failures that used to log and vanish (PTY spawn failure, layout load/save
failure, malformed layout files, shader compile errors) surface as non-blocking toasts. A
modal or banner was rejected as more intrusive than the failure warrants.

- Code that cannot reach the GUI state (the GL `PaintCallback` closures) records
  `last_error`, and the main thread drains it at the top of **every window's `update()`**.
  Draining only on the spawn path surfaced a compile error one window creation too late.
- Startup failures must be reachable by the toast system, so the first PTY spawn happens in
  `on_window_created`, not before the event loop (a failure there used to exit silently
  through `main`). A positional `command` also suppresses last-session restore, or the
  requested command never runs.
- Auto-save failures during shutdown are log-only: the UI is already tearing down.

## Channel Boundedness (Task 70)

A blanket "bound all channels" was rejected after a per-channel audit.

- The PTY read path is already backpressured by the OS pipe. Bounding it with _block_
  duplicates the kernel; bounding it with _drop_ corrupts output (bytes lost mid-sequence).
- `input_tx` is unbounded on purpose: it carries keystrokes, which must never be dropped. Its
  reply traffic is bounded at the source by the per-pane window-command budgets (Task 148).
- Only diagnostic traffic may drop on overflow: the FREC writer channel and the notification
  workers are bounded and drop on full, so they cannot stall a frame.

Any future bound needs measured high-water depths across platforms and workloads
(`cat large_file`, `yes`, `find /`, recording on and off); without that data a bound is a
guess.

## Library-Crate Hygiene (Task 70)

- **`TerminalHandler` belongs in `freminal-terminal-emulator`.** It is escape-sequence
  semantics, graphics protocols and PTY write paths; `freminal-buffer` stays a pure data
  model.
- **Thread names** are `freminal-<role>-<pane_id>`. `pane_id` alone is used because it is
  process-globally unique and the tab id is not reachable from the crate that owns the PTY
  threads.
- **Open-ended numeric namespaces stay `usize`.** SGR and DA2 parameters are not closed sets,
  so they are not enums. Closed sets (ED, EL, TBC) are enums.
- **"Complete" is audit-gated.** Tasks 26, 29, 30 and 31 were marked Complete while
  violations remained (casts, a bool-to-enum miss, oversized files). A completion claim needs
  a verification pass, not just the plan row.

## Screen Reverse Video (DECSCNM) Rendering (Task 115)

DECSCNM (`CSI ? 5 h`) is a **per-pane** mode, applied at render time to that pane's cells
only. Chrome (menu bar, tab bar, inter-pane background) never reacts to it. Before Task 115
the only consumer was the egui visuals builder, which repainted the whole window from the
_active pane's_ snapshot, so one pane's DECSCNM recoloured every pane. A window-global flag
cannot be correct with more than one pane.

- A cell's effective reverse is `SGR 7 XOR DECSCNM` (DEC/xterm semantics). It is implemented
  without touching `StateColors`: its accessors already return SGR-7-resolved colours, so
  under DECSCNM the vertex builders call the _opposite_ accessor (`effective_fg` /
  `effective_bg` in `renderer/vertex.rs`). Every per-cell colour read in the renderer goes
  through those two helpers; underline and strikethrough follow the same effective state.
- The flag comes from the pane's own snapshot (`ScreenInversion`). The buffer and parser do
  not know about the XOR.

## Text Selection State Machine (Task 116)

A click-drag selection used to clear itself on release and then stay stuck. Three invariants
fix it:

1. **Finalize from tracked state, never from a re-derivation.** Mouse-up commits the
   `selection.end` that `PointerMoved` maintained. Recomputing from the release position went
   through `visible_window_start`, which depends on the snapshot's `total_rows`; PTY output
   between the last move and the release shifted it, the end collided with the anchor, and
   `anchor == end` is the "plain click, collapse" path.
2. **A commit is an edge, distinct from `is_selecting`.** The content-changed auto-clear runs
   after input processing, in the frame where release already cleared `is_selecting`.
   `ViewState::selection_committed_this_frame` suppresses the auto-clear for exactly that
   frame. Do not weaken the `snap.content_changed` (versus `Arc::ptr_eq`) distinction it
   relies on.
3. **Interrupted drags finalize through the same collapse-or-keep rule.** Suppressed input, a
   menu, search, scrollbar drag, or a release outside the origin pane all end a drag without a
   release. `SelectionState::finalize_interrupted_drag` keeps a real range and clears a
   zero-length one, so `anchor`/`end` are never stranded (a stranded selection made the next
   press take the "clear, don't start a drag" branch).

## DECSLRM Scroll Confinement and Half-Width Wrap (Task 117)

- **Confinement has one decision point.** With DECLRMM enabled, every scroll that moves
  existing rows (SU, SD, IL, DL, margin-triggered LF/IND/RI/NEL) is confined to the
  left/right margins by the `scroll_slice_{up,down}_confined(_n)` helpers in
  `buffer/scroll.rs`. A new scroll path must go through them. The first fix patched only the
  primary-buffer helpers and missed the alternate-screen arms of `handle_lf` / `handle_ri`,
  so every scroll change must be tested on **both** buffer types.
- **Half-width wrap halves the span, not the stored column count.** On DECDWL/DECDHL rows
  `row_wrap_bounds` halves the usable span (at least 1), including the DECSLRM span. The
  row's stored column count is unchanged, so line width stays a render attribute everywhere
  except the wrap boundary.

---

## UI Chrome Styling and Bundled Assets (Tasks 111, 112)

### Why CaskaydiaCove Nerd Font is the bundled face

The ligature feature (`liga` + `calt`, on by default) did nothing with the previous bundled MesloLGS
Nerd Font Mono: its GSUB table held only `rtla`. CaskaydiaCove NF is a superset of MesloLGS (full Nerd
set plus Braille) and makes ligatures work out of the box. Constraints for any future font change:

- **Bundle the `Cove` (Code) variant under the name "CaskaydiaCove".** Cascadia Code's OFL reserves
  the name "Cascadia Code"; the Nerd Fonts patch renames it. `CaskaydiaMono` strips ligatures and must
  not be used.
- **`isFixedPitch=0` is benign.** ASCII advances are uniform; the flag is unset only because Nerd icons
  are double-width.
- **Ligatures are `calt` chaining substitutions into "ligature piece" glyphs**, not many-to-one
  collapses: shaped glyph count is unchanged and glyph IDs change. Tests assert on ID difference with
  ligatures on versus off, never on count reduction or a specific ID.
- **Hack was dropped** (never loaded). "Detect the platform default font" was rejected: platform fonts
  have neither ligatures nor powerline glyphs.
- **Licence gate:** no font is bundled until its licence is confirmed to permit redistribution.

### One centralized chrome style layer

All non-terminal chrome (menus, tabs, modals, toasts, dialogs, overlays) is styled by one function,
`chrome_style::build_visuals`, from the active `ThemePalette` plus a `StyleProfile` (`Modern`: rounded,
roomy; `Retro`: square, dense, heavier strokes). The terminal buffer has its own renderer and is out of
scope.

- **Data in `freminal-common`, mapping in the binary.** `GuiTheme` / `StyleProfile` are plain serde
  data with no egui types; `build_visuals` and `apply_chrome_spacing` live in `chrome_style.rs`, as
  `ThemePalette` / `colors.rs` do. Spacing cannot live in `Visuals`, so it is a sibling function.
- **Chrome colours come from authored UI roles, not borrowed ANSI slots.** Deriving chrome from
  `background`/`foreground`/`ansi[..]` produced invisible borders (Tokyo Night's `ansi[0]` is nearly
  its background) and low-contrast active tabs, because a terminal palette declares no UI surfaces.
  `ThemePalette` carries optional `chrome_*` roles, copied from upstream for vetted themes (never
  invented); raw palettes fall back to best-fit, then a contrast-guaranteed derivation. Do not tune
  contrast per theme: a fix is a transcribed upstream role or a resolver improvement for a whole class.
- **Apply visuals globally (`ctx.set_visuals`), never in a scoped `ui.scope`.** egui draws combo-box
  dropdowns and context menus in a top-level popup layer that ignores scoped style.
- **One application point, one cache.** The per-frame style hook rebuilds only when the
  `(display mode, palette, opacity, GuiTheme)` key changes. The standalone Settings window has its own
  egui context and applies `build_visuals` itself; anything new with its own context must do the same.
- **The tuning gallery is a cargo example** (`cargo run -p freminal --example chrome_gallery`), calling
  the real `build_visuals` for every built-in theme. It is a developer instrument, not a settings tab.

### Action glyphs are an icon font, not emoji, SVG or a PNG atlas

On Linux several chrome symbols were emoji codepoints that rendered as tofu without a system emoji
font. Chrome symbols now come from `ChromeIcon`, mapping each variant to a Nerd Font codepoint in the
bundled face, drawn monospace and tinted. Rejected: SVG via `egui_extras` + `resvg` (a heavy crate chain
for about 15 icons) and a pre-rasterised PNG atlas (opaque diffs, manual UV upkeep, one atlas per DPI).
A small supplemental icon TTF is the fallback for a symbol with no Nerd Font equivalent. Icons are
monochrome by design, which is what makes them tintable; every codepoint is checked against the bundled
face's cmap by a guard test.

## Primary-Buffer Height Grow (Task 113)

On the primary screen a height _grow_ appends no rows: it pops trailing pristine `ScrollFill` rows below
the cursor and lets the bottom-anchored window reveal real scrollback above. `rows.len()` may therefore
be less than `height`, as after `Buffer::new`, and the GUI pads below the live bottom. Appending blank
rows (the original behaviour) accumulated an unreclaimable tail under the cursor across grow/shrink
cycles (a tiling WM, repeated font-size changes), stranding the live prompt in scrollback until `clear`.
The alternate screen keeps its strict `rows.len() == height` invariant and top-anchored growth.

## Kitty Keyboard: Raw-Key Delivery and the Transition Rule (Task 114)

egui's `Key` enum has no variant for keypad operators, media keys, or Print/Pause/Menu; winit's
`KeyCode` carries them all. `Handler::window_event` in `freminal-windowing` intercepts only that blocked
physical-key set and calls `App::on_raw_key_event`; egui-winit stays the translator for every other key.
A raw-winit-primary rewrite was rejected as far larger and riskier (Task 137 revisits that choice). The
GUI does not encode in the callback: it queues onto `PerWindowState.pending_raw_keys` and drains in
`update()`, where the active pane's super state, keyboard-mode snapshot and input funnel are in scope.

The protocol reports **transitions; it does not diff state.** Rules for any lock-key or modifier work:

- Never synthesise a press or release from a state delta (for example on focus gain). The protocol
  has no resync event, and a fabricated release corrupts flag-8 consumers.
- Never track or report keys while unfocused, and never add a background keyboard-polling thread.
- On focus loss, clear held-key tracking and emit nothing.
- Ambient lock bits decorate other keys' reports; they are never events themselves.

Why the lock bits are absent is under "freminal current-state deltas: keyboard" in
`KITTY_PROTOCOL_REFERENCE.md`.

---

## Scrollback Memory and Resize Cost (Tasks 118-120)

### Why scrollback rows are stored compactly

A live `Cell` is 72 bytes: an 18-byte `TChar`, a 40-byte `FormatTag` duplicated in full on every
cell, two flags, and an 8-byte `Option<Box<ImagePlacement>>` that is `None` for essentially every
text cell. Adjacent cells almost always share one `FormatTag`. A feasibility spike (100k-line
corpora) measured the saving from run-length sharing of formatting and dropping the image slot:

| Corpus                  | Live cells   | Compact (Task 118) | Compact + LZ4 (Task 119) |
| ----------------------- | ------------ | ------------------ | ------------------------ |
| Shell session (typical) | ~4160 B/line | ~345 B/line (12x)  | ~106 B/line (~39x)       |
| Source / logs           | ~3739 B/line | ~310 B/line (12x)  | ~120 B/line (~31x)       |
| High-entropy coloured   | ~5800 B/line | ~732 B/line (8x)   | ~625 B/line (~9x)        |

The compact form (`CompactRow`) is the only thing safe to byte-compress, because a live `Cell`
holds `Arc` and `Box` pointers. It is lossless. Only rows that have scrolled out of the visible
window are compacted. Rows holding an inline image opt out (rare, and the `Box` complicates the
flat encoding for no gain). A compact row also holds no `row_cache` entry: that second flattened
copy of cold history was part of the saving. The settled cost is ~1.0-1.7 KB/line for realistic
coloured scrollback, so the default limit was raised from 4000 to 10000 at roughly constant memory.

### Compaction lives inside `Row`, not `Buffer`

Making `Buffer.rows` a `Vec<StoredRow { Live, Compact }>` touches ~228 `self.rows[...]` sites across
all of `buffer/`; giving `Row` an internal storage enum confines the change to `row.rs`. Neither costs
anything on the hot paths, so correctness risk decided it. `Row`'s borrow-returning accessors are the
decompaction seam: a compact row materialises to `Live` on first access. The compact form is internal
to `freminal-buffer`; snapshots and the GUI never see it. Resize does not decompact the whole buffer:
`reflow_rows` moves each row's cells out by value (`Row::into_cells`) and only restores LZ4 blocks up
front, because a block is one blob.

### Compaction and compression are idle work, never hot-path work

The most recent snapshot reaching the user is worth more than getting the memory saving immediately.
Compacting synchronously in `enforce_scrollback_limit` put the cost on hot paths (`softwrap_heavy`
+45%, because reflowing one long line to width 10 creates ~420 scrollback rows inside the timed
resize). So:

- No hot path compacts. Rows that scroll into history stay `Live` until idle.
- The PTY consumer thread's `select!` loop has an idle arm that runs a bounded budget of
  `compact_idle_scrollback` then `compress_idle_scrollback` (compact first: only compact rows are
  compression candidates). The thread still owns `TerminalEmulator` exclusively. The arm is not
  re-armed once nothing is left, so a quiescent terminal is never woken forever. Interval and
  budgets are constants in `freminal/src/gui/pty.rs`.
- After the backlog drains, a one-shot `malloc_trim(0)` returns freed pages to the OS (glibc only).

### LZ4 only, in blocks

- **LZ4 (`lz4_flex`, pure Rust, block format) is the only codec.** A zstd tier was dropped: a C
  dependency and a Windows cross-check burden for a ratio gain that does not justify it.
- **Blocks, never per line.** A ~40-byte line compresses terribly and pays fixed per-call overhead.
  A block is 256 rows (~88 KB flat, ~34 us to decompress). Per-call overhead, block granularity and
  allocation churn govern the on-the-fly cost, hence a reusable scratch buffer.
- **Single residency.** A row is `Live`, `Compact`, or in exactly one compressed block, never two.
  Touching a block decompresses it to `Compact` (not `Live`) and removes it from the block store;
  the idle tick compresses it again later. A decompressed-block LRU was planned and not built.
- The visible rows and the compact rows near the viewport are never compressed.

### Resize: coalesce, do not window

Measured 2026-10-08: a full-scrollback reflow was ~100-190 ms at the 10k default and ~1-1.5 s at the
100k maximum; LZ4 added only ~25-50%. The cost is proportional to scrollback depth, not to the width
delta. The real defect was uncoalesced drags: the GUI sends one `InputEvent::Resize` per frame in which
the grid changes, and the PTY thread reflowed every intermediate size in order while processing no
output (a 60-column drag queued ~3.8 s of reflow at 10k). About 70% of reflow time was `Cell`
clone/drop and per-row image-cell counting.

- **Coalesce on the PTY thread**, where the backlog forms. A GUI-side debounce was rejected: it adds
  latency to every resize and cannot help a backlog that forms during a long reflow.
- **Order is preserved.** Only a `Resize` superseded by a later `Resize` is dropped; the first
  non-resize event ends the drain. The child only sees the applied size (`TIOCSWINSZ`), and a FREC
  recording holds one `PaneResize` for it.
- **Move, do not clone.** `reflow_to_width` consumes the rows it discards; the image scan and recount
  are skipped when `image_cell_count` is zero (an exact, debug-asserted invariant).
- Result: 26-67 ms per resize at 10k. Windowed reflow was not built; see `FUTURE_PLANS.md` B.10.

Every width change reflows: font zoom, gutter toggle, pane split, zoom/unzoom, closing a pane, window
snap or maximize, and the Settings preview. Height-only changes do not.

## Orchestration Extraction (Task 122)

### What was actually wrong

`App::update` had grown to 3,132 lines, its `central_body` closure to 2,033, `widget.rs::show` to
1,882, and `write_input_to_terminal` to 1,226 lines with 17 parameters. The drift was entirely in the
frame path, with one nameable cause: **render-time state written during a frame purely so a consumer
outside any frame can read it, with no name, type or invariant.** The out-of-frame pointer-motion
repaint predicate needed toast phase, resize-HUD schedule, toast geometry, pane layout and gutter
widths, and each was cached ad hoc on `PerWindowState`.

Three lifetime classes were separated, because only one was unowned:

- **Intra-frame drains** (`take_frame_damage`, `take_chrome_damage`): drained by
  `freminal-windowing` synchronously after `update()`, reset-on-read. Left alone.
- **Cross-event-boundary reads** from the `CursorMoved` fast path between `update()` calls: given a
  type, `PublishedFrameState`. Each field reaches its final value at most once per completing
  `update()`, from one site; it is one frame stale by construction; an early-returning `update()`
  leaves the previous values intact.
- **Cross-module next-frame caches** on `PaneRenderCache`: noted, not restructured.

### Decisions

- **Modules designed as if they were crates.** Crate extraction needs maintainer sign-off and is
  deferred, not decided (see `DECOUPLING_FRAMEWORK.md`).
- **The `App` trait keeps `egui::Pos2`.** The app side converts to `freminal_common::geometry::Point`
  on entry. Changing the trait would widen a no-behaviour-change refactor into API design the egui
  decision has not been made for.
- **Neutral geometry is scoped to `panes/mod.rs`'s own math.** `Rect::contains` is inclusive on all
  four bounds: `split_rect` gives both halves the same boundary coordinate, and a half-open rule would
  make every split a one-pixel dead stripe.
- **`write_input_to_terminal` is not decomposed.** Its concerns are interleaved, and the apparent
  duplicate is a trap: `control_key` maps `Space` to `Ctrl(NUL)` and has a C0 table,
  `egui_key_to_terminal_input` maps `Space` to a space and has none, and both are right for their call
  sites. Characterisation tests pin the divergence (FUTURE_PLANS B.16). Its parameters were named
  instead (`InputCarryState`, `PaneFocus`).
- **Success criterion: the pointer-motion pane-resolution chain is headlessly testable**, not line
  counts.

### Extraction that does not relocate makes the file bigger

Three subtasks extracted logic but left the new types and tests in the god file, which then grew. Each
was fixed by moving the concept to its own module (`pointer_motion.rs`, `frame_drain.rs`,
`terminal/frame_dirty.rs`) with visibility no wider than `pub(super)`. **An extraction is not done until
the extracted code has a home outside the file it came from.**

### Benchmarking pane resolution on a development laptop

The `pane_resolution_bench` IDs are nanosecond-scale: two pinned runs of the identical binary differed
by more than 15% on 12 of 39 IDs. They are good for algorithmic shape within one run and for
order-of-magnitude changes, not for a 15% gate; do not report per-ID percentage deltas.

---

## Render Performance: the Phase 0 / Task 121 outcome (Task 124.8)

### Why this entry exists at all

`Documents/DECOUPLING_FRAMEWORK.md` §2A is the source of truth for the
Phase 0 measurements, and Task 121's plan (retired; in git history) was
the work tracker. This entry exists because both are documents a future
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

### The chrome cache: final disposition

The #436 chrome cache was removed (Task 124.5), not merely disabled. `ChromeMode::Replay` skipped
_constructing_ chrome widgets, and egui resolves hit-testing against the **previous** frame's widget
set, so unbuilt widgets were uninteractable. That shipped in 0.12.0-beta.7 as lost tab clicks and
inconsistent pane-border drags, worst while a TUI ran in a pane.

- **Hover is not evidence the cache works.** A `Replay` frame replays the cached texture, highlight
  included, so hover looks live while the widget set behind it is absent.
- **Input latency appears before dropped clicks.** A click that landed on a `Replay` frame waited for
  the next `Full` frame. If a cache is ever reintroduced, watch for latency first.
- **No scheduling policy repairs it.** The frame whose mode decides whether a click can land is the
  frame _before_ an event that has not happened yet. Three code-reading hypotheses were each disproved
  by one click test; measure this subsystem, do not argue about it.
- **The sound variant** (cache tessellated output while still constructing the widgets) has a measured
  ceiling of chrome construction 32.9 us plus tessellation 10.3 us, about 8% of a ~434 us frame. It was
  not built; it would bring back egui assumption A12.

### Candidates declined, refuted or never built

Recorded so they are not re-litigated; each stands on its own measurement.

| Candidate                                       | Verdict            | Why                                                                                                                                                                                               |
| ----------------------------------------------- | ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Incremental vertex-instance build (#459 item 3) | Refuted (Task 125) | Per-row instance counts are content-dependent, so one cell shifts every later row's offset; `deco_verts` is not row-major; a maintained offset index is the bug class of issue #432.              |
| ASCII / simple-text shaping fast path (item 4)  | Dead on arrival    | ASCII does not imply "cannot ligate" (`->`, `=>`, `!=`), so the only safe gate is `ligatures == false`, which is not the default. The surviving lever was the run-level shaping cache (Task 124). |
| Compute-dispatched buffer clear (item 6)        | No freminal lever  | freminal issues a plain `glClear`; the compute clear is Mesa radeonsi's internal fast-clear. The clear is load-bearing: default-background cells emit no quad because the clear paints them.      |
| `wayland_client_handle` call frequency (item 7) | Not actionable     | freminal fetches the handle once per window; the callers are in calloop-wayland-source and winit, and the sample share was suspect.                                                               |
| Per-`CursorMoved` `Vec` allocations             | Refuted            | `layout()` + `iter_panes()` cost ~22.8 ns at one pane, ~1.4% of the residual they were blamed for.                                                                                                |
| Unbounded suppressed-pointer fallback           | Rejected           | Driven by `Context::repaint_causes()`, it depends on five egui internals, two of them present-day holes, for ~0.075% of a core over the 500 ms fallback.                                          |
| Config kill switch for pointer suppression      | Rejected           | It ships two code paths and tests neither. Scheduling behaviour is not a user toggle; the remedy for misbehaviour is fix or revert.                                                               |

A CPU meter cannot discriminate between vetoed and unvetoed pointer paths (a ~1.1% burst lasting a few
hundred ms averages to 0.1-0.2%). Use the `frame-profiling` counters.

---

## GL Call Boundary and Measurement Harnesses (Task 123)

### Why a concrete `Gl` facade

`glow::HasContext` is sealed: it is implemented only for glow's own `native::Context`, so a recording
wrapper cannot implement it. The facade is a concrete `Gl<'a>` over an internal `Real` / `Recording`
enum, never a trait object or a generic parameter; a trait would have forced generics onto about 52
`&glow::Context` parameters, and the enum keeps the `Real` arm monomorphic.

The call surface is frozen in `gl_facade/surface.rs` (`GL_CALL_SURFACE`: draw, state and upload calls
plus the timer-query and capability calls added by Task 125). A test walks the `freminal` crate and
fails if `HasContext` is mentioned outside an allowlist, in both directions, so a new raw call fails and
a stale allowlist entry fails.

Known boundary: `freminal-windowing` still calls `glow` directly (`GlState::clear`, `clear_scissored`,
the offscreen context, the GPU timing source). The guard cannot see it, and the recording log omits two
zero-byte calls (`clear_color`, `clear`) on every `Full` frame. Extending the facade needs either
exporting `Gl` from a lower crate or duplicating it (FUTURE_PLANS B.14).

### Zero cost is verified by reading assembly, not by benchmarking

The facade adds at most one pointer load in front of an indirect jump into the driver; a Criterion
benchmark could only fail to detect that. `assets/ci/check-gl-dispatch-codegen.sh` builds
`gl_facade/codegen_probe.rs` (`gl-codegen-probe`) at `--release` and fails on conditional control flow
in the emitted bodies. The realistic shape (build the facade once, make several calls) is folded by LLVM
into an alias of the direct-call version. It is a manual script, not a `#[test]`, because it is
toolchain- and architecture-specific. A default build pins `size_of::<Gl>() == size_of::<&Context>()`.

### Phase 1: the recording harness

The `Recording` arm fabricates handles, needs no GPU or display, and runs in the normal four-platform
test matrix. Its tests are gated on `gl-recording` and run in CI only because `xtask test` passes
`--all-features`; if that flag is dropped the harness stops running and nothing fails.

Reference data (80x24, bundled font, `pixels_per_point = 1.0`):

| Workload            | Calls | Draws | State changes | Uploads | Bytes     |
| ------------------- | ----- | ----- | ------------- | ------- | --------- |
| One-time init       | 261   | 0     | 30            | 2       | 96        |
| Full redraw, steady | 52    | 2     | 21            | 4       | ~200,000  |
| Cursor-only, steady | ~48   | 2     | ~19           | ~2      | ~576      |
| Toast present       | 121   | 4     | 41            | 10      | 4,658,448 |

A full rebuild costs roughly 350x the bytes of a cursor-only frame for roughly 1.08x the calls: the
cursor-only path is a bandwidth optimisation, not a call-count one. Call count is independent of grid
size; only instance count and bytes scale. The harness drives the renderer below the GUI layer, so it
measures what each path costs, not which path a frame takes.

### Phase 2: the pixel harness

- Linux-only. winit has no headless backend and the CI `test` matrix does not run under Nix, so the
  harness has its own `gl-pixel` dev shell and an advisory `.github/workflows/gl-pixel.yml` (weekly plus
  manual), not a job in `nightly.yml`, where a rasteriser change could block a release.
- `glPixelEnv` (llvmpipe) is set in the `gl-pixel` shell only, never in `default` or `ci`: in `default`
  it forced every interactively run freminal onto the CPU rasteriser.
- The tolerance is exact, decided before any golden existed. A golden is only comparable on the
  rasteriser that captured it (the `.renderer` sidecar); llvmpipe numbers are never performance
  evidence.
- `freminal-windowing/src/frame_paint_harness.rs` runs egui's head/band/tail split on an offscreen
  surface with a caller-chosen buffer age, which is what makes the real double-buffered case (age 2)
  testable.

## Damage Model (Task 124)

The rules live in the `freminal-damage-model` skill; this records why the model has this shape.

### A bool cannot cross a many-producers-to-one-consumer boundary

The dirty signal was collapsed to a `bool` at four places between the buffer and the GPU. The emulator
computed an accurate content diff and the GUI discarded it for `Arc::ptr_eq`, because a sticky
`content_changed` bool went stale across the roughly 14 snapshots published between rendered frames. A
monotonic counter compared against a baseline the consumer holds cannot be lost by a skipped frame.

### Per-row content epoch

- It lives on the merge output (`MergeCache::row_epochs`), not on `RowCacheEntry`: URL refinement is
  applied at merge time and never written back, and per-row counters over an absolute-indexed cache
  alias when the window slides.
- One globally monotonic `u64` on `Buffer`, deliberately not a content hash: a collision is an
  under-report, and an under-report is silent visual corruption. The stamp can only over-report.
- `Row::dirty` means "was written to"; the epoch means "actually changed". Full-screen TUIs rewrite
  identical bytes by idiom.
- The basis is `chars`, post-refinement `tags`, `line_width` and inline image placements, compared in
  one choke point (`RowRenderBasis::row_renders_identically`). Compare a whole `ImagePlacement`
  literally: an omitted field is corruption, a field that does not matter only costs a repaint.
- Only under-reporting is dangerous, so every "does not bump" test needs a "does bump" control.
  `debug_verify_epochs` is an always-on `debug_assertions` oracle for the carrying direction.
- Accepted over-reporter: an alternate-screen round trip restamps every row (a per-buffer merge-cache
  stash was declined because an incorrect stash under-reports).

### The two missing states

- `PaneFrameDamage::Region(Vec<PaneDamageRect>)` carries rects, not one bounding box: a bbox is lossy
  (rows 3 and 40 would present the 37 between them). An empty rect set is reported as `Full`.
- `FrameDamage::None` still runs egui's UI pass but skips tessellation, clear, paint and swap, and
  records no `DamageHistory` entry.
- `VertexRebuild::Bounded` bounds the present, not the upload. `upload_verts` writes whole buffers,
  because no instance buffer has a fixed stride per row (see "Fixed-stride relayout" under Task 125).
- Selection and hover damage are cached in screen-row space, because buffer-absolute and snapshot-row
  spaces alias when the window moves. Hover tint is baked into the background buffer.
- Search crosses the terminal/chrome split: the framebuffer is one surface, so search highlight rows
  and the floating search-bar bounds (plus egui's popup shadow margin) are composed into `FrameDamage`.

### Partial present was inert, and three defects hid behind it

`decide_partial_present` required `buffer_age() == 1`. A double-buffered GPU surface reports 2 in
steady state, so on real hardware the path was taken 0 times in 8,160 frames; llvmpipe reports 1,
which is why a first, llvmpipe-contaminated measurement concluded the gate was open.

- Windowing owns the staleness arithmetic: `DamageHistory` unions the current damage with the previous
  `age - 1` frames and publishes one bounding `PresentRegion`. The app emits rects and never learns
  `buffer_age`. `age == 0` or history deeper than retained falls back to `Full`.
- Every egui primitive's clip rect is intersected with that region (head/band/tail chrome painted
  unconditionally, and the opaque `CentralPanel` erased everything outside the band).
- The clear is scissored to the region, not skipped: skipping it makes translucent content blend
  against stale pixels (`a*fill + (1-a)*stale`).
- The app's full-draw paint arm reads the same `PresentRegion` through one helper as the partial arm.
- One region governs clip, clear, draw and present. Multi-rect scissoring is a later optimisation to
  justify by measurement.
- Result (AMD GPU, Hyprland, sustained output): `buffer_age()` is 2 on every query and 86-88% of frames
  take the partial path, with no corruption.

The clear itself is not removable: it is two zero-byte calls on a fast-clear path. Painting every cell
opaque would delete window transparency (Task 34), an alpha-zero quad writes nothing under blending, and
disabling blending breaks the image and shader layers.

### Pointer motion and PTY mouse reports are separate axes

A mouse report used to be encoded only inside the frame path, masked by `mouse_tracking_active` forcing
a repaint on every motion event.

- Reports are emitted from a synchronous, PTY-agnostic `freminal-windowing::App` hook
  (`pty_mouse_report.rs`), independent of scheduling; one physical motion produces at most one report.
- Held-button state is window-owned; per-pane state could strand a press across a focus or tab change.
- Repaint suppression is positional (content cell, gutter row, scrollbar hit rect, outside, unknown),
  not a bool: a scrollbar A -> B move is `true -> true` as a bool but still needs a repaint. Missing
  geometry resolves to unknown and forces a repaint. A drag always forces.
- Result: a pointer over `btop` gave 920 events/s, 99.89% suppressed (~4.9 fps, btop's own redraws).

### Shaping: run-level cache

The line cache is keyed by line index, so a one-line scroll missed every line although 23 of 24 rows
were byte-identical. `ShapingCache` also holds a content-addressed run cache keyed by exact
`(FaceId, LigatureShaping, run text, per-character widths)` (exact equality, not a hash; widths joined
the key because equal text with different grapheme widths aliased). Two generations, rotating only on
miss-bearing calls; templates are shaped at column 0 and rebased on a hit. A one-line scroll went
975.7 -> 198.4 us.

### GPU buffer orphaning

`upload_verts` orphans small payloads only above `SMALL_UPLOAD_ORPHAN_THRESHOLD_BYTES` (4 KiB, a bound,
not a measured optimum). `glBufferSubData` is synchronous with respect to prior GL commands, so skipping
the orphan cannot tear; issue #432's corruption was CPU-side offset bookkeeping.

## Performance Parity and the `RowStore` (Task 125)

### Parity verdicts

Measured against WezTerm and Ghostty on a Radeon RX 7900 XTX under Hyprland, 10 workloads; the protocol
is in `PROFILING.md`. A gap is material only if the bootstrap 95% CI of the paired task-clock delta
excludes zero and the median delta is at least 0.5 ms per wall-second.

| Candidate                             | Verdict      |
| ------------------------------------- | ------------ |
| Idle / cursor-blink / retained chrome | refuted      |
| Fixed-stride per-row uploads          | refuted      |
| Incremental CPU row construction      | refuted      |
| Persistent GPU buffers                | refuted      |
| Scheduling / wakeups                  | refuted      |
| Presentation / compositor             | refuted      |
| Scroll-aware row reuse                | refuted      |
| Scrollback eviction                   | confirmed    |
| Pointer workload                      | inconclusive |

`sustained-output` was 428.9 ms/s against 19.9 (WezTerm) and 37.1 (Ghostty): 96.5% of samples were in
`memmove` under `Vec::drain` in `Buffer::enforce_scrollback_limit`. Once scrollback is full, every line
feed shifted ~20,000 entries.

### Fixed-stride relayout (refuted)

Do not revive it without new evidence. If it is revisited: fixed stride beats a maintained offset table
(the table must be invalidated on every per-row content, format and blink-visibility change); padded
slots must emit no fragments, because `DefaultBackground` means "leave pixels untouched"; `deco_verts`
keeps its cursor-quad-last invariant, which makes the cursor-only patch offset stable.

### Scrollback eviction and the `RowStore`

- Rows were addressed by physical position in three manually synchronised `Vec`s (`rows`, `row_cache`,
  `row_block_map`), so front eviction shifted storage and every holder of a row index had to be
  rewritten or silently drifted (selection, DECSC, kitty placements). Batched eviction was rejected: it
  stays O(retained), loosens the exact limit and keeps the drift bugs.
- `RowStore` keeps the three `Vec`s with a shared moving `head` plus a `base: RowNumber`. Eviction
  replaces slots with placeholders and advances `head` and `base`; it compacts at `max(live / 2, 64)`
  dead slots. `VecDeque` was rejected: flatten needs single contiguous `&mut` slices.
- `RowNumber(u64)` is never reused. RIS and ED 3 advance `base` rather than resetting it; the alternate
  screen's namespace starts at `1 << 63`. Width-changing reflow installs rows at the old `next_number()`
  and returns a `ReflowRemap` for prompts, blocks, saved cursor, images and kitty placements; anything
  not remapped falls below `base` and is detectably invalid instead of aliasing.
- Cursor position and `scroll_offset` stay screen-relative; prompts, block rows, image origins, kitty
  placements and GUI selection/search anchors are logical.
- Search staleness is keyed on `BufferExtent { row_base, total_rows }`, because a sum does not change
  across ED 3.
- Result: a 200-line burst at capacity fell from 4.39 ms to ~118 us; eviction no longer scales with
  retained rows. `sustained-output` became 8.9 ms/s versus 19.2 and 37.5.

### Benchmark-validity cautions

- `sustained-output` repeats `seq 1 200`, so the screen is identical after each burst and row epochs
  make Freminal paint almost nothing. `sustained-output-varying` (`seq n n+199`) removes that effect;
  Freminal still used the least CPU and GPU of the three.
- An OSC 133 workload only exercises blocks if the marks carry freminal's `freminal=1;fid=` parameters.

---

## Desktop Notification Backends (Task 150)

### What `freminal-notify` owns, and what stays a caller

"Show this notification on the operating system" has one owner: the
`freminal-notify` crate (`freminal-notify/src/`). It contains the OS backends
(the Linux/BSD D-Bus service in `linux/`, the macOS/Windows plain worker and the
macOS/Windows OSC 99 untracked worker), the icon pipeline (`icons/`: decode,
raster, temp files, lookup), `presentation.rs`, body-markup escaping
(`markup.rs`), the daemon capability types and the backend interface types.

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
backend. `plain_sink` and `with_backends` exist on every platform.
`ensure_started`, `is_started` and `drain_events` exist on Linux/BSD only: only
there is a service to start or events to drain. The GUI's two `ensure_started`
call sites (one per frame, one after a settings apply) and its per-frame
event-delivery call are each gated with one `#[cfg]` that says why; nothing else
in the GUI names a platform.

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

| Worker                    | Thread                    | Lifecycle                                                                                                                                                                                                                                  |
| ------------------------- | ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Plain (`plain_worker.rs`) | `freminal-notify`         | Process-wide static. Started on first use, restarted at most every 5 s if it exits, never shut down. Queue of 16.                                                                                                                          |
| OSC 99 (`untracked.rs`)   | `freminal-notify-deliver` | Owned by the untracked backend, spawned on the first notification. If it has exited, the notification that finds it gone is dropped and the next one spawns a new worker (no rate limit). Joined (bounded at 500 ms) on drop. Queue of 16. |

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

## TERM Strategy (Task 151)

### The reversed decision

Task 12 decided that freminal would keep `TERM=xterm-256color` (as WezTerm and Alacritty do)
and advertise anything extra through XTGETTCAP. Task 92 (a terminfo self-install subcommand)
was dropped on the strength of that decision. Task 151 reverses it: children get
`TERM=xterm-freminal` by default. The reasons:

- **The entry was never read.** `res/terminfo.tar` was embedded and extracted for every PTY,
  but the child's `TERM` named an entry that tar did not contain, so nothing used it.
- **XTGETTCAP is a poor substitute.** Only programs that send the query see its answers, and
  it is a runtime round trip per program. Terminfo is what ncurses, `tput`, tmux, vim and
  readline already read.
- **Programs should know which terminal they are in.** Capabilities such as `Smulx`,
  `Setulc`, `Sync` and `Tc` are declared once, accurately, instead of being inherited from
  an xterm that does not behave like freminal.
- **The `xterm-` prefix keeps compatibility.** Programs that match `xterm*` keep working, as
  they do for `xterm-kitty` and `xterm-ghostty`.

The price is the ssh case: a host that does not have the entry reports `unknown terminal type`.
The README's "Terminfo" section gives the fixes (below).

### The entry is checked, not trusted

A wrong capability is worse than a missing one once programs trust it, so the entry
(`res/freminal.ti`) is held to what freminal does:

- It was audited capability by capability against the parser, handler and input encoder
  (Task 151.1). Capabilities freminal does not implement are not declared: `invis` and the
  SGR 8 part of `sgr` (conceal is parsed and ignored), and `km` (Alt sends no 8th bit).
  `acsc` is `xterm-256color`'s value.
- A test parses `res/freminal.ti` and compares it with `lookup_termcap`, the XTGETTCAP
  table. Every name XTGETTCAP answers must match the entry, except a short list of
  XTGETTCAP-only names, and every entry capability is either answered identically or sits in
  an explicit terminfo-only list with a reason. A new arm on either side fails the test. Every
  `k*` value is also compared with the bytes the encoder sends, except `kent`, `kcbt` and
  `kmous`, which are declared but not sent and sit in an explicit `DECLARED_NOT_SENT` list
  with the reason for each.
- XTGETTCAP `TN` reports the pane's actual `TERM`, so the two cannot disagree. `Se` is
  `CSI 0 SP q` (DECSCUSR 0 restores the configured cursor style).
- An integration test spawns a real child through the production spawn path and checks that
  `infocmp -x xterm-freminal` and `tput` resolve the entry from the directory freminal put on
  `TERMINFO`, with a stale `~/.terminfo` copy present in the child's `HOME`, and that
  `TERM=xterm-256color` still resolves from the system database.
- The committed tree under `res/terminfo/` holds the bytes the build host's `tic` produced,
  and they can differ between ncurses versions. The tests therefore check that the tree and
  `res/terminfo.tar` agree with each other and that `res/freminal.ti` compiles, on the same
  host, to the committed entry; they do not pin one byte image across hosts.

### Which value the child gets

The effective `TERM` is resolved once per pane at spawn (`TermName::resolve`), in this order:

1. A `TERM` in the pane's `env` table, or the layout's (taken as given, not validated). An
   empty value is ignored. On Windows the key is matched case-insensitively (`term` names
   `TERM`, as the environment does there); elsewhere only `TERM` itself matches.
2. `[shell] term` in `config.toml`.
3. The platform default: `xterm-freminal`, and `xterm-256color` on Windows.

That single value is used for the child's environment, for XTGETTCAP `TN`, and for the FREC
recording header, with one limitation: the header is per recording, not per pane, so it
records the `[shell] term` resolution (steps 2 and 3) and a `TERM` set in one pane's `env` is
not reflected there. `FREC_FORMAT.md` says the same.

A `[shell] term` value is passed through unchecked apart from a syntax check (non-empty, at
most 128 bytes, no whitespace, control characters or `/`). An invalid value is dropped with a
config warning and the platform default is used. freminal does not check that the named entry
exists: it cannot see the remote end of an ssh session. Changes apply to new panes only.

**Windows keeps `xterm-256color`.** There is no terminfo distribution path on Windows, and
MSYS2 and Cygwin programs running under ConPTY read their own terminfo databases, which do not
have `xterm-freminal`. `TERM` is still set, and `[shell] term` is honoured on every platform.

### How terminfo reaches programs

The entry is shipped in two directory layouts: the letter layout (`x/xterm-freminal`,
`f/freminal`) that most ncurses uses, and the hex layout (`78/…`, `66/…`) that Apple's ncurses
uses. The compiled tree is committed under `res/terminfo/`, together with the embedded
`res/terminfo.tar`, and `cargo xtask terminfo` regenerates both from `res/freminal.ti`.
`build.rs` does not write them: a build script may write only to `OUT_DIR`, and a build that
rewrote the tree could embed a tar that lags the tree. The stdin-compile test in
`terminfo_tree.rs` fails when `res/freminal.ti` changes without a regeneration. Packaging
copies the committed files, so no package build needs `tic`.

The child's `TERMINFO` is `<dir>`, where `<dir>` is, in order of preference:

1. **macOS, inside a `.app` bundle:** `Contents/Resources/terminfo`, when that directory exists.
   The release workflows and the Nix Darwin package copy the tree into the bundle (before
   `codesign --deep` in the workflows). Nothing is extracted. A `.app` built locally with
   `cargo bundle` has no such directory.
2. **Otherwise (Linux, a raw macOS binary):** a per-pane temporary directory that the embedded
   tar is extracted into, deleted when the pane closes.

`TERMINFO` is not an exclusive directory, and it outranks the user's home. ncurses searches
`$TERMINFO`, then `$HOME/.terminfo`, then `$TERMINFO_DIRS` (an empty element means the system
database), then the compiled-in default (`db_iterator.c`; the same order is in Apple's
ncurses and in ncurses 5.7). The first directory that holds the requested entry wins and a
miss falls through to the next, so the tree here, which holds only `xterm-freminal` and
`freminal`, does not hide the system entries and every other `TERM` still resolves. kitty and
Ghostty set `TERMINFO` to their bundled directory the same way. The inherited `TERMINFO_DIRS`
is left as it is.

On Windows nothing is extracted and `TERMINFO` is not touched. On the other platforms
`TERMINFO` is set to the directory above. An inherited non-empty `TERMINFO` (a private database
the user exported for a custom `TERM`) is not discarded: it is moved to the front of the child's
`TERMINFO_DIRS`, followed by the inherited `TERMINFO_DIRS`, or by an empty element (which
ncurses reads as the system database) when that is unset. Without an inherited `TERMINFO` the
inherited `TERMINFO_DIRS` is left alone. Because the user's directory moves into
`TERMINFO_DIRS`, it is searched after `~/.terminfo`, so a same-named entry in `~/.terminfo`
shadows it (it ranked first while it was `TERMINFO`).

`FREMINAL_TERMINFO` is set to the same directory as `TERMINFO`. It is internal, not a setting,
and exists for one reason: a freminal started inside a pane inherits the parent pane's
`TERMINFO`, which is not the user's. An inherited `TERMINFO` that equals the pane's own
directory, or equals the inherited `FREMINAL_TERMINFO` (set by a parent freminal pane), is
therefore not moved into `TERMINFO_DIRS`; otherwise each nesting level would add another
ancestor's temporary directory to the list. A `TERMINFO` the user exports inside a pane
differs from `FREMINAL_TERMINFO` and is kept. Both variables are set even when
`set_term_program` is off.

Installed copies:

- The `.deb` and `.rpm` install `/usr/share/terminfo/{x,f}/…` (the `.deb` is repacked by
  `assets/ci/fix-linux-icon-metadata.sh`).
- The flake's `packages.<system>.terminfo` contains only `share/terminfo` (all four layouts),
  with no GUI dependencies, for servers that do not install freminal. The main package
  installs the same tree.
- The NixOS module option `programs.freminal.terminfo.enable` adds that package to
  `environment.systemPackages`. The home-manager module stays for desktops.
- The AppImage ships nothing extra and relies on per-pane extraction. Users who need the
  entry system-wide install one of the packages above or use the fallback commands.

### Known limits

- **`sudo` drops `TERMINFO`** (its `env_reset`), as it does for kitty and Ghostty, so `sudo`
  programs only find a system-installed copy. Separately, ncurses ignores `TERMINFO` and
  `TERMINFO_DIRS` in setuid and setgid programs; programs run as root read them unless ncurses
  was configured with `--disable-root-environ`. The `[shell_integration] sudo_preserve_terminfo` toggle
  (default on, maintainer decision 2026-11-19; Task 151.12) wraps `sudo` in the bundled bash,
  zsh and fish integrations as `sudo --preserve-env=TERMINFO` (except an edit, `-e` /
  `--edit`, which sudo refuses to combine with `--preserve-env`). The wrappers are freminal's
  own implementation, written clean-room from sudo(8)'s option syntax (Task 151.C5; the first
  version copied third-party GPL code and was replaced). An argument scan decides whether the
  call is an edit: it reads the arguments left to right up to the command word, skips option
  operands (`-u root`, `--user root`, `--user=root`, a value letter ending a grouped `-ku`
  word, or taking the rest of its word as in `-uroot`; a standalone `-h` also takes the next
  word as the host unless it starts with `-` or is a `NAME=value` assignment) and `NAME=value` words, stops at `--`,
  and treats `e` / `--edit` as an edit, so `sudo -u root -e file` is recognised as an edit and
  left unchanged. The feature is on by default whenever shell integration is active; a
  sudoers policy that forbids preserving the environment makes `sudo` refuse the command, and
  the user then sets the toggle to `false`. Freminal tells the scripts the feature is on
  through `FREMINAL_SHELL_FEATURES=sudo` (a comma-separated list matched by whole item), set
  at spawn only when the toggle is on and shell integration is injected, and removed otherwise
  so a value inherited from a parent freminal pane cannot turn it on.
- **Other users and other environments cannot read the pane's directory.** The per-pane
  directory is a temporary directory created mode 0700, so `sudo -u otheruser`, `su`, `doas`
  and containers (`docker exec -it` passes `TERM` but not the entry) cannot read it, and
  those tools typically reset the environment as well. They need a system-wide copy or
  `[shell] term`. ssh is not the only such case.
- **`tic -x` inside a pane installs into the pane's directory.** `tic` writes to `$TERMINFO`
  first, and in a pane that is the pane's temporary (or signed-bundle) directory, so the
  documented local installs name their target: `tic -x -o ~/.terminfo -` per user and
  `sudo env -u TERMINFO tic -x -` system-wide (the sudo wrapper keeps `TERMINFO`, so it has
  to be unset for a system install). `ssh HOST -- tic -x -` is unaffected because ssh does not
  forward `TERMINFO`.
- **Detached processes outlive the pane's temporary directory.** A tmux server started from a
  pane keeps running after the pane closes and deletes the per-pane directory, so it needs a
  system-wide or per-user copy.
- **A per-user `~/.terminfo` copy no longer shadows the pane's entry**, because `TERMINFO` is
  searched first, so a copy installed there by the `tic -x -o ~/.terminfo` fallback below cannot go stale
  inside a pane after freminal is upgraded. It is still the copy that other terminals and ssh
  sessions on the same account use, since they do not set `TERMINFO`.
- **Remote hosts have no entry** unless one of the fallbacks is used. Copying it automatically
  belongs to SSH integration (Task 86); there is deliberately no install subcommand
  (Task 92 stays dropped).

### The fallback that always works

The README documents installing the entry on any host, with or without freminal present, as
Ghostty's terminfo help page does:

- From inside a freminal pane (every build, the AppImage included, because each pane has the
  entry on `TERMINFO`): `infocmp -x xterm-freminal | ssh YOUR-SERVER -- tic -x -`.
- From anywhere, from the source file, piped through `ssh YOUR-SERVER -- tic -x -`, or on the
  local machine with an explicit target (see the limit above):
  `curl -fsSL https://raw.githubusercontent.com/fredsystems/freminal/main/res/freminal.ti | tic -x -o ~/.terminfo -`
  per user, or `... | sudo env -u TERMINFO tic -x -` system-wide. `res/freminal.ti` is therefore
  a public URL and must not be moved or renamed; its header comment says so.
- A version-pinned copy: every release and nightly carries `freminal.ti` as an asset.
  `releases/latest/download/…` is not used, because every release so far is a prerelease and
  GitHub's `latest` skips prereleases.

Outside freminal, `tic -x` run without root installs to `~/.terminfo`, which needs no
privileges; inside a pane it does not, which is why the local forms above use `-o`. Setting
`[shell] term = "xterm-256color"` is the other way out, at the cost of the extra capabilities.

---

## Workspace Lint Configuration (issue 552)

### Where the lints live

The clippy lint set is declared once, in `[workspace.lints.clippy]` in the root
`Cargo.toml`, and inherited with `[lints] workspace = true` by every first-party
crate: `freminal`, `freminal-buffer`, `freminal-common`, `freminal-notify`,
`freminal-terminal-emulator`, `freminal-windowing`, `xtask` and the vendored
`portable-pty`. The crate-root
`#![deny(...)]` blocks that used to repeat the set in every `lib.rs` / `main.rs`
are gone. `xtask` previously lacked the `cargo`, `unwrap_used` and `expect_used`
lints and now gets the full set.

The vendored `portable-pty` crate was originally left out, as third-party code
that could then track upstream without lint churn. It opted in on 2026-11-21
(maintainer decision): it runs in every pane, and the lint pass found real
Windows defects -- an unaligned `[u16]` view of registry bytes (undefined
behaviour), an under-aligned buffer handed to Win32 as a
`PROC_THREAD_ATTRIBUTE_LIST`, and a console size cast that wrapped silently. Freminal's copy has diverged from upstream
(`portable-pty/vendored.md` lists the changes), so a re-sync is a manual merge
in any case.

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
egui 0.37 / winit 0.30 windowing stack (objc2 0.5/0.6, smithay-client-toolkit
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
