# PLAN_VERSION_130_3.md — v0.13.3 "New Protocols: Text Sizing, Transfer & Clipboard"

Part of v0.13.x "Kitty: Full Protocol Compliance". The compliance audit, common-work
analysis, reference specs and cross-milestone design decisions live in the overview,
`PLAN_VERSION_130.md`. This document holds the v0.13.3 tasks only.

**Status:** Planned.

The shared consent prompt and the three large new protocols that need it or the width work:
text sizing (OSC 66), file transfer (OSC 5113) and the kitty clipboard (OSC 5522). Every task
is an enriched stub, decomposed at activation.

## Tasks

| #   | Task                           | Milestone | Scope | Status  | Depends on    |
| --- | ------------------------------ | --------- | ----- | ------- | ------------- |
| 133 | Shared Consent Prompt          | v0.13.3   | M     | Planned | None          |
| 104 | Kitty Text Sizing (OSC 66)     | v0.13.3   | XL    | Planned | 129, 134, 103 |
| 102 | Kitty File Transfer (OSC 5113) | v0.13.3   | XL    | Planned | 129, 130, 133 |
| 146 | Kitty Clipboard (OSC 5522)     | v0.13.3   | XL    | Planned | 129, 130, 133 |

---

## Task 133 — Shared Consent Prompt (stub, v0.13.3)

**Goal.** One consent-overlay component, used by file transfer (102), clipboard (146) and
drag and drop (105). Without it, each of those would build its own modal.

**Audit findings.**

- Three separate modal guards exist (`paste_guard.rs`, `close_guard.rs`,
  `broadcast_guard.rs`), each with its own `ui_overlay_open` registration.
- The dialogs are per window, but a consent request can come from a background tab or
  another window.
- Registration must cover both `ui_overlay_open` (`app_impl.rs:~2275-2281`) **and**
  `sample_dismissible_presence` / `chrome_damage::DismissiblePresence`
  (`app_impl.rs:~4325-4337`).

**Scope sketch.**

- A queue of typed consent requests, captured with (tab_id, pane_id) in the same way as
  `PasteTarget`.
- One overlay that follows `freminal-modal-input-suppression`.
- The decision travels back over a typed `InputEvent`, so the PTY-side state machine stays
  authoritative.
- Toast categories follow `freminal-toast-options`.

## Task 104 — Kitty Text Sizing (OSC 66) (stub, v0.13.3; previous breakdown superseded)

**Goal.** Implement `OSC 66 ; metadata ; text ST`.

- Integer scale: `s=1..7` occupies `s*w × s` cells.
- Explicit width: `w=0..7`.
- Fractional scale: `n/d`, aligned by `v` (top/bottom/centre) and `h` (left/right/centre).
- Payload: escape-safe UTF-8, at most 4096 bytes.
- Rules: 4 overwrite rules, the wrap/discard rules (DECAWM on and off), and erasure rules
  for 7 editing controls (ICH, DCH, ECH, ED incl. 22, EL, IL, DL), plus IRM.
- The block should draw as a large cursor.

**Corrections to the previous breakdown.**

- **There is no Contour OSC 66.** Contour's colour-scheme notification is `CSI ? 996 n` /
  `CSI ? 997 ; Ps n` / `?2031`. The freminal `OscTarget::ColorSchemeNotification` (commit
  `9c053f4b`) is an unsupported attribution and a warn-logging no-op.
  - **Decision:** delete the variant and map OSC 66 solely to text sizing. Do not sniff the
    metadata shape. Keep a regression test that the legacy `66;dark` produces no cells. The
    old 104.1 audit subtask is therefore discharged by this activation.
- **Detection needs no terminal reply.** The client measures cursor movement with CPR, so
  there is no handshake subtask. A width-only stage would pass the `w=2` probe and fail the
  `s=2` probe, which is truthful.
- **Width and segmentation are Task 134**, a separate gate. Width cannot be a pure function
  of a string: explicit `w` wins over VS15/VS16, and SARA AM depends on the previous cell.
- **The renderer change is smaller than claimed.** Instance geometry is already `f32`. There
  is a per-row scaling precedent (`x_scale`, `RowGlyphParams` for DECDWL/DECDHL).
  `GlyphKey` already carries `size_px`.
  - The real cost is the **model**: multi-row blocks in the shaping run model, and snapshot
    transport, since continuation cells are skipped from `visible_chars` and `TChar` caps at
    16 bytes.

**Dependencies to add.**

- Task 125 (stable row numbers);
- Task 124 (damage: a block mutation is multi-row and must bump every covered row's epoch);
- Task 136 (cell side-slot precedent);
- Task 103 (shared cursor geometry for the large cursor).

**Corrected staging.**

1. Parse and route (with the doc fix).
2. Cell model.
3. Width-only milestone (`w≠0, s=1`): a ship gate.
4. Multi-row blocks and editing rules.
5. Snapshot plus damage.
6. Renderer, with mandatory benchmarks.
7. Docs.

**Open questions (the spec is silent; do not guess).**

- Block behaviour under scrolling, DECSTBM, DECLRMM, backspace, alternate-screen switch,
  RIS, and resize/reflow.
- DECDWL/DECDHL interplay.
- The over-4096-byte payload policy.
- REP exclusion (kitty does not update `last_graphic_char`).

## Task 102 — Kitty File Transfer (OSC 5113) (stub, v0.13.3; previous breakdown superseded)

**Goal.** Implement the full file-transfer protocol in both directions:

- files, directories (recursive), symlinks and hard links;
- `zip=zlib`;
- rsync deltas (`tt=rsync`) in both directions;
- `q=` quiet levels, `cancel`, `pw=` bypass;
- a mandatory consent prompt by default.

**Corrections to the previous breakdown.**

- **Wire keys are the short names:** `ac`, `zip`, `ft`, `tt`, `id`, `fid`, `pw`, `q`, `mod`,
  `prm`, `sz`, `n`, `st`, `pr`, `d`. Not `quiet=` / `compression=` / `bypass=`.
- **The terminal's own base64 replies must be unpadded**, because the stock kitten uses Go
  `RawStdEncoding`.
- **Integer sentinels.** Kitty uses `-1` for unset mtime, permissions and size.
- **There is no `finished` action.** The spec prose's `finished` is a typo for `finish`.
- **The send-direction consent prompt cannot show file names.** No metadata exists until the
  terminal replies `OK`. The receive direction can show paths, if they are buffered.
- **"Files-only v1" is dropped.** Directories and links are in the spec, and the goal is 1:1.
- **Security requirements derived from kitty's CVE fixes:**
  - `O_NOFOLLOW` on destination and signature opens;
  - unlink an existing symlink or multi-link destination before writing;
  - rsync only into plain regular files with `nlink == 1`;
  - a temp file beside the destination, then an atomic rename;
  - never allow bypass without a configured secret;
  - compare in constant time.
- **rsync wire details found only in kitty's `algorithm.c`:**
  - the `Hash` op is XXH128 over the whole output, as 16 canonical big-endian bytes;
  - the default block size is 6 KiB, or `round(sqrt(size))` when the size is known;
  - the weak hash is classic rsync `a + (b << 16)` mod 2^16;
  - the strong hash is XXH3-64, written little-endian.
- **Reference limits:**
  - 10-minute session idle expiry;
  - at most 10 concurrent sessions per direction;
  - commands without an id are ignored;
  - a duplicate `send` on an active id aborts it.
- **Replies use the handler path** (`write_osc_response`), not the GUI-side `pty_write_tx`
  idiom (Task 130).
- **Receive-direction data needs backpressure**, because the write channel is unbounded.
- **New dependencies are needed.**
  - XXH3: a direct `twox-hash` dependency with the `xxhash3_64`/`xxhash3_128` features.
  - SHA-256: no crate is present today.
  - A constant-time comparison crate (e.g. `subtle`).
  - `flate2` is already a dependency.

  Each new dependency follows `rust-best-practices` dependency hygiene.

**Open questions.**

- Bypass scheme:
  - `sha256:` only (spec-compliant, but the stock kitten never sends it);
  - or also kitty's `kitty-1` scheme (ECDH + AES-256-GCM, published via
    `KITTY_PUBLIC_KEY`).
- I/O thread model: a worker per session that emits structured replies for the handler to
  serialise.
- Whether RIS aborts in-flight transfers (kitty: no).

## Task 146 — Kitty Clipboard (OSC 5522) (stub, v0.13.3)

**Goal.** Implement OSC 5522:

- `type=read|write|wdata|walias`, `mime=`, `loc=primary`, `id=`, `pw=`/`name=`;
- strict padded base64;
- chunking at ≤4096 raw bytes;
- `DONE` and error statuses (`EINVAL`, `ENOSYS`, `EPERM`, `EBUSY`, `EFBIG` at ≥64 MB);
- listing (`.`) without a prompt;
- a permission prompt with session-scoped `pw`/`name` allow-lists.

**Audit findings.**

- Zero code exists.
- arboard 3.6 supports only text, image and HTML, with no arbitrary MIME types. Full 5522
  needs per-platform multi-MIME backends (X11/Wayland incl. primary, macOS, Windows). That is
  a dependency decision.

**Scope sketch.**

- The protocol and state machine (on Task 129).
- The multi-MIME backend.
- Consent via Task 133.
- OSC 52 hardening: selection honoured, empty-data clear, binary-safe, framed replies.

**Gate.** The paste-events mode `?5522` (unsolicited MIME list on paste, taking precedence
over 2004) is **TENTATIVE**: its ancillary spec is third-party and was revised 2026-05 to
2026-08. Implementing it needs explicit maintainer sign-off at activation. Until then
DECRQM `?5522` correctly reports 0 (unsupported).

**Open questions.**

- Which clipboard backend crate(s) to use.
- The config surface under `[security]` (prompt/allow/deny).
