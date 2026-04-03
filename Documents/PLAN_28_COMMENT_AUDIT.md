# PLAN_28 — Code Comment Audit

## Status: In Progress

---

## Overview

The codebase has generally good documentation and comments, but a systematic audit has now been
performed to verify comment quality along four dimensions: accuracy, coverage, depth, and noise.

The audit walked every source file in all five crates in dependency order. Findings are compiled
below into 10 actionable subtasks grouped by severity and crate affinity.

**Dependencies:** None (independent)
**Dependents:** None
**Primary crates:** All (`freminal`, `freminal-terminal-emulator`, `freminal-buffer`,
`freminal-common`, `xtask`)
**Estimated scope:** 10 subtasks, medium effort

---

## Audit Summary

| Crate                        | Files | Incorrect | Stale  | Noise  | Missing Docs | Depth Gaps |
| ---------------------------- | ----- | --------- | ------ | ------ | ------------ | ---------- |
| `freminal-common`            | 63    | 11        | 2      | 2      | 15           | 4          |
| `freminal-buffer`            | 7     | 3         | 4      | 5      | ~35          | 5          |
| `freminal-terminal-emulator` | 54    | 4         | 4      | 4      | ~16          | 6          |
| `freminal` (GUI)             | 14    | 4         | 6      | 5      | ~45          | 6          |
| `xtask`                      | 1     | 3         | 2      | 2      | 4            | 4          |
| **Total**                    | 139   | **25**    | **18** | **18** | **~115**     | **25**     |

---

## Subtasks

### Subtask 28.1 — Fix incorrect comments across all crates (Highest priority)

**Severity:** Incorrect — misleading comments are bugs.
**Scope:** All 5 crates, ~25 individual fixes.
**Estimated effort:** Low-medium (each fix is a one- or two-line edit).

#### freminal-common (11 items)

| File                                                  | Line(s)         | Issue                                                           |
| ----------------------------------------------------- | --------------- | --------------------------------------------------------------- |
| `src/buffer_states/modes/allow_column_mode_switch.rs` | Module/type doc | Doc comment describes wrong mode (copied from DECTCEM template) |
| `src/buffer_states/modes/decarm.rs`                   | Module/type doc | Doc comment describes wrong mode (copied from DECTCEM template) |
| `src/buffer_states/modes/decom.rs`                    | Module/type doc | Doc comment describes wrong mode (copied from DECTCEM template) |
| `src/buffer_states/modes/decsclm.rs`                  | Module/type doc | Doc comment describes wrong mode (copied from DECTCEM template) |
| `src/buffer_states/modes/decscnm.rs`                  | Module/type doc | Doc comment describes wrong mode (copied from DECTCEM template) |
| `src/buffer_states/modes/reverse_wrap_around.rs`      | Module/type doc | Doc comment describes wrong mode (copied from DECTCEM template) |
| `src/buffer_states/modes/xtcblink.rs`                 | Module/type doc | Doc comment describes wrong mode (copied from DECTCEM template) |
| `src/colors.rs`                                       | xterm index 16  | Misrepresents the color value for xterm index 16                |
| `src/grapheme.rs`                                     | Doc comment     | Incorrect or misleading description                             |
| `src/terminal_output.rs`                              | Crate reference | References wrong crate                                          |
| `src/themes.rs`                                       | Doc comment     | Incorrect description                                           |

#### freminal-buffer (3 items)

| File                      | Line(s)                  | Issue                                                                                   |
| ------------------------- | ------------------------ | --------------------------------------------------------------------------------------- |
| `src/buffer.rs`           | ~2043-2055               | Doc comment from `enter_alternate` bleeds into `set_cursor_pos`                         |
| `src/buffer.rs`           | ~1683-1685               | `visible_window_start` has doubled/duplicated first sentence                            |
| `src/terminal_handler.rs` | ~823-824, 830, 3121-3122 | Stale "Task 7/8 wiring" notes reference completed performance plan tasks as future work |

#### freminal-terminal-emulator (4 items)

| File                                      | Line(s)               | Issue                                                              |
| ----------------------------------------- | --------------------- | ------------------------------------------------------------------ |
| `src/state/internal.rs`                   | `set_win_size` doc    | Says "Set the window title" — copy-paste from neighboring function |
| Error display impl                        | `UnhandledSGRCommand` | Display string says "Invalid cursor (SGR) set position sequence"   |
| `src/ansi_components/csi_commands/ech.rs` | Error return          | Returns `UnhandledDCHCommand` instead of `UnhandledECHCommand`     |
| `src/state/internal.rs`                   | `Default` impl        | Stale comment referencing old architecture                         |

#### freminal (GUI) (4 items)

| File                    | Line(s)             | Issue                                                                       |
| ----------------------- | ------------------- | --------------------------------------------------------------------------- |
| `src/gui/colors.rs`     | Faint dimming       | Doc diverges between GL and egui paths — describes one, code does the other |
| `src/gui/shaping.rs`    | "should not happen" | Comment says condition shouldn't occur, but code explicitly handles it      |
| `src/gui/mouse.rs`      | Uncertainty         | Unresolved uncertainty/question left in code                                |
| `src/gui/view_state.rs` | Task references     | References completed performance plan tasks as future work                  |

#### xtask (3 items)

| File          | Line(s)       | Issue                                           |
| ------------- | ------------- | ----------------------------------------------- |
| `src/main.rs` | `CI` variant  | Omits `deny`/`machete` from CI step description |
| `src/main.rs` | `ci()` fn doc | Same — omits `deny`/`machete`                   |
| `src/main.rs` | `test()` fn   | Mentions nonexistent "backends"                 |

**Verification:** `cargo test --all` passes.
`cargo clippy --all-targets --all-features -- -D warnings` clean.

---

### Subtask 28.2 — Remove stale comments and commented-out code

**Severity:** Stale — references old behavior, deleted code, or completed work items.
**Scope:** All 5 crates, ~18 individual items.
**Estimated effort:** Low.

| Crate                        | File                         | Issue                                            |
| ---------------------------- | ---------------------------- | ------------------------------------------------ |
| `freminal-common`            | `src/lib.rs`                 | Commented-out `#![warn(missing_docs)]`           |
| `freminal-common`            | `src/cursor.rs`              | Dead/stale field comment                         |
| `freminal-buffer`            | `src/buffer.rs` ~592-594     | Duplicate/merged `set_size` doc sentence         |
| `freminal-buffer`            | `src/buffer.rs` ~2290-2303   | Duplicate `visible_as_tchars_and_tags` doc       |
| `freminal-buffer`            | `src/buffer.rs` ~117         | Orphaned comment on `SavedPrimaryState`          |
| `freminal-buffer`            | `src/terminal_handler.rs`    | Task 7/8 wiring notes (also in 28.1)             |
| `freminal-terminal-emulator` | `src/ansi_components/osc.rs` | Commented-out import                             |
| `freminal-terminal-emulator` | `src/ansi_components/osc.rs` | Commented-out match arm                          |
| `freminal-terminal-emulator` | `src/ansi.rs` or similar     | `#[allow(dead_code)]` on `SequenceTraceable`     |
| `freminal-terminal-emulator` | `src/lib.rs`                 | Commented-out `#![warn(missing_docs)]`           |
| `freminal`                   | `src/main.rs`                | Commented-out `#![warn(missing_docs)]`           |
| `freminal`                   | `src/lib.rs`                 | Commented-out `#![warn(missing_docs)]`           |
| `freminal`                   | `src/gui/fonts.rs`           | Commented-out constant                           |
| `freminal`                   | `src/gui/font_manager.rs`    | Undeclared/orphan TODO                           |
| `freminal`                   | `src/gui/view_state.rs`      | Task references to completed work (also in 28.1) |
| `freminal`                   | `src/gui/fonts.rs`           | Usage example as `//` instead of `///`           |
| `xtask`                      | `src/main.rs`                | Commented-out `Backend` enum from ratatui        |
| `xtask`                      | `src/main.rs`                | Stale `test_docs` perf note from ratatui era     |

**Verification:** `cargo test --all` passes.
`cargo clippy --all-targets --all-features -- -D warnings` clean.

---

### Subtask 28.3 — Remove noise comments

**Severity:** Noise — comments that restate the obvious or add no information.
**Scope:** All 5 crates, ~18 individual items.
**Estimated effort:** Low.

| Crate                        | File                                      | Issue                                                     |
| ---------------------------- | ----------------------------------------- | --------------------------------------------------------- |
| `freminal-common`            | `src/sgr.rs`                              | Restates `#[default]` attribute                           |
| `freminal-common`            | `src/mouse.rs`                            | Self-deprecating developer note                           |
| `freminal-buffer`            | `src/buffer.rs`                           | `FIX #3:` label on an already-applied fix                 |
| `freminal-buffer`            | `src/buffer.rs`                           | `(keep your existing Alternate LF unchanged)` instruction |
| `freminal-buffer`            | `src/buffer.rs`                           | `(unchanged)` annotation                                  |
| `freminal-buffer`            | `src/buffer.rs`                           | Redundant `// tests` module label                         |
| `freminal-buffer`            | `src/terminal_handler.rs`                 | Thin restatement doc comments on delegation methods       |
| `freminal-terminal-emulator` | `src/ansi_components/osc.rs`              | `// get the parameter at the index`                       |
| `freminal-terminal-emulator` | `src/ansi_components/csi_commands/cnl.rs` | Restates the operation the code already shows             |
| `freminal-terminal-emulator` | `src/ansi_components/csi_commands/cpl.rs` | Restates the operation the code already shows             |
| `freminal-terminal-emulator` | `src/ansi_components/csi_commands/ich.rs` | Bare ECMA reference without explanation                   |
| `freminal`                   | `src/gui/terminal.rs`                     | Commented-out code                                        |
| `freminal`                   | `src/gui/fonts.rs`                        | Dead function body                                        |
| `freminal`                   | `src/gui/shaping.rs`                      | Trivial doc comment on obvious function                   |
| `freminal`                   | `src/gui/mod.rs`                          | Hedging language ("probably", "maybe")                    |
| `freminal`                   | `src/gui/mouse.rs`                        | Unexplained `#[allow(dead_code)]`                         |
| `xtask`                      | `src/main.rs`                             | Shallow `CARGO` env comment                               |
| `xtask`                      | `src/main.rs`                             | Commented-out lint directive                              |

**Verification:** `cargo test --all` passes.
`cargo clippy --all-targets --all-features -- -D warnings` clean.

---

### Subtask 28.4 — Add crate-level `//!` doc comments

**Severity:** Missing — every crate should have a top-level description.
**Scope:** All 5 crate `lib.rs` / `main.rs` files.
**Estimated effort:** Low.

| File                                    | What to add                                                  |
| --------------------------------------- | ------------------------------------------------------------ |
| `freminal-common/src/lib.rs`            | `//!` describing shared types and utilities                  |
| `freminal-buffer/src/lib.rs`            | `//!` describing the cell-based terminal buffer model        |
| `freminal-terminal-emulator/src/lib.rs` | `//!` describing the ANSI parser and terminal state machine  |
| `freminal/src/lib.rs`                   | `//!` describing the GUI application                         |
| `freminal/src/main.rs`                  | `//!` describing the binary entry point and PTY thread model |
| `xtask/src/main.rs`                     | `//!` describing the build/CI orchestration tool             |

**Verification:** `cargo doc --all --no-deps` produces no warnings. Crate docs have a
meaningful landing page.

---

### Subtask 28.5 — Add missing doc comments to `freminal-common` public APIs

**Severity:** Missing — public APIs must be documented.
**Scope:** `freminal-common` crate, ~15 items.
**Estimated effort:** Low-medium.

Items needing `///` doc comments:

- `src/buffer_states/modes/mod.rs` — trait definitions
- `src/buffer_states/mod.rs` — module-level types
- `src/window_manipulation.rs` — public types and variants
- `src/url.rs` — `UrlRange` and related types
- `src/format_tag.rs` — `FormatTag` fields and methods
- `src/cursor.rs` — `CursorPos`, `CursorVisualStyle`, related types
- `src/fonts.rs` — font-related types
- `src/sgr.rs` — SGR-related types beyond the default
- `src/terminal_size.rs` — `FreminalTerminalSize` and fields
- `src/terminfo.rs` — terminfo utility functions
- Several mode files — individual enum variants

**Verification:** `cargo doc --all --no-deps` produces no warnings for `freminal-common`.

---

### Subtask 28.6 — Add missing doc comments to `freminal-buffer` public APIs

**Severity:** Missing — public APIs must be documented.
**Scope:** `freminal-buffer` crate, ~35 items.
**Estimated effort:** Medium.

This is the largest documentation gap. Items needing `///` doc comments:

- `src/lib.rs` — re-exports
- `Cell` struct and all public methods
- `Row`, `RowOrigin`, `RowJoin` — structs, enums, and all public methods
- `Buffer` struct — overall purpose and design
- `SavedPrimaryState` — all fields
- `Buffer` methods — `push_row`, `scroll_slice_up/down`, `scroll_up`, `handle_lf`,
  `resize_height`, `reflow_to_width`, `enforce_scrollback_limit`, `visible_rows`,
  `visible_window_start`, `visible_as_tchars_and_tags`, `scrollback_as_tchars_and_tags`,
  `enter_alternate`, `leave_alternate`, `set_size`, `max_scroll_offset`, `mark_clean`,
  `any_visible_dirty`, and other public methods
- `terminal_handler.rs` — `handle_resize` and other undocumented public methods

**Verification:** `cargo doc --all --no-deps` produces no warnings for `freminal-buffer`.

---

### Subtask 28.7 — Add missing doc comments to `freminal-terminal-emulator` public APIs

**Severity:** Missing — public APIs must be documented.
**Scope:** `freminal-terminal-emulator` crate, ~16 items.
**Estimated effort:** Medium.

Items needing `///` doc comments:

- `TerminalEmulator` struct — overall purpose
- `FreminalAnsiParser` and `ParserInner` — struct-level docs
- `push` method on the parser — entry point docs
- `extract_param` / `parse_param_as` — utility function docs
- `TerminalState` — struct-level docs and key methods
- `PtyRead` — type doc
- `split_format_data_for_scrollback` — function doc
- `ParserFailures` — type doc
- `AnsiCsiParser`, `AnsiOscParser`, `StandardParser` — struct docs
- `SequenceTracer` — type doc
- `src/ansi_components/csi_commands/mod.rs` — module doc
- `handle_custom_color` — function doc

**Verification:** `cargo doc --all --no-deps` produces no warnings for
`freminal-terminal-emulator`.

---

### Subtask 28.8 — Add missing doc comments to `freminal` (GUI) public APIs

**Severity:** Missing — public APIs must be documented.
**Scope:** `freminal` crate, ~45 items.
**Estimated effort:** Medium-high (largest single subtask by item count).

This crate has the most documentation gaps. Items needing `///` doc comments:

- `src/main.rs` — `run_terminal`, PTY thread setup functions
- `src/lib.rs` — re-exports
- `src/gui/mod.rs` — `FreminalGui` struct and methods, `run()`, `handle_window_manipulation()`
- `src/gui/terminal.rs` — `FreminalTerminalWidget`, `show()`, `write_input_to_terminal()`,
  `render_terminal_output()`, `process_tags()`, `create_terminal_output_layout_job()`,
  `add_terminal_data_to_ui()`
- `src/gui/fonts.rs` — font loading functions, `FontMetrics`
- `src/gui/font_manager.rs` — `FontManager` struct and methods
- `src/gui/mouse.rs` — mouse handling functions, X11 encoding functions
- `src/gui/settings.rs` — settings modal types and methods
- `src/gui/colors.rs` — color conversion functions
- `src/gui/view_state.rs` — `ViewState` fields

**Verification:** `cargo doc --all --no-deps` produces no warnings for `freminal`.

---

### Subtask 28.9 — Add missing doc comments to `xtask`

**Severity:** Missing — public APIs should be documented.
**Scope:** `xtask/src/main.rs`, ~4 items.
**Estimated effort:** Low.

Items needing `///` doc comments:

- `Machete` variant — convert `//` to `///`
- `Args` struct and fields
- `ExpressionExt` impl

**Verification:** `cargo doc --no-deps -p xtask` produces no warnings.

---

### Subtask 28.10 — Add depth/design comments to complex algorithms

**Severity:** Depth gap — new contributors need design context for complex code.
**Scope:** All crates, ~25 items.
**Estimated effort:** Medium-high (requires understanding the code deeply to write good comments).

#### freminal-common (4 items)

- `src/mouse.rs` — `report()` logic needs explanation of encoding schemes
- `src/window_manipulation.rs` — stub handlers need notes on which are intentionally unimplemented
- `src/buffer_states/modes/decbkm.rs` — explain non-standard default choice
- `src/buffer_states/modes/deccolm.rs` — explain non-standard default choice

#### freminal-buffer (5 items)

- `src/buffer.rs` `reflow_to_width` — needs high-level design comment explaining the algorithm
- `src/buffer.rs` — needs explanation of primary/alternate buffer model and `SavedPrimaryState`
- `src/buffer.rs` tag-merge step in `rows_as_tchars_and_tags` — explain the merging logic
- `src/terminal_handler.rs` `handle_resize` — document the resize strategy
- `src/terminal_handler.rs` `process_output` — explain catch-all arms

#### freminal-terminal-emulator (6 items)

- `src/ansi.rs` `push` — state machine needs overview comment explaining states and transitions
- `src/state/internal.rs` `handle_incoming_data` — document the pipeline stages
- `src/state/internal.rs` — explain `sync_mode` architecture split
- `src/ansi_components/csi_commands/mod.rs` — CSI terminator dispatch table needs overview
- `src/ansi_components/csi_commands/da.rs` — explain three-case Device Attributes disambiguation
- `src/ansi_components/csi_commands/decslrm.rs` — explain self-contradicting defensive branch

#### freminal (GUI) (6 items)

- `src/main.rs` — PTY threading model overview (ArcSwap, channels, ownership)
- `src/gui/mod.rs` `handle_window_manipulation` — document the flow and Report\* handling
- `src/gui/terminal.rs` `write_input_to_terminal` — document input routing logic
- `src/gui/terminal.rs` `compute_cell_metrics` — explain the cell sizing calculation
- `src/gui/mouse.rs` — X11 mouse encoding functions need protocol context
- `src/gui/settings.rs` — settings state machine (pending/applied/saved) needs overview

#### xtask (4 items)

- `src/main.rs` — `docs-rs` soft dependency explanation
- `src/main.rs` — nightly toolchain mechanism for `fix_clippy`
- `src/main.rs` — `fix_clippy` flag choices explanation
- `src/main.rs` — `ci()` step ordering rationale

**Verification:** Code review confirms comments are accurate and helpful. `cargo test --all`
passes (no functional changes).

---

## Execution Notes

### Priority Order

1. **28.1** — Incorrect comments (bugs). Highest priority.
2. **28.2** — Stale comments. Remove before adding new docs to avoid documenting dead patterns.
3. **28.3** — Noise comments. Remove before adding new docs to avoid style inconsistency.
4. **28.4** — Crate-level `//!` docs. Quick win, sets the stage for per-item docs.
5. **28.5-28.9** — Missing doc comments, by crate. Can be parallelized across crates.
6. **28.10** — Depth/design comments. Requires the most domain knowledge; do last.

### Parallelism

- Subtasks 28.1-28.3 can be done in parallel (they touch different comment categories and rarely
  overlap on the same line).
- Subtasks 28.5-28.9 can be done in parallel (one per crate, no cross-crate conflicts).
- Subtask 28.10 should be done last — it benefits from having the other comments cleaned up first.

### Agent Guidance

- All subtasks are **PATCH_IMPLEMENTATION** mode — code changes only (comment edits).
- No functional code changes permitted. Only comments and doc strings change.
- Each subtask must leave `cargo test --all` and
  `cargo clippy --all-targets --all-features -- -D warnings` passing.
- For subtask 28.1 (incorrect comments), the agent must verify the correct behavior before
  writing the replacement comment — read the code, understand what it actually does, then write
  the comment.
- For subtask 28.10 (depth comments), the agent must read enough surrounding code to write
  genuinely helpful design comments, not shallow restatements.

---

## References

- `agents.md` — Agent rules, documentation rules
- `Documents/MASTER_PLAN.md` — Task 28 entry
