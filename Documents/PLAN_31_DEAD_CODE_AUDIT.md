# PLAN_31 — Dead Code Audit

## Status: Complete

---

## Overview

Rust's dead code lint only fires on _private_ items. Any item marked `pub` — whether it is
actually used by downstream crates or not — is invisible to `rustc`'s and clippy's dead code
analysis. Over the course of 25+ completed tasks, APIs have been added, refactored, and
superseded, leaving behind `pub` functions, methods, structs, fields, and enum variants that no
live code path reaches. These phantom-public items:

- Inflate the API surface and confuse onboarding.
- Mask genuine dead code that the linter would otherwise catch.
- Bloat the binary with unreachable code that LTO may or may not eliminate.
- Create false confidence that the item is exercised by tests (it appears reachable, but nothing
  calls it).

This task performs a systematic audit of every `pub` item across all four library crates and the
binary, identifies items with zero downstream callers, and removes them (or demotes their
visibility to `pub(crate)` / private if internal callers exist).

**Dependencies:** None (independent)
**Dependents:** Task 29 (God File Refactoring) — a smaller API surface makes splitting easier
**Primary crates:** All (`freminal`, `freminal-terminal-emulator`, `freminal-buffer`,
`freminal-common`)
**Estimated scope:** 6 subtasks (13 deletions, 15 demotions, plus iterative cleanup)

---

## Audit History

The audit was initiated 2026-04-02. Four parallel read-only subagents (one per crate) enumerated
all `pub` items and classified them by checking callers across the entire workspace using ripgrep.
Results were cross-validated manually before subtask creation. The "stub" phase is complete; the
document now contains actionable subtasks.

## The Problem with `pub`

Consider:

```rust
// freminal-buffer/src/buffer.rs
pub fn some_helper(&self) -> usize { ... }
```

If `some_helper` is only called from within `freminal-buffer`, it should be `pub(crate)`. If it
is not called at all, it should be deleted. But because it is `pub`, neither `rustc` nor clippy
will warn about it — the compiler assumes an external crate _might_ use it.

This is especially problematic at crate boundaries:

- `freminal-common` exports types consumed by all downstream crates. A type or method that was
  needed by `freminal-terminal-emulator` during an earlier design phase may now be unused, but
  `pub` visibility keeps it hidden from the linter.
- `freminal-buffer` exports buffer operations consumed by `freminal-terminal-emulator`. Methods
  added for features that were later redesigned may still be `pub`.
- `freminal-terminal-emulator` exports the emulator API consumed by `freminal`. Methods that
  existed for the old `FairMutex` locking model may be orphaned.

## Audit Procedure

When the audit is initiated:

1. **Enumerate all `pub` items** across the four library crates using a combination of:
   - `cargo doc --document-private-items` to see the full API surface.
   - `grep -rn '^pub ' --include='*.rs'` for a raw list.
   - IDE "find usages" or `rg` for cross-crate reference checking.

2. **For each `pub` item, determine its caller set:**
   - **Zero callers anywhere:** Dead code. Delete it.
   - **Callers only within the same crate:** Demote to `pub(crate)`.
   - **Callers in downstream crates:** Legitimate `pub`. Keep it.
   - **Callers only in tests:** Consider whether the item should be `#[cfg(test)]`-gated or
     moved to a test helper module.

3. **Check for transitively dead trees:** A `pub` function that calls three other `pub` functions
   — if the root is dead, all three callees may also become dead once the root is removed. Audit
   iteratively until no new dead items are found.

4. **Categorize findings** into actionable subtasks grouped by crate.

5. **Write the subtask list** in this document. Update status from "Stub" to "Pending".

### Tools

- `cargo-udeps` — detects unused _dependencies_, not unused code, but useful as a cross-check.
- `cargo-machete` — already in CI; confirms no unused crate deps.
- Manual `rg` (ripgrep) queries are the primary tool: `rg 'fn some_helper' --include='*.rs'`
  to find definitions, `rg 'some_helper' --include='*.rs'` to find all references.
- `rust-analyzer` IDE features (if available) for "find all references" on a per-item basis.

### Known Likely Sources of Dead `pub` Items

These areas have undergone significant refactoring and are likely to harbor dead public APIs:

| Area                       | Refactor That May Have Orphaned Items                                  |
| -------------------------- | ---------------------------------------------------------------------- |
| `TerminalEmulator` methods | Performance plan: FairMutex elimination moved GUI state to `ViewState` |
| `TerminalState` methods    | Task 5 (GUI fields removed), Task 25 (dead `Theme`, dead `scroll()`)   |
| `Buffer` methods           | Performance plan: `scroll_offset` externalized; Task 25 code quality   |
| `TerminalHandler` methods  | Extensive escape sequence work (Tasks 7, 10, 20, 21)                   |
| `freminal-common` types    | Types added for early designs that were later redesigned               |
| Snapshot/IO types          | Performance plan introduced new types; old intermediaries may remain   |

## Audit Results

The audit was performed across all four crates (2026-04-02). Each `pub` item was checked for
callers using `rg` / `grep` across the entire workspace, including integration tests and
benchmarks.

### Classification

- **DEAD** — Zero callers anywhere (production, tests, benchmarks). Delete.
- **DEMOTE** — Callers only within the defining crate. Change `pub` → `pub(crate)`.
- **KEEP** — Callers in downstream crates, integration tests, or benchmarks. Leave as `pub`.

---

## Subtasks

### Subtask 31.1 — Delete dead items in `freminal-common`

**Action:** Delete the following items that have zero callers anywhere in the workspace.

| Item                          | File                               | Line | Evidence                         |
| ----------------------------- | ---------------------------------- | ---- | -------------------------------- |
| `display_vec_tchar_as_string` | `buffer_states/tchar.rs`           | 138  | Only definition; zero references |
| `EraseMode` enum              | `buffer_states/terminal_output.rs` | 17   | Only definition; zero references |
| `CursorDirection` enum        | `buffer_states/terminal_output.rs` | 30   | Only definition; zero references |
| `LineOperation` enum          | `buffer_states/terminal_output.rs` | 39   | Only definition; zero references |
| `CharOperation` enum          | `buffer_states/terminal_output.rs` | 46   | Only definition; zero references |

**Note:** If `terminal_output.rs` becomes empty after deletions, delete the file and remove its
`pub mod` from `buffer_states/mod.rs`.

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

---

### Subtask 31.2 — Demote `pub` → `pub(crate)` in `freminal-common`

**Action:** Change visibility for items only used within `freminal-common` itself.

| Item                     | File                                   | Line | Callers                                      |
| ------------------------ | -------------------------------------- | ---- | -------------------------------------------- |
| `TCharError` enum        | `buffer_states/error.rs`               | 10   | Only `tchar.rs` within crate                 |
| `user_config_path` fn    | `config.rs`                            | 534  | Only `config.rs` internal calls              |
| `diacritic_to_index` fn  | `buffer_states/unicode_placeholder.rs` | 69   | Only same file (production + `#[cfg(test)]`) |
| `PLACEHOLDER_CHAR` const | `buffer_states/unicode_placeholder.rs` | 23   | Only same file                               |
| `PLACEHOLDER_UTF8` const | `buffer_states/unicode_placeholder.rs` | 26   | Only same file                               |
| `SixelBackground` enum   | `buffer_states/sixel.rs`               | 61   | Only same file                               |

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

---

### Subtask 31.3 — Delete dead items in `freminal-terminal-emulator`

**Action:** Delete orphaned `TerminalEmulator` wrapper methods and `TerminalState` accessors.

| Item                                               | File                | Line | Evidence                                                        |
| -------------------------------------------------- | ------------------- | ---- | --------------------------------------------------------------- |
| `TerminalEmulator::data()`                         | `interface.rs`      | 418  | Zero callers; `build_snapshot` uses `internal.handler` directly |
| `TerminalEmulator::data_and_format_data_for_gui()` | `interface.rs`      | 430  | Zero callers; `build_snapshot` uses `internal` directly         |
| `TerminalEmulator::cursor_pos()`                   | `interface.rs`      | 439  | Zero callers; `build_snapshot` uses `internal.cursor_pos()`     |
| `TerminalEmulator::show_cursor()`                  | `interface.rs`      | 443  | Zero callers; `build_snapshot` uses `internal.show_cursor()`    |
| `TerminalEmulator::get_cursor_visual_style()`      | `interface.rs`      | 261  | Zero callers; `build_snapshot` uses `internal` directly         |
| `TerminalEmulator::skip_draw_always()`             | `interface.rs`      | 266  | Zero callers; `build_snapshot` uses `internal` directly         |
| `TerminalState::cursor_color()`                    | `state/internal.rs` | 141  | Zero callers; codebase uses `cursor_color_override` instead     |

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

---

### Subtask 31.4 — Demote `pub` → `pub(crate)` in `freminal-terminal-emulator`

**Action:** Change visibility for items only used within the crate.

| Item                                       | File                          | Line | Callers                     |
| ------------------------------------------ | ----------------------------- | ---- | --------------------------- |
| `TerminalState::cursor_pos()`              | `state/internal.rs`           | 183  | Only `interface.rs`         |
| `TerminalState::show_cursor()`             | `state/internal.rs`           | 156  | Only `interface.rs`         |
| `TerminalState::get_cursor_visual_style()` | `state/internal.rs`           | 134  | Only `interface.rs`         |
| `TerminalState::skip_draw_always()`        | `state/internal.rs`           | 161  | Only `interface.rs` + tests |
| `TerminalState::is_normal_display()`       | `state/internal.rs`           | 146  | Only `interface.rs` + tests |
| `TerminalState::get_cursor_key_mode()`     | `state/internal.rs`           | 199  | Only `interface.rs`         |
| `AnsiCsiParserState` enum                  | `ansi_components/csi.rs`      | 78   | Only within crate           |
| `AnsiOscParserState` enum                  | `ansi_components/osc.rs`      | 22   | Only within crate           |
| `StandardParserState` enum                 | `ansi_components/standard.rs` | 13   | Only within crate           |

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

---

### Subtask 31.5 — Delete dead item in `freminal` (binary)

**Action:** Delete the following item with zero callers.

| Item                | File            | Line | Evidence                         |
| ------------------- | --------------- | ---- | -------------------------------- |
| `color32_to_f32` fn | `gui/colors.rs` | 16   | Only definition; zero references |

**Verify:** `cargo test --all`, `cargo clippy --all-targets --all-features -- -D warnings`

---

### Subtask 31.6 — Final verification and iterative cleanup

After all deletions and demotions are applied:

1. Run `cargo test --all` — all tests pass.
2. Run `cargo clippy --all-targets --all-features -- -D warnings` — clean.
   - Clippy may now fire new `dead_code` warnings on items that were previously reachable
     only through the deleted items (transitive dead code). Fix any such warnings by
     deleting or demoting the newly-exposed dead items.
3. Run `cargo-machete` — no unused dependencies.
4. Iterate until no new warnings appear.

---

## Deferred / Out of Scope

The following categories were identified during the audit but are deferred:

- **`freminal-buffer` DEMOTE candidates** (~37 items including `TerminalHandler::handle_*` methods,
  `Buffer` methods, `Row` fields, `ImageStore` methods): These are internal-only items used within
  `freminal-buffer` but exposed as `pub` because `TerminalHandler` is consumed by
  `freminal-terminal-emulator` via `pub internal: TerminalState` field chains. A proper fix
  requires making `TerminalState::handler` non-public and exposing only the needed API — this is
  architectural and belongs in Task 29 (God File Refactoring).

- **`freminal` binary DEMOTE candidates** (~65 items): Since this is a binary crate, `pub` items
  are only needed for benchmarks. A blanket `pub(crate)` sweep would break the benchmark harness.
  The items that benchmarks need must stay `pub`; the rest could be demoted but the risk/reward
  ratio is low for a binary crate where `pub` has no external consumers.

- **TEST-ONLY items** in `freminal-terminal-emulator` (~52 items including parser subcomponents,
  tracer infrastructure): These are `pub` for the integration test and benchmark files that live
  in `tests/` and `benches/` (which are external to the crate). Gating them behind `#[cfg(test)]`
  would break benchmarks. A proper solution requires a `test-support` feature flag — deferred.

---

## References

- `agents.md` — Dead Code Policy: "`#[allow(dead_code)]` is forbidden in production modules"
- `Documents/MASTER_PLAN.md` — Task 31 entry
- `Documents/PLAN_25_CODE_QUALITY.md` — Previous dead code removal (Theme, scroll, StandardOutput)
- `Documents/PERFORMANCE_PLAN.md` — Section 5: "What Gets Deleted" (may have left remnants)
