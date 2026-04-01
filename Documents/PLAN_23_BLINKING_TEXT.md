# PLAN_23 — Blinking Text (SGR 5/6)

## Status: Pending

---

## Overview

SGR 5 (slow blink) and SGR 6 (fast blink) are parsed correctly by Freminal's SGR parser but
silently discarded in `apply_sgr()`. There is no `Blink` variant in `FontDecorations`, no field
in `FormatTag` to carry blink state, and no rendering code to animate blinking text.

This task implements blinking text rendering end-to-end, from the SGR parser through to the GPU
renderer. The cursor blink implementation (timer, repaint scheduling, renderer fast path) serves
as a direct blueprint.

**Dependencies:** None (independent)
**Dependents:** Task 22 (vttest Menu 2.10 blinking text test can be updated after this)
**Primary crates:** `freminal-common`, `freminal-buffer`, `freminal-terminal-emulator`, `freminal`
**Estimated scope:** Medium (7 subtasks)

---

## Current State

### What Exists

- `SelectGraphicRendition::SlowBlink` (SGR 5) and `FastBlink` (SGR 6) are parsed in
  `freminal-common/src/sgr.rs`.
- `SelectGraphicRendition::NotBlinking` (SGR 25) is also parsed.
- In `apply_sgr()` (`freminal-buffer/src/terminal_handler.rs:3985-3990`), all three are in
  a match arm grouped with "intentionally ignored" attributes — they are silently dropped.

### What Does NOT Exist

- No `Blink` variant in `FontDecorations` (`freminal-common/src/buffer_states/fonts.rs`).
- No `blink` field in `FormatTag` (`freminal-common/src/buffer_states/format_tag.rs`).
- No blink state in `ShapedRun` or the rendering pipeline.
- No text blink timer or repaint scheduling for text blink.
- No renderer code to show/hide blinking text.

### Cursor Blink (Blueprint)

The existing cursor blink implementation provides the architectural pattern:

1. **Timer:** `FreminalGui` tracks `cursor_blink_timer: Instant` and `cursor_visible: bool`.
2. **Repaint scheduling:** In `update()`, if the cursor should be blinking, a
   `request_repaint_after(Duration)` is scheduled for the next blink toggle.
3. **Renderer:** The cursor rendering code checks `cursor_visible` and skips drawing the
   cursor when it should be hidden.
4. **Reset:** Cursor blink timer resets on keyboard input (cursor stays visible while typing).

Text blink follows the same pattern but at the `FormatTag` → `ShapedRun` → renderer level
rather than the cursor-specific path.

---

## Design

### Blink State Representation

Add a `blink` field to `FormatTag`:

```rust
pub enum BlinkState {
    None,
    Slow,  // SGR 5 — ~1 Hz (500ms on, 500ms off)
    Fast,  // SGR 6 — ~3 Hz (167ms on, 167ms off)
}
```

`FormatTag` already carries font decoration state. Adding `blink: BlinkState` is a natural
extension. Default is `BlinkState::None`.

### Snapshot Transport

`TerminalSnapshot` needs a flag indicating whether ANY visible cell has blink state, so the GUI
knows whether to schedule blink-rate repaints:

```rust
pub has_blinking_text: bool,
```

This avoids scheduling unnecessary repaints when no text is blinking (the common case).

### Rendering

The renderer receives a `text_blink_visible: bool` flag (toggled by the GUI timer). For cells
with `BlinkState::Slow` or `BlinkState::Fast`:

- When `text_blink_visible` is `true`: render normally.
- When `text_blink_visible` is `false`: render the cell background but skip the glyph.

The two blink rates (slow ~1 Hz, fast ~3 Hz) can be handled with separate timers or a single
timer at the fast rate where slow-blink cells only toggle on every 3rd cycle. The single-timer
approach is simpler.

### Repaint Scheduling

When `has_blinking_text` is true in the current snapshot:

- Schedule `request_repaint_after(Duration::from_millis(167))` for ~3 Hz refresh.
- Maintain a `text_blink_cycle: u8` counter (0-5) that increments each tick.
- Slow blink: visible when `cycle < 3`, hidden when `cycle >= 3` (500ms/500ms at 167ms ticks).
- Fast blink: visible when `cycle % 2 == 0`, hidden when `cycle % 2 == 1`.

---

## Subtasks

---

### 23.1 — Add `BlinkState` to `FormatTag`

- **Status:** Done (2026-03-31)
- **Priority:** 1 — High
- **Scope:** `freminal-common/src/buffer_states/format_tag.rs`,
  `freminal-common/src/buffer_states/fonts.rs`
- **Details:**
  1. Add `BlinkState` enum (`None`, `Slow`, `Fast`) to `fonts.rs`.
  2. Add `pub blink: BlinkState` field to `FormatTag`.
  3. Default value is `BlinkState::None`.
  4. Update `FormatTag::new()` and any builders to include the new field.
  5. Update `FormatTag::eq` / `PartialEq` to include `blink`.

- **Acceptance criteria:**
  - `FormatTag` has a `blink` field.
  - All existing tests compile and pass (new field is `None` everywhere by default).
  - `cargo test --all` passes.
- **Tests required:**
  - `FormatTag` with different `BlinkState` values are not equal.
  - Default `FormatTag` has `BlinkState::None`.
- **Completion notes:**
  - Added `BlinkState` enum with `Debug, Clone, Copy, Eq, PartialEq, Default` derives.
  - Added `pub blink: BlinkState` to `FormatTag`, defaulting to `BlinkState::None`.
  - Updated all explicit `FormatTag` struct literals in `buffer.rs`, `shaping.rs`,
    `buffer_row_bench.rs` to include `blink: BlinkState::None`.
  - Updated `tags_same_format()` in `buffer.rs` to compare `blink` field.
  - Added 5 unit tests in `format_tag.rs`. All verification passes.

---

### 23.2 — Wire `apply_sgr()` for SGR 5/6/25

- **Status:** Done (2026-03-31)
- **Priority:** 1 — High
- **Scope:** `freminal-buffer/src/terminal_handler.rs`
- **Details:**
  In `apply_sgr()` (lines 3985-3990), move `SlowBlink`, `FastBlink`, and `NotBlinking` out
  of the "intentionally ignored" group. Map them to the current format tag's `blink` field:
  - `SlowBlink` → `current_tag.blink = BlinkState::Slow`
  - `FastBlink` → `current_tag.blink = BlinkState::Fast`
  - `NotBlinking` → `current_tag.blink = BlinkState::None`

  Also ensure `SGR 0` (reset) sets `blink` back to `None`.

- **Acceptance criteria:**
  - `ESC[5m` sets `BlinkState::Slow` on subsequent text.
  - `ESC[6m` sets `BlinkState::Fast` on subsequent text.
  - `ESC[25m` clears blink state.
  - `ESC[0m` clears blink state.
  - Format tags on cells reflect the blink state correctly.
- **Tests required:**
  - Feed `ESC[5mHello`, verify FormatTag on "Hello" cells has `BlinkState::Slow`.
  - Feed `ESC[6mWorld`, verify `BlinkState::Fast`.
  - Feed `ESC[25m` after blink, verify `BlinkState::None`.
  - Feed `ESC[0m`, verify blink cleared.
  - Feed `ESC[1;5mBold+Blink`, verify both bold AND blink are set.
- **Completion notes:**
  - Moved `SlowBlink`, `FastBlink`, `NotBlinking` out of the ignored group in `apply_sgr()`.
  - Added 3 match arms mapping to `BlinkState::Slow`, `BlinkState::Fast`, `BlinkState::None`.
  - SGR 0 (Reset) already clears blink via `*tag = FormatTag::default()`.
  - Added `BlinkState` to the top-level import in `terminal_handler.rs`.
  - Added 9 tests: 5 `apply_sgr` unit tests + 4 `handle_sgr`/`process_outputs` integration tests.
  - All verification passes.

---

### 23.3 — Add `has_blinking_text` to `TerminalSnapshot`

- **Status:** Done (2026-03-31)
- **Priority:** 1 — High
- **Scope:** `freminal-terminal-emulator/src/snapshot.rs`,
  `freminal-terminal-emulator/src/interface.rs`
- **Details:**
  1. Add `pub has_blinking_text: bool` to `TerminalSnapshot`.
  2. In `build_snapshot()`, scan `visible_tags` for any tag with `blink != BlinkState::None`.
     Set `has_blinking_text` accordingly.
  3. Update `TerminalSnapshot::empty()` to set `has_blinking_text: false`.

- **Acceptance criteria:**
  - Snapshot reports `has_blinking_text: true` when visible cells have blink attributes.
  - Snapshot reports `has_blinking_text: false` when no visible cells blink.
- **Tests required:**
  - Feed blinking text, build snapshot, verify `has_blinking_text` is true.
  - Feed non-blinking text only, verify `has_blinking_text` is false.
- **Completion notes:**
  - Added `pub has_blinking_text: bool` to `TerminalSnapshot`.
  - `build_snapshot()` scans `visible_tags` for any tag with `blink != BlinkState::None`.
  - `TerminalSnapshot::empty()` sets `has_blinking_text: false`.
  - Added 1 unit test in `snapshot.rs` and 4 integration tests in `snapshot_build.rs`
    (plain text = false, SGR 5 = true, SGR 6 = true, overwritten blink = false).
  - All verification passes.

---

### 23.4 — Add Blink Timer to GUI

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** `freminal/src/gui/mod.rs`, `freminal/src/gui/view_state.rs`
- **Details:**
  1. Add blink state fields to `ViewState` (or `FreminalGui`):
     - `text_blink_cycle: u8` — cycles 0-5 at ~167ms intervals.
     - `text_blink_last_tick: Instant` — when the last cycle increment happened.
     - `text_blink_slow_visible: bool` — derived from cycle.
     - `text_blink_fast_visible: bool` — derived from cycle.

  2. In `update()`, when the loaded snapshot has `has_blinking_text: true`:
     - Check if 167ms has elapsed since `text_blink_last_tick`.
     - If yes, increment `text_blink_cycle` (wrapping at 6), update visibility flags.
     - Schedule `request_repaint_after(Duration::from_millis(167))`.

  3. When `has_blinking_text` is false, do not schedule blink repaints (save power).

- **Acceptance criteria:**
  - Blink timer runs only when blinking text is present.
  - Slow blink toggles at ~1 Hz.
  - Fast blink toggles at ~3 Hz.
  - No unnecessary repaints when no text is blinking.
- **Tests required:**
  - Unit test for cycle → visibility mapping:
    cycles 0,1,2 → slow visible; 3,4,5 → slow hidden.
    cycles 0,2,4 → fast visible; 1,3,5 → fast hidden.

---

### 23.5 — Wire Blink State Through Rendering Pipeline

- **Status:** Pending
- **Priority:** 1 — High
- **Scope:** `freminal/src/gui/terminal.rs`, `freminal/src/gui/renderer.rs`
- **Details:**
  1. In `process_tags()` or the shaped run building step, propagate `BlinkState` from
     `FormatTag` to the rendering data.
  2. Pass `text_blink_slow_visible` and `text_blink_fast_visible` to the renderer.
  3. In the vertex/glyph rendering code:
     - For cells with `BlinkState::Slow`: skip glyph rendering when `slow_visible` is false.
     - For cells with `BlinkState::Fast`: skip glyph rendering when `fast_visible` is false.
     - Always render the cell background (blinking text blinks the foreground only, not
       the background — this matches xterm/VTE behavior).
  4. For cells with `BlinkState::None`: render normally, no blink logic.

- **Acceptance criteria:**
  - Blinking text alternates between visible and hidden at the correct rate.
  - Cell backgrounds remain visible when text is in "hidden" phase.
  - Non-blinking text is unaffected.
  - Both slow and fast blink rates are visually distinct.
- **Tests required:**
  - Visual smoke test (manual): `echo -e "\e[5mSlow Blink\e[0m \e[6mFast Blink\e[0m"`
  - Verify non-blinking text does not flicker.

---

### 23.6 — DECSCUSR Interaction with Text Blink

- **Status:** Pending
- **Priority:** 3 — Low
- **Scope:** `freminal/src/gui/mod.rs`
- **Details:**
  Cursor blink and text blink are independent timers. Ensure they do not interfere:
  - Cursor blink runs at its own rate (typically ~530ms).
  - Text blink runs at 167ms when active.
  - Both can schedule `request_repaint_after()` — the earlier one wins.
  - Keyboard input resets cursor blink but does NOT reset text blink.

  This subtask is primarily a verification pass — confirm that the two blink systems coexist
  correctly and document any edge cases.

- **Acceptance criteria:**
  - Cursor blink and text blink operate independently.
  - Keyboard input makes cursor steady but text continues blinking.
  - Both blink rates are correct when both are active simultaneously.
- **Tests required:**
  - Manual verification with cursor in a blinking text region.

---

### 23.7 — Update SUPPORTED_CONTROL_CODES.md

- **Status:** Pending
- **Priority:** 3 — Low
- **Scope:** `Documents/SUPPORTED_CONTROL_CODES.md`, `Documents/SGR.md`
- **Details:**
  Update the SGR section to reflect that SGR 5 (slow blink), SGR 6 (fast blink), and SGR 25
  (not blinking) are now fully implemented. Update any notes that say blink is "intentionally
  ignored" or "not implemented".

- **Acceptance criteria:**
  - SUPPORTED_CONTROL_CODES.md reflects implemented blink support.
  - SGR.md (if it exists) reflects blink support.
- **Tests required:** None (documentation only).

---

## Implementation Notes

### Subtask Ordering

23.1 and 23.2 establish the data model and must be done first.
23.3 adds snapshot transport and depends on 23.1.
23.4 adds the timer and can be done in parallel with 23.3.
23.5 is the rendering work and depends on 23.1, 23.3, and 23.4.
23.6 and 23.7 are low-priority follow-ups.

**Recommended order:** 23.1 → 23.2 → 23.3 → 23.4 → 23.5 → 23.6 → 23.7

### Performance Considerations

- The blink timer adds at most 6 repaints per second (167ms interval) when blinking text is
  present. This is negligible compared to the cursor blink timer that already runs.
- When no text is blinking (the overwhelmingly common case), there is zero overhead.
- The `has_blinking_text` scan in `build_snapshot()` is O(number of visible tags), which is
  typically 10-50. Negligible.

### Blink Rate Standards

ECMA-48 does not define specific blink rates. De facto standards from other terminals:

| Terminal             | Slow Blink | Fast Blink                        |
| -------------------- | ---------- | --------------------------------- |
| xterm                | ~1 Hz      | Not implemented (treated as slow) |
| VTE (GNOME Terminal) | ~1 Hz      | ~3 Hz                             |
| Kitty                | ~1 Hz      | ~3 Hz                             |
| WezTerm              | ~1 Hz      | Not implemented                   |

Freminal will implement both rates matching VTE/Kitty behavior.

### Verification

Each subtask must pass before proceeding:

- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo-machete`

---

## References

- ECMA-48 Section 8.3.117 (SGR) — SGR 5, 6, 25 definitions
- `freminal-common/src/sgr.rs` — SGR enum definitions
- `freminal-common/src/buffer_states/fonts.rs` — `FontDecorations` enum
- `freminal-common/src/buffer_states/format_tag.rs` — `FormatTag` struct
- `freminal-buffer/src/terminal_handler.rs:3985-3990` — current discard location
- `freminal/src/gui/terminal.rs` — cursor blink rendering (blueprint)
- `freminal/src/gui/renderer.rs` — vertex building
- `freminal/src/gui/mod.rs` — repaint scheduling
