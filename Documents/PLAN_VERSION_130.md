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
subtask breakdown: **126**, **127**, **128**, **129** (activated 2026-10-09), and **130** and **131**
(activated 2026-10-10). Everything else is an **enriched stub**:
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
| 129 | Kitty Wire Infrastructure                   | v0.13.0   | L     | Complete      | None                               |
| 130 | Reverse-Path & Capability-Query Consistency | v0.13.0   | L     | Pending merge | 129                                |
| 131 | Screen-Scoped State & Reset Lifecycle       | v0.13.0   | L     | Pending merge | 130.1–130.3 (foundation)           |
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
| Desktop notifications | Done, incl. reports and `p=?` handshake             | Buttons reported 0-based; chunk metadata clobbered by defaults; `a=report` disables focus; close report lost after activation; no update-in-place; `p=close` doesn't close; ~~`p=?` answered after DA1~~ (fixed by 130)                              | 138           |
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
| **130** Reverse path & capability queries | GUI-originated replies go through handler framing (S8C1T; never tmux-wrapped); capability queries answered on the PTY thread so they precede DA1; advertised capabilities match reality              | 102, 135, 138, 139, 144, 146, 105           |
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
- **Then 129, 130, 131 and 132.** 130 depends on 129. 130 and 131 run as parallel worktrees
  after a shared foundation (130.1–130.3); see "130 Execution model". 132 is independent.
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

## Task 130 — Reverse-Path & Capability-Query Consistency

### 130 Summary

Every reply the terminal sends to the application is framed by the handler, so it honours
S8C1T. Every capability query is answered on the PTY thread, in byte-stream order, so its
reply comes before the DA1 reply that the kitty detection recipes rely on. Advertised
capabilities match what the running configuration and platform actually deliver. Activated
and decomposed on 2026-10-10 against `2862acb9` (main after PR #536).

### 130 Activation recon (2026-10-10)

Three read-only audits re-traced the stub. The stub undercounted the problem:

- **Reply classes.** Handler replies split four ways.
  - **Framed by the handler** (`write_*_response`, S8C1T-aware): DA1/2/3, XTVERSION, DSR, CPR,
    the `CSI t` reports answered on the PTY thread, OSC 4/10/11/12, DECRQSS, XTGETTCAP, and
    kitty graphics.
  - **Hand-formatted `\x1b[…` strings through `write_to_pty`:** DECRQM for handler-owned
    modes (`ReportMode::report()` returns a full `\x1b[?N;Ps$y` string) and the kitty
    keyboard query `CSI ? u`.
  - **`TerminalState::send_decrpm`** (`state/internal.rs:717`) bypasses the handler
    entirely, for 11 `TerminalState`-owned modes.
  - **GUI hand-formatted, all 7-bit:**
    - `rendering.rs`: the `CSI 1t/2t`, `3t`, `4t` and `5t` window reports, OSC `L`/`l`
      title reports, and the OSC 52 reply;
    - `app_impl.rs`: OSC 99 `p=alive` and `p=?`;
    - the `freminal-notify-99` thread: OSC 99 activation and close reports.
- **Ordering.** `handle_incoming_data` runs `handler.process_outputs` over the whole batch,
  then `sync_mode_flags` over the whole batch, then a post-hoc RIS scan, then the tmux
  reparse queue.
  - A DECRQM reply for a `TerminalState`-owned mode therefore lands **after** a DA1 that
    came later in the same read.
  - Anything that arrived through tmux passthrough is answered after everything else.
  - The OSC 99 `p=?` reply is built by the GUI on a later frame, so it always follows DA1.
- **tmux.**
  - `handle_tmux_passthrough` dispatches inner APC and DCS directly, with
    `in_tmux_passthrough` set so that their replies are wrapped in `ESC P tmux; … ESC \`.
  - Inner CSI goes through `dispatch_tmux_csi` (about 310 lines), a second CSI dispatcher
    that predates Task 128. It handles plain numeric cursor and erase commands only and
    exists purely to keep ordering against inner APC.
  - Everything else is pushed onto `tmux_reparse_queue`. That queue is fed into the
    **main** parser at the end of the batch, so a read that ended mid-sequence gets the
    reparsed bytes spliced into its partial sequence.
- **tmux never unwraps replies.**
  - tmux reads what the outer terminal sends as key input (`tty-keys.c`). It recognises only
    a fixed set of replies (DA, XTVERSION, OSC 10/11/4/52, …) and never unwraps
    `ESC P tmux;`.
  - The wrapped replies freminal sends today therefore reach tmux as garbage keystrokes.
  - kitty, WezTerm and Ghostty all reply unwrapped.
- **OSC 99 `p=?`** answers a constant string unconditionally:
  `a=report:c=1:o=always,unfocused,invisible:p=title,body,icon,buttons,alive,close,?:s=system,silent:u=0,1,2:w=1`.
  - No notification config is consulted, and notifications are disabled by default.
  - `a=report` is advertised on macOS and Windows, where activation is never reported.
- **Config on the PTY thread.**
  - The PTY thread sees only `PtyTabInitialState` (theme, URL detection, cursor style) and a
    handful of per-field `InputEvent`s broadcast by `apply_new_config`.
  - Nothing reaches it about notifications or capabilities.

### 130 Decisions (maintainer, 2026-10-10)

- **Replies are never tmux-wrapped.** Delete `wrap_tmux_passthrough` and the
  `in_tmux_passthrough` flag. The stub's "tmux-passthrough-wrapped" premise was wrong; see the
  recon.
- **Every GUI reply goes through the typed reply path.** That covers the window reports,
  title reports, OSC 52 and all four OSC 99 reports, not just the two the stub named.
- **Config reaches the PTY thread as a seed plus events**, following the existing precedent: a
  typed `HostCapabilities` seeded through `PtyTabInitialState`, and
  `InputEvent::HostCapabilitiesChange` broadcast from `apply_new_config`. No `ArcSwap` for
  config.
- **OSC 99 `p=?` truthfulness.**
  - No reply at all when `notifications.enabled` is off, `osc_99` is off, or `routing_osc99`
    is `Disabled`.
  - `a=report` is advertised only on Linux/BSD, and only when the routing can take the system
    leg (`System`, `Both`, `SystemWhenUnfocused`).
  - `c=1` is advertised only when the system leg is possible.
  - Every other key is unchanged; full conformance is Task 138.
- **The tmux passthrough is unified with the real parser** (the maintainer's "do it right"
  answer to the Task 131 stub question; it is foundation work here).
  - `TerminalState` processes parsed outputs one at a time, in event order.
  - Each tmux payload is drained immediately, through a fresh parser instance.
  - `dispatch_tmux_csi` is deleted.

### 130 Decisions (orchestrator, recorded so they are not re-litigated)

- **Event order is the processing model.**
  - For each parsed output, `TerminalState` does three things in order:
    1. calls the handler for that one output;
    2. syncs its own mode flags for it;
    3. applies its own RIS state reset if the output is `ResetDevice`.
  - It then drains any tmux payload the handler queued while processing that output.
  - The once-per-batch `prune_evicted_real_placements` stays once per batch.
  - This fixes the DECRQM-after-DA1 inversion and makes RIS apply in event order, as DECSTR
    already does.
- **The tmux parser is fresh per payload.**
  - It is seeded with the outer parser's `vt52_mode` and `s8c1t_mode`.
  - Inner payloads are complete sequences; a fresh parser cannot splice into the outer one.
  - Nesting depth is capped at `MAX_TMUX_PASSTHROUGH_DEPTH = 4`. Deeper payloads are dropped
    with a payload-free debug log.
- **DECRPM is framed by the handler.**
  - `ReportMode::report()` returns the body only (`?N;Ps$y`).
  - Every DECRPM, including the `TerminalState`-owned ones, goes out through
    `write_csi_response`. `send_decrpm` is deleted.
  - `CSI ? u` also goes through `write_csi_response`.
  - After 130.3 only two raw `write_to_pty` callers remain:
    - the VT52 DA1 reply `ESC / Z`, which is VT52 and has no C1 form;
    - the empty ENQ answerback.
- **`HostCapabilities` is the registry of host-dependent facts.** It holds exactly what the
  PTY thread cannot know by itself: config and platform. Answers that are intrinsic to the
  handler's own implementation (DA1, XTGETTCAP, kitty `a=q`, `CSI ? u`, DECRQSS) stay where
  they are, because their source of truth is the code that implements them. Today the
  registry holds OSC 99 support. Later tasks add to it (138, 146).
- **OSC 99 control requests while unsupported.** `p=?` gets no reply. `p=alive` gets no reply
  and is not forwarded to the GUI, because answering it would also advertise the protocol.
  **Superseded by adversarial-review finding 14:** every OSC 99 request (display payloads and
  `p=close` included) is dropped by the handler while `Unsupported`.
- **`GuiReply` lives in the emulator crate** (`io/gui_reply.rs`, next to `InputEvent`). It
  carries structured fields; framing is the handler's job.
- **Replies are not recorded to FREC.** This matches handler replies, which were never
  recorded. Only `InputEvent::Key` is recorded as `PtyInput`.
- **The notify thread must not keep the PTY consumer alive.**
  - The consumer thread exits when the pane's `input_tx` disconnects.
  - A strong `Sender<InputEvent>` clone held by the long-blocking `freminal-notify-99`
    thread would keep a closed pane's shell alive for as long as the notification is
    displayed.
  - So the pane owns an `Arc<Sender<InputEvent>>` (`Pane::reply_tx`, never cloned
    strongly), and the notify thread holds a `Weak`. A reply for a closed pane is dropped.
- **`Pane::pty_write_tx` stays**, but only for layout startup-command injection
  (`layout_ops.rs:262`). Moving that to `InputEvent::Key` would start recording it as user
  input, which is a separate decision. Its docs are corrected.

### 130 Execution model

Tasks 130 and 131 run as **parallel worktrees after a foundation**, per
`parallel-work-isolation`.

- **Foundation first:** 130.1–130.3 land sequentially on the integration branch
  `task-130-131/reverse-path-and-screen-state`.
  - Task 131 needs all three:
    - 130.1 owns the `TerminalState` processing loop, which 131 would otherwise touch for RIS
      ordering;
    - 130.2 deletes the tmux code, which 131's reset table would otherwise have to classify;
    - 130.3 changes `ReportMode`, which 131's new `?1047` mode type implements.
  - Landing them first removes the shared-type and shared-function conflicts.
- **Then two worktrees fork from the integration branch:**
  - `../freminal-130` on `task-130/reverse-path` (130.4–130.9);
  - `../freminal-131` on `task-131/screen-state` (131.1–131.11).
- **Remaining overlap after the fork:**
  - both edit `terminal_handler/mod.rs`: 130 adds the `host_capabilities` field and setter,
    131 restructures fields and resets;
  - both edit `ESCAPE_SEQUENCE_COVERAGE.md` / `ESCAPE_SEQUENCE_GAPS.md` (different rows);
  - this plan document (status notes only).

  The single semantic coupling is that 131's exhaustive reset table must classify 130's new
  field. That is enforced at compile time by the table's exhaustive destructuring, and it is
  resolved by the orchestrator at merge.

- **Merge order:** each task passes its adversarial review in its own worktree. 130 then
  merges into the integration branch, 131 rebases, and 131 merges. The full verification
  suite runs after each merge.
- Each subtask is implemented by a sub-agent, reviewed in full by the orchestrator against
  the subtask text, and committed by the orchestrator.

### 130 Subtasks

Every subtask's verification is `cargo test --all` and
`cargo clippy --all-targets --all-features -- -D warnings`, plus anything named below. Every
subtask's prohibitions include: do NOT touch files outside scope, do NOT commit, do NOT update
plan documents, and do NOT proceed to the next subtask. The stop condition is the same for
all: report files changed and verification results, then await review.

#### 130.1 — Event-order output processing (foundation)

Scope:

- `freminal-terminal-emulator/src/state/internal.rs` (production and tests);
- `freminal-terminal-emulator/src/terminal_handler/mod.rs`:
  - `process_outputs`;
  - a new `pub(crate)` per-output entry point;
  - a new `pub(crate) fn finish_output_batch(&mut self)` that runs
    `prune_evicted_real_placements`;
- new tests in `freminal-terminal-emulator/tests/terminal_state_tests.rs`.

What:

- `handle_incoming_data` (and the reparse path) iterates the parsed outputs once. For each
  output it does, in this order:
  1. call `handler.process_output_in_batch(output)` (the renamed, now `pub(crate)`,
     `process_output`);
  2. call `self.sync_mode_flags(output)`;
  3. if the output is `TerminalOutput::ResetDevice`, apply the `TerminalState` RIS reset
     **at that point**: modes, parser, `leftover_data`, `cursor_visual_style`, and
     `window_commands`.

  After the loop it calls `handler.finish_output_batch()`.

- Extract the RIS block into `fn apply_state_reset(&mut self)`.
- **RIS replaces the parser mid-iteration.** The outputs that follow in the same chunk were
  already parsed by the old parser. Only the next chunk sees the fresh parser. Document this
  at the reset site.
- `handler.process_outputs` keeps its public behaviour (tests, benches and the shadow handler
  use it). It becomes `for o in outputs { self.process_output_in_batch(o) }` followed by
  `self.finish_output_batch()`.
- The tmux reparse drain is left where it is in this subtask (130.2 moves it).
- Update the pipeline doc comment on `handle_incoming_data`. Stages 4–6 become one
  event-order stage.

Deliverable:

- the change;
- regression tests through `handle_incoming_data` with a write-channel receiver:
  - `CSI ?2026$p` followed by `CSI c` in one buffer yields the DECRPM reply **before** the
    DA1 reply. It fails before the change.
  - `ESC c` followed by `CSI ?1h` (DECCKM) in one buffer leaves DECCKM set. It fails before
    the change, because the post-hoc RIS reset wiped it.
  - `CSI ?1h` followed by `ESC c` leaves DECCKM reset.
  - `ESC c` followed by `ESC SP G` (S8C1T) in one buffer leaves the parser in 8-bit mode.

Benchmarks:

- Groups `bench_handle_incoming_data`, `bench_parse_bursty`, `bench_parse_cup_writes` and
  `bench_parse_sgr_heavy`.
- The orchestrator captures the baseline before the subtask starts, as
  `--save-baseline before_130_1`. The implementer captures the comparison.
- 15% threshold, per `performance-benchmarks`.

Verification: as above, plus `cargo bench --no-run --all`.

Prohibitions: do NOT change the tmux reparse path (130.2); do NOT change reply formats
(130.3); do NOT change what RIS resets (Task 131).

#### 130.2 — tmux passthrough through the real parser (foundation)

Scope:

- `freminal-terminal-emulator/src/terminal_handler/dcs.rs`:
  - `handle_tmux_passthrough`;
  - delete `dispatch_tmux_csi`, `wrap_tmux_passthrough` and their tests;
  - convert the surviving tmux tests;
- `freminal-terminal-emulator/src/terminal_handler/pty_writer.rs`: remove the wrapping branch
  from `write_bytes_to_pty`;
- `freminal-terminal-emulator/src/terminal_handler/mod.rs`:
  - remove the `in_tmux_passthrough` field;
  - rename `tmux_reparse_queue` to `tmux_passthrough_queue` and `take_tmux_reparse_queue` to
    `take_tmux_passthrough_queue`;
- `freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs`: only the test at about
  line 11563 that sets `in_tmux_passthrough`;
- `freminal-terminal-emulator/src/state/internal.rs`;
- `freminal-terminal-emulator/tests/standard_unit.rs` (tmux tests);
- new integration tests in `freminal-terminal-emulator/tests/tmux_passthrough.rs`.

What:

- **Handler side.** `handle_tmux_passthrough` un-doubles the ESC bytes and pushes the
  **whole** inner payload onto `tmux_passthrough_queue`, whatever its introducer. It no
  longer dispatches anything itself.
- **`TerminalState` side.** In the event-order loop from 130.1, after processing an output,
  `TerminalState` takes the handler's queue. For each payload it:
  1. builds a fresh `FreminalAnsiParser`;
  2. copies `vt52_mode` and `s8c1t_mode` from `self.parser`;
  3. parses the payload;
  4. processes the resulting outputs through the same per-output routine, recursively, with
     a depth argument.

  At depth `MAX_TMUX_PASSTHROUGH_DEPTH` (`4`, a module constant) a payload is dropped with a
  `debug!` that carries its length and depth only.

- **Delete** `drain_tmux_reparse_queue`, `dispatch_tmux_csi`, `wrap_tmux_passthrough`,
  `double_esc` (if it becomes unused) and `in_tmux_passthrough`. Replies are never wrapped
  (maintainer decision).
- **Convert the tests.**
  - Every deleted `dispatch_tmux_csi` test's scenario becomes a `TerminalState`-level test
    in `tests/tmux_passthrough.rs`, asserting the same buffer effect.
  - Tests asserting wrapped replies are rewritten to assert **unwrapped** replies.
  - Remove nothing without a replacement. List the old-to-new mapping in the report.

Deliverable:

- the change;
- tests:
  - inner CSI cursor/erase (the old direct-dispatch set);
  - inner SGR and mode set (previously reparsed);
  - inner OSC title;
  - inner kitty APC (`a=T`, `a=q` reply is **unwrapped**);
  - inner DCS DECRQSS (reply unwrapped);
  - nested tmux at depth 2 works;
  - depth 5 is dropped;
  - ordering: `tmux;CSI H` + `tmux;APC a=p` + `tmux;CSI 5;5H` + `tmux;APC a=p` in one buffer
    places the two images at the two cursor positions (the scenario `dispatch_tmux_csi`
    existed for);
  - a read ending mid-sequence followed by a tmux payload in the next read does not corrupt
    either sequence;
  - `DCS tmux; ESC [ c ST` yields an unwrapped DA1 reply, in order with a surrounding
    `CSI 5n`.

Verification: as above.

Prohibitions: do NOT change reply formats beyond removing the wrapping (130.3); do NOT
change non-tmux DCS dispatch.

#### 130.3 — Handler-framed CSI replies (foundation)

Scope:

- `freminal-common/src/buffer_states/modes/mod.rs` (`ReportMode` doc);
- every `ReportMode` impl in `freminal-common/src/buffer_states/modes/*.rs` and
  `freminal-common/src/buffer_states/mode.rs`, plus their tests;
- `freminal-common/tests/mode_boilerplate_tests.rs`;
- `freminal-common/tests/mouse_mode_tests.rs`;
- `freminal-terminal-emulator/src/terminal_handler/mod.rs` (DECRQM arms and the
  `KittyKeyboardQuery` arm);
- `freminal-terminal-emulator/src/terminal_handler/pty_writer.rs` (visibility only);
- `freminal-terminal-emulator/src/state/internal.rs` (`handle_mode_query`; delete
  `send_decrpm`);
- `freminal-terminal-emulator/tests/modes_tests.rs`;
- `freminal-terminal-emulator/tests/integration_all.rs`;
- `freminal-terminal-emulator/tests/decrpm_integration.rs`;
- `freminal-terminal-emulator/tests/vttest_s8c1t.rs`.

What:

- **`ReportMode::report()`** returns the DECRPM body without the introducer (`?2004;1$y`,
  `20;2$y`). Update the trait doc to say so.
- **Handler.** Every DECRQM arm and the `CSI ? u` arm call
  `self.write_csi_response(&body)`. The `?2031` theming reply in `handle_mode_query` becomes
  a body too.
- **`TerminalState`.** `handle_mode_query` calls `self.handler.write_csi_response(&body)`.
  Make that helper `pub(crate)` if it is not already reachable. Delete `send_decrpm` and its
  two tests, replacing them with S8C1T tests below.
- **`write_to_pty` callers.** After the change, the only remaining callers of `write_to_pty`
  are the VT52 `ESC / Z` reply and ENQ. Reduce `write_to_pty` to `pub(super)` or narrower if
  nothing outside `terminal_handler` needs it.
- List every remaining `write_to_pty` / `write_bytes_to_pty` caller in the report.

Deliverable:

- the change;
- tests:
  - DECRQM for one handler-owned mode (`?7`), one `TerminalState`-owned mode (`?2004`) and
    `?2031` in S8C1T mode starts with `0x9B` and contains no `ESC [`;
  - the same three in 7-bit mode are byte-identical to before;
  - `CSI ? u` in S8C1T mode is `0x9B ? <flags> u`;
  - unknown-mode DECRQM (`?9999$p`) stays framed.

Verification: as above.

Prohibitions: do NOT change any reply's content, only its framing; do NOT touch GUI replies
(130.7–130.8).

#### 130.4 — `HostCapabilities` on the PTY thread

Scope:

- new `freminal-common/src/host_capabilities.rs`, plus `freminal-common/src/lib.rs` (module
  declaration);
- `freminal-terminal-emulator/src/io/mod.rs` (new `InputEvent` variant);
- `freminal-terminal-emulator/src/terminal_handler/mod.rs` (field, setter, getter);
- `freminal/src/gui/pty.rs`:
  - a `PtyTabInitialState` field;
  - `apply_initial_state`;
  - the `handle_input` arm;
  - `input_event_needs_repaint`;
  - tests;
- `freminal/src/gui/notifications.rs`: `pub(crate) fn host_capabilities(config: &Config) -> HostCapabilities`,
  plus tests;
- `freminal/src/gui/settings_dispatch.rs` (broadcast);
- the five `PtyTabInitialState` construction sites:
  - `freminal/src/gui/tab_spawning.rs` (3);
  - `freminal/src/gui/app_impl.rs` (2);
  - the test site in `pty.rs`.

What:

- **`freminal-common::host_capabilities`:**

  ```rust
  pub struct HostCapabilities { pub osc99: Osc99Support }      // Debug, Clone, Copy, PartialEq, Eq, Default
  pub enum Osc99Support { #[default] Unsupported, Supported(Osc99Features) }
  pub struct Osc99Features { pub activation_report: Osc99ActivationReport, pub close_events: Osc99CloseEvents }
  pub enum Osc99ActivationReport { Reported, NotReported }
  pub enum Osc99CloseEvents { Reported, NotReported }
  ```

- **Handler:** a `host_capabilities: HostCapabilities` field (default `Unsupported`),
  `set_host_capabilities`, and `host_capabilities()`.
- **`InputEvent::HostCapabilitiesChange(HostCapabilities)`.** The consumer calls the setter.
  The repaint classifier returns false for it.
- **`host_capabilities(config)` in the GUI:**
  - `Unsupported` unless `config.notifications.enabled && config.notifications.osc_99 && routing_osc99 != Disabled`.
  - "System leg possible" means `routing_osc99` is `System`, `Both` or
    `SystemWhenUnfocused`.
  - `activation_report = Reported` iff the system leg is possible **and**
    `cfg!(all(unix, not(target_os = "macos")))`.
  - `close_events = Reported` iff the system leg is possible.
  - Doc-comment the reasoning: the activation callback exists only on the Linux/BSD D-Bus
    backend (`show_system_osc99`).
- **Seed and broadcast.**
  - Seed: add a `host_capabilities` field to `PtyTabInitialState` and set it in
    `apply_initial_state`. Every construction site uses `host_capabilities(&self.config)`.
  - Broadcast: in `apply_new_config`, if
    `host_capabilities(&new_cfg) != host_capabilities(&self.config)`, send the event to
    every pane in every window, following the `AutoDetectUrls` block's pattern.

Deliverable:

- the change;
- tests:
  - a `host_capabilities` truth table: enabled/osc_99/each routing value; the platform
    expectation uses `cfg!`;
  - `apply_initial_state` seeds the field;
  - the classifier test includes the new variant;
  - a handler setter/getter test.

Verification: as above, plus `cargo xtask check-windows` (platform `cfg`).

Prohibitions: do NOT change OSC 99 behaviour yet (130.5); do NOT add a config option.

#### 130.5 — OSC 99 `p=?` and `p=alive` gating on the PTY thread

Scope:

- `freminal-terminal-emulator/src/terminal_handler/osc.rs` (the `Notify99` arm);
- `freminal-terminal-emulator/src/terminal_handler/notify_99.rs` (new
  `fn osc99_query_reply_body`);
- `freminal/src/gui/notifications.rs`:
  - delete `OSC99_CAPABILITIES` and `osc99_query_response`;
  - delete or adapt their tests;
- `freminal/src/gui/app_impl.rs` (delete the `Query` branch);
- `freminal-common/src/buffer_states/window_manipulation.rs`: remove
  `Osc99ControlKind::Query` if it becomes unreachable, and its uses;
- tests in `freminal-terminal-emulator/tests/` (a new `osc99_query.rs`).

What:

- **`p=?` is answered in the handler.** In the `Notify99` arm, a finalized `p=?` request is
  answered right there with `write_osc_response(&body)`, and **no** window command is pushed.
  - The body is `99;i=<id or 0>:p=?;<capabilities>`.
  - `<capabilities>` comes from `osc99_query_reply_body(&Osc99Features)`. It is the current
    string with two changes:
    - `a=report` is present only when `activation_report == Reported`; with no supported
      actions the `a` key is omitted, per the spec;
    - `c=1` is present only when `close_events == Reported`.
  - Key order is unchanged.
  - When `host_capabilities.osc99 == Unsupported`, there is no reply (a debug log only).
- **`p=alive` while `Unsupported`** is dropped in the handler, with no window command and a
  debug log. `p=close` is unchanged.
- **The GUI's `Query` branch is deleted.** If `Osc99ControlKind::Query` then has no producer,
  delete the variant and its mapping. `control_kind` no longer maps `Query`, and the handler
  branches on the payload type instead.

Deliverable:

- the change;
- tests through `handle_incoming_data`:
  - `p=?` while `Unsupported` → no bytes;
  - `p=?` with full features → the exact old string;
  - with `NotReported` activation → the string without `a=report`;
  - with `NotReported` close → without `c=1`;
  - `p=?` then `CSI c` in one buffer → the OSC 99 reply precedes DA1;
  - S8C1T framing (`0x9D … 0x9C`);
  - `p=alive` while `Unsupported` → no window command;
  - `p=alive` while `Supported` → `Osc99Control { kind: Alive }` as before.

Verification: as above.

Prohibitions: do NOT change any other OSC 99 key or payload type (Task 138); do NOT move
the alive/activation/close reports yet (130.8).

#### 130.6 — Typed `GuiReply` and the handler serialiser

Scope:

- new `freminal-terminal-emulator/src/io/gui_reply.rs`, plus `io/mod.rs` (module and the
  `InputEvent::Reply` variant);
- `freminal-terminal-emulator/src/terminal_handler/pty_writer.rs` (the serialiser);
- `freminal-terminal-emulator/src/interface.rs` (`TerminalEmulator::write_gui_reply`);
- `freminal/src/gui/pty.rs` (the `handle_input` arm, classifier and tests).

What:

- **The `GuiReply` type:**

  ```rust
  pub enum GuiReply {
      WindowState(WindowStateReport),                 // CSI 1 t / CSI 2 t
      WindowPosition { x: usize, y: usize },          // CSI 3 ; x ; y t
      WindowSizePixels { height: usize, width: usize }, // CSI 4 ; h ; w t
      ScreenSizePixels { height: usize, width: usize }, // CSI 5 ; h ; w t
      IconLabel(String),                              // OSC L <label> ST
      WindowTitle(String),                            // OSC l <title> ST
      Clipboard { selection: String, base64_payload: String }, // OSC 52 ; sel ; payload ST
      Osc99Activation { id: Option<String>, button: Option<String> }, // OSC 99 ; i=<id> ; <button> ST
      Osc99Closed { id: Option<String>, tracking: Osc99CloseTracking }, // OSC 99 ; i=<id>:p=close ; [untracked] ST
      Osc99Alive { request_id: Option<String>, live_ids: Vec<String> }, // OSC 99 ; i=<id>:p=alive ; a,b ST
  }
  pub enum WindowStateReport { Normal, Iconified }
  pub enum Osc99CloseTracking { Tracked, Untracked }
  ```

- **Serialising.**
  - `TerminalHandler::write_gui_reply(&mut self, reply: &GuiReply)` builds each body and
    sends it through `write_csi_response` / `write_osc_response`.
  - Missing ids default to `0`, as the GUI builders did.
  - The bodies are byte-identical to today's GUI strings without their 7-bit framing.
- `TerminalEmulator::write_gui_reply` delegates to the handler.
- **`InputEvent::Reply(GuiReply)`.** The consumer calls `emulator.write_gui_reply`. The
  repaint classifier returns false. The event is not recorded to FREC.

Deliverable:

- the type and serialiser;
- a table test over every variant in 7-bit and 8-bit mode. The 7-bit bytes must equal the
  current GUI builders' output; copy the expected strings from `rendering.rs` and
  `notifications.rs`.

Verification: as above.

Prohibitions: do NOT migrate any GUI sender yet (130.7, 130.8).

#### 130.7 — GUI window, title and OSC 52 replies through `GuiReply`

Scope:

- `freminal/src/gui/rendering.rs`:
  - `handle_window_manipulation` signature and the report arms;
  - delete `send_pty_response`;
- `freminal/src/gui/frame_drain.rs` (call site, vec element types, tests);
- doc comments that name `pty_write_tx` for replies:
  - `rendering.rs`;
  - `panes/mod.rs:133`;
  - `pty.rs` (the `TabChannels` doc);
  - `freminal/src/main.rs:23`;
  - `freminal-terminal-emulator/src/io/mod.rs` (the `WindowCommand::Report` doc).

What:

- `handle_window_manipulation` takes `reply_tx: &Sender<InputEvent>` (the pane's
  `input_tx`) instead of `pty_write_tx`.
- Each report arm sends `InputEvent::Reply(GuiReply::…)` with the same values it formats
  today.
- The `osc99_*` vectors carry `Sender<InputEvent>`; their element types change here.
- `Pane::pty_write_tx` is documented as "layout startup-command injection only".

Deliverable:

- the change;
- a test calling `handle_window_manipulation` with a `ReportTitle` / `QueryClipboard` command
  and asserting the `InputEvent::Reply` received.

Verification: as above.

Prohibitions: do NOT change the OSC 52 security gating or toast events; do NOT touch the
OSC 99 senders beyond the vector element type (130.8).

#### 130.8 — OSC 99 reports through `GuiReply`, and the weak notify handle

Scope:

- `freminal/src/gui/panes/mod.rs`: a new `reply_tx: Arc<Sender<InputEvent>>` field, its
  construction, and every `Pane` literal in tests and benches:
  - `panes/mod.rs`;
  - `frame_drain.rs`;
  - `actions.rs`;
  - `tabs.rs`;
  - `freminal/benches/pane_resolution_bench.rs`;
- `freminal/src/gui/pty.rs` (`TabChannels`);
- `freminal/src/gui/app_impl.rs` (the `Alive` branch);
- `freminal/src/gui/notifications.rs`:
  - `route_osc99`;
  - `show_system_osc99`;
  - `osc99_action_report`;
  - delete the `osc99_activation_report` / `osc99_close_report` / `osc99_alive_report` byte
    builders and their tests, replacing them with `GuiReply` builders;
- `freminal/src/gui/rendering.rs` (which sender is pushed with the OSC 99 vectors).

What:

- `Pane::reply_tx` is an `Arc` holding a clone of the pane's `input_tx`. The pane is its only
  strong owner. Doc-comment why it exists (consumer-thread liveness).
- The OSC 99 notification and control vectors carry `Weak<Sender<InputEvent>>`, from
  `Arc::downgrade(&pane.reply_tx)`.
- **Alive:** `app_impl` upgrades the weak handle and sends `GuiReply::Osc99Alive`.
- **Notify thread:** `show_system_osc99` moves the `Weak` into `freminal-notify-99`, which
  upgrades it per send.
  - `osc99_action_report` returns `Option<GuiReply>`.
  - A failed upgrade drops the reply with a `debug!`.

Deliverable:

- the change;
- tests:
  - `osc99_action_report` returns the expected `GuiReply` for `__closed`, `default` and a
    button id, each with its gating flag;
  - dropping the `Arc` makes `upgrade()` fail, so no event is sent (a unit test of the
    send helper);
  - the alive path sends `GuiReply::Osc99Alive` with sorted ids.

Verification: as above, plus `cargo xtask check-windows` (the macOS/Windows branch of
`show_system_osc99` changes).

Prohibitions: do NOT change OSC 99 routing, toasts or the live map.

#### 130.9 — Escape-sequence dual-doc and reference update

Scope:

- `Documents/ESCAPE_SEQUENCE_COVERAGE.md`;
- `Documents/ESCAPE_SEQUENCE_GAPS.md`;
- `Documents/KITTY_PROTOCOL_REFERENCE.md` (the OSC 99 query section only).

What:

- Record:
  - S8C1T framing for DECRPM, `CSI ? u`, and all GUI replies;
  - tmux passthrough via the real parser, with unwrapped replies;
  - `p=?` answered in stream order and gated on configuration and platform;
  - `p=alive` gated.
- Refresh "Last updated" in all three documents.

Verification: the pre-commit markdownlint and prettier hooks pass on the three files.

Prohibitions: do NOT touch code.

### 130 Status notes

- **130.1 — Complete (2026-10-10), commit `d45d9389`.**
  - `TerminalState::process_parsed_outputs` runs handler, mode sync and the RIS state reset
    per output.
  - Three new regression tests were confirmed failing against the old code.
  - Benchmarks within ±5% of `before_130_1`.
- **130.2 — Complete (2026-10-10), commit `7b869c20`.**
  - The handler queues each un-doubled tmux payload, and `TerminalState` parses it with a
    fresh, seeded parser right after the producing output, up to depth 4.
  - `dispatch_tmux_csi`, `wrap_tmux_passthrough`, `double_esc` and `in_tmux_passthrough` are
    deleted.
  - A table-driven test proves every old direct-dispatch and fall-through shape behaves
    identically wrapped and direct.
  - Seven new tests were confirmed failing against the old code. One of them shows reparsed
    OSCs used to miss the same batch's window-command drain.
  - The orchestrator rewrote the mid-sequence splice test to use a sequence the old code
    actually reparsed.
  - Benchmarks within +1.2% of `before_130_1`.
- **130.3 — Complete (2026-10-10), commit `6bb4e529`.**
  - `ReportMode::report()` returns the body.
  - All DECRPM (handler and `TerminalState`) and `CSI ? u` replies go through
    `write_csi_response`, and `send_decrpm` is deleted.
  - Only VT52 `ESC / Z` and ENQ still call `write_to_pty`.
  - Two pre-existing reply-content bugs surfaced: 130.C1 and 130.C2.
- **130.4 — Complete (2026-10-10), commit `790a77e6`.**
  - `HostCapabilities` (`freminal-common`) is seeded through `PtyTabInitialState` at all five
    spawn sites and re-sent with `InputEvent::HostCapabilitiesChange` from `apply_new_config`.
  - The resolver later moved to `freminal/src/gui/host_capabilities.rs` (review finding 15).
- **130.5 — Complete (2026-10-10), commit `ebd361e8`.**
  - `p=?` is answered in `dispatch_finalized_osc99` with conditional `a=report` / `c=1`;
    `Osc99ControlKind::Query` and the GUI query path are deleted.
  - Review finding 14 later extended the gate so every OSC 99 request is dropped while
    unsupported.
- **130.6 — Complete (2026-10-10), commit `8b5b165e`.** `GuiReply`, `InputEvent::Reply` and the
  handler serialiser `write_gui_reply`, with byte-identical 7-bit output.
- **130.7 — Complete (2026-10-10), commit `165cd17f`.** Window, title, OSC 52 and OSC 99
  replies are sent as `GuiReply`; the GUI byte builders and `send_pty_response` are deleted.
- **130.8 — Complete (2026-10-10), commit `630469cf`.** `Pane::reply_tx` (`Arc`) with `Weak`
  handles for OSC 99 consumers; review fixes in `120d90fa` (single reply handle) and
  `dede495e` (a liveness test on a real `Pane`).
- **130.9 — Complete (2026-10-10), commit `1e03dd31`.** COVERAGE, GAPS and the kitty reference.
- **Cleanups:**
  - 130.C1 (`ce0e8cad`), 130.C2 and 130.C3 (`120d90fa`), and 130.C5 and 130.C6 (`74149b30`)
    are resolved.
  - 130.C4 is routed to Task 138.

### 130 Cleanup entries

#### 130.C1 — DECRQM for LNM (ANSI mode 20) answers in the DEC-private form

- **Surfaced:** 130.3 (2026-10-10). Predates Task 130.
- **Impact:**
  - `Lnm::report` returns `?20;Ps$y`. LNM is an ANSI mode, so `CSI 20 $ p` must be answered
    with `CSI 20 ; Ps $ y`, without `?`; IRM (`4;Ps$y`) already does this correctly.
  - An application parsing the reply can treat it as an answer about DEC private mode 20.
- **Scope of fix:** `freminal-common/src/buffer_states/modes/lnm.rs` and its tests; any
  emulator test that pins the `?20` form.
- **Verification:** `CSI 20 $ p` yields `CSI 20 ; 2 $ y` by default and `CSI 20 ; 1 $ y` after
  `CSI 20 h`.
- **Scheduling:** in the Task 130 worktree, before the adversarial review.

#### 130.C2 — DECRQM for DECSCLM answers "not recognised"

- **Surfaced:** 130.3 (2026-10-10). Predates Task 130.
- **Impact:**
  - `Decsclm::report` always returns `?4;0$y` ("not recognised"), although freminal parses
    DECSCLM and deliberately does not implement smooth scrolling.
  - The DECRPM value for a recognised mode that can never be set is `4` ("permanently
    reset"), as `?2027` already uses `3` for permanently set.
- **Scope of fix:** `freminal-common/src/buffer_states/modes/decsclm.rs` and its tests;
  `freminal-common/tests/mode_boilerplate_tests.rs`.
- **Verification:** `CSI ? 4 $ p` yields `CSI ? 4 ; 4 $ y` whatever the set/reset history.
- **Scheduling:** in the Task 130 worktree, before the adversarial review.
- **Scope widened (maintainer, 2026-10-10):** the C1 implementer found that `CSI ? 4 $ p` is
  never answered at all. `Mode::Decsclm(_)` falls into the "not acted on" arms of both
  `TerminalHandler` and `TerminalState::sync_mode`, so `Decsclm::report` is unreachable. The
  fix adds a `Mode::Decsclm(Decsclm::Query)` arm in `terminal_handler/mod.rs` that answers via
  `write_csi_response`.

#### 130.C3 — Title, icon-label and OSC 52 replies echo control bytes

- **Surfaced:** orchestrator re-review of 130.6 (2026-10-10). Predates Task 130.
- **Impact:**
  - An application can set the title `A ESC [6n B`; the OSC parser keeps an ESC that is not
    followed by `\` (129.C5). `CSI 21 t` then echoes that ESC back into the application's
    input, the title-report injection class (CVE-2003-0063).
  - `OSC 52 ; c ESC [6n ; ?` likewise echoes the selection parameter verbatim.
  - Reproduced: the title is stored as `"A\u{1b}[6nB"` and the selection as `"c\u{1b}[6n"`.
- **Scope of fix:**
  - `freminal-terminal-emulator/src/terminal_handler/pty_writer.rs` (`write_gui_reply`), the
    single point every GUI reply passes through;
  - its tests.
- **Approach:**
  - `IconLabel` / `WindowTitle`: drop every C0 control, DEL and C1 control (U+0080–U+009F)
    before framing.
  - `Clipboard`: keep only the characters xterm defines for `Pc` (`c p q s 0`–`7`), dropping
    everything else. An empty result is sent as empty; xterm's "empty means `s0`" rule
    concerns the request, not the reply.
- **Verification:** a title or selection carrying ESC, BEL, `0x9B` (as U+009B) and DEL produces
  a reply containing none of them, in 7-bit and 8-bit mode; ordinary titles are unchanged.
- **Scheduling:** in the Task 130 worktree, before the adversarial review.

#### 130.C4 — OSC 99 report flags cross the thread boundary as `bool`

- **Surfaced:** orchestrator re-review of 130.7 (2026-10-10). Predates Task 130.
- **Impact:**
  - `Notification99Data::{report_activation, focus_on_activation, close_report}` and the
    parser state in `osc_notify_99.rs` are raw `bool`s, carried from the PTY thread to the GUI.
  - `osc99_action_report` and `Osc99LiveEntry` take or store the same `bool`s.
  - This violates `freminal-state-representation` for transported state.
- **Scope of fix:** the OSC 99 data model across `freminal-common`
  (`window_manipulation.rs`, `osc_notify_99.rs`), `terminal_handler/notify_99.rs` and
  `freminal/src/gui/notifications.rs`.
- **Routing:** Task 138 (OSC 99 conformance) rewrites exactly this data model; doing it there
  avoids changing it twice. Not scheduled in Task 130.

#### 130.C5 — XTGETTCAP echoes the raw request on an invalid name

- **Surfaced:** 130 adversarial review (2026-10-10), finding 7. Predates Task 130.
- **Impact:**
  - For a name that is not valid hex, `handle_xtgettcap` replies `DCS 0 + r <raw request> ST`,
    echoing the request bytes lossily decoded.
  - Reproduced: `DCS + q ESC [6n BEL zz ST` gets the reply `DCS 0 + r ESC [6n BEL zz ST`, so
    an application can inject input into itself (the CVE-2003-0063 class, as 130.C3).
- **Fix:** follow xterm's ctlseqs, which specify `DCS 0 + r ST` for invalid requests: reply
  with no echo when the name is not valid hex. A name that _is_ valid hex but unknown keeps
  today's hex echo, since hex digits cannot carry a control.
- **Scope:** `terminal_handler/dcs.rs` (`handle_xtgettcap`) and tests.
- **Scheduling:** in the Task 130 worktree, before merge.

#### 130.C6 — A DECRQM for LNM overwrites the LNM state used for input encoding

- **Surfaced:** 130 adversarial review (2026-10-10), finding 1. Predates Task 130.
- **Impact:**
  - `TerminalState::sync_mode` assigns `Mode::LineFeedMode(v)` straight into
    `modes.line_feed_mode` and does not exclude `Lnm::Query`.
  - Reproduced: after `CSI 20 h`, a `CSI 20 $ p` leaves `line_feed_mode == Lnm::Query`, so
    Enter is encoded as if LNM were reset.
- **Fix:**
  - The query must not touch the stored state; route `Lnm::Query` to the handler-owned
    (no-op) arm.
  - Audit every other `sync_mode` assignment arm for the same pattern and pin each with a
    test.
- **Scope:** `state/internal.rs` and tests.
- **Scheduling:** in the Task 130 worktree, before merge.

### 130 Adversarial review (2026-10-10)

An adversarial review of `2862acb9..1e03dd31` found no blocker, one major and fourteen
minor/nit findings. Two were reproduced by the orchestrator (1 → 130.C6, 7 → 130.C5).
Disposition, all addressed before merge:

1. **MAJOR — LNM DECRQM clobbers state:** 130.C6.
2. **RIS drops the parser's in-flight state and the UTF-8 tail.** The behaviour belongs to
   the 131 reset table, which has a row for it, so 131.8 fixes it. 130 corrects the
   `apply_state_reset` comment, which understated it. **Fixed by 131.8** (the parser is no
   longer replaced; only its DECANM/S8C1T modes reset); the 131 merge kept 131's comment.
3. **The fresh parser's VT52/S8C1T seeding is untested:** add tests.
4. **`run_case(.., wrapped: bool)`:** becomes a `Delivery` enum.
5. **The liveness tests exercise `std`, not `Pane`:** add a test on a `Pane` built with
   `from_channels`.
6. **OSC 99 `id` / `button` / `live_ids` are echoed unsanitised, relying on validation in
   another crate:** the serialiser keeps only identifier characters, and an empty id is sent
   as `0` (also in the `p=?` reply).
7. **XTGETTCAP echo:** 130.C5.
8. **Mouse/focus/key event encodings are not S8C1T-aware**, while the docs say "every
   reply": the docs are scoped to replies to queries, and a GAPS row records the event
   encodings (unscheduled).
9. **Stale docs:** `TerminalEmulator::clone_write_tx` and `window_ops.rs`.
10. **Plan and doc inaccuracies:** 130 status notes, the Task 138 stub, the common-work table,
    the COVERAGE attribution of the OSC 99 report framing to 130.8, and the
    `freminal-version-activation` skill's reverse-write guidance. Fixed by the orchestrator.
11. **`config_example.toml` does not say OSC 99 queries go unanswered while notifications
    are off:** add a comment.
12. **`GuiReply::WindowState` doc names `CSI 18 t`;** the query is `CSI 11 t`.
13. **`TerminalHandler::process_outputs` doc:** a tmux payload is only queued there.
14. **While OSC 99 is `Unsupported`, display payloads and `p=close` still reach the GUI**
    (which then caches icons when routing is disabled). **Decision changed:** the handler
    drops every OSC 99 request while `Unsupported`, so the terminal is consistently one that
    does not speak the protocol. This supersedes the earlier "`p=close` is still forwarded"
    bullet. Tests cover `p=close` and display payloads while `Unsupported`, and `p=?` for a
    tombstoned id.
15. **Nits:**
    - the fully-qualified `Decsclm` path in `internal.rs`;
    - the vacuous `tmux_passthrough_queue_is_drained_by_handle_incoming_data` test (delete);
    - the `QueryClipboard` debug log prints the unescaped selection (escape and bound it);
    - `host_capabilities(&Config)` moves out of `gui/notifications.rs` into its own module
      (`gui/host_capabilities.rs`), since later tasks add non-notification facets.
    - Accepted: `is_control_payload -> bool` stays. A predicate returning `bool` is not a
      bool field or parameter.
    - Already routed: the bool fields in the new tests (130.C4).

### 130 Confirmation review (2026-10-10)

A second read-only review re-checked every adversarial-review finding against the fix
commits. All were FIXED except items 10 and R7, which were PARTIAL. It also found eight new
items, all addressed in `5d5fba25` and the follow-up docs commit:

- **N1:** two OSC 99 tests ran with OSC 99 unsupported and passed vacuously. They now use
  supported capabilities and carry positive controls.
- **N2:** an IRM DECRQM stored `Irm::Query`, switching insert mode off, and sent no reply.
  It is now answered `CSI 4 ; Ps $ y` without touching the state. This closes the IRM half
  of issue #528.
- **N3:** the docs still said only `p=alive` was ignored while unsupported, and did not
  record 130.C5 or 130.C6. Corrected.
- **N4:** the `Osc99Control` doc still mentioned `p=?`. Corrected.
- **N5:** the `Unsupported` gate ran after reassembly, so a transfer begun while unsupported
  could complete after support was enabled. The gate now runs before reassembly, and
  `set_host_capabilities` discards pending transfers when support is turned off.
- **N6 (accepted):** XTGETTCAP answers each name separately, like kitty, so two invalid names
  give two identical bare replies. That matches kitty's per-name behaviour.
- **N7 (accepted, recorded in GAPS):** in S8C1T mode a UTF-8 title reply can contain bytes in
  0x80–0x9F. This predates Task 130 and is niche.
- **N8:** a cosmetic doc wrap in `window_ops.rs`. The `MASTER_PLAN.md` mention of
  `write_to_pty` / `pty_write_tx` is a historical v0.11.0 dependency note and stays.

### 130 Orchestrator re-review (2026-10-10)

The maintainer found that 130.2–130.8 had been committed after a partial review: diffs read
truncated, new untracked files never opened, verification steps taken on trust. Every
commit was then re-read in full. Results:

- **130.2:** pass. One gap, R2a: no test pins that an _incomplete_ inner tmux sequence is
  discarded by the fresh parser instead of leaking into the outer stream.
- **130.3:** pass. The 33 `ReportMode` diffs were checked mechanically; every change is the
  `ESC [` removal or its rustfmt re-wrap.
- **130.4:** pass. The `apply_new_config` broadcast has no test, as no existing broadcast does:
  there is no `FreminalGui` test harness, and extracting a helper only to test it is what
  `freminal-extend-or-extract` forbids. Accepted.
- **130.5:** pass.
- **130.6:** pass, but surfaced 130.C3.
- **130.7:** pass, but surfaced 130.C4, plus R7: stale comments that still route `p=?` to
  the GUI (`Osc99Control` doc in `notifications.rs`, `osc99_controls` doc in
  `frame_drain.rs`).
- **130.8:** three findings:
  - R8a: `handle_window_manipulation` takes both `reply_tx: &Sender` and
    `reply_handle: &Arc<Sender>` for the same channel; keep only the `Arc`.
  - R8b: the new tests exercise `std`'s `Arc`/`Weak`, not freminal; add a test that the
    handles `handle_window_manipulation` collects stop upgrading once the pane's handles drop.
  - R8c: an over-long doc line in `show_system_osc99`.

R2a, R7 and R8a–c are fixed with 130.C2 and 130.C3 before the adversarial review.

---

## Task 131 — Screen-Scoped State & Reset Lifecycle

### 131 Summary

Give per-screen state one mechanism. Make the three alternate-screen modes behave as xterm
and Ghostty do. Make RIS and DECSTR reset an explicit, compiler-checked table instead of two
hand-maintained lists. Activated and decomposed on 2026-10-10 against `2862acb9`.

### 131 Activation recon (2026-10-10)

- **`?47`, `?1047` and `?1049` are one code path.**
  - `?47` and `?1047` parse to `Mode::AltScreen47`.
  - All three reach `handle_enter_alternate` / `handle_leave_alternate`.
  - Every entry saves the cursor, installs a blank alternate store and homes the cursor
    (twice: `CursorState::default()`, then `reset_scroll_region_to_full`).
  - Every leave drops the alternate rows and images and restores the primary cursor.
  - DECRQM for `?1047` reports `?47`.
- **B15 is confirmed.** `Buffer::enter_alternate` is idempotent; `handle_enter_alternate` is
  not. A second `?1049h` moves the alternate keyboard stack over the parked main stack, and
  the main stack is lost.
- **Handler state that should be per-screen but is global.**
  - `virtual_placements` is never touched on a switch.
  - `real_placements` is filtered on leave only.
  - Kitty `d=a`/`d=A` clears both screens' placements.
  - `saved_character_replace` (part of DECSC) is a single slot.
- **The buffer's DECSC slot is cloned on entry**, so the alternate screen inherits it, and
  alternate-side saves are discarded on leave. The scroll region is swapped per screen.
- **RIS misses a lot.**
  - Transfer state: `kitty_transfer`, iTerm2 `multipart_state`.
  - Modes: `insert_mode`, `nrc_mode`, `reverse_wrap`, `xt_rev_wrap2`, `vt52_mode`,
    `s8c1t_mode` (the parser is reset but the handler mirror is not),
    `in_band_resize_enabled`, `sixel_display_mode`, `private_color_registers`,
    `sixel_shared_palette`, `allow_alt_screen`.
  - Exact semantics are pinned by 131.1.
- **DECSTR** documents that it deliberately leaves the kitty keyboard stack.

#### Reference behaviour (read from source, 2026-10-10)

| Behaviour                          | xterm `charproc.c` | Ghostty `Terminal.zig` | WezTerm `terminalstate` | kitty `screen.c`    | freminal today |
| ---------------------------------- | ------------------ | ---------------------- | ----------------------- | ------------------- | -------------- |
| 47/1047 enter: save, clear?        | no, no             | no, no                 | no, no                  | no, no              | yes, yes       |
| 1047 leave clears alt              | yes                | yes                    | yes                     | no                  | (always new)   |
| 1049 enter: save, switch, clear    | yes                | yes                    | yes                     | yes                 | yes            |
| 1049 leave: switch, restore        | yes                | yes                    | yes                     | yes                 | yes            |
| Cursor position across a switch    | kept               | kept (copied)          | kept (1049: homed)      | homed, reset        | homed          |
| 2nd `1049h` while in alt           | save + clear       | save + clear           | no-op                   | no-op               | no-op          |
| `1049l` while on primary           | DECRC              | DECRC                  | no-op                   | no-op               | no-op          |
| Alt contents persist across uses   | yes                | yes                    | yes                     | yes                 | no             |
| DECSC slot                         | per screen         | per screen             | per screen              | per screen          | shared-ish     |
| Scroll margins                     | shared             | shared                 | shared                  | shared              | per screen     |
| Kitty keyboard stack               | –                  | –                      | –                       | per screen, persist | swapped, fresh |
| DECSTR clears kitty keyboard flags | –                  | –                      | –                       | yes (both)          | no             |

### 131 Decisions (maintainer, 2026-10-10)

- **Follow the consensus of xterm, Ghostty, WezTerm and kitty.** Where they split 2–2, follow
  xterm and Ghostty: xterm defines these modes, and Ghostty matches it deliberately.
  Concretely:
  - **`?47`:** switch screens only. No save, no clear; the cursor position and attributes
    are kept.
  - **`?1047`:** on enter, the same as `?47`. On leave, if on the alternate screen, clear it,
    then switch to the primary.
  - **`?1049` enter:**
    1. DECSC on the current screen; when already in alt this saves into the **alt** slot;
    2. switch to alternate (a no-op if already there);
    3. clear the alternate screen.

    The cursor position is kept. A second `?1049h` therefore re-saves and clears, and the
    main screen's save is untouched.

  - **`?1049` leave:** switch to primary (a no-op if already there), then DECRC
    unconditionally.
  - **Alternate contents persist** across sessions. Only `?1049`-enter and `?1047`-leave
    clear them.
  - **The DECSC slot is per screen.** The scroll margins are **shared** and are not touched
    by a switch.
- **DECSTR follows kitty for kitty state.** It clears both kitty keyboard stacks. Tasks 139
  and 103 do the same for pointer-shape stacks and extra cursors. Non-kitty state keeps the
  VT510 Table 5-9 behaviour.
- **The alternate kitty keyboard stack persists** between alternate sessions, as kitty's
  `alt_key_encoding_flags` does.
- **tmux CSI unification** was absorbed into Task 130 (130.2).

### 131 Decisions (orchestrator, recorded so they are not re-litigated)

- **`ScreenScoped<T> { primary: T, alternate: T }`** has no notion of an "active" screen. It
  is indexed with `get(BufferType)` / `get_mut(BufferType)`, and the handler passes
  `self.buffer.kind()`. The buffer is the single source of truth for which screen is active,
  so enter and leave cannot drift and are idempotent by construction; that fixes B15
  structurally. Module: `terminal_handler/screen_scoped.rs`.
- **Buffer API.** `enter_alternate(scroll_offset)` and `leave_alternate() -> usize` are
  replaced by:
  - `switch_to_alternate()` and `switch_to_primary()`, which are idempotent, keep the cursor
    position and attributes, and leave margins alone;
  - `clear_alternate_screen()`, valid only while alt is active.

  The `scroll_offset` round trip is dead (its only caller passes `0` and discards the
  result) and is deleted.

- **Parked screens.**
  - `SavedPrimaryState` becomes
    `ParkedScreen { rows, reflow_anchor, saved_cursor, image_store, image_cell_count, blocks, next_block_id }`.
  - The `Buffer` holds `parked_primary: Option<ParkedScreen>` and
    `parked_alternate: Option<ParkedScreen>`.
  - Invariant: exactly the inactive screen may be parked. The alternate may also be absent
    (never entered, or reset).
  - Margins are no longer parked. The cursor is not restored from a parked screen either;
    it keeps its screen position across a switch.
  - `ParkedScreen` does carry `reflow_anchor: CursorState`: the cursor the screen had when it
    was parked, used **only** to anchor reflow and height-shrink trimming while a parked
    screen is resized (`resize_saved_primary` / `resize_parked_alternate` feed it to the
    temporary buffer). It is never restored. Without it, a home cursor would make a shrink
    trim the wrong rows.
  - The image store is parked with `mem::take`, not cloned.
- **The cursor across a switch** keeps its **screen** coordinates:
  1. `cursor_screen_pos()` on the source screen;
  2. `visible_window_start(0) + y` on the target;
  3. push `ScrollFill` rows if the target store is shorter, as `restore_cursor` does.

  The x position and all cursor fields are carried unchanged.

- **Clearing** installs a fresh alternate `RowStore` at the old store's `next_number()`.
  Row numbers stay unique, so no mark, placement or horizon can alias the blank rows. Clearing
  also empties the alternate image store and drops alternate-namespace marks. The handler
  drops the alternate screen's placement maps on clear. Blank cells were first
  default-attributed; **superseded by adversarial-review finding 15:** the clear applies the
  current background (BCE), as xterm and Ghostty do.
- **Marks.** Alternate-namespace prompt marks and command blocks are still dropped on every
  leave (`drop_alternate_marks`). The GUI never shows gutters on the alternate screen, and
  keeping them would resurrect stale marks.
- **Resizing a parked alternate is eager.** `set_size` on the primary also resizes a parked
  alternate store, through `resize_parked_alternate`, which mirrors `resize_saved_primary`.
  It uses a temporary `Buffer` with `kind: Alternate`, so the alternate no-reflow branch runs.
  That keeps the invariant that every store matches the buffer's dimensions. The handler
  prunes the parked alternate placement map against the store's new base afterwards.
- **Kitty placements are per screen.** `virtual_placements` and `real_placements` become
  `ScreenScoped`.
  - `d=a`/`d=A` and the pruners act on the active screen only, as kitty's per-screen
    `grman` does.
  - `kitty_transfer` stays global: it is stream state, not screen state. RIS clears it.
- **The reset table is exhaustive destructuring.**
  - `TerminalHandler::reset(kind: ResetKind)` destructures `self` with no `..`.
  - Every field is either reset (with the per-kind rule) or bound to `_` with a comment
    giving the reason it survives.
  - Adding a field without classifying it is then a compile error. That is the explicit
    reset table the stub asked for.
  - `ResetKind { Hard, Soft }` lives in the same module.
  - `full_reset` and `soft_reset` become thin wrappers.
- **`ResetKind` stays handler-internal** (`pub(crate)`): nothing outside the emulator crate
  needs it.

### 131 Subtasks

The verification, standing prohibitions and stop condition are as for Task 130. Order:

1. 131.1, the audit; its findings fill the reset table.
2. 131.2, then 131.3.
3. 131.4.
4. 131.5, then 131.6, then 131.7.
5. 131.C4, then 131.8 together with 131.C3.
6. The cleanup entries 131.C1 and 131.C2.
7. 131.9, then 131.10.

#### 131.1 — RIS / DECSTR semantics audit (READ-ONLY)

Scope: read-only.

- `TerminalHandler` and `Buffer` fields, and the `TerminalState` RIS block;
- xterm `charproc.c` (`ReallyReset`, `VTReset`) and `cursor.c`;
- kitty `screen.c` (`do_screen_reset`).

What: for every `TerminalHandler` field and every `Buffer` field, give:

- its RIS behaviour;
- its DECSTR behaviour;
- the source line in xterm (or kitty, for kitty-protocol state) that justifies it;
- what freminal does today.

Flag every divergence.

Deliverable: the table, which the orchestrator copies into "131 Reset table" below.

Prohibitions: do NOT edit files.

#### 131.2 — `ScreenScoped<T>`

Scope: new `freminal-terminal-emulator/src/terminal_handler/screen_scoped.rs`, plus the
module declaration in `terminal_handler/mod.rs`.

What:

- `pub(crate) struct ScreenScoped<T> { primary: T, alternate: T }` with these methods:
  - `new(primary, alternate)`;
  - `get(&self, BufferType) -> &T`;
  - `get_mut(&mut self, BufferType) -> &mut T`;
  - `both_mut(&mut self) -> [&mut T; 2]`.
- Derive `Debug`, `Clone`, `Default` where `T` allows.
- Add `pub fn kind(&self) -> BufferType` to `Buffer` if it does not exist. This is a
  one-line addition in `freminal-buffer/src/buffer/mod.rs`, added to scope.
- If clippy flags the type as unused, gate it with
  `#[cfg_attr(not(test), expect(dead_code))]` plus a `TODO(131.3)` comment.

Deliverable: the type, with unit tests for indexing and independence.

#### 131.3 — Kitty keyboard stack per screen (fixes B15)

Scope:

- `freminal-terminal-emulator/src/terminal_handler/mod.rs`:
  - fields;
  - the four keyboard arms;
  - `kitty_keyboard_flags`;
  - `full_reset` / `soft_reset`;
  - the `soft_reset` doc comment;
- `freminal-terminal-emulator/src/terminal_handler/scroll_ops.rs` (delete the hand swap);
- new `freminal-terminal-emulator/src/terminal_handler/kitty_keyboard_stack.rs`;
- `freminal-terminal-emulator/tests/kitty_keyboard_stack.rs`.

What:

- **`KittyKeyboardStack`** wraps `Vec<u32>`. Its methods (`current`, `push`, `pop`, `set`,
  `clear`) carry exactly today's arm logic, including `MAX_STACK_DEPTH` eviction and the
  set-on-empty push.
- **Fields.** `kitty_keyboard_stack: ScreenScoped<KittyKeyboardStack>` replaces both old
  fields, and `kitty_keyboard_flags()` reads the active one.
- **Switching** no longer touches the stacks. The alternate stack persists between
  sessions.
- **RIS and DECSTR** clear both stacks. Update the `soft_reset` doc comment.

Deliverable:

- the change;
- tests:
  - B15: `?1049h`, `CSI >5u`, `?1049h`, `?1049l` restores the main stack;
  - alternate persistence: push in alt, leave, re-enter, and the flags are still there;
  - main unaffected by alt pushes;
  - DECSTR clears both;
  - RIS clears both;
  - the existing `alternate_screen_gets_independent_stack` updated to the persistence
    semantics.

Prohibitions: do NOT change the `KittyKeyboardQuery` reply (framed by 130.3).

#### 131.4 — Distinguish `?1047` from `?47`

Scope:

- `freminal-common/src/buffer_states/mode.rs`;
- `freminal-common/src/buffer_states/modes/xtextscrn.rs` (the new `AltScreen1047` type);
- their tests;
- `freminal-terminal-emulator/src/terminal_handler/mod.rs` (the mode arms: route
  `AltScreen1047` exactly as `AltScreen47` for now; the DECRQM arm);
- `freminal-terminal-emulator/src/state/internal.rs` (the `sync_mode` handler-owned list);
- `freminal-terminal-emulator/tests/modes_exhaustive.rs` and `tests/modes_unit.rs` if they
  enumerate modes.

What:

- `?1047` parses to `Mode::AltScreen1047(AltScreen1047)`, with the same `Alternate` /
  `Primary` / `Query` shape as `AltScreen47`.
- DECRQM for `?1047` reports `?1047;Ps$y`. Set means the alternate screen is active, as for
  47 and 1049 (xterm `DP_X_ALTBUF`).
- Behaviour is otherwise unchanged in this subtask.

Deliverable: parse, display and report tests, and a DECRQM `?1047` test.

#### 131.5 — Buffer: parked screens and the switch primitives

Scope:

- `freminal-buffer/src/buffer/mod.rs` (`ParkedScreen`, fields, invariants, tests);
- `freminal-buffer/src/buffer/resize_and_alt.rs`;
- `freminal-buffer/src/buffer/lifecycle.rs` (`full_reset`, `debug_assert_invariants`);
- `freminal-buffer/src/buffer/cursor.rs` (only if a helper is needed);
- `freminal-buffer/src/buffer/scroll.rs` (only so that `reset_scroll_region_to_full` is no
  longer called on a switch);
- buffer tests that pin the old semantics:
  - `freminal-buffer/src/buffer/*_tests.rs`;
  - `freminal-buffer/tests/scroll_region_edge_cases.rs`;
- `freminal-buffer/benches/buffer_row_bench.rs` (`bench_alternate_screen_switch`: add a
  re-entry case);
- `freminal-terminal-emulator/src/terminal_handler/scroll_ops.rs`: mechanical call-site
  update only. Enter becomes `save_cursor` + `switch_to_alternate` + `clear_alternate_screen`;
  leave becomes `switch_to_primary` + `restore_cursor`. This is the `?1049` shape for every
  mode until 131.6;
- tests in `freminal-terminal-emulator` that call `enter_alternate` / `leave_alternate`
  directly.

What: implement the "Buffer API", "Parked screens", "cursor across a switch", "Clearing",
"Marks" and "Resizing a parked alternate" decisions above.

- **`full_reset`** drops both parked screens and advances `next_alt_base` past a parked
  alternate's `next_number()`.
- **Invariants.** `debug_assert_invariants` asserts:
  - the parked/active exclusivity;
  - parked store dimensions;
  - `image_cell_count` per store.

Deliverable:

- the change;
- buffer tests:
  - alternate rows and images persist across leave/enter;
  - `clear_alternate_screen` gives blank rows with unique, monotonic row numbers;
  - the cursor screen position is kept both ways, including a 1-row primary;
  - margins are untouched by a switch;
  - the DECSC slot is per screen (save on primary, switch, save on alt, switch back, and
    restore gives the primary position);
  - resizing on the primary resizes the parked alternate (width clip and height
    shrink/grow);
  - `full_reset` from either screen keeps both namespaces monotonic;
  - switches are idempotent.
- Every test that pinned the old semantics is updated, and the report lists each with the
  reason.

Benchmarks:

- `bench_alternate_screen_switch` (buffer) and `bench_alt_screen_transition_e2e` (emulator),
  before and after.
- The orchestrator captures the baseline as `before_131_5`.
- Add an `alternate_reenter` case for the persisted re-entry path.

Verification: as above, plus `cargo bench --no-run --all`.

Prohibitions: do NOT implement the per-mode semantics in the handler (131.6); do NOT touch
the placement maps (131.7).

#### 131.6 — Handler: per-mode alternate-screen semantics and a per-screen DECSC charset

Scope:

- `freminal-terminal-emulator/src/terminal_handler/scroll_ops.rs`;
- `freminal-terminal-emulator/src/terminal_handler/mod.rs` (the mode arms and the
  `saved_character_replace` field);
- `freminal-terminal-emulator/src/terminal_handler/cursor_ops.rs` (DECSC / DECRC use the
  per-screen charset slot);
- handler tests;
- `freminal-terminal-emulator/tests/terminal_handler_integration.rs` (tests that pin the old
  semantics).

What:

- Implement the maintainer decisions for `?47`, `?1047` and `?1049` exactly as written.
  Replace `handle_enter_alternate` / `handle_leave_alternate` with
  `fn handle_alternate_screen(&mut self, mode: AltScreenMode, action: AltScreenAction)`,
  where:
  - `AltScreenMode` is `{ Legacy47, Clearing1047, SaveClear1049 }`;
  - `AltScreenAction` is `{ Enter, Leave }`;
  - both are private enums.
- `?1046` gating is unchanged.
- `saved_character_replace` becomes `ScreenScoped<Option<DecSpecialGraphics>>` and follows the
  buffer's DECSC slot.

Deliverable: handler tests for each mode and action, covering:

- cursor kept;
- clear or no clear;
- a second `?1049h` re-saves into the alternate slot and clears;
- `?1049l` on the primary performs DECRC;
- `?47` leave does not restore;
- `?1047` leave clears the alternate (re-entering with `?47` shows blank);
- `?47` re-entry shows the old alternate content.

Prohibitions: do NOT change the placement maps (131.7).

#### 131.7 — Kitty placement maps per screen

Scope:

- `freminal-terminal-emulator/src/terminal_handler/mod.rs` (fields, placement resolution,
  `visible_image_placements_extended`, `inject_virtual_parent_relatives`);
- `freminal-terminal-emulator/src/terminal_handler/graphics_kitty.rs` (every
  `virtual_placements` / `real_placements` access, `d=a`/`d=A`, the pruners, the reflow
  remap);
- `freminal-terminal-emulator/src/terminal_handler/scroll_ops.rs` (clear hook; delete the
  leave filter);
- graphics tests.

What:

- Both maps become `ScreenScoped` and are accessed through the active screen.
- `clear_alternate_screen` is paired with clearing the alternate maps.
- After a resize, the parked alternate map is pruned against the parked store; add a buffer
  getter for the parked alternate's base if needed (added to scope:
  `freminal-buffer/src/buffer/mod.rs`, getter only).
- `placement_prune_base` follows the active screen.
- Delete the old leave filter, and update
  `leaving_the_alternate_screen_drops_alt_placements_only` to the persistence semantics.

Deliverable:

- tests:
  - alternate placements persist across `?47` leave/enter;
  - `?1049` enter clears them;
  - `d=a` on the alternate leaves primary placements alone;
  - a virtual placement created on the alternate is not visible on the primary;
  - reflow on the primary does not corrupt parked alternate placements.

#### 131.8 — The exhaustive reset table

Scope:

- `freminal-terminal-emulator/src/terminal_handler/mod.rs` (`full_reset`, `soft_reset`,
  a new `reset`);
- new `freminal-terminal-emulator/src/terminal_handler/reset.rs` (`ResetKind` and `reset`);
- `freminal-terminal-emulator/src/terminal_handler/graphics_iterm2.rs` (only if a reset
  helper is needed);
- `freminal-terminal-emulator/tests/terminal_handler_integration.rs` (RIS/DECSTR tests).

What:

- Implement `reset(kind)` as exhaustive destructuring, following the "131 Reset table"
  below, which comes from 131.1.
- Fix every RIS divergence the table marks "fix".
- `full_reset` = `reset(Hard)` plus the existing DECCOLM restore sequence.
- `soft_reset` = `reset(Soft)` plus the existing cursor save/restore choreography.

Deliverable:

- the change;
- one RIS test per newly reset field, failing before the change;
- DECSTR tests for any changed field.

#### 131.9 — End-to-end alternate-screen suite

Scope: new `freminal-terminal-emulator/tests/alt_screen_lifecycle.rs`.

What: an e2e suite through `handle_incoming_data` and `build_snapshot`. It covers:

- `?47`, `?1047`, `?1049`: each enter/leave/double-enter/double-leave/query;
- disallowed (`?1046 l`);
- mixed modes (enter 47, leave 1049);
- cross-screen state: keyboard stack, placements, DECSC, margins, URL and selection snapshot
  fields;
- RIS from the alternate screen;
- DECSTR on the alternate screen.

Each assertion is on observable snapshot/reply state.

#### 131.10 — Escape-sequence dual-doc and reference update

Scope:

- `Documents/ESCAPE_SEQUENCE_COVERAGE.md`;
- `Documents/ESCAPE_SEQUENCE_GAPS.md`;
- `Documents/KITTY_PROTOCOL_REFERENCE.md` (the keyboard stack section).

What:

- Record the `?47`/`?1047`/`?1049` rows, DECSC per screen, DECSTR and RIS changes, and the
  per-screen kitty state.
- Refresh "Last updated" in all three documents.

Verification: the pre-commit markdownlint and prettier hooks pass on the three files.

### 131 Status notes

- **131.1 — Complete (2026-10-10).** Audit table folded into "131 Reset table"; surfaced
  131.C3 and 131.C4.
- **131.2 — Complete (2026-10-10), commit `ed74fa74`.** `ScreenScoped<T>` and `Buffer::kind`.
- **131.3 — Complete (2026-10-10), commit `aa707bb1`.**
  - `KittyKeyboardStack` per screen fixes B15.
  - The alternate stack persists between sessions.
  - RIS and DECSTR clear both stacks.
- **131.4 — Complete (2026-10-10), commit `9157cb76`.** `?1047` is `Mode::AltScreen1047`, and
  DECRQM reports 1047. Two test files outside the stated scope
  (`mode_dispatch_tests.rs`, `terminal_handler_integration.rs`) needed mechanical assertion
  updates that the parse split forced; accepted.
- **131.5 — Complete (2026-10-10), commit `b320677d`.**
  - `ParkedScreen` (with `reflow_anchor`) replaces `SavedPrimaryState`.
  - `switch_to_alternate` / `switch_to_primary` are idempotent, keep the cursor's screen
    position and leave the margins alone. `clear_alternate_screen` installs a fresh store
    at the old store's `next_number`.
  - Parked screens are resized eagerly. `full_reset` keeps both namespaces monotonic.
  - Every test that pinned the old semantics was updated (the list is in the commit), and
    new buffer tests were added in `screen_switch_tests.rs`.
  - Accepted nit: a first-ever entry builds a blank store and then the clear builds another.
    That skips `height` row numbers in a 63-bit space and costs one allocation.
  - **Benchmark finding:** `bench_alternate_screen_switch` dropped the buffer inside the
    timed closure, so its old figures (~37 µs) measured deallocation, not the switch. Each
    routine now returns the buffer.
  - Against `before_131_5`, measured in the old shape before that bench fix, no change was
    statistically significant (every p > 0.05; medians moved +1.7% for `enter_alternate`
    and −16% for `leave_alternate`, both within noise). Because the old shape measured
    deallocation and the switch API itself changed, **no valid pre-change baseline exists**
    for the corrected bench shape (adversarial-review finding 8).
  - Corrected measurements: `enter_alternate` ~1.0 µs, `leave_alternate` ~0.16 µs,
    `alternate_reenter` ~0.16 µs. `bench_alt_screen_transition_e2e`: no significant change
    (~16–18 µs).
  - The stale `handle_leave_alternate` comment noted here was removed by 131.7.
- **131.6 — Complete (2026-10-10), commit `91a663f8`.**
  - `handle_alternate_screen(mode, action)` implements the `?47` / `?1047` / `?1049`
    decisions exactly.
  - The DECSC charset slot is `ScreenScoped`.
  - New tests in `tests/alt_screen_modes.rs`.
  - The DECSTR active-screen charset slot has no dedicated test yet; 131.8 adds one.
- **131.7 — Complete (2026-10-10), commit `70b9bfcb`.**
  - The virtual and real placement maps and the prune base are `ScreenScoped`, and every
    operation acts on the active screen.
  - The reflow remap translates the primary map whichever screen is active, and the parked
    map is pruned against `Buffer::parked_row_span`.
  - A clear empties the alternate maps; a switch touches neither.
  - Surfaced 131.C5.
- **131.8 — Complete (2026-10-10), commits `998ee131` (131.8a, handler) and `4ea250fc` (131.8b,
  `TerminalState` and 131.C3).**
  - `TerminalHandler::reset(ResetKind)` destructures the handler with no `..`, so every field
    is classified at compile time.
  - RIS and DECSTR follow the reset table. The new `configured_cursor_visual_style` is the
    baseline RIS and DECSTR restore.
  - RIS in `TerminalState` keeps the parser's in-flight sequence, the UTF-8 tail, the theme
    state and queued commands.
  - The orchestrator deleted the now-unused `set_cursor_visual_style` and pointed the #406
    regression test at the configured-style seam.
- **Cleanups:**
  - 131.C1 and 131.C2: `57b657b6`. DECSC/DECRC save SGR and DECOM; DECRC without a save homes
    with defaults. The orchestrator added that an open OSC 8 hyperlink is not part of the
    save (xterm, kitty and Ghostty), with a test.
  - 131.C3: `4ea250fc`.
  - 131.C4: `1d7f22a3`.
  - 131.C5: `193fd200`.
- **131.9 — Complete (2026-10-10), commit `4071e616`.** 43 end-to-end tests through
  `build_snapshot`.
- **131.10 — Complete (2026-10-10), commit `9d0dae4d`.** COVERAGE, GAPS and the kitty reference.

### 131 Adversarial review (2026-10-10)

The adversarial review of `b6230618..b115cdce` found no blocker or major issue and 19
minor/nit/uncertain findings. Disposition, all addressed before merge:

1. **DECSCUSR `0` used the compiled default, not the configured style.** kitty (`screen.c`
   `screen_set_cursor`: `mode 0` → `NO_CURSOR_SHAPE`), Ghostty (`.default` →
   `default_style`) and WezTerm (`CursorStyle::Default`) use the configured style; xterm
   maps `0` to a blinking block (its configured style is `Ps 7`). Consensus: `CSI 0 SP q`
   restores the configured style, and `1` stays a blinking block. **Fixed.**
2. **An open OSC 8 hyperlink leaked across a screen switch.** kitty
   (`screen_toggle_screen_buffer` sets `active_hyperlink_id = 0`) and Ghostty
   (`switchScreen` ends the hyperlink) end it. **Fixed:** a real switch ends the live
   hyperlink.
3. **Image placement on the alternate screen scrolls the whole screen, ignoring DECSTBM.**
   This is consistent with the primary path, which pushes rows into scrollback regardless
   of margins. **Routed to Task 136**, whose stub owns the cursor-after-placement and
   margin-aware scroll rules. Recorded in the 131.C5 entry.
4. **A parked alternate keeps its image store and placements indefinitely** under `?47`
   use. This is the decided persistence (kitty keeps its alternate graphics manager across
   toggles too). **Documented** in the kitty reference.
5. **131.9 lacked the URL / selection snapshot coverage.** **Added** a URL test. Selection is
   GUI-side: alternate rows are in a different row namespace, so a primary selection becomes
   `Foreign` and is cleared (Task 125).
6. **RIS tests were missing** for the handler's DECANM mirror and the sixel shared palette.
   **Added.**
7. **`debug_assert_parked_screens` was thinner than planned.** **Extended:** parked row
   widths, row namespace, `reflow_anchor` bound, and parked block `live_rows`.
8. **The benchmark comparison is not like-for-like.** The old bench measured deallocation,
   and the switch API changed, so no valid pre-change baseline exists in the new shape.
   **Stated** in the status note, and its "±2% / −16%" contradiction corrected.
9. **Stale docs:** `restore_cursor` and `SavedCursor` (per-screen slots) and the `soft_reset`
   "Saved character set" section. **Fixed.**
10. **Other stale text** (`?1047` test comment, `AltScreen1047` doc, the `kind` field doc,
    plan-note order and counts, the reset-table note on `placement_prune_base`).
    **Fixed.** `resize_saved_primary` keeps its name, as planned.
11. **A `bool` closure parameter in `reset_table.rs`.** **Fixed.**
12. **`ResetKind` and `KittyKeyboardStack` are `pub`, not `pub(crate)`.** **Accepted:** both
    live in private modules, and clippy's `redundant_pub_crate` (denied) rejects
    `pub(crate)` there.
13. **`drop_alternate_marks` is O(marks) on every clear and switch.** **Fixed** with an O(1)
    guard where the mark lists are ordered.
14. **A first-ever entry builds two stores.** Already accepted (131.5 status).
15. **The alternate clear used default attributes.** xterm (`ClearScreen`, BCE) and Ghostty
    (`eraseDisplay` with the cursor background) apply the current background; kitty and
    WezTerm use the default. A 2–2 split, so the maintainer's tie-break (xterm + Ghostty)
    applies. **Fixed:** the clear is BCE. This supersedes the 131 decision bullet "BCE on
    this clear is not changed".
16. **`prev_placeholder` survived a screen switch.** **Fixed:** a real switch clears it.
17. **Bytes after a RIS in the same read are parsed under the pre-RIS DECANM/S8C1T mode.**
    This predates Task 131 and is the same limit as any mode change mid-read. **Recorded**
    as a GAPS paragraph ("Mode changes take effect per read"), and the COVERAGE RIS row
    qualified.
18. **Marks on popped padding rows of a parked primary.** **Accepted:** marks sit on the
    cursor row, and a padding pop never removes the cursor row, so this is unreachable.
19. **Prettier reflows whole tables in COVERAGE.** **Accepted:** the hook realigns tables, so
    this is unavoidable.

### 131 Reset table

From the 131.1 audit (2026-10-10). The references are:

- xterm `charproc.c` `ReallyReset`: the `if (full)` block is RIS and the `else` block is
  DECSTR; the lines above it apply to both.
- kitty `screen.c` `do_screen_reset`, for kitty-protocol state.
- Ghostty `Terminal.zig` `softReset` and WezTerm `Device::SoftReset`, as tie-breakers.

The **maintainer rule** is to follow the consensus. Where they split, xterm wins for non-kitty
state and kitty wins for kitty state. Only rows that change behaviour, or that were in doubt,
are listed. Every other field keeps today's behaviour, and 131.8's exhaustive destructuring
records each one with its reason.

| State                                                                 | RIS                                              | DECSTR             | Source                               | Today                              |
| --------------------------------------------------------------------- | ------------------------------------------------ | ------------------ | ------------------------------------ | ---------------------------------- |
| DECCOLM restore (`pre_deccolm_width`)                                 | restore **only if** DECCOLM changed the width    | –                  | xterm 14534                          | always forces 80 cols + PTY resize |
| `cursor_visual_style`                                                 | configured default                               | configured default | xterm 14379-83, Ghostty              | compiled default / untouched       |
| `insert_mode`, `nrc_mode`                                             | off                                              | off (unchanged)    | xterm 14510                          | not reset on RIS                   |
| `reverse_wrap`, `xt_rev_wrap2`                                        | default                                          | default            | xterm 14510/14549, Ghostty, WezTerm  | never reset                        |
| `s8c1t_mode`, `vt52_mode` (handler mirrors)                           | 7-bit / ANSI                                     | –                  | xterm 14500                          | not reset; diverge from parser     |
| `sixel_display_mode`, `private_color_registers`, sixel shared palette | default                                          | –                  | xterm 14474-87                       | not reset                          |
| `in_band_resize_enabled`                                              | off                                              | –                  | convention                           | not reset                          |
| `kitty_transfer`, `multipart_state`                                   | idle / none                                      | –                  | plan decision                        | not reset                          |
| `modify_other_keys_level`                                             | 0                                                | **0**              | xterm 14420, Ghostty, WezTerm        | DECSTR keeps it                    |
| `palette` (OSC 4)                                                     | reset                                            | **reset**          | xterm 14404, Ghostty, kitty          | DECSTR keeps it                    |
| DECAWM                                                                | on                                               | **on**             | xterm 14549, Ghostty, WezTerm, kitty | DECSTR sets off                    |
| kitty keyboard stacks                                                 | cleared                                          | **cleared**        | kitty 226-227                        | DECSTR keeps them                  |
| `pointer_shape`                                                       | default                                          | **default**        | kitty 216-217                        | DECSTR keeps it                    |
| `window_commands`, `pending_command_events`                           | **kept** (already-emitted events in transit)     | kept               | freminal-internal                    | cleared on RIS                     |
| OSC 7 cwd, `$HISTFILE`                                                | **kept** (maintainer)                            | kept               | describes the process                | cleared on RIS                     |
| `TerminalModes::theme_mode`, `theming`                                | **kept** (config and OS state)                   | kept               | freminal-internal                    | wiped by `TerminalModes::default`  |
| parser state, `leftover_data`                                         | reset only the parser's `vt52_mode`/`s8c1t_mode` | –                  | the chunk is already parsed          | parser replaced, leftover dropped  |
| DECSC slot                                                            | empty slot behaves as "home, defaults" (131.C2)  | home (unchanged)   | xterm 14546-47, Ghostty              | `None` (DECRC no-op)               |

Notes:

- The DECSTR DECAWM row follows xterm, Ghostty, WezTerm and kitty against the literal VT510
  Table 5-9. xterm's own comment records the deviation.
- RIS keeps the colour overrides it resets today (OSC 10/11/12: kitty, Ghostty and WezTerm
  reset them). DECSTR keeps them (xterm).
- The pure transport and cache fields (`write_tx`, `placement_prune_base` — kept on DECSTR,
  re-based on RIS together with the cleared maps —
  `row_epoch_counter`, `cell_pixel_*`, `theme`, `tmux_passthrough_queue`) are kept, as is
  `allow_alt_screen` (xterm does not reset it).
- Task 130's `host_capabilities` (config and platform facts seeded by the GUI and updated by
  `InputEvent::HostCapabilitiesChange`) is kept by both: it describes the host, not the
  terminal. Classified when Task 131 merged onto Task 130.

### 131 Cleanup entries

#### 131.C1 — DECSC / DECRC do not save SGR or DECOM

- **Surfaced:** activation recon (2026-10-10). Predates Task 131.
- **Impact:**
  - `handle_save_cursor` saves the buffer `CursorState`, whose attribute fields have no
    readers, plus the charset.
  - The live SGR state (`current_format` / `Buffer::current_tag`) is not saved or
    restored.
  - xterm `CursorSave2` / `CursorRestoreFlags` (`cursor.c`) save the SGR attributes and
    colours, DECOM, the selective-erase attribute, the charsets and the wrap flag.
  - `?1049` and DECSC/DECRC therefore lose SGR.
- **Scope of fix:**
  - `terminal_handler/cursor_ops.rs`;
  - the buffer `SavedCursor` (`freminal-buffer/src/buffer/cursor.rs`);
  - whatever owns `current_format`.
- **Suggested approach:** reproduce with a test first. Save `current_format` and DECOM in the
  per-screen slot next to the position; restore both.
- **Verification:**
  - `CSI 1;31m ESC 7 CSI 0m ESC 8 X` writes a bold red `X`;
  - DECOM survives DECSC/DECRC.
- **Scheduling:** after 131.6; resolved within Task 131.

#### 131.C2 — DECRC without a prior DECSC is a no-op

- **Surfaced:** activation recon (2026-10-10). Predates Task 131.
- **Impact:**
  - `Buffer::restore_cursor` does nothing when no cursor was saved.
  - xterm (`CursorRestoreFlags` with `sc->saved == False`) and Ghostty (`restoreCursor`)
    home the cursor, reset the attributes and reset the charsets.
  - Under per-screen DECSC slots this is reachable on every first alternate session.
- **Scope of fix:**
  - `freminal-buffer/src/buffer/cursor.rs`;
  - `terminal_handler/cursor_ops.rs`.
- **Suggested approach:** treat an empty slot as a saved default cursor at home.
- **Verification:** a DECRC-with-no-save test homes the cursor and resets SGR.
- **Scheduling:** after 131.C1.

#### 131.C3 — Dead `TerminalState` mirrors of handler state

- **Surfaced:** 131.1 audit (2026-10-10). Predates Task 131.
- **Impact:** three fields are written but never read in production. RIS resets them while
  the authoritative handler copies survive, so they misdocument the reset behaviour.
  - `TerminalState::cursor_visual_style` (the snapshot reads the handler's copy);
  - `TerminalModes::reverse_wrap_around` (the authority is `TerminalHandler::reverse_wrap`);
  - `TerminalModes::cursor_blinking` (the blink state lives in the handler's cursor style).
- **Scope of fix:**
  - `freminal-terminal-emulator/src/state/internal.rs`;
  - `freminal-common/src/buffer_states/mode.rs` (`TerminalModes`);
  - the tests that read the mirrors.
- **Suggested approach:** delete the three mirrors and route any test reads to the handler.
- **Verification:** `rg` finds no remaining reader; the suite passes.
- **Scheduling:** with 131.8.

#### 131.C4 — Reverse-wrap (`?45`) defaults to on, unlike xterm

- **Surfaced:** 131.1 audit (2026-10-10). Predates Task 131.
- **Impact:**
  - `ReverseWrapAround::default()` is `WrapAround`, and its doc claims that matches xterm.
  - It does not: xterm's `reverseWrap` resource defaults to false, Ghostty defaults off, and
    WezTerm resets it to off.
  - RIS and DECSTR reset the mode to its default after 131.8, so the default becomes
    observable.
- **Maintainer decision (2026-10-10):** change the default to off.
- **Scope of fix:**
  - `freminal-common/src/buffer_states/modes/reverse_wrap_around.rs` and its tests;
  - tests that assume reverse wrap is on by default (`rg` for `ReverseWrapAround` and
    reverse-wrap backspace tests);
  - the DECRQM `?45` default expectation.
- **Verification:**
  - `CSI ? 45 $ p` on a fresh terminal reports reset (`2`);
  - BS at column 0 does not wrap until `CSI ? 45 h`.
- **Scheduling:** before 131.8.

#### 131.C5 — Placing an image on the alternate screen's last row grows the store

- **Surfaced:** 131.7 implementation (2026-10-10), reproduced by the orchestrator on the
  integration branch before any Task 131 change, so it predates Task 131.
- **Impact:**
  - `Buffer::place_image` makes room below an image, and for the cursor after it, with
    `push_row`, then trims only on the primary screen (`enforce_scrollback_limit`).
  - On the alternate screen the store grows to `height + 1` rows. A debug build panics on
    the "alternate buffer must have exactly `height` rows" invariant. A release build keeps
    an oversized alternate store, which 131.5's parked-screen invariants assume cannot
    happen.
  - Reproduced with `?1049h`, then CUP to the last row, then `APC G a=T,…,r=1`.
- **Fix:** on the alternate screen, after each growth, evict the excess front rows with the
  alternate screen's own scroll path (`evict_front_rows`), so the content scrolls up exactly
  as a line feed at the bottom would. Then re-derive the image's base row from its stable
  origin row number, as the primary path does.
- **Scope:** `freminal-buffer/src/buffer/images.rs` (`place_image`) and tests.
- **Verification:**
  - Placing a 1-row and a 3-row image on the alternate screen's last row keeps
    `rows.len() == height`.
  - The image's cells sit on the expected scrolled rows, and the cursor is on the row below
    the image (or the last row).
  - The primary behaviour is unchanged.
- **Scheduling:** within Task 131, after 131.7.
- **Status: Resolved (2026-10-10), commit `193fd200`.** Routed to Task 136: placement
  scrolls the whole screen and ignores DECSTBM on both screens (adversarial-review
  finding 3), which belongs with the cursor-after-placement and margin-aware scroll items in
  the Task 136 stub.

---

## Foundation stubs (v0.13.0)

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
- margin-aware scroll containment, including the room an image placement makes below itself:
  today `Buffer::place_image` scrolls the whole screen (primary and alternate) whatever the
  DECSTBM margins (routed from Task 131 adversarial-review finding 3 / 131.C5).

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
  - ~~`p=?` is answered after DA1~~: fixed by Task 130 (answered on the PTY thread in stream
    order).
  - ~~Capabilities are advertised while notifications are disabled~~: fixed by Task 130 (every
    OSC 99 request is dropped while unsupported; `a=report` / `c=1` follow platform and
    routing).
  - Ids, buttons and live-id lists are sanitised at the reply serialiser since Task 130; the
    transported `report_activation` / `focus_on_activation` / `close_report` bools are
    130.C4, routed here.
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
