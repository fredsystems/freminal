---
name: freminal-module-cohesion
description: Use ONLY in the freminal repository when deciding which file a new type, struct, enum, or trait belongs in. Carries the freminal-specific path/type mechanical check table, the buffer_states/modes/ exemplar, and the boundary between this decision and the neighbouring freminal skills (extend-or-extract, state-representation, architecture). The generic one-concept-per-module rules live in the shared module-cohesion skill.
---

# Freminal: which file does this type go in

The generic rules -- one **concept** per module (not one type per
file), the path names the concept, length is a symptom not the rule,
never widen visibility to enable a split, no `utils`/`helpers` module --
live in the shared **`module-cohesion`** skill. Read that first. This
skill carries only the freminal-specific parts.

## The exemplar already in the repo

`freminal-common/src/buffer_states/modes/` -- 37 files, one named enum
each, the path naming the concept (`modes/decawm.rs`,
`modes/dectcem.rs`, `modes/mouse.rs`). That is the target shape. It is
also the exemplar for `freminal-state-representation`.

## The mechanical check, with real verdicts

Does your type's name relate to any segment of the file path?

| Type              | Path                              | Verdict             |
| ----------------- | --------------------------------- | ------------------- |
| `ViewState`       | `gui/view_state.rs`               | correct             |
| `Decawm`          | `buffer_states/modes/decawm.rs`   | correct             |
| `SearchState`     | `gui/view_state.rs`               | **no relationship** |
| `PaneRenderCache` | `gui/terminal/widget.rs`          | **no relationship** |

The two failures are the pattern to watch for: a state type parked in
whatever file the caller lived in, and a cache parked in the widget
that happened to need it.

## Boundary with the neighbouring skills

Four different questions, asked in this order:

| Question                             | Skill                           |
| ------------------------------------ | ------------------------------- |
| Does this logic belong here at all?  | `freminal-extend-or-extract`    |
| Which file, given that it does?      | **this skill** + `module-cohesion` |
| What type should the state be?       | `freminal-state-representation` |
| Which crate, and which direction?    | `freminal-architecture`         |

## When to stop and ask

- The right home is a new directory that reorganises an existing module
  tree. Stop -- that is a structural change, not a placement decision.
- The type is genuinely shared by two concepts and has no natural
  owner. That usually means the concept boundary is wrong, which is a
  `freminal-extend-or-extract` question.
- Placing the type correctly would require making it `pub` across a
  crate boundary. Stop -- see `freminal-architecture`.
- You believe an existing file needs splitting to do your task
  properly. Stop and report it. Per `agent-orchestration-protocol` that
  is a numbered cleanup entry for the orchestrator to schedule, not
  something to fold into the current change.
