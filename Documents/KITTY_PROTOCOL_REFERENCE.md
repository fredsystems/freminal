# Kitty Protocol Reference (freminal implementation notes)

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
  corresponding to kitty **~0.47.x**.
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

| Protocol                          | Version  | Task(s) | Status in this doc   |
| --------------------------------- | -------- | ------- | -------------------- |
| Desktop notifications (OSC 99)    | v0.11.0  | 99      | Filled in            |
| Graphics protocol completion      | v0.11.0  | 100     | Filled in            |
| Keyboard protocol compliance      | v0.11.0  | 101     | Filled in            |
| File transfer over TTY (OSC 5113) | v0.12.0  | 102     | Stub                 |
| Multiple cursors (CSI)            | v0.12.0  | 103     | Stub                 |
| Text sizing (OSC 66)              | v0.13.0  | 104     | Stub                 |
| Drag and drop (OSC 72)            | deferred | 105     | Stub (spec unstable) |

Colored/styled underlines and the base kitty keyboard/graphics subsets are
already shipped (Tasks 35, 13) and are covered here only where v0.11.0 extends
them.

---

## Desktop notifications (OSC 99)

Reference for Task 99. OSC 99 is the stateful sibling of OSC 9 / OSC 777
(Task 76): it adds notification identity, chunked payloads, activation/close/
alive reports written back to the application, buttons, icons, sounds, urgency,
occasion, auto-expiry, and a capability handshake.

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

Version notes upstream: `o` added 0.31.0; `c f t n s g` added 0.36.0; `u` added
0.35.0.

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
- Same `i=` after finalize updates the existing notification in place.
- A notification with neither title nor body is ignored. If it has body but no
  title, the body is used as the title.
- Identifiers (`i=`, `g=`): characters from `[a-zA-Z0-9_\-+.]` only. **Terminals
  must sanitize/reject ids before echoing them in reports** (injection guard).
  `i=0` is the reserved "no id" sentinel; apps must not use it as a real id.

### Reports written back to the application (reverse PTY path)

All reports go back through the reverse-write path (`Pane::pty_write_tx` /
`write_to_pty`), the same channel DSR/DA/OSC 52 responses use.

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

### Capability handshake (`p=?`)

```text
app→term:  ESC ] 99 ; i=<id> : p=? ; ST
term→app:  ESC ] 99 ; i=<id> : p=? ; key=value : key=value ST
```

The response's capability keys (after the second `;`, colon-separated) — freminal
must advertise **only what it actually implements** (truthful advertisement,
carried from Task 76):

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

### Icons, sounds, occasions, urgency

- Icon by name (`n=`, base64): required names any impl must resolve:
  `error`, `warn`/`warning`, `info`, `question`, `help`, `file-manager`,
  `system-monitor`, `text-editor`. App-id names use the `.desktop` stem (Linux)
  or bundle id (macOS).
- Icon by data (`p=icon`, `e=1`): PNG/JPEG/GIF. If both `n=` and `p=icon` given,
  a locally-found named icon wins; the transmitted image is the fallback.
- Icon cache (`g=`): cache transmitted data under this key for the session; later
  notifications can reuse it by sending `g=` alone.
- Standard sound names: `system`, `silent`, `error`, `warn`/`warning`, `info`,
  `question`. Others are implementation-defined (e.g. freedesktop names on Linux).
- Occasion `o=`: `always` (default), `unfocused` (source window lacks keyboard
  focus), `invisible` (unfocused and not visible, e.g. inactive tab).
- Urgency `u=`: `0` low, `1` normal, `2` critical.

### Encoding rules

- Escape-safe UTF-8: valid RFC 3629 UTF-8 with **no** C0 (U+0000–U+001F), DEL
  (U+007F), or C1 (U+0080–U+009F) codepoints (so no newlines/tabs/CR).
- Base64: RFC 4648 standard alphabet. When chunking base64, either chunk before
  encoding (≤2048 raw bytes/chunk, include padding) or after encoding (≤4096
  bytes/chunk, padding only on the last chunk; terminals handle either).

### freminal current-state deltas: notifications — implemented in v0.11.0 (Task 99)

OSC 99 routing landed across Tasks 99.1–99.8:

- Parser: `osc_notify_99.rs` scans `raw_params` (not the pre-split token
  vector) and splits on the **second** `;` only, so an escape-safe-UTF-8
  title/body containing a literal `;` parses correctly. Dispatch runs through
  the dedicated `OscTarget::Notify99` / `AnsiOscType::Notify99` variants
  (kept separate from the shared OSC 9/777 `Notify` variant).
- Chunk reassembly: `reassemble_osc99` accumulates multi-escape payloads
  (`pending_notifications` on `TerminalHandler`, cleared in `full_reset()`)
  before a fully-formed `Notification99Data` reaches the GUI.
- App→terminal control payloads (close/alive/`p=?` query) are split into
  `Osc99Control` / `Osc99ControlKind` rather than folded into the display
  path.
- Display: `NotificationRouter::route_osc99` (`freminal/src/gui/notifications.rs`)
  pushes the toast leg and/or a `notify-rust` desktop notification, honouring
  the `o=` occasion gate, urgency, sound, auto-expiry, buttons, and the `g=`
  icon-by-data cache (`icon_cache: HashMap<String, Vec<u8>>`).
- Reverse reports: activation, close, and alive reports are written back via
  the originating pane's `pty_write_tx` (Linux/BSD observes real
  activation/close through `wait_for_action`; macOS/Windows emit the
  `untracked` close form immediately, since no observable handle is available
  from a background thread there).
- Capability handshake: `p=?` is answered with `osc99_query_response`,
  truthfully advertising only what's implemented (see `OSC99_CAPABILITIES` in
  `notifications.rs`).
- Config: `[notifications] osc_99` (added in 110.0, wired through
  `ConfigPartial`/`apply_partial`) is enforced at the `route_osc99` drain site
  (Task 99.8) as a kill-switch alongside the master `enabled` gate.

**Known deferrals (not silently dropped — tracked):**

- **`osc_9`/`osc_777` are still not gated independently** (Task 99.10,
  scheduled as an independent Task 76 hygiene cleanup, not a v0.11.0
  blocker). Both currently collapse to the shared `NotificationKind::OscText`
  and are gated only by `enabled`/`routing_info`; the `osc_9`/`osc_777`
  config fields have no effect yet.
- **Alive-map pruning tradeoff:** an OS-observed close on Linux writes the
  close report directly from the notification thread but does not prune
  `live` (a `!Send` map on the GUI thread); the map is pruned only on an
  app-driven `p=close` control. A `p=alive` response may thus transiently
  over-report a user-dismissed notification — spec-tolerable for a
  best-effort poll.

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
| `f`  | Regular file — implemented via `read_kitty_file`; requires an absolute path, read via `std::fs::read`. Does **not** refuse symlinks, device/socket files, or restrict to specific directories (see freminal current-state note below).    |
| `t`  | Temp file — implemented via `read_kitty_file` with `delete_after: true`; same absolute-path-only check as `f`, deleted after read. Does **not** restrict to known temp dirs or require `tty-graphics-protocol` in the path.               |
| `s`  | Shared-memory object; read `S` bytes at offset `O`. POSIX: `shm_open`/`mmap`, then `shm_unlink`+close. Windows: `OpenFileMappingW`/`MapViewOfFile`, then unmap+close (no unlink; `S=` required). Payload = object name. Both implemented. |

Security (upstream spec): refuse device/socket/special files; may refuse
`/proc`, `/sys`, `/dev`. **freminal current state:** only the absolute-path
check is enforced for `f`/`t` — see the graphics current-state section below.

### Responses and error codes

```text
ESC _ G i=<id>[,p=<placement>] ; <OK-or-ERROR[:detail]> ESC \
```

- `p=` is included in the response **only if** the request specified a non-zero
  `p=`. (freminal's `format_kitty_response` emits `p=<pid>` for non-zero
  placements as of Task 100.)
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
- `format_kitty_response(image_id, ok, message)` and `send_kitty_error` now
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

**Confirmed still remaining (not resolved by Task 100):**

- **`t=f`/`t=t` file-path security is narrower than the upstream spec
  suggests.** `read_kitty_file` only rejects non-absolute paths
  (`path.is_absolute()`); it does not follow-vs-refuse symlinks, does not
  refuse device/socket/special files, and does not restrict temp-file reads to
  known temp directories or require `tty-graphics-protocol` in the path. The
  transmission-media table above has been corrected to describe this actual
  behavior rather than the previously-documented (and inaccurate) protections.

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

### freminal current-state deltas: keyboard (from 101.1 audit, 2026-07-01; updated 2026-07-05 after Task 101 and Task 114)

The 101.1 audit found the real blocker was **egui 0.35 (via egui-winit)**, not
freminal's encoding layer. Work split into "encoding-only" (Task 101, **done**)
and "egui-blocked" (Task 114, **done** — a raw-winit intercept in
`freminal-windowing`, since egui itself was never upgraded). Two items remain,
and neither is egui-blocked any more: ISO_Level3/5_Shift is blocked on
**winit** (no `KeyCode` variant), and `hyper`/`meta` remain unsourced on any
platform.

- Modifiers: `KeyModifiers` models all 8 bits (110.0); `modifier_param()` returns
  `Option<u16>` (max 256).
  - **✅ Done (101.2):** `super` (bit 8) is sourced by tracking
    `Key::SuperLeft`/`SuperRight` press/release in the GUI loop (macOS routes
    `Modifiers::mac_cmd` to `super_key`, split out of `ctrl`).
  - **✅ Done (Task 114):** true `caps_lock`/`num_lock` (bits 64/128) are sourced
    from an OS lock-state query — `evdev`/`EVIOCGLED` kernel LED read on Linux
    (one code path for X11 and Wayland; Wayland has no client-side lock-state
    query by protocol design, so the kernel LED read sidesteps the display
    server entirely), `GetKeyState(VK_CAPITAL/VK_NUMLOCK)` on Windows, and
    `CGEventSourceFlagsState`/`kCGEventFlagMaskAlphaShift` on macOS (caps only;
    num/scroll hardcoded `false` — the concept doesn't exist on Mac keyboards;
    the Input-Monitoring/TCC permission question is unverified on-device). The
    query is ambient (ran at cold-start + `WindowFocused(true)`, never emits a
    synthetic event) and additionally toggled on an observed CapsLock/NumLock
    key transition while focused.
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
    keys, and CapsLock/ScrollLock/NumLock/PrintScreen/Pause/Menu-as-keys are now
    delivered via a raw-winit `KeyboardInput` intercept
    (`App::on_raw_key_event` in `freminal-windowing/src/lib.rs`, wired through
    `event_loop.rs` before egui-winit translation) and encoded to
    `TerminalInput::KittyFunctional { codepoint, mods }` on the existing
    `build_csi_u` path — the same encoding Task 101 uses, just fed from a
    different delivery seam. Numpad digits/decimal remain unified with
    main-row digits per egui#3653.
  - **⏳ Gap (no winit `KeyCode`, not egui):** ISO_Level3_Shift / ISO_Level5_Shift
    (57453/57454) are still not delivered. The 114.5 recon confirmed the
    blocker moved from egui to **winit itself**: winit 0.30.13's `KeyCode` enum
    has no variant for these keys at all; the closest concept is the logical
    `NamedKey::AltGraph`, which carries no physical-key identity to intercept.
    Permanent, unscheduled gap pending upstream winit support.
- **✅ Done (101.3):** F3 is normalized to `13 ~` under KKP (was `ESC O R` SS3,
  which collides with CPR). The legacy (non-KKP) path keeps `ESC O R` for xterm
  terminfo compatibility.
- Conformant, do not touch: stack set/push/pop, `CSI ? u` query, XTGETTCAP `u`,
  separate main/alt-screen stacks (all tested). All 5 flag bits defined.
- Base-layout sub-field always equals the key codepoint (no physical-layout map).
- DA1 does not advertise kitty keyboard (correct — detection is via `CSI ? u`).

---

## Future-version protocols (stubs)

These are decomposed at their own version's activation, against the code as it
then exists. Durable pointers only.

### File transfer over TTY — OSC 5113 (v0.12.0, Task 102)

Stateful bidirectional transfer with a mandatory user-consent prompt; reuses the
reverse-write path. Spec: <https://sw.kovidgoyal.net/kitty/file-transfer-protocol/>.

### Multiple cursors — CSI (v0.12.0, Task 103)

Renderer-light: snapshot gains a cursor list. Spec:
<https://sw.kovidgoyal.net/kitty/multiple-cursors-protocol/>.

### Text sizing — OSC 66 (v0.13.0, Task 104)

Highest-risk rendering item (multicell blocks, fractional scaling). Mandatory
first subtask: resolve the OSC 66 kitty-vs-Contour collision (freminal currently
treats OSC 66 as the Contour ColorScheme notification). Spec:
<https://sw.kovidgoyal.net/kitty/text-sizing-protocol/>.

### Drag and drop — OSC 72 (deferred, Task 105)

Spec under active upstream development (kitty 0.47, issue #9984); do not decompose
against a moving target. Spec: <https://sw.kovidgoyal.net/kitty/dnd-protocol/>.
