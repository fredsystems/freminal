# Escape Sequence Gaps

Last updated: 2026-03-09 — Corrected via comprehensive codebase audit (Task 7)

This document lists ANSI / DEC / xterm / iTerm2 / WezTerm escape sequences **not yet fully
implemented in Freminal**, plus critical bugs in existing implementations. It serves as a
roadmap for improving compatibility and feature parity with modern terminals.

---

## Summary

The audit revealed that Freminal's actual coverage is lower than previously documented.
While core cursor movement, SGR colors, and basic screen editing work correctly, there are:

- **8 bugs** in implemented sequences (DECSTBM, DL wiring, DSR, CSI u, DEC mode swallowing,
  TerminalModes never written, silent CSI consumption, OSC double emission)
- **5 missing C0 control handlers** (HT, VT, FF, NUL, DEL) — tab characters don't work
- **14+ DEC private modes** parsed but silently swallowed (DECCKM, bracketed paste, mouse, etc.)
- **8+ missing CSI commands** (CNL, CPL, SU, SD, CBT, CHT, TBC, REP, HPA, CSI s)
- **Missing tab stop infrastructure** entirely (no HTS, TBC, CHT, CBT, no default 8-column stops)

Previously documented as missing but actually implemented:

- ~~CSI P (DCH)~~ — fully implemented
- ~~CSI @ (ICH)~~ — fully implemented
- ~~CSI 6n (DSR)~~ — implemented (though buggy, ignores Ps)
- ~~ESC M (Reverse Index)~~ — fully implemented
- ~~OSC 8 (Hyperlinks)~~ — fully implemented

Legend:

- **Importance:** 🟩 High (affects interoperability) | 🟨 Medium | ⬜ Low / optional
- **Type:** 🐛 Bug | ⬜ Missing | 🚧 Stub/Partial

---

## Critical Bugs in Existing Implementations

| Category  | Sequence / Area              | Importance | Type | Description                                                                                                                                                             |
| --------- | ---------------------------- | ---------- | ---- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **CSI**   | `CSI r` — DECSTBM            | 🟩         | 🐛   | Double-decrement: handler subtracts 1, then buffer subtracts 1 again. Scroll regions off by one. **Likely cause of vttest failures.**                                   |
| **CSI**   | `CSI M` — DL (Delete Lines)  | 🟩         | 🐛   | Handler and buffer code exist but CSI dispatch has no `b'M'` arm. Silently consumed.                                                                                    |
| **CSI**   | `CSI n` — DSR                | 🟨         | 🐛   | Always emits cursor position report regardless of Ps. Ps=5 should give status report.                                                                                   |
| **CSI**   | `CSI u` — Restore Cursor     | 🟨         | 🐛   | Mapped to Kitty keyboard protocol (always Skipped), blocking ANSI SCORC.                                                                                                |
| **Modes** | DEC Private Mode dispatch    | 🟩         | 🐛   | 14+ modes (including ?1, ?2004, ?1000–?1006, ?1004) fall through `_other` catch-all with no logging. TerminalModes struct fields are never written by the mode handler. |
| **CSI**   | Unrecognized CSI final bytes | 🟨         | 🐛   | Silently consumed at `csi.rs:240` with no log and no effect.                                                                                                            |
| **OSC**   | Unknown OSC handling         | ⬜         | 🐛   | Error-level logging + TerminalOutput::Invalid double emission. Should be debug-level.                                                                                   |

---

## Missing or Partial Sequences

### C0 Control Characters

| Sequence   | Importance | Type | Description                                                                                                     |
| ---------- | ---------- | ---- | --------------------------------------------------------------------------------------------------------------- |
| HT (0x09)  | 🟩         | ⬜   | Tab byte falls through as data. No tab stop array, no default 8-column stops. Breaks `ls`, `man`, shell output. |
| VT (0x0B)  | 🟩         | ⬜   | Should act as LF per VT spec. Falls through as data.                                                            |
| FF (0x0C)  | 🟩         | ⬜   | Should act as LF per VT spec. Falls through as data.                                                            |
| NUL (0x00) | 🟨         | ⬜   | Should be silently ignored. Currently included in data.                                                         |
| DEL (0x7F) | 🟨         | ⬜   | Should be silently ignored. Currently not handled.                                                              |
| SO (0x0E)  | ⬜         | ⬜   | G1 charset switching. Low priority.                                                                             |
| SI (0x0F)  | ⬜         | ⬜   | G0 charset switching. Low priority.                                                                             |

### CSI Commands

| Sequence      | Importance | Type | Description                                             |
| ------------- | ---------- | ---- | ------------------------------------------------------- |
| CSI E — CNL   | 🟩         | ⬜   | Cursor Next Line — move down N lines, column 1          |
| CSI F — CPL   | 🟩         | ⬜   | Cursor Previous Line — move up N lines, column 1        |
| CSI S — SU    | 🟩         | ⬜   | Scroll Up — scroll content up N lines                   |
| CSI T — SD    | 🟩         | ⬜   | Scroll Down — scroll content down N lines               |
| CSI I — CHT   | 🟨         | ⬜   | Cursor Horizontal Forward Tab — advance to Nth tab stop |
| CSI Z — CBT   | 🟨         | ⬜   | Cursor Backward Tab — move back to Nth tab stop         |
| CSI g — TBC   | 🟨         | ⬜   | Tab Clear — clear tab stops (Ps=0 current, Ps=3 all)    |
| CSI b — REP   | 🟨         | ⬜   | Repeat preceding character N times                      |
| CSI \` — HPA  | 🟨         | ⬜   | Horizontal Position Absolute — move to column N         |
| CSI s — SCOSC | 🟨         | ⬜   | Save Cursor Position — no `b's'` arm in CSI dispatch    |
| CSI Ps h — SM | 🟨         | ⬜   | Set Mode — IRM (4) and SRM (12) not implemented         |
| CSI Ps l — RM | 🟨         | ⬜   | Reset Mode — IRM (4) and SRM (12) not implemented       |

### ESC Sequences

| Sequence | Importance | Type | Description                                                             |
| -------- | ---------- | ---- | ----------------------------------------------------------------------- |
| ESC H    | 🟩         | ⬜   | HTS — Horizontal Tab Set. Not parsed. Needed for tab infrastructure.    |
| ESC c    | 🟩         | 🚧   | RIS — Full Reset. Parsed as stub with `warn!`, does not actually reset. |
| ESC =    | 🟨         | 🚧   | DECPAM — Application Keypad. Stub, no effect on keypad mode.            |
| ESC >    | 🟨         | 🚧   | DECPNM — Numeric Keypad. Stub, no effect on keypad mode.                |
| ESC Z    | ⬜         | ⬜   | DECID — Return Terminal ID. Not parsed.                                 |

### DEC Private Modes (parsed but silently swallowed)

| Mode  | Importance | Type | Description                                                                                      |
| ----- | ---------- | ---- | ------------------------------------------------------------------------------------------------ |
| ?1    | 🟩         | 🚧   | DECCKM — Cursor Keys. **Breaks vim, tmux, htop.** Parsed but TerminalModes.cursor_key never set. |
| ?2004 | 🟩         | 🚧   | Bracketed Paste. **Breaks paste in shells/editors.** Parsed but never written to TerminalModes.  |
| ?1000 | 🟩         | 🚧   | X11 Mouse — Normal Tracking. Silently swallowed.                                                 |
| ?1002 | 🟩         | 🚧   | X11 Mouse — Button Event Tracking. Silently swallowed.                                           |
| ?1003 | 🟩         | 🚧   | X11 Mouse — Any Event Tracking. Silently swallowed.                                              |
| ?1006 | 🟩         | 🚧   | SGR Mouse — Extended Coordinates. Silently swallowed.                                            |
| ?1004 | 🟨         | 🚧   | Focus Events. Silently swallowed.                                                                |
| ?5    | 🟨         | 🚧   | DECSCNM — Reverse Video. Silently swallowed.                                                     |
| ?6    | 🟨         | 🚧   | DECOM — Origin Mode. Silently swallowed.                                                         |
| ?3    | ⬜         | 🚧   | DECCOLM — 80/132 Column. Silently swallowed.                                                     |
| ?2026 | 🟨         | 🚧   | Synchronized Output. Silently swallowed.                                                         |

### DEC Private Modes (not even parsed)

| Mode          | Importance | Description                       |
| ------------- | ---------- | --------------------------------- |
| ?47 / ?1047   | 🟨         | Legacy alt screen buffer variants |
| ?1048         | 🟨         | Legacy save/restore cursor        |
| ?2 (DECANM)   | ⬜         | VT52 mode                         |
| ?66 (DECNKM)  | ⬜         | Numeric keypad mode               |
| ?67 (DECBKM)  | ⬜         | Backarrow key mode                |
| ?69 (DECLRMM) | ⬜         | Left/right margin mode            |

### OSC Commands

| Sequence                | Importance | Type | Description                                          |
| ----------------------- | ---------- | ---- | ---------------------------------------------------- |
| OSC 52 (clipboard)      | 🟩         | ⬜   | Clipboard copy/paste — used by shells, vim, tmux     |
| OSC 7 (CWD)             | 🟨         | 🚧   | Recognized/logged but no functional effect           |
| OSC 4 (palette)         | 🟨         | ⬜   | Dynamic palette change                               |
| OSC 12 (cursor color)   | ⬜         | ⬜   | Set cursor color                                     |
| OSC 104 (reset palette) | ⬜         | ⬜   | Reset palette entries                                |
| OSC 110 (reset fg)      | ⬜         | ⬜   | Reset foreground color                               |
| OSC 111 (reset bg)      | ⬜         | ⬜   | Reset background color                               |
| OSC 133 (FTCS)          | 🟨         | 🚧   | Shell integration markers — recognized but no effect |
| OSC 777 (notification)  | ⬜         | ⬜   | Konsole notification (rare)                          |

### DCS Sub-commands

| Sequence  | Importance | Type | Description                                   |
| --------- | ---------- | ---- | --------------------------------------------- |
| DECRQSS   | 🟨         | ⬜   | Request selection or setting — no DCS parsing |
| XTGETTCAP | 🟨         | ⬜   | Termcap query — nvim sends this               |
| Sixel     | ⬜         | ⬜   | Raster graphics — large undertaking           |

### Tab Stop Infrastructure (entirely absent)

The tab stop system is completely missing. This affects:

- **HT (0x09)** — Tab character does nothing
- **ESC H (HTS)** — Set tab stop at current column
- **CSI g (TBC)** — Clear tab stop(s)
- **CSI I (CHT)** — Cursor forward to tab stop
- **CSI Z (CBT)** — Cursor backward to tab stop
- **Default 8-column stops** — Not initialized

This is a Priority 1 gap because tab characters are fundamental to shell output.

---

## Roadmap by Priority

### Priority 1 — Critical / vttest (fix first)

| Item                               | Rationale                               |
| ---------------------------------- | --------------------------------------- |
| Fix DECSTBM double-decrement       | Causes vttest cursor movement failures  |
| Wire DL (CSI M) dispatch           | Handler exists, just needs dispatch arm |
| Implement tab stops + HT           | Tabs are fundamental to shell output    |
| Handle VT (0x0B) / FF (0x0C) as LF | Per VT spec, very simple fix            |
| Handle NUL (0x00) / DEL (0x7F)     | Silently ignore — simple fix            |
| Add CNL (CSI E) / CPL (CSI F)      | Basic cursor movement                   |

### Priority 2 — Breaks real apps (vim, tmux, htop)

| Item                              | Rationale                                  |
| --------------------------------- | ------------------------------------------ |
| Wire DECCKM (?1) to TerminalModes | Cursor keys don't enter application mode   |
| Wire bracketed paste (?2004)      | Paste in shells/editors is broken          |
| Wire mouse tracking (?1000–?1006) | All TUI apps with mouse support broken     |
| Wire focus events (?1004)         | Focus reporting broken                     |
| Implement SU (CSI S) / SD (CSI T) | Scroll operations used by TUI apps         |
| Fix DSR to check Ps value         | Ps=5 should give status, not cursor report |
| Implement ESC c (RIS) fully       | Full terminal reset needed for recovery    |
| Implement DECPAM / DECPNM         | Keypad mode needed for numpad in apps      |
| Wire ?5/?6/?3 to TerminalModes    | Stop silently swallowing these modes       |
| Add logging for unrecognized CSI  | Replace silent consumption with `warn!`    |

### Priority 3 — Modern features

| Item                        | Rationale                            |
| --------------------------- | ------------------------------------ |
| OSC 52 (clipboard)          | Copy/paste in tmux/vim/zsh           |
| OSC 7 (CWD tracking)        | Tab titles, file operations          |
| OSC 133 (shell integration) | Prompt/command markers for modern UX |
| HTS (ESC H) / TBC (CSI g)   | Tab stop management beyond defaults  |
| CHT (CSI I) / CBT (CSI Z)   | Tab cursor movement                  |
| DECRQSS / XTGETTCAP         | Terminal capability queries          |

### Priority 4 — Polish

| Item                                | Rationale                    |
| ----------------------------------- | ---------------------------- |
| CSI s (SCOSC) / fix CSI u (SCORC)   | Save/restore cursor position |
| REP (CSI b)                         | Repeat last character        |
| HPA (CSI \`)                        | Horizontal position absolute |
| OSC 4/104 (palette set/reset)       | Dynamic palette support      |
| DECALN (ESC # 8)                    | Screen alignment test        |
| ?47/?1047/?1048 (legacy alt screen) | Legacy compatibility         |
| Reduce OSC unknown log severity     | Error → debug level          |

---

## Implementation Hints

- **DECSTBM bug fix**: Remove the `-1` in either `handle_set_scroll_region` or `Buffer::set_scroll_region`, not both.
- **DL wiring**: Add `b'M'` arm in `csi.rs` dispatch, calling existing `handle_delete_lines()`.
- **Tab stops**: Add `Vec<bool>` tab stop array to Buffer, initialize with 8-column stops, add HT handler in `ansi.rs`.
- **DEC mode wiring**: Replace `_other` catch-all in `terminal_handler.rs:651` with explicit arms that write to `TerminalState.modes`.
- **OSC 52**: Encode/decode base64 payloads, forward to system clipboard (Wayland/X11).
- **Mouse**: TerminalModes fields exist — just need the mode handler to write them, and GUI to read and forward events.

---

## Strategic Notes

- Fixing **Priority 1 items** (6 items) will resolve vttest failures and make basic shell output correct.
- Fixing **Priority 2 items** (10 items) will make vim, tmux, htop, and other TUI apps work properly.
- Together, Priorities 1+2 bring Freminal to **~80% compatibility** with a modern terminal.
- Priority 3 adds modern conveniences for WezTerm/iTerm2 feature parity.
- Priority 4 is polish and edge cases.

---

© 2025 Freminal Project — MIT License.
