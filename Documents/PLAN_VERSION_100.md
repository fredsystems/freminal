# PLAN_VERSION_100.md — v0.10.0 "The Power-User Toolkit"

## Goal

Ship the features that turn freminal into a power user's daily driver: named profiles,
live theme preview, in-session ligature toggles, GPU-accelerated scrollback search, and
quick-select / hint mode for keyboard-driven selection.

Depends on v0.8.0 (correctness) and v0.9.0 (modern workflow primitives).

---

## Task Summary

| #   | Feature                                 | Scope  | Status | Depends On |
| --- | --------------------------------------- | ------ | ------ | ---------- |
| 78  | Profiles + Quick Profile Switching      | Medium | Stub   | v0.8.0     |
| 79  | Theme Preview + Color Picker            | Medium | Stub   | v0.8.0     |
| 80  | Font Ligatures Per-Profile Toggle       | Small  | Stub   | Task 78    |
| 81  | GPU-Accelerated Scrollback Regex Search | Medium | Stub   | v0.8.0     |
| 82  | Quick-Select / Hints Mode               | Medium | Stub   | v0.8.0     |
| 83  | Command Palette                         | Medium | Stub   | v0.8.0     |

Tasks 82 and 83 absorb FUTURE_PLANS.md items B.3 and B.2 respectively.

---

## Task 78 — Profiles + Quick Profile Switching

### 78 Summary

A profile is a named bundle of (theme, font, shell, startup command, env vars, bell mode,
ligature setting, opacity). Profiles live in `~/.config/freminal/profiles/*.toml`. The
new-tab menu gains a profile picker; a configurable keybinding cycles profiles.

### 78 Open Questions (decide at activation)

- Relationship to Workspace Env (Task 75) — is a profile a subset of a workspace, or
  orthogonal?
- Profile inheritance / composition semantics.
- Migration from current single-config users.

---

## Task 79 — Theme Preview + Color Picker

### 79 Summary

Settings → Theme hovers produce a live preview of the terminal with that theme applied.
For custom themes, a dedicated palette editor with a visible sample line beats hand-editing
TOML. Applies immediately or on explicit Save.

### 79 Open Questions (decide at activation)

- Preview scope: preview the Settings panel only vs. preview the real active terminal.
- Custom theme format / location (`~/.config/freminal/themes/*.toml`).
- Color picker widget: egui-native, or custom for better accessibility.

---

## Task 80 — Font Ligatures Per-Profile Toggle

### 80 Summary

Ligatures (Task 5) are currently a global toggle. Move to per-profile (Task 78) and add an
in-session keybinding to toggle on/off without reloading config.

### 80 Open Questions (decide at activation)

- Default: on or off per new profile.
- Whether to toggle individual ligature groups (`liga`, `calt`, `dlig`) independently.

---

## Task 81 — GPU-Accelerated Scrollback Regex Search

### 81 Summary

Leverage the existing glyph atlas / GPU pipeline to run regex search across the entire
scrollback buffer efficiently. Live highlight of matches as the user types the query. Jump
to next/previous match with keybindings.

### 81 Open Questions (decide at activation)

- Regex engine: `regex` crate vs. `fancy-regex`.
- Highlight rendering: quad overlay vs. cell background override.
- Search scope: current pane, current tab, all panes in window.

---

## Task 82 — Quick-Select / Hints Mode

### 82 Summary

Absorbs `FUTURE_PLANS.md` item B.3. Enter hints mode (configurable keybinding), and every
detected pattern (URL, file path, git hash, IP, number) is highlighted with a two-letter
label. Typing the label copies or opens the target.

### 82 Open Questions (decide at activation)

- Pattern registry: built-in vs. user-configurable regex list.
- Label alphabet (home-row Colemak-friendly vs. QWERTY).
- Action per pattern type (open URL, copy path, open file, `$EDITOR file:line`).

---

## Task 83 — Command Palette

### 83 Summary

Absorbs `FUTURE_PLANS.md` item B.2. `Ctrl+Shift+P` (configurable) opens a fuzzy-search
palette listing all `KeyAction`s with their current bindings. Selecting an action executes
it.

### 83 Open Questions (decide at activation)

- Fuzzy-match algorithm (skim, fzf-style, simple substring).
- Whether to include non-`KeyAction` commands (layout load, profile switch, theme picker).
- History and frecency ordering.

---

## Design Decisions (provisional)

- **v0.10.0 is keyboard-first.** Every feature has a keybinding and is usable without the
  mouse.
- **No scripting yet.** Scripting is the backbone of v0.11.0; we do not introduce it
  halfway here.
