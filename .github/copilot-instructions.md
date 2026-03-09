# Freminal — Copilot Instructions

Freminal is a Rust terminal emulator (Edition 2024, MSRV 1.92.0) using egui/glow
for rendering. It is a Cargo workspace with five crates.

## Architecture

Lock-free rendering: PTY thread owns TerminalEmulator exclusively, publishes
TerminalSnapshot via ArcSwap. GUI thread reads snapshots atomically — no locks.
Input flows through crossbeam channels (InputEvent, PtyWrite).

Crate dependency order: freminal-common < freminal-buffer <
freminal-terminal-emulator < freminal (GUI binary). Plus xtask for CI.

## Key Rules

- No `unwrap()` / `expect()` in production code (only in tests)
- No `unsafe` unless explicitly requested
- No `anyhow` in library crates
- All public APIs must have tests
- Changes must preserve the lock-free architecture
- Enforced: `#![deny(clippy::unwrap_used, clippy::expect_used)]`

## Build & Test

- Full CI: `cargo xtask ci`
- Tests: `cargo test --all`
- Lint: `cargo clippy --all-targets --all-features -- -D warnings`
- Unused deps: `cargo-machete`
- Benchmarks: `cargo bench --all`

## Reference

See `agents.md` in the project root for full agent instructions, architecture
details, crate-specific guidance, and coding standards.
