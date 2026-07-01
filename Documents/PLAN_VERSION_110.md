# PLAN_VERSION_110.md — v0.11.0 "Kitty: Notifications & Graphics"

## Goal

Close the first, lower-risk half of the remaining kitty terminal-protocol surface:
stateful desktop notifications (OSC 99), completion of the kitty graphics protocol
(animation, unicode placeholders, relative placements, storage quotas), and a
compliance-gap closure over the existing kitty keyboard protocol (Task 35). Every
protocol here targets a **stable** kitty spec, and all three build on plumbing that
already exists in the codebase (the keyboard work additionally needs new
modifier plumbing from the windowing layer up — see Task 101).

Depends on v0.9.0 (Task 76 notification routing for OSC 99; OSC 133 command blocks
already shipped).

This version is **decomposed** (per the `freminal-version-activation` skill) because it
is next-up and targets stable specs. The subtasks below were re-confirmed against the
current code seams during a 2026-07-01 activation recon (see the per-task current-state
maps in `Documents/KITTY_PROTOCOL_REFERENCE.md`). Re-confirm the seams again if execution
is deferred — the codebase may move.

---

## Task Summary

| #   | Feature                              | Scope       | Status  | Depends On       |
| --- | ------------------------------------ | ----------- | ------- | ---------------- |
| 99  | Kitty Desktop Notifications (OSC 99) | Medium-high | Planned | v0.9.0 (Task 76) |
| 100 | Kitty Graphics Protocol Completion   | Medium-high | Planned | Task 13          |
| 101 | Kitty Keyboard Protocol Compliance   | Medium      | Planned | Task 35          |

> **Scope note (from 2026-07-01 activation recon).** Task 101 was originally
> scoped "Small — verification". Recon against the current spec found freminal
> encodes only 3 of the 8 kitty modifier bits (missing super, hyper, meta,
> caps_lock, num_lock) and is missing whole functional-key classes (keypad
> `CSI u` forms, media keys, modifier-keys-as-keys, F13–F35, lock/print/pause/
> menu). Reaching 100% compliance is real implementation work, so Task 101 is
> re-scoped to **Medium (compliance-gap closure)** and retitled accordingly.
> Task 100 is likewise bumped **Medium → Medium-high**: the recon confirmed
> shared-memory transmission (`t=s`) and zlib compression (`o=z`) — both required
> for 100% compliance — are in scope on top of animation/placeholders/relative/
> quotas.

---

## Reference specs

- OSC 99 — <https://sw.kovidgoyal.net/kitty/desktop-notifications/>
- Graphics — <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
- Keyboard — <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>

A **distilled, freminal-facing** reference for all three protocols — wire
formats, key tables, report/response byte layouts, error codes, quota numbers,
and the per-protocol current-state deltas found during activation recon — lives
in `Documents/KITTY_PROTOCOL_REFERENCE.md`. Implementers and reviewers should
work from that reference (which cross-links back to these subtasks) rather than
re-fetching the upstream specs. Upstream URLs above remain authoritative on any
conflict.

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

### 99 Open questions (resolved at activation, 2026-07-01)

- **Icon-by-data cache (`g=`): in-memory only.** The cache lives for the session
  (terminal process lifetime), satisfying the spec minimum; not persisted across
  runs.
- **macOS close-tracking: emit the `untracked` close form.** On platforms that
  cannot observe OS-side close, reply immediately with
  `ESC ] 99 ; i=<id> : p=close ; untracked ST` and implement the `p=alive`
  polling response so applications can reconcile liveness. This is a spec mandate,
  not a judgment call.
- **OSC 99 routing: `o=` occasion is the primary display gate; `[notifications]`
  retains an on/off kill-switch.** OSC 99's `o=always/unfocused/invisible` model
  drives when a notification is honoured (a superset of the OSC 9/777 behaviour),
  but a master `[notifications] enabled` plus a new `osc_99` toggle still gate it
  on/off, wired end to end per `freminal-config-options` (no silent-drop).

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
behaviour (if any). Produce the precise gap list that 100.2–100.6 implement. Reconcile
one known recon error: an early sub-agent summary claimed relative placements were "a
separate CSI extension, out of scope" — confirm from the code that `P`/`Q`/`H`/`V` are
already typed in `KittyControlData` and that relative placements are APC graphics
commands handled by 100.4 (they are in scope).

Deliverable: a findings report (in chat / appended to this task's notes), not code.

Verification: none (read-only).

Prohibitions: do NOT edit any files; do NOT proceed to implementation.

Stop: report findings; await review and scoping confirmation of 100.2–100.6.

#### 100.2 — Animation: frame transfer + control + compose

Scope: `terminal_handler/graphics_kitty.rs`, `freminal-buffer/src/image_store.rs`
(frame storage), `freminal-common/src/buffer_states/kitty_graphics.rs` (only if a typed
gap is found in 100.1).

What: implement `a=f` (frame transfer, partial-frame rects, composition background
`c=`/`Y=`, blend mode `X=`, edit `r=`, gap `z=`), `a=a` (control: current frame `c=`,
stop/run/loop `s=`, loop count `v=`, per-frame gap), `a=c` (compose). Terminal-driven
animation timing drives the existing frame-advance; reuse the snapshot/renderer image
path. ACK/NACK responses via the existing `format_kitty_response` reverse path,
respecting `q=` quiet modes. While here, close the response-format gap the recon found:
`format_kitty_response` currently emits only `i=<id>`; extend it to include
`,p=<placement_id>` **when** the request specified a non-zero `p=` (the spec requires
`i=<id>,p=<placement>` in that case). Note the per-action key aliasing documented in
`KITTY_PROTOCOL_REFERENCE.md` (e.g. `s`/`v`/`c`/`r`/`z`/`X`/`Y` mean different things
under `a=f`/`a=a`/`a=c` than under transmit/display) — the handler must re-interpret
these by action, since the parser stores them in transmit/display-named fields.

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

What: relative placements are **graphics-protocol APC commands** (`a=p` with
`P=`/`Q=`/`H`/`V`) — not a separate CSI extension — and the parser already types
`P`/`Q`/`H`/`V` in `KittyControlData`, so this is handler/store work only.
Implement `P=`/`Q=` (parent image/placement) with optional `H`/`V` cell offsets;
lifecycle tied to parent (cascade delete); the cursor must not move after a
relative placement regardless of `C`; a virtual placement may be a parent but
cannot itself be made relative (`EINVAL`); chain depth limit (`ETOODEEP` past ≥8);
cycle rejection (`ECYCLE`); missing parent (`ENOPARENT`). Error responses via the
reverse path.

Deliverable: relative-placement handling + tests (offset, cascade delete, virtual
parent, depth/cycle/no-parent errors).

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

#### 100.6 — Shared-memory transmission (`t=s`) + zlib compression (`o=z`)

Scope: `freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs`,
`freminal-common/src/buffer_states/kitty_graphics.rs` (only if a decode helper
belongs there). A new crate dependency for a POSIX/Windows shared-memory object
may be required — if so, add it to the dev shell per `flake-dev-shell-discipline`
and to `Cargo.toml` per `rust-best-practices` before use.

What: two independent-but-related gaps the parser already types but the handler
does not honour.

- `o=z`: when `control.compression == Some(Zlib)`, RFC 1950 zlib-inflate the
  (already-base64-decoded) payload before it is interpreted as raw pixels or PNG.
  Applies to every `f=` format. With PNG + compression the client supplies `S=`
  (source size). Currently `o=z` is parsed and silently ignored, storing garbage.
- `t=s`: replace the current `ENOTSUP` stub with an actual shared-memory read —
  open the named object from the payload, read `S` bytes at offset `O`, then
  `shm_unlink` + `close` (POSIX) / `close` (Windows). Enforce the spec's
  special-file / sensitive-path refusals (mirror the existing `t=f`/`t=t`
  security checks).

Deliverable: both handlers + tests (zlib round-trip decode for RGB/RGBA/PNG; a
shared-memory read that asserts the object is unlinked after read; the security
refusals).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT touch animation, placeholders, or relative placements; do
NOT weaken the file/medium security checks; do NOT proceed.

Stop: report + await review.

#### 100.7 — Escape-sequence docs

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`,
`Documents/KITTY_PROTOCOL_REFERENCE.md`.

What: update the graphics rows to reflect animation / placeholders / relative
placements / quotas / `t=s` / `o=z` / `p=`-in-responses now implemented; refresh
the "Last updated" header. Also flip the graphics "current-state deltas" section
in `KITTY_PROTOCOL_REFERENCE.md` from gap-list to done, and bump its
`Distilled ... as of` date if any spec detail was reconfirmed.

Deliverable: dual-doc update (plus reference-doc refresh).

Verification: markdownlint clean (`markdownlint-cli2`), prettier clean.

Prohibitions: none beyond scope.

Stop: report + await review.

### 100 Open questions (resolved at activation, 2026-07-01)

- **Quota numbers: mirror kitty's defaults as named constants.** Base image
  budget ≈ 320 MB per buffer; animation frame budget = 5× base. Captured as
  constants so they can be tuned without a protocol change.
- **Shared-memory transmission (`t=s`): in scope.** Implement the POSIX/Windows
  shared-memory read (read `S` bytes at offset `O`, then unlink+close on POSIX /
  close on Windows), with the special-file/security refusals the spec requires.
  100% compliance requires it (added as subtask 100.6).
- **Zlib compression (`o=z`): in scope.** The parser types `o=z` but the handler
  never decompresses; implement RFC 1950 inflate before pixel/PNG interpretation
  (added as subtask 100.6 alongside `t=s`).

---

## Task 101 — Kitty Keyboard Protocol Compliance

### 101 Summary

Task 35 shipped the kitty keyboard protocol; the 2026-06-10 fix closed the
functional-key event-type defect. The 2026-07-01 activation recon found this is
**not** a pure verification task: freminal is materially short of 100% compliance
in two areas —

1. **Modifiers.** Only 3 of the 8 kitty modifier bits are modelled
   (shift=1, alt=2, ctrl=4). Missing: super=8, hyper=16, meta=32, caps_lock=64,
   num_lock=128. `KeyModifiers` in `input.rs` has no fields for them, and they are
   not captured at the windowing layer, so no amount of encoding work alone fixes
   it — the modifier state must be plumbed from winit through `InputEvent` first.
2. **Functional keys.** Whole classes are missing their `CSI u` encodings: keypad
   keys (KP_0–KP_9 and friends, 57399–57427), media keys (57428–57440),
   modifier-keys-as-keys (LEFT_SHIFT…ISO_LEVEL5_SHIFT, 57441–57454), F13–F35
   (57376–57398), and the lock/print/pause/menu keys (57358–57363). These are
   primarily reported under flag 8 (report-all-keys).

The stack semantics, set/push/pop handlers, `CSI ? u` query, XTGETTCAP `u`, and
separate main/alt-screen stacks are implemented and tested — those are verified
conformant and must not be rebuilt.

Per the 2026-07-01 activation decision, Task 101 is re-scoped from "verify" to
**close these gaps to 100% compliance**. The full spec surface (all 8 modifier
bits, the complete functional-key table, the encoding format) is captured in
`Documents/KITTY_PROTOCOL_REFERENCE.md`.

### 101 Subtasks

#### 101.1 — READ-ONLY conformance audit (confirm the recon gap list)

Scope: read-only across
`freminal-common/src/buffer_states/modes/kitty_keyboard.rs`,
`freminal-terminal-emulator/src/input.rs`, `terminal_handler/mod.rs` (stack),
`ansi_components/csi_commands/scorc.rs` (`>u`/`<u`/`?u`/`=u`), the winit key
handling in `freminal/src/gui/` and `freminal-windowing`, and the existing
`tests/kitty_keyboard_*.rs` + `input.rs` inline tests.

What: confirm and refine the recon gap list against the code as it stands at
execution time. For each of the 5 flags and the modifier/functional-key surface,
mark conformant vs gap with the spec citation (cite
`KITTY_PROTOCOL_REFERENCE.md` section + the upstream URL) and the exact code
location. Critically, determine **where** super/hyper/meta/caps_lock/num_lock
modifier state is available in the winit event today (if at all) and what must be
added to `InputEvent` / `KeyEventMeta` to carry it — this scopes 101.2. Confirm
which functional keys freminal's `to_payload_kkp` already emits vs. which are
missing.

Deliverable: a refined, code-anchored gap list that fixes the exact scope of
101.2–101.4. Not code.

Verification: none (read-only).

Prohibitions: do NOT edit files; do NOT proceed to fixes without review.

Stop: report findings; await confirmation of the 101.2–101.4 scope.

#### 101.2 — Modifier bits: capture super/hyper/meta/caps_lock/num_lock end to end

Scope: the winit key-event handling (`freminal-windowing` and/or
`freminal/src/gui/` input path), the `InputEvent` definition it feeds,
`KeyModifiers` / `KeyEventMeta` in `freminal-terminal-emulator/src/input.rs`.
**Architecture-affecting** (new fields cross the GUI→PTY `InputEvent` boundary) —
follow `freminal-architecture`; do not introduce shared state, only extend the
existing one-way `InputEvent` channel.

What: extend the modifier model from 3 bits to all 8 kitty bits. Capture
super/hyper/meta and the caps_lock/num_lock lock states from the winit modifiers
at the GUI layer, carry them through `InputEvent`, and surface them on
`KeyModifiers` so `modifier_param()` can compute the full `1 + bitmask` value
(super=8, hyper=16, meta=32, caps_lock=64, num_lock=128). Honour the flag-1
carve-out (lock modifiers are not reported for text-producing keys unless flag 8
is set). Do NOT yet add the missing functional-key encodings (101.3).

Deliverable: full 8-bit modifier plumbing + tests (each new modifier bit produces
the correct `1 + bitmask` value; the lock-modifier carve-out under flag 1 vs
flag 8).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT add functional-key encodings; do NOT alter the stack/query
code (it is conformant); do NOT introduce shared PTY/GUI state; do NOT proceed.

Stop: report + await review.

#### 101.3 — Missing functional-key encodings (keypad, media, modifier-keys, F13–F35, lock/print/pause/menu)

Scope: `freminal-terminal-emulator/src/input.rs` (`to_payload_kkp` and the
functional-key encoding helpers). Depends on 101.2 (modifier bits) being merged.

What: add the missing `CSI u` encodings from the kitty functional-key table
(reproduced in `KITTY_PROTOCOL_REFERENCE.md`): keypad keys KP_0–KP_9,
KP_Decimal/Divide/Multiply/Subtract/Add/Enter/Equal/Separator,
KP_Left/Right/Up/Down/PageUp/PageDown/Home/End/Insert/Delete/Begin (57399–57427);
media keys (57428–57440); modifier-keys-as-keys LEFT_SHIFT…RIGHT_META and
ISO_LEVEL3/5_SHIFT (57441–57454); F13–F35 (57376–57398); CAPS_LOCK, SCROLL_LOCK,
NUM_LOCK, PRINT_SCREEN, PAUSE, MENU (57358–57363). Respect which keys report only
under flag 8 (report-all-keys) vs the disambiguation set. Confirm F3 stays `13 ~`
and never `CSI R` (CPR collision). These keys must be available from the winit
layer — if a key class is not currently delivered as a distinct key, note it and
scope the windowing-layer addition (may fold back into 101.2's plumbing or become
a numbered follow-up).

Deliverable: the missing encodings + tests (a representative case per key class,
with and without modifiers, and under flag 8 for modifier-keys-as-keys).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT touch the modifier plumbing from 101.2 beyond consuming it;
do NOT change stack/query behaviour; do NOT proceed.

Stop: report + await review.

#### 101.4 — Escape-sequence docs

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`,
`Documents/KITTY_PROTOCOL_REFERENCE.md`.

What: record the compliance work (modifier bits + functional-key encodings now
complete); refresh the "Last updated" header; flip the keyboard "current-state
deltas" section in `KITTY_PROTOCOL_REFERENCE.md` from gap-list to done. State the
final compliance status explicitly so a future agent does not re-audit needlessly.

Deliverable: dual-doc update (plus reference-doc refresh).

Verification: markdownlint clean (`markdownlint-cli2`), prettier clean.

Prohibitions: none beyond scope.

Stop: report + await review.

### 101 Open questions (resolved at activation, 2026-07-01)

- **Re-scoped to full compliance, not verification.** The recon confirmed real
  gaps (3-of-8 modifier bits; missing keypad/media/modifier-key/F13–F35/lock key
  encodings). Task 101 now closes them. If 101.1 surfaces a gap materially larger
  than the recon list (e.g. the windowing layer cannot deliver a needed key
  class without significant work), stop and file it as a numbered cleanup entry
  per `freminal-orchestrator-protocol` rather than ballooning a subtask.

---

## Design Decisions

Provisional decisions are marked; the rest were confirmed at the 2026-07-01
activation.

- **v0.11.0 ships full kitty notifications & graphics & keyboard, not a subset.**
  The split across versions is about risk sequencing, not feature trimming. Within
  this version, every protocol is finished to spec (100% compliance is the goal).
- **Reverse-PTY-write reuses existing plumbing.** OSC 99 activation/close/alive
  reports and graphics responses go through `Pane::pty_write_tx` / `write_to_pty` —
  the same path DSR/DA responses and OSC 52 clipboard queries already use. No new
  channel without architecture sign-off (`freminal-architecture`).
- **Capability advertisement is truthful.** The OSC 99 `p=?` handshake (and any
  graphics `a=q` response) advertises only what is actually implemented — never a
  half-supported protocol. Carries forward the Task 76 capability-advertisement
  rule.
- **The three protocols are largely independent workstreams.** They share the
  APC/OSC dispatch and reverse-write plumbing but are otherwise independent and can
  be implemented in parallel (Task 99 vs Task 100 vs Task 101) by separate
  sub-agents. Note Task 101 is now a compliance-gap task, not a verification pass,
  and 101.2 crosses the GUI→PTY `InputEvent` boundary (architecture-affecting).
- **Activation decisions (2026-07-01):**
  - OSC 99 icon-data cache (`g=`) is **in-memory, session-lifetime** only.
  - macOS/untracked-close: emit the `untracked` close form and implement the
    `p=alive` polling response (spec mandate).
  - OSC 99 display gating: `o=` occasion is the primary gate; `[notifications]`
    keeps a wired on/off `osc_99` kill-switch (`freminal-config-options`).
  - Graphics `t=s` (shared memory) and `o=z` (zlib) are **both in scope**
    (subtask 100.6).
  - Task 101 re-scoped to full compliance (all 8 modifier bits + the complete
    functional-key table).
- **A distilled kitty-protocol reference is maintained.**
  `Documents/KITTY_PROTOCOL_REFERENCE.md` holds the wire formats / key tables /
  error codes / current-state deltas for all kitty protocols freminal implements.
  It is a snapshot (kitty ~0.47.x, 2026-07-01); upstream URLs remain authoritative
  on conflict, and each escape-sequence subtask refreshes it.

## Manual test scripts (to be produced after implementation, per maintainer request)

The maintainer requested runnable scripts to manually exercise the **full spec
set** for Tasks 99 and 100 (and, if tractable, 101). Per the "do not generate the
scripts until the full API surface exists" instruction, these are **produced at
the end of each task**, once the implemented surface is concrete — not up front.

- **Task 99 script:** drives every OSC 99 code path — single/chunked title+body,
  update-by-id, close, `c=1` close report, `a=report` activation, buttons (with
  activation index), icons (by name and by transmitted+cached data), sounds,
  urgency, occasion, auto-expiry, `p=alive`, and the `p=?` handshake — printing
  the exact escape sequences and reading back the reverse-path reports so a human
  can confirm each against the spec. Delivered as the final Task 99 subtask.
- **Task 100 script:** drives transmit/put/delete/query, animation (frame
  transfer, control run/stop/loop, compose), unicode placeholders, relative
  placements (incl. the error cases), `t=s`, `o=z`, source-rect crop, and
  quota/eviction, again echoing the wire bytes and any responses. Delivered as
  the final Task 100 subtask.
- **Task 101 script (tentative):** the maintainer noted this is subtler. A
  keyboard-protocol exerciser is best realized as an interactive mode that turns
  on each progressive-enhancement flag and prints the raw `CSI u` bytes freminal
  emits for a scripted set of key presses (all 8 modifiers, keypad/media/modifier/
  F13–F35 keys, event types, associated text), letting a human diff against the
  reference table. Feasibility is decided during 101.3; if an automated harness is
  cleaner than a manual script, that substitutes.

The scripts live under a to-be-decided path (candidate: a `scripts/` or
`test-scripts/` directory at the repo root) and are documented but not wired into
CI (they are manual exercisers, distinct from the mandated `cargo test` suites).
