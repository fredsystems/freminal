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

| Bit | Dec | Name                              | Implemented               |
| --- | --- | --------------------------------- | ------------------------- |
| 0   | 1   | `DISAMBIGUATE_ESCAPE`             | Yes — encoding active     |
| 1   | 2   | `REPORT_EVENT_TYPES`              | Parsed, stored, no output |
| 2   | 4   | `REPORT_ALTERNATE_KEYS`           | Parsed, stored, no output |
| 3   | 8   | `REPORT_ALL_KEYS_AS_ESCAPE_CODES` | Yes — encoding active     |
| 4   | 16  | `REPORT_ASSOCIATED_TEXT`          | Parsed, stored, no output |

Encoding activates only for `flags & (1 | 8)`. Flags 2, 4, 16 are v0.4.0 Task 50.

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
