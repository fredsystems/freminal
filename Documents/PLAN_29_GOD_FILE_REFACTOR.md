# PLAN_29 — God File Refactoring

## Status: Stub

---

## Overview

Several files in the workspace have grown far beyond a single responsibility. The two most
extreme cases are in `freminal-buffer`:

| File                  | Lines  | Approx. Subsystems |
| --------------------- | ------ | ------------------ |
| `terminal_handler.rs` | ~9,098 | ~10                |
| `buffer.rs`           | ~6,624 | ~8                 |

Together these two files account for ~71% of all code in the three library crates. Other files
across the workspace may also have grown beyond a comfortable single-responsibility scope.

The goal is to split every oversized file into focused, single-responsibility modules where each
file does one thing well. This improves:

- **Navigability** — finding code by topic instead of scrolling thousands of lines.
- **Merge safety** — smaller files reduce conflict surface area.
- **Testability** — focused modules are easier to test in isolation.
- **Onboarding** — new contributors can understand a 300-line module; a 9,000-line file is
  hostile.

**Dependencies:** All other tasks. This should be the last task executed.
**Dependents:** None
**Primary crates:** All, but primarily `freminal-buffer` and `freminal-terminal-emulator`
**Estimated scope:** High (unknown until audit completes)

---

## Why This Is a Stub

This task must be the **last** major work done on the codebase because:

1. **Merge conflict risk:** Splitting a 9,000-line file renames, moves, and restructures code
   that virtually every other task touches. Running this concurrently with feature work would
   produce constant merge conflicts.

2. **Scope depends on final state:** Other tasks (code quality refactoring, bool-to-enum,
   FIXME cleanup, etc.) will add, remove, and reorganize code. The file sizes, subsystem
   boundaries, and natural split points will be different after those tasks complete.

3. **The audit must happen at execution time:** A premature split plan would be outdated by the
   time it executes. The audit should examine the codebase in its final pre-split state.

## Why This Is Needed

The current god files conflate multiple subsystems behind a single module boundary:

**`terminal_handler.rs` (~9,098 lines)** likely contains:

- Mode dispatch (`process_outputs` — the giant match)
- Escape sequence handlers (OSC responses, DCS handling, window manipulation)
- Buffer mutation wrappers (insert, erase, scroll, cursor movement)
- Image protocol handlers (iTerm2, Kitty, Sixel)
- Tab stop management
- Character set / NRC handling
- DECRPM query responses
- Sixel palette management
- tmux passthrough bookkeeping

**`buffer.rs` (~6,624 lines)** likely contains:

- Core buffer data structure (rows, cursor, dimensions)
- Text insertion and wrapping
- Scroll operations (region scroll, full scroll)
- Erase operations (ED, EL, ECH)
- Resize / reflow
- Alternate screen management
- Margin management (DECSTBM, DECLRMM)
- Flatten / snapshot helpers (visible_as_tchars_and_tags, etc.)
- Row cache management

Other files across the workspace may also warrant splitting. The audit will identify all
candidates above a threshold (e.g., >800 lines or >3 distinct responsibilities).

## Audit Procedure

When this task is activated (after all other tasks are complete):

1. Measure every `.rs` file in the workspace by line count.
2. For files above the threshold (~800 lines), identify the distinct subsystems/responsibilities.
3. For each god file, propose a split plan:
   - Which subsystems become their own module.
   - What the new file names should be.
   - Which types/functions move where.
   - What the public API of each new module looks like.
   - How tests are distributed.
4. Write the subtask list in this document.
5. Update the status from "Stub" to "Pending".

## Guiding Principles

- **One responsibility per file.** A file should do one thing. If you need a compound name
  like `terminal_handler_image_protocols.rs`, it should probably just be `image_protocols.rs`
  inside a `terminal_handler/` directory.
- **Prefer directories over prefixed files.** If `terminal_handler.rs` splits into 8 modules,
  they should live in `terminal_handler/mod.rs` + `terminal_handler/modes.rs` +
  `terminal_handler/images.rs` + etc., not as 8 top-level files with `terminal_handler_` prefixes.
- **Internal (`pub(crate)`) over public.** Splitting a file into modules should not widen the
  public API. Use `pub(crate)` for inter-module access within the same crate.
- **Tests stay with the code they test.** Each new module gets its own `#[cfg(test)] mod tests`
  section containing the tests that were previously in the monolithic file.
- **No behavior changes.** This is purely structural. The refactor must not change any
  observable behavior. `cargo test --all` must pass at every intermediate step.

## Subtasks

To be created after the audit. Expected structure:

- One subtask per god file split (e.g., "Split `terminal_handler.rs` into N modules")
- Each subtask specifies the exact module boundaries and file layout
- Subtasks are ordered to minimize merge conflict between them
- Each subtask must leave `cargo test --all` passing

---

## References

- `agents.md` — Agent rules, crate-specific guidance
- `Documents/MASTER_PLAN.md` — Task 29 entry
- `Documents/PLAN_25_CODE_QUALITY.md` — Identified the god files as out of scope for Task 25
