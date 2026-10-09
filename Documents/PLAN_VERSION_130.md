# PLAN_VERSION_130.md — v0.13.x "Kitty: Full Protocol Compliance"

## Goal

Make freminal **1:1 compliant with every current, stable kitty terminal protocol
extension**, using <https://sw.kovidgoyal.net/kitty/protocol-extensions/> as the authoritative
source (kitty 0.49.2, 2026-10-01). That covers both the protocols freminal already claims to
support and the ones it does not yet implement.

The version was **re-activated and re-scoped on 2026-10-08**. The previous plan
("Transfer, Cursors & Text Sizing", Tasks 102–104) assumed the protocols freminal already
shipped were complete. A full code-grounded audit (summarised below) showed that the
assumption was wrong. Every shipped kitty protocol has real spec deviations, and six protocol
pages were not on the roadmap at all.

The first task the maintainer intends to land is the inactive-pane cursor change
(GitHub issue #531, Task 127). It is preceded by a small safety gate (Task 126) for
pre-existing security and panic bugs found by the audit.

**Phased point releases (maintainer decision, 2026-10-08).** All of the work lives in this
one plan document, grouped into milestones that ship as point releases:

| Milestone | Codename                                   | Tasks                        |
| --------- | ------------------------------------------ | ---------------------------- |
| v0.13.0   | Foundations, Safety & Small Conformance    | 126–132, 138, 139, 140       |
| v0.13.1   | Shipped-Protocol Conformance (heavy)       | 134, 135, 136, 137           |
| v0.13.2   | New Protocols: Small & Medium              | 103, 141, 142, 143, 144, 145 |
| v0.13.3   | New Protocols: Text Sizing, Transfer, Clip | 133, 104, 102, 146           |
| v0.13.4   | Drag & Drop                                | 105                          |

**Tiered decomposition.** Per `plan-decomposition`, only the next-up tasks carry a full
subtask breakdown: **126**, **127** and **128**. Everything else is an **enriched stub**:
goal, audit findings, durable decisions and open questions, with no subtasks. Tasks 102, 103
and 104 were previously decomposed. The audit found factual errors in those breakdowns (for
example, OSC 66 is not a Contour code, and the 102 wire keys were wrong), so they are now
enriched stubs again and get re-decomposed at activation.

---

## Task Summary

| #   | Task                                        | Milestone | Scope | Status        | Depends on                         |
| --- | ------------------------------------------- | --------- | ----- | ------------- | ---------------------------------- |
| 126 | Pre-existing Safety Gate                    | v0.13.0   | S     | Pending merge | None                               |
| 127 | Unfocused / Inactive-Pane Cursor (#531)     | v0.13.0   | M     | Planned       | 126 (schedule only)                |
| 128 | Prefix- & Intermediate-Aware CSI Dispatch   | v0.13.0   | M     | Planned       | 126.3                              |
| 129 | Kitty Wire Infrastructure                   | v0.13.0   | M     | Planned       | None                               |
| 130 | Reverse-Path & Capability-Query Consistency | v0.13.0   | M     | Planned       | 129                                |
| 131 | Screen-Scoped State & Reset Lifecycle       | v0.13.0   | M     | Planned       | None                               |
| 132 | Colour Foundation                           | v0.13.0   | L     | Planned       | 126.1                              |
| 138 | Desktop Notifications (OSC 99) Conformance  | v0.13.0   | L     | Planned       | 129, 130                           |
| 139 | Pointer Shapes (OSC 22) Completion          | v0.13.0   | M     | Planned       | 129, 130, 131                      |
| 140 | Underline & SGR Parity                      | v0.13.0   | S     | Planned       | None                               |
| 134 | Unicode Width & Segmentation Conformance    | v0.13.1   | XL    | Planned       | None                               |
| 135 | Graphics Protocol Conformance               | v0.13.1   | L     | Planned       | 126.2, 129, 130, 131               |
| 136 | Graphics Placement Model & Z-Layers         | v0.13.1   | XL    | Planned       | 135                                |
| 137 | Keyboard Protocol Conformance               | v0.13.1   | XL    | Planned       | 128, 131; architecture sign-off    |
| 103 | Multiple Cursors                            | v0.13.2   | L     | Planned       | 127, 128, 131, 132, 141            |
| 141 | Misc Protocol Extensions                    | v0.13.2   | M     | Planned       | 128, 131                           |
| 142 | Unscroll (`CSI Ps + T`)                     | v0.13.2   | M     | Planned       | 128                                |
| 143 | DECCARA / DECSACE                           | v0.13.2   | M     | Planned       | 128, 140                           |
| 144 | Color Control (OSC 21) & Colour Stack       | v0.13.2   | L     | Planned       | 128, 129, 130, 132                 |
| 145 | Kitty Shell-Integration Compatibility       | v0.13.2   | M     | Planned       | 129; maintainer decision           |
| 133 | Shared Consent Prompt                       | v0.13.3   | M     | Planned       | None                               |
| 104 | Kitty Text Sizing (OSC 66)                  | v0.13.3   | XL    | Planned       | 129, 134, 103                      |
| 102 | Kitty File Transfer (OSC 5113)              | v0.13.3   | XL    | Planned       | 129, 130, 133                      |
| 146 | Kitty Clipboard (OSC 5522)                  | v0.13.3   | XL    | Planned       | 129, 130, 133                      |
| 105 | Kitty Drag & Drop (OSC 72)                  | v0.13.4   | XL    | Planned       | 102, 133; windowing DnD capability |

**Numbering.** Task numbers here are **allocation order, not execution order**, following the
freminal convention recorded in `MASTER_PLAN.md`. Milestones carry execution order. Tasks
102–105 keep their historical numbers. New tasks were allocated from 126 upward. The
execution order inside each milestone is given under "Execution model" below.

---

## Reference specs

- Protocol index — <https://sw.kovidgoyal.net/kitty/protocol-extensions/>
- Underlines — <https://sw.kovidgoyal.net/kitty/underlines/>
- Graphics — <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
- Keyboard — <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>
- Text sizing — <https://sw.kovidgoyal.net/kitty/text-sizing-protocol/>
- Drag and drop — <https://sw.kovidgoyal.net/kitty/dnd-protocol/>
- Multiple cursors — <https://sw.kovidgoyal.net/kitty/multiple-cursors-protocol/>
- File transfer — <https://sw.kovidgoyal.net/kitty/file-transfer-protocol/>
- Desktop notifications — <https://sw.kovidgoyal.net/kitty/desktop-notifications/>
- Pointer shapes — <https://sw.kovidgoyal.net/kitty/pointer-shapes/>
- Unscroll — <https://sw.kovidgoyal.net/kitty/unscroll/>
- Color control — <https://sw.kovidgoyal.net/kitty/color-stack/>
- DECCARA — <https://sw.kovidgoyal.net/kitty/deccara/>
- Clipboard — <https://sw.kovidgoyal.net/kitty/clipboard/>
- Misc — <https://sw.kovidgoyal.net/kitty/misc-protocol/>
- Shell integration — <https://sw.kovidgoyal.net/kitty/shell-integration/>

Where the spec prose and kitty's reference implementation disagree, the task stubs below
record which one freminal follows. Kitty source (`kitty/*.c`, `kitty/*.py`,
`kittens/**/*.go`) is the tie-breaker only where the spec is silent or self-contradictory.
Each such choice is recorded as a design decision; none is made silently.

Every escape-sequence change triggers the dual-document update
(`ESCAPE_SEQUENCE_COVERAGE.md` + `ESCAPE_SEQUENCE_GAPS.md`) per
`freminal-escape-sequence-docs`. `KITTY_PROTOCOL_REFERENCE.md` is refreshed to the kitty
0.49.2 baseline by each protocol task as it lands.

---

## Compliance audit (2026-10-08)

Eight parallel read-only audits traced every spec requirement through parse → handler →
state → snapshot → render, then cross-checked ambiguous cases against kitty's source. The
findings below are grounded in the code at `7e0945a3`. Freminal's own docs were treated as
claims to verify, not as evidence.

### Stability verdicts

| Protocol                  | Stable?  | Evidence                                                                                                      |
| ------------------------- | -------- | ------------------------------------------------------------------------------------------------------------- |
| Underlines                | Yes      | Unchanged for years                                                                                           |
| Graphics                  | Yes      | No experimental language; features version-tagged 0.19.3–0.33                                                 |
| Keyboard                  | Yes      | Normative content stable since 0.20; last substantive fix 0.34                                                |
| Text sizing (escape code) | Yes      | Since 0.40.0                                                                                                  |
| Text sizing (width algo)  | Volatile | Upstream warning (#8533); 0.49.0 changed SARA AM and VS15 rules. Implement behind one replaceable module      |
| Drag and drop             | Yes      | Marked stable 2026-06-12 (commit `8be4ea67`, kitty #9984 closed); later edits are security fixes only         |
| Multiple cursors          | Yes      | "Under discussion" warning removed 2026-01-08 (`d6cb5c36`); no spec commits since                             |
| File transfer             | Yes      | Wire format frozen 2023; later changes are security fixes (`O_NOFOLLOW`, bypass-without-password)             |
| Desktop notifications     | Yes      | No spec change since 0.36                                                                                     |
| Pointer shapes            | Yes      | Since 0.31; no changes                                                                                        |
| Unscroll                  | Yes      | Since 0.20.2                                                                                                  |
| Color control             | Yes      | OSC 21 hardened (0.47.3, 0.49.0) with behaviour unchanged                                                     |
| DECCARA                   | Yes      | Stable                                                                                                        |
| Clipboard (OSC 5522)      | Yes      | Core stable; `EFBIG` added 0.49.0. **Paste-events mode `?5522` is still being revised; treated as TENTATIVE** |
| Misc protocol             | Yes      | Stable                                                                                                        |

### Claimed versus actual

| Protocol              | Freminal docs claimed                               | Actual (code)                                                                                                                                                                                                                                        | Task          |
| --------------------- | --------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------- |
| Underlines            | Complete                                            | Mostly compliant. SGR 21 means bold-off where kitty means double underline; `4:N>5` clears instead of clamping; underline geometry untested and unclamped; default colour wrong under SGR 7 + DECSCNM                                                | 140           |
| Graphics              | "Fully implemented", "graphics surface is complete" | ~30 deviations, 5 high. Replies to id-less commands; `t=t` deletes arbitrary files; text and erase destroy image tiles; placing an image pre-clears others below; no negative-z layering; cursor-after-placement rule wrong                          | 126, 135, 136 |
| Keyboard              | "Substantially compliant, remainder tracked"        | Encoder machinery sound, but the GUI layer feeding it is not. Alt/Super never applied to text keys; modified Enter dropped; Enter/Tab/BS/Esc carry no modifiers; key identity from typed text; flag-4 shifted-key rule violated; F13–F35 unreachable | 137           |
| Desktop notifications | Done, incl. reports and `p=?` handshake             | Buttons reported 0-based; chunk metadata clobbered by defaults; `a=report` disables focus; close report lost after activation; no update-in-place; `p=close` doesn't close; `p=?` answered after DA1                                                 | 138           |
| Pointer shapes        | Undocumented                                        | Plain set works. Push, pop and query (and `=`) all reset the shape to default instead; a query gets no reply; no stack                                                                                                                               | 139           |
| Clipboard (OSC 5522)  | Not mentioned                                       | Entirely missing; payloads would be warn-logged by the unknown-OSC path                                                                                                                                                                              | 146           |
| Color control         | Not mentioned                                       | OSC 21 and colour stack missing. OSC 10/11 "set" never reaches rendering. `CSI # P` deletes characters. `parse_color_spec` panics on non-ASCII input                                                                                                 | 126, 132, 144 |
| DECCARA / DECSACE     | Not mentioned                                       | Missing. `CSI … $ r` routes to DECSTBM; `CSI * x` writes an unsolicited DECREQTPARM reply                                                                                                                                                            | 128, 143      |
| Unscroll              | Not mentioned                                       | `CSI Ps + T` silently executes as SD                                                                                                                                                                                                                 | 128, 142      |
| Misc protocol         | Not mentioned                                       | XTSAVE/XTRESTORE, SGR 221/222, mouse-leave report and `CSI 22 J` all missing                                                                                                                                                                         | 140, 141      |
| Multiple cursors      | Planned (Task 103)                                  | Missing, and **colliding**: `CSI > 0;4 SP q` (clear all) currently makes freminal write an XTVERSION reply into the app's stdin                                                                                                                      | 126, 103      |
| Text sizing           | Planned; OSC 66 "collides with Contour ColorScheme" | No collision exists: Contour does not use OSC 66 (its colour-scheme notification is `CSI ? 996 n` / `?2031`). The OSC 66 handler is a warn-logging no-op that is not load-bearing                                                                    | 104           |
| File transfer         | Planned (Task 102)                                  | Missing; every chunk would be warn-logged; the plan's wire keys were wrong (`quiet=`, `compression=`, `bypass=` are really `q=`, `zip=`, `pw=`)                                                                                                      | 102           |
| Drag and drop         | Deferred (spec unstable)                            | Spec now stable; blocked locally by winit 0.30's DnD API                                                                                                                                                                                             | 105           |
| Shell integration     | Not assessed                                        | Kitty-format OSC 133 dropped by design (`freminal=1` gate); `kitty-shell-cwd://` OSC 7 rejected with a warn every prompt                                                                                                                             | 145           |

### Out of scope (with rationale)

| Surface                                                                        | Why out of scope                                                                                                  |
| ------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------- |
| Remote control protocol (`DCS @kitty-cmd{json}`)                               | Window-manager API for controlling kitty itself (launch, set-colors, screenshot), not an app-to-terminal protocol |
| Kitten plumbing DCS (`@kitty-print`, `echo`, `ssh`, `ask`, `clone`, `edit`, …) | Private kitten transport, undocumented as a protocol. Task 129 silences their warn-logging                        |
| XTGETTCAP kitty-private names (`kitty-query-*`)                                | Reveal kitty's own config; only meaningful for kitty                                                              |
| `OSC 1337;SetUserVar`                                                          | Not a documented kitty protocol (iTerm2 origin)                                                                   |
| OSC 3008                                                                       | Ignored by kitty too                                                                                              |
| Wide-gamut colours (`oklch()`, `lab()`)                                        | Config syntax only; no escape code. Optional in OSC 21 parsing (Task 132 open question)                           |
| DECRARA (`CSI … $ t`)                                                          | Standard DEC, but kitty does not implement it. Task 128 stops it from executing as a window op                    |
| Custom shaders, cursor trails, layouts, sessions                               | Kitty application features, not terminal protocols                                                                |

### Pre-existing bugs outside the kitty surface (recorded, routed)

The audit surfaced ordinary-terminal bugs. They are not dropped; each is routed to the task
that owns the seam:

- Invalid UTF-8 in a data chunk silently drops the **whole chunk**, including valid text, and
  a grapheme over 16 bytes does the same (`terminal_handler/mod.rs:802-804`,
  `tchar.rs:16,115`). → Task 134.
- Grapheme segmentation runs per data chunk, so a combining mark or VS16 arriving in a later
  read starts its own cell. → Task 134.
- With DECAWM off, freminal discards all remaining text instead of overwriting the last column
  (`buffer/mod.rs:879-888`). → Task 134 (decision recorded there).
- `AnsiOscParser::push` strips trailing `\` bytes from legitimate payloads
  (`osc.rs:108-116`). → Task 129.
- OSC 8 URLs containing `;` end the hyperlink instead of parsing it. → Task 129.
- `CSI 14 t` reports the OS window's outer rectangle for every pane. → Task 135.
- Erase operations copy the whole current format tag into blank cells, not just the
  background (needs verification against xterm/kitty first). → Task 140.
- OSC 52 ignores the selection parameter, cannot clear, and corrupts binary data. → Task 146.

---

## Common-work analysis

The audit found the same missing seams in protocol after protocol. Building them once, first,
removes duplicated work from every later task and fixes several live bugs on the way. Seven
foundation tasks result. Each one has at least three consumers.

| Foundation                                | What it provides                                                                                                                                                                                     | Consumers                                   |
| ----------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------- |
| **128** CSI dispatch                      | Route on (private prefix, intermediate, final). Fixes 6+ live misroutes                                                                                                                              | 103, 137, 141, 142, 143, 144                |
| **129** Wire infrastructure               | One bounded `key=value` metadata tokenizer; strict, streaming and unpadded base64; a bounded chunk accumulator; parser size caps; raw-params OSC dispatch; payload-safe logging; APC response helper | 102, 104, 105, 135, 138, 139, 144, 145, 146 |
| **130** Reverse path & capability queries | GUI-originated replies go through handler framing (S8C1T, tmux wrap); capability queries answered on the PTY thread so they precede DA1; advertised capabilities match reality                       | 102, 135, 138, 139, 144, 146, 105           |
| **131** Screen-scoped state & resets      | One registry for per-screen state (alt-screen enter/leave idempotent) and an explicit reset table (RIS vs DECSTR) per protocol                                                                       | 103, 135, 136, 137, 139, 141                |
| **132** Colour foundation                 | Spec-complete colour-spec parser; a reusable SGR-style colour extractor; dynamic colours that actually render                                                                                        | 103, 140, 144, 136 (placeholder ids)        |
| **133** Shared consent prompt             | One consent overlay component, registered once with modal-input suppression and dismissible-presence                                                                                                 | 102, 146, 105                               |
| **134** Width & segmentation              | Kitty-conformant cell splitting, with width stored on the cell as the single authority                                                                                                               | 104, ordinary text, 136 (placeholders)      |

Two seams were considered and **not** made separate tasks:

- **Snapshot-shape coordination** (cursor list, placement list, text-sizing block list). This
  is a design rule, recorded under "Design decisions", not a task. Each consumer adds its own
  field in the sparse-list shape.
- **A typed protocol error-code module.** Each protocol's error set differs (graphics, OSC 99
  and OSC 5113 do not share codes). A typed per-protocol error enum is required by the repo
  rules, and each protocol task owns its own.

---

## Execution model

- **Task 126 first, alone.** It is small, security-relevant and touches three unrelated files.
- **Then Tasks 127 and 128 in parallel**, in separate worktrees per `parallel-work-isolation`.
  127 lives entirely in the GUI crate (renderer, widget, config). 128 lives entirely in the
  parser (`ansi_components/`) and `terminal_handler/dcs.rs`. They are file-disjoint and
  behaviour-disjoint.
- **Then 129, 130, 131 and 132.** 130 depends on 129. 131 and 132 are independent of both and
  of each other.
- **Then 138, 139 and 140**, which close out v0.13.0.
- v0.13.1 onward is activated per milestone, against the code as it then exists. Activation
  re-reads the stub, resolves its open questions with the maintainer, and decomposes it.

Every implementation subtask leaves `cargo test --all` and
`cargo clippy --all-targets --all-features -- -D warnings` green. Every PR runs
`cargo machete` and `cargo xtask check-windows`.

---

## Task 126 — Pre-existing Safety Gate

### 126 Summary

The audit found three bugs that predate v0.13.0 and are dangerous enough to fix before
any feature work. Any program running in the terminal can trigger each one. Per the
maintainer's decision (2026-10-08), they land first, as small isolated fixes. 126.4 corrects
the coverage docs, which currently overstate support, so that later work starts from a
truthful baseline.

### 126 Subtasks

#### 126.1 — Make `parse_color_spec` panic-free on non-ASCII input

Scope: `freminal-common/src/colors.rs` (production code and its `#[cfg(test)]` module).

What: `parse_color_spec` slices `&hex[0..2]` by **byte offset** on attacker-controlled text.
`OSC 11;#aéaé ST` is 6 bytes long, so the slice cuts inside `é` and panics the PTY thread.
OSC 4, 10, 11 and 12 all reach it. Fix:

- In the `#` branch, return `None` unless `hex.bytes().all(|b| b.is_ascii_hexdigit())`.
- In the `rgb:` branch, apply the same check to each channel before calling
  `scale_hex_channel`.

The check runs before any slicing, so no slice can split a character.

Leave the `#RGB` expansion rule (`r * 17`) unchanged. Whether it should follow the spec's
"digits are the most significant bits" reading is Task 132's open question, not this
subtask's.

Deliverable:

- the fix;
- unit tests with non-ASCII inputs whose byte length is 3, 6 and 12, plus `rgb:` channels
  containing multi-byte characters, all returning `None`;
- a `proptest` (already a dev-dependency of `freminal-common`) asserting that
  `parse_color_spec` never panics for arbitrary `String` input.

Verification: `cargo test --all`; `cargo clippy --all-targets --all-features -- -D warnings`.

Prohibitions: do NOT change accepted formats or the expansion rule; do NOT touch OSC
dispatch; do NOT proceed to 126.2.

Stop: report files changed and verification results; await review.

**Status: Complete (2026-10-08).** ASCII-hex-digit check added ahead of all slicing in both
branches; unit tests for 3/6/12-byte non-ASCII `#` inputs and multi-byte `rgb:` channels;
three never-panics proptests. Side effect of the check: sign-prefixed channels such as
`#+a+a+a` and `rgb:+f/00/00`, which `from_str_radix` used to accept, now return `None`. No
documented format is affected.

#### 126.2 — Harden kitty graphics file transmission (`t=f`, `t=t`)

Scope:

- `freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs`: `read_kitty_file`,
  its call sites, and its tests;
- `freminal-terminal-emulator/Cargo.toml`: add the `"fs"` feature to the existing unix-only
  `nix` dependency.

What: `read_kitty_file` (`graphics_kitty.rs:~1061-1127`) has three security problems:

- it calls `std::fs::read` on any absolute path, so a FIFO or `/dev/zero` hangs the PTY
  thread or exhausts its memory;
- each failure produces a distinct error string, which leaks whether a file exists and
  whether it is readable;
- `t=t` deletes **any** readable path.

Implement the spec's file-safety rules:

1. Open the file read-only. On unix, pass `O_NONBLOCK` (via
   `std::os::unix::fs::OpenOptionsExt::custom_flags(nix::fcntl::OFlag::O_NONBLOCK.bits())`)
   so opening a FIFO cannot block. Then `fstat` the opened handle (`File::metadata`) and
   require `is_file()`. On Windows, open normally and require `metadata().is_file()`.
2. Canonicalise the path. Refuse any path under `/proc`, `/sys` or `/dev`. The refusal
   applies on every platform, because the check is a path-prefix test.
3. Read at most `MAX_KITTY_FILE_BYTES` bytes, a new module constant equal to
   `400 * 1024 * 1024` (kitty's `MAX_DATA_SZ`), using `Read::take`. If the cap is exceeded,
   fail.
4. Answer **every** failure in this function with the single response
   `EBADF:Failed to read image file`. This covers non-UTF-8 path, non-absolute path, open
   failure, not-a-regular-file, refused prefix, read error and over-cap. Keep the detailed
   reason in a `tracing::debug!` (not `warn!`) that does not include file contents.
5. For `t=t`, delete the file only when **both** of these hold:
   - its canonical path is inside a known temporary directory: `std::env::temp_dir()`
     canonicalised, `/tmp`, or `/dev/shm`;
   - its path string contains `tty-graphics-protocol`.

   Otherwise read the file but leave it in place.

**Maintainer decisions at execution (2026-10-08), amending rules 1, 2 and 5 above to match
kitty's reference (`kitty/utils.py` `is_ok_to_read_image_path` / `is_ok_to_read_image_file`):**

- **`/dev/shm` is exempt from the `/dev` refusal.** Paths under `/dev/shm/` may be read.
  Without this exemption rule 5's `/dev/shm` clause could never apply.
- **Check before opening.** Canonicalise the path and refuse a forbidden prefix _before_
  opening it, then open the canonical path (with `O_NONBLOCK` on unix) and require that the
  opened handle `is_file()`. Opening a device node can have side effects, and a failed open
  reveals whether the file exists.

Deliverable:

- the hardening;
- tests:
  - a FIFO path (unix only; create it with `nix::unistd::mkfifo` in a `tempfile` dir)
    returns `EBADF` without blocking;
  - a directory path returns `EBADF`;
  - a non-existent path returns `EBADF`;
  - a `/proc/self/status` path returns `EBADF`;
  - a `t=t` file named `…tty-graphics-protocol…` inside `temp_dir()` is deleted;
  - a `t=t` file without the marker is read but **not** deleted.
- Rewrite `kitty_temp_file_transmission_reads_and_deletes`, which currently enshrines the
  bug: its file name lacks the marker.

Verification: `cargo test --all`; clippy; `cargo xtask check-windows` (this touches
`#[cfg(unix)]` code and `nix` features).

Prohibitions:

- do NOT honour `S=`/`O=` for files (that is Task 135);
- do NOT change `t=s` or `t=d`;
- do NOT change responses for id-less commands (Task 135);
- do NOT proceed to 126.3.

Stop: report files changed and verification results; await review.

**Status: Complete (2026-10-08).** `load_kitty_file` validates the path (UTF-8, absolute),
canonicalises it and refuses `/proc`, `/sys` and `/dev` (except `/dev/shm`) before opening.
It then opens with `O_NONBLOCK`, requires that the opened handle `is_file()`, and reads
through `read_capped` (`MAX_KITTY_FILE_BYTES`). Every failure is answered with
`EBADF:Failed to read image file`, and the reason goes to a debug-level log via the private
`KittyFileError`. `t=t` deletion is gated by `may_delete_kitty_temp_file`. New tests cover
FIFO, directory, `/proc`, a component-wise prefix check, the cap boundary, and deletion with
and without the marker. `kitty_temp_file_transmission_reads_and_deletes` was rewritten. Three
legacy tests asserted the old per-failure codes (`EIO`, `EPERM`, `EINVAL`) and now expect the
uniform `EBADF`. `check-windows` is clean.

**PR #534 review follow-up (2026-10-08).** Copilot and CodeRabbit found two pathname races,
and both are now closed on unix:

- **Open race.** Files are opened `O_NOFOLLOW`. After opening, the path is re-resolved,
  vetted again, and its `(dev, ino)` must match the opened handle, as kitty's `samestat`
  check does.
- **`t=t` deletion race.** The entry is re-opened relative to a parent-directory handle and
  its identity checked against the file that was read. Only then is it removed, with
  `unlinkat`.

The suggestion to match the marker against the file name alone was declined. The spec says
"in its full file path", and kitty matches on the full path too.

#### 126.3 — Stop `CSI > … SP q` from triggering XTVERSION

Scope:

- `freminal-terminal-emulator/src/ansi_components/csi.rs` (the `Finished(b'q')` arm and its
  tests);
- `freminal-terminal-emulator/src/ansi_components/csi_commands/xtversion.rs` (and its
  tests).

What: the `q` arm (`csi.rs:365-370`) sends any `>`-prefixed sequence to XTVERSION and
ignores intermediates. `xtversion.rs` reads only `params[1]`. So `CSI > 0;4 SP q`, the kitty
multiple-cursors "clear all" command, writes an unsolicited XTVERSION DCS reply into the
application's stdin. Minimal fix, until Task 128 replaces the arm:

- **`>` prefix with any non-empty intermediates:** emit nothing. Log
  `tracing::warn!("Unhandled CSI final byte (valid grammar, no dispatch): {}", …)` using
  `format_raw_csi()`, matching the existing unknown-final log. Return `Finished`. The
  sequence is recognised and unimplemented until Task 103.
- **`>` prefix with no intermediate:** XTVERSION only when the params are exactly `>` or
  `>0`. Change `xtversion.rs` to validate the whole parameter string, not just
  `params[1]`; anything else returns the existing `UnhandledXTVERSIONCommand` failure.
- **No `>` prefix:** unchanged (DECSCUSR) in this subtask. The no-space `CSI Ps q` form is
  Task 128's decision.

Deliverable:

- the fix;
- tests:
  - `CSI > q` and `CSI > 0 q` yield `RequestDeviceNameAndVersion`;
  - `CSI > 0;4 SP q`, `CSI > SP q`, `CSI > 100 SP q` and `CSI > 1;2:3:4 SP q` yield no
    output;
  - `CSI > 0;4 q` (no space) is rejected by `xtversion`.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT implement multiple cursors; do NOT restructure the dispatcher (Task
128); do NOT proceed to 126.4.

Stop: report files changed and verification results; await review.

**Status: Complete (2026-10-08).** In the `q` arm, a `>` prefix with any intermediate now
warn-logs via `format_raw_csi()` and emits nothing. `xtversion.rs` accepts exactly `>` or `>0`
and rejects everything else, including `>0;4`, `>00` and `>1`. The no-prefix DECSCUSR path is
unchanged. New parser-level tests cover the two XTVERSION forms, the four kitty-style
`SP q` forms plus `>0;4$q` (no output), `>0;4q` (rejected) and `2 SP q` (still DECSCUSR).

#### 126.4 — Coverage-doc truth reset

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`,
`Documents/KITTY_PROTOCOL_REFERENCE.md`.

What: bring the docs in line with the 2026-10-08 audit, so they stop overstating support.
Change status markers only; this is not a rewrite.

- **Graphics APC `_G`, OSC 99 and kitty keyboard:** downgrade from ✅ to 🚧. Add a
  notes-column pointer to Tasks 135/136, 138 and 137 respectively.
- **Rows to add**, each pointing to its v0.13.x task:
  - OSC 22: 🚧, set/reset only;
  - OSC 5522: ❌;
  - OSC 21, OSC 30001/30101 and XTPUSHCOLORS/XTPOPCOLORS/XTREPORTCOLORS: ❌;
  - DECCARA/DECSACE: ❌;
  - unscroll `CSI + T`: ❌;
  - XTSAVE/XTRESTORE: ❌;
  - SGR 221/222: ❌;
  - `CSI 22 J`: ❌;
  - mouse-leave report: ❌;
  - multiple cursors: ❌;
  - OSC 5113: ❌;
  - OSC 72: ❌.
- **OSC 66:** correct the "Contour ColorScheme" description. Contour does not use OSC 66; the
  freminal handler is a vestigial no-op. Point to Task 104.
- **OSC 10/11:** note that "set" stores an override that does not render (Task 132).
- **GAPS:** replace the "fully implemented" and "substantially compliant" claims with
  pointers to the v0.13.x tasks.
- **`KITTY_PROTOCOL_REFERENCE.md`:** update the roadmap table to the new task numbers and
  milestones. Add a prominent note that the "current-state" sections are superseded by the
  2026-10-08 audit in `PLAN_VERSION_130.md`.
- Refresh every "Last updated" header.

Deliverable: the doc edits.

Verification: `cargo xtask lint-markdown`; the pre-commit markdownlint and prettier hooks
pass.

Prohibitions: do NOT touch code; do NOT expand the reference document's protocol sections
(each protocol task refreshes its own); do NOT proceed to Task 127.

Stop: report files changed; await review.

**Status: Complete (2026-10-08).** Graphics, OSC 99, kitty keyboard and OSC 10/11 set are
downgraded to 🚧 with task pointers. Every listed row was added to COVERAGE, and the gap
rows went into GAPS (a new "CSI Gaps" table holds the CSI extensions). OSC 66 was corrected
in all three docs. The GAPS "fully implemented" and "substantially compliant" claims were
replaced. The `KITTY_PROTOCOL_REFERENCE.md` roadmap table was renumbered, a superseded-audit
note added, and its stub pointers corrected. Each doc got a 2026-10-08 "Last updated" entry,
which also records the 126.1–126.3 behaviour changes. The three files are markdownlint and
prettier clean. `cargo xtask lint-markdown` still exits non-zero for reasons outside this
subtask; see 126.C1.

### 126 Cleanup entries

#### 126.C1 — `cargo xtask lint-markdown` fails on the whole tree

- **Surfaced:** 126.4 verification on `task-126/safety-gate` (2026-10-08).
- **Impact:** `cargo xtask lint-markdown` runs `markdownlint-cli2 "**/*.md" "!target"
"!**/target"` and exits 1, so the command cannot serve as a pass/fail gate. It reports
  about 14,200 errors before this branch, and that count is unchanged by it. About 13,600
  come from `.direnv/flake-inputs/*/` copies of the repo (the glob does not exclude
  `.direnv`). The rest (about 550) are existing lint debt in other `Documents/*.md` files,
  mostly MD060 table alignment and MD033 inline HTML in `PLAN_122_*.md` and
  `PLAN_VERSION_090.md`. The pre-commit markdownlint hook only checks staged files, so it
  passes. The three 126.4 docs went from 15 errors to 0.
- **Scope of fix:** `xtask/src/main.rs` (`lint_markdown`) and/or `.markdownlint-cli2.yaml`
  (add `.direnv` to the ignores), plus a lint-debt pass over the remaining `Documents/`
  files, or a deliberate decision to scope the command.
- **Suggested approach:** first exclude `.direnv/**` (the smallest change, and it removes
  ~96% of the noise). Then either fix the remaining files (prettier realignment fixes most
  MD060) or narrow the glob.
- **Verification:** `cargo xtask lint-markdown` exits 0 on a clean checkout with a
  populated `.direnv`.
- **Scheduling:** independent of all v0.13.x tasks. Can land any time; it does not block
  Task 127 or 128.

---

## Task 127 — Unfocused / Inactive-Pane Cursor (GitHub #531)

### 127 Summary

Today freminal hides the cursor entirely in inactive panes (`widget.rs:3244`:
`effective_show_cursor = snap.show_cursor && !is_echo_off && is_active_pane`). When the OS
window loses focus, the active pane keeps a solid, blinking cursor. The change: a pane that
does not have keyboard focus draws a **steady hollow block**. That means an inactive pane in a
focused window, **or** any pane in an unfocused window.

### 127 Behaviour (maintainer decisions, 2026-10-08)

- **Both triggers.** Inactive split panes **and** OS-window focus loss (the latter is the
  model used by kitty, Alacritty, Ghostty and WezTerm).
- **All styles become a hollow block.** Block, bar and underline all draw as a hollow block
  when unfocused (WezTerm, Ghostty and kitty's default). The "not focused" signal is then
  independent of the app's DECSCUSR.
- **Hidden stays hidden.** DECTCEM-off and password echo-off (the lock icon) hide the cursor
  in every focus state. Every terminal checked does this.
- **Steady, no trail, same colour.** The unfocused cursor never blinks: no 500 ms wake for
  unfocused panes or windows. Its trail snaps rather than animates. It uses the normal cursor
  colour (OSC 12 override or theme), undimmed.
- **Configurable.** New option `[cursor] unfocused_style = "hollow" | "unchanged" | "hidden"`,
  default `"hollow"`:
  - `unchanged` draws the app's DECSCUSR shape, solid but **steady**;
  - `hidden` reproduces today's behaviour (inactive panes show no cursor). It also hides the
    cursor in an unfocused window.

### 127 Types (decided)

- `freminal-common/src/config.rs`: `pub enum UnfocusedCursorStyle { Hollow, Unchanged, Hidden }`.
  - Serde `rename_all = "snake_case"`, `#[default] Hollow`.
  - New field `CursorConfig::unfocused_style: UnfocusedCursorStyle`.
- New module `freminal/src/gui/terminal/cursor_appearance.rs` (one concept: deciding how a
  pane's cursor is drawn). It contains:
  - `pub(crate) enum CursorFocus { Focused, InactivePane, UnfocusedWindow }`.
  - `pub(crate) enum CursorAppearance { Hidden, Solid(CursorVisualStyle), Hollow }`.
    `Solid` carries the DECSCUSR style; blink gating for `Solid` stays in
    `cursor_blink_is_visible`.
  - `pub(crate) const fn cursor_focus(pane: PaneFocus, window: WindowFocus) -> CursorFocus`.
    An unfocused window wins over an inactive pane: both produce a non-focused cursor, and
    the window variant is the one blink gating needs.
  - `pub(crate) fn resolve_cursor_appearance(inputs: &CursorAppearanceInputs) -> CursorAppearance`.
    The inputs struct has these fields:
    - `snapshot_visible: CursorVisibility`, an enum `{ Shown, Hidden }` derived from
      `snap.show_cursor`;
    - `echo: EchoState`, using the existing echo-off type if one exists, otherwise a new
      `{ Normal, EchoOff }` in this module;
    - `focus: CursorFocus`;
    - `style: CursorVisualStyle`;
    - `unfocused_style: UnfocusedCursorStyle`.

  Under `Unchanged`, a non-focused pane resolves to `Solid` with the **steady** variant of
  the style: `BlockCursorBlink` maps to `BlockCursorSteady`, and so on.

- `PaneFocus` (`terminal/input.rs:1395`) and `WindowFocus` (`frame_drain.rs:70`) already
  exist and are reused. No new `bool` is introduced anywhere
  (`freminal-state-representation`).

### 127 Subtasks

#### 127.1 — Config option `[cursor] unfocused_style`

Scope:

- `freminal-common/src/config.rs`: `UnfocusedCursorStyle`, `CursorConfig` field and
  `Default`, the `ConfigPartial` / `apply_partial` path, and tests;
- `config_example.toml`: the `[cursor]` section;
- `nix/home-manager-module.nix`: the cursor options.

What: add the enum and field exactly as specified in "127 Types". Follow the
`freminal-config-options` checklist end to end (struct, default, partial, apply, example,
home-manager, round-trip test). `CursorConfig` is a whole-section `Option` in
`ConfigPartial`, so `#[serde(default)]` on the new field keeps old configs loading.

Deliverable:

- the option;
- tests:
  - a TOML round-trip for all three values;
  - a missing key defaults to `hollow`;
  - an old config without the key loads;
  - an unknown value fails to parse.

Verification: `cargo test --all`; clippy; the `nix flake check` lint hooks (`nixfmt`,
`statix`, `deadnix`) pass on commit.

Prohibitions: do NOT touch the GUI; do NOT add a Settings UI control (127.7); do NOT proceed
to 127.2.

Stop: report and await review.

#### 127.2 — Pure appearance decision module

Scope:

- `freminal/src/gui/terminal/cursor_appearance.rs` (new);
- `freminal/src/gui/terminal/mod.rs` (module declaration only).

What: implement `CursorFocus`, `CursorAppearance`, `CursorAppearanceInputs`,
`cursor_focus()` and `resolve_cursor_appearance()` exactly as specified in "127 Types". This
module is pure: it does not depend on egui, the renderer or snapshot internals beyond
`CursorVisualStyle`. Nothing calls it yet.

Deliverable:

- the module;
- an exhaustive table test over visibility × echo × focus × all 6 styles × all 3 config
  values, asserting:
  - hidden or echo-off always gives `Hidden`;
  - `Focused` always gives `Solid(style)` unchanged;
  - non-focused with `Hollow` gives `Hollow`;
  - non-focused with `Unchanged` gives `Solid(steady variant)`;
  - non-focused with `Hidden` gives `Hidden`;
- `cursor_focus` tests covering all four input combinations.

Verification: `cargo test --all`; clippy (the module must not need `#[allow(dead_code)]`;
if clippy flags it as unused, gate it with `#[cfg_attr(not(test), expect(dead_code))]` plus
a `TODO(127.5)` comment — the documented temporary-refactor exception).

Prohibitions: do NOT wire it into `widget.rs`; do NOT proceed to 127.3.

Stop: report and await review.

#### 127.3 — Vertex builder: one cursor-quad helper, hollow geometry, exact cursor range

Scope: `freminal/src/gui/renderer/vertex.rs` (production code and tests); every
`BackgroundFrame` construction site and `build_cursor_verts_only` call site that must change
to compile:

- `freminal/src/gui/terminal/widget.rs` (call-site argument changes only);
- `freminal/src/gui/renderer/headless.rs`;
- `freminal/benches/render_loop_bench.rs`.

What:

1. Introduce `pub struct CursorDrawParams` in `vertex.rs` with these fields:
   - `appearance: CursorAppearance`;
   - `col`, `row`;
   - `color: [f32; 4]`;
   - `x_scale: f32` (DECDWL);
   - `blink_on`, reusing the existing blink-phase type if one exists. If the code uses a raw
     `bool`, wrap it in `pub enum CursorBlinkPhase { On, Off }` defined in `vertex.rs`.

   Replace the `show_cursor: bool` + `cursor_visual_style` pair in `BackgroundFrame` and in
   `build_cursor_verts_only`'s parameter list with a single `cursor: CursorDrawParams`. This
   also drops `build_cursor_verts_only` below clippy's argument limit, so remove its
   `too_many_arguments` allow if it becomes unnecessary.

2. Extract `fn push_cursor_quads(params: &CursorDrawParams, cell_width: u32, cell_height: u32, out: &mut Vec<f32>)`.
   It replaces both duplicated geometry `match`es (`vertex.rs:619-646` and `:677-701`):
   - `Hidden` emits nothing;
   - `Solid(style)` emits today's single quad exactly, gated by `cursor_blink_is_visible`;
   - `Hollow` emits **4 non-overlapping inset quads** (top, bottom, left, right). Thickness
     is `max(cell_width * 0.1, 1.0)`, rounded to whole pixels; the left and right edges span
     only the interior height, so corners are not double-blended. When the cell is too small
     for an interior, it degrades to a single filled quad. `Hollow` ignores blink phase
     (always steady).
3. Change `build_background_instances`' return value from "cursor quad appended" to
   `CursorVertRange { start: usize, len: usize }`, the exact float range of the cursor
   inside `deco` (`len == 0` when nothing was drawn). `build_cursor_verts_only` keeps
   returning the cursor floats.

Deliverable:

- the refactor and hollow geometry;
- tests:
  - hollow emits exactly `4 * CURSOR_QUAD_FLOATS` floats with no overlapping quads;
  - the degenerate tiny cell falls back to one quad;
  - `Solid` output is byte-identical to the pre-change output for all 6 styles (pin with
    the existing `bg_instances_cursor_*` tests);
  - `Hidden` emits nothing;
  - `build_background_instances` and `build_cursor_verts_only` emit identical cursor floats
    for the same params;
  - `CursorVertRange` is correct for all appearances.

Benchmarks: capture `instanced_bg` and `instanced_bg_partial_dirty`
(`freminal/benches/render_loop_bench.rs`) before and after, per `performance-benchmarks` and
`freminal-bench-table`. The 15% threshold applies; include the table in the report.

Verification: `cargo test --all`; clippy; `cargo bench --no-run --all`.

Prohibitions:

- do NOT change cursor-only patching in `widget.rs` beyond what compiles (127.4);
- do NOT produce `Hollow` from real focus state yet: callers pass `Solid` or `Hidden`,
  derived exactly from today's `effective_show_cursor`;
- do NOT change shaders or the vertex format;
- do NOT proceed to 127.4.

Stop: report with the benchmark table; await review.

#### 127.4 — Variable-length cursor patching

Scope: `freminal/src/gui/terminal/widget.rs`, limited to:

- `patch_cursor_only_deco_verts` and its tests (`widget.rs:152-167`, `:5786-5870`);
- the `cursor_vert_float_offset` bookkeeping (`widget.rs:~3781-3786`);
- the `PaneRenderCache` field that stores it.

What: the cursor-only fast path assumes the cursor is exactly 0 or `CURSOR_QUAD_FLOATS`
floats long. Replace that assumption with the authoritative `CursorVertRange` from 127.3:

- store `CursorVertRange` in the cache instead of a computed offset;
- the patch becomes **truncate `deco_verts` to `range.start`, then extend with the new
  cursor floats**, and the new length is recorded;
- hiding the cursor is then a truncate, and the "zero in place" path is deleted.

This is the issue #432 corruption class (offset bookkeeping), so it gets dedicated
regression tests.

Deliverable:

- the change;
- tests:
  - solid → hollow → hidden → solid patches across consecutive cursor-only frames leave
    `deco_verts` identical to a full rebuild each time;
  - the cursor remains the final region of `deco_verts`;
  - a #432-style regression test where a full rebuild happens between cursor-only frames of
    differing lengths.

Verification: `cargo test --all`; clippy. `headless_workloads.rs` byte and call-count
assertions must stay green for solid cursors.

Prohibitions: do NOT wire focus (127.5); do NOT touch `vertex.rs`; do NOT proceed to 127.5.

Stop: report and await review.

#### 127.5 — Wire focus into the pane renderer, with change detection

Scope:

- `freminal/src/gui/terminal/widget.rs`: `show()`, the `effective_show_cursor` block at
  `:3236-3244`, `CursorFrameInputs` (`:~3345`), the `PaneRenderCache` `previous_*` fields
  (`:~3979`), and the comment at `:~1746`;
- `freminal/src/gui/terminal/frame_dirty.rs`: `cursor_state_changed` (`:804-808`) and the
  cursor-visibility AND (`:~728`);
- `freminal/src/gui/app_impl.rs`: only the `show()` call site (`:~2860`), to pass window
  focus.

What:

- Compute `CursorFocus` via `cursor_focus(pane_focus_now, window_focus)`. `pane_focus_now`
  is already derived at `widget.rs:3046`. Take window focus from `WindowFocus` (passed into
  `show()` as a new parameter of that existing enum type — not a bool) at the call site.
- Replace `effective_show_cursor` with `resolve_cursor_appearance(…)`. The fold or off-screen
  visibility AND in `frame_dirty.rs:728` maps a visible appearance to `Hidden`.
- Feed the appearance into `CursorDrawParams` for both the full-rebuild path and the
  cursor-only path.
- **Change detection.** Add `CursorAppearance` to `CursorFrameInputs` and to the
  `previous_*` cache, and include it in `cursor_state_changed`. A focus switch changes the
  shape while visibility stays constant; without this the stale quad would be presented.
- For `Hollow` and the steady `Solid` produced for non-focused panes, normalise the
  blink-phase input to a constant, so blink flips do not register as cursor-state changes.
- Update the `widget.rs:~1746` comment ("only the active pane ever produces a cursor rect"),
  which becomes false.

Damage classification (`freminal-damage-model`): focus transitions are already `Full`
presents (`app_impl.rs:274-276` `active_pane_changed`; chrome `focus_changed`). An inactive
pane's cursor moving under background output is **BOUNDABLE-NOW**: it produces
`CursorOnly`/`Region` for its own old and new cells via `PaneDamageRect::from_cursor_cells`.
No new damage category is needed.

Deliverable:

- the wiring;
- `frame_dirty.rs` tests:
  - a focus-only change, with visibility constant, triggers `CursorOnly`;
  - a steady hollow cursor does not retrigger on blink-phase flips;
  - DECTCEM-off in an inactive pane stays hidden;
- a widget-level test that the inactive pane emits hollow quads and the active pane emits
  a solid quad.

Verification: `cargo test --all`; clippy; `cargo test -p freminal --features frame-profiling`
compiles and passes.

Prohibitions:

- do NOT change blink-wake scheduling or trail behaviour (127.6);
- do NOT add a `bool` parameter;
- do NOT touch the snapshot or PTY thread (focus is GUI-only state per
  `freminal-architecture`);
- do NOT proceed to 127.6.

Stop: report and await review.

#### 127.6 — Blink-wake gating and trail snapping for unfocused cursors

Scope:

- `freminal/src/gui/app_impl.rs`: `cursor_blink_wants_repaint` (`:99-112`), its call site
  (`:~3060-3091`), and its tests (`:~4911`, `:~4945`);
- `freminal/src/gui/terminal/frame_dirty.rs`: the `update_cursor_animation` call (`:~734`);
- `freminal/src/gui/view_state.rs`: only if `update_cursor_animation` needs a snap entry
  point.

What:

1. Change `cursor_blink_wants_repaint` to take `CursorAppearance` (or its inputs) instead of
   `show_cursor` / `is_active` / `is_echo_off` bools. A wake is wanted only when the
   appearance is `Solid` with a blink style and focus is `Focused`. An unfocused window
   therefore stops waking for blinks. This also retires three bool parameters.
2. When focus is not `Focused`, the cursor trail must not animate. If `update_cursor_animation`
   has no existing way to jump the trail straight to its target, add
   `ViewState::snap_cursor_animation(col, row)`, which sets the trail position equal to the
   target and returns "not animating". Call it instead of `update_cursor_animation` for
   non-focused panes.

Deliverable:

- the change;
- updated repaint-gating tests covering focused blink (wake), inactive pane (no wake),
  unfocused window (no wake), hidden (no wake) and echo-off (no wake);
- a test that a non-focused pane reports "not animating" after a cursor move with the trail
  enabled.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT change trail behaviour for the focused pane; do NOT proceed to 127.7.

Stop: report and await review.

#### 127.7 — Settings UI control and live preview

Scope: the Settings Cursor tab in `freminal/src/gui/settings/` (find it by the existing
`trail` / `blink` controls); `CursorPreview` and the `VisualPreview` diff
(`freminal/src/gui/visual_preview.rs:71-81`, `:364`, `:403-443`); the settings dispatch
(`settings_dispatch.rs:~577`); and the widget toggles (`widget.rs:2575-2587`) if the cursor
config is plumbed there.

What:

- Add an "Unfocused cursor" combo box offering Hollow, Unchanged and Hidden to the Cursor
  tab.
- Per `freminal-pending-visual-preview`, add `unfocused_style` as a field of `CursorPreview`
  so the choice previews live before Apply and reverts on Cancel. Do NOT add a new
  `Preview*`/`Revert*` action pair.
- The combo box follows `freminal-cursor-affordances` (combo box affordance).

Deliverable:

- the control and preview;
- a `VisualPreview` diff test that a changed `unfocused_style` is detected and reverted.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT add a `SettingsAction` variant beyond what the existing preview flow
requires; do NOT proceed to 127.8.

Stop: report and await review.

#### 127.8 — Pixel golden, final verification, Windows cross-check

Scope:

- `freminal/src/gui/renderer/pixel_harness.rs` tests;
- `freminal/tests/golden_pixels/` (new golden image);
- `CHANGELOG`/README entries only if the repo convention requires them for user-visible
  config.

What:

- Add a pixel-harness golden: one frame containing a solid block cursor in the active pane
  and a hollow cursor in an inactive pane. This runs in the `gl-pixel` dev shell and CI job.
- Run the full verification suite plus `cargo xtask check-windows`.

Deliverable: the golden test and a verification report.

Verification: `cargo test --all`; clippy; `cargo machete`; `cargo xtask check-windows`; the
pixel test under `nix develop .#gl-pixel`.

Prohibitions: do NOT loosen any pixel tolerance; do NOT proceed beyond Task 127.

Stop: report and await review. Task 127 then goes to PR.

---

## Task 128 — Prefix- & Intermediate-Aware CSI Dispatch

### 128 Summary

`AnsiCsiParser::ansiparser_inner_csi` (`csi.rs:257-453`) dispatches on the **final byte
only**. Private-marker bytes (`?`, `>`, `<`, `=`) sit inside `params`, and intermediates
(0x20–0x2F) are consulted only for `p` and `c`. As a result, these sequences execute as the
wrong command today:

| Sequence            | Real meaning     | Freminal today                                      |
| ------------------- | ---------------- | --------------------------------------------------- |
| `CSI Ps + T`        | kitty unscroll   | SD (scrolls in blank lines)                         |
| `CSI Ps # P`        | XTPUSHCOLORS     | DCH (**deletes characters**)                        |
| `CSI … $ r`         | DECCARA          | DECSTBM (rejected for >2 params, else sets margins) |
| `CSI Ps * x`        | DECSACE          | DECREQTPARM (**writes an unsolicited reply**)       |
| `CSI … $ t`         | DECRARA          | window ops (`Pt=8` can resize the window)           |
| `CSI Ps SP t`       | DECSWBV          | window ops                                          |
| `CSI Ps " q`        | DECSCA           | DECSCUSR (changes the cursor style)                 |
| `CSI Ps q`          | DECLL            | DECSCUSR                                            |
| `CSI Ps SP u`       | DECSMBV          | SCORC (restores the cursor)                         |
| `CSI Ps SP @`       | SL               | ICH                                                 |
| `CSI > SP c`        | (nothing)        | DA2                                                 |
| `CSI ? s`/`CSI ? r` | XTSAVE/XTRESTORE | DECSLRM/DECSTBM (fail to parse: `Invalid`)          |

This is the foundation for Tasks 103, 137, 141, 142, 143 and 144, and the first kitty
task executed. It **only routes**. No new sequence is implemented here: every newly
distinguished sequence becomes "recognised, unhandled" (warn-logged, no output) until its own
task implements it.

### 128 Design (decided)

- **`ansi_components/csi_key.rs`** (new, pure; one concept: the identity of a CSI sequence):
  - `pub(crate) enum CsiPrefix { None, Question, Greater, Less, Equals }`. Named by byte,
    because meaning depends on the final byte.
  - `pub(crate) enum CsiIntermediate { None, Space, Bang, DoubleQuote, Hash, Dollar, Percent, Ampersand, Apostrophe, LeftParen, RightParen, Star, Plus, Comma, Minus, Dot, Slash, Multiple }`.
    `Multiple` means two or more intermediates; no route accepts it.
  - `pub(crate) struct CsiKey { prefix: CsiPrefix, intermediate: CsiIntermediate, final_byte: u8 }`.
  - `pub(crate) enum CsiKeyError { MisplacedPrivateMarker { index: usize } }`. Returned when a
    0x3C–0x3F byte appears anywhere other than params index 0.
  - `impl CsiKey { pub(crate) fn classify(params: &[u8], intermediates: &[u8], final_byte: u8) -> Result<Self, CsiKeyError> }`.
- **`ansi_components/csi_dispatch.rs`** (new; one concept: the routing table):
  - Entry point:
    `pub(crate) fn dispatch_csi(key: CsiKey, params: &[u8], intermediates: &[u8], raw: &dyn Fn() -> String, output: &mut Vec<TerminalOutput>) -> ParserOutcome`.
    The `raw` closure produces `format_raw_csi()` lazily for logging.
  - It matches on `(prefix, intermediate)` first, selecting one small group function, then on
    `final_byte` inside the group.
  - Groups: `plain`, `question`, `greater`, `less`, `equals`, `space`, `bang`, `dollar`,
    `question_dollar`. No group function needs a `too_many_lines` allow. The `csi.rs:255`
    allow is removed.
- **Handler signatures are unchanged.** The router passes `params` unchanged, prefix byte
  included, so handlers that sniff their own prefix keep working. The `>` → modifyOtherKeys
  route goes **directly** to `modify_other_keys`, not via `sgr`.
- **Unknown combinations** (valid grammar, no route) and `CsiKeyError`: log with
  `tracing::warn!` using the existing unknown-final message format, emit **no** output, and
  return `ParserOutcome::Finished`. Never fall through to the base command.
  `TerminalOutput::Invalid` stays reserved for known keys with bad parameters, which is the
  handler's job.
- **The no-space `CSI Ps q` is no longer DECSCUSR (decision).** It is DECLL in xterm, which
  freminal does not implement, so it becomes recognised-unhandled. DECSCUSR requires the SP
  intermediate per spec. `tests/csi_commands_decrqm_decscusr.rs:110-120` is updated
  deliberately.
- **Routed set** (the complete legitimate table today; everything else is unhandled):

| Prefix | Inter | Finals                                                                  |
| ------ | ----- | ----------------------------------------------------------------------- |
| none   | none  | `A B C D E F G H I J K L M P S T X Z @ \` b d f g m h l n c r s u t x`  |
| none   | `$`   | `p` (DECRQM ANSI)                                                       |
| none   | `!`   | `p` (DECSTR)                                                            |
| none   | SP    | `q` (DECSCUSR)                                                          |
| `?`    | none  | `h`, `l` (DECSET/DECRST), `n` (DSR private), `u` (kitty keyboard query) |
| `?`    | `$`   | `p` (DECRQM private)                                                    |
| `>`    | none  | `c` (DA2), `q` (XTVERSION), `m` (XTMODKEYS), `u` (kitty push)           |
| `<`    | none  | `u` (kitty pop)                                                         |
| `=`    | none  | `c` (DA3), `u` (kitty set)                                              |

- **tmux passthrough** (`terminal_handler/dcs.rs` `dispatch_tmux_csi`) keeps its separate
  direct-dispatch path. That path exists for ordering: a DCS-wrapped CUP must run before a
  wrapped kitty APC in the same batch. It gains a guard: a 0x3C–0x3F first param byte, or
  any intermediate, makes it return `false` (queue for re-parse). **Unifying the tmux path
  with `AnsiCsiParser` is out of scope.** It changes reply wrapping and ordering and needs a
  maintainer decision (recorded under Task 131's open questions, since its reset-lifecycle
  work touches the same handler surface).

### 128 Subtasks

#### 128.1 — `CsiKey` classification types

Scope:

- `freminal-terminal-emulator/src/ansi_components/csi_key.rs` (new);
- `freminal-terminal-emulator/src/ansi_components/mod.rs` (module declaration).

What: implement `CsiPrefix`, `CsiIntermediate`, `CsiKey`, `CsiKeyError` and
`CsiKey::classify` exactly as designed. Behaviour-neutral: nothing calls it yet.

Deliverable: the module and table-driven tests:

- each prefix, plus no prefix;
- each of the 16 intermediate bytes, no intermediate, and two intermediates (`Multiple`);
- a misplaced marker in the middle or at the end of params gives `MisplacedPrivateMarker`
  with the correct index;
- colon sub-parameters do not affect classification.

Verification: `cargo test --all`; clippy. Use the documented `expect(dead_code)` +
`TODO(128.3)` exception if the types are unused outside tests.

Prohibitions: do NOT change dispatch; do NOT proceed to 128.2.

Stop: report and await review.

#### 128.2 — Extract inline arms into handler modules (pure moves)

Scope:

- `freminal-terminal-emulator/src/ansi_components/csi.rs`;
- `csi_commands/decreqtparm.rs` (new);
- `csi_commands/dec_modes.rs` (new);
- `csi_commands/mod.rs`;
- `csi_commands/decrqm.rs` (import path only).

What:

- Move the inline DECREQTPARM arm (`csi.rs:396-435`) into
  `pub fn ansi_parser_inner_csi_finished_decreqtparm(params: &[u8], output: &mut Vec<TerminalOutput>) -> ParserOutcome`.
  Move its tests (`csi.rs:~699-734`) with it.
- Move `push_split_mode_params` from `csi.rs` into `csi_commands/dec_modes.rs`. This removes
  the `decrqm.rs` → `csi.rs` import loop.
- Leave the `x` arm's dead `intermediates.contains(&b'>')` check in place; 128.5 removes
  it. This subtask changes **no** behaviour.

Deliverable: the moves; all existing tests pass unchanged except for their module path.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT change behaviour; do NOT proceed to 128.3.

Stop: report and await review.

#### 128.3 — Strict router

Scope:

- `freminal-terminal-emulator/src/ansi_components/csi.rs` (`ansiparser_inner_csi` and its
  tests);
- `ansi_components/csi_dispatch.rs` (new);
- `ansi_components/mod.rs`;
- `freminal-terminal-emulator/tests/csi_commands_decrqm_decscusr.rs` (the `:110-120` case);
- `freminal-terminal-emulator/tests/csi_dispatch_matrix.rs` (new).

What:

- Implement `dispatch_csi` with the group functions and exactly the routed set in "128
  Design".
- `ansiparser_inner_csi` becomes: `push`; on `Finished(b)`, `CsiKey::classify`, then
  `dispatch_csi`. Remove its `too_many_lines` allow.
- Route `(Greater, None, b'm')` directly to `modify_other_keys`.
- Update `decreqtparm_with_gt_prefix_is_invalid`: `CSI > 0 x` now yields no output, because
  there is no route for `(Greater, None, b'x')`.
- Update the `"\x1b[q"` case in `csi_commands_decrqm_decscusr.rs`.

Deliverable:

- the router;
- `csi_dispatch_matrix.rs`: a cross-product of the 5 prefixes × {none, SP, `!`, `"`, `#`,
  `$`, `*`, `+`, Multiple} × every final byte 0x40–0x7E. It asserts output is produced only
  for the routed set, and that every other key yields an empty output and `Finished`.
- One named regression test per row of the "128 Summary" table, asserting the misroute no
  longer happens.

Benchmarks: run `bench_parse_plain_text`, `bench_parse_sgr_heavy`, `bench_parse_cup_writes`
and `bench_parse_bursty` before and after (`freminal-bench-table` names their file). The 15%
threshold applies; include the table in the report.

Verification: `cargo test --all`; clippy; `cargo bench --no-run --all`.

Prohibitions: do NOT implement any newly distinguished sequence; do NOT change handler
signatures; do NOT touch the tmux path; do NOT proceed to 128.4.

Stop: report with the benchmark table; await review.

#### 128.4 — tmux passthrough prefix/intermediate guard

Scope: `freminal-terminal-emulator/src/terminal_handler/dcs.rs` (`dispatch_tmux_csi`, the
stale doc comment at `:131-132`, and its tests).

What:

- `dispatch_tmux_csi` returns `false` (queue for re-parse through the strict main parser)
  when the CSI body's first byte is 0x3C–0x3F or any intermediate is present. Reuse
  `CsiKey::classify`: any prefix other than `None`, or any intermediate other than `None`,
  falls through.
- Fix the stale "CSI: not yet supported" comment.

Deliverable:

- the guard;
- tests:
  - `CSI > 0 T` falls through instead of scrolling;
  - `CSI 3 + T` falls through;
  - plain CUP is still dispatched directly;
  - the existing ordering test still passes.

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT route the tmux path through `AnsiCsiParser`; do NOT change reply
wrapping; do NOT proceed to 128.5.

Stop: report and await review.

#### 128.5 — Dead-branch cleanup and dispatch-table doc

Scope:

- `csi_commands/da.rs`: the dead `intermediates` checks at `:49,54` and their tests at
  `:135,180,191`;
- `csi_commands/decrqm.rs`: the unreachable `h`/`l` branches and their tests;
- `csi_commands/decreqtparm.rs`: the dead `>` intermediate check;
- `csi_commands/sgr.rs`: the `>` delegation at `:31` and its tests;
- `csi_commands/mod.rs`: rewrite the stale dispatch-table doc at `:9-53`;
- `ansi_components/ansi.rs`: `split_params_into_colon_delimited_usize`. Keep it and add a
  doc note that Task 103 is its intended consumer. Do not delete it.

What: remove the branches made unreachable by 128.3, and correct the `mod.rs` doc. Its
current errors: `W` listed as TBC, `~` listed as REP, `p`+`>` as MODKEYS, `p`+`$` as DECSLPP,
private markers labelled "Intermediate", and missing rows. Keep `scorc.rs`'s numeric →
RestoreCursor branch: plain `CSI Ps u` is still routed there, and narrowing it is a separate
behaviour decision.

Deliverable: the cleanup; behaviour-neutral (the dispatch matrix test from 128.3 is
unchanged).

Verification: `cargo test --all`; clippy.

Prohibitions: do NOT change routed behaviour; do NOT proceed to 128.6.

Stop: report and await review.

#### 128.6 — Escape-sequence dual-doc update

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`.

What:

- **Correct canonical forms:** XTVERSION is `CSI > Ps q`. DECSCUSR requires SP (note that a
  bare `CSI Ps q` is no longer accepted).
- **Add missing rows:** `CSI < u`, DA3 `CSI = c`, `CSI > Ps ; Pv m`, DECREQTPARM `CSI Ps x`,
  DSR `CSI ? Ps n`.
- **Record as recognised/ignored**, each pointing to its implementing task:
  - `CSI # P/Q/R` → Task 144;
  - `CSI + T` → Task 142;
  - `CSI $ r` and `CSI * x` → Task 143;
  - `CSI ? s/r` → Task 141;
  - `CSI > … SP q` → Task 103;
  - DECRARA and DECSCA → out of scope / not implemented.
- **GAPS:** correct the DECSCA claim at `:227` and `:386`. It now genuinely has no effect.
- Refresh both "Last updated" headers.

Deliverable: the doc edits.

Verification: `cargo xtask lint-markdown`.

Prohibitions: do NOT touch code; do NOT proceed beyond Task 128.

Stop: report and await review. Task 128 then goes to PR.

---

## Foundation stubs (v0.13.0)

### Task 129 — Kitty Wire Infrastructure (stub)

**Goal.** Build once the parsing and payload plumbing that every kitty OSC, APC and DCS
protocol re-implements today.

**Audit findings.**

- **Four hand-rolled `key=value` parsers:**
  - OSC 99, colon-separated (`osc_notify_99.rs:409-464`);
  - graphics, comma-separated (`kitty_graphics.rs:336-446`);
  - FTCS (`ftcs.rs`);
  - iTerm2 (`osc_iterm2.rs`).

  Their integer and base64 helpers are private duplicates. OSC 5113, 5522, 66, 21 and 72
  would each add another.

- **The single base64 module is lenient.** `freminal-common/src/base64.rs` has optional
  padding, no whitespace handling, no streaming and no length cap, and it reserves `len*3/4`
  up front. OSC 5522 requires strict padded input and rejection of invalid input. OSC 5113's
  stock client emits **unpadded** base64. OSC 99 corrupts chunked base64 whose chunk length
  is not a multiple of 4.
- **Three chunk accumulators** with independent caps and abandonment rules: graphics
  (`terminal_handler/mod.rs:~99`), OSC 99 `pending_notifications`, and iTerm2 `FilePart`.
- **No size cap on any OSC, APC or DCS accumulator** (`osc.rs:25-33`, `apc.rs`, `dcs.rs`).
- **OSC tokenisation is lossy.** It splits on every `;` and numifies tokens
  (`osc.rs:391-398`). OSC 9, 99, 777 and 1337 work around it via `raw_params`. OSC 8 URLs
  containing `;` break.
- **`AnsiOscParser::push` pops every trailing `0x5c`/`0x07`/`0x1b`** (`osc.rs:108-116`), so a
  payload that ends in `\` loses it.
- **Unknown OSC and DCS log the full raw payload at warn** (`ansi_components/osc.rs:362-369`,
  `terminal_handler/dcs.rs:~49`). That is a privacy and log-spam hazard for clipboard,
  transfer and notification payloads, and for kitten `@kitty-*` DCS.
- **No APC response helper.** Graphics hand-builds `ESC _G … ESC \` (`kitty_graphics.rs:507-522`)
  and ignores S8C1T.

**Scope sketch.**

- **`freminal-common`:**
  - a bounded metadata tokenizer, parameterised by separator (`:`, `,`, `;`), yielding
    `(key, value)` slices with a pair-count cap;
  - strict and lenient base64 decoders, a streaming decoder for chunked payloads, and an
    unpadded encoder.
- **`freminal-terminal-emulator`:**
  - a `BoundedChunkAssembler` (per-sequence and per-session byte caps, explicit abandonment);
  - OSC, APC and DCS parser byte caps;
  - raw-params as the default OSC dispatch input (opt-in tokenisation for the numeric OSCs);
  - the trailing-backslash fix;
  - a payload-safe unknown-sequence log (length and introducer only; silence `@kitty-*`);
  - `apc_response()` in `pty_writer.rs`.
- **Migrate the existing consumers** (graphics, OSC 99, iTerm2) onto the shared pieces.
  Behaviour must stay identical, apart from the bugs listed above.

**Durable decisions.**

- One tokenizer, one base64 module, one chunk assembler. Protocol modules keep their own
  typed models and error enums.
- Caps follow the `MAX_OSC99_*` precedent: named constants, per protocol.

**Open questions (decide at activation).**

- Whether the OSC parser cap is global (e.g. kitty's 256 KB escape cap) or per-target.
- Whether raw-params becomes the default for **all** OSC targets or only kitty-family ones.

### Task 130 — Reverse-Path & Capability-Query Consistency (stub)

**Goal.** Every terminal-to-application reply is framed by the handler (S8C1T-aware,
tmux-passthrough-wrapped). Every capability query is answered on the PTY thread, so its
reply precedes the DA1 reply that the kitty detection recipes rely on. Advertised
capabilities match what is actually implemented and enabled.

**Audit findings.**

- **GUI-originated replies bypass `write_osc_response`.** The OSC 52 query reply
  (`rendering.rs:549`) and all OSC 99 reports (`notifications.rs:553-597`) are hand-formatted
  bytes sent on `pty_write_tx`, so they are not S8C1T-aware and not tmux-wrapped.
- **The OSC 99 `p=?` reply is GUI-asynchronous.** It arrives after DA1, so the spec's
  detection concludes "unsupported". It is also not sent while the window is not rendering
  frames.
- **OSC 99 advertises capabilities it lacks.** It answers `p=?` while notifications are
  disabled (the default), and advertises `a=report` on platforms that never send it.
- **Capability queries are scattered** across DECRQM, XTGETTCAP, `p=?`, `a=q` and `CSI ? u`,
  with no single source of truth.

**Scope sketch.**

- A handler-side reply path that GUI-originated events can request through: a typed
  `InputEvent` variant carrying a structured reply, serialised by the handler.
- The OSC 99 `p=?` reply moves to the PTY thread, computed from config passed to the PTY
  side.
- A small capability registry consulted by every query handler.

**Durable decisions.** No new PTY write channel. Replies use the existing
`write_to_pty`/`pty_writer.rs` helpers (a new channel would need maintainer sign-off per
`freminal-version-activation`).

**Open questions.**

- How config (notification enablement, routing) reaches the PTY thread for truthful
  capability answers: snapshot of config at spawn plus update events, or a shared immutable
  `Arc` swapped on config reload.

### Task 131 — Screen-Scoped State & Reset Lifecycle (stub)

**Goal.** One explicit mechanism for state that is per-screen (main vs alternate) and for
what RIS and DECSTR reset. Each protocol registers into it instead of being added to a
hand-maintained list.

**Audit findings.**

- **Kitty keyboard stack.** It is hand-swapped in `handle_enter_alternate` /
  `handle_leave_alternate` (`scroll_ops.rs:93-120`). A second `?1049h` replaces the saved
  main stack (B15): the call site does not guard double entry, while `Buffer::enter_alternate`
  does.
- **Graphics.** Placement maps are handler-global, while image stores are swapped per screen.
  `kitty_state` is not cleared on RIS.
- **Pointer-shape stack.** OSC 22 needs per-screen stacks (Task 139). Multiple cursors clear
  on screen switch (Task 103). XTSAVE needs one global slot (Task 141).
- **Reset lists are scattered.** `full_reset` (`mod.rs:602-640`) and `soft_reset`
  (`mod.rs:718+`) are flat lists. Kitty clears the keyboard flags and extra cursors on DECSTR
  too; freminal documents not doing so for keyboard.
- **The tmux passthrough re-implements CSI dispatch** (`terminal_handler/dcs.rs:228-522`).

**Scope sketch.**

- A `ScreenScoped<T>` holder with idempotent enter/leave.
- A per-protocol `reset(kind: ResetKind)` with `ResetKind { Hard, Soft }`.
- Migrate the keyboard stack and graphics placement maps.
- Fix B15 and the RIS `kitty_state` leak.
- An e2e `?1049`/`?47` test suite.

**Open questions.**

- DECSTR behaviour per protocol: follow kitty (clear keyboard flags and extra cursors) or
  stay conservative.
- Whether to unify the tmux passthrough CSI path with `AnsiCsiParser` (changes reply
  wrapping and ordering; maintainer decision).

### Task 132 — Colour Foundation (stub)

**Goal.** A spec-complete colour-spec parser, a reusable SGR-style colour extractor, and
dynamic colours that actually render.

**Audit findings.**

- **`parse_color_spec` format coverage.** It accepts only `rgb:`, `#RGB` and `#RRGGBB`, with
  a case-sensitive `rgb:` prefix. It lacks `#RRRGGGBBB`, `#RRRRGGGGBBBB`, `rgbi:`, `@alpha`
  and X11 colour names. (The panic itself is fixed in 126.1.)
- **`#RGB` expansion.** Freminal expands `#3a7` to `33aa77` (`r*17`). The spec says digits
  are the most significant bits (`#3000a0007000`). Kitty's implementation duplicates digits,
  so kitty's code disagrees with kitty's own spec text.
- **OSC 10/11 "set" never renders.** The override is read only by OSC 10/11 queries
  (`terminal_handler/mod.rs:254-264`; not in `snapshot.rs`).
- **OSC 4 changes apply only to later `38;5;n`.** SGR 30–37/90–97 resolve named colours from
  the theme at render time, and existing cells keep resolved RGB. Kitty and xterm look colours
  up at render time, so palette changes and pops recolour existing text.
- **The 256-colour default table is implemented twice and disagrees at index 16**
  (`colors.rs:95-159`).
- **Kitty unicode-placeholder ids may be decoded from theme RGB.** `handle_sgr` resolves
  `PaletteIndex` to RGB before the cell, so placeholder ids sent as `38;5;n`/`58;5;n` may be
  decoded from the theme's RGB (verify; Task 136 consumer).
- **No reusable `2:r:g:b` / `5:i` colour extractor.** SGR's is welded to
  `TerminalOutput::Sgr`. Task 103 needs one.

**Scope sketch.**

- `ParsedColor { rgb, alpha }` parser parity, case-insensitive.
- An extracted `parse_sgr_color(&[Option<usize>]) -> Option<TerminalColor>`.
- A dynamic-colour struct (fg, bg, cursor, cursor_text, selection fg/bg, visual_bell) carried
  in the snapshot and consumed by the renderer.
- Palette lookup at render time: index-aware cells.
- Dedupe the 256-colour table.
- Damage classification per `freminal-damage-model`: a palette change is genuinely global.

**Open questions.**

- `#RGB` semantics: spec text or kitty/xterm practice.
- Whether cells store a palette index (render-time lookup) or resolved RGB plus a repaint on
  palette change. This affects compact-row storage (Task 118 format).
- Whether `oklch()`/`lab()` are accepted in OSC 21.

---

## Shipped-protocol conformance stubs (v0.13.0 / v0.13.1)

### Task 134 — Unicode Width & Segmentation Conformance (stub, v0.13.1)

**Goal.** Ordinary text is split into cells exactly per kitty's algorithm ("The algorithm for
splitting text into cells", text-sizing spec). The buffer's stored cell width becomes the
single authority. This fixes ordinary-text bugs, and it is the foundation Task 104 needs.

**Audit findings (divergences D1–D12).**

- **D1, segmentation is per data chunk.** Segmentation runs inside one `TerminalOutput::Data`
  chunk (`tchar.rs:108-119`). A combining mark, VS15/VS16, ZWJ continuation or SARA AM
  arriving in a later read or after an SGR starts its own cell.
- **D2, zero-width graphemes take a column** (`row.rs:818` `.max(1)`).
- **D3, invalid UTF-8 drops the whole chunk** (`mod.rs:802-804`); kitty substitutes U+FFFD
  per maximal subpart. A grapheme over 16 bytes (`TChar` cap) drops the chunk the same way.
- **D4, a lone Regional Indicator gets width 1**; kitty gives 2.
- **D5, a standalone emoji modifier gets width 2**; kitty treats it as a zero-width mark.
- **D6, non-RGI ZWJ sequences** may sum to width 3.
- **D7, width 3 exists** (U+17D8); kitty never exceeds 2.
- **D8, `unicode-width` script ligature special cases** diverge.
- **D9, emoji wide-set edge differences.**
- **D10, explicit width cannot be expressed** (needed by OSC 66 `w=`).
- **D11, unlisted C0 controls and noncharacters become cells.**
- **D12, with DECAWM off, all remaining text is discarded** instead of overwriting the last
  column (`buffer/mod.rs:879-888`, pinned by `insert_text_no_auto_wrap_discards_excess`).
- **Width is recomputed from `TChar` at about 14 sites in two crates.** Three independent
  width implementations exist in the GUI crate alone.

**Scope sketch.**

- A stateful previous-cell attach in the buffer insert path.
- U+FFFD substitution; control and noncharacter filtering.
- Width stored on the cell (natural vs explicit, as an enum).
- Remove the recompute sites (`coords.rs`, `view_state.rs`, `search.rs`, `shaping.rs`).
- A vendored `GraphemeBreakTest.txt` plus a differential test.
- Parser, buffer and `build_snapshot` benchmarks before and after.

**Durable decisions.**

- The width algorithm lives in **one replaceable module**, because the upstream algorithm is
  volatile (#8533). Do not hand-copy kitty's tables without a conformance test.
- The cell's wide-char role becomes an enum: it replaces the `is_wide_head` /
  `is_wide_continuation` bool pair, which can currently represent an impossible state
  (`cell.rs:208`).

**Open questions.**

- Wrap `unicode-width` with overrides, or generate a kitty-identical table.
- Fix D12 (DECAWM-off overwrite) in this task: it changes ordinary-text behaviour and a test
  pins the current behaviour.
- The Unicode version pin.

### Task 135 — Graphics Protocol Conformance (stub, v0.13.1)

**Goal.** Fix every protocol-level graphics deviation that does not need the placement-model
rewrite.

**Audit findings (items not covered by 126.2 or Task 136).**

- **Responses.** Freminal replies to commands carrying neither `i` nor `I`; kitty is silent
  (`graphics_kitty.rs:1450-1454, 1526-1538, 2528-2539`). Parse errors get no response.
  `i`+`I` together is not rejected with `EINVAL`. `r=` is not echoed on `a=f`/`a=a`. The
  last chunk's `q` is ignored. The error codes `EPERM`, `EIO` and `ENOTSUP` are invented,
  while `EBADF`, `EFBIG`, `ENOSPC`, `ENOMEM` and `EILSEQ` are missing.
- **Ids.** Auto-assigned ids come from a process-global counter shared with sixel, iTerm2 and
  all panes, so they can collide with client ids (`image_store.rs:21,49`). Re-transmitting an
  existing id does not delete its placements.
- **Transmission.**
  - `a=q` checks only the format and never tries the medium.
  - `S=`/`O=` are ignored for files.
  - A shm name without a leading `/` is accepted.
  - Limits are absent: unbounded inflate, unbounded chunk accumulation, a client-supplied
    `S` trusted as capacity, and no 10000 px / 400 MB limits.
  - A chunked `a=f` continuation carrying `a=f` (required by spec) restarts the transfer
    (`graphics_kitty.rs:271-285`; the test uses a non-conformant shape).
- **Animation.**
  - An `a=f,r=N` edit does not use the existing frame as its canvas.
  - `a=a`/`a=c` ignore `I=`.
  - A forced current frame (`c=`) re-snaps every tick.
  - Gapless frames show for 40 ms.
  - `s=1` doesn't reset loops.
- **Deletes.**
  - `d=a/A` wipes virtual placements and all relative-placement records.
  - `d=n/N` ignores `p` and the "newest only" rule.
  - `d=f/F` deletes all frames instead of frame `r`.
- **Window size.**
  - `CSI 14 t` reports the OS window's outer rectangle (`rendering.rs:412-434`;
    `window_manipulation.rs:236-237` maps `14` to the window rather than the text area).
  - TIOCGWINSZ pixel size is 0 at first spawn and after DECCOLM.
- **Tests that enshrine deviations** must be rewritten: `kitty_delete_all_clears_virtual_placements`,
  `kitty_animation_frame_chunked_transfer_reassembles`, and
  `kitty_query_*` (format only).

**Scope sketch.** Work items G1, G2 (the remainder after 126.2), G3, G7, G8, G10 and G11 from
the audit:

- a typed `KittyGraphicsError` with a wire-code `Display`;
- id-model separation;
- `a=q` running the full load path without storing;
- limits via the Task 129 assembler;
- animation and delete parity;
- `CSI 14 t` semantics;
- a conformance suite ported from kitty's `kitty_tests/graphics.py`.

**Open questions.**

- Silence for id-less commands: follow kitty's code (silent) or the spec's "when specifying
  an id" reading; the two agree.
- `c`+`r` given together: stretch (kitty code; freminal today) or letterbox (spec prose).

### Task 136 — Graphics Placement Model & Z-Layers (stub, v0.13.1)

**Goal.** Give kitty placements a first-class model, independent of text cells, so that
text and images coexist, erases leave images alone, z-index layering works and placements
size themselves per placement.

**Audit findings.**

- **Text and erase destroy tiles.** Writing text over a kitty tile removes it, and
  EL/ED0/ED1/ECH/ICH/DCH erase tiles (`buffer/mod.rs:925-952`, `erase.rs`, `lines.rs`). The
  spec says other erase commands must not affect graphics.
- **Placement pre-clears unrelated images.** Placing an image clears image cells in the
  overlapping columns of **every row below the cursor** (`images.rs:219-244`).
- **No z-layering.** Negative z is not drawn under text, and `z < -1,073,741,824` is not
  drawn under backgrounds: images are always drawn last (`gpu.rs:767-787`).
- **Overlapping placements cannot coexist**: there is one `ImagePlacement` per cell.
- **Display size and mode are shared per image**, not per placement
  (`image_store.rs:158-168`).
- **Missing `c` or `r` is not derived from the aspect ratio.**
- **Cursor after placement** goes to column 0 of the row below. The spec and kitty move it
  right by the image's columns and down by rows−1.
- **Relative placements** only partially follow their parent:
  - real-parent children are stamped once and don't follow parent moves;
  - lowercase deletes don't cascade;
  - cascades work per image id rather than per placement;
  - re-putting a child leaves stale cells.
- **Unicode placeholders.**
  - The image is stretched rather than aspect-fit and centred.
  - Placement id 0 does not match any virtual placement.
  - 16-colour ids are not accepted.
  - The inheritance state is not cleared on cursor motion.

**Scope sketch.** Work items G4, G5, G6 and G9:

- a placement store with stable `RowNumber` anchors (Task 125 `RowStore`);
- a snapshot placement list (sparse);
- `build_image_verts` consuming placements;
- three render passes (below-background, below-text, above-text);
- margin-aware scroll containment.

**Durable decisions.**

- The snapshot carries a **sparse placement list**, not a per-cell parallel `Arc<Vec<…>>`.
- Every placement mutation bumps `row_epochs` for every row the placement covers. Damage is
  classified per `freminal-damage-model`; a placement change is BOUNDABLE-NOW over its
  covered rows.
- Mandatory benchmarks: `build_snapshot` and vertex build.

**Open questions.**

- Whether sixel and iTerm2 images move onto the same placement model or stay cell tiles.
- Reflow behaviour of placements.

### Task 137 — Keyboard Protocol Conformance (stub, v0.13.1)

**Goal.** The bytes emitted for every key event match kitty's `key_encoding.c` for every flag
combination, in legacy mode as well.

**Audit findings (B1–B18).**

- **B1 (high), Alt and Super never apply to text keys** (egui delivers `Event::Text`).
  Alt+b, Alt+f and Alt+Backspace are broken in legacy mode too.
- **B2 (high), modified Enter is dropped entirely** (`gui/terminal/input.rs:1894-1899`).
- **B3 (high), Enter/Tab/Backspace/Escape carry no modifiers.** Shift+Tab sends HT instead of
  `CSI Z`.
- **B4 (high), key identity comes from typed text.**
  - Non-ASCII text is emitted as one `CSI u` per UTF-8 byte.
  - Shifted symbols lose Shift and use the wrong code.
  - Caps Lock is inferred as Shift.
  - Text-key repeat is reported as press.
- **B5, the flag-4 shifted key is always emitted.** The base-layout key is faked, and the
  shifted table is hard-coded US-QWERTY. Tests pin the wrong output.
- **B6, `Ctrl(c)` hard-codes modifier 5.**
- **B7, Ctrl+digit and Ctrl+punctuation** emit raw C0 bytes under flag 1, or are dropped.
- **B8, unmodified F1–F4 and DECCKM arrows** keep SS3 under flags 1 and 8.
- **B9, Enter/Tab/Backspace emit release/repeat without flag 8.**
- **B10, flag 2 alone is inert.**
- **B11, keypad regressions** from the raw intercept: NumLock-off navigation keys,
  text-producing keypad keys sent as `CSI u`, and DECKPAM ignored.
- **B12, F13–F35 are unreachable from the GUI.**
- **B13, legacy modified F3 is sent as `CSI 1;mR`.**
- **B14, modifier-key events** likely carry stale modifier bits on Wayland.
- **B15, double alt-screen entry loses the main stack** (fixed in Task 131).
- **B16, the release path reconstructs inputs lossily.**
- **Missing keys:** hyper/meta bits and keys, AltGr (ISO_LEVEL3), and six media keys.
  These are available as winit logical keys.
- **Lock bits remain reverted** (114.11). winit still exposes no lock state (PR #4244 is
  open).

**Scope sketch.** Work items K0–K14:

- K0, a characterisation harness first: synthetic egui events across a flag matrix, with
  golden vectors from `key_encoding.c`;
- K1, a key-event model;
- K2–K11, fixes to the encoder and the GUI layer;
- K12, docs;
- K13, IME, coordinated with Task 88;
- K14, lock-key events (the event half only).

**Hard gate: architecture sign-off required before decomposition.** The fix needs key
identity at the GUI boundary (logical key without modifiers, text with all modifiers,
location, physical key). There are two options:

- (a) widen the Task 114 raw-winit intercept to all key events, reversing a Task 114 durable
  decision;
- (b) stay on egui 0.36 (`Event::Key.physical_key`) plus a small windowing side-channel.

This is a `freminal-architecture` and `autonomy-boundaries` stop.

**Open questions.**

- (a) vs (b) above.
- Ctrl+`-` maps to 0x1F (xterm) or `-` (kitty spec).
- A macOS option-as-alt config key.
- DECSTR clearing the flags (Task 131).
- Shipping CapsLock/NumLock/ScrollLock key events under flag 8 without the lock bits.

### Task 138 — Desktop Notifications (OSC 99) Conformance (stub, v0.13.0)

**Goal.** Close every OSC 99 deviation, so freminal matches kitty's notification semantics.

**Audit findings.**

- **Parse and reassembly.**
  - `a=` is not cumulative over the default `{focus}`, so `a=report` disables focus
    (test-pinned).
  - Chunk metadata is replaced by the final chunk's defaults.
  - Id-less `d=0` chunks are dropped.
  - Unknown `p=` payloads are appended to the title (test-pinned).
  - Chunked base64 is decoded per chunk (Task 129).
  - Empty notifications are displayed.
  - C1 bytes are not rejected.
- **Reverse path.**
  - Buttons are reported **0-based**; the spec says 1-based.
  - The close report is lost after activation (notify-rust `FnOnce`).
  - `p=?` is answered after DA1 (Task 130).
  - Capabilities are advertised while notifications are disabled.
  - The `p=alive` map is global across panes, unbounded and never pruned on user dismissal.
- **Lifecycle.**
  - No update-in-place for a repeated `i=`.
  - `p=close` does not close the notification.
  - `a=focus` is never acted on.
  - There is no robust `w=` expiry.
  - One thread per notification.
- **Presentation.**
  - The 8 mandatory icon names are not mapped.
  - Data beats name; the spec says name first.
  - `s=silent` becomes sound-name "silent".
  - `o=` is OS-window-only, not pane-aware.
  - `t=` and `f=` filtering is never consumed.

**Scope sketch.** Work items A-T1 to A-T4 from the audit: parse fidelity, reverse-path
correctness, lifecycle (retain the notify-rust handle; a single listener), and presentation.
User filtering goes through `freminal-config-options`.

**Open questions.**

- Whether the toast leg gains buttons and expiry.
- Platform parity for macOS and Windows activation reports: implement, or stop advertising
  them there.

### Task 139 — Pointer Shapes (OSC 22) Completion (stub, v0.13.0)

**Goal.** Full OSC 22: set (`=`), push (`>`), pop (`<`), query (`?`), per-screen stacks.

**Audit findings.**

- **Prefixed forms are not parsed.** Only plain `OSC 22;<name>` works. `=`, `>`, `<` and `?`
  are fed into the name lookup and reset the shape to `Default`. A query gets no reply.
- **No stack**, and no main/alternate separation.
- **`Display` emits non-spec names** (`col-resize`/`row-resize` instead of
  `ew-resize`/`ns-resize`).
- **Unknown names reset the shape**; kitty ignores them.
- **Legacy alias set:** about 40 X11 names in kitty versus about 7 in freminal.
- **No coverage-doc row.**

**Scope sketch.**

- Typed `PointerShapeOp { Set, Push, Pop, Query }`.
- `PointerShapeStack` (capacity ≥ 16, evict the bottom) registered as screen-scoped
  (Task 131).
- Query replies via `write_osc_response`, including `__current__`, `__default__` and
  `__grabbed__`.
- Canonical names; the snapshot carries only the stack top.

**Open questions.**

- `__default__`/`__grabbed__` reporting policy.
- Adopt kitty's full legacy alias table.

### Task 140 — Underline & SGR Parity (stub, v0.13.0)

**Goal.** Close the small SGR and underline gaps.

**Audit findings.**

- **SGR 21 means bold-off; kitty means double underline.** `SGR.md` marks it ✅.
- **`4:N` with N>5 clears the underline**; kitty clamps to dashed (test-pinned).
- **SGR 221/222** (independent bold-off and faint-off) are missing.
- **Underline geometry.** Double and curly lines are unclamped to the cell box, and the
  dot/dash phase restarts per text run. Geometry is untested.
- **The default underline colour is wrong under SGR 7 + DECSCNM.**
- **`Su`/`Smulx`/`Setulc` are reachable only via XTGETTCAP**; they are absent from
  `res/freminal.ti`.
- **Erase copies the whole format tag into blank cells**, not just the background colour.
  Verify against xterm/kitty before treating it as a bug.

**Scope sketch.** Work items A.1–A.4 from the audit, plus SGR 221/222 and an SGR 21
behaviour change. Underline vertex tests and pixel goldens. Terminfo additions plus
`terminfo.tar` rebuild.

**Open questions.**

- SGR 21 → double underline: a behaviour change, maintainer sign-off required.
- The erase-format (BCE) question.

---

## New-protocol stubs (v0.13.2 – v0.13.4)

### Task 141 — Misc Protocol Extensions (stub, v0.13.2)

**Goal.** Implement every item on the misc-protocol page.

**Audit findings.** All of the following are missing:

- **Bare XTSAVE/XTRESTORE (`CSI ? s`/`CSI ? r`)**, plus the explicit `CSI ? Pm s/r` xterm
  form. Kitty's side-effect-free set: LNM, IRM, DECARM, 2004, 1004, 2031, 2048, 5522,
  DECCKM, DECTCEM, DECAWM, mouse tracking and encoding, DECSCNM. There is one global slot.
- **`CSI 22 J`** (move screen to scrollback, then ED 2; main screen with full margins only).
  `EraseDisplayMode` has no variant for it. **Task 103 needs it**, because ED 22 clears extra
  cursors.
- **Mouse-leave report.** `CSI < 288;x;y M` is sent when the encoding is SGR-pixel, on window
  leave and pane-to-pane transitions. Bit `1<<8 | MOTION` per kitty `mouse.c`; the docs'
  "eighth bit" wording is ambiguous.
- (SGR 221/222 belong to Task 140.)

**Seams.**

- Mode state is split three ways: `TerminalModes`, handler-owned modes and buffer-owned
  modes. XTSAVE must snapshot all three consistently.
- The tmux-passthrough ED dispatch needs `22` as well.

**Open questions.**

- Whether mouse-leave also requires a tracking mode other than none (kitty doesn't check).
- How the explicit-list XTSAVE form handles DECOM and DECCOLM side effects.

### Task 142 — Unscroll `CSI Ps + T` (stub, v0.13.2)

**Goal.** Pull `Ps` lines back from scrollback into the top of the screen, filling with blank
lines when scrollback is empty or on the alternate screen.

**Seams.**

- `RowStore::pop` is documented as popping blank padding only.
- Stable-`RowNumber` anchors (selection, search, command blocks, placements) shift.
- Scrollback rows may be compressed.
- Cursor buffer-row accounting changes.
- `row_epochs` damage over the shifted rows.

**Open questions.** Behaviour with DECSTBM/DECLRMM margins: the spec is silent, and kitty's
behaviour looks like a quirk. This is a maintainer decision; do not guess terminal semantics.

### Task 143 — DECCARA / DECSACE (stub, v0.13.2)

**Goal.** Kitty's extended DECCARA.

- `CSI Pt;Pl;Pb;Pr;<SGR…> $ r` applies **any** SGR (colours, underline styles, colon forms) to
  a rectangle or stream extent.
- `CSI Ps * x` (DECSACE) selects the extent: 2 = rectangle, 0/1 = stream.
- DECOM-relative; visible screen only.
- Parameterless `CSI $ r` is a full-screen SGR reset (kitty).

**Seams.**

- A `Buffer` method taking a rectangle, an extent and a `FnMut(&mut FormatTag)`.
- `Cell::set_tag`.
- Reuse `apply_sgr` (`terminal_handler/sgr.rs:178`).
- Compact-row storage (Task 118).
- `row_epochs` bumps: region-bounded damage.
- Advertise DA1 feature 28 once implemented.

DECRARA is out of scope; kitty does not implement it.

### Task 144 — Color Control (OSC 21) & Colour Stack (stub, v0.13.2)

**Goal.** Full OSC 21 and the colour stack.

- **OSC 21 keys:** foreground, background, selection_background/foreground, cursor,
  cursor_text, visual_bell, transparent_background_colorN, and palette indices 0–255. Each
  supports `?` queries and the three-state set/empty/bare value. `unknown=` replies are
  unpadded base64.
- **Colour stack:** `CSI Ps # P` / `# Q` / `# R` (XTPUSHCOLORS/XTPOPCOLORS/XTREPORTCOLORS)
  and kitty's `OSC 30001`/`30101`. Depth 10, per window (not per screen). The reply is
  `CSI idx;count # Q`.
- **Saved state** covers dynamic colours, `cursor_text`, `visual_bell`, transparent-bg
  overrides and the 256-colour palette.

**Depends on.**

- Task 132: rendered dynamic colours and render-time palette lookup. Without them a pop does
  not recolour existing text.
- Task 128: `#` routing.

**Open questions.**

- `transparent_background_color1..7` (spec table) versus `1..8` (kitty code).
- Whether per-colour background opacity is in scope or split out.

### Task 145 — Kitty Shell-Integration Compatibility (stub, v0.13.2; decision-gated)

**Goal.** Accept the shell-integration escape codes that kitty's own scripts, fish 3.8+ and
nushell emit.

**Audit findings.**

- **`kitty-shell-cwd://host/path` OSC 7 is rejected**, with a warn on every prompt
  (`shell_integration.rs:25`). Kitty accepts both schemes.
- **`OSC 133;k;…`** (bash-kitty integration, six per prompt) warns on every prompt.
- **Untagged OSC 133 A/C/D is dropped by design.** The same goes for `k=s` secondary
  prompts, `cmdline=`/`cmdline_url=`, `redraw=`, `special_key=` and `click_events=`. The
  `freminal=1; fid=` gate is recorded in `DESIGN_DECISIONS.md:837-848`.
- **Click-to-move-cursor-at-prompt is missing.**

**Gate.** Accepting untagged OSC 133 reverses a recorded design decision and rewrites tests
that pin the drop, so it needs maintainer approval before decomposition. The OSC 7 scheme fix
and the silent consumption of `133;k;…` are independent and decision-free; they may be pulled
forward into v0.13.0 as cleanup.

### Task 103 — Multiple Cursors (stub, v0.13.2; previous breakdown superseded)

**Goal.** Implement the multiple-cursors protocol: `CSI > SHAPE ; (TYPE:COORDS)* SP q`.

**Spec surface.**

- Shapes: 0 clear, 1 block, 2 beam, 3 underline, 29 follow-main.
- Coordinate types: 0 main, 2 `y:x` points, 4 `top:left:bottom:right` rectangles (no numbers
  means the full screen).
- Colours: 40 cursor, 30 text-under-cursor. Colour spaces: 0 unset, 1 special
  (reverse-video), 2 sRGB, 5 indexed.
- Queries and replies:
  - support: `CSI > SP q` → `CSI > 1;2;3;29;30;40;100;101 SP q`;
  - cursors: `CSI > 100 SP q`;
  - colours: `CSI > 101 SP q`.
- Extras share the main cursor's blink and opacity, are **not** hidden by DECTCEM (kitty
  0.49 fix), do not scroll, and clear on ED 2/3/22, reset and alternate-screen switch.
- Stable since the warning was removed on 2026-01-08.

**Corrections to the previous breakdown.**

- The `q` dispatch collision is handled by 126.3 and 128.
- **103.3 is much larger than "iterate in `build_cursor_verts_only`".** It needs:
  - a variable-length cursor region (**delivered by 127.3/127.4**);
  - a text-under-cursor colour capability in the foreground pass;
  - change detection: an extras epoch;
  - BOUNDABLE-NOW damage via `PaneDamageRect::from_cursor_cells`;
  - its own visibility, scroll and fold gating.
- No reusable `2:r:g:b`/`5:i` colour parser exists (Task 132).
- `CSI 22 J` is a prerequisite (Task 141).
- The state lives in a new handler module, not inline in `terminal_handler/mod.rs`.
- Indexed colours are resolved at snapshot-build time.
- Query-100 replies are emitted in row-major order, for determinism.

**Open questions.**

- Type-4 rect with 1–3 numbers: the spec says ignore; kitty treats it as the full screen.
- Colour component outside 0–255: clamp or reject (kitty masks).
- DECSTR clears extras (kitty does).
- Extras while scrolled back: hide, or draw at screen coordinates.
- Extras in inactive panes: follow Task 127's hollow rule, or hide.
- Resize: clip out-of-range extras.

### Task 133 — Shared Consent Prompt (stub, v0.13.3)

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

### Task 104 — Kitty Text Sizing (OSC 66) (stub, v0.13.3; previous breakdown superseded)

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

### Task 102 — Kitty File Transfer (OSC 5113) (stub, v0.13.3; previous breakdown superseded)

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

### Task 146 — Kitty Clipboard (OSC 5522) (stub, v0.13.3)

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

### Task 105 — Kitty Drag & Drop (OSC 72) (stub, v0.13.4)

**Goal.** Implement the drag-and-drop protocol: accepting drops (including remote machines
and remote directory reading), starting drags (including to remote machines), the `t=q`
detection query, multiplexer behaviour and the machine-id.

**Status change (2026-10-08).** The spec was marked stable upstream on 2026-06-12 (commit
`8be4ea67`, "Mark dnd protocol as stable", kitty #9984 closed). Later edits are
security-driven (0.49.0: data requests before a drop must fail with `EPERM`; session data
is discarded on leave) or informational. The protocol therefore meets the "current, stable"
criterion and moved here from the retired `PLAN_VERSION_DND.md`.

**Blocker (local, not spec).** winit 0.30.13 exposes only `DroppedFile`, `HoveredFile` and
`HoveredFileCancelled`. It has no hover position, no MIME list, no drag source and no lazy
data. Activation requires one of:

- a winit upgrade (0.31 is still beta and its DnD API is unverified);
- per-platform integration in `freminal-windowing`, which likely needs FFI. `unsafe` needs
  explicit approval.

That is an architecture decision, made at activation.

**Durable constraints (carried over).**

- Reverse-write and consent reuse Tasks 130 and 133; no new mechanisms.
- **Security is high-stakes.** Refuse to serve data for a drag originating in the same window
  as the drop target. Enforce POSIX error replies (`EPERM`, `EMFILE`, `ENOMEM`, `EFBIG`)
  during remote directory traversal. HMAC-SHA256 the machine-id.
- The consent overlay follows `freminal-modal-input-suppression`.
- Dual-doc update on implementation.

---

## Design decisions

- **Spec first, kitty code as tie-breaker.** Where the spec is silent or self-contradictory,
  follow kitty's reference implementation, and record each case in the owning task (for
  example: graphics `c`+`r` stretch, `d=f` per-frame, mouse-leave bit 288). Where the
  spec is explicit, follow the spec even if kitty's code differs, unless the maintainer
  decides otherwise (e.g. `#RGB` expansion, Task 132).
- **Foundations before consumers.** Tasks 128–134 exist because their seams are shared by
  three or more protocols. A protocol task does not re-implement a foundation locally.
- **Snapshot additions are sparse lists.** Extra cursors, graphics placements and text-sizing
  blocks are carried as `Arc<[…]>` lists with generation caching (`command_blocks_cache`
  precedent). They are never per-cell parallel vectors like today's
  `visible_image_placements`. Coordinate any concurrent snapshot-shape change.
- **No payload ever reaches a log at warn level.** Clipboard, transfer, notification and
  graphics payloads may carry secrets. Unknown-sequence logging records length and
  introducer only (Task 129).
- **Capability answers are truthful and ordered.** Every capability query reply is computed
  on the PTY thread from the actual enabled feature set, before any following DA1 reply.
- **Consent is never bypassed by default.** OSC 5113 `pw=` and OSC 5522 `pw=`/`name=` allow
  bypass only when the user has configured it; OSC 72 always asks.
- **The unscaled-text fast path must not regress.** Tasks 104, 134 and 136 carry mandatory
  before/after benchmark captures (15% threshold).
- **New state is typed.** Every new mode, flag or focus value is a named enum
  (`freminal-state-representation`): no new `bool` fields or parameters.
- **TENTATIVE items need sign-off.** `?5522` paste events (Task 146), the keyboard
  architecture choice (Task 137), the untagged OSC 133 policy reversal (Task 145) and the
  SGR 21 behaviour change (Task 140) are not decomposed until the maintainer decides.
