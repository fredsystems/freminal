# PLAN_25 — Code Quality: Parser Split, CSI Naming, Crate Organization

## Status: Complete

---

## Overview

A structural audit of the codebase identified issues that do not affect correctness but impede
maintainability, readability, and onboarding. This task addresses five categories:

1. **Parser split:** `standard.rs` conflates ESC sequence parsing, DCS accumulation, and APC
   accumulation into a single file with forking `bool` flags. Split into focused modules.

2. **CSI command naming:** CSI command files use inconsistent naming patterns for their public
   functions. File names have been partially standardized to ECMA-48 mnemonics, but function
   names inside them still use legacy patterns (terminator-byte suffixes, English descriptions,
   or missing `finished_` prefix). Standardize all function names on the mnemonic convention.

3. **Crate organization:** `portable-pty` is pulled into `freminal-common` for a 12-line
   conversion impl, violating the "no platform-specific dependencies" rule. `osc.rs` handles
   15+ protocols inline. Misplaced types and dead code.

4. **Dead code cleanup:** `StandardOutput` enum (never used), `Theme` enum on `TerminalState`
   (superseded by `ThemePalette` in `freminal-common`), dead `scroll()` method, vestigial
   6-line re-export module.

5. **Input encoding extraction:** `interface.rs` (953 lines) conflates keyboard encoding logic
   with the emulator's public API. The input encoding subsystem should be its own module.

**Dependencies:** None (independent, pure refactoring)
**Dependents:** None
**Primary crates:** `freminal-terminal-emulator`, `freminal-common`, `freminal-buffer`
**Estimated scope:** Medium (10 subtasks)

---

## Current State

### Parser Structure — `standard.rs`

`freminal-terminal-emulator/src/ansi_components/standard.rs` is 518 lines and conflates three
responsibilities:

1. **Short ESC sequences** (ESC followed by a single character) — the core purpose.
2. **DCS accumulation** — gated by `dcs: bool` flag on `StandardParser` (line 47);
   `StandardParserState::Params` has dual meaning depending on this flag (lines 183–206).
3. **APC accumulation** — gated by `apc: bool` flag (line 48); same dual-meaning problem.

The `StandardOutput` enum (lines 22–39, 16 variants) is **never constructed or matched
anywhere** — it is dead code.

`contains_string_terminator()` (lines 103–128, with doc comment from line 85) exists only for
DCS/APC string accumulation and includes tmux ESC-doubling logic.

The existing parser architecture already has the correct pattern: `csi.rs` handles CSI via
`AnsiCsiParser`, `osc.rs` handles OSC via `AnsiOscParser`, and `ParserInner` (in `ansi.rs`,
lines 92–103) has variants for each. `ParserInner` currently has 7 variants: `Empty`, `Escape`,
`Csi(AnsiCsiParser)`, `Osc(AnsiOscParser)`, `Standard(StandardParser)`, `Vt52Escape`, and
`Vt52CursorAddress(Option<u8>)`. DCS and APC should follow the Csi/Osc pattern.

### CSI Command Naming

34 CSI command files exist in `csi_commands/`. File names have been partially standardized to
ECMA-48 mnemonics (e.g., `ed.rs`, `el.rs`, `cup.rs`), but the public function names inside
them still use 4 different legacy patterns:

| Pattern                   | Example                                                      | Count | Issue                           |
| ------------------------- | ------------------------------------------------------------ | ----- | ------------------------------- |
| A: Mnemonic suffix        | `_cbt`, `_dl`, `_vpa`                                        | 12    | Correct — target convention     |
| B: Terminator-byte suffix | `_set_position_j` (ED), `_set_position_h` (CUP)              | 9     | Legacy — byte letter, not name  |
| C: English description    | `_move_up` (CUU), `_move_cursor_left` (CUB)                  | 6     | Inconsistent with pattern A     |
| D: Missing `finished_`    | `_set_top_and_bottom_margins`, `_set_left_and_right_margins` | 2     | Missing the `finished_` segment |

Additionally:

- **File name error:** `ict.rs` contains function `_ich` (ICH = Insert Character). The file
  should be `ich.rs`.
- **File name error:** `report_xt_version.rs` should be `xtversion.rs` to match the mnemonic.
- **File name error:** `send_device_attributes.rs` should be `da.rs` (handles DA1 + DA2).

**3 file renames + 20 function renames** are needed to standardize on pattern A.

### Crate Organization

| Issue                          | Location                                           | Problem                                                                          |
| ------------------------------ | -------------------------------------------------- | -------------------------------------------------------------------------------- |
| `portable-pty` in common crate | `freminal-common/Cargo.toml`                       | 12-line `TryFrom` impl forces platform dep on all downstream crates              |
| `interface.rs` is 953 lines    | `freminal-terminal-emulator/src/`                  | Input encoding (~350 lines) mixed with emulator API (~460) and snapshot (~140)   |
| `osc.rs` is 1,293 lines        | `freminal-terminal-emulator/src/ansi_components/`  | 15+ OSC protocols handled inline; iTerm2 alone is ~227 lines                     |
| `state/data.rs` is 6 lines     | `freminal-terminal-emulator/src/state/`            | Single re-export line                                                            |
| Dead `Theme` enum              | `freminal-terminal-emulator/src/state/internal.rs` | Superseded by `ThemePalette` in `freminal-common/src/themes.rs`; no live callers |
| Dead `scroll()` method         | `freminal-terminal-emulator/src/state/internal.rs` | Discards return values; GUI owns scroll offset via `ViewState`                   |

### Dead Code

| Item                  | Location                  | Why Dead                                                            |
| --------------------- | ------------------------- | ------------------------------------------------------------------- |
| `StandardOutput` enum | `standard.rs:22-39`       | Never constructed or matched                                        |
| `Theme` enum          | `state/internal.rs:72-82` | Superseded by `ThemePalette`; `set_theme()` has no external callers |
| `scroll()` method     | `state/internal.rs`       | Return values discarded; GUI owns scroll offset via ViewState       |
| `state/data.rs`       | `state/data.rs`           | 6-line file with single re-export                                   |

### Identified but Out of Scope

The audit identified two extreme god files in `freminal-buffer` that need their own dedicated
plan:

| File                  | Lines | Subsystems | Recommendation             |
| --------------------- | ----- | ---------- | -------------------------- |
| `terminal_handler.rs` | 9,098 | ~10        | Needs dedicated split plan |
| `buffer.rs`           | 6,624 | ~8         | Needs dedicated split plan |

Together these two files account for ~71% of all code in the three library crates. Splitting
them is the single most impactful structural improvement possible, but the scope exceeds what
can be safely done alongside the other subtasks here. A separate `PLAN_26_BUFFER_SPLIT.md` is
recommended.

The mode dispatch split (buffer-affecting modes handled in `terminal_handler.rs`, GUI-concern
modes handled in `state/internal.rs::sync_mode()` via implicit double-dispatch) is also worth
documenting and potentially consolidating, but is closely tied to the `terminal_handler.rs`
split and should be addressed there.

---

## Subtasks

---

### 25.1 — Split `standard.rs` into ESC, DCS, and APC Parsers

- **Status:** Complete (2026-04-01)
- **Priority:** 1 — High
- **Scope:** `freminal-terminal-emulator/src/ansi_components/standard.rs` (modify),
  `freminal-terminal-emulator/src/ansi_components/dcs.rs` (new),
  `freminal-terminal-emulator/src/ansi_components/apc.rs` (new),
  `freminal-terminal-emulator/src/ansi.rs` (modify)
- **Details:**
  1. Create `dcs.rs` with a `DcsParser` struct that handles DCS string accumulation. Move
     the `dcs`-related logic from `StandardParser` into `DcsParser`:
     - State: accumulating params, collecting data bytes, waiting for ST.
     - `contains_string_terminator()` moves here (it is only needed for DCS/APC).
     - tmux ESC-doubling logic stays with the ST detection.

  2. Create `apc.rs` with an `ApcParser` struct for APC string accumulation. Same pattern
     as DCS but simpler (APC is opaque bytes until ST).

  3. Add `ParserInner::Dcs(DcsParser)` and `ParserInner::Apc(ApcParser)` variants to the
     parser state machine in `ansi.rs` (currently at lines 92–103), matching the existing
     `ParserInner::Csi(AnsiCsiParser)` and `ParserInner::Osc(AnsiOscParser)` pattern. This
     will bring `ParserInner` to 9 variants (including the two VT52 variants from Task 20).

  4. Remove `dcs: bool` and `apc: bool` flags from `StandardParser`. After the split,
     `StandardParser` handles only short ESC sequences (~280 lines).

  5. Delete the `StandardOutput` enum (dead code — 16 variants, lines 22–39, never used).

  6. Update `ansi_components/mod.rs` to export the new modules.

- **Acceptance criteria:**
  - `standard.rs` handles only ESC sequences (no DCS/APC logic).
  - DCS strings are parsed correctly via `DcsParser`.
  - APC strings are captured correctly via `ApcParser`.
  - `StandardOutput` enum is deleted.
  - All existing tests pass — parsing behavior is identical.
  - `standard.rs` is ~280 lines or less.
- **Tests required:**
  - DCS string accumulation: `ESC P ... ESC \` correctly captured.
  - APC string accumulation: `ESC _ ... ESC \` correctly captured.
  - DCS with tmux ESC-doubling: `ESC ESC` inside DCS does not terminate.
  - Short ESC sequences still work: `ESC H`, `ESC M`, `ESC 7`, `ESC 8`, etc.
  - DECRQSS and XTGETTCAP responses still work end-to-end.

---

### 25.2 — Standardize CSI Command File and Function Names

- **Status:** Complete (2026-04-01)
- **Priority:** 2 — Medium
- **Scope:** `freminal-terminal-emulator/src/ansi_components/csi_commands/` (20 files),
  `freminal-terminal-emulator/src/ansi_components/csi.rs` (dispatch table + imports)
- **Details:**
  Rename functions (and 3 files) to use ECMA-48 mnemonic naming consistently. All public
  functions should follow the pattern `ansi_parser_inner_csi_finished_<mnemonic>`.

  **File renames (3):**

  | Current File                | New File       | Notes                                   |
  | --------------------------- | -------------- | --------------------------------------- |
  | `ict.rs`                    | `ich.rs`       | Function name `_ich` already correct    |
  | `report_xt_version.rs`      | `xtversion.rs` | Function also needs rename              |
  | `send_device_attributes.rs` | `da.rs`        | Handles DA1 (`CSI c`) + DA2 (`CSI > c`) |

  **Function renames (20):**

  | File                   | Current Function                  | New Function                                       | Mnemonic  |
  | ---------------------- | --------------------------------- | -------------------------------------------------- | --------- |
  | `cha.rs`               | `_finished_set_cursor_position_g` | `_finished_cha`                                    | CHA       |
  | `cub.rs`               | `_finished_move_cursor_left`      | `_finished_cub`                                    | CUB       |
  | `cud.rs`               | `_finished_move_down`             | `_finished_cud`                                    | CUD       |
  | `cuf.rs`               | `_finished_move_right`            | `_finished_cuf`                                    | CUF       |
  | `cup.rs`               | `_finished_set_position_h`        | `_finished_cup`                                    | CUP       |
  | `cuu.rs`               | `_finished_move_up`               | `_finished_cuu`                                    | CUU       |
  | `dch.rs`               | `_finished_set_position_p`        | `_finished_dch`                                    | DCH       |
  | `decscusr.rs`          | `_finished_set_position_q`        | `_finished_decscusr`                               | DECSCUSR  |
  | `decslpp.rs`           | `_finished_set_position_t`        | `_finished_decslpp`                                | DECSLPP   |
  | `decslrm.rs`           | `_csi_set_left_and_right_margins` | `_csi_finished_decslrm`                            | DECSLRM   |
  | `decstbm.rs`           | `_csi_set_top_and_bottom_margins` | `_csi_finished_decstbm`                            | DECSTBM   |
  | `ech.rs`               | `_finished_set_position_x`        | `_finished_ech`                                    | ECH       |
  | `ed.rs`                | `_finished_set_position_j`        | `_finished_ed`                                     | ED        |
  | `el.rs`                | `_finished_set_position_k`        | `_finished_el`                                     | EL        |
  | `il.rs`                | `_finished_set_position_l`        | `_finished_il`                                     | IL        |
  | `scorc.rs`             | `_finished_u`                     | `_finished_scorc`                                  | SCORC     |
  | `sgr.rs`               | `_finished_sgr_ansi`              | `_finished_sgr`                                    | SGR       |
  | `da.rs` (new)          | `_finished_send_da`               | `_finished_da`                                     | DA        |
  | `xtversion.rs` (new)   | `_finished_report_version_q`      | `_finished_xtversion`                              | XTVERSION |
  | `modify_other_keys.rs` | `parse_modify_other_keys`         | `ansi_parser_inner_csi_finished_modify_other_keys` | —         |

  All function names in the table above are shown with the `ansi_parser_inner` prefix omitted
  for readability. The full name is always `ansi_parser_inner_csi_finished_<mnemonic>`.

  **Note on `modify_other_keys.rs`:** This function is a sub-dispatch called from `sgr.rs`,
  not from the top-level dispatch table in `csi.rs`. The prefix change aligns it with the
  convention; the file name stays as-is since "modify other keys" is the standard name for
  this xterm feature (it has no ECMA-48 mnemonic).

  **Note on combined dispatch arms in `csi.rs`:** The dispatch table routes `b'H' | b'f'` to
  `cup.rs` (CUP handles both CUP and HVP — they are equivalent per ECMA-48). Similarly,
  `b'G' | b'\`'`routes to`cha.rs`(CHA and HPA are equivalent). These are correct; no
separate`hvp.rs`or`hpa.rs` files are needed.

  **Files already using the correct pattern (no change needed, 12 files):** `cbt.rs`, `cht.rs`,
  `cnl.rs`, `cpl.rs`, `decrqm.rs`, `dl.rs`, `dsr.rs`, `rep.rs`, `sd.rs`, `su.rs`, `tbc.rs`,
  `vpa.rs`.

  Update the dispatch table imports in `csi.rs` (lines 54–78) and all call sites in the
  dispatch match (lines ~100–450) to reference the new names.

- **Acceptance criteria:**
  - All CSI command files follow mnemonic naming (file name = mnemonic).
  - All public functions follow `ansi_parser_inner_csi_finished_<mnemonic>` pattern.
  - Dispatch table in `csi.rs` updated with new imports and call sites.
  - All existing tests pass.
  - `cargo clippy` clean.
- **Tests required:**
  - Existing CSI command tests continue to pass (renaming should not change behavior).
  - `cargo test --all` to verify.

---

### 25.3 — Split Input Encoding Out of `interface.rs`

- **Status:** Complete
- **Priority:** 2 — Medium
- **Scope:** `freminal-terminal-emulator/src/interface.rs` (modify),
  `freminal-terminal-emulator/src/input.rs` (new)
- **Details:**
  `interface.rs` is 953 lines with three concerns. The input encoding logic (~350 lines,
  including `TerminalInput` enum, `to_payload()`, key-to-bytes conversion) should be
  extracted to `input.rs`.

  Move:
  - `TerminalInput` enum and all its variants (lines 154–180).
  - `TerminalInputPayload` enum.
  - `KeyModifiers` struct and its `impl`.
  - `to_payload()` method and all key encoding helpers (lines 199–376).
  - `char_to_ctrl_code()` (lines 63–67).
  - `modified_csi_final()` and `modified_csi_tilde()`.
  - `collect_text()` and `raw_ascii_bytes_to_terminal_input()`.
  - Any associated tests.

  Keep in `interface.rs`:
  - `TerminalEmulator` struct and its methods (emulator API).
  - `build_snapshot()` (snapshot building).
  - `split_format_data_for_scrollback()`.

  After the split, `interface.rs` should be ~600 lines and `input.rs` ~350 lines.

- **Acceptance criteria:**
  - `input.rs` contains all input encoding logic.
  - `interface.rs` contains only emulator API and snapshot building.
  - All existing tests pass.
  - Public API unchanged (re-export from `lib.rs` if needed).
- **Tests required:**
  - All existing input encoding tests pass from their new location.
  - `cargo test --all` passes.

---

### 25.4 — Inline `state/data.rs`

- **Status:** Complete
- **Priority:** 3 — Low
- **Scope:** `freminal-terminal-emulator/src/state/data.rs` (delete),
  `freminal-terminal-emulator/src/state/mod.rs` (modify),
  `freminal-terminal-emulator/src/state/internal.rs` (modify)
- **Details:**
  `state/data.rs` is 6 lines containing a single re-export:
  `pub use freminal_common::buffer_states::terminal_sections::TerminalSections;`

  Inline the re-export into `state/mod.rs` and delete `data.rs`. Update the two import sites:
  - `interface.rs:29`: `use crate::state::{data::TerminalSections, internal::TerminalState}`
    → `use crate::state::{TerminalSections, internal::TerminalState}`
  - `state/internal.rs:46`: `use super::data::TerminalSections`
    → `use super::TerminalSections`

- **Acceptance criteria:**
  - `state/data.rs` deleted.
  - Re-export moved to `state/mod.rs`.
  - All existing code compiles and tests pass.
- **Tests required:**
  - `cargo test --all` passes.

---

### 25.5 — Remove Dead `Theme` Enum from `TerminalState`

- **Status:** Complete
- **Priority:** 2 — Medium
- **Scope:** `freminal-terminal-emulator/src/state/internal.rs`
- **Details:**
  The `Theme` enum (lines 72–76) and `From<bool> for Theme` impl (lines 78–82) are dead code.
  The real theme system uses `ThemePalette` from `freminal-common/src/themes.rs` with 25
  curated palettes. All live code paths call `terminal.internal.handler.set_theme(theme)`
  with a `&'static ThemePalette` directly (see `main.rs` lines 255, 305, 412).

  `TerminalState::set_theme()` (lines 162–164) has no external callers. The `theme: Theme`
  field on `TerminalState` (line 92) is set during construction but never read by any live
  code path.

  Delete:
  - `Theme` enum and `From<bool>` impl.
  - `theme: Theme` field on `TerminalState`.
  - `set_theme()` method.
  - Any constructor code that initialises the `theme` field.

  **This is a dead code removal, not a move.** The original plan proposed moving `Theme` to
  `freminal-common`, but `freminal-common` already has the real theme system (`ThemePalette`),
  making `Theme` entirely redundant.

- **Acceptance criteria:**
  - `Theme` enum, field, and method all deleted.
  - No compilation errors (confirming no live callers).
  - No `#[allow(dead_code)]` needed.
- **Tests required:**
  - `cargo test --all` passes.
  - `cargo clippy` clean.

---

### 25.6 — Remove Dead `scroll()` Method and Update Tests

- **Status:** Complete
- **Priority:** 2 — Medium
- **Scope:** `freminal-terminal-emulator/src/state/internal.rs`,
  `freminal-terminal-emulator/tests/terminal_state_tests.rs`,
  `freminal-terminal-emulator/tests/shadow_handler.rs`
- **Details:**
  `TerminalState::scroll()` (lines 609–639) discards the return values from
  `handle_scroll_back()` and `handle_scroll_forward()` with `let _new_offset = ...`.
  Since the performance refactor (PERFORMANCE_PLAN.md Task 4), scroll offset is owned by the
  GUI's `ViewState`, not the emulator. The `scroll()` method is dead in production code.

  However, `scroll()` has **6 live test callers** that must be addressed before removal:
  - `terminal_state_tests.rs` lines 187, 214
  - `shadow_handler.rs` lines 134, 137, 140, 143

  These test call sites must be rewritten to exercise scrolling via the handler methods
  directly (`handle_scroll_back()`, `handle_scroll_forward()`) and track the returned offset,
  or removed if the tests are redundant with existing coverage.

  Steps:
  1. Audit the 6 test call sites to determine what behavior they are actually testing.
  2. Rewrite the tests to use `handle_scroll_back()` / `handle_scroll_forward()` directly.
  3. Remove the `scroll()` method.

- **Acceptance criteria:**
  - `scroll()` method removed from `TerminalState`.
  - All 6 test call sites rewritten or removed.
  - No compilation errors.
  - No `#[allow(dead_code)]` needed.
- **Tests required:**
  - `cargo test --all` passes.
  - `cargo clippy` clean (no dead code warnings).

---

### 25.7 — Delete Dead `StandardOutput` Enum

- **Status:** Complete
- **Details:**
  The `StandardOutput` enum (lines 22–39, 16 variants) is never constructed or pattern-matched
  anywhere in the codebase. The only match in the entire workspace is its own definition. It
  appears to be a remnant of an earlier parser design. Delete it along with its `derive`
  attribute at line 21.

  **Note:** If subtask 25.1 is done first, this deletion happens as part of that subtask.
  If 25.7 is done first, it is a standalone deletion. Either order is fine.

- **Acceptance criteria:**
  - `StandardOutput` enum deleted.
  - No compilation errors.
- **Tests required:**
  - `cargo test --all` passes.

---

### 25.8 — Add Doc Comments to CSI Command Public Functions

- **Status:** Complete
- **Priority:** 3 — Low
- **Scope:** `freminal-terminal-emulator/src/ansi_components/csi_commands/` (multiple files)
- **Details:**
  All 34 CSI command files already have `///` doc comments on their public functions (verified
  by audit). However, several doc comments are inaccurate or incomplete:
  1. **`decrqm.rs`:** Doc title says "DEC Private Mode Set" — this is wrong. DECRQM is
     "DEC Request Mode" (a query/report mechanism). The function handles Set (`h`), Reset
     (`l`), **and** Query (`$ h`) modes. Fix the doc to describe all three.

  2. **After 25.2 renames:** Any function whose name changed should have its doc comment
     updated to include the correct ECMA-48 mnemonic. Example format:

     ```rust
     /// ED — Erase in Display (`CSI Ps J`). Erases part of the display based on Ps.
     pub fn ansi_parser_inner_csi_finished_ed(params: &[u8]) -> TerminalOutput {
     ```

  3. **`decstbm.rs`:** The public function currently only has `/// # Errors` — add a
     proper mnemonic + description line.

  4. **`modify_other_keys.rs`:** Has a module-level `//!` doc and function `///` doc, but
     both should reference the xterm specification name and CSI sequence format.

  **Must be done after 25.2** so that doc comments use the final function names.

- **Acceptance criteria:**
  - All `ansi_parser_inner_csi_finished_*` functions have an accurate doc comment.
  - Doc comments include the mnemonic, sequence format, and brief description.
  - `decrqm.rs` doc title is corrected.
- **Tests required:** None (documentation only).

---

### 25.9 — Remove `portable-pty` from `freminal-common`

- **Status:** Complete
- **Details:**
  `freminal-common` depends on `portable-pty` solely for a 12-line `TryFrom` impl
  (`TryFrom<FreminalTerminalSize> for portable_pty::PtySize`) in `pty_write.rs` (lines 21–32).
  This violates the `agents.md` rule that `freminal-common` should have "no platform-specific
  dependencies beyond what is needed for type definitions." `portable-pty` brings in `libc`,
  `nix`, and `filedescriptor` — all of which `freminal-buffer` inherits transitively despite
  having zero interest in PTY operations.

  The only call site for `PtySize::try_from(FreminalTerminalSize)` is
  `freminal-terminal-emulator/src/io/pty.rs` (the `run_terminal` function, which already has
  its own direct `portable-pty` dependency).

  Steps:
  1. Move the `TryFrom<FreminalTerminalSize> for PtySize` impl from
     `freminal-common/src/pty_write.rs` to `freminal-terminal-emulator/src/io/pty.rs`.
  2. Remove `portable-pty` from `freminal-common/Cargo.toml`.
  3. Verify `freminal-buffer` no longer transitively depends on `portable-pty`.

  `FreminalTerminalSize` and `PtyWrite` stay in `freminal-common` — they are pure data types
  with no platform dependency. Only the conversion impl moves.

- **Acceptance criteria:**
  - `freminal-common/Cargo.toml` does not list `portable-pty`.
  - `FreminalTerminalSize` and `PtyWrite` remain in `freminal-common`.
  - The `TryFrom` impl compiles in its new location.
  - `cargo tree -p freminal-buffer` does not show `portable-pty`.
- **Tests required:**
  - `cargo test --all` passes.
  - `cargo clippy` clean.
  - `cargo-machete` clean.

---

### 25.10 — Split `osc.rs` iTerm2 and Clipboard Handlers into Submodules

- **Status:** Complete
- **Priority:** 2 — Medium
- **Scope:** `freminal-terminal-emulator/src/ansi_components/osc.rs` (modify),
  `freminal-terminal-emulator/src/ansi_components/osc_iterm2.rs` (new),
  `freminal-terminal-emulator/src/ansi_components/osc_clipboard.rs` (new),
  `freminal-terminal-emulator/src/ansi_components/osc_palette.rs` (new)
- **Details:**
  `osc.rs` is 1,293 lines and handles 15+ OSC protocols inline. The dispatch table
  (`dispatch_osc_target`, lines 194–289) is clean, but three protocol handlers have enough
  complexity to warrant extraction:
  1. **iTerm2 inline images** (OSC 1337): `handle_osc_iterm2` + 4 sub-handlers, ~227 lines
     (lines 447–673). This is the largest single protocol handler and deals with multipart
     file transfers, dimension parsing, aspect ratio, and `doNotMoveCursor`.
     → Extract to `osc_iterm2.rs`.

  2. **Clipboard** (OSC 52): `handle_osc_clipboard`, ~35 lines (lines 294–328). Small but
     self-contained protocol with base64 encoding/decoding.
     → Extract to `osc_clipboard.rs`.

  3. **Palette color** (OSC 4 + OSC 104): `handle_osc_palette_color` (~59 lines, 336–394)
     and `handle_osc_reset_palette` (~35 lines, 399–433), plus the `scale_hex_channel` and
     `parse_color_spec` helpers and their ~100 lines of tests.
     → Extract to `osc_palette.rs`.

  The remaining OSC protocols (title bar, foreground/background/cursor color queries, FTCS,
  URL, remote host) are 1–5 lines each and stay inline in `osc.rs`.

  After extraction, `osc.rs` should be ~500 lines (dispatch + simple handlers + remaining
  tests). The three new files should be ~200, ~50, and ~200 lines respectively.

- **Acceptance criteria:**
  - `osc.rs` handles only dispatch and simple (< 10 line) protocol handlers.
  - iTerm2, clipboard, and palette handlers are in dedicated files.
  - All existing OSC tests pass from their new locations.
  - `osc.rs` is under 600 lines.
- **Tests required:**
  - All existing OSC tests pass.
  - iTerm2 inline image tests pass (OSC 1337 basic, multipart, all args, error cases).
  - Palette color tests pass (OSC 4 set/query, OSC 104 reset).
  - `cargo test --all` passes.

---

## Implementation Notes

### Subtask Ordering

25.7 (dead StandardOutput) and 25.9 (portable-pty) are independent and can be done first.
25.1 (parser split) and 25.2 (CSI naming) are independent of each other.
25.3 (interface.rs split), 25.4 (data.rs inline), 25.5 (dead Theme), 25.6 (dead scroll)
are all independent.
25.8 (doc comments) must be done after 25.2 (names must be finalized first).
25.10 (osc.rs split) is independent.

**Recommended order:** 25.9 → 25.7 → 25.1 → 25.2 → 25.8 → 25.10 → (25.3, 25.4, 25.5, 25.6
in any order)

### Risk Assessment

All subtasks are pure refactoring — no behavior change. The primary risk is merge conflicts
if other tasks modify the same files concurrently. To minimize risk:

- Do the parser split (25.1) early, as it touches the most files.
- Do the CSI renaming (25.2) in a single commit to avoid half-renamed intermediate states.
- Do the portable-pty fix (25.9) early — it touches Cargo.toml files that other tasks may
  also modify.
- Each subtask should be a single atomic commit.

### IO Crate Split Assessment

A full `freminal-io` crate was considered for the channel protocol types (`PtyRead`,
`InputEvent`, `WindowCommand`, etc.) but is **not warranted at this time**. The IO module in
`freminal-terminal-emulator` is 503 lines across 2 files with only 2 consumers (the emulator
and the binary). The overhead of a new crate is not proportionate to the benefit. Subtask 25.9
addresses the concrete violation (`portable-pty` in `freminal-common`) without the new crate.

Revisit if: the channel protocol types grow significantly, a second binary appears, or the
`freminal-buffer` dependency on `PtyWrite` becomes problematic.

### Verification

Each subtask must pass before proceeding:

- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo-machete`

---

## References

- `freminal-terminal-emulator/src/ansi_components/standard.rs` — parser to split (518 lines)
- `freminal-terminal-emulator/src/ansi_components/csi_commands/` — 34 CSI command files
- `freminal-terminal-emulator/src/ansi_components/csi.rs` — CSI dispatch table (515 lines)
- `freminal-terminal-emulator/src/ansi_components/osc.rs` — OSC parser (1,293 lines)
- `freminal-terminal-emulator/src/interface.rs` — emulator API + input encoding (953 lines)
- `freminal-terminal-emulator/src/state/internal.rs` — dead `scroll()`, dead `Theme` (653 lines)
- `freminal-terminal-emulator/src/state/data.rs` — 6-line single re-export
- `freminal-terminal-emulator/src/ansi.rs` — top-level parser with `ParserInner` variants
- `freminal-terminal-emulator/src/io/pty.rs` — PTY spawn logic (397 lines)
- `freminal-common/src/pty_write.rs` — `PtyWrite` + `FreminalTerminalSize` + misplaced TryFrom
- `freminal-common/src/themes.rs` — real theme system (`ThemePalette`, 25 palettes)
