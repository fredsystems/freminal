# Vendored from WezTerm Portable PTY

- **Source:** <https://github.com/wezterm/wezterm> (crate `pty/`)
- **Version:** 0.9.1 (as declared in the upstream `Cargo.toml`)
- **Vendored on:** 2026-04-05
- **Why:** Upstream `portable-pty` had outdated transitive dependencies and we wanted to bring them up to newer versions.

## Changes from upstream

1. `Cargo.toml` — `serde = ["serde"]` → `serde = ["dep:serde"]` (circular feature fix).
2. `cmdbuilder.rs` — Removed unstable `str_as_str` usage in `as_os_str()` chain.
3. `lib.rs` — Fixed `Option<ExitStatus>` `From` impl (map inside `Option`).
4. `cmdbuilder.rs` — `arg()`, `args()`, `replace_default_prog()` return `anyhow::Result`
   instead of panicking.
5. `cmdbuilder.rs` — PATHEXT `expect()` replaced with `let Some(...) else { continue }`.
6. `unix.rs` — `MaybeUninit::zeroed().assume_init()` replaced with
   `MaybeUninit::uninit()` + init after `tcgetattr` succeeds.
7. `win/pseudocon.rs` — `load_conpty()` returns `anyhow::Result` instead of `expect()`.
8. `win/pseudocon.rs` — Typo `PSUEDOCONSOLE` → `PSEUDOCONSOLE_INHERIT_CURSOR`.
9. `win/pseudocon.rs` — Safety justification added for `unsafe impl Send/Sync`.
10. `win/conpty.rs` — `lock().unwrap()` replaced with `lock_inner()` helper returning `Result`.
11. `win/mod.rs` — `kill()` now propagates errors instead of swallowing them.
12. `win/mod.rs` — `Future::poll` on `WinChild` uses `AtomicBool` to prevent spawning
    duplicate waiter threads on repeated polls.
13. `Cargo.toml` — opts into `[lints] workspace = true` (2026-11-21); the
    findings were fixed rather than allowed. Behaviour changes:
    - `cmdbuilder.rs` — the Windows registry `REG_EXPAND_SZ` bytes are decoded
      as little-endian `u16`s instead of reinterpreted in place (the `Vec<u8>`
      is not 2-byte aligned, so that was undefined behaviour), and NUL-
      terminated before `ExpandEnvironmentStringsW`.
    - `win/procthreadattr.rs` — the attribute list buffer is `Vec<usize>`, so
      it has the alignment `PROC_THREAD_ATTRIBUTE_LIST` requires.
    - `win/conpty.rs` — a pty size above `i16::MAX` is an error instead of
      wrapping into a negative `COORD`.
    - `lib.rs` — a pid that does not fit `pid_t` is an `InvalidInput` error
      from `kill()` instead of a wrapped cast.
    - `win/pseudocon.rs` — `lazy_static!` replaced by `std::sync::LazyLock`
      (the `lazy_static` dependency is gone).
    - Public `CommandBuilder` / `ExitStatus` accessors gained `#[must_use]` and
      `const`; the internal unix and Windows `spawn_command` helpers take
      `&CommandBuilder`.
