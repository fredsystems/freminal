# MASTER_PLAN.md — Freminal Feature Roadmap

## Overview

This document orchestrates twenty-five major development tasks for Freminal. Each task has a dedicated
planning document with detailed subtasks, acceptance criteria, and affected files. Agents executing
any of these tasks MUST read this document first for context on dependencies and ordering.

All tasks are governed by the rules in `agents.md`, including the mandatory testing, benchmarking,
and plan document maintenance rules.

---

## Branch & Workflow Rules

- All implementation work is done on **feature branches**, never directly on `main`.
- Branch naming: `task-NN/short-description` (e.g., `task-02/cli-config`).
- **Pre-commit hooks must NOT be skipped** — `--no-verify` is forbidden.
- Each task branch is merged to `main` via pull request after completion.
- See `agents.md` "Branch & Commit Workflow" section for full rules.

---

## Task Summary

| #   | Task                                     | Plan Document                               | Status   | Dependencies         |
| --- | ---------------------------------------- | ------------------------------------------- | -------- | -------------------- |
| 1   | Custom Terminal Renderer                 | `PLAN_01_GLYPH_ATLAS.md`                    | Complete | None                 |
| 2   | CLI Args + TOML Config                   | `PLAN_02_CLI_CONFIG.md`                     | Complete | None                 |
| 3   | Settings Modal                           | `PLAN_03_SETTINGS_MODAL.md`                 | Complete | Task 2               |
| 4   | Deployment Flake                         | `PLAN_04_DEPLOYMENT_FLAKE.md`               | Complete | Task 2               |
| 5   | Font Ligatures                           | `PLAN_05_FONT_LIGATURES.md`                 | Complete | Task 1               |
| 6   | Test Gap Coverage                        | `PLAN_06_TEST_GAPS.md`                      | Complete | None                 |
| 7   | Escape Sequence Coverage                 | `PLAN_07_ESCAPE_SEQUENCES.md`               | Complete | None                 |
| 8   | Primary Screen Scrollback                | `PLAN_08_SCROLLBACK.md`                     | Complete | None                 |
| 9   | tmux Compat + Logging                    | `PLAN_09_TMUX_COMPAT_AND_LOGGING.md`        | Complete | None                 |
| 10  | vttest Cursor Movement                   | `PLAN_10_VTTEST_CURSOR_MOVEMENT.md`         | Complete | None                 |
| 11  | Theming                                  | `PLAN_11_THEMING.md`                        | Complete | Tasks 2, 3           |
| 12  | Terminfo Audit                           | `PLAN_12_TERMINFO.md`                       | Complete | None                 |
| 13  | Image Protocol Support                   | `PLAN_13_IMAGE_PROTOCOL.md`                 | Complete | Task 1               |
| 14  | Bug Fixes: Modes/URL/Selection           | `PLAN_14_MODE_NOISE_URL_HOVER_SELECTION.md` | Complete | None                 |
| 15  | Launch program from arg                  | `PLAN_15_LAUNCH_PROGRAM_FROM_ARG.md`        | Complete | None                 |
| 16  | Github Action for building and releasing | `PLAN_16_GITHUB_ACTIONS.md`                 | Complete | None                 |
| 17  | Update readme                            | `PLAN_17_UPDATE_README.md`                  | Complete | None                 |
| 18  | Client-Side Update Mechanism             | `PLAN_18_UPDATE_MECHANISM.md`               | Pending  | Tasks 2, 3, 16       |
| 19  | Update Service & Website                 | `PLAN_19_UPDATE_SERVICE_AND_WEBSITE.md`     | Pending  | None (separate repo) |
| 20  | DEC Private Mode Coverage                | `PLAN_20_DEC_MODE_COVERAGE.md`              | Complete | None                 |
| 21  | Tab Stop Correctness                     | `PLAN_21_TAB_STOPS.md`                      | Complete | None                 |
| 22  | vttest Integration Test Suite            | `PLAN_22_VTTEST_INTEGRATION.md`             | Pending  | None                 |
| 23  | Blinking Text                            | `PLAN_23_BLINKING_TEXT.md`                  | Complete | None                 |
| 24  | Benchmark Improvements                   | `PLAN_24_BENCHMARK_IMPROVEMENTS.md`         | Pending  | None                 |
| 25  | Code Quality Refactoring                 | `PLAN_25_CODE_QUALITY.md`                   | Pending  | None                 |

---

## Dependency Graph

```text
Task 1 (Custom Terminal Renderer) ──► Task 5 (Font Ligatures)

Task 2 (CLI Args + TOML Config) ──┬──► Task 3 (Settings Modal) ──┬──► Task 11 (Theming)
                                   └──► Task 4 (Deployment Flake) │
                                                                   └──► (Task 2 also required)

Task 6 (Test Gap Coverage) ── independent, can run any time

Task 7 (Escape Sequence Coverage) ── independent, can run any time

Task 8 (Primary Screen Scrollback) ── independent, can run any time

Task 9 (tmux Compat + Logging) ── independent, can run any time

Task 10 (vttest Cursor Movement) ── independent, can run any time

Task 12 (Terminfo Audit) ── independent, can run any time

Task 1 (Custom Terminal Renderer) ──► Task 13 (Image Protocol Support)

Tasks 2, 3, 16 ──► Task 18 (Client-Side Update Mechanism)

Task 19 (Update Service & Website) ── independent (separate repo, shares API contract with Task 18)

Task 20 (DEC Private Mode Coverage) ── independent, complete

Task 21 (Tab Stop Correctness) ── independent, can run any time

Task 22 (vttest Integration Test Suite) ── independent, can run any time

Task 23 (Blinking Text) ── independent, can run any time

Task 24 (Benchmark Improvements) ── independent, can run any time

Task 25 (Code Quality Refactoring) ── independent, can run any time
```

### Dependency Details

**Task 1 → Task 5:** Task 1 replaces the entire terminal rendering pipeline with a custom OpenGL
renderer (glow shaders via egui PaintCallback), introduces rustybuzz for text shaping, swash for
glyph rasterisation (including color emoji), and builds a glyph atlas with font fallback chain.
Task 5 builds on this by enabling OpenType ligature features (`liga`, `calt`, `dlig`) in the
rustybuzz shaping calls. Task 5 cannot begin until Task 1's shaping and rendering infrastructure
is complete and merged.

**Task 2 → Task 3:** Task 3 (Settings Modal) needs a complete, well-structured config system to
display and modify. Task 2 extends the config with all CLI flags surfaced as TOML options and adds
the `--config` override path. The Settings Modal writes back to TOML, so the config schema must be
stable first.

**Task 2 → Task 4:** Task 4 (Deployment Flake) generates `config.toml` from Nix attributes. The
home-manager module must mirror the final config schema, so Task 2's config extensions must be
complete first.

**Task 6:** Independent. Can run before, during, or after any other task. Recommended to start
early since it improves safety for all subsequent work.

**Task 7:** Independent. Can run before, during, or after any other task. Addresses escape
sequence correctness — critical for basic terminal compatibility (vttest, vim, tmux, etc.).
Recommended to start early since it fixes bugs that affect daily use.

**Task 8:** Independent. Wires the user's scroll offset from the GUI into the PTY thread so
primary-screen scrollback works. The `Buffer` layer already supports offset-based rendering;
this task is purely plumbing. Medium scope (7 subtasks).

**Task 10:** Independent. Addresses vttest cursor movement test failures. Can run any time but
benefits from Task 7's escape sequence fixes being complete.

**Task 11:** Depends on Tasks 2 and 3. Adds ~25 curated embedded color themes (Catppuccin variants,
Dracula, Solarized, Nord, Gruvbox, Tokyo Night, etc.) with a theme picker in the Settings Modal.
Requires the config system (Task 2) and settings UI (Task 3) to be complete. Theme selection
persists to `config.toml` and takes effect immediately via hot-reload.

**Task 12:** Independent. Audits the `freminal.ti` terminfo source for correctness, fixes the
build.rs rerun detection bug, and audits XTGETTCAP responses. Strategy: stay with
`TERM=xterm-256color` (matching WezTerm/Alacritty) and use XTGETTCAP to advertise extra
capabilities. Low scope (4 subtasks).

**Task 13:** Depends on Task 1. Implements inline image display via OSC protocols: iTerm2 inline
images (Phase 1), Kitty graphics protocol minimal subset (Phase 2), Sixel (Phase 3, deferred).
Requires the custom OpenGL renderer from Task 1 for GPU-side image textures. Medium-high scope
(9 subtasks across 3 phases).

**Task 18:** Depends on Tasks 2, 3, and 16. Adds a client-side update mechanism: background HTTP
check against `updates.freminal.dev`, version comparison via `semver`, menu bar indicator, and a
modal dialog for downloading updates. Requires the config system (Task 2) for the `[update]`
section, the settings modal infrastructure (Task 3) as the UI pattern template, and the GitHub
Actions release pipeline (Task 16) for compressed assets and SHA256 checksums. Also extends the
deploy workflow to produce `.tar.gz`/`.zip` compressed assets and a `SHA256SUMS.txt` manifest.
Medium scope (11 subtasks).

**Task 19:** Independent of the freminal repo. Lives in a separate `freminal-updates` repository.
Implements a Cloudflare Worker at `updates.freminal.dev` that proxies the GitHub Releases API with
1-hour KV cache, plus a project website at `freminal.dev`. Shares an API contract with Task 18
(the client-side consumer). Can be developed in parallel with Task 18 as long as the API contract
(defined in both plan documents) is respected. Medium scope (10 subtasks).

**Task 20:** Independent. Completed. Comprehensive DEC private mode audit — implemented 12 modes
(`?2`, `?42`, `?66`, `?67`, `?69`, `?80`, `?1001`, `?1007`, `?1045`, `?1046`, `?1070`, `?2027`)
and promoted `?2048` and `?7727`. Includes full VT52 parser and DECLRMM left/right margins.

**Task 21:** Independent. Fixes tab stop correctness issues: tab stops lost on resize, TBC Ps=1/2/4/5
not implemented, tab stops shared (not saved) across alternate screen transitions. Low scope
(6 subtasks).

**Task 22:** Independent. Builds a golden-file integration test suite driven by vttest. Captures
terminal buffer state after feeding vttest escape sequences and compares against known-good
snapshots. Covers menus 1, 2, 6, 8, 9 and xterm extensions. Medium-high scope (8 subtasks).

**Task 23:** Independent. Implements SGR 5/6 blinking text rendering. Currently parsed but silently
discarded in `apply_sgr()`. Requires adding `BlinkState` to `FormatTag`, transporting through
snapshots, and driving a GPU-side blink timer. Medium scope (7 subtasks).

**Task 24:** Independent. Fills benchmark gaps (scrollback rendering, shaping cache-miss, alt screen
switch), fixes fragile benchmarks, adds CI compilation checks and optional weekly regression runs.
Medium scope (6 subtasks).

**Task 25:** Independent. Code quality refactoring: split `standard.rs` parser into `dcs.rs`/`apc.rs`,
standardize CSI command naming to ECMA-48 mnemonics (17 renames), split `interface.rs`, inline
`data.rs`, remove dead `scroll()` and `StandardOutput` enum, move `Theme` to `freminal-common`.
Medium scope (8 subtasks).

---

## Recommended Execution Order

The following reflects the actual execution state: Tasks 1-17 and 20 are complete.
The remaining tasks are ordered as follows.

### Phase 3 — Next Up

- **Task 5** — Font Ligatures (unblocked by Task 1)

### Phase 4 — Feature Work

Run after Task 5 completes. These are independent of each other and can run in parallel:

- **Task 11** — Theming (unblocked by Tasks 2 + 3)
- **Task 12** — Terminfo Audit (independent)
- **Task 13** — Image Protocol Support (unblocked by Task 1)

### Phase 5 — Test Coverage

- **Task 6** — Test Gap Coverage (run after feature work to maximise coverage of new code)

### Phase 6 — Packaging

- **Task 4** — Deployment Flake (run last; benefits from a stable config schema and feature set)

### Phase 7 — Update Mechanism

Run after Phase 6. Task 18 depends on Tasks 2, 3, and 16 (all complete). Task 19 is independent
and can be developed in a separate repo in parallel with Task 18.

- **Task 18** — Client-Side Update Mechanism (unblocked by Tasks 2 + 3 + 16)
- **Task 19** — Update Service & Website (independent, separate repo; shares API contract with 18)

### Phase 8 — Correctness, Quality & Testing

Independent of each other and of Phases 3-7. Can run at any time in parallel with other work.

- **Task 21** — Tab Stop Correctness (independent)
- **Task 22** — vttest Integration Test Suite (independent)
- **Task 23** — Blinking Text (independent)
- **Task 24** — Benchmark Improvements (independent)
- **Task 25** — Code Quality Refactoring (independent)

```text
Complete:     Tasks 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 20, 21
              │
Phase 7:      ├── Task 18 (Update Client) ──┤
              ├── Task 19 (Update Service)   ┤ (parallel, separate repo)
              │                              │
Phase 8:      ├── Task 21 (Tab Stops)        ┤ (any time)
              ├── Task 22 (vttest Suite)     ┤ (any time)
              ├── Task 23 (Blinking Text)    ┤ (any time)
              ├── Task 24 (Benchmarks)       ┤ (any time)
              ├── Task 25 (Code Quality)     ┤ (any time)
```

---

## Agent Assignment Guidelines

### Model Selection

- **Claude Sonnet** (`github-copilot/claude-sonnet-4.6`): Use for well-scoped subtasks with clear
  instructions. Appropriate for most implementation work within individual tasks.
- **Claude Opus**: Use for orchestration, architectural decisions, complex cross-cutting work,
  and plan document creation/updates.

### Parallelism Strategy

Within each task, subtasks may be parallelizable. Refer to individual plan documents for
intra-task parallelism guidance. Cross-task parallelism follows the dependency graph above.

---

## Cross-Cutting Concerns

### Config Schema Evolution

Tasks 2, 3, 4, 11, and 18 all interact with the config schema:

- Task 2 defines the schema (Rust structs + TOML format)
- Task 3 reads and writes the schema (settings UI + persistence)
- Task 4 mirrors the schema (Nix attrs → TOML generation)
- Task 11 extends the schema (theme name in `[theme]` section, persisted on Apply)
- Task 18 extends the schema (`[update]` section with `check_enabled` and `check_interval_hours`)

Any config schema changes after Task 2 is complete must be propagated to Tasks 3, 4, 11, and 18.

### Rendering Pipeline

Tasks 1 and 5 both modify the rendering pipeline:

- Task 1 replaces the entire rendering approach with a custom OpenGL renderer: glow shaders via
  egui `PaintCallback`, rustybuzz text shaping, swash glyph rasterisation (including color emoji),
  a glyph atlas texture, and integer-pixel cell grid with no egui layout involvement. egui
  continues to handle chrome (menu bar, settings modal) only.
- Task 5 adds ligature support to the rustybuzz shaping layer introduced by Task 1

After Task 1, the rendering pipeline will be fundamentally different — egui is not involved in
terminal text rendering at all. Any rendering-related work in other tasks (e.g., font size changes
from the Settings Modal) must account for the new architecture: font metrics come from swash (not
egui's `fonts_mut`), cell size is integer pixels, and the terminal area is drawn by custom shaders.

### Benchmark Baselines

- Task 1 will dramatically change render performance characteristics. New baselines must be
  established after Task 1 completes.
- Task 6 may add new benchmarks as part of test gap coverage. These become part of the
  permanent benchmark suite.

---

## Completion Tracking

Update this section as tasks complete:

| Task | Started    | Completed  | Notes                                            |
| ---- | ---------- | ---------- | ------------------------------------------------ |
| 1    | 2026-03-10 | 2026-03-10 | 5 commits on task-01/glyph-atlas                 |
| 2    | 2026-03-09 | 2026-03-09 | 8 commits on task-02/cli-config                  |
| 3    | 2026-03-10 | 2026-03-10 | Menu bar + tabbed settings modal                 |
| 4    | 2026-03-15 | 2026-03-15 | All 8 subtasks on tasks/5-11-12-13-4             |
| 5    | 2026-03-12 | 2026-03-12 | All 8 subtasks complete on tasks/5-11-12-13-4    |
| 6    | 2026-03-16 | 2026-03-16 | All 13 subtasks complete; 71.6%→75.8% (+4.2pp)   |
| 7    | 2026-03-09 | 2026-03-09 | All 30 subtasks complete                         |
| 8    | 2026-03-09 | 2026-03-09 | All 7 subtasks complete                          |
| 9    | 2026-03-11 | 2026-03-11 | 12 subtasks on task-09/tmux-compat-logging       |
| 10   | 2026-03-11 | 2026-03-11 | All subtasks complete                            |
| 11   | 2026-03-12 | 2026-03-12 | All 9 subtasks complete on tasks/5-11-12-13-4    |
| 12   | 2026-03-12 | 2026-03-12 | All 4 subtasks complete on tasks/5-11-12-13-4    |
| 13   | 2026-03-14 | 2026-03-14 | All 9 subtasks complete on tasks/5-11-12-13-4    |
| 14   | 2026-03-15 | 2026-03-15 | Mode noise, URL hover, scrollback selection      |
| 15   | 2026-03-16 | 2026-03-16 | All 6 subtasks complete on tasks/15-16-17        |
| 16   | 2026-03-16 | 2026-03-16 | All 4 subtasks complete on tasks/15-16-17        |
| 17   | 2026-03-16 | 2026-03-16 | All 3 subtasks complete on tasks/15-16-17        |
| 18   |            |            |                                                  |
| 19   |            |            |                                                  |
| 20   | 2026-03-17 | 2026-03-17 | All 12 subtasks on task-20/dec-mode-coverage     |
| 21   | 2026-03-31 | 2026-03-31 | All 6 subtasks on task-21/tab-stops              |
| 22   |            |            |                                                  |
| 23   | 2026-04-01 | 2026-04-01 | All 7 subtasks complete on task-23/blinking-text |
| 24   |            |            |                                                  |
| 25   |            |            |                                                  |

---

## References

- `agents.md` — Agent rules, architecture, verification suite
- `Documents/PERFORMANCE_PLAN.md` — Completed performance refactor (Tasks 1-12)
- `Documents/TODO.md` — Version roadmap
- `Documents/PLAN_07_ESCAPE_SEQUENCES.md` — Escape sequence audit and implementation plan
- `Documents/PLAN_08_SCROLLBACK.md` — Primary screen scrollback architecture and wiring
- `Documents/PLAN_09_TMUX_COMPAT_AND_LOGGING.md` — tmux compatibility fixes and persistent logging
- `Documents/PLAN_10_VTTEST_CURSOR_MOVEMENT.md` — vttest cursor movement test failures
- `Documents/PLAN_11_THEMING.md` — Embedded color themes and theme picker
- `Documents/PLAN_12_TERMINFO.md` — Terminfo audit, build.rs fix, XTGETTCAP audit
- `Documents/PLAN_13_IMAGE_PROTOCOL.md` — Image protocol support (iTerm2, Kitty, Sixel)
- `Documents/PLAN_14_MODE_NOISE_URL_HOVER_SELECTION.md` — Mode noise, URL hover, scrollback selection
- `Documents/PLAN_18_UPDATE_MECHANISM.md` — Client-side update mechanism
- `Documents/PLAN_19_UPDATE_SERVICE_AND_WEBSITE.md` — Update service and website (separate repo)
- `Documents/PLAN_20_DEC_MODE_COVERAGE.md` — DEC private mode audit and implementation
- `Documents/PLAN_21_TAB_STOPS.md` — Tab stop correctness (resize, TBC variants, alt screen)
- `Documents/PLAN_22_VTTEST_INTEGRATION.md` — vttest golden-file integration test suite
- `Documents/PLAN_23_BLINKING_TEXT.md` — SGR 5/6 blinking text rendering
- `Documents/PLAN_24_BENCHMARK_IMPROVEMENTS.md` — Benchmark gaps, CI integration, fragile fixes
- `Documents/PLAN_25_CODE_QUALITY.md` — Parser split, CSI renames, dead code, doc comments
- `config_example.toml` — Current config format
