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
subtask breakdown: **126**, **127**, **128** and (activated 2026-10-09) **129**. Everything else is an **enriched stub**:
goal, audit findings, durable decisions and open questions, with no subtasks. Tasks 102, 103
and 104 were previously decomposed. The audit found factual errors in those breakdowns (for
example, OSC 66 is not a Contour code, and the 102 wire keys were wrong), so they are now
enriched stubs again and get re-decomposed at activation.

---

## Task Summary

| #   | Task                                        | Milestone | Scope | Status        | Depends on                         |
| --- | ------------------------------------------- | --------- | ----- | ------------- | ---------------------------------- |
| 126 | Pre-existing Safety Gate                    | v0.13.0   | S     | Complete      | None                               |
| 127 | Unfocused / Inactive-Pane Cursor (#531)     | v0.13.0   | M     | Complete      | 126 (schedule only)                |
| 128 | Prefix- & Intermediate-Aware CSI Dispatch   | v0.13.0   | M     | Complete      | 126.3                              |
| 129 | Kitty Wire Infrastructure                   | v0.13.0   | L     | Pending merge | None                               |
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

**Status: Complete (2026-10-09).** `UnfocusedCursorStyle { Hollow, Unchanged, Hidden }`
(snake_case, default `Hollow`) and `CursorConfig::unfocused_style`. `ConfigPartial` carries
the whole `[cursor]` section, so no partial wiring was needed; a test confirms
`apply_partial` applies it. `config_example.toml` and the home-manager module are updated.
Six tests: round trip, defaults, missing key, old config, partial, unknown value. Commit
`98101fbe`.

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

**Status: Complete (2026-10-09).** Implemented as specified, plus `CursorVisibility` and
`EchoState` (no echo-off enum existed). A private `steady_variant` maps blink styles to
steady ones. An exhaustive 216-case table test and four `cursor_focus` tests. Items are `pub`
inside the crate-private module because `clippy::redundant_pub_crate` is denied;
`cursor_focus` is `pub(super)` because `PaneFocus` is. Commit `9828fb77`.

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

**Status: Complete (2026-10-09).** `CursorDrawParams { appearance, col, row, color, x_scale,
blink_on }`, `CursorBlinkPhase { On, Off }` (with `from_blink_on` as the single bool→enum
boundary) and `CursorVertRange { start, len }`. `col`/`row` are the trail-animated visual
cell coordinates; the pixel origin uses the same float order as `frame_dirty.rs`, and `Solid`
output is pinned bit-identical to a pre-refactor capture for all six styles at `x_scale` 1
and 2. Hollow thickness is `max(cw * 0.1, 1).round()` where `cw` is the drawn width
(including `x_scale`). `BackgroundFrame.cursor` borrows the params. Both the
`too_many_arguments` and `struct_excessive_bools` allows are gone. **Maintainer-approved
scope change:** `gui/terminal/mod.rs` declares `pub mod cursor_appearance` so the public
bench can name `CursorAppearance`; the 127.2 `expect(dead_code)` gates were removed and
`#[must_use]` added in the same commit. Benchmarks, measured back to back against the
parent commit with the other worktree idle (the first captures were taken under load and
discarded): `instanced_bg` and `instanced_bg_partial_dirty` moved between 0% and +2.4%, all
within the 15% threshold. Commit `f26c722d`.

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

**Status: Complete (2026-10-09).** Plan drift: the offset lived on `RenderState`
(`cursor_vert_float_offset`), not `PaneRenderCache`; `RenderState` was treated as the cache.
It is now `cursor_vert_range`, and `patch_cursor_only_deco_verts` truncates to `start` and
appends, returning the new range. The GPU upload already uploads `deco_verts` at its current
length, so no GL change was needed. The four old zero-in-place tests were replaced, and
rebuild-equivalence and #432-style tests added. `headless_workloads.rs` is unchanged.
Commit `d29e1011`. The empty-prefix regression this introduced was caught in review and
fixed (see "127 Review").

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

**Status: Complete (2026-10-09).** `show()` takes `window_focus: WindowFocus`.
`effective_show_cursor` is replaced by the resolved `CursorAppearance`, which feeds change
detection (`CursorFrameInputs.appearance`, `PaneRenderCache.previous_cursor_appearance`) and
both draw paths. The fold/off-screen AND maps a visible appearance to `Hidden`. Non-focused
cursors use a constant blink phase. No new damage category. **Maintainer-approved scope
changes:** `WindowFocus` is `pub` in `gui/frame_drain.rs` (a `pub` method's parameter), and
`unfocused_cursor_style` is cached on `FreminalTerminalWidget` (set in `new()` and both
config-apply methods). `frame-profiling` tests pass. Plan drift: `CursorFrameInputs` is in
`frame_dirty.rs`. Commit `a9f75418`.

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

**Status: Complete (2026-10-09).** `cursor_blink_wants_repaint(&CursorAppearance,
CursorFocus)` replaces the three bool parameters. `ViewState::snap_cursor_animation` jumps
the trail to its target and clears the last-frame timestamp, and `frame_dirty.rs` calls it
for non-focused panes (`CursorFrameInputs.focus`). `cursor_focus` is not reachable from
`app_impl.rs`, so the call site repeats its rule; see 127.C3. Trail tests use 10 s
durations so they cannot race under load. Commit `63acd823`.

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

**Status: Complete (2026-10-09).** Plan drift: the settings UI is the single file
`gui/settings.rs`. An "Unfocused cursor" combo box (with `.clickable()` affordances and hover
text). `CursorPreview.unfocused_style` and `VisualPreviewDiff.cursor_unfocused_style`;
`apply_preview_cursor` pushes it to every window's widget through
`set_unfocused_cursor_style_preview`. Cancel reverts by previewing the committed config. No
new `SettingsAction`. Commit `90a00d44`.

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

**Status: Complete (2026-10-09).** `CursorPresence::Hollow` in `headless.rs` and
`capture_side_by_side` in `pixel_harness.rs` draw both panes into one framebuffer through
per-pane GL viewports, as the application does. Golden
`cursor_active_solid_inactive_hollow.png` (llvmpipe, LLVM 21.1.8) with exact tolerance,
explicit border/interior pixel assertions, a per-half equals-solo-capture check and a
bit-identical repeat check. Six consecutive `gl-pixel` runs with `FREMINAL_REQUIRE_GL=1`
passed. No CHANGELOG exists. `check-windows` is clean. The implementing sub-agent was cut
off by an infrastructure error; the orchestrator verified and committed its work. Commit
`bc917f34`.

### 127 Review

An adversarial sub-agent review of the full branch **failed** on one MAJOR finding. 127.4's
truncate-and-append patch empties `deco_verts` when the cursor is the only decoration (a
plain shell) on blink-off or DECTCEM hide. `deco_verts.is_empty()` was also the "never built"
damage signal, so every blink-on frame became a full rebuild and `Full` present. Fix commit
`438dcfce`:

- `RenderState::cursor_vert_range` is `Option<CursorVertRange>`, and `None` is the
  never-built signal in all three gates.
- Blink-phase pinning moved into `evaluate_frame_dirty_state` (`effective_cursor_blink_phase`)
  so it is tested.
- The blink re-anchors when focus is regained (`blink_anchor_action`), so a refocused cursor
  does not open on the off half.

Commit `32bffc25` updates the `freminal-damage-model` skill to name the new signal. A
confirmation review **passed**. Remaining findings are 127.C1–127.C3.

### 127 Cleanup entries

#### 127.C1 — New windows start with egui `focused = false`

- **Surfaced:** 127 review (2026-10-09).
- **Impact:** `freminal-windowing` never seeds egui's focus from `window.has_focus()`; egui-winit
  starts `focused: false` until winit delivers `Focused(true)`. A new window can draw a
  hollow, non-blinking cursor for its first frames. Cosmetic.
- **Scope of fix:** `freminal-windowing` (window creation / egui-winit state init).
- **Suggested approach:** seed the focus state from `window.has_focus()` at creation, or
  confirm the focus event always precedes the first redraw on every platform.
- **Verification:** a new window's first frame draws a solid cursor.
- **Scheduling:** independent; outside Task 127's crate scope.
- **Status: Resolved (2026-10-09), commit `f0cfb0e1`.** `EguiState::new` seeds
  `RawInput::focused` from `window.has_focus()` through the public `egui_input_mut()`, via a
  small `seed_initial_focus` helper and an `InitialWindowFocus` enum. The dependency on
  egui-winit behaviour is recorded as assumption A14 in `EGUI_UPGRADE_ASSUMPTIONS.md`. The
  helper is unit-tested; the first-frame visual needs a display and was not checked by
  hand.

#### 127.C2 — `skip_draw` frames record cursor baselines that were never drawn

- **Surfaced:** 127 confirmation review (2026-10-09).
- **Impact:** on a `skip_draw` frame, `widget.rs` still stores `previous_cursor_pos`,
  `previous_cursor_appearance` and `previous_cursor_focus`, though nothing was drawn (the blink
  phase is correctly not stored). A cursor move or focus change during a skipped frame,
  followed by a frame with no content change, can leave the old cursor on screen. Partly
  predates 127 (`previous_show_cursor` behaved the same). Unlikely in practice.
- **Scope of fix:** `freminal/src/gui/terminal/widget.rs` cache-update block.
- **Suggested approach:** store all cursor baselines only on drawn frames.
- **Verification:** a test with a skipped frame carrying a cursor change, then an unchanged
  frame, presents the new cursor.
- **Scheduling:** independent.
- **Status: Resolved (2026-10-09), commit `a9cf78ab`.** `PaneRenderCache::record_cursor_frame`
  takes `CursorFrame::{Drawn(DrawnCursor), Skipped}` and advances all six cursor baselines
  only on drawn frames. A focus regain during a skipped frame still re-anchors the blink on
  the first drawn frame. Tests cover a move, an appearance change and a colour change on a
  skipped frame.

#### 127.C3 — Cursor-focus derivation is duplicated and its `show()` wiring is untested

- **Surfaced:** 127.6 and the 127 reviews (2026-10-09).
- **Impact:** `app_impl.rs`'s blink-wake gate repeats `cursor_focus`'s rule because
  `cursor_focus` and `PaneFocus` are `pub(super)` in `gui::terminal`, and resolves a full
  appearance although the result does not depend on `unfocused_style`. The damage origin
  (`frame_dirty.rs` `cursor_pixel_pos`) and the draw origin (`vertex.rs`) are computed
  separately. The re-anchor/phase wiring inside `show()` has no direct test. No current
  defect; the copies can drift.
- **Scope of fix:** `gui/terminal/input.rs` / `cursor_appearance.rs` visibility, `app_impl.rs`
  blink gate, and possibly a small pure "frame cursor state" function used by `show()`.
- **Suggested approach:** make `cursor_focus` (and `PaneFocus`) `pub(crate)` and call it from
  the blink gate; derive the damage origin from `CursorDrawParams`; extract the per-frame
  focus/appearance/phase derivation so it can be tested.
- **Verification:** one definition of the focus rule; tests on the extracted function.
- **Scheduling:** independent.
- **Status: Resolved (2026-10-09), commits `17fa6df7`, `3fd57ee0`, `3779036a`.**
  `cursor_focus` is the only focus rule; the blink-wake gate calls it (`PaneFocus` is
  crate-visible). `CursorDrawParams::pixel_origin` is the only cursor-origin formula, used by
  drawing and damage; `frame_dirty.rs` no longer computes `cursor_pixel_pos`. The per-frame
  derivation is `frame_cursor_state(&CursorStateInputs)`, with an `ActivationBlinkReset`
  enum, tested directly. After the cleanup review it moved with the blink clock into a new
  `gui/terminal/cursor_blink.rs` (`widget.rs` shrank by about 550 lines). Output is unchanged:
  the bit-identical solid-cursor test passes unmodified.

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

**Status: Complete (2026-10-09).** As designed. Items are `pub` inside the crate-private
module (`redundant_pub_crate`). An out-of-range single intermediate maps to `Multiple`, so no
route accepts it. Twelve table-driven tests. Commit `ff27cb9c`.

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

**Status: Complete (2026-10-09).** DECREQTPARM moved to `csi_commands/decreqtparm.rs`
and `push_split_mode_params` to `csi_commands/dec_modes.rs`, removing the `decrqm` → `csi`
import loop. The dead `>`-intermediate check stayed in the `x` arm for 128.5. Test lists
before and after differ only by module path. Commit `2443e647`.

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

**Status: Complete (2026-10-09).** `csi_dispatch.rs` with the nine group functions
(`plain` is split into two to stay short); the `too_many_lines` allow is gone. Routed keys keep
their old handler, arguments and `ParserOutcome`. Exactly two existing tests changed, as the
plan said. `csi_dispatch_matrix.rs` covers 5 prefixes × 22 intermediate shapes × every final
byte, and after review pins the exact output and outcome for all 49 routed keys. Benchmarks,
back to back against the 128.2 commit with the other worktree idle: plain text +1.3%, CUP
+1.8%, bursty −0.8% (all noise), SGR-heavy +7.0%. The SGR result was investigated at the
maintainer's request. Four interleaved A/B runs gave medians of 79.7 µs before and 79.8 µs
after, with single runs spread from 72 to 117 µs. A `perf` profile showed the CSI parser's
share of samples fell from 17.2% to 12.4% (classify, dispatch and handlers inline into
`ansiparser_inner_csi`), while `memmove` (22–28%) drives the variance. The +7% is bench
noise, not router cost. Commit `1475525e`.

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

**Status: Complete (2026-10-09).** `dispatch_tmux_csi` falls through on any prefix,
intermediate or misplaced marker, using `CsiKey::classify`. After review it is
allocation-free. The stale OSC and CSI doc lines were corrected. A misplaced marker (`1?2H`)
used to dispatch and now falls through, per the plan. Commit `d9feaef8`.

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

**Status: Complete (2026-10-09).** The dead branches in `da.rs`, `decrqm.rs` (`h`/`l` and
the unhandled `else`), `decreqtparm.rs` and `sgr.rs` (`>` delegation) are removed. The
`decrqm` terminator parameter is kept as `_terminator` so the signature is unchanged (the
sub-agent's `debug_assert` was replaced). The `mod.rs` dispatch table is rewritten. Tests for
removed branches were deleted; the `sgr >` tests now drive the full parser. Leftovers are
128.C1 and 128.C2. Commit `c6bb1332`.

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

**Status: Complete (2026-10-09).** Both docs updated as specified. Plan drift: the GAPS
DECSCA claims were at lines ~53, 203, 262, 270 and 460, not 227/386. A DECRARA gap row was
added. Verified with `markdownlint-cli2` (0 issues), `prettier --check` and the pre-commit
hooks; `cargo xtask lint-markdown` remains broken tree-wide (126.C1). Commit `8b093649`.

### 128 Review

An adversarial sub-agent review found no BLOCKER or MAJOR issues: every routed key keeps
its handler, arguments and outcome, and no real-world sequence that used to work is now
dropped. Fix commit `b071cbaf` addressed the in-scope MINORs: the matrix pins the exact
output and outcome for all 49 routed keys (so swapped handlers fail), the tmux guard no
longer allocates, and two rustdoc issues were fixed. A confirmation review **passed**.
Remaining findings are 128.C1–128.C4.

### 128 Cleanup entries

#### 128.C1 — `ParserFailures::UnhandledDECRQMCommand` has no constructor

- **Surfaced:** 128.5 (2026-10-09).
- **Impact:** dead public variant in `freminal-terminal-emulator/src/error.rs:~118`; no lint
  fires because the enum is public.
- **Scope of fix:** `error.rs` (and any match arms naming it).
- **Suggested approach:** delete the variant.
- **Verification:** workspace builds; tests and clippy green.
- **Scheduling:** independent.
- **Status: Resolved (2026-10-09), commit `cee56916`.** Deleted, together with
  `MalformedDECRQMIntermediates`, which 128.C2 also made unreachable.

#### 128.C2 — Vestigial handler parameters left by the strict router

- **Surfaced:** 128 review (2026-10-09).
- **Impact:** `decrqm`'s `_terminator` is always `p`. `da`'s `intermediates` is always empty
  from the router (`greater()` passes `&[]` while `plain()`/`equals()` pass the real slice),
  so its "invalid intermediates" branch is unreachable, and an inner
  `params[0] == b'>'` re-check is tautological. The plan kept handler signatures unchanged
  through 128.5; this is the follow-up.
- **Scope of fix:** `csi_commands/decrqm.rs`, `csi_commands/da.rs`, their call sites in
  `csi_dispatch.rs`, and their tests.
- **Suggested approach:** drop the unused parameters and the unreachable branch; use
  `strip_prefix(b">")`.
- **Verification:** `csi_dispatch_matrix.rs` unchanged and green.
- **Scheduling:** independent; before Task 137 touches the same handlers.
- **Status: Resolved (2026-10-09), commit `cee56916`.** Dropped `decrqm`'s terminator and
  intermediates parameters (the router reaches it only for `$`-`p`), `da`'s intermediates
  parameter and its unreachable "invalid intermediates" branch, and the now-unused
  `intermediates` parameter of `dispatch_csi` and its group functions. `da` uses
  `strip_prefix(b">")`. `csi_dispatch_matrix.rs` passes unchanged.

#### 128.C3 — tmux direct dispatch accepts bodies with non-parameter bytes

- **Surfaced:** 128 review (2026-10-09). Predates Task 128.
- **Impact:** `dispatch_tmux_csi` direct-dispatches a body containing C0 controls or bytes
  ≥ 0x40 before the terminator (e.g. `1H2J`), where the main parser would end or split the
  sequence differently.
- **Scope of fix:** `terminal_handler/dcs.rs` `dispatch_tmux_csi`.
- **Suggested approach:** direct-dispatch only when every body byte is in `0x30..=0x3B`
  (digits, `:`, `;`); otherwise fall through to the re-parse queue.
- **Verification:** new tests for such bodies falling through; ordering test green.
- **Scheduling:** independent.
- **Status: Resolved (2026-10-09), commits `77d6d42b`, `500438e3`.** Deviation from the
  suggested approach: only digits and `;` are accepted, not `:`. The direct handlers do not
  interpret `:`, and the main parser's handlers reject a `:` field, so `:` bodies go to the
  re-parse queue. The cleanup review then found that an all-digit field overflowing `usize`
  was defaulted by the direct path but rejected by the main parser. The direct path now
  parses with the main parser's `split_params_into_semicolon_delimited_usize` and falls
  through on error, and the old `parse_csi_params` helper is gone. Real tmux traffic
  (`CSI r;c H`, `CSI n A`, `CSI K`, `CSI J`) still dispatches directly.

#### 128.C4 — Unrouted CSI is logged at `warn!` (maintainer decision)

- **Surfaced:** 128 review (2026-10-09).
- **Impact:** sequences that used to fail as `InvalidParserFailure` (logged at `debug!`), such
  as XTSMGRAPHICS `CSI ? Pi;Pa;Pv S` or `CSI > Ps t`, now log at `warn!` through
  `warn_unhandled`, and a misplaced private marker is logged with the "valid grammar"
  message. An application that probes on every redraw produces log noise. The level
  follows the plan's "log with `tracing::warn!`" decision.
- **Scope of fix:** `ansi_components/csi_dispatch.rs` / `csi.rs` logging only.
- **Suggested approach:** maintainer decides between keeping `warn!`, lowering to `debug!`, or
  rate-limiting; log `CsiKeyError` with its own message either way.
- **Verification:** log-level test or manual check.
- **Scheduling:** needs a maintainer decision.
- **Status: Resolved by maintainer decision (2026-10-09): keep `warn!`.** No code change.

#### 128.P1 — Per-sequence 8 KB trace buffer in every ANSI sub-parser

- **Surfaced:** the maintainer asked about the SGR-heavy benchmark's memory-copy share after
  128.3 (2026-10-09). Not a numbered cleanup entry; recorded here because it changed the
  parser.
- **Finding:** every escape sequence built a fresh sub-parser (CSI, OSC, DCS, APC,
  ESC-standard) that embedded an 8 KB `SequenceTracer` ring buffer. That meant zero-filling
  8 KB and moving the roughly 8 KB struct into `ParserInner`, about 16 KB of memory traffic
  for a 15-byte `ESC[38;2;r;g;bm`. `perf` put memmove plus memset at 26–36% of samples in
  `bench_parse_sgr_heavy`. The buffer was diagnostics only and duplicated bytes held
  elsewhere: the top-level parser's own tracer (what the `recent=` logs print), CSI's
  `sequence`, and OSC's `params`.
- **Fix (maintainer-chosen option), commits `1f5f7f1c`, `500438e3`:** sub-parsers no longer
  embed a tracer and render diagnostics from bytes they already hold. Log rendering of
  sequence bytes is bounded to 256 bytes (head and tail, with an omitted-byte count) by
  `escape_sequence_for_log_bounded` / `lossy_sequence_for_log_bounded`. OSC sub-dispatch
  takes raw bytes instead of `&SequenceTracer`. `ParserInner` went from about 8.3 KB to
  80 bytes; a size test (`sub_parsers_stay_small`, bound 128 bytes) guards against
  regression. Public API change: the sub-parsers no longer implement `SequenceTraceable`,
  and their `trace_str()` returns the sequence body without the introducer. No in-tree
  caller depended on either.
- **Measured** (interleaved A/B, three runs each, idle machine; before = `77d6d42b`):

  | Bench                           | Before (µs) | After (µs)  | Change |
  | ------------------------------- | ----------- | ----------- | ------ |
  | `parser_push_sgr/4097`          | 85.0–96.5   | 64.2–75.0   | ~−25%  |
  | `parser_push/4096` (plain text) | 22.4–22.7   | 19.1–19.2   | ~−15%  |
  | `parse_and_handle_80x24`        | 65.5–66.6   | 61.7–63.1   | ~−5%   |
  | `bursty_10_small_plus_1_large`  | 166.5–167.6 | 162.5–165.3 | ~−2%   |

- **Remaining:** the SGR profile now shows about 10% memmove and 8% malloc. That is
  ordinary allocation (the CSI parser's three small `Vec`s per sequence, the SGR handler's
  segment `Vec`s, and growth of the output `Vec`), not avoidable copying. Reusing one CSI
  parser's buffers across sequences would remove the per-sequence mallocs. That was the
  larger option the maintainer did not choose.

---

## Task 129 — Kitty Wire Infrastructure

### 129 Summary

Build once the parsing and payload plumbing that every kitty OSC, APC and DCS protocol
re-implements today, and fix the live wire-level bugs the audit found on the way. Activated
and decomposed on 2026-10-09 against `72a5f7e1` (main after PR #535).

### 129 Activation recon (2026-10-09)

Three read-only audits re-traced the stub's findings against the current code. All of them
were confirmed, and the recon added these details:

- **Four hand-rolled `key=value` parsers, four different rule sets.**
  - OSC 99 (`osc_notify_99.rs:409-464`): `:`-separated; the key must be exactly one byte;
    missing `=` is an error.
  - Graphics (`kitty_graphics.rs:395-421`): `,`-separated; the key is the **first byte** of
    the pair (`abc=1` is accepted as `a=1`); an empty value is an error.
  - FTCS (`ftcs.rs:169-243`): gets pre-split, re-stringified OSC tokens; uses positionals.
  - iTerm2 (`osc_iterm2.rs:77-131`): `;`-separated; a pair without `=` is silently skipped.
  - None of them caps the number of pairs.
- **base64** (`freminal-common/src/base64.rs`): `decode(&str) -> Result<Vec<u8>, String>`.
  Padding is optional, there is no length or tail-bit check, a lone trailing character
  decodes to nothing, and the error is an untyped `String`. `encode` always pads.
  - OSC 99 corruption is **confirmed**: `parse_osc_99` decodes each chunk on its own
    (`osc_notify_99.rs:380-395`), so encode-then-chunk input split mid-quantum silently
    produces wrong bytes. The spec allows both chunk-then-encode (each chunk padded) and
    encode-then-chunk (arbitrary split, at most 4096 bytes per chunk).
  - Graphics and iTerm2 `FilePart` also decode per chunk. The graphics spec requires
    non-final chunks to be a multiple of 4, so that is conformant; it is not changed here.
- **Accumulators.**
  - Graphics (`KittyImageState`, `terminal_handler/mod.rs:104`): no byte cap, and
    `Vec::with_capacity(S=)` pre-allocates up to about 4 GiB from a sender-chosen value
    (`graphics_kitty.rs:674`).
  - iTerm2 multipart (`MultipartImageState`, `mod.rs:92`): no byte cap, and
    `Vec::with_capacity(size=)` (`graphics_iterm2.rs:167`) is unchecked.
  - OSC 99 (`notify_99.rs`): capped at 128 pending ids and 1 MiB of decoded bytes per id.
  - Neither graphics nor iTerm2 state is cleared by `full_reset`; that is Task 131's reset
    work and is recorded there.
- **Parsers.** `AnsiOscParser.params`, `ApcParser.sequence` and `DcsParser.sequence` grow
  without bound. `sub_parsers_stay_small` (`ansi.rs:1958`) bounds each sub-parser at 128
  bytes.
- **Trailing backslash.** `AnsiOscParser::push` (`osc.rs:100-108`) pops every trailing
  `0x5c`/`0x07`/`0x1b`, so `OSC 0;C:\ BEL` sets the title `C:`.
- **Tokenise-first dispatch** (`osc.rs:150-176`). Every OSC is split on `;` and each field is
  parsed as a `u16` or a UTF-8 `String` before dispatch, so a non-UTF-8 byte anywhere
  invalidates every OSC, including the raw-payload targets (9, 99, 777, 1337, 1338).
  - OSC 8: `8;;http://a;b` tokenises to four fields, matches neither accepted shape, and
    becomes `UrlResponse::End`, closing the link. The `id` is stored as the whole params
    field (`"id=foo"`) rather than parsed.
  - OSC 0/1/2/7 use only field 1, so a title or URI containing `;` is truncated. Because the
    value goes through `AnsiOscInternalType`'s `Display`, a numeric title `123` becomes
    `Unknown(Some(OscValue(123)))` and a title of `?` becomes `Query`.
- **Logging.** Unknown OSC payloads are logged at warn (bounded, escaped). Unknown DCS
  payloads are logged at warn **unbounded** (`terminal_handler/dcs.rs:53-56`), as are the
  non-kitty APC (`terminal_handler/osc.rs:35-38`), DECRQSS, XTGETTCAP and tmux inner-sequence
  warns. Kitten `@kitty-*` DCS strings hit the unknown-DCS warn in full. `DCS received` is
  logged at debug, unbounded.
- **APC replies.** `format_kitty_response` (`freminal-common/.../kitty_graphics.rs:507-522`)
  hard-codes `ESC _ G … ESC \`; S8C1T is never consulted. `pty_writer.rs` has CSI, DCS and
  OSC introducer helpers but no APC one.

### 129 Decisions (maintainer, 2026-10-09)

- **Parser caps are per target, with a 1 MiB default.** OSC, APC and DCS default to
  `1 MiB`. OSC 52, OSC 1337 and DCS (sixel and tmux passthrough) get `64 MiB`, because a
  large single sequence is legitimate there. An over-cap sequence is consumed through its
  terminator, produces **no** output, and is warn-logged with its introducer, cap and total
  length only. kitty's own cap is 256 KiB (`vt-parser.c` `MAX_ESCAPE_CODE_LENGTH`); freminal
  is deliberately more generous because it also implements iTerm2 images and sixel.
- **Raw bytes are the OSC dispatch input for every target.** The dispatcher parses the OSC
  number from the bytes before the first `;` and hands each target its raw body. Targets
  that use tokens (4, 10, 11, 12, 22, 52, 104) tokenise for themselves, with identical
  behaviour. Side effect, accepted: a non-UTF-8 byte no longer invalidates the raw-payload
  targets (8, 9, 99, 777, 1337, 1338); each handler validates its own input.
- **OSC 0/1/2/7 take the full remainder after the first `;`**, as xterm and kitty do. This
  is in scope for 129.

### 129 Decisions (orchestrator, recorded so they are not re-litigated)

- **One tokenizer, one base64 module, one chunk assembler.** Protocol modules keep their own
  typed models and error enums. Caps are named constants per protocol (the `MAX_OSC99_*`
  precedent).
- **The tokenizer reports, consumers decide.** It yields `Pair { key, value }` for a segment
  with `=` (split at the first `=`, so the value may contain `=`; key or value may be empty)
  and `Bare(segment)` for a segment without one. Empty segments are skipped (all four
  current parsers do this). Past the pair cap it yields one `TooManyItems` error and stops.
  Each consumer keeps its current rule for bare items, empty keys, empty values and key
  width. **Graphics keeps its first-byte key quirk** (`abc=1` → `a`); kitty rejects such
  keys, but that is a conformance change and belongs to Task 135.
- **Pair caps: 64** for every consumer (`MAX_OSC99_METADATA_ITEMS`,
  `MAX_KITTY_CONTROL_ITEMS`, `MAX_ITERM2_FILE_ARGS`, `MAX_FTCS_ITEMS`, `MAX_OSC8_PARAMS`).
  OSC 99 and graphics turn `TooManyItems` into a parse error (new variants); iTerm2, FTCS and
  OSC 8 ignore everything past the cap.
- **base64 API.** `decode` stays the lenient default (optional padding) but takes `&[u8]`
  and returns a typed `Base64Error`. It now **rejects a dangling single character**
  (`len % 4 == 1` after padding is stripped), which RFC 4648 makes invalid and which today
  silently decodes to nothing. `decode_strict` requires padded, length-multiple-of-4 input.
  `encode_unpadded` is new. `StreamDecoder` decodes chunked input in which any chunk may end
  mid-quantum **or** end with its own padding; this covers both chunking styles the OSC 99
  spec permits.
- **Only OSC 99 moves to stream decoding.** `parse_osc_99` stops decoding the payload;
  `Osc99Command` carries the raw payload plus an `Osc99PayloadEncoding`, and reassembly
  decodes through the assembler. A notification whose accumulated base64 is invalid is
  dropped as a whole (debug log); the spec allows ignoring the entire escape code. Graphics
  and iTerm2 keep per-chunk decoding (graphics is spec-conformant; iTerm2 is not a listed
  bug) and use the assembler for caps only.
- **Assembler caps.** Graphics: `400 MiB` total (kitty's `MAX_DATA_SZ`, the existing
  `MAX_KITTY_FILE_BYTES` value). iTerm2 multipart: `64 MiB` total, matching the OSC 1337
  parser cap. OSC 99: unchanged (`1 MiB` per notification). Exceeding a cap abandons the
  transfer with a warn that carries sizes only and **no reply**; error replies for graphics
  are Task 135's. No assembler pre-allocates from a sender-supplied size.
- **OSC 8** follows the spec and kitty's `parse_osc_8`: the body after `8;` is
  `params ; URI`, split at the **first** `;`; the URI is the whole remainder; `params` is a
  `:`-separated list in which only a non-empty `id=` value is used. An empty URI is `End`.
  A body with no `;` is `End` (as today). A non-UTF-8 URI produces no output.
- **Titles and OSC 7.** The value is the remainder after the first `;`, as UTF-8. Non-UTF-8,
  or no `;` at all, produces no output (debug log). An empty remainder is an empty title.
- **APC replies are S8C1T-aware**, through a new `write_apc_response`. `format_kitty_response`
  becomes `format_kitty_response_body` and returns the body only (`G…;msg`).
- **Payload-free warn logging.** A warn-level log line never contains sequence payload
  bytes or strings derived from them. It may contain the introducer, the OSC number, a
  sub-command name freminal itself defines, and lengths. Payload detail moves to `debug!`,
  always bounded with the existing `*_for_log_bounded` helpers. A DCS whose body starts with
  `@kitty-` is logged at debug with its length only.

### 129 Execution model

Sequential, on one branch (`task-129/kitty-wire-infrastructure`) in the main checkout. The
subtasks are small and most of them touch `ansi_components/osc.rs` or its neighbours, so
parallel worktrees would buy nothing and cost merges. Each subtask is implemented by a
sub-agent, code-reviewed by the orchestrator, and committed by the orchestrator. Order:

1. Common building blocks: 129.1 (base64), 129.2 (tokenizer), 129.3 (OSC 99 and graphics
   on the tokenizer).
2. OSC parser: 129.4 (terminator), 129.5 (raw dispatch), 129.6 (titles, OSC 7), 129.7
   (OSC 8), 129.8 (FTCS and iTerm2 args on the tokenizer).
3. Caps and accumulation: 129.9 (parser caps), 129.10 (assembler), 129.11 (graphics and
   iTerm2 on the assembler), 129.12 (OSC 99 stream decoding).
4. Replies and logging: 129.13 (APC replies), 129.14 (payload-free logging).
5. 129.15 (escape-sequence dual-doc update).

Then an adversarial review of the whole branch, before the PR.

### 129 Subtasks

Every subtask's verification is `cargo test --all` and
`cargo clippy --all-targets --all-features -- -D warnings`, plus anything named below. Every
subtask's prohibitions include: do NOT touch files outside scope, do NOT commit, and do NOT
proceed to the next subtask. Its stop condition is: report files changed and verification
results, then await review.

#### 129.1 — Typed base64: strict, lenient, streaming and unpadded

Scope:

- `freminal-common/src/base64.rs` (production code and tests);
- call sites, mechanical changes only:
  - `freminal-common/src/buffer_states/kitty_graphics.rs`;
  - `freminal-common/src/buffer_states/osc_notify_99.rs`;
  - `freminal-terminal-emulator/src/ansi_components/osc_clipboard.rs`;
  - `freminal-terminal-emulator/src/ansi_components/osc_iterm2.rs`;
  - any test module that calls `base64::decode` and stops compiling.

What:

- `pub enum Base64Error` (`thiserror`), with variants:
  - `InvalidByte { offset: usize, byte: u8 }`;
  - `InvalidLength { len: usize }`, a dangling single character, or for strict input a
    length that is not a multiple of 4;
  - `MisplacedPadding { offset: usize }`, `=` anywhere other than the end of a quantum.
- `pub fn decode(input: &[u8]) -> Result<Vec<u8>, Base64Error>`, lenient. Optional trailing
  padding. Trailing partial quanta of 2 or 3 characters are accepted; 1 character is
  `InvalidLength`. No whitespace is accepted. Same alphabet as today.
- `pub fn decode_strict(input: &[u8]) -> Result<Vec<u8>, Base64Error>`. Length must be a
  multiple of 4. Padding (0–2 `=`) only in the final quantum.
- `pub fn encode(input: &[u8]) -> String`, unchanged. `pub fn encode_unpadded(input: &[u8]) -> String`.
- `pub struct StreamDecoder` with `new()`,
  `feed(&mut self, chunk: &[u8], out: &mut Vec<u8>) -> Result<(), Base64Error>`, and
  `finish(self, out: &mut Vec<u8>) -> Result<(), Base64Error>`. Semantics:
  - a partial quantum carries over between `feed` calls;
  - `=` at quantum position 2 or 3 closes the quantum (its bytes are emitted); further `=`
    up to position 4 are consumed; a following non-`=` byte starts a new quantum. `=` at
    position 0 or 1 is `MisplacedPadding`;
  - `finish` accepts a pending partial quantum of 2 or 3 characters and rejects 1;
  - `offset` in errors is the offset within the `feed` call's chunk.
- Call sites: pass bytes (`.as_bytes()` where they had a `&str`) and map `Base64Error` into
  their existing error variants or logs. Keep every caller's existing UTF-8 checks.

Deliverable:

- the API above;
- unit tests:
  - every existing behaviour of `decode` except the dangling-character case;
  - the dangling-character case now errors;
  - strict rejects unpadded input, interior padding and lengths that are not a multiple of 4;
  - `encode_unpadded` for lengths 0–4;
  - stream: the same text fed at every split point decodes identically to one-shot
    `decode`;
  - stream: per-chunk-padded input (`"YQ=="` + `"Yg=="` gives `"ab"`);
  - stream: 1-character tail at `finish` errors;
- proptests:
  - round trip `decode(encode(x)) == x`;
  - round trip through `encode_unpadded`;
  - stream decoding equals one-shot decoding for arbitrary split points;
  - `decode` and `decode_strict` never panic on arbitrary bytes.

Prohibitions: do NOT move any decode call to a different place in the code; do NOT add caps
to the decoders.

**Status: Complete (2026-10-09).** As designed. Lenient `decode` also rejects padding beyond
what completes the final quantum (`Zm9v=`, `YQ===`), which the old decoder accepted; `"YQ="`
is still accepted. One graphics test used a 5-character payload and now uses a valid one.
Commit `cebf0d28`.

#### 129.2 — Bounded `key=value` tokenizer

Scope:

- `freminal-common/src/key_value.rs` (new; one concept: splitting kitty-style metadata);
- `freminal-common/src/lib.rs` (module declaration only).

What:

- `pub enum KeyValueSeparator { Colon, Comma, Semicolon }`.
- `pub enum KeyValueItem<'a> { Pair { key: &'a [u8], value: &'a [u8] }, Bare(&'a [u8]) }`.
- `pub enum KeyValueError { TooManyItems { max: usize } }` (`thiserror`).
- `pub fn tokenize(input: &'a [u8], separator: KeyValueSeparator, max_items: usize) -> KeyValueTokens<'a>`,
  where `KeyValueTokens<'a>` implements `Iterator<Item = Result<KeyValueItem<'a>, KeyValueError>>`.
  - Splits on the separator byte. Empty segments are skipped and do not count.
  - A segment containing `=` is a `Pair`, split at the first `=`. A segment without one is
    `Bare`.
  - After `max_items` items, the next non-empty segment yields one
    `Err(TooManyItems { max })`, and then the iterator is exhausted.
  - No allocation.

Deliverable: the module and tests for each separator; empty, leading, trailing and doubled
separators; `=` inside a value; empty key (`=x`); empty value (`k=`); bare items; exactly
`max_items` items (no error); `max_items + 1` (one error, then `None`); `max_items == 0`;
non-UTF-8 bytes passed through untouched. A proptest that it never panics and never yields
more than `max_items + 1` items.

Prohibitions: do NOT migrate any consumer.

**Status: Complete (2026-10-09).** As designed; `tokenize` is a `const fn` and the iterator
is fused. Commit `244910c7`.

#### 129.3 — OSC 99 and graphics control data on the tokenizer

Scope:

- `freminal-common/src/buffer_states/osc_notify_99.rs` (`parse_osc_99`'s pair loop, the
  error enum, and tests);
- `freminal-common/src/buffer_states/kitty_graphics.rs` (`parse_control_data`, the error
  enum, and tests).

What:

- **Pin first.** Before changing either loop, add tests for behaviour that is untested
  today and must survive:
  - OSC 99: empty value for each key class; duplicate scalar keys (last wins); a multi-byte
    key (error); `=x` (error);
  - graphics: `abc=1` is accepted as `a=1`; `a=tXYZ` is accepted as `a=t`; duplicate keys
    (last wins).
- Replace both pair loops with `key_value::tokenize`, keeping every current rule:
  - OSC 99 (`Colon`): `Bare` → `InvalidMetadata`; key length ≠ 1 → `InvalidMetadata`.
  - Graphics (`Comma`): `Bare`, empty key or empty value → `InvalidControlPair`; the key
    is `key[0]`.
- Add `pub const MAX_OSC99_METADATA_ITEMS: usize = 64` and
  `pub const MAX_KITTY_CONTROL_ITEMS: usize = 64`, with new error variants
  `Osc99ParseError::TooManyMetadataItems { max: usize }` and
  `KittyParseError::TooManyControlItems { max: usize }`, including their `Display` arms.

Deliverable: the migration; every existing test passes unchanged; the pin tests; a cap test
for each (64 items accepted, 65 rejected).

Prohibitions: do NOT change the value parsers, defaults or the payload split; do NOT fix the
first-byte key quirk (Task 135).

**Status: Complete (2026-10-09).** Pin tests were added and passed against the old loops
before migration. Error strings are byte-identical. Commit `4c8f3d82`.

#### 129.4 — OSC terminator strips exactly the terminator

Scope: `freminal-terminal-emulator/src/ansi_components/osc.rs` (`push`,
`is_final_character_osc_terminator`, tests).

What: when `is_osc_terminator` matches, remove exactly the terminator: one byte for BEL,
two bytes for `ESC \`. Delete `is_final_character_osc_terminator`.

Deliverable: the fix and parser-level tests:

- `OSC 0;C:\ BEL` sets the title `C:\`;
- `OSC 0;C:\ ESC \` sets the title `C:\`;
- `OSC 0;a\\ BEL` keeps both backslashes;
- an OSC body ending in `ESC` and terminated by BEL keeps the `ESC` in the raw body
  (assert on the raw body passed to an OSC 1338 or OSC 9 handler);
- every existing OSC test still passes.

Prohibitions: do NOT change dispatch or tokenisation (129.5).

**Status: Complete (2026-10-09).** Commit `4436f5aa`.

#### 129.5 — Raw-body OSC dispatch

Scope: `freminal-terminal-emulator/src/ansi_components/osc.rs` (`ansiparser_inner_osc`,
`dispatch_osc_target`, tests); `freminal-terminal-emulator/benches/buffer_benches.rs`
(benchmark capture only, no edits unless a bench stops compiling).

What:

- On `Finished`: split `self.params` at the first `;` into `number` and `body` (`body` is
  empty when there is no `;`). Parse `number` with `parse_param_as::<AnsiOscToken>`. If it
  is empty or fails, keep today's path exactly: push `TerminalOutput::Invalid` and return
  `ParserOutcome::Invalid("Invalid OSC params: …")`.
- `dispatch_osc_target(osc_target: &OscTarget, raw_params: &[u8], output: &mut Vec<TerminalOutput>) -> ParserOutcome`.
  - Tokenising arms (Foreground, Background, CursorColor, TitleBar, IconName, RemoteHost,
    Ftcs, Url, Clipboard, PaletteColor, ResetPaletteColor, PointerShape) call
    `split_params_into_semicolon_delimited_tokens(raw_params)` and derive
    `AnsiOscInternalType` themselves. On a tokenisation error they push
    `TerminalOutput::Invalid` and return the same `ParserOutcome::Invalid` as today.
    Their behaviour is otherwise byte-identical. (TitleBar, IconName, RemoteHost, Ftcs and
    Url keep the token path here; 129.6, 129.7 and 129.8 replace it.)
  - Raw arms (ITerm2, ShellInfo, Notify9, Notify777, Notify99) and the logging arms are
    unchanged and no longer tokenise.
  - Every other arm returns `ParserOutcome::Finished`.

Deliverable:

- the refactor;
- tests:
  - each raw target with a non-UTF-8 byte in its payload now reaches its handler (OSC 9
    with a Latin-1 byte, for example), and each handler's own validation decides the
    outcome;
  - each tokenising target with a non-UTF-8 byte still yields `TerminalOutput::Invalid`;
  - an empty or non-UTF-8 OSC number still yields `Invalid` (a non-numeric UTF-8 number was
    and remains `OscTarget::Unknown`, consumed with a warn; corrected at review);
- every existing OSC test passes unchanged.

Benchmarks: `bench_parse_osc9`, `bench_parse_plain_text` and `bench_parse_bursty`, before and
after, per `performance-benchmarks` and `freminal-bench-table` (15% threshold). Include the
table in the report.

Prohibitions: do NOT change any handler outside `osc.rs`; do NOT change titles, OSC 7, OSC 8
or FTCS semantics yet.

**Status: Complete (2026-10-09).** The `too_many_lines` allow on `dispatch_osc_target` is
gone (FTCS and the two warn bodies moved into helpers). Benchmarks (`parse_osc9`, plain text,
bursty) moved between −3.7% and −0.8%, all noise. Commit `5399a401`.

#### 129.6 — OSC 0/1/2/7 take the full remainder

Scope: `freminal-terminal-emulator/src/ansi_components/osc.rs` (the TitleBar, IconName and
RemoteHost arms and tests).

What: the value is `raw_params` after the first `;`, as `&str` via `std::str::from_utf8`.
No `;` at all, or non-UTF-8: no output, `debug!` with the length only. An empty remainder
is an empty string. These arms no longer tokenise.

Deliverable: the change and tests:

- `OSC 2;a;b BEL` gives `a;b`;
- `OSC 0;123 BEL` gives `123`;
- `OSC 0;? BEL` gives `?`;
- `OSC 2; BEL` gives the empty string;
- `OSC 2 BEL` gives no output;
- a non-UTF-8 title gives no output;
- `OSC 7;file://host/a;b BEL` passes `file://host/a;b` through;
- OSC 1 still maps to `SetTitleBar`.

Update any existing test that pinned the `Display`-artefact titles, and name each such test
in the report.

Prohibitions: do NOT touch the handler side (`terminal_handler/`).

**Status: Complete (2026-10-09).** Commit `0a73cc15`.

#### 129.7 — OSC 8 parsed from the raw body

Scope:

- `freminal-common/src/buffer_states/osc.rs` (`UrlResponse` construction and its tests);
- `freminal-terminal-emulator/src/ansi_components/osc.rs` (the Url arm and tests).

What:

- Replace `impl From<Vec<Option<AnsiOscToken>>> for UrlResponse` with
  `impl UrlResponse { pub fn from_osc8_body(body: &[u8]) -> Option<Self> }`, where `body` is
  `raw_params` after `8;`:
  - no `;` in `body`: `Some(End)`;
  - split at the first `;` into `params` and `uri`; an empty `uri` gives `Some(End)`;
  - a non-UTF-8 `uri` gives `None` (no output);
  - `id` is the value of the first `id` `Pair` with a non-empty value from
    `key_value::tokenize(params, Colon, MAX_OSC8_PARAMS)` (`MAX_OSC8_PARAMS = 64`), as UTF-8;
    otherwise `None`. Items past the cap are ignored.
- The Url arm calls it and pushes `AnsiOscType::Url` when it returns `Some`.

Deliverable: the change and tests:

- `8;;http://a;b` gives URL `http://a;b`;
- `8;id=x;u` gives id `x`;
- `8;foo=1:id=x;u` gives id `x`;
- `8;id=;u` gives no id;
- `8;foo;u` gives no id;
- `8;;123` gives URL `123`;
- `8;;` gives `End`;
- `8` alone gives `End`;
- a non-UTF-8 URI gives no output.

Update the existing tests in `freminal-common/src/buffer_states/osc.rs` that pin the old
`From` shape.

Prohibitions: do NOT change `Url`, `AnsiOscType::Url` or the handler.

**Status: Complete (2026-10-09).** The `From<Vec<Option<AnsiOscToken>>>` impl had no other
users. Commit `2569a871`.

#### 129.8 — FTCS and iTerm2 `File=` arguments on the tokenizer

Scope:

- `freminal-common/src/buffer_states/ftcs.rs`;
- `freminal-terminal-emulator/src/ansi_components/osc.rs` (the Ftcs arm);
- `freminal-terminal-emulator/src/ansi_components/osc_iterm2.rs` (`parse_iterm2_file_args`
  and tests);
- `freminal-terminal-emulator/src/terminal_handler/shell_integration.rs` (tests only, if
  they call the FTCS functions directly).

What:

- `parse_ftcs_params(body: &[u8]) -> Option<FtcsMarker>` and
  `is_known_ftcs_marker(body: &[u8]) -> bool`, where `body` is `raw_params` after `133;`.
  Tokenise with `Semicolon` and `MAX_FTCS_ITEMS = 64` (items past the cap are ignored).
  Positions count every item, as today: item 0 is the marker; the exit code is item 1 when
  it is `Bare`. Values are converted with `std::str::from_utf8`; a conversion failure
  behaves like today's missing value. The Ftcs arm stops tokenising and passes the body.
- `parse_iterm2_file_args` tokenises its `&str` input with `Semicolon` and
  `MAX_ITERM2_FILE_ARGS = 64`. `Bare` items are skipped silently, as today; items past the
  cap are ignored. Key and value handling is unchanged.

Deliverable: the migration; every existing FTCS, OSC 133, shell-integration and iTerm2 test
passes (update only call signatures). New tests:

- FTCS exit-code parsing with leading zeros (`D;007`);
- FTCS with empty fields between items;
- an FTCS body containing a non-UTF-8 `fid` is rejected;
- the iTerm2 cap (65 args: the 65th is ignored).

Prohibitions: do NOT change the `freminal=1` policy (Task 145); do NOT change iTerm2 value
semantics.

**Status: Complete (2026-10-09).** A non-UTF-8 value skips its item rather than clearing an
earlier valid one, so `fid=ok;fid=<bad>` keeps `ok`. Commit `2a8e5fb4`.

#### 129.9 — OSC, APC and DCS byte caps

Scope:

- `freminal-terminal-emulator/src/ansi_components/osc.rs`, `apc.rs`, `dcs.rs` (parsers and
  tests);
- `freminal-terminal-emulator/src/ansi.rs` (only if the size test or dispatch needs it);
- `freminal-terminal-emulator/benches/buffer_benches.rs` (one new benchmark).

What:

- Constants in each parser module:
  - `osc.rs`: `MAX_OSC_BYTES = 1 MiB` and `MAX_OSC_LARGE_BYTES = 64 MiB`. The large cap
    applies when the OSC number is 52 or 1337. The number is only examined once the default
    cap is reached, so the common path is one length comparison;
  - `apc.rs`: `MAX_APC_BYTES = 1 MiB`;
  - `dcs.rs`: `MAX_DCS_BYTES = 64 MiB`.
- When a byte would take the accumulated sequence past its cap, the parser drops the buffer
  (`Vec::new()`, releasing the memory) and enters an overflow state that remembers only
  what terminator detection needs:
  - the total length;
  - whether the previous byte was ESC (OSC, APC, plain DCS);
  - for a DCS that began `Ptmux;`, the current run of consecutive ESC bytes, so that the
    odd/even rule in `contains_string_terminator` still holds.
- On the terminator, an overflowed sequence produces **no** output and returns
  `ParserOutcome::Finished`. It logs one `warn!` naming the introducer (`OSC`, `APC`,
  `DCS`), the cap and the total length, and nothing else.
- `sub_parsers_stay_small` must still pass.

Deliverable:

- the caps;
- tests:
  - exactly-at-cap is dispatched, and cap + 1 is dropped, for each parser;
  - the large OSC cap applies to 52 and 1337 and not to 2;
  - an overflowed tmux DCS containing doubled ESCs ends at the real ST and not before;
  - an overflowed sequence followed by ordinary text prints the text;
  - memory is released on overflow (the buffer's capacity is 0).

Benchmarks: add `bench_parse_kitty_apc_chunks` (a stream of 4096-byte graphics APC chunks)
to `buffer_benches.rs` **before** changing the parsers, capture it with `bench_parse_osc9`
and `bench_parse_plain_text`, then capture again after. 15% threshold; include the table.

Prohibitions: do NOT change what a within-cap sequence emits; do NOT add CAN/SUB handling.

**Status: Complete (2026-10-09).** The cap counts stored bytes before the terminator; a
trailing ESC is not counted until the next byte shows it is not the start of ST (the literal
rule would have made the real limit `cap − 2`). `PrevByte` lives in `ansi.rs` beside
`ParserOutcome`. The new `bench_parse_kitty_apc_chunks` moved +0.3%, the others −1% to −3%.
Commit `547ad3f2`.

#### 129.10 — `BoundedChunkAssembler`

Scope:

- `freminal-terminal-emulator/src/terminal_handler/chunk_assembler.rs` (new; one concept:
  accumulating a chunked payload under caps);
- `freminal-terminal-emulator/src/terminal_handler/mod.rs` (module declaration only).

What:

- `pub struct ChunkLimits { pub max_chunk_bytes: usize, pub max_total_bytes: usize }`.
- `pub enum ChunkEncoding { Raw, Base64 }`.
- `pub enum ChunkError { ChunkTooLarge { len: usize, max: usize }, TotalTooLarge { max: usize }, Base64(Base64Error) }`
  (`thiserror`).
- `pub struct BoundedChunkAssembler` with:
  - `new(limits: ChunkLimits) -> Self`, which allocates nothing;
  - `push(&mut self, chunk: &[u8], encoding: ChunkEncoding) -> Result<(), ChunkError>`;
  - `len(&self) -> usize`, the assembled (decoded) bytes so far;
  - `is_empty(&self) -> bool`;
  - `finish(self) -> Result<Vec<u8>, ChunkError>`.
- Semantics:
  - `max_chunk_bytes` applies to the chunk as received (encoded length);
  - `max_total_bytes` applies to the assembled decoded bytes;
  - `Base64` chunks go through one `base64::StreamDecoder` owned by the assembler;
  - a `Raw` chunk pushed while the decoder holds a partial quantum is a
    `Base64(InvalidLength)` error;
  - `finish` flushes the decoder.
- An error poisons the assembler: every later `push` and `finish` returns the same error.
  Abandonment is explicit: the owner drops the assembler.

Deliverable: the module and tests for each limit (at the cap, and over by one), raw and
base64 pushes, mixed encodings, a mid-quantum split, per-chunk padding, poisoning, and that
`new` does not allocate (capacity 0).

Use the documented `expect(dead_code)` + `TODO(129.11)` exception if clippy flags it unused.

Prohibitions: do NOT migrate any accumulator.

**Status: Complete (2026-10-09).** Plan drift: `StreamDecoder` had no way to report a
partial quantum, so `StreamPosition` / `position()` / `bytes_fed()` were added to
`base64.rs` in the same commit. Commit `5fa526c5`.

#### 129.11 — Graphics and iTerm2 accumulators on the assembler

Scope:

- `freminal-terminal-emulator/src/terminal_handler/mod.rs` (`KittyImageState`,
  `MultipartImageState`);
- `freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs`
  (`handle_kitty_chunk_start`, `handle_kitty_chunk`, tests);
- `freminal-terminal-emulator/src/terminal_handler/graphics_iterm2.rs`
  (`handle_iterm2_multipart_begin`, `handle_iterm2_file_part`, `handle_iterm2_file_end`,
  tests).

What:

- Replace `accumulated_data: Vec<u8>` in both states with a `BoundedChunkAssembler`, using
  `ChunkEncoding::Raw` (payloads are still decoded per chunk upstream).
- Limits:
  - graphics: `max_chunk_bytes = MAX_APC_BYTES`, and
    `max_total_bytes = MAX_KITTY_DATA_BYTES`, a new constant equal to `400 * 1024 * 1024`
    (`MAX_KITTY_FILE_BYTES` may be defined in terms of it);
  - iTerm2: `max_chunk_bytes = MAX_ITERM2_MULTIPART_BYTES` and
    `max_total_bytes = MAX_ITERM2_MULTIPART_BYTES`, a new constant equal to
    `64 * 1024 * 1024`.
- Remove both `Vec::with_capacity(sender-supplied size)` calls.
- A cap error abandons the transfer: the state is set to `None`, a `warn!` carries the
  sizes only, and **no** reply is sent.
- Abandonment rules and everything else are unchanged.

Deliverable: the migration; existing chunked-transfer tests pass unchanged. New tests:

- each cap: the transfer is abandoned, no image is placed, no reply is written, and a
  following transfer works;
- a huge `S=` or `size=` does not allocate up front.

Prohibitions: do NOT change abandonment rules, replies or `full_reset` (Task 131); do NOT
change per-chunk decoding.

**Status: Complete (2026-10-09).** `MAX_KITTY_FILE_BYTES` became `usize`
(`= MAX_KITTY_DATA_BYTES`) and is converted with `conv2` for `read_capped`. The review
found that the tail of an abandoned transfer was still dispatched (see "129 Review").
Commit `0a7f41c1`.

#### 129.12 — OSC 99 stream decoding

Scope:

- `freminal-common/src/buffer_states/osc_notify_99.rs` (`Osc99Command`, `parse_osc_99`,
  `decode_payload`, tests);
- `freminal-terminal-emulator/src/terminal_handler/notify_99.rs` (`PendingNotification`,
  `reassemble_osc99`, helpers, tests);
- `freminal-terminal-emulator/src/terminal_handler/osc.rs` (OSC 99 tests only);
- `freminal-terminal-emulator/src/ansi_components/osc_notify.rs` (tests only).

What:

- `pub enum Osc99PayloadEncoding { Plain, Base64 }`. `Osc99Command` gains
  `payload_encoding`, and `payload` now holds the **raw** payload bytes (still base64 text
  when `Base64`). Update its doc comment.
- `parse_osc_99` stops base64-decoding. It still validates UTF-8 for `Plain` payloads and
  still enforces `MAX_OSC99_SEQUENCE_BYTES`.
- `PendingNotification`'s four `Vec<u8>` fields become `BoundedChunkAssembler`s, with
  `max_chunk_bytes = MAX_OSC99_SEQUENCE_BYTES` and
  `max_total_bytes = MAX_OSC99_NOTIFICATION_BYTES`. Each chunk is pushed with the encoding
  its own `e=` declared. The existing combined 1 MiB check stays, computed from `len()`.
- The standalone (no-id) path decodes through a temporary assembler.
- Finalisation calls `finish` on each assembler. Any `ChunkError` drops the whole
  notification with a `debug!` that carries no payload.

Deliverable:

- the change;
- regression tests:
  - a title base64-encoded once and split at **every** offset into two chunks reassembles
    exactly;
  - chunk-then-encode input (each chunk padded) reassembles exactly;
  - a body mixing a `Plain` chunk and a `Base64` chunk that ends on a quantum boundary;
  - invalid base64 in a middle chunk drops the notification;
  - the standalone path still decodes;
- every existing OSC 99 test passes, updated only where it asserted that `parse_osc_99`
  returns decoded bytes. Name each updated test in the report.

Prohibitions: do NOT change any other OSC 99 conformance behaviour (Task 138); do NOT
change the caps.

**Status: Complete (2026-10-09).** Plan drift: the `display_ansi_osc_notify99` test literal
in `freminal-common/src/buffer_states/osc.rs` also needed the new field. The review found
that a dropped id was revived by its next chunk (see "129 Review"). Commit `ea007e85`.

#### 129.13 — S8C1T-aware APC replies

Scope:

- `freminal-terminal-emulator/src/terminal_handler/pty_writer.rs`;
- `freminal-common/src/buffer_states/kitty_graphics.rs` (`format_kitty_response` and tests);
- `freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs` (the call sites and
  tests).

What:

- `const fn apc_response(&self) -> &'static [u8]`: `0x9F` in 8-bit mode, else `ESC _`.
- `fn write_apc_response(&self, body: &str)`: APC + body + ST, via `write_bytes_to_pty` (so
  tmux wrapping still applies).
- Rename `format_kitty_response` to `format_kitty_response_body`, returning `G<keys>;<msg>`
  with no framing. Every call site writes through `write_apc_response`.

Deliverable: the change; existing graphics reply tests pass (7-bit framing is unchanged).
New tests: 8-bit mode frames a graphics reply as `0x9F … 0x9C`; tmux passthrough still
wraps a reply.

Prohibitions: do NOT change which replies are sent or their content (Task 135).

**Status: Complete (2026-10-09).** 7-bit output is byte-identical to before. Commit
`91f2df2e`.

#### 129.14 — Payload-free warn logging

Scope:

- `freminal-terminal-emulator/src/ansi_components/`: `osc.rs`, `osc_clipboard.rs`,
  `osc_iterm2.rs`, `osc_notify.rs`, `osc_palette.rs`, `osc_shell_info.rs`, `apc.rs`, `dcs.rs`;
- `freminal-terminal-emulator/src/terminal_handler/`: `dcs.rs`, `osc.rs`, `osc_colors.rs`,
  `graphics_kitty.rs`, `graphics_iterm2.rs`, `graphics_sixel.rs`, `notify_99.rs`;
- `freminal-terminal-emulator/src/ansi_components/tracer.rs`, only if a helper is needed.

What: apply the "payload-free warn logging" decision to every `warn!` and `error!` in scope.

- Each one that interpolates sequence bytes, or strings derived from them, becomes
  payload-free. It keeps the introducer, the OSC number or sub-command, and lengths.
- The payload moves to an adjacent `debug!`, bounded with
  `escape_sequence_for_log_bounded` or `lossy_sequence_for_log_bounded`. Drop it where it
  adds nothing.
- A DCS whose body starts with `@kitty-` is consumed with one `debug!` carrying its length
  only.
- The `DCS received` debug log and the XTGETTCAP query debug log become bounded.
- Fix the mislabelled `type_number=` in the unknown-OSC warn, so that it prints the OSC
  number.

Deliverable:

- the sweep;
- tests, using `tracing-test` or an equivalent existing capture helper in the crate:
  - an unknown OSC, an unknown DCS and a non-kitty APC each warn without any payload byte;
  - `@kitty-print` produces no warn;
- a list in the report of every log line changed, as `file:line`, old level, new level.

Prohibitions: do NOT change any behaviour other than logging; do NOT change the 128.C4
`warn!` level decision for CSI.

**Status: Complete (2026-10-09).** About 45 log lines changed across 12 files. Kitty parse
errors log a static kind at warn (`kitty_parse_error_kind`); shared-memory warns no longer
carry the sender-chosen object name. A `#[cfg(test)]` `log_capture` subscriber in
`tracer.rs` backs 18 payload-absence tests. Out of scope and recorded as 129.C6: the
trace-level `parsed terminal output` log. Commit `e0349570`.

#### 129.15 — Escape-sequence dual-doc update

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`.

What: record the behaviour changes from 129.4–129.9 and 129.12:

- the trailing-terminator fix;
- raw-body dispatch, and that non-UTF-8 no longer invalidates the raw targets;
- full-remainder titles and OSC 7;
- the OSC 8 `;` and `id=` fixes;
- the parser caps and over-cap behaviour;
- OSC 99 chunked base64;
- S8C1T-aware APC replies;
- `@kitty-*` DCS silenced.

Remove any GAPS row these close. Refresh both "Last updated" headers.

Verification: `markdownlint-cli2` and `prettier --check` on both files; the pre-commit hooks
(`cargo xtask lint-markdown` is broken tree-wide, see 126.C1).

Prohibitions: do NOT touch code.

**Status: Complete (2026-10-09).** A `DCS @kitty-…` row was added to both documents. No GAPS
row was removed: the fixed bugs were only ever recorded in this plan. Commit `272761d7`.

### 129 Review

An adversarial sub-agent review of the whole branch found no BLOCKER, one MAJOR and several
MINOR findings, each confirmed by reproduction:

- **MAJOR — abandoned kitty transfer tail.** After a cap abandonment the remaining
  continuation chunks were read as a new transmit, and the final `m=0` chunk wrote
  `EINVAL:missing width` with `i=0`, contradicting 129.11's "no reply". Fixed in `a8b00442`:
  `kitty_state: Option<…>` became `kitty_transfer: KittyTransfer { Idle, Receiving, Discarding }`;
  `Discarding` swallows continuation chunks until `m=0`, and an explicit `a=` ends it.
- **MINOR — OSC 99 dropped id revived.** A later chunk for a dropped id started a fresh
  notification, so a `d=1` tail could display a truncated one. Fixed in `4aa278cb` with a
  `PendingEntry::Dropped` tombstone that counts toward the 128-id cap; control requests for a
  tombstoned id behave as if no entry existed.
- **MINOR — logging.** The OSC 99 parse-error debug was unbounded (300 KB for a 100 KB
  payload), the kitty `a=p` unknown-id warn listed every stored id, and `Base64Error`'s
  Display put one payload byte into warn lines. Fixed in `a8b00442`, `4aa278cb` and
  `45d254fe`.
- **NIT — leading-zero OSC numbers.** `OSC 052` dispatched as 52 but got the default cap.
  `OscLimit` now compares the parsed number (`45d254fe`).
- Smaller fixes in the same commits: the dead `Osc99ParseError::InvalidBase64` variant is
  deleted, a bool test-helper parameter became an enum, the bench's `as u64` cast is gone, and
  `freminal-bench-table` lists `bench_parse_kitty_apc_chunks`.

**Process failure and second pass (2026-10-10).** Three commits (`5fa526c5`, `a8b00442`,
`45d254fe`) were salvaged from sub-agents that died mid-task ("endpoint unavailable") and
were committed after only a partial review. `45d254fe` shipped flaky log-capture tests
(7 failures in 200 runs) with a `rebuild_interest_cache` patch that could not work. Remedy:
every 129 commit was re-audited against its subtask text by read-only agents, a confirmation
review re-checked the three fixes, and each remaining finding was fixed or routed:

- **Flaky log capture, root-caused** (`8a297acf`). With at most one dispatcher registered,
  `tracing-core` computes a callsite's interest from the calling thread's default
  dispatcher and caches it process-wide, so a parallel test thread with no subscriber cached
  `never` for a callsite a capturing thread later used. `log_capture` (now its own
  `#[cfg(test)]` module, `src/log_capture.rs`) installs one global always-interested
  subscriber that routes events to a thread-local sink. 0 failures in 300 runs; a barrier
  test reproduces the old race.
- **Kitty** (`4e955a38`). `Discarding` swallows only a bare continuation (nothing but `m`/`q`),
  so an actionless new command after an abandonment whose `m=0` never came is processed;
  `o=z` inflation is capped (129.C4); a `Raw` push resets the assembler's base64 stream so a
  stale padding slot cannot absorb a later `=`; `StreamDecoder` is no longer `Copy` and
  `bytes_fed` is gone; tests for the bounded unknown-id log, the real chunk-limit constants
  and the shared-memory warn.
- **Parsers** (`1bbb6af3`, `912ab185`, `39544320`). `OscLimit` classifies the OSC number with
  the same `parse_param_as` dispatch uses (`+52` too); an OSC body made one over the cap by
  BEL after a held ESC is dropped; the parsed-output trace is bounded (129.C6); overflow-warn
  content tests for OSC, APC and DCS; the raw-target tests now prove the handler ran; stale
  line-number comments repaired.
- **Test gaps** (`fd597840`): OSC 99 accumulated content, the dedicated `@kitty-` debug line,
  FTCS non-UTF-8 duplicate values.
- **Docs**: COVERAGE/GAPS wording (OSC 52 cap "introduced", not "raised"; `DCS received`
  still logs a bounded body; GAPS DCS intro; the `o=z` inflation cap), the 1-mod-4 /
  excess-padding base64 change (`dba627cb`), `KITTY_PROTOCOL_REFERENCE.md` function name, the 129.5 non-numeric-number sentence, and
  the Task 131 stub's `kitty_state` references.

Accepted as is, with reasons:

- The kitty and iTerm2 total-cap tests swap in a small-limit assembler; the 400 MiB and
  64 MiB constants are pinned by a value test instead of a 400 MiB push.
- `Vec` doubling can transiently reserve about twice a total cap (documented on the type).
- `decode_strict` and `encode_unpadded` have no production caller; Tasks 102 and 146 are
  their consumers.
- The 129.5 and 129.9 benchmark tables live in the status notes, not in the commits.
- The unknown-id `a=p` warn is bounded but not rate-limited (one per command).
- One DCS test pushes 64 MiB through the full parser (about 1.2 s in debug).

- `is_bare_continuation` applies only while `Discarding`. While `Receiving`, any actionless
  command is still a continuation, which tolerates clients that repeat `i=`/`f=` on every
  chunk (kitty is equally lenient). After an abandonment, such a client's tail is read as a
  new command and can draw an error reply; spec-compliant clients are unaffected.

Routed to later tasks: `a=f` continuation chunks carry `a=`, so they never accumulate and
an `a=f` chunk ends a discard (pre-existing, Task 135); `o=z` is capped at
`MAX_KITTY_DATA_BYTES` only, not at the size implied by `s`/`v`/`f` (Task 135); `full_reset` leaves `kitty_transfer` live (Task 131, recorded in its stub); a
saturated 128-id OSC 99 map refuses even complete single-chunk notifications, and tombstones
keep unbounded id strings (Task 138).

### 129 Cleanup entries

#### 129.C1 — Tail chunks of an abandoned kitty transfer are dispatched

- **Surfaced:** 129.11 (2026-10-09), confirmed by the 129 review.
- **Status: Resolved (2026-10-09), commit `a8b00442`.** See "129 Review".

#### 129.C2 — `Osc99ParseError::InvalidBase64` has no constructor

- **Surfaced:** 129.12 (2026-10-09).
- **Status: Resolved (2026-10-09), commit `4aa278cb`.** Deleted.

#### 129.C3 — A dropped OSC 99 notification is revived by its next chunk

- **Surfaced:** 129.12 (2026-10-09), confirmed by the 129 review.
- **Status: Resolved (2026-10-09), commit `4aa278cb`.** See "129 Review".

#### 129.C4 — Kitty `o=z` inflation is unbounded

- **Surfaced:** 129 review (2026-10-09). Predates Task 129.
- **Impact:** `inflate_zlib` (`graphics_kitty.rs`) uses `read_to_end` with no limit, so a
  1 MiB compressed payload can inflate to about 1 GiB. That makes the 400 MiB assembly cap
  partly cosmetic.
- **Scope of fix:** `terminal_handler/graphics_kitty.rs` (`inflate_zlib` and its callers).
- **Suggested approach:** inflate through `Read::take(MAX_KITTY_DATA_BYTES + 1)` and fail
  over the cap, mirroring `read_capped`; when `S=`/`s`/`v` give an expected size, cap at it.
- **Verification:** a zlib bomb test fails cleanly without allocating past the cap.
- **Status: Resolved (2026-10-10), commit `4e955a38`.** `inflate_zlib` takes a limit
  (`MAX_KITTY_DATA_BYTES` in production) and an over-limit stream takes the corrupt-stream
  reply path.

#### 129.C5 — An invalid byte inside an OSC leaks the rest of the sequence as text

- **Surfaced:** 129 review (2026-10-09). Predates Task 129; behaviour is identical in-cap
  and over-cap.
- **Impact:** a C0 byte other than ESC/BEL (LF, CAN, …) inside an OSC makes the parser
  emit `Invalid` and return to ground, so the remaining payload bytes up to the terminator
  are printed as text. A program can use this to inject visible text from a payload.
- **Scope of fix:** `ansi_components/osc.rs` and the top-level dispatcher in `ansi.rs`.
- **Suggested approach:** follow the VT500 state machine: CAN/SUB abort the string; other
  C0 bytes inside an OSC string are ignored rather than aborting it. Look up xterm's and
  kitty's handling first; do not infer.
- **Verification:** tests for LF, CAN and SUB inside an OSC, at both in-cap and over-cap
  sizes.
- **Status: Resolved (2026-10-10).** Semantics checked first: kitty (`vt-parser.c`
  `find_st_terminator`) keeps every byte up to BEL or `ESC \` and never aborts; xterm and
  the DEC VT500 parser ignore C0 inside a string and let CAN/SUB cancel it. **Maintainer
  decision: xterm/VT500.** `osc_byte_class` classifies each byte: CAN/SUB cancel (new
  `Cancelled` state, no output, buffer released); other C0 except BEL/ESC, and DEL, are
  ignored without touching the ESC-before-`\` tracking; the old `Invalid`/`InvalidFinished`
  states are gone. Applies identically within the cap and after overflow. No kitty protocol
  carries raw C0 in an OSC, so compliant clients are unaffected. `bench_parse_osc9` and
  `bench_parse_plain_text` are within noise. ESC handling is unchanged (an ESC not followed
  by `\` stays in the body, as in kitty; VT500 would end the string).

#### 129.C6 — `parsed terminal output` trace log is unbounded

- **Surfaced:** 129.14 (2026-10-09).
- **Impact:** `state/internal.rs` logs every parsed `TerminalOutput` at trace level with
  `%output`, which prints whole DCS/APC/OSC payloads unbounded when trace logging is on.
- **Scope of fix:** `freminal-terminal-emulator/src/state/internal.rs`.
- **Suggested approach:** log the variant and a bounded rendering, or drop the payload.
- **Verification:** a log-capture test at trace level.
- **Status: Resolved (2026-10-10), commit `1bbb6af3`.** The output is rendered through
  `lossy_sequence_for_log_bounded`; the test fails with a 102 KB event when reverted.

---

## Foundation stubs (v0.13.0)

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
  The kitty chunked-transfer state (`kitty_transfer`, formerly `kitty_state`) is not cleared
  on RIS. Neither are the iTerm2 `multipart_state` nor the `tmux_reparse_queue` (found by
  the Task 129 activation recon and review).
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
- Fix B15 and the RIS `kitty_transfer` / `multipart_state` / `tmux_reparse_queue` leaks.
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
