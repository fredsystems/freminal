# PLAN_29 — God File Refactoring

## Status: Complete

---

## Overview

A full responsibility-based audit of every `.rs` file in the workspace (100+ files across 5
crates) identified **3 god files** that manage multiple distinct subsystems and should be split
into focused, single-responsibility modules.

| File                                      | Lines | Crate           | Distinct Subsystems |
| ----------------------------------------- | ----- | --------------- | ------------------- |
| `freminal-buffer/src/terminal_handler.rs` | 9,536 | freminal-buffer | 8                   |
| `freminal/src/gui/renderer.rs`            | 2,846 | freminal        | 3                   |
| `freminal/src/gui/terminal.rs`            | 2,130 | freminal        | 3                   |

The remaining files are either **clean** (single responsibility) or **minor** (slight mixing
that does not justify splitting). The audit found 10 minor files; these are documented for
future reference but are out of scope for this task.

**Dependencies:** All other tasks. This should be the last task executed.
**Dependents:** None
**Primary crates:** `freminal-buffer`, `freminal`
**Estimated scope:** High — 3 god files producing ~15 new modules

---

## Audit Methodology

The audit classified every `.rs` file by **responsibility count**, not line count. A 2,000-line
file with one responsibility is clean. A 400-line file with 3 unrelated concerns is a god file.
The criterion: does the file manage multiple distinct subsystems that could be developed, tested,
and understood independently?

---

## God File Analysis

### 1. `freminal-buffer/src/terminal_handler.rs` (9,536 lines)

**Production code:** ~4,431 lines | **Inline tests:** ~5,103 lines

Eight distinct, independently meaningful subsystems:

| #   | Subsystem                 | Description                                                                                  | Separable? |
| --- | ------------------------- | -------------------------------------------------------------------------------------------- | ---------- |
| 1   | PTY response encoding     | `write_to_pty`, `write_bytes_to_pty`, `write_csi_response`, `write_dcs_response`, C1 helpers | Yes        |
| 2   | SGR / FormatTag mapping   | `apply_sgr`, `handle_sgr`, `build_sgr_response`, color append helpers                        | Yes        |
| 3   | Kitty graphics protocol   | `handle_kitty_graphics` + 12 helper functions — complete self-contained APC sub-protocol     | Yes        |
| 4   | Sixel graphics protocol   | `handle_sixel` + all sixel decoding helpers — complete DCS sub-protocol                      | Yes        |
| 5   | iTerm2 inline images      | `handle_iterm2_inline_image`, multipart state machine, dimension/aspect helpers              | Yes        |
| 6   | DCS sub-protocol dispatch | `handle_device_control_string`, DECRQSS, XTGETTCAP, tmux passthrough dispatch                | Yes        |
| 7   | OSC color management      | `handle_osc_fg_bg_color`, palette overrides, fg/bg/cursor color handlers, theme integration  | Yes        |
| 8   | CWD + FTCS tracking       | `parse_osc7_uri`, `percent_decode`, OSC 133 prompt/command/output zone tracking              | Yes        |

**Proposed split:**

```text
freminal-buffer/src/terminal_handler/
    mod.rs                  — TerminalHandler struct, construction, process_output dispatch,
                              resize, scroll, cursor, erase, tab stops, mode management,
                              GUI data extraction (the core handler)
    pty_writer.rs           — write_to_pty, write_bytes_to_pty, write_csi/dcs/osc_response,
                              S8C1T/7-bit C1 encoding helpers
    sgr.rs                  — apply_sgr (free fn), handle_sgr, build_sgr_response,
                              append_color_sgr, append_underline_color_sgr
    dcs.rs                  — handle_device_control_string, handle_decrqss, handle_xtgettcap,
                              lookup_termcap, tmux passthrough (handle_tmux_passthrough,
                              dispatch_tmux_csi, undouble_esc, double_esc, wrap_tmux_passthrough)
    graphics_kitty.rs       — handle_kitty_graphics, handle_kitty_query, handle_kitty_chunk_start,
                              handle_kitty_chunk, handle_kitty_single, handle_kitty_put,
                              decode_kitty_payload, resolve_kitty_transmission, read_kitty_file,
                              require_kitty_dimensions, place_kitty_image, send_kitty_error,
                              handle_kitty_delete
    graphics_sixel.rs       — handle_sixel + all sixel decoding helpers
    graphics_iterm2.rs      — handle_iterm2_inline_image, handle_iterm2_multipart_begin,
                              handle_iterm2_file_part, handle_iterm2_file_end,
                              resolve_image_dimension, apply_aspect_ratio
    osc_colors.rs           — handle_osc_fg_bg_color, palette override state and handlers,
                              fg/bg/cursor color set/reset/query
    shell_integration.rs    — OSC 7 CWD tracking (parse_osc7_uri, percent_decode, hex_val),
                              OSC 133 FTCS prompt/command/output zone tracking
```

**Test distribution:** The existing 5,103-line inline test suite and the 5,636-line
`terminal_handler_integration.rs` external test file would be distributed to co-located
`#[cfg(test)] mod tests` blocks within each new module. Tests that exercise cross-module
interactions remain in the integration test file.

---

### 2. `freminal/src/gui/renderer.rs` (2,846 lines)

Three distinct subsystems:

| #   | Subsystem           | Description                                                              |
| --- | ------------------- | ------------------------------------------------------------------------ |
| 1   | GPU resource mgmt   | `TerminalRenderer` struct, GL state, `init()`, `draw_*()`, `destroy()`   |
| 2   | CPU vertex builders | `build_background_instances`, `build_foreground_instances`, `push_quad`, |
|     |                     | `build_cursor_verts_only`, `build_image_verts`, `FgRenderOptions`,       |
|     |                     | `ImageBounds`, `is_cell_selected`, `run_col_count`, `extract_atlas_rect` |
| 3   | GLSL shader sources | Four inline `const &str` blocks for decoration, bg, fg, and image passes |

The CPU vertex builders are pure functions over snapshot data — already have their own test suite,
zero GL dependencies. Extracting them makes them independently compilable and testable.

**Proposed split:**

```text
freminal/src/gui/renderer/
    mod.rs        — re-exports; public surface unchanged
    gpu.rs        — TerminalRenderer struct, init/draw/destroy, GL upload helpers,
                    VAO setup, shader compilation, numeric GL helpers (gl_i32, gl_f32, etc.)
    shaders.rs    — GLSL source string constants (4 shader passes)
    vertex.rs     — build_background_instances, build_foreground_instances,
                    build_cursor_verts_only, build_image_verts, push_quad,
                    FgRenderOptions, ImageBounds, is_cell_selected, run_col_count,
                    extract_atlas_rect — plus their existing test suite
```

---

### 3. `freminal/src/gui/terminal.rs` (2,130 lines)

Three distinct subsystems:

| #   | Subsystem            | Description                                                               |
| --- | -------------------- | ------------------------------------------------------------------------- |
| 1   | Render orchestration | `FreminalTerminalWidget`, `RenderState`, `show()`, scrollbar, URL hover,  |
|     |                      | `apply_config_changes`, `invalidate_theme_cache`, `sync_pixels_per_point` |
| 2   | Input translation    | `write_input_to_terminal` (~600 lines), `control_key`,                    |
|     |                      | `egui_mods_to_key_modifiers`, `send_terminal_inputs`, `InputModes`,       |
|     |                      | `handle_scroll_fallback` — protocol encoding, no GPU access               |
| 3   | Coordinate utilities | `visible_window_start`, `encode_egui_mouse_pos_as_usize`,                 |
|     |                      | `flat_index_for_cell` — pure math over snapshot fields                    |

**Proposed split:**

```text
freminal/src/gui/terminal/
    mod.rs        — re-exports; public surface unchanged
    widget.rs     — FreminalTerminalWidget, RenderState, show(), apply_config_changes,
                    invalidate_theme_cache, sync_pixels_per_point, paint_scrollbar,
                    URL hover detection
    input.rs      — write_input_to_terminal, control_key, egui_mods_to_key_modifiers,
                    send_terminal_inputs, InputModes, handle_scroll_fallback
    coords.rs     — visible_window_start, encode_egui_mouse_pos_as_usize,
                    flat_index_for_cell
```

---

## Minor Files (Out of Scope — Documented for Reference)

These files have slight responsibility mixing but are not worth splitting as part of this task.

| File                                                    | Lines | Crate             | Notes                                                            |
| ------------------------------------------------------- | ----- | ----------------- | ---------------------------------------------------------------- |
| `freminal-buffer/src/buffer.rs`                         | 7,068 | freminal-buffer   | Single responsibility (Buffer struct) — large but clean          |
| `freminal-terminal-emulator/src/ansi.rs`                | 1,512 | terminal-emulator | Param utilities shared across parsers could be a `params.rs`     |
| `freminal-terminal-emulator/src/io/pty.rs`              | 522   | terminal-emulator | Locale/terminfo setup separable from PTY I/O                     |
| `freminal-terminal-emulator/src/interface.rs`           | 705   | terminal-emulator | Snapshot building could move to `snapshot_builder.rs`            |
| `freminal-terminal-emulator/src/state/internal.rs`      | 650   | terminal-emulator | Mode sync is a distinct sub-concern                              |
| `freminal-terminal-emulator/src/ansi_components/osc.rs` | 337   | terminal-emulator | Duplicated param utilities from `ansi.rs`                        |
| `freminal-common/src/config.rs`                         | 918   | freminal-common   | `log_dir` is a minor tangent                                     |
| `freminal/src/main.rs`                                  | 570   | freminal          | Entry-point glue — expected breadth                              |
| `freminal/src/gui/mod.rs`                               | 980   | freminal          | `FreminalGui` + window manipulation — borderline                 |
| `freminal/src/gui/mouse.rs`                             | 790   | freminal          | Mouse tracking + encoding — tightly coupled, not worth splitting |

The duplicated param-parsing utilities in `ansi.rs` and `osc.rs` are the most actionable minor
item — consolidating them into a shared `params.rs` removes duplication without structural risk.
This could be done as an addendum subtask if time permits.

---

## Guiding Principles

- **One responsibility per file.** A file should do one thing. If you need a compound name
  like `terminal_handler_image_protocols.rs`, it should probably just be `image_protocols.rs`
  inside a `terminal_handler/` directory.
- **Prefer directories over prefixed files.** If `terminal_handler.rs` splits into 9 modules,
  they should live in `terminal_handler/mod.rs` + siblings, not as 9 top-level files with
  `terminal_handler_` prefixes.
- **Internal (`pub(crate)`) over public.** Splitting a file into modules should not widen the
  public API. Use `pub(crate)` for inter-module access within the same crate.
- **Tests stay with the code they test.** Each new module gets its own `#[cfg(test)] mod tests`
  section containing the tests that were previously in the monolithic file.
- **No behavior changes.** This is purely structural. The refactor must not change any
  observable behavior. `cargo test --all` must pass at every intermediate step.
- **Atomic commits.** One commit per subtask. Each commit must leave the tree green.

---

## Subtasks

Ordered to minimize merge conflict risk. `terminal_handler.rs` is split first because it is the
largest and has the cleanest subsystem boundaries. The `freminal` crate splits come second.

### Subtask 29.1 — Split `terminal_handler.rs`: extract `pty_writer.rs` ✅

Extract PTY response encoding into `terminal_handler/pty_writer.rs`:

- `write_to_pty`, `write_bytes_to_pty`, `write_csi_response`, `write_dcs_response`,
  `write_osc_response`, S8C1T/7-bit C1 encoding helpers
- Move associated tests

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

**Completed 2026-04-06.** Converted `terminal_handler.rs` into a `terminal_handler/` directory
with `mod.rs` + `pty_writer.rs`. Extracted the 5 C1 encoding helpers (`csi_response`,
`dcs_response`, `osc_response`, `st_response`) and the 5 write methods (`write_to_pty`,
`write_bytes_to_pty`, `write_csi_response`, `write_dcs_response`, `write_osc_response`) into
`pty_writer.rs` as a separate `impl TerminalHandler` block. All methods changed from private
`fn` to `pub(super)`. The `direct_write_not_wrapped` test was moved to `pty_writer.rs`.
`cargo test --all`: all tests pass. `cargo clippy --all-targets --all-features -- -D warnings`:
clean. `cargo-machete`: no unused dependencies.

### Subtask 29.2 — Split `terminal_handler.rs`: extract `sgr.rs` ✅

Extract SGR/FormatTag mapping into `terminal_handler/sgr.rs`:

- `apply_sgr` (free function), `handle_sgr`, `build_sgr_response`, `append_color_sgr`,
  `append_underline_color_sgr`
- Move associated tests

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

### Subtask 29.3 — Split `terminal_handler.rs`: extract `graphics_kitty.rs` ✅

Extract Kitty graphics protocol into `terminal_handler/graphics_kitty.rs`:

- All `handle_kitty_*` functions, `decode_kitty_payload`, `resolve_kitty_transmission`,
  `read_kitty_file`, `require_kitty_dimensions`, `place_kitty_image`, `send_kitty_error`
- Move associated tests

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

### Subtask 29.4 — Split `terminal_handler.rs`: extract `graphics_sixel.rs` ✅

Extract Sixel graphics protocol into `terminal_handler/graphics_sixel.rs`:

- `handle_sixel` + all sixel decoding helpers
- Move associated tests

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

### Subtask 29.5 — Split `terminal_handler.rs`: extract `graphics_iterm2.rs` ✅

Extract iTerm2 inline image protocol into `terminal_handler/graphics_iterm2.rs`:

- `handle_iterm2_inline_image`, `handle_iterm2_multipart_begin`, `handle_iterm2_file_part`,
  `handle_iterm2_file_end`, `resolve_image_dimension`, `apply_aspect_ratio`
- Move associated tests

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

### Subtask 29.6 — Split `terminal_handler.rs`: extract `dcs.rs` ✅

Extract DCS sub-protocol dispatch into `terminal_handler/dcs.rs`:

- `handle_device_control_string`, `handle_decrqss`, `handle_xtgettcap`, `lookup_termcap`,
  `handle_tmux_passthrough`, `dispatch_tmux_csi`, `undouble_esc`, `double_esc`,
  `wrap_tmux_passthrough`
- Move associated tests

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

### Subtask 29.7 — Split `terminal_handler.rs`: extract `osc_colors.rs` ✅

Extract OSC color management into `terminal_handler/osc_colors.rs`:

- `handle_osc_fg_bg_color`, palette override state and handlers, fg/bg/cursor color
  set/reset/query
- Move associated tests

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

### Subtask 29.8 — Split `terminal_handler.rs`: extract `shell_integration.rs` ✅

Extract CWD + FTCS tracking into `terminal_handler/shell_integration.rs`:

- `parse_osc7_uri`, `percent_decode`, `hex_val`, OSC 133 FTCS zone tracking
- Move associated tests

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

### Subtask 29.9 — Split `renderer.rs`: extract `vertex.rs` and `shaders.rs` ✅

Convert `freminal/src/gui/renderer.rs` into a `renderer/` directory:

- `renderer/mod.rs` — re-exports
- `renderer/gpu.rs` — `TerminalRenderer`, GL state, init/draw/destroy, upload helpers
- `renderer/shaders.rs` — GLSL source constants
- `renderer/vertex.rs` — `build_background_instances`, `build_foreground_instances`,
  `build_cursor_verts_only`, `build_image_verts`, `push_quad`, supporting types + tests

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

### Subtask 29.10 — Split `terminal.rs`: extract `input.rs` and `coords.rs` ✅

Convert `freminal/src/gui/terminal.rs` into a `terminal/` directory:

- `terminal/mod.rs` — re-exports
- `terminal/widget.rs` — `FreminalTerminalWidget`, `RenderState`, `show()`, config, scrollbar
- `terminal/input.rs` — `write_input_to_terminal`, `control_key`,
  `egui_mods_to_key_modifiers`, `send_terminal_inputs`, `InputModes`,
  `handle_scroll_fallback`
- `terminal/coords.rs` — `visible_window_start`, `encode_egui_mouse_pos_as_usize`,
  `flat_index_for_cell`

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

### Subtask 29.11 — Extract shared param utilities (addendum) ✅

Consolidate duplicated parameter-parsing utilities from
`freminal-terminal-emulator/src/ansi.rs` and `src/ansi_components/osc.rs`:

- Upgraded `ansi.rs::parse_param_as` to include `debug!()` logging from the `osc.rs` copy
- Deleted the duplicate `parse_param_as` from `osc.rs`; `osc.rs` now imports from `crate::ansi`
- Renamed `osc.rs::split_params_into_semicolon_delimited_usize` to
  `split_params_into_semicolon_delimited_tokens` (it returns `AnsiOscToken`, not `usize`)
- Demoted `osc.rs::extract_param` from `pub` to `fn` (only used within `osc.rs`)
- Note: `extract_param` was not a true duplicate (different element types: `usize` vs
  `AnsiOscToken`). `split_params_into_semicolon_delimited_usize` was similarly not a true
  duplicate (different return types). Only `parse_param_as<T>` was genuinely consolidatable.

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

### Subtask 29.12 — Final verification and plan update ✅

- Run full verification suite: `cargo test --all`, `cargo clippy --all-targets --all-features
-- -D warnings`, `cargo-machete` — all pass
- Benchmarks compile cleanly (`cargo bench --no-run --all`)
- This document updated: all subtasks marked complete
- `MASTER_PLAN.md` updated: Task 29 marked complete

---

## References

- `agents.md` — Agent rules, crate-specific guidance
- `Documents/MASTER_PLAN.md` — Task 29 entry
- `Documents/PLAN_25_CODE_QUALITY.md` — Identified the god files as out of scope for Task 25
