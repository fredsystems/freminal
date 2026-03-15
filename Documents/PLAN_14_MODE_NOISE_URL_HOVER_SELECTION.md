# PLAN_14 — Mode Debug Noise, URL Hover, Scrollback Selection

## Status: In Progress

---

## Overview

This task addresses three categories of issues discovered during daily use:

1. **Spurious debug log messages** from mode dispatch catch-all arms (Issues #1, #2, #4)
2. **Missing URL hover detection** in snapshot mode (Issue #3)
3. **Broken scrollback text selection** (Issue #6)

Issue #5 (tmux weirdness) is deferred and out of scope for this task.

All work is done on a single feature branch (`task-14/mode-noise-url-hover-selection`) with one
atomic commit per fix.

---

## Fix A — Make Mode Match Blocks Exhaustive

### Make Mode Match Blocks Exhaustive Problem

The terminal has a two-layer mode dispatch architecture:

- **TerminalHandler** (`freminal-buffer/src/terminal_handler.rs:2362-2505`) handles buffer-level
  modes: DECAWM, DECTCEM, alt screen, DECOM, DECCOLM, XtCBlink, LNM, AllowColumnModeSwitch,
  UnknownQuery.
- **TerminalState mode-sync** (`freminal-terminal-emulator/src/state/internal.rs:340-422`) handles
  GUI/input-concern modes: DECCKM, BracketedPaste, MouseMode, FocusReporting, DECSCNM, DECARM,
  ReverseWrapAround, SynchronizedUpdates, LNM (sync copy).

Both layers iterate the same `parsed` output vec. Each has a catch-all `other =>` arm that logs
a `debug!` message. Modes correctly handled by one layer produce spurious messages from the other
layer's catch-all. All three reported modes (Bracketed Paste, DECCKM, DECAWM) are fully
functional — the messages are pure noise.

### Make Mode Match Blocks Exhaustive Fix

Replace each catch-all `other =>` arm with explicit no-op arms for every mode variant that is
intentionally handled by the other layer. The catch-all is narrowed to only fire for genuinely
unhandled modes (`NoOp`, `Decsclm`, `GraphemeClustering`, `Theming`, `Unknown`).

### Make Mode Match Blocks Exhaustive Files

- `freminal-buffer/src/terminal_handler.rs` — line ~2500: add explicit no-op arms for modes
  handled by TerminalState (Decckm, BracketedPaste, MouseMode, XtMseWin, Decscnm, Decarm,
  ReverseWrapAround, SynchronizedUpdates).
- `freminal-terminal-emulator/src/state/internal.rs` — line ~419: add explicit no-op arms for
  modes handled by TerminalHandler (XtExtscrn, AltScreen47, SaveCursor1048, Decawm, Dectem,
  XtCBlink, Decom, Deccolm, AllowColumnModeSwitch, UnknownQuery).

### Make Mode Match Blocks Exhaustive Verification

- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- No `debug!` messages for modes that are intentionally handled elsewhere

---

## Fix B — Restore URL Hover Detection

### Restore URL Hover Detection Problem

After the performance refactor moved rendering to snapshots, URL hover detection was left as a
TODO stub (`terminal.rs:1384-1397`). The stub logs "No URL hover detection in snapshot mode yet"
on every frame when the mouse is over the terminal. Dead stubs remain in `interface.rs:468-469`
and `internal.rs:182-187`.

### Design

All data is already present in the snapshot pipeline:

- `FormatTag` carries `url: Option<Url>` (from `freminal-common/src/buffer_states/format_tag.rs`)
- The shaping pipeline propagates URLs: `TextRun.url` → `ShapedRun.url`
- `view_state.mouse_position` is populated from pointer events
- `encode_egui_mouse_pos_as_usize` converts pixel position to `(col, row)` cell coordinates

The fix is purely GUI-side: convert mouse position to cell coordinates, walk
`snap.visible_tags` to find the tag covering that cell, check for `url: Some(...)`,
set cursor icon to `CursorIcon::PointingHand`, and on click open the URL.

### Implementation

1. In `terminal.rs`, replace the TODO stub at lines 1384-1397 with:
   - Convert `mouse_position` to `(col, row)` using `encode_egui_mouse_pos_as_usize`
   - Compute flat index: walk `visible_chars` counting `TChar::NewLine` boundaries to find
     the character offset for `(col, row)`
   - Find the `FormatTag` in `visible_tags` whose `start..end` range covers that offset
   - If `tag.url.is_some()`, set `CursorIcon::PointingHand`; otherwise `CursorIcon::Default`
   - On primary click with Ctrl held (or Cmd on macOS), open the URL via `open::that()`

2. Remove dead stubs:
   - `interface.rs:468-469` (`is_mouse_hovered_on_url`)
   - `internal.rs:182-187` (`is_mouse_hovered_on_url`)

### Restore URL Hover Detection Files

- `freminal/src/gui/terminal.rs` — replace TODO stub, add URL lookup helper
- `freminal-terminal-emulator/src/interface.rs` — remove dead method
- `freminal-terminal-emulator/src/state/internal.rs` — remove dead method

### Restore URL Hover Detection Verification

- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo-machete`
- Manual: hover over an OSC 8 hyperlink — cursor becomes pointing hand; Ctrl+click opens it

---

## Fix C — Fix Scrollback Text Selection

### Fix Scrollback Text Selection Problem

Text selection in scrollback is broken in multiple interacting ways:

1. **B1 — Screen-relative coordinates**: `CellCoord.row` is screen-relative (0..height-1). When
   `scroll_offset` changes, `visible_chars` shows different rows but selection coordinates stay
   the same, causing the highlight to cover wrong text.

2. **B2 — Selection cleared on scroll**: Every scroll event invalidates the snapshot cache
   (`previous_visible_snap = None` in `interface.rs:668-671`), causing `content_changed = true`.
   At `terminal.rs:1180`, `if snap.content_changed && !view_state.selection.is_selecting` clears
   the selection. So scrolling while not actively dragging always clears the selection.

3. **B3 — PTY auto-scroll clears selection**: New PTY output resets `gui_scroll_offset = 0`
   (`interface.rs:487`), triggering the same `content_changed` → clear path.

4. **B4 — Coordinates not rebased on scroll**: Anchor/end coords are not adjusted when scroll
   offset changes, so the selection start visually "moves" during scrolling.

5. **B5 — `extract_selected_text` uses screen-local rows**: The extraction function indexes into
   `visible_chars` using screen-relative row indices that may be from a stale scroll context.

### Fix Scrollback Text Selection Design

The core fix is to make selection coordinates **buffer-absolute** rather than screen-relative.

**Buffer-absolute row** = `visible_window_start + screen_row`, where
`visible_window_start = total_rows - height - scroll_offset`.

This means:

- When the user clicks at screen row 5 with `scroll_offset=10`, the stored row is
  `total_rows - height - 10 + 5`, which identifies a specific buffer row regardless of scrolling.
- Selection coordinates survive scroll changes because they refer to fixed buffer positions.
- To render the selection, convert buffer-absolute back to screen-relative:
  `screen_row = absolute_row - visible_window_start`.

### Fix Scrollback Text Selection Implementation

1. **Add `total_rows` to snapshot** (already present — `snap.height` and we can compute
   `visible_window_start` from `snap.scroll_offset`, `snap.height`, and the row count in
   `visible_chars`).

   Actually, we need the total row count. Add a `total_rows: usize` field to `TerminalSnapshot`
   if not already present. Check: it's not there. We need it. Add it.

2. **Make selection coordinates buffer-absolute** in `terminal.rs`:
   - On mouse-down (line 687): `row = visible_window_start(snap) + y`
   - On drag (line 649): same conversion
   - On mouse-up (line 693): same conversion

3. **Guard selection clearing** in `terminal.ts:1180`:
   - Don't clear selection on `content_changed` when the change is purely a scroll offset change.
   - Add a check: only clear when `content_changed` is true AND the content actually changed
     (not just scroll-triggered cache invalidation).
   - Strategy: the snapshot already has `content_changed` which is `false` for scroll-only
     changes since 9.5-C (the dirty-row tracking). However, the scroll cache invalidation at
     `interface.rs:668-671` forces `previous_visible_snap = None`, which makes the next
     `build_snapshot` treat all rows as "first-ever" and set `content_changed = true`.
   - **Fix the root cause**: instead of clearing `previous_visible_snap` on scroll offset
     change, let the dirty-row system handle it. When scroll offset changes, the visible
     rows are different, but the per-row cache already handles this — the new visible rows
     may be clean (already cached) or dirty. Remove the scroll-offset invalidation from
     `build_snapshot`.
   - Actually, the per-row cache indices are tied to buffer row indices, and
     `visible_as_tchars_and_tags` takes `scroll_offset` as a parameter to select which rows
     to flatten. The row cache is per-buffer-row, so different scroll offsets can reuse cached
     rows. The `previous_visible_snap` comparison is what detects `content_changed` — if we
     remove the invalidation, and the new visible window has all clean rows, we'll reuse the
     previous snap's Arcs... but those Arcs contain the OLD visible window's data. That's wrong.
   - **Correct approach**: Keep the scroll-offset invalidation, but DON'T let scroll-triggered
     `content_changed` clear the selection. The selection coordinates are buffer-absolute, so
     they remain valid across scroll changes. Only clear when **actual PTY output** changes
     content (which shifts buffer rows and invalidates selection coordinates).
   - Distinguish scroll-triggered content changes from PTY-triggered content changes by adding
     a `scroll_changed: bool` field to `TerminalSnapshot`. When `content_changed && !scroll_changed`,
     clear the selection.

4. **Convert buffer-absolute to screen-relative for rendering** in `terminal.ts:1270`:
   - `current_selection.map(|(s, e)| ...)` currently passes raw `CellCoord` row values.
   - Compute `visible_window_start` and subtract it from each row before passing to the renderer.
   - Clamp: if a selection endpoint is outside the visible window, clamp to row 0 or height-1.

5. **Fix `extract_selected_text`** in `terminal.ts:836-897`:
   - Convert buffer-absolute coords to screen-relative before indexing into `visible_chars`.
   - Or, since we're passing `visible_chars` which is the current window's content, convert
     the selection range to screen-relative first.

### Fix Scrollback Text Selection Files

- `freminal-terminal-emulator/src/snapshot.rs` — add `total_rows` and `scroll_changed` fields
- `freminal-terminal-emulator/src/interface.rs` — populate `total_rows` and `scroll_changed`
- `freminal/src/gui/terminal.rs` — buffer-absolute coords, selection clearing guard, rendering
  conversion, `extract_selected_text` fix
- `freminal/src/gui/view_state.rs` — no structural changes to `SelectionState` needed

### Fix Scrollback Text Selection Verification

- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo-machete`
- Manual: select text, scroll — selection stays on the same text; copy works after scrolling;
  new PTY output clears stale selection; selection in scrollback copies correct text

---

## Checklist

- [ ] Fix A — Make mode match blocks exhaustive
- [ ] Fix B — Restore URL hover detection
- [ ] Fix C — Fix scrollback text selection
- [ ] Full verification suite passes
