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
| v0.13.0   | Foundations, Safety & Small Conformance    | 126–132, 138–140, 147, 148   |
| v0.13.1   | Shipped-Protocol Conformance (heavy)       | 134, 135, 136, 137           |
| v0.13.2   | New Protocols: Small & Medium              | 103, 141, 142, 143, 144, 145 |
| v0.13.3   | New Protocols: Text Sizing, Transfer, Clip | 133, 104, 102, 146           |
| v0.13.4   | Drag & Drop                                | 105                          |

**Tiered decomposition.** Per `plan-decomposition`, only the next-up tasks carry a full
subtask breakdown: **126**, **127**, **128**, **129** (activated 2026-10-09), and **130** and **131**
(activated 2026-10-10), **147** (created 2026-10-11), **132** (activated 2026-10-11), and
**138** (activated 2026-10-12), and **148** (created 2026-10-18).
Everything else is an **enriched stub**:
goal, audit findings, durable decisions and open questions, with no subtasks. Tasks 102, 103
and 104 were previously decomposed. The audit found factual errors in those breakdowns (for
example, OSC 66 is not a Contour code, and the 102 wire keys were wrong), so they are now
enriched stubs again and get re-decomposed at activation.

---

## Task Summary

| #   | Task                                        | Milestone | Scope | Status   | Depends on                         |
| --- | ------------------------------------------- | --------- | ----- | -------- | ---------------------------------- |
| 126 | Pre-existing Safety Gate                    | v0.13.0   | S     | Complete | None                               |
| 127 | Unfocused / Inactive-Pane Cursor (#531)     | v0.13.0   | M     | Complete | 126 (schedule only)                |
| 128 | Prefix- & Intermediate-Aware CSI Dispatch   | v0.13.0   | M     | Complete | 126.3                              |
| 129 | Kitty Wire Infrastructure                   | v0.13.0   | L     | Complete | None                               |
| 130 | Reverse-Path & Capability-Query Consistency | v0.13.0   | L     | Complete | 129                                |
| 131 | Screen-Scoped State & Reset Lifecycle       | v0.13.0   | L     | Complete | 130.1–130.3 (foundation)           |
| 132 | Colour Foundation                           | v0.13.0   | L     | Complete | 126.1                              |
| 138 | Desktop Notifications (OSC 99) Conformance  | v0.13.0   | L     | Complete | 129, 130                           |
| 139 | Pointer Shapes (OSC 22) Completion          | v0.13.0   | M     | Planned  | 129, 130, 131                      |
| 140 | Underline & SGR Parity                      | v0.13.0   | S     | Planned  | None                               |
| 147 | v0.13.0 Correctness Cleanup                 | v0.13.0   | L     | Planned  | 130, 131                           |
| 148 | Security & Fuzz Findings Remediation        | v0.13.0   | XL    | Planned  | None (148.20 waits for 147.4)      |
| 134 | Unicode Width & Segmentation Conformance    | v0.13.1   | XL    | Planned  | None                               |
| 135 | Graphics Protocol Conformance               | v0.13.1   | L     | Planned  | 126.2, 129, 130, 131               |
| 136 | Graphics Placement Model & Z-Layers         | v0.13.1   | XL    | Planned  | 135                                |
| 137 | Keyboard Protocol Conformance               | v0.13.1   | XL    | Planned  | 128, 131; architecture sign-off    |
| 103 | Multiple Cursors                            | v0.13.2   | L     | Planned  | 127, 128, 131, 132, 141            |
| 141 | Misc Protocol Extensions                    | v0.13.2   | M     | Planned  | 128, 131                           |
| 142 | Unscroll (`CSI Ps + T`)                     | v0.13.2   | M     | Planned  | 128                                |
| 143 | DECCARA / DECSACE                           | v0.13.2   | M     | Planned  | 128, 140                           |
| 144 | Color Control (OSC 21) & Colour Stack       | v0.13.2   | L     | Planned  | 128, 129, 130, 132                 |
| 145 | Kitty Shell-Integration Compatibility       | v0.13.2   | M     | Planned  | 129; maintainer decision           |
| 133 | Shared Consent Prompt                       | v0.13.3   | M     | Planned  | None                               |
| 104 | Kitty Text Sizing (OSC 66)                  | v0.13.3   | XL    | Planned  | 129, 134, 103                      |
| 102 | Kitty File Transfer (OSC 5113)              | v0.13.3   | XL    | Planned  | 129, 130, 133                      |
| 146 | Kitty Clipboard (OSC 5522)                  | v0.13.3   | XL    | Planned  | 129, 130, 133                      |
| 105 | Kitty Drag & Drop (OSC 72)                  | v0.13.4   | XL    | Planned  | 102, 133; windowing DnD capability |

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
| Desktop notifications | Done, incl. reports and `p=?` handshake             | ~~Buttons reported 0-based; chunk metadata clobbered by defaults; `a=report` disables focus; close report lost after activation; no update-in-place; `p=close` doesn't close~~ (all closed by Task 138); ~~`p=?` answered after DA1~~ (fixed by 130) | 138           |
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
| Wide-gamut colours (`oklch()`, `lab()`)                                        | Resolved: implemented by Task 132 (132.6) in every colour spec, CSS Color 4–correct                               |
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
- **Then 138, 139, 140, 147 and 148**, which close out v0.13.0. Task 147 fixes everything Tasks
  126–131 left unscheduled; see "147 Execution model". Task 148 fixes the #553 fuzzing and
  security-audit findings; see "148 Execution model". Its high-severity subtasks should land
  first, ahead of the rest of Task 147 if the two are not run in parallel.
- v0.13.1 onward is activated per milestone, against the code as it then exists. Activation
  re-reads the stub, resolves its open questions with the maintainer, and decomposes it.

Every implementation subtask leaves `cargo xtask test` (nextest plus doctests; the repo
moved to cargo-nextest in PR #541) and
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
- **Status:** resolved by 138.1 (the typed transport enums).

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
  reset them). DECSTR keeps them (xterm). Task 132 added the OSC 17/19 selection overrides to
  the same `DynamicColorOverrides` set, so they follow the same rule: reset by RIS, kept by
  DECSTR. The theme lives in `ColorState` and survives both.
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

## Task 132 — Colour Foundation

### 132 Summary

**Goal.** A spec-complete colour-spec parser, a reusable SGR-style colour extractor, and
dynamic colours that actually render. Cells keep the palette index they were written with, and
the renderer resolves it against the pane's live palette, so OSC 4, OSC 104 and theme changes
recolour existing text the way xterm and kitty do.

Activated and decomposed on 2026-10-11 on `task-132/colour-foundation`, branched from `main`
at `6edf882c` (after PR #537).

### 132 Activation recon (2026-10-11)

Four read-only recon passes (colour-spec parser and OSC handlers; SGR parsing and cell storage;
snapshot, render and damage; background layering) plus two spec-research passes (xterm,
kitty, Ghostty and X11 source; CSS Color 4 and the X.Org `rgb.txt`). Everything the stub
claimed was confirmed. The stub's line references were stale (the OSC 10/11 override fields are
at `terminal_handler/mod.rs:306-320`, read only by `terminal_handler/osc_colors.rs:40-48`).

Findings beyond the stub:

- **`handle_sgr` bakes every index, not just 16–255.** `ColorPalette::lookup` returns
  `TerminalColor::Custom` for 0–255 (`terminal_handler/sgr.rs:34-56`, `colors.rs:65-71`), so a
  theme change does not recolour `38;5;1` either. Named colours (SGR 30–37 / 90–97) stay
  symbolic and resolve from `theme.ansi` at render time, which means they ignore OSC 4.
- **The compact / LZ4 row format already round-trips `PaletteIndex`** (tag 21 plus one byte,
  `freminal-buffer/src/compact_row.rs:485-488, 520`). Storing the index needs no format change.
- **Placeholder ids confirmed broken.** `38;5;42` is stored as `Custom(0,215,135)` and decodes
  to image id 55175, not 42. The `PaletteIndex` arms of `color_to_image_id` /
  `color_to_placement_id` (`unicode_placeholder.rs:167-190`) are dead in production.
- **DECRQSS can never report `38;5;n`** for the same reason (`terminal_handler/sgr.rs:129-145`
  are dead arms).
- **Index 16 disagreement.** `lookup_default_256_color(16)` returns `theme.ansi[0]`;
  `default_index_to_rgb(16)` returns `(0,0,0)`. xterm (`256colres.h`) and kitty
  (`init_FG_BG_table`) both define index 16 as `(0,0,0)`. Only index 16 differs.
- **Reply widths are inconsistent.** OSC 4 replies are 16-bit (`rgb:rrrr/gggg/bbbb`); OSC
  10/11/12 replies are 8-bit. xterm, kitty and Ghostty all reply 16-bit.
- **OSC chaining is unsupported.** OSC 4 handles one pair, OSC 104 one index, OSC 10 one spec.
  OSC 17/19 are recognised no-ops; OSC 117/119 are unmapped.
- **`ThemePalette::cursor_text` has no reader**, and the visual bell colour is hard-coded.
- **The default background is never drawn.** `DefaultBackground` cells are skipped
  (`renderer/vertex.rs:561-566`); the visible colour is the window clear plus the egui panel
  fill, both from the config theme. There is no per-pane full-rect quad today.
- **No damage signal exists for a palette or dynamic-colour change.** Theme changes are caught
  by a GUI-side pointer comparison (`frame_dirty.rs:594-596`) that forces a pane-bounded full
  rebuild. A palette change bumps no row epoch, so the bounded path would reuse stale vertices.
- **DECSCNM has no damage signal at all** (verified by the orchestrator: `Mode::Decscnm` only
  stores the mode at `state/internal.rs:315`; `frame_dirty.rs` has no DECSCNM term). See
  132.C1.
- **SGR extractor edge cases.** Mode 5 wraps (`38;5;300` is index 44), a missing mode-5 index
  becomes index 0, mode-2 components over 255 emit `Invalid`, and a missing component becomes 0.
- **No benchmark exercises indexed colour.** Every SGR payload in the bench suite is `38;2`.

### 132 Decisions (maintainer, 2026-10-11)

1. **`#RGB` is nibble-doubled** (`#3a7` is `0x33aa77`), matching kitty's code and tests and
   Ghostty. kitty's spec prose and XParseColor say `0x30a070`; kitty's implementation does not
   follow its own text. `#RRRGGGBBB` / `#RRRRGGGGBBBB` keep the top byte of each channel, which
   all three agree on.
2. **Cells store the palette index; lookup is at render time.** The snapshot carries the pane's
   resolved palette; the renderer resolves `PaletteIndex` and the 16 named colours through it.
3. **X11 colour names are accepted**, from the full current X.Org `rgb.txt` (782 entries,
   MIT/X11-style licence, recorded in `ATTRIBUTIONS.md`). Lookup is ASCII-lowercase with spaces
   significant, matching kitty and the X server.
4. **`oklch()` and `lab()` are implemented in 132**, following **CSS Color 4**, not kitty's
   implementation. kitty's code deviates from CSS (and from its own docs) in four ways; each is
   recorded under "132 Decisions (orchestrator)".
5. **`cursor_text` and `visual_bell` are deferred to Task 144.** Their only setter is OSC 21.
   132 covers the xterm dynamic-colour set: OSC 10/11/12/17/19.
6. **OSC 11 renders through a pane base fill.** A full-rect quad drawn first in the pane pass
   (before the background image and cell backgrounds), at `background_opacity`, emitted only
   when the pane's effective background differs from the theme background. The gutter strip,
   pane borders and window clear keep the theme background.

### 132 Decisions (orchestrator, recorded so they are not re-litigated)

- **freminal is MIT, kitty is GPLv3.** No kitty code is copied. kitty is read for observable
  behaviour only. The wide-gamut maths comes from CSS Color 4 (§9, §13/14, §19 sample code) and
  Björn Ottosson's OKLab post (public domain / MIT).
- **Where kitty's `oklch()`/`lab()` differs from CSS, freminal follows CSS:**
  - kitty's achromatic shortcut feeds OKLab `L` straight into the sRGB OETF; CSS converts
    properly (`oklch(0.5 0 180)` is `#636363`, not `#BCBCBC`).
  - kitty treats `lab()` as D65; CSS specifies D50 with Bradford adaptation to D65.
  - kitty's gamut mapping is dead code (it clips before testing); freminal implements the CSS
    chroma-reduction search (JND 0.02, epsilon 0.0001, deltaEOK).
  - kitty scales `C%` by 1/100 and `a%`/`b%` by 1; CSS uses 100% = 0.4 and 100% = 125.
- **`rgb:` keeps today's scaling**, which already matches kitty: one digit is doubled, two are
  taken as-is, three and four keep the top byte.
- **`rgbi:`** values are clamped to `[0, 1]` and rounded half away from zero (`round(f * 255)`);
  NaN is rejected. This matches kitty.
- **Prefixes are case-insensitive and the spec is ASCII-trimmed** (kitty, X11). kitty's
  trailing `# comment` stripping is a config-file artefact and is **not** reproduced.
- **`@alpha` is not parsed in 132.** kitty honours it only for OSC 21
  `transparent_background_colorN`; everywhere else a spec with `@` is rejected. It moves to
  Task 144 with that key.
- **SGR colour edge cases follow kitty** (the spec is silent; kitty is the tie-breaker):
  - indices and components are masked with `& 0xFF` (`38;5;256` is index 0; `38;2;300;0;0` is
    red 44);
  - an incomplete colour (`38;5`, `38;2;1;2`, bare `38`) changes nothing;
  - an unknown colour type (`38;4;…`) drops the rest of that SGR sequence (parameters before it
    have already applied);
  - colon form decides the colourspace id by count: after `2`, four or more remaining values
    mean the first is a colourspace id and is ignored. An empty colon field counts as 0.
- **OSC 4 follows kitty:** an odd number of fields after `4` drops the whole sequence; a pair
  with a bad index or an unparsable spec is skipped and the rest still apply; `?` queries a
  pair.
- **OSC 104 follows kitty:** each index is reset; a bad index is skipped; no parameters reset
  the whole table.
- **OSC 10–19 chain** (xterm, kitty): `OSC 10;a;b` sets 10 then 11. Slots 13–16 and 18
  consume a value and are ignored. Each value is a spec or `?`.
- **OSC 110, 111, 112, 117, 119 reset one slot each** and ignore any parameters.
- **Replies are 16-bit** (`rgb:rrrr/gggg/bbbb`, each byte times 257) for OSC 4 and OSC
  10/11/12/17/19, terminated by the handler's framing (ST, S8C1T-aware), which matches kitty.
  A selection-colour query reports the override if set, else the theme colour.
- **Reset classification follows the 131 table:** the OSC 4 palette resets on RIS and DECSTR;
  the dynamic overrides (now including selection fg/bg) reset on RIS only (xterm). A theme
  change keeps both, as today.
- **Damage.** A change to the pane's resolved colours is classified **BOUNDABLE-NOW at pane
  level**: it is the same shape as `theme_changed` (every vertex of the pane is stale) and goes
  through `full_pane_rebuild_damage_rect`, so other panes are not repainted. It does not touch
  chrome. The OSC 12 cursor colour stays on the existing cursor-only path
  (`previous_cursor_color_override`) and is deliberately **not** folded into the resolved
  palette.
- **The resolved palette is cached, not rebuilt per snapshot.** The handler's colour state is
  one struct with private fields; every mutator invalidates the cached `Arc`, so
  `build_snapshot` reuses the same `Arc` until something changes, and the GUI can compare by
  `Arc::ptr_eq`.

### 132 Types (decided)

As built (updated after the adversarial review, R12). In `freminal-common/src/colors/`
(132.2 turned `colors.rs` into this directory):

```rust
// colors/dynamic.rs
pub enum DynamicColorSlot { Foreground, Background, Cursor, SelectionBackground, SelectionForeground }
// ALL, osc_code(), from_osc_code()
pub struct DynamicColorOverrides { /* [Option<(u8, u8, u8)>; 5], private */ }
// get(slot), set(slot, rgb), reset(slot), reset_all()

// colors/resolved.rs
pub struct ResolvedColors {
    pub palette: [(u8, u8, u8); 256],
    pub foreground: (u8, u8, u8),
    pub background: (u8, u8, u8),
    pub selection_background: (u8, u8, u8),
    pub selection_foreground: (u8, u8, u8),
}
// ResolvedColors::resolve(theme, &ColorPalette, &DynamicColorOverrides) -> Self
// ResolvedColors::rgb(&self, TerminalColor) -> (u8, u8, u8)

// colors/spec.rs (+ x11_names.rs: a `static` 782-entry table; wide_gamut.rs)
pub fn parse_color_spec(spec: &str) -> Result<(u8, u8, u8), ColorSpecError>
pub enum ColorSpecError { /* one variant per failure class */ }
```

In `freminal-common/src/buffer_states/osc_color.rs` (132.13):

```rust
pub enum ColorRequest { Set((u8, u8, u8)), Query }
pub struct PaletteColorOp { pub index: u8, pub request: ColorRequest }
pub enum PaletteReset { All, Indices(Vec<u8>) }
pub struct DynamicColorOp { pub slot: DynamicColorSlot, pub request: ColorRequest }
```

In `freminal-terminal-emulator/src/terminal_handler/color_state.rs` (132.9):

```rust
pub(super) struct ColorState {
    theme: &'static ThemePalette,
    palette: ColorPalette,
    dynamic: DynamicColorOverrides,
    resolved: Option<Arc<ResolvedColors>>, // None = stale
}
// palette_rgb, set_palette, reset_palette(_all), dynamic, effective_dynamic,
// set_dynamic, reset_dynamic(_all), resolved; invalidates only on an effective change
```

In `freminal-common/src/buffer_states/modes/decscnm.rs` (132.C1): `ScreenInversion { Normal,
Reversed }`. In the GUI renderer (132.11): `PaneBackground { opacity, base_fill }`.

### 132 Execution model

Serial, on one checkout (`task-132/colour-foundation`). The subtasks share `colors/`, the
handler and the renderer, so worktrees would buy nothing. Each subtask is implemented by a
sub-agent, then reviewed in full by the orchestrator against the subtask text (diff read line
by line, verification suite re-run by the orchestrator), and only then committed by the
orchestrator. A sub-agent that ends without a report is treated as unverified: its tree state is
inspected and the work is finished by a fresh agent, re-decomposed, or discarded.

Order: 132.1, 132.2, 132.3, 132.4, 132.5, 132.6, 132.7, 132.8, 132.9, 132.10, 132.C1, 132.11,
132.12, 132.13, 132.14, 132.C4. The adversarial review of the whole branch follows 132.14; its findings
become further `132.Cn` entries.

### 132 Subtasks

Every subtask's verification is `cargo test --all` and
`cargo clippy --all-targets --all-features -- -D warnings`, plus anything named below. Every
subtask's prohibitions include: do NOT touch files outside scope, do NOT commit, do NOT update
plan documents, and do NOT proceed to the next subtask. No `unwrap`/`expect` outside tests, no
raw numeric `as` casts (`conv2`), no new `bool` fields or parameters. The stop condition is the
same for all: report files changed and verification results, then await review.

#### 132.1 — Indexed-colour benchmarks and baseline

Scope:

- `freminal-terminal-emulator/benches/buffer_benches.rs`;
- `freminal/benches/render_loop_bench.rs`.

What:

- Add an indexed-colour payload (`ESC[38;5;Nm` and `ESC[48;5;Mm` with varying N and M, plus
  eight characters, repeated; same total size as `sgr_heavy_payload`).
- Add `bench_parse_sgr_indexed` (parser only, mirroring `bench_parse_sgr_heavy`) and
  `bench_handle_sgr_indexed` (handler level, mirroring `bench_handle_incoming_data`).
- Add `instanced_bg_indexed` and `instanced_fg_indexed` variants of `bench_bg_instances` /
  `bench_fg_instances` whose snapshot is produced by feeding the indexed payload.
- Run, on this commit, the baseline for: the four new benches, `bench_parse_sgr_heavy`,
  `bench_build_snapshot` (all three sub-benches), `instanced_bg`, `instanced_fg`. Report the
  numbers in the deliverable.

Deliverable: the benches, compiling under `cargo bench --no-run --all`, and the baseline table.

#### 132.2 — Turn `colors.rs` into a module directory (move only)

Scope: `freminal-common/src/colors.rs` becomes `freminal-common/src/colors/mod.rs`,
`colors/palette.rs` (the 256-colour tables, `cube_component`, `ColorPalette`) and
`colors/spec.rs` (`parse_color_spec`, `scale_hex_channel`). Tests move with their code.

What: a pure move. Every public path (`freminal_common::colors::…`) keeps working through
re-exports in `colors/mod.rs`. No behaviour change, no signature change, no call-site change
outside `freminal-common`.

Deliverable: the split, `git diff -M` showing moves; all tests pass unchanged.

#### 132.3 — One 256-colour default table; index 16 is `(0,0,0)`

Scope: `freminal-common/src/colors/palette.rs`, `freminal-common/tests/color_tests.rs`, and
the call sites of the removed function in `freminal-common` and `freminal` (`gui/colors.rs`).

What:

- One function computes the default 256-colour entry:
  `default_palette_rgb(index: u8, theme) -> (u8, u8, u8)`. 0–15 come from `theme.ansi`; 16–231
  are the cube; 232–255 the greyscale ramp. Index 16 is `(0,0,0)`.
- `lookup_default_256_color`, `default_index_to_rgb` and `resolve_palette_default` are replaced
  by it (or become thin wrappers over it, if a caller needs `TerminalColor`). The `256..` arm
  disappears with the `usize` signature.
- Tests: every index 0–255 agrees between the old query path and the SGR path (one table);
  index 16 is `(0,0,0)` under a theme whose `ansi[0]` is not black; cube and greyscale spot
  values (17 → `00/00/5f`, 231 → `ff/ff/ff`, 232 → `08/08/08`, 255 → `ee/ee/ee`).

Deliverable: the deduplicated table and tests. Note in the report that `38;5;16` now renders
`(0,0,0)` instead of the theme's black.

#### 132.4 — Colour-spec parser parity (hex, `rgb:`, `rgbi:`)

Scope: `freminal-common/src/colors/spec.rs` and the four call sites
(`ansi_components/osc_palette.rs:74`, `terminal_handler/osc_colors.rs:55, 67, 79`).

What:

- `parse_color_spec(&str) -> Result<(u8, u8, u8), ColorSpecError>`. `ColorSpecError` names
  each failure class (empty, unknown format, bad hex, wrong channel count, bad intensity, …).
  Call sites map the error to today's behaviour (skip and `debug!`/`warn!` as they do now).
- Trim ASCII whitespace; match prefixes case-insensitively.
- `#` with 3, 6, 9 or 12 hex digits: 3 is nibble-doubled; 6, 9 and 12 keep the top byte of each
  channel. Any other length is an error.
- `rgb:` unchanged (1–4 hex digits per channel; 1 doubled, 2 as-is, 3–4 top byte).
- `rgbi:` three `/`-separated floats, parsed with `str::parse::<f64>`; NaN is an error; values
  clamp to `[0, 1]`; channel is `round(f * 255)` (half away from zero). Use `conv2` for the
  float-to-`u8` conversion.
- Non-ASCII input must still be rejected without panicking (126.1 regression tests stay).
- Tests: every form, uppercase prefixes and digits, whitespace, `#3a7` is `33aa77`,
  `#abcabcdef` is `ab/ab/de`, `#123456789abc` is `12/56/9a`, `rgbi:2/-1/0` is `ff/00/00`,
  `rgbi:0.5/0.5/0.5` is `80/80/80`, `rgbi:nan/0/0` is an error, plus the existing proptests.

Deliverable: the parser and tests.

#### 132.5 — X11 colour names

Scope: new `freminal-common/src/colors/x11_names.rs`; `colors/spec.rs` (lookup hook);
`ATTRIBUTIONS.md`.

What:

- Fetch `https://gitlab.freedesktop.org/xorg/app/rgb/-/raw/master/rgb.txt` and its `COPYING`.
  Transcribe all 782 entries into a `const` table of `(&str, (u8, u8, u8))`, keys
  ASCII-lowercased, sorted for binary search. A header comment names the source URL, the
  upstream commit, and the licence.
- `parse_color_spec` tries the name table first (after trimming and lowercasing) when the spec
  does not start with `#` and is not a recognised prefix form. Spaces are significant
  (`dark slate gray` and `darkslategray` both exist; `dark  slate gray` does not match).
- Add an `ATTRIBUTIONS.md` entry carrying the licence notice.
- Tests: table is strictly sorted and has 782 unique keys; `green` is `(0,255,0)`; `gray` is
  `(190,190,190)`; `web gray` is `(128,128,128)`; `rebeccapurple` is `(102,51,153)`;
  `DarkSlateGray` and `dark slate gray` agree; an unknown name is an error.

Deliverable: the table, lookup, attribution and tests.

#### 132.6 — `oklch()` and `lab()` (CSS Color 4)

Scope: new `freminal-common/src/colors/wide_gamut.rs`; `colors/spec.rs` (dispatch).

What (clean-room: do not read or copy kitty's `colors.c`; use CSS Color 4 and Ottosson):

- Syntax, case-insensitive, after trimming: `oklch(` / `lab(` … `)`, with exactly three
  components separated by spaces and/or commas, optionally followed by `/ alpha` (parsed and
  ignored; the result is opaque). `none` means 0.
  - `oklch(L C H)`: L is a number (0–1) or a percentage (100% = 1); C is a number or a
    percentage (100% = 0.4), floored at 0; H is a number (degrees) or an angle with `deg`,
    `grad`, `rad` or `turn`.
  - `lab(L a b)`: L is a number (0–100) or a percentage (100% = 100); a and b are numbers or
    percentages (100% = 125).
  - L clamps to its range. Non-finite values are an error. Missing components are an error.
- Conversion (CSS Color 4 §19 / Ottosson matrices): OKLCh → OKLab → linear sRGB; Lab (D50) →
  XYZ (D50) → Bradford → XYZ (D65) → linear sRGB. Then the sRGB transfer function.
- Gamut mapping (CSS Color 4, "CSS gamut mapping to an RGB destination"): if in gamut, done;
  `L ≥ 1` is white and `L ≤ 0` is black; otherwise binary-search chroma in OKLCh with
  deltaEOK, JND 0.02, epsilon 0.0001. `lab()` out of gamut is converted to OKLCh and mapped the
  same way.
- Final channel is clamped to `[0, 1]` and rounded half away from zero, through `conv2`.
- Test vectors, exact (8-bit):
  - CITED (kitty tests, in gamut, CSS agrees): `oklch(0.5 0.1 180)` → `0,117,101`;
    `oklch(0.7 0.15 140)` → `104,180,87`; `oklch(0.9 0.05 265)` → `206,222,255`;
    `oklch(1 0 0)` → white; `oklch(0 0 0)` → black; `lab(100 0 0)` → white; `lab(0 0 0)` →
    black.
  - COMPUTED during activation from a CSS-correct scratch model (`/tmp/opencode/k/model.py`,
    reference only): `oklch(0.7 0.35 25)` → `255,86,85`; `oklch(0.5 0.5 0)` → `187,0,94`;
    `oklch(0.5 0 0)` → `99,99,99`; `lab(50 0 0)` → `119,119,119`;
    `lab(29.2345 39.3825 20.0664)` → `125,35,41` (CSS lists it as equal to
    `oklch(40.101% 0.12332 21.555)`); `lab(50 60 40)` → `214,60,55`;
    `lab(80 -120 120)` → `0,228,62`.
  - Syntax: commas, percentages, `deg`/`turn`, `none`, `/ 0.5` ignored, uppercase, and
    rejections (`oklch()`, `oklch(0.5 0.1)`, `oklch(a b c)`, `lab(50)`, trailing junk).
- If an implementation disagrees with a COMPUTED vector, **stop and report** with the
  intermediate values; do not adjust the vector or add a tolerance.

Deliverable: the module, dispatch and tests.

#### 132.7 — Reusable SGR colour extractor

Scope: new `freminal-terminal-emulator/src/ansi_components/csi_commands/sgr_color.rs`;
`ansi_components/csi_commands/sgr.rs` (use it, delete `handle_custom_color` and
`default_color`); `ansi_components/csi_commands/mod.rs`; the SGR test files
(`csi_commands/sgr.rs` tests, `tests/sgr_underline_styles.rs`, `tests/sgr_extended.rs`,
`tests/sgr_matrix.rs`) where an expectation changes under the decided semantics.

What:

- `pub enum SgrColorForm { Semicolon, Colon }`.
- `pub fn parse_sgr_color(params: &[Option<usize>], form: SgrColorForm) -> Result<SgrColor, SgrColorError>`
  where `params` starts at the colour-type value (the element after 38/48/58),
  `SgrColor { color: TerminalColor, consumed: usize }` (`color` is `PaletteIndex` or `Custom`;
  `consumed` counts the elements used, including the type), and `SgrColorError` distinguishes
  `Incomplete` from `UnknownType(usize)`.
- Semantics as decided: `& 0xFF` masking; semicolon `2` takes exactly three values (no
  colourspace); colon `2` skips the first of four or more remaining values; missing values are
  `Incomplete`; `None` (empty or unparsable) counts as 0.
- `sgr.rs`: `Incomplete` produces nothing; `UnknownType` drops the rest of the sequence (with a
  `warn!`). Bare `38`/`48`/`58` with nothing after is `Incomplete`. The stored colour is still
  converted by the handler exactly as today (132.12 changes storage).
- Tests: the extractor's table of inputs (each decision bullet), and parser-level tests for
  `38;5;256`, `38;5`, `38;2;300;0;0`, `38:2:1:2:3`, `38:2::1:2:3`, `38:2:9:1:2:3`,
  `38;4;1;1m` (bold not applied), `1;38;5m` (bold applied, colour unchanged).
- Benchmark: `bench_parse_sgr_heavy` and `bench_parse_sgr_indexed` before/after (15%).

Deliverable: the extractor, the rewired SGR parser, tests and bench numbers. List every changed
test expectation with the decision bullet that changed it.

#### 132.8 — `DynamicColorOverrides` and `ResolvedColors`

Scope: new `freminal-common/src/colors/dynamic.rs` and `colors/resolved.rs`; `colors/mod.rs`.

What: the types in "132 Types (decided)".

- `DynamicColorOverrides`: private per-slot `Option<(u8, u8, u8)>`; `get`, `set`, `reset`,
  `reset_all`; `Default` is all-unset.
- `ResolvedColors::resolve(theme, &ColorPalette, &DynamicColorOverrides)`: the palette is the
  default table (132.3) overlaid with OSC 4 overrides; `foreground` / `background` /
  `selection_*` are the override if set, else `theme.foreground` / `theme.background` /
  `theme.selection_bg` / `theme.selection_fg`. The cursor slot is not resolved here (it stays on
  the cursor path).
- `ResolvedColors::rgb(TerminalColor)`: `Default`, `DefaultUnderlineColor` and
  `DefaultCursorColor` → `foreground`; `DefaultBackground` → `background`; the 16 named
  variants → `palette[0..16]`; `PaletteIndex(n)` → `palette[n]`; `Custom` passes through.
- Derive `Debug, Clone, PartialEq, Eq`.
- Tests: overrides win; reset restores theme; named colours follow an OSC 4 override of 0–15;
  every `TerminalColor` variant resolves.

Deliverable: the types and tests. No consumers yet; gate unused items with
`#[cfg_attr(not(test), expect(dead_code))]` plus `TODO(132.9)` only if clippy requires it.

#### 132.9 — Handler `ColorState` and snapshot transport

Scope:

- new `freminal-terminal-emulator/src/terminal_handler/color_state.rs`;
- `terminal_handler/mod.rs` (fields `palette`, `theme`, `fg_color_override`,
  `bg_color_override`, `cursor_color_override` become one `color_state: ColorState`; accessors);
- `terminal_handler/osc.rs`, `terminal_handler/osc_colors.rs`, `terminal_handler/sgr.rs` (read
  through `ColorState`);
- `terminal_handler/reset.rs` (classify `color_state`: palette on both kinds, dynamic on Hard);
- `freminal-terminal-emulator/src/snapshot.rs` and `interface.rs` (`build_snapshot`);
- tests in those files.

What:

- `ColorState` as in "132 Types (decided)". Methods: `theme()`, `set_theme`,
  `palette_rgb(idx)`, `set_palette`, `reset_palette(idx)`, `reset_palette_all`,
  `dynamic(slot)`, `set_dynamic`, `reset_dynamic(slot)`, `reset_dynamic_all`,
  `resolved(&mut self) -> Arc<ResolvedColors>` (builds and caches when stale). Every mutator
  clears the cache, including `set_theme`.
- `TerminalSnapshot` gains `resolved_colors: Arc<ResolvedColors>`. `theme` and
  `cursor_color_override` stay (chrome and the cursor-only path read them).
  `TerminalSnapshot::empty()` builds one from `CATPPUCCIN_MOCHA`.
- Behaviour unchanged: OSC 10/11/12 set/query/reset and OSC 4/104 behave exactly as before
  (reply formats untouched until 132.13). `handle_sgr` still resolves indices through the
  palette (until 132.12).
- Tests: two consecutive snapshots with no colour change share the `Arc` (`Arc::ptr_eq`); an
  OSC 4 set, an OSC 10 set, an OSC 104, a theme change, RIS and DECSTR each produce a new `Arc`
  with the expected contents; DECSTR keeps the dynamic overrides and resets the palette.
- Benchmark: `bench_build_snapshot` (all three) before/after (15%).

Deliverable: the state struct, transport, tests and bench numbers.

#### 132.10 — Renderer resolves through `ResolvedColors`; resolved-colour damage

Scope:

- `freminal/src/gui/colors.rs` (`internal_color_to_gl` takes `&ResolvedColors` instead of the
  theme; selection-colour helpers);
- `freminal/src/gui/renderer/vertex.rs` (`BackgroundFrame` / foreground builder inputs;
  selection colours);
- `freminal/src/gui/terminal/widget.rs` (pass `snap.resolved_colors`; `PaneRenderCache` gains
  `previous_resolved_colors`; `invalidate_theme_cache` clears it);
- `freminal/src/gui/terminal/frame_dirty.rs` (`resolved_colors_changed`);
- `freminal/benches/render_loop_bench.rs` (adapt to the new signature);
- tests in those files.

What:

- Every terminal-content colour resolution reads `ResolvedColors`: cell fg/bg/underline/
  strikethrough, reverse video and DECSCNM composition, selection bg/fg. Chrome, toasts and the
  window clear keep reading the theme. The implementer lists every remaining `theme.` read in
  the terminal-content path (widget.rs, vertex.rs) and justifies each.
- `resolved_colors_changed = !Arc::ptr_eq(...)` against `previous_resolved_colors`; it joins
  `theme_changed` in `content_changed` and in the `bounded_change` veto, and is recorded where
  `previous_theme` is recorded.
- Damage classification per `freminal-damage-model`: BOUNDABLE-NOW at pane level (see
  decisions). Add a test that an OSC 4 change in one pane of two produces a damage region
  bounded to that pane.
- With no overrides, output is pixel-identical to before: the existing pixel goldens pass
  unchanged.
- Tests: an OSC 4 override of index 1 recolours an `SGR 31` cell's vertex colour; an OSC 10
  override changes a `Default` cell's foreground; an OSC 17 override (set directly on the
  overrides struct in the test) changes the selection quad colour.
- Benchmark: `instanced_bg`, `instanced_fg` and the indexed variants before/after (15%).

Deliverable: the change, tests, damage test and bench numbers.

#### 132.11 — OSC 11 pane base fill

Scope:

- `freminal/src/gui/renderer/vertex.rs` / `renderer/gpu.rs` (emit and draw the base fill first
  in both `draw_with_verts` and the cursor-only twin);
- `freminal/src/gui/terminal/widget.rs` (pass the pane rect and theme background);
- `freminal/src/gui/renderer/headless.rs`, `renderer/pixel_harness.rs` (let a
  `SyntheticFrame` carry a background override);
- new golden under `freminal/tests/golden_pixels/` if a golden is the clearest assertion.

What:

- When `resolved_colors.background != theme.background`, the pane emits one quad covering the
  whole `terminal_rect` (including the sub-cell remainder) in `resolved_colors.background`, with
  alpha 1.0 before `u_bg_opacity` (so it matches the panel fill at the configured opacity),
  drawn before the background image and the cell backgrounds. Otherwise nothing is emitted.
- `DefaultBackground` cells stay skipped.
- Tests: unit test that the fill is emitted only when the colours differ; pixel test (runs under
  `xvfb-run -a cargo test -p freminal --features gl-pixel` in the `gl-pixel` dev shell) that an
  empty pane with an override reads back the override colour at a cell pixel and in the
  sub-cell remainder; the existing `default_background_cells_are_left_untouched` still passes.
- Verify the cursor-only and partial-present paths keep the fill (a cursor move with an
  override active must not reveal the theme background).

Deliverable: the fill, tests, and the `gl-pixel` run output.

#### 132.12 — Cells keep the palette index

Scope:

- `freminal-terminal-emulator/src/terminal_handler/sgr.rs` (`handle_sgr` stops resolving;
  `build_sgr_response` then reports `38;5;n` / `48;5;n` / `58:5:n`);
- `freminal-common/src/colors/mod.rs` (`TerminalColor::PaletteIndex` doc comment);
- `freminal-common/src/buffer_states/unicode_placeholder.rs` (doc comments only, if they
  describe the old behaviour);
- tests: `terminal_handler/sgr.rs`, `terminal_handler/mod.rs` placeholder tests,
  `tests/vttest_extensions.rs:376-390`, and any other expectation of `Custom` from `38;5;n`.

What:

- `38;5;n` / `48;5;n` / `58;5;n` store `PaletteIndex(n)`. Nothing else in the handler resolves
  an index.
- Tests: OSC 4 after text written with `38;5;1` changes that text's resolved colour in the next
  snapshot's `ResolvedColors`; a theme change does the same for `38;5;3`; DECRQSS reports
  `38;5;196`; a unicode placeholder written with `38;5;42` decodes image id 42 and with
  `58;5;7` placement id 7; a compact-row / LZ4 round trip keeps `PaletteIndex`.
- Rename the misnamed test `build_sgr_response_palette_index_fg_is_38_5_n` so it tests what its
  name says.
- Benchmark: `bench_handle_sgr_indexed` and `instanced_fg_indexed` before/after (15%).

Deliverable: the change, tests and bench numbers.

#### 132.13 — OSC 4 / 104 / 10–19 / 110–119 parsing and replies

Scope:

- `freminal-common/src/buffer_states/osc.rs` (`OscTarget` gains 117/119; `AnsiOscType`
  colour variants replaced, see below);
- new `freminal-terminal-emulator/src/ansi_components/osc_colors.rs` (parser side, raw
  params), replacing `osc_palette.rs`; `ansi_components/osc.rs` dispatch;
- `freminal-terminal-emulator/src/terminal_handler/osc.rs`, `terminal_handler/osc_colors.rs`;
- tests in those files and any integration test expecting the old variants or 8-bit replies.

What:

- Replace `RequestColorQueryForeground/Background/Cursor`, `ResetCursorColor`,
  `ResetForegroundColor`, `ResetBackgroundColor`, `SetPaletteColor`, `QueryPaletteColor` and
  `ResetPaletteColor` with:
  - `PaletteColors(Vec<PaletteColorOp>)`, `PaletteColorOp { index: u8, request: ColorRequest }`;
  - `ResetPaletteColors(PaletteReset)`, `PaletteReset::{All, Indices(Vec<u8>)}`;
  - `DynamicColors(Vec<(DynamicColorSlot, ColorRequest)>)`;
  - `ResetDynamicColor(DynamicColorSlot)`;
  - `ColorRequest::{Set((u8, u8, u8)), Query}`.
- Parse from the raw parameter bytes (so names with spaces and numeric-looking specs survive),
  with the OSC 4 / 104 / 10–19 / 110–119 semantics in the decisions. Specs go through
  `parse_color_spec`. Unmapped slots 13–16, 18 consume a value and are dropped with a `debug!`.
- Handler: apply each op in order; queries reply in 16-bit. OSC 17/19 set the selection
  overrides; 117/119 reset them.
- Tests: `4;1;red;2;?` sets 1 and replies for 2; `4;1;red;2` is dropped entirely;
  `4;300;red;2;blue` sets only 2; `104;1;2` resets both; `104` resets all; `10;red;blue` sets
  fg and bg; `10;?;?` replies for both; `13;red;17;blue` style chains skip the ignored slot;
  `17;#ff0000` then `17;?` replies `rgb:ffff/0000/0000`; `117` resets; `11;dark slate gray`
  works; replies are 16-bit and ST-terminated (and 8-bit C1 under S8C1T).

Deliverable: the parser, handler, tests. List every removed variant and every changed reply.

#### 132.14 — Documentation

Scope: `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`,
`Documents/PLAN_VERSION_130.md` (reset table row, Task 144 stub, out-of-scope table),
`Documents/KITTY_PROTOCOL_REFERENCE.md` only if it describes colour-spec parsing.

What:

- Dual-doc update per `freminal-escape-sequence-docs`: OSC 4, 10, 11, 12, 17, 19, 104, 110,
  111, 112, 117, 119 rows (formats accepted, chaining, 16-bit replies, render-time palette);
  SGR 38/48/58 edge semantics; OSC 13–16, 18 recognised and ignored. Remove the OSC 10/11 gap.
- Reset table: selection overrides join the dynamic-colour row (RIS only).
- Task 144 stub: it now owns `cursor_text`, `visual_bell` and `@alpha`; it builds on
  `DynamicColorOverrides` and `ResolvedColors`.
- Out-of-scope table: the wide-gamut row is resolved (implemented in 132, CSS-correct).

Deliverable: the doc changes, markdownlint clean.

### 132 Cleanup entries

#### 132.C1 — DECSCNM toggle does not repaint

- **Surfaced:** activation recon (2026-10-11), verified by the orchestrator. Predates Task 132.
- **Impact:** `Mode::Decscnm` only stores the mode (`state/internal.rs:315`). DECSCNM colours
  are baked into the vertex buffers (`renderer/vertex.rs:106-128`), and `frame_dirty.rs` has no
  term for the display mode, so toggling `?5` without other output leaves the pane in its old
  colours until something else forces a rebuild. `TerminalSnapshot::is_normal_display` is also
  a raw `bool` crossing the snapshot boundary (`freminal-state-representation`).
- **Scope of fix:** `freminal-common/src/buffer_states/modes/decscnm.rs` (a two-state transport
  enum, e.g. `ScreenInversion::{Normal, Reversed}`, since `Decscnm` carries `Query`);
  `snapshot.rs`, `interface.rs`, `state/internal.rs`; `freminal/src/gui/terminal/widget.rs`,
  `frame_dirty.rs`; `freminal/benches/render_loop_bench.rs` (field rename).
- **Suggested approach:** reproduce with a frame-dirty test first. Carry the enum in the
  snapshot; add `previous_screen_inversion` to `PaneRenderCache`; a change joins
  `content_changed` and vetoes the bounded path, exactly like `resolved_colors_changed`.
- **Verification:** a test that `CSI ? 5 h` with no other output forces a pane rebuild, and
  `CSI ? 5 l` does too; existing DECSCNM tests pass.
- **Scheduling:** after 132.10 (same seam); resolved within Task 132.

#### 132.C2 — The full-rebuild damage mapping in `show()` is untested

- **Surfaced:** 132.10 review (2026-10-11). Predates Task 132.
- **Impact:** `PaneFrameDamage` for a full pane rebuild is decided inline in
  `TerminalWidget::show()` (`widget.rs`, the `FullRebuildDamage::Full => pane_rect_damage`
  arm). It needs a live egui `Ui`, so no test proves that any pane-wide trigger
  (`theme_changed`, `resolved_colors_changed`, `dims_changed`, …) reports
  `Region(pane rect)` rather than `Full`. The links on either side are tested
  (`frame_dirty.rs` decides `content_changed`; `frame_damage.rs` aggregates `Region`).
- **Scope of fix:** `freminal/src/gui/terminal/widget.rs`.
- **Suggested approach:** the mapping from
  `(FullRebuildDamage, Option<PaneDamageRect>, EmptyBoundedDamage inputs)` to `PaneFrameDamage`
  is a decision with real branches (the
  `Bounded` arm's search-only fallback); per `freminal-extend-or-extract`, give it a named
  function because it is a decision, not only to test it, then unit-test every arm.
- **Verification:** a test per arm, including `Full` with and without a degenerate pane rect.
- **Scheduling:** resolved within Task 132 (pulled back from Task 147 after the adversarial
  review, finding R1, because 132.10 required this test).

#### 132.C3 — `--features gl-pixel` builds with dead-code warnings

- **Surfaced:** 132.10 review (2026-10-11), running the pixel goldens in `.#gl-pixel`.
  Predates Task 132 (`gl_facade/` is untouched on this branch).
- **Impact:** `cargo test -p freminal --features gl-pixel` warns that several `Gl` facade
  methods, `RecordingState::next_query` and four `RecordingState` query methods are never
  used. Clippy with `--all-features` is clean because `frame-profiling` uses them; the
  `gl-pixel`-only combination (what the `gl-pixel` CI job builds) is not.
- **Scope of fix:** `freminal/src/gui/renderer/gl_facade/{facade,recording}.rs` (feature
  gates).
- **Suggested approach:** gate the query-related facade and recording items on the feature
  that uses them (`frame-profiling`), or on `any(test, …)` as appropriate; do not add
  `allow(dead_code)`.
- **Verification:** `cargo build -p freminal --features gl-pixel --all-targets` and the
  `--features frame-profiling` build are both warning-free; add the `gl-pixel`-only
  combination to whatever lint gate is cheapest so it cannot regress.
- **Scheduling:** routed to Task 147 (147.26).

#### 132.C4 — Per-draw quad uploads are missing from the upload byte counts

- **Surfaced:** 132.11 review (2026-10-11). The background-image half predates Task 132.
- **Impact:** under `frame-profiling`, `TerminalRenderer::draw_with_verts` /
  `draw_with_cursor_only_update` return `UploadByteCounts` that omit the quads uploaded at draw
  time: the background-image quad (`draw_background_image`, every frame with an image) and the
  OSC 11 base-fill quad (`draw_base_fill`, every frame with an override). The Task 125
  bandwidth measurements therefore under-count those frames.
- **Scope of fix:** `freminal/src/gui/renderer/profiling.rs` (a new category, its `total`,
  `merge` and flush output), `freminal/src/gui/renderer/gpu.rs` (return the bytes from both
  draw helpers and fold them in).
- **Suggested approach:** one `background_quad_vbo_bytes` category for both quads (they are
  the same kind of per-draw, one-quad upload); the draw helpers return the byte count they
  uploaded.
- **Verification:** a `frame-profiling` test that a frame with a base fill reports exactly
  144 extra bytes in the new category, and a frame with neither reports 0; existing totals
  unchanged otherwise.
- **Scheduling:** resolved within Task 132, after 132.14.

#### 132.C5 — SGR 7 on default colours ignores the default foreground/background

- **Surfaced:** adversarial review, finding R2 (2026-10-11). Predates Task 132.
- **Impact:** `StateColors::color()` / `background_color()` / `underline_color()`
  (`freminal-common/src/buffer_states/cursor.rs`) map `Default` → `White` and
  `DefaultBackground` → `Black` under `ReverseVideo::On` (`default_to_regular`), so reversed
  default text renders ANSI 7 on ANSI 0 instead of the default background on the default
  foreground, and ignores OSC 10/11 and the theme's own fg/bg. xterm and kitty swap the
  current default colours. DECSCNM already does this correctly, so SGR 7 and DECSCNM disagree.
- **Scope of fix:** `cursor.rs` (drop `default_to_regular` from the three accessors);
  `TerminalColor::default_to_regular` deleted if it has no remaining caller; their tests;
  `freminal/src/gui/renderer/vertex.rs` tests whose expectations encode White/Black.
- **Verification:** reversed default text resolves to (background, foreground) of the pane's
  `ResolvedColors`, including under an OSC 10/11 override; SGR 7 + DECSCNM still cancel.
- **Scheduling:** resolved within Task 132.

#### 132.C6 — Adversarial review fixes: common and emulator

Findings R3, R4, R6, R7 (allocation only), R8 (emulator-side coverage), R9, R15, R16, R19.
Resolved within Task 132; details under "132 Adversarial review".

#### 132.C7 — Adversarial review fixes: GUI

Findings R5, R8 (pixel ordering test), R10. Resolved within Task 132.

#### 132.C8 — PR #539 CI and review: MSRV `float_cmp`; unbounded gamut search

- **Surfaced:** PR #539 CI (`Check … 1.96.0` on all four platforms) and a Copilot review
  comment (high severity), 2026-10-11.
- **MSRV:** clippy 1.96.0 flags `float_cmp` on nine `[f32; 4]` equality assertions added to
  `vertex.rs` tests; the local stable toolchain (1.99) does not, so neither the orchestrator's
  checks nor either review caught it. Fixed with bit-exact `assert_same_rgba` /
  `assert_different_rgba` helpers. MSRV clippy is now runnable locally through the flake's
  rust-overlay (`rust-bin.stable."1.96.0"`), and was clean before the push.
- **Gamut search (missed by both reviews):** a finite chroma up to ~1e308 made
  `gamut_map_oklch` run ~1,037 conversion-plus-deltaEOK steps, and one OSC 4 body can carry
  thousands of entries, so output could stall the PTY thread. Fixed bit-exactly: every probe
  above chroma 1.0 provably takes the `max = probe` branch (out of gamut, deltaEOK > JND, or
  non-finite), so those steps are done as plain halvings; the bisection is then capped at 32
  steps with a compile-time proof that the epsilon stop is reached first. A test compares
  against the unaccelerated algorithm bit for bit (C up to 1e308), the orchestrator's
  independent 3000-vector oracle still matches exactly, and a 256-entry adversarial OSC 4
  body parses correctly.
- **Scheduling:** resolved within Task 132.

### 132 Adversarial review (2026-10-11)

One CODE-REVIEW pass over `main...HEAD` after 132.C4. No blockers.

| ID  | Sev   | Finding (short)                                                       | Disposition                         |
| --- | ----- | --------------------------------------------------------------------- | ----------------------------------- |
| R1  | major | 132.10's pane-bounded damage test missing (deferred to 147.25)        | 132.C2, pulled back into Task 132   |
| R2  | major | SGR 7 on defaults ignores default fg/bg (and OSC 10/11)               | 132.C5                              |
| R3  | major | `TerminalHandler::dynamic_color` public, unused, untested             | 132.C6: delete                      |
| R4  | minor | `scale_hex_channel` public and accepts a sign                         | 132.C6: validate, narrow visibility |
| R5  | minor | stale `struct_excessive_bools` allow on `FgRenderOptions`             | 132.C7                              |
| R6  | minor | dynamic mutators compare stored override, not effective colour        | 132.C6                              |
| R7  | minor | OSC 4/104 collect every field; no op cap                              | 132.C6 streams; cap: per spec       |
| R8  | minor | missing tests (RIS/DECSTR selection, 3-value chain, LZ4, pixel order) | 132.C6 / 132.C7                     |
| R9  | minor | `ColorSpecError` class never logged                                   | 132.C6                              |
| R10 | minor | `draw_base_fill` allocates per draw; test-only `without_base_fill`    | 132.C7                              |
| R11 | minor | DECSCNM leaves the sub-cell remainder un-inverted                     | Task 147.27                         |
| R12 | nit   | "132 Types (decided)" out of date                                     | orchestrator (plan)                 |
| R13 | nit   | status notes skip 132.2/3/5/8; MASTER_PLAN note stale                 | orchestrator (plan)                 |
| R14 | nit   | COVERAGE history paragraphs lack a superseded pointer                 | orchestrator (docs)                 |
| R15 | nit   | `ResolvedColors` size assertion looks layout-brittle                  | 132.C6: derive from component sizes |
| R16 | nit   | vttest 256-colour tests assert the table, not the SGR path            | 132.C6: assert end to end           |
| R17 | nit   | unknown SGR colour type `warn!` can repeat                            | not a defect (see below)            |
| R18 | nit   | translucent window: override compounds with the theme panel fill      | routed to Task 144                  |
| R19 | nit   | `ScreenInversion` variants undocumented                               | 132.C6                              |

- **R7 (op cap):** no per-sequence cap, deliberately: xterm and kitty answer every query, and
  the body is already bounded by the parser's OSC size cap (recorded under 132.13).
- **R16 (`ESC[xyzm`):** the separate "non-empty output" check in `sgr_matrix.rs` is not a
  weakening; `CSI x` is DECREQTPARM, so the sequence was never an SGR.
- **R17:** payload-free `warn!` on an unhandled sequence is the repo convention; the pre-132
  code warned on the same input.
- **R18:** fixing it needs a replace blend for one draw, and `blend_func` is not in the frozen
  GL facade surface. Task 144 already owns background-opacity questions; recorded there.

### 132 Final benchmarks (2026-10-11, isolated runs vs `pre132`)

| Bench                               | pre132           | final            | Change   |
| ----------------------------------- | ---------------- | ---------------- | -------- |
| parse_sgr_heavy                     | 57.3 µs          | 50.9 µs          | −11%     |
| parse_sgr_indexed                   | 56.0 µs          | 48.5 µs          | −13%     |
| handle_sgr_indexed                  | 90.1 µs          | 74.2 µs          | −18%     |
| build_snapshot dirty                | 32.40 µs         | 28.9 µs          | −11%     |
| build_snapshot clean                | 126.98 ns        | 128.0 ns         | +1%      |
| build_snapshot partial_dirty        | 24.23 µs         | 21.9 µs          | −10%     |
| instanced_bg 80x24 / 200x50         | 72 / 137 ns      | 72 / 132 ns      | 0 / −4%  |
| instanced_fg 80x24 / 200x50         | 766 / 911 µs     | 764 / 911 µs     | 0 / 0%   |
| instanced_bg_indexed 80x24 / 200x50 | 4.17 / 20.9 µs   | 4.31 / 21.7 µs   | +3 / +4% |
| instanced_fg_indexed 80x24 / 200x50 | 1.764 / 2.006 ms | 1.766 / 1.930 ms | 0 / −4%  |

No regression over the 15% threshold. A first pass run back to back with the render benches
showed build_snapshot +28% / +20%; isolated reruns (twice) did not reproduce it, so it was
contention, not code.

### 132 Confirmation review (2026-10-11)

A second CODE-REVIEW pass over the remediation (`4856c363..8da1091e`): R2–R19 confirmed done;
R1 partial. No blockers or majors. New findings and dispositions:

- **N1 (minor):** the `Bounded`-arm empty-fallback condition in `show()` was still inline
  and untested. Fixed: `empty_bounded_fallback(FrameDirtyObservations, &ChangedRows)` with an
  exhaustive 16-case test; this completes R1 / 132.C2.
- **N2 (nit):** the two-pane damage test passed `PriorVertexState::Built` literally. Fixed:
  derived with `PriorVertexState::of` from each pane's render state. The pane rect is still
  supplied directly (its geometry is covered by `full_pane_rebuild_damage_rect_tests`).
- **N3 (nit):** `StateColors` docs named the old accessors. Fixed.
- **N4 (nit):** a GAPS history paragraph lacked a "closed by" pointer. Fixed.
- **N5 (nit):** `huge_values_never_panic` rename called churn; not a defect, it was R8.
- **N6 (info):** the per-frame `cursor_vert_range` lock and the 256-entry `reset_palette_all`
  compare are negligible; recorded.

### 132 Status notes

- **132.1 (done).** Benches added; criterion baseline `pre132` saved. Medians: parse_sgr_heavy
  57.3 µs; parse_sgr_indexed 56.0 µs; handle_sgr_indexed 90.1 µs; build_snapshot dirty
  32.40 µs / clean 126.98 ns / partial 24.23 µs; instanced_bg 72.04 ns (80x24) / 136.77 ns
  (200x50); instanced_fg 765.67 µs / 910.68 µs; instanced_bg_indexed 4.169 µs / 20.929 µs;
  instanced_fg_indexed 1.764 ms / 2.006 ms. The parser benches are noisy (±15%).
- **132.2 (done).** Pure move; line-multiset diff showed only headers, imports and one
  `pub(super)`.
- **132.3 (done).** `default_palette_rgb` is the one table; index 16 is `(0, 0, 0)`.
- **132.4 (done).** The subtask text's `rgb::0/0` example was wrong (the second `:` belongs to
  the first channel, so it is `WrongChannelCount { found: 2 }`); the empty-channel case is
  tested as `rgb:/0/0` and `rgb:0//0`.
- **132.5 (done).** Table verified by the orchestrator against upstream `rgb.txt` at
  `2b2019ca` (782 names, identical values) and the `COPYING` notice byte-for-byte; a length
  guard skips the lowercase copy for over-long input.
- **132.6 (done).** Verified by the orchestrator against an independent CSS Color 4 oracle
  (written from the spec text, not shared with the implementer): 3000 random `oklch()`/`lab()`
  inputs, in and out of gamut, all bit-exact.
- **132.7 (done).** Behaviour changes, all per the decided SGR semantics: bare `38`/`48`/`58`
  no longer resets the colour (kitty does nothing); `38;2;256;0;0` masks instead of emitting
  `Invalid`; an unknown colour type drops the rest of the sequence. `from_usize_color` and
  `SgrParseError` deleted (no remaining callers). Parser benches vs `pre132`: sgr_heavy −7.4%,
  sgr_indexed −5.0%.
- **132.8 (done).** Types only; named-colour mapping matches the renderer's.
- **132.9 (done).** `ColorState` invalidates the cached `Arc<ResolvedColors>` only on an
  _effective_ change: a palette set/reset compares the effective RGB, `set_theme` treats equal
  themes as unchanged (a `&'static` to a `const` theme has no stable address, so pointer
  equality alone is insufficient), and the cursor slot never invalidates. build_snapshot vs
  `pre132`: dirty −4.9%, clean +1.3%, partial −10.8% (all within noise).
- **132.10 (done).** Every terminal-content colour resolves through `ResolvedColors`; the
  command-duration label also follows the resolved foreground (orchestrator change: it is
  drawn on the terminal surface). Kept theme-based, with reason: cursor (cursor-only path),
  gutter status and progress colours (chrome). `resolved_colors_changed` is proven to set
  `content_changed` and veto the bounded path; the inline `show()` mapping from a full rebuild
  to `Region(pane rect)` has no unit test for any trigger (132.C2). Pixel goldens compared and
  matched under `.#gl-pixel` (llvmpipe). Instance benches vs `pre132`: all within ±4.4%.
- **132.C1 (done).** Reproduced first (five `frame_dirty` tests failed without the trigger).
  `ScreenInversion` replaces the raw `bool` in the snapshot and throughout the renderer
  (`effective_fg`/`effective_bg` no longer take a bool parameter).
- **132.11 (done).** `PaneBackground { opacity, base_fill }` replaces the `bg_opacity`
  parameter of both draw entry points. The fill is drawn with the decoration program from a
  dedicated `base_fill_vbo` (init count 261 → 262: one `create_buffer`); with no override no
  GL call is made and every per-frame recording count is unchanged. Pixel tests (fill covers
  cells and the remainder strip, glyphs over it, scissored partial present, cursor-only arm
  repaints it on a cleared framebuffer) ran under `.#gl-pixel`. Translucent windows: the
  override is drawn at `background_opacity` over the theme panel fill, so at opacity < 1 a
  `(1 − opacity)` share of the theme background shows through; accepted. Its upload is not in
  the `frame-profiling` byte counts (132.C4).
- **132.12 (done).** `handle_sgr` stores `PaletteIndex` unresolved; `ColorPalette::lookup` and
  `ColorState::palette_lookup` deleted (no callers). OSC 4 and a theme change recolour existing
  cells with unchanged row epochs; DECRQSS reports `38;5;n`/`48;5;n`/`58;5;n`; placeholder
  ids from `38;5;42` / `58;5;7` decode as 42 / 7. Benches vs `pre132`: handle_sgr_indexed
  −11%, instanced_*_indexed within ±4.5%.
- **132.13 (done).** The seven OSC colour `AnsiOscType` variants and `AnsiOscInternalType` are
  replaced by `PaletteColors` / `ResetPaletteColors` / `DynamicColors` / `ResetDynamicColor`
  (request types in `buffer_states/osc_color.rs`); `OscTarget` collapses to
  `DynamicColors(10..=19)` / `ResetDynamicColor(110..=119)`. Chaining, 16-bit replies and the
  kitty skip rules as decided. A non-UTF-8 colour value is now a skipped value rather than
  `TerminalOutput::Invalid`. Reviewed, not a defect: a large OSC 4 body of `n;?` pairs
  produces one reply per query with no cap (xterm and kitty do the same; the body is already
  bounded by the parser's OSC size cap, and DA/DSR queries have the same amplification shape).
- **132.14 (done).** COVERAGE and GAPS updated (OSC 10/11 🚧 → ✅; rows added for OSC 17, 19,
  117, 119 and the ignored 13–16/18/113–116/118; SGR 38/48/58 edge semantics; DECSCNM; RIS and
  DECSTR colour behaviour). Reset-table note, Task 144 stub and the wide-gamut out-of-scope row
  updated by the orchestrator. `KITTY_PROTOCOL_REFERENCE.md` and `SGR.md` needed no change.
- **132.C4 (done).** `UploadByteCounts::background_quad_vbo_bytes` counts the base-fill quad
  (144 bytes) and the background-image quad (96 bytes) on full and cursor-only frames; added
  to the flush line. Compiled away without `frame-profiling`.
- **132.C2 (done).**
  `decide_full_rebuild(VertexRebuild, FrameDirtyObservations, PriorVertexState)` and
  `pane_damage_for_full_rebuild(Option<PaneDamageRect>)` named and
  unit-tested; `osc4_change_in_one_of_two_panes_damages_only_that_pane` runs the real chain to
  `decide_frame_damage` and gets `Partial` with exactly that pane's rect. The never-built read
  of `cursor_vert_range` is now taken on every non-skipped frame (one uncontended lock).
- **132.C5 (done).** SGR 7 swaps the stored colours verbatim (defaults kept), so reversed
  default text shows the resolved default background on the resolved default foreground,
  OSC 10/11 included; `TerminalColor::default_to_regular` deleted. Found in review:
  `underline_color()` substituted the background under SGR 7, which is wrong under DECSCNM
  (which it cannot see); it now returns the stored colour and the renderer's existing
  `DefaultUnderlineColor` → effective-foreground path handles all four SGR 7 × DECSCNM cases
  (orchestrator change, with a test over the four cases). COVERAGE SGR 7 row updated.
- **132.C6 (done).** R3 `dynamic_color` deleted; R4 `scale_hex_channel` private and rejects
  signs; R6 dynamic and reset-all mutators invalidate only on an effective change; R7 OSC 4/104
  stream their fields; R8 RIS/DECSTR for OSC 17/19, three- and four-value OSC 10 chains, an LZ4
  `PaletteIndex` block round trip; R9 skip reasons (`SkipReason`, payload-free) at debug;
  R15 size assertion derived from components; R16 vttest 256-colour tests assert the resolved
  colour; R19 variant docs.
- **132.C7 (done).** R5 stale `struct_excessive_bools` allow removed; R10 the base-fill quad
  is a stack array from `quad_vertices` (the single definition `push_quad` now appends),
  `without_base_fill` removed, the cfg-paired draw calls collapsed; R8 the glyph-over-fill
  pixel test is a font-independent ordering property over two captures (ran under
  `.#gl-pixel`). C6's first commit attempt was rejected by `xtask-check` ("files were modified
  by this hook") because the orchestrator started the C7 agent concurrently with the commit;
  retried alone, it passed.
- **Merged** 2026-10-11 via PR #539 (`9a04f73e`).

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
  maintainer to route.

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
  - **Pi=3 (ReGIS)** answers status 3; any other Pi answers status 1.
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

#### 147.22 — XTGETTCAP answers `indn`, `rin` and `query-os-name` <!-- codespell:ignore rin -->

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

#### 147.24 — Alt sends ESC (`?1036` / `?1039`) and macOS Option as Alt

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
- 147.19, 147.26, 147.27, 147.28 and 147.29 are independent.
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

### 147 Status notes

(none yet)

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
  `bounded(REPLY_QUEUE_CAPACITY = 256)` one for terminal-generated replies.
  `run_writer_loop` (`io/pty.rs`) uses `select_biased!` with user input first, so keystrokes
  no longer queue behind replies. A reply and a keystroke pending at the same moment can now be
  written in either order, user first. No locks; the GUI is unchanged. The handler sends
  replies with `try_send`; on a full queue the reply is dropped and counted (`dropped_replies`,
  rate-limited warn). The DECCOLM `PtyWrite::Resize` is not a reply: review found it would
  block the PTY thread on a full queue, so a new `control_tx` routes it through the user
  channel. XTGETTCAP answers at most `MAX_XTGETTCAP_NAMES` = 64 names per request and never
  echoes a name longer than 128 hex characters (freminal policy; xterm-412 answers every
  name). `new_headless` keeps one unbounded channel for the fuzz harness. Tests: 10,000 DA1
  queries with no reader leave exactly 256 queued; a keystroke sent while the writer is blocked
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
  `FALLBACK_MONITOR_EXTENT` (16384 logical px). `freminal-windowing` never populates
  `monitor_size`, so today the fallback is what bounds the window (148.C4).
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

    | Target | Dictionary | cov | ft | corpus |
    | --- | --- | --- | --- | --- |
    | `terminal_state` | with | 15356 | 72015 | 8892 |
    | `terminal_state` | without | 14576 | 65561 | 7470 |
    | `ansi_parser` | with | 4668 | 26402 | 6449 |
    | `ansi_parser` | without | 4725 | 27181 | 6707 |

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
- **148.C4 — `viewport().monitor_size` is never populated.** Surfaced in review of 148.17.
  `freminal-windowing` calls `egui_winit::State::take_egui_input` but not
  `egui_winit::update_viewport_info`, so `monitor_size` (like other `ViewportInfo` fields
  eframe fills) is always `None`. XTWINOPS resize and move clamp to the 16384 px fallback
  rather than the real monitor, so an opted-in user (`allow_window_ops = true`) can still be
  moved off a smaller monitor. Scope: populate `monitor_size` from winit's
  `current_monitor()` in the windowing crate. Changing egui viewport state falls under
  `freminal-egui-upgrade` / `EGUI_UPGRADE_ASSUMPTIONS.md`, so walk that checklist. Not
  scheduled in Task 148 (the default is off).
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
- **148.C9 — Reply bound is per message, and GUI-side queues stay unbounded.** Review finding
  R5. `REPLY_QUEUE_CAPACITY` (1024) bounds messages, not bytes: with
  `allow_clipboard_read = true`, OSC 52 query replies carry the clipboard. Upstream of it,
  `window_cmd_tx` (PTY to GUI report requests) and `input_tx` (GUI replies back) are
  unbounded, so a non-draining GUI still amplifies. Scope: a byte budget (an atomic counter
  shared by handler and writer, no locks) and bounds on the two GUI channels, per
  `freminal-architecture`. Not scheduled in Task 148.
- **148.C10 — OSC 7 / spawn-cwd follow-ups.** Review finding R9.
  - `resolve_spawn_cwd` runs `is_dir()` (and `read_cwd`, which uses `sysinfo` on macOS and
    Windows) on the GUI thread at new-tab or split time. An attacker-chosen path on a hung
    network mount can stall the GUI. Move it off-thread or add a deadline.
  - On Windows, an OSC 7 path (`/C:/...`) never passes `is_dir()`, so OSC 7 is silently
    unused there.
  - `last_session.toml` files written by older builds can carry an unvalidated cwd, and
    restore uses them as-is.

  Not scheduled in Task 148.
- **148.C11 — Notification worker is never restarted.** Review finding R11a. If the
  `freminal-notify` worker thread exits (panic), `OnceLock` keeps the dead sender and system
  notifications stay off until freminal restarts; each attempt logs `WorkerGone`. Scope:
  respawn on `WorkerGone`, which needs an atomically swappable sender (`ArcSwap`), not a
  `OnceLock`. Not scheduled in Task 148.

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
  - `o=z` inflation is capped at `MAX_KITTY_DATA_BYTES` only (Task 129), not at the size
    implied by `s`/`v`/`f` (129 review; written here by Task 147).
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
  Legacy Alt+text (ESC prefix, `?1036`/`?1039`, macOS Option as Alt) moved to Task 147.24.
  Super on text keys, Alt+Backspace (B3) and the kitty-protocol encoding remain here.
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
- **B11, keypad regressions** from the raw intercept: NumLock-off navigation keys and
  text-producing keypad keys sent as `CSI u`. The "DECKPAM ignored" half moved to Task 147.5
  (legacy application keypad sends SS3).
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
- Shipping CapsLock/NumLock/ScrollLock key events under flag 8 without the lock bits.

### Task 138 — Desktop Notifications (OSC 99) Conformance (activated)

Activated and decomposed on 2026-10-12. The audit findings, decisions and subtasks now live
under "Task 138 — Desktop Notifications (OSC 99) Conformance" above.

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
- RIS and DECSTR clear both screens' stacks, as the Task 131 decision records for the other
  screen-scoped kitty state (written here by Task 147).

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
- **Owned here, not by Task 132 (maintainer decision 2026-10-11):** the `cursor_text` and
  `visual_bell` colours (fields, rendering and setters; their only setter is OSC 21; today
  `ThemePalette::cursor_text` has no reader and the bell colour is hard-coded) and kitty's
  `@alpha` colour suffix (meaningful only for `transparent_background_colorN`; the shared
  colour-spec parser rejects `@`).
- **Builds on Task 132:** add the new slots to `DynamicColorOverrides` /
  `DynamicColorSlot` and, where they are content colours, to `ResolvedColors`; parse OSC 21
  values with the shared `parse_color_spec` (which already accepts names, `rgbi:`,
  `oklch()` and `lab()`); push/pop must go through `ColorState`'s mutators so the resolved
  `Arc` and the GUI's damage trigger follow.

**Depends on.**

- Task 132: rendered dynamic colours and render-time palette lookup. Without them a pop does
  not recolour existing text.
- Task 128: `#` routing.

**Open questions.**

- `transparent_background_color1..7` (spec table) versus `1..8` (kitty code).
- Whether per-colour background opacity is in scope or split out.
- From Task 132 review R18: with `background_opacity` < 1, the OSC 11 base fill is drawn at
  that opacity over the theme panel fill, so a `(1 − opacity)` share of the theme background
  shows through. A replace blend for the fill would fix it but needs `blend_func` in the
  frozen GL facade; decide here alongside per-colour opacity.

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
