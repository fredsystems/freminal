---
name: freminal-numeric-conversions
description: Use ONLY when working in the freminal repository AND writing or reviewing Rust code that converts between numeric types (usize, u32, i32, f32, f64, etc.). Codifies the "no raw `as` casts in production for numeric conversions" rule and mandates the `conv2` crate's `ValueFrom` / `ValueInto` / `ApproxFrom` / `ApproxInto` traits.
---

# Freminal: numeric conversions use the `conv2` crate

Raw `as` casts are **forbidden for numeric conversions in production
code** in the freminal workspace. `as` silently truncates, silently
wraps, and silently changes sign -- all behaviors that have caused real
bugs in this codebase.

Use the `conv2` crate's traits instead. They make every conversion
either explicit-and-checked or explicit-and-approximate.

## The rules

- **`ValueFrom` / `ValueInto`** for **lossless conversions that may
  fail** (e.g. `usize -> i32`). Returns a `Result`; the failure case
  must be handled.
- **`ApproxFrom` / `ApproxInto`** with `RoundToZero` (or other
  rounding strategies) for **float conversions** (e.g. `usize -> f32`,
  `f64 -> i32`).
- **`ConvUtil::value_as` and `ConvUtil::approx_as`** for inline
  conversions when chaining reads better than `.try_into()?`.

## When `as` is still OK

- **Casts the type system guarantees lossless** (e.g. `u8 -> u32`,
  `u16 -> usize` on a 32-bit-or-wider target). The conversion can't
  fail and isn't approximate.
- **Test code** (`#[cfg(test)]` modules and files under `tests/`).
- **Benchmark code**.

## Examples

```rust
// Wrong (production):
let bytes_written = (count as i32) - (skipped as i32);

// Right (production):
use conv2::ValueInto;
let bytes_written: i32 = count.value_into()? - skipped.value_into()?;
```

```rust
// Wrong (production):
let scaled = (px as f32) * scale;

// Right (production):
use conv2::ApproxInto;
let scaled: f32 = ApproxInto::<f32>::approx_into(px)? * scale;
// Or with ConvUtil:
use conv2::ConvUtil;
let scaled: f32 = px.approx_as::<f32>()? * scale;
```

## Handling the failure case

When a conversion can fail, **handle the error explicitly**. Do NOT
`.unwrap()` the result in production code -- that's both a panic
(see `rust-best-practices`) and a way to silently lose the structured
error.

Wrap the conversion in your domain error:

```rust
let row_idx: i32 = row
  .value_into()
  .map_err(|_| BufferError::RowIndexOverflow(row))?;
```

## Why this matters

The lint that enforces this (a custom clippy lint plus code review)
exists because silent truncation has hidden bugs in this codebase
that took hours to find. The `conv2` crate makes those bugs surface
as compile-time work or runtime structured errors, both of which are
fixable; silent `as` casts are not.

## When to stop and ask

- A genuinely-lossless `as` cast is being flagged. Check if the
  source type is wider than the destination on any target platform
  (e.g. `usize -> u32` is NOT lossless on 64-bit targets even though
  the values you have are small). If it is genuinely safe, an
  `#[allow]` with a justifying comment is acceptable.
- A loop hot path is benchmarked-slower with `conv2` than with `as`.
  Don't bypass; surface the benchmark to the user. There's usually
  a structural fix (operate in the destination type from the start).
- The failure case of a `value_into` is genuinely unreachable.
  Restructure so that's expressed via the type system (e.g. clamp
  on construction so the source type can't carry an out-of-range
  value), not by `.unwrap()`.
