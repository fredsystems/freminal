# PLAN_VERSION_100.md — v0.10.0 "Beautification & Fonts"

> **STATUS: ACTIVATED.** Decomposed into Tasks 111 and 112 (see "Task 111 —
> Bundled Font / Icon" and "Task 112 — UI Beautification" below). Durable design
> decisions and the activation-time audits (font analysis, egui capability) are
> captured in the sections above the subtask breakdowns.

## Goal

Pay down the visual debt accumulated across v0.2.0–v0.9.0 before later versions
add more surfaces on top of it. Two intertwined fronts, shipped together because
they solve the same problem — the bare-bones visual baseline — from different
angles:

1. **A major UI beautification pass.** Theme-consistent menu bars, modals, dialogs, and toast notifications; bundled glyph/icon assets so action symbols (close, recording, broadcast, etc.) render identically on every platform instead of relying on whatever the system font happens to supply.
2. **A bundled-font overhaul.** Out-of-the-box ligature _and_ powerline glyph
   coverage, so the default experience is complete without the user installing
   a Nerd Font.

Depends on v0.8.0 and v0.9.0. This version lands **before** the AI-assist
versions (v0.18.0–v0.19.0) deliberately: those versions introduce more modals
and overlays, and every new modal inherits whatever visual baseline exists.
Styling the baseline first means later work is styled by construction rather
than retrofitted.

**The terminal buffer is explicitly out of scope for the beautification pass.**
This version restyles chrome (modals, toasts, menus, dialogs) only. Buffer
rendering — cell grid, glyph atlas, shaping — is untouched except where the
bundled-font change alters the default glyph set.

---

## Task Summary

| #   | Feature                     | Scope  | Status   | Depends On       |
| --- | --------------------------- | ------ | -------- | ---------------- |
| 111 | Bundled Font / Icon         | Large  | Complete | v0.8.0, v0.9.0   |
| 112 | UI Beautification           | Large  | Active   | v0.8.0, Task 111 |
| 113 | Resize / Reflow Scroll Bugs | Medium | Pending  | None             |

Task numbers assigned at activation (111, 112). Source: `Documents/PLANNING.MD`
"Pre-1.0 Remediations" (UI aesthetic, built-in fonts). Task 112 depends on Task
111 because the bundled-icon asset pipeline (112) reuses the bundled-asset
precedent established by the font swap (111), and both touch glyph rendering.

Task 113 was added after a maintainer-reported intermittent scroll-corruption
bug was root-caused during a v0.10.0 investigation session. It is independent of
the beautification work (it lives in `freminal-buffer` and
`freminal-terminal-emulator`, not the chrome) but is folded into this version
because it is a correctness bug found while this version was active and should
ship before 1.0. It has no dependency on Tasks 111/112.

---

## UI Beautification

The chrome is visually inconsistent with the user's selected theme and leans on
fallible font resources for action glyphs.

Durable problems to solve (from `PLANNING.MD`):

- **Theme inconsistency.** Modal/dialog/toast/Menu bar colors do not follow the
  user-selected theme. The beautification pass themes all chrome to the active
  theme. The terminal buffer is NOT affected.
- **Glyph reliability.** Action symbols (close, recording indicator, broadcast
  keyboard, etc.) currently rely on an emoji/powerline glyph from the system
  font. On Linux some render as empty squares; macOS/Windows untested. Relying
  on a fallible external resource for our own UI is a failure mode we must
  eliminate — **we ship the asset ourselves** so every action symbol looks the
  same everywhere and matches our aesthetic.
- **Inspiration:** ACARS Hub and modern web apps — rounded corners, consistent
  color theming, cohesive spacing.

### egui capability audit (read-only, verified at activation)

A read-only survey of the chrome rendering path and the egui 0.34.3 `Style` /
`Visuals` API established the toolkit ceiling. No code was changed.

- **Versions.** `egui`, `egui_glow`, and `egui-winit` are all **0.34.3**;
  `winit` 0.30.13; `glutin` 0.32.3. `eframe` is fully removed — the
  `freminal-windowing` crate owns the winit+glutin+egui integration directly.
- **What egui gives us is sufficient for either aesthetic.** egui 0.34's
  `Style`/`Visuals` expose full control over the surfaces that matter:
  - Per-state widget visuals via `Visuals.widgets` — `noninteractive`,
    `inactive`, `hovered`, `active`, `open` — each a `WidgetVisuals` with
    `bg_fill`, `weak_bg_fill`, `bg_stroke`, `fg_stroke`, `corner_radius`, and
    `expansion`. This means buttons, menus, combo boxes, sliders, and tabs can
    be fully recolored and reshaped per interaction state.
  - `window_corner_radius` and `menu_corner_radius` (square → fully rounded),
    `Visuals.window_fill`, `window_stroke`, `panel_fill`, `selection`
    (`Selection { bg_fill, stroke }`), `override_text_color`, and `Spacing`
    (padding/margins/scroll-bar geometry).
  - Per-widget overrides via scoped `ui.style_mut()` and custom `egui::Frame`
    (`fill`, `stroke`, `corner_radius`, `inner_margin`) — already used in
    `toast.rs` (the only current use of `Stroke` + `CornerRadius`) and the
    settings window's opaque frame (`settings.rs`).

  Conclusion: **we are not hamstrung by egui.** Both a hyper-modern rounded
  look and a sharp/retro look are fully achievable; the only thing egui will
  not do for free is render arbitrary SVG/vector chrome — custom shapes go
  through `Painter` or a bundled raster/font asset (see icon decision below).

- **Current state: chrome is essentially un-themed.** The _only_ egui visual
  fields customized today are `style.visuals.window_fill` and
  `style.visuals.panel_fill` (set from the theme's `DefaultBackground` in
  `rendering.rs:set_egui_options` / `update_egui_theme` and the per-frame
  style-cache in `app_impl.rs`). Everything else — every widget background,
  hover/active state, stroke, corner radius, menu styling, separators, scroll
  bars — is **stock default egui 0.34 dark visuals**. This is exactly why the
  screenshots show a richly-themed terminal buffer inside flat, square,
  grey-blue default chrome that ignores the selected theme.

- **Live re-theming is already proven for the two fill fields and generalizes
  cleanly.** The active palette travels to the GUI thread as
  `TerminalSnapshot.theme: &'static ThemePalette`, available before any panel
  is drawn. A theme change in the settings modal emits `PreviewTheme(slug)` →
  `InputEvent::ThemeChange` to every pane → next `build_snapshot()` carries the
  new `&'static` reference → the GUI's per-frame style cache detects the change
  by pointer identity (`!std::ptr::eq(prev, snap.theme)`) and re-applies. The
  same hook that updates `window_fill`/`panel_fill` today can apply a full
  `Visuals` derived from the palette — **live re-theming of all chrome is a
  natural extension of the existing mechanism, not new infrastructure.**

- **Palette → `Color32` helpers already exist.** `colors.rs` provides
  `internal_color_to_egui` and `internal_color_to_egui_with_alpha` (map a
  `TerminalColor` against the active `ThemePalette`); a private
  `rgb_to_color32((u8,u8,u8))` already does the trivial conversion needed to
  pull `theme.background` / `theme.foreground` / `theme.ansi[..]` into chrome.

### Decision: centralized `ChromeStyle` with selectable Modern / Retro profiles

- **All non-terminal chrome is styled through one centralized layer.** Rather
  than committing to a single aesthetic, the beautification pass builds a single
  `ChromeStyle` abstraction that derives a complete egui `Visuals` (widget
  states, corner radii, strokes, fills, selection, text color, spacing) from
  **(a)** the active `ThemePalette` and **(b)** a selectable **style profile**.
  This is the decided direction (the aesthetic is a runtime choice, not a baked
  constant).
- **Two profiles ship: Modern and Retro.**
  - **Modern** — rounded corners, soft strokes, generous padding, subtle
    hover/active states, cohesive flat fills (the ACARS Hub / modern-web
    inspiration).
  - **Retro** — sharp/square corners, harder edges, denser spacing,
    higher-contrast borders, a terminal-native feel.

  Both are fully theme-driven (colors come from the palette); they differ only
  in geometry, stroke weight, and spacing. The profile is a config option and a
  settings-modal control.

- **Single application point.** The profile + palette resolve to one `Visuals`
  applied via the existing per-frame style hook (extending the
  `window_fill`/`panel_fill` cache to a full-`Visuals` cache keyed on
  `(theme, profile, opacity, reverse-video)`). No per-widget ad-hoc styling
  scattered across `menu.rs` / `settings.rs` / modal files — they consume the
  centralized style.
- **Scope of "all chrome":** menu bar (`menu.rs`), tab bar (`menu.rs`),
  right-click context menu (`widget.rs`), settings modal _and_ standalone
  settings window (`settings.rs`), command-history palette
  (`command_history.rs`), search bar (`search.rs`), toasts (`toast.rs`),
  welcome overlay (`welcome.rs`), and the broadcast / close / paste guard and
  About / Save-Layout / unsaved-changes dialogs. The terminal buffer is
  explicitly **excluded** (it has its own palette-driven renderer).

Open questions (decide at activation):

- **Asset format for bundled icons.** Working assumption is bundled SVGs for
  all action symbols, but alternatives (icon font, pre-rasterized PNG atlas)
  are on the table. egui does not render SVG natively, so this is the one chrome
  element that needs a concrete asset-pipeline decision against the renderer as
  it then exists. (The font/glyph work in this version already establishes a
  bundled-asset precedent.)
- **Reverse-video interaction.** The current style cache forces white
  fill in reverse-video (non-normal-display) mode; the `ChromeStyle` layer must
  define how each profile behaves under reverse video.

---

## Bundled Font / Icon Coverage

The bundled font does not guarantee ligature + powerline glyph coverage, so the
default experience depends on the user installing a suitable font.

Durable problems to solve (from `PLANNING.MD`):

- **Maximum ligature AND powerline coverage out of the box**, regardless of what
  the user has installed.

### Font audit findings (read-only, verified at activation)

A read-only audit of the bundled fonts, the renderer's shaping pipeline, and the
candidate replacement (`fonttools` via `nix shell`, `fc-list`, upstream license
text) established the following facts. These are durable inputs; the audit made
no code or asset changes.

- **The ligature feature is already wired but silently dead with the default
  font.** `shaping_features()` in `freminal/src/gui/shaping.rs` enables `liga`
  and `calt` when ligatures are on, `config.font.ligatures` defaults to `true`,
  and the Settings modal advertises "Enable Ligatures". But the bundled
  **MesloLGS Nerd Font Mono** has **no ligature features at all** (its GSUB
  table contains only `rtla`). So the default install ships a feature that is
  on, advertised, and incapable of doing anything. This validates the font
  overhaul premise — it is not misplaced; the gap is worse than "no guarantee".
- **Hack is dead weight and must be dropped.** The four `res/Hack-*.ttf` files
  (~1.27 MB) are **never loaded anywhere** — no `include_bytes!`, no build
  script, no runtime path. The sole `.rs` reference
  (`freminal-common/tests/config_tests.rs`) uses the bare string `"Hack"` as an
  arbitrary font-family value in a TOML round-trip test; it is not the font.
  Removing the files, the `ATTRIBUTIONS.md` "Hack" entry, and
  `res/fonts/Hack-LICENSE.md` is a clean, license-unencumbered deletion.
  **CaskaydiaCove Nerd Font is a strict superset of MesloLGS and the license is
  clear.** Verified against the four candidate faces:

| Font (Regular)        | Ligatures (`calt`/`liga`/`dlig`)     | Nerd / powerline    | Monospace                         | Glyphs |
| --------------------- | ------------------------------------ | ------------------- | --------------------------------- | ------ |
| MesloLGS NF (current) | NONE (GSUB has only `rtla`)          | full Nerd set       | yes                               | 12,784 |
| Hack (unused)         | NONE                                 | no (7/56 powerline) | yes                               | 1,573  |
| CaskaydiaCove NF      | YES — `calt`, 285 chaining subtables | full Nerd + Braille | yes (uniform ASCII advance, 1200) | 14,724 |
| Cascadia Code NF      | YES (same `calt`)                    | full Nerd + Braille | yes                               | 13,560 |

- **License: SIL OFL 1.1**, verified against upstream
  `microsoft/cascadia-code/LICENSE`. Same license class as the MesloLGS we
  already bundle — the hard gate is cleared, redistribution and embedding are
  permitted.
- **Reserved Font Name caveat.** Cascadia Code's OFL reserves the name
  "Cascadia Code". The Nerd Fonts patch already renames it to
  **"CaskaydiaCove"** to comply; bundle under that name, do not rename back.
- **Variant caveat.** Use the **`Cove`/Code** variant (285 `calt` subtables),
  **not `CaskaydiaMono`** — the Mono variant has ligatures stripped (1 `calt`
  subtable). `isFixedPitch=0` on CaskaydiaCove is benign: ASCII advances are
  uniform; the flag is unset only because Nerd icons are double-width.

### Decision: bundle CaskaydiaCove NF, drop Hack

- **Replace the bundled MesloLGS faces with CaskaydiaCove Nerd Font** (the
  ligature-bearing `Cove` variant). It clears the license gate (OFL-1.1), is a
  superset of MesloLGS coverage (full Nerd set plus Braille), and is the only
  way the already-implemented ligature feature produces ligatures out of the
  box. The previously-disfavoured "detect platform default font" option stays
  rejected — platform fonts lack ligatures and powerline glyphs.
- **Drop the Hack font** — files, `ATTRIBUTIONS.md` entry, and
  `res/fonts/Hack-LICENSE.md`. It is unused dead weight.

---

## Design Decisions (provisional; revisit only with explicit user approval)

- **Beautification and fonts ship together.** They solve the same bare-bones
  baseline problem from different angles and are highly intertwined (both touch
  glyph rendering). Splitting them across versions would leave a half-styled
  baseline.
- **This version lands before the AI-assist versions** so later modals inherit
  a styled baseline rather than retrofitting it.
- **The terminal buffer is out of scope** for the beautification pass.
- **We bundle our own action-glyph assets.** Relying on the system font for our
  own UI symbols is a failure mode, not an acceptable fallback.
- **All chrome is styled through one centralized `ChromeStyle` layer with
  selectable Modern / Retro profiles.** Decided rather than committing to a
  single aesthetic: the styling layer derives a complete egui `Visuals` from the
  active `ThemePalette` plus a user-selectable style profile. Both profiles are
  fully theme-driven; they differ in geometry (corner radius), stroke weight,
  and spacing. The profile is a config option and a settings-modal control.
- **Live chrome re-theming is in scope and uses the existing mechanism.** The
  active palette already reaches the GUI thread via `TerminalSnapshot.theme`;
  the per-frame style hook that sets `window_fill`/`panel_fill` today is
  extended to apply the full palette-derived `Visuals`. No new transport
  infrastructure is required.
- **egui 0.34.3 is sufficient — we are not hamstrung.** Verified at the
  activation audit: `Visuals`/`Style` expose per-state `WidgetVisuals`
  (fill/stroke/`corner_radius`), `window_corner_radius`, `menu_corner_radius`,
  `selection`, `override_text_color`, and `Spacing`. The one gap is native SVG
  rendering, which only affects the bundled-icon asset-format decision.
- **Bundled font is CaskaydiaCove Nerd Font (`Cove`/Code variant), replacing
  MesloLGS.** Decided at the activation audit: it clears the license gate
  (SIL OFL 1.1, confirmed against upstream), is a superset of MesloLGS coverage,
  and is required for the already-wired ligature feature to function out of the
  box. Must be bundled under the "CaskaydiaCove" name (OFL Reserved Font Name on
  "Cascadia Code") and must be the `Cove` variant, not `CaskaydiaMono` (which
  strips ligatures).
- **The Hack font is dropped.** Confirmed unused (never loaded anywhere); the
  files, `ATTRIBUTIONS.md` entry, and `res/fonts/Hack-LICENSE.md` are removed.
- **Bundled-font choice is license-gated.** No font is bundled until its license
  is confirmed to permit redistribution. (Satisfied for CaskaydiaCove: OFL-1.1.)
- **egui-capability research is complete** (folded into "UI Beautification"
  above). egui 0.34.3 supports both aesthetic directions and live palette-driven
  re-theming; the centralized `ChromeStyle` layer is the chosen approach. The
  remaining activation research is narrowed to the bundled-icon asset format.
- Any new or restyled dialog/overlay is a focusable surface and MUST follow
  `freminal-modal-input-suppression`.
- Escape-sequence support is not touched by this version (no
  `freminal-escape-sequence-docs` dual-doc update expected).
- **Crate architecture (decided at activation):** No new crate. The chrome
  styling is a centralized module in the `freminal` binary
  (`freminal/src/gui/chrome_style.rs`) — egui stays in the binary, matching the
  `freminal-architecture` invariant. The _toolkit-agnostic data type_
  (`GuiTheme` plus `StyleProfile`, serde-derivable, zero egui) lives in
  `freminal-common` beside `ThemeConfig`/`ThemePalette`, so a future
  Lua/custom-config layer edits
  config-shaped data and the GUI maps it — mirroring the existing
  `ThemePalette` (data in common) / `colors.rs` (egui mapping in binary) split.
  Palettes are **not** moved; `freminal-common::themes` already is the common
  theme home. A separate crate is justified only by a second egui consumer,
  which does not exist.

---

## Task 111 — Bundled Font / Icon

> **STATUS: COMPLETE.** All 7 subtasks (111.1–111.7) landed on
> `task-111/bundled-font`. CaskaydiaCove NF is bundled, MesloLGS and Hack are
> removed, attributions updated, ligatures are proven against the real bundled
> face, and the font picker annotates the bundled default and any system
> duplicate.
>
> **Durable finding (from 111.6):** CaskaydiaCove (like Cascadia Code)
> implements its coding ligatures via `calt` chaining-contextual substitution
> into dedicated "ligature-piece" glyphs, **not** a many-to-one ligature
> collapse — so the shaped glyph _count_ is unchanged while the glyph _IDs_
> change. The regression test asserts on glyph-ID difference (ligatures on vs
> off), not on glyph-count reduction. The font's GSUB has `calt` only (no
> `liga`), and `calt` is registered under `DFLT`/`latn`, so no
> `guess_segment_properties()` call is needed in the shaping path.

Replace the bundled MesloLGS faces with CaskaydiaCove Nerd Font (ligature-bearing
`Cove` variant) and remove the unused Hack font. This makes the already-wired
ligature feature functional out of the box and removes dead bundled assets. See
the "Font audit findings" and "Decision: bundle CaskaydiaCove NF, drop Hack"
sections above for the durable rationale.

Sequencing: 111.1 → 111.2 → 111.3 → 111.4 → 111.5 → 111.6; 111.7 (picker
labeling) needs 111.2. Each subtask leaves `cargo test --all` green.

### Task 111 subtasks

#### 111.1 — Vendor CaskaydiaCove NF font files + license

Scope: `res/` (new font files), `res/fonts/` (new license file). No `.rs`
changes.

What: Add the four ligature-bearing CaskaydiaCove Nerd Font `Cove` faces to
`res/`: `CaskaydiaCoveNerdFont-Regular.ttf`, `-Bold.ttf`, `-Italic.ttf`,
`-BoldItalic.ttf` (the `Cove`/Code variant with 285 `calt` subtables — **NOT**
`CaskaydiaMono`, which strips ligatures). Source: Nerd Fonts release 3.4.0 (the
patched files already use the OFL-compliant "CaskaydiaCove" Reserved-Font-Name
rename). Add `res/fonts/CaskaydiaCove-NerdFont-LICENSE.md` containing the SIL OFL
1.1 text (verbatim from `microsoft/cascadia-code/LICENSE`) plus a header noting
the Nerd Fonts patch and the Cascadia Code reserved name.

Deliverable: four `.ttf` files + one license markdown file present in `res/`.
Verify each face parses and carries `calt`: `ttx -l` (via
`nix shell nixpkgs#python3Packages.fonttools`) or the audit script confirms 285
`calt` chaining subtables on Regular.

Verification: `cargo build` (files present, nothing references them yet);
`markdownlint` on the new license file.

Prohibitions: do NOT delete the Meslo files yet (111.2 swaps the references
first); do NOT use the `CaskaydiaMono` variant; do NOT rename the font family.

Stop: report files added + the `calt` subtable count from the audit tool; await
review.

#### 111.2 — Swap the bundled-font references from Meslo to CaskaydiaCove

Scope: `freminal/src/gui/font_manager.rs`, `freminal/src/gui/fonts.rs`.

What: Repoint every `include_bytes!("../../../res/MesloLGSNerdFontMono-*.ttf")`
to the CaskaydiaCove faces from 111.1. Rename the `MESLO_*` statics
(`font_manager.rs:25-29`) and the bundled-face identifiers
(`load_bundled_*` / face name strings at `font_manager.rs:851-873`) to
`CASKAYDIA_*` / "CaskaydiaCove-\*". Update the `fonts.rs` egui `FontData`
inserts (`fonts.rs:133-155`). Update the `DEFAULT_LABEL` in `settings.rs:830`
("Default (CaskaydiaCove Nerd Font)") and the doc/comment references to the
bundled font name across `font_manager.rs`, `fonts.rs`, `widget.rs:3098`,
`nix/home-manager-module.nix:222`.

Deliverable: the binary embeds CaskaydiaCove; no source reference to Meslo
remains. Existing font-loading tests pass (update the metric-range assertions
in `font_manager.rs:1345-1397` to CaskaydiaCove's cell metrics — recompute the
expected 12pt cell width/height range against the new face).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features --
-D warnings`. The `font_manager` cell-metric tests must pass with recomputed
ranges.

Prohibitions: do NOT change the shaping pipeline (`shaping.rs` already requests
`liga`/`calt`); do NOT alter the user-font fallback logic; do NOT delete the
Meslo files yet (111.3).

Stop: report the recomputed cell-metric ranges + test results; await review.

#### 111.3 — Delete the Meslo font files

Scope: `res/MesloLGSNerdFontMono-*.ttf`, `res/fonts/MesloLGS-NerdFont-LICENSE.md`.

What: Remove the four Meslo `.ttf` files and the Meslo license markdown, now
that 111.2 references CaskaydiaCove exclusively.

Deliverable: Meslo files gone; build still succeeds (confirms 111.2 was
complete).

Verification: `cargo build`; `cargo test --all`; `rg -i meslo` returns only
historical references in `Documents/` (plan/changelog), not in `res/` or `*.rs`.

Prohibitions: do NOT remove anything referenced by code (111.2 must be merged
first); do NOT touch the Hack files here (111.4).

Stop: report files removed + `rg -i meslo` results; await review.

#### 111.4 — Delete the unused Hack font + its attribution

Scope: `res/Hack-*.ttf`, `res/fonts/Hack-LICENSE.md`, `ATTRIBUTIONS.md`,
`freminal-common/tests/config_tests.rs`.

What: Remove the four `res/Hack-*.ttf` files (confirmed never loaded — no
`include_bytes!`, no runtime path) and `res/fonts/Hack-LICENSE.md`. Delete the
"Hack" entry from `ATTRIBUTIONS.md:27-34`. In
`config_tests.rs:307,315`, change the arbitrary font-family test value from
`"Hack"` to a neutral literal (e.g. `"Test Font"`) so the round-trip test no
longer name-drops a font we don't ship.

Deliverable: no Hack artifacts remain anywhere; the config round-trip test
still asserts font-family round-trips with a neutral value.

Verification: `cargo test --all`; `rg -i "\bhack\b"` returns no `res/`, `*.rs`,
or `ATTRIBUTIONS.md` hits (historical `Documents/` mentions are acceptable);
`markdownlint ATTRIBUTIONS.md`.

Prohibitions: do NOT change the meaning of the round-trip test (it still
verifies an arbitrary family name survives serialization); do NOT touch the
CaskaydiaCove attribution (111.5).

Stop: report files/lines removed + `rg` results; await review.

#### 111.5 — Add CaskaydiaCove to ATTRIBUTIONS.md

Scope: `ATTRIBUTIONS.md`.

What: Add a "CaskaydiaCove Nerd Font" attribution entry (replacing the Meslo
entry, which 111.3's license-file removal orphans): upstream
`microsoft/cascadia-code` (base, OFL-1.1, reserved name "Cascadia Code") and
`ryanoasis/nerd-fonts` (patch + "CaskaydiaCove" rename), license SIL OFL 1.1,
license-text link to `res/fonts/CaskaydiaCove-NerdFont-LICENSE.md`, and the file
list `res/CaskaydiaCoveNerdFont-{Regular,Bold,Italic,BoldItalic}.ttf`. Remove
the now-stale "MesloLGS Nerd Font Mono" entry (`ATTRIBUTIONS.md:36-46`).

Deliverable: ATTRIBUTIONS.md lists CaskaydiaCove (OFL-1.1) and no longer lists
Meslo or Hack.

Verification: `markdownlint ATTRIBUTIONS.md`; manual read confirms license
class and reserved-name note are present.

Prohibitions: do NOT invent license terms — use OFL-1.1 as verified; do NOT
re-add Hack.

Stop: report the diff; await review.

#### 111.6 — Ligature smoke test against the bundled font

Scope: `freminal/src/gui/shaping.rs` (test module only), or a new test module
under `freminal/tests/` if integration-level.

What: Add a regression test proving the bundled default font now forms a
ligature when `ligatures = true`. Shape a known ligating sequence (e.g. `->`,
`=>`, `===`) with the bundled CaskaydiaCove face and assert the shaper emits a
single ligature glyph spanning multiple cells (the
`build_glyphs_two_char_ligature` / `_three_char_ligature` tests already define
the shape; this one must run against the _real bundled face_, not a synthetic
glyph-info fixture). This is the test that would have caught the Meslo
"feature-on-but-dead" bug.

Deliverable: a passing test that fails if the bundled font ever loses `calt`
(guards against a future accidental revert to a non-ligating bundled font).

Verification: `cargo test --all`; the new test passes and is named so its intent
is obvious (e.g. `bundled_font_forms_ligatures`).

Prohibitions: do NOT mock the font — load the actual bundled face; do NOT assert
on a specific glyph ID (those are font-version-specific) — assert on
cell-spanning ligature formation.

Stop: report the test name + result; await review.

#### 111.7 — Annotate the bundled default in the font picker + disambiguate a system duplicate

Scope: `freminal/src/gui/settings.rs` (`show_font_tab` ~832-859, `DEFAULT_LABEL`
~830).

What: The font-family picker (`ComboBox` "font_family") lists a `None`-valued
"Default" entry followed by every installed monospace family from
`self.monospace_families`. Two labeling changes:

1. **Annotate the bundled default.** The default entry's label gains an explicit
   bundled/default annotation, e.g. `CaskaydiaCove Nerd Font (default —
bundled)`. (After 111.2 the `DEFAULT_LABEL` already names CaskaydiaCove; this
   adds the "bundled/default" flag so the user knows it ships with Freminal.)
2. **Disambiguate a system duplicate.** If the user **also** has a font with the
   same family name installed (i.e. the bundled font's family name appears in
   `self.monospace_families`), that installed entry is annotated to
   differentiate it from the bundled default, e.g.
   `CaskaydiaCove Nerd Font (system)`. The two entries remain distinct
   selectable items: choosing the annotated default keeps `font.family = None`
   (use bundled); choosing the "(system)" entry sets
   `font.family = Some("CaskaydiaCove Nerd Font")` (use the system copy via the
   normal user-font path). The annotation is **display-only** — it must NOT be
   written into the persisted `font.family` value.

Match the bundled family name against the installed list using the same
whitespace-stripped comparison the font manager already uses for naming
variations (`font_manager.rs` "Caskaydia Cove" vs "CaskaydiaCove"); the bundled
family name should come from a single source of truth (a const/accessor, not a
duplicated string literal).

Deliverable: the picker shows the annotated bundled default always, and the
"(system)" annotation only when a same-named font is installed; selecting either
behaves as described. Tests: a unit test for the label-construction logic
covering (a) no system duplicate present and (b) system duplicate present,
asserting the produced labels and that the persisted `family` value is `None`
for the default and `Some(name)` for the system entry.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features --
-D warnings`; manual: open Font settings with and without the font installed
system-wide, confirm annotations.

Prohibitions: do NOT bake the annotation text into `font.family`; do NOT
hard-code the bundled family name in multiple places (single source of truth);
do NOT change the user-font loading path — this is picker labeling only.

Stop: report the label logic + test results + manual check; await review.

### Task 111 follow-up fixes

These fixes landed on `task-111/bundled-font` after the seven planned
subtasks. The CaskaydiaCove cell-metric change (111.2) altered cell height,
which changed how a pair of pre-existing emoji bugs manifested visually —
making them newly visible and worth fixing here alongside the font work.

#### 111.8 — Color-emoji sizing + premultiplied alpha (DONE)

Scope: `freminal/src/gui/atlas.rs`, `freminal/src/gui/renderer/vertex.rs`,
`freminal/src/gui/renderer/shaders/fg.frag`.

Two latent bugs in the color-emoji render path:

1. Color emoji were rasterised at their native bitmap-strike size (swash
   `StrikeWith::BestFit` does not downscale to the requested ppem) and the
   cell-boundary code _cropped_ rather than _scaled_ them, so emoji rendered
   oversized, clipped, and blurry. `emit_glyph_instance` now takes a dedicated
   color-glyph branch (`fit_color_glyph_rect`) that scales the glyph to fit the
   cell height with a 12% margin, preserves aspect ratio, and centres it in its
   advance box. Monochrome glyphs keep the existing crop-clip path.
2. swash returns straight (non-premultiplied) RGBA for color emoji, but the
   shader and egui's GL blend state both assume premultiplied alpha, leaving a
   white fringe. RGBA is now premultiplied on the CPU in `rasterize_glyph`
   (`premultiply_rgba_in_place`).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features --
-D warnings`; `cargo machete`; `bench_fg_instances` before/after (no change).
Nine new unit tests.

#### 111.9 — Insert-mode (IRM) wide-character cursor drift

Scope: `freminal-terminal-emulator/src/terminal_handler/mod.rs`
(`insert_text_irm_aware`, ~590).

Long-standing, pre-existing bug (reproduces off-branch). When insert mode (IRM,
`CSI 4 h`) is active — which shell line editors enable while editing the command
line — `insert_text_irm_aware` opens exactly **one** column per character via
`insert_spaces(1)` before writing the glyph. A double-width character (emoji,
CJK) needs **two** columns opened, so the shift is off by one per wide
character: a stray blank cell is left in front of the glyph, it overwrites part
of the prompt, and the cursor drifts. The fix opens `ch.display_width().max(1)`
columns instead of 1.

Deliverable: inserting a wide character in IRM mode shifts existing content by
the character's display width, not by 1. Regression test feeding a wide char in
insert mode and asserting the resulting buffer layout + cursor column.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features --
-D warnings`.

Prohibitions: do NOT change the non-insert path; do NOT alter `insert_spaces`
semantics (it correctly opens `n` columns) — the bug is the caller passing 1.

#### 111.10 — Backspace over a wide glyph drifts the cursor

Scope: `freminal-buffer/src/buffer/lines.rs` (`handle_backspace`, ~43).

Long-standing, pre-existing bug (reproduces off-branch; root cause of the
emoji paste / prompt-redraw cursor drift, confirmed via an FREC recording of
zsh). BS (cursor backward, `\x08`) must move the cursor **exactly one column**.
`handle_backspace` moved one column and **then** skipped left over any wide-glyph
continuation cells. Applications cross a double-width glyph by emitting **two**
backspaces (xterm/VT behaviour); the continuation-skip consumed both as a single
move, drifting the cursor by one per wide glyph. zsh redrawing a pasted emoji
(`\x08\x08` then rewrite) therefore landed one cell off, leaving a stray blank
cell and corrupting the prompt.

Fix: remove the continuation-skip loop. The cursor may legitimately land on a
continuation cell; the next write triggers the existing wide-overwrite cleanup
at that position.

Deliverable: one BS over a wide glyph lands on its continuation cell, a second
on its head. Regression test updated (`backspace_moves_one_column_over_wide_glyph`,
formerly `backspace_jumps_wide_glyph`, which asserted the buggy skip).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features --
-D warnings`.

Prohibitions: do NOT reintroduce the continuation skip; do NOT change the
reverse-wrap or pending-wrap branches.

---

## Task 112 — UI Beautification

Theme all non-terminal chrome to the active palette through one centralized
styling layer with selectable Modern / Retro profiles, and ship our own action
glyphs instead of relying on the system font. See the "egui capability audit"
and "Decision: centralized `ChromeStyle`…" sections above for the durable
rationale, and the crate-architecture decision in Design Decisions.

The work splits into four phases: **data type** (112.2, in `freminal-common`) →
**egui mapping + preview + application** (112.3, 112.3a, 112.4-112.5, in the
binary) → **per-surface adoption** (112.6-112.9) → **icons** (112.10-112.12).
An icon-asset audit (112.1) precedes everything. A dedicated preview-gallery
subtask (112.3a) — shipped as a standalone `cargo` example binary, **not** an
in-product settings tab — sits between the mapping and its global application so
the Modern/Retro geometry can be tweaked and the visual vision locked in against
the real `build_visuals` mapping **before** the profiles are applied app-wide and
rolled out per surface. Each subtask leaves `cargo test --all` green.

Sequencing: 112.1 ∥ 112.2 first (independent); 112.3 needs 112.2; 112.3a needs
112.3; 112.4-112.5 need 112.3a (the geometry baselines locked in the gallery
feed 112.4's global application); 112.6-112.9 need 112.5; 112.10 needs 112.1;
112.11-112.12 need 112.10.

### Task 112 subtasks

#### 112.1 — READ-ONLY audit: action-glyph inventory + asset-format decision

Scope: read-only. No file changes. Produces a findings section appended to this
plan doc under "Task 112 audit results".

What: Enumerate every action symbol the chrome currently draws via a font glyph
(close `×`, recording indicator, broadcast-keyboard indicator, lock, tab `+`,
menu chevrons, any powerline/emoji glyph in `menu.rs`, `toast.rs`,
`settings.rs`, guard dialogs). For each, record the current codepoint, where
it's drawn, and whether it renders on Linux (empty-square risk). Then decide the
bundled-icon asset format against the current renderer: SVG-via-`Painter`,
icon-font, or pre-rasterized PNG atlas. egui 0.34 has **no native SVG**, so this
must weigh `egui_extras`/`resvg` rasterization vs. an icon font vs. baking PNGs.
Recommend one with rationale.

Deliverable: a written inventory (symbol → location → codepoint → render risk)
and a recommended asset format with justification, appended to this plan.

Verification: N/A (audit). Maintainer signs off on the asset-format choice
before 112.10 begins.

Prohibitions: do NOT write code; do NOT pick the format unilaterally without
surfacing the tradeoffs.

Stop: post the inventory + recommendation; await maintainer decision.

##### Task 112 audit results (112.1)

> **STATUS: audit complete; asset format SIGNED OFF.** Read-only inventory; no
> code or assets changed. Maintainer approved the recommended icon-font /
> CaskaydiaCove NF direction (see "Recommended asset format" below); the icon
> track (112.10-112.12) is unblocked.

**Action-glyph inventory.** Every non-ASCII symbol the chrome renders through
egui's font pipeline, with codepoint, location, and Linux render risk. The five
HIGH-risk entries are color-emoji codepoints that fall through to tofu on a
Linux install lacking a system emoji font (the emoji fallback chain in
`fonts.rs` is system-sourced, **not** bundled — `fonts.rs:191-192`).

| Symbol           | Codepoint | Location                      | Represents                  | Linux risk |
| ---------------- | --------- | ----------------------------- | --------------------------- | ---------- |
| lock             | U+1F512   | `menu.rs:161` (menu-bar)      | echo-off indicator          | HIGH       |
| record dot + REC | U+25CF    | `menu.rs:169` (menu-bar)      | recording indicator         | LOW        |
| lock+key         | U+1F510   | `menu.rs:741-742` (tab label) | password-prompt tab         | HIGH       |
| bell             | U+1F514   | `menu.rs:743` (tab label)     | unacked bell on tab         | HIGH       |
| antenna          | U+1F4E1   | `menu.rs:750` (tab label)     | broadcast-input tab         | HIGH       |
| ellipsis         | U+2026    | `menu.rs:841`                 | "Rename Tab..." menu item   | LOW        |
| multiply         | U+00D7    | `menu.rs:852`                 | tab close button            | LOW        |
| multiply         | U+00D7    | `toast.rs:207`                | toast dismiss button        | LOW        |
| lock+key (text)  | U+1F510   | `settings.rs:1761`            | Security-tab description    | HIGH       |
| heavy minus      | U+2796    | `settings.rs:1844`            | remove paste-guard pattern  | MEDIUM     |
| heavy plus       | U+2795    | `settings.rs:1866`            | add paste-guard pattern     | MEDIUM     |
| warning          | U+26A0    | `settings.rs:2086`            | keybinding conflict warning | MEDIUM     |
| record (tech)    | U+23FA    | `settings.rs:2168`            | recording button label      | MEDIUM     |
| lock             | U+1F512   | `terminal/widget.rs:2488`     | in-pane echo-off overlay    | HIGH       |
| right triangle   | U+25B6    | `terminal/widget.rs:73`       | fold-placeholder prefix     | LOW        |
| ellipsis         | U+2026    | `terminal/widget.rs:93`       | fold-placeholder truncation | LOW        |
| middle dot       | U+00B7    | `close_guard.rs:256,282`      | hint/list separators        | LOW        |

Pure-ASCII button labels (`search.rs` `<`/`>`/`X`; `command_history.rs`
`OK`/`ER`/`..`/`??` status badges — already flagged in-code at
`command_history.rs:527-530` as awaiting icon replacement) carry no render risk
but are candidates for icon treatment for visual consistency. No actionable
glyphs in `broadcast_guard.rs`, `paste_guard.rs`, `welcome.rs`,
`notifications.rs`, `command_blocks.rs`.

**Renderer facts relevant to the format choice.** egui 0.34.3 has no native
SVG. No existing `egui_extras` / `RetainedImage` / egui `TextureHandle` usage in
the chrome. `image` (0.25) is already a dependency; background/inline images go
through `glow::Texture` in `renderer/gpu.rs` (not egui `TextureId`).
`egui::Image::tint(Color32)` works in 0.34.3, so a monochrome glyph/texture can
be tinted to the active palette. Task 111 bundled CaskaydiaCove NF (full Nerd
Font: Powerline + Material Design Icons + PUA sets) via `include_bytes!`, already
registered at position 0 of egui's `FontFamily::Monospace` (`fonts.rs:183`).

**Recommended asset format: bundled icon font — reuse the already-bundled
CaskaydiaCove Nerd Font.** Tradeoffs surfaced for sign-off:

- **Icon font / CaskaydiaCove (recommended).** Zero new crates, zero new assets
  for icons that have a Nerd Font equivalent (lock, bell, antenna, record,
  warning, plus/minus, arrows all exist in CaskaydiaCove's PUA/MDI ranges).
  Native tinting via `RichText::color()`. Consistent with the Task 111
  bundled-font precedent. Cost: a one-time codepoint-mapping pass (emoji
  codepoint → Nerd Font codepoint), and icons become monochrome (intended — it
  is what enables tinting). A supplemental <200 KB icon TTF is the fallback for
  any future symbol with no Nerd Font equivalent.
- **SVG via `egui_extras` + `resvg`/`usvg`.** Best DPI/scalability story and
  human-editable sources, but pulls a heavy transitive crate chain
  (`resvg`/`usvg`/`tiny-skia`/...) against the repo's `machete`/`deny`
  enforcement — not justified for ~15 icons.
- **Pre-rasterized PNG atlas.** No new crates (`image` already present), but
  opaque binary diffs, manual UV-coordinate maintenance, and multi-resolution
  atlases for HiDPI make it the highest-maintenance option.

**Decision (maintainer, signed off):** the icon-font / CaskaydiaCove NF
direction is approved. 112.10 implements the bundled-icon pipeline as an icon
font reusing the already-bundled CaskaydiaCove faces (supplemental <200 KB icon
TTF only if a future symbol has no Nerd Font equivalent). The icon-track
subtasks (112.10-112.12) are unblocked; the styling track (112.2 → 112.3 →
112.3a → 112.4-112.9) was never blocked on this.

#### 112.2 — Add `GuiTheme` + `StyleProfile` to `freminal-common` (data type only)

> **STATUS: COMPLETE.** `freminal-common/src/gui_theme.rs` adds `StyleProfile`
> (`Modern`/`Retro`, serde `lowercase`) and `GuiTheme`, re-exported from
> `lib.rs`. Fields: `profile`, `corner_radius`, `stroke_width`, `item_spacing`,
> `window_padding`, plus two judged-minimal extras — `menu_corner_radius` (nested
> menus warrant a distinct radius) and `widget_hover_expansion` (hover geometry
> differs sharply between profiles). No egui, no colors, no `Config` wiring.
> Baselines (starting points, to be tuned in 112.3a): Modern = radius 6 / stroke
> 1.0 / spacing (8,4) / padding 8 / menu-radius 4 / hover 2.0; Retro = radius 0 /
> stroke 1.5 / spacing (6,2) / padding 4 / menu-radius 0 / hover 0.0. 12 unit
> tests (defaults, `Default`, TOML round-trip, lowercase serde) pass;
> `cargo clippy -p freminal-common --all-targets --all-features -- -D warnings`
> clean.

Scope: `freminal-common/src/` (new module, e.g. `gui_theme.rs`, plus `lib.rs`
re-export). NO egui. NO config wiring yet (that is 112.13).

What: Define a toolkit-agnostic `GuiTheme` struct and `StyleProfile` enum,
serde-derivable, zero external GUI deps:

```text
pub enum StyleProfile { Modern, Retro }   // #[serde(rename_all = "lowercase")]

pub struct GuiTheme {
    pub profile: StyleProfile,
    pub corner_radius: u8,        // px; Modern default 6, Retro default 0
    pub stroke_width: f32,        // px; border weight
    pub item_spacing: (f32, f32), // x, y padding
    pub window_padding: f32,
    // … the minimal set the mapping in 112.3 needs; numbers/enums only
}
```

`StyleProfile` carries a `fn defaults(self) -> GuiTheme` returning the
profile's baseline geometry (Modern = rounded/soft, Retro = sharp/dense). All
fields are plain numbers/enums — colors are NOT here (they come from
`ThemePalette` at mapping time). Add `Default` (Modern), `Serialize`,
`Deserialize`, and unit tests for `defaults()` and serde round-trip. The
baseline numbers here are a working starting point; they are **empirically
dialed in and locked** in the 112.3a preview gallery against rendered chrome
before 112.4 applies them app-wide. If 112.3a is being done first to lock the
vision, land 112.2 with placeholder baselines and update them once 112.3a signs
off.

Deliverable: the type + tests in `freminal-common`; nothing consumes it yet.

Verification: `cargo test -p freminal-common`; `cargo clippy --all-targets
--all-features -- -D warnings`.

Prohibitions: do NOT add egui or any GUI crate to `freminal-common`'s
`Cargo.toml`; do NOT put `Color32` or palette colors in `GuiTheme`; do NOT wire
it into `Config` here.

Stop: report the type surface + test results; await review.

#### 112.3 — `chrome_style.rs`: map `(GuiTheme, ThemePalette) → egui::Visuals`

> **STATUS: COMPLETE.** `freminal/src/gui/chrome_style.rs` adds
> `pub fn build_visuals(gui_theme, palette, bg_opacity, normal_display) ->
egui::Visuals`. Starts from `Visuals::dark()` and overrides every listed
> field from palette + geometry: `window_fill`/`panel_fill` (preserving the
> reverse-video white-fill + opacity behavior verbatim from `app_impl.rs`),
> `window_stroke` (foreground, `stroke_width`), `window_corner_radius` /
> `menu_corner_radius` (from the respective `GuiTheme` radii), `selection`
> (palette `selection_bg`/`_fg`), `override_text_color` (foreground), and all
> five `widgets` states via a private `widget()` helper — noninteractive/inactive
> use background/`ansi[0]` fills with `expansion 0.0`; hovered/active use
> `selection_bg` (active at 80% alpha) with `expansion = widget_hover_expansion`;
> open uses `ansi[8]`. `colors::rgb_to_color32` was promoted private → `pub(crate)`
> for reuse. Nothing calls it yet (112.4 wires it). 13 unit tests (Retro→radius 0,
> Modern→nonzero, selection/text-color mapping, reverse-video white, opacity→alpha,
> per-state expansion) pass; `cargo clippy -p freminal --all-targets
--all-features -- -D warnings` clean. Test palette: `CATPPUCCIN_MOCHA`.

Scope: `freminal/src/gui/chrome_style.rs` (new), `freminal/src/gui/mod.rs`
(module decl). May read `colors.rs` helpers.

What: Implement the centralized mapping `pub fn build_visuals(gui_theme:
&GuiTheme, palette: &ThemePalette, bg_opacity: f32, normal_display: bool) ->
egui::Visuals`. Derive every chrome surface from the palette: `window_fill`,
`panel_fill` (opacity-aware, preserving current behavior), `window_stroke`,
`window_corner_radius`/`menu_corner_radius` (from `gui_theme.corner_radius`),
`selection` (from `palette.selection_bg`/`_fg`), `override_text_color`
(`palette.foreground`), and all five `widgets` states (`noninteractive`,
`inactive`, `hovered`, `active`, `open`) — each `WidgetVisuals` with `bg_fill`,
`weak_bg_fill`, `bg_stroke`, `fg_stroke`, `corner_radius`, `expansion` derived
from palette + profile geometry. Preserve the reverse-video (`!normal_display`)
white-fill behavior currently in `app_impl.rs:1072-1080`. Add a public
`palette_rgb_to_color32((u8,u8,u8)) -> Color32` helper (or reuse the existing
private `rgb_to_color32` by making it `pub(crate)`).

Deliverable: `build_visuals` + unit tests asserting key mappings (e.g. Retro
profile yields `corner_radius == 0`; selection color matches palette; reverse
video forces white). Nothing calls it yet (112.4 wires it).

Verification: `cargo test -p freminal`; `cargo clippy --all-targets
--all-features -- -D warnings`.

Prohibitions: do NOT apply the visuals to the context here (112.4); do NOT
hard-code any color — everything derives from the palette; do NOT touch the
terminal-buffer renderer.

Stop: report the function signature + which `Visuals` fields are set; await
review.

#### 112.3a — Style-profile preview gallery, as a standalone example binary (lock in the vision)

Scope: `freminal/examples/chrome_gallery.rs` (new **cargo example** — not
compiled into the product binary, not part of any release), plus the minimal
visibility promotion needed to reach the real mapping: in
`freminal/src/gui/mod.rs` promote `mod chrome_style` from `pub(crate)` to
`pub`, and in `freminal/src/gui/colors.rs` promote `rgb_to_color32` from
`pub(crate)` to `pub` (it is called transitively by `build_visuals`). Remove
the `#[allow(dead_code)]` / `TODO(112.4)` on `build_visuals` once the example is
its first real caller. May add `[dev-dependencies]` to `freminal/Cargo.toml`
**only** if strictly required (the example should need none — see below).

**Why an example binary, not a settings tab (decided at activation).** The
tuning gallery is a developer instrument used to dial in the `StyleProfile`
geometry and lock the aesthetic; it is used during this implementation and the
occasional future revision, never by end users. Baking it into the product's
`SettingsModal` would permanently couple a throwaway dev surface to the shipping
GUI's tab structure and force it to keep compiling as the settings UI evolves. A
`cargo run --example chrome_gallery` binary lives beside the code (stays
compilable, revisable) but is structurally outside the product — cargo examples
are not built into the release. The **user-facing** live preview (a small
themed-chrome swatch next to the profile picker) is a separate, deliberately
minimal concern and belongs to **112.7's** settings work, not here.

**Bootstrap (verified at activation).** `eframe` is gone; the app uses the
`freminal-windowing` crate, which exposes a reusable
`freminal_windowing::run(WindowConfig, impl App)` entry point. The example
implements the `App` trait (`update`, `on_window_created`, `on_close_requested`
→ `true`, `clear_color`) in ~35 lines, ignoring the `gl` parameter (no terminal
renderer involved), and renders the gallery via
`egui::CentralPanel::default().show(ctx, |ui| { … })`. `freminal-windowing`,
`egui`, and `egui_glow` are already direct deps of `freminal`, so an example
inherits them with **no new dependencies**.

What: This is the subtask where the Modern/Retro aesthetic is **tweaked and
locked in against the real `build_visuals` mapping** before it is committed
app-wide. The example must call the **real**
`freminal::gui::chrome_style::build_visuals(&gui_theme_draft, palette, opacity,
normal_display)` (this is why the visibility promotion is required) — it must
**not** reimplement or copy the function body, or it would be tuning a divergent
copy. Render a representative slice of chrome inside a region whose `Visuals`
come from that call (set the example window's egui style, or a locally-scoped
`ui.scope` / child `Ui` with `*ui.visuals_mut() = visuals`). The gallery must
show, at minimum:

- a button row (normal + a primary/accent button),
- a `ComboBox` and a `Slider` (the two most common settings widgets),
- a separator,
- an active-tab / inactive-tab pair (the tab-bar look),
- a sample bordered panel/window frame (corner radius + stroke visible),
- a sample toast frame (the only current `Stroke`+`CornerRadius` user),
- a line of selected text (selection bg/fg).

Alongside the gallery, expose **live controls** bound to an in-memory `GuiTheme`
draft: a `StyleProfile` selector (Modern/Retro — switching it resets the draft
to that profile's `defaults()`), sliders for the tunable geometry
(`corner_radius`, `stroke_width`, `item_spacing`, `window_padding`,
`menu_corner_radius`, `widget_hover_expansion`), an opacity slider, a
normal/reverse-video toggle, and a `ThemePalette` selector (pick from
`freminal_common::themes` so the look can be checked against a light and a dark
palette). Changing any control re-derives the `Visuals` next frame. This is the
surface the maintainer uses to dial in the profile baseline numbers — the values
that become `StyleProfile::defaults()` in 112.2 — and to sign off the look
before 112.4 applies it everywhere.

Deliverable: a runnable `cargo run -p freminal --example chrome_gallery` that
renders the gallery, restyled live by the real `build_visuals` against a local
`GuiTheme` draft, with the controls above — usable to lock in the profile
baselines. No unit test is required for an example binary (it is a dev tool,
exercised manually); the `build_visuals` mapping it drives is already unit-tested
in 112.3. The visibility promotions must keep `cargo test --all` and clippy
green.

Verification: `cargo run -p freminal --example chrome_gallery` launches and the
gallery restyles live as controls change; `cargo build -p freminal` (confirm the
example does not break the normal build and is not pulled into it);
`cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`
(examples are linted under `--all-targets`).

Prohibitions: do NOT reimplement or copy the `build_visuals` body — call the
real one (promote visibility instead); do NOT add the gallery to `SettingsModal`
/ `SettingsTab` (that is the rejected settings-tab approach; the user preview is
112.7); do NOT persist anything to `Config` or touch the snapshot/PTY path; do
NOT hard-code colors — the gallery's colors derive from the selected `palette`
via `build_visuals`; do NOT touch the terminal-buffer renderer; do NOT add a new
crate dependency (the example needs none).

Outcome to capture: once the maintainer signs off, record the locked-in
Modern/Retro geometry baselines (the numbers that land in 112.2's
`StyleProfile::defaults()`) in this plan under a short "112.3a — locked
baselines" note, so 112.4's global application uses the agreed values. If the
locked numbers differ from 112.2's placeholders, 112.2's `defaults()` is updated
as part of this subtask's sign-off.

Stop: report the gallery contents, the run command, the (proposed or locked)
geometry baselines, and the build/clippy results; await the maintainer's
sign-off on the look before 112.4.

#### 112.4 — Apply full `Visuals` via the per-frame style hook

Scope: `freminal/src/gui/rendering.rs` (`set_egui_options`,
`update_egui_theme`), `freminal/src/gui/app_impl.rs` (per-frame style-cache
~1042-1083), `freminal/src/gui/window.rs` (the `style_cache` field type ~49).

What: Replace the two-field (`window_fill`/`panel_fill`) style application with
a call to `chrome_style::build_visuals(...)` applied via
`ctx.set_visuals(...)`/`global_style_mut`. Extend the `style_cache` key from
`(bool, &'static ThemePalette, f32)` to also include the active `GuiTheme`
(profile + geometry) so a profile change invalidates the cache. Wire the active
`GuiTheme` in from config (read alongside the theme in `app_impl`). Keep the
pointer-identity theme comparison; add equality on `GuiTheme`.

Deliverable: all chrome now renders with palette-derived visuals in both
profiles; switching theme live re-themes chrome (the existing
`PreviewTheme`→snapshot path now also moves chrome colors).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features --
-D warnings`; manual: launch, switch themes in settings, confirm menu/tab/modal
colors follow.

Prohibitions: do NOT scatter styling into individual widgets (that is
112.6-112.9 consuming this centralized style); do NOT regress the
`background_opacity` behavior (panel_fill stays opacity-aware, window_fill
opaque).

Stop: report cache-key change + manual re-theming result; await review.

#### 112.5 — Performance: before/after frame-time capture

Scope: benchmark capture only (per `performance-benchmarks` +
`freminal-bench-table`). No production code change beyond what 112.4 landed.

What: The per-frame style application + `Visuals` rebuild touches the render
hot path. Capture the relevant benchmark IDs before (pre-112.4 baseline) and
after, confirm < 15% regression. If `build_visuals` is called every frame,
verify the `style_cache` actually short-circuits the rebuild on the steady-state
(unchanged theme/profile) path — a cache miss every frame would be the
regression.

Deliverable: recorded before/after numbers in the prescribed format; confirmed
within threshold (or a fix to the cache if not).

Verification: `cargo bench` on the named IDs; numbers recorded.

Prohibitions: do NOT skip this — chrome styling runs per frame; do NOT accept a

> 15% regression without a documented justification.

Stop: report before/after numbers; await review.

#### 112.6 — Adopt centralized style: menu bar + tab bar

Scope: `freminal/src/gui/menu.rs`.

What: Remove any ad-hoc per-widget fills/strokes/corner radii in the menu bar
and tab bar; let them inherit the centralized `Visuals` from 112.4. Where the
tab bar sets explicit `Frame` fills/`corner_radius` (`menu.rs:760-777`) and
selectable-button styling, replace with palette/profile-derived values from
`chrome_style` (e.g. active-tab fill = `palette.selection_bg`, inactive =
`weak_bg_fill`). Ensure the active-tab highlight still reads correctly in both
profiles.

Deliverable: menu + tab bar visually consistent with the active theme and
profile, no hard-coded colors.

Verification: `cargo test --all`; `cargo clippy …`; manual: tabs + menu in
Modern and Retro, in a light and a dark theme.

Prohibitions: do NOT introduce a second source of truth for colors; do NOT
break tab-rename `TextEdit` focus (`freminal-modal-input-suppression`).

Stop: report changes + manual check; await review.

#### 112.7 — Adopt centralized style: settings modal + standalone settings window

Scope: `freminal/src/gui/settings.rs`.

What: Replace the bespoke `opaque_frame` (`settings.rs:603-608`) and any inline
styling with the centralized `chrome_style` surface, keeping the
"settings stays opaque regardless of background_opacity" guarantee. The settings
tab bar, combo boxes, sliders, separators inherit the themed `Visuals`. Add the
**style-profile picker control** here (a `Modern`/`Retro` selector) and a live
preview — selecting a profile re-themes immediately via the 112.4 hook (parallel
to the existing live theme preview).

Deliverable: settings UI fully themed in both profiles; profile picker present
and live.

Verification: `cargo test --all`; `cargo clippy …`; manual: open settings,
switch profile, confirm immediate restyle; confirm settings opacity guarantee
holds.

Prohibitions: do NOT persist the profile here (112.13 handles config wiring);
the picker edits the in-memory draft + live preview only at this stage; do NOT
break settings-field focus rules.

Stop: report changes + manual check; await review.

#### 112.8 — Adopt centralized style: overlays (context menu, command history, search, toasts, welcome)

Scope: `freminal/src/gui/terminal/widget.rs` (context menu),
`freminal/src/gui/command_history.rs`, `freminal/src/gui/search.rs`,
`freminal/src/gui/toast.rs`, `freminal/src/gui/welcome.rs`.

What: Route each overlay's `Frame`/`Area` styling through `chrome_style`. The
toast `Frame` (`toast.rs:175-179`, currently the only `Stroke`+`CornerRadius`
user) derives its fill/stroke/corner radius from palette + profile. The
command-history and search popups, the right-click context menu, and the welcome
overlay inherit themed `Visuals`.

Deliverable: all listed overlays themed and profile-aware.

Verification: `cargo test --all`; `cargo clippy …`; manual: trigger each overlay
in both profiles + a light/dark theme.

Prohibitions: do NOT change overlay _behavior_ (positioning, dismissal, focus);
styling only; honor `freminal-modal-input-suppression` for the focusable ones.

Stop: report changes + manual check; await review.

#### 112.9 — Adopt centralized style: guard + info dialogs (broadcast, close, paste, About, save-layout, unsaved-changes)

Scope: `freminal/src/gui/broadcast_guard.rs`, `freminal/src/gui/close_guard.rs`,
`freminal/src/gui/paste_guard.rs`, `freminal/src/gui/menu.rs` (About,
Save-Layout), `freminal/src/gui/settings.rs` (unsaved-changes dialog).

What: Route each `egui::Window` dialog through the centralized themed visuals so
buttons, frames, and text follow palette + profile. These all already use
`egui::Window`, so most inherit automatically once 112.4 lands; this subtask
audits each and removes any remaining default-styled outliers and confirms each
honors the focus rules.

Deliverable: every guard/info dialog themed and profile-aware.

Verification: `cargo test --all`; `cargo clippy …`; manual: trigger each dialog
in both profiles.

Prohibitions: do NOT alter dialog logic; styling + focus-rule confirmation only.

Stop: report per-dialog status; await review.

#### 112.10 — Build the bundled-icon asset pipeline (format from 112.1)

Scope: depends on the 112.1 decision; new asset files under `res/` or
`assets/`, a new `freminal/src/gui/icons.rs` loader. Touches `Cargo.toml` only
if the chosen format needs a crate (e.g. `egui_extras`/`resvg`) — if so, follow
`flake-dev-shell-discipline` / `rust-best-practices` dependency rules.

What: Implement the chosen asset pipeline: bundle the action-glyph assets and
expose a typed accessor (e.g. `enum ChromeIcon { Close, Recording, Broadcast,
Lock, AddTab, … }` → texture/mesh) so chrome code requests an icon by name, not
a codepoint. Icons tint to the active palette where appropriate.

Deliverable: an icon loader + bundled assets; a test that every `ChromeIcon`
variant resolves to a loadable asset.

Verification: `cargo test --all`; `cargo clippy …`.

Prohibitions: do NOT proceed until 112.1's format is signed off; do NOT keep any
fallible system-font glyph for our own UI symbols.

Stop: report the icon set + loader API; await review.

#### 112.11 — Replace font-glyph action symbols with bundled icons

Scope: the chrome files identified in 112.1 (`menu.rs`, `toast.rs`,
`settings.rs`, guard dialogs as applicable).

What: Swap each font-glyph action symbol for the corresponding `ChromeIcon`
from 112.10. The close `×`, recording/broadcast/lock indicators, tab `+`, etc.
now render from bundled assets identically on every platform.

Deliverable: no chrome action symbol depends on a system/emoji/powerline font
glyph.

Verification: `cargo test --all`; `cargo clippy …`; manual on Linux (the
known empty-square platform) confirms every symbol renders.

Prohibitions: do NOT leave a single action glyph on the font path; do NOT change
the symbols' behavior or hit-targets.

Stop: report which symbols were swapped + Linux render confirmation; await
review.

#### 112.12 — Icon regression test

Scope: a test module (location per 112.10's loader).

What: Add a test asserting every `ChromeIcon` variant loads and (if the format
supports it) has nonzero dimensions — guarding against a future asset deletion
or rename silently reintroducing the empty-square failure mode this version
exists to kill.

Deliverable: passing test over all `ChromeIcon` variants.

Verification: `cargo test --all`.

Prohibitions: do NOT assert on pixel content (brittle); assert on
load-success + dimensions.

Stop: report test name + result; await review.

#### 112.13 — Config wiring for `[chrome]` profile (full `freminal-config-options` ritual)

Scope: `freminal-common/src/config.rs`, `config_example.toml`,
`freminal/src/gui/settings.rs` + `settings_dispatch.rs`,
`nix/home-manager-module.nix`. (Whole-new-section, so all four merge-wiring
steps apply.)

What: Add a `[chrome]` section to `Config` carrying the `GuiTheme` profile (and
any user-overridable geometry decided in 112.2). Per `freminal-config-options`:
add `pub chrome: ChromeConfig` to `Config` + `Default`; add `Option<ChromeConfig>`
to `ConfigPartial`; add the `apply_partial` merge arm; update the
`every_config_section_survives_partial_merge` guard test (mutation + assertion +
exhaustive destructure entry). Document `[chrome] profile = "modern"` (and
geometry keys) in `config_example.toml`. Persist the 112.7 profile picker
through Apply (wire `settings_dispatch.rs`). Mirror the section in the Nix
home-manager module (the three edits). Add a round-trip test.

Deliverable: `[chrome] profile = "retro"` in a user config takes effect and
persists; the guard test covers it; Nix module exposes it.

Verification: `cargo test --all` (incl. the guard test + new round-trip test);
`cargo clippy …`; `nixfmt --check && statix check && deadnix --fail` on the Nix
module; manual: set profile in TOML, launch, confirm it applies.

Prohibitions: do NOT silence the guard test's `E0027` with `field: _` — wire
`ConfigPartial`/`apply_partial` properly; do NOT skip the Nix mirror or the
Settings persistence without surfacing it.

Stop: report all four wiring steps + the guard-test diff + Nix lint results;
await review.

---

## Task 113 — Resize / Reflow Scroll Corruption

> **STATUS: COMPLETE.** Decomposed into three subtasks (113.1–113.3). 113.1 is
> the root-cause fix and the priority; 113.2 and 113.3 are independent
> amplifiers found alongside it. Each subtask leaves `cargo test --all` green.
>
> - **113.1 — COMPLETE.** Grow path no longer appends an unreclaimable blank
>   tail below the live cursor; it reclaims trailing pristine `ScrollFill`
>   padding instead, pinning the live cursor to the bottom of the (taller)
>   window and revealing real scrollback above. Bug G smoke test passes and is
>   kept uncommented. Added a `buffer_resize/grow_height` benchmark (no
>   regression: p = 0.08, "No change").
> - **113.2 — COMPLETE.** `reflow_to_width` now remaps `command_blocks` and
>   `prompt_rows` to their post-reflow row indices via a new
>   `remap_block_rows_after_reflow` helper (start fields anchor to the old
>   row's first cell, `end_row` to its last cell, matching the inclusive
>   `[output_start_row, end_row]` range the GUI consumes). Bug R smoke test
>   plus a multi-block/both-directions test pass and are kept uncommented.
>   `reflow_width` / `softwrap_heavy` benches within noise (< 15%).
> - **113.3 — COMPLETE.** `handle_incoming_data` now calls `reset_scroll_offset()`
>   (clearing both `gui_scroll_offset` and `gui_extra_rows`) instead of inlining
>   only the offset reset, and runs that reset only when either is non-zero so
>   output at the live bottom does not needlessly invalidate the snapshot cache.
>   Bug E smoke test plus two unit tests pass and are kept uncommented.
>   `bench_handle_incoming_data` no regression (p = 0.31, "No change").

### Background — the reported bug

A maintainer reported an intermittent, hard-to-reproduce buffer-scroll
corruption with these symptoms:

1. There is scrollback present.
2. The active prompt has scrolled to the top of the window even though it
   should not be there.
3. Sometimes the prompt is visible, sometimes it is itself in the scrollback
   and nothing is visible. Typing a command still runs it, but the output is
   missing (it lands in the scrollback).
4. **No amount of scrolling surfaces the missing content.**
5. Typing `clear` recovers the terminal.

One reporter could reproduce it semi-reliably on macOS while changing font
settings; the maintainer hits it on Hyprland (a tiling WM) **without** touching
fonts, and it predates the command-block feature (Task 72). The font-change
correlation is therefore a red herring: changing font size triggers a terminal
resize (cell size changes → row/col count changes), and a tiling WM streams many
resize events during interactive window manipulation. The common factor is
**repeated resizes**, not fonts.

### Root cause — Bug G (the real bug; predates command blocks)

`Buffer::resize_height` grow branch
(`freminal-buffer/src/buffer/resize_and_alt.rs`, ~line 490):

```rust
if new_height > old_height {
    let grow = new_height - old_height;
    for _ in 0..grow {
        self.rows.push(Row::new(self.width)); // append blank rows at the BOTTOM
        self.row_cache.push(None);
    }
    ...
}
```

When the window grows, this appends `new_height - old_height` blank rows **at
the bottom** of the buffer, leaving the live cursor's absolute `pos.y`
unchanged. The shrink branch (same file, ~line 530) deliberately **retains**
rows as scrollback and does not move the cursor. So the appended blanks are
never reclaimed.

Across a sequence of grow/shrink cycles — exactly what an interactive tiling-WM
resize, or repeated font-size changes, produces — the buffer accumulates an
unbounded tail of blank rows **below** the live cursor. The visible window
(anchored to the bottom of the buffer via `visible_window_start`) then shows
only that blank tail, while the live prompt/output is stranded near the **top**
of the buffer, in scrollback. Because `scroll_offset` and `max_scroll_offset`
are computed correctly from `rows.len()`, scrolling cannot recover it — the
corruption is in the row layout, not the offset. `clear` recovers because it
erases scrollback and re-homes the cursor.

This was confirmed with a failing test that drives a height-only resize cycle
(`[24, 30, 18, 40, 12, 50, 20, 24]`) and observes the live cursor escape the
visible window with ~70 blank rows below it. (The test is checked in,
commented out — see "Smoke tests" below.)

### Amplifier — Bug R (reflow does not remap command blocks / prompt rows)

`Buffer::reflow_to_width` (`resize_and_alt.rs`, ~line 263) rebuilds every row
from scratch on a **width** change (which a font-size change, or a tiling-WM
column-count change, triggers). It correctly remaps the cursor by flat-offset,
but it does **not** update `self.command_blocks` or `self.prompt_rows`, whose
row fields (`prompt_start_row`, `command_start_row`, `output_start_row`,
`end_row`, and each `prompt_rows` entry) are **buffer-absolute** indices. After
a reflow that changes the row count, those indices point at the wrong rows.

The trim path (`enforce_scrollback_limit` → `adjust_prompt_rows`,
`lifecycle.rs:132`) already does the equivalent remap when rows are dropped from
the front; reflow simply lacks it. Stale block rows corrupt the command-block
gutter and folding (`compute_fold_ranges` / `compute_extra_rows` /
`FoldLayout`), which can push content off-screen incorrectly when a fold is in
view — compounding Bug G's visual damage. Command blocks are **on by default**
(`CommandBlocksConfig::default().enabled == true`), so this fires in normal use
on any width-changing resize.

Confirmed with a failing test: a block ending at row 4 at width 20 still reports
`end_row == Some(4)` after reflowing to width 10, even though its last content
row moved to row 9.

### Amplifier — Bug E (new data clears scroll offset but not fold extra-rows)

`TerminalEmulator::handle_incoming_data`
(`freminal-terminal-emulator/src/interface.rs`, ~line 385) snaps to the live
bottom on new output by resetting `self.gui_scroll_offset = 0`, but leaves
`self.gui_extra_rows` stale. A dedicated `reset_scroll_offset()` (same file,
~line 507) clears **both** — `handle_incoming_data` should call it instead of
inlining only the offset reset. While a command-block fold is in view, the stale
`gui_extra_rows` keeps the flattened window extended above the live bottom
against a buffer that no longer has a fold there.

On its own (no fold active) the GUI's `render_skip` absorbs the stale extra
rows, so it self-heals; combined with a fold and Bug R's wrong block rows it
contributes to the mis-anchored window. It is still a genuine correctness bug.

Confirmed with a failing test: after `set_gui_scroll_window(10, 5)` then new
output, `scroll_offset` correctly resets to 0 but `window_extra_rows` stays 5.

### Smoke tests (already checked in, commented out)

Three failing reproductions are committed alongside this task as **commented-out
tests**, so the implementing agent has a built-in benchmark. Uncomment, run,
watch them fail, implement the fix, watch them pass, then **keep them
uncommented** as permanent regression coverage.

- Bug G + Bug R: `freminal-buffer/src/buffer/mod.rs`, module `task_113_smoke`
  (`task_113_repeated_resize_does_not_strand_live_content`,
  `task_113_reflow_remaps_command_block_rows`).
  Run: `cargo test -p freminal-buffer --lib task_113_smoke`.
- Bug E: `freminal-terminal-emulator/tests/snapshot_build.rs`, function
  `task_113_new_data_resets_extra_rows`.
  Run: `cargo test -p freminal-terminal-emulator --test snapshot_build task_113_new_data_resets_extra_rows`.

These tests are the acceptance criteria: a subtask is not done until its smoke
test is uncommented and passing.

Sequencing: 113.1 → 113.2 → 113.3 (independent; may be done in any order, but
113.1 is the priority because it is the user-visible root cause). Each subtask
leaves `cargo test --all` green.

### Task 113 subtasks

#### 113.1 — Fix the grow path so it never strands live content (Bug G)

Scope: `freminal-buffer/src/buffer/resize_and_alt.rs` (`resize_height` grow
branch, ~line 490). May add a private helper. Test changes in
`freminal-buffer/src/buffer/mod.rs`.

What: On a **primary-buffer** height grow, the live content must stay pinned to
the bottom of the visible window. Instead of appending `grow` blank rows at the
bottom, the grow must **re-expose existing scrollback rows above the live
bottom** to fill the taller window, and append blank rows **only** when there is
not enough scrollback to fill it. Concretely, the post-grow invariant is:

- If `rows.len()` already provides at least `new_height` rows below the start of
  the live region, the window grows by revealing scrollback upward (no rows
  appended); the live cursor's absolute `pos.y` stays valid and the live bottom
  stays at the bottom of the window.
- Only when there are fewer than `new_height` total rows are blank rows appended
  — and the number appended is bounded so a grow followed by a shrink/grow cycle
  cannot accumulate an unbounded blank tail below the cursor.

The **alternate buffer** keeps its current top-anchored behavior (it has no
scrollback and the strict `rows.len() == height` invariant; do NOT change the
alternate branch). Re-validate `debug_assert_invariants()` after the change.

Think carefully about the interaction with the full-screen LF fast path
(`lines.rs:169`), which assumes the live cursor sits at `rows.len() - 1`. The
fix must not leave the cursor above the last row with blank rows below it, or LF
will walk the cursor down through those blanks instead of scrolling.

Deliverable: the grow path no longer appends an unreclaimable blank tail;
repeated resize cycles keep the live cursor in the visible window with no blank
tail below it.

Verification: uncomment and pass the `task_113_smoke` module's
`task_113_repeated_resize_does_not_strand_live_content` test (keep it
uncommented); `cargo test --all`; `cargo clippy --all-targets --all-features --
-D warnings`. Per `freminal-bench-table` / `performance-benchmarks`, resize is a
buffer operation — capture the relevant buffer/resize benchmark before/after and
confirm < 15% regression.

Prohibitions: do NOT change the alternate-buffer branch; do NOT change the
shrink branch's row-retention behavior (it is correct); do NOT delete the
committed smoke tests — uncomment them; do NOT "fix" the symptom in the GUI
(`visible_window_start` / `render_skip`) — the bug is the buffer appending
unreclaimable rows.

Stop: report the new grow logic, the smoke-test result, and the benchmark
before/after; await review.

**Implementation note (113.1, as landed).** The grow now appends **nothing** on
the primary buffer (rather than "append only when scrollback is insufficient").
Instead, `resize_height` calls a new private helper
`reclaim_trailing_blank_padding()` that pops trailing pristine `ScrollFill` rows
(empty cells) lying strictly below the live cursor, then leaves the buffer as
short as its content requires. The taller window reveals real scrollback above
via the existing bottom-anchored `visible_window_start`; when no scrollback
exists the buffer is left with `rows.len() < height` and the GUI pads the
remainder below the live bottom — exactly mirroring `Buffer::new`, which
deliberately starts with a single row, not `height` blank rows. This is a
cleaner equivalent of the plan's invariant: it satisfies "the live bottom stays
at the bottom of the window," and because no rows are ever appended below the
cursor, a grow/shrink cycle can never accumulate a blank tail. The screen
re-fills through the normal `handle_lf` row-push path, so the LF fast path is
never handed a cursor sitting above a blank tail. Two `tests_gui_resize` cases
(`resize_grow_adds_rows`, the `preserve_scrollback_anchor_*` clamp tests) encoded
the old append-at-bottom mechanics and were rewritten to assert the new contract
(cursor stays pinned / scrollback is revealed). A `buffer_resize/grow_height`
Criterion benchmark was added to cover the grow path (it had none); before/after
showed no significant change (p = 0.08).

#### 113.2 — Remap command blocks and prompt rows across reflow (Bug R)

Scope: `freminal-buffer/src/buffer/resize_and_alt.rs` (`reflow_to_width`, ~line
263). May add a private helper mirroring `adjust_prompt_rows`. Test changes in
`freminal-buffer/src/buffer/mod.rs`.

What: `reflow_to_width` already tracks, during logical-line grouping, where each
old row maps in the new layout (it uses this to remap the cursor by flat
offset). Extend that tracking to also remap the buffer-absolute row fields of
`self.command_blocks` (`prompt_start_row`, `command_start_row`,
`output_start_row`, `end_row`) and each entry of `self.prompt_rows` to their new
post-reflow row indices. The natural approach: record, per old logical line, the
index of its first new row, and translate each stored row index through that
map. Drop or clamp any block/prompt row that no longer maps onto a real row
(matching the spirit of `adjust_prompt_rows`, which drops fully-scrolled-out
blocks). Keep `command_blocks` ordering and the deque cap intact.

Deliverable: after a width reflow, every command block's row fields and every
prompt-row marker point at the row that actually holds their content.

Verification: uncomment and pass `task_113_reflow_remaps_command_block_rows`
(keep it uncommented); `cargo test --all`; `cargo clippy --all-targets
--all-features -- -D warnings`. Add at least one more test covering a
multi-block, multi-prompt reflow (wide → narrow AND narrow → wide) so the remap
is exercised in both directions.

Prohibitions: do NOT change the cursor remap (it is correct); do NOT leak parser
/ escape-sequence knowledge into the buffer (this is pure row-index arithmetic
on data the buffer already owns); do NOT delete the committed smoke test.

Stop: report the remap approach + test results; await review.

**Implementation note (113.2, as landed).** During logical-line grouping,
`reflow_to_width` now records `old_row_meta[r] = (logical_line_idx,
flat_offset_of_row_start, row_len)` for every old row, and `line_new_starts`
records where each logical line's re-wrapped rows begin in the new buffer.
After installing the new rows and remapping the cursor, a new private helper
`remap_block_rows_after_reflow` translates each stored row index through that
map: an old row's content offset is located in the new layout the same way the
cursor remap works. Start fields (`prompt_start_row`, `command_start_row`,
`output_start_row`, and each `prompt_rows` entry) anchor to the old row's
**first** cell; `end_row` anchors to its **last** cell, because the GUI treats
`[output_start_row, end_row]` as an inclusive span and a row that re-wraps into
several narrower rows must keep `end_row` on the final piece (verified against
the consumers in `gui/terminal/input.rs`). Blocks whose `prompt_start_row` no
longer maps are dropped (mirroring `adjust_prompt_rows`); optional later fields
that no longer map are cleared rather than left dangling. Block ordering and the
deque are untouched. Tests: the Bug R smoke test plus a new
`task_113_reflow_remaps_multiple_blocks_both_directions` (two blocks, narrow→wide
**and** wide→narrow, asserting in-range, ordered, non-overlapping block spans).
Benchmarks `buffer_resize/reflow_width` and `softwrap_heavy` showed no regression
beyond the bench's inherent run-to-run noise (same-code reruns swing ±3%; the
added work is O(blocks + prompts) against an O(total cells) reflow).

#### 113.3 — Clear fold extra-rows on new output (Bug E)

Scope: `freminal-terminal-emulator/src/interface.rs` (`handle_incoming_data`,
~line 385). Test changes in
`freminal-terminal-emulator/tests/snapshot_build.rs`.

What: In `handle_incoming_data`, replace the inline `if self.gui_scroll_offset >
0 { self.gui_scroll_offset = 0; }` reset with a call to the existing
`reset_scroll_offset()` (which clears both `gui_scroll_offset` and
`gui_extra_rows`). Preserve the "only reset when actually scrolled back" intent
if it matters for snapshot-cache invalidation — i.e. call `reset_scroll_offset()`
when either `gui_scroll_offset > 0` or `gui_extra_rows > 0`, so new output always
snaps fully to the live bottom without needlessly invalidating caches when
already at the bottom with no fold extension.

Deliverable: new PTY output clears both the scroll offset and the fold
extra-rows request.

Verification: uncomment and pass `task_113_new_data_resets_extra_rows` (keep it
uncommented); `cargo test --all`; `cargo clippy --all-targets --all-features --
-D warnings`.

Prohibitions: do NOT change the alternate-screen scroll handling; do NOT alter
`reset_scroll_offset`'s semantics — call it; do NOT delete the committed smoke
test.

Stop: report the one-line change + test result; await review.

**Implementation note (113.3, as landed).** `handle_incoming_data` now calls the
existing `reset_scroll_offset()` (which clears both `gui_scroll_offset` and
`gui_extra_rows`) in place of the old inline reset that zeroed only
`gui_scroll_offset`. The reset runs only when the GUI is scrolled back or a fold
has extended the window — that is, when either `gui_scroll_offset` or
`gui_extra_rows` is non-zero — so that ordinary output at the live bottom (the
common case, both already zero) does not call the reset and therefore does not
needlessly touch state or invalidate the snapshot cache. `reset_scroll_offset`'s
semantics were not changed. Tests: the Bug E smoke test
(`task_113_new_data_resets_extra_rows`, integration) plus two interface unit
tests — `handle_incoming_data_clears_extra_rows` (offset already zero but a fold
is in view) and `handle_incoming_data_resets_both_offset_and_extra_rows` (both
set) — all kept uncommented. `bench_handle_incoming_data` showed no regression
(p = 0.31, "No change in performance detected").
