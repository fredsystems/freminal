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

- [ ] **6.1 — Run `cargo llvm-cov` and produce baseline coverage report**
  - Install `cargo-llvm-cov` if not present (`cargo install cargo-llvm-cov`).
  - Run: `cargo llvm-cov --all --lcov --output-path lcov.info`
  - Run: `cargo llvm-cov --all --text` to get a human-readable summary.
  - Record the per-crate and per-file coverage percentages in Section "Coverage Baseline" below.
  - Identify all files with 0% coverage.
  - Identify all files below 50% coverage.
  - Create a report (in this document) listing these files, their coverage, and any initial observations.
  - Do NOT write any tests yet — this is pure analysis.
  - **Verify:** Coverage report is generated. Baseline numbers are recorded in this document.

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

> Populated by subtask 6.1.

| Crate                        | Files | Line Coverage | Branch Coverage | Notes |
| ---------------------------- | ----- | ------------- | --------------- | ----- |
| `freminal-common`            | —     | —             | —               | —     |
| `freminal-buffer`            | —     | —             | —               | —     |
| `freminal-terminal-emulator` | —     | —             | —               | —     |
| `freminal` (binary)          | —     | —             | —               | —     |
| **Total**                    | —     | —             | —               | —     |

### Zero-Coverage Files

> Populated by subtask 6.1.

### Below-50% Files

> Populated by subtask 6.1.

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
