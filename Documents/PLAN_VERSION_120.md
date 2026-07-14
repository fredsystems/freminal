# PLAN_VERSION_120.md — v0.12.0 "Kitty: Transfer & Cursors + Scrollback Memory"

## Goal

Ship two stable-spec kitty protocols: file transfer over the TTY (OSC 5113) — a stateful
bidirectional session machine with a mandatory user-consent prompt — and multiple cursors
(CSI), a renderer-light addition. The heavy, consent-gated transfer work is balanced by
the small, safe cursor win.

This version also carries the **entire scrollback-memory effort** as a second theme,
deliberately pulled forward and completed in one place rather than spread across later point
releases (a conscious bending of the one-theme-per-version convention — the memory work is
cohesive and the context is hot, so it ships together):

- **Task 118 — Compact Cell Representation** (done): a buffer-layer memory optimisation that
  shrinks stored scrollback rows ~8–12× by sharing formatting across runs and dropping the
  always-null image pointer, plus idle-driven compaction off the hot path. Merged on this
  branch.
- **Task 119 — Scrollback Compression (LZ4)**: an incremental memory multiplier layered on
  the Task-118 compact form — block-granular LZ4 compression of idle scrollback,
  decompress-on-scroll with an LRU block cache, driven by the same idle tick Task 118
  established. LZ4-only (no zstd tier).
- **Task 120 — Compression-Aware Windowed Reflow** (enriched stub): once a very large
  scrollback is affordable, synchronous full-scrollback reflow becomes the new latency wall.
  This absorbs the former 118.10 lazy-reflow stub and the reflow half of the old Task 119,
  because band-decompression and lazy reflow are one control flow. Decomposed at its own
  activation session, not now.

The kitty tasks (102, 103) and the memory tasks (118, 119, 120) are independent and
parallelizable — different sub-agents, no shared seams.

Depends on v0.11.0 (Task 99 establishes the reverse-PTY-write notification path that file
transfer reuses) and the existing lock-free architecture.

**Decomposed** per the `freminal-version-activation` skill (next-up, stable specs), except
Task 120, which stays an enriched stub per the just-in-time planning policy. Re-confirm the
seams at activation before executing.

---

## Task Summary

| #   | Feature                           | Scope     | Status   | Depends On     |
| --- | --------------------------------- | --------- | -------- | -------------- |
| 102 | Kitty File Transfer (OSC 5113)    | Very high | Planned  | Task 99        |
| 103 | Multiple Cursors (CSI)            | Medium    | Planned  | None           |
| 118 | Compact Cell Representation       | Medium    | Complete | None           |
| 119 | Scrollback Compression (LZ4)      | Large     | Planned  | Task 118       |
| 120 | Compression-Aware Windowed Reflow | Large     | Stub     | Tasks 118, 119 |

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

- **The entire scrollback-memory effort lands in this version.** Tasks 118 (compact), 119
  (LZ4 compression), and 120 (compression-aware windowed reflow) were originally spread
  across v0.12.0 and a later v0.13.1 (`PLAN_VERSION_131.md`, now deleted). They are pulled
  together here deliberately — the work is cohesive, the infrastructure Task 118 built (compact
  form, idle driver, decompact-on-read seam, RSS reclaim) is exactly what 119 and 120 reuse,
  and doing it in one place while that context is fresh is worth bending the
  one-theme-per-version convention for. The memory tasks and the kitty tasks share no seams
  and are fully parallelizable.
- **Task 120 is an enriched stub; 119 is fully decomposed.** Per `freminal-version-activation`,
  the large, subtle reflow task is decomposed at its own activation, not now; the compression
  core (119) is decomposed because its prerequisites (Task 118) are already merged.
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

---

## Task 118 — Compact Cell Representation

### 118 Summary

Reduce the resident-memory cost of scrollback so a much larger default scrollback becomes
affordable, by shrinking the in-memory footprint of stored rows. This is **phase one** of the
three-phase scrollback-memory effort that all lands in this version. Phase one is pure
representation/serialization — **no compression codec, no decompression-on-scroll, no reflow
complexity, no new dependency** — and captures the large majority of the achievable win.
Phase two (Task 119) adds LZ4 idle compression as an incremental multiplier on top; phase
three (Task 120) makes reflow of the resulting very-large scrollback affordable.

The measured motivation (feasibility spike, 100k-line corpora with realistic
"stable-structure + unique-content" data):

| Corpus                    | In-memory today | Flat compact repr | Reduction |
| ------------------------- | --------------- | ----------------- | --------- |
| Shell session (typical)   | ~4160 B/line    | ~345 B/line       | **~12×**  |
| Source / logs             | ~3739 B/line    | ~310 B/line       | **~12×**  |
| High-entropy colored (WC) | ~5800 B/line    | ~732 B/line       | **~8×**   |

The in-memory `Cell` is **72 bytes** (18-byte `TChar` + 40-byte `FormatTag` +
2 bools + 8-byte `Option<Box<ImagePlacement>>`, padded to 72). The 40-byte `FormatTag`
is duplicated in full on **every** cell even though runs of adjacent cells almost always
share identical formatting, and the 8-byte image pointer is `None` for essentially every
text cell. A compact representation that (a) shares formatting across runs instead of
per-cell and (b) drops the always-null image slot from the common-case storage recovers
~8–12× with zero runtime decompression cost.

### 118 Design decisions (durable)

- **Phase one is representation only; no codec.** The ~8–12× win comes entirely from
  removing per-cell `FormatTag` duplication and the always-`None` image pointer from stored
  scrollback rows. This is guaranteed regardless of content and adds **zero** read-path
  latency (it is a compact layout, not a compressed blob). Compression (Task 119) is layered
  on later and is explicitly out of scope here.
- **Format-run sharing, not per-cell tags.** Store a row's formatting as a small run list
  (`(FormatTag, run_length)` or a per-row interned tag table + per-cell index), reflecting
  that adjacent cells overwhelmingly share a `FormatTag`. This mirrors the existing
  `FormatTag { start, end, … }` range model already used at the flatten boundary
  (`freminal-common/src/buffer_states/format_tag.rs`) and in `RowCacheEntry.tags`
  (`freminal-buffer/src/buffer/flatten.rs`), so the run model is not a new concept in the
  codebase.
- **Scope the compaction to scrollback rows, not the active region.** The visible/active
  region (`Buffer.rows` tail of length `height`) is mutated constantly and must stay in the
  fast random-access `Vec<Cell>` form. Only rows that have scrolled into history
  (`rows[0 .. rows.len()-height]`) are candidates for the compact form. The boundary is
  crossed when a row scrolls out of the visible window.
- **The `row_cache` duplicate is part of the prize.** `Buffer.row_cache:
  Vec<Option<RowCacheEntry>>` (`buffer/mod.rs:84`) holds a *second*, fully-flattened copy of
  every row (`chars`, `tags`, `bytes`, `byte_to_char`, `auto_urls`). For scrollback rows this
  is pure duplication of data that is rarely read. Evicting / not-populating the cache entry
  for compacted scrollback rows is a first-class part of this task's memory win, separate
  from the cell compaction itself.
- **Correctness is preserved exactly.** The compact form must round-trip losslessly to the
  current `Row`/`Cell` (same `TChar`, same `FormatTag`, same wide-head/continuation flags,
  same image placement when present). Inline-image scrollback rows (rare) may opt out of
  compaction and stay in the `Vec<Cell>` form rather than complicate the compact encoding.
- **No public snapshot/API change if avoidable.** `build_snapshot()` and the flatten path
  consume rows via the existing accessors; the compact form should be internal to
  `freminal-buffer` and decompacted on read at the flatten boundary, so the terminal-emulator
  and GUI layers are unaffected. This respects the crate dependency boundaries in
  `freminal-architecture`.
- **Raising the default scrollback is a deliberate outcome, decided with data.** Once the
  per-line cost drops ~8–12×, the default `ScrollbackConfig.limit` (currently 4000, range
  1..=100_000, `freminal-common/src/config.rs`) can be raised substantially at net-lower
  memory. The exact new default is chosen in 118.5 against the measured post-compaction
  per-line cost, not guessed here.
- **Compaction lives inside `Row`, not `Buffer` (chosen at 118.3 activation, with recon).**
  Two designs were weighed against measured blast radius: (A) change `Buffer.rows` to
  `Vec<StoredRow>` where `StoredRow = enum { Live(Row), Compact(CompactRow) }`, and (B) keep
  `Buffer.rows: Vec<Row>` and give `Row` an internal storage enum
  (`{ Live(Vec<Cell>), Compact(CompactRow) }`) so compaction is transparent behind `Row`'s
  existing accessors. Recon showed Design A touches ~228 `self.rows[...]` sites plus ~28 bare
  `pub`-field accesses (`origin`/`join`/`dirty`/`line_width`) across all 10 `buffer/` files —
  every one a match-arm or new accessor, a large correctness-risk surface. Design B confines
  the storage change to `row.rs`; `Buffer`'s call sites are untouched. **Design B chosen.** The
  performance rationale: the change adds **zero** cost to the hot paths (frame render / flatten,
  PTY-ingest `insert_text`, scroll) under either design, so the deciding factor is
  correctness-risk, and B's is far smaller.
- **Compact rows are never mutated in place — decompact-all-on-resize (118.3 recon finding).**
  The initial assumption that scrollback rows are read-only once they leave the visible window
  is **false**. Recon confirmed three in-place scrollback-touch paths: (1) the `set_size`
  width-change pass (`resize_and_alt.rs:132-146`) force-dirties and re-widths every row incl.
  scrollback after reflow; (2) the `resize_height` grow pass (`resize_and_alt.rs:730-733`)
  dirties `0..old_height` (scrollback indices — see cleanup note below); (3) the whole-buffer
  image-placement-clear family (`images.rs`), reachable from everyday `insert_text`/erase when
  a partially-visible image is overwritten, and from kitty `a=d`. Path (3) is neutralised for
  free: **image rows opt out of compaction**, so an image-clear only ever touches `Live` rows.
  Paths (1)/(2) are handled by **decompacting every compact scrollback row back to `Live` at
  the start of any resize**, letting the existing (delicate) reflow/dirty passes run unchanged,
  then re-compacting out-of-window rows afterward. This is chosen for performance where it
  matters: resize is a rare, human-timescale event already doing O(all rows) work (reflow does
  `mem::take` + full rebuild), so the extra decompact pass is negligible against reflow's own
  cost, adds **zero** cost to every hot path, and keeps the fragile resize logic untouched.
  Making the resize passes compact-aware was rejected: it saves nothing measurable (the saving
  is invisible against reflow, and resize is not hot) at real correctness risk to the most
  delicate code in the buffer.
- **Compaction is a deferred, idle-driven, budgeted background task — NEVER synchronous on a hot
  path (revised after CPU benchmarking).** The original 118.3 design compacted scrollback rows
  synchronously inside `enforce_scrollback_limit` (reached from `insert_text`/`handle_lf`/resize).
  Benchmarking showed this put `CompactRow::from_row` cost directly on hot paths: the worst case
  was `softwrap_heavy` (+45% → +23% after the O(n²) reflow-offset fix), where reflowing one
  5000-char line to width 10 creates ~420 scrollback rows that were all compacted *inside the
  timed resize*. Resize is a hot loop and always will be; a giant line can hit the buffer and
  immediately scroll into history. The durable principle: **having the most recent snapshot
  available to the user, at the expense of slightly delaying the memory saving, is more valuable
  than getting the memory win immediately.** Therefore compaction is moved entirely off the hot
  paths:
  - `enforce_scrollback_limit` (and every other hot path) no longer compacts. Rows that scroll
    into history simply stay `Live` until idle compaction runs.
  - The PTY consumer thread's `select!` loop (`freminal/src/gui/pty.rs`) gains a genuine idle
    tick: a `recv(crossbeam_channel::after(~250ms))` arm that fires only when neither PTY data
    nor GUI input has arrived (the loop previously blocked indefinitely with no timeout arm).
    On that idle tick the thread compacts scrollback. This respects the lock-free architecture:
    the PTY thread still owns `TerminalEmulator` exclusively; no separate timer thread touches
    the buffer.
  - Each idle tick compacts at most a **bounded budget** of rows (e.g. 512) so even a large
    backlog never causes a single long stall; the remainder compacts on subsequent ticks. When
    no compaction work remains, the tick need not re-arm (avoid waking a fully-idle terminal
    forever — battery).
  - Entry point: a new `pub` `Buffer::compact_idle_scrollback(budget) -> usize` (rows compacted),
    passed through `TerminalHandler`/`TerminalEmulator`, callable from the PTY loop via
    `emulator.internal.handler.buffer_mut()`.
  This makes the memory win **eventually-consistent** rather than immediate, which is the correct
  tradeoff for a memory optimisation: the user never pays compaction latency during typing,
  scrolling, or resizing; they get the full snapshot immediately and the memory is reclaimed a
  few hundred ms later once the terminal goes quiet.
- **`row_cache` decompaction seam is via `Row`'s accessors, memoized (118.3).** The 3
  borrow-returning accessors (`cells()`, `characters()`, `cells_mut()`) are the seam: a
  `Compact` `Row` materialises back to `Live` on first cell access and stays `Live` for the
  duration of the read burst, so repeated accesses within one flatten/extract are zero-cost.
  Since scrollback is read-mostly, this one-time cost per read burst is acceptable per 118.5's
  "cold decompact-on-read may slow down; visible-region path must not regress" rule.

### 118 Cleanup entries (surfaced during recon)

- **118.8 — `compact_newly_scrolled_rows` early-stop can miss a decompacted mid-scrollback
  row. RESOLVED / OBSOLETE by 118.9.** This concerned the incremental backward-scan-with-early-
  stop in `compact_newly_scrolled_rows` (the resize-regression fix): an image-clear could
  decompact a mid-scrollback row in place, and the backward scan would break early and never
  re-compact it. 118.9 (deferred idle compaction) **deleted `compact_newly_scrolled_rows`
  entirely** — compaction no longer runs on any hot path, and `compact_idle_scrollback` uses a
  simple forward scan over `0..visible_window_start(0)` that skips already-compact rows and
  compacts any `Live` compactable row it finds, with no early-stop invariant to violate. A row
  an image-clear left `Live` is therefore re-compacted on the next idle tick automatically. No
  further action needed.
- **118.7 — `resize_height` grow-pass dirties scrollback rows, not the new visible window.
  RESOLVED (folded into the 118.5 pass).** The height-grow branch marked rows `0..old_height`
  dirty, but the visible window is bottom-anchored, so when scrollback existed those were the
  OLDEST scrollback rows at the top of the buffer — the wrong rows: it over-invalidated cold
  scrollback (wasting cache rebuilds, and needlessly touching compact rows) while leaving the
  genuinely-newly-visible rows stale. Fixed to invalidate the new bottom-anchored visible window
  (`rows.len()-new_height..rows.len()`). Regression test added
  (`height_grow_invalidates_new_visible_window_not_top_scrollback`) asserting the new visible
  window is invalidated and top-of-scrollback cache entries are retained. Impact was benign
  (over-invalidation, not corruption), consistent with the original assessment.

### 118 Current-state map (from recon)

- **`Cell`** — `freminal-buffer/src/cell.rs:15` (`value: TChar`, `format: FormatTag`,
  `is_wide_head`, `is_wide_continuation`, `image: Option<Box<ImagePlacement>>`). Fields are
  private with accessors; construction via `Cell::new` / `blank_with_tag` /
  `wide_continuation`.
- **`Row`** — `freminal-buffer/src/row.rs:68` (`cells: Vec<Cell>`, `width`, `origin`, `join`,
  `dirty`, `line_width`). Rows are already **sparse** (trailing default-blank cells trimmed,
  e.g. `row.rs:570`).
- **`Buffer.rows: Vec<Row>`** — `buffer/mod.rs:78`; scrollback = indices
  `0..rows.len()-height`, visible = last `height`. **`Buffer.row_cache:
  Vec<Option<RowCacheEntry>>`** — `buffer/mod.rs:84`, index-parallel to `rows`.
- **`FormatTag`** — `freminal-common/src/buffer_states/format_tag.rs:22`; 40 bytes; the only
  heap field is `url: Option<Arc<Url>>` (cloning bumps a refcount, never deep-copies).
  `is_visually_default()` (`format_tag.rs:60`) is the cheap default check.
- **Flatten boundary** — `Buffer::flatten_row` / `rows_as_tchars_and_tags_cached`
  (`buffer/flatten.rs`) is where rows become `RowCacheEntry`; a compact scrollback row must
  decompact correctly through this path.
- **Benchmarks** — `freminal-buffer/benches/buffer_row_bench.rs`
  (`bench_scrollback_flatten`, `bench_scrollback_render`, `buffer_resize`, `softwrap_heavy`)
  and `freminal-terminal-emulator/benches/buffer_benches.rs`
  (`bench_build_snapshot_with_scrollback`) cover the hot paths this task touches.

### 118 Subtasks

#### 118.1 — READ-ONLY audit + compact-representation design

Scope: read-only across `freminal-buffer/src/cell.rs`, `row.rs`, `buffer/mod.rs`,
`buffer/flatten.rs`, `freminal-common/src/buffer_states/format_tag.rs`, and the buffer
benches.

What: produce the concrete design for the compact scrollback-row representation. Decide:
the exact compact type (e.g. `CompactRow { chars: Vec<TChar>, tag_runs: Vec<(FormatTag,
u32)>, flags: …, line_width, origin, join }` or an interned-tag-table variant); the
compaction trigger (row scrolls out of the visible window) and decompaction trigger (row
re-enters visible window / is read for flatten); how inline-image rows are handled (opt out
vs encode); how `row_cache` eviction for compacted rows is wired; and the exact accessor/
flatten seam where decompaction happens so no higher layer sees the compact form. Confirm
the sparse-row invariant interaction. Name every file each later subtask touches.

Deliverable: design report with the chosen type definitions and the file-scoping for
118.2–118.6. No code.

Verification: none (read-only).

Prohibitions: do NOT edit files; do NOT introduce a compression codec (that is Task 119);
do NOT begin implementation; do NOT proceed without maintainer review of the design.

Stop: report design; await explicit sign-off before 118.2.

#### 118.2 — Compact row type + lossless round-trip (pure, in `freminal-buffer`)

Scope: new module `freminal-buffer/src/compact_row.rs` (or as named in 118.1);
`freminal-buffer/src/lib.rs` (module decl); unit tests in the new module.

What: implement the compact row type chosen in 118.1 and the two conversions
`Row -> CompactRow` and `CompactRow -> Row`, exactly lossless for `TChar`, `FormatTag`
(including `Arc<Url>` sharing), wide-head/continuation flags, `line_width`, `origin`,
`join`, and inline-image placement (or the documented opt-out). Pure data transform; no
Buffer integration yet.

Deliverable: the type + conversions + exhaustive round-trip tests (plain rows, colored
runs, mixed tags, wide chars, URL tags, blank/sparse rows, and — per the 118.1 decision —
image rows or the opt-out path). A `size_of`/heap-size assertion demonstrating the
per-row reduction on a representative row.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT touch `Buffer`; do NOT change `Cell`/`Row` public API; do NOT add a
codec; do NOT proceed.

Stop: report + await review.

#### 118.3 — Buffer integration: compact scrollback rows on scroll-out

Scope: `freminal-buffer/src/buffer/mod.rs` (row storage + the scroll-out path), the
scroll/enforce-scrollback sites (`enforce_scrollback_limit`, the scroll path around
`buffer/mod.rs:333`), and `buffer/flatten.rs` (decompact-on-read seam).

What: store scrollback rows in the compact form, decompacting at the flatten/read boundary
so no higher layer observes the change. Compact a row when it scrolls out of the visible
window; keep the visible `height` rows in the existing `Vec<Cell>` form. Preserve every
existing `Buffer` behaviour (visible_rows, resize, alt-screen switch, prompt_rows/
command_blocks index shifting on drain).

Deliverable: integration + tests proving identical observable output (flatten, visible_rows,
snapshot content) before/after compaction across scroll, and that scrollback eviction still
shifts dependent indices correctly.

Verification: `cargo test --all`; clippy. Existing buffer tests must pass unchanged.

Prohibitions: do NOT alter visible-region storage; do NOT change snapshot/public API; do
NOT add a codec; do NOT proceed.

Stop: report + await review.

#### 118.4 — `row_cache` eviction for compacted scrollback rows

Scope: `freminal-buffer/src/buffer/mod.rs` (`row_cache` population/invalidation),
`buffer/flatten.rs` (cache lookup).

What: stop populating / actively evict `RowCacheEntry` for rows that are in the compact
scrollback form, so the second flattened copy is not held for cold history. Re-populate on
demand when such a row is read (decompacted) for flatten. Ensure the cache-index parallelism
with `rows` is maintained through drains and resizes.

Deliverable: cache-eviction logic + tests asserting compacted scrollback rows hold no cache
entry, that reading one repopulates correctly, and that URL auto-detection still works on
re-read.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT evict cache for visible rows; do NOT proceed.

Stop: report + await review.

#### 118.5 — Raise default scrollback + benchmark before/after

Scope: `freminal-common/src/config.rs` (`ScrollbackConfig::default`), `config_example.toml`,
the buffer benches (`freminal-buffer/benches/buffer_row_bench.rs`,
`freminal-terminal-emulator/benches/buffer_benches.rs`).

What: capture before/after memory + throughput per `performance-benchmarks` +
`freminal-bench-table` for `bench_scrollback_flatten`, `bench_scrollback_render`,
`bench_build_snapshot_with_scrollback`, `buffer_resize`, and `softwrap_heavy`. Confirm no
>15% regression on the read/flatten hot paths (some slowdown on the cold decompact-on-read
path is acceptable and expected; the visible-region path must not regress). Using the
measured post-compaction per-line cost, raise `ScrollbackConfig`'s default `limit` to a value
that is net-lower-or-equal memory versus today's 4000-line default (proposed target decided
here with data, not guessed), and update `config_example.toml` and the field doc/comment.

Deliverable: benchmark record (before/after) + the new default + config doc update.

Verification: `cargo test --all`; clippy; `cargo bench --no-run --all`; markdownlint clean
for any doc edits.

Prohibitions: do NOT raise the default without the benchmark justifying it; do NOT regress
the visible-region path >15%; do NOT proceed.

Stop: report + await review.

**DONE.** Default raised **4000 → 10000** (`ScrollbackConfig::default` and the buffer's
compiled-in fallback in `lifecycle.rs`, kept in sync; `config_example.toml` updated). Chosen
with data: measured settled per-line cost after compaction is ~1.0–1.7 KB/line for realistic
colored scrollback (worst realistic ~1.7 KB/line), so 10000 lines ≈ 17 MB resident ≈ the old
4000-line default's ~16.6 MB — 2.5× the history at net-neutral steady-state memory. Config
default-assertion tests updated across `freminal-common`, `freminal-buffer`,
`freminal-terminal-emulator`. Also folded in cleanup 118.7 (below).

#### 118.6 — Windows cross-check + final verification

Scope: no new logic; verification only (plus any trivial fix the cross-check surfaces).

What: run `cargo xtask check-windows` (per `freminal-windows-crosscheck`) since this touches
buffer storage/threading-adjacent code, and the full verification suite. Fix any
Windows-only issue surfaced.

Deliverable: green verification across the suite + Windows cross-check.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`;
`cargo machete`; `cargo fmt --all -- --check`; `cargo xtask check-windows`.

Prohibitions: do NOT add features here; do NOT proceed past a failing check.

Stop: report results.

#### 118.9 — Deferred idle-driven compaction (move compaction off hot paths)

Scope: `freminal-buffer/src/buffer/` (remove hot-path compaction, add
`compact_idle_scrollback(budget)`), `freminal-terminal-emulator` (passthrough on
`TerminalHandler`/`TerminalEmulator`), `freminal/src/gui/pty.rs` (PTY-loop idle tick).

What: implement the "compaction is a deferred, budgeted, idle-driven background task, never
synchronous on a hot path" decision recorded in the durable-decisions section. Two halves:
(a) **buffer layer** — remove every synchronous compaction call from `enforce_scrollback_limit`
and any other hot path; add `pub fn Buffer::compact_idle_scrollback(&mut self, budget: usize) ->
usize`; adapt tests to call it explicitly before asserting compaction. (b) **PTY-loop wiring** —
add a real idle-tick arm (`recv(crossbeam_channel::after(~250ms))`) to the `select!` in
`spawn_pty_consumer_thread`; on idle, call the budgeted compaction through
`emulator.internal.handler.buffer_mut().compact_idle_scrollback(BUDGET)`; re-arm the tick while
the return value is `> 0`, and let it lapse (no re-arm) when there is nothing left to compact so a
fully-idle terminal is not woken forever. Must respect the lock-free architecture: PTY thread owns
`TerminalEmulator` exclusively; no separate timer thread touches the buffer.

Deliverable: hot paths compaction-free (proven by a test that a scrollback fill leaves rows `Live`
until the idle call runs); idle tick wired; before/after CPU benches showing hot paths
(insert/lf/resize/softwrap) no longer pay compaction cost; before/after memory benches confirming
compaction still happens (just deferred).

Verification: `cargo test --all`; clippy; `cargo bench --bench buffer_row_bench -- --baseline
before_118_3` (softwrap_heavy back to ~0%); memory benches; `cargo xtask check-windows` (touches
the PTY thread / crossbeam select).

Prohibitions: do NOT compact on any hot path; do NOT spawn a separate thread that touches the
buffer; do NOT let the idle tick busy-wake a fully-idle terminal.

Stop: report + benches + await review.

#### 118.10 — Windowed / lazy reflow (DISSOLVED into Task 120)

Status: **dissolved.** This subtask was promoted to a first-class task (**Task 120 —
Compression-Aware Windowed Reflow**, later in this document) and merged with the reflow half
of the original Task 119, because band-decompression-on-reflow and lazy reflow are the same
control flow (the band you decompress is the band you reflow, and the async tail is shared).
Building them separately would mean constructing the lazy-reflow band machinery twice. The
durable design principle it captured is preserved in the Task 120 section; nothing is lost.

### 118 Open questions (resolve at activation)

- Compact encoding shape: format-run list vs per-row interned tag table + indices. (Lean:
  run list, since `FormatTag` runs are already the mental model and runs are typically very
  few per row. Decide in 118.1 with a quick size comparison on representative rows.)
- Inline-image scrollback rows: encode into the compact form, or opt out and keep as
  `Vec<Cell>`? (Lean: opt out — image rows are rare and the `Box<ImagePlacement>` complicates
  the flat encoding for negligible gain. Decide in 118.1.)
- New default scrollback value: decided in 118.5 from the measured per-line cost. Candidate
  framing: pick the largest round number whose post-compaction memory ≤ today's 4000-line
  uncompacted memory (likely in the tens of thousands).

---

## Task 119 — Scrollback Compression (LZ4)

### 119 Summary

Compress **blocks** of idle scrollback — already in the Task-118 flat compact form — with
LZ4, decompress on demand when scrolled into view, keep decompressed while visible, and
recompress/evict when the region scrolls back out. This is **phase two** of the
scrollback-memory effort: an incremental multiplier layered on the guaranteed Task-118 win,
targeting the aggregate-memory case that actually hurts — **many tabs/panes open at once**,
where the sum across buffers, not any single buffer, is the pressure.

Scope is deliberately the **compression core only**. Reflow interaction
(band-decompression) is explicitly **out of scope** and lives in Task 120, because it is the
same control flow as lazy reflow and must be built once, together. Task 119 must therefore
leave the existing (synchronous, full-scrollback) reflow path working correctly by
decompressing whatever it needs — slow on a huge scrollback, but correct; Task 120 makes it
fast.

### 119 What Task 118 already provides (reuse, do not rebuild)

Task 118 shipped the infrastructure that made compression the *smaller* half of the effort:

- **The flat, pointer-free compact form** (`CompactRow`, `freminal-buffer/src/compact_row.rs`)
  is the only thing safe to byte-compress — raw `Cell` holds `Arc`/`Box` pointers. LZ4
  operates on the serialized compact bytes, never on `Cell` directly.
- **The idle-driven background driver** (`freminal/src/gui/pty.rs` `select!` idle-tick arm:
  `crossbeam_channel::after(...)`, budgeted work, re-arm-while-work-remains, `never()` disarm
  when caught up) is exactly the mechanism idle *compression* needs. Compression is another
  kind of budgeted work the same tick drives — **do not add a second timer/thread.**
- **The decompact-on-read seam** via `Row`'s memoized accessors (`cells()`/`characters()`/
  `cells_mut()`) is architecturally the same seam decompress-on-scroll needs; extend it, don't
  invent a parallel one.
- **RSS-reclaim discipline** (`malloc_trim(0)` after the backlog drains, glibc-only) is
  already coded; compression's eviction path should honour the same lesson.

### 119 Design decisions (durable)

- **LZ4 is the only codec (no zstd tier).** The `fast-over-ratio` preference plus the
  on-the-fly profile (frequent, small, hot-path block reads) point at LZ4's low per-call
  overhead and ~2,600 MB/s decompress. A zstd "max savings" tier was explicitly dropped: it
  pulls a C dependency and a Windows-cross-check burden for a ratio gain that does not justify
  the complexity here. It may be revisited as a future refinement, not in this version.
- **Compress in blocks, never per line.** A ~40-byte line gives a terrible ratio and pays
  fixed per-call overhead on every access. Block granularity ≈128–256 logical scrollback rows
  (tuned in 119.5). Reading one line decompresses its whole block — a 256-line block ≈ ~88 KB
  flat ≈ ~34 µs at LZ4 speed, well under one 16.6 ms frame.
- **Never compress the active/visible region, nor the compact-but-uncompressed rows near the
  viewport.** Only scrollback idle past a threshold is a compression candidate. The visible
  `height` rows stay `Live`; the Task-118 compact rows near the viewport stay directly
  readable; only cold, deep blocks compress.
- **LRU cache of decompressed blocks + a reusable scratch buffer.** The jank in naive designs
  is allocation churn, not the codec. Decompress once on scroll-into-view, keep live while
  visible, recompress/evict on scroll-out. Steady-state scrolling within a cached region does
  **zero** decompression.
- **New dependency (`lz4_flex`), pure-Rust, added via `flake.nix` + `Cargo.toml`** per
  `flake-dev-shell-discipline` (add to flake, STOP, wait for `nix develop`). Pure-Rust LZ4,
  no C toolchain, Windows-clean. `freminal-buffer` currently has zero
  serialization/compression dependencies; this is the first.
- **Lives in `freminal-buffer`, below the snapshot line.** Compression is internal to the
  buffer; `build_snapshot()`, the terminal-emulator, and the GUI are unaffected — they read
  decompressed/decompacted rows through the existing flatten accessors. Respects the crate
  dependency boundaries in `freminal-architecture`.
- **Compressed blocks hold no `row_cache` entry.** Task 118 already evicts cache for compact
  scrollback rows; a compressed block is even colder and likewise carries no second flattened
  copy. Repopulate on decompress-for-read.
- **Correctness over ratio.** Every block round-trips losslessly to the Task-118 compact form
  and thence to `Row`/`Cell`. A wrong scrollback line is worse than a larger one.

### 119 Measured motivation (feasibility spike)

Ratios are **on top of** the Task-118 flat compact form (100k-line corpora, realistic
"stable-structure + unique-content" data + a pessimistic high-entropy bracket):

| Corpus                    | Flat (Task 118) | flat + LZ4  | Total vs. 72-byte cell |
| ------------------------- | --------------- | ----------- | ---------------------- |
| Shell session (typical)   | ~345 B/line     | ~106 B/line | ~39×                   |
| Source / logs             | ~310 B/line     | ~120 B/line | ~31×                   |
| High-entropy colored (WC) | ~732 B/line     | ~625 B/line | ~9× (worst case)       |

LZ4 decompress ~2,600 MB/s — far above any plausible scroll rate. The bulk throughput number
is **not** what governs on-the-fly cost; per-call overhead, block granularity, and allocation
churn are (addressed by the block + LRU + scratch-buffer decisions above).

### 119 Current-state map (confirm at activation)

- **`CompactRow`** — `freminal-buffer/src/compact_row.rs` (Task 118). Needs a stable byte
  serialization for LZ4 input; confirm whether the in-memory `CompactRow` is already
  contiguous-serializable or needs an explicit encode step.
- **Row storage enum** — `freminal-buffer/src/row.rs` (Task 118 Design B: `Row` holds
  `{ Live(Vec<Cell>), Compact(CompactRow) }`). A third state — a *reference into a compressed
  block* — is the shape to weigh in 119.1 (a `Row` whose storage is `Compressed(block_id,
  offset)` decompressed on access), vs. a separate block store indexed alongside `Buffer.rows`.
- **Idle driver** — `freminal/src/gui/pty.rs` idle-tick arm + `Buffer::compact_idle_scrollback`
  passthrough (`TerminalHandler`/`TerminalEmulator`). Compression reuses this entry point
  pattern (`compress_idle_scrollback` alongside/after compaction).
- **Flatten/read seam** — `Row` accessors + `buffer/flatten.rs`; decompress-on-read extends
  the Task-118 decompact-on-read.
- **Benchmarks** — `freminal-buffer/benches/buffer_row_bench.rs` + the memory benches Task 118
  hardened; add compression-specific block round-trip and scroll-into-compressed benches.

### 119 Subtasks

#### 119.1 — READ-ONLY design audit: block model + storage state + cache/driver seams

Scope: read-only across `compact_row.rs`, `row.rs`, `buffer/mod.rs`, `buffer/flatten.rs`, the
idle driver in `freminal/src/gui/pty.rs`, and the buffer benches.

What: produce the concrete design for: the compressed-block type and where blocks are stored
(a `Row` `Compressed` storage variant vs. a separate block store keyed alongside
`Buffer.rows`); the byte serialization of `CompactRow` fed to LZ4; block size and how logical
scrollback rows map to blocks (and how block boundaries survive scrollback eviction/drain
index-shifting); the LRU cache + scratch-buffer shape; the decompress-on-read seam
(extending Task 118's) and the compress-on-idle entry point (`Buffer::compress_idle_scrollback`
reusing the existing idle tick); `row_cache` interaction; and how the existing synchronous
reflow path decompresses what it needs (correct-but-slow, since fast reflow is Task 120).
Name every file each later subtask touches.

Deliverable: design report with the chosen types and file-scoping for 119.2–119.6. No code.

Verification: none (read-only).

Prohibitions: do NOT edit files; do NOT touch reflow performance (that is Task 120); do NOT
add the dependency yet; do NOT begin implementation; do NOT proceed without maintainer review.

Stop: report design; await explicit sign-off before 119.2.

#### 119.2 — Add `lz4_flex` dependency (flake + Cargo)

Scope: `flake.nix`, `freminal-buffer/Cargo.toml`, workspace `Cargo.toml` if versions are
pinned there.

What: add the pure-Rust `lz4_flex` crate per `flake-dev-shell-discipline` and the
dependency-hygiene rules in `rust-best-practices` (alphabetical sort, full semver pin). Per
the flake discipline: add to `flake.nix`, then **STOP and tell the maintainer to run
`nix develop` / `direnv allow`**, and wait for confirmation before writing code against it.

Deliverable: dependency added + confirmed available in the dev shell.

Verification: `cargo build` (the crate resolves); `cargo machete` (not flagged unused once
119.3 uses it — sequence accordingly).

Prohibitions: do NOT vendor a C-based codec; do NOT add zstd; do NOT proceed past the
STOP-and-wait until the shell is confirmed.

Stop: report; await confirmation the dev shell has the dep.

#### 119.3 — Compressed block type + lossless block round-trip (pure, in `freminal-buffer`)

Scope: new module `freminal-buffer/src/compressed_block.rs` (or as named in 119.1);
`freminal-buffer/src/lib.rs` (module decl); unit tests in the new module.

What: implement the block type chosen in 119.1: serialize a run of `CompactRow`s to bytes,
LZ4-compress to a block, and decompress back to the exact `CompactRow`s (and thence `Row`s).
Reusable scratch buffer for the decompress output. Pure data transform; no `Buffer`
integration yet.

Deliverable: block type + compress/decompress + exhaustive round-trip tests (plain, colored
runs, wide chars, URL tags, blank/sparse rows, block-boundary rows, a high-entropy block) and
a size assertion demonstrating the on-top-of-compact reduction on a representative block.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT touch `Buffer`; do NOT change `Cell`/`Row`/`CompactRow` public API; do
NOT wire the idle driver; do NOT proceed.

Stop: report + await review.

#### 119.4 — Buffer integration: compress cold blocks, decompress-on-read

Scope: `freminal-buffer/src/buffer/mod.rs` (block storage + the compress/decompress paths),
`row.rs` (storage state if a `Compressed` variant is chosen), `buffer/flatten.rs`
(decompress-on-read seam), the LRU cache.

What: store deep-cold scrollback as compressed blocks; decompress at the flatten/read boundary
(extending Task 118's decompact-on-read) so no higher layer observes the change; LRU-cache
decompressed blocks, keep live while visible, recompress/evict on scroll-out. Preserve every
existing `Buffer` behaviour (visible_rows, scrollback eviction index-shifting for
`prompt_rows`/`command_blocks`, alt-screen switch). The **existing synchronous reflow must
still work** by decompressing what it needs (slow on huge scrollback — Task 120 fixes speed).

Deliverable: integration + tests proving identical observable output (flatten, visible_rows,
snapshot content) before/after compression across scroll; scroll-into-a-compressed-block
decompresses and caches; scroll-out recompresses/evicts; eviction still shifts dependent
indices correctly.

Verification: `cargo test --all`; clippy. Existing buffer + Task-118 tests pass unchanged.

Prohibitions: do NOT compress the visible region; do NOT change snapshot/public API; do NOT
optimise reflow (Task 120); do NOT proceed.

Stop: report + await review.

#### 119.5 — Idle-driven compression via the existing tick + block-size tuning

Scope: `Buffer::compress_idle_scrollback(budget)` (`freminal-buffer`), passthrough on
`TerminalHandler`/`TerminalEmulator`, `freminal/src/gui/pty.rs` (extend the existing idle-tick
arm — no new timer/thread).

What: implement compression as budgeted idle work on the **existing** PTY-thread idle tick,
after compaction has caught up (compact first, then compress the now-cold compact blocks).
Re-arm while either compaction or compression has work; disarm (`never()`) when both are
caught up so a quiescent pane is not woken. Honour the `malloc_trim` RSS-reclaim discipline
Task 118 established. Tune block size (128 vs 256) and the idle-past threshold against measured
behaviour.

Deliverable: idle compression wired into the one tick; a test that a scrollback fill stays
`Live` → compacts → compresses across successive idle calls; block-size decision recorded with
the measurement.

Verification: `cargo test --all`; clippy; `cargo xtask check-windows` (touches the PTY thread
/ crossbeam select).

Prohibitions: do NOT add a second timer or thread; do NOT compress on any hot path; do NOT let
the tick busy-wake a fully-idle terminal; do NOT proceed.

Stop: report + await review.

#### 119.6 — Benchmarks, config, escape-sequence-doc check, Windows cross-check

Scope: buffer + emulator benches; `freminal-common/src/config.rs` (any new
`[scrollback]`/compression key, full `freminal-config-options` wiring if added);
`config_example.toml`; verification suite.

What: before/after memory + throughput per `performance-benchmarks` + `freminal-bench-table`
for the scrollback flatten/render/build_snapshot benches plus new
block-round-trip / scroll-into-compressed benches. Confirm no >15% regression on the
read/flatten hot paths (cold decompress-on-read may slow; the visible-region path must not
regress). If a config toggle or capacity knob is added, wire it fully (no `apply_partial`
omission). No escape-sequence surface changes are expected — confirm and note. Run the full
suite + `cargo xtask check-windows`.

Deliverable: benchmark record (before/after) + any config wiring + green suite + Windows
cross-check.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`;
`cargo machete`; `cargo fmt --all -- --check`; `cargo bench --no-run --all`;
`cargo xtask check-windows`; markdownlint clean for any doc edits.

Prohibitions: do NOT skip config wiring if a key is added; do NOT regress the visible-region
path >15%; do NOT proceed past a failing check.

Stop: report results.

### 119 Open questions (resolve at activation)

- Block storage: a `Row` `Compressed(block_id, offset)` storage variant vs. a separate block
  store indexed alongside `Buffer.rows`. (Lean: separate block store — a block spans many rows,
  so per-row storage variants fragment the mental model. Decide in 119.1.)
- Block size (128 vs 256 lines) and idle-past threshold — tune in 119.5 against measured
  scroll behaviour.
- LRU sizing (how many decompressed blocks kept live) and eviction policy — decide in 119.4/119.5.
- Compress ordering vs. compaction on the shared idle tick: strictly compact-then-compress, or
  interleaved under one budget? (Lean: compact-then-compress; simpler invariants. Decide in 119.5.)

---

## Task 120 — Compression-Aware Windowed Reflow

> **STATUS: ENRICHED STUB.** Durable design decisions are captured below; per-subtask
> decomposition happens at activation in a dedicated session, against the code as it then
> exists (see the `freminal-version-activation` skill). Do not invent subtasks early.

### 120 Summary

Make width-resize reflow of a very large scrollback affordable. Once Task 118 (compact) and
Task 119 (LZ4 compression) make tens-of-thousands-to-100k-line scrollback the norm,
**synchronous full-scrollback reflow becomes the new latency wall** — and Task 119
deliberately left the existing reflow correct-but-slow (it decompresses everything it needs).
This task fixes reflow speed with the same recency-first, eventually-consistent philosophy the
memory tasks apply to compaction and compression.

This task **absorbs two previously-separate pieces** that turned out to be one control flow:

1. The former **118.10** lazy/windowed-reflow stub.
2. The **reflow half of the original Task 119** (band-decompression on resize).

They are unified because *the band you decompress is the band you reflow, and the async tail
that finishes decompression is the async tail that finishes reflow.* Building them separately
would construct the lazy-reflow band machinery twice.

### 120 Design principle (durable)

On a width resize:

1. Reflow only the **visible region plus a small scroll-headroom margin** synchronously —
   band-decompressing only the blocks that band needs — producing a correct snapshot for the
   current viewport essentially instantly.
2. **Publish that snapshot immediately**; the user sees the resized view with no perceptible
   delay.
3. Reflow (and re-decompress as needed) the remaining scrollback **lazily/incrementally in the
   background** — reusing the Task-118/119 idle-tick driver — and/or **on-demand as the user
   scrolls up** into not-yet-reflowed history. Recompaction and recompression of reflowed rows
   then follow the normal deferred path.

Reflow cost becomes proportional to what is *visible*, not to total scrollback depth.

### 120 Why this is a stub, not decomposed now

Lazy, compression-aware reflow is substantially larger and subtler than the 118/119 memory
work. It touches logical-line reconstruction, cursor remapping, band-decompression, and — the
hard part — the **`command_blocks` / `prompt_rows` absolute-index remapping** (Task 113 "Bug
R") across a buffer that is only *partially* reflowed to the current width **and** partially
compressed. The buffer must track which scrollback regions are reflowed-to-current-width vs
stale, handle a scroll into a stale and/or compressed region (reflow-and-decompress-on-read),
and keep the absolute-index remaps correct while regions carry mixed widths and mixed
compression states.

Open design questions to resolve at activation:

- How to represent "target width" per row/region, and whether stale regions store their
  pre-resize width for on-read reflow.
- How scroll-offset maps onto a mixed-width, mixed-compression buffer.
- How `visible_window_start` and snapshot bounds behave mid-reflow.
- How the single idle driver sequences three kinds of deferred work — compaction (118),
  compression (119), and reflow-tail (120): shared budget? strict ordering? priority?
- Which thread performs the deferred full reflow, and how a partially-reflowed snapshot is
  represented without violating the lock-free snapshot model (`freminal-architecture`).

Depends on Task 118 (compact representation + idle driver) and Task 119 (block compression +
band-decompression primitive). Decompose in a dedicated session against the code as it then
exists, per `freminal-version-activation`.
