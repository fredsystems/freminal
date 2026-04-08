# PLAN_VERSION_040.md — v0.4.0 "Search & Protocol"

## Goal

Deliver search and advanced selection capabilities, complete the terminal protocol coverage
(SGR underline styles, BCE, DECDWL/DECDHL, KKP flags 2/4/16), and add password detection
and adaptive theming.

---

## Task Summary

| #   | Feature                       | Scope        | Status   |
| --- | ----------------------------- | ------------ | -------- |
| 45  | Search in Scrollback          | Medium-Large | Pending  |
| 46  | Rectangular / Block Selection | Medium       | Pending  |
| 47  | SGR Underline Styles          | Medium       | Pending  |
| 48  | BCE (Background Color Erase)  | Medium       | Complete |
| 49  | DECDWL / DECDHL Rendering     | Medium       | Pending  |
| 50  | KKP Flags 2/4/16              | Medium       | Pending  |
| 51  | Password Input Detection      | Small        | Pending  |
| 52  | Adaptive Light/Dark Theming   | Small        | Pending  |

---

## Task 45 — Search in Scrollback

### 45 Overview

Implement Ctrl+Shift+F search with match highlighting, scroll-to-match navigation, and the
ability to jump between previous commands using OSC command markers.

### 45 Design

**Search UI:** A search overlay bar at the top or bottom of the terminal area (not a modal).
Contains: text input field, "Next" / "Prev" buttons, match count indicator, "Regex" toggle,
close button (or Escape to close).

**Buffer Search:** The search operates on the full scrollback + visible buffer text. Matching
is performed on the `TChar` text content extracted from the snapshot or requested via the PTY
thread (for access to the full buffer).

**Match Highlighting:** Matched ranges are highlighted with a distinct background color (e.g.,
bright yellow with dark text). The current match has a different highlight from other matches.

**Command Jump:** Since OSC command data is already captured (shell integration / OSC 133),
add key bindings to jump backward/forward to the previous/next command prompt boundary in
scrollback. This is independent of text search — it navigates between shell command outputs.

### 45 Subtasks

1. **45.1 — Search overlay UI**
   Create `freminal/src/gui/search.rs` with the search bar widget. Render as an overlay on
   top of the terminal area. Handle text input, navigation buttons, Escape to close.

2. **45.2 — Buffer text search**
   Implement substring search across the terminal buffer. The search must work on the full
   scrollback. Options: send a search query to the PTY thread (which has buffer access) and
   receive match positions back, or search the snapshot's text data directly.

3. **45.3 — Regex search toggle**
   When the "Regex" toggle is active, compile the search string as a regex. Use the `regex`
   crate. Handle invalid regex gracefully (show error in the search bar, no panic).

4. **45.4 — Match highlighting in renderer**
   Thread match positions through to the vertex builder. Matched cells get a highlight
   background color. The "current" match gets a distinct color.

5. **45.5 — Scroll-to-match navigation**
   "Next" scrolls forward to the next match, "Prev" scrolls backward. Wrap around when
   reaching the end/start. Update `scroll_offset` in `ViewState` to bring the match into view.

6. **45.6 — Command jump navigation**
   Parse OSC 133 markers (command prompt boundaries) from the buffer. Add keybindings:
   Ctrl+Shift+Up (previous command), Ctrl+Shift+Down (next command). Scroll to the command
   boundary.

7. **45.7 — Keybinding integration**
   Add `KeyAction::OpenSearch`, `KeyAction::SearchNext`, `KeyAction::SearchPrev`,
   `KeyAction::PrevCommand`, `KeyAction::NextCommand` to the keybinding system (Task 37).
   Default bindings: Ctrl+Shift+F (open), Enter (next), Shift+Enter (prev),
   Ctrl+Shift+Up/Down (command jump).

8. **45.8 — Tests**
   Unit tests: substring search, regex search, match indexing, command boundary detection.
   Integration: search + scroll + highlight end-to-end.

### 45 Primary Files

- `freminal/src/gui/search.rs` (new)
- `freminal/src/gui/terminal/widget.rs` (overlay rendering)
- `freminal/src/gui/renderer/vertex.rs` (match highlighting)
- `freminal-common/src/keybindings.rs` (new actions)

---

## Task 46 — Rectangular / Block Selection

### 46 Overview

Implement Alt+drag rectangular (block) selection. Currently `SelectionState` only supports
linear (stream) selection. Rectangular selection selects an independent column range per row.

### 46 Design

**Selection Mode:** Add a `SelectionMode` enum to `view_state.rs`:

```rust
pub enum SelectionMode {
    /// Normal stream selection (current behavior).
    Stream,
    /// Rectangular / block / column selection.
    Block,
}
```

Add `mode: SelectionMode` to `SelectionState`.

**Activation:** When Alt is held during a mouse press, set `mode = Block`. Without Alt,
use `Stream` (current behavior).

**Rendering:** In `Block` mode, the selection highlight is a rectangle defined by
`(min_col, min_row)` to `(max_col, max_row)` — every row in the range gets the same column
span highlighted, regardless of line content.

**Copy Behavior:** In `Block` mode, each row's selected columns are extracted independently
and joined with newlines. This produces columnar text (useful for tabular output, log files).

### 46 Subtasks

1. **46.1 — `SelectionMode` enum and state changes**
   Add `SelectionMode` to `SelectionState`. Default to `Stream`. Set to `Block` when Alt is
   held on mouse press.

2. **46.2 — Block selection rendering**
   Modify the selection highlight rendering in the vertex builder to support rectangular
   highlighting: all rows in the range get the same column span.

3. **46.3 — Block selection copy**
   Modify the text extraction for clipboard copy to handle block mode: extract the column
   range from each row independently.

4. **46.4 — Tests**
   Unit tests: block selection coordinates, copy produces columnar text, mode transitions.

### 46 Primary Files

- `freminal/src/gui/view_state.rs` (`SelectionState`, `SelectionMode`)
- `freminal/src/gui/terminal/widget.rs` (Alt detection on mouse press)
- `freminal/src/gui/renderer/vertex.rs` (block highlight rendering)

---

## Task 47 — SGR Underline Styles

### 47 Overview

Render distinct underline styles: plain (4:1), double (4:2), curly (4:3), dotted (4:4),
dashed (4:5). Currently all map to a single solid underline. `Smulx` is advertised via
XTGETTCAP, so applications believe these styles are supported.

### 47 Design

**Enum Extension:** Replace the single `FontDecorations::Underline` variant with an
`UnderlineStyle` sub-enum or extend `FontDecorations` with style-specific variants:

```rust
pub enum UnderlineStyle {
    Single,   // SGR 4 or 4:1
    Double,   // SGR 4:2
    Curly,    // SGR 4:3
    Dotted,   // SGR 4:4
    Dashed,   // SGR 4:5
}
```

`FontDecorationFlags` needs to encode the underline style. Since underline styles are
mutually exclusive, use a 3-bit field within the existing `u8` bitfield (bits 1-3, giving
0 = none, 1-5 = styles).

**SGR Parsing:** `apply_sgr()` in `freminal-buffer/src/terminal_handler/sgr.rs` already
handles SGR 4 (underline on) and SGR 24 (underline off). Extend to handle the colon-separated
subparameter form `4:N` where N indicates the style.

**Renderer:** In `renderer/vertex.rs`, the underline is currently drawn as a solid rectangle
below the cell. Replace with style-specific drawing:

- **Single:** Solid horizontal line (current behavior)
- **Double:** Two parallel horizontal lines
- **Curly:** Sine wave approximation using multiple small quads or a textured strip
- **Dotted:** Series of small squares with gaps
- **Dashed:** Longer rectangles with gaps

**Underline Color:** SGR 58 (set underline color) and SGR 59 (reset) are already parsed
into `CursorState.underline_color`. Verify this color is threaded through `FormatTag` to the
renderer and used for all underline styles.

### 47 Subtasks

1. **47.1 — `UnderlineStyle` enum and `FontDecorationFlags` encoding**
   Add `UnderlineStyle` to `freminal-common/src/buffer_states/fonts.rs`. Modify
   `FontDecorationFlags` to store the style. Update all callsites.

2. **47.2 — SGR 4:N parsing**
   Update `apply_sgr()` to parse the colon-separated subparameter `4:N` and set the
   appropriate underline style. Handle edge cases: `4` alone = `Single`, `4:0` = off,
   `4:1` through `4:5`.

3. **47.3 — Thread underline style through FormatTag and snapshot**
   Ensure `FormatTag` carries the `UnderlineStyle`. The snapshot builder and the vertex
   builder must have access to it.

4. **47.4 — Renderer: distinct underline drawing**
   Implement each underline style as distinct geometry in the vertex builder. Curly underline
   requires a sine-wave approximation (4-8 quad segments per cell width).

5. **47.5 — Underline color verification**
   Verify that `underline_color` from `CursorState` is correctly threaded through `FormatTag`
   → snapshot → renderer and used as the color for all underline styles. Fix if not.

6. **47.6 — Tests**
   Unit tests: SGR 4:N parsing, `FontDecorationFlags` encoding/decoding, format tag
   propagation. Visual verification: render each style and confirm distinct appearance.

### 47 Primary Files

- `freminal-common/src/buffer_states/fonts.rs` (`FontDecorations`, `FontDecorationFlags`)
- `freminal-buffer/src/terminal_handler/sgr.rs` (`apply_sgr`)
- `freminal/src/gui/renderer/vertex.rs` (underline drawing)
- `freminal-common/src/cursor.rs` (underline color threading)

---

## Task 48 — BCE (Background Color Erase)

### 48 Overview

Implement Background Color Erase: erase operations (ED, EL, ECH, etc.) should fill erased
cells with the current SGR background color, not the default background.

### 48 Design

Currently, erase operations in `Buffer` create cells with the default format. With BCE, the
current SGR background color from `CursorState` must be passed to and applied by all erase
operations.

**Affected Operations:**

- `ED` (Erase Display) — `clear()`, `clear_from()`, `clear_to()` in `Buffer`
- `EL` (Erase in Line) — `clear_line_from()`, `clear_line_to()`, `clear_line()` in `Buffer`
- `ECH` (Erase Characters)
- `ICH/DCH` (Insert/Delete Characters) — shifted cells may need BCE fill
- Scroll operations that create new blank lines

Each erase method needs to accept the current background color (or a `FormatTag` / `CursorState`
reference) and apply it to newly created cells.

**Terminfo:** After implementing BCE, update the terminfo source (`freminal.ti` if it exists)
to advertise the `bce` capability.

### 48 Subtasks

1. **48.1 — Pass current format to erase operations** ✅
   Modify `Buffer` erase methods to accept a format/background parameter. Update all callsites
   in `terminal_handler/mod.rs` to pass the current `CursorState`'s background color.

2. **48.2 — Apply background to erased cells** ✅
   In each erase method, newly created or cleared cells receive the passed background color
   instead of `DefaultBackground`.

3. **48.3 — Scroll fill with BCE** ✅
   When scroll operations create new blank lines, those lines should also receive the current
   background color.

4. **48.4 — Terminfo update** ✅
   Advertise `bce` capability in the terminfo source and XTGETTCAP responses.
   Note: terminfo already had `bce`; added `ut` to XTGETTCAP `lookup_termcap()`.

5. **48.5 — Tests** ✅
   Unit tests: erase with custom background, scroll with custom background, verify cells
   carry the correct color. Integration: `printf '\e[41m\e[2J'` fills screen with red.
   12 new tests added (6 row-level, 6 buffer-level).

### 48 Primary Files

- `freminal-buffer/src/buffer/mod.rs` (erase methods)
- `freminal-buffer/src/row.rs` (cell creation)
- `freminal-buffer/src/terminal_handler/mod.rs` (callsites)
- `freminal-terminal-emulator/src/ansi_components/` (terminfo)

---

## Task 49 — DECDWL / DECDHL Rendering

### 49 Overview

Render double-width lines (DECDWL, ESC # 6) and double-height lines (DECDHL top/bottom,
ESC # 3 / # 4). Currently parsed but ignored at `terminal_handler/mod.rs:2014`:
`"DoubleWidthLine not yet implemented (ignored)"`.

### 49 Design

**Row-Level Attribute:** Add a `LineWidth` enum to the row model:

```rust
pub enum LineWidth {
    /// Normal single-width, single-height line.
    Normal,
    /// Double-width line (DECDWL, ESC # 6). Each character occupies two columns.
    DoubleWidth,
    /// Double-height line, top half (DECDHL, ESC # 3).
    DoubleHeightTop,
    /// Double-height line, bottom half (DECDHL, ESC # 4).
    DoubleHeightBottom,
}
```

Add `line_width: LineWidth` to the `Row` struct (or equivalent).

**Renderer:** In the vertex builder / GPU renderer:

- **DECDWL:** Each glyph is scaled 2x horizontally. The row effectively contains half as
  many visible characters. Cursor column is in logical (half-width) coordinates.
- **DECDHL top:** Glyph is scaled 2x both horizontally and vertically, but only the top
  half is visible on this row.
- **DECDHL bottom:** Same glyph scaled 2x, but only the bottom half is visible.

**Cursor Positioning:** When the cursor is on a double-width line, the effective column count
is halved. Cursor movement and character insertion must account for this.

**Terminal Handler:** When `TerminalOutput::DoubleWidthLine` is processed, set the current
row's `line_width` attribute.

### 49 Subtasks

1. **49.1 — `LineWidth` enum and row attribute**
   Add `LineWidth` to `freminal-buffer/src/row.rs`. Default to `Normal`. Add setter and getter.

2. **49.2 — Terminal handler: set line width**
   In the `DoubleWidthLine` handler, set the current row's `line_width`. Also handle ESC # 3
   (top half) and ESC # 4 (bottom half) — check if these are already parsed; if not, add
   parsing in `standard.rs`.

3. **49.3 — Snapshot: thread line width**
   Ensure `line_width` is included in the snapshot's row data so the renderer has access.

4. **49.4 — Renderer: double-width glyph scaling**
   Modify the vertex builder to detect `LineWidth::DoubleWidth` rows and scale glyph quads
   2x horizontally.

5. **49.5 — Renderer: double-height glyph clipping**
   For `DoubleHeightTop` and `DoubleHeightBottom`, scale glyphs 2x in both dimensions and
   clip to the appropriate half.

6. **49.6 — Cursor positioning adjustments**
   When the cursor is on a double-width/height line, adjust column calculations. The effective
   column count is halved.

7. **49.7 — Tests**
   Unit tests: line width attribute setting, cursor positioning on double-width lines.
   Integration: vttest double-width/height test screens render correctly.

### 49 Primary Files

- `freminal-buffer/src/row.rs` (`LineWidth`)
- `freminal-buffer/src/terminal_handler/mod.rs` (handler)
- `freminal-terminal-emulator/src/ansi_components/standard.rs` (parser for ESC # 3/4/5/6)
- `freminal/src/gui/renderer/vertex.rs` (glyph scaling)
- `freminal/src/gui/renderer/gpu.rs` (shader adjustments if needed)

---

## Task 50 — KKP Flags 2/4/16

### 50 Overview

Complete the Kitty Keyboard Protocol by implementing output for flags 2 (report event types),
4 (report alternate keys), and 16 (report associated text). Currently these flags are parsed
and stored on the KKP stack but produce no output.

The comment in `input.rs:108` notes: "Flags 2, 4, 16 are parsed and stored on the KKP stack
but do not yet produce additional output fields."

### 50 Design

**Flag 2 — Report Event Types:** Add `:event-type` field to CSI u sequences:

- `1` = key press (default, omitted when flag 2 is not active)
- `2` = key repeat
- `3` = key release

Requires detecting press vs. repeat vs. release from egui's key events. egui provides
`InputState::events` with `Event::Key { pressed, repeat, .. }`.

**Flag 4 — Report Alternate Keys:** Add `:shifted-key` and `:base-layout-key` fields.

- `shifted-key`: the key that would be produced with Shift held (e.g., Shift+1 = `!`)
- `base-layout-key`: the key on the standard US QWERTY layout for the same physical key

This requires access to keyboard layout information, which egui does not directly provide.
Implementation options: best-effort using egui's `Key` enum mappings, or platform-specific
APIs. May need to be partial (US QWERTY only) with a note that full layout support requires
future work.

**Flag 16 — Report Associated Text:** Add `associated_text` field containing the text that
the key would produce. For letter keys, this is the character itself. For function keys and
modifiers, this is empty.

### 50 Subtasks

1. **50.1 — Thread key event metadata through `TerminalInput`**
   Extend `TerminalInput` (or create a new `KeyEvent` struct) to carry: press/repeat/release
   state, shifted key value, base layout key value, and associated text.

2. **50.2 — Detect press/repeat/release from egui events**
   Map egui's `Event::Key { pressed, repeat, .. }` to the three event types. Release events
   require tracking key-down state to detect the transition.

3. **50.3 — Implement flag 2 output (event types)**
   When flag 2 is active, append `:event-type` to CSI u sequences in `to_payload_kkp()`.

4. **50.4 — Implement flag 4 output (alternate keys)**
   When flag 4 is active, append `:shifted-key:base-layout-key` fields. Best-effort: cover
   US QWERTY layout mappings. Document limitations for non-QWERTY layouts.

5. **50.5 — Implement flag 16 output (associated text)**
   When flag 16 is active, append `;associated_text=` to CSI u sequences.

6. **50.6 — Tests**
   Unit tests: CSI u output format for each flag combination. Integration: test with a KKP
   client (e.g., `kitten show-key` from Kitty) if available.

### 50 Primary Files

- `freminal-common/src/input.rs` (`TerminalInput` extension)
- `freminal/src/gui/terminal/input.rs` (event type detection, payload generation)

---

## Task 51 — Password Input Detection

### 51 Overview

Detect when the terminal is likely prompting for a password (TTY echo-off mode) and provide
visual feedback. Optionally suppress scrollback recording during password entry.

### 51 Design

**Detection:** The terminal already tracks echo mode — when an application sets the TTY to
no-echo mode (e.g., `stty -echo` or via termios), this is detectable from the PTY attributes.
Alternatively, monitor for common password prompt patterns (heuristic, less reliable).

The more reliable approach: check the PTY's terminal attributes for echo-off. The PTY
consumer thread can query this.

**Visual Indicator:** When echo-off is detected:

- Show a small lock icon or "Password mode" indicator in the status area (tab bar or title bar).
- Optionally use a different cursor color or style.

**Scrollback Suppression:** When echo-off is active, optionally suppress scrollback recording
so password characters (even masked) are not stored in history.

**Config:**

```toml
[security]
# Show a visual indicator when password input is detected (TTY echo off).
password_indicator = true
# Suppress scrollback recording during password input.
# suppress_password_scrollback = false
```

### 51 Subtasks

1. **51.1 — Echo-off detection**
   Implement TTY echo-off detection. Either query the PTY attributes from the consumer thread,
   or add a mechanism to detect when the child process changes the terminal's echo setting.

2. **51.2 — Visual indicator**
   Add a password mode indicator to the GUI. Show in the tab bar (if tabs exist) or title bar.
   Use a lock icon or colored label.

3. **51.3 — Scrollback suppression (optional)**
   When `suppress_password_scrollback` is enabled and echo-off is detected, stop appending to
   the scrollback buffer. Resume when echo is re-enabled.

4. **51.4 — Config and tests**
   Add config options to `[security]` section. Tests for echo-off detection and indicator
   state transitions.

### 51 Primary Files

- `freminal-terminal-emulator/src/io/pty.rs` (TTY attribute query)
- `freminal/src/gui/mod.rs` (indicator rendering)
- `freminal-common/src/config.rs` (`SecurityConfig` extension)

---

## Task 52 — Adaptive Light/Dark Theming

### 52 Overview

Automatic theme switching based on the OS light/dark mode preference. The theme mode
should be "auto" (default), "dark", or "light".

### 52 Design

**Theme Mode:** Add to `ThemeConfig`:

```rust
pub enum ThemeMode {
    Auto,  // Follow OS preference
    Dark,  // Always use the dark theme
    Light, // Always use the light theme
}
```

**Config:**

```toml
[theme]
name = "catppuccin-mocha"         # Used when mode = "dark" or as the dark theme in "auto"
mode = "auto"                     # "auto", "dark", or "light"
light_name = "catppuccin-latte"   # Used when mode = "light" or as the light theme in "auto"
```

**OS Detection:** egui provides `ctx.style().visuals.dark_mode` which reflects the OS
preference (when the windowing backend supports it). On each frame, check the OS preference.
If it has changed and `mode = "auto"`, switch between `name` (dark) and `light_name` (light).

**DECRQM ?2031:** The codebase already has mode `?2031`. Verify that it correctly reports
the current light/dark state. Applications can query this to adapt their own colors.

### 52 Subtasks

1. **52.1 — `ThemeMode` enum and config extension**
   Add `ThemeMode` to `ThemeConfig`. Add `mode` and `light_name` fields. Update defaults,
   validation, config parsing.

2. **52.2 — OS preference detection**
   Detect the OS dark/light preference via egui's visuals API. Track changes between frames.

3. **52.3 — Automatic theme switching**
   When `mode = "auto"` and the OS preference changes, send `InputEvent::ThemeChange` with
   the appropriate theme. Update `update_egui_theme()`.

4. **52.4 — DECRQM ?2031 verification**
   Verify that mode `?2031` correctly reports the current dark/light state. Fix if needed.

5. **52.5 — Config propagation**
   Update `config_example.toml`, `nix/home-manager-module.nix`, Settings Modal theme tab.

6. **52.6 — Tests**
   Unit tests: config parsing for all mode values, theme selection logic.

### 52 Primary Files

- `freminal-common/src/config.rs` (`ThemeConfig`, `ThemeMode`)
- `freminal/src/gui/mod.rs` (OS preference detection, theme switching)
- `freminal-common/src/themes.rs` (theme lookup)
- `config_example.toml`
- `nix/home-manager-module.nix`

---

## Dependency Graph

```text
Task 45 (Search) ── depends on keybindings (Task 37, v0.3.0) for shortcut registration
Task 46 (Block Selection) ── depends on selection model (Task 38, v0.3.0)
Tasks 47-50 are independent of each other and of v0.3.0 features.
Task 51 (Password) ── independent
Task 52 (Adaptive Theming) ── independent
```

All v0.4.0 tasks can begin once v0.3.0 is complete. Tasks 47-52 have no inter-dependencies
and can run fully in parallel. Tasks 45 and 46 benefit from the keybinding and selection
infrastructure from v0.3.0 but could be developed on stubs if needed.

---

## Config Schema Additions Summary

```toml
[theme]
mode = "auto"
light_name = "catppuccin-latte"

[security]
password_indicator = true
# suppress_password_scrollback = false
```

Plus new `KeyAction` variants for search and command jump keybindings.

---

## Completion Criteria

Per `agents.md`, each task is complete when:

1. All subtasks marked complete
2. `cargo test --all` passes
3. `cargo clippy --all-targets --all-features -- -D warnings` passes
4. `cargo-machete` passes
5. Benchmarks show no unexplained regressions for render/buffer changes
6. Config schema additions propagated to config.rs, config_example.toml, home-manager, settings
