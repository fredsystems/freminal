# AGENTS.md

## Project Objective

This crate implements a **cell-based terminal buffer model** intended for
eventual integration into the Freminal terminal emulator.

The buffer is responsible for:

- Representing terminal content as structured cells
- Managing rows, wrapping, and logical vs physical lines
- Tracking cursor position and movement
- Producing explicit, inspectable mutation results (e.g. damage / diffs)

This crate does **not**:

- Parse escape sequences
- Implement terminal semantics (VT100, xterm, etc.)
- Perform rendering or font measurement
- Interact with UI frameworks
- Optimize for performance prematurely

Correctness, clarity, and explicit invariants are paramount.

If unsure, stop and ask.

---

## Non-Negotiable Rules

- No unsafe code unless explicitly requested
- Prefer clarity over cleverness
- No public APIs without tests
- No breaking changes without explanation
- All observable behavior must be testable
- Correctness always takes precedence over performance
- AGENTS.md is authoritative
- Agents must not reinterpret, weaken, or “improve” rules found here
- If a rule appears inconsistent with the codebase, stop and ask

---

## Architectural Constraints

- The buffer is a **pure data model**
- No rendering, font, or UI concerns
- No terminal escape parsing
- No OS or platform assumptions
- No global state

All state transitions must be:

- Explicit
- Localized
- Observable
- Testable

Hidden side effects are forbidden.

---

## Development Environment & Tooling

- The crate is developed using standard Rust tooling
- Missing tools indicate an incomplete environment, not broken code
- Agents must not work around missing tools by modifying logic
- If additional tooling is required, stop and ask

---

## Panic-Free Production Code (Non-Negotiable)

- `unwrap()` and `expect()` are **forbidden** in all production code
- Panics must never be used to enforce invariants
- All invariant violations must surface as typed, recoverable errors

The only permitted use of `unwrap()` / `expect()` is in:

- Test code (`#[cfg(test)]` or `tests/`)
- Test-only helpers

Any production `unwrap` / `expect` is a correctness bug.

This is enforced via:

```rust
#![deny(clippy::unwrap_used, clippy::expect_used)]
```

---

## Error Handling

- Errors must be explicit, typed, and structured
- Prefer domain-specific error enums over generic errors
- Errors must be testable
- Do NOT use `anyhow` in library code
- Error variants should encode _what went wrong_, not _what to do_

---

## Code Style

- Idiomatic, clippy-clean Rust
- Prefer small, composable functions
- Avoid macros unless clearly justified
- Document public types and functions
- Prefer explicit types for clarity
- Follow standard Rust naming conventions

### Dead Code Policy

- `#[allow(dead_code)]` is forbidden in production modules
- Acceptable uses:
  - test-only helpers
  - temporary refactors with an explicit TODO
- If code exists in production, it must be reachable or intentionally gated

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

---

## Invariants & Semantics

### Cells

- A `Cell` is the smallest addressable unit
- A cell is always valid
- Empty cells are explicit, not implicit
- Cells do not know about fonts or glyph metrics

### Rows

- Rows own cells
- Rows have a fixed width
- Wrapping produces new rows, not hidden overflow
- Logical vs physical rows must be explicit

### Cursor

- Cursor movement does not mutate cells
- Mutations happen through explicit operations
- Cursor state must always be internally consistent

### Mutation Model

- All mutations must be explicit
- Mutations should return a structured description of what changed
- Silent state changes are forbidden

---

## Agent Working Modes

Agents may be instructed to operate in one of the following modes:

### READ_ONLY_AUDIT

- No code changes
- Identify broken invariants, dead code, or inconsistencies

### DESIGN_CRITIQUE

- Compare implementation to intended buffer model
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

## AI-Specific Rules

- Do NOT invent APIs
- Do NOT guess terminal semantics
- Do NOT silently change behavior
- Do NOT refactor unrelated code
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
- The agent is tempted to “fill in” missing semantics
- The agent feels unsure but thinks it can guess

Correctness > completeness > speed.
