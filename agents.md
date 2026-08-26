# AGENTS.md -- Freminal Workspace

This document is the always-on orientation for AI coding agents working
in the Freminal workspace. **Operational procedures are no longer
inlined here** -- they live as opencode skills (the shared ones are
installed at `~/.config/opencode/skills/` from fred's nixos config
repo; freminal-specific ones live in this repo's `.opencode/skills/`)
and are loaded on demand. This document gives you the map; the skills
give you the moves.

---

## Execution model

This repo uses the shared orchestration model. `autonomy-boundaries`
governs when to continue versus stop: the assigned scope is the
boundary, not a step count, and irreversible operations still need
explicit approval. `agent-orchestration-protocol` governs sub-agent
scoping, `plan-decomposition` governs turning plans into subtasks,
`plan-sequencing-discipline` governs the numbered plan set (ordering,
cross-plan dependencies, status at merge), and
`parallel-work-isolation` governs concurrent work.

**"Do version X" means all of version X, front to back, and nothing
else.** Do not stop between subtasks to ask permission for work that is
already in the plan. Do stop the moment a hard trigger fires -- see
`autonomy-boundaries` for the full list, and "When to Stop" below for
the freminal-specific ones.

Note what that declaration does. opencode discovers the shared skills
globally and loads them by description match, so their presence is not
opt-in. What this section opts into is **authority**: `agents.md` is
always-on core context and therefore outranks an on-demand skill, so
without this declaration this file would win any conflict.

If you want the older one-step-at-a-time cadence for a particular
session, say so in the prompt (`Step-by-step mode: ...`) and it takes
precedence for that session.

---

## Project Overview

Freminal is a modern terminal emulator written in Rust (Edition 2024,
MSRV 1.96.0). It targets deep ANSI/DEC/xterm escape-sequence
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
`freminal-architecture` skill.** See also `Documents/DESIGN_DECISIONS.md`
("Multi-Window Architecture", "Render Loop Optimization", "Built-in
Multiplexer / PaneTree") for the durable rationale.

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
| `freminal-bench-table`             | Touching render / PTY / buffer / parser / `build_snapshot`. Names which bench file covers what (procedure lives in `performance-benchmarks`).     |
| `freminal-damage-model`            | Damage derivation and `PaneFrameDamage` / `FrameDamage` / `ChromeDamage`; classify before using `Full`.                                           |
| `freminal-frec-decoder`            | Analyzing `.frec` / `.bin` recording files. Use `sequence_decoder.py`, not ad-hoc parsers.                                                        |
| `freminal-escape-sequence-docs`    | Adding / removing / altering escape sequence support. Dual-doc update required.                                                                    |
| `freminal-extend-or-extract`       | About to make something bigger rather than give it a home: a field only an outside reader needs, a branch on an already-long function, an extra parameter, a copied computation, a helper extracted only to make a test possible, a test re-implementing production logic, a `too_many_lines` / `too_many_arguments` allow. Also "should this be a new module/crate". |
| `freminal-numeric-conversions`     | Numeric type conversions. `conv2` crate; no raw `as` in production.                                                                               |
| `freminal-config-options`          | Adding / renaming / removing a config option (`Config` field in `config.rs`). Mandatory `ConfigPartial` / `apply_partial` wiring checklist.       |
| `freminal-plan-status-lifecycle`   | Changing task / version status in `MASTER_PLAN.md` (esp. when a PR merges). Two-tables-agree invariant; merge is the `Complete` trigger.          |
| `freminal-state-representation`     | About to add a `bool` field or `bool` parameter, pass a bare `true` / `false`, add a bool pair that can't both be true, transport a bool across a crate/thread/frame boundary, or lean on an `excessive_bools` allow. Named domain enums (`BlinkState::Enabled`); and the three cases where a bool is correct. |
| `freminal-modal-input-suppression` | Adding / debugging a GUI modal, dialog, or overlay with a text field. Register in `ui_overlay_open` + `lock_focus(true)` or it can't be typed in. |
| `freminal-module-cohesion`         | About to add a type to an existing file whose name describes a different concept, add a second unrelated test module, or add to a file you had to scroll to the end of. One *concept* per module; path should name the concept; decline splits that widen visibility. |
| `freminal-windows-crosscheck`      | Before any PR, esp. `#[cfg(windows)]` / `portable-pty` / path / thread changes. Run `cargo xtask check-windows` (clippy for windows-gnu) locally. |
| `agent-orchestration-protocol`     | About to spawn sub-agents. Action classes, scope, prohibitions, stop conditions. Mandatory.                                                       |
| `autonomy-boundaries`              | Executing a multi-step plan. Scope is the boundary, not a step count; irreversible operations need approval.                                      |
| `plan-decomposition`               | Turning a version or epic into implementable subtasks.                                                                                            |
| `plan-sequencing-discipline`       | Maintaining the numbered plan set: ordering, cross-plan deps, merge barrier, status at merge, index drift.                                        |
| `parallel-work-isolation`          | Running more than one implementation agent at once. Worktrees, foundation-first, merge-back.                                                      |
| `module-cohesion`                  | Generic one-concept-per-module rule (freminal specifics in `freminal-module-cohesion`).                                                            |
| `state-representation`             | Generic bool-to-enum rule (freminal high-risk sites in `freminal-state-representation`).                                                           |
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

`freminal-architecture` adds that raw `bool` is OK "only when no enum
exists". **That hatch is narrower than it reads**, and
`freminal-state-representation` takes precedence: if the value is
transported -- through `TerminalSnapshot`, a channel, or any public
signature -- create the enum rather than taking the hatch. Every mode
reaching the GUI crosses the snapshot boundary, so that is nearly all
of them. A raw `bool` is fine only for a value that never leaves the
function computing it.

That rule generalises beyond modes: state is a **named domain enum**
(`BlinkState::Enabled`), never a bare `bool` and never a shared generic
`Enabled` / `Disabled`. Bool *parameters* are forbidden outright. The
three cases where a `bool` is still correct -- independent simultaneous
signals, modifier sets, and TOML config toggles -- are enumerated in
`freminal-state-representation`, which also records that the runtime
cost of the fix is measured at zero.

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
| `cargo xtask check-windows`                                | Clippy for windows-gnu (Windows cross-check; `default` shell) |

### Verification suite (mandatory before "done")

1. `cargo test --all`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo machete`

Additionally, **before opening a PR** — especially for changes touching
`#[cfg(windows)]` code, `portable-pty`, cross-platform paths, or
threads/closures — run `cargo xtask check-windows` from the `default`
dev shell to catch Windows-only compile errors and lints locally. See
the `freminal-windows-crosscheck` skill.

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
documented in `agent-orchestration-protocol`. Use that when
decomposing.

---

## Multi-Step Task Protocol

For tasks with ordered dependencies (e.g. multi-phase refactors):

1. Read the entire task document before doing anything.
2. Reconcile the document against git before trusting it: check the
   current branch, whether it is pushed, and whether it is ahead of
   or behind the target branch -- recent commits may already
   implement steps the document still shows as open. A plan document
   records intent; only git records what actually landed. Where they
   disagree on *bookkeeping* -- a step done but still shown open --
   git wins and the document gets corrected in passing (for
   `MASTER_PLAN.md` specifically, `freminal-plan-status-lifecycle`
   governs the status values). Where they disagree on *substance* --
   the plan's technical premise no longer matches the code -- that is
   an `autonomy-boundaries` hard stop, not a correction.
3. Find the first incomplete step.
4. Execute it. Keep its scope exactly as written.
5. Run the verification suite -- confirm it passes.
6. Update the tracking document: mark the step complete, add a brief
   note.
7. Continue to the next step.

Each step must leave `cargo test --all` passing before the next one
starts -- a red suite is a full stop, not something to fix on the way
past. `autonomy-boundaries` governs when to continue and when to stop;
the assigned scope is the boundary, not a step count. Finish the
version, then report.

Pre-existing bugs surfaced mid-task become numbered cleanup entries in
the host task's plan document (see Task 72.16 in
`Documents/PLAN_VERSION_090.md` for the convention -- that is the
freminal example of the generic procedure in
`agent-orchestration-protocol`).

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

`autonomy-boundaries` carries the generic trigger list, including the
rule that irreversible operations need approval even when they are in
scope. These are the freminal-specific additions:

- Requirements are ambiguous, or behavior is unclear / under-specified.
- A change would weaken a stated invariant -- particularly the
  lock-free architecture or the crate dependency direction.
- You're tempted to "fill in" missing terminal semantics. Look it up in
  the spec or stop; do not infer what an escape sequence "probably"
  does.
- A sub-task requires modifying files outside its assigned scope.
- A plan document marks the task `TENTATIVE` or "needs maintainer
  approval".
- A test needs its tolerance loosened to pass. That is a red flag, not
  a fix.

Note what is deliberately **not** on this list any more: "you feel
unsure but think you can guess". It was unfalsifiable -- an agent can
always claim it, so it licensed stopping anywhere. Uncertainty about
*terminal semantics* is a real stop (above); a general feeling of
unease about correct in-scope work is not.

Correctness > completeness > speed.
