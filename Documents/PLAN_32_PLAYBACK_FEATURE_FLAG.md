# PLAN_32 — Gate Playback and Recording Behind Feature Flags

## Status: In Progress

---

## Overview

Freminal has a fully working session recording and playback system:

- **Recording:** `--recording-path <PATH>` captures raw PTY output to a `.frec` binary file with
  per-frame timestamps.
- **Playback:** `--with-playback-file <PATH>` replays a recorded session through the terminal
  emulator with three modes (instant, real-time, frame-stepping) and GUI controls.

This functionality is valuable for debugging and development but adds code, binary size, and CLI
surface area that most end users will never touch. The task gates both features behind Cargo
feature flags that are **not enabled by default** in release builds, while remaining easily
activatable for development.

**Dependencies:** None (independent)
**Dependents:** None
**Primary crates:** `freminal`, `freminal-terminal-emulator`, `freminal-common`
**Estimated scope:** Medium (subtask count TBD after design decisions are made)

---

## Why This Is a Stub

The feature-flag design requires decisions before subtasks can be written:

1. **One flag or two?** Recording (`--recording-path`) and playback (`--with-playback-file`) are
   logically separate but share the `recording.rs` module (file format, frame type). Options:
   - Single `recording` feature that enables both recording and playback.
   - Two features: `recording` (capture) and `playback` (replay + GUI controls), where
     `playback` implies `recording` (needs the frame parser).
   - Two fully independent features with shared format types always compiled.

2. **What about `new_for_playback()`?** `TerminalEmulator::new_for_playback()` is also used by
   the `snapshot_build.rs` test as a convenient headless constructor. If it's gated behind the
   playback feature, the test needs an alternative. Options:
   - Rename to `new_headless()` and keep it unconditional (it's useful for tests/benchmarks).
   - Gate it and provide a separate `#[cfg(test)]` constructor.

3. **Development workflow:** Should `cargo xtask ci` enable the feature for testing? Should the
   Nix devshell enable it by default? The flag should be invisible to contributors who don't
   need it, but CI must still test the gated code.

## Current Code Surface

The audit identified the following coupling points between playback/recording and the rest of
the codebase:

### Dedicated Files (100% Feature-Specific)

| File                                          | Crate    | Purpose                                                                                 |
| --------------------------------------------- | -------- | --------------------------------------------------------------------------------------- |
| `freminal-terminal-emulator/src/recording.rs` | emulator | FREC format: `PlaybackFrame`, `RecordingError`, `write_header/frame`, `parse_recording` |
| `freminal/src/playback.rs`                    | binary   | Playback thread: `PlaybackState` state machine, `run_playback_thread`, frame feeding    |

### Playback-Specific Items in Shared Files

| File                                          | Item                                                                                                                             | Notes                            |
| --------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- | -------------------------------- |
| `freminal-common/src/args.rs`                 | `Args::recording`, `Args::playback`                                                                                              | Two CLI fields                   |
| `freminal-terminal-emulator/src/io/mod.rs`    | `InputEvent::PlaybackControl(PlaybackCommand)`, `PlaybackMode`, `PlaybackCommand`                                                | One enum variant + two enums     |
| `freminal-terminal-emulator/src/snapshot.rs`  | `PlaybackInfo`, `TerminalSnapshot::playback_info`                                                                                | One struct + one field           |
| `freminal-terminal-emulator/src/interface.rs` | `TerminalEmulator::new_for_playback()`                                                                                           | One constructor                  |
| `freminal/src/gui/mod.rs`                     | `FreminalGui::is_playback`, `selected_playback_mode`, `show_playback_controls()`, `playback_mode_label()`, `send_playback_cmd()` | Two fields + three methods       |
| `freminal/src/main.rs`                        | Playback startup branch (lines 218–297), `pub mod playback;`                                                                     | ~80 lines of conditional startup |

### Recording-Specific Items in Shared Files

| File                                       | Item                                                                                                 | Notes                          |
| ------------------------------------------ | ---------------------------------------------------------------------------------------------------- | ------------------------------ |
| `freminal-terminal-emulator/src/io/pty.rs` | `recording_path` parameter, `recording`/`recording_start` locals, `write_header`/`write_frame` calls | ~20 lines in PTY reader thread |
| `freminal-terminal-emulator/src/lib.rs`    | `pub mod recording;`                                                                                 | Module declaration             |

## Design Decisions (Resolved)

1. **Single `playback` flag.** One feature gates both recording and playback. Recording without
   playback has no independent use case, and a single flag is simpler to maintain.

2. **Rename `new_for_playback()` to `new_headless()`, keep unconditional.** The constructor has
   no playback-specific logic — it just skips PTY setup. Tests and benchmarks use it as a
   headless constructor. It stays available regardless of the feature flag.

3. **Test both configurations in CI.** `cargo xtask ci` runs the full suite without the flag
   AND with `--features playback`. This catches breakage in both the gated and ungated paths.

4. **Hidden from `--help`, clap error when disabled.** The `--recording-path` and
   `--with-playback-file` CLI fields are gated out entirely with `#[cfg(feature = "playback")]`.
   Clap gives "unexpected argument" if a user tries to pass them in a build without the feature.

5. **Nix devshell enables `playback` by default.** Developers get the full feature set in the
   dev environment.

---

## Subtasks

### 32.1 — Add `playback` feature flag to `Cargo.toml` files

- [ ] Add `playback = []` to `freminal-common/Cargo.toml` under `[features]`.
- [ ] Add `playback = ["freminal-common/playback"]` to
      `freminal-terminal-emulator/Cargo.toml` under `[features]` (propagates to common).
- [ ] Add `playback = ["freminal-terminal-emulator/playback"]` to `freminal/Cargo.toml`
      under `[features]` (propagates through emulator to common).
- [ ] Verify: `cargo build --all` passes. `cargo build --all --features playback` passes.
      No behaviour change yet — the flag exists but nothing is gated.

### 32.2 — Rename `new_for_playback()` to `new_headless()`

- [ ] In `freminal-terminal-emulator/src/interface.rs`, rename `new_for_playback()` to
      `new_headless()`. Update the doc comment to reflect its general-purpose nature.
- [ ] Update all call sites:
  - `freminal-terminal-emulator/tests/interface_tests.rs`
  - `freminal-terminal-emulator/tests/snapshot_build.rs`
  - `freminal/src/playback.rs`
  - Any benchmarks that use the constructor.
- [ ] Verify: `cargo test --all` passes. No behaviour change.

### 32.3 — Gate dedicated modules (`recording.rs`, `playback.rs`)

- [ ] In `freminal-terminal-emulator/src/lib.rs`, wrap `pub mod recording;` with
      `#[cfg(feature = "playback")]`.
- [ ] In `freminal/src/main.rs`, wrap `pub mod playback;` with
      `#[cfg(feature = "playback")]`.
- [ ] Verify: `cargo build --all` passes (without feature — modules are excluded).
      `cargo build --all --features playback` passes (modules are included).

### 32.4 — Gate playback/recording items in shared emulator files

- [ ] `freminal-terminal-emulator/src/io/mod.rs`:
  - Gate `PlaybackMode`, `PlaybackCommand` enums with `#[cfg(feature = "playback")]`.
  - Gate `InputEvent::PlaybackControl(PlaybackCommand)` variant with
    `#[cfg(feature = "playback")]`.
- [ ] `freminal-terminal-emulator/src/snapshot.rs`:
  - Gate `PlaybackInfo` struct with `#[cfg(feature = "playback")]`.
  - Gate `TerminalSnapshot::playback_info` field with `#[cfg(feature = "playback")]`.
  - Update `TerminalSnapshot::empty()` to only include `playback_info` when the feature
    is enabled.
- [ ] `freminal-terminal-emulator/src/interface.rs`:
  - Gate `build_snapshot()`'s `playback_info` field population with
    `#[cfg(feature = "playback")]`.
  - Gate `new_for_playback()`… wait, this was renamed to `new_headless()` and stays
    unconditional. Skip this.
- [ ] `freminal-terminal-emulator/src/io/pty.rs`:
  - Gate the `recording_path` parameter, `recording`/`recording_start` locals, and
    `write_header`/`write_frame` calls with `#[cfg(feature = "playback")]`.
  - When the feature is disabled, the `recording_path` parameter is removed from the
    function signature. Update the call site in `main.rs` accordingly.
- [ ] Verify: `cargo build --all` passes. `cargo build --all --features playback` passes.

### 32.5 — Gate CLI args in `args.rs`

- [ ] In `freminal-common/src/args.rs`, wrap `Args::recording` and `Args::playback` fields
      with `#[cfg(feature = "playback")]`.
- [ ] Verify: `cargo build --all` passes (fields hidden from CLI).
      `cargo build --all --features playback` passes (fields visible).

### 32.6 — Gate GUI playback controls and `main.rs` startup branch

- [ ] `freminal/src/gui/mod.rs`:
  - Gate `FreminalGui::is_playback` and `selected_playback_mode` fields with
    `#[cfg(feature = "playback")]`.
  - Gate `show_playback_controls()`, `playback_mode_label()`, and `send_playback_cmd()`
    methods with `#[cfg(feature = "playback")]`.
  - Gate any call sites of these methods/fields within `update()` and `new()`.
- [ ] `freminal/src/main.rs`:
  - Gate the playback startup branch (~80 lines) with `#[cfg(feature = "playback")]`.
  - Gate `pub mod playback;` (already done in 32.3, verify).
  - Gate references to `args.playback` and `args.recording` with
    `#[cfg(feature = "playback")]`.
- [ ] Verify: `cargo build --all` passes. `cargo build --all --features playback` passes.
      `cargo test --all` passes both ways.

### 32.7 — Update `xtask` CI to test both configurations

- [ ] In `xtask/src/main.rs` (or wherever `ci` subcommand is defined), add a second test
      pass that runs `cargo test --all --features playback` and
      `cargo clippy --all-targets --features playback -- -D warnings` after the default pass.
- [ ] Verify: `cargo xtask ci` passes and exercises both configurations.

### 32.8 — Update Nix devshell to enable `playback` by default

- [ ] In `flake.nix` (or the relevant Nix build expression), add `--features playback` to
      the default `cargo build` / `cargo test` invocations in the devshell.
- [ ] Verify: `nix develop` shell has playback enabled by default.

### 32.9 — Final verification

- [ ] `cargo test --all` passes (without feature).
- [ ] `cargo test --all --features playback` passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo-machete` passes.
- [ ] `cargo xtask ci` passes.
- [ ] Manual smoke test: build without feature, confirm `--recording-path` and
      `--with-playback-file` are not in `--help`. Build with feature, confirm they work.

---

## References

- `freminal-terminal-emulator/src/recording.rs` — FREC format implementation
- `freminal/src/playback.rs` — Playback thread and state machine
- `freminal-common/src/args.rs` — CLI arg definitions
- `freminal-terminal-emulator/src/io/mod.rs` — `InputEvent::PlaybackControl`, `PlaybackMode`
- `freminal-terminal-emulator/src/snapshot.rs` — `PlaybackInfo`
- `freminal/src/gui/mod.rs` — Playback GUI controls
- `freminal/src/main.rs` — Playback startup branch
- `Documents/PLAN_09_TMUX_COMPAT_AND_LOGGING.md` — Original recording implementation (Task 9)
- `Documents/MASTER_PLAN.md` — Task 32 entry
