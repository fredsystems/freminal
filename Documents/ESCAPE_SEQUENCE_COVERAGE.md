# Escape Sequence Coverage

## Last updated

Last updated: 2026-03-12 — Rewritten post-Task 7 completion (all 30 subtasks done)

## Overview

Freminal implements approximately **85–90 %** of the escape sequences needed for a fully
compatible modern terminal emulator. All critical bugs identified in the March 2026 audit have
been fixed. Core cursor movement, SGR colors, screen manipulation, tab stops, DEC private
modes (including cursor key mode, DECOM, DECCOLM, bracketed paste, mouse tracking, focus
reporting), clipboard (OSC 52), hyperlinks (OSC 8), shell integration (OSC 133), and modern
queries (DECRQSS, XTGETTCAP, XTVERSION, DA1/DA2) all work correctly. C0 mid-sequence handling
is ECMA-48 compliant.

The remaining gaps are primarily optional features: full renderer-side reverse video (DECSCNM),
double-height/width lines, legacy G1–G3 charset switching, and a handful of niche DEC modes.

### Status Legend

- ✅ Implemented and working correctly
- 🚧 Partially implemented or has known limitations
- ⬜ Recognized/parsed but not functional (stub or silently swallowed)
- ❌ Not implemented / not planned

---

## Known Bugs (pre-Task 7, now fixed)

All eight bugs documented in the previous audit have been resolved:

| Bug                                      | Status   | Fix Summary                                                        |
| ---------------------------------------- | -------- | ------------------------------------------------------------------ |
| DECSTBM double-decrement                 | ✅ Fixed | Removed extra `-1` from `Buffer::set_scroll_region`                |
| DL (CSI M) not wired                     | ✅ Fixed | Added `b'M'` arm in CSI dispatch calling `handle_delete_lines()`   |
| DSR ignores Ps value                     | ✅ Fixed | Ps=5 → device status, Ps=6 → cursor position report                |
| CSI u blocks ANSI restore cursor (SCORC) | ✅ Fixed | CSI u now handles SCORC; Kitty uses `CSI > u` (different prefix)   |
| DEC private modes silently swallowed     | ✅ Fixed | All modes have explicit handler arms or TerminalState mode-sync    |
| TerminalModes never written              | ✅ Fixed | mode-sync loop in `state/internal.rs` writes all modes after parse |
| Unrecognized CSI silently consumed       | ✅ Fixed | `warn!` log emitted for unknown CSI final bytes                    |
| OSC unknown double emission              | ✅ Fixed | Unknown OSC now emits `debug!` only, no `TerminalOutput::Invalid`  |

---

## C0 / C1 Control Characters

| Code       | Name            | Status | Notes                                                                |
| ---------- | --------------- | ------ | -------------------------------------------------------------------- |
| NUL (0x00) | Null            | ✅     | Silently ignored                                                     |
| BEL (0x07) | Bell            | ✅     | Emits `TerminalOutput::Bell`                                         |
| BS (0x08)  | Backspace       | ✅     | Moves cursor left one cell                                           |
| HT (0x09)  | Horizontal Tab  | ✅     | Advances to next 8-column tab stop; tab stop infrastructure complete |
| LF (0x0A)  | Line Feed       | ✅     | Moves cursor down one line                                           |
| VT (0x0B)  | Vertical Tab    | ✅     | Treated as LF per VT spec                                            |
| FF (0x0C)  | Form Feed       | ✅     | Treated as LF per VT spec                                            |
| CR (0x0D)  | Carriage Return | ✅     | Moves cursor to column 0                                             |
| SO (0x0E)  | Shift Out       | ⬜     | G1 charset switching not implemented (parsed, no functional effect)  |
| SI (0x0F)  | Shift In        | ⬜     | G0 charset switching not implemented (parsed, no functional effect)  |
| ESC (0x1B) | Escape          | ✅     | Introduces C1/ESC/CSI/OSC sequences                                  |
| DEL (0x7F) | Delete          | ✅     | Silently ignored                                                     |
| CSI (0x9B) | CSI (8-bit)     | ⬜     | 8-bit C1 controls not parsed                                         |

### C0 Mid-Sequence Handling

The VT500 spec requires C0 controls to be executed inline even during CSI/OSC sequence
parsing. Freminal's parser is **ECMA-48 compliant** here: C0 controls (BS, CR, LF, VT, FF,
etc.) encountered mid-CSI are executed inline, and the CSI sequence resumes afterward. This
is verified by unit tests (`c0_bs_inside_csi`, `c0_cr_inside_csi`, `c0_vt_inside_csi`).

---

## Standard ESC Sequences

| Sequence               | Name                       | Status | Notes                                                                      |
| ---------------------- | -------------------------- | ------ | -------------------------------------------------------------------------- |
| ESC 7                  | Save Cursor (DECSC)        | ✅     | Saves cursor position and attributes                                       |
| ESC 8                  | Restore Cursor (DECRC)     | ✅     | Restores saved cursor                                                      |
| ESC =                  | DECPAM                     | ✅     | Sets application keypad mode in `TerminalModes.keypad_mode`                |
| ESC >                  | DECPNM                     | ✅     | Sets numeric keypad mode in `TerminalModes.keypad_mode`                    |
| ESC F                  | Cursor to lower-left       | ⬜     | Parsed, stub with debug log                                                |
| ESC c                  | RIS — Full Reset           | ✅     | Fully implemented — resets buffer, cursor, modes, tab stops, scroll region |
| ESC D                  | Index (IND)                | ✅     | Move cursor down one line, scrolls if at bottom                            |
| ESC E                  | Next Line (NEL)            | ✅     | CR + LF combined                                                           |
| ESC H                  | Tab Set (HTS)              | ✅     | Sets tab stop at current cursor column                                     |
| ESC M                  | Reverse Index (RI)         | ✅     | Scroll up one line — fully implemented                                     |
| ESC Z                  | Return Terminal ID (DECID) | ⬜     | Not parsed                                                                 |
| ESC l                  | Memory Lock                | ⬜     | Parsed, stub with debug log                                                |
| ESC m                  | Memory Unlock              | ⬜     | Parsed, stub with debug log                                                |
| ESC ( 0                | G0 Charset — Line Drawing  | ✅     | DEC Special Graphics charset                                               |
| ESC ( B                | G0 Charset — US ASCII      | ✅     | Default ASCII charset                                                      |
| ESC n / o / \| / } / ~ | Charset invokes (GL/GR)    | ⬜     | Parsed, no functional effect (G1–G3 switching not implemented)             |
| ESC # 8                | DECALN                     | ✅     | Fills screen with 'E', resets cursor and scroll region                     |
| ESC % @ / G            | Charset set default/UTF    | ❌     | Not planned                                                                |
| ESC SP F / G           | 7-/8-bit control indicator | ❌     | Out of scope                                                               |

---

## C1 (8-bit) Control Characters

| Sequence        | Name                        | Status | Notes                                                                               |
| --------------- | --------------------------- | ------ | ----------------------------------------------------------------------------------- |
| ESC P           | DCS (Device Control String) | ✅     | Sub-command dispatch implemented: DECRQSS (`$q`) and XTGETTCAP (`+q`) fully handled |
| ESC X / ESC V/W | Start/End Guarded Area      | ❌     | Not implemented                                                                     |
| ESC [           | CSI intro                   | ✅     | Delegated to CSI parser                                                             |
| ESC ]           | OSC intro                   | ✅     | Delegated to OSC parser                                                             |
| ESC ^           | Privacy Message (PM)        | ❌     | Not implemented                                                                     |
| ESC \_          | APC                         | 🚧     | Captured as opaque bytes, no sub-command parsing                                    |

---

## CSI — Control Sequence Introducer

| Sequence      | Name                                | Status | Notes                                                                       |
| ------------- | ----------------------------------- | ------ | --------------------------------------------------------------------------- |
| CSI Ps A      | CUU — Cursor Up                     | ✅     | Relative cursor motion                                                      |
| CSI Ps B      | CUD — Cursor Down                   | ✅     | Relative cursor motion                                                      |
| CSI Ps C      | CUF — Cursor Forward                | ✅     | Relative cursor motion                                                      |
| CSI Ps D      | CUB — Cursor Backward               | ✅     | Relative cursor motion                                                      |
| CSI Ps E      | CNL — Cursor Next Line              | ✅     | Moves down Ps lines, then to column 1                                       |
| CSI Ps F      | CPL — Cursor Previous Line          | ✅     | Moves up Ps lines, then to column 1                                         |
| CSI Ps G      | CHA — Cursor Horizontal Absolute    | ✅     | Move cursor to column n                                                     |
| CSI Ps H      | CUP — Cursor Position               | ✅     | Move cursor to row;col                                                      |
| CSI Ps I      | CHT — Cursor Horizontal Forward Tab | ✅     | Advances cursor by Ps tab stops                                             |
| CSI Ps J      | ED — Erase in Display               | ✅     | 0 → end, 1 → begin, 2 → all, 3 → scrollback                                 |
| CSI Ps K      | EL — Erase in Line                  | ✅     | 0 → end, 1 → begin, 2 → all                                                 |
| CSI Ps L      | IL — Insert Lines                   | ✅     | Inserts n blank lines                                                       |
| CSI Ps M      | DL — Delete Lines                   | ✅     | Deletes n lines at cursor position                                          |
| CSI Ps P      | DCH — Delete Characters             | ✅     | Fully implemented                                                           |
| CSI Ps S      | SU — Scroll Up                      | ✅     | Scrolls content up n lines within scroll region                             |
| CSI Ps T      | SD — Scroll Down                    | ✅     | Scrolls content down n lines within scroll region                           |
| CSI Ps X      | ECH — Erase Characters              | ✅     | Erases n cells on line                                                      |
| CSI Ps Z      | CBT — Cursor Backward Tab           | ✅     | Moves cursor back by Ps tab stops                                           |
| CSI Ps @      | ICH — Insert Characters             | ✅     | Fully implemented                                                           |
| CSI Ps \`     | HPA — Horizontal Position Absolute  | ✅     | Alias for CHA (CSI G)                                                       |
| CSI Ps b      | REP — Repeat Last Character         | ✅     | Repeats preceding graphic character Ps times with same SGR attributes       |
| CSI Ps d      | VPA — Vertical Position Absolute    | ✅     | Move cursor to row n                                                        |
| CSI Ps f      | HVP — Horizontal Vertical Position  | ✅     | Same as CUP                                                                 |
| CSI Ps g      | TBC — Tab Clear                     | ✅     | Ps=0 clears at current column, Ps=3 clears all tab stops                    |
| CSI Ps ; Ps r | DECSTBM — Set Scrolling Margins     | ✅     | Double-decrement bug fixed; correct 0-based region from 1-based params      |
| CSI Ps n      | DSR — Device Status Report          | ✅     | Ps=5 → device status, Ps=6 → cursor position report                         |
| CSI Ps c      | DA1 — Primary Device Attributes     | ✅     | Responds with device attributes                                             |
| CSI > Ps c    | DA2 — Secondary Device Attributes   | ✅     | Responds with version info                                                  |
| CSI Ps > q    | XTVERSION                           | ✅     | Reports emulator version                                                    |
| CSI Ps SP q   | DECSCUSR — Set Cursor Style         | ✅     | Block, underline, bar cursor styles                                         |
| CSI Ps m      | SGR — Select Graphic Rendition      | ✅     | Full color + attribute support ([SGR.md](./SGR.md))                         |
| CSI Ps t      | Window Manipulation                 | ✅     | Terminal geometry interactions                                              |
| CSI ? Pm h    | DECSET — Set DEC Private Mode       | ✅     | All recognized modes have explicit handlers (see DEC Private Modes section) |
| CSI ? Pm l    | DECRST — Reset DEC Private Mode     | ✅     | Same as DECSET                                                              |
| CSI s         | Save Cursor Position (SCOSC)        | ✅     | Implemented                                                                 |
| CSI u         | Restore Cursor Position (SCORC)     | ✅     | Fixed; Kitty protocol uses `CSI > u` (different prefix)                     |
| CSI ? Pm $p   | DECRQM — Request Mode               | ✅     | Full mode query support via mode-sync loop                                  |
| CSI Ps h      | SM — Set Standard Mode              | 🚧     | LNM (mode 20) implemented. IRM (4), SRM (12) missing.                       |
| CSI Ps l      | RM — Reset Standard Mode            | 🚧     | Same as SM                                                                  |

---

## OSC — Operating System Commands

| Sequence                 | Purpose                       | Status | Notes                                                             |
| ------------------------ | ----------------------------- | ------ | ----------------------------------------------------------------- |
| OSC 0 ; txt BEL          | Set icon + window title       | 🚧     | Works, but icon name vs. title not distinguished                  |
| OSC 1 ; txt BEL          | Set icon title only           | 🚧     | Shares handler with OSC 0 (treats as full title)                  |
| OSC 2 ; txt BEL          | Set window title only         | ✅     | Implemented                                                       |
| OSC 4 ; n ; rgb          | Set palette entry             | ✅     | Sets 256-color palette entry; query responds with current value   |
| OSC 7 ; URI              | Current Working Directory     | ✅     | Parsed and stored in `TerminalHandler.current_working_directory`  |
| OSC 8 ; params ; URI BEL | Hyperlink                     | ✅     | Fully implemented — hyperlink start/end with URL metadata         |
| OSC 10 ; ? BEL           | Foreground color query/set    | ✅     | Query returns theme fg (or dynamic override); set stores override |
| OSC 11 ; ? BEL           | Background color query/set    | ✅     | Query returns theme bg (or dynamic override); set stores override |
| OSC 12 ; color           | Set cursor color              | ⬜     | Not implemented                                                   |
| OSC 52 ; c ; data BEL    | Clipboard copy/paste          | ✅     | Implemented — base64 encode/decode, clipboard set/query           |
| OSC 104                  | Reset palette entry           | ✅     | Resets specific or all palette entries to defaults                |
| OSC 110                  | Reset foreground color        | ✅     | Clears dynamic fg override; query returns theme default           |
| OSC 111                  | Reset background color        | ✅     | Clears dynamic bg override; query returns theme default           |
| OSC 112                  | Reset cursor color            | ⬜     | Empty match arm, no-op                                            |
| OSC 133 ; …              | FTCS / Shell Integration      | ✅     | All four markers parsed and stored in `FtcsState`                 |
| OSC 777                  | System notification (Konsole) | ⬜     | Not implemented                                                   |
| OSC 1337                 | iTerm2 / WezTerm extensions   | ⬜     | Recognized if enabled (debug log only)                            |

---

## DCS — Device Control String

| Sequence     | Name                 | Status | Notes                                                                          |
| ------------ | -------------------- | ------ | ------------------------------------------------------------------------------ |
| DCS (all)    | General DCS handling | ✅     | Sub-command dispatch via `handle_device_control_string()`                      |
| DCS $ q … ST | DECRQSS              | ✅     | Supports `m` (SGR), `r` (DECSTBM), `SP q` (DECSCUSR); unknown → error response |
| DCS + q … ST | XTGETTCAP            | ✅     | Responds to common capability queries; unknown → error response                |
| DCS Sixel    | Sixel Graphics       | ⬜     | Not parsed — large undertaking                                                 |

---

## DEC Private Modes (CSI ? Pm h / l)

| ?Ps   | Name                             | Status | Notes                                                                                                                                |
| ----- | -------------------------------- | ------ | ------------------------------------------------------------------------------------------------------------------------------------ |
| ?1    | DECCKM — Cursor Keys Mode        | ✅     | Wired to `TerminalModes.cursor_key`; GUI reads for application/normal arrow key translation                                          |
| ?3    | DECCOLM — 80/132 Column Mode     | ✅     | Mode stored and acted on when `AllowColumnModeSwitch (?40)` is enabled                                                               |
| ?5    | DECSCNM — Reverse Video          | 🚧     | Mode stored in `TerminalModes.invert_screen`; renderer swaps background fill but full cell-level fg/bg inversion not yet implemented |
| ?6    | DECOM — Origin Mode              | ✅     | Mode stored and applied; CUP row 1 → top of scroll region when set                                                                   |
| ?7    | DECAWM — Auto Wrap Mode          | ✅     | Implemented (`Decawm` enum)                                                                                                          |
| ?8    | DECARM — Auto Repeat Keys        | ✅     | Mode stored in `TerminalModes.repeat_keys`                                                                                           |
| ?12   | XtCBlink — Cursor Blink          | ✅     | Implemented                                                                                                                          |
| ?25   | DECTCEM — Show/Hide Cursor       | ✅     | Implemented                                                                                                                          |
| ?40   | AllowColumnModeSwitch            | ✅     | Gates DECCOLM behavior                                                                                                               |
| ?45   | ReverseWrapAround                | ✅     | Mode stored in `TerminalModes.reverse_wrap_around`                                                                                   |
| ?47   | Alt Screen Buffer (legacy)       | ✅     | Wired to same alt-screen machinery as ?1049                                                                                          |
| ?1000 | X11 Mouse — Normal Tracking      | ✅     | Mode stored in `TerminalModes.mouse_tracking`; GUI reads and forwards events                                                         |
| ?1002 | X11 Mouse — Button Event         | ✅     | Mode stored in `TerminalModes.mouse_tracking`                                                                                        |
| ?1003 | X11 Mouse — Any Event            | ✅     | Mode stored in `TerminalModes.mouse_tracking`                                                                                        |
| ?1004 | Focus Reporting                  | ✅     | Mode stored in `TerminalModes.focus_reporting`; GUI sends focus events                                                               |
| ?1006 | SGR Mouse — Extended Coordinates | ✅     | Mode stored in `TerminalModes.mouse_tracking`                                                                                        |
| ?1047 | Alt Screen Buffer (legacy)       | ✅     | Wired to same alt-screen machinery as ?1049                                                                                          |
| ?1048 | Save/Restore Cursor (legacy)     | ✅     | Wired to existing save/restore cursor machinery                                                                                      |
| ?1049 | Alt Screen Buffer + Save Cursor  | ✅     | Implemented — swaps screen buffers                                                                                                   |
| ?2004 | Bracketed Paste                  | ✅     | Mode stored in `TerminalModes.bracketed_paste`; GUI wraps paste with bracket sequences                                               |
| ?2026 | Synchronized Output              | ✅     | Mode stored in `TerminalModes.synchronized_updates`                                                                                  |

### Not Yet Parsed

| Mode          | Description                    |
| ------------- | ------------------------------ |
| ?2 (DECANM)   | VT52 mode                      |
| ?66 (DECNKM)  | Numeric keypad mode (DEC)      |
| ?67 (DECBKM)  | Backarrow key mode             |
| ?69 (DECLRMM) | Left/right margin mode         |
| ?1001, ?1007  | Hilite mouse, alternate scroll |
| ?1034         | Interpret meta key             |

---

## Standard Modes (CSI Pm h / l)

| Ps  | Name                          | Status | Notes           |
| --- | ----------------------------- | ------ | --------------- |
| 4   | IRM — Insert/Replace Mode     | ⬜     | Not implemented |
| 12  | SRM — Send/Receive Mode       | ⬜     | Not implemented |
| 20  | LNM — Line Feed/New Line Mode | ✅     | Implemented     |

---

## FTCS — FinalTerm Control Sequences (OSC 133)

| Sequence  | Name                  | Status | Notes                                            |
| --------- | --------------------- | ------ | ------------------------------------------------ |
| OSC 133 A | Prompt Start          | ✅     | Parsed and stored in `FtcsState`                 |
| OSC 133 B | Prompt End            | ✅     | Parsed and stored in `FtcsState`                 |
| OSC 133 C | Pre-execution (input) | ✅     | Parsed and stored in `FtcsState`                 |
| OSC 133 D | Command Finished      | ✅     | Parsed with exit code stored in `last_exit_code` |

---

## Specification Coverage Summary

| Category                       | Freminal Status | Common in VT/xterm | Notes                                                                  |
| ------------------------------ | --------------- | ------------------ | ---------------------------------------------------------------------- |
| Core C0/C1                     | ✅              | ✅                 | BEL/BS/LF/CR/HT/VT/FF/ESC/NUL/DEL all handled correctly                |
| ESC                            | ✅              | ✅                 | Save/restore cursor, IND, NEL, RI, HTS, RIS, DECPAM/DECPNM all working |
| CSI Cursor + Erase             | ✅              | ✅                 | CUU/CUD/CUF/CUB/CHA/CUP/CNL/CPL/ED/EL all correct                      |
| CSI Edit (IL/DL/DCH/ICH/REP)   | ✅              | ✅                 | All working including REP                                              |
| CSI Scroll (SU/SD)             | ✅              | ✅                 | Implemented, respects scroll region                                    |
| Tab Stops (HT/HTS/TBC/CHT/CBT) | ✅              | ✅                 | Full tab stop infrastructure with default 8-column stops               |
| SGR (Colors/Attrs)             | ✅              | ✅                 | 256 + TrueColor supported                                              |
| OSC 0/2 (Title)                | ✅              | ✅                 | Implemented                                                            |
| OSC 4/104 (Palette)            | ✅              | ✅                 | Mutable 256-color palette with set/query/reset                         |
| OSC 7 (CWD)                    | ✅              | ✅                 | CWD parsed and stored                                                  |
| OSC 8 (Hyperlink)              | ✅              | ✅                 | Fully implemented                                                      |
| OSC 52 (Clipboard)             | ✅              | ✅                 | Clipboard copy/query via base64                                        |
| OSC 133 (FTCS)                 | ✅              | 🚧                 | All four markers parsed and stored                                     |
| Mouse Tracking                 | ✅              | ✅                 | Modes wired; GUI reads and forwards events                             |
| Bracketed Paste                | ✅              | ✅                 | Mode wired; GUI wraps paste events                                     |
| DSR/DA Queries                 | ✅              | ✅                 | DA1/DA2/DSR all work correctly                                         |
| DECSET Modes                   | ✅              | ✅                 | All commonly-used modes handled                                        |
| DCS Sub-commands               | ✅              | 🚧                 | DECRQSS and XTGETTCAP fully implemented                                |
| DECOM / Origin Mode            | ✅              | ✅                 | CUP addressing relative to scroll region when DECOM is set             |
| DECCOLM                        | ✅              | 🚧                 | Column switching works when AllowColumnModeSwitch (?40) is enabled     |
| DECSCNM / Reverse Video        | 🚧              | ✅                 | Mode tracked; renderer screen-inversion not yet implemented            |
| Double-height/width lines      | ⬜              | 🚧                 | Parsed but renderer does not implement visual double-height/width      |
| SO/SI (G1 charset switching)   | ⬜              | 🚧                 | Parsed but G1 rendering not implemented                                |
| Sixel Graphics                 | ⬜              | 🚧                 | Not implemented                                                        |
| OSC 10/11 (FG/BG color query)  | ✅              | ✅                 | Query/set/reset fully implemented with theme-aware defaults            |

---

## References

- [SGR.md](./SGR.md) — Detailed SGR attribute coverage
- [SUPPORTED_CONTROL_CODES.md](./SUPPORTED_CONTROL_CODES.md) — Raw control code listing
- [ESCAPE_SEQUENCE_GAPS.md](./ESCAPE_SEQUENCE_GAPS.md) — Remaining gaps and roadmap
- [PLAN_07_ESCAPE_SEQUENCES.md](./PLAN_07_ESCAPE_SEQUENCES.md) — Completed implementation plan (all 30 subtasks done)

---

## Remaining Gaps

The gaps that remain are either low-priority polish or require significant new infrastructure:

1. **Renderer-side DECSCNM** — The mode is tracked but the renderer does not yet invert the screen colors.
2. **Double-height/width lines** (ESC # 3/4/5/6) — The renderer does not support these.
3. **SO/SI G1 charset switching** — Parsed but rendering from G1 not implemented.
4. **OSC 12** (cursor color) — Not implemented.
5. **Sixel Graphics** — Large undertaking, not planned near-term.
6. **Standard modes IRM/SRM** — Rare in practice.
7. **Unparsed DEC modes** — ?2 (DECANM/VT52), ?66 (DECNKM), ?67 (DECBKM), ?69 (DECLRMM).

---

© 2025 Freminal Project. Licensed under MIT.
