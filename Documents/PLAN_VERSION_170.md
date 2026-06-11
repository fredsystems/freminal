# PLAN_VERSION_170.md — v0.17.0 "Status Bar"

> **STATUS: ENRICHED STUB.** Durable design decisions are captured below;
> per-subtask decomposition happens at activation in a dedicated session,
> against the code as it then exists (see the `freminal-version-activation`
> skill). Do not invent subtasks early.

## Goal

A minimal, toggleable, powerline-capable status bar showing CWD, git branch, layout name,
session clock, and custom segments.

Depends on v0.8.0. **Ships self-contained** (built-in segments + a shell-out segment type)
— it does NOT wait for the event hook API. Hook-driven segments are added later when
Task 84 lands (v0.20.0).

---

## Task Summary

| #   | Feature                      | Scope  | Status | Depends On |
| --- | ---------------------------- | ------ | ------ | ---------- |
| 85  | Powerline-Capable Status Bar | Medium | Stub   | v0.8.0     |

---

## Task 85 — Powerline-Capable Status Bar

A minimal, toggleable status bar (per-window or per-tab) with built-in segments (CWD, git
branch, layout name, session clock) and a **shell-out segment type** (run a command, show
its output) so users get dynamic content without scripting. Powerline glyphs via the font
fallback chain.

Coexists with the per-pane title bar (Task 96) — a different bar at a different scale; the
two must not conflict.

Open questions (decide at activation):

- Position: top vs. bottom, per-window vs. per-tab.
- Powerline glyph requirements (font fallback chain).
- Refresh rate and performance cost (especially for shell-out segments — must not block
  the GUI thread; shell-out runs off-thread).
- Segment config schema in `config.toml` (follow `freminal-config-options`).

---

## Design Decisions (provisional)

- **Self-contained first, scriptable later.** The earlier plan made the status bar
  "ideally driven by scripting". Since the event hook API (Task 84) now lands dead last,
  the status bar ships with built-in segments + a shell-out segment type in v0.17.0 and
  gains hook-driven segments when Task 84 lands. It is not blocked on scripting.
- **Shell-out segments never block the frame.** Command execution for a segment runs
  off the GUI thread; the bar renders the last known value.
- **Built-in segments cover the common case** so non-scripting users get a useful bar out
  of the box.
