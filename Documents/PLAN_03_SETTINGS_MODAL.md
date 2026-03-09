# PLAN_03 — Settings Modal

## Overview

Implement a right-click context menu with a settings modal that covers ALL configuration options
and persists changes to disk by writing back to TOML.

**Dependencies:** Task 2 (CLI Args + TOML Config) — needs complete config schema and serialization
**Dependents:** None
**Primary crate:** `freminal` (GUI binary)
**Estimated scope:** Medium

---

## Problem Statement

Freminal currently has no GUI for viewing or modifying settings. Users must manually edit
`config.toml` with a text editor and restart the application. There is no right-click context
menu or any in-app settings UI.

The config system (after Task 2) will support serialization back to TOML, making runtime
persistence feasible.

---

## Architecture Design

### Interaction Flow

```text
Right-click in terminal area
    │
    ▼
Context Menu (egui popup)
    ├── Copy
    ├── Paste
    ├── ───────────
    ├── Settings...  ◄── opens modal
    └── About...
         │
         ▼
Settings Modal (egui::Window, modal behavior)
    ├── Font tab
    │   ├── Family (dropdown/text)
    │   └── Size (slider 4.0 - 96.0)
    ├── Cursor tab
    │   ├── Shape (dropdown: block/underline/bar)
    │   └── Blink (checkbox)
    ├── Theme tab
    │   └── Name (dropdown of built-in themes)
    ├── Shell tab
    │   └── Path (text input)
    ├── Scrollback tab
    │   └── Limit (number input, 1 - 100,000)
    └── Logging tab
        └── Write to file (checkbox)
    │
    ├── [Apply] — save to disk, apply live where possible
    ├── [Cancel] — discard changes, close modal
    └── [Reset to Defaults] — restore factory defaults
```

### Design Decisions

1. **egui native widgets** — Use egui's built-in widgets (ComboBox, Slider, Checkbox, TextEdit).
   No custom widget framework needed.

2. **Modal window** — Settings should block interaction with the terminal while open. Use
   `egui::Window` with modal-like behavior (area response blocks clicks through).

3. **Tab-based layout** — Group settings by config section (Font, Cursor, Theme, Shell,
   Scrollback, Logging). Matches TOML structure.

4. **Apply semantics** — Changes are not applied until user clicks "Apply" or "OK". This prevents
   partial/broken states. Live preview can be added later.

5. **Persistence** — Uses `save_config()` from Task 2 (subtask 2.6) to write changes to disk.
   The config path used is the user-level default (or the `--config` override if specified).

6. **Hot-reload where possible** — Font size, cursor shape/blink, and theme can potentially be
   applied without restart. Shell path and scrollback limit changes require restart (noted in UI).

---

## Subtasks

### 3.1 — Implement right-click context menu

- **Status:** Not Started
- **Scope:** `freminal/src/gui/mod.rs`, `freminal/src/gui/terminal.rs`
- **Details:**
  - Detect right-click on terminal area
  - Show egui popup menu with items: Copy, Paste, separator, Settings..., About
  - Copy/Paste: wire to existing clipboard functionality (or add it)
  - Settings: set flag to open settings modal
  - About: simple dialog with version info
  - Context menu should not interfere with mouse reporting mode (if terminal app captures mouse)
- **Acceptance criteria:**
  - Right-click shows context menu
  - Menu items are functional
  - Menu dismisses on click outside or Escape
  - Does not fire when terminal app is in mouse capture mode
- **Tests required:**
  - Context menu state management (open/close)
  - Mouse capture mode suppresses context menu

### 3.2 — Create settings modal UI framework

- **Status:** Not Started
- **Scope:** New module `freminal/src/gui/settings.rs`
- **Details:**
  - `SettingsModal` struct holding:
    - `is_open: bool`
    - `draft_config: FreminalConfig` (working copy for editing)
    - `active_tab: SettingsTab` (enum)
  - `SettingsTab` enum: Font, Cursor, Theme, Shell, Scrollback, Logging
  - `show()` method renders the modal window with tab bar
  - Tab bar switches between sections
  - Bottom buttons: Apply, Cancel, Reset to Defaults
- **Acceptance criteria:**
  - Modal opens and closes correctly
  - Tab switching works
  - Draft config is independent of live config (edits don't apply until "Apply")
- **Tests required:**
  - Modal state transitions (open, close, tab switch)
  - Draft config is a clone of live config on open

### 3.3 — Implement Font settings tab

- **Status:** Not Started
- **Scope:** `freminal/src/gui/settings.rs`
- **Details:**
  - Font family: `TextEdit` for family name, or `ComboBox` with system font list
  - Font size: `Slider` with range 4.0–96.0, step 0.5
  - Preview text showing current font at selected size (stretch goal)
  - Validation: font family must be non-empty, size must be in valid range
- **Acceptance criteria:**
  - Font family and size can be edited
  - Validation prevents invalid values
  - Changes reflected in draft config

### 3.4 — Implement Cursor settings tab

- **Status:** Not Started
- **Scope:** `freminal/src/gui/settings.rs`
- **Details:**
  - Cursor shape: `ComboBox` with options: Block, Underline, Bar
  - Cursor blink: `Checkbox`
  - Visual preview of cursor shape (stretch goal)
- **Acceptance criteria:**
  - Both cursor options are editable
  - Changes reflected in draft config

### 3.5 — Implement Theme settings tab

- **Status:** Not Started
- **Scope:** `freminal/src/gui/settings.rs`
- **Details:**
  - Theme name: `ComboBox` with available built-in themes
  - Initially only "catppuccin-mocha" is available (note: theme system needs implementation)
  - Show note that custom themes are planned for future
  - Color preview swatch showing theme colors (stretch goal)
- **Acceptance criteria:**
  - Theme can be selected from available options
  - Changes reflected in draft config

### 3.6 — Implement Shell, Scrollback, and Logging tabs

- **Status:** Not Started
- **Scope:** `freminal/src/gui/settings.rs`
- **Details:**
  - Shell tab:
    - Path: `TextEdit` for shell path
    - Note: "Changes take effect on next session"
  - Scrollback tab:
    - Limit: `DragValue` or `TextEdit` with range 1–100,000
    - Note: "Changes take effect on next session"
  - Logging tab:
    - Write to file: `Checkbox`
    - Note: "Changes take effect on next launch"
- **Acceptance criteria:**
  - All options are editable with appropriate widgets
  - Validation prevents invalid values
  - Restart-required notes are visible

### 3.7 — Implement Apply / Cancel / Reset logic

- **Status:** Not Started
- **Scope:** `freminal/src/gui/settings.rs`, `freminal/src/gui/mod.rs`
- **Details:**
  - **Apply:**
    1. Validate all draft config values
    2. Call `save_config()` to write to disk
    3. Apply hot-reloadable settings (font size, cursor, theme) to live state
    4. Close modal
    5. Show success/error toast or status
  - **Cancel:** Discard draft config, close modal
  - **Reset to Defaults:**
    - Reset draft config to `FreminalConfig::default()`
    - Do NOT save — user must still click Apply
  - Handle save errors gracefully (show error message, don't close modal)
- **Acceptance criteria:**
  - Apply saves to disk and applies live settings
  - Cancel discards without saving
  - Reset restores defaults in the draft (not on disk until Apply)
  - Save errors are displayed to user
- **Tests required:**
  - Apply triggers save
  - Cancel does not modify config
  - Reset produces default config
  - Save error is handled gracefully

### 3.8 — Hot-reload settings application

- **Status:** Not Started
- **Scope:** `freminal/src/gui/mod.rs`, `freminal/src/gui/fonts.rs`
- **Details:**
  - Font size change: update egui font definitions, recalculate cell size
  - Cursor shape/blink: update cursor rendering parameters
  - Theme: update color palette (once theme system is implemented)
  - Scrollback/shell/logging: display "restart required" — no live reload
- **Acceptance criteria:**
  - Font size changes take effect immediately after Apply
  - Cursor changes take effect immediately after Apply
  - No crashes or visual glitches during hot-reload

### 3.9 — Integration and testing

- **Status:** Not Started
- **Scope:** All modified files
- **Details:**
  - End-to-end flow: right-click → settings → change font size → apply → verify
  - Verify saved TOML is valid and re-loadable
  - Verify settings survive app restart
  - Run full verification suite
- **Acceptance criteria:**
  - Complete flow works without errors
  - All tests pass, clippy clean

---

## Affected Files

| File                            | Change Type                                  |
| ------------------------------- | -------------------------------------------- |
| `freminal/src/gui/mod.rs`       | Add context menu, settings modal integration |
| `freminal/src/gui/terminal.rs`  | Right-click detection, context menu trigger  |
| `freminal/src/gui/settings.rs`  | NEW — settings modal implementation          |
| `freminal/src/gui/fonts.rs`     | Hot-reload font changes                      |
| `freminal-common/src/config.rs` | Used via save_config() from Task 2           |

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

| Risk                                        | Likelihood | Impact | Mitigation                   |
| ------------------------------------------- | ---------- | ------ | ---------------------------- |
| Modal blocks terminal input unexpectedly    | Medium     | Medium | Careful focus management     |
| Hot-reload causes visual glitch             | Medium     | Low    | Only reload safe properties  |
| Config write fails (permissions)            | Low        | Medium | Show error, don't lose draft |
| Context menu conflicts with mouse reporting | Medium     | Medium | Check mouse capture mode     |
