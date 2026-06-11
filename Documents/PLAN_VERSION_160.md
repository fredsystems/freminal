# PLAN_VERSION_160.md — v0.16.0 "Reach & Credibility"

> **STATUS: ENRICHED STUB.** Durable design decisions are captured below;
> per-subtask decomposition happens at activation in a dedicated session,
> against the code as it then exists (see the `freminal-version-activation`
> skill). Do not invent subtasks early.

## Goal

Close the credibility gaps that block adoption by specific user populations: CJK / IME
users, accessibility users, users evaluating freminal against their existing terminal, and
users who hit crashes. None glamorous; all high-leverage for growing the user base.

Depends on v0.8.0 (and Task 19 for crash reporting's server side).

---

## Task Summary

| #   | Feature                            | Scope  | Status | Depends On |
| --- | ---------------------------------- | ------ | ------ | ---------- |
| 88  | IME / CJK Input Support            | Large  | Stub   | v0.8.0     |
| 89  | Accessibility Hooks (AT-SPI, NSA)  | Large  | Stub   | v0.8.0     |
| 91  | Crash Reporting (opt-in)           | Medium | Stub   | Task 19    |
| 93  | Config Import from Other Terminals | Medium | Stub   | None       |

Task 88 absorbs `FUTURE_PLANS.md` item B.8. **Task 90 (Windows Platform Quality Pass) and
Task 92 (Terminfo Self-Install) were dropped** — see `MASTER_PLAN.md` "Dropped Tasks".

---

## Task 88 — IME / CJK Input Support

Absorbs `FUTURE_PLANS.md` B.8. Input Method Editor support for Chinese, Japanese, Korean —
a blocking gap for a large part of the global developer population, required before 1.0.

Scope: verify/extend IME event forwarding from `freminal-windowing` (winit), position the
candidate window at the cursor, render pre-edit (composing) text, and handle fullwidth
cells correctly in the buffer.

Open questions (decide at activation): pre-edit rendering (inline vs overlay popup); cell
width for composing text; testing strategy (requires CJK testers — plan manual QA cycles).

---

## Task 89 — Accessibility Hooks

AT-SPI on Linux and NSAccessibility on macOS so screen readers can surface terminal
content to blind and low-vision users; Windows UI Automation if scope allows. None of the
GPU-accelerated terminals do this well today — a modest investment is a genuine
differentiator and an inclusivity win.

Open questions (decide at activation): which surfaces are exposed (live region for new
output, focusable cells, menu chrome); performance cost of continuous emission; testing
with NVDA, JAWS, VoiceOver, Orca.

---

## Task 91 — Crash Reporting (opt-in)

Local crash log dumps by default (always on, never sent anywhere). Optional, user-gated
"send to updates.freminal.dev" that uploads a redacted dump. Piggybacks on the Task 19
update-service infrastructure. Strictly opt-in, fully redacted (no env, no CWD, no command
history), local-first.

Open questions (decide at activation): dump format (minidump, backtrace-rs text, both);
redaction policy; server-side aggregation and deduplication.

---

## Task 93 — Config Import from Other Terminals

`freminal +import-config wezterm|alacritty|kitty|ghostty <path>` generates a best-effort
`config.toml` from the source terminal's configuration. The single most effective
acquisition feature for a new terminal: removes the "reconfiguring my terminal is an
afternoon" objection.

Open questions (decide at activation): import coverage (theme + keybindings + font + shell
at minimum); handling of unsupported features (log and skip vs annotated TODOs in the
generated config); export-to-other-terminal is out of scope.

---

## Design Decisions (provisional)

- **Accessibility is not optional for 1.0.** Task 89 is a non-negotiable, not a
  nice-to-have.
- **Crash reporting is local-first and opt-in.** No data leaves the machine without an
  explicit, per-incident user action; dumps are redacted.
- **Windows quality stays ad hoc** (Task 90 dropped). Regressions are fixed inline as they
  surface, as Task 68 already did.
