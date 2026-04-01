# PLAN 34 — Window Background Opacity

## Status: Pending

---

## Overview

Add a configurable background opacity setting that makes the terminal window semi-transparent,
allowing the desktop or windows behind Freminal to show through. Only the background color is
affected — text, images, cursor, and other content remain fully opaque.

The setting applies to:

- The terminal area background (cells with `DefaultBackground`)
- The menu bar background

Non-default cell backgrounds (e.g. colored `ls` output, vim status bars) remain fully opaque.

**Dependencies:** None (Tasks 2 and 3, which define the config system and settings modal, are
already complete)
**Dependents:** None
**Primary crates:** `freminal-common` (config), `freminal` (GUI rendering)
**Estimated scope:** Low-medium (5 subtasks)

---

## Current State

### Config System

- `UiConfig` (`freminal-common/src/config.rs:175-180`) currently has a single field:
  `hide_menu_bar: bool`.
- `ConfigPartial` mirrors `Config` with `Option` wrappers for layered TOML merging.
- `Config::validate()` (line 279) checks font size, version, scrollback limits, and theme slug.
- `config_example.toml` documents all config options (154 lines).

### Rendering Pipeline — How Backgrounds Work

1. **Panel fills** (`gui/mod.rs:34-66`): `set_egui_options()` and `update_egui_theme()` set
   `window_fill` and `panel_fill` to the theme's `DefaultBackground` color via
   `internal_color_to_egui()`. These are fully opaque (`Color32::from_rgb` — no alpha channel).

2. **eframe setup** (`gui/mod.rs:801`): `NativeOptions::default()` — no transparency configured,
   no clear color override.

3. **OpenGL renderer** (`renderer.rs:1244-1266`): `DefaultBackground` cells are **skipped** —
   they produce no explicit quads. The theme's background color shows through from the egui
   panel fill beneath. Non-default background cells are drawn as explicit quads at full alpha.

4. **`rgb_to_f32()`** (`colors.rs:34`): Always returns `alpha = 1.0`.

5. **`internal_color_to_egui()`** (`colors.rs:79`): Returns `Color32::from_rgb()` — no alpha.

### Key Insight

Because `DefaultBackground` cells are already transparent (skipped in the renderer), making the
panel fill semi-transparent automatically makes the terminal background semi-transparent. No
shader changes are needed. Non-default cell backgrounds are drawn as explicit opaque quads on
top, so they remain solid.

### Settings Modal — UI Tab

`show_ui_tab()` in `settings.rs:479-491` currently has only a `hide_menu_bar` checkbox. The
opacity slider will be added here.

### Platform Transparency Support

- **Wayland:** Compositors natively support window transparency via the alpha channel of the
  surface buffer. Works out of the box when the toolkit signals transparency intent.
- **macOS:** Core Animation layers support transparent backgrounds natively.
- **Windows:** DWM supports transparent windows via `WS_EX_LAYERED` or DirectComposition.
- **X11:** Requires a running compositor (e.g. picom, compton, xcompmgr). Without a compositor,
  the transparent areas render as black. This is a well-known X11 limitation shared by all
  transparent terminal emulators (Alacritty, Kitty, WezTerm).

---

## Subtasks

---

### 34.1 — Add `background_opacity` to `UiConfig` and validation

**Status:** Pending
**Priority:** 1 — High
**Scope:** `freminal-common/src/config.rs`

**Details:**

1. Add `background_opacity: f32` to `UiConfig`:

   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   #[serde(default)]
   pub struct UiConfig {
       pub hide_menu_bar: bool,
       /// Background opacity (0.0 = fully transparent, 1.0 = fully opaque).
       /// Only affects the terminal and menu bar backgrounds; text and content
       /// remain fully opaque.
       pub background_opacity: f32,
   }
   ```

2. Update `UiConfig`'s `Default` impl (currently derived) to an explicit impl that sets
   `background_opacity: 1.0` (fully opaque — no visual change for existing users).

3. Add validation in `Config::validate()`:

   ```rust
   if !(0.0..=1.0).contains(&self.ui.background_opacity) {
       return Err(ConfigError::Validation(format!(
           "ui.background_opacity={} out of allowed range (0.0–1.0)",
           self.ui.background_opacity
       )));
   }
   ```

4. `ConfigPartial` already wraps `UiConfig` as `Option<UiConfig>`, so no change is needed
   to the partial merge machinery — the entire `UiConfig` section is replaced when present.

**Acceptance criteria:**

- `UiConfig::default().background_opacity` is `1.0`.
- `Config::validate()` rejects values outside `[0.0, 1.0]`.
- Existing configs without `background_opacity` deserialize correctly (default to `1.0`).
- `cargo test --all` passes.
- `cargo clippy --all-targets --all-features -- -D warnings` passes.

**Tests required:**

- Default config has `background_opacity == 1.0`.
- Validation accepts `0.0`, `0.5`, `1.0`.
- Validation rejects `-0.1`, `1.1`, `2.0`.
- Round-trip: serialize then deserialize preserves the value.
- Missing field in TOML defaults to `1.0` (backward compatibility).

---

### 34.2 — Update `config_example.toml`

**Status:** Pending
**Priority:** 2 — Medium
**Scope:** `config_example.toml`

**Details:**

Add the `background_opacity` field to the `[ui]` section with documentation:

```toml
# Background opacity (0.0 = fully transparent, 1.0 = fully opaque).
# Only the terminal and menu bar backgrounds are affected; text, images,
# cursor, and colored cell backgrounds remain fully opaque.
#
# Note: On X11, transparency requires a running compositor (e.g. picom).
# Without a compositor, transparent areas will render as black.
#
# background_opacity = 1.0
```

Place it after the `hide_menu_bar` entry.

**Acceptance criteria:**

- The example documents the field, its range, default, what it affects, and the X11 caveat.
- The field is commented out (default value) matching the convention of other optional fields.

**Tests required:** None (documentation only).

---

### 34.3 — Add opacity slider to the Settings Modal UI tab

**Status:** Pending
**Priority:** 2 — Medium
**Scope:** `freminal/src/gui/settings.rs`

**Details:**

In `show_ui_tab()`, add a slider for `background_opacity` after the `hide_menu_bar` checkbox:

```rust
fn show_ui_tab(&mut self, ui: &mut Ui) {
    ui.checkbox(&mut self.draft.ui.hide_menu_bar, "Hide Menu Bar");
    // ... existing help text ...

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    ui.label("Background Opacity:");
    ui.add(Slider::new(&mut self.draft.ui.background_opacity, 0.0..=1.0).step_by(0.05));
    ui.add_space(4.0);
    ui.colored_label(
        egui::Color32::GRAY,
        "Only affects backgrounds. Text and content remain fully opaque.",
    );
    ui.colored_label(
        egui::Color32::GRAY,
        "On X11, requires a running compositor (e.g. picom).",
    );
}
```

The `Slider` import already exists in `settings.rs` (line 6).

**Acceptance criteria:**

- The UI tab shows a slider with range `[0.0, 1.0]` and step `0.05`.
- The slider value is stored in `draft.ui.background_opacity`.
- Help text explains the effect and the X11 caveat.
- Clicking Apply persists the value to `config.toml`.

**Tests required:**

- Existing settings modal tests pass unchanged.
- No new tests needed (slider behavior is an egui widget; config persistence is covered by
  existing `try_apply` / `save_config` tests).

---

### 34.4 — Wire opacity into eframe viewport and egui panel fills

**Status:** Pending
**Priority:** 1 — High
**Scope:** `freminal/src/gui/mod.rs`, `freminal/src/gui/colors.rs`

**Details:**

This is the core rendering change. Three things must happen:

**A. Enable viewport transparency when opacity < 1.0.**

In the `run()` function (`gui/mod.rs`), after constructing `NativeOptions`, conditionally
enable transparency:

```rust
if config.ui.background_opacity < 1.0 {
    native_options.viewport.transparent = Some(true);
}
```

The `viewport.transparent` field tells eframe/winit to request a window with an alpha channel
from the compositor. When opacity is 1.0 (the default), this is not set, preserving the
current behavior exactly.

**B. Set panel fills with alpha.**

In `update_egui_theme()` and `set_egui_options()`, the `window_fill` and `panel_fill` must
include the opacity alpha. Currently they use `internal_color_to_egui()` which returns
`Color32::from_rgb()` (no alpha channel).

Add a helper function to `colors.rs`:

```rust
/// Map a `TerminalColor` to an egui `Color32` with an explicit alpha channel.
#[must_use]
pub fn internal_color_to_egui_with_alpha(
    color: TerminalColor,
    make_faint: bool,
    theme: &ThemePalette,
    alpha: f32,
) -> Color32 {
    let base = internal_color_to_egui(color, make_faint, theme);
    let [r, g, b, _] = base.to_array();
    let a = (alpha * 255.0) as u8;
    Color32::from_rgba_unmultiplied(r, g, b, a)
}
```

Then use it in `set_egui_options()` and `update_egui_theme()`:

```rust
fn update_egui_theme(ctx: &egui::Context, theme: &ThemePalette, opacity: f32) {
    ctx.global_style_mut(|style| {
        let fill = internal_color_to_egui_with_alpha(
            TerminalColor::DefaultBackground, false, theme, opacity,
        );
        style.visuals.window_fill = fill;
        style.visuals.panel_fill = fill;
    });
}
```

This requires threading the `background_opacity` value through `update_egui_theme()` and
`set_egui_options()`. The `FreminalGui` struct already holds a `Config` reference; the opacity
is read from `self.config.ui.background_opacity`.

**C. Ensure the GL clear color also has alpha.**

When eframe/egui clears the framebuffer before rendering, the clear color must include the
alpha channel for transparency to work. egui uses the `window_fill` as the clear color
when rendering, so setting it with alpha (step B) should be sufficient. However, verify
that the OpenGL renderer's background shader (if any) does not force alpha = 1.0.

The current `rgb_to_f32()` in `colors.rs:34` returns `alpha = 1.0`. This function is used
for OpenGL quad colors (non-default cell backgrounds). These quads should remain fully
opaque, so `rgb_to_f32()` does **not** need to change.

**Acceptance criteria:**

- When `background_opacity = 1.0`: no visual change from current behavior.
- When `background_opacity < 1.0`: terminal and menu bar backgrounds are semi-transparent;
  desktop shows through.
- Non-default cell backgrounds (colored output) remain fully opaque.
- Text, images, cursor remain fully opaque.
- The opacity value is read from the live config and applied on theme change / config reload.

**Tests required:**

- Unit test for `internal_color_to_egui_with_alpha`: verify alpha channel is set correctly.
- `internal_color_to_egui_with_alpha` with `alpha = 1.0` produces the same RGB as
  `internal_color_to_egui` (regression guard).
- Existing color conversion tests pass unchanged.
- Manual smoke test: set `background_opacity = 0.5`, verify transparency works on the
  developer's platform. Verify text remains crisp and opaque.

---

### 34.5 — Handle opacity changes on Apply (hot-reload)

**Status:** Pending
**Priority:** 2 — Medium
**Scope:** `freminal/src/gui/mod.rs`

**Details:**

When the user changes `background_opacity` in the Settings Modal and clicks Apply, the
change must take effect immediately without restarting the application.

The existing hot-reload path (in the `SettingsAction::Applied` handler in `update()`) already
calls `update_egui_theme()` when a theme change is detected. Extend this to also apply the
new opacity value.

Two cases:

1. **Opacity changed, transparency was already enabled (opacity was already < 1.0 before):**
   Call `update_egui_theme()` with the new opacity. The panel fills update immediately.

2. **Opacity changed from 1.0 to < 1.0 (transparency was not enabled at startup):**
   The viewport was created without `transparent = true`. Enabling transparency after
   window creation may not be possible on all platforms. In this case, display a status
   message in the settings modal: "Restart required for transparency to take effect."
   The config is still saved; it will apply on next launch.

3. **Opacity changed from < 1.0 to 1.0 (disabling transparency):**
   Call `update_egui_theme()` with `alpha = 1.0`. The panel fills become fully opaque.
   No restart needed.

The simplest approach: always call `update_egui_theme()` with the current opacity on Apply.
The viewport transparency flag is a startup-time decision — if it was not set at launch,
semi-transparent fills will render against the opaque framebuffer (showing the theme's
background color at reduced alpha against black, rather than the desktop). This is acceptable
but visually wrong. The status message guides the user to restart.

To detect whether a restart is needed: compare the startup opacity (stored on `FreminalGui`
at construction) with the new opacity. If the startup opacity was 1.0 and the new opacity
is < 1.0, a restart is needed. Otherwise, no restart is needed.

**Acceptance criteria:**

- Changing opacity from 0.7 to 0.5 (both < 1.0): takes effect immediately.
- Changing opacity from 1.0 to 0.5: config saved, status message shown, takes effect after
  restart.
- Changing opacity from 0.5 to 1.0: takes effect immediately.
- The live config stored on `FreminalGui` is updated after Apply.

**Tests required:**

- No new automated tests (hot-reload behavior requires a running GUI context).
- Manual verification of all three opacity change scenarios above.

---

## Implementation Notes

### Subtask Ordering

34.1 (config field) must be done first. 34.2 (example config) can be done alongside 34.1.
34.3 (settings slider) depends on 34.1. 34.4 (rendering) depends on 34.1. 34.5 (hot-reload)
depends on 34.3 and 34.4.

**Recommended order:** 34.1 → 34.2 → 34.3 → 34.4 → 34.5

### Risk Assessment

- **Low risk.** When `background_opacity = 1.0` (the default), no code path changes behavior.
  The `viewport.transparent` flag is not set, `Color32::from_rgba_unmultiplied(r, g, b, 255)`
  is equivalent to `Color32::from_rgb(r, g, b)`, and the renderer is untouched.

- **Platform variability.** X11 without a compositor will show black instead of transparency.
  This is documented in the config example and the settings modal help text. It is not a bug
  in Freminal — it is a platform limitation.

- **No shader changes.** The OpenGL renderer does not need modification. `DefaultBackground`
  cells are already skipped (no quads emitted), so they inherit the panel fill. Non-default
  backgrounds are drawn as explicit quads with `alpha = 1.0` and remain opaque.

### Interaction with Other Tasks

- **Task 2 (Config):** Extends the config schema with a new field in `[ui]`. The layered
  merge machinery handles this automatically.
- **Task 3 (Settings Modal):** Adds a slider widget to the existing UI tab.
- **Task 11 (Theming):** The opacity applies to the theme's `DefaultBackground` color.
  Switching themes correctly updates the panel fill with the current opacity.
- **Task 1 (Custom Renderer):** The renderer skips `DefaultBackground` cells, so no
  renderer changes are needed. The opacity is applied at the egui panel fill level, which
  is beneath the renderer's OpenGL paint callback.

### Verification

Each subtask must pass before proceeding:

- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo-machete`

---

## References

- `freminal-common/src/config.rs` — Config structs, `UiConfig`, validation
- `freminal/src/gui/mod.rs` — `set_egui_options()`, `update_egui_theme()`, `NativeOptions`
- `freminal/src/gui/colors.rs` — `internal_color_to_egui()`, `rgb_to_f32()`
- `freminal/src/gui/renderer.rs` — Background quad merging, `DefaultBackground` skip logic
- `freminal/src/gui/settings.rs` — Settings modal, `show_ui_tab()`
- `config_example.toml` — Current config documentation
- `Documents/PLAN_11_THEMING.md` — Theming implementation (theme affects background color)
