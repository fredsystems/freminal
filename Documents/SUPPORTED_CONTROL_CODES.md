# Supported Control Codes

Last updated: 2026-03-12 — Rewritten post-Task 7 completion (all 30 subtasks done)

## Key

- ✅ — Implemented and working correctly
- 🚧 — Partially implemented / state tracked but no full effect (see notes)
- ⬜ — Not implemented
- ❌ — Will not be implemented

## C0 Control Characters

| Code       | Name            | Implemented | Notes                                                            |
| ---------- | --------------- | ----------- | ---------------------------------------------------------------- |
| NUL (0x00) | Null            | ✅          | Silently ignored                                                 |
| BEL (0x07) | Bell            | 🚧          | Emits `TerminalOutput::Bell`; no audio/visual bell in GUI        |
| BS (0x08)  | Backspace       | ✅          | Moves cursor left one cell                                       |
| HT (0x09)  | Horizontal Tab  | ✅          | Advances to next 8-column tab stop; full tab stop infrastructure |
| LF (0x0A)  | Line Feed       | ✅          | Moves cursor down one line                                       |
| VT (0x0B)  | Vertical Tab    | ✅          | Treated as LF per VT spec                                        |
| FF (0x0C)  | Form Feed       | ✅          | Treated as LF per VT spec                                        |
| CR (0x0D)  | Carriage Return | ✅          | Moves cursor to column 0                                         |
| SO (0x0E)  | Shift Out       | 🚧          | Parsed; selects G1 into GL — G1 rendering not implemented        |
| SI (0x0F)  | Shift In        | 🚧          | Parsed; restores G0 — no effect since G1 rendering is absent     |
| ESC (0x1B) | Escape          | ✅          | Introduces escape sequences                                      |
| DEL (0x7F) | Delete          | ✅          | Silently ignored                                                 |

## C1 (8-bit) Control Characters

| Control Code | Name                              | Implemented | Notes                                                                   |
| ------------ | --------------------------------- | ----------- | ----------------------------------------------------------------------- |
| ESC D        | Index (IND)                       | ✅          | Moves cursor down, scrolls at bottom                                    |
| ESC E        | Next Line (NEL)                   | ✅          | CR + LF combined                                                        |
| ESC H        | Tab Set (HTS)                     | ✅          | Sets tab stop at current cursor column                                  |
| ESC M        | Reverse Index (RI)                | ✅          | Scrolls up one line — fully implemented                                 |
| ESC N        | Single Shift Select of G2 Charset | ❌          |                                                                         |
| ESC O        | Single Shift Select of G3 Charset | ❌          |                                                                         |
| ESC P        | Device Control String (DCS)       | ✅          | Sub-command dispatch: DECRQSS (`$q`) and XTGETTCAP (`+q`) fully handled |
| ESC V        | Start of Guarded Area             | ❌          |                                                                         |
| ESC W        | End of Guarded Area               | ❌          |                                                                         |
| ESC X        | Start of String                   | ❌          |                                                                         |
| ESC Z        | Return of Terminal ID (DECID)     | ⬜          | Not parsed                                                              |
| ESC \        | String Terminator (ST)            | ✅          | Terminates DCS/OSC/APC strings                                          |
| ESC [        | Control Sequence Introducer (CSI) | ✅          | Delegated to CSI parser                                                 |
| ESC ]        | Operating System Command (OSC)    | ✅          | Delegated to OSC parser                                                 |
| ESC ^        | Privacy Message (PM)              | ❌          | Not implemented                                                         |
| ESC \_       | Application Program Command (APC) | 🚧          | Captured as opaque bytes; no sub-command parsing                        |

## Standard Escape Codes

| Control Code | Name                   | Description                                                                               | Implemented |
| ------------ | ---------------------- | ----------------------------------------------------------------------------------------- | ----------- |
| ESC SP F     | 7 Bit Control          |                                                                                           | ❌          |
| ESC SP G     | 8 Bit Control          |                                                                                           | ❌          |
| ESC SP L     | Ansi Conformance Level | Level 1                                                                                   | ❌          |
| ESC SP M     | Ansi Conformance Level | Level 2                                                                                   | ❌          |
| ESC SP N     | Ansi Conformance Level | Level 3                                                                                   | ❌          |
| ESC # 3      | DECDHL                 | Double Line Height, Top Half — parsed, renderer ignores                                   | 🚧          |
| ESC # 4      | DECDHL                 | Double Line Height, Bottom Half — parsed, renderer ignores                                | 🚧          |
| ESC # 5      | DECSWL                 | Single Width Line — parsed                                                                | 🚧          |
| ESC # 6      | DECDWL                 | Double Width Line — parsed, renderer ignores                                              | 🚧          |
| ESC # 8      | DECALN                 | Screen Alignment Test — fills screen with 'E', resets cursor and scroll region            | ✅          |
| ESC % @      | Character Set          | Default Character Set                                                                     | ❌          |
| ESC % G      | Character Set          | UTF Character Set                                                                         | ❌          |
| ESC ( 0      | Character Set          | G0 — DEC Special Graphics (line drawing)                                                  | ✅          |
| ESC ( B      | Character Set          | G0 — US ASCII                                                                             | ✅          |
| ESC ( C      | Character Set          | G0 — other charsets                                                                       | ⬜          |
| ESC ) C      | Character Set          | G1 Character Set                                                                          | ❌          |
| ESC \* C     | Character Set          | G2 Character Set                                                                          | ❌          |
| ESC + C      | Character Set          | Where `C` is a charset defined at [xfree86](https://www.xfree86.org/current/ctlseqs.html) | ❌          |
| ESC 7        | Save Cursor (DECSC)    | Saves cursor position and attributes                                                      | ✅          |
| ESC 8        | Restore Cursor (DECRC) | Restores saved cursor                                                                     | ✅          |
| ESC =        | DECPAM                 | Application Keypad Mode — sets `TerminalModes.keypad_mode`                                | ✅          |
| ESC >        | DECPNM                 | Numeric Keypad Mode — sets `TerminalModes.keypad_mode`                                    | ✅          |
| ESC c        | RIS                    | Full reset — resets buffer, cursor, modes, tab stops, scroll region                       | ✅          |
| ESC F        |                        | Cursor to lower left of screen — parsed with debug log, no effect                         | 🚧          |
| ESC l        | Memory Lock            | Parsed with debug log, no effect                                                          | 🚧          |
| ESC m        | Memory Unlock          | Parsed with debug log, no effect                                                          | 🚧          |
| ESC n        | Character Set          | Invoke the G2 character set as GL — parsed, no effect                                     | 🚧          |
| ESC o        | Character Set          | Invoke the G3 character set as GL — parsed, no effect                                     | 🚧          |
| ESC \|       | Character Set          | Invoke the G3 character set as GR — parsed, no effect                                     | 🚧          |
| ESC }        | Character Set          | Invoke the G2 character set as GR — parsed, no effect                                     | 🚧          |
| ESC ~        | Character Set          | Invoke the G1 character set as GR — parsed, no effect                                     | 🚧          |

## CSI Control Codes

| Control Code  | Name     | Description                                              | Implemented |
| ------------- | -------- | -------------------------------------------------------- | ----------- |
| CSI Ps A      | CUU      | Cursor Up [Ps] (default = 1)                             | ✅          |
| CSI Ps B      | CUD      | Cursor Down [Ps] (default = 1)                           | ✅          |
| CSI Ps C      | CUF      | Cursor Forward [Ps] (default = 1)                        | ✅          |
| CSI Ps D      | CUB      | Cursor Backward [Ps] (default = 1)                       | ✅          |
| CSI Ps E      | CNL      | Cursor Next Line [Ps] (default = 1)                      | ✅          |
| CSI Ps F      | CPL      | Cursor Previous Line [Ps] (default = 1)                  | ✅          |
| CSI Ps G      | CHA      | Cursor Horizontal Absolute [column] (default = 1)        | ✅          |
| CSI Ps ; Ps H | CUP      | Cursor Position [row;col] (default = [1,1])              | ✅          |
| CSI Ps I      | CHT      | Cursor Horizontal Forward Tab [Ps] (default = 1)         | ✅          |
| CSI Ps J      | ED       | Erase in Display (0=end, 1=begin, 2=all, 3=saved)        | ✅          |
| CSI Ps K      | EL       | Erase in Line (0=end, 1=begin, 2=all)                    | ✅          |
| CSI Ps L      | IL       | Insert Lines [Ps] (default = 1)                          | ✅          |
| CSI Ps M      | DL       | Delete Lines [Ps] (default = 1)                          | ✅          |
| CSI Ps P      | DCH      | Delete Characters [Ps] (default = 1)                     | ✅          |
| CSI Ps S      | SU       | Scroll Up [Ps] (default = 1)                             | ✅          |
| CSI Ps T      | SD       | Scroll Down [Ps] (default = 1)                           | ✅          |
| CSI Ps X      | ECH      | Erase Characters [Ps] (default = 1)                      | ✅          |
| CSI Ps Z      | CBT      | Cursor Backward Tab [Ps] (default = 1)                   | ✅          |
| CSI Ps @      | ICH      | Insert Characters [Ps] (default = 1)                     | ✅          |
| CSI Ps \`     | HPA      | Horizontal Position Absolute [column] — alias for CHA    | ✅          |
| CSI Ps b      | REP      | Repeat Preceding Character [Ps] times                    | ✅          |
| CSI Ps c      | DA1      | Primary Device Attributes                                | ✅          |
| CSI > Ps c    | DA2      | Secondary Device Attributes                              | ✅          |
| CSI Ps d      | VPA      | Vertical Position Absolute [row]                         | ✅          |
| CSI Ps ; Ps f | HVP      | Horizontal Vertical Position [row;col] — same as CUP     | ✅          |
| CSI Ps g      | TBC      | Tab Clear (0=current column, 3=all)                      | ✅          |
| CSI Ps h      | SM       | Set Mode — LNM (20) implemented; IRM (4), SRM (12) not   | 🚧          |
| CSI Ps l      | RM       | Reset Mode — same as SM                                  | 🚧          |
| CSI Ps m      | SGR      | Select Graphic Rendition ([SGR.md](./SGR.md))            | ✅          |
| CSI Ps n      | DSR      | Device Status Report — Ps=5 status, Ps=6 cursor position | ✅          |
| CSI Ps > q    | XTVER    | XTVERSION query                                          | ✅          |
| CSI Ps SP q   | DECSCUSR | Set Cursor Style                                         | ✅          |
| CSI Ps ; Ps r | DECSTBM  | Set Scrolling Margins (top;bottom)                       | ✅          |
| CSI s         | SCOSC    | Save Cursor Position                                     | ✅          |
| CSI Ps t      | Window   | Window Manipulation                                      | ✅          |
| CSI u         | SCORC    | Restore Cursor Position                                  | ✅          |
| CSI ? Pm h    | DECSET   | Set DEC Private Mode                                     | ✅          |
| CSI ? Pm l    | DECRST   | Reset DEC Private Mode                                   | ✅          |
| CSI ? Pm $p   | DECRQM   | Request Mode — full mode query via mode-sync loop        | ✅          |

## DEC Private Modes (CSI ? Pm h / l)

| Mode  | Name                             | Implemented | Notes                                                                               |
| ----- | -------------------------------- | ----------- | ----------------------------------------------------------------------------------- |
| ?1    | DECCKM — Cursor Keys Mode        | ✅          | `TerminalModes.cursor_key`; GUI translates arrow keys when set                      |
| ?3    | DECCOLM — 80/132 Column Mode     | ✅          | Column switching active when AllowColumnModeSwitch (?40) is enabled                 |
| ?5    | DECSCNM — Reverse Video          | 🚧          | `TerminalModes.invert_screen` set correctly; renderer screen-inversion not yet done |
| ?6    | DECOM — Origin Mode              | ✅          | CUP row 1 → top of scroll region when set                                           |
| ?7    | DECAWM — Auto Wrap Mode          | ✅          | Implemented (`Decawm` enum)                                                         |
| ?8    | DECARM — Auto Repeat Keys        | ✅          | `TerminalModes.repeat_keys`; GUI reads                                              |
| ?12   | XtCBlink — Cursor Blink          | ✅          | Implemented                                                                         |
| ?25   | DECTCEM — Show/Hide Cursor       | ✅          | Implemented                                                                         |
| ?40   | AllowColumnModeSwitch            | ✅          | Gates DECCOLM behavior                                                              |
| ?45   | ReverseWrapAround                | ✅          | `TerminalModes.reverse_wrap_around`                                                 |
| ?47   | Alt Screen Buffer (legacy)       | ✅          | Wired to same alt-screen machinery as ?1049                                         |
| ?1000 | X11 Mouse — Normal Tracking      | ✅          | `TerminalModes.mouse_tracking`; GUI reads and forwards mouse events                 |
| ?1002 | X11 Mouse — Button Event         | ✅          | `TerminalModes.mouse_tracking`                                                      |
| ?1003 | X11 Mouse — Any Event            | ✅          | `TerminalModes.mouse_tracking`                                                      |
| ?1004 | Focus Reporting                  | ✅          | `TerminalModes.focus_reporting`; GUI sends `InputEvent::FocusChange`                |
| ?1006 | SGR Mouse — Extended Coordinates | ✅          | `TerminalModes.mouse_tracking`                                                      |
| ?1047 | Alt Screen Buffer (legacy)       | ✅          | Wired to same alt-screen machinery as ?1049                                         |
| ?1048 | Save/Restore Cursor (legacy)     | ✅          | Wired to existing save/restore cursor machinery                                     |
| ?1049 | Alt Screen Buffer + Save Cursor  | ✅          | Implemented — swaps screen buffers, saves/restores cursor                           |
| ?2004 | Bracketed Paste                  | ✅          | `TerminalModes.bracketed_paste`; GUI wraps paste with `\e[200~` / `\e[201~`         |
| ?2026 | Synchronized Output              | ✅          | `TerminalModes.synchronized_updates`                                                |

### Not Parsed

| Mode          | Description                    |
| ------------- | ------------------------------ |
| ?2 (DECANM)   | VT52 mode                      |
| ?66 (DECNKM)  | Numeric keypad mode (DEC)      |
| ?67 (DECBKM)  | Backarrow key mode             |
| ?69 (DECLRMM) | Left/right margin mode         |
| ?1001, ?1007  | Hilite mouse, alternate scroll |
| ?1034         | Interpret meta key             |

## Standard Modes (CSI Pm h / l)

| Ps  | Name                          | Implemented | Notes           |
| --- | ----------------------------- | ----------- | --------------- |
| 4   | IRM — Insert/Replace Mode     | ⬜          | Not implemented |
| 12  | SRM — Send/Receive Mode       | ⬜          | Not implemented |
| 20  | LNM — Line Feed/New Line Mode | ✅          | Implemented     |

## OSC — Operating System Commands

| Sequence                 | Purpose                       | Implemented | Notes                                                   |
| ------------------------ | ----------------------------- | ----------- | ------------------------------------------------------- |
| OSC 0 ; txt BEL          | Set icon + window title       | 🚧          | Works; icon name vs. title not distinguished            |
| OSC 1 ; txt BEL          | Set icon title                | 🚧          | Shares handler with OSC 0 (treated as full title)       |
| OSC 2 ; txt BEL          | Set window title              | ✅          | Implemented                                             |
| OSC 4 ; n ; rgb BEL      | Set palette entry             | ✅          | Sets 256-color palette entry; query responds with value |
| OSC 7 ; URI BEL          | Current Working Directory     | ✅          | Stored in `TerminalHandler.current_working_directory`   |
| OSC 8 ; params ; URI BEL | Hyperlink                     | ✅          | Fully implemented — hyperlink start/end with URL        |
| OSC 10 ; ? BEL           | Foreground color query/set    | 🚧          | Query → hardcoded Catppuccin default; set is a no-op    |
| OSC 11 ; ? BEL           | Background color query/set    | 🚧          | Query → hardcoded Catppuccin default; set is a no-op    |
| OSC 12 ; color BEL       | Set cursor color              | ⬜          | Not implemented                                         |
| OSC 52 ; c ; data BEL    | Clipboard copy/paste          | ✅          | Clipboard set/query forwarded via `WindowManipulation`  |
| OSC 104 BEL              | Reset palette entry (or all)  | ✅          | Resets specific or all palette entries to defaults      |
| OSC 110 BEL              | Reset foreground color        | ⬜          | Not implemented                                         |
| OSC 111 BEL              | Reset background color        | ⬜          | Not implemented                                         |
| OSC 112 BEL              | Reset cursor color            | ⬜          | Empty match arm, no-op                                  |
| OSC 133 ; … BEL          | FTCS / Shell Integration      | ✅          | All four markers (A/B/C/D) parsed and stored            |
| OSC 777                  | System notification (Konsole) | ⬜          | Not implemented                                         |
| OSC 1337                 | iTerm2 / WezTerm extensions   | ⬜          | Recognized with debug log; no sub-command dispatch      |

## DCS — Device Control String

| Sequence     | Name      | Implemented | Notes                                                                       |
| ------------ | --------- | ----------- | --------------------------------------------------------------------------- |
| DCS (all)    | General   | ✅          | Sub-command dispatch via `handle_device_control_string()`                   |
| DCS $ q … ST | DECRQSS   | ✅          | Supports `m` (SGR), `r` (DECSTBM), `q` (DECSCUSR); unknown → error response |
| DCS + q … ST | XTGETTCAP | ✅          | Responds to common capability queries; unknown → error response             |
| DCS Sixel    | Sixel     | ⬜          | Not implemented                                                             |

## FTCS — FinalTerm Control Sequences (OSC 133)

| Sequence  | Name                  | Implemented | Notes                                        |
| --------- | --------------------- | ----------- | -------------------------------------------- |
| OSC 133 A | Prompt Start          | ✅          | Parsed and stored in `FtcsState`             |
| OSC 133 B | Prompt End            | ✅          | Parsed and stored in `FtcsState`             |
| OSC 133 C | Pre-execution (input) | ✅          | Parsed and stored in `FtcsState`             |
| OSC 133 D | Command Finished      | ✅          | Parsed; exit code stored in `last_exit_code` |
