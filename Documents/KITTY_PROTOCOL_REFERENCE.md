# Kitty Protocol Reference (freminal implementation notes)

Last updated: 2026-10-12 — Task 138 — the "Desktop notifications (OSC 99)"
section is re-distilled from the kitty 0.49.2 page and rewritten to the shipped
behaviour, which is conformant: the per-key and per-payload behaviour, the reply
formats, the `p=?` answer per routing, platform and daemon, the leg capability
table and the documented differences from kitty. The standard icon names are
honoured on every leg (Tasks 138.C4 and 138.C5), so the "Remaining conformance
gap" subsection is removed. The Task 99 current-state notes and the alive-map
deferral are removed (superseded). The section was re-checked against the code
after the adversarial-review cleanups (138.C6 to 138.C15): hard text limits,
reassembly bounds, rejected-chunk handling, per-pane quotas, `p=alive`
semantics, bus-down routing, the D-Bus service and the macOS/Windows delivery
worker. No other section changed.

Last updated: 2026-10-10 — Task 131 — the keyboard mode stack is per screen
(131.3, fixes B15) and kitty graphics placements are per screen (131.7, 131.C5);
see "freminal current-state deltas: keyboard" and "freminal current-state:
graphics". No other section changed.

Last updated: 2026-10-10 — Task 130 — the OSC 99 `p=?` capability query is
now answered on the PTY thread in stream order, gated on configuration and
platform (see "Capability handshake" under the current-state deltas); the
rest of this file is unchanged.

Last updated: 2026-10-08 — Task 126.4 — roadmap table and future-version stubs
renumbered to the v0.13.x plan; current-state sections flagged as superseded
(see the note below).

**Superseded current-state sections.** Every "freminal current-state" section in
this file except notifications (graphics, keyboard) predates the **2026-10-08
compliance audit** in `PLAN_VERSION_130.md` ("Compliance audit (2026-10-08)").
That audit found real spec deviations in every protocol freminal had claimed to
support, so statements here that a protocol is "implemented" or "done" describe
the feature surface that shipped, not conformance to the kitty spec. Where this
file and the audit disagree, the audit wins. Each protocol task in v0.13.x
refreshes its own section here as it lands; the notifications section was
refreshed by Task 138 and is current.

## Provenance and scope

This document is a **distilled, freminal-facing** reference for the kitty
terminal protocol extensions that freminal implements or plans to implement. It
is a working aid for the kitty-protocol roadmap (v0.11.0 onward) so that
implementers and reviewers do not have to re-fetch and re-digest the upstream
specs for every subtask.

It is **not** a verbatim copy of the kitty documentation. It captures the wire
formats, key tables, response/report byte layouts, error codes, and numeric
limits that freminal must match — the mechanical surface — not the upstream
prose, rationale, or examples.

- Source: the kitty terminal documentation, sections _Desktop notifications_,
  _Terminal graphics protocol_, and _Comprehensive keyboard handling in
  terminals_.
- Upstream authority (always defer to these; this file is a snapshot):
  - <https://sw.kovidgoyal.net/kitty/desktop-notifications/>
  - <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
  - <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>
- Distilled from upstream as of **2026-07-05** (keyboard key tables reconfirmed
  during Task 101 and Task 114 — the functional-key codepoint table was
  re-checked against upstream when wiring the raw-winit delivery path),
  corresponding to kitty **~0.47.x**. The desktop-notifications section was
  re-distilled on **2026-10-12** against kitty **0.49.2**.
- Attribution: the kitty documentation is authored by Kovid Goyal and is
  licensed GPL-3.0. This file is a distilled derivative for implementation
  reference within freminal.

**Drift guard.** kitty's specs are versioned and evolve. Treat any conflict
between this file and the upstream URLs above as "upstream wins", and update
this file (and its `Distilled ... as of` date) when reconciling. Do not let this
file silently rot into a liar; if a subtask finds it stale, fix it as part of
that subtask.

The **goal for all kitty protocol work in freminal is 100% compliance** with the
stable kitty spec for each protocol. This reference exists to make that
achievable and verifiable.

## How this maps to the roadmap

| Protocol                                | Milestone | Task(s)                                               | Status in this doc                                    |
| --------------------------------------- | --------- | ----------------------------------------------------- | ----------------------------------------------------- |
| Desktop notifications (OSC 99)          | v0.13.0   | 138 (conformance; 99 shipped)                         | Filled in; conformant                                 |
| Pointer shapes (OSC 22)                 | v0.13.0   | 139                                                   | Not covered                                           |
| Underlines and SGR parity               | v0.13.0   | 140                                                   | Not covered                                           |
| Graphics protocol                       | v0.13.1   | 135, 136 (conformance and placement; 13, 100 shipped) | Filled in                                             |
| Keyboard protocol                       | v0.13.1   | 137 (conformance; 35, 101, 114 shipped)               | Filled in                                             |
| Multiple cursors (CSI)                  | v0.13.2   | 103                                                   | Stub                                                  |
| Misc protocol extensions                | v0.13.2   | 141                                                   | Not covered                                           |
| Unscroll (`CSI Ps + T`)                 | v0.13.2   | 142                                                   | Not covered                                           |
| DECCARA / DECSACE                       | v0.13.2   | 143                                                   | Not covered                                           |
| Color control (OSC 21) and colour stack | v0.13.2   | 144                                                   | Not covered                                           |
| Shell integration compatibility         | v0.13.2   | 145                                                   | Not covered                                           |
| File transfer over TTY (OSC 5113)       | v0.13.3   | 102                                                   | Stub                                                  |
| Text sizing (OSC 66)                    | v0.13.3   | 104                                                   | Stub                                                  |
| Clipboard (OSC 5522)                    | v0.13.3   | 146                                                   | Not covered                                           |
| Drag and drop (OSC 72)                  | v0.13.4   | 105                                                   | Stub (stable upstream; blocked locally by winit 0.30) |

Colored/styled underlines and the base kitty keyboard/graphics subsets are
already shipped (Tasks 35, 13) and are covered here only where v0.11.0 extends
them.

Shared foundation work for these tasks (CSI dispatch, wire infrastructure, reverse
path, screen-scoped state, colour foundation, consent prompt, Unicode width) is
Tasks 128–134; see `PLAN_VERSION_130.md`.

---

## Desktop notifications (OSC 99)

Reference for Tasks 99 and 138. OSC 99 is the stateful sibling of OSC 9 /
OSC 777 (Task 76): it adds notification identity, chunked payloads, activation,
close and alive reports written back to the application, buttons, icons,
sounds, urgency, occasion, auto-expiry, filtering hints and a capability
handshake.

**Baseline: kitty 0.49.2** (this section was re-distilled from the upstream page
on 2026-10-12). freminal conforms to it (Task 138, v0.13.0, including the
standard icon names on every leg, Tasks 138.C4 and 138.C5). Where freminal
deliberately differs from kitty, the difference is listed under "Documented
differences from kitty" below; everything else in this section is what the spec
says and what freminal does.

### Envelope (OSC 99)

```text
ESC ] 99 ; <metadata> ; <payload> ST
```

- `ESC ]` is `0x1B 0x5D`. `ST` is `ESC \` = `0x1B 0x5C`.
- `<metadata>` is zero or more `key=value` pairs separated by **colons** (`:`).
- **Both semicolons are always present**, even when metadata is empty.
- Every metadata key is a single character `[a-zA-Z]`; values are words from the
  set ``a-zA-Z0-9-_/+.,(){}[]*&^%$#@!`~`` (so raw base64, including `+ / =`,
  is a legal value).
- The payload is interpreted according to the `p=` key.
- A receiver must ignore keys it does not understand. For a known key with a
  value it does not understand, it should either ignore the whole escape code
  or make a best effort. freminal ignores that key and applies the rest.

Minimal example (title only, no metadata): `ESC ] 99 ; ; Hello world ST`.

### Metadata key table

| Key | Values                                                        | Default        | Meaning                                                                               |
| --- | ------------------------------------------------------------- | -------------- | ------------------------------------------------------------------------------------- |
| `a` | comma list of `report`, `focus`, each optionally `-`-prefixed | `focus`        | Action on activation: `focus` the source window and/or `report` back to the app.      |
| `c` | `0` or `1`                                                    | `0`            | When `1`, send a close report to the app when the notification is closed.             |
| `d` | `0` or `1`                                                    | `1`            | Done flag. `0` = more chunks coming, hold display. `1` = complete, display now.       |
| `e` | `0` or `1`                                                    | `0`            | When `1`, this chunk's payload is base64 (RFC 4648). Otherwise escape-safe UTF-8.     |
| `f` | base64 UTF-8 app name                                         | unset          | Application name. Used for filtering; also a fallback icon source.                    |
| `g` | identifier                                                    | unset          | Icon-data cache key (UUID-like). Caches transmitted icon data for the session.        |
| `i` | identifier                                                    | unset          | Notification id. UUID-like. Required to chunk and to receive reports. `i=0` reserved. |
| `n` | base64 UTF-8 icon name                                        | unset          | Icon name (symbol or app id). May repeat; first available wins.                       |
| `o` | `always`, `unfocused`, `invisible`                            | `always`       | Occasion: when to honour the notification.                                            |
| `p` | payload type (see below)                                      | `title`        | Interpretation of the payload.                                                        |
| `s` | base64 sound name                                             | `system`       | Sound to play. `silent` = none. `system` = platform default.                          |
| `t` | base64 UTF-8 type                                             | unset          | Notification type/category. May repeat. Used for filtering.                           |
| `u` | `0`, `1`, `2`                                                 | unset (normal) | Urgency: `0` low, `1` normal, `2` critical. Plain integer, not base64.                |
| `w` | integer `>= -1`                                               | `-1`           | Auto-expire after N ms. `-1` = OS default, `0` = never, `>0` = ms (best-effort=0).    |

Version notes upstream: `o` added 0.31.0; `u` 0.35.0; `c f t n s g` and the
update, close and query actions 0.36.0.

### How freminal handles each key

| Key | freminal                                                                                                                                                                                                                                                                                                                                                       |
| --- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `a` | Words are edits applied in arrival order, starting from `{focus}`; unknown words are ignored; an empty `a=` changes nothing; edits keep accumulating across chunks (differs from kitty). `focus` raises the pane, `report` sends a report.                                                                                                                     |
| `c` | `0` is off; any other value, an empty one included, is on (kitty's `v != 0`). On macOS and Windows a `c=1` request is answered with `p=close;untracked` when the notification is shown.                                                                                                                                                                        |
| `d` | `0` means more chunks follow; any other value, an empty one included, is done (kitty's `v != 0`).                                                                                                                                                                                                                                                              |
| `e` | `1` is base64; any other non-empty value is plain text. Base64 chunks decode as one stream, so chunk-then-encode and encode-then-chunk both work.                                                                                                                                                                                                              |
| `f` | Base64 UTF-8; invalid or empty is ignored. Sent as the D-Bus `app_name`, matched by `osc99_filters` (`app`), and the fallback for the `.desktop` icon lookup on Linux/BSD.                                                                                                                                                                                     |
| `g` | Sanitised like `i`. The icon cache is global (not per pane), least-recently-used, 64 entries and 4 MiB. A transmitted icon is cached before the `o=` gate, so a suppressed notification still fills it. A notification with no title and no body but icon data and a `g=` key is not shown; it only primes the cache (kitty caches before its no-title check). |
| `i` | Sanitised: bytes outside `[A-Za-z0-9_+.-]` are removed and the result is cut to 512 bytes; an id that is empty afterwards is absent. `i=0` is an ordinary id.                                                                                                                                                                                                  |
| `n` | Base64 UTF-8; invalid or empty is ignored. The eight standard names are honoured on every leg; any other name is resolved on Linux/BSD only (see icons below).                                                                                                                                                                                                 |
| `o` | Pane-aware (see occasions below). An unrecognised value is ignored.                                                                                                                                                                                                                                                                                            |
| `p` | An unknown value is its own payload type: its payload is discarded and its metadata and `d=` still apply.                                                                                                                                                                                                                                                      |
| `s` | Base64 UTF-8; invalid or empty is ignored. See sounds below.                                                                                                                                                                                                                                                                                                   |
| `t` | Base64 UTF-8; invalid or empty is ignored. The first non-empty type is the D-Bus `category` hint; all are matched by `osc99_filters` (`type`).                                                                                                                                                                                                                 |
| `u` | An unparsable value is ignored. Linux/BSD honours it (the `urgency` hint); the macOS and Windows system legs have no urgency and do not advertise it. The toast shows `2` as an error toast.                                                                                                                                                                    |
| `w` | An unparsable value is ignored; a value below `-1` is `-1`; an integer beyond the 64-bit range is clamped, and a positive value above `u32::MAX` ms is clamped to it. A positive value is enforced by freminal on every tracked leg; see expiry below.                                                                                                         |

### Payload types (`p=`)

| `p=` value | Direction | Payload meaning                                                          |
| ---------- | --------- | ------------------------------------------------------------------------ |
| `title`    | app→term  | Notification title (default). Concatenated across chunks.                |
| `body`     | app→term  | Notification body. Concatenated across chunks.                           |
| `close`    | both      | app→term: close the notification with this `i=`. term→app: close report. |
| `icon`     | app→term  | Icon image bytes (PNG/JPEG/GIF), must be `e=1`. 256x256 recommended.     |
| `alive`    | both      | app→term: liveness poll. term→app: comma list of live ids.               |
| `buttons`  | app→term  | Button labels, U+2028-separated. Escape-safe UTF-8 or base64.            |
| `?`        | both      | Capability query / response (see handshake).                             |

Unknown `p=` values must be ignored (forward compat).

### Identity, chunking, size limits

- `i=` ties chunks together. Send the title/body across multiple escape codes;
  the terminal concatenates same-typed payloads.
- `d=0` on every chunk except the last; `d=1` (or absent, default) finalizes and
  triggers display.
- Per-chunk payload limit: **2048 bytes before encoding** or **4096 bytes after
  encoding**. Terminals may impose a sane total cap (DoS guard).
- A notification with an `i=` that is still displayed is **replaced**; otherwise
  a new one is displayed. A notification with no `i=` is never an update: two
  id-less notifications are two unidentified notifications.
- `p=close` and `p=alive` with no `i=`: a no-op (freminal sends no reply).
- A notification with neither title nor body is ignored. If it has body but no
  title, the body is used as the title.
- Identifiers (`i=`, `g=`): characters from `[a-zA-Z0-9_\-+.]` only. **Terminals
  must sanitize or reject ids before echoing them in reports** (injection
  guard). `i=0` is the reserved "no id" sentinel for backwards compatibility;
  apps must not use it as a real id.

freminal's limits and reassembly:

- Receiver leniency: a sequence (metadata plus payload) may be up to 1 MiB, more
  than the senders' 2048 and 4096 byte limits, and a notification up to 1 MiB in
  total (payloads plus the cost of its list metadata). A sequence over 1 MiB, or
  with more than 64 metadata items, is rejected by the parser (debug-logged, no
  payload bytes).
- A rejected non-final chunk does not leave a truncated notification behind: the
  handler recovers the chunk's `i=` and `d=` from the raw metadata, best effort,
  and tombstones the transfer, so the rest of it, final chunk included, emits
  nothing. A rejected id-less chunk abandons the unidentified slot only when
  something is in it.
- Per-notification caps, each of which drops the transfer the same way (a
  tombstone for a non-final chunk): the title, body and buttons payloads are held
  to 64 KiB each (finalisation cuts them far shorter anyway), the icon payload to
  1 MiB, and `n=`, `t=` and `a=` entries to 64 each.
- A handler-wide budget of 8 MiB covers every in-flight notification together,
  the unidentified slot included (payload bytes plus list-metadata cost). A chunk
  that would pass it first evicts the least recently touched other entries, which
  are removed, not tombstoned; if the chunk's own notification still does not fit,
  the transfer is dropped. The budget is checked after a chunk lands, so it can be
  exceeded by at most one chunk before eviction.
- Chunks with no `i=` share one unidentified slot, like kitty's single pending
  command keyed by "no id". An identified content chunk discards an accumulating
  unidentified slot (a different id replaces kitty's one pending command); a
  dropped transfer's tombstone in that slot is kept.
- Identified chunks are held in a map of at most 128 ids. A new id at the cap
  evicts the least recently touched entry (a dropped transfer's tombstone
  included), so the map never refuses a chunk. A complete single-chunk
  notification with no pending entry is finalised without entering the map.
- Metadata merges across chunks: scalars (`c o u w f g s`) take the latest
  explicit value, `n` and `t` concatenate, `a=` edits accumulate, and `e`, `p`
  and `d` are per chunk. Defaults apply once, at finalisation.
- `p=?`, `p=close` and `p=alive` act at once whatever `d=` says and never touch
  the pending map.
- At finalisation: C0 controls except LF, DEL and the C1 controls are removed; a
  title or body longer than 1024 characters is cut to its first 1024 characters
  with no marker (kitty's `limit_size`); at most 8 buttons are kept, each cut the
  same way at 256 characters; an empty title takes the body. A button label that
  is empty before stripping is not a button; one that strips to nothing becomes
  U+FFFD, so later button numbers do not shift. App name, types, icon names and
  the sound name also lose LF.
- A plain (`e=0`) payload that is not UTF-8 decodes lossily (U+FFFD).

### Reports written back to the application (reverse PTY path)

The GUI sends each report to the originating pane's PTY thread as a structured
`GuiReply` (`InputEvent::Reply`), and the handler frames it like every other
reply, honouring S8C1T (Task 130). Ids, button numbers and live-id lists are
sanitised to the identifier set when they are written. The `p=?` handshake never
reaches the GUI: the PTY thread answers it itself.

Activation (only when `a=report` was set):

```text
ESC ] 99 ; i=<id> ; <button-index-or-empty> ST
```

- Whole-notification activation: empty payload — `ESC ] 99 ; i=<id> ; ST`.
- Button activation: 1-based button number as the payload —
  `ESC ] 99 ; i=<id> ; 1 ST` for the first button.
- If the notification had no `i=`, use `i=0` in the report.
- The terminal **must not** send a report unless `a=report` is set.

Close (only when `c=1` was set):

```text
ESC ] 99 ; i=<id> : p=close ; ST
```

- On platforms that cannot track close (e.g. macOS), reply immediately with the
  literal payload `untracked`: `ESC ] 99 ; i=<id> : p=close ; untracked ST`.
  freminal does this on macOS and Windows, when the notification is shown.
- If a notification is updated, the close event is not sent unless the updated
  notification also requested one.
- If both `a=report` (activated) and `c=1`, both the activation and close reports
  are sent.

Alive (liveness polling, for `untracked` platforms):

```text
app→term:  ESC ] 99 ; i=<any-id> : p=alive ; ST
term→app:  ESC ] 99 ; i=<any-id> : p=alive ; id1,id2,id3 ST
```

- The `i=` in the response echoes the request's `i=` (multiplexer routing).
- Payload is the comma-separated list of currently-live notification ids.
- freminal lists the requesting pane's identified instances, sorted and without
  duplicates. Other panes' instances are never listed. An instance is listed
  while one of its legs is open and has not been asked to close: `p=close` and
  expiry remove it at once, while an activated `c=1` instance whose desktop
  notification is still displayed stays listed until the daemon closes it, as in
  kitty. A display on the macOS/Windows system leg alone is shown, answered and
  forgotten at once, so it is never listed.

### Capability handshake (`p=?`)

```text
app→term:  ESC ] 99 ; i=<id> : p=? ; ST
term→app:  ESC ] 99 ; i=<id> : p=? ; key=value : key=value ST
```

An id-less query is answered with `i=0`. The response's capability keys (after
the second `;`, colon-separated) — freminal advertises **only what it actually
implements** for the configured routing, the platform and the notification
daemon:

| Key | Value / rule                                                                        |
| --- | ----------------------------------------------------------------------------------- |
| `a` | comma list of supported actions. If none supported, omit `a` entirely.              |
| `c` | `c=1` if close events supported; otherwise omit `c`.                                |
| `o` | comma list of supported occasions. If none, send `o=always`.                        |
| `p` | comma list of supported payload types. Must contain at least `title`.               |
| `s` | comma list of supported sound names. Should include at least `system` and `silent`. |
| `u` | comma list of supported urgency values. If unsupported, omit `u`.                   |
| `w` | `w=1` if auto-expiry supported; otherwise omit.                                     |

Detection: the app sends `p=?` then a DA1 request; if only the DA1 response comes
back, OSC 99 is unsupported.

freminal's answer:

- It is written on the PTY thread in stream order, so it precedes the answer to
  a DA1 sent after it in the same write.
- The keys come in kitty's order: `a`, `o`, `u`, `p`, `c`, `w`, `s`. The `p` list
  is in the order `title,body,?,close,icon,alive,buttons`; `title`, `?`, `close`
  and `alive` are always present and `body`, `icon` and `buttons` only when
  displayed. `p=close` stays advertised on macOS and Windows although the
  desktop notification there cannot be closed: kitty advertises it
  unconditionally, and the request still closes the toast leg and forgets the
  instance. `o=always,unfocused,invisible` is always sent. `a` lists `focus`
  and `report` as supported and is omitted when neither is. `u=0,1,2` is sent
  only when urgency is advertised. `s` is `system,silent`, plus
  `error,info,question,warn,warning` when the standard names are honoured.
- **OSC 99 is unsupported** (every request is ignored before reassembly and
  `p=?` is not answered, so the application sees a terminal that does not speak
  OSC 99) when `[notifications] enabled` is false, `osc_99` is false,
  `routing_osc99` is `disabled`, or `routing_osc99` is `system` and the
  Linux/BSD session bus cannot be reached. Turning support off discards
  every in-flight transfer.
- What is advertised follows the legs the routing uses. Each leg has a
  capability set:

| Capability           | Toast | Linux/BSD system     | macOS system | Windows system |
| -------------------- | ----- | -------------------- | ------------ | -------------- |
| Activation report    | yes   | daemon has `actions` | no           | no             |
| Focus on activation  | yes   | daemon has `actions` | no           | no             |
| Close events (track) | yes   | yes                  | no           | no             |
| Auto-expiry          | yes   | yes                  | no           | no             |
| Urgency              | yes   | yes                  | no           | no             |
| Standard sound names | no    | daemon has `sound`   | no           | no             |
| Body                 | yes   | daemon has `body`    | yes          | yes            |
| Buttons              | yes   | daemon has `actions` | no           | no             |
| Icon image data      | no    | yes                  | yes          | yes            |

- The Icon image data row is the `icons` capability, which `p=?` advertises as
  `icon`: it is about a transmitted `p=icon` payload. The eight standard icon
  names (`n=`) are honoured on every leg whatever it says: a bundled-font glyph on
  the toast, a rendered glyph image on macOS and Windows, a freedesktop icon on
  Linux/BSD.
- The routing combines its legs:
  - `toast` and `system` use their one leg.
  - `both` shows both legs together. A feature that one leg suffices for
    (report, focus, urgency, sound names, body, buttons, icons) is advertised if
    either leg has it. A per-notification guarantee (close events, auto-expiry)
    is advertised only if both legs have it. With no system leg, only the toast
    shows.
  - `system_when_unfocused` shows one leg or the other depending on window
    focus, so every feature needs both legs. With no system leg (the Linux/BSD
    session bus cannot be reached) every notification routes to the toast at
    every focus state, as `both` does, so it advertises the toast leg's
    features.
- The Linux/BSD daemon's capabilities (`body`, `actions`, `body-markup`,
  `sound`) come from `GetCapabilities`. Until the daemon answers, `body`,
  `actions` and `body-markup` are assumed and `sound` is not. A later answer, or
  an unreachable bus, is re-broadcast to every pane so the next query is
  truthful. Capabilities are fetched again after every change of the daemon's
  owner.

Example replies (written without the spaces the spec shows around the colons):

```text
toast, any platform:
a=focus,report:o=always,unfocused,invisible:u=0,1,2:p=title,body,?,close,alive,buttons:c=1:w=1:s=system,silent

system, Linux/BSD, daemon with body, actions and sound:
a=focus,report:o=always,unfocused,invisible:u=0,1,2:p=title,body,?,close,icon,alive,buttons:c=1:w=1:s=system,silent,error,info,question,warn,warning

system, macOS:
o=always,unfocused,invisible:p=title,body,?,close,icon,alive:s=system,silent

system, Windows:
o=always,unfocused,invisible:p=title,body,?,close,icon,alive:s=system,silent

both, macOS:
a=focus,report:o=always,unfocused,invisible:u=0,1,2:p=title,body,?,close,icon,alive,buttons:s=system,silent

system_when_unfocused, macOS:
o=always,unfocused,invisible:p=title,body,?,close,alive:s=system,silent

system_when_unfocused, Linux/BSD with the session bus unreachable (the toast
leg, the same reply as toast routing):
a=focus,report:o=always,unfocused,invisible:u=0,1,2:p=title,body,?,close,alive,buttons:c=1:w=1:s=system,silent
```

### Icons, sounds, occasions, urgency, expiry

- Icon by name (`n=`, base64): required names any impl must resolve:
  `error`, `warn`/`warning`, `info`, `question`, `help`, `file-manager`,
  `system-monitor`, `text-editor`. App-id names use the `.desktop` stem (Linux)
  or bundle id (macOS). With no icon, the app name (`f=`) is used to find one.
- Icon by data (`p=icon`, `e=1`): PNG/JPEG/GIF. If both `n=` and `p=icon` given,
  a locally-found named icon wins; the transmitted image is the fallback.
- Icon cache (`g=`): cache transmitted data under this key for the session; later
  notifications can reuse it by sending `g=` alone. The cache may be bounded and
  evict the least recently used entry.
- Standard sound names: `system`, `silent`, `error`, `warn`/`warning`, `info`,
  `question`. Others are implementation-defined (e.g. freedesktop names on Linux).
- Occasion `o=`: `always` (default), `unfocused` (source window lacks keyboard
  focus), `invisible` (unfocused and not visible, e.g. inactive tab).
- Urgency `u=`: `0` low, `1` normal, `2` critical.
- Expiry `w=`: `-1` OS default, `0` never (best effort), `>0` milliseconds.
  Positive values are robust because the terminal can close the notification
  itself.

freminal:

- **Linux/BSD icons** are chosen in kitty's order. Each `n=` name in order: a
  standard name maps to its freedesktop name (`error` to `dialog-error`,
  `warn`/`warning` to `dialog-warning`, `info` to `dialog-information`,
  `question` to `dialog-question`, `help` to `system-help`, `file-manager` to
  `system-file-manager`, `system-monitor` to `utilities-system-monitor`,
  `text-editor` to `utilities-text-editor`); any other name is used when the XDG
  icon scan finds it (the scan runs once on a background thread, so until it
  finishes unknown names fall through, and the on-disk spelling of the match is
  what is sent). If no name matched, the transmitted data is used, else the
  first name verbatim. With no names: the data, else the icon of the `.desktop`
  file whose stem or `Name=` equals `f=`, else freminal's own. Transmitted data
  that does not decode counts as no data. Decoding uses the `image` crate (PNG,
  JPEG, GIF, WebP; at most 4096x4096 and 64 MiB), applies the EXIF orientation,
  and downscales anything over 256 pixels to fit 256x256 (premultiplied, so
  transparent edges do not darken); the decoded pixels are sent as the
  `image-data` hint, as one byte array, and the last 8 decoded icons are kept.
  Every notification also carries the `desktop-entry` hint `freminal`.
- **Linux/BSD sounds:** `system` sends no sound hint, `silent` sends
  `suppress-sound`, `error`, `warn`, `warning`, `info` and `question` send
  `sound-name` `dialog-error`, `dialog-warning`, `dialog-information` and
  `dialog-question`, and any other name is sent verbatim as `sound-name`. The
  standard names are advertised only when the daemon has `sound`.
- **macOS and Windows sounds:** `silent` is silent; `system` and any named sound
  play the platform default. Standard sound names are not advertised.
- **macOS and Windows icons:** names come first, as on Linux. The first `n=`
  name that is one of the eight standard names is drawn from the bundled
  `CaskaydiaCove` glyph for it to a 256x256 PNG (a white glyph, its coverage as
  alpha, centred on an opaque `#5A5A5A` rounded square tile inside a margin, so
  it reads on light and dark backgrounds) and shown as the notification image;
  each standard icon is drawn once per process. With no standard name, a
  transmitted icon is shown: it is decoded within the limits above and
  re-encoded as a PNG, so the OS never decodes the bytes the program sent. If
  the drawing or the decode fails there is no image. A name that is not standard
  cannot be resolved on these platforms (neither OS API takes a freedesktop icon
  name) and is ignored. Only the eight standard names are mandatory, so this is
  allowed.
- **Toast:** the first `n=` name that is standard shows as its bundled-font
  glyph in the toast's icon slot (the accent colour still follows the urgency);
  any other name is ignored, and a transmitted icon image is never shown. No
  sounds. Urgency `2` is an error toast that stays until dismissed, `1` is an
  info toast with its default lifetime (6 seconds), `0` is an info toast for 3
  seconds (all three when `w=` is `-1` or unset; `w=` overrides). A body click
  activates, each button reports its 1-based number, the dismiss square closes,
  and the toast shows a hand cursor with hover and pressed fills on buttons.
  More than five toasts evict the oldest that is not waiting to be dismissed
  first.
- **Occasions** are pane-aware. A pane is _focused_ when its OS window is focused,
  its tab is active and it is the tab's active pane. It is _visible_ when its OS
  window is focused and not minimised, its tab is active and no other pane in the
  tab is zoomed. `unfocused` shows only when the pane is not focused;
  `invisible` only when it is neither focused nor visible. (The choice of leg
  under `routing_osc99` still follows OS-window focus; that is freminal's own
  routing, not the protocol's.) Visibility requires OS-window focus because
  freminal cannot tell an occluded window from a merely unfocused one, so a pane
  in an unfocused OS window counts as not visible even when it is fully on
  screen, and `o=invisible` shows there. Minimisation comes from the window
  system's viewport information, which a Wayland compositor does not provide:
  there a minimised window reads as restored.
- **Expiry:** a positive `w=` is enforced by freminal, measured from display (an
  update restarts it), on every tracked leg (toast and Linux/BSD): at the
  deadline every leg is closed whatever the OS or hover state, and a close
  report follows with `c=1`. The toast of a `w=` notification has no timer of its
  own (it stays until dismissed), so hovering it cannot hold it past the
  deadline; only the default and low-urgency toast lifetimes pause while the
  pointer is over them. The value is also sent to the Linux daemon as
  `expire_timeout`. `w=0` never expires (toast: until dismissed). On macOS and
  Windows `w=` is passed as the OS timeout (a short or long toast on Windows,
  ignored on macOS) and freminal does not enforce it, so `w=1` is not advertised
  there.

### Allowing users to filter notifications

Well-behaved applications identify themselves with `f=` (application name,
ideally the `.desktop` stem or bundle id) and `t=` (notification type, may
repeat), both base64 UTF-8. They exist so users can filter notifications.

freminal: `[notifications] osc99_filters` is a list of `location:regex` rules
(and the bare rule `all`); locations are `title`, `body`, `app` (the `f=` value)
and `type` (any `t=` value). The regex is a search, case-sensitive (use `(?i)`),
built with a 1 MiB size limit. An absent body or app name is matched as the empty
string, as in kitty, so `body:^$` matches a notification with no body. Any
matching rule drops the notification silently: it is not shown, not registered
and gets no reply. The filter runs after the `o=` gate, as in kitty. An invalid
rule is logged and ignored. There is no `and` / `or` / `not`. A change applies
on config reload; if the registry is busy when it is applied, it is retried
every frame until it lands.

### Encoding rules

- Escape-safe UTF-8: valid RFC 3629 UTF-8 with **no** C0 (U+0000–U+001F), DEL
  (U+007F), or C1 (U+0080–U+009F) codepoints (so no newlines/tabs/CR).
- Base64: RFC 4648 standard alphabet. When chunking base64, either chunk before
  encoding (≤2048 raw bytes/chunk, include padding) or after encoding (≤4096
  bytes/chunk, padding only on the last chunk; terminals handle either).
- Title and body are plain text; no markup is allowed. On a Linux/BSD daemon that
  interprets markup (`body-markup`), freminal inserts U+200C after every `<` and
  `&` so they cannot form a tag or an entity, as kitty does.

### freminal current-state: notifications — conformant (Task 138, v0.13.0)

The feature shipped in Task 99 (v0.11.0) and was made conformant by Task 138
(v0.13.0), with the shared groundwork of Tasks 128 to 132. The pieces:

- **Parser** (`freminal-common/src/buffer_states/osc_notify_99.rs`):
  `parse_osc_99` turns one sequence into an `Osc99Command` in which every field
  with a default is unset-aware, so reassembly can merge chunks. An invalid value
  ignores its key; only the size guards are errors, and a rejected sequence is
  reported to the handler as `AnsiOscType::Notify99Rejected` so it can abandon
  the transfer it belonged to.
- **Reassembly and finalisation**
  (`freminal-terminal-emulator/src/terminal_handler/notify_99.rs`):
  `TerminalHandler::handle_osc99` is the single entry point. It answers `p=?`
  itself, forwards `p=close` and `p=alive` with an id as
  `WindowManipulation::Osc99Control`, and reassembles content into the typed
  transport `Notification99Data`
  (`freminal-common/src/buffer_states/osc99_notification.rs`; every flag is a
  named domain enum).
- **Registry** (`freminal/src/gui/osc99/registry.rs`): a pure state machine. Each
  displayed notification is an instance keyed by pane and `i=`, with the legs it
  is shown on. It returns effects (reply, show, close, focus) that a thin
  executor applies. A pane holds at most 32 instances (its own oldest is
  forgotten first, so a noisy pane cannot evict another pane's), and 128 overall
  as a backstop.
- **Legs:** the keyed toast (`freminal/src/gui/toast/`), the Linux/BSD zbus
  service (`osc99/dbus_service/`, `presentation.rs`, `icon_lookup.rs`,
  `icon_decode.rs`), and the untracked macOS/Windows backend
  (`osc99/untracked.rs`, `icon_raster.rs`, `icon_tempfiles.rs`,
  `icon_decode.rs`).
- **Routing and capabilities:** `route_osc99` in `freminal/src/gui/notifications.rs`
  applies the `o=` gate, the filters and `routing_osc99`; a frame's requests
  (displays, controls and icon primings) are routed strictly in arrival order by
  `osc99/glue.rs`. `freminal/src/gui/host_capabilities.rs` holds the leg tables
  and the pure combination function behind `p=?`.
- **Config:** `[notifications] enabled`, `osc_99`, `routing_osc99` (`toast`,
  `system`, `both`, `system_when_unfocused`, `disabled`) and `osc99_filters`.
  See `config_example.toml`.

Lifecycle summary:

- Update in place, `p=close` and `p=alive` see only the requesting pane's
  instances. An update replaces every leg in place (toast keyed replace, D-Bus
  `replaces_id`) and sends no close report for the replaced notification; the
  untracked leg (macOS, Windows) cannot update, so it shows another
  notification.
- `p=close` and expiry close every open leg (`CloseNotification` on Linux/BSD).
- Activation from any leg sends the activation report (when `a=report`), focuses
  (when `focus`: switch to the pane's tab, unzoom, activate the pane, raise the
  OS window; a compositor may refuse the raise, and freminal does not consume the
  D-Bus `ActivationToken`), and closes the other legs. Without `c=1` the instance
  is then forgotten; with `c=1` exactly one close report is sent once every
  tracked leg has closed.
- On Linux/BSD the signals `ActionInvoked` and `NotificationClosed` are believed
  only from the current owner of `org.freedesktop.Notifications`, so another
  session-bus client cannot forge an activation or a close. `GetNameOwner` and
  `NameOwnerChanged` follow a daemon restart. Each mapping records the unique
  name that answered its `Notify`, and an owner change closes only the instances
  recorded under a different owner. If `GetNameOwner` fails or finds nobody, the
  sender of the next successful `Notify` reply is adopted as the owner.
- `GetNameOwner` and `GetCapabilities` carry `NO_AUTO_START`, so starting freminal
  never activates a daemon; the first real `Notify` does, and the resulting owner
  change refreshes the capabilities. Every call is bounded at 25 seconds. After
  a timed-out call, shows fail and closes complete without calling the daemon for
  30 seconds. If either signal pump ends, every mapped instance is reported
  closed and the service becomes unavailable; after a failed connect the next
  show reconnects, at most once per 30 seconds.
- An instance whose pane is gone is pruned every frame; its notifications stay on
  screen and produce no replies.
- An unreachable session bus makes `system` routing unsupported, and `both` and
  `system_when_unfocused` the toast alone (advertised and routed alike); a show
  that cannot be queued ends that notification's system leg.
- Requests that find a GUI cell busy are deferred to the next frame, not dropped
  (at most 256 are held); the repaint wake falls back to any live window.

### Documented differences from kitty

freminal is MIT and kitty is GPLv3, so kitty's behaviour was read for what it
does, not copied. These are the places freminal deliberately differs:

1. **`a=` edits accumulate across chunks.** kitty merges chunks by union, so a
   later chunk without `a=` can bring back a `focus` an earlier chunk removed.
   freminal applies every edit in order and resolves them once.
2. **Control payloads do not discard a pending transfer.** `p=?`, `p=close` and
   `p=alive` act at once as in kitty, but kitty also discards the window's
   pending command; freminal leaves the pending map untouched.
3. **Control stripping is wider.** Besides title and body, freminal strips
   control characters (and LF, for the names) from button labels, icon names,
   the app name, types and the sound name, because they are shown in the toast
   and sent over D-Bus. kitty leaves those alone.
4. **Pane-scoped `p=close`.** kitty's `p=close` looks across all windows; freminal
   looks only at the requesting pane's instances (and update and `p=alive` are
   scoped the same way), so one pane cannot close or see another's
   notifications.
5. **Named sounds use the freedesktop hint.** kitty plays named sounds itself
   through libcanberra; freminal has no sound library and sends `sound-name` (and
   `suppress-sound`) to the daemon.

### Platform limits

macOS and Windows give a background thread no way to observe a notification, and
freminal adds no lifecycle code there. Their system leg is untracked: no
activation reports, no close tracking, no update in place, no programmatic close
and no expiry enforced by freminal, no buttons, and only the eight standard icon
names are resolved (as drawn glyph images; otherwise a transmitted icon is
shown). `p=?` omits what they cannot do: no `a`, no `c`, no `w`, no `buttons`,
no `u`, and `s=system,silent`. A `c=1` request is answered with
`p=close;untracked` at display. A display on that leg alone is not kept, so
`p=alive` does not list it; with `both` routing the toast leg keeps the
instance, and `p=alive` lists it until `p=close`, a positive `w=` deadline, the
toast closing, the instance cap or the pane going away removes it.

Delivery on those platforms runs on one long-lived thread
(`freminal-osc99-deliver`) fed by a queue of 16 requests; a request that finds
the queue full is dropped with a debug log, so a flood cannot stall the GUI or
grow memory. Icon files live in a per-process directory,
`freminal-osc99-icons-<pid>` under the temp directory (mode `0700`, files
`0600`, a symlinked or permissive existing directory is refused), are deleted
after 10 minutes unused or past 64 files, and are removed at exit; the OS may
therefore show no image when it redraws an old notification. macOS cannot report
a delivery failure to freminal at all. Windows builds the toast directly with
`tauri-winrt-notification` (the icon is an `appLogoOverride`, attributed to the
PowerShell app id) and has no reminder scenario (it needs buttons, which this leg
cannot report), so the Windows toast has no urgency levels and `u` is not
advertised there.

On Linux/BSD, `actions` (report, focus, buttons), `body` and `sound` follow the
daemon. The toast, on every platform, plays no sounds and shows no transmitted
icon images (standard icon names show as glyphs).

---

## Graphics protocol completion

Reference for Task 100, now implemented (v0.11.0). Task 13 shipped
transmit/put/delete/query and unicode placeholders; Task 100 added animation
(frame transfer/control/compose), image-number (`I=`) references, relative
placements (`P`/`Q`/`H`/`V`, incl. their parser keys), storage quotas +
eviction, compression (`o=z`), shared memory (`t=s`, POSIX + Windows),
source-rect crop (`a=p` `x/y/w/h`), delete-target correctness, `p=` in
responses, and z-index render ordering. A follow-up live-testing pass
(Tasks 100.11–100.20) fixed the end-to-end render path: animation/compose
repaint, relative-placement origin under scrollback, image persistence across
subsequent output, `C=1` on `a=T`, native-vs-explicit display sizing,
per-placement identity (coexisting placements of one image), the sub-cell
`X`/`Y` offset, and placement-scoped delete. See the "freminal current-state"
section below for the full done list; the graphics surface is complete (only
the `t=f`/`t=t` security-hardening note remains, not a Task 100 item).

### Envelope (graphics)

```text
ESC _ G <control-data> ; <base64-payload> ESC \
```

`ESC _` (APC) is `0x1B 0x5F`. Control data is comma-separated `key=value` pairs.
On chunked transfers (`m=1`), only the first escape carries the full control set;
subsequent chunks carry only `m` (and optionally `q`), plus `a=f` for animation
frames.

### Control key table — meaning depends on the action

**This is the single most important table for Task 100.** Several key letters
mean completely different things depending on `a=`. Getting this wrong is the
main correctness hazard.

Action and global:

| Key | Values            | Default | Meaning                                                                                 |
| --- | ----------------- | ------- | --------------------------------------------------------------------------------------- |
| `a` | `t T q p d f a c` | `t`     | Action: transmit / transmit+display / query / put / delete / frame / animate / compose. |
| `q` | `0 1 2`           | `0`     | Suppress responses: `1` suppress OK, `2` suppress all.                                  |

Transmission (used by `a=t`, `a=T`, `a=f`):

| Key | Values      | Default | Meaning                                                   |
| --- | ----------- | ------- | --------------------------------------------------------- |
| `f` | `24 32 100` | `32`    | Pixel format: RGB / RGBA / PNG.                           |
| `t` | `d f t s`   | `d`     | Medium: direct / file / temp-file / shared-memory.        |
| `s` | u32         | `0`     | Image width (pixels).                                     |
| `v` | u32         | `0`     | Image height (pixels).                                    |
| `S` | u32         | `0`     | Bytes to read from file/shm.                              |
| `O` | u32         | `0`     | Byte offset to read from file/shm.                        |
| `i` | u32         | `0`     | Image id.                                                 |
| `I` | u32         | `0`     | Image number.                                             |
| `p` | u32         | `0`     | Placement id.                                             |
| `o` | `z`         | none    | Compression: `z` = RFC 1950 zlib (applied before base64). |
| `m` | `0 1`       | `0`     | More chunks follow.                                       |

Display (used by `a=p`, `a=T`):

| Key | Values | Default | Meaning                                                           |
| --- | ------ | ------- | ----------------------------------------------------------------- |
| `x` | u32    | `0`     | Source-crop left edge (px) of the region of the image to display. |
| `y` | u32    | `0`     | Source-crop top edge (px).                                        |
| `w` | u32    | `0`     | Source-crop width (px); `0` = full width.                         |
| `h` | u32    | `0`     | Source-crop height (px); `0` = full height.                       |
| `X` | u32    | `0`     | x-offset within the first cell to start drawing.                  |
| `Y` | u32    | `0`     | y-offset within the first cell.                                   |
| `c` | u32    | `0`     | Columns to display over.                                          |
| `r` | u32    | `0`     | Rows to display over.                                             |
| `C` | u32    | `0`     | Cursor policy: `0` move after image, `1` do not move.             |
| `U` | u32    | `0`     | `1` = create a virtual placement for a unicode placeholder.       |
| `z` | i32    | `0`     | z-index (stacking order).                                         |
| `P` | u32    | `0`     | Parent image id (relative placement).                             |
| `Q` | u32    | `0`     | Parent placement id (relative placement).                         |
| `H` | i32    | `0`     | Horizontal cell offset from parent (relative placement).          |
| `V` | i32    | `0`     | Vertical cell offset from parent (relative placement).            |

Animation frame load (`a=f`) — reused letters with new meaning:

| Key     | Meaning under `a=f`                                                                            |
| ------- | ---------------------------------------------------------------------------------------------- |
| `x` `y` | Destination origin (px) within the frame where the transmitted rect is written.                |
| `s` `v` | Width/height of the transmitted rectangle (transmit-group meaning).                            |
| `c`     | 1-based frame number whose data is the base canvas (`c=1` = root). Default: black/transparent. |
| `r`     | 1-based frame number to **edit** (patch into an existing frame). Default: create new frame.    |
| `z`     | Gap-to-next-frame (ms). `0` ignored, negative = gapless, default `40ms` (root default `0`).    |
| `X`     | Compose mode: default alpha-blend, `1` = overwrite.                                            |
| `Y`     | Background color as 32-bit RGBA integer for unspecified pixels. Default `0`.                   |

Animation control (`a=a`) — reused letters with new meaning:

| Key | Meaning under `a=a`                                                                     |
| --- | --------------------------------------------------------------------------------------- |
| `s` | `1` stop, `2` run in loading mode (wait for frames at end), `3` run normally (loop).    |
| `r` | 1-based frame number being affected (gap target).                                       |
| `z` | Gap (ms) for the frame named by `r`. `0` ignored, negative = gapless.                   |
| `c` | 1-based frame number to make the current frame (client-driven step).                    |
| `v` | Loop count: `0` ignored, `1` = infinite (default), `N>=2` = play `N-1` loops then stop. |

Animation compose (`a=c`) — reused letters with new meaning:

| Key     | Meaning under `a=c`                                                     |
| ------- | ----------------------------------------------------------------------- |
| `r`     | 1-based **source** frame number (overlaid data).                        |
| `c`     | 1-based **destination** frame number (edited).                          |
| `x` `y` | Top-left (px) of the **destination** rectangle.                         |
| `X` `Y` | Top-left (px) of the **source** rectangle.                              |
| `w` `h` | Rectangle size (px), same for source and destination; `0` = full image. |
| `C`     | Compose mode: default alpha-blend, `1` = overwrite.                     |

Delete (`a=d`): the `d=` target selector, table below.

### Animation semantics summary

- `a=f` (frame transfer): pastes a (possibly partial, `x/y/s/v`) rectangle onto a
  canvas that is either a background color (`Y=`), a previous frame (`c=`), or an
  edited existing frame (`r=`), using alpha-blend or overwrite (`X=`). Sets the
  frame gap (`z=`). Chunkable like image transmit; subsequent chunks need `a=f` +
  `m`.
- `a=a` (control): client-driven step (`c=`), or terminal-driven run/stop (`s=`)
  with loop count (`v=`) and per-frame gap (`r=`/`z=`). "Loop N times then stop" =
  `v=N+1`. Stopping resets the loop counter. The root frame's gap must be set via
  this control (it defaults to `0`).
- `a=c` (compose): copies a pixel rectangle from source frame (`r=`, offset
  `X/Y`) onto destination frame (`c=`, offset `x/y`), size `w/h`, blend `C`.
  Errors: `ENOENT` (frame/image missing), `EINVAL` (out of bounds, or same-frame
  overlapping rects), `ENOSPC` (compose forced a full render past quota).

### Unicode placeholders

- Placeholder codepoint: **U+10EEEE** (UTF-8 `F4 8E BB AE`).
- Create a virtual placement first: `ESC _ G a=p,U=1,i=<id>,c=<cols>,r=<rows> ESC \`
  (or fold into `a=T`). The image should have been transmitted quietly (`q=2`).
- Image id lower bits ride in the **foreground color** (8-bit in 256-color, 24-bit
  in truecolor); the **most-significant byte** rides in an optional 3rd diacritic.
- Placement id rides in the **underline color** (0 = any virtual placement).
- Cell position: 1st diacritic = row (0-based), 2nd diacritic = column (0-based).
  `U+0305`→0, `U+030D`→1, `U+030E`→2, ... per `rowcolumn-diacritics.txt`.
- Missing diacritics inherit left-to-right from the previous placeholder cell
  (same fg+underline color) per the 3 inheritance rules.
- Virtual placements are only deletable via `d=` in `{i, I, r, R, n, N}`;
  positional deletes never touch them.

freminal already implements this (dedicated `unicode_placeholder.rs`, 297-entry
diacritic table, inheritance rules, tests). Task 100.3 mainly verifies conformance
and closes the `image_number` reference-by-number gap.

### Relative placements (implemented — Task 100.4)

Relative placements are part of the **graphics protocol proper**, transmitted in
the same `ESC _ G ... ESC \` APC envelope with `a=p` — they are _not_ a separate
CSI extension. `P`/`Q`/`H`/`V` are typed fields on `KittyControlData` (added in
foundation subtask 110.0) and are handled by a dedicated relative-placement path:
real-parent placements get cell-stamp positioning (recorded at place time);
virtual-placeholder parents get render-time position derivation. Error handling
(`ENOPARENT`, `ECYCLE`, `ETOODEEP` at depth ≥ 8, and `EINVAL` for a
virtual-relative parent) and cascade delete are implemented; the cursor never
moves for a relative placement (Task 100.4a/100.4b).

```text
ESC _ G a=p,i=<id>,p=<placement>,P=<parent_img>,Q=<parent_placement> ESC \
```

- `P=`/`Q=` name the parent image/placement; `H=`/`V=` offset in cells (positive =
  right/down, origin = parent top-left).
- Lifetime is tied to the parent: parent deleted ⇒ child deleted; a parent plus
  its relatives form a group. Chains are allowed; implementations must support
  depth ≥ 8.
- The cursor must **not** move after a relative placement, regardless of `C`.
- A virtual placement may be a parent, but cannot itself be made relative
  (`EINVAL`). A virtual parent's position is the min x / min y of its placeholder
  cells.
- Errors: `ENOPARENT` (missing parent), `ECYCLE` (would form a cycle),
  `ETOODEEP` (chain exceeds max depth).

### Delete targets (`d=`)

Lowercase = delete placements only (keep image data); uppercase = also free image
data (if not referenced in scrollback). `x/y` are cursor-style 1-based cells.

| `d=`    | Deletes                                                    |
| ------- | ---------------------------------------------------------- |
| `a` `A` | All placements visible on screen.                          |
| `i` `I` | Images with id `i=` (optionally placement `p=`).           |
| `n` `N` | Newest image with number `I=` (optionally placement `p=`). |
| `c` `C` | Placements intersecting the cursor cell.                   |
| `f` `F` | Animation frames.                                          |
| `p` `P` | Placements intersecting cell `x=`,`y=`.                    |
| `q` `Q` | Placements intersecting cell `x=`,`y=` with z-index `z=`.  |
| `r` `R` | Images with id in `[x, y]` (added kitty 0.33.0).           |
| `x` `X` | Placements intersecting column `x=`.                       |
| `y` `Y` | Placements intersecting row `y=`.                          |
| `z` `Z` | Placements with z-index `z=`.                              |

When all placements for an image are gone and the uppercase form was used, the
image is deleted. Under quota pressure, images without placements are evicted
first. A delete received mid-chunked-upload aborts the partial upload.

### Transmission media (`t=`)

| `t=` | Medium                                                                                                                                                                                                                                    |
| ---- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `d`  | Direct — data in the escape payload (implemented).                                                                                                                                                                                        |
| `f`  | Regular file — implemented via `read_kitty_file`; hardened by Task 126.2 (see the security note below). `S=`/`O=` are not honoured for files (Task 135).                                                                                  |
| `t`  | Temp file — as `f`, then deleted only if the canonical path is inside a temp directory (`temp_dir()`, `/tmp`, `/dev/shm`) and contains `tty-graphics-protocol` (Task 126.2).                                                              |
| `s`  | Shared-memory object; read `S` bytes at offset `O`. POSIX: `shm_open`/`mmap`, then `shm_unlink`+close. Windows: `OpenFileMappingW`/`MapViewOfFile`, then unmap+close (no unlink; `S=` required). Payload = object name. Both implemented. |

Security (upstream spec): refuse device/socket/special files; may refuse
`/proc`, `/sys`, `/dev`. **freminal current state (Task 126.2):** the path must
be absolute UTF-8; it is canonicalised and refused under `/proc`, `/sys` or
`/dev` (except `/dev/shm`) before opening; it is opened `O_NONBLOCK |
O_NOFOLLOW` (unix), must be a regular file, and is re-resolved after opening
with its `(dev, ino)` required to match the opened handle; reads are capped at
400 MiB. Every failure is answered with `EBADF:Failed to read image file`.
A `t=t` deletion is bound to the file that was read: the entry is re-opened
relative to a parent-directory handle, its identity checked, then `unlinkat`
(unix; pathname removal elsewhere).

### Responses and error codes

```text
ESC _ G i=<id>[,p=<placement>] ; <OK-or-ERROR[:detail]> ESC \
```

- `p=` is included in the response **only if** the request specified a non-zero
  `p=`. (freminal's `format_kitty_response_body` emits `p=<pid>` for non-zero
  placements as of Task 100; renamed from `format_kitty_response` and framed
  S8C1T-aware by `write_apc_response` since Task 129.13.)
- `q=1` suppresses OK; `q=2` suppresses all. Message is printable ASCII.

Named error codes: `ENOENT`, `EINVAL`, `ENOTSUP`, `ETOODEEP`, `ECYCLE`,
`ENOPARENT`, `ENOSPC`. Error form is `CODE:detail`; success is bare `OK`.

### Compression (`o=z`)

`o=z` marks the payload as RFC 1950 zlib-deflated (before base64). Decompress
before interpreting pixels/PNG. Valid for any `f=`. With PNG + compression,
provide `S=` (source data size). Implemented in Task 100 (RFC 1950 inflate
via `flate2`/`miniz_oxide`).

### Storage quotas

kitty's reference values (freminal should mirror as named constants unless a
reason to differ):

| Budget                  | kitty value        |
| ----------------------- | ------------------ |
| Base image storage      | 320 MB per buffer  |
| Animation frame storage | 5x base (~1600 MB) |

Eviction: LRU on overflow, preferring images without placements. The protocol
floor is "at least a few full-screen images"; the exact number is an
implementation choice.

### freminal current-state: graphics — implemented in v0.11.0 (Task 100)

- Relative placements: `P`/`Q`/`H`/`V` are typed fields on `KittyControlData`
  (110.0) and fully handled: real-parent cell-stamp positioning at place time,
  virtual-placeholder-parent render-time position derivation, `ENOPARENT`/
  `ECYCLE`/`ETOODEEP` (depth ≥ 8)/virtual-relative `EINVAL`, cascade delete, and
  the cursor never moves for a relative placement (Task 100.4a/100.4b).
- Animation `a=f` (frame transfer), `a=a` (control: run/stop/loop, current
  frame, per-frame gap), and `a=c` (compose) are implemented with the
  action-dependent key re-aliasing documented in the control-key table above.
  `InlineImage` carries a frame model (`frames`, `root_gap_ms`,
  `AnimationControl`); the GUI selects the current frame by wall-clock and the
  renderer re-uploads the texture on frame change (Tasks 100.2a/100.2b/100.2c).
- Image number (`I=`) reference-by-number is implemented: transmit records
  number→id in the image store; `a=p`/`a=f` resolve a bare `I=` to the
  newest-transmitted image with that number; the response echoes
  `i=<id>,I=<n>` (Task 100.3).
- `o=z` (RFC 1950 zlib, via `flate2`) is decompressed before interpreting
  pixels/PNG; `t=s` shared memory is implemented on both POSIX
  (`shm_open`/`mmap`/`shm_unlink` via the `nix` crate's `"mman"` feature) and
  Windows (`OpenFileMappingW`/`MapViewOfFile`/`UnmapViewOfFile`/`CloseHandle`
  via `winapi`; Task 100.10); `O=` byte offset is applied when reading a
  file/shm object (Tasks 100.6, 100.10).
- `format_kitty_response(image_id, ok, message)` (since Task 129.13
  `format_kitty_response_body`) and `send_kitty_error` now
  take a placement id and emit `,p=<pid>` in the response when the request had
  a non-zero placement id (Tasks 100.2a, 100.3).
- Image storage quota + LRU eviction is implemented in `ImageStore` (base
  320 MB, animation budget 5× base; eviction prefers placement-less images,
  then oldest) (Task 100.5).
- Delete-target correctness (Task 100.7a): lowercase `d=` targets remove
  placements only and keep image data; uppercase targets additionally free the
  image data if no placement (including scrollback) still references it. Added
  the previously-missing `d=q`/`d=Q` (cell + z-index), `d=r`/`d=R` (id range,
  kitty 0.33.0+), and `d=f`/`d=F` (delete frames) variants; `d=a` (visible-only)
  vs `d=A` (all) is distinguished; the earlier `d=n`/`d=N` store-free bug is
  fixed.
- Image quads render in `(z-index, placement-instance)` order, with higher
  z-index drawn on top of lower (Task 100.7b; the tie-break basis changed from
  image id to placement-instance in Task 100.18) — the earlier id-only sort
  order was a real stacking bug and is fixed.
- Source-rect crop for `a=p`/`a=T` (Task 100.9): the `x`/`y`/`w`/`h` display
  keys select a pixel sub-rectangle of the transmitted image; the crop rides
  the per-placement `ImagePlacement.source_crop` and `compute_image_quad`
  composes it into the UV window (`w=0`/`h=0`/absent = full from the offset).
- Images survive a terminal reflow as coherent rectangles across all three
  supported protocols (Task 100.4.0 atomicity fix).
- A displayed image survives subsequent output: `place_image` leaves a fresh
  blank row below the image and moves the cursor there, so following text no
  longer overwrites (destroys) the image's cells (Task 100.15). `C=1` (no
  cursor movement) is honoured on `a=T`/Put, not only `a=p` (Task 100.16).
- Display sizing honours the spec's native-vs-explicit distinction (Task
  100.17): with no `c`/`r` (kitty), `auto` (iTerm2), or always (sixel) the image
  is drawn at its native pixel size anchored at the cell top-left
  (`ImageSizeMode::NativePixels`); with explicit `c`/`r` (kitty) or
  `width`/`height` (iTerm2) it is scaled to fill the declared cell grid
  (`ImageSizeMode::ExplicitCells`). Previously every image was stretched to fill
  its cell grid.
- Multiple placements of the same image coexist correctly (Task 100.18): per
  the spec, `a=p` puts with placement id `0`/unspecified create distinct
  coexisting placements, and a second put with the same non-zero placement id
  replaces the first. Each placement carries a monotonic `placement_instance`
  id; `build_image_verts` buckets by it (previously by `image_id` alone, which
  merged two on-screen placements of one image into a single oversized quad).
  This also resolved the earlier `z_index`/`source_crop` first-seen-collapse
  limitations for free.
- Sub-cell `X`/`Y` pixel offsets ARE applied (Task 100.19): the `X`/`Y` display
  keys shift the drawing origin within the first cell by that many pixels
  (clamped `< cell size`), via `ImagePlacement.subcell_offset` and an additive
  quad-origin translation in `compute_image_quad`. Orthogonal to size mode
  (position only) and source-crop (UV only). Kitty-only (iTerm2/sixel have no
  such key).
- Unicode-placeholder virtual placements with placement id `0` coexist
  distinctly, and `d=i,p=<n>` deletes only the named placement (Task 100.20).
- Kitty image DATA persists across scroll-out: when a placement scrolls off
  the end of scrollback (or is removed by a lowercase `d=` delete), the
  transmitted image data is retained and stays addressable by `id`/number for
  a later `a=p` put, per the spec's separate-lifetime rule. The scrollback GC
  (`ImageStore::retain_referenced`) is protocol-aware — it only garbage-
  collects cell-owned Sixel/iTerm2 images on scroll-out; Kitty image data is
  released only by an explicit uppercase free/delete or the quota-LRU path.
  (Previously scroll-out freed Kitty data unconditionally, breaking
  transmit-only `a=t` images and lowercase-deleted images referenced again by
  id — PR #382 review fix.)

- **Placements are per screen (Task 131.7).** `virtual_placements`,
  `real_placements` and the scrollback-prune base are kept per screen, as kitty's
  per-screen graphics manager does. `a=d` with `d=a`/`d=A` and the pruners act on
  the active screen only; a screen switch touches neither map, and a clear of the
  alternate screen (`?1049h`, or `?1047l` on the alternate) empties the alternate
  map. RIS clears both screens' maps. Placing an image at the alternate screen's
  bottom row scrolls the screen as a line feed would, instead of growing the
  store (Task 131.C5). Because the alternate screen persists while parked, a
  parked alternate keeps its image store and placements until it is cleared
  (`?1049h`, `?1047l`) or RIS, as kitty's alternate graphics manager does across
  toggles. A real screen switch also ends the placeholder continuity run.

**Closed after Task 100:**

- **`t=f`/`t=t` file-path security** — Task 100 left `read_kitty_file`
  checking only `is_absolute()`. Task 126.2 (v0.13.0) implemented the spec's
  file-safety rules; see the security note under "Transmission media" above.

---

## Keyboard protocol compliance

Reference for Task 101. Task 35 shipped the protocol; this task closes the
remaining compliance gaps against the current spec. freminal's known gaps: it
encodes only 3 of 8 modifier bits and a subset of functional keys.

### CSI u encoding

```text
CSI <key-code>[:<shifted>[:<base-layout>]] ; <modifiers>[:<event-type>] ; <text-codepoints> u
```

- `CSI` = `0x1B 0x5B`; terminator `u` = `0x75`. All params decimal.
- Only `key-code` is mandatory. `modifiers` default `1`; `event-type` default `1`
  (press).
- `key-code` is the **un-shifted** codepoint (ctrl+shift+a ⇒ `CSI 97;… u`, never
  `65`).
- Alternate keys (flag 4): `shifted` is the shifted codepoint in the active
  layout (present only if shift is in modifiers); `base-layout` is the codepoint
  of the physical key on standard PC-101 US. One alternate ⇒ it is `shifted`.
  Base-only ⇒ empty shifted sub-field: `CSI code::base u`.
- Some functional keys use the legacy-compatible forms `CSI number ; mods ~` and
  `CSI 1 ; mods {ABCDEFHPQS}` (the `1` is omitted when there are no modifiers).

### Modifier bitmask (encoded as `1 + bits`)

| Modifier  | Bit value |
| --------- | --------- |
| shift     | 1         |
| alt       | 2         |
| ctrl      | 4         |
| super     | 8         |
| hyper     | 16        |
| meta      | 32        |
| caps_lock | 64        |
| num_lock  | 128       |

The wire value is `1 + bitmask` (no modifiers ⇒ `1`; ctrl+shift ⇒ `1 + 5 = 6`).
Modifier-key press events set their own bit. Lock modifiers are **not** reported
for text-producing keys under flag 1 alone; they are under flag 8.

**freminal gap:** only shift/alt/ctrl are modelled — super/hyper/meta/caps_lock/
num_lock (bits 8–128) are missing.

### Event types (flag 2)

`:1` press (default, omitted), `:2` repeat, `:3` release, as a sub-field of
modifiers. Enter/Tab/Backspace have no release events unless flag 8 is set.

### Progressive-enhancement flags

| Bit | Name                            | Effect                                                                                                                         |
| --- | ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| 1   | Disambiguate escape codes       | Esc, alt+key, ctrl+key, ctrl+alt+key, shift+alt+key, and non-text keypad keys become `CSI u`. Enter/Tab/Backspace stay legacy. |
| 2   | Report event types              | Emit repeat/release events (see above).                                                                                        |
| 4   | Report alternate keys           | Add `shifted:base-layout` sub-fields (only to keys already sent as `CSI u`).                                                   |
| 8   | Report all keys as escape codes | Text-producing keys stop emitting text and emit `CSI u`; modifier keys become reportable; implies disambiguation for all keys. |
| 16  | Report associated text          | Append text codepoints as a 3rd field. Undefined without flag 8.                                                               |

Set flags: `CSI = <flags> ; <mode> u` (mode `1` replace, `2` OR, `3` AND-NOT;
default `1`). Stack: `CSI > <flags> u` push, `CSI < <n> u` pop (default 1).
Separate stacks for main and alternate screens are required.

### Detection handshake

```text
app→term:  CSI ? u    then    CSI c   (DA1)
term→app:  CSI ? <flags> u    then    CSI ? ... c
```

If only the DA1 response arrives, the protocol is unsupported. `CSI ? u` alone
replies `CSI ? <flags> u` with the current top-of-stack flags.

### Associated text (flag 16)

Third `;`-separated field: colon-separated decimal codepoints of the text. Key
code `0` for pure text events with no known key. No control codes (< U+0020, C1)
in the text. Omitted when there is no text.

### Functional key codes

Most functional keys use Private Use Area codepoints (57344–63743); a handful use
sub-32/127 legacy numbers. Selected values freminal must confirm/emit:

| Key(s)                                  | Encoding                                                |
| --------------------------------------- | ------------------------------------------------------- |
| Escape / Enter / Tab / Backspace        | `27 u` / `13 u` / `9 u` / `127 u`                       |
| Insert / Delete                         | `2 ~` / `3 ~`                                           |
| Arrows Left/Right/Up/Down               | `1 D` / `1 C` / `1 A` / `1 B`                           |
| PageUp / PageDown                       | `5 ~` / `6 ~`                                           |
| Home / End                              | `1 H` or `7 ~` / `1 F` or `8 ~`                         |
| CapsLock/ScrollLock/NumLock             | `57358 u` / `57359 u` / `57360 u`                       |
| PrintScreen / Pause / Menu              | `57361 u` / `57362 u` / `57363 u`                       |
| F1–F4                                   | `1 P` / `1 Q` / `13 ~` / `1 S`                          |
| F5–F12                                  | `15 ~` `17 ~` `18 ~` `19 ~` `20 ~` `21 ~` `23 ~` `24 ~` |
| F13–F35                                 | `57376 u` … `57398 u`                                   |
| Keypad KP_0–KP_9                        | `57399 u` … `57408 u`                                   |
| KP_Decimal/Divide/Multiply/Subtract/Add | `57409`–`57413 u`                                       |
| KP_Enter/Equal/Separator                | `57414 u` / `57415 u` / `57416 u`                       |
| KP_Left/Right/Up/Down                   | `57417`–`57420 u`                                       |
| KP_PageUp/PageDown/Home/End             | `57421`–`57424 u`                                       |
| KP_Insert/Delete                        | `57425 u` / `57426 u`                                   |
| KP_Begin                                | `1 E` or `57427 ~`                                      |
| Media keys                              | `57428 u` … `57440 u`                                   |
| Left/Right modifier keys                | `57441 u` … `57452 u`                                   |
| ISO_Level3/5_Shift                      | `57453 u` / `57454 u`                                   |

F3 must be `13 ~` only (not `CSI R`, which collides with CPR). Modifier keys,
keypad keys, and media keys are reported as their own `CSI u` codes primarily
under flag 8.

### Legacy ctrl mapping (reference for flag behaviour)

`ctrl+a`..`ctrl+z` → 1..26. `ctrl+space`/`ctrl+2`/`ctrl+@` → 0. `ctrl+3`/`ctrl+[`
→ 27. `ctrl+4`/`ctrl+\` → 28. `ctrl+5`/`ctrl+]` → 29. `ctrl+6`/`ctrl+^`/`ctrl+~`
→ 30. `ctrl+7`/`ctrl+/`/`ctrl+_` → 31. `ctrl+8`/`ctrl+?` → 127. `ctrl+9`/`ctrl+i`
→ 9. `ctrl+0` → 48, `ctrl+1` → 49 (no transform). Any other ASCII key is left
untouched by ctrl.

### freminal current-state deltas: keyboard (from 101.1 audit, 2026-07-01; updated 2026-07-05 after Task 101; lock-state reverted 2026-07-06)

The 101.1 audit found the real blocker was **egui 0.35 (via egui-winit)**, not
freminal's encoding layer. Work split into "encoding-only" (Task 101, **done**)
and "egui-blocked" (Task 114 — a raw-winit intercept in `freminal-windowing`,
since egui itself was never upgraded). Task 114 delivered the keypad/media/
Print/Pause/Menu keys (kept, correct on every platform), but its **lock-state
half was reverted** (see below): `caps_lock`/`num_lock` decoration and the
CapsLock/NumLock/ScrollLock transition events cannot be produced uniformly
across platforms. Remaining gaps: lock-state (reverted), ISO_Level3/5_Shift
(no winit `KeyCode`), and `hyper`/`meta` (no platform source).

- Modifiers: `KeyModifiers` models all 8 bits (110.0); `modifier_param()` returns
  `Option<u16>` (max 256).
  - **✅ Done (101.2):** `super` (bit 8) is sourced by tracking
    `Key::SuperLeft`/`SuperRight` press/release in the GUI loop (macOS routes
    `Modifiers::mac_cmd` to `super_key`, split out of `ctrl`).
  - **⏳ Gap (REVERTED 2026-07-06 — not producible uniformly):** true
    `caps_lock`/`num_lock` (bits 64/128) are **not** sourced. A Task 114 attempt
    (`evdev` on Linux, `GetKeyState` on Windows, `CGEventSourceFlagsState` on
    macOS) was reverted: on **Wayland** the compositor consumes the lock keys and
    sends only `wl_keyboard.modifiers` (winit delivers no `KeyboardInput`), so the
    state is not observable via events; on **Windows/macOS** the OS query is a
    level (current on/off) sampled only at cold-start/focus-gain, so decoration is
    stale mid-focus. A Linux-only `evdev` kernel LED watcher was rejected as
    fragile and platform-divergent. Half-shipping would over-advertise capability.
    The `KeyModifiers` fields stay `0`. Tracked upstream: egui#3653, egui#2041,
    winit#1426 (alacritty#7937 is the same limitation elsewhere).
  - **⏳ Gap (no platform source):** `hyper`/`meta` (16/32) remain unavailable on
    any current platform path. The `KeyModifiers` fields exist but stay `0`.
- Functional keys present: arrows, Home/End, Insert/Delete, PageUp/Down, F1–F35,
  Enter/Tab/Backspace/Escape, modifier-keys-as-keys. `KeyPad(u8)` carries only
  legacy bytes, not KKP codepoints.
  - **✅ Done (101.3):** F13–F35 (`CSI 57376 u`…`57398 u`, KKP path only) and
    modifier-keys-as-keys ShiftLeft/Right, ControlLeft/Right, AltLeft/Right,
    SuperLeft/Right (57441–57444 / 57447–57450, under flag 8; Hyper/Meta omitted —
    egui has no such `Key` variants).
  - **✅ Done (Task 114):** keypad operators/directional/KP_Enter/KP_Begin, media
    keys, and PrintScreen/Pause/Menu-as-keys are delivered via a raw-winit
    `KeyboardInput` intercept (`App::on_raw_key_event` in
    `freminal-windowing/src/lib.rs`, wired through `event_loop.rs` before
    egui-winit translation) and encoded to
    `TerminalInput::KittyFunctional { codepoint, mods }` on the existing
    `build_csi_u` path — the same encoding Task 101 uses, just fed from a
    different delivery seam. Numpad digits/decimal remain unified with
    main-row digits per egui#3653.
  - **⏳ Gap (REVERTED 2026-07-06):** CapsLock/ScrollLock/NumLock **as keys**
    (their `CSI 57358/57359/57360 u` transition events) are **not** delivered.
    The transition is not observable off X11 (Wayland compositor consumes the
    key; Windows/macOS expose a level, not an edge), so shipping it only on X11
    would make one platform behave differently. Declined; tracked upstream
    (egui#3653, winit#1426). The `KKP_*_CODEPOINT` constants stay defined in
    `freminal-terminal-emulator` (spec table) — only the GUI delivery is absent.
  - **⏳ Gap (no winit `KeyCode`, not egui):** ISO_Level3_Shift / ISO_Level5_Shift
    (57453/57454) are still not delivered. The 114.5 recon confirmed the
    blocker moved from egui to **winit itself**: winit 0.30.13's `KeyCode` enum
    has no variant for these keys at all; the closest concept is the logical
    `NamedKey::AltGraph`, which carries no physical-key identity to intercept.
    Permanent, unscheduled gap pending upstream winit support.
- **✅ Done (101.3):** F3 is normalized to `13 ~` under KKP (was `ESC O R` SS3,
  which collides with CPR). The legacy (non-KKP) path keeps `ESC O R` for xterm
  terminfo compatibility.
- **✅ Done (Task 131.3, fixes B15): one stack per screen.** The primary and
  alternate screens each keep their own mode stack (`ScreenScoped<KittyKeyboardStack>`),
  and the active screen's stack is the one `CSI > u`, `CSI < u`, `CSI = u` and
  `CSI ? u` act on. The alternate stack persists between alternate-screen
  sessions, as kitty's `alt_key_encoding_flags` does, instead of being replaced by
  a fresh one on entry. Before, a second `?1049h` moved the alternate stack over
  the parked main stack and the main stack was lost. RIS **and DECSTR** clear both
  stacks, as kitty's `do_screen_reset` does (DECSTR previously left them).
- Conformant, do not touch: stack set/push/pop, `CSI ? u` query, XTGETTCAP `u`
  (all tested). All 5 flag bits defined.
- Base-layout sub-field always equals the key codepoint (no physical-layout map).
- DA1 does not advertise kitty keyboard (correct — detection is via `CSI ? u`).

---

## Future-version protocols (stubs)

These are decomposed at their own version's activation, against the code as it
then exists. Durable pointers only.

### File transfer over TTY — OSC 5113 (v0.13.3, Task 102)

Stateful bidirectional transfer with a mandatory user-consent prompt; reuses the
reverse-write path. Spec: <https://sw.kovidgoyal.net/kitty/file-transfer-protocol/>.

### Multiple cursors — CSI (v0.13.2, Task 103)

Renderer-light: snapshot gains a cursor list. Spec:
<https://sw.kovidgoyal.net/kitty/multiple-cursors-protocol/>.

### Text sizing — OSC 66 (v0.13.3, Task 104)

Highest-risk rendering item (multicell blocks, fractional scaling). The earlier
claim that OSC 66 collides with Contour's ColorScheme notification was wrong:
Contour does not use OSC 66 (its notification is `CSI ? 996 n` / `?2031`), and
freminal's current OSC 66 handler is a vestigial no-op. Spec:
<https://sw.kovidgoyal.net/kitty/text-sizing-protocol/>.

### Drag and drop — OSC 72 (v0.13.4, Task 105)

Spec marked stable upstream on 2026-06-12 (kitty issue #9984 closed); still blocked
locally by winit 0.30's DnD API. Spec: <https://sw.kovidgoyal.net/kitty/dnd-protocol/>.
