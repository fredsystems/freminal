---
name: freminal-windows-crosscheck
description: Use ONLY when working in the freminal repository AND about to open a pull request, finalize a change, or declare a task done — especially any change touching `#[cfg(windows)]` / `#[cfg(target_os = "windows")]` code, the vendored `portable-pty` crate, cross-platform path or filesystem logic, threads/closures/`Send` bounds, or anything else that compiles differently on Windows. Mandates running `cargo xtask check-windows` (clippy for the x86_64-pc-windows-gnu target) before the PR, so Windows-only compile errors and lints are caught locally instead of only on the `windows-latest` CI job. Also states what this gate does NOT catch (runtime failures) and where those are caught instead.
---

# Freminal: run `cargo xtask check-windows` before a PR

Freminal builds and tests on Linux, macOS, and Windows. The Linux dev
host only compiles the Linux `#[cfg]` paths, so Windows-only code
(portable-pty's ConPTY backend, the kitty shared-memory / file
transmission tests, any `#[cfg(windows)]` branch) is invisible to a
normal `cargo check` / `cargo clippy` run. That code has repeatedly
broken in ways only discovered after a CI round-trip.

**Before opening a PR** (and before declaring any Windows-affecting
task done), run:

```sh
cargo xtask check-windows
```

This runs clippy for the `x86_64-pc-windows-gnu` target with
`-D warnings`, exercising the Windows `#[cfg]` paths locally. It is a
few seconds after the first (cached) run and catches the whole
compile-and-lint class before CI does.

## When this is mandatory

Run `check-windows` before a PR whenever the change touches any of:

- `#[cfg(windows)]` / `#[cfg(target_os = "windows")]` code.
- The vendored `portable-pty` crate (its Windows backend under
  `src/win/`).
- Cross-platform path logic (`Path::is_absolute`, path separators,
  `std::env::temp_dir`, drive letters).
- Threads, closures, or `Send`/`Sync` bounds (edition-2021 disjoint
  closure capture behaves differently across targets).
- Numeric / OS limits that differ by platform (e.g. POSIX shm name
  length).
- Any dependency, edition, or toolchain bump.

When in doubt, run it — it is cheap.

## Requirements: the `default` dev shell

`check-windows` needs the Windows cross toolchain, which lives in the
`default` dev shell only (never the `ci` shell). If you are not in it:

```sh
nix develop        # or: direnv allow
```

The shell provides the `x86_64-pc-windows-gnu` rust target, a
`mingw-w64` cross `cc` (for cargo's link-probe), and the
`FREMINAL_WINDOWS_CARGO` env var that `check-windows` uses to pick the
windows-capable cargo. If `FREMINAL_WINDOWS_CARGO` is unset,
`check-windows` warns and falls back to the cargo on `PATH` (which has
no windows-gnu std and will fail) — that warning means "you're not in
the `default` shell".

The mingw toolchain is a large first download; subsequent runs are
cached.

## What `check-windows` does NOT catch

`check-windows` is clippy — it **type-checks but does not link or run**.
It catches:

- Windows-only compile errors (e.g. edition-2021 `Send` capture
  regressions).
- Windows-only clippy lints (e.g. `redundant_clone`, `dead_code`,
  `uninit_vec` in `#[cfg(windows)]` code).

It does **not** catch **runtime** behavior differences — a test that
compiles fine on Windows but panics when run. Examples seen in this
repo:

- `shm_open` returning `ENAMETOOLONG` because a name exceeded macOS's
  31-char limit (runtime).
- `Path::is_absolute()` treating `/tmp/...` as relative on Windows, so
  a handler took the `EPERM` branch instead of the expected `EIO`
  (runtime).

That runtime class is caught by the **`windows-latest` CI test job**
(`cargo xtask test` on real Windows), not by `check-windows`. Do not
try to replicate it locally with Wine — it is lower-fidelity for
ConPTY / shm and not worth the setup. Let CI own the runtime class.

## Summary

| Gate                        | Where               | Catches                           |
| --------------------------- | ------------------- | --------------------------------- |
| `cargo xtask check`         | local + CI          | Linux compile + lints             |
| `cargo xtask check-windows` | local (dev shell)   | Windows compile + lints           |
| `cargo xtask test`          | `windows-latest` CI | Windows runtime failures / panics |

The pre-PR ritual: `check` (Linux), `check-windows` (Windows compile),
then let CI run `test` on real Windows for the runtime class.
