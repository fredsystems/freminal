# PLAN_VERSION_080.md — v0.8.0 "Correctness & Polish"

## Goal

Before adding a single new feature, close every correctness and hygiene gap identified in the
post-v0.7.0 senior-engineer audit, and land every user-visible polish item from the UX audit
Top-20. No new thrusts begin until this version is shipped.

This version is explicitly _not_ about new features. It is about making sure the foundation
laid by v0.2.0–v0.7.0 is actually as solid as the `MASTER_PLAN.md` status columns claim it is,
and that no advertised feature silently does nothing.

---

## Task Summary

| #   | Feature                          | Scope  | Status  | Dependencies |
| --- | -------------------------------- | ------ | ------- | ------------ |
| 70  | Code Correctness & Hygiene Sweep | Large  | Pending | None         |
| 71  | UX Completeness & Polish Sweep   | Medium | Pending | None         |

Both tasks are independent and may be executed in parallel across sub-agents.

---

## Task 70 — Code Correctness & Hygiene Sweep

### 70 Overview

The post-v0.7.0 audit identified a series of `agents.md` rule violations and latent correctness
issues that were masked by previous "complete" status entries in `MASTER_PLAN.md`. This task
closes all of them.

Task 70 is organized by severity. All subtasks must be completed before Task 70 is considered
done — no subset deferral.

### 70 Subtasks

#### 70.A — Immediate Correctness Fix

- **70.A.1** — Fix codepoint truncation bug at
  `freminal-terminal-emulator/src/input.rs:511`. The expression `codepoint as u8` silently
  masks any non-ASCII character. Replace with explicit UTF-8 encoding. Add a regression test
  covering a non-ASCII keybinding / character input.

#### 70.B — CRITICAL: `anyhow` in Library Crates

`agents.md` "Error Handling" rule is explicit: `anyhow` is forbidden in `freminal-common`,
`freminal-buffer`, and `freminal-terminal-emulator`. Current violations span 10 files.

- **70.B.1** — Design typed error enums per module. At minimum:
  - `freminal-common`: `SgrParseError`, `ColorParseError`, `TcharError`,
    `WindowManipulationError`, `OscParseError`.
  - `freminal-terminal-emulator`: `AnsiParseError`, `InterfaceError`, `PtyError`,
    `InternalStateError`, `OscHandlerError`.
- **70.B.2** — Replace `anyhow::Result` and `anyhow::anyhow!` / `anyhow::bail!` call sites in
  all 10 files. Preserve error chains via `#[source]`.
- **70.B.3** — Move `anyhow` from `[dependencies]` to `[dev-dependencies]` in the three
  library crates. `freminal` (binary) and `xtask` retain it.
- **70.B.4** — Run full verification suite: `cargo test --all`, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo-machete`.

#### 70.C — CRITICAL: Relocate `TerminalHandler` to the Correct Crate

`freminal-buffer/src/terminal_handler/` currently contains escape-sequence parsing, mode state
machines, Kitty / iTerm2 / Sixel graphics protocols, DCS/APC parsing, shell integration, and
PTY write paths. This violates the `freminal-buffer` contract ("pure data model, no terminal
semantics"). The 5,741-line integration test file is similarly misplaced.

- **70.C.1** — Move the entire `freminal-buffer/src/terminal_handler/` subtree into
  `freminal-terminal-emulator/src/terminal_handler/`. No logic changes.
- **70.C.2** — Move `freminal-buffer/tests/terminal_handler_integration.rs` into
  `freminal-terminal-emulator/tests/`.
- **70.C.3** — Update all imports across the workspace.
- **70.C.4** — Verify `freminal-buffer` no longer depends on anything that expresses terminal
  semantics. Update `freminal-buffer/Cargo.toml` dependency list if dependencies were only
  needed by the relocated code.
- **70.C.5** — Run full verification suite; run all benchmarks to confirm zero perf regression
  (pure code movement).

#### 70.D — HIGH: Eliminate Production Panic Sites ✅ COMPLETE (2026-04-22)

`agents.md` forbids `unwrap`/`expect` and requires panics never to enforce invariants. All
surviving production panic sites must become typed errors.

- **70.D.1** ✅ — `freminal/src/gui/tabs.rs` — `active_pane()` / `active_pane_mut()` now return
  `Option<&Pane>` / `Option<&mut Pane>`; 27 non-test + 38 test callers updated across
  `tabs.rs`, `actions.rs`, `menu.rs`, `mod.rs`.
- **70.D.2** ✅ — `osc.rs:122` — `unreachable!()` replaced with `ParserOutcome::Invalid`.
- **70.D.3** ✅ — `csi.rs:184` — `unreachable!()` replaced with `ParserOutcome::Invalid`.
- **70.D.4** ✅ — `font_manager.rs` — introduced typed `FontManagerError` (thiserror) with
  variants `BundledFontCorrupt`, `ReparseFailed`, `FontRefUnavailable`. All 8 `unreachable!()`
  sites eliminated. `FontManager::new/rebuild/set_font_size/update_pixels_per_point` now
  return `Result`. Cascade through `FreminalTerminalWidget::new`. Runtime-path methods
  (`sync_pixels_per_point`, `apply_config_changes{,_no_ctx}`, `apply_font_zoom`) log+exit(1)
  on error. Unused `impl Default for FontManager` removed. Introduced private `CellMetrics`
  struct to avoid type_complexity lint.
- **70.D.5** ✅ — `gl_context.rs:176` — prime-and-fold pattern + log+exit(1).

#### 70.E — HIGH: Typed Errors for GPU Renderer

`freminal/src/gui/renderer/gpu.rs` currently returns `Result<(), String>` across 12 functions
with 22 `.map_err(|e| format!(...))` call sites.

- **70.E.1** ✅ — Introduced `GpuInitError`, `ShaderCompileError`, `TextureUploadError`,
  `BufferAllocError` enums in new `freminal/src/gui/renderer/errors.rs`. `GpuInitError`
  flattens the three sub-errors via `#[from]`. `TextureUploadError::ImageDecode` uses
  `#[source]` to chain the underlying `image::ImageError`. `Display` impls preserve the
  original string messages byte-for-byte so `error!("... {e}")` log output is unchanged.
- **70.E.2** ✅ — Converted all 12 functions and 22 call sites in `gpu.rs`. `init`,
  `init_bg_inst_pass`, `init_deco_pass`, `init_fg_pass`, `init_atlas_texture`,
  `init_image_pass`, `init_bg_image_pass`, `update_background_image`,
  `WindowPostRenderer::init`, `WindowPostRenderer::update_shader` now return
  `Result<(), GpuInitError>`. `compile_program` and `compile_shader` return
  `Result<_, ShaderCompileError>`. `label` parameter tightened to `&'static str`
  (all callers were literals). External callers in `widget.rs` and `mod.rs` unchanged
  (they use `{e}` Display).
- **70.E.3** — Surface shader compile errors to the user (see Task 71 item 4) — deferred
  to Task 71.

#### 70.F — HIGH: Thread Hygiene ✅

- **70.F.1** ✅ — Every spawned thread now uses
  `std::thread::Builder::new().name(...)`. Conventions adopted:
  - `freminal-pty-read-<pane_id>` (was raw `thread::spawn`)
  - `freminal-pty-write-<pane_id>` (was raw `thread::spawn`)
  - `freminal-child-watcher-<pane_id>` (new; previously raw spawn)
  - `freminal-pty-consumer-<pane_id>` (was raw `thread::spawn` in `gui/pty.rs`)
  - `freminal-recording-writer` (renamed from `frec-writer`)
  - `freminal-open-url` (two sites in `widget.rs`, was raw `thread::spawn`)
  - `freminal-win-proc-waiter` (Windows-only waiter in `portable-pty`)

  **Deviation from original plan:** the plan called for
  `<tab_id>-<pane_id>` pairs, but `tab_id` is not reachable from the
  terminal emulator crate (which owns the PTY threads) — only `pane_id`
  is. Since `pane_id` is already globally unique across the process
  (assigned by the recording subsystem), we use `pane_id` alone. The
  `input-pump-<window_id>` and `emulator-<window_id>` conventions from
  the plan sketch did not correspond to actual threads in the current
  codebase. `PtyInitError::Spawn(String)` is used to propagate
  `thread::Builder::spawn` failures where the caller can handle them;
  fire-and-forget spawns (open-url, PTY consumer, Windows waiter) log
  the failure and continue.

- **70.F.2** ✅ — Existing doc-comments above each spawn already describe
  thread ownership and channel endpoints (see `pty.rs` child-watcher
  block at 379–383, reader block at 401–424, writer block at 443–446,
  and the PTY consumer doc at `gui/pty.rs` 181–190). No additional
  documentation was required.

#### 70.G — DEFERRED: Bounded Channels

**Status:** deferred out of v0.8.0. The original framing of this subtask
assumed unbounded channels were an unqualified memory-safety risk.
A per-channel audit during 70.F showed the picture is more nuanced:

- `pty_read_rx` is the only high-volume channel, and it is already
  backpressured by the OS PTY pipe buffer. Bounding it with `block`
  duplicates kernel behavior; bounding it with `drop` would corrupt
  terminal output (lost bytes mid escape sequence). Neither is
  desirable.
- `input_rx` and `window_cmd_rx` are low-volume GUI→PTY and PTY→GUI
  channels. Bounding is safe but the realistic queue depth is in the
  single digits; the correctness and perf risk of a bad bound outweighs
  the speculative memory benefit.
- The recording writer channel is the only place where drop-on-overflow
  is semantically acceptable (recording is diagnostic). This is worth
  bounding eventually but is not a v0.8.0 correctness gate.

Before taking action we want real measurements of channel high-water
depths across a fast Linux box, a constrained laptop, macOS, and
Windows, under realistic workloads (`cat large_file`, `yes`,
`find /`, recording on/off). Without that data any chosen bound is a
guess.

**Re-open criteria:** observed OOM or unbounded growth in production,
OR the measurement pass above is completed and shows a real need.
Until then, the existing unbounded channels are correct.

Original subtasks preserved for reference (not to be executed now):

- ~~70.G.1 — Replace unbounded channels with `bounded(N)` per endpoint.~~
- ~~70.G.2 — Choose block vs drop-with-counter per channel.~~
- ~~70.G.3 — Saturation stress test.~~

#### 70.H — MEDIUM: Complete Cast Audit (Task 30 re-open)

Task 30 is marked complete in `MASTER_PLAN.md`, but ~190 raw `as` casts remain in production
code and ~33 allow-attributes (`#[allow(clippy::cast_*)]`) survive.

Executed as a file-by-file sweep so each commit is reviewable in isolation:

- **70.H.a** ✅ — `freminal-buffer/src/buffer.rs`. Audit revealed only one production cast
  site (`move_cursor_relative`, 9 casts behind a single `#[allow(...)]`). Extracted helper
  `clamped_offset(base, delta, lo, hi)` using `conv2::ValueFrom` for both `usize → i32` and
  `i32 → usize` conversions. Fallback on overflow: no-op (cursor does not move). Added
  `bench_move_cursor_relative` benchmark to the buffer bench suite. Benchmark result:
  24.2 µs → 25.0 µs on 10 000 iterations (+3.4%, ≈ +0.1 ns/call), well under the 15%
  regression threshold. Removed one `#[allow(clippy::cast_*)]` attribute. Remaining casts
  in this file are all in `#[cfg(test)]` modules and left as-is.
- **70.H.b** ✅ — `freminal-terminal-emulator/src/input.rs`. Audit showed only 3 cast sites
  in production: 22 `b'X' as u32` lossless byte-to-codepoint casts in the const fn
  `us_qwerty_shifted`, one `codepoint as u8` guarded truncation in `build_csi_u`, and one
  `ch as u32` in the text-field codepoint serialisation. The `as u32` casts in the const fn
  are kept per the workspace policy exception for trivially-lossless `u8 → u32` casts
  (`u32::from` is not yet const-stable; tracking rust-lang/rust#143874); the `codepoint as u8`
  site was replaced with `u8::try_from(codepoint).ok().and_then(...)` and the `ch as u32`
  site was replaced with `u32::from(ch)`. Removed the one `#[allow(clippy::cast_possible_truncation)]`
  attribute. No benchmark required — input encoding is not on the hot path (one call per
  keystroke).
- **70.H.c** ✅ — Renderer cluster: `freminal/src/gui/renderer/vertex.rs` (4 production
  casts in `extract_atlas_rect`; remaining 8 apparent hits were all in `#[cfg(test)]` or
  in format-string literals, not real casts), `freminal/src/gui/atlas.rs` (13 production
  casts across `new`, `blit_glyph`, `evict_shelf`, and `try_grow`), `freminal/src/gui/renderer/gpu.rs`
  (0 production casts — all grep hits were in `error!` format strings). Introduced a
  local `usize_from_u32` helper in `atlas.rs` (and an inline closure in `vertex.rs`) using
  `conv2::ValueFrom` with a `0` fallback and `saturating_mul` for products. Graceful
  degradation: on hypothetical 32-bit hosts where an atlas coordinate exceeds `usize::MAX`,
  the bounds-checked slice access in each call site silently declines to blit/copy — no
  panic path. No benchmark: on 64-bit targets the generated code is identical to the
  previous `as` casts (`value_from` + `unwrap_or(0)` folds to a no-op), and atlas blit
  is not exercised by `render_loop_bench.rs` (glyphs are rasterised once on cache miss).
  The 3 `#[allow(clippy::cast_precision_loss)]` attributes in this cluster all sit in the
  `#[cfg(test)]` module and are left in place per the workspace test-code exception.
- **70.H.d** ✅ — GUI shaping cluster. Audit revealed most apparent cast counts were inflated
  by test-module hits (boundary at line 688 in `shaping.rs`, 818 in `view_state.rs`, 156 in
  `coords.rs`, 1320 in `font_manager.rs`, 2083 in `widget.rs`, 3154 in `mod.rs`, 213 in
  `colors.rs`) and by `u8 as f32` hits that qualify for the type-system-lossless exception
  (`colors.rs:21-23`). Actual production casts fixed:
  - `freminal/src/gui/shaping.rs`: 2 `u8 as usize` (replaced with `usize::from(*len)`), 2
    `u32 as usize` (replaced with `usize::value_from(...).unwrap_or(0)` on rustybuzz cluster
    byte-offsets).
  - `freminal/src/gui/terminal/input.rs`: 8 `usize as u32` truncations for recording-event
    pixel and row/col coords (replaced with `u32::try_from(x).unwrap_or(u32::MAX)`; removed
    3 `#[allow(clippy::cast_possible_truncation)]` attrs).
  - `freminal/src/gui/font_manager.rs`: 2 `u32 as usize` on `fontdb` face indices (replaced
    with `usize::value_from(...).unwrap_or(0)`).
  - `freminal/src/gui/terminal/widget.rs`: 1 pointer-to-usize cast for Arc identity
    comparison (replaced with `Arc::as_ptr(...).addr()`, stable since Rust 1.84).
  - `freminal/src/gui/mod.rs`: 2 `u64 as u32` truncations on `PaneId::raw()` for recording
    events (replaced with `u32::try_from(...).unwrap_or(u32::MAX)`; removed 2
    `#[allow(clippy::cast_possible_truncation)]` attrs).

  Zero production casts in `view_state.rs`, `coords.rs`, `colors.rs`, or `renderer/gpu.rs`.
  Remaining 8 `#[allow(clippy::cast_precision_loss)]` attrs in `shaping.rs` (lines 798-1127)
  and 1 in `font_manager.rs` (line 1400) all sit in `#[cfg(test)]` modules and are left in
  place per the workspace test-code exception.

  Benchmark (`cargo bench -p freminal --bench render_loop_bench shaping_ligatures`):
  `shape_visible_cache_hit`: **15.4% faster** (6.90 µs → 5.84 µs). The `usize::value_from` +
  `unwrap_or(0)` pattern lowers to the same machine code as `as` on 64-bit; the improvement
  is measurement noise from the smaller helper invocations but easily within the ±15% policy
  band in the favorable direction. No regression.

- **70.H.e** ✅ — Emulator + common tail. Audit of 8 files (same distinguish-prod-vs-test
  pattern as 70.H.d) showed most cast counts were partially inflated by test-module sites.
  Actual production casts touched: `graphics_kitty.rs` (8), `tchar.rs` (8), `recording.rs` (2;
  the other two already have documented `#[allow]` for u128→u64 Duration micros and
  `#[repr(u8)]` enum discriminant), `terminal_handler/mod.rs` (4 production; the 2 in test
  module were skipped, and one at line 2165 was the `u32→usize` for `KittyKeyboardPop`),
  `base64.rs` (4), `fonts.rs` (2), `egui_integration.rs` (1). `sixel.rs` had 2 allows in
  `const fn` helpers (documented policy exceptions) — left as-is.
  Conversion patterns used: `usize::from(u8)` for `*len as usize` (lossless by type),
  `u8::try_from(...).unwrap_or(0)` where a runtime invariant bounds the range,
  `char::from(u8)` for `*c as char`, `usize::value_from(u32).unwrap_or(0)` for u32→usize
  reads, `saturating_mul` for Kitty image `w * h * N` on 32-bit wrap protection, and
  `approx_as::<f32>().unwrap_or(1.0)` for the `f64→f32` scale factor.
  Benchmarks (baseline captured via git-stash of this subtask's changes):

  | Benchmark                                      | Before    | After     | Change  |
  | ---------------------------------------------- | --------- | --------- | ------- |
  | bench_handle_incoming_data                     | 138.52 µs | 137.04 µs | −0.67%  |
  | bench_parse_bursty                             | 24.03 µs  | 23.42 µs  | −3.31%  |
  | bench_build_snapshot/clean                     | 96.62 ns  | 97.63 ns  | +0.70%  |
  | bench_build_snapshot_with_scrollback/10k_clean | 1.346 ms  | 1.154 ms  | −13.57% |
  | bench_build_snapshot/dirty                     | 99.23 ns  | 97.24 ns  | −2.83%  |
  | buffer_insert_large_line/insert_full/500000    | 19.01 ms  | 19.85 ms  | +4.24%  |
  | bench_visible_flatten/visible_200x50           | 2.31 µs   | 2.30 µs   | −0.54%  |

  All within the 15% regression threshold; `u8 as usize` and `usize::from(u8)` produce
  identical codegen on amd64, so the deltas are measurement noise.

- **70.H.2** — Delete every `#[allow(clippy::cast_*)]` attribute whose underlying cast has
  been replaced. Document any remaining allow with a `// SAFETY:` comment explaining why the
  conversion is lossless in context. Performed as part of each file cluster.
- **70.H.3** ✅ — Explicitly denied `clippy::cast_possible_truncation`,
  `clippy::cast_sign_loss`, and `clippy::cast_possible_wrap` at the crate root of all three
  library crates (`freminal-common`, `freminal-buffer`, `freminal-terminal-emulator`). These
  lints were already part of the `clippy::pedantic` group already denied in each lib.rs, but
  naming them directly documents the Task 70.H contract and survives future pedantic
  reorganization. All remaining `as` casts in library code are either inside test modules or
  covered by a local `#[allow(...)]` with a justification comment. The `freminal` binary
  crate relies on `clippy::pedantic` only (not explicit denies) because its rendering/GUI
  layer interacts with egui/winit APIs that frequently require fallible conversions guarded
  by local allows.

#### 70.I — MEDIUM: Complete Bool-to-Enum (Task 26 re-open)

Task 26 missed one field.

- **70.I.1** ✅ — Replaced `TerminalHandler::in_band_resize_enabled: bool` with
  `InBandResizeMode` (from `freminal-common/src/buffer_states/modes/`). The field stores
  only `Set` / `Reset` (the `Query` variant is a transient dispatch state used only at the
  `DECRQM ?2048` parse site). Default is `InBandResizeMode::Reset`. Call sites updated:
  the field assignment in both arms of the DECSET/DECRST handler, the `== Set` check at the
  resize-notification guard (`handle_resize`), the query-response ternary, and the test
  helper at `send_in_band_resize_dispatched`. No `to_payload`, snapshot, or
  `send_terminal_inputs` change was required — this field is local to the handler and never
  crossed the thread boundary. Verification: `cargo test --all` 4913+ green,
  `cargo clippy --all-targets --all-features -- -D warnings` clean, `cargo machete` clean.
  No hot-path code changed (simple field type swap, `bool` and `enum` with no payload have
  identical codegen), so no benchmark run was required.

#### 70.J — MEDIUM: Split Remaining God Files (Task 29 re-open)

Task 29 is marked complete, but three files are still oversized:

- `freminal-buffer/src/buffer.rs` — 11,012 lines
- `freminal-buffer/src/terminal_handler/mod.rs` — 5,188 lines (this becomes
  `freminal-terminal-emulator/src/terminal_handler/mod.rs` after 70.C)
- `freminal/src/gui/mod.rs` — 3,212 lines

- **70.J.1** ✅ — Split `buffer.rs` (11,024 lines) along natural seams into
  `buffer/{cursor,erase,flatten,images,lifecycle,lines,resize_and_alt,scroll,tabs}.rs`.
  `mod.rs` became a facade at 7,483 lines (still carries struct defs, insertion core,
  free helpers, and test modules). Committed in three stages: `70.J.1.a` (facade
  setup + git rename), `70.J.1.b` (tabs extraction as pattern template), `70.J.1.c`
  (eight remaining impl groups). Buffer fields changed to `pub(in crate::buffer)` —
  minimum privilege for sibling-module visibility.
- **70.J.2** ✅ — Split `terminal_handler/mod.rs` (5,193 lines) into six new sibling
  files: `cursor_ops.rs`, `edit_ops.rs`, `scroll_ops.rs`, `reports.rs`, `window_ops.rs`,
  `osc.rs`. `mod.rs` reduced to 4,528 lines (struct def, `new`/`with_scrollback_limit`/
  `full_reset`, core data-feeding pipeline, `set_format` + accessors, `apply_dec_special`
  free function, test modules). Handler fields changed to `pub(super)`.
- **70.J.3** ✅ — Split `gui/mod.rs` (3,244 lines) into six new sibling files:
  `tab_spawning.rs` (375), `layout_ops.rs` (604), `settings_dispatch.rs` (197),
  `session.rs` (141), `app_impl.rs` (1,547 — entire `impl App for FreminalGui`),
  `run.rs` (81, re-exported via `pub use run::run;`). `mod.rs` reduced to 410
  lines (type defs, `new`, `recording_window_id`, `compute_initial_size`, test
  module). No visibility changes needed — `FreminalGui` is already private to
  the `gui` module. Two module-level free helpers (`layout_dir_to_pane_dir`,
  `extract_root_leaf`) promoted to `pub(super)`.
- **70.J.4** ✅ — All three splits left `cargo test --all` passing at every commit.
  Benchmarks run post-split (within-session comparison to rule out measurement
  bias) showed only noise-level variation: worst case ~11% on `bench_lf_heavy`
  and `bench_insert_with_color_changes`, most under 3%, with roughly equal
  improvements and regressions — consistent with CPU/thermal/scheduler jitter
  on the same commit. The first run's wild deltas (+15000%, −94%) compared
  against a 17-day-old `base/` directory dated Apr 5, capturing the cumulative
  effect of Tasks 59, 61, 68, 69, 70.A–70.I, and 70.J.1/70.J.2, not the splits
  themselves. Pure module moves do not affect Rust codegen (LLVM inlines freely
  across module boundaries within a crate), so this result is expected.

#### 70.K — MEDIUM: Typed CSI Mode Discriminants ✅

- **70.K.1** ✅ — Added `EraseDisplayMode` (4 variants) and `EraseLineMode` (3 variants)
  enums in `ed.rs` / `el.rs` with `TryFrom<usize>` impls and typed error variants.
  Changed `handle_erase_in_display` / `handle_erase_in_line` to accept the enums;
  removed now-unreachable fallthrough match arms.
- **70.K.2** ✅ — Audited remaining `mode: usize` parameters. Typified
  `TerminalOutput::TabClear(usize)` → `TabClear(TabClearMode)` (6 variants) in
  `freminal-common`. Two remaining `usize` payloads (`SGR param`,
  `RequestSecondaryDeviceAttributes::param`) intentionally left as-is — both
  represent legitimately open-ended numeric namespaces, not a closed mode set.

#### 70.L — MEDIUM: Dead Code Attribute Cleanup ✅

- **70.L.1** ✅ — `freminal/src/gui/terminal/mouse.rs` — deleted unused
  `FreminalMousePosition` pixel fields; derived `Eq` on `PreviousMouseState`.
- **70.L.2** ✅ — `freminal/src/gui/renderer/gpu.rs` — deleted unused `gl_f32_u32`
  duplicate helper.

#### 70.M — MEDIUM: Extract Duplicated Helpers ✅

- **70.M.1** ✅ — Lifted `param_or` into
  `freminal-terminal-emulator/src/ansi_components/csi_commands/util.rs`; updated
  both `decstbm.rs` and `decslpp.rs` call sites.

#### 70.N — MEDIUM: `send_or_log` Helper ✅

- **70.N.1** ✅ — Added `send_or_log!` macro in `freminal-common/src/logging.rs`.
  Macro form preserves `tracing` span context and avoids closure overhead.
- **70.N.2** ✅ — Applied at 28 call sites across 12 files. Remaining `.send()`
  sites were either in tests or had non-standard error handling (e.g. returning
  `Err` rather than logging-and-continuing) and were intentionally left as-is.
  Added `#[allow(clippy::too_many_lines)]` on `dispatch_binding_action` — rustfmt
  expands the macro call sites to multi-line form, pushing the function over the
  100-line clippy limit; the expansion is mechanical and splitting would harm
  readability.

#### 70.O — LOW: Convention & Polish

- **70.O.1** ✅ — Dropped `get_` prefix from 14 accessors (15 impl sites including
  3 `win_size`); ~400+ call sites updated across the workspace.
- **70.O.2** ⏭️ **SKIPPED** — `#[non_exhaustive]` provides value only across
  SemVer boundaries. All `freminal-*` crates are consumed exclusively via
  `path = "..."` dependencies within this single-repo workspace — there is no
  external SemVer boundary. Adding `#[non_exhaustive]` would force `_ => { ... }`
  catch-all arms in workspace-internal matches, which _removes_ the compiler's
  exhaustiveness check — precisely the diagnostic we rely on when adding new
  variants. Revisit when Task 84 (scripting layer) exposes a genuine public
  plugin API with third-party consumers.
- **70.O.3** ✅ — `collect_text` now takes `&str` instead of `&String`; deref
  coercion makes this transparent at all non-test call sites. Two test sites
  updated to pass string literals directly.
- **70.O.4** ✅ — Refactored `build_background_instances` to take a
  `BackgroundFrame<'a>` struct (17 fields grouped) instead of 18 positional
  parameters. All 3 call sites updated; `clippy::too_many_arguments` allow
  removed from the function.
- **70.O.5** ✅ — Expanded doc comments on `RenderState` (authoritative site),
  the pane `render_state` field in `gui::panes::Pane`, the window
  `window_post` field in `gui::window::Window`, and the pane
  `window_post` field in `RenderState`. Each now explicitly states that
  the `Arc<Mutex<…>>` wrapper provides interior mutability for egui
  `PaintCallback` captures rather than cross-thread synchronisation, and
  that these types are GUI-thread-only in practice.

### 70 Verification

A single full-workspace verification suite at the end of each subtask group:

1. `cargo test --all`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo-machete`
4. Full benchmark compile + run (Criterion) for subtasks that touch hot paths (70.C, 70.E,
   70.H, 70.J). Record before/after numbers in the completion notes per the benchmark rule in
   `agents.md`.

Task 70 is complete only when all subtasks 70.A through 70.O are individually complete and
committed on `task-70/correctness-sweep` (or similar), and the verification suite passes on
the final commit.

---

## Task 71 — UX Completeness & Polish Sweep

### 71 Overview

The UX audit identified 20 concrete issues ranked P0–P3. The most damaging are features that
are advertised (keybinding exists, settings list exists) but silently do nothing, and
error paths that log-and-disappear with no user feedback.

### 71 Subtasks

#### 71.P0 — Fix Advertised-but-Broken Features

- **71.1** — Wire up `RenameTab`. `freminal/src/gui/actions.rs:299-301` is currently a
  `trace!` no-op. Implement an inline text-entry overlay on the target tab (similar to
  a rename in a file manager). Persist the custom name on the tab struct; clear it if the
  shell sets a title via OSC 0/1/2. **COMPLETE (2026-04-22).** Added `custom_name:
Option<String>` and `display_name()` to `Tab`; added `renaming_tab` + `rename_buffer`
  to `PerWindowState`. `KeyAction::RenameTab` and double-click on a tab now open an
  inline `TextEdit`. `TabBarAction` gained `BeginRename` / `CommitRename` / `CancelRename`.
  `handle_window_manipulation` now returns whether the shell asserted a title this frame;
  the caller clears `tab.custom_name` so shell-driven OSC 0/1/2 titles remain authoritative.
  Window title sync uses `Tab::display_name()`. 4 new unit tests.
- **71.2** — PTY spawn failure surface. When a shell fails to launch (bad path, missing
  binary, permission error), show an inline error row inside the tab (or a toast) with the
  error message and a retry button. Currently silent. **COMPLETE (2026-04-22).** Added a
  reusable app-level toast system (`freminal/src/gui/toast.rs`) with `ToastKind`
  (`Error`/`Warning`/`Info`), FIFO stack (MAX_TOASTS=5), kind-based auto-expire
  (Error=10s / Warning=6s / Info=3s), 200ms hover keep-alive, and dismiss button.
  `FreminalGui::toasts` uses `RefCell<ToastStack>` with a `push_error_toast(&self, …)`
  helper to avoid cascading `&mut self` through spawn call chains. Wired all 4 PTY spawn
  failure sites (new window in `app_impl.rs`, new tab / split pane / layout leaf in
  `tab_spawning.rs`). Toasts render top-right via `egui::Area` after the central panel.
  7 new unit tests.
- **71.3** — Layout load failure surface. TOML parse errors and missing-file errors currently
  log and disappear. Show a modal dialog naming the layout file and the specific error.
  **COMPLETE (2026-04-22).** Reused the toast system from 71.2 instead of a modal dialog —
  non-blocking toasts are less intrusive for recoverable failures. Wired all layout
  load/save failure sites: CLI `--layout` / `startup.layout` load and resolve errors
  (`app_impl.rs`), Layouts menu selection failures (`menu.rs`), `SaveLayout` action
  failures including missing library dir, directory creation failure, and TOML write
  failure (`actions.rs`), and `restore_last_session` apply failures (`session.rs`).
  Auto-save-on-shutdown failures in `auto_save_session` are intentionally log-only since
  the UI is already tearing down.
- **71.4** — Shader compile error surface. When a custom shader fails to compile, show a
  dismissible error banner naming the shader file and including the first line of the GLSL
  error. Piggybacks on `GpuInitError` types introduced in 70.E.
  **COMPLETE (2026-04-22).** Used the toast system (71.2) instead of a banner.
  `WindowPostRenderer` gained a `last_error: Option<String>` field written by
  `PaintCallback` closures (which cannot access `FreminalGui` directly — they run on
  the render thread with only `Arc<Mutex<WindowPostRenderer>>` in scope). The main
  thread drains this field at the top of each window's `update()` call and pushes an
  error toast. Both failure paths wired: `WindowPostRenderer::init` failure and
  `update_shader` compile failure. The `GpuInitError`'s `Display` impl already contains
  the shader label and GLSL error from glow's `get_shader_info_log`, so no additional
  formatting is needed.

#### 71.P1 — Discoverability

- **71.5** — Add Edit menu. Contains Copy, Paste, Select All, Find. Each item shows its
  current keybinding from `BindingMap`. Platform-appropriate placement (macOS menubar vs.
  Linux/Windows in-window menu bar).
  **COMPLETE (2026-04-22).** Added `Edit` menu between `Freminal` and `Tab` with Copy,
  Paste, Select All, and Find entries. Each button uses `menu_button_for(label, action)`
  to show the current keybinding combo from the `BindingMap`. Menu clicks push onto a new
  `PerWindowState::pending_menu_actions: Vec<KeyAction>` queue (menu closures do not have
  mutable access to the active pane's `ViewState` / `input_tx`). The queue is drained at
  the top of `FreminalGui::update` via a new `dispatch_menu_action` associated function
  that applies Copy/Paste/Select All directly to the active pane and routes the rest
  (OpenSearch, etc.) through the existing `all_deferred_actions` pipeline. For Copy, a
  new `Pane::pending_copy` boolean signals the widget to read `clipboard_rx` on its next
  `show()` call (mirroring the in-widget `clipboard_pending` flow). The Edit menu body
  was extracted into a `show_edit_menu` helper to keep `show_menu_bar` under clippy's
  `too_many_lines` threshold. This plumbing is reusable for 71.6 (Help menu).
- **71.6** — **COMPLETE (2026-04-22).** Added `Help` menu between `Layouts` and the lock
  indicator with three entries: `About Freminal`, `Report Issue...`, and `Keybindings...`.
  `About Freminal` opens an in-app floating `egui::Window` centered on the screen, showing
  the package name, `CARGO_PKG_VERSION`, build hash (re-exported as
  `freminal_terminal_emulator::GIT_DESCRIBE` from the `VERGEN_GIT_DESCRIBE` already emitted
  by the emulator's `build.rs`), a short description, and a Close button. The dialog is
  self-dismissing via its Close button or title-bar X. `Report Issue...` opens
  `https://github.com/fredsystems/freminal/issues/new` via `open::that`, surfacing failures
  as error toasts. `Keybindings...` sets a new `pending_open_keybindings` flag that the next
  `update()` frame drains — mirroring the existing Settings-menu flow, it either focuses an
  already-open settings window (and switches it to the Keybindings tab via a new
  `SettingsModal::set_active_tab` method) or opens a new settings modal pre-focused on
  Keybindings via a new `SettingsModal::open_to_tab` method. Two new bool fields were added
  to `FreminalGui` (`about_window_open`, `pending_open_keybindings`), crossing the
  `struct_excessive_bools` threshold; a targeted `#[allow]` with justification was added to
  the struct — each bool is an independent, short-lived UI intent flag and combining them
  into a state machine would couple unrelated concerns. No new `KeyAction` variants were
  introduced (the Help items have no shortcuts by design), so `agents.md`'s keybinding
  convention does not apply. Unit tests added for `open_to_tab` and `set_active_tab`.
- **71.7** — URL hover tooltip. When the mouse hovers over an OSC 8 or auto-detected URL,
  show a tooltip with the target URL and change the cursor to a pointer. **COMPLETE** —
  Added an `egui::Tooltip::always_open` at the pointer in `widget.rs` (after the cursor-icon
  update) driven by `cache.cached_hovered_url`. The tooltip displays the URL text and a
  platform-appropriate hint ("Ctrl+click to open" / "Cmd+click to open"). Suppressed while
  `view_state.selection.is_selecting` to avoid visually competing with an in-progress
  selection drag. Cursor-pointer switching was already implemented; this subtask adds only
  the tooltip.

#### 71.P2 — Search Polish

- **71.8** — Case-sensitivity toggle in the search bar (`Aa` icon or checkbox).
- **71.9** — Tooltips on `<` / `>` / `X` buttons ("Previous match", "Next match", "Close").
- **71.10** — Red-background tint on the search input when match count is zero.
- **71.11** — Verify Task 69's search panel positioning fix landed and still behaves
  correctly under all window sizes and tab configurations.

#### 71.P2 — Tab & Pane UX

- **71.12** — Tab close button ("×") on each tab, tab drag-reorder within a window (using
  egui's drag sense), and in-place tab rename (double-click, tied to the `RenameTab`
  implementation from 71.1).
- **71.13** — Add a `ClearScrollback` `KeyAction` (distinct from the existing
  `ClearScrollbackandDisplay`). Bind to a sensible default (`Ctrl+K` on macOS convention,
  configurable). Include in `KeyAction::ALL`, `name()`, `display_label()`, `FromStr`, and
  `BindingMap::default()` per the keybinding convention in `agents.md`.

#### 71.P2 — Feature Completeness

- **71.14** — Extend `BellMode` in `freminal-common/src/config.rs:406` with `Audio` and
  `Both` variants. Wire `Audio` to a simple system-bell sound (platform-appropriate — `\a`
  on Linux, `NSBeep` on macOS, `MessageBeep` on Windows). Add a config option for a custom
  sound file path. Update Settings Modal picker.
- **71.15** — In-app recording toggle. Add a `ToggleRecording` `KeyAction`, a menu item in
  the Edit menu (or a dedicated "Session" menu), and a visible `● REC` indicator in the
  tab/window chrome when recording is active. Recording currently only activates via
  `--recording-path`. Requires Task 59's FREC v2 runtime start/stop support (verify it
  exists; if not, add a small runtime API on the recorder).
- **71.16** — Cross-platform CWD readback. `freminal/src/gui/mod.rs:950-961` uses
  `/proc/<pid>/cwd` (Linux-only), which means Layout restore silently degrades on macOS and
  Windows. Implement:
  - macOS: `libproc::proc_pidinfo` with `PROC_PIDVNODEPATHINFO`.
  - Windows: query the console's current directory via `NtQueryInformationProcess` or
    `GetFinalPathNameByHandle` on the process handle.
  - Abstract behind a `platform::read_cwd(pid)` function with per-OS implementations.
- **71.17** — Config hot-reload. Currently only shaders hot-reload. Add a "Reload Config"
  menu item that re-reads `config.toml` and applies theme / font / keybinding / opacity
  changes live without restart. Use a file-watcher-optional design (opt-in auto-reload).

#### 71.P3 — Polish

- **71.18** — Unsaved-changes guard on Settings close. If Settings has pending unsaved
  edits and the user dismisses the modal, prompt to Save / Discard / Cancel.
- **71.19** — Startup tab layout setting in Settings Modal becomes a dropdown of layouts
  discovered in `~/.config/freminal/layouts/`, not a free-text field.
- **71.20** — First-run onboarding. Show a 3-panel overlay on first launch explaining the
  menu bar, the settings shortcut, and the layouts directory. Store a `first_run_complete`
  flag in the config. Skippable and permanently dismissible.

### 71 Verification

- Full verification suite after each P-level group.
- Manual UX walkthrough covering every item. Smoke-test with a clean config (no
  `config.toml`) and with an existing user config.
- Cross-platform verification of 71.14 (bell audio), 71.16 (CWD readback) — at minimum
  one Linux, one macOS, one Windows run.

Task 71 is complete when every one of the 20 items is implemented, tested, and verified.

---

## Sequencing

Task 70 and Task 71 are independent and may run in parallel on separate branches
(`task-70/correctness-sweep` and `task-71/ux-polish-sweep`), each with a nested set of
sub-branches per subtask group if the orchestrator prefers.

However, 71.4 (shader error surface) depends on 70.E (typed GPU errors), and 71.15
(recording toggle) may require minor work on the Task 59 recorder. Orchestrator should
sequence those two pairs accordingly.

---

## Design Decisions

- **No new features in v0.8.0.** Explicit non-goal. Any feature request that arrives during
  this version is logged to `FUTURE_PLANS.md` or the appropriate `PLAN_VERSION_NNN.md` and
  deferred.
- **"Complete" task statuses are audit-gated.** Going forward, marking a task complete in
  `MASTER_PLAN.md` requires that a subsequent audit pass is scheduled to verify the
  completion claim. This version is the corrective pass for the tasks that drifted.
- **Error design is typed.** No `String`-typed errors in any new code. No `anyhow` in
  library crates, ever. The rules in `agents.md` are the contract.
