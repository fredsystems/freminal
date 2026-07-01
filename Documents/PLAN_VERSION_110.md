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

| #   | Feature                                   | Scope       | Status  | Depends On       |
| --- | ----------------------------------------- | ----------- | ------- | ---------------- |
| 99  | Kitty Desktop Notifications (OSC 99)      | Medium-high | Planned | v0.9.0 (Task 76) |
| 100 | Kitty Graphics Protocol Completion        | Medium-high | Planned | Task 13          |
| 101 | Kitty Keyboard Compliance (encoding-only) | Medium      | Planned | Task 35          |
| 114 | Kitty Keyboard: egui-blocked keys (winit) | Medium-high | Stub    | Task 101         |

> **Scope note (from 2026-07-01 activation audits).** The 101.1 audit found the
> binding constraint for keyboard compliance is **egui 0.35**, not freminal's
> encoding layer: egui drops super/caps_lock/num_lock modifiers and does not even
> deliver keypad-operator / media / lock / print / pause / menu keys to freminal
> (absent from egui's `Key` enum). So keyboard work is split: **Task 101** does the
> encoding-only wins that need no windowing change (8-bit modifier arithmetic +
> `super` via SuperLeft/Right tracking; F13–F35 and modifier-keys-as-keys
> encodings), and **new Task 114** does the egui-blocked remainder via a raw-winit
> intercept in `freminal-windowing` or an egui upgrade — isolated because it is
> architecture-touching input work with regression risk. v0.11.0 keyboard ships
> "substantially compliant, remainder tracked". Task 100 is bumped **Medium →
> Medium-high**: the 100.1 audit confirmed `t=s`, `o=z`, the relative-placement
> parser gap (`P/Q/H/V` were NOT parsed), delete-target correctness, and z-index
> ordering are all in scope on top of animation/placeholders/quotas.

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

## Execution model — branches & parallelism

Tasks 99, 100, and 101 are **largely independent in their primary code** (OSC
dispatch/notifications vs. graphics handler/store/renderer vs. keyboard input),
so they can run as three isolated workstreams. They are **not** independent in
three shared chokepoints the activation recon identified, and those drive the
branch model:

1. **`freminal-common` type crate** — the bottom of the dependency graph; every
   crate recompiles when it changes. Task 99 adds a `WindowManipulation::Notification99`
   variant + an `osc_99` config key; Task 100 adds `P/Q/H/V` parser fields to
   `KittyControlData`; Task 101 adds modifier fields to `KeyModifiers` (in the
   terminal-emulator crate). 110.0 lands all of these once.
2. **`KeyModifiers` + the GUI modifier-translation layer** — the 101.1 audit found
   `InputEvent::Key` carries only encoded bytes (modifiers are already encoded), so
   the modifier work is in `KeyModifiers` (`input.rs`, terminal-emulator) plus
   `egui_mods_to_key_modifiers` (GUI), **not** a new `InputEvent` field. 110.0 lands
   the `KeyModifiers` fields; 101.2 wires the GUI translation.
3. **`TerminalHandler` (`terminal_handler/mod.rs`)** — Task 99 adds a
   pending-notification map field; Task 101 touches the stack region; Task 100's
   handler dispatch is adjacent. New struct fields + `new()` initializers collide
   here.

### Branch topology (single v0.11.0 PR)

```text
main
 └── v0.11.0-kitty                 integration branch; the eventual single PR
      ├── task-99/osc99-notifications
      ├── task-100/graphics-completion
      └── task-101/keyboard-compliance
```

- Each task branches **off `v0.11.0-kitty`**, not off `main`, and merges **back
  into `v0.11.0-kitty`** as its subtasks complete.
- After each task branch merges, the other live task branches **rebase on
  `v0.11.0-kitty`** to pick up the shared changes before continuing.
- When all three are in, **one PR: `v0.11.0-kitty` → `main`** — the single v0.11.0
  merge.

### Execution order (audits parallel → foundation → staggered implementation)

1. **Audits in parallel.** The READ-ONLY audit subtasks (100.1, 101.1, and a 99
   design/seam pass) write no code and cannot collide — run them concurrently.
2. **Foundation first (110.0), on `v0.11.0-kitty` directly.** Land the shared
   `freminal-common` type _shells_ (and the `KeyModifiers` fields) once, before the feature
   branches fork, so no two branches race the same `freminal-common` edit. See
   110.0 below.
3. **Staggered implementation.** Feature branches fork from the
   foundation-carrying `v0.11.0-kitty`. Keep **at most one actively-editing agent
   per shared-file region at a time** (`terminal_handler/mod.rs`, `config.rs`);
   rebase each branch after every merge. Branches isolate the files (no
   corruption); staggering minimizes the merge-conflict + review-serialization
   tax, which is inherent to shared-file edits and not removed by branching.

Each subtask still stops at its review gate per `freminal-orchestrator-protocol`;
"parallel" means parallel _branches/workstreams_, not three agents editing the
same file at the same instant.

### 110.0 — Shared foundation (land first, on `v0.11.0-kitty`)

Scope: `freminal-common/src/buffer_states/window_manipulation.rs`
(a **new** `WindowManipulation::Notification99` variant + `NotificationKind`),
`freminal-common/src/config.rs` (`NotificationsConfig` + `ConfigPartial` +
`apply_partial`), `freminal-common/src/buffer_states/kitty_graphics.rs`
(`KittyControlData` + `apply_control_pair` — add the relative-placement keys),
`KeyModifiers` in `freminal-terminal-emulator/src/input.rs`.

What: land the **type shells only** that the feature branches would otherwise each
reach into `freminal-common` (and `input.rs`) to add — so the churn happens once,
up front, on the integration branch. Four independent, behaviour-free additions:

- **Notifications (type half of 99.4):** add a **new**
  `WindowManipulation::Notification99 { … }` variant (id, title, body, icon_data,
  icon_names, icon_cache_key, button_labels, actions, close_report, urgency,
  occasion, sound, app_name, notification_type, expire_ms). Do **not** modify the
  existing `Notification { kind, title, body }` variant — OSC 9/777 call sites keep
  using it unchanged. Extend the snapshot/`WindowCommand` transport for the new
  variant.
- **Config (config-type half of 99.8):** add `osc_99: bool` to
  `NotificationsConfig` (default true) with the **full** `ConfigPartial` /
  `apply_partial` round-trip per `freminal-config-options`. (The `NotificationsConfig`
  section is already merged atomically; add the field + default + a partial-merge
  test. Do NOT yet add the drain-site gate — that behaviour is 99.8.)
- **Graphics relative-placement keys (parser half of 100.4):** add four fields
  (`parent_image_id`, `parent_placement_id`, `h_offset`, `v_offset`) to
  `KittyControlData` and four arms (`P`/`Q`/`H`/`V`) to `apply_control_pair`. These
  are currently silently dropped by the `_ => {}` wildcard (100.1 audit). Parser
  only — no handler behaviour.
- **Keyboard modifier fields (data half of 101.2):** add `super_key`, `hyper`,
  `meta`, `caps_lock`, `num_lock` to `KeyModifiers`, defaulted false, and widen
  `modifier_param()`'s return past `u8` (max 1+255=256) to add all 8 bits —
  **without** populating the new bits from the GUI/winit layer (that is 101.2's
  behaviour half) and without changing any encoding. Existing 3-bit tests stay
  green because the new bits default false.

Deliverable: the four type/field additions + a snapshot round-trip test (new
variant) + a config partial-merge test + a `KittyControlData` parse test for
`P/Q/H/V` + a `modifier_param` test proving the new bits are inert when false;
**no behaviour change** (`cargo test --all` stays green). 99.4, 99.8, 100.4, and
101.2 then _consume/populate_ these shells rather than introducing them.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT add any behaviour (no GUI changes, no winit modifier capture,
no encoding changes, no dispatch, no config drain-site gate, no graphics handler
work); do NOT modify the existing `Notification` variant. This subtask is
types-only by design.

Stop: report + await review; then fork the three feature branches from
`v0.11.0-kitty`.

Note: 99.4, 99.8, 100.4, and 101.2 in the task sections below retain their full
descriptions, but with 110.0 landed they become "populate/extend the shell 110.0
added" rather than "introduce the type". Re-scope their file lists at execution
time to exclude the shared-type edits 110.0 already made.

#### 110.0 execution decisions (recorded 2026-07-01, at execution against the real seams)

Recon of the four seams confirmed the plan and resolved the open shape choices.
These are the Opus decisions the implementer must follow (they are not open
questions):

- **`Notification99` field types are primitive shells, refined later.** The
  variant carries `id: Option<String>`, `title: Option<String>`,
  `body: Option<String>`, `icon_data: Option<Vec<u8>>`, `icon_names: Vec<String>`,
  `icon_cache_key: Option<String>`, `button_labels: Vec<String>`,
  `report_activation: bool` (the `a=report` flag), `close_report: bool` (the `c=1`
  flag), `urgency: Option<u8>` (0/1/2), `occasion: Option<String>`,
  `sound: Option<String>`, `app_name: Option<String>`,
  `notification_type: Vec<String>`, `expire_ms: Option<i64>`. Domain enums for
  urgency/occasion/action are **owned by 99.1's typed parser** in
  `freminal-common`; 110.0 does **not** invent a competing enum family, and 99.4
  refines these primitive fields to the parser's typed enums when it wires the
  real path. `NotificationKind` is **not** modified (it stays OSC 9/777-only).
- **Transport is the `WindowCommand` channel, not the snapshot.** Per
  `KITTY_PROTOCOL_REFERENCE.md`, the new variant travels
  `terminal_handler` → `window_commands` → `pty.rs` classification
  (`WindowCommand::Viewport`/`Report`) → GUI `rendering.rs`. The 110.0
  deliverable's "snapshot round-trip test" wording is superseded: the test is a
  **construct + clone + pattern-match** unit test on the `Notification99` variant
  (and, if cheap, a `WindowCommand::Report(Notification99{…})` wrap/unwrap
  assertion). No `TerminalSnapshot` field is added.
- **One unavoidable inert GUI arm.** `freminal/src/gui/rendering.rs`
  `handle_window_manipulation` is the only **exhaustive** match on
  `WindowManipulation` (no wildcard). Adding the variant forces a new arm there or
  the workspace will not compile. 110.0 adds a **behaviour-free placeholder** arm
  (log at `trace!`, drop the command, comment pointing to 99.5) — this is the
  minimum to keep `cargo test --all` green and is **not** OSC 99 GUI behaviour
  (that is 99.5). `pty.rs` needs no change: its `_ => WindowCommand::Viewport(cmd)`
  wildcard already absorbs the new variant (99.x will reclassify it to `Report`).
- **Config `osc_99` is wired end-to-end as a _loadable_ option now; the routing
  gate stays in 99.8.** Because `NotificationsConfig` already merges atomically
  through `ConfigPartial`/`apply_partial`, the new field rides the existing section
  merge (no new `apply_partial` arm). "Full `freminal-config-options` wiring" for
  a _loadable/persistable_ option means, in addition to the field + `Default`:
  `config_example.toml` (`[notifications]` block), the Nix home-manager module
  mirror (`nix/home-manager-module.nix`: add `osc_99` to the
  `notificationsSection` `inherit` list **and** an `osc_99` `mkOption`), the
  Settings-UI toggle (`freminal/src/gui/settings.rs`, mirroring the `osc_9`/
  `osc_777` checkboxes), the round-trip test (`notifications_round_trip_through_toml`),
  and a dedicated partial-merge test (mirror the existing `osc_9 = false` partial
  test). The **drain-site behaviour gate** (actually consulting `osc_99` when
  routing an OSC 99 notification) is explicitly **out of 110.0** and lands in 99.8.
- **`modifier_param()` widens `Option<u8>` → `Option<u16>`.** Max is
  `1 + 255 = 256`, which overflows `u8`. All ~15 callers are inside
  `input.rs`; the four format helpers (`modified_csi_final`, `modified_csi_tilde`,
  `kkp_csi_final_event`, `kkp_csi_tilde_event`) and `build_csi_u`'s
  `modifier_param: Option<u8>` parameter widen to `u16` in tandem so no `as` cast
  is introduced (values flow only into `format!` display). New `KeyModifiers`
  bits (`super_key`=8, `hyper`=16, `meta`=32, `caps_lock`=64, `num_lock`=128)
  default false and are added to `is_empty()` and the `NONE` constant; no bit is
  populated from any input source in 110.0.
- **Scope expansion (recorded).** The literal 110.0 Scope line named only the four
  code files; this execution adds the config-companion files above
  (`config_example.toml`, `nix/home-manager-module.nix`,
  `freminal/src/gui/settings.rs`) and the one inert GUI arm
  (`freminal/src/gui/rendering.rs`), all **required** by the plan's own
  `freminal-config-options` reference and by the exhaustive-match compile
  constraint. No behaviour is added.

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
behaviour (if any). Produce the precise gap list that 100.2–100.8 implement. Reconcile
one known recon error: an early sub-agent summary claimed relative placements were "a
separate CSI extension, out of scope" — confirm from the code that `P`/`Q`/`H`/`V` are
already typed in `KittyControlData` and that relative placements are APC graphics
commands handled by 100.4 (they are in scope).

Deliverable: a findings report (in chat / appended to this task's notes), not code.

Verification: none (read-only).

Prohibitions: do NOT edit any files; do NOT proceed to implementation.

Stop: report findings; await review and scoping confirmation of 100.2–100.8.

#### 100.2 — Animation: frame transfer + control + compose

Scope: `terminal_handler/graphics_kitty.rs`, `freminal-buffer/src/image_store.rs`
(frame storage), `freminal/src/gui/view_state.rs` + the renderer image path (a
GUI-side frame selector), `freminal-common/src/buffer_states/kitty_graphics.rs`
(response-format change).

What: implement `a=f` (frame transfer, partial-frame rects, composition background
`c=`/`Y=`, blend mode `X=`, edit `r=`, gap `z=`), `a=a` (control: current frame `c=`,
stop/run/loop `s=` with s=1 stop / s=2 loading / s=3 run, loop count `v=` where
v=N plays N-1 loops, per-frame gap `r=`/`z=`), `a=c` (compose, with `ENOENT`/
`EINVAL`/`ENOSPC` errors). **The 100.1 audit found there is no frame model and no
image-animation tick today** — `InlineImage` is a single flat buffer, and the only
animation infra is the unrelated cursor-trail. So this subtask must: (a) add a
frame list (per-frame pixels + gap-ms) to `InlineImage` or a new animated-image
type in `image_store.rs`; (b) add a GUI-side wall-clock frame selector in
`ViewState` (the snapshot ships all frames; the GUI picks the current frame by
elapsed time — do NOT put frame-index state in the snapshot). ACK/NACK via
`format_kitty_response`, respecting `q=`. **Key aliasing (critical, from 100.1):**
the parser stores animation keys under transmit/display-named fields (`s`→
`src_width`, `v`→`src_height`, `c`→`display_cols`, `r`→`display_rows`, `z`→
`z_index`, `X`→`cell_x_offset`, `Y`→`cell_y_offset`); the handler MUST re-interpret
them by action (see the aliasing table in `KITTY_PROTOCOL_REFERENCE.md`). **Also
close the response-`p=` gap here:** `format_kitty_response(image_id, ok, message)`
emits only `i=<id>`; add a `placement_id: Option<u32>` param that appends
`,p=<pid>` when non-zero, and thread it through the `send_kitty_error` helper and
the 5 call sites (2 query sites pass `None`; the put/place sites pass
`control.placement_id`).

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
`P=`/`Q=`/`H`/`V`) — not a separate CSI extension. The 100.1 audit found the parser
does **not** currently type `P`/`Q`/`H`/`V` (they hit the `_ => {}` wildcard and are
dropped); **foundation subtask 110.0 adds those 4 parser fields + arms**, so this
subtask is handler/store work only — read the fields 110.0 added and act on them.
Implement `P=`/`Q=` (parent image/placement) with optional `H`/`V` cell offsets;
lifecycle tied to parent (cascade delete); the cursor must not move after a
relative placement regardless of `C`; a virtual placement may be a parent but
cannot itself be made relative (`EINVAL`); chain depth limit (`ETOODEEP` past ≥8);
cycle rejection (`ECYCLE`); missing parent (`ENOPARENT`). Error responses via the
reverse path. `image_store.rs` has no placement-group concept today — add a
placement registry keyed by `(image_id, placement_id)` with a `parent` link and
child-list/group semantics for cascade delete (the existing `virtual_placements`
map is the closest analog but has no parent link).

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
`freminal-terminal-emulator/Cargo.toml` (add the `"mman"` feature to the existing
`nix` dep for POSIX shared memory), the workspace `Cargo.toml` (add a zlib crate —
none exists today). Per `flake-dev-shell-discipline` + `rust-best-practices`, add
any new dependency and stop for `nix develop` before use.

What: two independent-but-related gaps the parser already types but the handler
does not honour (both confirmed unimplemented by the 100.1 audit).

- `o=z`: when `control.compression == Some(Zlib)`, RFC 1950 zlib-inflate the
  (already-base64-decoded) payload before it is interpreted as raw pixels or PNG.
  Applies to every `f=` format. With PNG + compression the client supplies `S=`
  (source size). Currently `o=z` is parsed and never read anywhere in
  `graphics_kitty.rs` — the compressed bytes are stored as-is (garbage). **No
  `flate2`/`miniz_oxide` is in the workspace** — add one (`flate2` is the RFC 1950
  wrapper) as a workspace dependency.
- `t=s`: replace the current `ENOTSUP` stub (`resolve_kitty_transmission`) with an
  actual shared-memory read — open the named object from the payload, read `S`
  bytes at offset `O`, then `shm_unlink` + `close` (POSIX) / `close` (Windows).
  The `nix` dep is already present but lacks the `"mman"` feature that provides
  `shm_open`/`shm_unlink`; add it. Windows uses `winapi` (already a workspace dep).
  Enforce the spec's special-file / sensitive-path refusals (mirror the existing
  `t=f`/`t=t` security checks).

Deliverable: both handlers + tests (zlib round-trip decode for RGB/RGBA/PNG; a
shared-memory read that asserts the object is unlinked after read; the security
refusals).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT touch animation, placeholders, or relative placements; do
NOT weaken the file/medium security checks; do NOT proceed.

Stop: report + await review.

#### 100.7 — Delete-target correctness + z-index render order

Scope: `freminal-common/src/buffer_states/kitty_graphics.rs` (`KittyDeleteTarget`
enum + `parse_delete_target`), `freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs`
(`handle_kitty_delete`), `freminal/src/gui/renderer/vertex.rs` + `.../gpu.rs`
(image quad ordering).

What: close the delete-target and stacking gaps the 100.1 audit enumerated.

- **Lowercase vs uppercase data-freeing:** `d=i` (and `d=n`) must delete placements
  but **keep** the stored image data; only the uppercase forms (`d=I`, `d=N`, `d=A`)
  free image data. Today lowercase `d=i` also removes the image from the store.
- **Visible vs all:** `d=a` deletes visible placements only; `d=A` deletes all
  including non-visible/scrollback. Today both clear the whole store.
- **"And-after" variants:** `d=X`/`d=Y`/`d=Z` (column/row/z-index "and after")
  currently collapse to their non-after counterparts — implement the rightward/
  downward/higher-z semantics.
- **Missing enum variants:** add `d=f`/`d=F` (delete animation frames — pairs with
  100.2) and `d=r`/`d=R` (delete images with id in `[x, y]`, kitty 0.33.0); both
  currently return `UnknownDeleteTarget` and are ignored.
- **Z-index render order:** `build_image_verts` / `draw_images` sort quads by
  `image_id`, not `z_index`; higher z must render above lower z. Sort by z-index
  (then id for stability).

Deliverable: corrected delete handling + z-ordered rendering + tests (lowercase
keeps data, uppercase frees, visible-vs-all, and-after semantics, `d=f`/`d=r`,
a two-image z-order assertion).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT change transmit/put/query parsing beyond adding the two
delete-target variants; do NOT proceed.

Stop: report + await review.

#### 100.8 — Escape-sequence docs

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`,
`Documents/KITTY_PROTOCOL_REFERENCE.md`.

What: update the graphics rows to reflect animation / placeholders / relative
placements / quotas / `t=s` / `o=z` / `p=`-in-responses / delete-correctness /
z-order now implemented; refresh the "Last updated" header. Also flip the graphics
"current-state deltas" section in `KITTY_PROTOCOL_REFERENCE.md` from gap-list to
done, and bump its `Distilled ... as of` date if any spec detail was reconfirmed.

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
functional-key event-type defect. The 2026-07-01 audit (101.1, now complete) found
freminal is materially short of 100% compliance — but, critically, that **the
binding constraint is egui 0.35 (via egui-winit), not freminal's encoding layer**:

1. **Modifiers.** `KeyModifiers` models only 3 of 8 bits (shift=1, alt=2, ctrl=4);
   `egui_mods_to_key_modifiers` drops the rest, and egui's `Modifiers` struct has
   no super/hyper/meta/caps_lock/num_lock at all.
2. **Functional keys.** Missing `CSI u` encodings for keypad keys, media keys,
   modifier-keys-as-keys, F13–F35, and lock/print/pause/menu keys. Some of these
   egui does not even deliver to freminal (they are absent from egui's `Key` enum).

Per the 2026-07-01 activation decision, the work is split by the egui boundary:

- **Task 101 (this task) does the encoding-only wins** achievable without touching
  the windowing layer: the 8-bit modifier arithmetic + `super` via SuperLeft/Right
  press-tracking (101.2), and the F13–F35 + modifier-keys-as-keys encodings (101.3
  — egui _does_ deliver `Key::*Left/*Right`).
- **The egui-blocked remainder becomes a separate task (see MASTER_PLAN):** keypad
  operators/directional keys, media keys, ISO-level shifts, CapsLock/ScrollLock/
  NumLock/PrintScreen/Pause/Menu, and true caps_lock/num_lock modifier state — all
  require a raw-winit intercept in `freminal-windowing` (bypassing egui-winit's key
  translation) or an egui/egui-winit upgrade. That is architecture-touching input
  work with regression risk across the whole input path, so it is isolated.

v0.11.0 therefore ships keyboard as **substantially compliant with the egui-blocked
gaps tracked**, not literally 100%. The stack semantics, set/push/pop, `CSI ? u`
query, XTGETTCAP `u`, and separate main/alt-screen stacks are conformant and must
not be rebuilt. The full spec surface is in `Documents/KITTY_PROTOCOL_REFERENCE.md`.

### 101 Subtasks

#### 101.1 — READ-ONLY conformance audit (COMPLETE, 2026-07-01)

Done. The audit produced the code-anchored gap list now reflected in the keyboard
"current-state deltas" section of `KITTY_PROTOCOL_REFERENCE.md` and in the 101.2/
101.3 scopes below. Key finding: the split between encoding-only work (101.2/101.3)
and egui-blocked work (separate task) — see Summary. No further audit needed.

#### 101.2 — Modifier bits: 8-bit arithmetic + `super` (encoding-only)

Scope: `KeyModifiers` + `modifier_param()` in
`freminal-terminal-emulator/src/input.rs` (the field/arithmetic half is landed by
110.0), and `egui_mods_to_key_modifiers` + the GUI key-event loop in
`freminal/src/gui/terminal/input.rs` (to source `super`). Do **not** add a raw
winit intercept here.

What: with 110.0 having added the 5 modifier fields to `KeyModifiers` and widened
`modifier_param()`, this subtask **populates** the bits that egui _can_ supply:

- `super` (bit 8): egui's `Modifiers` has no super flag on Linux/Windows, but
  egui delivers `Key::SuperLeft`/`Key::SuperRight` press/release events. Track
  their pressed state in the GUI key-event loop and set `KeyModifiers.super_key`.
  (On macOS, `Modifiers::command`/`mac_cmd` already carries it — currently merged
  into `ctrl`; split it out to `super_key`.)
- `hyper`/`meta` (bits 16/32): no platform path via egui — document and leave
  emitting `0`.
- `caps_lock`/`num_lock` (bits 64/128): **egui-blocked** — no egui API for the lock
  state. Leave to the separate egui-blocked task. Do NOT attempt a winit intercept
  here.

Honour the flag-1 carve-out (lock modifiers not reported for text keys unless flag
8). The `modifier_param()` arithmetic for all 8 bits is already in place from 110.0;
this subtask just feeds it the `super` bit truthfully.

Deliverable: `super` populated end to end + tests (super-modified key produces the
correct `1+bitmask`; macOS command→super mapping; hyper/meta/lock remain 0 with a
documented reason).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT add a raw-winit intercept; do NOT attempt caps_lock/num_lock;
do NOT add functional-key encodings (101.3); do NOT alter the stack/query code; do
NOT proceed.

Stop: report + await review.

#### 101.3 — Encoding-only functional keys: F13–F35 + modifier-keys-as-keys

Scope: `freminal-terminal-emulator/src/input.rs` (`to_payload_kkp`, the
`FunctionKey` encoder), the `TerminalInput` enum, and the GUI key-event loop in
`freminal/src/gui/terminal/input.rs` (to route the modifier-key presses egui
already delivers). Depends on 101.2 merged.

What: add only the encodings that do **not** need windowing-layer work, because
egui already delivers these keys:

- **F13–F35** (`CSI 57376 u` … `CSI 57398 u`): `FunctionKey(u8)` currently drops
  n>12 (`function_key_unknown_returns_empty`); add arms for 13..=35.
- **Modifier-keys-as-keys** ShiftLeft/Right, ControlLeft/Right, AltLeft/Right,
  SuperLeft/Right (`CSI 57441 u` … `CSI 57452 u`, under flag 8): egui delivers
  these as `Key::ShiftLeft` etc., but the event loop has no arm for them. Add
  `TerminalInput` variants + event-loop arms + `to_payload_kkp` branches, emitted
  only when flag 8 is set.
- **F3 normalization:** confirm/normalize F3 to `13 ~` under KKP (it is currently
  `ESC O R` SS3 — neither the prohibited `CSI R` nor the spec's `13 ~`).

Everything else from the functional-key table (keypad operators/directional, media
keys, ISO-level shifts, lock/print/pause/menu) is **egui-blocked** and belongs to
the separate task — do NOT attempt it here.

Deliverable: F13–F35 + modifier-key encodings + F3 normalization + tests (a case
per key class, with/without modifiers, modifier-keys only under flag 8).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT add keypad/media/ISO/lock/print/pause/menu keys (egui-blocked
task); do NOT add a raw-winit intercept; do NOT change stack/query behaviour; do
NOT proceed.

Stop: report + await review.

#### 101.4 — Escape-sequence docs

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`,
`Documents/KITTY_PROTOCOL_REFERENCE.md`.

What: record the encoding-only compliance work (super modifier + F13–F35 +
modifier-keys-as-keys + F3 normalization); refresh the "Last updated" header. In
`KITTY_PROTOCOL_REFERENCE.md`, mark the encoding-only items done and leave the
egui-blocked items flagged as pending the separate task (with a pointer to it in
MASTER_PLAN). State the split explicitly so a future agent does not re-audit.

Deliverable: dual-doc update (plus reference-doc refresh).

Verification: markdownlint clean (`markdownlint-cli2`), prettier clean.

Prohibitions: none beyond scope.

Stop: report + await review.

### 101 Open questions (resolved at activation, 2026-07-01)

- **Split by the egui boundary.** The 101.1 audit found the binding constraint is
  egui 0.35, not freminal's encoding. Task 101 does the encoding-only wins (super
  modifier, F13–F35, modifier-keys-as-keys); the egui-blocked remainder (keypad/
  media/ISO/lock/print/pause/menu keys + true caps_lock/num_lock state) is a
  separate MASTER_PLAN task needing a raw-winit intercept or egui upgrade. This is
  a deliberate scope cut, not a gap left unnoticed — v0.11.0 keyboard is
  "substantially compliant, remainder tracked".

---

## Task 114 — Kitty Keyboard: egui-blocked keys (windowing layer) [STUB]

### 114 Summary

The 101.1 audit found that a set of kitty keyboard keys and modifiers cannot be
delivered to freminal at all because **egui 0.35 (via egui-winit) does not expose
them**. Task 101 handles everything achievable without touching the windowing
layer; this task handles the remainder, which requires either a **raw-winit
`KeyboardInput` / `ModifiersChanged` intercept** in `freminal-windowing` (before
egui-winit's translation) or an **egui/egui-winit upgrade** that surfaces these
keys. It is isolated from Task 101 because it is architecture-touching input work
with regression risk across the entire keyboard path.

This is an **enriched stub** per the `freminal-version-activation` skill: the
durable findings and decisions are captured; subtask decomposition happens at this
task's own activation, against the code as it then exists (and against whatever
egui version is then in use).

### 114 In scope (the egui-blocked remainder)

- **True `caps_lock` / `num_lock` modifier state** (bits 64/128): no egui API;
  needs raw winit `ModifiersChanged` or lock-key press tracking.
- **Keypad operators and directional keys:** KP_Divide, KP_Multiply, KP_Subtract,
  KP_Add, KP_Enter, KP_Equal, KP_Separator, KP_Left/Right/Up/Down,
  KP_PageUp/PageDown, KP_Home/End, KP_Insert/Delete, KP_Begin (57410–57427). Absent
  from egui's `Key` enum. (Numpad digits are unified with main-row digits by
  egui#3653 — distinguishing them also needs winit-level physical-key info.)
- **Media keys** (57428–57440): absent from egui's `Key` enum.
- **ISO_Level3_Shift / ISO_Level5_Shift** (57453/57454): absent from egui.
- **CapsLock / ScrollLock / NumLock / PrintScreen / Pause / Menu as keys**
  (57358–57363): absent from egui's `Key` enum.

Once these keys reach freminal, their `CSI u` encodings follow the same
functional-key table in `KITTY_PROTOCOL_REFERENCE.md` that Task 101 uses — the hard
part is delivery, not encoding.

### 114 Durable decisions (captured at v0.11.0 activation)

- **Do not balloon Task 101.** The encoding-only and delivery-blocked work are cut
  along the egui boundary deliberately; keep them separate PRs/branches.
- **Prefer the least-invasive delivery mechanism.** A targeted raw-winit intercept
  for just the missing keys is likely safer than a full egui upgrade, but the
  choice is deferred to this task's activation (an egui upgrade may by then be
  desirable for other reasons). Either way it is architecture-affecting — invoke
  `freminal-architecture` and get sign-off before rewiring the input path.
- **This is why v0.11.0 keyboard is "substantially compliant, not 100%".** The gap
  is explicit and tracked, not silent.

### 114 Open questions (decide at activation)

- Raw-winit intercept vs egui/egui-winit upgrade — which, and what is the
  regression surface for the existing input path?
- Can the intercept be scoped to only the missing keys, leaving egui-winit as the
  primary translator for everything else, to minimize risk?
- Are `hyper`/`meta` modifiers ever reachable on any target platform, or do they
  stay permanently `0`?

---

## Design Decisions

Provisional decisions are marked; the rest were confirmed at the 2026-07-01
activation.

- **v0.11.0 ships full kitty notifications & graphics; keyboard is substantially
  compliant with the egui-blocked remainder tracked (Task 114).** Notifications and
  graphics are finished to spec this version. Keyboard's 100% is gated on egui
  delivering keys it currently drops — the encoding-achievable part ships in Task
  101; the delivery-blocked part is the explicit, tracked Task 114.
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
  sub-agents.
- **Single-PR branch model (2026-07-01 decision).** Work happens on an integration
  branch `v0.11.0-kitty` off `main`; a shared-foundation subtask (110.0) lands the
  `freminal-common` type shells there first; the three task
  branches (`task-99/…`, `task-100/…`, `task-101/…`) fork from it, merge back into
  it, and rebase on it after each merge; the whole version ships as one PR
  `v0.11.0-kitty → main` (Task 114, if done this cycle, is a separate branch/PR).
  Audits run in parallel; implementation is staggered with at most one active
  editor per shared-file region. See "Execution model" above for the full rationale
  (the collision surface is `freminal-common`, `KeyModifiers`, and
  `terminal_handler/mod.rs`).
- **Activation decisions (2026-07-01):**
  - OSC 99 icon-data cache (`g=`) is **in-memory, session-lifetime** only.
  - macOS/untracked-close: emit the `untracked` close form and implement the
    `p=alive` polling response (spec mandate).
  - OSC 99 display gating: `o=` occasion is the primary gate; `[notifications]`
    keeps a wired on/off `osc_99` kill-switch (`freminal-config-options`).
  - Graphics `t=s` (shared memory) and `o=z` (zlib) are **both in scope**
    (subtask 100.6).
  - **Graphics relative-placement parser keys (`P/Q/H/V`) were NOT parsed** (100.1
    audit corrected an earlier claim); their 4 fields + parser arms are added in
    foundation subtask 110.0, so 100.4 is handler/store-only.
  - **Keyboard split by the egui boundary:** Task 101 does the encoding-only wins
    (super modifier via SuperLeft/Right; F13–F35; modifier-keys-as-keys); the
    egui-blocked remainder (keypad operators/directional, media, ISO-level shifts,
    lock/print/pause/menu keys, true caps_lock/num_lock) is new **Task 114** needing
    a raw-winit intercept or egui upgrade.
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
