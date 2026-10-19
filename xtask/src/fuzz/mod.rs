// Copyright (C) 2024-2026 Fred Clausen and the ratatui project contributors
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `cargo xtask fuzz*` -- orchestration for the standalone `fuzz/` crate.
//!
//! The fuzz crate is a cargo-fuzz project that is excluded from the root
//! workspace (it needs a pinned nightly toolchain and sanitizer flags). Its
//! tooling only exists in the `fuzz` dev shell (`nix develop .#fuzz`).
//!
//! Directory layout (all relative to the workspace root unless overridden):
//!
//! * `fuzz/seeds/<target>/`       -- curated seeds, committed, **never written**.
//! * `fuzz/dict/*.dict`           -- libFuzzer dictionaries, committed, never written.
//! * `fuzz/regressions/<target>/` -- crash reproducers, committed, never written.
//! * `fuzz/corpus/<target>/`      -- working corpus, gitignored.
//! * `fuzz/artifacts/<target>/`   -- crash artifacts, gitignored.
//!
//! Everything that can be decided without touching the filesystem or running a
//! process (argument construction, sandbox cmdline, path planning, parsing) is a
//! pure function so it can be unit-tested. This file holds the command-line
//! types, the per-invocation context and the subcommand entry points; the
//! pieces they are built from live in the submodules:
//!
//! * [`sandbox`] -- process isolation and the bubblewrap command line;
//! * [`args`] -- libFuzzer arguments for fuzz, replay and merge runs;
//! * [`dictionary`] -- which target uses which `fuzz/dict/*.dict` file;
//! * [`exit`] -- running a fuzz process and classifying how it ended;
//! * [`artifacts`] -- finding artifacts and run summaries;
//! * [`cmin`] -- corpus minimisation planning and the directory swap;
//! * [`corpus_repo`] -- persisting the working corpus in a separate git
//!   repository (`fuzz-corpus pull|push`).

mod args;
mod artifacts;
mod cmin;
mod corpus_repo;
mod dictionary;
mod exit;
mod sandbox;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use cargo_metadata::{MetadataCommand, TargetKind};
use clap::{Args, ValueEnum};
use color_eyre::{
    eyre::{bail, ensure, eyre, Context},
    Result,
};
use duct::cmd;

#[cfg(test)]
pub use self::corpus_repo::{CorpusAction, DEFAULT_REPO_URL};
use self::{
    args::{cmin_args, collect_replay_inputs, fuzz_args, replay_args, FuzzArgsPlan},
    artifacts::{finding_artifacts, findings_summary, new_findings},
    cmin::{
        cmin_preflight, count_files, decide_cmin_swap, plan_cmin, remove_dir_if_exists, swap_cmin,
        CminPreflight, DirPresence, SwapDecision,
    },
    dictionary::dictionary_for_target,
    exit::{run_fuzz_binary, FindingsLocation},
    sandbox::ensure_inputs_visible,
};
pub use self::{corpus_repo::FuzzCorpusArgs, sandbox::Sandbox};

/// What a fuzz run does when libFuzzer finds a crash, OOM or timeout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum CrashPolicy {
    /// Stop at the first finding (libFuzzer's default behaviour). The default.
    #[default]
    Stop,
    /// Keep fuzzing in fork mode, collecting every finding as an artifact.
    #[value(name = "keep-going")]
    KeepGoing,
}

/// Options shared by every fuzz subcommand that runs a fuzz binary.
#[derive(Clone, Debug, Args)]
pub struct FuzzEnvArgs {
    /// How to isolate the fuzz process.
    #[arg(long, value_enum, default_value_t = Sandbox::Bwrap)]
    pub sandbox: Sandbox,

    /// Directory holding the working corpora (`<DIR>/<target>/`).
    #[arg(
        long,
        value_name = "DIR",
        env = "FREMINAL_FUZZ_CORPUS_DIR",
        default_value = "fuzz/corpus"
    )]
    pub corpus_root: PathBuf,

    /// Directory receiving crash artifacts (`<DIR>/<target>/`).
    #[arg(
        long,
        value_name = "DIR",
        env = "FREMINAL_FUZZ_ARTIFACTS_DIR",
        default_value = "fuzz/artifacts"
    )]
    pub artifacts_root: PathBuf,
}

/// Arguments of `cargo xtask fuzz`.
#[derive(Clone, Debug, Args)]
pub struct FuzzRunArgs {
    /// Fuzz target name (see `cargo xtask fuzz-list`).
    pub target: String,

    /// How long to fuzz, in seconds.
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 300,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub time: u64,

    /// What to do when a crash, OOM or timeout is found.
    #[arg(long, value_enum, default_value_t = CrashPolicy::Stop)]
    pub on_crash: CrashPolicy,

    /// Worker processes (only meaningful with `--on-crash keep-going`).
    #[arg(
        long,
        value_name = "N",
        default_value_t = 1,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    pub jobs: u32,

    /// Sandbox and directory options.
    #[command(flatten)]
    pub env: FuzzEnvArgs,
}

/// Arguments of `cargo xtask fuzz-smoke`.
#[derive(Clone, Debug, Args)]
pub struct FuzzSmokeArgs {
    /// Seconds to fuzz each target.
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = 45,
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub time: u64,

    /// Sandbox and directory options.
    #[command(flatten)]
    pub env: FuzzEnvArgs,
}

/// Arguments of `cargo xtask fuzz-replay`.
///
/// Replay reads only committed inputs and writes no seeds, regressions, corpus
/// or artifacts (building the targets writes only `fuzz/target`), so unlike
/// the other commands it takes no corpus or artifacts directory.
#[derive(Clone, Debug, Args)]
pub struct FuzzReplayArgs {
    /// How to isolate the fuzz process.
    #[arg(long, value_enum, default_value_t = Sandbox::Bwrap)]
    pub sandbox: Sandbox,
}

/// Arguments of `cargo xtask fuzz-cmin`.
#[derive(Clone, Debug, Args)]
pub struct FuzzCminArgs {
    /// Fuzz target whose working corpus to minimise.
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    pub target: Option<String>,

    /// Minimise the working corpus of every target.
    // A clap command-line flag: it is converted to `CminSelection` at the
    // boundary (`FuzzCminArgs::selection`) and never travels further.
    #[arg(long)]
    pub all: bool,

    /// Sandbox and directory options.
    #[command(flatten)]
    pub env: FuzzEnvArgs,
}

/// Which targets `fuzz-cmin` operates on.
#[derive(Clone, Debug, PartialEq, Eq)]
enum CminSelection {
    /// Every discovered target.
    All,
    /// One named target.
    One(String),
}

impl FuzzCminArgs {
    /// Convert the parsed flags into a [`CminSelection`].
    fn selection(&self) -> Result<CminSelection> {
        match (&self.target, self.all) {
            (Some(target), false) => Ok(CminSelection::One(target.clone())),
            (None, true) => Ok(CminSelection::All),
            _ => bail!("specify exactly one of <TARGET> or --all"),
        }
    }
}

/// The outcome of one target in a multi-target command.
#[derive(Debug)]
enum Outcome {
    /// The run finished cleanly.
    Passed,
    /// The run failed; the message says why.
    Failed(String),
}

// ---------------------------------------------------------------------------
// Environment discovery
// ---------------------------------------------------------------------------

/// Names of the `bin` targets among `targets` (`(name, kinds)` pairs), in the
/// order given.
fn bin_target_names<'a>(
    targets: impl IntoIterator<Item = (&'a str, &'a [TargetKind])>,
) -> Vec<String> {
    targets
        .into_iter()
        .filter(|(_, kinds)| kinds.contains(&TargetKind::Bin))
        .map(|(name, _)| name.to_owned())
        .collect()
}

/// Extract the host triple from the output of `rustc -vV`.
fn parse_host_triple(rustc_vv: &str) -> Result<String> {
    rustc_vv
        .lines()
        .find_map(|line| line.strip_prefix("host:"))
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| eyre!("no `host:` line in `rustc -vV` output"))
}

/// Location of a built fuzz binary: `<root>/fuzz/target/<triple>/release/<target>`.
fn binary_path(root: &Path, host_triple: &str, target: &str) -> PathBuf {
    root.join("fuzz")
        .join("target")
        .join(host_triple)
        .join("release")
        .join(format!("{target}{}", std::env::consts::EXE_SUFFIX))
}

/// Resolve a possibly-relative path against the workspace root.
fn resolve_against(root: &Path, path: &Path) -> PathBuf {
    root.join(path)
}

/// Everything resolved once per command invocation.
struct FuzzContext {
    /// Workspace root; every relative path is resolved against it.
    root: PathBuf,
    /// Discovered fuzz target names.
    targets: Vec<String>,
    /// The host triple, from `rustc -vV`. Only the commands that run a built
    /// fuzz binary need it, so it is queried on first use (see
    /// [`FuzzContext::host_triple`]); `fuzz-list` never asks for it.
    host_triple: OnceLock<String>,
}

impl FuzzContext {
    /// A context for `root` with the given fuzz `targets`.
    const fn new(root: PathBuf, targets: Vec<String>) -> Self {
        Self {
            root,
            targets,
            host_triple: OnceLock::new(),
        }
    }

    /// Locate the workspace root and read the fuzz targets from
    /// `fuzz/Cargo.toml`. Works on stable; no nightly toolchain is needed.
    fn discover() -> Result<Self> {
        let root = MetadataCommand::new()
            .no_deps()
            .exec()
            .wrap_err("failed to get cargo metadata for the workspace")?
            .workspace_root
            .into_std_path_buf();
        let manifest = root.join("fuzz").join("Cargo.toml");
        let meta = MetadataCommand::new()
            .manifest_path(&manifest)
            .no_deps()
            .exec()
            .wrap_err_with(|| format!("failed to read fuzz targets from {}", manifest.display()))?;
        let targets = bin_target_names(
            meta.packages
                .iter()
                .flat_map(|pkg| pkg.targets.iter())
                .map(|t| (t.name.as_str(), t.kind.as_slice())),
        );
        ensure!(
            !targets.is_empty(),
            "no fuzz targets in {}",
            manifest.display()
        );
        Ok(Self::new(root, targets))
    }

    /// The host triple, resolved once per command on first use.
    fn host_triple(&self) -> Result<&str> {
        if let Some(triple) = self.host_triple.get() {
            return Ok(triple);
        }
        let triple = query_host_triple()?;
        Ok(self.host_triple.get_or_init(|| triple))
    }

    /// Location of the built fuzz binary for `target`.
    fn binary(&self, target: &str) -> Result<PathBuf> {
        Ok(binary_path(&self.root, self.host_triple()?, target))
    }

    /// Fail unless `target` is a known fuzz target.
    fn validate_target(&self, target: &str) -> Result<()> {
        ensure!(
            self.targets.iter().any(|t| t == target),
            "unknown fuzz target `{target}`; available targets: {}",
            self.targets.join(", ")
        );
        Ok(())
    }

    /// Curated, committed seeds of `target`.
    fn seeds_dir(&self, target: &str) -> PathBuf {
        self.root.join("fuzz").join("seeds").join(target)
    }

    /// Committed crash reproducers of `target`.
    fn regressions_dir(&self, target: &str) -> PathBuf {
        self.root.join("fuzz").join("regressions").join(target)
    }
}

/// Create `dir` (and parents) and return its canonical absolute path.
fn prepare_dir(dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir).wrap_err_with(|| format!("failed to create {}", dir.display()))?;
    dir.canonicalize()
        .wrap_err_with(|| format!("failed to canonicalise {}", dir.display()))
}

/// Fail with a pointer at the fuzz dev shell when `tool` does not run.
fn ensure_fuzz_tool(program: &str, args: &[&str], what: &str) -> Result<()> {
    cmd(program, args.iter().copied())
        .stdout_null()
        .stderr_null()
        .run()
        .map(|_| ())
        .map_err(|err| eyre!("`{what}` is not available ({err}); run inside `nix develop .#fuzz`"))
}

/// Verify the tools a fuzz command needs are present.
fn require_tools(sandbox: Sandbox) -> Result<()> {
    ensure_fuzz_tool("cargo", &["fuzz", "--version"], "cargo fuzz")?;
    if sandbox == Sandbox::Bwrap {
        ensure_fuzz_tool("bwrap", &["--version"], "bwrap")?;
    }
    Ok(())
}

/// Build one fuzz target, or every target when `target` is `None`.
///
/// Never sandboxed: the build needs the network and writes `fuzz/target`.
fn build_targets(ctx: &FuzzContext, target: Option<&str>) -> Result<()> {
    let mut args = vec!["fuzz", "build"];
    args.extend(target);
    let fuzz_dir = ctx.root.join("fuzz");
    tracing::info!(
        "running command: cargo {} (in {})",
        args.join(" "),
        fuzz_dir.display()
    );
    // The corpus token is for the git credential helper only; build scripts
    // of every fuzz-crate dependency must not see it.
    cmd("cargo", args)
        .dir(&fuzz_dir)
        .env_remove(corpus_repo::TOKEN_ENV)
        .run()
        .wrap_err("`cargo fuzz build` failed")?;
    Ok(())
}

/// Ask `rustc -vV` for the host triple.
fn query_host_triple() -> Result<String> {
    let out = cmd!("rustc", "-vV")
        .read()
        .wrap_err("failed to run `rustc -vV`")?;
    parse_host_triple(&out)
}

/// The sandbox and the (resolved, not yet created) corpus and artifacts roots
/// of one fuzz session.
///
/// Nothing touches the filesystem until a command asks for a directory, so a
/// command only ever creates the directories it actually uses.
struct Session {
    /// How fuzz processes are isolated.
    sandbox: Sandbox,
    /// `<corpus-root>`, resolved against the workspace root.
    corpus_root: PathBuf,
    /// `<artifacts-root>`, resolved against the workspace root.
    artifacts_root: PathBuf,
}

impl Session {
    /// Resolve the roots against the workspace. Creates nothing.
    fn resolve(ctx: &FuzzContext, env: &FuzzEnvArgs) -> Self {
        Self {
            sandbox: env.sandbox,
            corpus_root: resolve_against(&ctx.root, &env.corpus_root),
            artifacts_root: resolve_against(&ctx.root, &env.artifacts_root),
        }
    }

    /// The corpus root, created if missing, canonical.
    fn prepare_corpus_root(&self) -> Result<PathBuf> {
        prepare_dir(&self.corpus_root)
    }

    /// `<corpus-root>/<target>`, created if missing, canonical.
    fn prepare_corpus_dir(&self, target: &str) -> Result<PathBuf> {
        prepare_dir(&self.corpus_root.join(target))
    }

    /// `<artifacts-root>/<target>`, created if missing, canonical.
    fn prepare_artifacts_dir(&self, target: &str) -> Result<PathBuf> {
        prepare_dir(&self.artifacts_root.join(target))
    }
}

// ---------------------------------------------------------------------------
// Per-target operations
// ---------------------------------------------------------------------------

/// What [`fuzz_one`] fuzzes and how.
struct FuzzRequest<'a> {
    /// The target to fuzz.
    target: &'a str,
    /// Wall-clock budget in seconds.
    time_secs: u64,
    /// What to do on a finding.
    policy: CrashPolicy,
    /// Fork-mode workers (used only for [`CrashPolicy::KeepGoing`]).
    jobs: u32,
}

/// Fuzz one already-built target.
///
/// Under [`CrashPolicy::KeepGoing`] the exit code cannot be trusted to report
/// findings (fork mode exits with the last child's code, which is usually 0
/// once the workers are stopped), so the artifacts directory is compared
/// before and after the run and the difference is always reported.
fn fuzz_one(ctx: &FuzzContext, session: &Session, request: &FuzzRequest<'_>) -> Result<()> {
    let target = request.target;
    let corpus = session.prepare_corpus_dir(target)?;
    let artifacts = session.prepare_artifacts_dir(target)?;
    let seeds = ctx.seeds_dir(target);
    ensure!(
        seeds.is_dir(),
        "seed directory missing: {}",
        seeds.display()
    );
    let binary = ctx.binary(target)?;
    let dictionary = dictionary_for_target(&ctx.root, target);
    let mut visible: Vec<&Path> = vec![&binary, &seeds];
    visible.extend(dictionary.as_deref());
    ensure_inputs_visible(session.sandbox, &visible)?;
    let args = fuzz_args(&FuzzArgsPlan {
        corpus: &corpus,
        seeds: &seeds,
        artifacts: &artifacts,
        time_secs: request.time_secs,
        policy: request.policy,
        jobs: request.jobs,
        dictionary: dictionary.as_deref(),
    });
    let before = match request.policy {
        CrashPolicy::Stop => None,
        CrashPolicy::KeepGoing => Some(finding_artifacts(&artifacts)?),
    };
    let with_dictionary = dictionary.as_deref().map_or_else(String::new, |dict| {
        format!(" with dictionary {}", dict.display())
    });
    tracing::info!(
        "fuzzing `{target}` for {}s{with_dictionary}",
        request.time_secs
    );
    let result = run_fuzz_binary(
        session.sandbox,
        request.policy,
        &binary,
        &args,
        &[corpus, artifacts.clone()],
        FindingsLocation::Artifacts(&artifacts),
    );
    if let Some(before) = before {
        let after = finding_artifacts(&artifacts)?;
        let found = new_findings(&before, &after);
        let summary = findings_summary(&artifacts, &found, after.len());
        if found.is_empty() {
            tracing::info!("`{target}`: {summary}");
        } else {
            tracing::warn!("`{target}`: {summary}");
        }
    }
    result
}

/// Replay seeds and regressions of one already-built target.
///
/// Every file is run once as an individual input (see [`replay_args`]), so a
/// failure is attributed to a specific file and no seed, regression, corpus or
/// artifact is written.
fn replay_one(ctx: &FuzzContext, sandbox: Sandbox, target: &str) -> Result<()> {
    let seeds = ctx.seeds_dir(target);
    ensure!(
        seeds.is_dir(),
        "seed directory missing: {}",
        seeds.display()
    );
    let mut dirs = vec![seeds];
    let regressions = ctx.regressions_dir(target);
    if regressions.is_dir() {
        dirs.push(regressions);
    }
    let inputs = collect_replay_inputs(&dirs)?;
    ensure!(
        !inputs.is_empty(),
        "no replay inputs for `{target}` (looked in {})",
        dirs.iter()
            .map(|dir| dir.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let binary = ctx.binary(target)?;
    let visible: Vec<&Path> = std::iter::once(binary.as_path())
        .chain(dirs.iter().map(PathBuf::as_path))
        .collect();
    ensure_inputs_visible(sandbox, &visible)?;
    tracing::info!("replaying `{target}` ({} inputs)", inputs.len());
    // libFuzzer writes nothing in individual-file mode, so nothing needs to be
    // writable.
    run_fuzz_binary(
        sandbox,
        CrashPolicy::Stop,
        &binary,
        &replay_args(&inputs),
        &[],
        FindingsLocation::ReplayedInput,
    )
}

/// Minimise one target's working corpus.
fn cmin_one(ctx: &FuzzContext, session: &Session, target: &str) -> Result<()> {
    let plan = plan_cmin(&session.prepare_corpus_root()?, target);
    match cmin_preflight(DirPresence::of(&plan.live), DirPresence::of(&plan.old)) {
        CminPreflight::Proceed => {}
        CminPreflight::RestoreOld => {
            tracing::warn!(
                "`{target}`: {} is missing but {} exists: an earlier minimisation was \
                 interrupted mid-swap; restoring the original corpus",
                plan.live.display(),
                plan.old.display()
            );
            fs::rename(&plan.old, &plan.live).wrap_err_with(|| {
                format!(
                    "failed to restore {} from {}",
                    plan.live.display(),
                    plan.old.display()
                )
            })?;
        }
        CminPreflight::AbortBothExist => bail!(
            "both {} and {} exist (an earlier minimisation was interrupted); not deleting \
             either corpus automatically. Keep the one you want as {} and remove the other",
            plan.live.display(),
            plan.old.display(),
            plan.live.display()
        ),
        CminPreflight::NothingToMinimise => {
            bail!(
                "no working corpus for `{target}` at {}",
                plan.live.display()
            )
        }
    }
    let binary = ctx.binary(target)?;
    ensure_inputs_visible(session.sandbox, &[&binary, &plan.live])?;
    let artifacts = session.prepare_artifacts_dir(target)?;

    remove_dir_if_exists(&plan.fresh)?;
    let fresh = prepare_dir(&plan.fresh)?;
    let before = count_files(&plan.live)?;
    let args = cmin_args(&fresh, &plan.live, &artifacts);
    tracing::info!("minimising corpus of `{target}` ({before} files)");
    run_fuzz_binary(
        session.sandbox,
        CrashPolicy::Stop,
        &binary,
        &args,
        &[fresh.clone(), artifacts.clone()],
        FindingsLocation::Artifacts(&artifacts),
    )?;

    match decide_cmin_swap(before, DirPresence::of(&plan.old), count_files(&fresh)?) {
        SwapDecision::Swap => {}
        SwapDecision::RefuseEmptyFresh => bail!(
            "the merge of {before} files into {} produced no files; keeping {} untouched \
             (the empty merge output is left in place for inspection)",
            plan.fresh.display(),
            plan.live.display()
        ),
        SwapDecision::RefuseStrayOld => bail!(
            "{} appeared while merging; not replacing {} (merged corpus left in {})",
            plan.old.display(),
            plan.live.display(),
            plan.fresh.display()
        ),
    }
    swap_cmin(&plan)?;
    let after = count_files(&plan.live)?;
    tracing::info!("corpus of `{target}`: {before} -> {after} files");
    Ok(())
}

/// Print a `target / result` table and fail if any target failed.
fn summarize(title: &str, results: &[(String, Outcome)]) -> Result<()> {
    let width = results
        .iter()
        .map(|(target, _)| target.len())
        .chain(std::iter::once("target".len()))
        .max()
        .unwrap_or(0);
    println!("\n{title} summary");
    println!("{:<width$}  result", "target");
    println!("{:-<width$}  ------", "");
    let mut failed = 0_usize;
    for (target, outcome) in results {
        match outcome {
            Outcome::Passed => println!("{target:<width$}  ok"),
            Outcome::Failed(why) => {
                failed += 1;
                println!("{target:<width$}  FAILED ({why})");
            }
        }
    }
    ensure!(
        failed == 0,
        "{title}: {failed} of {} targets failed",
        results.len()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Subcommand entry points
// ---------------------------------------------------------------------------

/// `cargo xtask fuzz-list`
pub fn fuzz_list() -> Result<()> {
    let ctx = FuzzContext::discover()?;
    for target in &ctx.targets {
        println!("{target}");
    }
    Ok(())
}

/// `cargo xtask fuzz <TARGET>`
pub fn fuzz(args: &FuzzRunArgs) -> Result<()> {
    let ctx = FuzzContext::discover()?;
    ctx.validate_target(&args.target)?;
    if args.on_crash == CrashPolicy::Stop && args.jobs > 1 {
        tracing::warn!("--jobs only applies with `--on-crash keep-going`; ignoring");
    }
    require_tools(args.env.sandbox)?;
    let session = Session::resolve(&ctx, &args.env);
    build_targets(&ctx, Some(&args.target))?;
    fuzz_one(
        &ctx,
        &session,
        &FuzzRequest {
            target: &args.target,
            time_secs: args.time,
            policy: args.on_crash,
            jobs: args.jobs,
        },
    )
}

/// `cargo xtask fuzz-smoke`
pub fn fuzz_smoke(args: &FuzzSmokeArgs) -> Result<()> {
    let ctx = FuzzContext::discover()?;
    require_tools(args.env.sandbox)?;
    let session = Session::resolve(&ctx, &args.env);
    build_targets(&ctx, None)?;
    let results: Vec<(String, Outcome)> = ctx
        .targets
        .iter()
        .map(|target| {
            let outcome = match fuzz_one(
                &ctx,
                &session,
                &FuzzRequest {
                    target,
                    time_secs: args.time,
                    policy: CrashPolicy::Stop,
                    jobs: 1,
                },
            ) {
                Ok(()) => Outcome::Passed,
                Err(err) => {
                    tracing::error!("`{target}` failed: {err:#}");
                    Outcome::Failed(format!("{err:#}"))
                }
            };
            (target.clone(), outcome)
        })
        .collect();
    summarize("fuzz-smoke", &results)
}

/// The checks CI runs on the `fuzz/` crate itself, as `cargo` arguments, run
/// in `fuzz/`. The crate is outside the root workspace, so none of the
/// workspace gates cover it.
const FUZZ_CRATE_CHECKS: [&[&str]; 3] = [
    &["fmt", "--check"],
    &["clippy", "--all-targets", "--", "-D", "warnings"],
    &["test", "--lib"],
];

/// Format-check, lint and unit-test the `fuzz/` crate, failing at the first
/// problem.
fn check_fuzz_crate(ctx: &FuzzContext) -> Result<()> {
    let fuzz_dir = ctx.root.join("fuzz");
    for args in FUZZ_CRATE_CHECKS {
        tracing::info!(
            "running command: cargo {} (in {})",
            args.join(" "),
            fuzz_dir.display()
        );
        cmd("cargo", args.iter().copied())
            .dir(&fuzz_dir)
            .run()
            .wrap_err_with(|| format!("fuzz crate check `cargo {}` failed", args.join(" ")))?;
    }
    Ok(())
}

/// `cargo xtask fuzz-replay`
pub fn fuzz_replay(args: &FuzzReplayArgs) -> Result<()> {
    let ctx = FuzzContext::discover()?;
    require_tools(args.sandbox)?;
    check_fuzz_crate(&ctx)?;
    build_targets(&ctx, None)?;
    let results: Vec<(String, Outcome)> = ctx
        .targets
        .iter()
        .map(|target| {
            let outcome = match replay_one(&ctx, args.sandbox, target) {
                Ok(()) => Outcome::Passed,
                Err(err) => {
                    tracing::error!("`{target}` failed: {err:#}");
                    Outcome::Failed(format!("{err:#}"))
                }
            };
            (target.clone(), outcome)
        })
        .collect();
    summarize("fuzz-replay", &results)
}

/// `cargo xtask fuzz-cmin <TARGET|--all>`
pub fn fuzz_cmin(args: &FuzzCminArgs) -> Result<()> {
    let ctx = FuzzContext::discover()?;
    let selection = args.selection()?;
    let targets: Vec<String> = match selection {
        CminSelection::All => ctx.targets.clone(),
        CminSelection::One(target) => {
            ctx.validate_target(&target)?;
            vec![target]
        }
    };
    let session = Session::resolve(&ctx, &args.env);
    cmin_targets(&ctx, &session, targets)
}

/// `cargo xtask fuzz-corpus <pull|push>`
pub fn fuzz_corpus(args: &FuzzCorpusArgs) -> Result<()> {
    corpus_repo::run(args)
}

/// Minimise the working corpus of each of `targets` (already validated).
///
/// With several targets, one that has no corpus is skipped; with exactly one,
/// a missing corpus is an error. Prints a summary table and fails if any
/// target failed.
fn cmin_targets(ctx: &FuzzContext, session: &Session, targets: Vec<String>) -> Result<()> {
    require_tools(session.sandbox)?;
    let corpus_root = session.prepare_corpus_root()?;
    let single = targets.len() == 1;
    for target in &targets {
        build_targets(ctx, Some(target))?;
    }
    let mut results: Vec<(String, Outcome)> = Vec::new();
    for target in targets {
        let plan = plan_cmin(&corpus_root, &target);
        // A missing live corpus with a parked `.old` is an interrupted swap that
        // `cmin_one` repairs, so it is not "no corpus".
        if !single && !plan.live.is_dir() && !plan.old.is_dir() {
            tracing::info!("skipping `{target}`: no working corpus");
            continue;
        }
        let outcome = match cmin_one(ctx, session, &target) {
            Ok(()) => Outcome::Passed,
            Err(err) => {
                tracing::error!("`{target}` failed: {err:#}");
                Outcome::Failed(format!("{err:#}"))
            }
        };
        results.push((target, outcome));
    }
    summarize("fuzz-cmin", &results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_triple_is_parsed_from_rustc_output() {
        let out = "rustc 1.99.0 (abc 2026-01-01)\nbinary: rustc\nhost: x86_64-unknown-linux-gnu\nrelease: 1.99.0\n";
        assert_eq!(parse_host_triple(out).unwrap(), "x86_64-unknown-linux-gnu");
    }

    #[test]
    fn host_triple_missing_line_is_an_error() {
        assert!(parse_host_triple("rustc 1.99.0\nrelease: 1.99.0\n").is_err());
        assert!(parse_host_triple("").is_err());
        assert!(parse_host_triple("host:   \n").is_err());
    }

    #[test]
    fn only_bin_targets_are_listed_in_order() {
        let bin = [TargetKind::Bin];
        let lib = [TargetKind::Lib];
        let both = [TargetKind::Lib, TargetKind::Bin];
        let names = bin_target_names([("a", &bin[..]), ("lib", &lib[..]), ("b", &both[..])]);
        assert_eq!(names, vec!["a".to_owned(), "b".to_owned()]);
        let none: Vec<(&str, &[TargetKind])> = Vec::new();
        assert_eq!(bin_target_names(none), Vec::<String>::new());
    }

    #[test]
    fn binary_path_is_under_fuzz_target_release() {
        let path = binary_path(
            Path::new("/repo"),
            "x86_64-unknown-linux-gnu",
            "osc99_parse",
        );
        let expected = format!(
            "/repo/fuzz/target/x86_64-unknown-linux-gnu/release/osc99_parse{}",
            std::env::consts::EXE_SUFFIX
        );
        assert_eq!(path, PathBuf::from(expected));
    }

    #[test]
    fn relative_paths_resolve_against_root_and_absolute_are_kept() {
        let root = Path::new("/repo");
        assert_eq!(
            resolve_against(root, Path::new("fuzz/corpus")),
            PathBuf::from("/repo/fuzz/corpus")
        );
        assert_eq!(
            resolve_against(root, Path::new("/data/corpus")),
            PathBuf::from("/data/corpus")
        );
    }

    #[test]
    fn value_enums_parse_documented_names() {
        assert_eq!(Sandbox::from_str("bwrap", true).unwrap(), Sandbox::Bwrap);
        assert_eq!(Sandbox::from_str("none", true).unwrap(), Sandbox::None);
        assert_eq!(
            CrashPolicy::from_str("stop", true).unwrap(),
            CrashPolicy::Stop
        );
        assert_eq!(
            CrashPolicy::from_str("keep-going", true).unwrap(),
            CrashPolicy::KeepGoing
        );
        assert!(Sandbox::from_str("docker", true).is_err());
        assert!(CrashPolicy::from_str("keep_going", false).is_err());
    }

    #[test]
    fn cmin_selection_requires_exactly_one_of_target_or_all() {
        let env = FuzzEnvArgs {
            sandbox: Sandbox::None,
            corpus_root: PathBuf::from("c"),
            artifacts_root: PathBuf::from("a"),
        };
        let one = FuzzCminArgs {
            target: Some("t".to_owned()),
            all: false,
            env: env.clone(),
        };
        let all = FuzzCminArgs {
            target: None,
            all: true,
            env: env.clone(),
        };
        let neither = FuzzCminArgs {
            target: None,
            all: false,
            env,
        };
        assert_eq!(one.selection().unwrap(), CminSelection::One("t".to_owned()));
        assert_eq!(all.selection().unwrap(), CminSelection::All);
        assert!(neither.selection().is_err());
    }

    #[test]
    fn summarize_is_ok_when_every_target_passed() {
        let results = vec![
            ("a".to_owned(), Outcome::Passed),
            ("longer_name".to_owned(), Outcome::Passed),
        ];
        assert!(summarize("t", &results).is_ok());
        assert!(summarize("t", &[]).is_ok());
    }

    #[test]
    fn summarize_fails_and_counts_failed_targets() {
        let results = vec![
            ("a".to_owned(), Outcome::Passed),
            ("b".to_owned(), Outcome::Failed("boom".to_owned())),
            ("c".to_owned(), Outcome::Failed("bang".to_owned())),
        ];
        let err = summarize("fuzz-replay", &results).unwrap_err().to_string();
        assert!(err.contains("2 of 3"), "{err}");
        assert!(err.contains("fuzz-replay"), "{err}");
    }

    #[test]
    fn fuzz_crate_checks_match_what_ci_runs() {
        let rendered: Vec<String> = FUZZ_CRATE_CHECKS.iter().map(|a| a.join(" ")).collect();
        assert_eq!(
            rendered,
            vec![
                "fmt --check",
                "clippy --all-targets -- -D warnings",
                "test --lib"
            ]
        );
    }

    #[test]
    fn host_triple_is_resolved_once_and_cached() {
        let ctx = FuzzContext::new(PathBuf::from("/repo"), vec!["t".to_owned()]);
        assert!(ctx.host_triple.get().is_none());
        let first = ctx.host_triple().unwrap().to_owned();
        assert_ne!(first, "");
        assert_eq!(
            ctx.host_triple.get().map(String::as_str),
            Some(first.as_str())
        );
        assert_eq!(
            ctx.binary("t").unwrap(),
            binary_path(Path::new("/repo"), &first, "t")
        );
    }

    #[test]
    fn session_resolves_without_creating_directories() {
        let root = tempfile::tempdir().unwrap();
        let ctx = FuzzContext::new(root.path().to_path_buf(), Vec::new());
        let env = FuzzEnvArgs {
            sandbox: Sandbox::None,
            corpus_root: PathBuf::from("c"),
            artifacts_root: PathBuf::from("a"),
        };
        let session = Session::resolve(&ctx, &env);
        assert!(!root.path().join("c").exists());
        assert!(!root.path().join("a").exists());

        session.prepare_artifacts_dir("t").unwrap();
        assert!(root.path().join("a").join("t").is_dir());
        assert!(
            !root.path().join("c").exists(),
            "corpus must stay untouched"
        );

        session.prepare_corpus_dir("t").unwrap();
        assert!(root.path().join("c").join("t").is_dir());
    }
}
