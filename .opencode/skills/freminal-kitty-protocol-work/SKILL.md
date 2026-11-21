---
name: freminal-kitty-protocol-work
description: Use ONLY when working in the freminal repository AND implementing, changing, activating or reviewing support for a kitty protocol extension -- graphics (APC G), keyboard (CSI u), text sizing (OSC 66), file transfer (OSC 5113), clipboard (OSC 5522), drag and drop (OSC 72), desktop notifications (OSC 99), pointer shapes (OSC 22), colour control (OSC 21), multiple cursors, unscroll, DECCARA, the misc protocol, underlines, or kitty shell integration -- i.e. any v0.13.x task. Codifies spec-first with kitty's code only as tie-breaker, the clean-room rule (kitty is GPLv3, freminal is MIT), truthful and ordered capability answers, payload-free warn logging, the shared foundations a protocol task must reuse, and the KITTY_PROTOCOL_REFERENCE.md update that goes with every change.
---

# Freminal: kitty protocol work

v0.13.x aims at 1:1 compliance with every current, stable kitty protocol extension
(<https://sw.kovidgoyal.net/kitty/protocol-extensions/>). The audit, milestones and
cross-milestone decisions are in `Documents/PLAN_VERSION_130.md`; each task's stub or breakdown is
in its point-release document. This skill is the set of rules every one of those tasks follows.

## Source of truth

- **The spec first.** Implement what the protocol page says.
- **kitty's code only as tie-breaker**, where the spec is silent or self-contradictory. Record
  each such choice in the task's decisions, with the kitty behaviour observed.
- **Where freminal deliberately differs** from kitty, mark it "(differs from kitty)" in the plan
  and list it in `KITTY_PROTOCOL_REFERENCE.md`'s differences for that protocol.
- **Do not infer terminal semantics.** If neither spec nor kitty settles it, stop and ask.
- Check the protocol page's stability first; an unstable or TENTATIVE feature (for example
  clipboard paste events `?5522`) needs maintainer sign-off before decomposition.

## Clean-room rule

kitty is GPLv3 and freminal is MIT. kitty (and any GPL terminal or shell script, including
Ghostty's bash and zsh integration, which derive from kitty's) is read for **observable
behaviour only**. No code, tables, comments or test vectors are copied. Write a behaviour
specification and implement from that. Ghostty's MIT-licensed files may be used with
attribution in `ATTRIBUTIONS.md`.

## Reuse the foundations

These were built once in v0.13.0 so protocol tasks do not re-implement them:

| Need                                           | Use                                                                                    |
| ---------------------------------------------- | -------------------------------------------------------------------------------------- |
| Route a CSI with a prefix or intermediate      | `CsiKey` / `csi_dispatch.rs` (Task 128); add a route, never a final-byte special case  |
| Parse `key=value` metadata                     | `freminal_common::key_value::tokenize` with a named item cap                           |
| base64 (strict, lenient, streaming, unpadded)  | `freminal_common::base64`                                                              |
| Accumulate a chunked payload under caps        | `BoundedChunkAssembler`; never `Vec::with_capacity` from a sender-supplied size        |
| Send a reply                                   | the handler's framed writers (`freminal-reply-path`)                                   |
| Advertise a capability that depends on config  | `HostCapabilities` (Task 130), resolved in `freminal/src/gui/host_capabilities.rs`     |
| Per-screen state and RIS/DECSTR behaviour      | `ScreenScoped<T>`; classify the new field in the exhaustive `reset()` table (Task 131) |
| Colour specs and colour state                  | `parse_color_spec`, `ColorState`, `ResolvedColors` (Task 132)                          |
| Ask the user before reading files or clipboard | the shared consent prompt (Task 133, once built); never a protocol-local modal         |

## Rules every protocol change follows

- **Capability answers are truthful and ordered.** A query reply is computed on the PTY thread
  from what the running config and platform actually deliver, and is written in byte-stream
  order so it precedes a following DA1 reply. Never advertise a feature behind a disabled
  option or a platform that cannot do it.
- **No payload reaches a warn-level log.** Clipboard, transfer, notification and graphics
  payloads can carry secrets. Warn lines carry the introducer, the protocol number, a
  freminal-defined sub-command name and lengths only; detail goes to `debug!`, bounded with the
  `*_for_log_bounded` helpers.
- **Every input is capped.** Names, item counts, chunk sizes and totals each get a named
  constant and a test at the cap and one past it (`freminal-trust-boundary`).
- **New state is typed** (`freminal-state-representation`); a transported flag is never a bool.
- **Snapshot additions are sparse lists** (`Arc<[...]>` with generation caching), never a
  per-cell parallel vector.
- **Consent is never bypassed by default.** A bypass (`pw=`, `name=`) works only when the user
  configured it.
- **Mandatory benchmarks** for anything on the text, snapshot or vertex path
  (`freminal-bench-table`); the unscaled-text fast path must not regress.

## Docs that change with the code

1. `Documents/KITTY_PROTOCOL_REFERENCE.md`: the protocol's section, refreshed to the kitty
   version checked against, including the list of deliberate differences.
2. `ESCAPE_SEQUENCE_COVERAGE.md` and `ESCAPE_SEQUENCE_GAPS.md` (`freminal-escape-sequence-docs`).
3. `res/freminal.ti` and `lookup_termcap` if a terminfo capability describes the feature
   (`freminal-terminfo`).
4. A `test-scripts/` exerciser where the behaviour is only checkable by a human (notifications,
   graphics).

## When to stop and ask

- The spec and kitty disagree and the task's decisions do not already settle it.
- The change needs an architecture decision recorded as a gate in the stub (Task 137 keyboard
  key identity, Task 105 winit drag and drop, Task 145 untagged OSC 133).
- You are about to read a GPL implementation's source for anything beyond behaviour.
- A capability would have to be advertised that the platform or config cannot deliver.
