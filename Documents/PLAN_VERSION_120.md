# PLAN_VERSION_120.md — v0.12.0 "Kitty: Transfer & Cursors"

## Goal

Ship two stable-spec kitty protocols: file transfer over the TTY (OSC 5113) — a stateful
bidirectional session machine with a mandatory user-consent prompt — and multiple cursors
(CSI), a renderer-light addition. The heavy, consent-gated transfer work is balanced by
the small, safe cursor win, so the version stays focused even if transfer expands.

Depends on v0.11.0 (Task 99 establishes the reverse-PTY-write notification path that file
transfer reuses) and the existing lock-free architecture.

**Decomposed** per the `freminal-version-activation` skill (next-up, stable specs).
Re-confirm the seams at activation before executing.

---

## Task Summary

| #   | Feature                        | Scope     | Status  | Depends On |
| --- | ------------------------------ | --------- | ------- | ---------- |
| 102 | Kitty File Transfer (OSC 5113) | Very high | Planned | Task 99    |
| 103 | Multiple Cursors (CSI)         | Medium    | Planned | None       |

---

## Reference specs

- File transfer — <https://sw.kovidgoyal.net/kitty/file-transfer-protocol/>
- Multiple cursors — <https://sw.kovidgoyal.net/kitty/multiple-cursors-protocol/>

Every escape-sequence change triggers the dual-document update
(`ESCAPE_SEQUENCE_COVERAGE.md` + `ESCAPE_SEQUENCE_GAPS.md`) per
`freminal-escape-sequence-docs`.

---

## Current-state map (from activation recon)

- **OSC dispatch / reverse-write:** same seams as v0.11.0 — `dispatch_osc_target()`,
  the 5-step OSC pattern, `write_to_pty` (PTY thread) and `Pane::pty_write_tx` (GUI
  thread). File transfer's bidirectional acks reuse the path Task 99 exercised.
- **Cursor rendering:** `freminal/src/gui/renderer/vertex.rs` `build_cursor_verts_only()`
  emits a single quad from `TerminalSnapshot::cursor_pos` / `cursor_visual_style`.
  Multiple cursors = the snapshot gains a cursor list and this function iterates.
- **Consent modals:** any user-authorization dialog must follow the
  `freminal-modal-input-suppression` skill (register in `ui_overlay_open`,
  `lock_focus(true)`, per-frame `request_focus`) or it cannot be interacted with.

---

## Task 102 — Kitty File Transfer (OSC 5113)

### 102 Summary

A stateful, bidirectional file-transfer session protocol over the TTY. Send/receive
files (and directories, symlinks, hardlinks), with optional zlib compression, rsync-style
binary deltas (signatures + deltas), quiet modes, and — critically — a **mandatory
user-consent prompt** before any transfer proceeds. The terminal writes status/ack/error
back to the application at every step (reverse path). Authorization bypass via
`bypass=sha256:<hash>` is supported but off by default.

This is the highest-complexity item in this version: a full session state machine plus
filesystem I/O on the terminal side plus a consent UX. Decompose conservatively; isolate
the state machine from the I/O from the UI.

### 102 Escape-sequence shape (from spec)

`ESC ] 5113 ; key=value ; ... ESC \` with short wire keys (`ac=` action, `id=` session,
`n=` base64 name, `d=` base64 data, etc.). Flow: client `ac=send|receive id=<sid>` →
terminal `ac=status id=<sid> status=OK|EPERM` → `ac=file` metadata → acks → `ac=data` /
`ac=end_data` chunks with `PROGRESS`/`OK` → `ac=finish`/`ac=cancel`. Quiet: `quiet=1`
(suppress OK), `quiet=2` (suppress all). Compression: `compression=zlib` per file.

### 102 Subtasks

#### 102.1 — READ-ONLY design audit: session model + I/O boundary + consent placement

Scope: read-only across the OSC dispatch seams, the reverse-write path, the GUI overlay
infrastructure (`freminal-modal-input-suppression` targets), and the pane/PTY ownership
model (`freminal-architecture`).

What: produce the concrete design for: where the session state machine lives (PTY-thread
`TerminalHandler` vs a dedicated type), how filesystem I/O is kept off any hot path and
off the GUI thread, how the consent prompt is surfaced (GUI overlay) and how its result
flows back to authorize/deny the session, and how status/ack/error bytes are written via
the reverse path. Identify every file each later subtask will touch.

Deliverable: design report + the file-scoping for 102.2–102.7. No code.

Verification: none (read-only).

Prohibitions: do NOT edit files; do NOT begin implementation; do NOT proceed without
maintainer review of the design (this is the riskiest task in the version).

Stop: report design; await explicit sign-off before 102.2.

#### 102.2 — OSC 5113 wire parser + typed command model

Scope: `freminal-common/src/buffer_states/` (new `osc_file_transfer.rs` or similar),
`freminal-common` tests.

What: parse the `ac=`/`id=`/`n=`/`d=`/… wire format into a typed `FileTransferCommand`
enum (send/receive/file/data/end_data/finish/cancel/status), with base64 decode and the
quiet/compression flags. Pure parser, no state, no I/O.

Deliverable: parser + exhaustive unit tests (each action, base64, quiet modes, malformed).

Verification: `cargo test --all`; clippy.

Prohibitions: no dispatch, no state machine, no I/O; do NOT proceed.

Stop: report + await review.

#### 102.3 — Dispatch wiring (parse path only)

Scope: `OscTarget` (`freminal-common`), `dispatch_osc_target` / `AnsiOscType`
(`freminal-terminal-emulator`), a new handler module stub.

What: route OSC 5113 through the 5-step pattern to a new `TerminalOutput`/`AnsiOscType`
variant carrying the parsed command. No session logic yet.

Deliverable: dispatch wiring + integration test (a `send` start reaches the handler
boundary).

Verification: `cargo test --all`; clippy.

Prohibitions: no state machine, no consent, no I/O; do NOT proceed.

Stop: report + await review.

#### 102.4 — Session state machine (no I/O, no UI)

Scope: the file-transfer handler module (`freminal-terminal-emulator`), session state on
`TerminalHandler`.

What: implement the session lifecycle as a pure state machine: track sessions by `id=`,
sequence the send/receive handshake, decide what status/ack/error each incoming command
produces, gate everything behind an `Authorized`/`Pending`/`Denied` state. I/O is
represented as effects/requests, not performed here. Reverse-write of status/ack/error
goes through the existing path.

Deliverable: state machine + tests driving full send and receive handshakes with a
stubbed authorization=granted and =denied, asserting the exact response bytes.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT perform filesystem I/O; do NOT build the consent UI; do NOT proceed.

Stop: report + await review.

#### 102.5 — Consent prompt (GUI overlay)

Scope: `freminal/src/gui/` (new overlay), wiring from the session's `Pending` state to
the overlay and the user's decision back to the state machine.

What: a modal overlay that names the session (direction, file count/names, size) and
offers Allow / Deny. MUST follow `freminal-modal-input-suppression` (register in
`ui_overlay_open`, `lock_focus(true)`, per-frame `request_focus`) so it is actually
interactable. The decision authorizes/denies the session via the reverse path.

Deliverable: overlay + the authorize/deny round-trip; unit-testable decision logic
separated from egui where practical.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT auto-authorize; do NOT skip the modal-input-suppression
registration; do NOT proceed.

Stop: report + await review.

#### 102.6 — Filesystem I/O (off the hot path / off the GUI thread)

Scope: the file-transfer I/O layer (new module), invoked by the state machine's effects.

What: perform the actual reads/writes for authorized sessions on an appropriate thread
(never the GUI frame thread, never a parser hot path). Honour POSIX-style error replies
(EPERM/ENOENT/EFBIG/etc.) mapped to the protocol's status responses. Symlink/hardlink
handling per spec.

Deliverable: I/O layer + tests against a temp directory (round-trip a small file both
directions; permission-error path).

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT block the GUI or PTY-parse threads; do NOT proceed.

Stop: report + await review.

#### 102.7 — Compression, deltas, bypass-auth; config + escape-sequence docs

Scope: the I/O + session modules; `freminal-common/src/config.rs` (transfer config, full
`freminal-config-options` wiring); `Documents/ESCAPE_SEQUENCE_COVERAGE.md` /
`ESCAPE_SEQUENCE_GAPS.md`; `config_example.toml`.

What: zlib compression per file; rsync signature/delta transfer; `bypass=sha256:<hash>`
authorization bypass (default off, documented as a security tradeoff); any config keys
fully wired (no `apply_partial` omission); dual-doc update.

Deliverable: features + tests (compressed round-trip, delta round-trip, bypass accepted
when configured and rejected otherwise); docs.

Verification: `cargo test --all`; clippy; markdownlint clean.

Prohibitions: do NOT default bypass on; do NOT skip config wiring; do NOT proceed.

Stop: report + await review.

### 102 Open questions (resolve at activation)

- Thread model for I/O: a dedicated transfer thread per session, a shared worker, or
  async on an existing runtime? (Decide in 102.1; must respect the lock-free
  architecture.)
- "Don't ask again" for a trusted peer: in scope, or always-prompt v1? (Lean:
  always-prompt v1; the `bypass` mechanism is the escape hatch.)
- Directory transfers: full recursive support v1, or files-only v1 with directories
  deferred? (Decide in 102.1 based on state-machine complexity.)

---

## Task 103 — Multiple Cursors (CSI)

### 103 Summary

Render extra cursors set by the application via `CSI > … SP q`. Stateful (the terminal
persists the cursor set until cleared) but renderer-light: the snapshot gains a list of
extra cursors, and `build_cursor_verts_only()` iterates. Supports shapes, per-set
colours (cursor + text-under-cursor, with reverse-video modes), clear, and query.

### 103 Escape-sequence shape (from spec)

`CSI > SHAPE ; COORD_TYPE : COORDS ; … SP q`. Shapes: 0 none, 1 block, 2 beam,
3 underline, 29 follow-main, 30 text-under-cursor colour, 40 cursor colour, 100 query
cursors, 101 query colours. Coord types: 0 main, 2 point list `y:x`, 4 rect list
`top:left:bottom:right`. Support query `CSI > SP q` → `CSI > 1;2;3;29;30;40;100;101 SP q`.
Extra cursors clear on ED 2/3/22, reset, alt-screen switch; do NOT scroll with content.

### 103 Subtasks

#### 103.1 — Parser: CSI `> … SP q` multi-cursor commands

Scope: `freminal-terminal-emulator/src/ansi_components/csi_commands/` (the CSI dispatch
for the `SP q` final with `>` prefix), `freminal-common` types for the cursor set.

What: parse set/clear/colour/query forms into typed commands (`MultiCursorCommand`),
including point-list and rect-list coordinate types and the colour-space sub-forms.

Deliverable: parser + unit tests (each shape, each coord type, colour spaces, query
forms, support query).

Verification: `cargo test --all`; clippy.

Prohibitions: no state, no render, no reverse-write; do NOT proceed.

Stop: report + await review.

#### 103.2 — State: extra-cursor set on the terminal, with clear semantics

Scope: `freminal-terminal-emulator/src/terminal_handler/` (cursor-set field), the
existing clear sites (ED 2/3/22, RIS, alt-screen switch).

What: store the extra-cursor set + their colours; apply the clear rules at the right
sites; expand rect lists to cell positions. Extra cursors do not scroll.

Deliverable: state + tests (set then clear via each trigger; rect expansion).

Verification: `cargo test --all`; clippy.

Prohibitions: no render, no reverse-write; do NOT proceed.

Stop: report + await review.

#### 103.3 — Snapshot transport + renderer iteration

Scope: `TerminalSnapshot` (add the extra-cursor list + colours),
`build_snapshot()`, `freminal/src/gui/renderer/vertex.rs` `build_cursor_verts_only()`.

What: carry the extra-cursor list through the snapshot; iterate it in
`build_cursor_verts_only()`, emitting a quad per extra cursor with its shape/colour,
sharing blink state/opacity with the main cursor. Follow `freminal-architecture`
(snapshot is the only transport; `ViewState` not involved).

Deliverable: transport + render + a snapshot round-trip test and a vertex-count
assertion for N extra cursors. If the snapshot-build path is benchmarked, a before/after
capture per `performance-benchmarks` + `freminal-bench-table`.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT route through `ViewState`; do NOT proceed.

Stop: report + await review.

#### 103.4 — Query responses + support handshake

Scope: the multi-cursor handler + reverse-write helper.

What: answer `CSI > 100 SP q` (set cursors), `CSI > 101 SP q` (colours), and the support
query `CSI > SP q` with exactly the supported shape list. FIFO ordering for multiplexer
correctness. Truthful capability advertisement.

Deliverable: query/handshake handlers + tests asserting exact response bytes.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT advertise unsupported shapes; do NOT proceed.

Stop: report + await review.

#### 103.5 — Escape-sequence docs

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`.

What: add the multiple-cursors rows; refresh the "Last updated" header.

Deliverable: dual-doc update.

Verification: markdownlint clean.

Prohibitions: none beyond scope.

Stop: report + await review.

### 103 Open questions (resolve at activation)

- Whether extra cursors blink in lockstep with the main cursor or independently. (Spec:
  share blink state — confirm against the renderer's blink timer.)
- Reverse-video / partial-reverse colour modes: full support v1 or sRGB+indexed first?
  (Lean: full, the colour model is the bulk of the work and is small.)

---

## Design Decisions (provisional, confirm at activation)

- **File transfer and multiple cursors are deliberately mismatched in size.** The medium
  cursor task is the safe win that lets the version ship something even if file transfer
  expands. They are independent and parallelizable (different sub-agents).
- **File transfer consent is never bypassed by default.** The `bypass=sha256` mechanism
  exists per spec but defaults off and is documented as a security tradeoff. The default
  path is always the user-consent overlay.
- **The consent overlay is a real modal.** It MUST register under
  `freminal-modal-input-suppression`; a transfer prompt the user cannot interact with is
  a release blocker.
- **Reverse-write reuses Task 99's path.** No new channel.
- **Drag-and-drop (OSC 72) is NOT in this version.** It is deferred (Task 105,
  `PLAN_VERSION_DND.md`) because its spec is still under active development upstream
  (kitty 0.47, issue #9984). Building it against a moving target violates the
  build-against-a-frozen-spec rule.
