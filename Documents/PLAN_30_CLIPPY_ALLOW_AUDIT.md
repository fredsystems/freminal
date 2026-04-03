# PLAN_30 — Clippy Allow Audit: Eliminate Lint Suppressions

## Status: Pending

---

## Overview

A full audit of every `#[allow(clippy::...)]` attribute in the workspace revealed **231 lint
suppressions across 191 attribute lines** (196 production, 33 test, 2 bench). The dominant class
is casting lints (158 of 231, 68%), which are concentrated in the GPU renderer, terminal handler,
and shaping modules. These `as` casts are suppressed because Rust's `as` operator silently
truncates, wraps, or loses precision — exactly the failure modes the project wants to prevent.

The project already uses `conv2` (the maintained replacement for the `conv` crate) in three of
four library crates. This task replaces all `as` casts that have clippy suppressions with `conv2`
checked/approximate conversions, then eliminates or justifies every remaining non-casting
`#[allow(clippy::...)]` attribute.

**Goal:** Minimal clippy allows. Proper type conversions that can never fail in an unexpected way.
Test-code allows (`unwrap_used`, `expect_used`) are acceptable where they make functional sense.

**Dependencies:** None (independent, pure refactoring)
**Dependents:** None
**Primary crates:** All four library crates + `xtask`
**Estimated scope:** High (8 subtasks, ~158 casting sites + ~30 non-casting sites)

---

## Current State

### Grand Total: 231 Lint Suppressions

| Category              | Count | Production | Test | Bench |
| --------------------- | ----- | ---------- | ---- | ----- |
| Casting lints         | 158   | 135        | 19   | 2     |
| Bool-related          | 8     | 8          | 0    | 0     |
| Structural/complexity | 29    | 29         | 0    | 0     |
| Test infrastructure   | 28    | 1          | 26   | 0     |
| Naming/style          | 5     | 5          | 0    | 0     |
| Miscellaneous         | 3     | 3          | 0    | 0     |

### Casting Lint Breakdown

| Lint                       | Total | Production | Test | Bench |
| -------------------------- | ----- | ---------- | ---- | ----- |
| `cast_possible_truncation` | 71    | 61         | 8    | 0     |
| `cast_precision_loss`      | 59    | 48         | 9    | 2     |
| `cast_possible_wrap`       | 28    | 26         | 2    | 0     |
| `cast_sign_loss`           | 6     | 6          | 0    | 0     |

### Casting Hotspot Files

| File                                                                               | Approximate Suppression Count | Primary Pattern                          |
| ---------------------------------------------------------------------------------- | ----------------------------- | ---------------------------------------- |
| `freminal/src/gui/renderer.rs`                                                     | ~60+                          | `usize` -> `i32`/`f32` for OpenGL API    |
| `freminal-buffer/src/terminal_handler.rs`                                          | ~22                           | Image dimensions, cursor `usize`<->`i32` |
| `freminal/src/gui/shaping.rs`                                                      | ~10                           | Rendering coordinate math `usize`->`f32` |
| `freminal/src/gui/terminal.rs`                                                     | ~8                            | Scroll computation, scrollbar rendering  |
| `freminal-common/src/buffer_states/sixel.rs`                                       | ~8                            | Sixel color math                         |
| `freminal-common/src/colors.rs`                                                    | ~6                            | Color channel scaling                    |
| `freminal/src/gui/font_manager.rs`                                                 | ~4                            | Cell dimension casting                   |
| `freminal/src/gui/atlas.rs`                                                        | ~4                            | Atlas entry dimension casting            |
| `freminal/src/gui/mod.rs`                                                          | ~4                            | Cell size casting                        |
| `freminal/src/gui/view_state.rs`                                                   | ~2                            | Blink tick casting                       |
| `freminal-terminal-emulator/src/ansi_components/osc_palette.rs`                    | ~4                            | Palette color casting                    |
| `freminal-terminal-emulator/src/ansi_components/csi_commands/sgr.rs`               | ~2                            | SGR lookup casting                       |
| `freminal-terminal-emulator/src/ansi_components/csi_commands/modify_other_keys.rs` | ~2                            | Level casting                            |
| `freminal-terminal-emulator/src/interface.rs`                                      | ~2                            | Resize pixel dimension casting           |
| `freminal-buffer/src/buffer.rs`                                                    | ~2                            | Test-only casting                        |

### Common Casting Patterns

1. **`usize` -> `i32`/`u32` for OpenGL API** (renderer.rs): Cell counts, pixel offsets, vertex
   strides. Values are small (terminal dimensions \* cell size). `conv2::ValueFrom` with explicit
   error handling or `.value_as::<i32>().unwrap_or(0)` in contexts where the value is guaranteed
   small.

2. **`usize`/`u32` -> `f32` for GPU coordinate math** (renderer.rs, shaping.rs, terminal.rs):
   Values fit within f32's 24-bit mantissa for any reasonable terminal size. Use
   `conv2::ApproxFrom<_, RoundToZero>` or `.approx_as::<f32>()`.

3. **`u32`/`usize` -> `u8`/`u16` for protocol values** (sixel.rs, colors.rs, osc_palette.rs):
   Values are contextually guaranteed to fit (e.g., color channels 0-255). Use
   `conv2::ValueFrom` with proper error handling.

4. **`usize` <-> `i32` for cursor movement** (terminal_handler.rs): Small terminal dimensions.
   Use `conv2::ValueFrom` with fallible conversion.

### Non-Casting Lint Assessment

| Lint                                                           | Count | Assessment                                                                                                                                                                  |
| -------------------------------------------------------------- | ----- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `struct_excessive_bools` (6) + `fn_params_excessive_bools` (2) | 8     | **Addressed by Task 26** (Bool-to-Enum refactor). No action here.                                                                                                           |
| `too_many_lines`                                               | 16    | Mixed: some are inherent (parser dispatch, renderer), some fixable by Task 29 (god file split). Keep allows on inherently-large functions; remove when Task 29 splits them. |
| `too_many_arguments`                                           | 13    | Mixed: renderer functions pass many GPU params (inherent); others may be fixable by grouping into structs. Evaluate per-site.                                               |
| `unwrap_used` / `expect_used` (test)                           | 27    | **Keep** — standard practice in test code.                                                                                                                                  |
| `unwrap_used` (production, 1)                                  | 1     | `shaping.rs:180` — restructure to eliminate.                                                                                                                                |
| `module_name_repetitions`                                      | 5     | **Keep** — types are re-exported; renaming reduces clarity.                                                                                                                 |
| `significant_drop_tightening`                                  | 1     | **Keep** — false positive on Arc closure in paint callback.                                                                                                                 |
| `implicit_hasher`                                              | 1     | **Keep** — internal function; generic hasher adds no value.                                                                                                                 |
| `missing_const_for_fn`                                         | 2     | **Fix** — make the functions const.                                                                                                                                         |
| `unnecessary_wraps`                                            | 1     | **Fix** — return inner type directly.                                                                                                                                       |
| `needless_pass_by_ref_mut`                                     | 1     | **Fix** — take `&self` instead.                                                                                                                                             |

---

## Subtasks

---

### 30.1 — Add `conv2` to `freminal-buffer` and Update `agents.md`

- **Status:** Complete
- **Completed:** 2026-04-03. Added `conv2.workspace = true` to `freminal-buffer/Cargo.toml`.
  Added "Numeric Conversions" subsection to `agents.md` under "Code Style". `cargo build --all`
  passes.
- **Priority:** 1 — High (unblocks all other subtasks)
- **Scope:** `freminal-buffer/Cargo.toml`, `agents.md`
- **Details:**
  1. Add `conv2.workspace = true` to `freminal-buffer/Cargo.toml` dependencies. This is the
     only library crate that does not yet have `conv2`.

  2. Add a "Numeric Conversions" subsection to `agents.md` under "Code Style" that establishes
     the `conv2` convention:

     ```markdown
     ### Numeric Conversions

     - Raw `as` casts are forbidden for numeric type conversions in production code
     - Use `conv2` crate traits for all numeric conversions:
       - `ValueFrom` / `ValueInto` for lossless conversions that may fail (e.g., `usize` -> `i32`)
       - `ApproxFrom` / `ApproxInto` with `RoundToZero` for float conversions (e.g., `usize` -> `f32`)
       - `ConvUtil::value_as` and `ConvUtil::approx_as` for inline conversions
     - `as` casts are permitted only for:
       - Casts that are guaranteed lossless by the type system (e.g., `u8` -> `u32`)
       - Test code (`#[cfg(test)]` or `tests/`)
       - Benchmark code
     - When a conversion can fail, handle the error explicitly — do not use `.unwrap()` on the
       conversion result in production code
     ```

- **Acceptance criteria:**
  - `freminal-buffer/Cargo.toml` lists `conv2.workspace = true`.
  - `agents.md` contains the numeric conversion convention.
  - `cargo build --all` passes (no code changes yet, just dependency addition).
  - `cargo-machete` may flag `conv2` as unused in `freminal-buffer` until 30.2 is complete —
    this is expected and acceptable temporarily.
- **Tests required:** `cargo build --all` passes.

---

### 30.2 — Replace Casting Suppressions in `freminal-buffer`

- **Status:** Complete
- **Completed:** 2026-04-03. Replaced all 24 production `#[allow(clippy::cast_*)]` in
  `terminal_handler.rs`. Cursor movement uses `i32::value_from().unwrap_or(i32::MAX)`.
  Image dimension `u32->usize` uses `usize::value_from().unwrap_or(0)`. Aspect ratio
  `usize->u64` and `u64->usize` use `ValueFrom`. Bit-extraction `u32->u8` uses
  `u8::try_from((x >> N) & 0xFF).unwrap_or(0)`. Two test-only allows in `buffer.rs` kept
  (acceptable per convention). Zero production casting suppression attributes remain.
- **Priority:** 2 — High
- **Scope:** `freminal-buffer/src/terminal_handler.rs` (~22 sites),
  `freminal-buffer/src/buffer.rs` (~2 test sites)
- **Details:**
  Replace all `#[allow(clippy::cast_*)]` attributes in `freminal-buffer` with `conv2` conversions.

  **`terminal_handler.rs` patterns (~22 sites):**
  - Image dimension arithmetic: `u32` -> `usize`, `usize` -> `u32`. Use `ValueFrom`/`ValueInto`.
  - Cursor movement: `usize` <-> `i32` for relative positioning. Use `ValueFrom` with explicit
    error handling (return error or clamp).
  - Protocol values: `usize` -> `u8`/`u16` where context guarantees range. Use `ValueFrom`.

  **`buffer.rs` patterns (~2 sites, test-only):**
  - These are in `#[cfg(test)]` and can remain as `as` casts. Remove the `#[allow]` attributes
    only if the casts can be trivially replaced; otherwise leave test code alone.

  After this subtask, `freminal-buffer` should have zero production `#[allow(clippy::cast_*)]`
  attributes.

- **Acceptance criteria:**
  - Zero `#[allow(clippy::cast_*)]` in production code in `freminal-buffer/src/`.
  - All conversions use `conv2` traits with proper error handling.
  - No panicking conversion paths in production code.
  - `cargo test --all` passes.
  - `cargo clippy --all-targets --all-features -- -D warnings` passes.
- **Tests required:**
  - Existing tests pass (behavior unchanged).
  - If any conversion site could realistically fail (e.g., image dimension overflow), add a test
    that exercises the error path.

---

### 30.3 — Replace Casting Suppressions in `freminal-common`

- **Status:** Complete
- **Completed:** 2026-04-03. Replaced 6 of 8 `#[allow(clippy::cast_*)]` in `freminal-common`.
  `colors.rs`: `scale_hex_channel` uses `u8::try_from().ok()`. `sixel.rs`: `pct_to_rgb` uses
  `u8::value_from().unwrap_or(0)`, `finish`/`finish_with_palette` use `u32::value_from().ok()?`.
  `unicode_placeholder.rs`: `diacritic_to_index` uses `u16::try_from().ok()`. `base64.rs`:
  `decode` uses `u8::try_from().unwrap_or(0)`. Two allows kept in const fns (`f64_to_u8`,
  `usize_from_u32`) with justification comments — `conv2` traits not available in `const fn`.
- **Priority:** 2 — High
- **Scope:** `freminal-common/src/buffer_states/sixel.rs` (~8 sites),
  `freminal-common/src/colors.rs` (~6 sites)
- **Details:**
  Replace all `#[allow(clippy::cast_*)]` attributes in `freminal-common` with `conv2` conversions.

  **`sixel.rs` patterns (~8 sites):**
  - Sixel color math: channel scaling between different integer widths and float.
  - Use `ValueFrom` for integer narrowing, `ApproxFrom<_, RoundToZero>` for float.

  **`colors.rs` patterns (~6 sites):**
  - Color channel scaling (already partially uses `conv2` based on the `ValueInto` import
    at line 7). Complete the migration for remaining `as` casts.

  After this subtask, `freminal-common` should have zero `#[allow(clippy::cast_*)]` attributes.

- **Acceptance criteria:**
  - Zero `#[allow(clippy::cast_*)]` in `freminal-common/src/`.
  - `cargo test --all` passes.
  - `cargo clippy --all-targets --all-features -- -D warnings` passes.
- **Tests required:**
  - Existing color tests pass.
  - Existing sixel tests pass.

---

### 30.4 — Replace Casting Suppressions in `freminal-terminal-emulator`

- **Status:** Complete
- **Completed:** 2026-04-03. Replaced all 8 `#[allow(clippy::cast_*)]` in
  `freminal-terminal-emulator`. `osc_palette.rs` (4 sites): `u8::try_from().unwrap_or(0)`.
  `sgr.rs` (1 site): `u8::try_from(lookup & 0xFF).unwrap_or(0)`. `modify_other_keys.rs`
  (1 site): `u8::try_from(level).unwrap_or(0)`. `interface.rs` (2 sites):
  `u32::value_from().unwrap_or(0)` for pixel dimensions. Zero casting suppressions remain.
- **Priority:** 2 — High
- **Scope:** `freminal-terminal-emulator/src/ansi_components/osc_palette.rs` (~4 sites),
  `freminal-terminal-emulator/src/ansi_components/csi_commands/sgr.rs` (~2 sites),
  `freminal-terminal-emulator/src/ansi_components/csi_commands/modify_other_keys.rs` (~2 sites),
  `freminal-terminal-emulator/src/interface.rs` (~2 sites)
- **Details:**
  Replace all `#[allow(clippy::cast_*)]` attributes in `freminal-terminal-emulator` with `conv2`
  conversions.

  **`osc_palette.rs` (~4 sites):** Color parsing math — integer narrowing.
  **`sgr.rs` (~2 sites):** SGR parameter lookup table indexing.
  **`modify_other_keys.rs` (~2 sites):** ModifyOtherKeys level casting.
  **`interface.rs` (~2 sites):** Resize pixel dimension casting in `handle_resize_event`.

  After this subtask, `freminal-terminal-emulator` should have zero `#[allow(clippy::cast_*)]`
  attributes.

- **Acceptance criteria:**
  - Zero `#[allow(clippy::cast_*)]` in `freminal-terminal-emulator/src/`.
  - `cargo test --all` passes.
  - `cargo clippy --all-targets --all-features -- -D warnings` passes.
- **Tests required:**
  - Existing parser tests pass.
  - Existing SGR tests pass.

---

### 30.5 — Replace Casting Suppressions in `freminal` (Non-Renderer)

- **Status:** Complete
- **Completed:** 2026-04-03. Replaced all production `#[allow(clippy::cast_*)]` in
  `terminal.rs`, `mod.rs`, `font_manager.rs`, `atlas.rs`, `shaping.rs`, `view_state.rs`,
  and `mouse.rs`. Committed as `33c6aaa`.
- **Priority:** 2 — High
- **Scope:** `freminal/src/gui/terminal.rs` (~8 sites),
  `freminal/src/gui/mod.rs` (~4 sites),
  `freminal/src/gui/font_manager.rs` (~4 sites),
  `freminal/src/gui/atlas.rs` (~4 sites),
  `freminal/src/gui/shaping.rs` (~10 sites),
  `freminal/src/gui/view_state.rs` (~2 sites),
  `freminal/src/gui/mouse.rs` (if any)
- **Details:**
  Replace all `#[allow(clippy::cast_*)]` attributes in the `freminal` crate **except**
  `renderer.rs` (which is handled separately in 30.6 due to its size).

  **Common patterns:**
  - `terminal.rs`: Scroll line computation, scrollbar dimensions — `usize` -> `f32` for UI
    coordinates. Use `ApproxFrom<_, RoundToZero>`.
  - `shaping.rs`: Rendering coordinates — `usize`/`i32` -> `f32`. Use `ApproxFrom`.
  - `font_manager.rs`: Cell dimensions — `f32` -> `usize` and vice versa. Use `ApproxFrom`
    or `ValueFrom` as appropriate.
  - `atlas.rs`: Atlas entry dimensions — similar to font_manager.
  - `view_state.rs`: Blink tick — `f64` -> `u64` for timer math.
  - `mod.rs`: Cell size casting.

- **Acceptance criteria:**
  - Zero `#[allow(clippy::cast_*)]` in `freminal/src/gui/` except `renderer.rs`.
  - `cargo test --all` passes.
  - `cargo clippy --all-targets --all-features -- -D warnings` passes.
- **Tests required:**
  - Existing GUI tests pass.
  - Manual smoke test recommended: verify font rendering, scrollbar, atlas at multiple DPI.

---

### 30.6 — Replace Casting Suppressions in `renderer.rs`

- **Status:** Complete
- **Completed:** 2026-04-03. Replaced all ~60 production `#[allow(clippy::cast_*)]` in
  `renderer.rs`. Added helper functions `gl_i32(usize)->i32`, `gl_i32_u32(u32)->i32`,
  `gl_f32(usize)->f32`, `gl_f32_u32(u32)->f32`, `gl_f32_i32(i32)->f32` using `conv2`
  traits. All OpenGL stride/offset/count casts use `gl_i32`; coordinate casts use `gl_f32`
  variants. `emit_glyph_instance` narrowing `u32->u16` uses `u16::value_from().unwrap_or(u16::MAX)`.
  Two `cast_precision_loss` allows in `#[cfg(test)]` retained (acceptable per convention).
  Zero production casting suppression attributes remain.
- **Priority:** 2 — High
- **Scope:** `freminal/src/gui/renderer.rs` (~60+ sites)
- **Details:**
  This is the single largest file for casting suppressions. The renderer interfaces with OpenGL
  via `glow`, which requires `i32` for strides, offsets, counts, and buffer sizes, and `f32`
  for vertex coordinates and uniforms. All values originate as `usize` from terminal cell
  dimensions.

  **Strategy:**
  - Create a small set of helper functions at the top of `renderer.rs` (or in a
    `renderer_conv.rs` helper module) that encapsulate the common conversion patterns:

    ```rust
    /// Convert a usize to i32 for OpenGL. Returns 0 on overflow (defensive).
    fn gl_i32(val: usize) -> i32 { ... }

    /// Convert a usize to f32 for GPU coordinates.
    fn gl_f32(val: usize) -> f32 { ... }
    ```

  - Replace all `val as i32` / `val as f32` call sites with the helpers.
  - Internally, the helpers use `conv2::ValueFrom` or `conv2::ApproxFrom` with a documented
    fallback strategy (return 0 or clamp) for the astronomically unlikely overflow case.
  - This approach avoids cluttering every OpenGL call with inline conversion boilerplate.

  **Note:** Some casts in `renderer.rs` are `i32` -> `isize` for pointer offset calculations
  in OpenGL buffer mapping. These are guaranteed safe on all supported platforms (64-bit) but
  should still go through `conv2` for consistency.

- **Acceptance criteria:**
  - Zero `#[allow(clippy::cast_*)]` in `renderer.rs`.
  - Helper functions are documented with their safety rationale.
  - `cargo test --all` passes.
  - `cargo clippy --all-targets --all-features -- -D warnings` passes.
  - Manual smoke test: terminal renders correctly, text alignment is pixel-perfect.
- **Tests required:**
  - Unit tests for the helper functions covering:
    - Normal values (0, 1, 80, 200, 1920)
    - Edge values near `i32::MAX` / `f32` mantissa limit
    - Overflow behavior (returns fallback, does not panic)
  - Existing render benchmarks show no regression (the helpers should inline).

---

### 30.7 — Fix Remaining Non-Casting Suppressions

- **Status:** Complete
- **Completed:** 2026-04-03. Fixed the one genuinely fixable production allow: replaced the
  `#[allow(clippy::unwrap_used)]` in `shaping.rs:182` with an `if let` pattern that avoids the
  double-lookup entirely. The two `missing_const_for_fn`/`needless_pass_by_ref_mut` allows in
  `internal.rs` and the `missing_const_for_fn`/`unnecessary_wraps` allow in `config.rs` are
  legitimate and were annotated with justification comments (cannot be made `const` due to
  `PathBuf::from()` / row-cache mutation; `&mut self` genuinely required). All 29
  `too_many_lines` and 13 `too_many_arguments` sites were annotated with justification comments
  explaining why splitting is not warranted. `significant_drop_tightening`, `implicit_hasher`,
  and `module_name_repetitions` sites were also annotated.
- **Priority:** 3 — Medium
- **Scope:** Various files across all crates
- **Details:**
  Address the non-casting `#[allow(clippy::...)]` attributes that are fixable:
  1. **`missing_const_for_fn` (2 sites):**
     - `config.rs:495` — make the function `const`.
     - `internal.rs:171` — make the function `const`.

  2. **`unnecessary_wraps` (1 site):**
     - `config.rs:495` — return the inner type directly instead of wrapping in `Result`/`Option`.
     - Note: This may be on the same function as the `missing_const_for_fn` allow. If so,
       making it `const` and removing the unnecessary wrap is a single change.

  3. **`needless_pass_by_ref_mut` (1 site):**
     - `internal.rs:172` — take `&self` instead of `&mut self`.

  4. **`unwrap_used` in production (1 site):**
     - `shaping.rs:180` — restructure to avoid the `unwrap()`. The proof comment suggests the
       unwrap is logically unreachable; replace with a `match` or `.unwrap_or_default()` or
       propagate the error.

  5. **`too_many_lines` (16 sites) and `too_many_arguments` (13 sites):**
     - Review each site. For functions that are inherently large (parser dispatch tables,
       OpenGL render passes), add a brief comment explaining why the allow is justified and
       keep the suppress.
     - For functions that can be reasonably split, split them and remove the suppress.
     - Do NOT force-split functions if the result is worse (artificial intermediate functions
       with no clear responsibility). The allows are acceptable when justified.

  6. **Suppresses addressed by other tasks:**
     - `struct_excessive_bools` (6) + `fn_params_excessive_bools` (2) — handled by Task 26.
       Leave these alone; they will be removed when Task 26 is executed.

  7. **Legitimate keeps (document but do not change):**
     - `module_name_repetitions` (5) — types are re-exported; renaming reduces clarity.
     - `significant_drop_tightening` (1) — false positive on Arc in paint callback.
     - `implicit_hasher` (1) — internal function; generic hasher adds no value.
     - `unwrap_used` / `expect_used` in test code (27) — standard practice.

- **Acceptance criteria:**
  - `missing_const_for_fn`, `unnecessary_wraps`, `needless_pass_by_ref_mut` allows removed.
  - Production `unwrap_used` in `shaping.rs` eliminated.
  - `too_many_lines` / `too_many_arguments` allows are either removed (function split) or
    annotated with a justification comment.
  - `cargo test --all` passes.
  - `cargo clippy --all-targets --all-features -- -D warnings` passes.
- **Tests required:**
  - Existing tests pass (behavior unchanged for const/wrap/ref-mut fixes).
  - If `shaping.rs` error path changes, verify the shaping still works correctly.

---

### 30.8 — Final Audit and Verification

- **Status:** Pending
- **Priority:** 3 — Medium
- **Scope:** All crates
- **Details:**
  After subtasks 30.1–30.7 are complete, run a final audit:
  1. Search for all remaining `#[allow(clippy::` attributes in the workspace.
  2. Categorize each as:
     - **Justified** — has a comment explaining why (keep).
     - **Addressed by other task** — references the task number (keep until that task runs).
     - **Unjustified** — remove or fix.
  3. Verify the total count is minimal (target: < 40 remaining, consisting of test-code
     `unwrap_used`/`expect_used`, documented `module_name_repetitions`, and Task 26 bools).
  4. Update this plan document with the final suppression count and breakdown.
  5. Run the full verification suite.
  6. Run `cargo bench --all` and verify no performance regressions from the `conv2` changes.
     The `conv2` operations should compile down to the same machine code as `as` casts for
     in-range values (they are `#[inline]` and optimized away), but this must be confirmed.

- **Acceptance criteria:**
  - All remaining `#[allow(clippy::...)]` attributes are justified with a comment or
    documented as pending another task.
  - Total suppression count is documented in this plan.
  - `cargo test --all` passes.
  - `cargo clippy --all-targets --all-features -- -D warnings` passes.
  - `cargo-machete` passes.
  - Benchmarks show no regression vs. pre-task baseline.
- **Tests required:**
  - Full verification suite.
  - `cargo bench --all -- --test` (compile check, no full run needed unless 30.6 touched
    hot paths).

---

## Implementation Notes

### Subtask Ordering

30.1 must be done first (adds `conv2` to `freminal-buffer` and establishes the convention).

30.2 through 30.6 are independent of each other (they touch different crates/files) and can be
done in any order or in parallel. However, starting with 30.3 (smallest scope) as a warm-up is
recommended.

30.7 is independent of 30.2–30.6 and can be done in parallel.

30.8 must be done last (final audit).

**Recommended order:** 30.1 -> 30.3 -> 30.4 -> 30.2 -> 30.5 -> 30.6 -> 30.7 -> 30.8

### `conv2` Trait Selection Guide

| Source Type | Target Type | Pattern           | `conv2` Trait                | Example                   |
| ----------- | ----------- | ----------------- | ---------------------------- | ------------------------- |
| `usize`     | `i32`       | Truncation + wrap | `ValueFrom`                  | `i32::value_from(val)?`   |
| `usize`     | `u32`       | Truncation        | `ValueFrom`                  | `u32::value_from(val)?`   |
| `usize`     | `u16`       | Truncation        | `ValueFrom`                  | `u16::value_from(val)?`   |
| `usize`     | `u8`        | Truncation        | `ValueFrom`                  | `u8::value_from(val)?`    |
| `usize`     | `f32`       | Precision loss    | `ApproxFrom<_, RoundToZero>` | `f32::approx_from(val)`   |
| `u32`       | `f32`       | Precision loss    | `ApproxFrom<_, RoundToZero>` | `f32::approx_from(val)`   |
| `i32`       | `usize`     | Sign loss         | `ValueFrom`                  | `usize::value_from(val)?` |
| `f32`       | `usize`     | Truncation + sign | `ApproxFrom<_, RoundToZero>` | `usize::approx_from(val)` |
| `f64`       | `u64`       | Precision loss    | `ApproxFrom<_, RoundToZero>` | `u64::approx_from(val)`   |
| `u8`        | `u32`       | Lossless          | Plain `as` or `From`         | `u32::from(val)`          |
| `u8`        | `usize`     | Lossless          | Plain `as` or `From`         | `usize::from(val)`        |

For lossless widening conversions (e.g., `u8` -> `u32`), prefer `From`/`Into` over `conv2` —
the standard library already guarantees these.

### Error Handling Strategy

When a `conv2` conversion returns `Err`:

- **Renderer/GPU code:** Use a helper function that returns a sensible default (0 for offsets,
  1 for dimensions) and logs a `tracing::warn!`. Terminal dimensions cannot realistically
  overflow `i32` (max 2 billion cells), so this path is defensive only.
- **Protocol parsing:** Return the appropriate parse error variant (the calling code already
  has error handling for malformed input).
- **Color math:** Clamp to valid range (0–255 for channels). Color rendering should never
  panic on out-of-range input.

### Benchmark Impact

The `conv2` traits are `#[inline]` and compile to range checks that the optimizer can often
eliminate when the value is provably in range. For hot paths (renderer, parser), verify that
benchmark numbers do not regress by more than noise (~2%). If they do, profile the specific
conversion site and consider whether a `debug_assert!` + raw `as` cast is justified (document
the rationale if so).

### Interaction with Other Tasks

- **Task 26 (Bool-to-Enum):** The 8 `struct_excessive_bools` / `fn_params_excessive_bools`
  suppressions are handled by Task 26, not this task. Leave those allows in place.
- **Task 29 (God File Refactoring):** Some `too_many_lines` / `too_many_arguments` allows
  will be resolved when god files are split. For now, add justification comments to inherently
  large functions and leave the allows.

### Risk Assessment

- **Low risk:** All changes are mechanical (replace `as` with `conv2`). No behavior change for
  in-range values. Out-of-range values now produce explicit errors instead of silent truncation.
- **Benchmark risk:** The `conv2` range checks add a branch per conversion. In the renderer hot
  path (~60 conversions per frame), this could add measurable overhead if not optimized away.
  Subtask 30.6 includes benchmark verification. The helper-function approach allows centralized
  tuning if needed.
- **Merge conflict risk:** `renderer.rs` and `terminal_handler.rs` are frequently modified by
  other tasks. Run this task on a feature branch and rebase before merge.

---

## References

- `conv2` crate documentation: <https://docs.rs/conv2/>
- `freminal/src/gui/renderer.rs` — largest casting hotspot (~60+ sites)
- `freminal-buffer/src/terminal_handler.rs` — second largest hotspot (~22 sites)
- `freminal/src/gui/shaping.rs` — rendering coordinate casting
- `freminal/src/gui/terminal.rs` — scroll computation casting
- `freminal-common/src/buffer_states/sixel.rs` — sixel color math
- `freminal-common/src/colors.rs` — color channel scaling
- `agents.md` — will be updated with `conv2` convention in subtask 30.1
- `Documents/PLAN_26_BOOL_TO_ENUM.md` — handles the 8 bool-related suppressions
- `Documents/PLAN_29_GOD_FILE_REFACTOR.md` — may resolve some `too_many_lines` suppressions
