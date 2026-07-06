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

| #   | Feature                                   | Scope       | Status   | Depends On       |
| --- | ----------------------------------------- | ----------- | -------- | ---------------- |
| 99  | Kitty Desktop Notifications (OSC 99)      | Medium-high | Planned  | v0.9.0 (Task 76) |
| 100 | Kitty Graphics Protocol Completion        | Medium-high | Planned  | Task 13          |
| 101 | Kitty Keyboard Compliance (encoding-only) | Medium      | Complete | Task 35          |
| 114 | Kitty Keyboard: egui-blocked keys (winit) | Medium-high | Active   | Task 101         |

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

##### 99.4 execution decisions (recorded 2026-07-01, against the real seams)

A recon of the mapping seam confirmed 110.0's `Notification99Data` shell is the
target and the `WindowCommand` channel is the transport. Resolutions (Opus
decisions, not open questions):

- **No `snapshot.rs` change.** Transport is `window_commands` →
  `WindowCommand::Viewport/Report` → GUI, fully generic over
  `WindowManipulation`; `snapshot.rs` never carries `WindowManipulation`. The
  literal 99.4 Scope line naming `snapshot.rs` is superseded by the 110.0
  "execution decisions" note (transport is the `WindowCommand` channel).
- **No `pty.rs` reclassification in 99.4.** `Notification99` rides the
  `_ => Viewport` wildcard, which reaches the GUI (both wrappers unwrap
  identically). Reclassifying to `Report` for the reverse-write path is 99.6.
- **`focus_on_activation` gets a home: extend the shell.** `Osc99Actions` carries
  both `report_activation` and `focus_on_activation`, but 110.0's
  `Notification99Data` only had `report_activation`. Silently dropping `a=focus`
  is an observable-behaviour loss, so **99.4 adds `focus_on_activation: bool` to
  `Notification99Data`** (completing the OSC 99 superset this subtask is meant to
  carry). This is the one `window_manipulation.rs` type change in 99.4.
- **Field mapping `FinalizedNotification`/`Osc99Command` → `Notification99Data`:**
  `id`/`title`/`body` direct; `icon_data ← finalized.icon`;
  `icon_names`/`icon_cache_key`/`sound`/`app_name`/`notification_type` from
  `meta`; `report_activation`/`focus_on_activation ← meta.actions.*`;
  `close_report ← meta.close_report`;
  `urgency`: `NotificationUrgency` → `Option<u8>` (`Low`→`Some(0)`,
  `Normal`→`Some(1)`, `Critical`→`Some(2)`, `None`→`None`);
  `occasion`: `NotificationOccasion` → `Option<String>` with **`Always → None`**
  (the default; behaviourally identical to unset), `Unfocused →
Some("unfocused")`, `Invisible → Some("invisible")`;
  `expire_ms`: `i64` → `Option<i64>` with **`-1 → None`** (the spec's "OS
  default" sentinel), any other value → `Some(v)`.
- **`button_labels` is `Vec::new()` for now (tracked, not silently dropped).**
  Button-label extraction from the `p=buttons` (U+2028-separated) payload is not
  implemented anywhere in the 99.1 parser / 99.3 reassembler yet, so 99.4 has no
  source. See cleanup entry **99.9** below.

##### 99.9 — Cleanup: OSC 99 button-label extraction (surfaced during 99.4)

- **Surface point:** 99.4 mapping recon (2026-07-01), on `task-99/osc99-notifications`.
- **Impact:** OSC 99 `p=buttons` payloads (U+2028-separated labels) are parsed to
  the `Buttons` payload type but the individual labels are never extracted, so
  `Notification99Data.button_labels` is always empty and the GUI (99.5) cannot
  render notification buttons or map a button-activation index back (99.6).
- **Scope of fix:** accumulate `p=buttons` payload in `FinalizedNotification`
  (99.3's reassembler) and split it on U+2028 into `Vec<String>`; populate
  `Notification99Data.button_labels` in the 99.4 mapping. Purely additive.
- **Suggested approach:** add a `buttons: Vec<u8>` accumulator to
  `PendingNotification` + a `buttons: Vec<String>` field to
  `FinalizedNotification` (split on U+2028 at finalize); map it in 99.4.
- **Verification:** a reassembly test feeding a `p=buttons` chunk asserts the
  split labels; a 99.4 mapping test asserts `button_labels` is populated.
- **Scheduling:** do before or with 99.5 (the GUI consumes buttons). Not a
  blocker for the 99.4 field-mapping commit.

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

##### 99.5 execution decisions (recorded 2026-07-01, against the real seams)

A recon of the GUI notification machinery found 99.5 is larger than one
Sonnet-sized subtask and crosses shared-state / dependency / platform lines that
need explicit resolution. Maintainer-approved decisions:

- **Split into 99.5a and 99.5b.**
  - **99.5a — render leg + state:** keep `NotificationRouter` the stateless unit
    type it is (additive — do NOT rewire the existing `route`/`route_test` call
    sites); add a live-notification-by-id map and a session `g=` icon-data cache
    (`HashMap<String, Vec<u8>>`) as fields on `FreminalGui` (app-level, `RefCell`,
    alongside `toasts` — matching that precedent); add a `route_osc99` associated
    function taking a `Notification99Data` plus the maps by `&mut`. The OSC 99
    command is collected into an out-param `Vec` in `rendering.rs` (mirroring the
    OSC 9/777 `NotificationRequest` collection) and routed after the pane loop in
    `app_impl.rs` where `self.config`, `self.toasts`, and the maps are borrowable.
    Render the toast leg + the OS-notification leg with
    buttons/urgency/sound/expiry/icon-by-name (`.icon()`) / icon-by-data (temp
    file + `.image_path()`) and the occasion gate. **No handle retention yet**
    (still fire-and-forget in 99.5a); no reverse-write.
  - **99.5b — handle retention:** change the OSC 99 OS-notification path so the
    spawned per-notification thread **keeps** the `notify-rust` handle and calls
    `wait_for_action` / `on_close`, turning observed activation/close into events
    delivered back toward the PTY. 99.5b pairs with 99.6 (the reverse-write that
    consumes those events). Written per `freminal-architecture`; no new
    steady-state GUI↔PTY shared lock.
- **Handle retention = per-notification thread keeps the handle** (not a
  synchronous GUI-thread `.show()`). Preserves Task 76's off-thread D-Bus model.
  The Linux handle is `Send` (zbus `Connection`); macOS/Windows handle types
  differ — 99.5b keeps the retention logic behind the same `#[cfg]` structure
  `show_system` already uses, and only assumes the common
  `wait_for_action`/`close`/`on_close` shape.
- **Icon-by-data = temp file + `notify-rust` `.image_path()`** (no Cargo feature
  change, no new system dep). `.image_data()` is behind the unset
  `images_no_default_features` feature; the temp-file path avoids touching the
  dependency. Icon-by-name uses `.icon()`. The `g=` cache stores the transmitted
  bytes for the session so later `g=`-only notifications reuse them.
- **`expire_ms` mapping:** `None` → `notify-rust` `Timeout::Default`; `Some(0)` →
  `Timeout::Never` (kitty `w=0` = "never expire", which matches `notify-rust`'s
  `Timeout::from(0)` = Never); `Some(n>0)` → `Timeout::Milliseconds(n)`.
- **Occasion gate:** `None`(=Always) → always display; `unfocused` → gate on
  `!flags.window_focused` (already threaded); `invisible` → gate on window
  minimized (`ui.ctx().input(|i| i.viewport().minimized)`, already reachable in
  `handle_window_manipulation`) OR the imperfect `!flags.is_active` background
  proxy — 99.5a uses minimized as the honest signal and treats background-tab
  occlusion as out of scope (documented, not silently dropped).
- **Truthful-advertisement gaps to carry into 99.7:** `sound` is only a
  freedesktop hint (no guaranteed playback); `notification_type` has no clean
  `notify-rust` mapping; on macOS there is no urgency setter (matches the
  existing Task 76 `#[cfg(not(target_os = "macos"))]` gate). 99.7 must not
  over-advertise these.

##### Reverse-path gaps found during 99.5b/99.6 recon (2026-07-01)

The recon of the reverse-write path found two correctness gaps and a lifecycle
gap that reshape the reverse-path work. Maintainer-approved resolutions:

- **Gap 1 — control payload types were misrouted (bug).**
  `FinalizedNotification::into_notification99_data()` discards
  `Osc99PayloadType`, so `p=close` (close request), `p=alive` (liveness poll),
  and `p=?` (capability query) currently reach the GUI as empty `Notification99`
  display requests and get wrongly toasted / OS-notified. **Resolution: a new
  `WindowManipulation::Osc99Control { id: Option<String>, kind: Osc99ControlKind }`
  variant** (with `enum Osc99ControlKind { Close, Alive, Query }` in
  `freminal-common`), emitted by the `handle_osc` `Notify99` arm in
  `terminal_handler/osc.rs` when `finalized.meta.payload_type` is
  `Close`/`Alive`/`Query`; the display types (`Title`/`Body`/`Icon`/`Buttons`)
  keep flowing as `Notification99`. Display stays clean; control routes to
  distinct GUI handling (close-request/alive-response in 99.6, query in 99.7).
- **Gap 2 — pane identity is lost.** The reverse report must reach the
  **originating pane's** `pty_write_tx`, but `Notification99Data` carries no pane
  id and 99.5a's post-loop routing site has no pane context. `pty_write_tx` IS in
  scope at the per-pane `rendering.rs` `Notification99`/`Osc99Control` arm.
  **Resolution: clone `pty_write_tx` at the rendering site and carry the clone
  alongside each collected item** — the OSC 99 out-param becomes
  `Vec<(Notification99Data, Sender<PtyWrite>)>` (and a parallel one for
  `Osc99Control`), so the post-loop router writes reports back to the correct
  pane. Cloning the existing `Sender<PtyWrite>` (already `Send + Clone`, precedent
  in `layout_ops.rs`) satisfies the "no new channel" rule.
- **Gap 3 — `osc99_live` never pruned.** Entries are only inserted (99.5a),
  never removed, so the map grows unbounded and a `p=alive` response would over-
  report. **Resolution: remove the entry on observed close** (99.5b/99.6) and on
  an app-driven `p=close`.

##### 99.5c — Reverse-path prerequisites (control variant + pane-tx + prune scaffolding)

Scope: `freminal-common/src/buffer_states/window_manipulation.rs`
(`Osc99Control` variant + `Osc99ControlKind`), `freminal-common` `osc_notify_99.rs`
(expose `payload_type` on the finalized path if needed),
`freminal-terminal-emulator/src/terminal_handler/osc.rs` (branch on
`payload_type`; emit `Notification99` vs `Osc99Control`),
`freminal-terminal-emulator/src/terminal_handler/notify_99.rs`
(carry `payload_type` to the emit site),
`freminal/src/gui/rendering.rs` (clone `pty_write_tx` into the OSC 99 out-param
tuples; add an inert `Osc99Control` arm), `freminal/src/gui/app_impl.rs`
(thread the tupled out-params), `freminal/src/gui/pty.rs` (classify
`Notification99` and `Osc99Control` as `Report`, not `Viewport`, since they now
drive reverse-writes).

What: land the type/routing shells so 99.5b/99.6 can be a focused behaviour
change. **No handle retention, no report bytes yet** — the `Osc99Control` GUI arm
and the tupled `pty_write_tx` are inert placeholders (routing + plumbing only).
Add the `pty.rs` `Report` reclassification here (it is a pure classification
change; both wrappers unwrap identically GUI-side, so it is behaviour-neutral
until 99.6 uses the write path). Prune-on-close scaffolding: add a `remove`-capable
path for `osc99_live` (used by 99.6).

Deliverable: the variant + routing split + pane-tx threading + tests (a
`p=close`/`p=alive`/`p=?` sequence produces `Osc99Control{kind}` not
`Notification99`; a display sequence still produces `Notification99`).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT retain the notify-rust handle or write report bytes (99.5b/99.6);
do NOT change display rendering (99.5a).

Stop: report + await review.

##### 99.5b + 99.6 (combined) — handle retention + reverse reports

Per the recon, 99.5b (retain the handle, produce `(id, action-string)` events) and
99.6 (turn events into report bytes on the originating pane's `pty_write_tx`) are
structurally inseparable and land as **one reviewed change** on top of 99.5c: the
per-notification thread keeps the handle, calls `wait_for_action` (Linux; covers
both activation `"default"`/button-index and close `"__closed"`), and — using the
`pty_write_tx` clone threaded in 99.5c — writes `ESC ] 99 ; i=<id> ; <btn> ST`
(activation, when `a=report`), `ESC ] 99 ; i=<id> : p=close ; ST` (close, when
`c=1`), pruning `osc99_live` on close. The `Osc99Control{Alive}` arm answers the
`p=alive` poll with `ESC ] 99 ; i=<id> : p=alive ; <live-ids> ST`. macOS/Windows:
`untracked` close form + `p=alive` (main-run-loop hazard confirmed in recon).

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

##### 99.8 execution decisions (recorded 2026-07-01)

- **`osc_99` field + full config wiring already landed in 110.0** (field +
  `ConfigPartial`/`apply_partial` + `config_example.toml` + Nix mirror + Settings
  UI + partial-merge test). 99.8's config work is therefore only the **drain-site
  gate**: `route_osc99` returns early when `!config.osc_99` (in addition to the
  existing `!config.enabled` gate). No further config plumbing needed.
- **`osc_9`/`osc_777` separate enforcement is deferred to cleanup 99.10.** The
  `KITTY_PROTOCOL_REFERENCE` note asked 99.8 to also retroactively enforce
  `osc_9`/`osc_777`, but those cannot be gated separately without threading the
  OSC source (9 vs 777) through `AnsiOscType::Notify` →
  `WindowManipulation::Notification` → `NotificationKind` → `route` (a 4-layer,
  two-crate change) — both currently collapse to `NotificationKind::OscText`.
  That is a pre-existing Task 76 gap, out of proportion to fold into 99.8;
  tracked as 99.10 instead of silently skipped.
- **Dual-doc:** OSC 99 was never in `ESCAPE_SEQUENCE_GAPS.md` (never tracked as a
  gap), so per `freminal-escape-sequence-docs` it is **added to
  `ESCAPE_SEQUENCE_COVERAGE.md` only** (a new `OSC 99` row), NOT to GAPS. Both
  docs' "Last updated" headers are refreshed. The `KITTY_PROTOCOL_REFERENCE.md`
  notifications "current-state deltas" flip from gap-list to done.

##### 99.10 — Cleanup: separate `osc_9` / `osc_777` drain-site enforcement (surfaced during 99.8)

- **Surface point:** 99.8 config-gate work (2026-07-01), on `task-99/osc99-notifications`.
- **Impact:** `[notifications] osc_9` and `osc_777` are declared, defaulted,
  documented, and Settings-exposed, but **never read** at the drain site — both
  OSC 9 and OSC 777 notifications collapse to `NotificationKind::OscText` and are
  gated only by `enabled`/`routing_info`. Toggling `osc_9 = false` (or
  `osc_777 = false`) has no effect (a silent-drop, the exact
  `freminal-config-options` bug class — pre-existing since Task 76).
- **Scope of fix:** thread the OSC source (9 vs 777) from the parser to the
  router so each can be gated: add the source to `AnsiOscType::Notify` (or a
  discriminant on `WindowManipulation::Notification`), carry it to a
  `NotificationKind`/`NotificationRequest` discriminant, and gate in `route`.
- **Verification:** a routing test asserting `osc_9 = false` suppresses an OSC 9
  notification while OSC 777 still shows (and vice-versa).
- **Scheduling:** independent of OSC 99; can be done any time (a Task 76 hygiene
  fix). Not a blocker for the v0.11.0 OSC 99 work.

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

##### 100.2 execution decisions (recorded 2026-07-01, at execution against the real seams)

A recon of the six 100.2 seams confirmed the plan and found that the literal
single-subtask scope materially exceeds one Sonnet-sized pass: it spans 6 files
across 3 crates and carries two subtle hazards — the renderer uploads exactly one
GL texture per image id and never re-uploads when pixels change
(`sync_image_textures` early-continues if the id already has a texture), so making
it frame-aware is architecture-touching GPU work; and the compose-rectangle
alpha-blend math plus the `a=f`/`a=a`/`a=c` key re-aliasing are error-prone.
Maintainer-approved decision: **split 100.2 into three individually-reviewable
sub-passes** (mirroring the 99.5a/99.5b/99.5c precedent), each leaving
`cargo test --all` green, committed as `feat(v0.11.0): 100.2a/2b/2c`:

- **100.2a — frame model + snapshot transport + response `p=` gap (data/types
  layer).** Scope: `freminal-buffer/src/image_store.rs` (add a frame list to
  `InlineImage` — per-frame pixels + gap-ms — keeping `ImageStore` pure and
  time-free), `freminal-terminal-emulator/src/snapshot.rs` +
  `freminal-terminal-emulator/src/interface.rs` (ship all frames per animated
  image through `collect_visible_images`),
  `freminal-common/src/buffer_states/kitty_graphics.rs` (add
  `placement_id: Option<u32>` to `format_kitty_response`, emitting `,p=<pid>` when
  non-zero) and its ~6 call sites + `send_kitty_error` in
  `freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs` (query sites
  pass `None`; put/place sites pass `control.placement_id`). No animation
  behaviour, no GUI change. Leaves a single-frame image behaving exactly as today.
- **100.2b — animation handler `a=f`/`a=a`/`a=c` + frame-chunking (behaviour,
  headless-testable).** Scope: `freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs`
  **and** `freminal-buffer/src/image_store.rs`. The `image_store.rs` inclusion
  (a refinement of the 100.2a boundary): `a=a` control commands carry declarative
  animation-playback state (run/stop mode `s=`, loop count `v=`, app-forced
  current frame `c=`) that the handler must record on the image and the snapshot
  must ship for the GUI's wall-clock selector (100.2c) to read. That state is
  plain data (not wall-clock), so it lives on `InlineImage` as a new
  `AnimationControl` — set here in 100.2b, consumed in 100.2c. Making the handler
  headless-testable (assert frames added, gaps, compose results, loop/run state in
  the buffer) requires this buffer-side state. Ingests frames into the 100.2a
  model with the **key re-aliasing table**
  (parser stores animation keys under transmit/display-named fields; the handler
  re-interprets by action per `KITTY_PROTOCOL_REFERENCE.md`), including
  partial-frame rects, compose background (`c=`/`Y=`), blend (`X=`), edit (`r=`),
  gap (`z=`), control run/stop/loop (`s=`/`v=`/`c=`/`r=`/`z=`), compose
  (`ENOENT`/`EINVAL`/`ENOSPC`), and correct routing of `a=f` + `m=1` chunked frame
  transfers. Tested at buffer/handler level with no renderer dependency.
- **100.2c — GUI wall-clock frame selector + renderer per-frame re-upload (the
  display leg).** Scope: `freminal/src/gui/view_state.rs` (a wall-clock frame
  selector in `ViewState`, mirroring the `text_blink_cycle`/`text_blink_last_tick`
  tick precedent; the snapshot ships all frames, the GUI picks the current frame by
  elapsed time — **no frame-index state in the snapshot**),
  `freminal/src/gui/terminal/widget.rs` (plug the selector into the
  `build_image_verts` call site where `view_state` is in scope),
  `freminal/src/gui/renderer/vertex.rs` + `freminal/src/gui/renderer/gpu.rs` (make
  `sync_image_textures`/`draw_images` frame-aware — re-upload / key textures by the
  currently-selected frame so a frame change is drawn). Respects the lock-free
  GUI/PTY split. Scope additions at execution: (1) recon found the changed-content
  texture re-upload is the correctness-critical piece (`sync_image_textures`
  early-returns on a known id, so a same-id frame swap would silently no-op — it
  gains Arc-pointer-identity tracking to detect frame swaps); `vertex.rs` needs no
  change (`build_image_verts` reads only frame-invariant `display_cols`/rows).
  (2) Repaint scheduling in `widget.rs`: a running animation re-arms
  `ui.ctx().request_repaint_after(gap)` (alongside the existing cursor-trail
  re-arm) so frames advance while the terminal is otherwise idle. (3) `freminal/benches/render_loop_bench.rs`: a new CPU bench for the
  frame-selector tick (the GPU texture path is headless-excluded, and
  `build_image_verts` is unchanged, so the tick is the benchable new hot path).
  (4) One additive re-export line in `freminal-terminal-emulator/src/lib.rs`
  (`AnimationControl`, `AnimationRunMode`, `ImageFrame` added to the existing
  `image_store` `pub use`) so the `freminal` crate can name the animation types
  without a direct `freminal-buffer` dependency — the re-export mechanism already
  exists for `ImagePlacement`/`InlineImage`; 100.2a/2b simply did not need the
  animation types GUI-side yet.

**100.2a scope expansion (recorded).** Adding the two non-`Option` fields
(`frames`, `root_gap_ms`) to `InlineImage` — which derives neither `Default` nor
allows `..Default::default()` — forces a compile-break at every `InlineImage`
struct literal in the workspace, not just the kitty ones. The 100.2a recon missed
five: two **production** sites in the sixel and iTerm2 image handlers
(`freminal-terminal-emulator/src/terminal_handler/graphics_sixel.rs`,
`freminal-terminal-emulator/src/terminal_handler/graphics_iterm2.rs`) and three
test literals (`freminal-buffer/src/buffer/mod.rs`, `freminal-buffer/src/row.rs`,
`freminal-terminal-emulator/src/terminal_handler/mod.rs`). All five gain the
still-image defaults (`frames: Vec::new(), root_gap_ms: 0`); this is a
behaviour-neutral, compile-forced consequence, so 100.2a's file scope is expanded
to include them. No behaviour changes in sixel/iTerm2 (both stay single-frame).

The three sub-passes retain 100.2's overall deliverable and prohibitions; each
stops at its own review gate.

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

##### 100.3 execution decisions (recorded 2026-07-01, against the real seams + upstream spec)

Recon confirmed the core placeholder machinery is **already complete and
rendered** (`freminal-common/src/buffer_states/unicode_placeholder.rs`: 297-entry
diacritic table, `PLACEHOLDER_UTF8`, `parse_placeholder_diacritics`,
`color_to_image_id`/`color_to_placement_id`; `TerminalHandler` `handle_data_with_placeholders`
/ `handle_placeholder_char` / `resolve_placeholder_diacritics` with the 3
left-to-right inheritance rules and `PrevPlaceholder` state; virtual placements
render through the generic `build_image_verts` path; tests cover diacritic decode,
inheritance, creation, delete-by-id/all). So 100.3 does **not** rebuild any of
that. The renderer needs no change; `build_image_verts` was miscited in the
original scope line — it already handles placeholder cells generically.

**The `image_number` (`I=`) gap, resolved against the upstream spec (kitty
graphics-protocol, "Unicode placeholders" + "Requesting image ids").** The spec
(re-read 2026-07-01) is unambiguous: a Unicode placeholder's foreground color
_always_ encodes an image **id** (the 3rd diacritic extends its MSB) — there is
**no** placeholder-references-by-number mechanism, so the earlier worry about a
"number-vs-id bit in the placeholder" is void. The real gap is the general
**`I=` reference-by-number** feature freminal lacks entirely: (1) transmitting
with `I=<n>` (no `i=`) must create a new image, assign it an id, record a
`number → newest-id` association, and reply `\e_Gi=<id>,I=<n>;OK\e\`; (2) later
`a=p,I=<n>` (put) and `a=f,I=<n>` (animation frame) must resolve to the **newest**
image with that number; (3) `d=n`/`d=N` delete-by-number must also prune
`virtual_placements` (today it only clears cell placements). freminal has **no**
`number → id` index anywhere (`ImageStore` is a flat `HashMap<u64, InlineImage>`;
`image_number` lives only on per-cell `ImagePlacement`).

Scope (single Sonnet pass — coherent "image-number resolution", no GPU/architecture
hazard): `freminal-buffer/src/image_store.rs` (a `number_to_id: HashMap<u32, u64>`
"newest id per number" index, maintained on `insert`/`remove`/`clear`, + a
`newest_id_for_number(n) -> Option<u64>` resolver);
`freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs` (record the
number→id association at transmit; resolve `I=` in `handle_kitty_put` and
`handle_kitty_animation_frame` when `i=` is absent/0; prune `virtual_placements`
in the `d=n`/`d=N` arms); `freminal-common/src/buffer_states/kitty_graphics.rs`
(the OK response must echo `I=<n>` alongside `i=<id>` for an `I=` transmit — extend
the response formatter, e.g. a `KittyResponseId { image_id, image_number:
Option<u32>, placement_id: Option<u32> }` replacing the 100.2a `(image_id,
placement_id)` args, so the 3 optional identity fields are carried without a
4-scalar signature). Plus the "verify conformance" tests. Dual-doc update is
100.8, not here.

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

##### 100.4 execution decisions (recorded 2026-07-01, against the real seams + maintainer direction)

Recon revealed the plan's "handler/store work only" scoping materially
under-estimated 100.4, and surfaced a **pre-existing, protocol-agnostic
image-under-reflow bug** that must be fixed first. Findings and the
maintainer-approved restructure:

- **freminal has no first-class placement objects.** Every non-virtual placement
  is only per-cell `ImagePlacement` stamps written into `Cell`s by
  `Buffer::place_image`; there is no registry keyed by `(image_id, placement_id)`,
  and a placement's screen origin is never stored (it exists transiently as the
  cursor position at `place_image` time). The only placement-keyed map is
  `virtual_placements` (Unicode-placeholder tile sizes, no parent link). So 100.4
  builds the **first** real-placement registry.
- **Image position is cell-anchored, so scroll/reflow tracking is _already_ solved
  for every protocol** — an image lives in the row cells, which scroll and reflow.
  The renderer draws each image "wherever its cells are". Relative placements only
  add a _derived_ position (child at `parent_origin + (H,V)`).
- **The real fail case is reflow, and it is a pre-existing, protocol-agnostic
  bug.** `Buffer::reflow_to_width` re-wraps image-stamped cells by glyph width like
  ordinary text (each image row is its own hard-break logical line), so a
  primary-screen image wider than the new width gets **soft-wrapped and its
  `(row_in_image, col_in_image)` rectangle fragments** — the renderer then draws it
  distorted. This hits kitty/sixel/iTerm2 alike; it is latent today only because
  images are effectively alt-screen (apps redraw on resize). Building relative
  placements on this cracked foundation is wrong. **Maintainer decision: fix image
  reflow atomicity first.**
- **Renderer bounds images by `image_id` only** (not `(image_id, placement_id)`) in
  `build_image_verts`, so two on-screen placements of the same image visually
  merge — relevant to same-image relative placements; addressed in 100.4b if it
  bites.
- **`ETOODEEP`/`ECYCLE`/`ENOPARENT` error strings do not exist yet**; no
  cascade-delete concept exists.

**Restructure (maintainer-approved): 100.4.0 → 100.4a → 100.4b.**

- **100.4.0 — Image reflow atomicity (protocol-agnostic prerequisite).** Scope:
  `freminal-buffer/src/buffer/resize_and_alt.rs` (+ `buffer/images.rs` / `row.rs`
  as needed). Make reflow treat an image's cell-rectangle atomically so images
  survive reflow as coherent rectangles for **all** protocols (keep image rows off
  the soft-wrap path — clamp/scale to the new width or preserve the block intact —
  rather than re-wrapping image cells by glyph width). Fixes the actual fail case
  the maintainer identified; a real bug fix, so it gets a regression test that
  fails before / passes after. Recon the reflow path precisely before implementing.
- **100.4a — Relative-placement registry + validation + real-parent positioning.**
  Scope: `freminal-buffer/src/image_store.rs` (or a new registry) +
  `freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs`. Add the
  first-class real-placement registry keyed by `(image_id, placement_id)` holding
  the persisted screen origin + `parent` link + child list; cascade delete;
  `ENOPARENT`/`ECYCLE`/`ETOODEEP` (depth ≥ 8); a virtual placement may be a parent
  but cannot itself be relative (`EINVAL`); the cursor must not move after a
  relative placement regardless of `C`. Position a child of a **real** parent by
  cell-stamping at `parent_origin + (H,V)` (the child becomes real cells that
  scroll/reflow correctly on their own, atop the 100.4.0 fix). Headless-testable.
- **100.4b — Virtual-placeholder-parent live position derivation.** Scope: the
  snapshot/render seam (recon first). When the parent is a **virtual** placement
  (no fixed cells; the primary kitty use — a placeholder anchored in text), derive
  the child's position each snapshot from the parent placeholder's live cell
  positions (min-x / min-y of the placeholder cells, per spec) so it follows text
  through scroll/reflow. This is the render-touching piece; recon its exact seam
  before writing it.

Each stops at its own review gate.

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

##### 100.5 execution decisions (recorded 2026-07-01, against the real seams + upstream spec)

Recon resolved three open shape questions:

- **LRU age proxy: a dedicated insertion sequence, NOT the image id.** kitty's
  `i=` lets a client pick an arbitrary id, so `InlineImage.id` is not a reliable
  age proxy. `ImageStore` gains a monotonic `u64` insertion counter and stamps an
  insertion sequence per stored image (independent of the protocol id); eviction
  picks the lowest sequence (oldest).
- **Placement-aware eviction via a `placed` set maintained by `retain_referenced`,
  no plumbing through place/clear sites.** `ImageStore::insert` cannot see the cell
  grid, and most inserts bypass `Buffer::place_image`, so a per-insert placement
  oracle would need error-prone plumbing through ~7 place/clear sites. Instead,
  `retain_referenced` (already driven by the `Buffer` on resize/scroll, and which
  already removes truly-unreferenced images) also records the current referenced
  id set into a `placed: HashSet<u64>` on the store. Eviction prefers images NOT
  in `placed` (placement-less first), then falls back to oldest by insertion
  sequence. The `placed` set may be slightly stale between `retain_referenced`
  runs — acceptable: the quota is a DoS guard that only fires under memory
  pressure, and the spec explicitly leaves the exact eviction count an
  implementation choice ("at least a few full-screen images").
- **Two separate byte pools (base vs animation), mirroring the spec's two-row
  table.** Base pool = sum of root `pixels.len()` across all images, cap
  `KITTY_IMAGE_BASE_QUOTA_BYTES = 320 * 1024 * 1024`. Animation pool = sum of
  `frames[].pixels.len()`, cap `KITTY_IMAGE_ANIM_QUOTA_BYTES = 5 * base`. Eviction
  fires when EITHER pool exceeds its cap (evicting whole images —
  placement-less-then-oldest — until both pools are within budget). Named
  constants so they can be tuned without a protocol change. (The spec is
  ambiguous on single-vs-two-pool; two pools is the closer reading of kitty's
  actual behaviour and is cleaner to reason about.)
- **Benchmark: add one (none exists).** `freminal-buffer/benches/buffer_row_bench.rs`
  has no image-store bench, so per `freminal-bench-table` a new
  `bench_image_store_insert_at_quota` (insert-one-more at/near the cap, forcing an
  eviction pass) is added and its baseline captured. `build_snapshot`'s image path
  is not changed by 100.5, so no render-bench delta is expected.

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

##### 100.6 execution decisions (recorded 2026-07-01, against the real seams + maintainer direction)

- **No flake stop-and-wait (maintainer-confirmed).** The plan and the orchestrator
  brief anticipated a dependency-add STOP-and-wait for `nix develop`. Recon plus an
  empirical `Cargo.lock` check showed neither dep pulls a **system** library:
  `flate2 1.1.9` is already in `Cargo.lock` transitively (via `png`/`tiff`) and
  resolves to the pure-Rust `miniz_oxide` backend (no `libz-sys`/`zlib-ng`
  anywhere); the `nix` crate's `"mman"` feature is a thin libc wrapper (libc is
  already a workspace dep). Per `flake-dev-shell-discipline`, the mandatory
  stop-and-wait fires only when a **system tool/library** is added to `flake.nix` —
  not for these. So 100.6 is pure Cargo.toml edits (workspace `Cargo.toml`:
  `flate2` alphabetically, full semver; `freminal-terminal-emulator/Cargo.toml`:
  `flate2.workspace = true` + add `"mman"` to the existing
  `nix = { workspace = true, features = ["term"] }`), which cargo picks up in the
  current shell with no `flake.nix` change and no `nix develop` re-enter.
- **Scope expansion: add `O=` (byte offset) parsing (maintainer-confirmed).** The
  spec's `O=` key ("read `S` bytes at offset `O`" from file/shm) is **not** parsed
  in `freminal-common` today (only `S=`/`data_size` is). Full `t=s` correctness
  needs it, so 100.6's scope is expanded to add a `data_offset: Option<u32>` field
  and an `O` parser arm to `freminal-common/src/buffer_states/kitty_graphics.rs`
  (mechanical, mirrors `S=`), honoured in the shm read (and available to `t=f`/`t=t`
  too).
- **`t=s` security = spec's special-file / sensitive-path refusal (new code).**
  Recon found the reference doc's claim that `t=f`/`t=t` already "refuse special
  files / restrict temp dirs" is **aspirational** — the real `read_kitty_file`
  only checks `is_absolute()`. 100.6 writes the spec's refusal (reject
  device/socket/special files; refuse `/proc`, `/sys`, `/dev`) for the **shm**
  path per the plan, and does NOT weaken the existing `t=f`/`t=t` checks. The
  doc-vs-code discrepancy on `t=f`/`t=t` hardening is noted for the 100.8 dual-doc
  pass (do not silently "fix" `t=f`/`t=t` here — out of scope).
- **Decompress in `decode_kitty_payload`** right after `resolve_kitty_transmission`
  yields the raw bytes and **before** the RGB/RGBA/PNG format branch (the single
  choke point; `o=z` applies to every `f=`). Windows shm uses a new
  `#[cfg(windows)]` path (winapi file-mapping); POSIX uses `nix` `"mman"`
  (`shm_open`/`mmap`/`shm_unlink`), inline `#[cfg(unix)]`/`#[cfg(windows)]`
  matching the `io/pty.rs` precedent.

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
- **~~"And-after" variants~~ (CORRECTED — see 100.7 execution decisions):** the
  plan's "and-after" framing is a **misreading of the spec**. There is no
  "and-after" concept; the lowercase/UPPERCASE axis is the data-freeing axis for
  EVERY `d=` target (uppercase also frees the image data). The misnamed enum
  variants are repurposed accordingly.
- **Missing enum variants:** add `d=f`/`d=F` (delete animation frames — pairs with
  100.2), `d=r`/`d=R` (delete images with id in `[x, y]`, kitty 0.33.0), and
  `d=q`/`d=Q` (placements at cell `x,y` with z-index `z`); all currently return
  `UnknownDeleteTarget` and are ignored.
- **Z-index render order:** `build_image_verts` / `draw_images` sort quads by
  `image_id`, not `z_index`; higher z must render above lower z. Sort by z-index
  (then id for stability).

Deliverable: corrected delete handling + z-ordered rendering + tests (lowercase
keeps data, uppercase frees, visible-vs-all, `d=f`/`d=r`/`d=q`, a two-image
z-order assertion).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT change transmit/put/query parsing beyond adding the new
delete-target variants; do NOT proceed.

Stop: report + await review.

##### 100.7 execution decisions (recorded 2026-07-01, against the real seams + upstream spec)

Recon + the authoritative kitty graphics spec ("Deleting images") corrected a
material misreading and resolved the renderer data-flow question:

- **There is NO "and-after" concept — the case axis is data-freeing.** The spec
  is explicit: for EVERY `d=` target, lowercase deletes placements only (keeps the
  stored image data so it can be re-displayed without resending); UPPERCASE
  additionally frees the image data **provided the image is not referenced
  elsewhere (e.g. in scrollback)**. So freminal's enum variant names
  `*AndAfter`/`*CursorOrAfter` (and their doc-comments) are **wrong** — uppercase
  `A/I/N/C/P/Q/X/Y/Z` mean "same target + free data", not "and after". The
  existing handler collapses each lowercase|uppercase pair to identical behaviour,
  so today no uppercase actually frees data correctly, and `d=n`/`d=N` never frees
  store data at all (a latent bug). Maintainer-approved: implement the real spec.
- **Enum correction (maintainer-approved option 1).** Rename the misnamed
  `KittyDeleteTarget` variants to the data-freeing axis (e.g. a lowercase target +
  an uppercase "…FreeData" sibling per letter, OR a `free_data: bool` on unified
  variants — the implementer picks the cleaner shape and records it), fix the
  stale doc-comments, and ADD `d=f`/`d=F`, `d=r`/`d=R`, `d=q`/`d=Q`. `d=a` deletes
  visible-on-screen placements only; `d=A` all. "Uppercase frees data if not
  referenced elsewhere" reuses the existing cell-reference check (the same idea as
  `ImageStore::retain_referenced`): after clearing the targeted cells, an
  uppercase delete removes the image from the store only if no remaining cell (in
  the full `self.rows`, scrollback included) still references it. The 10
  `clear_image_placements_*` Buffer methods touch cell-stamps only (never the
  store), so data-freeing stays a handler concern layered on top.
- **`d=a` visible-only needs a visible-window-scoped clear (new).** No existing
  clear method scopes to the visible window — all iterate the full `self.rows`.
  `d=a` (visible on screen) uses `visible_window_start(0)..rows.len()` (the live
  bottom `height` rows — the PTY thread's only notion of "on screen", since the
  GUI scroll offset lives in `ViewState`); `d=A` keeps the full-`self.rows`
  behaviour. A small buffer helper is added for the visible-scoped clear.
- **Cascade + registry hygiene.** Positional deletes (`c/p/q/x/y/z`) currently do
  NOT prune `virtual_placements`/`real_placements`; the id/number arms do (via
  `cascade_delete_real_placements_for_image`). Keep the existing cascade for
  id/number; for the new/positional arms, prune `real_placements`/`virtual_placements`
  for any image whose last cell was just cleared (so no stale parent-link metadata
  survives). Do not regress the 100.4a cascade.
- **Two commits: 100.7a (delete correctness) + 100.7b (z-index render order).**
  They are independent; splitting keeps each reviewable. 100.7a =
  `freminal-common` enum/parser + `graphics_kitty.rs` handler + `freminal-buffer`
  visible-scoped clear helper. 100.7b = the renderer z-index sort.
- **Z-index render order via an explicit draw-order side-channel (maintainer-approved).**
  `build_image_verts` (GUI thread) has each image's `z_index` via the per-cell
  `ImagePlacement`s; `draw_images` runs in the `Send+Sync+'static` `PaintCallback`
  and only receives `snap_images` (no z-index). Rather than pollute `InlineImage`
  with placement-level z, `build_image_verts` computes the authoritative
  `(z_index, id)` draw order and records it as a `Vec<u64>` in `RenderState`
  alongside `image_verts`; `draw_images` consumes that list directly instead of
  recomputing its own `u64`-ascending sort. Vertex-emission order and draw order
  then share one authoritative list (no fragile "both independently reproduce the
  same sort" invariant). Higher z renders above lower z; ties break by id.
- **100.7b scope expansion (recorded):** the mandatory `build_image_verts`
  z-order unit tests need to construct `ImagePlacement` literals, which requires
  naming `ImageProtocol` — not previously re-exported from
  `freminal-terminal-emulator`. A one-line, behaviour-free addition of
  `ImageProtocol` to the existing `image_store` `pub use` in
  `freminal-terminal-emulator/src/lib.rs` (mirroring the 100.2c re-export
  expansion) unblocks the tests; 100.7b's scope includes it.

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

#### 100.9 — Source-rect crop for `a=p` (`x/y/w/h`)

Added 2026-07-01 (maintainer-approved gap closure). The 100.1 audit listed
source-rect crop as a gap but it was never given its own subtask — a planning
omission surfaced at Task-100 doc time. kitty's `a=p` display keys `x=`/`y=`
(top-left of the source region, px) and `w=`/`h=` (its size, px) select a
SUB-RECTANGLE of the image to display; freminal parses them but a placement
always shows the full image because the renderer's `compute_image_quad`
(`vertex.rs`) derives UVs from the cell grid (`min/max col/row_in_image`), not a
pixel source-rect.

Scope: `freminal-buffer/src/image_store.rs` (`ImagePlacement` gains an optional
pixel source-crop rect), the handler place path
(`freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs`, stamp the
crop onto the placement), the snapshot (rides the existing per-cell
`ImagePlacement`), and `freminal/src/gui/renderer/vertex.rs` (`compute_image_quad`
maps the pixel crop into the UV sub-rectangle). Recon the exact UV math first.

Deliverable: source-rect crop applied on display + tests (a placement with a
sub-rect crop yields the expected UV sub-region; no crop = full image, unchanged).

Scope expansion (recorded): adding the non-`Option`-defaultable `source_crop`
field to `ImagePlacement` and a `source_crop` param to
`Buffer::place_image`/`place_image_at` is compile-forced across every
`ImagePlacement` literal and `place_image*` call site in the workspace
(`freminal-buffer/src/buffer/mod.rs`, `row.rs`; the sixel/iTerm2 handlers;
`terminal_handler/mod.rs`'s placeholder + virtual-parent injection). All get the
still-image default (`source_crop: None` / trailing `None`) — behaviour-neutral
for every non-`a=p`/`a=T` path. `SourceCrop` is re-exported from
`freminal-terminal-emulator/src/lib.rs` (one-line, mirroring `ImageProtocol`) so
the renderer can name it. The `ImageBounds` map remains keyed by `image_id`
(not `(image_id, placement_id)`), so two on-screen placements of the same image
with different crops collapse to first-seen — a pre-existing limitation shared
with `z_index` (100.7b), out of 100.9 scope.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT change `a=c` compose (which already uses `x/y/w/h`
correctly); do NOT proceed.

Stop: report + await review.

#### 100.10 — Windows `t=s` shared memory (winapi file-mapping)

Added 2026-07-01 (maintainer-approved gap closure). 100.6 implemented `t=s` on
POSIX (real `shm_open`/`mmap`/`shm_unlink` via `nix` `mman`) but left the Windows
path a compile-safe `ENOTSUP` stub, deferring the `winapi` dependency add. This
closes it: `winapi` is already a workspace dependency (used by `portable-pty`),
so adding it to `freminal-terminal-emulator` is a small, non-flake Cargo.toml
change plus the `#[cfg(windows)]` file-mapping read.

Scope: `freminal-terminal-emulator/Cargo.toml` (add a
`[target.'cfg(windows)'.dependencies]` block with `winapi.workspace = true` +
the file-mapping features — `memoryapi`, `handleapi`, and any others
`OpenFileMappingW`/`MapViewOfFile`/`UnmapViewOfFile`/`CloseHandle` need),
`freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs` (replace the
Windows `read_kitty_shared_memory` ENOTSUP stub with a real
`OpenFileMappingW`(FILE_MAP_READ) → `MapViewOfFile` → copy `S` bytes at offset
`O` → `UnmapViewOfFile` + `CloseHandle` read; mirror the POSIX security
refusal). No flake change (winapi is a Windows-only Rust crate, no system lib).

Deliverable: Windows `t=s` read + a `#[cfg(windows)]` test (a real
`CreateFileMappingW`-backed object round-trip, hermetic), mirroring the POSIX
test shape. If a Windows CI runner is unavailable, ensure the code compiles under
`--target x86_64-pc-windows-*` gating and the POSIX suite stays green.

Verification: `cargo test --all` (POSIX); `cargo clippy --all-targets --all-features -- -D warnings`;
confirm the `#[cfg(windows)]` code compiles (cargo check for the windows target
if the toolchain is available, else careful cfg review).

Prohibitions: do NOT touch the POSIX path (already correct); do NOT weaken the
security refusal; do NOT add a flake change (winapi needs none); do NOT proceed.

Stop: report + await review.

#### 100.11 — Live render bug: animation does not animate (`a=a`)

Surfaced 2026-07-02 by a maintainer live-window run of `test-scripts/kitty_graphics.py`
step 7, on top of `4561a275`. Unit tests (handler/store/`ViewState` state) are green;
the state-only tests could not catch a GUI repaint/frame-clock defect.

- **Surface point:** commit `4561a275` (manual test script); the animation display leg
  landed in 100.2c (`f48ac368`).
- **Impact:** after `a=a,i=3,s=3,v=1` (run, infinite loop), the image never cycles
  red→green→blue; it holds on the first frame. `a=a` is intentionally silent (no PTY
  response, per spec — 100.2b), which is correct and not the bug.
- **Scope:** `freminal/src/gui/view_state.rs` (`tick_image_animations` /
  `ImageAnimationClock`); `freminal/src/gui/terminal/widget.rs` (the full-rebuild
  trigger set + `request_repaint_after` re-arm). READ-ONLY recon (2026-07-02) found a
  confirmed defect: the GUI clock's `frame_started` is anchored the first time an image
  is observed with `is_animated()` (which ignores `run_mode`), i.e. as soon as the first
  `a=f` frame arrives while still `Stopped`, and is never re-anchored on the
  `Stopped → Running` transition at `a=a`. Whether this alone freezes playback vs. only
  skips an initial frame is a runtime-timing question the recon could not close
  statically.
- **Approach:** pending confirmation from a live `.frec` capture of the exact step-7
  bytes + behavior (per `freminal-frec-decoder`). Candidate fix: re-anchor
  `clock.frame_started = now` (and reset `loops_done`) on a `Stopped → Running`
  transition, so playback starts cleanly from the run point.
- **Verification:** a regression test closer to end-to-end than the state-only tests —
  assert `tick_image_animations` advances the selected frame over successive wall-clock
  ticks given a `Running` image in the snapshot map (using the `#[cfg(test)]`
  back-dated-clock seeding helper).
- **Scheduling:** part of Task 100; blocks the v0.11.0-kitty merge.

#### 100.12 — Live render bug: animation compose does nothing (`a=c`)

Surfaced 2026-07-02 by the same live run (step 8), on top of `4561a275`.

- **Surface point:** commit `4561a275`; the compose handler landed in 100.2b
  (`17e1f28b`), the display leg in 100.2c (`f48ac368`).
- **Impact:** `a=c,i=3,r=1,c=2` composes frame 1 pixels onto frame 2 (a store-mutation
  that changes no cell and no `run_mode`). Nothing visibly happens.
- **Scope:** `freminal/src/gui/terminal/widget.rs` (the full-rebuild trigger set at the
  `show()` rebuild decision). READ-ONLY recon (2026-07-02) found the **definitive** root
  cause: the PTY thread republishes the snapshot unconditionally after every batch (not
  damage-gated), so the new composed-frame pixel `Arc` reaches the GUI — but the widget's
  full-rebuild trigger set has **no term for a store-only image-pixel mutation**.
  `image_frame_changed` fires only from `tick_image_animations`'s `changed` set
  (wall-clock stepping or app-forced `current_frame`), never from a raw store mutation.
  The full-rebuild path is the only path that refreshes `RenderState.snap_images` and the
  only path that calls `sync_image_textures` (`draw_with_cursor_only_update` never does),
  so the GPU texture stays stale until an unrelated trigger forces a full rebuild.
- **Approach:** give the widget a trigger that detects an image-pixel change independent
  of the wall-clock clock — e.g. track the per-id pixel-`Arc` pointer identity of the
  snapshot images against the previous frame's, and force the full rebuild when any
  animated image's selected-frame pixel `Arc` differs. (Shares the same architectural
  seam as 100.11 but is a distinct fix.)
- **Verification:** a regression test that a compose-mutated image's changed pixels are
  reflected in the rebuilt `RenderState.snap_images` (or the rebuild-trigger predicate
  fires) — closer to end-to-end than the state-only compose test.
- **Scheduling:** part of Task 100; blocks the v0.11.0-kitty merge.

##### 100.12 execution decisions (recorded 2026-07-02, fix landed `f7d77ac2`)

Root cause confirmed definitively as recon predicted. The fix adds an
`image_pixels_changed` full-rebuild trigger in `freminal/src/gui/terminal/widget.rs`.
`PaneRenderCache` gains `last_rendered_image_pixel_ptrs: HashMap<u64, usize>` recording
each visible image's selected-frame pixel-`Arc` pointer address as of the last full
rebuild; `show()` rebuilds that map from the current snapshot and forces a full rebuild
(and excludes the cursor-only fast path) when it differs. The map is built by the pure
free function `build_image_pixel_ptrs`, which mirrors exactly which `Arc` the rebuild
block uploads (animated → `frame_pixels(selected_frame(id))` with a root-`pixels`
fallback; still → root `pixels`), so the trigger and the upload always agree. The cache
is refreshed on every full rebuild, so a one-off mutation cannot pin the pane in
perpetual full-rebuild (no CPU spin). The predicate is a pure function tested directly
(no live GUI): a selected-frame pixel-`Arc` replacement is flagged; an unchanged pointer
and an unselected-frame mutation are not (fail-before / pass-after verified). Chosen
over widening the snapshot/`content_changed` signal because the change is entirely
GUI-side, respects the lock-free read-only `update()` contract, and adds no snapshot
transport. Per-frame cost for the no-image common case is a zero-iteration map build.

#### 100.13 — Live render bug: unicode placeholder does not render (`U=1`)

Surfaced 2026-07-02 by the same live run (step 9), on top of `4561a275`.

- **Surface point:** commit `4561a275`; the placeholder machinery predates Task 100 and
  was audited complete in 100.3 (`f2c08209`).
- **Impact:** transmit quietly (`a=t,…,q=2`), create a virtual placement
  (`a=p,i=4,U=1,c=2,r=2`), print U+10EEEE cells with SGR-truecolor fg (image id) +
  row/col diacritics — nothing renders in the placeholder cells.
- **Scope:** `freminal-terminal-emulator/src/terminal_handler/mod.rs`
  (`handle_data_with_placeholders` / `handle_placeholder_char` fast-path guard),
  `graphics_kitty.rs` (the `a=p,U=1` virtual-placement registration + the `q=2` transmit
  decode path), `freminal-common/src/buffer_states/unicode_placeholder.rs`
  (`color_to_image_id`). READ-ONLY recon (2026-07-02) found **no code-logic break** — the
  ordering, id extraction, cell stamping, and snapshot transport are all sound and the
  unit test mirrors the script and passes. Leading hypothesis (live-only): the `q=2`
  transmit's payload size may not match `s=2,v=2,f=32`, so `decode_kitty_payload` fails
  silently (`q=2` suppresses the error), the image is never stored, and the subsequent
  `a=p,U=1` hits `handle_kitty_put`'s `ENOENT` early-return **before**
  `register_virtual_placement` runs — so no virtual placement is ever created and the
  placeholder cells fall through to the space fallback.
- **Approach:** pending a live `.frec` capture (per `freminal-frec-decoder`,
  `sequence_decoder.py --convert-escape`) to see the exact bytes freminal receives —
  specifically the transmit payload byte-count, the `a=p,U=1` control keys, and the SGR
  fg. Fix shape decided once the capture identifies the real break.
- **Verification:** a regression test closer to end-to-end than the state-only test —
  drive the exact step-9 byte sequence through the handler and assert the placeholder
  cells carry `ImagePlacement`s that reach a built `TerminalSnapshot`'s visible image
  placements.
- **Scheduling:** part of Task 100; blocks the v0.11.0-kitty merge.

#### 100.14 — Live render bug: relative placement (real parent) lands at wrong offset (`P=`)

Surfaced 2026-07-02 by the same live run (step 10), on top of `4561a275`.

- **Surface point:** commit `4561a275`; the real-parent positioning landed in 100.4a
  (`1dfd82eb`).
- **Impact:** `a=T,i=5` (cyan) displays; `a=t,i=6` stored; `a=p,i=6,P=5,H=2,V=1` should
  render magenta offset from cyan — but the child does not appear clearly at
  parent+offset, and parent behavior is unclear.
- **Scope:** `freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs`
  (`place_kitty_image` / `stamp_kitty_put` — the `record_real_placement` origin capture);
  possibly `freminal-buffer/src/buffer/images.rs` (`place_image` return value). READ-ONLY
  recon (2026-07-02) found a **concrete, evidenced candidate**: `record_real_placement`
  captures `self.buffer.cursor().pos` **before** `place_image()` is called, but
  `place_image` may call `enforce_scrollback_limit`, which drains rows from the top and
  decrements the buffer's own `cursor.pos.y`. The recorded `origin_row` is then stale
  (too large by the drained-row count), so the child origin derived as
  `parent_origin.row + V` lands wrong. Only manifests in a live session with accumulated
  scrollback (unit tests use a fresh small buffer, so the drain never fires) — which
  matches "unit-green but live-broken".
- **Approach:** capture the placement origin from the **actual** post-`place_image` row
  index rather than the stale pre-drain cursor copy — either capture the cursor after
  `place_image` returns, or have `place_image`/`place_image_at` return the true stamped
  origin and use that in `record_real_placement`. Applies to both the `a=T`/`a=t`-display
  branch (`place_kitty_image`) and the `a=p` branch (`stamp_kitty_put`).
- **Verification:** a regression test that grows the buffer past `scrollback_limit` so a
  placement triggers a scrollback drain, then asserts the recorded real-placement origin
  matches the actual stamped cell row (fails before / passes after) — the specific gap no
  existing test exercises.
- **Scheduling:** part of Task 100; blocks the v0.11.0-kitty merge.

#### 100.15 — Live render bug: displayed image destroyed by subsequent output (shared root cause of 100.11/100.13)

Surfaced 2026-07-02 by a maintainer live comparison against real kitty, then confirmed
by READ-ONLY recon. This is the **shared root cause** behind the "animation does
nothing" (100.11) and "placeholder does not render" (100.13) symptoms: the displayed
image is destroyed before any effect can be observed.

- **Surface point:** predates Task 100 (a latent `Buffer::place_image` defect), exposed
  by the Task 100 live testing on `4561a275`.
- **Impact:** after an image is displayed at the buffer tail (the normal case), the next
  character write (shell prompt, the test-script menu re-print, an animation frame's
  surrounding text) fully destroys the image — it disappears entirely rather than
  scrolling with the text as in kitty.
- **Scope:** `freminal-buffer/src/buffer/images.rs` (`place_image` cursor-final-position
  logic, ~544-551). ROOT CAUSE (recon-confirmed): when the image is placed at the tail,
  the row-creation loop leaves `self.rows.len() == base_row + display_rows == final_row`
  exactly, so the guard `if final_row < self.rows.len()` is false (equality), the `else`
  branch fires, and the cursor is parked on the image's **own last row** instead of a
  fresh row below it — contradicting `place_image`'s own doc comment. The next text write
  then overwrites the image's cells (`insert_text` → `clear_images_overwritten_by_text`).
  Protocol-agnostic trigger; total for short/1-row images. Existing tests missed it by
  pre-padding rows or manually resetting the cursor before writing.
- **Approach:** guarantee a fresh blank row exists **below** the placed image and move
  the cursor there (append one more row when `final_row == rows.len()`, then set
  `cursor.pos.y = final_row`), so subsequent output goes below the image and never
  overwrites it — matching kitty/iTerm2. Honours the plan's cell-anchored decision (the
  scroll path already carries image cells correctly); this only fixes where the cursor
  lands. Must not double-append when rows already exist below, and must interoperate with
  the 100.14 post-drain origin capture (the extra pushed row may itself trigger a
  scrollback drain).
- **Verification:** a regression test that places an image at the buffer tail with NO
  pre-padding and NO manual cursor reset, then writes text, and asserts the image's cells
  survive and the cursor is on a fresh row below (fails before / passes after). After this
  lands, re-test steps 7 (animation) and 9 (placeholder) live to determine any residual
  100.11/100.13 work.
- **Scheduling:** part of Task 100; blocks the v0.11.0-kitty merge. Likely subsumes the
  visible symptoms of 100.11 and 100.13.

#### 100.16 — `C=1` (no cursor movement) not honoured on `a=T`/Put

Surfaced 2026-07-02 while implementing 100.15: the corrected cursor positioning
unmasked a genuine pre-existing gap that the old off-by-one had been coincidentally
masking.

- **Surface point:** exposed by the 100.15 cursor fix; the gap itself predates Task 100.
- **Impact:** kitty `C=1` (do not move the cursor after displaying) is honoured only on
  the `a=p` path (`stamp_kitty_put` saves/restores the cursor), NOT on the `a=T` /
  `TransmitAndDisplay` (and `Put` via `place_kitty_image`) path. The test
  `kitty_transmit_and_display_with_no_cursor_movement` passed only because the old
  cursor bug clamped a 1-row image's cursor back to its origin (== the pre-call cursor),
  coincidentally matching the `C=1` expectation.
- **Scope:** `freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs`
  (`place_kitty_image` display branch). Save the cursor before `place_image` and restore
  it when `control.no_cursor_movement` is set, mirroring `stamp_kitty_put`.
- **Approach:** in the `else if should_display` branch of `place_kitty_image`, capture
  the cursor before placement and, if `no_cursor_movement`, restore it after — matching
  the existing `stamp_kitty_put` save/restore. The real-placement origin recorded via
  `record_real_placement` must remain the image's true stamped origin (100.14), not the
  restored cursor.
- **Verification:** `kitty_transmit_and_display_with_no_cursor_movement` passes for the
  right reason (cursor genuinely restored, not coincidentally clamped); add/confirm a
  companion test that the cursor DOES move below the image when `C` is absent/0.
- **Scheduling:** landed together with 100.15 (the fixes are tightly coupled — 100.15
  unmasks 100.16 — so they ship as one atomic, suite-green commit per
  `commit-discipline`).

#### 100.17 — Images scaled to cell grid instead of drawn at native pixel size (all protocols)

Surfaced 2026-07-02 by a maintainer live comparison against kitty; confirmed by
READ-ONLY recon against the kitty, iTerm2, and sixel specs. A cross-protocol
rendering-fidelity bug, pre-existing since Task 13, distinct from the four live render
bugs (100.11–100.14, all fixed).

- **Surface point:** predates Task 100 (Task 13 renderer); exposed by Task 100 live
  testing on `4561a275`.
- **Impact:** freminal ALWAYS scales a displayed image to fill its `div_ceil(px, cell_px)`
  cell grid, but the specs require **native pixel size by default**. A 4×4px image
  reserves 1 cell (correct) but is stretched to fill the whole ~8×16px cell (~4× too
  large). Affects all three supported protocols; correct only when an explicit display
  size was requested.
- **Spec basis (upstream wins):**
  - kitty graphics-protocol, "Controlling displayed image layout": with no `c`/`r`, the
    image is "rendered at the current cursor position, from the upper left corner of the
    current cell" at native size; scaling happens **only** when `c`/`r` are given (if only
    one is given, the other is derived to preserve aspect ratio). freminal is wrong for
    kitty-default, correct for kitty-with-`c`/`r`.
  - iTerm2 (`iterm2.com/documentation-images.html`): default `width`/`height` = `auto` =
    the image's inherent (native) size; `imgcat` displays "at their full size". Wrong for
    iTerm2-default, correct for explicit `width`/`height`.
  - sixel (`vt100.net/shuford/terminal/all_about_sixels.txt`): strictly 1:1 native pixels,
    no cell-grid concept in the protocol at all. Wrong for **every** sixel image (sixel has
    no size arg).
- **Scope:** `freminal-buffer/src/image_store.rs` (`InlineImage` gains a size-mode signal),
  the three handlers (`graphics_kitty.rs`, `graphics_sixel.rs`, `graphics_iterm2.rs` — set
  the mode at construction while the `c`/`r`/`width`/`height` provenance is still known),
  the snapshot transport (the mode rides `InlineImage`, already shipped), and
  `freminal/src/gui/renderer/vertex.rs` (`compute_image_quad` — the shared render path;
  no per-protocol branch exists today).
- **Approach:** add a `SizeMode` enum (`NativePixels` | `ExplicitCells`) to `InlineImage`,
  set `ExplicitCells` when the protocol carried an explicit display size (kitty `c`/`r`,
  iTerm2 `width`/`height` non-auto) and `NativePixels` otherwise (kitty default, iTerm2
  auto, **always** sixel). `compute_image_quad` draws the quad at
  `InlineImage.width_px`/`height_px` (native, already stored and already passed in) anchored
  at the placement's top-left cell for `NativePixels`, and keeps the current scale-to-cell
  behaviour for `ExplicitCells`. The mode cannot be inferred downstream (comparing
  `display_cols == div_ceil(px)` false-positives when a user explicitly requests a
  native-equivalent size), so the signal must be set at construction time. Interacts with
  the documented sub-cell `X`/`Y` pixel-offset limitation — native-size rendering makes that
  gap distinctly visible (image sits exactly at the cell corner), so note it (do not
  necessarily fix it here).
- **Decomposition:** 100.17a (data + all-three-handler mode-setting + snapshot transport,
  behaviour-neutral wiring) → 100.17b (renderer branch in `compute_image_quad` + tests +
  a before/after Criterion capture on `build_image_verts`/`compute_image_quad` per
  `freminal-bench-table`, ≤15% regression).
- **Verification:** regression tests — a `NativePixels` image yields a native-pixel-sized
  quad; an `ExplicitCells` image still fills the declared cell grid; per-protocol
  construction sets the right mode. Criterion before/after on the render hot path. Live
  re-test that images now match kitty's size.
- **Scheduling:** part of Task 100; blocks the v0.11.0-kitty merge (maintainer directed
  fixing it before the merge — all protocols need it).

#### 100.18 — Per-placement identity: coexisting placements of the same image

Surfaced 2026-07-02 by a maintainer live run (step 4, `a=p,i=2,c=8,r=4` after a prior
`a=p,i=2`), root-caused from a `.frec` (decoded with `sequence_decoder.py`) + the kitty
spec. A pre-existing bug flagged (but never fixed) in the 100.4 and 100.9 execution
decisions.

- **Surface point:** predates Task 100; the id-only `ImageBounds` bucketing was flagged in
  100.4 (`a3e29d50`/notes) and 100.9. Exposed by Task 100 live testing.
- **Impact:** `build_image_verts` buckets `ImageBounds` by `image_id` alone, so two
  on-screen placements of the same image merge into one oversized bounding box → a
  grossly stretched/misplaced quad. Per the kitty spec (graphics-protocol lines
  1105/1111): multiple `a=p` puts with placement id `0`/unspecified create MULTIPLE
  COEXISTING placements; only two puts with the same NON-ZERO placement id replace each
  other. freminal wrongly merges any two same-image-id placements.
- **Scope:** `freminal-buffer/src/image_store.rs` (`next_placement_instance_id()`,
  `ImagePlacement.placement_instance`, `PlaceImageResult.placement_instance`),
  `freminal-buffer/src/buffer/images.rs` (`place_image`/`place_image_at` param;
  `clear_image_placements_by_placement`), the handlers (mint + thread the instance id;
  replace-clear on same non-zero `p=`), `freminal-terminal-emulator/src/terminal_handler/mod.rs`
  (`RealPlacement.placement_instance`), and the renderer
  (`freminal/src/gui/renderer/vertex.rs` bucket by `placement_instance`;
  `freminal/src/gui/renderer/gpu.rs` + `RenderState.image_draw_order`).
- **Approach:** a monotonic per-put placement-instance id (mirroring `next_image_id`)
  stamped on every cell of a placement; `build_image_verts` buckets by it. Same non-zero
  `p=` re-put clears the prior placement's cells first (`clear_image_placements_by_placement`).
  **Critical draw-order split:** `build_image_verts`'s `draw_order` is both the vertex-slab
  bucket key (must become per-instance) AND the GPU texture-lookup key (must stay per
  image id — `gpu.rs` `image_textures` is keyed by image id); it becomes a
  `Vec<ImageDrawEntry { instance_id, image_id }>` so the slab is per-instance while
  texture binding stays per-image-id. Per-instance bucketing also resolves the
  `z_index`/`source_crop` first-seen-collapse limitations (100.7b/100.9) for free.
- **Verification:** two same-image `p=0` puts render as two independent quads; a same
  non-zero `p=` re-put replaces (one quad); z-index/crop no longer collapse across
  placements; a `build_image_verts` Criterion before/after (the bench added in 100.17b),
  ≤15% regression.
- **Scheduling:** part of Task 100; blocks the v0.11.0-kitty merge. Landed as one combined
  pass with 100.19 + 100.20 (total site overlap).

#### 100.19 — Kitty sub-cell `X`/`Y` pixel offset on display

Full-spec-compliance gap: kitty `X=`/`Y=` (parsed as `cell_x_offset`/`cell_y_offset`)
shift an image's drawing origin within the top-left cell by that many pixels (< cell
size). Parsed but never applied on the display path.

- **Scope:** `freminal-buffer/src/image_store.rs` (`SubCellOffset` type +
  `ImagePlacement.subcell_offset`), `place_image`/`place_image_at` param,
  `freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs`
  (`resolve_subcell_offset`, display-actions-only, clamped `< cell` — mirroring
  `resolve_source_crop`), and `freminal/src/gui/renderer/vertex.rs` (`ImageBounds`
  field; additive quad-origin shift applied after `compute_image_quad_position`, defensively
  re-clamped against `cell_width`/`cell_height`).
- **Spec basis:** kitty graphics-protocol line 1121 (X/Y offset within the first cell,
  must be `< cell size`); KITTY-ONLY (iTerm2 and sixel have no sub-cell offset — confirmed).
- **Approach:** mirror the Task 100.9 `source_crop` wiring exactly; orthogonal to size mode
  (position only) and crop (UV only).
- **Verification:** an `X`/`Y` offset shifts the quad origin by that many pixels, clamped
  `< cell`; tests mirror the crop tests but assert position.
- **Scheduling:** landed with 100.18 (combined render-path pass). Corrects a prior wrong
  "out of scope" framing — full spec compliance requires it.

#### 100.20 — Placement-identity edge cases (virtual p=0 coexist; `d=i,p=` narrowing)

Two adjacent gaps completed alongside 100.18/100.19 (maintainer directed "do everything
now").

- **Virtual p=0 coexistence:** `VirtualPlacement` gains `placement_instance`, populated at
  registration and read by `handle_placeholder_char`, so two independent `a=p,U=1`/`a=T,U=1`
  registrations of the same image with `p=0` coexist as distinct placements (temporal
  disambiguation via register-then-write order). Scope:
  `freminal-common/src/buffer_states/unicode_placeholder.rs` (field) +
  `freminal-terminal-emulator/src/terminal_handler/*` (populate/read).
- **`d=i,p=` narrowing:** `handle_kitty_delete_by_id` currently ignores `p=` and clears all
  placements of the image; per spec, `d=i,p=<n>` deletes only that `(image_id, placement_id)`.
  Wire it to the `clear_image_placements_by_placement` method added in 100.18. Scope:
  `freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs`.
- **Verification:** two `p=0` virtual placements of one image coexist; `d=i,p=<n>` removes
  only the named placement while others survive.
- **Scheduling:** landed with 100.18/100.19.

##### 100.18 + 100.19 + 100.20 execution decisions (recorded 2026-07-02, fix landed `2370b84f`)

Implemented as one combined pass (total site overlap; the two new `ImagePlacement`
fields and the `place_image` param churn touch the same ~27 literals / ~37 call sites).
Discriminator: a monotonic `next_placement_instance_id()` (mirrors `next_image_id`)
stamped per put on `ImagePlacement.placement_instance`; `build_image_verts` buckets
`ImageBounds` by it. Same non-zero `p=` re-put clears the prior placement first via
`clear_image_placements_by_placement`; `p=0`/unspecified coexist (kitty spec lines
1105/1111). The **critical, easily-missed** piece the design recon surfaced: the
renderer's draw order was doing double duty as the vertex-slab key (now per-instance) and
the GPU texture-lookup key (must stay per image id — `gpu.rs` `image_textures` is keyed by
image id); it became a `Vec<ImageDrawEntry { instance_id, image_id }>`. Per-instance
bucketing resolved the `z_index`/`source_crop` first-seen-collapse limitations
(100.7b/100.9) for free. X/Y sub-cell offset (100.19) mirrors the 100.9 `source_crop`
wiring (a display-only `resolve_subcell_offset` clamped `< cell`, applied as an additive
quad-origin translation after `compute_image_quad_position`, orthogonal to size mode and
crop). 100.20 closed both flagged edge gaps: `VirtualPlacement.placement_instance` for
p=0 virtual coexistence, and `d=i,p=` delete narrowing. The same-z tie-break basis changed
from image-id order to instance-id/creation order (harmless; noted). `build_image_verts`
Criterion within noise (no regression). Fixes the live step-4 "huge image" bug (two
coexisting p=0 placements of image 2 merging into one stretched quad).

##### 100.15 + 100.16 execution decisions (recorded 2026-07-02, fix landed `091b1caa`)

Root cause confirmed exactly as the maintainer hypothesised and recon traced. `place_image`
now appends a fresh blank row below a tail-placed image and moves the cursor onto it, and
re-runs `enforce_scrollback_limit` after the append (re-deriving `base_row` by the same
delta) so the append can never push the primary buffer over its cap and the 100.14
post-drain `origin_row` invariant still holds. This honours the plan's cell-anchored
decision (line ~978) — the scroll path already carries image cells; only the cursor final
position was wrong. 100.16 (unmasked by 100.15) adds the `C=1` save/restore to
`place_kitty_image`'s `a=T`/Put branch, mirroring `stamp_kitty_put`; the recorded
real-placement origin is unchanged (only the cursor is restored). The two fixes shipped as
one atomic commit because 100.15 unmasks 100.16 and splitting would leave a
suite-red intermediate. Two durable test decisions: (1) the `grow_buffer_rows` test helper
was decoupled from `place_image` (now drives plain `handle_lf`) so unrelated
relative-placement tests do not inherit `place_image`'s cursor behaviour; (2) the 100.14
regression test's expected origin was recomputed for the two-stage drain 100.15 introduces
(a tail placement that drains always lands the append-row exactly at the cap, so it is
drained again by one) — `origin_row` in the drain regime is `max_rows - display_rows - 1`,
independent of the starting cursor row. New/strengthened tests: image survives a following
text write (buffer, 2 tests); `C=1` on a 2-row `a=T` genuinely restores the cursor;
cursor moves below the image when `C` is absent. Whether the animation (100.11) and
placeholder (100.13) symptoms are now resolved is pending a live re-test.

##### 100.14 execution decisions (recorded 2026-07-02, fix landed `004d2eb2`)

Root cause confirmed exactly as recon predicted. `Buffer::place_image` now returns
a `PlaceImageResult { scroll_offset, origin_row, origin_col }` (a new public struct
in `freminal-buffer/src/buffer/images.rs`, re-exported from `buffer/mod.rs`) instead
of a bare `usize` scroll offset. `origin_row` is the post-drain `base_row` and
`origin_col` is the pre-placement `start_col` — the image's true stamped top-left.
The two Kitty real-placement sites (`place_kitty_image`, `stamp_kitty_put`) record
that origin; `graphics_sixel.rs` / `graphics_iterm2.rs` consume `.scroll_offset` for
byte-identical behaviour. Chosen over the alternatives (hidden `last_placement_origin`
buffer field; changing the return to only the drained count) because the explicit
struct is the most testable and keeps `freminal-buffer` a pure, side-effect-free data
model. Regression test `kitty_real_placement_origin_survives_scrollback_drain` grows
the buffer past `scrollback_limit` so the placing call drains, and asserts the
recorded origin matches the actual stamped row (fail-before / pass-after verified).

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

#### 101.2 — Modifier bits: 8-bit arithmetic + `super` (encoding-only) (COMPLETE, 2026-07-05)

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

#### 101.3 — Encoding-only functional keys: F13–F35 + modifier-keys-as-keys (COMPLETE, 2026-07-05)

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

#### 101.4 — Escape-sequence docs (COMPLETE, 2026-07-05)

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

## Task 114 — Kitty Keyboard: egui-blocked keys (windowing layer)

> **Activated 2026-07-05.** Decomposed against the current code per
> `freminal-version-activation`. The durable decisions below (lock-query per
> platform, ambient-vs-transition model, evdev on Linux, new `App`-trait
> key-delivery seam, scoped `unsafe` for Win/macOS) are settled; the numbered
> subtasks (114.1–114.10) implement them. Branch: **`task-114/keyboard-egui-blocked`
> off `v0.11.0-kitty`**, its own PR (Task 114 is explicitly separate from the
> single v0.11.0 PR per the branch-model note).

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

- **True `caps_lock` / `num_lock` modifier state** (bits 64/128), plus
  **scroll-lock** state for the `ScrollLock`-as-key path below: no egui API — and,
  critically, **no winit API either.** winit 0.30.13's `ModifiersState` exposes only
  shift/ctrl/alt/super (split L/R); it has **no** `caps_lock`/`num_lock`/`scroll_lock`
  accessor, and `ModifiersChanged` never carries toggle state. winit _does_ deliver
  the key **press/release events** (`KeyCode::CapsLock`/`NumLock`/`ScrollLock`), so a
  local toggle bit can track _changes_ — but a press-tracked bit has **no correct
  cold-start value**: if the OS lock is already ON when freminal launches, the local
  bit defaults `false`, is silently **inverted** from reality, and stays wrong until
  the next physical lock-key press re-syncs it. That is a correctness bug, not a
  cosmetic one. **Mandatory 114 activation investigation: query the true OS lock
  state at startup (and, ideally, on focus-gain) per platform, and do it ourselves if
  winit/egui won't.** The OS-level query exists on every platform we support — X11
  `XkbGetState`, Win32 `GetKeyState(VK_CAPITAL/VK_NUMLOCK/VK_SCROLL)`, macOS
  IOKit / `CGEventSourceKeyState`. **Wayland is the known-hard case** and the
  investigation must resolve it explicitly (there is no portable Wayland lock-state
  query; determine the real answer — a compositor protocol, a fallback, or a
  documented degradation — rather than assuming). Do NOT ship the lock bits driven by
  a cold-start guess.
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
- **The intercept seam already exists (recon 2026-07-05).** `Handler::window_event`
  in `freminal-windowing/src/event_loop.rs` is the single chokepoint where raw
  `WindowEvent::KeyboardInput` / `ModifiersChanged` are observed before egui. There
  is already precedent for peeking a raw key event and conditionally bypassing egui:
  the Wayland clipboard-paste interceptor at `event_loop.rs:357-400` reads
  `winit::event::KeyEvent.logical_key` and `return`s early (`:396`) without calling
  `state.egui.on_window_event(...)` (`:404`); the mouse-motion short-circuit at
  `:329-348` is a second precedent. A 114 intercept extends this pattern. **Both
  existing precedents are _narrow_** (egui stays the primary translator; only the
  special-cased event is swallowed) — a "raw-winit-primary" rewrite is a much bigger,
  riskier change and is NOT what these precedents establish.
- **winit 0.30.13 _does_ carry every dropped key.** `PhysicalKey::Code(KeyCode::…)`
  distinguishes all numpad operators/directional keys, media keys, lock keys,
  PrintScreen, Pause, ContextMenu, and ISO-level keys (verified against the vendored
  winit source, 2026-07-05). The block is egui 0.35's `Key` enum, not winit —
  delivery is achievable, matching "the hard part is delivery, not encoding".
- **Downstream is already wired.** `KeyModifiers` already has the
  `super_key`/`hyper`/`meta`/`caps_lock`/`num_lock` fields, and
  `InputEvent::Key` / `TerminalInput::to_payload` already encode the full kitty
  functional-key table. A 114 intercept changes only how keys are observed/classified
  in `freminal-windowing`; nothing below the GUI input layer (channel, snapshot,
  `ArcSwap`, PTY thread) changes.
- **This is why v0.11.0 keyboard is "substantially compliant, not 100%".** The gap
  is explicit and tracked, not silent.

### 114 Open questions (decide at activation)

- Raw-winit intercept vs egui/egui-winit upgrade — which, and what is the
  regression surface for the existing input path?
- Can the intercept be scoped to only the missing keys, leaving egui-winit as the
  primary translator for everything else, to minimize risk? (The existing
  `event_loop.rs` precedents are all narrow; a raw-winit-**primary** rewrite is the
  only variant that would justify resequencing 114 before 101 — decide explicitly
  whether narrow or primary, and record why.)
- Are `hyper`/`meta` modifiers ever reachable on any target platform, or do they
  stay permanently `0`?

### 114 Mandatory activation investigation: OS lock-state query (blocking subtask)

**This is a required first subtask when Task 114 activates, not an optional
question.** The `caps_lock`/`num_lock`/`scroll_lock` bits MUST reflect the true OS
state, including at cold start — a press-tracked local bit alone is a correctness
bug (silent inversion; see "In scope" above). Neither winit nor egui exposes the
toggle state, so freminal queries the OS directly. The investigation must, per
platform we support, determine and prototype the query:

- **X11:** `XkbGetState` (or `XkbGetIndicatorState`) — read `locked_mods`
  caps/num/scroll bits. Confirm the crate path (`x11rb`/`x11-dl`) and whether it can
  run alongside winit's X11 connection without conflict.
- **Windows:** `GetKeyState(VK_CAPITAL)` / `VK_NUMLOCK` / `VK_SCROLL`, low bit =
  toggle state. Trivial; confirm the `winapi`/`windows` binding already available.
- **macOS:** IOKit HID element state or `CGEventSourceKeyState` for the lock
  modifiers; confirm reachability and entitlement/sandbox implications.
- **Wayland (KNOWN HARD — resolve explicitly, do not hand-wave):** there is no
  portable client-side lock-state query. Investigate whether a compositor protocol
  (or libxkbcommon state derived from the seat's keymap + modifier events winit
  already forwards) can supply it, and if not, decide and DOCUMENT the honest
  fallback (e.g. press-tracked-only with a stated cold-start caveat on Wayland,
  correct everywhere else). The deliverable is a real answer for Wayland, not an
  assumption.

Deliverable of the investigation subtask: a per-platform query plan (with the
concrete API + crate for each), the Wayland resolution, where the query is invoked
(startup + focus-gain into `WindowState`), and how the queried truth reconciles with
the press-event tracking that maintains it thereafter. Any new system-level
dependency triggers `flake-dev-shell-discipline` (add to `flake.nix`, stop, wait for
`nix develop`). Only after this lands does the rest of Task 114 wire the bits.

### 114 Durable decision: per-platform lock-state query resolved (recorded 2026-07-05)

The mandatory investigation above is **resolved**. Findings and the
maintainer-approved decisions (these supersede the "investigate" framing; the
first implementation subtask _consumes_ these, it does not re-derive them):

- **Linux (X11 AND Wayland): `evdev` / `EVIOCGLED` kernel LED read — ONE code path
  for both display servers.** Rationale (the key finding): Wayland has **no**
  client-side global lock-state query by protocol design; the absolute
  `wl_keyboard.modifiers.mods_locked` value IS delivered on every focus-enter, but
  **winit owns the Wayland `wl_keyboard` and discards the `locked` field** before it
  reaches freminal — so neither `xkbcommon` (a pure state-machine/keymap library
  that can only _interpret_ events it is fed) nor any second Wayland client (a
  non-focused snooping surface never receives `modifiers`) can recover it. Reading
  the kernel LED state via `evdev` sidesteps the display server entirely and works
  identically under X11 and Wayland. **Chosen over** X11 `XkbGetState`/`x11rb` XKB
  (X11-only, would still leave Wayland unsolved) and over `xkbcommon` (cannot
  cold-start-query Wayland at all). Confirmed in the dev environment: LED nodes
  readable, user in `input` group, `numlock=1`/`caps=0`/`scroll=0` read correctly.
  - **Crate: `evdev` 0.13.2** (the pure-Rust `cmr/evdev`, NOT the libevdev-FFI
    `evdev-rs`). Pure Rust over `libc`/`nix`; **no system library**, so
    `flake-dev-shell-discipline` does NOT fire — plain Cargo.toml add. Root
    `workspace.dependencies`: `evdev = "0.13.2"` (alphabetical, full pin);
    `freminal-windowing/Cargo.toml`:
    `[target.'cfg(target_os = "linux")'.dependencies]` → `evdev = { workspace = true }`.
  - **API:** `evdev::enumerate() -> impl Iterator<Item=(PathBuf, Device)>`;
    filter by `Device::supported_leds()` containing the LED; read
    `Device::get_led_state() -> io::Result<AttributeSet<LedCode>>`; variants
    `LedCode::LED_CAPSL` / `LED_NUML` / `LED_SCROLLL` (note the doubled `L` on
    scroll). Read-only `Device::open` suffices; `get_led_state()` is a single
    synchronous `EVIOCGLED` ioctl (NOT a blocking event read) — safe to call at
    startup + focus-gain, not in a per-keystroke hot path.
  - **Aggregation: OR across all LED-capable devices** (the machine here has 29
    input nodes; most are non-keyboards). Lock-LED state is kernel-synchronized
    across physical keyboards, so OR-ing is safe. Do NOT "pick the first device."
    Re-enumerate on each query rather than caching the device list (hotplug).
- **Windows: `GetKeyState(VK_CAPITAL/VK_NUMLOCK/VK_SCROLL)`**, low bit = toggle,
  via the existing `winapi` workspace dep (add the `"winuser"` feature); no new
  crate. Focus-independent system query.
- **macOS: Caps Lock only** via `CGEventSourceFlagsState` /
  `kCGEventFlagMaskAlphaShift` (raw framework FFI). **Num Lock / Scroll Lock are
  hardcoded `false`** (the concept does not exist on Mac keyboards). The
  Input-Monitoring TCC-permission question for the polling API is UNRESOLVED from
  docs and must be verified on a real target macOS build before that platform's
  subtask is accepted; macOS is the lowest-priority platform and may ship the
  caps-only path or defer with a documented caveat.
- **`unsafe` FFI is unavoidable** on Windows/macOS (and `evdev` uses `unsafe`
  internally but exposes a safe API — Linux needs no `unsafe` in freminal). Per the
  "no unsafe unless explicitly requested" rule, the Windows/macOS query modules are
  scoped, `# Safety`-documented `unsafe extern` blocks mirroring the existing
  `graphics_kitty.rs` / `platform.rs::system_beep` precedent. **Linux (the primary
  path) needs no freminal-side `unsafe`.**

### 114 Durable decision: lock state is ambient; key events are transition-only (recorded 2026-07-05)

This resolves the "how do we emit kitty events given a cold-start + focus-gain
query" question and is **binding** — a sub-agent must NOT "helpfully" add a
synthetic-release resync. The kitty keyboard protocol is a **transition-reporting**
protocol, not a **state-diffing** one. Two concepts that look similar must be kept
strictly separate:

1. **Lock-state modifier bits (`caps_lock`=64, `num_lock`=128) reported _alongside_
   another key's report.** These are an _ambient snapshot_ — "what was the lock
   state when this key was pressed" — never an event in themselves. They only ever
   appear decorating a real key report. The OS query (cold start + focus-gain)
   exists **solely** to keep this snapshot correct.
2. **Key _events_ for the lock/modifier keys themselves** (`CapsLock`-as-key 57358,
   `Super`-as-key 57441/57447, etc., with the `:1`/`:2`/`:3` press/repeat/release
   suffix under flag 8 + flag 2). These are emitted **only** from physical
   `WindowEvent::KeyboardInput` transitions the terminal actually observed **while
   focused**.

**Binding rules:**

- **Never synthesize a press/release from a state delta.** A `release` means the
  terminal physically saw the key go up. Emitting a fabricated
  `super released` / `caps_lock released` on focus-gain because the queried state
  differs from tracked state is _inventing a transition that never happened_ — it
  corrupts flag-8 event consumers (games, modal editors). The protocol has **no
  resync/"state changed" event**; do not add one.
- **Never track or report keys while unfocused.** An unfocused terminal's
  keystrokes belong to whatever surface _is_ focused; reporting them injects
  phantom input into the PTY. **No background keyboard-polling thread** (it would
  also violate the lock-free architecture by adding a second keyboard-state writer
  off the PTY/GUI threads, and still only produce fabricated deltas). The rejected
  alternatives — "emit events on focus-gain where prior state differs" and "poll
  keyboard state on a background thread while unfocused" — are **both rejected** for
  these reasons.
- **Cold start + focus-gain:** query OS lock state, update the ambient caps/num
  bits, **emit nothing**. If a lock was toggled while unfocused, the focus-gain
  requery silently corrects the ambient snapshot — no event.
- **While focused:** real key transitions update tracking _and_ emit events (under
  the active flags). Lock-key transitions additionally update the ambient bits.
- **Focus-loss:** clear the _held-keys tracking set_ (so no stale "held" state
  leaks into later reports), **emit nothing** (no synthetic releases for the held
  set). Ambient lock bits are left as-is; they are re-queried on the next
  focus-gain regardless.
- **Only the _locks_ are OS-queried; the chorded modifiers are not.** The
  Shift/Ctrl/Alt/Super _held-for-decoration_ modifiers keep coming from
  winit/egui's `ModifiersChanged`, which the compositor already re-delivers on
  focus-enter carrying current pressed-modifier state (correct-on-focus-enter by
  protocol). Do NOT route the chorded modifiers through the xkb/OS lock query — the
  query is scoped to `caps_lock`/`num_lock` (and `scroll_lock` for the
  `ScrollLock`-as-key path) only.

Rationale: the caps/num bits are read-on-use ambient state, so a silent
focus-gain correction is invisible and correct; the held-for-decoration modifiers
are compositor-refreshed on focus-enter; and transition events stay honest by only
ever mirroring real observed key up/down while focused.

### 114 Durable decision: key-delivery seam + unsafe scope (recorded 2026-07-05)

- **Both halves ship this cycle:** (A) the true `caps_lock`/`num_lock` bits, and
  (B) delivering the egui-dropped keys (keypad operators/directional, media,
  ISO-level shifts, lock/print/pause/menu-as-keys). egui 0.35's `Key` enum has **no
  variant** for the half-B keys, so the `inject_paste`/`raw_input_hook` route is
  impossible — a raw key must be delivered to freminal **outside** egui.
- **Delivery seam = a new `App`-trait method** (maintainer-approved), NOT a
  `RefCell` side-channel. Add
  `fn on_raw_key_event(&mut self, window_id: WindowId, event: &winit::event::KeyEvent, mods: RawKeyMods)`
  (default empty body) to `trait App` in `freminal-windowing/src/lib.rs`, mirroring
  the `on_close_requested` / `on_window_created` precedent. It is called from
  `Handler::window_event`'s `WindowEvent::KeyboardInput` arm **only** for the
  blocked physical-key set (a narrow intercept — egui-winit stays the primary
  translator for everything else, matching the existing paste/mouse narrow-intercept
  precedents in `event_loop.rs`). The `RawKeyMods` payload carries the chorded
  Shift/Ctrl/Alt/Super state (from `state.egui.modifiers()`, already available) so
  the GUI can encode; the ambient lock bits are read GUI-side from the lock-state
  cache (half A), NOT passed here.
- **Architecture note (`freminal-architecture`):** this changes only how keys are
  _observed/classified_ in `freminal-windowing`; nothing below the GUI input layer
  changes. The GUI's `App::on_raw_key_event` impl builds `Vec<TerminalInput>` and
  routes through the existing `send_terminal_inputs` funnel → `InputEvent::Key`
  channel → PTY thread. No new GUI↔PTY channel; the lock-free snapshot/`ArcSwap`
  transport is untouched. Reviewed and signed off at activation.
- **Scoped `unsafe` approved (maintainer) for Windows/macOS query modules only.**
  `GetKeyState` (Windows) and `CGEventSourceFlagsState` (macOS) are wrapped in
  scoped, `# Safety`-documented `unsafe extern` blocks mirroring
  `freminal/src/gui/platform.rs::system_beep` and
  `terminal_handler/graphics_kitty.rs`. **The Linux/evdev path (the primary target)
  uses `evdev`'s safe API and needs NO freminal-side `unsafe`.**

### 114 Subtask decomposition (activated 2026-07-05)

Ordering: the mandatory lock-query investigation is resolved (decisions above), so
114.1 is the Linux lock-query implementation (no audit subtask needed — the recon
already produced the seam map). Half A (lock bits, 114.1–114.4) and half B (key
delivery, 114.5–114.8) are largely independent but both touch
`freminal/src/gui/terminal/input.rs` (the `egui_mods_to_key_modifiers` /
`write_input_to_terminal` region) and `freminal-windowing`, so they are **staggered,
not parallel-on-the-same-file** (one active editor per shared region at a time), per
`freminal-orchestrator-protocol`. Each subtask stops at its review gate; each leaves
`cargo test --all` green.

#### 114.1 — Linux lock-state query (evdev) in `freminal-windowing`

Scope: workspace `Cargo.toml` (add `evdev = "0.13.2"` to `workspace.dependencies`,
alphabetical, full pin); `freminal-windowing/Cargo.toml` (add
`[target.'cfg(target_os = "linux")'.dependencies]` → `evdev = { workspace = true }`);
a **new** module `freminal-windowing/src/lock_state.rs`.

What: add a `LockState { caps: bool, num: bool, scroll: bool }` struct and a
`query_lock_state() -> LockState` free function. Linux impl: `evdev::enumerate()`,
filter devices whose `supported_leds()` contains `LedCode::LED_CAPSL` (etc.), read
`get_led_state()` on each, **OR** the results across all LED-capable devices (do NOT
pick the first; re-enumerate each call for hotplug). Non-Linux impl (this subtask):
a `#[cfg(not(target_os = "linux"))]` stub returning `LockState::default()` (all
false) so the crate compiles on every platform — Windows/macOS real queries are
114.2/114.3. No `unsafe` in this subtask.

Deliverable: the module + a Linux `#[cfg(target_os = "linux")]` test that calls
`query_lock_state()` and asserts it returns without error (state value is
environment-dependent, so assert the call succeeds / does not panic; a
device-capability-filter unit test on a synthetic `AttributeSet` if feasible).

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT add Windows/macOS real queries (114.2/114.3); do NOT wire the
result into `KeyModifiers` yet (114.4); do NOT touch the key-delivery half; do NOT
proceed to 114.2. NO `unsafe`.

Stop: report files changed + verification; await review.

#### 114.2 — Windows lock-state query (`GetKeyState`)

Scope: `freminal-windowing/src/lock_state.rs` (the `#[cfg(target_os = "windows")]`
arm), `freminal-windowing/Cargo.toml` (add a
`[target.'cfg(target_os = "windows")'.dependencies]` block with
`winapi = { workspace = true, features = ["winuser"] }` — reuse the existing
workspace `winapi` pin).

What: implement `query_lock_state()` for Windows via
`GetKeyState(VK_CAPITAL/VK_NUMLOCK/VK_SCROLL)`, `& 0x0001` for the toggle bit, in a
scoped `# Safety`-documented `unsafe` block. Replaces the 114.1 stub on Windows.

Deliverable: the Windows arm + (if a Windows toolchain/CI is reachable) a
`#[cfg(target_os = "windows")]` smoke test; otherwise ensure it compiles under a
windows target check and the POSIX suite stays green.

Verification: `cargo test --all` (Linux); `cargo clippy --all-targets --all-features -- -D warnings`;
`cargo check --target x86_64-pc-windows-gnu` if the target is installed, else careful
cfg review.

Prohibitions: do NOT touch the Linux path; do NOT add macOS (114.3); do NOT weaken
`unsafe` documentation; do NOT proceed.

Stop: report + await review.

#### 114.3 — macOS lock-state query (Caps Lock only)

Scope: `freminal-windowing/src/lock_state.rs` (the `#[cfg(target_os = "macos")]`
arm), `freminal-windowing/Cargo.toml` if a framework-link is needed.

What: implement Caps Lock via `CGEventSourceFlagsState` /
`kCGEventFlagMaskAlphaShift` in a scoped `# Safety`-documented `unsafe extern`
block linking the `CoreGraphics` framework (mirroring `platform.rs::system_beep`'s
`NSBeep` pattern). `num`/`scroll` hardcoded `false` with a one-line rationale
comment. **Flag the Input-Monitoring TCC-permission risk in a code comment**; if the
query triggers a permission prompt on the target macOS version, that is a
known-caveat to record (do not silently degrade).

Deliverable: the macOS arm; compiles under a macOS target check (real runtime
verification of the permission behaviour is deferred to a maintainer on-device pass
and noted as such).

Verification: `cargo test --all` (Linux stays green); clippy; `cargo check --target
aarch64-apple-darwin` if available, else careful cfg review.

Prohibitions: do NOT attempt num/scroll on macOS; do NOT touch Linux/Windows; do
NOT proceed.

Stop: report + await review, explicitly surfacing the TCC-permission caveat.

#### 114.4 — Wire lock state into the GUI ambient cache + `KeyModifiers` (half-A integration)

Scope: `freminal-windowing/src/event_loop.rs` (add a `WindowEvent::Focused(true)`
handler that calls `query_lock_state()`; query once at window creation too — see
`on_window_created` / `resumed`); the `App` trait / a delivery of `LockState` to the
GUI (a new `App` method `fn on_lock_state(&mut self, window_id, LockState)` OR store
on `WindowState` and expose — Opus decides: use an `App` callback
`on_lock_state(window_id, LockState)` for symmetry with 114.5's `on_raw_key_event`);
`freminal/src/gui/**` GUI-side ambient cache (mirror the existing `super_held`
pattern in `freminal/src/gui/terminal/input.rs`/`widget.rs`), and
`egui_mods_to_key_modifiers` in `freminal/src/gui/terminal/input.rs:168-181`
(populate `caps_lock`/`num_lock` from the cache instead of hardcoded `false`).

What: freminal queries lock state at cold start + on focus-gain, caches it GUI-side
(ambient), and `egui_mods_to_key_modifiers` reads the cache to set the
`caps_lock`/`num_lock` bits on every emitted `KeyModifiers`. Also update the ambient
cache from observed CapsLock/NumLock key-down transitions while focused (per the
ambient/transition decision). **Emit no events on requery** (ambient only). Clear
the held-keys set on focus-loss (the held-set clearing pairs with 114.6; if the
held-set does not yet exist here, add the focus-loss clear hook as a no-op stub and
note it for 114.6).

Deliverable: caps/num bits now reflect true OS state at cold start and after
focus-gain; a test that `egui_mods_to_key_modifiers` maps a cached caps/num into the
right `KeyModifiers` bits; existing modifier tests stay green.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT synthesize any key event from a lock-state change (binding
decision); do NOT start half B's key delivery (114.5); do NOT proceed.

Stop: report + await review.

#### 114.5 — Key-delivery seam: `App::on_raw_key_event` + narrow winit intercept

Scope: `freminal-windowing/src/lib.rs` (add `on_raw_key_event` to `trait App` with a
default empty body; add a `RawKeyMods` type carrying shift/ctrl/alt/super),
`freminal-windowing/src/event_loop.rs` (`Handler::window_event` `KeyboardInput` arm:
match the blocked `PhysicalKey::Code(KeyCode::…)` set — keypad operators/directional,
media, ISO-level, lock/print/pause/menu — and call `app.on_raw_key_event(...)`
BEFORE handing to egui, `return`-ing early like the paste precedent; everything else
falls through to egui unchanged).

What: land the delivery seam only. The blocked-key set is an explicit `matches!` on
the exact `KeyCode` variants. The GUI impl of `on_raw_key_event` is an inert
placeholder in this subtask (log at `trace!`) — encoding is 114.6/114.7. This keeps
the architecture change (new trait method + narrow intercept) isolated and
reviewable.

Deliverable: the trait method + `RawKeyMods` + the narrow intercept + an inert GUI
impl; a windowing-level unit/doc test that the intercept classifies a blocked
`KeyCode` (and does NOT intercept a normal letter key). Existing input tests green.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT encode kitty bytes yet (114.6/114.7); do NOT let the intercept
swallow keys egui still needs (only the blocked set); do NOT proceed.

Stop: report + await review (this is the `freminal-architecture` sign-off gate for
the input-path change).

#### 114.6 — Functional-key codepoint tables (keypad/media/ISO/lock/print/pause/menu)

Scope: `freminal-terminal-emulator/src/input.rs` (add the missing `const … _CODEPOINT`
values and a generic `TerminalInput::KittyFunctional(u32, KeyModifiers)` variant OR
per-group variants — Opus decides: a single generic
`TerminalInput::KittyFunctional { codepoint: u32, mods: KeyModifiers }` variant,
encoded via the existing `build_csi_u` in `to_payload_kkp`, mirroring the F13–F35
arm at `input.rs:1018`).

What: type the codepoint tables the recon found MISSING — keypad operators/directional
(57399–57427), media (57428–57440), ISO_Level3/5_Shift (57453/57454),
CapsLock/ScrollLock/NumLock/PrintScreen/Pause/Menu-as-keys (57358–57363) — as
`const` codepoints, and route them through `build_csi_u` (which already exists; no
encoding-mechanism change). Pure encoding-layer addition; no delivery wiring yet.

Deliverable: the variant + codepoints + exhaustive `to_payload_kkp` unit tests
(one representative key per group, asserting the exact `CSI <cp>;<mods> u` bytes,
under flags 1/2/8 as appropriate).

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT wire the GUI delivery (114.7); do NOT change `build_csi_u`; do
NOT proceed.

Stop: report + await review.

#### 114.7 — GUI: encode delivered raw keys → `TerminalInput` (half-B integration)

Scope: `freminal/src/gui/**` — the real impl of `App::on_raw_key_event` (replace the
114.5 placeholder): map the intercepted `winit` `KeyCode` + `RawKeyMods` + the
ambient lock cache (114.4) + KKP flag state to the `TerminalInput::KittyFunctional`
codepoints (114.6), build the `KeyEventMeta` (press/repeat/release from the winit
`KeyEvent`), and route through the existing `send_terminal_inputs` funnel in
`freminal/src/gui/terminal/input.rs`. Honour the ambient/transition decision:
these keys emit events only while focused (the intercept only fires for the focused
window by winit contract); lock-as-keys ALSO update the ambient cache on key-down.

What: turn delivered raw keys into encoded kitty bytes on the existing input path.
Respect the KKP flags (only emit functional-key escapes when the relevant flag is
set; fall back to legacy behaviour otherwise, matching how egui-delivered keys
behave today).

Deliverable: end-to-end encoding for the blocked keys + tests (feed a synthetic
`on_raw_key_event` for a keypad/media/lock key under flag 8 and assert the bytes on
the input channel); existing keyboard tests green.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT synthesize releases from focus/lock deltas; do NOT add a new
channel; do NOT proceed.

Stop: report + await review.

#### 114.8 — Focus-loss held-key reset (transition-model correctness)

Scope: `freminal/src/gui/terminal/input.rs` (+ `widget.rs` if the held-set lives
there), `freminal-windowing/src/event_loop.rs` `WindowEvent::Focused(false)` if a
hook is needed.

What: on focus-loss, clear the held-keys tracking set (so no stale "held" state
leaks into later reports) and **emit nothing** — no synthetic releases. This
finalizes the transition-only decision. (If 114.4/114.7 already added the clear, this
subtask verifies + tests it and is a no-op-or-test-only pass.)

Deliverable: held-set reset on focus-loss + a test asserting no bytes are emitted on
focus-loss and that post-focus-gain the first real key rebuilds tracking cleanly.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT emit synthetic releases; do NOT proceed.

Stop: report + await review.

#### 114.9 — Escape-sequence dual-doc + reference update

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`,
`Documents/KITTY_PROTOCOL_REFERENCE.md`.

What: flip the Task-114-tracked gaps (keypad/media/ISO/lock/print/pause/menu keys,
`caps_lock`/`num_lock` bits) from tracked-gap to implemented; update the kitty
keyboard rows in COVERAGE; refresh both "Last updated" headers; update the reference
doc's keyboard current-state deltas. Note the remaining honest caveats
(`hyper`/`meta` bits stay `0` — no platform source; macOS num/scroll `false`; macOS
TCC caveat) rather than over-claiming.

Deliverable: dual-doc + reference update.

Verification: `markdownlint-cli2` clean; prettier clean.

Prohibitions: do NOT over-advertise (hyper/meta, macOS num/scroll); none beyond
scope.

Stop: report + await review.

#### 114.10 — MASTER_PLAN + plan status update

Scope: `Documents/MASTER_PLAN.md` (Task 114 status row + completion tracking table),
`Documents/PLAN_VERSION_110.md` (Task Summary table status; this decomposition's
completion notes).

What: mark Task 114 complete with the branch name and subtask commit range; note the
carried caveats (hyper/meta, macOS).

Deliverable: status updates.

Verification: `markdownlint-cli2` clean.

Prohibitions: none beyond scope.

Stop: report + await review; then the Task 114 PR (`task-114/keyboard-egui-blocked`
→ `main`, separate from the v0.11.0 PR).

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
