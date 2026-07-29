# PLAN_VERSION_130.md — v0.13.0 "Kitty: Transfer, Cursors & Text Sizing"

## Goal

Finish the remaining stable-spec kitty protocol surface in one version:

- **Task 102 — Kitty File Transfer (OSC 5113):** a stateful bidirectional session machine
  with a mandatory user-consent prompt.
- **Task 103 — Multiple Cursors (CSI):** a renderer-light addition; the small, safe win
  that balances the heavier transfer work.
- **Task 104 — Kitty Text Sizing (OSC 66):** integer-scaled and fractionally-scaled
  multi-cell text. This is the **highest-risk rendering item** in the entire kitty
  surface — it reworks the cell-grid model to support multicell blocks, fractional font
  scaling within cells, a custom character-width algorithm, and complex overwrite/wrap
  semantics.

Tasks 102 and 103 were originally planned for v0.12.0. They moved here when v0.12.0 was
redefined as a bug-fix-and-performance-only release; their content is unchanged.

Task 104 remains the pacing item. If it slips, 102 and 103 can still ship — the three
tasks are independent and parallelizable (different sub-agents, no shared seams beyond
the OSC dispatch pattern).

Depends on v0.11.0 (Task 99 establishes the reverse-PTY-write path that file transfer
reuses) and on Task 13 (the custom OpenGL renderer / glyph atlas / shaping pipeline that
text sizing extends).

**Decomposed** per the `freminal-version-activation` skill (next-up, stable specs). The
specs are stable; the _risk_ in Task 104 is implementation, not protocol churn.
Re-confirm the renderer and OSC-dispatch seams at activation — they are the ones most
likely to have moved.

---

## Task Summary

| #   | Feature                        | Scope     | Status  | Depends On |
| --- | ------------------------------ | --------- | ------- | ---------- |
| 102 | Kitty File Transfer (OSC 5113) | Very high | Planned | Task 99    |
| 103 | Multiple Cursors (CSI)         | Medium    | Planned | None       |
| 104 | Kitty Text Sizing (OSC 66)     | Very high | Planned | Task 13    |

---

## Reference specs

- File transfer — <https://sw.kovidgoyal.net/kitty/file-transfer-protocol/>
- Multiple cursors — <https://sw.kovidgoyal.net/kitty/multiple-cursors-protocol/>
- Text sizing — <https://sw.kovidgoyal.net/kitty/text-sizing-protocol/>

Every escape-sequence change triggers the dual-document update
(`ESCAPE_SEQUENCE_COVERAGE.md` + `ESCAPE_SEQUENCE_GAPS.md`) per
`freminal-escape-sequence-docs`.

---

## The OSC 66 collision (read before scoping anything)

**freminal already uses OSC 66.** Per `ESCAPE_SEQUENCE_COVERAGE.md`, OSC 66 is currently
treated as the Contour "ColorScheme Notification" — recognised and silently consumed,
with DECRPM `?2031` as the functional adaptive-theme path. Kitty's text-sizing protocol
_also_ uses OSC 66. These are two different protocols claiming the same OSC number.

**This ambiguity MUST be resolved before any text-sizing implementation.** The first
subtask is a code audit (104.1) that confirms what freminal's OSC 66 handler actually
does today, determines whether the Contour interpretation is load-bearing or vestigial,
and decides how OSC 66 is disambiguated (by metadata shape, or by dropping the dormant
Contour interpretation). No implementation proceeds until that decision is made and
recorded in the escape-sequence docs.

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
- **OSC 66 today:** recognised/silently consumed (Contour ColorScheme). Find the handler
  via `OscTarget` (`freminal-common/src/buffer_states/osc.rs`) and
  `dispatch_osc_target()` (`freminal-terminal-emulator/src/ansi_components/osc.rs`).
- **Cell grid / shaping / render (the risk surface):**
  - Shaping: `freminal/src/gui/shaping.rs` `ShapingCache::shape_visible()` →
    `ShapedLine` / `TextRun` / `ShapedGlyph`. `TextRun` already carries `char_widths`
    (1 normal, 2 wide) and a `col_start`/`col_count` model for wide glyphs.
  - Atlas: `freminal/src/gui/atlas.rs` `GlyphAtlas::get_or_insert()` (swash raster).
  - Vertices: `freminal/src/gui/renderer/vertex.rs` `build_foreground_instances()` /
    `build_background_instances()`. The renderer uses **integer cell-grid coordinates
    throughout** — fractional/sub-cell positioning requires moving to float-pixel
    coordinates in both `shaping.rs` and `vertex.rs`. Localized but real.
  - Buffer cell model: `freminal-buffer` — multicell blocks need an ownership concept so
    erase/overwrite (ICH/DCH/ECH/ED/EL/IL/DL) behave per the 7 spec rules.

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

## Task 104 — Kitty Text Sizing (OSC 66)

### 104 Summary

`ESC ] 66 ; <metadata> ; <text> ST` renders `text` at an integer scale (`s=1..7`,
occupying `s*w` columns × `s` rows) and/or a fractional scale (`n/d` with vertical/
horizontal alignment). The terminal must maintain multicell blocks, track their cell
ownership for correct overwrite semantics, apply fractional font scaling within cells,
and implement the kitty character-width algorithm (Unicode 16 grapheme segmentation,
variation selectors). Fire-and-forget (no reverse path beyond the support handshake).

### 104 Subtasks

#### 104.1 — READ-ONLY audit: resolve the OSC 66 collision

Scope: read-only across the OSC 66 handler (`OscTarget`, `dispatch_osc_target`, any
`osc_*` module handling it), `DECRPM ?2031` adaptive-theme path, and the escape-sequence
docs.

What: determine exactly what freminal does with OSC 66 today; whether the Contour
ColorScheme interpretation is reachable/used or vestigial; and how kitty text-sizing OSC
66 can be disambiguated from it (metadata shape differs — kitty uses `s=`/`w=`/`n=`/`d=`
keys; Contour's payload differs). Produce the disambiguation decision and the plan for
the parser to branch correctly (or to drop the Contour path if dormant).

Deliverable: findings + disambiguation decision (chat / task notes). No code.

Verification: none (read-only).

Prohibitions: do NOT edit files; do NOT begin implementation; do NOT proceed without
maintainer sign-off on the disambiguation decision.

Stop: report; await explicit decision before 104.2.

#### 104.2 — OSC 66 parser + disambiguation; escape-sequence docs for the resolution

Scope: the OSC 66 handler module + `freminal-common` types; `OscTarget` /
`dispatch_osc_target` if branching changes; `Documents/ESCAPE_SEQUENCE_COVERAGE.md` /
`ESCAPE_SEQUENCE_GAPS.md`.

What: implement the disambiguation from 104.1 and parse the text-sizing metadata
(`s`,`w`,`n`,`d`,`v`,`h`) into a typed `TextSizingSpec`. Record the OSC 66 resolution in
the escape-sequence docs (this is a behaviour change worth documenting even before the
renderer work).

Deliverable: parser + disambiguation + tests (text-sizing forms, Contour form routed
correctly or removed); dual-doc update of the resolution.

Verification: `cargo test --all`; clippy; markdownlint clean.

Prohibitions: no buffer/render work yet; do NOT proceed.

Stop: report + await review.

#### 104.3 — Character-width algorithm (kitty Unicode-16 grapheme model)

Scope: `freminal-common` or `freminal-buffer` (wherever character width is currently
computed), tests.

What: implement the kitty "splitting text into cells" algorithm: Unicode 16 grapheme
segmentation and variation-selector handling (U+FE0E/U+FE0F changing width). This is the
foundation the multicell model and the rest of the protocol depend on; keep it pure and
exhaustively tested against the spec's examples.

Deliverable: width algorithm + a large table-driven test suite (graphemes, VS15/VS16,
the spec's worked examples).

Verification: `cargo test --all`; clippy.

Prohibitions: no rendering yet; do NOT proceed.

Stop: report + await review.

#### 104.4 — Buffer: multicell block model + ownership + overwrite rules

Scope: `freminal-buffer` (cell model, the mutation ops ICH/DCH/ECH/ED/EL/IL/DL).

What: add the multicell-block concept (a scaled block owns `s*w` columns × `s` rows);
track ownership so the 7 spec overwrite/erase rules behave correctly; ensure all
mutations return the existing structured change description (`freminal-buffer` contract).
No rendering.

Deliverable: multicell model + tests for each of the 7 overwrite/erase interactions.

Verification: `cargo test --all`; clippy.

Prohibitions: no shaping/render changes; do NOT proceed.

Stop: report + await review.

#### 104.5 — Wrapping & overwriting behaviour at the line level

Scope: `freminal-buffer` / `freminal-terminal-emulator` line-handling for scaled blocks.

What: implement the spec's wrapping rules for multicell text (a block that doesn't fit
the line, cursor movement across blocks, editing controls interacting with blocks).

Deliverable: wrap/edit behaviour + tests (block at line end, cursor over a block, edits
spanning blocks).

Verification: `cargo test --all`; clippy.

Prohibitions: no render changes; do NOT proceed.

Stop: report + await review.

#### 104.6 — Renderer: integer→float cell coordinates + scaled-glyph rendering

Scope: `freminal/src/gui/shaping.rs`, `freminal/src/gui/renderer/vertex.rs`,
`freminal/src/gui/atlas.rs` (scaled raster keys if needed).

What: move the affected vertex path from integer cell-grid to float-pixel coordinates so
integer- and fractionally-scaled blocks render at the right size and alignment
(`v=`/`h=`). Render scaled glyphs (raster at `s × base` or scale at draw). Preserve the
existing integer path for normal text (no regression for unscaled content). Follow
`freminal-architecture` (snapshot transport; renderer is a pure read).

Deliverable: scaled rendering + tests (vertex geometry for s=2, fractional n/d with each
alignment); a visual/integration assertion at the achievable level.

Verification: `cargo test --all`; clippy; **mandatory** before/after benchmark capture on
the shaping + `build_snapshot` + render hot paths per `performance-benchmarks` +
`freminal-bench-table` (this is the most performance-sensitive change in the kitty work;
the 15% regression threshold applies).

Prohibitions: do NOT regress the unscaled-text fast path; do NOT proceed.

Stop: report + await review (include the benchmark table).

#### 104.7 — Support-detection handshake

Scope: the OSC 66 handler + reverse-write helper.

What: implement the detection sequence (the spec's CPR-based width/scale probe): respond
so a client probing with `w=2` and `s=2` between CPR queries sees the correct cursor
advancement that signals width-only vs full-scale support — truthfully reflecting what is
implemented.

Deliverable: handshake support + tests asserting the probe responses.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT advertise full scale if only width is implemented; do NOT proceed.

Stop: report + await review.

#### 104.8 — Final escape-sequence docs

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`.

What: mark OSC 66 text sizing implemented with the supported-capability summary (integer
scale, fractional, alignments); refresh the "Last updated" header; ensure the
Contour-vs-kitty resolution from 104.2 is reflected in the final state.

Deliverable: dual-doc update.

Verification: markdownlint clean.

Prohibitions: none beyond scope.

Stop: report + await review.

### 104 Open questions (resolve at activation)

- The integer→float renderer coordinate change (104.6): a localized branch for scaled
  blocks only, or a broader coordinate-system change? The recon says localized; confirm
  against the then-current `vertex.rs` before committing to scope.
- Scaled-glyph rasterisation: raster at the scaled size (atlas key includes scale) vs
  raster once and scale the quad. Quality vs atlas-size tradeoff — decide at activation.
- If 104.4/104.5 reveal the multicell model is larger than estimated, stop and re-scope
  with the maintainer. Task 104 is deliberately independent of Tasks 102 and 103, so it
  can slip without dragging them with it.

---

## Design Decisions (provisional, confirm at activation)

- **This version is the whole remaining stable-spec kitty surface.** Tasks 102 and 103
  were planned for v0.12.0 and moved here when v0.12.0 was redefined as a
  bug-fix-and-performance-only release. Their content moved unchanged; nothing about the
  file-transfer or multiple-cursors design was revisited by the move.
- **Text sizing is isolated from the other two by dependency, not by version.** Task 104
  is the single highest-risk rendering item in the kitty surface. It shares no seams with
  102 or 103, so it can slip without blocking them. Do not add work to v0.13.0 that
  couples them.
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
- **The OSC 66 collision is resolved before implementation.** The Contour ColorScheme
  interpretation and kitty text-sizing share OSC 66; 104.1 resolves the ambiguity and the
  resolution is documented before the renderer work begins. Never silently assume one
  wins.
- **The unscaled fast path must not regress.** Normal (scale-1) text keeps the existing
  integer-cell path; the float-coordinate work is additive for scaled blocks. The
  mandatory benchmark capture in 104.6 guards this.
- **Character width is foundational and pure.** The Unicode-16 grapheme/width algorithm
  (104.3) is built and tested in isolation before anything depends on it.
