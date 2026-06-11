# PLAN_VERSION_180.md — v0.18.0 "AI Assist — Advisory"

> **STATUS: ENRICHED STUB.** Durable design decisions are captured below;
> per-subtask decomposition happens at activation in a dedicated session,
> against the code as it then exists (see the `freminal-version-activation`
> skill). Do not invent subtasks early.

## Goal

The first, read-only half of AI assist: opt-in features that produce **text the user
reads** — output analysis & remediation, error explanation, and summarization — riding on
the OSC 133 command blocks already shipped (Task 72). This version also builds the shared
AI foundation (endpoint client, consent, key storage) that the generative half
(v0.19.0, Task 87b) reuses.

Depends on Task 72 (OSC 133 command blocks, complete). **Does NOT depend on the event hook
API** (Task 84) — the earlier scripting dependency was aspirational and is dropped. AI
needs only command-block context and a configured endpoint.

---

## Task Summary

| #   | Feature                       | Scope  | Status | Depends On |
| --- | ----------------------------- | ------ | ------ | ---------- |
| 87a | AI Assist — Advisory (opt-in) | Medium | Stub   | Task 72    |

---

## Task 87a — AI Assist — Advisory

Read-only AI features, all triggered by an explicit user gesture, all producing reviewable
text:

- **Output analysis & remediation** — a command exited (OSC 133 gives exact block
  boundaries and exit code); feed the block's output to the endpoint for analysis.
- **Explain-this-error** — on a non-zero exit, a quiet, **local** gutter affordance offers
  "Ask AI about this?"; clicking it (a user gesture) sends the block.
- **Summarization** — summarize a long output block, a `git diff`/`git log`, or the last N
  commands.

### Shared AI foundation (built here, reused by v0.19.0)

This is subtask 1 of the version: every later AI feature sits on it.

- **One OpenAI-compatible chat client.** Config: `base_url`, `model`, `api_key`. Ollama,
  llama.cpp, LM Studio, OpenAI, OpenRouter, etc. all speak this dialect, so there is **one
  client, not a provider matrix**. (Ollama exposes `/v1/chat/completions`.)
- **Default-off, default-local.** Ships pointing at nothing (or example Ollama localhost);
  the user must deliberately configure a remote endpoint + key. No preconfigured cloud.
- **Blocking request + spinner + cancel** in this version (streaming is a v0.19.0
  consideration if it earns its keep).
- **Key storage — layered tagged reference** in `config.toml`:
  - `api_key = { keychain = "..." }` — OS keychain via the `keyring` 4.x crate.
  - `api_key = { env = "..." }` — env-var reference (universal; the GUI round-trips the
    reference, never the secret).
  - `api_key = { literal = "..." }` — discouraged; **the settings GUI never writes this**.
  - **macOS** `use_apple_keychain_store`; **Windows** `use_windows_native_store`.
  - **Linux ladder, persistence-first, never the auto-picker:** Secret Service (zbus) →
    kernel keyutils (with a visible "won't survive reboot" warning) → env-ref/literal.
    Rationale: Secret Service is persistent; keyutils is session-scoped (silent key loss
    on reboot), so it is a warned fallback, not a default.
  - **No SQLite store** (`use_sqlite_store`) — encrypted-with-what-key theater; rejected.
  - Pull only the needed store crates, target-gated, full-semver pinned (`keyring` 4.x +
    per-platform store crates).
- **Privacy = legibility, not redaction.** See invariants below.

### Open questions (decide at activation)

- Which Advisory features ship in this first cut vs slip (error-explain is the anchor;
  summarization is cheap; full output-analysis prompt design needs care).
- Payload-preview UX: the three-button consent (below) is decided; the exact rendering of
  the preview is not.
- Key storage: ship the full tagged-reference (`keychain`/`env`/`literal`) now, or start
  `env`+`literal` (local Ollama needs no key) and add `keychain` in v0.19.0? (Advisory
  against localhost needs no auth, so the key UI can lag by one version if desired.)
- The proactive gutter affordance's exact placement (ties into Task 73 command gutters).

---

## AI Invariants (NON-NEGOTIABLE — apply to v0.18.0 and v0.19.0)

These are durable design decisions, not open questions. They are stated here once and
referenced by `PLAN_VERSION_190.md`.

1. **Never auto-execute.** AI output never runs on its own. It populates the prompt line
   or a scratch buffer; the human presses the key. (Relevant mostly to v0.19.0 generation,
   but stated globally.)
2. **No agentic interaction.** freminal will never run LLM-decided commands in a loop. An
   agent that auto-runs commands is a different product and is explicitly out of scope.
3. **No redaction — byte-for-byte send.** freminal sends exactly what the user asked for,
   unmodified. A scrubber is rejected: it cannot reliably catch secrets and it manufactures
   false trust that makes the user less careful. The honest contract keeps the user's own
   vigilance engaged.
4. **Explicit privacy warnings** on both the AI config page (persistent) and the confirm
   dialog: AI sends terminal output to the configured endpoint, exactly as shown, with no
   filtering; if pointed at a remote service, that service receives whatever is on screen,
   including any visible secrets; freminal does not and cannot scrub this.
5. **Local detection, gestured send.** Proactive _offers_ (e.g. an "Ask AI?" gutter
   affordance on a non-zero exit) are 100% local — detecting the failure and showing the
   affordance sends zero bytes. The network call happens only on a user gesture (a
   keybinding or clicking the affordance). No bytes leave the machine without a gesture in
   _this_ interaction. No background traffic, no model pre-warming, no telemetry.
6. **First-time-per-endpoint consent**, three-button: `[Send & don't ask again for this
endpoint] [Send once] [Cancel]`. Adding a new remote endpoint re-triggers the prompt.
   The payload preview shows the exact bytes to be sent. (Lean: preview always for remote
   endpoints; localhost may skip — decide at activation.)
7. **Adults make the decision.** freminal does not police endpoint choice. Its job is to
   make the data flow legible (visible preview, no surprise traffic), not to restrict it.

## Design Decisions (provisional, but the invariants above are firm)

- **The Advisory/Generative split mirrors the kitty low-risk-first sequencing.** Read-only
  advisory (zero execution risk) ships first and builds the shared endpoint/consent/key
  foundation; generation rides on it in v0.19.0.
- **Out of scope (both AI versions):** ghost-text inline autocomplete (fights the
  architecture, most me-too), suggest-next-command (wants to call ahead of a gesture —
  violates invariant 5), agentic execution (violates invariants 1–2).
- Any consent/preview dialog is a focusable overlay and MUST follow
  `freminal-modal-input-suppression`.
