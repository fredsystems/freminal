# PLAN_VERSION_110.md — v0.11.0 "The Platform Play"

## Goal

Make freminal a platform, not just a terminal. Introduce a scripting layer, a configurable
status bar, first-class SSH with layout propagation, and optional AI command-assist. This
is the version where freminal moves from "a polished implementation of the terminal
protocol" to "a programmable surface."

Depends on v0.8.0, v0.9.0, and v0.10.0. Tasks within v0.11.0 have internal dependencies
because the status bar (Task 85) and AI assist (Task 87) are both ideally driven by the
scripting layer (Task 84).

---

## Task Summary

| #   | Feature                       | Scope      | Status | Depends On        |
| --- | ----------------------------- | ---------- | ------ | ----------------- |
| 84  | Scripting Layer (Lua or WASM) | Very Large | Stub   | v0.10.0           |
| 85  | Powerline-Capable Status Bar  | Medium     | Stub   | Task 84 preferred |
| 86  | SSH Integration + Remote Mux  | Very Large | Stub   | v0.10.0           |
| 87  | AI Command Assist (opt-in)    | Medium     | Stub   | Task 72, Task 84  |

Task 86 absorbs `FUTURE_PLANS.md` items B.1 (Remote Mux) and B.7 (SSH Integration).

---

## Task 84 — Scripting Layer

### 84 Summary

Expose a small, stable scripting surface for user extensibility. The working assumption is
Lua via `mlua` (matches WezTerm's successful model), with WASM as an alternative if
sandboxing requirements grow.

Initial surface should cover:

- Keybinding handlers (override or augment `KeyAction` dispatch).
- OSC 133 event hooks (pre-command, post-command).
- Tab title and status-bar content generation.
- Layout save/load hooks.
- Notification filtering.

### 84 Open Questions (decide at activation)

- Lua vs. WASM (primary decision).
- Sandboxing / capability model.
- Configuration model: single `config.lua` replacing `config.toml`, or additive.
- Scripting ABI versioning and stability guarantees.
- Performance budget per script invocation.

---

## Task 85 — Powerline-Capable Status Bar

### 85 Summary

A minimal, toggleable status bar (per-window or per-tab) showing CWD, git branch, layout
name, session clock, custom user segments. Ideally driven by user scripting (Task 84) so
users compose their own segments, with sensible built-in defaults for users who do not
script.

### 85 Open Questions (decide at activation)

- Position: top vs. bottom, per-window vs. per-tab.
- Powerline glyph requirements (font fallback chain).
- Refresh rate and performance cost.

---

## Task 86 — SSH Integration + Remote Mux

### 86 Summary

Absorbs `FUTURE_PLANS.md` B.1 and B.7. Direct SSH connection from the terminal (connection
dialog, key management) plus remote multiplexer protocol that lets a layout move with the
user to a remote host, survives disconnects, and supports detach/reattach.

Differentiator vs. WezTerm: ship this with first-class layout propagation — "take my saved
workspace with me."

### 86 Open Questions (decide at activation)

- Wire protocol (new vs. piggyback on existing WezTerm mux protocol).
- Authentication (key delegation, agent forwarding, OS keychain integration).
- Security boundary for scripting (Task 84) across the mux link.
- Minimum viable first ship vs. full feature set.

---

## Task 87 — AI Command Assist (opt-in)

### 87 Summary

Opt-in "Ask" keybinding that sends the last N lines of terminal output (and optionally the
last command from OSC 133) to a user-configured LLM endpoint and shows a suggestion
overlay. Privacy-first: offline endpoints (Ollama, `llama.cpp`) supported; nothing leaves
the machine unless the user explicitly configures a remote endpoint.

### 87 Open Questions (decide at activation)

- Endpoint abstraction (OpenAI-compatible + Ollama + custom).
- What data is sent (user-visible preview before send).
- UI affordance: inline suggestion, modal, side panel.
- Interaction with scripting (Task 84) for user-defined prompts.

---

## Design Decisions (provisional)

- **Scripting is the spine of v0.11.0.** Without it, the status bar is fixed and AI assist
  is rigid. Task 84 lands first.
- **SSH is a full version's worth of work.** It may slip to its own minor version if
  scope balloons during design.
- **AI is optional and opt-in, always.** No telemetry, no cloud by default, no surprise
  network traffic.
