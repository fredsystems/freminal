# PLAN_VERSION_100.md — v0.10.0 "Kitty: Notifications & Graphics"

## Goal

Close the first, lower-risk half of the remaining kitty terminal-protocol surface:
stateful desktop notifications (OSC 99), completion of the kitty graphics protocol
(animation, unicode placeholders, relative placements, storage quotas), and a
verification pass over the existing kitty keyboard protocol (Task 35). Every protocol
here targets a **stable** kitty spec, and all three reuse plumbing that already exists
in the codebase.

Depends on v0.9.0 (Task 76 notification routing for OSC 99; OSC 133 command blocks
already shipped).

This version is **decomposed** (per the `freminal-version-activation` skill) because it
is next-up and targets stable specs. The subtasks below are written against the current
code seams. Re-confirm the seams at activation before executing — the codebase may have
moved.

---

## Task Summary

| #   | Feature                              | Scope       | Status  | Depends On       |
| --- | ------------------------------------ | ----------- | ------- | ---------------- |
| 99  | Kitty Desktop Notifications (OSC 99) | Medium-high | Planned | v0.9.0 (Task 76) |
| 100 | Kitty Graphics Protocol Completion   | Medium      | Planned | Task 13          |
| 101 | Kitty Keyboard Protocol Verification | Small       | Planned | Task 35          |

---

## Reference specs

- OSC 99 — <https://sw.kovidgoyal.net/kitty/desktop-notifications/>
- Graphics — <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
- Keyboard — <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>

Every escape-sequence change here triggers the mandatory dual-document update
(`ESCAPE_SEQUENCE_COVERAGE.md` + `ESCAPE_SEQUENCE_GAPS.md`) per the
`freminal-escape-sequence-docs` skill.

---

## Current-state map (from activation recon)

These are the seams the subtasks target. Verify at activation.

- **OSC dispatch:** `freminal-terminal-emulator/src/ansi_components/osc.rs`
  `dispatch_osc_target()`; `OscTarget` enum in
  `freminal-common/src/buffer_states/osc.rs`; per-feature handler modules
  (`osc_notify.rs`, etc.). Adding an OSC is a mechanical 5-step pattern (variant →
  `OscTarget::from()` → `AnsiOscType` variant → dispatch arm → `handle_osc()` arm).
- **APC dispatch:** `ApcParser` (`ansi_components/apc.rs`) is protocol-agnostic;
  `TerminalHandler::handle_application_program_command()` in `terminal_handler/osc.rs`
  is the single dispatch point.
- **Reverse PTY-write path (exists):** `TerminalHandler::write_to_pty()` /
  `write_osc_response()` (`terminal_handler/pty_writer.rs`) on the PTY thread;
  `Pane::pty_write_tx` + `send_pty_response()` (`gui/panes/`, `gui/.../rendering.rs`) on
  the GUI thread. No new channel needed.
- **Notification routing (exists, fire-and-forget):** `NotificationRouter` /
  `NotificationRequest` (`freminal/src/gui/notifications.rs`); `notify-rust` `.show()`
  spawned detached, handle dropped (no activation/close capture today);
  `WindowManipulation::Notification` transports parse→GUI; `NotificationsConfig` in
  `freminal-common/src/config.rs`.
- **Kitty graphics (exists, partial):** `parse_kitty_graphics()` +
  `KittyControlData`/`KittyAction` (`freminal-common/src/buffer_states/kitty_graphics.rs`)
  already parse **every** control key including `a=f/a/c` (animation), `t=s` (shared
  memory), `U=1` (unicode placeholder), `z` (z-index), source rects. Handler arms for
  animation are warn-and-skip in `terminal_handler/graphics_kitty.rs`. `ImageStore` /
  `InlineImage` in `freminal-buffer/src/image_store.rs`.
- **Kitty keyboard (exists, believed complete):** `KittyKeyboardFlags` (5 bits) in
  `freminal-common/src/buffer_states/modes/kitty_keyboard.rs`; per-screen stack in
  `terminal_handler/mod.rs`; key encoding in `freminal-terminal-emulator/src/input.rs`.

---

## Task 99 — Kitty Desktop Notifications (OSC 99)

### 99 Summary

OSC 99 is the **stateful** sibling of the OSC 9/777 fire-and-forget notifications
shipped in Task 76. It adds: multi-chunk base64 payloads reassembled by notification id
(`i=`, `d=` done flag), notification identity for update/close, **activation and close
reports written back to the PTY** (reverse path), buttons, icons (by name and by
transmitted/cached data), sounds, urgency, auto-expiry, and a `p=?` support-query
handshake.

`notify-rust`'s one-shot `.show()` (used by Task 76) does not cover the
update/close/activation half. This task captures the notification handle and its
action/close events instead of discarding it.

### 99 Escape-sequence shape (from spec)

`ESC ] 99 ; <colon-separated key=value metadata> ; <payload> ST`. Key metadata keys:
`a` (actions: `report`/`focus`), `c` (close events), `d` (done/chunking), `e` (base64),
`f` (app name), `g` (icon cache key), `i` (id), `n` (icon name), `o` (occasion),
`p` (payload type: `title`/`body`/`close`/`icon`/`?`/`alive`/`buttons`), `s` (sound),
`t` (type), `u` (urgency 0/1/2), `w` (auto-expire ms). Reverse reports:
activation `ESC ] 99 ; i=<id> ; <btn-index-or-empty> ST`; close
`ESC ] 99 ; i=<id>:p=close ; ST`; alive `ESC ] 99 ; i=<id>:p=alive ; id1,id2 ST`.
Support query `i=<id>:p=?` → response listing supported `a/c/o/p/s/u/w`.

### 99 Subtasks

#### 99.1 — OSC 99 metadata parser + types

Scope: `freminal-common/src/buffer_states/osc.rs` (or a new
`freminal-common/src/buffer_states/osc_notify_99.rs` module), `freminal-common` tests.

What: add an `Osc99Command` type and a `parse_osc_99(metadata, payload)` function that
parses the colon-separated `key=value` metadata into a typed struct (mirror the kitty
spec key table: `Osc99Payload` enum for `p=`, `Osc99Action`, urgency enum, etc.) and
decodes the payload (base64 when `e=1`). Pure parser — no handler, no state. Follow the
existing `kitty_graphics.rs` parser style (typed enums, `KittyParseError`-style error).

Deliverable: the parser + exhaustive unit tests (one per key, chunking flag, base64
on/off, malformed metadata, the `p=?` query form).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT wire it into dispatch yet; do NOT add reverse-write here; do NOT
proceed to 99.2.

Stop: report files changed + verification; await review.

#### 99.2 — OSC 99 dispatch wiring (parse path only)

Scope: `freminal-common/src/buffer_states/osc.rs` (`OscTarget`),
`freminal-terminal-emulator/src/ansi_components/osc.rs` (`dispatch_osc_target`,
`AnsiOscType`), `freminal-terminal-emulator/src/ansi_components/osc_notify.rs` (or a new
`osc_notify_99.rs`).

What: wire OSC number 99 through the 5-step OSC pattern so a parsed `Osc99Command`
reaches a new `TerminalOutput`/`AnsiOscType` variant. No state machine yet — a single
non-chunked title notification should reach the handler boundary.

Deliverable: dispatch wiring + a parser-to-dispatch integration test.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT implement chunk reassembly or reverse-write; do NOT touch the GUI
notification router; do NOT proceed to 99.3.

Stop: report + await review.

#### 99.3 — Notification identity + chunk reassembly state

Scope: `freminal-terminal-emulator/src/terminal_handler/` (new field on
`TerminalHandler` for the in-flight notification map; handler for the dispatched OSC 99
variant).

What: add a `HashMap<NotificationId, PendingNotification>` to `TerminalHandler`. Reassemble
multi-chunk payloads (`d=0` → accumulate, `d=1`/default → finalize). On finalize, emit a
`WindowManipulation::Notification`-family command (extended in 99.4) carrying id, title,
body, buttons, urgency, sound, icon, expiry, and the `a=`/`c=` flags that determine
whether reports are expected. Update-existing (same `i=`) replaces in place.

Deliverable: reassembly + identity logic + unit tests (chunked title+body, update by id,
unidentified-never-merged).

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT implement the reverse report path (99.6) or the GUI display (99.5);
do NOT proceed.

Stop: report + await review.

#### 99.4 — Extend WindowManipulation::Notification for OSC 99 fields

Scope: `freminal-common/src/buffer_states/window_manipulation.rs` (the
`WindowManipulation::Notification` variant + `NotificationKind`), snapshot transport in
`freminal-terminal-emulator/src/.../snapshot.rs`.

What: extend the notification command/snapshot payload to carry the OSC 99 superset
(id, buttons, urgency, sound, icon spec, expiry, report-wanted flags) without breaking
the existing OSC 9/777 producers (they fill `None`/defaults). This is a config-shaped
change — follow `freminal-config-options` discipline if any new config field is implied
(none expected here).

Deliverable: the extended type + snapshot round-trip test; existing OSC 9/777 tests
still green.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT change GUI behaviour yet; do NOT proceed.

Stop: report + await review.

#### 99.5 — GUI: render OSC 99 notifications with identity, buttons, icons, expiry

Scope: `freminal/src/gui/notifications.rs`, the notification drain site in `freminal/src/gui/`
(where `WindowManipulation::Notification` is consumed).

What: extend `NotificationRouter` to (a) track live notifications by id so update/close
work, (b) pass buttons/urgency/sound/expiry/icon to `notify-rust`, (c) **retain the
`notify-rust` handle** rather than dropping it, so action/close callbacks can be observed.
Icon-by-name and icon-by-data (with `g=` cache) supported. Keep the existing toast leg
working for notifications that want it.

Deliverable: extended router + unit tests for routing/identity/expiry decisions (the OS
display leg stays best-effort/unasserted as today).

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT wire the reverse-write yet (99.6); do NOT proceed.

Stop: report + await review.

#### 99.6 — Reverse path: activation + close + alive reports to the PTY

Scope: `freminal/src/gui/notifications.rs` (capture `notify-rust` action/close events),
the GUI pane plumbing that owns `Pane::pty_write_tx`, and
`freminal-terminal-emulator/src/terminal_handler/pty_writer.rs` if a helper is needed.

What: when a tracked notification is activated (whole-notification or a button) and
`a=report` was set, write `ESC ] 99 ; i=<id> ; <btn-index-or-empty> ST` back via
`Pane::pty_write_tx`. When closed and `c=1`, write the `p=close` report. Implement the
`p=alive` poll response. This is the established reverse-write path — no new channel.

Deliverable: the write-back wiring + tests that assert the exact bytes written for
activation/close/alive given a tracked notification.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT invent a new channel; do NOT proceed.

Stop: report + await review.

#### 99.7 — Support-query handshake (`p=?`)

Scope: the OSC 99 handler (`terminal_handler/`) + reverse-write helper.

What: answer `i=<id>:p=?` with the response form listing exactly the actions/occasions/
payload-types/sounds/urgencies/expiry freminal actually supports — **truthfully**, never
advertising unimplemented capability (capability-advertisement rule from Task 76).

Deliverable: handshake handler + test asserting the response string matches implemented
capability.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT advertise unimplemented features; do NOT proceed.

Stop: report + await review.

#### 99.8 — Config surface + escape-sequence docs

Scope: `freminal-common/src/config.rs` (if OSC 99 needs any `[notifications]`
additions — follow the `freminal-config-options` `ConfigPartial`/`apply_partial`
checklist in full), `Documents/ESCAPE_SEQUENCE_COVERAGE.md`,
`Documents/ESCAPE_SEQUENCE_GAPS.md`, `config_example.toml` if a key is added.

What: any new config keys wired end to end (no silent-drop); dual-doc update marking OSC
99 implemented with the supported-capability summary and "Last updated" header.

Deliverable: docs updated; config (if any) fully wired with a partial-merge test.

Verification: `cargo test --all`; clippy; markdownlint clean.

Prohibitions: do NOT skip the `apply_partial` wiring if a config key is added.

Stop: report + await review.

### 99 Open questions (resolve at activation)

- Icon-by-data cache (`g=`): in-memory only, or persisted across runs? (Lean: in-memory.)
- macOS close-tracking limitation (spec notes close is untracked) — emit the
  `untracked` close form or suppress? (Spec says emit `untracked`.)
- Do we surface OSC 99 notifications through the same `[notifications]` routing policy as
  OSC 9/777, or does OSC 99's richer `o=` occasion model override it?

---

## Task 100 — Kitty Graphics Protocol Completion

### 100 Summary

Finish the kitty graphics subset shipped in Task 13. The control-data parser
(`kitty_graphics.rs`) already types every key; the work is filling stubbed handler arms
and adding the storage-management policy. Scope: animation (frame transfer, control,
compose), unicode placeholders (U+10EEEE + diacritics), relative placements
(parent/child groups), and image persistence / storage quotas.

### 100 Subtasks

#### 100.1 — READ-ONLY audit of current graphics handler completeness

Scope: read-only across `terminal_handler/graphics_kitty.rs`,
`freminal-buffer/src/image_store.rs`, `freminal-buffer/src/buffer/images.rs`,
`freminal/src/gui/renderer/vertex.rs` (`build_image_verts`).

What: enumerate exactly which `KittyAction` arms are warn-and-skip vs implemented; which
control keys are parsed-but-ignored at handler level; the current image-store eviction
behaviour (if any). Produce the precise gap list that 100.2–100.5 implement.

Deliverable: a findings report (in chat / appended to this task's notes), not code.

Verification: none (read-only).

Prohibitions: do NOT edit any files; do NOT proceed to implementation.

Stop: report findings; await review and scoping confirmation of 100.2–100.5.

#### 100.2 — Animation: frame transfer + control + compose

Scope: `terminal_handler/graphics_kitty.rs`, `freminal-buffer/src/image_store.rs`
(frame storage), `freminal-common/src/buffer_states/kitty_graphics.rs` (only if a typed
gap is found in 100.1).

What: implement `a=f` (frame transfer, partial-frame rects, composition background
`c=`/`Y=`, blend mode `X=`, edit `r=`, gap `z=`), `a=a` (control: current frame `c=`,
stop/run/loop `s=`, loop count `v=`, per-frame gap), `a=c` (compose). Terminal-driven
animation timing drives the existing frame-advance; reuse the snapshot/renderer image
path. ACK/NACK responses via the existing `format_kitty_response` reverse path,
respecting `q=` quiet modes.

Deliverable: animation handling + tests (frame add, gap timing, loop count, compose
rectangle).

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT touch unicode placeholders or relative placements; do NOT proceed.

Stop: report + await review.

#### 100.3 — Unicode placeholders (U+10EEEE + diacritics)

Scope: `terminal_handler/graphics_kitty.rs` (virtual placement on `a=p,U=1`), the cell
write path that must recognise U+10EEEE + row/column diacritics, renderer
`build_image_verts` (place image section per diacritics).

What: create a virtual placement on `a=p,U=1,i=,c=,r=`; watch the character stream for
U+10EEEE carrying image-id-in-foreground-color + row/column combining diacritics; render
the indicated image section in that cell. Use the kitty `rowcolumn-diacritics` mapping.

Deliverable: placeholder handling + tests (virtual placement creation, diacritic decode,
a small grid render assertion at the buffer level).

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT touch animation or relative placements; do NOT proceed.

Stop: report + await review.

#### 100.4 — Relative placements (parent/child groups)

Scope: `terminal_handler/graphics_kitty.rs`, `image_store.rs` (placement group links).

What: implement `P=`/`Q=` (parent image/placement) with optional `H`/`V` cell offsets;
lifecycle tied to parent (cascade delete); chain depth limit (`ETOODEEP` past ≥8); cycle
rejection (`ECYCLE`); missing parent (`ENOPARENT`). Error responses via the reverse path.

Deliverable: relative-placement handling + tests (offset, cascade delete, depth/cycle
errors).

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT touch animation or placeholders; do NOT proceed.

Stop: report + await review.

#### 100.5 — Storage quotas + eviction policy

Scope: `freminal-buffer/src/image_store.rs`.

What: enforce a storage quota (base-image budget; larger budget for animation frames);
on overflow evict oldest, preferring placement-less images. No I/O on hot paths beyond
what Task 13 already does.

Deliverable: quota + eviction + tests (eviction order, placement-less preference).

Verification: `cargo test --all`; clippy; if the image hot path is benchmarked, a
before/after capture per `performance-benchmarks` + `freminal-bench-table`.

Prohibitions: do NOT change protocol parsing; do NOT proceed.

Stop: report + await review.

#### 100.6 — Escape-sequence docs

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`.

What: update the graphics rows to reflect animation/placeholders/relative/quotas now
implemented; refresh the "Last updated" header.

Deliverable: dual-doc update.

Verification: markdownlint clean.

Prohibitions: none beyond scope.

Stop: report + await review.

### 100 Open questions (resolve at activation)

- Quota numbers: mirror kitty's defaults (≈320 MB base, 5× frames) or pick freminal
  values? (Lean: mirror kitty, make it a constant.)
- Shared-memory transmission (`t=s`): in scope for this version or deferred? (The parser
  types it; the handler may stub it. Decide based on 100.1 findings.)

---

## Task 101 — Kitty Keyboard Protocol Verification

### 101 Summary

Task 35 shipped the kitty keyboard protocol; the 2026-06-10 fix closed the
functional-key event-type defect. This task **verifies completeness** against the current
spec rather than building new infrastructure: confirm all five progressive-enhancement
flags are correctly encoded, the per-screen stack semantics are correct, and the
detection handshake is right. Close any drift found; do not rebuild what works.

### 101 Subtasks

#### 101.1 — READ-ONLY conformance audit against current spec

Scope: read-only across
`freminal-common/src/buffer_states/modes/kitty_keyboard.rs`,
`freminal-terminal-emulator/src/input.rs`, `terminal_handler/mod.rs` (stack),
`ansi_components/csi_commands/` (`>u`/`<u`/`?u`/`=u`), and the existing
`tests/kitty_keyboard_*.rs`.

What: check each of the 5 flags (disambiguate, report-event-types, report-alternate-keys,
report-all-keys-as-escape-codes, report-associated-text) against the spec encoding,
including the `key:shifted:base` sub-fields, modifier bitmask, and the detection
handshake (`CSI ? u` then DA1). Produce a precise drift list: conformant vs gap, each
with the spec citation and the offending code location.

Deliverable: findings report (chat / task notes), not code.

Verification: none (read-only).

Prohibitions: do NOT edit files; do NOT proceed to fixes without review.

Stop: report findings; await scoping of 101.2 fixes (if any).

#### 101.2 — Close any drift found (scoped from 101.1)

Scope: defined by the 101.1 findings — strictly the files identified, nothing else.

What: fix the specific encoding/stack/handshake drift items the audit found. If 101.1
finds full conformance, this subtask is a no-op closed with "verified conformant; no
changes" and the audit becomes the deliverable.

Deliverable: targeted fixes + regression tests for each drift item, OR a documented
"verified conformant" result.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT refactor beyond the drift list; do NOT change unrelated keyboard
behaviour; do NOT proceed.

Stop: report + await review.

#### 101.3 — Escape-sequence docs

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`.

What: record the verification result (and any fixes); refresh the "Last updated" header.
If conformant, state so explicitly so a future agent does not re-audit needlessly.

Deliverable: dual-doc update.

Verification: markdownlint clean.

Prohibitions: none beyond scope.

Stop: report + await review.

### 101 Open questions (resolve at activation)

- None expected; this is a verification task. If 101.1 surfaces a large gap (unlikely),
  stop and re-scope with the maintainer rather than ballooning 101.2.

---

## Design Decisions (provisional, confirm at activation)

- **0.10.0 ships full kitty notifications & graphics, not a subset.** The split across
  three versions is about risk sequencing, not feature trimming. Within this version,
  every protocol is finished to spec.
- **Reverse-PTY-write reuses existing plumbing.** OSC 99 activation/close reports go
  through `Pane::pty_write_tx` / `write_to_pty` — the same path DSR/DA responses and OSC
  52 clipboard queries already use. No new channel without architecture sign-off
  (`freminal-architecture`).
- **Capability advertisement is truthful.** The OSC 99 `p=?` handshake (and any graphics
  `a=q` response) advertises only what is actually implemented — never a half-supported
  protocol. Carries forward the Task 76 capability-advertisement rule.
- **Notifications & graphics are two workstreams in one version.** They share the APC/OSC
  dispatch and reverse-write plumbing but are otherwise independent; they can be
  implemented in parallel (Task 99 vs Task 100) by separate sub-agents, with Task 101
  (verification) running independently of both.
