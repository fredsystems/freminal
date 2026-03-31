# PLAN_21 — Tab Stop Completeness

## Status: Pending

---

## Overview

The tab stop infrastructure in Freminal is solid for common use: HTS, TBC Ps=0/3, CHT, and CBT
all work correctly. However, several edge cases and lesser-used TBC parameter values are either
silently ignored or handled incorrectly, and the tab stop vector is reset to defaults on every
resize instead of being extended/truncated.

This task addresses the gaps: TBC Ps=1/2/4/5 handling, resize preservation, documentation, and
comprehensive test coverage.

**Dependencies:** None (independent)
**Dependents:** None
**Primary crates:** `freminal-buffer`, `freminal-terminal-emulator`
**Estimated scope:** Small (6 subtasks)

---

## Current State

### Implemented

| Feature                   | Location              | Notes                                           |
| ------------------------- | --------------------- | ----------------------------------------------- |
| Tab stop data structure   | `buffer.rs:98-101`    | `Vec<bool>`, indexed by column                  |
| Default stops (every 8th) | `buffer.rs:133-140`   | `default_tab_stops()`                           |
| HTS (`ESC H`)             | `buffer.rs:1047-1052` | Sets stop at cursor column                      |
| TBC Ps=0                  | `buffer.rs:1055-1061` | Clears stop at cursor column                    |
| TBC Ps=3                  | `buffer.rs:1063-1066` | Clears all tab stops                            |
| CHT (`CSI Ps I`)          | `buffer.rs:1085-1093` | Advance to next tab stop, respects custom stops |
| CBT (`CSI Ps Z`)          | `buffer.rs:1068-1083` | Move to previous tab stop                       |

### Not Implemented / Broken

| Feature                              | Current Behavior                                | Correct Behavior                                |
| ------------------------------------ | ----------------------------------------------- | ----------------------------------------------- |
| TBC Ps=1 (clear line tab at cursor)  | Logged as warning, ignored                      | Explicit no-op (line tab stops not supported)   |
| TBC Ps=2 (clear all on current line) | Logged as warning, ignored                      | Equivalent to Ps=0 for character tab stops      |
| TBC Ps=4 (clear all line tab stops)  | Logged as warning, ignored                      | Explicit no-op (line tab stops not supported)   |
| TBC Ps=5 (clear all tab stops)       | Logged as warning, ignored                      | Equivalent to Ps=3 (clear all character stops)  |
| Tab stops on resize                  | `set_size()` resets to defaults unconditionally | Extend/truncate vector; preserve existing stops |
| Line tabulation (VTS, CVT, TSM)      | Not implemented at all                          | Deferred — no modern TE implements these        |

### Design Decisions

- **TBC Ps=1 and Ps=4:** These control line tab stops (vertical tab stops set on specific lines).
  No modern terminal emulator implements line tabulation. These should be explicit no-ops that
  do NOT log warnings (they are valid ECMA-48 codes, just not applicable).
- **TBC Ps=2:** Clears all character tab stops on the current line. Since Freminal uses a single
  tab stop vector (not per-line), this is equivalent to TBC Ps=0 (clear at cursor column).
- **TBC Ps=5:** Clears all tab stops (both character and line). Since line tab stops are not
  implemented, this is equivalent to TBC Ps=3.
- **Line tabulation (VTS, CVT, TSM mode):** Deferred entirely. No modern terminal implements
  these. They would require a per-line tab stop vector and a Tab Stop Mode (TSM) flag. Not
  worth the complexity.
- **Tab stops across alternate screen:** Tab stops are shared between primary and alternate
  screens. This matches xterm's behavior and is intentional — not a bug.

---

## Subtasks

---

### 21.1 — Preserve Tab Stops Across Resize

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** `freminal-buffer/src/buffer.rs`
- **Details:**
  `Buffer::set_size()` (line 647-650) currently calls `self.tab_stops = default_tab_stops(width)`
  unconditionally, destroying any custom tab stops on every resize. This is incorrect — programs
  that set custom tab stops (e.g. via HTS) lose them when the user resizes the window.

  Fix: replace the unconditional reset with extend/truncate logic:
  - If the new width is larger: extend `tab_stops` with `false` values for the new columns,
    then set default stops (every 8th column) only for the newly added columns.
  - If the new width is smaller: truncate `tab_stops` to the new width.
  - If the width is unchanged: do nothing.

  This preserves all user-set tab stops in the existing column range while providing sensible
  defaults for newly visible columns.

- **Acceptance criteria:**
  - Custom tab stops survive a width increase.
  - Custom tab stops within the new width survive a width decrease.
  - Newly added columns (on width increase) get default 8-column stops.
  - Existing default stops in the preserved range are not affected.
- **Tests required:**
  - Set custom stops at columns 5, 15, 25 → resize wider → verify stops preserved
  - Set custom stops → resize narrower → resize back wider → verify preserved stops in range
  - Verify new columns after resize have default 8-column stops

---

### 21.2 — Handle TBC Ps=1, 2, 4, 5

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-buffer/src/terminal_handler.rs`
- **Details:**
  Lines 3347-3352 in `terminal_handler.rs` log a warning for TBC with Ps=1, 2, 4, or 5 and
  take no action. These should be handled as follows:

  | Ps  | Behavior                                                           |
  | --- | ------------------------------------------------------------------ |
  | 1   | No-op (line tab stop at cursor — not implemented, silently accept) |
  | 2   | Equivalent to Ps=0 — clear character tab stop at cursor column     |
  | 4   | No-op (all line tab stops — not implemented, silently accept)      |
  | 5   | Equivalent to Ps=3 — clear all character tab stops                 |

  Remove the warning log for these values. They are valid ECMA-48 parameters and should not
  produce noise.

- **Acceptance criteria:**
  - TBC Ps=1 silently accepted, no warning, no state change.
  - TBC Ps=2 clears the tab stop at the current cursor column.
  - TBC Ps=4 silently accepted, no warning, no state change.
  - TBC Ps=5 clears all tab stops.
  - No warning messages logged for any of Ps=1,2,4,5.
- **Tests required:**
  - TBC Ps=1: set a stop, send Ps=1, verify stop still present
  - TBC Ps=2: set a stop at cursor, send Ps=2, verify stop cleared
  - TBC Ps=4: verify no state change
  - TBC Ps=5: set multiple stops, send Ps=5, verify all cleared

---

### 21.3 — Update TBC Parser Documentation

- **Status:** Pending
- **Priority:** 3 — Low
- **Scope:** `freminal-terminal-emulator/src/ansi_components/csi_commands/tbc.rs`
- **Details:**
  The TBC parser file (`tbc.rs`, 32 lines) documents only Ps=0 and Ps=3. Update the doc
  comment to list all valid Ps values (0-5) with their ECMA-48 definitions and Freminal's
  handling of each.

- **Acceptance criteria:**
  - Doc comment on `finished_parsing_tbc` lists all 6 Ps values.
  - Each value notes whether it is implemented, a no-op, or equivalent to another.
- **Tests required:** None (documentation only).

---

### 21.4 — Fix Misleading "Unimplemented Operations" Comment

- **Status:** Pending
- **Priority:** 3 — Low
- **Scope:** `freminal-buffer/src/terminal_handler.rs`
- **Details:**
  Line 3337 has a comment `// === Unimplemented Operations - TODO ===` that sits above fully
  implemented tab stop operations (CHT, TBC, CBT). The comment is misleading — these operations
  ARE implemented. Either remove the comment or move it to the correct location (above the
  actual unimplemented operations further down in the file).

- **Acceptance criteria:**
  - The misleading comment is either removed or relocated to an accurate position.
- **Tests required:** None (comment only).

---

### 21.5 — Document Tab Stop Sharing Across Alternate Screen

- **Status:** Pending
- **Priority:** 3 — Low
- **Scope:** `freminal-buffer/src/buffer.rs`
- **Details:**
  Tab stops are stored on `Buffer` and shared between primary and alternate screens. When
  `enter_alternate()` is called, the alternate buffer starts with whatever tab stops the primary
  had. When `leave_alternate()` restores the primary buffer, the primary's tab stops are
  restored from `SavedPrimaryState`.

  This matches xterm behavior but is not documented. Add a comment to `SavedPrimaryState` and
  to `enter_alternate()`/`leave_alternate()` explaining this design choice.

  Note: `SavedPrimaryState` currently does NOT save `tab_stops`. This means tab stop changes
  made while in alternate screen mode affect the primary screen when returning. This is actually
  correct (xterm behaves the same way — tab stops are truly shared, not per-buffer). Document
  this explicitly.

- **Acceptance criteria:**
  - Comments on `SavedPrimaryState`, `enter_alternate()`, and `leave_alternate()` explain
    tab stop sharing behavior.
- **Tests required:**
  - Set custom tab stops → enter alternate → verify stops present in alternate
  - In alternate screen, clear all stops → leave alternate → verify stops are cleared in primary
    (confirming shared behavior)

---

### 21.6 — Comprehensive Tab Stop Test Suite

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** `freminal-buffer/tests/terminal_handler_integration.rs`
- **Details:**
  The existing tab stop tests (5 unit tests in `terminal_handler.rs`, 7 integration tests in
  `terminal_handler_integration.rs`, 5 shadow handler tests) cover basic functionality but miss
  several edge cases. Add the following test cases:
  1. **Tab at last column:** cursor at width-1, CHT → cursor should not advance past width.
  2. **Tab with no stops set:** clear all stops, CHT → cursor should advance to last column.
  3. **Multiple CHT:** set stops at 10, 20, 30 → CHT with Ps=2 → cursor at 20 (skips 10).
  4. **CBT at column 0:** cursor at 0, CBT → cursor stays at 0.
  5. **CBT past all stops:** cursor at column 5, all stops at 8+ → CBT → cursor at 0.
  6. **HTS then TBC round-trip:** set stop, verify present, clear stop, verify absent.
  7. **Default stops after RIS:** send RIS → verify 8-column stops restored.
  8. **Tab stop at column 0:** attempt HTS at col 0 → verify behavior (should set stop).
  9. **Very wide terminal:** 200 columns, verify default stops extend correctly.
  10. **Resize preserves stops:** (after 21.1) set custom stops, resize, verify preserved.

- **Acceptance criteria:**
  - All 10 test cases pass.
  - Tests are in `terminal_handler_integration.rs` for consistency with existing tab tests.
- **Tests required:** This subtask IS the tests.

---

## Implementation Notes

### Subtask Ordering

21.1 (resize preservation) and 21.2 (TBC Ps values) are independent and can be done in parallel.
21.3 and 21.4 are trivial and can be done at any time.
21.5 is documentation and can be done at any time.
21.6 (test suite) should be done last as it validates 21.1 and 21.2.

**Recommended order:** 21.1 → 21.2 → 21.6 → 21.3 → 21.4 → 21.5

### Verification

Each subtask must pass before proceeding:

- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo-machete`

---

## References

- ECMA-48 Section 8.3.155 (TBC) — all Ps values defined
- `freminal-buffer/src/buffer.rs:98-101` — tab stop data structure
- `freminal-buffer/src/buffer.rs:133-140` — `default_tab_stops()`
- `freminal-buffer/src/buffer.rs:647-650` — resize tab stop reset (the bug)
- `freminal-buffer/src/buffer.rs:1047-1093` — HTS, TBC, CBT, CHT implementations
- `freminal-buffer/src/terminal_handler.rs:3337-3359` — handler dispatch for tab ops
- `freminal-terminal-emulator/src/ansi_components/csi_commands/tbc.rs` — TBC parser
