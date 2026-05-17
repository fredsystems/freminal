# PLAN_VERSION_090.md — v0.9.0 "The Modern Workflow Terminal"

## Goal

Turn freminal from a "very good terminal" into a "modern workflow terminal" by landing the
features that Warp, WezTerm, and Ghostty use to pull ahead: command-aware rendering,
visible command status, ergonomic multi-pane workflows, and first-class notifications.

All tasks in this version depend on v0.8.0 being complete — we do not build new features on
top of the correctness debts identified in the post-v0.7.0 audit.

---

## Task Summary

| #   | Feature                                 | Scope        | Status | Depends On      |
| --- | --------------------------------------- | ------------ | ------ | --------------- |
| 72  | OSC 133 Command Blocks                  | Large        | Stub   | v0.8.0          |
| 73  | Command Gutters (exit-status indicator) | Small        | Stub   | Task 72         |
| 74  | Broadcast Input to Panes                | Medium       | Stub   | v0.8.0, Task 58 |
| 75  | Workspace-Scoped Environment            | Medium       | Stub   | v0.8.0, Task 61 |
| 76  | Notification System (OSC 9 / OSC 777)   | Medium       | Stub   | v0.8.0, Task 72 |
| 77  | Smart Paste Guard                       | Small–Medium | Stub   | v0.8.0          |
| 94  | Tab Title Precedence (OSC vs custom)    | Small        | Stub   | v0.8.0 (71.1)   |
| 95  | Persist Custom Tab Names in Layouts     | Small        | Stub   | v0.8.0 (71.1)   |

These tasks are stubs. Detailed design and subtask breakdown happens when the version is
activated, not now.

---

## Task 72 — OSC 133 Command Blocks

### 72 Summary

Parse OSC 133 A/B/C/D sequences (already partially parsed) and use them to construct
"command blocks": each user command's input and output forms a selectable, collapsible
unit. Enable prompt-jump navigation (previous/next prompt), copy-output-only, and form
the foundation for Tasks 73 and 76.

### 72 Open Questions (decide at activation)

- Rendering model: in-buffer overlay, side gutter, or both.
- Collapse semantics: persist across scrollback trim.
- Interaction: keyboard-driven only, or mouse affordance.
- Shell integration shipped with freminal (bash, zsh, fish helper scripts).

---

## Task 73 — Command Gutters

### 73 Summary

A 4-pixel left gutter on each command block, colored by exit status: green (success), red
(non-zero), yellow (running). Passive, non-intrusive visual feedback at a glance.

### 73 Open Questions (decide at activation)

- Color source: theme palette or dedicated status colors.
- Whether to support a "currently selected command" accent.

---

## Task 74 — Broadcast Input to Panes

### 74 Summary

"Type once, send to all selected panes." A user selects a set of panes (visual selection
mode), types a command, and each keystroke is echoed to every selected PTY simultaneously.
Essential for multi-host admin workflows.

### 74 Open Questions (decide at activation)

- Selection UI: modifier+click, dedicated mode, pane "sync" lock icon.
- Visual indicator for panes currently receiving broadcast.
- Whether broadcast includes bracketed paste and control sequences or text only.

---

## Task 75 — Workspace-Scoped Environment

### 75 Summary

Extend Saved Layouts (Task 61) so each layout carries a declared environment: env vars,
theme override, font override, profile binding. Loading a layout applies the environment
atomically. "This layout = work context" becomes first-class.

### 75 Open Questions (decide at activation)

- Env inheritance: replace vs. extend shell env.
- Precedence rules: layout vs. profile vs. global config.
- Secret handling (API keys in layouts — explicit opt-in storage, never checked in).

---

## Task 76 — Notification System (OSC 9 / OSC 777)

### 76 Summary

When a background tab or pane's command completes (detected via OSC 133 D event), fire a
desktop notification. Also support OSC 9 and OSC 777 text notifications from explicit
shell invocations. Cross-platform via `notify-rust` or equivalent.

The existing in-app toast system (`freminal/src/gui/toast.rs`, introduced in 71.2) covers
in-window non-fatal notifications. Task 76 explicitly adds the _system-level_ notification
path so notifications still reach the user when freminal is minimized, in the background,
or on another desktop. The two systems are complementary, not alternatives.

### 76 Open Questions (decide at activation)

- Notification filtering: by tab, by focus, by command duration threshold.
- Which OSC sequences to support (standardize on OSC 9 or OSC 777, or both).
- Notification content template (command, exit status, duration).
- Routing policy: which events go to system notifications vs. in-app toasts vs. both.
  Provisional: errors and command-completion → system notifications when freminal is not
  focused; in-app toasts always; user-configurable per category.

---

## Task 77 — Smart Paste Guard

### 77 Summary

When the user pastes content containing multi-line input, control characters, dangerous
command patterns (`rm -rf`, `curl | sh`, `sudo` without explicit prefix), or bracketed-paste
escape attacks, show a preview dialog requiring explicit confirmation.

### 77 Open Questions (decide at activation)

- Detection pattern list: start small and expand.
- Per-profile toggle to disable (advanced users who know better).
- Whether to defend against bracketed-paste escape injection at the parser level as well.

---

## Task 94 — Tab Title Precedence (OSC vs Custom Rename)

### 94 Summary

71.1 added custom tab names via inline rename (double-click or `RenameTab` action), and
71.1 already clears the custom name when the shell asserts a title via OSC 0/1/2. That
auto-clear is the wrong default for most users: a user who explicitly renames a tab to
"build" generally does _not_ want the next `cd ~/some-project` to wipe their rename.

Make the precedence model explicit and user-controllable. At minimum:

- A user-set custom name persists across subsequent OSC 0/1/2 events from the shell.
- A config option (and/or `[tab_title]` settings panel entry) controls the policy:
  - `custom_wins` (proposed default): custom name wins forever until cleared.
  - `osc_wins` (today's behaviour): OSC clears custom name on each assertion.
  - `prefix` or `suffix`: display "$custom — $osc" / "$osc — $custom".
- A clear way to drop back to OSC-driven titles (right-click → Clear Custom Name, or an
  empty rename submission).

### 94 Open Questions (decide at activation)

- Default policy: `custom_wins` is the user-affirmed expected behavior — confirm at activation.
- Whether window title (separate from tab title) follows the same policy or always tracks
  OSC.
- Interaction with Task 95 (persisting custom names in layouts) — saved layouts should
  capture the custom name and the active policy.

### 94 Scope

Small. Touches `freminal/src/gui/tabs.rs` (`Tab` struct + `display_name()`), the OSC handler
in `freminal/src/gui/app_impl.rs`, the config schema in `freminal-common/src/config.rs`, and
the Settings Modal.

### 94 Dependencies

- 71.1 (custom tab rename) — landed in v0.8.0.

### 94 Reference

Triaged from `bugs.txt` Idea 3 (2026-05-17).

---

## Task 95 — Persist Custom Tab Names in Layouts and Last Session

### 95 Summary

71.1 added `Tab::custom_name`, but neither the Saved Layouts (Task 61) format nor the
last-session auto-save captures it. After a layout reload or session restore, every tab
falls back to its OSC- or shell-derived title and the user's renames are lost.

Extend the layout TOML schema (`LAYOUT_FORMAT.md`) with an optional per-tab `custom_name`
field and the corresponding write/read paths. Same change applies to `last_session.toml`.

### 95 Open Questions (decide at activation)

- Forward/backward compatibility with existing layout files (field is optional, so older
  files load fine — verify the read path defaults to `None` cleanly).
- Whether `last_session` and user-authored layouts get the same field shape (proposed: yes,
  identical TOML).
- Whether custom names should be variable-substituted (`$1`, `${name}`) — proposed: no,
  custom names are literal strings.

### 95 Scope

Small. Touches `freminal-common/src/layout.rs` (schema), `freminal/src/gui/session.rs`
(auto-save), `freminal/src/gui/layout_ops.rs` (build_window_from_pending_layout —
populate `tab.custom_name`), and `LAYOUT_FORMAT.md` (docs).

### 95 Dependencies

- 71.1 (custom tab rename) — landed in v0.8.0.
- Task 61 (Saved Layouts) — landed in v0.7.0.

### 95 Reference

Triaged from `bugs.txt` Idea 5 (2026-05-17).

---

## Design Decisions (provisional, revisit at activation)

- **OSC 133 is the anchor.** Tasks 73 and 76 both depend on it; Task 72 is therefore the
  keystone of v0.9.0 and must land first.
- **No scripting in v0.9.0.** Scripting (Lua/WASM) is deferred to v0.10.0's Thrust B work.
  v0.9.0 features are user-facing, not extensibility primitives.
- **No remote features in v0.9.0.** SSH and remote mux remain deferred to v0.11.0.
