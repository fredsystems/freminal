# PLAN_VERSION_080.md — v0.8.0 "Correctness & Polish"

## Goal

Before adding a single new feature, close every correctness and hygiene gap identified in the
post-v0.7.0 senior-engineer audit, and land every user-visible polish item from the UX audit
Top-20. No new thrusts begin until this version is shipped.

This version is explicitly _not_ about new features. It is about making sure the foundation
laid by v0.2.0–v0.7.0 is actually as solid as the `MASTER_PLAN.md` status columns claim it is,
and that no advertised feature silently does nothing.

---

## Task Summary

| #   | Feature                          | Scope  | Status  | Dependencies |
| --- | -------------------------------- | ------ | ------- | ------------ |
| 70  | Code Correctness & Hygiene Sweep | Large  | Pending | None         |
| 71  | UX Completeness & Polish Sweep   | Medium | Pending | None         |

Both tasks are independent and may be executed in parallel across sub-agents.

---

## Task 70 — Code Correctness & Hygiene Sweep

### 70 Overview

The post-v0.7.0 audit identified a series of `agents.md` rule violations and latent correctness
issues that were masked by previous "complete" status entries in `MASTER_PLAN.md`. This task
closes all of them.

Task 70 is organized by severity. All subtasks must be completed before Task 70 is considered
done — no subset deferral.

### 70 Subtasks

#### 70.A — Immediate Correctness Fix

- **70.A.1** — Fix codepoint truncation bug at
  `freminal-terminal-emulator/src/input.rs:511`. The expression `codepoint as u8` silently
  masks any non-ASCII character. Replace with explicit UTF-8 encoding. Add a regression test
  covering a non-ASCII keybinding / character input.

#### 70.B — CRITICAL: `anyhow` in Library Crates

`agents.md` "Error Handling" rule is explicit: `anyhow` is forbidden in `freminal-common`,
`freminal-buffer`, and `freminal-terminal-emulator`. Current violations span 10 files.

- **70.B.1** — Design typed error enums per module. At minimum:
  - `freminal-common`: `SgrParseError`, `ColorParseError`, `TcharError`,
    `WindowManipulationError`, `OscParseError`.
  - `freminal-terminal-emulator`: `AnsiParseError`, `InterfaceError`, `PtyError`,
    `InternalStateError`, `OscHandlerError`.
- **70.B.2** — Replace `anyhow::Result` and `anyhow::anyhow!` / `anyhow::bail!` call sites in
  all 10 files. Preserve error chains via `#[source]`.
- **70.B.3** — Move `anyhow` from `[dependencies]` to `[dev-dependencies]` in the three
  library crates. `freminal` (binary) and `xtask` retain it.
- **70.B.4** — Run full verification suite: `cargo test --all`, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo-machete`.

#### 70.C — CRITICAL: Relocate `TerminalHandler` to the Correct Crate

`freminal-buffer/src/terminal_handler/` currently contains escape-sequence parsing, mode state
machines, Kitty / iTerm2 / Sixel graphics protocols, DCS/APC parsing, shell integration, and
PTY write paths. This violates the `freminal-buffer` contract ("pure data model, no terminal
semantics"). The 5,741-line integration test file is similarly misplaced.

- **70.C.1** — Move the entire `freminal-buffer/src/terminal_handler/` subtree into
  `freminal-terminal-emulator/src/terminal_handler/`. No logic changes.
- **70.C.2** — Move `freminal-buffer/tests/terminal_handler_integration.rs` into
  `freminal-terminal-emulator/tests/`.
- **70.C.3** — Update all imports across the workspace.
- **70.C.4** — Verify `freminal-buffer` no longer depends on anything that expresses terminal
  semantics. Update `freminal-buffer/Cargo.toml` dependency list if dependencies were only
  needed by the relocated code.
- **70.C.5** — Run full verification suite; run all benchmarks to confirm zero perf regression
  (pure code movement).

#### 70.D — HIGH: Eliminate Production Panic Sites ✅ COMPLETE (2026-04-22)

`agents.md` forbids `unwrap`/`expect` and requires panics never to enforce invariants. All
surviving production panic sites must become typed errors.

- **70.D.1** ✅ — `freminal/src/gui/tabs.rs` — `active_pane()` / `active_pane_mut()` now return
  `Option<&Pane>` / `Option<&mut Pane>`; 27 non-test + 38 test callers updated across
  `tabs.rs`, `actions.rs`, `menu.rs`, `mod.rs`.
- **70.D.2** ✅ — `osc.rs:122` — `unreachable!()` replaced with `ParserOutcome::Invalid`.
- **70.D.3** ✅ — `csi.rs:184` — `unreachable!()` replaced with `ParserOutcome::Invalid`.
- **70.D.4** ✅ — `font_manager.rs` — introduced typed `FontManagerError` (thiserror) with
  variants `BundledFontCorrupt`, `ReparseFailed`, `FontRefUnavailable`. All 8 `unreachable!()`
  sites eliminated. `FontManager::new/rebuild/set_font_size/update_pixels_per_point` now
  return `Result`. Cascade through `FreminalTerminalWidget::new`. Runtime-path methods
  (`sync_pixels_per_point`, `apply_config_changes{,_no_ctx}`, `apply_font_zoom`) log+exit(1)
  on error. Unused `impl Default for FontManager` removed. Introduced private `CellMetrics`
  struct to avoid type_complexity lint.
- **70.D.5** ✅ — `gl_context.rs:176` — prime-and-fold pattern + log+exit(1).

#### 70.E — HIGH: Typed Errors for GPU Renderer

`freminal/src/gui/renderer/gpu.rs` currently returns `Result<(), String>` across 12 functions
with 22 `.map_err(|e| format!(...))` call sites.

- **70.E.1** — Introduce `GpuInitError`, `ShaderCompileError`, `TextureUploadError`,
  `BufferAllocError` enums. Use `#[source]` for chains.
- **70.E.2** — Convert all 12 functions and 22 call sites. Preserve log messages via `Display`
  impls on the error types.
- **70.E.3** — Surface shader compile errors to the user (see Task 71 item 4).

#### 70.F — HIGH: Thread Hygiene

- **70.F.1** — Name every spawned thread with
  `std::thread::Builder::new().name("freminal-...")`. Convention:
  - `freminal-pty-read-<tab_id>-<pane_id>`
  - `freminal-pty-write-<tab_id>-<pane_id>`
  - `freminal-input-pump-<window_id>`
  - `freminal-recording-writer`
  - `freminal-emulator-<window_id>`
  - any other worker threads discovered during the audit.
- **70.F.2** — Audit and document each thread's ownership of state and its channel endpoints.

#### 70.G — HIGH: Bounded Channels

Unbounded `crossbeam_channel` endpoints allow producers to exhaust memory before any
backpressure is applied. Affected: `InputEvent` channel, recording writer channel, and any
others discovered during audit.

- **70.G.1** — Replace unbounded channels with `bounded(N)` per endpoint. Size chosen per
  channel's traffic profile (input: small ~64; recording: larger ~4096).
- **70.G.2** — Choose a policy per channel: block briefly (input), or drop-with-counter and
  log a throttled warning (recording). Expose drop counters via a debug overlay or logs.
- **70.G.3** — Add a stress test that saturates each channel and confirms the drop / block
  policy behaves as designed.

#### 70.H — MEDIUM: Complete Cast Audit (Task 30 re-open)

Task 30 is marked complete in `MASTER_PLAN.md`, but ~165 raw `as` casts remain and ~32
allow-attributes (`#[allow(clippy::cast_*)]`) survive. Hot spots:
`freminal/src/gui/shaping.rs` (8× `as f32`), `freminal-buffer/src/buffer.rs` (39 casts).

- **70.H.1** — Full audit pass: every remaining `as` cast in production code is either
  justified by the type system as provably lossless, or replaced with the appropriate `conv2`
  trait (`ValueFrom` / `ValueInto` / `ApproxFrom` with `RoundToZero`).
- **70.H.2** — Delete every `#[allow(clippy::cast_*)]` attribute whose underlying cast has
  been replaced. Document any remaining allow with a `// SAFETY:` comment explaining why the
  conversion is lossless in context.
- **70.H.3** — Re-enable workspace-level `#![deny(clippy::cast_possible_truncation,
clippy::cast_sign_loss, clippy::cast_possible_wrap)]` in the three library crates.

#### 70.I — MEDIUM: Complete Bool-to-Enum (Task 26 re-open)

Task 26 missed one field.

- **70.I.1** — Replace `TerminalHandler::in_band_resize_enabled: bool` with
  `InBandResizeMode` from `freminal-common/src/buffer_states/modes/`. Update all call sites
  (`to_payload`, snapshot building, `send_terminal_inputs` if applicable).

#### 70.J — MEDIUM: Split Remaining God Files (Task 29 re-open)

Task 29 is marked complete, but three files are still oversized:

- `freminal-buffer/src/buffer.rs` — 11,012 lines
- `freminal-buffer/src/terminal_handler/mod.rs` — 5,188 lines (this becomes
  `freminal-terminal-emulator/src/terminal_handler/mod.rs` after 70.C)
- `freminal/src/gui/mod.rs` — 3,212 lines

- **70.J.1** — Split `buffer.rs` along natural seams: `buffer/cells.rs`, `buffer/cursor.rs`,
  `buffer/scrollback.rs`, `buffer/resize.rs`, `buffer/wrapping.rs`. `mod.rs` becomes a facade.
- **70.J.2** — Split `terminal_handler/mod.rs` along mode / response / dispatch seams. Do
  this after 70.C so it happens in the correct crate.
- **70.J.3** — Split `gui/mod.rs` along menu / settings dispatch / window lifecycle / input
  routing seams.
- **70.J.4** — Each split must leave `cargo test --all` passing at every commit. Run
  full benchmark suite after all three splits to confirm no regression.

#### 70.K — MEDIUM: Typed CSI Mode Discriminants

- **70.K.1** — Replace `handle_erase_in_display(mode: usize)` and
  `handle_erase_in_line(mode: usize)` with typed `EraseDisplayMode` and `EraseLineMode`
  enums. Provide `TryFrom<u16>` impls that surface an error for unknown modes.
- **70.K.2** — Audit the rest of `ansi_components/csi_commands/` for other `mode: usize`
  parameters and typify each.

#### 70.L — MEDIUM: Dead Code Attribute Cleanup

- **70.L.1** — `freminal/src/gui/terminal/mouse.rs:87` — either delete, wire up, or replace
  the bare `#[allow(dead_code)]` with a `// TODO(task-NN): ...` justification per rule.
- **70.L.2** — `freminal/src/gui/renderer/gpu.rs:73` — same.

#### 70.M — MEDIUM: Extract Duplicated Helpers

- **70.M.1** — Lift `param_or` (currently duplicated verbatim in
  `freminal-terminal-emulator/src/ansi_components/csi_commands/decstbm.rs` and
  `decslpp.rs`) into a shared `csi_commands/util.rs`. Update both call sites.

#### 70.N — MEDIUM: `send_or_log` Helper

- **70.N.1** — Introduce a small macro or helper that wraps the 38 repeated
  `match sender.send(...) { Err(e) => warn!(...) }` blocks. Prefer macro for zero-overhead
  inlining and to preserve `tracing` span context.
- **70.N.2** — Apply the helper at every call site.

#### 70.O — LOW: Convention & Polish

- **70.O.1** — Rename the 27 `get_*` accessor methods in production code to drop the `get_`
  prefix per Rust convention. Take care around deprecation aliases if any are public.
- **70.O.2** — Add `#[non_exhaustive]` to semver-sensitive enums: `KeyAction`, `InputEvent`,
  `WindowCommand`, and any other public enum whose variant set is expected to grow.
- **70.O.3** — Change public API `collect_text(text: &String)` to take `&str`.
- **70.O.4** — Refactor `build_background_instances` to take a `BackgroundFrame` struct
  rather than 20 positional parameters.
- **70.O.5** — Add clarifying doc comments to the `Arc<Mutex<WindowPostRenderer>>` and
  `Arc<Mutex<RenderState>>` sites explaining that these are GUI-thread-only and the `Mutex`
  exists solely for interior mutability inside the `PaintCallback` Arc-sharing mechanism.

### 70 Verification

A single full-workspace verification suite at the end of each subtask group:

1. `cargo test --all`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo-machete`
4. Full benchmark compile + run (Criterion) for subtasks that touch hot paths (70.C, 70.E,
   70.H, 70.J). Record before/after numbers in the completion notes per the benchmark rule in
   `agents.md`.

Task 70 is complete only when all subtasks 70.A through 70.O are individually complete and
committed on `task-70/correctness-sweep` (or similar), and the verification suite passes on
the final commit.

---

## Task 71 — UX Completeness & Polish Sweep

### 71 Overview

The UX audit identified 20 concrete issues ranked P0–P3. The most damaging are features that
are advertised (keybinding exists, settings list exists) but silently do nothing, and
error paths that log-and-disappear with no user feedback.

### 71 Subtasks

#### 71.P0 — Fix Advertised-but-Broken Features

- **71.1** — Wire up `RenameTab`. `freminal/src/gui/actions.rs:299-301` is currently a
  `trace!` no-op. Implement an inline text-entry overlay on the target tab (similar to
  a rename in a file manager). Persist the custom name on the tab struct; clear it if the
  shell sets a title via OSC 0/1/2.
- **71.2** — PTY spawn failure surface. When a shell fails to launch (bad path, missing
  binary, permission error), show an inline error row inside the tab (or a toast) with the
  error message and a retry button. Currently silent.
- **71.3** — Layout load failure surface. TOML parse errors and missing-file errors currently
  log and disappear. Show a modal dialog naming the layout file and the specific error.
- **71.4** — Shader compile error surface. When a custom shader fails to compile, show a
  dismissible error banner naming the shader file and including the first line of the GLSL
  error. Piggybacks on `GpuInitError` types introduced in 70.E.

#### 71.P1 — Discoverability

- **71.5** — Add Edit menu. Contains Copy, Paste, Select All, Find. Each item shows its
  current keybinding from `BindingMap`. Platform-appropriate placement (macOS menubar vs.
  Linux/Windows in-window menu bar).
- **71.6** — Add Help menu. Contains About (version + build hash, embedded via Task 16
  pipeline), "Report Issue…" (opens GitHub issue tracker URL), "Keybindings…" (jumps to
  Settings Modal keybindings tab).
- **71.7** — URL hover tooltip. When the mouse hovers over an OSC 8 or auto-detected URL,
  show a tooltip with the target URL and change the cursor to a pointer.

#### 71.P2 — Search Polish

- **71.8** — Case-sensitivity toggle in the search bar (`Aa` icon or checkbox).
- **71.9** — Tooltips on `<` / `>` / `X` buttons ("Previous match", "Next match", "Close").
- **71.10** — Red-background tint on the search input when match count is zero.
- **71.11** — Verify Task 69's search panel positioning fix landed and still behaves
  correctly under all window sizes and tab configurations.

#### 71.P2 — Tab & Pane UX

- **71.12** — Tab close button ("×") on each tab, tab drag-reorder within a window (using
  egui's drag sense), and in-place tab rename (double-click, tied to the `RenameTab`
  implementation from 71.1).
- **71.13** — Add a `ClearScrollback` `KeyAction` (distinct from the existing
  `ClearScrollbackandDisplay`). Bind to a sensible default (`Ctrl+K` on macOS convention,
  configurable). Include in `KeyAction::ALL`, `name()`, `display_label()`, `FromStr`, and
  `BindingMap::default()` per the keybinding convention in `agents.md`.

#### 71.P2 — Feature Completeness

- **71.14** — Extend `BellMode` in `freminal-common/src/config.rs:406` with `Audio` and
  `Both` variants. Wire `Audio` to a simple system-bell sound (platform-appropriate — `\a`
  on Linux, `NSBeep` on macOS, `MessageBeep` on Windows). Add a config option for a custom
  sound file path. Update Settings Modal picker.
- **71.15** — In-app recording toggle. Add a `ToggleRecording` `KeyAction`, a menu item in
  the Edit menu (or a dedicated "Session" menu), and a visible `● REC` indicator in the
  tab/window chrome when recording is active. Recording currently only activates via
  `--recording-path`. Requires Task 59's FREC v2 runtime start/stop support (verify it
  exists; if not, add a small runtime API on the recorder).
- **71.16** — Cross-platform CWD readback. `freminal/src/gui/mod.rs:950-961` uses
  `/proc/<pid>/cwd` (Linux-only), which means Layout restore silently degrades on macOS and
  Windows. Implement:
  - macOS: `libproc::proc_pidinfo` with `PROC_PIDVNODEPATHINFO`.
  - Windows: query the console's current directory via `NtQueryInformationProcess` or
    `GetFinalPathNameByHandle` on the process handle.
  - Abstract behind a `platform::read_cwd(pid)` function with per-OS implementations.
- **71.17** — Config hot-reload. Currently only shaders hot-reload. Add a "Reload Config"
  menu item that re-reads `config.toml` and applies theme / font / keybinding / opacity
  changes live without restart. Use a file-watcher-optional design (opt-in auto-reload).

#### 71.P3 — Polish

- **71.18** — Unsaved-changes guard on Settings close. If Settings has pending unsaved
  edits and the user dismisses the modal, prompt to Save / Discard / Cancel.
- **71.19** — Startup tab layout setting in Settings Modal becomes a dropdown of layouts
  discovered in `~/.config/freminal/layouts/`, not a free-text field.
- **71.20** — First-run onboarding. Show a 3-panel overlay on first launch explaining the
  menu bar, the settings shortcut, and the layouts directory. Store a `first_run_complete`
  flag in the config. Skippable and permanently dismissible.

### 71 Verification

- Full verification suite after each P-level group.
- Manual UX walkthrough covering every item. Smoke-test with a clean config (no
  `config.toml`) and with an existing user config.
- Cross-platform verification of 71.14 (bell audio), 71.16 (CWD readback) — at minimum
  one Linux, one macOS, one Windows run.

Task 71 is complete when every one of the 20 items is implemented, tested, and verified.

---

## Sequencing

Task 70 and Task 71 are independent and may run in parallel on separate branches
(`task-70/correctness-sweep` and `task-71/ux-polish-sweep`), each with a nested set of
sub-branches per subtask group if the orchestrator prefers.

However, 71.4 (shader error surface) depends on 70.E (typed GPU errors), and 71.15
(recording toggle) may require minor work on the Task 59 recorder. Orchestrator should
sequence those two pairs accordingly.

---

## Design Decisions

- **No new features in v0.8.0.** Explicit non-goal. Any feature request that arrives during
  this version is logged to `FUTURE_PLANS.md` or the appropriate `PLAN_VERSION_NNN.md` and
  deferred.
- **"Complete" task statuses are audit-gated.** Going forward, marking a task complete in
  `MASTER_PLAN.md` requires that a subsequent audit pass is scheduled to verify the
  completion claim. This version is the corrective pass for the tasks that drifted.
- **Error design is typed.** No `String`-typed errors in any new code. No `anyhow` in
  library crates, ever. The rules in `agents.md` are the contract.
