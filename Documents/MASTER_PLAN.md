# MASTER_PLAN.md — Freminal Feature Roadmap

## Overview

This document orchestrates thirteen major development tasks for Freminal. Each task has a dedicated
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

| #   | Task                      | Plan Document                        | Status      | Dependencies |
| --- | ------------------------- | ------------------------------------ | ----------- | ------------ |
| 1   | Custom Terminal Renderer  | `PLAN_01_GLYPH_ATLAS.md`             | Complete    | None         |
| 2   | CLI Args + TOML Config    | `PLAN_02_CLI_CONFIG.md`              | Complete    | None         |
| 3   | Settings Modal            | `PLAN_03_SETTINGS_MODAL.md`          | Complete    | Task 2       |
| 4   | Deployment Flake          | `PLAN_04_DEPLOYMENT_FLAKE.md`        | Complete    | Task 2       |
| 5   | Font Ligatures            | `PLAN_05_FONT_LIGATURES.md`          | Complete    | Task 1       |
| 6   | Test Gap Coverage         | `PLAN_06_TEST_GAPS.md`               | Not Started | None         |
| 7   | Escape Sequence Coverage  | `PLAN_07_ESCAPE_SEQUENCES.md`        | Complete    | None         |
| 8   | Primary Screen Scrollback | `PLAN_08_SCROLLBACK.md`              | Complete    | None         |
| 9   | tmux Compat + Logging     | `PLAN_09_TMUX_COMPAT_AND_LOGGING.md` | Complete    | None         |
| 10  | vttest Cursor Movement    | `PLAN_10_VTTEST_CURSOR_MOVEMENT.md`  | Complete    | None         |
| 11  | Theming                   | `PLAN_11_THEMING.md`                 | Complete    | Tasks 2, 3   |
| 12  | Terminfo Audit            | `PLAN_12_TERMINFO.md`                | Complete    | None         |
| 13  | Image Protocol Support    | `PLAN_13_IMAGE_PROTOCOL.md`          | Complete    | Task 1       |

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

---

## Recommended Execution Order

The following reflects the actual execution state: Tasks 1, 2, 3, 7, 8, 9, and 10 are complete.
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

```text
Complete:     Tasks 1, 2, 3, 7, 8, 9, 10
              │
Phase 3:      ├── Task 5  (Font Ligatures) ────────────────────┤
              │                                                 │
Phase 4:      │                        ├── Task 11 (Theming)    ┤
              │                        ├── Task 12 (Terminfo)   ┤
              │                        ├── Task 13 (Images)     ┤
              │                                                 │
Phase 5:      │                        ├── Task 6  (Test Gaps) ──┤
              │                                                   │
Phase 6:      │                        ├── Task 4  (Deployment) ──┤
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

Tasks 2, 3, 4, and 11 all interact with the config schema:

- Task 2 defines the schema (Rust structs + TOML format)
- Task 3 reads and writes the schema (settings UI + persistence)
- Task 4 mirrors the schema (Nix attrs → TOML generation)
- Task 11 extends the schema (theme name in `[theme]` section, persisted on Apply)

Any config schema changes after Task 2 is complete must be propagated to Tasks 3, 4, and 11.

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

| Task | Started    | Completed  | Notes                                         |
| ---- | ---------- | ---------- | --------------------------------------------- |
| 1    | 2026-03-10 | 2026-03-10 | 5 commits on task-01/glyph-atlas              |
| 2    | 2026-03-09 | 2026-03-09 | 8 commits on task-02/cli-config               |
| 3    | 2026-03-10 | 2026-03-10 | Menu bar + tabbed settings modal              |
| 4    | 2026-03-15 | 2026-03-15 | All 8 subtasks on tasks/5-11-12-13-4          |
| 5    | 2026-03-12 | 2026-03-12 | All 8 subtasks complete on tasks/5-11-12-13-4 |
| 6    | —          | —          |                                               |
| 7    | 2026-03-09 | 2026-03-09 | All 30 subtasks complete                      |
| 8    | 2026-03-09 | 2026-03-09 | All 7 subtasks complete                       |
| 9    | 2026-03-11 | 2026-03-11 | 12 subtasks on task-09/tmux-compat-logging    |
| 10   | 2026-03-11 | 2026-03-11 | All subtasks complete                         |
| 11   | 2026-03-12 | 2026-03-12 | All 9 subtasks complete on tasks/5-11-12-13-4 |
| 12   | 2026-03-12 | 2026-03-12 | All 4 subtasks complete on tasks/5-11-12-13-4 |
| 13   | 2026-03-14 | 2026-03-14 | All 9 subtasks complete on tasks/5-11-12-13-4 |

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
- `config_example.toml` — Current config format
