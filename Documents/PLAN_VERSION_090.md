# PLAN_VERSION_090.md — v0.9.0 "The Modern Workflow Terminal"

## Goal

Turn freminal from a "very good terminal" into a "modern workflow terminal" by landing the
features that Warp, WezTerm, and Ghostty use to pull ahead: command-aware rendering, visible
command status, ergonomic multi-pane workflows, first-class notifications, and a paste
safety net.

All tasks in this version depend on v0.8.0 being complete — we do not build new features on
top of the correctness debts identified in the post-v0.7.0 audit.

> **Agents read this whole document before executing any subtask.** The "Pre-existing
> Infrastructure" sections below describe what is already done — do not re-implement it.
> The "Subtasks" sections describe what must be added.

---

## Task Summary

| #   | Feature                                 | Scope        | Status  | Depends On      | Branch                           |
| --- | --------------------------------------- | ------------ | ------- | --------------- | -------------------------------- |
| 72  | OSC 133 Command Blocks                  | Large        | Pending | v0.8.0          | `task-72/osc-133-command-blocks` |
| 73  | Command Gutters (exit-status indicator) | Small        | Pending | Task 72         | `task-73/command-gutters`        |
| 74  | Broadcast Input to Panes                | Medium       | Pending | v0.8.0, Task 58 | `task-74/broadcast-input`        |
| 75  | Verify per-pane env round-trip          | Small        | Pending | v0.8.0          | `task-75/pane-env-roundtrip`     |
| 76  | Notification System (OSC 9 / OSC 777)   | Medium       | Pending | v0.8.0, Task 72 | `task-76/notifications`          |
| 77  | Smart Paste Guard                       | Small–Medium | Pending | v0.8.0          | `task-77/paste-guard`            |
| 94  | Tab Title Precedence (prefix default)   | Small        | Pending | v0.8.0 (71.1)   | `task-94/tab-title-precedence`   |
| 95  | Persist Custom Tab Names in Layouts     | Small        | Pending | v0.8.0, Task 61 | `task-95/persist-tab-names`      |

### Execution order

Sequential, one feature branch per task. Recommended ordering:

1. **Task 72** — keystone (CommandBlock storage unlocks 73 and 76)
2. **Task 73** — gutters (depends on 72)
3. **Task 94** — tab title precedence (small, independent, ships a default-behavior change)
4. **Task 95** — persist custom names (small, lands right after 94)
5. **Task 76** — notifications (depends on 72's `CommandFinishedEvent`)
6. **Task 77** — paste guard (independent)
7. **Task 74** — broadcast input (independent)
8. **Task 75** — pane env round-trip verification (smallest, can also run in parallel)

Each task gets its own PR. Each subtask within a task is committed individually per
`agents.md` ("Plan Subtask Commits"). `--no-verify` is forbidden.

---

## Cross-Cutting Design Decisions

These decisions apply across multiple tasks and are fixed at planning time. Do not revisit
in subtasks unless the user explicitly asks.

- **OSC 133 is the anchor.** Tasks 73 and 76 both depend on Task 72's `CommandBlock`
  storage. Task 72 lands first.
- **Shell integration scripts ship with v0.9.0.** Task 72 includes `bash`, `zsh`, `fish`
  helpers and an auto-install on first run.
- **TERM_PROGRAM identification.** v0.9.0 sets `TERM_PROGRAM=freminal` and
  `TERM_PROGRAM_VERSION=<cargo-version>` in the PTY environment so shell scripts can
  detect us. (Subtask of Task 72.)
- **`notify-rust = "4"` is added as a runtime dep.** Pure Rust on Linux/macOS, winapi on
  Windows. Opt-in via `[notifications] enabled = false` default.
- **Tab title default policy is `prefix`.** Format: `"{custom}: {osc}"`. Configurable.
- **Broadcast is per-tab.** All leaves in the active tab when the toggle is on. Mouse
  events are not broadcast.
- **Paste guard default is multi-line only.** Pattern matching is opt-in.
- **No scripting (Lua/WASM) in v0.9.0.** Deferred to v0.10.0.
- **No remote features in v0.9.0.** Deferred to v0.11.0.
- **No fold/collapse persistence across scrollback trim.** Folds are a view-state-only
  concept; when the underlying rows scroll out of the buffer the fold is dropped.

---

## Pre-existing Infrastructure (Do Not Re-Implement)

The post-v0.7.0/v0.8.0 codebase already has more OSC 133 plumbing than the v0.9.0 plan
originally implied. Agents must check the following code paths before adding anything:

| Concern                           | Where it lives today                                                                   |
| --------------------------------- | -------------------------------------------------------------------------------------- |
| OSC 133 parsing                   | `freminal-common/src/buffer_states/ftcs.rs` (`FtcsMarker`, `parse_ftcs_params`)        |
| OSC 133 dispatch                  | `freminal-terminal-emulator/src/terminal_handler/osc.rs:149` (`handle_osc_ftcs`)       |
| FTCS state machine                | `freminal-terminal-emulator/src/terminal_handler/mod.rs:159` (`ftcs_state` field)      |
| `prompt_rows: Vec<usize>` storage | `freminal-buffer/src/buffer/mod.rs:184`, `lifecycle.rs:110` (`mark_prompt_row`)        |
| `last_exit_code: Option<i32>`     | `freminal-terminal-emulator/src/terminal_handler/mod.rs:161`                           |
| Snapshot transport                | `freminal-terminal-emulator/src/snapshot.rs:262,269` (`last_exit_code`, `prompt_rows`) |
| Prev/Next Command keybindings     | `freminal-common/src/keybindings.rs:702-704` (`PrevCommand`, `NextCommand`)            |
| Prev/Next Command scroll          | `freminal/src/gui/search.rs:318,352` (`jump_to_prev_command`, `jump_to_next_command`)  |
| `Tab::custom_name`                | `freminal/src/gui/tabs.rs:80`                                                          |
| Tab display name precedence       | `freminal/src/gui/tabs.rs:104` (`display_name()`)                                      |
| OSC 0/1/2 → clear custom_name     | `freminal/src/gui/app_impl.rs:868` (this is the regression Task 94 fixes)              |
| Inline rename UI                  | `freminal/src/gui/menu.rs:756`, `freminal/src/gui/actions.rs:489`                      |
| Layout schema                     | `freminal-common/src/layout.rs`                                                        |
| `LayoutPane::env`                 | `freminal-common/src/layout.rs:175` (HashMap, already round-trips)                     |
| `LayoutTab::title`                | `freminal-common/src/layout.rs:221` (Option<String>, today only used for authoring)    |
| `extra_env` PTY plumbing          | `freminal/src/gui/pty.rs:95`, `freminal/src/gui/tab_spawning.rs:341`                   |
| OSC 7 CWD tracking                | `freminal-terminal-emulator/src/terminal_handler/shell_integration.rs`                 |
| Toast stack                       | `freminal/src/gui/toast.rs` (`ToastStack::error`, `info`)                              |
| `PaneTree::iter_panes(_mut)`      | `freminal/src/gui/panes/mod.rs:816,828` (returns `Vec<&Pane>` of all leaves)           |
| Bracketed paste handling          | `freminal/src/gui/terminal/input.rs:210,1191`                                          |
| `KeyAction` registry              | `freminal-common/src/keybindings.rs`                                                   |
| Settings tabs                     | `freminal/src/gui/settings.rs` (`SettingsTab` enum + `ALL`)                            |
| Existing config sections          | `freminal-common/src/config.rs` (`Config` with subsections, `SecurityConfig`)          |

If any of the above looks broken or insufficient, **stop and report** — do not silently
extend or rewrite it.

---

## Task 72 — OSC 133 Command Blocks

### 72 Summary

Build full `CommandBlock` storage on top of the existing `prompt_rows` / `last_exit_code`
plumbing. Each block records prompt start, command-input start, output start, end row,
exit code, cwd at command time, and start/end timestamps. Surface blocks through the
snapshot. Add fold/collapse view state, copy-output actions, hover highlight, and a
command-duration overlay. Ship shell integration scripts for bash/zsh/fish.

This task is the keystone of v0.9.0. Tasks 73 (gutters) and 76 (notifications) depend on
its `CommandBlock` snapshot data and `CommandFinishedEvent` channel signal.

### 72 Decisions (fixed)

- **Storage model:** `VecDeque<CommandBlock>` on `Buffer`, ring-bounded by scrollback size,
  trimmed in lockstep with `prompt_rows`.
- **Interaction model:** Full Warp-style — navigation + fold/collapse + per-block
  selection.
- **Shell integration scripts:** Ship `bash`, `zsh`, `fish` in repo at
  `shell-integration/`, auto-install to `~/.config/freminal/shell-integration/` on first
  run.
- **TERM_PROGRAM:** Set `TERM_PROGRAM=freminal` and `TERM_PROGRAM_VERSION=<crate version>`
  in the PTY environment.
- **Fold persistence:** Folds live on `ViewState` (per-pane GUI state), not in the buffer.
  Folds key off `CommandBlock.id`; when a block is trimmed from the ring buffer its fold
  state is dropped silently.

### 72 Architecture

```text
PTY thread (TerminalHandler)
  ├── handle_osc_ftcs receives FtcsMarker (already exists)
  ├── NEW: drives Buffer::start_command_block / end_command_block
  │        based on marker kind
  ├── Buffer maintains VecDeque<CommandBlock> alongside prompt_rows
  ├── on CommandFinished: emit WindowCommand::CommandFinished {
  │        pane_id, exit_code, duration, command, working_dir }
  │        for the GUI thread to consume
  └── build_snapshot() exposes Arc<[CommandBlock]>

GUI thread
  ├── ViewState::folded_blocks: HashSet<BlockId>
  ├── Renderer skips rows belonging to folded blocks
  ├── New KeyActions: ToggleFoldAtCursor, FoldAll, UnfoldAll,
  │        CopyLastCommandOutput, CopySelectedCommandOutput
  ├── New mouse: hover highlight, click-gutter-to-select, click-gutter-to-fold
  └── On WindowCommand::CommandFinished:
        - update internal command-event log (used by Task 76 notifications)
        - update tab badge for unfocused tabs
```

### 72 New Types

In `freminal-common/src/buffer_states/ftcs.rs` (or a new sibling module
`freminal-common/src/buffer_states/command_block.rs` — agent's choice based on
file size after adding):

```rust
/// Stable identifier for a command block, monotonically increasing.
/// Used by GUI ViewState to remember per-block UI state (fold/collapse,
/// selection highlight) across re-renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandBlockId(pub u64);

/// A single shell command's full lifecycle, as derived from OSC 133 A/B/C/D
/// markers.
#[derive(Debug, Clone)]
pub struct CommandBlock {
    pub id: CommandBlockId,
    /// Row of `OSC 133 A` (prompt start).
    pub prompt_start_row: usize,
    /// Row of `OSC 133 B` (end of prompt, start of user input). May equal
    /// prompt_start_row for single-line prompts.
    pub command_start_row: Option<usize>,
    /// Row of `OSC 133 C` (start of output / command executed).
    pub output_start_row: Option<usize>,
    /// Row of `OSC 133 D` (command finished). None while the command is
    /// still running.
    pub end_row: Option<usize>,
    /// Exit code from `OSC 133 D ; <code>`. None if not yet finished or if
    /// the shell omitted the code.
    pub exit_code: Option<i32>,
    /// CWD captured from OSC 7 at the time of prompt start.
    pub cwd: Option<String>,
    /// Wall clock timestamps. Use std::time::SystemTime so they survive
    /// across a recording (FREC v2).
    pub started_at: std::time::SystemTime,
    pub finished_at: Option<std::time::SystemTime>,
}

impl CommandBlock {
    /// Status: Running, Success, Failure, Unknown.
    pub fn status(&self) -> CommandStatus { /* ... */ }
    /// Duration if finished.
    pub fn duration(&self) -> Option<std::time::Duration> { /* ... */ }
    /// Row range [start, end] inclusive, considering the running state.
    pub fn row_range(&self) -> (usize, Option<usize>) { /* ... */ }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandStatus { Running, Success, Failure(i32), Unknown }
```

### 72 Subtasks

Commit one per subtask. Each subtask leaves `cargo test --all` passing.

#### 72.1 — Define `CommandBlock` types in `freminal-common` ✅ 2026-05-17

**Scope:** `freminal-common/src/buffer_states/`.

- Add the types above. Place in a new file `command_block.rs` (do not inflate
  `ftcs.rs`) and re-export from `buffer_states/mod.rs`.
- Implement `status()`, `duration()`, `row_range()` and their unit tests
  (Running, Success, Failure, Unknown each verified).
- Implement `Display` for `CommandBlockId` and `CommandStatus`.
- `CommandBlockId::next()` helper returning a new monotonic id from a
  `std::sync::atomic::AtomicU64` static — agents may also pass a counter
  through `Buffer`; either way the id must be deterministic in tests.

**Verification:** Unit tests for status/duration/row_range. Clippy clean.

**Completion notes (commit `f77fc9d`):**

- Added `freminal-common/src/buffer_states/command_block.rs` with
  `CommandBlockId`, `CommandStatus`, `CommandBlock`, `CommandBlockId::next()`,
  `CommandBlock::new_running()`, `status()`, `duration()`, `row_range()`,
  and `Display` impls for `CommandBlockId` and `CommandStatus`.
- 19 unit tests cover all seven mandatory scenarios plus id uniqueness and
  `cwd = None` preservation.
- `cargo test -p freminal-common`, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo fmt --check`, and `cargo-machete`
  all pass on the full workspace.
- `status()` is `const fn` (clippy `missing_const_for_fn`); `duration()` is
  not const-fn because `SystemTime::duration_since` is non-const.
- No `unwrap()`/`expect()` in production code. Tests use `match` + `panic!`
  rather than `unwrap()` even though tests are permitted to use it.

#### 72.2 — Add `command_blocks: VecDeque<CommandBlock>` to `Buffer`

**Scope:** `freminal-buffer/src/buffer/mod.rs`, `lifecycle.rs`, `scroll.rs`,
`resize_and_alt.rs`.

- Add field `command_blocks: VecDeque<CommandBlock>` to `Buffer`. Cap at
  `scrollback_size` (same cap as `prompt_rows`).
- Add `Buffer::start_command_block(&mut self)` — pushes a new block whose
  `prompt_start_row = cursor.pos.y` and assigns a fresh `CommandBlockId`. Also
  records the current cwd if available (Buffer doesn't know cwd; cwd is filled
  by the handler — see 72.3).
- Add `Buffer::mark_command_start_row(&mut self)` — sets `command_start_row =
Some(cursor.pos.y)` on the most recent open block.
- Add `Buffer::mark_output_start_row(&mut self)` — sets `output_start_row =
Some(cursor.pos.y)` on the most recent open block.
- Add `Buffer::finish_command_block(&mut self, exit_code: Option<i32>)` — sets
  `end_row = Some(cursor.pos.y)`, `exit_code`, and `finished_at = Some(now())`
  on the most recent open block (one whose `end_row` is None).
- Add `Buffer::command_blocks(&self) -> &VecDeque<CommandBlock>` getter.
- Extend `adjust_prompt_rows(removed)` and the scrollback-trim path to also
  trim/adjust `command_blocks`. Blocks whose `prompt_start_row < removed` are
  removed entirely; blocks straddling the boundary have their rows clamped to
  0 (but typically those blocks should also be dropped; agents must verify
  the chosen policy by tests).
- Resize/alt-screen paths must clear or preserve `command_blocks` consistent
  with `prompt_rows` (today resize clears prompt_rows for alt-screen — preserve
  the same policy for command_blocks).

**Verification:**

- Unit tests for each new method (start/mark_command/mark_output/finish in
  sequence; finish without start; double-finish; mark_command before start).
- Trim/scrollback tests: emit 100 blocks, scroll, verify oldest are trimmed.
- Resize tests: alt-screen toggle preserves/clears blocks correctly.
- Benchmark: extend `freminal-buffer/benches/buffer_row_bench.rs` with a
  `bench_command_block_record` measuring 10k start/finish cycles. Record
  before/after numbers in the commit message.

**Completion notes (commit `66522f6`, 2026-05-17):**

- `scroll.rs` was read but not modified. `adjust_prompt_rows` is the
  single trim entry point for `erase_scrollback`, `enforce_scrollback_limit`,
  and the alt-screen `resize_height` path, so extending it covers all three.
- Cap policy uses the existing `Buffer::scrollback_limit` field (default
  4000), not a new constant. One block per prompt is a natural pairing
  with the row scrollback cap.
- Alt-screen entry and exit do NOT clear `command_blocks` — exactly
  matching `prompt_rows`. `SavedPrimaryState` does not carry either field;
  both persist trivially across alt-screen toggles. Shells emit FTCS
  markers only at the primary prompt, so full-screen TUIs on the alternate
  screen do not interact with this storage.
- `command_blocks()` is `const fn` (clippy nursery `missing_const_for_fn`).
  `finish_command_block` is `#[must_use]` so callers cannot accidentally
  drop the returned block needed by 72.3's `WindowCommand::CommandFinished`.
- 13 unit tests added in `lifecycle.rs::command_block_tests`, all
  mandatory scenarios plus interrupted A→A and finished-block row
  shifting.
- `cargo test -p freminal-buffer` — 490 pass.
- `cargo clippy --all-targets --all-features -- -D warnings` (workspace) — clean.
- `cargo-machete` — clean.
- New `bench_command_block_record_10k` measures ~45 ns per
  start+finish pair (≈450 µs for 10,000 cycles). No regression on
  `bench_lf_heavy` once machine variance is accounted for — baseline and
  72.2 both measure in the 5–10 ms band; `adjust_prompt_rows` iterates an
  empty deque in this bench so there is no plausible mechanism for slowdown.

#### 72.3 — Wire FTCS markers into Buffer command-block API

**Scope:** `freminal-terminal-emulator/src/terminal_handler/osc.rs`,
`terminal_handler/mod.rs`.

- In `handle_osc_ftcs`:
  - `PromptStart` (A): call `self.buffer.start_command_block()`. Also fetch
    `self.current_working_directory()` and pass it through (extend the
    `start_command_block` signature to accept `cwd: Option<String>`).
  - `CommandStart` (B): call `self.buffer.mark_command_start_row()`.
  - `OutputStart` (C): call `self.buffer.mark_output_start_row()`.
  - `CommandFinished(exit_code)` (D): call
    `self.buffer.finish_command_block(*exit_code)`. Also update
    `self.last_exit_code` (existing behavior — keep it).
  - `PromptProperty`: still informational, no change to buffer state.
- After `CommandFinished`, the handler queues the finished `CommandBlock`
  on a new `pending_command_events: Vec<CommandBlock>` field. The PTY loop
  drains the queue via `drain_command_events()` after each batch is
  processed (wired in 72.9).

**Architectural note — Path C in effect (decided 2026-05-17):**

The original plan called for adding a `WindowCommand::CommandFinished`
variant. Investigation showed the `WindowCommand` enum
(`freminal-terminal-emulator/src/io/mod.rs`) carries only viewport/report
manipulations today, and the handler does not own a `Sender<WindowCommand>`
— the PTY loop in `freminal/src/gui/pty.rs` wraps handler outputs into
`WindowCommand::Viewport`/`Report`. Adding `CommandFinished` would require
either pulling `WindowCommand` into the handler crate or restructuring the
PTY loop. We chose Path C: queue on the handler, drain from the PTY loop
in 72.9, deliver to the GUI through whatever channel 72.9 deems
appropriate (most likely a new dedicated channel, since `CommandFinished`
is semantically different from viewport ops). The `WindowCommand` enum
remains untouched in 72.3.

**Verification:**

- Extend existing tests in `terminal_handler/shell_integration.rs` to verify
  command_blocks contents after each FTCS sequence.
- A test that emits a full A→B→C→D cycle and verifies one fully populated
  CommandBlock results.
- A test that emits an interrupted A→B (no D) and verifies the block exists
  with `end_row = None` and `status() == Running`.
- A test that drain_command_events returns the finished blocks in FIFO
  order and empties the queue.

**Completion notes (commit `2880d3a`, 2026-05-17):**

- `pending_command_events: Vec<CommandBlock>` added as a sibling to
  `window_commands` on `TerminalHandler`. Same visibility (bare), same
  init/clear pattern.
- `pub fn drain_command_events(&mut self) -> Vec<CommandBlock>` exposed
  using `std::mem::take`. `#[must_use]` to prevent dropped events.
- `mark_prompt_row()` is preserved alongside `start_command_block(cwd)`;
  it still drives the already-shipping PrevCommand/NextCommand navigation.
- cwd is captured at PromptStart time via
  `self.current_working_directory().map(str::to_owned)`.
- 10 new tests in `shell_integration.rs` cover all mandatory scenarios
  including interrupted A→A→D, missing-B fallthrough, and FIFO drain.
- `cargo test --all` workspace-wide passes (no regressions in any crate).
- `cargo clippy --all-targets --all-features -- -D warnings` clean.
- `cargo-machete` clean.
- freminal-terminal-emulator test count: 2310 → 2320 (+10).

#### 72.4 — Expose `command_blocks` through `TerminalSnapshot`

**Scope:** `freminal-terminal-emulator/src/snapshot.rs`,
`freminal-terminal-emulator/src/interface.rs` (the snapshot builder).

- Add `pub command_blocks: Arc<[CommandBlock]>` to `TerminalSnapshot` next to
  `prompt_rows`.
- In `build_snapshot()`, populate from `self.internal.handler.buffer().command_blocks()`.
  Use `Arc::<[CommandBlock]>::from(deque.iter().cloned().collect::<Vec<_>>())`
  or equivalent. Confirm allocation cost is acceptable via the existing
  `bench_build_snapshot` and `bench_build_snapshot_with_scrollback` benchmarks
  (record before/after).
- Update `Default for TerminalSnapshot` to set `command_blocks: Arc::from([])`.

**Verification:**

- Snapshot serialization (if FREC v2 captures snapshots) must continue to work;
  `CommandBlock` does not need to be FREC-encoded yet (FREC v2 already
  captures the OSC 133 byte stream which is the source of truth).
- Benchmark before/after: `bench_build_snapshot_with_scrollback` must not
  regress by more than 15% per the AGENTS.md regression threshold.

**Completion notes (commit `27fa949`, 2026-05-17):**

- The actual constructor in `snapshot.rs` is `TerminalSnapshot::empty()`,
  not `Default`. Default-init line was added to `empty()`. Default impl
  does not exist on this type.
- No caching layer was added. `command_blocks` is rebuilt every frame
  from a `Vec → Arc<[T]>` conversion, mirroring the existing pattern
  used for `prompt_rows`. A previous 72.4 attempt added a
  `previous_command_blocks` cache field on `TerminalEmulator` plus two
  helper methods; that attempt was rejected and reverted as scope creep
  before this commit.
- `build_snapshot()` gained one local `#[allow(clippy::too_many_lines)]`
  because the new local pushed it from 96 to 106 counted lines. This is
  consistent with similar allows in `sgr.rs` and
  `freminal-windowing/src/event_loop.rs` where a function's bulk is a
  flat struct literal or a wide match. Preferred over factoring out a
  helper method.
- 4 new tests in `interface::tests`. Byte-stream-driven tests do NOT
  assert on exit_code because of a pre-existing OSC parser bug filed
  as **72.16** (Cleanup, see below). Handler-direct exit-code coverage
  is in `shell_integration.rs` from 72.3 and is unaffected.
- Benchmarks (15% AGENTS.md budget):
  - `build_snapshot_80x24_dirty`: 31.9 → 31.9 µs (no change)
  - `build_snapshot_80x24_clean`: 97.0 → 114.8 ns (+10.2% — within budget)
  - `snapshot_10k_scrollback_dirty`: 1.34 → 1.28 ms (improved)
  - `snapshot_10k_scrollback_clean`: 95.8 → 116.1 ns (+10.6% — within budget)
- `cargo test -p freminal-terminal-emulator`: passes (test count +4).
- `cargo clippy --all-targets --all-features -- -D warnings`: clean.
- `cargo-machete`: clean.

**Pre-existing bug surfaced (filed as 72.16):** the OSC 133 dispatcher
filter drops numeric exit-code tokens. See subtask 72.16 below for the
full report and fix scope.

#### 72.5 — Settings: `[shell_integration]` and `[command_blocks]` config sections

**Scope:** `freminal-common/src/config.rs`, `config_example.toml`,
`freminal/src/gui/settings.rs` and `settings_dispatch.rs`.

- Add `[shell_integration]` section:

  ```toml
  [shell_integration]
  # When true, freminal sets TERM_PROGRAM=freminal and TERM_PROGRAM_VERSION
  # in the PTY environment. Default true.
  set_term_program = true

  # When true, freminal auto-installs shell integration scripts to
  # ~/.config/freminal/shell-integration/ on first launch.  Default true.
  auto_install = true
  ```

- Add `[command_blocks]` section:

  ```toml
  [command_blocks]
  # Master switch.  When false, OSC 133 markers are still parsed (because
  # FtcsState matters for other features) but command_blocks is not populated
  # in snapshots and the UI shows no command-aware affordances.  Default true.
  enabled = true

  # Show the duration of long-running commands ("1.3s") next to the gutter.
  # Threshold below which the duration is suppressed.  Default 2 seconds.
  show_duration = true
  duration_threshold_secs = 2.0
  ```

- Add corresponding `ShellIntegrationConfig` and `CommandBlocksConfig` structs
  with `#[serde(default)]`.
- Add a new Settings Modal tab: **Shell Integration**.
  - Toggle for `set_term_program`.
  - Toggle for `auto_install`.
  - Read-only display of the install path with "Open Folder" / "Copy Path"
    buttons.
  - A "Re-install Scripts" button that overwrites the on-disk copies.
  - A snippet preview showing the bash one-liner to source the file.
- Add a new section "Command Blocks" inside the existing **Behavior** tab
  (or wherever fits naturally — agent chooses based on current Settings
  taxonomy):
  - Toggle for `command_blocks.enabled`.
  - Toggle for `show_duration` and a slider for `duration_threshold_secs`.

**Verification:** Round-trip TOML test (write defaults, parse back, equal).
Snapshot test that disabling `command_blocks.enabled` makes
`snap.command_blocks` empty.

**Completion notes (commit `467ca40`, 2026-05-17):**

- The plan originally said Command Blocks should go inside a
  "Behavior" tab. That tab does not exist in the current Settings UI.
  Decision: bundle BOTH new sections into ONE new "Shell Integration"
  tab between Tabs and Bell. Rationale: keeps the OSC 133 story in one
  place; avoids disturbing the existing 12-tab taxonomy.
- `SettingsTab::ALL` array size adjusted `[Self; 12] → [Self; 13]`; the
  `ALL.len() == 12` test assertion adjusted to 13. These are the only
  deletions in the entire diff.
- Read-only install path is rendered via `ui.monospace(...)` (the
  codebase's egui version does not expose `ui.code(...)`).
- "Re-install Scripts" and "Copy Path" buttons are no-op placeholders
  with `on_hover_text("Wired in subtask 72.8 — currently inactive.")`.
  Real wiring lands in 72.8.
- Duration threshold uses `egui::DragValue::range(0.0..=60.0).suffix(" s")`
  with `speed(0.1)`. `DragValue::range` exists in this codebase (used in
  `settings.rs:960`).
- `#[serde(default)]` on both new structs and on every field so old
  config files load unchanged.
- 1 new round-trip TOML test in `freminal-common/src/config.rs`
  (`shell_integration_and_command_blocks_round_trip_through_toml`),
  +1 assertion line in `freminal/src/gui/settings.rs`
  (`settings_tab_labels`), `ALL.len()` assertion updated.
- `cargo test --all` workspace-wide passes (no regressions).
- `cargo clippy --all-targets --all-features -- -D warnings` clean.
- `cargo-machete` clean.
- `freminal-common`: 807 → 808; `freminal`: 388 → 389.
- `settings_dispatch.rs` was NOT modified — the new config sections
  do not require live broadcast; they take effect on next PTY spawn
  (72.6) or first launch (72.8).

#### 72.6 — TERM_PROGRAM environment variables

**Scope:** `freminal/src/gui/pty.rs`, `tab_spawning.rs`.

- In the PTY spawn path, always set `TERM_PROGRAM=freminal` and
  `TERM_PROGRAM_VERSION=<env!("CARGO_PKG_VERSION")>` (gated on
  `config.shell_integration.set_term_program`).
- These env vars layer beneath `LayoutPane::env` overrides (so users can
  unset them by setting an empty value in a layout).

**Verification:** Unit test on the env-merge helper (existing or new). Manual
verification by running `echo $TERM_PROGRAM` in a freminal session.

**Completion notes (commit `46dfbfd`, 2026-05-17):**

- **Scope correction:** The plan listed the scope as `freminal/src/gui/pty.rs`
  and `tab_spawning.rs` only. In practice, `TERM_PROGRAM` was already set
  unconditionally in `freminal-terminal-emulator/src/io/pty.rs`, so 72.6
  is the _gating_ of pre-existing functionality, not net-new code.
  Actual files modified: 5 (the gating flag must plumb through
  `PtyTabConfig` → `TerminalEmulator::new` → `PtySpawnConfig` →
  `run_terminal`).
- **Pre-existing version-string format preserved exactly:**
  `format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("VERGEN_GIT_DESCRIBE"))`.
  A prior attempt silently degraded this to plain
  `env!("CARGO_PKG_VERSION")` (dropping the git-describe suffix); that
  attempt was rejected and reverted. The retry adds an explicit
  regression test (`version_string_carries_vergen_git_describe_in_parens`)
  that asserts the suffix is present.
- New `pub fn term_program_env_pairs() -> [(&'static str, String); 2]`
  helper in `freminal-terminal-emulator/src/io/pty.rs`. Non-const because
  the version string is a runtime `format!()` result.
- `PtySpawnConfig::set_term_program: bool` added.
- `TerminalEmulator::new` gains a `set_term_program: bool` parameter at
  the end of the argument list. Already had `#[allow(clippy::too_many_arguments)]` from prior work, so no new allow needed.
- `PtyTabConfig::set_term_program: bool` added.
- 5 `PtyTabConfig { ... }` construction sites updated to pass
  `self.config.shell_integration.set_term_program`: 3 in
  `tab_spawning.rs` (spawn_new_tab, spawn_split_pane, spawn_pane_from_leaf)
  and 2 in `app_impl.rs` (on_window_created at line 155,
  create_first_window_with_default_pty at line 1621).
- TERM_PROGRAM is applied BEFORE the `extra_env` loop in `run_terminal`,
  so layout/user env can override (e.g. setting `TERM_PROGRAM = ""` in a
  layout disables our value for that pane).
- 5 new tests in `io::pty::term_program_tests` covering the (key, value)
  contract and the VERGEN-suffix regression guard.
- `cargo test --all` workspace passes (no regressions).
- `cargo clippy --all-targets --all-features -- -D warnings` clean.
- `cargo-machete` clean.
- `freminal-terminal-emulator` test count: 2324 → 2329.

#### 72.7 — Ship shell integration scripts

**Scope:** New top-level directory `shell-integration/`.

- Create three scripts:
  - `shell-integration/freminal.bash`
  - `shell-integration/freminal.zsh`
  - `shell-integration/freminal.fish`
- Each script must:
  - Emit `OSC 133 ; A ST` immediately before the prompt is drawn (PROMPT_COMMAND
    in bash; precmd in zsh; fish_prompt event in fish).
  - Emit `OSC 133 ; B ST` immediately after the prompt is drawn (PS1 prefix in
    bash via `\[\e]133;B\a\]`, or via a precmd that wraps PS1; fish_prompt
    epilogue).
  - Emit `OSC 133 ; C ST` immediately before executing the command (DEBUG trap
    in bash; preexec in zsh; fish_preexec in fish).
  - Emit `OSC 133 ; D ; $? ST` immediately after command completion
    (PROMPT_COMMAND in bash, precmd in zsh, fish_postexec in fish).
  - Emit `OSC 7 ; file://hostname/pwd ST` from precmd / fish_prompt for cwd
    tracking.
  - Be idempotent (sourcing twice is harmless).
  - Detect freminal via `$TERM_PROGRAM` and no-op otherwise (so users can
    safely source unconditionally in their rc files).
- Reference implementations to study (do not copy verbatim):
  - WezTerm's `assets/shell-integration/` directory.
  - iTerm2's `iterm2_shell_integration` scripts.
- Add `README.md` in `shell-integration/` explaining what each does and how to
  source them.

**Verification:**

- Each script must parse cleanly in its target shell (`bash -n`, `zsh -n`,
  `fish -n`). Add a CI check (extend `cargo xtask ci` if reasonable, or a
  GitHub Action step) running these.
- Manual end-to-end test: source the script, run a few commands, verify
  freminal builds `CommandBlock`s with correct exit codes.

#### 72.8 — Auto-install shell integration scripts on first launch

**Scope:** `freminal/src/main.rs` or `freminal/src/gui/run.rs` (wherever
startup-side filesystem setup lives).

- On startup, if `config.shell_integration.auto_install == true` and the
  destination directory does not exist, copy the three scripts from a
  compile-time `include_str!()` bundle into
  `~/.config/freminal/shell-integration/`.
- Embed the scripts via `include_str!("../shell-integration/freminal.bash")`
  etc. in a small `shell_integration.rs` module.
- If any individual file already exists in the destination, do not overwrite
  (respect user customizations). Skip silently.
- Failures are non-fatal — log a warning via `tracing::warn!` and surface a
  toast via the existing `ToastStack::warning` API.
- The "Re-install Scripts" Settings button (from 72.5) explicitly overwrites.

**Verification:** Integration test (with a tempdir as `$XDG_CONFIG_HOME`)
verifying that the three files exist after first launch and are not
overwritten on second launch.

#### 72.9 — `WindowCommand::CommandFinished` GUI handling

**Scope:** `freminal/src/gui/app_impl.rs` (the WindowCommand match), and a new
small per-pane data structure for the GUI's view of finished commands.

- Add a new field on `Pane` (or `Tab`):
  `recent_commands: VecDeque<CommandBlock>` capped at e.g. 64 entries.
- On receiving `WindowCommand::CommandFinished { pane_id, block }`:
  - Push `block` onto `recent_commands` for the matching pane.
  - If the tab containing `pane_id` is currently visible and focused, do
    nothing else (the snapshot data already drives the gutter).
  - If the tab is not focused, set a `tab.has_pending_event = true` flag and
    refresh the tab bar. (Visual indicator handled in 72.10.)
  - Hand off to Task 76's notification path. Task 76 implementation may add
    the dispatch here behind a feature check.

**Verification:** Unit test of the recent_commands ring buffer. Integration
test verifying that a CommandFinished event reaches the GUI thread.

#### 72.10 — Fold/collapse view state

**Scope:** `freminal/src/gui/view_state.rs`, the terminal renderer
(`freminal/src/gui/renderer/` or `terminal/widget.rs`).

- Add to `ViewState`:

  ```rust
  pub folded_blocks: std::collections::HashSet<CommandBlockId>,
  ```

- A "folded" block hides all rows from `command_start_row` to `end_row`
  (inclusive), keeping only the prompt and command line visible. If the block
  has no `end_row` (still running) it is not foldable.
- When rendering the visible cell grid:
  - Walk `snap.command_blocks` once per frame, build a sparse
    `Vec<RowSkip>` describing which row ranges to skip.
  - In the row iteration loop, skip rows belonging to folded blocks.
  - Render a single-row placeholder line at the fold point reading
    `"  N lines hidden"` with a unicode triangle. The placeholder is mouse-
    clickable to unfold.
- New `KeyAction` variants (per AGENTS.md keybinding convention):
  - `ToggleFoldAtCursor` — fold/unfold the block at the active pane's cursor
    or topmost visible row. Default binding: `Ctrl+Shift+F`.
  - `FoldAll` — fold every block in `recent_commands`. Default binding: none.
  - `UnfoldAll` — clear `folded_blocks`. Default binding: `Ctrl+Shift+U`.
- Mouse: clicking on a command's gutter (Task 73 surface) also toggles fold.
  Until Task 73 lands, document the gutter region as "reserved 4px column,
  click target".

**Verification:**

- Unit test: fold/unfold round-trip on a known block.
- Visual test (manual): commands shorter than 1 row cannot be folded, multi-
  row commands display the placeholder and are unfoldable.
- Benchmark: extend `freminal/benches/render_loop_bench.rs` with a benchmark
  rendering 100 blocks with 50% folded. Acceptable regression budget per
  AGENTS.md: 15%.

#### 72.11 — Copy command output actions

**Scope:** `freminal-common/src/keybindings.rs`, `freminal/src/gui/actions.rs`,
`freminal/src/gui/clipboard.rs` (or wherever clipboard writes happen today).

- New `KeyAction` variants:
  - `CopyLastCommandOutput` — copy the output range of the most recent
    finished block to clipboard. Default binding: `Ctrl+Shift+Y`.
  - `CopyCommandOutputAtCursor` — copy the output range of the block
    containing the cursor's current visible row. Default: no binding (added
    via right-click menu, see below).
- Implementation:
  - Find the target block by id.
  - Compute the row range `[output_start_row, end_row]`.
  - Extract the corresponding rows from the snapshot's text data (same path
    used by selection copy today — search for `ExtractSelection` usage).
  - Send the resulting text to the clipboard via the existing `arboard` path.
- Add a right-click context menu entry "Copy Command Output" when the click
  falls within a known command block (find the block whose row range
  contains the clicked row).

**Verification:** Unit test for block-by-row lookup. Integration test that
puts known text on the clipboard after the action fires.

#### 72.12 — Hover highlight and command-duration overlay

**Scope:** `freminal/src/gui/renderer/`, `freminal/src/gui/mouse.rs`.

- On mouse hover, identify the command block under the cursor by row. Tint
  the block's row range with a subtle background overlay (e.g. theme's
  selection-tint at 25% alpha).
- For finished blocks where `duration() >= config.command_blocks.duration_threshold_secs`,
  draw a small right-aligned label `"1.3s"` (or `"15ms"`, `"2.1m"` etc. — use
  `humantime` or a small inline formatter) at the end of the command's first
  row. Use a muted theme color.
- Hover highlight is purely view-state; no snapshot mutation.
- Hover is disabled when `config.command_blocks.enabled == false`.

**Verification:** Manual visual verification with a recording. Unit test for
the duration-formatting helper.

#### 72.13 — `ESCAPE_SEQUENCE_COVERAGE.md` and `ESCAPE_SEQUENCE_GAPS.md` updates

**Scope:** `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`.

- OSC 133 A/B/C/D/P are already parsed; the coverage table should already
  reflect that. Verify and update:
  - Status icon for OSC 133 → fully supported (was partial).
  - Notes column → "Drives CommandBlock storage, gutters, notifications,
    fold/collapse." Task reference: 72.
- Update both "Last updated" lines.

**Verification:** The two docs must parse without warnings (markdownlint if
the project runs it) and continue to align with each other.

#### 72.16 — Cleanup: pre-existing bugs surfaced during Task 72

**Convention (project-wide, established 2026-05-17):** when a sub-agent
surfaces a bug during a subtask that is genuinely out-of-scope for that
subtask, the bug is filed as a numbered cleanup entry in the host task's
plan section. The cleanup subtask runs near the end of the task, before
any user-facing subtask that would expose the bug. The original subtask's
completion notes link to the cleanup entry by number. This is the
durable record — informal "known issues" notes are not used.

**Items in 72.16's queue:**

##### 72.16.a — Fix OSC 133 numeric-param filter

**Surfaced in:** 72.4 (commit `27fa949`, 2026-05-17).

**Bug:** `freminal-terminal-emulator/src/ansi_components/osc.rs:251-254`
— the OSC 133 (FTCS) dispatcher's params filter only keeps
`AnsiOscToken::String` tokens, silently dropping `AnsiOscToken::OscValue`
numeric tokens. The `AnsiOscToken::from_str` impl parses any numeric
substring as `OscValue(u16)`, so `OSC 133 ; D ; 0 ST` arrives at
`parse_ftcs_params` as `["D"]` rather than `["D", "0"]`, producing
`CommandFinished(None)` instead of `CommandFinished(Some(0))`.

**Impact:**

- Real shells emitting `OSC 133 ; D ; <code>` (bash, zsh, fish via the
  shell integration scripts shipping in 72.7) lose the exit code.
- FREC v2 recordings of OSC 133 streams replay with no exit codes
  (no observable effect today because no GUI surface consumes
  `exit_code` yet; matters once 73 ships gutters).
- All command-block visualization that depends on exit_code (gutters,
  notifications, copy-on-failure flows).
- The handler-direct test path in `shell_integration.rs` is UNAFFECTED
  (it constructs `FtcsMarker::CommandFinished(Some(0))` directly), which
  is why the bug went undetected through 72.3.

**Scope:** `freminal-terminal-emulator/src/ansi_components/osc.rs` only.

**Suggested approach (revisit at activation):** Build the
`ftcs_strs: Vec<String>` by mapping each token to its display form
(`AnsiOscToken::OscValue(n)` → `n.to_string()`,
`AnsiOscToken::String(s)` → `s.clone()`). Then collect `&str` refs into
a second `Vec<&str>` from those owned strings and pass to
`parse_ftcs_params`. Cost: one allocation per numeric token, acceptable
for OSC dispatch frequency.

**Verification:**

- Add a byte-stream-driven test in `freminal-terminal-emulator/src/interface.rs`
  (`mod tests`) that emits `OSC 133 D ; 127 ST` via
  `handle_incoming_data` and verifies the resulting `CommandBlock` has
  `exit_code == Some(127)` and `status() == Failure(127)`.
- Tighten the existing 72.4 tests
  (`build_snapshot_populated_command_blocks`,
  `build_snapshot_command_blocks_ordering`) to assert exit codes once
  the filter is fixed.
- Check whether the same numeric-filter issue affects other OSC
  handlers in `osc.rs`. If yes, list them here and decide whether to
  fix in this subtask or file additional 72.16.x entries.

**Scheduling:** Must complete before 72.7 (shell integration scripts).
Real shells will emit numeric exit codes the moment users source the
scripts; shipping 72.7 without 72.16.a would mean shipping a known
broken feature.

**Status:** Pending.

### 72 Open Questions Resolved

All resolved at planning time. Refer to "Cross-Cutting Design Decisions" above.

### 72 Benchmarks (mandatory before/after)

Per AGENTS.md: any change touching the buffer, parser, snapshot builder, or
renderer must capture before/after benchmark numbers. The relevant benches:

- `buffer_row_bench.rs` — affected by 72.2.
- `buffer_benches.rs` — `bench_build_snapshot`, `bench_build_snapshot_with_scrollback`
  affected by 72.4.
- `render_loop_bench.rs` — affected by 72.10 (fold) and 72.12 (hover/duration).

Each subtask's commit message must contain a before/after table. Regressions

> 15% halt the subtask.

---

## Task 73 — Command Gutters

### 73 Summary

A 4-pixel left gutter rendered inside the terminal area, left of the cell
grid. Each command block's row range is filled with a status color: green
(success), red (failure), yellow (running), gray (unknown). The gutter is
clickable (toggles fold, see 72.10) and hover-able (highlights the block).

### 73 Decisions (fixed)

- **Rendering:** A thin (4px) column reserved at the very left of the
  terminal area, inside the cell grid bounds. The 4px slice is taken from
  the available pixel width before computing cell columns. Cell column count
  is unchanged for content; effective rendering width shrinks by 4px when
  the gutter is enabled.
- **Color source:** Theme palette. Add three dedicated semantic colors to
  `ThemeConfig`/`ThemePalette` with sensible defaults derived from the
  palette's existing green/red/yellow.

### 73 Subtasks

#### 73.1 — `ThemePalette` gutter colors

**Scope:** `freminal-common/src/themes.rs` (or wherever `ThemePalette` lives).

- Add three new optional fields to `ThemePalette`:

  ```rust
  pub gutter_success: Option<Color32>,  // defaults to palette.ansi.green
  pub gutter_failure: Option<Color32>,  // defaults to palette.ansi.red
  pub gutter_running: Option<Color32>,  // defaults to palette.ansi.yellow
  ```

- Provide a `gutter_color_for(status)` resolver returning the configured
  color or the appropriate fallback.

**Verification:** Round-trip TOML test. The themes count test (mentioned in
`PLAN_33_WEZTERM_GHOSTTY_PALETTES.md` history) does not need updating —
existing themes default to None and use the fallback.

#### 73.2 — Render the gutter column

**Scope:** `freminal/src/gui/renderer/` (whichever pass draws the terminal
cell background — likely a glow shader / atlas layer).

- Add a config knob `[command_blocks] gutter = "left" | "off"` (default
  `"left"`).
- When `gutter == "left"`, reserve 4px from the terminal area's left edge.
  Cell origin shifts right by 4px. Cell width calculation uses
  `terminal_width_px - 4` instead of `terminal_width_px`.
- For each visible row, determine which `CommandBlock` (if any) contains it
  and draw a 4px×row_height rectangle in the appropriate status color.
- Rows not inside any block render an empty gutter (background color).
- Folded-block placeholder rows render a slightly desaturated gutter color.

**Verification:** Visual verification via a recorded `.frec` of a typical
session. Benchmark: `render_loop_bench` should not regress > 15%.

#### 73.3 — Gutter click and hover

**Scope:** `freminal/src/gui/mouse.rs`, `terminal/widget.rs`.

- Mouse events whose `x` coordinate falls within the gutter (0..4px) are
  intercepted before the usual cell-coordinate routing.
- Single click on a finished block: toggle fold (same path as 72.10's
  `ToggleFoldAtCursor` keybinding).
- Single click on a running block: no-op (cannot fold).
- Hover within the gutter: emit the same hover-highlight overlay as 72.12
  (entire block tinted).

**Verification:** Integration test using egui's test harness for mouse
events.

#### 73.4 — Settings UI: gutter toggle

**Scope:** `freminal/src/gui/settings.rs` / `settings_dispatch.rs`.

- Add a dropdown in the Command Blocks section (introduced in 72.5): Gutter
  position (`Left` / `Off`).

**Verification:** Toggle persists via TOML round-trip.

### 73 Open Questions Resolved

All resolved.

### 73 Benchmarks

- `render_loop_bench` — gutter is a new render pass. Record before/after.

---

## Task 74 — Broadcast Input to Panes

### 74 Summary

A per-tab toggle. When on, every `InputEvent::Key` event fans out to every
leaf pane in the tab. Mouse events and per-pane window commands are not
broadcast. A visual indicator on the tab bar shows broadcast is active.

### 74 Decisions (fixed)

- **Selection model:** Per-tab toggle, not per-pane selection set.
- **What gets broadcast:** All keyboard `InputEvent::Key` payloads (text,
  control sequences, paste payloads — each pane independently applies its
  bracketed-paste wrap based on its own mode). Mouse events do not broadcast.
- **No cross-tab/cross-window broadcast** in v0.9.0.

### 74 Subtasks

#### 74.1 — `Tab::broadcast_input` flag

**Scope:** `freminal/src/gui/tabs.rs`.

- Add `pub broadcast_input: bool` to `Tab`, default `false`.
- `Tab::default()` and constructor initialize to `false`.
- `Tab::toggle_broadcast(&mut self)` helper.

**Verification:** Trivial unit tests.

#### 74.2 — `KeyAction::ToggleBroadcastInput`

**Scope:** `freminal-common/src/keybindings.rs`, `freminal/src/gui/actions.rs`.

- Add `KeyAction::ToggleBroadcastInput` per the AGENTS.md keybinding
  convention: `name()`, `display_label()`, `FromStr`, `ALL`, default binding
  (proposed `Ctrl+Shift+I`), `config_example.toml` entry, and dispatch in
  `dispatch_binding_action` (or `gui/actions.rs` if it needs higher-level
  state).
- The action toggles `active_tab.broadcast_input`.

**Verification:** Keybinding round-trip test (existing harness covers this).

#### 74.3 — Fan-out in input dispatch

**Scope:** `freminal/src/gui/terminal/input.rs`.

- Where `InputEvent::Key(bytes)` is sent to a single pane's `input_tx`, gate
  on `tab.broadcast_input`:
  - If `false`: send to active pane only (current behavior).
  - If `true`: walk `tab.pane_tree.iter_panes()` and send to each pane's
    `input_tx`. Failed sends are logged but do not abort the broadcast.
- This applies to:
  - Direct `Event::Text` keyboard payloads.
  - `KeyAction::Paste` payloads (broadcasts the bracketed-paste-wrapped
    bytes to every pane).
  - Synthesized control sequences (e.g. arrow keys mapped through the
    kitty/legacy paths).
- This does **not** apply to:
  - Mouse events (`InputEvent::Mouse*`).
  - Resize events (each pane has its own size).
  - Snapshot requests.
  - Copy/selection requests (selection is view-state, per-pane).
- Visual indicator: when broadcast is active, the tab bar item for that tab
  shows a small icon (e.g. a "📡" or a colored dot — agent chooses, document
  the choice in the subtask commit). Existing `Tab::custom_name` rendering
  in `menu.rs` is the integration point.

**Verification:**

- Integration test: enable broadcast on a tab with 3 panes, send a key,
  verify all 3 panes' input_tx received the bytes.
- Mouse-event test: send a mouse event with broadcast on, verify only the
  pane under the cursor received it.
- Manual smoke: ssh to two hosts in split panes, broadcast `uptime`.

#### 74.4 — Visual indicator on panes too

**Scope:** `freminal/src/gui/renderer/` or `terminal/widget.rs`.

- When `tab.broadcast_input` is `true`, each pane's border (the existing
  split-border rendering) is tinted (e.g. yellow or a configurable color).
- A small text label "BROADCAST" in the pane's top-right corner — agent
  chooses placement, must not conflict with the password-indicator lock
  icon location.

**Verification:** Manual visual verification.

#### 74.5 — Settings UI

**Scope:** `freminal/src/gui/settings.rs`.

- Add a section in the existing Tabs settings panel: "Broadcast Input".
  - Show the current keybinding (read-only, with a "Change" button that
    opens the keybinding editor for the action).
  - Confirm dialog toggle: "Confirm before enabling broadcast" (default
    false). When true, the first-time `ToggleBroadcastInput` shows a modal
    "You are about to broadcast keyboard input to N panes. Continue?".

**Verification:** Toggle persists.

### 74 Open Questions Resolved

All resolved.

### 74 Benchmarks

Broadcast adds a fan-out loop. Acceptable because it only runs on key events
(not per-frame). No benchmark required unless 74.3 introduces unexpected
per-frame work.

---

## Task 75 — Verify Per-Pane Env Round-Trip

### 75 Summary

`LayoutPane::env` already exists (`freminal-common/src/layout.rs:175`) and is
already passed through PTY spawn (`freminal/src/gui/tab_spawning.rs:341`).
The original Task 75 ("Workspace-Scoped Environment") asked for more
ambitious workspace-level env + theme + font + profile binding, but the
profile concept doesn't exist in v0.9.0 and per-pane env covers the realistic
v0.9.0 use cases. **Defer broader workspace scoping to v0.10.0 alongside
Profiles (Task 78).**

Task 75 in v0.9.0 is reduced to: verify the round-trip works, document it,
and add explicit tests.

### 75 Decisions (fixed)

- **Scope:** Per-pane env only. No layout-wide / window-wide / tab-wide env
  layering in v0.9.0.
- **Defer to v0.10.0:** All of theme override, font override, profile
  binding, layout-wide `[layout.env]` section.

### 75 Subtasks

#### 75.1 — Round-trip test

**Scope:** `freminal/tests/` or `freminal-common/src/layout.rs` tests.

- Author a layout TOML with two panes, each carrying `env = { FOO = "bar",
BAZ = "${user}" }`.
- Load via `Layout::from_toml_str`, resolve with variables, verify each
  resolved pane has the expected env map.
- Save a synthetic in-memory layout via `save_layout` and verify the env
  appears in the output TOML.

**Verification:** New tests pass; existing tests still pass.

#### 75.2 — Documentation

**Scope:** `Documents/LAYOUT_FORMAT.md`.

- Confirm the `env` row in the schema table is documented (already at
  line 163). Add an example using variable substitution:

  ```toml
  [[windows.tabs.panes]]
  directory = "$1"
  env = { PROJECT_ROOT = "$1", AWS_PROFILE = "$ENV{AWS_PROFILE}" }
  ```

- Update the "Last updated" header line.

**Verification:** Doc compiles via markdownlint (if configured), example
parses with the layout loader.

### 75 Open Questions Resolved

All resolved.

### 75 Benchmarks

None. No code change.

---

## Task 76 — Notification System (OSC 9 / OSC 777)

### 76 Summary

When OSC 133 D fires (Task 72.3), or when the shell explicitly emits OSC 9
or OSC 777, surface a notification. Notifications route to:

- The in-app toast stack (always, when enabled).
- The system notification daemon via `notify-rust` (when enabled and
  freminal is unfocused).

Also: a configurable bell sound on command completion (extending the
existing `BellConfig`).

### 76 Decisions (fixed)

- **Crate:** `notify-rust = "4"` added to `freminal/Cargo.toml`. Pure-Rust
  on Linux/macOS, winapi-based on Windows.
- **Default:** `[notifications] enabled = false` — opt-in.
- **OSC sequences:** OSC 9 (iTerm2/WezTerm) and OSC 777 (urxvt). OSC 99
  (kitty) deferred.
- **Capability advertisement:** Set `TERM_PROGRAM=freminal` (handled in
  72.6) and advertise via terminfo + XTGETTCAP. Document detection in the
  shell integration README.

### 76 Subtasks

#### 76.1 — Add `notify-rust` and capability flags

**Scope:** `freminal/Cargo.toml`, `freminal-common/src/config.rs`.

- Add `notify-rust = "4"` to `freminal/Cargo.toml` (binary crate only).
  **Do not** add it to `freminal-common`, `freminal-buffer`, or
  `freminal-terminal-emulator`.
- Add `[notifications]` config section:

  ```toml
  [notifications]
  # Master switch for the notification system.  Default false (opt-in).
  enabled = false

  # When true, OSC 9 (iTerm2) text payloads create notifications.
  osc_9 = true

  # When true, OSC 777 "notify;TITLE;BODY" payloads create notifications.
  osc_777 = true

  # When true, OSC 133 D (command finished) fires a notification if the
  # tab is currently unfocused.  Default true.
  on_command_finished = true

  # Minimum command duration (seconds) before command-finished
  # notifications fire.  Avoids spamming for fast commands.  Default 10.
  command_finished_threshold_secs = 10.0

  # Routing per category: "toast", "system", or "both".  Default:
  # errors and command-completion → system when unfocused, toast always.
  routing_error = "both"
  routing_info = "toast"
  routing_command_finished = "system_when_unfocused"
  ```

- Add `NotificationsConfig` struct with `#[serde(default)]`.

**Verification:** Config round-trip test. `cargo build` succeeds with
notify-rust dep.

#### 76.2 — OSC 9 and OSC 777 parsing

**Scope:** `freminal-common/src/buffer_states/osc.rs`,
`freminal-terminal-emulator/src/ansi_components/osc.rs`.

- Add `OscTarget::Notify9` and `OscTarget::Notify777` variants.
- Map OSC 9 to `Notify9`, OSC 777 to `Notify777` in the OSC code→target
  table.
- Add `AnsiOscType::Notify { title: Option<String>, body: String }` (one
  variant for both; the parsers differ).
- OSC 9 parsing: the entire payload after `9;` is the body. No title.
- OSC 777 parsing: payload of the form `notify;TITLE;BODY` (urxvt's
  convention). Split on `;` at most twice. If only `TITLE` is present,
  body is empty. If `notify;` prefix is missing, the entire payload is the
  body.
- Add unit tests covering well-formed and malformed payloads for each.

**Verification:** Unit tests for both parsers. Update
`ESCAPE_SEQUENCE_COVERAGE.md` and `ESCAPE_SEQUENCE_GAPS.md` per AGENTS.md.

#### 76.3 — OSC 9/777 dispatch

**Scope:** `freminal-terminal-emulator/src/terminal_handler/osc.rs`.

- New `handle_osc_notify(&mut self, notify: &AnsiOscType::Notify)` arm that
  forwards a new `WindowCommand::Notification { kind: NotificationKind,
title: Option<String>, body: String }` to the GUI thread via the existing
  window-post channel.
- `NotificationKind` enum: `OscText`, `CommandFinished`, `Error`, `Info`.

**Verification:** Unit test that emitting OSC 9 produces a
`WindowCommand::Notification`.

#### 76.4 — GUI notification router

**Scope:** `freminal/src/gui/app_impl.rs` (handle WindowCommand) and a new
`freminal/src/gui/notifications.rs` module.

- New module `notifications.rs` exporting `NotificationRouter`.
- Router consumes `WindowCommand::Notification` and
  `WindowCommand::CommandFinished` events.
- For each event, apply the routing policy from config:
  - If routing includes `toast`, push to `ToastStack` (kind based on
    NotificationKind).
  - If routing includes `system` or `system_when_unfocused` (and the latter's
    focus check passes), call `notify_rust::Notification::new().summary(...).body(...).show()`.
    Wrap the call in `tokio::task::spawn_blocking` or `std::thread::spawn`
    since notify-rust blocks briefly on Linux dbus calls.
- For `CommandFinished`, format the body using a template:
  `"{command} finished in {duration} with exit code {exit_code}"`. Make
  the template configurable in 76.5.
- Threshold check for command-finished: skip if `duration < threshold`.

**Verification:** Integration tests using a mock router that records calls
instead of dispatching. Manual test on Linux (notify-osd or KDE
notifications), macOS (Notification Center), Windows (toast notifications).

#### 76.5 — Notification templates and bell

**Scope:** `freminal-common/src/config.rs` (extend NotificationsConfig and
existing `BellConfig`).

- Add to NotificationsConfig:

  ```toml
  # Template for command-completion notifications.  Tokens: {command},
  # {duration}, {exit_code}, {cwd}, {tab_name}.
  command_finished_template = "{command} finished in {duration} (exit {exit_code})"
  ```

- Extend `BellConfig`:

  ```toml
  [bell]
  # Existing: enabled, audio, visual
  # NEW: also ring the bell on OSC 133 D when tab is unfocused.
  on_command_finished = false
  ```

- The notification router, when firing a command-finished event, also
  invokes `bell::ring()` (the existing audio path) gated on
  `bell.on_command_finished`.

**Verification:** Template-render unit test. Bell-ring integration with the
existing test harness (if any).

#### 76.6 — Capability advertisement

**Scope:** `freminal/freminal.ti` (terminfo source),
`freminal-terminal-emulator/src/terminal_handler/` (XTGETTCAP responder).

- Add notification-related capabilities to terminfo:
  - Document that freminal supports OSC 9 and OSC 777 (terminfo doesn't
    have a dedicated capability for these; add comment lines).
- Extend XTGETTCAP responses (the existing handler — find via grep for
  `XTGETTCAP`):
  - Respond to `XTGETTCAP TN` (Terminal Name) with `freminal`.
  - Respond to `XTVERSION` with `\eP>|freminal v<version>\e\\`.
- Document detection recipes in `shell-integration/README.md`:

  ```bash
  if [ "${TERM_PROGRAM:-}" = "freminal" ]; then
      notify_via_osc9() { printf '\e]9;%s\a' "$1"; }
  fi
  ```

**Verification:** XTGETTCAP and XTVERSION round-trip tests (existing test
harness for terminfo audit, see PLAN_12_TERMINFO.md).

#### 76.7 — Settings UI: Notifications tab

**Scope:** `freminal/src/gui/settings.rs`, `settings_dispatch.rs`.

- New top-level Settings tab: **Notifications**.
- Sections:
  - **Master switch:** `enabled` toggle.
  - **OSC Sources:** `osc_9`, `osc_777`, `on_command_finished` toggles.
  - **Command Threshold:** slider for `command_finished_threshold_secs`.
  - **Routing:** three dropdowns for `routing_error`, `routing_info`,
    `routing_command_finished` (values: Toast / System / Both /
    SystemWhenUnfocused).
  - **Template:** text edit for `command_finished_template` with a
    "Test Notification" button that dispatches a sample notification.
  - **Bell:** toggle for `bell.on_command_finished` (cross-references the
    Bell settings panel).

**Verification:** Toggle persistence; "Test Notification" actually fires a
notification through the configured routing.

#### 76.8 — Docs

**Scope:** `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`,
`shell-integration/README.md`.

- Add OSC 9 and OSC 777 to the coverage table (status: implemented).
- Remove from the gaps doc.
- Document the OSC 9 / OSC 777 examples in the shell-integration README
  with the `TERM_PROGRAM` detection idiom.

**Verification:** Per AGENTS.md "Escape Sequence Documentation" rules.

### 76 Open Questions Resolved

- **Filtering:** Duration threshold (per-config) for command-finished
  notifications. Per-tab filtering deferred to v0.10.0 with Profiles.
- **OSC coverage:** OSC 9 + OSC 777. OSC 99 (kitty) deferred.
- **Template:** Configurable string template with `{command}`, `{duration}`,
  `{exit_code}`, `{cwd}`, `{tab_name}` tokens.
- **Routing:** Configurable per category (error / info / command_finished).
  Default routing for command_finished is `system_when_unfocused`.

### 76 Benchmarks

None mandated. Notifications are event-driven, not per-frame.

---

## Task 77 — Smart Paste Guard

### 77 Summary

Multi-line paste shows a preview dialog requiring confirmation before
sending to the PTY. Optional pattern-based detection escalates the warning
for dangerous patterns. Per-config toggle to disable.

### 77 Decisions (fixed)

- **Default trigger:** Multi-line content (contains `\n`).
- **Optional trigger:** Dangerous pattern match — opt-in.
- **Bypass:** `KeyAction::PasteUnsafe` skips the guard.
- **Profile-level toggle:** Deferred to v0.10.0 (profiles don't exist yet).
  Use a global config toggle in v0.9.0.

### 77 Subtasks

#### 77.1 — Config schema

**Scope:** `freminal-common/src/config.rs`, `config_example.toml`.

- Add `[paste_guard]` section:

  ```toml
  [paste_guard]
  # Master switch.  Default true (safer-by-default).
  enabled = true

  # Confirm any paste containing a newline.  Default true.
  multiline = true

  # Confirm any paste containing control characters (ESC, BEL, etc.) other
  # than the bracketed-paste markers we wrap with.  Default true.
  control_chars = true

  # When true, additionally match dangerous patterns.  Default false.
  patterns = false

  # Additional regex patterns to treat as dangerous (Rust regex syntax).
  # Default list:
  pattern_list = [
      "\\brm\\s+-rf?\\b",
      "\\bcurl\\b[^|]+\\|\\s*(sh|bash|zsh|fish)\\b",
      "\\bwget\\b[^|]+\\|\\s*(sh|bash|zsh|fish)\\b",
      "\\bsudo\\b",
      "\\bdoas\\b",
      "\\bdd\\s+.*of=/dev/",
      "\\bmkfs\\.",
  ]
  ```

- Add `PasteGuardConfig` struct with `#[serde(default)]`.
- Validate that user-supplied regex patterns compile at load time; report
  malformed patterns via the existing toast system.

**Verification:** Round-trip TOML test. Regex compile-time validation test.

#### 77.2 — Paste analyzer

**Scope:** New module `freminal/src/gui/paste_guard.rs`.

- Pure-function `analyze(payload: &str, config: &PasteGuardConfig) -> PasteAnalysis`.
- `PasteAnalysis` enum:
  - `Safe` — no triggers fired.
  - `Multiline { line_count: usize, byte_count: usize }`.
  - `ControlChars { chars: Vec<char> }`.
  - `Patterns { matched: Vec<String> }`.
  - `Multiple { triggers: Vec<PasteAnalysis> }` (when more than one fires).
- Pre-compile pattern regexes once at config-load time and cache them on
  `PasteGuardConfig` (use `OnceCell` or rebuild on hot-reload).

**Verification:** Unit tests for each trigger and combination thereof.

#### 77.3 — Preview dialog

**Scope:** `freminal/src/gui/paste_guard.rs` (UI) and integration with the
existing modal pattern (look at the existing Settings modal infra in
`freminal/src/gui/settings.rs` as a template).

- An egui modal window titled "Confirm Paste":
  - Top: a banner indicating the trigger ("Multi-line paste — 17 lines",
    "Dangerous patterns detected: `rm -rf`, `sudo`").
  - Middle: a scrollable read-only text area showing the paste content with
    syntax highlighting via the existing renderer (or plain monospace if
    that's too invasive).
  - Bottom: "Paste Anyway" (focused-but-not-default), "Cancel" (default),
    "Edit and Paste" (opens a text edit pre-filled with the content).
  - Escape and Enter shortcuts: Escape = Cancel, Ctrl+Enter = Paste Anyway.
- When dismissed via Paste Anyway, the original (or edited) payload is
  sent to the PTY through the same `InputEvent::Key` path that bypasses
  the guard.

**Verification:** Manual visual test. Snapshot test of the analyzer
classifications.

#### 77.4 — Wire into paste handling

**Scope:** `freminal/src/gui/terminal/input.rs` (line 210 `KeyAction::Paste`
and line 1191 `Event::Paste`).

- Before sending the paste payload, call `paste_guard::analyze`.
- If result is `Safe`, send as today.
- Otherwise, show the preview dialog (set a `pending_paste_dialog` field on
  the GUI state). Suspend further input until resolved.
- When the dialog resolves with "Paste Anyway" or "Edit and Paste", send
  the resolved payload through the existing send path.
- `KeyAction::PasteUnsafe` (new variant, see 77.5) skips the analyzer
  entirely.

**Verification:** Integration test: paste multi-line text → dialog appears
→ confirm → PTY receives bytes.

#### 77.5 — KeyAction::PasteUnsafe

**Scope:** `freminal-common/src/keybindings.rs`, dispatch.

- Add per the keybinding convention. Default binding `Ctrl+Shift+V`
  (note: this conflicts with existing `Paste` binding on some platforms;
  resolve by setting a sensible default and documenting).
- Document the action as "Paste without confirmation".

**Verification:** Round-trip keybinding test.

#### 77.6 — Settings UI

**Scope:** Existing Security settings tab (`freminal/src/gui/settings.rs`).

- Add a Paste Guard section:
  - Master toggle.
  - Multi-line trigger toggle.
  - Control-character trigger toggle.
  - Pattern trigger toggle.
  - Editable list of patterns with add/remove buttons and regex
    validation.
  - "Test Paste" button: opens the preview dialog with sample content.

**Verification:** Round-trip persistence. Pattern editor accepts/rejects
malformed regex correctly.

### 77 Open Questions Resolved

All resolved.

### 77 Benchmarks

`paste_guard::analyze` runs in the GUI thread on a paste event. For a 1MB
paste with all triggers and 20 patterns, it must complete in < 50ms.
Add a benchmark in a new `freminal/benches/paste_guard_bench.rs`.

---

## Task 94 — Tab Title Precedence

### 94 Summary

The current behavior (committed in 71.1) is `osc_wins`: OSC 0/1/2 clears
`custom_name` on every assertion. This is the wrong default. v0.9.0 changes
the default to `prefix` with format `"{custom}: {osc}"`. The behavior is
configurable.

### 94 Decisions (fixed)

- **Default policy:** `prefix`, format `"{custom}: {osc}"`.
- **Window title:** Follows the same policy as the active tab. (If user
  prefers a separate policy in the future, that's a v0.10.0 question.)
- **Empty rename:** Submitting an empty name clears `custom_name`,
  reverting to pure OSC behavior for that tab.

### 94 Subtasks

#### 94.1 — Config schema

**Scope:** `freminal-common/src/config.rs`, `config_example.toml`.

- Add `[tab_title]` section:

  ```toml
  [tab_title]
  # Precedence policy when a tab has both a custom name and an OSC title:
  #   "prefix"      — show "{custom}: {osc}"            (default)
  #   "suffix"      — show "{osc}: {custom}"
  #   "custom_wins" — show custom only
  #   "osc_wins"    — show osc only; OSC events clear custom_name
  policy = "prefix"

  # Separator used in `prefix` and `suffix` policies.  Default ": ".
  separator = ": "
  ```

- Add `TabTitleConfig` struct.
- Add `TabTitlePolicy` enum: `Prefix`, `Suffix`, `CustomWins`, `OscWins`.

**Verification:** Round-trip TOML.

#### 94.2 — `Tab::display_name` honors policy

**Scope:** `freminal/src/gui/tabs.rs`.

- `display_name(&self, policy: TabTitlePolicy, separator: &str) -> Cow<str>`.
- Signature change: callers must pass the config — find and update them all
  (`menu.rs`, etc.).
- Logic:
  - `Prefix`: if both custom and osc exist, return `"{custom}{separator}{osc}"`.
    If only custom, return custom. If only osc, return osc. Fall back to
    `"Terminal N"`.
  - `Suffix`: mirror of Prefix.
  - `CustomWins`: custom if present, else osc, else fallback.
  - `OscWins`: osc if present, else custom, else fallback.

**Verification:** Unit tests for each policy with every (custom, osc)
combination.

#### 94.3 — Remove the OSC-clears-custom behavior

**Scope:** `freminal/src/gui/app_impl.rs:868-870`.

- Delete the block that clears `tab.custom_name` when the shell asserts an
  OSC title — except when `policy == OscWins`.
- The OSC title still lives in the snapshot; the policy decides what gets
  rendered.

**Verification:** Integration test: set a custom name, emit OSC 0 with a
new title, verify under each policy:

- `Prefix`: tab shows `"{custom}: {osc}"`.
- `Suffix`: tab shows `"{osc}: {custom}"`.
- `CustomWins`: tab shows custom.
- `OscWins`: tab shows osc (and custom_name is cleared).

#### 94.4 — Window title follows policy

**Scope:** `freminal/src/gui/app_impl.rs` (window title setter).

- The current code likely sets the window title from the active tab. Pass
  the config through and use `display_name(policy, separator)` for the
  window title too.

**Verification:** Manual test under each policy.

#### 94.5 — Right-click "Clear Custom Name"

**Scope:** `freminal/src/gui/menu.rs` (tab context menu).

- Add a context-menu entry "Clear Custom Name", visible only when the tab
  has a `custom_name`.
- Action: set `tab.custom_name = None`.

**Verification:** Manual UI test.

#### 94.6 — Settings UI

**Scope:** Existing Tabs settings tab.

- Add a Title Policy dropdown (Prefix / Suffix / Custom Wins / OSC Wins).
- A separator text field (shown when policy is Prefix or Suffix).
- A live preview line showing `"my-rename: ~/projects"` with the current
  policy applied.

**Verification:** Round-trip persistence; live preview updates.

#### 94.7 — Docs

**Scope:** `config_example.toml`, README (if it documents tab behavior).

- Document the new `[tab_title]` section.

**Verification:** TOML compiles.

### 94 Open Questions Resolved

All resolved.

### 94 Benchmarks

None mandated.

---

## Task 95 — Persist Custom Tab Names in Layouts

### 95 Summary

`Tab::custom_name` is currently not saved into layouts or the
last-session file. Add `LayoutTab::custom_name` as a new field distinct
from the existing `LayoutTab::title` (which now means "author-supplied
initial title for seeding new tabs"). Round-trip through save and load.

### 95 Decisions (fixed)

- **Schema:** Add `LayoutTab::custom_name: Option<String>` distinct from
  `LayoutTab::title`. Both optional. Backward compatible (old files load
  with `custom_name = None`).
- **Load precedence:** If `custom_name` is present, populate
  `Tab::custom_name`. Otherwise, if `title` is present, use it as the
  initial OSC title (i.e. seed the tab's "shell-asserted" title). If
  neither, defaults apply.
- **Variable substitution:** No, custom_name strings are literal (no
  `$1`/`${name}` expansion).

### 95 Subtasks

#### 95.1 — Schema field

**Scope:** `freminal-common/src/layout.rs`.

- Add `pub custom_name: Option<String>` to `LayoutTab`, with
  `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- Also add it to the resolved variant `ResolvedTab` (around line 700).

**Verification:** Round-trip serialization test with and without
custom_name set.

#### 95.2 — Save path writes custom_name

**Scope:** `freminal/src/gui/layout_ops.rs:349` (the `save_layout` helper).

- Replace `title: None` with:

  ```rust
  title: None,                    // author seed only; not used at save time
  custom_name: tab.custom_name.clone(),
  ```

**Verification:** Save → load → verify `Tab::custom_name` is preserved.
Existing layout TOML files (in `Documents/LAYOUT_FORMAT.md` examples)
must continue to load.

#### 95.3 — Load path populates `Tab::custom_name`

**Scope:** `freminal/src/gui/layout_ops.rs` (the `apply_layout` /
`build_tabs_for_window` helpers).

- After creating each `Tab`, set `tab.custom_name = layout_tab.custom_name.clone()`.
- If `layout_tab.title` is present and `custom_name` is not, set the
  initial display name via whatever mechanism `LayoutTab::title` uses today
  (treat it as a one-shot OSC seed — find where the existing `title` field
  is consumed and preserve that behavior).

**Verification:** Load a layout with `custom_name` set, verify the loaded
`Tab` has the expected `custom_name`.

#### 95.4 — Last-session round-trip

**Scope:** `freminal/src/gui/session.rs`.

- The `auto_save_session` path already goes through `save_layout`, so 95.2
  covers it. Verify there are no other places `last_session.toml` is
  written.
- Add a test: create a window with renamed tabs, call `auto_save_session`,
  re-read the file, verify the custom names round-trip.

**Verification:** Round-trip test.

#### 95.5 — Docs

**Scope:** `Documents/LAYOUT_FORMAT.md`.

- Add the new `custom_name` row to the `[[windows.tabs]]` schema table.
- Explain the distinction: `title` = author seed for the OSC title;
  `custom_name` = persisted user rename (saved by freminal).
- Update "Last updated" line.

**Verification:** Doc example loads cleanly with the layout parser.

### 95 Open Questions Resolved

All resolved.

### 95 Benchmarks

None mandated.

---

## Tangential Features Approved for v0.9.0

The following features were identified during planning audit as small,
high-value adds tangent to the main v0.9.0 work. They are scoped as
discrete subtasks inside the host task indicated, not as new top-level
tasks.

### T1 — OSC 8 Hyperlink Action Menu (lands in Task 72)

**Scope:** ~1 day. `freminal/src/gui/mouse.rs`, context-menu rendering.

- OSC 8 hyperlink hover already works (URL displayed in status / tooltip).
- Add Ctrl+click on a hyperlinked region to open in the default browser
  (use `open` crate, already in dep tree per the URL-hover work).
- Add right-click on a hyperlinked region to show a context menu: "Open
  URL", "Copy URL". (Replaces or augments the existing right-click menu
  when the hover is over a hyperlink.)
- Land as **Task 72.14** (separate commit, even though it's unrelated to
  command blocks topically — it fits the same UX-polish theme and the
  branch is open).

### T2 — Command Duration Display (already in Task 72.12)

Covered by 72.12. No separate task.

### T3 — Quick Command History Palette (lands in Task 72)

**Scope:** ~2 days. New module `freminal/src/gui/command_history.rs`.

- A fuzzy-searchable palette over `pane.recent_commands` (from 72.9).
- New `KeyAction::ShowCommandHistory`, default `Ctrl+R`.
- Egui modal with:
  - Text input at top (fuzzy filter via `nucleo-matcher` or simple
    case-insensitive substring; whichever is already in the dep tree —
    check `Cargo.lock`).
  - List of recent commands with timestamp, exit code icon, command
    preview.
  - Enter on a selection: send the command text as keyboard input to the
    current pane (does **not** auto-execute — user reviews and presses
    Enter themselves).
- Land as **Task 72.15**.

### T4 — Bell on Command Completion (folded into Task 76.5)

Covered by 76.5. No separate task.

### T5 — Per-Pane Environment Indicator Badge (lands in Task 75)

**Scope:** ~half a day. `freminal/src/gui/renderer/` (pane chrome).

- When a pane was spawned with non-empty `extra_env` (from a layout), show
  a small badge in the pane's title bar (or top-right corner if no title
  bar). Tooltip on hover lists the overridden env vars.
- Land as **Task 75.3**.

---

## Cross-Cutting Concerns

### Config Schema Evolution

v0.9.0 adds five new config sections / extends one:

| Section               | Task(s) | Default state                       |
| --------------------- | ------- | ----------------------------------- |
| `[shell_integration]` | 72      | On (auto_install + term_program)    |
| `[command_blocks]`    | 72, 73  | On (gutter visible, blocks enabled) |
| `[tab_title]`         | 94      | `policy = "prefix"`                 |
| `[notifications]`     | 76      | Off (`enabled = false`)             |
| `[paste_guard]`       | 77      | On (multi-line confirmation)        |
| `[bell]` (extended)   | 76      | `on_command_finished = false`       |

All new fields use `#[serde(default)]` so old config files load unchanged.
The deployment flake (Task 4) home-manager module must be extended to
mirror the new schema — included in each task's subtasks where applicable.

### Snapshot Schema Evolution

v0.9.0 adds `command_blocks: Arc<[CommandBlock]>` to `TerminalSnapshot`.
FREC v2 captures the OSC 133 byte stream, not the derived snapshot, so no
FREC format change is required.

### Keybinding Additions

| KeyAction                   | Default             | Task  |
| --------------------------- | ------------------- | ----- |
| `ToggleFoldAtCursor`        | `Ctrl+Shift+F`      | 72.10 |
| `FoldAll`                   | (none)              | 72.10 |
| `UnfoldAll`                 | `Ctrl+Shift+U`      | 72.10 |
| `CopyLastCommandOutput`     | `Ctrl+Shift+Y`      | 72.11 |
| `CopyCommandOutputAtCursor` | (none, right-click) | 72.11 |
| `ShowCommandHistory`        | `Ctrl+R`            | 72.15 |
| `ToggleBroadcastInput`      | `Ctrl+Shift+I`      | 74.2  |
| `PasteUnsafe`               | `Ctrl+Shift+V`      | 77.5  |

All follow the AGENTS.md keybinding convention (4-step process: enum,
default binding, dispatch, config_example.toml).

### Documentation Updates

The following Documents/ files must be updated by the indicated tasks:

| File                              | Tasks that touch it |
| --------------------------------- | ------------------- |
| `ESCAPE_SEQUENCE_COVERAGE.md`     | 72.13, 76.8         |
| `ESCAPE_SEQUENCE_GAPS.md`         | 72.13, 76.8         |
| `LAYOUT_FORMAT.md`                | 75.2, 95.5          |
| `MASTER_PLAN.md`                  | After each task     |
| `PLAN_VERSION_090.md` (this file) | After each subtask  |

### Verification Suite (per AGENTS.md)

Every subtask completion requires:

1. `cargo test --all` passing.
2. `cargo clippy --all-targets --all-features -- -D warnings` clean.
3. `cargo-machete` clean.
4. Relevant benchmarks captured before/after with regression < 15%.
5. Plan document updated with completion status and benchmark numbers.

---

## Out of Scope for v0.9.0

The following ideas surfaced during planning but are explicitly deferred:

- **OSC 99 (kitty notifications)** — defer to v0.10.0 or v0.12.0
  (Completeness) alongside other kitty-protocol additions.
- **Layout-wide / window-wide `[layout.env]`** — defer to v0.10.0 with
  Profiles (Task 78).
- **Theme / font / profile binding per layout** — defer to v0.10.0 with
  Profiles.
- **Cross-tab and cross-window broadcast** — defer; explicit-pane-selection
  is a v0.10.0 design question.
- **Per-tab notification filters** — defer to v0.10.0 with Profiles
  (filters fit naturally as a profile attribute).
- **Tmux-style command history search across all panes** — defer; v0.9.0
  ships per-pane history only (T3).
- **Fold persistence across sessions** — folds are view-state only;
  reloading a layout does not restore fold state.
- **Notification batching** (collapse five "command finished" notifications
  into one) — defer to v0.10.0.
- **Scripting hooks on command finished** — defer to v0.11.0 (Task 84
  scripting layer).

---

## Activation Checklist

When v0.9.0 is activated (after v0.8.0 merges), follow this order:

1. Read this entire document plus the v0.8.0 close-out notes in
   `MASTER_PLAN.md`.
2. Branch from `main` to `task-72/osc-133-command-blocks`.
3. Execute Task 72 subtasks 72.1 → 72.6, then **72.16 (cleanup)**, then
   72.7 → 72.15, one commit each. Pause after each subtask for user
   confirmation per the Multi-Step Task Protocol in `agents.md`. The
   72.16 cleanup section accumulates bugs surfaced during earlier
   subtasks; revisit the schedule if a 72.16 entry blocks something
   earlier than 72.7.
4. After Task 72 merges, branch `task-73/command-gutters` and repeat.
5. Continue through Tasks 94, 95, 76, 77, 74, 75 in that order.
6. After all eight tasks merge, update `MASTER_PLAN.md` status table and
   release v0.9.0.

---

## Design Decisions (provisional; revisit only with explicit user approval)

- **OSC 133 is the anchor.** Tasks 73 and 76 both depend on it; Task 72 is
  therefore the keystone of v0.9.0 and lands first.
- **No scripting in v0.9.0.** Scripting (Lua/WASM) is deferred to v0.11.0
  Task 84.
- **No remote features in v0.9.0.** SSH and remote mux remain deferred to
  v0.11.0.
- **No profile concept in v0.9.0.** Profiles arrive in v0.10.0 Task 78.
- **`notify-rust` is added in v0.9.0.** This is the first runtime
  dependency added since v0.8.0; agents must update `cargo deny` allowlists
  if needed.
- **Default policies are conservative.** Notifications opt-in
  (`enabled = false`); paste guard opt-out (`enabled = true`). The
  rationale: notifications can be noisy and platform-dependent (annoying
  if surprising); paste guards prevent real foot-guns and are easy to
  bypass with `PasteUnsafe` once.
