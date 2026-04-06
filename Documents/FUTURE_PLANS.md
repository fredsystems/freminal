# FUTURE_PLANS.md — Freminal Gap Analysis & Future Work

## Overview

This document catalogs every identified gap between Freminal's current state and what a
production-quality terminal emulator should deliver. Items are organized into three categories:

- **Category A** — Standard terminal emulator expectations (features users expect from any
  terminal). Missing these causes users to bounce.
- **Category B** — Competitor differentiation features (WezTerm, Ghostty, Kitty). Nice-to-have
  features that elevate Freminal beyond "functional" to "compelling."
- **Category C** — Remaining Master Plan tasks already tracked elsewhere.

Each item includes a severity rating, affected files, and estimated scope.

### Severity Ratings

| Rating       | Meaning                                                            |
| ------------ | ------------------------------------------------------------------ |
| **Critical** | Users will abandon the terminal immediately without this           |
| **High**     | Serious daily-use friction; power users will notice within minutes |
| **Medium**   | Noticeable gap but workarounds exist; affects specific workflows   |
| **Low**      | Polish item; absence is tolerable but presence signals maturity    |

---

## Category A — Standard Terminal Emulator Expectations

These are features that users expect from any terminal emulator. Their absence is not a
differentiation trade-off — it is a gap.

---

### A.1 — No Tabs

Severity: Critical

Freminal launches a single terminal instance per window. There is no tab bar, no Ctrl+Shift+T to
open a new tab, no tab switching, no tab reordering, no tab close. Users who work with multiple
sessions must open multiple OS windows.

Every major competitor (WezTerm, Ghostty, Kitty, Alacritty+tmux, iTerm2, Windows Terminal)
provides tabs as a core feature. This is the single most likely reason a user would close
Freminal and return to their previous terminal.

**Scope:** Large. Requires:

- Tab bar UI component in the GUI layer
- Per-tab `TerminalEmulator` + PTY ownership (each tab owns its own PTY thread, ArcSwap, channels)
- Tab lifecycle management (create, close, reorder, rename)
- Keyboard shortcuts (Ctrl+Shift+T new, Ctrl+Shift+W close, Ctrl+Tab / Ctrl+Shift+Tab switch)
- Config integration (`[tabs]` section: position, show_single_tab, etc.)
- Session state: which tab is active, tab order

**Primary files:** `freminal/src/gui/mod.rs`, `freminal/src/main.rs`, new `freminal/src/gui/tabs.rs`

---

### A.2 — No Split Panes

Severity: Medium

No horizontal or vertical split pane support. Users who want side-by-side terminals must use
OS window tiling or tmux.

Less critical than tabs because tmux/zellij are common workarounds, but expected by users coming
from iTerm2, WezTerm, or Windows Terminal.

**Scope:** Large. Requires:

- Pane layout engine (tree of horizontal/vertical splits)
- Per-pane PTY ownership
- Focus tracking across panes
- Keyboard shortcuts for split/navigate/resize
- Pane border rendering

**Primary files:** `freminal/src/gui/mod.rs`, new `freminal/src/gui/panes.rs`

---

### A.3 — No Search in Scrollback

Severity: Critical

There is no Ctrl+Shift+F, no search bar, no find-in-scrollback functionality. Users cannot
search terminal output history. This is a basic expectation of any terminal with scrollback.

**Scope:** Medium. Requires:

- Search UI overlay (text input + next/prev/close buttons)
- Buffer search implementation (substring and optionally regex)
- Match highlighting in the rendered output
- Scroll-to-match navigation
- Keyboard shortcuts (Ctrl+Shift+F open, Enter next, Shift+Enter prev, Escape close)

**Primary files:** `freminal/src/gui/terminal.rs`, `freminal-buffer/src/buffer.rs`,
new `freminal/src/gui/search.rs`

---

### A.4 — No Double-Click Word Selection

Severity: Critical

Clicking with the primary mouse button handles single-click selection only (press to start,
drag to extend, release to end). There is no double-click detection and no word-boundary
selection. Users expect double-click to select a word — this is a universal convention across
all text-displaying applications.

The mouse handling in `terminal.rs` (lines ~773–839) uses a simple press/release model with
no click-count tracking.

**Scope:** Small-Medium. Requires:

- Click-count tracking (timestamp-based double/triple detection)
- Word boundary detection (alphanumeric + configurable word characters)
- Selection model update to support word-granularity selection
- Drag-to-extend-by-word when initiated from a double-click

**Primary files:** `freminal/src/gui/terminal.rs`

---

### A.5 — No Triple-Click Line Selection

Severity: High

No triple-click to select an entire line. This is the standard complement to double-click word
selection and is expected by all terminal users.

**Scope:** Small. Requires the same click-count infrastructure as A.4, plus line-granularity
selection on triple-click. Should be implemented together with A.4.

**Primary files:** `freminal/src/gui/terminal.rs`

---

### A.6 — No Rectangular / Block Selection

Severity: Low

No Alt+drag rectangular selection. Standard in most terminals for selecting columnar data
(log files, tabular output). Less critical than word/line selection but expected by power users.

**Scope:** Medium. Requires:

- Alt+drag detection
- Rectangular selection model (independent start/end column per row)
- Modified copy behavior (copy rectangular region, not contiguous text)
- Visual rendering of rectangular selection highlight

**Primary files:** `freminal/src/gui/terminal.rs`

---

### A.7 — No Right-Click Context Menu

Severity: Medium

Right-click (`PointerButton::Secondary`) is only handled for X10/SGR mouse reporting to the
application (`mouse.rs` line 242). There is no context menu offering Copy, Paste, Select All,
Search, Open URL, etc.

Most terminals provide a right-click context menu. Its absence is particularly noticeable on
Linux where middle-click paste is not universal.

**Scope:** Small-Medium. Requires:

- egui context menu on right-click in the terminal area
- Menu items: Copy, Paste, Select All, Clear, Search (if A.3 exists)
- Conditional items: Open URL (when right-clicking a detected URL)

**Primary files:** `freminal/src/gui/terminal.rs`

---

### A.8 — Font Zoom Disabled

Severity: High

eframe's built-in keyboard zoom is explicitly disabled at `mod.rs` line 55:
`options.zoom_with_keyboard = false;`. There is no Ctrl+Plus / Ctrl+Minus / Ctrl+0 font size
adjustment. Users cannot change font size without editing the config file and restarting.

This is a basic accessibility and usability expectation. Every competitor supports runtime font
size changes.

**Scope:** Small-Medium. Requires:

- Re-enable or reimplement Ctrl+Plus/Minus/0 handling
- Update the glyph atlas and cell metrics on font size change (Task 1's custom renderer
  recalculates these from font metrics — need to trigger a full atlas rebuild)
- Persist the zoom level or font size to config on change
- Ensure the terminal reflows correctly after cell size changes

**Primary files:** `freminal/src/gui/mod.rs`, `freminal/src/gui/terminal.rs`,
`freminal/src/gui/renderer/` (atlas rebuild)

---

### A.9 — No Bell Handling

Severity: Medium

`TerminalOutput::Bell` is parsed by the ANSI parser and reaches `terminal_handler.rs` line
3624–3625 where it is `tracing::debug!("Bell (ignored)")`. It is never forwarded to the GUI.
No audio bell, no visual bell (screen flash), no window urgency hint.

Most terminals support at least visual bell and urgency hints. Audio bell is configurable but
expected as an option.

**Scope:** Small. Requires:

- Forward Bell event through `WindowCommand` channel to GUI
- Implement visual bell (brief background flash or border flash)
- Optionally: system audio bell via platform API
- Optionally: set window urgency hint when unfocused
- Config option: `bell = "visual" | "audio" | "none"` (default: "visual")

**Primary files:** `freminal-buffer/src/terminal_handler.rs`, `freminal/src/gui/mod.rs`,
`freminal-common/src/config.rs`

---

### A.10 — No Configurable Key Bindings

Severity: Medium

All keyboard shortcuts are hardcoded match arms in `terminal.rs` and `input.rs`. There is no
`[keybindings]` section in the config, no way to remap shortcuts without recompiling.

**Scope:** Medium-Large. Requires:

- Key binding data model (action enum + key combination struct)
- Config parser for `[keybindings]` section
- Key event dispatch that checks user bindings before falling through to defaults
- Settings modal UI for viewing/editing bindings (optional, can defer)
- Default binding set that matches current hardcoded behavior

**Primary files:** `freminal-common/src/config.rs`, `freminal/src/gui/terminal.rs`,
`freminal/src/gui/input.rs`, new `freminal-common/src/keybindings.rs`

---

### A.11 — No Drag-and-Drop

Severity: Low

No file drag-and-drop handling. Dropping a file onto the terminal window does nothing. Most
terminals paste the file path (shell-escaped) into the input stream on drop.

**Scope:** Small. Requires:

- Handle egui's `dropped_files` / `hovered_files` events in the GUI
- Shell-escape the file path(s)
- Send the escaped path(s) as `InputEvent::Key` bytes

**Primary files:** `freminal/src/gui/mod.rs` or `freminal/src/gui/terminal.rs`

---

### A.12 — SGR Underline Styles Not Rendered

Severity: Medium

`FontDecorations` has a single `Underline` variant. SGR 4:1 through 4:5 (plain, double, curly,
dotted, dashed) all map to the same plain solid underline rectangle. `Smulx` is advertised via
XTGETTCAP (`terminal_handler.rs` line 2117), which tells applications these styles are supported
when they are not visually distinguished.

Applications like `delta`, `bat`, and neovim diagnostics use curly underlines for error/warning
indicators. Rendering all styles as plain underline loses semantic information.

**Scope:** Medium. Requires:

- Extend `FontDecorations` enum with `UnderlineCurly`, `UnderlineDouble`, `UnderlineDotted`,
  `UnderlineDashed` variants (or a sub-enum)
- Parse SGR 4:N into the correct variant in `apply_sgr()`
- Thread the variant through `FormatTag` → snapshot → renderer
- Implement distinct drawing for each style in the custom renderer (curly = sine wave,
  double = two lines, dotted/dashed = dashed stroke)
- Underline color support (SGR 58/59) — already parsed? Verify.

**Primary files:** `freminal-terminal-emulator/src/ansi_components/sgr.rs`,
`freminal-common/src/colors.rs` (or format types), `freminal/src/gui/renderer/`

---

### A.13 — BCE (Background Color Erase) Not Implemented

Severity: Medium

Background Color Erase means that erase operations (ED, EL, ECH, etc.) should fill erased
cells with the current SGR background color, not the default background. Freminal's erase
operations use the default background.

This affects applications that set a background color and then clear the screen — the cleared
area should retain the background color. Programs like `vim`, `less`, and `tmux` rely on this.

Freminal does not advertise BCE in its terminfo capabilities, which is correct for the current
behavior, but implementing BCE is expected for xterm-compatible terminals.

**Scope:** Medium. Requires:

- Pass the current SGR background color to all erase operations in `Buffer`
- `clear()`, `clear_from()`, `clear_to()`, `clear_with_tag()` need to accept and apply the
  current background
- Erase operations in `terminal_handler.rs` need to forward the active background from the
  current format state
- Update terminfo to advertise BCE once implemented

**Primary files:** `freminal-buffer/src/buffer.rs`, `freminal-buffer/src/row.rs`,
`freminal-buffer/src/terminal_handler.rs`

---

### A.14 — DECDWL / DECDHL Not Rendered

Severity: Low

Double-Width Line (DECDWL, ESC # 6) and Double-Height Line (DECDHL top/bottom, ESC # 3 / # 4)
are parsed but explicitly ignored: `terminal_handler.rs` line 4059–4060 logs
`"DoubleWidthLine not yet implemented (ignored)"`.

These are legacy VT100 features. Few modern applications use them, but vttest exercises them
and some retro/artistic terminal applications depend on them.

**Scope:** Medium. Requires:

- Row-level attribute for line width/height mode
- Renderer changes to scale glyphs 2x horizontally (DECDWL) or 2x vertically (DECDHL)
- Cursor positioning adjustments (cursor column is in logical space, not physical)

**Primary files:** `freminal-buffer/src/row.rs`, `freminal-buffer/src/buffer.rs`,
`freminal/src/gui/renderer/`

---

### A.15 — KKP Flags 2/4/16 Don't Produce Output

Severity: Medium

The Kitty Keyboard Protocol implementation (Task 35, completed) correctly parses and stores
all progressive enhancement flags on a mode stack. However, flags 2 (report event types),
4 (report alternate keys), and 16 (report associated text) are stored but produce no output.

`input.rs` lines 411–414 contain an explicit comment:

> "Flags 2, 4, 16 are parsed and stored on the KKP stack but do not yet produce additional
> output fields (event-type, alternate keys, associated text). They require key event metadata
> not currently threaded through `TerminalInput`."

Only flags 1 (disambiguate escape codes) and 8 (report all keys as escape codes) produce
output. Applications that request flags 2/4/16 will not receive the expected CSI u extensions.

**Scope:** Medium. Requires:

- Thread key event metadata (press/repeat/release, shifted key, base layout key) through
  `TerminalInput` or a new `KeyEvent` struct
- Extend `to_payload_kkp` to emit the `:event-type`, `:shifted-key`, `:base-key`, and
  `;associated-text` fields when the corresponding flags are active
- Test with applications that use these flags (e.g., neovim with KKP enabled)

**Primary files:** `freminal/src/gui/input.rs`, `freminal-common/src/input.rs`

---

### A.16 — OSC 52 Clipboard Read Returns Empty

Severity: Low

OSC 52 clipboard write works (copies to system clipboard). OSC 52 clipboard read (query)
responds with an empty payload. `mod.rs` lines 584–588 log:
`"OSC 52 query for selection '{sel}' — responding empty"`.

The doc comment on `WindowManipulation` explains: "QueryClipboard responds with an empty OSC 52
payload because egui's public API does not support reading the clipboard."

This affects applications that use OSC 52 to read the clipboard (e.g., some neovim clipboard
providers, tmux `set-clipboard`).

**Scope:** Small. Requires:

- Use `arboard` or `copypasta` crate to read the system clipboard directly (bypassing egui)
- Security consideration: clipboard read should be gated behind a config option
  (`allow_clipboard_read = false` by default) since it can leak sensitive data
- Return base64-encoded clipboard contents in the OSC 52 response

**Primary files:** `freminal/src/gui/mod.rs`, `freminal-common/src/config.rs`

---

## Category B — Competitor Differentiation Features

These are features present in one or more major competitors (WezTerm, Ghostty, Kitty) that
elevate a terminal from "functional" to "compelling." None are strictly required, but each
adds significant value for specific user segments.

---

### B.1 — Built-in Multiplexer / Remote Mux

**Severity: Low** | **Reference: WezTerm**

WezTerm includes a built-in multiplexer that supports remote sessions (SSH + mux protocol),
eliminating the need for tmux in many workflows. This is WezTerm's signature differentiator.

**Scope:** Very Large. This is a major architectural feature that would require a mux protocol,
session persistence, and SSH transport. Recommend deferring until core features are solid.

---

### B.2 — Command Palette

**Severity: Low** | **Reference: WezTerm**

A searchable command palette (Ctrl+Shift+P or similar) listing all available actions. Useful
for discoverability and for users who prefer keyboard-driven workflows.

**Scope:** Medium. Requires:

- Action registry (all keybinding-able actions as an enum)
- Fuzzy search UI (text input + filtered list)
- Action dispatch

**Primary files:** `freminal/src/gui/mod.rs`, new `freminal/src/gui/command_palette.rs`

---

### B.3 — Quick-Select / Hints Mode

**Severity: Low** | **Reference: WezTerm, Kitty**

A mode where detected patterns (URLs, file paths, git hashes, IP addresses) are highlighted
with letter labels. Typing the label copies or opens the target. Eliminates mouse usage for
common selection tasks.

**Scope:** Medium. Requires:

- Pattern detection engine (regex-based, configurable)
- Overlay rendering with labels
- Label dispatch (copy to clipboard, open URL, etc.)

---

### B.4 — Multiple Windows

**Severity: Low** | **Reference: WezTerm, Ghostty**

Ability to open multiple OS windows from a single Freminal instance, sharing configuration
and theme state. Currently each Freminal invocation is independent.

**Scope:** Large. Requires rethinking the application lifecycle model.

---

### B.5 — Background Images

**Severity: Low** | **Reference: WezTerm**

Configurable background image behind the terminal grid, with opacity/blur controls.
Background opacity (Task 34) is already implemented — background images are the natural
extension.

**Scope:** Medium. The custom renderer (Task 1) already has a GL pipeline; adding a texture
quad behind the terminal grid is straightforward.

**Primary files:** `freminal/src/gui/renderer/`, `freminal-common/src/config.rs`

---

### B.6 — Custom Shaders

**Severity: Low** | **Reference: WezTerm, Ghostty**

User-provided GLSL fragment shaders for post-processing effects (CRT scanlines, bloom, color
grading). The custom glow renderer (Task 1) makes this feasible.

**Scope:** Medium. Requires:

- Shader loading from config path
- Render-to-texture pipeline (render terminal to FBO, then apply post-processing shader)
- Hot-reload on shader file change

**Primary files:** `freminal/src/gui/renderer/`, `freminal-common/src/config.rs`

---

### B.7 — SSH Integration

**Severity: Low** | **Reference: WezTerm**

Direct SSH connection from the terminal (connection dialog, key management, session
persistence) without requiring an external SSH client. Often paired with the multiplexer
(B.1).

**Scope:** Very Large. Recommend deferring.

---

### B.8 — IME Support for CJK Input

**Severity: Medium** | **Reference: WezTerm, Ghostty, Kitty**

Input Method Editor support for Chinese, Japanese, Korean text input. This is a hard
requirement for CJK users — without it, Freminal is unusable for a significant portion of
the global developer population.

egui has partial IME support. The degree to which it works with Freminal's custom renderer
needs investigation.

**Scope:** Medium-Large. Requires:

- Verify egui's IME events are correctly forwarded
- Position the IME candidate window at the cursor location
- Handle pre-edit (composing) text display
- Wide character (fullwidth) cell handling in the buffer

**Primary files:** `freminal/src/gui/terminal.rs`, `freminal/src/gui/input.rs`

---

### B.9 — Password Input Detection

**Severity: Low** | **Reference: WezTerm**

Detect when the terminal is likely prompting for a password (based on prompt patterns or
tty echo-off mode) and suppress scrollback recording, disable selection copy, and optionally
show a visual indicator.

**Scope:** Small. Requires:

- Detect tty echo-off mode (already tracked in terminal state?)
- Suppress scrollback append during echo-off
- Visual indicator (lock icon or similar)

---

### B.10 — Session Restore / Startup Commands

**Severity: Low** | **Reference: WezTerm, Ghostty**

Configurable startup commands per tab/pane (e.g., auto-SSH to a server, cd to a project,
run a command). Session layouts that restore a multi-tab/pane arrangement.

**Scope:** Medium. Depends on tabs (A.1) and optionally panes (A.2) being implemented first.

---

### B.11 — Cursor Trail / Smooth Cursor

**Severity: Low** | **Reference: Ghostty**

Animated cursor movement — the cursor smoothly interpolates between positions instead of
jumping instantly. A visual polish feature that Ghostty popularized.

**Scope:** Small-Medium. Requires:

- Cursor animation state (current position, target position, interpolation progress)
- Animation timer driving repaints during cursor transitions
- Config option to enable/disable

**Primary files:** `freminal/src/gui/renderer/`, `freminal/src/gui/view_state.rs`

---

### B.12 — Adaptive Light/Dark Theming

**Severity: Low** | **Reference: WezTerm, Ghostty**

Automatic theme switching based on the OS light/dark mode preference. DECRQM mode `?2031`
exists in the codebase but it is unclear whether the GUI responds to OS theme changes by
switching the terminal color palette.

**Scope:** Small. Requires:

- Detect OS dark/light preference (egui provides this via `Visuals`)
- Map to a configured light theme and dark theme
- Switch palette when OS preference changes
- Report via `?2031` mode response

**Primary files:** `freminal/src/gui/mod.rs`, `freminal-common/src/config.rs`

---

## Category C — Remaining Master Plan Tasks

These are already tracked in `Documents/MASTER_PLAN.md` and their respective plan documents.
They are listed here for completeness so this document provides a single unified view of all
outstanding work.

---

### C.1 — Performance Plan Task 11: Dead Code Cleanup

**Plan:** `Documents/PERFORMANCE_PLAN.md` — Task 11

Delete dead code left over from the FairMutex elimination. Remove unused imports, types, and
methods that were part of the old locking architecture. Run clippy and machete to verify.

**Status:** Unchecked in PERFORMANCE_PLAN.md.

---

### C.2 — Task 18: Client-Side Update Mechanism

**Plan:** `Documents/PLAN_18_UPDATE_MECHANISM.md`

Background HTTP check against `updates.freminal.dev`, version comparison, menu bar indicator,
and modal dialog for downloading updates.

**Status:** Pending. Depends on Tasks 2, 3, 16 (all complete).

---

### C.3 — Task 19: Update Service & Website

**Plan:** `Documents/PLAN_19_UPDATE_SERVICE_AND_WEBSITE.md`

Cloudflare Worker at `updates.freminal.dev` proxying GitHub Releases API with KV cache, plus
a project website at `freminal.dev`. Separate repository.

**Status:** Pending. Independent of the main repo.

---

### C.4 — Task 29: God File Refactoring

**Plan:** `Documents/PLAN_29_GOD_FILE_REFACTOR.md`

Split `terminal_handler.rs` (~9,098 lines) and `buffer.rs` (~6,624 lines) into focused,
single-responsibility modules. Must be done last to avoid merge conflicts with all other work.

**Status:** Stub. Depends on all other tasks completing first.

---

### C.5 — Task 32: Playback Feature Flag

**Plan:** `Documents/PLAN_32_PLAYBACK_FEATURE_FLAG.md`

Gate the existing playback/recording feature behind a Cargo feature flag (`playback`) not
enabled by default.

**Status:** Stub. Requires audit of coupling surface first.

---

## Priority Recommendations

### Immediate (table-stakes — implement before any public release)

1. **A.1 — Tabs** (Critical)
2. **A.3 — Search in Scrollback** (Critical)
3. **A.4 + A.5 — Double/Triple-Click Selection** (Critical + High, implement together)
4. **A.8 — Font Zoom** (High)

### Near-term (significant daily-use friction)

1. **A.7 — Right-Click Context Menu** (Medium)
2. **A.9 — Bell Handling** (Medium)
3. **A.12 — SGR Underline Styles** (Medium)
4. **A.13 — BCE** (Medium)
5. **A.15 — KKP Flags 2/4/16** (Medium)
6. **B.8 — IME Support** (Medium — critical for CJK users)

### Medium-term (polish and power-user features)

1. **A.10 — Configurable Key Bindings** (Medium)
2. **A.2 — Split Panes** (Medium)
3. **A.6 — Rectangular Selection** (Low)
4. **A.11 — Drag-and-Drop** (Low)
5. **A.16 — OSC 52 Clipboard Read** (Low)
6. **A.14 — DECDWL/DECDHL** (Low)
7. **B.2 — Command Palette** (Low)
8. **B.3 — Quick-Select / Hints** (Low)
9. **B.12 — Adaptive Theming** (Low)

### Long-term (differentiation and completeness)

1. **B.5 — Background Images** (Low)
2. **B.6 — Custom Shaders** (Low)
3. **B.9 — Password Detection** (Low)
4. **B.10 — Session Restore** (Low, depends on A.1)
5. **B.11 — Cursor Trail** (Low)
6. **B.4 — Multiple Windows** (Low)
7. **B.1 — Built-in Multiplexer** (Low)
8. **B.7 — SSH Integration** (Low)

### Housekeeping (existing tracked work)

1. **C.1 — Dead Code Cleanup**
2. **C.2 — Update Mechanism**
3. **C.3 — Update Service**
4. **C.4 — God File Refactoring**
5. **C.5 — Playback Feature Flag**

---

## Notes

- The escape sequence and VT compliance is genuinely strong — 289+ vttest tests passing, 12
  DEC private modes implemented, full Kitty Keyboard Protocol, inline images (iTerm2 + Kitty),
  extensive SGR coverage. The gaps above are almost entirely on the UX/GUI side, not the
  terminal emulation side.
- The lock-free architecture (ArcSwap + channels) established in the performance refactor is
  well-suited for tabs and panes — each tab/pane would own its own PTY thread and ArcSwap,
  and the GUI thread would load snapshots from whichever tab/pane is currently visible.
- A separate agent is working on hot-path rendering optimizations (Section 10 of
  PERFORMANCE_PLAN.md). That work is complementary to everything listed here and should not
  be duplicated.
