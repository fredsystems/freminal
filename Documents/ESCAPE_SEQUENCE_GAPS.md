# Escape Sequence Gaps

Last updated: 2026-03-12 — Rewritten post-Task 7 completion (all 30 subtasks done)

This document lists escape sequences and features that are **not yet fully implemented** in
Freminal. All bugs and gaps documented in the previous audit (March 2026, pre-Task 7) have been
resolved. This document reflects only the genuine remaining work.

For the full coverage picture see [ESCAPE_SEQUENCE_COVERAGE.md](./ESCAPE_SEQUENCE_COVERAGE.md).

---

## Summary

All critical bugs have been fixed. All commonly-used DEC private modes (DECCKM, bracketed paste,
mouse tracking, focus events, DECOM, DECSCNM, DECCOLM, DECARM, ReverseWrapAround, synchronized
output) are now parsed and written to `TerminalModes`. All tab stop infrastructure is in place.
DCS sub-commands (DECRQSS, XTGETTCAP) are implemented. The remaining gaps are:

- **Renderer gaps:** DECSCNM full cell-level inversion (background swap exists), double-height/width lines
- **OSC gaps:** OSC 10/11 set (no-op), OSC 12/110/111 (not implemented)
- **Charset gaps:** SO/SI (G1 rendering), G2/G3 switching
- **Rare/low-priority:** Sixel, IRM/SRM standard modes, unparsed DEC modes (?2, ?66, ?67, ?69)
- **8-bit C1 controls** (0x9B CSI) not parsed (no practical impact)

Legend:

- **Importance:** 🟩 High | 🟨 Medium | ⬜ Low / optional
- **Type:** 🚧 Partial (mode tracked, no renderer effect) | ⬜ Not implemented

---

## Renderer Gaps

These features are tracked at the state-machine level but the renderer does not yet act on them.

| Feature                         | Importance | Type | Notes                                                                                                                          |
| ------------------------------- | ---------- | ---- | ------------------------------------------------------------------------------------------------------------------------------ |
| DECSCNM — Reverse Video (?5)    | 🟨         | 🚧   | `TerminalModes.invert_screen` set; renderer swaps background fill to white but full cell-level fg/bg inversion not implemented |
| Double-height lines (ESC # 3/4) | ⬜         | ⬜   | Parsed and emits `TerminalOutput::DoubleHeightLine*`; renderer ignores                                                         |
| Double-width line (ESC # 6)     | ⬜         | ⬜   | Parsed and emits `TerminalOutput::DoubleWidthLine`; renderer ignores                                                           |
| BEL audio/visual bell (0x07)    | ⬜         | 🚧   | `TerminalOutput::Bell` emitted correctly; no audio or visual bell in GUI                                                       |

---

## OSC Gaps

| Sequence           | Importance | Type | Notes                                                                  |
| ------------------ | ---------- | ---- | ---------------------------------------------------------------------- |
| OSC 10 ; color BEL | 🟨         | 🚧   | Query responds with hardcoded Catppuccin Mocha default; set is a no-op |
| OSC 11 ; color BEL | 🟨         | 🚧   | Query responds with hardcoded Catppuccin Mocha default; set is a no-op |
| OSC 12 ; color BEL | ⬜         | ⬜   | Set cursor color — not implemented                                     |
| OSC 110 BEL        | ⬜         | ⬜   | Reset foreground color — not implemented                               |
| OSC 111 BEL        | ⬜         | ⬜   | Reset background color — not implemented                               |
| OSC 777            | ⬜         | ⬜   | Konsole system notification — not implemented                          |

**Implementation note for OSC 10/11:** The OSC 10/11 query path already works. To make the set
path functional, the configured theme foreground/background colors need to be mutable at runtime
and the renderer needs to read them from `TerminalSnapshot`. This is a config integration task,
not a parser task.

---

## Charset / G-Set Gaps

| Feature               | Importance | Type | Notes                                                                |
| --------------------- | ---------- | ---- | -------------------------------------------------------------------- |
| SO (0x0E) — Shift Out | ⬜         | 🚧   | Parsed; selects G1 into GL, but G1 rendering is not implemented      |
| SI (0x0F) — Shift In  | ⬜         | 🚧   | Parsed; restores G0 into GL — no effect since G1 rendering is absent |
| ESC n (LS2)           | ⬜         | 🚧   | Invoke G2 as GL — parsed, no functional effect                       |
| ESC o (LS3)           | ⬜         | ⬜   | Invoke G3 as GL — parsed, no functional effect                       |
| ESC \| / \} / \~      | ⬜         | 🚧   | Invoke G3/G2/G1 as GR — parsed, no functional effect                 |
| ESC ) C / ESC \* C    | ❌         | ⬜   | G1/G2 charset designation — not planned                              |

G0 with DEC Special Graphics (`ESC ( 0`) and US ASCII (`ESC ( B`) both work correctly; these are
the overwhelmingly common cases.

---

## ESC Gaps

| Sequence | Importance | Type | Notes                                                             |
| -------- | ---------- | ---- | ----------------------------------------------------------------- |
| ESC Z    | ⬜         | ⬜   | DECID — Return Terminal ID. Not parsed.                           |
| ESC F    | ⬜         | 🚧   | Cursor to lower-left of screen — parsed with debug log, no effect |
| ESC l    | ⬜         | 🚧   | Memory Lock — parsed with debug log, no effect                    |
| ESC m    | ⬜         | 🚧   | Memory Unlock — parsed with debug log, no effect                  |

---

## CSI Standard Mode Gaps

| Mode | Name                      | Importance | Notes                             |
| ---- | ------------------------- | ---------- | --------------------------------- |
| 4    | IRM — Insert/Replace Mode | ⬜         | Not implemented; rare in practice |
| 12   | SRM — Send/Receive Mode   | ⬜         | Not implemented; rare in practice |

LNM (mode 20) is implemented.

---

## DEC Private Mode Gaps (not yet parsed)

These modes are not recognized at all — they will be logged as unknown.

| Mode          | Name               | Importance | Notes                                    |
| ------------- | ------------------ | ---------- | ---------------------------------------- |
| ?2 (DECANM)   | VT52 Mode          | ⬜         | VT52 backward compat — not planned       |
| ?66 (DECNKM)  | Numeric Keypad     | ⬜         | DEC keypad mode — low priority           |
| ?67 (DECBKM)  | Backarrow Key      | ⬜         | Backspace sends DEL or BS — low priority |
| ?69 (DECLRMM) | Left/Right Margins | ⬜         | Left/right margin mode — not planned     |
| ?1001         | Hilite Mouse       | ⬜         | Obsolete mouse mode                      |
| ?1007         | Alternate Scroll   | ⬜         | Mouse wheel → scroll in alternate screen |
| ?1034         | Interpret Meta     | ⬜         | Meta key sends ESC prefix                |

---

## DCS / Graphics Gaps

| Sequence | Importance | Notes                                                      |
| -------- | ---------- | ---------------------------------------------------------- |
| Sixel    | ⬜         | Raster graphics — large undertaking, not planned near-term |
| APC      | ⬜         | Captured as opaque bytes only; no sub-command dispatch     |

---

## 8-bit C1 Control Gap

The 8-bit C1 controls (0x80–0x9F) — in particular 0x9B as a one-byte CSI introducer — are not
parsed. Freminal parses only 7-bit ESC sequences. This has no practical impact on modern terminal
output, which universally uses 7-bit sequences, but is a conformance gap.

---

## C0 Mid-Sequence Handling

**Resolved.** Freminal's parser correctly executes C0 controls (BS, CR, LF, VT, FF) inline
during CSI sequence parsing, per ECMA-48. This is verified by unit tests. This is no longer a gap.

---

## Roadmap by Priority

### Priority 1 — Renderer integration

| Item                       | Rationale                                                        |
| -------------------------- | ---------------------------------------------------------------- |
| DECSCNM renderer inversion | Mode is tracked; renderer just needs to invert the color palette |
| OSC 10/11 set path         | Requires making theme FG/BG mutable; config integration work     |

### Priority 2 — Polish

| Item                      | Rationale                                           |
| ------------------------- | --------------------------------------------------- |
| OSC 12 (cursor color)     | Per-session cursor color customization              |
| OSC 110/111 (reset FG/BG) | Counterpart to OSC 10/11 set                        |
| BEL visual bell           | Accessibility and feedback for scripts that use BEL |
| ESC Z (DECID)             | Some legacy scripts probe terminal type via DECID   |

### Priority 3 — Low priority / optional

| Item                          | Rationale                                             |
| ----------------------------- | ----------------------------------------------------- |
| Double-height/width lines     | Rarely used outside vttest; requires renderer changes |
| SO/SI + G1 rendering          | Almost never used in practice since UTF-8 took over   |
| IRM / SRM standard modes      | Extremely rare in modern terminal output              |
| Unparsed DEC modes (?66 etc.) | Niche compatibility; low demand                       |
| 8-bit C1 controls (0x9B)      | Modern terminals always use 7-bit sequences           |
| Sixel graphics                | Large undertaking; not planned near-term              |

---

© 2025 Freminal Project — MIT License.
