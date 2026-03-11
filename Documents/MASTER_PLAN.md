# MASTER_PLAN.md — Freminal Feature Roadmap

## Overview

This document orchestrates eight major development tasks for Freminal. Each task has a dedicated
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

| #   | Task                      | Plan Document                 | Status      | Dependencies |
| --- | ------------------------- | ----------------------------- | ----------- | ------------ |
| 1   | Custom Terminal Renderer  | `PLAN_01_GLYPH_ATLAS.md`      | Not Started | None         |
| 2   | CLI Args + TOML Config    | `PLAN_02_CLI_CONFIG.md`       | Complete    | None         |
| 3   | Settings Modal            | `PLAN_03_SETTINGS_MODAL.md`   | Complete    | Task 2       |
| 4   | Deployment Flake          | `PLAN_04_DEPLOYMENT_FLAKE.md` | Not Started | Task 2       |
| 5   | Font Ligatures            | `PLAN_05_FONT_LIGATURES.md`   | Not Started | Task 1       |
| 6   | Test Gap Coverage         | `PLAN_06_TEST_GAPS.md`        | Not Started | None         |
| 7   | Escape Sequence Coverage  | `PLAN_07_ESCAPE_SEQUENCES.md` | Complete    | None         |
| 8   | Primary Screen Scrollback | `PLAN_08_SCROLLBACK.md`       | Complete    | None         |

---

## Dependency Graph

```text
Task 1 (Custom Terminal Renderer) ──► Task 5 (Font Ligatures)

Task 2 (CLI Args + TOML Config) ──┬──► Task 3 (Settings Modal)
                                   └──► Task 4 (Deployment Flake)

Task 6 (Test Gap Coverage) ── independent, can run any time

Task 7 (Escape Sequence Coverage) ── independent, can run any time

Task 8 (Primary Screen Scrollback) ── independent, can run any time
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

---

## Recommended Execution Order

### Phase 1 — Foundation (Parallel)

Run these four tasks in parallel since they have no dependencies on each other:

- **Task 2** — CLI Args + TOML Config (foundation for Tasks 3 & 4)
- **Task 6** — Test Gap Coverage (improves safety net for everything)
- **Task 7** — Escape Sequence Coverage (fixes bugs, improves daily-use compatibility)
- **Task 8** — Primary Screen Scrollback (wires scroll offset into snapshot pipeline)
- **Task 1** — Custom Terminal Renderer (largest task, long lead time)

### Phase 2 — Dependents (After Phase 1 completes)

These tasks depend on Phase 1 completions:

- **Task 3** — Settings Modal (requires Task 2)
- **Task 4** — Deployment Flake (requires Task 2)
- **Task 5** — Font Ligatures (requires Task 1)

Tasks 3 and 4 can run in parallel with each other. Task 5 can run in parallel with Tasks 3 and 4,
but only after Task 1 is complete.

```text
Phase 1:  ├── Task 2 (CLI/Config) ────────────┤
          ├── Task 6 (Test Gaps) ──────────────┤
          ├── Task 7 (Escape Sequences) ───────┤
          ├── Task 8 (Scrollback) ─────────────┤
          ├── Task 1 (Custom Renderer) ─────────────────────────────┤
          │                                    │                   │
Phase 2:  │                                    ├── Task 3 (Modal) ─┤
          │                                    ├── Task 4 (Flake) ─┤
          │                                                        ├── Task 5 (Ligatures) ─┤
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

Tasks 2, 3, and 4 all interact with the config schema:

- Task 2 defines the schema (Rust structs + TOML format)
- Task 3 reads and writes the schema (settings UI + persistence)
- Task 4 mirrors the schema (Nix attrs → TOML generation)

Any config schema changes after Task 2 is complete must be propagated to Tasks 3 and 4.

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

| Task | Started    | Completed  | Notes                            |
| ---- | ---------- | ---------- | -------------------------------- |
| 1    | —          | —          |                                  |
| 2    | 2026-03-09 | 2026-03-09 | 8 commits on task-02/cli-config  |
| 3    | 2026-03-10 | 2026-03-10 | Menu bar + tabbed settings modal |
| 4    | —          | —          | Unblocked (Task 2 complete)      |
| 5    | —          | —          | Blocked on Task 1                |
| 6    | —          | —          |                                  |
| 7    | 2026-03-09 | 2026-03-09 | All 30 subtasks complete         |
| 8    | 2026-03-09 | 2026-03-09 | All 7 subtasks complete          |

---

## References

- `agents.md` — Agent rules, architecture, verification suite
- `Documents/PERFORMANCE_PLAN.md` — Completed performance refactor (Tasks 1-12)
- `Documents/TODO.md` — Version roadmap
- `Documents/PLAN_07_ESCAPE_SEQUENCES.md` — Escape sequence audit and implementation plan
- `Documents/PLAN_08_SCROLLBACK.md` — Primary screen scrollback architecture and wiring
- `config_example.toml` — Current config format
