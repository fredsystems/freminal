# PLAN_VERSION_120.md — v0.12.0 "Completeness & Credibility"

## Goal

Close the remaining credibility gaps that block adoption by specific user populations:
CJK / IME users, accessibility users, Windows users, and users evaluating freminal against
their existing terminal. None of these are glamorous; all are high-leverage for growing
the user base.

This version is intentionally the last before 1.0 stabilization work begins. Everything
shipped here must be rock-solid and cross-platform.

---

## Task Summary

| #   | Feature                            | Scope  | Status | Depends On |
| --- | ---------------------------------- | ------ | ------ | ---------- |
| 88  | IME / CJK Input Support            | Large  | Stub   | v0.8.0     |
| 89  | Accessibility Hooks (AT-SPI, NSA)  | Large  | Stub   | v0.8.0     |
| 90  | Windows Platform Quality Pass      | Medium | Stub   | v0.8.0     |
| 91  | Crash Reporting (opt-in)           | Medium | Stub   | Task 19    |
| 92  | Terminfo Self-Install              | Small  | Stub   | None       |
| 93  | Config Import from Other Terminals | Medium | Stub   | None       |

Task 88 absorbs `FUTURE_PLANS.md` item B.8.

---

## Task 88 — IME / CJK Input Support

### 88 Summary

Absorbs `FUTURE_PLANS.md` B.8. Input Method Editor support for Chinese, Japanese, Korean.
This is a blocking gap for a significant portion of the global developer population and
must be addressed before 1.0.

Scope: verify and extend IME event forwarding from `freminal-windowing` (winit), position
the IME candidate window at the cursor, handle pre-edit (composing) text display, and
correctly handle fullwidth cells in the buffer.

### 88 Open Questions (decide at activation)

- Pre-edit rendering: inline vs. overlay popup.
- Cell width handling for composing text.
- Testing strategy (requires CJK testers; plan manual QA cycles).

---

## Task 89 — Accessibility Hooks

### 89 Summary

Implement AT-SPI on Linux and NSAccessibility on macOS so screen readers can surface
terminal content to blind and low-vision users. Windows UI Automation if scope allows.

None of the GPU-accelerated terminals do this well today; a modest investment here is a
genuine differentiator and an inclusivity win.

### 89 Open Questions (decide at activation)

- Which surfaces are exposed: live region (new output), focusable cells, menu chrome.
- Performance cost of continuous AT-SPI emission.
- Testing with NVDA, JAWS, VoiceOver, Orca.

---

## Task 90 — Windows Platform Quality Pass

### 90 Summary

Task 68 addressed specific Windows bugs (split-pane resize) ad hoc. This task is a
dedicated triage: ConPTY edge cases, `%USERPROFILE%` CWD handling, GPU driver matrix
(Intel / NVIDIA / AMD / WARP), installer pipeline, Defender false positives, high-DPI,
PowerShell and pwsh integration.

### 90 Open Questions (decide at activation)

- Installer technology (MSI, MSIX, scoop/winget, all of the above).
- Code signing strategy and budget.
- Minimum supported Windows version.

---

## Task 91 — Crash Reporting (opt-in)

### 91 Summary

Local crash log dumps by default (always on, never sent anywhere). Optional user-gated
"send to updates.freminal.dev" button that uploads a redacted dump. Piggybacks on the
Task 19 update-service infrastructure.

Strictly opt-in, fully redacted (no environment, no CWD, no command history), local-first.

### 91 Open Questions (decide at activation)

- Dump format (minidump, backtrace-rs text, both).
- Redaction policy.
- Server-side aggregation and deduplication.

---

## Task 92 — Terminfo Self-Install

### 92 Summary

`freminal +install-terminfo` subcommand that runs `tic` on the bundled `freminal.ti` into
`~/.terminfo`. Removes the papercut for users who set `TERM=freminal` and find `less` or
`htop` misbehaving.

### 92 Open Questions (decide at activation)

- Whether to auto-run on first launch.
- Fallback when `tic` is not available (bundle a pure-Rust terminfo compiler).

---

## Task 93 — Config Import from Other Terminals

### 93 Summary

`freminal +import-config wezterm|alacritty|kitty|ghostty <path>` generates a best-effort
`config.toml` from the source terminal's configuration. Single most effective acquisition
feature for any new terminal: removes the "I'd try it but reconfiguring my terminal is an
afternoon" objection.

### 93 Open Questions (decide at activation)

- Import coverage: theme + keybindings + font + shell at minimum.
- What to do with features freminal does not support (log and skip, or annotated TODOs
  in the generated config).
- Bidirectional: export-to-wezterm is probably out of scope.

---

## Design Decisions (provisional)

- **No glamour version.** v0.12.0 exists because these items individually block user
  groups. Shipping all of them is required before 1.0.
- **Accessibility is not optional.** Task 89 is not a "nice to have"; it is a
  non-negotiable for 1.0.
