# PLAN_VERSION_130_4.md — v0.13.4 "Drag & Drop"

Part of v0.13.x "Kitty: Full Protocol Compliance". The compliance audit, common-work
analysis, reference specs and cross-milestone design decisions live in the overview,
`PLAN_VERSION_130.md`. This document holds the v0.13.4 tasks only.

**Status:** Planned.

The kitty drag-and-drop protocol (OSC 72). An enriched stub, blocked locally by winit's
drag-and-drop API; decomposed at activation.

## Tasks

| #   | Task                       | Milestone | Scope | Status  | Depends on                         |
| --- | -------------------------- | --------- | ----- | ------- | ---------------------------------- |
| 105 | Kitty Drag & Drop (OSC 72) | v0.13.4   | XL    | Planned | 102, 133; windowing DnD capability |

---

## Task 105 — Kitty Drag & Drop (OSC 72) (stub, v0.13.4)

**Goal.** Implement the drag-and-drop protocol: accepting drops (including remote machines
and remote directory reading), starting drags (including to remote machines), the `t=q`
detection query, multiplexer behaviour and the machine-id.

**Status change (2026-10-08).** The spec was marked stable upstream on 2026-06-12 (commit
`8be4ea67`, "Mark dnd protocol as stable", kitty #9984 closed). Later edits are
security-driven (0.49.0: data requests before a drop must fail with `EPERM`; session data
is discarded on leave) or informational. The protocol therefore meets the "current, stable"
criterion and moved here from the retired `PLAN_VERSION_DND.md`.

**Blocker (local, not spec).** winit 0.30.13 exposes only `DroppedFile`, `HoveredFile` and
`HoveredFileCancelled`. It has no hover position, no MIME list, no drag source and no lazy
data. Activation requires one of:

- a winit upgrade (0.31 is still beta and its DnD API is unverified);
- per-platform integration in `freminal-windowing`, which likely needs FFI. `unsafe` needs
  explicit approval.

That is an architecture decision, made at activation.

**Durable constraints (carried over).**

- Reverse-write and consent reuse Tasks 130 and 133; no new mechanisms.
- **Security is high-stakes.** Refuse to serve data for a drag originating in the same window
  as the drop target. Enforce POSIX error replies (`EPERM`, `EMFILE`, `ENOMEM`, `EFBIG`)
  during remote directory traversal. HMAC-SHA256 the machine-id.
- The consent overlay follows `freminal-modal-input-suppression`.
- Dual-doc update on implementation.
