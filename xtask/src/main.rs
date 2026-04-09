// Copyright (C) 2024-2026 Fred Clausen and the ratatui project contributors
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Build and CI orchestration tool for the Freminal workspace.
//!
//! This is a `cargo xtask` binary — run it with `cargo xtask <subcommand>`.
//! It provides subcommands for building, testing, linting, formatting,
//! generating coverage reports, and checking dependencies.
//!
//! This crate is **not** production code. `anyhow`/`color-eyre` are
//! acceptable here.

#![deny(
    clippy::pedantic,
    clippy::nursery,
    clippy::style,
    clippy::correctness,
    clippy::all
)]

use std::{fmt::Debug, io, process::Output, vec};

use cargo_metadata::MetadataCommand;
use clap::{Parser, Subcommand};
use clap_verbosity_flag::{InfoLevel, Verbosity};
use color_eyre::{eyre::Context, Result};
use duct::cmd;
use tracing::level_filters::LevelFilter;
use tracing_log::AsTrace;

fn main() -> Result<()> {
    color_eyre::install()?;
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_max_level(args.log_level())
        .without_time()
        .init();

    match args.run() {
        Ok(()) => (),
        Err(err) => {
            tracing::error!("{err}");
            std::process::exit(1);
        }
    }
    Ok(())
}

/// CLI argument parser for `cargo xtask`.
#[derive(Debug, Parser)]
#[command(bin_name = "cargo xtask", styles = clap_cargo::style::CLAP_STYLING)]
struct Args {
    /// The subcommand to run.
    #[command(subcommand)]
    command: Command,

    /// Verbosity level; repeat `-v` to increase, `-q` to decrease.
    #[command(flatten)]
    verbosity: Verbosity<InfoLevel>,
}

impl Args {
    fn run(self) -> Result<()> {
        self.command.run()
    }

    fn log_level(&self) -> LevelFilter {
        self.verbosity.log_level_filter().as_trace()
    }
}

#[derive(Clone, Debug, Subcommand)]
enum Command {
    /// Run CI checks (lint, deny, machete, build, test, bench compile)
    ///
    /// This is the **full** CI pipeline intended for GitHub Actions.  For
    /// pre-commit hooks, use `cargo xtask precommit` instead — it skips checks
    /// already covered by separate pre-commit hooks (clippy, rustfmt, etc.).
    CI,

    /// Build the project
    #[command(visible_alias = "b")]
    Build,

    /// Run cargo check
    #[command(visible_alias = "c")]
    Check,

    /// Check if README.md is up-to-date
    #[command(visible_alias = "cr")]
    CheckReadme,

    /// Generate code coverage report
    #[command(visible_alias = "cov")]
    Coverage,

    /// Check dependencies
    #[command(visible_alias = "cd")]
    Deny,

    /// Check for unused dependencies with `cargo-machete`
    #[command(visible_alias = "m")]
    Machete,

    /// Lint formatting, typos, clippy, and docs
    #[command(visible_alias = "l")]
    Lint,

    /// Run clippy on the project
    #[command(visible_alias = "cl")]
    LintClippy,

    /// Check documentation for errors and warnings
    #[command(visible_alias = "d")]
    LintDocs,

    /// Check for formatting issues in the project
    #[command(visible_alias = "lf")]
    LintFormatting,

    /// Lint markdown files
    #[command(visible_alias = "md")]
    LintMarkdown,

    /// Check for typos in the project
    #[command(visible_alias = "lt")]
    LintTypos,

    /// Fix clippy warnings in the project
    #[command(visible_alias = "fc")]
    FixClippy,

    /// Fix formatting issues in the project
    #[command(visible_alias = "fmt")]
    FixFormatting,

    /// Fix typos in the project
    #[command(visible_alias = "typos")]
    FixTypos,

    /// Run tests
    #[command(visible_alias = "t")]
    Test,

    /// Run doc tests
    #[command(visible_alias = "td")]
    TestDocs,

    /// Run lib tests
    #[command(visible_alias = "tl")]
    TestLibs,

    /// Compile all benchmarks without running them
    #[command(visible_alias = "bc")]
    BenchCompile,

    /// Lightweight pre-commit check (test + machete + default-features test)
    ///
    /// Runs only the checks that are NOT already covered by separate
    /// pre-commit hooks (clippy, rustfmt, codespell, markdownlint, prettier).
    /// Use this instead of `ci` in the pre-commit hook for faster commits.
    #[command(visible_alias = "pc")]
    Precommit,
}

impl Command {
    fn run(self) -> Result<()> {
        match self {
            Self::CI => ci(),
            Self::Build => build(),
            Self::Check => check(),
            Self::Deny => deny(),
            Self::Machete => machete(),
            Self::CheckReadme => check_readme(),
            Self::Coverage => coverage(),
            Self::Lint => lint(),
            Self::LintClippy => lint_clippy(),
            Self::LintDocs => lint_docs(),
            Self::LintFormatting => lint_format(),
            Self::LintTypos => lint_typos(),
            Self::LintMarkdown => lint_markdown(),
            Self::FixClippy => fix_clippy(),
            Self::FixFormatting => fix_format(),
            Self::FixTypos => fix_typos(),
            Self::Test => test(),
            Self::TestDocs => test_docs(),
            Self::TestLibs => test_libs(),
            Self::BenchCompile => bench_compile(),
            Self::Precommit => precommit(),
        }
    }
}

/// Run CI checks (lint, deny, machete, build, test, bench compile)
///
/// ## Step ordering rationale
///
/// 1. `lint` — fast feedback on style, typos, and clippy warnings before
///    spending time building or running tests.
/// 2. `deny` — checks license compatibility and known-vulnerable dependencies;
///    fails fast if a new dep violates policy.
/// 3. `machete` — detects unused dependencies that would otherwise silently
///    inflate compile times and binary size.
/// 4. `build` — validates that the workspace compiles cleanly with all targets
///    and features; must pass before tests are meaningful.
/// 5. `test` — runs lib + doc tests only after the build is confirmed clean.
/// 6. `bench_compile` — compiles all benchmarks without running them;
///    catches benchmark compilation failures before the next manual bench run.
/// 7. `test_default_features` — runs clippy and tests again with **no** optional
///    features enabled. This catches `#[cfg(feature = "…")]` gating errors
///    where code behind a feature flag accidentally leaks into the default
///    build. (The previous steps use `--all-features`.)
fn ci() -> Result<()> {
    lint()?;
    deny()?;
    machete()?;
    build()?;
    test()?;
    bench_compile()?;
    test_default_features()?;
    Ok(())
}

/// Check license compatibility and known-vulnerable dependencies with `cargo-deny`
fn deny() -> Result<()> {
    run_cargo(vec!["deny", "check"])
}

/// Check for unused dependencies with `cargo-machete`
fn machete() -> Result<()> {
    cmd!("cargo-machete").run_with_trace()?;
    Ok(())
}

/// Build the project
fn build() -> Result<()> {
    run_cargo(vec!["build", "--all-targets", "--all-features"])
}

/// Run cargo check
fn check() -> Result<()> {
    run_cargo(vec!["check", "--all-targets", "--all-features"])
}

/// Run cargo-rdme to check if README.md is up-to-date with the library documentation
fn check_readme() -> Result<()> {
    run_cargo(vec!["rdme", "--workspace-project", "freminal", "--check"])
}

/// Generate code coverage report
fn coverage() -> Result<()> {
    run_cargo(vec![
        "llvm-cov",
        "--lcov",
        "--output-path",
        "target/lcov.info",
        "--all-features",
    ])
}

/// Lint formatting, typos, clippy, and docs (and a soft fail on markdown)
fn lint() -> Result<()> {
    lint_clippy()?;
    lint_docs()?;
    lint_format()?;
    lint_typos()?;
    if let Err(err) = lint_markdown() {
        tracing::warn!("known issue: markdownlint is currently noisy and can be ignored: {err}");
    }
    Ok(())
}

/// Run clippy on the project
fn lint_clippy() -> Result<()> {
    run_cargo(vec![
        "clippy",
        "--all-targets",
        "--all-features",
        "--",
        "-D",
        "warnings",
    ])
}

/// Fix clippy warnings in the project
fn fix_clippy() -> Result<()> {
    run_cargo(vec![
        "clippy",
        "--all-targets",
        "--all-features",
        "--fix",
        "--allow-dirty",
        "--allow-staged",
        "--",
        "-D",
        "warnings",
    ])
}

/// Check that docs build without errors using flags for docs.rs
///
/// `docs-rs` is a third-party cargo subcommand that simulates the docs.rs
/// build environment, running `cargo doc` with the same feature flags and
/// `RUSTDOCFLAGS` that docs.rs uses.  It is a **soft dependency**: if the
/// binary is not installed the check is skipped with a warning rather than
/// failing CI.  This allows contributors without the tool installed to run
/// `cargo xtask ci` locally without errors.
///
/// The function iterates over all workspace default packages because docs.rs
/// builds each crate independently — a doc error in one crate may not surface
/// when building the whole workspace at once.
///
/// `run_cargo_nightly` is used because docs.rs requires the nightly toolchain
/// to resolve intra-doc links and generate the `--document-private-items`
/// output that docs.rs produces.
fn lint_docs() -> Result<()> {
    // ensure docs-rs is installed, if not, just return Ok(())
    if cmd!("docs-rs").run().is_err() {
        tracing::warn!("docs-rs is not installed, skipping lint_docs.");
        return Ok(());
    }

    let meta = MetadataCommand::new()
        .exec()
        .wrap_err("failed to get cargo metadata")?;
    for package in meta.workspace_default_packages() {
        run_cargo_nightly(vec!["docs-rs", "--package", &package.name])?;
    }
    Ok(())
}

/// Lint formatting issues in the project
fn lint_format() -> Result<()> {
    run_cargo_nightly(vec!["fmt", "--all", "--check"])
}

/// Fix formatting issues in the project
fn fix_format() -> Result<()> {
    run_cargo_nightly(vec!["fmt", "--all"])
}

/// Lint markdown files using [markdownlint-cli2](https://github.com/DavidAnson/markdownlint-cli2)
fn lint_markdown() -> Result<()> {
    cmd!("markdownlint-cli2", "**/*.md", "!target", "!**/target").run_with_trace()?;

    Ok(())
}

/// Check for typos in the project using [typos-cli](https://github.com/crate-ci/typos/)
fn lint_typos() -> Result<()> {
    cmd!("typos").run_with_trace()?;
    Ok(())
}

/// Fix typos in the project
fn fix_typos() -> Result<()> {
    cmd!("typos", "-w").run_with_trace()?;
    Ok(())
}

/// Run tests for libs and docs
fn test() -> Result<()> {
    test_libs()?;
    test_docs()?;
    Ok(())
}

/// Run doc tests for the workspace's default packages
fn test_docs() -> Result<()> {
    run_cargo(vec!["test", "--doc", "--all-features"])
}

/// Run lib tests for the workspace's default packages
fn test_libs() -> Result<()> {
    run_cargo(vec!["test", "--all-targets", "--all-features"])
}

/// Compile all benchmarks without running them.
///
/// This catches benchmark compilation failures early, without the cost of
/// actually executing the benchmarks. Benchmark results on shared CI runners
/// are meaningless due to noisy neighbors, so only compilation is verified.
fn bench_compile() -> Result<()> {
    run_cargo(vec!["bench", "--no-run", "--all"])
}

/// Run clippy and tests with **default features only** (no `--all-features`).
///
/// This is the complement to the main CI steps which use `--all-features`.
/// It catches `#[cfg(feature = "…")]` gating errors where code behind an
/// optional feature flag accidentally leaks into or breaks the default build.
/// For example, the `playback` feature gates recording/playback code; this
/// step ensures the workspace compiles and tests pass without it.
fn test_default_features() -> Result<()> {
    tracing::info!("running default-features pass (no optional features)");
    // Unset CARGO_BUILD_FEATURES so the Nix devshell's "playback" default
    // doesn't leak into this pass — we need to verify the code compiles and
    // passes tests with NO optional features enabled.
    run_cargo_no_features(vec!["clippy", "--all-targets", "--", "-D", "warnings"])?;
    run_cargo_no_features(vec!["test", "--all-targets"])?;
    run_cargo_no_features(vec!["test", "--doc"])?;
    Ok(())
}

/// Lightweight pre-commit check.
///
/// The separate pre-commit hooks already cover clippy, rustfmt, codespell,
/// markdownlint, and prettier.  This subcommand runs only the checks that
/// those hooks do NOT cover:
///
/// 1. `test` — lib + doc tests with all features.
/// 2. `machete` — unused dependency detection (fast).
///
/// Skipped (handled by other hooks or too slow for pre-commit):
/// - clippy (separate hook)
/// - rustfmt (separate hook)
/// - typos / codespell (separate hook)
/// - markdownlint (separate hook)
/// - `lint_docs` / docs-rs (slow, CI-only)
/// - cargo-deny (slow, CI-only)
/// - cargo build (redundant — tests compile everything)
/// - `bench_compile` (slow, CI-only)
/// - `test_default_features` (slow, CI-only — catches feature gating errors)
fn precommit() -> Result<()> {
    test()?;
    machete()?;
    Ok(())
}

/// Run a cargo subcommand with `CARGO_BUILD_FEATURES` removed from the
/// environment, ensuring no optional features leak in from the devshell.
fn run_cargo_no_features(args: Vec<&str>) -> Result<()> {
    cmd("cargo", args)
        .env_remove("CARGO_BUILD_FEATURES")
        .run_with_trace()?;
    Ok(())
}

/// Run a cargo subcommand with the default toolchain
fn run_cargo(args: Vec<&str>) -> Result<()> {
    cmd("cargo", args).run_with_trace()?;
    Ok(())
}

/// Run a cargo subcommand with the nightly toolchain
///
/// The nightly toolchain is requested by setting the `RUSTUP_TOOLCHAIN=nightly`
/// environment variable rather than a `+nightly` flag on the cargo invocation.
/// This is necessary because `cargo xtask` is itself a cargo subcommand: the
/// `CARGO` environment variable is set by the outer `cargo` process to the path
/// of the stable binary, which `duct::cmd("cargo", …)` inherits.  Passing
/// `+nightly` as an argument would be parsed by the *outer* cargo, not by the
/// inner invocation.  Removing `CARGO` and setting `RUSTUP_TOOLCHAIN` bypasses
/// this and lets rustup dispatch to the nightly binary directly.
///
/// ## Flag choices for `fix_clippy`
///
/// `--fix` — applies clippy's machine-applicable suggestions automatically.
/// `--allow-dirty` — required when the working tree has unstaged changes;
///   without it cargo refuses to modify files it cannot safely roll back.
/// `--allow-staged` — required when some changes are already staged;
///   prevents cargo from refusing to run when the index and worktree differ.
/// `-D warnings` — ensures that `fix_clippy` targets the same strictness
///   level as `lint_clippy`, so all auto-fixed suggestions are ones that
///   would have been errors under CI.
fn run_cargo_nightly(args: Vec<&str>) -> Result<()> {
    cmd("cargo", args)
        // CARGO env var is set because we're running in a cargo subcommand
        .env_remove("CARGO")
        .env("RUSTUP_TOOLCHAIN", "nightly")
        .run_with_trace()?;
    Ok(())
}

/// An extension trait for `duct::Expression` that logs the command being run
/// before running it.
trait ExpressionExt {
    /// Run the command and log the command being run
    fn run_with_trace(&self) -> io::Result<Output>;
}

impl ExpressionExt for duct::Expression {
    fn run_with_trace(&self) -> io::Result<Output> {
        tracing::info!("running command: {:?}", self);
        self.run().inspect_err(|_| {
            // The command that was run may have scrolled off the screen, so repeat it here
            tracing::error!("failed to run command: {:?}", self);
        })
    }
}
