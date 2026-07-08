---
name: freminal-plan-status-lifecycle
description: Use ONLY when working in the freminal repository AND changing task or version status in Documents/MASTER_PLAN.md — most importantly when a task's PR merges, when starting a task branch, or any time you edit MASTER_PLAN.md. Codifies the one-directional status lifecycle (Stub/Planned -> In progress -> Pending merge -> Complete), the mandatory merge-time transition that gets forgotten, the two-tables-must-agree invariant (Task Summary Status column vs. Completion Tracking dates), and the main-lag caveat for long-running integration branches like v0.11.0-kitty.
---

# Freminal: MASTER_PLAN status lifecycle

`Documents/MASTER_PLAN.md` is the roadmap of record. It tracks status
in **two** hand-maintained places that must always agree:

1. The **Version Roadmap** table and the **Task Summary** table near
   the top — the `Status` column.
2. The **Completion Tracking** table near the bottom — the `Started` /
   `Completed` / `Notes` columns.

These have repeatedly drifted apart: a task whose Completion Tracking
row shows a completion date while its Task Summary row still reads
`Pending merge` or `Planned`. The drift is a bookkeeping failure, not a
code problem, but it makes the plan lie about what is done. This skill
exists to prevent it.

## Why the drift happens

Three compounding causes, all observed in this repo:

1. **`main` lags the work.** A long-running integration branch (e.g.
   `v0.11.0-kitty`) can be dozens of commits ahead of `main`, so "is it
   merged?" has two answers depending on which branch you mean.
2. **Status is maintained in two tables** and only one gets updated.
3. **The workflow updates the plan at task-completion time on the
   branch, but has no step that advances status when the PR merges** —
   so statuses freeze at whatever they were when the task branch closed.

## The status values and their transitions

Status moves in one direction only:

`Stub` / `Planned` -> `In progress` -> `Pending merge` -> `Complete`

- **`Stub` / `Planned`** — not yet activated (no subtask breakdown), or
  activated but not started. `Stub` for far-term versions,
  `Planned` for activated-but-unstarted tasks.
- **`In progress`** — a task branch exists and work is underway. Set
  this when the branch is created.
- **`Pending merge`** — all subtasks done, `cargo test --all` green on
  the branch, PR open but not yet merged.
- **`Complete`** — **the defining event is the merge**, not the last
  subtask commit. Advance to `Complete` the moment the PR merges into
  its target branch (whether that is `main` or a long-running
  integration branch).

## The merge-time rule (the one that gets forgotten)

**When a task's PR merges, in the SAME change that records the merge you
MUST:**

1. Flip that task's `Status` in the Task Summary table from
   `Pending merge` to `Complete`.
2. Flip the parent version's `Status` in the Version Roadmap table to
   `Complete` **iff every task in that version is now `Complete`**.
3. Fill in / confirm the task's `Completed` date and `Notes` in the
   Completion Tracking table.

Do not leave a task at `Pending merge` after its PR has merged. Do not
mark a task `Complete` in one table while the other still says
`Planned` / `Pending merge`.

## The `main`-lag caveat

`Complete` means **merged into its target branch**, which may be an
integration branch (e.g. `v0.11.0-kitty`) rather than `main`. That is
still `Complete`. Do NOT invent a separate status string for "done on
the integration branch but not yet on `main`". If that distinction ever
matters, record it in the Completion Tracking `Notes` column, not as a
new status value.

## Consistency check (run it on every MASTER_PLAN edit)

Before finishing any edit to `MASTER_PLAN.md`, verify all three:

1. Every task with a `Completed` date in the Completion Tracking table
   reads `Complete` in the Task Summary table, and vice versa.
2. No `Pending merge` string survives for a task whose PR has merged.
   When unsure which branch a task landed on, cross-check with git:

   ```bash
   git branch --merged <target-branch>        # did the task branch merge?
   git log --oneline <target-branch>..<branch> # how far ahead is the branch?
   ```

3. A version row is `Complete` only when all its member tasks are
   `Complete`.

## Hard rules

- Status is one-directional. A task never moves backward
  (`Complete` -> `In progress`) without an explicit maintainer decision
  recorded in the Notes.
- The two tables must always agree. If you touch one, reconcile the
  other in the same change.
- The merge is the `Complete` trigger — not the last commit on the
  branch, not the PR being opened.
- No informal "done-ish" states. A task is exactly one of the four
  status values.

## When to stop and ask

- A task's PR merged into an unexpected branch, or the branch history is
  ambiguous about whether it merged at all. Confirm with the maintainer
  before flipping status.
- You find pre-existing drift (a `Pending merge` task that clearly
  merged long ago). Reconcile it — but if reconciling reveals a task
  that was reverted or partially landed, stop and ask rather than
  guessing its true status.
- A version's tasks are all `Complete` but the version was never
  formally "released" / tagged. Marking the version-row `Complete` is
  correct (it reflects task completion, not a release); flag the tagging
  question to the maintainer separately.

Base directory for this skill:
file:///home/fred/GitHub/freminal/.opencode/skills/freminal-plan-status-lifecycle
