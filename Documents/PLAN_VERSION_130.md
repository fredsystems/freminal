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

**Phased point releases (maintainer decision, 2026-10-08).** The work is grouped into
milestones that ship as point releases. This document is the overview: goal, task summary,
reference specs, the compliance audit, the common-work analysis and the design decisions that
span milestones. Each milestone's tasks live in their own document:

| Milestone | Codename                                   | Tasks                        | Plan document                                         | Status   |
| --------- | ------------------------------------------ | ---------------------------- | ----------------------------------------------------- | -------- |
| v0.13.0   | Foundations, Safety & Small Conformance    | 126–132, 138–140, 147–151    | `PLAN_VERSION_130_0.md` and `PLAN_VERSION_130_0_b.md` | Complete |
| v0.13.1   | Shipped-Protocol Conformance (heavy)       | 134, 135, 136, 137, 152      | `PLAN_VERSION_130_1.md`                               | Planned  |
| v0.13.2   | New Protocols: Small & Medium              | 103, 141, 142, 143, 144, 145 | `PLAN_VERSION_130_2.md`                               | Planned  |
| v0.13.3   | New Protocols: Text Sizing, Transfer, Clip | 133, 104, 102, 146           | `PLAN_VERSION_130_3.md`                               | Planned  |
| v0.13.4   | Drag & Drop                                | 105                          | `PLAN_VERSION_130_4.md`                               | Planned  |

**Tiered decomposition.** Per `plan-decomposition`, a task carries a full subtask breakdown
only once its milestone is activated. Every v0.13.0 task was decomposed and is complete.
Every task from v0.13.1 onward is an **enriched stub**: goal, audit findings, durable
decisions and open questions, with no subtasks. Tasks 102, 103 and 104 were previously
decomposed; the audit found factual errors in those breakdowns (for example, OSC 66 is not a
Contour code, and the 102 wire keys were wrong), so they are enriched stubs again and get
re-decomposed at activation.

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
| 139 | Pointer Shapes (OSC 22) Completion          | v0.13.0   | M     | Complete | 129, 130, 131                      |
| 140 | Underline & SGR Parity                      | v0.13.0   | S     | Complete | None                               |
| 147 | v0.13.0 Correctness Cleanup                 | v0.13.0   | L     | Complete | 130, 131                           |
| 148 | Security & Fuzz Findings Remediation        | v0.13.0   | XL    | Complete | None                               |
| 149 | zbus 6 Migration & Notification Backends    | v0.13.0   | M     | Complete | 138                                |
| 150 | Desktop Notification Backend Extraction     | v0.13.0   | L     | Complete | 149                                |
| 151 | Own TERM & Terminfo Distribution            | v0.13.0   | M     | Complete | 140                                |
| 134 | Unicode Width & Segmentation Conformance    | v0.13.1   | XL    | Planned  | None                               |
| 135 | Graphics Protocol Conformance               | v0.13.1   | L     | Planned  | 126.2, 129, 130, 131               |
| 136 | Graphics Placement Model & Z-Layers         | v0.13.1   | XL    | Planned  | 135                                |
| 137 | Keyboard Protocol Conformance               | v0.13.1   | XL    | Planned  | 128, 131; architecture sign-off    |
| 152 | macOS Option as Alt & Per-Side Alt          | v0.13.1   | M     | Planned  | 147; 137 coordination              |
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
execution order inside each milestone is given in its own plan document.

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

- Milestones ship in order: v0.13.0, then v0.13.1, v0.13.2, v0.13.3 and v0.13.4.
- v0.13.1 onward is activated per milestone, against the code as it then exists. Activation
  re-reads the stub, resolves its open questions with the maintainer, and decomposes it in the
  milestone's own plan document.
- The v0.13.0 execution order (foundations first, then parallel worktrees) is recorded in
  `PLAN_VERSION_130_0.md`.

Every implementation subtask leaves `cargo xtask test` and
`cargo clippy --all-targets --all-features -- -D warnings` green. Every PR runs
`cargo machete` and `cargo xtask check-windows`.

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
