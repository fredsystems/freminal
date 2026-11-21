# PLAN_VERSION_130_0_b.md — v0.13.0 "Foundations, Safety & Small Conformance"

Part of v0.13.x "Kitty: Full Protocol Compliance". The compliance audit, common-work
analysis, reference specs and cross-milestone design decisions live in the overview,
`PLAN_VERSION_130.md`. This document holds the second half of the v0.13.0 tasks
(138–140, 147–151); the foundations (126–132) and the v0.13.0 execution model are in
`PLAN_VERSION_130_0.md`. The two files are split only to stay under the repository's
file-size limit.

**Status:** Complete.

The foundation tasks (CSI dispatch, wire infrastructure, reverse path, screen-scoped state,
colour), the safety gate, the small conformance tasks, the security and fuzz remediation, the
notification backend work and freminal's own `TERM`. Every task here is complete. The subtask
breakdowns, reviews and status notes are kept as the record of how the foundations were built,
while the later point releases still build on them.

## Tasks

| #   | Task                                        | Milestone | Scope | Status        | Depends on                         |
| --- | ------------------------------------------- | --------- | ----- | ------------- | ---------------------------------- |
| 138 | Desktop Notifications (OSC 99) Conformance  | v0.13.0   | L     | Complete      | 129, 130                           |
| 139 | Pointer Shapes (OSC 22) Completion          | v0.13.0   | M     | Complete      | 129, 130, 131                      |
| 140 | Underline & SGR Parity                      | v0.13.0   | S     | Complete      | None                               |
| 147 | v0.13.0 Correctness Cleanup                 | v0.13.0   | L     | Complete      | 130, 131                           |
| 148 | Security & Fuzz Findings Remediation        | v0.13.0   | XL    | Complete      | None                               |
| 149 | zbus 6 Migration & Notification Backends    | v0.13.0   | M     | Complete      | 138                                |
| 150 | Desktop Notification Backend Extraction     | v0.13.0   | L     | Complete      | 149                                |
| 151 | Own TERM & Terminfo Distribution            | v0.13.0   | M     | Complete      | 140                                |

---

## Task 138 — Desktop Notifications (OSC 99) Conformance

### 138 Summary

**Goal.** Close every OSC 99 deviation, so freminal matches the kitty desktop-notification
spec (kitty 0.49.2): parse and reassembly fidelity, a correct reverse path, the full lifecycle
(update in place, close, activation followed by close, expiry, focus), presentation (icons,
sounds, urgency, category, markup), user filtering, and a `p=?` reply that is truthful for
every routing and platform.

Activated and decomposed on 2026-10-12 on `task-138/osc99-conformance`, branched from `main`
at `8c040150` (after PR #541) and since rebased onto `724b291c` (after PR #551). The branch
contains a `chore(deps): cargo update` commit (`794a7980`) that is not Task 138 work. Closes
130.C4 and the 147 routing note on the saturated 128-id map.

### 138 Activation recon (2026-10-12)

Four read-only passes: the PTY-side data model, parser and reassembly; the GUI routing,
notify-rust usage and toast leg; the notify-rust 4.18.1 / zbus 5.19.0 source; and kitty's
reference implementation (`kitty/notifications.py`, `constants.py`, `xdg.py`,
`glfw/linux_notify.c`) for the points the spec leaves to the implementation.

Every stub finding was confirmed, except one: **chunked base64 is already decoded as one
stream** (Task 129's `BoundedChunkAssembler`, tests in `notify_99.rs`). Findings beyond the
stub:

- **Parse.** An invalid value for any known key (`d=2`, `c=2`, `u=3`, `w<-1`, bad base64 in
  `f`/`n`/`s`/`t`, a bad id character) drops the whole sequence. Dropping a middle chunk this
  way silently truncates the transfer. An empty `a=` clears both actions. A plain payload that
  is not UTF-8 drops the sequence. Ids have no length cap.
- **Reassembly.** Control payloads (`p=close` / `p=alive` / `p=?`) wait for `d=1` and occupy a
  pending slot. A saturated map also refuses `p=?`. A final control chunk for an id with
  pending content discards that content, because routing uses the final chunk's `p=`.
- **Reverse path.** No `"default"` action is registered, so a click on the notification body
  may never be reported on many daemons. `p=alive` without an id replies with `i=0`; kitty does
  not reply. The live-id map is global across panes, so one pane sees and can forget another
  pane's ids.
- **Lifecycle.** notify-rust's `wait_for_action` is one-shot and consuming, and `show()` opens
  a D-Bus connection per notification. A close after an activation, update in place and
  programmatic close are impossible through its public API. macOS and Windows backends have no
  `close()`, `update()` or `id()`.
- **Presentation.** The icon temp file is deleted as soon as `show()` returns, which races the
  daemon reading it. `s=system` is forwarded as the sound name `"system"`. On Windows, no
  `s=` means _no sound_, because notify-rust maps a missing sound name to a silent toast. The
  body is passed to daemons that interpret markup. A missing title duplicates the body into
  both summary and detail. Buttons are uncapped.
- **Advertisement.** `p=?` advertises `w=1` (OS-advisory only), `s=system,silent` (`silent`
  not honoured), `p=close` (does nothing), `u=0,1,2` on macOS (no urgency there), `c=1` on
  macOS/Windows (the "close" is fabricated at display) and never `a=focus`.
- **GUI.** Pane identity is dropped before routing, so `o=` uses OS-window focus only. A failed
  `RefCell` borrow silently drops a frame's OSC 99 notifications. The toast leg has no keyed
  lifecycle, no per-toast lifetime and no buttons.
- **Tests that pin non-conformant behaviour** (updated by the subtasks that change it):
  `a_equals_report_report_true_focus_false`, `a_equals_minus_report`,
  `empty_value_for_actions_clears_both_flags`, `p_equals_unknown_ignored_stays_title`,
  `d_equals_two_invalid_integer`, `u_equals_three_invalid`, `w_equals_minus_two_invalid`,
  `empty_value_for_flag_keys_is_invalid_integer`, `i_equals_illegal_char_returns_invalid_id`,
  `e_equals_zero_non_utf8_payload_is_rejected`, `unidentified_non_final_never_merged`,
  `reassembly_caps_number_of_pending_notifications`, `tombstones_count_toward_pending_cap`,
  the `query_reply_body_*` tests, `tests/osc99_query.rs` `TAIL`, and the
  `route_osc99_occasion_*` tests.

### 138 Decisions (maintainer, 2026-10-12)

1. **Linux/BSD system leg moves to a zbus service.** `zbus` becomes a direct dependency on
   `cfg(all(unix, not(target_os = "macos")))`. One dispatcher thread owns one D-Bus
   connection and calls `Notify`, `CloseNotification` and `GetCapabilities`. One signal-pump
   thread forwards `ActionInvoked` and `NotificationClosed`. They talk to the GUI over two
   GUI-internal crossbeam channels (commands in, events out, with a repaint wake). No new PTY
   channel. OSC 9 / OSC 777 / command-finished notifications stay on notify-rust.
2. **macOS and Windows: truthful advertisement only.** No new lifecycle code on those
   platforms. Their system leg is _untracked_: no activation reports, no close tracking (a
   `c=1` request gets `p=close;untracked` at display, as kitty does on macOS), no update, no
   programmatic close, no expiry. `p=?` omits whatever the routing cannot honour (see the leg
   capability rules below).
3. **The toast leg gets full lifecycle parity.** Update in place by id, close by id, a
   per-toast lifetime from `w=`, rendered clickable buttons (1-based reports), body click →
   activation (report and focus), dismiss / expiry / eviction / app close → close report.
4. **User filtering is a `location:regex` rule list**: `[notifications] osc99_filters`, kitty
   semantics (regex _search_, any match drops silently, no reply), locations `title`, `body`,
   `app`, `type`, plus the bare rule `all`. No boolean `and`/`or`/`not`. Invalid rules are
   logged and ignored.

### 138 Decisions (orchestrator, recorded so they are not re-litigated)

freminal is MIT and kitty is GPLv3: kitty is read for observable behaviour only, no code is
copied. Where the spec is silent kitty's behaviour is the tie-breaker; every place freminal
deliberately differs from kitty is marked **(differs from kitty)**.

**Parsing (PTY thread).**

- An invalid value for a known key **ignores that key**; the rest of the sequence still
  applies (spec line 826 allows this; kitty does it). `c=` and `d=` are true for any value
  other than `0`; `e=` is base64 only for `1`; an unparsable `u=` is ignored; `w=` is clamped
  to `max(-1, v)` and an unparsable `w=` is ignored; a `f`/`n`/`s`/`t` value that is not
  base64 UTF-8 is ignored.
- A malformed metadata token (no `=`, or a key that is not exactly one byte) is ignored, not a
  sequence error. Unknown keys are ignored, as today.
- `i=` and `g=` are **sanitised, not rejected**: characters outside `[A-Za-z0-9_+.-]` are
  removed and the result is truncated to 512 bytes (kitty). An id that is empty after
  sanitisation is treated as absent. `i=0` is an ordinary id (the spec only tells
  applications not to use it).
- An empty `a=` changes nothing. `a=` tokens are **edits applied in arrival order**, starting
  from the default `{focus}`; `report`/`focus` add, `-report`/`-focus` remove, unknown words
  are ignored. Across chunks the edits keep accumulating **(differs from kitty**, whose union
  merge lets a later chunk without `a=` resurrect a removed `focus`).
- An unknown `p=` value is its own payload type: its payload is discarded and its metadata
  and `d=` still apply.
- A plain payload that is not valid UTF-8 is decoded lossily (U+FFFD), not rejected.
- The per-chunk 1 MiB cap stays. Receiving larger chunks than the spec's 2048 / 4096-byte
  sender limit is receiver leniency, not a defect.

**Reassembly (PTY thread).**

- **Control payloads act immediately, whatever `d=` says**, and never enter or touch the
  pending map (kitty acts immediately but also discards the window's pending command;
  freminal keeps it **(differs from kitty)**).
  - `p=?` is answered (id-less replies use `i=0`, as today).
  - `p=close` and `p=alive` without an id are no-ops (spec line 868; kitty for `alive`).
  - With an id they are forwarded to the GUI as `Osc99Control`.
- **Id-less content chunks accumulate in one unidentified slot** per handler (kitty's single
  pending command, keyed by "no id"). A `d=0` id-less chunk no longer drops.
- **Metadata merges across chunks.** Scalars (`c`, `o`, `u`, `w`, `f`, `g`, `s`) take the
  latest explicitly-set value; lists (`n`, `t`) concatenate in arrival order; `a=` edits
  accumulate (above); `e=` and `p=` are per chunk. Defaults are applied once, at finalisation.
- **The pending map never refuses.** At 128 entries the least-recently-touched entry
  (tombstones included) is evicted. Combined with the 512-byte id cap this bounds tombstones.
- At finalisation:
  - Title and body have C0 controls except LF, DEL and C1 controls removed, matching kitty's
    `[\x00-\x09\x0b-\x1f\x7f-\x9f]`. The same stripping applies to button labels, icon names,
    app name, types and sound name **(differs from kitty**, which leaves those alone; they are
    shown in freminal's toast and sent over D-Bus).
  - A title or body longer than 1024 characters is cut to 1023 characters plus `…` (kitty's
    `limit_size`). At most 8 buttons are kept, each limited to 256 characters the same way.
    **Superseded by 138.C7:** the cut is a hard one at 1024 / 256 characters with no `…`,
    which is what `limit_size` does.
  - An empty title takes the body, and the body becomes empty. A notification with neither is
    dropped with no reply (spec line 1132).

**Transport (130.C4).** Every transported OSC 99 flag is a named domain enum
(`freminal-state-representation`). Urgency, occasion and expiry use typed values, not
`Option<u8>` / `Option<String>` / `Option<i64>`. The title is guaranteed non-empty.

**GUI lifecycle.**

- The GUI owns an `Osc99Registry`: notification instances keyed by `(PaneId, id)`, plus
  id-less instances, each with a fresh `Osc99InstanceId`. The registry is a pure state machine
  that returns effects (reply, focus, show/update/close per leg). A thin executor in the GUI
  applies them. It is capped at 128 instances (oldest forgotten, with no reply), like kitty.
- **Scoping is per pane.** Update in place, `p=close` and `p=alive` only see the requesting
  pane's instances **(differs from kitty**, whose `p=close` looks across all windows). Ids are
  meant to be unique, and pane scoping stops one pane closing or seeing another's
  notifications.
- Pane identity is the `Weak` reply handle plus `(WindowId, PaneId)`. An instance whose pane
  is gone (`Weak` no longer upgrades) is pruned every frame. Its displayed notifications stay
  on screen and simply produce no replies. No close-path hooks are needed.
- **Legs.** An instance is shown on one or more legs (toast, Linux system, untracked system)
  chosen by the existing routing at display time.
  - Activation from any leg is reported (when `a=report`) and focuses (when `focus`), and
    closes the instance's other legs (the user has acted on it).
  - An activation without `c=1` forgets the instance (kitty). With `c=1` the instance stays
    until its last tracked leg closes, and then exactly one close report is sent.
  - `p=close` and expiry close every leg.
  - An update replaces every leg in place and suppresses the replaced notification's close
    report (kitty).
  - If any displayed leg is untracked and `c=1` was requested, `p=close;untracked` is sent at
    display and no later close report is sent.
- **Expiry.** A positive `w=` is enforced by the registry on every leg: at the deadline every
  leg is closed, whatever the OS or hover state. The GUI schedules a repaint for the nearest
  deadline. `w=0` means never expire (toast: until dismissed). `w=-1` uses the leg's default.
- **`o=` is pane-aware** (spec line 1124, kitty `ui_state`).
  - _Focused_ means the OS window is focused, the pane's tab is active and the pane is the
    tab's active pane.
  - _Visible_ means the OS window is focused and not minimised, the tab is active, and no
    other pane is zoomed.
  - `unfocused` shows only when not focused. `invisible` shows only when not focused and not
    visible.
  - The routing choice (`wants_toast` / `wants_system`) keeps using OS-window focus. That is
    freminal's own routing semantics, not the protocol's.
- **Focus action.** Activation with `focus` switches the pane's window to the pane's tab,
  unzooms that tab if another pane is zoomed, makes the pane active (with the usual
  `FocusChange` events), and calls `WindowHandle::focus_window`. Compositor focus-stealing
  rules (for example on Wayland) can refuse the OS-window raise. That is platform policy, so
  the D-Bus `ActivationToken` signal is not consumed.
- **Filtering** runs after the `o=` check, as in kitty. A filtered notification is dropped
  with no reply.
- **The `g=` icon cache stays global** (spec: it lives as long as the terminal runs) but
  becomes least-recently-used, as the spec asks, keeping today's 64-entry / 4 MiB bound.

**Toast leg.**

- `ToastStack` gains a generic keyed lifecycle (`ToastKey`, opaque to the toast module) and
  emits `ToastEvent`s for keyed toasts only. Freminal's own toasts are unchanged.
- Body click gives `Activated` then `Closed`. The `×` gives `Closed`. Button _n_ gives
  `Button(n)` then `Closed`. Expiry, eviction past `MAX_TOASTS` and `close_keyed` give
  `Closed`.
- Urgency: `Critical` uses `ToastKind::Error` and stays until dismissed unless `w>0`;
  `Normal` uses `ToastKind::Info` with its default lifetime; `Low` uses `ToastKind::Info` at
  half the Info lifetime.
- Toasts do not render icons and play no sounds (see the capability rules). **Superseded by
  138.C4 for icons:** the first standard `n=` name shows as a bundled-font glyph in the toast's
  icon slot. Toasts still render no transmitted images and play no sounds.

**Linux/BSD system leg (zbus).**

- The service is started on the first capability resolution where OSC 99 is supported and the
  routing can take the system leg. It lives for the rest of the process. The dispatcher thread
  exits when the GUI drops its command sender. The pump exits on its next send after that.
- The pump forwards signals **into the dispatcher's input channel**, not to the GUI. The
  dispatcher owns the D-Bus-id ↔ instance map. A signal that arrives during a `Notify` call
  is therefore processed after the call has recorded its id, which removes the ordering race
  between the two threads.
- One match rule (signals on interface `org.freedesktop.Notifications`, path
  `/org/freedesktop/Notifications`) on the pump. Members other than `ActionInvoked` and
  `NotificationClosed` are ignored.
- `Notify` arguments:
  - `app_name` is the `f=` value, else `freminal`.
  - `replaces_id` is the replaced instance's D-Bus id, else 0.
  - `expire_timeout` is −1 / 0 / ms.
  - `actions` is `["default", ""]` plus `["1", label1, …]`, sent only when the daemon has the
    `actions` capability.
  - Hints: `urgency` (byte), `category` (the first non-empty `t=`), `suppress-sound=true` for
    `silent`, `sound-name` for a named sound, and `image-data` (`(iiibiiay)`, RGBA) when
    transmitted icon data is used.
- The body has `<` and `&` followed by U+200C when the daemon reports `body-markup` (kitty).
- Icon resolution (kitty order):
  - Each `n=` name in order: a standard name maps to its freedesktop name; otherwise the name is
    used if it exists in the XDG icon scan.
  - If none matches, the transmitted icon data (decoded with the workspace `image` crate) is
    used, else the first `n=` name verbatim.
  - With no `n=` names: icon data, else the icon of the `.desktop` file matching `f=`, else
    freminal's icon.
  - Standard names: `error`→`dialog-error`, `warn`/`warning`→`dialog-warning`,
    `info`→`dialog-information`, `question`→`dialog-question`, `help`→`system-help`,
    `file-manager`→`system-file-manager`, `system-monitor`→`utilities-system-monitor`,
    `text-editor`→`utilities-text-editor`.
- Sounds:
  - `system` sends no sound hint. `silent` sends `suppress-sound`.
  - `error`/`warn`/`warning`/`info`/`question` send `sound-name` `dialog-error` /
    `dialog-warning` / `dialog-information` / `dialog-question`. Any other name is sent
    verbatim as `sound-name`.
  - **(differs from kitty**, which plays named sounds itself through libcanberra; freminal has
    no sound library and uses the freedesktop hint.)
- Until `GetCapabilities` answers, the daemon is assumed to have `body`, `actions` and
  `body-markup`, and not `sound` (kitty also assumes support before the reply). If the session
  bus cannot be reached, the system leg is unavailable: displays on it close immediately, it
  contributes no capabilities, `System` routing becomes `Osc99Support::Unsupported` (nothing
  could ever be displayed), and one warning is logged.
- Signals are mapped as follows:
  - `ActionInvoked("default")` is a body activation; `"1"`–`"8"` are button activations.
  - `NotificationClosed` with any reason is a close.
  - A close or action for a D-Bus id with no live instance is ignored (this covers replaced
    notifications).
- The `image-data` hint key is used unconditionally; notification spec 1.2 daemons are
  universal. A decoded icon with a side over 256 is downscaled to fit 256×256 before sending
  (superseded at 138.14 from "over 1024", for transport cost). Decoding is limited to
  4096×4096 and 64 MiB.

**macOS / Windows system leg.**

- Fire-and-forget notify-rust, with no waiting thread. Buttons are not registered, because
  their activation cannot be observed. (**Superseded by 138.C15:** one delivery thread with a
  queue of 16 does the work, and Windows builds its toast directly with
  `tauri-winrt-notification`.)
- Sound: `system` uses the platform default (`NSUserNotificationDefaultSoundName` on macOS,
  `"Default"` on Windows). `silent` sets no sound name, which both backends treat as silent.
  Other names fall back to the platform default.
- Icon temp files are written once per content hash into
  `std::env::temp_dir()/freminal-osc99-icons/`, capped at 64 files (oldest deleted), and the
  directory is removed at shutdown. This replaces deleting the file right after `show()`.
  (**Superseded by 138.C15:** the directory is per process, `freminal-osc99-icons-<pid>`, mode
  `0700`, and files are also deleted by age.)

**Leg capabilities and `p=?` truthfulness.** Each leg has a capability set:

| Capability           | Toast | Linux/BSD system     | macOS system | Windows system |
| -------------------- | ----- | -------------------- | ------------ | -------------- |
| Activation report    | yes   | daemon has `actions` | no           | no             |
| Focus on activation  | yes   | daemon has `actions` | no           | no             |
| Close events (track) | yes   | yes                  | no           | no             |
| Auto-expiry          | yes   | yes                  | no           | no             |
| Urgency              | yes   | yes                  | no           | yes            |
| Standard sound names | no    | daemon has `sound`   | no           | no             |
| Body                 | yes   | daemon has `body`    | yes          | yes            |
| Buttons              | yes   | daemon has `actions` | no           | no             |
| Icons                | no    | yes                  | yes          | yes            |

The routing combines its legs:

- `Toast` and `System` use their one leg.
- `Both` shows both legs together. A feature that one leg suffices for (report, focus, urgency,
  sound names, body, buttons, icons) is advertised if **either** leg has it. A per-notification
  guarantee (close events, auto-expiry) is advertised only if **both** legs have it.
- `SystemWhenUnfocused` shows either leg depending on focus, so every feature needs **both**
  legs.

The reply follows kitty's key order and values:

```text
a=focus,report : o=always,unfocused,invisible : u=0,1,2 : p=title,body,?,close,icon,alive,buttons : c=1 : w=1 : s=system,silent,error,info,question,warn,warning
```

Absent features drop their key or list item: `a` lists only supported actions and is omitted
if none; `body` / `icon` / `buttons` drop out of `p`; `u`, `c` and `w` are omitted;
`s` is `system,silent` without standard sound names.

### 138 Types (decided)

In `freminal-common/src/buffer_states/osc99_notification.rs` (new; 138.1). The transport
types move here from `window_manipulation.rs`:

```rust
pub enum Osc99ActivationReporting { Report, Silent }
pub enum Osc99ActivationFocus { Focus, NoFocus }
pub enum Osc99CloseReporting { Report, Silent }
pub enum Osc99Expiry { OsDefault, Never, After(NonZeroU32) }   // milliseconds
pub enum Osc99Sound { System, Silent, Named(String) }
pub enum Osc99ControlKind { Close, Alive }                      // moved, unchanged
pub struct Notification99Data {
    pub id: Option<String>,
    pub title: String,                     // non-empty after 138.3
    pub body: Option<String>,
    pub icon_data: Option<Vec<u8>>,
    pub icon_names: Vec<String>,
    pub icon_cache_key: Option<String>,
    pub buttons: Vec<String>,
    pub activation_reporting: Osc99ActivationReporting,
    pub activation_focus: Osc99ActivationFocus,
    pub close_reporting: Osc99CloseReporting,
    pub urgency: NotificationUrgency,      // Normal when unset
    pub occasion: NotificationOccasion,
    pub sound: Osc99Sound,
    pub app_name: Option<String>,
    pub notification_types: Vec<String>,
    pub expiry: Osc99Expiry,
}
```

`WindowManipulation::Osc99Control { id: String, kind }`. After 138.3 the id is mandatory,
because id-less controls never leave the PTY thread.

In `freminal-common/src/host_capabilities.rs` (138.1), `Osc99Features` gains the following
fields, each a two-variant enum (`Supported` / `Unsupported`, except where named):

```rust
pub struct Osc99Features {
    pub activation_report: Osc99ActivationReport,   // existing
    pub activation_focus: Osc99FocusAction,          // Supported | Unsupported
    pub close_events: Osc99CloseEvents,              // existing
    pub auto_expiry: Osc99AutoExpiry,
    pub urgency: Osc99UrgencySupport,
    pub sound_names: Osc99SoundNames,                // SystemAndSilent | Standard
    pub body: Osc99BodySupport,
    pub buttons: Osc99ButtonSupport,
    pub icons: Osc99IconSupport,
}
```

In `freminal-common/src/buffer_states/osc_notify_99.rs` (138.2), `Osc99Command` becomes
unset-aware, so reassembly can merge:

```rust
pub enum Osc99ChunkState { More, Done }
pub enum Osc99ActionEdit { Add(Osc99Action), Remove(Osc99Action) }
pub enum Osc99Action { Report, Focus }
pub enum Osc99PayloadType { Title, Body, Close, Icon, Alive, Buttons, Query, Unknown }
pub struct Osc99Command {
    pub id: Option<String>,
    pub payload_type: Osc99PayloadType,
    pub chunk_state: Osc99ChunkState,
    pub payload: Vec<u8>,
    pub payload_encoding: Osc99PayloadEncoding,
    pub action_edits: Vec<Osc99ActionEdit>,
    pub close_reporting: Option<Osc99CloseReporting>,
    pub app_name: Option<String>,
    pub icon_cache_key: Option<String>,
    pub icon_names: Vec<String>,
    pub occasion: Option<NotificationOccasion>,
    pub sound: Option<String>,             // raw name; mapped to Osc99Sound at finalisation
    pub notification_types: Vec<String>,
    pub urgency: Option<NotificationUrgency>,
    pub expiry: Option<Osc99Expiry>,
}
```

In the GUI (`freminal/src/gui/osc99/`, new directory; 138.6 onward):

```rust
pub(crate) struct Osc99InstanceId(u64);
pub(crate) struct Osc99Origin { pub window_id: WindowId, pub pane_id: PaneId }
pub(crate) enum PaneKeyboardFocus { Focused, Unfocused }
pub(crate) enum PaneVisibility { Visible, Hidden }
pub(crate) struct Osc99PaneState { pub focus: PaneKeyboardFocus, pub visibility: PaneVisibility }
pub(crate) enum Osc99Leg { Toast, LinuxSystem, UntrackedSystem }
pub(crate) enum Osc99Effect {
    Reply { reply_tx: Weak<Sender<InputEvent>>, reply: GuiReply },
    FocusPane(Osc99Origin),
    ShowToast { instance: Osc99InstanceId, replaces: Option<Osc99InstanceId>, spec: Osc99ToastSpec },
    CloseToast(Osc99InstanceId),
    ShowLinuxSystem { instance: Osc99InstanceId, replaces: Option<Osc99InstanceId>, spec: Box<Notification99Data> },
    CloseLinuxSystem(Osc99InstanceId),
    ShowUntrackedSystem(Box<Notification99Data>),
}
```

In `freminal/src/gui/toast/mod.rs` (138.7; `gui/toast.rs` until 138.C1 split it):

```rust
pub(super) struct ToastKey(pub u64);
pub(super) enum ToastLifetime { KindDefault, Fixed(Duration), UntilDismissed }
pub(super) enum ToastEventKind { Activated, Button(u8), Closed }   // Button is 1-based
pub(super) struct ToastEvent { pub key: ToastKey, pub kind: ToastEventKind }
```

### 138 Execution model

Serial, on one checkout (`task-138/osc99-conformance`). The subtasks share
`notify_99.rs`, `notifications.rs`, the new `gui/osc99/` modules and `toast.rs` (later
`toast/`), so worktrees
would buy nothing.

Each subtask is done in four steps:

1. A sub-agent implements it.
2. The orchestrator reviews it in full against the subtask text, reading the diff line by line.
3. The orchestrator re-runs the verification suite itself.
4. Only then does the orchestrator commit it.

A sub-agent that ends without a report is treated as unverified. Its tree state is inspected,
and the work is then finished by a fresh agent, re-decomposed, or discarded.

Order: 138.1 → 138.18 in number order (138.7 runs as 138.7a then 138.7b; 138.17, the pure
capability tables, runs before 138.15, which wires them). The adversarial
review of the whole branch follows
138.18, and its findings become `138.Cn` entries.

No benchmark subtask: OSC 99 is not on a benchmarked path (`freminal-bench-table` lists none,
and no bench feeds OSC 99). The parser change keeps `MAX_OSC99_SEQUENCE_BYTES` and the
tokenizer, so the general OSC path is untouched.

### 138 Subtasks

Every subtask's verification is `cargo xtask test` and
`cargo clippy --all-targets --all-features -- -D warnings`, plus anything named below.

Every subtask's prohibitions include: do NOT touch files outside scope, do NOT commit, do NOT
update plan documents, and do NOT proceed to the next subtask.

Coding rules for every subtask: no `unwrap`/`expect` outside tests, no raw numeric `as`
casts (`conv2`), no new `bool` fields or parameters, and no `#[allow(dead_code)]`.

The stop condition is the same for all: report files changed and verification results, then
await review.

#### 138.1 — Transport and capability types (no behaviour change)

Scope:

- `freminal-common/src/buffer_states/osc99_notification.rs` (new),
  `freminal-common/src/buffer_states/mod.rs`,
  `freminal-common/src/buffer_states/window_manipulation.rs`;
- `freminal-common/src/host_capabilities.rs`;
- `freminal-terminal-emulator/src/terminal_handler/notify_99.rs`
  (`into_notification99_data`, `osc99_query_reply_body`), and the OSC 99 test fixtures in
  `terminal_handler/osc.rs`;
- `freminal/src/gui/host_capabilities.rs`, `freminal/src/gui/notifications.rs`,
  `freminal/src/gui/rendering.rs` (test fixture only), `freminal/src/gui/app_impl.rs` (only
  where `Notification99Data` / `Osc99ControlKind` paths change).

What:

- Create `osc99_notification.rs` with the enums and `Notification99Data` exactly as in
  "138 Types". Move `Osc99ControlKind` there. `window_manipulation.rs` imports both;
  `Osc99Control.id` stays `Option<String>` in this subtask.
- Behaviour is preserved:
  - **Title:** `title` becomes `String`; a missing title maps to `String::new()` in
    `into_notification99_data`. The GUI reproduces today's "title else body" choice from that.
  - **Mapping from today's fields:**
    - `urgency` is `meta.urgency.unwrap_or(Normal)`.
    - `occasion` is passed through.
    - `expiry` maps −1 → `OsDefault`, 0 → `Never`, n>0 → `After(n)` (clamped to `u32::MAX`).
    - `sound` maps `None` → `System`, `Some("silent")` → `Silent`, `Some("system")` →
      `System`, other → `Named`.
  - **GUI reads:** the GUI is adapted to read the new fields with today's semantics.
    `Silent` is still sent to notify-rust as the sound name `"silent"`, `System` sends no
    sound name, and the toast leg uses `urgency == Critical` for `error`.
    `Osc99LiveEntry` stores the two enums instead of `bool`s, and `osc99_action_report` takes
    `Osc99ActivationReporting` and `Osc99CloseReporting` instead of `bool`s.
- Add the seven new capability enums and fields to `Osc99Features`. `host_capabilities()`
  fills them with values that reproduce today's reply:
  - `activation_focus: Unsupported`, `auto_expiry: Supported`, `urgency: Supported`,
    `sound_names: SystemAndSilent`;
  - `body`, `buttons` and `icons`: `Supported`.
- `osc99_query_reply_body` reads the new fields but keeps its current output and key order
  byte-for-byte (138.4 changes the format).

Deliverable:

- The types, with doc comments.
- Every existing test passing with only mechanical fixture updates.
- New unit tests for each mapping above: expiry −1 / 0 / 5000 / > `u32::MAX`, the four sound
  cases, urgency unset.

Prohibitions: do NOT change any observable behaviour; do NOT touch the parser
(`osc_notify_99.rs`) or reassembly logic.

#### 138.2 — Parser fidelity

Scope:

- `freminal-common/src/buffer_states/osc_notify_99.rs` (parser and its tests);
- `freminal-terminal-emulator/src/terminal_handler/notify_99.rs` and
  `terminal_handler/osc.rs`, only to adapt to the new `Osc99Command` shape with **today's**
  reassembly semantics. The latest chunk still wins, now computed from the unset-aware
  fields with the same defaults.

What:

- `Osc99Command` and its new enums exactly as in "138 Types". `Osc99Actions` is deleted.
- Lenient values, id sanitisation (strip to `[A-Za-z0-9_+.-]`, truncate to 512 bytes, empty
  means absent), malformed tokens ignored, `Unknown` payload type, and lossy plain payload,
  all as in "138 Decisions (orchestrator) → Parsing".
- `Osc99ParseError` loses the variants that can no longer occur. Any variant left without a
  producer is deleted. Only `SequenceTooLarge` and `TooManyMetadataItems` remain errors.
- Update the parser tests that pinned the old behaviour (listed in the recon). Each becomes a
  test of the new behaviour, keeping its name only if the name is still accurate.

Deliverable: the parser plus tests for every rule:

- `c=2` reports; `d=2` is done; `e=2` is plain;
- `u=3` is ignored and an earlier `u=` survives in the same sequence;
- `w=-5` is −1; `w=abc` is ignored;
- a bad `f=` is ignored while the rest of the sequence applies;
- `i=a/b` becomes `ab`; a 600-byte id is truncated to 512; `i=///` is absent;
- empty `a=`, and `a=report` giving edits `[Add(Report)]`;
- `p=frobnicate` is `Unknown`; a bare token is ignored; a two-byte key is ignored;
- a non-UTF-8 plain payload is lossy.

Prohibitions: do NOT change reassembly semantics (138.3); do NOT change the GUI.

#### 138.3 — Reassembly rework

Scope:

- `freminal-terminal-emulator/src/terminal_handler/notify_99.rs`,
  `terminal_handler/osc.rs` (the `Notify99` arm and its tests),
  `terminal_handler/mod.rs` (the pending-state fields), `terminal_handler/reset.rs`;
- `freminal-terminal-emulator/tests/osc99_query.rs`;
- `freminal-common/src/buffer_states/window_manipulation.rs` (`Osc99Control.id: String`)
  and the GUI match arms that destructure it (`freminal/src/gui/app_impl.rs`,
  `rendering.rs`).

What, as in "138 Decisions (orchestrator) → Reassembly":

- Control payloads act immediately and never touch pending state. Id-less `p=close` /
  `p=alive` are no-ops.
- One unidentified slot. Metadata merge. LRU eviction at 128 instead of refusal.
- Finalisation does the stripping, truncation, title promotion and the empty drop.
- `into_notification99_data` builds the transport from the merged metadata.
- RIS clears the unidentified slot as well as the map. DECSTR keeps both, as today.

Deliverable:

- **Merging:** a chunk with `a=report` followed by a final chunk with no `a=` yields report
  plus focus; earlier `u=2` survives a final chunk without `u=`; `n=` from two chunks keeps
  both in order.
- **Id-less chunks:** id-less `d=0` then id-less `d=1` assembles one notification.
- **Controls:** `p=?` with `d=0` is answered at once; `p=close` with `d=0` and an id is
  forwarded at once, and an id's pending content survives a control chunk for the same id.
- **Saturation:** 128 pending ids plus a new complete single-chunk notification displays
  without evicting anything (it never enters the store); a new `d=0` id evicts the
  least-recently-touched entry; a `p=?` on a saturated map is answered.
- **Finalisation:** controls are stripped from the title (ESC, C1 U+0085 and TAB removed, LF
  kept); a 2000-character body becomes 1024 characters ending in `…` (superseded by 138.C7:
  no `…`); nine buttons become
  eight; a body-only notification becomes title-only; a notification with no title and no
  body pushes nothing.
- **Existing tests:** the existing pinning tests are updated.

Prohibitions: do NOT change the `p=?` reply format (138.4); do NOT change GUI behaviour beyond
the `Osc99Control.id` type.

#### 138.4 — Truthful `p=?` reply body

Scope: `freminal-terminal-emulator/src/terminal_handler/notify_99.rs`
(`osc99_query_reply_body` and its tests), `freminal-terminal-emulator/tests/osc99_query.rs`.

What: build the reply from every `Osc99Features` field, in kitty's key order, exactly as in
"Leg capabilities and `p=?` truthfulness". The `p` list order is
`title,body,?,close,icon,alive,buttons`, with absent items removed.

Deliverable:

- A table test over the features. It must cover: all supported; all unsupported (giving
  `o=always,unfocused,invisible:p=title,?,close,alive:s=system,silent`); focus-only; and
  standard sounds.
- The integration test's expected tail updated to the reply produced by the 138.1 defaults.

Prohibitions: do NOT change `host_capabilities()` (138.17).

#### 138.5 — Origin and pane-state plumbing; pane-aware `o=`

Scope: `freminal/src/gui/frame_drain.rs`, `freminal/src/gui/rendering.rs`,
`freminal/src/gui/app_impl.rs` (`route_window_manipulation_events` and its caller),
`freminal/src/gui/notifications.rs` (`Osc99DisplayContext`, `occasion_allows_display` and
their tests), and the new `freminal/src/gui/osc99/mod.rs` (just `Osc99Origin`,
`PaneKeyboardFocus`, `PaneVisibility`, `Osc99PaneState`, registered in `gui/mod.rs`).

What:

- The OSC 99 collector tuples carry `Osc99Origin` and, for display requests,
  `Osc99PaneState`, computed in `drain_window_manipulation_commands`, where `is_active_tab`,
  the active pane, the zoom state, OS-window focus and minimisation are known.
  `route_window_manipulation_events` takes the `WindowId`.
- `occasion_allows_display(NotificationOccasion, Osc99PaneState) -> Osc99DisplayDecision`,
  where `Osc99DisplayDecision` is `Show` or `Suppress`. It follows the pane-aware rule in
  "GUI lifecycle".
- `Osc99DisplayContext` keeps `window_focused` for routing only.
- Remove the silent drop when a `RefCell` borrow fails. If a borrow cannot be taken, log one
  `warn!` naming the cell; nothing is dropped silently.

Deliverable:

- Unit tests for the decision table. They must cover: `unfocused` in a background tab of a
  focused window shows; `unfocused` on the active pane of a focused window is suppressed;
  `invisible` for a non-active pane in the active tab is suppressed (visible); `invisible`
  for a pane hidden by another pane's zoom shows.
- A `frame_drain` test that the pane state is computed per pane.
- The `route_osc99_occasion_*` tests updated.

#### 138.6 — `Osc99Registry`: instances, pane scoping, alive and close

Scope: `freminal/src/gui/osc99/registry.rs` (new), `freminal/src/gui/osc99/mod.rs`,
`freminal/src/gui/notifications.rs`, `freminal/src/gui/mod.rs` (the `osc99_live` field becomes
`osc99_registry`), `freminal/src/gui/app_impl.rs` (control handling and the effect executor).

What:

- `Osc99Registry` replaces `Osc99LiveEntry`, `osc99_live`, `forget_osc99`, `live_ids_sorted`
  and `send_osc99_alive`.
- Its API is pure: methods take inputs and return `Vec<Osc99Effect>`.
- Instances are keyed by `(PaneId, id)` plus id-less instances, capped at 128 (the oldest is
  forgotten).
- Registration happens on display, after the occasion check, recording the legs that fired.
- `p=alive` replies with the requesting pane's ids only, sorted.
- `p=close` forgets the pane's instance for that id (leg closing arrives in 138.7 and 138.13).
- `prune_dead_panes()` runs every frame and drops instances whose `Weak` no longer upgrades.
- `Osc99Effect::Reply` is executed through `send_reply_weak`. Other variants are added when
  their subtask needs them (no unused variants).

Deliverable:

- Registry unit tests: pane A's ids are not in pane B's alive reply; pane B's `p=close` cannot
  forget pane A's id; the cap evicts the oldest; a dead pane is pruned; an id-less instance is
  never listed.
- Existing alive/close tests ported.

#### 138.7 — Toast keyed lifecycle; toast leg through the registry

Split at execution time (2026-10-12) into 138.7a and 138.7b, so each sub-agent pass stays
bounded.

##### 138.7a — `ToastStack` keyed lifecycle and events

Scope: `freminal/src/gui/toast.rs` only (now the `gui/toast/` directory, split by 138.C1).

What:

- **Types** (as in "138 Types"): `ToastKey(pub u64)`;
  `ToastLifetime { KindDefault, Fixed(Duration), UntilDismissed }`;
  `ToastEventKind { Activated, Button(u8), Closed }`
  (`Button` unused until 138.8, so not added yet); `ToastEvent { key, kind }`.
- **API:**
  - `push_keyed(key, kind, title, detail, lifetime, placement)`;
  - `replace_keyed(old, new_key, kind, title, detail, lifetime)`. This updates the toast in
    place: it keeps its stack slot, placement and entry-animation state, and **its lifetime
    restarts**. Keeping the age would make a toast updated every second expire mid-update.
    Returns whether `old` existed; if not, it behaves as `push_keyed`;
  - `close_keyed(key)`;
  - `take_events() -> Vec<ToastEvent>`.
- **Events** are produced for keyed toasts only:
  - a body click gives `Activated` then `Closed`;
  - a click on the dismiss glyph gives `Closed`;
  - expiry, eviction past `MAX_TOASTS` and `close_keyed` give `Closed`;
  - `replace_keyed` produces no event for the replaced toast.
- **Dismiss rect:** `ToastLayoutOutput` gains a dismiss hit rect (logical points) around the
  "x" glyph. It is tested before the body rect, for keyed toasts only.
- `Toast::is_expired` honours `ToastLifetime` (`UntilDismissed` never expires).
- **Unkeyed toasts** (every existing caller) behave and lay out exactly as today: any click
  dismisses and no events are produced.

Deliverable: unit tests for every event path and lifetime, eviction of a keyed toast, replace
keeping the slot, and unkeyed behaviour unchanged.

##### 138.7b — Toast leg through the registry

Scope: `freminal/src/gui/osc99/registry.rs`, `freminal/src/gui/osc99/mod.rs`,
`freminal/src/gui/notifications.rs`, `freminal/src/gui/app_impl.rs` (drain toast events after
`ToastStack::show`; executor sink).

What:

- **Effects and executor.** `Osc99Effect` gains `ShowToast { instance, replaces, spec }` and
  `CloseToast(instance)`. `Osc99ToastSpec { kind, title, detail, lifetime }` is built by a pure
  function from `Notification99Data`, using the urgency and `w=` mapping in "Toast leg". The
  executor takes an `Osc99EffectSink { toasts: &mut ToastStack }` (later subtasks add fields).
  `ToastKey` is the instance's raw id.
- **Routing.** `route_osc99` no longer pushes toasts directly: `register` returns the
  effects.
- **Instance state.** Each instance tracks its open legs.
  - An update of a live `(pane, id)` replaces the toast in place (or closes the old toast when
    the new routing has no toast leg) and forgets the old instance silently.
  - `p=close` emits `CloseToast`. The instance stays until the toast's `Closed` event arrives,
    and then a close report is sent when `c=1`.
  - `Activated` sends `GuiReply::Osc99Activation { button: None }` when `a=report`. Without
    `c=1` the instance is forgotten. With `c=1` it waits for the last tracked leg to close,
    then sends exactly one `Osc99Closed { Tracked }`.
  - An event for an unknown instance is ignored. This covers replaced and forgotten
    instances.
- **Interim rules until 138.12, 138.13 and 138.16.**
  - The `LinuxSystem` leg is still driven by the old per-notification thread, which sends its
    own activation and close reports. The registry treats it as externally managed: it never
    reports for it, `p=close` drops it silently, and while it is open no registry close report
    is sent (otherwise `Both` routing would report twice).
  - An instance with an `UntrackedSystem` leg never gets a registry close report, because the
    old code already sent `untracked` at display. This is the final 138.16 rule, applied now.

Deliverable: the registry tests listed below.

- update-in-place suppresses the old close;
- activation without `c=1` sends no close;
- activation with `c=1` sends the activation and then exactly one close when the toast
  closes;
- `p=close` sends a close report only when `c=1`;
- the interim `LinuxSystem` and `UntrackedSystem` rules.

The "activation on one leg closes the other legs" test moves to 138.13, the first point at
which two closable legs exist.

#### 138.8 — Toast buttons

Scope: `freminal/src/gui/toast.rs` (layout, rendering, hit-test; now the `gui/toast/`
directory, split by 138.C1), `freminal/src/gui/osc99/registry.rs`.

What:

- **Layout.** Keyed toasts carry up to 8 button labels, laid out in a row under the detail.
  Labels are fitted with the existing `fit_text`, and the row wraps onto further rows when it
  does not fit `MAX_PILL_WIDTH_PTS`. Each button is a rounded rect in the toast's colours.
- **Hit-testing.** Each button has its own hit rect, tested before the body rect.
  `ToastEventKind::Button(n)` is 1-based, followed by `Closed`.
- **Reports.** The registry reports `GuiReply::Osc99Activation { button: Some("n") }`.

Deliverable:

- Layout unit tests: rect count and order, wrapping at width, hit-test priority over the body.
- A registry test that button 2 is reported as `"2"`.
- A layout test that unkeyed toasts' geometry is unchanged.

#### 138.9 — Focus on activation

Scope: `freminal/src/gui/osc99/registry.rs` (emit `FocusPane`), `freminal/src/gui/app_impl.rs`
(executor), plus the pane/tab helpers it calls (`freminal/src/gui/actions.rs`,
`freminal/src/gui/tabs.rs`, `freminal/src/gui/panes/mod.rs`) only if a missing helper must be
added.

What:

- An activation of an instance with `Osc99ActivationFocus::Focus` emits
  `Osc99Effect::FocusPane(origin)`.
- The executor finds the origin window, switches to the pane's tab (`TabManager::switch_to`
  through the existing UI wrapper that also clears `bell_active`), unzooms if another pane is
  zoomed, and makes the pane active with the `FocusChange(false/true)` pair, reusing the
  existing click-to-focus helper.
- It then calls `WindowHandle::focus_window(window_id)` and requests a repaint of that window.
- A missing window or pane is a no-op.
- `FocusPane` applies to every leg that reports activation. Today that is the toast leg; the
  Linux leg arrives in 138.12.

Deliverable:

- Registry tests: focus is emitted only with `Focus`, and both focus and report are emitted
  for `a=focus,report`.
- An executor test on a two-tab window: the pane's tab becomes active and the pane becomes
  active.

#### 138.10 — Terminal-side expiry

Scope: `freminal/src/gui/osc99/registry.rs`, `freminal/src/gui/app_impl.rs` (tick and repaint
scheduling).

What:

- Each instance with `Osc99Expiry::After(ms)` gets a deadline measured from display (and from
  each update).
- `registry.tick(now) -> Vec<Osc99Effect>` closes every leg of every instance past its
  deadline, through the same path as `p=close`.
- `registry.next_deadline() -> Option<Instant>` feeds the frame's repaint scheduling, through
  the same mechanism the toast stack's repaint delay uses.
- `now` is injected, so the tests use no real time.

Deliverable: registry tests with injected `Instant`s — closed at the deadline even though the
toast leg's own lifetime is longer; an update resets the deadline; `w=0` never expires;
`next_deadline` is the minimum across instances.

#### 138.11 — `osc99_filters` config option and filtering

Scope:

- The full `freminal-config-options` checklist: `freminal-common/src/config.rs`
  (`NotificationsConfig` field, its defaults and `ConfigPartial` coverage),
  `freminal/src/gui/settings.rs` (settings UI), `config_example.toml`,
  `nix/home-manager-module.nix`;
- `freminal/src/gui/osc99/filter.rs` (new), `freminal/src/gui/osc99/registry.rs`.

What:

- **Config.** `osc99_filters: Vec<String>`, defaulting to empty.
- **Filter type.** `Osc99Filter::compile(&[String]) -> (Osc99Filter, Vec<Osc99FilterError>)`.
  - Each rule is `all` or `location:regex`, with location `title`, `body`, `app` or `type`.
    Matching is a regex search, case-sensitive (kitty).
  - `type` matches if any `t=` value matches.
  - Invalid rules are returned as errors and skipped. Each is logged once at config apply.
- **Registry.** The registry drops a matching notification after the occasion check, sending
  no reply. The compiled filter is rebuilt on config change.
- **Settings UI.** A multi-line text field with one rule per line, showing the invalid lines
  below it.
- **Nix.** The Nix option is `listOf str`.

Deliverable:

- Filter unit tests: each location; `all`; an invalid regex skipped while the others still
  apply; `type` with multiple values.
- A registry test that a filtered notification is not displayed and gets no reply.
- Config round-trip and partial-apply tests per the checklist.

#### 138.12 — zbus notification service; Linux system leg

Scope: `Cargo.toml` (workspace dependency), `freminal/Cargo.toml` (target-specific dependency,
dev-dependency features), `freminal/src/gui/osc99/dbus_service.rs` (new),
`freminal/src/gui/osc99/mod.rs`, `freminal/src/gui/osc99/registry.rs`,
`freminal/src/gui/notifications.rs`, `freminal/src/gui/app_impl.rs` and
`freminal/src/gui/mod.rs` (service handle, event drain, repaint wake).

What:

- **Dependency.** `zbus = { version = "5.19.0", default-features = false, features = [...] }`,
  added under `[target.'cfg(all(unix, not(target_os = "macos")))'.dependencies]`. The
  features are those needed for the blocking API on the async-io executor that notify-rust
  already uses. Verify the feature names against the registry source. If the blocking API
  needs a different executor than notify-rust's, stop and report.
- **Service.** `Osc99DbusService::spawn(waker: Box<dyn Fn() + Send>) -> Osc99DbusHandle`
  (session bus) and `spawn_with_connection(conn, waker)` (used by tests).
  - The dispatcher thread is `freminal-osc99-dbus`; the pump thread is
    `freminal-osc99-signals`.
  - Commands: `Show { instance, replaces, notification }` and `Close { instance }`.
  - Events: `Shown { instance }`, `ShowFailed { instance }`, `Activated { instance, action }`
    (action is `Default` or `Button(u8)`), `Closed { instance }`, `Capabilities(..)`,
    `Unavailable`.
  - The dispatcher's state transitions live in a pure `DispatcherState`, which owns the
    D-Bus-id ↔ instance map, so they are unit-testable without a bus.
- **This subtask's protocol coverage.** `Notify` sends title, body, app name, the `default`
  action plus 1-based button actions, urgency and `expire_timeout`. Presentation hints come in
  138.14. `GetCapabilities` is called once at start.
- **Registry.** The registry's `LinuxSystem` leg sends `ShowLinuxSystem`. Its events feed
  activation (report, focus), button (1-based) and close, exactly like the toast leg's events.
  This retires `show_system_osc99`'s Linux path, its per-notification threads,
  `osc99_action_report` and the `#[allow(clippy::too_many_arguments)]` builder
  (`build_osc99_notification` keeps only what the macOS/Windows path needs).

Deliverable:

- `DispatcherState` unit tests: a signal for an unknown id is ignored; a close after an
  activation maps both.
- An integration test over a peer-to-peer zbus connection (a socket pair, so no daemon is
  needed) against an in-test fake `org.freedesktop.Notifications` server. It must check that
  `Notify` arguments include `default`, `1` and `2`; that an emitted `ActionInvoked("2")` and
  then `NotificationClosed` arrive as `Activated(Button(2))` and then `Closed`; and that
  `GetCapabilities` is reported.
- Run `cargo machete` (the new dependency is used) and `cargo xtask check-windows` (the cfg
  gates compile out on Windows).

#### 138.13 — Linux update in place, `p=close` and expiry through the service

Scope: `freminal/src/gui/osc99/dbus_service.rs`, `freminal/src/gui/osc99/registry.rs`.

What:

- **Update in place.** `Show` with `replaces` sends the replaced instance's D-Bus id as
  `replaces_id` and drops the replaced mapping, so a later `NotificationClosed` for it is
  ignored.
- **Close.** `Close` calls `CloseNotification`, and the resulting signal produces `Closed`.
  `Close` for an instance with no D-Bus id yet (not shown, or show failed) emits `Closed`
  immediately (kitty's fallback).
- **Registry.** The registry routes update, `p=close` and expiry to the Linux leg.

Deliverable:

- `DispatcherState` tests for each path.
- The fake-server integration test extended: an update sends `replaces_id`, and the old id's
  close signal produces no event; `Close` calls `CloseNotification` once.
- A registry test: an activation on one leg (toast or Linux) emits a close for the
  instance's other leg (moved here from 138.7).

#### 138.14 — Linux presentation: icons, sounds, category, markup

Scope: `freminal/src/gui/osc99/presentation.rs` (new), `freminal/src/gui/osc99/icon_lookup.rs`
(new, Linux/BSD), `freminal/src/gui/osc99/dbus_service.rs`, `freminal/src/gui/notifications.rs`
(the `g=` icon cache becomes LRU), `freminal/src/gui/mod.rs` if the cache type changes.

What, as in "Linux/BSD system leg":

- Standard icon-name table and resolution order. The XDG icon scan and the `.desktop` `Icon=`
  lookup for `f=` run lazily once on the dispatcher thread.
  - Icon dirs: `$XDG_DATA_HOME/icons`, each `$XDG_DATA_DIRS/icons`, `~/.icons`,
    `/usr/share/pixmaps`.
  - Applications dirs: `$XDG_DATA_HOME/applications`, each `$XDG_DATA_DIRS/applications`.
  - A desktop file matches when its stem or its `Name=` equals `f=` case-insensitively.
- Icon data is decoded to RGBA for the `image-data` hint, and downscaled to 256×256 when over
  1024×1024 (**superseded at 138.14 execution:** it is downscaled whenever a side exceeds 256;
  see the 138.14 status note). A decode failure falls back to the next source.
- Sound hints, the `category` hint, and body-markup escaping when the daemon reports
  `body-markup`.
- The `g=` cache is least-recently-used, keeping the 64-entry / 4 MiB bound.
- The resolution functions are pure; the filesystem roots are injected so tests use a temp
  dir.

Deliverable:

- Unit tests:
  - icons: each standard name; an existing name beats data; data beats an unknown name; an
    unknown name with no data is passed verbatim; `f=` resolves through a temp `.desktop`;
  - sounds: `silent` gives `suppress-sound`; `system` gives no hint; `warn` gives
    `dialog-warning`;
  - markup: `<` and `&` are escaped only with `body-markup`;
  - the LRU order.
- The fake-server test asserts the hints.

#### 138.15 — Daemon capabilities feed `HostCapabilities`; bus failure

Scope: `freminal/src/gui/host_capabilities.rs`, `freminal/src/gui/osc99/mod.rs` (leg capability
type), `freminal/src/gui/osc99/dbus_service.rs`, `freminal/src/gui/app_impl.rs`,
`freminal/src/gui/settings_dispatch.rs` (where the capability broadcast lives).

What:

- **Capability type.** `Osc99LegCapabilities` is a typed struct holding the table's rows
  (**superseded by 138.17:** the rows are `Osc99Features` values, and the daemon types live
  in `osc99/capabilities.rs`).
- **Daemon reply.** The `Capabilities(..)` event updates the Linux leg's capabilities, from
  the defaults until it arrives.
- **Recompute and broadcast.** `host_capabilities` now takes the config plus the Linux
  daemon's capability state. If the result changed, it is re-broadcast to every pane as
  `InputEvent::HostCapabilitiesChange`, the same path config changes use.
- **Bus failure.** `Unavailable` marks the Linux leg unavailable. Its displays close
  immediately, it contributes no capabilities, `System` routing becomes `Unsupported`, and one
  `warn!` is logged.
- **Service start.** The service is started at the first capability resolution where OSC 99 is
  supported and the routing can take the system leg.

Deliverable:

- Tests: a capability reply without `actions` removes `a=` and `buttons` from a System-routed
  pane's next `p=?` (end to end through the pane's capabilities); `Unavailable` makes a
  System-routed pane answer no `p=?` at all, and a Both-routed pane advertise the toast leg's
  capabilities only.
- The routing truth-table test is extended.

#### 138.16 — macOS / Windows untracked system leg

Scope: `freminal/src/gui/notifications.rs` (the macOS/Windows OSC 99 path),
`freminal/src/gui/osc99/registry.rs` (the `UntrackedSystem` leg), and a new
`freminal/src/gui/osc99/icon_tempfiles.rs` (macOS/Windows only).

What, as in "macOS / Windows system leg" and the untracked-leg rules:

- Fire-and-forget display with no buttons.
- `p=close;untracked` at display when `c=1`.
- An untracked leg never holds an instance alive.
- Platform sound mapping.
- Content-hash icon temp files.
- On these platforms the registry never emits `ShowLinuxSystem`.

Deliverable:

- Registry tests, which are platform-independent because the leg is a value: an untracked-only
  display with `c=1` sends the untracked close immediately and nothing later; `Both` with a
  toast plus an untracked leg sends the untracked close and no tracked close.
- Unit tests for the sound mapping function and the temp-file naming/cap. Both are pure and
  compiled on every platform, used only behind the cfg.
- `cargo xtask check-windows` passes. macOS is compiled by CI only, which the report must
  state.

#### 138.17 — Leg capability tables and routing combination

Scope: `freminal/src/gui/host_capabilities.rs`, `freminal/src/gui/osc99/mod.rs`.

What:

- Implement the leg capability table and the routing combination rules exactly as in "Leg
  capabilities and `p=?` truthfulness". Every routing variant and every platform is classified
  with no wildcard arm.
- The toast leg's capabilities are constants.
- The macOS and Windows rows are selected by `cfg!`, so all of them are compiled and tested on
  every platform through a platform parameter in the pure combination function.

Deliverable:

- A truth-table test over routing × platform × daemon state, asserting the full `p=?` reply
  string for: Toast (all platforms); Both on Linux with a full daemon; SystemWhenUnfocused on
  macOS; and System on Windows.

#### 138.18 — Documentation

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`,
`Documents/KITTY_PROTOCOL_REFERENCE.md` (OSC 99 section to the kitty 0.49.2 baseline),
`config_example.toml` (the `[notifications]` OSC 99 comments only).

What:

- The dual-document update per `freminal-escape-sequence-docs`, with the "Last updated"
  header.
- The OSC 99 rows describe the shipped behaviour, the leg capability rules and the platform
  limits.
- The GAPS entries closed by Task 138 get "closed by" pointers.
- The config comments describe the routing's effect on `p=?`.

Deliverable: docs that pass markdownlint, with every claim checked against the code.

### 138 Cleanup entries

#### 138.C1 — Split `gui/toast.rs` into a `gui/toast/` module directory

- **Surfaced:** 138.8 (sub-agent report, confirmed by the orchestrator). After 138.7a and
  138.8, `freminal/src/gui/toast.rs` is about 4,700 lines: about 2,500 of production code
  (stack and lifecycle, measurement, layout, the button flow, hit-testing) plus about 2,200 of
  tests.
- **Impact:** module cohesion (`freminal-module-cohesion`). One file holds several concepts. A
  sibling `toast_buttons.rs` would have to widen the visibility of private layout helpers,
  which that skill forbids, so the fix is a directory module.
- **Fix:** a move-only split into `gui/toast/mod.rs` (types, `ToastStack`, lifecycle and
  events), `gui/toast/layout.rs` (measurement inputs, `layout_toasts`, `layout_one_toast`,
  anchors), `gui/toast/buttons.rs` (button flow and quads) and `gui/toast/hit.rs` (hit-testing
  and click targeting). Tests move with their code. Visibility is `pub(super)` within the
  directory only, and nothing outside `gui::toast` changes.
- **Verification:** `git diff -M` shows moves; the test count is unchanged; clippy is clean.
- **Scheduling:** in Task 138, after 138.18 and before the adversarial review.
- **Status:** done. The files are `mod.rs` (1,903 lines), `layout.rs` (2,189), `buttons.rs`
  (282), `hit.rs` (310) and `test_support.rs` (135), at commit time (138.C14 added
  `input.rs` and grew the others). The 2,084-test count is identical
  before and after. The orchestrator confirmed the move is move-only with a sorted-line
  multiset diff: only visibility prefixes, imports and rustfmt re-wraps differ. Nothing is
  visible outside `gui::toast`.
  - The orchestrator compared rustdoc against `main` and found 15 intra-doc links the branch
    had broken: 13 from this move and 2 from 138.16. All are fixed. Four links to private
    sibling items became plain backticks rather than widening visibility. The branch adds no
    rustdoc link warnings compared with `main`.

#### 138.C2 — The OSC 99 signal pump does not filter signals by sender

- **Surfaced:** 138.12 (sub-agent report, confirmed by the orchestrator).
- **Impact:** the match rule selects `org.freedesktop.Notifications` signals on
  `/org/freedesktop/Notifications` from any sender. A session-bus peer that guesses a live
  D-Bus id can forge `ActionInvoked` or `NotificationClosed`. That makes freminal send an
  activation or close report (and possibly a focus) for one of its notifications. The old
  notify-rust path had the same exposure, and a session-bus peer can already do more harmful
  things.
- **Fix:** at start, resolve the unique name that owns `org.freedesktop.Notifications`
  (`GetNameOwner`) and drop signals whose sender differs. Re-resolve on `NameOwnerChanged` for
  that name, so a daemon restart keeps working. zbus's client-side match does not resolve
  well-known sender names, so this is done in `parse_signal` / the dispatcher, not by putting
  a well-known name in the match rule. Over p2p (tests) there is no sender, and filtering is
  skipped.
- **Verification:** a `DispatcherState` test showing that a signal from a non-owner is
  ignored, and an owner change re-targets.
- **Scheduling:** in Task 138, with 138.13 (same files).
- **Status:** done (see the 138.C2 status note).

#### 138.C3 — Reload Config does not reach the window it was triggered from

- **Surfaced:** 138.15 (sub-agent report, confirmed by the orchestrator: `KeyAction::ReloadConfig`
  is dispatched from `actions.rs` inside `update()`, while the window being rendered is out of
  `self.windows`). Predates 138.
- **Impact:** `reload_config_from_disk` → `apply_new_config` broadcasts theme, cursor,
  host-capability and every other per-pane change only to the windows in the map. The window
  where the user pressed the key or clicked the menu entry, usually the only window, misses
  them until something else re-sends them. For OSC 99 this means a reloaded routing change is
  not advertised in that window's panes. The Settings-window Apply path is unaffected.
- **Fix:** thread `current: Option<&PerWindowState>` through `reload_config_from_disk` and
  `apply_new_config`. Every per-pane loop in `apply_new_config` goes through the existing
  window iteration plus `current` (as the 138.15 broadcast helper does). Pass `Some(win)` from
  the `ReloadConfig` dispatch and `None` from the Settings Apply path.
- **Verification:** a test that `apply_new_config`'s per-pane fan-out reaches a passed
  in-flight window (through the extracted helper); the existing settings tests stay green.
- **Scheduling:** in Task 138, after 138.18 and before the adversarial review (with 138.C1).
- **Status:** done. The sub-agent verified the callers:
  - Reload Config (shortcut and menu) runs with the window out of the map, and now passes
    `Some(win)`.
  - The Settings window's Apply runs with every terminal window in the map, and passes `None`.
  - There are no other callers.

  Every per-window loop in `apply_new_config` goes through `with_current`. The 50 ms
  follow-up repaint is also requested for the in-flight window.

#### 138.C4 — Standard icon names on the toast leg

- **Surfaced:** orchestrator review of 138.18.
- **Impact:** the spec says the eight standard names (`error`, `warn`/`warning`, `info`,
  `question`, `help`, `file-manager`, `system-monitor`, `text-editor`) must be supported. The
  toast leg (`toast`, and the toast half of `both` / `system_when_unfocused`) shows only its
  kind icon.
- **Fix:**
  - `ChromeIcon` gains glyphs from the bundled CaskaydiaCove Nerd Font for the names it lacks
    (`Warning` exists). The existing all-glyphs-resolve regression test covers the new ones.
  - A pure `standard_icon_glyph(name) -> Option<ChromeIcon>` is added.
  - The toast spec carries an optional icon override: the first `n=` name that is standard,
    names in order. The toast draws that glyph in its icon slot instead of the kind icon. The
    accent colour still follows the kind.
  - A non-standard name or transmitted data leaves the kind icon. Toasts render no images,
    and that stays a documented toast limit.
- **Verification:** unit tests for the mapping, that the spec carries the override, and that
  layout uses the override glyph. The glyph-resolves test covers every new `ChromeIcon`.
- **Scheduling:** in Task 138, before the adversarial review.
- **Status:** done. The new glyphs, all FontAwesome codepoints proven present in the bundled
  font, are:

  | Variant         | Codepoint | Nerd Font name          |
  | --------------- | --------- | ----------------------- |
  | `Error`         | U+F057    | `nf-fa-times_circle`    |
  | `Info`          | U+F05A    | `nf-fa-info_circle`     |
  | `Question`      | U+F059    | `nf-fa-question_circle` |
  | `Help`          | U+F1CD    | `nf-fa-life_ring`       |
  | `FileManager`   | U+F07C    | `nf-fa-folder_open`     |
  | `SystemMonitor` | U+F0E4    | `nf-fa-tachometer`      |
  | `TextEditor`    | U+F15C    | `nf-fa-file_text`       |

  `standard_icon_glyph` lives in `icons.rs` (all platforms). `Toast::icon()` is the single
  source for measuring and drawing, and the accent colour stays per kind.

#### 138.C5 — Standard icon names on the macOS/Windows system leg

- **Surfaced:** orchestrator review of 138.18.
- **Impact:** the untracked backend passes names to `notify-rust`'s `icon()`, which is a no-op
  on both platforms, so standard names are never shown.
- **Fix:**
  - When the first resolvable icon is a standard name (names first, as on Linux), rasterise
    the 138.C4 `ChromeIcon` glyph from the embedded bundled font to a 256×256 RGBA PNG
    (swash, encoded with the workspace `image` crate) and send it through the existing
    `IconTempFiles` / `image_path` path.
  - Otherwise use transmitted data, as today.
  - The rasteriser is a pure function, compiled for macOS, Windows and tests.
- **Verification:** unit tests that every standard name rasterises to a non-empty 256×256 PNG
  with non-transparent pixels, and that a standard name beats transmitted data.
  `check-windows` must pass.
- **Scheduling:** in Task 138, after 138.C4. When both land, the OSC 99 docs rows move to ✅,
  and that change is part of 138.C5.
- **Status:** done. `rasterize_chrome_icon_png` renders the bundled glyph with swash: white,
  on an opaque `#5A5A5A` rounded tile, so it reads on light and dark notification
  backgrounds.
  - The original `#404040` glyph was changed at orchestrator review because it is invisible
    on dark backgrounds. The orchestrator inspected the rendered PNGs visually.
  - The untracked backend tries names first (the first standard name, rasterised at 256 px),
    then transmitted data.
  - The OSC 99 rows in COVERAGE and the kitty reference are now ✅, and the GAPS row is
    removed.

#### 138.C6 — Bounded, linear-cost OSC 99 reassembly

- **Surfaced:** adversarial review, PTY findings P1, P6 and P7.
- **Fix:**
  - **P1, list metadata.** `PendingNotification` keeps a running list-metadata cost, charged
    only for newly merged items at `len + 32` bytes per entry. `n=` and `t=` are capped at 64
    entries each and `a=` edits at 64; a breach drops the transfer through the tombstone path.
  - **P6, aggregate budget.** A handler-wide budget of 8 MiB applies across every pending
    entry, the unidentified slot included. A chunk that would exceed it evicts
    least-recently-touched entries first, and if that is still not enough the transfer is
    dropped. Title, body and button accumulators are capped at 64 KiB each, because
    finalisation cuts them to 1024 / 8 × 256 characters anyway. Icon data keeps the 1 MiB cap.
  - **P7, per-chunk copy.** The content path no longer clones the whole command. The handler
    passes the payload by reference into the assembler and clones metadata only.
- **Tests:** a hostile stream of 100k tiny `n=` chunks is dropped early (assert entry count,
  not time); the aggregate budget evicts and then drops; a large icon chunk is not copied
  (assert through the assembler API that no owned clone of the payload is made).
- **Status:** done.
  - A typed `Osc99Reject` covers the list caps and the budget. `ChunkError` is shared with
    graphics, so it is left alone.
  - The running costs are kept per slot and per store, with a test-only consistency check.
  - The command cannot be moved (`handle_osc` takes `&AnsiOscType`), so reassembly takes
    `&Osc99Command` and the payload slice goes straight into the assembler.
  - The budget is checked after a chunk lands, so it can be exceeded by at most one chunk
    (1 MiB) before eviction; this is documented.
  - Evicted entries are removed, not tombstoned, as with the count cap.

#### 138.C7 — OSC 99 parse and finalisation fidelity

- **Surfaced:** adversarial review, PTY findings P2–P5, P8–P11, P13 and P14.
- **Fix:**
  - **P2, icon priming.** A finalised notification with no title and no body but with icon
    data and a `g=` key is not displayed. Instead it pushes a new
    `WindowManipulation::Osc99CacheIcon { key, data }`, which the GUI stores in the `g=`
    cache (kitty caches before its no-title check). Anything else with no title and no body
    is still dropped silently.
  - **P3, truncation.** Over-length text is a hard cut at 1024 / 256 characters with no `…`,
    as kitty's `limit_size` does. The plan's citation is corrected.
  - **P4, rejected chunks.** When the parser rejects a sequence (item cap or size cap), the
    handler extracts `i=` and `d=` from the raw metadata, best effort, and tombstones a
    non-final id, so the rest of the transfer cannot assemble a truncated notification.
  - **P5, stale unidentified slot.** An identified content chunk discards an `Accumulating`
    unidentified slot. kitty's single pending command is replaced by any chunk with a
    different id.
  - **P8, name fields.** App name, types, icon names and sound name also have LF removed.
    Bidi controls are reviewed-not-a-defect (kitty parity).
  - **P9, kitty value parity.** An empty `c=` / `d=` counts as not `0` (report / done);
    `w=` beyond the `i64` range is clamped to the maximum. (Correction at execution: kitty
    strips `+` only after a leading `-`, so a bare `+report` stays an unknown word; the
    review's `+` item was wrong, and freminal matches kitty.)
  - **P10, dead metadata.** Finalisation goes into a narrow `MergedOsc99Metadata`, so the
    dead per-chunk fields and their pinning test are removed.
  - **P11, docs and visibility.** Stale docs are fixed, and `FinalizedNotification` becomes
    `pub(in crate::terminal_handler)`.
  - **P13, button numbering.** A button label is dropped only if it was empty before
    stripping. One that strips to nothing becomes `U+FFFD`, so later button numbers do not
    shift.
- **Tests (P14):**
  - DECSTR keeps pending state.
  - `i=0` end to end.
  - A plain multi-byte character split across chunks.
  - `g=` priming.
  - The trivial derive-only tests are removed.
  - The `tests/osc99_query.rs` replies use the production `host_capabilities` value through a
    shared helper rather than hard-coded strings.
- **Status:** done, for every item.
  - P2 pushes `Osc99CacheIcon` and the GUI stores it in the `g=` cache. It is collected in
    its own per-frame vector and stored before that frame's displays; 138.C10's ordered
    requests remove the remaining same-frame ordering gap.
  - P4 carries the rejected chunk's id and `d=` inside `Osc99ParseError`, through a
    temporary stand-in that 138.C8 replaces.
  - P9 deviates from the review where the review was wrong (see above). An empty `e=` stays
    unchanged.
  - The integration test cannot reach `host_capabilities`, which lives in the `freminal`
    crate, so it keeps explicit literals per feature set, documented in the file header.

#### 138.C8 — OSC 99 type placement and naming

- **Surfaced:** adversarial review, PTY finding P12.
- **Fix:**
  - `NotificationUrgency` and `NotificationOccasion` move to `osc99_notification.rs`, which
    breaks the import cycle; `osc_notify_99.rs` imports from there.
  - The `window_manipulation.rs` re-exports are removed and every import uses one path.
  - `PendingOsc99::is_tombstone` becomes `touch_if_tombstone`.
  - `Osc99FocusAction` is renamed `Osc99FocusSupport`.
  - The 138.C7 stand-in `Osc99PayloadType::Rejected` (a non-wire value in the wire enum) is
    replaced by a dedicated `AnsiOscType::Notify99Rejected(Osc99RejectedChunk)` variant,
    handled by the `Notify99` handler path.
  - The 130-era `Reported` / `NotReported` capability enums stay (reviewed: their meaning is
    "reported", not "supported").
- **Verification:** mechanical; the test count is unchanged.
- **Status:** done.
  - The import cycle is broken, and the `window_manipulation` re-exports are removed.
  - `touch_if_tombstone` and `Osc99FocusSupport` are in place.
  - `AnsiOscType::Notify99Rejected` replaces the stand-in, gated on OSC 99 support by
    `handle_osc99_rejected`.
  - The test count is 8526 before and after; one test was replaced by a `Display` test.

#### 138.C9 — D-Bus service robustness

- **Surfaced:** adversarial review, zbus findings Z1, Z2, Z4–Z6, Z12–Z15, Z17 and Z18.
- **Fix:**
  - **Z1 and Z2, owner tracking.** Each mapping records the unique name that answered its
    `Notify` (the reply header sender). An owner change closes only the instances recorded
    under a different owner. Only `NameHasNoOwner` from `GetNameOwner` means `NoOwner`; any
    other failure (or `NoOwner`) adopts the sender of the next successful `Notify` reply.
  - **Z4, pump loss.** If either pump ends, every mapped instance gets `Closed` and the
    service emits `Unavailable`.
  - **Z5, capabilities.** They are fetched again on every owner change, and a new
    `Capabilities` event is emitted.
  - **Z6, circuit breaker.** After a method timeout the dispatcher fails queued `Show`s
    immediately, without calling the daemon, for 30 s. Then it tries again.
  - **Z12, reused ids.** `record_shown` evicts a different instance that held the same D-Bus
    id.
  - **Z13, failed replacement.** A failed replacing `Notify` sends a best-effort
    `CloseNotification` for the old id.
  - **Z14, docs.** The shutdown docs say what actually exits. A failed pump spawn stops the
    already-started pump.
  - **Z15, fail-closed default.** `Osc99SignalSender` loses its `Default`.
  - **Z18, auto-start.** `GetNameOwner` and `GetCapabilities` are sent with `NO_AUTO_START`,
    so starting freminal does not D-Bus-activate a daemon. A missing daemon is logged at
    debug, and the first real `Notify` activates it; the resulting owner change refreshes the
    capabilities. After `Unavailable`, the service tries to reconnect on the next show, at
    most once per 30 s.
- **Tests (Z17):**
  - Owner mode is injectable, so the p2p rig covers the bus branch: owner filter, owner
    change, pump loss, and a failed `GetNameOwner`.
  - The handle drop ends the dispatcher.
  - Vacuous `try_events().next().is_none()` checks become sentinel round trips.
  - The two method-timeout tests are removed. They depended on a 2 s bound on calls the fake
    does answer, and the timeout itself is zbus behaviour. The error paths are covered by the
    error-reply tests.
- **Status:** done, for every item.
  - **`NO_AUTO_START` calls.** zbus's flag-taking blocking call has no timeout, so
    `GetNameOwner` and `GetCapabilities` run on a short-lived `freminal-osc99-call` helper
    thread bounded at 25 s. (**Superseded by 138.C26:** an async call raced against a timer, with
    no thread.)
  - **Reused D-Bus id.** The evicted instance gets `Closed`, so its registry leg cannot stay
    open.
  - **Rig.** It stamps sender headers on p2p messages, so the bus-mode owner logic is
    exercised end to end.
  - **Re-checks by the orchestrator.**
    - Deliberately broken versions of Z1, Z2, Z4 and Z13 are each caught.
    - `freminal` passed 15 consecutive runs, and the service tests 40 runs under CPU load.
    - The real-session-bus smoke tests were run again (temporary, not committed):
      capabilities, show, 3,000 forged closes ignored, real close.

#### 138.C10 — Registry, routing and GUI-glue correctness

- **Surfaced:** adversarial review, GUI findings G1, G2, G4–G9, G11, G13 and G14; zbus
  finding Z3; docs finding D9.
- **Fix:**
  - **G1, ordering.** One ordered `Vec<Osc99Request>` (display or control) per frame,
    routed in arrival order.
  - **G2, untracked-only instances.** An instance with no open leg after registration is not
    kept, but an update still forgets the old instance.
  - **G4, alive.** `p=alive` leaves out instances whose close was requested.
  - **G5, activation.** A repeated activation of an already activated instance is ignored.
    The deadline still applies to legs left open after an activation: expiry closes them.
  - **G6 and Z3, wake and focus.** The wake falls back to any live window: windows are
    registered on creation and removed on close. The focus drain un-minimises a minimised
    window before focusing it.
  - **G7, quota.** Each pane may hold at most 32 instances and evicts its own oldest first.
    The global 128 cap stays as a backstop.
  - **G8, bus down.** `SystemWhenUnfocused` with the daemon unavailable routes to the toast
    leg and advertises the toast leg (as `Both` does).
  - **G9, filters.** An absent body or app name is matched as `""` (kitty), and the docs are
    updated.
  - **G11, capability helpers.** A generic per-field helper replaces the copied
    union/intersection chains.
  - **G13, icon copies.** No redundant icon clones; system effects share the data through an
    `Arc`.
  - **G14, smaller items:**
    - the low-urgency lifetime is derived from `ToastKind::Info`;
    - `Osc99Instance` fields become private;
    - the registry eviction doc is corrected;
    - a busy cell defers display requests instead of dropping them;
    - `Osc99FailedSystemLegs` is folded into the sink;
    - the Settings editor compares normalised rules;
    - `set_osc99_daemon`'s warning names the effect per routing.
  - **D9, filter recompile.** A busy registry when recompiling the filter defers the config
    swap of `osc99_filters`, so the change is not lost.
- **Tests:** one per item (G3's extraction in 138.C13 makes the glue testable).
- **Status:**
  - **138.C10a (done):** G2, G4, G5 (each leg is asked to close at most once, and expiry
    still closes legs left open after an activation), G7, the `combine` half of G8, G9, G11,
    G13 (`Arc` effect payloads, `Cow` icon bytes) and part of G14.
    - Orchestrator change: `p=alive` lists an instance while it has an open leg that has not
      been asked to close. An activated `c=1` instance whose notification is still displayed
      stays listed, as in kitty. The sub-agent's `closing == Open` filter would have hidden
      it.
  - **Left for 138.C10b, because they need `app_impl.rs`:** the routing half of G8, the fold
    of `Osc99FailedSystemLegs`, the busy-cell defer and the daemon-warning wording.
  - **138.C10b (done):**
    - **G1.** One ordered `Vec<Osc99Request>` (display, control, icon priming) is routed in
      arrival order.
    - **G6 / Z3.** `Osc99Wake` tracks live terminal windows, registered at creation and
      removed on close or destroy, and falls back to any live window. The focus drain
      un-minimises before focusing; `set_minimized(false)` is a no-op on a window that is not
      minimised (winit 0.30.13, verified by reading the source).
    - **G8 routing.** `resolve_osc99_legs` substitutes the toast leg when the Linux leg is
      unavailable.
    - **D9.** A pending filter is retried every frame.
    - **G14.** Busy cells defer requests (at most 256, with a repaint wake), and the warning
      wording is corrected.
    - Deferred requests use the window focus of the frame that routes them. This only matters
      for a busy cell, which cannot happen single-threaded.
    - `Osc99FailedSystemLegs` is still not folded (its registry tests construct it), so the
      fold moves to 138.C13.

#### 138.C11 — Linux presentation fixes

- **Surfaced:** adversarial review, zbus findings Z7–Z11 and Z19.
- **Fix:**
  - **Z7, desktop fallback.** Undecodable icon data falls back to the `.desktop` icon for
    `f=`, then to `freminal`.
  - **Z8, name case.** `icon_exists` returns the matched on-disk stem, and that stem is what
    is sent.
  - **Z9, decoding.**
    - Decode into `into_rgba8`, without a copy.
    - Premultiply around the resize so transparent edges do not get dark fringes.
    - Apply the EXIF orientation.
    - Keep decoded icons in an 8-entry cache keyed by content hash.
    - WebP stays accepted (reviewed: same bounded decoder, receiver leniency).
  - **Z10, wire format.** The hints are serialised through a typed
    `#[derive(SerializeDict, Type)]` struct with `as_value` fields, so `image-data` is no
    longer one `Value` per byte.
  - **Z11, XDG scan.** The scan runs on a background thread started with the service, so
    `Notify` never waits for it; until it finishes, unknown names fall through. It uses
    `file_type()`, caps visited entries at 200k and logs truncation once.
  - **Z19:**
    - a `desktop-entry` hint of `freminal` is added;
    - a BOM is stripped from `.desktop` files;
    - file stems take precedence over `Name=`;
    - desktop IDs in subdirectories are mapped (`sub/x.desktop` becomes `sub-x`).
- **Reviewed, not defects:**
  - Zero-width non-joiner markup escaping (kitty parity, a recorded decision).
  - `n=` paths passed verbatim (kitty parity).
  - `ActivationToken` not consumed (a recorded decision).
  - The cfg covers non-Linux Unix targets freminal does not ship.
  - No fallback timer after `CloseNotification`: the notification spec requires the daemon
    to emit `NotificationClosed`. (**Superseded by 138.C23:** wayle 0.1.3 does not, so a
    successful call now counts as the close.)
- **Status:** done, for every item.
  - **Hints (Z10).** Serialised through `WireHints` / `WireImage` (`serde` + `zvariant::Type`
    with `as_value`). The image bytes are borrowed straight onto the wire, with no `Value`
    tree.
  - **Decoding.**
    - The decoded-icon LRU lives on `DispatcherState`.
    - EXIF orientation is applied, with the allocation check re-added by hand on that decode
      path.
    - Resizing is premultiplied.
  - **XDG scan.** It runs on a `freminal-osc99-icons` thread and falls through until it is
    done.
  - **`serde` dependency.** `serde` became a direct Linux/BSD dependency of `freminal`,
    because the derive needs it; it was already a workspace dependency.
  - **Real-bus check.** The orchestrator re-ran a real-session-bus smoke test (temporary, not
    committed). A notification with an `image-data` hint, category, `suppress-sound`, the
    desktop-entry hint and markup was accepted (`Shown`) and then closed.

#### 138.C12 — Split `osc99/dbus_service.rs` into a directory (move only)

- **Surfaced:** adversarial review, zbus finding Z16.
- **Fix:** `osc99/dbus_service/{mod.rs, wake.rs, state.rs, wire.rs, pump.rs}` plus a
  `test_rig.rs`, following 138.C1's rules. Run after 138.C9 and 138.C11.
- **Status:** done. The files are `mod.rs` (1,740 lines), `state.rs` (1,793), `wire.rs` (500),
  `pump.rs` (301), `wake.rs` (62) and `test_rig.rs` (669), at commit time (later cleanups
  changed them); about half of `mod.rs` and `state.rs` is tests.
  - The 114 service test names are identical before and after; `freminal` has 2,165 tests
    both times.
  - The orchestrator confirmed the move with a sorted-line diff: only re-wraps, imports and
    one helper path differ.
  - Nothing is visible outside `osc99::dbus_service`, and no rustdoc links are broken.

#### 138.C13 — Move the OSC 99 GUI glue out of `app_impl.rs`; split `osc99/mod.rs`

- **Surfaced:** adversarial review, GUI findings G3 and G12.
- **Fix:**
  - The nine OSC 99 methods move into `gui/osc99/glue.rs` as an `impl FreminalGui` block.
  - Their decisions are extracted into free functions over `&mut Osc99Registry`,
    `&mut ToastStack` and the window map, with unit tests: route a frame, expire, deliver
    toast and system events, feed back failed legs, drain focus requests.
  - `osc99/mod.rs` is split into `pane_state.rs`, `effects.rs` and `toast_spec.rs`.
  - The stale "four separate blocks" doc is fixed.
  - `Osc99FailedSystemLegs` is folded into the sink's vectors (moved here from 138.C10).
- **Status:** done. `app_impl.rs` went from 6,030 to 5,515 lines.
  - `osc99/glue.rs` has the OSC 99 `impl FreminalGui` block plus eight tested free functions
    (frame routing, the deferral cap, expiry, toast and system event delivery, failed-leg
    feedback, daemon-state recording, and the focus drain over an `Osc99FocusTargets`
    trait), with 28 new tests. Disabling the feedback pass makes the failed-leg tests fail.
  - `osc99/mod.rs` went from 837 to 214 lines, split into `pane_state.rs`, `effects.rs` and
    `toast_spec.rs`.
  - Two behaviour-neutral simplifications: the cells are borrowed once per frame rather than
    once per request, and the registry stays borrowed through the feedback pass. That removes
    an unreachable busy-warning path. No `FreminalGui` field visibility changed.

#### 138.C14 — Toast interaction and layout fixes

- **Surfaced:** adversarial review, toast findings T1 and T9–T17, plus the toast nits.
- **Fix:**
  - **T1, affordance.** Toast areas call `.clickable()` (pointer hand), and buttons get a
    hover and pressed fill.
  - **T9, press and release.** The press part comes from `press_origin` and must match the
    release part.
  - **T10, missing hover position.** Fall back to `interact_pos` when `hover_pos` is `None`.
  - **T11, light themes.** Buttons darken on light pills.
  - **T12, `w=` toasts.** `w=` toasts use `UntilDismissed` and the registry enforces the
    deadline, which makes the hover behaviour consistent; the docs are updated.
  - **T13, short lifetimes.** `Fixed` is clamped to at least `ANIM_IN + ANIM_OUT`.
  - **T14, eviction order.** Eviction prefers the oldest toast that is not `UntilDismissed`.
  - **T15, `replace_keyed`.** It closes any other toast holding `new_key`, sets
    `cached_is_animating`, and loses its unused return value.
  - **T16, overflow.** Toasts that would extend below the content rect are not laid out until
    space frees up; they stay on the stack.
  - **T17, docs and tests.** The stale `WindowId` docs are fixed. Tests for
    `layout_positioned` (two `WindowId::for_test`) and a headless egui `hit_test` are added.
  - **Nits:**
    - shared input types move to `toast/input.rs`, which breaks the `buttons` ↔ `layout`
      cycle;
    - `layout.rs` docs are corrected;
    - the test-support bool parameter becomes an enum;
    - the `Toast::id` doc is corrected.
  - **Test gaps:**
    - duplicate keys;
    - hover past the lifetime;
    - replace while fading;
    - short `Fixed`;
    - buttons at ppp 2 mid-animation;
    - dismiss rect versus button rects;
    - `measure_buttons` truncation.
- **Status:** done, for every item.
  - **T1.** The hand cursor is set with `HoverAffordance::clickable()` on each toast area's
    response (the cursor skill's sanctioned path; `output.cursor_icon` is never written).
    Buttons get hover and pressed fills.
  - **T9.** The press part is captured from the frame's own raw press event, because egui
    clears `press_origin` on release.
  - **T12.** `w=` toasts are `UntilDismissed`, and the registry owns the deadline.
  - **T14.** A just-pushed toast is never the one evicted.
  - **T16.** The oldest toast in a group is always placed, so a tall `UntilDismissed` toast
    can still be dismissed.
  - **`is_animating`.** The flag (which only drives repaint on pointer motion) stays set while
    the pointer is over a toast, so hover state clears when the pointer leaves.
  - **Structure.** `toast/input.rs` breaks the `layout` ↔ `buttons` cycle.
  - **Orchestrator re-check.** Reverting the press-part check, the position fallback or
    `.clickable()` makes the new headless tests fail.
  - **Not changed.** The pre-existing `resolve_anchor(is_origin_window: bool)` parameter is
    untouched; it predates Task 138 and only its stale justification doc was corrected. It is
    routed to 147.29.

#### 138.C15 — macOS/Windows untracked leg hardening

- **Surfaced:** adversarial review, toast findings T2–T8, plus the icon nits.
- **Fix:**
  - **T3, worker thread.** One long-lived delivery thread with a bounded queue of 16 (excess
    dropped with a debug log) does the temp-file writes and `show()`. The GUI thread only
    builds a request. The rasterised PNG is cached per `ChromeIcon`.
  - **T4 and T5, temp directory.** Each process gets its own directory,
    `freminal-osc99-icons-<pid>`. On Unix it is created with `create_dir` and mode `0700`,
    and refused if it is a symlink or owned by someone else.
  - **T6, untrusted image bytes.** Transmitted bytes are decoded with the bounded `image`
    decoder (moved into a shared module) and re-encoded as PNG before the OS sees them.
  - **T7, file lifetime.** Files are deleted by age (older than 10 minutes) as well as by the
    64-file cap, and the directory is removed at exit. The limit that Action Center may show a
    blank image after exit is documented.
  - **T8, macOS failure docs.** They now state that delivery failures cannot be observed.
  - **T2, Windows icon.**
    - Windows builds its toast directly with `tauri-winrt-notification` (a new direct Windows
      dependency, same version as the lock file), so the icon is an `appLogoOverride`.
    - The `Critical` → reminder claim is dropped.
    - The `appname` docs are corrected.
    - The file URI is checked for spaces and `%`.
  - **Nits:**
    - `write_exclusive` and symlink tests;
    - a test with two stores holding the same content;
    - `size_px` is capped at 1024;
    - `get_pixel_mut_checked` replaces `get_pixel_mut`.
- **Status:** done.
  - **Recovery.** The first sub-agent was killed by an upstream error before it reported.
    Its uncommitted work compiled cleanly. A fresh agent audited it item by item against this
    entry, found every item present and correct, and fixed one over-long doc line. The
    orchestrator then reviewed the worker, the directory-safety checks and the Windows URI
    encoding directly.
  - **Worker.** One bounded `freminal-osc99-deliver` worker (queue of 16, drop when full)
    does the image preparation and `show()`, with the PNG cached per `ChromeIcon`.
  - **Directory.** `freminal-osc99-icons-<pid>` is created `0700` with `0600` files.
    Symlinked or permissive existing directories are refused.
  - **Images.** Transmitted bytes are re-encoded through the shared bounded decoder
    (`icon_decode.rs`).
  - **Eviction.** Files older than 10 minutes are evicted, in addition to the 64-file cap.
  - **Windows.** The toast is built directly with `tauri-winrt-notification` 0.8.1 (one copy
    in the lock file), using `appLogoOverride` and a percent-encoded `file:///` path.
  - **Ownership check.** It is done by mode rather than uid, which would need unsafe or
    libc: a `0700` directory owned by someone else cannot be written by us anyway.
  - **macOS** was checked by reading the source only.

#### 138.C16 — Documentation and plan corrections from the adversarial review

- **Surfaced:** adversarial review, docs findings D1–D8 and D10–D15.
- **Fix:**
  - **D1:** markdownlint (`cargo xtask lint-markdown` / `markdownlint-cli2`) passes; the
    MD060 tables are re-padded.
  - **D2–D5:** stale icon and toast claims in COVERAGE, `config_example.toml`, the
    `toast_leg` doc and plan line "Toasts do not render icons" (marked superseded by C4);
    `toast.rs` paths in `KITTY_PROTOCOL_REFERENCE.md`, the plan and the
    `freminal-toast-options` skill.
  - **D6:** the stale 138.1 commit hash.
  - **D7 and D8:** closed markers on the 147 routing bullet, 130.C4 and the audit row.
  - **D10:** the `p=close` advertisement rationale on macOS and Windows.
  - **D11:** the `cargo test --all` mentions in `README.md` and
    `.github/copilot-instructions.md`.
  - **D12:** the MASTER_PLAN note.
  - **D13:** superseded plan text in 138.14, 138.15, 138.18 and the 138.C1 line counts.
  - **D14 and D15:** formatting artefacts, the `c=`/`d=` empty-value wording (after 138.C7),
    the `config_example.toml` key order, the `dbus_service.rs` doc tense, and a branch-base
    note that the branch was rebased and contains a `cargo update` commit.
- Runs last, after every code cleanup, and re-checks every OSC 99 doc claim.
- **Status:** done. Every item was applied. Every OSC 99 claim in COVERAGE, GAPS and the
  kitty reference was re-checked against the code as it stands after the cleanup work, and
  about 20 claims made stale by 138.C6–C15 were corrected. The re-check found one code
  defect, recorded as 138.C17.

#### 138.C17 — Windows advertised urgency it no longer honours

- **Surfaced:** 138.C16's docs re-check.
- **Defect:** 138.C15 dropped the `Critical` → reminder mapping (a reminder needs buttons),
  so the Windows toast has no urgency levels. `system_leg(Windows)` still advertised
  `u=0,1,2`, which is untruthful.
- **Fix:** both untracked platforms now advertise `Osc99UrgencySupport::Unsupported`; the
  `system_on_windows` test and the docs are updated.
- **Status:** done (orchestrator).

### 138 Adversarial review (2026-10-12)

Five parallel CODE-REVIEW passes covered the whole `main...HEAD` diff: PTY side, GUI
registry and glue, D-Bus leg, toast and macOS/Windows leg, and docs/config/plan. Three passes
failed on an upstream error before reporting and were re-run in full. Every finding has a
disposition; nothing is left as an informal note.

| Finding                                 | Severity | Disposition                     |
| --------------------------------------- | -------- | ------------------------------- |
| P1 quadratic list metadata cost         | major    | 138.C6                          |
| P2 icon-only `g=` priming dropped       | minor    | 138.C7                          |
| P3 `…` truncation not kitty's           | minor    | 138.C7                          |
| P4 rejected middle chunk truncates      | minor    | 138.C7                          |
| P5 stale unidentified slot              | minor    | 138.C7                          |
| P6 no aggregate memory budget           | minor    | 138.C6                          |
| P7 per-chunk command clone              | minor    | 138.C6                          |
| P8 LF in names; bidi controls           | minor    | 138.C7; bidi: not a defect      |
| P9 empty `c`/`d`, `+report`, huge `w`   | nit      | 138.C7                          |
| P10 dead per-chunk metadata             | nit      | 138.C7                          |
| P11 stale docs, `pub` visibility        | nit      | 138.C7                          |
| P12 naming, cycle, double paths         | nit      | 138.C8                          |
| P13 stripped button shifts numbering    | nit      | 138.C7                          |
| P14 PTY test gaps                       | minor    | 138.C7                          |
| Z1 owner change closes new daemon's     | major    | 138.C9                          |
| Z2 `GetNameOwner` error → deaf forever  | major    | 138.C9                          |
| Z3 wake targets a dead window           | major    | 138.C10                         |
| Z4 pump loss unhandled                  | minor    | 138.C9                          |
| Z5 capabilities never refreshed         | minor    | 138.C9                          |
| Z6 hung daemon serialises failures      | minor    | 138.C9                          |
| Z7 bad icon data skips `.desktop`       | minor    | 138.C11                         |
| Z8 icon-name case mismatch              | minor    | 138.C11                         |
| Z9 decode copies, fringes, EXIF, WebP   | minor    | 138.C11; WebP: not a defect     |
| Z10 `image-data` one `Value` per byte   | minor    | 138.C11                         |
| Z11 XDG scan cost and blocking          | minor    | 138.C11                         |
| Z12 reused D-Bus id orphan              | minor    | 138.C9                          |
| Z13 failed replace leaves old shown     | nit      | 138.C9                          |
| Z14 shutdown docs, orphan pump          | nit      | 138.C9                          |
| Z15 fail-open `Default`                 | nit      | 138.C9                          |
| Z16 `dbus_service.rs` size              | minor    | 138.C12                         |
| Z17 D-Bus test gaps, vacuous checks     | minor    | 138.C9                          |
| Z18 daemon auto-start, noisy warn       | nit      | 138.C9                          |
| Z19 markup, hints, `.desktop` details   | nit      | 138.C11 (partly not defects)    |
| G1 same-frame display/control order     | major    | 138.C10                         |
| G2 untracked-only instances kept        | major    | 138.C10                         |
| G3 GUI glue untested                    | major    | 138.C13                         |
| G4 alive lists closing instances        | minor    | 138.C10                         |
| G5 sticky state after activation        | minor    | 138.C10                         |
| G6 wake loss; focus of minimised window | minor    | 138.C10                         |
| G7 one pane evicts another's instances  | minor    | 138.C10                         |
| G8 `SystemWhenUnfocused` with bus down  | minor    | 138.C10                         |
| G9 filter on absent fields              | minor    | 138.C10                         |
| G10 `o=` on Wayland, occlusion          | minor    | 138.C16 (doc note)              |
| G11 repetitive capability helpers       | minor    | 138.C10                         |
| G12 `app_impl.rs` growth, mixed module  | minor    | 138.C13                         |
| G13 wasted icon copies                  | nit      | 138.C10                         |
| G14 smaller GUI items                   | nit      | 138.C10; `from_bool`: precedent |
| T1 no cursor or hover affordance        | major    | 138.C14                         |
| T2 Windows inline image, reminder claim | major    | 138.C15                         |
| T3 untracked leg unbounded, GUI thread  | major    | 138.C15                         |
| T4 cross-process temp-file collisions   | minor    | 138.C15                         |
| T5 temp directory hardening             | minor    | 138.C15                         |
| T6 unvalidated bytes to OS decoders     | minor    | 138.C15                         |
| T7 temp-file lifetime vs OS read        | minor    | 138.C15                         |
| T8 macOS failure docs                   | minor    | 138.C15                         |
| T9 press and release on different parts | minor    | 138.C14                         |
| T10 click lost without hover position   | minor    | 138.C14                         |
| T11 buttons invisible on light themes   | minor    | 138.C14                         |
| T12 hover pause defeated for `w=`       | minor    | 138.C14                         |
| T13 `Fixed` shorter than animation      | minor    | 138.C14                         |
| T14 eviction ignores lifetime           | minor    | 138.C14                         |
| T15 `replace_keyed` edge cases          | minor    | 138.C14                         |
| T16 tall stacks overflow                | minor    | 138.C14                         |
| T17 stale `WindowId` docs; tests        | minor    | 138.C14                         |
| Toast and icon nits, test gaps          | nit      | 138.C14, 138.C15                |
| D1 markdownlint MD060 failures          | major    | 138.C16                         |
| D2–D8, D10–D15 doc and plan drift       | minor    | 138.C16                         |
| D9 filter change lost on busy borrow    | minor    | 138.C10                         |
| C16 re-check: Windows advertises `u`    | minor    | 138.C17                         |

Execution order: 138.C6, C7, C8, C9, C11, C12, C10, C13, C14, C15, C16. Each goes through
sub-agent implementation, orchestrator review and the verification suite. A confirmation
review of the cleanup diff follows.

#### 138.C18 — PTY follow-ups from the confirmation review

- **Surfaced:** confirmation review, PTY findings N2, N3, N4, N5 and N8.
- **Fix:**
  - **N3, name lengths.** The parser caps a decoded `f=`, `s=`, `n=` or `t=` value at 256
    characters and ignores a longer one. This bounds the scalar metadata outside the
    pending budget.
  - **N4, rejected id-less chunk.** A rejected `d=0` chunk with no id tombstones the
    unidentified slot even when nothing is pending, so the rest of that transfer cannot
    assemble a truncated notification. A rejected final chunk with no id stays a no-op.
  - **N5, doc.** The `Osc99Closing::Requested` doc is corrected to match how `alive` behaves.
  - **N8, strip order.** The strip-then-limit order is documented as a deliberate difference
    from kitty, which limits first. freminal keeps more printable characters, and it
    promotes the body when the title strips to nothing.
  - **N2, size cap.** A chunk over 1 MiB is dropped silently by the OSC accumulator, before
    OSC 99 sees it. The spec limits chunks to 4096 bytes, so only a non-conforming sender is
    affected. Reviewed and documented, not a defect. The P4 status wording and the
    rejected-oversized test's doc say that the path is reachable only from a direct call.
- **Tests:** a long `f=` is ignored; a rejected id-less `d=0` chunk tombstones the slot.
- **Status:** done. A rejected id-less final chunk still ends an accumulating unidentified
  transfer, as it did before, and is a no-op only when nothing is pending. The name cap
  counts characters, not bytes.

#### 138.C19 — GUI follow-ups from the confirmation review

- **Surfaced:** confirmation review, GUI findings N1, N6 and N7.
- **Fix:**
  - **N1, pruned panes.** `prune_dead_panes` returns close effects for the open legs of the
    instances it prunes, so a `w=` toast does not outlive its pane. The glue applies them.
  - **N6, focus and deferral tests.** The window calls of the focus drain go through a small
    trait whose test double records call order (un-minimise before focus). The busy-cell
    deferral decision is a plain function and is tested.
  - **N7, copies.** `register` takes the notification by value, so the `Arc` is built
    without copying it. Icon primes are consumed.
- **Tests:** one per item.
- **Status:** done.
  - Pruning closes the pruned instance's open legs through the existing feedback path.
  - `Osc99WindowOps` fixes the order: un-minimise, focus, repaint.
  - `plan_osc99_routing` decides with an `Osc99CellAccess` enum.
  - Requests and the notification travel by value end to end. Two pointer-identity tests
    show the icon is not copied.

#### 138.C20 — D-Bus follow-ups from the confirmation review

- **Surfaced:** confirmation review, D-Bus findings 1, 2 and 7.
- **Fix:**
  - **Reconnect (1).** While unavailable, after a failed connect or lost signals, the
    dispatcher retries connecting every 30 s on its own timer (`recv_timeout`), with no
    `Show` needed. A successful reconnect emits `Capabilities`, so the GUI's daemon state
    becomes available again and the capabilities are re-broadcast. The surviving pump and
    the connection from the lost session are closed.
  - **Breaker wiring (2).** Dispatcher-level tests cover the `Skip` branches (show, close,
    replaced close, capability refresh), with the breaker forced open.
  - **Icon scan (7).** The XDG scan starts on the first notification that carries `n=` or
    `f=`, not with the service. The decoded-icon cache key is length-prefixed.
- **Tests:** reconnect after a failed connect through an injectable connect closure and
  clock; the breaker branches.
- **Status:** done.
  - One reconnect mechanism: a 30 s timer while the service is unavailable. The
    reconnect-on-`Show` gate is removed.
  - A reconnected session with no daemon to ask reports the assumed capabilities, so the
    GUI's daemon state recovers.
  - The breaker tests close the breaker again afterwards, so a handler that never calls the
    daemon cannot pass.
  - The XDG scan starts on the first query.
  - The service tests passed 10 consecutive runs. The orchestrator re-ran the
    real-session-bus smoke test (temporary, not committed): capabilities, show, close.

#### 138.C21 — macOS/Windows and toast follow-ups from the confirmation review

- **Surfaced:** confirmation review, toast/platform findings 4, 5 and 6.
- **Fix:**
  - **Ownership (4).** On Unix, an existing icon directory must also be owned by the current
    user. The uid is taken from a probe file created in the parent temp directory and
    compared with the directory's uid (`MetadataExt::uid`, no unsafe code).
  - **Stale directories (5).** On first use, sibling `freminal-osc99-icons-*` directories
    that pass the same checks and are more than a day old are removed.
  - **Fade-out (6).** `close_keyed` starts the exit fade instead of removing the toast. The
    toast is removed, and `Closed` is emitted, when the fade completes.
- **Tests:** a foreign-owned directory is refused (if a foreign uid cannot be made in a
  test, the comparison is unit-tested); the sweep removes old directories and keeps fresh
  ones; `close_keyed` fades, then emits `Closed` once.
- **Status:** done.
  - `owned_by` returns an `Ownership` enum. The probe uid is memoised per process by the
    orchestrator, so later checks need no extra file.
  - The sweep runs once per store and never follows symlinks.
  - `close_keyed` fades over `ANIM_OUT`. `Closed` now arrives from `sweep_expired` (the next
    `show`), as expiry already does. A closing toast ignores clicks and hover.
    `replace_keyed` on a closing toast cancels the close.

#### 138.C22 — Documentation follow-ups from the confirmation review

- **Surfaced:** confirmation review, docs findings 3, D12 and D11 (`opencode.json`).
- **Fix:**
  - COVERAGE's "urgency is advertised on Windows only" is removed.
  - The COVERAGE header range becomes 138.C6–138.C22.
  - The MASTER_PLAN note is updated.
  - The 138.C12 `wake.rs` line count is marked "at commit time".
  - `opencode.json`'s `cargo test --all` becomes `cargo xtask test`.
  - The reconnect and scan docs follow 138.C20.
  - Runs after 138.C18–C21.
- **Status:** done. The C18–C21 behaviour is reflected in COVERAGE and the kitty reference,
  including two stale claims there (the parser rejecting >1 MiB, and notifications staying on
  screen after their pane closes). Strip-then-limit is listed as deliberate difference 6.

#### 138.C23 — A successful `CloseNotification` is the close

- **Surfaced:** maintainer manual test with `test-scripts/osc99_notifications.py` against
  wayle 0.1.3. Steps 9 and 10 waited out their 25 s timeout with no `c=1` close report.
- **Cause:** wayle treats `expire_timeout` 0 (spec: "never expires") as already expired. It
  drops the notification from its tracking list without a signal, so a later
  `CloseNotification` emits no `NotificationClosed`. This was confirmed with a zbus probe:
  timeouts of -1 and 8000 get the signal and 0 never does. freminal, like kitty, waited for
  the signal. The leg stayed open forever, so the close report never came and `p=alive` kept
  listing the notification. The same happens after a desktop activation, because wayle
  removes the activated notification the same silent way.
- **Fix (maintainer decision; differs from kitty):**
  - `close_instance` drops the mappings and emits `Closed` after any `CloseNotification`
    outcome. The daemon's own signal, if it comes, is for a forgotten id and is ignored.
  - After an `Activated` event the service closes the activated notification.
    freminal never sends `resident`, so the spec has the daemon remove it anyway.
  - The test rig's `fail_close` bool becomes a `CloseAnswer` enum (`CloseAndSignal` /
    `CloseSilently` / `Fail`).
- **Tests:** a close the daemon never signals is reported once; an activation the daemon
  never follows with a close still closes; a desktop activation calls `CloseNotification`;
  forged-signal and adopted-owner tests now expect the follow-up `Closed`.
- **Status:** done.

#### 138.C24 — Named icons also travel as `image-path`

- **Surfaced:** the same manual test, step 14. wayle's default `icon-source = "automatic"`
  ignores `app_icon` and draws only `image-path` / `image-data`, so the standard icon names
  showed no icon. Transmitted data (step 15) did show.
- **Fix (maintainer decision; differs from kitty):** an icon chosen from the notification's
  own `n=` names goes in both `app_icon` and the `image-path` hint. The `.desktop` icon and
  freminal's own icon are the application's icon and stay `app_icon`-only. The hints carry a
  `NotifyImage` enum (`None` / `Path` / `Data`) in place of `Option<Osc99IconImage>`, so
  path and data cannot both be set.
- **Tests:** `image-path` on the wire; standard, matched and verbatim names give `Path`; the
  desktop and freminal fallbacks give `None`; a rig test sees `app_icon` and `image-path`.
- **Status:** done.

#### 138.C25 — Bound the D-Bus command queue

- **Surfaced:** CodeRabbit on PR #555 (`dbus_service/mod.rs`).
- **Problem:** the GUI-to-dispatcher command channel was unbounded. Each `Show` carries a
  full `Notification99Data` clone, icon bytes included. While a hung daemon holds the
  dispatcher for up to 25 s per call, a program flooding OSC 99 grows the queue without
  limit. The untracked leg already drops what does not fit its bounded queue.
- **Fix:** a bounded channel (`COMMAND_QUEUE_CAPACITY` = 256) with `try_send`. A full queue
  returns `Osc99SystemDispatch::Failed`, which the registry already handles by ending the
  leg. The GUI thread never blocks.
- **Tests:** a show and a close to a full queue both fail without blocking and queue no event.
- **Status:** done.

#### 138.C26 — No helper thread for `NO_AUTO_START` calls

- **Surfaced:** Copilot on PR #555 (`dbus_service/wire.rs`).
- **Problem:** `GetNameOwner` and `GetCapabilities` ran on a detached helper thread so they
  could be timed out. A timed-out thread stayed blocked until a reply or the end of the
  connection. The cooldown spaced out new probes but did not cap their number, so a daemon
  that kept calls pending leaked one thread and one connection clone per probe.
- **Fix:** `call_no_auto_start` builds the call with the async proxy on the blocking
  connection's inner connection and races it against a `smol::Timer` under `smol::block_on`.
  This is the async-io runtime zbus already uses; `smol` was already a workspace dependency.
  On a timeout the pending call is dropped, so nothing waits on.
- **Tests:** an unanswered call times out with zbus's timeout error, and the connection then
  answers a normal call.
- **Status:** done.

#### 138.C27 — OSC 99 filters editor placement

- **Surfaced:** CodeRabbit on PR #555 (`settings.rs`).
- **Problem:** the filters editor (its own heading and help text) sat between the "OSC 99" and
  "Command finished" routing rows, so the "Command finished" row read as a filter setting.
- **Fix:** the editor now follows the last routing row.
- **Status:** done.

### 138 Confirmation review (2026-10-12)

Two CODE-REVIEW passes over `1986117d..HEAD` checked every finding in the table above.

- **Verified:** every finding, with the partials below.
- **P4:** partial; see C18 N2.
- **P14:** partial. The integration test cannot reach `host_capabilities`, which lives in
  another crate; that is documented and accepted.
- **G6:** partial; see C19 N6.
- **G13:** partial; see C19 N7.
- **D12:** partial; see C22.

The new findings are disposed of in 138.C18–C22.

### 138 Pre-PR gates (2026-10-12)

| Gate                                                       | Result                                         |
| ---------------------------------------------------------- | ---------------------------------------------- |
| `cargo xtask test`                                         | 8,839 passed, doctests pass (after C23–C27)    |
| `cargo clippy --all-targets --all-features -- -D warnings` | clean                                          |
| `cargo fmt --all -- --check`                               | clean                                          |
| `cargo machete`                                            | clean                                          |
| `cargo xtask check-windows`                                | clean (macOS checked by reading source only)   |
| `cargo bench --no-run --all`                               | compiles                                       |
| rustdoc intra-doc links vs `main`                          | no new broken links                            |
| `cargo deny check`                                         | advisories fail, identically on `main` (below) |

- **Benchmarks.** `buffer_benches` against a `main` baseline, isolated runs, all within noise
  (threshold 15%):
  - `parse_plain_text`: +1.2%
  - `parse_sgr_heavy`: +0.7%
  - `parse_osc9`: notification −24% (p=0.08), progress +2.1%
  - `handle_incoming_data`: +0.8%
  - `handle_sgr_indexed`: +2.0%

  OSC 99 itself has no benchmark; no bench feeds it.

- **`cargo deny` advisories.** RUSTSEC-2026-0206 (`rustybuzz` unmaintained) and
  RUSTSEC-2026-0192 (`ttf-parser` unmaintained) also fail on `main`. They predate Task 138
  and come from the font-shaping stack. Task 138 does not address them; they are for the
  maintainer to route. (#530 has since been resolved on branch `task-530/harfrust-migration`:
  `rustybuzz` replaced by `harfrust` and `ttf-parser` removed from the graph.)

### 138 Status notes

- **138.1 (done, `061cdc39`).** Types as decided; `window_manipulation.rs` re-exports
  `Notification99Data` / `Osc99ControlKind` from the new module. Out-of-scope edits accepted
  as mechanical: `Osc99Features` struct literals in test fixtures in
  `terminal_handler/mod.rs`, `gui/pty.rs` and `tests/osc99_query.rs`. Intended observable
  changes: `s=system` no longer forwards a `"system"` sound name, and an empty title falls back
  to the body. `route_osc99_unrecognised_occasion_treated_as_always` was deleted because it can
  no longer be written (`NotificationOccasion` is exhaustive). The toast-title fallback test
  can only check the toast count; 138.7 adds title/detail assertions. Orchestrator change: the
  `Osc99SoundNames::Standard` doc names the OSC 99 standard sounds, not freedesktop sounds.
- **138.2 (done).** `ParseState` is removed; the parser fills `Osc99Command` directly, so there
  is no mirror struct and no `bool`. `osc99_expiry_from_ms` and `resolve_osc99_actions` are
  public in `osc_notify_99.rs` (one implementation each). Malformed tokens still count toward
  the 64-item cap. An empty `c`/`d`/`e`/`o`/`u`/`w`/`i`/`g` is ignored, and an empty `p=` is
  `Unknown`. Two orchestrator changes:
  - an `f`/`n`/`s`/`t` value that decodes to the empty string is ignored (kitty treats an empty
    name as unset);
  - the payload is copied only after the metadata is accepted.

  The pre-existing `abandon_notification(.., done: bool)` parameter is removed by 138.3.

- **138.3 (done).** `handle_osc99` is the single entry point. Controls act at once and never
  touch `PendingOsc99`, which holds the by-id map with LRU eviction plus the unidentified
  slot. A looked-up tombstone counts as touched. Merged list metadata (`n`, `t`, `a` edits)
  counts toward the 1 MiB per-notification cap, so endless `d=0` chunks cannot grow it.
  `dispatch_finalized_osc99`, `control_kind` and `is_control_payload` are deleted. Content
  chunks are still cloned once, because `handle_osc` takes `&AnsiOscType`; controls clone
  nothing. Orchestrator change, and a plan correction: a complete chunk for a key with no
  pending entry is finalised without entering the store. The plan text asked for an
  eviction, which would have destroyed another key's in-progress transfer for no reason.
  Verification from this subtask on uses `cargo xtask test`, which runs nextest plus the
  doctests (main moved to nextest in PR #541).
- **138.4 (done).** The reply is built from every `Osc99Features` field in kitty's key order,
  with exhaustive matches. For the 138.1 defaults the reply now carries `u` before `p`, and `p`
  in kitty's order. Its content is unchanged until 138.17 changes the inputs.
- **138.5 (done).** The sub-agent stopped correctly because `freminal_windowing::WindowId`
  had no test constructor. The orchestrator re-scoped the subtask: `WindowId::for_test(n)`
  sits behind a new `freminal-windowing` feature, `test-utils` (the `-support` suffix is
  rejected by `clippy::cargo`), which `freminal`'s dev-dependencies enable. Production builds
  never enable it. Other changes:
  - `route_window_manipulation_events` lost its `ui` parameter (minimisation moved into the
    drain); the `WindowId` comes back with the 138.9 focus executor.
  - The new `gui/osc99` types are `pub` inside the private module (`redundant_pub_crate`).
  - `Osc99DisplayContext` holds `WindowFocus` and `Osc99PaneState` (no `bool`s).
  - A focused-but-minimised window reports Focused and Hidden.
  - Zoom and split occlusion are tested through the pure per-pane helper, because a split
    tree needs a PTY per pane.
  - Orchestrator change: the drained `Notification99Data` is moved out of its `Box`, not
    cloned.
- **138.6 (done).** `Osc99Registry` keeps a `Vec` in registration order, so the oldest is
  index 0. Lookups are linear, bounded by the 128-instance cap. Toast-only and id-less
  notifications are now registered. Until 138.7, a dismissed toast or an OS-dismissed
  notification is still listed by `p=alive`; the old code had the same over-reporting for
  system notifications. Instance fields with no reader yet are read by a `trace!` when the
  instance is forgotten, so no `dead_code` allow is needed; 138.7 adds the real readers.
  Orchestrator change: a stale `osc99_live` mention in the `frame_drain.rs` module doc.
- **138.7a (done).** The keyed API, events, the dismiss rect and `sweep_expired(now)` are in
  place.
  - `replace_keyed` has no placement parameter, as the plan states: a replaced toast keeps its
    placement and the `Pushed` fallback uses `TOP_RIGHT`. Adding the parameter would exceed
    `too_many_arguments`.
  - Clicks are resolved by the pure `click_target` plus `apply_click`, so `hit_test` removes
    clicked toasts itself.
  - Six temporary `expect(dead_code)` attributes (reason: wired by Task 138.7b) must all be
    gone after 138.7b: `ToastLifetime::Fixed`,
    `ToastLifetime::UntilDismissed`, `push_keyed`, `replace_keyed`, `close_keyed` and
    `take_events`.
  - Unkeyed layout and behaviour are unchanged; no existing layout test was edited.
- **138.7b (done).** `register` returns the toast effects, and the toast events are delivered
  once per window frame through `route_osc99_toast_events`. Both cells are borrowed before the
  events are taken, so a busy cell defers delivery to the next frame and loses nothing.
  - Instances keep `open_legs` (legs still displayed and observable) and `shown_legs` (legs
    ever shown). `UntrackedSystem` is never an open leg, because it cannot be observed
    closing.
  - Interim rule: the close report is suppressed whenever the instance was ever shown on a
    `LinuxSystem` or `UntrackedSystem` leg, so the old notification thread and the registry
    never both report. 138.12 and 138.16 replace this rule.
  - All six temporary `expect(dead_code)` attributes from 138.7a are removed.
  - The toast title/body test checks the `osc99_toast_spec` mapping, because `ToastStack` has
    no title accessor.
- **138.8 (done).** Toast buttons are laid out under the text, wrap at `MAX_PILL_WIDTH_PTS`,
  and are drawn as lightened `ToastQuad`s straight after their own pill. `ToastLayoutOutput`
  gains `button_quads` and `button_rects_logical`, and `ToastContent` keeps `push_keyed` and
  `replace_keyed` under the argument limit. Geometry without buttons is unchanged (178×40, and
  178×60 with a detail). Button quads reuse the pill shader, so they carry its small drop
  shadow; accepted. `toast.rs` reached about 4,700 lines (about 2,500 of production code);
  the move-only directory split is 138.C1.
- **138.9 (done).** `FocusPane` follows the activation reply, on body and button activations,
  whatever `a=report` says.
  - `route_osc99` now returns its effects, and every effect is applied through
    `apply_osc99_effects_queueing_focus` in `app_impl`.
  - The executor has no window access, so focus requests are queued, and
    `drain_osc99_focus_requests` runs at the very top of `update()`. That has to happen before
    the current window is taken out of `self.windows`. A repaint wake makes an idle window run
    the drain.
  - `focus_pane_in_tabs` switches the tab, clears another pane's zoom, and sends the
    `FocusChange` pair. Tested on a two-tab manager. The `FocusChange(false)` to a real old
    split pane is not asserted, because a split needs a PTY per pane; the path copies the
    click-to-focus code.
- **138.10 (done).** Instances gain a `deadline` and an `Osc99Closing { Open, Requested }`
  state, so `p=close` and expiry are idempotent and share `close_instance_at`. `tick(now)` and
  `next_deadline()` run once per window frame, through `expire_osc99_notifications`, just
  before toast events are delivered; the next deadline is folded into the window's repaint
  scheduling. `Osc99DisplayContext` carries `now`, so `register` measures the deadline from
  display. A busy cell retries after 16 ms.
- **138.11 (done).** `osc99_filters` is wired through every step of the config checklist:
  the field, the guard-test mutation, a load-path test, the Settings editor (which keeps a
  text buffer apart from the draft `Vec`, so blank lines can be typed), `config_example.toml`
  and the Nix option (nixfmt, statix and deadnix clean).
  - The compiled filter lives in `Osc99Registry`. It is checked after the occasion gate, is
    compiled at startup, and is recompiled on config apply only when the rules change.
  - Regexes are built with a 1 MiB size limit.
  - The Settings window is a separate OS window, so modal input suppression does not apply.
  - A pre-existing gap the sub-agent found, the Nix module missing
    `command_finished_template`, is routed to 147.28.
- **138.12 (done).** zbus 5.19.0 is a direct dependency on Linux/BSD only (`async-io` and
  `blocking-api`; `p2p` only for the dev-dependency), and there is still a single copy in the
  lock file. The service follows the decided design: one dispatcher, one pump that forwards
  into the dispatcher, the pure `DispatcherState`, and connection and subscription on the
  dispatcher thread.
  - The integration tests run a hand-written fake daemon over a p2p socket pair. The
    `#[zbus::interface]` macro expansion trips pedantic lints that would need allows. The
    orchestrator re-ran the 30 service tests 5 more times; all passed.
  - Wake: `Osc99Wake` keeps the `RepaintProxy` and the most recently rendered window, and is
    re-pointed every frame. Any window's frame drains the shared event channel.
  - The registry now owns the Linux leg's activation and close reports. Only
    `UntrackedSystem` still suppresses the close report. Toast and system activation share one
    path (`Osc99LegActivation`).
  - Review fix by the orchestrator: a show that cannot be queued (handle busy, service gone)
    used to leave the Linux leg open forever. `Osc99SystemBackend::show` now returns
    `Osc99ShowDispatch`, and a failure is fed back to the registry as `ShowFailed`.
  - Until 138.13, `p=close`, expiry and cap eviction drop the Linux leg silently and the
    desktop notification stays on screen. Until 138.14, Linux icons are not sent.
  - `Capabilities` and `Unavailable` are logged only; 138.15 consumes them.
  - Signals are not filtered by sender; see 138.C2.
  - The stale `host_capabilities.rs` doc is fixed by 138.15.
- **138.13 (done).**
  - **Close and update.** `CloseLinuxSystem` and `DbusCommand::Close` (through the pure
    `close_target`) are in place, as is the `Closed` fallback when a call fails or no id is
    known. Failed closes are fed back as `Closed`.
  - **Legs on close and activation.** `p=close`, expiry and cap eviction close every open leg.
    An update that drops the Linux leg closes the old notification. An activation closes the
    instance's other legs.
  - **Activation with `c=1`.** The instance enters `Requested` and reports once, after every
    leg has closed. As in kitty, freminal does not close the activated desktop notification
    itself.
  - **Flake (root-caused).** The orchestrator's re-runs caught a load-dependent test flake,
    about 1 in 6 full `freminal` runs. The fake daemon's message stream was created on its
    thread after the client could already have called it, so the call was lost and the client
    blocked.
    - Fix: the rig subscribes before the service starts.
    - Hardening exposed by the same analysis: production calls now carry a 25 s method timeout
      (the libdbus default), so a hung daemon cannot block the dispatcher forever. Two
      deterministic timeout tests cover it.
    - After the fix: 28 consecutive full `freminal` runs and 20 service-only runs all passed.
  - One out-of-scope edit was accepted: a sink struct literal in the `notifications.rs` test
    helper.
- **138.C2 (done).** Signals carry their sender, and the dispatcher believes only the current
  owner of `org.freedesktop.Notifications`.
  - The owner is resolved with `GetNameOwner` and followed by a second pump on
    `NameOwnerChanged`; a forged `NameOwnerChanged` not sent by `org.freedesktop.DBus` is
    rejected. A p2p connection is `Unfiltered`.
  - A change from one owner to another (or to nobody) closes every mapped instance, so no leg
    stays open after a daemon restart.
  - The orchestrator verified the filter against the real session bus on the development
    machine (daemon `:1.21`), using temporary ignored tests that were not committed. A real
    show and `CloseNotification` produced `Shown` then `Closed`. 5,000 forged
    `NotificationClosed` signals from a second connection produced no event.
- **138.14 (done).** Presentation is pure (`presentation.rs`). The XDG scans
  (`icon_lookup.rs`) are lazy and run on the dispatcher thread.
  - The scans follow symlinks, which NixOS profiles need, but enter each canonical directory
    only once. They are bounded at depth 6, 100k icon names and 20k `.desktop` files.
  - `Osc99IconCache` is LRU (64 entries / 4 MiB). Resolved bytes reach every system leg
    through `icon_data`.
  - When no rule finds an icon, `app_icon` is `freminal`. Tests use an empty lookup and never
    touch the real filesystem.
  - Orchestrator review fixes, both decisions:
    - Icon decoding goes through `ImageReader` with strict limits: 4096×4096 and a 64 MiB
      allocation cap. A 1 MiB PNG declaring 30000×30000 is rejected; a header-patched test
      covers this.
    - `image-data` is downscaled whenever a side exceeds 256, because zvariant encodes the
      byte array as one `Value` per byte; a 1024² icon would cost about 100 MB in transit.
      The plan's "over 1024 → 256" is superseded.
  - Sound hints are sent whatever the daemon's `sound` capability says (daemons ignore
    unknown hints). The advertisement follows the capability (138.17).
- **138.17 (done, before 138.15).** Legs are expressed as `Osc99Features` values. The pure
  functions are `toast_leg`, `system_leg(platform, daemon)`, `union`, `intersection` and
  `combine(routing, platform, daemon)`, with no wildcard arms.
  - The daemon-capability types moved to `osc99/capabilities.rs` for all platforms, with
    `Osc99LinuxDaemon` and `Osc99Platform::current()`.
  - With the Linux leg unavailable, `System` is `Unsupported` and `Both` is the toast leg.
    `SystemWhenUnfocused` is `Supported` with every optional feature off.
  - `Osc99LinuxDaemon::assumed()` is `Unavailable` off Linux/BSD, so that no variant goes
    unconstructed.
  - `body_markup` is gated to Linux/BSD, its only reader.
  - The full reply string is covered by 138.4's features-to-string table, because the reply
    builder is private to the emulator crate.
  - Interim: a one-argument `host_capabilities(config)` wrapper (passing `assumed()`) remains
    because `tab_spawning.rs` was out of scope; 138.15 removes it.
- **138.15 (done).** `FreminalGui::osc99_daemon` (a `Cell<Osc99LinuxDaemon>`) feeds every
  `host_capabilities` call, and the one-argument wrapper is gone.
  - The daemon's `Capabilities` and `Unavailable` events update the cell through
    `set_osc99_daemon`, which re-broadcasts only when the resolved capabilities change (pure
    `daemon_state_change`).
  - One broadcast helper, shared with config apply, takes the in-flight window explicitly,
    because the window being rendered is out of `self.windows`.
  - The service starts from `ensure_osc99_dbus_started` early in every `update()`, and after
    a config apply, whenever the config can use the system leg.
  - The end-to-end test drives a real `TerminalState`: after a no-`actions` capability
    change, `p=?` loses `a=` and `buttons`; `System` with the bus down gives no reply.
  - The sub-agent found that Reload Config already missed the in-flight window for every
    config dimension; recorded as 138.C3.
- **138.16 (done).** The registry now produces the untracked leg: `ShowUntrackedSystem`, plus
  the `p=close;untracked` reply at display when `c=1`.
  - The old per-notification thread, `build_osc99_notification`, the temp-file functions and
    the `cfg!(test)` guard are gone.
  - `NotifyRustUntrackedBackend` (macOS/Windows) builds the notification on the GUI thread,
    with no buttons, per-platform urgency and sound mapping, and an icon from
    `IconTempFiles`.
  - Windows shows inline.
  - macOS shows on a short-lived `freminal-osc99-deliver` thread. This is an orchestrator
    decision: the sub-agent found by reading the source that mac-notification-sys blocks until
    delivery confirmation, up to 2 s, even on the asynchronous drop path, so an inline call
    could stall the render thread.
  - Icon temp files are content-hash named, written with `create_new`, capped at 64 (LRU),
    and removed when the backend is dropped at shutdown. The windowing layer has no app exit
    hook, so files survive a crash.
  - Fixed on the way: Windows `s=system` used to be silent; it now plays the default sound.
  - macOS was not compiled locally. Its code paths were checked against the notify-rust and
    mac-notification-sys source, and macOS CI is the first compile. Windows passes
    `check-windows`.
- **138.18 (done).** COVERAGE, GAPS, `KITTY_PROTOCOL_REFERENCE.md` (0.49.2 baseline,
  including freminal's deliberate differences from kitty) and the `config_example.toml`
  routing comments are rewritten.
  - The orchestrator found that the eight standard icon names, a spec MUST, are honoured only
    on the Linux system leg; the docs agent had called this a platform limit. OSC 99 therefore
    stayed 🚧, with exactly that gap, and the fix was scheduled as 138.C4 and 138.C5.
    🚧 superseded by 138.C5: OSC 99 is ✅ in every document, and the docs were rewritten by
    138.C16 against the post-cleanup code.
  - The orchestrator checked the example `p=?` replies (macOS `both` and
    `system_when_unfocused`) against `combine`.

---

## Task 147 — v0.13.0 Correctness Cleanup

### 147 Summary

Fix every defect, spec deviation, missing test and tooling break that Tasks 126–131 and their
reviews surfaced but left without a scheduled owner. Also fix a pre-existing defect found in
the same review (application-keypad encoding), and two pre-v0.13.0 xterm deviations the
maintainer added on 2026-10-11 (147.18 and 147.19). The remaining pre-v0.13.0 charset and
mode gaps were also resolved on 2026-10-11: 147.20 implements the character sets, and 147.21
records five modes as not planned. 147.22–147.24 cover XTGETTCAP, the HP escapes and
Alt-sends-ESC. Created and decomposed on 2026-10-11 against
`72109bad`. The source was a maintainer review of PR #537: unscheduled findings are latent
bugs, so each one gets a subtask here or a line in the stub of the task that owns it.

The sweep re-read the Task 126–131 sections in full: status notes, cleanup entries,
adversarial, confirmation and orchestrator reviews, and plan-drift notes. It also covered
`ESCAPE_SEQUENCE_GAPS.md`, `ESCAPE_SEQUENCE_COVERAGE.md`, `KITTY_PROTOCOL_REFERENCE.md`,
source TODOs and the commit log. Every item it found ended up in one of four places:

- a subtask below;
- a line added to the stub of the task that owns the code (listed under "Routed to existing
  stubs");
- confirmed resolved;
- shown not to be a defect, with evidence (listed under "Reviewed, not defects").

Source review: PR #537 (Copilot and CodeRabbit).

### 147 Maintainer decisions (2026-10-11)

- **A bare ESC inside an OSC, DCS or APC string** (one not followed by `\`) ends the string,
  following Ghostty and WezTerm (the Williams VT state machine). The body received so far is
  dispatched. The ESC then starts a new escape sequence, so the next byte is parsed as part
  of it.
  - xterm instead drops the string. kitty, and freminal today, keep the ESC in the body.
  - Exception: inside `DCS tmux;` a doubled `ESC ESC` is data, exactly as today.
- **DECRC with DECOM set keeps clamping to the screen only.** kitty, Ghostty and WezTerm do the
  same. xterm alone clamps to the bottom and right margins (not the top or left). There is no
  code change; the GAPS entry is corrected and closed.
- **XTSMTITLE / XTRMTITLE (`CSI > Pm t` / `CSI > Pm T`)** are recognised and ignored without a
  warning, as kitty, Ghostty and WezTerm do. They are documented as not planned.
- **UTF-8 title and icon replies in S8C1T mode** keep sending raw UTF-8, matching xterm, the
  only reference terminal with S8C1T (`charproc.c:8950-8973`, `ptydata.c:614-629`). There is
  no code change; the GAPS entry is closed with that reasoning.
- **Layout startup commands go through the input channel**, so FREC records them and they
  are ordered with all other input.
- **The CSI parser reuses its buffers** across sequences: the larger 128.P1 option, now
  accepted.
- **`CSI Ps u` with any parameter is ignored**, as xterm (`charproc.c:4912-4925`), kitty
  (`vt-parser.c:1370-1374`) and WezTerm (`csi.rs` `noparams!`) do. Ghostty alone restores the
  cursor.
- **DECSC saves the pending-wrap flag**, as xterm (`cursor.c:408,494`), Ghostty
  (`gh_term.zig:2080,2116`) and WezTerm (`wez_ts.rs:2724,2763`) do. kitty loses it.
- **Application keypad (DECKPAM) sends SS3 sequences.** This follows the VT220 table in xterm
  (`ctlseqs.txt:2625-2652`, `input.c:1390-1400`) and Ghostty (`function_keys.zig:121-141`).
  kitty sends plain digits and WezTerm ignores DECKPAM. With the split at 2–2, the standing
  xterm + Ghostty tie-break applies.
- **Every CSI or SS3 introducer the terminal generates honours S8C1T**, as xterm does: key,
  focus, mouse and bracketed-paste reports all go through `unparseputc1`
  (`charproc.c:9356-9365`; `button.c:5525-5529,5694-5709`; `input.c:1341-1411`).
  - kitty, Ghostty and WezTerm have no S8C1T at all.
  - This covers the kitty-keyboard and `modifyOtherKeys` CSI encodings: they are CSI
    sequences, and xterm's rule is per introducer.
  - VT52 mode is unaffected, and so is Alt-prefixed text (`ESC` followed by a character).

### 147 Subtasks

Every subtask leaves `cargo xtask test` and
`cargo clippy --all-targets --all-features -- -D warnings` green. Parser and encoder subtasks
name their benchmark (`freminal-bench-table`) and record before and after results in their
status note (`performance-benchmarks`, 15% threshold). Escape-sequence behaviour changes are
documented in 147.17. A subtask may update a COVERAGE or GAPS row that it closes itself.

#### 147.1 — DECRQM for an unknown ANSI mode keeps its namespace (issue #528 remainder)

- **Source:** 130 confirmation review N2; GAPS "DECRQM for ANSI modes"; CodeRabbit on PR #537
  (`mode.rs:291`).
- **Defect:** `terminal_mode_from_params` drops the `?` when it builds
  `Mode::UnknownQuery(Vec<u8>)`, and `Mode::report` always formats `?{digits};0$y`. So an
  unsupported ANSI query such as `CSI 2 $ p` is answered as DEC private mode 2. The spec
  (`ctlseqs.txt:1502-1510`) gives the ANSI form `CSI Ps ; Pm $ y`, with no `?`.
- **Scope:**
  - `freminal-common/src/buffer_states/mode.rs`: `UnknownQuery` carries the existing
    `ModeNamespace`, as `Unknown(UnknownMode)` already does, and its construction sites and
    report arm use it;
  - `freminal-terminal-emulator/src/state/internal.rs` (the `UnknownQuery` arm);
  - `freminal-terminal-emulator/src/terminal_handler/mod.rs` (the DECRQM reply);
  - the tests in `mode_dispatch_tests.rs` and `decrpm_integration.rs`.
- **Deliverable:** `CSI 2 $ p` gets `CSI 2 ; 0 $ y` and `CSI ? 9999 $ p` gets
  `CSI ? 9999 ; 0 $ y`, both framed per S8C1T. Tests cover both namespaces in both framings.
- **Closes:** issue #528, when the PR merges.

#### 147.2 — Parser mode changes take effect within the same read

- **Source:** 131 adversarial review finding 17; GAPS "Mode changes take effect per read".
- **Defect:** `TerminalState` parses a whole PTY read (`self.parser.push(&incoming)`) and then
  applies the outputs. S8C1T/S7C1T, DECANM and RIS change the parser's own mode only when
  their output is applied, so the rest of that read was already parsed under the old mode.
- **Design (resolved):** the parser applies its own mode at the moment it emits the output
  that changes it. There is precedent: `ansi.rs:708` already sets `vt52_mode` itself on VT52
  `ESC <`. The cases are:
  - `EightBitControl` and `SevenBitControl` set `s8c1t_mode`;
  - `Mode::Decanm(Vt52 | Ansi)` sets `vt52_mode`;
  - `ResetDevice` resets both.

  `TerminalState` keeps mirroring each change to the handler. The parser no longer needs to
  be resumable.

- **Scope:**
  - `freminal-terminal-emulator/src/ansi.rs` and the sub-parsers that emit those outputs;
  - `state/internal.rs`: remove the now-redundant parser writes and keep the handler mirror;
  - tests that feed a mode change and a mode-dependent sequence in **one** read. Examples:
    `ESC SP G` then `0x9B 6 n`; `CSI ? 2 l` then a VT52 `ESC A`; S8C1T, then RIS, then
    `0x9B` (must now be text);
  - the tmux passthrough seeding (`internal.rs` seeds a fresh parser from `self.parser`).
- **Benchmark:** `parser_push`, `parser_push_sgr`, `parse_and_handle_80x24`.

#### 147.3 — S8C1T reaches the GUI encoders (foundation for 147.4)

- **Source:** 130 adversarial review finding 8.
- **Scope:**
  - add `s8c1t: S8c1t` to `TerminalSnapshot` (it already has an enum; transported state
    stays typed, per `freminal-state-representation`) and fill it in `build_snapshot`;
  - add it to `InputModes` (`freminal/src/gui/terminal/input.rs`) in
    `InputModes::from_snapshot`;
  - update snapshot equality and the idle checks if they enumerate fields.
- **Deliverable:** a test that `ESC SP G` is visible in the next snapshot and in
  `InputModes`.

#### 147.4 — Generated CSI / SS3 introducers honour S8C1T

- **Note (148.20, 2026-10-19):** 148.20 landed first by maintainer decision. The paste
  wrapper is now `wrap_bracketed_paste` in `freminal/src/gui/actions.rs`, which strips
  embedded markers before framing; 147.4 rebases its S8C1T framing onto that helper.
- **Source:** 130 adversarial review finding 8; GAPS "8-bit C1 Control Gap".
- **Design (resolved):** a rewrite of the encoded bytes is not safe, because Alt+`[` and
  Alt+`O` legitimately send `ESC [` and `ESC O`. Instead, each construction site emits its
  introducer through one helper: an `Introducer { Csi, Ss3 }` written as `ESC [` / `ESC O`
  in 7-bit mode and `0x9B` / `0x8F` in 8-bit mode. The helper lives with the input encoders
  in `freminal-terminal-emulator/src/input.rs`.
- **Scope:**
  - legacy `to_payload` and `to_payload_kkp`: arrows, Home/End, F-keys (SS3 and CSI), the
    `CSI ~` keys, `modifyOtherKeys` and kitty `CSI u`;
  - focus `CSI I` / `CSI O`, on the PTY-thread path `state/internal.rs` `send_focus_event`
    (it reads the handler's mode);
  - GUI mouse: X10/normal `CSI M`, SGR `CSI <` and SGR-pixel (`freminal/src/gui/mouse.rs`);
  - bracketed-paste markers `CSI 200~` / `CSI 201~` (`freminal/src/gui/actions.rs`), wrapping
    paste text that is never rewritten;
  - not VT52 mode, and not Alt-prefixed text.
- **Deliverable:** table tests, one row per encoder, in both modes. Existing 7-bit tests stay
  unchanged.
- **Benchmark:** the input-encoding benchmark named in `freminal-bench-table` (if none covers
  `to_payload`, add one first, per `performance-benchmarks`).
- **Depends on:** 147.3.

#### 147.5 — Application keypad (DECKPAM) sends SS3

- **Source:** found during this sweep (pre-existing, outside 126–131); related to Task 137 B11.
- **Defect:**
  - The live keypad path (Task 114 raw intercept → `drain_pending_raw_keys` →
    `legacy_keypad_byte`) never reads `keypad_app_mode`, so DECKPAM is ignored end to end.
  - The only code that does read it, `TerminalInput::KeyPad`, is never constructed outside
    tests. Its application arms also emit a wrong `ESC [ O p` form, and they match integers
    0–9 while callers pass ASCII.
- **Scope:**
  - `freminal/src/gui/terminal/input.rs`: in application keypad mode, the legacy (non-kitty)
    keypad path sends the VT220 SS3 form from the 147 decisions: digits `SS3 p`…`SS3 y`,
    `.` `SS3 n`, `-` `SS3 m`, `,` `SS3 l`, `+` `SS3 k`, `*` `SS3 j`, `/` `SS3 o`, `=` `SS3 X`,
    Enter `SS3 M`. It goes through the 147.4 introducer helper;
  - delete `TerminalInput::KeyPad` and the tests that pin its wrong output, in both
    `to_payload` and `to_payload_kkp`;
  - kitty-keyboard keypad encodings are unchanged.
- **Out of scope:** NumLock-off navigation semantics (B11's NumLock half) stay in Task 137,
  because winit exposes no lock state.
- **Depends on:** 147.4.

#### 147.6 — DECSC / DECRC save the pending-wrap flag

- **Source:** GAPS "DECSC saved state" (the remainder after 131.C1).
- **Scope:**
  - `freminal-buffer/src/buffer/cursor.rs`: `SavedCursor` records whether the cursor was in
    the pending-wrap position (`x == width`), and `restore_cursor` restores it instead of
    clamping to `width - 1`, following xterm, Ghostty and WezTerm;
  - this applies to all three save paths: DECSC, `CSI s` and `?1048`/`?1049`;
  - tests: a save at the last column with wrap pending, then a restore, then printing a
    character wraps to the next line. Also a restore after a resize that narrows the screen.

#### 147.7 — `CSI Ps u` with parameters is ignored

- **Source:** 128.5 status note ("separate behaviour decision").
- **Scope:** `ansi_components/csi_commands/scorc.rs`. Any parameter, including an explicit
  `0`, makes the sequence recognised-and-ignored with no `RestoreCursor` output. Update the
  doc comment and the tests. `CSI u` with no parameters still restores.

#### 147.8 — A bare ESC ends OSC / DCS / APC strings

- **Source:** 129.C5 residual; 147 maintainer decision.
- **Scope:**
  - `ansi_components/osc.rs` (`osc_byte_class`, the terminator path), `ansi_components/dcs.rs`
    and `ansi_components/apc.rs`. An ESC followed by anything other than `\` dispatches the
    string received so far. That ESC then starts a new escape sequence, so the following
    byte is parsed as part of it;
  - the `DCS tmux;` ESC-doubling rule is unchanged;
  - an over-cap sequence that is being consumed and dropped also ends at a bare ESC;
  - BEL and C1 ST (in S8C1T) terminators, and CAN/SUB cancellation, are unchanged;
  - tests:
    - an unterminated `OSC 2;title` followed by `CSI 31 m` sets the title and applies the
      SGR;
    - a DCS and an APC behave the same way;
    - tmux passthrough with doubled ESC still works;
    - `ESC \` still terminates.
  - Update the tests that pin the kitty behaviour (`dcs.rs:419-430`, `apc.rs:290-299` and
    their OSC counterparts).
- **Benchmark:** `parser_push` and the OSC/DCS benches.

#### 147.9 — XTSMGRAPHICS, plus XTSMTITLE / XTRMTITLE recognised

- **Source:** 128.C4 (both are named there as unrouted CSI that now warns); untracked in
  COVERAGE and GAPS.
- **XTSMGRAPHICS `CSI ? Pi ; Pa ; Pv S`** follows xterm with ReGIS absent
  (`charproc.c:5160-5286`, `ctlseqs.txt:672-696`). The reply is `CSI ? Pi ; Ps [; Pv…] S`,
  framed by the handler (S8C1T-aware).
  - A parameter count other than 3 gets no reply.
  - **Pi=1 (colour registers):**
    - Pa=1 (read), Pa=2 (reset) and Pa=4 (read max) answer status 0 with `MAX_PALETTE`
      (256);
    - Pa=3 (set) answers status 3, because the register count is fixed;
    - any other Pa answers status 2.
  - **Pi=2 (sixel geometry):**
    - Pa=1 answers status 0 with the current text-area pixel size (clamped to the decoder's
      maximum, if it has one);
    - Pa=4 answers status 0 with the maximum geometry;
    - Pa=2 and Pa=3 answer status 3, as in xterm;
    - any other Pa answers status 2.
  - **Pi=3 (ReGIS)** answers status 1, like any other unknown Pi (corrected 2026-11-20: xterm
    built without ReGIS reaches its `default:` arm; this line said 3).
- **`CSI > Pm t` and `CSI > Pm T`** are routed to recognised-and-ignored outputs with no
  `warn!`.
- **Scope:** `ansi_components/csi_dispatch.rs` (the `question` and `greater` groups), a new
  `csi_commands/xtsmgraphics.rs`, the handler reply in `terminal_handler/reports.rs`, and
  tests through the full stack.

#### 147.10 — Layout startup commands go through the input channel

- **Source:** 130 decision note on `Pane::pty_write_tx`.
- **Scope:**
  - `freminal/src/gui/layout_ops.rs` sends startup commands as `InputEvent::Key(bytes)` on the
    pane's input sender, so FREC records them and they are ordered with all other input;
  - if `layout_ops` is the last production user, remove `Pane::pty_write_tx` and the
    `pty.rs` clone that feeds it, together with their test and bench constructors;
  - a test that a layout startup command arrives as an `InputEvent` and is recorded.
- **Stop condition:** if `pty_write_tx` turns out to have another production user, stop and
  report. Do not route around it.

#### 147.11 — The host-capability broadcast gets a home and a test

- **Source:** 130 orchestrator re-review (130.4 untested).
- **Defect:** the diff-and-broadcast step in `freminal/src/gui/settings_dispatch.rs`
  (`apply_new_config` → `InputEvent::HostCapabilitiesChange` to every pane) has no test. A
  wrong comparison would leave every pane's OSC 99 support stale after a settings change.
- **Design (resolved):** decide whether a config change produces a capability change, and
  which events to send, in one function in `freminal/src/gui/host_capabilities.rs`.
  `settings_dispatch.rs` calls it. Per `freminal-extend-or-extract`, this gives the concept a
  home; it is not a helper extracted only for testing.
- **Deliverable:** unit tests for the cases below, plus a test over real
  `crossbeam` senders standing in for panes:
  - unchanged config: no events;
  - changed routing: one event per pane;
  - platform-only fields: no events.

#### 147.12 — The first alternate-screen entry builds one store

- **Source:** 131.5 status note; 131 adversarial review finding 14.
- **Scope:** `freminal-buffer/src/buffer/resize_and_alt.rs`. The first `switch_to_alternate`
  followed by `clear_alternate_screen` builds one blank store, not two. Row numbering must
  stay monotonic. `debug_assert_parked_screens` and the namespace invariants keep passing.
- **Deliverable:** a test that a first `?1049h` allocates one store, and that the alternate
  `next_number` follows the parked store.
- **Benchmark:** `bench_alternate_screen_switch` (the corrected 131.5 shape is the baseline).

#### 147.13 — Graphics transfer hygiene

- **Source:** 129 adversarial review, "Accepted as is".
- **Scope:**
  - `terminal_handler/chunk_assembler.rs`: growth never reserves past the total cap. Today
    `Vec` doubling can transiently reserve about twice the 400 MiB kitty cap. Add a test with
    a small-cap assembler that checks capacity never exceeds the cap;
  - `terminal_handler/graphics_kitty.rs`: the unknown-id `a=p` log drops to `debug!`, because
    the client already gets `ENOENT` on the wire. A client probing every frame then no longer
    fills the logs.

#### 147.14 — Windows `t=t` deletion goes by handle

- **Source:** 126.2 (the unix `unlinkat` and `(dev, ino)` binding has no Windows equivalent).
- **Scope:**
  - `terminal_handler/graphics_kitty.rs` `#[cfg(windows)]` path: open the temp file with
    `FILE_FLAG_DELETE_ON_CLOSE` (`std::os::windows::fs::OpenOptionsExt::custom_flags`) and a
    share mode that includes delete;
  - check that the handle is a regular file;
  - read through that handle and drop it, so the file deleted is the file read. No new
    dependency.
- **Verification:** `cargo xtask check-windows`; the Windows CI test job runs the existing
  `t=t` tests.

#### 147.15 — The CSI parser reuses its buffers

- **Source:** 128.P1 remainder (maintainer decision 2026-10-11).
- **Scope:** `ansi_components/csi.rs`. Its three per-sequence `Vec`s become parser-owned
  buffers, cleared and reused for each sequence. Outputs that need owned data still copy
  out, and the public API is unchanged. The `sub_parsers_stay_small` size bound is revisited
  only if the buffers must live in `ParserInner`; record the decision.
- **Benchmark:** `parser_push_sgr`, `parser_push`, `parse_and_handle_80x24`. The status note
  records the before/after table.
- **Scheduling:** after 147.2 and 147.8 (same files).

#### 147.16 — `cargo xtask lint-markdown` passes (126.C1)

- **Source:** 126.C1.
- **Scope:**
  - `xtask/src/main.rs`: the glob excludes `.direnv` (and `target`), and the "known issue"
    swallow at `main.rs:398` is removed, so the command is a real gate;
  - fix the remaining lint debt (about 550 errors, mostly MD060 and MD033) in the
    `Documents/` files it reports;
  - run `prettier --write` on each reported file **by name** (never a glob), then fix by
    hand whatever prettier does not;
  - inline HTML that is load-bearing gets a scoped rule in `.markdownlint-cli2.yaml` with a
    comment, not a blanket disable.
- **Deliverable:** `cargo xtask lint-markdown` exits 0.

#### 147.18 — The saved cursor follows its text through a resize

- **Source:** GAPS "Saved cursor on resize, alternate active" (pre-v0.13.0, from Task 125;
  added to 147 by maintainer decision 2026-10-11).
- **Defect:** the DECSC slot stores a screen position, and a resize never moves it. When the
  resize reflows or pulls rows back from scrollback, a later DECRC puts the cursor on a
  different line of text from the one it was saved on. The classic case: `?1049h`, resize
  the window, `?1049l`, and the shell prompt is redrawn in the wrong place.
- **Reference behaviour:**
  - Ghostty pins every screen's saved cursor to its cell for the resize and reads it back
    afterwards (`Screen.zig` `resize`, `saved_cursor_pin`). If the pinned cell left the
    active area, the cursor goes to the top-left. If pending wrap was set and the cursor is
    no longer in the last column, pending wrap is cleared and x advances by one.
  - xterm does not reflow, but shifts the main screen's saved row by the number of lines
    pulled from scrollback while the alternate screen is up (`cursor.c:380-392`
    `AdjustSavedCursor`, called from `screen.c:2435`).
  - Freminal reflows, so it follows Ghostty, which is the general form of the xterm rule.
- **Scope:**
  - `freminal-buffer/src/buffer/resize_and_alt.rs` and `cursor.rs`: the live screen's slot
    and a parked screen's slot both track their cell through `set_size` / reflow. Use the
    same row-number/offset remap that already carries `reflow_anchor` and the cursor
    (`pending_reflow_remap`), not a recomputed screen offset;
  - Ghostty's fallbacks apply: the cell left the screen, or the pending-wrap fix-up;
  - tests:
    - a primary-screen save, then a narrowing reflow, then DECRC lands on the same
      character;
    - `?1049h`, then a resize that grows the height and pulls rows from scrollback, then
      `?1049l`: the cursor is on the saved text (the xterm case);
    - the saved cell scrolled off the screen;
    - pending wrap across a widening resize.
- **Depends on:** 147.6 (pending wrap is part of the saved state) and 147.12 (same file).

#### 147.19 — Sixel under DECSDM draws at the screen origin and never scrolls

- **Source:** GAPS "Sixel placement under DECSDM" (pre-v0.13.0, from Task 125; added to 147
  by maintainer decision 2026-10-11).
- **Defect:** with sixel display mode set (`?80 h`, sixel scrolling off), freminal places the
  image at the cursor, lets `place_image` scroll the screen, and then puts the cursor back.
  xterm places the image at the top-left of the screen (`graphics_sixel.c:395-405`,
  `get_new_graphic(xw, 0, 0, …)`). It never scrolls; the scroll at `:853-866` happens only
  when sixel scrolling is on, and rows past the bottom are discarded. The text cursor does
  not move.
- **Scope:**
  - `freminal-buffer`: `place_image` gets an explicit placement mode. Use a named enum (for
    example an inline mode against a fixed-origin mode with no scrolling), never a bool, per
    `freminal-state-representation`. In fixed-origin mode the image is anchored at screen
    row 0, column 0, clipped to the visible rows, and neither the cursor nor the scroll
    position changes;
  - `terminal_handler/graphics_sixel.rs` uses that mode when `Decsdm::DisplayMode` is set.
    `restore_cursor_to_image_origin` is deleted if this was its last caller;
  - the existing DECSDM tests (`sixel_display_mode_cursor_does_not_advance`,
    `sixel_display_mode_at_scrollback_capacity_keeps_cursor_on_image_origin`) are rewritten
    to the xterm behaviour;
  - new tests: an image taller than the screen is clipped with no scrollback growth, and the
    cursor is unmoved in the middle of the screen.
- **Damage:** placement damage stays bounded to the image's rows (`freminal-damage-model`).

#### 147.20 — Character sets: G0–G3, locking and single shifts

- **Source:** GAPS "Charset / G-Set Gaps" (SO/SI, LS2/LS3, `ESC ) C` / `ESC * C`). These
  are pre-v0.13.0; the maintainer added them to 147 on 2026-10-11.
- **Defects today:**
  - SO and SI are dropped in `ansi.rs`.
  - `ESC ) 0` (G1) and `ESC * C` (G2) are recognised but change nothing. `ESC ( A` (UK) is
    rejected as invalid.
  - `ESC + 0` (G3) switches on the single global line-drawing flag. That is wrong: it only
    designates G3. The wrong behaviour is pinned by
    `esc_plus_0_designates_g3_as_dec_special_graphics`.
  - LS2, LS3, LS1R, LS2R and LS3R are logged and ignored.
  - 7-bit SS2/SS3 (`ESC N` / `ESC O`) parse as `Invalid`, and 8-bit `0x8E`/`0x8F` are ignored.
  - So an application that draws boxes with `ESC ) 0`, then SO, prints `lqqqk` instead of
    box corners and lines.
- **Decisions (maintainer, 2026-10-11, from the 147.20 reference read):**
  - **The full G0–G3 model with GL, GR and a pending single shift**, as xterm
    (`charproc.c:3774-3844,5481-5493,6174-6207`) and Ghostty (`Screen.zig` `CharsetState`;
    `stream.zig:1440-1441,2850-2917,3004-3123`) have. kitty and WezTerm stop at G0/G1 with
    SO/SI.
  - **Sets:** ASCII (`B`), UK (`A`) and DEC Special Graphics (`0`), in all four slots.
    - UK maps `#` to `£` unconditionally, as kitty, Ghostty and WezTerm do. xterm alone
      requires NRC mode.
    - The 96-character designators (`ESC - . /`) are not planned. No modern reference
      implements them: Ghostty has a TODO, and kitty and WezTerm lack them.
  - **An unknown designator final is ignored** and the slot keeps its set, as kitty, Ghostty
    and WezTerm do. xterm alone resets the slot to ASCII.
  - **GR is tracked and saved but never applied to bytes 0x80 and above.** Those bytes are
    UTF-8, and Ghostty and xterm in UTF-8 mode don't apply GR either
    (`Terminal.zig:1660`, `ptydata.c:94`).
  - **A single shift applies to the next printable character only and is then cleared.** A
    non-ASCII character consumes it and prints unmapped, as in Ghostty.
  - **DECSC saves G0–G3, GL, GR and the pending single shift.** This follows the VT420 and
    VT520 manuals (quoted in xterm `cursor.c:425-440`) and Ghostty. DECRC restores them, or
    the defaults if nothing was saved.
  - **RIS and DECSTR reset all of it to the defaults:** G0–G3 ASCII, GL=G0, GR=G2, no single
    shift. That is what xterm's `resetCharsets`, Ghostty and kitty do. RIS clears both DECSC
    slots, and DECSTR sets the active one, as today.
  - **VT52:** `ESC F` / `ESC G` select line drawing / ASCII as today, expressed through the
    new model. Entering VT52 saves the ANSI charset state and leaving restores it, as xterm
    does (`charproc.c:7546-7550,5055-5064`).
- **Scope:**
  - A new `freminal-common/src/buffer_states/charset.rs` holding `CharacterSet`,
    `CharsetSlot` (G0–G3) and `CharsetState { sets, gl, gr, single_shift }`. These are named
    enums, not bools. It supersedes `line_draw.rs`'s `DecSpecialGraphics`. Per
    `freminal-module-cohesion`, the line-drawing table moves with it.
  - `terminal_output.rs`: `DesignateCharset { slot, set }`, `InvokeGl(slot)`,
    `InvokeGr(slot)` and `SingleShift(slot)` replace `DecSpecialGraphics(..)` and the
    roughly 27 `Charset*` placeholder variants.
  - `ansi.rs`:
    - SO and SI emit `InvokeGl` after the existing data flush;
    - 8-bit SS2/SS3 emit `SingleShift` when S8C1T is active;
    - VT52 `ESC F` / `ESC G` move to the new model.
  - `ansi_components/standard.rs`:
    - one designator helper for `( ) * +`;
    - `N` and `O` become SS2/SS3;
    - `n o ~ } |` become the locking shifts.
  - `terminal_handler/mod.rs`:
    - a `charset: CharsetState` field replaces `character_replace`;
    - `handle_data` keeps its zero-copy fast path (GL is ASCII and no shift is pending) and
      otherwise uses a table-driven map.
  - `terminal_handler/cursor_ops.rs` (`SavedDecscState`) and `terminal_handler/reset.rs`.
    The reset table classifies the new field.
  - Tests:
    - designation and invocation for every slot, SO/SI box drawing, LS2/LS3, GR stored but
      not applied;
    - SS2/SS3 (7-bit, and 8-bit under S8C1T) affecting exactly one character;
    - DECSC/DECRC round trip including the single shift;
    - RIS and DECSTR;
    - VT52 save and restore;
    - an unknown final leaving the slot unchanged.
  - Rewrite `tests/vttest_charsets.rs` and the placeholder-pinning tests in `standard.rs` and
    `ansi.rs`.
- **Benchmark:** `parser_push` and `parse_and_handle_80x24`. The plain-ASCII fast path must
  not regress.
- **Scheduling:** after 147.8 (shared parser files), before 147.15.

#### 147.21 — KAM, SRM, `?1001`, `?1034` and `?1035` recognised and not implemented

- **Source:** GAPS "CSI Standard Mode Gaps" (KAM, SRM) and "DEC Private Mode Gaps"
  (`?1001`, `?1034`). These are pre-v0.13.0; the maintainer decided on 2026-10-11.
- **Decisions (from the 147.21 reference read):**
  - **KAM (ANSI mode 2, keyboard lock)** is recognised and not implemented. kitty and
    WezTerm don't implement it. Ghostty only enforces it behind `vt-kam-allowed`, which is
    off by default, because letting any program lock the keyboard is hostile. xterm alone
    enforces it.
  - **SRM (ANSI mode 12, local echo)** is recognised and not implemented. kitty and WezTerm
    don't implement it, and Ghostty stores it with no consumer. On a modern tty the kernel
    echoes, so turning local echo on only doubles every character.
  - **`?1001` (hilite mouse tracking)** is recognised and not implemented. kitty, Ghostty and
    WezTerm don't implement it; WezTerm's docs warn that it can hang the terminal.
    - freminal currently aliases it to `?1000`: it sends normal button reports the
      application never asked for, and claims support in DECRQM. The alias is removed.
  - **`?1034` (Meta sets the eighth bit)** is recognised and not implemented. Ghostty,
    WezTerm and kitty don't implement it, and it conflicts with UTF-8 input.
  - **`?1035` (NumLock overrides the application keypad)** is recognised and not
    implemented. winit exposes no NumLock state (Task 114.11), so the mode cannot be
    honoured. If lock state becomes available, it belongs with Task 137 K14.
  - **"Recognised and not implemented" means:**
    - set and reset are typed modes that log at `debug!`;
    - DECRQM answers `Ps = 0` (not recognised), as kitty, Ghostty and WezTerm do for modes
      they lack. KAM and SRM use the ANSI form without `?` (147.1);
    - nothing else changes.
- **Scope:**
  - `freminal-common/src/buffer_states/mode.rs` and `modes/`: typed variants for ANSI 2 and
    12 and DEC `?1034` and `?1035`. `?1001` leaves `MouseTrack` (remove `XtMseHilite`) and becomes its
    own not-implemented mode;
  - `terminal_handler/mod.rs` and `state/internal.rs` (DECRQM, and `sync_mode` lists);
  - `freminal/src/gui/mouse.rs` (remove the `XtMseHilite` arms);
  - tests: set, reset and DECRQM for each mode. `?1001h` followed by a click sends no mouse
    report.
- **Depends on:** 147.1 (the ANSI DECRQM form; same file).

#### 147.22 — XTGETTCAP answers `indn`, `rin` and `query-os-name` (moved to Task 151 as 151.2) <!-- codespell:ignore rin -->

- **Source:** GAPS "XTGETTCAP capability expansion" (pre-v0.13.0; added to 147 by maintainer
  decision 2026-10-11). The GAPS row calls `indn` "indent N"; it is "scroll forward N lines".
- **Reference behaviour:**
  - kitty (`terminfo.py:183,211,403,425`), Ghostty (`ghostty.zig:203-213`) and WezTerm (its
    terminfo database via `xt_get_tcap`) all answer `indn` (`CSI %p1%d S`) and `rin` <!-- codespell:ignore rin -->
    (`CSI %p1%d T`), and kitty also answers the termcap aliases `SF` and `SR`. xterm answers
    only its key table.
  - `query-os-name` (alias `query-os_name`) is answered by kitty (`terminfo.py:555-557`) and
    foot with `uname -s` (`Linux`, `Darwin`, `FreeBSD`, ...). kitty issue #9217 is where it
    was proposed as a terminal-agnostic query. Ghostty and WezTerm don't answer it.
- **Scope:** `terminal_handler/dcs.rs` `lookup_termcap`:
  - `indn`/`SF` → `\x1b[%p1%dS` and `rin`/`SR` → `\x1b[%p1%dT`, using the table's <!-- codespell:ignore rin -->
    existing raw-ESC convention (as `Smulx` does). Both are truthful: SU and SD are
    implemented and respect the scroll region;
  - `query-os-name` / `query-os_name` → a `cfg!(target_os)` mapping to `uname -s` names:
    linux `Linux`, macos `Darwin`, freebsd `FreeBSD`, openbsd `OpenBSD`, netbsd `NetBSD`,
    windows `Windows`, otherwise `Unknown`. No new dependency;
  - not `kitty-query-os_name`, which is kitty's own namespace with different values;
  - tests: each name and alias, a mixed multi-name request, and the reply hex.
  - `rin` trips codespell; mark its lines with `// codespell:ignore rin` (Markdown: the
    equivalent HTML comment).

#### 147.23 — HP `ESC F`, `ESC l`, `ESC m` recognised and not implemented

- **Source:** GAPS "ESC Gaps" (pre-v0.13.0; added to 147 by maintainer decision 2026-10-11).
- **Reference behaviour:**
  - xterm: `ESC F` moves to the lower-left corner **only** if the `hpLowerleftBugCompat`
    resource is set, and it defaults to off (`charproc.c:448,3982-3990`). `ESC l` / `ESC m`
    set or clear the top margin at the cursor row (`charproc.c:4759-4770`). These are HP
    compatibility hacks.
  - kitty, Ghostty and WezTerm ignore all three.
- **Decision (majority):** recognised and not implemented.
- **Scope:** `terminal_handler/mod.rs`. The three arms log at `debug!` as intentionally
  ignored HP compatibility sequences, instead of the `warn!` "not yet implemented". A test
  checks there is no state change. VT52 `ESC F` (graphics mode) stays a separate, working
  path.

#### 147.24 — Alt sends ESC (`?1036` / `?1039`) (macOS half moved to Task 152)

**Amended 2026-11-20:** by maintainer decision (2026-11-19, option 1) the macOS Option-as-Alt
half of this subtask (the `[keyboard] macos_option_as_alt` option and its per-side choices)
moved to Task 152; macOS Option keeps composing. The rest stands as written.

- **Source:** maintainer decision 2026-10-11. This takes the legacy text half of Task 137 B1
  ("Alt and Super never apply to text keys").
- **Defect today:** Alt+letter sends a plain letter. egui delivers `Event::Key { B, alt }`,
  which matches no arm and is dropped, and `Event::Text("b")`, which is sent as `b`. So
  readline/emacs bindings like Alt+b, Alt+f and Alt+d don't work. No Alt combination
  produces an ESC prefix anywhere.
- **Reference behaviour:**
  - Ghostty: `?1036` (`alt_esc_prefix`, default **set**) gates the ESC prefix on Alt+text.
    When it is reset, the plain character is sent (`key_encode.zig:540,555-583`). `?1039` is
    stored and reported but not consumed.
  - xterm: `metaSendsEscape` / `altSendsEscape` (`input.c:1470-1535`). Without eight-bit
    meta, a reset mode sends the plain character.
  - kitty: legacy Alt always prefixes ESC (`key_encoding.c:141,149,343-348`).
  - macOS Option composes characters unless an option-as-alt setting says otherwise:
    kitty `macos_option_as_alt`; Ghostty `macos-option-as-alt`.
- **Decisions:**
  - Follow Ghostty. `?1036` defaults to set; while set, legacy Alt+text sends `ESC`
    followed by the text, and while reset it sends the text alone. `?1039` is a typed,
    stored mode that DECRQM reports truthfully and that has no further effect. Neither mode
    is part of RIS or DECSTR state beyond returning to its default; classify both in the
    reset table.
  - DECRQM reports `?1036` and `?1039` as set or reset (`Ps` 1/2).
  - **Only the legacy path changes.** When any kitty keyboard flag is active, Alt+text
    behaves as today; Task 137 owns its `CSI u` encoding.
  - A new `[keyboard] macos_option_as_alt` config option: a named enum `none` (default),
    `left`, `right`, `both`. While it is `none`, macOS Option keeps composing and is not
    treated as Alt. It is wired per `freminal-config-options`, with the full `ConfigPartial`
    / `apply_partial` checklist, `config_example.toml`, the settings UI, and the Nix module
    if it mirrors the schema.
  - **Stop condition for `left`/`right`:** they need to know which Option key is held. If
    winit's per-side modifier state cannot reach the encoder through the existing Task 114
    raw-key path without a new channel, implement `none`/`both` only, and stop and report
    before adding a channel (`freminal-architecture`).
  - AltGr must never be prefixed. On Linux AltGr is not Alt in winit. On Windows AltGr
    arrives as Ctrl+Alt; tests must pin that AltGr text is sent unprefixed.
- **Scope:**
  - `freminal-common`: the `?1036`/`?1039` mode types (`modes/`), parsing in `mode.rs`, and
    the config enum and section;
  - `freminal-terminal-emulator`: `TerminalModes` fields, `sync_mode`, DECRQM, the snapshot
    field (typed), and the `TerminalInput` text variant gains its Alt prefix through the
    147.4 encoder (built at the construction site, never by rewriting encoded bytes);
  - `freminal/src/gui/terminal/input.rs`: when Alt (per the Option setting on macOS) is held,
    `Event::Text` emits the prefixed form, and the stray `Event::Key { .., alt }` stays
    dropped so nothing is sent twice;
  - tests:
    - Alt+b sends `ESC b` with `?1036` set and `b` with it reset;
    - a multi-byte Alt+text gets one prefix;
    - kitty flags active means unchanged;
    - AltGr text is unprefixed;
    - the macOS Option settings, through the input-mapping unit tests;
    - DECRQM for both modes;
    - the config round trip.
- **Out of scope (stays in Task 137):** modifiers on Enter, Tab, Backspace and Escape (B2,
  B3), Super/Hyper/Meta, and kitty-protocol Alt encoding.
- **Depends on:** 147.3 and 147.4 (snapshot input-mode transport and the encoder helpers),
  and 147.21 (same mode files).

#### 147.26 — `--features gl-pixel` dead-code warnings in `gl_facade`

- **Source:** Task 132 cleanup entry 132.C3 (surfaced running the pixel goldens in
  `.#gl-pixel`). Predates 132.
- **Defect today:** building `freminal` with `--features gl-pixel` alone warns that several
  `Gl` facade methods, `RecordingState::next_query` and four `RecordingState` query methods
  are never used. Clippy with `--all-features` hides it because `frame-profiling` uses them.
- **Scope:** `freminal/src/gui/renderer/gl_facade/facade.rs`, `recording.rs`.
- **Fix:** gate the query items on the feature(s) that use them; no `allow(dead_code)`.
- **Tests / verification:** `cargo build -p freminal --features gl-pixel --all-targets` and
  `--features frame-profiling` are both warning-free; add the `gl-pixel`-only combination to
  a lint gate so it cannot regress.
- **Depends on:** nothing.

#### 147.27 — DECSCNM leaves the pane's sub-cell remainder in normal video

- **Source:** Task 132 adversarial review, finding R11. Predates 132.
- **Defect today:** with DECSCNM (`?5h`) set, cells are drawn inverted, but the strip of the
  pane's `terminal_rect` right of and below the last cell (less than one cell wide) shows the
  normal default background. xterm inverts the whole window area.
- **Constraint found in review:** the obvious fix (a base fill of the resolved foreground while
  reversed, reusing Task 132.11) is wrong on its own: a cell with SGR 7 under DECSCNM has an
  effective background of `DefaultBackground`, which the renderer skips, so it would show the
  fill (the foreground colour) instead of the default background. The skip rule must become
  "skip only when the effective background equals what lies beneath".
- **Scope:** `freminal/src/gui/renderer/{gpu,vertex}.rs` (`PaneBackground::base_fill_for`,
  the `DefaultBackground` skip), `freminal/src/gui/terminal/widget.rs`.
- **Tests:** pixel tests for the remainder strip under DECSCNM, and for SGR 7 + DECSCNM
  (double reversal shows the default background), via the `.#gl-pixel` harness.
- **Depends on:** nothing.

#### 147.28 — Nix module lacks `[notifications] command_finished_template`

- **Source:** Task 138.11 (sub-agent report, confirmed by the orchestrator). Predates 138.
- **Defect today:** `NotificationsConfig::command_finished_template`
  (`freminal-common/src/config.rs`) and `config_example.toml` both have the key, but
  `nix/home-manager-module.nix` has no option for it. Managed installs cannot set it, against
  `freminal-config-options` step 5.
- **Scope:** `nix/home-manager-module.nix` only: the option, plus its entry in the
  notifications section binding.
- **Tests:** `nixfmt --check`, `statix check`, `deadnix --fail`; evaluate a module config that
  sets the key and check it reaches the generated TOML.
- **Depends on:** nothing.

#### 147.29 — `resolve_anchor` takes a bool parameter

- **Source:** Task 138.C14 (found while editing the toast layout). Predates 138.
- **Defect today:** `freminal/src/gui/toast/layout.rs`
  `resolve_anchor(.., is_origin_window: bool)` violates `freminal-state-representation` (no
  bool parameters).
- **Scope:** `toast/layout.rs` and its tests: a two-variant named enum
  (`ToastOriginWindow { This, Other }`).
- **Tests:** the existing anchor tests, ported.
- **Depends on:** nothing.

#### 147.30 — Fuzz sandbox symlink test depends on the checkout location

- **Source:** issue #530 per-commit verification (found running `cargo xtask test` in a
  worktree under `/tmp/opencode/`). Predates #530; reproduces on `main`.
- **Defect today:** `xtask/src/fuzz/sandbox.rs`
  `a_symlink_into_tmp_is_rejected_under_bwrap` creates its link directory with
  `tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))` and asserts
  `!hidden_by_private_tmpfs(&link)`. When the checkout itself lives under `/tmp`, the link is
  under `/tmp` too, so the assertion fails and `cargo xtask test` stops there. CI and normal
  checkouts are unaffected, but the test is not hermetic with respect to where the repository
  sits.
- **Scope:** `xtask/src/fuzz/sandbox.rs` (that test only).
- **Fix:** create the link directory somewhere guaranteed not to be under `/tmp` or
  `/dev/shm` (or skip the "link is not itself hidden" premise with an explicit, asserted
  precondition), so the test checks the resolved-target rule regardless of checkout location.
- **Tests / verification:** the test passes from a checkout under `/tmp` and from the normal
  checkout.
- **Depends on:** nothing.

#### 147.31 — `cargo doc -p freminal` emits 68 rustdoc warnings

- **Source:** issue #530 review (found checking that the branch added no rustdoc warnings; the
  three it did add were fixed there). Predates #530; reproduces on `main`.
- **Defect today:** `cargo doc -p freminal --no-deps` warns 68 times (58 when found; the Task
  148 merge added 10): public docs linking to private items (`private_intra_doc_links`, e.g.
  `box_drawing.rs`, `shaping.rs` `RunCache`,
  `toast_pass.rs`, `settings.rs` `VisualPreview`) and unresolved links
  (`broken_intra_doc_links`, e.g. `renderer/mod.rs` `profiling::*`, `chrome_style.rs`
  `Visuals`, `panes/mod.rs` `PaneTreeSnapshot`). No CI job runs rustdoc, so they accumulate.
- **Scope:** doc comments across `freminal/src/` only; no code changes.
- **Fix:** resolve each link (correct path, or plain backticks where the target is private),
  then add `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS="-D warnings"` to
  `cargo xtask ci` (and the CI workflow) so it cannot regress.
- **Tests / verification:** `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` is
  clean.
- **Depends on:** nothing.

#### 147.32 — Unused `RowStore` import in a `freminal-buffer` test module

- **Source:** Task 149 final verification (2026-10-22). Predates Task 149 (the file is
  unchanged on `task-149/zbus-6`); reproduces on `main`.
- **Defect today:** `cargo bench --no-run --all` warns `unused import: RowStore` at
  `freminal-buffer/src/buffer/screen_switch_tests.rs:26`. Clippy with `-D warnings` does not
  see it, because the import is only unused in the bench-profile test build.
- **Scope:** that one import (or its `cfg`), nothing else.
- **Fix:** drop `RowStore` from the `use`, or gate it to the configuration that uses it.
- **Tests / verification:** `cargo bench --no-run --all` emits no warnings.
- **Depends on:** nothing.

#### 147.33 — Plain-text status cells in the coverage tables

- **Source:** CodeRabbit on PR #587 (Task 140), with the maintainer's decision (2026-11-19) to
  convert the whole files in one pass rather than two rows.
- **Defect today:** `markdown-lint-discipline` forbids emoji in table cells (their display width
  is ambiguous, so linters and prettier disagree about alignment), but the status columns of
  `ESCAPE_SEQUENCE_COVERAGE.md` (about 196 cells, with a ✅/🚧/❌ legend), `SGR.md` (about 29)
  and the gap tables in `ESCAPE_SEQUENCE_GAPS.md` use them throughout.
- **Scope:** those three files only: their status cells and legends become plain text (for
  example `done`, `partial`, `missing`, `not planned`), with the legend rewritten to match;
  tables re-padded with `prettier --write` by file name. No wording changes beyond the status
  values. Update the `freminal-escape-sequence-docs` skill if it names the emoji markers.
- **Tests / verification:** markdownlint and `prettier --check` on the three files; a grep for
  the emoji markers in their tables is empty.
- **Depends on:** nothing.

#### 147.17 — Documentation

- **Scope:**
  - **COVERAGE and GAPS:**
    - every behaviour change above;
    - remove the "Mode changes take effect per read" paragraph and the event-encoding and
      UTF-8-title sentences from the 8-bit section. The UTF-8 title behaviour is closed as
      matching xterm, with the citations;
    - close the DECRC/DECOM row as matching kitty, Ghostty and WezTerm, and correct its
      description of xterm (bottom/right clamp only);
    - fix the malformed `ESC \` row (GAPS around line 392);
    - the charset section for 147.20, including the 96-character designators as not
      planned;
    - KAM, SRM, `?1001`, `?1034` and `?1035` as not planned, with the reasons (147.21).
      Correct the `?1034` row, which describes `?1036`;
    - `?1036`/`?1039` rows (147.24), XTGETTCAP `indn`/`rin`/`query-os-name` (147.22), and HP <!-- codespell:ignore rin -->
      `ESC F`/`l`/`m` as not planned (147.23);
    - `config_example.toml` documentation for `[keyboard]` is part of 147.24, not this
      subtask;
    - add XTSMGRAPHICS and XTSMTITLE/XTRMTITLE rows;
    - update the DECKPAM and `CSI Ps u` rows;
  - **`KITTY_PROTOCOL_REFERENCE.md`:** the string-terminator change;
  - refresh both "Last updated" headers;
  - `prettier` and `markdownlint` on the named files only.

### 147 Routed to existing stubs (added 2026-10-11)

These are owned by tasks that rewrite the code involved. Each is now written into that
task's stub:

- **Task 138:** a saturated 128-id OSC 99 map refuses even complete single-chunk
  notifications. Tombstoned ids are unbounded strings. (129 review; previously routed, but
  missing from the stub.) Closed by 138.3 (LRU, never refuses) and 138.2 (ids capped at 512
  bytes).
- **Task 138:** the transported OSC 99 `bool`s (130.C4). Already present.
- **Task 135:** cap `o=z` inflation at the size implied by `s`/`v`/`f`, not only at
  `MAX_KITTY_DATA_BYTES`. (129 review; previously routed, but missing from the stub.)
- **Task 136:** image placement ignores DECSTBM (131 review finding 3, 131.C5). Already
  present.
- **Task 139:** RIS and DECSTR clear the pointer-shape stack. (131 decision; missing from the
  stub.)
- **Task 137:** B11's DECKPAM half moves to 147.5; the NumLock half stays.

Stale stub text was corrected at the same time:

- Task 141: the tmux ED line (the tmux CSI dispatch was deleted by 130.2).
- Task 137: the "DECSTR clearing the flags" question (done by 131.3).

### 147 Reviewed, not defects

| Item                                         | Why it is not a defect                                                                  |
| -------------------------------------------- | --------------------------------------------------------------------------------------- |
| Parked alternate keeps its images (131 r4)   | The decided persistence; kitty's alternate graphics manager does the same.              |
| Selection cleared on a switch (131 r5)       | By design: alternate rows are a different row namespace (Task 125).                     |
| `ResetKind` / `KittyKeyboardStack` are `pub` | Private modules; `clippy::redundant_pub_crate` (denied) rejects `pub(crate)`.           |
| Marks on popped padding rows (131 r18)       | Unreachable: marks sit on the cursor row, which a padding pop never removes.            |
| Prettier reflows COVERAGE tables (131 r19)   | Formatter behaviour, not a code defect.                                                 |
| `?1049l` on the primary restores the cursor  | Matches xterm (`charproc.c:7736-7741`); 2–2 split resolved by the xterm + Ghostty rule. |
| XTGETTCAP answers per name (130 N6)          | Matches kitty.                                                                          |
| `is_control_payload -> bool`                 | A predicate returning `bool`, not a bool field or parameter.                            |
| 129 small-limit assembler tests              | The real caps are pinned by value tests; a 400 MiB push in a unit test adds nothing.    |
| `is_bare_continuation` leniency (129)        | Matches kitty's leniency for clients that repeat `i=`/`f=` on every chunk.              |
| `decode_strict` / `encode_unpadded` unused   | Their consumers are Tasks 102 and 146 (both stubs name them).                           |
| 131.5 benchmark baseline                     | Not a defect: the corrected figures are recorded and are 147.12's baseline.             |
| 127.C1 first-frame visual                    | Checked by the maintainer at the 147 PR (see verification).                             |

### 147 Execution model

- 147.3 → 147.4 → 147.5 run in order (foundation, encoders, keypad).
- 147.2 → 147.8 → 147.20 → 147.15 run in order (shared parser files).
- 147.1 → 147.21 run in order (`mode.rs`).
- 147.24 runs after 147.4 and 147.21.
- 147.22 and 147.23 are independent.
- 147.1, 147.6, 147.7, 147.9, 147.10, 147.11, 147.12, 147.13, 147.14 and 147.16 are
  independent of each other and of the two chains. Run them in any order, or in parallel
  worktrees per `parallel-work-isolation`.
- 147.6 → 147.12 → 147.18 run in order (the saved-cursor state, then the same buffer file).
- 147.19, 147.26, 147.27, 147.28, 147.29, 147.30, 147.31 and 147.33 are independent.
- 147.17 runs last.
- Every subtask goes through the orchestrator's full-diff review. Each behaviour fix needs a
  test that fails on the old code. The task ends with an adversarial review and a
  confirmation review, as for 130 and 131.

### 147 Verification

- `cargo xtask test`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo fmt --all -- --check`, `cargo machete`, `cargo bench --no-run --all`,
  `cargo xtask check-windows`, `cargo xtask lint-markdown`.
- Benchmarks for 147.2, 147.4, 147.8, 147.12, 147.15, 147.18 and 147.20 stay within the 15% threshold, or
  improve.
- Manual check by the maintainer at the PR: a freshly opened, unfocused window draws the
  unfocused cursor style on its first frame (127.C1).

### 147 Cleanup entries

#### 147.C1 — Mouse reports are re-encoded as keys under kitty flag 8

- **Surfaced:** 147.4 (2026-11-19). Predates Task 147.
- **Impact:** GUI mouse reports are built as `TerminalInput::Ascii` bytes and sent through
  `send_terminal_inputs`, which encodes them with the key encoder. With kitty keyboard flag 8
  (report all keys as escape codes) set, every byte of a report becomes its own `CSI u`, so
  an application using flag 8 and mouse tracking gets garbage instead of mouse reports.
- **Scope of fix:** `freminal/src/gui/mouse.rs`, `freminal/src/gui/terminal/pty_mouse_report.rs`,
  `freminal/src/gui/terminal/input.rs` (the mouse call sites).
- **Suggested approach:** mouse reports are not key events; send their bytes raw
  (`InputEvent::Key(bytes)`, as paste does) instead of through the key encoder.
- **Verification:** a test with kitty flag 8 set that a click produces the SGR report bytes
  unchanged; existing mouse tests green.
- **Scheduling:** within Task 147, after 147.5 (same `input.rs`).
- **Status: done (2026-11-19).** `handle_pointer_button`/`_moved`/`_scroll` return the
  framed report bytes; `send_mouse_report` sends them as `InputEvent::Key` (the button path
  keeps its broadcast and recording tail, minus the encoding). The alternate-screen
  wheel-to-arrow path stays on the key encoder. Flag 2 had the same effect as flag 8 on
  release/repeat-typed bytes; both are gone. New tests under flags 8 and 1|2|8 fail on the
  old path. The orchestrator deleted the now-unused `raw_ascii_bytes_to_terminal_input` and
  its re-export.

#### 147.C2 — SO/SI are acted on in VT52 mode

- **Surfaced:** 147.20 (2026-11-19).
- **Impact:** after 147.20, SO and SI shift GL in VT52 mode too. Whether a VT52 terminal
  (and xterm's VT52 emulation) honours them is not established.
- **Scope of fix:** `freminal-terminal-emulator/src/ansi.rs` (the SO/SI arm) and tests.
- **Suggested approach:** check xterm's VT52 parse table (`VTPrsTbl.c` `vt52_table`) and the
  VT100 User Guide's VT52-mode description before deciding; follow xterm unless the DEC
  documentation contradicts it. Do not infer.
- **Verification:** a VT52-mode test for SO/SI matching the chosen reference.
- **Scheduling:** within Task 147, before 147.17.
- **Status: done (2026-11-20).** xterm `VTPrsTbl.c` `vt52_table` maps SO/SI to `CASE_IGNORE`
  (9262-9263), as does `vt52_esc_table` (9586-9587). In VT52 mode the SO/SI arm returns
  without output or a text-run flush. Two parser tests and five `vttest_vt52.rs` tests (SO
  inside a text run, `ESC F`/`ESC G` unaffected, ANSI state restored, SO/SI after a VT52
  round trip). The fix's removal fails four of them.

#### 147.C5 — VT52 escape sequences xterm ignores

- **Surfaced:** 147.C2 (2026-11-20). Predates Task 147.
- **Impact:** in xterm's `vt52_esc_table`, `ESC ( ) * + SP $` are `CASE_VT52_IGNORE`
  (`VTPrsTbl.c:9609-9627`, `charproc.c:3732-3735`): the parser enters `vt52_ignore_table`,
  where the next printable byte is swallowed (`CASE_GROUND_STATE`) while C0 controls still
  execute. freminal does not swallow it, so `ESC ( 0` in VT52 mode prints `0`. Separately,
  `ESC N O n o | } ~` are `CASE_GROUND_STATE` in xterm (silently ignored; 9666-9667,
  9703-9707, 9724-9726), where freminal's `handle_vt52_escape` emits `TerminalOutput::Invalid`
  with a `warn!`.
- **Scope of fix:** `freminal-terminal-emulator/src/ansi.rs` VT52 escape handling and tests.
- **Verification:** VT52 tests: `ESC ( 0 A` prints `A` only; a C0 control after `ESC (` still
  executes; `ESC N` and friends produce no output and no warning.
- **Scheduling:** within Task 147, before 147.17.
- **Status: done (2026-11-20; scope widened in review to all of `vt52_esc_table`).** After a
  VT52 `ESC`, freminal now follows xterm's table byte for byte: all sixteen 0x20-0x2F bytes are
  `CASE_VT52_IGNORE` (`Vt52Ignore`, which swallows the next printable byte); `CASE_GROUND_STATE`
  finals are silent (`debug!`); CR/LF/VT/FF/BS/HT/BEL/ENQ execute and the escape continues
  (`parsestate` is untouched, `charproc.c:3680-3769`); other C0 and DEL are dropped; ESC
  restarts; CAN/SUB return to ground (`ResetState`). Non-ASCII follows `charproc.c:3221-3239`:
  a character above U+00FF ends either state, U+0080-U+00FF indexes the table (ignored, state
  stays); the partial character is held across `push` in `Vt52NonAscii` (named
  `Vt52Table`, `Utf8Prefix`), with invalid UTF-8 as one replacement character. 26 parser and
  11 `vttest_vt52.rs` tests; mutations fail 13 and 11 of them. Not mirrored: xterm absorbing a
  combining mark onto the previous cell before the table lookup, and the CAN/SUB error glyph.

#### 147.C3 — `kent` joins the encoder-checked terminfo set after Task 151

- **Surfaced:** Task 151 confirmation review, N6 (2026-11-19).
- **Impact:** Task 151 (PR #588) lists `kent=\EOM` in `DECLARED_NOT_SENT` in its
  table-of-truth test (`terminal_handler/dcs/terminfo_truth_tests.rs`), because on `main`
  keypad Enter never sends it. 147.5 makes keypad Enter in application mode send `SS3 M`,
  which is `kent`'s value, so once both are on one branch `kent` is sent and must be checked
  against the encoder like every other `k*` capability.
- **Scope of fix:** that test file only (move `kent` out of `DECLARED_NOT_SENT` and check it
  against the 147.5 raw-key keypad path's bytes in application keypad mode).
- **Verification:** the table-of-truth test passes with `kent` checked and fails if the
  keypad Enter bytes change.
- **Scheduling:** when Task 147 rebases onto `main` after #588 merges (if Task 147 merges
  first instead, Task 151 does it on its rebase). Before the Task 147 adversarial review.
- **Status: done (2026-11-20).** The keypad encoder is GUI-side (147.5), which the emulator
  crate cannot reach, so the terminfo parser moved to `freminal_common::terminfo_source`
  behind a test-only `test-utils` feature (the `freminal-notify` / `freminal-windowing`
  pattern; absent from normal and release builds). The truth test gains `CHECKED_IN_GUI`
  (only `kent`) and checks every `k*` capability is in exactly one of three sets.
  `keypad_enter_matches_terminfo_kent` compares the real keypad path's bytes with the parsed
  `kent` value (a mutation to `SS3 N` fails it). Rebase note: 151's truth test called
  `to_payload` with the pre-147.4/147.5 argument list; the orchestrator folded that fix into
  147.4 and 147.5 so every commit builds (each checked with clippy).

#### 147.C4 — Licence check of the shell-integration scripts' third-party-derived parts

- **Surfaced:** Task 151 PR review P1 and 151.C5 (2026-11-20). Predates Task 147.
- **Impact:** Task 151 found that Ghostty's bash and zsh integration files are GPLv3 (they
  derive from kitty's), and that copying from them into freminal's MIT scripts is not
  allowed. `Documents/DESIGN_DECISIONS.md` (around the zsh `.zshenv` `ZDOTDIR` description)
  calls part of freminal's existing zsh bootstrap "Ghostty-derived". If that code was copied,
  the same problem exists on `main` today.
- **Scope of fix:** a READ-ONLY provenance audit of `shell-integration/` (git history of each
  file, the comments, and a comparison with the upstream files done by someone who then does
  not write the replacement), then, for anything copied from a GPL source, a clean-room
  rewrite from a behaviour specification, as 151.C5 did. Ghostty's MIT-licensed files (for
  example its fish integration) may be used with the MIT notice in `ATTRIBUTIONS.md`.
- **Verification:** each script's third-party-derived parts are either rewritten clean-room
  or attributed under a compatible licence; the existing shell-integration tests pass.
- **Scheduling:** within Task 147, before 147.17. If the audit finds GPL-derived code, stop
  and report to the maintainer before rewriting.
- **Status: done (2026-11-20).** A READ-ONLY audit compared every file under
  `shell-integration/` with Ghostty's and kitty's current bash, zsh and fish integrations
  (history, comments, a normalised line-overlap count, block-by-block reading). Verdict:
  nothing COPIED or DERIVED-CLOSELY from a GPL source. `.zshenv` shares no lines with any
  upstream file; the other scripts share only forced strings (`command sudo
  --preserve-env=TERMINFO`, `break`, `*)`) and standard idioms (`${arr:#name}` hook ordering,
  `declare -p PROMPT_COMMAND`). One verbatim string came from Ghostty's MIT fish file (the
  `sudo` function's `-d` description); it was reworded. The "Ghostty-derived `+X`" wording
  in `DESIGN_DECISIONS.md` and `PLAN_VERSION_090.md` (the code uses `${+name}` and is
  independent) and the `.zshenv` comment that echoed it were corrected. No rewrite needed.

### 147 Adversarial review (2026-11-20)

Three independent reviewers (parser/emulator, buffer/GUI, tooling/docs/plan) reviewed the
branch diff against each subtask's spec and status note. 37 findings, two major. Every
finding is fixed on this branch or routed to the task that owns the code.

| ID  | Severity | Subtask   | Finding (short)                                                                | Disposition                        |
| --- | -------- | --------- | ------------------------------------------------------------------------------ | ---------------------------------- |
| A1  | major    | 147.8     | C1 ST (0x9C) does not end OSC/DCS/APC under S8C1T, though the docs say it does | fix (implement)                    |
| A2  | minor    | 147.8     | CAN/SUB do not cancel DCS/APC                                                  | fix (implement, as xterm)          |
| A3  | minor    | 147.8/20  | ESC inside an ESC-intermediate sequence does not restart the escape            | fix                                |
| A4  | minor    | 147.20    | a designator final after a second intermediate prints as text                  | fix (swallow)                      |
| A5  | minor    | 147.24    | non-ASCII `AltText` under kitty flag 8 is encoded per byte; the test pins it   | fix (per character)                |
| A6  | minor    | 147.2     | `ris_resets_vt52_mode_within_the_same_push` passes without its fix             | fix                                |
| A7  | minor    | 147.20    | `ESC % @` / `ESC % G` still `warn!` "not yet implemented"                      | fix (`debug!`)                     |
| A8  | minor    | 147.17    | `SUPPORTED_CONTROL_CODES.md` contradicts the code                              | fix (with C3)                      |
| A9  | nit      | 147.C3    | `terminfo_source` panics under the `test-utils` feature                        | accepted: the `test-utils` pattern |
| A10 | nit      | 147.1     | unknown-mode DECRQM logs `warn!` with a byte vector                            | fix                                |
| A11 | nit      | 147.24    | `sync_mode_flags` doc list is stale                                            | fix                                |
| A12 | nit      | 147.5     | application keypad drops modifiers                                             | Task 137                           |
| A13 | minor    | 147.14    | Windows: no handle-identity check before delete-on-close                       | Task 135                           |
| B1  | minor    | 147.19    | fixed-origin sixel clears images below its own rows                            | fix                                |
| B2  | minor    | 147.24    | a bound Alt+letter leaks `ESC letter` through `Event::Text`                    | fix                                |
| B3  | minor    | 147.19    | new `too_many_arguments` allowance on `place_image_fixed_origin`               | fix (placement struct)             |
| B4  | nit      | 147.12    | `alternate_reenter` −15.2% is not from the code change                         | fix (note)                         |
| B5  | nit      | 147.5     | keypad modifier parameter and VT52 `ESC ? x` keypad                            | Task 137, GAPS row                 |
| B6  | nit      | 147.5     | an over-long, badly wrapped doc line in `input.rs`                             | fix                                |
| B7  | minor    | 147.10    | `inject_layout_commands` and recording are untested                            | fix                                |
| B8  | minor    | 147.11    | the `apply_new_config` glue is untested                                        | fix                                |
| B9  | minor    | 147.27    | opacity < 1 under DECSCNM may tint explicit-background cells                   | Task 136 (confirmed, not fixed)    |
| B10 | nit      | 147.24    | `modifyOtherKeys` Alt+letter form                                              | Task 137                           |
| C1  | major    | 147.26    | the gl-pixel-only clippy fails on Windows/macOS CI legs                        | fix                                |
| C2  | minor    | 147.31    | `check-docs` skips feature-gated modules (30 warnings)                         | fix (`--all-features`)             |
| C3  | minor    | 147.17    | `SUPPORTED_CONTROL_CODES.md` stale (with A8)                                   | fix                                |
| C4  | minor    | 147.10    | stale `pty_write_tx` / G1 references in a skill and DESIGN_DECISIONS           | fix                                |
| C5  | minor    | plan      | 147.24 and 147.9 specs not amended                                             | fix                                |
| C6  | minor    | config    | three config keys missing from the Nix module                                  | fix                                |
| C7  | minor    | 147.C4    | shell scripts edited without a version bump; a stale PLAN_090 line             | fix                                |
| C8  | nit      | plan      | MASTER_PLAN ranges omit 151/152                                                | fix                                |
| C9  | nit      | plan      | Task 151 has no `Merged` line                                                  | fix                                |
| C10 | nit      | plan      | MASTER_PLAN prettier drift                                                     | fix                                |
| C11 | nit      | agents.md | `xtask ci` row and subcommand list                                             | fix                                |
| C12 | nit      | 147.16    | repo vs pre-commit markdownlint configs differ                                 | fix (document in `agents.md`)      |
| C13 | nit      | plan      | "Resolved" vs "done" labels                                                    | fix                                |
| C14 | nit      | 147.26    | gl-pixel-only clippy compiles on every leg                                     | fix (with C1)                      |

- **A9 (accepted):** `freminal_common::terminfo_source` exists only for tests in two crates,
  so it cannot be `cfg(test)`; the `test-utils` feature is the workspace's established form
  for that (`freminal-notify`, `freminal-windowing`), and no non-test build enables it.
- **Routed to Task 137:** A12, B5, B10 (keypad modifier parameter, VT52 `ESC ? x` keypad,
  `modifyOtherKeys` Alt+letter). **Routed to Task 135:** A13.
- **Fix outcomes (2026-11-20).** Every finding marked "fix" is fixed with a test that fails
  without it (mutations run). Notes:
  - A1: 0x9C terminates OSC/DCS/APC only while S8C1T is active, the gate freminal already
    uses for every C1 control (xterm's `sos_table` treats it as ST unconditionally). A5: a
    new `TerminalInput::NonAscii(char)`; plain text and `AltText` both encode per character
    under kitty flag 8 (`é` → `CSI 233 u`). A6: the RIS `vt52_mode` reset was unreachable
    (`ESC c` is not a VT52 command) and is removed. String-state byte checks cost +1.5%
    (OSC) and +4.4% (kitty APC) in user-space instructions; plain text, CSI and SGR are
    unchanged (`perf stat`, because Criterion was unusable under load).
  - B1's sibling (the `Inline` path also clears same-column images below the new image's
    rows when the cursor is mid-screen) is pre-existing and routed to Task 136. B3 introduced
    `PlaceImageRequest`; `place_image_at` keeps its own pre-existing allowance. B7/B8 added
    a shared GUI test fixture (`gui/test_support.rs`) and split `apply_config_state` out of
    `apply_new_config`.
  - **B9 routed to Task 136:** at `background_opacity < 1`, explicit-background cells are
    composited over the translucent base fill under DECSCNM and under an OSC 11 override
    (the latter since Task 132.11). A correct fix needs blend/stencil control in `gl_facade`
    or a per-cell base fill; three characterisation pixel tests pin today's values.
  - C2 found 30 more rustdoc warnings behind features; `check-docs` now uses
    `--all-features` and is clean. C1/C14: the `decision` field's `cfg` is narrowed to its
    readers, and the gl-pixel-only clippy runs from `cargo xtask check-gl-pixel` (one Linux
    CI step), `lint` and `check-windows`, not from `check`. C6: `ui.auto_detect_urls` and
    `security.password_indicator` were added; `logging.write_to_file` is deprecated and
    ignored at runtime, so it is deliberately not exposed in the module or the example.
  - New, routed to Task 137: under kitty flags 1/8, Alt+text sends `CSI 98 u` with no Alt
    modifier (147.24 left Alt to Task 137; the kitty spec gives `CSI 98;3u`).

#### 147 Confirmation review (2026-11-20)

All 37 dispositions confirmed (reproducers re-run; mutations on A1-A5, B1, B2, B8 fail the new
tests); `cargo xtask test`, clippy, `check-docs`, `check-gl-pixel`, `check-windows` and
`lint-markdown` pass. New findings: (1) A4's designator intermediates were stored without a
cap (unbounded memory on a hostile stream) — fixed with a bound and a regression test;
(2, 4, 5, 6) COVERAGE/GAPS, the `standard.rs` xterm attribution and the `push()` state-machine
doc did not yet describe the A1-A5 behaviour (multi-byte designators are ignored, unlike
xterm's `CASE_SCS_PERCENT`/`_DQUOTE`/`_AMPRSND`) — fixed; (3, 8) plan text — fixed; (7) the
whole-string `associated_text` under flags 8+16 — routed to Task 137; (10) a unit test for
`gl_pixel_only_clippy_args` — added. (9) `apply_config_state` carries the
`too_many_lines` allowance that `apply_new_config` already had on `main` (same justification:
seven config dimensions); not shrunk here. (11) `prettier --check` differences on three plan
files predate the review (hook excludes `.md`).

#### 147 PR review (PR #589, 2026-11-20)

CI failed `Test macos-latest`; CodeRabbit left 23 actionable comments (three outside the
diff). Dispositions:

| ID  | Where                      | Finding (short)                                                                   | Disposition                                                               |
| --- | -------------------------- | --------------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| R1  | CI                         | macOS: Alt-prefix GUI tests assume a non-macOS host                               | fix (platform-aware expectations)                                         |
| R2  | agents.md, COVERAGE, GAPS  | table padding                                                                     | fix                                                                       |
| R3  | plan                       | 147.26 note still says `cargo xtask check` runs gl-pixel clippy                   | fix                                                                       |
| R4  | SUPPORTED_CONTROL_CODES    | `?1036` row lacks the macOS exception                                             | fix                                                                       |
| R5  | cursor.rs                  | pending wrap at the DECLRMM right margin is saved as settled                      | fix                                                                       |
| R6  | terminfo_source.rs         | entry body runs into the next entry                                               | fix                                                                       |
| R7  | dcs.rs                     | C1 ST after a doubled tmux `ESC ESC` drops a body byte                            | fix                                                                       |
| R8  | ansi.rs                    | under S8C1T a UTF-8 continuation `0x9C` (`Ŝ`) ends an OSC                         | fix (ST only at a character boundary)                                     |
| R9  | ansi.rs                    | a mode change inside a tmux payload governs later outer bytes only after the read | fix if contained; else route                                              |
| R10 | ansi.rs                    | CSI buffer-recycling test cannot detect a dropped buffer                          | fix                                                                       |
| R11 | input.rs                   | `associated_text` repeated on every character                                     | Task 137 (already routed)                                                 |
| R12 | input.rs                   | shifted alternate reported without Shift                                          | fix                                                                       |
| R13 | internal.rs                | RIS during tmux replay leaves parser DECANM stale                                 | fix                                                                       |
| R14 | graphics_kitty.rs          | Windows: delete access refused by a non-sharing reader rejects the image          | fix (fall back to a plain read)                                           |
| R15 | graphics_kitty.rs          | Windows: a link swapped in before the open is followed and deleted                | fix (`FILE_FLAG_OPEN_REPARSE_POINT`); residue Task 135                    |
| R16 | gpu.rs                     | DECSCNM base fill at opacity < 1 double-composites cells (B9, new for DECSCNM)    | fix (no DECSCNM base fill below full opacity); OSC 11 path stays Task 136 |
| R17 | pixel_harness.rs           | blend state is not set per drawn frame                                            | fix                                                                       |
| R18 | GUI input.rs               | Alt prefix uses end-of-frame modifiers, not the text event's                      | fix                                                                       |
| R19 | test_support.rs            | fixture reads persisted app state and layouts                                     | fix (injected empty state)                                                |
| R20 | xtask                      | `CARGO_ENCODED_RUSTDOCFLAGS` overrides the appended `-D warnings`                 | fix                                                                       |
| R21 | csi.rs (outside diff)      | an unterminated CSI grows its buffers without bound                               | fix (byte cap)                                                            |
| R22 | standard.rs (outside diff) | `ESC - A`, `ESC $ B` print their final                                            | fix (consume and ignore)                                                  |

- **Outcomes (2026-11-20).** Every "fix" row is fixed with a test. R9 was contained: the parser
  stops after a completed `DCS tmux;` wrapper (`push_through_tmux_passthrough`), the state
  layer applies its payload, and parsing resumes, so a payload's S8C1T or DECANM governs the
  rest of the read. R8 tracks pending UTF-8 continuation bytes in string states, so `0x9C`
  ends a string only at a character boundary. R21 caps an open CSI at `MAX_CSI_BYTES` and
  discards the rest through its final. R22 consumes `ESC - . / $` designators through their
  final. R16: below full opacity DECSCNM uses the normal pane base (no reversed remainder),
  `pane_base_color(inversion, opacity)` stays the single source, and an opacity change now
  forces a pane rebuild (`bg_opacity_changed`). R15: the delete-on-close open adds
  `FILE_FLAG_OPEN_REPARSE_POINT` and rejects reparse points; a rejected link entry is itself
  removed when the handle closes, its target never opened. R14 falls back to a plain read on
  a sharing or access error. R2: no change; every row has the same column width in
  characters and prettier passes (the comment counted bytes of `—` and `£`). Benchmarks
  against HEAD on a quiet machine: parser and encoder within ±4%, `instanced_bg*` within ±3%
  (one +23% case re-ran at −0.9% and −2.1%).

- **Second round (CodeRabbit, 2026-11-20), R23-R35:** R23 the bracketed-paste row gives the
  S8C1T form; R24 the saved cursor's wrap edge is the row's (`wrap_column_for_row`, the same
  `row_wrap_bounds` `insert_text` uses), so a double-width row wraps at half its span; R25 a
  pending cell that settles anywhere but exactly at the edge becomes the next column; R26-R27
  the UTF-8 tracker validates each continuation against its lead (`E0 9C` is ST) and resets
  when a string ends; R28 a bare ESC that ends a `DCS tmux;` wrapper leaves its second byte
  for after the payload (`resume_pending_escape` picks the escape table); R29 the exact-cap
  CSI test compares with `ESC [ 0 m`; R30 `finish_output_batch` runs once per read; R31 a
  suppressed frame refreshes the carried modifiers (`observe_suppressed_frame`); R32 the
  ground state applies the same UTF-8 rule to every 8-bit C1 byte (`Ŏ` is text, not SS2);
  R34 a parked screen is resized under the shared left/right margins; R35 an empty inherited
  `CARGO_ENCODED_RUSTDOCFLAGS` carries the denial. R33 (per-destination broadcast encoding)
  is routed to Task 137. Mutation checks were run on R24-R28, R31, R32 and R34.
- **Third round (CodeRabbit, 2026-11-20), R36 (outside the diff):** the state layer held back a
  read's tail whenever a lead byte's declared length ran past the end, so `ESC SP G E0 84`
  withheld the `84` (IND under S8C1T) until the next read. The tail is now held only when it
  is a well-formed incomplete prefix (`Utf8Tracker::is_incomplete_prefix`, the parser's own
  Table 3-7 ranges); `F0 9F` is still held. End-to-end tests for both; the IND test fails
  without the fix.

### 147 Status notes

- **147.1 (done).** `Mode::UnknownQuery { namespace, params }` reuses `ModeNamespace`; the
  reply comes from `Mode::report`, so `CSI 2 $ p` gets `CSI 2 ; 0 $ y` and `CSI ? 9999 $ p`
  keeps the `?` form, in both 7-bit and S8C1T framing. Five new tests (four fail on the old
  `?{digits}` format).
- **147.2 (done).** `apply_parser_mode_change` (in `ansi.rs`) is the one rule: `push()`
  applies it to each output a byte emits, so S8C1T/S7C1T, DECANM and RIS govern the next byte
  of the same read. `TerminalState` no longer writes the parser's modes; it folds the same
  function over the outputs in event order (`WireModes`) to seed each tmux payload parser
  with the mode at its position, and writes the result back so a mode change inside a
  payload still governs later reads. Changed test: `ris_resets_the_handlers_decanm_mirror_to_ansi`
  (`reset_table.rs`) drove RIS and DECRQM through a VT52 read, which only worked under the old
  per-read rule; it now drives the handler directly. The plan's "S8C1T, RIS, `0x9B`" case
  cannot fail on the old code (that read was 7-bit throughout), so it became "already 8-bit,
  RIS, `0x9B`". Benchmarks (quiet machine, vs `before_147_2`): `parser_push` −5.0%,
  `parser_push_sgr` +2.4% / +1.4%, `parse_and_handle_80x24` −0.7%,
  `handle_incoming_data_4096` −1.6%. The GAPS "per read" paragraph goes in 147.17.
- **147.3 (done).** `TerminalSnapshot::s8c1t` (from the handler's mirror, so RIS resets it)
  and `InputModes::s8c1t`, which carries the temporary `TODO(147.4)` dead-code exception.
  Not added to change detection: it changes nothing rendered, and the encoders read the
  latest snapshot.
- **147.4 (done).** `Introducer { Csi, Ss3 }` (`input.rs`) writes every generated CSI/SS3:
  `to_payload`/`to_payload_kkp` take `S8c1t` (all key families, `modifyOtherKeys`, the
  application escape key, kitty `CSI u` and event-type forms); focus reports follow the
  handler's mode; GUI mouse reports (X11, SGR, SGR-pixel) and bracketed-paste markers take it
  from the snapshot. `wrap_bracketed_paste` now returns bytes (a `String` would turn `0x9B`
  into `C2 9B`). 7-bit fixed sequences stay borrowed (no allocation). No expected byte
  string changed. There is no VT52 key encoder, so the VT52 case has nothing to test. New
  `bench_to_payload` (added before the change, row added to `freminal-bench-table`):
  interleaved minimum of 10, `legacy_mix` 160.8 → 147.6 ns, `kkp_mix` 210.8 → 199.6 ns
  (machine load about 13; within noise). Surfaced 147.C1.
- **147.5 (done).** The raw-key keypad path (`drain_pending_raw_keys`) honours DECKPAM:
  `legacy_keypad_bytes` sends `SS3 p`…`SS3 y`, `n m l k j o X M` through
  `Introducer::Ss3` (8-bit `0x8F` under S8C1T); numeric and kitty paths unchanged.
  `TerminalInput::KeyPad`, its arms, its twelve tests and `to_payload`'s keypad parameter are
  deleted. New tests drive real snapshots (`ESC =`, `ESC >`, `ESC SP G`). No benchmark: the
  only encoder change is removed arms and one fewer parameter.
- **147.6 (done).** `SavedCursor` gains `wrap: SavedWrapState { Settled, Pending }`; a save at
  `x == width` stores column `width - 1` plus `Pending`, and DECRC restores `x = width` while
  the saved column is still the right edge (width unchanged). After a resize it clamps and
  clears pending wrap; 147.18 replaces that with full resize tracking. All three save paths
  share `Buffer::save_cursor`/`restore_cursor`. Tests at buffer and emulator level, for
  DECSC, `CSI s`/`u`, `?1048` and `?1049`.
- **147.7 (done).** `CSI Ps u` with any parameter (`0` included) emits nothing and logs a
  payload-free `debug!`; `CSI u` still restores; the kitty prefixed forms are unchanged.
  Changed test: `numeric_params_are_scorc` became three "ignored" tests. New end-to-end test
  through `TerminalState`.
- **147.8 (done).** New `ParserOutcome::FinishedAtEsc`: an OSC, DCS or APC sub-parser that
  sees ESC followed by anything but `\` dispatches the body (DCS and APC as if ST had
  arrived; an over-cap string is dropped with its one warn), and the top-level parser
  re-enters the escape state with the following byte (`restart_escape_with`), within a read
  or across reads. Decisions at execution: inside `DCS tmux;` an `ESC ESC` pair is still data
  and only an unpaired ESC followed by a byte other than `\` ends the string (tmux tests
  unchanged); in OSC, `ESC BEL` now ends the string at the ESC and `BEL` is the second byte of
  a new escape (as for a top-level `ESC BEL`), while a plain BEL still terminates; CAN/SUB
  and the ignored C0 bytes keep the 129.C5 rules. Changed tests: the ESC-kept-in-body and
  ESC-then-BEL cap tests and three overflow tests (each renamed to the new rule). New
  `tests/string_bare_esc.rs`: 15 behaviour tests fail on the old tree, 6 guards pass. The
  orchestrator made the new `FinishedAtEsc` arms in the CSI and standard-parser matches
  handle the outcome instead of `unreachable!()`. Benchmarks (vs `before_147_8`, before run
  load-affected): plain text −4%, SGR −8.7%, OSC 9 −5% / +2%, kitty APC −28%, sixel −37%; no
  regression. `fuzz-replay` clean.
- **147.20 (done).** New `buffer_states/charset.rs` (`CharacterSet`, `CharsetSlot`,
  `CharsetState::translate`, the line-drawing table moved from the deleted `line_draw.rs`).
  `TerminalOutput` gains `DesignateCharset`, `InvokeGl`, `InvokeGr` and `SingleShift` and
  loses `DecSpecialGraphics` and 23 placeholder variants. SO/SI, `ESC n o ~ } |`, `ESC N O`
  and 8-bit SS2/SS3 (under S8C1T) are implemented; one designator helper serves
  `( ) * +` with B/A/0; an unknown final in 0x30–0x7E is ignored, outside that range it is
  still `Invalid`. The handler's `charset: CharsetState` replaces `character_replace`
  (borrowed fast path when GL is ASCII and no shift is pending); DECSC/DECRC, RIS, DECSTR
  and VT52 entry/exit (`vt52_saved_charset`) as decided. `reset()` stays under the line limit
  through a small `reset_parser_modes` helper. `CharsetDefault`/`CharsetUTF8` (`ESC % @/G`)
  remain log-only (not G-set designations). Changed tests: the placeholder-pinning tests are
  replaced, `si_so_consumed_as_control_characters` became
  `si_so_switch_gl_between_g0_and_g1`, `ESC ( Z` moved from invalid to ignored, and the C1
  fall-through probe uses PM (SS2 is no longer ignored). Benchmarks (vs `before_147_20`):
  `parser_push` −17%, `handle_incoming_data_4096` −3%, `parse_and_handle_80x24` −8% to −11%
  on three re-runs (+11% on a load-affected first run). `fuzz-replay` clean. Surfaced 147.C2.
- **147.15 (done).** `FreminalAnsiParser` keeps one spare `CsiBuffers` set; `start_csi` lends
  it to each new `AnsiCsiParser` (`from_buffers`) and every exit path (finish, invalid,
  failure, `FinishedAtEsc`, abort on ESC) takes it back cleared (`take_buffers`); a buffer
  over `RETAINED_CAPACITY_MAX` (1024 bytes) shrinks to its initial size. Decision: the
  buffers live on `FreminalAnsiParser`, not `ParserInner`, so `sub_parsers_stay_small` (128
  bytes) is unchanged; `FreminalAnsiParser` grew by 72 bytes. Benchmarks (old and new
  binaries interleaved 30–40 times under load; min / median): SGR-heavy −6% / −5%, SGR
  indexed −6% / −9%, plain text and `parse_and_handle_80x24` within noise. A real but modest
  gain: the mallocs removed were fast-path ones. `fuzz-replay` clean.
- **147.21 (done).** New mode types `Kam`, `Srm`, `HiliteMouseTracking`, `MetaSendsEighthBit`,
  `NumLockOverridesKeypad` (one file each); set/reset log at `debug!` and change nothing;
  DECRQM answers `Ps = 0` (ANSI form for KAM/SRM, DEC form for the rest) through
  `Mode::report`. `MouseTrack::XtMseHilite` is removed, so `?1001h` no longer enables normal
  button reports or claims support. Changed tests: the four `test_hilite_mouse_tracking_*`
  tests and three `?1001` dispatch tests (rewritten for "recognised, not implemented"); the
  147.1 "unknown ANSI mode" tests query `99` now that `2` is KAM. COVERAGE/GAPS are 147.17.
- **147.24 (done, macOS half moved to Task 152).** `?1036` (`MetaSendsEscape`, default set)
  and `?1039` (`AltSendsEscape`, stored and reported) as typed modes; DECRQM 1/2; RIS and
  DECSTR return them to default (`TerminalModes`, so no handler reset-table row). The GUI
  builds `TerminalInput::AltText` where the event is read (`gui/terminal/alt_text.rs`, a pure
  decision over `HostPlatform`): Alt without Ctrl prefixes `ESC` while `?1036` is set, Ctrl+Alt
  (Windows AltGr) never does, and with any kitty keyboard flag the bytes equal plain text.
  **Maintainer decision (2026-11-19, option 1):** on macOS Option keeps composing; the
  `[keyboard] macos_option_as_alt` option was implemented, found not to work (egui's text is
  the Option-composed character, so `both` sent `ESC ∫`) and removed; it is Task 152, with
  per-side `left`/`right`. `bench_to_payload` (vs `before_147_24`): `legacy_mix` −14.3%,
  `kkp_mix` −5.3% (baseline under load; no regression).
- **147.9 (done).** New `TerminalOutput::GraphicsAttributeRequest { item, action }`
  (`buffer_states/xtsmgraphics.rs`), parser `csi_commands/xtsmgraphics.rs`, reply in
  `reports.rs` through `write_csi_response`. Forms read from xterm `charproc.c`
  (`CASE_GRAPHICS_ATTRIBUTES`): value fields only on status 0, two values for geometry;
  exactly three parameters or no reply; a non-numeric `Pi`/`Pa` gets no reply. Sixel read is
  grid × cell pixels clamped to 4096, the maximum `min(MAX_SIXEL_DIMENSION,
  isqrt(MAX_SIXEL_PIXELS))`. **Plan correction:** the subtask table said `Pi=3` answers status
  3, but its own premise ("xterm with ReGIS absent") gives status 1: with
  `OPT_REGIS_GRAPHICS` compiled out, `Pi=3` reaches xterm's `default:` arm. The orchestrator
  changed it to 1. `CSI > Pm t` / `CSI > Pm T` are routed to a debug-logged ignore. The
  dispatch matrix gains three routed keys (52). COVERAGE/GAPS are 147.17.
- **147.10 (done).** `layout_ops` sends each startup command as one
  `InputEvent::Key(command + "\n")` on `Pane::input_tx` (`send_startup_command`), so it is
  ordered with other input and recorded as `PtyInput` (redacted while echo is off, like typed
  input). `layout_ops` was the only production user of `Pane::pty_write_tx`, so the field,
  the `TabChannels` field and the `pty.rs` clone are removed, with their test and bench
  constructors; the orchestrator also removed `TerminalEmulator::clone_write_tx`, which then
  had no caller.
- **147.11 (done).** `host_capabilities::config_change(old, new, daemon)` decides (`Some`
  only when the resolved capabilities differ); `broadcast_config_change` sends through the
  existing `send_host_capabilities`; `settings_dispatch::apply_new_config` calls
  `FreminalGui::broadcast_config_host_capabilities`. Tests: unchanged config across every
  routing and daemon; routing and toggle changes; fields capabilities do not read (font,
  cursor, non-OSC-99 routings, a routing change with `osc_99` off); and real `crossbeam`
  senders in two windows (one in flight) receive exactly one event per pane, or none.
- **147.12 (done).** `Buffer::switch_to_alternate_cleared` is the `?1049` entry: a first-ever
  (or post-RIS) alternate screen is built once, blank with the erase background; a parked one
  is switched in and cleared by the usual rebuild (renumbered past it). Both switch entries
  share `enter_alternate(AlternateContents)`, and `blank_alternate_store` is the one BCE row
  builder. The handler's `?1049` Enter arm calls a handler counterpart that also clears the
  alternate placements (`scroll_ops.rs`, outside the listed file, approved). A first version
  that kept the store by inferring "untouched" from row shape was rejected in review as a
  heuristic. `bench_alternate_screen_switch` vs `before_147_12`: `enter_alternate` −12.2%,
  `leave_alternate` −5.4% (noise). `alternate_reenter` times plain `switch_to_alternate`, which
  this change only restructured; its −15.2% is not attributable to the change (review B4).
- **147.18 (done).** `set_size` pins the DECSC slot to its cell (`pin_saved_cursor`: row number
  plus column), carries it through reflow with the new `ReflowRemap::map_cell` (the stage
  remap that carries marks and blocks, now cell-granular; `offset_to_new_row` delegates to
  it), and settles it back to screen coordinates after the scrollback limit
  (`settle_saved_cursor`). Parked screens get it through `resize_parked_screen`'s `set_size`,
  which covers xterm's `AdjustSavedCursor`. Ghostty's fallbacks: a cell off the active area goes
  to the top-left; pending wrap off the last column becomes `x + 1`. A slot saved on blank
  space below every stored row keeps its screen position. 13 buffer tests plus two handler
  tests (widening, `?1049h`/resize/`?1049l`). Five earlier tests (147.6's "clear pending wrap
  on resize", two 125.C6 reflow tests) were rewritten to the 147.18 semantics.
  `buffer_resize` / `reflow_full_depth`: within noise (a back-to-back rerun; the first run's
  baseline was taken under load).
- **147.13 (done).** `BoundedChunkAssembler` grows through `append_within_cap`: the cap is
  checked first, then capacity doubles clamped to the cap (`reserve_exact`). Base64 chunks
  decode into a scratch buffer bounded by `max_decoded_len` and are checked before they are
  appended; `finish()` flushes the final quantum the same way. Small-cap tests pin capacity
  `<=` cap for raw, base64 and the flush. The unknown-id `a=p` log is `debug!`.
- **147.14 (done).** The deletion decision is made before the open
  (`KittyFileUse { Read, Consume }` → `KittyFileFate { Keep, Delete }`, same
  `may_delete_kitty_temp_file` rule). On Windows a `Delete` file is opened with
  `GENERIC_READ | DELETE`, read/write/delete sharing and `FILE_FLAG_DELETE_ON_CLOSE` (winapi
  0.3.9 `winbase` feature on the existing dependency; no new crate), checked to be a regular
  file by handle, read and dropped; `delete_kitty_temp_file` only reports a path still
  present. Unix unchanged. Three Windows-only tests run on the Windows CI job. **Platform
  difference, routed to Task 135:** on Windows a marked `t=t` file is deleted even if reading
  it then fails (over the size cap); on unix it is kept. Task 135 settles which is right
  against kitty's source and aligns both. The orchestrator also gated
  `FreminalGui::broadcast_host_capabilities` to the daemon platforms: 147.11 left the
  daemon glue as its only caller, which `check-windows` flagged as dead code.
- **147.16 (done).** `lint_markdown` also excludes `.direnv`, `**/.direnv` and
  `**/node_modules` (`.opencode/node_modules` was a second noise source), and `lint()` no
  longer swallows its failure. With that glob markdownlint-cli2 lints the 62 tracked `.md`
  files with 0 issues: the ~550 MD060/MD033 errors cited in 126.C1 had already been cleared,
  so no prettier pass or scoped rule was needed. 126.C1 is closed by this. Noted, not
  changed: the pre-commit hook's Nix-store markdownlint config disables MD033 and MD060 while
  `.markdownlint-cli2.yaml` does not (the repo config is the stricter of the two and passes
  today), and no GitHub workflow runs `cargo xtask lint`, so the gate is local.
- **147.32 (done).** `RowStore` is used only by a `#[cfg(debug_assertions)]` test, so the
  import is gone and that test names `crate::buffer::RowStore`. `cargo bench --no-run --all`
  emits no warnings (apart from the pre-existing `portable-pty` manifest note).
- **147.19 (done).** xterm checked (`graphics_sixel.c:395-405`, `:853-866`): with sixel
  scrolling off the graphic is created at `(0, 0)` and never scrolls. `place_image` takes
  `ImagePlacementMode { Inline, FixedOrigin }`; `FixedOrigin` anchors at the first visible
  row, column 0, clipped to the screen, without pushing or evicting rows or moving the cursor
  (a young primary buffer materialises only the screen rows the image covers). Sixel uses it
  under `Decsdm::DisplayMode` and no longer restores the cursor; `restore_cursor_to_image_origin`
  stays (kitty `C=1`, iTerm2 `doNotMoveCursor`). Kitty and iTerm2 pass `Inline`. The two
  DECSDM tests were rewritten to xterm's behaviour; eight new buffer tests, including damage
  bounded to the image's rows and an `Inline` regression guard. `place_image_fixed_origin`
  carries the same justified `too_many_arguments` allowance as `place_image`, which it
  forwards. Not changed (outside the plan): a narrower image over a wider one clears only its
  own columns, in both modes.
- **147.23 (done).** xterm checked (`charproc.c:448` `hpLowerleftBugCompat` defaults off;
  `:4759-4770` `ESC l`/`ESC m` set margins). The three handler arms log at `debug!` as ignored
  HP sequences; tests check the cursor and scroll region are unchanged. VT52 `ESC F` is a
  separate parser path, covered by `vt52_special_graphics_charset`.
- **147.29 (done).** `resolve_anchor` takes `ToastOriginWindow { This, Other }`;
  `layout_positioned` maps a matching window to `This`, a foreign or missing origin to
  `Other`. The six `resolve_anchor_*` tests are ported unchanged in behaviour.
- **147.26 (done).** The query items' real users are `gpu-profiling` and the `gl-recording`
  tests (the plan said `frame-profiling`); the seven `Gl` methods, `RecordingState::next_query`
  and its four query methods are gated on those. No `allow(dead_code)`. `clippy_gl_pixel_only`
  (`-p freminal --features gl-pixel --all-targets`, `-D warnings`) first ran from
  `cargo xtask check` and `lint`; review finding C1 moved it out of `check` (it failed on the
  Windows and macOS legs) to `cargo xtask check-gl-pixel` (one Linux CI step), `lint` and
  `check-windows`. The weekly `gl-pixel` workflow only runs tests.
- **147.28 (done).** `command_finished_template` (`nullOr str`, default null) is in the
  home-manager module and its `notificationsSection` binding; a `lib.evalModules` evaluation
  setting it produced `command_finished_template = ...` under `[notifications]` in the
  generated `config.toml`. nixfmt, statix and deadnix pass.
- **147.30 (done).** The link directory is the first of `CARGO_MANIFEST_DIR` and `/var/tmp`
  that is not itself under `/tmp` or `/dev/shm`. From a `git archive` copy under
  `/tmp/opencode` the test failed before the fix and the eight sandbox tests pass after it.
- **147.31 (done).** 168 rustdoc diagnostics across `freminal-common`, `freminal-buffer`,
  `freminal-terminal-emulator` and `freminal` (the plan's 68 was `freminal` alone and had
  grown), now 0 workspace-wide; doc comments only. Links to private items became plain
  backticks (no visibility widened); unresolved ones got a correct public path or backticks.
  Sixteen `freminal-common` modules had a `///` summary on `pub mod` as well as `//!` docs,
  which rustdoc concatenates and resolves in the parent scope; the `///` lines were removed.
  `cargo xtask check-docs` (alias `cdo`) sets `RUSTDOCFLAGS` for that one call, runs from
  `cargo xtask ci`, and is a step in the CI `check` matrix on `stable` + `ubuntu-latest`
  only (Windows- and macOS-only items are not documented there).
- **147.33 (done).** Status markers became plain text, per file and column (the same
  emoji meant different things): COVERAGE `done` / `partial` / `stub` / `missing` (and
  `done (fixed)` in Known Bugs); SGR `done` / `partial` / `missing` / `not planned`; GAPS
  Importance `high` / `medium` / `low` / `not planned` and Type `partial` / `not implemented`.
  Legends rewritten; the 21 dated COVERAGE changelog lines were converted too, so no file
  refers to a marker its legend no longer defines. A word-level diff shows only marker
  changes and padding. The skill names no markers. The malformed GAPS `ESC \` row stays for
  147.17.
- **147.27 (done).** One function, `pane_base_color(ScreenInversion)`, defines the pane base
  (`DefaultBackground` normally, the foreground under DECSCNM) for both sides: the base fill
  (`PaneBackground::base_fill_for(theme_bg, &ResolvedColors, ScreenInversion)`) covers the
  whole `terminal_rect`, so the sub-cell remainder is reversed, and a cell skips its
  background quad only when its effective background is that base (`is_pane_base_color`, a
  pattern match; an `==` on `TerminalColor` measured +25-33% and was replaced). SGR 7 under
  DECSCNM draws the default background. Behaviour change: a background image is now visible
  under DECSCNM (plain cells no longer paint opaque foreground quads over it), consistent with
  normal video. Pixel tests (`decscnm_pixel_tests`, `.#gl-pixel`) cover the remainder, double
  reversal, normal video and cursor-only frames; the harness's `SyntheticFrame` gained
  `screen_inversion` and `reversed_cell` (outside the listed files, as 132.11 did).
  `instanced_bg*`: no regression (the baseline was taken under load).
- **147.17 (done).** COVERAGE, GAPS and `KITTY_PROTOCOL_REFERENCE.md` carry every 147
  behaviour change, with dated 2026-11-20 headers. Spec corrections applied: the `?1036`/`?1039`
  rows document macOS Option composing (Task 152); XTSMGRAPHICS `Pi=3` is status 1; the
  XTGETTCAP `indn`/`rin`/`query-os-name` row came with Task 151 and was verified, not <!-- codespell:ignore rin -->
  duplicated. The malformed `ESC \` row and a COVERAGE row that had lost its Notes cell are
  rebuilt. The GAPS rows for pending wrap, the per-read paragraph, the saved cursor on resize
  and sixel under DECSDM are closed; KAM, SRM, `?1001`, `?1034`, `?1035`, the 96-character
  designators and HP `ESC F`/`l`/`m` are not planned with reasons. `SGR.md` needed no change.
- **147.22 moved to Task 151 (151.2)** by maintainer decision (2026-11-19), so Task 151 owns
  `lookup_termcap` and `res/freminal.ti`. Task 151 runs in the worktree `../freminal-151`.

**Merged** 2026-11-20 via PR #589 (`7cbaa865`).

---

## Task 148 — Security & Fuzz Findings Remediation

### 148 Summary

Fix every finding from the #553 fuzzing and trust-boundary audit: the thirteen published
security advisories, the open fuzz-crash issues, and the audit's lower-severity items. Created
and decomposed on 2026-10-18 on branch `issues-552-553-554/lints-fuzz-coderabbit`, from the
published advisories (read with `gh api repos/fredsystems/freminal/security-advisories`) and
issues #556–#564, #566 and #568.

Scope is exactly those items. Out of scope:

- **#567 and #530** (unmaintained dependencies): fixed after this task, as their own work.
  (#530 has since been resolved on branch `task-530/harfrust-migration`: `rustybuzz` replaced
  by `harfrust` and `ttf-parser` removed from the graph.)
- **#565** (xtask is never linted for Windows): tooling, not a security or fuzz finding.
- **#554** (CodeRabbit configuration) and **#552** (lint relocation): already handled on this
  branch.

Why this is v0.13.0 work: three of the advisories are high severity and are triggered by output
from a benign command (`cat` of a file, remote `ssh` output), so they must ship before the
v0.13.x protocol tasks add more parser surface. Task 126 is the precedent for a safety gate in
this version.

**Severity order is not subtask order.** The subtasks are grouped by the code they touch, so
that parallel worktrees stay file-disjoint (see "148 Execution model"). Priority within a group
is: high-severity advisories first, then medium, then low, then hygiene.

### 148 Maintainer decisions (2026-10-18)

- **All thirteen advisories are published**, before their fixes ship. The public `sixel_parse`
  and `terminal_state` fuzz targets rediscover several of them within seconds, and public CI
  uploads fuzz artifacts, so withholding the details would not have kept them confidential.
  `.github/SECURITY.md` records the policy: public disclosure before a fix is allowed when the
  maintainers judge it serves users better.
- **Remediation is tracked openly** in this task and in the published advisories. When a release
  contains a fix, the matching advisory is updated with its patched version (see 148.27).
- **Reproducers are committed only together with their fix.** `fuzz-replay` is a required CI
  gate, so a reproducer without a fix turns main red. Every subtask that fixes a crash adds its
  minimised reproducer to `fuzz/regressions/<target>/` in the same commit as the fix, plus a
  unit test in the owning crate that fails on the old code. Reproducers live in the advisories
  and issues, not in this document.

### 148 Open decisions (not yet made; resolve at the start of the named subtask)

Per `autonomy-boundaries`, these are stop conditions for their subtask, not inferences to make.
The "leaning" column is the orchestrator's suggestion, not a decision.

| Subtask | Question                                                                      | Leaning                                                      |
| ------- | ----------------------------------------------------------------------------- | ------------------------------------------------------------ |
| 148.15  | Default for kitty file/shm transmission: off, ask (needs Task 133), or on     | Off by default; `ask` waits for Task 133                     |
| 148.16  | Default for OSC 52 clipboard write: active pane only, ask, or off             | Active pane only, capped at 1 MiB                            |
| 148.17  | Default for XTWINOPS window operations                                        | Off, as xterm's `allowWindowOps`; reports stay on            |
| 148.21  | Recordings: redact keystrokes while echo is off, or document only             | 0600 mode now; redaction only if echo state is cheap to read |
| 148.22  | Layouts that run `command` / `shell` / `env`: document as trusted, or confirm | Document as trusted input; confirm only if cheap             |

### 148 Subtasks

Every subtask leaves `cargo xtask test` and
`cargo clippy --all-targets --all-features -- -D warnings` green. Every fix has a regression
test that fails on the old code. Parser, buffer and handler subtasks name their benchmark
(`freminal-bench-table`) and record before and after results in their status note
(`performance-benchmarks`, 15% threshold). A subtask that adds or changes a `Config` field
follows `freminal-config-options` in full: `ConfigPartial`, `apply_partial`, the Settings
modal, `config_example.toml` and `nix/home-manager-module.nix`. Escape-sequence behaviour
changes are documented in 148.26. No subtask may weaken a limit to make a test pass.

Group A — Output-triggered crashes and unbounded allocation:

#### 148.1 — REP allocates its repeat count with no cap

- **Source:** advisory GHSA-2746-x3cj-58c7 (high).
- **Defect:** `handle_repeat_character` (`terminal_handler/edit_ops.rs`) builds a `Vec` of the
  CSI parameter's length before inserting, so one short sequence requests tens of gigabytes.
- **Scope:** clamp the count to what can still be visible, and insert without materialising the
  whole repeat. Check xterm's REP semantics in `ctlseqs.txt` and its source before choosing the
  clamp, and record the citation; do not infer them.
- **Deliverable:** a huge REP count completes in bounded time and memory; normal REP output is
  unchanged. Test at the handler and through the full parser.
- **Benchmark:** `bench_parse_plain_text`, `buffer_insert_chunks`.
- **Closes:** GHSA-2746-x3cj-58c7, when the fix is released.

#### 148.2 — Sixel raster attributes allocate unbounded images; `ensure_size` panics

- **Source:** advisory GHSA-w475-m9wf-775r (medium), including its 2026-10-18 addendum.
- **Defect:** `parse_sixel` (`freminal-common/src/buffer_states/sixel.rs`) feeds declared and
  grown dimensions to `SixelDecoder::ensure_size` with no bound, so a 33-byte DCS requests
  multiple gigabytes. `ensure_size` also panics on a slice index and overflows capacity when
  growing.
- **Scope:**
  - cap declared and grown dimensions before any allocation, against the image-store pixel
    quota (or one named constant if the decoder has no access to it);
  - validate dimensions and the old buffer size before copying rows, with checked arithmetic;
  - a capped image is drawn at the cap or rejected; pick one and record why.
- **Deliverable:** the `sixel_parse` regressions replay clean; a unit test covers each of the
  three failure shapes (oversize declaration, oversize growth, the slice-index panic).
- **Benchmark:** the sixel decode benchmark if one exists; otherwise add one first.
- **Closes:** GHSA-w475-m9wf-775r, when the fix is released.

#### 148.3 — Cursor can end up past the right margin; CBT panics

- **Source:** advisory GHSA-8f32-mj37-fw6v (high); the column half of #568 (the `:568`
  invariant, 70 of 793 findings).
- **Defect:** some escape-sequence paths leave `cursor.pos.x` greater than the width. `CSI Z`
  then slices `tab_stops[..col]` out of range in `tab_backward`
  (`freminal-buffer/src/buffer/tabs.rs`), which panics in release builds and kills the process.
- **Scope:**
  - find every path that produces the out-of-range column (the fuzz findings span sizes from
    2x1 to 80x24; minimise first, and expect more than one path);
  - fix the producers, not only the consumer;
  - also make `tab_backward` and `tab_forward` tolerate a stale column (defence in depth),
    since a resize can strand it too;
  - the `lifecycle.rs` invariant stays as the tripwire.
- **Deliverable:** the minimised reproducers replay clean; a unit test per producer found.
- **Benchmark:** `bench_cursor_ops`, `bench_move_cursor_relative`.
- **Closes:** GHSA-8f32-mj37-fw6v, when the fix is released.

#### 148.4 — BS in a 1-column terminal underflows

- **Source:** issue #557.
- **Defect:** `effective_x - 1` in `freminal-buffer/src/buffer/lines.rs` underflows when the
  width is 1 (debug panic; wraps to `usize::MAX` in release). A one-column pane is reachable
  from the GUI.
- **Scope:** saturate or return early at column 0, matching the margin-aware BS semantics;
  audit the neighbouring `- 1` sites in the same file for the same one-column assumption.
- **Deliverable:** a one-column regression test for BS; reproducer added to
  `fuzz/regressions/terminal_state/`.
- **Closes:** #557.

#### 148.5 — A glyph wider than the line makes wrap loop forever

- **Source:** issue #558 (hypothesis in the issue is unconfirmed).
- **Defect:** `Buffer::push_row`, via `advance_row_for_wrap` and `insert_text`, grows without
  bound in a one-column terminal until it requests a multi-gigabyte `Vec<Row>`.
- **Scope:** confirm the cause first (the issue's hypothesis is a double-width glyph that can
  never fit); define what happens to a glyph wider than the line (replace or drop), checking
  xterm and kitty rather than guessing; and guarantee wrap always makes progress.
- **Deliverable:** the reproducers replay clean; a unit test for a wide glyph in a one-column
  buffer; a note in the status entry on the chosen behaviour and its precedent.
- **Benchmark:** `softwrap_heavy`, `buffer_insert_large_line`.
- **Closes:** #558.

#### 148.6 — Incremental flatten diverges from the full-merge oracle

- **Source:** issue #559 (a `#[cfg(debug_assertions)]` oracle in `flatten.rs`).
- **Defect:** the incremental fast path (#405 Part C) produces a different hyperlink tag range
  than the full merge, so release builds silently render the incremental, wrong, result.
- **Scope:**
  - minimise the 1128-byte reproducer;
  - decide which result is correct and fix the other path;
  - keep the oracle, and add a property-style test that drives the same sequence and compares
    both paths.
- **Deliverable:** reproducer replays clean; a unit test for the minimised case.
- **Benchmark:** `bench_build_snapshot`, `bench_visible_flatten`,
  `bench_flatten_url_heavy`. The fast path exists for speed, so any fix must keep the
  `build_snapshot_80x24_partial_dirty` result.
- **Closes:** #559.

#### 148.7 — Buffer invariant violations (alternate rows, cursor row, image accounting)

- **Source:** issue #568 (the three remaining rows; the `:568` column row is 148.3).
- **Defect:** four `debug_assert_invariants` checks in `freminal-buffer/src/buffer/lifecycle.rs`
  fire under fuzzing: the alternate buffer not having exactly `height` rows (156 findings),
  `cursor.y` equal to the row count, `image_cell_count` disagreeing with the real count, and
  the parked alternate store not having `height` rows. Release builds do not panic; they carry
  a state the buffer's own rules forbid, which is a basis for later index panics.
- **Scope:** minimise each reproducer, find the producer, fix it. The four may share causes;
  record which do. A resize or an alternate-screen transition is the likely common ground.
- **Deliverable:** all four reproducers replay clean; a unit test per distinct root cause.
- **Benchmark:** `buffer_resize`, `bench_alternate_screen_switch`,
  `bench_alt_screen_transition_e2e`.
- **Closes:** #568, when 148.3 and this subtask are both done.

#### 148.8 — `.frec` parser requests multi-terabyte allocations

- **Source:** issue #556.
- **Defect:** `parse_recording_from_bytes` (`freminal-terminal-emulator/src/recording.rs`) takes
  counts and lengths from the file and uses them for `Vec::with_capacity` and as unchecked
  position arithmetic. Fuzzing reaches multi-terabyte requests, `capacity overflow` panics and
  OOMs. Recordings are only parsed by tests and tooling today, so this is robustness, not a
  remote vector.
- **Scope:** bound every count and length against the bytes actually remaining; use checked
  arithmetic for positions; a malformed file returns the existing typed error.
- **Deliverable:** the `frec_parse` regressions replay clean; a test per bound.
- **Closes:** #556.

Group B — PTY output reaches GUI-side state:

#### 148.9 — OSC 1338 `HISTFILE` makes freminal read any file

- **Source:** advisory GHSA-4whv-2cx6-772h (high).
- **Defect:** `HISTFILE=<path>` from terminal output is stored unvalidated and handed to a
  loader that does `std::fs::read` with no size cap and no regular-file check
  (`freminal/src/gui/shell_history.rs`). The basename allow-list only runs for shells other
  than bash, zsh and fish.
- **Scope:**
  - open the file without following symlinks, require a regular file, and cap the read;
  - always apply the basename allow-list, whatever the shell;
  - check the loader cannot hang on a FIFO (open non-blocking, or check the type first);
  - the cap is a named constant with a test.
- **Deliverable:** a device file, a FIFO, an oversized file and a non-history basename each
  produce no allocation and no history entries; a normal history file still loads.
- **Closes:** GHSA-4whv-2cx6-772h, when the fix is released.

#### 148.10 — The title stack is unbounded

- **Source:** advisory GHSA-jh4m-7w5g-j4qg (medium).
- **Defect:** `Vec<String>` in `freminal/src/gui/panes/mod.rs`, pushed from
  `freminal/src/gui/rendering.rs` for both inactive and active panes. Each push clones a title
  that can be up to 1 MiB.
- **Scope:** cap the depth (xterm uses 10; verify it); truncate titles at parse time to a
  sane length; the oldest entry is dropped on overflow, as xterm does.
- **Deliverable:** a test that thousands of pushes stay within the cap, for an active and a
  background pane.
- **Closes:** GHSA-jh4m-7w5g-j4qg, when the fix is released.

#### 148.11 — Terminal replies queue without bound

- **Source:** advisory GHSA-6cxq-f7hx-92v2 (low).
- **Defect:** terminal-generated replies share an unbounded channel with user input; the writer
  blocks when the child does not read its tty, so output made of queries amplifies memory 8–12x
  and freezes keystrokes.
- **Scope:** a separate bounded channel (drop or coalesce) for terminal-generated replies; cap
  the number of names answered per XTGETTCAP.
- **Constraint:** this touches the PTY-thread / writer boundary. Follow `freminal-architecture`;
  no locks, and the GUI thread stays a pure reader. Stop and report if the fix needs a design
  change to that boundary.
- **Deliverable:** a test with a writer that never drains: memory stays bounded, input still
  flows.
- **Benchmark:** `bench_handle_incoming_data`.
- **Closes:** GHSA-6cxq-f7hx-92v2, when the fix is released.

#### 148.12 — OSC 7 working directory is unauthenticated

- **Source:** advisory GHSA-8836-pm5j-jcp4 (low).
- **Defect:** the host is ignored, `%00` is decoded, and the path is used as-is for new tabs
  and panes and written to the auto-saved session.
- **Scope:** check the host against the local hostname (or accept empty and `localhost`);
  require an existing directory with no NUL or control characters; otherwise fall back to the
  process working directory (`/proc/<pid>/cwd` on Linux).
- **Deliverable:** table tests for host, NUL, non-directory and valid cases; a restored
  session never contains a rejected path.
- **Closes:** GHSA-8836-pm5j-jcp4, when the fix is released.

Group C — Kitty graphics file, shm and path handling:

All three live in `terminal_handler/graphics_kitty.rs`, so they run in order.

#### 148.13 — Kitty `t=s` shared memory: uncapped copy, SIGBUS, arbitrary unlink

- **Source:** advisory GHSA-j533-jhh2-ff2x (low).
- **Defect:** `read_kitty_shm_range` and `mmap_copy_range` map the whole object and copy
  `read_len` with no cap (the file path has a 400 MiB cap); truncation between `fstat` and the
  copy is a SIGBUS; `shm_unlink` runs on any readable object. The Windows path accepts a size
  up to 4 GiB.
- **Scope:** cap `read_len` at `MAX_KITTY_DATA_BYTES` on both platforms. On unix, replace the
  `mmap` copy with `pread` on the shm fd (looping on partial reads, treating a short read or
  EOF as an error): mapping only the requested range is NOT enough, because another process
  can still truncate the object while it is mapped and touching the removed pages raises
  SIGBUS (POSIX `mmap`). On Windows the `MapViewOfFile` path may stay: a section cannot shrink
  while a view is mapped. Restrict the unlink to the names the kitty convention produces, and
  record what that convention is.
- **Deliverable:** tests for an oversize request, a truncation (the object shrinks between
  `fstat` and the read: an error, never a signal) and a foreign name.
- **Closes:** GHSA-j533-jhh2-ff2x, when the fix is released.

#### 148.14 — Kitty `t=f` on Windows accepts UNC paths

- **Source:** advisory GHSA-3g79-vfp3-q57g (medium). Not yet reproduced on Windows.
- **Defect:** `Path::is_absolute` is true for a UNC path, and `fs::canonicalize` then contacts
  the host, leaking an NTLMv2 challenge-response. The refused-prefix and identity checks cover
  Unix only.
- **Scope:** on Windows, reject `\\`, `\\?\UNC` and `\\.\` prefixes and require a local
  drive-letter path, before any filesystem call.
- **Deliverable:** a pure path-classification function with table tests that run on every
  platform (so the Linux CI job covers it); `cargo xtask check-windows` is green.
- **Closes:** GHSA-3g79-vfp3-q57g, when the fix is released.

#### 148.15 — Kitty file/shm transmission has no consent gate and leaks through replies

- **Source:** advisory GHSA-8prj-mhmf-xpjg (medium).
- **Defect:** `t=f`, `t=t` and `t=s` read local files with no `[security]` switch. A load
  failure replies `EBADF`, but a successful read replies `OK`, a size mismatch or a PNG decode
  failure, so a remote party learns whether a path exists, its size and whether it is a PNG.
- **Scope:**
  - a `[security]` option for file and shm transmission, per the 148 open decision. Task 133
    (Shared Consent Prompt) is not built, so `ask` is not available in this task;
  - every post-read failure collapses into the same reply as a failed open;
  - the gate applies before any filesystem access, not after.
- **Deliverable:** with the option off, no read and a uniform reply; with it on, existing
  behaviour minus the distinguishable replies; the option follows the full config checklist.
- **Closes:** GHSA-8prj-mhmf-xpjg, when the fix is released.

Group D — Policy for features terminal output can drive:

These add `[security]` options, so they serialise on `freminal-common/src/config.rs`,
`config_example.toml`, the Settings modal and the Nix module.

#### 148.16 — OSC 52 clipboard write is ungated

- **Source:** advisory GHSA-v2hw-gcg8-2rfr (low).
- **Defect:** any pane, including a background tab, writes the clipboard from terminal output,
  and the OSC 52 payload cap is 64 MiB. `SecurityConfig` only has `allow_clipboard_read`.
- **Scope:** an `allow_clipboard_write` policy (per the open decision) and a cap near 1 MiB
  for OSC 52; a write from a background pane is dropped under the default. The paste-guard
  toast behaviour is unchanged.
- **Deliverable:** tests for each policy value, an inactive pane, and an oversize payload.
- **Closes:** GHSA-v2hw-gcg8-2rfr, when the fix is released.

#### 148.17 — XTWINOPS window operations are ungated; resize multiplies unchecked

- **Source:** advisory GHSA-67hw-cw29-r66f (low).
- **Defect:** minimise, move, resize, maximise and fullscreen run from terminal output
  (`freminal/src/gui/rendering.rs`); the resize path computes `input_width * font_width`
  unchecked (debug panic, release wrap); resize targets are not clamped.
- **Scope:**
  - a `[security] window_ops` policy (per the open decision); report-only operations stay
    available;
  - clamp resize targets to the monitor size;
  - checked or saturating arithmetic and `conv2` conversions
    (`freminal-numeric-conversions`).
- **Deliverable:** tests for each policy value and for overflowing inputs.
- **Closes:** GHSA-67hw-cw29-r66f, when the fix is released.

#### 148.18 — OSC 8 hyperlinks are opened with no scheme allow-list

- **Source:** advisory GHSA-jhw2-xjg6-mwhq (medium).
- **Defect:** `open::that` runs on any OSC 8 URI after a modifier-click or from the context
  menu (`freminal/src/gui/terminal/widget.rs`). Auto-detected URLs are restricted to
  http(s), file, ftp and mailto; OSC 8 is not.
- **Scope:** an allow-list (http, https, mailto, and local `file` as decided), applied at the
  click site so both entry points share it; reject non-URL strings and UNC paths; anything
  else is shown but not opened without confirmation. Prefer one shared predicate with the
  auto-detection rule.
- **Deliverable:** table tests for allowed, rejected and UNC inputs, with the predicate in a
  location both entry points call.
- **Closes:** GHSA-jhw2-xjg6-mwhq, when the fix is released.

Group E — Audit findings without an advisory:

#### 148.19 — OSC 9 / OSC 777 notifications are not markup-escaped

- **Source:** issue #560 (low; notifications are off by default).
- **Scope:** `build_system_notification` (`freminal/src/gui/notifications.rs`) reuses
  `escape_body_markup` (OSC 99 already does); the per-notification thread gets a bounded queue
  or rate limit.
- **Deliverable:** a test that markup in summary and body arrives escaped; a test that a burst
  does not spawn unbounded threads.
- **Closes:** #560.

#### 148.20 — Bracketed-paste end marker is not stripped

- **Source:** issue #561.
- **Defect:** `freminal/src/gui/actions.rs` wraps the payload verbatim, so clipboard text
  containing the end marker ends the paste early and the shell runs the remainder. The guard
  only flags this by default; "Paste Anyway" or a disabled `control_chars` check bypasses it.
- **Scope:** when bracketed paste is on, strip both markers from the payload, and C0 controls
  other than TAB, CR and LF if that matches the other terminals (check, do not infer). The
  S8C1T framing from 147.4 still wraps the stripped text.
- **Deliverable:** a test that a payload containing the marker produces exactly one start and
  one end marker.
- **Depends on:** 147.4 if it has not yet landed, since both edit the wrapping code.
- **Closes:** #561.

#### 148.21 — Recordings are world-readable and contain keystrokes

- **Source:** issue #563 (low; local and opt-in).
- **Scope:** create the file with mode 0600 on Unix (Windows ACL behaviour is documented, not
  changed); per the open decision, redact key events while echo is off, or document that
  recordings contain typed passwords. `FREC_FORMAT.md` and the `--recording-path` help text
  carry the warning either way.
- **Deliverable:** a Unix test on the created file's mode.
- **Closes:** #563.

#### 148.22 — Layouts with `command` / `shell` / `env` run without a trust prompt

- **Source:** issue #562 (informational, probably by design).
- **Scope:** per the open decision. If "trusted input": document it in `LAYOUT_FORMAT.md` and
  the user docs, with the reach (a hostile file in the layout library or a `--layout`
  argument; terminal output cannot write these fields). If "confirm": this needs Task 133 and
  is deferred to it, with the issue left open.
- **Deliverable:** the decision recorded here, plus its docs.
- **Closes:** #562, if documented; otherwise it stays open and routes to Task 133.

Group F — Hygiene:

#### 148.23 — Dead `CARGO_BUILD_FEATURES` in the dev shell

- **Source:** issue #564.
- **Scope:** delete the `CARGO_BUILD_FEATURES = "playback"` setting and its comment from
  `mkFreminalShell` in `flake.nix`. No crate declares a `playback` feature (removed with
  Task 59). `nixfmt`, `statix` and `deadnix` green.
- **Stop condition:** per `flake-dev-shell-discipline`, a `flake.nix` edit means the
  environment changes; stop and tell the maintainer to re-enter `nix develop`.
- **Closes:** #564.

#### 148.24 — Manifest hygiene

- **Source:** issue #566.
- **Scope:**
  - drop the normal `vergen` dependency from `freminal-terminal-emulator/Cargo.toml` (only
    `build.rs` uses it) and the `cargo-machete` ignore in the root `Cargo.toml`, if nothing
    else needs them;
  - resolve the nightly `package.homepage is redundant with package.repository` warning for
    every crate: point `homepage` at a real page or drop it.
- **Deliverable:** `cargo machete` green with the ignore removed;
  `nix develop .#fuzz --command bash -c 'cd fuzz && cargo check'` prints no manifest warnings.
- **Closes:** #566.

#### 148.25 — Fuzz targets and corpus follow-up

- **Scope:**
  - confirm each `fuzz/regressions/<target>/` reproducer added by 148.1–148.8 replays under
    `nix develop .#fuzz --command cargo xtask fuzz-replay`;
  - rerun each affected target for at least five minutes against the fixed tree; any new
    finding gets an issue and a numbered subtask here, not a silent fix;
  - update the `fuzz-smoke` comment in `.github/workflows/ci.yml` (it lists open crashes);
  - publish the corpus (`fuzz-corpus push`) only if the maintainer asks; that is a
    state-changing remote operation;
  - add libFuzzer dictionaries (`fuzz/dict/ansi.dict`, passed with `-dict=`) for
    `ansi_parser` and `terminal_state`: C0/C1 controls, CSI/OSC/DCS/APC/ST introducers,
    common parameters and private modes. xtask passes the dictionary when one exists for the
    target. Measure coverage after 20 minutes with and without, and record both;
  - expose a pure string-based config loader in `freminal-common` (for example
    `Config::from_toml_str(&str) -> Result<Config, ConfigError>`, running `ConfigPartial`,
    `apply_partial` and validation with no filesystem or environment access), with tests,
    and point `config_toml` at it so the fuzz target covers the real loading path;
  - retune `fuzz/nightly-plan.txt` from a week of nightly results: raise `sixel_parse` and
    `frec_parse` once 148.2 and 148.8 land, and shorten anything that has plateaued.
- **Deliverable:** the replay and the reruns are green; dictionary and retune measurements are
  recorded in the status notes.

#### 148.26 — Documentation

- **Scope:**
  - `ESCAPE_SEQUENCE_COVERAGE.md` and `ESCAPE_SEQUENCE_GAPS.md` for every behaviour change
    (REP clamp, sixel caps, BS, wide-glyph wrap, XTWINOPS and OSC 52 policy, OSC 8 scheme
    rule, OSC 1338, OSC 7, title stack depth, kitty file/shm gating), and both "Last updated"
    headers;
  - `config_example.toml` and `nix/home-manager-module.nix` for each new `[security]` option;
  - `.github/SECURITY.md` only if a policy statement changes;
  - `prettier` and `markdownlint` on the named files only.
- **Depends on:** the behaviour subtasks above.

#### 148.27 — Advisory follow-up (maintainer action)

- **Scope:** when a release contains the fixes, update each published advisory's patched
  version and request CVEs where warranted. These edit published external state, so they are
  the maintainer's, not an agent's: list each advisory with its fixing subtask and commit,
  then stop.
- **Deliverable:** a table in the status notes mapping each advisory to its fix.

### 148 Execution model

- **Group A** (148.1–148.8) touches the handler, the buffer, `freminal-common` and
  `recording.rs`. 148.1, 148.2 and 148.8 are file-disjoint and independent. 148.3, 148.4,
  148.5 and 148.7 all edit `freminal-buffer` and run in order: 148.4, then 148.3, then 148.5,
  then 148.7 (the broadest). 148.6 is `flatten.rs` and independent.
- **Group B** (148.9–148.12) lives in the GUI and `interface.rs`. 148.9, 148.10 and 148.12 are
  independent; 148.11 is the PTY writer boundary and runs alone.
- **Group C** (148.13 → 148.14 → 148.15) runs in order (one file).
- **Group D** (148.16, 148.17, 148.18) serialises on `config.rs` for the two config options
  (148.16, then 148.17); 148.18 adds no option and is independent.
- **Group E** (148.19–148.22) is independent per subtask, except that 148.20 waits for 147.4.
- **Group F** (148.23, 148.24) is independent of everything.
- 148.25, 148.26 and 148.27 run last.
- Groups A, B and C are file-disjoint with each other and can run in parallel worktrees per
  `parallel-work-isolation`. High-severity subtasks (148.1, 148.2, 148.3, 148.9) go first.
- Every subtask goes through the orchestrator's full-diff review. The task ends with an
  adversarial review, as for 130 and 131, because every subtask changes a trust boundary.

### 148 Verification

- `cargo xtask test`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo fmt --all -- --check`, `cargo machete`, `cargo bench --no-run --all`,
  `cargo xtask check-windows` (148.14 and 148.17 compile differently on Windows),
  `cargo xtask lint-markdown`.
- `nix develop .#fuzz --command cargo xtask fuzz-replay` is green with every new reproducer.
- Benchmarks named in 148.1–148.7 and 148.11 stay within the 15% threshold, or improve.
- Not covered by tests, and checked by the maintainer: the Windows UNC behaviour of 148.14 and
  the Windows size cap of 148.13, because neither runs in CI.

### 148 Status notes

- **148.1 (done):** REP clamps its count to 65535 and writes in 256-char chunks
  (`terminal_handler/edit_ops.rs`). Clamp citation: xterm-412 caps every CSI numeric
  parameter at `MAX_I_PARAM` = 65535 (`ptyx.h:406`, applied in `charproc.c` `CASE_ESC_DIGIT`)
  and `CASE_REP` prints the character through `dotext`, so xterm's REP maximum is 65535 and
  autowrap/scroll apply as for text. This deliberately clamps to xterm's limit rather than to
  the visible cell count: a visible-cell clamp would change the cursor column and scrollback
  for counts past one screen. Chunked `insert_text` is equivalent to one call (cursor,
  including pending wrap, is the only carried state); pinned by
  `tests/rep_repeat_count.rs` (autowrap on/off, DECSLRM, DECDWL, wide glyph, cap,
  `usize::MAX`, full parser). Reproducer `fuzz/regressions/terminal_state/rep-huge-count`
  fails on the old code (36 GB malloc) and replays clean. Benchmarks vs `main`:
  `bench_parse_plain_text/parser_push/4096` −3.5%, `buffer_insert_chunks/insert_chunks_1000/500`
  −2.0% (noise; REP is not on either path).
- **148.2 (done):** sixel decoder caps (`freminal-common/src/buffer_states/sixel.rs`):
  `MAX_SIXEL_DIMENSION` = 10000 per axis (kitty `MAX_IMAGE_DIMENSION`, `kitty/graphics.c:28`)
  and `MAX_SIXEL_PIXELS` = 4096x4096 (64 MiB RGBA, a named constant because `freminal-common`
  cannot see the image-store quota). Behaviour follows xterm-412 `graphics_sixel.c`: a raster
  declaration over the cap is a data error and ignored (~624-631); data beyond the cap is
  clipped, not rejected (~153). `!` repeats visit only on-canvas columns (O(width)); all size
  arithmetic is checked; the decoder keeps an allocation separate from the logical extent and
  grows geometrically, which also removed a quadratic re-copy (a 3.3 KB input took 17.9 s,
  now 92 ms). Output is byte-identical to `main` (4000-input comparison against a test-only
  copy of the old decoder, plus the palette-0-redefinition Paint case). Reproducers
  `oversize-raster-declaration`, `ensure-size-overflow-slice-panic`, `huge-repeat-growth`,
  `quadratic-growth` in `fuzz/regressions/sixel_parse/`; the old code fails replay. New
  benchmark `bench_parse_sixel/400x300` (added first): 262.9 µs -> 213.9 µs (−18.4%).
  **Review fix (R1):** the adversarial review found a remaining quadratic path at the pixel
  cap: once doubling would exceed `MAX_SIXEL_PIXELS`, the capacity was trimmed to the exact
  logical width, so every further column reallocated the whole canvas (3308 reallocations,
  30 s, from 5 KB). `grown_capacity` now splits the remaining pixel budget around the logical
  extent (`cap_w = isqrt(MAX * w / h)`), giving O(log) reallocations (19 on that trace; doc
  comment carries the argument). Reproducer `pixel-cap-boundary-regrowth`.
  `bench_parse_sixel` is still −15.9% against the pre-148.2 baseline.
- **148.4 (done):** BS from pending wrap in a one-column buffer now lands at column 0 instead
  of computing `0 - 1` (`buffer/lines.rs`). Semantics per xterm-412 `cursor.c` `CursorBack`
  (~147-169): with `do_wrap` set, reverse-wrap only consumes the pending wrap, otherwise the
  column is decremented and clamped to the left margin; at width 1 both give column 0 with
  no reverse wrap. Width >= 2 is unchanged. The same-file audit found one more underflow,
  `self.height - 1` in `handle_lf` on a zero-height buffer, fixed with `saturating_sub`; all
  other subtractions are guarded. Reproducer
  `fuzz/regressions/terminal_state/bs-one-column-underflow` (the #557 bytes).
- **148.3 (done):** one producer found. `Buffer::set_size` never revalidated the DECSLRM
  left/right margins, so after a narrowing resize `scroll_region_right` kept the old right
  edge; text written under DECLRMM wrapped past the buffer width and parked the cursor there,
  and a later CBT sliced the tab-stop table out of range. New
  `revalidate_left_right_margins` (`buffer/resize_and_alt.rs`): full-width margins follow the
  new width, margins that no longer fit reset to full width, a still-valid partial pair is
  kept (mirrors the existing top/bottom handling; xterm-412 `ScreenResize` resets margins
  unconditionally via `resetMargins`, and `set_lr_margins` clamps to `max_col`). This also
  fixes a widening resize leaving default margins at the old width. Defence in depth:
  `tab_backward` clamps the start column to the tab table and slices with `get`. Minimised
  46-byte reproducer `fuzz/regressions/terminal_state/cbt-stale-column-ghsa-8f32`; the
  3062-byte advisory input also replays clean. Two sandboxed 10-minute `terminal_state` runs
  on the fixed tree found no further column producers; their other findings (flatten oracle,
  alternate row count, cursor row, parked alternate store) are 148.6 / 148.7 material.
  Benchmarks (A/B on the same tree): `bench_cursor_ops` −0.2%, `bench_move_cursor_relative`
  −1.6%, `buffer_resize/shrink_height` +0.3%, `grow_height` −0.4%. A +20-27% delta on the
  two height benches against the session-start `main` baseline is environmental: reverting
  every buffer change since `main` gives the same numbers.
- **148.9 (done):** new `freminal/src/gui/history_file_reader.rs`: `read_history_file` opens
  with `O_NONBLOCK` (a FIFO never blocks) and, for the OSC 1338 path, `O_NOFOLLOW`
  (`SymlinkPolicy::Refuse`); `fstat` on the opened handle must be a regular file; reads are
  capped at `MAX_HISTORY_FILE_BYTES` (8 MiB), taking the newest tail from a line boundary.
  `load_from_path` (OSC 1338) now always applies the basename allow-list, whatever the
  shell; the list gains `.zhistory` (prezto default) and `.histfile` (zsh-newuser-install
  default). The environment-derived path (trusted) uses the same reader with
  `SymlinkPolicy::Follow`. User-visible consequence: an OSC-reported `HISTFILE` with any
  other basename, or a symlinked one, is no longer loaded over OSC; the environment path
  still is. Windows UNC paths on this path are 148.C1.
- **148.5 (done):** cause confirmed. `Row::insert_text_with_limit` returns
  `Leftover { leftover_start: 0 }` for a glyph wider than even a fresh row, and
  `Buffer::insert_text` wrapped again with no progress: a row pushed per pass on the primary
  screen (the #558 OOM), a scroll per pass on the alternate screen (a hang). Chosen
  behaviour: a glyph wider than the fresh-row span (`wrap_col - wrap_start_col` for that row,
  after DECDWL halving and DECSLRM margins) is discarded: not written, cursor not moved, no
  wrap; the following text continues. Precedent: kitty `screen.c` `screen_draw_text`
  ("discard too wide character") and xterm-412 `charproc.c` `dotext` ("give up - it is too
  big", against the DECDWL-halved right margin). This also changes DECDWL rows: a wide glyph
  on a DECDWL row with span 1 used to wrap and be written on the next row, and is now
  discarded as xterm does. Termination no longer depends on the span arithmetic:
  `wrap_unless_stalled` refuses a second wrap with no glyph placed and drops the head glyph,
  so one call wraps at most `text.len() + 1` times. Reproducers
  `wide-glyph-one-column-primary-oom` and `wide-glyph-one-column-alternate-hang` (6 and 14
  bytes). Benchmarks (A/B): `softwrap_heavy` +2.0%, `buffer_insert_large_line` +3.0%,
  `buffer_insert_chunks` +0.7%, all within threshold.
- **148.7 (done):** one producer for three of the four invariants. `advance_row_for_wrap`
  appended a row to the alternate screen when an auto-wrap happened on the last screen row
  below the DECSTBM bottom margin (`:595`, height + 1 rows); `?1049l` then parked that store
  (`:750`), and a later `?1049h` rebuilt `height` rows around the stale cursor row (`:554`).
  Fix: on the alternate screen's last row the wrap keeps the cursor on that row and the
  wrapped text overwrites it from the wrap column, as xterm-412 does (`WrapLine` ->
  `xtermIndex` -> `CursorDown`, which clamps to `max_row` below `bot_marg`). The
  `image_cell_count` case (`:643`, r3) no longer reproduces from 148.3 (`ac8ee7a5`) onwards:
  its stream relies on a stale DECSLRM margin. The invariants are unchanged. Reproducer
  `alt-wrap-below-margin-grows-screen` (57 bytes). A 664 s sandboxed run found none of the
  four classes; it found 123 flatten-oracle crashes (148.6) and one timeout (148.C2). The
  primary-screen counterpart is 148.C3. Benchmarks (A/B): `softwrap_heavy` +2.1%,
  `buffer_resize/*` −6.1% to +0.6%, `bench_alternate_screen_switch` and
  `bench_alt_screen_transition_e2e` not significant (one +95% outlier on
  `leave_alt_to_primary` did not repeat on two reruns: −0.03%, −1.8%).
- **148.6 (done):** the incremental path was wrong; the full merge is correct. Both causes are
  in wrapped auto-URL refinement in `refresh_row_cache_and_refine_wrapped_urls`
  (`buffer/flatten.rs`), not OSC 8 itself: (1) a clean wrapped-URL group below the first
  rebuilt row was skipped for redetection, so the tail re-merge fell back to its
  wrap-truncated per-row URLs; (2) a rebuilt soft-wrap continuation row (or a row a
  previous URL ran into) invalidated the refinement of rows before it, which the copied
  prefix kept. Fix: the first rebuilt row becomes a merge floor, pulled back to the start of
  the logical-line group when refinement can reach across it; a group is skipped only when it
  ends at or before the floor. Common case unchanged (floor = first rebuilt row, pinned by a
  test). The oracle stays; a 100-seed x 250-step property test in
  `freminal-terminal-emulator/tests/flatten_incremental_oracle.rs` compares both paths
  explicitly, so it also holds in release test builds. Reproducers
  `flatten-incremental-clean-wrapped-url-group-below-rebuilt-row` (minimised from #559) and
  `flatten-incremental-stale-url-before-rebuilt-continuation-row`. All 124 divergence
  artifacts from the 148.7 run pass; a 669 s run on the fixed tree found nothing in any
  class. Benchmarks (A/B): `build_snapshot_80x24_partial_dirty` −2.0% / −3.2% / −5.4% over
  three runs (wide intervals, not significant), `build_snapshot_80x24_clean` −7.9%,
  `bench_visible_flatten` +1.5%, `bench_flatten_url_heavy` 0.0%,
  `bench_flatten_wrapped_url_heavy` +1.6% (n.s.).
- **148.8 (done):** the only crash cause was the seek index: `Vec::with_capacity(entry_count)`
  from an unchecked u64 (the #556 allocation, the 155 GB OOMs and the `capacity overflow`
  panics; all 28,546 historical fork-mode artifacts classify as this). `parse_seek_index`
  now checks `entry_count * 16` against the bytes before the footer, with checked arithmetic,
  before allocating. All other offsets go through a checked `slice_at`, and
  `read_u32_le`/`read_u64_le` are fallible (hardening; the old comparisons already rejected
  those inputs on 64-bit). The msgpack hypothesis was refuted: rmp-serde 1.3.1 `from_slice`
  borrows str/bin/ext and fails on a declared length past the slice, and serde caps
  array/map preallocation; a test pins it. Reproducers `seek-index-huge-entry-count` and
  `seek-index-capacity-overflow` in `fuzz/regressions/frec_parse/`. A 300 s `frec_parse` run
  (26M execs) and all 36,388 historical artifacts are clean.
- **148.10 (done):** new `freminal/src/gui/title_stack.rs` `TitleStack`, depth 10, a push onto
  a full stack drops the oldest entry: xterm-412 `ptyx.h` `MAX_SAVED_TITLES 10` with
  `misc.c` `xtermPushTitle`'s ring, and kitty `window.py` `deque(maxlen=10)`. It replaces the
  `Vec<String>` on every pane, active or background. OSC 0/1/2 titles are truncated at parse
  time to `MAX_TITLE_BYTES` = 4096 at a UTF-8 boundary (`ansi_components/osc.rs`). This is a
  freminal resource cap; neither xterm nor kitty bounds titles below its OSC buffer. Tests:
  5000 pushes stay at depth 10 for an active and a background pane; a 1 MiB title becomes
  4096 bytes.
- **148.12 (done):** `parse_osc7_uri` now returns `Result<String, Osc7Rejection>`. The host must
  be empty, `localhost`, or the local hostname (`gethostname` / `COMPUTERNAME`, computed once),
  compared ASCII case-insensitively, with a short-name (first DNS label) match either way for
  macOS shells (RFC 8089 §2-3, RFC 4343). The decoded path must be valid UTF-8, absolute and
  free of control characters (so `%00` is rejected). A rejected URI clears the stored cwd. New
  `platform::resolve_spawn_cwd` trusts the OSC 7 path only if `is_dir()` holds, otherwise falls
  back to `read_cwd(child_pid)`; new tabs, splits and layout/session saving all use it, so a
  saved session never holds a rejected or nonexistent OSC 7 path. The suspected
  `percent_decode` off-by-one was not real; a trailing `%XX` test pins it. No layout-save
  harness exists, so the save path is covered through the shared helper's tests.
- **148.11 (done):** the PTY writer thread now reads two crossbeam channels: the existing
  unbounded one for user input (keystrokes, paste, resize, the GUI's `pty_write_tx`), and a new
  `bounded(REPLY_QUEUE_CAPACITY)` one for terminal-generated replies (256 here; raised to
  1024 by review fix R5, and given a 4 MiB byte budget by 148.C9).
  `run_writer_loop` (`io/pty.rs`) uses `select_biased!` with user input first, so keystrokes
  no longer queue behind replies. A reply and a keystroke pending at the same moment can now be
  written in either order, user first. No locks; the GUI is unchanged. The handler sends
  replies with `try_send`; on a full queue the reply is dropped and counted (`dropped_replies`,
  rate-limited warn). The DECCOLM `PtyWrite::Resize` is not a reply: review found it would
  block the PTY thread on a full queue, so a new `control_tx` routes it through the user
  channel. XTGETTCAP answers at most `MAX_XTGETTCAP_NAMES` = 64 names per request and never
  echoes a name longer than 128 hex characters (freminal policy; xterm-412 answers every
  name). `new_headless` keeps one unbounded channel for the fuzz harness. Tests: 10,000 DA1
  queries with no reader leave exactly `REPLY_QUEUE_CAPACITY` queued; a keystroke sent while the writer is blocked
  is written before every queued reply; a DECCOLM resize gets through a full reply queue.
  `bench_handle_incoming_data`: −0.5%.
- **148.13 (done):** `shm_read_bounds` now enforces `MAX_KITTY_DATA_BYTES` on both platforms
  before anything is mapped or read (`EFBIG`), whether the length comes from `S=` or from the
  object size. Unix reads with `FileExt::read_at` in a loop; a short read is
  `KittyShmError::Truncated` (`EIO`), never SIGBUS. The `mmap` path and all of its `unsafe`
  are gone (POSIX `mmap`: touching pages past the end of a truncated object raises SIGBUS, so
  mapping only the requested range would not help). Windows keeps `MapViewOfFile` under the
  cap. **Plan premise corrected:** kitty has no naming convention for `t=s`; `graphics.c`
  unlinks every object it opened, and the `tty-graphics-protocol` rule is for `t=t` only.
  Maintainer decision (2026-10-19): unlink as kitty does, but only when the opened object's
  `st_uid` equals our effective uid (`shm_unlink_decision`); a failed `fstat` no longer
  unlinks. Tests: cap table (every platform), a sparse cap+1 object, a deterministic
  shrink-after-size read returning `Truncated`, and the owner-decision table.
- **148.14 (done, with 148.C1):** new pure `freminal-common/src/windows_path.rs`.
  `classify_windows_path` returns a `WindowsPathKind` (`LocalDrive`, `Unc`, `DeviceOrVerbatim`,
  `DriveRelative`, `Relative`); it treats `\` and `/` alike and is table-tested on every
  platform. On Windows, `load_kitty_file` (`t=f` / `t=t`) and `read_history_file` (OSC 1338
  and the env HISTFILE path) refuse anything but a `X:\` local-drive path before any
  filesystem call, so no SMB connection (and no NTLM challenge-response) is attempted. The
  kitty reply stays the uniform `EBADF`. A drive letter mapped to a network share cannot be
  detected from the string and is out of scope. The Windows-only tests compile and pass
  `check-windows` clippy, but nothing runs them on Windows (maintainer check per
  "148 Verification").
- **148.15 (done):** maintainer decision (2026-10-19): **off by default**; "ask" waits for
  Task 133. New `[security] allow_kitty_file_transfer = false`, wired per
  `freminal-config-options`: field, Settings Security tab, `config_example.toml`, the Nix
  module, and tests. It reaches the PTY thread as
  `HostCapabilities::kitty_file_transmission` (`KittyFileTransmission::{Refused, Allowed}`),
  through the existing initial-state and `HostCapabilitiesChange` paths; Settings Apply
  rebroadcasts on change. The gate runs before any file or shm access for `t=f`, `t=t` and
  `t=s`: nothing is read, unlinked or deleted, and the reply is the uniform failed-open
  `EBADF`. When allowed, every failure on a file-backed medium (open, path or name refusal,
  bounds, too large, truncated, size mismatch, PNG or zlib failure, missing or zero
  dimensions, `a=f` rect overflow) maps to the same reply in one place
  (`send_kitty_medium_error`); `t=d` keeps its detailed errors.
- **148.16 (done):** maintainer decision (2026-10-19): **active pane only, 1 MiB**. New
  `[security] allow_clipboard_write = "active_pane" | "all" | "off"` (`ClipboardWritePolicy`),
  with the full config wiring (Settings combo box, `config_example.toml`, Nix enum option,
  tests). `handle_window_manipulation` writes only when `clipboard_write_permitted(policy,
  is_active)`, i.e. only from the active pane of the active tab by default; a dropped write
  shows no toast and is logged at debug with the byte count only. The decoded OSC 52 write
  cap is `MAX_OSC52_WRITE_BYTES` = 1 MiB (`osc_clipboard.rs`), and the parser gives OSC 52
  its own accumulation limit, `MAX_OSC_CLIPBOARD_BYTES` (the base64 size of the cap plus 256
  bytes, about 1.33 MiB). An over-limit OSC 52 is discarded as before but no longer buffers
  64 MiB; OSC 1337 keeps `MAX_OSC_LARGE_BYTES`. Query handling is unchanged.
- **148.17 (done):** maintainer decision (2026-10-19): **off by default**, as xterm's
  `allowWindowOps`. New `[security] allow_window_ops = false`, with the full config wiring.
  `WindowManipFlags.window_ops` (`WindowOpsPolicy`) is checked before any handling.
  `is_mutating_window_op` is an exhaustive `const fn`, so a new variant must be classified
  before it compiles. It gates CSI 1/2/3/4/8/9/10 t; reports, title set and title push/pop
  stay available. When allowed, CSI 8 uses `saturating_mul` and conv2. Resize targets are
  clamped to [1, monitor] and move targets to [0, monitor); the parameters are `usize`, so
  only the upper bound can overflow. When `monitor_size` is unknown, the bound is
  `FALLBACK_MONITOR_EXTENT` (16384 logical px); 148.C4 made `freminal-windowing`
  populate `monitor_size`.
- **148.18 (done):** new `freminal/src/gui/link_open.rs`. `classify_link` decides and
  `open_link` opens; both entry points use it (Ctrl/Cmd-click, and the context menu, whose
  "Open URL" entry is now disabled for a refused target). Allowed: `http`/`https`/`ftp` with
  `//`, `mailto`, and local `file` (empty or `localhost` authority, absolute path, no UNC
  form, backslash, `%5C`, or leading `/%2F`). This is the same scheme set auto-detection
  produces, so one rule covers OSC 8 and detected URLs. The scheme syntax follows RFC 3986
  §3.1; a one-letter scheme is treated as a drive letter. Everything else (`ms-msdt:`,
  `search-ms:`, `smb:`, bare paths, `\\host\share`) is refused with a warn naming only the
  reason. There is no confirmation dialog: confirming other schemes waits for Task 133, and
  refusing is the strict subset. Residual: a local `file:` link to a launcher (`.desktop`,
  `.command`) is still handed to the OS opener, as the plan's "local file" allows.
- **148.19 (done):** `build_system_notification` applies `escape_body_markup` (moved to the
  every-platform `gui/osc99/markup.rs` and re-exported, not duplicated) to the summary and the
  body. The per-notification thread is replaced by one lazily started `freminal-notify`
  worker fed by `bounded(SYSTEM_NOTIFICATION_QUEUE = 16)` in a `OnceLock`. `enqueue_bounded`
  returns `Enqueued | DroppedFull | WorkerGone`, and a burst drops notifications rather than
  spawning threads. The test-only skip of the OS call is unchanged.
- **148.20 (done):** done ahead of 147.4 (maintainer decision 2026-10-19; 147.4 rebases onto
  `wrap_bracketed_paste`). When bracketed paste is on, `sanitize_bracketed_paste` removes
  both markers (`200~` and `201~`) in their 7-bit `ESC [` and 8-bit U+009B forms, including
  markers reassembled around a removed one, in one linear pass (removal happens on the output
  buffer). Precedent: kitty `utils.py` `sanitize_for_bracketed_paste` strips the end marker to
  a fixpoint; stripping the start marker too is freminal's extension. Other C0 controls are
  not stripped: the terminals disagree (xterm-412 turns BS, DEL, ENQ, EOT, ESC, NUL and the
  stty specials into spaces by default via `DEF_DISALLOWED_PASTE_CONTROLS`; kitty leaves
  them), and the paste guard already flags them. Unbracketed paste is unchanged.
- **148.21 (done):** maintainer decision (2026-10-19): **0600 plus redaction**. On Unix,
  `create_recording_file` opens with `mode(0o600)` and tightens a pre-existing file before
  truncating it; non-regular targets such as `/dev/null` are left alone. Windows inherits the
  directory ACL (documented). While the pane's echo is off (the existing `echo_off` atomic),
  `redact_input_for_echo` replaces `KeyboardInput` (`key_name` becomes `"<redacted>"`,
  `encoded` is emptied), `ClipboardPaste` and `PtyInput` contents; event timing and modifiers
  are kept, and the FREC format is unchanged. All three emit sites use it (GUI via
  `RecordingContext::emit_input`, the PTY consumer thread directly). `FREC_FORMAT.md` gains a
  Security section and `--recording-path` help a warning. PTY output is still recorded in
  full. The GUI emit sites are covered through `emit_input`, not an egui harness.
- **148.22 (done):** maintainer decision (2026-10-19): **document as trusted input**.
  `LAYOUT_FORMAT.md` has a new "Security: layouts are trusted input" section: `command` is
  typed into the shell, `shell` picks the program, and `env` is exported, all without
  confirmation. It lists the reach (the layout library including `last_session.toml`,
  `--layout`, `[startup] layout`, the Layouts menu); terminal output cannot write these
  fields. A one-line warning was added to `README.md` and the `[startup] layout` comment in
  `config_example.toml`. The doc's stale auto-save section was corrected while there
  (`last_session.toml`, not `_last_session.toml`; `restore_last_session` defaults to
  `true`). #562 closes.
- **148.24 (done):** `vergen` is now only a build-dependency of `freminal-terminal-emulator`
  (its `build.rs` is the sole user). `freminal-buffer` had an unused `vergen`
  build-dependency and no `build.rs`, so it was dropped. The workspace cargo-machete ignore
  for `vergen` is gone and `cargo machete` is clean without it. `homepage` (identical to
  `repository`; nothing reads `CARGO_PKG_HOMEPAGE`) is removed from `[workspace.package]` and
  every member. The fuzz `cargo check` prints no manifest warnings.
- **148.25 (done, retune deferred to 148.C6):** all reproducers from 148.1–148.8 and 148.C2
  replay clean under `fuzz-replay`.
  - **Reruns:** each affected target ran 330 s with 8 jobs, keep-going, sandboxed, on the fixed
    tree. Execs: `terminal_state` 0.85M, `sixel_parse` 3.5M, `frec_parse` 198M,
    `ansi_parser` 17.9M, `config_toml` 67.5M, `kitty_graphics_parse` 545M. None found a crash,
    OOM or timeout. Earlier 10-minute `terminal_state` runs during 148.3, 148.6 and 148.7
    surfaced the findings fixed there and 148.C2.
  - **Config loader:** new `Config::from_toml_str` (`ConfigPartial` + `apply_partial` +
    `validate`, no filesystem or environment; parse errors are `ConfigError::ParseStr`). The
    `config_toml` target now fuzzes it, and the fuzz crate's unused `toml` dependency is gone.
  - **Dictionary:** `fuzz/dict/ansi.dict` (93 tokens: C0/C1, introducers, common CSI/OSC/DCS/APC
    forms, sixel and UTF-8 tokens). xtask's `dictionary_for_target` passes `-dict=` for
    `ansi_parser` and `terminal_state` (not for replay or cmin).
  - **Dictionary coverage:** 20-minute runs from the seeds only, `-fork=4`, with and without:

    | Target           | Dictionary | cov   | ft    | corpus |
    | ---------------- | ---------- | ----- | ----- | ------ |
    | `terminal_state` | with       | 15356 | 72015 | 8892   |
    | `terminal_state` | without    | 14576 | 65561 | 7470   |
    | `ansi_parser`    | with       | 4668  | 26402 | 6449   |
    | `ansi_parser`    | without    | 4725  | 27181 | 6707   |

    That is +5.4% cov and +9.9% ft for `terminal_state`; `ansi_parser` is −1.2% / −2.9% (one run
    each, so within likely noise; the seeds probably already carry most tokens). The mapping
    keeps both targets for now; 148.C6 revisits it with nightly data.
  - **CI comment:** the `fuzz-smoke` comment in `ci.yml` no longer lists the fixed crashes.
  - **Corpus:** not published (maintainer-only).
- **148.26 (done):** `ESCAPE_SEQUENCE_COVERAGE.md` rows updated for BS, DECDWL, REP,
  DECSLRM, XTWINOPS and the title stack, OSC 0/1/2, 7, 8 and 52, the OSC size caps, XTGETTCAP,
  sixel, kitty `_G` and `?2004`. OSC 1338 gained its first row. Both "Last updated" headers
  have a 2026-10-19 Task 148 entry; `GAPS` records that the new limits and off-by-default
  policies are deliberate, not gaps. CHT has no limits discussion, so it was skipped.
  `config_example.toml` and the Nix module were updated in their subtasks (148.15–148.17).
  `.github/SECURITY.md` makes no statement Task 148 falsifies, so it is unchanged.
- **148.27 (table ready; the rest is the maintainer's):** each published advisory and its fix
  on `task-148/security-remediation`. The hashes are branch commits; re-map them if the PR is
  squash- or rebase-merged. Once a release ships, the maintainer sets each advisory's patched
  version and requests CVEs where warranted. No agent edits advisories.

  | Advisory            | Severity | Fix    | Branch commit |
  | ------------------- | -------- | ------ | ------------- |
  | GHSA-2746-x3cj-58c7 | high     | 148.1  | `a1647497`    |
  | GHSA-w475-m9wf-775r | medium   | 148.2  | `68bc89b8`    |
  | GHSA-8f32-mj37-fw6v | high     | 148.3  | `ac8ee7a5`    |
  | GHSA-4whv-2cx6-772h | high     | 148.9  | `48880c72`    |
  | GHSA-jh4m-7w5g-j4qg | medium   | 148.10 | `c3f421ee`    |
  | GHSA-6cxq-f7hx-92v2 | low      | 148.11 | `bf950f58`    |
  | GHSA-8836-pm5j-jcp4 | low      | 148.12 | `37b46a8e`    |
  | GHSA-j533-jhh2-ff2x | low      | 148.13 | `9ff54e8b`    |
  | GHSA-3g79-vfp3-q57g | medium   | 148.14 | `b9a2388b`    |
  | GHSA-8prj-mhmf-xpjg | medium   | 148.15 | `ae490c87`    |
  | GHSA-v2hw-gcg8-2rfr | low      | 148.16 | `9d828944`    |
  | GHSA-67hw-cw29-r66f | low      | 148.17 | `58b202eb`    |
  | GHSA-jhw2-xjg6-mwhq | medium   | 148.18 | `32a09934`    |

  **Done (2026-10-20):** PR #572 merged as `1d9b7166` (merge commit, so the branch hashes
  above are on `main`). At the maintainer's direction all 13 advisories now read vulnerable
  `< 0.13.0-beta.2`, patched `0.13.0-beta.2`; CVE requests remain the maintainer's.
  Issues closed by the merge: #556 (148.8), #557 (148.4), #558 (148.5), #559 (148.6), #560
  (148.19), #561 (148.20), #562 (148.22), #563 (148.21), #564 (148.23), #566 (148.24), and #568
  (148.3 and 148.7). The windows-only checks for 148.13 (size cap) and 148.14 (UNC refusal)
  still need a maintainer run on Windows.
- **148.23 (done):** removed `CARGO_BUILD_FEATURES = "playback"` and its comment from
  `mkFreminalShell`, plus the fuzz-shell sentence about it (cargo never reads that variable,
  and no crate declares `playback`). xtask's `run_cargo_no_features` existed only to strip it
  and is gone; its callers use `run_cargo`. `nixfmt`, `statix` and `deadnix` are green, and
  the three dev shells evaluate. Per `flake-dev-shell-discipline`, the maintainer must
  re-enter `nix develop`; the variable had no effect on any tool, so later verification in
  the old shell remains valid.

- **Adversarial review (2026-10-19):** one CODE-REVIEW pass over the whole branch diff; the
  fixes are commits `148.R*`.
  - **R1:** the sixel quadratic at the pixel cap (a 148.2 note covers it).
  - **R2:** kitty `a=q` now answers the uniform `EBADF` for a refused file-backed medium,
    instead of claiming support.
  - **R5:** `REPLY_QUEUE_CAPACITY` is raised to 1024, and the docs now say the bound is in
    messages, not bytes. An OSC 52 query reply carries the clipboard when reads are allowed.
  - **R8:** `FREC_FORMAT.md` and `--recording-path` now state the redaction limits: raw-mode
    prompts are not detected, Windows is never redacted, and timing is kept.
  - **R9a:** OSC 7 no longer accepts a host whose first label equals the local name
    (`myhost.attacker.example`).
  - **R12:** `t=s` reads grow in 1 MiB chunks instead of zero-filling the full request.
  - **R14d:** the shm tests clean up through a drop guard.
  - **R3:** under `active_pane`, an OSC 52 write also needs the window focused (the docs
    already said so).
  - **R4:** `link_open` refuses `file:` launchers and executables (`LAUNCHER_EXTENSIONS` after
    percent-decoding, NTFS stream and trailing-dot tricks, plus the unix exec bit at click
    time via `decide_open`), and `mailto:` with an `attach`/`attachment` parameter.
  - **R6:** window-ops docs say the clamp is to the monitor when reported, else 16384 px.
  - **R10:** OSC 1338 requires an absolute `HISTFILE`.
  - **R11b:** the ZWNJ markup escape applies only to Linux/BSD notifications.
  - **R14a:** the FIFO tests fail loudly without `mkfifo`.
  - **R14b:** `AUTO_DETECT_SCHEMES` is exported from `freminal-buffer` (re-exported by the
    emulator), and `link_open` tests iterate it so scheme drift fails.
  - **R14c:** the window-op tests filter for the specific viewport commands.
  - **R7:** `insert_text` no longer wraps when nothing placeable remains after a full row
    (`wrap_would_place_text`), so a discarded glyph leaves no blank line. Benchmarks (A/B):
    `softwrap_heavy` −2.8%, `buffer_insert_large_line` −1.8%, `buffer_insert_chunks` 0.0%.
    The DECDWL-landing-row residual is 148.C8.
  - **Recorded rather than fixed:** 148.C8–148.C11 below. Notes-only items: R13 (sixel work
    is bounded by the DCS cap, inherent) and R14 dictionary (148.C6).

  The remaining review items are below.

- **Cleanup review (2026-10-19):** one CODE-REVIEW pass over the 148.C3/C4/C8/C9/C10/C11/C12
  commits. C3 and C12 match xterm; the C9 counters and the C11 respawn were judged sound.
  Fixed as RC1–RC10:
  - **RC1:** OSC 7 tests now use a per-platform URI (`sample_osc7`), or run the pure tables
    with `Osc7PathStyle::Posix`, so Windows CI passes after 148.C10.
  - **RC4:** a stalled wrap resumes from the end of the filled row again.
  - **RC5:** PRE-WRAP uses the same landing-row check as the Leftover wrap, sharing a new
    `take_wrap` helper. Two tests fail on the old code.
  - **RC10a:** a debug `eprintln!` is removed.
  - **RC2:** Viewport-class window commands are capped too (`MAX_OUTSTANDING_VIEWPORT_COMMANDS`
    = 4096 per pane, `WindowCommandBudget`). Under a flood against a stalled GUI, intermediate
    titles, clipboard writes and notifications past the cap are dropped (documented).
  - **RC8:** the first drop of either class warns and names the class.
  - **RC3:** at most `MAX_STUCK_PROBES` (4) directory-probe threads may remain stuck after
    their deadline. Past that, `with_deadline` returns `Skipped` at once without spawning or
    waiting, so a hung mount costs at most four 250 ms stalls and four threads.
  - **RC6:** monitor-size refreshes on `Moved` / `Resized` are debounced to 250 ms. A
    deferred refresh is retried on a later frame (`DeferredRefresh`), so a drag that ends
    inside the interval still updates.
  - **RC10b:** nit fixes (comment wrap, log wording, test classification).
  - **RC7, RC9:** doc fixes: the A14 wording, and the LF/IND/NEL and DECSLRM coverage rows for
    148.C3 and 148.C12.

- **PR #572 review (2026-10-19):** CI and the CodeRabbit / Copilot comments.
  - **RD1 (CI):** macOS shared-memory objects reject `read`/`pread` (`ESPIPE`), so 148.13
    broke `t=s` on macOS. macOS now reads through `mmap`, which is safe there because XNU fixes
    an object's size at its first `ftruncate` (`pshm_truncate`); the other unix platforms keep
    `pread`. A 148.15 edit had replaced the Windows `t=s` round-trip test's success check with
    a refusal check; restored.
  - **RD2:**
    - **Resize coalescing:** DECCOLM resizes coalesce through a latest-value slot
      (`PtyWrite::ApplyPendingResize`), so at most one marker is ever queued.
    - **GUI replies:** dropped once the pane's input queue already holds 1024 messages
      (keystrokes are never dropped).
    - **Window commands:** the budget also bounds payload bytes (16 MiB, both classes; OSC 99
      icons count).
    - **Sixel:** the stored pixels release spare capacity, which the image-store quota did not
      charge.
    - **Links:** the `file:` exec-bit `stat` moved to the opener thread; `LAUNCHER_EXTENSIONS`
      gains macOS and Windows run-on-open formats; the Ctrl/Cmd-click tooltip says when a link
      is blocked.
    - **Docs and tests:** `dropped_replies` docs, a misnamed `.frec` test, the 148.11 capacity
      numbers and the dictionary table.
  - **RD4:** the coalesced DECCOLM resize could reorder against the GUI's direct resizes (a
    marker queued before a GUI resize applied a later DECCOLM size, leaving the PTY and the
    emulator at different widths). All runtime resizes (`set_win_size`, `handle_resize_event`,
    DECCOLM) now go through `TerminalHandler::route_pty_resize` and the same latest-value
    slot, so the last size stored is the one applied. CodeRabbit's suggestion (clear the slot
    before a direct resize) still lets an earlier marker pick up a later DECCOLM size, so it was
    not used. A regression test fails on the previous code.

### 148 Cleanup entries

- **148.C1 — OSC 1338 `HISTFILE` on Windows accepts UNC paths.** Surfaced in review of 148.9.
  The bounded reader opens whatever path the shell reports; on Windows a `\\host\share\...`
  path with an allow-listed basename still makes the OS contact the host (the same NTLM
  leak as GHSA-3g79-vfp3-q57g for kitty `t=f`). Scope: once 148.14 lands its pure
  path-classification function, apply it to the OSC 1338 path before any filesystem call.
  Verification: a table test on the classification at the OSC entry point;
  `cargo xtask check-windows`. Schedule: with or right after 148.14.
  **Done** in the 148.14 commit.
- **148.C2 — CHT (`CSI Ps I`) loops `Ps` times with no progress check.** Surfaced by the
  148.7 fuzz run (a 3241-byte `terminal_state` timeout; top frames `Buffer::tab_forward` from
  `process_output_in_batch`). `tab_forward` calls `advance_to_next_tab_stop` `count` times
  even once the cursor sits on the last column, so a huge parameter is a CPU hang, the same
  family as GHSA-2746-x3cj-58c7. Scope: stop when a step does not move the cursor (or clamp
  `count` to the number of stops); check `tab_backward` and any other count-driven loop in
  `buffer/tabs.rs`. Verification: a test with `usize::MAX`; the minimised timeout as a
  `terminal_state` regression. Schedule: before 148.25's fuzz reruns.
  **Done:** `tab_forward` stops once a step leaves the column unchanged
  (`advance_to_next_tab_stop` only writes the cursor column, so later steps would be no-ops);
  `tab_backward` already runs at most `width + 1` times; no count-driven loop in the
  handler. The original timeout was `CSI 42222222222 I`; it now runs in 0.03 s. Reproducer
  `cht-huge-count-hang` (hand-crafted, `usize::MAX` count).
- **148.C3 — Wrap / LF below the bottom margin on the primary screen's last row scrolls.**
  Surfaced by 148.7. xterm-412 clamps there (`xtermIndex` -> `CursorDown` to `max_row`);
  freminal's primary buffer pushes a row. No invariant breaks, so this is a behaviour
  difference, not a crash. Scope: decide whether to match xterm; any change needs the
  scrollback consequences thought through. Not scheduled in Task 148.
  **Done (maintainer request, 2026-10-19):** the primary screen now matches xterm-412
  (`xtermIndex` -> `CursorDown`, which clamps to `max_row` below `bot_marg`; `WrapLine` uses
  the same path). Below the bottom margin on the last visible row, LF/IND/NEL and auto-wrap
  push no row and scroll nothing; a wrap overwrites that row from the wrap column. The new
  `is_cursor_on_last_screen_row` serves both screens. A full-screen region can't have the
  cursor below it, so ordinary scrollback is unchanged. Tests: 11 buffer tests and one
  full-pipeline test; the old `lf_outside_region_below_in_primary_creates_scrollback` test
  asserted the old behaviour and now asserts the xterm one. Benchmarks: `softwrap_heavy`,
  `buffer_insert_large_line` and the eviction benches are within ±4%. `bench_lf_heavy` is
  bimodal on this machine: the unchanged code measured 7.0 ms and 9.5 ms in back-to-back runs
  (−28% / +36% A/B swings, same code), so it gives no signal; the changed arm is off its
  path (full-screen region).
- **148.C4 — `viewport().monitor_size` is never populated.** Surfaced in review of 148.17.
  `freminal-windowing` calls `egui_winit::State::take_egui_input` but not
  `egui_winit::update_viewport_info`, so `monitor_size` (like other `ViewportInfo` fields
  eframe fills) is always `None`. XTWINOPS resize and move clamp to the 16384 px fallback
  rather than the real monitor, so an opted-in user (`allow_window_ops = true`) can still be
  moved off a smaller monitor. Scope: populate `monitor_size` from winit's
  `current_monitor()` in the windowing crate. Changing egui viewport state falls under
  `freminal-egui-upgrade` / `EGUI_UPGRADE_ASSUMPTIONS.md`, so walk that checklist. Not
  scheduled in Task 148 (the default is off).
  **Done (2026-10-19):** new `freminal-windowing/src/monitor_size.rs` caches each window's
  monitor size in logical points (`current_monitor()`, converted as egui-winit 0.36.2
  `update_viewport_info` does), refreshed at creation and on `Resized`, `Moved` and
  `ScaleFactorChanged`, never per frame. It is injected into
  `RawInput::viewports[id].monitor_size` after `take_egui_input`. Only that field is written;
  the full `update_viewport_info` is deliberately not called, since it would change title,
  rect and focus inputs the app relies on. `EGUI_UPGRADE_ASSUMPTIONS.md` A13 gains a note
  (scale-factor divisor); A14 (`RawInput::focused`) is untouched because only
  `monitor_size` is written, and a test pins that the other `ViewportInfo` fields stay unset. The 16384 px fallback now applies only
  when no monitor is reported.
- **148.C5 — `cargo xtask lint-markdown` fails on `main`.** Surfaced while verifying 148.21.
  Pre-existing MD060 / MD033 errors in `PLAN_122_ORCHESTRATION_EXTRACTION.md` and
  `PLAN_VERSION_090.md` fail the whole-repo markdown lint, which "148 Verification" lists.
  Files touched by Task 148 are linted individually and pass. Scope: fix those two documents
  (formatting only). Schedule: with 148.26.
  **Done:** formatting-only fixes in those two files and in `DECOUPLING_FRAMEWORK.md`,
  `PLAN_121_PERF_REMEDIATION.md`, `FUTURE_PLANS.md` (an MD033 disable around a real
  `<details>` block) and the `freminal-module-cohesion` skill; every tracked `.md` lints clean.
  A local `cargo xtask lint-markdown` can still fail on git-ignored `.direnv/` and
  `.opencode/node_modules/`, which CI does not have.
- **148.C6 — Retune `fuzz/nightly-plan.txt` from a week of nightly results.** Deferred from
  148.25 with maintainer agreement (2026-10-19): it needs a week of nightly data. Raise
  `sixel_parse` and `frec_parse` (unblocked by 148.2 and 148.8), shorten plateaued targets,
  and revisit the `ansi_parser` dictionary mapping (148.25 measured −1.2% cov in one run).
  Schedule: after a week of nightly runs on main with Task 148 merged.
  **Decision (2026-10-19):** after merge, raise `sixel_parse` and `frec_parse` and do the
  full retune from 3–5 nightly runs on `main` with these fixes (the two runs so far were on
  pre-fix `812b1550`, distorted by known crashes).
  **Blocked (2026-10-19):** the only cleanup entry not done in this PR. It needs a week of
  nightly fuzz results from `main` with these fixes merged, which cannot exist yet.
- **148.C7 — `cargo xtask ci` lint fails on `main` (typos).** Surfaced while verifying 148.23.
  `typos` flagged the terminfo reverse-index capability name (10 times in this document), `gam_sRGB` in
  `wide_gamut.rs`, and a test name with a misspelt `registry` in `osc99/dbus_service/mod.rs`; the
  `codespell:ignore` comments do not apply to typos. **Done:** `rin` and `gam` are added to
  `typos.toml` `extend-words` with their origin, and the test is renamed `..._registry_...`.
- **148.C8 — A wrap onto a DECDWL row can still be empty.** Review finding R7 (residual).
  `wrap_would_place_text` checks the widest fresh-row span, because the landing row's width is
  unknown before the wrap. If the landing row is double-width, so its span is half, a wrap
  can still land and place nothing, leaving one spurious blank line. It is reachable only
  with DECDWL and a glyph wider than the half span. Scope: compute the landing row's span
  before wrapping. Not scheduled in Task 148.
  **Done (2026-10-19):** `landing_row_span_for_wrap` predicts, without mutating, the row
  `advance_row_for_wrap` would land on and returns its span (DECDWL-halved). A narrow landing
  row is possible in three branches: the next existing row (`reuse_row_as_softwrap` keeps
  `line_width`), staying on the last row below the margin (148.C3), and a DECLRMM
  column-confined scroll on the primary screen. Pushed and freshly blanked rows are always
  single-width. A `debug_assert` in the new `wrap_after_leftover`, plus eleven per-branch unit
  tests, keep the prediction in sync with the real wrap; `wrap_unless_stalled` stays as the
  backstop. Benchmarks (A/B): `softwrap_heavy` +1.1%, `buffer_insert_large_line` −0.8%,
  `buffer_insert_chunks` −1.8%. Surfaced 148.C12.
- **148.C9 — Reply bound is per message, and GUI-side queues stay unbounded.** Review finding
  R5. `REPLY_QUEUE_CAPACITY` (1024) bounds messages, not bytes: with
  `allow_clipboard_read = true`, OSC 52 query replies carry the clipboard. Upstream of it,
  `window_cmd_tx` (PTY to GUI report requests) and `input_tx` (GUI replies back) are
  unbounded, so a non-draining GUI still amplifies. Scope: a byte budget (an atomic counter
  shared by handler and writer, no locks) and bounds on the two GUI channels, per
  `freminal-architecture`. Not scheduled in Task 148.
  **Done (2026-10-19):** atomics only, no new channels or locks.
  - **Byte budget:** `MAX_QUEUED_REPLY_BYTES` (4 MiB) is tracked in an `Arc<AtomicUsize>`
    shared by the handler and the writer (`ReplyQueue`). A reply that would exceed it is
    dropped unless the queue is empty, so one large legitimate clipboard reply still fits.
    The handler adds before `try_send` and refunds on failure; the writer subtracts after
    writing a reply.
  - **Report cap:** `MAX_OUTSTANDING_REPORTS` (256) per pane counts Report-class window
    commands sent to the GUI but not yet handled. `post_event` drops a report at the cap (the
    query goes unanswered, as with a dropped reply), and `handle_window_manipulation`
    releases one slot per drained Report before any branch, so refused ones release too.
  - **`input_tx`:** stays unbounded and documented: it carries keystrokes, and GUI replies on
    it are now bounded by the report cap.
  - **Tests:** 1000 `CSI 14 t` with no GUI draining leave exactly 256 reports; two 3 MiB
    replies, the second dropped; budget refunds and writer release.
  - **Benchmark:** `bench_handle_incoming_data` −2.7%.
- **148.C10 — OSC 7 / spawn-cwd follow-ups.** Review finding R9.
  - `resolve_spawn_cwd` runs `is_dir()` (and `read_cwd`, which uses `sysinfo` on macOS and
    Windows) on the GUI thread at new-tab or split time. An attacker-chosen path on a hung
    network mount can stall the GUI. Move it off-thread or add a deadline.
  - On Windows, an OSC 7 path (`/C:/...`) never passes `is_dir()`, so OSC 7 is silently
    unused there.
  - `last_session.toml` files written by older builds can carry an unvalidated cwd, and
    restore uses them as-is.

  **Done (2026-10-19):**
  - `resolve_spawn_cwd` runs on a helper thread under `SPAWN_CWD_DEADLINE` (250 ms,
    `with_deadline` -> `Done | TimedOut | Unavailable`). On timeout the pane starts in the
    default directory. A thread stuck on a hung mount is left to finish; this runs once per
    user action, or once per pane per minute for the session autosave.
  - `parse_osc7_uri` takes an `Osc7PathStyle`: on Windows, `/C:/x` becomes `C:/x` and anything
    but a local-drive path is rejected. Both styles are tested on Linux.
  - Layout and session restore check `directory` (no control characters, deadline-bounded
    `is_dir`) and fall back to the default directory instead of failing the pane
    (`LAYOUT_FORMAT.md` says so). An existing but hostile directory saved by an old build
    cannot be told apart; layouts are trusted input (148.22).
- **148.C11 — Notification worker is never restarted.** Review finding R11a. If the
  `freminal-notify` worker thread exits (panic), `OnceLock` keeps the dead sender and system
  notifications stay off until freminal restarts; each attempt logs `WorkerGone`. Scope:
  respawn on `WorkerGone`, which needs an atomically swappable sender (`ArcSwap`), not a
  `OnceLock`. Not scheduled in Task 148.
  **Done (2026-10-19):** the worker sender lives in a static `ArcSwapOption`. On
  `WorkerGone` (or empty) `enqueue_with_respawn` spawns a fresh worker, installs it with
  `compare_and_swap` (a racing winner's queue is used instead), and retries the enqueue once.
  Respawns are rate-limited to one per 5 s (`claim_respawn`, an atomic timestamp), so a
  panicking worker cannot become a thread per notification. No locks. Generic over the spawn
  closure and tested without the OS call.
- **148.C12 — Auto-wrap at the region bottom on the alternate screen ignores DECLRMM.**
  Surfaced by 148.C8. `scroll_region_up_for_wrap` scrolls whole rows on the alternate screen
  (`scroll_slice_up`), even with left/right margins set. The primary screen and SU use the
  column-confined `scroll_slice_up_confined`. xterm-412 `WrapLine` -> `xtermIndex` ->
  `xtermScroll` scrolls only inside the margins in left/right mode. Scope: use the confined
  scroll on the alternate screen when DECLRMM is on.
  **Done (2026-10-19):** with DECLRMM on and partial margins, the alternate screen uses
  `scroll_slice_up_confined`; full-width margins or DECLRMM off still scroll whole rows.
  `landing_row_span_for_wrap` predicts the cursor's own row in that case. Tests: buffer and
  full-pipeline margin-confinement tests (both fail on the old code) and both pins.

## Task 149 — zbus 6 Migration and Per-Platform Notification Backends

### 149 Summary

Close issue #567. `cargo deny check advisories` reports RUSTSEC-2026-0370 (`zvariant`),
RUSTSEC-2026-0371 (`zbus_names`) and RUSTSEC-2026-0372 (`zvariant_derive`) as unmaintained:
zbus 6.0.0 merged them into `zbus::wire`. The three are ignored in `deny.toml` until this task
lands. Created and decomposed on 2026-10-22 on branch `task-149/zbus-6`.

Two crates pull zbus 5 (`cargo tree -i zbus --target all --all-features`): freminal itself
(the OSC 99 D-Bus service, Task 138) and `notify-rust` 4.18.2. notify-rust's zbus 6 bump
(upstream PR #294) is unmerged, and dropping notify-rust alone would not clear the advisories,
because freminal's own `zbus = 5.19.0` pulls the same three crates. Waiting for notify-rust
would only save the Linux rerouting (149.2), so this task does not wait.

notify-rust is needed only for its zbus backend on Linux/BSD and its `mac-notification-sys`
backend on macOS. On Windows it wraps `tauri-winrt-notification`, which freminal already calls
directly for OSC 99 (Task 138.C15). On Linux/BSD it refuses to build without a D-Bus backend
(`compile_error!` in `xdg/mod.rs` unless `z` or `d` is on). So the end state is one native
backend per platform, used by every notification source (OSC 9, OSC 777, OSC 133 `D`
command-finished, the Settings "Test Notification" button, and OSC 99):

| Platform  | Backend                                                |
| --------- | ------------------------------------------------------ |
| Linux/BSD | freminal's own D-Bus service (`osc99/dbus_service/`)   |
| Windows   | `tauri-winrt-notification`, called directly            |
| macOS     | `notify-rust` (`mac-notification-sys`), macOS-only dep |

Out of scope: moving or renaming `osc99/dbus_service/` (see decisions), and any change to
which notifications are shown or when (routing, gates and focus rules are untouched).

### 149 Maintainer decisions (2026-10-22)

- **Migrate now**, without waiting for a zbus-6 notify-rust.
- **Windows goes direct to `tauri-winrt-notification`** for every notification source, for
  consistency with OSC 99.
- **notify-rust becomes a `cfg(target_os = "macos")` dependency** with
  `default-features = false`. Without `z`, its optional zbus 5 never enters the graph.

### 149 Orchestrator decisions (2026-10-22)

These were made at decomposition. Each is a deliberate behaviour choice, listed so review can
check the code against it.

- **Linux legacy notifications ride the existing dispatcher**, as a new fire-and-forget
  `DbusCommand` variant. They reuse its connection, circuit breaker, reconnect loop and daemon
  capabilities. A separate one-shot `wire.rs` path was rejected: it would duplicate the
  breaker and open a connection per notification, as notify-rust does today.
- **Legacy notifications are not tracked.** They have no registry entry, no `Shown` /
  `Closed` / `Activated` events and no actions (as today). When the service is reconnecting,
  a legacy command is dropped with a `debug` log, as `report_unserved` does for `Close`.
- **Linux legacy wire content changes in three small ways**, all toward what OSC 99 already
  sends:
  - the `desktop-entry` hint is now sent (the daemon can group by app);
  - the icon is sent through the dispatcher's existing image path (the `image-data` key,
    downscaled to at most 256 px) instead of a full-resolution `image_data` whose key notify-rust
    chose from the server's spec version;
  - the summary and body are escaped only when the daemon advertises `body-markup`
    (`state.rs`), instead of always. Both are escaped, as before, because some daemons format
    the summary too (#560). The escape is kept wherever the daemon would parse markup.

  Unchanged: app name `freminal`, app icon `freminal`, summary fallback, `Critical` urgency for
  `Error` and `Normal` otherwise, expire timeout `-1`, no actions.

- **Windows legacy toasts match freminal's OSC 99 toast, not notify-rust's.** That means
  `title(summary)`, `text1(body)` (left out for an empty body), silent, short duration, the
  PowerShell app id, and **no
  `Scenario::Reminder` for `Error`**. Task 138.C15 dropped the reminder scenario because a
  reminder needs buttons, and 138.C17 stopped advertising urgency on Windows for that reason.
  Today's `text1("")` / `text2(body)` layout is notify-rust's, not a freminal choice.
- **macOS is unchanged.** The worker thread and queue stay for macOS and Windows, but carry a
  freminal-owned payload instead of `notify_rust::Notification`, so the queue, respawn and
  policy tests compile on every platform.
- **`osc99/dbus_service/` keeps its path and names.** Moving it to a platform-neutral module
  would make the diff mostly renames and hide the port. The module docs gain a paragraph
  stating that the service also carries OSC 9 / 777 / 133 `D` notifications on Linux/BSD.
- **Threading model under zbus 6:** keep the dispatcher thread and the pump threads. Each
  zbus call becomes `zbus::block_on(...)`. Each pump becomes one
  `zbus::block_on(async { while let Some(m) = stream.next().await { ... } })` loop. This is
  safe because the pumps forward over **unbounded** crossbeam channels (`mod.rs:606`), so a
  pump never blocks a thread inside `block_on`. 149.1 must confirm the multi-thread driver
  hand-off in `zruntime` source and cite it in its status note. If that check fails, stop and
  return to the orchestrator; collapsing into one async loop is a redesign, not a fallback.

### 149 Subtasks

Every subtask leaves `cargo xtask test` and
`cargo clippy --all-targets --all-features -- -D warnings` green, and runs
`cargo xtask check-windows`. macOS cannot be cross-checked locally (`mac-notification-sys`
compiles Objective-C), so macOS-gated code is kept minimal and is verified by the macOS CI
job. No benchmarks: nothing here is on a `freminal-bench-table` path.

#### 149.1 — Port the D-Bus service to zbus 6

- **Defect:** `osc99/dbus_service/` is written against zbus 5's blocking API
  (`zbus::blocking::{Connection, MessageIterator}`), which zbus 6 removes. It also uses
  `zvariant`, which is now `zbus::wire` and pulls the unmaintained crates.
- **Scope:** `Cargo.toml` (workspace `zbus` entry: `6.0.0`, `default-features = false`,
  features `default-rt` and `proxy`, comment updated), `freminal/Cargo.toml` (the dev-only
  `p2p` feature), and every file under `freminal/src/gui/osc99/dbus_service/`
  (`mod.rs`, `wire.rs`, `pump.rs`, `state.rs`, `test_rig.rs`, `breaker_tests.rs`, `wake.rs`
  if touched). notify-rust is untouched, so zbus 5 stays in the lock file through it until
  149.4.
  - Blocking calls become async calls under `zbus::block_on`. The threading model is as
    decided above.
  - `MessageIterator` becomes `MessageStream`. `MatchRule::builder()` setters lose their `?`,
    and `build()` gains one.
  - `MethodFlags::NoAutoStart.into()` becomes `MethodFlags::NO_AUTO_START`. `Proxy::new`
    takes the async connection.
  - `zvariant` paths move to `zbus::{Type, Signature, wire, as_variant}`; `Value` /
    `OwnedValue` become `Variant` / `OwnedVariant`. No deprecated `zbus::zvariant` alias may
    remain.
  - **Error classification (behaviour-critical).** zbus 6 converts every error reply through
    `fdo::Error`, so `org.freedesktop.DBus.Error.NameHasNoOwner` and `ServiceUnknown` now
    arrive as `zbus::Error::FDO(..)`, not `MethodError`. `is_method_error` / `is_no_daemon`
    must match the `FDO` variants, or "no daemon" silently becomes a warning. Method timeouts
    still surface as `Error::InputOutput` of kind `TimedOut` (`runtime/timeout.rs`), but only
    from `Connection::call_method`, so `call_no_auto_start` keeps its own deadline.
  - `Connection::close` is async. The test rig's p2p connection uses
    `connection::Builder::unix_stream(std::os::unix::net::UnixStream)`.
- **Deliverable:** the service behaves exactly as before. Every existing test passes, ported
  but not weakened: no deleted assertions, no loosened timeouts. A new test proves that a
  `NameHasNoOwner` and a `ServiceUnknown` reply are each classified as "no daemon" under
  zbus 6.

#### 149.2 — Route Linux/BSD legacy notifications through the D-Bus service

- **Scope:** `freminal/src/gui/notifications.rs`, `freminal/src/gui/osc99/dbus_service/`
  (`mod.rs`, `state.rs`, `wire.rs`), `freminal/src/gui/osc99/mod.rs` (re-exports),
  `freminal/src/gui/osc99/glue.rs` and `host_capabilities.rs` (service start gating), and the
  three callers: `app_impl.rs` (OSC 9 / 777), `frame_drain.rs` (OSC 133 `D`) and
  `settings_dispatch.rs` (Test Notification).
  - Add `DbusCommand::ShowPlain { notification: Box<PlainNotification> }`.
    `PlainNotification` carries `summary`, `body` and `urgency` and lives in `dbus_service`.
  - Add a pure builder `DispatcherState::plain_notify_args(&PlainNotification) -> NotifyArgs`
    beside `notify_args`. It has no actions, `replaces_id` 0, expire `-1` and the embedded
    freminal icon. Escaping of summary and body follows `body-markup`.
  - The dispatcher sends it under `daemon_gate` / `note_call`, without `record_shown` or any
    `Osc99DbusEvent`. `report_unserved` drops it with a `debug` log.
  - Add a cloneable `PlainNotifySender` (wraps the command `Sender`). `Osc99DbusHandle`
    exposes it, and the three callers receive it explicitly. **No new process-wide static.**
  - The service starts when either OSC 99 may use the system leg (as now) or any legacy
    category's routing can use the system leg. Add a helper beside `osc99_may_use_system_leg`
    and test it.
  - On Linux/BSD, `NotificationRouter::show_system` hands the request to the sender and no
    longer touches notify-rust or the worker. On macOS and Windows it is unchanged in this
    subtask.
- **Deliverable:** with the fake daemon in `test_rig.rs`, an OSC 9 / command-finished request
  produces one `Notify` call whose hints and arguments match the decisions above, including
  `Critical` urgency for `Error`, no actions, and escaping only when the daemon advertises
  `body-markup`. Further tests: a legacy command while reconnecting is dropped and never
  answered; a legacy show while the breaker is open does not call `Notify`; the start-gating
  helper's truth table.

#### 149.3 — Windows legacy notifications go direct to `tauri-winrt-notification`

- **Scope:** `freminal/src/gui/notifications.rs` and `freminal/src/gui/osc99/untracked.rs`
  (only to share the toast-building helper, if one is extracted).
  - The worker queue's payload becomes a freminal-owned `SystemNotification` (summary and
    body; no kind, which neither platform reads) instead of `notify_rust::Notification`. On
    Windows the worker builds a `Toast` exactly as decided above. On macOS it builds the same `notify_rust::Notification` as today
    (appname, icon, summary, body; no urgency).
  - The queue, respawn and policy tests stay platform-neutral. Tests that inspected a
    `notify_rust::Notification` are replaced by tests of a pure mapping from request to
    platform fields that compiles everywhere. Assertions are kept: summary fallback, the #560
    escaping rule and urgency by kind. Only the type they inspect changes.
  - In `untracked.rs`, the `build_macos_notification` tests that compile on Linux through
    `cfg(any(target_os = "macos", test))` are re-gated or rewritten the same way, so nothing
    outside macOS names `notify_rust`.
- **Deliverable:** `notify_rust` is referenced only under `cfg(target_os = "macos")`. Windows
  OSC 9 / 777 / 133 `D` toasts are built by the same path style as OSC 99.
  `cargo xtask check-windows` is clean.

#### 149.4 — Make notify-rust macOS-only and drop the advisory ignores

- **Scope:** `Cargo.toml`, `freminal/Cargo.toml`, `Cargo.lock`, `deny.toml`, and the
  documentation that names notify-rust or zbus for these paths:
  - `Documents/ESCAPE_SEQUENCE_COVERAGE.md`
  - `Documents/ESCAPE_SEQUENCE_GAPS.md`
  - `Documents/KITTY_PROTOCOL_REFERENCE.md`
  - `Documents/DESIGN_DECISIONS.md`
  - stale comments in `main.rs` (the `zbus=warn` filter stays)
  - the workspace comments at the `notify-rust` and `zbus` entries

  The changes:

  - notify-rust moves to `[target.'cfg(target_os = "macos")'.dependencies]` with
    `default-features = false` and no features (`images_no_default_features` and `z` go).
  - Remove the three RUSTSEC ignores.

- **Deliverable:** `cargo tree --target all --all-features -i zvariant` finds nothing, and the
  lock file has no zbus 5, `zvariant`, `zbus_names` or `zvariant_derive`.
  `cargo deny check` passes with no ignores for #567. `cargo machete` is clean.

### 149 Execution model

Strictly sequential: 149.1, then 149.2, then 149.3, then 149.4. 149.2 and 149.3 both edit
`notifications.rs`, and 149.2 depends on 149.1's port. Each subtask is one IMPLEMENTATION
sub-agent pass followed by the orchestrator's full-diff review against this section. Then
comes the verification suite, then a commit. When 149.4 is green, an adversarial CODE-REVIEW
pass covers the whole `main...HEAD` diff. Every finding, nit through blocker, is remediated
as a numbered `149.Cn` cleanup entry before the PR opens.

### 149 Verification

- `cargo xtask test`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo fmt --all -- --check`, `cargo machete`, `cargo deny check`,
  `cargo xtask check-windows`, `cargo xtask lint-markdown`.
- Not covered by tests, and checked by the maintainer before merge: a real Linux desktop shows
  OSC 9, command-finished, Test Notification and OSC 99 notifications with the freminal icon;
  a Windows toast for OSC 9 and the Test Notification button, and an `Error`-kind toast that
  is now an ordinary toast rather than a reminder; macOS unchanged (CI build plus a
  manual check).

### 149 Status notes

- **149.1 (done):** zbus `6.0.0` with `default-rt` and `proxy` (`p2p` for tests). The
  blocking calls are `zbus::block_on` around the async API; pumps loop over `MessageStream`
  inside one `block_on`. Threading check: zruntime 0.2.0 `driver.rs` gives each thread its own
  runtime with a driver "seat", parks a second `block_on` until it is woken, and runs work that
  outlives every `block_on` on a helper thread (`driver.rs` module docs, `Driving::take`,
  `OWN`). A connection's tasks run on the dispatcher's runtime. No thread blocks inside a
  `block_on`: pumps only send on unbounded channels. `NameHasNoOwner` and `ServiceUnknown`
  now arrive as `Error::FDO` (`error.rs`, `From<Message>`) and are matched there. A real
  fake-daemon reply test pins it.
- **149.2 (done):** `DbusCommand::ShowPlain` and `PlainNotifySender` live in
  `dbus_service/plain.rs`, and `plain_notify_args` is in `state.rs`.
  - Callers get a `SystemNotifySink` from `FreminalGui::system_notify_sink`.
  - `PlainNotifyEntry::{Routed, SettingsTest}` gates the service start. The Test button
    ignores `enabled`, as `route_test` does.
  - **Deviation from the decisions above, accepted:** the summary is escaped as well as the
    body, under the same `body-markup` condition, because the legacy path escaped both
    (#560: some daemons format the summary).
  - The app icon PNG is one `static` (`gui::run::FREMINAL_ICON_PNG`).
  - `drain_command_finished_events` takes `&TabTitleConfig` instead of two fields, to stay
    under the argument limit.
  - End-to-end tests run `NotificationRouter::route` / `route_test` into the fake daemon.
- **149.3 (done):** the macOS/Windows worker carries a freminal-owned `SystemNotification`
  (summary, body; no kind, as neither platform reads one).
  - **Windows:** `windows_toast` builds the OSC 99-style toast: title, `text1` (skipped when
    the body is empty), silent, short, no scenario.
  - **macOS:** `macos_notification` keeps today's content.
  - **OSC 99 macOS builder:** in `untracked.rs` it is split into a pure
    `macos_notification_fields` (tested everywhere) and a thin macOS-only conversion.
  - `notify_rust` now appears only under `cfg(target_os = "macos")`. Its macOS APIs were
    checked against notify-rust 4.18.2's source: every field and method used is public on
    macOS. That code is not compiled locally; the macOS CI job compiles it.
- **149.4 (done):** `notify-rust` 4.18.2 is a `cfg(target_os = "macos")` dependency with
  `default-features = false`. The three RUSTSEC ignores are gone (`ignore = []`).
  - `cargo tree --target all --all-features -i zvariant` (and `zbus_names`,
    `zvariant_derive`) matches nothing.
  - zbus 6.0.0 is the only zbus, used by freminal alone.
  - `cargo deny check` passes all four sections. The lock file drops zbus 5, zbus_macros 5,
    zbus_names, zvariant, zvariant_derive, zvariant_utils, async-broadcast, async-recursion
    and enumflags2.
  - Docs: `ESCAPE_SEQUENCE_COVERAGE.md` (a new "Legacy notifications" note),
    `KITTY_PROTOCOL_REFERENCE.md` and `DESIGN_DECISIONS.md`.

### 149 Cleanup entries

From the adversarial review of 2026-10-22. Two parallel CODE-REVIEW passes covered the
`main...HEAD` diff: one on the zbus port (Z1–Z14) and one on routing, platforms, dependencies
and docs (R1–R14). Neither found a blocker. Every finding is fixed here or scheduled into
Task 150 (structural ones; see "150 Evidence").

- **149.C1 — Plain notifications share the OSC 99 command queue (R1, Z2; major).** Before,
  OSC 9 / 777 / 133 `D` had their own 16-slot queue. Now a loop on OSC 9 can queue 256
  `Notify` calls and starve OSC 99 `Show` / `Close`.
  - Fix: `PlainNotifySender` shares an in-flight counter capped at 16
    (`PLAIN_NOTIFICATION_BUDGET`). It is incremented on send and decremented when the
    dispatcher handles or drops the command.
  - Test: a full plain budget still lets an OSC 99 `Show` queue.
  - Docs: `COMMAND_QUEUE_CAPACITY`, `plain.rs` and `ESCAPE_SEQUENCE_COVERAGE.md`.
- **149.C2 — The `method_timeout` → `is_method_timeout` path is never exercised (Z1;
  major).** Every breaker test feeds a synthetic timeout error. Fix: a rig variant whose
  client sets a short `method_timeout`, and a fake-daemon summary that is never answered.
  - Tests: `notify` and `close_notification` fail with `is_method_timeout`; at service level,
    a hung `Notify` gives `ShowFailed`, then the gate skips.
- **149.C3 — zbus 6 logs nothing (Z3).** Without its `tracing` feature, the `zbus=warn`
  filters in `main.rs` are dead. Fix: enable zbus's `tracing` feature and keep the filters.
- **149.C4 — `smol` is a second async stack kept for three small uses (Z4).** Fix: depend
  on `async-io` (`Timer`) and `futures-lite` (`StreamExt`, `FutureExt::or`) directly, and
  drop `smol`. Reword the comment: async-io drives its timers on its own process-wide thread.
- **149.C5 — `serve` does not close the connection when the GUI is gone (Z5).** Fix: call
  `close_connection` before returning `GuiGone`, so the pumps end, and shorten the shutdown
  doc.
- **149.C6 — Test-only error-name constants in production `mod.rs` (Z6).** Fix: move them
  into `test_rig.rs`.
- **149.C7 — The service imports the app icon from `gui::run` (R12, Z7).** Fix: a small
  `gui::app_icon` module owns `FREMINAL_ICON_PNG`; `run.rs` and the service import it.
- **149.C8 — Logging (R2, Z9).**
  - Service-wide log lines no longer say "OSC 99:" on paths that serve every notification.
  - A busy handle cell in `system_notify_sink` logs at debug instead of dropping silently.
  - A plain notification dropped because the service is unavailable warns once, then logs
    at debug.
  - `ESCAPE_SEQUENCE_COVERAGE.md` records that notifications are dropped during the up-to-30 s
    reconnect window. Before this task, each notification made its own connection.
- **149.C9 — The service-start gate is untested as a whole (R3).** Fix: extract
  `notification_service_wanted(&Config)` in `host_capabilities.rs` and use it from
  `ensure_osc99_dbus_started`.
  - Truth-table test, including `Config::default()`.
  - A test that ties `plain_notifications_may_use_system_leg` to
    `NotificationRouter::routing_for` for every `NotificationKind`, so a new kind cannot be
    missed.
- **149.C10 — The plain-path escape test lost the richer #560 input (R4).** Fix: restore the
  `<a href=…>` input with per-character ZWNJ checks.
  - Record in `ESCAPE_SEQUENCE_COVERAGE.md` that escaping follows the daemon's `body-markup`
    capability. A daemon that formats text without advertising `body-markup` gets raw text.
    This is accepted, because OSC 99's body already works this way.
- **149.C11 — `MASTER_PLAN.md` mentions that stop at 148 (R5).** Fix: the dependency notes
  and the reference list include 149 and 150.
- **149.C12 — Stale comments and docs (R6, R14, Z10, Z11).**
  - Code comments:
    - `app_impl.rs` service-start comment;
    - the `osc99_dbus` field doc in `gui/mod.rs`;
    - the "thread is best-effort" test comment in `notifications.rs`;
    - the zbus / Linux-section comments in both `Cargo.toml` files;
    - the zruntime seat-hand-off sentence in `dbus_service/mod.rs` (each thread drives its own
      runtime; the socket reader runs on the dispatcher's or the helper thread);
    - "held for one routing call" (it is held for the rest of `update()`).
  - `DESIGN_DECISIONS.md` wording.
- **149.C13 — Plan text contradicts the shipped code (R7).** Fix: the 149 decisions and
  subtask text are corrected:
  - summary and body are both escaped;
  - the worker payload has no `kind`;
  - a Windows toast skips `text1` for an empty body.

  The deviation notes stay.
- **149.C14 — Unrelated reformatting of Task 148 status notes (R8).** It came from
  `prettier --write` on the whole file; `main`'s copy already fails prettier, and the hook
  skips markdown. Fix: revert those hunks.
- **149.C15 — The notification sink is built every frame (R9).** Fix: build it only when
  `drain_command_finished_events` has a notification to route. Take a shared borrow when the
  service is already running.
- **149.C16 — A test with no assertions (Z8).** Fix:
  `sending_to_a_full_queue_or_a_gone_dispatcher_does_not_block_or_panic` asserts the queue
  length and the dropped extra send.
- **149.C17 — A plain `Notify` never adopts the reply's sender as the daemon owner (Z13).**
  With bus tracking and a failed `GetNameOwner`, plain-only use never refreshes the
  capabilities. Fix: `show_plain` adopts the owner as `record_shown` does, with a test.
- **Status (2026-10-22):** C1–C17 are done in commits `7b304012` (C11, C13, C14),
  `ed8db385` (C1, C2, C5, C6, C10, C16, C17 and the service side of C8 and C12) and
  `d6fe5bbe` (C3, C4, C7, C9, C15 and the GUI and docs side of C8, C10 and C12).
  - A confirmation review verified every entry and found no blocker or major. Its five
    follow-ups are done in the next commit:
    - the shutdown doc's sender-lifetime sentence;
    - `routing_for` is private again, and its tie-in test moved to `notifications.rs`;
    - a tested `DropLogLevel` for the warn-once drop log;
    - `resolve_system_notify_sink`, with tests for the busy, running, unwanted and spawn
      cases;
    - an implicit slot drop.
  - Enabling zbus's `tracing` feature moved `tokio` 1.53.2 → 1.53.3 in the lock file and
    added `mio` / `wasi` entries. No target builds them (`cargo tree -e normal,build -i tokio`
    is empty on Linux, Windows, macOS and FreeBSD).
- **Scheduled into Task 150, not fixed here:**
  - R10 / Z14 (`let _ = self;` in the non-Linux `system_notify_sink`);
  - R11 (the repeated platform `cfg`);
  - R13 (the untestable Windows toast builder; the dropped `Error` reminder scenario is added
    to the maintainer's manual checks below);
  - Z12 (the plain icon is cloned, 256 KiB, per notification; the OSC 99 path does the same,
    and the icon pipeline moves in Task 150).
- **Merged** 2026-10-22 via PR #575 (`fd3c7ede`).

## Task 150 — Desktop Notification Backend Extraction

### 150 Summary

Give the "show this notification on the OS" side of freminal one owner. Created as an
enriched stub on 2026-10-22, from the Task 149 adversarial review and a maintainer
discussion. Activated and decomposed on 2026-10-22 on branch
`task-150/notification-backend-extraction`, from `main` at `fd3c7ede` (after PR #575).

After Task 149, every system notification has exactly one native backend per platform, but
the backends have no home of their own. They are spread across:

- `freminal/src/gui/notifications.rs` (3,488 lines). It holds routing, plus the macOS/Windows
  `freminal-notify` worker, its bounded queue and respawn policy, and the per-platform
  builders.
- `freminal/src/gui/osc99/` (about 18,000 lines). It holds the OSC 99 registry and protocol
  logic, mixed with the Linux/BSD D-Bus service (`dbus_service/`). That service also carries
  OSC 9 / 777 / 133 `D` since Task 149.2. The same directory holds the macOS/Windows
  untracked backend (`untracked.rs`) and icon handling (`icon_decode.rs`, `icon_raster.rs`,
  `icon_tempfiles.rs`, `icon_lookup.rs`, `presentation.rs`, `markup.rs`).

Each backend was added where its first caller happened to be. That is the
`freminal-extend-or-extract` trigger: the concept has no owner. The size of the files is only
a symptom.

### 150 Evidence (Task 149 adversarial review, 2026-10-22)

Every one of these is a symptom of the missing owner. 149 fixed only what was a defect of its
own change; the structural part is this task's.

- **Shared queue.** Plain notifications (OSC 9 / 777 / 133 `D`) were put on the OSC 99
  command queue, so a flood of one could starve the other. 149 added a separate cap (see the
  149 cleanup entries). A backend that owns its queues would have made the budget explicit.
- **Wrong names.** The service is named and logged as "OSC 99" (`Osc99DbusService`,
  `Osc99DbusHandle`, `osc99_dbus` field, "OSC 99: …" log lines) although it carries every
  Linux/BSD notification. 149 decided not to rename it, to keep the port reviewable.
- **Repeated platform `cfg`s.** In `notifications.rs` the expression
  `cfg(any(test, not(all(unix, not(target_os = "macos")))))` is repeated about 20 times around
  the worker, queue and respawn machinery.
- **Platform-shaped call sites.** `FreminalGui::system_notify_sink` has a non-Linux
  `const fn` that does `let _ = self;` only so every platform has the same call site.
  `SystemNotifySink` is a struct whose field exists on one platform only.
- **Shared assets.** The app icon PNG was first put in `gui/run.rs` and imported by the D-Bus
  service (149.C7 moved it to `gui::app_icon`). The plain icon is also cloned (256 KiB) per
  notification, as the OSC 99 path clones its icons (review finding Z12). The icon pipeline
  should share decoded images (`Arc`).
- **Untestable builders.** The Windows toast builder cannot be unit-tested
  (`tauri-winrt-notification`'s `Toast` has no getters). The macOS builders only compile on
  macOS CI. 149.3 split the OSC 99 macOS builder into pure fields plus a thin conversion. The
  same pattern is wanted for every platform builder.

### 150 Shape (from the stub; confirmed at activation, 2026-10-22)

- **One interface, two request kinds.**
  - **Plain** (fire-and-forget): summary, body, urgency. Used by OSC 9, OSC 777,
    command-finished and the Settings Test button.
  - **Tracked:** an instance id, with `Shown` / `ShowFailed` / `Closed` / `Activated` /
    capability events coming back. Used by the OSC 99 registry.

  Callers never name a platform. The backend owns the OS connection, worker threads, queue
  budgets (per request kind), icon decoding and temp files, and markup escaping.
- **Two steps (maintainer decision, 2026-10-22):**
  1. **A module first.** `freminal/src/gui/desktop_notify/`, designed as if
     it were a crate: an explicit public surface, nothing reaching into its internals, and no
     behaviour change. The OSC 99 protocol logic (registry, filter, pane state, effects,
     glue) stays in `osc99/` and becomes a caller.
  2. **Then a mechanical move into a `freminal-notify` crate.** The maintainer approved the
     crate in principle on 2026-10-22 and confirmed it at activation, as the final subtask
     of this task (same branch and PR). The crate may depend on `freminal-common` only
     (`freminal-architecture`):
     - `icon_raster.rs` reads the bundled font through `crate::gui::font_manager`, so it
       must take the font bytes as input instead;
     - toast / glyph types from `crate::gui::icons` / `crate::gui::toast` must stay on the
       caller's side.

  The point of the crate is that zbus, `tauri-winrt-notification`, `notify-rust` and their
  platform `cfg`s leave the `freminal` crate entirely.
- **Wake-up integration.** The D-Bus service wakes idle windows through a
  `Box<dyn Fn() + Send>` waker (now `NotifyWake`, in the GUI). That is already crate-safe;
  keep it.

### 150 Scope limits

- No behaviour change in either step: notification content, routing, gates, `p=?` answers
  and capability broadcasts stay identical, and the existing tests prove it.
- Out of scope: new notification features, and any change to which notifications are shown
  or when.

### 150 Activation recon (2026-10-22)

Two read-only passes mapped every backend file's imports, its public items and their outside
users, and every caller. Findings:

- **Most of the backend already needs only `freminal-common`.** Everything under
  `osc99/dbus_service/` except `wake.rs`, plus `icon_decode.rs`, `icon_tempfiles.rs`,
  `icon_lookup.rs`, `presentation.rs`, `markup.rs` and `capabilities.rs`, imports only
  `freminal_common`, external crates and sibling backend files. No backend file imports
  `freminal_terminal_emulator`.
- **The upward dependencies to invert are four:**
  - `dbus_service/wake.rs` uses `freminal_windowing::{RepaintProxy, WindowId}`. It is the GUI's
    adapter. The service itself only takes `Box<dyn Fn() + Send>`
    (`Osc99DbusService::spawn`), so `wake.rs` stays in the GUI.
  - `icon_raster.rs` reads the font through `crate::gui::font_manager::bundled_regular_font_bytes`
    and takes a `crate::gui::icons::ChromeIcon`. `icons.rs` imports `egui`.
  - `untracked.rs` uses `ChromeIcon` (as a `HashMap` key) and `crate::gui::icons::standard_icon_glyph`.
  - `dbus_service/state.rs` uses `crate::gui::app_icon::FREMINAL_ICON_PNG`.
- **Interface types live on the GUI side today.** `Osc99InstanceId` (private `u64`, read as
  `.0` by `state.rs` and `effects.rs`), `Osc99SystemBackend` / `Osc99SystemDispatch` /
  `NoSystemBackend` / `RecordingSystemBackend` (`osc99/effects.rs`) and
  `Osc99SystemAction::to_leg_activation` (which returns the registry's `Osc99LegActivation`).
- **Two tables of the eight standard icon names:** `presentation::standard_icon_name` (the
  freedesktop names) and `icons::standard_icon_glyph` (the glyphs).
- **Two macOS/Windows delivery workers with different lifecycles:**
  - `freminal-notify` (`notifications.rs`, plain): a process-wide `ArcSwapOption` static,
    started lazily and restarted at most every 5 s if it dies. It has no shutdown.
  - `freminal-osc99-deliver` (`untracked.rs`, OSC 99): owned by the GUI and joined (bounded
    wait) on drop, which also removes its icon temp files. If it has exited, the request
    that finds it gone is dropped and the next one spawns a new worker, with no rate limit.
    (Corrected at PR #576 review: this line first said "never restarted".)

  Each has its own enqueue outcome type (`EnqueueOutcome`, `Submit`).
- **Platform `cfg` reaches callers:**
  - `FreminalGui` fields `osc99_dbus` and `osc99_wake`;
  - wake register/retarget/unregister and `ensure_osc99_dbus_started` in `app_impl.rs` and
    `settings_dispatch.rs`;
  - about 15 Linux-only items in `osc99/glue.rs`, including two copies of
    `apply_and_feed_back` and of `system_notify_sink`;
  - 34 attribute gates in `notifications.rs` production code, 16 of them the
    `cfg(any(test, not(all(unix, not(target_os = "macos")))))` expression;
  - a `cfg!(test)` early return in the macOS/Windows `show_system`;
  - a `cfg!` leg choice in `route_osc99`.
- **Z12 confirmed.** `plain_notify_args` decodes the app icon once (`OnceLock`) but clones the
  decoded RGBA `Vec` (256 KiB) into every notification.
- **Tests that cross the boundary:**
  - four router-to-fake-daemon tests in `dbus_service/plain.rs` import
    `gui::notifications` and `gui::toast`;
  - `glue.rs` tests use `dbus_service::{detached_handle, DetachedQueue}`;
  - `registry.rs`, `notifications.rs` and `glue.rs` tests use the recording backends.
- The flake builds from `cleanSource ./.`, so a new crate directory needs no flake change.
  The workspace `members` / `default-members` lists do.

### 150 Decisions (maintainer, 2026-10-22)

1. **Names.**
   - Module `freminal/src/gui/desktop_notify/`; crate `freminal-notify`.
   - Backend-neutral names replace the `Osc99` ones on items that serve every notification
     (table in 150.6).
   - Thread names `freminal-osc99-*` become `freminal-notify-*`.
   - `"OSC 99:"` log prefixes on paths that serve every notification become
     `"desktop notifications:"`.
   - OSC 99 registry and protocol types keep their names.
2. **Two macOS/Windows workers under one owner.** Both move into the backend. Each keeps its
   lifecycle exactly (plain: static plus respawn; OSC 99: owned plus shutdown join), so a
   slow macOS delivery for one kind cannot stall the other. Only the duplicated bounded-enqueue
   outcome type is merged.
3. **The on-disk icon directory prefix stays `freminal-osc99-icons-`.** The startup sweep then
   keeps cleaning directories left by earlier builds. The code constant is renamed; its
   value is not.
4. **The crate move is the final subtask of this task**, on the same branch and PR.
5. **The tracked payload stays `&Notification99Data`.** It is in `freminal-common`, the only
   crate the backend may depend on.

### 150 Decisions (orchestrator, recorded so they are not re-litigated)

- **Already settled by the code:**
  - Markup escaping (#560) belongs to the backend. It already lives in `state.rs` and depends
    on the daemon's `body-markup` capability.
  - The icon pipeline (decode, raster, temp files, lookup, presentation) moves whole.
  - The OSC 99 `g=` cache (`icon_cache.rs`) is protocol state and stays in `osc99/`.
- **Boundary.** `desktop_notify` contains the OS backends (Linux D-Bus service; the
  macOS/Windows OSC 99 worker and plain worker), the icon pipeline, markup escaping, daemon
  capability types and the backend interface types. These stay in the GUI as callers:
  - OSC 99 registry, glue, effects, filter, pane state and toast spec;
  - routing (`notifications.rs`);
  - capability policy (`host_capabilities.rs`);
  - the wake adapter.
- **One surface.** Outside code names only items re-exported from `desktop_notify/mod.rs`.
  No path such as `desktop_notify::linux::state::…` appears outside the module. Test doubles
  and the fake-daemon rig are re-exported from `desktop_notify::test_support`
  (`#[cfg(test)]`). It becomes a `test-support` feature at the crate step.
- **Assets are injected.** `NotifyAssets { app_icon_png: &'static [u8], icon_font: &'static [u8] }`
  is built once by the GUI and passed to every backend constructor. The `include_bytes!`
  calls stay in the GUI.
- **Standard icon names have one table.** A backend-side `StandardIcon` enum (the eight names)
  carries the kitty name, the freedesktop name and the glyph codepoint. `ChromeIcon` gains
  `From<StandardIcon>`. The GUI's `standard_icon_glyph` and the backend's
  `standard_icon_name` both read it.
- **Interface types move to the backend** as plain data:
  - `Osc99InstanceId`, with `const fn new(u64)` and `const fn get(self)`;
  - the tracked and untracked backend traits, the dispatch result and the no-op and
    recording doubles;
  - the system action enum.

  The GUI-only conversions (`to_toast_key` / `from_toast_key`, action to
  `Osc99LegActivation`) become GUI-side functions or `From` impls in `osc99/`.
- **The wake adapter** (`dbus_service/wake.rs`) moves to `gui/notify_wake.rs`, outside the
  backend. In 150.7 it is made to compile on every platform, so its callers lose their
  `cfg`.
- **Moves are pure moves.** Renames happen only in 150.6, and behaviour changes only where a
  subtask says so. Each move is verified with a sorted-line-multiset diff and an unchanged
  test count.

### 150 Execution model

Serial, on one checkout (`task-150/notification-backend-extraction`): the subtasks move the
same files, so worktrees would only add merge work. Each subtask runs as follows:

1. A sub-agent implements it.
2. The orchestrator reviews the whole diff against the subtask text, line by line, including
   every new file in full.
3. The orchestrator reruns the verification suite.
4. The orchestrator commits.

A sub-agent that ends without a report is treated as unverified.

Order: 150.1, 150.2, 150.3, 150.4a, 150.4b, 150.5, 150.6, 150.7a, 150.7b, 150.8, 150.9,
150.10. After 150.10 an adversarial CODE-REVIEW pass covers `main...HEAD`. Every finding, nit
through blocker, becomes a numbered `150.Cn` entry and is fixed before the PR opens, followed
by a confirmation review.

No benchmarks: nothing here is on a `freminal-bench-table` path.

### 150 Subtasks

Every subtask's verification is `cargo xtask test`,
`cargo clippy --all-targets --all-features -- -D warnings` and `cargo xtask check-windows`.
Every subtask touches platform-gated code; macOS-only code is checked by the macOS CI job.

Every subtask's prohibitions include:

- do NOT touch files outside scope;
- do NOT commit;
- do NOT update plan documents;
- do NOT proceed to the next subtask;
- no behaviour change unless the subtask names one;
- no `unwrap`/`expect` outside tests, no raw numeric `as` casts, no new `bool` fields or
  parameters, no `#[allow(dead_code)]`;
- no deleted or weakened test assertion (tests may only move or change paths and names).

The stop condition is the same for all: report files changed, the test count before and
after, and verification results; then await review.

#### 150.1 — The module, and one standard-icon table

Scope:

- `freminal/src/gui/desktop_notify/mod.rs` (new: module docs stating the boundary and the
  one-surface rule, plus the declarations below);
- `freminal/src/gui/desktop_notify/standard_icon.rs` (new);
- `freminal/src/gui/mod.rs` (module declaration);
- `freminal/src/gui/icons.rs` (`standard_icon_glyph`, `From<StandardIcon> for ChromeIcon`);
- `freminal/src/gui/osc99/presentation.rs` (`standard_icon_name`);
- `freminal/src/gui/osc99/icon_raster.rs` and `osc99/untracked.rs` (take `StandardIcon`);
- `freminal/src/gui/osc99/toast_spec.rs` (only if `standard_icon_glyph`'s signature changes).

What:

- `pub enum StandardIcon { Error, Warning, Info, Question, Help, FileManager, SystemMonitor, TextEditor }`
  with these methods:
  - `from_name(&str) -> Option<Self>`, accepting the kitty names, including both `warn` and
    `warning`;
  - `freedesktop_name(self) -> &'static str`;
  - `codepoint(self) -> char`.

  The values are exactly today's: the freedesktop names from `standard_icon_name`, the
  codepoints from the matching `ChromeIcon` variants.
- `standard_icon_name` and `standard_icon_glyph` become one-line delegations.
- `rasterize_chrome_icon_png` and the untracked `IconPreparer` key on `StandardIcon`, not
  `ChromeIcon`. Rename the function `rasterize_standard_icon_png`.
- **Pin first:** before changing the two tables, add tests asserting today's full mapping for
  every name (name to freedesktop name, name to `ChromeIcon`). The tests must pass both before
  and after.
- Also add a test that `ChromeIcon::from(icon).codepoint() == icon.codepoint()` for all eight.

Deliverable: the enum, the delegations, the pin tests. `icon_raster.rs` and `untracked.rs` no
longer import `crate::gui::icons`.

#### 150.2 — Inject the backend's assets

Scope:

- `freminal/src/gui/desktop_notify/assets.rs` (new) and `desktop_notify/mod.rs`;
- `freminal/src/gui/font_manager.rs` (accessor gate only);
- `freminal/src/gui/osc99/icon_raster.rs`, `osc99/untracked.rs`;
- `freminal/src/gui/osc99/dbus_service/{mod.rs,state.rs,plain.rs,test_rig.rs}`;
- `freminal/src/gui/osc99/glue.rs` (`spawn_osc99_dbus`);
- `freminal/src/gui/mod.rs` (where `platform_untracked_backend()` is built).

What:

- Add `#[derive(Clone, Copy)] pub struct NotifyAssets { pub app_icon_png: &'static [u8], pub icon_font: &'static [u8] }`.
- The GUI builds it once, from `app_icon::FREMINAL_ICON_PNG` and
  `font_manager::bundled_regular_font_bytes()`. Ungate that accessor (today
  `cfg(any(macos, windows, test))`); the static it returns already exists on every platform.
- `Osc99DbusService::spawn(waker, assets)` passes it on to `DispatcherState`.
- `platform_untracked_backend(assets)` and `rasterize_standard_icon_png(font, icon, size)`
  receive it.
- Test constructors use a test helper that builds `NotifyAssets` from the same statics.
- Backend files no longer import `crate::gui::app_icon` or `crate::gui::font_manager`.

Deliverable: the injection, with output unchanged. The existing icon, raster and `Notify`
content tests pass unchanged.

#### 150.3 — Share decoded icons instead of cloning them (Z12)

Scope: `freminal/src/gui/osc99/presentation.rs` (`Osc99IconImage`, `Osc99DecodedIconCache`),
`freminal/src/gui/osc99/dbus_service/{state.rs,wire.rs,plain.rs}` and their tests.

What:

- The decoded RGBA moves into an `Arc<[u8]>` (or the image into an `Arc`), so the plain icon
  `OnceLock` and the decoded-icon cache hand out reference-counted handles.
- `wire.rs` keeps serialising the bytes borrowed, so the bytes on the wire are unchanged.

Deliverable:

- the change;
- a test that two consecutive `plain_notify_args` calls share one allocation (`Arc::ptr_eq`
  or equal data pointers);
- a test that a cached transmitted icon is not copied per notification;
- the existing wire-format tests, unchanged.

#### 150.4a — Move the leaf backend files (pure move)

Scope: these files move with `git mv`, and the `use` paths of their users are updated:

| From                                        | To                                     |
| ------------------------------------------- | -------------------------------------- |
| `osc99/icon_decode.rs`                      | `desktop_notify/icons/decode.rs`       |
| `osc99/icon_raster.rs`                      | `desktop_notify/icons/raster.rs`       |
| `osc99/icon_tempfiles.rs`                   | `desktop_notify/icons/tempfiles.rs`    |
| `osc99/icon_lookup.rs`                      | `desktop_notify/icons/lookup.rs`       |
| `osc99/presentation.rs`                     | `desktop_notify/presentation.rs`       |
| `osc99/markup.rs`                           | `desktop_notify/markup.rs`             |
| `osc99/capabilities.rs`                     | `desktop_notify/capabilities.rs`       |
| `osc99/untracked.rs`                        | `desktop_notify/untracked.rs`          |

The users to update are `osc99/mod.rs`, `osc99/{effects,glue,registry,toast_spec}.rs`,
`osc99/dbus_service/*`, `notifications.rs`, `host_capabilities.rs` and `gui/mod.rs`.

What:

- Every `cfg` gate travels with its module declaration.
- Outside users import through `desktop_notify::` re-exports only. Visibility within the
  directory is the narrowest that compiles.
- The test doubles (`RecordingUntrackedBackend`, `NoUntrackedBackend`) are reached through
  `desktop_notify::test_support` where they are test-only.
- No item is renamed.

Deliverable:

- the move;
- a sorted-line-multiset diff of the moved files showing only `use` paths, visibility
  prefixes and `rustfmt` re-wraps;
- an identical test count.

#### 150.4b — Move the D-Bus service and the interface types (pure move)

Scope:

- `osc99/dbus_service/{mod,plain,pump,state,wire,test_rig,breaker_tests}.rs` move to
  `desktop_notify/linux/` with the same names;
- `osc99/dbus_service/wake.rs` moves to `gui/notify_wake.rs`;
- `Osc99InstanceId` (from `osc99/mod.rs`), and `Osc99SystemBackend`, `Osc99SystemDispatch`,
  `NoSystemBackend`, `RecordingSystemBackend` and `RecordedShow` (from `osc99/effects.rs`),
  move to `desktop_notify/backend.rs`;
- the `use` sites in `osc99/*`, `notifications.rs`, `host_capabilities.rs`, `gui/mod.rs`,
  `app_impl.rs` and `settings_dispatch.rs`.

What:

- `Osc99InstanceId` gets `pub const fn new(u64) -> Self` and `pub const fn get(self) -> u64`.
  Its `.0` reads and constructions go through them.
- `to_toast_key` and `from_toast_key` become GUI-side free functions in `osc99/`.
- `Osc99SystemAction::to_leg_activation` becomes `impl From<Osc99SystemAction> for Osc99LegActivation`
  in `osc99/mod.rs`.
- `detached_handle` and `DetachedQueue`, the fake-daemon rig helpers, and the recording
  backends are re-exported only through `desktop_notify::test_support`.
- The four router-to-fake-daemon tests in `dbus_service/plain.rs` move into the
  `notifications.rs` test module, using the rig through `test_support`. The backend no longer
  imports `gui::notifications` or `gui::toast`, even in tests.
- No item is renamed.

Deliverable: as in 150.4a. Also, outside `desktop_notify`, a grep shows no path through
`desktop_notify::linux::` or `desktop_notify::icons::`.

#### 150.5 — Move the macOS/Windows plain worker into the backend

Scope: `freminal/src/gui/notifications.rs`, `freminal/src/gui/desktop_notify/plain_worker.rs`
(new), `desktop_notify/untracked.rs` and `desktop_notify/mod.rs`.

What:

- These move into `plain_worker.rs`:
  - `SystemNotification`;
  - `macos_notification`, `windows_toast`, `show_platform_notification`;
  - `SYSTEM_NOTIFICATION_QUEUE`, `EnqueueOutcome`, `enqueue_bounded`;
  - the respawn machinery (`RespawnDecision`, `respawn_decision`, `claim_respawn`,
    `claim_notify_worker_respawn`, `enqueue_with_respawn`, `same_queue`);
  - `log_dropped_notification`, `NOTIFY_WORKER`, `spawn_notify_worker`;
  - their tests.
- It exposes one entry point, `pub fn show_plain(notification: SystemNotification)`.
  `notifications.rs` keeps the routing-side mapping `system_notification(req)` from a
  `NotificationRequest`.
- `EnqueueOutcome` and untracked's `Submit` merge into one `desktop_notify::queue` type,
  provided their variants and logging are equivalent. If they are not, stop and report.
- Each worker's lifecycle is unchanged: static plus respawn for plain, owned plus shutdown for
  OSC 99.

Deliverable:

- the move;
- `notifications.rs` production code no longer names `notify_rust`, `tauri_winrt_notification`,
  `ArcSwapOption` or the worker;
- the queue and respawn tests unchanged in content, at their new path;
- an identical test count.

#### 150.6 — Backend-neutral names

Scope: `freminal/src/gui/desktop_notify/**`, `gui/notify_wake.rs`, and the users of renamed
items in `gui/` (the same file list as 150.4a and 150.4b).

What: rename exactly this set:

| Old                                              | New                                           |
| ------------------------------------------------ | --------------------------------------------- |
| `Osc99DbusService` / `Osc99DbusHandle`           | `LinuxNotifyService` / `LinuxNotifyHandle`    |
| `Osc99DbusEvent`                                 | `SystemNotifyEvent`                           |
| `Osc99SystemAction`                              | `SystemAction`                                |
| `Osc99SystemBackend` / `Osc99SystemDispatch`     | `TrackedBackend` / `SystemDispatch`           |
| `NoSystemBackend` / `RecordingSystemBackend`     | `NoTrackedBackend` / `RecordingTrackedBackend`|
| `Osc99UntrackedBackend`                          | `UntrackedBackend`                            |
| `Osc99InstanceId`                                | `NotificationInstanceId`                      |
| `Osc99DaemonSupport` / `Osc99DaemonCapabilities` | `DaemonSupport` / `DaemonCapabilities`        |
| `Osc99LinuxDaemon` / `Osc99Platform`             | `LinuxDaemon` / `NotifyPlatform`              |
| `Osc99IconLookup` / `Osc99IconChoice`            | `IconLookup` / `IconChoice`                   |
| `Osc99IconImage` / `Osc99DecodedIconCache`       | `IconImage` / `DecodedIconCache`              |
| `Osc99SoundHint`                                 | `SoundHint`                                   |
| `Osc99SignalSender` / `Osc99DaemonHealth`        | `SignalSender` / `DaemonHealth`               |
| `Osc99Wake`                                      | `NotifyWake`                                  |
| `OSC99_RECONNECT_INTERVAL` etc.                  | `RECONNECT_INTERVAL` etc. (drop the prefix)   |
| threads `freminal-osc99-{dbus,owner,signals}`    | `freminal-notify-{dbus,owner,signals}`        |
| threads `freminal-osc99-{icons,deliver}`         | `freminal-notify-{icons,deliver}`             |
| `FreminalGui::osc99_dbus` / `osc99_wake`         | `notify_service` / `notify_wake`              |

- `"OSC 99:"` log prefixes in backend files become `"desktop notifications:"`, except where
  the path serves only OSC 99. Each kept one is listed in the report.
- The icon directory constant is renamed, but its value stays `"freminal-osc99-icons-"`.
  The probe-file prefix is unchanged too. A comment on the constant records why
  (maintainer decision 3).
- `Osc99PlatformSound` / `untracked_sound` stay: they map OSC 99's `s=`.
- OSC 99 protocol and registry types keep their names.

Deliverable:

- the renames;
- a grep proving no listed old name remains;
- a test asserting the icon directory prefix value;
- an identical test count.

#### 150.7a — One plain-notification sink for every platform

Scope: `freminal/src/gui/desktop_notify/{plain_sink.rs (new),mod.rs,linux/plain.rs,plain_worker.rs}`,
`freminal/src/gui/notifications.rs`, `freminal/src/gui/osc99/glue.rs`,
`freminal/src/gui/frame_drain.rs` (the sink type in `drain_command_finished_events` and its
tests), and the `system_notify_sink` call sites in `freminal/src/gui/app_impl.rs` and
`freminal/src/gui/settings_dispatch.rs`.

What:

- The backend provides `PlainSink`, a cloneable value with one method on every platform,
  `show(&self, PlainNotification)`.
  - Its private enum holds the Linux `PlainNotifySender`, the macOS/Windows plain worker, a
    "no service" variant (today's `none()`, with its debug log), and a test recorder
    (`#[cfg(test)]`).
  - `PlainNotification` becomes the request on every platform. On macOS and Windows, urgency
    is not read, as today.
- `SystemNotifySink` and both `show_system` copies are replaced. The router calls `sink.show`.
- Delete:
  - the `cfg!(test)` early return (tests pass a recorder instead);
  - the non-Linux `system_notify_sink` with its `let _ = self;`;
  - the platform split of `resolve_system_notify_sink`.
- The service-start policy (`plain_notifications_may_use_system_leg`) stays in the GUI and is
  passed in, as now.
- Tests that relied on the `cfg!(test)` guard use the recorder, and assert what was sent.

Deliverable:

- no platform `cfg` in `notifications.rs` production code outside the request mapping, with
  any that remain listed and justified;
- the router tests and the plain fake-daemon tests unchanged in content;
- `check-windows` clean.

#### 150.7b — The `DesktopNotifier` facade

Scope: `freminal/src/gui/desktop_notify/{notifier.rs (new),mod.rs}`, `gui/notify_wake.rs`,
`freminal/src/gui/mod.rs` (fields), `freminal/src/gui/osc99/glue.rs`, `osc99/effects.rs`,
`freminal/src/gui/app_impl.rs`, `freminal/src/gui/settings_dispatch.rs` and
`freminal/src/gui/notifications.rs` (the `route_osc99` leg choice).

What:

- `DesktopNotifier` is the one `FreminalGui` field that owns the backends. It replaces
  `notify_service` (`RefCell<Option<LinuxNotifyHandle>>`) and `osc99_untracked`. It is
  constructed with `NotifyAssets` and the waker, and has these methods on every platform:

  - `ensure_started(ServiceDemand)`: today's `ensure_osc99_dbus_started`; a no-op off
    Linux/BSD.
  - `plain_sink(impl FnOnce() -> ServiceDemand) -> PlainSink`: today's resolution, including
    the busy-cell fallback.
  - `with_backends(f)`: calls `f(&mut dyn TrackedBackend, &mut dyn UntrackedBackend)`,
    falling back to the no-op backends when a cell is busy, exactly as `with_osc99_sink`
    does.
  - `drain_events() -> Vec<SystemNotifyEvent>`: empty off Linux/BSD.

  `ServiceDemand { Wanted, NotWanted }` is a named enum: the policy stays in
  `host_capabilities.rs`, and the notifier only obeys.
- `desktop_notify::SYSTEM_LEG` is a `const` of a two-variant enum (`Tracked` on Linux/BSD,
  `Untracked` elsewhere). It replaces the `cfg!` in `route_osc99`. The registry's `Osc99Leg`
  is chosen from it.
- `NotifyWake` compiles on every platform, so its register/retarget/unregister calls in
  `app_impl.rs` lose their `cfg`.
- The two `apply_and_feed_back` copies in `glue.rs` merge into one. So do the two
  `FreminalGui::system_notify_sink` copies, which also removes the non-Linux `let _ = self;`
  (moved here from 150.7a). `glue.rs`, `app_impl.rs` and `settings_dispatch.rs` keep no
  backend platform `cfg`. Policy `cfg`s in `host_capabilities.rs` are left alone and listed.
- The shutdown order is unchanged: dropping `FreminalGui` drops the untracked worker (joining
  it and removing temp files) and the service handle.

Deliverable:

- the facade;
- unit tests for each method's fallback (busy, not started, not wanted) moved or ported from
  the `resolve_system_notify_sink` and `with_osc99_sink` tests;
- a list of every remaining platform `cfg` outside `desktop_notify`, each with its reason.

#### 150.8 — Testable platform builders (R13)

Scope: `freminal/src/gui/desktop_notify/{plain_worker.rs,untracked.rs}`.

What:

- Every platform builder becomes a pure fields function, compiled and tested on every
  platform, plus a thin conversion compiled only on its platform. This is the pattern 149.3
  used for `macos_notification_fields`.
  - **Windows plain toast:** title, `text1` (left out for an empty body), silent, short
    duration, app id.
  - **macOS plain notification:** appname, icon, summary, body.
  - **Windows untracked toast** (OSC 99): including the `appLogoOverride` image URI and sound.
- The conversions only copy fields; no decision remains in them.

Deliverable: fields tests for each builder, run on Linux CI. The conversions are byte-for-byte
the old builders, which review checks by reading them.

#### 150.9 — Documentation

Scope:

- `Documents/DESIGN_DECISIONS.md` (a "Desktop notification backends" section: the boundary,
  the two request kinds, the one-surface rule, injected assets, worker lifecycles, why the
  icon directory prefix is kept);
- `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `ESCAPE_SEQUENCE_GAPS.md`,
  `KITTY_PROTOCOL_REFERENCE.md` (paths, type and thread names only; no behaviour claims
  change);
- the `.opencode/skills/` files that name `osc99/dbus_service`, `untracked.rs` or the renamed
  types (find them with grep);
- module docs in `desktop_notify/mod.rs`.

What: every reference to a moved path, a renamed type or a renamed thread is updated. Every
doc claim about notification behaviour is checked against the code and left unchanged.

Verification: markdownlint and `prettier --check` on the named files only.

#### 150.10 — The `freminal-notify` crate

Scope:

- `freminal/src/gui/desktop_notify/**` moves with `git mv` to `freminal-notify/src/**`;
- `freminal-notify/Cargo.toml` (new);
- the root `Cargo.toml` (`members`, `default-members`, workspace dependency entries);
- `freminal/Cargo.toml`;
- `Cargo.lock`;
- the `use` sites in `freminal/src/gui/`;
- `agents.md` (workspace layout block), the `freminal-architecture` skill (crate list and
  dependency direction), `DESIGN_DECISIONS.md` (the 150.9 section);
- `xtask/src/**`, `.github/workflows/*.yml`, `deny.toml` and `codecov.yml`, but only where
  they list crates by name.

What:

- `freminal-notify` depends on `freminal-common` and external crates only. It inherits the
  workspace lints, and its package metadata matches its siblings.
- Its target-specific dependencies move out of `freminal/Cargo.toml`:
  - `zbus`, `async-io`, `futures-lite`, `serde` and the `image` use on Linux/BSD;
  - `notify-rust` on macOS;
  - `tauri-winrt-notification` on Windows;
  - `swash` and `conv2` where the raster path needs them;
  - `crossbeam-channel`, `arc-swap`, `thiserror` and `tracing`.

  `freminal` keeps each one only if it still uses it itself (`cargo machete` decides).
- `pub(in crate::gui)` becomes `pub` on the surface `mod.rs` re-exports. Internals stay
  private.
- `test_support` becomes a `test-support` feature that `freminal`'s dev-dependency enables;
  production builds never enable it. Rig dev-dependencies, such as `zbus`'s `p2p` feature,
  move with it.
- Nothing else changes: the diff outside manifests and docs is paths and visibility only.
- Remove the temporary module-stage dead-code workarounds. An exported item of a library is
  never dead code:
  - the `cfg` gates on `StandardIcon::{ALL, codepoint, freedesktop_name}` (150.1);
  - the `cfg_attr(…, expect(dead_code))` on the `NotifyAssets` fields (150.2);
  - any other `TODO(150.10)`.

  A grep for `TODO(150.10)` is empty afterwards.

Deliverable:

- the crate;
- `cargo tree -p freminal-notify` shows no workspace crate except `freminal-common`;
- `cargo machete`, `cargo deny check` and `cargo xtask ci` pass;
- an identical test count across the workspace.

### 150 Status notes

- **150.1 (done, `21ad3f3d`).** `StandardIcon` is the one table. `standard_icon_glyph` and
  `standard_icon_name` delegate to it, and pin tests passed against the old tables first.
  `ALL`, `codepoint` and `freedesktop_name` are `cfg`-gated to their callers, because in the
  binary crate an unused item is dead code. The gates go at 150.10.
- **150.2 (done, `14c6de74`).** `NotifyAssets` is built once by `app_icon::notify_assets()`
  and passed to the D-Bus service and the untracked backend. `bundled_regular_font_bytes` is
  ungated.
  - Each field is read on one platform only, so each carries a
    `cfg_attr(…, expect(dead_code, reason = "…TODO(150.10)…"))`. This is the documented
    temporary-refactor exception, and it goes at 150.10.
  - The Linux/BSD `platform_untracked_backend` is its own definition with an `_assets`
    parameter.
  - Backend tests use `test_support::test_assets()`, which embeds the same repository files.
- **150.3 (done, `742395ff`).** `Osc99IconImage.rgba` is `Arc<[u8]>`, so the plain icon and
  the decoded-icon cache hand out shared buffers. The wire bytes are unchanged.
- **150.4a (done, `4e3d5fc5`).** A pure move. The sorted-line diffs show only paths, and the
  test count is unchanged (9536). Two module-doc links from `untracked.rs` into `osc99` were
  rewritten in 150.4b.
- **150.4b (done, `8a9b16c9`).** A pure move. Test count unchanged (9536), rustdoc warnings
  unchanged (69).
  - `Osc99InstanceId::{new, get}` replace the tuple field outside `backend.rs`.
  - The toast-key conversions are `osc99::{instance_toast_key, instance_from_toast_key}`.
  - `From<Osc99SystemAction> for Osc99LegActivation` replaces `to_leg_activation`.
  - The four router-to-fake-daemon tests moved to a `fake_daemon` module in
    `notifications.rs`.
  - Backend docs no longer link to GUI items.
  - `grep crate::gui:: desktop_notify/` is empty.
  - Test-rig items re-exported through `test_support` became `pub` (test-only).
- **150.5 (done, `2a8685c1`).** The plain worker and its respawn machinery moved to
  `plain_worker.rs`, behind `show_plain(SystemNotification)`.
  - **Deviation:** `EnqueueOutcome` and `Submit` are not equivalent. `EnqueueOutcome` has
    `RespawnSuppressed` and hands a disconnected item back, while `Submit` logs inline. So
    only their shared non-blocking offer moved to `queue.rs` (`offer` → `Offered`), and each
    worker keeps its own outcome type and logs.
  - In review the orchestrator caught that a test doc comment had been split by the move and
    restored it. A doc-line multiset check then showed nothing lost.
- **150.6 (done, `2d2f6a6c`).** Renames as tabled. Assertion counts per file are unchanged
  apart from the new `ICON_DIR_PREFIX` value test. No log line needed changing: every
  `"OSC 99:"` line left in the backend sits on an OSC-99-only path (the tracked show and
  close, the untracked worker, icon lookup, raster, temp files and transmitted-icon decode).
  Every log on a shared path already said `"desktop notifications:"`.
- **150.7a (done, `87107bd4`).** `PlainSink` (`nowhere`, `through_service`,
  `platform_worker`, plus the test-only `recording`) and an all-platform `PlainNotification`
  replace `SystemNotifySink` and `SystemNotification`.
  - The router has one `show_system`, and the `cfg!(test)` guard is gone.
  - The macOS-abort reason for the guard moved into the `PlainSink` module docs as the
    invariant: only GUI production code constructs an OS-reaching sink.
  - Four new router tests cover the system leg on every platform through the recorder.
  - **Amendment:** merging the two `FreminalGui::system_notify_sink` copies (and removing
    `let _ = self;`) needs the facade to own the service cell, so it moved to 150.7b.
  - `Route::Nowhere` / `nowhere()` are gated `any(test, Linux/BSD)`: macOS/Windows
    production never builds one.
- **150.7b (done, `d294c454`).** `DesktopNotifier` (`desktop_notify/notifier.rs`) is the one
  `FreminalGui` field that owns the backends. Its methods on every platform are
  `ensure_started(ServiceDemand)`, `plain_sink(…)` and `with_backends(…)`. `is_started` and
  `drain_events` exist on Linux/BSD only, where the events exist.
  - It takes a waker factory, so the service gets its waker when it first starts.
  - The platform split is a private `platform` module of free functions.
  - `SYSTEM_LEG` replaces the `cfg!` leg choice in `route_osc99`.
  - `NotifyWake` and its calls are ungated.
  - The `system_notify_sink` and `apply_and_feed_back` pairs are one definition each.
  - The four `resolve_system_notify_sink` tests moved to `notifier.rs` (17 notifier tests in
    all).
  - **Review changes by the orchestrator:**
    - The sub-agent made the system-event entry point cfg-free with a macOS/Windows twin
      whose only body was a `debug_assert`, there to read `self`. That is the shape Task 149
      flagged (R10), so the twin was removed. The single `route_osc99_system_events` call in
      `app_impl.rs` keeps one `#[cfg]`, with a comment giving the reason: the event types
      exist only where a tracked leg does.
  - **Accepted, with reasons:**
    - ~~Off Linux/BSD, `ensure_started` emits one `trace!` line.~~ **Reversed by 150.C18:**
      `ensure_started` is now Linux/BSD-only, with one `#[cfg]` at each of its two call
      sites.
    - The ported `route_through` helper no longer asserts that System routing pushes no
      toast. That is a router property, and it is asserted by the router tests in
      `notifications.rs`.
    - The non-Linux `feed_back_failed_legs` keeps the old `debug_assert` that nothing failed.
  - **Remaining platform `cfg`s outside `desktop_notify`:**
    - `glue.rs`: the event chain that consumes `SystemNotifyEvent`, and the feed-back pair;
    - `host_capabilities.rs`: `daemon_state_change`;
    - `osc99/mod.rs` and `registry.rs`: the Linux tracked-leg types and methods;
    - the one call in `app_impl.rs` described above;
    - test modules;
    - added by 150.C18: the `ensure_notification_service_started` calls in `app_impl.rs` and
      `settings_dispatch.rs`, and the method itself in `glue.rs`.
- **150.8 (done, `40dcbc30`).** Each of the four platform builders is now a pure fields
  function, tested on every platform, plus a thin conversion that only copies fields:
  - plain macOS: `macos_plain_fields`;
  - plain Windows: `windows_plain_toast_fields`;
  - untracked Windows: `windows_untracked_toast_fields`, with
    `ToastImage { None, Logo, Unusable }`;
  - untracked macOS already had this shape.

  The orchestrator compared each old conversion with its new one; they make the same calls,
  in the same order, with the same values. 14 new fields tests.
  - **Defect found in review and fixed here:** 150.7a left the macOS-only test
    `macos_notification_sets_freminal_icon` building a `PlainNotification` without
    `urgency`, so the macOS test build would not have compiled. Linux and windows-gnu clippy
    cannot see a macOS-only test. Its four assertions now live in a fields test that runs
    everywhere.
- **150.9 (done, `1772450f`).** A "Desktop Notification Backends (Task 150)" section in
  `DESIGN_DECISIONS.md`. Moved paths and renamed threads are updated in the kitty reference
  and COVERAGE, and COVERAGE, GAPS and the kitty reference each got a "Last updated" entry.
  No skill named a moved item. No behaviour claim was found false.
- **150.10 (done, `b879b0ae`).** `freminal-notify` is a workspace member and a default member.
  It depends only on `freminal-common` plus external crates (`cargo tree` checked).
  - **Feature name:** `test-utils`, not `test-support`. `clippy::cargo` rejects the
    `-support` suffix, and `freminal-windowing`'s `test-utils` (138.5) is the precedent.
    `freminal` enables it in its dev-dependency only. The module keeps the name
    `test_support`.
  - **Gates:** items the GUI's tests use moved from `#[cfg(test)]` to
    `any(test, feature = "test-utils")`. Rig helpers only the crate's own tests use stay
    `#[cfg(test)]`, so nothing extra is compiled under the feature.
  - **Workarounds removed:** every `TODO(150.10)` dead-code workaround is gone (the
    `StandardIcon` gates and the `NotifyAssets` `expect`s).
  - **Dependencies:** `zbus`, `async-io`, `futures-lite`, `serde`, `notify-rust` and
    `tauri-winrt-notification` left `freminal`. In review, the orchestrator moved `swash`,
    `conv2` and `arc-swap` to a macOS/Windows table (plus dev-dependencies), because only
    macOS/Windows code uses them.
  - **Docs:** `agents.md`, the architecture skill, `.github/` docs and the reference docs
    are updated.
  - **Verification:** rustdoc `-D warnings` is clean for the crate, with and without
    private items and the feature. Test count unchanged (9571).

### 150 Cleanup entries

These come from the adversarial review of 2026-10-22: two parallel CODE-REVIEW passes over
`main...HEAD`, (A) on the crate and (B) on the GUI and docs. Neither found a blocker. (A)
confirmed by diffing against `main` that the moved code changes no behaviour.

- **150.C1 (major; A1, B1): the crate's tests did not build on their own.**
  - `zbus/p2p` was reachable only through the `test-utils` feature, so
    `cargo test -p freminal-notify` and `cargo clippy -p freminal-notify --all-targets`
    failed. The workspace run passed only through feature unification.
  - Fix: a Linux/BSD target dev-dependency `zbus = { workspace = true, features = ["p2p"] }`.
  - Verify: both per-package commands pass.
- **150.C2 (A2, B5): `PlainSink::platform_worker` was `pub`.** It becomes `pub(crate)`, so
  the "tests never reach the OS" invariant is enforced by the compiler.
- **150.C3 (A3): `PlainRecorder` could not be named.** Re-export it from `test_support`.
- **150.C4 (A4, B2): the public API varied by platform and feature.** These were the
  binary-crate dead-code gates that 150.10 should have removed:
  - `SystemDispatch::Queued`;
  - the `NoTrackedBackend`, `NoUntrackedBackend` and `DaemonCapabilities` exports;
  - `DaemonCapabilities::body_markup`.

  All become unconditional, except where a `cfg` removes a real platform dependency.
- **150.C5 (A5): `linux/test_rig.rs` had about 25 scattered `#[cfg(test)]` attributes.**
  Split it into a feature-visible rig and a `#[cfg(test)]` sibling of crate-only helpers.
- **150.C6 (A6, B6): `freminal-notify` was missing from repo-level lists.** Add it to
  `.coderabbit.yaml` (a path block, the dependency-direction text, the no-`anyhow` list) and
  to `agents.md` (the no-`anyhow` rule and "Crate-specific guidance").
- **150.C7 (A7): the crate had the generic workspace description.** It gets its own.
- **150.C8 (A8): crate files without a licence header.** Add the header every other crate's
  files carry.
- **150.C9 (A9, B7): crate docs named GUI-internal types.** Reword them in the crate's own
  terms.
- **150.C10 (A10): derived `Debug` printed the 2 MB font.** On `NotifyAssets`,
  `PlatformUntrackedBackend` and `IconPreparer`, `Debug` is written by hand and prints byte
  counts.
- **150.C11 (A11): typo.** "an [`LinuxNotifyHandle`]" becomes "a [`LinuxNotifyHandle`]".
- **150.C12 (A12): redundant per-item `cfg`s in `plain_worker.rs` and `queue.rs`.** Their
  module is already gated by the same expression, so the per-item gates are removed.
- **150.C13 (A13): one-variant enums in the 150.8 fields types** (`ToastAppId`,
  `PlainToastSound`, `PlainToastDuration`). They are not decisions, so they are removed and
  the constants live in the conversion. The tests lose only assertions that checked those
  constants.
- **150.C14 (A14): two tests asserted nothing real.** The `nowhere` sink drop and the
  not-wanted `plain_sink` now assert that nothing was delivered or queued.
- **150.C15 (A15, A16): stale and task-citing comments.** The "Slice 3 (#433)" test doc, and
  the "maintainer decision, Task 150" comments in `tempfiles.rs`, keep only the reason.
- **150.C16 (A17): reviewed, not a defect.** `presentation::sound_hint` maps the five OSC 99
  standard sound names (`error`, `warn`, `warning`, `info`, `question`). That is a different
  set from the eight icon names. Routing it through `StandardIcon` would add sounds for
  `help` and the others, which would change behaviour.
- **150.C17 (B2): three new rustdoc warnings in `freminal` (private items).** They are in
  `notifications.rs` (`PlainSink::recording`), `osc99/mod.rs` (`UntrackedBackend`) and
  `glue.rs` (a redundant link target).
- **150.C18 (B3): the R10 shape remained in two places.**
  - The non-Linux `feed_back_failed_legs` twin goes. The one call in `apply_and_feed_back`
    is `cfg`-gated instead.
  - The non-Linux `ensure_started` existed only to emit a `trace!` that dodged a lint.
    `DesktopNotifier::ensure_started` and `ensure_notification_service_started` become
    Linux/BSD-only, and their two call sites carry one `#[cfg]` each, with a reason. This
    reverses the 150.7b acceptance.
  - `NotifyWake` running on macOS/Windows is kept. It does real work there, its state is
    consistent though unread, and it has no dead code. Gating it would put four `cfg`s back
    at call sites. The reason is recorded in `notify_wake.rs`.
- **150.C19 (B4): the new policy wrappers were untested.** Add tests for
  `notification_service_demand` and `plain_notification_demand`. The pure-policy tests in
  `notifications.rs` and `host_capabilities.rs` lose their Linux-only `cfg`.
- **150.C20 (B7): stale comments.**
  - `notifications.rs:15-16` says the service is in `osc99`.
  - `notifications.rs:387-389` confuses the crate name with the thread name.
  - `presentation.rs:448` names `icon_decode`.
  - The Task 150 stub's "Wake-up integration" bullet names `Osc99Wake`.
- **150.C21 (B8): doc formatting.**
  - Ragged wraps in the new "Last updated" blocks (COVERAGE, the kitty reference) and in
    `DESIGN_DECISIONS.md`.
  - `MASTER_PLAN.md` tables need re-padding to pass `prettier --check`, as they did on
    `main`.
- **150.C22 (B9): import grouping.** `freminal_notify` imports join the external-crate group.
- **150.C23 (B10): verified, nothing to fix.** `test-utils = ["zbus/p2p"]` on Windows is
  covered by `cargo xtask check-windows`, which runs `--all-features` and passed.
- **Status (2026-10-22): all done.** They landed in three commits:
  - **`68a25cdb`**: the crate side, C1–C5, C7–C15, the crate half of C18 and the crate
    half of C20.
    - `cargo test -p freminal-notify` passes on its own (360 tests).
    - `tombi` reordered the new manifest's tables.
    - The orchestrator removed one vacuous assertion the C14 rewrite had added, and stopped
      `IconPreparer`'s `Debug` from printing the drawn PNGs.
    - C13 removed exactly one test (`windows_plain_toast_fields_are_silent_short_and_powershell`,
      three constant-only assertions).
  - **`de53e55a`**: the GUI side, C17, the GUI half of C18, C19, the GUI half of C20, and
    C22.
  - **`c239c4f8`**: the docs, C6 and C21.

  Two decisions inside C18:
  - **`feed_back_failed_legs`:** the planned "gate the call" form fails `check-windows`
    (unused `registry`, `needless_pass_by_ref_mut`). Every other fix needs a `cfg` at eight
    call sites or a suppression. The non-Linux definition is kept: it states that
    platform's real semantics (nothing can fail, so there is nothing to feed back) and
    asserts them. This is not the R10 shape, and `main` had the same split one level up.
  - **`NotifyWake`:** kept on every platform, with the reason in its module docs. Why this
    is not the case `ensure_started` was: its methods change real state on every platform,
    and nothing in it exists only to satisfy a lint. The off-Linux `ensure_started` had no
    work at all, and its `trace!` existed only to defeat `missing_const_for_fn`. The waker
    factory that `DesktopNotifier::new` drops off Linux/BSD is the remaining cost.
  - The docs pass also added a `freminal-windowing` bullet to `agents.md` "Crate-specific
    guidance", which no finding asked for. It was checked against that crate's `lib.rs`;
    mention it in the PR description.

  C18 also moved three policy predicates to `any(test, Linux/BSD)`, and their pure-policy
  tests now run on every platform. Final counts: 9574 tests, clippy clean, `check-windows`
  clean, rustdoc warnings for `freminal` 69 public (unchanged) and 105 with private items
  (three fewer, by C17).
- **Confirmation review (2026-10-22)** of `5efce5ec..HEAD`: 22 of 23 entries FIXED or
  accepted as documented, C20 partial. No blocker or major. Its nine new items were all
  fixed in the follow-up commit:
  - C20's stale `Osc99Wake` stub bullet;
  - the `platform_worker` doc wording;
  - the 150.7b note, marked as reversed by C18, with the new `cfg` sites listed;
  - the `NotifyWake` versus `ensure_started` reasoning, now stated;
  - the extra `freminal-windowing` bullet, recorded for the PR;
  - two "egui frame thread" mentions in crate docs;
  - the rustdoc figure;
  - the moved `tombi` comment.

  Items 7 and 10 needed no change: the nowhere test is structural by design, and the
  prettier results on `agents.md` and this plan already fail on `main`.
- **PR #576 review (Copilot, CodeRabbit), 2026-10-22.** All three findings were valid:
  - **150.C24 (Copilot):** `DESIGN_DECISIONS.md` still listed `ensure_started` among the
    all-platform methods after 150.C18 made it Linux/BSD-only. The doc now lists it with
    `is_started` and `drain_events` and names the three gated GUI call sites.
  - **150.C25 (Copilot):** the OSC 99 untracked worker was documented as "never restarted",
    both in this plan (recon and decision 2) and in `DESIGN_DECISIONS.md`. In fact
    `PlatformUntrackedBackend::show` drops a worker that reports `WorkerGone`, and the next
    notification spawns a new one, with no rate limit. The code's own doc comment was
    correct. Both documents now describe the real behaviour; no code change.
  - **150.C26 (CodeRabbit):** `RecordingTrackedBackend::fail_shows: bool` is public
    test-utils API. It becomes `show_answer: RecordedShowAnswer { Queue, Fail }`, and the
    test usages are updated with no assertion change.
- **Manual-test finding, 2026-10-22: not a freminal defect.** The maintainer ran
  `test-scripts/osc99_notifications.py` against wayle 0.1.3.
  - **Step 6** (`[0001][0002]…`) is the intended numbered long body.
  - **Step 7**: the toast updated but the desktop popup did not. A session-bus capture shows
    freminal sending the update with `replaces_id` set to the first notification's id (177).
    It then closes 177 after `w=5000`, and wayle confirms the close of 177.
  - The same `Notify` pair sent from `busctl` with freminal's exact actions and hints also
    left the popup unchanged. A bare pair (no actions or hints) was repainted.
  - So wayle accepts `replaces_id` but does not repaint a visible popup that carries
    actions or hints. It is an upstream wayle bug, reproducible with `busctl` alone, and
    no freminal change is planned.

---

## Task 139 — Pointer Shapes (OSC 22) Completion

### 139 Summary

**Goal.** Full kitty OSC 22 (<https://sw.kovidgoyal.net/kitty/pointer-shapes/>): set (`=` or
no prefix), push (`>`), pop (`<`) and query (`?`), on separate main and alternate screen
stacks, cleared by RIS and DECSTR. Activated and decomposed on 2026-11-19 on branch
`task-139/pointer-shapes`, from `main` at `07fd4030`.

### 139 Activation recon (2026-11-19)

Two read-only passes: freminal's OSC 22 path end to end, and kitty's reference implementation
(`kitty/window.py` `set_pointer_shape`, `kitty/screen.c` `change_pointer_shape` /
`current_pointer_shape` / `do_screen_reset`, `kitty/glfw.c` `is_css_pointer_name_valid`,
`kitty/mouse.c`). Every stub finding was confirmed. Additional findings:

- **Parser.** `OSC 22` goes through `with_osc_tokens` (`ansi_components/osc.rs`), which splits
  the body on `;` and reads only token 1. `handle_osc_pointer_shape` emits
  `AnsiOscType::SetPointerShape(PointerShape::from(name))`. `PointerShape::from(&str)` is
  infallible and maps every unknown name, and every prefixed form, to `Default`.
- **Handler.** One global `pointer_shape: PointerShape` field (`terminal_handler/mod.rs`), set
  in `terminal_handler/osc.rs`. `reset.rs` already resets it on **both** RIS and DECSTR, as the
  131 reset table records, so only the field's shape changes. `ScreenScoped<KittyKeyboardStack>`
  is the template: indexed by `self.buffer.kind()`, cleared with `both_mut()`.
- **Snapshot and GUI.** `TerminalSnapshot::pointer_shape: PointerShape` is read from the
  handler getter in `build_snapshot`. The GUI (`widget.rs`) maps it to an `egui::CursorIcon`
  only when the pointer is over terminal content (gutter, fold placeholder and URL hover take
  precedence), writing `cursor_icon` once per frame from the pane under the pointer. Nothing
  in the GUI changes the shape when mouse reporting is on, and nothing overrides it during a
  selection drag. The GUI needs no change for this task.
- **Kitty reference behaviour** (the spec is silent on most of these; kitty is the
  tie-breaker):
  - The first byte selects the op only if it is `=`, `>`, `<` or `?`; otherwise the op is set
    and the byte is part of the name. Names are split on `,` with no trimming and matched
    case-sensitively.
  - **Set** replaces the top entry (an empty stack first grows to one entry); it never pushes
    or clears. A set list is applied name by name, so the last valid name wins. An **empty
    name** in a set stores an "unset" entry on top: the visible shape is the default, the
    query reports `0`, and entries below survive (a later pop restores them). Unknown names
    are ignored, one at a time.
  - **Push** pushes each valid non-empty name in order; the last is the top. Capacity 16; a
    push onto a full stack evicts the bottom. Empty and unknown names are skipped.
  - **Pop** pops exactly one entry and ignores its names; a pop of an empty stack does nothing.
  - **Query** always replies, with `OSC 22 ; <list> ST` (ST even for a BEL-terminated request),
    one comma-joined item per queried name in order: `1`/`0` for support, the current shape's
    canonical CSS name for `__current__` (`0` when the stack is empty or its top is unset), and
    the CSS name of the default and grabbed shapes for `__default__` / `__grabbed__`. A set,
    push or pop sends no reply.
  - The alternate stack persists across alternate-screen switches; only RIS and DECSTR clear
    the stacks, and both clear both.
  - Kitty accepts 46 legacy X11 / internal aliases for set and push (table in "139 Decisions").
    Its support query answers `0` for most of them (and `1` for `arrow`, `beam`, `hand`).
- **Freminal-only names.** freminal accepts `none`, `context-menu`, `all-scroll` (CSS cursor
  names absent from the kitty table) and the CSS names `col-resize` / `row-resize`, mapped to
  `ew-resize` / `ns-resize`. (The three extras were removed by review finding R7;
  `col-resize` / `row-resize` stay, see the confirmation review.)

### 139 Decisions (orchestrator, recorded so they are not re-litigated)

Spec first, kitty code as tie-breaker. Each place freminal deliberately differs from kitty is
marked **(differs from kitty)**.

- **Parsing follows kitty** exactly: the op prefix rule, `,` splitting with no trimming,
  case-sensitive names, per-name ignoring of unknown names. The body is the raw OSC body after
  `22;` (a raw-body dispatch arm, no `;` tokenisation); `OSC 22` with no `;` is an empty set.
  A non-UTF-8 name is unknown. Names are looked up in one table; there is no separate
  `a-z0-9_-` check (kitty).
- **Per-request name cap: 64** (`MAX_OSC22_NAMES`). Names past the cap are ignored, for every
  op. Freminal policy, like the XTGETTCAP cap (148.11): it bounds the reply a single query can
  produce. **(differs from kitty**, which has no cap; no real request comes near it.)
- **Set, push, pop and the "unset" entry follow kitty** as described in the recon, including
  that an empty set leaves lower entries in place.
- **Stack capacity is 16** (`POINTER_SHAPE_STACK_DEPTH`), the spec minimum and kitty's value;
  a push onto a full stack evicts the bottom.
- **Name table.** Accepted for set, push and the support query:
  - the 30 spec CSS names;
  - freminal's existing extras `none`, `context-menu`, `all-scroll` (**superseded by review
    finding R7: removed**), and `col-resize` / `row-resize` as aliases of `ew-resize` /
    `ns-resize`;
  - kitty's full alias table: `left_ptr`, `arrow` → default; `xterm`, `ibeam`, `beam` → text;
    `pointing_hand`, `hand2`, `hand` → pointer; `question_arrow`, `whats_this` → help;
    `clock`, `watch` → wait; `half-busy`, `left_ptr_watch` → progress; `tcross` → crosshair;
    `plus`, `cross` → cell; `fleur`, `pointer-move` → move; `right_side` → e-resize;
    `top_right_corner` → ne-resize; `top_left_corner` → nw-resize; `top_side` → n-resize;
    `bottom_right_corner` → se-resize; `bottom_left_corner` → sw-resize; `bottom_side` →
    s-resize; `left_side` → w-resize; `sb_h_double_arrow`, `split_h` → ew-resize;
    `sb_v_double_arrow`, `split_v` → ns-resize; `size_bdiag`, `size-bdiag` → nesw-resize;
    `size_fdiag`, `size-fdiag` → nwse-resize; `zoom_in` → zoom-in; `zoom_out` → zoom-out;
    `dnd-link` → alias; `dnd-copy` → copy; `forbidden`, `crossed_circle` → not-allowed;
    `dnd-no-drop` → no-drop; `openhand`, `hand1` → grab; `closedhand`, `dnd-none` → grabbing.
- **Support query answers `1` for every name freminal accepts**, aliases and extras included.
  A name that can be set is supported, so this is the truthful answer. **(differs from
  kitty**, whose query answers `0` for most aliases while accepting them.)
- **`Display` / the reply name is the canonical CSS name** (`ew-resize`, `ns-resize`; `none`,
  `context-menu`, `all-scroll` for the extras, which R7 removed). A shape set through an alias is reported by its
  canonical name (kitty).
- **`__default__` and `__grabbed__` both answer `default`.** freminal shows the default arrow
  over terminal content when no shape is set, and does not change it when mouse reporting is
  on. Truthful for freminal; kitty answers its own configured shapes (`text`, `default`).
- **Replies** go through `write_osc_response(&format!("22;{list}"))` on the PTY thread, so they
  are S8C1T-framed, ST-terminated and ordered before a following DA1 (Task 130).
- **Logging.** An ignored name is logged at `debug!` with its length only (payload-free
  logging, Task 129).
- **Snapshot keeps `pointer_shape: PointerShape`**, the active screen's resolved shape:
  `Default` for an empty stack or an unset top. The GUI is unchanged.
- **Reset**: RIS and DECSTR clear both stacks (already decided in the 131 reset table). Screen
  switches touch neither stack.

### 139 Types (decided)

In `freminal-common/src/buffer_states/pointer_shape.rs`:

```rust
impl PointerShape {
    pub fn from_name(name: &str) -> Option<Self>;   // the one name table; case-sensitive
    pub const fn css_name(self) -> &'static str;    // canonical name; Display delegates
}
pub const MAX_OSC22_NAMES: usize = 64;
pub enum PointerShapeEntry { Shape(PointerShape), Unset }
pub enum PointerShapeQuery { Supported, Unsupported, Current, Default, Grabbed }
pub enum PointerShapeOp {
    Set(Vec<PointerShapeEntry>),   // applied in order; unknown names already dropped
    Push(Vec<PointerShape>),       // empty and unknown names already dropped
    Pop,
    Query(Vec<PointerShapeQuery>),
}
```

`AnsiOscType::SetPointerShape(PointerShape)` is replaced by
`AnsiOscType::PointerShape(PointerShapeOp)`.

In `freminal-terminal-emulator/src/terminal_handler/pointer_shape_stack.rs`:

```rust
pub const POINTER_SHAPE_STACK_DEPTH: usize = 16;
pub struct PointerShapeStack { entries: Vec<PointerShapeEntry> }
// set(entry), push(shape), pop(), clear(), current() -> Option<PointerShape>
```

The handler field becomes `pointer_shape_stack: ScreenScoped<PointerShapeStack>`.

### 139 Execution model

Serial, on one checkout (`task-139/pointer-shapes`): the subtasks share the common pointer
module and the handler. Each subtask is implemented by a sub-agent, reviewed in full by the
orchestrator against this text (the whole diff, every new file read in full, the spec and the
decisions above), verified again by the orchestrator, and only then committed by the
orchestrator. Order: 139.1, 139.2, 139.3, 139.4, 139.5. An adversarial review of `main...HEAD`
against this section and the kitty spec follows 139.5; every finding, nit and above, becomes a
`139.Cn` entry and is fixed (or routed to the task that owns the code) before the PR.

No benchmark subtask: OSC 22 is not on a `freminal-bench-table` path, and the change touches
only the OSC 22 dispatch arm.

### 139 Subtasks

Every subtask's verification is `cargo xtask test` and
`cargo clippy --all-targets --all-features -- -D warnings`. Prohibitions for every subtask: do
NOT touch files outside scope, do NOT commit, do NOT update plan documents, do NOT proceed to
the next subtask; no `unwrap`/`expect` outside tests, no raw numeric `as` casts, no new `bool`
fields or parameters, no `#[allow(dead_code)]`; do not delete or weaken a test assertion
except where the subtask says the pinned behaviour changes. Stop condition for all: report
files changed, every test whose expectation changed (with the decision that changed it), and
verification results, then await review.

#### 139.1 — The name table and canonical names

Scope: `freminal-common/src/buffer_states/pointer_shape.rs` (production and tests).

What:

- `PointerShape::from_name(&str) -> Option<Self>`: the name table in "139 Decisions",
  case-sensitive, no trimming, `None` for anything else (including the empty string).
- `PointerShape::css_name(self) -> &'static str`; `Display` delegates to it. `ResizeHorizontal`
  is `ew-resize` and `ResizeVertical` is `ns-resize`.
- `From<&str>` stays for this subtask only, as a shim with today's semantics
  (`from_name(name.trim()).unwrap_or_default()`), so the parser keeps compiling; 139.3 removes
  it. Its doc comment says so.
- Tests: every table entry maps as listed; `css_name(from_name(n)) == n` for the 30 spec names
  and the three extras; `from_name(css_name(v)) == Some(v)` for every variant; case and
  whitespace variants are `None`; unknown names are `None`; the shim keeps today's behaviour.
  `display_all_pointer_shape_variants` changes for the two resize names (decision: canonical
  names). `unknown_name_maps_to_default` keeps testing the shim.

#### 139.2 — Per-screen pointer-shape stacks in the handler

Scope:

- `freminal-terminal-emulator/src/terminal_handler/pointer_shape_stack.rs` (new) and its
  declaration in `terminal_handler/mod.rs`;
- `terminal_handler/mod.rs` (the field, its initialiser, its doc, `pointer_shape()`, the
  `soft_reset` doc, and the OSC 22 handler tests);
- `terminal_handler/osc.rs` (the `SetPointerShape` arm only);
- `terminal_handler/reset.rs` (classification);
- `freminal-common/src/buffer_states/pointer_shape.rs` (only to add `PointerShapeEntry`);
- `freminal-terminal-emulator/tests/reset_table.rs`.

What:

- `PointerShapeEntry` as in "139 Types" (in `freminal-common`).
- `PointerShapeStack` as in "139 Types", with the kitty semantics: `set` replaces the top (or
  pushes onto an empty stack); `push` evicts the bottom at `POINTER_SHAPE_STACK_DEPTH`; `pop`
  removes one entry and is a no-op when empty; `current` is `None` for an empty stack or an
  `Unset` top.
- The handler field is `ScreenScoped<PointerShapeStack>`. `pointer_shape()` returns the active
  screen's `current().unwrap_or_default()` (drop `const` if needed).
- Interim mapping until 139.3: `SetPointerShape(shape)` performs `set(Shape(shape))` on the
  active screen's stack. This is behaviour-neutral for the visible shape.
- `reset.rs`: both kinds clear both stacks (`both_mut()`), replacing the single-field reset.
- Tests: the stack's own unit tests (set on empty, set replaces top, push order, eviction at
  16 and 17, pop to empty, pop on empty, unset top hides lower entries and a pop reveals them);
  handler tests that a set on the alternate screen does not change the primary's shape and
  survives a `?1049l` / `?1049h` round trip; RIS and DECSTR each clear both screens' stacks
  (`reset_table.rs`; there is no RIS pointer test there today).

#### 139.3 — Typed OSC 22 operations, parser and replies

Scope:

- `freminal-common/src/buffer_states/pointer_shape.rs` (`PointerShapeQuery`, `PointerShapeOp`,
  `MAX_OSC22_NAMES`; remove the `From<&str>` shim and the tests that pin only it);
- `freminal-common/src/buffer_states/osc.rs` (`AnsiOscType` variant, its `Display`, the
  `OscTarget::PointerShape` doc, tests);
- `freminal-terminal-emulator/src/ansi_components/osc.rs` (a raw-body OSC 22 arm replacing
  `with_osc_tokens` + `handle_osc_pointer_shape`; tests);
- `freminal-terminal-emulator/src/terminal_handler/osc_pointer.rs` (new: applying an op and
  building the reply) and its declaration in `terminal_handler/mod.rs`;
- `terminal_handler/osc.rs` (the arm delegates to `osc_pointer.rs`);
- `terminal_handler/mod.rs` (the OSC 22 handler tests).

What:

- The parser turns the body after `22;` into one `PointerShapeOp` following "139 Decisions":
  the op prefix, `,` splitting, the 64-name cap, per-name lookup through `from_name`, empty
  names kept only as `Unset` in a set, empty and unknown names dropped from a push, names
  ignored for a pop, and each query item classified (`__current__`, `__default__`,
  `__grabbed__`, otherwise `Supported` / `Unsupported` from `from_name`). A non-UTF-8 name is
  unknown. Ignored names are logged at `debug!` with their length only.
- The handler applies `Set` entries in order, `Push` shapes in order, `Pop` once, on the active
  screen's stack. `Query` writes exactly one reply,
  `write_osc_response(&format!("22;{list}"))`: `1` / `0`, the `css_name` of `current()` or `0`,
  and `default` for both `__default__` and `__grabbed__`. Set, push and pop send nothing.
- Tests, parser level: each op and prefix; no prefix with a list (last wins); `=` with an empty
  name; `22` with no `;`; a name containing `;`; leading space is unknown; uppercase is unknown;
  the cap (64 names honoured, the 65th ignored); non-UTF-8. Handler level, through
  `handle_incoming_data`: the kitty examples (`?__current__` gives `0` on an empty stack,
  `?pointer,crosshair,no-such-name,wait` gives `1,1,0,1`, `?__default__,__grabbed__` gives
  `default,default`, an alias query gives `1`); `?__current__` after `=left_ptr` gives
  `default` and after `>fleur` gives `move`; push of 17 evicts the bottom; empty set then pop
  restores the lower shape; BEL-terminated query gets an ST reply; S8C1T framing (`0x9D … 0x9C`);
  the reply precedes a following DA1; set/push/pop send nothing.
- Update every test that pinned the old variant or the reset-on-unknown behaviour, listing each.

#### 139.4 — End-to-end OSC 22 suite

Scope: `freminal-terminal-emulator/tests/osc22_pointer_shapes.rs` (new), and
`fuzz/seeds/terminal_state/` (one or two OSC 22 seed files; check the directory's naming
convention first).

What: an end-to-end suite through `TerminalState::handle_incoming_data` and `build_snapshot`
asserting observable state only (snapshot `pointer_shape` and reply bytes): per-screen stacks
across `?47`, `?1047` and `?1049`; the alternate stack persisting across sessions; RIS from the
alternate screen clearing both and returning to the primary with the default shape; DECSTR on
the alternate screen clearing both; OSC 22 inside tmux passthrough (reply unwrapped); a
mixed-op single read (`>wait`, `>help`, `<`, `?__current__` gives `wait`). Seed files exercise
every op.

#### 139.5 — Documentation

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`,
`Documents/KITTY_PROTOCOL_REFERENCE.md`.

What: the dual-document update per `freminal-escape-sequence-docs`: the OSC 22 rows (now ✅,
with the decided semantics, the cap and the two differences from kitty), the RIS and DECSTR
rows ("both OSC 22 pointer-shape stacks"), the GAPS row removed or closed, both "Last updated"
headers; a new OSC 22 section in the kitty reference (against kitty 0.49.2) and its roadmap row.
Verification: markdownlint and `prettier --check` on the three files.

### 139 Status notes

- **139.1 (done).** `PointerShape::from_name` (the one table: 30 spec names, three extras,
  `col-resize` / `row-resize`, 46 kitty aliases) and `css_name`; `Display` delegates. Every
  name the old `From<&str>` accepted maps identically through the temporary shim. Changed
  expectation: `display_all_pointer_shape_variants` (`ew-resize`, `ns-resize`; canonical-name
  decision).
- **139.2 (done).** `PointerShapeEntry` and `PointerShapeStack` (depth 16, kitty set / push /
  pop semantics); the handler holds `ScreenScoped<PointerShapeStack>`, and RIS and DECSTR clear
  both. Interim: `SetPointerShape` performs a set. `push` / `pop` carry a temporary
  `expect(dead_code)` (`TODO(139.3)`). New RIS pointer test in `reset_table.rs`; no changed
  expectations.
- **139.3 (done).** `AnsiOscType::PointerShape(PointerShapeOp)` replaces `SetPointerShape`;
  the `From<&str>` shim and the 139.2 interim mapping are gone. Orchestrator scope change: the
  body parser lives in a new `ansi_components/osc_pointer_shape.rs` (the `osc_colors.rs`
  precedent) instead of growing `osc.rs`; the handler side is `terminal_handler/osc_pointer.rs`.
  A set or push whose names were all unknown is emitted with an empty list (a no-op). Changed
  expectations: the empty-name and no-`;` parser tests now expect `Set([Unset])`; the old
  `from(&str)` tests assert through `from_name` (empty, whitespace and unknown are `None`);
  OSC 22 left `tokenising_targets_with_non_utf8_byte_still_invalid`, since a non-UTF-8 name is
  now an unknown name, not `Invalid`. Plan correction: an empty set on a one-entry stack
  replaces that entry, so "empty set then pop restores the lower shape" needs two entries.
- **139.4 (done).** `tests/osc22_pointer_shapes.rs` (16 tests through `TerminalEmulator` and
  `build_snapshot`: per-screen stacks for `?47` / `?1047` / `?1049`, alternate persistence,
  RIS and DECSTR, tmux passthrough with an unwrapped reply, mixed ops in one read, reply order
  between DSR and DA1, S8C1T framing) and two `terminal_state` fuzz seeds; `fuzz-replay` is
  clean. No production bug found.
- **139.5 (done).** COVERAGE (OSC 22 rows ✅; RIS / DECSTR rows; OSC 22 left the token-target
  list), GAPS (OSC 22 row removed), and a new "Pointer shapes (OSC 22)" section in the kitty
  reference with the three deliberate differences from kitty. Prettier re-padded the COVERAGE
  OSC table.

### 139 Adversarial review (2026-11-19)

One CODE-REVIEW pass over `main...HEAD` against the kitty spec, kitty's source, this section and
the repo rules. No BLOCKER or MAJOR. The wire behaviour, the name table, the stack semantics,
the reply framing and ordering, the per-screen stacks and the resets were all confirmed
against kitty. Findings and dispositions:

| ID  | Sev   | Finding (short)                                                         | Disposition                  |
| --- | ----- | ----------------------------------------------------------------------- | ---------------------------- |
| R1  | minor | `PointerShape` / module docs still describe a reset, not a stack        | 139.C1                       |
| R2  | minor | `seventeenth_push_evicts_the_bottom` passes without eviction            | 139.C1                       |
| R3  | minor | MASTER_PLAN Completion note and two "decomposed" lists stale            | 139.C2                       |
| R4  | minor | MASTER_PLAN and the 130 status table no longer prettier-clean           | 139.C2                       |
| R5  | minor | COVERAGE "Dispatch" bullet lists stale token targets                    | 139.C2                       |
| R6  | minor | no handler/e2e test of a >16-name push or a >64-name query reply        | 139.C1                       |
| R7  | minor | `none` / `context-menu` / `all-scroll` are outside the spec table       | 139.C1 (remove the extras)   |
| R8  | minor | possible `redundant_closure_for_method_calls` in `osc_pointer.rs`       | not a defect (clippy clean)  |
| R9  | minor | unrelated Task 150 status commit on the branch                          | 139.C2 (PR description note) |
| R10 | nit   | kitty 0.51.0 shipped; baseline still 0.49.2                             | 139.C2                       |
| R11 | nit   | grammar in the kitty reference header                                   | 139.C2                       |
| R12 | nit   | parser splits for a pop; `iter.count()` scans the whole body            | 139.C1                       |
| R13 | nit   | duplicated name tests; `ALL_VARIANTS` not compile-time exhaustive       | 139.C1                       |
| R14 | nit   | stale assertion messages in `terminal_handler/mod.rs` OSC 22 tests      | 139.C1                       |
| R15 | nit   | snapshot `pointer_shape` doc imprecise                                  | 139.C1                       |
| R16 | nit   | fuzz seeds and dictionary miss the cap and non-UTF-8                    | 139.C1                       |
| R17 | nit   | a >64-name query reply has 64 items, so positions past 64 are lost      | not a defect (decision kept) |
| R18 | nit   | GAPS says "closed" before merge                                         | 139.C2 (re-check at merge)   |
| R19 | nit   | cleanup section empty                                                   | this entry                   |

- **R7 (decision changed):** the spec defines the valid names and says `__current__` answers
  "a name from the table … or 0"; `none`, `context-menu` and `all-scroll` are not in it, and
  `none` lets a program hide the pointer over the pane until the next reset. The extras are
  removed from the name table, and with them the now-unreachable `PointerShape::None`,
  `ContextMenu` and `AllScroll` variants and their GUI arms. `col-resize` / `row-resize` stay:
  they are aliases and report the canonical `ew-resize` / `ns-resize`. This supersedes the
  "freminal's existing extras" bullet of "139 Decisions".
- **R8:** `cargo clippy --all-targets --all-features -- -D warnings` was run by the
  orchestrator after 139.3 and is clean: `css_name` is a `const fn` taking `self`, which the
  lint does not suggest replacing here.
- **R17:** answering `0` for names past the cap would make the reply unbounded again (about one
  byte per input byte up to the 1 MiB OSC cap). The cap decision stands; it is documented.

### 139 Cleanup entries

#### 139.C1 — Review fixes: code, tests and the name-table extras

- **Surfaced:** adversarial review R1, R2, R6, R7, R12–R16.
- **Scope:** `freminal-common/src/buffer_states/pointer_shape.rs`;
  `freminal-terminal-emulator/src/ansi_components/osc_pointer_shape.rs`;
  `terminal_handler/osc_pointer.rs`; `terminal_handler/mod.rs` (OSC 22 test messages);
  `freminal-terminal-emulator/src/snapshot.rs` (field doc);
  `freminal-terminal-emulator/tests/osc22_pointer_shapes.rs`; `freminal/src/gui/terminal/widget.rs`
  (the three removed variants' arms and the test that uses `None`); `fuzz/seeds/terminal_state/`;
  `fuzz/dict/ansi.dict`; the OSC 22 claims about the extras in `ESCAPE_SEQUENCE_COVERAGE.md` and
  `KITTY_PROTOCOL_REFERENCE.md`.
- **Fix:** rewrite the stale docs (R1, R15); make the eviction test fail without eviction (R2);
  add a >16-name single push and a >64-name query reply test (R6); remove the extras (R7);
  split only for set/push/query and stop counting the remainder (R12); drop the exactly
  duplicated tests and add a compile-time exhaustive variant guard (R13); reword the messages
  (R14); add a seed with >64 names and a non-UTF-8 name, and `>`, `<`, `?__current__`
  dictionary tokens (R16).
- **Verification:** the suite, clippy, `check-windows`, `fuzz-replay`, markdownlint.

#### 139.C2 — Review fixes: plan and doc bookkeeping

- **Surfaced:** adversarial review R3–R5, R9–R11, R18.
- **Fix (orchestrator):** MASTER_PLAN Completion note and both "decomposed" lists (R3); the
  table widths (R4); the COVERAGE "Dispatch" bullet (R5); state the 0.51.0 re-check in the
  kitty reference: its changelog's OSC 22 fix (#10838) is about kitty's own pointer refresh on
  a screen switch, which freminal gets from rebuilding the snapshot (R10); the header grammar
  (R11). R9 and R18 are PR-time checks: the PR description names the Task 150 status commit,
  and the GAPS wording is re-checked at merge.
- **Status: done (both).** 139.C1: the extras and their three variants are removed (a query
  for them answers `0`); a test-only `declare_variants!` macro builds `ALL_VARIANTS` from the
  same list as an exhaustive `match`, so a missing variant fails to compile (it replaces the
  suggested index guard, which would not have caught an omission); the eviction test fails
  without eviction; new e2e tests for a 20-name push and a 70-name query; the parser no longer
  splits for a pop or counts the remainder; new seed `osc22_cap_and_invalid_utf8` and three
  dictionary tokens. Deleted tests: three exact duplicates and the three tests of the removed
  extras (now pinned as "not names"). 139.C2: as listed above; the MASTER_PLAN and 130 status
  tables were re-padded for the narrower Status column. The implementing sub-agent briefly ran
  `git stash` / `git stash pop`; the orchestrator confirmed every plan edit survived.
  Orchestrator verification at `98cc4ac5`: `cargo xtask test` (9667 passed), clippy,
  `cargo fmt --check`, `check-windows`, `cargo machete`, `fuzz-replay` (every target ok),
  markdownlint and prettier on the touched docs, all clean.

#### 139.C3 — Confirmation-review fixes

- **Surfaced:** the confirmation review of `98cc4ac5` (below), findings N1–N10.
- **Fix:** see "139 Confirmation review".

### 139 Confirmation review (2026-11-19)

A second CODE-REVIEW pass over `98cc4ac5` against R1–R19: R2, R4–R7, R10, R12–R16 and R19
FIXED; R3 and R11 PARTIAL (closed by N5 and N3 below); R8, R9, R17 and R18
ACCEPTED-AS-DOCUMENTED. No BLOCKER or MAJOR. New findings, all fixed in 139.C3:

- **N1 (minor):** `col-resize` / `row-resize` are outside the spec table too, and kitty does
  not know them, yet they were not listed as a difference. **Decision:** they stay. R7 removed
  the extras because `__current__` would report a non-spec name and `none` could hide the
  pointer; neither applies to these two, which are reported by their canonical spec names.
  They are now the fourth documented difference from kitty (kitty reference, COVERAGE, GAPS).
- **N2 (minor):** the recon and decision bullets about the extras now say R7 removed them.
- **N3, N4, N6 (nits):** kitty reference header grammar; the 0.51.0 note now covers both halves
  of #10838; three seeds and the cap tests are listed.
- **N5 (minor):** the MASTER_PLAN Completion note.
- **N7 (nit):** the duplicate eviction test and the vacuous half of
  `all_variants_lists_every_variant_exactly_once` are removed. The per-shape name tests in
  `pointer_shape.rs` are kept deliberately: `agents.md` accepts test duplication that improves
  clarity, and each names one shape family.
- **N8 (nit):** the R-table is re-padded (prettier-clean).
- **N9 (nit, pre-existing):** the misplaced `pointer_shape_to_cursor_icon` doc comment in
  `widget.rs` is moved and corrected, and a test pins explicit icons for that function.
- **N10 (nit):** verification results are recorded above.

**Merged** 2026-11-19 via PR #586 (`9fb9d63a`).

---

## Task 140 — Underline & SGR Parity

### 140 Summary

**Goal.** SGR and underline behaviour matches the kitty underline spec
(<https://sw.kovidgoyal.net/kitty/underlines/>) and the SGR part of the misc protocol
(<https://sw.kovidgoyal.net/kitty/misc-protocol/>):

- SGR 21 is double underline;
- `4:N` follows kitty's rules;
- SGR 221 and 222 reset bold and faint independently;
- erased cells take only the background colour;
- underline geometry is clamped to the cell and tiles per cell;
- the shipped terminfo advertises styled and coloured underlines.

Activated and decomposed on 2026-11-19 on branch `task-140/underline-sgr-parity`, from `main`
at `9fb9d63a` (after PR #586).

### 140 Activation recon (2026-11-19)

Three read-only passes: the emulator SGR, erase and terminfo paths; the GUI underline renderer;
and reference-terminal source (kitty `cursor.c`, `line.c`, `decorations.c`, `terminfo.py`;
xterm `charproc.c`, `screen.c`, `ptyx.h`; Ghostty `sgr.zig`, `Screen.zig`, `style.zig`;
WezTerm `csi.rs` and `terminalstate/mod.rs`). Results, stub claim by stub claim:

- **SGR 21 is bold-off: confirmed.** `freminal-common/src/sgr.rs` maps `21` to `ResetBold`,
  and the handler clears bold only. kitty, xterm, Ghostty and WezTerm all treat 21 as double
  underline. Only `sgr_reset_bold_clears_font_weight` pins the apply side, and it tests the
  `ResetBold` variant, not the number 21.
- **`4:N` with N>5 clears the underline: confirmed.** `handle_underline_subparam`
  (`ansi_components/csi_commands/sgr.rs`) maps through `UnderlineStyle::from_sgr_param`, which
  returns `None` for anything but 1–5. An empty `4:` also clears it. Pinned by
  `sgr_colon_underline_style_unknown_value_clears` (`tests/sgr_underline_styles.rs`). kitty
  clamps with `MIN(5, N)`; Ghostty and WezTerm draw a single underline; xterm ignores `4:N`.
- **SGR 221/222 are missing: confirmed.** Both fall to `Unknown`. kitty: 221 clears bold only,
  222 clears faint only, 22 clears both. Ghostty and WezTerm do not implement them. Bold
  (`FontWeight`) and faint (`FontDecorations::Faint`) are already separate, so no storage
  change is needed.
- **Underline geometry: confirmed.** `push_underline_quads` (`freminal/src/gui/renderer/vertex.rs`)
  never sees the cell height, so double (down to `ul_y + 3*thick + 1`) and curly
  (`ul_y ± 2*thick`, plus `thick`) can leave the cell. Dotted and dashed start at the run's
  `x0`, so their phase restarts at every text run, and their pitches do not divide the cell
  width. On DECDWL rows `x0`/`x1` are scaled but the pattern's `cell_width` is not. There are
  no geometry tests, and only `Single` is ever constructed in tests. kitty draws each style
  as a per-cell sprite clamped to the cell, so patterns tile across cells.
- **Default underline colour wrong under SGR 7 + DECSCNM: refuted.** Fixed by 132.C5 and
  pinned by `default_underline_follows_the_effective_foreground_under_sgr7_and_decscnm`.
  Nothing to do.
- **`Su`/`Smulx`/`Setulc` absent from `res/freminal.ti`: confirmed.** XTGETTCAP answers all
  three (`terminal_handler/dcs.rs` `lookup_termcap`). `build.rs` rebuilds `res/terminfo.tar`
  with `tic` when the source changes; `tic` is in the dev shell. kitty's `terminfo.py` defines
  `Su` (boolean), `Smulx=\E[4:%p1%dm` and a colon-form `Setulc`.
- **Erase copies the whole format tag: confirmed.** ED, EL, ECH, ICH and DCH (and the DECLRMM
  column-scroll fills) write `Cell::blank_with_tag(current_tag.clone())`, so erased cells get
  fg, bold, underline, reverse, blink and even an open OSC 8 URL. The alternate-screen clear
  (Task 131) is already background-only. References split two against two: xterm (colour
  flags only, `ClearCells`) and Ghostty (`blankCell`, background only) on one side; kitty
  (`line_apply_cursor`, full pen) and WezTerm (`clone_sgr_only`) on the other.
- **New finding: the BCE docs over-claim.** IL, DL, SU, SD and line-feed scrolling fill new
  rows with default attributes (`scroll.rs`: "Scroll-created blank rows use default background
  (no BCE)"). Only the DECLRMM column-confined variants use the current tag. COVERAGE and
  `DESIGN_DECISIONS.md` say IL and DL are BCE-aware.
- **No benchmark corpus contains decorations**, so the decoration build has no meaningful
  regression bench today.

### 140 Decisions (maintainer, 2026-11-19)

1. **SGR 21 is double underline**, as in all four reference terminals. SGR 24 clears it.
2. **`4:N` follows kitty.** N>5 clamps to dashed; an empty `4:` subparameter is a single
   underline; `4:0` is no underline.
3. **Erased cells take only the current background colour** (xterm and Ghostty, the standing
   tie-break; consistent with the Task 131 alternate-screen clear). This also stops an open
   OSC 8 hyperlink leaking into erased cells.
4. **Whole-row IL/DL/scroll fills stay default-attributed** (kitty parity; keeps scrollback rows
   sparse). The DECLRMM column-confined fills take the background-only erase tag (decision 3).
   Only the docs that claim otherwise are corrected.
5. **Underline geometry: clamp every decoration to the cell box, and lay dotted, dashed and
   curly out per cell, anchored to the cell grid**, so a pattern is the same whatever the run
   boundaries. Position and thickness still come from the font metrics.

### 140 Decisions (orchestrator, recorded so they are not re-litigated)

- **kitty is GPLv3, freminal is MIT.** kitty is read for observable behaviour only. The
  per-cell layouts below are freminal's own, specified here, with the same intent as
  kitty's sprites; no kitty code is copied.
- **SGR numbers.** `21` maps to `UnderlineWithStyle(Double)`; `221` maps to the existing
  `ResetBold` (bold off only); `222` maps to a new `ResetFaint` (faint off only). `22` keeps
  clearing both. DECRQSS already reports `4:2` for a double underline, so no reply change.
- **Empty `4:`.** freminal's SGR tokeniser cannot tell a trailing `4:` from `4:;…`; kitty's
  differing answer for the second form (no underline) is an artefact of its parser. Both are a
  single underline in freminal. Recorded as a difference from kitty.
- **`4:N` clamping lives in the SGR handler**, not in `UnderlineStyle::from_sgr_param`, which
  keeps its strict 0–5 mapping for any other caller.
- **The erase tag.** It is `FormatTag::default()` with
  `StateColors::default().with_background_color(current_tag.colors.background_color)`: the
  stored background, not swapped by SGR 7 (xterm `cur_background`). One `Buffer` helper builds
  it, and every BCE site uses it, including `clear_alternate_screen`. Row sparsity rules are
  unchanged.
- **Per-cell geometry.** Let `cw` be the drawn cell width (`cell_width * scale` on DECDWL rows),
  `ch` the cell height, `top` the row's top and `bot = top + ch`, and for cell `c` of a run
  `cx = c * cw`.
  - **Thickness and position:** `t = clamp(stroke_size.max(1), 1, ch)`;
    `ul_y = clamp(top + ascent - underline_offset, top, bot - t)`.
  - **Single:** one quad `[x0, x1) × [ul_y, ul_y + t)`, unchanged apart from the clamp.
  - **Double:** two lines of height `t` separated by a gap of `t + 1` (today's spacing). If the
    lower line would pass `bot`, the lower line moves to `bot - t` and the upper to
    `bot - t - (t + 1) - t`; the upper is finally clamped to `≥ top`. Each line is one quad
    across the run.
  - **Curly:** one period per cell, `y = centre - amp * cos(2π (x - cx) / cw)`, so every cell
    starts and ends at the same height and adjacent cells join. `amp = min(2t, …)` reduced so
    `centre ± amp` plus `t` stays inside `[top, bot)`; `centre = ul_y`, moved up only as far as
    needed to fit. `CURLY_SEGMENTS` strips per cell, as today.
  - **Dotted:** per cell, `n = max(1, floor(cw / (2t)))` dots, slot `s = cw / n`, dot width
    `d = max(1, min(t, floor(s / 2)))`, dot `i` at `x = cx + round((i + 0.5) s - d / 2)`.
  - **Dashed:** per cell, `q = floor(cw / 4)`, dash width `w = cw - 3q`; dashes at
    `[cx, cx + w)` and `[cx + 3q, cx + cw)`. The right dash of one cell and the left dash of
    the next join into one longer dash.
  - **Strikethrough** is clamped the same way as single.
- **Terminfo.** `Su` (boolean), `Smulx=\E[4:%p1%dm` and `Setulc` use exactly the strings
  freminal's XTGETTCAP already returns, so the two cannot disagree.

### 140 Execution model

Serial, on one checkout (`task-140/underline-sgr-parity`). Each subtask is implemented by a
sub-agent, reviewed by the orchestrator against this text (the whole diff, every new file read
in full), verified again by the orchestrator, and only then committed by the orchestrator.
Order: 140.1, 140.2, 140.3, 140.4, 140.5. An adversarial review of `main...HEAD` against this
section and the kitty spec follows 140.5; every finding, nit and above, becomes a `140.Cn`
entry and is fixed (or routed to the task that owns the code) before the PR.

### 140 Subtasks

Every subtask's verification is `cargo xtask test` and
`cargo clippy --all-targets --all-features -- -D warnings`. Prohibitions for every subtask: do
NOT touch files outside scope, do NOT commit, do NOT update plan documents, do NOT proceed to
the next subtask; no `unwrap`/`expect` outside tests, no raw numeric `as` casts (`conv2`), no
new `bool` fields or parameters, no `#[allow(dead_code)]`; do not delete or weaken a test
assertion except where the subtask says the pinned behaviour changes. Stop condition for all:
report files changed, every test whose expectation changed (with the decision that changed it),
and verification results, then await review.

#### 140.1 — SGR 21, `4:N`, 221 and 222

Scope: `freminal-common/src/sgr.rs` (and its tests);
`freminal-terminal-emulator/src/ansi_components/csi_commands/sgr.rs`;
`freminal-terminal-emulator/src/terminal_handler/sgr.rs`;
`freminal-terminal-emulator/tests/sgr_underline_styles.rs` and any SGR test that pins 21 or
`4:N>5`.

What:

- `21` → `UnderlineWithStyle(UnderlineStyle::Double)`; `221` → `ResetBold`; `222` → new
  `ResetFaint`, which removes `FontDecorations::Faint` only.
- `handle_underline_subparam`: N>5 is dashed; an empty subparameter is single; `4:0` is none.
- Tests: `21` gives a double underline and DECRQSS `4:2`; `24` after `21` clears it; `21`
  leaves bold alone; `221` clears bold and keeps faint; `222` clears faint and keeps bold; `22`
  clears both; `4:6` and `4:255` are dashed; `4:` is single; `from_usize` tests for 21, 22, 221
  and 222. `sgr_colon_underline_style_unknown_value_clears` changes (decision 2).

#### 140.2 — Erased cells take only the background colour

Scope: `freminal-buffer/src/buffer/` (`erase.rs`, `lines.rs`, `scroll.rs` column fills,
`resize_and_alt.rs` `clear_alternate_screen`, and wherever the new helper lives);
`freminal-buffer/src/row.rs` only if a signature must change; `freminal-buffer/tests/` erase
and BCE tests; `freminal-terminal-emulator/tests/` only for a new end-to-end test.

What:

- One `Buffer` helper builds the erase tag as decided. ED 0/1/2, EL 0/1/2, ECH, ICH, DCH (both
  variants), the DECLRMM column-scroll fills and `clear_alternate_screen` use it.
- Tests: for each operation, an active tag with fg, bold, underline, reverse, blink, an OSC 8
  URL and a background leaves erased cells with the background only; reverse with no background
  gives default cells (and the row stays sparse); a default tag keeps rows sparse; the
  alternate-screen clear is unchanged; end to end through `handle_incoming_data`,
  `ESC[1;4;31;44m ESC[K` gives blank cells with only the blue background.
- Benchmark: `bench_erase_display`, `bench_erase_display_full`, `bench_erase_display_bce`
  before and after (15%).

#### 140.3 — Underline geometry: clamped and per cell

Scope: `freminal/src/gui/renderer/vertex.rs` (`push_underline_quads` and its helpers,
`UnderlineParams`, the strikethrough quad, tests); `freminal/benches/render_loop_bench.rs`.

What:

- **First**, add a decoration-heavy corpus to `render_loop_bench.rs` (every underline style
  plus strikethrough, with colour changes splitting runs) and an `instanced_bg_decorated`
  bench; capture the baseline on the unchanged renderer.
- Implement the per-cell geometry exactly as decided. `UnderlineParams` gains the cell height
  and the drawn cell width.
- Tests, on the `deco` buffer: every quad of every style stays inside the cell box, including
  for an offset below the cell, a negative offset and a thickness larger than the cell; a run of
  six cells and the same cells split into runs of two and four produce the same quad positions;
  on a DECDWL row the pattern scales with the cell; the two double lines do not overlap when
  they fit; curly is continuous at cell boundaries; dashed joins across a boundary.
- Benchmark: the new bench and `instanced_bg` before and after (15%).

#### 140.4 — Terminfo advertises styled and coloured underlines

Scope: `res/freminal.ti`; `res/terminfo.tar` (regenerated by the build); a test beside
`lookup_termcap` in `freminal-terminal-emulator/src/terminal_handler/dcs.rs` or the terminfo
module; `freminal-terminal-emulator/build.rs` (only to pass `-x` to `tic`; scope widened at
execution, see the 140.4 status note).

What: add `Su`, `Smulx` and `Setulc` to the `xterm-freminal` entry with exactly XTGETTCAP's
strings. Rebuild the tar through `build.rs`. A test reads `res/freminal.ti` (`include_str!`)
and checks that each of the three is present and matches `lookup_termcap`. Verify with
`infocmp -x` on the compiled entry and record the output.

#### 140.5 — Documentation

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`,
`Documents/SGR.md`, `Documents/DESIGN_DECISIONS.md` (the BCE paragraph),
`Documents/KITTY_PROTOCOL_REFERENCE.md` (an underlines section against kitty 0.49.2).

What: the dual-document update per `freminal-escape-sequence-docs`: SGR 21, `4:N`, 221/222
(❌ → ✅), erase attributes, the IL/DL/scroll fill correction, terminfo; the GAPS rows removed
or closed; both "Last updated" headers; the difference from kitty for `4:;`. Verification:
markdownlint and `prettier --check` on the named files.

### 140 Status notes

- **140.1 (done).** `21` is `UnderlineWithStyle(Double)`, `221` is `ResetBold`, `222` is the
  new `ResetFaint`; `22` is unchanged. `handle_underline_subparam` clamps N>5 to dashed
  (`MAX_UNDERLINE_STYLE_PARAM`), and an empty `4:` is single. Changed expectation:
  `sgr_colon_underline_style_unknown_value_clears` became
  `sgr_colon_underline_style_above_five_is_dashed` (decision 2).
- **140.2 (done).** `Buffer::erase_tag()` (background only, not swapped by SGR 7) is used by
  ED, EL, ECH, ICH, DCH, the DECLRMM column-confined fills (so IL/DL under DECLRMM take it) and
  `clear_alternate_screen`. Whole-row IL/DL/SU/SD and line feeds are unchanged. Plain DCH writes
  no fill; its tag only reaches a split wide glyph's remnant, which is tested (superseded by
  140.C2: plain DCH now fills the vacated right edge). New
  `freminal-buffer/tests/erase_bce_tests.rs` (17) and
  `freminal-terminal-emulator/tests/erase_bce_end_to_end.rs` (4); 19 of the 21 fail on the old
  code (the two that pass are the sparse-row and whole-row guards). Changed expectation:
  `insert_space_tests::ich_preserves_shifted_tags` (ICH blanks of a bold tag are now default;
  decision 3). Benchmarks (no change detected by Criterion): `erase_display` −7.8%,
  `erase_display_full` −2.7%, `erase_display_bce` +4.7%.
- **140.3 (done).** Per-cell geometry as decided, in a new out-of-line `push_run_decorations`
  (kept out of `build_background_instances` so the undecorated path stays tight); strikethrough
  is clamped with the same `clamp_to_cell`. **Deviation, accepted:** a dashed cell whose two
  dashes would touch or overlap (`w ≥ 3q`, e.g. 6–7 px cells, or under 4 px) is one solid dash;
  at the default 8 px cell the layout is exactly the spec's. New `instanced_bg_decorated` bench
  and corpus, baseline captured before the change. 11 geometry tests (box clamps across seven
  metric cases, run-boundary independence, DECDWL scaling, double spacing and relocation,
  curly continuity, dash joins, dot spacing); no existing assertion changed. Orchestrator rerun
  on an idle machine against `before_140_3`: worst `instanced_bg_indexed` 80x24 +9.5%,
  `instanced_bg_decorated` −17% to −20% and `instanced_bg` −20% to −27% (the baseline was taken
  under load, so the improvements are overstated; superseded by the idle re-measure in the
  140.C4 status).
- **140.4 (done).** `Su`, `Smulx` and `Setulc` added to `xterm-freminal` with XTGETTCAP's strings.
  **Scope widened at execution:** `build.rs` ran `tic` without `-x`, which drops user-defined
  capabilities with only a warning, so the new entries would never have reached
  `res/terminfo.tar`. `tic` now gets `-x`; the orchestrator checked that `-x` alone leaves the
  existing entry unchanged (`infocmp -x` identical). `infocmp -x` on the entry extracted from the
  regenerated tar shows all three. New test
  `terminfo_source_advertises_styled_underline_caps_matching_xtgettcap` ties the source to
  `lookup_termcap`. A rebuild leaves the tar byte-identical. **Caveat found after commit
  (maintainer question):** children run with `TERM=xterm-256color`, so no program looks up the
  bundled `xterm-freminal` entry; the capabilities reach programs only through XTGETTCAP. The
  docs say so. Making the bundled terminfo actually reach programs is outside Task 140; see
  140.C1.
- **140.5 (done).** COVERAGE, GAPS, `SGR.md`, the BCE section of `DESIGN_DECISIONS.md` (which
  wrongly called IL/DL BCE-aware and described the format as an erase parameter) and a new
  "Underlines and SGR parity" section in the kitty reference with five documented differences
  from kitty (three after 140.C4 and 140.C5: the narrow-cell dash and the whole-row fills
  turned out to match kitty). Orchestrator
  fix: COVERAGE said every dashed cell under 8 px may be solid; only 6–7 px and sub-4 px cells
  are.

### 140 Cleanup entries

#### 140.C1 — The bundled terminfo entry is never used

- **Surfaced:** maintainer question after 140.4 (2026-11-19). Predates Task 140.
- **Impact:** `res/terminfo.tar` is embedded and extracted for every PTY, and children get
  `TERMINFO_DIRS=<tempdir>:`, but they also get `TERM=xterm-256color`
  (`freminal-terminal-emulator/src/io/pty.rs` ~600–633). The temp dir holds only `freminal` and
  `xterm-freminal`, so no program ever reads the bundled entry; ncurses resolves
  `xterm-256color` from the system database. The flake package does not install the entry
  either (`postInstall` copies icons only). Capabilities beyond `xterm-256color` reach programs
  only through XTGETTCAP, and only programs that query it.
- **Options (maintainer decision pending):** (1) keep `xterm-256color` and delete or
  document-as-reference the bundled entry and its extraction; (2) an opt-in `[shell] term`
  option (default `xterm-256color`, or `xterm-freminal`) plus installing the entry to
  `$out/share/terminfo`; (3) default `TERM=xterm-freminal` as kitty and Ghostty do, which breaks
  ssh to hosts without the entry until SSH integration (Task 86) and reverses the recorded TERM
  decision (Task 92 was dropped for this reason).
- **Verification, whichever option:** an integration test that spawns a real PTY child with the
  production environment (the same code path as `run_terminal`) and runs `tput`/`infocmp -x`
  inside it, asserting what the child actually resolves. Under today's code it would show the
  entry is unreachable.
- **Scheduling:** moved to Task 151 (maintainer decision 2026-11-19: freminal defaults to its
  own `TERM` and ships the terminfo entry for remote hosts). Not blocking the Task 140 PR.

### 140 Adversarial review (2026-11-19)

One CODE-REVIEW pass over `main...HEAD` against the kitty underline and misc-protocol pages,
this section and the repo rules. No blocker. Every finding, nit and above, is fixed:

| ID  | Sev   | Finding (short)                                                         | Disposition |
| --- | ----- | ----------------------------------------------------------------------- | ----------- |
| R1  | major | plain DCH never fills the vacated right edge, docs say it does          | 140.C2      |
| R2  | minor | DECDHL decorations are not scaled vertically like the glyphs            | 140.C3      |
| R3  | minor | an overflowing `4:N` is `Invalid`, not dashed; no `4:2:3` / `21:x` test | 140.C2      |
| R4  | minor | `erase_tag` lives in `resize_and_alt.rs`, not `erase.rs`                | 140.C2      |
| R5  | minor | decoration geometry grew `vertex.rs`; give it `renderer/decorations.rs` | 140.C3      |
| R6  | minor | narrow-cell solid dashes match kitty; docs call them a difference       | 140.C4      |
| R7  | minor | 140.3 benchmarks compared against a baseline taken under load           | 140.C4      |
| R8  | minor | geometry tests miss zero height, DECDHL, wide glyphs, tiny cells        | 140.C3      |
| R9  | nit   | redundant decoration early-out in `push_run_decorations`                | 140.C3      |
| R10 | nit   | `erase_tag()` rebuilt per row in the ED loops                           | 140.C2      |
| R11 | nit   | `sgr_22_*` tests duplicate the existing `NormalIntensity` tests         | 140.C2      |
| R12 | nit   | BCE history wording, meta-notes in COVERAGE rows, a ragged wrap         | 140.C4      |
| R13 | nit   | 140.4 note lacks the `infocmp` output; new caps sit mid-entry in `.ti`  | 140.C4      |
| R14 | nit   | Task 151 must list the "`TERM=xterm-256color`" doc sentences            | 140.C4      |

#### 140.C2 — Emulator and buffer review fixes

- **R1:** plain DCH (no DECLRMM) fills the `n` vacated cells at the right edge with
  `erase_tag()`, as xterm and kitty do; with a default background the row stays sparse (no
  explicit cells). Tests: a loud tag fills the right edge with the background only; a default
  tag leaves the row sparse; a split wide glyph still works.
- **R3:** a `4:` subparameter that overflows `usize` saturates (dashed) instead of making the
  segment `Invalid`; tests for that, for `4:2:3` (double, extra colons ignored) and `21:x`
  (double underline).
- **R4:** `erase_tag` moves to `buffer/erase.rs`. **R10:** hoisted out of the ED/EL row loops.
- **R11:** drop `sgr_22_clears_both_bold_and_faint` and the integration `sgr_22_resets_*`
  (existing `NormalIntensity` tests cover them).
- **Status: done.** R1 lives in `Row::delete_cells_at`: with a non-default erase tag the
  vacated columns (`logical_end - start`, wider than `n` when a wide glyph widens the deletion)
  take the tag, and a DCH past the stored content still fills the edge; a default tag keeps the
  row sparse. Five new DCH tests, four of which fail on the old code (the sparse-row one pins
  existing behaviour). R3: `parse_colon_parts` saturates only the first `4:` subparameter;
  colour overflow and `4:x` stay `Invalid`. R11 is covered by
  `sgr_normal_intensity_clears_bold_and_faint` and the CSI `22m` assertion. R4: `erase_tag`
  moved to `erase.rs`; R10: hoisted out of the ED row loops (EL touches one row). No DCH
  benchmark exists.

#### 140.C3 — Renderer review fixes

- **R5:** the decoration geometry (`push_run_decorations`, `UnderlineParams`, `clamp_to_cell`,
  the style helpers) and its tests move to a new `freminal/src/gui/renderer/decorations.rs`;
  pure move plus the R2 change.
- **R2:** on DECDHL rows the decoration is laid out in the doubled box (the pair's top at the
  top half's row top, height `2 * cell_height`, ascent, offsets and stroke doubled), exactly as
  the glyph is scaled, then each quad is clipped to the row's own visible cell and dropped if
  empty. So the underline appears once, on the half where the doubled glyph's underline falls.
- **R8:** tests for `cell_height == 0` (no quads), DECDHL top and bottom halves, runs with a
  two-column glyph, cells 2–4 px high for curly and double; the run-boundary test uses a stroke
  whose old dot pitch did not divide the cell, so it fails on the old code.
- **R9:** remove the redundant early-out.
- **Status: done.** The move left the count at 9727; `effective_fg` and `x_scale` became
  `pub(super)` (inside `renderer`) for the new module. DECDHL uses a `RowLayout` that mirrors
  the glyph path exactly (doubled box at the pair top, bottom half shifted up one cell) and
  `clip_quads_to_rows`. Seven new tests (9734); breaking the doubled scale or the clip fails four
  each. The `STROKE_3` run-boundary case fails on pre-140.3 code (old dot pitch 6 restarted at
  each run: 8 quads whole, 9 split).

#### 140.C4 — Docs and plan review fixes

- **R1:** DCH rows and BCE summaries match the fixed code. **R2:** COVERAGE notes DECDHL
  decorations. **R6:** the narrow-cell dash is "matches kitty", not a difference (four
  differences remain). **R12:** wording fixes. **R13:** the `infocmp` output in the 140.4 note;
  move `Su`/`Smulx`/`Setulc` to the end of the entry. **R14:** list the docs to update in
  Task 151. **R7:** re-measure 140.3 against a baseline of the pre-140.3 commit taken on an idle
  machine, and record it.
- **Status: done.** R1, R2, R6, R12 and R14 in COVERAGE, GAPS, `DESIGN_DECISIONS.md`, the
  kitty reference and the Task 151 stub (sub-agent; orchestrator also fixed two wraps in this
  plan that markdown parsed as list items). **R13** (orchestrator): `Su`/`Smulx`/`Setulc` moved
  to the end of the entry; `res/terminfo.tar` is byte-identical afterwards. A first attempt with
  an indented `#` comment line made `tic` swallow `Su` into a bogus name, and the 140.4 test
  caught it; the comment was dropped. `infocmp -x -A` on the entry extracted from the tar:

  ```text
  am, bce, ccc, hs, km, mc5i, mir, msgr, npc, xenl, Su,
  Setulc=\E[58:2::%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%d%;m,
  Smulx=\E[4:%p1%dm,
  ```

  **R7** (orchestrator): the pre-140.3 commit (`78791a73`) with the current bench file, measured
  idle as `pre140_3_idle`, against the branch head (after 140.C3). Everything is within 15%:
  `instanced_bg` −3.5% / +0.5%, `instanced_bg_decorated` −0.2% / −0.8% (no change),
  `instanced_bg_indexed` −2.1% / −0.6%, `instanced_bg_partial_dirty` +1.0% to +9.0% (worst
  `one_row` `sparse_10_percent` +9.0%). The earlier −20% figures were load artefacts.

### 140 Confirmation review (2026-11-19)

A second CODE-REVIEW pass over `fd0870ec..HEAD` and `main...HEAD`: R1–R13 FIXED (R3 and R12
with a nit each, R7 not reproducible by reading), R14 PARTIAL. No blocker. New findings, all
fixed in 140.C5:

| ID  | Sev   | Finding (short)                                                         |
| --- | ----- | ----------------------------------------------------------------------- |
| N1  | minor | whole-row IL/DL fills match kitty, yet are listed as a difference       |
| N2  | nit   | DCH sentence broke the BCE tie-break antecedent; ragged wrap            |
| N3  | nit   | stale comment in the split-wide-glyph DCH test                          |
| N4  | minor | `CSI <huge> P` overflows `col + n` / `start + n` (debug panic)          |
| N5  | nit   | Task 151 cites a `DESIGN_DECISIONS.md` TERM section that does not exist |
| N6  | nit   | Task 151 list misses the FREC `term` metadata and `FREC_FORMAT.md`      |
| N7  | nit   | `DESIGN_DECISIONS.md` DECDWL/DECDHL section is stale                    |
| N8  | nit   | R3 says `21:x` is tested; the test is `21:5` (`21:x` is `Invalid`)      |
| N9  | nit   | DCH tests miss a short row, last column, `n = 0`, end-to-end `CSI P`    |
| N10 | nit   | quad layout indices duplicated in `clip_quads_to_rows` and test helper  |
| N11 | nit   | no benchmark exercises the DECDHL decoration clip                       |
| N12 | nit   | plan notes: 140.C2 status omits R4/R10, 140.3 note lacks a pointer      |

#### 140.C5 — Confirmation-review fixes

- **Orchestrator (docs and plan):** N1 (three differences; the whole-row fills recorded as a
  match), N2 and N7 (`DESIGN_DECISIONS.md`), N5 and N6 (Task 151 stub), N12 (status notes).
  The 140.C4 commit is typed `docs:` although it also reordered `res/freminal.ti` (no
  compiled change); recorded here rather than rewriting history.
- **Sub-agent (code):** N4 saturating arithmetic at every count-driven DCH/ICH/ECH site, with
  huge-count tests and a `terminal_state` fuzz reproducer; N3; N8 (`21:x` is `Invalid`, tested);
  N9 tests; N10 a shared quad-decode helper beside `quad_vertices`; N11 a DECDHL decorated
  bench.
- **Status: done.** N4 used `saturating_add` at `row.rs` (`delete_cells_at`,
  `erase_cells_at`), `lines.rs` (the DCH image sweep, both branches) and `erase.rs` (the ECH
  image sweep, which also overflowed); the ICH paths and DECLRMM DCH already clamped `n`.
  The huge-count tests panic on the old arithmetic; reproducer
  `fuzz/regressions/terminal_state/dch-huge-count-overflow`, and `fuzz-replay` is clean for
  every target. N9: for `ABCDE`, col 1, `n = 2`, the remaining text is `ADE` in columns 0–2,
  columns 3–7 are default blanks and 8–9 take the fill (the entry above had the blank range
  wrong). N10: `QUAD_FLOATS`, `QuadBounds` and `quad_bounds` beside `quad_vertices`, with a
  round-trip test. N11: `instanced_bg_decorated_dhl` (every row pair DECDHL): 35.3 µs at 80x24
  and 194.5 µs at 200x50, against 27.6 µs and 152.2 µs for the same corpus without DECDHL
  (first measurement, no earlier baseline exists). Tests: 9748.

#### 140.C6 — PR #587 review comments

- **Copilot (DCH on doubled rows):** on DECDWL/DECDHL rows the visible row is half the
  buffer width, and text already wraps at the halved margin, but DCH (and ICH and ECH) shift,
  fill and count image cells across the full width. The new DCH fill therefore lands in the
  hidden half and the visible right edge stays default. **Declined (maintainer, 2026-11-19):
  matches xterm.** xterm's stored-cell operations run across the full row width
  (`ScrnDeleteChar` / `ScrnInsertChar` take `ScrnRightMargin`, which is `MaxCols - 1` without
  DECLRMM; `ClearRight` uses `MaxCols`); only text writes use the halved `LineMaxCol`. xterm's
  double-width branch in `DeleteChar` / `InsertChar` only paints the fill at the visible edge,
  and a later repaint shows the stored cells. kitty and Ghostty do not implement DECDWL.
  Four tests in `erase_bce_tests.rs` pin the full-width behaviour (DCH, ICH and ECH on a
  DECDWL row, DCH on a DECDHL row). Tests: 9755.
- **CodeRabbit, outside the diff (`row.rs` `delete_cells_at_with_right_limit`):** the DECLRMM DCH
  path returns early when the cursor is past the stored cells, so its margin edge is never
  filled (the case 140.C2 fixed for plain DCH). **Fixed:** the early return now applies only
  when the tag is visually default; `dch_under_declrmm_past_stored_cells_fills_the_margin_edge`
  fails on the old code. ICH under DECLRMM never had the early return (guard test added).
- **CodeRabbit (decision 4 wording):** "IL/DL/scroll fills" now says whole-row, and names the
  column-confined fills. Fixed by the orchestrator.
- **CodeRabbit (emoji in the new SGR 221/222 table rows):** the rule is real, but both files
  use emoji status cells throughout (196 in COVERAGE, 29 in `SGR.md`). **Maintainer decision:**
  keep them in the new rows for consistency and convert the whole files in one pass, scheduled
  as 147.33.

**Merged** 2026-11-19 via PR #587 (`13f73832`).

---

## Task 151 — Own TERM & Terminfo Distribution (activated, v0.13.0)

**Goal.** Programs running in freminal know they are in freminal, and get freminal's real
capabilities from terminfo, not only from XTGETTCAP. Children get `TERM=xterm-freminal` by
default, the bundled entry is accurate, and hosts that do not install freminal can be given
the entry. Created 2026-11-19 from 140.C1; reverses the recorded "TERM Strategy" decision
(the comment in `io/pty.rs` and the Task 12 and Task 92 notes in `MASTER_PLAN.md`) that Task 92
was dropped under.

**Findings (140.C1).**

- `res/terminfo.tar` is embedded and extracted per PTY into a temp dir on `TERMINFO_DIRS`, but
  children get `TERM=xterm-256color`, so nothing reads it.
- The flake package does not install the entry (`postInstall` copies icons only), and neither
  module installs it.
- The `.ti` file was kept "as a reference" (source comment in `io/pty.rs`), so it has never
  been exercised: it must be audited before it becomes authoritative.
- XTGETTCAP `TN` is pinned to `xterm-256color` by a regression test (Task 76.6), so that it
  cannot desync from `TERM`.
- Task 86 (SSH integration, which would copy the entry to remote hosts) is a v0.15.0 stub; it is
  not done.

**Durable decisions (maintainer, 2026-11-19).**

1. **Children get `TERM=xterm-freminal` by default.** The `xterm-` prefix keeps programs that
   match `xterm*` working (as `xterm-kitty` and `xterm-ghostty` do).
2. **`[shell] term` config option**, default `xterm-freminal`, so a user can go back to
   `xterm-256color` (or anything). Full `freminal-config-options` wiring. Layout and pane `env`
   still override it.
3. **Flake `packages.<system>.terminfo`**: only `$out/share/terminfo`, with no GUI
   dependencies, for servers that do not install freminal. The main package also installs the
   entry under `$out/share/terminfo`.
4. **A NixOS module option** (for example `programs.freminal.terminfo.enable`) that adds the
   terminfo package to the system for headless servers. The home-manager module stays for
   desktops.
5. **XTGETTCAP `TN` reports the pane's actual `TERM`**, so it can never desync. The Task 76.6
   regression test changes accordingly.

**Required at activation.**

- **Audit the entry first** (READ-ONLY): every capability in `res/freminal.ti` checked against
  what the parser and handler do, and every capability XTGETTCAP answers present in the entry.
  A wrong capability is worse than a missing one once programs trust it.
- **The PTY-spawn test** from 140.C1: spawn a real child with the production environment and run
  `tput` / `infocmp -x` inside it, asserting the entry resolves with the expected values; keep
  the per-PTY `TERMINFO_DIRS` extraction so it resolves even where nothing is installed.
- **One table of truth** shared by the `.ti` and `lookup_termcap`, or a test that compares them
  for every capability (140.4's test covers three).
- Docs: a new `DESIGN_DECISIONS.md` section for the TERM strategy (none exists today; the
  reversed decision, with reasons), the ssh consequence
  and how to fix it (install the terminfo package,
  `infocmp -x xterm-freminal | ssh host tic -x -`, or set `[shell] term`), COVERAGE/GAPS,
  README.
- **Update every sentence that says children get `TERM=xterm-256color`** (found 2026-11-19
  with `rg xterm-256color Documents/ freminal*/src`; re-run the search at activation):
  - `ESCAPE_SEQUENCE_COVERAGE.md`: the Task 140 "Last updated" entry ("child processes get
    `TERM=xterm-256color`, so the entry is used only if a user sets `TERM=xterm-freminal`") and
    the SGR row ("programs get `TERM=xterm-256color` and discover these through XTGETTCAP").
  - `ESCAPE_SEQUENCE_GAPS.md`: the Task 140 "Last updated" entry ("programs get
    `TERM=xterm-256color`, so what they discover at runtime comes from XTGETTCAP").
  - `KITTY_PROTOCOL_REFERENCE.md`, "Underlines and SGR parity", Terminfo bullet ("Child
    processes are started with `TERM=xterm-256color`, so that entry is used only if a user sets
    `TERM=xterm-freminal`").
  - `MASTER_PLAN.md`: the Task 12 dependency note ("Strategy: stay with `TERM=xterm-256color`
    (matching WezTerm/Alacritty)") and the Task 92 "Dropped Tasks" entry ("freminal deliberately
    advertises `TERM=xterm-256color`"; it already carries a Task 151 addendum).
  - `PLAN_VERSION_090.md` (Task 76.6 notes): "XTGETTCAP `TN` stays `xterm-256color`" and
    "`io::pty::run_terminal` sets `TERM=xterm-256color` as the documented Task-12 compatibility
    strategy".
  - `PLAN_VERSION_150.md`: "Whether a `freminal` TERM is introduced at all (today:
    `TERM=xterm-256color` + XTGETTCAP per Task 12)".
  - `PLAN_VERSION_130.md` itself: the 140.4 status note ("children run with
    `TERM=xterm-256color`") and 140.C1 are history and stay.
  - Code comments: `freminal-terminal-emulator/src/io/pty.rs` (the "TERM Strategy" comment and
    the `cmd.env("TERM", "xterm-256color")` block, ~605-633) and
    `freminal-terminal-emulator/src/terminal_handler/dcs.rs` (the `TN` comment, "Freminal
    deliberately presents as `xterm-256color`", and the Task 76.6 regression test near it).
  - FREC recording metadata hard-codes the value: `freminal/src/main.rs` (`term:
    "xterm-256color"`) and `freminal/src/gui/recording.rs` (the same), plus the example in
    `Documents/FREC_FORMAT.md`; record the pane's actual `TERM` instead.

**Open questions.**

- macOS: does the system ncurses read the extended (`tic -x`) entry from `TERMINFO_DIRS`?
  Verify on the macOS CI job before relying on it.
- Windows (ConPTY): whether `TERM` is set at all today, and whether anything changes.
- What freminal does if `[shell] term` names an entry it does not ship (pass through, with a
  warning).

  All three are resolved below (151 decisions and 151.9).

### 151 Activation (2026-11-19)

Activated and decomposed on 2026-11-19 on branch `task-151/own-term`, from `main` at
`13f73832` (after PR #587), in the worktree `../freminal-151`. It runs alongside Task 147
(`task-147/correctness-cleanup`) and has its own PR.

**Packaging recon (2026-11-19).**

- The entry is compiled by `freminal-terminal-emulator/build.rs` (`tic -x`) into
  `res/terminfo.tar`. The tar holds two legacy-format (magic 0432) files in Linux letter
  directories, `x/xterm-freminal` and `f/freminal`, with no hex (`78/`, `66/`) copies. The tar
  is embedded in the binary (`freminal-common/src/terminfo.rs`).
- Each PTY extracts it into a temp dir held by the PTY I/O object (deleted when the pane
  closes). Children get `TERMINFO_DIRS=<tempdir>:` and `TERMINFO` is removed. On Windows
  nothing is extracted, but `TERM` is still set. The Windows comment about symlinks in the
  tar is stale.
- No package installs the entry: not the cargo-bundle `.app`, `.deb` or AppImage
  (`[package.metadata.bundle]` has no `resources`), not the rpm (`generate-rpm` `assets`),
  and not the flake (`postInstall`). The flake has no NixOS module.
- freminal has no notion of its own resource directory beyond `shell_integration_dir()`
  (`FREMINAL_RESOURCES_DIR`, `XDG_DATA_DIRS`, per-user dir). `current_exe()` and `APPDIR`
  are not used anywhere.
- Release packaging is in `.github/workflows/deploy.yaml` and `nightly.yml`. The deb and
  AppImage are repacked after `cargo bundle` by `assets/ci/fix-linux-icon-metadata.sh`.

**Decisions (maintainer, 2026-11-19).**

1. **147.22 moves to Task 151** (as 151.2), so Task 151 owns `lookup_termcap` and
   `res/freminal.ti` outright and the table-of-truth check covers `indn` and `rin`. <!-- codespell:ignore rin -->
2. **macOS `.app`: bundle copy plus fallback** (the kitty and Ghostty model). The entry ships
   in `Contents/Resources/terminfo/` in both letter and hex layouts, signed with the app.
   freminal finds it from `current_exe()` and puts it first on `TERMINFO_DIRS` (superseded by
   151.C1: it is named by `TERMINFO`). The raw
   binary (no bundle) keeps the per-pane extraction. `sudo` (drops `TERMINFO_DIRS`) is a
   known limit, as in kitty and Ghostty.
3. **AppImage: per-pane extraction only.** Nothing extra ships in the AppImage. Users who
   want the entry system-wide (sudo, detached tmux servers) install the deb, the rpm, the
   flake `terminfo` package or the NixOS module option. The detached-process limit is
   documented.
4. **A fallback that always works (2026-11-19, added after activation).** In addition to the
   planned distribution, the README documents installing the entry on any host, with or
   without freminal present, as Ghostty's terminfo help page does:
   - from inside a freminal pane (every build, the AppImage included, because each pane has
     the entry on `TERMINFO_DIRS`): `infocmp -x xterm-freminal | ssh HOST -- tic -x -`;
   - from anywhere, without freminal: the source file on GitHub,
     `curl -fsSL https://raw.githubusercontent.com/fredsystems/freminal/main/res/freminal.ti | tic -x -`
     (or piped through `ssh HOST -- tic -x -`). `res/freminal.ti` therefore becomes a public
     URL and must not be moved or renamed;
   - a version-pinned copy: every release and nightly carries `freminal.ti` as an asset.
     `releases/latest/download/…` is not used, because every release today is a prerelease
     and GitHub's `latest` skips prereleases.
   - `tic -x` run without root installs to `~/.terminfo`, which needs no privileges.
5. **`sudo` keeps `TERMINFO` through the shell integration (2026-11-19, added after
   activation), exactly as Ghostty does.** `sudo`'s default `env_reset` drops `TERMINFO`, so
   programs run under `sudo` lose the pane's entry. freminal's bash, zsh and fish integrations
   wrap `sudo` to pass `--preserve-env=TERMINFO`, except for `sudo -e` / `--edit`
   (sudoedit), which is called unchanged. It is **on by default** whenever the shell
   integration is active, with a config toggle to opt out (maintainer decision; unlike
   Ghostty, whose `sudo` feature defaults off). A sudoers policy that forbids preserving the
   environment makes `sudo` refuse the command; those users turn the toggle off.

**Decisions (orchestrator, recorded so they are not re-litigated).**

- **Both directory layouts everywhere.** The embedded tar and the bundle copy carry letter
  (`x/`, `f/`) and hex (`78/`, `66/`) directories. Apple's ncurses uses hex directories; the
  extra copies cost about 5 KB and remove the need to know which one a given ncurses reads.
- **The compiled tree is committed.** `build.rs` keeps a committed `res/terminfo/` tree (both
  layouts, deterministic) in step with `res/freminal.ti`, as it already does for the tar, and
  the tar is packed from that tree. cargo-bundle's `resources` and the rpm `assets` point at
  the committed files, so packaging needs no `tic` at bundle time.
- **`TERMINFO_DIRS` order** (superseded by 151.C1, which names the pane's directory with
  `TERMINFO` and leaves `TERMINFO_DIRS` alone) is: the `.app` bundle directory (macOS, when it exists), then the
  per-pane temp dir (only when there is no bundle directory), then the system default (the
  trailing empty entry). An installed system copy is found through the default.
- **Windows keeps `xterm-256color` as its default.** There is no terminfo distribution path on
  Windows, and MSYS2/Cygwin programs run under ConPTY read their own terminfo databases, which
  do not have `xterm-freminal`. The `[shell] term` option is honoured on every platform.
- **The configured `TERM` is passed through unchecked**, apart from validation that it is a
  non-empty name with no control characters, `/` or whitespace. An invalid value falls back
  to the platform default with a config warning. freminal does not check that the named entry
  exists (it cannot see the remote side of an ssh session anyway).
- **Precedence.** A layout or pane `env` `TERM` wins over `[shell] term`. The effective value
  is computed once at spawn and is the single value used for the child's environment,
  XTGETTCAP `TN` and the FREC `term` metadata.
- **No new install command.** Task 92 stays dropped; remote copying stays with Task 86.

### 151 Audit (151.1, 2026-11-19)

A read-only audit compared every capability in `res/freminal.ti` with the parser, handler and
input encoder, and every `lookup_termcap` name with the entry (system ncurses 6.6 `infocmp -x`
for `xterm-256color`, `xterm-kitty`, `xterm-ghostty`, `foot`, `wezterm`; `tic -x -c -v2`).
The entry is standalone (no `use=`); `tic` accepts it with four warnings. Every capability not
listed below is implemented and value-correct.

| Item                                     | Finding                                                                                                                                                                   | Decision (151.3)                                                    |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| `invis`, `sgr` p7                        | SGR 8 parses to `Conceal`, which `apply_sgr` ignores                                                                                                                      | Remove both; record SGR 8 as a GAPS row                             |
| `km`                                     | Alt sends no 8th bit and there is no `smm`/`rmm` (`?1034` not implemented)                                                                                                | Remove (foot omits it)                                              |
| `acsc` `hh`, `++,,--..00`                | `h` maps to U+2424, not a board glyph; `+ , - . 0` print as ASCII                                                                                                         | Remove; match `xterm-256color`                                      |
| `ich1`                                   | Works, but no reference entry has it and `tic` warns                                                                                                                      | Remove                                                              |
| `kcbt`, `kent`                           | Standard values, never sent (Shift+Tab sends HT; keypad Enter sends CR)                                                                                                   | Keep (as `xterm-256color`); the encoder is Task 137 B3/B11          |
| `kHOM` `kEND` `kNXT` `kPRV` `kind` `kri` | Correct, but default keybindings consume the keys first                                                                                                                   | Keep; documented                                                    |
| Header                                   | Description is one word; line 1 cites a stale macOS path                                                                                                                  | Fix                                                                 |
| XTGETTCAP `Se`                           | `\E[2 q` (steady block); freminal's DECSCUSR 0 restores the configured style                                                                                              | `\E[0 q` (kitty, Ghostty, Alacritty)                                |
| XTGETTCAP `TN`                           | `xterm-256color`                                                                                                                                                          | The effective `TERM` (151.6)                                        |
| Undeclared, implemented                  | `Tc` `setrgbf` `setrgbb` `Ss` `Se` `Ms` `Cs` `Cr` `BD` `BE` `PS` `PE` `fd` `fe` `kxIN` `kxOUT` `E3` `XR` `XM` `Sync` `smxx` `rmxx` `AX` `XF` `nel`, modified-key families | Add, each value checked against freminal and cited from a reference |
| Undeclared, not added                    | `RGB` (string form unverified), `xm`, `RV`, `is2`/`rs2`, `smglp`/`smglr`/`smgrp` (DECSLRM argument handling unverified)                                                   | Not added; listed in the table-of-truth test with the reason        |

Where both sides answer a capability, the entry and `lookup_termcap` use identical strings
(OSC 52 `Ms` keeps ST, as kitty and foot do; `setrgbf`/`setrgbb` keep the semicolon form
freminal already answers). `Sync` is the `?2026` form, not kitty's DCS form.

### 151 Subtasks

Every subtask leaves `cargo xtask test` and
`cargo clippy --all-targets --all-features -- -D warnings` green, and runs
`cargo xtask check-windows` where it touches platform code. Standing prohibitions: no files
outside scope, no commits, no plan-document edits, no next subtask; no `unwrap`/`expect`
outside tests, no raw numeric `as` casts, no new `bool` fields or parameters, no
`#[allow(dead_code)]`; no deleted or weakened assertion unless the subtask names the
behaviour change. A `flake.nix` change that alters a dev shell stops for `nix develop`
(`flake-dev-shell-discipline`).

#### 151.1 — Terminfo entry audit (READ-ONLY) (done; see "151 Audit")

Every capability in `res/freminal.ti` is checked against what the parser and handler actually
do (with the code location that implements it, or "missing"), and every name
`lookup_termcap` answers is checked for presence and an identical value in the entry. The
`xterm-256color` capabilities freminal does not implement, and the ones freminal implements
that neither entry declares, are listed. Deliverable: the table, folded into this section as
"151 Audit". No edits.

#### 151.2 — XTGETTCAP answers `indn`, `rin` and `query-os-name` (moved from 147.22) <!-- codespell:ignore rin -->

The 147.22 text applies unchanged (scope `terminal_handler/dcs.rs` `lookup_termcap` and its
tests), plus: `indn` and `rin` are added to `res/freminal.ti` with the same strings, so the <!-- codespell:ignore rin -->
two cannot disagree. `query-os-name` is XTGETTCAP-only (it is not a terminfo capability).

#### 151.3 — Entry corrections and one table of truth

Scope: `res/freminal.ti`, `lookup_termcap`, their tests, and the SGR 8 row in
`ESCAPE_SEQUENCE_GAPS.md`. Apply every decision in "151 Audit" (each change cites its row). Replace the three-capability
test from 140.4 with one that parses `res/freminal.ti` (`include_str!`) and checks, for every
name `lookup_termcap` answers that is a terminfo capability, that the entry has the same
value, and the reverse for every string capability in the entry that XTGETTCAP should answer.
Names deliberately answered by only one side are listed in the test with a reason.

#### 151.4 — Both layouts, and a committed compiled tree

Scope: `freminal-terminal-emulator/build.rs`, `res/terminfo/` (new, generated),
`res/terminfo.tar`, `freminal-common/src/terminfo.rs` (docs), the stale Windows comment in
`io/pty.rs`, `.coderabbit.yaml` (exclude the new binary tree). `build.rs` writes the compiled
entries into `res/terminfo/{x,f,78,66}/` deterministically (only when the bytes change) and
packs the tar from that tree. A test checks that the tar holds all four paths with identical
bytes, and that the tar's members equal the committed tree.

#### 151.5 — `[shell] term`

The full `freminal-config-options` checklist: `ShellConfig::term`, `ConfigPartial` /
`apply_partial`, validation (as decided), the Settings modal, `config_example.toml`, the
home-manager module. The default is resolved per platform (`xterm-freminal`; Windows
`xterm-256color`). Tests: round trip, partial apply, defaults per platform (pure function
over a platform enum), invalid values.

#### 151.6 — The effective `TERM` reaches the child, XTGETTCAP and FREC

Scope: `freminal-terminal-emulator/src/io/pty.rs` (`run_terminal`, the TERM comment),
`interface.rs` and the handler (a typed effective-`TERM` value given at construction),
`terminal_handler/dcs.rs` (`TN` reports it; the Task 76.6 regression test now checks that `TN`
equals the child's `TERM`), `freminal/src/gui/pty.rs`, `tab_spawning.rs` and `layout_ops.rs`
(computing the effective value with layout/pane `env` precedence), `freminal/src/main.rs`,
`freminal/src/gui/recording.rs` and `freminal-terminal-emulator/src/recording.rs` (FREC
`term`). Tests: config default, config override, pane `env` override, `TN` and FREC agree with
the child's value in each case.

#### 151.7 — macOS bundle terminfo

Scope (amended at execution, see the 151.7 status note): the release workflows' macOS
staging step instead of cargo-bundle `resources`, `io/pty.rs` (a pure function from the
executable path to an optional bundle terminfo directory, and the `TERMINFO_DIRS` order as
decided), `flake.nix` (the Darwin `postInstall` copies the tree into
`Freminal.app/Contents/Resources/terminfo`). Tests: the pure resolver on every platform
(inside a `.app`, a raw binary, a symlinked `$out/bin/freminal`); the `TERMINFO_DIRS` builder
for each case. The release workflow is checked by reading only; the macOS CI job compiles it.

#### 151.8 — Linux and Nix packaging

Scope: `freminal/Cargo.toml` (rpm `assets`: `/usr/share/terminfo/{x,f}/…`),
`assets/ci/fix-linux-icon-metadata.sh` (add `usr/share/terminfo` to the repacked `.deb` only;
the AppImage is unchanged, per decision 3), `flake.nix` (`packages.<system>.terminfo`, only
`$out/share/terminfo`, no GUI dependencies; the main package installs the tree too;
`nixosModules.default`), a new `nix/nixos-module.nix` (`programs.freminal.terminfo.enable`
adds the terminfo package to `environment.systemPackages`). Verification adds `nixfmt`,
`statix`, `deadnix`, `nix flake check` and `nix build .#terminfo`.

#### 151.11 — `freminal.ti` as a release asset and a stable URL

Scope: `.github/workflows/deploy.yaml` and `nightly.yml` (attach `res/freminal.ti` to the
release, named `freminal.ti`, through the existing release-upload step), the `res/freminal.ti`
header comment (the file is linked from the README by its raw `main` URL, so its path is
public), and a test that the path exists and compiles from stdin (`tic -x -o <tmp> -` with the
file on stdin, skipped with a visible message when `tic` is absent). Runs after 151.8, before
151.9. The README text is 151.10.

#### 151.12 — `sudo` preserves `TERMINFO` (on by default; Ghostty's wrapper logic)

Scope: `freminal-common/src/config.rs` (`ShellIntegrationConfig`: a new TOML toggle, default
on, full `freminal-config-options` checklist including Settings, `config_example.toml` and
the home-manager module), the spawn-time shell-integration environment in
`freminal-terminal-emulator/src/io/pty.rs` (tell the scripts the feature is on, as Ghostty
does with `GHOSTTY_SHELL_FEATURES`), the bundled bash, zsh and fish integration scripts, and
their tests. The wrapper follows Ghostty's `ghostty.bash`, `ghostty-integration` (zsh) and
`ghostty-shell-integration.fish` logic exactly (read their source; cite it): it is defined
only when the feature is on and `TERMINFO` is set, scans the arguments for `-e` / `--edit`
before the first non-option argument and then calls `sudo` unchanged, and otherwise calls
`command sudo --preserve-env=TERMINFO "$@"`. Docs: the README Terminfo section and the
`DESIGN_DECISIONS.md` sudo limit. Runs after 151.C1 (same spawn code).

#### 151.9 — A real child resolves the entry

Scope: a new integration test in `freminal-terminal-emulator/tests/`. It spawns a real PTY
child through the production spawn path, with the production environment, and runs
`infocmp -x xterm-freminal` (and `tput Smulx 3`) in it, asserting that the entry resolves
with the expected values. It runs on the Linux and macOS CI jobs, which settles the macOS
open question; it is skipped with a visible message only when `infocmp` is not installed.
Not on Windows.

#### 151.10 — Documentation

Scope: a new "TERM strategy" section in `DESIGN_DECISIONS.md` (the reversed decision and its
reasons, the `TERMINFO_DIRS` order, the sudo and detached-process limits, and how to fix ssh:
install the terminfo package, `infocmp -x xterm-freminal | ssh host tic -x -`, or set
`[shell] term`), a README "Terminfo" section with decision 4's three commands (in-pane
`infocmp`, the raw GitHub URL, the release asset), the `~/.terminfo` note and the
`[shell] term = "xterm-256color"` escape hatch, every sentence in the list above, COVERAGE/GAPS (XTGETTCAP rows from 151.2,
`TN`), README, `FREC_FORMAT.md`.

### 151 Execution model

Serial inside the worktree: 151.1, 151.2, 151.3, 151.4, 151.5, 151.6, 151.7, 151.8, 151.11,
151.9, 151.10, 151.C1, 151.12. Each subtask is implemented by a sub-agent and reviewed in full by the orchestrator
(whole diff, every new file read, compared with this section) before the orchestrator commits
it. Then an adversarial review of `main...HEAD`; every finding, nit and above, becomes a
`151.Cn` entry and is fixed before the PR, followed by a confirmation review. Files shared
with Task 147 (`config.rs`, `settings.rs`, `config_example.toml`, the home-manager module,
COVERAGE/GAPS) are reconciled by rebasing whichever PR merges second.

### 151 Cleanup entries

#### 151.C1 — A stale `~/.terminfo` entry overrides the pane's own

- **Surfaced:** 151.9 (2026-11-19). On the development machine the child resolved
  `~/.terminfo/x/xterm-freminal` rather than the per-pane copy.
- **Impact:** ncurses searches `~/.terminfo` before `TERMINFO_DIRS`, so any per-user copy wins
  over the bundle or per-pane copy, which always matches the running freminal. Decision 4's
  fallback (`curl … | tic -x -` without root) installs exactly such a per-user copy, so a
  user who installed it once keeps that version after upgrading freminal.
- **Scope of fix:** `freminal-terminal-emulator/src/io/pty.rs` (`run_terminal`, the
  `TERMINFO`/`TERMINFO_DIRS` comment and setup), its tests, `tests/terminfo_child.rs`.
- **Suggested approach:** first establish, from the ncurses source (`read_entry.c`,
  `db_iterator.c`) and Apple's ncurses, the real search order and whether `TERMINFO` is
  exclusive (the existing code comment says it is; kitty and Ghostty set `TERMINFO` to their
  bundled directory and other `TERM` values keep working). Do not infer it. If `TERMINFO` is
  searched first and not exclusively, set `TERMINFO` to the pane's directory (bundle or
  per-pane) as kitty and Ghostty do, keep `TERMINFO_DIRS` for the rest, and test that
  `xterm-freminal` resolves from the pane's directory with a stale `~/.terminfo` copy present
  and that `TERM=xterm-256color` still resolves from the system database.
- **Verification:** `terminfo_child.rs` without the empty-`HOME` workaround, plus a stale
  `~/.terminfo` copy in a temp `HOME`.
- **Scheduling:** within Task 151, after 151.10 and before the adversarial review.
- **Status: Resolved (2026-11-19).** Research (ncurses 6.6 `include/term_entry.h:94-109`,
  `tinfo/db_iterator.c:312-365`, `read_entry.c:902-910`; Apple's `ncurses-79` and ncurses 5.7
  identical; experiments): the order is `$TERMINFO`, `$HOME/.terminfo`, `$TERMINFO_DIRS`, the
  compiled-in default, and `TERMINFO` is not exclusive (the first directory with the entry
  wins; misses fall through). kitty (`child.py`) and Ghostty (`Exec.zig`) set `TERMINFO` and
  leave `TERMINFO_DIRS` alone. `run_terminal` now does the same (`apply_terminfo_env`), and
  the old comment calling `TERMINFO` exclusive is corrected. `terminfo_child.rs` plants a
  stale `colors#8` copy in the child's `~/.terminfo` and checks the pane's entry wins (and
  that the stale one would win without `TERMINFO`), and that `TERM=xterm-256color` still
  resolves from the system database. README and `DESIGN_DECISIONS.md` updated. Surfaced
  decision 5 (151.12).
- **151.12 (done).** `[shell_integration] sudo_preserve_terminfo` (TOML toggle, default
  `true`, under the `set_term_program` master switch; full config checklist). The GUI
  converts it to `SudoTerminfo { Preserve, Unchanged }` (`io/shell_features.rs`, no `Default`),
  carried by `PtyTabConfig`, `TerminalEmulator::new` (now ten parameters, under its existing
  `too_many_arguments` allow) and `PtySpawnConfig`; the spawn sets
  `FREMINAL_SHELL_FEATURES=sudo` only when the integration is injected and the toggle is on,
  and removes an inherited value otherwise. The bash, zsh and fish wrappers reproduce Ghostty
  at `d69f9ed8` (`ghostty.bash` L89-115, `ghostty-integration` L305-327,
  `ghostty-shell-integration.fish` L97-120), cited in each script; bash and zsh read the
  variables as `${VAR:-}` (safe under `set -u`). As in Ghostty, the scan stops at the first
  argument that is neither an option nor an assignment, so `sudo -u root -e file` is wrapped
  and `sudo -e -u root file` is not (pinned by a test, noted in `DESIGN_DECISIONS.md`). Script
  tests run all three shells against a fake `sudo`.

### 151 Adversarial review (2026-11-19)

One CODE-REVIEW pass over `main...HEAD` against this section and the repo rules. No blocker.
`cargo xtask test`, clippy, `check-windows`, `machete`, markdownlint and the Nix lints were
clean; the committed entry is byte-identical to a fresh `tic -x` (ncurses 6.6); the sudo
wrappers match Ghostty at `d69f9ed8` line for line.

| ID  | Sev   | Finding (short)                                                              | Disposition |
| --- | ----- | ---------------------------------------------------------------------------- | ----------- |
| R1  | major | `tic -x` inside a pane writes into the pane's `$TERMINFO` (temp/bundle dir)  | 151.C3      |
| R2  | minor | a user's own `TERMINFO` is overwritten; decision not recorded                | 151.C2      |
| R3  | minor | `FREMINAL_SHELL_FEATURES` is removed only on an injected spawn               | 151.C2      |
| R4  | minor | "every `k*` value is compared" overstated; `kf*`/unmodified keys untested    | 151.C2      |
| R5  | minor | `TerminalEmulator::new` grew to ten parameters under an existing allow       | 151.C2      |
| R6  | minor | "single value … FREC" decision text not amended; stale doc example           | 151.C3      |
| R7  | minor | 147.22 still reads as an open Task 147 subtask in this branch                | 151.C3      |
| R8  | minor | decision 2 / `TERMINFO_DIRS` bullet / 151.7 note predate 151.C1              | 151.C3      |
| R9  | minor | `shell-integration/README.md` does not mention the sudo wrapper              | 151.C3      |
| R10 | minor | `sudo -u other`, `su`, `doas`, containers cannot read the 0700 pane dir      | 151.C3      |
| R11 | minor | committed tree bytes are per `tic` version; not stated                       | 151.C3      |
| R12 | nit   | stale test name; `TERMINFO_DIRS` test vacuous when the variable is unset     | 151.C2      |
| R13 | nit   | table-of-truth scan of `dcs.rs` misses multi-line arms; parser in DCS tests  | 151.C2      |
| R14 | nit   | pane `env` `TERM` key case-sensitive on Windows; empty value accepted        | 151.C2      |
| R15 | nit   | sudo checkbox enabled while `set_term_program` is off                        | 151.C2      |
| R16 | nit   | `query-os-name` maps DragonFly to `Unknown`                                  | 151.C2      |
| R17 | nit   | `gui::recording` made `pub` instead of `pub(crate)`                          | 151.C2      |
| R18 | nit   | `.ti` header "every capability implemented" (kcbt/kent); md5sums comment     | 151.C3      |
| R19 | nit   | real-PTY tests leave `sleep 30` children                                     | 151.C2      |
| R20 | nit   | `packages.terminfo` has no `meta.license`                                    | 151.C2      |
| R21 | nit   | tool-missing skips pass vacuously on CI                                      | 151.C2      |
| R22 | n/a   | `nix build .#freminal` (full check phase) not run                            | before PR   |

Orchestrator decisions on the open points:

- **R1:** the documented local installs name their target: `tic -x -o ~/.terminfo -` (per
  user) and `sudo env -u TERMINFO tic -x -` (system-wide; the sudo wrapper keeps `TERMINFO`,
  so it must be unset). The `ssh HOST -- tic -x -` forms are unaffected: ssh does not forward
  `TERMINFO`.
- **R2:** an inherited `TERMINFO` is kept reachable by putting it at the front of the child's
  `TERMINFO_DIRS` (followed by the inherited `TERMINFO_DIRS`, or the system default when that
  is unset); freminal's directory stays in `TERMINFO`. `TERMINFO` is set whatever `TERM` is:
  it is not exclusive (151.C1), so a non-bundled `TERM` still resolves.
- **R4:** the keypad `\x1b[OM` encoding no longer exists once Task 147 merges (147.5 deleted
  `TerminalInput::KeyPad` and sends `SS3 M`); `kent` and `kcbt` go in an explicit "declared,
  not sent" list with the reason.
- **R5:** the per-pane spawn-environment values move into one struct passed to
  `TerminalEmulator::new` (`freminal-extend-or-extract`). The `reset()` `#[expect]` stays: the
  exhaustive destructuring is the reset table's compile-time check.
- **R14:** on Windows the `TERM` key is matched case-insensitively; an empty pane-`env` `TERM`
  is treated as absent.
- **R21:** when the `CI` variable is set, a missing `tic` or `infocmp` fails the terminfo tests
  instead of skipping (every Unix CI runner has them); missing `zsh`/`fish` still skip.

### 151 Cleanup entries (review)

#### 151.C2 — Review fixes: code and tests

R2, R3, R4, R5, R12, R13, R14, R15, R16, R17, R19, R20, R21, as decided above. Scope: the files
each finding names, plus the `TerminalEmulator::new` call sites for R5.

- **Status: done (2026-11-19).** R2: pure `terminfo_env` (an inherited non-empty `TERMINFO`
  heads the child's `TERMINFO_DIRS`, then the inherited list or the system default), tested
  over every combination. R3: `apply_spawn_shell_features` runs on every spawn path. R4: every
  `k*` capability is compared with `to_payload` output, except `kent`, `kcbt` and `kmous` in
  `DECLARED_NOT_SENT` with reasons. R5: `PaneSpawnEnv` (`io/pane_spawn_env.rs`) cuts
  `TerminalEmulator::new` to five parameters and removes its `too_many_arguments` allow; the
  orchestrator also had the TOML bool carried as `TermProgramEnv { Advertise, Omit }`
  (`io/term_program_env.rs`, in `PaneSpawnEnv` and `PtySpawnConfig`). R12: renamed and made
  non-vacuous. R13: the parser and table-of-truth tests are
  `terminal_handler/dcs/terminfo_truth_tests.rs`, and the arm scan panics on any line it does
  not understand (a mutation test confirmed). R14: `TermName::is_env_key(platform, key)`
  (case-insensitive on Windows), empty pane-`env` `TERM` ignored. R15, R16, R19, R20, R21 as
  decided. **R17 not done, correctly:** `freminal/src/main.rs` is a separate crate root (the
  binary) that reaches `freminal::gui::recording`, so the module stays `pub`.

#### 151.C3 — Review fixes: docs and plan

R1, R6, R7, R8, R9, R10, R11, R18. Scope: `README.md`, `Documents/DESIGN_DECISIONS.md`,
`shell-integration/README.md`, `res/freminal.ti` (header), `assets/ci/fix-linux-icon-metadata.sh`
(comment), `freminal-terminal-emulator/build.rs` (comment), `freminal-terminal-emulator/src/recording.rs`
(doc example), `Documents/FREC_FORMAT.md`; the plan items are the orchestrator's.

- **Status: done (2026-11-19).** R1: the README and `DESIGN_DECISIONS.md` local installs name
  their target (`tic -x -o ~/.terminfo -`; `sudo env -u TERMINFO tic -x -`, because the sudo
  wrapper keeps `TERMINFO`); the `ssh … tic -x -` forms are unchanged. R6, R9 (new
  "The `sudo` Wrapper (Terminfo)" section in `shell-integration/README.md`), R10, R11, R18 as
  decided, plus the R2 and R14 behaviour in the README and `DESIGN_DECISIONS.md`.
  `FREC_FORMAT.md` already agreed. R7 and R8 were done by the orchestrator in 151.C2's
  commit.

### 151 Confirmation review (2026-11-19)

A second CODE-REVIEW pass over `7dee17d0..HEAD`: R1–R16, R18 and R20 FIXED; R17
ACCEPTED-AS-DOCUMENTED; R19 FIXED for Task 151's tests (the `sleep 30` echo-off tests predate
the branch and need the PTY held open); R21 PARTIAL (N1, N4). New findings:

| ID  | Sev   | Finding (short)                                                                 | Disposition |
| --- | ----- | ------------------------------------------------------------------------------- | ----------- |
| N1  | major | CI-strict skip fails the stdin-compile test on `windows-latest` (no `tic`)      | 151.C4      |
| N2  | minor | nested freminal panes carry the parent pane's `TERMINFO` into `TERMINFO_DIRS`   | 151.C4      |
| N3  | nit   | a user `TERMINFO` now ranks after `~/.terminfo`; not documented                 | 151.C4      |
| N4  | nit   | missing `xterm-256color` still skips silently on CI                             | 151.C4      |
| N5  | nit   | CI-skip helpers duplicated in two test files                                    | 151.C4      |
| N6  | nit   | `kcbt` declared, never sent; `kent` exemption is conditional on Task 147        | routed      |
| N7  | nit   | README "On a remote host:" heading over a command run locally                   | 151.C4      |
| N8  | nit   | `TermProgramEnv` doc overstates what `Omit` leaves alone                        | 151.C4      |
| N9  | nit   | `DESIGN_DECISIONS.md` says a forgotten version bump fails CI; it does not       | 151.C4      |

- **N6 routing:** sending `CSI Z` for Shift+Tab is Task 137 B3 (modifiers on Tab), so `kcbt`
  stays declared (it is the standard value) and Task 137's stub gains the note. `kent`:
  147.5 makes keypad Enter in application mode send `SS3 M`, which is `kent`'s value, so
  whichever of the Task 147 and Task 151 PRs merges second moves `kent` from
  `DECLARED_NOT_SENT` to the encoder-checked set (a merge-time reconciliation, recorded in
  both tasks' notes).
- **N2 decision:** an inherited `TERMINFO` equal to the pane's own directory is dropped; a
  pane marks its directory with a `FREMINAL_TERMINFO` variable naming it, and an inherited
  `TERMINFO` equal to the inherited `FREMINAL_TERMINFO` (a parent freminal pane's directory)
  is not carried into `TERMINFO_DIRS`.

#### 151.C4 — Confirmation-review fixes

N1 (gate the `tic` tests to Unix, or exempt Windows from the CI-strict rule), N2 (as decided),
N3, N4, N5 (a shared `tests/common` helper), N7, N8, N9.

- **Status: done (2026-11-19).** N1: the stdin-compile test and its source constant are
  `#[cfg(unix)]`. N2: the child also gets `FREMINAL_TERMINFO=<pane dir>`; `terminfo_env`
  drops an inherited `TERMINFO` equal to the pane's directory or to the inherited
  `FREMINAL_TERMINFO`, tested for nested panes, the bundle duplicate and a user value under a
  nested pane. N3–N5, N7–N9 as recorded (`tests/common/mod.rs` holds the CI-strict helper).
  `terminfo_child` passed 10 of 10, and with `CI=1`.
- **R22 (`nix build .#freminal`, 2026-11-19).** The package's check phase failed seven
  `sudo_wrapper_tests`: their generated fake `sudo` used `#!/usr/bin/env sh`, and the Nix
  build sandbox has no `/usr/bin/env`. Fixed by the orchestrator: the generated fixture names
  the `sh` found on `PATH` absolutely (it is a test fixture, not a shipped script, so
  `shebang-env-discipline` does not apply).
  The second build then failed `child_term_real_pty_tests`: with no working directory,
  portable-pty starts the child in `$HOME`, which does not exist in the sandbox
  (`/homeless-shelter`; the flake already skips three echo-off PTY tests for this). The new
  real-child tests now pass an explicit working directory instead of joining the skip list.

### 151 PR review (PR #588, 2026-11-20)

CI failed `Test macos-latest`, and Copilot and CodeRabbit left five comments. Dispositions:

| ID  | Source          | Finding (short)                                                                 | Disposition |
| --- | --------------- | ------------------------------------------------------------------------------- | ----------- |
| P1  | Copilot         | the bash and zsh wrappers copy GPLv3 code into MIT freminal                     | 151.C5      |
| P2  | CodeRabbit      | `sudo -u root -e file` is wrapped, and sudo rejects `--preserve-env` with `-e`  | 151.C5      |
| P3  | Copilot, CR     | `build.rs` rewrites `res/` during a build; the embed can lag the tree           | 151.C6      |
| P4  | Copilot, CR, CI | macOS `tic` writes `78/xterm-freminal`; the stale-entry setup accepts `x/` only | 151.C6      |
| P5  | CodeRabbit      | the in-pane `infocmp` install command does not apply on Windows                 | 151.C6      |

- **P1 (confirmed):** Ghostty's `ghostty.bash` and `ghostty-integration` (zsh) state that they
  derive from kitty's integration and are distributed under GPLv3; its fish file has no such
  notice (Ghostty is MIT). 151.12 asked for Ghostty's wrapper "exactly", which copied GPLv3
  code. That was an orchestrator error and contradicts this plan's own rule (kitty is read for
  behaviour only). **Decision:** all three wrappers are rewritten clean-room from a behaviour
  specification written by the orchestrator, by an implementer that does not read Ghostty's or
  kitty's shell scripts; the "reproduced from Ghostty" citations go. Decision 5's behaviour
  (on by default, separate toggle, sudoedit unchanged) stands; "exactly as Ghostty" is
  withdrawn.
- **P2 (accepted):** the clean-room scan follows sudo's own option syntax, so option operands
  (`-u root`, `--user=root`, `--user root`, grouped `-ku root`) are skipped and `-e` after them
  is still recognised. This deliberately differs from Ghostty.
- **P3 decision:** regenerating the compiled tree and tar becomes an explicit
  `cargo xtask terminfo` step; `build.rs` no longer writes into `res/` (Cargo's rule that build
  scripts write only to `OUT_DIR`). The committed tree and tar are the single generation
  everything embeds and packages; the existing stdin-compile test fails when `res/freminal.ti`
  changes without a regeneration, and says to run the xtask.

#### 151.C5 — Clean-room `sudo` wrapper (P1, P2)

Scope: the three shell-integration scripts' `sudo` blocks, `freminal/src/shell_integration/sudo_wrapper_tests.rs`,
`shell-integration/README.md` and `DESIGN_DECISIONS.md` (the wrapper description).
Behaviour specification (the implementer works only from this):

- The wrapper is defined only when `FREMINAL_SHELL_FEATURES` contains the item `sudo` (a
  comma-separated list; match whole items, not substrings) and `TERMINFO` is non-empty. In
  fish it is not defined if a `sudo` function or alias already exists; bash and zsh define it
  regardless (it replaces nothing a later alias cannot override).
- It calls the real `sudo` (bypassing itself: `command sudo` in bash, zsh and fish) with
  `--preserve-env=TERMINFO` inserted first, unless the invocation is an edit, in which case it
  passes the arguments through unchanged.
- An invocation is an edit when, scanning the arguments from the left, `-e` or `--edit`
  appears before the first command word. The scan, following sudo(8):
  - `--` ends the options (no edit found by then: not an edit);
  - a long option with `=` (`--user=root`) is one word; a long option that takes a value
    (`--chdir`, `--chroot`, `--close-from`, `--group`, `--host`, `--prompt`, `--role`,
    `--type`, `--command-timeout`, `--other-user`, `--user`) given without `=` consumes the
    next word too;
  - a short-option word `-abc` is read letter by letter: `e` means edit; a letter that takes
    a value (`C D g h p R r T t U u`) consumes the rest of the word, or the next word if it is
    the last letter; other letters take none;
  - a word of the form `NAME=value` (before the command) is skipped;
  - any other word is the command: stop (not an edit).
- Tests (all three shells, fake `sudo`): `sudo ls`, `sudo -u root ls`, `sudo VAR=1 cmd` and
  `sudo -- ls` get the flag; `sudo -e f`, `sudo --edit f`, `sudo -u root -e f`,
  `sudo --user=root -e f`, `sudo --user root -e f`, `sudo -ke f` and `sudo -n -e f` do not;
  `sudo -ue root f` gets the flag (`e` is `u`'s value, a user named "e", and `root` is the
  command: not an edit); feature off, `TERMINFO` empty, and a feature list such as `nosudo`
  (no whole-item match) leave `sudo` alone; fish leaves an existing `sudo` function alone.
- **Status: done (2026-11-20).** The implementer attested to the clean-room rule (old blocks
  deleted by line range unread; no Ghostty or kitty source, git history or recall used). Each
  script has its own `__freminal_sudo_is_edit` scan and a `sudo` function; bash and zsh match
  the feature item as `,…,` containing `,sudo,`. `shipped_scripts_cite_ghostty` is replaced by
  `sudo_blocks_name_no_third_party_source`; 14 flag cases, 12 edit cases, the whole-item
  matching and the fish non-clobber case run in bash, zsh and fish. Found while testing: `?`
  is not a wildcard in fish `case`. A pre-existing "Ghostty-derived" `.zshenv` `ZDOTDIR` step
  (not part of this task) is routed to 147.C4 for the same licence check.

#### 151.C6 — Build generation, macOS layout, Windows README (P3, P4, P5)

Scope: `freminal-terminal-emulator/build.rs` (remove the terminfo generation and its
`rerun-if-changed` lines), `xtask/` (a new `terminfo` subcommand that runs `tic -x` and writes
the committed tree and tar exactly as `build.rs` did), `freminal-terminal-emulator/tests/terminfo_tree.rs`
(the stdin-compile test's failure message names `cargo xtask terminfo`),
`freminal-terminal-emulator/tests/terminfo_child.rs` (accept `x/` or `78/` wherever the setup
or the child probe looks for a compiled entry), `README.md` (the in-pane command applies where
freminal bundles the entry, not on Windows; Windows users use the source-file command),
`DESIGN_DECISIONS.md` and `agents.md`'s xtask list (the new subcommand), and the `res/freminal.ti`
header (how to regenerate).

- **Status: done (2026-11-20).** `build.rs` keeps only the version logic and writes nothing
  outside `OUT_DIR`; `cargo xtask terminfo` (alias `ti`, `xtask/src/terminfo.rs`) holds the
  unchanged generation and fails clearly without `tic`. The stdin-compile test names the
  xtask on failure. `terminfo_child.rs` accepts `x/` or `78/` in the setup and the child
  probe. The README limits the in-pane command to Linux, macOS and BSD. Two stale "`build.rs`
  passes `-x`" mentions (COVERAGE, kitty reference) were updated. A clean build and a
  `cargo xtask terminfo` run both leave `res/` unchanged.

#### 151.C7 — PR #588 second review round (P6-P9)

CodeRabbit's second pass on PR #588 (2026-11-20) left four comments; all are accepted. While
checking P7 against sudo's own option table (`src/parse_args.c`, `sudo_short_opts` and
`sudo_long_opts`, ISC-licensed, read for its option list only) the orchestrator found that
151.C5's specification was itself incomplete, so the scan rules below supersede 151.C5's.

| ID  | Finding (short)                                                                       |
| --- | ------------------------------------------------------------------------------------- |
| P6  | `DESIGN_DECISIONS.md` says root programs ignore `TERMINFO`; default ncurses allows it |
| P7  | sudo accepts unique long-option prefixes (`--ed`), which the scan does not            |
| P8  | zsh `KSH_ARRAYS` breaks the scan's string subscripts                                  |
| P9  | the stdin-compile test compares raw `tic` bytes across ncurses builds                 |

- **P6:** ncurses ignores `TERMINFO`/`TERMINFO_DIRS` in setuid/setgid programs; programs run
  as root read them unless ncurses was configured with `--disable-root-environ`. Reword the
  "Known limits" paragraph accordingly.
- **P7 (scan rules, superseding 151.C5):**
  - Short letters that require a value (consume the rest of the word, or the next word when
    last): `a C c D g p R r T t U u`. `h` takes an optional value: it consumes the rest of
    its word if any, and never the next word. `e` is edit. Other letters take none.
  - Long options (sudo's full list): `askpass auth-type background bell chdir chroot
    close-from command-timeout edit group help host list login login-class no-update
    non-interactive other-user preserve-env preserve-groups prompt remove-timestamp
    reset-timestamp role set-home shell stdin type user validate version`. Value-required:
    `auth-type chdir chroot close-from command-timeout group host login-class other-user
    prompt role type user` (without `=` they consume the next word). `preserve-env` takes a
    value only with `=`.
  - A long option word (the part before any `=`) matches an exact name first; otherwise a
    prefix of exactly one name (`--ed` is `--edit`, `--us` is `--user`). A prefix of more than
    one name (`--re`) or of none is an unknown option taking no value (sudo itself rejects
    it, so the result does not matter beyond not crashing).
- **P8:** the zsh scan starts with `emulate -L zsh`.
- **P9:** the test compares the decompiled capabilities instead of bytes: `infocmp -x` of the
  stdin-compiled entry against `infocmp -x` of the committed tree (both read with the host's
  `infocmp`, `TERMINFO` pointed at each tree; header lines naming the file are dropped).
  Byte-equality of the committed tree with the archive stays.
- **Status: done (2026-11-20).** Scan rewritten to the P7 rules in all three shells (a long-name
  list and a `__freminal_sudo_long_name` prefix helper each), clean-room attested again. zsh
  helpers start with `emulate -L zsh`; `zsh_scan_is_independent_of_ksh_arrays` was shown to
  fail without it. Sixteen new cross-shell cases (`--ed`, `-h -e`, `-a`/`-c`, abbreviated
  value options, `--login` exact, ambiguous `--re`). The stdin-compile test compares
  `infocmp -x` output (comments dropped, `TERMINFO_DIRS` cleared, empty `HOME`); a `cols#81`
  edit was shown to differ. The README's remaining Ghostty comparison sentence was removed.

#### 151.C8 — Standalone `-h` takes a detached host (P10)

CodeRabbit's third pass on PR #588 (2026-11-20): `sudo -h localhost -e f` is an edit, but the
151.C7 scan treats `localhost` as the command. Confirmed in sudo's `src/parse_args.c`
(`case 'h'`, `got_host_flag`, `is_envar`): when the previous word is exactly `-h` (a standalone
word, not grouped such as `-kh` and not attached such as `-hfoo`), sudo takes the next word as
the remote host if it does not start with `-` and is not an environment assignment (contains
`=` and starts with neither `/` nor `=`). This refines the 151.C7 `h` rule; everything else in
151.C7 stands. Tests (all three shells): `-h localhost -e f` is an edit; `-h -e f` stays an
edit; `-h VAR=1 ls` and `-h ls` get the flag (`ls` is the host there, so no command is found
before the end: not an edit); `-kh host -e f` gets the flag (grouped: no lookahead, `host` is
the command); `-hfoo -e f` stays an edit; `-h /a=b -e f` is an edit (`/a=b` is a host).

- **Status: done (2026-11-20).** The scanners carry a one-word "host" state after an exact
  `-h`; a following word that is neither an option nor an assignment is consumed. Found while
  testing: zsh expands an unquoted `=*` `case` pattern as `=command`, so bash and zsh use
  `[=]*`; fish's `switch` now quotes its word. Six new cross-shell cases plus two in the zsh
  `KSH_ARRAYS` run. Clean-room attested.

### 151 Status notes

- **151.1 (done).** See "151 Audit".
- **151.2 (done).** `lookup_termcap` answers `indn`/`SF`, `rin`/`SR` (byte-identical to the <!-- codespell:ignore rin -->
  entry, which already had both) and `query-os-name`/`query-os_name` through the pure
  `os_name_for_target`. `kitty-query-os_name` is not answered. The `.ti`-vs-XTGETTCAP test
  now also checks `indn` and `rin`. <!-- codespell:ignore rin -->
- **151.3 (done).** Every "151 Audit" decision applied. `acsc` now equals `xterm-256color`'s;
  `Se` is `\E[0 q` on both sides; the added capabilities were each checked against the
  code. The test parses `res/freminal.ti` and reads `lookup_termcap`'s arms from `dcs.rs`, so
  a new arm on either side fails it: every XTGETTCAP name must match the entry except four
  listed XTGETTCAP-only names, every entry capability is either answered identically or in an
  explicit `TERMINFO_ONLY` list, and every `k*` value equals the encoder's bytes. `tic -x -c`
  leaves four warnings, all shared with `xterm-256color` or kitty (`kind`/`kDN` and
  `kri`/`kUP` duplicates, `tsl` parameter count, `%;` in `Setulc`, undocumented extended
  names). The SGR 8 GAPS row is new.
- **151.4 (done).** `build.rs` writes `res/terminfo/{x,f,78,66}/…` (explicit paths, identical
  bytes; `tic`'s alias is no longer counted twice) and packs the tar from that tree. Entry
  contents are unchanged (the letter copy is byte-identical to the old tar). New
  `tests/terminfo_tree.rs` pins the four members against the committed files. The
  orchestrator also marked the tree and tar `binary` in `.gitattributes` and corrected the
  `.ti` header.
- **151.5 (done).** New `freminal-common/src/term_name.rs` (`TermPlatform`, the two defaults,
  `validate_term_name` with a typed `TermNameError`); `ShellConfig::term: Option<String>` and
  `ShellConfig::effective_term(platform)`, the single resolved value. An invalid value is
  dropped at load with a warning on the existing load-warning channel (silently in
  `from_toml_str`, which has none). `[shell]` was already a whole-section `ConfigPartial`
  entry, so only the guard test changed. Settings Shell tab, `config_example.toml` and the
  home-manager option are wired.
- **151.6 (done).** `TermName` (`term_name.rs`) with `TermName::resolve(&ShellConfig,
  TermPlatform, pane env)`: a pane `env` `TERM` (taken as given), then `[shell] term`, then
  the platform default. Every spawn site (`tab_spawning.rs` new tab, split and layout leaf;
  `app_impl.rs` initial and second spawn) resolves it once and passes it through
  `PtyTabConfig` to `TerminalEmulator::new` (now nine parameters, under its existing
  `too_many_arguments` allow) and `PtySpawnConfig`. `run_terminal` sets `TERM` once
  (`apply_term_env`) and the extra-env loop skips `TERM`. XTGETTCAP `TN` reads the handler's
  `term_name` (`lookup_termcap` now takes it); the Task 76.6 guard checks `TN` equals the
  child's `TERM`. FREC `term` is per recording, so it records the `[shell] term` resolution
  (`recording_term`); a pane-level `env` `TERM` is not visible in the header. `reset()` grew
  one line past `too_many_lines` with the new reset-table row and carries an `#[expect]`
  with the reason. A Unix test spawns a real `/bin/sh` and checks `$TERM` in each case.
- **151.7 (done).** Plan correction: cargo-bundle's `resources` key cannot place files at
  `Contents/Resources/terminfo/` (it keeps the source path, so `res/terminfo` would land at
  `Contents/Resources/res/terminfo`), the key is global, and it would also add a copy to the
  `.deb` and the AppImage (`usr/lib/freminal/…`, cargo-bundle `osx_bundle.rs`,
  `deb_bundle.rs`, `appimage/app_dir.rs`), against decision 3. Orchestrator decision: no
  `resources` key; the release workflows (`deploy.yaml`, `nightly.yml`) copy `res/terminfo`
  into the staged `.app` before `codesign --deep`, and the flake's Darwin `postInstall` does
  the same. A locally built `cargo bundle` `.app` has no tree and uses per-pane extraction.
  `pty.rs` gains the pure `bundle_terminfo_dir` (exe path → `X.app/Contents/Resources/terminfo`),
  `TerminfoPlatform`, `TerminfoSource { Unused, Bundle, PerPaneExtraction }` and
  `terminfo_dirs_value` (replaced by `apply_terminfo_env` in 151.C1); on macOS a present
  bundle directory replaces per-pane extraction.
  Neither `cargo bundle` nor the macOS workflow was run locally.
- **151.8 (done).** rpm `assets` and the repacked `.deb` (`fix-linux-icon-metadata.sh`
  `patch_deb` only; the AppImage is untouched) install `/usr/share/terminfo/{x,f}/…`.
  `packages.<system>.terminfo` copies the committed tree (source filtered with
  `lib.fileset`, all four layouts) into `$out/share/terminfo`; the main package installs it
  too. `nixosModules.default` (`nix/nixos-module.nix`): `programs.freminal.terminfo.enable`
  and `.package`, added to `environment.systemPackages` under `lib.hiPrio` so a
  mixed-revision profile resolves deterministically. Verified: `nix build .#terminfo` plus
  `infocmp`, `nix flake check --no-build`, a `nixosSystem` evaluation with and without the
  option, darwin `drvPath` evaluation. Not run locally: the full package build and a real
  rpm/deb.
- **151.11 (done).** Both release jobs copy `res/freminal.ti` into `artifacts/` before the
  upload (`deploy.yaml`'s release job gained a checkout). The `.ti` header records that the
  path is public. `source_compiles_from_stdin_to_the_committed_entry` pipes the file through
  `tic -x -o <tmp> -` and compares the result with the committed entry (skipped, with a
  message, without `tic`). The raw `main` URL resolves once this branch merges.
- **151.9 (done).** `tests/terminfo_child.rs` spawns `/bin/sh` through
  `FreminalPtyInputOutput::new` with the default config and checks, inside the child, `TERM`,
  that the first `TERMINFO_DIRS` entry holds `x/xterm-freminal`, that `infocmp -x` resolves
  it from that directory with `Smulx` and `Sync`, and `tput colors` / `tput setaf 9`. Plan
  correction: `tput` cannot read extended capabilities, so `Smulx` is checked through
  `infocmp`. The child gets an empty `HOME` through the pane `env`, because ncurses searches
  `~/.terminfo` before `TERMINFO_DIRS`; that surfaced 151.C1. 20 sequential runs and 5 under
  load passed. Its macOS CI run settles the Apple ncurses question.
- **151.10 (done).** New "TERM Strategy (Task 151)" in `DESIGN_DECISIONS.md` and a README
  "Terminfo" section (symptom, the three decision-4 commands, the release asset, the
  `~/.terminfo` note, packages, the `[shell] term` escape hatch). The listed
  `xterm-256color` sentences are updated (COVERAGE, GAPS, kitty reference, `FREC_FORMAT.md`;
  history notes in `PLAN_VERSION_090.md`, `PLAN_VERSION_150.md` and `MASTER_PLAN.md` get a
  "superseded by Task 151" line). The GAPS "XTGETTCAP capability expansion" row is closed by
  151.2. The `kbs` comments in `decbkm.rs` and `input.rs` stay accurate (`\177` in both
  entries).

**Merged** 2026-11-20 via PR #588 (`2ee931b8`).
