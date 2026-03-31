# PLAN_20 — DEC Private Mode Coverage

## Status: Pending

---

## Overview

Comprehensive audit of DEC private mode support in Freminal, cross-referenced against the
community-maintained mode registry at <https://wiki.tau.garden/dec-modes/>. The audit identified
27 fully implemented modes, 3 parsed-but-stub modes, 1 intentionally omitted mode, and a
prioritized list of gaps.

This task addresses the high-priority and medium-priority gaps — modes that real programs send
and expect to work, plus modes needed for correctness with existing features (Sixel, grapheme
clustering). Terminal-specific extensions (mintty, Contour, VTE, RLogin, foot, Kitty, etc.) and
archaic DEC hardware modes are explicitly out of scope.

**Dependencies:** None (independent)
**Dependents:** None
**Primary crates:** `freminal-common`, `freminal-buffer`, `freminal-terminal-emulator`
**Estimated scope:** Medium-Large (12 subtasks)

---

## Current State

### Fully Implemented (27 modes)

All of these store state, affect behavior, and respond to DECRQM queries.

| Mode    | Name                   | What It Controls                                  |
| ------- | ---------------------- | ------------------------------------------------- |
| `?1`    | DECCKM                 | Cursor key encoding (application vs normal)       |
| `?3`    | DECCOLM                | 132/80 column switch                              |
| `?5`    | DECSCNM                | Screen invert (light/dark)                        |
| `?6`    | DECOM                  | Origin mode (cursor relative to scroll region)    |
| `?7`    | DECAWM                 | Auto-wrap at right margin                         |
| `?8`    | DECARM                 | Auto-repeat keys                                  |
| `?9`    | X10 Mouse              | X10 mouse tracking                                |
| `?12`   | XtCBlink               | Cursor blink (xterm)                              |
| `?25`   | DECTCEM                | Cursor visibility                                 |
| `?40`   | Allow 80⇒132           | Gates `?3` behavior                               |
| `?45`   | Reverse Wraparound     | Cursor wraps left past column 0                   |
| `?47`   | Alt Screen (47)        | Alternate screen buffer                           |
| `?1000` | X11 Mouse              | X11 mouse button tracking                         |
| `?1002` | Cell Motion Mouse      | Mouse motion while button held                    |
| `?1003` | All Motion Mouse       | All mouse motion events                           |
| `?1004` | Focus Events           | FocusIn/FocusOut reporting                        |
| `?1005` | UTF-8 Mouse            | UTF-8 mouse coordinate encoding                   |
| `?1006` | SGR Mouse              | SGR mouse coordinate encoding                     |
| `?1016` | SGR Pixel Mouse        | SGR mouse with pixel coordinates                  |
| `?1047` | Alt Screen (1047)      | Alternate screen buffer (variant)                 |
| `?1048` | Save/Restore Cursor    | DECSC/DECRC via DECSET/DECRST                     |
| `?1049` | Save + Alt Screen      | Combined `?1048` + `?1047`                        |
| `?2004` | Bracketed Paste        | Paste delimiters for shells/editors               |
| `?2026` | Synchronized Output    | Frame batching (skip_draw)                        |
| `?2048` | modifyOtherKeys DEC    | DEC private mode alias for modifyOtherKeys        |
| `?7727` | Application Escape Key | Unambiguous Escape encoding for tmux              |
| `20`    | LNM                    | Line feed / new line mode (ANSI, not DEC private) |

### Parsed But Stub (3 modes)

These have `Mode` enum variants and are recognized by the parser, but the handler logs a
warning and takes no action. DECRQM is not implemented for these.

| Mode    | Name                    | Current Behavior | Assessment                                                                                                                                                                                                                                          |
| ------- | ----------------------- | ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `?4`    | DECSCLM — Smooth Scroll | Logged, dropped  | Smooth vs jump scroll. No modern terminal implements this. All rendering is already "smooth" at 60+ fps. **No action needed — leave as stub.**                                                                                                      |
| `?2027` | Grapheme Clustering     | Logged, dropped  | Freminal already unconditionally uses `unicode-segmentation` `graphemes(true)` in `TChar::from_vec`. The mode type reports `;3$y` (permanently set) but the dispatch is in the catch-all. **Subtask 20.7 promotes this to a properly routed mode.** |
| `?2031` | Color Palette Updates   | Logged, dropped  | Contour extension for dark/light mode change notifications. **Niche — no action needed.**                                                                                                                                                           |

### Intentionally Omitted (1 mode)

| Mode    | Name                 | Rationale                                                                                                                                                                            |
| ------- | -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `?1015` | urxvt Mouse Encoding | Documented in `mode.rs:151-153`. The encoding format clashes with DL/SD/window manipulation sequences. `?1006` (SGR) is the universally preferred replacement. **Do not implement.** |

---

## Subtasks — High Priority

These are modes that real, widely-used programs send and expect to work.

---

### 20.1 — Implement `?69` DECLRMM (Left/Right Margin Mode)

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** `freminal-buffer/src/buffer.rs`, `freminal-buffer/src/terminal_handler.rs`,
  `freminal-common/src/buffer_states/mode.rs`, `freminal-common/src/buffer_states/modes/`
- **Details:**
  DECLRMM enables horizontal scroll margins set by DECSLRM (`CSI Pl ; Pr s`). When active,
  text insertion, deletion, and cursor movement respect left and right margin boundaries,
  not just the physical screen edges. This is the horizontal equivalent of DECSTBM (vertical
  scroll regions).

  Without DECLRMM, programs that set left/right margins get garbled output. This includes
  vttest origin mode / margin tests, `dialog` in certain layouts, and some tmux split-pane
  rendering.

  Implementation requires:
  1. Add `Mode::Declrmm` variant to `mode.rs` with `Set`/`Reset`/`Query` states.
  2. Add `left_margin` / `right_margin` fields to `Buffer`.
  3. Parse DECSLRM (`CSI Pl ; Pr s`) — note this shares the final byte `s` with SCOSC
     (Save Cursor). The disambiguation rule: if DECLRMM is active, `CSI s` with parameters
     is DECSLRM; `CSI s` with no parameters is SCOSC. If DECLRMM is not active, `CSI s` is
     always SCOSC.
  4. Update `insert_text`, `delete_chars`, `insert_chars`, `erase_chars`,
     `cursor_forward`, `cursor_backward`, and scroll operations to respect left/right
     margins when DECLRMM is set.
  5. DECRQM response for `?69`.
  6. Reset margins on DECLRMM reset, on DECCOLM change, and on hard reset.

- **Acceptance criteria:**
  - `CSI ? 69 h` enables DECLRMM; `CSI ? 69 l` disables it.
  - `CSI 10 ; 70 s` (with DECLRMM active) sets left margin to column 10, right to 70.
  - Text insertion wraps at the right margin, not the screen edge.
  - IL/DL/ICH/DCH operate within the left/right margin boundaries.
  - Cursor movement (CUF, CUB) stops at margin boundaries when DECLRMM is active.
  - DECRQM `?69` responds correctly.
  - `CSI s` with no params still performs SCOSC regardless of DECLRMM state.
- **Tests required:**
  - DECLRMM set/reset/query
  - DECSLRM with valid params, clamped params, reversed params
  - Text insertion at right margin wraps to left margin of next line
  - ICH/DCH within margins
  - IL/DL within margins (vertical scroll respects horizontal bounds)
  - CUF/CUB stop at margins
  - SCOSC disambiguation: `CSI s` with no params is always SCOSC
  - Reset: margins clear on DECLRMM reset

---

### 20.2 — Implement `?66` DECNKM (Numeric Keypad Mode)

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** `freminal-common/src/buffer_states/mode.rs`,
  `freminal-common/src/buffer_states/modes/`,
  `freminal-buffer/src/terminal_handler.rs`,
  `freminal-terminal-emulator/src/state/internal.rs`
- **Details:**
  DECNKM is the DECSET/DECRST alias for keypad application mode. Freminal already tracks
  keypad mode via `ESC =` (DECKPAM) and `ESC >` (DECKPNM), stored as
  `modes.keypad_mode`. However, some programs use `CSI ? 66 h` / `CSI ? 66 l` instead
  of the ESC sequences. Currently these fall into the `Unknown` catch-all.

  Implementation:
  1. Add `Mode::Decnkm` variant with `Application`/`Numeric`/`Query` states.
  2. In the handler or `sync_mode_flags`, map `Decnkm(Application)` →
     `modes.keypad_mode = Application`, `Decnkm(Numeric)` → `modes.keypad_mode = Numeric`.
  3. DECRQM response for `?66`.

- **Acceptance criteria:**
  - `CSI ? 66 h` sets keypad to application mode (same effect as `ESC =`).
  - `CSI ? 66 l` sets keypad to numeric mode (same effect as `ESC >`).
  - DECRQM `?66` reports current keypad mode.
  - Keypad keys send correct sequences in both modes.
- **Tests required:**
  - DECNKM set/reset/query
  - Verify keypad mode matches after `CSI ? 66 h` vs `ESC =`
  - Verify keypad mode matches after `CSI ? 66 l` vs `ESC >`

---

### 20.3 — Implement `?67` DECBKM (Backarrow Key Mode)

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** `freminal-common/src/buffer_states/mode.rs`,
  `freminal-common/src/buffer_states/modes/`,
  `freminal-buffer/src/terminal_handler.rs`,
  `freminal-terminal-emulator/src/interface.rs`,
  `freminal-terminal-emulator/src/snapshot.rs`
- **Details:**
  DECBKM controls what the Backspace key sends:
  - Set (`?67 h`): Backspace sends `BS` (0x08) — this is Freminal's current hardcoded
    behavior (`char_to_ctrl_code(b'H')` in `to_payload()`).
  - Reset (`?67 l`): Backspace sends `DEL` (0x7F).

  Most programs rely on `stty erase` rather than this mode, but some programs (particularly
  those that set up their own terminal modes) use DECBKM directly. The terminfo entry
  `kbs=\177` for `xterm-256color` says Backspace should send DEL, which conflicts with
  Freminal's hardcoded BS. Implementing DECBKM makes this configurable and correct.

  Implementation:
  1. Add `Mode::Decbkm` variant with `BackarrowSendsBs`/`BackarrowSendsDel`/`Query` states.
  2. Store `backarrow_key_mode` in `TerminalHandler`, default to `BackarrowSendsBs` (matching
     current behavior; can revisit default after terminfo audit).
  3. Expose in snapshot.
  4. In `to_payload()`, `Self::Backspace` reads the mode: BS (0x08) when set, DEL (0x7F)
     when reset.
  5. DECRQM response for `?67`.

- **Acceptance criteria:**
  - `CSI ? 67 h` makes Backspace send 0x08.
  - `CSI ? 67 l` makes Backspace send 0x7F.
  - DECRQM `?67` reports current state.
  - Default behavior unchanged (Backspace sends 0x08).
- **Tests required:**
  - DECBKM set/reset/query
  - `to_payload()` for `Backspace` with each mode state
  - Default state sends BS

---

### 20.4 — Implement `?1007` Alternate Scroll Mode

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** `freminal-common/src/buffer_states/mode.rs`,
  `freminal-common/src/buffer_states/modes/`,
  `freminal-terminal-emulator/src/state/internal.rs`,
  `freminal-terminal-emulator/src/snapshot.rs`,
  `freminal/src/gui/terminal.rs`
- **Details:**
  When `?1007` is set and the alternate screen is active, mouse scroll wheel events should
  be translated to arrow key sequences (Up/Down) instead of being sent as mouse events or
  used for scrollback navigation.

  Freminal currently has logic in `terminal.rs` that sends arrow keys for scroll events on
  the alternate screen, but this is unconditional — it always does it. The correct behavior
  is to gate this on `?1007`:
  - `?1007` set + alternate screen: scroll → arrow keys.
  - `?1007` reset + alternate screen: scroll events are ignored (or handled as mouse events
    if mouse tracking is active).
  - Primary screen: `?1007` has no effect; scroll always navigates scrollback.

  Implementation:
  1. Add `Mode::AlternateScroll` variant with `Set`/`Reset`/`Query` states.
  2. Store the flag in `sync_mode_flags` (this is a GUI-concern mode).
  3. Expose in snapshot.
  4. In `terminal.rs`, gate the alt-screen scroll-to-arrow-keys logic on the snapshot field.
  5. DECRQM response for `?1007`.

- **Acceptance criteria:**
  - `CSI ? 1007 h` enables alternate scroll mode.
  - `CSI ? 1007 l` disables it.
  - With `?1007` set and alternate screen active, mouse scroll sends arrow keys.
  - With `?1007` reset and alternate screen active, mouse scroll does not send arrow keys.
  - Primary screen scroll behavior is unaffected by `?1007`.
  - DECRQM `?1007` reports current state.
- **Tests required:**
  - AlternateScroll set/reset/query
  - Snapshot carries the flag correctly

---

### 20.5 — Implement `?80` DECSDM (Sixel Display Mode)

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** `freminal-common/src/buffer_states/mode.rs`,
  `freminal-common/src/buffer_states/modes/`,
  `freminal-buffer/src/terminal_handler.rs`
- **Details:**
  Freminal has **complete Sixel support** — parser (`freminal-common/src/buffer_states/sixel.rs`,
  826 lines, 17 tests), DCS dispatch (`terminal_handler.rs:926-927`, `1412-1470`), buffer
  placement via `ImageProtocol::Sixel`, and GPU rendering via the shared image shader in
  `renderer.rs`. However, `?80` (DECSDM) is not in the mode parser — it falls through to
  `Unknown`.

  DECSDM controls Sixel scrolling behavior:
  - Set (`?80 h`): Sixel Display Mode — Sixel images are displayed at the cursor position
    and the cursor does NOT advance past the image (display mode, image overwrites in-place).
  - Reset (`?80 l`): Sixel Scrolling Mode (default) — Sixel images are displayed at the
    cursor position and the cursor advances below the image, scrolling if needed.

  The current implementation in `handle_sixel` places images and advances the cursor
  (scrolling mode behavior), which is the correct default. DECSDM adds the ability to
  suppress cursor advancement.

  Implementation:
  1. Add `Mode::Decsdm` variant with `DisplayMode`/`ScrollingMode`/`Query` states.
  2. Store `sixel_display_mode: bool` in `TerminalHandler`, default `false` (scrolling mode).
  3. In `handle_sixel`, skip cursor advancement and scrolling when `sixel_display_mode`
     is true.
  4. DECRQM response for `?80`.

- **Acceptance criteria:**
  - `CSI ? 80 h` enables Sixel display mode (no scroll after image).
  - `CSI ? 80 l` restores scrolling mode (cursor advances past image).
  - DECRQM `?80` reports current state.
  - Default behavior unchanged (scrolling mode).
  - Sixel images render correctly in both modes.
- **Tests required:**
  - DECSDM set/reset/query
  - Sixel placement with scrolling mode: cursor below image after placement
  - Sixel placement with display mode: cursor unchanged after placement

---

## Subtasks — Medium Priority

---

### 20.6 — Implement `?1046` Enable/Disable Alternate Screen Switching

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-common/src/buffer_states/mode.rs`,
  `freminal-common/src/buffer_states/modes/`,
  `freminal-buffer/src/terminal_handler.rs`
- **Details:**
  `?1046` controls whether `?47`, `?1047`, and `?1049` are honored. When `?1046` is reset,
  alternate screen switching is blocked — DECSET/DECRST of `?47`/`?1047`/`?1049` become
  no-ops. tmux and GNU screen sometimes set/reset this.

  Implementation:
  1. Add `Mode::AllowAltScreen` variant with `Allow`/`Disallow`/`Query` states.
  2. Store `allow_alt_screen: bool` in `TerminalHandler`, default `true`.
  3. Gate `handle_enter_alternate()` and `handle_leave_alternate()` on this flag.
  4. DECRQM response for `?1046`.

- **Acceptance criteria:**
  - `CSI ? 1046 l` prevents subsequent `CSI ? 1049 h` from switching to alternate screen.
  - `CSI ? 1046 h` re-enables alternate screen switching.
  - DECRQM `?1046` reports current state.
  - Default is enabled (current behavior preserved).
- **Tests required:**
  - AllowAltScreen set/reset/query
  - Alt screen switch blocked when `?1046` is reset
  - Alt screen switch works when `?1046` is set

---

### 20.7 — Promote `?2027` Grapheme Clustering to Properly Routed Mode

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-buffer/src/terminal_handler.rs`,
  `freminal-terminal-emulator/src/state/internal.rs`
- **Details:**
  Freminal **already performs grapheme clustering unconditionally** via
  `unicode-segmentation`'s `graphemes(true)` in `TChar::from_vec`
  (`freminal-common/src/buffer_states/tchar.rs:79-87`). The `?2027` mode type already
  exists in `freminal-common/src/buffer_states/modes/grapheme.rs` and its `report()` method
  returns `;3$y` (permanently set) for both the `Unicode` and `Legacy` variants — correctly
  advertising that grapheme clustering is always on.

  However, the mode is currently in the "not acted on" catch-all in both
  `terminal_handler.rs` and `state/internal.rs`, which means:
  1. DECRQM queries for `?2027` do not get a response (the query path is never reached).
  2. A `debug!` warning fires every time a program sets or resets `?2027`.

  The fix is to route the mode properly so queries work and noise is eliminated. No runtime
  behavior change is needed — Freminal is correct to always do grapheme clustering.

  Implementation:
  1. Move `GraphemeClustering` out of the catch-all in `terminal_handler.rs` into an
     explicit arm that handles `Query` (sends DECRQM `;3$y`) and silently accepts
     `Set`/`Reset` (no state change needed — behavior is permanent).
  2. Add explicit no-op arm in `sync_mode_flags` for `GraphemeClustering` (handled by
     handler layer).
  3. Remove the `debug!` warning for this mode.

- **Acceptance criteria:**
  - `CSI ? 2027 $ p` (DECRQM) responds with `CSI ? 2027 ; 3 $ y` (permanently set).
  - `CSI ? 2027 h` and `CSI ? 2027 l` are silently accepted (no log noise).
  - Grapheme clustering behavior unchanged (always on).
- **Tests required:**
  - DECRQM response for `?2027` returns `;3$y`
  - Set/reset accepted without error

---

### 20.8 — Implement `?2` DECANM (VT52 Mode)

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-common/src/buffer_states/mode.rs`,
  `freminal-common/src/buffer_states/modes/`,
  `freminal-terminal-emulator/src/ansi_components/`,
  `freminal-buffer/src/terminal_handler.rs`
- **Details:**
  DECANM switches the terminal between ANSI mode and VT52 compatibility mode. When reset
  (`CSI ? 2 l`), the terminal enters VT52 mode where escape sequences use the shorter VT52
  format (e.g. `ESC A` for cursor up instead of `CSI A`). Setting (`CSI ? 2 h`) returns to
  ANSI mode.

  This is required for vttest compliance (vttest has a dedicated VT52 mode test section).
  In real-world use, essentially no modern program sends this — it is a compatibility
  test artifact.

  This is the highest-effort medium-priority item because it requires a second parser path
  for VT52 sequences. The VT52 command set is small (roughly 15 commands), but the parser
  must be able to switch between VT52 and ANSI mode at runtime.

  Implementation:
  1. Add `Mode::Decanm` variant with `Ansi`/`Vt52`/`Query` states.
  2. Add a VT52 parser mode to `FreminalAnsiParser` that interprets the reduced VT52
     escape set: `ESC A` (up), `ESC B` (down), `ESC C` (right), `ESC D` (left),
     `ESC F` (enter graphics), `ESC G` (exit graphics), `ESC H` (cursor home),
     `ESC I` (reverse line feed), `ESC J` (erase to end of screen), `ESC K` (erase to
     end of line), `ESC Y Pl Pc` (direct cursor address), `ESC Z` (identify),
     `ESC =` (alt keypad), `ESC >` (exit alt keypad), `ESC <` (exit VT52 mode → ANSI).
  3. Store parser mode flag; switch on `?2` set/reset.
  4. `ESC <` from within VT52 mode returns to ANSI mode.

- **Acceptance criteria:**
  - `CSI ? 2 l` enters VT52 mode.
  - VT52 cursor movement sequences work.
  - `ESC <` returns to ANSI mode.
  - vttest VT52 mode section passes basic cursor and erase tests.
- **Tests required:**
  - Mode set/reset/query
  - VT52 cursor movement (all 4 directions)
  - VT52 cursor home, direct cursor address
  - VT52 erase commands
  - VT52 identify response
  - Round-trip: ANSI → VT52 → ANSI

---

### 20.9 — Implement `?1045` XTREVWRAP2 (Extended Reverse-Wraparound)

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-common/src/buffer_states/mode.rs`,
  `freminal-common/src/buffer_states/modes/`,
  `freminal-buffer/src/buffer.rs`,
  `freminal-buffer/src/terminal_handler.rs`
- **Details:**
  `?1045` is xterm's extended reverse-wraparound mode. Standard `?45` (already implemented)
  allows the cursor to wrap backwards past column 0 to the end of the previous line.
  `?1045` extends this to allow wrapping past the top of the screen into the scrollback
  buffer.

  Few programs use this directly, but xterm implements it and some automated terminal tests
  check for it.

  Implementation:
  1. Add `Mode::XtRevWrap2` variant with `Set`/`Reset`/`Query` states.
  2. Store flag in `TerminalHandler` or `Buffer`.
  3. Extend the existing reverse-wrap logic in `cursor_backward` / `backspace` to continue
     wrapping into the scrollback region when `?1045` is set.
  4. DECRQM response for `?1045`.

- **Acceptance criteria:**
  - `CSI ? 1045 h` enables extended reverse-wrap.
  - Cursor at (0,0) wrapping left enters scrollback when `?1045` is set.
  - Normal `?45` behavior unchanged when `?1045` is not set.
  - DECRQM `?1045` responds correctly.
- **Tests required:**
  - Mode set/reset/query
  - Wrap into scrollback at top-left corner
  - No wrap into scrollback when mode is off

---

### 20.10 — Implement `?1001` Hilite Mouse Tracking

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-common/src/buffer_states/mode.rs`,
  `freminal-common/src/buffer_states/modes/mouse.rs`,
  `freminal-terminal-emulator/src/state/internal.rs`,
  `freminal/src/gui/terminal.rs`
- **Details:**
  `?1001` enables hilite mouse tracking, an X11-era protocol where the terminal highlights
  a text region on mouse press and sends the start/end coordinates to the application on
  release. Almost no modern program uses this — it exists primarily for X11 protocol
  completeness and terminal compliance tests.

  The mouse tracking infrastructure already exists (`MouseTrack` enum, encoding modes, GUI
  mouse event handling). This adds another variant.

  Implementation:
  1. Add `MouseTrack::XtMseHilite` variant for `?1001`.
  2. Add `b"?1001"` parser arm in `mode.rs`.
  3. In the GUI mouse handler, implement the hilite tracking protocol: on button press,
     begin highlighting; on release, send both press and release coordinates.
  4. DECRQM response via existing `MouseMode(Query(...))` path.

- **Acceptance criteria:**
  - `CSI ? 1001 h` enables hilite mouse tracking.
  - Mouse press starts highlight; release sends coordinate pair.
  - DECRQM `?1001` responds correctly.
- **Tests required:**
  - Mode set/reset/query
  - Correct `MouseTrack` variant stored

---

### 20.11 — Implement `?1070` Private Color Registers for Sixel

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-common/src/buffer_states/mode.rs`,
  `freminal-common/src/buffer_states/modes/`,
  `freminal-buffer/src/terminal_handler.rs`,
  `freminal-common/src/buffer_states/sixel.rs`
- **Details:**
  `?1070` controls whether each Sixel graphic gets its own private color register set or
  whether all graphics share a single global palette. The default (reset) is shared
  registers; set gives each graphic independent registers.

  Freminal's current Sixel parser (`parse_sixel`) initializes a fresh 256-entry palette for
  each image, which is effectively private-color-register behavior. This means the current
  implementation already behaves as if `?1070` is set.

  Implementation:
  1. Add `Mode::PrivateColorRegisters` variant with `Private`/`Shared`/`Query` states.
  2. Store flag in `TerminalHandler`, default `true` (matching current behavior).
  3. When `Shared` mode: maintain a persistent Sixel palette across `handle_sixel` calls
     instead of reinitializing it each time.
  4. DECRQM response for `?1070`.

  Note: shared registers are rarely useful in practice. Most implementations default to
  private registers. The main value is correctly responding to DECRQM queries.

- **Acceptance criteria:**
  - `CSI ? 1070 h` enables private color registers (current behavior).
  - `CSI ? 1070 l` enables shared registers.
  - DECRQM `?1070` responds correctly.
  - Default behavior unchanged.
- **Tests required:**
  - Mode set/reset/query
  - Two sequential Sixel images with shared registers: second image inherits first's palette

---

### 20.12 — Implement `?42` DECNRCM (National Replacement Character Set Mode)

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-common/src/buffer_states/mode.rs`,
  `freminal-common/src/buffer_states/modes/`,
  `freminal-buffer/src/terminal_handler.rs`
- **Details:**
  DECNRCM controls whether the terminal uses national replacement character sets (NRCs) for
  G0–G3 designations. When set, character set designations (SCS sequences like `ESC ( A` for
  UK) map specific ASCII positions to national characters (e.g. `#` → `£` for UK). When
  reset, the terminal uses standard character sets.

  This is largely supplanted by UTF-8, but some legacy applications and international
  environments still use NRC sets. The existing G0 charset infrastructure (DEC line drawing
  via `ESC ( 0`, ASCII via `ESC ( B`) provides the foundation.

  Implementation:
  1. Add `Mode::Decnrcm` variant with `NrcEnabled`/`NrcDisabled`/`Query` states.
  2. Store flag in `TerminalHandler`, default `false` (NRC disabled — standard behavior).
  3. When NRC is enabled, extend the existing `DecSpecialGraphics` / character replacement
     logic to support additional character set designations (UK, French, German, etc.).
  4. DECRQM response for `?42`.

  Note: full NRC support requires implementing multiple national character set tables.
  An initial implementation can just track the mode and respond to DECRQM, with the actual
  character substitution tables added incrementally.

- **Acceptance criteria:**
  - `CSI ? 42 h` enables NRC mode.
  - `CSI ? 42 l` disables NRC mode.
  - DECRQM `?42` responds correctly.
  - When enabled, `ESC ( A` (UK charset) maps `#` to `£`.
- **Tests required:**
  - Mode set/reset/query
  - UK character set substitution when NRC is enabled
  - No substitution when NRC is disabled

---

## Implementation Notes

### Subtask Ordering

Subtasks 20.2, 20.3, 20.4, 20.5, 20.6, 20.7, 20.9, 20.10, 20.11, and 20.12 are independent
of each other and can be implemented in any order or in parallel.

Subtask 20.1 (DECLRMM) is the most complex and touches the buffer layer extensively. It should
be implemented last to avoid conflicts with the simpler mode additions.

Subtask 20.8 (VT52) is high-effort and self-contained — it adds a second parser mode. It can
be done at any point but should not block the simpler items.

**Recommended order:** 20.2 → 20.3 → 20.7 → 20.4 → 20.5 → 20.6 → 20.10 → 20.11 → 20.12
→ 20.9 → 20.5 → 20.8 → 20.1

### Pattern to Follow

Each new mode follows the same pattern established by the existing mode infrastructure:

1. Create a type in `freminal-common/src/buffer_states/modes/<name>.rs` with set/reset/query
   variants and a `report()` method for DECRQM.
2. Add `pub mod <name>;` to `freminal-common/src/buffer_states/modes/mod.rs`.
3. Add `Mode::<Name>(<Type>)` variant to `freminal-common/src/buffer_states/mode.rs`.
4. Add parser arm in `terminal_mode_from_params()` for `b"?NN"`.
5. Add handler arm in `terminal_handler.rs` `process_output` match.
6. Add `sync_mode_flags` arm in `state/internal.rs` if the mode is GUI-concern.
7. Add explicit no-op arms in whichever layer does NOT own the mode.
8. Add snapshot field if the GUI needs to read the mode.
9. Add DECRQM response.
10. Add unit tests for the mode type, handler dispatch, and DECRQM.

### Verification

Each subtask must pass before proceeding:

- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo-machete`

---

## Appendix A — Complete Mode Registry Audit

Every mode from <https://wiki.tau.garden/dec-modes/> is listed below with its classification
relative to Freminal. This table is the authoritative reference for deciding whether a mode
should be implemented.

### Classification Key

| Code     | Meaning                                                          |
| -------- | ---------------------------------------------------------------- |
| **IMPL** | Fully implemented — state stored, behavior acts, DECRQM responds |
| **STUB** | Parsed and recognized but no behavior; handler logs/drops        |
| **PLAN** | Not yet implemented; subtask exists in this document             |
| **SKIP** | Intentionally not implemented; rationale provided                |

### Full Mode Table

| Mode            | Origin         | Name                                  | Classification | Notes                                                |
| --------------- | -------------- | ------------------------------------- | -------------- | ---------------------------------------------------- |
| `?1`            | DEC            | DECCKM — Cursor Keys                  | **IMPL**       | `modes.cursor_key`                                   |
| `?2`            | DEC            | DECANM — VT52 Mode                    | **PLAN 20.8**  | Requires second parser path                          |
| `?3`            | DEC            | DECCOLM — Column                      | **IMPL**       | Buffer resize + PTY resize                           |
| `?4`            | DEC            | DECSCLM — Scrolling                   | **STUB**       | Smooth vs jump scroll; irrelevant at 60+ fps         |
| `?5`            | DEC            | DECSCNM — Screen Invert               | **IMPL**       | `modes.invert_screen`                                |
| `?6`            | DEC            | DECOM — Origin Mode                   | **IMPL**       | Buffer tracks origin-relative cursor                 |
| `?7`            | DEC            | DECAWM — Auto Wrap                    | **IMPL**       | Buffer tracks wrap state                             |
| `?8`            | DEC            | DECARM — Auto Repeat                  | **IMPL**       | `modes.repeat_keys`                                  |
| `?9`            | DEC            | DECINLM — Interlace Mode              | **SKIP**       | DEC hardware interlace; not applicable               |
| `?9`            | xterm          | X10 Mouse Tracking                    | **IMPL**       | `MouseTrack::XtMsex10`                               |
| `?10`           | DEC            | DECEDM — Editing Mode                 | **SKIP**       | DEC hardware editing mode                            |
| `?10`           | rxvt           | Show toolbar                          | **SKIP**       | rxvt-specific UI chrome                              |
| `?11`           | DEC            | DECLTM — Line Transmit                | **SKIP**       | DEC hardware line transmit timing                    |
| `?12`           | DEC            | DECKANAM — Katakana Shift             | **SKIP**       | DEC hardware Katakana; xterm `?12` handled instead   |
| `?12`           | xterm          | Blinking cursor                       | **IMPL**       | `cursor_visual_style`                                |
| `?13`           | DEC            | DECSCFDM — Space Compression          | **SKIP**       | DEC hardware field delimiter                         |
| `?13`           | xterm          | Start blinking cursor                 | **SKIP**       | xterm-specific blink variant; `?12` sufficient       |
| `?14`           | DEC            | DECTEM — Transmit Execution           | **SKIP**       | DEC hardware transmit mode                           |
| `?14`           | xterm          | XOR blinking cursor                   | **SKIP**       | xterm-specific blink variant                         |
| `?16`           | DEC            | DECEKEM — Edit Key Execution          | **SKIP**       | DEC hardware edit key mode                           |
| `?18`           | DEC            | DECPFF — Print Form Feed              | **SKIP**       | Printer mode; no printer support                     |
| `?19`           | DEC            | DECPEX — Printer Extent               | **SKIP**       | Printer mode; no printer support                     |
| `?20`           | DEC            | OV1 — Overstrike                      | **SKIP**       | DEC hardware overstrike                              |
| `20`            | —              | LNM — Line Feed/New Line              | **IMPL**       | ANSI mode (not DEC private)                          |
| `?21`           | DEC            | BA1 — Local BASIC                     | **SKIP**       | DEC hardware BASIC interpreter                       |
| `?22`           | DEC            | BA2 — Host BASIC                      | **SKIP**       | DEC hardware BASIC interpreter                       |
| `?23`           | DEC            | PK1 — Programmable Keypad             | **SKIP**       | DEC hardware programmable keypad                     |
| `?24`           | DEC            | AH1 — Auto Hardcopy                   | **SKIP**       | DEC hardware hardcopy                                |
| `?25`           | DEC            | DECTCEM — Cursor Visible              | **IMPL**       | `show_cursor`                                        |
| `?27`           | DEC            | DECPSP — Proportional Spacing         | **SKIP**       | Monospace terminal; not applicable                   |
| `?29`           | DEC            | DECPSM — Pitch Select                 | **SKIP**       | DEC hardware pitch control                           |
| `?30`           | rxvt           | Show scrollbar                        | **SKIP**       | rxvt-specific UI chrome                              |
| `?34`           | DEC            | DECRLM — Right-to-Left                | **SKIP**       | Requires full BiDi support; massive effort           |
| `?35`           | DEC            | DECHEBM — Hebrew Keyboard             | **SKIP**       | DEC-specific Hebrew keyboard mode                    |
| `?35`           | rxvt           | Font-shifting functions               | **SKIP**       | rxvt-specific font switching                         |
| `?36`           | DEC            | DECHEM — Hebrew Encoding              | **SKIP**       | DEC-specific Hebrew encoding                         |
| `?38`           | DEC            | DECTEK — Tektronix Mode               | **SKIP**       | Tektronix 4010/4014 emulation; extremely niche       |
| `?40`           | DEC            | DECCRNLM — CR/NL Mode                 | **SKIP**       | DEC hardware CR/NL mode                              |
| `?40`           | xterm          | Allow 80⇒132                          | **IMPL**       | `allow_column_mode_switch`                           |
| `?41`           | DEC            | DECUPM — Unidirectional Print         | **SKIP**       | DEC hardware printer mode                            |
| `?41`           | xterm          | `more(1)` fix                         | **SKIP**       | Obscure xterm workaround                             |
| `?42`           | DEC            | DECNRCM — National Replacement        | **PLAN 20.12** | Character set substitution                           |
| `?43`           | DEC            | DECGEPM — Graphics Expanded Print     | **SKIP**       | DEC hardware graphics print                          |
| `?44`           | DEC            | DECGPCM — Graphics Print Color        | **SKIP**       | DEC hardware graphics color print                    |
| `?44`           | xterm          | Margin bell                           | **SKIP**       | xterm-specific audio bell                            |
| `?45`           | DEC            | DECGPCS — Graphics Print Color Syntax | **SKIP**       | DEC hardware graphics print                          |
| `?45`           | xterm          | Reverse-wraparound                    | **IMPL**       | `modes.reverse_wrap_around`                          |
| `?46`           | DEC            | DECGPBM — Graphics Print Background   | **SKIP**       | DEC hardware graphics print                          |
| `?46`           | xterm          | Start logging                         | **SKIP**       | xterm-specific file logging                          |
| `?47`           | DEC            | DECGRPM — Graphics Rotated Print      | **SKIP**       | DEC hardware graphics print                          |
| `?47`           | xterm          | Alternate Screen Buffer               | **IMPL**       | `handle_enter/leave_alternate`                       |
| `?49`           | DEC            | DECTHAIM — Thai Input                 | **SKIP**       | DEC hardware Thai input                              |
| `?50`           | DEC            | DECTHAICM — Thai Cursor               | **SKIP**       | DEC hardware Thai cursor                             |
| `?51`           | DEC            | DECBWRM — B/W Reversal                | **SKIP**       | DEC hardware B/W mode                                |
| `?52`           | DEC            | DECOPM — Origin Placement             | **SKIP**       | DEC hardware origin placement                        |
| `?53`           | DEC            | DEC131TM — VT131 Transmit             | **SKIP**       | DEC hardware VT131 transmit                          |
| `?55`           | DEC            | DECBPM — Bold Page                    | **SKIP**       | DEC hardware bold page                               |
| `?57`           | DEC            | DECNAKB — Greek/N-A Keyboard          | **SKIP**       | DEC hardware Greek keyboard                          |
| `?58`           | DEC            | DECIPEM — IBM Proprinter Emulation    | **SKIP**       | DEC hardware printer emulation                       |
| `?59`           | DEC            | DECKKDM — Kanji/Katakana Display      | **SKIP**       | DEC hardware Kanji display                           |
| `?60`           | DEC            | DECHCCM — Horizontal Cursor Coupling  | **SKIP**       | DEC hardware cursor coupling                         |
| `?61`           | DEC            | DECVCCM — Vertical Cursor Coupling    | **SKIP**       | DEC hardware cursor coupling                         |
| `?64`           | DEC            | DECPCCM — Page Cursor Coupling        | **SKIP**       | DEC hardware page coupling                           |
| `?65`           | DEC            | DECBCMM — Business Color Matching     | **SKIP**       | DEC hardware color matching                          |
| `?66`           | DEC            | DECNKM — Numeric Keypad               | **PLAN 20.2**  | DECSET alias for DECKPAM/DECKPNM                     |
| `?67`           | DEC            | DECBKM — Backarrow Key                | **PLAN 20.3**  | Toggles BS (0x08) vs DEL (0x7F)                      |
| `?68`           | DEC            | DECKBUM — Keyboard Usage              | **SKIP**       | DEC hardware keyboard usage mode                     |
| `?69`           | DEC            | DECLRMM — Left/Right Margins          | **PLAN 20.1**  | Horizontal scroll regions                            |
| `?70`           | DEC            | DECFPM — Force Plot                   | **SKIP**       | DEC hardware force plot                              |
| `?73`           | DEC            | DECXRLM — Transmission Rate           | **SKIP**       | DEC hardware rate limiting                           |
| `?80`           | DEC            | DECSDM — Sixel Display Mode           | **PLAN 20.5**  | Controls Sixel scroll vs display                     |
| `?81`           | DEC            | DECKPM — Key Position Mode            | **SKIP**       | DEC hardware key position                            |
| `?83`           | WY-370         | 52 line                               | **SKIP**       | Wyse hardware 52-line mode                           |
| `?84`           | WY-370         | Erasable attribute select             | **SKIP**       | Wyse hardware attribute                              |
| `?85`           | WY-370         | Replacement character color           | **SKIP**       | Wyse hardware color                                  |
| `?90`           | DEC            | DECTHAISCM — Thai Space Compensating  | **SKIP**       | DEC hardware Thai spacing                            |
| `?95`           | DEC            | DECNCSM — No Clear on Column Change   | **SKIP**       | Niche DEC behavior                                   |
| `?96`           | DEC            | DECRLCM — Right to Left Copy          | **SKIP**       | DEC hardware RTL copy                                |
| `?97`           | DEC            | DECCRTSM — CRT Save                   | **SKIP**       | DEC hardware CRT save                                |
| `?98`           | DEC            | DECARSM — Auto Resize                 | **SKIP**       | DEC hardware auto resize                             |
| `?99`           | DEC            | DECMCM — Modem Control                | **SKIP**       | DEC hardware modem control                           |
| `?100`          | DEC            | DECAAM — Auto Answerback              | **SKIP**       | DEC hardware answerback                              |
| `?101`          | DEC            | DECCANSM — Conceal Answerback         | **SKIP**       | DEC hardware answerback                              |
| `?102`          | DEC            | DECNULM — Ignore Null                 | **SKIP**       | DEC hardware null handling                           |
| `?103`          | DEC            | DECHDPXM — Half Duplex                | **SKIP**       | DEC hardware half duplex                             |
| `?104`          | DEC            | DECESKM — Secondary Keyboard          | **SKIP**       | DEC hardware secondary keyboard                      |
| `?106`          | DEC            | DECOSCNM — Overscan                   | **SKIP**       | DEC hardware overscan                                |
| `?108`          | DEC            | DECNUMLK — NumLock                    | **SKIP**       | DEC hardware NumLock                                 |
| `?109`          | DEC            | DECCAPSLK — Caps Lock                 | **SKIP**       | DEC hardware Caps Lock                               |
| `?110`          | DEC            | DECKLHIM — Keyboard LEDs              | **SKIP**       | DEC hardware LED control                             |
| `?111`          | DEC            | DECFWM — Framed Windows               | **SKIP**       | DEC hardware framed windows                          |
| `?112`          | DEC            | DECRPL — Review Previous Lines        | **SKIP**       | DEC hardware review mode                             |
| `?113`          | DEC            | DECHWUM — Host Wake-Up                | **SKIP**       | DEC hardware wake-up                                 |
| `?114`          | DEC            | DECATCUM — Alt Text Color Underline   | **SKIP**       | DEC hardware underline color                         |
| `?115`          | DEC            | DECATCBM — Alt Text Color Blink       | **SKIP**       | DEC hardware blink color                             |
| `?116`          | DEC            | DECBBSM — Bold and Blink Style        | **SKIP**       | DEC hardware bold/blink                              |
| `?117`          | DEC            | DECECM — Erase Color                  | **SKIP**       | DEC hardware erase color                             |
| `?1000`         | xterm          | X11 Mouse Press                       | **IMPL**       | `MouseTrack::XtMseX11`                               |
| `?1001`         | xterm          | Hilite Mouse Tracking                 | **PLAN 20.10** | X11 highlight protocol                               |
| `?1002`         | xterm          | Cell Motion Mouse                     | **IMPL**       | `MouseTrack::XtMseBtn`                               |
| `?1003`         | xterm          | All Motion Mouse                      | **IMPL**       | `MouseTrack::XtMseAny`                               |
| `?1004`         | xterm          | Focus In/Out Events                   | **IMPL**       | `modes.focus_reporting`                              |
| `?1005`         | xterm          | UTF-8 Mouse Encoding                  | **IMPL**       | `MouseEncoding::Utf8`                                |
| `?1006`         | xterm          | SGR Mouse Encoding                    | **IMPL**       | `MouseEncoding::Sgr`                                 |
| `?1007`         | xterm          | Alternate Scroll Mode                 | **PLAN 20.4**  | Scroll → arrow keys on alt screen                    |
| `?1010`         | rxvt           | Scroll to bottom on output            | **SKIP**       | rxvt-specific behavior                               |
| `?1011`         | rxvt           | Scroll to bottom on keypress          | **SKIP**       | rxvt-specific behavior                               |
| `?1014`         | xterm          | fastScroll resource                   | **SKIP**       | xterm-specific performance hint                      |
| `?1015`         | urxvt          | urxvt Mouse Encoding                  | **SKIP**       | Format clashes with CSI sequences; `?1006` preferred |
| `?1016`         | xterm          | SGR Pixel Mouse                       | **IMPL**       | `MouseEncoding::SgrPixels`                           |
| `?1021`         | rxvt           | Bold = high intensity                 | **SKIP**       | rxvt-specific rendering                              |
| `?1034`         | xterm          | Interpret meta key                    | **SKIP**       | xterm-specific X11 meta handling                     |
| `?1035`         | xterm          | Special modifiers for Alt/NumLock     | **SKIP**       | xterm-specific modifier handling                     |
| `?1036`         | xterm          | Send ESC when Meta modifies           | **SKIP**       | xterm-specific meta handling                         |
| `?1037`         | xterm          | Send DEL from Delete key              | **SKIP**       | xterm-specific delete handling                       |
| `?1039`         | xterm          | Send ESC when Alt modifies            | **SKIP**       | xterm-specific alt handling                          |
| `?1040`         | xterm          | Keep selection unhighlighted          | **SKIP**       | xterm-specific X11 selection                         |
| `?1041`         | xterm          | Use CLIPBOARD selection               | **SKIP**       | xterm-specific X11 clipboard                         |
| `?1042`         | xterm          | Urgency on Ctrl-G                     | **SKIP**       | xterm-specific window manager hint                   |
| `?1043`         | xterm          | Raise window on Ctrl-G                | **SKIP**       | xterm-specific window manager                        |
| `?1044`         | xterm          | Reuse CLIPBOARD data                  | **SKIP**       | xterm-specific X11 clipboard                         |
| `?1045`         | xterm          | XTREVWRAP2 — Extended Reverse-Wrap    | **PLAN 20.9**  | Wrap into scrollback                                 |
| `?1046`         | xterm          | Enable/Disable Alt Screen             | **PLAN 20.6**  | Gates `?47`/`?1047`/`?1049`                          |
| `?1047`         | xterm          | Alternate Screen Buffer               | **IMPL**       | Same handler as `?47`                                |
| `?1048`         | xterm          | Save/Restore Cursor                   | **IMPL**       | DECSC/DECRC via DECSET                               |
| `?1049`         | xterm          | Save Cursor + Alt Screen              | **IMPL**       | Combined `?1048` + `?1047`                           |
| `?1050`         | xterm          | terminfo function-key mode            | **SKIP**       | xterm-specific function key encoding                 |
| `?1051`         | xterm          | Sun function-key mode                 | **SKIP**       | xterm-specific function key encoding                 |
| `?1052`         | xterm          | HP function-key mode                  | **SKIP**       | xterm-specific function key encoding                 |
| `?1053`         | xterm          | SCO function-key mode                 | **SKIP**       | xterm-specific function key encoding                 |
| `?1060`         | xterm          | Legacy keyboard emulation             | **SKIP**       | xterm-specific X11R6 keyboard                        |
| `?1061`         | xterm          | VT220 keyboard emulation              | **SKIP**       | xterm-specific keyboard                              |
| `?1070`         | xterm          | Private color registers               | **PLAN 20.11** | Per-graphic Sixel palette                            |
| `?1243`         | VTE            | Arrow keys BiDi                       | **SKIP**       | VTE-specific BiDi extension                          |
| `?1337`         | iTerm2         | Report Key Up                         | **SKIP**       | iTerm2-specific key reporting                        |
| `?2001`         | xterm          | Readline mouse button-1               | **SKIP**       | xterm-specific readline integration                  |
| `?2002`         | xterm          | Readline mouse button-2               | **SKIP**       | xterm-specific readline integration                  |
| `?2003`         | xterm          | Readline mouse button-3               | **SKIP**       | xterm-specific readline integration                  |
| `?2004`         | xterm          | Bracketed Paste                       | **IMPL**       | `modes.bracketed_paste`                              |
| `?2005`         | xterm          | Readline character-quoting            | **SKIP**       | xterm-specific readline integration                  |
| `?2006`         | xterm          | Readline newline pasting              | **SKIP**       | xterm-specific readline integration                  |
| `?2026`         | Contour        | Synchronized Output                   | **IMPL**       | `modes.synchronized_updates`                         |
| `?2027`         | mintty/Contour | Grapheme Clustering                   | **PLAN 20.7**  | Always-on; promote from stub to proper route         |
| `?2028`         | Contour        | Text reflow                           | **SKIP**       | Contour-specific extension                           |
| `?2029`         | Contour        | Passive Mouse Tracking                | **SKIP**       | Contour-specific extension                           |
| `?2030`         | Contour        | Grid cell selection                   | **SKIP**       | Contour-specific extension                           |
| `?2031`         | Contour        | Color palette updates                 | **STUB**       | Niche; leave as stub                                 |
| `?2048`         | rockorager     | In-Band Window Resize                 | **IMPL**       | modifyOtherKeys DEC mode alias                       |
| `?2500`         | VTE            | Mirror box drawing                    | **SKIP**       | VTE-specific extension                               |
| `?2501`         | VTE            | BiDi autodetection                    | **SKIP**       | VTE-specific BiDi                                    |
| `?7700`         | mintty         | Ambiguous width reporting             | **SKIP**       | mintty-specific                                      |
| `?7711`         | mintty         | Scroll markers                        | **SKIP**       | mintty-specific                                      |
| `?7723`         | mintty         | Rewrap on resize                      | **SKIP**       | mintty-specific (deprecated)                         |
| `?7727`         | mintty         | Application Escape Key                | **IMPL**       | Unambiguous Escape for tmux                          |
| `?7728`         | mintty         | Alt escape `^\`                       | **SKIP**       | mintty-specific                                      |
| `?7730`         | mintty         | Graphics position                     | **SKIP**       | mintty-specific                                      |
| `?7765`         | mintty         | Alt mousewheel mode                   | **SKIP**       | mintty-specific                                      |
| `?7766`         | mintty         | Show/hide scrollbar                   | **SKIP**       | mintty-specific                                      |
| `?7767`         | mintty         | Font change reporting                 | **SKIP**       | mintty-specific                                      |
| `?7780`         | mintty         | Graphics position                     | **SKIP**       | mintty-specific                                      |
| `?7783`         | mintty         | Shortcut key mode                     | **SKIP**       | mintty-specific                                      |
| `?7786`         | mintty         | Mousewheel reporting                  | **SKIP**       | mintty-specific                                      |
| `?7787`         | mintty         | Application mousewheel                | **SKIP**       | mintty-specific                                      |
| `?7796`         | mintty         | BiDi on current line                  | **SKIP**       | mintty-specific                                      |
| `?8200`         | Tera Term      | TTCTH                                 | **SKIP**       | Tera Term-specific                                   |
| `?8400`–`?8459` | RLogin         | Various                               | **SKIP**       | RLogin-specific (15+ modes)                          |
| `?8452`         | xterm/RLogin   | Sixel cursor position                 | **SKIP**       | Sixel cursor left-of-graphic; niche                  |
| `?8800`–`?8804` | DRCSTerm       | Various                               | **SKIP**       | DRCSTerm-specific                                    |
| `?8840`         | Tanasinn       | Ambiguous width as double             | **SKIP**       | Tanasinn-specific                                    |
| `?9001`         | conpty         | win32-input-mode                      | **SKIP**       | Windows Terminal-specific                            |
| `?19997`        | Kitty          | Ctrl-C/Ctrl-Z handling                | **SKIP**       | Kitty-specific signal mode                           |
| `?77096`        | mintty         | BiDi                                  | **SKIP**       | mintty-specific                                      |
| `?737769`       | foot           | IME mode                              | **SKIP**       | foot-specific                                        |

---

## References

- <https://wiki.tau.garden/dec-modes/> — Community DEC mode registry
- <https://vt100.net/emu/dec_private_modes.html> — DEC private mode reference
- <https://invisible-island.net/xterm/ctlseqs/ctlseqs.html> — xterm control sequences
- `freminal-common/src/buffer_states/mode.rs` — Mode enum and parser
- `freminal-buffer/src/terminal_handler.rs:3222-3417` — Handler mode dispatch
- `freminal-terminal-emulator/src/state/internal.rs:246-348` — sync_mode_flags dispatch
- `freminal-common/src/buffer_states/sixel.rs` — Sixel parser (826 lines)
- `freminal-common/src/buffer_states/tchar.rs:79-87` — Grapheme clustering via `unicode-segmentation`
- `freminal-common/src/buffer_states/modes/grapheme.rs` — `?2027` mode type
