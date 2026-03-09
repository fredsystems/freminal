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

- [x] **Task 5 — Move GUI-local fields off `TerminalState`**
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
  - ✅ **Completed 2026-03-09.** Removed `mouse_position`, `window_focused`, `ctx`, `changed`
    from `TerminalState` and their associated methods (`is_changed`, `set_state_changed`,
    `clear_changed`, `set_ctx`, private `request_redraw`). Renamed `set_window_focused` →
    `send_focus_event` to retain the focus-report escape-sequence logic without owning
    `window_focused` state. Removed the `set_state_changed()` + `request_redraw()` tail
    from `handle_incoming_data()` — signalling is now the `TerminalEmulator` wrapper's
    responsibility. Added `changed: bool` field to `TerminalEmulator`; added
    `handle_incoming_data()` wrapper that calls `internal.handle_incoming_data`, sets
    `self.changed = true`, and calls `request_repaint()`. `needs_redraw()` reads
    `self.changed` instead of `self.internal.is_changed()`. Removed `set_mouse_position*`,
    `get_mouse_position`, `set_window_focused` from `TerminalEmulator`; `set_egui_ctx_if_missing`
    no longer forwards ctx to `internal.set_ctx`. Added `view_state: ViewState` field to
    `FreminalGui`; threaded `&mut self.view_state` into `terminal_widget.show()`. Added
    `view_state: &mut ViewState` parameter to `show()` and `write_input_to_terminal()`;
    `PointerGone`, `WindowFocused`, `PointerMoved` events now write directly to `ViewState`
    fields; URL hover check reads `view_state.mouse_position`. Updated `main.rs` to call
    `terminal.lock().handle_incoming_data(...)` through the wrapper. Also added
    `#[allow(dead_code)]` to `gen_line_tchars` in `buffer_row_bench.rs` (unused under
    `--all-features`) and added `arc-swap` to the `cargo-machete` ignore list in
    `freminal-terminal-emulator/Cargo.toml` (wired in Task 8). Committed as 3ee252d.
    `cargo test --all`: 498 tests passed, 0 failed.
    `cargo clippy --all-targets --all-features -- -D warnings`: clean.
    `cargo-machete`: clean.

---

- [x] **Task 6 — Collapse the internal PTY write forwarding thread**
  - In `TerminalState::new()` in `freminal-terminal-emulator/src/state/internal.rs`, remove the
    `bytes_tx` / `bytes_rx` channel pair and the spawned forwarding thread.
  - Update `TerminalHandler::set_write_tx` (in `freminal-buffer/src/terminal_handler.rs`) to
    accept `crossbeam_channel::Sender<PtyWrite>` directly instead of `Sender<Vec<u8>>`.
  - Update `TerminalHandler`'s internal `write_to_pty` method to send `PtyWrite::Write(bytes)`
    directly.
  - Pass the existing `write_tx: Sender<PtyWrite>` from `TerminalState` directly into the handler.
  - **Verify:** `cargo test --all` passes. PTY write-back responses (device attribute queries,
    cursor position reports, etc.) still function correctly end-to-end.
  - ✅ **Completed 2026-03-09.** `PtyWrite` and `FreminalTerminalSize` moved to
    `freminal-common/src/pty_write.rs` (with `portable-pty` added as a workspace dep of
    `freminal-common` so the `TryFrom<FreminalTerminalSize> for PtySize` impl compiles there).
    `freminal-terminal-emulator/src/io/mod.rs` now re-exports both types from `freminal-common`
    and drops its own local definitions and the now-redundant `TryFrom` impl.
    `TerminalHandler::set_write_tx` signature changed from `Sender<Vec<u8>>` to
    `Sender<PtyWrite>`; `write_to_pty` wraps the byte slice in `PtyWrite::Write(...)`.
    `TerminalState::new()` removes the `bytes_tx`/`bytes_rx` unbounded channel pair and the
    spawned forwarding thread; it now calls `h.set_write_tx(write_tx.clone())` directly.
    Two integration tests in `freminal-buffer/tests/terminal_handler_integration.rs` updated to
    create `unbounded::<PtyWrite>()` channels and unwrap the `Write` variant before asserting on
    the response bytes. Committed as 32aed9a.
    `cargo test --all`: 498 passed, 0 failed.
    `cargo clippy --all-targets --all-features -- -D warnings`: clean.
    `cargo-machete`: no unused dependencies.

---

- [x] **Task 7 — Move resize out of the render loop**
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
  - ✅ **Completed 2026-03-09.** `InputEvent::Resize` extended to carry four fields
    `(width_chars, height_chars, font_pixel_width, font_pixel_height)` so the consumer
    thread has everything it needs to build a correct `PtyWrite::Resize` payload without
    touching the GUI. Added `TerminalEmulator::handle_resize_event(w, h, pw, ph)` which
    calls `internal.set_win_size`, sends `PtyWrite::Resize`, clears `previous_pass_valid`,
    and requests a repaint — all the work that was previously done inside the GUI lock by
    `set_win_size`. Added `crossbeam-channel` workspace dep to `freminal/Cargo.toml`.
    In `main.rs`: created an `unbounded::<InputEvent>()` channel pair; passed `input_tx`
    to `gui::run()`; in the consumer thread loop, drained pending `InputEvent`s with
    `try_recv()` before each blocking `rx.recv()` — `Resize` events are dispatched to
    `handle_resize_event()` under the lock; `Key`/`FocusChange` are reserved stubs for
    Task 8. In `gui/mod.rs`: added `input_tx: Sender<InputEvent>` field to `FreminalGui`,
    threaded through `new()` and `run()`; in `update()` computed the new size, compared
    to `view_state.last_sent_size`, sent `InputEvent::Resize` only on change and updated
    `last_sent_size`; removed `lock.set_win_size()` from the lock-held path entirely.
    Committed as e366739.
    `cargo test --all`: 498 passed, 0 failed.
    `cargo clippy --all-targets --all-features -- -D warnings`: clean.
    `cargo-machete`: no unused dependencies.

---

- [x] **Task 8 — Move the PTY consumer thread off the `FairMutex` (central step)**

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
  - ✅ **Completed 2026-03-09.** `Arc<FairMutex<TerminalEmulator>>` fully eliminated.
    `TerminalEmulator` is now owned exclusively by the PTY consumer thread; no lock is
    taken on the GUI render path.

    Key changes:
    - `main.rs`: created `Arc<ArcSwap<TerminalSnapshot>>` (shared via `arc_swap_gui`);
      cloned `pty_write_tx` from emulator via `clone_write_tx()` before moving emulator
      into the thread; created `(window_cmd_tx, window_cmd_rx)` pair; replaced the
      previous `try_recv` + blocking `recv` loop with `crossbeam::select!` over
      `pty_read_rx` and `input_rx`; after each event drains `emulator.internal.window_commands`,
      classifies each as `WindowCommand::Viewport` or `WindowCommand::Report`, sends via
      `window_cmd_tx`; then calls `emulator.build_snapshot()` and stores via `arc_swap.store()`.
    - `interface.rs`: added `TerminalEmulator::clone_write_tx()` to expose the write
      channel before the emulator is moved; added `write_raw_bytes(&[u8])` to forward
      `InputEvent::Key` bytes as `PtyWrite::Write` without re-encoding through
      `TerminalInput`; updated `build_snapshot()` to include `bracketed_paste`,
      `mouse_tracking`, `repeat_keys`, `cursor_key_app_mode`, and `skip_draw` fields
      so the GUI can handle input and rendering entirely from the snapshot.
    - `snapshot.rs`: added `bracketed_paste: RlBracket`, `mouse_tracking: MouseTrack`,
      `repeat_keys: bool`, `cursor_key_app_mode: bool`, `skip_draw: bool` to
      `TerminalSnapshot`; added `TerminalSnapshot::empty()` as the initial value for
      the `ArcSwap` before the PTY thread has produced real data.
    - `gui/mod.rs`: `FreminalGui` now holds `Arc<ArcSwap<TerminalSnapshot>>`,
      `Sender<InputEvent>`, `Sender<PtyWrite>`, and `Receiver<WindowCommand>` — no
      `FairMutex` anywhere; `update()` loads snapshot with `arc_swap.load()` (single
      atomic pointer load, no blocking); `handle_window_manipulation` rewritten to drain
      from `window_cmd_rx` via non-blocking `try_recv()`; `Report*` variants now build
      escape strings inline and send via `pty_write_tx` rather than calling
      `terminal_emulator.internal.report_*()`.
    - `gui/terminal.rs`: `show()` signature changed to accept `&TerminalSnapshot`,
      `&Sender<InputEvent>`, and `&Sender<PtyWrite>` — generic `Io` type parameter
      removed; `write_input_to_terminal` uses snapshot fields for modes
      (`bracketed_paste`, `mouse_tracking`, `repeat_keys`); keyboard input converted
      via `to_payload(cursor_key_app_mode, cursor_key_app_mode)` and sent as
      `InputEvent::Key(bytes)`; focus events sent as `InputEvent::FocusChange`;
      `render_terminal_output` takes `&TerminalSnapshot` and reads `visible_chars` /
      `visible_tags` directly.
    - `freminal/Cargo.toml`: added `arc-swap.workspace = true`; removed `parking_lot`
      (no longer needed now that `FairMutex` is gone).

    Committed as 6e89e1a.
    `cargo test --all`: 498 passed, 0 failed.
    `cargo clippy --all-targets --all-features -- -D warnings`: clean.
    `cargo-machete`: no unused dependencies.
    Pre-commit hooks (clippy, rustfmt, xtask-check, codespell, etc.): all passed.

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

| Benchmark                                                    | Time     | Throughput    |
| ------------------------------------------------------------ | -------- | ------------- |
| `buffer_insert_large_line/insert_full/500000`                | ~1.32 s  | ~378 Kelem/s  |
| `buffer_insert_chunks/insert_chunks_1000/500`                | ~35.1 ms | ~14.2 Melem/s |
| `buffer_resize/reflow_width/40`                              | ~1.35 s  | —             |
| `buffer_resize/shrink_height/20`                             | ~1.33 s  | —             |
| `softwrap_heavy/wrap_long_line_to_width_10`                  | ~361 µs  | —             |
| `bench_visible_flatten/visible_200x50`                       | ~30.5 µs | ~327 Melem/s  |
| `bench_scrollback_flatten/scrollback_1024_rows`              | ~289 µs  | ~283 Melem/s  |
| `bench_insert_with_color_changes/color_change_every_8_chars` | ~141 µs  | ~28.4 Melem/s |
| `bench_cursor_ops/cup_then_data_24x80`                       | ~61.0 µs | ~31.5 Melem/s |
| `bench_lf_heavy/lf_4100_times`                               | ~3.89 ms | ~1.05 Melem/s |
| `bench_erase_display/erase_to_end_of_display_80x24`          | ~22.8 µs | —             |

**`freminal-terminal-emulator` (`buffer_benches.rs`):**

| Benchmark                                              | Time     | Throughput   |
| ------------------------------------------------------ | -------- | ------------ |
| `bench_parse_plain_text/parser_push/4096`              | ~9.74 µs | ~401 MiB/s   |
| `bench_parse_sgr_heavy/parser_push_sgr/4097`           | ~98.7 µs | ~39.6 MiB/s  |
| `bench_parse_cup_writes/parse_and_handle_80x24`        | ~118 µs  | ~16.8 MiB/s  |
| `bench_parse_bursty/bursty_10_small_plus_1_large`      | ~278 µs  | ~14.2 MiB/s  |
| `bench_handle_incoming_data/handle_incoming_data_4096` | ~276 µs  | ~14.2 MiB/s  |
| `bench_data_and_format_for_gui/flatten_80x24`          | ~5.81 µs | ~330 Melem/s |
| `bench_build_snapshot/build_snapshot_80x24`            | ~16.1 µs | ~119 Melem/s |

**`freminal` (`render_loop_bench.rs`):**

| Benchmark                                                        | Time     | Throughput    |
| ---------------------------------------------------------------- | -------- | ------------- |
| `render_terminal_text/feed_data_incremental/100_lines`           | ~383 µs  | ~12.9 MiB/s   |
| `render_terminal_text/feed_data_incremental/1000_lines`          | ~5.94 ms | ~8.35 MiB/s   |
| `render_terminal_text_ansi_heavy/feed_data_ansi_heavy/24_lines`  | ~281 µs  | ~20.2 MiB/s   |
| `render_terminal_text_ansi_heavy/feed_data_ansi_heavy/240_lines` | ~2.43 ms | ~23.5 MiB/s   |
| `render_terminal_text_bursty/feed_data_bursty_5_rounds`          | ~1.46 ms | ~13.9 MiB/s   |
| `render_terminal_text_snapshot/build_snapshot_after_ansi_feed`   | ~47.3 µs | ~40.6 Melem/s |

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

### 9.1 — UTF-8 Leftover Detection (Medium priority) ✅

**Location:** `freminal-terminal-emulator/src/state/internal.rs` — `handle_incoming_data()`

The `while let Err(_) = String::from_utf8(incoming.clone())` loop clones the entire incoming
buffer (up to 4096 bytes) to detect an incomplete UTF-8 sequence at the tail. A UTF-8 sequence is
at most 4 bytes. Scanning only the last 3 bytes is sufficient to detect any incomplete sequence
without cloning the full buffer.

✅ **Completed.** Replaced the clone-heavy `while let Err` loop with an O(1) tail scan over at
most the last 3 bytes. The algorithm walks backwards looking for a leading byte whose declared
sequence length extends past the end of the buffer. When an incomplete sequence is found,
`Vec::split_off` is used to move just those bytes into `self.leftover_data` — no full-buffer
clone occurs in the common case (pure ASCII or complete sequences). The existing semantics are
fully preserved: split sequences are reassembled correctly on the next call.

### 9.2 — `remaining.to_vec()` in `Buffer::insert_text` (Medium priority) ✅

**Location:** `freminal-buffer/src/buffer.rs` — `Buffer::insert_text()`

`let mut remaining = text.to_vec()` unconditionally clones the input on every call. Replace with
an index cursor into the original slice. The `Leftover` variant of `InsertResponse` would return
a start index rather than an owned `Vec<TChar>`.

✅ **Completed.** `InsertResponse::Leftover` was changed from `{ data: Vec<TChar>, final_col }`
to `{ leftover_start: usize, final_col }`. `Row::insert_text` now returns the index into its
`text` input at which the un-inserted portion begins — no `to_vec()` anywhere. `Buffer::insert_text`
replaced `let mut remaining = text.to_vec()` with `let mut start: usize = 0` and advances the
cursor on each `Leftover` return (`start += leftover_start`), passing `&text[start..]` to the
next row. All tests in `row_tests.rs` that matched on the old `data` field were updated to assert
`&text[leftover_start..]` instead.

### 9.3 — Font Metric Queries Inside Character Loop (Medium priority) ✅

**Location:** `freminal/src/gui/terminal.rs` — `render_terminal_text()`

`ui.ctx().fonts_mut()` is called twice per character inside the render loop to get `glyph_width`
and `row_height`. Both values are constant for a monospace font for the duration of a frame. Hoist
both calls above the loop. Eliminates ~20,000 `Mutex` acquisitions per frame at 200×50.

✅ **Completed.** Added a `section_cell_width` variable hoisted above the inner character loop,
computed once per section via `ui.ctx().fonts_mut(|f| f.glyph_width(&font_id, 'W'))`. The
per-character `fonts_mut` call for `natural_width` was replaced with `section_cell_width`. The
`glyph_width` and `row_height` calls that were already above the outer section loop (used for
layout sizing) remain; only the redundant per-character call inside the inner loop was eliminated.

### 9.4 — `apply_dec_special` Allocates on Every `handle_data` Call (Low priority) ✅

**Location:** `freminal-buffer/src/terminal_handler.rs` — `handle_data()`

`apply_dec_special` performs a `.iter().map().collect()` into a new `Vec<u8>` on every invocation,
even when `character_replace == DecSpecialGraphics::Inactive` (the overwhelmingly common case). Add
a fast path that returns the input slice directly when the mode is inactive.

✅ **Completed.** Changed `apply_dec_special` return type from `Vec<u8>` to `Cow<'a, [u8]>`.
The `DontReplace` arm now returns `Cow::Borrowed(data)` — zero allocation in the overwhelmingly
common case. The `Replace` arm returns `Cow::Owned(out)` as before. `handle_data` updated to
hold `Cow<[u8]>` and pass a deref to `TChar::from_vec`. Added `use std::borrow::Cow` to the
import block.

### 9.5 — Dirty-Row Tracking in Buffer (High effort — split into three sub-tasks)

**Goal:** Make `build_snapshot` cost O(changed rows) instead of O(all visible rows) by adding a
`dirty` flag to each `Row` and maintaining a per-row flat-representation cache in `Buffer`.

This is too large for a single agent session. It has been broken into three sequential sub-tasks
(9.5-A, 9.5-B, 9.5-C). Each sub-task must leave `cargo test --all` passing before the next
begins. A broken compilation state between sessions is acceptable if noted explicitly.

---

#### 9.5-A — Add `dirty: bool` to `Row` and instrument all mutation sites ✅

**Files touched:** `freminal-buffer/src/row.rs`, `freminal-buffer/src/buffer.rs`

**What to do:**

1. Add `dirty: bool` to `Row`:

   ```rust
   pub struct Row {
       cells: Vec<Cell>,
       width: usize,
       pub origin: RowOrigin,
       pub join: RowJoin,
       pub dirty: bool,   // ← new field
   }
   ```

2. All constructors (`new`, `new_with_origin`, `from_cells`) must set `dirty: true`.
   Newly created rows are always dirty — they have never been snapshotted.

3. Every mutating method on `Row` must set `self.dirty = true` at entry (or on any
   code path that actually changes cells). The full list as of this writing:
   - `clear()`
   - `clear_from()`
   - `clear_to()`
   - `clear_with_tag()`
   - `insert_text()` — set dirty only when `InsertResponse::Consumed` is returned
     (i.e. at least one cell was written). If `Leftover { leftover_start: 0 }` is
     returned immediately (col >= width, nothing written), do NOT set dirty.
   - `insert_spaces_at()`
   - `erase_cells_at()`
   - `delete_cells_at()`
   - `cleanup_wide_overwrite()` (private — sets dirty because it blanks cells)

4. In `Buffer`, every site that directly replaces a row by assignment also produces a
   dirty row. The new `Row::new*` constructors already set `dirty: true`, so the
   following sites are handled automatically as long as constructors are correct:
   - `push_row` — calls `Row::new_with_origin`
   - `scroll_slice_up` — assigns `self.rows[last] = Row::new(self.width)`
   - `scroll_slice_down` — assigns `self.rows[first] = Row::new(self.width)`
   - `scroll_up` — `self.rows.push(Row::new(self.width))`
   - `handle_lf` — `self.rows.push(Row::new_with_origin(...))`
   - `resize_height` — `self.rows.push(Row::new(self.width))`

   Additionally, `scroll_slice_up` and `scroll_slice_down` copy rows by cloning:

   ```rust
   let next = self.rows[row_idx + 1].clone();
   self.rows[row_idx] = next;
   ```

   The clone preserves the source row's `dirty` flag — that is **correct** (if the
   source was dirty the copy is also dirty; if clean the copy inherits the clean
   state and will be re-snapshotted if it moves into the visible window).

5. `reflow_to_width` tears down `self.rows` and rebuilds it from scratch via
   `Row::from_cells`. Since all new rows from `from_cells` set `dirty: true` by
   construction, no extra work is needed here.

6. `enforce_scrollback_limit` calls `self.rows.drain(0..overflow)`. This removes
   rows from the front, shifting all row indices. The dirty flags of surviving rows
   are unaffected and remain correct (a previously-clean row is still clean after the
   drain).

7. Add a `pub fn mark_clean(&mut self)` method to `Row`:

   ```rust
   pub fn mark_clean(&mut self) {
       self.dirty = false;
   }
   ```

   This will be called by the snapshot machinery in 9.5-B.

**Verification:** `cargo test --all` passes. No behaviour change — dirty is set but
never read yet. Clippy clean.

**Hint for the next session (9.5-B):** The `dirty` field needs to be ignored in all
`PartialEq` / `Debug` derives that are used in tests. Since `Row` derives `Clone`
but not `PartialEq`, there is no immediate issue. `Cell` and `Row` test helpers in
`row_tests.rs` compare cell content directly; they do not go through `Row::eq`.
No changes to tests should be needed beyond updating the struct literal constructors
in `from_cells` calls if any tests construct `Row` directly via struct syntax (check
with `grep -r 'Row {' freminal-buffer`).

---

#### 9.5-B — Per-row flat-representation cache in `Buffer` ✅

**Files touched:** `freminal-buffer/src/buffer.rs`, `freminal-buffer/src/terminal_handler.rs`

**Depends on:** 9.5-A complete and passing.

**What to do:**

Add a `row_cache: Vec<Option<(Vec<TChar>, Vec<FormatTag>)>>` field to `Buffer`.
`None` means the row is dirty (not yet cached); `Some(...)` holds the last-good flat
representation for that row.

```rust
pub struct Buffer {
    // ... existing fields ...

    /// Per-row flat-representation cache.  Index matches `self.rows`.
    /// `None` = dirty (must be re-flattened on next snapshot).
    /// `Some((chars, tags))` = clean (can be reused directly).
    row_cache: Vec<Option<(Vec<TChar>, Vec<FormatTag>)>>,
}
```

Rules for keeping `row_cache` consistent with `self.rows`:

- **`push_row` / any `self.rows.push`:** also push `None` to `row_cache`.
- **`self.rows.drain(0..n)`:** also `row_cache.drain(0..n)`.
- **`scroll_slice_up(first, last)`:** rotate the cache entries the same way rows are
  rotated, then set `row_cache[last] = None` (the new blank row is dirty).
- **`scroll_slice_down(first, last)`:** same in reverse; set `row_cache[first] = None`.
- **`scroll_up`:** `row_cache.remove(0)` then `row_cache.push(None)`.
- **`reflow_to_width`:** `row_cache = vec![None; self.rows.len()]` after the new rows
  are installed (all rows are dirty post-reflow by construction, so this is correct).
- **`resize_height` (grow):** push `None` for each new row added.
- **`resize_height` (shrink):** truncate `row_cache` to match.
- **`enter_alternate` / `leave_alternate`:** save and restore the cache alongside the
  rows, exactly as `SavedPrimaryState` saves cursor and scroll offset. Add a
  `row_cache: Vec<Option<...>>` field to `SavedPrimaryState`.

Change `rows_as_tchars_and_tags` to accept `&mut [Row]` and `&mut Vec<Option<...>>`
so it can populate the cache for dirty rows and mark them clean:

```rust
fn rows_as_tchars_and_tags_cached(
    rows: &mut [Row],
    cache: &mut Vec<Option<(Vec<TChar>, Vec<FormatTag>)>>,
) -> (Vec<TChar>, Vec<FormatTag>) {
    // For each row:
    //   if row.dirty || cache[i].is_none():
    //     flatten the row → (row_chars, row_tags)
    //     cache[i] = Some((row_chars, row_tags))
    //     row.mark_clean()
    //   else:
    //     use cache[i].as_ref().unwrap()
    // Then merge all per-row results with NewLine separators (same tag-merge
    // logic as the current rows_as_tchars_and_tags).
}
```

Keep the existing `rows_as_tchars_and_tags` signature as a thin wrapper that calls
the new function, so external call sites (benchmarks etc.) are not broken.

Update `visible_as_tchars_and_tags` and `scrollback_as_tchars_and_tags` to pass
`&mut self.row_cache` slices. Because these methods now mutate the cache they will
need `&mut self` receivers (they are already called from `&mut self` contexts, so
this is fine).

**Invariant to enforce in `debug_assert_invariants`:**

```rust
assert_eq!(self.rows.len(), self.row_cache.len(),
    "row_cache length {} != rows length {}", self.row_cache.len(), self.rows.len());
```

**Verification:** `cargo test --all` passes. The cache is populated but
`build_snapshot` still calls the full flatten path — that is intentional; 9.5-C
wires it up. Clippy clean.

**Hint:** The tag-merge step in `rows_as_tchars_and_tags` merges _across_ row
boundaries (a tag that ends at exactly the start of a NewLine separator is extended
rather than split). When using the cache, individual row tag-vectors have their
`start`/`end` offsets relative to that row's character slice, not the global flat
vector. The cache should store **raw per-row** tag offsets (starting at 0 for each
row); the merge step re-computes global offsets each time. This is simpler and
correct: the cache saves the cell-iteration work, not the merge work. The merge is
O(visible rows), which is fast (typically 24–50 iterations with no allocation if
every row is clean).

---

#### 9.5-C — Wire `build_snapshot` to honour the cache; fix `content_changed`

**Files touched:** `freminal-terminal-emulator/src/interface.rs`,
`freminal-terminal-emulator/src/snapshot.rs`

**Depends on:** 9.5-B complete and passing.

**What to do:**

1. Add a `previous_visible_snap: Option<(Vec<TChar>, Vec<FormatTag>)>` field to
   `TerminalEmulator` (or to `TerminalState`). This holds the last published
   visible flat representation so `build_snapshot` can return it unchanged when no
   visible row is dirty.

   ```rust
   pub struct TerminalEmulator<Io: FreminalTermInputOutput> {
       pub internal: TerminalState,
       pub changed: bool,
       ctx: Option<egui::Context>,
       previous_visible_snap: Option<(Vec<TChar>, Vec<FormatTag>)>,
   }
   ```

2. In `build_snapshot`, check whether any visible row is dirty before flattening:

   ```rust
   pub fn build_snapshot(&mut self) -> TerminalSnapshot {
       let (term_width, term_height) = self.internal.handler.get_win_size();
       // ... other cheap reads ...

       let any_visible_dirty = {
           let vis_start = self.internal.handler.buffer().visible_window_start(0);
           let vis_end = (vis_start + term_height)
               .min(self.internal.handler.buffer().get_rows().len());
           self.internal.handler.buffer().get_rows()[vis_start..vis_end]
               .iter()
               .any(|r| r.dirty)
       };

       let (visible_chars, visible_tags, content_changed) = if any_visible_dirty {
           let (chars, tags) = self.internal.data_and_format_data_for_gui();  // uses cache
           let changed = self.previous_visible_snap.as_ref()
               .map_or(true, |(pc, _)| pc != &chars.visible);
           self.previous_visible_snap = Some((chars.visible.clone(), tags.visible.clone()));
           (chars.visible, tags.visible, changed)
       } else if let Some((chars, tags)) = &self.previous_visible_snap {
           (chars.clone(), tags.clone(), false)  // nothing changed
       } else {
           // First call ever; no previous snap.
           let (chars, tags) = self.internal.data_and_format_data_for_gui();
           self.previous_visible_snap = Some((chars.visible.clone(), tags.visible.clone()));
           (chars.visible, tags.visible, true)
       };

       TerminalSnapshot {
           visible_chars,
           visible_tags,
           content_changed,   // ← now correctly false when nothing changed
           // ... rest unchanged ...
       }
   }
   ```

   **Important:** `visible_window_start` is currently `pub(crate)` in `buffer.rs`.
   You will need to make it `pub` or add a dedicated `pub fn any_visible_dirty(&self,
scroll_offset: usize) -> bool` method to `Buffer` and expose it through
   `TerminalHandler`. The latter is cleaner and avoids leaking the index arithmetic.

3. The `content_changed` flag in `TerminalSnapshot` can now be `false`. The GUI
   currently does not act on `content_changed` (it renders every frame regardless),
   but it should eventually use it to skip `render_terminal_output` on unchanged
   frames. Wiring that optimisation is out of scope here — just ensure the flag
   value is correct.

4. Remove the `// content_changed: true` hardcoded comment from `interface.rs`.

**Verification:** `cargo test --all` passes. Confirm with a benchmark run that
`bench_build_snapshot` improves vs. the baseline recorded in Section 8.2 (~16 µs
target → ideally < 5 µs for a static screen). Clippy clean. Update Section 8.2
with the new numbers.

**Edge cases to test:**

- Snapshot after a purely-scrollback write (cursor in scrollback, visible unchanged):
  `content_changed` must be `false`.
- Snapshot after cursor movement only (no cell mutation): `content_changed` must be
  `false` because cursor position is carried separately in the snapshot and does not
  affect `visible_chars`.
- Snapshot after `erase_display`: all visible rows are dirty; `content_changed` must
  be `true`.
- Snapshot after `enter_alternate` / `leave_alternate`: the alternate buffer starts
  all-dirty; switching back to primary should restore the primary cache and set
  `content_changed` only if the primary visible rows actually changed while the
  alternate was active (they don't, so it should be `false` for a static primary).

---

**Overall verification for 9.5 (all three sub-tasks):**

- `cargo test --all`: 498+ tests pass, 0 fail.
- `cargo clippy --all-targets --all-features -- -D warnings`: clean.
- `cargo machete`: no unused dependencies.
- `cargo bench --all -- --baseline before_refactor`: update Section 8.2 numbers.
- Pre-commit hooks: all pass.

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
- [x] Task 4 complete
- [x] Task 5 complete
- [x] Task 6 complete
- [x] Task 7 complete
- [x] Task 8 complete (`FairMutex` eliminated)
- [ ] Task 9 complete
- [ ] Task 10 complete
- [ ] Task 11 complete (dead code deleted, clippy clean)
- [ ] Task 12 complete (benchmarks re-baselined, results recorded)
- [ ] Phase 3 — 9.5-A complete (dirty flag on Row, all mutation sites instrumented)
- [ ] Phase 3 — 9.5-B complete (per-row cache in Buffer, cache coherence maintained)
- [ ] Phase 3 — 9.5-C complete (build_snapshot uses cache, content_changed correct)
