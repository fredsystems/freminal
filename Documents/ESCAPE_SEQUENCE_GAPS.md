# Escape Sequence Gaps

Last updated: 2026-11-20 — Task 147 (v0.13.0 correctness cleanup). Added: the
multi-byte designators (`ESC ( % 5`) as not planned (Task 147 review). Closed and
removed: the DECRC scroll-region clamp row (a decision, not a gap: DECRC with
DECOM set clamps to the screen only, as kitty, Ghostty and WezTerm do; xterm alone
also clamps to the bottom and right margins, not the top or left); "DECSC saved
state" (147.6: the pending-wrap flag is saved); "Saved cursor on resize,
alternate active" (147.18: the saved cursor follows its text through a resize
and reflow); "Sixel placement under DECSDM" (147.19: the image is drawn at the
top-left, clipped, with no scrolling and no cursor move, as xterm); the whole
charset section's open items (147.20: SO / SI, LS2 / LS3, `ESC ) C` / `ESC * C`
and G3 now work through a full G0 to G3, GL, GR and single-shift model); the
"DECRQM for ANSI modes" section (147.1: unknown ANSI modes answer in the ANSI
form, issue #528); the "Mode changes take effect per read" paragraph (147.2:
S8C1T, DECANM and RIS govern the next byte); and, from the 8-bit C1 section, the
event-encoding sentence (147.3, 147.4: key, focus, mouse and bracketed-paste
introducers honour S8C1T) and the UTF-8 title sentence. That last one is closed
as matching xterm, the only reference terminal with S8C1T: it sends raw UTF-8
title and icon replies in S8C1T mode (`charproc.c:8950-8973`,
`ptydata.c:614-629`). Recorded as not planned, with the reasons in
`ESCAPE_SEQUENCE_COVERAGE.md`: KAM, SRM, `?1001`, `?1034` and `?1035`
(147.21; the `?1034` row described `?1036` and is corrected), the HP escapes
`ESC F`, `ESC l` and `ESC m` (147.23), XTSMTITLE / XTRMTITLE (147.9), and the
96-character charset designators and GR application to bytes `0x80` and above
(147.20). New open row: macOS Option as Alt (Task 152; 147.24 shipped `?1036`
without the macOS half). Alt on text keys is no longer dropped on the legacy
keyboard path (147.24); under a kitty keyboard flag it still is (Task 137).
XTSMGRAPHICS, `?1036` and `?1039` need no gap row. The XTGETTCAP expansion (147.22)
moved to Task 151.

Last updated: 2026-11-19 — Task 151 (own `TERM` and terminfo). The "XTGETTCAP
capability expansion" row in "Priority 2 — Polish" is removed (it was Task 147.22,
which moved to Task 151.2): `indn`, `rin` <!-- codespell:ignore rin -->
and `query-os-name` are answered, `Se` is `CSI 0 SP q`, and `TN` is the pane's
actual `TERM`, which is `xterm-freminal` by default. No new gap is added.
`kitty-query-os_name` (kitty's own namespace) is deliberately not answered.

Last updated: 2026-11-19 — Task 151.3 (terminfo entry corrections). New row in
"CSI Gaps": SGR 8 (conceal) is parsed to `Conceal` but `apply_sgr` ignores it, so
`res/freminal.ti` no longer declares `invis` and the `sgr` string no longer has
the `%?%p7%t;8%;` part. The entry now matches what freminal implements, and the
XTGETTCAP `Se` answer is `CSI 0 SP q` (DECSCUSR 0 restores the configured style).

Last updated: 2026-11-19 — Task 140 (underline and SGR parity), closed. The
"CSI 221 m / CSI 222 m" row in "CSI Gaps" is removed: SGR 221 (bold off only) and
SGR 222 (faint off only) are implemented, SGR 21 is double underline instead of
bold-off, `4:N` follows kitty (N above 5 is dashed), underlines are clamped to the
cell and laid out per cell, and the bundled `xterm-freminal` terminfo entry lists
`Su` / `Smulx` / `Setulc` (when this was written programs got
`TERM=xterm-256color` and discovered them through XTGETTCAP; since Task 151 they
get `TERM=xterm-freminal` by default). The open "erase format" question is
resolved too: erased cells take only the current background colour. See
`ESCAPE_SEQUENCE_COVERAGE.md`. Not gaps, but deliberate differences from kitty,
recorded in `KITTY_PROTOCOL_REFERENCE.md`: an empty `4:;` is a single underline
(kitty gives no underline there, an artefact of its parser, and freminal's SGR
tokeniser cannot tell it from a trailing `4:`); erase is background-only (kitty
copies the whole pen); and double-underline lines use the font stroke thickness
rather than kitty's 1 px rows. Whole-row IL/DL/SU/SD and line-feed fills use
default attributes, and a dashed cell too narrow to hold two separate dashes is
drawn solid; both match kitty. Nothing in underlines or SGR
21/221/222 remains open.

Last updated: 2026-11-19 — Task 139 (OSC 22 pointer shapes), closed. The OSC 22
row in "OSC Gaps" is removed: set, push, pop and query now work on per-screen
stacks cleared by RIS and DECSTR, with kitty's name table and parsing rules (see
`ESCAPE_SEQUENCE_COVERAGE.md`). No other gap row was itemised for the rest of
Task 139 (the name table, the stack depth, the reply framing, the unset entry).
Not gaps, but deliberate differences from kitty recorded in
`KITTY_PROTOCOL_REFERENCE.md`: a 64-name cap per request, a support query that
answers `1` for every accepted name including aliases, `__default__` /
`__grabbed__` reporting freminal's own `default`, and `col-resize` / `row-resize`
accepted as aliases. Nothing in OSC 22 remains open.

Last updated: 2026-10-22 — Task 150 (desktop notification backend extraction).
No gap row is added or removed and no behaviour changes: the notification
backends moved from `gui/osc99/` to the `freminal-notify` crate
(`freminal-notify/src/`).

Last updated: 2026-10-22 — Task 149 (zbus 6 migration and per-platform
notification backends, #567). No gap row is added or removed. OSC 9, OSC 777,
OSC 133 `D` command-finished and OSC 99 desktop notifications now use one
native backend per platform: freminal's own zbus 6 service on Linux/BSD,
`tauri-winrt-notification` on Windows, and `notify-rust` on macOS only. The
changes to their presentation (the Linux/BSD hints, escaping that follows the
daemon's `body-markup` capability, a cap of 16 OSC 9 / 777 / 133 `D`
notifications in flight, and Windows `Error` notifications as ordinary toasts)
are recorded in `ESCAPE_SEQUENCE_COVERAGE.md`.

Last updated: 2026-10-19 — Task 148 (security and fuzz findings remediation).
No gap row is added or removed: every Task 148 change is a limit, policy or
crash fix on an implemented sequence, recorded in `ESCAPE_SEQUENCE_COVERAGE.md`
(REP, BS, wide glyphs, DECSLRM, Sixel, OSC 0/1/2, OSC 7, OSC 8, OSC 52,
OSC 1338, `CSI t`, XTGETTCAP, kitty graphics file media, bracketed paste).
XTGETTCAP answering at most 64 names per request is a deliberate freminal limit
(xterm answers every name). The mutating `CSI t` window operations and kitty
graphics `t=f` / `t=t` / `t=s` are off by default (`[security] allow_window_ops`,
`allow_kitty_file_transfer`); they work once enabled. A confirmation prompt
instead of a plain off switch waits for Task 133.

Last updated: 2026-10-12 — Task 138 (OSC 99 conformance), closed. The OSC 99 row
in "OSC Gaps" (Kitty notifications) is removed. Its last open item was the eight
standard icon names, which the spec requires every implementation to support:
they are now shown as bundled-font glyphs on the toast (138.C4) and as
rasterised glyph images on the macOS/Windows system leg (138.C5), as well as
through the freedesktop mapping on Linux/BSD. OSC 99 conforms to kitty 0.49.2:
chunk metadata merges instead of being clobbered by defaults, buttons are
reported 1-based, `a=report` no longer disables focus, update in place,
`p=close`, expiry and the close report after an activation work, and `p=?` is
truthful per routing, platform and daemon. No other gap row was itemised for the
rest of Task 138 (per-pane lifecycle, pane-aware `o=`, `a=focus`,
`osc99_filters`, icons, sounds, category, the zbus Linux/BSD service, bounded
reassembly); they are recorded in `ESCAPE_SEQUENCE_COVERAGE.md`. Not a gap:
macOS and Windows cannot observe activation or close and have no update in
place, expiry or buttons, so their `p=?` omits those features (the OS limit,
recorded in the coverage notes). Nothing in OSC 99 remains open. The
confirmation-review cleanups (138.C18–138.C22) opened no gap: the stripped-then-
limited text order and the silent drop of an OSC body over 1 MiB (before OSC 99
sees it; spec senders use chunks of 4096 bytes or less) are deliberate
differences, recorded in `KITTY_PROTOCOL_REFERENCE.md`.

Last updated: 2026-10-11 — Task 132 (colour foundation). Closed and removed from
"OSC Gaps": "OSC 10/11 set" — an override now reaches rendering (default
foreground; default background through a pane base fill), and OSC 17 / OSC 19
(selection colours) are implemented and rendered. No other gap row was itemised
for the colour-spec parser, OSC 4 / 104 multi-entry forms, OSC 10-19 chaining,
16-bit replies, render-time palette resolution, the SGR 38 / 48 / 58 edge cases
or the DECSCNM repaint; they are recorded as fixed in
`ESCAPE_SEQUENCE_COVERAGE.md`. Still open and owned by Task 144: OSC 21, the
colour stack (OSC 30001 / 30101, `CSI # P` / `# Q` / `# R`), kitty's `@alpha`
colour suffix, and the `cursor_text` and `visual_bell` colours (their only
setter is OSC 21).

Last updated: 2026-10-10 — Task 131 (screen-scoped state and reset lifecycle).
Closed and removed from "Buffer Semantics Gaps": the DECSC save slot per screen
(131.5, 131.6) and DECRC with nothing saved (131.C2). Narrowed: "DECRC and DECOM"
and "DECSC saved state" (131.C1 — DECOM, the SGR rendition and the character set
are now saved and restored; the DECOM scroll-region re-clamp and the pending-wrap
flag remain). No other gap row was itemised for the alternate-screen modes
(`?47`/`?1047`/`?1049`), the RIS and DECSTR resets, `?45`'s default, the
per-screen kitty keyboard stack (B15) or per-screen kitty placements; they are
recorded as fixed in `ESCAPE_SEQUENCE_COVERAGE.md`.

Last updated: 2026-10-10 — Task 130 (reverse-path and capability-query
consistency). Closes the LNM half of the ANSI-mode DECRQM gap (`CSI 20 $ p` now
answers `CSI 20 ; Ps $ y`, 130.C1) and the DECSCLM no-reply case (`CSI ? 4 $ p`
now answers `CSI ? 4 ; 4 $ y`, 130.C2); the unknown-ANSI-mode half stays open
(issue #528). Closes the OSC 99 `p=?` "answered after DA1" defect
(130.5): it is answered in stream order, gated on configuration and platform,
and every OSC 99 request is ignored while OSC 99 is unsupported. The IRM half of
issue #528 is also closed: `CSI 4 $ p` answers `CSI 4 ; Ps $ y` and no longer
switches insert mode off. Also recorded (no gap
rows were itemised for them): DECRPM, `CSI ? u` and every GUI-originated reply
now honour S8C1T (130.3, 130.6, 130.7); replies are produced in byte-stream
order and RIS applies at its stream position (130.1); tmux DCS passthrough runs
every inner sequence through the real parser and never wraps replies (130.2);
title, icon-label and OSC 52 replies no longer echo control characters
(130.C3). OSC 99 conformance beyond this was closed by Task 138 (see the
2026-10-12 entry above). See `ESCAPE_SEQUENCE_COVERAGE.md`.

Last updated: 2026-10-10 — 129.C5 closed: a C0 byte inside an OSC made the
parser emit `Invalid` and print the rest of the payload as text. Control bytes
now follow ECMA-48 / DEC VT500 (CAN/SUB cancel silently; other C0 and DEL are
ignored). No gap row was itemised for it.

Last updated: 2026-10-09 — Task 129 (kitty wire infrastructure) — wire-level
bugs fixed; no gap row closes because none was itemised here (they were
recorded in the `PLAN_VERSION_130.md` audit under "Pre-existing bugs outside
the kitty surface"). Closed: (129.4) an OSC payload ending in `\` lost it
(`OSC 0;C:\ BEL` set the title `C:`); only the terminator is stripped now.
(129.5–129.8) a non-UTF-8 byte anywhere invalidated every OSC; every target now
gets its raw body and only the token targets (4, 10, 11, 12, 22, 52, 104) still
reject non-UTF-8. (129.6) OSC 0/1/2 titles and OSC 7 URIs containing `;` were
truncated; they now take the full remainder. (129.7) OSC 8 URIs containing `;`
closed the hyperlink and `id=` was stored as the whole params field; both are
fixed. (129.9, 129.11) OSC, APC, DCS and the kitty and iTerm2 chunk
accumulators were unbounded; they are capped (OSC 1 MiB, OSC 52 / 1337 64 MiB,
APC 1 MiB, DCS 64 MiB, kitty graphics 400 MiB total, iTerm2 multipart 64 MiB;
over-cap input is consumed and dropped, warn-logged with its length only).
(129.12) OSC 99 base64 payloads split mid-quantum across chunks decoded to the
wrong bytes; they now decode as one stream. (129.13) kitty graphics replies
ignored S8C1T; they are now 8-bit framed when it is active. (129.14) `DCS
@kitty-*` kitten strings are consumed silently; a new not-planned row records
them under "DCS / Graphics Gaps". Kitty graphics remains not conformant (Tasks
135, 136); the OSC 99 conformance gap was closed by Task 138 (see the 2026-10-12
entry above). See `ESCAPE_SEQUENCE_COVERAGE.md`.

Last updated: 2026-10-09 — Task 128 (strict CSI dispatch) — the CSI router now
matches on private-marker prefix, intermediate and final byte together, so the
misroutes recorded under "CSI Gaps" are fixed: `CSI Ps + T`, `CSI Ps # P`,
`CSI … $ r`, `CSI Ps * x`, `CSI ? s` / `CSI ? r` and `CSI > … SP q` are now
recognised and ignored (warn-logged, no output) until their tasks land. The
DECSCA claims are corrected: before Task 128, DECSCA (`CSI Ps " q`) was
misrouted to DECSCUSR and changed the cursor style; it now genuinely has no
effect (recognised and ignored). Selective erase (DECSED / DECSEL) remains
unimplemented because the buffer has no protected-cell bit. Other forms that
used to misroute and are now recognised and ignored are DECRARA
(`CSI … $ t`, not planned), `CSI Ps SP t`, DECLL (`CSI Ps q`), `CSI Ps SP u`,
SL (`CSI Ps SP @`) and `CSI > SP c`. DECSCUSR now requires its SP
intermediate; a bare `CSI Ps q` is no longer accepted. See
`ESCAPE_SEQUENCE_COVERAGE.md` for the updated rows.

Last updated: 2026-10-08 — Task 126.4 (v0.13.0 audit truth reset) — the
2026-10-08 kitty-compliance audit (`PLAN_VERSION_130.md`) overturned two
claims in this document: "Kitty graphics is fully implemented" and "Kitty
keyboard protocol is substantially compliant". Both are replaced by pointers
to the v0.13.x tasks (graphics: Tasks 135, 136; keyboard: Task 137), and OSC 99
conformance (Task 138, closed 2026-10-12; see the entry above) is added. New gap
entries, each with its planned task: OSC 21 / colour stack (Task 144), OSC 22
pointer shapes (Task 139; closed 2026-11-19, see the entry above), OSC 5522 (Task 146), OSC 5113 (Task 102), OSC 72
(Task 105), OSC 10/11 set not reaching rendering (Task 132; closed by the
2026-10-11 entry above), a new "CSI Gaps" section for unscroll (Task 142),
DECCARA / DECSACE (Task 143), XTSAVE/XTRESTORE, `CSI 22 J` and the mouse-leave
report (Task 141), SGR 221/222 (Task 140) and multiple cursors (Task 103). The
OSC 66 entry is corrected: Contour does not use OSC 66 (its notification is
`CSI ? 996 n` / `?2031`); OSC 66 is kitty text sizing (Task 104) and the
handler is a vestigial no-op. Behaviour changes that landed with the safety
gate: (126.1) OSC 4/10/11/12 colour specs with non-ASCII or non-hex digits,
including sign-prefixed forms such as `#+a+a+a`, are now rejected instead of
panicking or being accepted; (126.2) kitty graphics `t=f`/`t=t` file
transmission is hardened (canonicalised paths, `/proc`, `/sys`, `/dev` except
`/dev/shm` refused, non-regular files refused, 400 MiB cap, uniform
`EBADF:Failed to read image file` for every failure, and `t=t` deletes only
`tty-graphics-protocol` files inside a temporary directory); (126.3)
`CSI > … <intermediate> q` emits nothing and XTVERSION answers only
`CSI > q` / `CSI > 0 q`. The CSI misroutes noted below (`CSI # P`,
`CSI … $ r`, `CSI * x`, `CSI Ps + T`) were fixed by Task 128 (see the
2026-10-09 entry above).

Last updated: 2026-10-05 — PR #527 review — recorded the ANSI-mode DECRQM
(`CSI Pa $ p`) defects under "CSI Standard Mode Gaps" below: replies use the
DEC-private form, and an IRM query clears insert mode (issue #528). The same
review made DECRQM ignore malformed intermediates and queries with no mode
number (see ESCAPE_SEQUENCE_COVERAGE.md).

Last updated: 2026-10-05 — issue #507 — the "OSC 9;4 ConEmu progress UI"
gap is closed. Per-pane OSC 9;4 progress state is now resolved into a typed
`ProgressReport`, transported on `TerminalSnapshot`, and rendered as a bar
across the top of each pane (static, non-animated, for indeterminate),
gated for display only by the new `[progress] enabled` config option and
cleared on `s=0`, RIS, and DECSTR; a hardcoded 15s staleness timeout
(matching ghostty) covers programs that die without clearing their
progress. The "OSC gaps" summary bullet, the "OSC 9;4 ConEmu progress UI"
row in the OSC Gaps table, and the "OSC 9;4 ConEmu progress UI" row in the
Priority 2 roadmap table are all removed. No tab-level aggregation was
built; if a tab-level summary is added later, the agreed rule is
worst-state-wins (Error > Paused > Indeterminate > In progress > Inactive).
See `Documents/ESCAPE_SEQUENCE_COVERAGE.md` for the full coverage-row
update.

Last updated: 2026-10-05 — issue #507 — implementing DECSTR (`CSI ! p`,
Soft Terminal Reset, scoped into #507 for clearing per-pane progress state
on soft reset) surfaced two pre-existing gaps that were not previously
tracked in this document: KAM (keyboard action mode) has no representation
in freminal (added to "CSI Standard Mode Gaps" below), and DECSCA (select
character attribute) has no effect because the buffer's cell model has no
per-cell protected-character bit (at the time of this entry DECSCA was in fact
misrouted to DECSCUSR and changed the cursor style; Task 128 made it a true
no-op), which as a consequence also leaves
DECSED/DECSEL (selective erase) unimplemented (added to "Buffer Semantics
Gaps" below). Neither is a regression from DECSTR; both were already true
and are now recorded. See `Documents/ESCAPE_SEQUENCE_COVERAGE.md` for the
full DECSTR row and `TerminalHandler::soft_reset` for the complete list of
Table 5-9 items freminal does not model.

Last updated: 2026-10-04 — Task 125.C6 review — recorded five known
divergences from xterm in cursor save/restore (DECSC/DECRC, `CSI s`/`CSI u`,
`?1048`/`?1049`) and one in Sixel placement under DECSDM (`?80`); see "Buffer
Semantics Gaps" and "DCS / Graphics Gaps" below. Documentation only: no
behaviour changed and none of the six is scheduled.

Last updated: 2026-10-04 — Task 125.C14 — kitty `d=x`/`d=y` use 1-based screen
coordinates and `d=c` intersects only the cursor cell (see
ESCAPE_SEQUENCE_COVERAGE.md). No gap entries added or removed.

Last updated: 2026-10-04 — Task 125.C8 — kitty `d=p`/`d=q` now interpret `x=`/`y=`
as 1-based screen cells (see ESCAPE_SEQUENCE_COVERAGE.md). No gap entries added
or removed.

Last updated: 2026-10-04 — Task 125.C6 — DECSC/DECRC now restore a
screen-relative cursor position (xterm behaviour; see ESCAPE_SEQUENCE_COVERAGE.md).
No gap entries added or removed.

Last updated: 2026-08-26 — issue #502 — valid ConEmu `OSC 9;4` progress
reports are now recognized and silently consumed instead of being misrouted
as desktop notifications. Visual per-pane progress state remains unimplemented
and is tracked by issue #507. Protocol reference:
<https://ghostty.org/docs/vt/osc/conemu>. Earlier: 2026-07-25 — issue #433 — OSC 9/777 per-source notification
enable toggles now enforced (see ESCAPE_SEQUENCE_COVERAGE.md). No gap
entries changed: the OSC 9 ConEmu progress-report gap below is unrelated
and unaffected. Earlier: 2026-07-08 — Task 115 (v0.11.1) closed the DECSCNM
cell-level fg/bg swap renderer gap: DECSCNM (?5) now performs a per-pane,
per-cell foreground/background swap at render time in the vertex builders
(`freminal/src/gui/renderer/vertex.rs`), XOR-composed with per-cell SGR-7
reverse video. The previous behavior — forcing the egui window
chrome/`panel_fill` to solid white instead of touching individual cells —
was removed, and the "DECSCNM — Reverse Video (?5)" row and the
"DECSCNM cell-level fg/bg swap" roadmap row are both removed from this
document (see "Renderer Gaps" and "Roadmap by Priority" below — the
Renderer Gaps table is now empty of DECSCNM). Also 2026-07-08 — Task 117
(v0.11.1) closed two of the three
buffer-semantics gaps added earlier today by the drift-reconciliation pass
(see "Earlier" below) and both entries are removed from this document: (1)
double-width/double-height rows now halve the auto-wrap column
(`Buffer::insert_text`, `freminal-buffer/src/buffer/mod.rs:355-365`, checks
`row.line_width.is_double_width()` and halves the DECLRMM-margin-derived
span, clamped to a minimum of 1 column); (2) SU/SD (`CSI Ps S` / `CSI Ps T`)
and margin-triggered IND/RI/LF/NEL auto-scroll are now confined to DECSLRM
left/right margins when DECLRMM is active, for both the primary and
alternate buffers (`scroll_region_up_n` / `scroll_region_down_n`,
`scroll.rs:334-379`; `scroll_region_up_primary` / `scroll_region_down_primary`,
`scroll.rs:272-300`; and the alternate-buffer arms of `handle_lf` /
`handle_ri`, `lines.rs:270-282`, `351-361` — all now branch on
`declrmm_enabled` and call `scroll_slice_up_columns` /
`scroll_slice_down_columns`, mirroring IL/DL). The third gap from that pass —
OSC 9 implementing only the iTerm2/WezTerm simple-body variant, with the
ConEmu progress-report sub-protocol (`OSC 9;4`) misparsed as literal
notification text (`freminal-terminal-emulator/src/ansi_components/osc_notify.rs:45-77`)
— remained open until issue #502 corrected the dispatch. Visual progress
support remains listed below under issue #507. Earlier: 2026-07-08 —
Documentation drift-reconciliation pass (no code changes) added the three
gaps above and corrected the DECDHL entry: the previous "top-half-only"
characterization was wrong — both top and bottom halves render correctly
(verified against `RowGlyphParams::new`,
`freminal/src/gui/renderer/vertex.rs:963-982`, and test
`row_glyph_params_double_height_bottom_shifts_origin`, `vertex.rs:2489-2496`)
— this is **not** a gap and is not listed below. SGR reverse video,
kitty keyboard, bracketed paste, and mouse tracking gap entries were
spot-checked against code and found already accurate — no change.
(DECSCNM was subsequently closed as a gap by Task 115 — see above.) Earlier:
2026-07-06 — Task 114's lock-state half was **reverted**. The
keypad operators/directional keys, media keys, and print/pause/menu-as-keys
are delivered via a raw-winit intercept (`App::on_raw_key_event` in
`freminal-windowing`) and encoded through the existing KKP `CSI u` path — this
is kept and correct on every platform. But `caps_lock`/`num_lock` decoration
bits (64/128) and the CapsLock/NumLock/ScrollLock **transition events** cannot
be produced correctly or uniformly across platforms (Wayland compositors
consume the lock keys so winit delivers no `KeyboardInput`; Windows/macOS offer
only level queries at focus-gain, never the transition), so the `evdev` /
`GetKeyState` / `CGEventSourceFlagsState` machinery was removed rather than
half-shipped. Those are now tracked as gaps against upstream (egui#3653,
egui#2041, winit#1426; alacritty#7937 documents the same limitation). Earlier:
2026-07-05 — Task 101 (v0.11.0), kitty keyboard encoding-only compliance: super
modifier, F13–F35, modifier-keys-as-keys (flag 8), and F3 → `CSI 13 ~` are
implemented. Earlier:
2026-07-02 — Kitty graphics render-path fixes (Tasks
100.11–100.20, v0.11.0) closed the sub-cell `X`/`Y` offset and the
native-vs-explicit display-sizing / per-placement-identity render gaps, plus
animation/compose repaint, image persistence, and `C=1` on `a=T`. These were
tracked in `KITTY_PROTOCOL_REFERENCE.md`'s current-state notes, not as itemized
rows here, so no GAPS entry is removed — "DCS / Graphics Gaps: None" remains
accurate. Earlier: 2026-07-01 — Task 100 (kitty graphics protocol completion,
v0.11.0) closed animation, relative placements, storage quotas, `t=s`/`o=z`
transmission, delete-target correctness, and z-index ordering. These were
tracked as open items in `KITTY_PROTOCOL_REFERENCE.md`'s 100.1 audit, not as
itemized rows in this GAPS file, so no GAPS entry is removed — the
"DCS / Graphics Gaps: None" claim below is now accurate. OSC 99 (kitty
desktop notifications) implemented directly (Task 99, v0.11.0); it was never
a tracked gap, so no GAPS entry is removed for that either.
(Tasks 20, 22, 23, 35, 41, 47, 48, 49, 52, 72, 76, 99, 100, 101, 114, 115, 117)

This document lists escape sequences and features that are **not yet fully implemented** in
Freminal. Items resolved during v0.3.0–v0.7.0 have been removed; this document reflects only
the genuine remaining work.

For the full coverage picture see [ESCAPE_SEQUENCE_COVERAGE.md](./ESCAPE_SEQUENCE_COVERAGE.md).
For durable architectural rationale on completed work, see [DESIGN_DECISIONS.md](./DESIGN_DECISIONS.md).

---

## Summary

All critical bugs have been fixed. All commonly-used DEC private modes (DECCKM, DECANM/VT52,
DECNKM, DECBKM, DECLRMM, bracketed paste, mouse tracking, focus events, DECOM, DECSCNM,
DECCOLM, DECARM, ReverseWrapAround, synchronized output, alternate-scroll, adaptive theme)
are parsed and wired. DECDWL/DECDHL are rendered correctly (both DECDHL top
and bottom halves render — a genuine VT100 split), and the auto-wrap column
is now halved on double-width/height rows (Task 117, v0.11.1). Bell is
visual + audible.
Blinking text renders. IRM is implemented. DCS sub-commands (DECRQSS, XTGETTCAP) and the
APC parser (dispatching `_G…` to Kitty graphics) are implemented. Sixel and iTerm2 inline
images (OSC 1337) are fully implemented (Task 13). The Kitty graphics protocol is
implemented (Tasks 13, 100) but **not conformant**: the 2026-10-08 audit found about
30 deviations, tracked by Tasks 135 and 136 (v0.13.1). The Kitty keyboard protocol
(Task 35, the Task 101 encoding-only wins, and Task 114's raw-winit delivery of
keypad/media/print/pause/menu keys) has a sound encoder, but the GUI layer feeding it
drops Super on text keys, Alt on text keys while a kitty flag is active (legacy Alt+text sends ESC since Task 147.24) and modified Enter; that is Task 137 (v0.13.1). The
lock-key half of Task 114 was reverted (see below). OSC 99 is conformant with kitty 0.49.2
(Task 138, v0.13.0, including 138.C4 and 138.C5), and so is OSC 22 pointer shapes
(Task 139, v0.13.0). Underlines and SGR parity (Task 140, v0.13.0) are closed. The
remaining gaps are:

- **Kitty protocol compliance (v0.13.x):** see the 2026-10-08 audit in
  `PLAN_VERSION_130.md` and the "Kitty Protocol Gaps" tables below. Graphics (Tasks 135,
  136), keyboard (Task 137), misc extensions (Task 141), unscroll (Task 142), DECCARA/DECSACE
  (Task 143), colour control (Task 144), multiple cursors (Task 103), text sizing
  (Task 104), file transfer (Task 102), clipboard (Task 146), drag and drop (Task 105)
- **OSC gaps:** OSC 66 (kitty text sizing; a vestigial no-op, Task 104)
- **Keyboard gaps:** `caps_lock`/`num_lock` decoration bits + CapsLock/NumLock/ScrollLock
  transition events (reverted — not producible uniformly across platforms),
  ISO_Level3/5_Shift (no winit `KeyCode` variant), and hyper/meta modifier bits
  (no platform source) — all tracked upstream, unscheduled
- **macOS Option as Alt:** Option keeps composing characters on macOS; `?1036`
  prefixes ESC for Alt on the other platforms (Task 152, v0.13.1)
- **Charset gaps:** only the 96-character designators and GR application to bytes
  `0x80` and above, both not planned (G0 to G3, SO/SI and single shifts work since
  Task 147.20)
- **Rare/low-priority, not planned:** KAM, SRM, `?1001`, `?1034` and `?1035`
  (recognised, not implemented), the HP escapes `ESC F` / `ESC l` / `ESC m`,
  XTSMTITLE / XTRMTITLE, DECSCA/selective-erase (no per-cell protected bit) and
  SGR 8 conceal (parsed, ignored)
- **UI work:** OSC 133 command-block gutter rendering (v0.9.0 Task 73; markers,
  storage, navigation, fold/copy/hover/duration all complete under Task 72)

Legend:

- **Importance:** `high` | `medium` | `low` / optional | `not planned`
- **Type:** `partial` (mode tracked, no renderer effect) | `not implemented`
- **Planned:** Version / task that will close the gap, or `—` if unscheduled

---

## Renderer Gaps

None currently tracked. DECSCNM (?5) cell-level fg/bg swap was the last entry
in this section; it was closed by Task 115 (v0.11.1), which performs the
swap per-pane, per-cell at render time in the vertex builders and removed
the prior panel-fill-only white inversion.

---

## OSC Gaps

| Sequence          | Importance | Type            | Planned          | Notes                                                                                                                                                                                                                                     |
| ----------------- | ---------- | --------------- | ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| OSC 66            | low        | not implemented | v0.13.3 Task 104 | Kitty text sizing — not implemented; the handler is a vestigial no-op that warn-logs/silently consumes the payload. Not a Contour code (Contour uses `CSI ? 996 n` / `?2031`). DECRPM ?2031 is the adaptive-theme query path we implement |
| OSC 21            | medium     | not implemented | v0.13.2 Task 144 | Kitty color control — missing; payloads are warn-logged by the unknown-OSC path. Also owns `@alpha` (the colour-spec parser does not accept it), `cursor_text` and `visual_bell`, whose only setter is OSC 21                             |
| OSC 30001 / 30101 | low        | not implemented | v0.13.2 Task 144 | Kitty colour stack push / pop — missing (with `CSI # P` / `# Q` / `# R`; see CSI Gaps)                                                                                                                                                    |
| OSC 5113          | medium     | not implemented | v0.13.3 Task 102 | Kitty file transfer — missing; chunks are warn-logged                                                                                                                                                                                     |
| OSC 5522          | medium     | not implemented | v0.13.3 Task 146 | Kitty clipboard — missing; payloads are warn-logged. OSC 52 selection parameter, clear and binary data are also Task 146                                                                                                                  |
| OSC 72            | low        | not implemented | v0.13.4 Task 105 | Kitty drag and drop — missing; spec stable upstream, blocked locally by winit 0.30's DnD API                                                                                                                                              |
| OSC 133 UI        | medium     | partial         | v0.9.0 Task 73   | Markers A/B/C/D parsed and stored; fold/copy/hover/duration overlays shipped under Task 72; gutter rendering remains under Task 73                                                                                                        |

---

## Buffer Semantics Gaps

DECDWL/DECDHL rendering, the auto-wrap column on double-width/height rows, and
DECSLRM margin confinement (ECH/ICH/DCH/IL/DL, SU/SD, and margin-triggered
IND/RI/LF/NEL) are all implemented and correct (see
`ESCAPE_SEQUENCE_COVERAGE.md`). Cursor save/restore (DECSC/DECRC, SCOSC/SCORC
and `?1048`/`?1049`, which share one slot per screen) has no open divergence
from xterm tracked here: Task 147 closed the pending-wrap flag (147.6) and the
saved cursor on resize (147.18), and decided that DECRC with DECOM set clamps
to the screen only. That matches kitty, Ghostty and WezTerm; xterm alone also
clamps to the bottom and right margins (not the top or left). The one thing
the slot does not save is DECSCA protection, which is unsupported (below).

One gap, unrelated to cursor save/restore, surfaced during the DECSTR
(Soft Terminal Reset, issue #507) audit:

| Feature                                    | Importance | Type            | Planned | Notes                                                                                                                                                                                                                                                                                                       |
| ------------------------------------------ | ---------- | --------------- | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| DECSCA / selective erase (protected cells) | low        | not implemented | —       | No per-cell protected-character bit exists in the buffer's cell model, so DECSCA (`CSI Ps " q`) is recognised and ignored (Task 128; before, it was misrouted to DECSCUSR and changed the cursor style); DECSED (`CSI ? Ps J`) and DECSEL (`CSI ? Ps K`) selective erase are unimplemented as a consequence |

---

## Keyboard Gaps

The kitty keyboard protocol is **not yet 1:1 compliant**. The 2026-10-08 audit found
the encoder machinery sound (Task 35, the Task 101 encoding-only wins, and Task 114's
raw-winit delivery of keypad operators/directional/KP_Begin, media keys, and
PrintScreen/Pause/Menu) but the GUI layer feeding it is not: Super is never applied to
text keys, Alt is applied to them only on the legacy path (Task 147.24), modified Enter is dropped, Enter/Tab/Backspace/Escape carry no modifiers,
key identity comes from typed text, the flag-4 shifted-key rule is violated, and F13–F35
are unreachable. That work is **Task 137 (v0.13.1)**, which also needs architecture
sign-off. Separately, the **lock-state half of Task 114 was reverted** because it cannot
be produced correctly or uniformly:

- **`caps_lock`/`num_lock` decoration + CapsLock/NumLock/ScrollLock transition
  events** — the spec asks the terminal to (a) decorate key reports with lock
  state and (b) emit lock-key press/release events. Neither is achievable
  uniformly: on **Wayland** the compositor consumes the lock keys and sends only
  `wl_keyboard.modifiers` (winit delivers no `KeyboardInput`), so neither the
  state nor the transition is observable; on **Windows/macOS** the OS query is a
  level (current on/off) sampled only at focus-gain, so decoration is stale
  mid-focus and the transition is never observable. Only X11 could do both, which
  would make one platform behave fundamentally differently. Reverted rather than
  half-shipped; tracked upstream (egui#3653, egui#2041, winit#1426; alacritty#7937
  is the same limitation in another kitty-protocol terminal).
- **ISO_Level3/5_Shift** is blocked on **winit**: winit 0.30.13's `KeyCode` enum
  has no variant for these keys (the closest concept is the logical
  `NamedKey::AltGraph`, which carries no physical-key identity to intercept).
- **hyper/meta modifier bits** have no source on any platform freminal targets.

| Feature                                 | Importance | Type            | Planned          | Notes                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| --------------------------------------- | ---------- | --------------- | ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| caps_lock / num_lock modifier state     | medium     | not implemented | —                | Bits 64 / 128 — no uniform cross-platform source (Wayland compositor-consumed; Win/macOS level-only query). Reverted; tracked upstream (egui#3653, winit#1426)                                                                                                                                                                                                                                                                           |
| CapsLock / NumLock / ScrollLock as keys | low        | not implemented | —                | `CSI 57358 u` / `57359 u` / `57360 u` — transition not observable off X11 (Wayland consumes; Win/macOS give a level, not an edge). Declined; tracked upstream                                                                                                                                                                                                                                                                            |
| ISO_Level3/5_Shift                      | low        | not implemented | —                | `CSI 57453 u` / `57454 u` — no winit `KeyCode` variant (winit 0.30.13; closest is the logical `NamedKey::AltGraph`); blocked on upstream winit, unscheduled                                                                                                                                                                                                                                                                              |
| hyper / meta modifier bits              | low        | not implemented | —                | Modifier bits 16 / 32 — no platform source on any target; `KeyModifiers` fields exist but stay `0`                                                                                                                                                                                                                                                                                                                                       |
| Kitty keyboard conformance (GUI layer)  | high       | partial         | v0.13.1 Task 137 | Super never applied to text keys; Alt applied only on the legacy path (Task 147.24), not under a kitty flag; modified Enter dropped; Enter/Tab/BS/Esc carry no modifiers; key identity from typed text; flag-4 shifted-key rule violated; F13–F35 unreachable                                                                                                                                                                            |
| macOS Option as Alt                     | medium     | not implemented | v0.13.1 Task 152 | Option keeps composing characters on macOS (Option+b types the composed character), so `?1036` prefixes ESC for Alt on the other platforms only. A `[keyboard] macos_option_as_alt` option was built in Task 147.24, found not to work (egui supplies the Option-composed character, so `both` sent `ESC` plus that character) and removed; Task 152 carries the base key through the windowing layer and adds per-side `left` / `right` |
| Application keypad modifiers            | low        | not implemented | v0.13.1 Task 137 | Application-keypad keys (DECPAM) send the plain VT220 `SS3` form with no modifier parameter; xterm adds one (`SS3 <mod> <final>`, `input.c:1394-1396`). In VT52 mode the keypad keys should send `ESC ? <final>` (xterm `input.c:855-859`); freminal sends the ANSI forms                                                                                                                                                                |
| `modifyOtherKeys` Alt+letter            | low        | partial         | v0.13.1 Task 137 | Under `modifyOtherKeys`, xterm sends some Alt+letter keys as `CSI 27 ; mod ; code ~`; freminal always uses the ESC prefix for them                                                                                                                                                                                                                                                                                                       |

---

## Charset / G-Set Gaps

G0 to G3 take ASCII (`B`), UK (`A`) and DEC Special Graphics (`0`) through
`ESC ( ) * + C`; GL is switched by SO, SI, `ESC n` and `ESC o`, and a single shift
(`ESC N` / `ESC O`, and `0x8E` / `0x8F` under S8C1T) maps the next character
(Task 147.20). DECSC saves the whole state; RIS and DECSTR reset it. Only two
things are not done, and both are deliberate:

| Feature                           | Importance  | Type            | Planned | Notes                                                                                                                                                                                                                         |
| --------------------------------- | ----------- | --------------- | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ESC - C` / `ESC . C` / `ESC / C` | not planned | not implemented | —       | 96-character set designation (G1 / G2 / G3). No modern reference implements it: Ghostty has a TODO; kitty and WezTerm lack it                                                                                                 |
| Multi-byte designators            | not planned | not implemented | —       | `ESC ( % 5`, `ESC ( " 4`, `ESC ( & 4` and the like (DEC Supplemental, Hebrew, Cyrillic): recognised and ignored, slot unchanged. xterm implements some (`charproc.c` `CASE_SCS_PERCENT` / `_DQUOTE` / `_AMPRSND`)             |
| GR applied to `0x80` and above    | not planned | partial         | —       | `ESC ~` / `ESC }` / `ESC \|` (LS1R / LS2R / LS3R) are tracked, saved and restored, but never applied: those bytes are UTF-8, and xterm in UTF-8 mode and Ghostty do not apply GR either (`ptydata.c:94`, `Terminal.zig:1660`) |

---

## ESC Gaps

| Sequence | Importance  | Type            | Planned | Notes                                                                                                                                                                                                                                                                        |
| -------- | ----------- | --------------- | ------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| ESC F    | not planned | not implemented | —       | Cursor to lower-left (HP). Recognised and ignored with a `debug!` log. xterm acts only when `hpLowerleftBugCompat` is set, and it defaults to off (`charproc.c:448,3982-3990`); kitty, Ghostty and WezTerm ignore it (Task 147.23). VT52 `ESC F` is a separate, working path |
| ESC l    | not planned | not implemented | —       | Memory Lock (HP). Recognised and ignored with a `debug!` log. xterm sets the top margin at the cursor row (`charproc.c:4759-4770`); kitty, Ghostty and WezTerm ignore it (Task 147.23)                                                                                       |
| ESC m    | not planned | not implemented | —       | Memory Unlock (HP). Recognised and ignored with a `debug!` log. xterm clears the top margin (`charproc.c:4759-4770`); kitty, Ghostty and WezTerm ignore it (Task 147.23)                                                                                                     |

---

## CSI Gaps

Kitty and xterm extensions that are missing. Since Task 128 the CSI router matches on
prefix, intermediate and final byte together, so each sequence below is recognised and
ignored (warn-logged, no output) until its task lands; none of them misroutes any more.

| Sequence                                                                   | Importance  | Type            | Planned          | Notes                                                                                                                                                                |
| -------------------------------------------------------------------------- | ----------- | --------------- | ---------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CSI Ps + T` (unscroll)                                                    | medium      | not implemented | v0.13.2 Task 142 | Recognised and ignored since Task 128 (before, it executed as SD)                                                                                                    |
| `CSI … $ r` (DECCARA), `CSI Ps * x` (DECSACE)                              | medium      | not implemented | v0.13.2 Task 143 | Both recognised and ignored since Task 128 (before, `CSI … $ r` was routed to DECSTBM and `CSI * x` wrote an unsolicited DECREQTPARM reply)                          |
| `CSI Ps # P` / `# Q` / `# R` (XTPUSHCOLORS / XTPOPCOLORS / XTREPORTCOLORS) | low         | not implemented | v0.13.2 Task 144 | Recognised and ignored since Task 128 (before, `CSI # P` was routed to DCH and deleted characters)                                                                   |
| `CSI ? Pm s` / `CSI ? Pm r` (XTSAVE / XTRESTORE)                           | low         | not implemented | v0.13.2 Task 141 | Missing, including the bare `CSI ? s` / `CSI ? r` forms. Recognised and ignored since Task 128 (before, they were routed to DECSLRM / DECSTBM and failed as Invalid) |
| `CSI 22 J`                                                                 | low         | not implemented | v0.13.2 Task 141 | Move screen to scrollback, then ED 2; no `EraseDisplayMode` variant. Prerequisite for multiple cursors                                                               |
| `CSI < 288 ; x ; y M` (mouse-leave report)                                 | low         | not implemented | v0.13.2 Task 141 | Not sent on window leave or pane-to-pane transitions under SGR-pixel mouse encoding                                                                                  |
| `CSI > … SP q` (multiple cursors)                                          | medium      | not implemented | v0.13.2 Task 103 | Missing. Recognised and ignored since 126.3 (before, it triggered an unsolicited XTVERSION reply); Task 128 makes this a routing rule                                |
| `CSI … $ t` (DECRARA)                                                      | low         | not implemented | —                | Not implemented and not planned. Recognised and ignored since Task 128 (before, it was handled as a window operation)                                                |
| `CSI > Pm t` / `CSI > Pm T` (XTSMTITLE / XTRMTITLE)                        | not planned | not implemented | —                | Recognised and ignored without a warning, as kitty, Ghostty and WezTerm do (Task 147.9)                                                                              |
| `CSI 8 m` (SGR conceal)                                                    | low         | not implemented | —                | Parsed to `Conceal` and ignored by `apply_sgr`; text stays visible. The terminfo entry does not declare `invis` (Task 151.3)                                         |

---

## CSI Standard Mode Gaps

Both modes are typed and recognised: set and reset log at `debug!` and change
nothing, and DECRQM answers `Ps = 0` in the ANSI form (`CSI Pa ; 0 $ y`).

| Mode | Name                       | Importance  | Planned | Notes                                                                                                                                                                                                            |
| ---- | -------------------------- | ----------- | ------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 2    | KAM — Keyboard Action Mode | not planned | —       | kitty and WezTerm do not implement it. Ghostty enforces it only behind `vt-kam-allowed`, which is off by default because letting any program lock the keyboard is hostile; xterm alone enforces it (Task 147.21) |
| 12   | SRM — Send/Receive Mode    | not planned | —       | kitty and WezTerm do not implement it and Ghostty stores it with no consumer. On a modern tty the kernel echoes, so turning local echo on would only double every character (Task 147.21)                        |

LNM (mode 20) and IRM (mode 4) are implemented, and DECRQM answers every ANSI-mode
query in the ANSI form `CSI Pa ; Ps $ y`, an unknown mode included (Task 147.1,
issue #528).

---

## DEC Private Mode Gaps

`?1001`, `?1034` and `?1035` are typed and recognised: set and reset log at
`debug!` and change nothing, and DECRQM answers `Ps = 0`.

| Mode  | Name                                     | Importance  | Type            | Planned | Notes                                                                                                                                                                                                     |
| ----- | ---------------------------------------- | ----------- | --------------- | ------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| ?1001 | Hilite Mouse                             | not planned | not implemented | —       | kitty, Ghostty and WezTerm do not implement it, and WezTerm's docs warn that it can hang the terminal. The old alias to `?1000` is removed, so `?1001h` followed by a click sends no report (Task 147.21) |
| ?1034 | Meta Sets Eighth Bit                     | not planned | not implemented | —       | Ghostty, WezTerm and kitty do not implement it, and it conflicts with UTF-8 input. (This row used to describe the ESC prefix, which is `?1036`, implemented.) (Task 147.21)                               |
| ?1035 | NumLock Overrides the Application Keypad | not planned | not implemented | —       | winit exposes no NumLock state (Task 114.11), so the mode cannot be honoured. If lock state becomes available it belongs with Task 137 K14 (Task 147.21)                                                  |
| ?1039 | Alt Sends Escape                         | low         | partial         | —       | Stored and reported by DECRQM, with no further effect, as in Ghostty (Task 147.24)                                                                                                                        |

Fully implemented and removed from prior gap lists during v0.3.0–v0.7.0:
`?2 (DECANM/VT52)`, `?66 (DECNKM)`, `?67 (DECBKM)`, `?69 (DECLRMM)`, `?1007 (AlternateScroll)`,
`?2031 (Adaptive Theme)`.

---

## DCS / Graphics Gaps

The table below lists the remaining DCS and graphics gaps. Sixel (DCS) and iTerm2
inline images (OSC 1337 `File=` / `MultipartFile=`) are otherwise fully implemented. The Kitty graphics
protocol (APC `_G`, Tasks 13, 100) is implemented but **not conformant**: the
2026-10-08 audit found about 30 deviations, five of them high-severity (replies to
id-less commands, `t=t` deleting arbitrary files — closed by 126.2 —, text and erase
destroying image tiles, an image placement pre-clearing others below it, no
negative-z layering, and the wrong cursor-after-placement rule). They are tracked
by Task 135 (conformance) and Task 136 (placement model and z-layers), both
v0.13.1.

| Behaviour                                   | Importance | Type            | Planned          | Notes                                                                                                                                                                                                                                                                                                                                                                         |
| ------------------------------------------- | ---------- | --------------- | ---------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Kitty graphics conformance                  | high       | partial         | v0.13.1 Task 135 | About 30 deviations: replies to id-less commands, text and erase destroy image tiles, `S=`/`O=` ignored for files, `CSI 14 t` reports the OS window rectangle for every pane                                                                                                                                                                                                  |
| Kitty graphics placement model and z-layers | high       | partial         | v0.13.1 Task 136 | Placing an image pre-clears others below; no negative-z layering; cursor-after-placement rule wrong                                                                                                                                                                                                                                                                           |
| Kitty kitten transport (`DCS @kitty-…`)     | low        | not implemented | —                | Not implemented and not planned: private kitten plumbing (`@kitty-print`, `echo`, `ssh`, `ask`, `clone`, `edit`) and the `@kitty-cmd` remote-control API are out of scope (`PLAN_VERSION_130.md` Out of scope). Consumed silently since Task 129.14: no warn, and a dedicated debug line with the length only (the generic, bounded `DCS received` debug line still applies). |

Task 100 added the Kitty graphics feature surface — animation,
image-number references, relative placements, storage quotas + eviction,
shared memory (`t=s`, POSIX and Windows), zlib (`o=z`), source-rect crop,
delete-target correctness, and z-index render ordering — but did not make it
conformant (Tasks 135, 136; see above). The APC parser dispatches `_G…` to the
Kitty handler; non-Kitty APCs are logged and ignored, which is spec-compliant.

---

## 8-bit C1 Control Gap

8-bit C1 controls (0x80–0x9F), in particular 0x9B as a one-byte CSI introducer, are supported
**only when S8C1T mode is active** (`ESC SP G`). The default is 7-bit (S7C1T). Modern terminal
output universally uses 7-bit sequences, so the default is appropriate. The remaining gap is
that S8C1T is off by default; there is no user-facing config to change this. Kitty graphics replies honour S8C1T (8-bit APC / ST framing when active) since Task 129.13; since Task 130 so do DECRPM, `CSI ? u` and every GUI-originated reply (window reports, title/icon reports, OSC 52, OSC 99). VT52 `ESC / Z` stays 7-bit by definition. Every CSI or SS3 introducer the terminal generates (key reports, focus, mouse, bracketed-paste markers, kitty `CSI u`, `modifyOtherKeys`) honours S8C1T since Task 147.4, as in xterm. VT52 mode and Alt-prefixed text stay 7-bit.

---

## C0 Mid-Sequence Handling

**Resolved.** Freminal's parser correctly executes C0 controls (BS, CR, LF, VT, FF) inline
during CSI sequence parsing, per ECMA-48. This is verified by unit tests. This is no longer a gap.

---

## Roadmap by Priority

### Priority 1 — Renderer integration

| Item                     | Rationale                                                                  | Planned        |
| ------------------------ | -------------------------------------------------------------------------- | -------------- |
| OSC 133 command-block UI | Storage + navigation done under Task 72; only the gutter rendering remains | v0.9.0 Task 73 |

### Priority 2 — Polish

| Item                | Rationale                                              | Planned          |
| ------------------- | ------------------------------------------------------ | ---------------- |
| macOS Option as Alt | Option keeps composing characters; see "Keyboard Gaps" | v0.13.1 Task 152 |

### Priority 3 — Low priority / optional

| Item                                                  | Rationale                                                                                       | Planned     |
| ----------------------------------------------------- | ----------------------------------------------------------------------------------------------- | ----------- |
| KAM, SRM, `?1001`, `?1034`, `?1035`                   | Recognised, not implemented; reasons under "CSI Standard Mode Gaps" and "DEC Private Mode Gaps" | Not planned |
| HP `ESC F` / `ESC l` / `ESC m`, XTSMTITLE / XTRMTITLE | Recognised and ignored, as kitty, Ghostty and WezTerm do                                        | Not planned |
| 96-character designators, GR on `0x80` and above      | No modern reference implements them; those bytes are UTF-8                                      | Not planned |
| DECSCA / selective erase                              | No per-cell protected-character bit in buffer model                                             | —           |
| 8-bit C1 default on                                   | Modern terminals always use 7-bit sequences                                                     | —           |

---

© 2025 Freminal Project — MIT License.
