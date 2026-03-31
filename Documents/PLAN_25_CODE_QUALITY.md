# PLAN_25 — Code Quality: Parser Split, CSI Naming, Crate Organization

## Status: Pending

---

## Overview

A code quality audit identified several structural issues in the codebase that do not affect
correctness but impede maintainability, readability, and onboarding. This task addresses four
categories:

1. **Parser split:** `standard.rs` conflates ESC sequence parsing, DCS accumulation, and APC
   accumulation into a single file with forking `bool` flags. Split into focused modules.

2. **CSI command naming:** 34 CSI command files use 5 different naming patterns. Standardize
   on ECMA-48 mnemonics.

3. **Crate organization:** Misplaced types, oversized files, and dead code. Move types to
   correct homes, split large files, remove dead code.

4. **Dead code cleanup:** Specific dead code identified during audit.

**Dependencies:** None (independent, pure refactoring)
**Dependents:** None
**Primary crates:** `freminal-terminal-emulator`, `freminal-common`, `freminal-buffer`
**Estimated scope:** Medium (8 subtasks)

---

## Current State

### Parser Structure — `standard.rs`

`freminal-terminal-emulator/src/ansi_components/standard.rs` is 518 lines and conflates three
responsibilities:

1. **Short ESC sequences** (ESC followed by a single character) — the core purpose.
2. **DCS accumulation** — gated by `dcs: bool` flag; `StandardParserState::Params` has dual
   meaning depending on this flag.
3. **APC accumulation** — gated by `apc: bool` flag; same dual-meaning problem.

The `StandardOutput` enum (lines 21-39, 16 variants) is **never constructed or matched
anywhere** — it is dead code.

`contains_string_terminator()` (44 lines with tmux ESC-doubling logic) exists only for
DCS/APC string accumulation.

The existing parser architecture already has the correct pattern: `csi.rs` handles CSI,
`osc.rs` handles OSC, and `ParserInner` has variants for each. DCS and APC should follow
this pattern.

### CSI Command Naming

34 CSI command files in `csi_commands/` use 5 different naming patterns:

| Pattern                  | Example                         | Count | Standard     |
| ------------------------ | ------------------------------- | ----- | ------------ |
| A: Mnemonic-based        | `_cbt`, `_dl`, `_il`            | ~15   | Correct      |
| B: Terminator-byte-based | `_set_position_j` (= ED)        | ~9    | Legacy       |
| C: English description   | `_move_up`, `_move_cursor_left` | ~4    | Inconsistent |
| D: Missing prefix        | `_set_top_and_bottom_margins`   | 2     | Incorrect    |
| E: Different prefix      | `parse_modify_other_keys`       | 2     | Non-standard |

**File name error:** `ict.rs` contains function `_ich` (ICH = Insert Character). The file
should be `ich.rs`.

**17 renames needed** to standardize all files and functions on Pattern A (ECMA-48 mnemonics).

### Crate Organization

| Issue                       | Location                                           | Problem                                                                       |
| --------------------------- | -------------------------------------------------- | ----------------------------------------------------------------------------- |
| `interface.rs` is 947 lines | `freminal-terminal-emulator/src/`                  | 3 concerns: input encoding (~375 lines), emulator API (~400), snapshot (~200) |
| `osc.rs` is 1293 lines      | `freminal-terminal-emulator/src/ansi_components/`  | 10+ OSC protocols handled inline                                              |
| `state/data.rs` is 6 lines  | `freminal-terminal-emulator/src/state/`            | Single re-export line                                                         |
| `Theme` enum misplaced      | `freminal-terminal-emulator/src/state/internal.rs` | Should be in `freminal-common`                                                |
| `scroll()` is dead code     | `freminal-terminal-emulator/src/state/internal.rs` | Discards return values, bypassed by GUI                                       |

### Dead Code

| Item                   | Location            | Why Dead                                        |
| ---------------------- | ------------------- | ----------------------------------------------- |
| `StandardOutput` enum  | `standard.rs:21-39` | Never constructed or matched                    |
| `scroll()` method      | `state/internal.rs` | Return values discarded; GUI owns scroll offset |
| `state/data.rs` module | `state/data.rs`     | 6-line file with single re-export               |

---

## Subtasks

---

### 25.1 — Split `standard.rs` into ESC, DCS, and APC Parsers

- **Status:** Pending
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
     parser state machine in `ansi.rs`, matching the existing `ParserInner::Csi(CsiParser)`
     and `ParserInner::Osc(OscParser)` pattern.

  4. Remove `dcs: bool` and `apc: bool` flags from `StandardParser`. After the split,
     `StandardParser` handles only short ESC sequences (~280 lines).

  5. Delete the `StandardOutput` enum (dead code — 16 variants never used).

  6. Update `mod.rs` to export the new modules.

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

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-terminal-emulator/src/ansi_components/csi_commands/` (all 34 files),
  `freminal-terminal-emulator/src/ansi_components/csi.rs` (dispatch table)
- **Details:**
  Rename files and functions to use ECMA-48 mnemonic naming consistently. The complete
  rename table:

  | Current File                    | Current Function                              | New File                  | New Function                            | Mnemonic          |
  | ------------------------------- | --------------------------------------------- | ------------------------- | --------------------------------------- | ----------------- |
  | `set_position_j.rs`             | `finished_parsing_set_position_j`             | `ed.rs`                   | `finished_parsing_ed`                   | ED                |
  | `erase_in_line.rs`              | `finished_parsing_erase_in_line`              | `el.rs`                   | `finished_parsing_el`                   | EL                |
  | `set_position.rs`               | `finished_parsing_set_position`               | `cup.rs`                  | `finished_parsing_cup`                  | CUP               |
  | `hvp.rs`                        | `finished_parsing_hvp`                        | —                         | —                                       | (already correct) |
  | `move_up.rs`                    | `finished_parsing_move_up`                    | `cuu.rs`                  | `finished_parsing_cuu`                  | CUU               |
  | `move_down.rs`                  | `finished_parsing_move_down`                  | `cud.rs`                  | `finished_parsing_cud`                  | CUD               |
  | `move_cursor_right.rs`          | `finished_parsing_move_cursor_right`          | `cuf.rs`                  | `finished_parsing_cuf`                  | CUF               |
  | `move_cursor_left.rs`           | `finished_parsing_move_cursor_left`           | `cub.rs`                  | `finished_parsing_cub`                  | CUB               |
  | `ict.rs`                        | `finished_parsing_ich`                        | `ich.rs`                  | `finished_parsing_ich`                  | ICH               |
  | `set_top_and_bottom_margins.rs` | `finished_parsing_set_top_and_bottom_margins` | `decstbm.rs`              | `finished_parsing_decstbm`              | DECSTBM           |
  | `decslpp.rs`                    | `finished_parsing_decslpp`                    | —                         | —                                       | (already correct) |
  | `modify_other_keys.rs`          | `parse_modify_other_keys`                     | `modify_other_keys.rs`    | `finished_parsing_modify_other_keys`    | —                 |
  | `modify_other_keys_v2.rs`       | `parse_modify_other_keys_v2`                  | `modify_other_keys_v2.rs` | `finished_parsing_modify_other_keys_v2` | —                 |
  | `set_position_r.rs`             | `finished_parsing_set_position_r`             | `cup_r.rs`                | `finished_parsing_cup_r`                | CUP (`;` variant) |
  | `vpa.rs`                        | `finished_parsing_vpa`                        | —                         | —                                       | (already correct) |
  | `cha.rs`                        | `finished_parsing_cha`                        | —                         | —                                       | (already correct) |
  | `hpa.rs`                        | `finished_parsing_hpa`                        | —                         | —                                       | (already correct) |

  Files already using the correct pattern (no change needed): `cbt.rs`, `cht.rs`, `cnl.rs`,
  `cpl.rs`, `da1.rs`, `da2.rs`, `da3.rs`, `dch.rs`, `decrqm.rs`, `decscusr.rs`, `dl.rs`,
  `dsr.rs`, `ech.rs`, `hpa.rs`, `hvp.rs`, `il.rs`, `rep.rs`, `sd.rs`, `sgr.rs`, `su.rs`,
  `tbc.rs`, `vpa.rs`, `xtversion.rs`, `window_manipulation.rs`.

  Update the dispatch table in `csi.rs` to reference the new names.

  **Note on `set_position_r.rs`:** This handles `CSI ; r` which is actually DECSTBM (same as
  `set_top_and_bottom_margins.rs`). Investigate whether these can be merged. If they handle
  different parameter patterns of the same sequence, merge into a single `decstbm.rs`.

- **Acceptance criteria:**
  - All CSI command files follow mnemonic naming.
  - All functions follow `finished_parsing_<mnemonic>` pattern.
  - Dispatch table in `csi.rs` updated.
  - All existing tests pass.
  - `cargo clippy` clean.
- **Tests required:**
  - Existing CSI command tests continue to pass (renaming should not change behavior).
  - Run `cargo test --all` to verify.

---

### 25.3 — Split Input Encoding Out of `interface.rs`

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-terminal-emulator/src/interface.rs` (modify),
  `freminal-terminal-emulator/src/input.rs` (new)
- **Details:**
  `interface.rs` is 947 lines with three concerns. The input encoding logic (~375 lines,
  including `TerminalInput` enum, `to_payload()`, key-to-bytes conversion) should be
  extracted to `input.rs`.

  Move:
  - `TerminalInput` enum and all its variants.
  - `to_payload()` method and all key encoding helpers.
  - `char_to_ctrl_code()` and related utility functions.
  - Any associated tests.

  Keep in `interface.rs`:
  - `TerminalEmulator` struct and its methods (emulator API).
  - `build_snapshot()` (snapshot building).
  - `FreminalTermInputOutput` trait.

  After the split, `interface.rs` should be ~570 lines and `input.rs` ~375 lines.

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

- **Status:** Pending
- **Priority:** 3 — Low
- **Scope:** `freminal-terminal-emulator/src/state/data.rs` (delete),
  `freminal-terminal-emulator/src/state/mod.rs` (modify),
  `freminal-terminal-emulator/src/state/internal.rs` (modify)
- **Details:**
  `state/data.rs` is 6 lines containing a single re-export. Inline the re-export into
  `state/mod.rs` and delete `data.rs`.

- **Acceptance criteria:**
  - `state/data.rs` deleted.
  - Re-export moved to `state/mod.rs`.
  - All existing code compiles and tests pass.
- **Tests required:**
  - `cargo test --all` passes.

---

### 25.5 — Move `Theme` Enum to `freminal-common`

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-terminal-emulator/src/state/internal.rs` (modify),
  `freminal-common/src/` (modify)
- **Details:**
  The `Theme` enum in `state/internal.rs` is a UI concern that should live in
  `freminal-common` where other shared types reside. This allows the GUI crate to depend on
  the theme type without going through the emulator crate.
  1. Move the `Theme` enum to `freminal-common/src/theme.rs` (or an appropriate location).
  2. Add `pub mod theme;` to `freminal-common/src/lib.rs`.
  3. Update all import paths in `freminal-terminal-emulator` and `freminal`.

- **Acceptance criteria:**
  - `Theme` lives in `freminal-common`.
  - All imports updated.
  - No compilation errors.
- **Tests required:**
  - `cargo test --all` passes.

---

### 25.6 — Remove Dead `scroll()` Method

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-terminal-emulator/src/state/internal.rs`
- **Details:**
  `TerminalState::scroll()` discards the return values from `handle_scroll_back()`,
  `handle_scroll_forward()`, and `handle_scroll_to_bottom()`. Since the performance refactor
  (PERFORMANCE_PLAN.md Task 4), scroll offset is owned by the GUI's `ViewState`, not the
  emulator. The GUI calls the handler methods directly via the snapshot/channel architecture.
  The `scroll()` method on `TerminalState` is dead code — it is never called from a path
  that uses the return values.

  Verify that `scroll()` has no callers (or that all callers are also dead), then remove it.

- **Acceptance criteria:**
  - `scroll()` method removed from `TerminalState`.
  - No compilation errors (confirming no live callers).
  - No `#[allow(dead_code)]` needed.
- **Tests required:**
  - `cargo test --all` passes.
  - `cargo clippy` clean (no dead code warnings).

---

### 25.7 — Delete Dead `StandardOutput` Enum

- **Status:** Pending
- **Priority:** 2 — Medium
- **Scope:** `freminal-terminal-emulator/src/ansi_components/standard.rs`
- **Details:**
  The `StandardOutput` enum (lines 21-39, 16 variants) is never constructed or pattern-matched
  anywhere in the codebase. It appears to be a remnant of an earlier parser design. Delete it.

  **Note:** If subtask 25.1 is done first, this deletion happens as part of that subtask.
  If 25.7 is done first, it is a standalone deletion. Either order is fine.

- **Acceptance criteria:**
  - `StandardOutput` enum deleted.
  - No compilation errors.
- **Tests required:**
  - `cargo test --all` passes.

---

### 25.8 — Add Doc Comments to CSI Command Public Functions

- **Status:** Pending
- **Priority:** 3 — Low
- **Scope:** `freminal-terminal-emulator/src/ansi_components/csi_commands/` (multiple files)
- **Details:**
  Several CSI command files have doc comments on private helpers but not on the public
  `finished_parsing_*` functions. Add a one-line doc comment to each public function
  identifying:
  - The ECMA-48 / DEC mnemonic.
  - The CSI sequence format (e.g. `CSI Ps J`).
  - A brief description of what the command does.

  Example:

  ```rust
  /// ED — Erase in Display (`CSI Ps J`). Erases part of the display based on Ps.
  pub fn finished_parsing_ed(params: &[u8]) -> TerminalOutput {
  ```

  Also fix `decrqm.rs` where the doc title is incorrect (describes wrong command).

- **Acceptance criteria:**
  - All `finished_parsing_*` functions have a doc comment.
  - Doc comments include the mnemonic, sequence format, and brief description.
  - `decrqm.rs` doc title is correct.
- **Tests required:** None (documentation only).

---

## Implementation Notes

### Subtask Ordering

25.1 (parser split) and 25.2 (CSI naming) are independent.
25.3 (interface.rs split) is independent.
25.4 (data.rs inline) is trivial and independent.
25.5 (Theme move) is independent.
25.6 (dead scroll) is independent.
25.7 (dead StandardOutput) can be done as part of 25.1 or independently.
25.8 (doc comments) should be done after 25.2 (names are finalized).

Many of these can run in parallel. The only ordering constraint is:

- 25.8 after 25.2 (doc comments should use final names).
- 25.7 before or as part of 25.1.

**Recommended order:** 25.7 → 25.1 → 25.2 → 25.8 → (25.3, 25.4, 25.5, 25.6 in any order)

### Risk Assessment

All subtasks are pure refactoring — no behavior change. The primary risk is merge conflicts
if other tasks modify the same files concurrently. To minimize risk:

- Do the parser split (25.1) early, as it touches the most files.
- Do the CSI renaming (25.2) in a single commit to avoid half-renamed intermediate states.
- Each subtask should be a single atomic commit.

### Verification

Each subtask must pass before proceeding:

- `cargo test --all`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo-machete`

---

## References

- `freminal-terminal-emulator/src/ansi_components/standard.rs` — parser to split (518 lines)
- `freminal-terminal-emulator/src/ansi_components/csi_commands/` — 34 CSI command files
- `freminal-terminal-emulator/src/ansi_components/csi.rs` — CSI dispatch table
- `freminal-terminal-emulator/src/interface.rs` — emulator API + input encoding (947 lines)
- `freminal-terminal-emulator/src/state/internal.rs` — dead `scroll()`, misplaced `Theme`
- `freminal-terminal-emulator/src/state/data.rs` — 6-line single re-export
- `freminal-terminal-emulator/src/ansi.rs` — top-level parser with `ParserInner` variants
