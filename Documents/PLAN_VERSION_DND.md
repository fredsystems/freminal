# PLAN_VERSION_DND.md — Kitty Drag & Drop (DEFERRED)

> **STATUS: DEFERRED STUB. Do not decompose or implement.** The kitty
> drag-and-drop protocol (OSC 72) spec is still under active development
> upstream (kitty 0.47, issue #9984). Per the `freminal-version-activation`
> skill, a version targeting an unstable spec is NOT decomposed. This stub
> exists so the work is tracked and its design constraints captured; it gets
> a real version number and a subtask breakdown only once kitty freezes the
> protocol.

## Goal

Implement the kitty drag-and-drop protocol (OSC 72): accept drops into terminal programs
and start drags out of them, including across machines.

## Task Summary

| #   | Feature                    | Scope          | Status   | Depends On            |
| --- | -------------------------- | -------------- | -------- | --------------------- |
| 105 | Kitty Drag & Drop (OSC 72) | Extremely high | Deferred | Task 102 (consent UX) |

## Reference spec

- DnD — <https://sw.kovidgoyal.net/kitty/dnd-protocol/>
- Tracking issue (instability) — kitty issue #9984; protocol new in kitty 0.47.

## Why deferred

- **Unstable spec.** OSC 72 is the newest kitty extension (0.47, 2026) and is still under
  active development upstream. Building against a moving target violates the
  build-against-a-frozen-spec rule applied to every other decomposed version.
- **Extremely high complexity.** A two-sided stateful protocol (accepting drops AND
  starting drags), with remote-machine support, remote directory traversal, a machine-id
  concept, a support-detection handshake, multiplexer behavior, chunked binary transfers,
  and platform-specific drag-and-drop API integration (X11/Wayland/macOS/Windows).

## Durable design constraints (captured now, for whenever it activates)

- **Reverse-PTY-write + consent reuse Task 102.** DnD is a sibling of file transfer
  (OSC 5113): both move data across a boundary, both need the reverse-write path and a
  user-consent surface. When activated, DnD reuses the consent overlay pattern and the
  reverse-write plumbing established by Task 102 — it does not invent new ones.
- **Security surface is high.** The terminal must refuse to serve data for a drag
  originating in the same window as the drop target; enforce POSIX error replies for
  permission/IO/size errors on remote directory traversal; HMAC-SHA256 the machine-id.
  These are spec requirements, not optional.
- **Consent overlay** (when built) MUST follow `freminal-modal-input-suppression`.
- **Escape-sequence dual-doc update** (`ESCAPE_SEQUENCE_COVERAGE.md` +
  `ESCAPE_SEQUENCE_GAPS.md`) applies when implemented.

## Activation gate

Do not write subtasks or begin implementation until:

1. The kitty OSC 72 spec is frozen upstream (issue #9984 resolved / protocol marked
   stable), AND
2. The maintainer assigns this a version number and greenlights activation.

Until then this stays a stub. Re-read the spec at activation — it will have changed.
