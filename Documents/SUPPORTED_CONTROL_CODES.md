# Supported Control Codes

Last updated: 2026-03-09 — Corrected via comprehensive codebase audit (Task 7)

## Key

- ⬜️ - Not implemented yet
- ✅ - Implemented
- 🚧 - Partially implemented / stub with warn log
- 🐛 - Implemented but has known bug
- ❌ - Will not be implemented

## C0 Control Characters

| Code       | Name            | Implemented | Notes                                         |
| ---------- | --------------- | ----------- | --------------------------------------------- |
| NUL (0x00) | Null            | ⬜          | Should be silently ignored; included in data  |
| BEL (0x07) | Bell            | ✅          | Emits `TerminalOutput::Bell`                  |
| BS (0x08)  | Backspace       | ✅          | Moves cursor left one cell                    |
| HT (0x09)  | Horizontal Tab  | ⬜          | Not handled as C0; no tab stop infrastructure |
| LF (0x0A)  | Line Feed       | ✅          | Moves cursor down one line                    |
| VT (0x0B)  | Vertical Tab    | ⬜          | Should act as LF; falls through as data       |
| FF (0x0C)  | Form Feed       | ⬜          | Should act as LF; falls through as data       |
| CR (0x0D)  | Carriage Return | ✅          | Moves cursor to column 0                      |
| SO (0x0E)  | Shift Out       | ⬜          | G1 charset switching not implemented          |
| SI (0x0F)  | Shift In        | ⬜          | G0 charset switching not implemented          |
| ESC (0x1B) | Escape          | ✅          | Introduces escape sequences                   |
| DEL (0x7F) | Delete          | ⬜          | Should be silently ignored; not handled       |

## C1 (8 Bit) Control Characters

| Control Code | Name                              | Implemented | Notes                                           |
| ------------ | --------------------------------- | ----------- | ----------------------------------------------- |
| ESC D        | Index (IND)                       | ✅          | Moves cursor down, scrolls at bottom            |
| ESC E        | Next Line (NEL)                   | ✅          | CR + LF combined                                |
| ESC H        | Tab Set (HTS)                     | ⬜          | Not parsed; no tab stop infrastructure          |
| ESC M        | Reverse Index (RI)                | ✅          | Scrolls up one line — fully implemented         |
| ESC N        | Single Shift Select of G2 Charset | ❌          |                                                 |
| ESC O        | Single Shift Select of G3 Charset | ❌          |                                                 |
| ESC P        | Device Control String (DCS)       | 🚧          | Captures bytes, debug log only, no sub-commands |
| ESC V        | Start of Guarded Area             | ❌          |                                                 |
| ESC W        | End of Guarded Area               | ❌          |                                                 |
| ESC X        | Start of String                   | ❌          |                                                 |
| ESC Z        | Return of Terminal ID (DECID)     | ⬜          | Not parsed                                      |
| ESC \        | String Terminator (ST)            | ✅          | Terminates DCS/OSC/APC strings                  |
| ESC [        | Control Sequence Introducer (CSI) | ✅          | Delegated to CSI parser                         |
| ESC ]        | Operating System Command (OSC)    | ✅          | Delegated to OSC parser                         |
| ESC ^        | Privacy Message (PM)              | ❌          | Not implemented                                 |
| ESC \_       | Application Program Command (APC) | 🚧          | Captured as opaque bytes                        |

## Standard Escape Codes

| Control Code | Name                   | Description                                                                                | Implemented |
| ------------ | ---------------------- | ------------------------------------------------------------------------------------------ | ----------- |
| ESC SP F     | 7 Bit Control          |                                                                                            | ❌          |
| ESC SP G     | 8 Bit Control          |                                                                                            | ❌          |
| ESC SP L     | Ansi Conformance Level | Level 1                                                                                    | ❌          |
| ESC SP M     | Ansi Conformance Level | Level 2                                                                                    | ❌          |
| ESC SP N     | Ansi Conformance Level | Level 3                                                                                    | ❌          |
| ESC # 3      | DECDHL                 | Double Line Height, Top Half                                                               | ⬜          |
| ESC # 4      | DECDHL                 | Double Line Height, Bottom Half                                                            | ⬜          |
| ESC # 5      | DECSWL                 | Single Width Line                                                                          | ⬜          |
| ESC # 6      | DECDWL                 | Double Width Line                                                                          | ⬜          |
| ESC # 8      | DECALN                 | Screen Alignment Test                                                                      | 🚧          |
| ESC % @      | Character Set          | Default Character Set                                                                      | ❌          |
| ESC % G      | Character Set          | UTF Character Set                                                                          | ❌          |
| ESC ( 0      | Character Set          | G0 — DEC Special Graphics (line drawing)                                                   | ✅          |
| ESC ( B      | Character Set          | G0 — US ASCII                                                                              | ✅          |
| ESC ( C      | Character Set          | G0 — other charsets                                                                        | ⬜          |
| ESC ) C      | Character Set          | G1 Character Set                                                                           | ❌          |
| ESC \* C     | Character Set          | G2 Character Set                                                                           | ❌          |
| ESC + C      | Character Set          | Where `C` is a charset defined at [xfreeorg](https://www.xfree86.org/current/ctlseqs.html) | ❌          |
| ESC 7        | Save Cursor (DECSC)    | Saves cursor position and attributes                                                       | ✅          |
| ESC 8        | Restore Cursor (DECRC) | Restores saved cursor                                                                      | ✅          |
| ESC =        | DECPAM                 | Application Keypad Mode                                                                    | 🚧          |
| ESC >        | DECPNM                 | Numeric Keypad Mode                                                                        | 🚧          |
| ESC F        |                        | Cursor to lower left of screen                                                             | 🚧          |
| ESC c        | RIS                    | Full reset — stub only, does not actually reset state                                      | 🚧          |
| ESC l        | Memory Lock            | Parsed with `warn!` log                                                                    | 🚧          |
| ESC m        | Memory Unlock          | Parsed with `warn!` log                                                                    | 🚧          |
| ESC n        | Character Set          | Invoke the G2 character set as GL                                                          | 🚧          |
| ESC o        | Character Set          | Invoke the G3 character set as GL                                                          | 🚧          |
| ESC \|       | Character Set          | Invoke the G3 character set as GR                                                          | 🚧          |
| ESC }        | Character Set          | Invoke the G2 character set as GR                                                          | 🚧          |
| ESC ~        | Character Set          | Invoke the G1 character set as GR                                                          | 🚧          |

## CSI Control Codes

| Control Code  | Name     | Description                                       | Implemented |
| ------------- | -------- | ------------------------------------------------- | ----------- |
| CSI Ps A      | CUU      | Cursor Up [Ps] (default = 1)                      | ✅          |
| CSI Ps B      | CUD      | Cursor Down [Ps] (default = 1)                    | ✅          |
| CSI Ps C      | CUF      | Cursor Forward [Ps] (default = 1)                 | ✅          |
| CSI Ps D      | CUB      | Cursor Backward [Ps] (default = 1)                | ✅          |
| CSI Ps E      | CNL      | Cursor Next Line [Ps] (default = 1)               | ⬜          |
| CSI Ps F      | CPL      | Cursor Previous Line [Ps] (default = 1)           | ⬜          |
| CSI Ps G      | CHA      | Cursor Horizontal Absolute [column] (default = 1) | ✅          |
| CSI Ps ; Ps H | CUP      | Cursor Position [row;col] (default = [1,1])       | ✅          |
| CSI Ps I      | CHT      | Cursor Horizontal Forward Tab [Ps] (default = 1)  | ⬜          |
| CSI Ps J      | ED       | Erase in Display (0=end, 1=begin, 2=all, 3=saved) | ✅          |
| CSI Ps K      | EL       | Erase in Line (0=end, 1=begin, 2=all)             | ✅          |
| CSI Ps L      | IL       | Insert Lines [Ps] (default = 1)                   | ✅          |
| CSI Ps M      | DL       | Delete Lines [Ps] (default = 1)                   | 🐛          |
| CSI Ps P      | DCH      | Delete Characters [Ps] (default = 1)              | ✅          |
| CSI Ps S      | SU       | Scroll Up [Ps] (default = 1)                      | ⬜          |
| CSI Ps T      | SD       | Scroll Down [Ps] (default = 1)                    | ⬜          |
| CSI Ps X      | ECH      | Erase Characters [Ps] (default = 1)               | ✅          |
| CSI Ps Z      | CBT      | Cursor Backward Tab [Ps] (default = 1)            | ⬜          |
| CSI Ps @      | ICH      | Insert Characters [Ps] (default = 1)              | ✅          |
| CSI Ps \`     | HPA      | Horizontal Position Absolute [column]             | ⬜          |
| CSI Ps b      | REP      | Repeat Preceding Character [Ps] times             | ⬜          |
| CSI Ps c      | DA1      | Primary Device Attributes                         | ✅          |
| CSI > Ps c    | DA2      | Secondary Device Attributes                       | ✅          |
| CSI Ps d      | VPA      | Vertical Position Absolute [row]                  | ✅          |
| CSI Ps ; Ps f | HVP      | Horizontal Vertical Position [row;col]            | ✅          |
| CSI Ps g      | TBC      | Tab Clear (0=current, 3=all)                      | ⬜          |
| CSI Ps h      | SM       | Set Mode — only LNM (20) implemented              | 🚧          |
| CSI Ps l      | RM       | Reset Mode — only LNM (20) implemented            | 🚧          |
| CSI Ps m      | SGR      | Select Graphic Rendition ([SGR.md](./SGR.md))     | ✅          |
| CSI Ps n      | DSR      | Device Status Report                              | 🐛          |
| CSI Ps > q    | XTVER    | XTVERSION query                                   | ✅          |
| CSI Ps SP q   | DECSCUSR | Set Cursor Style                                  | ✅          |
| CSI Ps ; Ps r | DECSTBM  | Set Scrolling Margins (top;bottom)                | 🐛          |
| CSI s         | SCOSC    | Save Cursor Position                              | ⬜          |
| CSI Ps t      | Window   | Window Manipulation                               | ✅          |
| CSI u         | SCORC    | Restore Cursor Position                           | 🐛          |
| CSI ? Pm h    | DECSET   | Set DEC Private Mode                              | 🚧          |
| CSI ? Pm l    | DECRST   | Reset DEC Private Mode                            | 🚧          |
| CSI ? Pm $p   | DECRQM   | Request Mode                                      | 🚧          |

## DEC Private Modes (CSI ? Pm h / l)

| Mode  | Name     | Description                     | Implemented |
| ----- | -------- | ------------------------------- | ----------- |
| ?1    | DECCKM   | Cursor Keys Mode                | ⬜          |
| ?3    | DECCOLM  | 80/132 Column Mode              | ⬜          |
| ?5    | DECSCNM  | Reverse Video                   | ⬜          |
| ?6    | DECOM    | Origin Mode                     | ⬜          |
| ?7    | DECAWM   | Auto Wrap Mode                  | ✅          |
| ?12   | XtCBlink | Cursor Blink                    | ✅          |
| ?25   | DECTCEM  | Show/Hide Cursor                | ✅          |
| ?47   | AltBuf   | Alt Screen Buffer (legacy)      | ⬜          |
| ?1000 | Mouse    | X11 Normal Tracking             | ⬜          |
| ?1002 | Mouse    | X11 Button Event Tracking       | ⬜          |
| ?1003 | Mouse    | X11 Any Event Tracking          | ⬜          |
| ?1004 | Focus    | Focus Reporting                 | ⬜          |
| ?1006 | Mouse    | SGR Extended Coordinates        | ⬜          |
| ?1047 | AltBuf   | Alt Screen Buffer (legacy)      | ⬜          |
| ?1048 | Cursor   | Save/Restore Cursor (legacy)    | ⬜          |
| ?1049 | AltBuf   | Alt Screen Buffer + Save Cursor | ✅          |
| ?2004 | BPaste   | Bracketed Paste                 | ⬜          |
| ?2026 | SyncUpd  | Synchronized Output             | ⬜          |

## OSC — Operating System Commands

| Sequence                 | Purpose                   | Implemented | Notes                                 |
| ------------------------ | ------------------------- | ----------- | ------------------------------------- |
| OSC 0 ; txt BEL          | Set icon + window title   | 🚧          | No icon/title distinction             |
| OSC 1 ; txt BEL          | Set icon title            | 🚧          | Treated as full title                 |
| OSC 2 ; txt BEL          | Set window title          | ✅          |                                       |
| OSC 4 ; n ; rgb          | Set palette entry         | ⬜          |                                       |
| OSC 7 ; URI              | Current Working Directory | ⬜          | Recognized, debug log only            |
| OSC 8 ; params ; URI BEL | Hyperlink                 | ✅          | Fully implemented                     |
| OSC 10 ; ? BEL           | Foreground color query    | 🚧          | Query works (hardcoded), set is no-op |
| OSC 11 ; ? BEL           | Background color query    | 🚧          | Query works (hardcoded), set is no-op |
| OSC 12 ; color           | Set cursor color          | ⬜          |                                       |
| OSC 52 ; c ; data BEL    | Clipboard copy/paste      | ⬜          |                                       |
| OSC 104                  | Reset palette entry       | ⬜          |                                       |
| OSC 110                  | Reset foreground color    | ⬜          |                                       |
| OSC 111                  | Reset background color    | ⬜          |                                       |
| OSC 112                  | Reset cursor color        | ⬜          | Empty match arm, no-op                |
| OSC 133 ; …              | FTCS / Shell Integration  | ⬜          | Recognized, debug log only            |
| OSC 777                  | System notification       | ⬜          |                                       |
| OSC 1337                 | iTerm2 / WezTerm          | ⬜          | Recognized, debug log only            |

## DCS — Device Control String

| Sequence      | Name      | Implemented | Notes                        |
| ------------- | --------- | ----------- | ---------------------------- |
| DCS (general) |           | 🚧          | Captured as bytes, debug log |
| DCS $ q … ST  | DECRQSS   | ⬜          | No sub-command parsing       |
| DCS + q … ST  | XTGETTCAP | ⬜          | No sub-command parsing       |
| DCS Sixel     | Sixel     | ⬜          | Not implemented              |
