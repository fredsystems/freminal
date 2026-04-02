# PLAN_26 — Replace Terminal Mode Booleans with Typed Enums

## Status: Complete

---

## Overview

`freminal-common/src/buffer_states/modes/` defines a comprehensive set of typed enums for every
terminal mode (e.g., `Decawm`, `Decanm`, `ApplicationEscapeKey`, `Lnm`, etc.). The mode dispatch
system already routes these enums through `TerminalOutput::Mode(Mode::Xyz(...))` into
`TerminalHandler::process_outputs()` and `TerminalState::sync_mode()`.

However, both `TerminalHandler` and `Buffer` **match on the enum variants and then store the
resolved state as raw `bool` fields**, discarding the type information. This bool state then
propagates through `SnapshotModeFields` → `TerminalSnapshot` → function parameters (`to_payload`,
`send_terminal_inputs`), creating a 3–4 layer chain of raw bools that could be typed enums.

This is a purely mechanical refactor: every target bool already has a corresponding enum. No new
enums need to be created.

**Dependencies:** None (independent, pure refactoring)
**Dependents:** None
**Primary crates:** `freminal-buffer`, `freminal-terminal-emulator`, `freminal`
**Estimated scope:** Medium (6 subtasks)

---

## Current State

### The Two-Tier Anti-Pattern

The dispatch code already receives and matches on the correct enum:

```rust
// terminal_handler.rs:3460-3461
Mode::Decawm(Decawm::AutoWrap) => self.handle_set_wrap(true),
Mode::Decawm(Decawm::NoAutoWrap) => self.handle_set_wrap(false),
```

But then stores the result as a raw bool:

```rust
// buffer.rs:77
wrap_enabled: bool,
```

The enum's type information is discarded at the storage boundary, and all downstream consumers
work with untyped bools.

### Target Fields — `TerminalHandler` (9 mode-bool fields)

`freminal-buffer/src/terminal_handler.rs`, struct at line 118.
`#[allow(clippy::struct_excessive_bools)]` at line 117.

| Field (line)                     | Default | Target Enum             |
| -------------------------------- | ------- | ----------------------- |
| `allow_column_mode_switch` (145) | `true`  | `AllowColumnModeSwitch` |
| `allow_alt_screen` (150)         | `true`  | `AllowAltScreen`        |
| `application_escape_key` (235)   | `false` | `ApplicationEscapeKey`  |
| `sixel_display_mode` (242)       | `false` | `Decsdm`                |
| `private_color_registers` (246)  | `true`  | `PrivateColorRegisters` |
| `nrc_mode` (250)                 | `false` | `Decnrcm`               |
| `reverse_wrap` (256)             | `true`  | `ReverseWrapAround`     |
| `xt_rev_wrap2` (262)             | `false` | `XtRevWrap2`            |
| `vt52_mode` (267)                | `false` | `Decanm`                |

**Not converted:** `in_tmux_passthrough` (line 221) — genuine bookkeeping bool, no corresponding
mode enum. `modify_other_keys_level: u8` (line 229) — not a bool, separate concern.

### Target Fields — `Buffer` (4 mode-bool fields)

`freminal-buffer/src/buffer.rs`, struct at line 22.
`#[allow(clippy::struct_excessive_bools)]` at line 21.

| Field (line)           | Default | Target Enum |
| ---------------------- | ------- | ----------- |
| `lnm_enabled` (72)     | `false` | `Lnm`       |
| `wrap_enabled` (77)    | `true`  | `Decawm`    |
| `decom_enabled` (105)  | `false` | `Decom`     |
| `declrmm_enabled` (96) | `false` | `Declrmm`   |

**Not converted:** `preserve_scrollback_anchor` (line 80) — genuine bookkeeping bool.

### Target Field — `FreminalAnsiParser` (1 field)

`freminal-terminal-emulator/src/ansi.rs`, line 117.

| Field       | Default | Target Enum |
| ----------- | ------- | ----------- |
| `vt52_mode` | `false` | `Decanm`    |

### Target Fields — `SnapshotModeFields` (6 bool fields)

`freminal-terminal-emulator/src/interface.rs`, struct at line 55.
`#[allow(clippy::struct_excessive_bools)]` at line 54.

| Field (line)                  | Target Enum            |
| ----------------------------- | ---------------------- |
| `repeat_keys` (59)            | `Decarm`               |
| `cursor_key_app_mode` (60)    | `Decckm`               |
| `keypad_app_mode` (61)        | `KeypadMode`           |
| `application_escape_key` (64) | `ApplicationEscapeKey` |
| `backarrow_sends_bs` (65)     | `Decbkm`               |
| `alternate_scroll` (66)       | `AlternateScroll`      |

**Not converted:** `skip_draw` (62) — derived from `SynchronizedUpdates` but genuinely binary
rendering decision, no direct mode enum storage benefit. `modify_other_keys: u8` (63) — not a
bool.

### Target Fields — `TerminalSnapshot` (6 bool fields)

`freminal-terminal-emulator/src/snapshot.rs`, struct at line 64.
`#[allow(clippy::struct_excessive_bools)]` at line 62.

| Field (line)                   | Target Enum            |
| ------------------------------ | ---------------------- |
| `repeat_keys` (169)            | `Decarm`               |
| `cursor_key_app_mode` (175)    | `Decckm`               |
| `keypad_app_mode` (182)        | `KeypadMode`           |
| `application_escape_key` (203) | `ApplicationEscapeKey` |
| `backarrow_sends_bs` (209)     | `Decbkm`               |
| `alternate_scroll` (217)       | `AlternateScroll`      |

**Not converted:** `show_cursor` (98) — derived from `Dectcem` but is a pure rendering flag.
`is_alternate_screen` (104), `is_normal_display` (107), `content_changed` (128),
`has_blinking_text` (135), `scroll_changed` (144), `skip_draw` (188) — all genuine bookkeeping.

### Target Function Signatures — `to_payload()` (4 bool params)

`freminal-terminal-emulator/src/input.rs`, line 158.
`#[allow(clippy::fn_params_excessive_bools)]` at line 157.

| Parameter (line)               | Target Enum            |
| ------------------------------ | ---------------------- |
| `decckm_mode` (160)            | `Decckm`               |
| `keypad_mode` (161)            | `KeypadMode`           |
| `application_escape_key` (163) | `ApplicationEscapeKey` |
| `backarrow_sends_bs` (164)     | `Decbkm`               |

**Not converted:** `modify_other_keys: u8` (162) — not a bool.

### Target Function Signatures — `send_terminal_inputs()` (4 bool params)

`freminal/src/gui/terminal.rs`, line 259.
`#[allow(clippy::fn_params_excessive_bools)]` at line 258.

| Parameter (line)               | Target Enum            |
| ------------------------------ | ---------------------- |
| `cursor_key_app_mode` (262)    | `Decckm`               |
| `keypad_app_mode` (263)        | `KeypadMode`           |
| `application_escape_key` (265) | `ApplicationEscapeKey` |
| `backarrow_sends_bs` (266)     | `Decbkm`               |

**Not converted:** `modify_other_keys: u8` (264) — not a bool.

### Clippy Suppressions to be Removed

After the refactor, the following 6 `#[allow(...)]` attributes should be removable:

| Attribute                   | Location              | Line |
| --------------------------- | --------------------- | ---- |
| `struct_excessive_bools`    | `terminal_handler.rs` | 117  |
| `struct_excessive_bools`    | `buffer.rs`           | 21   |
| `struct_excessive_bools`    | `interface.rs`        | 54   |
| `struct_excessive_bools`    | `snapshot.rs`         | 62   |
| `fn_params_excessive_bools` | `input.rs`            | 157  |
| `fn_params_excessive_bools` | `terminal.rs` (gui)   | 258  |

**Not removed:** `struct_excessive_bools` at `terminal.rs:1053` (`FreminalTerminalWidget`) —
unrelated render-cache bools. `struct_excessive_bools` at `state/internal.rs:71`
(`TerminalState`) — this struct has no mode bools left (they are on `TerminalModes` which
already uses enums), but may still have enough bools from other concerns.

### Existing Correct Usage (No Change Needed)

`TerminalModes` (`freminal-common/src/buffer_states/mode.rs:65`) already stores modes as typed
enums: `Decckm`, `RlBracket`, `MouseTrack`, `Decarm`, `Lnm`, `KeypadMode`, `Decbkm`,
`AlternateScroll`, `ReverseWrapAround`, etc. This is the target pattern. The refactor extends
this pattern through the storage and transport layers.

### `Query` Variant Note

Every mode enum has a `Query` variant used only as input to mode dispatch (never stored). After
the refactor, field types technically permit storing `Query` but no code path does. This is
acceptable — the `Query` variant is part of the enum's public API and removing it would break
the dispatch system. Adding newtypes to prevent storage of `Query` is not worth the complexity.

### Identified but Out of Scope

- `modify_other_keys: u8` → `ModifyOtherKeysMode` — not a bool, different concern.
- `show_cursor: bool` in snapshot — derived from `Dectcem` but genuinely binary rendering flag.
- `Cell::is_wide_head` / `is_wide_continuation` → potential `CellWidth` enum — not a mode.
- `append_color_sgr(is_fg: bool)` → potential `ColorTarget` enum — not a mode.
- `FreminalGui::is_playback` — not a terminal mode.

---

## Subtasks

---

### 26.1 — Document Bool-to-Enum Convention in `agents.md`

- **Status:** Complete (2026-04-01)
- **Priority:** 1 — High
- **Scope:** `agents.md` (modify — add convention to `freminal-buffer` and/or cross-cutting
  section)
- **Details:**
  Add a convention rule to `agents.md` under the "Crate-Specific Guidance" or a new
  "Cross-Cutting Conventions" section:

  > **Terminal mode representation rule:** If a terminal mode has a corresponding enum in
  > `freminal-common/src/buffer_states/modes/`, that enum must be used for storage, transport,
  > and function parameters — never a raw `bool`. If no enum exists for a flag, `bool` is fine.

  This ensures future agents follow the convention regardless of whether this refactor has
  been completed.

- **Acceptance criteria:**
  - `agents.md` contains the convention in a discoverable location.
  - The rule is clear and unambiguous.
  - No code changes in this subtask.
- **Tests required:** None (documentation only).

---

### 26.2 — Replace 13 Bool Fields in `TerminalHandler` and `Buffer` with Enums

- **Status:** Complete (2026-04-01)
- **Priority:** 1 — High
- **Scope:**
  - `freminal-buffer/src/terminal_handler.rs` (9 fields + all reader/writer sites)
  - `freminal-buffer/src/buffer.rs` (4 fields + all reader/writer sites)
  - `freminal-buffer/src/terminal_handler.rs` tests (update assertions)
  - `freminal-buffer/tests/` (update integration tests if needed)
- **Details:**
  This is the foundation subtask. All downstream changes (snapshot, function signatures) depend
  on the storage layer using enums.

  **`TerminalHandler` — 9 field conversions:**

  | Old Field → New Field                        | Type Change                      |
  | -------------------------------------------- | -------------------------------- |
  | `allow_column_mode_switch: bool` → same name | `bool` → `AllowColumnModeSwitch` |
  | `allow_alt_screen: bool` → same name         | `bool` → `AllowAltScreen`        |
  | `application_escape_key: bool` → same name   | `bool` → `ApplicationEscapeKey`  |
  | `sixel_display_mode: bool` → same name       | `bool` → `Decsdm`                |
  | `private_color_registers: bool` → same name  | `bool` → `PrivateColorRegisters` |
  | `nrc_mode: bool` → same name                 | `bool` → `Decnrcm`               |
  | `reverse_wrap: bool` → same name             | `bool` → `ReverseWrapAround`     |
  | `xt_rev_wrap2: bool` → same name             | `bool` → `XtRevWrap2`            |
  | `vt52_mode: bool` → same name                | `bool` → `Decanm`                |

  For each field:
  1. Change the field type.
  2. Update the constructor default (e.g., `true` → `AllowColumnModeSwitch::AllowColumnModeSwitch`,
     `false` → `Decsdm::ScrollingMode`).
  3. Update the `process_outputs` match arms (e.g., `self.allow_column_mode_switch = true` →
     `self.allow_column_mode_switch = AllowColumnModeSwitch::AllowColumnModeSwitch`).
  4. Update all reader sites that compare with `true`/`false` to compare with enum variants
     (e.g., `if self.vt52_mode` → `if self.vt52_mode == Decanm::Vt52`).
  5. Update DECRPM query arms to derive the response from the enum value rather than the bool.

  **`Buffer` — 4 field conversions:**

  | Old Field → New Field               | Type Change        |
  | ----------------------------------- | ------------------ |
  | `lnm_enabled: bool` → same name     | `bool` → `Lnm`     |
  | `wrap_enabled: bool` → same name    | `bool` → `Decawm`  |
  | `decom_enabled: bool` → same name   | `bool` → `Decom`   |
  | `declrmm_enabled: bool` → same name | `bool` → `Declrmm` |

  For each field:
  1. Change the field type.
  2. Update the constructor default.
  3. Update setter methods (`set_wrap()`, `set_decom()`, `set_lnm()`, `set_declrmm()`) to accept
     the enum type instead of `bool`.
  4. Update all reader sites.

  **Naming:** Field names may optionally be renamed to match the enum name (e.g.,
  `wrap_enabled` → `decawm` or `wrap_mode`), but this is a style choice. The primary
  requirement is the type change. Keeping existing names with the new type is acceptable.

  **Approach for conditionals:** Where code currently writes `if self.wrap_enabled`, replace
  with `if self.wrap_enabled == Decawm::AutoWrap` (or use `matches!` if preferred). Do NOT
  implement `From<EnumType> for bool` — that defeats the purpose of the refactor.

- **Acceptance criteria:**
  - All 13 fields use the corresponding mode enum type.
  - No `bool`-typed terminal mode fields remain on `TerminalHandler` or `Buffer`.
  - The `process_outputs` match arms store enum values directly (no bool intermediary).
  - DECRPM query responses are derived from the stored enum value.
  - All existing tests pass with updated assertions.
  - `cargo test --all` passes.
  - `cargo clippy --all-targets --all-features -- -D warnings` clean.
- **Tests required:**
  - All existing `TerminalHandler` and `Buffer` unit tests pass.
  - All integration tests in `freminal-buffer/tests/` pass.
  - Verify DECRPM responses for all 13 modes are correct (existing tests cover this).

---

### 26.3 — Replace `FreminalAnsiParser::vt52_mode` with `Decanm`

- **Status:** Complete (2026-04-01)
- **Priority:** 2 — Medium
- **Scope:**
  - `freminal-terminal-emulator/src/ansi.rs` (field change + all 10 usage sites)
  - `freminal-terminal-emulator/src/state/internal.rs` (2 sites in `sync_mode`)
- **Details:**
  Change `FreminalAnsiParser::vt52_mode: bool` (line 117) to `vt52_mode: Decanm`.

  Update usage sites:
  - Constructor: `vt52_mode: false` → `vt52_mode: Decanm::Ansi` (line 144).
  - `push()`: `if self.vt52_mode` → `if self.vt52_mode == Decanm::Vt52` (line 155).
  - `sync_mode()` in `internal.rs`: `self.parser.vt52_mode = true` →
    `self.parser.vt52_mode = Decanm::Vt52` (lines 311, 314).
  - Test assertions: update `assert!(!p.vt52_mode)` →
    `assert_eq!(p.vt52_mode, Decanm::Ansi)` etc. (lines 1026, 1262, 1266, 1287).

- **Acceptance criteria:**
  - `FreminalAnsiParser::vt52_mode` is typed `Decanm`.
  - All VT52 mode tests pass.
  - Parser correctly routes to VT52 vs ANSI state machine.
- **Tests required:**
  - Existing VT52 parser tests pass.
  - `cargo test --all` passes.

---

### 26.4 — Replace `SnapshotModeFields` and `TerminalSnapshot` Bool Fields with Enums

- **Status:** Complete (2026-04-01)
- **Priority:** 2 — Medium
- **Scope:**
  - `freminal-terminal-emulator/src/interface.rs` (`SnapshotModeFields`, `build_snapshot`)
  - `freminal-terminal-emulator/src/snapshot.rs` (`TerminalSnapshot`)
  - `freminal/src/gui/terminal.rs` (GUI reader sites)
  - `freminal/src/gui/mod.rs` (GUI reader sites for `alternate_scroll`)
- **Details:**
  **`SnapshotModeFields` — 6 field conversions:**

  | Old Field                      | New Type               |
  | ------------------------------ | ---------------------- |
  | `repeat_keys: bool`            | `Decarm`               |
  | `cursor_key_app_mode: bool`    | `Decckm`               |
  | `keypad_app_mode: bool`        | `KeypadMode`           |
  | `application_escape_key: bool` | `ApplicationEscapeKey` |
  | `backarrow_sends_bs: bool`     | `Decbkm`               |
  | `alternate_scroll: bool`       | `AlternateScroll`      |

  **`TerminalSnapshot` — same 6 fields, same conversions.**

  The `build_snapshot()` method in `interface.rs` currently reads the enums from
  `TerminalModes` and converts to bools:

  ```rust
  repeat_keys: self.internal.modes.repeat_keys == Decarm::AutoRepeat,
  ```

  After the refactor, it passes the enum directly:

  ```rust
  repeat_keys: self.internal.modes.repeat_keys.clone(),
  ```

  The GUI reader sites must be updated to compare with enum variants instead of bools.
  For example, in `terminal.rs` where `snap.alternate_scroll` is checked:

  ```rust
  // Before:
  if snap.alternate_scroll { ... }
  // After:
  if snap.alternate_scroll == AlternateScroll::Enabled { ... }
  ```

  **Note:** `SnapshotModeFields` may optionally be renamed to better reflect that it now
  carries typed mode values, but this is not required.

- **Acceptance criteria:**
  - All 6 fields on both `SnapshotModeFields` and `TerminalSnapshot` use typed enums.
  - `build_snapshot()` passes enums directly from `TerminalModes` without bool conversion.
  - All GUI reader sites updated to compare with enum variants.
  - `cargo test --all` passes.
  - `cargo clippy --all-targets --all-features -- -D warnings` clean.
- **Tests required:**
  - All existing snapshot and GUI tests pass.
  - `cargo test --all` passes.

---

### 26.5 — Replace `to_payload()` and `send_terminal_inputs()` Bool Params with Enums

- **Status:** Complete (2026-04-01)
- **Priority:** 2 — Medium
- **Scope:**
  - `freminal-terminal-emulator/src/input.rs` (`to_payload` signature + body)
  - `freminal/src/gui/terminal.rs` (`send_terminal_inputs` signature + body, all call sites)
- **Details:**
  **`to_payload()` — 4 parameter conversions:**

  | Old Parameter                  | New Type               |
  | ------------------------------ | ---------------------- |
  | `decckm_mode: bool`            | `Decckm`               |
  | `keypad_mode: bool`            | `KeypadMode`           |
  | `application_escape_key: bool` | `ApplicationEscapeKey` |
  | `backarrow_sends_bs: bool`     | `Decbkm`               |

  **`send_terminal_inputs()` — same 4 parameters, same conversions.**

  Inside `to_payload()`, conditionals like `if decckm_mode` become
  `if decckm_mode == Decckm::Application`. The match arms in the key encoding logic
  must be updated accordingly.

  All call sites (in `send_terminal_inputs` and any test code) must pass enum values
  instead of bools. After 26.4 is complete, the snapshot already carries enum values,
  so call sites simply pass `snap.cursor_key_app_mode` directly (already the correct type).

  **This subtask depends on 26.4** — the snapshot must carry enums before function
  signatures can accept them without a conversion step at the call site.

- **Acceptance criteria:**
  - `to_payload()` and `send_terminal_inputs()` accept typed enums, not bools.
  - No bool-to-enum conversion at any call site.
  - All key encoding logic produces identical output.
  - `cargo test --all` passes.
  - `cargo clippy --all-targets --all-features -- -D warnings` clean.
- **Tests required:**
  - All existing input encoding tests pass.
  - Key encoding tests for DECCKM, keypad mode, application escape key, DECBKM all pass.
  - `cargo test --all` passes.

---

### 26.6 — Remove Clippy Bool Suppression Attributes

- **Status:** Complete (2026-04-01)
- **Priority:** 3 — Low
- **Scope:**
  - `freminal-buffer/src/terminal_handler.rs` (line 117)
  - `freminal-buffer/src/buffer.rs` (line 21)
  - `freminal-terminal-emulator/src/interface.rs` (line 54)
  - `freminal-terminal-emulator/src/snapshot.rs` (line 62)
  - `freminal-terminal-emulator/src/input.rs` (line 157)
  - `freminal/src/gui/terminal.rs` (line 258)
- **Details:**
  After subtasks 26.2–26.5 are complete, the following `#[allow(...)]` attributes should
  be removable:
  1. `#[allow(clippy::struct_excessive_bools)]` on `TerminalHandler` (line 117) — 9 bools
     converted to enums; remaining bools (`in_tmux_passthrough`) should be under the
     threshold.
  2. `#[allow(clippy::struct_excessive_bools)]` on `Buffer` (line 21) — 4 bools converted;
     remaining bool (`preserve_scrollback_anchor`) is under the threshold.
  3. `#[allow(clippy::struct_excessive_bools)]` on `SnapshotModeFields` (line 54) — 6 bools
     converted; remaining (`skip_draw`) is under the threshold.
  4. `#[allow(clippy::struct_excessive_bools)]` on `TerminalSnapshot` (line 62) — 6 mode
     bools converted. Remaining bools (`show_cursor`, `is_alternate_screen`,
     `is_normal_display`, `content_changed`, `has_blinking_text`, `scroll_changed`,
     `skip_draw`) are genuine bookkeeping — check if the count still exceeds clippy's
     threshold (default 3). If so, the attribute may need to stay with an updated comment.
  5. `#[allow(clippy::fn_params_excessive_bools)]` on `to_payload` (line 157) — 4 bool
     params converted.
  6. `#[allow(clippy::fn_params_excessive_bools)]` on `send_terminal_inputs` (line 258) —
     4 bool params converted.

  **Important:** Remove each attribute, run `cargo clippy`, and verify no new warnings.
  If `TerminalSnapshot` still exceeds the threshold due to its remaining 7 bookkeeping bools,
  keep the suppress with an updated comment explaining which bools remain and why they are
  genuine.

  Also check `TerminalState` (`state/internal.rs:71`) — it currently has the suppress but
  has no mode bools left after the `TerminalModes` migration. If the remaining fields are
  under the threshold, remove it.

- **Acceptance criteria:**
  - All removable `#[allow(clippy::*_excessive_bools)]` attributes deleted.
  - Any retained attributes have updated comments explaining why.
  - `cargo clippy --all-targets --all-features -- -D warnings` clean.
- **Tests required:**
  - `cargo test --all` passes (no behavior change).
  - `cargo clippy --all-targets --all-features -- -D warnings` clean.

---

## Implementation Notes

### Subtask Ordering

Strict sequential ordering is required:

```text
26.1 (document convention) — can be done at any time
26.2 (TerminalHandler + Buffer) — foundation, must be first code change
26.3 (FreminalAnsiParser) — independent of 26.2, can run in parallel
26.4 (SnapshotModeFields + TerminalSnapshot) — depends on 26.2 (reads handler fields)
26.5 (to_payload + send_terminal_inputs) — depends on 26.4 (reads snapshot fields)
26.6 (remove clippy suppressions) — depends on 26.2–26.5 all complete
```

**Recommended order:** 26.1 → 26.2 + 26.3 (parallel) → 26.4 → 26.5 → 26.6

### Risk Assessment

All subtasks are pure refactoring — no behavior change. The primary risks:

- **Enum variant naming inconsistency:** Some enums have verbose variant names
  (e.g., `AllowColumnModeSwitch::AllowColumnModeSwitch` for the "enabled" variant).
  This is pre-existing and out of scope for this task.
- **`Query` variant storage:** After the refactor, nothing prevents storing `Query` in a
  field. No code path does this, and adding newtypes is not worth the complexity.
- **Merge conflicts:** `terminal_handler.rs` (9,098 lines) and `buffer.rs` (6,624 lines)
  are large files touched by many tasks. Minimize the time the branch is open.

### Approach for Conditionals

Where code currently writes:

```rust
if self.wrap_enabled { ... }
```

Replace with explicit enum comparison:

```rust
if self.wrap_enabled == Decawm::AutoWrap { ... }
```

Do NOT implement `From<EnumType> for bool` or add `.is_enabled()` helper methods — that
re-introduces the bool indirection this refactor eliminates. The match on enum variants
makes the intent explicit and grep-able.

### Bright-Line Rule

**If a terminal mode has a corresponding enum in `freminal-common/src/buffer_states/modes/`,
use the enum everywhere. If no enum exists, `bool` is fine.**

This is simple, unambiguous, and requires zero judgment calls.

### Verification

Each subtask must pass before proceeding:

- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo-machete`

---

## References

- `freminal-common/src/buffer_states/modes/` — all mode enum definitions (18 enums)
- `freminal-common/src/buffer_states/mode.rs` — `TerminalModes` struct (line 65), `Mode` enum
- `freminal-buffer/src/terminal_handler.rs` — `TerminalHandler` struct (line 118),
  `process_outputs` mode dispatch (~line 3398–3680)
- `freminal-buffer/src/buffer.rs` — `Buffer` struct (line 22)
- `freminal-terminal-emulator/src/ansi.rs` — `FreminalAnsiParser` (line 117)
- `freminal-terminal-emulator/src/interface.rs` — `SnapshotModeFields` (line 55),
  `build_snapshot()` method
- `freminal-terminal-emulator/src/snapshot.rs` — `TerminalSnapshot` (line 64)
- `freminal-terminal-emulator/src/input.rs` — `to_payload()` (line 158)
- `freminal/src/gui/terminal.rs` — `send_terminal_inputs()` (line 259)
- `freminal-terminal-emulator/src/state/internal.rs` — `sync_mode()` (line 244)
