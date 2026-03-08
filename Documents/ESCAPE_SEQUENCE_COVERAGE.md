# Escape Sequence Coverage

## Last updated

Last updated: 2025-11-09 — Generated from Freminal snapshot

## Overview

Freminal currently implements approximately **70 %** of the commonly-used ANSI / DEC / xterm escape sequences.
This includes full SGR color handling (256 + true-color), comprehensive cursor movement and text-editing CSI commands,
and baseline OSC support for window and icon titles. Remaining unimplemented areas are mostly low-usage legacy
controls (alternate fonts, blink, proportional spacing) or extended features from iTerm2 / Kitty (OSC 52 clipboard,
OSC 8 hyperlinks, etc.).

## C0 / C1 Control Characters

| Code       | Name                        | Status | Notes                               |
| ---------- | --------------------------- | ------ | ----------------------------------- |
| BEL (0x07) | Bell                        | ✅     | Emits `TerminalOutput::Bell`        |
| BS (0x08)  | Backspace                   | ✅     | Moves cursor left one cell          |
| HT (0x09)  | Horizontal Tab              | ⬜     | Tab-stop management not implemented |
| LF (0x0A)  | Line Feed                   | ✅     | Moves cursor down one line          |
| CR (0x0D)  | Carriage Return             | ✅     | Moves cursor to column 0            |
| ESC (0x1B) | Escape                      | ✅     | Introduces C1/ESC/CSI/OSC sequences |
| CSI (0x9B) | Control Sequence Introducer | ✅     | Delegated to CSI parser             |

---

## Standard ESC Sequences

| Sequence               | Name                       | Status | Notes                                |
| ---------------------- | -------------------------- | ------ | ------------------------------------ |
| ESC 7                  | Save Cursor (DECSC)        | ✅     | Saves cursor position and attributes |
| ESC 8                  | Restore Cursor (DECRC)     | ✅     | Restores saved cursor                |
| ESC =                  | DECPAM                     | ✅     | Application keypad mode on           |
| ESC >                  | DECPNM                     | ✅     | Numeric keypad mode on               |
| ESC F                  | Cursor to lower-left       | ✅     | Home cursor to bottom left           |
| ESC c                  | RIS — Full Reset           | ✅     | Resets entire terminal state         |
| ESC D                  | Index                      | ✅     | Move cursor down one line            |
| ESC E                  | Next Line                  | ✅     | CR + LF combined                     |
| ESC M                  | Reverse Index              | ⬜     | Scroll up one line not yet wired     |
| ESC l                  | Memory Lock                | ✅     | Handled by standard parser           |
| ESC m                  | Memory Unlock              | ✅     | Handled by standard parser           |
| ESC n / o / \| / } / ~ | Charset invokes (GL/GR)    | ✅     | Implements G0–G3 charsets            |
| ESC # 8                | DECALN                     | ⬜     | Screen alignment test stub only      |
| ESC % @ / G            | Charset set default/UTF    | ❌     | Not planned                          |
| ESC SP F / G           | 7-/8-bit control indicator | ❌     | Out of scope                         |
| ESC (Z)                | Return terminal ID         | ⬜     | Recognized, not answered             |

---

## C1 (8-bit) Control Characters

| Sequence        | Name                        | Status | Notes                   |
| --------------- | --------------------------- | ------ | ----------------------- |
| ESC P           | DCS (Device Control String) | 🚧     | Parser stub present     |
| ESC X / ESC V/W | Start/End Guarded Area      | ❌     | Not implemented         |
| ESC [           | CSI intro                   | ✅     | Delegated to CSI parser |
| ESC ]           | OSC intro                   | ✅     | Delegated to OSC parser |
| ESC ^ / \_      | Privacy / APC               | ❌     | Not implemented         |

---

## CSI — Control Sequence Introducer

| Sequence         | Name                                        | Status | Notes                                               |
| ---------------- | ------------------------------------------- | ------ | --------------------------------------------------- |
| CSI A/B/C/D      | CUU/CUD/CUF/CUB – Cursor Up/Down/Right/Left | ✅     | Relative cursor motion                              |
| CSI E/F          | CNL/CPL – Next/Prev Line                    | ✅     | Move cursor by lines, column = 1                    |
| CSI G            | CHA – Cursor Horizontal Absolute            | ✅     | Move cursor to column n                             |
| CSI H or f       | CUP – Cursor Position                       | ✅     | Move cursor to row;col                              |
| CSI J            | ED – Erase in Display                       | ✅     | 0 → end, 1 → begin, 2 → all                         |
| CSI K            | EL – Erase in Line                          | ✅     | 0 → end, 1 → begin, 2 → all                         |
| CSI L            | IL – Insert Lines                           | ✅     | Inserts n blank lines                               |
| CSI M            | DL – Delete Lines                           | ⬜     | Not implemented yet                                 |
| CSI P            | DCH – Delete Characters                     | ⬜     | Placeholder only                                    |
| CSI X            | ECH – Erase Characters                      | ✅     | Erases n cells on line                              |
| CSI r            | DECSTBM – Set Scrolling Margins             | ✅     | Defines top/bottom scroll region                    |
| CSI n            | DSR – Device Status Report                  | 🚧     | Basic cursor pos query handled                      |
| CSI c            | DA – Device Attributes                      | ⬜     | Recognized but not answered                         |
| CSI > 0 q        | XTVERSION query                             | ✅     | Reports emulator version                            |
| CSI m            | SGR – Select Graphic Rendition              | ✅     | Full color + attribute support ([SGR.md](./SGR.md)) |
| CSI s / u        | Save / Restore Cursor Pos                   | ✅     | Handled in cursor state                             |
| CSI ? Pm h / l   | DECSET / DECRST                             | ✅     | Toggle DEC private modes                            |
| CSI ? Pm $q / $p | DECRQM / DECRQM Response                    | 🚧     | Partial mode query support                          |

---

## OSC — Operating System Commands

| Sequence                 | Purpose                             | Status | Notes                             |
| ------------------------ | ----------------------------------- | ------ | --------------------------------- |
| OSC 0 ; txt BEL          | Set icon + window title             | ✅     | Tested (`hi`)                     |
| OSC 1 ; txt BEL          | Set icon title only                 | ✅     | Shares handler with OSC 0         |
| OSC 2 ; txt BEL          | Set window title only               | ✅     | Implemented                       |
| OSC 4 ; n ; rgb          | Set palette entry                   | ⬜     | Placeholder                       |
| OSC 8 ; params ; URI BEL | Hyperlink                           | ❌     | Not yet implemented               |
| OSC 10 / 11              | Foreground / Background color query | ⬜     | Not wired                         |
| OSC 52 ; c ; data BEL    | Clipboard copy/paste                | ❌     | Planned                           |
| OSC 1337                 | iTerm2 / WezTerm extensions         | 🚧     | Recognized if enabled (1327 path) |
| OSC 777                  | System notification (Konsole)       | ❌     | Not implemented                   |

---

## DEC Private Modes (? Pm h / l)

| ?Ps               | Name                         | Status | Notes                              |
| ----------------- | ---------------------------- | ------ | ---------------------------------- |
| ?1                | DECCKM – Cursor Keys Mode    | ✅     | Normal vs Application arrows       |
| ?3                | DECCOLM – 80/132 Column Mode | ✅     | Width switch supported             |
| ?5                | DECSCNM – Reverse Video      | ✅     | Inverts colors                     |
| ?6                | DECOM – Origin Mode          | ✅     | Relative to scroll region          |
| ?7                | DECAWM – Auto Wrap Mode      | ✅     | Implemented (`Decawm` enum)        |
| ?25               | DECTCEM – Show/Hide Cursor   | ✅     | Handled by mode enum               |
| ?47 / 1047 / 1049 | Alt Screen Buffer            | ✅     | Swaps screen buffers               |
| ?1000–1006        | Mouse Tracking Modes         | 🚧     | Structure present, partial UI hook |
| ?2026             | Sync Updates Mode            | 🚧     | Supported flag only                |

---

## FTCS — FinalTerm Control Sequences

| Sequence | Name | Status | Notes                   |
| -------- | ---- | ------ | ----------------------- |
| N/A      | —    | ❌     | No FTCS implemented yet |

---

## Specification Coverage Summary

| Category             | Freminal Status | Common in VT/xterm | Notes                                |
| -------------------- | --------------- | ------------------ | ------------------------------------ |
| Core C0/C1           | ✅              | ✅                 | All practical controls covered       |
| ESC                  | ✅              | ✅                 | RIS, cursor save/restore implemented |
| CSI Cursor + Erase   | ✅              | ✅                 | Matches xterm semantics              |
| CSI Edit (IL/DL/DCH) | 🚧              | ✅                 | IL done; DL/DCH todo                 |
| SGR (Colors/Attrs)   | ✅              | ✅                 | 256 + TrueColor supported            |
| OSC 0/2 (Title)      | ✅              | ✅                 | Implemented and tested               |
| OSC 52 (Clipboard)   | ❌              | ✅                 | Common in modern terms               |
| OSC 8 (Hyperlink)    | ❌              | ✅                 | Useful for WezTerm/Kitty parity      |
| Mouse Tracking       | 🚧              | ✅                 | Partial data path                    |
| DSR/DA Queries       | 🚧              | ✅                 | Minimal responses implemented        |
| DECSET Modes         | ✅              | ✅                 | Full DECAWM/DECTCEM/DECCOLM          |
| FTCS                 | ❌              | ⬜                 | Rare outside FinalTerm / WezTerm     |
| Sixel Graphics       | ❌              | 🚧                 | Planned extension                    |

---

## References

- [SGR.md](./SGR.md) — Detailed SGR attribute coverage
- [SUPPORTED_CONTROL_CODES.md](./SUPPORTED_CONTROL_CODES.md) — Raw control code listing

---

## Next Steps

1. **Implement OSC 52** (clipboard) and **OSC 8** (hyperlinks) for iTerm2/WezTerm parity.
2. **Add DL/DCH** (CSI M/P) for full line/char editing.
3. **Complete DSR/DA** responses to improve app interoperability (`\x1B[6n`, DA2 queries).
4. **Expand Mouse Tracking** (?1000–1006) integration with GUI event system.
5. **Optional:** add DECSLRM (left/right margins) and Sixel graphics if future renderer supports it.
6. Continue updating this document as new sequences are implemented.

---

© 2025 Freminal Project. Licensed under MIT.
