# PLAN_06 — Test Gap Coverage

## Overview

Analyze test gaps across the entire codebase, produce a prioritized plan to reduce them (ordered
by risk), and review with the user before implementation. This task is a planning + implementation
hybrid: planning phase produces the gap analysis, implementation phase fills the gaps.

**Dependencies:** None
**Dependents:** None (but improves safety for all other tasks)
**Primary crates:** All
**Estimated scope:** Large (analysis + implementation)

---

## Current Test State

### Coverage Summary

| Crate                        | Tests | Coverage Areas                    | Major Gaps                                 |
| ---------------------------- | ----- | --------------------------------- | ------------------------------------------ |
| `freminal-common`            | 49    | Types, format tags, color mapping | Config loading, arg parsing                |
| `freminal-buffer`            | 111+  | Buffer ops, rows, cells, cursor   | Scrollback edge cases                      |
| `freminal-terminal-emulator` | 204+  | ANSI parser, escape sequences     | Snapshot building, PTY I/O                 |
| `freminal` (binary)          | 0     | —                                 | Everything: GUI, fonts, rendering, main.rs |
| **Total**                    | ~497  |                                   |                                            |

Additional: 10 proptest blocks, 22 benchmark groups across 3 bench files.

### Zero-Test Areas (Ranked by Risk)

1. **Config loading/saving** — Used at startup, will be extended by Tasks 2-4. Bugs here break
   the entire application.
2. **Snapshot building** (`build_snapshot()`) — Core data pipeline from emulator to GUI. Bugs
   cause visual corruption.
3. **Color mapping** (`internal_color_to_egui()`) — Called hundreds of times per frame. Currently
   uses `Color32::from_hex()` at runtime.
4. **Font loading** — Application crashes if fonts fail to load. No fallback testing.
5. **GUI logic** — No tests for any GUI behavior (frame loop, input handling, scroll, mouse).
6. **main.rs** — Entry point with arg parsing, config loading, PTY setup. Completely untested.
7. **Arg parsing** — Hand-rolled (will be replaced by clap in Task 2, but needs tests until then).

---

## Prioritized Test Plan

Tests are organized by risk priority. Each group includes specific test cases to implement.

### Priority 1: Config System (HIGH RISK)

Config bugs break startup. Config is the foundation for Tasks 2, 3, and 4.

#### 6.1 — Config loading tests

- **Status:** Not Started
- **Scope:** `freminal-common/tests/` or `freminal-common/src/config.rs`
- **Test cases:**
  - Load valid complete config from TOML string
  - Load valid partial config (missing optional sections)
  - Load empty config (all defaults)
  - Load config with invalid font size (< 4.0, > 96.0) → error
  - Load config with invalid cursor shape → error
  - Load config with unknown keys → ignored (forward compat)
  - Load config with wrong types → error
  - Default config path resolution per platform
  - Layered loading: system < user < env var < explicit path
  - `load_config(None)` returns defaults when no file exists
  - `load_config(Some(path))` reads specified file
  - `load_config(Some(missing_path))` returns appropriate error
- **Acceptance criteria:** All config loading paths have test coverage

#### 6.2 — Config serialization round-trip tests

- **Status:** Not Started
- **Scope:** `freminal-common/tests/`
- **Test cases:**
  - Default config → serialize → deserialize → equals original
  - Custom config → serialize → deserialize → equals original
  - Serialized output is valid TOML
  - Serialized output is human-readable (sections, keys, comments)
- **Notes:** Depends on Task 2 (subtask 2.6) for save_config(). Can stub until then.
- **Acceptance criteria:** Config round-trips without data loss

### Priority 2: Snapshot Building (HIGH RISK)

Snapshot is the sole data pipeline between emulator and GUI. Bugs here cause visual corruption.

#### 6.3 — Snapshot content tests

- **Status:** Not Started
- **Scope:** `freminal-terminal-emulator/tests/`
- **Test cases:**
  - Empty terminal produces snapshot with correct dimensions
  - Terminal with text produces snapshot with correct TChar content
  - Terminal with formatting (bold, color, etc.) produces correct FormatTags
  - Cursor position is correctly reflected in snapshot
  - Scrollback content is included in snapshot
  - Snapshot after terminal resize has correct dimensions
  - Snapshot with wrapped lines has correct content
  - Snapshot with wide characters has correct cell representation
  - Multiple rapid snapshots produce consistent state
- **Acceptance criteria:** Snapshot output matches expected terminal state for all test inputs

#### 6.4 — Snapshot with escape sequences

- **Status:** Not Started
- **Scope:** `freminal-terminal-emulator/tests/`
- **Test cases:**
  - SGR color codes reflected in snapshot format tags
  - Cursor movement sequences reflected in snapshot cursor position
  - Clear screen produces empty snapshot
  - Scroll region operations reflected in snapshot
  - Alternate screen buffer switch reflected in snapshot
- **Acceptance criteria:** Escape sequence effects are visible in snapshot

### Priority 3: Color Mapping (MEDIUM RISK)

Called hundreds of times per frame. Correctness affects all rendered output.

#### 6.5 — Color conversion tests

- **Status:** Not Started
- **Scope:** `freminal/tests/` or inline tests in `freminal/src/gui/colors.rs`
- **Test cases:**
  - All 16 named terminal colors map to correct Color32 values
  - 256-color palette index → correct Color32
  - RGB color (r, g, b) → correct Color32
  - Default foreground color → correct Color32
  - Default background color → correct Color32
  - Each Catppuccin Mocha color constant is correct (verify against reference)
  - `InternalColor::Default` maps correctly for both fg and bg context
- **Acceptance criteria:** Every color variant produces the expected Color32 value

### Priority 4: Font Loading (MEDIUM RISK)

Font loading failure crashes the app. No fallback chain testing exists.

#### 6.6 — Font loading tests

- **Status:** Not Started
- **Scope:** `freminal/tests/` or inline tests in `freminal/src/gui/fonts.rs`
- **Test cases:**
  - Bundled fonts load successfully (all 4 variants)
  - `get_char_size()` returns consistent non-zero dimensions
  - `get_char_size()` with space `' '` vs `'W'` — document the difference
  - Font family names are correctly registered with egui
  - System font discovery finds at least one monospace font (platform-dependent)
  - Missing custom font family falls back gracefully
  - Emoji fallback chain works (if system fonts available)
- **Acceptance criteria:** Font loading is robust and fallback chain is tested
- **Notes:** Some tests may need to be `#[cfg(target_os = "...")]` gated

### Priority 5: GUI Logic Extraction (MEDIUM-LOW RISK)

GUI logic is currently untestable because it's embedded in egui rendering code. This subtask
focuses on extracting testable logic, not testing egui directly.

#### 6.7 — Extract and test scroll logic

- **Status:** Not Started
- **Scope:** `freminal/src/gui/view_state.rs`
- **Test cases:**
  - Scroll down by N lines from top
  - Scroll up by N lines from bottom
  - Scroll doesn't go past scrollback limit
  - Scroll doesn't go below terminal bottom
  - Page up / page down
  - Scroll to top / scroll to bottom
  - New output auto-scrolls when at bottom
  - New output does NOT auto-scroll when scrolled up
- **Acceptance criteria:** Scroll behavior is correct for all edge cases

#### 6.8 — Extract and test input encoding

- **Status:** Not Started
- **Scope:** `freminal/src/gui/` (keyboard and mouse modules)
- **Test cases:**
  - Regular ASCII key → correct byte sequence
  - Arrow keys → correct escape sequence
  - Function keys → correct escape sequences
  - Ctrl+C → correct byte
  - Mouse click encoding (when mouse reporting is enabled)
  - Mouse wheel encoding
  - Modifier combinations (Shift+Arrow, Ctrl+Arrow, etc.)
- **Acceptance criteria:** Input encoding matches xterm specification

#### 6.9 — Extract and test layout calculations

- **Status:** Not Started
- **Scope:** `freminal/src/gui/terminal.rs`
- **Test cases:**
  - Terminal dimensions calculation from pixel size and cell size
  - Cell position from pixel coordinates
  - Pixel position from cell coordinates
  - Visible line range calculation from scroll offset
- **Acceptance criteria:** Layout calculations are mathematically correct

### Priority 6: Additional Buffer Edge Cases (LOW RISK — already well-tested)

#### 6.10 — Scrollback edge cases

- **Status:** Not Started
- **Scope:** `freminal-buffer/tests/`
- **Test cases:**
  - Scrollback at exactly the limit — next line evicts oldest
  - Scrollback with 0 lines — still functional
  - Scrollback with very large limit (100,000)
  - Scrollback interaction with terminal resize
  - Scrollback content after clear screen (should preserve history)
- **Acceptance criteria:** Buffer handles all scrollback boundary conditions

---

## Implementation Order

The subtasks above are ordered by risk. Recommended implementation order:

1. **6.1, 6.2** — Config tests (foundation for Tasks 2-4)
2. **6.3, 6.4** — Snapshot tests (foundation for Task 1)
3. **6.5** — Color mapping tests (quick win, high call frequency)
4. **6.6** — Font loading tests (crash prevention)
5. **6.7, 6.8, 6.9** — GUI logic extraction and tests (requires refactoring)
6. **6.10** — Buffer edge cases (incremental improvement)

---

## Affected Files

| File                                                 | Change Type                            |
| ---------------------------------------------------- | -------------------------------------- |
| `freminal-common/tests/config_tests.rs`              | NEW — config loading/saving tests      |
| `freminal-terminal-emulator/tests/snapshot_tests.rs` | NEW — snapshot building tests          |
| `freminal/src/gui/colors.rs`                         | Add inline tests or separate test file |
| `freminal/src/gui/fonts.rs`                          | Add inline tests                       |
| `freminal/src/gui/view_state.rs`                     | Refactor for testability, add tests    |
| `freminal/src/gui/terminal.rs`                       | Extract testable logic                 |
| `freminal-buffer/tests/scrollback_tests.rs`          | NEW or extend existing                 |

---

## Metrics

Track test count and coverage as subtasks complete:

| Subtask | Tests Added | Coverage Before | Coverage After |
| ------- | ----------- | --------------- | -------------- |
| 6.1     | —           | —               | —              |
| 6.2     | —           | —               | —              |
| 6.3     | —           | —               | —              |
| 6.4     | —           | —               | —              |
| 6.5     | —           | —               | —              |
| 6.6     | —           | —               | —              |
| 6.7     | —           | —               | —              |
| 6.8     | —           | —               | —              |
| 6.9     | —           | —               | —              |
| 6.10    | —           | —               | —              |

---

## Risk Assessment

| Risk                                       | Likelihood | Impact | Mitigation                          |
| ------------------------------------------ | ---------- | ------ | ----------------------------------- |
| Tests require GUI context (egui)           | High       | Medium | Extract pure logic, test separately |
| Platform-specific behavior (fonts, paths)  | Medium     | Low    | Conditional compilation, CI matrix  |
| Test infrastructure churn from other tasks | Medium     | Low    | Write stable interface tests        |
| Large scope → incomplete coverage          | Medium     | Medium | Prioritize by risk, iterate         |

---

## Review Gate

**IMPORTANT:** This plan must be reviewed by the user before implementation begins. The user
requested that test gaps be prioritized by risk and reviewed before filling them.

Specifically, review:

1. Are the priorities correct?
2. Are any critical test cases missing?
3. Is the implementation order acceptable?
4. Are there any areas the user wants to skip or defer?
