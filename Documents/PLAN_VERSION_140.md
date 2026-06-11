# PLAN_VERSION_140.md — v0.14.0 "Remote"

> **STATUS: ENRICHED STUB.** Durable design decisions are captured below;
> per-subtask decomposition happens at activation in a dedicated session,
> against the code as it then exists (see the `freminal-version-activation`
> skill). Do not invent subtasks early.

## Goal

First-class SSH and remote multiplexing: connect to remote hosts from the terminal, and —
the differentiator — carry a saved freminal layout with you to the remote host, surviving
disconnects with detach/reattach.

Depends on v0.8.0. **Ships non-scriptable**: the event hook API (Task 84) lands later
(v0.19.0), so SSH v1 has no scripting hooks; hook integration is retrofitted when Task 84
lands.

---

## Task Summary

| #   | Feature                      | Scope      | Status | Depends On |
| --- | ---------------------------- | ---------- | ------ | ---------- |
| 86  | SSH Integration + Remote Mux | Very Large | Stub   | v0.8.0     |

Task 86 absorbs `FUTURE_PLANS.md` items B.1 (Remote Mux) and B.7 (SSH Integration).

---

## Task 86 — SSH Integration + Remote Mux

Direct SSH connection from the terminal (connection dialog, key management) plus a remote
multiplexer protocol that lets a layout move with the user to a remote host, survives
disconnects, and supports detach/reattach.

Differentiator vs. WezTerm: first-class **layout propagation** — "take my saved workspace
with me."

**Terminfo propagation lands here** (absorbed from the dropped Task 92). If freminal ever
introduces a real `freminal` TERM entry, propagating it to remote hosts belongs to SSH
integration — mirroring kitty's `kitten ssh` — not a standalone `+install-terminfo`
subcommand. Local terminfo distribution remains an OS-packaging concern.

Open questions (decide at activation):

- Wire protocol (new vs. piggyback on the WezTerm mux protocol).
- Authentication (key delegation, agent forwarding, OS keychain integration — note the
  `keyring` 4.x decisions captured for AI key storage in `PLAN_VERSION_170.md` may apply
  here too).
- Whether a `freminal` TERM is introduced at all (today: `TERM=xterm-256color` +
  XTGETTCAP per Task 12); if not, terminfo propagation is moot.
- Minimum viable first ship vs. full feature set. SSH may itself slip into more than one
  version if scope balloons during design — decompose conservatively at activation.

---

## Design Decisions (provisional)

- **SSH ships non-scriptable.** The earlier plan coupled SSH's security boundary to the
  scripting layer; that dependency is cut. SSH v1 has no hooks; the event hook API
  (Task 84, v0.19.0) retrofits remote-connection hooks later.
- **Layout propagation is the differentiator** and should be in scope from the first
  ship, not deferred.
- **Terminfo-to-remote belongs here, not in a self-install subcommand** (Task 92 dropped).
- Any connection/key-management dialog is a focusable overlay and MUST follow
  `freminal-modal-input-suppression`.
