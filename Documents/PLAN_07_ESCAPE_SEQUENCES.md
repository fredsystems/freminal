# PLAN_07 — Escape Sequence Coverage

## Overview

Comprehensive audit and implementation of missing, broken, and silently-swallowed escape
sequences in Freminal. This task was prompted by vttest cursor movement test failures and
the discovery that 14+ DEC private modes are parsed but silently swallowed with no effect.

**Dependencies:** None (independent, like Task 6)
**Dependents:** None
**Primary crates:** `freminal-terminal-emulator`, `freminal-buffer`, `freminal-common`
**Estimated scope:** Large (30 subtasks across 4 priority tiers)

---

## Problem Statement

A comprehensive codebase audit (2026-03-09) revealed that Freminal's escape sequence
coverage is significantly lower than previously documented (~50-55% vs. the claimed ~70%).
The existing documentation (`ESCAPE_SEQUENCE_COVERAGE.md`, `ESCAPE_SEQUENCE_GAPS.md`,
`SUPPORTED_CONTROL_CODES.md`) had many incorrect statuses — items marked as "implemented"
that are actually stubs or silently swallowed, and items marked as "missing" that are
actually fully implemented.

### Critical Findings

1. **DECSTBM double-decrement bug** — Scroll region boundaries are off by one. Both
   `handle_set_scroll_region` and `Buffer::set_scroll_region` subtract 1 from params.
   This is the likely cause of vttest cursor movement failures.

2. **DL (CSI M) not wired** — The buffer has `delete_lines()` and the handler has
   `handle_delete_lines()`, but the CSI dispatch table has no `b'M'` arm. The sequence
   is silently consumed.

3. **14+ DEC private modes silently swallowed** — Modes including DECCKM (?1), bracketed
   paste (?2004), mouse tracking (?1000-?1006), and focus events (?1004) fall through the
   `_other` catch-all at `terminal_handler.rs:651` with no logging and no effect on
   `TerminalModes`. This breaks vim, tmux, htop, and paste in shells/editors.

4. **TerminalModes never written** — `TerminalState.modes` has fields for cursor_key,
   bracketed_paste, focus_reporting, mouse_tracking, and synchronized_updates. These are
   read by snapshots but **never written** by the mode handler. Only `playback.rs` writes
   them.

5. **No tab stop infrastructure** — The HT byte (0x09) falls through as data instead of
   being handled as a C0 control. There is no tab stop array, no default 8-column stops,
   and no HTS/TBC/CHT/CBT support. This breaks basic shell output (`ls`, `man`, etc.).

6. **Missing C0 handlers** — VT (0x0B), FF (0x0C), NUL (0x00), and DEL (0x7F) are not
   handled. VT and FF should act as LF per VT spec; NUL and DEL should be silently ignored.

---

## Current State

### What Works Correctly

- Cursor movement: CUU(A), CUD(B), CUF(C), CUB(D), CHA(G), CUP(H/f), VPA(d)
- Text editing: ED(J), EL(K), IL(L), DCH(P), ECH(X), ICH(@)
- SGR: Full 256-color + TrueColor support
- ESC: Save/restore cursor (ESC 7/8), IND (ESC D), NEL (ESC E), RI (ESC M)
- G0 charset: Line drawing (ESC ( 0) and ASCII (ESC ( B)
- OSC: Window title (OSC 0/1/2), hyperlinks (OSC 8)
- DEC modes: DECAWM (?7), DECTCEM (?25), XtCBlink (?12), alt screen (?1049)
- Queries: DA1, DA2, XTVERSION, DECSCUSR, window manipulation

### What's Broken (bugs in existing code)

See `ESCAPE_SEQUENCE_COVERAGE.md` "Known Bugs" section for the complete list.

### What's Missing

See `ESCAPE_SEQUENCE_GAPS.md` for the complete categorized list.

---

## Subtasks

### Priority 1 — Critical / vttest

These items fix bugs that cause vttest failures and break basic shell output.

---

### 7.1 — Fix DECSTBM double-decrement bug

- **Status:** Done
- **Priority:** 1 — Critical
- **Scope:** `freminal-buffer/src/terminal_handler.rs` (lines 212-216), `freminal-buffer/src/buffer.rs` (lines 1198-1225)
- **Details:**
  - `handle_set_scroll_region` converts 1-based params to 0-based by subtracting 1
  - `Buffer::set_scroll_region` ALSO subtracts 1 from the values it receives
  - For `CSI 3;20 r`, the scroll region becomes [1,18] instead of the correct [2,19]
  - The default case (full screen) accidentally works because both subtractions happen to
    produce the right result for boundary values
  - Fix: Remove the `-1` in ONE of the two locations (prefer removing it from buffer since
    the handler should convert to 0-based and buffer should accept 0-based)
  - Must verify that the default case (no params / `CSI r`) still works after the fix
- **Acceptance criteria:**
  - `CSI 3;20 r` sets scroll region to rows [2,19] (0-based)
  - `CSI r` (default) sets scroll region to full screen
  - `CSI 1;N r` where N = terminal height sets scroll region to full screen
  - vttest cursor movement tests pass
- **Tests required:**
  - Scroll region with explicit params matches expected 0-based range
  - Default scroll region covers full screen
  - Single-param edge cases (e.g., `CSI 1;1 r`)
  - Scroll region respects terminal height bounds

---

### 7.2 — Wire DL (CSI M) to dispatch table

- **Status:** Done
- **Priority:** 1 — Critical
- **Scope:** `freminal-terminal-emulator/src/ansi_components/csi.rs` (dispatch table, ~line 156-244)
- **Details:**
  - `Buffer::delete_lines()` exists in `buffer.rs`
  - `TerminalHandler::handle_delete_lines()` exists in `terminal_handler.rs`
  - CSI dispatch table in `csi.rs` has NO `b'M'` arm — the sequence is silently consumed
  - Fix: Add `b'M' => { ... }` arm calling `handle_delete_lines()` with the parsed param
  - Follow the same pattern as the existing `b'L'` (IL) arm
- **Acceptance criteria:**
  - `CSI 3 M` deletes 3 lines at cursor position
  - `CSI M` (default) deletes 1 line
  - Deleted lines scroll the region up and blank lines appear at bottom of scroll region
  - Works correctly inside and outside scroll regions
- **Tests required:**
  - DL with explicit count
  - DL with default count (1)
  - DL at top of scroll region
  - DL at bottom of scroll region
  - DL with count exceeding remaining lines (should clamp)

---

### 7.3 — Implement tab stop infrastructure and HT handler

- **Status:** Done
- **Priority:** 1 — Critical
- **Scope:** `freminal-buffer/src/buffer.rs`, `freminal-terminal-emulator/src/ansi.rs` (C0 dispatch, lines 133-168)
- **Details:**
  - Add `tab_stops: Vec<bool>` field to `Buffer` (or `BTreeSet<usize>` for sparse storage)
  - Initialize with default 8-column tab stops on construction and resize
  - Add `Buffer::advance_to_next_tab_stop(&mut self)` method
  - In `ansi.rs` C0 dispatch (around line 133), add handler for byte `0x09` (HT) that calls
    `advance_to_next_tab_stop()`
  - Tab advancement: move cursor right to next tab stop position, do not wrap to next line
  - If cursor is at or past the last tab stop, move to the rightmost column
  - This subtask does NOT include HTS (ESC H), TBC (CSI g), CHT (CSI I), or CBT (CSI Z) —
    those are Priority 3 (subtask 7.22)
- **Acceptance criteria:**
  - `\t` (HT) advances cursor to next 8-column tab stop
  - Tab stops at columns 8, 16, 24, 32, ... (0-based: 7, 15, 23, 31, ...)
  - Tab at column 0 moves to column 8
  - Tab at column 7 moves to column 8
  - Tab at column 8 moves to column 16
  - Tab past last stop moves to rightmost column
  - Tab does not wrap to next line
  - `ls` output is properly aligned
  - `man` pages display correctly
- **Tests required:**
  - Default tab stops at 8-column intervals
  - HT from column 0
  - HT from column just before a tab stop
  - HT from exactly on a tab stop
  - HT near end of line (should not wrap)
  - Multiple HT in sequence
  - Tab stops after terminal resize

---

### 7.4 — Handle VT (0x0B) and FF (0x0C) as LF

- **Status:** Done
- **Priority:** 1 — Critical
- **Scope:** `freminal-terminal-emulator/src/ansi.rs` (C0 dispatch, lines 133-168)
- **Details:**
  - Per VT100/VT220/VT500 spec, VT and FF should be treated identically to LF
  - Add cases for `0x0B` and `0x0C` in the C0 dispatch that delegate to the same
    LF handler
  - This is a trivial change — just add two more byte matches to the existing LF arm
- **Acceptance criteria:**
  - VT (0x0B) moves cursor down one line (same as LF)
  - FF (0x0C) moves cursor down one line (same as LF)
  - Scrolling behavior at bottom of screen matches LF behavior
- **Tests required:**
  - VT produces same cursor movement as LF
  - FF produces same cursor movement as LF
  - VT/FF at bottom of screen scroll up

---

### 7.5 — Handle NUL (0x00) and DEL (0x7F)

- **Status:** Done
- **Priority:** 1 — Critical
- **Scope:** `freminal-terminal-emulator/src/ansi.rs` (C0 dispatch, lines 133-168)
- **Details:**
  - NUL (0x00) should be silently ignored (not included in text data)
  - DEL (0x7F) should be silently ignored (not included in text data)
  - Currently NUL may be included in data bytes, and DEL is not handled
  - Add cases that consume and discard these bytes
- **Acceptance criteria:**
  - NUL bytes in input stream are silently ignored
  - DEL bytes in input stream are silently ignored
  - Neither byte produces visible output or affects cursor position
- **Tests required:**
  - NUL in text stream produces no output
  - DEL in text stream produces no output
  - NUL/DEL do not interrupt adjacent text output

---

### 7.6 — Implement CNL (CSI E) and CPL (CSI F)

- **Status:** Done
- **Priority:** 1 — Critical
- **Scope:** `freminal-terminal-emulator/src/ansi_components/csi.rs` (dispatch table), `freminal-buffer/src/terminal_handler.rs`
- **Details:**
  - CNL (CSI Ps E): Move cursor down Ps lines, then to column 1. Default Ps = 1.
  - CPL (CSI Ps F): Move cursor up Ps lines, then to column 1. Default Ps = 1.
  - These can be implemented by combining existing CUD/CUU with a carriage-return-to-column-0
  - Add `b'E'` and `b'F'` arms to CSI dispatch table
  - Add handler functions (or reuse existing cursor movement + explicit column set)
- **Acceptance criteria:**
  - `CSI 3 E` moves cursor down 3 lines and to column 1
  - `CSI E` (default) moves down 1 line and to column 1
  - `CSI 3 F` moves cursor up 3 lines and to column 1
  - `CSI F` (default) moves up 1 line and to column 1
  - Movement is clamped to screen boundaries
- **Tests required:**
  - CNL with explicit count
  - CNL with default count
  - CPL with explicit count
  - CPL with default count
  - CNL/CPL at screen boundaries (should clamp, not wrap)
  - Cursor column is set to 1 (0-based: 0) after movement

---

### Priority 2 — Breaks Real Apps

These items fix issues that break vim, tmux, htop, and other common TUI applications.

---

### 7.7 — Wire DECCKM (?1) to TerminalModes

- **Status:** Done
- **Priority:** 2 — High
- **Scope:** `freminal-buffer/src/terminal_handler.rs` (mode dispatch, lines 634-655), `freminal-common/src/buffer_states/mode.rs`, `freminal-terminal-emulator/src/state/internal.rs`
- **Details:**
  - DECCKM (Cursor Key Mode) determines whether arrow keys send normal (`CSI A/B/C/D`) or
    application (`ESC O A/B/C/D`) sequences
  - Currently falls through the `_other` catch-all at `terminal_handler.rs:651` with no effect
  - Fix: Replace the `_other` catch-all with explicit match arms for known modes
  - For ?1: Set `TerminalState.modes.cursor_key` to `CursorKeyMode::Application` on set,
    `CursorKeyMode::Normal` on reset
  - The GUI/PTY layer must then read this mode when translating key events
  - `TerminalModes` already has a `cursor_key` field — it just needs to be written
- **Acceptance criteria:**
  - `CSI ? 1 h` sets cursor key mode to Application
  - `CSI ? 1 l` resets cursor key mode to Normal
  - Arrow keys in application mode send `ESC O A/B/C/D`
  - Arrow keys in normal mode send `CSI A/B/C/D`
  - vim navigation works correctly (vim sends `CSI ? 1 h` on startup)
- **Tests required:**
  - DECCKM set changes TerminalModes.cursor_key
  - DECCKM reset changes TerminalModes.cursor_key
  - Mode persists across multiple set/reset cycles
  - Mode is included in terminal snapshots

---

### 7.8 — Wire bracketed paste (?2004) to TerminalModes

- **Status:** Done
- **Priority:** 2 — High
- **Scope:** `freminal-buffer/src/terminal_handler.rs` (mode dispatch), `freminal-common/src/buffer_states/mode.rs`, `freminal-terminal-emulator/src/state/internal.rs`
- **Details:**
  - Bracketed paste mode causes the terminal to wrap pasted text with `ESC [ 200 ~` and
    `ESC [ 201 ~` markers so applications can distinguish typed input from pasted text
  - Currently falls through `_other` catch-all with no effect
  - Fix: Set `TerminalState.modes.bracketed_paste` to true on set, false on reset
  - The GUI layer must then read this mode and wrap paste events with the bracket sequences
  - `TerminalModes` already has a `bracketed_paste` field
- **Acceptance criteria:**
  - `CSI ? 2004 h` enables bracketed paste mode
  - `CSI ? 2004 l` disables bracketed paste mode
  - When enabled, pasted text is wrapped with `ESC [ 200 ~` ... `ESC [ 201 ~`
  - When disabled, pasted text is sent without wrapping
  - Paste in bash/zsh works correctly with bracketed paste enabled
- **Tests required:**
  - Mode set/reset changes TerminalModes.bracketed_paste
  - Mode is included in terminal snapshots
  - Default state is disabled

---

### 7.9 — Wire mouse tracking modes (?1000/?1002/?1003/?1006)

- **Status:** Done
- **Priority:** 2 — High
- **Scope:** `freminal-buffer/src/terminal_handler.rs` (mode dispatch), `freminal-common/src/buffer_states/mode.rs`, `freminal-terminal-emulator/src/state/internal.rs`
- **Details:**
  - ?1000: X11 Normal Tracking — report button press/release
  - ?1002: X11 Button Event Tracking — also report motion while button held
  - ?1003: X11 Any Event Tracking — report all motion
  - ?1006: SGR Extended Coordinates — use `CSI < ...` format instead of X10 encoding
  - All four currently fall through `_other` catch-all
  - Fix: Set appropriate `TerminalModes.mouse_tracking` field for each mode
  - The GUI layer must read these modes and forward mouse events to the PTY
  - `TerminalModes` already has mouse_tracking fields
  - Higher-numbered tracking modes supersede lower ones; resetting returns to previous
- **Acceptance criteria:**
  - Each mode set/reset correctly updates TerminalModes
  - Modes are mutually exclusive (setting ?1003 implies ?1000 and ?1002)
  - Resetting all mouse modes disables mouse reporting
  - Modes are included in terminal snapshots
- **Tests required:**
  - Each mode set/reset individually
  - Mode supersession (setting ?1003 then resetting ?1003 returns to no mouse)
  - Default state is no mouse tracking

---

### 7.10 — Wire focus events (?1004) to TerminalModes

- **Status:** Done
- **Priority:** 2 — High
- **Scope:** `freminal-buffer/src/terminal_handler.rs` (mode dispatch), `freminal-common/src/buffer_states/mode.rs`
- **Details:**
  - ?1004: When enabled, terminal sends `CSI I` on focus-in and `CSI O` on focus-out
  - Currently falls through `_other` catch-all
  - Fix: Set `TerminalState.modes.focus_reporting` on set/reset
  - GUI layer must read this mode and send focus events to PTY when window gains/loses focus
  - `TerminalModes` already has a `focus_reporting` field
- **Acceptance criteria:**
  - `CSI ? 1004 h` enables focus reporting
  - `CSI ? 1004 l` disables focus reporting
  - Mode is included in terminal snapshots
- **Tests required:**
  - Mode set/reset changes TerminalModes.focus_reporting
  - Default state is disabled

---

### 7.11 — Implement SU (CSI S) and SD (CSI T)

- **Status:** Done
- **Priority:** 2 — High
- **Scope:** `freminal-terminal-emulator/src/ansi_components/csi.rs` (dispatch table), `freminal-buffer/src/terminal_handler.rs`, `freminal-buffer/src/buffer.rs`
- **Details:**
  - SU (CSI Ps S): Scroll up Ps lines. Content moves up, blank lines appear at bottom.
  - SD (CSI Ps T): Scroll down Ps lines. Content moves down, blank lines appear at top.
  - These operate on the scroll region if one is set
  - May be able to reuse existing scroll machinery (IND/RI do single-line scrolling)
  - Add `b'S'` and `b'T'` arms to CSI dispatch table
  - Add handler/buffer methods for multi-line scrolling
- **Acceptance criteria:**
  - `CSI 3 S` scrolls content up 3 lines within scroll region
  - `CSI S` (default) scrolls up 1 line
  - `CSI 3 T` scrolls content down 3 lines within scroll region
  - `CSI T` (default) scrolls down 1 line
  - Blank lines are inserted at the appropriate edge
  - Respects current scroll region boundaries
- **Tests required:**
  - SU/SD with explicit and default counts
  - SU/SD with scroll region set
  - SU/SD without scroll region (whole screen)
  - SU/SD with count exceeding region size (clamp to region)

---

### 7.12 — Fix DSR (CSI n) to check Ps value

- **Status:** Done
- **Priority:** 2 — High
- **Scope:** `freminal-buffer/src/terminal_handler.rs`
- **Details:**
  - Currently `CSI n` always emits a cursor position report (`CSI row ; col R`) regardless
    of the Ps parameter
  - Ps=5 should respond with device status: `CSI 0 n` (device OK)
  - Ps=6 should respond with cursor position: `CSI row ; col R` (current behavior)
  - Fix: Check the Ps parameter and dispatch to the appropriate response
- **Acceptance criteria:**
  - `CSI 5 n` responds with `CSI 0 n` (device status OK)
  - `CSI 6 n` responds with `CSI row ; col R` (cursor position)
  - `CSI n` (default Ps=0) is handled gracefully (ignored or treated as Ps=5)
- **Tests required:**
  - DSR with Ps=5 returns status report
  - DSR with Ps=6 returns cursor position
  - Cursor position report has correct 1-based row and column

---

### 7.13 — Implement ESC c (RIS) fully

- **Status:** Done
- **Priority:** 2 — High
- **Scope:** `freminal-terminal-emulator/src/ansi_components/standard.rs`, `freminal-buffer/src/terminal_handler.rs`, `freminal-buffer/src/buffer.rs`
- **Details:**
  - ESC c (Reset to Initial State) should fully reset the terminal:
    - Clear screen and scrollback
    - Reset cursor to home position (0,0)
    - Reset all character attributes (SGR)
    - Reset all DEC private modes to defaults
    - Reset scroll region to full screen
    - Reset tab stops to default 8-column positions
    - Reset character set to default (G0 = ASCII)
  - Currently parsed as a stub with `warn!` log and no actual effect
  - Fix: Implement a `full_reset()` method on Buffer/TerminalHandler that resets all state
- **Acceptance criteria:**
  - `ESC c` resets screen, cursor, attributes, modes, scroll region, and tab stops
  - Terminal is in same state as initial startup after RIS
  - No `warn!` log — this is a normal operation
- **Tests required:**
  - RIS clears screen
  - RIS resets cursor to home
  - RIS resets SGR attributes
  - RIS resets scroll region
  - RIS resets DEC modes to defaults

---

### 7.14 — Implement DECPAM (ESC =) and DECPNM (ESC >)

- **Status:** Done
- **Priority:** 2 — High
- **Scope:** `freminal-terminal-emulator/src/ansi_components/standard.rs`, `freminal-common/src/buffer_states/mode.rs`, `freminal-terminal-emulator/src/state/internal.rs`
- **Details:**
  - ESC = (DECPAM): Set Application Keypad Mode — numpad keys send application sequences
  - ESC > (DECPNM): Set Numeric Keypad Mode — numpad keys send numeric values
  - Currently both are parsed with `warn!` log but have no effect on terminal state
  - Fix: Add keypad_mode field to TerminalModes (or TerminalState) and set it appropriately
  - GUI layer must read this mode when translating numpad key events
- **Acceptance criteria:**
  - `ESC =` sets keypad to application mode
  - `ESC >` sets keypad to numeric mode
  - Mode is included in terminal snapshots
  - No `warn!` log — these are normal operations
- **Tests required:**
  - DECPAM sets application keypad mode
  - DECPNM sets numeric keypad mode
  - Default state is numeric mode

---

### 7.15 — Wire ?5 (DECSCNM), ?6 (DECOM), ?3 (DECCOLM)

- **Status:** Done (partially — DECSCNM/?5, DECARM/?8, ReverseWrapAround/?45, SynchronizedUpdates/?2026, and LNM/20 are wired; DECOM/?6 and DECCOLM/?3 deferred to functional implementation)
- **Priority:** 2 — Medium
- **Scope:** `freminal-buffer/src/terminal_handler.rs` (mode dispatch), `freminal-common/src/buffer_states/mode.rs`
- **Details:**
  - ?5 (DECSCNM — Reverse Video): Swap default foreground/background colors for entire screen.
    Requires renderer support to honor a "reverse video" flag.
  - ?6 (DECOM — Origin Mode): When set, cursor addressing is relative to the scroll region
    top. CUP row 1 goes to top of scroll region, not top of screen.
  - ?3 (DECCOLM — 132 Column Mode): Switches between 80 and 132 column mode. Modern terminals
    often ignore this or just clear the screen.
  - All three currently fall through `_other` catch-all
  - At minimum: store the mode values in TerminalModes so apps can query them via DECRQM.
    Full functional implementation depends on renderer/buffer capabilities.
- **Acceptance criteria:**
  - Each mode set/reset updates TerminalModes
  - Modes are queryable via DECRQM
  - DECOM: cursor positioning is relative to scroll region when set
  - DECSCNM: at minimum, mode is stored (full rendering may be deferred)
  - DECCOLM: at minimum, mode is stored (column change may be deferred)
- **Tests required:**
  - Each mode set/reset updates state
  - DECOM affects CUP cursor addressing

---

### 7.16 — Add logging for unrecognized CSI and DEC modes

- **Status:** Done
- **Priority:** 2 — Medium
- **Scope:** `freminal-terminal-emulator/src/ansi_components/csi.rs` (line 240), `freminal-buffer/src/terminal_handler.rs` (line 651)
- **Details:**
  - Unrecognized CSI final bytes silently fall through at `csi.rs:240` with no log
  - Unrecognized DEC private modes fall through `_other` at `terminal_handler.rs:651`
  - Replace both with `warn!` logging that includes the unrecognized byte/mode number
  - This helps diagnose compatibility issues without breaking anything
  - Also reduce OSC unknown logging from `error!` to `debug!` and remove the double
    `TerminalOutput::Invalid` emission
- **Acceptance criteria:**
  - Unrecognized CSI final bytes produce `warn!` log with the byte value
  - Unrecognized DEC modes produce `warn!` log with the mode number
  - Unknown OSC produces `debug!` log (not `error!`) and no double emission
  - No functional change to terminal behavior for unrecognized sequences
- **Tests required:**
  - Unrecognized CSI byte is logged (check log output or test return value)
  - Unrecognized DEC mode is logged
  - Known sequences are not affected by logging changes

---

### Priority 3 — Modern Features

These items add modern terminal capabilities for feature parity with WezTerm/iTerm2.

---

### 7.17 — Implement OSC 52 (clipboard)

- **Status:** Not Started
- **Priority:** 3 — Medium
- **Scope:** `freminal-terminal-emulator/src/ansi_components/osc.rs`, `freminal-common/src/buffer_states/osc.rs`, `freminal/src/gui/` (clipboard integration)
- **Details:**
  - OSC 52 enables applications to set/query the system clipboard via escape sequences
  - Format: `OSC 52 ; Pc ; Pd ST` where Pc is clipboard name (c=clipboard, p=primary,
    s=selection) and Pd is base64-encoded data (or `?` to query)
  - Parse OSC 52 in the OSC handler
  - Decode base64 payload
  - Forward to GUI layer via TerminalOutput variant for clipboard set
  - For query (`?`): read clipboard and respond with base64-encoded content
  - Security consideration: may want to prompt user or limit clipboard access
- **Acceptance criteria:**
  - Applications can set clipboard content via `OSC 52 ; c ; <base64> ST`
  - Applications can query clipboard via `OSC 52 ; c ; ? ST`
  - Base64 encoding/decoding is correct
  - Works with tmux copy-pipe, vim `+clipboard`, zsh selection
- **Tests required:**
  - OSC 52 set parses correctly and produces clipboard output
  - OSC 52 query produces correct response
  - Base64 round-trip is correct
  - Invalid base64 is handled gracefully

---

### 7.18 — Implement OSC 7 (CWD tracking)

- **Status:** Not Started
- **Priority:** 3 — Medium
- **Scope:** `freminal-terminal-emulator/src/ansi_components/osc.rs`, `freminal-common/src/buffer_states/osc.rs`, `freminal-terminal-emulator/src/state/internal.rs`
- **Details:**
  - OSC 7 reports the current working directory of the shell
  - Format: `OSC 7 ; file://hostname/path ST`
  - Currently recognized with debug log but no functional effect
  - Fix: Parse the URI, store the CWD in TerminalState, include in snapshots
  - GUI can use this for tab titles, "Open file manager here", etc.
- **Acceptance criteria:**
  - `OSC 7 ; file://localhost/home/user ST` stores `/home/user` as CWD
  - CWD is included in terminal snapshots
  - CWD updates when shell changes directory (shell must be configured to emit OSC 7)
- **Tests required:**
  - OSC 7 with valid file URI stores CWD
  - OSC 7 with hostname extraction
  - CWD is available in terminal state

---

### 7.19 — Implement OSC 133 (shell integration / FTCS)

- **Status:** Not Started
- **Priority:** 3 — Medium
- **Scope:** `freminal-terminal-emulator/src/ansi_components/osc.rs`, `freminal-common/src/buffer_states/osc.rs`, `freminal-terminal-emulator/src/state/internal.rs`
- **Details:**
  - OSC 133 provides prompt/command/output markers for shell integration:
    - `OSC 133 ; A ST` — Prompt start
    - `OSC 133 ; B ST` — Prompt end / command start
    - `OSC 133 ; C ST` — Command end / output start (pre-execution)
    - `OSC 133 ; D ; exitcode ST` — Command finished with exit code
  - Currently recognized with debug log but no functional effect
  - Fix: Store prompt regions and command boundaries in buffer metadata
  - Enables features like: click to re-run command, scroll between prompts, dim old output
- **Acceptance criteria:**
  - All four FTCS markers are parsed and stored
  - Prompt/command/output regions can be queried from terminal state
  - Exit codes are captured
- **Tests required:**
  - Each marker type is parsed correctly
  - Markers store correct buffer positions
  - Exit code is captured from `OSC 133 ; D`

---

### 7.20 — Implement synchronized output (?2026)

- **Status:** Done (completed as part of subtask 7.15 in Priority 2)
- **Priority:** 3 — Medium
- **Scope:** `freminal-buffer/src/terminal_handler.rs` (mode dispatch), `freminal-terminal-emulator/src/state/internal.rs`
- **Details:**
  - ?2026: When set, terminal should defer rendering until mode is reset
  - Prevents partial screen updates / tearing during rapid output
  - Currently falls through `_other` catch-all
  - Fix: Set `TerminalState.modes.synchronized_updates` flag
  - GUI layer should check this flag and batch rendering updates
  - `TerminalModes` already has a `synchronized_updates` field
- **Acceptance criteria:**
  - `CSI ? 2026 h` enables synchronized updates
  - `CSI ? 2026 l` disables synchronized updates and triggers a render
  - Mode is included in terminal snapshots
- **Tests required:**
  - Mode set/reset changes TerminalModes
  - Default state is disabled

---

### 7.21 — Implement HTS (ESC H), TBC (CSI g), CHT (CSI I), CBT (CSI Z)

- **Status:** Done
- **Priority:** 3 — Medium
- **Scope:** `freminal-terminal-emulator/src/ansi_components/standard.rs`, `freminal-terminal-emulator/src/ansi_components/csi.rs`, `freminal-buffer/src/buffer.rs`
- **Details:**
  - Depends on tab stop infrastructure from subtask 7.3
  - HTS (ESC H): Set a tab stop at the current cursor column
  - TBC (CSI Ps g): Clear tab stops. Ps=0: clear at current column. Ps=3: clear all.
  - CHT (CSI Ps I): Move cursor forward to Ps-th next tab stop
  - CBT (CSI Ps Z): Move cursor backward to Ps-th previous tab stop
  - Add methods to Buffer for set/clear tab stops
  - Add ESC H handler in standard.rs
  - Add CSI g, I, Z arms in csi.rs dispatch
- **Acceptance criteria:**
  - ESC H sets tab stop at current column
  - CSI 0 g clears tab stop at current column
  - CSI 3 g clears all tab stops
  - CSI 2 I moves forward 2 tab stops
  - CSI 2 Z moves backward 2 tab stops
  - Tab stop changes persist until cleared or terminal reset
- **Tests required:**
  - HTS sets tab stop at specific column
  - TBC clears individual and all tab stops
  - CHT moves forward correct number of stops
  - CBT moves backward correct number of stops
  - Edge cases: no tab stop in direction, at screen boundary

---

### 7.22 — Implement DECRQSS (DCS $ q ... ST)

- **Status:** Not Started
- **Priority:** 3 — Low
- **Scope:** `freminal-terminal-emulator/src/ansi_components/standard.rs` (DCS handling)
- **Details:**
  - DECRQSS (Request Selection or Setting) allows applications to query current terminal
    settings like SGR attributes, DECSTBM margins, DECSCUSR cursor style, etc.
  - Format: `DCS $ q Pt ST` where Pt identifies the setting to query
  - Response: `DCS Ps $ r Pt ST` where Ps=1 (valid) or Ps=0 (invalid)
  - Common queries: `m` (SGR), `r` (DECSTBM), `SP q` (DECSCUSR)
  - Currently all DCS is captured as opaque bytes with no sub-command parsing
  - Fix: Add DCS sub-command dispatch, implement DECRQSS for common settings
- **Acceptance criteria:**
  - `DCS $ q m ST` responds with current SGR attributes
  - `DCS $ q r ST` responds with current scroll region
  - Invalid queries respond with `DCS 0 $ r ST`
- **Tests required:**
  - DECRQSS for SGR returns current attributes
  - DECRQSS for DECSTBM returns current margins
  - DECRQSS for unknown setting returns error response

---

### 7.23 — Implement XTGETTCAP (DCS + q ... ST)

- **Status:** Not Started
- **Priority:** 3 — Low
- **Scope:** `freminal-terminal-emulator/src/ansi_components/standard.rs` (DCS handling)
- **Details:**
  - XTGETTCAP allows applications (notably nvim) to query termcap/terminfo capabilities
  - Format: `DCS + q Pt ST` where Pt is hex-encoded capability name
  - Response: `DCS Ps + r Pt = Pv ST` where Pv is hex-encoded value
  - nvim uses this to query capabilities like `RGB`, `setrgbf`, `setrgbb`
  - At minimum: respond to common queries. Unknown capabilities get `DCS 0 + r ST`.
- **Acceptance criteria:**
  - Common capability queries (RGB, colors, etc.) get correct responses
  - Unknown capabilities get error response
  - nvim does not emit warnings about unsupported terminal
- **Tests required:**
  - XTGETTCAP for known capabilities returns correct values
  - XTGETTCAP for unknown capabilities returns error response
  - Hex encoding/decoding is correct

---

### Priority 4 — Polish

These items improve completeness and edge-case handling.

---

### 7.24 — Fix CSI u (restore cursor) and implement CSI s (save cursor)

- **Status:** Done
- **Priority:** 4 — Low
- **Scope:** `freminal-terminal-emulator/src/ansi_components/csi.rs` (dispatch table)
- **Details:**
  - CSI s (SCOSC): Save cursor position. No `b's'` arm exists in dispatch.
  - CSI u (SCORC): Restore cursor position. Currently mapped to Kitty keyboard protocol
    handler which always returns `Skipped`, blocking the standard ANSI function.
  - Fix CSI u: Check if Kitty keyboard protocol is actually in use. If not, treat as SCORC.
    Alternatively, use a different detection method (Kitty uses `CSI > u` with `>` prefix).
  - Add CSI s arm for save cursor.
  - Note: CSI s is also used for DECSLRM (set left/right margins) when DECLRMM (?69) is
    enabled. Since we don't support DECLRMM, treating CSI s as SCOSC is safe.
- **Acceptance criteria:**
  - CSI s saves cursor position
  - CSI u restores cursor position (when Kitty protocol not active)
  - Kitty keyboard protocol detection still works if needed
- **Tests required:**
  - CSI s / CSI u round-trip cursor position
  - CSI u without prior CSI s is handled gracefully

---

### 7.25 — Implement REP (CSI b)

- **Status:** Done
- **Priority:** 4 — Low
- **Scope:** `freminal-terminal-emulator/src/ansi_components/csi.rs`, `freminal-buffer/src/terminal_handler.rs`
- **Details:**
  - REP (CSI Ps b): Repeat the preceding graphic character Ps times
  - Need to track the last graphic character written
  - Add `b'b'` arm to CSI dispatch
  - Repeat the character with the same attributes
- **Acceptance criteria:**
  - `A CSI 5 b` produces `AAAAAA` (A + 5 repeats)
  - `CSI b` (default) repeats once
  - Repeats use same SGR attributes as original character
  - Line wrapping works correctly during repeats
- **Tests required:**
  - REP with explicit count
  - REP with default count
  - REP preserves attributes
  - REP wraps at line end if DECAWM is set

---

### 7.26 — Implement HPA (CSI \`)

- **Status:** Done
- **Priority:** 4 — Low
- **Scope:** `freminal-terminal-emulator/src/ansi_components/csi.rs`
- **Details:**
  - HPA (CSI Ps \`): Move cursor to column Ps (1-based). Same as CHA (CSI G).
  - Add `b'\x60'` (backtick) arm to CSI dispatch, reusing CHA handler
- **Acceptance criteria:**
  - `CSI 10 \`` moves cursor to column 10
  - Behaves identically to CHA (CSI G)
- **Tests required:**
  - HPA moves to correct column
  - HPA with default (column 1)

---

### 7.27 — Implement OSC 4/104 (palette set/reset)

- **Status:** Not Started
- **Priority:** 4 — Low
- **Scope:** `freminal-terminal-emulator/src/ansi_components/osc.rs`, color palette in buffer/renderer
- **Details:**
  - OSC 4 ; index ; spec: Set palette color at index to spec (rgb:RR/GG/BB format)
  - OSC 4 ; index ; ?: Query palette color at index
  - OSC 104 ; index: Reset palette color at index to default
  - OSC 104 (no index): Reset all palette colors to defaults
  - Requires a mutable 256-color palette in the terminal state
- **Acceptance criteria:**
  - OSC 4 sets palette entry to specified color
  - OSC 4 query responds with current palette color
  - OSC 104 resets specific or all palette entries
- **Tests required:**
  - Set and query a palette entry
  - Reset individual entry
  - Reset all entries

---

### 7.28 — Implement DECALN (ESC # 8)

- **Status:** Done
- **Priority:** 4 — Low
- **Scope:** `freminal-terminal-emulator/src/ansi_components/standard.rs`, `freminal-buffer/src/buffer.rs`
- **Details:**
  - DECALN (Screen Alignment Test): Fill entire screen with 'E' characters
  - Currently a stub
  - Fix: Implement by filling all cells in the visible screen with 'E' using default attributes
  - Also resets scroll region and cursor position to home
- **Acceptance criteria:**
  - `ESC # 8` fills visible screen with 'E' characters
  - Scroll region is reset to full screen
  - Cursor is moved to home position
- **Tests required:**
  - Screen filled with 'E' after DECALN
  - Cursor at home position after DECALN
  - Scroll region reset after DECALN

---

### 7.29 — Wire ?47/?1047/?1048 (legacy alt screen variants)

- **Status:** Done
- **Priority:** 4 — Low
- **Scope:** `freminal-buffer/src/terminal_handler.rs` (mode dispatch)
- **Details:**
  - ?47: Alt Screen Buffer (no cursor save/restore) — legacy, used by some older programs
  - ?1047: Alt Screen Buffer (clear on switch) — used by some ncurses apps
  - ?1048: Save/Restore Cursor — often used in combination with ?1047
  - Currently not parsed at all (not even recognized by the mode dispatch)
  - ?1049 (which IS implemented) is equivalent to ?1048 + ?1047 combined
  - Fix: Add mode dispatch arms that reuse existing alt screen machinery
- **Acceptance criteria:**
  - ?47 h/l switches to/from alt screen without cursor save
  - ?1047 h/l switches to/from alt screen with clear
  - ?1048 h/l saves/restores cursor
  - Programs that use these legacy modes work correctly
- **Tests required:**
  - Each variant switches screen buffers
  - ?47 does not save/restore cursor
  - ?1047 clears alt screen on switch
  - ?1048 saves and restores cursor position

---

### 7.30 — Reduce OSC unknown logging severity

- **Status:** Done
- **Priority:** 4 — Low
- **Scope:** `freminal-terminal-emulator/src/ansi_components/osc.rs`
- **Details:**
  - Unknown/unrecognized OSC sequences currently produce `error!`-level logging
  - They also emit `TerminalOutput::Invalid` (double emission with the log)
  - Most terminals silently consume unknown OSC
  - Fix: Change `error!` to `debug!`, remove the `TerminalOutput::Invalid` emission
  - Unknown OSC should be silently consumed (like xterm/VTE behavior)
- **Acceptance criteria:**
  - Unknown OSC produces `debug!` log (not `error!`)
  - No `TerminalOutput::Invalid` for unknown OSC
  - Known OSC handling is not affected
- **Tests required:**
  - Unknown OSC does not produce Invalid output
  - Known OSC still works correctly

---

## Affected Files

| File                                                         | Change Type                                   |
| ------------------------------------------------------------ | --------------------------------------------- |
| `freminal-terminal-emulator/src/ansi.rs`                     | C0 dispatch (HT, VT, FF, NUL, DEL)            |
| `freminal-terminal-emulator/src/ansi_components/csi.rs`      | CSI dispatch (DL, CNL, CPL, SU, SD, etc.)     |
| `freminal-terminal-emulator/src/ansi_components/osc.rs`      | OSC 52, OSC 7, OSC 133, logging severity      |
| `freminal-terminal-emulator/src/ansi_components/standard.rs` | ESC handlers (RIS, DECPAM/DECPNM, HTS, DCS)   |
| `freminal-buffer/src/buffer.rs`                              | Tab stops, scroll, DECSTBM fix, DECALN        |
| `freminal-buffer/src/terminal_handler.rs`                    | Mode dispatch, DEC modes, DSR, RIS            |
| `freminal-common/src/buffer_states/mode.rs`                  | TerminalModes fields, new mode enums          |
| `freminal-common/src/buffer_states/osc.rs`                   | OscTarget for OSC 52, CWD, FTCS               |
| `freminal-common/src/buffer_states/terminal_output.rs`       | New TerminalOutput variants (clipboard, etc.) |
| `freminal-terminal-emulator/src/state/internal.rs`           | TerminalState fields (CWD, modes)             |

---

## Risk Assessment

| Risk                                        | Likelihood | Impact | Mitigation                                       |
| ------------------------------------------- | ---------- | ------ | ------------------------------------------------ |
| DECSTBM fix breaks existing scroll behavior | Medium     | High   | Thorough testing, vttest verification            |
| Mode wiring introduces state sync issues    | Medium     | Medium | Test mode persistence across snapshots           |
| Tab stop impl affects performance           | Low        | Low    | BTreeSet for sparse storage, benchmark if needed |
| OSC 52 clipboard security concerns          | Medium     | Medium | Consider user prompt or config option to enable  |
| Breaking CSI u / Kitty keyboard conflict    | Medium     | Medium | Detect Kitty protocol activation to disambiguate |
| DEC mode changes affect apps unexpectedly   | Low        | Medium | Incremental rollout, test with vim/tmux/htop     |

---

## Verification

After each subtask, run:

```bash
cargo test --all
cargo clippy --all-targets --all-features -- -D warnings
cargo machete
```

For subtasks touching rendering, buffer, or PTY code, also run benchmarks:

```bash
cargo bench
```

For Priority 1 items, verify with vttest after completion:

```bash
# Run vttest in the terminal and check cursor movement tests
vttest
```

---

## Execution Notes

- Subtasks within each priority tier can generally be done in any order
- Priority 1 should be completed first as it fixes the most critical issues
- Within Priority 2, subtasks 7.7–7.10 (mode wiring) share a common pattern and can be
  done as a batch
- Subtask 7.3 (tab stops) is a prerequisite for 7.21 (HTS/TBC/CHT/CBT)
- Subtask 7.13 (RIS) should include resetting tab stops if 7.3 is complete

---

© 2025 Freminal Project — MIT License.
