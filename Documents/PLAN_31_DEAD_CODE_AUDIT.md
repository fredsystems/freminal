# PLAN_31 — Dead Code Audit

## Status: Stub

---

## Overview

Rust's dead code lint only fires on _private_ items. Any item marked `pub` — whether it is
actually used by downstream crates or not — is invisible to `rustc`'s and clippy's dead code
analysis. Over the course of 25+ completed tasks, APIs have been added, refactored, and
superseded, leaving behind `pub` functions, methods, structs, fields, and enum variants that no
live code path reaches. These phantom-public items:

- Inflate the API surface and confuse onboarding.
- Mask genuine dead code that the linter would otherwise catch.
- Bloat the binary with unreachable code that LTO may or may not eliminate.
- Create false confidence that the item is exercised by tests (it appears reachable, but nothing
  calls it).

This task performs a systematic audit of every `pub` item across all four library crates and the
binary, identifies items with zero downstream callers, and removes them (or demotes their
visibility to `pub(crate)` / private if internal callers exist).

**Dependencies:** None (independent)
**Dependents:** Task 29 (God File Refactoring) — a smaller API surface makes splitting easier
**Primary crates:** All (`freminal`, `freminal-terminal-emulator`, `freminal-buffer`,
`freminal-common`)
**Estimated scope:** Unknown until audit completes

---

## Why This Is a Stub

The subtask list cannot be written until the audit is performed. The codebase has been through
extensive refactoring (performance plan, code quality task, escape sequence overhaul, etc.) and
each phase likely left behind public items whose callers were removed or relocated. A manual
audit is required because automated tools cannot reliably detect this across workspace crate
boundaries.

## The Problem with `pub`

Consider:

```rust
// freminal-buffer/src/buffer.rs
pub fn some_helper(&self) -> usize { ... }
```

If `some_helper` is only called from within `freminal-buffer`, it should be `pub(crate)`. If it
is not called at all, it should be deleted. But because it is `pub`, neither `rustc` nor clippy
will warn about it — the compiler assumes an external crate _might_ use it.

This is especially problematic at crate boundaries:

- `freminal-common` exports types consumed by all downstream crates. A type or method that was
  needed by `freminal-terminal-emulator` during an earlier design phase may now be unused, but
  `pub` visibility keeps it hidden from the linter.
- `freminal-buffer` exports buffer operations consumed by `freminal-terminal-emulator`. Methods
  added for features that were later redesigned may still be `pub`.
- `freminal-terminal-emulator` exports the emulator API consumed by `freminal`. Methods that
  existed for the old `FairMutex` locking model may be orphaned.

## Audit Procedure

When the audit is initiated:

1. **Enumerate all `pub` items** across the four library crates using a combination of:
   - `cargo doc --document-private-items` to see the full API surface.
   - `grep -rn '^pub ' --include='*.rs'` for a raw list.
   - IDE "find usages" or `rg` for cross-crate reference checking.

2. **For each `pub` item, determine its caller set:**
   - **Zero callers anywhere:** Dead code. Delete it.
   - **Callers only within the same crate:** Demote to `pub(crate)`.
   - **Callers in downstream crates:** Legitimate `pub`. Keep it.
   - **Callers only in tests:** Consider whether the item should be `#[cfg(test)]`-gated or
     moved to a test helper module.

3. **Check for transitively dead trees:** A `pub` function that calls three other `pub` functions
   — if the root is dead, all three callees may also become dead once the root is removed. Audit
   iteratively until no new dead items are found.

4. **Categorize findings** into actionable subtasks grouped by crate.

5. **Write the subtask list** in this document. Update status from "Stub" to "Pending".

### Tools

- `cargo-udeps` — detects unused _dependencies_, not unused code, but useful as a cross-check.
- `cargo-machete` — already in CI; confirms no unused crate deps.
- Manual `rg` (ripgrep) queries are the primary tool: `rg 'fn some_helper' --include='*.rs'`
  to find definitions, `rg 'some_helper' --include='*.rs'` to find all references.
- `rust-analyzer` IDE features (if available) for "find all references" on a per-item basis.

### Known Likely Sources of Dead `pub` Items

These areas have undergone significant refactoring and are likely to harbor dead public APIs:

| Area                       | Refactor That May Have Orphaned Items                                  |
| -------------------------- | ---------------------------------------------------------------------- |
| `TerminalEmulator` methods | Performance plan: FairMutex elimination moved GUI state to `ViewState` |
| `TerminalState` methods    | Task 5 (GUI fields removed), Task 25 (dead `Theme`, dead `scroll()`)   |
| `Buffer` methods           | Performance plan: `scroll_offset` externalized; Task 25 code quality   |
| `TerminalHandler` methods  | Extensive escape sequence work (Tasks 7, 10, 20, 21)                   |
| `freminal-common` types    | Types added for early designs that were later redesigned               |
| Snapshot/IO types          | Performance plan introduced new types; old intermediaries may remain   |

## Subtasks

To be created after the audit. Expected categories:

- Delete dead `pub` items with zero callers
- Demote `pub` to `pub(crate)` for items only used within their crate
- Gate test-only items behind `#[cfg(test)]`
- Document findings for items that appear dead but are intentionally kept (e.g., future use)

---

## References

- `agents.md` — Dead Code Policy: "`#[allow(dead_code)]` is forbidden in production modules"
- `Documents/MASTER_PLAN.md` — Task 31 entry
- `Documents/PLAN_25_CODE_QUALITY.md` — Previous dead code removal (Theme, scroll, StandardOutput)
- `Documents/PERFORMANCE_PLAN.md` — Section 5: "What Gets Deleted" (may have left remnants)
