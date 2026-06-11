---
name: freminal-version-activation
description: Use ONLY in the freminal repository when activating a version from MASTER_PLAN.md whose plan document is a stub (no per-subtask breakdown), or when fleshing out / decomposing any version's tasks into implementable subtasks. Codifies the just-in-time planning policy (don't decompose far-future versions; flesh a version out at activation against the real code), the Sonnet-sized subtask shape (scope, deliverable, verification, prohibitions, stop condition), and the Opus-orchestrates / Sonnet-implements / Opus-reviews division of labour. Pairs with freminal-orchestrator-protocol (planning-time companion to that execution-time skill).
---

# Freminal: version activation & subtask scoping

This skill governs **planning time** in freminal: turning a stub version
plan into implementable, tightly-scoped subtasks. It is the companion to
`freminal-orchestrator-protocol`, which governs **execution time** (how
sub-agent prompts are written once subtasks exist). Read both when
activating a version.

## The core policy: flesh out just in time, against real code

Freminal's roadmap (`Documents/MASTER_PLAN.md`) spans many versions.
**Plan documents are deliberately written in two tiers:**

- **Near-term, imminently-activated versions** carry a full
  per-subtask breakdown, written against the _current_ codebase.
- **Far-term versions** are **enriched stubs**: goal, task summary,
  every durable design decision already made, and open questions
  deferred to activation -- but **no subtask decomposition**.

The reason is economic and correctness-driven, not laziness:

1. A subtask breakdown written N versions early is a guess about a
   codebase that will have changed underneath it. It rots.
2. Decomposition is the expensive (Opus) orchestration work. Doing it
   early means paying for it, watching it go stale, and paying again to
   re-validate at activation. Do it once, late, correct.
3. An extension/feature API (and any plan) should crystallise a
   _stable, shipped_ feature set -- not lead it.

**Durable decisions are recorded; perishable breakdowns are deferred.**
When you make a real design decision during discussion (an invariant, a
dependency cut, a scope change, a chosen crate), write it into the
relevant stub now. Do NOT invent Sonnet-level subtask lists for a
version that is not next.

### What "enriched stub" means concretely

A far-term plan document contains:

- A one-paragraph goal.
- A task-summary table (feature, scope estimate, status, deps).
- Every durable design decision captured as prose or a decisions
  section (invariants, chosen approach, rejected alternatives + why).
- Open questions explicitly tagged "decide at activation".
- A pointer to this skill for how it gets decomposed.

It does NOT contain: numbered subtasks, file-level scoping, per-subtask
verification steps, or commit-mapping. Those are produced at activation.

## Activation: one dedicated session per version

When a version is activated (moved from stub to active work):

1. **Read first.** `MASTER_PLAN.md`, the version's stub, every
   dependency's plan doc, and -- critically -- the _current code_ the
   version will touch. Use `freminal-architecture` and the relevant
   project skills to map the seams. For escape-sequence work, read
   `ESCAPE_SEQUENCE_COVERAGE.md` / `ESCAPE_SEQUENCE_GAPS.md` and the
   authoritative external spec before scoping anything.
2. **Resolve open questions** with the maintainer. The stub's "decide
   at activation" list is the agenda. Do not silently pick answers.
3. **Decompose into Sonnet-sized subtasks** (shape below), written
   against the real seams found in step 1.
4. **Write the breakdown into the version's plan doc**, replacing the
   stub body. Update the `MASTER_PLAN.md` status row.
5. **Then, and only then, begin execution** under
   `freminal-orchestrator-protocol` and the multi-step task protocol in
   `agents.md`.

Activation planning is itself Opus work. It is orchestration, not
implementation.

## The Sonnet-sized subtask shape

The division of labour:

- **Opus orchestrates**: reads code, decomposes, writes subtasks,
  sequences them, reviews sub-agent output, makes architectural calls.
- **Sonnet implements**: executes one tightly-scoped subtask per
  invocation with no architectural latitude.
- **Opus reviews**: every implementation subtask gets a CODE-REVIEW
  pass before it is accepted.

A subtask is correctly scoped for Sonnet when **all** of these hold:

1. **Single concern.** One logical change. If the description needs
   "and also", split it.
2. **Explicit file scope.** The exact files Sonnet may touch are
   named. No "and related files".
3. **No architectural decisions left open.** Every type name, enum
   variant, function signature, and design choice is already decided by
   Opus and written into the subtask. Sonnet fills in the body, not the
   shape.
4. **Self-contained verification.** The subtask names the exact
   commands that prove it correct (`cargo test --all`,
   `cargo clippy --all-targets --all-features -- -D warnings`, a
   specific new test module). Each subtask leaves `cargo test --all`
   green -- the commit-discipline invariant.
5. **Bounded.** Roughly one focused implementation pass. If it spans
   many files across crate boundaries or needs judgement calls
   mid-stream, it is an Opus task or needs splitting.

Each written subtask records: number, title, scope (file list),
deliverable, verification commands, explicit prohibitions, and stop
condition -- the same five-part contract
`freminal-orchestrator-protocol` requires in the spawned prompt. The
plan-doc subtask and the sub-agent prompt are two views of the same
contract.

### Subtask entry template (in the plan doc)

```text
#### NN.M -- <single-concern title>

Scope: <exact file list>

What: <the one change, with concrete type/fn/enum names Opus has
already chosen -- not "design a way to ...">

Deliverable: <the code + the tests that prove it>

Verification: cargo test --all; cargo clippy --all-targets
--all-features -- -D warnings; <any subtask-specific test>

Prohibitions: do NOT touch files outside scope; do NOT decide
<the thing already decided above>; do NOT proceed to NN.(M+1).

Stop: report files changed + verification results; await review.
```

## Decomposition heuristics

- **Audit before implement.** When current behaviour is ambiguous (a
  reused OSC number, a stubbed-but-typed handler, a "verify
  completeness" item), the FIRST subtask is a READ-ONLY audit that
  resolves the ambiguity and feeds the implementation subtasks. Do not
  fold the audit into the first implementation subtask.
- **Types/state before behaviour before render.** A typical
  parser/handler/renderer feature splits cleanly: (a) add the typed
  state in `freminal-common`, (b) wire the
  parser/handler in `freminal-terminal-emulator`, (c) transport via the
  snapshot, (d) render in `freminal`. Each is its own subtask; (a)
  precedes (b) precedes (c) precedes (d).
- **Reverse-PTY-write features** (anything where the terminal writes
  back to the application -- notification activation, transfer acks,
  query responses) get an explicit subtask for the write path, scoped
  to the existing `write_to_pty` / `Pane::pty_write_tx` plumbing, never
  a new channel without Opus sign-off.
- **Config options** that a feature introduces follow the
  `freminal-config-options` wiring checklist as their own subtask --
  never bolted onto a feature subtask, because the `ConfigPartial` /
  `apply_partial` omission is a known silent-failure class.
- **Escape-sequence changes** carry a final subtask for the mandatory
  dual-doc update (`freminal-escape-sequence-docs`).
- **Benchmarks**: if the version touches a benchmarked hot path, a
  before/after capture subtask is mandatory
  (`performance-benchmarks` + `freminal-bench-table`).

## Hard rules

- Do NOT decompose a version that is not the one being activated.
  Capturing a durable decision in a far-term stub is fine; writing its
  subtasks is not.
- Do NOT begin implementation in the same breath as decomposition.
  Decompose, get maintainer sign-off on the breakdown, then execute.
- Do NOT let a subtask carry an unresolved design decision into Sonnet.
  If Sonnet would have to choose a type, a name, or an approach, the
  decomposition is incomplete -- that choice is Opus's.
- Do NOT write subtasks against remembered code. Re-read the seams at
  activation; the codebase moved since the stub was written.

## When to stop and ask

- An open question in the stub has no obvious answer and the maintainer
  has not weighed in. Stop; that is the activation conversation.
- The external spec a version targets is unstable / under active
  revision. Do NOT decompose against a moving target; surface it and
  keep the version a stub with the instability noted (this is exactly
  why a version can be deferred).
- Decomposition reveals the version is far larger than its stub
  estimate. Stop and re-scope with the maintainer before writing twenty
  subtasks.
- You are tempted to flesh out the _next_ version too "while you're
  here". Don't. One version per activation session.

Base directory for this skill:
file:///home/fred/GitHub/freminal/.opencode/skills/freminal-version-activation
