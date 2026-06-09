# PLAN — Full Kitty Protocol Support (working title "Plan 13")

> **STATUS: STUB.** This is a placeholder/parking document. It will be
> reordered, renumbered, and broken into properly-scoped tasks before any
> implementation begins. Do not execute anything here yet. The numbering
> ("Plan 13") is provisional — the existing Task 13 is "Image Protocol
> Support" (a kitty-graphics subset), so this document will be assigned a
> real task number when it is activated.

## Goal

Bring freminal to **full coverage of the kitty terminal protocol
extensions** — every protocol kitty has published, not just the keyboard
and graphics subsets we ship today. The driving principle: there is no
good reason to support some notification/input/transfer paths and silently
drop others. If a protocol is well-specified and broadly useful, we want
it.

Read the spec index here before scoping any sub-task:
<https://sw.kovidgoyal.net/kitty/protocol-extensions>

## Why this document exists

During v0.9.0 Task 76 planning it surfaced that OSC 99 (kitty desktop
notifications) had been silently deferred to "v0.10.0 or v0.12.0" as a
loose bullet, with no tracked task. While checking the kitty spec to make
that call, several other kitty protocol extensions came to light that
freminal does not implement and that were never explicitly triaged
(file transfer over the TTY, drag-and-drop, text-sizing, multiple
cursors). Rather than leave these scattered as deferred bullets across
version plans, they are collected here as a single durable backlog so the
full kitty surface can be reasoned about, prioritised, and reordered as
one body of work.

## Current state (what freminal already supports)

| Protocol extension          | Status today    | Notes                                                            |
| --------------------------- | --------------- | ---------------------------------------------------------------- |
| Comprehensive keyboard      | Full            | `CSI > u` / `CSI ? u` / `CSI = u`, push/pop/query (Task 35)      |
| Terminal graphics           | Partial         | APC `_G` transmit/place/delete, RGB/RGBA/PNG, chunked+file (T13) |
| Desktop notifications (99)  | Not implemented | Deferred from v0.9.0 Task 76; this document owns it              |
| File transfer over the TTY  | Not implemented | Never triaged                                                    |
| Drag and drop               | Not implemented | Never triaged                                                    |
| Text sizing                 | Not implemented | Never triaged                                                    |
| Multiple cursors            | Not implemented | Never triaged                                                    |
| Colored / styled underlines | Verify          | May already be covered by SGR work; confirm during scoping       |

## Scope — the full kitty protocol-extension surface

Each item below is a candidate task. Scope, subtask breakdown, and open
design questions are filled in when this document is activated. Each links
to its section of the spec index.

### Desktop notifications — OSC 99

Spec: <https://sw.kovidgoyal.net/kitty/desktop-notifications/>

The reason OSC 99 is not a one-line addition to OSC 9/777 (Task 76): it is
a **stateful** protocol, not a fire-and-forget one. The full surface
includes:

- Multi-chunk, base64-encoded payloads reassembled by notification id.
- Notification identity (`i=<id>`) for updating or closing a live
  notification after it has been shown.
- Action callbacks — the terminal writes an escape sequence **back** to
  the PTY when the user activates or closes a notification (a reverse
  PTY-write path freminal does not have for notifications today).
- Buttons, transmitted icon image data, sounds, expiry timers, urgency.
- Per-notification filtering.
- A support-query handshake (`p=?`).

This is medium-to-large on its own. `notify-rust`'s one-shot `.show()`
(the crate Task 76 uses for OSC 9/777) does not cover the
update/close/activation half of OSC 99.

### Terminal graphics protocol — finish the subset

Spec: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>

We ship transmit/place/delete + RGB/RGBA/PNG + chunked/file transfer +
`a=q` query. Gaps to triage: animation (frame transfer, controlling and
composing animations), unicode placeholders, relative placements, shared
memory transmission, image persistence/storage quotas.

### Comprehensive keyboard handling — verify completeness

Spec: <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>

Believed complete (Task 35). Confirm against the current spec
(progressive-enhancement flags, event types, alternate keys, associated
text, all-keys-as-escape-codes) and close any drift.

### File transfer over the TTY

Spec: <https://sw.kovidgoyal.net/kitty/file-transfer-protocol/>

Send/receive files through the terminal, including symbolic/hard links,
binary deltas (signatures + deltas), compression, and authorization
bypass. Has security implications (the user must authorize transfers);
design the consent UX carefully.

### Drag and drop protocol

Spec: <https://sw.kovidgoyal.net/kitty/dnd-protocol/>

Accepting drops (including from remote machines and reading remote
directories), starting drags (including to remote machines), a
support-detection handshake, multiplexer behavior, a metadata reference,
and a machine-id concept.

### Text sizing protocol

Spec: <https://sw.kovidgoyal.net/kitty/text-sizing-protocol/>

Multi-cell / fractionally-scaled text. Interacts heavily with our cell
grid, shaping, and the glyph atlas. Touches the character-width problem.
Likely the highest-risk item for the rendering pipeline; scope against the
custom OpenGL renderer carefully.

### Multiple cursors protocol

Spec: <https://sw.kovidgoyal.net/kitty/multiple-cursors-protocol/>

Set extra cursors, color them, clear them, query for already-set cursors
and their colors, plus a support query and interaction rules with other
terminal state.

### Colored and styled underlines

Spec: <https://sw.kovidgoyal.net/kitty/underlines/>

Confirm whether existing SGR underline work already covers this; if so,
mark complete and remove from scope. If not, scope the gap.

## Cross-cutting concerns (to resolve at activation time)

- **Support queries.** Several of these protocols define a detection
  handshake so clients can probe support before using the feature. We
  must answer these consistently (and truthfully — never advertise a
  protocol we only half-implement). See the capability-advertisement
  discussion attached to v0.9.0 Task 76.
- **Reverse PTY-write paths.** Notifications (activation callbacks), file
  transfer, and drag-and-drop all require the terminal to write back to
  the PTY in response to user actions. Confirm the architecture supports
  this cleanly under the lock-free GUI/PTY split before committing to
  scope.
- **Security / consent.** File transfer and drag-and-drop move data
  between the local machine and remote sessions. Each needs an explicit
  authorization UX.
- **Escape-sequence documentation.** Every protocol added or altered here
  triggers the mandatory dual-document update
  (`ESCAPE_SEQUENCE_COVERAGE.md` + `ESCAPE_SEQUENCE_GAPS.md`) per
  `agents.md` and the `freminal-escape-sequence-docs` skill.

## References

- Kitty protocol-extensions index —
  <https://sw.kovidgoyal.net/kitty/protocol-extensions>
- `Documents/ESCAPE_SEQUENCE_COVERAGE.md` — current escape-sequence status
- `Documents/ESCAPE_SEQUENCE_GAPS.md` — gap roadmap
- `Documents/MASTER_PLAN.md` — task roadmap (this doc will get a row when
  activated)
- v0.9.0 Task 76 (`PLAN_VERSION_090.md`) — OSC 9/777 notifications, the
  fire-and-forget sibling of OSC 99
