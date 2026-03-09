# Escape Sequence Coverage

## Last updated

Last updated: 2026-03-09 — Corrected via comprehensive codebase audit (Task 7)

## Overview

Freminal implements approximately **50–55 %** of the escape sequences needed for a fully
compatible modern terminal emulator. Core cursor movement, SGR colors, and basic screen
manipulation work correctly. However, the audit revealed several **critical bugs** (DECSTBM
double-decrement, DL not wired, DEC private modes silently swallowed) and significant gaps
in C0 control handling, tab stop infrastructure, and modern features (mouse tracking,
bracketed paste, clipboard). These findings are tracked in `PLAN_07_ESCAPE_SEQUENCES.md`.

### Status Legend

- ✅ Implemented and working correctly
- 🚧 Partially implemented or has known bugs
- ⬜ Recognized/parsed but not functional (stub or silently swallowed)
- ❌ Not implemented / not planned

---

## Known Bugs

These are sequences that appear to work but produce incorrect behavior:

| Bug                                      | Location                                              | Description                                                                                                                                                                                                                                                  |
| ---------------------------------------- | ----------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **DECSTBM double-decrement**             | `terminal_handler.rs:212-216` + `buffer.rs:1198-1225` | `handle_set_scroll_region` subtracts 1 from params, then `Buffer::set_scroll_region` subtracts 1 again. `CSI 3;20 r` produces region [1,18] instead of correct [2,19]. Default case accidentally works. **Likely cause of vttest cursor movement failures.** |
| **DL (CSI M) not wired**                 | `csi.rs` dispatch table                               | Buffer has `delete_lines()`, handler has `handle_delete_lines()`, but CSI dispatch has NO `b'M'` arm. The sequence is silently consumed.                                                                                                                     |
| **DSR ignores Ps value**                 | `terminal_handler.rs`                                 | `CSI n` always emits cursor position report regardless of Ps. `Ps=5` should respond with device status `CSI 0 n`, not cursor position.                                                                                                                       |
| **CSI u blocks ANSI restore cursor**     | `csi.rs` dispatch                                     | CSI u is mapped to Kitty keyboard protocol handler (always returns `Skipped`), preventing standard ANSI restore-cursor (SCORC) from working.                                                                                                                 |
| **DEC private modes silently swallowed** | `terminal_handler.rs:651-654`                         | 14+ DEC private modes (including ?1 DECCKM, ?2004 bracketed paste, ?1000–?1006 mouse) fall through the `_other` catch-all with NO logging and no effect on `TerminalModes`.                                                                                  |
| **TerminalModes never written**          | `state/internal.rs`                                   | `TerminalState.modes` has fields for cursor_key, bracketed_paste, focus_reporting, mouse_tracking, synchronized_updates, etc. — these are read by snapshots but NEVER WRITTEN by the mode handler. Only `playback.rs` writes them.                           |
| **Unrecognized CSI silently consumed**   | `csi.rs:240`                                          | Unknown CSI final bytes fall through with no log and no effect. Should at least emit `warn!`.                                                                                                                                                                |
| **OSC unknown double emission**          | `osc.rs`                                              | Unrecognized OSC produces error-level logging AND `TerminalOutput::Invalid` (double emission). Most terminals silently consume unknown OSC.                                                                                                                  |

---

## C0 / C1 Control Characters

| Code       | Name            | Status | Notes                                                                                                                                                                    |
| ---------- | --------------- | ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| NUL (0x00) | Null            | ⬜     | **Not handled** — included in data instead of being ignored                                                                                                              |
| BEL (0x07) | Bell            | ✅     | Emits `TerminalOutput::Bell`                                                                                                                                             |
| BS (0x08)  | Backspace       | ✅     | Moves cursor left one cell                                                                                                                                               |
| HT (0x09)  | Horizontal Tab  | ⬜     | **Not handled as C0** — byte falls through as data. No tab stop infrastructure exists (no default 8-column stops, no tab stop array). Breaks `ls`, `man`, shell prompts. |
| LF (0x0A)  | Line Feed       | ✅     | Moves cursor down one line                                                                                                                                               |
| VT (0x0B)  | Vertical Tab    | ⬜     | **Not handled** — should act as LF per VT spec                                                                                                                           |
| FF (0x0C)  | Form Feed       | ⬜     | **Not handled** — should act as LF per VT spec                                                                                                                           |
| CR (0x0D)  | Carriage Return | ✅     | Moves cursor to column 0                                                                                                                                                 |
| SO (0x0E)  | Shift Out       | ⬜     | **Not handled** — G1 charset switching missing                                                                                                                           |
| SI (0x0F)  | Shift In        | ⬜     | **Not handled** — G0 charset switching missing                                                                                                                           |
| ESC (0x1B) | Escape          | ✅     | Introduces C1/ESC/CSI/OSC sequences                                                                                                                                      |
| DEL (0x7F) | Delete          | ⬜     | **Not handled** — should be silently ignored                                                                                                                             |
| CSI (0x9B) | CSI (8-bit)     | ⬜     | **Not recognized** — 8-bit C1 controls not parsed                                                                                                                        |

### C0 Mid-Sequence Handling

The VT500 spec requires C0 controls to be executed inline even during CSI/OSC sequence
parsing. The current parser treats C0 mid-CSI as errors and aborts the sequence. This is
a conformance issue but low practical impact.

---

## Standard ESC Sequences

| Sequence               | Name                       | Status | Notes                                                                      |
| ---------------------- | -------------------------- | ------ | -------------------------------------------------------------------------- |
| ESC 7                  | Save Cursor (DECSC)        | ✅     | Saves cursor position and attributes                                       |
| ESC 8                  | Restore Cursor (DECRC)     | ✅     | Restores saved cursor                                                      |
| ESC =                  | DECPAM                     | ⬜     | **Stub** — parsed with `warn!` log, no effect on keypad mode               |
| ESC >                  | DECPNM                     | ⬜     | **Stub** — parsed with `warn!` log, no effect on keypad mode               |
| ESC F                  | Cursor to lower-left       | ⬜     | **Stub** — parsed with `warn!` log                                         |
| ESC c                  | RIS — Full Reset           | ⬜     | **Stub** — parsed with `warn!` log, does not actually reset terminal state |
| ESC D                  | Index (IND)                | ✅     | Move cursor down one line, scrolls if at bottom                            |
| ESC E                  | Next Line (NEL)            | ✅     | CR + LF combined                                                           |
| ESC H                  | Tab Set (HTS)              | ⬜     | **Not parsed** — no tab stop infrastructure                                |
| ESC M                  | Reverse Index (RI)         | ✅     | Scroll up one line — fully implemented                                     |
| ESC Z                  | Return Terminal ID (DECID) | ⬜     | **Not parsed**                                                             |
| ESC l                  | Memory Lock                | ⬜     | **Stub** — parsed with `warn!` log                                         |
| ESC m                  | Memory Unlock              | ⬜     | **Stub** — parsed with `warn!` log                                         |
| ESC ( 0                | G0 Charset — Line Drawing  | ✅     | DEC Special Graphics charset                                               |
| ESC ( B                | G0 Charset — US ASCII      | ✅     | Default ASCII charset                                                      |
| ESC n / o / \| / } / ~ | Charset invokes (GL/GR)    | ⬜     | **Stub** — parsed in standard.rs, no functional effect                     |
| ESC # 8                | DECALN                     | ⬜     | Screen alignment test — stub only                                          |
| ESC % @ / G            | Charset set default/UTF    | ❌     | Not planned                                                                |
| ESC SP F / G           | 7-/8-bit control indicator | ❌     | Out of scope                                                               |

---

## C1 (8-bit) Control Characters

| Sequence        | Name                        | Status | Notes                                                                                                  |
| --------------- | --------------------------- | ------ | ------------------------------------------------------------------------------------------------------ |
| ESC P           | DCS (Device Control String) | 🚧     | Parser captures bytes, debug log only. No sub-command parsing (DECRQSS, XTGETTCAP, Sixel all missing). |
| ESC X / ESC V/W | Start/End Guarded Area      | ❌     | Not implemented                                                                                        |
| ESC [           | CSI intro                   | ✅     | Delegated to CSI parser                                                                                |
| ESC ]           | OSC intro                   | ✅     | Delegated to OSC parser                                                                                |
| ESC ^           | Privacy Message (PM)        | ❌     | Not implemented (no PM state in parser)                                                                |
| ESC \_          | APC                         | 🚧     | Captured as opaque bytes, no sub-command parsing                                                       |

---

## CSI — Control Sequence Introducer

| Sequence      | Name                                | Status | Notes                                                                               |
| ------------- | ----------------------------------- | ------ | ----------------------------------------------------------------------------------- |
| CSI Ps A      | CUU — Cursor Up                     | ✅     | Relative cursor motion                                                              |
| CSI Ps B      | CUD — Cursor Down                   | ✅     | Relative cursor motion                                                              |
| CSI Ps C      | CUF — Cursor Forward                | ✅     | Relative cursor motion                                                              |
| CSI Ps D      | CUB — Cursor Backward               | ✅     | Relative cursor motion                                                              |
| CSI Ps E      | CNL — Cursor Next Line              | ⬜     | **Not implemented** — missing from CSI dispatch                                     |
| CSI Ps F      | CPL — Cursor Previous Line          | ⬜     | **Not implemented** — missing from CSI dispatch                                     |
| CSI Ps G      | CHA — Cursor Horizontal Absolute    | ✅     | Move cursor to column n                                                             |
| CSI Ps H      | CUP — Cursor Position               | ✅     | Move cursor to row;col                                                              |
| CSI Ps I      | CHT — Cursor Horizontal Forward Tab | ⬜     | **Not implemented** — no tab stop infrastructure                                    |
| CSI Ps J      | ED — Erase in Display               | ✅     | 0 → end, 1 → begin, 2 → all                                                         |
| CSI Ps K      | EL — Erase in Line                  | ✅     | 0 → end, 1 → begin, 2 → all                                                         |
| CSI Ps L      | IL — Insert Lines                   | ✅     | Inserts n blank lines                                                               |
| CSI Ps M      | DL — Delete Lines                   | 🐛     | **BUG: Not wired** — handler + buffer code exist but CSI dispatch has no `b'M'` arm |
| CSI Ps P      | DCH — Delete Characters             | ✅     | Fully implemented                                                                   |
| CSI Ps S      | SU — Scroll Up                      | ⬜     | **Not implemented**                                                                 |
| CSI Ps T      | SD — Scroll Down                    | ⬜     | **Not implemented**                                                                 |
| CSI Ps X      | ECH — Erase Characters              | ✅     | Erases n cells on line                                                              |
| CSI Ps Z      | CBT — Cursor Backward Tab           | ⬜     | **Not implemented** — no tab stop infrastructure                                    |
| CSI Ps @      | ICH — Insert Characters             | ✅     | Fully implemented                                                                   |
| CSI Ps \`     | HPA — Horizontal Position Absolute  | ⬜     | **Not implemented**                                                                 |
| CSI Ps b      | REP — Repeat Last Character         | ⬜     | **Not implemented**                                                                 |
| CSI Ps d      | VPA — Vertical Position Absolute    | ✅     | Move cursor to row n                                                                |
| CSI Ps f      | HVP — Horizontal Vertical Position  | ✅     | Same as CUP                                                                         |
| CSI Ps g      | TBC — Tab Clear                     | ⬜     | **Not implemented** — no tab stop infrastructure                                    |
| CSI Ps ; Ps r | DECSTBM — Set Scrolling Margins     | 🐛     | **BUG: Double-decrement** — both handler and buffer subtract 1 from params          |
| CSI Ps n      | DSR — Device Status Report          | 🐛     | **BUG: Ignores Ps** — always emits cursor report, Ps=5 should give status report    |
| CSI Ps c      | DA1 — Primary Device Attributes     | ✅     | Responds with device attributes                                                     |
| CSI > Ps c    | DA2 — Secondary Device Attributes   | ✅     | Responds with version info                                                          |
| CSI Ps > q    | XTVERSION                           | ✅     | Reports emulator version                                                            |
| CSI Ps SP q   | DECSCUSR — Set Cursor Style         | ✅     | Block, underline, bar cursor styles                                                 |
| CSI Ps m      | SGR — Select Graphic Rendition      | ✅     | Full color + attribute support ([SGR.md](./SGR.md))                                 |
| CSI Ps t      | Window Manipulation                 | ✅     | Terminal geometry interactions                                                      |
| CSI ? Pm h    | DECSET — Set DEC Private Mode       | 🚧     | Only 4 of 18+ modes actually take effect (see DEC Private Modes section)            |
| CSI ? Pm l    | DECRST — Reset DEC Private Mode     | 🚧     | Same issue as DECSET                                                                |
| CSI s         | Save Cursor Position (SCOSC)        | ⬜     | **Not implemented** — no `b's'` arm in CSI dispatch                                 |
| CSI u         | Restore Cursor Position (SCORC)     | ⬜     | **Blocked** — mapped to Kitty keyboard protocol (always Skipped)                    |
| CSI ? Pm $p   | DECRQM — Request Mode               | 🚧     | Partial mode query support                                                          |
| CSI Ps h      | SM — Set Standard Mode              | 🚧     | Only LNM (mode 20) implemented. IRM (4), SRM (12) missing.                          |
| CSI Ps l      | RM — Reset Standard Mode            | 🚧     | Same as SM                                                                          |

---

## OSC — Operating System Commands

| Sequence                 | Purpose                       | Status | Notes                                                         |
| ------------------------ | ----------------------------- | ------ | ------------------------------------------------------------- |
| OSC 0 ; txt BEL          | Set icon + window title       | 🚧     | Works, but icon name vs. title not distinguished              |
| OSC 1 ; txt BEL          | Set icon title only           | 🚧     | Shares handler with OSC 0 (treats as full title)              |
| OSC 2 ; txt BEL          | Set window title only         | ✅     | Implemented                                                   |
| OSC 4 ; n ; rgb          | Set palette entry             | ⬜     | Not implemented                                               |
| OSC 7 ; URI              | Current Working Directory     | ⬜     | Recognized, debug log only, no functional effect              |
| OSC 8 ; params ; URI BEL | Hyperlink                     | ✅     | **Fully implemented** — hyperlink start/end with URL metadata |
| OSC 10 ; ? BEL           | Foreground color query        | 🚧     | Query responds with hardcoded color; set is a no-op           |
| OSC 11 ; ? BEL           | Background color query        | 🚧     | Query responds with hardcoded color; set is a no-op           |
| OSC 12 ; color           | Set cursor color              | ⬜     | Not implemented                                               |
| OSC 52 ; c ; data BEL    | Clipboard copy/paste          | ⬜     | Not implemented                                               |
| OSC 104                  | Reset palette entry           | ⬜     | Not implemented                                               |
| OSC 110                  | Reset foreground color        | ⬜     | Not implemented                                               |
| OSC 111                  | Reset background color        | ⬜     | Not implemented                                               |
| OSC 112                  | Reset cursor color            | ⬜     | Recognized — empty match arm, no-op                           |
| OSC 133 ; …              | FTCS / Shell Integration      | ⬜     | Recognized, debug log only, no functional effect              |
| OSC 777                  | System notification (Konsole) | ⬜     | Not implemented                                               |
| OSC 1337                 | iTerm2 / WezTerm extensions   | ⬜     | Recognized if enabled (debug log only)                        |

---

## DCS — Device Control String

| Sequence     | Name                 | Status | Notes                                    |
| ------------ | -------------------- | ------ | ---------------------------------------- |
| DCS (all)    | General DCS handling | 🚧     | Captured as opaque bytes, debug log only |
| DCS $ q … ST | DECRQSS              | ⬜     | Not parsed — no sub-command dispatch     |
| DCS + q … ST | XTGETTCAP            | ⬜     | Not parsed — nvim sends this             |
| DCS Sixel    | Sixel Graphics       | ⬜     | Not parsed — large undertaking           |

---

## DEC Private Modes (CSI ? Pm h / l)

| ?Ps   | Name                             | Status | Notes                                                                                                                                                    |
| ----- | -------------------------------- | ------ | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| ?1    | DECCKM — Cursor Keys Mode        | ⬜     | **Parsed but silently swallowed** — `_other` catch-all at `terminal_handler.rs:651`. TerminalModes.cursor_key never written. **Breaks vim, tmux, htop.** |
| ?3    | DECCOLM — 80/132 Column Mode     | ⬜     | **Parsed but silently swallowed** via `_other` catch-all                                                                                                 |
| ?5    | DECSCNM — Reverse Video          | ⬜     | **Parsed but silently swallowed** — reverse video never toggles                                                                                          |
| ?6    | DECOM — Origin Mode              | ⬜     | **Parsed but silently swallowed** — origin mode ignored                                                                                                  |
| ?7    | DECAWM — Auto Wrap Mode          | ✅     | Implemented (`Decawm` enum)                                                                                                                              |
| ?12   | XtCBlink — Cursor Blink          | ✅     | Implemented                                                                                                                                              |
| ?25   | DECTCEM — Show/Hide Cursor       | ✅     | Implemented                                                                                                                                              |
| ?47   | Alt Screen Buffer (legacy)       | ⬜     | **Not parsed** — only ?1049 is implemented                                                                                                               |
| ?1000 | X11 Mouse — Normal Tracking      | ⬜     | **Parsed but silently swallowed** — TerminalModes never written                                                                                          |
| ?1002 | X11 Mouse — Button Event         | ⬜     | **Parsed but silently swallowed**                                                                                                                        |
| ?1003 | X11 Mouse — Any Event            | ⬜     | **Parsed but silently swallowed**                                                                                                                        |
| ?1004 | Focus Reporting                  | ⬜     | **Parsed but silently swallowed**                                                                                                                        |
| ?1006 | SGR Mouse — Extended Coordinates | ⬜     | **Parsed but silently swallowed**                                                                                                                        |
| ?1047 | Alt Screen Buffer (legacy)       | ⬜     | **Not parsed** — only ?1049 is implemented                                                                                                               |
| ?1048 | Save/Restore Cursor (legacy)     | ⬜     | **Not parsed**                                                                                                                                           |
| ?1049 | Alt Screen Buffer + Save Cursor  | ✅     | Implemented — swaps screen buffers                                                                                                                       |
| ?2004 | Bracketed Paste                  | ⬜     | **Parsed but silently swallowed** — TerminalModes never written. **Breaks paste in shells/editors.**                                                     |
| ?2026 | Synchronized Output              | ⬜     | **Parsed but silently swallowed** — TerminalModes never written                                                                                          |

### Not Even Parsed

These DEC private modes are not recognized by the parser at all:

- ?2 (DECANM), ?66 (DECNKM), ?67 (DECBKM), ?69 (DECLRMM), ?1001, ?1007, ?1034

---

## Standard Modes (CSI Pm h / l)

| Ps  | Name                          | Status | Notes           |
| --- | ----------------------------- | ------ | --------------- |
| 4   | IRM — Insert/Replace Mode     | ⬜     | Not implemented |
| 12  | SRM — Send/Receive Mode       | ⬜     | Not implemented |
| 20  | LNM — Line Feed/New Line Mode | ✅     | Implemented     |

---

## FTCS — FinalTerm Control Sequences

| Sequence  | Name                  | Status | Notes                      |
| --------- | --------------------- | ------ | -------------------------- |
| OSC 133 A | Prompt Start          | ⬜     | Recognized, debug log only |
| OSC 133 B | Prompt End            | ⬜     | Recognized, debug log only |
| OSC 133 C | Pre-execution (input) | ⬜     | Recognized, debug log only |
| OSC 133 D | Command Finished      | ⬜     | Recognized, debug log only |

---

## Specification Coverage Summary

| Category                       | Freminal Status | Common in VT/xterm | Notes                                                                                  |
| ------------------------------ | --------------- | ------------------ | -------------------------------------------------------------------------------------- |
| Core C0/C1                     | 🚧              | ✅                 | BEL/BS/LF/CR/ESC work; **HT, VT, FF, NUL, DEL missing**                                |
| ESC                            | 🚧              | ✅                 | Save/restore cursor, IND, NEL, RI work; DECPAM/DECPNM/RIS are stubs; HTS not parsed    |
| CSI Cursor + Erase             | ✅              | ✅                 | CUU/CUD/CUF/CUB/CHA/CUP/ED/EL all correct                                              |
| CSI Edit (IL/DL/DCH)           | 🚧              | ✅                 | IL and DCH work; **DL not wired**; ICH works                                           |
| CSI Scroll (SU/SD)             | ⬜              | ✅                 | **Not implemented**                                                                    |
| Tab Stops (HT/HTS/TBC/CHT/CBT) | ⬜              | ✅                 | **Entirely missing** — no tab stop infrastructure                                      |
| SGR (Colors/Attrs)             | ✅              | ✅                 | 256 + TrueColor supported                                                              |
| OSC 0/2 (Title)                | ✅              | ✅                 | Implemented                                                                            |
| OSC 8 (Hyperlink)              | ✅              | ✅                 | Fully implemented                                                                      |
| OSC 52 (Clipboard)             | ⬜              | ✅                 | Not implemented                                                                        |
| Mouse Tracking                 | ⬜              | ✅                 | **Parsed but silently swallowed** — TerminalModes never written                        |
| Bracketed Paste                | ⬜              | ✅                 | **Parsed but silently swallowed** — TerminalModes never written                        |
| DSR/DA Queries                 | 🚧              | ✅                 | DA1/DA2 work; DSR has Ps-ignoring bug                                                  |
| DECSET Modes                   | 🚧              | ✅                 | Only DECAWM, XtCBlink, DECTCEM, ?1049 actually work. **14+ modes silently swallowed.** |
| FTCS                           | ⬜              | ⬜                 | Recognized but no functional effect                                                    |
| Sixel Graphics                 | ⬜              | 🚧                 | Not implemented                                                                        |
| DCS Sub-commands               | ⬜              | 🚧                 | Captured as bytes but no parsing (DECRQSS, XTGETTCAP)                                  |

---

## References

- [SGR.md](./SGR.md) — Detailed SGR attribute coverage
- [SUPPORTED_CONTROL_CODES.md](./SUPPORTED_CONTROL_CODES.md) — Raw control code listing
- [ESCAPE_SEQUENCE_GAPS.md](./ESCAPE_SEQUENCE_GAPS.md) — Gap analysis and roadmap
- [PLAN_07_ESCAPE_SEQUENCES.md](./PLAN_07_ESCAPE_SEQUENCES.md) — Implementation plan for fixing all gaps

---

## Next Steps

See `PLAN_07_ESCAPE_SEQUENCES.md` for the full prioritized implementation plan. Summary:

1. **Priority 1 (Critical):** Fix DECSTBM double-decrement, wire DL, implement tab stops, handle VT/FF/NUL/DEL, add CNL/CPL.
2. **Priority 2 (Breaks real apps):** Wire DECCKM, bracketed paste, mouse tracking, focus events to TerminalModes. Implement SU/SD. Fix DSR. Implement RIS/DECPAM/DECPNM properly.
3. **Priority 3 (Modern features):** OSC 52 clipboard, OSC 7 CWD, OSC 133 shell integration, HTS/TBC/CHT/CBT, DECRQSS, XTGETTCAP.
4. **Priority 4 (Polish):** CSI s/u cursor save/restore, REP, HPA, OSC palette, DECALN, legacy alt screen variants.

---

© 2025 Freminal Project. Licensed under MIT.
