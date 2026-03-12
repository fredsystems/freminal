# PLAN_10_VTTEST_CURSOR_MOVEMENT.md — vttest Cursor Movement Fixes

## Overview

This plan addresses failures in vttest's "Test of cursor movements" suite (menu item 1,
sub-tests 1-5). Test 6 (leading zeros in ESC sequences) already passes.

Three major bugs and one minor issue were identified through byte-level analysis of the
vttest control sequences.

---

## Bug Summary

| #   | Bug                                                 | Severity | Affects       | Root Cause                                                                      |
| --- | --------------------------------------------------- | -------- | ------------- | ------------------------------------------------------------------------------- |
| 1   | C0 controls inside CSI sequences abort the sequence | High     | Test 5        | C0 bytes (BS, CR, VT) fall through CSI parser predicates → Invalid              |
| 2   | DECOM (Origin Mode) not implemented                 | High     | Tests 3, 4    | `CSI?6h/6l` parsed but never acted upon; `set_cursor_pos` ignores scroll region |
| 3   | DECCOLM (132-column mode) not implemented           | High     | Tests 1, 2, 4 | `CSI?3h/3l` parsed but never acted upon; no buffer resize occurs                |
| 4   | DECSTBM single-param form rejected                  | Low      | Minor         | Parser requires exactly 2 params; `CSI 3r` rejected                             |

---

## Implementation Order

Fixes are ordered from most self-contained to most invasive:

1. **C0-in-CSI** (parser only, no state changes)
2. **DECSTBM single-param** (parser tweak, low risk)
3. **DECOM** (adds mode field + cursor offset logic)
4. **DECCOLM** (most invasive: buffer resize + PTY signal + screen clear)

---

## Subtask 1 — Fix C0 Controls Inside CSI Sequences

**Status:** Complete

**Fixes:** Test 5

**Root cause:** When `FreminalAnsiParser` is in `ParserInner::Csi` state and receives a C0
byte (0x00-0x1F), it passes the byte directly to `AnsiCsiParser::push()`. The CSI parser's
byte classifiers (`is_csi_param` 0x30-0x3F, `is_csi_intermediate` 0x20-0x2F,
`is_csi_terminator` 0x40-0x7E) all reject C0 bytes, causing:

- The CSI sequence to be aborted (`state = Invalid`)
- The C0 control to be lost (not executed)

**Per ECMA-48 §5.5:** C0 control characters received during a control sequence are executed
immediately, then the control sequence continues parsing.

**Files to change:**

- `freminal-terminal-emulator/src/ansi.rs` — In the `ParserInner::Csi` match arm (line 298),
  intercept C0 bytes before they reach the CSI parser:
  - `0x1B` (ESC): abort CSI, set `inner = ParserInner::Escape`, `continue`
  - `0x00-0x1A, 0x1C-0x1F`: call `ansi_parser_inner_empty()` to execute the control, then
    `continue` — CSI parsing resumes on the next byte

**Tests to add:** (in `ansi.rs` test module)

- `c0_bs_inside_csi` — `\x1b[2\x08C` → Backspace + CursorForward(2)
- `c0_cr_inside_csi` — `\x1b[\x0d2C` → CarriageReturn + CursorForward(2)
- `c0_vt_inside_csi` — `\x1b[1\x0bA` → Newline + CursorUp(1)
- `c0_nul_inside_csi` — `\x1b[1\x00A` → NUL silently ignored, CursorUp(1)
- `c0_esc_inside_csi_aborts` — `\x1b[2\x1b[1A` → CSI aborted, new CSI 1A = CursorUp(1)

**Verify:** `cargo test --all` passes.

---

## Subtask 2 — Fix DECSTBM Single-Param Form

**Status:** Complete

**Fixes:** Minor edge case (vttest sends `CSI r` with no params for reset, which already
works; the single-param form `CSI 3r` is less common but should be valid per spec).

**Root cause:** `decstbm.rs` line 60 rejects any input that doesn't have exactly 2 params.
Per the DEC VT spec, `CSI Pt r` (single param) should set top margin to `Pt` and bottom
margin to the page size.

**Files to change:**

- `freminal-terminal-emulator/src/ansi_components/csi_commands/decstbm.rs` — Change the
  `params.len() != 2` check to allow 1-param form: treat missing `Pb` as `usize::MAX`.

**Also:** After `set_scroll_region`, xterm homes the cursor to position (1,1) — or to
(1,1) relative to the scroll region if DECOM is active. The current `Buffer::set_scroll_region`
already does `set_cursor_pos(Some(0), Some(top))` which puts the cursor at the scroll region
top. This should be `set_cursor_pos(Some(0), Some(0))` for absolute mode (DECOM off), or
`set_cursor_pos(Some(0), Some(scroll_region_top))` for DECOM on. This will be addressed
as part of Subtask 3 (DECOM).

**Tests to add:**

- `decstbm_single_param` — `CSI 3r` should set top=3, bottom=page size

**Verify:** `cargo test --all` passes.

---

## Subtask 3 — Implement DECOM (Origin Mode)

**Status:** Complete

**Fixes:** Tests 3, 4

**Root cause:** `CSI?6h`/`CSI?6l` is parsed to `Mode::Decom(OriginMode/NormalCursor)` but
falls into the `other =>` catch-all in `terminal_handler.rs`. No `decom` field exists on
`Buffer`, so `set_cursor_pos` never offsets by the scroll region.

**Specification (DEC VT100/VT220 manual + xterm):**

When DECOM is set (`CSI?6h`):

- CUP/HVP coordinates are relative to the scroll region (row 1 = scroll_region_top)
- Cursor is constrained to the scroll region vertically
- Cursor moves to (1,1) of the scroll region (i.e., scroll_region_top, col 0)

When DECOM is reset (`CSI?6l`):

- CUP/HVP coordinates are absolute (row 1 = screen top)
- Cursor is not constrained to the scroll region
- Cursor moves to (1,1) absolute

**Files to change:**

1. **`freminal-buffer/src/buffer.rs`**:
   - Add `decom_enabled: bool` field to `Buffer` (default `false`)
   - Add `pub fn set_decom(&mut self, enabled: bool)` — sets the field and homes the cursor:
     if enabled, cursor → `(0, scroll_region_top)`; if disabled, cursor → `(0, 0)`
   - Modify `set_cursor_pos`: when `decom_enabled && y.is_some()`:
     - Offset `y` by `scroll_region_top`
     - Clamp to `[scroll_region_top, scroll_region_bottom]`
   - Modify `set_scroll_region`: cursor home position should respect DECOM
   - Include `decom_enabled` in `SavedPrimaryState` (save/restore on alternate screen)
   - Include `decom_enabled` in saved cursor state

2. **`freminal-buffer/src/terminal_handler.rs`**:
   - Add match arms for `Mode::Decom(Decom::OriginMode)` and `Mode::Decom(Decom::NormalCursor)`
     in the mode dispatch (currently the `other =>` catch-all)
   - Call `self.buffer.set_decom(true/false)`

3. **Tests** — Expand `freminal-terminal-emulator/tests/modes_decom.rs` or add buffer-level
   tests in `freminal-buffer/tests/`:
   - CUP with DECOM on: `CSI?6h` + `CSI 3;21r` + `CSI 1;1H` → cursor at
     (scroll_region_top, 0) not (0, 0)
   - CUP with DECOM off: standard absolute positioning
   - Cursor clamped to scroll region when DECOM is on
   - DECOM reset homes cursor to (0,0) absolute

**Verify:** `cargo test --all` passes.

---

## Subtask 4 — Implement DECCOLM (132-Column Mode)

**Status:** Complete

**Fixes:** Tests 1, 2, 4

**Root cause:** `CSI?3h`/`CSI?3l` is parsed to `Mode::Deccolm(Column132/Column80)` but
falls into the `other =>` catch-all in `terminal_handler.rs`. No column resize occurs.

**Specification (DEC VT100/xterm):**

When DECCOLM is set (`CSI?3h` → 132-column mode):

- Terminal switches to 132 columns wide
- Screen is cleared
- Scroll region is reset to full screen
- Cursor moves to (1,1)
- DECOM is reset

When DECCOLM is reset (`CSI?3l` → 80-column mode):

- Same effects but switches to 80 columns

**Important:** xterm gates DECCOLM behind `CSI?40h` (AllowColumnModeSwitch). The
`AllowColumnModeSwitch` type already exists. For vttest compatibility, we should default to
allowing column mode switching (the existing `AllowColumnModeSwitch` default is
`AllowColumnModeSwitch` which is correct).

**Files to change:**

1. **`freminal-buffer/src/buffer.rs`**:
   - Add `pub fn set_column_mode(&mut self, columns: usize)` — resizes width to the
     specified column count, clears screen, resets scroll region, resets DECOM, homes cursor

2. **`freminal-buffer/src/terminal_handler.rs`**:
   - Add `allow_column_mode_switch: bool` field (default `true`)
   - Add match arms for `Mode::Deccolm(Deccolm::Column132)` and
     `Mode::Deccolm(Deccolm::Column80)` in the mode dispatch
   - Guard behind `allow_column_mode_switch`
   - Add match arms for `Mode::AllowColumnModeSwitch(...)` to set the guard flag
   - The handler needs to signal back to the PTY that the terminal size changed. This
     requires sending a `PtyWrite::Resize` through the write channel. The handler already
     has `write_tx: Option<Sender<PtyWrite>>`.

3. **Tests:**
   - DECCOLM set: verify buffer width changes to 132, screen cleared, cursor at (0,0)
   - DECCOLM reset: verify buffer width changes to 80
   - DECCOLM blocked by AllowColumnModeSwitch=false

**Verify:** `cargo test --all` passes.

---

## Verification

After all subtasks complete:

1. `cargo test --all` — all tests pass
2. `cargo clippy --all-targets --all-features -- -D warnings` — clean
3. `cargo-machete` — no unused dependencies

---

## Progress

- [x] Subtask 1 — C0 controls inside CSI sequences
  - Completed 2026-03-12. commit dd6fde4
- [x] Subtask 2 — DECSTBM single-param form
  - Completed 2026-03-12. commit 056a1ea
- [x] Subtask 3 — DECOM (Origin Mode)
  - Completed 2026-03-12. commit 33db573
- [x] Subtask 4 — DECCOLM (132-column mode)
  - Completed 2026-03-12. commit 33db573
- [x] Final verification suite
  - cargo test --all: 640 tests, 0 failed. clippy: clean. cargo-machete: clean.
