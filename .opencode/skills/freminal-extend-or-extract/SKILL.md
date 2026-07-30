---
name: freminal-extend-or-extract
description: Use ONLY when working in the freminal repository AND you are about to make something bigger rather than give it a home -- adding a field to a struct so that code outside its owner can read it later (across a frame boundary, a thread boundary, or the GUI/event-layer split), adding a branch or block to a function that is already very long, adding a parameter to a function that already has many, copying a computation because the real one is unreachable from where you need it, extracting a pure helper solely so a predicate can be unit-tested, or writing a test that re-implements production logic inline in order to pin it. Also triggers on "where should this live", "should this be a new crate", "should this be a new module", and on any `#[allow(clippy::too_many_lines)]` / `too_many_arguments` you are about to add or rely on. Gives explicit scope to propose a new module or crate instead of extending in place, and mandates surfacing the choice rather than silently taking either option.
---

# Freminal: extend in place, or give it a home?

This skill exists because the default answer has been "extend in place"
by omission -- no skill gave agents scope to propose anything else, so
they extended whatever function or struct was already in scope. The
result is measured, not hypothetical: `App::update` reached **3,132
lines**, `central_body` **2,033**, `write_input_to_terminal` grew a
**17th parameter** and returns a 7-tuple, and render-time state ended
up cached ad hoc across ~10 `PerWindowState` fields with no name, no
type and no invariant.

Full case study: `Documents/PLAN_122_ORCHESTRATION_EXTRACTION.md`.
Architecture invariants that constrain any answer:
`freminal-architecture`.

**Proposing a new module or crate is in scope. Extending in place
because you had no mandate to do otherwise is not.**

## The trigger, concretely

Stop and consider a new home when any of these is true:

- **You are adding a field to a struct purely so that something
  outside its owner can read it later.** Especially across a frame
  boundary, a thread boundary, or the GUI/event-layer split. That
  field has no owner. Say so.
- **You are adding a branch or block to a function that is already
  very long**, and your addition has no natural relationship to what
  the function is for -- it is simply the place that happened to have
  the values in scope.
- **You are adding a parameter to a function that already has many**,
  or relying on an existing `#[allow(clippy::too_many_arguments)]` /
  `#[allow(clippy::too_many_lines)]`. Those allows are a record of a
  previous decision to extend in place. Do not treat them as
  permission to do it again.
- **You are copying a computation** because the "real" one is
  unreachable from where you need it.
- **You are extracting a pure helper solely so that a predicate can be
  unit-tested.** The helper is welcome; the reason you needed it is
  the signal. The predicate cannot be constructed where it lives.
- **A test has to re-implement production logic inline** in order to
  pin it. That means the logic has no callable home. (Real example:
  `overlay_suppress_input_tests` in `terminal/widget.rs` re-derives
  `show`'s inline booleans as test-local closures, because `show`
  cannot be called.)

## What to propose

1. **Default to a module, not a crate.** Design it as if it were a
   crate -- explicit public surface, nothing reaching into its
   internals, documented invariants -- but land it as a module. Crate
   boundaries are the highest-friction refactor to undo.
2. **Crate extraction is a separate, final, mechanical step** and
   requires maintainer sign-off. Never create a crate mid-task.
3. **Name the invariant the new home enforces.** "These four fields are
   written during a frame and read outside one, are one frame stale by
   construction, and survive an early return unchanged" is a design.
   "A struct to hold some things" is not. If you cannot state the
   invariant, you have not found the boundary yet.
4. **Respect the dependency graph.** `freminal-architecture` governs.
   A new home that needs an upward dependency is still forbidden; a
   new crate may depend on `freminal-common` and nothing upward.
5. **Prefer the smallest home that fixes the ownership problem.** A
   module in the crate that already owns the data beats a new crate.
   A named struct beats a module. A better-named existing type beats
   a new one.

Two neighbouring skills answer the follow-on questions once you have
decided the logic does need a home here: **`freminal-module-cohesion`**
(which file — one concept per module) and
**`freminal-state-representation`** (what type the state should be — a
named domain enum, not a bare `bool`).

## Surface it -- do not silently take either option

This is the actual rule, and it cuts both ways:

- **Do not silently extend in place.** That is the failure this skill
  exists to stop.
- **Do not silently create the new home either.** Inventing a module
  or crate mid-subtask is its own scope violation.

Report the choice and let the orchestrator decide. Per
`freminal-orchestrator-protocol`, a sub-agent that hits one of the
triggers above **stops and reports**; the orchestrator resolves it by
re-scoping or sequencing, not by widening the sub-agent's scope. If
you are the orchestrator, this is your decision to make and to write
into the plan document as a numbered subtask.

## What this does not license

- **Not a mandate to restructure adjacent code you happen to dislike.**
  The proposal must be *caused* by the change you were asked to make.
- **Not a route around the architecture invariants.** See
  `freminal-architecture`. A new home does not make an upward
  dependency acceptable.
- **Not a crate per concept.** freminal has six crates and does not
  need sixteen.
- **Not justified by line count alone.** A 2,000-line module with one
  clear responsibility is fine. A 200-line function accumulating
  decisions that belong to four different layers is not. **Absence of
  an owner is the justification; size is only ever a symptom.**

## When to stop and ask

- You have found a trigger but the right home is genuinely unclear.
  Stop -- describe the ownership problem and let the maintainer choose.
- The new home would need a new crate. Stop -- that needs sign-off,
  always.
- The new home would require an upward dependency. Stop -- that is a
  hard invariant, not a trade-off.
- Fixing the ownership problem properly is much larger than the task
  you were given. Stop and say so, rather than doing a partial version
  that leaves two homes for one concept. A numbered cleanup entry in
  the host task's plan document is the correct output here (see
  `freminal-orchestrator-protocol`).
