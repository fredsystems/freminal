# Escape Sequence Gaps

Last updated: 2026-07-05 — Task 114 (v0.11.0) implemented the egui-blocked
keyboard remainder: keypad operators/directional keys, media keys, and
lock/print/pause/menu-as-keys are now delivered via a raw-winit intercept
(`App::on_raw_key_event` in `freminal-windowing`) and encoded through the
existing KKP `CSI u` path (114.5–114.7); true `caps_lock`/`num_lock` modifier
bits (64/128) are now sourced from an OS lock-state query — `evdev`/EVIOCGLED
on Linux (one code path for X11 and Wayland), `GetKeyState` on Windows,
`CGEventSourceFlagsState` on macOS (caps only; num/scroll hardcoded `false` —
the concept doesn't exist on Mac keyboards; the macOS Input-Monitoring/TCC
permission question is unverified on-device) — queried at cold-start and
focus-gain, plus toggled on observed CapsLock/NumLock presses while focused
(114.4/114.7). The **Keyboard Gaps** section below is now much shorter: only
ISO_Level3/5_Shift (no winit `KeyCode` variant — a winit limitation, not
egui) and the `hyper`/`meta` modifier bits (no platform source on any target)
remain tracked, both unscheduled. Same day — Task 101 (v0.11.0), kitty
keyboard encoding-only compliance: super modifier, F13–F35,
modifier-keys-as-keys (flag 8), and F3 → `CSI 13 ~` are implemented. Earlier:
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
(Tasks 20, 22, 23, 35, 41, 47, 48, 49, 52, 72, 76, 99, 100, 101, 114)

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
are parsed and wired. DECDWL/DECDHL are rendered. Bell is visual + audible.
Blinking text renders. IRM is implemented. DCS sub-commands (DECRQSS, XTGETTCAP) and the
APC parser (dispatching `_G…` to Kitty graphics) are implemented. Sixel and iTerm2 inline
images (OSC 1337) are fully implemented (Task 13). Kitty graphics is fully implemented
(Tasks 13, 100). Kitty keyboard protocol is now effectively complete: Task 35, the
Task 101 encoding-only wins (super modifier, F13–F35, modifier-keys-as-keys under flag 8,
F3 → `CSI 13 ~`), and Task 114 (raw-winit delivery of keypad/media/lock/print/pause/menu
keys, plus true caps_lock/num_lock modifier bits via OS lock-state query). Only two
permanent, unscheduled keyboard gaps remain (ISO_Level3/5_Shift, hyper/meta bits). The
remaining gaps are:

- **Renderer gaps:** DECSCNM cell-level fg/bg swap (panel-fill swap exists)
- **OSC gaps:** OSC 66 (recognized but no effect)
- **Keyboard gaps:** ISO_Level3/5_Shift (no winit `KeyCode` variant) and
  hyper/meta modifier bits (no platform source) — both permanent, unscheduled
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

These features are tracked at the state-machine level but the renderer does not yet fully act on them.

| Feature                      | Importance | Type | Planned | Notes                                                                                                         |
| ---------------------------- | ---------- | ---- | ------- | ------------------------------------------------------------------------------------------------------------- |
| DECSCNM — Reverse Video (?5) | 🟨         | 🚧   | —       | `TerminalModes.invert_screen` set; renderer swaps panel-fill background but per-cell fg/bg inversion not done |

---

## OSC Gaps

| Sequence   | Importance | Type | Planned        | Notes                                                                                                                              |
| ---------- | ---------- | ---- | -------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| OSC 66     | ⬜         | ⬜   | —              | ColorScheme Notification (Contour) — recognized/silently consumed; DECRPM ?2031 is the query path we implement                     |
| OSC 133 UI | 🟨         | 🚧   | v0.9.0 Task 73 | Markers A/B/C/D parsed and stored; fold/copy/hover/duration overlays shipped under Task 72; gutter rendering remains under Task 73 |

---

## Keyboard Gaps

The kitty keyboard protocol is now effectively complete (Task 35, the Task 101
encoding-only wins, and Task 114's raw-winit key-delivery intercept + OS
lock-state query). Task 114 delivered the previously egui-blocked keys —
keypad operators/directional/KP_Begin, media keys, and lock/print/pause/menu
keys (via `App::on_raw_key_event`) — and the true `caps_lock`/`num_lock`
modifier bits (via an OS lock-state query: `evdev`/EVIOCGLED on Linux,
`GetKeyState` on Windows, `CGEventSourceFlagsState` on macOS caps-only). Two
gaps remain, neither one egui-blocked any longer:

- **ISO_Level3/5_Shift** is blocked on **winit**, not egui: winit 0.30.13's
  `KeyCode` enum has no variant for these keys at all (the closest concept is
  the logical `NamedKey::AltGraph`, which carries no physical-key identity to
  intercept).
- **hyper/meta modifier bits** have no source on any platform freminal
  targets — this is a permanent, unscheduled gap, not an egui or winit
  limitation.

| Feature                    | Importance | Type | Planned | Notes                                                                                                                                                       |
| -------------------------- | ---------- | ---- | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| ISO_Level3/5_Shift         | ⬜         | ⬜   | —       | `CSI 57453 u` / `57454 u` — no winit `KeyCode` variant (winit 0.30.13; closest is the logical `NamedKey::AltGraph`); blocked on upstream winit, unscheduled |
| hyper / meta modifier bits | ⬜         | ⬜   | —       | Modifier bits 16 / 32 — no platform source on any target; `KeyModifiers` fields exist but stay `0`                                                          |

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

| Item                          | Rationale                                                                  | Planned        |
| ----------------------------- | -------------------------------------------------------------------------- | -------------- |
| DECSCNM cell-level fg/bg swap | Panel-fill inversion lands today; true per-cell inversion still missing    | —              |
| OSC 133 command-block UI      | Storage + navigation done under Task 72; only the gutter rendering remains | v0.9.0 Task 73 |

### Priority 2 — Polish

| Item                           | Rationale                                                                                                                                                                    | Planned |
| ------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------- |
| XTGETTCAP capability expansion | Common queries we currently decline: `indn` (indent N), `query-os-name` (Kitty extension). Both protocol-correct with `0+r<hex>`; recognising them is a cosmetic improvement | —       |

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
