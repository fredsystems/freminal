---
name: freminal-architecture
description: Use ONLY when working in the freminal repository (the Rust terminal emulator at ~/GitHub/freminal with crates freminal, freminal-terminal-emulator, freminal-buffer, freminal-common, freminal-windowing, xtask). Triggers on architecture-affecting changes: anything touching the GUI/PTY split, the ArcSwap snapshot transport, the channel-based input system, crate dependency boundaries, or `TerminalEmulator` / `TerminalSnapshot` / `ViewState`. Codifies the post-refactor lock-free architecture invariants.
---

# Freminal: lock-free architecture invariants

Freminal underwent a major refactor (documented in
`Documents/PERFORMANCE_PLAN.md` Sections 4-6) to eliminate the
`FairMutex` between the GUI thread and the PTY-processing thread.
The post-refactor architecture has hard invariants that **must not
regress**, even accidentally during an unrelated change.

## The model

```text
PTY Processing Thread (owns TerminalEmulator exclusively)
  -> Receives PtyRead from OS PTY reader thread
  -> Receives InputEvent from GUI (keyboard, resize, focus) via crossbeam channel
  -> After each batch: publishes Arc<TerminalSnapshot> via ArcSwap
  -> Sends WindowCommand to GUI for Report* / Viewport handling

GUI Thread (eframe update() -- pure render, NO mutation)
  -> Loads TerminalSnapshot from ArcSwap (atomic, lock-free)
  -> Sends InputEvent through crossbeam channel
  -> Sends PtyWrite directly for Report* responses
  -> Owns ViewState (scroll offset, mouse, focus) -- NEVER shared
```

## Hard invariants

1. **`TerminalEmulator` is owned exclusively by the PTY thread.** The
   GUI must never hold a reference to it, never lock it, never
   inspect it.
2. **No shared mutable state between PTY and GUI threads at steady
   state.** The only cross-thread channels are:
   - `ArcSwap<TerminalSnapshot>` (PTY -> GUI, read-only on the GUI
     side)
   - `Sender<InputEvent>` (GUI -> PTY)
   - `Sender<PtyWrite>` (GUI -> OS PTY directly, for Report\*)
   - `Sender<WindowCommand>` (PTY -> GUI)
3. **The GUI `update()` function is a pure read.** No terminal-state
   mutation may happen there. If a render-time observation needs to
   trigger a state change, it goes through `InputEvent`.
4. **`ViewState` (scroll offset, mouse position, focus) is owned
   entirely by the GUI** and is never sent to or read by the PTY
   thread.
5. **Crate dependency boundaries are one-directional.** The graph is:

   ```text
   freminal (binary) -> freminal-terminal-emulator -> freminal-buffer -> freminal-common
   ```

   plus every crate may depend on `freminal-common`. Upward
   dependencies are forbidden -- `freminal-buffer` may never import
   from `freminal-terminal-emulator`, etc.

## Crate responsibilities (no leaks)

- **`freminal-common`**: shared types and utilities only. No business
  logic, no terminal semantics, no platform-specific dependencies
  beyond what's needed for type definitions. Changes here affect every
  downstream crate -- think carefully before adding to it.

- **`freminal-buffer`**: pure data model for terminal content. Cells,
  rows, cursor tracking, wrapping, explicit mutation results
  (damage/diffs). Does NOT parse escape sequences, implement terminal
  semantics, render, interact with UI frameworks, or access
  OS/platform APIs. No global state. All state transitions must be
  explicit, localized, observable, and testable. Hidden side effects
  are forbidden.

  Buffer invariants:
  - A `Cell` is the smallest addressable unit and is always valid
    (empty cells are explicit).
  - Rows own cells, have a fixed width; wrapping produces new rows,
    not hidden overflow.
  - Logical vs physical rows must be explicit.
  - Cursor movement does not mutate cells -- mutations happen through
    explicit operations.
  - All mutations return a structured description of what changed.

- **`freminal-terminal-emulator`**: ANSI parser and terminal state
  machine. Owns `TerminalState` and `TerminalHandler` which drive
  buffer mutations. Produces `TerminalSnapshot` for the GUI via
  `build_snapshot()`. Owns `FreminalAnsiParser`. Does NOT render,
  interact with egui, or hold GUI state.

- **`freminal` (binary)**: the GUI application using eframe/egui. The
  render loop is a pure read of `TerminalSnapshot`. All input goes
  through `Sender<InputEvent>`. `ViewState` lives entirely here.

- **`xtask`**: build and CI orchestration. Not production code.
  `anyhow` / `color-eyre` are acceptable here. Subcommands: `ci`,
  `build`, `check`, `lint`, `test`, `coverage`, `deny`, `machete`.

## Conventions tied to architecture

- **Terminal modes**: if a mode has an enum in
  `freminal-common/src/buffer_states/modes/`, that enum is used for
  storage, transport, and function parameters -- never a raw `bool`.
  Applies to `TerminalHandler`, `Buffer`, `FreminalAnsiParser`,
  `SnapshotModeFields`, `TerminalSnapshot`, and function signatures
  like `to_payload()` / `send_terminal_inputs()`. Raw `bool` is OK
  only when no enum exists.

- **Keybindings**: every feature that adds or modifies a keyboard
  shortcut MUST:
  1. Add a `KeyAction` variant in
     `freminal-common/src/keybindings.rs` (with `name()`,
     `display_label()`, `FromStr`, and inclusion in `ALL`).
  2. Add a default binding in `BindingMap::default()` (in the
     `register_*_bindings()` helpers).
  3. Handle the action in `dispatch_binding_action()` in
     `freminal/src/gui/terminal/input.rs` (or higher in `gui/mod.rs`
     for actions needing full GUI state).
  4. Document the default combo in `config_example.toml` under
     `[keybindings]`.

  Hardcoded shortcuts outside the `BindingMap` system are forbidden.
  Every shortcut must be discoverable and configurable.

## When to stop and ask

- A change would re-introduce a shared lock between PTY and GUI
  threads. Stop -- this was explicitly removed and a regression here
  is a correctness bug.
- A `feature/bugfix` requires `freminal-buffer` to know something
  about escape sequences. Stop -- the buffer is pure data; the
  parser owns semantics. Find a way to surface the information
  through `MutationResult` instead of leaking parser knowledge
  downward.
- A new crate dependency would be upward. Stop -- restructure so the
  arrow points down.
- A keybinding needs to be hardcoded "just this once". Don't. Wire
  it through `BindingMap` like everything else.
