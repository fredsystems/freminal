# FUTURE_PLANS.md — Freminal Deferred & Unscoped Work

## Overview

This document tracks features and work items that are **not yet assigned to a version milestone**.
Items here are either deferred pending further design thinking, or are existing tracked tasks from
the Master Plan that have not been folded into a release.

For scoped, versioned work see:

- `PLAN_VERSION_030.md` — v0.3.0 (Daily Driver)
- `PLAN_VERSION_040.md` — v0.4.0 (Search & Protocol)
- `PLAN_VERSION_050.md` — v0.5.0 (Multi-Instance & Visual)
- `PLAN_VERSION_060.md` — v0.6.0 (Foundation / eframe Replacement)
- `PLAN_VERSION_070.md` — v0.7.0 (Recording & Layouts)
- `PLAN_VERSION_080.md` — v0.8.0 (Correctness & Polish — hard gate)
- `PLAN_VERSION_090.md` — v0.9.0 (Modern Workflow Terminal, stubs)
- `PLAN_VERSION_100.md` — v0.10.0 (Power-User Toolkit, stubs)
- `PLAN_VERSION_110.md` — v0.11.0 (Platform Play, stubs)
- `PLAN_VERSION_120.md` — v0.12.0 (Completeness & Credibility, stubs)

### Promotion Note (post-v0.7.0 audit)

Items B.1, B.2, B.3, B.7, and B.8 below have been **promoted from deferred into scheduled
versions** as a result of the post-v0.7.0 senior-engineer audit:

- **B.1 (Remote Mux)** and **B.7 (SSH Integration)** → absorbed into **Task 86** in v0.11.0.
- **B.2 (Command Palette)** → absorbed into **Task 83** in v0.10.0.
- **B.3 (Quick-Select / Hints)** → absorbed into **Task 82** in v0.10.0.
- **B.8 (IME / CJK)** → absorbed into **Task 88** in v0.12.0.

Their entries below are retained for historical context but the source of truth is now the
respective version plan document.

### Severity Ratings

| Rating       | Meaning                                                            |
| ------------ | ------------------------------------------------------------------ |
| **Critical** | Users will abandon the terminal immediately without this           |
| **High**     | Serious daily-use friction; power users will notice within minutes |
| **Medium**   | Noticeable gap but workarounds exist; affects specific workflows   |
| **Low**      | Polish item; absence is tolerable but presence signals maturity    |

---

## Deferred Features — Awaiting Design Decisions

These are features present in one or more major competitors that require deeper thinking about
scope, architecture, and whether/how they fit Freminal's direction. They are not assigned to
any version.

---

### B.1 — Built-in Multiplexer / Remote Mux

**Severity: Low** | **Reference: WezTerm**

WezTerm includes a built-in multiplexer that supports remote sessions (SSH + mux protocol),
eliminating the need for tmux in many workflows. This is WezTerm's signature differentiator.

**Note:** The _local_ multiplexing portion (split panes, navigation, resize, zoom) is now
covered by Task 58 in v0.5.0. B.1 remains as a placeholder for the _remote_ mux features:
SSH domains, detach/reattach, session persistence across network connections, and a mux
wire protocol. These are very large scope and deferred until core features are solid.

**Scope:** Very Large. Requires a mux protocol, session persistence, and SSH transport.

---

### B.2 — Command Palette

**Severity: Low** | **Reference: WezTerm**

A searchable command palette (Ctrl+Shift+P or similar) listing all available actions. Useful
for discoverability and for users who prefer keyboard-driven workflows.

**Scope:** Medium. Requires:

- Action registry (all keybinding-able actions as an enum)
- Fuzzy search UI (text input + filtered list)
- Action dispatch

**Primary files:** `freminal/src/gui/mod.rs`, new `freminal/src/gui/command_palette.rs`

---

### B.3 — Quick-Select / Hints Mode

**Severity: Low** | **Reference: WezTerm, Kitty**

A mode where detected patterns (URLs, file paths, git hashes, IP addresses) are highlighted
with letter labels. Typing the label copies or opens the target. Eliminates mouse usage for
common selection tasks.

**Scope:** Medium. Requires:

- Pattern detection engine (regex-based, configurable)
- Overlay rendering with labels
- Label dispatch (copy to clipboard, open URL, etc.)

---

### B.7 — SSH Integration

**Severity: Low** | **Reference: WezTerm**

Direct SSH connection from the terminal (connection dialog, key management, session
persistence) without requiring an external SSH client. Often paired with the multiplexer
(B.1).

**Scope:** Very Large. Recommend deferring.

---

### B.8 — IME Support for CJK Input

**Severity: Medium** | **Reference: WezTerm, Ghostty, Kitty**

Input Method Editor support for Chinese, Japanese, Korean text input. This is a hard
requirement for CJK users — without it, Freminal is unusable for a significant portion of
the global developer population.

egui has partial IME support. The degree to which it works with Freminal's custom renderer
needs investigation.

**Scope:** Medium-Large. Requires:

- Verify egui's IME events are correctly forwarded
- Position the IME candidate window at the cursor location
- Handle pre-edit (composing) text display
- Wide character (fullwidth) cell handling in the buffer

**Primary files:** `freminal/src/gui/terminal/`, `freminal/src/gui/input.rs`

---

### B.9 — Logging Hygiene & Targeted Filters

**Severity: Low** | **Reference: internal audit (2026-05-17 from `bugs.txt`)**

The codebase has accumulated a large volume of `debug!` and a smaller volume of `trace!`
calls. Two related problems make the current state useless for actual debugging:

1. **Wrong levels.** Many sites that fire on every PTY batch, every render frame, or every
   parsed escape sequence are emitted at `debug!`. Running with `RUST_LOG=debug` produces
   firehose output dominated by routine activity, which drowns out the rare interesting
   message a developer is actually trying to find. Most of these calls should be `trace!`.
2. **No grouping / no targeted filtering.** `tracing` supports per-target filters
   (`RUST_LOG=freminal::gui::renderer=debug,freminal::pty=trace`), but Freminal does not
   organize its instrumentation around stable target names. There are no documented "log
   groups" (e.g. `parser`, `render`, `pty`, `mux`, `recording`) that a contributor can
   enable in isolation while debugging one subsystem.

**Proposed scope:**

- Audit every `debug!` / `trace!` call in the workspace and reclassify against an explicit
  rubric: `debug!` = one-shot lifecycle / configuration / per-user-action events; `trace!`
  = per-frame, per-batch, per-byte, per-sequence events.
- Establish stable `target = "..."` names per subsystem and apply them via tracing spans
  or explicit `target:` parameters. Suggested groups: `freminal::parser`, `freminal::render`,
  `freminal::pty`, `freminal::mux`, `freminal::recording`, `freminal::config`,
  `freminal::input`, `freminal::layout`.
- Document the group names in a new `Documents/LOGGING.md` (or in `agents.md`) so users
  and developers know which filters to use.
- Optional: add a `[logging.targets]` config section so users can persist their preferred
  filter set without exporting `RUST_LOG`.

**Out of scope for the initial pass:**

- Structured logging (JSON output) — possibly worth a later task but orthogonal.
- Sampling / rate-limiting of high-frequency events — keep as a follow-up if firehose
  output is still a problem after reclassification.
- Replacing `tracing` with another framework.

**Primary files:** workspace-wide audit; concentrated in
`freminal-terminal-emulator/src/`, `freminal-buffer/src/buffer/`, `freminal/src/gui/`,
and `freminal/src/io/`.

**Scope:** Medium (mostly mechanical reclassification but spans every crate).

---

### A.2 — Split Panes

**Status: Subsumed by Task 58 (Built-in Multiplexer) in v0.5.0.**

Task 58 in `PLAN_VERSION_050.md` implements built-in terminal multiplexing with a binary pane
tree, directional navigation, resize, zoom, and all the functionality described below. See
Task 58 for the full design.

<details>
<summary>Original description (for reference)</summary>

Severity: Medium. No horizontal or vertical split pane support. Users who want side-by-side terminals must use
OS window tiling or tmux.

Less critical than tabs because tmux/zellij are common workarounds, but expected by users coming
from iTerm2, WezTerm, or Windows Terminal.

**Scope:** Large. Requires:

- Pane layout engine (tree of horizontal/vertical splits)
- Per-pane PTY ownership
- Focus tracking across panes
- Keyboard shortcuts for split/navigate/resize
- Pane border rendering

**Primary files:** `freminal/src/gui/mod.rs`, new `freminal/src/gui/panes.rs`

</details>

---

## Category C — Remaining Master Plan Tasks

These are tracked in `Documents/MASTER_PLAN.md` and their respective plan documents. They are
not assigned to a version milestone.

---

### C.1 — Performance Plan Task 11: Dead Code Cleanup

**Status:** Largely completed during Task 8 (FairMutex elimination) and Task 31 (Dead Code
Audit). May need a final verification pass.

---

### C.2 — Task 18: Client-Side Update Mechanism

**Plan:** `Documents/PLAN_18_UPDATE_MECHANISM.md`

Background HTTP check against `updates.freminal.dev`, version comparison, menu bar indicator,
and modal dialog for downloading updates.

**Status:** Pending. Depends on Tasks 2, 3, 16 (all complete).

---

### C.3 — Task 19: Update Service & Website

**Plan:** `Documents/PLAN_19_UPDATE_SERVICE_AND_WEBSITE.md`

Cloudflare Worker at `updates.freminal.dev` proxying GitHub Releases API with KV cache, plus
a project website at `freminal.dev`. Separate repository.

**Status:** Pending. Independent of the main repo.

---

## Notes

- B.1 (Remote Mux) remains deferred — local muxing is now Task 58 in v0.5.0.
- B.2, B.3, B.7, and B.8 are deferred pending design decisions — not rejected.
- B.9 (Logging Hygiene) was added 2026-05-17 from a `bugs.txt` audit item.
- A.2 (Split Panes) is subsumed by Task 58 (Built-in Multiplexer) in v0.5.0.
- Task 56 (Session Restore) is subsumed by Task 61 (Saved Layouts) in v0.7.0.
- Category C items remain tracked in `MASTER_PLAN.md` with their existing plan documents.
