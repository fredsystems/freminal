# PLAN_VERSION_190.md — v0.19.0 "Event Hook API"

> **STATUS: ENRICHED STUB.** Durable design decisions are captured below;
> per-subtask decomposition happens at activation in a dedicated session,
> against the code as it then exists (see the `freminal-version-activation`
> skill). Do not invent subtasks early. This version is the pre-1.0
> capstone and is deliberately scheduled last.

## Goal

Expose an **event hook API**: let users register handlers (in Lua, e.g. via `mlua`) that
fire on terminal events and may respond by acting on the terminal. This is freminal's
extensibility surface. It is scheduled **dead last before 1.0 stabilization** so the event
vocabulary crystallises a frozen, shipped feature set rather than leading it.

Depends on essentially all prior feature work (v0.18.0 and earlier). Every subsystem that
emits events must already exist and be stable.

---

## Task Summary

| #   | Feature              | Scope      | Status | Depends On             |
| --- | -------------------- | ---------- | ------ | ---------------------- |
| 84  | Event Hook API (Lua) | Very Large | Stub   | v0.18.0 (all features) |

---

## Task 84 — Event Hook API

### What this is — and explicitly is NOT

**It is an event-handler API, not a config language.** `config.toml` remains the single
source of truth for configuration; the settings GUI keeps round-tripping it. There is **no
`config.lua`**. Lua-as-config (the WezTerm/Hyprland model) is explicitly rejected for
freminal because freminal committed early to a GUI settings story, and a Turing-complete
config file cannot be safely round-tripped by a settings panel. The value here is _behavior
in response to events_, which no config file can express.

Users register handlers (a separate `hooks.lua` / `~/.config/freminal/hooks/`, orthogonal
to config) that fire on events and may respond.

### Why last

An extension API must crystallise a _stable_ system, not lead it. Building it last means
every subsystem that emits events (notifications, command blocks, SSH, profiles, layouts,
paste guard, panes/tabs/windows) already exists and is battle-tested, so the event payloads
and the response API can be enumerated from real, frozen code instead of guessed. The
event/response surface becomes a 1.x compatibility commitment — define it against a frozen
feature set, immediately before stabilization.

### The surface is genuinely large (this is the point)

A serious deep dive at activation should enumerate the full vocabulary. Illustrative,
non-exhaustive event categories (the deep dive expands these):

- **Command lifecycle** (OSC 133): pre-exec, post-exec + exit code, duration, command text.
- **Output stream**: line matched regex, scrollback threshold, specific escape seen.
- **Pane/tab/window lifecycle**: created, closed, focused, split, zoomed, renamed, moved.
- **Process**: child spawned/exited, foreground process changed, CWD changed (OSC 7).
- **Input**: keybinding pressed, paste (Task 77), selection made.
- **Notification**: OSC 9/777/99 received (filtering — Task 76).
- **Connection**: SSH connect/disconnect, mux attach/detach (Task 86).
- **Config/theme**: reloaded, profile switched (Task 78), theme changed.
- **Layout**: saved, loaded, applied (Task 61).
- **Misc**: bell, focus in/out, resize, clipboard read/write, URL/hint activated.

Each event needs a stable payload schema. The **response API** (what a hook may do back to
the terminal — write to PTY, set title, spawn pane, show notification, etc.) is arguably
larger than the event surface and is part of the compatibility commitment.

Lua earns its place _because_ the surface is this rich and stateful — declarative TOML
rules and shell-out hooks stop scaling once there are ~30 event types and cross-event
state. (Those lighter mechanisms remain reasonable for narrow cases; the deep dive decides
the boundary.)

### Open questions (decide at activation — explicitly NOT now)

- The full event taxonomy and per-event payload schema (the first activation subtask is
  this deep dive).
- The response-API surface and its capability/security model.
- `mlua` integration specifics, sandboxing, performance budget per hook invocation, ABI
  versioning and stability guarantees.
- Where hooks live and how they are loaded/reloaded (orthogonal to `config.toml`).
- Which already-shipped subsystems expose hooks first vs later (status bar segments —
  Task 85; AI prompt hook; notification filtering — Task 76).

---

## Design Decisions (provisional)

- **Events, not config.** TOML stays the config source of truth; no `config.lua`. The API
  is for behavior in response to events.
- **Scheduled last on purpose.** The event/response vocabulary is a 1.x compatibility
  surface; it is defined against a frozen, shipped feature set, immediately before
  stabilization. Building it earlier would churn the schema.
- **Lua (mlua) is the working assumption** for the event-handler runtime, justified by the
  richness and statefulness of the event surface — not by imitating WezTerm's config model.
- **It retrofits, it does not gate.** Status bar (Task 85) and AI (Tasks 87a/87b) shipped
  self-contained earlier; this version _adds_ hook integration to them. Nothing shipped
  before v0.19.0 was blocked waiting on it.
- **The deep dive is the first activation subtask.** Per `freminal-version-activation`,
  this stub is not decomposed now; the event/response taxonomy is enumerated against the
  then-current code when the version is activated.
