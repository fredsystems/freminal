# Escape Sequence Gaps

Last updated: 2026-04-21 — Audited against source; removed OSC 12 and ESC Z (both fully implemented)
(Tasks 20, 22, 23, 35, 41, 47, 48, 49, 52)

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
APC parser (dispatching `_G…` to Kitty graphics) are implemented. Sixel, Kitty graphics,
and iTerm2 inline images (OSC 1337) are fully implemented (Task 13). Kitty keyboard
protocol is complete (Task 35). The remaining gaps are:

- **Renderer gaps:** DECSCNM cell-level fg/bg swap (panel-fill swap exists)
- **OSC gaps:** OSC 66 (recognized but no effect), OSC 777 (Konsole notification)
- **Charset gaps:** SO/SI (G1 rendering), G2/G3 switching
- **Rare/low-priority:** SRM standard mode, ?1034, functional ?1001 hilite tracking
- **UI work:** OSC 133 command-block navigation (markers are parsed; UI is Task 72)

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

| Sequence   | Importance | Type | Planned        | Notes                                                                                                          |
| ---------- | ---------- | ---- | -------------- | -------------------------------------------------------------------------------------------------------------- |
| OSC 66     | ⬜         | ⬜   | —              | ColorScheme Notification (Contour) — recognized/silently consumed; DECRPM ?2031 is the query path we implement |
| OSC 777    | ⬜         | ⬜   | v0.9.0 Task 76 | Konsole system notification — scheduled under Notification System task                                         |
| OSC 133 UI | 🟨         | 🚧   | v0.9.0 Task 72 | Markers A/B/C/D all parsed and stored; gutter/jump-to-prompt UI is the outstanding work                        |

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

None. Sixel (DCS), Kitty graphics protocol (APC `_G`), and iTerm2 inline images
(OSC 1337 `File=` / `MultipartFile=`) are all fully implemented. The APC parser
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

| Item                          | Rationale                                                               | Planned        |
| ----------------------------- | ----------------------------------------------------------------------- | -------------- |
| DECSCNM cell-level fg/bg swap | Panel-fill inversion lands today; true per-cell inversion still missing | —              |
| OSC 133 command-block UI      | Markers parsed; gutter + jump-to-prompt is the outstanding work         | v0.9.0 Task 72 |

### Priority 2 — Polish

| Item    | Rationale                   | Planned        |
| ------- | --------------------------- | -------------- |
| OSC 777 | Konsole notification compat | v0.9.0 Task 76 |

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
