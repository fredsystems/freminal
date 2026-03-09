# Performance Planning — Freminal

## Status: Draft — Phase 2 Architecture

---

## ⚠️ Instructions for Future Agents ⚠️

This document is the single source of truth for the performance refactor. It is used by both
humans and agents to track progress.

**If you are an agent working from this document, you MUST follow this protocol exactly:**

1. Read this entire document before doing anything.
2. Find the first unchecked task (`- [ ]`) in the Implementation Checklist (Section 7).
3. Execute that one task and nothing else. Do not begin the next task.
4. When the task is complete, update this document: change the task's `- [ ]` to `- [x]` and
   add a brief completion note beneath it.
5. Run the verification command(s) listed for the task. Confirm they pass.
6. Stop. Post a summary of what you did and wait for the user to confirm before continuing.

**Do not execute multiple tasks in one session, even if they seem small.**
**Do not proceed to the next task without explicit user confirmation.**
**Do not modify any other part of this document except the specific task you completed.**

---

## 1. Executive Summary

The buffer refactor (Phase 1–6) replaced a flat `Vec`-backed format tracker with a structured
`Buffer` of `Row` → `Cell` objects. This gained correctness and eliminated several O(n²) re-scan
patterns, but the threading model was not revisited at the same time. The result is a system where
two threads contend on a single `FairMutex` across a wide critical section, and the render loop
performs mutations that have no business being there.

This document captures:

1. The as-built architecture and its problems.
2. A complete analysis of every mutation the render loop currently performs, and what category
   each one falls into.
3. The target architecture: lock-free rendering via `ArcSwap`, a fully PTY-owned emulator, and a
   clean channel-based input/event model.
4. The benchmark suite that must exist before and after the refactor to measure progress and catch
   regressions.
5. Scoped implementation steps for the architectural work.
6. Remaining lower-level optimisations that are independent of the architecture change.

---

## 2. Architecture: As-Built

```text
OS PTY fd
  └─ reader thread (spawned in run_terminal)
       reads 4096-byte chunks
       sends PtyRead { buf, read_amount } → crossbeam channel → main.rs loop

main.rs PTY consumer thread (spawned in main())
  recv PtyRead
  acquires FairMutex<TerminalEmulator>   ← LOCK HELD
    handle_incoming_data()
      ├─ UTF-8 leftover reassembly       (Vec<u8> clone every call)
      ├─ FreminalAnsiParser::push()      → Vec<TerminalOutput>
      └─ TerminalHandler::process_outputs()
            └─ Buffer mutations (insert_text, handle_lf, set_cursor_pos, …)
  releases FairMutex                     ← LOCK RELEASED
  calls ctx.request_repaint()

eframe update() — ~60 Hz or on repaint request
  acquires FairMutex<TerminalEmulator>   ← LOCK HELD for entire frame
    set_win_size()                       → Buffer::set_size() → full reflow if changed
    handle_window_manipulation()         → drains window_commands, writes Report* back to PTY
    FreminalTerminalWidget::show()
      ├─ write_input_to_terminal()       → terminal_emulator.write() → channel send to PTY
      ├─ scroll()                        → Buffer::scroll_offset mutation
      ├─ set_mouse_position*()           → TerminalState field mutation
      ├─ set_window_focused()            → TerminalState field mutation
      ├─ set_egui_ctx_if_missing()       → TerminalState field mutation
      ├─ needs_redraw() / set_previous_pass_invalid() / set_previous_pass_valid()
      └─ render_terminal_output()
            ├─ data_and_format_data_for_gui()
            │     ├─ Buffer::visible_as_tchars_and_tags()    ← clone every visible cell
            │     └─ Buffer::scrollback_as_tchars_and_tags() ← clone entire scrollback
            ├─ create_terminal_output_layout_job()
            │     ├─ TChar → Vec<u8>                         (allocation)
            │     └─ FormatTag offset remap                  (allocation)
            ├─ process_tags()            → builds egui LayoutJob sections
            └─ render_terminal_text()
                  └─ per-character painter.text() calls
                        fonts_mut() called TWICE per character inside the loop
  releases FairMutex                     ← LOCK RELEASED
```

### Problems with this model

**Lock contention.** The PTY consumer and the GUI hold the same `FairMutex` across wide critical
sections. During burst PTY output (cat of a large file, htop refresh, git log) the PTY side holds
the lock for the full parse + buffer-write cycle. The GUI holds it for the entire render pipeline.
These windows overlap constantly. The `FairMutex` prevents starvation but does nothing to reduce
the contention window.

**The render loop mutates terminal state.** `set_win_size`, `scroll`, `set_mouse_position`,
`set_window_focused`, `set_egui_ctx_if_missing`, `needs_redraw`/`set_previous_pass_*`, and the
`Report*` write-backs inside `handle_window_manipulation` all mutate the `TerminalEmulator` from
inside `update()`. A render loop should be a pure read of stable data. None of these mutations
belong there.

---

## 3. Render Loop Mutation Analysis

Every mutation currently performed in `update()` / `show()` is categorised below. Understanding
each one is necessary before the target architecture can be defined.

### 3.1 — Not real mutations: channel sends disguised as mutations

| Call site                                                 | What it actually does                                                                                                          |
| --------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `terminal_emulator.write(input)`                          | Sends bytes through a `crossbeam_channel::Sender` to the PTY writer thread. Never touches the buffer.                          |
| `terminal_emulator.internal.scroll()` in alternate screen | Sends arrow-key bytes through the write channel. Never touches the buffer.                                                     |
| `handle_window_manipulation` Report\* variants            | Calls `internal.report_*(…)` which calls `self.write()` which sends bytes through the write channel. Never touches the buffer. |

**Solution:** These should send directly to a `crossbeam_channel::Sender<InputEvent>` or
`Sender<PtyWrite>` that the GUI owns directly. The emulator does not need to be involved at all.

### 3.2 — GUI-local bookkeeping incorrectly placed on the emulator

| Call site                                                                      | What it should be                                                                                                                                                                       |
| ------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `set_mouse_position*()`                                                        | Field on `FreminalGui` / `FreminalTerminalWidget`. The emulator doesn't need to know the mouse pixel position.                                                                          |
| `set_window_focused()`                                                         | Field on `FreminalGui`. Focus change sends a `TerminalInput::InFocus/LostFocus` through the input channel — no emulator mutation needed.                                                |
| `set_egui_ctx_if_missing()`                                                    | One-time setup. Move to construction. The PTY thread signals repaint via `ctx.request_repaint()` on a `Context` it receives at startup, not one injected through a lock-guarded setter. |
| `needs_redraw()` / `set_previous_pass_valid()` / `set_previous_pass_invalid()` | The GUI's own "do I need to re-render?" flag. Should live on `FreminalTerminalWidget`, driven by whether a new snapshot has arrived.                                                    |

**Solution:** Remove these fields from `TerminalState`/`TerminalEmulator` entirely. Keep them on
the GUI side.

### 3.3 — Scroll offset

`scroll()` (in primary screen mode) mutates `Buffer::scroll_offset`. This is pure view state: how
far the user has scrolled back through history. The PTY side has no interest in this value. It
currently lives on `Buffer` because `Buffer::visible_rows()` uses it to compute the display window,
and `insert_text`/`handle_lf` reset it to 0 when new data arrives.

**Solution:** Move `scroll_offset` out of `Buffer` and into a `ViewState` struct owned by the GUI.
The snapshot exposed to the GUI includes the full `rows` data (behind an `Arc` so no copy is
needed). The GUI selects its own display window by applying `scroll_offset` locally. When a new
snapshot arrives, the GUI resets `scroll_offset` to 0 if content changed while the user was scrolled
back. The PTY side never sees `scroll_offset` again.

### 3.4 — Resize: the one genuine mutation

`set_win_size()` is called on every frame. It computes available pixel area, divides by character
size, and calls `Buffer::set_size()` which triggers a full reflow when dimensions change. This is a
real mutation of buffer content and it happens from the render loop.

**Solution:** Detect window resize as an event, not a per-frame poll. The GUI computes
`(width_chars, height_chars)` from the available size and compares it to the last known size it
sent. When they differ, it sends a `PtyWrite::Resize` through the existing resize channel. The PTY
thread applies the resize to the buffer before the next snapshot. The GUI never calls into the
buffer for resize. This matches how the PTY's own resize already works via `PtyWrite::Resize`.

### 3.5 — Window manipulation Report\* write-backs

When the PTY program queries the window state (e.g. "what is your size in characters?"), the
handler queues a `WindowManipulation::Report*` command. The current code drains these from
`terminal_emulator.internal.window_commands` inside `update()`, gathers the answer from the
viewport, then calls back into the emulator to write the response.

**Solution:** The GUI owns a `Sender<PtyWrite>` directly. When it processes a `Report*` command
from the window-command channel, it constructs the response string and sends it through its own
sender without touching the emulator at all.

---

## 4. Target Architecture

The `FairMutex` is eliminated entirely. There is no shared mutable state between the GUI thread
and the PTY-processing thread at steady state.

```text
┌──────────────────────────────────────────────────────────────────┐
│  OS PTY fd                                                       │
│    └─ reader thread: reads chunks, sends PtyRead over channel    │
└──────────────────────────────────────────────────────────────────┘
                          │ PtyRead channel
                          ▼
┌──────────────────────────────────────────────────────────────────┐
│  PTY Processing Thread  (owns TerminalEmulator exclusively)      │
│                                                                  │
│  loop {                                                          │
│    select! {                                                     │
│      recv(pty_read_rx)  → handle_incoming_data()                 │
│      recv(input_rx)     → handle input event                     │
│                           (key bytes → PTY writer channel,       │
│                            resize    → Buffer::set_size(),       │
│                            focus     → focus-report bytes)       │
│    }                                                             │
│    // After each batch: publish snapshot                         │
│    let snap = Arc::new(build_snapshot(&emulator));               │
│    arc_swap.store(snap);                                         │
│    ctx.request_repaint();                                        │
│  }                                                               │
└──────────────────────────────────────────────────────────────────┘
      │ input_tx (crossbeam Sender<InputEvent>)   ▲
      │ pty_write_tx (crossbeam Sender<PtyWrite>) ▲  (GUI sends here directly)
      ▼ ArcSwap<TerminalSnapshot>                 │  (GUI reads here)
      ▼ WindowCommand channel                     │  (PTY → GUI, for Report* handling)
┌──────────────────────────────────────────────────────────────────┐
│  GUI Thread  (eframe update())                                   │
│                                                                  │
│  let snap = arc_swap.load();   ← atomic pointer load, no lock   │
│                                                                  │
│  // Drain window commands and handle:                            │
│  //   viewport commands → ctx.send_viewport_cmd()               │
│  //   Report* → build response, send via pty_write_tx directly  │
│                                                                  │
│  // Handle input events: keyboard, mouse, scroll                 │
│  //   key/mouse bytes → input_tx.send(InputEvent::Key(…))       │
│  //   scroll          → update local ViewState::scroll_offset   │
│  //   resize detected → input_tx.send(InputEvent::Resize(…))    │
│                                                                  │
│  // Render purely from snap — no lock, no mutation               │
│  render(snap, view_state);                                       │
└──────────────────────────────────────────────────────────────────┘
```

### 4.1 — TerminalSnapshot

`TerminalSnapshot` is the data contract between the PTY thread and the GUI thread. It is produced
by the PTY thread after each processed batch and published atomically. The GUI reads it lock-free.

```rust
pub struct TerminalSnapshot {
    /// Flattened visible content, already converted from Row/Cell.
    /// Produced once on the PTY side; GUI reads directly.
    pub visible_chars: Vec<TChar>,
    pub visible_tags: Vec<FormatTag>,

    /// Raw rows behind an Arc so the GUI can apply its own scroll_offset
    /// without copying all row data. Only needed once scrollback rendering
    /// is active; can be None until then.
    pub rows: Arc<Vec<Row>>,

    /// Total number of rows (scrollback + visible) so the GUI can compute
    /// the max scroll offset without touching rows directly in most cases.
    pub total_rows: usize,

    /// Height of the visible window in rows.
    pub height: usize,

    /// Cursor position in screen coordinates (0-indexed).
    pub cursor_pos: CursorPos,
    pub show_cursor: bool,
    pub cursor_visual_style: CursorVisualStyle,

    /// Whether the alternate screen is active.
    pub is_alternate_screen: bool,

    /// Whether the display is in normal (non-inverted) mode.
    pub is_normal_display: bool,

    /// Terminal dimensions in characters (for resize detection by the GUI).
    pub term_width: usize,
    pub term_height: usize,

    /// Set to true when visible content changed since the previous snapshot.
    /// The GUI uses this to reset ViewState::scroll_offset to 0 when the
    /// user is scrolled back and new output arrives.
    pub content_changed: bool,
}
```

Key properties:

- Produced after every `handle_incoming_data` call and after every `InputEvent::Resize`.
- The `visible_chars` / `visible_tags` fields are **already flattened** by the PTY thread, which
  has idle time between frames anyway. The GUI pays zero cost to flatten.
- `Arc<Vec<Row>>` for the full row data means the snapshot is cheap to produce (reference count
  bump). The data is only truly copied if the GUI holds a reference past the next swap.
- When scrollback rendering is not active, `rows` can be an empty `Arc<Vec<Row>>`.

### 4.2 — ArcSwap

`arc_swap::ArcSwap<TerminalSnapshot>` (from the `arc-swap` crate) provides:

- **Store:** PTY thread calls `arc_swap.store(Arc::new(snapshot))`. O(1), never blocks.
- **Load:** GUI thread calls `arc_swap.load()`. Returns a guard holding an `Arc`. O(1), never
  blocks, no lock.

The old snapshot's `Arc` is dropped when the GUI releases its load guard and the PTY thread swaps
in the new one. Memory is reclaimed automatically via reference counting. There is no double-free
and no data race.

### 4.3 — InputEvent channel

```rust
pub enum InputEvent {
    Key(Vec<u8>),           // raw bytes to write to the PTY
    Resize(usize, usize),   // new (width_chars, height_chars)
    FocusChange(bool),      // focused / unfocused
}
```

The GUI sends these through a `crossbeam_channel::Sender<InputEvent>`. The PTY processing thread
receives them in the same `select!` loop as `PtyRead`. This means:

- Keyboard input: GUI sends `InputEvent::Key(bytes)` → PTY thread forwards to PTY writer.
- Resize: GUI sends `InputEvent::Resize(w, h)` → PTY thread calls `Buffer::set_size(w, h)` and
  sends `PtyWrite::Resize` to the PTY writer.
- Focus: GUI sends `InputEvent::FocusChange(focused)` → PTY thread sends focus-report bytes if
  the mode is enabled.
- Scroll (primary screen): GUI updates its own `ViewState::scroll_offset` locally. No PTY
  involvement needed.
- Scroll (alternate screen): GUI sends `InputEvent::Key(arrow_key_bytes)`.

### 4.4 — WindowCommand channel

```rust
pub enum WindowCommand {
    Viewport(WindowManipulation),  // commands that need ctx.send_viewport_cmd()
    Report(WindowManipulation),    // queries that need a PTY write-back
}
```

The PTY thread sends these through a `crossbeam_channel::Sender<WindowCommand>`. The GUI drains
them at the start of each `update()` with `try_recv()` in a loop (non-blocking). For `Report*`
variants, the GUI reads the viewport geometry, builds the response bytes, and sends them via its
own `Sender<PtyWrite>` — the PTY thread is not involved.

### 4.5 — ViewState

```rust
pub struct ViewState {
    pub scroll_offset: usize,
    pub mouse_position: Option<egui::Pos2>,
    pub window_focused: bool,
    pub last_sent_size: (usize, usize),       // to detect resize
    pub previous_key: Option<egui::Key>,
    pub previous_scroll_amount: f32,
    pub previous_mouse_state: Option<PreviousMouseState>,
}
```

Owned entirely by `FreminalGui`. Never shared with the PTY thread. The emulator no longer carries
any of these fields.

---

## 5. What Gets Deleted

The following components are removed as a consequence of this refactor:

| Component                                                            | Location                | Why                                                                              |
| -------------------------------------------------------------------- | ----------------------- | -------------------------------------------------------------------------------- |
| `FairMutex<TerminalEmulator>`                                        | `main.rs`, `gui/mod.rs` | Replaced by `ArcSwap` + channels                                                 |
| `Arc<FairMutex<TerminalEmulator>>` field on `FreminalGui`            | `gui/mod.rs`            | Emulator moves to PTY thread                                                     |
| `TerminalState::mouse_position`                                      | `state/internal.rs`     | Moves to `ViewState`                                                             |
| `TerminalState::window_focused`                                      | `state/internal.rs`     | Moves to `ViewState`                                                             |
| `TerminalState::changed` / `clear_changed()` / `set_state_changed()` | `state/internal.rs`     | `needs_redraw` concept moves to GUI via snapshot generation                      |
| `TerminalState::ctx` / `set_ctx()` / `request_redraw()`              | `state/internal.rs`     | `Context` passed directly to PTY thread at construction                          |
| `TerminalEmulator::set_previous_pass_invalid/valid()`                | `interface.rs`          | Moves to `FreminalTerminalWidget`                                                |
| `TerminalEmulator::needs_redraw()`                                   | `interface.rs`          | GUI tracks this locally: has the `ArcSwap` pointer changed since the last frame? |
| `TerminalEmulator::set_egui_ctx_if_missing()`                        | `interface.rs`          | Context passed at construction                                                   |
| `TerminalEmulator::set_mouse_position*()`                            | `interface.rs`          | Moves to `ViewState`                                                             |
| `TerminalEmulator::set_window_focused()`                             | `interface.rs`          | Moves to `ViewState`; focus event sent through `InputEvent` channel              |
| `TerminalEmulator::request_redraw()`                                 | `interface.rs`          | PTY thread calls `ctx.request_repaint()` directly                                |
| `Buffer::scroll_offset`                                              | `buffer.rs`             | Moves to `ViewState`                                                             |
| Internal bytes-forwarding thread in `TerminalState::new()`           | `state/internal.rs`     | Collapsed: `TerminalHandler` holds `Sender<PtyWrite>` directly                   |
| `TerminalEmulator::dummy_for_bench()`                                | `interface.rs`          | Replace with benchmark helpers that operate on `TerminalState` directly          |

---

## 6. What Stays / Is Restructured

- `TerminalState`, `TerminalHandler`, `Buffer`, `Row`, `Cell`, `FreminalAnsiParser` — all
  unchanged internally. The refactor is purely about ownership and threading, not processing logic.
- `FreminalTerminalWidget::show()` becomes a pure render function. It takes `&TerminalSnapshot`
  and `&mut ViewState` and has no access to the emulator at all.
- `handle_window_manipulation` becomes a standalone function that takes a
  `Receiver<WindowCommand>`, a `&Sender<PtyWrite>`, and the egui `Ui`. No emulator parameter.
- The `write_input_to_terminal` function sends to `&Sender<InputEvent>` instead of
  `&mut TerminalEmulator<Io>`. The `Io` type parameter on everything in `terminal.rs` goes away.

---

## 7. Implementation Checklist

> **Agent instructions:** Find the first unchecked item below. Execute it and nothing else.
> Update its checkbox to `[x]` and add a short completion note. Run the verification steps.
> Then **stop and wait for user confirmation** before touching anything else.

The steps are strictly ordered. Each one must be complete and verified before the next begins.
No step should leave the tree in a state where `cargo test --all` fails.

---

- [x] **Task 1 — Add `arc-swap` dependency and define `TerminalSnapshot`**
  - Add `arc-swap` to the workspace `Cargo.toml` and to `freminal-terminal-emulator/Cargo.toml`.
  - Create `freminal-terminal-emulator/src/snapshot.rs` and define the `TerminalSnapshot` struct
    exactly as specified in Section 4.1.
  - Add `pub mod snapshot;` to `freminal-terminal-emulator/src/lib.rs`.
  - Add a `build_snapshot(&self) -> TerminalSnapshot` method to `TerminalEmulator` in
    `interface.rs`. Initially it calls `data_and_format_data_for_gui()` and copies cursor/mode
    fields. `content_changed` can be hardcoded `true` for now.
  - No existing behaviour changes. No call sites updated yet.
  - **Verify:** `cargo test --all` passes. `cargo build --all` passes.
  - ✅ **Completed 2026-03-09.** Added `arc-swap = "1.7.1"` to workspace deps and
    `arc-swap.workspace = true` to `freminal-terminal-emulator/Cargo.toml`. Created
    `freminal-terminal-emulator/src/snapshot.rs` with `TerminalSnapshot` matching the Section 4.1
    spec exactly (plus `#[allow(clippy::struct_excessive_bools)]` for the four semantic flags).
    Added `pub mod snapshot;` to `lib.rs`. Added `build_snapshot(&mut self) -> TerminalSnapshot`
    to `TerminalEmulator` in `interface.rs`; it extracts handler-derived fields first (immutable
    borrow), then calls the `&mut self` methods on `internal`, and assembles the snapshot with
    `content_changed: true` hardcoded. `cargo test --all` — 237 tests passed, 0 failed.
    `cargo build --all` — clean. No clippy warnings.

---

- [x] **Task 2 — Define `InputEvent` and `WindowCommand` channel types**
  - Add `InputEvent` and `WindowCommand` enums to
    `freminal-terminal-emulator/src/io/mod.rs` alongside the existing `PtyRead` / `PtyWrite`.
  - Use the definitions from Section 4.3 and 4.4 exactly.
  - No call sites wired up yet — these are type definitions only.
  - **Verify:** `cargo test --all` passes. `cargo build --all` passes.
  - ✅ **Completed 2026-03-09.** Added `InputEvent { Key(Vec<u8>), Resize(usize, usize),
    FocusChange(bool) }` and `WindowCommand { Viewport(WindowManipulation),
    Report(WindowManipulation) }` to `freminal-terminal-emulator/src/io/mod.rs`.
    `WindowManipulation` is referenced via its full path from `freminal-common`.
    No call sites wired up. `cargo build --all` clean, `cargo test --all` passed,
    no diagnostics.

---

- [x] **Task 3 — Write and baseline the benchmark suite**
  - Implement all benchmarks described in Section 8 across all three crates before any
    optimisation work begins. The benchmarks must compile and produce stable numbers against the
    current (pre-refactor) code.
  - Fix `freminal-buffer/benches/buffer_row_bench.rs`: replace the external file dependency with
    inline generated data. Keep the file-based variant behind
    `#[cfg(feature = "bench_fixtures")]`. Add the new benchmarks from Section 8.3.
  - Replace `freminal-terminal-emulator/benches/buffer_benches.rs` with the benchmarks from
    Section 8.4. (`bench_build_snapshot` can be added but will be trivial until Task 1 is fully
    wired.)
  - Rewrite `freminal/benches/render_loop_bench.rs` with the benchmarks from Section 8.5.
    The `TerminalEmulator`-based tests remain valid for now; the `TerminalSnapshot`-based tests
    will be completed after Task 8.
  - Save a formal baseline: `cargo bench --all -- --save-baseline before_refactor`
  - **Verify:** All benchmarks compile. All benchmarks produce output (no panics, no
    empty results). Baseline is saved successfully.
  - ✅ **Completed 2026-03-09.** All three benchmark files rewritten/augmented. Added
    `bench_fixtures` feature to `freminal-buffer/Cargo.toml` to gate the file-based variant.
    Replaced the no-op stub in `buffer_benches.rs` with 7 real benchmarks. Rewrote
    `render_loop_bench.rs` with `iter_batched` (fresh terminal per sample), removed the
    meaningless `logic_only` benchmark, added ANSI-heavy, bursty, and snapshot variants.
    Replaced all deprecated `criterion::black_box` calls with `std::hint::black_box`.
    All 18 benchmarks pass `--test` (no panics). Baseline saved per-crate via
    `--save-baseline before_refactor`. Numbers recorded in Section 8.2.

---

- [x] **Task 4 — Move `scroll_offset` out of `Buffer`**
  - Remove the `scroll_offset` field from `Buffer` in `freminal-buffer/src/buffer.rs`.
  - Create `freminal/src/gui/view_state.rs` and define the `ViewState` struct as specified in
    Section 4.5. Add `pub mod view_state;` to `freminal/src/gui/mod.rs`.
  - Update `Buffer::visible_rows()` to accept `scroll_offset: usize` as a parameter instead of
    reading from `self`.
  - Update all internal `Buffer` call sites that previously relied on `self.scroll_offset`
    (primarily `insert_text`, `handle_lf`, and `max_scroll_offset`).
  - The scroll-reset-on-new-data logic currently in `insert_text` / `handle_lf` is removed from
    `Buffer`. Instead, `build_snapshot` (Task 1) sets `content_changed: true` whenever the visible
    content has changed, so the GUI can reset `ViewState::scroll_offset` itself.
  - Update `visible_as_tchars_and_tags` and `scrollback_as_tchars_and_tags` call paths as needed.
  - For now, callers outside the buffer that previously read `scroll_offset` from the buffer
    should be updated to pass `0` temporarily; the correct `ViewState` wiring happens in Task 7.
  - **Verify:** `cargo test --all` passes. All buffer unit tests pass.
  - ✅ **Completed 2026-03-09.** Removed `scroll_offset` field from `Buffer` struct.
    Changed `visible_rows`, `visible_window_start`, `visible_as_tchars_and_tags`,
    `scrollback_as_tchars_and_tags` to accept `scroll_offset: usize` parameter.
    `set_size(w, h, scroll_offset) -> usize` now takes and returns the offset (reflow
    resets to 0; resize clamps or resets per `preserve_scrollback_anchor`).
    `enter_alternate(scroll_offset)` saves the external offset into `SavedPrimaryState`;
    `leave_alternate() -> usize` returns the restored offset. `enforce_scrollback_limit`
    takes and returns the adjusted offset. `scroll_back` / `scroll_forward` /
    `scroll_to_bottom` are now pure functions that compute and return the new offset
    without mutating `self`. Scroll-reset-on-data guards removed from `insert_text` and
    `handle_lf`; `scroll_offset > 0` guards removed from `insert_lines` and
    `delete_lines` (PTY always operates at live bottom). `max_scroll_offset()` made
    `pub`. All internal PTY-side callers pass `0`. `TerminalHandler::handle_scroll_back
    / forward` now take and return offset; `handle_scroll_to_bottom` is a const fn.
    `TerminalState::scroll()` discards returned offsets temporarily (wired in Task 7/8).
    Created `freminal/src/gui/view_state.rs` with `ViewState` matching Section 4.5 spec.
    Added `pub mod view_state` to `gui/mod.rs`. Updated all tests (buffer unit tests,
    integration tests, benchmarks, and external test files). Committed as 758903b.
    `cargo test --all`: 498 tests passed, 0 failed. `cargo build --all`: clean.

---

- [ ] **Task 5 — Move GUI-local fields off `TerminalState`**
  - Remove the following fields from `TerminalState` in
    `freminal-terminal-emulator/src/state/internal.rs`:
    `mouse_position`, `window_focused`, `ctx`, `changed`.
  - Remove the associated methods: `set_mouse_position*`, `set_window_focused`, `set_ctx`,
    `set_state_changed`, `clear_changed`, `is_changed`, `request_redraw`.
  - Remove the mirrored methods from `TerminalEmulator` in `interface.rs`:
    `set_mouse_position*`, `set_window_focused`, `set_egui_ctx_if_missing`,
    `needs_redraw`, `set_previous_pass_invalid`, `set_previous_pass_valid`, `request_redraw`.
  - Add equivalent fields to `ViewState` (Task 4) where they are needed by the GUI.
  - Update all call sites in `freminal/src/gui/terminal.rs` and `freminal/src/gui/mod.rs` to use
    `ViewState` fields directly instead of going through the emulator. The emulator is still
    locked at this stage — the plumbing changes but the lock is not removed yet.
  - **Verify:** `cargo test --all` passes. The application compiles and runs.

---

- [ ] **Task 6 — Collapse the internal PTY write forwarding thread**
  - In `TerminalState::new()` in `freminal-terminal-emulator/src/state/internal.rs`, remove the
    `bytes_tx` / `bytes_rx` channel pair and the spawned forwarding thread.
  - Update `TerminalHandler::set_write_tx` (in `freminal-buffer/src/terminal_handler.rs`) to
    accept `crossbeam_channel::Sender<PtyWrite>` directly instead of `Sender<Vec<u8>>`.
  - Update `TerminalHandler`'s internal `write_to_pty` method to send `PtyWrite::Write(bytes)`
    directly.
  - Pass the existing `write_tx: Sender<PtyWrite>` from `TerminalState` directly into the handler.
  - **Verify:** `cargo test --all` passes. PTY write-back responses (device attribute queries,
    cursor position reports, etc.) still function correctly end-to-end.

---

- [ ] **Task 7 — Move resize out of the render loop**
  - In `FreminalGui::update()` in `freminal/src/gui/mod.rs`, replace the
    `lock.set_win_size(width_chars, height_chars, …)` call with a comparison against
    `ViewState::last_sent_size`.
  - When the computed size differs from `last_sent_size`, send `InputEvent::Resize(w, h)` through
    a `Sender<InputEvent>`. Update `last_sent_size` after sending.
  - For now the `Sender<InputEvent>` can be a stub that the PTY consumer thread already holds as a
    `Receiver<InputEvent>` (wired up in Task 8). The channel can be created in `main.rs` and
    passed through to `FreminalGui`. The PTY consumer thread can handle the `InputEvent::Resize`
    variant in its existing `recv` loop (not yet a `select!` — that comes in Task 8).
  - Remove `set_win_size` from the render-loop lock-held path entirely.
  - **Verify:** `cargo test --all` passes. Resizing the terminal window causes the PTY to receive
    the correct new dimensions.

---

- [ ] **Task 8 — Move the PTY consumer thread off the `FairMutex` (central step)**

  This is the step where the `FairMutex` is eliminated. All prior tasks have been removing
  mutations from the GUI side to make this step safe.
  - In `freminal/src/main.rs`, create:
    - `Arc<ArcSwap<TerminalSnapshot>>` — shared between the PTY thread and `FreminalGui`.
    - `crossbeam_channel` pair for `InputEvent` — sender to `FreminalGui`, receiver to PTY thread.
    - `crossbeam_channel` pair for `WindowCommand` — sender to PTY thread, receiver to `FreminalGui`.
  - Move `TerminalEmulator` into the PTY consumer thread closure. It is no longer wrapped in
    `Arc<FairMutex>`. The thread takes full ownership.
  - Replace the thread's `recv()` loop with a `crossbeam::select!` over `pty_read_rx` and
    `input_rx`. Handle each `InputEvent` variant:
    - `Key(bytes)` → send `PtyWrite::Write(bytes)` to the PTY writer channel.
    - `Resize(w, h)` → call `emulator.internal.set_win_size(w, h)`, send `PtyWrite::Resize`.
    - `FocusChange(focused)` → send focus-report bytes if the terminal mode requires it.
  - After processing each batch of work (PTY data or input event), call
    `emulator.build_snapshot()`, wrap in `Arc`, and store via `arc_swap.store(…)`. Then call
    `ctx.request_repaint()`.
  - When the emulator produces `WindowManipulation` commands via `take_window_commands()`, send
    them through the `WindowCommand` sender to the GUI.
  - Update `FreminalGui` to hold:
    - `Arc<ArcSwap<TerminalSnapshot>>` instead of `Arc<FairMutex<TerminalEmulator>>`.
    - `Sender<InputEvent>`.
    - `Sender<PtyWrite>` (for Report\* write-backs from `handle_window_manipulation`).
    - `Receiver<WindowCommand>`.
    - `ViewState`.
  - In `FreminalGui::update()`, replace `self.terminal_emulator.lock()` with
    `self.arc_swap.load()`.
  - **Verify:** `cargo test --all` passes. The application runs. PTY output appears. Keyboard
    input works. Window resize works.

---

- [ ] **Task 9 — Refactor `FreminalTerminalWidget::show()` to take `&TerminalSnapshot`**
  - Remove the `Io: FreminalTermInputOutput` type parameter from `show()`,
    `render_terminal_output()`, `add_terminal_data_to_ui()`, and all related functions in
    `freminal/src/gui/terminal.rs`.
  - Change `show()` signature to:

    ```text
    pub fn show(
        &mut self,
        ui: &mut Ui,
        snap: &TerminalSnapshot,
        view_state: &mut ViewState,
        input_tx: &Sender<InputEvent>,
        pty_write_tx: &Sender<PtyWrite>,
    )
    ```

  - Change `render_terminal_output()` to take `&TerminalSnapshot` instead of
    `&mut TerminalEmulator<Io>`. It reads `snap.visible_chars`, `snap.visible_tags`,
    `snap.cursor_pos`, etc. directly.
  - Change `write_input_to_terminal()` to take `&Sender<InputEvent>` and `&mut ViewState`
    instead of `&mut TerminalEmulator<Io>`. Input events are sent through the channel. Mouse
    position, scroll offset, and focus state are read from / written to `ViewState`.
  - Update `FreminalGui::update()` to pass the loaded snapshot, view state, and channel senders
    into `show()`.
  - **Verify:** `cargo test --all` passes. The application renders correctly. Input works.
    Cursor is painted at the correct position from snapshot data.

---

- [ ] **Task 10 — Refactor `handle_window_manipulation`**
  - Change the signature of `handle_window_manipulation` in `freminal/src/gui/mod.rs` to:

    ```text
    fn handle_window_manipulation(
        ui: &egui::Ui,
        window_cmd_rx: &Receiver<WindowCommand>,
        pty_write_tx: &Sender<PtyWrite>,
        font_width: usize,
        font_height: usize,
        window_width: egui::Rect,
        title_stack: &mut Vec<String>,
    )
    ```

  - Replace the `window_commands.drain()` from the emulator with `window_cmd_rx.try_recv()` in
    a loop.
  - For `Report*` variants: read the viewport geometry from `ui.ctx()`, build the response string,
    and send it via `pty_write_tx.send(PtyWrite::Write(bytes))`. No emulator call needed.
  - For `Viewport` variants: call `ui.ctx().send_viewport_cmd(…)` as before.
  - **Verify:** `cargo test --all` passes. Window manipulation commands (title changes,
    resize-to-columns, minimize, fullscreen, etc.) work correctly. Report\* responses reach the
    PTY.

---

- [ ] **Task 11 — Delete dead code and clean up**
  - Remove `FairMutex` import and all usages everywhere in the `freminal` crate.
  - Verify `parking_lot` is no longer needed by `freminal/Cargo.toml` and remove it if so
    (check all transitive uses first).
  - Remove all fields and methods listed in the Section 5 deletion table that have not already
    been removed in Tasks 4–7.
  - Remove the `Io: FreminalTermInputOutput` type parameter from `FreminalGui` and
    `FreminalTerminalWidget` (the emulator is no longer a field on either).
  - Remove `TerminalEmulator::dummy_for_bench()` from `interface.rs`. Update any benchmark
    harnesses that used it to construct a `TerminalState` directly.
  - Remove the `#[allow(clippy::significant_drop_tightening)]` suppress at the top of
    `freminal/src/gui/mod.rs` — it was there because of the long lock hold, which is now gone.
  - Run `cargo clippy --all -- -D warnings` and fix all warnings.
  - **Verify:** `cargo test --all` passes. `cargo clippy --all -- -D warnings` passes clean.
    `cargo build --release --all` produces a working binary.

---

- [ ] **Task 12 — Complete and re-baseline the benchmark suite**
  - Update `freminal/benches/render_loop_bench.rs` to use `&TerminalSnapshot` directly now that
    `show()` has been refactored in Task 9. Remove the `TerminalEmulator`-based render benchmarks
    and replace them with the snapshot-based variants from Section 8.5.
  - Add the `bench_build_snapshot` benchmark to
    `freminal-terminal-emulator/benches/buffer_benches.rs` now that `build_snapshot` is fully
    wired.
  - Add the `ArcSwap` round-trip latency benchmark described at the end of Section 8.5.
  - Run the full benchmark suite and save results against the pre-refactor baseline:
    `cargo bench --all -- --baseline before_refactor`
  - Record the key before/after numbers in Section 8.2 of this document.
  - **Verify:** All benchmarks compile and run without panics. Criterion produces before/after
    comparison reports. Results are noted in this document.

---

## 8. Benchmark Plan

The goal is a suite that:

1. Can be run in CI to catch regressions.
2. Covers the hot paths before and after the refactor.
3. Mixes synthetic loads (isolate a single function) with realistic loads (simulate real PTY
   output patterns: bursty, large, ANSI-heavy, TUI cursor-heavy).
4. Does not depend on external fixture files at runtime (inline fallbacks for any test data).

### 8.1 Existing Benchmark Audit

| Crate                        | File                   | Current State                                                                                             | Action                                     |
| ---------------------------- | ---------------------- | --------------------------------------------------------------------------------------------------------- | ------------------------------------------ |
| `freminal-buffer`            | `buffer_row_bench.rs`  | Sound but depends on external `10000_lines.txt` file; missing flatten, tag, and cursor-op coverage        | Fix fixture dependency; add new benchmarks |
| `freminal-terminal-emulator` | `buffer_benches.rs`    | No-op stub — all content removed with old buffer                                                          | Replace entirely                           |
| `freminal`                   | `render_loop_bench.rs` | Terminal not reset between iterations; `logic_only` measures ~177ps and is meaningless; no ANSI, no burst | Rewrite                                    |

### 8.2 Benchmark Numbers

**Pre-refactor baseline (captured before Task 3):**

**`freminal-buffer` (`buffer_row_bench.rs`) — inline 500 000-elem dataset:**

| Benchmark                                                    | Time      | Throughput      |
| ------------------------------------------------------------ | --------- | --------------- |
| `buffer_insert_large_line/insert_full/500000`                | ~1.32 s   | ~378 Kelem/s    |
| `buffer_insert_chunks/insert_chunks_1000/500`                | ~35.1 ms  | ~14.2 Melem/s   |
| `buffer_resize/reflow_width/40`                              | ~1.35 s   | —               |
| `buffer_resize/shrink_height/20`                             | ~1.33 s   | —               |
| `softwrap_heavy/wrap_long_line_to_width_10`                  | ~361 µs   | —               |
| `bench_visible_flatten/visible_200x50`                       | ~30.5 µs  | ~327 Melem/s    |
| `bench_scrollback_flatten/scrollback_1024_rows`              | ~289 µs   | ~283 Melem/s    |
| `bench_insert_with_color_changes/color_change_every_8_chars` | ~141 µs   | ~28.4 Melem/s   |
| `bench_cursor_ops/cup_then_data_24x80`                       | ~61.0 µs  | ~31.5 Melem/s   |
| `bench_lf_heavy/lf_4100_times`                               | ~3.89 ms  | ~1.05 Melem/s   |
| `bench_erase_display/erase_to_end_of_display_80x24`          | ~22.8 µs  | —               |

**`freminal-terminal-emulator` (`buffer_benches.rs`):**

| Benchmark                                            | Time      | Throughput      |
| ---------------------------------------------------- | --------- | --------------- |
| `bench_parse_plain_text/parser_push/4096`            | ~9.74 µs  | ~401 MiB/s      |
| `bench_parse_sgr_heavy/parser_push_sgr/4097`         | ~98.7 µs  | ~39.6 MiB/s     |
| `bench_parse_cup_writes/parse_and_handle_80x24`      | ~118 µs   | ~16.8 MiB/s     |
| `bench_parse_bursty/bursty_10_small_plus_1_large`    | ~278 µs   | ~14.2 MiB/s     |
| `bench_handle_incoming_data/handle_incoming_data_4096` | ~276 µs | ~14.2 MiB/s     |
| `bench_data_and_format_for_gui/flatten_80x24`        | ~5.81 µs  | ~330 Melem/s    |
| `bench_build_snapshot/build_snapshot_80x24`          | ~16.1 µs  | ~119 Melem/s    |

**`freminal` (`render_loop_bench.rs`):**

| Benchmark                                                          | Time      | Throughput    |
| ------------------------------------------------------------------ | --------- | ------------- |
| `render_terminal_text/feed_data_incremental/100_lines`             | ~383 µs   | ~12.9 MiB/s   |
| `render_terminal_text/feed_data_incremental/1000_lines`            | ~5.94 ms  | ~8.35 MiB/s   |
| `render_terminal_text_ansi_heavy/feed_data_ansi_heavy/24_lines`    | ~281 µs   | ~20.2 MiB/s   |
| `render_terminal_text_ansi_heavy/feed_data_ansi_heavy/240_lines`   | ~2.43 ms  | ~23.5 MiB/s   |
| `render_terminal_text_bursty/feed_data_bursty_5_rounds`            | ~1.46 ms  | ~13.9 MiB/s   |
| `render_terminal_text_snapshot/build_snapshot_after_ansi_feed`     | ~47.3 µs  | ~40.6 Melem/s |

**Post-refactor results (to be filled in after Task 12):**

_Not yet recorded._

### 8.3 `freminal-buffer` — Augment `buffer_row_bench.rs`

**Keep:** `bench_insert_chunks`, `bench_softwrap_heavy` — these are sound.

**Fix:** `bench_insert_large_line` and `bench_resize` — generate test data inline instead of
reading from a file. Keep the file-based variant behind `#[cfg(feature = "bench_fixtures")]`.

**Add:**

| Benchmark                         | What It Measures                                                        | Workload Design                                                |
| --------------------------------- | ----------------------------------------------------------------------- | -------------------------------------------------------------- |
| `bench_visible_flatten`           | `Buffer::visible_as_tchars_and_tags()` on a full 200×50 visible window  | Pre-populate buffer outside `b.iter()`; measure flatten only   |
| `bench_scrollback_flatten`        | `scrollback_as_tchars_and_tags()` on a buffer with 1000 scrollback rows | Pre-populate outside iter; measure flatten only                |
| `bench_insert_with_color_changes` | Insert with a new `FormatTag` every N characters                        | Exercises tag fragmentation — real-world colored output        |
| `bench_insert_ansi_heavy`         | Full parse + insert of a realistic ANSI-dense payload                   | Inline payload similar to `ls --color` or `git diff` output    |
| `bench_cursor_ops`                | CUP + data interleaved in a loop                                        | Simulates a TUI application doing a full screen redraw         |
| `bench_lf_heavy`                  | Repeated LF until scrollback limit is hit                               | Exercises `enforce_scrollback_limit` and `handle_lf` fast path |
| `bench_erase_display`             | ED (erase display) on a full buffer                                     | Common TUI operation; touches every visible row                |

**Design rules for all buffer benchmarks:**

- Insert benchmarks: create a fresh `Buffer` inside `b.iter()` (or use `iter_batched`) so each
  sample starts clean.
- Flatten benchmarks: prepare the buffer outside `b.iter()`; only the target method is timed.
- Use `criterion::black_box()` on all return values.
- Use `Throughput::Elements(n)` or `Throughput::Bytes(n)` wherever meaningful.

### 8.4 `freminal-terminal-emulator` — Replace stub `buffer_benches.rs`

Replace the no-op entirely.

| Benchmark                       | What It Measures                                                                        |
| ------------------------------- | --------------------------------------------------------------------------------------- |
| `bench_parse_plain_text`        | `FreminalAnsiParser::push()` on plain ASCII bytes, no escape sequences                  |
| `bench_parse_sgr_heavy`         | Parser + handler on a payload dense with SGR escapes (color changes every few chars)    |
| `bench_parse_cup_writes`        | Parser + handler on CUP + data interleaved — the TUI screen-draw pattern                |
| `bench_parse_bursty`            | Bursty PTY output: 10 chunks < 100 bytes followed by one 4096-byte chunk, repeated 100× |
| `bench_handle_incoming_data`    | Full `handle_incoming_data()` including UTF-8 reassembly overhead                       |
| `bench_build_snapshot`          | `build_snapshot()` on a pre-populated emulator — measures the snapshot production cost  |
| `bench_data_and_format_for_gui` | `data_and_format_data_for_gui()` on a pre-populated handler, in isolation               |

**Bursty workload design:** Build the `Vec<Vec<u8>>` of payloads outside `b.iter()`. Use
`iter_batched` with `BatchSize::SmallInput` so a fresh `TerminalState` is constructed per outer
iteration — state does not drift across samples.

### 8.5 `freminal` — Rewrite `render_loop_bench.rs`

After Task 9, `show()` takes `&TerminalSnapshot` directly. The render benchmarks become clean and
do not need a `TerminalEmulator` at all.

| Benchmark                         | What It Measures                                                                                        |
| --------------------------------- | ------------------------------------------------------------------------------------------------------- |
| `bench_snapshot_render_plain`     | Full `show()` render from a snapshot containing plain ASCII, 200×50                                     |
| `bench_snapshot_render_colored`   | Full `show()` render from a snapshot with dense SGR formatting                                          |
| `bench_snapshot_render_no_change` | Full `show()` call when the snapshot is identical to the previous frame (the cache path)                |
| `bench_flatten_and_convert`       | `create_terminal_output_layout_job()` in isolation from a pre-built snapshot                            |
| `bench_feed_data_incremental`     | `handle_incoming_data()` in small chunks. Fresh `TerminalState` per outer iteration via `iter_batched`. |
| `bench_arcswap_roundtrip`         | `ArcSwap::store` + `ArcSwap::load` under concurrent load — verifies the atomic swap stays fast          |

**Removed:** `logic_only` — it measured `needs_redraw()` at 177 ps and told us nothing.

---

## 9. Remaining Lower-Level Optimisations

These are independent of the architecture refactor and should be done after Task 12 is complete.
Each is a self-contained change that does not affect the architecture.

### 9.1 — UTF-8 Leftover Detection (Medium priority)

**Location:** `freminal-terminal-emulator/src/state/internal.rs` — `handle_incoming_data()`

The `while let Err(_) = String::from_utf8(incoming.clone())` loop clones the entire incoming
buffer (up to 4096 bytes) to detect an incomplete UTF-8 sequence at the tail. A UTF-8 sequence is
at most 4 bytes. Scanning only the last 3 bytes is sufficient to detect any incomplete sequence
without cloning the full buffer.

### 9.2 — `remaining.to_vec()` in `Buffer::insert_text` (Medium priority)

**Location:** `freminal-buffer/src/buffer.rs` — `Buffer::insert_text()`

`let mut remaining = text.to_vec()` unconditionally clones the input on every call. Replace with
an index cursor into the original slice. The `Leftover` variant of `InsertResponse` would return
a start index rather than an owned `Vec<TChar>`.

### 9.3 — Font Metric Queries Inside Character Loop (Medium priority)

**Location:** `freminal/src/gui/terminal.rs` — `render_terminal_text()`

`ui.ctx().fonts_mut()` is called twice per character inside the render loop to get `glyph_width`
and `row_height`. Both values are constant for a monospace font for the duration of a frame. Hoist
both calls above the loop. Eliminates ~20,000 `Mutex` acquisitions per frame at 200×50.

### 9.4 — `apply_dec_special` Allocates on Every `handle_data` Call (Low priority)

**Location:** `freminal-buffer/src/terminal_handler.rs` — `handle_data()`

`apply_dec_special` performs a `.iter().map().collect()` into a new `Vec<u8>` on every invocation,
even when `character_replace == DecSpecialGraphics::Inactive` (the overwhelmingly common case). Add
a fast path that returns the input slice directly when the mode is inactive.

### 9.5 — Dirty-Row Tracking in Buffer (High effort, deferred)

Add a `dirty: bool` flag to each `Row`, cleared when the row is snapshotted and set on any
mutation. `visible_as_tchars_and_tags` (called by `build_snapshot`) can then skip clean rows and
reuse a cached flat representation, making the snapshot cost O(changed rows) instead of O(all
visible rows).

Defer until the simpler wins in 9.1–9.4 are implemented and profiled, and the architecture
refactor is stable.

---

## 10. Open Questions

1. **Snapshot granularity.** Should `build_snapshot` be called after every `handle_incoming_data`
   invocation, or should it be batched? Under very heavy PTY output the PTY thread might produce
   snapshots faster than the GUI can consume them. `ArcSwap::store` will simply overwrite any
   unconsumed snapshot — this is correct (the GUI always gets the latest state) but means some
   intermediate states are never rendered. This is acceptable and desirable behaviour. No
   back-pressure mechanism is needed.

2. **Scrollback rendering.** Scrollback display is currently disabled. When it is re-enabled, the
   `TerminalSnapshot::rows: Arc<Vec<Row>>` field provides the raw data. The GUI applies
   `ViewState::scroll_offset` to select the display window. This design already supports scrollback
   without further changes to the architecture — it just requires the render path in
   `render_terminal_output` to use the rows directly rather than the pre-flattened visible data.

3. **`render_terminal_text` long-term.** The v0.3.0 roadmap plans a move to raw OpenGL/shaders.
   The per-character `painter.text()` approach is a known temporary solution. Optimisation 9.3 is
   worth doing regardless (low effort, real gain). Larger structural changes to the render path
   should be scoped as part of v0.3.0 work, not here.

4. **`crossbeam::select!` vs `tokio`.** The PTY processing thread currently uses blocking
   `recv()`. The new design adds a second channel (`input_rx`) requiring `select!`. Using
   `crossbeam::select!` keeps the dependency surface small and avoids introducing an async runtime.
   This is the right call unless the project moves to async elsewhere.

5. **Benchmark validity without a GPU.** The headless egui context in `render_loop_bench` does not
   perform GPU tessellation. The benchmarks measure CPU cost only. This is still useful for
   tracking allocation and layout regressions. Real-world GPU performance requires profiling under
   a running instance with `perf` or `cargo-flamegraph`.

---

## 11. Overall Progress

- [ ] Document reviewed and agreed by user
- [x] Task 1 complete
- [x] Task 2 complete
- [x] Task 3 complete (benchmarks written and baselined)
- [ ] Task 4 complete
- [ ] Task 5 complete
- [ ] Task 6 complete
- [ ] Task 7 complete
- [ ] Task 8 complete (`FairMutex` eliminated)
- [ ] Task 9 complete
- [ ] Task 10 complete
- [ ] Task 11 complete (dead code deleted, clippy clean)
- [ ] Task 12 complete (benchmarks re-baselined, results recorded)
- [ ] Phase 3 (Section 9 optimisations) planned
