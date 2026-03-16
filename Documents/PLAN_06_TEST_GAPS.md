# PLAN_06 — Test Gap Coverage

## Overview

Data-driven test gap analysis. Instead of pre-specifying test cases, this plan uses `cargo llvm-cov`
to identify actual coverage gaps at execution time, then fills them in priority order.

**Dependencies:** None
**Dependents:** None (but improves safety for all other tasks)
**Primary crates:** All
**Estimated scope:** Large (analysis + implementation)

---

## Approach

This plan is intentionally a lightweight shell. The agent executing it will:

1. Run `cargo llvm-cov` to produce a per-file coverage report.
2. Analyze the report to identify files and functions with low or zero coverage.
3. Prioritize gaps by risk (crash potential, data corruption, high call frequency).
4. Fill in the plan dynamically — writing specific subtasks based on real data, not speculation.
5. Implement tests in priority order, re-running coverage after each batch.

This avoids the failure mode of prescribing hundreds of specific test cases up front that may
not match the actual codebase state by the time implementation begins.

---

## Implementation Checklist

> **Agent instructions:** Follow the Multi-Step Task Protocol from `agents.md`.
> Execute one task at a time. Update this document after each. Stop and wait for confirmation.

---

- [x] **6.1 — Run `cargo llvm-cov` and produce baseline coverage report**
  - Install `cargo-llvm-cov` if not present (`cargo install cargo-llvm-cov`).
  - Run: `cargo llvm-cov --all --lcov --output-path lcov.info`
  - Run: `cargo llvm-cov --all --text` to get a human-readable summary.
  - Record the per-crate and per-file coverage percentages in Section "Coverage Baseline" below.
  - Identify all files with 0% coverage.
  - Identify all files below 50% coverage.
  - Create a report (in this document) listing these files, their coverage, and any initial observations.
  - Do NOT write any tests yet — this is pure analysis.
  - **Verify:** Coverage report is generated. Baseline numbers are recorded in this document.
  - ✅ **Completed 2026-03-16.** Baseline coverage recorded below. 71.6% overall line coverage
    across 100 files. 16 files at 0% coverage, 17 additional files below 50%. Key observations:
    `freminal-buffer` is strong at 91.9%; `freminal` binary is weakest at 48.5% (dominated by
    untestable GUI/main code); `freminal-common` modes have many small files with boilerplate
    at 0–48% coverage.

---

- [x] **6.2 — Analyze gaps and populate the test plan**
  - Review the coverage report from 6.1.
  - For each file below 50% coverage, identify the specific uncovered functions and code paths.
  - Prioritize by risk:
    - **P0 (Critical):** Code that can panic, crash, or corrupt data if wrong. Startup paths,
      config loading, PTY setup, snapshot building.
    - **P1 (High):** Code called on every frame or every PTY read. Hot paths where bugs cause
      visual corruption or incorrect behavior.
    - **P2 (Medium):** Code for less-common features. Escape sequence edge cases, mouse encoding,
      window manipulation.
    - **P3 (Low):** Code that is already well-tested or inherently low-risk. Pure data types,
      simple getters/setters.
  - Add new subtasks (6.3, 6.4, 6.5, ...) to this document, one per logical group of tests.
    Each subtask must specify:
    - Which file(s) and function(s) to cover.
    - Why this gap matters (risk category).
    - Where the tests should live (inline `#[cfg(test)]` module or `tests/` directory).
  - **Verify:** Subtasks are added to this document. Each has clear scope and acceptance criteria.
  - ✅ **Completed 2026-03-16.** Three parallel analysis passes covered: (1) `interface.rs` —
    `build_snapshot`, `split_format_data_for_scrollback`, resize/scroll/write paths;
    (2) `internal.rs` — UTF-8 tail-scan, `send_focus_event`, `scroll()`, `report_*` methods
    (found to be dead code — never called after Task 8 refactor); (3) `freminal-common` modes,
    cursor, mouse, SGR, and all zero-coverage CSI parsers. 13 subtasks defined below (6.3–6.15).

---

### Dead Code Discovery

The following `pub` methods in `TerminalState` (`internal.rs`) are **never called** anywhere in
the codebase after the Task 8 performance refactor eliminated the `FairMutex`. They should be
deleted rather than tested:

- `report_window_state`, `report_window_position`, `report_window_size`,
  `report_root_window_size`, `report_character_size`, `report_terminal_size_in_characters`,
  `report_root_terminal_size_in_characters`, `report_icon_label`, `report_title`, `report_mode`

These were previously called from `handle_window_manipulation` in the GUI, but that function was
rewritten during Task 8 to build response strings inline and send them via `pty_write_tx` directly.
The dead methods remain in `internal.rs` and inflate the uncovered-lines count.

**Action:** Subtask 6.3 deletes these before any test work begins, to avoid wasting effort testing
dead code and to get an accurate coverage baseline for `internal.rs`.

---

- [x] **6.3 — Delete dead `report_*` methods from `TerminalState`**
  - **Priority:** P0 (prerequisite — cleans up dead code before test work)
  - **Files:** `freminal-terminal-emulator/src/state/internal.rs`
  - **What:** Delete `report_window_state`, `report_window_position`, `report_window_size`,
    `report_root_window_size`, `report_character_size`, `report_terminal_size_in_characters`,
    `report_root_terminal_size_in_characters`, `report_icon_label`, `report_title`, `report_mode`.
    Also delete `send_decrpm` if it is only used by the dead code path (verify first).
  - **Why:** Dead code policy from `agents.md` forbids `#[allow(dead_code)]` in production.
    These methods are pub but never called. Deleting them before test work avoids wasting effort
    and gives an accurate uncovered-lines baseline for `internal.rs`.
  - **Verify:** `cargo test --all` passes. `cargo clippy --all-targets --all-features -- -D warnings` passes. No call site references the deleted methods.
  - ✅ **Completed 2026-03-16.** Deleted all 10 `report_*` methods (124 lines). `send_decrpm`
    was kept — it is actively used by `handle_incoming_data` for DECRPM query responses.
    Removed unused `collect_text` import. File reduced from 693 to 569 lines. All verifications
    pass: `cargo test --all` (all pass), `cargo clippy` (clean), `cargo-machete` (clean).

---

- [x] **6.4 — `build_snapshot` behavioral tests (P0)**
  - **Priority:** P0 — every frame depends on correct snapshot production
  - **Files:** `freminal-terminal-emulator/src/interface.rs`
  - **Tests in:** `freminal-terminal-emulator/tests/snapshot_build.rs` (new file)
  - **What to test:**
    - `content_changed` is `true` after PTY data arrives, `false` on a second call with no data.
    - `content_changed` is `true` after `erase_display`.
    - `content_changed` is `false` after cursor-only movement (no cell mutation).
    - `content_changed` transitions correctly across `enter_alternate` / `leave_alternate`.
    - `visible_chars` / `visible_tags` match expected content after inserting known text.
    - `cursor_pos` reflects current position.
    - `show_cursor` reflects DECTCEM state.
    - `is_alternate_screen` flag is correct after switching.
    - `scroll_offset` clamping: `set_gui_scroll_offset` with a value beyond max is clamped.
    - Auto-scroll reset: `gui_scroll_offset` resets to 0 when new PTY data arrives while scrolled back.
  - **Verify:** `cargo test --all` passes.
  - ✅ **Completed 2026-03-16.** Created `freminal-terminal-emulator/tests/snapshot_build.rs`
    with 27 behavioral tests grouped by invariant: first-ever snapshot (3), clean path / Arc
    reuse (2), dirty path (3), cursor-only move (1), alternate screen (5), show_cursor
    suppression when scrolled back (1), scroll_changed flag (3), auto-scroll reset (1), offset
    clamping (1), dimension tracking (2), is_normal_display / DECSCNM (3), visible content
    match (1), erase display cache invalidation (1). Default terminal size discovered to be
    100×100 (not 24×80); `fill_scrollback` helper writes 150 lines to guarantee scrollback.
    All 27 tests pass. Clippy clean. Machete clean.

---

- [x] **6.5 — `split_format_data_for_scrollback` unit tests (P0)**
  - **Priority:** P0 — pure function with tricky offset arithmetic, zero tests
  - **Files:** `freminal-terminal-emulator/src/interface.rs`
  - **Tests in:** `freminal-terminal-emulator/tests/split_format_data_tests.rs` (new file)
  - **What to test:**
    - Tags entirely before the split point → appear in scrollback section, untouched.
    - Tags entirely after the split point → appear in visible section, offsets re-based to 0.
    - Tags that span the split boundary → clamped: scrollback gets `end = split`, visible gets
      `start = 0`.
    - Empty tag vector → both sections empty.
    - `include_scrollback = false` → scrollback section is empty.
    - Single tag covering the entire range → correctly split into both sections.
  - **Verify:** `cargo test --all` passes.
  - ✅ **Completed 2026-03-16.** Created `freminal-terminal-emulator/tests/split_format_data_tests.rs`
    with 18 unit tests covering: empty input (2), `include_scrollback = false` (1), tag entirely
    before split (1), tag entirely after split / rebased (1), spanning tag in both sections (1),
    exact boundary cases — end at split, start at split (2), tag past `visible_end` dropped (1),
    sentinel `usize::MAX` end behaviour (3 — clamped in scrollback, dropped from visible, passes
    when `visible_end` is also `MAX`), multiple mixed tags (1), zero split point (1), single tag
    covering full range (1), zero-width tags (2), `visible_end == split` (1). Also documented
    that the function currently has zero production callers — it was written for the scrollback
    rendering path but is not yet wired up.

---

- [ ] **6.6 — UTF-8 tail-scan split in `handle_incoming_data` (P0)**
  - **Priority:** P0 — data integrity for multi-byte characters split across PTY reads
  - **Files:** `freminal-terminal-emulator/src/state/internal.rs`
  - **Tests in:** `freminal-terminal-emulator/tests/utf8_split_tests.rs` (new file)
  - **What to test:**
    - 2-byte UTF-8 char (e.g. `é` = `\xC3\xA9`) split: first call gets `\xC3`, second call
      gets `\xA9` → character appears correctly in the buffer.
    - 3-byte UTF-8 char (e.g. `€` = `\xE2\x82\xAC`) split at each possible boundary.
    - 4-byte UTF-8 char (e.g. `😀` = `\xF0\x9F\x98\x80`) split at each possible boundary.
    - Complete ASCII input → no leftover, immediate processing.
    - Complete multi-byte sequence → no leftover, immediate processing.
    - Multiple incomplete sequences in succession (pathological: 1 byte at a time for a 4-byte char).
    - Mixed ASCII + split multi-byte: `"hello\xC3"` then `"\xA9 world"`.
  - **Verify:** `cargo test --all` passes.

---

- [ ] **6.7 — `StateColors` reverse-video logic (P1)**
  - **Priority:** P1 — affects every cell's color when reverse video is active
  - **Files:** `freminal-common/src/buffer_states/cursor.rs`
  - **Tests in:** `freminal-common/tests/cursor_tests.rs` (new file)
  - **What to test:**
    - `get_color()` with `ReverseVideo::Off` → returns the foreground color as-is.
    - `get_color()` with `ReverseVideo::On` → returns background color with `default_to_regular`.
    - `get_background_color()` with `ReverseVideo::Off` → returns background as-is.
    - `get_background_color()` with `ReverseVideo::On` → returns foreground with `default_to_regular`.
    - `get_underline_color()` both modes.
    - `flip_reverse_video` toggles `On` ↔ `Off`.
    - `set_default` resets all fields including reverse video.
    - Builder methods produce correct state.
    - Custom colors (not Default) under reverse video are swapped correctly.
    - `CursorPos` Display format.
  - **Verify:** `cargo test --all` passes.

---

- [ ] **6.8 — `terminal_mode_from_params` dispatch table (P1)**
  - **Priority:** P1 — central mode dispatch; incorrect dispatch → wrong terminal behavior
  - **Files:** `freminal-common/src/buffer_states/mode.rs`
  - **Tests in:** `freminal-common/tests/mode_dispatch_tests.rs` (new file)
  - **What to test:**
    - Every known param (1, 3, 4, 5, 6, 7, 8, 9, 12, 20, 25, 40, 45, 47, 69, 1000, 1002,
      1003, 1004, 1005, 1006, 1016, 1047, 1048, 1049, 2004, 2026, 2027, 2031) × DecSet/DecRst
      → returns the correct `Mode` variant.
    - DecQuery for each known param → returns the correct query variant.
    - Unknown param with `?` prefix → `Mode::UnknownQuery`.
    - Unknown param without `?` → `Mode::Unknown`.
    - Non-DEC mode `20` (LineFeedMode) with `DecSet`/`DecRst` → correct `Mode::LineFeedMode`.
    - Mouse encoding mode `DecQuery` quirk: params 1005, 1006, 1016 with `DecQuery` return
      `MouseMode(Query(...))` not `MouseEncodingMode(...)` — document this as intentional or flag
      as a bug.
  - **Verify:** `cargo test --all` passes.

---

- [ ] **6.9 — Mouse mode types: `MouseEncoding` and `MouseTrack` (P1)**
  - **Priority:** P1 — mouse support correctness
  - **Files:** `freminal-common/src/buffer_states/modes/mouse.rs`
  - **Tests in:** `freminal-common/tests/mouse_mode_tests.rs` (new file)
  - **What to test:**
    - `MouseEncoding::mouse_mode_number()` for all 4 variants → correct numeric IDs.
    - `MouseEncoding::report()` for all 4 variants × `DecSet`/`DecRst`/`DecQuery`/`None`
      → correct DECRPM escape strings.
    - `MouseEncoding::fmt` (Display) for all variants → correct string names.
    - `MouseTrack::mouse_mode_number()` for all variants including `Query(v)`.
    - `MouseTrack::report()` for all variants × `SetMode` values → correct DECRPM strings.
    - `MouseTrack::fmt` (Display) for all variants including `Query(v)`.
    - Edge case: `NoTracking` report values for each `SetMode`.
  - **Verify:** `cargo test --all` passes.

---

- [ ] **6.10 — `send_focus_event` and `write()` in `TerminalState` (P1)**
  - **Priority:** P1 — focus events and PTY write are critical I/O paths
  - **Files:** `freminal-terminal-emulator/src/state/internal.rs`
  - **Tests in:** `freminal-terminal-emulator/tests/terminal_state_tests.rs` (new file)
  - **What to test:**
    - `send_focus_event(true)` when focus reporting is enabled → sends `\x1b[I` to PTY channel.
    - `send_focus_event(false)` when focus reporting is enabled → sends `\x1b[O` to PTY channel.
    - `send_focus_event(true)` when focus reporting is disabled → nothing sent.
    - `write(&TerminalInput::Ascii(b'A'))` → sends `PtyWrite::Write(vec![b'A'])`.
    - `write()` with a closed channel → returns `Err`.
    - `scroll()` in alternate screen → sends arrow key bytes via write channel.
    - `scroll()` in primary screen → calls handler scroll methods (verify offset returned).
    - Mode accessors: `is_normal_display`, `should_repeat_keys`, `skip_draw_always` return
      correct values after feeding the corresponding mode-set escape sequences.
  - **Verify:** `cargo test --all` passes.

---

- [ ] **6.11 — CSI command parsers: CPL, ICH, SD, DL, IL, SU (P2)**
  - **Priority:** P2 — less common escape sequences but zero/low coverage
  - **Files:**
    - `freminal-terminal-emulator/src/ansi_components/csi_commands/cpl.rs`
    - `freminal-terminal-emulator/src/ansi_components/csi_commands/ict.rs`
    - `freminal-terminal-emulator/src/ansi_components/csi_commands/sd.rs`
    - `freminal-terminal-emulator/src/ansi_components/csi_commands/dl.rs`
    - `freminal-terminal-emulator/src/ansi_components/csi_commands/il.rs`
    - `freminal-terminal-emulator/src/ansi_components/csi_commands/su.rs`
  - **Tests in:** `freminal-terminal-emulator/tests/csi_commands_param_tests.rs` (new file)
  - **What to test for each parser:**
    - Default param (no param / 0 / 1) → output with value 1.
    - Explicit param > 1 → output with correct value.
    - Invalid non-numeric param → `TerminalOutput::Invalid` or error return.
    - For CPL specifically: verify two outputs (relative cursor move + absolute column 1).
  - **Verify:** `cargo test --all` passes.

---

- [ ] **6.12 — SGR `from_usize` and `from_usize_color` direct tests (P2)**
  - **Priority:** P2 — SGR is already well-tested via parser integration, but direct unit tests
    catch edge cases in the lookup table
  - **Files:** `freminal-common/src/sgr.rs`
  - **Tests in:** `freminal-common/tests/sgr_direct_tests.rs` (new file)
  - **What to test:**
    - `from_usize(0)` → `Reset`.
    - `from_usize(38)` → `Foreground(Default)` (the error-log fallback path for bare `38`).
    - `from_usize(48)` → `Background(DefaultBackground)` (same pattern).
    - `from_usize(58)` → `UnderlineColor(DefaultUnderlineColor)`.
    - `from_usize(val)` for all explicitly listed values (1–9, 21–29, 30–37, 39, 40–47, 49,
      51–55, 58–65, 73–75, 90–97, 100–107) → correct variant.
    - `from_usize(110)` (out of range) → `Unknown(110)`.
    - `from_usize_color(38, 128, 64, 0)` → `Foreground(Custom(128, 64, 0))`.
    - `from_usize_color(48, 0, 0, 255)` → `Background(Custom(0, 0, 255))`.
    - `from_usize_color(58, 10, 20, 30)` → `UnderlineColor(Custom(10, 20, 30))`.
    - `from_usize_color(38, 256, 0, 0)` → `Unknown` (overflow).
    - `from_usize_color(99, 0, 0, 0)` → `Unknown(99)`.
  - **Verify:** `cargo test --all` passes.

---

- [ ] **6.13 — Mode boilerplate types batch tests (P2)**
  - **Priority:** P2 — many small files at ~48% coverage, each with identical patterns
  - **Files:** All `freminal-common/src/buffer_states/modes/*.rs` files at ~48%:
    `decarm.rs`, `lnm.rs`, `reverse_wrap_around.rs`, `sync_updates.rs`, `xtmsewin.rs`,
    `allow_column_mode_switch.rs`, `theme.rs`, `decsclm.rs`, `grapheme.rs`, `keypad.rs`,
    `decscnm.rs`, `xtextscrn.rs`, `url.rs`
  - **Tests in:** `freminal-common/tests/mode_boilerplate_tests.rs` (new file)
  - **What to test (for each mode type):**
    - Default value → expected variant.
    - Construct each variant → `Display` produces expected string.
    - `ReportMode::report()` for `DecSet`/`DecRst`/`DecQuery`/`None` → correct DECRPM string.
    - `From<SetMode>` conversion for each variant → correct mapping.
  - **Notes:** Use a parameterized test macro or helper function to avoid repetitive test code.
    Each mode type follows the same pattern: 2-3 variants, `From<SetMode>`, `Display`, `report()`.
  - **Verify:** `cargo test --all` passes.

---

- [ ] **6.14 — `interface.rs` channel and resize tests (P1)**
  - **Priority:** P1 — resize and write paths are exercised on every frame / every keypress
  - **Files:** `freminal-terminal-emulator/src/interface.rs`
  - **Tests in:** `freminal-terminal-emulator/tests/interface_tests.rs` (new file)
  - **What to test:**
    - `write_raw_bytes(&[0x41])` → receiver gets `PtyWrite::Write(vec![0x41])`.
    - `write_raw_bytes` with empty bytes → sends empty `PtyWrite::Write`.
    - `set_win_size` with same dimensions → no `PtyWrite::Resize` sent.
    - `set_win_size` with different dimensions → `PtyWrite::Resize` sent with correct values.
    - `set_win_size` with closed channel → returns `Err`.
    - `handle_resize_event` sends `PtyWrite::Resize` and does not return `Err` (logs instead).
    - `clone_write_tx` returns a working sender (send through it, verify receiver gets data).
    - `set_gui_scroll_offset` / `reset_scroll_offset` affect `build_snapshot` output.
    - `extract_selection_text` returns correct text for a known buffer region.
  - **Verify:** `cargo test --all` passes.

---

- [ ] **6.15 — Final coverage report and gap assessment**
  - Run `cargo llvm-cov --all --text` and compare to baseline.
  - Update the Coverage Progress table in this document.
  - Identify any remaining gaps above P2 priority.
  - Record final per-crate coverage percentages.
  - If coverage target (defined in `agents.md` as 100%) is not met, document what remains
    untestable (GUI code, platform-specific PTY code) and what could be addressed in future work.
  - **Verify:** Report is recorded in this document.

---

## Coverage Baseline

> Populated by subtask 6.1 on 2026-03-16.

| Crate                        | Files | Line Coverage           | Notes                                          |
| ---------------------------- | ----- | ----------------------- | ---------------------------------------------- |
| `freminal-common`            | 41    | 76.6% (2396/3128)       | Many small mode files with boilerplate at ~48% |
| `freminal-buffer`            | 5     | 91.9% (6991/7611)       | Strong coverage; row.rs at 81%                 |
| `freminal-terminal-emulator` | 40    | 69.7% (2560/3673)       | interface.rs 21%, internal.rs 47%              |
| `freminal` (binary)          | 13    | 48.5% (3179/6557)       | GUI code largely untestable                    |
| `xtask`                      | 1     | 0.0% (0/165)            | CI tool, not production code                   |
| **Total**                    | 100   | **71.6% (15126/21134)** |                                                |

Branch coverage: not reported by `cargo-llvm-cov` for this project (0/0 branches instrumented).

### Zero-Coverage Files

| File                                                                 | Lines | Observation                                          |
| -------------------------------------------------------------------- | ----- | ---------------------------------------------------- |
| `freminal-common/src/buffer_states/modes/decsclm.rs`                 | 15    | Small mode type — boilerplate `From`/`Display` impls |
| `freminal-common/src/buffer_states/modes/grapheme.rs`                | 23    | Small mode type — boilerplate                        |
| `freminal-common/src/buffer_states/modes/keypad.rs`                  | 5     | Tiny mode type                                       |
| `freminal-common/src/buffer_states/modes/mouse.rs`                   | 62    | Mouse tracking mode — larger, has real logic         |
| `freminal-common/src/buffer_states/url.rs`                           | 5     | Tiny URL mode type                                   |
| `freminal-common/src/pty_write.rs`                                   | 6     | `TryFrom` impl for `PtySize`                         |
| `freminal-terminal-emulator/src/ansi_components/csi_commands/cpl.rs` | 21    | CSI CPL (Cursor Previous Line) parser                |
| `freminal-terminal-emulator/src/ansi_components/csi_commands/ict.rs` | 16    | CSI ICT (Initiate Highlight) parser                  |
| `freminal-terminal-emulator/src/ansi_components/csi_commands/sd.rs`  | 16    | CSI SD (Scroll Down) parser                          |
| `freminal-terminal-emulator/src/io/pty.rs`                           | 173   | PTY I/O — platform-specific, requires live PTY       |
| `freminal/src/gui/fonts.rs`                                          | 173   | Font loading — requires filesystem, hard to test     |
| `freminal/src/gui/mod.rs`                                            | 543   | GUI main loop — requires egui context                |
| `freminal/src/gui/view_state.rs`                                     | 19    | ViewState — simple struct + `Default`                |
| `freminal/src/main.rs`                                               | 247   | Binary entrypoint — requires runtime env             |
| `freminal/src/playback.rs`                                           | 244   | Recording playback — requires runtime env            |
| `xtask/src/main.rs`                                                  | 165   | CI orchestration — not production code               |

### Below-50% Files (excluding 0%)

| File                                                    | Coverage         | Lines | Observation                                           |
| ------------------------------------------------------- | ---------------- | ----- | ----------------------------------------------------- |
| `freminal/src/gui/terminal.rs`                          | 13.4% (132/985)  | 985   | Large; mostly GUI rendering, some testable logic      |
| `freminal-common/.../modes/allow_column_mode_switch.rs` | 16.0% (4/25)     | 25    | Mode boilerplate                                      |
| `freminal-common/.../modes/theme.rs`                    | 16.0% (4/25)     | 25    | Mode boilerplate                                      |
| `freminal-terminal-emulator/src/interface.rs`           | 21.2% (97/458)   | 458   | Snapshot building, PTY coordination — **P0 critical** |
| `freminal/src/gui/settings.rs`                          | 26.6% (76/286)   | 286   | Settings modal UI — partially testable                |
| `freminal-common/.../cursor.rs`                         | 39.6% (36/91)    | 91    | Cursor types — `From`/`Display` impls                 |
| `freminal-common/.../mode.rs`                           | 39.7% (48/121)   | 121   | Mode type dispatching — set/reset/query               |
| `freminal-common/.../modes/xtextscrn.rs`                | 40.3% (31/77)    | 77    | XtExtScrn mode                                        |
| `freminal-common/.../modes/decscnm.rs`                  | 42.9% (12/28)    | 28    | Screen mode                                           |
| `freminal-common/src/sgr.rs`                            | 43.0% (43/100)   | 100   | SGR attribute types — `Display` impls                 |
| `freminal-terminal-emulator/src/state/internal.rs`      | 47.1% (171/363)  | 363   | TerminalState — **P0/P1 critical**                    |
| `freminal-common/.../modes/decarm.rs`                   | 48.0% (12/25)    | 25    | Mode boilerplate                                      |
| `freminal-common/.../modes/lnm.rs`                      | 48.0% (12/25)    | 25    | Mode boilerplate                                      |
| `freminal-common/.../modes/reverse_wrap_around.rs`      | 48.0% (12/25)    | 25    | Mode boilerplate                                      |
| `freminal-common/.../modes/sync_updates.rs`             | 48.0% (12/25)    | 25    | Mode boilerplate                                      |
| `freminal-common/.../modes/xtmsewin.rs`                 | 48.0% (12/25)    | 25    | Mode boilerplate                                      |
| `freminal/src/gui/renderer.rs`                          | 48.4% (708/1464) | 1464  | OpenGL renderer — largely untestable                  |

---

## Coverage Progress

> Updated after each test implementation subtask.

| Subtask | Target File(s) | Tests Added | Coverage Before | Coverage After |
| ------- | -------------- | ----------- | --------------- | -------------- |

---

## Constraints

- Tests must be hermetic, order-independent, and focused on observable behavior.
- No `unwrap()` or `expect()` in test helper code that could mask failures — use them only on
  values that are genuinely expected to succeed (with a comment explaining why).
- GUI-dependent code (egui context, GL context) cannot be unit-tested directly. For those paths,
  extract pure logic into testable functions. Do not try to instantiate egui in tests.
- Platform-specific tests must be gated with `#[cfg(target_os = "...")]`.
- Each subtask must leave `cargo test --all` passing.
