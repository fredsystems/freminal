# Escape Sequence Gaps

Last updated: 2026-07-08 — Task 115 (v0.11.1) closed the DECSCNM
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
ConEmu progress-report sub-protocol (`OSC 9;1`–`9;4`) misparsed as literal
notification text (`freminal-terminal-emulator/src/ansi_components/osc_notify.rs:45-77`)
— remains open and is still listed below. Earlier: 2026-07-08 —
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
images (OSC 1337) are fully implemented (Task 13). Kitty graphics is fully implemented
(Tasks 13, 100). Kitty keyboard protocol is substantially compliant: Task 35, the
Task 101 encoding-only wins (super modifier, F13–F35, modifier-keys-as-keys under flag 8,
F3 → `CSI 13 ~`), and Task 114's raw-winit delivery of keypad/media/print/pause/menu keys.
The lock-key half of Task 114 was reverted (see below). The remaining gaps are:

- **OSC gaps:** OSC 66 (recognized but no effect); OSC 9 ConEmu progress-report
  sub-protocol (`9;1`–`9;4`, misparsed as literal notification text)
- **Keyboard gaps:** `caps_lock`/`num_lock` decoration bits + CapsLock/NumLock/ScrollLock
  transition events (reverted — not producible uniformly across platforms),
  ISO_Level3/5_Shift (no winit `KeyCode` variant), and hyper/meta modifier bits
  (no platform source) — all tracked upstream, unscheduled
- **Charset gaps:** SO/SI (G1 rendering), G2/G3 switching
- **Rare/low-priority:** SRM standard mode, ?1034, functional ?1001 hilite tracking
- **UI work:** OSC 133 command-block gutter rendering (v0.9.0 Task 73; markers,
  storage, navigation, fold/copy/hover/duration all complete under Task 72)

Legend:

- **Importance:** 🟩 High | 🟨 Medium | ⬜ Low / optional
- **Type:** 🚧 Partial (mode tracked, no renderer effect) | ⬜ Not implemented
- **Planned:** Version / task that will close the gap, or `—` if unscheduled

---

## Renderer Gaps

None currently tracked. DECSCNM (?5) cell-level fg/bg swap was the last entry
in this section; it was closed by Task 115 (v0.11.1), which performs the
swap per-pane, per-cell at render time in the vertex builders and removed
the prior panel-fill-only white inversion.

---

## OSC Gaps

| Sequence                            | Importance | Type | Planned        | Notes                                                                                                                                                                                                                |
| ----------------------------------- | ---------- | ---- | -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| OSC 66                              | ⬜         | ⬜   | —              | ColorScheme Notification (Contour) — recognized/silently consumed; DECRPM ?2031 is the query path we implement                                                                                                       |
| OSC 133 UI                          | 🟨         | 🚧   | v0.9.0 Task 73 | Markers A/B/C/D parsed and stored; fold/copy/hover/duration overlays shipped under Task 72; gutter rendering remains under Task 73                                                                                   |
| OSC 9 ConEmu (`9;1`–`9;4`) progress | ⬜         | ⬜   | —              | Only the iTerm2/WezTerm simple-body variant is recognized (`handle_osc_notify_9`, `osc_notify.rs:45-77`); ConEmu-style progress-state sequences are misparsed as literal notification body text, not ignored/handled |

---

## Buffer Semantics Gaps

No known buffer-semantics gaps. DECDWL/DECDHL rendering, the auto-wrap
column on double-width/height rows, and DECSLRM margin confinement
(ECH/ICH/DCH/IL/DL, SU/SD, and margin-triggered IND/RI/LF/NEL) are all
implemented and correct (see `ESCAPE_SEQUENCE_COVERAGE.md`). The last two
narrower gaps in this category — the double-width/height auto-wrap column
and SU/SD/margin-triggered IND/RI DECSLRM confinement — were closed by
Task 117 (v0.11.1).

---

## Keyboard Gaps

The kitty keyboard protocol is substantially compliant (Task 35, the Task 101
encoding-only wins, and Task 114's raw-winit delivery of keypad operators/
directional/KP_Begin, media keys, and PrintScreen/Pause/Menu — all correct on
every platform). The **lock-state half of Task 114 was reverted** because it
cannot be produced correctly or uniformly:

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

| Feature                                 | Importance | Type | Planned | Notes                                                                                                                                                          |
| --------------------------------------- | ---------- | ---- | ------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| caps_lock / num_lock modifier state     | 🟨         | ⬜   | —       | Bits 64 / 128 — no uniform cross-platform source (Wayland compositor-consumed; Win/macOS level-only query). Reverted; tracked upstream (egui#3653, winit#1426) |
| CapsLock / NumLock / ScrollLock as keys | ⬜         | ⬜   | —       | `CSI 57358 u` / `57359 u` / `57360 u` — transition not observable off X11 (Wayland consumes; Win/macOS give a level, not an edge). Declined; tracked upstream  |
| ISO_Level3/5_Shift                      | ⬜         | ⬜   | —       | `CSI 57453 u` / `57454 u` — no winit `KeyCode` variant (winit 0.30.13; closest is the logical `NamedKey::AltGraph`); blocked on upstream winit, unscheduled    |
| hyper / meta modifier bits              | ⬜         | ⬜   | —       | Modifier bits 16 / 32 — no platform source on any target; `KeyModifiers` fields exist but stay `0`                                                             |

---

## Charset / G-Set Gaps

| Feature               | Importance | Type | Planned | Notes                                                                                      |
| --------------------- | ---------- | ---- | ------- | ------------------------------------------------------------------------------------------ |
| SO (0x0E) — Shift Out | ⬜         | 🚧   | —       | Parsed; selects G1 into GL, but G1 rendering is not implemented                            |
| SI (0x0F) — Shift In  | ⬜         | 🚧   | —       | Parsed; restores G0 into GL — no effect since G1 rendering is absent                       |
| ESC n (LS2)           | ⬜         | 🚧   | —       | Invoke G2 as GL — parsed, no functional effect                                             |
| ESC o (LS3)           | ⬜         | ⬜   | —       | Invoke G3 as GL — parsed, no functional effect                                             |
| ESC \                 | / \} / \~  | ⬜   | 🚧      | —                                                                                          |
| ESC ) C / ESC \* C    | ❌         | ⬜   | —       | G1/G2 arbitrary charset designation — not planned (ESC ) B / G1=ASCII works since Task 22) |

G0 with DEC Special Graphics (`ESC ( 0`) and US ASCII (`ESC ( B`) both work correctly, and
`ESC ) B` (G1=ASCII) was fixed in Task 22; these are the overwhelmingly common cases.

---

## ESC Gaps

| Sequence | Importance | Type | Planned | Notes                                                             |
| -------- | ---------- | ---- | ------- | ----------------------------------------------------------------- |
| ESC F    | ⬜         | 🚧   | —       | Cursor to lower-left of screen — parsed with debug log, no effect |
| ESC l    | ⬜         | 🚧   | —       | Memory Lock — parsed with debug log, no effect                    |
| ESC m    | ⬜         | 🚧   | —       | Memory Unlock — parsed with debug log, no effect                  |

---

## CSI Standard Mode Gaps

| Mode | Name                    | Importance | Planned | Notes                             |
| ---- | ----------------------- | ---------- | ------- | --------------------------------- |
| 12   | SRM — Send/Receive Mode | ⬜         | —       | Not implemented; rare in practice |

LNM (mode 20) and IRM (mode 4) are implemented.

---

## DEC Private Mode Gaps

| Mode  | Name           | Importance | Type | Planned | Notes                                                                |
| ----- | -------------- | ---------- | ---- | ------- | -------------------------------------------------------------------- |
| ?1001 | Hilite Mouse   | ⬜         | 🚧   | —       | Mode parsed/stored; obsolete hilite tracking not functionally active |
| ?1034 | Interpret Meta | ⬜         | ⬜   | —       | Meta key sends ESC prefix — not recognized                           |

Fully implemented and removed from prior gap lists during v0.3.0–v0.7.0:
`?2 (DECANM/VT52)`, `?66 (DECNKM)`, `?67 (DECBKM)`, `?69 (DECLRMM)`, `?1007 (AlternateScroll)`,
`?2031 (Adaptive Theme)`.

---

## DCS / Graphics Gaps

None. Sixel (DCS), the Kitty graphics protocol (APC `_G`, Tasks 13, 100), and
iTerm2 inline images (OSC 1337 `File=` / `MultipartFile=`) are all fully
implemented. Task 100 completed the Kitty graphics surface — animation,
image-number references, relative placements, storage quotas + eviction,
shared memory (`t=s`, POSIX and Windows), zlib (`o=z`), source-rect crop,
delete-target correctness, and z-index render ordering. The APC parser
dispatches `_G…` to the Kitty handler; non-Kitty APCs are logged and ignored,
which is spec-compliant.

---

## 8-bit C1 Control Gap

8-bit C1 controls (0x80–0x9F), in particular 0x9B as a one-byte CSI introducer, are supported
**only when S8C1T mode is active** (`ESC SP G`). The default is 7-bit (S7C1T). Modern terminal
output universally uses 7-bit sequences, so the default is appropriate. The remaining gap is
that S8C1T is off by default; there is no user-facing config to change this.

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

| Item                               | Rationale                                                                                                                                                                    | Planned |
| ---------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------- |
| XTGETTCAP capability expansion     | Common queries we currently decline: `indn` (indent N), `query-os-name` (Kitty extension). Both protocol-correct with `0+r<hex>`; recognising them is a cosmetic improvement | —       |
| OSC 9 ConEmu progress sub-protocol | Affects Windows Terminal/ConEmu-targeting scripts (e.g. progress bars); freminal shows a spurious notification instead of ignoring the sequence                              | —       |

### Priority 3 — Low priority / optional

| Item                     | Rationale                                           | Planned |
| ------------------------ | --------------------------------------------------- | ------- |
| SO/SI + G1 rendering     | Almost never used in practice since UTF-8 took over | —       |
| SRM standard mode        | Extremely rare in modern terminal output            | —       |
| ?1001 hilite tracking    | Obsolete mouse mode                                 | —       |
| ?1034 interpret-meta key | Niche compatibility                                 | —       |
| 8-bit C1 default on      | Modern terminals always use 7-bit sequences         | —       |

---

© 2025 Freminal Project — MIT License.
