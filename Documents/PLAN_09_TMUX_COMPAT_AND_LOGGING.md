# PLAN_09 — tmux Compatibility and Persistent Logging

## Status: In Progress

## Overview

tmux does not work in Freminal. Starting `tmux` or attaching to an existing session produces
corrupted layout and display. This task fixes the root causes and adds always-on persistent
logging so that escape sequence issues can be diagnosed without special CLI flags.

**Dependencies:** None
**Dependents:** None
**Primary crates:** `freminal-common`, `freminal-buffer`, `freminal-terminal-emulator`, `freminal`
**Estimated scope:** Large (12 subtasks across two work streams)
**Branch:** `task-09/tmux-compat-logging`

---

## Problem Analysis

### tmux compatibility

Three issues were identified through code investigation. They are listed in order of severity.

#### Bug #1 — Multi-parameter mode sequences are silently dropped (Critical)

**Location:** `freminal-common/src/buffer_states/mode.rs:109-157`

`terminal_mode_from_params` performs exact byte-slice matching against the full parameter
string. It handles `b"?1049"` and `b"?2004"` individually, but when a program sends a
compound sequence like `ESC[?1049;2004h` (set alternate screen AND bracketed paste in one
CSI), the params arrive as the byte slice `?1049;2004`. No match arm contains a `;`, so the
entire sequence falls through to `Mode::Unknown` and both modes are silently ignored.

tmux routinely sends compound mode sequences as its very first output. Examples observed in
tmux 3.x startup:

```text
ESC[?1049h          — alternate screen (works individually)
ESC[?1h             — DECCKM cursor keys (works individually)
ESC[?1049;1h        — alternate screen + DECCKM (BROKEN — both dropped)
ESC[?1000;1006h     — X11 mouse + SGR mouse encoding (BROKEN — both dropped)
ESC[?2004h          — bracketed paste (works individually)
```

**Impact:** This is almost certainly the primary cause of display corruption. When alternate
screen, cursor key mode, mouse tracking, and other fundamental modes are silently dropped,
the terminal is in a wrong state from the first frame.

The CSI `h`/`l` handlers at `csi.rs:230-243` pass the entire params blob as one unit:

```rust
AnsiCsiParserState::Finished(b'h') => {
    output.push(TerminalOutput::Mode(Mode::terminal_mode_from_params(
        &self.params,
        &SetMode::DecSet,
    )));
    // ...
}
```

**Fix:** Split `self.params` on `;` before the match. Emit one `TerminalOutput::Mode` per
sub-parameter. The `?` prefix applies to all sub-parameters (it is a DEC private indicator
for the entire sequence).

#### Bug #2 — DECRPM query responses are never sent (Critical)

**Location:** `freminal-buffer/src/terminal_handler.rs:1137-1145`

tmux sends DECRPM queries (`ESC[?Pn$p`) on startup to discover which modes the terminal
supports. Freminal parses these correctly and every mode enum has a `report()` method that
produces the correct `ESC[?Pn;Ps$y` response string via the `ReportMode` trait. However,
the responses are never written to the PTY:

```rust
Mode::XtExtscrn(XtExtscrn::Query)
| Mode::AltScreen47(AltScreen47::Query)
| Mode::SaveCursor1048(SaveCursor1048::Query)
| Mode::Decawm(Decawm::Query)
| Mode::LineFeedMode(Lnm::Query)
| Mode::Dectem(Dectcem::Query) => {
    // TODO: Step 3.5 — report mode via outbound write channel
}
```

The comment references "Step 3.5" from Task 7 (Escape Sequence Coverage) which was never
completed.

**Impact:** tmux times out waiting for DECRPM responses and falls back to guessing terminal
capabilities, often incorrectly. This compounds Bug #1 since tmux cannot verify that its
mode-set commands took effect.

Note: All `Query` variants across all mode enums need to be handled, not just the six
listed in the current match arm. Any mode that tmux queries and gets no response for is a
potential source of misbehavior.

#### Gap #3 — No modifier key encoding for special keys (Significant)

**Location:** `freminal-terminal-emulator/src/interface.rs:88-186`

`TerminalInput` has no modifier variants. `ArrowUp` always encodes as `ESC[A` or `ESCOA`
depending on DECCKM mode. There is no way to encode `Shift+Up` (`ESC[1;2A`),
`Ctrl+Left` (`ESC[1;5D`), `Alt+F1` (`ESC[1;3P`), etc.

The xterm modified key encoding convention is:

```text
ESC [ 1 ; <modifier> <final>

Where <modifier> is:
  2 = Shift
  3 = Alt
  4 = Shift+Alt
  5 = Ctrl
  6 = Ctrl+Shift
  7 = Ctrl+Alt
  8 = Ctrl+Alt+Shift
```

tmux's default bindings use `Ctrl+B` as the prefix (which works — it's a simple control
character), but pane navigation defaults to `Ctrl+Arrow` and window selection to
`Shift+Arrow`. Without modifier encoding, these keybindings produce bare arrow sequences
and the user cannot navigate tmux panes or windows using defaults.

**Impact:** tmux is functionally usable (commands work) but pane/window navigation requires
custom keybindings that avoid modified special keys. This is a significant UX gap for any
tmux user.

### Logging

The current logging infrastructure has several problems that make diagnosing issues like the
tmux breakage unnecessarily difficult:

1. **Opt-in only** — file logging requires `--write-logs-to-file` or a config toggle. When
   a problem occurs, there are no logs to look at unless the user anticipated needing them.
2. **Writes to `"./"` (CWD)** — log file location depends on how the application was
   launched. Not discoverable, not platform-canonical.
3. **Aggressive rotation** — 2 hourly files means at most 2 hours of history. Logs from a
   previous session are gone.
4. **No PTY byte tracing** — there is no `trace!`-level instrumentation at the
   `handle_incoming_data` entry point. The only way to see raw escape sequences is the
   separate `--recording-path` mechanism which is disconnected from the log system and
   outputs decimal bytes with no timestamps.
5. **No log level config** — the only way to change log level is the `RUST_LOG` env var.
   There is no persistent config option.

---

## Work Streams

The task is split into two independent work streams that can be implemented in either order.
Stream A (tmux compatibility) is higher priority because it fixes user-facing breakage.
Stream B (logging) is lower priority but directly improves the ability to diagnose issues
like the ones in Stream A.

### Stream A — tmux Compatibility (Subtasks 9.1–9.7)

### Stream B — Persistent Logging (Subtasks 9.8–9.12)

---

## Subtasks

### 9.1 — Split multi-parameter mode sequences

**Files:**

- `freminal-terminal-emulator/src/ansi_components/csi.rs` (CSI `h`/`l` handlers)
- `freminal-common/src/buffer_states/mode.rs` (`terminal_mode_from_params`)

**What to do:**

In the CSI `Finished(b'h')` and `Finished(b'l')` handlers at `csi.rs:230-243`, replace the
single `Mode::terminal_mode_from_params(&self.params, ...)` call with a loop that splits
`self.params` on `;` and emits one `TerminalOutput::Mode(...)` per sub-parameter.

The `?` prefix is a DEC private indicator that applies to the entire sequence. When params
are `?1049;2004`, the split produces `?1049` and `2004`. The second sub-parameter must be
re-prefixed with `?` since the individual match arms in `terminal_mode_from_params` expect
it (e.g. `b"?2004"` not `b"2004"`).

Algorithm:

```text
1. Check if params starts with b'?'
2. If yes: strip the '?', split remainder on ';'
   For each sub-param: push Mode::terminal_mode_from_params(b"?" + sub_param, mode)
3. If no: split on ';'
   For each sub-param: push Mode::terminal_mode_from_params(sub_param, mode)
```

The same splitting logic must also be applied in the DECRQM handler
(`decrqm.rs:26-30`) for the `DecQuery` path.

**Tests to write:**

- Unit test: `ESC[?1049;2004h` produces two `TerminalOutput::Mode` entries:
  `XtExtscrn(Alternate)` and `BracketedPaste(Enabled)`.
- Unit test: `ESC[?1049;1h` produces `XtExtscrn(Alternate)` and `Decckm(Application)`.
- Unit test: `ESC[?1000;1006h` produces `MouseMode(XtMseX11)` and `MouseMode(XtMseSgr)`.
- Unit test: `ESC[?1049h` (single param) still works unchanged.
- Unit test: `ESC[20h` (non-DEC, single param without `?`) still works.
- Integration test: Feed a compound mode set through `TerminalState::handle_incoming_data`
  and verify both modes are activated.

**Acceptance criteria:**

- Compound `h`/`l` sequences produce one Mode per sub-parameter
- Single-parameter sequences still work identically
- All existing mode tests still pass
- `cargo test --all` passes

---

### 9.2 — Wire DECRPM query responses through `write_to_pty`

**Files:**

- `freminal-buffer/src/terminal_handler.rs` (the `Mode::*Query` match arms)
- `freminal-common/src/buffer_states/mode.rs` (`ReportMode` trait — already implemented)

**What to do:**

The `TerminalHandler` already has a `Sender<PtyWrite>` (wired in the performance refactor,
Task 6). The `ReportMode` trait is fully implemented on every mode enum and produces the
correct `ESC[?Pn;Ps$y` response string.

Replace the empty TODO block at `terminal_handler.rs:1137-1145` with code that:

1. Calls `mode.report(None)` to get the response string.
2. Sends the response bytes via `self.write_to_pty(response.as_bytes())`.

The current match arm only covers 6 specific Query variants. This is incomplete — any mode
enum can have a Query variant. The fix must handle all Query variants, not just the six
currently listed. The cleanest approach is to add a catch-all pattern that detects Query
variants generically.

However, there is a subtlety: the response must reflect the **current** mode state, not
just "mode 0 (not recognized)". The `report(None)` method on each mode variant already
returns the correct `Ps` value (1 = set, 2 = reset, 0 = not recognized) based on the
variant's internal state. But for a `Query` variant constructed from a DECRQM request, the
variant is `Query` — it doesn't carry the current state.

The correct approach:

1. When a `Query` variant arrives, look up the **current** state of that mode in the
   handler's fields (e.g. `self.show_cursor` for DECTCEM, `self.auto_wrap_mode` for DECAWM).
2. Call `report()` on the **current** state variant, not on the `Query` variant itself.
3. For modes tracked in `TerminalState::modes` rather than `TerminalHandler` (e.g. DECCKM,
   bracketed paste, mouse tracking), the handler does not have direct access. These must be
   handled at the `TerminalState` level in the mode-sync loop at
   `internal.rs:300-328`, which already iterates parsed output.

This means the DECRPM response logic is split:

- Handler-owned modes (DECAWM, DECTCEM, LNM, XtExtscrn, AltScreen47, SaveCursor1048,
  XtCBlink): respond in `terminal_handler.rs`.
- State-owned modes (DECCKM, bracketed paste, mouse tracking, focus reporting, DECSCNM,
  DECARM, reverse wrap, synchronized updates, grapheme clustering, theming): respond in
  the mode-sync loop in `internal.rs`.
- Unknown modes: respond with `Ps=0` (not recognized) — the `UnknownQuery` variant's
  `report()` already does this.

**Tests to write:**

- Unit test: send `ESC[?25$p` (query DECTCEM), verify response is `ESC[?25;1$y` (cursor
  shown) or `ESC[?25;2$y` (cursor hidden) depending on current state.
- Unit test: send `ESC[?1$p` (query DECCKM), verify correct response.
- Unit test: send `ESC[?2004$p` (query bracketed paste), verify correct response.
- Unit test: send `ESC[?9999$p` (query unknown mode), verify response is `ESC[?9999;0$y`.
- Integration test: set a mode, query it, verify response reflects the set state.

**Acceptance criteria:**

- All DECRPM queries produce correct responses on the PTY write channel
- Handler-owned modes respond from `terminal_handler.rs`
- State-owned modes respond from `internal.rs`
- Unknown modes respond with `Ps=0`
- `cargo test --all` passes

---

### 9.3 — Add modifier key encoding to `TerminalInput`

**Files:**

- `freminal-terminal-emulator/src/interface.rs` (`TerminalInput` enum and `to_payload`)

**What to do:**

Add modifier variants to `TerminalInput` for the keys that support xterm-style modifier
encoding. The xterm convention for modified special keys is:

```text
CSI 1 ; <modifier> <final>
```

Where `<modifier>` is: 2=Shift, 3=Alt, 4=Shift+Alt, 5=Ctrl, 6=Ctrl+Shift, 7=Ctrl+Alt,
8=Ctrl+Alt+Shift.

Keys that use this encoding:

- Arrow keys: `ESC[1;NmA/B/C/D`
- Home/End: `ESC[1;NmH/F`
- Function keys F1–F4: `ESC[1;NmP/Q/R/S` (SS3 form not used with modifiers)
- Function keys F5–F12: `ESC[15;Nm~`, `ESC[17;Nm~`, etc.
- Insert/Delete/PageUp/PageDown: `ESC[2;Nm~`, `ESC[3;Nm~`, `ESC[5;Nm~`, `ESC[6;Nm~`

Design options:

**(a) Add a `Modifiers` bitfield to each variant:**

```rust
pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

pub enum TerminalInput {
    Ascii(u8),
    Ctrl(u8),
    ArrowRight(KeyModifiers),
    ArrowLeft(KeyModifiers),
    // ...
}
```

This is the cleanest approach. `KeyModifiers::NONE` produces the unmodified sequence.
`to_payload` checks for modifiers and switches to the `ESC[1;Nm...` form when any are set.

**(b) Add separate `Modified(key, modifier_code)` variant:**

Less invasive but harder to maintain. Option (a) is preferred.

The `to_payload` method needs to produce `Vec<u8>` for modified keys (the sequences are
variable-length and cannot be represented as `&'static [u8]`). This means
`TerminalInputPayload` needs an `Owned(Vec<u8>)` variant in addition to `Single(u8)` and
`Many(&'static [u8])`.

**Tests to write:**

- Unit test: `ArrowUp` with `Shift` → `ESC[1;2A`
- Unit test: `ArrowLeft` with `Ctrl` → `ESC[1;5D`
- Unit test: `Home` with `Alt` → `ESC[1;3H`
- Unit test: `F5` with `Ctrl+Shift` → `ESC[15;6~`
- Unit test: `Delete` with `Shift` → `ESC[3;2~`
- Unit test: `ArrowUp` with no modifiers → `ESC[A` (unchanged)
- Unit test: DECCKM mode + `ArrowUp` with no modifiers → `ESCOA` (unchanged)

**Acceptance criteria:**

- All special keys support Shift, Ctrl, Alt, and combinations
- Unmodified keys produce identical output to current behavior
- `to_payload` with DECCKM mode and modifiers produces CSI form (not SS3)
- `cargo test --all` passes

---

### 9.4 — Wire modifier keys from GUI to `TerminalInput`

**Files:**

- `freminal/src/gui/terminal.rs` (`write_input_to_terminal` and key handling)

**What to do:**

The GUI key handling code in `write_input_to_terminal` currently maps egui key events to
`TerminalInput` variants without checking `event.modifiers`. Update the key mapping to:

1. Read `event.modifiers.shift`, `event.modifiers.ctrl`, `event.modifiers.alt` from
   the egui `KeyEvent`.
2. Construct a `KeyModifiers` from these flags.
3. Pass the modifiers into the appropriate `TerminalInput` variant (from subtask 9.3).

Care must be taken to not double-apply Ctrl: `Ctrl+C` should still produce
`TerminalInput::Ctrl(b'c')`, not `TerminalInput::Ascii(b'c')` with a Ctrl modifier.
The modifier encoding applies to special keys (arrows, function keys, etc.), not to
regular ASCII keys where Ctrl already produces a control code.

**Tests to write:**

- This is primarily a GUI wiring change. Manual testing is the primary verification:
  - `tmux` → `Ctrl+B`, `Ctrl+Arrow` for pane navigation
  - `Shift+Arrow` for tmux window selection
  - `Alt+Arrow` for word movement in bash/zsh

**Acceptance criteria:**

- Modifier state is correctly read from egui events
- Modified special keys produce correct escape sequences
- Ctrl+letter keys still produce control codes (not double-modified)
- `cargo test --all` passes

---

### 9.5 — Handle `ESC[?6n` (DEC private DSR)

**Files:**

- `freminal-terminal-emulator/src/ansi_components/csi_commands/dsr.rs`
  (or wherever DSR is parsed)

**What to do:**

Some programs (including tmux) send `ESC[?6n` — a DEC private variant of the Device
Status Report that requests cursor position. The current DSR parser does not handle the
`?` prefix. `ESC[?6n` either falls through as unknown or is misidentified.

The response for `ESC[?6n` is the same as `ESC[6n`: `ESC[Pr;PcR` with the current
cursor row and column (1-indexed). Some terminals add a `?` in the response
(`ESC[?Pr;PcR`) but this is not universally expected.

Check the current DSR parser, understand how it handles `?6n`, and fix it to respond
correctly.

**Tests to write:**

- Unit test: `ESC[?6n` produces a `TerminalOutput::CursorReport` (same as `ESC[6n`).
- Unit test: verify the response format is `ESC[row;colR`.

**Acceptance criteria:**

- `ESC[?6n` is recognized and produces a cursor position response
- `ESC[6n` continues to work unchanged
- `cargo test --all` passes

---

### 9.6 — tmux integration test script

**Files:**

- `tests/tmux_smoke.sh` (new — shell script, not Rust)

**What to do:**

Create a manual integration test script that exercises the tmux-critical paths. This is
not an automated test (tmux requires a real PTY), but a documented procedure:

1. Start Freminal
2. Run `tmux new-session -d -s test`
3. Run `tmux attach -t test`
4. Verify: status bar renders at the bottom
5. Verify: `Ctrl+B c` creates a new window (window indicator changes)
6. Verify: `Ctrl+B %` splits pane vertically (pane border appears)
7. Verify: `Ctrl+B Arrow` navigates between panes
8. Verify: `Ctrl+B d` detaches cleanly
9. Verify: `tmux attach -t test` re-attaches with correct layout
10. Verify: `tmux kill-server` exits cleanly

Also document known limitations (if any remain after subtasks 9.1–9.5).

**Acceptance criteria:**

- Script exists and documents the test procedure
- All 10 steps pass when run manually against the built binary

---

### 9.7 — Add `trace!`-level escape sequence logging

**Files:**

- `freminal-terminal-emulator/src/state/internal.rs` (`handle_incoming_data`)
- `freminal-terminal-emulator/src/ansi.rs` (parser output)

**What to do:**

Add `trace!`-level instrumentation at two points:

1. **Raw PTY bytes** — at the entry to `handle_incoming_data`, log the first 512 bytes
   of the incoming buffer in hex format. This captures exactly what the child process
   sent, before any parsing.

   ```rust
   trace!(
       bytes = incoming.len(),
       hex = %hex_preview(&incoming, 512),
       "PTY data received"
   );
   ```

2. **Parsed output** — after `self.parser.push(&incoming)` returns, log each
   `TerminalOutput` variant at `trace!` level. This shows what the parser interpreted
   from the raw bytes.

   ```rust
   for output in &parsed {
       trace!(%output, "parsed terminal output");
   }
   ```

   This requires `TerminalOutput` to implement `Display` (it already implements `Debug`;
   if `Display` is not present, use `Debug` formatting).

These are `trace!`-level, so they are compiled out in release builds unless the
`tracing/max_level_trace` feature is enabled, and are filtered out at runtime unless the
subscriber's filter includes `trace` for `freminal_terminal_emulator`. There is zero
overhead in normal operation.

**Tests to write:**

- No functional tests needed (this is instrumentation only).
- Verify: `RUST_LOG=freminal_terminal_emulator=trace cargo run` produces hex byte dumps
  and parsed output in the log.

**Acceptance criteria:**

- `trace!` calls exist at both instrumentation points
- Normal operation (`INFO` level) shows no new output
- `RUST_LOG=freminal_terminal_emulator=trace` shows raw bytes and parsed output
- `cargo test --all` passes
- `cargo clippy --all-targets --all-features -- -D warnings` passes

---

### 9.8 — Compute platform-canonical log directory

**Files:**

- `freminal-common/src/config.rs` (add `log_dir()` function)

**What to do:**

Add a `pub fn log_dir() -> Option<PathBuf>` function that returns the platform-canonical
log directory using the `directories` crate (already a dependency of `freminal-common`):

| Platform  | Path                            | Rationale                                 |
| --------- | ------------------------------- | ----------------------------------------- |
| Linux/BSD | `$XDG_STATE_HOME/freminal/`     | XDG Base Directory spec for runtime state |
| macOS     | `~/Library/Logs/Freminal/`      | macOS convention; visible in Console.app  |
| Windows   | `%LOCALAPPDATA%\Freminal\logs\` | Windows convention for local app data     |

Implementation notes:

- On Linux, `BaseDirs::new()?.state_dir()` returns `$XDG_STATE_HOME` (typically
  `~/.local/state`). Append `freminal/` for our log directory.
  Note: `state_dir()` returns `Option<&Path>` — it is `None` on non-Linux platforms.
- On macOS, `BaseDirs::new()?.home_dir().join("Library/Logs/Freminal")`. The `~/Library/Logs/`
  convention is used by Apple's own apps and is browsable via Console.app.
- On Windows, `BaseDirs::new()?.data_local_dir().join("Freminal\\logs")`.
- Create the directory if it does not exist (reuse existing `create_dir_if_missing`).
- Follow the same platform-conditional `#[cfg(...)]` pattern used by `user_config_path()`.

**Tests to write:**

- Unit test: `log_dir()` returns `Some(...)` (not `None`) on the current platform.
- Unit test: the returned path ends with the expected directory name for the platform.

**Acceptance criteria:**

- `log_dir()` returns the correct platform path
- Directory is created if missing
- `cargo test --all` passes

---

### 9.9 — Make file logging always-on with platform log directory

**Files:**

- `freminal/src/main.rs` (logging initialization)
- `freminal-common/src/config.rs` (`LoggingConfig`)
- `freminal-common/src/args.rs` (CLI args)
- `config_example.toml`

**What to do:**

Replace the opt-in file logging with always-on persistent logging:

1. **Always write to the log directory.** Remove the `write_to_file` gate. The file
   appender is always created, targeting the path from `log_dir()` (subtask 9.8).

2. **Change rotation policy.** Replace `Rotation::HOURLY` / `max_log_files(2)` with
   `Rotation::DAILY` / `max_log_files(7)`. This gives 7 days of history.

3. **File log level: `DEBUG`.** The file appender's layer should use a filter that
   defaults to `DEBUG` (captures mode changes, sequence warnings, all diagnostic info).
   The framework silencers (`winit=off`, `wgpu=off`, etc.) should still apply to the
   file layer.

4. **Stdout log level: `INFO` (unchanged).** Stdout remains at `INFO` as the default.
   `RUST_LOG` continues to override both layers.

5. **Deprecate `--write-logs-to-file` and `[logging] write_to_file`.** Since file
   logging is now always on, these are no longer needed. For backwards compatibility:
   - Keep the CLI flag but make it a no-op. Print a deprecation notice if it is used.
   - Keep the TOML field but ignore it. Document the deprecation in `config_example.toml`.

6. **Add `[logging] level` config option.** A new optional string field that sets the
   file log level. Default: `"debug"`. Accepts standard tracing levels: `"trace"`,
   `"debug"`, `"info"`, `"warn"`, `"error"`.

7. **Log the log file path at startup.** After the subscriber is initialized, emit an
   `info!` message with the log directory path so users can find their logs.

**Tests to write:**

- The logging initialization is side-effectful and hard to unit test. Primary verification
  is manual:
  - Start Freminal normally. Verify log files appear in the platform log directory.
  - Verify `tail -f <logdir>/freminal.YYYY-MM-DD.log` shows real-time output.
  - Verify `--write-logs-to-file` prints a deprecation notice.

**Acceptance criteria:**

- Log files are always written to the platform log directory
- Rotation is daily with 7-day retention
- File log level defaults to DEBUG
- Stdout log level remains INFO
- `RUST_LOG` still works for both layers
- `--write-logs-to-file` is a no-op with deprecation notice
- `[logging] level` config option works
- `cargo test --all` passes

---

### 9.10 — Update settings modal for new logging config

**Files:**

- `freminal/src/gui/settings.rs` (`show_logging_tab`)

**What to do:**

Update the logging tab in the settings modal:

1. Remove the `write_to_file` checkbox (logging is always on).
2. Add a read-only label showing the log directory path.
3. Add a dropdown for `[logging] level` with options: Trace, Debug (default), Info,
   Warn, Error.
4. Add a note: "Log level changes take effect on next launch."

**Tests to write:**

- No unit tests (UI code). Manual verification.

**Acceptance criteria:**

- Settings modal shows the log directory path
- Log level dropdown works and persists to config
- No reference to the removed `write_to_file` option
- `cargo test --all` passes

---

### 9.11 — Log parsed escape sequences at `debug!` level for unknown/unhandled

**Files:**

- `freminal-buffer/src/terminal_handler.rs` (the `other =>` and `Mode::Unknown` arms)
- `freminal-terminal-emulator/src/ansi.rs` (parser error paths)

**What to do:**

Upgrade the visibility of unhandled sequences. Currently, many unhandled or unknown
sequences are logged at `debug!` level, which is filtered out at the default `INFO`
stdout level. Since file logging will now default to `DEBUG`, these messages will
automatically appear in log files without any code changes.

However, review the existing `debug!`/`warn!` messages for:

1. **Consistency** — all unhandled `Mode` variants should log at the same level
   (`debug!`) with a consistent format that includes the raw parameter bytes.
2. **Actionability** — the log message should include enough context to identify the
   sequence. For `Mode::Unknown`, log the raw params. For `TerminalOutput::Unknown*`
   variants in the parser, log the raw bytes.

This subtask is primarily a review/cleanup pass, not a major code change.

**Tests to write:**

- No functional tests needed (logging output only).

**Acceptance criteria:**

- All unhandled modes/sequences log at `debug!` with raw params included
- Log messages are consistent in format
- `cargo test --all` passes

---

### 9.12 — Document logging for users

**Files:**

- `config_example.toml` (update `[logging]` section)

**What to do:**

Update `config_example.toml` to document the new logging behavior:

```toml
[logging]
# Log level for file output. Logs are always written to the platform
# log directory:
#   Linux:   ~/.local/state/freminal/
#   macOS:   ~/Library/Logs/Freminal/
#   Windows: %LOCALAPPDATA%\Freminal\logs\
#
# Valid levels: "trace", "debug", "info", "warn", "error"
# Default: "debug"
#
# To view logs in real time:
#   tail -f ~/.local/state/freminal/freminal.YYYY-MM-DD.log
#
# Log files are rotated daily and kept for 7 days.
# level = "debug"
```

**Acceptance criteria:**

- `config_example.toml` documents the log directory, level option, and rotation policy
- Platform-specific paths are listed
- Real-time viewing instructions are included

---

## Subtask Dependencies

```text
Stream A (tmux):
  9.1 (multi-param split)  ── can start immediately
  9.2 (DECRPM responses)   ── can start immediately (independent of 9.1)
  9.3 (modifier encoding)  ── can start immediately (independent of 9.1, 9.2)
  9.4 (GUI modifier wiring) ── depends on 9.3
  9.5 (DEC private DSR)    ── can start immediately
  9.6 (tmux smoke test)    ── depends on 9.1, 9.2, 9.3, 9.4, 9.5
  9.7 (trace logging)      ── can start immediately

Stream B (logging):
  9.8  (log_dir function)          ── can start immediately
  9.9  (always-on file logging)    ── depends on 9.8
  9.10 (settings modal update)     ── depends on 9.9
  9.11 (debug log review)          ── can start immediately
  9.12 (config documentation)      ── depends on 9.9

Cross-stream: 9.7 (trace logging) benefits from 9.9 (always-on logging) but does
not strictly depend on it — trace output works through RUST_LOG regardless.
```

Parallelism: Subtasks 9.1, 9.2, 9.3, 9.5, 9.7, 9.8, and 9.11 can all run in
parallel. 9.4 waits on 9.3. 9.6 waits on all of Stream A. 9.9 waits on 9.8.
9.10 waits on 9.9. 9.12 waits on 9.9.

---

## Verification

Full verification after all subtasks:

1. `cargo test --all` — all tests pass
2. `cargo clippy --all-targets --all-features -- -D warnings` — no warnings
3. `cargo-machete` — no unused dependencies
4. Manual tmux smoke test (subtask 9.6 procedure) — all steps pass
5. Log files appear in platform log directory without any CLI flags
6. `tail -f <logdir>/freminal.*.log` shows real-time output
7. `RUST_LOG=freminal_terminal_emulator=trace` shows raw PTY bytes and parsed sequences

---

## Files Modified (Expected)

| File                                                                    | Subtask(s) | Changes                                             |
| ----------------------------------------------------------------------- | ---------- | --------------------------------------------------- |
| `freminal-terminal-emulator/src/ansi_components/csi.rs`                 | 9.1        | Split params on `;` before mode dispatch            |
| `freminal-terminal-emulator/src/ansi_components/csi_commands/decrqm.rs` | 9.1        | Same splitting for DECRQM query path                |
| `freminal-common/src/buffer_states/mode.rs`                             | 9.1        | No changes needed (match arms are per-single-param) |
| `freminal-buffer/src/terminal_handler.rs`                               | 9.2, 9.11  | Wire DECRPM responses; review debug log messages    |
| `freminal-terminal-emulator/src/state/internal.rs`                      | 9.2, 9.7   | DECRPM for state-owned modes; trace instrumentation |
| `freminal-terminal-emulator/src/interface.rs`                           | 9.3        | Add `KeyModifiers`, modify `TerminalInput` variants |
| `freminal/src/gui/terminal.rs`                                          | 9.4        | Read egui modifiers, construct `KeyModifiers`       |
| `freminal-terminal-emulator/src/ansi_components/csi_commands/dsr.rs`    | 9.5        | Handle `?6n` variant                                |
| `tests/tmux_smoke.sh`                                                   | 9.6        | New file — manual test procedure                    |
| `freminal-terminal-emulator/src/ansi.rs`                                | 9.7        | Trace-level parsed output logging                   |
| `freminal-common/src/config.rs`                                         | 9.8, 9.9   | `log_dir()` function; `LoggingConfig` changes       |
| `freminal/src/main.rs`                                                  | 9.9        | Always-on file logging; dual-layer subscriber       |
| `freminal-common/src/args.rs`                                           | 9.9        | Deprecate `--write-logs-to-file`                    |
| `config_example.toml`                                                   | 9.12       | Updated `[logging]` documentation                   |
| `freminal/src/gui/settings.rs`                                          | 9.10       | Updated logging tab                                 |
