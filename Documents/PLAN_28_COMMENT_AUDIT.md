# PLAN_28 — Code Comment Audit

## Status: Stub

---

## Overview

The codebase has generally good documentation and comments, but no systematic audit has been
performed to verify that:

1. **Comments are accurate** — code evolves, comments sometimes don't. Stale or misleading
   comments are worse than no comments.
2. **Public APIs are documented** — all `pub` functions, types, and modules should have `///`
   doc comments explaining what they do, not just how.
3. **Complex logic is explained** — non-obvious algorithms, tricky edge cases, and "why"
   decisions should have inline comments.
4. **Comments are not redundant** — comments that merely restate what the code already says
   clearly add noise.

This task audits the entire codebase for comment quality and produces actionable subtasks.

**Dependencies:** None (independent)
**Dependents:** None
**Primary crates:** All (`freminal`, `freminal-terminal-emulator`, `freminal-buffer`,
`freminal-common`, `xtask`)
**Estimated scope:** Unknown until audit completes

---

## Why This Is a Stub

The subtask list cannot be written until the audit is performed. The audit will examine every
file in the workspace and assess comment quality along four dimensions:

1. **Accuracy:** Do existing comments match the current code behavior? Flag any that describe
   old behavior, reference deleted types/functions, or contradict what the code actually does.

2. **Coverage:** Are public APIs (`pub fn`, `pub struct`, `pub enum`, `pub mod`) documented
   with `///` doc comments? Are complex private functions documented? Are module-level `//!`
   comments present where helpful?

3. **Depth:** For complex logic (e.g., the reflow algorithm in `buffer.rs`, the parser state
   machine in `ansi.rs`, the snapshot caching in `interface.rs`), is there enough explanation
   for a new contributor to understand the design intent without reading every line?

4. **Noise:** Are there comments that add no value? Examples: `// increment counter` above
   `counter += 1`, comments that restate a function's already-clear name, commented-out code
   that should have been deleted.

## Audit Procedure

When the audit is initiated:

1. Walk each crate in dependency order: `freminal-common` → `freminal-buffer` →
   `freminal-terminal-emulator` → `freminal` → `xtask`.
2. For each file, assess the four dimensions above.
3. Categorize findings by severity:
   - **Incorrect** — comment contradicts code (highest priority).
   - **Missing** — public API or complex logic lacks documentation.
   - **Stale** — comment references old behavior or deleted code.
   - **Noisy** — comment adds no information.
4. Group findings into actionable subtasks.
5. Write the subtask list in this document.
6. Update the status from "Stub" to "Pending".

## Subtasks

To be created after the audit. Expected categories:

- Fix incorrect comments (highest priority — misleading comments are bugs)
- Add missing doc comments to public APIs
- Add explanatory comments to complex algorithms
- Remove stale comments referencing old code
- Remove noise comments that restate the obvious

---

## References

- `agents.md` — Agent rules, documentation rules
- `Documents/MASTER_PLAN.md` — Task 28 entry
