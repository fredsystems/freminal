# PLAN_03 — Settings Modal

## Overview

Implement a top menu bar with a Settings entry and a tabbed settings modal that covers ALL
configuration options and persists changes to disk by writing back to TOML.

**Dependencies:** Task 2 (CLI Args + TOML Config) — needs complete config schema and serialization
**Dependents:** None
**Primary crate:** `freminal` (GUI binary)
**Estimated scope:** Medium
**Status:** Complete (2026-03-10)

---

## Problem Statement

Freminal currently has no GUI for viewing or modifying settings. Users must manually edit
`config.toml` with a text editor and restart the application. There is no menu bar or any
in-app settings UI.

The config system (after Task 2) will support serialization back to TOML, making runtime
persistence feasible.

---

## Design Pivot: Context Menu → Menu Bar

The original design called for a right-click context menu. During implementation, this was
abandoned because:

1. The terminal widget inside `CentralPanel` consumes all pointer events (including right-clicks)
   before the panel response can see them.
2. Right-click events may be terminal-level responses needed by applications like vim/htop and
   should not be swallowed by the GUI.

The final design uses a **top menu bar** (`egui::TopBottomPanel::top` + `egui::MenuBar`) with a
"Terminal" menu containing: Settings..., a separator, a debug renderer checkbox (debug builds
only), and Quit.

---

## Architecture Design

### Interaction Flow

```text
Menu Bar → "Terminal" menu
    ├── Settings...  ◄── opens modal
    ├── ───────────
    ├── ☑ Debug Renderer (debug builds only)
    └── Quit
         │
         ▼
Settings Modal (egui::Window, modal behavior)
    ├── Font tab
    │   ├── Family (text edit)
    │   └── Size (slider 4.0 - 96.0)
    ├── Cursor tab
    │   ├── Shape (combo: block/underline/bar)
    │   └── Blink (checkbox)
    ├── Theme tab
    │   └── Name (combo with built-in themes)
    ├── Shell tab
    │   └── Path (text edit)
    ├── Scrollback tab
    │   └── Limit (drag value, 1 - 100,000)
    └── Logging tab
        └── Write to file (checkbox)
    │
    ├── [Apply] — save to disk, apply live where possible
    ├── [Cancel] — discard changes, close modal
    └── [Reset to Defaults] — restore factory defaults
```

### Design Decisions

1. **egui native widgets** — Use egui's built-in widgets (ComboBox, Slider, Checkbox, TextEdit,
   DragValue). No custom widget framework needed.

2. **Top menu bar** — `egui::TopBottomPanel::top` with `egui::MenuBar` provides a standard,
   always-visible entry point. Does not conflict with terminal mouse events.

3. **Modal window** — Settings should block interaction with the terminal while open. Use
   `egui::Window` with modal-like behavior (area response blocks clicks through).

4. **Tab-based layout** — Group settings by config section (Font, Cursor, Theme, Shell,
   Scrollback, Logging). Matches TOML structure.

5. **Apply semantics** — Changes are not applied until user clicks "Apply". This prevents
   partial/broken states. Live preview can be added later.

6. **Persistence** — Uses `save_config()` from Task 2 (subtask 2.6) to write changes to disk.
   The config path used is the user-level default (or the `--config` override if specified).

7. **Hot-reload where possible** — Font family and size changes are applied immediately via
   `apply_config_changes()` on `FreminalTerminalWidget`. Cursor shape/blink changes are saved
   to config but require restart (they come from the PTY snapshot). Shell path, scrollback limit,
   and logging changes require restart (noted in UI).

---

## Subtasks

### 3.1 — Implement top menu bar

- **Status:** Complete (2026-03-10)
- **Scope:** `freminal/src/gui/mod.rs`
- **Details:**
  - Add `egui::TopBottomPanel::top("menu_bar")` with `egui::MenuBar::new().ui()` in `update()`
  - "Terminal" menu contains: Settings..., separator, Debug Renderer checkbox (debug builds
    only), Quit
  - Settings opens the settings modal; Quit calls `ctx.send_viewport_cmd(ViewportCommand::Close)`
  - Menu bar renders above `CentralPanel` and does not interfere with terminal mouse events
- **Acceptance criteria:**
  - Menu bar is visible at the top of the window
  - "Terminal" → "Settings..." opens the settings modal
  - "Terminal" → "Quit" closes the application
  - Debug checkbox only visible in debug builds
- **Completion notes:** Implemented `show_menu_bar()` on `FreminalGui` using
  `egui::TopBottomPanel::top` + `egui::MenuBar::new().ui()`. Pivoted from original right-click
  context menu design because the terminal widget consumes all pointer events and right-clicks
  may be needed by terminal applications. Removed the old `show_context_menu()` and unused
  `show_options()` methods.

### 3.2 — Create settings modal UI framework

- **Status:** Complete (2026-03-10)
- **Scope:** New module `freminal/src/gui/settings.rs`
- **Details:**
  - `SettingsModal` struct holding:
    - `is_open: bool`
    - `draft: Config` (working copy for editing)
    - `active_tab: SettingsTab` (enum)
    - `status_message: Option<String>` (error display)
    - `config_path: Option<PathBuf>` (for `--config` override)
  - `SettingsTab` enum: Font, Cursor, Theme, Shell, Scrollback, Logging
  - `show()` method renders the modal window with horizontal tab bar
  - Returns `SettingsAction` enum (None/Apply/Cancel) for caller to act on
  - Bottom buttons: Apply, Cancel, Reset to Defaults
- **Acceptance criteria:**
  - Modal opens and closes correctly
  - Tab switching works
  - Draft config is independent of live config (edits don't apply until "Apply")
- **Tests:** 7 unit tests cover modal state management, tab labels, cursor shape labels
- **Completion notes:** Created `SettingsModal` with `open()`, `show()`, `applied_config()`
  methods. `open()` clones live config into draft. `show()` renders `egui::Window` with
  `collapsible(false)` and horizontal selectable-label tab bar.

### 3.3 — Implement Font settings tab

- **Status:** Complete (2026-03-10)
- **Scope:** `freminal/src/gui/settings.rs`
- **Details:**
  - Font family: `TextEdit::singleline` for family name
  - Font size: `Slider` with range 4.0–96.0, step 0.5
  - Reads/writes `draft.font.family` (`Option<String>`) and `draft.font.size` (`f32`)
- **Acceptance criteria:**
  - Font family and size can be edited
  - Changes reflected in draft config
- **Completion notes:** Implemented inline in `show_font_tab()`. Family text edit uses
  a local `String` bound to `draft.font.family.get_or_insert_with(String::new)`.

### 3.4 — Implement Cursor settings tab

- **Status:** Complete (2026-03-10)
- **Scope:** `freminal/src/gui/settings.rs`
- **Details:**
  - Cursor shape: `ComboBox` with options via `selectable_value` (Block, Underline, Bar)
  - Cursor blink: `Checkbox`
  - Note displayed: cursor changes require restart (shape comes from PTY snapshot)
- **Acceptance criteria:**
  - Both cursor options are editable
  - Changes reflected in draft config
- **Completion notes:** Implemented in `show_cursor_tab()`. Added `PartialEq, Eq` derives
  to `CursorShapeConfig` in `freminal-common/src/config.rs` for `selectable_value` compat.
  Helper `cursor_shape_label()` maps enum variants to display strings.

### 3.5 — Implement Theme settings tab

- **Status:** Complete (2026-03-10)
- **Scope:** `freminal/src/gui/settings.rs`
- **Details:**
  - Theme name: `ComboBox` with available built-in themes
  - Currently only "catppuccin-mocha" is available
  - Note displayed: additional themes are planned for a future release
- **Acceptance criteria:**
  - Theme can be selected from available options
  - Changes reflected in draft config
- **Completion notes:** Implemented in `show_theme_tab()`. Single entry "catppuccin-mocha"
  in the combo box.

### 3.6 — Implement Shell, Scrollback, and Logging tabs

- **Status:** Complete (2026-03-10)
- **Scope:** `freminal/src/gui/settings.rs`
- **Details:**
  - Shell tab: `TextEdit::singleline` for shell path, "changes take effect on next session" note
  - Scrollback tab: `DragValue` with range 1–100,000, "requires restart" note
  - Logging tab: `Checkbox` for write-to-file, "changes take effect on next launch" note
- **Acceptance criteria:**
  - All options are editable with appropriate widgets
  - Restart-required notes are visible
- **Completion notes:** Implemented in `show_shell_tab()`, `show_scrollback_tab()`,
  `show_logging_tab()`. Shell path bound to `draft.shell.as_deref()` with local `String`.

### 3.7 — Implement Apply / Cancel / Reset logic

- **Status:** Complete (2026-03-10)
- **Scope:** `freminal/src/gui/settings.rs`, `freminal/src/gui/mod.rs`
- **Details:**
  - **Apply:** Returns `SettingsAction::Apply`; caller calls `save_config()` to write to disk,
    then calls `apply_config_changes()` for hot-reload. On save error, `status_message` is set
    and modal stays open.
  - **Cancel:** Returns `SettingsAction::Cancel`; caller closes modal, draft is discarded.
  - **Reset to Defaults:** Resets `draft` to `Config::default()` without saving — user must
    still click Apply.
- **Acceptance criteria:**
  - Apply saves to disk and applies live settings
  - Cancel discards without saving
  - Reset restores defaults in the draft (not on disk until Apply)
  - Save errors are displayed to user
- **Completion notes:** `FreminalGui::update()` matches on `SettingsAction::Apply` to call
  `save_config()` with the appropriate path, then calls `terminal_widget.apply_config_changes()`
  and updates `self.config`. Error handling sets `settings_modal.status_message`.

### 3.8 — Hot-reload settings application

- **Status:** Complete (2026-03-10)
- **Scope:** `freminal/src/gui/terminal.rs`, `freminal/src/gui/mod.rs`
- **Details:**
  - Added `apply_config_changes(&mut self, config: &Config, ctx: &egui::Context)` to
    `FreminalTerminalWidget`
  - Font family + size changes: calls `setup_font_files(ctx, &new_gui_font_config)` and
    updates `self.font_defs` and `self.terminal_fonts`
  - Cursor shape/blink: saved to config file but require restart (driven by PTY snapshot)
  - Theme: saved to config file, no live reload (single theme currently)
  - Shell/scrollback/logging: display "restart required" notes in UI
- **Acceptance criteria:**
  - Font family and size changes take effect immediately after Apply
  - No crashes or visual glitches during hot-reload
- **Completion notes:** `apply_config_changes()` builds a new `gui::fonts::FontConfig` from
  the config values, calls `setup_font_files()` for egui font registration, then updates
  internal font state. The accessor `debug_renderer_enabled()` was also added as a const fn.

### 3.9 — Integration and testing

- **Status:** Complete (2026-03-10)
- **Scope:** All modified files
- **Details:**
  - Full verification suite passes:
    - `cargo test --all`: 507 tests pass, 0 fail
    - `cargo clippy --all-targets --all-features -- -D warnings`: clean
    - `cargo-machete`: no unused dependencies
    - `cargo build --all`: clean
  - 7 unit tests added in `settings.rs` covering modal state management, tab labels,
    and cursor shape labels
- **Acceptance criteria:**
  - All tests pass, clippy clean
  - Settings modal opens, edits, saves, and closes without errors

---

## Affected Files

| File                            | Change Type                                           |
| ------------------------------- | ----------------------------------------------------- |
| `freminal/src/gui/mod.rs`       | Add menu bar, settings modal integration              |
| `freminal/src/gui/terminal.rs`  | Remove `show_options()`, add `apply_config_changes()` |
| `freminal/src/gui/settings.rs`  | NEW — settings modal implementation (all tabs)        |
| `freminal/src/main.rs`          | Pass `config_path` to `gui::run()`                    |
| `freminal-common/src/config.rs` | Add `PartialEq, Eq` derives to `CursorShapeConfig`    |

---

## UI Mockup (ASCII)

```text
┌─ Settings ──────────────────────────────────────┐
│                                                  │
│  [Font] [Cursor] [Theme] [Shell] [Scroll] [Log] │
│  ─────────────────────────────────────────────── │
│                                                  │
│  Font Family: [CaskaydiaCove Nerd Font    ▾]    │
│                                                  │
│  Font Size:   ──●────────────── 12.0            │
│               4.0              96.0              │
│                                                  │
│                                                  │
│                                                  │
│                                                  │
│  ─────────────────────────────────────────────── │
│  [Reset to Defaults]        [Cancel]   [Apply]   │
└──────────────────────────────────────────────────┘
```

---

## Risk Assessment

| Risk                                     | Likelihood | Impact | Mitigation                       |
| ---------------------------------------- | ---------- | ------ | -------------------------------- |
| Modal blocks terminal input unexpectedly | Medium     | Medium | Careful focus management         |
| Hot-reload causes visual glitch          | Medium     | Low    | Only reload font family and size |
| Config write fails (permissions)         | Low        | Medium | Show error, don't lose draft     |
| Menu bar takes vertical space            | Low        | Low    | Single row, minimal height       |
