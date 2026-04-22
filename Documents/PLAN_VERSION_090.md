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

### 76 Open Questions (decide at activation)

- Notification filtering: by tab, by focus, by command duration threshold.
- Which OSC sequences to support (standardize on OSC 9 or OSC 777, or both).
- Notification content template (command, exit status, duration).

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

## Design Decisions (provisional, revisit at activation)

- **OSC 133 is the anchor.** Tasks 73 and 76 both depend on it; Task 72 is therefore the
  keystone of v0.9.0 and must land first.
- **No scripting in v0.9.0.** Scripting (Lua/WASM) is deferred to v0.10.0's Thrust B work.
  v0.9.0 features are user-facing, not extensibility primitives.
- **No remote features in v0.9.0.** SSH and remote mux remain deferred to v0.11.0.
