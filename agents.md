# AGENTS.md -- Freminal Workspace

This document is the always-on orientation for AI coding agents working
in the Freminal workspace. **Operational procedures are no longer
inlined here** -- they live as opencode skills (sourced from fred's
nixos config repo at `~/GitHub/nixos/.opencode/skills/`, wired in via
this repo's `opencode.json`) and are loaded on demand. This document
gives you the map; the skills give you the moves.

---

## Project Overview

Freminal is a modern terminal emulator written in Rust (Edition 2024,
MSRV 1.95.0). It targets deep ANSI/DEC/xterm escape-sequence
compatibility, sub-millisecond frame times, and pixel-perfect
rendering via egui/glow.

### Workspace layout

```text
freminal (binary -- GUI application)
  ├── freminal-terminal-emulator (terminal emulation logic)
  │   ├── freminal-buffer (cell-based terminal buffer model)
  │   │   └── freminal-common (shared types and utilities)
  │   └── freminal-common
  └── freminal-common

xtask (build/CI orchestration -- not production code)
```

### Architecture, in one paragraph

The `FairMutex` has been eliminated. The PTY-processing thread owns
`TerminalEmulator` exclusively and publishes `Arc<TerminalSnapshot>` via
`ArcSwap`. The GUI thread is a pure read of that snapshot and sends
input via a `crossbeam` channel. `ViewState` (scroll, mouse, focus)
lives entirely on the GUI side and is never shared. Crate dependencies
point one direction: `freminal` -> `freminal-terminal-emulator` ->
`freminal-buffer` -> `freminal-common`. **Full invariants and the
"don't accidentally regress this" rules are in the
`freminal-architecture` skill.** See also `Documents/PERFORMANCE_PLAN.md`
sections 4-6 for the historical context.

---

## Non-Negotiable Rules

These are always-on. The expanded forms live in skills, but the
headlines:

- No unsafe code unless explicitly requested.
- Prefer clarity over cleverness.
- No public APIs without tests.
- No breaking changes without explanation.
- All observable behavior must be testable.
- Correctness > performance.
- AGENTS.md and skills are authoritative -- agents must not
  reinterpret, weaken, or "improve" rules.
- If a rule appears inconsistent with the codebase, stop and ask.
- Changes must not break the lock-free architecture.
- Respect crate dependency boundaries.
- **Panic-free production code**: `unwrap()` / `expect()` forbidden
  outside `#[cfg(test)]` / `tests/`. Enforced by
  `#![deny(clippy::unwrap_used, clippy::expect_used)]`.
- **Errors must be explicit, typed, and structured.** No `anyhow` in
  library crates (`freminal-common`, `freminal-buffer`,
  `freminal-terminal-emulator`); `anyhow` / `color-eyre` OK in
  `xtask`. Error variants encode what went wrong, not what to do.
- **No `#[allow(dead_code)]` in production modules.** Acceptable only
  for test-only helpers and temporary refactors with an explicit
  TODO.

The `rust-best-practices` skill expands the panic/dead-code/cast rules.
The `freminal-numeric-conversions` skill expands the `as`-casts /
`conv2` policy.

---

## Skills you will need in this repo

| Skill                              | When it fires                                                                                                                                     |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `freminal-architecture`            | Architecture-affecting changes (GUI/PTY split, snapshot transport, crate boundaries).                                                             |
| `freminal-orchestrator-protocol`   | About to spawn sub-agents. The action-class / scope / stop-condition discipline is mandatory.                                                     |
| `freminal-bench-table`             | Touching render / PTY / buffer / parser / `build_snapshot`. Names which bench file covers what (procedure lives in `performance-benchmarks`).     |
| `freminal-frec-decoder`            | Analyzing `.frec` / `.bin` recording files. Use `sequence_decoder.py`, not ad-hoc parsers.                                                        |
| `freminal-escape-sequence-docs`    | Adding / removing / altering escape sequence support. Dual-doc update required.                                                                   |
| `freminal-numeric-conversions`     | Numeric type conversions. `conv2` crate; no raw `as` in production.                                                                               |
| `freminal-config-options`          | Adding / renaming / removing a config option (`Config` field in `config.rs`). Mandatory `ConfigPartial` / `apply_partial` wiring checklist.       |
| `freminal-modal-input-suppression` | Adding / debugging a GUI modal, dialog, or overlay with a text field. Register in `ui_overlay_open` + `lock_focus(true)` or it can't be typed in. |
| `rust-best-practices`              | Any Rust edit. Panic-free production, clippy maxed, no bypass.                                                                                    |
| `performance-benchmarks`           | Generic before/after capture procedure and 15% regression threshold (used together with `freminal-bench-table`).                                  |
| `flake-dev-shell-discipline`       | About to need a system tool not in the dev shell. Add to `flake.nix`, stop, wait for `nix develop`.                                               |
| `precommit-fix-loop`               | When a commit is rejected by pre-commit hooks.                                                                                                    |
| `commit-discipline`                | Before any commit / PR. Plan-subtask numbering convention is freminal-specific.                                                                   |
| `testing-mandate`                  | Before declaring any task done.                                                                                                                   |
| `no-summary-documents`             | Before creating any new markdown file (no PHASE_X_SUMMARY.md, no IMPLEMENTATION_PROGRESS.md, etc.).                                               |
| `markdown-lint-discipline`         | Before writing or editing any `.md` file. Common markdownlint pitfalls (MD031, MD040, table widths).                                              |
| `flaky-tests-are-bugs`             | A test fails sporadically. Root-cause it; no retries / `#[ignore]` / longer timeouts.                                                             |

---

## Crate-specific guidance (one-paragraph each)

The full architecture invariants and what-not-to-leak rules live in the
`freminal-architecture` skill. Quick reference:

- **`freminal-common`** -- shared types and utilities only. No business
  logic. Changes here affect every downstream crate.
- **`freminal-buffer`** -- pure data model. No escape parsing, no
  rendering, no UI, no OS APIs. All mutations return a structured
  description of what changed.
- **`freminal-terminal-emulator`** -- ANSI parser and terminal state
  machine. Owns `TerminalState` / `TerminalHandler` / `FreminalAnsiParser`.
  Produces `TerminalSnapshot` via `build_snapshot()`. No rendering, no
  egui, no GUI state.
- **`freminal` (binary)** -- the GUI. `update()` is a pure read of the
  snapshot. All input flows through `Sender<InputEvent>`. `ViewState`
  is owned here, never shared.
- **`xtask`** -- build/CI orchestration. Subcommands: `ci`, `build`,
  `check`, `lint`, `test`, `coverage`, `deny`, `machete`.

### Terminal mode representation

If a mode has an enum in `freminal-common/src/buffer_states/modes/`,
that enum is the type used everywhere -- never a raw `bool`. See
`freminal-architecture` for the full surface.

### Keybindings

Every keyboard shortcut goes through the `BindingMap` system. The
four-step ritual (KeyAction variant, default binding, dispatch,
documentation in `config_example.toml`) is in `freminal-architecture`.
Hardcoded shortcuts outside `BindingMap` are forbidden.

---

## Development Environment & Verification

### Build & test commands

| Command                                                    | Purpose                                                       |
| ---------------------------------------------------------- | ------------------------------------------------------------- |
| `cargo xtask ci`                                           | Full CI: lint + deny + machete + build + test + bench compile |
| `cargo test --all`                                         | Run all unit and integration tests                            |
| `cargo clippy --all-targets --all-features -- -D warnings` | Lint with strict warnings                                     |
| `cargo machete`                                            | Detect unused dependencies                                    |
| `cargo bench --all`                                        | Run all benchmarks (Criterion)                                |
| `cargo bench --no-run --all`                               | Compile benchmarks without running                            |
| `cargo xtask coverage`                                     | Generate coverage report (lcov)                               |
| `cargo fmt --all -- --check`                               | Check formatting                                              |

### Verification suite (mandatory before "done")

1. `cargo test --all`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo machete`

If any step fails, fix it. Don't ship around it. The `testing-mandate`
skill expands the "what done means" definition.

### Tooling

- Nix devshell is the preferred environment (`nix develop` or
  `direnv allow`).
- Missing tools indicate an incomplete environment, not broken code.
  Don't work around missing tools by modifying logic. Full rule in
  the `flake-dev-shell-discipline` skill: add to `flake.nix`, then
  stop and wait for the user to run `nix develop`.

---

## Branch & Commit Workflow

- All implementation work happens on feature branches, never directly
  on `main`.
- Branch naming: `task-NN/short-description` (e.g. `task-02/cli-config`,
  `task-06/test-gaps`). One branch per major plan task.
- Commits follow conventional-commits format. Plan-subtask commits
  reference the subtask number: `refactor: 30.3 -- replace casting
suppressions in freminal-common`. Combining multiple subtasks into
  one commit is acceptable under specific conditions; see
  `commit-discipline`.
- Each commit must leave `cargo test --all` passing. No broken
  intermediate states.
- `--no-verify` is forbidden on commits. See `precommit-fix-loop` if a
  hook rejects.

---

## Working Modes

Agents may be instructed to operate in one of:

- **READ_ONLY_AUDIT** -- no code changes; find broken invariants, dead
  code, inconsistencies.
- **DESIGN_CRITIQUE** -- compare implementation to intended
  architecture; identify drift.
- **TEST_GAP_ANALYSIS** -- find missing test coverage; describe
  untested scenarios.
- **PATCH_PROPOSAL** -- describe intended changes; explain why
  correct; identify risks.
- **PATCH_IMPLEMENTATION** -- implement only the approved proposal;
  minimal diffs; update tests.

The orchestrator spawning sub-agents uses the more granular
**READ-ONLY / CODE-REVIEW / IMPLEMENTATION / COMMIT** action classes
documented in `freminal-orchestrator-protocol`. Use that when
decomposing.

---

## Multi-Step Task Protocol

For tasks with ordered dependencies (e.g. multi-phase refactors):

1. Read the entire task document before doing anything.
2. Find the first incomplete step.
3. Execute that one step and nothing else.
4. Run the verification suite -- confirm it passes.
5. Update the tracking document: mark the step complete, add a brief
   note.
6. Stop and post a summary in chat -- wait for user confirmation
   before continuing.

Do NOT execute multiple steps in one session, even if they seem
small. Do NOT proceed to the next step without explicit user
confirmation. Each step must leave `cargo test --all` passing.

Pre-existing bugs surfaced mid-task become numbered cleanup entries in
the host task's plan document (see Task 72.16 in
`Documents/PLAN_VERSION_090.md` for the convention). Full procedure in
`freminal-orchestrator-protocol`.

---

## Testing Philosophy (headlines)

Testing is first-class code. Tests must be hermetic, order-independent,
focused on observable behavior, written for humans first. Coverage
target: 100% across crates. Duplication in tests is acceptable if it
improves clarity. Full mandate in `testing-mandate`; benchmark
procedure in `performance-benchmarks` + freminal-specific catalog in
`freminal-bench-table`; flake rules in `flaky-tests-are-bugs`.

---

## Documentation Rules

- Do NOT create new markdown files by default. (See
  `no-summary-documents` for the full prohibition list.)
- Documentation must serve a clear, durable purpose.
- Propose documentation changes before creating files.
- Avoid duplicating information already present.
- **Escape-sequence changes have a mandatory dual-document update** --
  see `freminal-escape-sequence-docs`.

---

## AI-Specific Rules

- Do NOT invent APIs.
- Do NOT guess terminal semantics.
- Do NOT silently change behavior.
- Do NOT refactor unrelated code.
- Do NOT create new markdown files unless explicitly requested.
- If intent is unclear, stop and ask.

---

## When to Stop

Stop and ask if:

- Requirements are ambiguous.
- A change would weaken invariants.
- Behavior is unclear or under-specified.
- You're tempted to "fill in" missing semantics.
- You feel unsure but think you can guess.
- A sub-task requires modifying files outside its assigned scope.

Correctness > completeness > speed.
