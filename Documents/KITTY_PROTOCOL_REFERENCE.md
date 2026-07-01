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
- Distilled from upstream as of **2026-07-01**, corresponding to kitty
  **~0.47.x**.
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

### freminal current-state deltas: notifications (from 99 seam audit, 2026-07-01)

- No OSC 99 routing yet: `OscTarget` has `Notify9`/`Notify777` but no `Notify99`;
  OSC 99 currently falls through to `Unknown` and is dropped.
- **Parser must use `raw_params`, not the pre-split token vector.** The OSC
  splitter naively splits on every `;`; an escape-safe-UTF-8 title/body may
  contain a literal `;` (0x3B is a legal payload byte). `handle_osc_notify_99`
  must scan `raw_params` and split on the **second** `;` only — the same reason
  `handle_osc_notify_9/_777` already parse from `raw_params`.
- `AnsiOscType::Notify { title, body }` is the shared OSC 9/777 output; OSC 99
  needs a **new** variant (id, payload-type, done, base64, actions, close, urgency,
  occasion, sound, app-name, icon-cache-key, icon-names, type, expiry, payload
  bytes) — this is the parser→handler transport.
- `WindowManipulation` gets a **new** `Notification99 { … }` variant (110.0), NOT
  an extension of the existing `Notification` variant (that would break the OSC
  9/777 call sites). Transport is the `WindowCommand` channel
  (`window_commands` → `handle_window_manipulation`), not the snapshot.
- Chunk reassembly: add `pending_notifications: HashMap<String, PendingNotification>`
  at the **end** of `TerminalHandler` (after the KKP stack fields, to minimize
  collision with Task 101) + `clear()` in `full_reset()`.
- `notify-rust` 4.18.0 supports actions/buttons, urgency, and `wait_for_action`
  (Linux/Windows; macOS legacy backend limited → use the `untracked` close form).
  `wait_for_action` **blocks** its thread until dismissed/activated — acceptable on
  the already-spawned notification thread. Icon-by-data (`p=icon`) needs the
  `images_no_default_features` feature or a temp-file + `image_path()`.
- Reverse-write for reports needs the **originating pane's** `pty_write_tx`, but
  the current drain drops pane identity before routing. Fix: carry
  `pane_id`/`pty_write_tx` on the notification request, or reuse the
  `HashMap<PaneId, Sender<PtyWrite>>` that broadcast-input (Task 74) already builds.
- **`osc_9`/`osc_777` config fields are declared but never read** (the `route()`
  call site ignores them). 99.8 adds `osc_99` wired end-to-end AND retroactively
  enforces `osc_9`/`osc_777` at the drain site (do not repeat the silent-drop) —
  per `freminal-config-options`.

---

## Graphics protocol completion

Reference for Task 100. Task 13 shipped transmit/put/delete/query and unicode
placeholders; the parser types most control keys (but **not** the relative-
placement keys `P`/`Q`/`H`/`V` — see below). Remaining: animation, storage
quotas, compression (`o=z`), shared memory (`t=s`), source-rect crop, relative
placements (incl. adding their parser keys), and `p=` in responses.

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

### Relative placements (in scope — Task 100.4)

Relative placements are part of the **graphics protocol proper**, transmitted in
the same `ESC _ G ... ESC \` APC envelope with `a=p` — they are _not_ a separate
CSI extension. **Correction (100.1 audit, 2026-07-01):** contrary to an earlier
recon note, `P`/`Q`/`H`/`V` are **not** currently typed in `KittyControlData` —
they hit the `_ => {}` wildcard in `apply_control_pair` and are silently dropped.
Task 100.4 therefore requires adding the 4 fields + 4 parser arms in
`freminal-common` (folded into foundation subtask 110.0) **before** the
handler/store work.

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

| `t=` | Medium                                                                                                                                |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `d`  | Direct — data in the escape payload (already implemented).                                                                            |
| `f`  | Regular file (implemented; follows symlinks, refuses special files).                                                                  |
| `t`  | Temp file — deleted after read; only in known temp dirs and path contains `tty-graphics-protocol` (implemented).                      |
| `s`  | POSIX/Windows shared-memory object; read `S` bytes at offset `O`, then unlink+close (POSIX) / close (Windows). Payload = object name. |

Security: refuse device/socket/special files; may refuse `/proc`, `/sys`, `/dev`.

### Responses and error codes

```text
ESC _ G i=<id>[,p=<placement>] ; <OK-or-ERROR[:detail]> ESC \
```

- `p=` is included in the response **only if** the request specified a non-zero
  `p=`. (freminal's `format_kitty_response` currently emits `i=` only — a gap to
  close in Task 100.)
- `q=1` suppresses OK; `q=2` suppresses all. Message is printable ASCII.

Named error codes: `ENOENT`, `EINVAL`, `ENOTSUP`, `ETOODEEP`, `ECYCLE`,
`ENOPARENT`, `ENOSPC`. Error form is `CODE:detail`; success is bare `OK`.

### Compression (`o=z`)

`o=z` marks the payload as RFC 1950 zlib-deflated (before base64). Decompress
before interpreting pixels/PNG. Valid for any `f=`. With PNG + compression,
provide `S=` (source data size). freminal parses `o=z` but does not yet
decompress — a gap in Task 100.

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

### freminal current-state deltas: graphics (from 100.1 audit, 2026-07-01)

- Parser types most control keys, **but NOT `P`/`Q`/`H`/`V`** (relative
  placements) — those hit the `_ => {}` wildcard in `apply_control_pair` and are
  silently dropped. 100.4 must add 4 fields + 4 parser arms (via 110.0).
- `a=f`/`a=a`/`a=c` are one warn-and-skip arm; `InlineImage` has no frame concept,
  and there is no image-animation tick anywhere (the only animation infra is the
  unrelated cursor-trail in `view_state.rs`). Animation needs a frame model on
  `InlineImage` + a GUI-side wall-clock frame selector.
- The parser stores animation keys under transmit/display-named fields (`s`→
  `src_width`, `v`→`src_height`, `c`→`display_cols`, `r`→`display_rows`, `z`→
  `z_index`, `X`→`cell_x_offset`, `Y`→`cell_y_offset`), so the handler must
  re-interpret them per action (the key-aliasing table in this doc).
- `o=z` parsed but never decompressed; no zlib crate in the workspace
  (`flate2`/`miniz_oxide` must be added). `t=s` returns `ENOTSUP`; the `nix` dep
  needs its `"mman"` feature for POSIX `shm_open`/`shm_unlink` (Windows: `winapi`).
- Source-rect crop (`x/y/w/h`) and cell offsets (`X/Y`) parsed but not applied.
  The renderer UV logic (`compute_image_quad`) is cell-grid based (min/max
  col/row_in_image), not pixel based — sub-cell `X/Y` offsets need new geometry.
- `format_kitty_response(image_id, ok, message)` omits `p=`; it needs a
  `placement_id: Option<u32>` param, emitting `,p=<pid>` when the request had a
  non-zero placement id. Same for the `send_kitty_error` helper. 5 call sites.
- Renderer sorts image quads by id, not z-index (a real stacking bug).
- No storage quota; only scrollback-driven `retain_referenced` GC. Byte size is
  `InlineImage.pixels.len()`; a quota check + LRU (prefer placement-less) hooks in
  `ImageStore::insert`.
- Delete gaps: `d=a` vs `d=A` (both over-delete the store), `d=i` vs `d=I` (both
  remove image data; lowercase should keep it), `d=x/X` `d=y/Y` `d=z/Z` (uppercase
  "and-after" collapses to non-after). Missing enum variants entirely: `d=f`/`d=F`
  (delete frames) and `d=r`/`d=R` (delete id-range, kitty 0.33.0).
- **Correction to an earlier recon note:** an early sub-agent summary claimed
  relative placements were "a separate CSI extension, out of scope". That is wrong
  — relative placements are APC graphics commands (`a=p` + `P/Q/H/V`) and are in
  scope as Task 100.4.

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

### freminal current-state deltas: keyboard (from 101.1 audit, 2026-07-01)

The audit found the real blocker is **egui 0.35 (via egui-winit)**, not freminal's
encoding layer. Work splits into "encoding-only" (doable in Task 101) and
"egui-blocked" (a separate windowing-layer task — see the roadmap).

- Modifiers: `KeyModifiers` models only shift/alt/ctrl (bits 1/2/4);
  `modifier_param()` returns 1+shift+alt·2+ctrl·4. `egui_mods_to_key_modifiers`
  drops everything else (egui's `Modifiers` has no super/hyper/meta/caps/num).
  - **Encoding-only (101.2):** add the 5 fields + arithmetic; source `super` (bit 8) by tracking `Key::SuperLeft`/`SuperRight` press/release in the GUI loop.
    `modifier_param` return type must widen past `u8` (max is 1+255=256).
  - **egui-blocked:** true `caps_lock`/`num_lock` (bits 64/128) — no egui API;
    needs raw winit `ModifiersChanged`. `hyper`/`meta` (16/32) unavailable on any
    current platform path.
- Functional keys present: arrows, Home/End, Insert/Delete, PageUp/Down, F1–F12,
  Enter/Tab/Backspace/Escape. `KeyPad(u8)` carries only legacy bytes, not KKP
  codepoints.
  - **Encoding-only (101.3):** F13–F35 (`FunctionKey(u8)` silently drops n>12; add
    57376–57398), and modifier-keys-as-keys ShiftLeft/Right, ControlLeft/Right,
    AltLeft/Right, SuperLeft/Right (57441–57452; egui _does_ deliver these as
    `Key::*Left/*Right`, but the event loop has no arm for them) — under flag 8.
  - **egui-blocked (new task):** keypad operators/directional/KP_Enter/KP_Begin,
    media keys, ISO_Level3/5_Shift, CapsLock/ScrollLock/NumLock/PrintScreen/Pause/
    Menu — **absent from egui 0.35's `Key` enum entirely** (numpad digits are also
    unified with main-row digits, egui#3653). Needs a raw-winit intercept in
    `freminal-windowing` or an egui/egui-winit upgrade.
- F3: currently `ESC O R` (SS3), which is neither the prohibited `CSI R` nor the
  spec's `13 ~`. 101.3 should confirm/normalize to `13 ~` under KKP.
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
