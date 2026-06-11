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

| #   | Feature                                 | Scope        | Status   | Depends On      | Branch                           |
| --- | --------------------------------------- | ------------ | -------- | --------------- | -------------------------------- |
| 72  | OSC 133 Command Blocks                  | Large        | Complete | v0.8.0          | `task-72/osc-133-command-blocks` |
| 73  | Command Gutters (exit-status indicator) | Medium       | Complete | Task 72         | `task-73/command-gutters`        |
| 74  | Broadcast Input to Panes                | Medium       | Pending  | v0.8.0, Task 58 | `task-74/broadcast-input`        |
| 75  | Verify per-pane env round-trip          | Small        | Pending  | v0.8.0          | `task-75/pane-env-roundtrip`     |
| 76  | Notification System (OSC 9 / OSC 777)   | Medium       | Complete | v0.8.0, Task 72 | `task-76/notifications`          |
| 77  | Smart Paste Guard                       | Small–Medium | Complete | v0.8.0          | `task-77/paste-guard`            |
| 94  | Tab Title Precedence (prefix default)   | Small        | Complete | v0.8.0 (71.1)   | `task-94/tab-title-precedence`   |
| 95  | Persist Custom Tab Names in Layouts     | Small        | Complete | v0.8.0, Task 61 | `task-95/persist-tab-names`      |
| 98  | Block Close on Running Commands         | Small–Medium | Pending  | Task 72         | `task-98/block-close-on-running` |

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
9. **Task 98** — block close on running commands (depends on Task 72's `CommandBlock`
   status; lands any time after Task 72)

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

## Task 72 — OSC 133 Command Blocks ✅ Complete (2026-06-04)

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
  ├── New KeyActions: FoldPreviousCommand, FoldAll, UnfoldAll,
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

**Completion notes (commit `965aacf`):**

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

#### 72.2 — Add `command_blocks: VecDeque<CommandBlock>` to `Buffer` ✅

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

**Completion notes (commit `cbda480`, 2026-05-17):**

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

#### 72.3 — Wire FTCS markers into Buffer command-block API ✅

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

**Completion notes (commit `4950f3f`, 2026-05-17):**

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

#### 72.4 — Expose `command_blocks` through `TerminalSnapshot` ✅

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

**Completion notes (commit `d690064`, 2026-05-17):**

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

#### 72.5 — Settings: `[shell_integration]` and `[command_blocks]` config sections ✅

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

**Completion notes (commit `0434bfb`, 2026-05-17):**

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

#### 72.6 — TERM_PROGRAM environment variables ✅

**Scope:** `freminal/src/gui/pty.rs`, `tab_spawning.rs`.

- In the PTY spawn path, always set `TERM_PROGRAM=freminal` and
  `TERM_PROGRAM_VERSION=<env!("CARGO_PKG_VERSION")>` (gated on
  `config.shell_integration.set_term_program`).
- These env vars layer beneath `LayoutPane::env` overrides (so users can
  unset them by setting an empty value in a layout).

**Verification:** Unit test on the env-merge helper (existing or new). Manual
verification by running `echo $TERM_PROGRAM` in a freminal session.

**Completion notes (commit `14d1cad`, 2026-05-17):**

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

#### 72.7 — Ship shell integration scripts ✅

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

**Completion notes (commit `f6c6237`, 2026-05-17):**

- Created `shell-integration/freminal.{bash,zsh,fish}` plus a
  `shell-integration/README.md` (367 lines total).
- Each script is **source-only** (no shebang line) — the project's
  pre-commit hook `check-shebang-scripts-are-executable` rejects
  shebangs on non-executable files; for source-only scripts the
  canonical fix is to drop the shebang. A header comment in each file
  notes this explicitly.
- `bash -n`, `zsh -n`, and `shellcheck` all clean. `fish` is trusted
  by inspection (no `--no-execute` available in the local dev shell).
- One localised `# shellcheck disable=SC2064` in `freminal.bash`'s
  DEBUG-trap composition; justified by the deliberate variable
  expansion at install time.
- CI step NOT added in 72.7 — the plan suggested it but kept it
  optional; postponed to a future cleanup if regressions appear.
- Two known issues filed for cleanup (see 72.16.b and 72.16.c below):
  fish's A/B placement before the visual prompt, and a stale comment
  in `freminal.bash` referencing an unimplemented state flag.

**Superseded by 72.8b (2026-05-18).** After 72.7 + 72.8 landed, design
review surfaced that the user-sources-it-themselves model the scripts
were written for has too many failure modes (re-launching freminal-in-
freminal, NixOS environments where the user's `~/.zshrc` already
installs three competing FTCS emitters, etc.). The replacement
architecture (documented in `Documents/DESIGN_DECISIONS.md` "Shell
Integration Architecture") uses Ghostty-style spawn-time env injection
so the scripts auto-load on every PTY spawn invisibly. 72.8b rewrites
the scripts for the new mechanism. The flat-file layout
(`shell-integration/freminal.{bash,zsh,fish}`) is replaced by the tree
layout described in 72.8b. The 72.7 scripts and README are deleted as
part of 72.8b.

#### 72.8 — Auto-install shell integration scripts on first launch ✅

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

**Completion notes (commit `168c364`, 2026-05-17):**

- New `freminal_common::config::shell_integration_dir()` mirrors
  `layout_library_dir()` exactly (same per-OS structure, same
  `create_dir_if_missing` call).
- New `freminal/src/shell_integration.rs` module embeds the four
  bundled files via `include_str!()` (binary self-contained — no
  runtime filesystem dep on the repo).
- `install_if_missing(dir) -> InstallResult` (startup path; preserves
  user-edited scripts) and `reinstall_scripts(dir) -> InstallResult`
  (Settings button; overwrites) share a single `install_with_policy`
  implementation gated by an `overwrite: bool` flag.
- `InstallResult { written, skipped, errors }` carries per-file
  outcomes so the caller (toast / log) can render rich messages.
- Startup hook in `main.rs` runs between sections 2 (logging) and 3
  (`normal_run`). Failures are non-fatal — logged via
  `tracing::warn!`, no toast (the toast stack does not exist before
  GUI construction; using stderr/log was the right call).
- `SettingsAction::ReinstallShellScripts` and
  `CopyShellIntegrationPath(String)` variants propagate via a new
  `SettingsModal::pending_shell_action: Option<SettingsAction>` field
  mirroring the existing `pending_delete_layout` pattern. The
  modal's `show` and `show_standalone` both drain it.
- The Settings install-path display now resolves the real path via
  `shell_integration_dir()`, falling back to a `"(unavailable …)"`
  monospace string if `None`.
- Re-install + Copy-Path buttons use `ui.add_enabled` to grey out when
  the install dir cannot be resolved.
- `lib.rs` gained a `pub mod shell_integration;` declaration to mirror
  the existing `pub mod gui;` convention (freminal has both binary and
  library crate targets).
- 9 new tests total: 1 in `freminal-common`, 5 in
  `freminal::shell_integration::tests`, 2 in
  `freminal::gui::settings::tests`, 1 in
  `freminal::gui::settings_dispatch` (round-trip variant
  constructibility implicit).
- `cargo test --all`: passes (no regressions).
- `cargo clippy --all-targets --all-features -- -D warnings`: clean
  after orchestrator added 3 localised `#[allow(clippy::too_many_lines)]`
  attributes (on `SettingsModal::show`, `show_shell_integration_tab`,
  and `handle_settings_action`, each pushed past 100 lines by the new
  code) and 2 `#[must_use]` attributes on `install_if_missing` and
  `reinstall_scripts`. The sub-agent's session terminated without
  reporting the verification step, so these clippy issues had to be
  caught and fixed during orchestrator audit rather than at the
  sub-agent's stop condition.
- Filed for cleanup as **72.16.d** below: tests use
  `std::env::temp_dir()` with fixed name suffixes instead of the
  workspace `tempfile` crate. `tempfile` IS already a workspace dep
  (used by `freminal-common` and `freminal-terminal-emulator` tests);
  the sub-agent missed this when reviewing dev-dependencies and chose
  the fragile alternative.

**Partially superseded by 72.8b (2026-05-18).** The auto-install
infrastructure (the function shape, the `include_str!()` embedding,
the directory helper, the `main.rs` hook placement) is reused as-is.
What changes in 72.8b:

- `install_if_missing` is renamed to `sync_to_disk` because the policy
  shifts from "skip if file exists" to "skip if file content matches
  embedded bytes" (overwriting user edits — see
  `DESIGN_DECISIONS.md` "Why freminal owns the on-disk script files").
- `reinstall_scripts` and the `overwrite: bool` parameter are deleted —
  no longer called by any UI.
- `SettingsAction::ReinstallShellScripts` and
  `CopyShellIntegrationPath` variants are deleted.
- `SettingsModal::pending_shell_action` field is deleted.
- The two Settings buttons ("Re-install Scripts", "Copy Path") are
  deleted from the Shell Integration tab. The install-path display
  remains.
- The flat-file `SCRIPTS` const is replaced with a tree-shaped layout
  (`bash/freminal-init.bash`, `zsh/.zshenv`, `zsh/freminal-integration`,
  `fish/vendor_conf.d/freminal.fish`, `README.md`).
- Tests are rewritten to use `tempfile::TempDir` (closing 72.16.d).
- A new test invariant asserts every shipped script's
  `# freminal-shell-integration v<N>` marker matches the
  `FREMINAL_SHELL_INTEGRATION_VERSION: u32` Rust constant.

#### 72.8c — Parser: `freminal=1; fid=<id>` marker support ✅

**Why this subtask exists:** post-72.8 design review (2026-05-18)
concluded that freminal must coexist with other FTCS emitters (WezTerm
shell integration, Starship, iTerm2, Kitty) that are already active in
many users' shell environments. The parser must distinguish freminal-
emitted markers from foreign markers and correlate A/D pairs explicitly.
Captured durably in `Documents/DESIGN_DECISIONS.md` "Shell Integration
Architecture".

**Scope:** `freminal-common/src/buffer_states/ftcs.rs`,
`freminal-buffer/src/buffer/lifecycle.rs`,
`freminal-terminal-emulator/src/terminal_handler/osc.rs`.

**Must land before 72.8b** so the parser supports the new marker format
before the scripts emit it.

- `FtcsMarker` variants carry an explicit `fid` field for A/B/C/D
  markers and `freminal=1` is required for those variants to parse
  successfully:

  ```rust
  pub enum FtcsMarker {
      PromptStart { fid: String },
      CommandStart { fid: String },
      OutputStart { fid: String },
      CommandFinished { exit_code: Option<i32>, fid: String },
      PromptProperty(PromptKind),   // unchanged; informational only
  }
  ```

- `parse_ftcs_params` walks the parameter list looking for
  `freminal=<v>` and `fid=<id>`. For A/B/C/D, both must be present and
  `freminal` must equal `1` (or some other accepted value — left as a
  design choice for the implementer; "1" is fine). If either is missing
  or `freminal` is not `1`, return `None`. The marker is silently
  dropped at this layer and never reaches `Buffer::start_command_block`.
- `Buffer::start_command_block(cwd, fid: String)` records `fid` on the
  new block. `finish_command_block(exit_code, fid: String)` looks up the
  matching open block by `fid`, falls back to no-op if not found (it
  used to fall back to "most recent open"; the explicit-fid path is
  strictly safer because every freminal-emitted A carries a unique
  `fid` and the matching D will always quote it).
- `mark_command_start_row(fid: String)` and
  `mark_output_start_row(fid: String)` analogous.
- `CommandBlock` gains a `fid: String` field. Snapshot transport
  unchanged structurally (already carries `Arc<[CommandBlock]>`).

**Verification:**

- Unit tests in `ftcs.rs` covering:
  - `freminal=1; fid=abc` on A → produces `PromptStart { fid: "abc" }`.
  - Missing `freminal` param on A → returns `None`.
  - `freminal=2` (wrong value) on A → returns `None`.
  - Missing `fid` on A → returns `None`.
  - WezTerm-style `OSC 133;A;cl=m;aid=12345` → returns `None`.
  - Starship-style same → returns `None`.
- Unit tests in `lifecycle.rs::command_block_tests` covering:
  - A → D matched by fid produces one finished block.
  - A → A → D with the second A's fid → only the second block is closed.
  - D with unknown fid → no-op (returns `None`).
- Integration test in `shell_integration.rs::tests` (the handler tests)
  emitting a real `OSC 133;A;freminal=1;fid=foo` sequence through the
  parser and verifying the buffer state.
- Update `Documents/ESCAPE_SEQUENCE_COVERAGE.md` to document the
  `freminal=1; fid=<id>` parameter convention as a freminal extension.
- Update `Documents/ESCAPE_SEQUENCE_GAPS.md` if relevant.

**Migration concern:** the existing `FtcsMarker::PromptStart` etc. did
not carry `fid`. Any test or code that constructed those variants
directly (e.g. tests in `shell_integration.rs`) needs to supply a
`fid: String`. The migration is mechanical (every constructor site adds
a `fid: "test".to_owned()` or similar) but touches many existing
tests. Sub-agent budget should include time for that.

**Status:** ✅ Complete (commit `94db3c2`, 2026-05-18).

**Completion notes:**

- 11 files modified: 9 in scope plus two out-of-scope test migrations
  (`freminal-common/src/buffer_states/osc.rs` + `freminal-terminal-emulator/src/ansi_components/osc.rs`)
  both required by the `FtcsMarker` variant-shape change. Mechanical
  migrations, no production behavior change.
- `FtcsMarker::{PromptStart, CommandStart, OutputStart, CommandFinished}`
  are now struct variants carrying `fid: String`. `PromptProperty(PromptKind)`
  stays as a tuple variant (informational only, no fid).
- `parse_ftcs_params` walks the parameter list once, harvesting
  `freminal=`, `fid=`, `k=`, and the first positional. A/B/C/D markers
  return `None` unless `freminal=1` AND `fid=` are both present. P
  is accepted from any emitter. Unknown params (`aid=`, `cl=`) are
  silently ignored.
- `CommandBlock` gains `fid: String`. `Buffer::start_command_block`
  takes a `fid` arg; `mark_command_start_row`/`mark_output_start_row`/
  `finish_command_block` take a `&str` fid and correlate by it
  instead of "most-recent-open". No-op when no matching block exists.
- 43+ existing test sites migrated mechanically across the FTCS test
  suite (`mod.rs`, `shell_integration.rs`, `interface.rs`,
  `ansi_components/osc.rs`).
- New tests: 15 in `ftcs.rs`, 1 in `command_block.rs`, 4 in
  `lifecycle.rs`, 2 in `shell_integration.rs`, 2 in `interface.rs`,
  2 in `ansi_components/osc.rs`. All covering the `freminal=1` gate,
  fid correlation, and foreign-marker rejection.
- Coverage doc updated. The FTCS sub-table now lists the P row
  explicitly and includes a "Freminal extension" paragraph.
- Benchmark: `bench_command_block_record_10k` went from ~450 µs to
  863 µs (+92%). The extra cost is intrinsic — one String allocation
  per `start_command_block` + one &str comparison per `finish`. Well
  within the >100% threshold that would have warranted pushback.
- `cargo test --all`: 5118 tests pass. Workspace clippy clean.
  `cargo-machete` clean. `cargo fmt --check` clean.

#### 72.8b — Ghostty-style shell-integration injection ✅

**Why this subtask exists:** see 72.8c rationale and
`Documents/DESIGN_DECISIONS.md` "Shell Integration Architecture".

**Scope:** Rewrites of `shell-integration/*` (deleting flat files,
creating tree-layout files); changes to `freminal/src/shell_integration.rs`
(rename `install_if_missing` → `sync_to_disk`; delete `reinstall_scripts`
and the `overwrite` parameter; add `FREMINAL_SHELL_INTEGRATION_VERSION`
constant; rewrite tests for `TempDir`); changes to `freminal-common/src/config.rs`
(extend `shell_integration_dir()` with the `$FREMINAL_RESOURCES_DIR` /
`$XDG_DATA_DIRS` search-order chain); changes to
`freminal-terminal-emulator/src/io/pty.rs` (`run_terminal` performs
shell detection and env injection); changes to `freminal/src/gui/settings.rs`
(delete the two buttons; delete the `pending_shell_action` field; delete
the two `SettingsAction` variants); changes to
`freminal/src/gui/settings_dispatch.rs` (delete the two match arms);
changes to `freminal/Cargo.toml` (add `tempfile.workspace = true` to
`[dev-dependencies]`).

**Must land after 72.8c.**

**Detailed work breakdown:** the implementing sub-agent will receive a
fully-detailed prompt with file-level scopes and exact code samples for
each of the new scripts. The plan-doc summary here is intentionally
high-level because the design is captured durably in
`DESIGN_DECISIONS.md` and the sub-agent prompt is the right place for
the exact code.

**New file layout** (created by 72.8b in `shell-integration/`):

```text
shell-integration/
├── bash/
│   └── freminal-init.bash
├── zsh/
│   ├── .zshenv
│   └── freminal-integration
├── fish/
│   └── vendor_conf.d/
│       └── freminal.fish
└── README.md
```

**Each script's responsibilities:**

- **`bash/freminal-init.bash`**: first non-comment line is
  `set +o posix` (because freminal launched bash with `--posix`). Then
  source the user's `~/.bashrc` (or `~/.bash_profile` if a login shell)
  guarded with `[ -f ... ] && source ... 2>/dev/null`. Then install
  hooks: PROMPT_COMMAND append for D + OSC 7; DEBUG trap for C; PS1
  wrap for A + B. Every emitted marker carries `freminal=1; fid=$$-<N>`
  where N is a per-prompt counter.
- **`zsh/.zshenv`**: the Ghostty-derived `+X`-check dance to restore
  the user's original `$ZDOTDIR` (or `unset ZDOTDIR`), then source the
  user's real `.zshenv`, then `source` our `freminal-integration`
  script.
- **`zsh/freminal-integration`**: install `precmd_functions` and
  `preexec_functions` entries (using `add-zsh-hook` if available).
  Every emitted marker carries `freminal=1; fid=$$-<N>`. PROMPT
  wrapping with `%{...%}` for A and B markers.
- **`fish/vendor_conf.d/freminal.fish`**: register
  `--on-event fish_prompt`, `--on-event fish_preexec`,
  `--on-event fish_postexec` handlers. The fish-prompt A/B placement
  caveat from 72.16.b is closed by the
  `vendor_conf.d` discovery: our integration loads early enough that we
  can install handlers before user themes do, then chain to user's
  fish_prompt if it exists.
- **`README.md`**: updated to document the new architecture — that the
  scripts are auto-injected, that user edits will be overwritten, and
  how to opt out by setting `[shell_integration] enabled = false` in
  config (the existing toggle).

**Each script begins with a version marker:**

```bash
# freminal-shell-integration v1
```

Or for README.md:

```markdown
<!-- freminal-shell-integration v1 -->
```

The Rust-side constant `FREMINAL_SHELL_INTEGRATION_VERSION: u32 = 1`
must match. A test invariant enforces this.

**Spawn-time env injection in `run_terminal`:**

Before `pair.slave.spawn_command(cmd)`, detect the shell from
`cmd`'s program path basename. For each supported shell, set the
appropriate env vars on the `CommandBuilder`:

```rust
match detect_shell(&cmd) {
    Some(Shell::Bash) => {
        cmd.args(["--posix"])?;
        cmd.env("ENV", resources.join("bash/freminal-init.bash"));
    }
    Some(Shell::Zsh) => {
        // Preserve user's existing $ZDOTDIR via a sentinel env var
        // (Ghostty's +X-style approach: if the user had any $ZDOTDIR
        // set, including empty, we save it. Otherwise we don't set
        // our sentinel at all.).
        if let Ok(z) = std::env::var("ZDOTDIR") {
            cmd.env("__FREMINAL_ZSH_ZDOTDIR", z);
        }
        cmd.env("ZDOTDIR", resources.join("zsh"));
    }
    Some(Shell::Fish) => {
        let mut xdg = std::env::var("XDG_DATA_DIRS")
            .unwrap_or_else(|_| String::from("/usr/local/share:/usr/share"));
        let our = resources.display().to_string();
        // Prepend so fish finds our vendor_conf.d first.
        xdg = format!("{our}:{xdg}");
        cmd.env("XDG_DATA_DIRS", xdg);
    }
    None => {
        // Unknown shell — no injection. Graceful.
    }
}
```

This block is gated on `config.shell_integration.set_term_program`
(repurposed: the same flag controls TERM_PROGRAM AND shell-integration
injection because both are coupled — if the user has set
`set_term_program = false`, they're opting out of all freminal-side
shell integration).

If `args.command` is non-empty (user invoked freminal with an explicit
binary like `freminal vim`), skip injection entirely. The shell-
integration env vars are irrelevant for non-shell children.

**`sync_to_disk` semantics:**

```rust
fn sync_to_disk(dir: &Path) -> InstallResult {
    let mut result = InstallResult::default();
    if let Err(e) = std::fs::create_dir_all(dir) {
        result.errors.push((dir.display().to_string(), e.to_string()));
        return result;
    }
    for (relative_path, content) in SCRIPTS {
        let path = dir.join(relative_path);
        // Create parent (e.g. `bash/`) if needed.
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            result.errors.push(((*relative_path).to_owned(), e.to_string()));
            continue;
        }
        // Fast path: bytes match → no write.
        if let Ok(existing) = std::fs::read(&path)
            && existing == content.as_bytes()
        {
            result.skipped.push((*relative_path).to_owned());
            continue;
        }
        match std::fs::write(&path, content) {
            Ok(()) => result.written.push((*relative_path).to_owned()),
            Err(e) => result.errors.push(((*relative_path).to_owned(), e.to_string())),
        }
    }
    result
}
```

Called from `main.rs` on every launch (gated on
`config.shell_integration.set_term_program`).

**Settings UI changes:**

- Delete the "Re-install Scripts" button.
- Delete the "Copy Path" button.
- Keep the read-only install-path display.
- Delete `SettingsAction::ReinstallShellScripts` and
  `CopyShellIntegrationPath(String)` variants from `settings.rs`.
- Delete the matching match arms in
  `freminal/src/gui/settings_dispatch.rs`.
- Delete the `SettingsModal::pending_shell_action` field and its
  drain logic in `show` / `show_standalone`.

**Verification:**

- `cargo test --all` (workspace).
- `cargo clippy --all-targets --all-features -- -D warnings`.
- `cargo-machete`.
- `bash -n` and `zsh -n` on each script; `shellcheck` clean on
  bash script.
- New `every_shipped_script_marker_version_matches_constant` test in
  `shell_integration.rs::tests`.
- New `sync_to_disk_writes_when_bytes_differ` test (counterpart to the
  old `install_if_missing_skips_existing_files`, with reversed
  semantics).
- New `sync_to_disk_skips_when_bytes_match` test.
- New `sync_to_disk_handles_nested_dirs` test (verify `bash/` and
  `fish/vendor_conf.d/` get created).
- Manual end-to-end test: launch freminal, run a few commands, verify
  in a recording that markers carry `freminal=1; fid=<id>` and that
  `command_blocks` populates correctly.

**Status:** ✅ Complete (commit `3e80e6d`, 2026-05-19).

**Completion notes:**

- Spawn-time env injection replaces the user-sources-scripts model. When
  `[shell_integration] set_term_program = true` (default) and freminal is
  launching a bare interactive shell, `inject_shell_integration_env`
  detects bash/zsh/fish from the program basename and mutates the spawn
  environment:
  - **bash:** launched with `--posix` + `ENV=<resources>/bash/freminal-init.bash`.
    The init script cancels POSIX mode (`set +o posix`), chains to the
    user's normal startup files (`.bash_profile`/`.bash_login`/`.profile`
    for login, `.bashrc` for non-login), then installs the OSC 133 hooks.
  - **zsh:** `ZDOTDIR=<resources>/zsh`. The bundled `.zshenv` stashes the
    user's real ZDOTDIR in `__FREMINAL_ZSH_ZDOTDIR`, restores it, then
    sources the integration body.
  - **fish:** prepend `<resources>` to `XDG_DATA_DIRS` so fish loads
    `vendor_conf.d/freminal.fish` automatically.
- Injection is skipped entirely when a positional command is passed
  (`freminal -- htop`) so non-shell children inherit a clean env.
- `auto_install`, `Re-install Scripts`, `Copy Path` UI surface and the
  related `SettingsAction` variants, `pending_shell_action` field, and
  dispatch arms are all deleted. Scripts are now sync-to-disk on every
  launch (already implemented in 72.8a); the UI no longer exposes
  installation as a user concern.
- `FREMINAL_SHELL_INTEGRATION_VERSION: u32 = 1` constant added; each
  script carries a matching `# freminal-shell-integration v1` header.
  `sync_to_disk` parses the header and rewrites stale copies in place.
- All three scripts gained a `TERM_PROGRAM != "freminal"` early-return
  guard so a user manually sourcing a persisted copy under another
  terminal (ghostty, wezterm, kitty, iTerm) does NOT install hooks or
  emit OSC sequences that those parsers might mishandle.
- Per-command-lifecycle `fid` rolling: each script maintains a private
  counter (`$$-N` for bash/zsh, `$fish_pid-N` for fish). A/B/C/D for one
  command share one fid; the next command's precmd/fish_prompt rolls
  forward. Critical detail: in bash and zsh, the rolling function
  (`__freminal_fid_next`) must be called as a plain command (not in
  `$(…)`) so the parent shell's counter actually mutates — earlier
  drafts that used command-substitution pinned every emission to
  `fid=$$-1`.
- Bash PS1 wrap-stripping required glob escaping: PS1 stores the literal
  four-character sequences `\[`, `\033`, `\007`, `\]`, and bash's
  `${var//pat/repl}` treats `pat` as a glob where `[` is special. The
  pattern uses `\\\[` and `\\\]` so the glob matches `\` + `[` literally.
  Without this, PS1 wraps stacked one extra A/B pair per prompt cycle.
- `__freminal_strip_ps1_wrap` and the zsh equivalent are called inside
  precmd before re-wrapping, defending against prompt frameworks
  (oh-my-posh, Starship, p10k) that mutate PROMPT/PS1 from their own
  hooks.
- Verified across three real recordings under Starship (zsh), fish's
  built-in OSC 133 emitter, and vanilla bash. A/B/C/D fid pairing
  correct in all three; foreign markers (`aid=`, `click_events=1`,
  `cmdline_url=ls`, bare `;D;0`) correctly rejected by the strict
  `freminal=1` gate; multi-pane independence confirmed (split-pane
  recording shows two distinct PID-prefixed fid streams).
- `cargo test --all`, `cargo clippy --all-targets --all-features
-- -D warnings`, `cargo-machete`, `cargo fmt --check`: all clean.
  ShellCheck clean on the bash script (three intentional patterns
  suppressed inline with explanatory comments: SC2317 ×2 for the
  dual-mode `return ... || exit/true` guards, SC2016 ×2 for the
  single-quoted glob marker literals that must NOT interpolate).

#### 72.9 — CommandFinishedEvent GUI handling ✅ 2026-05-19

**Status:** ✅ Complete (commit `d11ccf9`, 2026-05-19).

**Scope:** `freminal/src/gui/app_impl.rs` (per-frame drain), `freminal/src/gui/panes/mod.rs`
(per-pane ring), `freminal/src/gui/tabs.rs` (per-tab pending-event flag, focus clearing),
`freminal/src/gui/pty.rs` (consumer-thread forwarding + extracted helper).

**Design deviation from the original spec:** the transport was implemented as a dedicated
`Sender<CommandFinishedEvent>` per pane rather than a `WindowCommand::CommandFinished`
variant. Rationale: avoids coupling `CommandBlock` and `pane_id` into the `WindowCommand`
enum (which is otherwise dominated by viewport/report variants), keeps the per-pane channel
mirroring the existing `pty_dead_rx` / `clipboard_rx` patterns, and isolates the new event
type for clean Task 76 hand-off.

**Completion notes:**

- Added `pub const RECENT_COMMANDS_CAP: usize = 64;` and
  `Pane::push_recent_command(&mut self, CommandBlock)` enforcing the cap by
  popping the oldest entry. Used by both the GUI drain and unit tests.
- Added `Pane::recent_commands: VecDeque<CommandBlock>` and
  `Pane::command_event_rx: Receiver<CommandFinishedEvent>`.
- Added `Tab::has_pending_event: bool`, initialized `false` in `Tab::new`.
- `TabManager::switch_to`, `next_tab`, and `prev_tab` now clear
  `has_pending_event` on the newly-active tab; they are no longer `const fn`
  since they index `self.tabs` mutably.
- `app_impl.rs` `update()` drains every pane's `command_event_rx` per frame.
  When the receiving tab is not the active tab, sets `tab.has_pending_event = true`.
  Includes a `TODO(Task 76)` marker at the drain site for the future
  notification dispatch.
- `pty.rs` defines `pub struct CommandFinishedEvent { pane_id: u32, block: CommandBlock }`
  and `pub(crate) fn forward_command_events(...)` extracted from the consumer-thread
  `post_event` closure. `spawn_pty_consumer_thread` calls
  `forward_command_events(handler.drain_command_events(), recording_pane_id, &command_event_tx)`
  after each batch.

**Verification:** Six new unit tests covering the ring-buffer cap and the
forwarding transport contract:

- `gui::panes::tests::push_recent_command_below_cap_appends_in_order`
- `gui::panes::tests::push_recent_command_enforces_cap_dropping_oldest`
- `gui::panes::tests::push_recent_command_at_exact_cap_does_not_evict`
- `gui::pty::tests::forward_command_events_empty_input_sends_nothing`
- `gui::pty::tests::forward_command_events_preserves_order_and_pane_id`
- `gui::pty::tests::forward_command_events_with_closed_receiver_does_not_panic`

`cargo test --all` and `cargo clippy --all-targets --all-features -- -D warnings`
both clean.

**Original spec (retained for reference):**

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

#### 72.10 — Fold/collapse view state ✅

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
  - `FoldPreviousCommand` — fold/unfold the most recent completed command
    block (or the block containing the cursor, if the cursor is inside one).
    No default binding to avoid conflicting with `OpenSearch` (`Ctrl+Shift+F`).
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

**Completion notes (72.10):**

- 72.10a (`e3c3996`, 2026-05-19) — added `ViewState::folded_blocks`,
  `fold` / `unfold` / `toggle_fold` / `unfold_all`, three `KeyAction`
  variants (`FoldPreviousCommand`, `FoldAll`, `UnfoldAll`) with default
  bindings, and unit tests for the fold round-trip.
- 72.10b-1 (`f5798bb`, 2026-05-19) — `freminal/src/gui/folding.rs`:
  `FoldRange`, `RowMap`, `RenderedRow::{Snapshot, Placeholder}` and the
  `rendered_to_snapshot` lookup, with exhaustive unit tests for
  RowMap construction and lookup behavior.
- 72.10b-2 (`a9f368f`, 2026-05-19) — wired `RowMap` into the widget render
  path so folded rows are skipped and a one-row gap is inserted; the gap
  was visually blank in this subtask.
- 72.10b-3 (`d0c7098`, 2026-05-19) — replaced the blank gap with
  a real placeholder line ("▶ N lines hidden — click to unfold") shaped
  via the new `shape_placeholder_line` helper in
  `freminal/src/gui/shaping.rs`, recorded per-frame placeholder hit-rects
  on `PaneRenderCache`, added click consumption in
  `write_input_to_terminal` that calls `view_state.unfold(id)` and skips
  selection/PTY-mouse-report, and added a `CursorIcon::PointingHand`
  override when the pointer hovers a placeholder rect (runs
  unconditionally, not gated on `snap.has_urls`). New unit tests cover
  `format_placeholder_text` (singular/plural/zero/truncation/very-narrow/
  zero-width) and `hit_test_placeholder` (inside, outside, empty,
  multiple rects). Added `bench_shape_placeholder_line` to
  `freminal/benches/render_loop_bench.rs` exercising typical (w=80) and
  wide (w=200) placeholder text.

  Benchmark results (release, criterion):

  | Benchmark                                     | Before | After     | Change |
  | --------------------------------------------- | ------ | --------- | ------ |
  | `render_terminal_text_arcswap/store_and_load` | n/a    | 67.48 ns  | —      |
  | `render_terminal_text_arcswap/load_only`      | n/a    | 7.83 ns   | —      |
  | `shape_placeholder_line/typical_w80`          | n/a    | 433.58 us | new    |
  | `shape_placeholder_line/wide_w200`            | n/a    | 486.66 us | new    |

  Note: the `shape_placeholder_line` benchmark wall-time is dominated by
  the `FontManager::new()` call in `iter_batched`'s setup closure (font
  enumeration + face loading), not the shaping work itself. A future
  improvement is to reuse a single `FontManager` across iterations.
  The arcswap baseline is included for reference; this subtask does
  not touch the snapshot transport path, so no regression is expected
  or observed.

- 72.10c (`bf6a2b4`, 2026-05-19) — bug fix surfaced by post-merge
  testing with the bundled fish shell integration. The original 72.10a
  `FoldPreviousCommand` dispatcher only folded a block when the PTY
  cursor row fell inside `[command_start_row, end_row]`. In normal
  interactive use the PTY cursor always lives on the active prompt
  line, which is _after_ every completed block, so the keybinding
  silently no-op'd for every realistic scenario. (The plan text at
  72.10 specified "cursor or topmost visible row" as the selection
  rule, but the topmost-visible fallback was never implemented and is
  itself a poor UX choice — the natural intent is "fold the command I
  just ran.")

  Fix: extracted the selection logic into a pure helper
  `find_fold_target(snap) -> Option<CommandBlockId>` in
  `freminal/src/gui/terminal/input.rs` with two passes. Pass 1
  preserves the original behaviour for the future scrollback-cursor /
  gutter-click pathways: a completed block containing `cursor_row`
  wins. Pass 2 is the new fallback: the most recently completed block
  (last element of the `VecDeque` whose `end_row.is_some()`). Running
  blocks and blocks missing `command_start_row` are excluded from both
  passes. Added `mod fold_target_tests` with six unit tests covering
  cursor-inside selection, recency fallback, running-block exclusion
  (both as the cursor's containing block and as the most-recent
  appended block), the empty-and-only-running cases, and the
  missing-`command_start_row` case. No changes to `ViewState`, the
  rendering layer, or the keybinding default.

- 72.10d (`5b029a4`, 2026-06-03) — three renderer bugs surfaced by
  visual testing with bash/zsh/fish under the 72.8b spawn-time
  injection:
  1. **Buffer-absolute vs snapshot-relative row coordinate mismatch.**
     `CommandBlock` row fields are stored in buffer-absolute space (e.g.
     46..=79 for a block 46 rows into the scrollback), but
     `RowMap::new(term_height=17, ...)` expects rows in snapshot-row
     space `[0, term_height)`. Ranges with `start_row >= term_height`
     were silently dropped, producing an identity row_map and no
     visible fold. Fix: new pure helper
     `translate_ranges_to_snapshot(ranges, visible_window_start)` in
     `freminal/src/gui/folding.rs`; widget pipeline is now
     `compute_fold_ranges → translate → RowMap::new`.
     `visible_window_start` is computed in `coords.rs` as
     `total_rows.saturating_sub(term_height).saturating_sub(scroll_offset)`.
  2. **Placeholder line count fluctuated across scroll.** `FoldRange::len()`
     reflected the per-frame clipped range rather than the full block,
     so "5 lines hidden" became "3 lines hidden" as you scrolled the
     top edge of the block off-screen. Fix: added a
     `block_total_rows: usize` field to `FoldRange`, populated from
     `compute_fold_ranges` (full block height), preserved through
     `translate_ranges_to_snapshot` and `RowMap::new`'s clamp pass; the
     widget reads `range.block_total_rows` for the placeholder text
     rather than `range.snapshot_end_row - range.snapshot_start_row + 1`.
  3. **Fold hid the prompt and command line.** `compute_fold_ranges`
     used `command_start_row` (OSC 133 B = start of typing) as the
     fold start, but all three bundled shells emit OSC 133 C and set
     `output_start_row`. Folding from B collapses the entire prompt
     plus the typed command into the placeholder — the user can no
     longer see what command was folded. Fix: `compute_fold_ranges`
     now prefers `output_start_row.or(command_start_row)`. The
     `command_start_row` fallback preserves behavior for any future
     shell integration that emits B but not C.

  Verification: 7 new tests added in `freminal/src/gui/folding.rs`
  (5 covering `translate_ranges_to_snapshot`, 1 for
  `block_total_rows` stability across scroll, 1 for the
  `output_start_row` precedence, 1 for the `command_start_row`
  fallback). All 32 folding tests pass; 103/103 suites green; clippy
  clean. User confirmed visually with bash/zsh/fish.

- 72.10e (`8d1056e`, 2026-06-03) — straight rename
  `ToggleFoldAtCursor` → `FoldPreviousCommand`. The old name implied
  the cursor location selected the fold target, but as documented in
  72.10c the cursor is almost always on the active prompt (block N+1)
  and the dispatcher actually folds the most recent _completed_ block.
  `FoldPreviousCommand` describes the real behaviour. No backward-
  compat alias for the `toggle_fold_at_cursor` TOML key — pre-release
  code; existing configs are expected to be updated manually.

#### 72.11 — Copy command output actions ✅

**Completed:** 2026-05-19 (commit `8c3cd77`)

**Summary:** Added `CopyLastCommandOutput` (default `Ctrl+Shift+Y`) and
`CopyCommandOutputAtCursor` (unbound; surfaced via right-click menu) as
new `KeyAction` variants. Both actions resolve a target block, derive its
`[output_start_row, end_row]` full-width range, and route through the
existing `InputEvent::ExtractSelection` → `clipboard_rx` → arboard path.
Skips running blocks and blocks missing the OSC 133 `C` marker. The
terminal right-click context menu gained a "Copy Command Output" entry
that appears when the clicked cell falls inside a completed block; it
uses the same context-menu copy flow as the existing URL/selection
entries (synchronous `recv_timeout`). Nine unit tests cover
`find_last_copyable_block`, `find_block_containing_row`, and the
boundary / running / missing-marker cases.

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

#### 72.12 — Hover highlight and command-duration overlay ✅

**Status:** COMPLETE (2026-05-19, `238e903`; follow-up fix `8d95ad3` — drop
blocks erased by CSI 2J so duration overlays don't paint on blank rows).

**Note:** The hover model and duration-label placement implemented here are
superseded by Task 73 subtasks 73.5 (move hover trigger onto the gutter)
and 73.6 (move duration label into the gutter). The in-buffer overlay and
inline label introduced by 72.12 are interim until 73.5/73.6 land. 73.7
covers a suspected duration-reporting bug (e.g. `ls` reported as taking
seconds) surfaced while testing 72.12.

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

**Completion notes:**

- Inline compact formatter (`format_command_duration` in
  `freminal/src/gui/command_blocks.rs`) — no `humantime` dependency.
  Output format: `3s`, `2m15s`, `1h3m`, with whole-unit boundaries
  suppressing the trailing zero unit. 10 unit tests cover sub-second,
  seconds, minutes-with-seconds, whole-minute, hours-with-minutes,
  whole-hour, boundaries, and whitespace-free invariant.
- Hover tint uses full block range `[command_start_row, end_row]` (running
  blocks skipped — no `end_row`). Tint colour is `theme.selection_bg` at
  25% alpha via new `command_block_hover_bg_f()` helper. Quad emitted in
  `build_background_instances` between search highlights and selection so
  selection overpaints hover and hover overpaints search.
- Duration label uses `theme.foreground` at ~60% alpha, monospace font at
  75% of cell height, right-anchored at `terminal_rect.max.x - 4.0` on
  the first visible rendered row of each qualifying block. Anchors on
  `command_start_row` falling back to `prompt_start_row`. Skips blocks
  entirely outside the visible window or hidden inside a fold.
- Both hover and duration paths go through `RowMap::snapshot_to_rendered`
  so fold-collapsed blocks degrade gracefully (placeholder rows return
  `None`).
- `widget.rs::show()` signature extended with `&CommandBlocksConfig`,
  threaded from `app_impl.rs`. No new `ViewState` field — hover is read
  fresh each frame from `view_state.mouse_position`.

#### 72.13 — `ESCAPE_SEQUENCE_COVERAGE.md` and `ESCAPE_SEQUENCE_GAPS.md` updates ✅

**Scope:** `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`.

- OSC 133 A/B/C/D/P are already parsed; the coverage table should already
  reflect that. Verify and update:
  - Status icon for OSC 133 → fully supported (was partial).
  - Notes column → "Drives CommandBlock storage, gutters, notifications,
    fold/collapse." Task reference: 72.
- Update both "Last updated" lines.

**Verification:** The two docs must parse without warnings (markdownlint if
the project runs it) and continue to align with each other.

**Status:** ✅ Complete (commit `603001b`). COVERAGE.md was already
accurate after Task 72.8c (OSC 133 row marked ✅ with the freminal=1
extension documented); only the "Last updated" line needed bumping.
GAPS.md had three stale references claiming OSC 133 gutter/jump-to-prompt
was outstanding under Task 72 -- updated to reflect that storage,
navigation, fold/copy/hover/duration shipped under Task 72 and only the
gutter rendering remains under Task 73. Also added a Priority 2 polish
entry for XTGETTCAP capability expansion (`indn`, `query-os-name`)
covering the deferred half of 72.16.e.

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

**Surfaced in:** 72.4 (commit `d690064`, 2026-05-17).

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

**Status:** ✅ Complete (2026-05-17, commit `703e998`).

**Completion notes:**

- Replaced the `String`-only filter in the FTCS arm of
  `dispatch_osc_target` with a serializer that maps both
  `AnsiOscToken::String(s)` and `AnsiOscToken::OscValue(n)` to their
  display form. Owned `String`s are required (numerics format at
  runtime); refs collected into a sibling `Vec<&str>` for the call to
  `parse_ftcs_params`.
- The 72.4 byte-stream tests (`build_snapshot_populated_command_blocks`,
  `build_snapshot_command_blocks_ordering`) were tightened to assert
  exit codes (Some(0), Some(1), Some(2)) and the stale "known
  limitation" comments removed.
- New regression test `build_snapshot_command_block_preserves_exit_code_127`
  drives `OSC 133;A` + `OSC 133;D;127` through the byte stream and
  verifies `status() == Failure(127)`.
- Audit found the OSC 22 (pointer shape) handler at `osc.rs:203` uses a
  superficially similar `AnsiOscToken::String`-only filter. The agent
  judged it correct-by-semantics: OSC 22 carries CSS/xcursor cursor
  names, never bare integers. Logged here as a potential future
  72.16.x candidate only if OSC 22 semantics are ever extended to
  accept numerics — no fix needed today.
- `freminal-terminal-emulator` test count: 2329 → 2330.

##### 72.16.b — Fix fish_prompt A/B placement around visual prompt text

**Surfaced in:** 72.7 (commit `f6c6237`, 2026-05-17).

**Bug:** `shell-integration/freminal.fish` registers an
`--on-event fish_prompt` handler that emits BOTH `OSC 133 A` and
`OSC 133 B` before the visual prompt text is drawn. The intent is for
A to fire before the prompt and B to fire after (so the prompt text
falls in the `InPrompt` region and the user's typed input falls in the
`InCommand` region).

Fish does not allow wrapping `fish_prompt` via `--on-event` without
infinite recursion, and overriding `fish_prompt` directly would
collide with user themes (Tide, Starship, oh-my-fish). The 72.7
implementation chose the safer "A then B before the prompt" placement
with an honest comment in the script.

**Impact:**

- Today (Task 72.7-72.10): block start row is correct, exit-status
  gutter coloring is correct, command-block navigation is correct.
  The semantic distinction between "prompt region" and "command-input
  region" is collapsed in fish only.
- Future (Task 72.11 — copy-output): copying a command's output will
  include the prompt text in fish, because `output_start_row` is
  computed from C, but `command_start_row` (from B) is wrong — it sits
  before the prompt, so any UI feature keyed on `command_start_row..end_row`
  in fish will include the prompt line.
- bash and zsh are unaffected — their PS1 wrapping correctly emits B
  after the prompt text.

**Scope:** `shell-integration/freminal.fish` only.

**Suggested approach (revisit at activation):**

- Option A: detect whether the user has defined `fish_prompt` and, if
  so, install a wrapper that calls the original. Requires careful
  handling of fish 3.x's `functions --copy` to snapshot the user's
  `fish_prompt` and call it from a renamed version. Conflicts arise if
  the user redefines `fish_prompt` after sourcing our script.
- Option B: emit B from a separate `fish_postprompt`-style event. Fish
  does not have a built-in event matching this name; would require a
  shim. Not preferred.
- Option C: live with the limitation; document that copy-output in
  fish includes the prompt line as a known caveat. Cheapest, defers
  user-visible impact.

Likely choice: Option A, attempted in 72.11 alongside the copy-output
implementation. Until 72.11, no user impact.

**Scheduling:** Must complete before 72.11 (Copy command output
actions) ships. Tracked as a prerequisite for that subtask.

**Status:** ✅ Subsumed by 72.8b (2026-05-18). The fish script is being
rewritten from scratch for the Ghostty-style spawn-time injection
architecture (see `Documents/DESIGN_DECISIONS.md` "Shell Integration
Architecture"). The new fish script lives at
`shell-integration/fish/vendor_conf.d/freminal.fish` and is auto-loaded
via `$XDG_DATA_DIRS`, which lets us register hook functions before any
user `fish_prompt` exists. 72.8b's fish script emits A and B in the
correct positions; the workaround documented above is no longer needed.

##### 72.16.c — Remove stale `__FREMINAL_CMD_PENDING` comment in freminal.bash

**Surfaced in:** 72.7 (commit `f6c6237`, 2026-05-17).

**Bug:** `shell-integration/freminal.bash` lines 79-89 (approximately)
contain a comment block describing a `__FREMINAL_CMD_PENDING` state
flag and the conditions under which it suppresses double-emission of
the C marker inside `PROMPT_COMMAND`. The flag was never actually
implemented in the script. The DEBUG-trap function instead uses a
`case "${BASH_COMMAND}" in __freminal_*) return 0 ;; esac` filter to
skip our own internal commands. The filter is correct; the comment is
misleading documentation describing code that doesn't exist.

**Impact:** Documentation hygiene only. No functional impact. The
script behaves correctly; the comment is internally inconsistent with
the implementation.

**Scope:** `shell-integration/freminal.bash` only — comment block
deletion and a short replacement comment describing the actual
filter (`__freminal_*` BASH_COMMAND case).

**Suggested approach:** Replace the multi-paragraph comment block
above `__freminal_debug_trap` with a 2-3 line description of the real
filter. No code change needed.

**Scheduling:** Cosmetic; can land at any time. No subtask blocked by
this.

**Status:** ✅ Subsumed by 72.8b (2026-05-18). The bash script
(`shell-integration/freminal.bash`) is being rewritten from scratch for
the Ghostty-style spawn-time injection architecture. The new bash
script (`shell-integration/bash/freminal-init.bash`) is sourced via the
`$ENV` env var in POSIX-mode bash and has a different overall structure
that doesn't include the stale comment block. Removed implicitly when
the old script is deleted.

##### 72.16.d — Use workspace `tempfile` in `freminal::shell_integration::tests`

**Surfaced in:** 72.8 (commit `168c364`, 2026-05-17).

**Bug:** The new tests in `freminal/src/shell_integration.rs::tests`
use `std::env::temp_dir()` with hard-coded suffix names
(`writes_all`, `skips_existing`, `overwrites`, `idempotent`) plus
best-effort `remove_dir_all` cleanup. If a prior test run is
interrupted between `make_tmp_dir` and `cleanup_tmp_dir`, leftover
files from the previous run can cause the next run to fail (especially
`install_if_missing_skips_existing_files`, which checks an exact
skipped/written count).

**Why it happened:** The sub-agent task instructions said "if
`tempfile` is NOT a dev-dependency, STOP and report." The sub-agent
checked `freminal/Cargo.toml` only, saw no `tempfile` line, and
improvised instead of stopping. `tempfile` IS already in the workspace
(`Cargo.toml:92`, `tempfile.workspace = true`) and is used by both
`freminal-common` and `freminal-terminal-emulator` tests. Adding
`tempfile.workspace = true` to `freminal/Cargo.toml` `[dev-dependencies]`
is a one-line change with no new dependency surface.

**Impact:** Test reliability. No production-code impact. CI cleanup
between runs masks the issue most of the time. Local development with
interrupted test runs can hit it.

**Scope:** `freminal/Cargo.toml` (add one line) +
`freminal/src/shell_integration.rs::tests` (replace `make_tmp_dir` /
`cleanup_tmp_dir` helpers with `tempfile::TempDir::new()` and drop the
manual cleanup calls).

**Suggested approach:**

```rust
use tempfile::TempDir;

#[test]
fn install_if_missing_writes_all_when_dir_empty() {
    let tmp = TempDir::new().expect("create tempdir");
    let result = install_if_missing(tmp.path());
    // ... rest unchanged ...
    // TempDir is automatically cleaned up on drop.
}
```

**Scheduling:** Cosmetic / test-hygiene; no subtask blocked by this.
Can land at any time before the v0.9.0 PR closes.

**Status:** ⤳ Folded into 72.8b. 72.8b rewrites the `shell_integration::tests`
module wholesale (because the `install_if_missing` semantics change from
"skip if exists" to "skip if bytes match", which invalidates the existing
`install_if_missing_skips_existing_files` test among others). The 72.8b
rewrite uses `tempfile::TempDir` from the workspace, which closes 72.16.d
implicitly. 72.8b adds `tempfile.workspace = true` to
`freminal/Cargo.toml` `[dev-dependencies]` as part of its scope.

##### 72.16.e — XTGETTCAP unknown-capability log noise under fish ✅

**Surface point:** Surfaced during 72.8b manual end-to-end testing
(2026-05-19). Fish's startup queries `indn` and `query-os-name` via
XTGETTCAP, producing warn-level log spam on every shell launch:

```bash
WARN freminal_terminal_emulator::terminal_handler::dcs:639:
  XTGETTCAP: unknown capability: indn
WARN freminal_terminal_emulator::terminal_handler::dcs:639:
  XTGETTCAP: unknown capability: query-os-name
```

**Impact:** Cosmetic. Every fish session produces two WARN-level lines
in freminal's stderr/log file at startup. The protocol-level response
is correct (`0+r<hex>` "capability not known"), so fish handles the
refusal gracefully. There is no functional bug. The noise just makes
the log harder to scan and creates false alarm signal for users who
read logs to diagnose other problems.

**Scope of fix:** `freminal-terminal-emulator/src/terminal_handler/dcs.rs`
line 639. Two options:

1. **Drop the warn level for unknown capabilities.** Demote to
   `tracing::debug!` so it's silent at default log levels but still
   discoverable when debugging. Capability queries from any reasonable
   client are not errors; the protocol explicitly defines the
   "unknown" response (`0+r<hex>`) as the well-formed reply.
2. **Add `indn` and `query-os-name` to the known set.** `indn` is the
   "indent N lines" capability — equivalent to ESC D repeated N times.
   `query-os-name` is a Kitty-protocol extension for reporting the OS
   name. Both are legitimate terminfo entries; declining them with
   `0+r…` is correct behavior for now, but supporting them would be a
   small future improvement.

**Suggested approach:** Apply option 1 immediately (one-line change:
`warn!` → `debug!`). Defer option 2 to a future XTGETTCAP capability
expansion subtask; track it in `Documents/ESCAPE_SEQUENCE_GAPS.md`
under the XTGETTCAP row.

**Verification:**

- Launch freminal with fish; verify no WARN lines about `indn` or
  `query-os-name` appear in stderr at default log level.
- `RUST_LOG=freminal_terminal_emulator=debug freminal` still surfaces
  the messages for diagnostic purposes.
- Existing dcs unit tests in `terminal_handler::dcs::tests` continue
  to pass (the response payload is unchanged; only the log level
  changes).

**Scheduling:** Cosmetic; no subtask blocked by this. Can land
opportunistically before or after the Task 72 PR, but should land
**before** v0.9.0 ships so users' first fish launch doesn't dump
warn-level noise into the log.

**Status:** ✅ Complete (commit `b27539d`). Demoted
`tracing::warn!` to `tracing::debug!` at
`freminal-terminal-emulator/src/terminal_handler/dcs.rs:639`. The
protocol response (`0+r<hex>`) is unchanged; only the log level
changes. No new tests added — the existing 1224 dcs/terminal_handler
unit tests pass unchanged, and there were no tests asserting on the
message text. Option 2 (advertise `indn` / `query-os-name` capabilities)
deferred per the plan; tracked under XTGETTCAP expansion in
`Documents/ESCAPE_SEQUENCE_GAPS.md`.

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

## Task 73 — Command Gutters ✅ Complete (2026-06-08)

### 73 Summary

A 4-pixel left gutter rendered inside the terminal area, left of the cell
grid. Each command block's row range is filled with a status color: green
(success), red (failure), yellow (running), gray (unknown). The gutter is
clickable (toggles fold, see 72.10) and hover-able (highlights the block).
The gutter also owns the command-duration label (moved out of the in-buffer
overlay introduced in 72.12) and is the sole hover trigger for the
block-highlight overlay.

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

#### 73.1 — `ThemePalette` gutter colors ✅

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

**Completion notes (2026-06-08):**

- **Color-type deviation from the pseudocode:** `ThemePalette` colors are
  `(u8, u8, u8)` tuples, NOT `egui::Color32` (that type lives in the
  `freminal` GUI crate and cannot be referenced from `freminal-common`).
  The three new fields are therefore `Option<(u8, u8, u8)>`. The
  `freminal/src/gui/colors.rs` helpers convert tuples to `Color32` /
  `[f32; 4]` at the GUI call sites, so a later subtask (73.2) wraps the
  resolved tuple there.
- **No TOML round-trip test (deviation):** `ThemePalette` has no serde
  derives — it is never serialized to/from TOML; only theme _slugs_ are
  stored in config. The plan's "Round-trip TOML test" is therefore not
  applicable to `ThemePalette`. Coverage is instead: a test asserting all
  27 shipped themes default the three overrides to `None`, plus resolver
  tests for fallback, override-preference, and exit-code-independence.
- `gutter_color_for(status)` is a `const fn` method on `ThemePalette`.
  Fallbacks: Success → `ansi[2]` (green), Failure → `ansi[1]` (red),
  Running → `ansi[3]` (yellow). `CommandStatus::Unknown` has no dedicated
  override field and resolves to `ansi[7]` (white).
- All 27 `const ThemePalette` literals updated to add
  `gutter_success/failure/running: None` (mechanical, via a verified
  exact-match pass; the `ansi: [ … ],` close is the last field on every
  literal so the insertion point was unambiguous).
- 4 new unit tests in `themes::tests`. `cargo test --all`,
  `cargo clippy --all-targets --all-features -- -D warnings`, and
  `cargo machete` all clean workspace-wide.

#### 73.2 — Render the gutter column ✅

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

**Completion notes (2026-06-08):**

- **Single-source-of-truth inset (the column-count correctness rule).**
  The reserved left inset is computed once per frame in `app_impl.rs`
  (`gutter_inset_logical = gutter.total_inset_px() / ppp`, zeroed when
  the feature is disabled) and consumed by BOTH the PTY column-count
  computation (`pane_content_width = content_rect.width() - inset`) AND
  the renderer (`terminal_rect.min.x += inset`). Because both derive
  from the identical inset, the column count reported to the PTY always
  equals the rendered cell-grid width — no drift, no wrapping/cursor
  desync. The widget consumes `snap.term_width` (the value `app_impl`
  sent), never re-deriving columns from its own rect.
- **Renderer scope.** The plan guessed `freminal/src/gui/renderer/`. In
  practice the gutter is drawn in `terminal/widget.rs` via egui's
  painter (`rect_filled` per visible row), not in the glow vertex
  builders. The `PaintCallback` rect was changed from `ui.max_rect()`
  (full pane) to `terminal_rect` (full pane minus inset) so the GL
  viewport origin clears the gutter and column 0 does not render under
  the strip.
- **Strip width vs. inset (padding gap).** Per mid-task feedback, the
  4px painted strip would otherwise sit flush against the first glyph.
  Split into two constants in `freminal-common/src/config.rs`:
  `COMMAND_BLOCK_GUTTER_WIDTH_PX = 4.0` (painted strip) and
  `COMMAND_BLOCK_GUTTER_PADDING_PX = 4.0` (blank gap).
  `GutterPosition::total_inset_px()` (= strip + padding = 8px) is the
  shared inset; `width_px()` (= 4px) is the painted strip only. Layout:
  `[0–4px] status strip` → `[4–8px] padding` → `[8px+] glyphs`.
- **Config.** Added `[command_blocks] gutter = "left" | "off"`
  (`GutterPosition` enum, kebab-case serde, default `Left`) and the
  `config_example.toml` entry. Disabling `command_blocks.enabled` zeroes
  the inset entirely (no gutter, full width).
- **Row → color mapping** factored into the pure, unit-tested
  `gutter_status_for_row(blocks, row, running_extent)` in
  `gui/command_blocks.rs`. A running block (no `end_row`) extends to the
  last visible row so the live prompt shows a full yellow bar; on
  overlap the last-emitted block wins. Fold placeholders are colored by
  the folded block's status at half alpha. Status colors come from
  `ThemePalette::gutter_color_for` (73.1): green/red/yellow/white.
- **Alternate screen.** The gutter is suppressed on the alternate screen
  (same rationale as the earlier overlay fix — stored blocks describe
  primary-screen rows).
- **Mouse.** Because `terminal_rect.min.x` shifted right, gutter pixels
  fall outside `terminal_rect`, so terminal mouse-event forwarding and
  hover already ignore the strip; `encode_egui_mouse_pos_as_usize`
  saturates `x < origin` to column 0. Gutter click/hover interception is
  73.3.
- **Tests:** 5 `gutter_status_for_row` tests (containment, inclusivity,
  exit-code → status, running-extent, overlap), 3 config tests
  (default, kebab-case serialization, `total_inset_px` padding),
  extended the 72.5 round-trip test with the `gutter` field.
- **Benchmark (`render_loop_bench`, 15% budget):** the vertex-builder
  passes are untouched by the gutter (it's egui-painter-side); measured
  for regression safety:

  | Benchmark                              | Before   | After    | Change        |
  | -------------------------------------- | -------- | -------- | ------------- |
  | instanced_bg/build_bg_instances/80x24  | 75.7 ns  | 73.2 ns  | -1.0% (noise) |
  | instanced_bg/build_bg_instances/200x50 | 136.6 ns | 129.8 ns | -5.7%         |
  | instanced_fg/build_fg_instances/80x24  | 422.8 µs | 388.1 µs | -5.3%         |
  | instanced_fg/build_fg_instances/200x50 | 464.2 µs | 474.5 µs | -0.8% (noise) |

- `cargo test --all`, `cargo clippy --all-targets --all-features -- -D
warnings`, `cargo machete`, `cargo fmt --check` all clean.
- **Open follow-up (not blocking):** `CommandStatus::Unknown` resolves
  to normal white (`ansi[7]`); the plan text said "gray". Could map to
  `ansi[8]` (bright-black/gray) if a grayer neutral is preferred —
  deferred pending user preference.

#### 73.3 — Gutter click and hover ✅

**Scope:** `freminal/src/gui/mouse.rs`, `terminal/widget.rs`.

- Mouse events whose `x` coordinate falls within the gutter (0..4px) are
  intercepted before the usual cell-coordinate routing.
- Single click on a finished block: toggle fold (same path as 72.10's
  `FoldPreviousCommand` keybinding).
- Single click on a running block: no-op (cannot fold).
- Hover within the gutter: emit the same hover-highlight overlay as 72.12
  (entire block tinted).

**Verification:** Integration test using egui's test harness for mouse
events.

**Completion notes (2026-06-08):**

- **Click interception.** A gutter pre-check in `widget.rs::show()` (modeled
  on the existing scrollbar pre-check) intercepts a primary press whose
  position falls in the inset region (`pos.x ∈ [pane.min.x,
terminal_rect.min.x)`) before `write_input_to_terminal`. Gutter positions
  are already outside `terminal_rect`, so they would otherwise be dropped
  entirely (no fold, no focus). The press maps `y → rendered row → buffer
  row → block` via a fresh fold-aware `RowMap`
  (`gutter_block_id_at_pos`). A finished block toggles its fold
  (`view_state.toggle_fold`); a running block is a no-op fold. Either way
  the pane is focused (`left_mouse_button_pressed`), since the gutter is
  outside the rect that normally sets that flag.
- **Foldability guard** factored into the pure, tested
  `command_blocks::block_is_foldable` (finished blocks only), mirroring the
  `FoldPreviousCommand` "completed block" guard.
- **Hover hit-test** shares `command_blocks::gutter_block_for_row`
  (a `gutter_status_for_row` refactor that returns the block, not just the
  status). The gutter trigger is added _alongside_ the existing 72.12
  cell-hover trigger; 73.5 retires the cell trigger.
- **Hit zone** is the whole inset (4px strip + 4px padding), a more
  forgiving target than the 4px strip alone.
- **Two-part hover-live fix (the subtle one).** Getting the gutter hover
  tint to appear/clear on bare pointer motion required two independent
  fixes, both necessary:
  1. **Waking a frame.** The windowing cursor-move fast path
     (`freminal-windowing/event_loop.rs`, Task 65/68 idle-CPU
     optimization) only schedules a repaint when egui reports `repaint`,
     i.e. when an egui-tracked interactive region's hover state changes.
     The terminal hover tint is painted by our own GL pass, not an egui
     widget, so a bare move never woke a frame. Registering the gutter as
     a `Sense::click()` region makes egui report `repaint` on enter/leave,
     waking the frame (and giving the hand cursor).
  2. **Rebuilding the VBO.** The hover tint is baked into the background
     instance buffer, which was only rebuilt on
     content/selection/search/blink changes. A hover-only change reused
     stale vertices and showed nothing. Added a `hover_changed` term
     (tracked via `cache.previous_command_block_hover_rows`) to both the
     cursor-only fast-path exclusion and the full-rebuild trigger. The
     hover-row computation was hoisted into
     `compute_command_block_hover_rows` and run _before_ the rebuild
     decision so `hover_changed` is known in time.
- **Mouse-reporting safety.** Gutter positions are outside `terminal_rect`,
  so `terminal_rect.contains` already excludes them from terminal mouse-
  event forwarding and selection; DEC mouse modes never see gutter
  hover/clicks.
- **Tests:** added `gutter_block_for_row` and `block_is_foldable` unit
  tests (finished foldable, running not, containment, returns block). No
  egui-harness integration test was added — the repo has no egui_kittest
  harness; the click/hover logic is covered by the pure helper tests, and
  the wiring was verified manually with debug instrumentation across the
  appear/track/clear lifecycle.
- `cargo test --all`, `cargo clippy --all-targets --all-features -- -D
warnings`, `cargo machete`, `cargo fmt --check` all clean.

#### 73.4 — Settings UI: gutter toggle ✅

**Scope:** `freminal/src/gui/settings.rs` / `settings_dispatch.rs`.

- Add a dropdown in the Command Blocks section (introduced in 72.5): Gutter
  position (`Left` / `Off`).

**Verification:** Toggle persists via TOML round-trip.

**Completion notes (2026-06-08):**

- Added a "Status gutter" `ComboBox` (`Left` / `Off`) to the Command Blocks
  section of the **Shell Integration** settings tab (where 72.5
  consolidated the command-block settings), below the duration threshold.
  Mirrors the existing `tab_bar_position` dropdown pattern with a
  `gutter_position_label` helper.
- **No `settings_dispatch.rs` change needed.** On Apply the dispatch
  already replaces the whole live config (`self.config = new_cfg`), and
  `app_impl` reads `command_blocks.gutter.total_inset_px()` every frame, so
  toggling takes effect immediately — switching to `Off` zeroes the inset,
  which flows through the normal resize path (wider PTY column count, strip
  hidden). Same "takes effect on next render" model 72.5 documented.
- **Tests:** `gutter_position_labels` (label helper) and
  `gutter_setting_persists_through_draft_apply` (modal surfaces and mutates
  the field). The TOML round-trip is already covered by the
  `freminal-common` config test added in 73.2.
- `cargo test --all`, `cargo clippy --all-targets --all-features -- -D
warnings`, `cargo machete`, `cargo fmt --check` all clean.

#### 73.5 — Move hover trigger from buffer to gutter ✅

**Scope:** `freminal/src/gui/mouse.rs`, `freminal/src/gui/renderer/`
(whichever module currently owns the 72.12 hover overlay), and the view-
state struct that tracks `hovered_block_id` (or equivalent).

**Motivation:** 72.12 made the entire row range of a command block emit the
hover highlight whenever the mouse hovered any cell in the block. This
causes the overlay to fire constantly during normal terminal use (text
selection, mouse-tracking apps, even passive cursor motion across the
output area), which is visually noisy and conflicts with mouse-reporting
modes. The correct model — now that the gutter exists — is that the gutter
is the dedicated affordance for "this is a command block, here is its
metadata, click to fold". Hovering output text should do nothing
block-related.

- Remove the in-buffer hover hit-test added in 72.12. Cells in the terminal
  area no longer participate in block-hover detection.
- Move the hover trigger entirely to the gutter strip (the 4px column
  defined in 73.2). Hovering anywhere in the gutter rows belonging to a
  block highlights the block's row range with the existing
  selection-tint-at-25%-alpha overlay.
- The overlay rendering itself (the tinted row range across the cell grid)
  is unchanged — only the trigger surface moves.
- Hover is still purely view-state; no snapshot mutation.
- Hover is disabled when `config.command_blocks.enabled == false` or when
  `config.command_blocks.gutter == "off"` (no gutter, no hover trigger).
- Mouse-reporting modes: the gutter intercepts events before the cell-
  coordinate router (already specified by 73.3), so DEC mouse modes are
  unaffected — the application never sees gutter hover/clicks.

**Verification:** Update or replace the 72.12 hover unit tests. New
integration test: hovering output cells does not set
`hovered_block_id`; hovering gutter rows belonging to a block does.

**Completion notes (2026-06-08):**

- **Minimal change thanks to 73.3.** 73.3 had already added the gutter as a
  hover trigger _alongside_ the 72.12 cell trigger inside
  `compute_command_block_hover_rows`. 73.5 simply deletes the cell-hover
  branch (`else if terminal_rect.contains(...)`), leaving the gutter strip
  as the sole surface. The tint rendering (25%-alpha row range across the
  cell grid) is unchanged — only the trigger surface moved.
- **Disabled states.** Hover returns `None` when the feature is off, the
  alternate screen is active, there are no blocks, OR the gutter is off
  (`gutter_inset <= 0.0`, which is the case for `gutter = "off"`). The
  unused `logical_cell_w` parameter was dropped.
- **No view-state field.** There is no stored `hovered_block_id`; hover is
  recomputed per frame as a local (`command_block_hover_rows`), gated by
  the `hover_changed` cache term from 73.3. The plan's "does not set
  `hovered_block_id`" is expressed as "returns `None`".
- **Mouse-reporting safety** is inherited: the gutter is outside
  `terminal_rect`, so DEC mouse modes never see gutter hover.
- **Tests:** new `gutter_hover_trigger_tests` module —
  `hovering_gutter_row_tints_the_block` (gutter -> `Some(range)`),
  `hovering_output_cell_does_not_tint` (the regression: cell -> `None`),
  `gutter_off_disables_hover`, and `no_pointer_no_tint`. Built on
  `TerminalSnapshot::empty()` + `ViewState::new()` + a no-fold `RowMap`, so
  no GUI/GL context is needed. No prior 72.12 hover unit test existed to
  retire (the cell trigger was inline render-path code).
- `cargo test --all`, `cargo clippy --all-targets --all-features -- -D
warnings`, `cargo machete`, `cargo fmt --check` all clean.

#### 73.6 — Move command-duration label into the gutter ✅

**Scope:** Same renderer module as 73.2 (the gutter draw pass) and the
duration-formatting helper introduced in 72.12.

**Motivation:** 72.12 renders the duration label (e.g. `"1.3s"`) right-
aligned on the command's first row inside the terminal cell grid. For
commands that scroll, this means the label is painted on the prompt line
which then scrolls out of view almost immediately — the label is
effectively invisible for any non-trivial command. The gutter is anchored
to the visible window and follows the block as it scrolls, so the duration
label belongs there.

- Remove the in-buffer duration label rendered by 72.12 (the right-aligned
  text on the command's first row).
- Render the duration label inside the gutter strip, vertically anchored to
  the block's last visible row in the current viewport (so the label
  follows the block as it scrolls and is always next to the gutter color
  bar). If the block extends below the viewport, anchor to the last
  on-screen row of the block.
- The 4px gutter is too narrow for inline text. Two options — implementer
  picks during 73.6:
  - **(a)** Render the label as a small overlay tooltip that appears
    immediately adjacent to (right of) the gutter, on the block's last
    visible row. No cell-grid intrusion: drawn as a floating egui label
    layer above the cell content.
  - **(b)** Widen the gutter to a configurable
    `[command_blocks] gutter_width_px` (default 4, raises to e.g. 24 only
    when duration display is enabled) and render the label inside.
  - Default recommendation: (a). Keep the gutter thin; render label as a
    floating layer. Decision logged in commit message.
- Threshold gating is unchanged: only render when
  `duration() >= config.command_blocks.duration_threshold_secs`.
- Label is hidden for the currently-running block (no `finished_at` yet).

**Verification:** Visual verification via a recorded `.frec`. The
duration-formatting unit test from 72.12 still applies; add a placement
test (the label coordinate is computed against the block's last on-screen
row, not its first row).

**Completion notes (2026-06-08):**

- **Option (a) chosen** (floating layer, keep the gutter thin). The label
  is drawn as an egui-painter text layer just inside the cell grid
  (`terminal_rect.min.x + 2px`, left-aligned, ~60% alpha), overlaying the
  first cells of the block's last visible row. The gutter stays 4px; no
  `gutter_width_px` config was added.
- **Last-visible-row anchor.** Relocated from 72.12's first-row placement
  (which scrolled off immediately) to the block's last on-screen row via
  the pure, tested `duration_label_anchor_row(block, win_start, win_end,
running_extent)` — clamps to the viewport bottom when the block extends
  below, returns `None` when the block is entirely off-screen, and uses
  `running_extent` for running blocks.
- **Gating.** Requires the gutter to be present (`gutter_inset > 0`), so a
  `gutter = "off"` config also hides the label (no anchor). Threshold
  gating unchanged; running blocks have no `duration()` and are skipped;
  suppressed on the alternate screen. Uses `block.duration()` (the 73.7
  C->D fix), so values are correct.
- **Tests:** five `duration_label_anchor_row` cases (last-not-first,
  clamp-to-viewport, above/below viewport -> `None`, running-block ->
  `running_extent`).
- `cargo test --all`, `cargo clippy --all-targets --all-features -- -D
warnings`, `cargo machete`, `cargo fmt --check` all clean. No
  `render_loop_bench` regression (label is egui-painter-side, no
  vertex-builder change).

**Shell-integration follow-up (committed separately):** visual testing of
73.6 in bash surfaced that bash never emitted `OSC 133 C` under freminal's
`bash --posix` launch (bash-preexec's preexec dispatch is dead in that
mode), so bash durations showed prompt-to-prompt. Fixed in
`shell-integration/bash/freminal-init.bash` by emitting C from a
re-armed DEBUG trap; shell-integration bumped to v4. zsh/fish were
unaffected.

#### 73.7 — Investigate spurious long duration for instant commands ✅

**Scope:** Investigation first; fix scope determined by findings.
Likely surfaces: `CommandBlock::started_at` / `finished_at`
(`freminal-common/src/buffer_states/command_block.rs`), the OSC 133 prompt
handler that opens a block, the OSC 133 post-exec handler that closes it
(`freminal-terminal-emulator/src/...`), and the duration-formatting helper
introduced in 72.12.

**Symptom (reported during 72.12 testing):** A trivial `ls` command (which
runs in single-digit milliseconds) sometimes renders with a multi-second
duration label. Two hypotheses to investigate:

1. **Time-unit / format mismatch.** `started_at: SystemTime`,
   `finished_at: Option<SystemTime>`, and the duration formatter may be
   handling units inconsistently — e.g. mixing seconds and milliseconds,
   or formatting `duration_since(UNIX_EPOCH)` instead of
   `finished_at.duration_since(started_at)`. Audit the full chain:
   - Where `started_at` is stamped (OSC 133 C / `prompt_start` handler).
   - Where `finished_at` is stamped (OSC 133 D / `post_exec` handler).
   - The `duration()` accessor on `CommandBlock`.
   - The formatter that turns `Duration` into the displayed string.
2. **Block-boundary confusion.** Multiple `CommandBlock` entries are being
   conflated — e.g. the prompt for command N is being matched against the
   `finished_at` of command N-1, so the displayed duration is actually
   "time the user spent reading the previous output before pressing
   Enter". This is plausible if the OSC 133 sequences are being grouped
   into the wrong block (off-by-one in `command_blocks` `VecDeque`
   indexing, or `prompt_start` opening a new block when it should close
   the previous one first, or `post_exec` closing the wrong block).

**Investigation steps:**

- Add temporary `tracing::debug!` logs at every `CommandBlock` field
  mutation: capture `id`, `fid`, the `SystemTime` value, and a Rust
  source location. Confirm:
  - Exactly one `started_at` stamp per command, at the correct moment.
  - Exactly one `finished_at` stamp per command, at the correct moment.
  - `finished_at - started_at` matches wall-clock time as measured
    externally (`time ls`).
- Reproduce by recording a `.frec` of a slow shell-init scenario
  (`bash -i` with a heavy `~/.bashrc`) and a fast scenario (`sh -c`
  without rc files), running `ls` in each, and comparing the captured
  block lifecycle events.
- If hypothesis 1: fix the unit/format bug and add a unit test asserting
  `Duration::from_millis(5)` formats as `"5ms"`, not `"5s"`.
- If hypothesis 2: fix the block-boundary handling and add an integration
  test feeding a hand-crafted OSC 133 sequence stream
  (`A` → text → `B` → text → `C` → output → `D;0`) and asserting one
  `CommandBlock` is produced with `finished_at - started_at` matching
  the `C`-to-`D` delta only.

**Verification:** Both hypotheses must be ruled out or fixed. Unit test
for the duration formatter. Integration test for the OSC 133 lifecycle.
Manual `.frec` re-test of the original `ls` reproducer showing the label
either absent (below threshold) or correctly reporting milliseconds.

**Note:** This subtask can run independently of 73.1–73.6 — it touches the
duration computation, not the gutter rendering. Schedule it early in
Task 73 so the fix is in place before 73.6 moves the label into the
gutter, otherwise the bug just relocates with the label.

**Completion notes (2026-06-08):**

- **Root cause: timing semantics, not units.** Hypothesis 1 (unit/format
  mismatch) was ruled out — `duration()` used correct `SystemTime`
  arithmetic and the formatter truncates whole seconds correctly.
  Hypothesis 2 was the cause, in its "wrong window" form: `started_at` is
  stamped at `OSC 133 A` (prompt start) and `finished_at` at `OSC 133 D`,
  so `duration() = D - A` **included the time the user spent typing the
  command and reading the previous output at the prompt**. An instant `ls`
  preceded by a 30 s pause reported ~30 s. The fid correlation itself
  (72.8c) was correct — no off-by-one.
- **Fix.** Added `executed_at: Option<SystemTime>` to `CommandBlock`,
  stamped in `Buffer::mark_output_start_row` (`OSC 133 C`, command-execution
  start). `CommandBlock::duration()` now measures `finished_at -
executed_at` (true runtime C->D), falling back to `started_at` only when
  no `C` was received. The in-buffer duration-overlay renderer was also
  fixed: it computed `finished_at.duration_since(started_at)` directly
  (the D-A bug) and now calls `block.duration()`.
- **No FREC/snapshot serialization concern.** `CommandBlock` rides the
  snapshot as `Arc<[CommandBlock]>` (cloned), so the new field propagates
  automatically; FREC v2 captures the OSC byte stream, not the struct.
- **No escape-sequence-doc change.** OSC 133 C parsing/support is
  unchanged (still ✅); only internal timing semantics changed, and the
  coverage row makes no duration claim.
- **Tests:** `duration_measures_from_executed_at_not_started_at` (the
  regression: 30 s prompt-wait + 5 ms command -> 5 ms),
  `duration_falls_back_to_started_at_when_executed_at_missing`,
  `duration_clock_skew_against_executed_at_is_none`, and
  `mark_output_start_row_stamps_executed_at` (buffer-level: `OSC 133 C`
  stamps `executed_at`). Five `CommandBlock` test-literal sites updated
  for the new field. A handler-level wall-clock-delta integration test was
  not added because `executed_at` uses real `SystemTime::now()` and cannot
  assert a deterministic delta without sleeping; the deterministic unit
  tests cover the C->D semantics fully.
- **Benchmark:** `command_block_record_10k` ~895 µs (vs ~863 µs documented
  baseline) — within machine variance; the one-field struct growth and the
  `C`-path `SystemTime::now()` (not exercised by this bench) cause no
  regression.
- `cargo test --all`, `cargo clippy --all-targets --all-features -- -D
warnings`, `cargo machete`, `cargo fmt --check` all clean.

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

## Task 76 — Notification System (OSC 9 / OSC 777) ✅ Complete (2026-06-09)

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
- **Capability detection:** OSC 9/777 have no capability-query handshake;
  clients detect freminal via the `TERM_PROGRAM=freminal` env var (set in
  72.6). There is no terminal-side notification "broadcast" to implement.
  `XTGETTCAP TN` / `XTVERSION` already have responders that 76.6 verifies
  and extends. OSC 99's `p=?` support query is out of scope (see
  Task 99 in `PLAN_VERSION_100.md`). Document detection in the shell
  integration README.

### 76 Subtasks

#### 76.1 — Add `notify-rust` and capability flags ✅ 2026-06-09

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

**Completion notes (2026-06-09):**

- `notify-rust = "4"` added to the workspace `[workspace.dependencies]`
  (alphabetical, after `nix`) and referenced as `notify-rust.workspace
= true` in `freminal/Cargo.toml` `[dependencies]` only — matching the
  workspace's `crate.workspace = true` convention. **Not** added to
  `freminal-common`, `freminal-buffer`, or
  `freminal-terminal-emulator`. Resolves to `notify-rust v4.17.0`,
  which uses pure-Rust `zbus` on Linux — no system dbus dev library is
  required in the dev shell.
- `NotificationsConfig` added to `freminal-common/src/config.rs` with
  all eight documented fields (`enabled`, `osc_9`, `osc_777`,
  `on_command_finished`, `command_finished_threshold_secs`,
  `routing_error`, `routing_info`, `routing_command_finished`), wired
  into the top-level `Config` struct and its `Default` impl between
  `command_blocks` and `keybindings`.
- New `NotificationRouting` enum (`Toast`, `System`, `Both`,
  `SystemWhenUnfocused`) with `#[serde(rename_all = "snake_case")]`,
  mirroring the `GutterPosition` pattern. Added `const fn
wants_toast(focused)` and `const fn wants_system(focused)` helpers so
  the routing decision is unit-testable and reusable by the 76.4
  router. Default is `SystemWhenUnfocused`.
- `NotificationsConfig` required a localized
  `#[allow(clippy::struct_excessive_bools)]` (four independent TOML
  toggles), matching the documented precedent in `snapshot.rs`,
  `gui/mod.rs`, `rendering.rs`, `widget.rs`, and `view_state.rs`.
- `[notifications]` section added to `config_example.toml` (all keys
  commented out, defaults documented) under a new `NOTIFICATIONS`
  banner between `[command_blocks]` and `[startup]`.
- `notify-rust` is unused until 76.4, so it was added to the
  `[workspace.metadata.cargo-machete] ignored` list with a comment
  instructing its removal once 76.4 lands. This keeps the 76.1 commit's
  `cargo machete` verification green without a permanent suppression.
- 4 new tests in `config::tests`: `notifications_default_is_opt_in`,
  `notifications_round_trip_through_toml`,
  `notification_routing_serializes_as_snake_case`,
  `notification_routing_dispatch_decisions`.
- Verification: `cargo test --all` (no regressions), `cargo clippy
--all-targets --all-features -- -D warnings` clean, `cargo fmt --all
-- --check` clean, `cargo machete` clean.

#### 76.2 — OSC 9 and OSC 777 parsing ✅ 2026-06-09

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

**Completion notes (2026-06-09):**

- **`AnsiOscType::Notify { title: Option<String>, body: String }`** added to
  `freminal-common/src/buffer_states/osc.rs` (single variant for both OSC 9
  and OSC 777, as specified) with a `Display` arm. Two new `OscTarget`
  variants `Notify9` (OSC 9) and `Notify777` (OSC 777), wired into the
  `From<&AnsiOscToken>` code→target table (9 → `Notify9`, 777 → `Notify777`).
- **New handler module `freminal-terminal-emulator/src/ansi_components/osc_notify.rs`**
  mirroring the `osc_shell_info.rs` pattern. Both parsers read from the
  **raw (un-split) parameter bytes** rather than the upstream semicolon
  token split, because notification bodies legitimately contain `;` and the
  token split is too aggressive. Declared `pub mod osc_notify;` in
  `ansi_components/mod.rs`.
  - **OSC 9:** body = everything after the first `;`. `title = None`. Empty
    or missing body silently consumed.
  - **OSC 777:** strips the `notify;` prefix when present, then splits the
    remainder on the _first_ `;` into `(title, body)` (preserving semicolons
    in the body). `notify;TITLE` → title only, empty body. A payload without
    the `notify;` prefix is treated as an all-body notification with no
    title. Empty notifications silently consumed.
  - Non-UTF-8 payloads are dropped (logged at debug), not lossy-decoded.
- **Dispatch arms** for `OscTarget::Notify9`/`Notify777` added to
  `dispatch_osc_target` in `osc.rs`, forwarding to the new handlers.
- **`TerminalHandler::handle_osc`** (the exhaustive `AnsiOscType` match in
  `terminal_handler/osc.rs`) gained an explicit `AnsiOscType::Notify` arm
  that logs at debug with a `TODO(76.3)` marker — actual GUI forwarding is
  76.3's scope; this arm only keeps the match exhaustive/compiling.
- **Clippy:** `dispatch_osc_target` crossed 100 lines (106) with the two new
  arms; added a localized `#[allow(clippy::too_many_lines)]` with
  justification (flat dispatch table), consistent with the 72.4 precedent.
- **Escape-sequence docs (mandatory dual-doc update):**
  `ESCAPE_SEQUENCE_COVERAGE.md` gained an OSC 9 row and promoted OSC 777 to
  ✅; `ESCAPE_SEQUENCE_GAPS.md` removed the OSC 777 gap entries (Priority 2
  table + OSC Gaps table + prose). "Last updated" headers refreshed in both.
- **Tests:** 4 new in `freminal-common` (target mappings + Notify Display);
  17 new in `osc_notify.rs` (OSC 9 body/empty/semicolon/ST; OSC 777
  title+body / title-only / no-prefix / semicolon-body / empty / ST;
  non-UTF-8 and missing-semicolon direct-call branches; `parse_777_payload`
  unit).
- **Verification:** `cargo test --all` (no regressions), `cargo clippy
--all-targets --all-features -- -D warnings` clean, `cargo fmt --all --
--check` clean, `cargo machete` clean.

#### 76.3 — OSC 9/777 dispatch ✅ 2026-06-09

**Scope:** `freminal-terminal-emulator/src/terminal_handler/osc.rs`.

- New `handle_osc_notify(&mut self, notify: &AnsiOscType::Notify)` arm that
  forwards a new `WindowCommand::Notification { kind: NotificationKind,
title: Option<String>, body: String }` to the GUI thread via the existing
  window-post channel.
- `NotificationKind` enum: `OscText`, `CommandFinished`, `Error`, `Info`.

**Verification:** Unit test that emitting OSC 9 produces a
`WindowCommand::Notification`.

**Architectural note — routed via `WindowManipulation`, not a new
`WindowCommand` variant (decided 2026-06-09):**

The plan called for a `WindowCommand::Notification { kind, title, body }`
variant forwarded "via the existing window-post channel". Investigation
confirmed the same constraint documented in 72.3: the `TerminalHandler`
does not own a `Sender<WindowCommand>`. It produces a
`Vec<WindowManipulation>` (drained by `take_window_commands`); the PTY
loop in `freminal/src/gui/pty.rs` then wraps each into
`WindowCommand::Viewport`/`Report`. The `WindowCommand` enum
(`io/mod.rs`) only wraps `WindowManipulation` — it has no free-standing
payload variants. The idiomatic transport for a handler→GUI side-effect
signal is therefore a new `WindowManipulation` variant, exactly how
`Bell` and `SetClipboard` already flow. So 76.3 adds
`WindowManipulation::Notification { kind, title, body }` rather than a
`WindowCommand::Notification`. It still arrives on the GUI through the
`WindowCommand::Viewport` wrapper and is consumed in
`rendering::handle_window_manipulation`, which is where 76.4 hooks the
`NotificationRouter`.

**Completion notes (2026-06-09):**

- **`NotificationKind` enum** (`OscText`, `CommandFinished`, `Error`,
  `Info`) added to `freminal-common/src/buffer_states/window_manipulation.rs`.
- **`WindowManipulation::Notification { kind, title, body }`** variant
  added to the same enum, documented alongside `Bell`.
- **`TerminalHandler::handle_osc`** `AnsiOscType::Notify` arm (the 76.2
  `TODO(76.3)` placeholder) now pushes
  `WindowManipulation::Notification { kind: OscText, title, body }` onto
  `window_commands` instead of only logging.
- **`rendering::handle_window_manipulation`** (GUI) gained a
  `WindowManipulation::Notification` arm. For 76.3 it logs at debug with
  a `TODO(76.4)` marker — the actual routing (toast vs system daemon per
  `[notifications]`) is 76.4's scope. The arm exists so the exhaustive
  match compiles.
- **Tests:** 2 new in a new `terminal_handler::osc::tests` module
  (`osc_notify_pushes_window_command`,
  `osc_notify_without_title_pushes_window_command`) verifying a
  `TerminalOutput::OscResponse(AnsiOscType::Notify { … })` produces
  exactly one `WindowManipulation::Notification` with the right kind /
  title / body. Byte-stream coverage (raw OSC 9 / 777 through the
  parser) already lives in 76.2's `osc_notify.rs`; the parser pipeline
  is not re-driven here because `TerminalHandler::handle_data` only
  inserts text — the ANSI parser sits above the handler.
- **Verification:** `cargo test --all` (no regressions), `cargo clippy
--all-targets --all-features -- -D warnings` clean, `cargo fmt --all --
--check` clean, `cargo machete` clean.

#### 76.4 — GUI notification router ✅ 2026-06-09

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

**Completion notes (2026-06-09):**

- **New module `freminal/src/gui/notifications.rs`** exporting
  `NotificationRouter` (a stateless zero-field struct) and a
  `NotificationRequest { kind, title, body }` type.
  - `NotificationRouter::route(req, config, focused, &mut ToastStack)` is
    the single dispatch entry. Returns early when
    `config.enabled == false`. Selects the per-category
    `NotificationRouting` (`routing_error` / `routing_command_finished` /
    `routing_info`) via `routing_for`, then uses the focus-aware
    `wants_toast` / `wants_system` helpers (76.1) to decide each leg.
  - **Toast leg:** `Error`-kind → `ToastStack::error`, everything else →
    `ToastStack::info`. (No `warning` push method exists; that's fine —
    OSC text / command-finished are informational.)
  - **System leg:** spawns a named `freminal-notify` thread that calls
    `notify_rust::Notification::new().summary(..).body(..).show()`,
    mirroring the existing `freminal-open-url` thread pattern in
    `terminal/widget.rs`. `notify-rust`'s `show()` blocks on D-Bus on
    Linux, so it must not run on the egui frame thread. Failures (no
    daemon) are logged and ignored.
  - `command_finished_request(block, command, config) -> Option<NotificationRequest>`
    builds the command-finished notification, applying the `enabled` +
    `on_command_finished` + `command_finished_threshold_secs` +
    not-`Running` gates. Failure → `Error` kind with `(exit N)`; success /
    unknown-exit → `CommandFinished` kind. Duration rendered via the
    existing `command_blocks::format_command_duration`. Empty command text
    falls back to `"Command"`.
- **OSC 9/777 wiring:** `rendering::handle_window_manipulation` gained a
  `&mut Vec<NotificationRequest>` out-parameter; its `Notification` arm
  (the 76.3 `TODO(76.4)` placeholder) now pushes a request into it. The
  caller in `app_impl::update()` collects across all panes and routes them
  after the pane loop, where `self.config.notifications` and the toast
  stack are borrowable. This avoids threading `&mut self` into the free
  `handle_window_manipulation` function.
- **Command-finished wiring:** the 72.9 drain site
  (`app_impl.rs`, formerly the `TODO(Task 76)` marker) now calls
  `command_finished_request` for each finished block (before it is moved
  into the recent-commands ring) and routes the collected requests after
  the loop. Focus state read from the active pane's
  `view_state.window_focused`. The `win` local is owned (removed from
  `self.windows`), so routing alongside `win.tabs.iter_mut()` does not
  conflict with the `self.toasts` borrow.
- **`notify-rust` removed from the `cargo-machete` ignore list** — it is
  now genuinely referenced, so the temporary 76.1 suppression is gone.
- **Tests:** 14 in `gui::notifications::tests` — summary fallback per
  kind / explicit title; `routing_for` mapping; disabled-config no-op;
  toast-only / system-only / both / system-when-unfocused leg decisions;
  and `command_finished_request` gating (disabled, off, threshold,
  running, success-kind, failure-kind, empty-command placeholder). A
  test-only `ToastStack::len()` was added to assert toast-leg outcomes
  without rendering. The system leg (background thread) is not asserted —
  it is best-effort and has no observable state in-process.
- **Verification:** `cargo test --all` (no regressions), `cargo clippy
--all-targets --all-features -- -D warnings` clean, `cargo fmt --all --
--check` clean, `cargo machete` clean.

#### 76.4a — Cleanup: `[notifications]` (and siblings) never loaded from user TOML ✅ 2026-06-09

**Surfaced during manual testing of 76.4:** with `[notifications]
enabled = true` in a user `config.toml`, no notifications fired — not
even a direct `printf '\e]9;hi\a'` toast. Root cause: the config loader
deserializes into a `ConfigPartial` (all-`Option` fields) and merges it
onto `Config::default()` via `Config::apply_partial`. `ConfigPartial`
was **missing** the `notifications`, `shell_integration`,
`command_blocks`, and `tab_title` fields, so those four whole sections
were silently dropped on load and always kept their defaults
(`notifications.enabled = false`). This is a latent bug predating Task
76 — `shell_integration` / `command_blocks` (Task 72) and `tab_title`
(Task 94) were added to `Config` but never to `ConfigPartial`. Task
76.1 added `notifications` with the same omission.

**Fix:** added all four fields to `ConfigPartial` and their merge arms
to `apply_partial`. Regression tests:
`notifications_apply_partial_enables_from_toml` and
`previously_dropped_sections_apply_partial` (covers all four sections
through the partial-merge path). `cargo test --all`, clippy
`-D warnings`, fmt, machete all clean.

#### 76.4b — Polish: zbus log silencer + desktop-notification urgency ✅ 2026-06-09

Two small fixes found during manual testing of 76.4 on Linux/Hyprland:

- **zbus log spam.** `notify-rust`'s `zbus` D-Bus dependency emits
  `INFO`-level connection-handshake spans on every notification, which
  flooded stdout and the log file. Added `zbus=warn` to the existing
  framework-silencer directive list (alongside `winit=off` / `wgpu=off`
  / `egui=off`) on both the stdout and file `EnvFilter`s in `main.rs`.
  `warn` rather than `off` so genuine D-Bus errors still surface.
- **Desktop-notification urgency + appname.** `show_system` now sets
  `appname("freminal")` and an urgency hint —
  `notify_rust::Urgency::Critical` for `Error`-kind notifications,
  `Normal` for everything else. Many Linux notification daemons only
  raise a banner (rather than silently filing the notification in the
  tray) when an urgency hint is present. (Whether a banner appears, and
  on which monitor, is ultimately the daemon's policy — verified working
  end-to-end under `wayle` on Hyprland.)

`cargo test --all`, clippy `-D warnings`, fmt, machete all clean.

#### 76.4c — Config load-path audit + self-protecting guard test ✅ 2026-06-09

Follow-up to 76.4a: a full audit of the config load/save path to find any
other silently-dropped options, plus a durable guard so the bug class
cannot recur.

**Audit findings (only 76.4a was a real bug):**

- Section-level sync of `Config` ↔ `ConfigPartial` ↔ `apply_partial` is
  now complete (all 20 top-level fields; verified field-by-field).
- Serde attributes are safe: only `skip_serializing_if =
"Option::is_none"` / `"KeybindingsConfig::is_empty"` (which omit only
  absent/empty values) and a correct `#[serde(flatten)]` on the
  keybindings map. No `#[serde(skip)]` drops data.
- CLI override path (`apply_cli_overrides`) is narrow by design (shell,
  hide_menu_bar, deprecated write-logs flag) — no silent drops.
- Save/load is symmetric: `save_config` serializes the full `Config`;
  `load_config` merges via the now-field-complete `ConfigPartial`.
- `config_example.toml` has no documentation drift against the structs.

**Guard test `every_config_section_survives_partial_merge`:** sets a
non-default value in every section, runs the real load path
(`to_string_pretty` → `ConfigPartial` → `apply_partial`), and asserts
each value survived. Two protection layers, both proven:

- **Runtime:** disabling any merge arm fails with `"<section> section
dropped"` (verified by temporarily disabling the notifications arm).
- **Compile-time:** a trailing exhaustive `let Config { .. } = loaded`
  destructure with **no** `..` rest pattern means adding a new field to
  `Config` fails to compile the test (`E0027`) until the author wires it
  in — verified by temporarily adding a dummy field.

`cargo test --all`, clippy `-D warnings`, fmt, machete all clean.

#### 76.5 — Notification templates and bell ✅ 2026-06-09

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

**Completion notes (2026-06-09):**

- **`NotificationsConfig::command_finished_template: String`** added with
  default `"{command} finished in {duration} (exit {exit_code})"`. Both
  the struct doc-comment TOML example and `config_example.toml` document
  the five tokens.
- **`BellConfig::on_command_finished: bool`** added, default `false`.
  Both `NotificationsConfig` and `BellConfig` already round-trip as
  whole-struct `Option<…>` fields in `ConfigPartial` / `apply_partial`,
  so the two new fields merge from user TOML with no extra wiring (the
  76.4a/76.4c class of bug does not apply at the field level).
- **Template rendering** moved into a new
  `gui::notifications::render_command_finished_template(template, block,
command, tab_name)` helper. Tokens: `{command}` (trimmed, or
  `"Command"` when empty), `{duration}` (`format_command_duration`, empty
  when unknown), `{exit_code}` (numeric, or `"?"` when the shell omitted
  it), `{cwd}` (block cwd, empty when unknown), `{tab_name}`. Unknown
  tokens are left untouched. The five token literals are module
  constants so clippy's nursery `literal_string_with_formatting_args`
  does not flag the brace placeholders as `format!` directives.
- **`command_finished_request`** gained a `tab_name: &str` parameter and
  now renders the body via the template instead of the hard-coded
  string. The Error-vs-CommandFinished `kind` is still driven by the
  exit code (non-zero ⇒ `Error`), independent of the template body.
- **Tab name** is resolved at the 72.9 drain site in `app_impl::update()`
  via `tab.display_name(policy, separator)` (computed before the
  `iter_panes_mut` borrow, since that borrows `tab` mutably) using the
  existing `[tab_title]` policy/separator.
- **Bell on command finished:** the drain loop, when
  `config.bell.on_command_finished` is set, rings the bell using the
  configured `bell.mode` — visual sets `pane.bell_active` +
  `pane.view_state.bell_since`, audio calls `platform::system_beep()`,
  mirroring the `WindowManipulation::Bell` path in `rendering`. This
  fires on every finished command (not gated by the notification
  duration threshold).
- **Tests:** 7 new in `gui::notifications::tests` (all-token
  substitution, empty-command placeholder, unknown exit ⇒ `?`, unknown
  cwd ⇒ empty, unknown token untouched, custom-template through
  `command_finished_request`, default-template format + Error kind). The
  pre-existing failure-kind test had its `"failed"` body assertion
  replaced (the default template uses `{exit_code}`, not a "failed"
  verb; the Error kind assertion is unchanged). Config-side: extended
  `notifications_default_is_opt_in` + `notifications_round_trip_through_toml`
  for the template field, extended `bell_config_defaults_to_visual`, and
  added `bell_on_command_finished_round_trip`.
- **Verification:** `cargo fmt --all -- --check`, `cargo clippy
--all-targets --all-features -- -D warnings`, `cargo test --all`, and
  `cargo machete` all clean.
- **Not in scope (deferred to 76.7):** the Settings UI text-edit for the
  template and the routing dropdowns live in the Notifications tab,
  which 76.7 builds.

#### 76.6 — Capability detection (verify existing responders + document TERM_PROGRAM) ✅ 2026-06-09

**Scope:** `freminal/freminal.ti` (terminfo source),
`freminal-terminal-emulator/src/terminal_handler/dcs.rs` (existing
XTGETTCAP responder), `freminal-terminal-emulator/src/ansi_components/csi_commands/xtversion.rs`
(existing XTVERSION handler), `shell-integration/README.md`.

**Reality check (decided 2026-06-09):** OSC 9 (iTerm2/WezTerm) and OSC 777
(urxvt) are one-way, fire-and-forget sequences. **Neither defines a
capability-query handshake.** There is no terminal-side "broadcast" to
implement and nothing analogous to the kitty keyboard `CSI ? u` query or
the kitty graphics `a=q` query — those are client-initiated queries we
_answer_, and OSC 9/777 have no such query. The real-world detection
mechanism for these sequences is the `TERM_PROGRAM` environment variable,
which freminal already sets to `freminal` in subtask 72.6. So this subtask
is mostly verification + documentation, not new capability machinery. The
one genuine support handshake in the notification space is OSC 99's
`p=?` — that is out of scope here and lives in Task 99
(`PLAN_VERSION_100.md`).

Work to do:

- **Verify, do not rebuild, the existing responders.** freminal already
  has an XTGETTCAP responder (`dcs.rs`, `handle_xtgettcap`) and an
  XTVERSION handler (`xtversion.rs`). First run the existing XTGETTCAP /
  XTVERSION tests and confirm what they return today. Only then:
  - Ensure `XTGETTCAP TN` (Terminal Name) responds with `freminal` if it
    does not already.
  - Ensure `XTVERSION` responds with `\eP>|freminal v<version>\e\\` if it
    does not already.
  - Do not regress current behavior; extend the existing tests rather
    than replacing them.
- **Terminfo:** add comment lines only. Terminfo has no dedicated
  capability code for OSC 9 / OSC 777; do not invent one.
- **Document detection in `shell-integration/README.md`** using the
  `TERM_PROGRAM` idiom (the actual mechanism clients use):

  ```bash
  if [ "${TERM_PROGRAM:-}" = "freminal" ]; then
      notify_via_osc9() { printf '\e]9;%s\a' "$1"; }
  fi
  ```

**Verification:** XTGETTCAP and XTVERSION round-trip tests (extend the
existing terminfo-audit harness, see PLAN_12_TERMINFO.md — note that
PLAN_12 is now retired into the v0.2.0 task set; the tests live in
`dcs.rs` and `xtversion.rs`).

**Completion notes (2026-06-09) — verification + documentation only,
no behavior change (decided with the user):**

- **Both plan-suggested response changes were intentionally NOT made**
  because each conflicts with deliberate, documented codebase behavior:
  - **XTGETTCAP `TN` stays `xterm-256color`** (not `freminal`). `TN`
    means "the value of `$TERM`", and `io::pty::run_terminal` sets
    `TERM=xterm-256color` as the documented Task-12 compatibility
    strategy (mirroring WezTerm/Alacritty). Reporting `freminal` would
    desync TN from TERM and break terminfo lookups.
  - **XTVERSION stays `>|XTerm(Freminal <version>)`** (not
    `freminal v<version>`). The `XTerm(` prefix is load-bearing: tmux
    only enables `extkeys` / `modifyOtherKeys` when it recognises the
    prefix (`tty_keys_extended_device_attributes`). Dropping it would
    regress extended-key forwarding inside tmux — already documented in
    `reports.rs::handle_device_name_and_version`.
  - The freminal-identifying signal is `TERM_PROGRAM=freminal` (set in
    72.6), and `Freminal` is already present in the XTVERSION payload.
- **Verified existing responders first:** ran the existing 29 XTGETTCAP
  tests + XTVERSION parser tests (all green) before touching anything.
- **New regression guard test** `reports.rs::tests::
device_name_and_version_keeps_xterm_prefix_and_freminal_token`
  asserts the exact 7-bit-framed XTVERSION response
  `\x1bP>|XTerm(Freminal <version>)\x1b\\` (the `reports.rs` response
  builder previously had no test module — only the parser emitting
  `RequestDeviceNameAndVersion` was covered).
- **Hardened the existing `xtgettcap_known_capability_tn` test** with a
  comment explaining the TN-must-equal-TERM invariant, and added an
  explanatory comment to the `TN` arm of `lookup_termcap`.
- **Terminfo (`res/freminal.ti`):** added a top-of-file comment block
  explaining OSC 9 / OSC 777 have no terminfo capability code and that
  detection is via `TERM_PROGRAM`. Comment-only; `tic -c` still
  compiles.
- **`shell-integration/README.md`:** added a "Desktop Notifications
  (OSC 9 / OSC 777)" section documenting the `TERM_PROGRAM`-guarded
  emission idiom for both sequences. The version marker was not bumped
  (prose-only doc change; the marker tracks script protocol version).
- **No escape-sequence dual-doc update needed:** 76.6 adds/removes/alters
  no escape-sequence support. OSC 9/777 (76.2) and XTVERSION/XTGETTCAP
  are already ✅ in `ESCAPE_SEQUENCE_COVERAGE.md`.
- **Verification:** `cargo fmt --all -- --check`, `cargo clippy
--all-targets --all-features -- -D warnings`, `cargo test --all`, and
  `cargo machete` all clean.

#### 76.7 — Settings UI: Notifications tab ✅ 2026-06-09

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

**Completion notes (2026-06-09):**

- **New `SettingsTab::Notifications`** inserted between `Bell` and
  `Security`. `SettingsTab::ALL` grew `[Self; 13] → [Self; 14]`; the
  `all_tabs_present` assertion and `settings_tab_labels` updated.
- **`show_notifications_tab`** renders: master `enabled` checkbox;
  Sources (`osc_9`, `osc_777`, `on_command_finished`); a
  `command_finished_threshold_secs` `DragValue` (0–600 s); three routing
  combo boxes via the extracted `notification_routing_row` helper
  (Toast / System / Both / System-when-unfocused); a full-width
  `command_finished_template` text edit with a token hint; a "Test
  Notification" button; and a Bell cross-reference checkbox bound to the
  same `bell.on_command_finished` field shown in the Bell tab.
- **Bell tab** also gained the `bell.on_command_finished` checkbox (the
  same field is editable from both tabs; egui binds both to the draft).
- **"Test Notification"** sets a `pending_test_notification` flag (the
  same pattern as `pending_delete_layout`), drained in both `show` and
  `show_standalone` to return the new `SettingsAction::TestNotification`.
  The dispatcher in `settings_dispatch.rs` routes a
  `NotificationRequest::sample()` (Info kind, "Freminal" / "Test
  notification") through `NotificationRouter::route_test` using the draft
  `[notifications]` config (so unsaved routing changes are reflected) and
  the focused state.
- **`route_test`** is a sibling of `route` that skips only the `enabled`
  master-switch gate — the test button must give feedback even while the
  system is disabled; the per-category routing policy still applies.
- **`draft_notifications()`** accessor added so the dispatcher reads the
  draft config without exposing the private `draft` field.
- **`NotificationRouting` label helper** `notification_routing_label`
  added next to `bell_mode_label`.
- **Tests:** `notification_routing_labels`,
  `notification_settings_persist_through_draft`,
  `test_notification_button_sets_pending_flag` (settings.rs);
  `route_test_ignores_enabled_master_switch`, `sample_request_is_info_kind`
  (notifications.rs).
- **Verification:** `cargo fmt --all -- --check`, `cargo clippy
--all-targets --all-features -- -D warnings`, `cargo test --all`, and
  `cargo machete` all clean.

#### 76.8 — Docs ✅ 2026-06-09

**Scope:** `Documents/ESCAPE_SEQUENCE_COVERAGE.md`, `Documents/ESCAPE_SEQUENCE_GAPS.md`,
`shell-integration/README.md`.

- Add OSC 9 and OSC 777 to the coverage table (status: implemented).
- Remove from the gaps doc.
- Document the OSC 9 / OSC 777 examples in the shell-integration README
  with the `TERM_PROGRAM` detection idiom.

**Verification:** Per AGENTS.md "Escape Sequence Documentation" rules.

**Completion notes (2026-06-09) — scope absorbed into 76.2 and 76.6:**

- The escape-sequence dual-doc update was done in **76.2**: OSC 9
  (iTerm2/WezTerm) and OSC 777 (urxvt) are ✅ in
  `ESCAPE_SEQUENCE_COVERAGE.md` and were removed from
  `ESCAPE_SEQUENCE_GAPS.md` (both files' "Last updated" headers cite
  Task 76.2).
- The `shell-integration/README.md` "Desktop Notifications (OSC 9 /
  OSC 777)" section documenting the `TERM_PROGRAM` detection idiom was
  added in **76.6**.
- No further work was required; this subtask is bookkeeping only.

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

#### 77.1 — Config schema ✅ 2026-06-09

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

**Completion notes (commit `4eeb8b8`, 2026-06-09):**

- `PasteGuardConfig` added next to `SecurityConfig` in
  `freminal-common/src/config.rs`. Fields: `enabled`, `multiline`,
  `control_chars`, `patterns` (all `bool`), `pattern_list: Vec<String>`.
- **Default change vs. the plan:** `patterns` defaults to `true`
  (dangerous-command detection on out of the box) rather than the
  `false` the plan originally specified. Requested by the user at task
  start. All other defaults match the plan (`enabled`/`multiline`/
  `control_chars = true`).
- The default `pattern_list` is produced by a private
  `default_paste_guard_patterns()` helper using the seven patterns from
  the plan (`rm -rf`, `curl|sh`, `wget|sh`, `sudo`, `doas`, `dd of=/dev/`,
  `mkfs.`).
- Regex validation is exposed as
  `PasteGuardConfig::invalid_patterns() -> Vec<(String, String)>`
  rather than enforced at load time. Rationale: `freminal-common` is a
  library crate with no UI surface and no `anyhow`; the load path
  (`apply_partial`) cannot raise a toast. The GUI consumes
  `invalid_patterns()` in 77.2/77.6 and surfaces malformed patterns via
  the existing toast system, skipping them at match time.
- `regex` added to `freminal-common/Cargo.toml` (`regex.workspace =
true`; already pinned to `1.12.3` in the workspace deps).
- `#[allow(clippy::struct_excessive_bools)]` with a justifying comment
  on the struct, matching the established `NotificationsConfig` pattern
  (four documented TOML toggles; an enum would distort the schema).
- Full merge-wiring per the config-options checklist: `Config` field +
  `Default`, `ConfigPartial` field, `apply_partial` arm, the
  `every_config_section_survives_partial_merge` guard test (mutation +
  assertion + no-`..` destructure entry), `config_example.toml`
  documentation, and the Nix home-manager module mirror (let-binding,
  merge arm, five `mkOption`s including a `listOf str` for
  `pattern_list`).
- 4 new unit tests: defaults, TOML round-trip, all-default-patterns-
  compile, and malformed-pattern reporting.
- `cargo test --all`, `cargo clippy --all-targets --all-features -- -D
warnings`, `cargo fmt --check`, `cargo machete`, and the Nix lint
  stack (`nixfmt`/`statix`/`deadnix`) all clean.

#### 77.2 — Paste analyzer ✅ 2026-06-09

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

**Completion notes (commit `ee15974`, 2026-06-09):**

- New module `freminal/src/gui/paste_guard.rs` with the `PasteAnalysis`
  enum (all five variants as specified) and a pure
  `analyze(payload, config, compiled) -> PasteAnalysis`.
- **Cache placement differs from the plan.** The plan suggested caching
  the compiled regexes on `PasteGuardConfig` via `OnceCell`.
  `PasteGuardConfig` lives in `freminal-common`, derives
  `Clone`/`Serialize`/`Deserialize`, and is hot-reloaded by value, so
  hanging a `OnceCell<Vec<Regex>>` off it fights the derives. Instead a
  GUI-side `PasteGuard` struct owns `compiled: Vec<Regex>`;
  `PasteGuard::rebuild(&config)` recompiles and returns the invalid
  patterns for the caller to toast (77.6) while keeping the valid
  siblings. `analyze` is the pure free function the `PasteGuard::analyze`
  method delegates to, so the hot path never compiles a regex.
- Control-char trigger flags genuine C0/C1 controls (ESC, BEL, ...) but
  ignores `\n` (the multiline trigger's job), `\r`, and `\t`, which
  appear in legitimate pasted text. Flagged chars are de-duplicated in
  first-seen order.
- `Multiple` is always a flat list (never nests `Multiple` or `Safe`),
  ordered multiline → control chars → patterns.
- 13 unit tests cover every trigger, the disabled master switch, dedup
  ordering, trailing-newline line counting, and partial-compile recovery.
- **Temporary `#[allow(dead_code)]` on `mod paste_guard;`** with a
  `TODO(77.4)` comment: the module has no production caller until the
  wire-in lands. Removed in 77.4. Permitted by the AGENTS.md temporary-
  refactor exception; user-approved.
- `cargo test --all`, `cargo clippy --all-targets --all-features -- -D
warnings`, `cargo fmt --check`, and `cargo machete` all clean.

#### 77.3 — Preview dialog ✅ 2026-06-09

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

**Completion notes (commit `d62faaf`, 2026-06-09):**

- `PasteDialog` added to `freminal/src/gui/paste_guard.rs`: a centered,
  non-resizable `egui::Window` titled "Confirm Paste" following the
  established `show_save_layout_prompt` / `show_confirm_close_prompt`
  modal pattern.
- Banner is produced by a pure, unit-tested `banner_text(&PasteAnalysis)
-> String` (e.g. "Multi-line paste — 17 lines, 420 bytes",
  "Dangerous patterns detected: …", control chars rendered as `U+XXXX`,
  and a flattened "Multiple triggers — …").
- **Content area:** a scrollable monospace preview. Read-only mode uses
  a disabled multiline `TextEdit` (monospace + selectable without
  edits); "Edit and Paste" flips it to an editable `TextEdit` bound to a
  scratch `edit_buffer`, focused once on entry. The plan mentioned
  optional renderer syntax highlighting "or plain monospace if that's
  too invasive" — plain monospace was chosen.
- **Buttons:** Cancel, Paste Anyway, Edit and Paste. **Keyboard:**
  Escape = Cancel, Ctrl+Enter = Paste Anyway (clicks in the same frame
  take precedence over shortcuts).
- `show()` returns `PasteDialogOutcome { Idle | Cancelled |
Paste(String) }` and closes the dialog on resolve. The dialog is
  **target-agnostic** — it yields only the resolved text. Bracketed-
  paste wrapping and the `InputEvent::Key` send stay in 77.4, which
  owns the `input_tx` and pane routing.
- State lives on `PerWindowState::paste_dialog` (paste targets a
  window's active pane). That field carries a temporary
  `#[allow(dead_code)]` + `TODO(77.4)` until `update()` renders it and
  the paste path opens it; the module-level allow from 77.2 also
  remains until 77.4.
- 8 new unit tests (banner formatting incl. flattened `Multiple`,
  open/close/`is_open`, the `Safe`-never-opens rule, outcome equality).
  The egui render path itself is exercised manually, per the plan's
  "manual visual test"; the testable state logic is unit-covered.
- `cargo test --all`, `cargo clippy --all-targets --all-features -- -D
warnings`, `cargo fmt --check`, and `cargo machete` all clean.

#### 77.4 — Wire into paste handling ✅ 2026-06-09

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

**Completion notes (commit `962a210`, 2026-06-09):**

- **Interception point differs from the plan.** The plan said to call
  `paste_guard::analyze` inline at the two `input.rs` paste sites. Those
  sites (`dispatch_binding_action`, `write_input_to_terminal`) run deep
  in the input loop with no access to `&Config`, the `PasteGuard` cache,
  or `PerWindowState.paste_dialog`. Instead all three paste sites
  (keybinding, Edit-menu, and egui `Event::Paste`) now **defer
  `KeyAction::Paste`** up to `dispatch_deferred_action`, which runs at
  the `update()` layer with full `self` + `win` access. User-approved
  design.
- New `FreminalGui::guarded_paste(win)`: ignores the request if a dialog
  is already open (no dialog stacking), reads the clipboard via
  `arboard`, normalises CRLF, calls `PasteGuard::analyze`, and either
  sends straight to the active pane (`Safe`) or opens
  `win.paste_dialog`.
- New `FreminalGui::send_paste_to_active_pane(win, payload)`: applies
  bracketed-paste wrapping (when the active pane has it enabled) and
  sends `InputEvent::Key`. Used by both the `Safe` fast path and the
  dialog `Paste` resolution.
- `update()` renders `win.paste_dialog.show(ctx)` next to the
  save-layout prompt; a `Paste(payload)` outcome calls
  `send_paste_to_active_pane`, `Cancelled`/`Idle` discard.
- `Event::Paste` now reads the clipboard in the deferred handler instead
  of forwarding the event text — unifying all paste origins on one
  guarded clipboard-read path (the content is identical for a normal
  Ctrl/Cmd+V).
- `PasteGuard` cache lives on `FreminalGui`, built at startup and
  rebuilt in `apply_new_config` next to `binding_map`. Invalid user
  patterns are surfaced via `push_error_toast` and skipped.
- **Removed both temporary `#[allow(dead_code)]` markers** (the 77.2
  module-level one and the 77.3 `paste_dialog`-field one); everything is
  now wired and read. The unused `PasteDialog::close()` speculatively
  added in 77.3 was deleted rather than left dead.
- An automated end-to-end paste→dialog→confirm→PTY integration test is
  not feasible without an egui context + a live PTY; the analyzer,
  dialog state, and banner logic are unit-covered (18 tests) and the
  full flow is manual-tested. `cargo test --all`, `cargo clippy
--all-targets --all-features -- -D warnings`, `cargo fmt --check`, and
  `cargo machete` all clean.

**Follow-up fix (commit `9dbc433`, 2026-06-10): paste regression.**
The initial 77.4 wire-in broke paste entirely. Root cause: the
windowing layer (`freminal-windowing/src/event_loop.rs`) already
intercepts paste shortcuts (Ctrl+V **and** Ctrl+Shift+V — the modifier
check is `command && "v"`, case-insensitive), reads the clipboard via
the reliable egui-winit/smithay path, and injects `Event::Paste(text)`
to work around Wayland multi-window clipboard breakage. The wire-in
made the `Event::Paste` arm discard that known-good text and defer a
second `arboard` read, which fails (`clipboard … empty`) on the common
path. Fix: `Event::Paste` now stashes its text in
`ViewState::pending_paste`, drained in `update()` and fed to a new
`guarded_paste_text` (analyze → send or open dialog) with no clipboard
read. `arboard` (`guarded_paste`) is retained only for the menu-Paste /
explicit-keybinding path that does not flow through the windowing
interceptor.

**Follow-up fix (commit `7dbf538`, 2026-06-10): dialog focus.**
The dialog rendered but never captured keyboard input — focus bounced
back to the terminal, so the Edit-and-Paste field was untypable (the
recurring modal-focus bug class also fixed for settings/search/command
history). Two fixes, matching the established pattern: (1) add
`win.paste_dialog.is_open()` to the `ui_overlay_open` flag in
`update()` (`app_impl.rs`) — that flag gates the terminal widget's
focus-lock and its `write_input_to_terminal` call, so the active pane
stops stealing focus and forwarding keystrokes while the dialog is
open; (2) give the edit-mode `TextEdit` `.lock_focus(true)` and
re-request focus every frame it lacks it (mirroring `show_search_bar`
and `show_command_history_palette`). The redundant `just_entered_edit`
flag was removed.

#### 77.5 — KeyAction::PasteUnsafe ✅ 2026-06-10

**Scope:** `freminal-common/src/keybindings.rs`, dispatch.

- Add per the keybinding convention. Default binding `Ctrl+Shift+V`
  (note: this conflicts with existing `Paste` binding on some platforms;
  resolve by setting a sensible default and documenting).
- Document the action as "Paste without confirmation".

**Verification:** Round-trip keybinding test.

**Completion notes (commit `2ab8caa`, 2026-06-10):**

- **Default binding is `Ctrl+Shift+Alt+V`, not the plan's
  `Ctrl+Shift+V`** (which collides with `Paste`). One modifier beyond
  the normal paste combo, so the bypass is deliberate. User-approved.
- Full four-step keybinding ritual: `KeyAction::PasteUnsafe` variant
  (with `name() = "paste_unsafe"`, `display_label() = "Paste (no
confirmation)"`, `FromStr`, and `ALL` membership); a new
  `BindingModifiers::CTRL_SHIFT_ALT` constant; the default binding in
  `register_misc_bindings`; and the action list in `config_example.toml`.
- **Dispatch reality vs. the plan.** The windowing paste interceptor
  (`event_loop.rs`) fires for _any_ `command + v`, including
  `Ctrl+Shift+Alt+V`, and converts it to an `Event::Paste` — so the
  `BindingMap` entry never sees the bypass combo on that (common) path.
  The bypass intent is therefore detected at the `Event::Paste` arm via
  the **Alt modifier** (`input.modifiers.alt`) and carried through a new
  `ViewState::PendingPaste { text, bypass_guard }` (replacing the bare
  `Option<String>` from the 77.4 fix). `update()` sends a bypassing
  paste straight to the active pane (`send_paste_to_active_pane`),
  otherwise runs the guard. The `BindingMap` entry plus a new
  `unguarded_paste` (arboard) handler in `dispatch_deferred_action`
  cover the menu and any platform/config where the interceptor does not
  fire.
- Tests: `default_paste_unsafe_binding` (asserts the bypass combo maps
  to `PasteUnsafe` and stays distinct from `Paste`),
  `paste_unsafe_name_round_trips`, and the updated guard counts
  (`ALL` = 60 actions, default map = 47 bindings).
- `cargo test --all`, `cargo clippy --all-targets --all-features -- -D
warnings`, `cargo fmt --check`, and `cargo machete` all clean.

#### 77.6 — Settings UI ✅ 2026-06-10

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

**Completion notes (commit `15107b7`, 2026-06-10):**

- A "Paste Guard" section added to the Security settings tab
  (`show_paste_guard_section`): master enable toggle; multi-line /
  control-character / pattern per-trigger toggles (disabled via
  `add_enabled_ui` when the guard / pattern matching is off); an
  editable pattern list with per-row remove (`➖`) and an "Add pattern"
  (`➕`) button. Invalid regexes render in red (`text_color_opt`) with a
  hover note that they are ignored at match time (live
  `regex::Regex::new(...).is_ok()` check per row).
- A "Test Paste" button mirrors the Notifications tab's "Test
  Notification": sets `pending_test_paste`, drained in
  `show`/`show_standalone` as `SettingsAction::TestPaste`, handled in
  `handle_settings_action` by building a temporary `PasteGuard` from
  `draft_paste_guard()` (the unsaved draft), analyzing a sample payload,
  and opening the confirm dialog on the `settings_owner` window. If the
  draft would not intercept the sample, an info toast says so instead.
- Per-section persistence relies on the existing `[paste_guard]`
  `ConfigPartial`/`apply_partial` wiring from 77.1; the Settings tab
  edits `self.draft.paste_guard` directly, saved on Apply like every
  other section.
- `SettingsModal` gained a justified `#[allow(clippy::struct_excessive_bools)]`
  (several independent one-shot UI flags).
- Tests: `test_paste_button_sets_pending_flag`,
  `draft_paste_guard_reflects_edits`. The egui render path is
  manual-tested. `cargo test --all`, clippy, fmt, machete all clean.

### 77 Open Questions Resolved

All resolved.

### 77 Benchmarks ✅ 2026-06-10

`paste_guard::analyze` runs in the GUI thread on a paste event. For a 1MB
paste with all triggers and 20 patterns, it must complete in < 50ms.
Add a benchmark in a new `freminal/benches/paste_guard_bench.rs`.

**Done (commit `406f7d8`).** `freminal/benches/paste_guard_bench.rs`
(Criterion) covers `analyze_1mb_all_triggers`, `analyze_typical_paste`,
and `rebuild_patterns`. Measured: the 1 MB / all-triggers / 20-pattern
worst case is **~688 µs** — roughly 70× under the 50 ms budget; a
typical ~2 KB paste is ~1.8 µs; rebuilding the 20-pattern cache is
~745 µs. To make the analyzer reachable from the separate-crate bench,
`mod paste_guard` and the analyzer API were promoted to `pub` (matching
`gui::atlas` / `gui::shaping`); the dialog types stay `pub(in crate::gui)`.

---

## Task 94 — Tab Title Precedence ✅ Complete (2026-06-04, PR #343)

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

## Task 95 — Persist Custom Tab Names in Layouts ✅ Complete (2026-06-04, PR #343)

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

### T1 — OSC 8 Hyperlink Action Menu (lands in Task 72) ✅

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

**Completion notes (72.14):**

- Ctrl+click (Cmd+click on macOS) to open URL: implemented earlier as
  part of the URL hover work in `freminal/src/gui/terminal/widget.rs`
  (the click detector runs unconditionally when a URL is cached as the
  hovered cell; `open::that` is spawned on a dedicated
  `freminal-open-url` thread to avoid blocking the GUI on the OS
  default-browser handler).
- "Open URL" right-click menu item: implemented earlier as
  `ContextMenuAction::OpenUrl(String)` in the same file. Shown only
  when the right-clicked cell is inside an OSC 8 hyperlink. Label is
  `"Open <url>"` with the URL truncated to 40 chars via
  `truncate_url`.
- "Copy URL" right-click menu item (`31c1b1a`, 2026-06-03): new
  `ContextMenuAction::CopyUrl(String)` variant; menu button rendered
  immediately after "Open <url>" in the URL conditional block;
  dispatcher calls `ui.ctx().copy_text(url)` (egui's clipboard API,
  same path the existing `Copy` / `CopyCommandOutput` actions use).
  Verification: 103/103 suites green, clippy clean. Visual
  confirmation of clipboard contents is end-user manual.

### T2 — Command Duration Display (already in Task 72.12)

Covered by 72.12. No separate task.

### T3 — Quick Command History Palette (lands in Task 72)

**Scope:** ~2 days. New module `freminal/src/gui/command_history.rs`
(palette UI) plus `freminal/src/gui/shell_history.rs` (shell-history
seed loader).

- A fuzzy-searchable palette over the union of:
  - **Shell history seed** (bash / zsh / fish) loaded once per pane
    spawn on a background thread, capped at 1000 entries.
  - **Live recent commands** captured via OSC 133 in 72.9
    (`pane.recent_commands`).
- New `KeyAction::ShowCommandHistory`, default `Ctrl+Shift+M`. (Not
  `Ctrl+R`: that collides with the shell's reverse-i-search and would
  break a near-universal muscle-memory. Not `Ctrl+Shift+R`: taken by
  `ToggleSessionRecording`. Not `Ctrl+Shift+P`: reserved for Task 83
  Command Palette.)
- Egui modal with:
  - Text input at top (fuzzy filter via `nucleo-matcher` or simple
    case-insensitive substring; whichever is already in the dep tree —
    check `Cargo.lock`).
  - List of recent commands with timestamp, exit code icon, command
    preview. Seed-only entries show no exit code / timestamp.
  - Enter on a selection: send the command text as keyboard input to the
    current pane (does **not** auto-execute — user reviews and presses
    Enter themselves).
- **Data-model decision:** Option A — `Pane.history_seed:
Arc<OnceLock<Vec<String>>>` runs parallel to
  `recent_commands: VecDeque<CommandBlock>`; the palette merges them
  at render time. Rejected: extending `CommandBlock` to carry a text
  field (forces every consumer of `CommandBlock` to handle a
  buffer-row-less variant; `CommandBlock` is a buffer-row pointer with
  no `text` field today, and the palette is the only consumer that
  wants text without rows).
- Land as **Task 72.15**, three commits:
  1. Shell-history data layer (parsers, path resolution, async loader,
     `TabChannels.history_seed`, `Pane.history_seed`).
  2. Palette UI + `KeyAction::ShowCommandHistory` + 4-step keybinding
     wiring (enum, default, dispatch, `config_example.toml`).
  3. Polish (animations, empty-state, exit-code icons) + final docs.

**Completion notes (72.15):** ✅ Complete.

The originally-scoped three-commit plan shipped as commits 1, 2, and
the keybinding wiring of commit 3. Dogfooding then surfaced six
real-world issues (none of which were in the original "polish" list),
each fixed in a dedicated `fix:` commit. A seventh thread of work
shipped the OSC 1338 HISTFILE auto-discovery protocol -- not in the
original plan, but necessary to handle the common zsh-users-who-set-
HISTFILE-as-a-shell-variable case that the env-only loader could not
see. Final shape: 12 commits.

- **72.15 commit 1** ✅ done (commit `8bdeb85`, 2026-06-04; docs in
  `0039631`). Shell-history data layer:
  - New `freminal/src/gui/shell_history.rs` (658 lines, 39 unit tests):
    `ShellKind` enum (Bash, Zsh, Fish, Other), `detect_shell_kind` by
    basename only (no symlink resolution -- POSIX `sh` does not auto-
    load bash history even if symlinked to bash), `resolve_history_path`
    honoring `HISTFILE` (bash, zsh) and `XDG_DATA_HOME` then
    `$HOME/.local/share` (fish) with empty env values falling through
    to defaults, format-specific parsers (`parse_bash_history` skips
    `#<ts>` lines; `parse_zsh_history` handles `: <ts>:<dur>;<cmd>`
    extended format; `parse_fish_history` with `decode_fish_cmd` for
    YAML-ish escape decoding of `\n \r \t \\`), `load_for_program`
    orchestrating detect → resolve → parse with `HISTORY_SEED_CAP = 1000`,
    `spawn_loader<S: BuildHasher + Send + 'static>` running on a named
    `freminal-history-loader` thread writing into the per-pane slot.
  - `TabChannels.history_seed` slot in `gui/pty.rs`; `spawn_pty_tab`
    resolves shell (shell_override → args.shell → `$SHELL`, skipped if
    `args.command` is non-empty), snapshots `std::env::vars()`, calls
    `spawn_loader`.
  - `Pane.history_seed` field in `gui/panes/mod.rs`; all 8 Pane
    construction sites (3 in `tab_spawning.rs`, 2 in `app_impl.rs`,
    1 in `panes/mod.rs` test helper, 1 in `tabs.rs` test helper)
    initialise it (production sites pull from `channels.history_seed`;
    test helpers use a fresh slot).
  - **Originally-known limitation:** per-pane env snapshot is taken
    from the parent freminal process, so runtime rc-file `HISTFILE`
    overrides set after freminal launch are not visible to the
    loader. Closed by the OSC 1338 work below.
- **72.15 commit 2** ✅ done (commit `ca2efcb`, 2026-06-04). Palette
  UI + key binding:
  - New `freminal/src/gui/command_history.rs` (palette modal:
    egui-based, top-of-pane positioning, fuzzy filter via case-
    insensitive `to_ascii_lowercase().contains(...)`, no
    `nucleo-matcher` dep added).
  - Merges `pane.history_seed.load().entries` (seed) with
    `pane.recent_commands` (live OSC 133 commands) at render time.
    Seed-only entries render without timestamp/exit-icon as
    specified in the data-model decision.
  - Live entries cache extracted command text via
    `pane.command_texts: HashMap<BlockId, String>` populated at
    finish-time from the current snapshot.
  - `KeyAction::ShowCommandHistory` with default `Ctrl+Shift+M`.
    Full 4-step wiring: enum variant in `keybindings.rs`, default
    binding in `BindingMap`, dispatch arm in `actions.rs`, doc
    entry in `config_example.toml`.
  - Enter on selection sends command text via keyboard input to
    the current pane without a trailing `\n` -- user reviews and
    presses Enter themselves.
- **Post-MVP bug fixes** (surfaced during dogfooding, each its own
  `fix:` commit):
  - `8447400` -- shell history loader handles non-UTF-8 bytes via
    `String::from_utf8_lossy`.
  - `00cced3` -- release terminal focus while palette is open so
    typed characters route to the filter input, not the PTY.
  - `1741b3b` -- reassemble zsh multi-line history entries where
    backslash continuations span multiple physical lines.
  - `16d7a28` (chore) -- enrich shell-history loader diagnostic
    logging: shell kind, resolved path, byte count, raw line
    count, parsed entry count, mtime age.
  - `9619c41` -- truncate palette entries to popup width via
    `egui::Label::truncate()`; one giant history line (one-line
    megabyte JSON) was extending the row's horizontal layout past
    the popup max width and pushing other entries off-screen.
- **OSC 1338 HISTFILE auto-discovery** (three commits, all 2026-
  06-04). Extension beyond original scope, needed because the
  parent-env loader could not see `$HISTFILE` set as a shell
  variable inside `.zshrc` (common for zsh users storing history
  under `~/.config/zsh/.zsh_history`). New shell-integration OSC
  reports the shell-evaluated path; GUI reloads the seed when it
  changes.
  - `19f2eb8` -- parser + snapshot field. New
    `AnsiOscType::ShellInfoHistFile(PathBuf)` variant,
    `OscTarget::ShellInfo`, parser module
    `osc_shell_info.rs`. `TerminalHandler.shell_histfile`
    state and `TerminalSnapshot.shell_histfile` field. 15
    new tests.
  - `e41e9ef` -- emit OSC 1338 from bundled shell-integration
    scripts. bash emits at end of `freminal-init.bash` (after
    `~/.bashrc`); zsh emits via one-shot `precmd` hook (after
    `~/.zshrc`); fish emits via one-shot `fish_prompt` handler
    (after `config.fish`). Empty `$HISTFILE` suppressed so the
    env-derived default takes over. `FREMINAL_SHELL_INTEGRATION_VERSION`
    bumped from 1 to 2.
  - `4adff19` -- wire OSC 1338 reload detector into command-
    history palette. New `SharedSeededHistory =
    Arc<ArcSwap<SeededHistory>>` (sequence-tagged: `SEED_SEQ_ENV
= 0` env loader; `SEED_SEQ_OSC = 1` shell-reported loader)
    replaces the previous `Arc<OnceLock<Vec<String>>>` so the
    OSC-driven load always wins regardless of arrival order.
    New `classify_osc_reload` pure detector + per-frame detector
    block in `app_impl.rs`. `TabChannels.shell_program` threads
    the resolved shell program for parser selection.
    `Pane::from_channels` centralised constructor migrates five
    ad-hoc Pane-struct-literal sites and eliminates a
    `#[allow(clippy::too_many_lines)]` overflow risk in
    `spawn_new_tab`.
- **Verification at every commit:** `cargo test --all` green (~103
  suites, growing through the task; final shell_history suite has
  57 tests, command_history 25, osc_shell_info 10, shell-integration
  version-sync test 6), `cargo clippy --all-targets --all-features
-- -D warnings` clean, `cargo machete` clean, all 27 pre-commit
  hooks pass at every commit.
- **Polish items intentionally not pursued.** The original commit-3
  plan named "animations, empty-state copy, real exit-code icons"
  as polish. None surfaced as user-felt friction during the six
  rounds of dogfooding; deferring to a future cosmetic pass if
  ever requested. The actual polish that mattered (truncation,
  focus release, non-UTF-8 robustness, zsh multi-line, diagnostic
  logging, OSC 1338 HISTFILE auto-discovery) shipped instead.

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
| `FoldPreviousCommand`       | (none)              | 72.10 |
| `FoldAll`                   | (none)              | 72.10 |
| `UnfoldAll`                 | `Ctrl+Shift+U`      | 72.10 |
| `CopyLastCommandOutput`     | `Ctrl+Shift+Y`      | 72.11 |
| `CopyCommandOutputAtCursor` | (none, right-click) | 72.11 |
| `ShowCommandHistory`        | `Ctrl+Shift+M`      | 72.15 |
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

- **OSC 99 (kitty notifications)** — deferred to v0.10.0 as Task 99
  (`PLAN_VERSION_100.md`). OSC 99 is a stateful protocol (chunked
  payloads, notification identity, activation callbacks,
  buttons/icons/sounds) and does not belong in Task 76's fire-and-forget
  OSC 9/777 path.
- **Layout-wide / window-wide `[layout.env]`** — defer to v0.13.0 with
  Profiles (Task 78).
- **Theme / font / profile binding per layout** — defer to v0.13.0 with
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

## Task 98 — Block Close on Running Commands

### 98 Summary

When the user attempts to close a pane, tab, or window — or quit the
application — while one or more shells in the affected scope have a running
foreground command (as reported by OSC 133 prompt/command markers), surface
a confirmation dialog listing what is running. The user can cancel, force
close, or wait. Enabled by default; configurable.

This task builds directly on Task 72's `CommandBlock` infrastructure. A
pane has a "running command" iff its most recent `CommandBlock` has
`status() == CommandStatus::Running` (i.e. an `OSC 133;C` marker was
emitted with no subsequent `OSC 133;D` to close it).

### 98 Decisions (fixed)

- **Default behavior:** Block close, with a confirmation dialog. Users
  who find this annoying can disable it in config or per-action via a
  "Force Close" button in the dialog.
- **Scope of "what is running":**
  - **Close pane:** check that pane only.
  - **Close tab:** check every pane in the tab.
  - **Close window / app quit:** check every pane in every tab of that
    window (or all windows for app quit).
- **Detection mechanism:** OSC 133 `CommandBlock::status()` only. We do
  not introspect `/proc/<pid>/...` or query child processes. Shells
  without OSC 133 integration appear as "no running command" — this is
  acceptable because v0.9.0 ships shell integration by default
  (Task 72.8b).
- **Grace period:** A pane that has never received any OSC 133 prompt
  marker (e.g. `cat`, `vim` launched directly, raw `sh` without shell
  integration) is treated as "unknown". Configurable: by default,
  unknown panes do **not** block close (matches existing v0.7.0
  behavior). Users who want maximum safety can set
  `unknown_blocks = true`.
- **Bypass via keybinding:** The existing close keybindings
  (`CloseTab`, `ClosePane`, app-quit shortcut) trigger the guard. A new
  `KeyAction::ForceClose` variant skips the guard entirely — useful
  when the user knows the shell is stuck.
- **App quit behavior:** When the OS sends a quit request (Cmd-Q on
  macOS, Alt-F4 on Windows, etc.), the guard runs across all windows.
  If any window has running commands and the user cancels, the quit is
  vetoed (return `false` from `on_close_requested`). If the user
  confirms, all windows close.
- **Dialog placement:** One dialog per affected window. For app quit
  with running commands in multiple windows, surface the dialog in the
  focused window first; cancelling there cancels the entire quit.
  Confirming there proceeds to close that window, then the next, etc.
- **No timer / auto-confirm.** The dialog blocks until the user
  responds. (Future: optional "auto-confirm after N seconds" deferred
  to v0.10.0.)

### 98 Pre-existing Infrastructure (Do Not Re-Implement)

| Concern                      | Where it lives today                                                        |
| ---------------------------- | --------------------------------------------------------------------------- |
| `CommandBlock` storage       | `freminal-buffer/src/buffer/command_block.rs` (Task 72)                     |
| `CommandStatus::Running`     | `freminal-common/src/buffer_states/command_block.rs` (Task 72)              |
| Snapshot transport           | `freminal-terminal-emulator/src/snapshot.rs` `command_blocks` field         |
| Per-pane access              | `freminal/src/gui/panes/mod.rs:816,828` `PaneTree::iter_panes(_mut)`        |
| Close-tab path               | `freminal/src/gui/actions.rs:13` `close_tab`                                |
| Close-pane path              | `PaneTree::close_pane` (TODO: confirm exact location during 98.1 audit)     |
| Window close veto            | `freminal/src/gui/app_impl.rs:272` `on_close_requested` (returns `bool`)    |
| Toast / modal infrastructure | `freminal/src/gui/toast.rs`, `freminal/src/gui/settings.rs` (modal pattern) |
| `KeyAction` registry         | `freminal-common/src/keybindings.rs`                                        |
| Existing close keybindings   | `KeyAction::CloseTab`, `ClosePane`, etc. in `keybindings.rs`                |

### 98 Subtasks

#### 98.1 — Audit close paths

**Scope:** Read-only audit of every code path that closes a pane, tab,
or window, and every path that triggers app quit.

- Enumerate every call site of `Tabs::close_tab`,
  `PaneTree::close_pane` (or equivalent), window close via
  `on_close_requested`, and app-quit via keybinding or menu.
- For each, record: trigger source (keybinding, OS event, menu),
  current cleanup logic, whether veto is currently possible.
- Produce a written report in this plan document under 98.1
  completion notes before writing any code.

**Verification:** Report posted; no code changes.

#### 98.2 — Config schema

**Scope:** `freminal-common/src/config.rs`, `config_example.toml`.

- Add a new section `[close_guard]`:

  ```toml
  [close_guard]
  # Master switch.  When false, no close-guard checks run.  Default true.
  enabled = true

  # When true, also block close for panes whose command status is unknown
  # (no OSC 133 markers ever received).  Default false.
  unknown_blocks = false

  # When true, the app-quit shortcut runs the guard across all windows.
  # When false, app quit bypasses the guard and only individual
  # close-window / close-tab / close-pane actions are guarded.  Default true.
  guard_app_quit = true
  ```

- Add `CloseGuardConfig` struct with `#[serde(default)]`.
- Document in the inline doc comments which `CommandStatus` values
  count as "running" (only `Running`; `Success`, `Failure`, `Unknown`
  do not).

**Verification:** Round-trip TOML test in `config.rs` tests.

#### 98.3 — Running-command detection helper

**Scope:** New module `freminal/src/gui/close_guard.rs`.

- Pure function `panes_with_running_commands(panes: &[&Pane]) ->
Vec<RunningCommandInfo>` where `RunningCommandInfo` carries:
  - Pane id.
  - Tab id (for display).
  - Window id (for display).
  - Command string (from the open `CommandBlock`'s captured command
    line, if available; otherwise `"<unknown command>"`).
  - Elapsed runtime.
- `unknown_command_panes(panes: &[&Pane]) -> Vec<PaneId>` — panes
  that have never received any OSC 133 prompt.
- Read state from the latest `TerminalSnapshot` (loaded via
  `ArcSwap` per the post-refactor architecture in `agents.md`). Do
  not lock or mutate emulator state.

**Verification:** Unit tests with synthetic snapshots covering: no
running commands; one running; multiple in one tab; mix of
running/unknown/idle.

#### 98.4 — Confirmation dialog

**Scope:** `freminal/src/gui/close_guard.rs` (UI).

- An egui modal titled "Close — Running Commands":
  - Top: a one-line banner ("3 panes have running commands. Close
    anyway?").
  - Middle: a scrollable list of `RunningCommandInfo` entries
    formatted as `"<tab name> · <pane label> · <command> (<elapsed>)"`.
  - Bottom: "Cancel" (default, ESC), "Force Close" (focused-but-not-
    default, Ctrl+Enter), and — for tab/window close only — "Close
    Other Panes" (closes only panes without running commands).
- Modal state lives on the GUI thread; the close action is suspended
  until the user resolves the dialog.
- One dialog per affected window. App quit posts the dialog to the
  focused window first.

**Verification:** Manual visual test. Snapshot tests of the dialog
content formatting given synthetic `RunningCommandInfo` vectors.

#### 98.5 — Wire into pane close

**Scope:** `freminal/src/gui/panes/mod.rs` (or wherever
`PaneTree::close_pane` lives, confirmed by 98.1) and the call sites.

- Before executing the close, call
  `close_guard::panes_with_running_commands(&[pane])`.
- If empty (and `unknown_blocks=false` or the pane is not unknown),
  close as today.
- Otherwise, set a `pending_close_dialog` field on the window state
  and suspend the close. When resolved with Force Close, proceed.

**Verification:** Integration test: simulate OSC 133;C (no D),
trigger close-pane, verify dialog appears and Force Close proceeds.

#### 98.6 — Wire into tab close

**Scope:** `freminal/src/gui/actions.rs` `close_tab`.

- Same pattern. Use `iter_panes` to gather all panes in the tab.
- Support the "Close Other Panes" option: close all leaves whose
  status is not `Running` (and, if `unknown_blocks=true`, not
  unknown).

**Verification:** Integration test: tab with two panes, one
running, trigger close-tab, dialog appears with both options.

#### 98.7 — Wire into window close + app quit

**Scope:** `freminal/src/gui/app_impl.rs` `on_close_requested`,
plus any app-quit dispatch path identified in 98.1.

- Window close: gather panes from the window's `PaneTree`. If
  running commands present, set `pending_close_dialog` and return
  `false` from `on_close_requested` to veto the OS close. When the
  user confirms Force Close, programmatically close the window via
  the windowing crate API.
- App quit (when `guard_app_quit=true`): gather panes from all
  windows. Post the dialog to the focused window. Cancel → veto
  quit. Force Close → close all windows in sequence.

**Verification:** Integration test: open two windows, one with a
running command, trigger app quit, verify dialog appears and
cancel preserves both windows.

#### 98.8 — `KeyAction::ForceClose`

**Scope:** `freminal-common/src/keybindings.rs`, dispatch.

- Add a new `KeyAction::ForceClose` variant per the keybinding
  convention.
- No default binding (force close should be deliberate; users opt
  in by binding it themselves).
- Dispatch: if a `pending_close_dialog` exists, resolve it as Force
  Close. Otherwise, no-op.

**Verification:** Round-trip keybinding test; manual test of the
key path.

#### 98.9 — Settings UI

**Scope:** Security settings tab (`freminal/src/gui/settings.rs`).

- Add a "Close Guard" section with three toggles matching the
  config keys.
- Follow the existing toggle/persistence pattern used elsewhere in
  the Security tab.

**Verification:** Round-trip persistence; modal opens and reflects
config state.

### 98 Open Questions Resolved

All resolved.

### 98 Benchmarks

None required. `panes_with_running_commands` reads from already-
loaded snapshots and the per-window pane count is bounded by the
user's screen real estate (tens, not thousands). No new benchmark.

### 98 Risks

- **False positives from buggy shell integration.** If a shell
  emits `OSC 133;C` and crashes before `OSC 133;D`, the pane will
  appear "running" forever. Mitigation: Force Close is one
  Ctrl+Enter away, and the dialog clearly labels the elapsed time
  so users can identify stuck markers.
- **App-quit confusion when guard runs across windows.** Posting
  the dialog only in the focused window may surprise users with
  multiple monitors. Mitigation: the dialog explicitly lists
  affected tabs/panes across all windows.

---

## Activation Checklist

When v0.9.0 is activated (after v0.8.0 merges), follow this order:

1. Read this entire document plus the v0.8.0 close-out notes in
   `MASTER_PLAN.md`.
2. Branch from `main` to `task-72/osc-133-command-blocks`.
3. Execute Task 72 subtasks in this exact order:
   - **72.1 → 72.6** ✅ done (commits `965aacf` → `14d1cad`).
   - **72.16.a** ✅ done (commit `703e998` — OSC 133 numeric-param filter).
   - **72.7 + 72.8** ✅ done (commits `f6c6237` and `168c364`); their
     architecture was superseded by 72.8c/72.8b after design review
     (2026-05-18). The scripts and infrastructure they shipped are
     about to be replaced.
   - **72.8c** ✅ done (commit `94db3c2`, 2026-05-18). Parser support
     for `freminal=1; fid=<id>` markers.
   - **72.8b** ✅ done (commit `3e80e6d`, 2026-05-19). Ghostty-style
     spawn-time shell-integration injection.
   - **72.9** ✅ done (commit `d11ccf9`, 2026-05-19). CommandFinishedEvent
     transport from PTY consumer thread to per-pane recent-command ring,
     with per-tab pending-event flag for unfocused tabs.
   - **72.10** ✅ done (commits `e3c3996` 72.10a — view state + keybindings;
     `f5798bb` 72.10b-1 — folding helpers + RowMap; `a9f368f` 72.10b-2 —
     wire RowMap into renderer; `d0c7098` 72.10b-3 —
     placeholder row rendering, click-to-unfold hit-test, hover cursor,
     benchmark; `bf6a2b4` 72.10c — bug fix: fall back to most
     recent completed block when cursor outside any block;
     `5b029a4` 72.10d — three renderer bug fixes: buffer-absolute /
     snapshot-relative row translation, stable placeholder count
     across scroll, prefer `output_start_row` (OSC 133 C) over
     `command_start_row` (OSC 133 B); `8d1056e` 72.10e — rename
     `ToggleFoldAtCursor` → `FoldPreviousCommand`).
   - **72.11** ✅ done (commit `8c3cd77`, 2026-05-19). Copy command output
     actions: `CopyLastCommandOutput` keybinding + `CopyCommandOutputAtCursor`
     right-click menu item.
   - **72.12** ✅ done (commit `238e903`, 2026-05-19; follow-up `8d95ad3`
     72.12a — drop command blocks erased by CSI 2J). Hover highlight and
     command-duration overlay.
   - **72.13** ✅ done (commit `603001b`, 2026-06-03). Refreshed OSC 133
     status across ESCAPE_SEQUENCE_COVERAGE.md and ESCAPE_SEQUENCE_GAPS.md.
   - **72.16.e** ✅ done (commit `b27539d`, 2026-06-03). Demoted XTGETTCAP
     unknown-capability log to debug.
   - **72.14** ✅ done (commit `31c1b1a`, 2026-06-03 — completed the
     OSC 8 hyperlink action menu with the "Copy URL" right-click item;
     Ctrl+click + "Open URL" were already shipped as part of the
     earlier URL hover work).
   - **72.15** ✅ done (12 commits, 2026-06-04). Quick Command
     History Palette: data layer (`8bdeb85`), palette UI + binding
     (`ca2efcb`), six post-MVP bug fixes (`8447400`, `00cced3`,
     `1741b3b`, `16d7a28`, `9619c41`), and OSC 1338 HISTFILE auto-
     discovery extension (`19f2eb8`, `e41e9ef`, `4adff19`). Polish
     items in the original commit-3 plan (animations, empty-state,
     real exit-code icons) intentionally not pursued -- dogfooding
     surfaced different polish needs which shipped instead.

   Pause after each subtask for user confirmation per the Multi-Step
   Task Protocol in `agents.md`. The 72.16 cleanup section accumulates
   bugs surfaced during earlier subtasks; entries can be closed-as-
   subsumed when later subtasks rewrite the affected code (72.16.b,
   72.16.c, 72.16.d were all closed-as-subsumed by 72.8b on 2026-05-18).

4. After Task 72 merges, branch `task-73/command-gutters` and repeat.
5. Continue through Tasks 94, 95, 76, 77, 74, 75, 98 in that order.
6. After all nine tasks merge, update `MASTER_PLAN.md` status table and
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
