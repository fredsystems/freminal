# AGENTS.md — Freminal Workspace

## Project Overview

Freminal is a modern terminal emulator written in Rust (Edition 2024, MSRV 1.92.0). It targets
deep ANSI/DEC/xterm escape sequence compatibility, sub-millisecond frame times, and pixel-perfect
rendering via egui/glow.

The workspace contains five crates:

```text
freminal (binary — GUI application)
  ├── freminal-terminal-emulator (terminal emulation logic)
  │   ├── freminal-buffer (cell-based terminal buffer model)
  │   │   └── freminal-common (shared types and utilities)
  │   └── freminal-common
  └── freminal-common

xtask (build/CI orchestration — not production code)
```

### Architecture (Post-Refactor)

The `FairMutex` has been eliminated. There is no shared mutable state between the GUI thread and
the PTY-processing thread at steady state.

```text
PTY Processing Thread (owns TerminalEmulator exclusively)
  ├── Receives PtyRead from OS PTY reader thread
  ├── Receives InputEvent from GUI (keyboard, resize, focus)
  ├── After each batch: publishes Arc<TerminalSnapshot> via ArcSwap
  └── Sends WindowCommand to GUI for Report*/Viewport handling

GUI Thread (eframe update() — pure render, no mutation)
  ├── Loads TerminalSnapshot from ArcSwap (atomic, lock-free)
  ├── Sends InputEvent through crossbeam channel
  ├── Sends PtyWrite directly for Report* responses
  └── Owns ViewState (scroll offset, mouse, focus — never shared)
```

See `Documents/PERFORMANCE_PLAN.md` Sections 4-6 for detailed architecture diagrams and the
full implementation history.

---

## Non-Negotiable Rules

- No unsafe code unless explicitly requested
- Prefer clarity over cleverness
- No public APIs without tests
- No breaking changes without explanation
- All observable behavior must be testable
- Correctness always takes precedence over performance
- AGENTS.md is authoritative — agents must not reinterpret, weaken, or "improve" rules found here
- If a rule appears inconsistent with the codebase, stop and ask
- Changes must not break the lock-free architecture (ArcSwap snapshot model, channel-based input)
- Respect crate dependency boundaries — never introduce upward dependencies

### Panic-Free Production Code

- `unwrap()` and `expect()` are forbidden in all production code
- Panics must never be used to enforce invariants
- All invariant violations must surface as typed, recoverable errors
- The only permitted use of `unwrap()` / `expect()` is in test code (`#[cfg(test)]` or `tests/`)
- Any production `unwrap` / `expect` is a correctness bug
- Enforced via: `#![deny(clippy::unwrap_used, clippy::expect_used)]`

### Error Handling

- Errors must be explicit, typed, and structured
- Prefer domain-specific error enums over generic errors
- Errors must be testable
- Do NOT use `anyhow` in library code (freminal-common, freminal-buffer, freminal-terminal-emulator)
- Error variants should encode what went wrong, not what to do

### Dead Code Policy

- `#[allow(dead_code)]` is forbidden in production modules
- Acceptable uses: test-only helpers, temporary refactors with an explicit TODO
- If code exists in production, it must be reachable or intentionally gated

---

## Crate-Specific Guidance

### freminal-common

Shared types and utilities only. No business logic. No platform-specific dependencies beyond what
is needed for type definitions. No terminal semantics. Changes here affect all downstream crates.

### freminal-buffer

Pure data model for terminal content. Responsible for cells, rows, cursor tracking, wrapping, and
producing explicit mutation results (damage/diffs). Does NOT parse escape sequences, implement
terminal semantics, perform rendering, interact with UI frameworks, or access OS/platform APIs.
No global state. All state transitions must be explicit, localized, observable, and testable.
Hidden side effects are forbidden.

#### Buffer Invariants

- A `Cell` is the smallest addressable unit — always valid, empty cells are explicit
- Rows own cells, have a fixed width, wrapping produces new rows (not hidden overflow)
- Logical vs physical rows must be explicit
- Cursor movement does not mutate cells — mutations happen through explicit operations
- All mutations must return a structured description of what changed

### freminal-terminal-emulator

ANSI parser and terminal state machine. Owns the `TerminalState` and `TerminalHandler` which drive
buffer mutations. Produces `TerminalSnapshot` for the GUI via `build_snapshot()`. Owns the
`FreminalAnsiParser`. Does NOT render, interact with egui, or hold GUI state.

### freminal (binary)

GUI application using eframe/egui. The render loop in `update()` must be a pure read of
`TerminalSnapshot` — no terminal state mutation. All input goes through `Sender<InputEvent>`.
`ViewState` (scroll offset, mouse position, focus, etc.) is owned entirely by the GUI and never
shared with the PTY thread.

### xtask

Build and CI orchestration tool. Not production code. `anyhow` / `color-eyre` are acceptable here.
Provides subcommands: `ci`, `build`, `check`, `lint`, `test`, `coverage`, `deny`, `machete`.

---

## Development Environment & Verification

### Build & Test Commands

| Command                                                    | Purpose                                       |
| ---------------------------------------------------------- | --------------------------------------------- |
| `cargo xtask ci`                                           | Full CI: lint + deny + machete + build + test |
| `cargo test --all`                                         | Run all unit and integration tests            |
| `cargo clippy --all-targets --all-features -- -D warnings` | Lint with strict warnings                     |
| `cargo-machete`                                            | Detect unused dependencies                    |
| `cargo bench --all`                                        | Run all benchmarks (Criterion)                |
| `cargo xtask coverage`                                     | Generate coverage report (lcov)               |
| `cargo fmt --all -- --check`                               | Check formatting                              |

### Full Verification Suite

All agents must run the following before reporting completion:

1. `cargo test --all` — all tests pass
2. `cargo clippy --all-targets --all-features -- -D warnings` — no warnings
3. `cargo-machete` — no unused dependencies

If any step fails, fix the issue before proceeding.

### Tooling Notes

- Missing tools indicate an incomplete environment, not broken code
- Agents must not work around missing tools by modifying logic
- If additional tooling is required, stop and ask
- Nix devshell is the preferred environment (`nix develop` or `direnv allow`)

---

## Orchestrator Protocol

When acting as an orchestrator (decomposing a task into sub-agent work), follow this protocol:

### Task Decomposition

1. **Analyze scope** — Identify which crates and files are affected
2. **Identify parallelism** — Tasks touching different crates or independent features can run in
   parallel. Tasks with data dependencies must be sequential.
3. **Define sub-tasks** — Each sub-task must specify:
   - Exact scope (which files/modules to modify)
   - Clear success criteria
   - Verification steps
   - What NOT to touch (scope boundaries)
4. **Launch sub-agents** — Use parallel sub-agents for independent work. Chain sequential
   sub-agents for dependent work.
5. **Verify integration** — After all sub-agents report, run the full verification suite to
   confirm the combined changes work together.

### Parallelism Patterns

**Crate-level:** Different sub-agents work on different crates simultaneously. Safe when changes
do not cross crate boundaries in incompatible ways.

**Task-type:** One sub-agent implements, another writes tests, another reviews. The implementation
sub-agent must complete before the test sub-agent starts if tests depend on new APIs.

**Feature-level:** Different sub-agents implement independent features. Safe when features do not
modify the same files.

### Scope Enforcement

- Sub-agents must not modify files outside their assigned scope
- If a sub-agent discovers it needs to modify files outside scope, it must stop and report back
- The orchestrator resolves cross-scope dependencies by reassigning or sequencing work

---

## Sub-Agent Execution Protocol

When executing a focused task assigned by an orchestrator (or directly by the user):

1. **Read the full task description** before starting any work
2. **Execute only the assigned scope** — do not refactor unrelated code
3. **Keep diffs minimal and focused** — one concern per change
4. **Run the verification suite** before reporting completion:
   - `cargo test --all`
   - `cargo clippy --all-targets --all-features -- -D warnings`
   - `cargo-machete`
5. **Report back** with:
   - Summary of changes made
   - Files modified
   - Verification results
   - Any issues or questions discovered
6. **If blocked or unclear, stop and report** — do not guess

---

## Multi-Step Task Protocol

For tasks with ordered dependencies (e.g., multi-phase refactors), follow this protocol. This is
generalized from the protocol used successfully in `Documents/PERFORMANCE_PLAN.md`.

1. Read the entire task document before doing anything
2. Find the first incomplete step
3. Execute that one step and nothing else
4. Run the verification suite — confirm it passes
5. Update the tracking document: mark the step complete, add a brief completion note
6. Stop and post a summary — wait for user confirmation before continuing

**Do not execute multiple steps in one session, even if they seem small.**
**Do not proceed to the next step without explicit user confirmation.**
**Each step must leave `cargo test --all` passing.**

---

## Mandatory Testing & Benchmarking Rules

These rules apply to ALL agents and ALL implementation work going forward.

### Testing Is Mandatory

- Every new feature, bug fix, or refactor MUST include tests that cover the new/changed behavior.
- "It compiles and existing tests pass" is insufficient — new code must have NEW tests.
- If an area has no existing tests, the implementing agent must create the test infrastructure
  (test module, test helpers, fixtures) as part of the task.
- Task completion is contingent on all tests passing. A task is NOT complete until
  `cargo test --all` passes with zero failures.

### Benchmarking for Performance-Sensitive Code

- If changes touch the **rendering pipeline**, **PTY I/O**, or **buffer operations**, the agent
  MUST capture benchmark numbers **before and after** the change and include them in the
  completion report.
- Relevant benchmark suites:
  - Rendering: `freminal/benches/render_loop_bench.rs`
  - Buffer: `freminal-buffer/benches/buffer_row_bench.rs`
  - Emulator/Parser: `freminal-terminal-emulator/benches/buffer_benches.rs`
- If no appropriate benchmark exists for the code being changed, the agent MUST create a new
  benchmark as part of the task before proceeding with the change.
- Performance regressions must be justified and documented, or the change must be revised.

### Plan Document Maintenance

- Each major task has a planning document in `Documents/PLAN_XX_*.md`.
- The agent executing a task MUST update its plan document with:
  - Subtask completion status (mark completed items, add completion dates)
  - Any deviations from the plan with justification
  - Benchmark results if applicable
  - Issues discovered during implementation
- The master plan (`Documents/MASTER_PLAN.md`) must also be updated when a major task
  changes status (started, blocked, completed).

### Task Completion Criteria

A task is complete ONLY when ALL of the following are true:

1. All subtasks in the plan document are marked complete
2. `cargo test --all` passes
3. `cargo clippy --all-targets --all-features -- -D warnings` passes
4. `cargo-machete` passes
5. Benchmarks show no unexplained regressions (for render/PTY/buffer changes)
6. Plan document is updated with completion status and notes

---

## Working Modes

Agents may be instructed to operate in one of the following modes:

### READ_ONLY_AUDIT

- No code changes
- Identify broken invariants, dead code, or inconsistencies

### DESIGN_CRITIQUE

- Compare implementation to intended architecture
- Identify architectural drift or unclear responsibilities

### TEST_GAP_ANALYSIS

- Identify missing test coverage
- Describe untested scenarios

### PATCH_PROPOSAL

- Describe intended changes
- Explain why they are correct
- Identify risks

### PATCH_IMPLEMENTATION

- Implement only the approved proposal
- Keep diffs minimal
- Update tests as needed

---

## Testing Philosophy

Testing is first-class code.

- Every non-trivial behavior must be tested
- Tests must document a specific invariant
- Success and failure cases are required unless impossible
- Bug fixes must include regression tests

Tests must be:

- Hermetic
- Order-independent
- Focused on observable behavior
- Written for humans first

Duplication in tests is acceptable if it improves clarity.

Coverage target: 100% across crates.

---

## Code Style

- Idiomatic, clippy-clean Rust
- Prefer small, composable functions
- Avoid macros unless clearly justified
- Document public types and functions
- Prefer explicit types for clarity
- Follow standard Rust naming conventions

---

## AI-Specific Rules

- Do NOT invent APIs
- Do NOT guess terminal semantics
- Do NOT silently change behavior
- Do NOT refactor unrelated code
- Do NOT create new markdown files unless explicitly requested
- If intent is unclear, stop and ask

---

## Documentation Rules

- Do NOT create new markdown files by default
- Documentation must serve a clear, durable purpose
- Propose documentation changes before creating files
- Avoid duplicating information already present

---

## When to Stop

Stop and ask if:

- Requirements are ambiguous
- A change would weaken invariants
- Behavior is unclear or under-specified
- The agent is tempted to "fill in" missing semantics
- The agent feels unsure but thinks it can guess
- A sub-task requires modifying files outside its assigned scope

Correctness > completeness > speed.
