---
name: freminal-orchestrator-protocol
description: Use ONLY when working in the freminal repository AND about to spawn sub-agents to decompose a task. Codifies the mandatory action-class scoping protocol for sub-agent prompts -- READ-ONLY, CODE-REVIEW, IMPLEMENTATION, COMMIT -- with explicit scope, deliverable, prohibitions, and stop condition in every spawned prompt. This is the single most important orchestration rule in the freminal repo; ignore it and sub-agents go off-script.
---

# Freminal: sub-agent orchestrator protocol

When acting as an orchestrator in the freminal repo -- decomposing a
task into sub-agent work -- this protocol is **mandatory and
non-negotiable**. It exists because sub-agents told to "understand the
code" repeatedly decided on their own to start writing code,
committing, and moving to the next task. That has happened many times
and is unacceptable.

## Every sub-agent prompt MUST contain all five of these

1. **Action class** -- one of the four below, stated at the top of
   the prompt in bold / caps.
2. **Exact scope** -- which files/modules the agent may read or
   modify. Be specific. Glob if you must but prefer file lists.
3. **Deliverable** -- what the agent must return.
4. **Explicit prohibitions** -- what the agent must NOT do.
   Always include at minimum one of:
   - "Do NOT edit any files." (for READ-ONLY)
   - "Do NOT commit." (for IMPLEMENTATION)
   - "Do NOT proceed to the next subtask." (always)
5. **Stop condition** -- when to stop. "Stop after reporting
   findings." or "Stop after the verification suite passes."

If any of these is missing, the orchestrator has failed. Do not
spawn the sub-agent.

## Action classes

| Action class       | MAY                                               | MUST NOT                                                          |
| ------------------ | ------------------------------------------------- | ----------------------------------------------------------------- |
| **READ-ONLY**      | Read files, search, analyze, report findings      | Edit files, write files, run cargo/git commands that mutate state |
| **CODE-REVIEW**    | Read files, analyze diffs, report issues          | Edit files, write files, commit, run tests                        |
| **IMPLEMENTATION** | Read + edit + write files within the stated scope | Touch files outside scope, commit, push, move to the next subtask |
| **COMMIT**         | Stage and commit specified changes                | Edit code, start new work                                         |

## Templates

### READ-ONLY exploration

```text
ACTION CLASS: READ-ONLY. Do NOT edit, write, or create any files. Do NOT run git commit.

Read the following files and report back:
- freminal/src/gui/mod.rs (lines 1600-1720)
- freminal/src/gui/terminal/input.rs (the write_input_to_terminal function)

Return: the full function signatures, how keyboard input flows from key
press to PTY, and how mouse events are currently routed.

Stop after reporting. Do NOT write code. Do NOT proceed to implementation.
```

### Scoped IMPLEMENTATION

```text
ACTION CLASS: IMPLEMENTATION. You may edit files listed below. Do NOT commit.
Do NOT proceed to the next subtask. Do NOT touch files outside this list.

Scope: freminal/src/gui/terminal/input.rs, freminal/src/gui/terminal/widget.rs

Task: Add an `is_active_pane: bool` parameter to `write_input_to_terminal`.
When false, suppress all events except primary left-click. Thread the
parameter through from `show()` in widget.rs.

Verification: run `cargo test --all` and
`cargo clippy --all-targets --all-features -- -D warnings`.

Stop condition: report back with files modified, summary of changes, and
verification results. Do NOT commit. Do NOT update plan documents. Do NOT
start the next subtask.
```

## Parallelism patterns

- **Crate-level**: different sub-agents work on different crates
  simultaneously. Safe when changes don't cross crate boundaries.
- **Task-type**: one sub-agent implements, another writes tests,
  another reviews. The implementation agent completes before the
  test agent starts if tests depend on new APIs.
- **Feature-level**: independent features in different files. Safe
  when there's no overlap.

If a sub-agent discovers it needs to modify files outside its
assigned scope, it must stop and report back. The orchestrator
resolves cross-scope dependencies by reassigning or sequencing
work -- not by expanding the sub-agent's scope mid-task.

## Pre-existing bugs surfaced during a subtask

If a sub-agent finds a bug outside the current subtask's scope:

1. The sub-agent MUST stop and report. It MUST NOT fix the bug as
   part of the current subtask, even if the fix is small.
2. The orchestrator files it as a numbered cleanup entry in the
   host task's plan section, following the convention used in
   `Documents/PLAN_VERSION_090.md` Task 72.16. The cleanup subtask
   is part of the task -- not a separate task, not a TODO comment
   in code, not a tracking issue elsewhere.
3. The cleanup subtask must include: surface point (commit +
   subtask), bug impact, scope of fix, suggested approach,
   verification criteria, and scheduling constraints.
4. The original subtask's completion notes link to the cleanup
   entry by number -- they do NOT carry the full bug description.
5. Informal "known issues" sections in plan documents are NOT used.
   Every surfaced bug is either resolved or has a numbered cleanup
   entry.

## When to stop and ask (orchestrator level)

- A sub-agent's report contradicts what you expected. Do NOT spawn
  the next one; surface the discrepancy to the user first.
- A sub-agent reports "I couldn't do this without expanding scope".
  Resolve it -- either re-scope or sequence -- before continuing.
- The plan calls for >5 parallel sub-agents on the same file area.
  Stop and rethink decomposition; that's usually a sign of
  over-decomposition.
