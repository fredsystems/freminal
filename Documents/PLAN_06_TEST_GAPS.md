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

- [ ] **6.2 — Analyze gaps and populate the test plan**
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

---

- [ ] **6.3–6.N — (Populated dynamically by 6.2)**

  Placeholder for test implementation subtasks. Each will be a focused batch:
  - Write tests for the identified gap.
  - Run `cargo test --all` — all tests pass.
  - Run `cargo llvm-cov --all --text` — coverage improved for the targeted files.
  - Update the coverage table in this document.
  - At the end of the last subtask, create a final report summarizing the coverage improvements and any remaining gaps.

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
