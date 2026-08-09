---
name: freminal-version-activation
description: Use ONLY in the freminal repository when activating a version from MASTER_PLAN.md whose plan document is a stub (no per-subtask breakdown), or when fleshing out / decomposing any version's tasks into implementable subtasks. Names the freminal-specific reading list for activation recon, the decomposition heuristics peculiar to this codebase (types-before-behaviour-before-render, reverse-PTY-write paths, config-option wiring, escape-sequence dual-doc, benchmark capture), and which skill owns each downstream step. The generic just-in-time planning policy and subtask contract live in the shared plan-decomposition skill.
---

# Freminal: version activation specifics

The generic policy -- two-tier plans, just-in-time decomposition, the
orchestrator/implementer division of labour, the five-part subtask
contract and its template, and the "no unresolved design decision
reaches the implementer" rule -- lives in **`plan-decomposition`**.
Read that first. This skill only carries what is specific to freminal.

## Activation recon: what to read in this repo

Before decomposing anything, read the _current code_ the version will
touch. In freminal that means:

- `Documents/MASTER_PLAN.md` (the version's row and its dependencies)
  and the version's own stub document.
- The `freminal-architecture` skill, for the lock-free PTY/GUI split
  and the crate dependency direction the version must not violate.
- For escape-sequence work: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`,
  `Documents/ESCAPE_SEQUENCE_GAPS.md`, **and the authoritative external
  spec**. Do not scope escape-sequence work from memory of what a
  sequence does.
- For anything in the egui rendering stack:
  `Documents/EGUI_UPGRADE_ASSUMPTIONS.md`, because the chrome-caching
  work depends on undocumented egui 0.35.0 behaviour.

## Freminal decomposition heuristics

These are the seams this codebase actually splits along:

- **Types/state before behaviour before render.** A typical
  parser/handler/renderer feature splits cleanly: (a) add the typed
  state in `freminal-common`, (b) wire the parser/handler in
  `freminal-terminal-emulator`, (c) transport it via the snapshot,
  (d) render in `freminal`. Each is its own subtask, in that order.
  Because `freminal-common` sits at the bottom of the dependency
  graph, (a) is also the natural foundation-first subtask when the
  version will run as parallel workstreams -- see
  `parallel-work-isolation`.
- **Audit before implement.** When current behaviour is ambiguous -- a
  reused OSC number, a stubbed-but-typed handler, a "verify
  completeness" item -- the first subtask is a READ-ONLY audit whose
  findings feed the implementation subtasks. Do not fold the audit into
  the first implementation subtask.
- **Reverse-PTY-write features** (notification activation, transfer
  acks, query responses -- anything where the terminal writes back to
  the application) get an explicit subtask for the write path, scoped
  to the existing `write_to_pty` / `Pane::pty_write_tx` plumbing.
  A new channel needs maintainer sign-off.
- **Config options** follow the `freminal-config-options` wiring
  checklist as their own subtask, never bolted onto a feature subtask.
  The `ConfigPartial` / `apply_partial` omission is a known
  silent-failure class in this repo.
- **Escape-sequence changes** carry a final subtask for the mandatory
  dual-doc update (`freminal-escape-sequence-docs`).
- **Benchmarked hot paths** carry a before/after capture subtask
  (`performance-benchmarks` for the procedure,
  `freminal-bench-table` for which bench file covers what).

## Verification in every subtask

Every implementation subtask names these, and leaves them green:

```text
cargo test --all
cargo clippy --all-targets --all-features -- -D warnings
```

Plus `cargo xtask check-windows` before any PR touching
`#[cfg(windows)]`, `portable-pty`, paths, or threads
(`freminal-windows-crosscheck`).

## Status, once activated

Set the version to `In progress` and its tasks to `Planned` as part of
writing the breakdown. Every subsequent transition -- including the one
that gets forgotten, advancing to `Complete` when the PR merges -- is
governed by **`freminal-plan-status-lifecycle`** for this repo's
vocabulary and two-table invariant, and by
**`plan-sequencing-discipline`** for the generic rules (merge is the
completion trigger, the merge barrier, no forward dependencies).

## When to stop and ask

The generic stop conditions are in `plan-decomposition`. Freminal
additions:

- The external spec the version targets is unstable or under active
  revision. Do not decompose against a moving target; keep the version
  a stub and note the instability.
- The version touches the egui stack and
  `EGUI_UPGRADE_ASSUMPTIONS.md`'s assumptions no longer hold against
  the pinned version.
- Decomposition would require a new PTY write channel, a new crate, or
  a change to the snapshot transport. Those are architecture decisions
  (`freminal-architecture`, `freminal-extend-or-extract`), not
  decomposition ones.
