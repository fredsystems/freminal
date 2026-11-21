---
name: freminal-source-comments
description: Use ONLY when working in the freminal repository AND writing or editing any comment, doc comment, test name, log message or error string in a source or build file (.rs, Cargo.toml, .nix, shell, Python, xtask, CI workflow YAML, config_example.toml, res/freminal.ti), or reviewing a diff that does. Codifies that source never references plan documents, task / subtask / cleanup-entry numbers (Task 124, 124.3b, 147.C1), freminal's own GitHub issues or PRs (#432) or advisories (GHSA-...); the reason is stated inline, or a spec or durable reference document is cited instead. The only exception is a temporary TODO(<task>) marking code that intentionally deviates while that task is in progress.
---

# Freminal: source comments explain, they do not cite the plan

A comment that says "see Task 124.14a" or "regression for #432" tells the reader nothing and
sends them into git archaeology. Task numbers and issue numbers rot: plans are retired, issues
are closed, and the number stops resolving to anything a reader can open. The reason something
is the way it is belongs in the comment itself. Task numbers belong in commit messages and plan
documents, where they are history.

## The rule

Nowhere in source or build files -- comments, doc comments, test function and module names,
log and error message strings, comments in `Cargo.toml`, `.nix`, shell, Python, xtask, CI
workflow YAML, `config_example.toml`, `res/freminal.ti` -- may you reference:

- a plan document (`PLAN_VERSION_*.md`, `PLAN_NNN_*.md`, `MASTER_PLAN.md`, `FUTURE_PLANS.md`);
- a task, subtask or cleanup-entry number (`Task 125`, `124.3b`, `147.C1`, `(Task 151.12)`);
- a freminal GitHub issue or pull request (`#432`, `PR #503`);
- a freminal security advisory id (`GHSA-...`).

Instead:

- **State the reason inline**, in a sentence: what would break, which invariant holds, which
  alternative was rejected and why.
- **Cite the spec or reference implementation** when behaviour follows one: xterm's
  `ctlseqs.txt` or a `charproc.c` function name, the kitty protocol page, an RFC section, the
  VT510 manual. These are stable and openable.
- **Point at a durable reference document** for long rationale: `DESIGN_DECISIONS.md` (quote
  the section title without its "(Task NN)" suffix), `PROFILING.md`, `FREC_FORMAT.md`,
  `LAYOUT_FORMAT.md`, `KITTY_PROTOCOL_REFERENCE.md`, `ESCAPE_SEQUENCE_COVERAGE.md`.
- **Name tests by behaviour**: `grow_reclaims_trailing_blank_rows`, not `task_113_smoke`; a
  regression test's doc says what the bug was in behaviour terms, not which task fixed it.
- **Log and error strings** say what happened, never which task added them.

## Allowed

- **Upstream third-party trackers** that explain a workaround and tell the reader when it can
  go (`winit#1426`, `egui#3653`, `kitty #9984`). Keep the repository name in the reference.
- **CVE and RUSTSEC ids** naming a vulnerability class or a dependency advisory.
- **Spec section numbers** (`ECMA-48 8.3.117`, `RFC 3986 3.1`).
- **The work-in-progress exception:** a `TODO(<task>)` on code that intentionally does
  something different while that task is in progress -- a temporary
  `#[cfg_attr(not(test), expect(dead_code, reason = "... TODO(150.10)"))]`, a stub arm, a
  deliberately incomplete path. It must say what is temporary, and it must be gone before that
  task's PR merges. A grep for `TODO(` with a task number is part of the task's final review.
- **A plain `TODO`** for unscheduled future work, describing the work itself (no plan pointer).

## Checklist for a change

- [ ] `rg -nP '\bTask\s+\d+|\b\d{2,3}\.(C\d+|\d+[a-z]?)\b|PLAN_|(?<![\w/])#\d{3,}\b|GHSA-'`
      over the files you touched finds only allowed references (version numbers and similar
      false positives aside).
- [ ] Every comment you removed a citation from still says _why_; if the reason only lived in
      the cited plan, write it into the comment (or into `DESIGN_DECISIONS.md` and point there).
- [ ] Renamed tests are not referenced by name elsewhere (docs, CI filters, skills).
- [ ] Changed log or error strings are not asserted verbatim by a test you did not update.
- [ ] No `TODO(<task>)` remains for a task that is merging.

## When to stop and ask

- The only explanation for a piece of code is a plan entry you cannot find or understand; ask
  rather than guess the reason.
- A log or error string is part of a user-facing or machine-read contract (a documented message,
  a test harness that greps logs).
