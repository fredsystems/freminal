# PLAN_32 — Gate Playback and Recording Behind Feature Flags

## Status: Stub

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

## Design Questions to Resolve Before Subtask Creation

1. Single feature (`recording`) vs. two features (`recording` + `playback`)?
2. How to handle `new_for_playback()` — rename to `new_headless()` or gate?
3. CI strategy: always test with feature enabled, or test both configurations?
4. Should the `--recording-path` and `--with-playback-file` CLI flags be hidden (not shown in
   `--help`) when the feature is disabled, or should they produce a clear error message?
5. Should the Nix flake / devshell enable the features by default for development?

## Subtasks

To be created after the design questions above are resolved. Expected work:

- Add `recording` and/or `playback` feature flags to relevant `Cargo.toml` files
- Wrap dedicated modules (`recording.rs`, `playback.rs`) in `#[cfg(feature = "...")]`
- Gate CLI args, enum variants, struct fields, and methods behind the feature
- Update CI (`xtask`) to test both with and without the feature
- Update the Nix flake / devshell if needed
- Update `README.md` to document the feature flags
- Ensure `cargo test --all` passes with and without the features enabled

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
