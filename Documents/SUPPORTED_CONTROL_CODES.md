# Supported Control Codes

Last updated: 2026-11-20 — Task 147: statuses re-checked against
`ESCAPE_SEQUENCE_COVERAGE.md` (character sets and SO/SI, SS2/SS3, the modes
KAM / SRM / `?1001` / `?1034` / `?1035` and the HP escapes), and the emoji status
cells replaced by the plain-text words COVERAGE uses.

Last updated: 2026-08-25 — Task 124.3a added the `?1016` (SGR-Pixels
Mouse) row and fixed its encoding to report one-based physical pixel
coordinates instead of cell coordinates.

Last updated: 2026-03-31 — Updated post-Task 20 completion (DEC Private Mode Coverage)

This is a quick-reference summary. [`ESCAPE_SEQUENCE_COVERAGE.md`](./ESCAPE_SEQUENCE_COVERAGE.md)
is the authoritative status table; where the two disagree, COVERAGE is right.

## Key

- `done` — Implemented and working correctly
- `partial` — Partially implemented, or state tracked with limited effect (see notes)
- `stub` — Parsed and logged or ignored, not functional
- `missing` — Not implemented
- `not planned` — Not implemented, and deliberately not going to be

## C0 Control Characters

| Code       | Name            | Status | Notes                                                                      |
| ---------- | --------------- | ------ | -------------------------------------------------------------------------- |
| NUL (0x00) | Null            | done   | Silently ignored                                                           |
| BEL (0x07) | Bell            | done   | Emits `TerminalOutput::Bell`; visual tab-bar flash + optional audible beep |
| BS (0x08)  | Backspace       | done   | Moves cursor left one cell                                                 |
| HT (0x09)  | Horizontal Tab  | done   | Advances to next 8-column tab stop; full tab stop infrastructure           |
| LF (0x0A)  | Line Feed       | done   | Moves cursor down one line                                                 |
| VT (0x0B)  | Vertical Tab    | done   | Treated as LF per VT spec                                                  |
| FF (0x0C)  | Form Feed       | done   | Treated as LF per VT spec                                                  |
| CR (0x0D)  | Carriage Return | done   | Moves cursor to column 0                                                   |
| SO (0x0E)  | Shift Out       | done   | Invokes G1 into GL (locking shift); ignored in VT52 mode                   |
| SI (0x0F)  | Shift In        | done   | Invokes G0 into GL (locking shift); ignored in VT52 mode                   |
| ESC (0x1B) | Escape          | done   | Introduces escape sequences                                                |
| DEL (0x7F) | Delete          | done   | Silently ignored                                                           |

## C1 (8-bit) Control Characters

| Control Code | Name                              | Status  | Notes                                                                                                  |
| ------------ | --------------------------------- | ------- | ------------------------------------------------------------------------------------------------------ |
| ESC D        | Index (IND)                       | done    | Moves cursor down, scrolls at bottom                                                                   |
| ESC E        | Next Line (NEL)                   | done    | CR + LF combined                                                                                       |
| ESC H        | Tab Set (HTS)                     | done    | Sets tab stop at current cursor column                                                                 |
| ESC M        | Reverse Index (RI)                | done    | Scrolls up one line — fully implemented                                                                |
| ESC N        | Single Shift 2 (SS2)              | done    | Maps the next printable character through G2, then clears; the 8-bit form `0x8E` acts only under S8C1T |
| ESC O        | Single Shift 3 (SS3)              | done    | Maps the next printable character through G3, then clears; the 8-bit form `0x8F` acts only under S8C1T |
| ESC P        | Device Control String (DCS)       | done    | Sub-command dispatch: DECRQSS (`$q`) and XTGETTCAP (`+q`) fully handled                                |
| ESC V        | Start of Guarded Area             | missing | Not implemented                                                                                        |
| ESC W        | End of Guarded Area               | missing | Not implemented                                                                                        |
| ESC X        | Start of String                   | missing | Not implemented                                                                                        |
| ESC Z        | Return of Terminal ID (DECID)     | done    | Emits a device-attributes request; VT52 mode answers `ESC / Z`                                         |
| ESC \        | String Terminator (ST)            | done    | Terminates DCS/OSC/APC strings                                                                         |
| ESC [        | Control Sequence Introducer (CSI) | done    | Delegated to CSI parser                                                                                |
| ESC ]        | Operating System Command (OSC)    | done    | Delegated to OSC parser                                                                                |
| ESC ^        | Privacy Message (PM)              | missing | Not implemented                                                                                        |
| ESC \_       | Application Program Command (APC) | done    | Full parser; `_G` dispatches to the kitty graphics handler, other APCs are logged and ignored          |

## Standard Escape Codes

| Control Code                | Name                    | Description                                                                                              | Status      |
| --------------------------- | ----------------------- | -------------------------------------------------------------------------------------------------------- | ----------- |
| ESC SP F                    | S7C1T — 7 Bit Controls  | Default mode                                                                                             | done        |
| ESC SP G                    | S8C1T — 8 Bit Controls  | Enables `0x9B` as CSI and `0x8E` / `0x8F` as SS2 / SS3; terminal replies then use 8-bit introducers      | done        |
| ESC SP L                    | Ansi Conformance Level  | Level 1 — parsed and warn-logged, no effect                                                              | stub        |
| ESC SP M                    | Ansi Conformance Level  | Level 2 — parsed and warn-logged, no effect                                                              | stub        |
| ESC SP N                    | Ansi Conformance Level  | Level 3 — parsed and warn-logged, no effect                                                              | stub        |
| ESC # 3                     | DECDHL                  | Double Line Height, Top Half — renderer scales the glyph and shows the top half                          | done        |
| ESC # 4                     | DECDHL                  | Double Line Height, Bottom Half — renderer scales the glyph and shows the bottom half                    | done        |
| ESC # 5                     | DECSWL                  | Single Width Line                                                                                        | done        |
| ESC # 6                     | DECDWL                  | Double Width Line — renderer applies 2× x-scale                                                          | done        |
| ESC # 8                     | DECALN                  | Screen Alignment Test — fills screen with 'E', resets cursor and scroll region                           | done        |
| ESC % @                     | Character Set           | Default Character Set                                                                                    | not planned |
| ESC % G                     | Character Set           | UTF Character Set                                                                                        | not planned |
| ESC ( 0                     | Character Set           | G0 — DEC Special Graphics (line drawing)                                                                 | done        |
| ESC ( B                     | Character Set           | G0 — US ASCII                                                                                            | done        |
| ESC ( C                     | Character Set           | G0 — any other final: ignored, the slot keeps its set (no NRC sets other than UK)                        | missing     |
| ESC ) C                     | Character Set           | G1 designator — `C` is `B` (ASCII), `A` (UK) or `0` (DEC Special Graphics)                               | done        |
| ESC \* C                    | Character Set           | G2 designator — `C` is `B` (ASCII), `A` (UK) or `0` (DEC Special Graphics)                               | done        |
| ESC + C                     | Character Set           | G3 designator — `C` is `B` (ASCII), `A` (UK) or `0` (DEC Special Graphics)                               | done        |
| ESC - C / ESC . C / ESC / C | Character Set           | G1 / G2 / G3 designators for 96-character sets                                                           | not planned |
| ESC 7                       | Save Cursor (DECSC)     | Saves cursor position and attributes                                                                     | done        |
| ESC 8                       | Restore Cursor (DECRC)  | Restores saved cursor                                                                                    | done        |
| ESC =                       | DECPAM                  | Application Keypad Mode — sets `TerminalModes.keypad_mode`                                               | done        |
| ESC >                       | DECPNM                  | Numeric Keypad Mode — sets `TerminalModes.keypad_mode`                                                   | done        |
| ESC c                       | RIS                     | Full reset — resets buffer, cursor, modes, tab stops, scroll region                                      | done        |
| ESC F                       | HP Cursor to lower left | Recognised and ignored with a `debug!` log (xterm acts only with `hpLowerleftBugCompat`, off by default) | not planned |
| ESC l                       | Memory Lock             | HP Memory Lock — recognised and ignored with a `debug!` log                                              | not planned |
| ESC m                       | Memory Unlock           | HP Memory Unlock — recognised and ignored with a `debug!` log                                            | not planned |
| ESC n                       | LS2                     | Invoke the G2 character set as GL (locking shift)                                                        | done        |
| ESC o                       | LS3                     | Invoke the G3 character set as GL (locking shift)                                                        | done        |
| ESC \|                      | LS3R                    | Invoke the G3 character set as GR — tracked, never applied (bytes `0x80` and above are UTF-8)            | partial     |
| ESC }                       | LS2R                    | Invoke the G2 character set as GR — tracked, never applied                                               | partial     |
| ESC ~                       | LS1R                    | Invoke the G1 character set as GR — tracked, never applied                                               | partial     |

## CSI Control Codes

| Control Code  | Name     | Description                                                                                                                  | Status  |
| ------------- | -------- | ---------------------------------------------------------------------------------------------------------------------------- | ------- |
| CSI Ps A      | CUU      | Cursor Up [Ps] (default = 1)                                                                                                 | done    |
| CSI Ps B      | CUD      | Cursor Down [Ps] (default = 1)                                                                                               | done    |
| CSI Ps C      | CUF      | Cursor Forward [Ps] (default = 1)                                                                                            | done    |
| CSI Ps D      | CUB      | Cursor Backward [Ps] (default = 1)                                                                                           | done    |
| CSI Ps E      | CNL      | Cursor Next Line [Ps] (default = 1)                                                                                          | done    |
| CSI Ps F      | CPL      | Cursor Previous Line [Ps] (default = 1)                                                                                      | done    |
| CSI Ps G      | CHA      | Cursor Horizontal Absolute [column] (default = 1)                                                                            | done    |
| CSI Ps ; Ps H | CUP      | Cursor Position [row;col] (default = [1,1])                                                                                  | done    |
| CSI Ps I      | CHT      | Cursor Horizontal Forward Tab [Ps] (default = 1)                                                                             | done    |
| CSI Ps J      | ED       | Erase in Display (0=end, 1=begin, 2=all, 3=saved)                                                                            | done    |
| CSI Ps K      | EL       | Erase in Line (0=end, 1=begin, 2=all)                                                                                        | done    |
| CSI Ps L      | IL       | Insert Lines [Ps] (default = 1)                                                                                              | done    |
| CSI Ps M      | DL       | Delete Lines [Ps] (default = 1)                                                                                              | done    |
| CSI Ps P      | DCH      | Delete Characters [Ps] (default = 1)                                                                                         | done    |
| CSI Ps S      | SU       | Scroll Up [Ps] (default = 1)                                                                                                 | done    |
| CSI Ps T      | SD       | Scroll Down [Ps] (default = 1)                                                                                               | done    |
| CSI Ps X      | ECH      | Erase Characters [Ps] (default = 1)                                                                                          | done    |
| CSI Ps Z      | CBT      | Cursor Backward Tab [Ps] (default = 1)                                                                                       | done    |
| CSI Ps @      | ICH      | Insert Characters [Ps] (default = 1)                                                                                         | done    |
| CSI Ps \`     | HPA      | Horizontal Position Absolute [column] — alias for CHA                                                                        | done    |
| CSI Ps b      | REP      | Repeat Preceding Character [Ps] times                                                                                        | done    |
| CSI Ps c      | DA1      | Primary Device Attributes                                                                                                    | done    |
| CSI > Ps c    | DA2      | Secondary Device Attributes                                                                                                  | done    |
| CSI Ps d      | VPA      | Vertical Position Absolute [row]                                                                                             | done    |
| CSI Ps ; Ps f | HVP      | Horizontal Vertical Position [row;col] — same as CUP                                                                         | done    |
| CSI Ps g      | TBC      | Tab Clear (0=current column, 3=all)                                                                                          | done    |
| CSI Ps h      | SM       | Set Mode — LNM (20) and IRM (4) implemented; KAM (2) and SRM (12) recognised, not implemented                                | partial |
| CSI Ps l      | RM       | Reset Mode — same as SM                                                                                                      | partial |
| CSI Ps m      | SGR      | Select Graphic Rendition ([SGR.md](./SGR.md))                                                                                | done    |
| CSI Ps n      | DSR      | Device Status Report — Ps=5 status, Ps=6 cursor position                                                                     | done    |
| CSI Ps > q    | XTVER    | XTVERSION query                                                                                                              | done    |
| CSI Ps SP q   | DECSCUSR | Set Cursor Style                                                                                                             | done    |
| CSI Ps ; Ps r | DECSTBM  | Set Scrolling Margins (top;bottom)                                                                                           | done    |
| CSI s         | SCOSC    | Save Cursor Position                                                                                                         | done    |
| CSI Ps t      | Window   | Window Manipulation (the plain form only; `CSI … $ t` and `CSI Ps SP t` are recognised and ignored)                          | done    |
| CSI u         | SCORC    | Restore Cursor Position; `CSI Ps u` with any parameter is ignored (kitty uses `CSI > u` / `CSI < u` / `CSI = u` / `CSI ? u`) | done    |
| CSI ? Pm h    | DECSET   | Set DEC Private Mode                                                                                                         | done    |
| CSI ? Pm l    | DECRST   | Reset DEC Private Mode                                                                                                       | done    |
| CSI ? Pm $p   | DECRQM   | Request Mode — full mode query via mode-sync loop                                                                            | done    |

## DEC Private Modes (CSI ? Pm h / l)

| Mode  | Name                                     | Status      | Notes                                                                                                                                     |
| ----- | ---------------------------------------- | ----------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| ?1    | DECCKM — Cursor Keys Mode                | done        | `TerminalModes.cursor_key`; GUI translates arrow keys when set                                                                            |
| ?2    | DECANM — VT52 Mode                       | done        | Full VT52 parser with 15 commands; `ESC <` returns to ANSI mode (Task 20.8)                                                               |
| ?3    | DECCOLM — 80/132 Column Mode             | done        | Column switching active when AllowColumnModeSwitch (?40) is enabled                                                                       |
| ?4    | DECSCLM — Scrolling Mode                 | partial     | Recognised but never settable (no smooth scrolling); DECRQM answers permanently reset                                                     |
| ?5    | DECSCNM — Reverse Video                  | done        | Per-pane, per-cell foreground/background swap at render time; a toggle repaints immediately                                               |
| ?6    | DECOM — Origin Mode                      | done        | CUP row 1 → top of scroll region when set                                                                                                 |
| ?7    | DECAWM — Auto Wrap Mode                  | done        | Implemented (`Decawm` enum)                                                                                                               |
| ?8    | DECARM — Auto Repeat Keys                | done        | `TerminalModes.repeat_keys`; GUI reads                                                                                                    |
| ?12   | XtCBlink — Cursor Blink                  | done        | Implemented                                                                                                                               |
| ?25   | DECTCEM — Show/Hide Cursor               | done        | Implemented                                                                                                                               |
| ?40   | AllowColumnModeSwitch                    | done        | Gates DECCOLM behavior                                                                                                                    |
| ?42   | DECNRCM — National Replacement           | partial     | Stored and reported by DECRQM; the UK set maps `#` to `£` regardless of this mode                                                         |
| ?45   | ReverseWrapAround                        | done        | `TerminalModes.reverse_wrap_around`                                                                                                       |
| ?47   | Alt Screen Buffer (legacy)               | done        | Switches screens and nothing else: no cursor save, no clear                                                                               |
| ?66   | DECNKM — Numeric Keypad Mode             | done        | DECSET alias for DECKPAM/DECKPNM; maps to `modes.keypad_mode` (Task 20.2)                                                                 |
| ?67   | DECBKM — Backarrow Key Mode              | done        | Set: BS sends 0x08; Reset: BS sends 0x7F (Task 20.3)                                                                                      |
| ?69   | DECLRMM — Left/Right Margin Mode         | done        | Horizontal scroll regions via DECSLRM; full margin-aware cursor/edit ops (Task 20.1)                                                      |
| ?80   | DECSDM — Sixel Display Mode              | done        | Set: Sixel no-scroll (display mode); Reset: cursor advances past image (Task 20.5)                                                        |
| ?1000 | X11 Mouse — Normal Tracking              | done        | `TerminalModes.mouse_tracking`; GUI reads and forwards mouse events                                                                       |
| ?1001 | XtMseHilite — Highlight Tracking         | not planned | Recognised and not implemented: set and reset log at `debug!`, DECRQM answers `Ps = 0`; does not turn on mouse reports                    |
| ?1002 | X11 Mouse — Button Event                 | done        | `TerminalModes.mouse_tracking`                                                                                                            |
| ?1003 | X11 Mouse — Any Event                    | done        | `TerminalModes.mouse_tracking`                                                                                                            |
| ?1004 | Focus Reporting                          | done        | `TerminalModes.focus_reporting`; GUI sends `InputEvent::FocusChange`                                                                      |
| ?1006 | SGR Mouse — Extended Coordinates         | done        | `TerminalModes.mouse_tracking`                                                                                                            |
| ?1007 | Alternate Scroll Mode                    | done        | Scroll → arrow keys on alt screen when set (Task 20.4)                                                                                    |
| ?1016 | SGR-Pixels Mouse — Pixel Coords          | done        | `TerminalModes.mouse_encoding`; one-based physical pixel coords (Task 124.3a)                                                             |
| ?1034 | Meta Sets Eighth Bit (Interpret Meta)    | not planned | Recognised and not implemented: `debug!` log, DECRQM `Ps = 0`; conflicts with UTF-8 input                                                 |
| ?1035 | NumLock Overrides the Application Keypad | not planned | Recognised and not implemented: `debug!` log, DECRQM `Ps = 0`; winit exposes no NumLock state                                             |
| ?1036 | Meta Sends Escape (Alt Sends ESC)        | done        | Default set: legacy Alt+text sends `ESC` then the text (Task 147.24). Not on macOS, where Option composes text; Option as Alt is Task 152 |
| ?1039 | Alt Sends Escape                         | partial     | Stored and reported by DECRQM, with no further effect                                                                                     |
| ?1045 | XTREVWRAP2 — Extended Rev-Wrap           | done        | Extends ?45 reverse-wrap into scrollback region (Task 20.9)                                                                               |
| ?1046 | AllowAltScreen                           | done        | Gates ?47/?1047/?1049 alternate screen switching (Task 20.6)                                                                              |
| ?1047 | Alt Screen Buffer (legacy)               | done        | Enter is the same as ?47; leaving clears the alternate screen, then switches                                                              |
| ?1048 | Save/Restore Cursor (legacy)             | done        | Save/restore through the DECSC slot of the active screen                                                                                  |
| ?1049 | Alt Screen Buffer + Save Cursor          | done        | DECSC, switch to the alternate screen, then clear it; leaving restores the cursor                                                         |
| ?1070 | Private Color Registers                  | done        | Per-graphic Sixel palette; default on (Task 20.11)                                                                                        |
| ?2004 | Bracketed Paste                          | done        | `TerminalModes.bracketed_paste`; GUI wraps paste with `\e[200~` / `\e[201~`                                                               |
| ?2026 | Synchronized Output                      | done        | `TerminalModes.synchronized_updates`                                                                                                      |
| ?2027 | Grapheme Clustering                      | done        | Always-on via `unicode-segmentation`; DECRQM reports permanently set (Task 20.7)                                                          |
| ?2048 | In-Band Resize Notifications             | done        | Set sends an in-band size report immediately; DECRQM supported                                                                            |
| ?7727 | Application Escape Key                   | done        | Unambiguous Escape encoding for tmux                                                                                                      |

## Standard Modes (CSI Pm h / l)

| Ps  | Name                          | Status      | Notes                                                                                                    |
| --- | ----------------------------- | ----------- | -------------------------------------------------------------------------------------------------------- |
| 2   | KAM — Keyboard Action Mode    | not planned | Recognised and not implemented: `debug!` log, DECRQM `Ps = 0`; kitty and WezTerm do not implement it     |
| 4   | IRM — Insert/Replace Mode     | done        | Insert-mode text shifting; DECRQM supported                                                              |
| 12  | SRM — Send/Receive Mode       | not planned | Recognised and not implemented: `debug!` log, DECRQM `Ps = 0`; the kernel already echoes on a modern tty |
| 20  | LNM — Line Feed/New Line Mode | done        | Implemented                                                                                              |

## OSC — Operating System Commands

| Sequence                   | Purpose                                 | Status  | Notes                                                                                                                |
| -------------------------- | --------------------------------------- | ------- | -------------------------------------------------------------------------------------------------------------------- |
| OSC 0 ; txt BEL            | Set icon + window title                 | partial | Works; icon name vs. title not distinguished                                                                         |
| OSC 1 ; txt BEL            | Set icon title                          | partial | Shares handler with OSC 0 (treated as full title)                                                                    |
| OSC 2 ; txt BEL            | Set window title                        | done    | Implemented                                                                                                          |
| OSC 4 ; n ; rgb BEL        | Set palette entry                       | done    | Sets 256-color palette entry; query responds with value                                                              |
| OSC 7 ; URI BEL            | Current Working Directory               | done    | Stored in `TerminalHandler.current_working_directory`                                                                |
| OSC 8 ; params ; URI BEL   | Hyperlink                               | done    | Fully implemented — hyperlink start/end with URL                                                                     |
| OSC 9 ; body BEL           | Desktop notification (iTerm2)           | done    | Plus OSC 9 ; 4 progress (ConEmu)                                                                                     |
| OSC 10 ; spec BEL          | Foreground color query/set              | done    | Set overrides the default foreground and renders; `?` replies with the override or the theme colour                  |
| OSC 11 ; spec BEL          | Background color query/set              | done    | Set overrides the default background (pane base fill) and renders; `?` replies with the override or the theme colour |
| OSC 12 ; color BEL         | Set/query cursor color                  | done    | Set, query and reset of the cursor colour override                                                                   |
| OSC 17 / OSC 19 ; spec BEL | Selection background / foreground color | done    | Set, query and render; reset with OSC 117 / OSC 119                                                                  |
| OSC 21                     | Kitty color control                     | missing | Task 144                                                                                                             |
| OSC 22 ; op names ST       | Kitty pointer shapes                    | done    | Set, push, pop and query on per-screen stacks                                                                        |
| OSC 52 ; c ; data BEL      | Clipboard copy/paste                    | done    | Clipboard set/query forwarded via `WindowManipulation`                                                               |
| OSC 66 ; meta ; text ST    | Kitty text sizing                       | stub    | Vestigial no-op (Task 104)                                                                                           |
| OSC 72                     | Kitty drag and drop                     | missing | Task 105                                                                                                             |
| OSC 99 ; meta ; payload    | Kitty desktop notifications             | done    | Conformant with kitty 0.49.2 (Task 138)                                                                              |
| OSC 104 BEL                | Reset palette entry (or all)            | done    | Resets specific or all palette entries to defaults                                                                   |
| OSC 110 BEL                | Reset foreground color                  | done    | Clears the dynamic foreground override                                                                               |
| OSC 111 BEL                | Reset background color                  | done    | Clears the dynamic background override                                                                               |
| OSC 112 BEL                | Reset cursor color                      | done    | Clears the cursor colour override                                                                                    |
| OSC 133 ; … BEL            | FTCS / Shell Integration                | done    | All four markers (A/B/C/D) parsed and stored                                                                         |
| OSC 777                    | System notification (urxvt)             | done    | `notify;TITLE;BODY`, routed per `[notifications]` config                                                             |
| OSC 1337                   | iTerm2 inline images                    | done    | `File=` and multipart `MultipartFile=` / `FilePart=` / `FileEnd`                                                     |

## DCS — Device Control String

| Sequence        | Name                  | Status  | Notes                                                                       |
| --------------- | --------------------- | ------- | --------------------------------------------------------------------------- |
| DCS (all)       | General               | done    | Sub-command dispatch via `handle_device_control_string()`                   |
| DCS $ q … ST    | DECRQSS               | done    | Supports `m` (SGR), `r` (DECSTBM), `q` (DECSCUSR); unknown → error response |
| DCS + q … ST    | XTGETTCAP             | done    | Responds to common capability queries; unknown → error response             |
| DCS tmux ; … ST | tmux passthrough      | done    | Inner sequences are run through a fresh parser, nesting up to 4 levels      |
| DCS Sixel       | Sixel                 | done    | Fully implemented — parser, DCS dispatch, buffer placement, GPU rendering   |
| APC _G … ST     | Kitty Graphics        | partial | See COVERAGE (Tasks 135, 136)                                               |
| APC (other)     | Other APC sub-command | stub    | Logged and ignored                                                          |

## FTCS — FinalTerm Control Sequences (OSC 133)

| Sequence  | Name                  | Status | Notes                                        |
| --------- | --------------------- | ------ | -------------------------------------------- |
| OSC 133 A | Prompt Start          | done   | Parsed and stored in `FtcsState`             |
| OSC 133 B | Prompt End            | done   | Parsed and stored in `FtcsState`             |
| OSC 133 C | Pre-execution (input) | done   | Parsed and stored in `FtcsState`             |
| OSC 133 D | Command Finished      | done   | Parsed; exit code stored in `last_exit_code` |
