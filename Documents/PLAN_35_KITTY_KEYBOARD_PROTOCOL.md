# PLAN 35 — Kitty Keyboard Protocol

## Status: Pending

---

## Overview

Implement the [Kitty Keyboard Protocol (KKP)](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
— a modern, unambiguous keyboard encoding designed to eliminate all the historical ambiguities in
legacy xterm/VT key encoding.

KKP is increasingly required for correctness with modern TUI programs such as Neovim (requires
KKP for key disambiguation in many plugins), Helix, Kakoune, Emacs (evil-mode), and others. These
programs query for KKP support via `CSI ? u` and fall back to legacy encoding only when the
terminal reports `CSI ? 0 u` (no flags set). Freminal currently reports `CSI ? 0 u` unconditionally,
which is correct for "protocol not active" but means these programs never benefit from unambiguous
key encoding.

### What KKP solves

Legacy xterm key encoding has several well-known ambiguities:

| Problem                           | Example                                                          |
| --------------------------------- | ---------------------------------------------------------------- |
| `ESC` vs start of escape sequence | `ESC` and `ESC [` are indistinguishable at the byte level        |
| Ctrl+letter vs control character  | `Ctrl+M` = `\r`, `Ctrl+I` = `\t`, `Ctrl+[` = `ESC`               |
| Shifted key vs unshifted          | `Shift+a` = `A`; the terminal cannot distinguish from typing `A` |
| Alt+letter vs Meta prefix         | `Alt+a` = `ESC a`; ambiguous with ESC followed by `a`            |
| Key release                       | Never reported in legacy encoding                                |
| Key repeat                        | Indistinguishable from new press in legacy encoding              |

KKP uses a structured `CSI u` format: `CSI keycode ; modifiers : event-type u` where every
field is explicit and unambiguous. All existing legacy encoding remains valid as the baseline.

### Protocol Revision being targeted

This plan targets **KKP revision 1** as documented on
<https://sw.kovidgoyal.net/kitty/keyboard-protocol/>. The flag bitmask and progressive
enhancement model come from that specification.

### Scope

This plan implements the **terminal emulator side** of KKP — the sequences the terminal receives
from programs requesting keyboard mode changes, and the key event encoding the terminal sends back.
It does **not** change the xterm/VT input encoding that Freminal uses internally (the
`TerminalInput` + `to_payload()` system); rather, KKP encoding runs as a parallel path when the
protocol is active.

**Dependencies:** None
**Dependents:** None
**Primary crates:** `freminal-common`, `freminal-terminal-emulator`, `freminal-buffer`, `freminal`
**Estimated scope:** Medium-high (11 subtasks)

---

## Protocol Reference

### Sequences Received by the Terminal (PTY-to-terminal direction)

| Sequence               | Meaning                                                              |
| ---------------------- | -------------------------------------------------------------------- |
| `CSI ? u`              | Query current mode stack top. Respond with `CSI ? flags u`.          |
| `CSI > flags u`        | Push `flags` onto the mode stack. `flags` is a bitmask.              |
| `CSI < number u`       | Pop `number` entries from the mode stack (default 1).                |
| `CSI = flags ; mode u` | Set current flags. `mode` selects replacement semantics (default 1). |

### Mode Flags Bitmask

| Bit | Decimal | Name                              | Meaning                                                        |
| --- | ------- | --------------------------------- | -------------------------------------------------------------- |
| 0   | 1       | `DISAMBIGUATE_ESCAPE`             | Disambiguate modifier keys that produce C0 codes (Ctrl+letter) |
| 1   | 2       | `REPORT_EVENT_TYPES`              | Report key repeat and key release events, not just press       |
| 2   | 4       | `REPORT_ALTERNATE_KEYS`           | Report shifted/alt key codes in addition to base key code      |
| 3   | 8       | `REPORT_ALL_KEYS_AS_ESCAPE_CODES` | Report every key as a CSI u sequence (no bare ASCII)           |
| 4   | 16      | `REPORT_ASSOCIATED_TEXT`          | Include associated Unicode text in the key event               |

Flags 0–1 are the minimum required for Neovim. Flags 2–4 extend the protocol progressively.

### `CSI = mode` Values

| Value       | Meaning                                                                      |
| ----------- | ---------------------------------------------------------------------------- |
| 1 (default) | Set all specified bits, reset all unspecified bits (replace)                 |
| 2           | Set all specified bits, leave unspecified bits unchanged (OR)                |
| 3           | Reset all specified bits, leave unspecified bits unchanged (AND-NOT / clear) |

### Key Event Format (terminal-to-PTY direction, i.e. keys Freminal sends)

KKP uses two forms of encoding:

**Form 1 — `CSI u` encoding** (for text keys and certain functional keys):

```text
CSI keycode [; modifiers [: event-type]] u
```

**Form 2 — Legacy functional encoding** (for arrow keys, Home, End, F1–F12, etc.):

```text
CSI number ; modifiers ~
CSI 1 ; modifiers [ABCDEFHPQS]
```

Fields:

- `keycode`: Unicode codepoint of the key (always lowercase/unshifted), or a KKP-defined
  code from the Private Use Area for non-Unicode keys
- `modifiers`: bitmask — `1 + (shift?1:0) + (alt?2:0) + (ctrl?4:0) + (super?8:0) + (hyper?16:0) + (meta?32:0) + (capslock?64:0) + (numlock?128:0)`. **Note:** the modifier bitmask base is 1 (not 0). No modifier held = parameter omitted or `1`.
- `event-type`: `1` = press (default, omitted when no other fields follow), `2` = repeat, `3` = release

#### Functional key encoding table

These keys use **legacy encoding** (NOT `CSI u`). With modifiers, the modifier parameter is
inserted: e.g., Ctrl+Up = `CSI 1;5 A`, Shift+F5 = `CSI 15;2 ~`.

| Key       | Encoding (no modifiers) | Encoding (with modifiers) |
| --------- | ----------------------- | ------------------------- |
| Insert    | `CSI 2 ~`               | `CSI 2 ; mods ~`          |
| Delete    | `CSI 3 ~`               | `CSI 3 ; mods ~`          |
| Page Up   | `CSI 5 ~`               | `CSI 5 ; mods ~`          |
| Page Down | `CSI 6 ~`               | `CSI 6 ; mods ~`          |
| Up        | `CSI A`                 | `CSI 1 ; mods A`          |
| Down      | `CSI B`                 | `CSI 1 ; mods B`          |
| Right     | `CSI C`                 | `CSI 1 ; mods C`          |
| Left      | `CSI D`                 | `CSI 1 ; mods D`          |
| Home      | `CSI H`                 | `CSI 1 ; mods H`          |
| End       | `CSI F`                 | `CSI 1 ; mods F`          |
| F1        | `CSI P`                 | `CSI 1 ; mods P`          |
| F2        | `CSI Q`                 | `CSI 1 ; mods Q`          |
| F3        | `CSI 13 ~`              | `CSI 13 ; mods ~`         |
| F4        | `CSI S`                 | `CSI 1 ; mods S`          |
| F5        | `CSI 15 ~`              | `CSI 15 ; mods ~`         |
| F6        | `CSI 17 ~`              | `CSI 17 ; mods ~`         |
| F7        | `CSI 18 ~`              | `CSI 18 ; mods ~`         |
| F8        | `CSI 19 ~`              | `CSI 19 ; mods ~`         |
| F9        | `CSI 20 ~`              | `CSI 20 ; mods ~`         |
| F10       | `CSI 21 ~`              | `CSI 21 ; mods ~`         |
| F11       | `CSI 23 ~`              | `CSI 23 ; mods ~`         |
| F12       | `CSI 24 ~`              | `CSI 24 ; mods ~`         |

These keys use **`CSI u` encoding** (C0 legacy codes repurposed by KKP):

| Key       | KKP code | Notes                    |
| --------- | -------- | ------------------------ |
| Escape    | 27       | Legacy: bare `0x1b`      |
| Enter     | 13       | Legacy: `0x0d`           |
| Tab       | 9        | Legacy: `0x09`           |
| Backspace | 127      | Legacy: `0x7f` or `0x08` |

These keys use **`CSI u` encoding** with Private Use Area codes (F13+, locks, etc.):

| Key          | KKP code    |
| ------------ | ----------- |
| Caps Lock    | 57358       |
| Scroll Lock  | 57359       |
| Num Lock     | 57360       |
| Print Screen | 57361       |
| Pause        | 57362       |
| Menu         | 57363       |
| F13–F35      | 57376–57398 |

### XTGETTCAP capability

Programs may also query `u` (the capability name for KKP flags) via XTGETTCAP. When KKP is
supported, the response is the current mode-stack top flags as a decimal string. Kitty itself
responds with `u=0` when the stack is empty (no flags active). We add `"u"` to `lookup_termcap`
returning the current flags as a string.

---

## Architecture

### New data: mode stack

The KKP state is a stack of `u32` flag values. Programs push new flags on entry, pop on exit.
The maximum stack depth in Kitty's reference implementation is 256. A `Vec<u32>` bounded to 256
entries is sufficient.

The current flags are `stack.last().copied().unwrap_or(0)`.

**Separate stacks for main and alternate screens.** The spec requires that the main and alternate
screens maintain independent keyboard mode stacks. When `enter_alternate_screen` is called, the
current stack is saved and a fresh empty stack is started. When `leave_alternate_screen` is
called, the alternate stack is discarded and the saved main stack is restored. This follows the
same save/restore pattern used for `SavedPrimaryState` in `Buffer`.

### Where state lives

Following the same pattern as every other terminal mode:

1. `freminal-common/src/buffer_states/modes/kitty_keyboard.rs` — the mode type
2. `TerminalHandler` field — the stack
3. `TerminalOutput` variants — `KittyKeyboardPush(u32)`, `KittyKeyboardPop(u32)`,
   `KittyKeyboardSet { flags: u32, mode: u32 }` (alongside the existing `KittyKeyboardQuery`)
4. `TerminalSnapshot` field — `kitty_keyboard_flags: u32` (stack top, 0 if empty)
5. `InputModes` field — `kitty_keyboard_flags: u32`
6. `TerminalInput::to_payload()` — gains a `kitty_keyboard_flags: u32` parameter; new encoding
   path when `flags != 0`

### Parser changes

`scorc.rs` already stubs the push and pop cases:

- `Some(b'>')` → silently ignores (Kitty push)
- `Some(b'<')` → silently ignores (Kitty pop)
- `Some(b'=')` would need to be added (Kitty set-bottom)

These need to emit real `TerminalOutput` variants.

The query path (`Some(b'?')`) already emits `TerminalOutput::KittyKeyboardQuery`. The handler in
`terminal_handler.rs` (line 4072) already responds with `\x1b[?0u`. After this plan, that
response should use the actual current stack-top flags.

---

## Affected Files

| File                                                                   | Change                                                                                                                                                  |
| ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `freminal-common/src/buffer_states/modes/kitty_keyboard.rs`            | **New file** — `KittyKeyboardMode` type                                                                                                                 |
| `freminal-common/src/buffer_states/modes/mod.rs`                       | Add `pub mod kitty_keyboard;`                                                                                                                           |
| `freminal-common/src/buffer_states/terminal_output.rs`                 | Add `KittyKeyboardPush(u32)`, `KittyKeyboardPop(u32)`, `KittyKeyboardSet { flags: u32, mode: u32 }` to `TerminalOutput`                                 |
| `freminal-terminal-emulator/src/ansi_components/csi_commands/scorc.rs` | Implement push, pop, and set arms                                                                                                                       |
| `freminal-terminal-emulator/src/ansi_components/csi_commands/mod.rs`   | No change needed (scorc module already listed)                                                                                                          |
| `freminal-buffer/src/terminal_handler.rs`                              | Add `kitty_keyboard_stack: Vec<u32>` field, `kitty_keyboard_flags()` accessor, handle `KittyKeyboardPush/Pop/Set`, update `KittyKeyboardQuery` response |
| `freminal-buffer/src/terminal_handler.rs` (XTGETTCAP)                  | Add `"u"` to `lookup_termcap`                                                                                                                           |
| `freminal-terminal-emulator/src/snapshot.rs`                           | Add `kitty_keyboard_flags: u32` field to `TerminalSnapshot` and `TerminalSnapshot::empty()`                                                             |
| `freminal-terminal-emulator/src/interface.rs`                          | Add `kitty_keyboard_flags` to `SnapshotModeFields` and `collect_mode_fields()`                                                                          |
| `freminal-terminal-emulator/src/input.rs`                              | Add `kitty_keyboard_flags: u32` param to `to_payload()`; implement KKP encoding path                                                                    |
| `freminal/src/gui/terminal.rs`                                         | Add `kitty_keyboard_flags` to `InputModes`, `InputModes::from_snapshot()`, and `send_terminal_inputs()`                                                 |
| `freminal-terminal-emulator/tests/terminal_input_payload.rs`           | New test cases for KKP encoding                                                                                                                         |

---

## Subtask List

> **Agent instructions:** Find the first unchecked subtask. Execute it and nothing else.
> Update its checkbox to `[x]` and add a brief completion note. Run the verification suite.
> Then **stop and wait for user confirmation** before touching anything else.

---

### Subtask 35.1 — Create `KittyKeyboardMode` type in `freminal-common`

**File:** `freminal-common/src/buffer_states/modes/kitty_keyboard.rs` (new file)
**File:** `freminal-common/src/buffer_states/modes/mod.rs` (add `pub mod kitty_keyboard;`)

Define a `KittyKeyboardFlags` newtype wrapping `u32`, representing the KKP mode bitmask. Export
the named bit-flag constants. Also define a `KittyKeyboardStack` type alias (`Vec<KittyKeyboardFlags>`).

```rust
/// Kitty Keyboard Protocol flag bits (CSI > flags u / CSI ? u).
///
/// Bit 0 (1):  DISAMBIGUATE_ESCAPE_CODES
/// Bit 1 (2):  REPORT_EVENT_TYPES
/// Bit 2 (4):  REPORT_ALTERNATE_KEYS
/// Bit 3 (8):  REPORT_ALL_KEYS_AS_ESCAPE_CODES
/// Bit 4 (16): REPORT_ASSOCIATED_TEXT
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct KittyKeyboardFlags(pub u32);

impl KittyKeyboardFlags {
    pub const NONE: Self = Self(0);
    pub const DISAMBIGUATE_ESCAPE_CODES: Self = Self(1);
    pub const REPORT_EVENT_TYPES: Self = Self(2);
    pub const REPORT_ALTERNATE_KEYS: Self = Self(4);
    pub const REPORT_ALL_KEYS_AS_ESCAPE_CODES: Self = Self(8);
    pub const REPORT_ASSOCIATED_TEXT: Self = Self(16);

    /// Maximum stack depth per the Kitty protocol specification.
    pub const MAX_STACK_DEPTH: usize = 256;

    /// Returns the inner `u32` flag value.
    pub const fn bits(self) -> u32 { self.0 }

    /// Returns `true` when no flags are set.
    pub const fn is_empty(self) -> bool { self.0 == 0 }
}
```

No `ReportMode` trait needed — KKP uses a stack model, not a DECRQM mode.

**Unit tests to write:**

- `default_is_zero()` — `KittyKeyboardFlags::default().bits() == 0`
- `named_constants_have_correct_bit_values()` — each constant matches expected value
- `is_empty_true_for_none()` and `is_empty_false_for_nonzero()`

**Verification:** `cargo test --all` passes. `cargo clippy --all-targets --all-features -- -D warnings` clean.

- [ ] Subtask 35.1 complete

---

### Subtask 35.2 — Add `KittyKeyboardPush`, `KittyKeyboardPop`, `KittyKeyboardSet` to `TerminalOutput`

**File:** `freminal-common/src/buffer_states/terminal_output.rs`

Add three new variants alongside the existing `KittyKeyboardQuery`:

```rust
/// CSI > flags u — Push keyboard flags onto the Kitty keyboard protocol stack.
///
/// `flags` is the raw bitmask from the CSI parameter.
KittyKeyboardPush(u32),

/// CSI < number u — Pop `number` entries from the Kitty keyboard protocol stack.
///
/// If `number` is 0 or absent, defaults to 1.  Popping more entries than are
/// on the stack empties the stack and resets all flags (not an error per the spec).
KittyKeyboardPop(u32),

/// CSI = flags ; mode u — Set the current Kitty keyboard protocol flags.
///
/// `flags` is the bitmask; `mode` is 1 (replace, default), 2 (OR), or 3 (AND-NOT / clear).
KittyKeyboardSet { flags: u32, mode: u32 },
```

Also add `Display` arms for each new variant (required by the `impl Display for TerminalOutput`
block already in the file):

```rust
Self::KittyKeyboardPush(f) => write!(f, "KittyKeyboardPush({f})"),
Self::KittyKeyboardPop(n) => write!(f, "KittyKeyboardPop({n})"),
Self::KittyKeyboardSet { flags, mode } => {
    write!(f, "KittyKeyboardSet(flags={flags}, mode={mode})")
}
```

**Verification:** `cargo test --all` passes. `cargo clippy --all-targets --all-features -- -D warnings` clean.

- [ ] Subtask 35.2 complete

---

### Subtask 35.3 — Implement push/pop/set parsing in `scorc.rs`

**File:** `freminal-terminal-emulator/src/ansi_components/csi_commands/scorc.rs`

Replace the silent `trace!` stubs for `b'>'`, `b'<'`, and add `b'='` with real parsing:

```rust
Some(b'>') => {
    // CSI > flags u — Kitty keyboard push
    let flags = parse_decimal(&params[1..]).unwrap_or(0);
    output.push(TerminalOutput::KittyKeyboardPush(flags));
}
Some(b'<') => {
    // CSI < number u — Kitty keyboard pop
    let n = parse_decimal(&params[1..]).unwrap_or(1).max(1);
    output.push(TerminalOutput::KittyKeyboardPop(n));
}
Some(b'=') => {
    // CSI = flags ; mode u — Kitty keyboard set
    // params[1..] is like b"3;1" or b"3" etc.
    let (flags, mode) = parse_two_params(&params[1..]);
    // mode defaults to 1 (replace) per the spec
    let mode = if mode == 0 { 1 } else { mode };
    output.push(TerminalOutput::KittyKeyboardSet { flags, mode });
}
```

Add a private `parse_decimal(bytes: &[u8]) -> Option<u32>` and
`parse_two_params(bytes: &[u8]) -> (u32, u32)` helper at the bottom of the file (not pub).

Update the module-level doc comment to reflect the new behaviour.

**Unit tests to add** (in the existing `#[cfg(test)]` block):

- `csi_gt_u_push_flags_0()` — `CSI > 0 u` → `KittyKeyboardPush(0)`
- `csi_gt_u_push_flags_27()` — `CSI > 27 u` → `KittyKeyboardPush(27)`
- `csi_lt_u_pop_default_1()` — `CSI < u` → `KittyKeyboardPop(1)`
- `csi_lt_u_pop_explicit_3()` — `CSI < 3 u` → `KittyKeyboardPop(3)`
- `csi_eq_u_set_replace()` — `CSI = 3 u` → `KittyKeyboardSet { flags: 3, mode: 1 }`
- `csi_eq_u_set_or()` — `CSI = 3;2 u` → `KittyKeyboardSet { flags: 3, mode: 2 }`
- `csi_eq_u_set_clear()` — `CSI = 3;3 u` → `KittyKeyboardSet { flags: 3, mode: 3 }`

**Verification:** `cargo test --all` passes. `cargo clippy --all-targets --all-features -- -D warnings` clean.

- [ ] Subtask 35.3 complete

---

### Subtask 35.4 — Implement KKP stack in `TerminalHandler`

**File:** `freminal-buffer/src/terminal_handler.rs`

1. Add field:

   ```rust
   /// Kitty keyboard protocol mode stack.
   ///
   /// Each entry is a `u32` bitmask.  Programs push on entry and pop on exit.
   /// The active flags are `kitty_keyboard_stack.last().copied().unwrap_or(0)`.
   /// Bounded to `KittyKeyboardFlags::MAX_STACK_DEPTH` (256) entries.
   kitty_keyboard_stack: Vec<u32>,
   ```

   Initialize to `Vec::new()` in `TerminalHandler::new()`.

2. Add accessor:

   ```rust
   /// Returns the currently active Kitty keyboard protocol flags.
   ///
   /// Returns `0` when the stack is empty (protocol not active).
   pub const fn kitty_keyboard_flags(&self) -> u32 {
       // Can't use .last() in const fn; caller reads stack.last() directly.
       // Expose as a method so callers don't need to know the field name.
   }
   ```

   Since `const fn` cannot call `slice::last()`, implement as a regular `fn`:

   ```rust
   pub fn kitty_keyboard_flags(&self) -> u32 {
       self.kitty_keyboard_stack.last().copied().unwrap_or(0)
   }
   ```

3. Update `KittyKeyboardQuery` arm (line 4072) to respond with actual current flags:

   ```rust
   TerminalOutput::KittyKeyboardQuery => {
       let flags = self.kitty_keyboard_flags();
       self.write_to_pty(&format!("\x1b[?{flags}u"));
   }
   ```

4. Add new arms in `process_output()` (after `KittyKeyboardQuery`):

   ```rust
   TerminalOutput::KittyKeyboardPush(flags) => {
       if self.kitty_keyboard_stack.len() >= KittyKeyboardFlags::MAX_STACK_DEPTH {
           // Evict the oldest entry (bottom of the stack) per the spec.
           self.kitty_keyboard_stack.remove(0);
       }
       self.kitty_keyboard_stack.push(*flags);
   }
   TerminalOutput::KittyKeyboardPop(n) => {
       let n = (*n as usize).min(self.kitty_keyboard_stack.len());
       let new_len = self.kitty_keyboard_stack.len() - n;
       self.kitty_keyboard_stack.truncate(new_len);
   }
   TerminalOutput::KittyKeyboardSet { flags, mode } => {
       // CSI = flags ; mode u — set current flags
       // This operates on the current flags (stack top or implicit 0).
       let current = self.kitty_keyboard_flags();
       let new_flags = match mode {
           1 => *flags,                     // replace: set bits as given, reset all others
           2 => current | *flags,           // OR: set specified bits, leave others
           3 => current & !*flags,          // AND-NOT: clear specified bits, leave others
           _ => current,                    // Unknown mode: no change
       };
       // If the stack is empty, push the new value; otherwise replace the top.
       if self.kitty_keyboard_stack.is_empty() {
           self.kitty_keyboard_stack.push(new_flags);
       } else {
           let top = self.kitty_keyboard_stack.len() - 1;
           self.kitty_keyboard_stack[top] = new_flags;
       }
   }
   ```

5. Add `"u"` to `lookup_termcap` in the XTGETTCAP section. Since `lookup_termcap` is a static
   function and the stack is instance state, change the design slightly: `handle_xtgettcap` must
   handle `"u"` directly before calling `lookup_termcap`, since it needs instance access:

   ```rust
   // In handle_xtgettcap, after decoding cap_name:
   if cap_name == "u" {
       let flags = self.kitty_keyboard_flags();
       let hex_value = Self::hex_encode(&flags.to_string());
       self.write_dcs_response(&format!("1+r{hex_name}={hex_value}"));
       continue;
   }
   ```

6. **Save/restore stack on alternate screen transitions.** Add a
   `saved_kitty_keyboard_stack: Option<Vec<u32>>` field to `TerminalHandler`. When
   `enter_alternate_screen` is called, save the current stack into this field and replace
   `kitty_keyboard_stack` with an empty `Vec`. When `leave_alternate_screen` is called,
   restore the saved stack (if `Some`). This ensures programs using the alternate screen
   (editors, TUI apps) get an independent keyboard mode context.

**Unit tests to write** (in a new `#[cfg(test)]` block at the bottom of a new
`freminal-buffer/tests/kitty_keyboard_stack.rs` integration test file, following the pattern of
`terminal_handler_integration.rs`):

- `push_sets_active_flags()` — push 3, flags() returns 3
- `push_stack_top_wins()` — push 1, push 2, flags() returns 2
- `pop_restores_previous()` — push 1, push 2, pop 1, flags() returns 1
- `pop_clears_stack()` — push 1, pop 1, flags() returns 0
- `pop_more_than_stack_does_not_panic()` — push 1, pop 5, flags() returns 0
- `set_replace_updates_top()` — push 3, set flags=5 mode=1, flags() returns 5
- `set_or_merges()` — push 3, set flags=4 mode=2, flags() returns 7
- `set_and_not_clears_bits()` — push 7, set flags=2 mode=3, flags() returns 5
- `set_on_empty_stack_creates_entry()` — set flags=3 mode=1, flags() returns 3
- `query_responds_with_current_flags()` — push 3, process KittyKeyboardQuery, PTY receives `\x1b[?3u`
- `max_stack_depth_evicts_oldest()` — push 257 times, stack len stays 256, top is the last pushed value

**Verification:** `cargo test --all` passes. `cargo clippy --all-targets --all-features -- -D warnings` clean. `cargo-machete` clean.

- [ ] Subtask 35.4 complete

---

### Subtask 35.5 — Propagate `kitty_keyboard_flags` through snapshot pipeline

**Files:**

- `freminal-terminal-emulator/src/interface.rs` — `SnapshotModeFields`, `collect_mode_fields()`
- `freminal-terminal-emulator/src/snapshot.rs` — `TerminalSnapshot`, `TerminalSnapshot::empty()`

1. Add to `SnapshotModeFields`:

   ```rust
   kitty_keyboard_flags: u32,
   ```

2. Add to `collect_mode_fields()`:

   ```rust
   kitty_keyboard_flags: self.internal.handler.kitty_keyboard_flags(),
   ```

3. Add to `TerminalSnapshot`:

   ```rust
   /// Currently active Kitty keyboard protocol flags (stack top, 0 if empty).
   ///
   /// When non-zero, the GUI must encode key events using KKP format instead
   /// of the legacy xterm encoding.  The specific flags determine which
   /// extensions are active (disambiguation, event types, etc.).
   pub kitty_keyboard_flags: u32,
   ```

4. Add to `TerminalSnapshot::empty()`:

   ```rust
   kitty_keyboard_flags: 0,
   ```

5. Add to the `TerminalSnapshot { .. }` constructor in `build_snapshot()`:

   ```rust
   kitty_keyboard_flags: mode_fields.kitty_keyboard_flags,
   ```

**Unit test to add** in `snapshot.rs` `#[cfg(test)]` block:

- `empty_kitty_keyboard_flags_is_zero()` — `TerminalSnapshot::empty().kitty_keyboard_flags == 0`

**Verification:** `cargo test --all` passes. `cargo clippy --all-targets --all-features -- -D warnings` clean.

- [ ] Subtask 35.5 complete

---

### Subtask 35.6 — Add KKP encoding to `TerminalInput::to_payload()`

**File:** `freminal-terminal-emulator/src/input.rs`

Add `kitty_keyboard_flags: u32` as the last parameter to `to_payload()`. The signature becomes:

```rust
pub fn to_payload(
    &self,
    decckm_mode: Decckm,
    keypad_mode: KeypadMode,
    modify_other_keys: u8,
    application_escape_key: ApplicationEscapeKey,
    backarrow_sends_bs: Decbkm,
    line_feed_mode: Lnm,
    kitty_keyboard_flags: u32,
) -> TerminalInputPayload
```

When `kitty_keyboard_flags != 0`, the following flags trigger changed behavior:

**Flag 1 — `DISAMBIGUATE_ESCAPE_CODES`:** Keys that normally produce C0 control bytes are sent as
explicit CSI u sequences instead:

- `Ctrl+letter` (currently `\x01`–`\x1a`): send `CSI keycode ; 5 u` where `keycode` is the
  ASCII code of the lowercase letter (`97`–`122`), modifier = `5` (Ctrl = 1+4).
  Example: `Ctrl+C` → `\x1b[99;5u` (note: **lowercase** codepoint per spec)
- `Ctrl+A` → `\x1b[97;5u` — this is the key fix for the tmux prefix key bug
- `Escape` (currently bare `\x1b`): send `CSI 27 u`
- `Alt+key`: send `CSI keycode ; 3 u` (Alt = 1+2) instead of `ESC + key`
- `Ctrl+Alt+key`: send `CSI keycode ; 7 u` (Ctrl+Alt = 1+4+2)

**Important exceptions (flag 1 only):** Enter, Tab, and Backspace **still send legacy bytes**
(`0x0d`, `0x09`, `0x7f`/`0x08`) when only flag 1 is active. This is required by the spec so
that `reset` can be typed at a shell prompt if a program crashes without clearing KKP mode.

**Flag 8 — `REPORT_ALL_KEYS_AS_ESCAPE_CODES`:** Every key press is sent as a CSI u or legacy
functional escape code. This includes:

- Plain printable ASCII: `a` → `CSI 97 u`, `A` (with shift) → `CSI 97 ; 2 u`
- Enter → `CSI 13 u`, Tab → `CSI 9 u`, Backspace → `CSI 127 u` (these now also use CSI u)
- Modifier key presses are also reported

**Flag 2 — `REPORT_EVENT_TYPES`:** Adds event-type sub-field for repeat/release. For the initial
implementation, all events are press (event-type = 1, omitted). A follow-up plan can add
press/repeat/release from egui.

**Functional keys (Arrow, Home, End, F1–F12, Insert, Delete, PageUp, PageDown)** retain their
legacy encoding format regardless of KKP flags. When KKP is active and modifiers are present:

- Arrow keys: `CSI 1 ; mods D` (Left), `CSI 1 ; mods C` (Right), etc.
- F-keys: `CSI number ; mods ~` (e.g., F5 = `CSI 15 ; mods ~`)
- Home/End: `CSI 1 ; mods H` / `CSI 1 ; mods F`

This is **already the encoding Freminal uses** for modified functional keys. The only change
needed for functional keys under KKP is to ensure the modifier parameter uses KKP encoding
(base 1) rather than the xterm modifier encoding, which is the same formula.

For the initial implementation, support flags 1 and 8. Flags 2, 4, and 16 are recognized but
produce the same output as flag 1 + 8 respectively (they require additional key event metadata not
currently available from egui). Add a code comment noting this.

**KKP modifier bitmask** differs from xterm's: base is `1`, encoding is
`1 + (shift?1:0) + (alt?2:0) + (ctrl?4:0)`. This is the same base formula as `KeyModifiers::modifier_param()` but with base 1 instead of returning `None` for no modifier. Create a
helper:

```rust
/// Compute the KKP modifier parameter.
///
/// KKP base is 1: no modifier = 1, Shift = 2, Alt = 3, Ctrl = 5, etc.
fn kkp_modifier(mods: KeyModifiers) -> u8 {
    1 + (if mods.shift { 1 } else { 0 })
      + (if mods.alt   { 2 } else { 0 })
      + (if mods.ctrl  { 4 } else { 0 })
}
```

**Unit tests to add** in `freminal-terminal-emulator/tests/terminal_input_payload.rs`:

- `kkp_flag0_no_change_to_ctrl_c()` — flags=0, `Ctrl+C` still → `\x03`
- `kkp_disambiguate_ctrl_c()` — flags=1, `Ctrl+C` → `\x1b[99;5u` (lowercase c = 99)
- `kkp_disambiguate_ctrl_a()` — flags=1, `Ctrl+A` → `\x1b[97;5u` (lowercase a = 97)
- `kkp_disambiguate_escape()` — flags=1, `Escape` → `\x1b[27u`
- `kkp_disambiguate_enter_still_legacy()` — flags=1, `Enter` → `\r` (NOT CSI u)
- `kkp_disambiguate_tab_still_legacy()` — flags=1, `Tab` → `\x09` (NOT CSI u)
- `kkp_disambiguate_backspace_still_legacy()` — flags=1, `Backspace` → `\x7f` (NOT CSI u)
- `kkp_all_keys_enter()` — flags=8, `Enter` → `\x1b[13u`
- `kkp_all_keys_tab()` — flags=8, `Tab` → `\x1b[9u`
- `kkp_all_keys_backspace()` — flags=8, `Backspace` → `\x1b[127u`
- `kkp_all_keys_ascii_a()` — flags=8, `Ascii(b'a')` → `\x1b[97u`
- `kkp_all_keys_ascii_shift_a()` — flags=8, `Ascii(b'A')` with shift mod → `\x1b[97;2u`
- `kkp_arrow_left_no_mods()` — flags=1, `ArrowLeft(NONE)` → legacy `\x1b[D` (unchanged)
- `kkp_arrow_left_ctrl()` — flags=1, `ArrowLeft(ctrl)` → `\x1b[1;5D`
- `kkp_home_no_mods()` — flags=1, `Home(NONE)` → legacy `\x1b[H` (unchanged)
- `kkp_f1_no_mods()` — flags=1, `FunctionKey(1, NONE)` → legacy `\x1bOP` (unchanged)
- `kkp_f5_shift()` — flags=1, `FunctionKey(5, shift)` → `\x1b[15;2~`

**Verification:** `cargo test --all` passes. `cargo clippy --all-targets --all-features -- -D warnings` clean.

- [ ] Subtask 35.6 complete

---

### Subtask 35.7 — Wire `kitty_keyboard_flags` into `InputModes` and `send_terminal_inputs()`

**File:** `freminal/src/gui/terminal.rs`

1. Add to `InputModes`:

   ```rust
   kitty_keyboard_flags: u32,
   ```

2. Add to `InputModes::from_snapshot()`:

   ```rust
   kitty_keyboard_flags: snap.kitty_keyboard_flags,
   ```

3. Add to the `input.to_payload(...)` call in `send_terminal_inputs()`:

   ```rust
   modes.kitty_keyboard_flags,
   ```

   (as the new last argument)

All other call sites of `to_payload()` in the codebase must also be updated to pass the new
argument. Find them with: `grep -rn 'to_payload(' freminal-terminal-emulator/tests/` — the test
file `terminal_input_payload.rs` calls `to_payload()` directly and must pass `0` for all existing
non-KKP tests.

**Verification:** `cargo test --all` passes (including all existing `terminal_input_payload.rs`
tests, which should all still pass since they will now pass `kitty_keyboard_flags: 0`).
`cargo clippy --all-targets --all-features -- -D warnings` clean. `cargo-machete` clean.

- [ ] Subtask 35.7 complete

---

### Subtask 35.8 — Update `TerminalSnapshot::empty()` and `#[allow]` budget

**File:** `freminal-terminal-emulator/src/snapshot.rs`

The `#[allow(clippy::struct_excessive_bools)]` attribute on `TerminalSnapshot` has a justification
comment listing seven boolean fields. After adding `kitty_keyboard_flags: u32`, verify the comment
is still accurate. No new `bool` is added in this plan so the count does not change.

Also verify `TerminalSnapshot::empty()` already sets `kitty_keyboard_flags: 0` (added in 35.5)
and the `empty_kitty_keyboard_flags_is_zero` test is present.

This subtask is a checkpoint only — no code changes expected if 35.5 was done correctly.

**Verification:** `cargo test --all` passes. `cargo clippy --all-targets --all-features -- -D warnings` clean.

- [ ] Subtask 35.8 complete

---

### Subtask 35.9 — Integration test: push/pop/query round-trip

**File:** `freminal-terminal-emulator/tests/` (new file: `kitty_keyboard_integration.rs`)

Write end-to-end tests that feed raw PTY bytes through a `TerminalState` and assert on the bytes
the PTY receives back, following the pattern in
`freminal-buffer/tests/terminal_handler_integration.rs`.

Tests to write:

- `query_without_push_responds_0u()` — feed `CSI ? u`, assert PTY response = `\x1b[?0u`
- `push_then_query_responds_with_flags()` — feed `CSI > 3 u` then `CSI ? u`, assert `\x1b[?3u`
- `push_push_pop_query()` — push 3, push 5, pop 1, query → `\x1b[?3u`
- `pop_empty_stack_does_not_panic()` — pop on empty stack, no panic, query → `\x1b[?0u`
- `set_replace_on_empty_stack_creates_entry()` — `CSI = 7 u`, query → `\x1b[?7u`
- `set_or_on_existing_flags()` — push 1, `CSI = 4 ; 2 u`, query → `\x1b[?5u`
- `set_and_not_clears_bits()` — push 7, `CSI = 2 ; 3 u`, query → `\x1b[?5u`
- `push_exceeds_max_depth_evicts_oldest()` — push 257 times, stack len stays 256

**Verification:** `cargo test --all` passes. `cargo clippy --all-targets --all-features -- -D warnings` clean.

- [ ] Subtask 35.9 complete

---

### Subtask 35.10 — Update `MASTER_PLAN.md` and `PLAN_35`

**Files:** `Documents/MASTER_PLAN.md`, `Documents/PLAN_35_KITTY_KEYBOARD_PROTOCOL.md`

1. Add Task 35 row to the Task Summary table in `MASTER_PLAN.md`:

   ```text
   | 35  | Kitty Keyboard Protocol | `PLAN_35_KITTY_KEYBOARD_PROTOCOL.md` | Complete | None |
   ```

2. Add Task 35 to the Completion Tracking table with start/end dates.

3. Update `PLAN_35_KITTY_KEYBOARD_PROTOCOL.md` status to `Complete`.

**Verification:** `cargo test --all` passes. No code changes in this subtask.

- [ ] Subtask 35.10 complete

---

## Verification Suite

Run after all subtasks complete:

```bash
cargo test --all
cargo clippy --all-targets --all-features -- -D warnings
cargo-machete
```

All three must pass with zero errors, zero warnings, and zero unused dependencies.

---

## Known Limitations and Non-Goals for This Plan

1. **Key repeat and key release events (flag 2):** egui's `Event::Key` exposes both
   `pressed: bool` (false = release) and `repeat: bool` (true = repeat), so flag 2 is
   implementable. This plan implements flag 2: key repeat events include event-type `:2`
   and key release events include event-type `:3` when flag 2 is active.

2. **Alternate key codes (flag 4):** Report shifted/alt variants in an additional field
   (`CSI code ; mods ; shifted_code u`). Requires knowledge of the shifted variant of each
   key. Not implemented in this plan; treated as flags=1+8. A follow-up can add shifted
   code reporting.

3. **Associated text (flag 16):** egui provides `Event::Text(String)` which gives the text
   a key produces, and `Modifiers` are available. This plan implements flag 16: when active,
   the associated text from `Event::Text` is appended to the CSI u sequence as
   `CSI code ; mods ; event-type ; text u` where `text` is the Unicode codepoints of the
   associated text. However, full implementation requires careful coordination between
   `Event::Key` and `Event::Text` events — the initial implementation includes the
   framework but associated text is only reported for simple cases where the text can be
   inferred from the key event itself.

4. **Super/Hyper/Meta modifiers:** egui's `Modifiers` exposes `command` (which maps to Super
   on Linux/Windows) and `mac_cmd` (Mac ⌘, always false on non-Mac). The `command` field can
   map to the Super bit (+8) in the KKP modifier bitmask on Linux. This plan implements Super
   via `command` on non-Mac platforms. Hyper (+16) and Meta (+32) are not available from egui
   and are not implemented.

5. **Numpad keys as distinct from regular keys:** KKP allows distinguishing `KP_Enter` from
   `Enter`. The `TerminalInput::KeyPad(u8)` variant is available but the mapping to KKP key
   codes for numeric keypad is not implemented in this plan.

---

## References

- Kitty Keyboard Protocol specification: <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>
- xterm modifyOtherKeys: <https://invisible-island.net/xterm/ctlseqs/ctlseqs.html>
- Neovim KKP support: <https://neovim.io/doc/user/term.html#tui-kitty-keyboard>
- ECMA-48: <https://www.ecma-international.org/publications-and-standards/standards/ecma-48/>
