# PLAN_02 — CLI Args + TOML Config

## Overview

Migrate hand-rolled CLI argument parsing to `clap`, extend TOML configuration support with all
relevant options, and add a `--config` flag for specifying an override config path.

**Dependencies:** None
**Dependents:** Task 3 (Settings Modal), Task 4 (Deployment Flake)
**Primary crates:** `freminal-common`, `freminal` (binary)
**Estimated scope:** Medium

---

## Problem Statement

### CLI Args

The CLI argument parser in `freminal-common/src/args.rs` was originally hand-rolled, handling 4
flags with manual string matching and custom error handling. This was fragile, didn't generate
help text, and required manual maintenance for each new flag.

The parser has been migrated to `clap` (derive) as part of this task. `clap` was already in the
workspace dependencies (used by `xtask`) and is now also used by the main binary.

### Config System

The config system in `freminal-common/src/config.rs` supports layered loading
(system → user → env var → explicit path) but:

- `load_config()` is always called with `None` for the explicit path — no way for users to
  specify a custom config path
- Theme name is stored in config but never applied (hardcoded Catppuccin Mocha)
- Scrollback limit is hardcoded to 4000 in `Buffer::new()` — not configurable
- Some CLI flags have no TOML equivalents and vice versa

---

## Current State

### Existing CLI Flags

| Flag                      | Current  | Proposed TOML   | Notes                                               |
| ------------------------- | -------- | --------------- | --------------------------------------------------- |
| `--recording-path <path>` | CLI only | CLI only        | Session recording — ephemeral, CLI-only makes sense |
| `--shell <path>`          | CLI only | Both CLI + TOML | Default shell — useful as persistent config         |
| `--show-all-debug`        | CLI only | CLI only        | Debug mode — development/debugging only             |
| `--write-logs-to-file`    | CLI only | Both CLI + TOML | Logging — useful as persistent default              |
| `--config <path>`         | NEW      | N/A             | Override config file path                           |

### Existing TOML Config

```toml
version = 1

[font]
family = "CaskaydiaCove Nerd Font"
size = 12.0

[cursor]
shape = "block"
blink = true

[theme]
name = "catppuccin-mocha"
```

### Proposed New TOML Sections

```toml
[shell]
path = "/bin/zsh"   # Default shell (overridden by --shell CLI flag)

[logging]
write_to_file = false   # Persistent default for --write-logs-to-file

[scrollback]
limit = 4000   # Max scrollback lines (currently hardcoded)
```

---

## Subtasks

### 2.1 — Migrate CLI parsing to clap

- **Status:** Complete
- **Scope:** `freminal-common/src/args.rs`, `freminal-common/Cargo.toml`
- **Details:**
  - Add `clap` with `derive` feature to `freminal-common` dependencies
  - Define `#[derive(Parser)]` struct replacing the hand-rolled `Args`
  - Preserve all 4 existing flags with identical behavior
  - Add `--config <path>` flag (optional, `PathBuf`)
  - Auto-generate `--help` and `--version`
  - Remove the hand-rolled parsing functions
  - Update `freminal/src/main.rs` to use new clap-based parsing
- **Acceptance criteria:**
  - All existing flags work identically
  - `--help` prints usage information
  - `--version` prints version
  - `--config /path/to/config.toml` passes path through
  - Invalid flags produce helpful error messages
- **Tests required:**
  - Each flag parses correctly
  - Default values are correct when flags are omitted
  - Invalid flag produces error
  - `--config` accepts valid path

### 2.2 — Extend TOML config schema

- **Status:** Complete
- **Scope:** `freminal-common/src/config.rs`, `config_example.toml`
- **Details:**
  - Add `ShellConfig { path: Option<String> }` section
  - Add `LoggingConfig { write_to_file: bool }` section
  - Add `ScrollbackConfig { limit: usize }` section
  - All new sections are optional with sensible defaults
  - Add validation: scrollback limit must be > 0 and ≤ 100_000
  - Update `config_example.toml` with new sections and documentation
  - Maintain backward compatibility — old config files must still load
- **Acceptance criteria:**
  - New sections deserialize correctly
  - Missing sections use defaults
  - Old config files (without new sections) load without errors
  - Invalid values produce clear error messages
  - `config_example.toml` documents all options
- **Tests required:**
  - Deserialize config with all sections
  - Deserialize config with missing optional sections
  - Validation rejects invalid scrollback values
  - Backward compatibility with v1 configs

### 2.3 — Wire --config flag to config loading

- **Status:** Complete
- **Scope:** `freminal/src/main.rs`, `freminal-common/src/config.rs`
- **Details:**
  - Pass `--config` path from CLI args to `load_config()`
  - `load_config()` already supports an explicit path parameter — just needs to be wired
  - If `--config` is specified and file doesn't exist, fail with a clear error (don't fall back)
  - If `--config` is not specified, use existing layered loading
- **Acceptance criteria:**
  - `--config /path/to/file.toml` loads that specific file
  - Missing file with `--config` produces clear error, does not silently fall back
  - Without `--config`, behavior is unchanged
- **Tests required:**
  - Config loads from explicit path
  - Missing explicit path produces error
  - Default path loading still works

### 2.4 — Implement CLI + TOML precedence for shared options

- **Status:** Complete
- **Scope:** `freminal/src/main.rs`
- **Details:**
  - For options that exist in both CLI and TOML (`--shell`, `--write-logs-to-file`):
    - CLI flag takes precedence over TOML value
    - TOML value takes precedence over default
  - Implement merge logic: parse CLI args, load config, merge with CLI overriding
  - Document precedence: CLI > TOML > env var > system config > defaults
- **Acceptance criteria:**
  - `--shell /bin/bash` overrides `shell.path` in TOML
  - `--write-logs-to-file` overrides `logging.write_to_file` in TOML
  - TOML values used when CLI flags are absent
  - Defaults used when both CLI and TOML are absent
- **Tests required:**
  - CLI overrides TOML for each shared option
  - TOML used when CLI absent
  - Default used when both absent
  - Full precedence chain works correctly

### 2.5 — Wire scrollback limit to buffer

- **Status:** Complete
- **Scope:** `freminal-buffer/src/buffer.rs`, `freminal-buffer/src/terminal_handler.rs`, `freminal-terminal-emulator/src/state/internal.rs`, `freminal-terminal-emulator/src/interface.rs`, `freminal/src/main.rs`
- **Details:**
  - Added `Buffer::with_scrollback_limit(self, limit)` builder method and `Buffer::scrollback_limit()` getter
  - Added `TerminalHandler::with_scrollback_limit(self, limit)` builder method
  - Extended `TerminalState::new()` and `TerminalEmulator::new()` with `scrollback_limit: Option<usize>` parameter
  - Production call in `main.rs` passes `Some(cfg.scrollback.limit)`
  - All 110+ existing test/bench call sites unchanged (default 4000 preserved)
  - Validate at config load time (already done in 2.2)
- **Acceptance criteria:**
  - Scrollback limit configurable via TOML and respected by buffer
  - Default remains 4000 when not specified
- **Tests required:**
  - Buffer respects custom scrollback limit
  - Default scrollback limit is 4000
- **Tests added:**
  - `buffer::scrollback_limit_tests` — 4 tests (default, override, enforced, zero)
  - `terminal_handler::tests` — 2 tests (default, override)
  - `shadow_handler` — 3 tests (default, custom, None)

### 2.6 — Implement config serialization (write-back support)

- **Status:** COMPLETE ✓
- **Scope:** `freminal-common/src/config.rs`
- **Details:**
  - Added `save_config(config: &Config, path: Option<&Path>) -> Result<(), ConfigError>`
  - Validates config before writing (reuses existing `validate()`)
  - Resolves path: explicit path if provided, otherwise user-level default (`~/.config/freminal/config.toml`)
  - Creates parent directories if they don't exist
  - Serializes with `toml::to_string_pretty()` for human-readable output
  - Added `ConfigError::Serialize(String)` and `ConfigError::Write { path, source }` variants
  - `None` shell path and font family are omitted via `skip_serializing_if`
- **Acceptance criteria:**
  - Config round-trips: load → save → load produces identical config ✓
  - Written TOML is human-readable and well-formatted ✓
  - Saved to correct platform-specific path ✓
- **Tests added:**
  - `save_default_config_to_explicit_path_creates_valid_file` — saves default config, verifies file exists and is valid TOML
  - `save_config_round_trips_custom_values` — custom font size, shell path, scrollback limit survive save/load cycle
  - `save_config_creates_parent_directories` — deeply nested path has parent dirs auto-created
  - `save_config_rejects_invalid_config` — font size out of range is rejected before write
  - `save_config_output_is_valid_toml` — serialized output parses as valid TOML
  - `save_then_modify_then_save_overwrites` — second save overwrites first, final values reflect update
  - `save_config_preserves_none_shell_path` — `None` shell path omitted from TOML, round-trips correctly

### 2.7 — Cleanup and documentation

- **Status:** COMPLETE ✓
- **Scope:** All modified files
- **Details:**
  - Old hand-rolled arg parsing code was fully removed in subtask 2.1
  - Verified no dead code warnings (clippy clean, no warnings in test output)
  - No unused dependencies (cargo machete clean)
  - Updated README.md with CLI usage table documenting all flags
  - Updated PLAN_02 problem statement to reflect completed migration (past tense)
  - Updated `config_example.toml` (already current from earlier subtasks)
- **Acceptance criteria:**
  - No dead code from old arg parser ✓
  - All tests pass, clippy clean, no unused deps ✓

---

## Affected Files

| File                                          | Change Type                      |
| --------------------------------------------- | -------------------------------- |
| `freminal-common/Cargo.toml`                  | Add clap dependency              |
| `freminal-common/src/args.rs`                 | Rewrite with clap derive         |
| `freminal-common/src/config.rs`               | Extend schema, add serialization |
| `freminal/src/main.rs`                        | Wire --config, merge logic       |
| `freminal-terminal-emulator/src/interface.rs` | Accept scrollback config         |
| `freminal-buffer/src/buffer.rs`               | Configurable scrollback limit    |
| `config_example.toml`                         | Add new sections                 |

---

## CLI Flag Reference

Final CLI flag inventory after this task:

| Flag                      | Type              | TOML Equivalent         | Behavior                       |
| ------------------------- | ----------------- | ----------------------- | ------------------------------ |
| `--recording-path <path>` | `Option<PathBuf>` | None (CLI only)         | Record session to file         |
| `--shell <path>`          | `Option<String>`  | `shell.path`            | Override default shell         |
| `--show-all-debug`        | `bool`            | None (CLI only)         | Enable debug output            |
| `--write-logs-to-file`    | `bool`            | `logging.write_to_file` | Write logs to file             |
| `--config <path>`         | `Option<PathBuf>` | N/A                     | Override config file path      |
| `--help`                  | —                 | N/A                     | Print usage (auto-generated)   |
| `--version`               | —                 | N/A                     | Print version (auto-generated) |

---

## Risk Assessment

| Risk                            | Likelihood | Impact | Mitigation                             |
| ------------------------------- | ---------- | ------ | -------------------------------------- |
| Breaking existing CLI usage     | Low        | High   | Preserve exact flag names and behavior |
| Config backward incompatibility | Low        | Medium | All new sections are optional          |
| Merge precedence bugs           | Medium     | Medium | Comprehensive test matrix              |
| Config write corruption         | Low        | High   | Round-trip tests, atomic writes        |
