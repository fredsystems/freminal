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

use std::{fmt::Debug, fs, io, process::Output, vec};

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

    /// Lightweight pre-commit check (test + machete)
    ///
    /// Runs only the checks that are NOT already covered by separate
    /// pre-commit hooks (clippy, rustfmt, codespell, markdownlint, prettier).
    /// Use this instead of `ci` in the pre-commit hook for faster commits.
    #[command(visible_alias = "pc")]
    Precommit,

    /// Bump the version string in all locations (Cargo.toml, flake.nix)
    ///
    /// Updates the workspace version in `Cargo.toml` and all version strings
    /// in `flake.nix` (package version + macOS Info.plist bundle versions).
    #[command(visible_alias = "bv")]
    BumpVersion {
        /// The new version string (e.g. "0.8.0")
        version: String,
    },

    /// Build Linux packages locally with a portable ELF interpreter.
    ///
    /// Inside the Nix dev shell the Rust toolchain links the binary against
    /// the Nix-store glibc and stamps its ELF interpreter as
    /// `/nix/store/.../ld-linux-x86-64.so.2`.  That path exists only on a Nix
    /// system, so a locally-built rpm/deb/AppImage fails to execute on a
    /// normal distro with "cannot execute: required file not found".  This
    /// subcommand builds the packages and resets the embedded binary's
    /// interpreter to the standard `/lib64/ld-linux-x86-64.so.2` (and strips
    /// the Nix-store rpath) so the artifacts run on glibc distros.
    ///
    /// This is a **local testing aid only**.  The release CI builds on a
    /// plain Ubuntu runner with rustup, which already links against the
    /// system loader, so CI artifacts never need this.
    #[command(visible_alias = "pl")]
    PackageLocal {
        /// Which package format(s) to build (default: all).
        #[arg(value_enum)]
        formats: Vec<LocalPackageFormat>,
    },
}

/// Linux package formats that [`package_local`] can produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum LocalPackageFormat {
    /// `.rpm` via `cargo-generate-rpm`.
    Rpm,
    /// `.deb` via `cargo-bundle`.
    Deb,
    /// `.AppImage` via `cargo-bundle`.
    Appimage,
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
            Self::BumpVersion { version } => bump_version(&version),
            Self::PackageLocal { formats } => package_local(&formats),
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
///
/// Uses explicit target flags instead of `--all-targets` to exclude
/// benchmarks.  `--all-targets` is equivalent to `--lib --bins --tests
/// --benches --examples`; including `--benches` causes Criterion harnesses
/// to compile and run in test mode, which is unnecessary overhead —
/// especially in the pre-commit hook.  Benchmark compilation is verified
/// separately by `bench_compile()`.
fn test_libs() -> Result<()> {
    run_cargo(vec![
        "test",
        "--lib",
        "--bins",
        "--tests",
        "--examples",
        "--all-features",
    ])
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
    run_cargo_no_features(vec!["test", "--lib", "--bins", "--tests", "--examples"])?;
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

/// Bump the version string in all locations across the workspace.
///
/// Updates:
/// 1. `Cargo.toml` — `[workspace.package] version = "…"`
/// 2. `flake.nix` — `version = "…"` (nix package version)
/// 3. `flake.nix` — `CFBundleVersion` and `CFBundleShortVersionString` in the
///    macOS Info.plist heredoc
///
/// The function reads the current version from `Cargo.toml` and replaces
/// matching occurrences with the new version. Missing expected occurrences
/// are not ignored silently: the command emits warnings for files that do
/// not contain the old version string and then continues successfully.
/// Translate a `SemVer` version into an RPM-legal `Version` string.
///
/// RPM's `Version` field forbids the hyphen `-` (it is the separator between
/// the `Version` and `Release` components of a `NEVRA`), but `SemVer` uses `-`
/// to introduce the pre-release identifier. RPM does, however, understand the
/// tilde operator `~`, which sorts *older* than the same version without it —
/// exactly mirroring `SemVer` pre-release ordering (`0.9.0~beta.2` < `0.9.0`).
///
/// The translation is:
///
/// * the first `-` (the `SemVer` pre-release separator) becomes `~`;
/// * any further `-` (hyphens permitted *inside* `SemVer` identifiers, e.g.
///   `0.9.0-x-y`) become `_`, which is RPM-legal and ordering-neutral;
/// * everything else is left untouched (`.`, `+` build metadata, and ASCII
///   alphanumerics are all valid in an RPM `Version`).
///
/// The input is first parsed with [`semver::Version::parse`], so it is
/// guaranteed to contain only the `SemVer`-legal character set
/// (`0-9A-Za-z.-+`). That guarantee is what makes this translation total: the
/// only RPM-illegal character a parsed `SemVer` can contain is `-`, and both
/// occurrences of it are handled above. Any genuinely malformed input (e.g.
/// containing `&`) is rejected by the parse before translation.
fn rpm_version(new_version: &str) -> Result<String> {
    // Parsing rejects anything that is not well-formed SemVer, which also
    // rejects any character outside the SemVer-legal set.
    semver::Version::parse(new_version)
        .wrap_err_with(|| format!("invalid semver version: {new_version}"))?;

    let mut out = String::with_capacity(new_version.len());
    let mut seen_hyphen = false;
    for ch in new_version.chars() {
        if ch == '-' {
            if seen_hyphen {
                // A hyphen inside an identifier — RPM-illegal, map to '_'.
                out.push('_');
            } else {
                // The pre-release separator — map to the tilde operator.
                out.push('~');
                seen_hyphen = true;
            }
        } else {
            out.push(ch);
        }
    }
    Ok(out)
}

fn bump_version(new_version: &str) -> Result<()> {
    // Validate that the new version is valid semver, and derive the RPM-legal
    // form up front so an invalid version fails before any files are touched.
    semver::Version::parse(new_version)
        .wrap_err_with(|| format!("invalid semver version: {new_version}"))?;
    let new_rpm_version = rpm_version(new_version)?;

    // Read current version from Cargo.toml.
    let cargo_toml_path = "Cargo.toml";
    let cargo_content = fs::read_to_string(cargo_toml_path)
        .wrap_err_with(|| format!("failed to read {cargo_toml_path}"))?;

    // Extract current version from the workspace.package section.
    let old_version = cargo_content
        .lines()
        .find(|line| line.starts_with("version = \""))
        .and_then(|line| line.strip_prefix("version = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or_else(|| {
            color_eyre::eyre::eyre!("could not find 'version = \"…\"' in {cargo_toml_path}")
        })?
        .to_owned();

    if old_version == new_version {
        tracing::info!("version is already {new_version}, nothing to do");
        // Still reconcile the RPM Version override in case it has drifted out
        // of sync with the workspace version (the override is RPM-illegal to
        // leave stale, and `cargo generate-rpm` reads it instead of the
        // workspace version).
        update_rpm_version("freminal/Cargo.toml", &new_rpm_version)?;
        return Ok(());
    }

    tracing::info!("bumping version: {old_version} -> {new_version}");

    // 1. Update Cargo.toml
    let old_cargo_line = format!("version = \"{old_version}\"");
    let new_cargo_line = format!("version = \"{new_version}\"");

    let updated_cargo = cargo_content.replacen(&old_cargo_line, &new_cargo_line, 1);
    color_eyre::eyre::ensure!(
        updated_cargo != cargo_content,
        "failed to replace version in {cargo_toml_path}"
    );
    fs::write(cargo_toml_path, &updated_cargo)
        .wrap_err_with(|| format!("failed to write {cargo_toml_path}"))?;
    tracing::info!("  updated {cargo_toml_path}");

    // 2. Update flake.nix — all occurrences of the old version.
    let flake_path = "flake.nix";
    let flake_content =
        fs::read_to_string(flake_path).wrap_err_with(|| format!("failed to read {flake_path}"))?;

    let updated_flake = flake_content.replace(&old_version, new_version);
    let replacements = flake_content.matches(&old_version).count();
    if replacements == 0 {
        tracing::warn!("no version strings found in {flake_path} — is it already updated?");
    } else {
        fs::write(flake_path, &updated_flake)
            .wrap_err_with(|| format!("failed to write {flake_path}"))?;
        tracing::info!("  updated {flake_path} ({replacements} occurrences)");
    }

    // 3. Update the RPM `Version` override in freminal/Cargo.toml.
    //
    // RPM forbids '-' in its Version field, so `cargo generate-rpm` reads this
    // tilde-translated override from `[package.metadata.generate-rpm].version`
    // instead of the workspace SemVer version.
    update_rpm_version("freminal/Cargo.toml", &new_rpm_version)?;

    tracing::info!("version bump complete: {old_version} -> {new_version}");
    tracing::info!("  rpm Version override: {new_rpm_version}");
    tracing::info!("remember to run `cargo check` to update Cargo.lock");

    Ok(())
}

/// Rewrite the `version = "…"` line under `[package.metadata.generate-rpm]`
/// in the given manifest to `new_rpm_version`.
///
/// The package's own version is set via `version.workspace = true`, so the
/// only literal `version = "…"` line in `freminal/Cargo.toml` is the RPM
/// override; this locates the `[package.metadata.generate-rpm]` table and
/// rewrites the first `version = "…"` line that follows it.
fn update_rpm_version(manifest_path: &str, new_rpm_version: &str) -> Result<()> {
    let content = fs::read_to_string(manifest_path)
        .wrap_err_with(|| format!("failed to read {manifest_path}"))?;

    let mut in_rpm_section = false;
    let mut replaced = false;
    let mut updated = String::with_capacity(content.len());

    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            in_rpm_section = trimmed.starts_with("[package.metadata.generate-rpm]");
        }

        if in_rpm_section && !replaced && trimmed.starts_with("version = \"") {
            updated.push_str("version = \"");
            updated.push_str(new_rpm_version);
            updated.push('"');
            replaced = true;
        } else {
            updated.push_str(line);
        }
        updated.push('\n');
    }

    color_eyre::eyre::ensure!(
        replaced,
        "could not find a 'version = \"…\"' line under \
         [package.metadata.generate-rpm] in {manifest_path}"
    );

    // Preserve the absence of a trailing newline if the original lacked one.
    let final_content = if content.ends_with('\n') {
        updated
    } else {
        updated.trim_end_matches('\n').to_owned()
    };

    fs::write(manifest_path, final_content)
        .wrap_err_with(|| format!("failed to write {manifest_path}"))?;
    tracing::info!("  updated {manifest_path} (rpm Version = {new_rpm_version})");
    Ok(())
}

/// The standard glibc ELF interpreter path on x86-64 Linux distros.
///
/// Nix-built binaries point their interpreter at a `/nix/store/...` glibc that
/// does not exist on a normal distro; resetting it to this canonical path is
/// what makes a locally-built artifact portable.
const PORTABLE_INTERPRETER: &str = "/lib64/ld-linux-x86-64.so.2";

/// The release binary path that every Linux packager reads from.
const RELEASE_BINARY: &str = "target/release/freminal";

/// Build Linux packages locally with a portable ELF interpreter.
///
/// See the [`Command::PackageLocal`] documentation for the why.  The mechanics
/// differ per format because of *when* the binary is read:
///
/// * `cargo-generate-rpm` copies `target/release/freminal` verbatim and does
///   **not** rebuild, so the binary is patched in place *before* packaging.
/// * `cargo-bundle` runs its own `cargo build` (which relinks and restores the
///   Nix interpreter) before copying, so pre-patching is futile; instead the
///   binary embedded *inside* the built `.deb` / `.AppImage` is patched and the
///   artifact repacked.
fn package_local(formats: &[LocalPackageFormat]) -> Result<()> {
    let formats: Vec<LocalPackageFormat> = if formats.is_empty() {
        vec![
            LocalPackageFormat::Rpm,
            LocalPackageFormat::Deb,
            LocalPackageFormat::Appimage,
        ]
    } else {
        formats.to_vec()
    };

    ensure_tool("patchelf")?;

    // A clean release build first so every packager sees the same binary.
    run_cargo(vec!["build", "--release", "-p", "freminal"])?;

    let want_rpm = formats.contains(&LocalPackageFormat::Rpm);
    let want_deb = formats.contains(&LocalPackageFormat::Deb);
    let want_appimage = formats.contains(&LocalPackageFormat::Appimage);

    // RPM path: patch the binary in place, then generate-rpm (no rebuild).
    if want_rpm {
        ensure_tool("cargo-generate-rpm")?;
        patch_binary_in_place(RELEASE_BINARY)?;
        cmd!("cargo-generate-rpm", "-p", "freminal").run_with_trace()?;
        tracing::info!("rpm: target/generate-rpm/*.rpm (interpreter patched)");
    }

    // Bundle path: cargo-bundle relinks, so patch the embedded binary after.
    if want_deb || want_appimage {
        ensure_tool("cargo-bundle")?;
        run_cargo(vec!["bundle", "--release", "-p", "freminal"])?;

        if want_deb {
            ensure_tool("dpkg-deb")?;
            patch_deb_interpreter("target/release/bundle/deb")?;
        }
        if want_appimage {
            ensure_tool("unsquashfs")?;
            ensure_tool("mksquashfs")?;
            patch_appimage_interpreter("target/release/bundle/appimage")?;
        }
    }

    Ok(())
}

/// Fail with a clear message if a required external tool is not on `PATH`.
///
/// Per the flake-dev-shell-discipline, missing tools indicate an incomplete
/// dev shell, not broken logic — so this surfaces the gap instead of trying to
/// work around it.  A std-only `PATH` scan avoids adding a dependency just to
/// locate an executable.
fn ensure_tool(tool: &str) -> Result<()> {
    let path =
        std::env::var_os("PATH").ok_or_else(|| color_eyre::eyre::eyre!("PATH is not set"))?;
    let found = std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(tool);
        candidate.is_file() && is_executable(&candidate)
    });
    color_eyre::eyre::ensure!(found, "required tool `{tool}` not found on PATH");
    Ok(())
}

/// Whether a path is executable by the current user (Unix); on other platforms
/// existence as a file is treated as sufficient.
#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &std::path::Path) -> bool {
    true
}

/// Reset a binary's ELF interpreter to the portable path and strip its rpath.
///
/// patchelf cannot rewrite a file that is hardlinked / busy in place reliably,
/// so the edit is done on a copy that is then moved over the original.
fn patch_binary_in_place(path: &str) -> Result<()> {
    let before = cmd!("patchelf", "--print-interpreter", path)
        .read()
        .wrap_err_with(|| format!("failed to read interpreter of {path}"))?;
    tracing::info!("patching {path} (was {})", before.trim());

    let tmp = format!("{path}.patched");
    fs::copy(path, &tmp).wrap_err_with(|| format!("failed to copy {path}"))?;
    cmd!("patchelf", "--set-interpreter", PORTABLE_INTERPRETER, &tmp).run_with_trace()?;
    cmd!("patchelf", "--remove-rpath", &tmp).run_with_trace()?;
    fs::rename(&tmp, path).wrap_err_with(|| format!("failed to replace {path}"))?;
    Ok(())
}

/// Find the single artifact with the given extension in a bundle output dir.
fn single_artifact(dir: &str, extension: &str) -> Result<std::path::PathBuf> {
    let mut found = None;
    for entry in fs::read_dir(dir).wrap_err_with(|| format!("failed to read {dir}"))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some(extension) {
            if found.is_some() {
                color_eyre::eyre::bail!("multiple .{extension} artifacts in {dir}");
            }
            found = Some(path);
        }
    }
    found.ok_or_else(|| color_eyre::eyre::eyre!("no .{extension} artifact in {dir}"))
}

/// Patch the interpreter of the binary inside a built `.deb` and repack it.
///
/// `dpkg-deb -R` / `-b` round-trips the package and regenerates `md5sums`, so
/// rewriting `usr/bin/freminal` in the extracted tree and rebuilding is
/// sufficient. `--root-owner-group` keeps files owned by root:root.
fn patch_deb_interpreter(dir: &str) -> Result<()> {
    let deb = single_artifact(dir, "deb")?;
    let deb = deb.to_string_lossy().into_owned();
    let workdir = tempfile::tempdir().wrap_err("failed to create tempdir")?;
    let work = workdir.path().to_string_lossy().into_owned();

    cmd!("dpkg-deb", "-R", &deb, &work).run_with_trace()?;
    let bin = format!("{work}/usr/bin/freminal");
    patch_binary_in_place(&bin)?;
    cmd!("dpkg-deb", "--root-owner-group", "-b", &work, &deb).run_with_trace()?;
    tracing::info!("deb: {deb} (interpreter patched)");
    Ok(())
}

/// Patch the interpreter of the binary inside a built `.AppImage` and repack.
///
/// `cargo-bundle`'s `AppImage` is a type-2 ELF runtime header followed by a
/// squashfs payload.  The runtime does not implement `--appimage-extract`, so
/// the payload is operated on directly: the squashfs offset is the end of the
/// runtime ELF (section-header table is last in this runtime, so the payload
/// begins at `e_shoff + e_shnum * e_shentsize`), the runtime header bytes are
/// split off verbatim, the payload is `unsquashfs`-ed, the binary patched, and
/// the tree re-`mksquashfs`-ed and re-prepended.  This mirrors the existing
/// `assets/ci/fix-linux-icon-metadata.sh` `AppImage` logic.
fn patch_appimage_interpreter(dir: &str) -> Result<()> {
    let appimage = single_artifact(dir, "AppImage")?;
    let appimage = appimage.to_string_lossy().into_owned();

    let bytes = fs::read(&appimage).wrap_err_with(|| format!("failed to read {appimage}"))?;
    color_eyre::eyre::ensure!(
        bytes.get(0..4) == Some(b"\x7fELF"),
        "AppImage does not start with an ELF runtime header"
    );
    // 64-bit ELF header fields: e_shoff @ 0x28 (u64), e_shentsize @ 0x3A (u16),
    // e_shnum @ 0x3C (u16).
    let e_shoff = u64::from_le_bytes(bytes[0x28..0x30].try_into()?);
    let e_shentsize = u16::from_le_bytes(bytes[0x3A..0x3C].try_into()?);
    let e_shnum = u16::from_le_bytes(bytes[0x3C..0x3E].try_into()?);
    let offset = e_shoff + u64::from(e_shentsize) * u64::from(e_shnum);
    let offset_usize = usize::try_from(offset).wrap_err("squashfs offset overflows usize")?;

    color_eyre::eyre::ensure!(
        bytes.get(offset_usize..offset_usize + 4) == Some(b"hsqs"),
        "no squashfs magic at computed offset {offset}"
    );

    let workdir = tempfile::tempdir().wrap_err("failed to create tempdir")?;
    let work = workdir.path();
    let runtime = work.join("runtime");
    fs::write(&runtime, &bytes[..offset_usize]).wrap_err("failed to write runtime header")?;

    let appdir = work.join("squashfs-root");
    cmd!(
        "unsquashfs",
        "-o",
        offset.to_string(),
        "-d",
        appdir.to_string_lossy().as_ref(),
        &appimage
    )
    .run_with_trace()?;

    let bin = appdir.join("usr/bin/freminal");
    patch_binary_in_place(bin.to_string_lossy().as_ref())?;

    let payload = work.join("payload.squashfs");
    cmd!(
        "mksquashfs",
        appdir.to_string_lossy().as_ref(),
        payload.to_string_lossy().as_ref(),
        "-root-owned",
        "-noappend",
        "-quiet"
    )
    .run_with_trace()?;

    // Concatenate runtime header + new payload back into the AppImage.
    let mut out = fs::read(&runtime)?;
    out.extend_from_slice(&fs::read(&payload)?);
    fs::write(&appimage, &out).wrap_err_with(|| format!("failed to rewrite {appimage}"))?;
    set_executable(&appimage)?;
    tracing::info!("appimage: {appimage} (interpreter patched)");
    Ok(())
}

/// Make a file executable (mode |= 0o111) on Unix; no-op elsewhere.
#[cfg(unix)]
fn set_executable(path: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    let mode = perms.mode();
    perms.set_mode(mode | 0o111);
    fs::set_permissions(path, perms).wrap_err_with(|| format!("failed to chmod {path}"))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &str) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::rpm_version;

    #[test]
    fn release_version_is_unchanged() {
        // A plain release version contains no RPM-illegal characters.
        assert_eq!(rpm_version("0.9.0").unwrap(), "0.9.0");
        assert_eq!(rpm_version("1.2.3").unwrap(), "1.2.3");
    }

    #[test]
    fn pre_release_separator_becomes_tilde() {
        assert_eq!(rpm_version("0.9.0-beta.2").unwrap(), "0.9.0~beta.2");
        assert_eq!(rpm_version("0.9.0-rc.1").unwrap(), "0.9.0~rc.1");
        assert_eq!(rpm_version("1.0.0-alpha").unwrap(), "1.0.0~alpha");
    }

    #[test]
    fn nightly_pre_release_becomes_tilde() {
        // The exact case from the failing nightly run.
        assert_eq!(
            rpm_version("0.9.0-beta.2.nightly.20260609").unwrap(),
            "0.9.0~beta.2.nightly.20260609"
        );
        // Base version without a pre-release, nightly appended as a pre-release.
        assert_eq!(
            rpm_version("0.9.0-nightly.20260609").unwrap(),
            "0.9.0~nightly.20260609"
        );
    }

    #[test]
    fn build_metadata_plus_is_preserved() {
        // '+' is RPM-legal and must survive untouched.
        assert_eq!(
            rpm_version("0.9.0-beta.2+build.5").unwrap(),
            "0.9.0~beta.2+build.5"
        );
        assert_eq!(rpm_version("0.9.0+build.5").unwrap(), "0.9.0+build.5");
    }

    #[test]
    fn hyphen_inside_identifier_becomes_underscore() {
        // SemVer permits hyphens inside pre-release identifiers; only the
        // first (separator) hyphen becomes '~', the rest become '_'.
        assert_eq!(rpm_version("0.9.0-x-y").unwrap(), "0.9.0~x_y");
        assert_eq!(rpm_version("0.9.0-a-b-c").unwrap(), "0.9.0~a_b_c");
    }

    #[test]
    fn invalid_semver_is_rejected() {
        // Garbage that is not SemVer at all is rejected before translation,
        // so RPM-illegal characters like '&' can never reach the output.
        assert!(rpm_version("0.9.0-beta&2").is_err());
        assert!(rpm_version("not-a-version").is_err());
        assert!(rpm_version("1.0").is_err());
        assert!(rpm_version("").is_err());
    }
}
