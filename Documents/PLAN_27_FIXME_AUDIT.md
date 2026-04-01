# PLAN_27 — FIXME/TODO Audit

## Status: Stub

---

## Overview

The codebase contains `FIXME`, `TODO`, `HACK`, and `XXX` comments accumulated over the course of
development. Some of these mark genuine unfinished work, some are stale (the issue was fixed but
the comment was never removed), and some describe problems that no longer exist after subsequent
refactors.

This task audits every such marker comment across all five crates, assesses each for veracity and
relevance, and produces actionable subtasks for the ones that require mitigation.

**Dependencies:** None (independent)
**Dependents:** None
**Primary crates:** All (`freminal`, `freminal-terminal-emulator`, `freminal-buffer`,
`freminal-common`, `xtask`)
**Estimated scope:** Unknown until audit completes

---

## Why This Is a Stub

The subtask list cannot be written until the audit is performed. The audit will:

1. Search the entire codebase for `FIXME`, `TODO`, `HACK`, `XXX`, and any other marker comments.
2. For each marker, determine:
   - **Veracity:** Is the described problem real? Does it still exist?
   - **Relevance:** Is this still applicable after recent refactors (performance plan, code
     quality task, etc.)?
   - **Severity:** Is this a correctness issue, a performance issue, a cosmetic issue, or
     aspirational?
   - **Mitigation:** What concrete action is needed? (fix, delete the comment, convert to a
     proper issue, etc.)
3. Group the findings into actionable subtasks (e.g., "Remove 12 stale TODOs", "Fix 3
   correctness FIXMEs in buffer.rs", etc.).

## Audit Procedure

When the audit is initiated:

1. Run `grep -rn 'FIXME\|TODO\|HACK\|XXX' --include='*.rs'` across the workspace.
2. Exclude `target/`, `.git/`, and vendored dependencies.
3. For each hit, read the surrounding context (at least 10 lines) to understand what the
   comment is describing.
4. Categorize into: **Stale** (can be deleted), **Valid** (needs a fix), **Aspirational**
   (nice-to-have, not blocking), **Out of scope** (belongs in a different task).
5. Write the subtask list in this document.
6. Update the status from "Stub" to "Pending".

## Subtasks

To be created after the audit. Expected categories:

- Remove stale markers (comments describing problems that no longer exist)
- Fix valid markers (comments describing real issues that need code changes)
- Convert aspirational markers to tracked issues or future plan tasks
- Standardize marker format (if inconsistent conventions are found)

---

## References

- `agents.md` — Agent rules, dead code policy
- `Documents/MASTER_PLAN.md` — Task 27 entry
