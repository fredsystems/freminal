# PLAN_11_THEMING.md — Embedded Color Themes

## Status: Complete

---

## Goal

Replace the hardcoded Catppuccin Mocha color palette with a runtime-switchable theme system.
Users pick from a curated set of ~25 embedded themes via the Settings Modal or TOML config.
No OS-aware light/dark detection. No external theme file loading (Phase 1). Custom user-defined
themes via TOML are a future extension (Phase 2, out of scope for this plan).

---

## Architecture Overview

Colors are currently resolved in **two independent places**:

1. **GUI renderer** (`freminal/src/gui/colors.rs`): `internal_color_to_egui()` and
   `internal_color_to_gl()` map `TerminalColor` enum variants to hardcoded Catppuccin Mocha
   `Color32` / `[f32; 4]` constants.

2. **Buffer layer** (`freminal-common/src/colors.rs`): `default_index_to_rgb()` maps palette
   indices 0–15 to hardcoded Catppuccin Mocha RGB triples. Used by `ColorPalette::get_rgb()`
   as the fallback when no OSC 4 override is set, and by OSC 10/11 query responses.

Both must read from the same runtime palette when a theme is switched. The 6×6×6 color cube
(indices 16–231) and greyscale ramp (indices 232–255) are computed mathematically and are
theme-independent — they do not change.

### Data Flow After Refactor

```text
Config (TOML / Settings Modal)
  └─ theme.name = "dracula"
       │
       ▼
  ThemePalette::by_name("dracula")  →  &'static ThemePalette
       │
       ├──► PTY thread: TerminalState holds &'static ThemePalette
       │      └─ default_index_to_rgb() reads from it
       │      └─ OSC 10/11 query responds from it
       │      └─ build_snapshot() includes palette reference
       │
       └──► GUI thread: reads ThemePalette from snapshot (or from config)
              └─ internal_color_to_gl() reads from it
              └─ internal_color_to_egui() reads from it
              └─ CURSOR, SELECTION_BG, etc. read from it
```

---

## ThemePalette Struct

The central data type. Each embedded theme is a `const` instance of this struct.

```rust
/// A complete terminal color palette.
///
/// Contains the 16 ANSI colors (normal + bright), special-purpose colors
/// (foreground, background, cursor, selection), and metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePalette {
    /// Human-readable name for display in the Settings Modal.
    pub name: &'static str,

    /// Machine-readable slug for TOML config (e.g. "catppuccin-mocha").
    pub slug: &'static str,

    /// Default foreground color (text).
    pub foreground: (u8, u8, u8),

    /// Default background color.
    pub background: (u8, u8, u8),

    /// Cursor fill color.
    pub cursor: (u8, u8, u8),

    /// Text color when drawn under the cursor.
    pub cursor_text: (u8, u8, u8),

    /// Selection highlight background.
    pub selection_bg: (u8, u8, u8),

    /// Selection highlight foreground.
    pub selection_fg: (u8, u8, u8),

    /// The 16 ANSI colors: indices 0–7 (normal) and 8–15 (bright).
    /// Layout: [black, red, green, yellow, blue, magenta, cyan, white,
    ///          bright_black, bright_red, ..., bright_white]
    pub ansi: [(u8, u8, u8); 16],
}
```

### Location

`freminal-common/src/themes.rs` — new module in `freminal-common`. This keeps theme data
available to both the buffer layer and the GUI crate without introducing upward dependencies.

---

## Curated Theme List (~25 themes)

The following themes cover the most popular choices across major terminal emulators. Each
requires only 22 RGB values (16 ANSI + fg + bg + cursor + cursor_text + selection_bg +
selection_fg). Values will be sourced from each theme's official documentation or canonical
color definitions.

### Dark Themes

| #   | Theme Name           | Slug                 | Source / License    |
| --- | -------------------- | -------------------- | ------------------- |
| 1   | Catppuccin Mocha     | catppuccin-mocha     | Official (MIT)      |
| 2   | Catppuccin Macchiato | catppuccin-macchiato | Official (MIT)      |
| 3   | Catppuccin Frappe    | catppuccin-frappe    | Official (MIT)      |
| 4   | Dracula              | dracula              | Official (MIT)      |
| 5   | Nord                 | nord                 | Official (MIT)      |
| 6   | Solarized Dark       | solarized-dark       | Official (MIT)      |
| 7   | Gruvbox Dark         | gruvbox-dark         | Official (MIT)      |
| 8   | One Dark             | one-dark             | Atom (MIT)          |
| 9   | Tokyo Night          | tokyo-night          | Official (MIT)      |
| 10  | Tokyo Night Storm    | tokyo-night-storm    | Official (MIT)      |
| 11  | Kanagawa             | kanagawa             | Official (MIT)      |
| 12  | Rose Pine            | rose-pine            | Official (MIT)      |
| 13  | Rose Pine Moon       | rose-pine-moon       | Official (MIT)      |
| 14  | Monokai Pro          | monokai-pro          | Public color values |
| 15  | Ayu Dark             | ayu-dark             | Official (MIT)      |
| 16  | Everforest Dark      | everforest-dark      | Official (MIT)      |
| 17  | Material Dark        | material-dark        | Public color values |

### Light Themes

| #   | Theme Name       | Slug             | Source / License |
| --- | ---------------- | ---------------- | ---------------- |
| 18  | Catppuccin Latte | catppuccin-latte | Official (MIT)   |
| 19  | Solarized Light  | solarized-light  | Official (MIT)   |
| 20  | Gruvbox Light    | gruvbox-light    | Official (MIT)   |
| 21  | One Light        | one-light        | Atom (MIT)       |
| 22  | Rose Pine Dawn   | rose-pine-dawn   | Official (MIT)   |
| 23  | Ayu Light        | ayu-light        | Official (MIT)   |
| 24  | Everforest Light | everforest-light | Official (MIT)   |

### Classic / Terminal Defaults

| #   | Theme Name    | Slug          | Source / License      |
| --- | ------------- | ------------- | --------------------- |
| 25  | XTerm Default | xterm-default | Standard xterm colors |

The exact list can be adjusted during implementation. The infrastructure supports adding more
themes trivially — each theme is a single `const ThemePalette` definition.

---

## Affected Files

### `freminal-common` (shared types)

| File                  | Change                                                                         |
| --------------------- | ------------------------------------------------------------------------------ |
| `src/themes.rs` (NEW) | `ThemePalette` struct, `const` definitions for all ~25 themes, lookup fn       |
| `src/lib.rs`          | Add `pub mod themes;`                                                          |
| `src/colors.rs`       | `default_index_to_rgb()` takes `&ThemePalette` parameter instead of hardcoding |
| `src/config.rs`       | `ThemeConfig` validated against known theme slugs                              |

### `freminal-terminal-emulator` (PTY-side)

| File                    | Change                                                                              |
| ----------------------- | ----------------------------------------------------------------------------------- |
| `src/state/internal.rs` | `TerminalState` holds active `&'static ThemePalette`; OSC 10/11 query reads from it |
| `src/interface.rs`      | `build_snapshot()` includes theme reference in snapshot                             |
| `src/snapshot.rs`       | Add `theme: &'static ThemePalette` field (or foreground/background RGB)             |

### `freminal` (GUI)

| File                  | Change                                                                       |
| --------------------- | ---------------------------------------------------------------------------- |
| `src/gui/colors.rs`   | `internal_color_to_egui()` and `internal_color_to_gl()` take `&ThemePalette` |
| `src/gui/colors.rs`   | Remove all hardcoded `const` color values (replace with palette reads)       |
| `src/gui/renderer.rs` | Pass theme to color conversion calls                                         |
| `src/gui/mod.rs`      | Background fill reads from theme; DECSCNM inversion reads from theme         |
| `src/gui/settings.rs` | Theme tab lists all embedded themes in ComboBox                              |
| `src/main.rs`         | Look up `ThemePalette` from config at startup; pass to PTY thread and GUI    |

### Config

| File                  | Change                                    |
| --------------------- | ----------------------------------------- |
| `config_example.toml` | Document available theme slugs in comment |

---

## Implementation Subtasks

Subtasks are strictly ordered. Each must leave `cargo test --all` passing.

### 11.1 — Define `ThemePalette` struct and Catppuccin Mocha theme

**Scope:** `freminal-common/src/themes.rs` (new), `freminal-common/src/lib.rs`

- Create `freminal-common/src/themes.rs`.
- Define the `ThemePalette` struct as specified above.
- Define `pub const CATPPUCCIN_MOCHA: ThemePalette` with the exact RGB values currently
  hardcoded in `gui/colors.rs` and `colors.rs`.
- Add `pub fn by_slug(slug: &str) -> Option<&'static ThemePalette>` that returns the
  matching theme.
- Add `pub fn all_themes() -> &'static [&'static ThemePalette]` that returns all themes
  in display order.
- Add `pub mod themes;` to `lib.rs`.
- Add unit tests: `by_slug("catppuccin-mocha")` returns the correct palette, unknown slug
  returns `None`, `all_themes()` is non-empty and contains Catppuccin Mocha.
- **Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

**Status:** Not Started

---

### 11.2 — Add all ~25 embedded theme definitions

**Scope:** `freminal-common/src/themes.rs`

- Define `const` `ThemePalette` instances for all themes listed in Section "Curated Theme
  List". Each requires 22 RGB values sourced from the theme's official documentation.
- Update `by_slug()` and `all_themes()` to include all themes.
- Add a unit test that verifies all slugs are unique and all names are unique.
- Add a unit test that verifies each theme's ANSI array has exactly 16 entries (enforced
  by the type system, but a smoke test is good documentation).
- **Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

**Status:** Not Started

---

### 11.3 — Wire `default_index_to_rgb()` to accept a `ThemePalette`

**Scope:** `freminal-common/src/colors.rs`, `freminal-buffer/src/terminal_handler.rs`,
`freminal-terminal-emulator/src/state/internal.rs`

- Change `default_index_to_rgb(index: u8)` to `default_index_to_rgb(index: u8, theme: &ThemePalette)`.
- The function reads `theme.ansi[i]` for indices 0–15 instead of the hardcoded match arms.
- Indices 16–231 (cube) and 232–255 (greyscale) remain unchanged — they are theme-independent.
- Similarly update `lookup_default_256_color()` to take `&ThemePalette`.
- Update `ColorPalette::lookup()` and `ColorPalette::get_rgb()` to accept `&ThemePalette`
  and pass it through to the default lookup.
- Update all call sites. Initially, all call sites pass `&themes::CATPPUCCIN_MOCHA` so
  behaviour is unchanged.
- **Verify:** `cargo test --all` — all existing color tests pass with identical results.

**Status:** Not Started

---

### 11.4 — Wire `internal_color_to_egui()` and `internal_color_to_gl()` to `ThemePalette`

**Scope:** `freminal/src/gui/colors.rs`, `freminal/src/gui/renderer.rs`,
`freminal/src/gui/mod.rs`

- Change `internal_color_to_egui(color, make_faint)` to
  `internal_color_to_egui(color, make_faint, theme: &ThemePalette)`.
- Change `internal_color_to_gl(color, make_faint)` to
  `internal_color_to_gl(color, make_faint, theme: &ThemePalette)`.
- Both functions read from `theme.ansi[i]`, `theme.foreground`, `theme.background` instead
  of the hardcoded `const` values.
- Remove all hardcoded `const` palette values (`BLACK`, `RED`, ..., `BRIGHT_WHITE`, `TEXT`,
  `BASE`, and their `_F` counterparts). Replace with runtime reads from the theme.
- Keep `color32_to_f32()` as a utility.
- Keep `CURSOR`, `CURSOR_TEXT`, `SELECTION_BG`, `SELECTION_FG` reads from
  `theme.cursor`, `theme.cursor_text`, `theme.selection_bg`, `theme.selection_fg`.
- Update all call sites in `renderer.rs` and `mod.rs` to pass the active theme.
- Initially, all call sites pass `&themes::CATPPUCCIN_MOCHA` so behaviour is unchanged.
- Update existing tests in `colors.rs` to pass the Catppuccin Mocha palette explicitly.
- **Verify:** `cargo test --all` — identical rendering behaviour.

**Status:** Not Started

---

### 11.5 — Thread theme through PTY side and snapshot

**Scope:** `freminal-terminal-emulator/src/state/internal.rs`,
`freminal-terminal-emulator/src/interface.rs`,
`freminal-terminal-emulator/src/snapshot.rs`,
`freminal/src/main.rs`

- Add `theme: &'static ThemePalette` field to `TerminalState`. Pass it through the
  constructor.
- `TerminalState` passes the theme to `default_index_to_rgb()` calls (for OSC 10/11
  query responses and any other palette-default lookups).
- Add `theme: &'static ThemePalette` to `TerminalSnapshot`. `build_snapshot()` copies
  the reference.
- In `main.rs`: look up the theme from `config.theme.name` using `themes::by_slug()`.
  Fall back to `CATPPUCCIN_MOCHA` if the slug is unrecognized (with a warning log).
  Pass the theme to the `TerminalState` constructor.
- The GUI reads `snapshot.theme` and passes it to `internal_color_to_gl()` etc.
- **Verify:** `cargo test --all`. Application runs with correct Catppuccin Mocha colors.

**Status:** Not Started

---

### 11.6 — Wire the Settings Modal theme picker

**Scope:** `freminal/src/gui/settings.rs`, `freminal-common/src/config.rs`

- In `show_theme_tab()`: replace the single "Catppuccin Mocha" entry with a loop over
  `themes::all_themes()`, displaying each theme's `name` and setting `draft.theme.name`
  to the theme's `slug`.
- Remove the "Custom themes are planned for a future release" placeholder text.
- Add a small color preview strip below the ComboBox: render 16 small colored rectangles
  showing the selected theme's ANSI colors, plus fg/bg swatches. This gives the user
  immediate visual feedback before clicking Apply.
- Add validation in `Config::validate()`: if `theme.name` is not a recognized slug, return
  a `ConfigError::Validation` error.
- Update `config_example.toml` with a comment listing all available theme slugs.
- **Verify:** `cargo test --all`. Settings Modal shows all themes. Selecting a different
  theme and clicking Apply changes the terminal colors.

**Status:** Not Started

---

### 11.7 — Live theme switching (hot-reload on Apply)

**Scope:** `freminal/src/gui/mod.rs`, `freminal/src/main.rs`

- When `SettingsAction::Applied` is returned from the settings modal, look up the new
  theme from `applied_config().theme.name`.
- Send the new theme to the PTY thread. This requires a new `InputEvent` variant:
  `InputEvent::ThemeChange(&'static ThemePalette)`. The PTY thread updates its
  `TerminalState.theme` field and re-publishes a snapshot.
- The GUI thread updates its own theme reference (used for egui chrome colors like
  `window_fill`, `panel_fill`).
- The next frame renders with the new colors — no restart required.
- **Verify:** `cargo test --all`. Switching themes in the settings modal takes effect
  immediately.

**Status:** Not Started

---

### 11.8 — OSC 10/11 set path and OSC 110/111 reset

**Scope:** `freminal-terminal-emulator/src/ansi_components/osc.rs`,
`freminal-buffer/src/terminal_handler.rs`

- With the theme now threaded through, implement the OSC 10/11 "set" path: when an
  application sends `OSC 10 ; rgb:RR/GG/BB ST`, store the override in a new
  `fg_override: Option<(u8,u8,u8)>` field on `TerminalState` (similar to how
  `ColorPalette` stores per-index overrides).
- OSC 110 resets `fg_override` to `None` (restoring the theme default).
- OSC 111 resets `bg_override` to `None`.
- OSC 10/11 query path reads the override first, then falls back to the theme.
- The snapshot carries the effective fg/bg (override or theme default) so the GUI
  uses the correct colors.
- Add tests for set, query, and reset paths.
- **Verify:** `cargo test --all`. `printf '\e]10;rgb:ff/00/00\a'` changes the
  foreground to red. `printf '\e]110\a'` resets it.

**Status:** Not Started

---

### 11.9 — Tests and documentation

**Scope:** All crates

- Ensure every public function added has unit tests.
- Ensure theme switching is covered by an integration test: construct a `TerminalState`
  with theme A, process some output, switch to theme B, verify `default_index_to_rgb()`
  returns theme B values.
- Verify OSC 10/11/110/111 round-trip with theme changes.
- Run full verification suite:
  - `cargo test --all`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo-machete`
- Update `ESCAPE_SEQUENCE_COVERAGE.md` to mark OSC 10/11 set and OSC 110/111 as ✅.
- Update `ESCAPE_SEQUENCE_GAPS.md` to remove the resolved entries.

**Status:** Not Started

---

## Design Decisions

### Why `&'static ThemePalette` instead of `Arc<ThemePalette>`?

All embedded themes are `const` values with `'static` lifetime. A `&'static` reference is
zero-cost: no heap allocation, no reference counting, no atomic operations. The snapshot
carries a pointer, not owned data. When we add custom user-defined themes (Phase 2), those
will be `Box::leak`-ed or stored in a global `Vec` that outlives all threads — still
`&'static`.

### Why not carry full RGB arrays in the snapshot?

Carrying 22 RGB values (528 bytes) in every snapshot is wasteful when a `&'static` pointer
(8 bytes) suffices. The theme changes at most once per user action, not per PTY batch.

### Why not use `TerminalColor` enum variants for theme resolution?

The current `TerminalColor` enum uses named variants (`Red`, `BrightBlue`, etc.) for the
16 ANSI colors. These are resolved to concrete RGB at the renderer boundary. The theme
system slots into this existing resolution step cleanly: `TerminalColor::Red` →
`theme.ansi[1]`. No enum changes needed.

### Why not resolve colors earlier (in the buffer layer)?

Cells store `TerminalColor` values, not concrete RGB. This is correct: if the user
switches themes, all existing text should re-render in the new colors. Storing concrete
RGB in cells would require a full buffer re-write on every theme change.

### Phase 2 — Custom User Themes (out of scope)

A future extension would allow users to define custom themes in TOML:

```toml
[theme]
name = "my-custom-theme"

[theme.colors]
foreground = "#d4d4d4"
background = "#1e1e1e"
black = "#000000"
red = "#cd3131"
# ... etc
```

This requires: TOML parsing into `ThemePalette`, validation, `Box::leak` for `'static`
lifetime, and fallback logic for missing colors. The infrastructure built in Phase 1
supports this naturally — the `by_slug()` function just needs an additional lookup path.

---

## Dependencies

- **Task 1 (Custom Terminal Renderer):** The renderer uses `internal_color_to_gl()` which
  this task modifies. If Task 1 is in progress concurrently, coordinate on the color
  conversion function signatures.
- **Task 3 (Settings Modal):** Already complete. The theme tab exists but is a stub. This
  task replaces the stub with a real implementation.

---

## Verification

A subtask is complete when all of the following pass:

1. `cargo test --all` — zero failures
2. `cargo clippy --all-targets --all-features -- -D warnings` — zero warnings
3. `cargo-machete` — no unused dependencies

The overall task is complete when:

- All 9 subtasks are marked Done
- Switching themes in the Settings Modal changes terminal colors immediately
- Theme persists across restart via TOML config
- OSC 10/11 set/reset works with the active theme
- All escape sequence docs are updated

---

## Overall Progress

- [x] 11.1 — Define `ThemePalette` struct and Catppuccin Mocha theme
- [x] 11.2 — Add all ~25 embedded theme definitions
- [x] 11.3 — Wire `default_index_to_rgb()` to accept `ThemePalette`
- [x] 11.4 — Wire `internal_color_to_egui/gl()` to `ThemePalette`
- [x] 11.5 — Thread theme through PTY side and snapshot
- [x] 11.6 — Wire Settings Modal theme picker
- [x] 11.7 — Live theme switching (hot-reload on Apply)
- [x] 11.8 — OSC 10/11 set path and OSC 110/111 reset
- [x] 11.9 — Tests and documentation
