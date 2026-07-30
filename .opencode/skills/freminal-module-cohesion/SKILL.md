---
name: freminal-module-cohesion
description: Use ONLY when working in the freminal repository AND you are about to add a new `struct`, `enum`, or `trait` to an existing file, add a second unrelated `#[cfg(test)] mod tests` block to a file, or you are deciding which file a new type belongs in. Triggers on "where should this type go", "which file", "should I make a new module", on adding a type to a file whose name describes a different concept, and on adding anything to a file long enough that you had to search for its end. Codifies one CONCEPT per module (not one type per file), the path-should-name-the-concept check, and the rule that a split which forces `pub(super)` to widen to `pub(crate)` must be declined.
---

# Freminal: one concept per module

The GUI crate accumulates types in whichever file was already open.
Measured across the workspace:

| Crate                        | Types per file |
| ---------------------------- | -------------- |
| `freminal` (GUI binary)      | **4.39**       |
| `freminal-terminal-emulator` | 3.29           |
| `freminal-common`            | 2.57           |

The GUI is the worst, but **cardinality is the wrong metric**, and
reading the table as "get `freminal` down to 2.57" would make the
codebase worse. Two data points show why:

- `freminal-common/src/config.rs` — **35 types, 4,650 lines**, the
  largest in the repo — is **fine**. All 35 mirror one TOML schema.
  One concept, correctly co-located.
- `freminal/src/gui/terminal/widget.rs` — **9 types, 5,137 lines, and
  12 separate `#[cfg(test)] mod` blocks** — is **not fine**. Those 12
  unrelated test modules are the loudest signal in the tree that a file
  has stopped being about one thing.

A count-based rule flags the first and passes the second. So this skill
is trigger-based, like `freminal-extend-or-extract`, not threshold-based.

**The metric is cohesion: does everything in this file serve one
concept?**

## The exemplar already in the repo

`freminal-common/src/buffer_states/modes/` — 37 files, one named enum
each, the path naming the concept (`modes/decawm.rs`, `modes/dectcem.rs`,
`modes/mouse.rs`). That is the target shape. It is also the exemplar for
`freminal-state-representation`.

## The trigger

Stop and pick a home when any of these is true:

- **Your type's name has no relationship to any segment of the file
  path.** This is the mechanical check, and it is the most useful one:

  | Type                    | Path                       | Verdict |
  | ----------------------- | -------------------------- | ------- |
  | `ViewState`             | `gui/view_state.rs`        | correct |
  | `Decawm`                | `buffer_states/modes/decawm.rs` | correct |
  | `SearchState`           | `gui/view_state.rs`        | **no relationship** |
  | `PaneRenderCache`       | `gui/terminal/widget.rs`   | **no relationship** |

- **You are adding a second unrelated `#[cfg(test)] mod` block** to a
  file. One file, one concept, one test module (plus focused submodules
  of it). Twelve means twelve concepts.
- **You had to search to find the end of the file** you are adding to.
- **You are adding a type whose only connection to the file is that the
  values it needs happen to be in scope there.** That is the same
  smell `freminal-extend-or-extract` names, applied to types.

## Rules

1. **One concept per module, not one type per file.** A type plus its
   tightly-coupled satellites belongs together. `PendingPaste` living
   beside `ViewState` because only `ViewState` holds one is **cohesion,
   not a violation**. Do not split a type away from its only user.
2. **The path should name the concept.** If a reader cannot guess the
   file from the type name, or the type from the file name, the type is
   in the wrong place.
3. **Prefer `foo/mod.rs` + `foo/bar.rs` over one growing `foo.rs`.**
   Modules are free. When a file acquires a second concept, that is the
   moment to make it a directory.
4. **Decline a split that forces visibility to widen.** If moving a type
   to its own file means `pub(super)` fields must become `pub(crate)`,
   or private fields must gain accessors purely to satisfy layout,
   **the split is wrong**. Say so and leave it. File organisation is a
   readability goal; encapsulation is a correctness one, and
   correctness wins. Note this in a comment if it is non-obvious, so
   the next agent does not retry it.
5. **Serialisation-schema aggregates are legitimately large.**
   `config.rs`'s 35 structs mirror `config.toml`; splitting them across
   35 files would scatter one schema. The same applies to layout and
   recording formats. **Size alone is never the trigger.**

## What this does not license

- **Not a mandate to split by line count.** A 2,000-line module with one
  responsibility is fine. See rule 5.
- **Not a file per tiny helper.** A three-line newtype used once does not
  need its own file; that makes navigation worse, not better.
- **Not a licence to move types between crates.** Crate placement is
  `freminal-architecture`'s business and the dependency graph still
  governs.
- **Not retroactive remediation.** By maintainer decision, existing
  grab-bag files are **left alone**. This skill governs where *new*
  types go. Do not open a refactor of `widget.rs`, `view_state.rs` or
  anything else off the back of it, and do not file a task to do so.

## Boundary with the neighbouring skills

Three different questions, asked in this order:

| Question                              | Skill                           |
| ------------------------------------- | ------------------------------- |
| Does this logic belong here at all?   | `freminal-extend-or-extract`    |
| Which file, given that it does?       | **this skill**                  |
| What type should the state be?        | `freminal-state-representation` |

Crate placement and dependency direction remain `freminal-architecture`.

## When to stop and ask

- The right home is a new directory that reorganises an existing module
  tree. Stop — that is a structural change, not a placement decision.
- The type is genuinely shared by two concepts and has no natural owner.
  Stop; that usually means the concept boundary is wrong, which is an
  `freminal-extend-or-extract` question.
- Placing the type correctly would require making it `pub` across a
  crate boundary. Stop — see rule 4 and `freminal-architecture`.
- You believe an existing file needs splitting to do your task properly.
  Stop and report it. Per `freminal-orchestrator-protocol` that is a
  numbered cleanup entry for the orchestrator to schedule, not
  something to fold into the current change.
