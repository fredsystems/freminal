# PLAN_VERSION_120.md — v0.12.0 "Kitty: Transfer & Cursors"

## Goal

Ship two stable-spec kitty protocols: file transfer over the TTY (OSC 5113) — a stateful
bidirectional session machine with a mandatory user-consent prompt — and multiple cursors
(CSI), a renderer-light addition. The heavy, consent-gated transfer work is balanced by
the small, safe cursor win, so the version stays focused even if transfer expands.

This version also carries **Task 118 — Compact Cell Representation**, a buffer-layer memory
optimisation unrelated to the kitty work but deliberately slotted here because it is a
cheap, low-risk, high-value change to the buffer's shape, and doing buffer-shape work
*before* more features accrete on top of the buffer is prudent. It is the first phase of a
two-phase scrollback-memory effort; phase two (LZ4 idle compression, Task 119) is a
separate later version (v0.13.1, `PLAN_VERSION_131.md`) that builds on the compact
representation this task introduces.

Depends on v0.11.0 (Task 99 establishes the reverse-PTY-write notification path that file
transfer reuses) and the existing lock-free architecture. Task 118 has no dependency on the
kitty tasks and can proceed in parallel with them.

**Decomposed** per the `freminal-version-activation` skill (next-up, stable specs).
Re-confirm the seams at activation before executing.

---

## Task Summary

| #   | Feature                        | Scope     | Status  | Depends On |
| --- | ------------------------------ | --------- | ------- | ---------- |
| 102 | Kitty File Transfer (OSC 5113) | Very high | Planned | Task 99    |
| 103 | Multiple Cursors (CSI)         | Medium    | Planned | None       |
| 118 | Compact Cell Representation    | Medium    | Planned | None       |

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

---

## Task 118 — Compact Cell Representation

### 118 Summary

Reduce the resident-memory cost of scrollback so a much larger default scrollback becomes
affordable, by shrinking the in-memory footprint of stored rows. This is **phase one** of a
two-phase scrollback-memory effort. Phase one is pure representation/serialization — **no
compression codec, no decompression-on-scroll, no reflow complexity, no new dependency** —
and captures the large majority of the achievable win. Phase two (Task 119, v0.13.1) adds
LZ4 idle compression as an incremental multiplier on top.

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
- **`row_cache` decompaction seam is via `Row`'s accessors, memoized (118.3).** The 3
  borrow-returning accessors (`cells()`, `characters()`, `cells_mut()`) are the seam: a
  `Compact` `Row` materialises back to `Live` on first cell access and stays `Live` for the
  duration of the read burst, so repeated accesses within one flatten/extract are zero-cost.
  Since scrollback is read-mostly, this one-time cost per read burst is acceptable per 118.5's
  "cold decompact-on-read may slow down; visible-region path must not regress" rule.

### 118 Cleanup entries (surfaced during recon)

- **118.7 — `resize_height` grow-pass dirties scrollback rows, not the old visible window.**
  Surface point: 118.3 recon (`resize_and_alt.rs:730-733`). The height-grow branch marks rows
  `0..old_height` dirty, but when scrollback exists the old visible window was
  `rows.len()-old_height..rows.len()`, so `0..old_height` are scrollback-index rows — the wrong
  rows are being dirtied/cache-invalidated. Impact: likely benign today (over-invalidation, not
  corruption) but wastes cache work and is a latent correctness smell; it also interacts with
  compaction (dirtying a compact scrollback row). Scope: `resize_and_alt.rs` height-grow branch
  only. Suggested approach: dirty the correct visible-window range; add a regression test
  asserting scrollback rows retain their cache across a height grow. Verification: `cargo test
  --all`; the new regression test. Scheduling: address within Task 118 (before or alongside
  118.3's decompact-on-resize, since that path overlaps); do not defer past v0.12.0.

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
