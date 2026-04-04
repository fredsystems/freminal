# PLAN_22 — vttest Integration Testing

## Status: Pending

---

## Overview

vttest is the de facto compliance test suite for terminal emulators, covering cursor movement,
screen features, character sets, double-sized characters, keyboard, device reports, VT52 mode,
insert/delete operations, known VT100 bugs, and non-VT100 extensions. Freminal's vttest
compliance is currently evaluated manually — there are no automated tests that exercise vttest
scenarios.

This task creates a two-tier automated test infrastructure:

1. **Individual escape sequence variant tests** — Unit-level tests for each escape sequence
   exercised by vttest, verifying buffer state after specific operations. These are fast,
   hermetic, and catch regressions precisely.

2. **Full-screen buffer comparison tests** — Feed the same escape sequence payloads that vttest
   sends, then compare the resulting buffer state against golden reference snapshots. These
   catch rendering-level regressions that individual tests might miss.

The vttest source is available at `vttest-20251205/` for reference but tests do NOT depend on
running the vttest binary — they replay known escape sequences directly into
`TerminalState`/`Buffer`.

**Dependencies:** None (independent; benefits from Task 20 DEC mode coverage being complete)
**Dependents:** None
**Primary crates:** `freminal-buffer`, `freminal-terminal-emulator`
**Estimated scope:** Large (8 subtasks)

---

## vttest Menu Classification

Every vttest test has been classified for relevance to Freminal and automation potential.

### Classification Key

| Code     | Meaning                                                                                     |
| -------- | ------------------------------------------------------------------------------------------- |
| `[A]`    | Fully automatable — deterministic escape sequences, buffer-verifiable output                |
| `[I]`    | Interactive verification needed — visual check required (but input sequence is automatable) |
| `[V]`    | Visual only — must be checked visually, cannot be buffer-compared meaningfully              |
| `[SKIP]` | Not relevant for Freminal (hardware modes, VT52, printer, Tektronix, etc.)                  |

### Menu 1 — Cursor Movements (All `[I]` but input is automatable)

All tests in this menu send deterministic escape sequences and produce predictable screen
content. The "interactive" aspect is visual verification — but the buffer state IS verifiable.

| Test                | Sequences Exercised        | Classification | Notes                      |
| ------------------- | -------------------------- | -------------- | -------------------------- |
| 1.1 CUF/CUB/CUU/CUD | `CSI A/B/C/D` with params  | `[A]`          | Buffer position verifiable |
| 1.2 CUP/HVP         | `CSI H`, `CSI f`           | `[A]`          | Buffer position verifiable |
| 1.3 ED              | `CSI J` (0, 1, 2)          | `[A]`          | Buffer content verifiable  |
| 1.4 EL              | `CSI K` (0, 1, 2)          | `[A]`          | Buffer content verifiable  |
| 1.5 DECALN          | `ESC # 8`                  | `[A]`          | Screen fills with 'E'      |
| 1.6 DECAWM          | `CSI ? 7 h/l` + long lines | `[A]`          | Wrap/no-wrap verifiable    |
| 1.7 IND/NEL/RI      | `ESC D`, `ESC E`, `ESC M`  | `[A]`          | Scroll + cursor verifiable |
| 1.8 Scroll region   | DECSTBM + scroll ops       | `[A]`          | Buffer content verifiable  |

### Menu 2 — Screen Features (All `[I]` but input is automatable)

| Test               | Sequences Exercised      | Classification | Notes                          |
| ------------------ | ------------------------ | -------------- | ------------------------------ |
| 2.1 DECSTBM        | Scroll region boundaries | `[A]`          | Buffer content verifiable      |
| 2.2 TBC + HTS      | Tab stops                | `[A]`          | Cursor position verifiable     |
| 2.3 DECCOLM        | 80/132 column switch     | `[A]`          | Buffer width verifiable        |
| 2.4 DECSCLM        | Smooth scroll            | `[SKIP]`       | No visible effect at 60+ fps   |
| 2.5 DECSCNM        | Screen invert            | `[V]`          | Visual only — color inversion  |
| 2.6 DECOM          | Origin mode              | `[A]`          | Cursor position verifiable     |
| 2.7 DECAWM         | Auto-wrap test           | `[A]`          | Buffer content verifiable      |
| 2.8 SGR            | Character attributes     | `[A]`          | FormatTag verifiable           |
| 2.9 DECSC/DECRC    | Save/restore cursor      | `[A]`          | Cursor + attributes verifiable |
| 2.10 Blinking text | SGR 5                    | `[V]`          | Visual only — blink animation  |

### Menu 3 — Character Sets

| Test                  | Sequences Exercised      | Classification | Notes                                        |
| --------------------- | ------------------------ | -------------- | -------------------------------------------- |
| 3.1 G0/G1 (SCS/SI/SO) | `ESC ( 0/B`, SI, SO      | `[A]`          | Cell content verifiable (DEC graphics chars) |
| 3.2 VT220 shifts      | SS2, SS3, locking shifts | `[SKIP]`       | G2/G3 not implemented                        |
| 3.3 NRC sets          | National character sets  | `[SKIP]`       | Requires DECNRCM (Task 20.12)                |
| 3.4 ISO Latin         | ISO character sets       | `[SKIP]`       | Not implemented                              |

### Menu 4 — Double-Sized Characters

| Test       | Sequences Exercised | Classification | Notes                                      |
| ---------- | ------------------- | -------------- | ------------------------------------------ |
| 4.1 DECDWL | `ESC # 6`           | `[V]`          | Renderer doesn't support double-width yet  |
| 4.2 DECDHL | `ESC # 3/4`         | `[V]`          | Renderer doesn't support double-height yet |
| 4.3 DECSWL | `ESC # 5`           | `[A]`          | Single-width is default behavior           |

### Menu 5 — Keyboard

| Test                | Sequences Exercised         | Classification | Notes                                   |
| ------------------- | --------------------------- | -------------- | --------------------------------------- |
| 5.1 LEDs            | DECLL                       | `[SKIP]`       | No LED support                          |
| 5.2 DECARM          | Auto-repeat                 | `[SKIP]`       | OS-level repeat; not testable in buffer |
| 5.3 DECCKM          | Cursor key mode             | `[A]`          | Key encoding verifiable                 |
| 5.4 DECKPAM/DECKPNM | Keypad mode                 | `[A]`          | Key encoding verifiable                 |
| 5.5 Editing keys    | Function keys, Delete, etc. | `[A]`          | Key encoding verifiable                 |
| 5.6 Answerback      | ENQ response                | `[SKIP]`       | Not implementing answerback             |

### Menu 6 — Reports (Many `[A]`)

| Test            | Sequences Exercised         | Classification | Notes                     |
| --------------- | --------------------------- | -------------- | ------------------------- |
| 6.1 DSR (Ps=5)  | Device Status Report        | `[A]`          | Response bytes verifiable |
| 6.2 DSR (Ps=6)  | Cursor Position Report      | `[A]`          | Response bytes verifiable |
| 6.3 DA1         | Primary Device Attributes   | `[A]`          | Response bytes verifiable |
| 6.4 DA2         | Secondary Device Attributes | `[A]`          | Response bytes verifiable |
| 6.5 DA3         | Tertiary Device Attributes  | `[A]`          | Response bytes verifiable |
| 6.6 DECREQTPARM | Request Terminal Parameters | `[A]`          | Response bytes verifiable |

### Menu 7 — VT52 Mode

All tests: `[SKIP]` — VT52 mode not implemented (planned in Task 20.8).

### Menu 8 — Insert/Delete Operations

| Test    | Sequences Exercised | Classification | Notes                     |
| ------- | ------------------- | -------------- | ------------------------- |
| 8.1 IRM | Insert/Replace mode | `[A]`          | Buffer content verifiable |
| 8.2 ICH | Insert Characters   | `[A]`          | Buffer content verifiable |
| 8.3 DCH | Delete Characters   | `[A]`          | Buffer content verifiable |
| 8.4 IL  | Insert Lines        | `[A]`          | Buffer content verifiable |
| 8.5 DL  | Delete Lines        | `[A]`          | Buffer content verifiable |

### Menu 9 — Known VT100 Bugs

All tests are regression checks for historical VT100 firmware bugs. All `[A]` — they test
specific escape sequence edge cases that have deterministic buffer outcomes.

### Menu 10 — Reset and Self-Test

| Test        | Sequences Exercised | Classification | Notes                               |
| ----------- | ------------------- | -------------- | ----------------------------------- |
| 10.1 RIS    | `ESC c`             | `[A]`          | Buffer state verifiable after reset |
| 10.2 DECTST | Self-test           | `[SKIP]`       | Hardware diagnostic                 |

### Menu 11 — Non-VT100 Features

This is a large submenu tree. Key subtests:

| Test                  | Sequences Exercised        | Classification | Notes                           |
| --------------------- | -------------------------- | -------------- | ------------------------------- |
| 11.1 ECMA-48 cursor   | CNL, CPL, HPA, VPA, CHA    | `[A]`          | All implemented and verifiable  |
| 11.2 ECMA-48 misc     | SU, SD, ECH, REP, CBT, CHT | `[A]`          | All implemented and verifiable  |
| 11.3 DECSCUSR         | Cursor style               | `[A]`          | Style enum verifiable           |
| 11.4 DECTCEM          | Cursor visibility          | `[A]`          | show_cursor flag verifiable     |
| 11.5 Colors (256/RGB) | SGR 38/48 with params      | `[A]`          | FormatTag color verifiable      |
| 11.6 BCE              | Background color erase     | `[A]`          | Erased cells inherit current bg |
| 11.7 Alt screen       | `?1049` / `?47` / `?1047`  | `[A]`          | Buffer state verifiable         |
| 11.8 Bracketed paste  | `?2004`                    | `[A]`          | Mode flag verifiable            |
| 11.9 Mouse tracking   | `?1000` etc.               | `[SKIP]`       | Requires GUI interaction        |
| 11.10 Xterm window    | Window manipulation        | `[SKIP]`       | Requires GUI interaction        |

---

## Subtasks

---

### 22.1 — Test Infrastructure: Golden Buffer Comparison Framework

- **Status:** Complete
- **Priority:** 1 — High
- **Scope:** `freminal-terminal-emulator/tests/vttest_common.rs` (new),
  `freminal-terminal-emulator/tests/golden/` (new directory)
- **Details:**
  Create a test helper module that:
  1. Constructs a `TerminalState` with a given size (default 80x24).
  2. Feeds a byte sequence via `handle_incoming_data()`.
  3. Extracts the visible buffer as a grid of characters (Vec of String, one per row).
  4. Extracts format tags for the visible region.
  5. Compares against a "golden" reference stored as a `.txt` file in `tests/golden/`.
  6. On mismatch, prints a clear diff showing expected vs actual, row by row.
  7. Provides a `UPDATE_GOLDEN=1` environment variable mode that writes actual output as the
     new golden file (for initial creation and intentional changes).

  The helper should handle:
  - Trailing whitespace normalization (terminal rows are padded with spaces).
  - Cursor position assertion (separate from content).
  - Optional format tag assertion (for SGR-focused tests).

- **Acceptance criteria:**
  - `VtTestHelper::new(80, 24)` creates a usable test terminal.
  - `helper.feed(bytes)` processes escape sequences.
  - `helper.assert_screen("test_name")` compares against `tests/golden/test_name.txt`.
  - `UPDATE_GOLDEN=1 cargo test` regenerates golden files.
  - Clear, readable diff output on mismatch.
- **Tests required:**
  - Self-test: a trivial golden comparison (feed "Hello", compare).
  - Self-test: a deliberate mismatch produces readable error.

---

### 22.2 — Menu 1: Cursor Movement Tests

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** `freminal-terminal-emulator/tests/vttest_cursor.rs` (new)
- **Details:**
  Implement individual tests for each cursor movement operation as exercised by vttest Menu 1:
  - CUF/CUB/CUU/CUD with various Ps values including 0 (default), 1, and large values.
  - CUP/HVP with absolute positioning, including out-of-bounds clamping.
  - ED Ps=0 (erase to end), Ps=1 (erase to beginning), Ps=2 (erase all).
  - EL Ps=0/1/2.
  - DECALN fills entire screen with 'E'.
  - DECAWM wrap and no-wrap behavior.
  - IND at bottom of screen (scrolls up), NEL (CR+LF), RI at top (scrolls down).
  - DECSTBM scroll region: scroll within region, cursor clamping at boundaries.

  Each test feeds the exact escape sequence and asserts buffer content and cursor position.
  Also create golden buffer snapshots for the composite vttest screens.

- **Acceptance criteria:**
  - All cursor movement operations have individual unit tests.
  - Golden buffer comparisons pass for vttest Menu 1 screens.
  - At least 20 individual test cases.
- **Tests required:** This subtask IS the tests.

---

### 22.3 — Menu 2: Screen Feature Tests

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** `freminal-terminal-emulator/tests/vttest_screen.rs` (new)
- **Details:**
  Test vttest Menu 2 scenarios:
  - DECSTBM with various region sizes, including full-screen and single-line regions.
  - TBC + HTS: set custom tab stops, clear specific/all, verify CHT advances correctly.
  - DECCOLM: switch to 132 columns, verify buffer width changes.
  - DECOM: origin mode cursor positioning relative to scroll region.
  - DECAWM: auto-wrap at right margin, reverse wrap at left margin.
  - SGR: bold, underline, inverse, strikethrough, colors — verify FormatTag fields.
  - DECSC/DECRC: save cursor position + attributes, modify both, restore, verify.

  **Note on blinking text (2.10):** SGR 5 (slow blink) and SGR 6 (fast blink) are parsed but
  silently discarded. The vttest screen for this will show all text as non-blinking. This is
  a known gap addressed in Task 23 (PLAN_23_BLINKING_TEXT.md). The golden snapshot for this
  screen should document the expected visual discrepancy.

- **Acceptance criteria:**
  - All screen feature operations have tests.
  - Golden snapshots for vttest Menu 2 screens.
  - At least 15 individual test cases.
- **Tests required:** This subtask IS the tests.

---

### 22.4 — Menu 6: Device Report Tests

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** `freminal-terminal-emulator/tests/vttest_reports.rs` (new)
- **Details:**
  Device reports are the most cleanly automatable vttest tests because they have deterministic
  byte-level responses. Test:
  - DSR Ps=5: response is `CSI 0 n` ("terminal OK").
  - DSR Ps=6: response is `CSI Pr ; Pc R` with correct cursor position.
  - DA1: response matches Freminal's device attributes string.
  - DA2: response matches Freminal's secondary device attributes.
  - DECRQM for all implemented modes: verify response byte pattern.

  These tests feed the query sequence and capture the bytes written back to the PTY channel,
  then assert the exact response.

- **Acceptance criteria:**
  - All report queries produce correct response bytes.
  - Response bytes match the format expected by vttest.
  - At least 10 test cases (DSR + DA + multiple DECRQM queries).
- **Tests required:** This subtask IS the tests.

---

### 22.5 — Menu 8: Insert/Delete Operation Tests

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** `freminal-terminal-emulator/tests/vttest_insert_delete.rs` (new)
- **Details:**
  Test vttest Menu 8 insert/delete operations:
  - ICH: insert N characters at cursor, shifting existing content right.
  - DCH: delete N characters at cursor, shifting remaining content left.
  - IL: insert N blank lines at cursor row, shifting content down.
  - DL: delete N lines at cursor row, shifting content up.
  - IRM: toggle insert mode, verify character insertion vs replacement.

  Each test creates specific buffer content, applies the operation, and verifies the resulting
  buffer state character-by-character.

- **Acceptance criteria:**
  - All insert/delete operations have individual tests.
  - Tests cover edge cases: at margins, within scroll regions, with count > available space.
  - At least 15 test cases.
- **Tests required:** This subtask IS the tests.

---

### 22.6 — Menu 9: VT100 Known Bug Regression Tests

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-terminal-emulator/tests/vttest_bugs.rs` (new)
- **Details:**
  vttest Menu 9 contains regression tests for specific VT100 firmware bugs. Each test sends
  a specific sequence that triggered a bug in the original VT100 hardware and verifies the
  correct behavior. These are valuable regression tests because the same edge cases trip up
  software terminal emulators.

  Specific bugs tested:
  - Wrap at column 80 (bug: cursor wraps to wrong position).
  - Tab after wrap (bug: tab at right margin moves to wrong column).
  - ED after wrap (bug: erase doesn't affect wrapped line correctly).
  - Various scroll region + cursor movement interactions.

  Each test sends the exact sequence and compares against the golden reference.

- **Acceptance criteria:**
  - All Menu 9 regression tests have automated equivalents.
  - Tests document which VT100 bug they are testing.
- **Tests required:** This subtask IS the tests.

---

### 22.7 — Menu 11: Non-VT100 Extension Tests

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-terminal-emulator/tests/vttest_extensions.rs` (new)
- **Details:**
  Test non-VT100 extensions from vttest Menu 11:
  - ECMA-48 cursor commands: CNL, CPL, HPA, VPA, CHA — verify absolute positioning.
  - ECMA-48 misc: SU (scroll up), SD (scroll down), ECH, REP, CBT, CHT.
  - DECSCUSR: all cursor styles (block, underline, bar, blinking/steady).
  - DECTCEM: show/hide cursor flag.
  - 256-color and RGB color: SGR 38;5;N, SGR 38;2;R;G;B, SGR 48 variants.
  - BCE: erase operations inherit current background color.
  - Alternate screen: enter/leave, content preserved, cursor saved/restored.
  - Bracketed paste: mode flag set/reset correctly.

- **Acceptance criteria:**
  - Non-VT100 extensions exercised by vttest have automated tests.
  - At least 20 test cases covering the breadth of Menu 11.
- **Tests required:** This subtask IS the tests.

---

### 22.8 — Character Set Tests (G0/DEC Special Graphics)

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-terminal-emulator/tests/vttest_charsets.rs` (new)
- **Details:**
  Test vttest Menu 3 character set functionality (the subset Freminal implements):
  - `ESC ( 0` activates DEC Special Graphics for G0. Verify that characters in the 0x60-0x7E
    range render as line drawing characters (stored as Unicode equivalents in cells).
  - `ESC ( B` restores US ASCII for G0.
  - SI/SO invoke G0/G1 respectively (G1 rendering is currently a no-op).
  - Verify the complete DEC Special Graphics mapping table (all 31 characters).
  - Verify that characters outside the 0x60-0x7E range are unaffected by G0 designation.

- **Acceptance criteria:**
  - DEC Special Graphics character mapping is fully tested.
  - G0 designation switching works correctly.
  - All 31 line drawing characters map to correct Unicode code points.
- **Tests required:** This subtask IS the tests.

---

## Implementation Notes

### Subtask Ordering

22.1 (infrastructure) must be completed first — all other subtasks depend on it.
22.2-22.8 are independent and can be done in any order or in parallel.

**Recommended order:** 22.1 → 22.4 (reports, easiest) → 22.2 (cursor) → 22.5 (insert/delete)
→ 22.3 (screen) → 22.8 (charsets) → 22.6 (bugs) → 22.7 (extensions)

### Test File Organization

```text
freminal-terminal-emulator/
  tests/
    vttest_common.rs          — shared test helpers
    vttest_cursor.rs          — Menu 1 tests
    vttest_screen.rs          — Menu 2 tests
    vttest_charsets.rs        — Menu 3 tests
    vttest_reports.rs         — Menu 6 tests
    vttest_insert_delete.rs   — Menu 8 tests
    vttest_bugs.rs            — Menu 9 tests
    vttest_extensions.rs      — Menu 11 tests
    golden/                   — golden reference files
      cursor_movement_*.txt
      screen_features_*.txt
      ...
```

### Tests That Are NOT Appropriate for Automation

The following vttest tests are explicitly excluded from automation:

- **Menu 4** (double-sized characters): DECDWL/DECDHL rendering is not implemented. Tests would
  only verify that the escape sequences are parsed without error, which is already covered.
- **Menu 5** (keyboard): Requires actual key input events, not escape sequence replay. Key
  encoding is already tested in the input encoding test suite.
- **Menu 7** (VT52 mode): VT52 mode is not implemented (Task 20.8). Tests would all fail.
- **Menu 10.2** (DECTST self-test): Hardware diagnostic, not applicable.
- **Menu 11.9** (mouse tracking): Requires GUI mouse events.
- **Menu 11.10** (window manipulation): Requires GUI window context.
- **Any test requiring user visual confirmation of blinking**: Blink is not implemented (Task 23).

### Verification

Each subtask must pass before proceeding:

- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo-machete`

---

## References

- `vttest-20251205/` — vttest source (reference for escape sequences used)
- `vttest-20251205/main.c` — menu structure and test descriptions
- `freminal-terminal-emulator/tests/` — existing test infrastructure
- `freminal-buffer/tests/terminal_handler_integration.rs` — existing integration tests
- <https://invisible-island.net/vttest/vttest.html> — vttest documentation
