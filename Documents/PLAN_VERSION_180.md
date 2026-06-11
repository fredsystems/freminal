# PLAN_VERSION_180.md — v0.18.0 "AI Assist — Generative"

> **STATUS: ENRICHED STUB.** Durable design decisions are captured below;
> per-subtask decomposition happens at activation in a dedicated session,
> against the code as it then exists (see the `freminal-version-activation`
> skill). Do not invent subtasks early.

## Goal

The second, generative half of AI assist: opt-in features that produce **a command or
script that lands somewhere editable**, which the user reviews and triggers. Builds on the
shared AI foundation (endpoint client, consent, key storage) established in v0.17.0
(Task 87a).

Depends on Task 87a (v0.17.0 Advisory — the endpoint/consent/key foundation must be solid
first). **Does NOT depend on the event hook API** (Task 84).

---

## Task Summary

| #   | Feature                         | Scope  | Status | Depends On |
| --- | ------------------------------- | ------ | ------ | ---------- |
| 87b | AI Assist — Generative (opt-in) | Medium | Stub   | Task 87a   |

---

## Task 87b — AI Assist — Generative

Generation features, all producing runnable text into a reviewable surface:

- **Command generation (NL→command)** — the highest daily-driver value ("generate a
  command to push this repo to GitHub"). Output lands at the prompt for review; the user
  presses enter. The "I forget the git/rsync syntax" case.
- **Script generation** — "parse this dir and sort by X"; output lands in a scratch buffer
  / `$EDITOR`, never executed.
- **Regex / jq / awk / sed builder** — "jq to extract `.name` from this array", with the
  actual JSON from scrollback as context. This is where terminal-context AI beats a
  generic chatbot: it sees the real data.
- **Commit message generation** from the staged diff — narrow, beloved, low-risk.
- **Pipeline explanation (reverse)** — explain a gnarly pasted one-liner before running it
  (sits between advisory and generative; include here or in v0.17.0 at activation).

### Open questions (decide at activation)

- Which generation features ship first (command-gen is the anchor).
- Where generated commands land: prompt line vs a reviewable inline widget. Whichever, it
  is **populate-then-user-triggers**, never auto-run (invariant 1).
- Whether streaming responses (typewriter effect) are added here vs kept blocking. (Lean:
  consider streaming now that the foundation is proven; weigh SSE-parsing + cancel-mid-
  stream complexity.)
- Whether `keychain` key storage (if deferred from v0.17.0) is completed here.
- Prompt templates / customization surface (without inventing the event hook API).

---

## AI Invariants

**All invariants from `PLAN_VERSION_170.md` apply unchanged.** The ones that bite hardest
for the generative half:

- **Never auto-execute** (invariant 1). Generated commands/scripts populate the prompt or
  a buffer; the human presses the key. This single rule is the entire trust model for
  generation.
- **No agentic interaction** (invariant 2). No loop of generate→run→observe→generate.
- **Local detection, gestured send** (invariant 5); **first-per-endpoint three-button
  consent** with payload preview (invariant 6); **adults decide** the endpoint
  (invariant 7).

See `PLAN_VERSION_170.md` for the full, authoritative invariant list and the shared
foundation (one OpenAI-compatible client, default-off/default-local, `keyring` 4.x layered
key storage with the persistence-first Linux ladder, no redaction).

## Design Decisions (provisional, invariants firm)

- **Generation rides on the proven Advisory foundation.** The endpoint client, consent
  flow, and key storage are not re-litigated here — they were built and hardened in
  v0.17.0. This version adds the generation features and (optionally) streaming.
- **The reviewable-surface rule is the safety model.** Everything generative produces
  output into a place the user edits and then triggers. Anything that would auto-run is out
  of scope.
- **Out of scope (carried from v0.17.0):** ghost-text autocomplete, suggest-next-command,
  agentic execution.
- Any generation-review / consent overlay is focusable and MUST follow
  `freminal-modal-input-suppression`.
