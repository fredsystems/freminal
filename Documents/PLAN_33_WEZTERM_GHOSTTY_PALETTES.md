# PLAN 33 — WezTerm & Ghostty Default Color Palettes

## Status: Complete

## Overview

Add the default color palettes from WezTerm and Ghostty as built-in themes in Freminal. Both are
popular modern terminal emulators and users migrating from either will expect their familiar colors
to be available.

This is a small, self-contained change to a single file (`freminal-common/src/themes.rs`).

## Current State

- 25 built-in themes in `freminal-common/src/themes.rs`
- Each theme is a `const ThemePalette` with: name, slug, foreground, background, cursor,
  cursor_text, selection_bg, selection_fg, and 16 ANSI colors
- Adding a new theme requires: (1) define the const, (2) add to `ALL_THEMES`, (3) update test count
- No WezTerm or Ghostty palettes exist in the current set

## Color Data

### WezTerm Default

Source: `term/src/color.rs` — `ColorPalette::compute_default()` in the WezTerm repository.

**Special colors:**

| Role        | Hex       | RGB                  | Notes                                                      |
| ----------- | --------- | -------------------- | ---------------------------------------------------------- |
| Foreground  | `#b2b2b2` | `(0xb2, 0xb2, 0xb2)` | `colors[249]` — greyscale ramp entry                       |
| Background  | `#000000` | `(0x00, 0x00, 0x00)` | ANSI Black (index 0)                                       |
| Cursor      | `#52ad70` | `(0x52, 0xad, 0x70)` | Distinctive green — a WezTerm signature                    |
| Cursor text | `#000000` | `(0x00, 0x00, 0x00)` | Same as background                                         |
| Selection   | —         | see below            | WezTerm uses semi-transparent purple; needs opaque mapping |

**Selection color rationale:** WezTerm uses `rgba(0.5, 0.4, 0.6, 0.5)` for selection background
(semi-transparent purple) and fully transparent foreground (inherits cell color). Since
`ThemePalette` requires opaque RGB values, we use a muted purple approximation for the background
and the foreground color for selection text:

- `selection_bg: (0x4d, 0x40, 0x60)` — opaque approximation of the purple tint
- `selection_fg: (0xd0, 0xd0, 0xd0)` — light grey matching the terminal's text feel

**16 ANSI colors:**

| Index | Name          | Hex       | RGB                  |
| ----- | ------------- | --------- | -------------------- |
| 0     | Black         | `#000000` | `(0x00, 0x00, 0x00)` |
| 1     | Red           | `#cc5555` | `(0xcc, 0x55, 0x55)` |
| 2     | Green         | `#55cc55` | `(0x55, 0xcc, 0x55)` |
| 3     | Yellow        | `#cdcd55` | `(0xcd, 0xcd, 0x55)` |
| 4     | Blue          | `#5455cb` | `(0x54, 0x55, 0xcb)` |
| 5     | Magenta       | `#cc55cc` | `(0xcc, 0x55, 0xcc)` |
| 6     | Cyan          | `#7acaca` | `(0x7a, 0xca, 0xca)` |
| 7     | White         | `#cccccc` | `(0xcc, 0xcc, 0xcc)` |
| 8     | BrightBlack   | `#555555` | `(0x55, 0x55, 0x55)` |
| 9     | BrightRed     | `#ff5555` | `(0xff, 0x55, 0x55)` |
| 10    | BrightGreen   | `#55ff55` | `(0x55, 0xff, 0x55)` |
| 11    | BrightYellow  | `#ffff55` | `(0xff, 0xff, 0x55)` |
| 12    | BrightBlue    | `#5555ff` | `(0x55, 0x55, 0xff)` |
| 13    | BrightMagenta | `#ff55ff` | `(0xff, 0x55, 0xff)` |
| 14    | BrightCyan    | `#55ffff` | `(0x55, 0xff, 0xff)` |
| 15    | BrightWhite   | `#ffffff` | `(0xff, 0xff, 0xff)` |

---

### Ghostty Default (Tomorrow Night)

Source: `src/terminal/color.zig` — `Name.default()` and `src/config/Config.zig` in the Ghostty
repository. Ghostty's default palette is the Tomorrow Night color scheme by Chris Kempson.

**Special colors:**

| Role        | Hex       | RGB                  | Notes                                           |
| ----------- | --------- | -------------------- | ----------------------------------------------- |
| Foreground  | `#ffffff` | `(0xff, 0xff, 0xff)` | Pure white                                      |
| Background  | `#282c34` | `(0x28, 0x2c, 0x34)` | Dark blue-grey                                  |
| Cursor      | `#ffffff` | `(0xff, 0xff, 0xff)` | Not set in config; defaults to foreground       |
| Cursor text | `#282c34` | `(0x28, 0x2c, 0x34)` | Not set in config; defaults to background       |
| Selection   | —         | see below            | Ghostty uses transparency; needs opaque mapping |

**Selection color rationale:** Ghostty does not define explicit default selection colors; it uses a
transparency-based highlight at render time. The background `#282c34` is nearly identical to One
Dark's `#282c3e`, so we use similar selection values that complement the Tomorrow Night palette:

- `selection_bg: (0x3e, 0x44, 0x52)` — lighter blue-grey, visible against the dark background
- `selection_fg: (0xc5, 0xc8, 0xc6)` — matches ANSI White (index 7), natural for the palette

**16 ANSI colors:**

| Index | Name          | Hex       | RGB                  |
| ----- | ------------- | --------- | -------------------- |
| 0     | Black         | `#1d1f21` | `(0x1d, 0x1f, 0x21)` |
| 1     | Red           | `#cc6666` | `(0xcc, 0x66, 0x66)` |
| 2     | Green         | `#b5bd68` | `(0xb5, 0xbd, 0x68)` |
| 3     | Yellow        | `#f0c674` | `(0xf0, 0xc6, 0x74)` |
| 4     | Blue          | `#81a2be` | `(0x81, 0xa2, 0xbe)` |
| 5     | Magenta       | `#b294bb` | `(0xb2, 0x94, 0xbb)` |
| 6     | Cyan          | `#8abeb7` | `(0x8a, 0xbe, 0xb7)` |
| 7     | White         | `#c5c8c6` | `(0xc5, 0xc8, 0xc6)` |
| 8     | BrightBlack   | `#666666` | `(0x66, 0x66, 0x66)` |
| 9     | BrightRed     | `#d54e53` | `(0xd5, 0x4e, 0x53)` |
| 10    | BrightGreen   | `#b9ca4a` | `(0xb9, 0xca, 0x4a)` |
| 11    | BrightYellow  | `#e7c547` | `(0xe7, 0xc5, 0x47)` |
| 12    | BrightBlue    | `#7aa6da` | `(0x7a, 0xa6, 0xda)` |
| 13    | BrightMagenta | `#c397d8` | `(0xc3, 0x97, 0xd8)` |
| 14    | BrightCyan    | `#70c0b1` | `(0x70, 0xc0, 0xb1)` |
| 15    | BrightWhite   | `#eaeaea` | `(0xea, 0xea, 0xea)` |

---

## Subtasks

### 33.1 — Add WezTerm Default and Ghostty Default theme consts

**Status:** Complete (2026-04-01)
**Priority:** Medium
**Scope:** `freminal-common/src/themes.rs` only

**Details:**

1. Add `pub const WEZTERM_DEFAULT: ThemePalette` after `XTERM_DEFAULT` (before `DEFAULT_THEME`),
   using the exact color values from the WezTerm section above. Source comment:
   `/// Source: <https://github.com/wez/wezterm> (term/src/color.rs)`

2. Add `pub const GHOSTTY_DEFAULT: ThemePalette` after `WEZTERM_DEFAULT`, using the exact color
   values from the Ghostty section above. Source comment:
   `/// Source: <https://github.com/ghostty-org/ghostty> (src/terminal/color.zig)`

3. Add `&WEZTERM_DEFAULT` and `&GHOSTTY_DEFAULT` to the `ALL_THEMES` array, after `&XTERM_DEFAULT`.

4. Update the `all_themes_contains_25_themes` test to assert 27 instead of 25. Rename it to
   `all_themes_contains_27_themes`.

**Acceptance criteria:**

- Both themes appear in `all_themes()` and can be looked up via `by_slug("wezterm-default")`
  and `by_slug("ghostty-default")`.
- All existing theme tests pass unchanged (unique slugs, unique names, 16 ANSI entries each).
- `cargo test --all` passes.
- `cargo clippy --all-targets --all-features -- -D warnings` passes.
- `cargo-machete` passes.

**Tests required:**

- The existing `all_slugs_are_unique` and `all_names_are_unique` tests cover the new themes
  automatically.
- The existing `all_ansi_arrays_have_16_entries` test covers the new themes automatically.
- The count test update from 25 to 27 validates registration.
- No additional test functions are needed — the existing test suite exercises all themes
  generically.

---

## Implementation Notes

- **Single file change.** Only `freminal-common/src/themes.rs` is modified. No other crate or
  file needs any change. The theme picker, snapshot threading, and color conversion functions all
  consume `ALL_THEMES` dynamically.

- **Selection color approximation.** Both WezTerm and Ghostty use transparency-based selection
  highlighting at render time. `ThemePalette` requires opaque `(u8, u8, u8)` values. The
  approximations chosen are documented above with rationale. If the user finds the visual result
  unsatisfactory after testing, the values can be tuned in a follow-up commit.

- **Naming convention.** Following the existing `XTERM_DEFAULT` pattern: `WEZTERM_DEFAULT`
  (name: "WezTerm Default", slug: "wezterm-default") and `GHOSTTY_DEFAULT` (name: "Ghostty
  Default", slug: "ghostty-default").

- **Display order.** WezTerm and Ghostty are placed after XTerm Default, grouping all
  "terminal emulator defaults" together at the end of the list.

## References

- `freminal-common/src/themes.rs` — theme definitions, registry, and tests
- WezTerm source: `term/src/color.rs` — `ColorPalette::compute_default()`
- Ghostty source: `src/terminal/color.zig` — `Name.default()`
- `Documents/PLAN_11_THEMING.md` — original theming implementation plan
