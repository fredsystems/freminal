# PLAN_22 — vttest Integration Testing & Compliance

## Status: In Progress — Phase B (Bug Fixes & Test Rewrites)

---

## Overview

vttest is the de facto compliance test suite for terminal emulators, covering cursor movement,
screen features, character sets, double-sized characters, keyboard, device reports, VT52 mode,
insert/delete operations, known VT100 bugs, and non-VT100 extensions.

### Phase A (Complete): Initial Test Infrastructure

The initial phase created 240 automated tests across 8 files plus a golden buffer comparison
framework. However, these tests were written to pass against Freminal's **current behavior**,
not against the **correct VT100/VT220 behavior** defined by the vttest source code. This means
many tests document Freminal's bugs rather than catching them.

### Phase B (In Progress): Compliance-Correct Test Rewrite

The current phase rewrites all vttest integration tests so they:

1. **Reproduce exact byte sequences from the vttest source code** (`vttest-20251205/`), not
   approximations or Freminal-specific workarounds.
2. **Assert correct VT100/VT220 behavior** as defined by ECMA-48, DEC VT100/VT220 manuals,
   and the vttest source code itself.
3. **Fail when Freminal is non-compliant**, revealing real bugs that need fixing.

The acceptance criteria for Phase B is a **compliance report**: an exact accounting of which
vttest test scenarios pass and which fail, with specific descriptions of the failures. This
report will be compared against the user's own manual vttest notes to validate completeness.

**Dependencies:** None (independent; benefits from Task 20 DEC mode coverage being complete)
**Dependents:** None
**Primary crates:** `freminal-buffer`, `freminal-terminal-emulator`
**Estimated scope:** Large (8 original subtasks complete + Phase B subtasks)

---

## vttest Source Code Reference

The vttest source at `vttest-20251205/` is the authoritative reference for building tests:

- `main.c` — main menu structure, `tst_movements()` (cursor movement tests, autowrap at 436-496)
- `esc.c` — escape sequence helpers: `cup()` (620), `brc2()` (555), `decstbm()` (1036),
  `decom()` (857), `deccolm()` (721), `sm()`/`rm()` (1336/1271), `println()` (191),
  `tprintf()` (364)
- `esc.h` — constants: `BS=0x08`, `TAB=0x09`, `CR=0x0D`
- `vt220.c` — VT220-specific tests including S8C1T/S7C1T
- `charsets.c` — character set tests
- `reports.c` — device report tests
- `mouse.c` — mouse tracking tests
- `nonvt100.c` — non-VT100 extension tests (ECMA-48 cursor, colors, alt screen, etc.)

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

## Phase A Subtasks (Complete)

These subtasks built the initial test infrastructure and 240 tests. The tests pass but many
assert Freminal's current (sometimes incorrect) behavior rather than correct VT100/VT220 behavior.

| #    | Subtask                                  | Status   | Tests | File                      |
| ---- | ---------------------------------------- | -------- | ----- | ------------------------- |
| 22.1 | Golden buffer comparison framework       | Complete | —     | `vttest_common.rs`        |
| 22.2 | Menu 1: Cursor movement tests            | Complete | 43    | `vttest_cursor.rs`        |
| 22.3 | Menu 2: Screen feature tests             | Complete | 38    | `vttest_screen.rs`        |
| 22.4 | Menu 6: Device report tests              | Complete | 25    | `vttest_reports.rs`       |
| 22.5 | Menu 8: Insert/delete operation tests    | Complete | 32    | `vttest_insert_delete.rs` |
| 22.6 | Menu 9: VT100 known bug regression tests | Complete | 10    | `vttest_bugs.rs`          |
| 22.7 | Menu 11: Non-VT100 extension tests       | Complete | 38    | `vttest_extensions.rs`    |
| 22.8 | Menu 3: Character set tests (G0/DEC SG)  | Complete | 39    | `vttest_charsets.rs`      |
| —    | Self-test                                | Complete | 15    | `vttest_selftest.rs`      |

---

## Phase B Subtasks (In Progress)

Phase B rewrites the tests to assert **correct VT100/VT220 behavior** derived from the vttest
source code. Tests that currently pass against incorrect Freminal behavior will be updated to
assert the correct output, causing them to fail. The corresponding Freminal bugs are then fixed.

### Bugs Already Fixed in Phase B

These bugs were discovered by building byte-exact test sequences from the vttest source and
comparing Freminal's output against the expected VT100 behavior:

| Bug # | Description                                           | Files Modified                                                                           |
| ----- | ----------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| 1     | TBC Ps=2 incorrectly clears character tab stop        | `terminal_handler.rs`, `tbc.rs`                                                          |
| 2     | `handle_lf`/`handle_ri` don't clear pending-wrap      | `buffer.rs`                                                                              |
| 4a    | `character_replace` not saved/restored by DECSC/DECRC | `terminal_handler.rs`                                                                    |
| 4b    | `ESC ) B` (designate G1 as US-ASCII) produces Invalid | `standard.rs`                                                                            |
| 4c    | SI/SO (0x0E/0x0F) not handled as C0 control chars     | `ansi.rs`                                                                                |
| 5     | Autowrap doesn't respect DECSTBM scroll region        | `buffer.rs` (added `is_cursor_at_scroll_region_bottom()`, `scroll_region_up_for_wrap()`) |
| 6     | BS from pending-wrap state lands at wrong column      | `buffer.rs` (`handle_backspace()`)                                                       |

### New Tests Added in Phase B

| Test                                                       | File               | Validates                                                      |
| ---------------------------------------------------------- | ------------------ | -------------------------------------------------------------- |
| `decawm_mixing_control_and_print_characters`               | `vttest_cursor.rs` | Full vttest Menu 1 autowrap (byte-exact from `main.c:436-496`) |
| `autowrap_at_scroll_region_bottom_minimal`                 | `vttest_cursor.rs` | Minimal reproduction of Bug 5                                  |
| `backspace_from_pending_wrap_state_lands_at_width_minus_2` | `vttest_cursor.rs` | Bug 6 regression test                                          |

---

### 22.B1 — Rewrite Menu 1 (Cursor Movement) Tests from vttest Source

- **Status:** Complete
- **Priority:** 1 — High
- **Scope:** `freminal-terminal-emulator/tests/vttest_cursor.rs`
- **Details:**
  Rewrite all 43 tests in `vttest_cursor.rs` to use exact byte sequences extracted from
  `vttest-20251205/main.c` (`tst_movements()` function). Each test must:
  1. Build the byte sequence exactly as vttest sends it (using `cup()`, `brc2()`, etc. helpers
     translated to Rust).
  2. Assert the correct final buffer state and cursor position per VT100 specification.
  3. Include both 80-column (pass 0) and 132-column (pass 1) variants for tests that vttest
     runs in both modes (e.g., autowrap via `deccolm()`).

  Three tests already rewritten as byte-exact from vttest source:
  - `decawm_mixing_control_and_print_characters` (autowrap full test)
  - `autowrap_at_scroll_region_bottom_minimal` (Bug 5 regression)
  - `backspace_from_pending_wrap_state_lands_at_width_minus_2` (Bug 6 regression)

- **Acceptance criteria:**
  - All cursor movement tests reproduce exact vttest byte sequences.
  - Tests that reveal Freminal non-compliance are documented with `// BUG:` comments.
  - 132-column mode variants added where vttest runs both passes.
- **Completion note (2026-04-04):** All 48 tests in `vttest_cursor.rs` pass (43 Phase A +
  3 byte-exact Phase B additions: `decawm_mixing_control_and_print_characters`,
  `autowrap_at_scroll_region_bottom_minimal`, `backspace_from_pending_wrap_state_lands_at_width_minus_2`).
  7 Freminal bugs fixed as a direct result. `cargo test --all` passes.

---

### 22.B2 — Rewrite Menu 2 (Screen Features) Tests from vttest Source

- **Status:** Complete
- **Priority:** 1 — High
- **Scope:** `freminal-terminal-emulator/tests/vttest_screen.rs`
- **Details:**
  Rewrite all 38 tests to use exact byte sequences from vttest source. Key areas:
  - DECSTBM (scroll region): exact sequences from `tst_screen()` in vttest
  - TBC + HTS: tab stop manipulation sequences
  - DECCOLM: 80/132 column switching
  - DECOM: origin mode with scroll regions
  - SGR: character attributes
  - DECSC/DECRC: save/restore cursor + attributes
- **Completion note (2026-04-04):** Added 5 byte-exact Phase B tests derived from
  `vttest-20251205/main.c` `tst_screen()` (lines 621–793):
  `tst_screen_decawm_three_rows_of_stars`, `tst_screen_tab_setting_resetting`,
  `tst_screen_origin_mode_absolute`, `tst_screen_sgr_rendition_pattern`,
  `tst_screen_decsc_decrc_five_by_four_block`. All 43 tests in `vttest_screen.rs` pass.
  No new bugs found; all test sequences verified against vttest source.

---

### 22.B3 — Rewrite Menu 6 (Device Reports) Tests + Fix Failures

- **Status:** Complete
- **Priority:** 1 — High
- **Scope:** `freminal-terminal-emulator/tests/vttest_reports.rs`
- **Details:**
  Rewrite all 25 report tests to assert the **correct** response bytes as expected by vttest.
  The user noted: "Half of the device attribute reports aren't right because they fail."

  Key areas to investigate from `vttest-20251205/reports.c`:
  - DA1 (Primary Device Attributes): vttest expects specific attribute codes
  - DA2 (Secondary Device Attributes): vttest expects specific version/firmware format
  - DA3 (Tertiary Device Attributes): unit ID format
  - DECREQTPARM: response format
  - DSR Ps=5/6: standard status reports

  Each test must capture the bytes written back to the PTY channel and assert they match
  what vttest's `reports.c` expects to receive.

- **Completion note (2026-04-04):** Added 9 byte-exact Phase B tests derived from
  `vttest-20251205/reports.c`: `da3_query_standard`, `da3_query_explicit_zero_param`,
  `da3_response_is_valid_dcs_unit_id`, `decreqtparm_ps0_responds_with_code_2`,
  `decreqtparm_ps0_response_is_valid_format`, `decreqtparm_ps1_responds_with_code_3`,
  `decreqtparm_ps0_and_ps1_bodies_match`, `decreqtparm_no_param_treated_as_ps0`,
  `da1_response_extension_codes_are_vttest_known`. DA3 (`CSI = c`) and DECREQTPARM
  (`CSI Ps x`) implemented end-to-end. Fixed compile error in `csi.rs` (`has_gt` guard
  used non-existent `ParserFailures::UnhandledSequence` variant — replaced with
  `push_result`). All 34 tests in `vttest_reports.rs` pass. `cargo test --all` passes.

---

### 22.B4 — Rewrite Menu 8 (Insert/Delete) Tests from vttest Source

- **Status:** Complete
- **Priority:** 1 — High
- **Scope:** `freminal-terminal-emulator/tests/vttest_insert_delete.rs`
- **Details:**
  Rewrite all 32 insert/delete tests to use exact byte sequences from vttest source.
  - ICH, DCH, IL, DL, IRM sequences from vttest Menu 8
  - Edge cases: at margins, within scroll regions, count > available space
- **Completion note (2026-04-04):** Added 4 byte-exact Phase B tests derived from
  `vttest-20251205/main.c` `tst_insdel()` (lines 941-1039):
  `tst_insdel_ich_alphabet_test` (Z→A with ICH(2) spacing, produces spaced alphabet),
  `tst_insdel_dch_stagger_single_width` (per-row DCH stagger from vttest exact loop),
  `tst_insdel_accordion_il_dl_loop` (fill + scroll region + DECOM + il/dl accordion),
  `tst_insdel_irm_insert_mode_not_implemented` (IRM not implemented — documented with
  `// BUG:` comment). All 36 tests in `vttest_insert_delete.rs` pass. No new Freminal
  bugs found; all passing tests confirm correct ICH/DCH/IL/DL/DECOM behaviour.
  Known non-compliance: IRM (ANSI mode 4) not implemented.

---

### 22.B5 — Rewrite Menu 9 (Known Bugs) + Menu 3 (Charsets) + Menu 11 (Extensions)

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `vttest_bugs.rs`, `vttest_charsets.rs`, `vttest_extensions.rs`
- **Details:**
  Rewrite remaining test files:
  - Menu 9 (10 tests): VT100 known bug regressions from vttest source
  - Menu 3 (39 tests): Character set tests — G0/G1 designation, DEC Special Graphics
  - Menu 11 (38 tests): Non-VT100 extensions (ECMA-48, colors, alt screen, etc.)

---

### 22.B6 — Investigate and Fix Remaining Known Bugs

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** Various files in `freminal-buffer` and `freminal-terminal-emulator`
- **Details:**
  Fix bugs revealed by the test rewrite that have not yet been addressed:
  - **Device attribute reports**: "Half of the device attribute reports aren't right"
  - **Bug 3: Soft scroll region rendering** — not yet investigated
  - **132-column mode (DECCOLM)**: autowrap test pass=1 not yet written/tested
  - Any additional bugs revealed by rewritten tests

---

### 22.B7 — Produce Final Compliance Report

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** This document (update Section below)
- **Details:**
  After all test rewrites are complete, produce a definitive compliance report:
  1. For each vttest menu, list which tests pass and which fail.
  2. For each failure, describe the specific non-compliance.
  3. Categorize failures: fixable bugs vs. unimplemented features vs. intentional deviations.
  4. The user will compare this report against their own manual vttest notes.

- **Acceptance criteria:**
  - Every automatable vttest scenario (`[A]` classification) has a test.
  - Every test asserts correct VT100/VT220 behavior.
  - The compliance report is complete and accurate.

---

## Compliance Report (To Be Completed in 22.B7)

### Bugs Fixed During Phase B

| Bug # | vttest Menu | Description                                  | Status |
| ----- | ----------- | -------------------------------------------- | ------ |
| 1     | 2 (TBC)     | TBC Ps=2 incorrectly clears char tab stop    | Fixed  |
| 2     | 1 (IND/RI)  | handle_lf/handle_ri don't clear pending-wrap | Fixed  |
| 4a    | 3 (SCS)     | character_replace not saved by DECSC/DECRC   | Fixed  |
| 4b    | 3 (SCS)     | ESC)B produces Invalid                       | Fixed  |
| 4c    | 3 (SCS)     | SI/SO not handled as C0 control characters   | Fixed  |
| 5     | 1 (DECAWM)  | Autowrap ignores DECSTBM scroll region       | Fixed  |
| 6     | 1 (BS)      | BS from pending-wrap lands at wrong column   | Fixed  |

### Known Remaining Failures (To Be Filled In)

| vttest Menu | Test               | Description                                     | Bug # | Status |
| ----------- | ------------------ | ----------------------------------------------- | ----- | ------ |
| 6           | DA reports         | "Half of device attribute reports aren't right" | TBD   | Open   |
| 1/2         | Soft scroll region | Bug 3 — not yet investigated                    | 3     | Open   |
| 1           | DECCOLM 132-col    | Autowrap pass=1 not tested                      | TBD   | Open   |

---

## Architecture Notes

### Pending-Wrap Model

Freminal encodes pending-wrap state **implicitly**: `cursor.pos.x == width` (e.g., `x == 80` in
an 80-column terminal). There is NO explicit `pending_wrap` boolean flag. This means:

- `set_cursor_pos` (CUP/HVP) clamps `x` to `width-1` — correctly clears pending wrap
- `move_cursor_relative` (CUU/CUD/CUF/CUB) clamps to `width-1` — correct
- `handle_backspace` clamps from pending wrap before subtracting — fixed in Bug 6
- `handle_cr` sets `x=0` — correct
- `handle_lf` and `handle_ri` now clamp x — fixed in Bug 2
- `insert_text()` now respects scroll region bottom during autowrap — fixed in Bug 5

### Terminal Dimensions

- vttest assumes **80x24** (standard VT100 size) for all tests
- vttest also tests **132-column mode** (DECCOLM): autowrap and other tests run two passes
  (pass 0 at 80 columns, pass 1 at 132 columns)
- VT220 tests in `vt220.c` include 8-bit C1 control sequence tests (S8C1T/S7C1T), orthogonal
  to column width

### Test File Organization

```text
freminal-terminal-emulator/
  tests/
    vttest_common.rs          — shared test helpers (VtTestHelper)
    vttest_cursor.rs          — Menu 1 tests (43 tests + 3 Phase B additions)
    vttest_screen.rs          — Menu 2 tests (43 tests; 5 Phase B additions)
    vttest_charsets.rs        — Menu 3 tests (39 tests)
    vttest_reports.rs         — Menu 6 tests (25 tests)
    vttest_insert_delete.rs   — Menu 8 tests (32 tests)
    vttest_bugs.rs            — Menu 9 tests (10 tests)
    vttest_extensions.rs      — Menu 11 tests (38 tests)
    vttest_selftest.rs        — Self-tests (15 tests)
    golden/                   — golden reference files
```

### Tests NOT Appropriate for Automation

- **Menu 4** (double-sized characters): DECDWL/DECDHL rendering not implemented.
- **Menu 5** (keyboard): Requires actual key input events, not escape sequence replay.
- **Menu 7** (VT52 mode): VT52 mode implemented in Task 20.8 but vttest VT52 tests require
  interactive mode switching.
- **Menu 10.2** (DECTST self-test): Hardware diagnostic.
- **Menu 11.9** (mouse tracking): Requires GUI mouse events.
- **Menu 11.10** (window manipulation): Requires GUI window context.
- **Blinking text visual tests**: Blink rendering addressed in Task 23.

---

## Verification

Each subtask must pass before proceeding:

- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo-machete`

---

## References

- `vttest-20251205/` — vttest source (authoritative reference for escape sequences)
- `vttest-20251205/main.c` — menu structure, `tst_movements()`, autowrap test
- `vttest-20251205/esc.c` — escape sequence helper functions
- `vttest-20251205/reports.c` — device report test logic
- `vttest-20251205/charsets.c` — character set test logic
- `vttest-20251205/vt220.c` — VT220-specific test logic
- `freminal-terminal-emulator/tests/` — test files
- `freminal-buffer/tests/terminal_handler_integration.rs` — integration tests
- `./test.bin` — FREC recording of vttest session (57511 bytes, 1066 frames)
- <https://invisible-island.net/vttest/vttest.html> — vttest documentation
