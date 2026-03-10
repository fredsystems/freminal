# PLAN_08 — Primary Screen Scrollback

## Overview

Wire the user's scroll offset from the GUI thread into the PTY thread so that
primary-screen scrollback renders correctly. The alternate-screen scroll
(arrow-key fallback for less/vim/htop) already works; this task covers only the
primary screen where historical output scrolls off the top.

**Dependencies:** None (the `Buffer` layer already supports offset-based
rendering)
**Dependents:** None
**Primary crates:** `freminal` (GUI), `freminal-terminal-emulator`,
`freminal-buffer`
**Estimated scope:** Medium (7 subtasks)

---

## Problem Statement

The GUI stores a `scroll_offset` in `ViewState`, but nothing ever feeds it back
to the PTY thread. `build_snapshot()` always flattens the visible window at
offset 0, so the user sees only the most recent screenful of output. Scroll
events in the primary screen update `ViewState.scroll_offset` but the snapshot
is oblivious to the change, producing no visual effect.

### Root Cause Chain

| Component                            | File                                          | Issue                                                              |
| ------------------------------------ | --------------------------------------------- | ------------------------------------------------------------------ |
| `ViewState::scroll_offset`           | `freminal/src/gui/view_state.rs`              | Stores scroll position; nothing reads it for rendering             |
| `build_snapshot()`                   | `freminal-terminal-emulator/src/interface.rs` | Always builds snapshot at offset 0; has no access to `ViewState`   |
| `data_and_format_data_for_gui()`     | `freminal-buffer/src/terminal_handler.rs`     | Hardcodes `visible_as_tchars_and_tags(0)`                          |
| `render_terminal_output()`           | `freminal/src/gui/terminal.rs`                | Paints `snap.visible_chars` verbatim; does not apply scroll offset |
| `visible_rows(offset)`               | `freminal-buffer/src/buffer.rs`               | Correctly implemented, never called with non-zero offset           |
| `visible_as_tchars_and_tags(offset)` | `freminal-buffer/src/buffer.rs`               | Correctly implemented, only called with 0                          |
| `scroll_back()` / `scroll_forward()` | `freminal-buffer/src/buffer.rs`               | Pure arithmetic, return new offset — callers discard results       |

### Key Insight

The `Buffer` layer is fully scroll-aware. `visible_rows(offset)` and
`visible_as_tchars_and_tags(offset)` already do the right thing. The fix is
purely a wiring problem: the scroll offset must flow from the GUI into the
snapshot pipeline, following the same pattern as resize events (`InputEvent`
channel).

### Design Principle

The GUI must **never** be involved in row flattening or buffer slicing. The
snapshot must contain exactly what needs to be drawn on screen — nothing more.
The PTY thread owns the buffer and produces snapshots at the correct offset.

---

## Architecture

### Data Flow (Current — Broken)

```text
GUI: scroll wheel → ViewState.scroll_offset += delta
                     (dead end — nothing reads this for rendering)

PTY: build_snapshot() → visible_as_tchars_and_tags(0) → snapshot
GUI: render(snapshot)  → always shows offset 0
```

### Data Flow (Target)

```text
GUI: scroll wheel → send InputEvent::ScrollOffset(offset) via channel
                  → also update ViewState.scroll_offset for local bookkeeping

PTY: receive InputEvent::ScrollOffset(offset)
   → store self.gui_scroll_offset = offset
   → build_snapshot() uses gui_scroll_offset
   → visible_as_tchars_and_tags(gui_scroll_offset)
   → snapshot contains the scrolled-back content

GUI: render(snapshot)  → shows correct historical content
```

### Scroll Offset Reset

When new output arrives from the child process and the user is scrolled back,
the terminal should auto-scroll to the bottom (offset 0). This matches the
behavior of every major terminal emulator. The PTY thread resets
`gui_scroll_offset = 0` when it processes new data and the current offset is
non-zero. The GUI detects this via the snapshot (e.g., a `scroll_offset` field
in `TerminalSnapshot`) and updates `ViewState` accordingly.

---

## Subtasks

### 8.1 — Add `InputEvent::ScrollOffset` variant

**File:** `freminal-terminal-emulator/src/io/mod.rs`

Add `ScrollOffset(usize)` to the `InputEvent` enum. This carries the desired
scroll offset (in rows from the bottom, 0 = latest) from the GUI to the PTY
thread.

**Acceptance criteria:**

- `InputEvent::ScrollOffset(usize)` compiles
- Existing `InputEvent` match arms still work (non-exhaustive pattern will
  force updates)

---

### 8.2 — Store scroll offset in `TerminalEmulator`

**File:** `freminal-terminal-emulator/src/interface.rs`

Add a `gui_scroll_offset: usize` field to `TerminalEmulator`. Initialize to 0.
Handle `InputEvent::ScrollOffset(n)` in the PTY event loop by updating this
field.

**Acceptance criteria:**

- Field exists, initialized to 0 in both `new()` and `dummy_for_bench()`
- PTY event loop dispatches `InputEvent::ScrollOffset` to update the field

---

### 8.3 — Thread scroll offset through `build_snapshot()`

**Files:**

- `freminal-terminal-emulator/src/interface.rs` — `build_snapshot()`
- `freminal-buffer/src/terminal_handler.rs` — `data_and_format_data_for_gui()`

Pass `self.gui_scroll_offset` into `data_and_format_data_for_gui()` which
forwards it to `visible_as_tchars_and_tags(offset)`. The snapshot now
contains content at the user's scroll position.

Also update `any_visible_dirty()` to accept the offset so the dirty-check
window matches the rendered window.

**Acceptance criteria:**

- `data_and_format_data_for_gui()` takes an `offset: usize` parameter
- `visible_as_tchars_and_tags(offset)` is called with the real offset
- `any_visible_dirty(offset)` is called with the real offset
- When `gui_scroll_offset > 0`, the snapshot contains historical rows

---

### 8.4 — Add `scroll_offset` to `TerminalSnapshot`

**File:** `freminal-terminal-emulator/src/snapshot.rs`

Add `scroll_offset: usize` to the snapshot struct. The GUI reads this to know
the current scroll position (useful for scroll indicators, auto-scroll-to-bottom
detection, etc.).

**Acceptance criteria:**

- `TerminalSnapshot::scroll_offset` exists
- `build_snapshot()` populates it from `self.gui_scroll_offset`
- `TerminalSnapshot::empty()` sets it to 0

---

### 8.5 — Send scroll events from GUI to PTY thread

**File:** `freminal/src/gui/terminal.rs` — `handle_scroll_fallback()` and
callers

When `is_alternate_screen` is false, compute the new scroll offset using
`Buffer::scroll_back()` / `scroll_forward()` arithmetic (clamped to
`total_rows - visible_height`), and send `InputEvent::ScrollOffset(new_offset)`
through the input channel.

Note: The GUI needs to know `total_rows` to clamp the offset. Either:

- (a) Add `total_rows` to `TerminalSnapshot` (cheap — one `usize`), or
- (b) Compute the max offset as `total_rows.saturating_sub(visible_height)` on
  the PTY side and clamp there.

Option (b) is preferred (keeps buffer knowledge out of the GUI).

**Acceptance criteria:**

- Scroll wheel in primary screen sends `InputEvent::ScrollOffset` with correct
  offset
- Offset is clamped: cannot exceed `total_rows - visible_height`, cannot go
  below 0
- Alternate screen path unchanged (still sends arrow keys)

---

### 8.6 — Auto-scroll to bottom on new output

**Files:**

- `freminal-terminal-emulator/src/interface.rs` — PTY data processing
- `freminal/src/gui/terminal.rs` — GUI-side `ViewState` sync

When the PTY thread receives new data from the child process and
`gui_scroll_offset > 0`, reset it to 0. The next snapshot will be at offset 0,
and the GUI reads `snap.scroll_offset` to sync `ViewState`.

**Acceptance criteria:**

- New output from child auto-scrolls to bottom
- Manual scroll position is preserved when no new output arrives
- GUI's `ViewState.scroll_offset` stays in sync with `snap.scroll_offset`

---

### 8.7 — Scroll position indicator (stretch goal)

**File:** `freminal/src/gui/terminal.rs`

Optionally render a visual indicator when the user is scrolled back (e.g., a
thin bar showing position, or a "[scrolled]" badge). This is cosmetic and can
be deferred.

**Acceptance criteria:**

- Some visual feedback when `scroll_offset > 0`
- Indicator disappears when back at bottom

---

## Verification

- `cargo test --all` passes
- `cargo clippy --all-targets --all-features -- -D warnings` clean
- `cargo-machete` clean
- Manual test: `seq 1 10000` then scroll up/down in primary screen
- Manual test: `less /etc/services` scroll still works (alternate screen)
- Manual test: New output auto-scrolls to bottom
- Before/after benchmarks for `build_snapshot()` (should be faster without
  the removed `rows` clone)

---

## Files Modified (Expected)

| File                                          | Changes                                                                   |
| --------------------------------------------- | ------------------------------------------------------------------------- |
| `freminal-terminal-emulator/src/io/mod.rs`    | Add `ScrollOffset(usize)` to `InputEvent`                                 |
| `freminal-terminal-emulator/src/interface.rs` | Add `gui_scroll_offset` field; thread through `build_snapshot()`          |
| `freminal-terminal-emulator/src/snapshot.rs`  | Add `scroll_offset: usize`                                                |
| `freminal-buffer/src/terminal_handler.rs`     | `data_and_format_data_for_gui()` takes offset param                       |
| `freminal/src/gui/terminal.rs`                | Send `InputEvent::ScrollOffset` on scroll; sync `ViewState` from snapshot |
| `freminal/src/gui/view_state.rs`              | Possibly simplify (offset may be derived from snapshot)                   |
| `freminal/src/main.rs`                        | Handle `InputEvent::ScrollOffset` in PTY event loop                       |
