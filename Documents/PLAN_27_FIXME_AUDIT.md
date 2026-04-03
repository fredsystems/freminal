# PLAN_27 — FIXME/TODO Audit

## Status: Complete

---

## Overview

The codebase contains `FIXME`, `TODO`, `HACK`, and `XXX` comments accumulated over the course of
development. Some of these mark genuine unfinished work, some are stale (the issue was fixed but
the comment was never removed), and some describe problems that no longer exist after subsequent
refactors.

This task audits every such marker comment across all five crates, assesses each for veracity and
relevance, and produces actionable subtasks for the ones that require mitigation.

**Dependencies:** None (independent)
**Dependents:** None
**Primary crates:** All (`freminal`, `freminal-terminal-emulator`, `freminal-buffer`,
`freminal-common`, `xtask`)
**Estimated scope:** 19 markers across 3 crates (xtask and freminal-buffer have none)

---

## Audit Results

The audit searched all `.rs` files for `FIXME`, `TODO`, `HACK`, and `XXX` marker comments.
19 real markers were found (2 grep hits were literal string data in tests, not comments).
No `HACK` markers were found. No markers exist in `xtask` or `freminal-buffer`.

### Summary by Category

| Category            | Count | Action                                           |
| ------------------- | ----- | ------------------------------------------------ |
| Stale               | 7     | Delete or replace with explanatory comment       |
| Valid (correctness) | 7     | Fix the described problem                        |
| Aspirational        | 3     | Reclassify as NOTE; track in future tasks        |
| Upstream limitation | 1     | Reclassify as NOTE with upstream issue reference |

---

## Full Marker Inventory

### Stale — Delete or Replace (7 markers)

These comments describe problems that have been resolved, questions that have been answered, or
constraints that are permanent and not actionable.

| #   | File                                                                 | Line | Marker                                                                         | Why Stale                                                                                                                                  |
| --- | -------------------------------------------------------------------- | ---- | ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------ |
| S1  | `freminal-common/src/buffer_states/tchar.rs`                         | 170  | `FIXME: Ideally this should be a generic implementation for all types`         | Rust's orphan/coherence rules prevent a blanket `impl<T> PartialEq<T>`. Three separate impls are the correct, idiomatic approach.          |
| S2  | `freminal-common/src/buffer_states/format_tag.rs`                    | 14   | `FIXME: The start and end are irrelevant once we move to the line buffer`      | Line buffer migration happened; `start`/`end` are still load-bearing for the snapshot rendering pipeline. Premise is false.                |
| S3  | `freminal-terminal-emulator/src/ansi_components/osc.rs`              | 297  | `FIXME: Support ST (0x1b)\ as a terminator`                                    | The paired function `is_osc_terminator` (line 293) already handles the full 2-byte ST sequence. The FIXME predates the two-function split. |
| S4  | `freminal-terminal-emulator/src/input.rs`                            | 188  | `TODO: investigate further — the tty driver should be handling this.`          | Investigation complete — the four-line comment block immediately above (lines 184–187) fully explains why CR is correct.                   |
| S5  | `freminal-terminal-emulator/src/ansi_components/csi_commands/sgr.rs` | 80   | `FIXME: we'll treat '\x1b[38m' or '\x1b[48m' as a color reset.`                | Behaviour is correct per de facto standard (xterm, VTE). Replace with explanatory `// NOTE:` citing the convention.                        |
| S6  | `freminal/src/gui/terminal.rs`                                       | 829  | `TODO: should we care if we scrolled in the x axis?`                           | Answered: no. `encode_x11_mouse_wheel` in `mouse.rs` explicitly guards against horizontal-only scroll and documents why.                   |
| S7  | `freminal/src/gui/mouse.rs`                                          | 231  | `FIXME: This is not correct. eframe encodes a x and y event together I think.` | Now handled correctly — `encode_x11_mouse_wheel` guards on `delta.y == 0.0` and ignores horizontal-only scroll.                            |

### Valid — Needs a Code Fix (7 markers)

These describe real problems that still exist in the current code.

| #   | File                                                         | Line | Marker                                                                                | Problem                                                                                                                                                                                                                                                                            | Severity                 |
| --- | ------------------------------------------------------------ | ---- | ------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------ |
| V1  | `freminal-common/src/buffer_states/tchar.rs`                 | 162  | `FIXME: We should probably propagate the error instead of ignoring it`                | `impl From<Vec<u8>> for TChar` silently swallows errors from `new_from_many_chars` with a `TChar::Ascii(0)` fallback. The `From` trait cannot return `Result`; needs `TryFrom` or deletion if unused.                                                                              | Correctness (minor)      |
| V2  | `freminal-common/src/buffer_states/cursor.rs`                | 112  | `FIXME: How does this work if an underline color is set but reverse video is on?`     | `get_underline_color()` discards explicitly-set underline colour when reverse video is on, falling back to background colour. Per xterm semantics, underline colour should be independent of fg/bg inversion. Companion test at `cursor_tests.rs:122` asserts the wrong behaviour. | Correctness              |
| V3  | `freminal-common/src/buffer_states/modes/sync_updates.rs`    | 10   | `FIXME: We should handle timeouts here.`                                              | `SynchronizedUpdates::DontDraw` has no timeout. A crashed program that sets `?2026 h` without resetting would freeze rendering indefinitely. The spec recommends ~200ms auto-resume.                                                                                               | Correctness (robustness) |
| V4  | `freminal-common/src/buffer_states/modes/unknown.rs`         | 29   | `FIXME: we may need to get specific about DEC vs ANSI here.`                          | `UnknownMode::report()` always emits DEC-format DECRPM (`\x1b[?...`). Unrecognised ANSI modes (no `?` prefix) get a wrong response format.                                                                                                                                         | Correctness (minor)      |
| V5  | `freminal-terminal-emulator/src/ansi_components/standard.rs` | 273  | `FIXME: Should this be the same as DecSpecialGraphics::Replace?`                      | `ESC + 0` (G3 ← DEC Special Graphics) emits `TerminalOutput::DecSpecial` which is a no-op. Should emit `DecSpecialGraphics::Replace` like `ESC ( 0` does for G0.                                                                                                                   | Correctness              |
| V6  | `freminal-terminal-emulator/src/io/pty.rs`                   | 186  | `FIXME: I don't know if this works for all locales`                                   | Locale handling unconditionally appends `.UTF-8` and replaces `-` with `_`. Breaks on non-UTF-8 system locales (e.g., `EUC-JP`). Only fires when `LANG` is unset.                                                                                                                  | Correctness (narrow)     |
| V7  | `freminal-common/src/terminfo.rs`                            | 6    | `FIXME: I would really really like this to be compiled as part of the build pipeline` | `terminfo.tar` is a committed binary artifact with no automated rebuild. Editing `freminal.ti` without regenerating the tar silently ships stale terminfo. The `build.rs` `rerun-if-changed` directive needs fixing.                                                               | Correctness (process)    |

### Aspirational — Reclassify as NOTE (3 markers)

These describe forward-looking improvements that are not currently broken.

| #   | File                                                  | Line | Marker                                                           | Why Aspirational                                                                                                                                    |
| --- | ----------------------------------------------------- | ---- | ---------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| A1  | `freminal-common/src/buffer_states/osc.rs`            | 251  | `FIXME: We're handling 0 and 2 as just title bar for now`        | OSC 0 vs 2 semantic distinction is lost. Only matters if tabs are added. Current single-window behaviour is correct.                                |
| A2  | `freminal-common/src/buffer_states/modes/xtcblink.rs` | 10   | `FIXME: I'm not sure we actually want to blink the cursor.`      | `?12` mode is tracked and DECRPM is correct. Rendering cursor blink is a UX feature, not a correctness bug. Blink infra exists from Task 23.        |
| A3  | `freminal/src/gui/fonts.rs`                           | 173  | `FIXME: for now, we're just going to ignore bundled emoji fonts` | System emoji fallback works. Bundling an emoji font is a binary-size trade-off. The dead `load_bundled_nerd_symbols` function should be cleaned up. |

### Upstream Limitation — Reclassify as NOTE (1 marker)

| #   | File                           | Line | Marker                                                          | Why Upstream                                                                                                        |
| --- | ------------------------------ | ---- | --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| U1  | `freminal/src/gui/terminal.rs` | 357  | `FIXME: We don't support separating out numpad vs regular keys` | egui unifies numpad and main-row keys. Tracked upstream as egui#3653. Not fixable in Freminal without egui changes. |

---

## Subtasks

### Subtask 27.1 — Remove 7 stale marker comments

**Scope:** S1–S7 from the inventory above.
**Action:** Delete each stale FIXME/TODO comment. Where the comment had useful context that is
now answered, replace with a brief `// NOTE:` explaining the resolution. Specifically:

- S1 (`tchar.rs:170`): Delete. Rust language limitation, not actionable.
- S2 (`format_tag.rs:14`): Delete. Premise is false — fields are still needed.
- S3 (`osc.rs:297`): Delete FIXME. Optionally add a `// NOTE:` explaining the two-function
  relationship between `is_osc_terminator` and `is_final_character_osc_terminator`.
- S4 (`input.rs:188`): Delete. The preceding comment block already contains the answer.
- S5 (`sgr.rs:80`): Replace `FIXME` with `// NOTE: Per xterm/VTE convention, bare 38/48/58
with no subparam resets the respective color channel.`
- S6 (`terminal.rs:829`): Delete. Cross-reference to `mouse.rs` optional.
- S7 (`mouse.rs:231`): Delete FIXME and its two follow-on comment lines.

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`.

---

### Subtask 27.2 — Reclassify 4 aspirational/upstream markers as NOTE comments

**Scope:** A1–A3 and U1 from the inventory above.
**Action:** Replace each `FIXME` prefix with `// NOTE:` or `// LIMITATION:` and adjust wording
to reflect the current understanding:

- A1 (`osc.rs:251`): `// NOTE: OSC 0 and 2 are conflated as title-bar-only. If tabs are added,
OSC 0 should also set the icon name and OSC 2 should set only the title.`
- A2 (`xtcblink.rs:10`): `// NOTE: Cursor blink mode (?12) is tracked and reported via DECRPM.
Rendering actual cursor blinking is deferred — the blink infrastructure from Task 23 can be
reused when this is implemented.`
- A3 (`fonts.rs:173`): `// NOTE: Emoji fonts are loaded from the system fallback chain (step 4).
Bundling an emoji font would increase binary size; deferred as a quality-of-life improvement.`
  Also: delete the dead `load_bundled_nerd_symbols` function body or the entire function if it
  has no callers.
- U1 (`terminal.rs:357`): `// LIMITATION (egui#3653): egui unifies numpad and main-row keys.
Application keypad mode cannot distinguish them until egui exposes separate key variants.`

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`.

---

### Subtask 27.3 — Fix underline color under reverse video

**Scope:** V2 — `freminal-common/src/buffer_states/cursor.rs:112` and
`freminal-common/tests/cursor_tests.rs:122`.
**Problem:** `get_underline_color()` discards an explicitly-set underline colour when
`reverse_video` is `On`, unconditionally returning the background colour instead.
**Fix:** Check whether `self.underline_color` is `DefaultUnderlineColor`. If it is the default,
fall back to `background_color.default_to_regular()` under reverse video (current behaviour).
If it is an explicitly-set colour, return it unchanged — underline colour is independent of
fg/bg inversion per xterm semantics.
**Also:** Update `test_get_underline_color_reverse_on_custom_ul` to assert the correct
behaviour (explicit green underline preserved under reverse video) and remove the `// FIXME
behavior:` comment.
**Verify:** `cargo test --all` — the updated test must pass.

---

### Subtask 27.4 — Implement synchronized updates timeout

**Scope:** V3 — `freminal-common/src/buffer_states/modes/sync_updates.rs:10`.
**Problem:** `SynchronizedUpdates::DontDraw` has no timeout. A crashed program that enables
`?2026` without disabling it would freeze rendering indefinitely.
**Fix:** Record the `Instant` when `DontDraw` is entered. In the PTY consumer thread's main
loop (or in `build_snapshot`), check if the timeout (200ms per spec guidance) has elapsed and
automatically transition back to `Draw`.
**Verify:** Add a unit test that confirms the auto-resume after timeout. `cargo test --all`.

---

### Subtask 27.5 — Fix DEC vs ANSI mode report prefix in UnknownMode

**Scope:** V4 — `freminal-common/src/buffer_states/modes/unknown.rs:29`.
**Problem:** `UnknownMode::report()` always emits DEC-format DECRPM (`\x1b[?...`), even for
unrecognised ANSI modes that should use `\x1b[...` (no `?` prefix).
**Fix:** Add a `is_dec: bool` field (or a `ModeKind` enum) to `UnknownMode`. Populate it from
the mode dispatcher in `mode.rs` based on whether the original sequence had a `?` prefix. Emit
the appropriate response format in `report()`.
**Verify:** Add unit tests for both DEC and ANSI unknown mode reports. `cargo test --all`.

---

### Subtask 27.6 — Fix G3 DEC Special Graphics designation

**Scope:** V5 — `freminal-terminal-emulator/src/ansi_components/standard.rs:273`.
**Problem:** `ESC + 0` (designate G3 as DEC Special Graphics) emits `TerminalOutput::DecSpecial`,
which is caught by a no-op handler that logs "not yet implemented". It should emit
`TerminalOutput::DecSpecialGraphics(DecSpecialGraphics::Replace)`, matching `ESC ( 0` for G0.
**Fix:** Change the `b'0'` arm under the `b'+'` match to emit
`DecSpecialGraphics(DecSpecialGraphics::Replace)`. Audit the other charset designation arms
(`b'A'`, `b'4'`, etc.) under `b'+'` for similar issues.
**Verify:** Add a test that feeds `ESC + 0` and confirms DEC line-drawing is activated.
`cargo test --all`.

---

### Subtask 27.7 — Replace `From<Vec<u8>>` with `TryFrom` for TChar

**Scope:** V1 — `freminal-common/src/buffer_states/tchar.rs:162`.
**Problem:** `impl From<Vec<u8>> for TChar` cannot return `Result`, so errors from
`new_from_many_chars` are silently swallowed with a `TChar::Ascii(0)` fallback.
**Fix:** Audit call sites. If `From<Vec<u8>>` is only used in tests, delete it. If it has
production callers, replace with `impl TryFrom<Vec<u8>> for TChar` and propagate the error.
Remove the FIXME.
**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`.

---

### Subtask 27.8 — Improve locale handling for PTY child process

**Scope:** V6 — `freminal-terminal-emulator/src/io/pty.rs:186`.
**Problem:** Locale detection unconditionally appends `.UTF-8` and replaces `-` with `_`. This
breaks on non-UTF-8 system locales and on locales that already include a codeset suffix.
**Fix:** Check whether the locale string already contains a `.` (codeset separator). If it does,
use it as-is. If it doesn't, append `.UTF-8`. Use `_` normalisation only on the language/region
portion, not the codeset. Update the FIXME to a `// NOTE:` documenting the remaining assumption
(UTF-8 only).
**Verify:** Add unit tests for locale string normalisation. `cargo test --all`.

---

### Subtask 27.9 — Automate terminfo compilation in build.rs

**Scope:** V7 — `freminal-common/src/terminfo.rs:6`.
**Problem:** `terminfo.tar` is a committed binary artifact with no automated rebuild. Editing
`freminal.ti` without manually running `tic` + `tar` silently ships stale terminfo.
**Fix:** Add a `build.rs` step (in `freminal-terminal-emulator` or the appropriate crate) that:

1. Emits `cargo:rerun-if-changed=res/freminal.ti` (and only that file).
2. Runs `tic` to compile the terminfo.
3. Runs `tar` to produce `terminfo.tar`.
4. Only re-runs when `freminal.ti` has actually changed (the targeted `rerun-if-changed`
   directive prevents the always-rebuild problem described in the original FIXME).
   Remove the manual-recompile warning once verified.
   **Verify:** Modify `freminal.ti` trivially, run `cargo build`, confirm `terminfo.tar` is
   regenerated. Revert the trivial change, run `cargo build` again, confirm no rebuild. `cargo
test --all`.

---

## Execution Order

Subtasks 27.1 and 27.2 are mechanical comment changes with no code behaviour changes. They can
be done first (and combined into a single commit).

Subtasks 27.3–27.9 are independent of each other and can be done in any order. Each involves
code changes and new tests. Recommended priority:

1. **27.1 + 27.2** — Comment cleanup (no behaviour change, low risk)
2. **27.3** — Underline color fix (correctness bug with existing failing-by-design test)
3. **27.4** — Sync updates timeout (robustness, prevents rendering freeze)
4. **27.5** — DEC vs ANSI mode report (correctness, straightforward)
5. **27.6** — G3 DEC Special Graphics (correctness, straightforward)
6. **27.7** — TChar TryFrom (correctness, low risk)
7. **27.8** — Locale handling (correctness, narrow scope)
8. **27.9** — Terminfo build automation (process improvement, requires `tic` in dev env)

---

## References

- `agents.md` — Agent rules, dead code policy
- `Documents/MASTER_PLAN.md` — Task 27 entry
