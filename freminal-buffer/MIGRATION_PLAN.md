# freminal-buffer Migration Plan

## Agent Instructions

This document is a sequential, step-by-step migration plan for replacing the old
flat-buffer implementation in `freminal-terminal-emulator` with the new row/cell-based
`freminal-buffer` crate.

**Each agent working from this plan must:**

1. Tackle **exactly one numbered step** — no more.
2. Write tests that verify the new behaviour before considering the step done.
3. Ensure `cargo test --workspace` passes with zero failures before stopping.
4. Update the checkbox next to the completed step (`- [ ]` → `- [x]`).
5. Stop and hand off. Do not begin the next step.

If a step is already checked off, skip it and pick the next unchecked one.

---

## Background & Architecture

### Old buffer (`freminal-terminal-emulator/src/state/buffer.rs`)

A flat `Vec<TChar>` with a parallel `FormatTracker` (a separate `Vec<FormatTag>` with
byte-range indices). The `TerminalState` struct in `internal.rs` owns both the buffer
and all terminal modes, mixing buffer concerns with input/protocol concerns in one large
struct.

### New buffer (`freminal-buffer`)

Three clean layers:

| Layer             | File                  | Responsibility                                                                 |
| ----------------- | --------------------- | ------------------------------------------------------------------------------ |
| `Buffer`          | `buffer.rs`           | Row/cell storage, cursor, scrollback, scroll regions, primary/alternate switch |
| `TerminalHandler` | `terminal_handler.rs` | Dispatcher: receives `TerminalOutput` events, routes to `Buffer` methods       |
| `Row` / `Cell`    | `row.rs`, `cell.rs`   | Per-line and per-character storage; each `Cell` carries its own `FormatTag`    |

The new design is strictly better: format data lives with the character it describes
instead of in a fragile parallel index structure.

---

## Separation of Concerns — What the Buffer Owns vs. What It Doesn't

This is a deliberate design constraint that every step in this plan must respect.

### Buffer / TerminalHandler is the source of truth for

- **Cursor position** (`CursorPos`) — where the next character will be written.
- **Current format tag** (`FormatTag`) — the SGR state applied to the next character written.
  This includes fg/bg colors, bold, italic, underline, strikethrough, URL, etc.
- **Saved cursor** (DECSC/DECRC) — saved cursor position and format state.
- **Row/cell content** — the actual characters on screen, their format, wide-char metadata.
- **Scrollback rows** — history in the primary buffer.
- **Scroll offset** — how far the user has scrolled back from the live bottom.
- **Scroll region** (DECSTBM top/bottom margins) — which rows the buffer scrolls when LF
  is received at the bottom margin.
- **LNM mode** (`lnm_enabled`) — whether LF implies CR. This directly changes how
  `handle_lf` moves the cursor, so the buffer owns it.
- **DECAWM (auto-wrap)** — whether text wraps at the terminal width. `insert_text` is
  the only place this matters, so the buffer owns it as `wrap_enabled: bool`.
- **Primary/alternate buffer identity** (`BufferType`) — which logical screen is active.
- **Terminal dimensions** (width × height).

### TerminalHandler owns (not Buffer)

- **Show cursor flag** (`Dectcem`) — whether to paint the cursor. This is a rendering
  hint to the GUI, not a buffer operation. The handler holds it and exposes it.
- **Cursor visual style** (`CursorVisualStyle`) — block/underline/bar, blink/steady.
  Pure rendering metadata, owned by the handler.
- **Character replacement mode** (`DecSpecialGraphics`) — whether incoming data bytes
  should be remapped to box-drawing Unicode before being passed to the buffer. Owned
  by the handler; applied in `handle_data` before calling `buffer.insert_text`.

### Neither Buffer nor TerminalHandler owns — these live in the terminal emulator layer

- **Cursor key mode** (`Decckm`) — affects how arrow keys are _encoded for output_, not
  how the buffer renders. Lives in the emulator/input layer.
- **Mouse tracking mode** — affects how mouse events are _encoded for output_.
- **Bracketed paste mode** — affects how paste content is _encoded for output_.
- **Focus reporting** (`XtMseWin`) — affects what is _written to the PTY_ when focus changes.
- **Synchronized updates** — a rendering pipeline hint for the GUI, not a buffer concern.
- **Reverse wrap-around** (`ReverseWrapAround`) — affects backspace-past-column-0 behaviour;
  deferred to a later step.
- **Screen inversion** (`Decscnm`) — a rendering colormap transform, not a buffer concern.
- **Theme** — a rendering concern.
- **Window commands** (`Vec<WindowManipulation>`) — queued by the handler when OSC/CSI
  requests it; drained by the emulator and forwarded to the GUI.
- **PTY write channel** (`write_tx`) — used to write cursor-position reports, DA responses,
  etc. back to the running process. Held by the emulator, injected into the handler when
  needed (Step 3.2).

> **Rule of thumb:** if removing a mode would require changes to `buffer.rs` or `row.rs`,
> the buffer owns it. If it only affects what bytes flow out of the terminal or how the
> GUI paints, it does not belong in the buffer.

---

## Phase 1 — Semantic Bug Fixes and Missing Dispatcher Wires

These steps fix incorrect behaviour and wire up already-implemented buffer methods that
are not yet reachable through the dispatcher. No GUI changes. No emulator integration.

---

### Step 1.1 — Fix the `Delete(n)` DCH vs. DL semantic mismatch

- [x] **Done**

#### 1.1 Why this step exists

`TerminalOutput::Delete(n)` means DCH — Delete Character — which removes `n` characters
at the cursor column, shifting remaining characters on the same row left (the row does
not change length beyond its logical width). The dispatcher currently calls
`handle_delete_lines(n)` which removes entire rows. This is wrong.

#### 1.1 Files to change

- `freminal-buffer/src/buffer.rs`
- `freminal-buffer/src/terminal_handler.rs`

#### 1.1 What to implement

1. Add `pub fn delete_chars(&mut self, n: usize)` to `Buffer`.
   - Operates on the row at `cursor.pos.y`.
   - Removes `n` cells starting at `cursor.pos.x`, shifting the cells to the right
     of the deleted range left. Cells shifted off the right edge are discarded.
   - If `n` exceeds the number of cells to the right of the cursor, clears to the end
     of the stored cells.
   - Cursor does not move.
   - Handles wide-glyph cleanup: if the cell at `cursor.pos.x` is a continuation cell,
     first move left to the head; if it is a head, clear its continuations before deletion.
   - Call `self.debug_assert_invariants()` at the end.

2. Add `pub fn handle_delete_chars(&mut self, n: usize)` to `TerminalHandler` that
   delegates to `self.buffer.delete_chars(n)`.

3. In `TerminalHandler::process_output`, change the `TerminalOutput::Delete(n)` arm from
   `self.handle_delete_lines(*n)` to `self.handle_delete_chars(*n)`. The `Delete(n)`
   variant in `TerminalOutput` is DCH per the old `internal.rs` dispatch at L1499.

4. **Do not remove `handle_delete_lines`** — it is called by other arms and is correct for
   DL (Delete Lines). The naming confusion is in the dispatcher wiring only.

#### 1.1 Tests to add

(inside `buffer.rs` or a new `tests/dch_tests.rs`)

- `dch_simple`: insert `"ABCDE"`, cursor at col 1, `delete_chars(2)` → row reads `"ADE"`.
- `dch_clamps`: insert `"ABC"`, cursor at col 1, `delete_chars(100)` → row reads `"A"`.
- `dch_at_col_zero`: cursor at 0, `delete_chars(1)` → first character removed, rest shift left.
- `dch_noop_past_end`: cursor at col beyond stored cells → no panic, no change.
- `dch_wide_head`: insert a wide character at col 0, `delete_chars(1)` at col 0 →
  both head and continuation removed.

---

### Step 1.2 — Add `Erase(n)` / ECH (Erase Character) implementation

- [ ] **Done**

#### 1.2 Why this step exists

`TerminalOutput::Erase(n)` is ECH — Erase Character. It replaces `n` characters starting
at the cursor column with blanks (using the current format tag for the blank cells). The
cursor does not move. Remaining characters to the right are NOT shifted. This is
semantically different from `delete_chars` (Step 1.1) which does shift them.

#### 1.2 Files to change

- `freminal-buffer/src/buffer.rs`
- `freminal-buffer/src/terminal_handler.rs`

#### 1.2 What to implement

1. Add `pub fn erase_chars(&mut self, n: usize)` to `Buffer`.
   - Operates on the row at `cursor.pos.y`.
   - Replaces cells at `[cursor.pos.x .. cursor.pos.x + n]` with
     `Cell::blank_with_tag(self.current_tag.clone())`.
   - If the range extends beyond the stored cells, pad with blanks up to
     `cursor.pos.x + n` but never beyond `self.width`.
   - Wide-glyph cleanup: apply `cleanup_wide_overwrite`-style logic for any wide head
     or continuation that falls in the erased range.
   - Cursor does not move.
   - Call `self.debug_assert_invariants()` at the end.

2. Add `pub fn handle_erase_chars(&mut self, n: usize)` to `TerminalHandler`.

3. In `process_output`, change `TerminalOutput::Erase(n)` from `todo!(...)` to
   `self.handle_erase_chars(*n)`.

#### 1.2 Tests to add

- `ech_simple`: insert `"ABCDE"`, cursor at col 1, `erase_chars(2)` → content is
  `"A  DE"` (spaces, not shifted).
- `ech_clamps_at_width`: cursor at col `width - 2`, `erase_chars(10)` → only 2 cells
  erased, no out-of-bounds.
- `ech_at_col_zero`: `erase_chars(3)` at col 0 → first 3 cells become blanks.
- `ech_vs_dch_differ`: verify that after `erase_chars` the character to the right of the
  erased region is still in place (distinguishing it from DCH).

---

### Step 1.3 — Wire IND, RI, NEL into the dispatcher

- [ ] **Done**

#### 1.3 Why this step exists

`Buffer` has `handle_ind()`, `handle_ri()`, and `handle_nel()` fully implemented.
`TerminalHandler` has `handle_index()`, `handle_reverse_index()`, and `handle_next_line()`
wrapping them. But `process_output` has no arms for these because `TerminalOutput` in
`freminal-common` does not currently have matching variants.

#### 1.3 Files to change

- `freminal-common/src/buffer_states/terminal_output.rs`
- `freminal-terminal-emulator/src/ansi_components/standard.rs` (parser emission)
- `freminal-buffer/src/terminal_handler.rs`

#### 1.3 What to implement

1. Add three variants to the `TerminalOutput` enum in `freminal-common`:

   ```text
   Index,          // ESC D — IND
   ReverseIndex,   // ESC M — RI
   NextLine,       // ESC E — NEL
   ```

   Add `Display` arms for each (e.g. `Self::Index => write!(f, "Index")`).

2. In the ANSI parser (`standard.rs` or wherever ESC sequences are dispatched), find
   where ESC D, ESC E, and ESC M are handled and change their emissions to the new variants.
   Grep for `b'D'`, `b'E'`, `b'M'` inside the escape-sequence handler to locate them.

3. In `TerminalHandler::process_output`, add arms:

   ```rust
   TerminalOutput::Index => self.handle_index(),
   TerminalOutput::ReverseIndex => self.handle_reverse_index(),
   TerminalOutput::NextLine => self.handle_next_line(),
   ```

4. In the old `TerminalState::handle_incoming_data`, add matching arms so the old buffer
   path also handles the new variants (prevents breakage during parallel-run phase).

#### 1.3 Tests to add

(in `tests/terminal_handler_integration.rs` or a new file)

- `ind_scrolls_at_bottom_margin`: set scroll region [0..4] on a 5-row buffer, cursor at
  row 4, send `Index` → region scrolls up, cursor stays at row 4.
- `ri_scrolls_at_top_margin`: cursor at top of scroll region, send `ReverseIndex` →
  region scrolls down (blank at top), cursor stays.
- `nel_is_cr_plus_lf`: cursor mid-row, send `NextLine` → cursor moves to col 0 of next row.

---

### Step 1.4 — Wire alternate-buffer enter/leave through the Mode dispatcher

- [ ] **Done**

#### 1.4 Why this step exists

`TerminalHandler` has `handle_enter_alternate()` and `handle_leave_alternate()`, and
`Buffer` has `enter_alternate()` / `leave_alternate()` fully implemented and tested.
However, `process_output` hits `todo!("Mode switching not yet implemented")` for all
`Mode(_)` variants, which means opening vim or less will panic.

This step implements the minimum viable mode dispatch needed to make alternate-screen
applications not crash.

#### 1.4 Files to change

- `freminal-buffer/src/terminal_handler.rs`

#### 1.4 What to implement

Replace the single `TerminalOutput::Mode(_mode) => todo!(...)` arm with a `match` on
the mode. For this step, handle only:

```rust
TerminalOutput::Mode(mode) => match mode {
    Mode::XtExtscrn(XtExtscrn::Alternate) => self.handle_enter_alternate(),
    Mode::XtExtscrn(XtExtscrn::Primary)   => self.handle_leave_alternate(),
    Mode::XtExtscrn(XtExtscrn::Query)     => { /* TODO: report mode */ }
    _other => {
        // All other modes: log at debug level and ignore.
        // Do NOT use todo!() here — unknown modes must never panic.
        debug!("Unhandled mode: {:?}", _other);
    }
},
```

The key constraint is that the fallthrough must be a silent ignore, not a `todo!()`.
Subsequent steps will fill in individual mode arms one at a time.

#### 1.4 Tests to add

(in `tests/terminal_handler_integration.rs`)

- `alternate_enter_clears_screen`: write text, enter alternate, verify visible rows are
  empty (no content from primary bleeds through).
- `alternate_leave_restores_content`: write `"hello"` in primary, enter alternate, write
  `"world"`, leave alternate → visible rows show `"hello"` again, not `"world"`.
- `unknown_mode_does_not_panic`: send a `Mode::NoOp` and a `Mode::Decarm(...)` through
  `process_outputs` — must not panic.

---

### Step 1.5 — Implement Save/Restore Cursor (DECSC / DECRC)

- [ ] **Done**

#### 1.5 Why this step exists

`TerminalOutput::SaveCursor` and `TerminalOutput::RestoreCursor` are `todo!()` in the
dispatcher. Many terminal applications (tmux, screen, various prompts) use these.

#### 1.5 Files to change

- `freminal-buffer/src/buffer.rs`
- `freminal-buffer/src/terminal_handler.rs`

#### 1.5 What to implement

1. Add `saved_cursor: Option<CursorState>` field to `Buffer`.
   Initialized to `None` in `Buffer::new`.
   The field must be included in `SavedPrimaryState` so it is preserved across
   alternate-screen round-trips (save in primary, enter alt, leave alt, restore
   in primary must still work).

2. Add `pub fn save_cursor(&mut self)` to `Buffer`:
   `self.saved_cursor = Some(self.cursor.clone());`

3. Add `pub fn restore_cursor(&mut self)` to `Buffer`:
   If `self.saved_cursor` is `Some`, copy it back to `self.cursor`. Clamp
   `cursor.pos.x` to `[0, self.width - 1]` and `cursor.pos.y` to a valid
   row index after restoring, in case the terminal was resized between save and restore.

4. Add `pub fn handle_save_cursor(&mut self)` and `pub fn handle_restore_cursor(&mut self)`
   to `TerminalHandler`, delegating to `self.buffer`.

5. In `process_output`, replace the `todo!()` arms:

   ```rust
   TerminalOutput::SaveCursor    => self.handle_save_cursor(),
   TerminalOutput::RestoreCursor => self.handle_restore_cursor(),
   ```

#### 1.5 Tests to add

- `save_restore_position`: move cursor to (5, 3), save, move to (0, 0), restore →
  cursor is back at (5, 3).
- `save_restore_preserves_format`: set a non-default `FormatTag`, save cursor, reset
  format to default, restore → format tag is back to the saved state.
  _(Note: `CursorState` in `freminal-common` does not currently hold `FormatTag`
  directly — it holds `StateColors`, `font_weight`, `font_decorations`. The saved
  cursor captures those fields. The buffer's `current_tag` is a separate field; for
  this step it is acceptable to save/restore only the cursor position and
  `CursorState` fields. A follow-up step can unify format tracking.)_
- `restore_without_save_is_noop`: call restore without a prior save → cursor stays
  at current position, no panic.
- `save_survives_alternate_roundtrip`: save in primary, enter alternate, leave alternate,
  restore → position is what was saved in primary.

---

## Phase 2 — SGR: Colors and Text Attributes

### Step 2.1 — Implement `SelectGraphicRendition` to `FormatTag` accumulation

- [ ] **Done**

#### 2.1 Why this step exists

`TerminalOutput::Sgr(SelectGraphicRendition)` is `todo!()`. Without SGR, the terminal
renders everything in default colors with no bold/italic/underline. This affects nearly
every real-world terminal session.

The new buffer stores format per-cell via `Buffer::current_tag: FormatTag`. Each SGR
event is an incremental mutation of that tag. The tag is cloned into every cell written
after the mutation.

#### 2.1 Files to change

- `freminal-buffer/src/terminal_handler.rs`
- `freminal-common/src/buffer_states/format_tag.rs` (add a helper method, see below)

#### 2.1 What to implement

1. Add a free function (or inherent method) in `terminal_handler.rs`:

   ```rust
   fn apply_sgr(tag: &mut FormatTag, sgr: &SelectGraphicRendition) { ... }
   ```

   Port the logic from `TerminalState::sgr()` in
   `freminal-terminal-emulator/src/state/internal.rs` (lines 762–843).
   The mapping is:

   | SGR variant                     | FormatTag mutation                                                |
   | ------------------------------- | ----------------------------------------------------------------- |
   | `Reset`                         | `*tag = FormatTag::default()`                                     |
   | `Bold`                          | `tag.font_weight = FontWeight::Bold`                              |
   | `ResetBold` / `NormalIntensity` | `tag.font_weight = FontWeight::Normal`                            |
   | `Italic`                        | add `FontDecorations::Italic` to `tag.font_decorations` if absent |
   | `NotItalic`                     | remove `FontDecorations::Italic`                                  |
   | `Faint`                         | add `FontDecorations::Faint`                                      |
   | `Underline`                     | add `FontDecorations::Underline`                                  |
   | `NotUnderlined`                 | remove `FontDecorations::Underline`                               |
   | `Strikethrough`                 | add `FontDecorations::Strikethrough`                              |
   | `NotStrikethrough`              | remove `FontDecorations::Strikethrough`                           |
   | `ReverseVideo`                  | `tag.colors.set_reverse_video(ReverseVideo::On)`                  |
   | `ResetReverseVideo`             | `tag.colors.set_reverse_video(ReverseVideo::Off)`                 |
   | `Foreground(c)`                 | `tag.colors.set_color(c)`                                         |
   | `Background(c)`                 | `tag.colors.set_background_color(c)`                              |
   | `UnderlineColor(c)`             | `tag.colors.set_underline_color(c)`                               |
   | All others                      | log at `debug!` level, ignore                                     |

2. Update `TerminalHandler::handle_sgr` (which currently is a stub):

   ```rust
   pub fn handle_sgr(&mut self, sgr: &SelectGraphicRendition) {
       apply_sgr(&mut self.current_format, sgr);
       self.buffer.set_format(self.current_format.clone());
   }
   ```

3. In `process_output`, replace `TerminalOutput::Sgr(_sgr) => todo!(...)` with:

   ```rust
   TerminalOutput::Sgr(sgr) => self.handle_sgr(sgr),
   ```

#### 2.1 Tests to add

(in `terminal_handler.rs` or a new `tests/sgr_tests.rs`)

- `sgr_bold_sets_font_weight`: send `Sgr(Bold)`, write a character, check the cell's
  tag has `FontWeight::Bold`.
- `sgr_reset_clears_all`: send `Sgr(Bold)`, `Sgr(Foreground(Red))`, then `Sgr(Reset)` →
  next character's tag equals `FormatTag::default()`.
- `sgr_fg_color`: send `Sgr(Foreground(TerminalColor::Red))`, write `"A"` → cell tag
  has `colors.color == TerminalColor::Red`.
- `sgr_bg_color`: similar for background.
- `sgr_custom_rgb`: send `Sgr(Foreground(TerminalColor::Custom(255, 128, 0)))` → correct
  color stored.
- `sgr_italic_toggle`: add italic, verify present; send `NotItalic`, verify absent.
- `sgr_multiple_accumulate`: Bold + Underline + Red foreground all active simultaneously
  after three separate SGR events.
- `sgr_reverse_video`: set reverse video, verify `ReverseVideo::On`; reset, verify Off.

---

## Phase 3 — Mode Switching, State Exposure, and Outbound Communication

### Step 3.1 — Add DECAWM (auto-wrap) to the buffer

- [ ] **Done**

#### 3.1 Why this step exists

`Buffer::insert_text` always wraps at the terminal width. The old buffer respects
`Decawm::NoAutoWrap`, which is used by programs that do their own cursor positioning.
Without this, text in `NoAutoWrap` mode gets mangled.

#### 3.1 Files to change

- `freminal-buffer/src/buffer.rs`
- `freminal-buffer/src/terminal_handler.rs`

#### 3.1 What to implement

1. Add `wrap_enabled: bool` field to `Buffer`, initialized to `true` in `Buffer::new`.

2. Add `pub fn set_wrap(&mut self, enabled: bool)` to `Buffer`.

3. In `Buffer::insert_text`, at the PRE-WRAP check (`if col >= self.width`), gate the
   wrap:

   ```rust
   if col >= self.width {
       if !self.wrap_enabled {
           // Clamp cursor to last column; discard remaining text.
           self.cursor.pos.x = self.width.saturating_sub(1);
           self.cursor.pos.y = row_idx;
           self.enforce_scrollback_limit();
           return;
       }
       // ... existing wrap logic ...
   }
   ```

   Apply the same guard at the POST-WRAP / `InsertResponse::Leftover` branch.

4. In `TerminalHandler`, expose:

   ```rust
   pub fn handle_set_wrap(&mut self, enabled: bool) {
       self.buffer.set_wrap(enabled);
   }
   ```

5. In the `Mode(_)` match arm (from Step 1.4), add:

   ```rust
   Mode::Decawm(Decawm::AutoWrap)   => self.handle_set_wrap(true),
   Mode::Decawm(Decawm::NoAutoWrap) => self.handle_set_wrap(false),
   Mode::Decawm(Decawm::Query)      => { /* TODO: report mode — later step */ }
   ```

#### 3.1 Tests to add

- `wrap_enabled_default`: write 85 characters to an 80-column buffer → row 1 exists and
  contains the overflow.
- `wrap_disabled_clamps`: disable wrap, write 85 characters → only 80 characters on row 0,
  no row 1, cursor at col 79.
- `wrap_re_enable`: disable, write overflow, re-enable, write more → new text wraps normally.

---

### Step 3.2 — Implement LNM (Line Feed Mode) setter

- [ ] **Done**

#### 3.2 Why this step exists

`Buffer` has `lnm_enabled: bool` but there is no public setter and the `Mode` dispatcher
does not set it. The `LineFeedMode(Lnm::NewLine)` mode makes LF behave like CRLF.

#### 3.2 Files to change

- `freminal-buffer/src/buffer.rs`
- `freminal-buffer/src/terminal_handler.rs`

#### 3.2 What to implement

1. `lnm_enabled` is currently a private field with no setter. Add:

   ```rust
   pub fn set_lnm(&mut self, enabled: bool) {
       self.lnm_enabled = enabled;
   }
   ```

2. In `TerminalHandler`, add:

   ```rust
   pub fn handle_set_lnm(&mut self, enabled: bool) {
       self.buffer.set_lnm(enabled);
   }
   ```

3. In the `Mode(_)` match, add:

   ```rust
   Mode::LineFeedMode(Lnm::NewLine)  => self.handle_set_lnm(true),
   Mode::LineFeedMode(Lnm::LineFeed) => self.handle_set_lnm(false),
   Mode::LineFeedMode(Lnm::Query)    => { /* TODO: report mode */ }
   ```

#### 3.2 Tests to add

- `lnm_off_lf_does_not_reset_x`: LNM disabled (default), write `"hello"`, send LF →
  cursor Y increments, cursor X stays at 5.
- `lnm_on_lf_resets_x`: enable LNM, write `"hello"`, send LF → cursor X = 0, cursor Y
  increments.
- `lnm_toggle`: enable, verify, disable, verify behaviour reverts.

---

### Step 3.3 — Expose show-cursor and cursor-visual-style from TerminalHandler

- [ ] **Done**

#### 3.3 Why this step exists

The GUI needs to know (a) whether to paint a cursor at all (`Dectcem`), and (b) what
shape/blink it should be (`CursorVisualStyle`). These are not buffer concerns but the
`TerminalHandler` is where the `Dectcem` mode event arrives, so it must store and
expose them.

#### 3.3 Files to change

- `freminal-buffer/src/terminal_handler.rs`

#### 3.3 What to implement

1. Add fields to `TerminalHandler`:

   ```rust
   show_cursor: Dectcem,                    // default: Show
   cursor_visual_style: CursorVisualStyle,  // default: BlockCursorBlink or platform default
   ```

2. Add accessors:

   ```rust
   pub const fn show_cursor(&self) -> bool {
       matches!(self.show_cursor, Dectcem::Show)
   }
   pub fn cursor_visual_style(&self) -> CursorVisualStyle {
       self.cursor_visual_style.clone()
   }
   ```

3. In the `Mode(_)` match, add:

   ```rust
   Mode::Dectem(Dectcem::Show)  => self.show_cursor = Dectcem::Show,
   Mode::Dectem(Dectcem::Hide)  => self.show_cursor = Dectcem::Hide,
   Mode::Dectem(Dectcem::Query) => { /* TODO: report mode */ }
   ```

4. For `CursorVisualStyle`, it arrives via `TerminalOutput::CursorVisualStyle(style)`.
   Replace the `todo!()` arm with:

   ```rust
   TerminalOutput::CursorVisualStyle(style) => {
       self.cursor_visual_style = style.clone();
   }
   ```

5. Handle `XtCBlink` in the mode match similarly to the old code — translate it into the
   appropriate `CursorVisualStyle` blink/steady variant:

   ```rust
   Mode::XtCBlink(XtCBlink::Blinking)    => { /* set blink variant of current style */ }
   Mode::XtCBlink(XtCBlink::NotBlinking) => { /* set steady variant */ }
   Mode::XtCBlink(XtCBlink::Query)       => { /* TODO: report mode */ }
   ```

#### 3.3 Tests to add

- `show_cursor_default_true`: new handler → `show_cursor()` is `true`.
- `hide_cursor_mode`: send `Mode(Dectem(Hide))` via `process_outputs` → `show_cursor()`
  is `false`.
- `show_cursor_mode`: send hide then show → `show_cursor()` is `true`.
- `cursor_visual_style_set`: send `CursorVisualStyle(VerticalLineCursorSteady)` →
  `cursor_visual_style()` returns that value.

---

### Step 3.4 — Add DecSpecialGraphics character remapping

- [ ] **Done**

#### 3.4 Why this step exists

`TerminalOutput::DecSpecialGraphics(DecSpecialGraphics)` is `todo!()`. The DEC Special
Graphics character set maps bytes 0x5F–0x7E to box-drawing Unicode characters. Programs
that draw borders (htop, dialog, etc.) depend on this.

#### 3.4 Files to change

- `freminal-buffer/src/terminal_handler.rs`

#### 3.4 What to implement

1. Add `character_replace: DecSpecialGraphics` to `TerminalHandler`, initialized to
   `DecSpecialGraphics::DontReplace`.

2. In `handle_data`, before calling `TChar::from_vec(data)`, check
   `self.character_replace` and apply the remapping table. Port the 35-entry `match c`
   table from `TerminalState::handle_data` in `internal.rs` (lines 361–416):

   ```rust
   let data = match self.character_replace {
       DecSpecialGraphics::Replace => {
           let mut out = Vec::with_capacity(data.len() * 3);
           for &b in data {
               match b {
                   0x5f => out.extend_from_slice("\u{00A0}".as_bytes()),
                   0x60 => out.extend_from_slice("\u{25C6}".as_bytes()),
                   // ... (all 35 entries) ...
                   _ => out.push(b),
               }
           }
           out
       }
       DecSpecialGraphics::DontReplace => data.to_vec(),
   };
   // then TChar::from_vec(&data) ...
   ```

3. In `process_output`, replace the `DecSpecialGraphics(_)` `todo!()` arm:

   ```rust
   TerminalOutput::DecSpecialGraphics(dsg) => {
       self.character_replace = dsg.clone();
   }
   ```

#### 3.4 Tests to add

- `dec_special_replace_lower_right_corner`: enable replace mode, send byte `0x6a` →
  the stored cell contains `TChar::Utf8` for `┘` (U+2518).
- `dec_special_dont_replace_passthrough`: in `DontReplace` mode, byte `0x6a` is stored
  as `TChar::Ascii(0x6a)`.
- `dec_special_toggle`: enable, write a box char, disable, write `0x6a` as ASCII →
  two cells, first Unicode, second ASCII.
- `dec_special_all_passthrough_above_7e`: byte `0x80` is never remapped regardless of mode.

---

### Step 3.5 — Add outbound write channel and implement cursor-position report

- [ ] **Done**

#### 3.5 Why this step exists

Some terminal sequences require writing a response back to the running process via the
PTY. The most critical is CPR — Cursor Position Report — sent when the application
sends `ESC [ 6 n`. Without this, many terminal detection routines hang waiting for a
response. Device Attributes (DA1) is similarly important for capability detection.

#### 3.5 Files to change

- `freminal-buffer/src/terminal_handler.rs`
- `freminal-buffer/Cargo.toml` (add `crossbeam-channel` dependency)

#### 3.5 What to implement

1. Add `crossbeam-channel` to `freminal-buffer/Cargo.toml` as a workspace dependency.

2. Add to `TerminalHandler`:

   ```rust
   write_tx: Option<crossbeam_channel::Sender<PtyWrite>>,
   window_commands: Vec<WindowManipulation>,
   ```

   `write_tx` defaults to `None`; `TerminalHandler::new` creates without a channel.

3. Add:

   ```rust
   pub fn set_write_tx(&mut self, tx: crossbeam_channel::Sender<PtyWrite>) {
       self.write_tx = Some(tx);
   }

   pub fn take_window_commands(&mut self) -> Vec<WindowManipulation> {
       std::mem::take(&mut self.window_commands)
   }
   ```

4. Add a private helper `fn write_to_pty(&self, text: &str)` that sends bytes through
   `write_tx` if present, silently dropping if `None`.

5. Implement `handle_cursor_report(&mut self)`:

   ```rust
   pub fn handle_cursor_report(&mut self) {
       let pos = self.buffer.get_cursor().pos;
       let x = pos.x + 1;
       let y = pos.y + 1; // TODO: convert buffer-row to screen-row in a later step
       self.write_to_pty(&format!("\x1b[{y};{x}R"));
   }
   ```

   Note: the y value here may need refinement once the GUI output API is in place
   (Step 4.x). For now, using raw buffer row is acceptable as a placeholder.

6. Implement `handle_request_device_attributes(&mut self)` — send the DA1 response
   (copy the string from `TerminalState::report_da` in `internal.rs` lines 1106–1127).

7. In `process_output`, replace the `todo!()` arms:

   ```rust
   TerminalOutput::CursorReport => self.handle_cursor_report(),
   TerminalOutput::RequestDeviceAttributes => self.handle_request_device_attributes(),
   ```

8. Implement `handle_window_manipulation`:

   ```rust
   TerminalOutput::WindowManipulation(wm) => {
       self.window_commands.push(wm.clone());
   }
   ```

#### 3.5 Tests to add

- `cursor_report_sends_correct_position`: create a handler with a mock channel, move
  cursor to (4, 2), send `CursorReport` → channel receives `"\x1b[3;5R"` (1-indexed).
- `da1_sends_response`: send `RequestDeviceAttributes` → channel receives the DA1 string.
- `window_manipulation_queued`: send `WindowManipulation(SetTitleBarText("test"))` →
  `take_window_commands()` returns that command.
- `no_write_tx_does_not_panic`: cursor report with `write_tx = None` → no panic.

---

### Step 3.6 — Implement OSC response handling

- [ ] **Done**

#### 3.6 Why this step exists

`TerminalOutput::OscResponse(AnsiOscType)` is `todo!()`. OSC sequences handle window
titles, hyperlinks, and color queries. `SetTitleBar` is used by shells and editors
constantly.

#### 3.6 Files to change

- `freminal-buffer/src/terminal_handler.rs`

#### 3.6 What to implement

Add `handle_osc(&mut self, osc: &AnsiOscType)` and port the logic from
`TerminalState::osc_response` in `internal.rs` (lines 1129–1217):

| OSC variant                                   | Action                                                                                                                  |
| --------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `AnsiOscType::NoOp`                           | ignore                                                                                                                  |
| `AnsiOscType::Url(UrlResponse::Url(u))`       | set `self.current_format.url = Some(Url { url: u.clone() })` then `self.buffer.set_format(self.current_format.clone())` |
| `AnsiOscType::Url(UrlResponse::End)`          | clear `self.current_format.url = None` then sync format                                                                 |
| `AnsiOscType::SetTitleBar(t)`                 | push `WindowManipulation::SetTitleBarText(t.clone())` to `self.window_commands`                                         |
| `AnsiOscType::RequestColorQueryBackground(_)` | send hardcoded background color response via `write_to_pty` (copy from old code; mark with TODO to make configurable)   |
| `AnsiOscType::RequestColorQueryForeground(_)` | same for foreground                                                                                                     |
| `AnsiOscType::RemoteHost(_)`                  | debug log, ignore                                                                                                       |
| `AnsiOscType::Ftcs(_)`                        | debug log, ignore                                                                                                       |
| `AnsiOscType::ResetCursorColor`               | ignore for now (cursor color is not yet tracked by handler)                                                             |
| `AnsiOscType::ITerm2`                         | debug log, ignore                                                                                                       |

In `process_output`:

```rust
TerminalOutput::OscResponse(osc) => self.handle_osc(osc),
```

#### 3.6 Tests to add

- `osc_title_queued`: send `OscResponse(SetTitleBar("vim"))` → `take_window_commands()`
  returns `SetTitleBarText("vim")`.
- `osc_url_sets_format`: send URL start, write `"click"`, send URL end → the `"click"`
  cells have `tag.url` set; cells after URL end have `tag.url` as `None`.
- `osc_noop_does_not_panic`: send `OscResponse(NoOp)` → no panic, no side effects.

---

## Phase 4 — GUI Output API

### Step 4.1 — Implement `visible_as_tchars_and_tags` on Buffer

- [ ] **Done**

#### 4.1 Why this step exists

The GUI's `render_terminal_output` function calls `terminal_emulator.data_and_format_data_for_gui()`
which returns `(TerminalSections<Vec<TChar>>, TerminalSections<Vec<FormatTag>>)`. The
new buffer stores data as `Vec<Row>` of `Vec<Cell>`. This step creates the bridge method
that converts the new model into the format the GUI expects.

#### 4.1 Files to change

- `freminal-buffer/src/buffer.rs`

#### 4.1 What to implement

Add to `Buffer`:

```rust
pub fn visible_as_tchars_and_tags(&self) -> (Vec<TChar>, Vec<FormatTag>) { ... }
```

Algorithm:

1. Call `self.visible_rows()` to get the slice of rows to render.
2. Allocate `chars: Vec<TChar>` and `tags: Vec<FormatTag>`.
3. Track `byte_pos: usize = 0` — the current index into `chars`.
4. For each row in visible rows:
   - For each cell in `row.get_characters()`:
     - If `cell.is_continuation()`, skip (the head cell already accounts for the
       column width; continuations are internal bookkeeping only).
     - Otherwise, note the start byte position.
     - Append `cell.tchar().clone()` to `chars`.
     - If the last tag in `tags` has the same format as `cell.tag()` AND
       `last_tag.end == byte_pos` (adjacent), extend `last_tag.end += 1` (merge).
     - Otherwise, push a new `FormatTag { start: byte_pos, end: byte_pos + 1, ... }`.
     - Increment `byte_pos`.
   - After all cells in a row (unless it is the last row), push `TChar::NewLine`
     and extend the last `FormatTag` end by 1 (the newline inherits the last format).
     Increment `byte_pos`.
5. If `tags` is empty after processing, push a single default-format tag covering
   the entire range.
6. Ensure the last tag in `tags` has `end = chars.len()` (or `usize::MAX` if empty
   to match the old FormatTracker convention).

#### 4.1 Tests to add

- `empty_buffer_returns_empty`: new buffer → both vecs empty.
- `single_char_one_tag`: write `"A"`, `visible_as_tchars_and_tags()` → chars is
  `[TChar::Ascii(b'A')]`, one tag covering `[0, 1)`.
- `multiple_same_format_merged`: write `"ABC"` with same tag → one merged tag `[0, 3)`.
- `color_change_splits_tag`: write `"A"`, change fg color, write `"B"` → two tags.
- `newline_between_rows`: write `"hi"`, LF, CR, write `"bye"` → chars contains
  `NewLine` between them; tags span continuously.
- `wide_char_no_continuation_in_output`: write a wide char (CJK) → `chars` contains
  the `TChar::Utf8` once, not twice (continuation skipped).

---

### Step 4.2 — Implement `scrollback_as_tchars_and_tags` and the full GUI data API

- [ ] **Done**

#### 4.2 Why this step exists

Step 4.1 provides visible rows only. The old buffer also provides scrollback content
(though the current GUI passes `include_scrollback: false` for the main render path,
the data shape must match). This step completes the API surface and adds the method
to `TerminalHandler`.

#### 4.2 Files to change

- `freminal-buffer/src/buffer.rs`
- `freminal-buffer/src/terminal_handler.rs`

#### 4.2 What to implement

1. Add to `Buffer`:

   ```rust
   pub fn scrollback_as_tchars_and_tags(&self) -> (Vec<TChar>, Vec<FormatTag>) { ... }
   ```

   Same algorithm as `visible_as_tchars_and_tags` but over the scrollback rows
   (all rows before `visible_window_start()`). Returns `(vec![], vec![])` if
   `kind == BufferType::Alternate` (no scrollback in alternate).

2. Add to `TerminalHandler`:

   ```rust
   pub fn data_and_format_data_for_gui(
       &mut self,
   ) -> (TerminalSections<Vec<TChar>>, TerminalSections<Vec<FormatTag>>) {
       let (visible_chars, visible_tags) = self.buffer.visible_as_tchars_and_tags();
       let (scrollback_chars, scrollback_tags) = self.buffer.scrollback_as_tchars_and_tags();
       (
           TerminalSections { scrollback: scrollback_chars, visible: visible_chars },
           TerminalSections { scrollback: scrollback_tags,  visible: visible_tags  },
       )
   }
   ```

3. Add to `TerminalHandler`:

   ```rust
   pub fn cursor_pos(&self) -> CursorPos {
       self.buffer.get_cursor().pos
   }

   pub fn get_win_size(&self) -> (usize, usize) {
       (self.buffer.width, self.buffer.height)
   }
   ```

   (Add public `width` / `height` accessors to `Buffer` if not already present.)

#### 4.2 Tests to add

- `gui_data_visible_only`: write two rows, `data_and_format_data_for_gui().0.scrollback`
  is empty, `.visible` has content.
- `gui_data_scrollback_present`: in primary buffer, write enough lines to push content
  into scrollback; `scrollback` portion is non-empty.
- `cursor_pos_accessor`: move cursor, `cursor_pos()` returns matching value.
- `win_size_accessor`: `get_win_size()` returns the dimensions passed to `new`.

---

## Phase 5 — Integration into `freminal-terminal-emulator`

### Step 5.1 — Add `freminal-buffer` as a dependency; create parallel-run infrastructure

- [ ] **Done**

#### 5.1 Why this step exists

This step integrates `freminal-buffer` into `freminal-terminal-emulator` without
changing any visible behaviour. Both the old and new buffer process the same events;
the old buffer still drives the GUI. Discrepancies between old and new can be logged
for debugging.

#### 5.1 Files to change

- `freminal-terminal-emulator/Cargo.toml`
- `freminal-terminal-emulator/src/state/internal.rs`

#### 5.1 What to implement

1. Add to `freminal-terminal-emulator/Cargo.toml`:

   ```toml
   [dependencies]
   freminal-buffer = { path = "../freminal-buffer" }
   ```

2. Add a `shadow_handler: freminal_buffer::terminal_handler::TerminalHandler` field to
   `TerminalState`, constructed with the same width/height as the primary buffer.

3. In `TerminalState::handle_incoming_data`, after the existing `for segment in parsed`
   loop, add a second pass that feeds the same `parsed` slice to `shadow_handler`:

   ```rust
   self.shadow_handler.process_outputs(&parsed);
   ```

   The `parsed` vec must be cloned before the first loop if it is consumed by it,
   or the loop refactored to not consume it.

4. Wrap the shadow handler invocation in `#[cfg(debug_assertions)]` so it compiles away
   in release builds.

5. Call `shadow_handler.set_write_tx(self.write_tx.clone())` after constructing it so
   cursor-report writes from the shadow handler go to the PTY (or alternatively, skip
   this for debug builds and let them be dropped).

#### 5.1 Tests to add

- `shadow_handler_does_not_panic_on_basic_session`: push a realistic sequence of events
  (data, CR, LF, SGR, mode change) through `handle_incoming_data` in a test harness →
  no panics, old buffer state unchanged.

---

### Step 5.2 — Add `new-buffer` feature flag; gate GUI output on it

- [ ] **Done**

#### 5.2 Why this step exists

Once the shadow handler has been validated to produce correct output, this step adds a
Cargo feature that switches the GUI data source from the old buffer to the new one.
Both paths exist simultaneously; flipping the feature is the only change to user-facing
behaviour.

#### 5.2 Files to change

- `freminal-terminal-emulator/Cargo.toml`
- `freminal-terminal-emulator/src/state/internal.rs`
- `freminal-terminal-emulator/src/interface.rs`

#### 5.2 What to implement

1. Add to `freminal-terminal-emulator/Cargo.toml`:

   ```toml
   [features]
   new-buffer = []
   ```

2. In `TerminalState::data_and_format_data_for_gui`:

   ```rust
   #[cfg(feature = "new-buffer")]
   {
       return convert_new_buffer_output(self.shadow_handler.data_and_format_data_for_gui());
   }
   #[cfg(not(feature = "new-buffer"))]
   { /* existing implementation */ }
   ```

   The `convert_new_buffer_output` function adjusts the `TerminalSections` shape if
   needed (the new handler already returns the right type, so this may be a no-op cast).

3. Gate `cursor_pos()` and `show_cursor()` similarly.

4. Add a brief comment explaining how to enable:

   ```toml
   # To use the new buffer implementation:
   # cargo build --features new-buffer
   ```

#### 5.2 Tests to add

Build with `--features new-buffer` and run the full test suite. All tests must pass.
Add a CI note (comment in the Cargo.toml or a small shell script) documenting this.

---

### Step 5.3 — Enable `new-buffer` by default; remove parallel run

- [ ] **Done**

#### 5.3 Why this step exists

After Step 5.2 has been validated (ideally by running the real application with
`--features new-buffer`), this step makes the new buffer the default and removes the
shadow-run overhead.

#### 5.3 Files to change

- `freminal-terminal-emulator/Cargo.toml`
- `freminal-terminal-emulator/src/state/internal.rs`
- `freminal/freminal/Cargo.toml`

#### 5.3 What to implement

1. Change the feature definition:

   ```toml
   [features]
   default = ["new-buffer"]
   new-buffer = []
   old-buffer = []   # escape hatch if someone needs to revert
   ```

2. In `internal.rs`, remove the `shadow_handler` field and its parallel-run invocation.
   The `new-buffer` feature-gated paths in `data_and_format_data_for_gui` etc. become
   the only paths.

3. In `freminal/freminal/Cargo.toml`, ensure the default features are used (no explicit
   feature list override).

#### 5.3 Tests to add

- `cargo test --workspace` passes in default configuration.
- `cargo test --workspace --no-default-features` (old-buffer path) also passes.

---

## Phase 6 — Remove the Old Buffer

These steps are irreversible. Proceed only after Phase 5 has been running in production
for at least a few sessions without regressions.

---

### Step 6.1 — Delete `state/buffer.rs` and `format_tracker.rs`; remove all references

- [ ] **Done**

#### 6.1 Files to delete

- `freminal-terminal-emulator/src/state/buffer.rs`
- `freminal-terminal-emulator/src/format_tracker.rs`

#### 6.1 Files to change

- `freminal-terminal-emulator/src/state/mod.rs` — remove `pub mod buffer`
- `freminal-terminal-emulator/src/lib.rs` — remove `pub mod format_tracker` if present
- `freminal-terminal-emulator/src/state/internal.rs` — remove the old `Buffer` struct
  (lines 57–99), `TerminalState` fields (`primary_buffer`, `alternate_buffer`,
  `current_buffer`), `get_current_buffer()`, `clip_buffer_lines()`, all format-tracker
  calls, and all `#[cfg(not(feature = "new-buffer"))]` dead paths
- All `use` imports referencing deleted types

#### 6.1 What to verify

- `cargo build --workspace` compiles with zero errors.
- `cargo test --workspace` passes with zero failures.

---

### Step 6.2 — Final cleanup

- [ ] **Done**

#### 6.2 Files to change

- `freminal-buffer/src/lib.rs` — remove `#![allow(dead_code, unused_imports)]`
- All files — run `cargo clippy --workspace --all-targets -- -D warnings` and fix
  every warning.
- `freminal-buffer/PROGRESS.md` — update completion status.
- `freminal-buffer/API.md` — update to reflect completed SGR, modes, etc.
- This file (`MIGRATION_PLAN.md`) — all steps should be checked off.

#### 6.2 Tests to add

None new — this is purely a cleanup step. The test suite must continue to pass in its
entirety.

---

## Quick Reference: Responsibility Matrix

| Concern                       | Owned By                            |
| ----------------------------- | ----------------------------------- |
| Cursor position (x, y)        | `Buffer`                            |
| Current format / SGR state    | `Buffer` (via `current_tag`)        |
| Saved cursor (DECSC)          | `Buffer`                            |
| Row / cell content            | `Buffer`                            |
| Scrollback rows               | `Buffer`                            |
| Scroll offset (user scroll)   | `Buffer`                            |
| DECSTBM scroll region         | `Buffer`                            |
| LNM (LF implies CR)           | `Buffer` (`lnm_enabled`)            |
| DECAWM (auto-wrap)            | `Buffer` (`wrap_enabled`)           |
| Primary / alternate buffer    | `Buffer`                            |
| Terminal dimensions           | `Buffer`                            |
| Show cursor (Dectcem)         | `TerminalHandler`                   |
| Cursor visual style / blink   | `TerminalHandler`                   |
| DEC special graphics mode     | `TerminalHandler`                   |
| Window commands queue         | `TerminalHandler`                   |
| PTY write channel             | `TerminalHandler` (injected)        |
| Cursor key mode (Decckm)      | Terminal emulator layer             |
| Mouse tracking mode           | Terminal emulator layer             |
| Bracketed paste mode          | Terminal emulator layer             |
| Focus reporting mode          | Terminal emulator layer             |
| Synchronized updates          | Terminal emulator layer             |
| Screen inversion (Decscnm)    | Terminal emulator layer (rendering) |
| Theme                         | Terminal emulator / GUI layer       |
| Input encoding (key payloads) | Terminal emulator layer             |
