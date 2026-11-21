---
name: freminal-retire-plan
description: Use ONLY when working in the freminal repository AND a plan document under Documents/ (PLAN_VERSION_*.md, a per-task PLAN_NNN_*.md, or a point-release PLAN_VERSION_*_N.md) may be ready to delete -- when a version's release tag ships, when a reader asks whether an old plan is still needed, when MASTER_PLAN.md points at a completed plan, or when auditing Documents/ for stale plans. Codifies when a plan is retired (all its tasks Complete AND the release that contains them is tagged), and the retirement procedure: extract the durable "why" into DESIGN_DECISIONS.md, move open items, repoint every inbound reference (including code comments), fix MASTER_PLAN.md, then delete.
---

# Freminal: retiring a completed plan document

Plan documents record intent and execution: subtask contracts, status notes, review logs,
benchmarks taken on one machine on one day. Once the work has shipped, almost all of that is
history, and git keeps history. What does not belong in history is the "why" a future engineer
needs and cannot recover from the code. A plan that outlives its work becomes a second, staler
source of truth that agents and humans keep reading. Retiring it on time is drift prevention.

Status transitions (Planned -> In progress -> Complete) are `freminal-plan-status-lifecycle`.
This skill starts where that one ends.

## When a plan is retired

All of these must hold:

1. **Every task in it is `Complete`** in both `MASTER_PLAN.md` tables (Task Summary and
   Completion Tracking). Cleanup entries (`NN.Cn`) are resolved or have moved elsewhere.
2. **The release that contains the work is tagged** (`git tag` shows the final `vX.Y.Z`, not
   only a beta). Until then a beta bug may still need the plan's context.
3. **Nothing not yet done lives only in it.** Enriched stubs for later point releases, open
   questions, deferred subtasks and routed findings are moved out first (step 2 below).

A multi-file plan retires piece by piece: a point-release file (`PLAN_VERSION_130_0.md`) can go
once its release is tagged, while the overview stays until the last point release ships. When
the overview of a version retires, the cross-milestone design decisions in it move too.

Retirement is the default once the three conditions hold. Keeping a completed plan needs a
stated reason (it is a live decision record, like `DECOUPLING_FRAMEWORK.md`, which is not a
plan and is not retired by this rule).

## The procedure

Commit 2253483f (v0.3.0-v0.7.0) and the v0.8.0-v0.12.0 retirement are the templates.

1. **Audit, read-only.** For each document: read it in full; list durable content not already
   in `DESIGN_DECISIONS.md`, a reference doc (`FREC_FORMAT.md`, `LAYOUT_FORMAT.md`,
   `PROFILING.md`, `KITTY_PROTOCOL_REFERENCE.md`, `EGUI_UPGRADE_ASSUMPTIONS.md`, the
   escape-sequence docs) or a skill; list open items; list discrepancies with `MASTER_PLAN.md`;
   list every inbound reference (`rg --hidden -g '!.git' -g '!target' -g '!.direnv' <name>`).
   Verify each candidate decision against current code with `rg`: a decision the code no
   longer reflects is dropped or corrected, never carried forward. Parallel READ-ONLY
   sub-agents work well here (`agent-orchestration-protocol`).
2. **Move open items.** Deferred features go to `FUTURE_PLANS.md` (next free `B.n`); work that
   belongs to a scheduled task goes into that task's stub; unfixed cleanup entries go to the
   task that owns the code. Nothing is dropped silently: obsolete items are dropped with the
   reason stated in the commit message.
3. **Write the durable "why" into `DESIGN_DECISIONS.md`.** One `## Topic (Task NN)` section per
   concept, `###` subsections, terse. Keep: decisions with their reasons, rejected
   alternatives, invariants, and the measured numbers that justify a design. Drop: subtask
   mechanics, status notes, review logs, commit hashes, line numbers, anything the code says.
   Correct any existing section the audit found stale. Format specs go to their own reference
   document, not `DESIGN_DECISIONS.md`.
4. **Repoint every inbound reference**, including code comments, tests, benches, `flake.nix`,
   CI scripts, skills, `agents.md` and other docs. A comment citing the plan for its reasoning
   points at the new `DESIGN_DECISIONS.md` section by title; a comment citing a subtask only
   for provenance keeps the number and drops the filename (`Task 72.9`). Bare task numbers
   with no filename stay. Fix comments that state things the audit found no longer true.
   These are comment-only edits; they ride in the same change.
5. **Fix `MASTER_PLAN.md`.** Version rows: `(retired — see DESIGN_DECISIONS.md)`. Task rows:
   `See DESIGN_DECISIONS.md` (or the reference doc that now holds the content). Correct any
   status or Completion Tracking discrepancy the audit found (merge PRs from
   `git log --merges`). Reword prose that cites the plan. Remove its References entry.
6. **Delete the document** with `git rm`, in the same change.
7. **Verify:** the `rg` for the deleted filenames finds only history inside other plans;
   `cargo xtask lint-markdown`; `cargo xtask check-docs`; `cargo fmt --all -- --check`;
   `cargo clippy --all-targets --all-features -- -D warnings` and `cargo xtask test` if any
   source comment changed.

## Hard rules

- Never delete a plan whose open items have not moved.
- Never copy a plan's text into `DESIGN_DECISIONS.md` wholesale. If it is not a decision, a
  reason or a number someone needs, it stays in git history.
- Never leave a filename reference to a deleted document anywhere outside historical prose in
  other plans.
- Do not create summary documents of the retirement (`no-summary-documents`); the commit
  message is the record.

## When to stop and ask

- A plan meets the conditions but the maintainer has said to keep it for now.
- A decision in the plan contradicts the code and it is unclear which is right.
- An open item has no obvious home (no task, no FUTURE_PLANS category).
- A new file produced by the move exceeds the pre-commit size limit; split it, do not bypass.
