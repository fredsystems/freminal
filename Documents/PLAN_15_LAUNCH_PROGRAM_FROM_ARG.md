# PLAN 15 — Launch Program from Arg

## Overview

Allow users to run `freminal <program> [args...]` to launch a specific program inside
freminal. When that program exits, freminal terminates. This enables use cases like
`freminal yazi`, `freminal htop`, or `freminal -- nvim file.txt`.

## Current State

- CLI args are defined in `freminal-common/src/args.rs` using clap derive macros.
- No positional arguments exist — all args are named flags/options.
- The `--shell <PATH>` flag sets the shell binary but does not pass arguments to it.
- PTY spawning in `freminal-terminal-emulator/src/io/pty.rs` uses `CommandBuilder::new(shell)`
  which takes a single program path with no additional arguments.
- `FreminalPtyInputOutput::new()` accepts `shell: Option<String>` — a single path string.
- When the spawned program exits, the PTY reader thread detects `Ok(0)`, drops the send
  channel, and the consumer thread sends `ViewportCommand::Close` — clean exit already works.

## Design

### CLI Syntax

Use clap's trailing positional arguments with `--` separator support:

```text
freminal [OPTIONS] [--] [COMMAND [ARGS...]]
```

Examples:

- `freminal yazi` — launch yazi
- `freminal -- nvim -u NONE file.txt` — launch nvim with args
- `freminal` — launch default shell (unchanged behavior)
- `freminal --shell /bin/zsh` — launch zsh (unchanged behavior)

### Argument Definition

Add to `Args`:

```rust
/// Program to run instead of the default shell.
///
/// Everything after `--` (or the first non-option argument) is treated as
/// a command and its arguments. When specified, freminal launches this
/// program and exits when it terminates.
#[arg(trailing_var_arg = true)]
pub command: Vec<String>,
```

### Precedence

`command` (positional) takes priority over `--shell`. If both are specified, `command` wins
and `--shell` is ignored (with a warning logged).

### Data Flow

1. `main.rs`: After `Args::parse()`, check `args.command`:
   - If non-empty: extract `(program, args)` from the `Vec<String>`.
   - Pass as a new `command: Option<(String, Vec<String>)>` field through to emulator creation.
2. `TerminalEmulator::new()`: Accept the command tuple instead of just a shell path.
3. `FreminalPtyInputOutput::new()`: Accept `command: Option<(String, Vec<String>)>`.
4. `run_terminal()`: When `command` is `Some((prog, args))`, use
   `CommandBuilder::new(prog)` + `cmd.args(args)` instead of `CommandBuilder::new(shell)`.

### Exit Behavior

No changes needed. The existing lifecycle handles program exit correctly:
PTY reader gets `Ok(0)` -> drops channel -> consumer thread detects close ->
sends `ViewportCommand::Close` -> clean exit.

## Affected Files

| File                                          | Change                                                                       |
| --------------------------------------------- | ---------------------------------------------------------------------------- |
| `freminal-common/src/args.rs`                 | Add `command: Vec<String>` positional arg                                    |
| `freminal/src/main.rs`                        | Extract command from args, pass to emulator                                  |
| `freminal-terminal-emulator/src/interface.rs` | Accept command tuple in `new()`                                              |
| `freminal-terminal-emulator/src/io/pty.rs`    | Accept command tuple in `run_terminal()` and `FreminalPtyInputOutput::new()` |

## Subtasks

- [x] **15.1** Add `command: Vec<String>` trailing positional arg to `Args` struct
  - ✅ Completed 2026-03-16. Added with `trailing_var_arg = true`. Removed
    `allow_hyphen_values` to preserve unknown-flag detection; users use `--` for
    command args starting with `-`.
- [x] **15.2** Update `run_terminal()` and `FreminalPtyInputOutput::new()` to accept
      `Option<(String, Vec<String>)>` and use `CommandBuilder::new(prog) + cmd.args(args)`
  - ✅ Completed 2026-03-16. Both functions accept `command` and `shell` separately.
- [x] **15.3** Update `TerminalEmulator::new()` to accept and forward command tuple
  - ✅ Completed 2026-03-16. Extracts command tuple from `args.command` Vec.
- [x] **15.4** Wire up `main.rs` to extract command from `args.command`, handle precedence
      over `--shell`, and pass through to emulator creation
  - ✅ Completed 2026-03-16. Warning logged when both `--shell` and command specified.
- [x] **15.5** Add tests for argument parsing (command with args, command alone, empty,
      precedence over `--shell`)
  - ✅ Completed 2026-03-16. Added 6 new tests + updated property test. 27 arg tests
    total, all passing.
- [x] **15.6** Update `--help` text and verify `freminal --help` output is correct
  - ✅ Completed 2026-03-16. Help shows `[COMMAND]...` usage with examples.

## Verification

- `cargo test --all` passes
- `cargo clippy --all-targets --all-features -- -D warnings` clean
- `cargo-machete` clean
- Manual: `freminal bash` launches bash and exits when bash exits
- Manual: `freminal -- ls -la` runs ls and freminal closes after
- Manual: `freminal` still launches default shell
