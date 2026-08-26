# PROFILING.md — measuring freminal's CPU cost

**Status:** Authoritative reference. Update this file whenever the profiling
harness, build profile, or tooling changes.

## Why this file exists

The workspace `Cargo.toml`'s `[profile.profiling]` comment used to say "See
CONTRIBUTING / the profiling notes for the btop idle-CPU investigation
(issue #405)". Neither existed — there is no `CONTRIBUTING.md` in this repo and
there were no profiling notes anywhere. This document is that reference
(Task 121 subtask 121.23), and the `Cargo.toml` comment now points here.

It exists as the **fallback for when a performance finding is disputed**. Per
`DECOUPLING_FRAMEWORK.md` §8 subtask 0.3, the in-app harness proved sufficient to
root-cause all three Phase 0 findings and the `perf` cross-validation was never
needed. That does not retire the methodology; it means Tier 1 below is almost
always the right starting point and Tier 2 is held in reserve.

## Which tool answers which question

| Question | Tool |
| --- | --- |
| How many frames are we drawing, and why? | Tier 1 harness |
| Where does a frame's wall-clock time go? | Tier 1 harness |
| Is the chrome cache (`ChromeMode::Replay`) actually engaging? | Tier 1 harness |
| Which suppression veto is firing, and how often? | Tier 1 harness |
| Which *function* is hot, including inside dependencies? | Tier 2 `perf` |
| Did this change make a specific operation slower? | Tier 3 Criterion |
| Did this change alter what is *drawn*? | **Nothing — see "What this cannot measure"** |

## Tier 1 — the in-app `frame-profiling` harness

The primary tool. Feature-gated and **not** enabled by default, deliberately: a
default build must be byte-identical to one with the feature absent from the
crate graph, because `Instant` calls in the frame path would perturb the thing
being measured.

### Running it

```sh
cargo run --release --features frame-profiling
```

The `freminal` crate's `frame-profiling` feature forwards to
`freminal-windowing/frame-profiling`, so one flag turns on both crates'
instrumentation.

Output goes to `tracing` at **`debug` level** under two explicit targets, which
are below the default `info` threshold. To see the harness output and nothing
else:

```sh
RUST_LOG=none,freminal::frame_profiling=debug,freminal_windowing::frame_profiling=debug \
  cargo run --release --features frame-profiling
```

Note that `RUST_LOG` directives match the string passed to `tracing`'s
`target:`, not the source module path — the two targets above are set
explicitly and are not module paths.

Log lines also reach the rolling file appender (`$XDG_STATE_HOME/freminal/` on
Linux, `~/Library/Logs/Freminal/` on macOS, `%LOCALAPPDATA%\Freminal\logs\` on
Windows), whose level is set by `[logging] level` in `config.toml` and defaults
to `"info"` — so raise it to `"debug"` if you want the harness output captured
to disk rather than read from stdout.

### What you get

Two independent summary lines per window, each flushed **every 120 drawn
frames** (`FrameStats::FLUSH_EVERY` / `FrameProfile::FLUSH_EVERY`, kept equal so
the two are easy to correlate by eye). This is a frame count, not a wall-clock
timer.

The phase nesting is `run_frame` wraps `run_ui` wraps `App::update` wraps
`central_body` wraps per-pane `show()`.

**`freminal_windowing::frame_profiling`** — the windowing-owned split:

| Field | Meaning |
| --- | --- |
| `phase_total_*` | whole `run_frame` |
| `run_ui_*` | egui's `run_ui`, which contains `App::update` |
| `tessellate_*`, `paint_*`, `swap_*` | tessellation, GL paint, buffer swap |
| `chrome_mode_full` / `chrome_mode_replay` | chrome-cache duty cycle |
| `chrome_replay_duty_cycle_pct` | the same as a percentage |
| `gate_blocked_*` | which of the four gate predicates denied `Replay` (non-exclusive) |
| `settle_*` | the delay values behind a `repaint_settled` failure |
| `repaint_cause_top8` | egui's own `RepaintCause` list, most frequent first |
| `pointer_frames_scheduled` / `pointer_frames_suppressed` | pointer-motion suppression rate |

**`freminal::frame_profiling`** — the app-owned split:

| Field | Meaning |
| --- | --- |
| `phase_app_update_*` | all of `App::update` |
| `phase_panes_*` | summed per-pane `show()` |
| `phase_orchestration_*` | `central_body` minus `phase_panes` |
| `frame_damage_full` / `frame_damage_partial` | final, post-composition damage |
| `zero_change_presented` | frames presented with no pixel change |
| `chrome_signals_fired` | which chrome-damage signals fired |
| `pointer_repaint_conditions_fired` | **which condition FORCED a repaint** (Task 124.3b, ten counters: `first_motion`, `focus_change_pending`, `chrome_interactive`, `overlay_open`, `pointer_pane_unresolved`, `unknown_geometry`, `url_forced`, `gutter_forced`, `scrollbar_forced`, `selection_forced` — each counted only when it actually forced, not merely when the underlying observation was true) |

That last row is usually the one you want when asking "why isn't pointer
suppression engaging?"

### Reading it correctly

- **Difference successive flushes.** The fields are cumulative, so the first
  flush includes warm-up. `DECOUPLING_FRAMEWORK.md` §2's numbers are a
  steady-state 120-frame interval differenced between flushes.
- **Use a release build.** A `dev` build's frame costs are not meaningful.
- **The two frame counters drift.** `frame_counter` increments once per
  `run_frame`; `frames_drawn` skips three `App::update` early-return paths
  (settings window, dead-window cleanup, no-active-pane). Once any of those
  fires for a window, the two permanently diverge for that `window_id`. Do not
  assume they are comparable across a long session.
- The feature is **instrumentation only** — every gated block reads
  already-computed values. Note this is *not* true of the Task 121 pointer-motion
  suppression itself, which is always compiled in and changes scheduling for
  every build; `frame-profiling` gates only its diagnostics.

## Tier 2 — `perf` and flamegraphs

Use when Tier 1 says "time is going somewhere in this phase" but not which
function — in particular when the cost is inside a dependency freminal cannot
instrument (the `unicode-properties` cost inside `rustybuzz` found by issue #459
is the canonical example).

### Build

```sh
cargo build --profile profiling
```

Binary lands at `target/profiling/freminal`. The profile inherits `release` and
sets `debug = "full"` and `strip = "none"` so DWARF unwinding produces
symbol-rich, inline-aware stacks. There is no explicit `[profile.release]` in
this workspace, so `lto` and `codegen-units` are Cargo's implicit release
defaults (`lto = false`, `codegen-units = 16`) — inlining is therefore not as
aggressive as a fully-optimised release, which is deliberate for readability.

### Record, then report

**These are two commands, not one.** `perf record` does **not** accept
`--no-inline`; only `perf report` and `perf script` do. (Earlier plan documents
stated a single `perf record --call-graph dwarf,65528 --no-inline` invocation,
which fails.)

```sh
perf record --call-graph dwarf,65528 -- ./target/profiling/freminal
perf report --no-inline
```

Both flags are load-bearing:

- **`dwarf,65528`** sets the per-sample user stack snapshot size. The default
  (8192 bytes) is far too small for freminal's call depth, and `perf` truncates
  silently — you get a flamegraph that looks fine and is a **false negative**,
  with the hot leaf frames simply missing.
- **`--no-inline`** on the *report* step. Inline-frame resolution against a
  full-debuginfo binary this size is pathologically slow; without it `perf
  report` can appear to hang.

For a flamegraph, `cargo-flamegraph` is in the dev shell and takes the same
profile:

```sh
cargo flamegraph --profile profiling
```

Pass record options after `--`; the same `dwarf,65528` caveat applies.

### Permissions

If `perf record` reports a permission error, `kernel.perf_event_paranoid` needs
lowering for the session. This is a host configuration matter, not a repo one.

### Tooling

`perf`, `cargo-flamegraph` and `cargo-profiler` are already in the **`default`**
dev shell — no `flake.nix` change is required. They are deliberately excluded
from the `ci` shell. `perf` is Linux-only (gated on `stdenv.isLinux`); Tier 2 is
therefore a Linux-only workflow. `hotspot` is **not** present; add it to
`flake.nix` per `flake-dev-shell-discipline` if you want a GUI, rather than
installing it out of band.

## Tier 3 — Criterion benchmarks

Use for "did this specific operation regress?". The catalog mapping code areas
to bench files and benchmark IDs is the `freminal-bench-table` skill; the
before/after procedure and the 15% regression threshold are in the
`performance-benchmarks` skill. Bench targets live in `freminal-buffer`,
`freminal`, and `freminal-terminal-emulator`.

```sh
cargo bench --no-run --all    # compile only (part of `cargo xtask ci`)
cargo bench --all             # run
```

CI runs benches on a **weekly schedule and on manual dispatch only**, not on
push or PR (`.github/workflows/bench.yml`), saving a Criterion baseline named
`current`. The regression threshold there is enforced by **human review of the
Criterion output, not an automated gate**, because runner noise makes automated
gating unreliable. Do not assume a green PR means benchmarks were checked.

There is no `cargo xtask` profiling subcommand.

## Reporting discipline

**Check the rasteriser before you record a single number.** The `gl-pixel`
dev shell deliberately sets `LIBGL_ALWAYS_SOFTWARE=1` (plus
`LIBGL_DRIVERS_PATH` and `__EGL_VENDOR_LIBRARY_FILENAMES`) so the Phase 2
pixel harness runs deterministically on Mesa's llvmpipe. Those variables are
inherited by **every process launched from that shell**, freminal included,
and they move idle CPU by roughly **100x** — measured at 0.10% of a core on
the GPU against 11.3% on llvmpipe, same binary, same content. A run on
llvmpipe is not a slower version of the real thing; it is a different
program's cost profile, dominated by 32 `llvmpipe-*` rasteriser threads that
have nothing to do with the code under test.

Before measuring:

```bash
echo "${LIBGL_ALWAYS_SOFTWARE:-<unset>}"   # must print <unset>
```

If it prints `1`, you are in `gl-pixel` (or a stale shell predating the
123.C2 fix). Re-enter `nix develop` / `direnv reload`, or launch with
`env -u LIBGL_ALWAYS_SOFTWARE`.

That environment-variable check is only a quick guard, not confirmation --
it says what freminal *asked for*, not what it *got*. The actual
confirmation is the startup log line every `freminal` process emits once
its GL context is current (`freminal-windowing`'s `GlState::new`):

```text
Active OpenGL renderer: <string>
```

Inspect that exact line (`RUST_LOG` at `info` or above reaches it, and it
also lands in the rolling log file described under Tier 1 above). Reject
the run as software-rendered if `<string>` contains `llvmpipe`, `softpipe`,
or `swrast` -- Mesa's three CPU rasterisers -- and re-check the environment
above. Do not infer the rasteriser from `/proc` thread names; the log line
is the authoritative source and needs no additional tooling beyond what is
already in the dev shells.

This is not a hypothetical footgun. It shipped in the `default` shell from
`2d917ffc` until 2026-08-23, was reported as a product CPU regression, and
cost a full bisect plus a session of invalidated agent measurements before
anyone looked at the environment. Worse, it is invisible to A/B comparison:
both endpoints are equally affected, so the ~12% floor simply swamps
whatever is under test. See 123.C2 in
`PLAN_123_GL_MEASUREMENT_HARNESS.md`.

**Always report frame rate and per-frame cost as a pair, never a single CPU
number.** Total CPU is the product of the two, so a single figure cannot
distinguish "we draw fewer frames" from "each frame is cheaper" — and work on
one axis will otherwise appear to validate or invalidate work on the other. This
matters concretely: reducing per-frame cost can mask a scheduling regression,
and vice versa.

**A CPU meter cannot measure anything bursty.** This is not hypothetical. Pointer
motion over a pane containing one hyperlink sustains 61 fps and ~1.1% of a core,
against ~0.06% for a clean pane — roughly 20×. Both read as "0.1–0.2%, spiky" on
btop, because human mouse movement lasts a few hundred milliseconds and the
meter averages over one to two seconds. The counters resolved in one run what
informal observation had read as "no difference" across several sessions. If the
workload is intermittent, use `pointer_frames_scheduled` /
`pointer_frames_suppressed` and `pointer_repaint_conditions_fired`, and derive
frame rate from the interval between two flush timestamps at 120 frames each.

**Difference the flushes before quoting a per-frame cost.** In that same run the
cumulative mean read 388 µs/frame while the differenced steady-state window was
185 µs — the cumulative figure was still carrying warm-up.

`DECOUPLING_FRAMEWORK.md` §2 and §2A are the **source of truth** for the Phase 0
measurements, the three findings, and the known gaps in the Finding 3 spike. Do
not re-derive those numbers and do not restate them more strongly than §2A does.

## What this cannot measure

- **Pixels.** There is no headless-GL or pixel-readback harness — the "436.9
  pixel harness" never landed (issue #440, tracked as subtask 121.28).
  Everything in Task 121 is validated by counters, `perf` samples and human
  observation. **A regression that changes what is drawn, rather than how often,
  is undetectable in CI.** This is the single biggest gap in the methodology, and
  it is why changes to the render path should prefer differential testing
  (new fast path versus the existing path, asserted equal) over appearance
  checks wherever that is possible.
- **Typing.** Still unmeasured (subtask 121.25).
- **Sustained-motion cost under btop.** A pre-124.3b capture found the
  then-current pane-wide `mouse_tracking_active` veto firing on 216 of 217
  checks, but that capture averaged ~8 pointer events/s, so it was not a
  sustained-motion measurement even before the term it named was removed.
  `mouse_tracking_active` is no longer a repaint-forcing term at all (Task
  124.3b: PTY mouse-tracking report delivery is independent of repaint
  scheduling, see 124.3a) — a fresh capture citing it would be measuring
  something that no longer exists.

The pre-124.3b pane-wide pointer-motion vetoes (`has_urls`, a nonzero
scroll offset, `mouse_tracking_active`, the gutter strip) were measured in
subtask 121.17/124.13 (`PLAN_121_PERF_REMEDIATION.md` and
`PLAN_124_RENDER_EFFICIENCY.md`). Task 124.3b replaced the `has_urls` and
scroll-offset vetoes with the cell-granular positional terms named in the
table row above; a fresh capture pairing `pointer_repaint_conditions_fired`
with the observed event rate is the correct way to characterize the
post-124.3b suppression rate, not a reuse of the pre-124.3b numbers.
