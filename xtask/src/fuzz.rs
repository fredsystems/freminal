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
//! * `fuzz/regressions/<target>/` -- crash reproducers, committed.
//! * `fuzz/corpus/<target>/`      -- working corpus, gitignored.
//! * `fuzz/artifacts/<target>/`   -- crash artifacts, gitignored.
//!
//! Everything that can be decided without touching the filesystem or running a
//! process (argument construction, sandbox cmdline, path planning, parsing) is a
//! pure function so it can be unit-tested.

use std::{
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
};

use cargo_metadata::{MetadataCommand, TargetKind};
use clap::{Args, ValueEnum};
use color_eyre::{
    eyre::{bail, ensure, eyre, Context},
    Result,
};
use duct::cmd;

/// Per-process RSS limit handed to libFuzzer, in megabytes.
const RSS_LIMIT_MB: u32 = 2048;
/// Per-allocation limit handed to libFuzzer, in megabytes.
const MALLOC_LIMIT_MB: u32 = 2048;
/// Per-input timeout handed to libFuzzer, in seconds.
const INPUT_TIMEOUT_SECS: u32 = 25;

/// libFuzzer's `TimeoutExitCode` (`FuzzerOptions.h`): a child hit `-timeout`.
///
/// Passed explicitly as `-timeout_exitcode` so the mapping never depends on
/// libFuzzer's default.
const LIBFUZZER_TIMEOUT_EXIT: i32 = 70;
/// libFuzzer's `OOMExitCode` (`FuzzerOptions.h`): a child exceeded
/// `-rss_limit_mb` or `-malloc_limit_mb`. Not configurable by a flag.
const LIBFUZZER_OOM_EXIT: i32 = 71;
/// libFuzzer's `InterruptExitCode` (`FuzzerOptions.h`): the process was
/// interrupted (SIGINT). That is an operator action, not a finding.
const LIBFUZZER_INTERRUPT_EXIT: i32 = 72;
/// libFuzzer's `ErrorExitCode` (`FuzzerOptions.h`): a child crashed or
/// libFuzzer reported a bug. Passed explicitly as `-error_exitcode`, and
/// also the code `AddressSanitizer` is told to exit with (see [`asan_options`]).
const LIBFUZZER_ERROR_EXIT: i32 = 77;

/// How a fuzz process is isolated from the host.
///
/// # Why a sandbox exists
///
/// The `terminal_state` target drives the real terminal handler end to end,
/// including kitty graphics file and shared-memory transmission. Those paths
/// read arbitrary files and **delete** files that are marked as kitty
/// temporaries under `/tmp` and `/dev/shm`. A fuzzer will happily produce such
/// a payload, so an unsandboxed local run can delete real files.
///
/// [`Sandbox::Bwrap`] runs the fuzz binary under bubblewrap with a read-only
/// root and private tmpfs mounts over `/tmp` and `/dev/shm`, so nothing outside
/// the explicitly writable directories can be modified or removed. CI runs in
/// disposable VMs (or a systemd-hardened runner unit) and passes
/// `--sandbox none`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Sandbox {
    /// Run under bubblewrap (Linux only). The default.
    #[default]
    Bwrap,
    /// Run directly on the host. Only for disposable environments.
    None,
}

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
// Pure helpers
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

/// `-artifact_prefix=<dir>/` (libFuzzer concatenates the prefix verbatim, so a
/// trailing separator is required for a directory).
fn artifact_prefix_arg(dir: &Path) -> OsString {
    let mut arg = OsString::from("-artifact_prefix=");
    arg.push(dir);
    arg.push("/");
    arg
}

/// The resource-limit arguments common to every libFuzzer invocation.
fn limit_args() -> Vec<OsString> {
    vec![
        OsString::from(format!("-rss_limit_mb={RSS_LIMIT_MB}")),
        OsString::from(format!("-malloc_limit_mb={MALLOC_LIMIT_MB}")),
        OsString::from(format!("-timeout={INPUT_TIMEOUT_SECS}")),
        OsString::from(format!("-timeout_exitcode={LIBFUZZER_TIMEOUT_EXIT}")),
        OsString::from(format!("-error_exitcode={LIBFUZZER_ERROR_EXIT}")),
    ]
}

/// The `ASAN_OPTIONS` value for a fuzz process: `existing` (when set and
/// non-empty) followed by `exitcode=77`.
///
/// `AddressSanitizer` exits with its own `exitcode`, default 1, when it reports
/// a bug. That collides with libFuzzer's usage errors, which also exit 1, so
/// a finding could not be told apart from a bad command line. Mapping `ASan` to
/// libFuzzer's `ErrorExitCode` ([`LIBFUZZER_ERROR_EXIT`]) makes every finding
/// report as 77. The option is appended last because the sanitizers let a
/// later occurrence of a key win, so it overrides any inherited `exitcode`.
fn asan_options(existing: Option<&OsStr>) -> OsString {
    let ours = format!("exitcode={LIBFUZZER_ERROR_EXIT}");
    match existing {
        Some(prev) if !prev.is_empty() => {
            let mut value = prev.to_owned();
            value.push(":");
            value.push(ours);
            value
        }
        _ => OsString::from(ours),
    }
}

/// How a finished fuzz process ended, as judged by [`classify_exit`].
#[derive(Debug, PartialEq, Eq)]
enum RunEnd {
    /// Exit code 0: nothing was found.
    Clean,
    /// The (last) process reported a crash, OOM or timeout with this exit
    /// code. Under [`CrashPolicy::KeepGoing`] the run itself still completed.
    Findings(i32),
    /// The run did not complete: interrupted, killed by a signal, or an
    /// exit code that is neither success nor a known finding code.
    InfraFailure(String),
}

/// Classify a fuzz process exit. `code` is `ExitStatus::code()`, so `None`
/// means the process was killed by a signal.
///
/// In fork mode libFuzzer's parent exits with the *last* child's exit code,
/// so a finding code means the last child hit a finding while the run
/// completed. Only an interrupt (72), a signal, or an unknown code is an
/// infrastructure failure. The crash policy is deliberately not an input: the
/// call site decides what a [`RunEnd::Findings`] means per [`CrashPolicy`]
/// (see [`run_fuzz_binary`]).
fn classify_exit(code: Option<i32>) -> RunEnd {
    match code {
        Some(0) => RunEnd::Clean,
        Some(code @ (LIBFUZZER_TIMEOUT_EXIT | LIBFUZZER_OOM_EXIT | LIBFUZZER_ERROR_EXIT)) => {
            RunEnd::Findings(code)
        }
        Some(LIBFUZZER_INTERRUPT_EXIT) => RunEnd::InfraFailure(format!(
            "the fuzz process was interrupted (exit {LIBFUZZER_INTERRUPT_EXIT})"
        )),
        Some(code) => RunEnd::InfraFailure(format!(
            "the fuzz process exited with unexpected code {code} (not a libFuzzer finding code; \
             likely a usage error or an infrastructure failure)"
        )),
        None => RunEnd::InfraFailure("the fuzz process was killed by a signal".to_owned()),
    }
}

/// What a libFuzzer finding exit code means, for messages.
const fn finding_description(code: i32) -> &'static str {
    match code {
        LIBFUZZER_TIMEOUT_EXIT => "timeout",
        LIBFUZZER_OOM_EXIT => "out-of-memory",
        _ => "crash",
    }
}

/// Inputs for [`fuzz_args`].
struct FuzzArgsPlan<'a> {
    /// Working corpus (libFuzzer writes new inputs to the *first* directory).
    corpus: &'a Path,
    /// Curated seeds (read-only).
    seeds: &'a Path,
    /// Directory receiving crash artifacts.
    artifacts: &'a Path,
    /// Wall-clock budget in seconds.
    time_secs: u64,
    /// What to do on a finding.
    policy: CrashPolicy,
    /// Fork-mode workers (used only for [`CrashPolicy::KeepGoing`]).
    jobs: u32,
}

/// libFuzzer arguments for a normal fuzzing run.
fn fuzz_args(plan: &FuzzArgsPlan<'_>) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        plan.corpus.as_os_str().to_owned(),
        plan.seeds.as_os_str().to_owned(),
        artifact_prefix_arg(plan.artifacts),
        OsString::from(format!("-max_total_time={}", plan.time_secs)),
    ];
    args.extend(limit_args());
    match plan.policy {
        CrashPolicy::Stop => {}
        CrashPolicy::KeepGoing => {
            args.push(OsString::from(format!("-fork={}", plan.jobs)));
            args.push(OsString::from("-ignore_crashes=1"));
            args.push(OsString::from("-ignore_ooms=1"));
            args.push(OsString::from("-ignore_timeouts=1"));
        }
    }
    args
}

/// libFuzzer arguments for a deterministic replay (`-runs=0`) of `inputs`.
///
/// The first positional directory is where libFuzzer writes any input it finds
/// interesting, so a throwaway `scratch` directory goes first and the real
/// (committed) input directories only ever appear after it.
fn replay_args(scratch: &Path, inputs: &[PathBuf], artifacts: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![scratch.as_os_str().to_owned()];
    args.extend(inputs.iter().map(|dir| dir.as_os_str().to_owned()));
    args.push(artifact_prefix_arg(artifacts));
    args.push(OsString::from("-runs=0"));
    args.extend(limit_args());
    args
}

/// libFuzzer arguments for a corpus merge (`-merge=1 <fresh> <corpus>`).
fn cmin_args(fresh: &Path, corpus: &Path, artifacts: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        OsString::from("-merge=1"),
        fresh.as_os_str().to_owned(),
        corpus.as_os_str().to_owned(),
        artifact_prefix_arg(artifacts),
    ];
    args.extend(limit_args());
    args
}

/// Directory names used while minimising one target's corpus.
#[derive(Debug, PartialEq, Eq)]
struct CminPlan {
    /// The live working corpus, `<root>/<target>`.
    live: PathBuf,
    /// The freshly merged corpus, `<root>/<target>.cmin`.
    fresh: PathBuf,
    /// Where the old corpus is parked during the swap, `<root>/<target>.old`.
    old: PathBuf,
}

/// Plan the directory names for minimising `target` under `corpus_root`.
fn plan_cmin(corpus_root: &Path, target: &str) -> CminPlan {
    CminPlan {
        live: corpus_root.join(target),
        fresh: corpus_root.join(format!("{target}.cmin")),
        old: corpus_root.join(format!("{target}.old")),
    }
}

/// Whether bubblewrap's private tmpfs mounts hide `path` from the sandbox.
fn hidden_by_private_tmpfs(path: &Path) -> bool {
    path.starts_with("/tmp") || path.starts_with("/dev/shm")
}

/// Fail if a read-only input would be masked by the sandbox's private tmpfs.
fn ensure_inputs_visible(sandbox: Sandbox, inputs: &[&Path]) -> Result<()> {
    if sandbox != Sandbox::Bwrap {
        return Ok(());
    }
    for input in inputs {
        ensure!(
            !hidden_by_private_tmpfs(input),
            "{} lives under /tmp or /dev/shm, which the bwrap sandbox replaces with an empty \
             tmpfs; move it elsewhere or pass `--sandbox none` knowingly",
            input.display()
        );
    }
    Ok(())
}

/// Build the full cmdline (program first) for running `binary` with `args`.
///
/// For [`Sandbox::Bwrap`] the binary runs under a read-only root with private
/// tmpfs mounts over `/tmp` and `/dev/shm` (see [`Sandbox`] for why). The
/// `writable` directories are bound read-write **after** the tmpfs mounts so
/// they are not masked by them. All paths must be absolute.
fn sandboxed_argv(
    sandbox: Sandbox,
    binary: &Path,
    args: &[OsString],
    writable: &[PathBuf],
) -> Result<Vec<OsString>> {
    ensure!(
        binary.is_absolute(),
        "fuzz binary path must be absolute: {}",
        binary.display()
    );
    match sandbox {
        Sandbox::None => {
            let mut cmdline = vec![binary.as_os_str().to_owned()];
            cmdline.extend(args.iter().cloned());
            Ok(cmdline)
        }
        Sandbox::Bwrap => {
            ensure!(
                cfg!(target_os = "linux"),
                "the bwrap sandbox is Linux-only (bubblewrap does not exist on this platform); \
                 pass `--sandbox none` knowingly if you accept that fuzzing can delete files"
            );
            for dir in writable {
                ensure!(
                    dir.is_absolute(),
                    "writable sandbox path must be absolute: {}",
                    dir.display()
                );
            }
            Ok(bwrap_argv(binary, args, writable))
        }
    }
}

/// The bubblewrap cmdline; see [`sandboxed_argv`].
fn bwrap_argv(binary: &Path, args: &[OsString], writable: &[PathBuf]) -> Vec<OsString> {
    let mut cmdline: Vec<OsString> = [
        "bwrap",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--tmpfs",
        "/tmp",
        "--tmpfs",
        "/dev/shm",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    for dir in writable {
        cmdline.push(OsString::from("--bind"));
        cmdline.push(dir.as_os_str().to_owned());
        cmdline.push(dir.as_os_str().to_owned());
    }
    // The host `TMPDIR` (e.g. `/tmp/nix-shell.XXXX` in a dev shell) does not
    // exist in the private tmpfs, and libFuzzer writes its merge control file
    // and fork-mode logs there.
    cmdline.extend(["--setenv", "TMPDIR", "/tmp"].iter().map(OsString::from));
    cmdline.extend(
        ["--unshare-all", "--die-with-parent", "--new-session", "--"]
            .iter()
            .map(OsString::from),
    );
    cmdline.push(binary.as_os_str().to_owned());
    cmdline.extend(args.iter().cloned());
    cmdline
}

// ---------------------------------------------------------------------------
// Environment discovery and process helpers
// ---------------------------------------------------------------------------

/// Everything resolved once per command invocation.
struct FuzzContext {
    /// Workspace root; every relative path is resolved against it.
    root: PathBuf,
    /// Discovered fuzz target names.
    targets: Vec<String>,
}

impl FuzzContext {
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
        Ok(Self { root, targets })
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

    fn seeds_dir(&self, target: &str) -> PathBuf {
        self.root.join("fuzz").join("seeds").join(target)
    }

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
    cmd("cargo", args)
        .dir(&fuzz_dir)
        .run()
        .wrap_err("`cargo fuzz build` failed")?;
    Ok(())
}

/// The host triple, from `rustc -vV`.
fn host_triple() -> Result<String> {
    let out = cmd!("rustc", "-vV")
        .read()
        .wrap_err("failed to run `rustc -vV`")?;
    parse_host_triple(&out)
}

/// Where a finding from a fuzz process can be looked for afterwards.
#[derive(Debug, Clone, Copy)]
enum FindingsLocation<'a> {
    /// libFuzzer wrote crash artifacts into this directory.
    Artifacts(&'a Path),
    /// A replay run: nothing is written anywhere durable. The failing input
    /// is one of the committed files and is named in libFuzzer's output.
    ReplayedInput,
}

impl FindingsLocation<'_> {
    /// A user-facing sentence fragment saying where to look.
    fn hint(&self) -> String {
        match self {
            Self::Artifacts(dir) => format!("see artifacts in {}", dir.display()),
            Self::ReplayedInput => "the failing input is one of the committed files under \
                 fuzz/seeds or fuzz/regressions and is named in libFuzzer's output above"
                .to_owned(),
        }
    }
}

/// Run `binary` with `args` under `sandbox` and judge how it ended.
///
/// `ASAN_OPTIONS` is always set (see [`asan_options`]); bubblewrap inherits
/// the environment (`bwrap_argv` never passes `--clearenv`). The exit code is
/// classified by [`classify_exit`]:
///
/// * clean exit: `Ok`;
/// * a finding under [`CrashPolicy::Stop`]: an error;
/// * a finding under [`CrashPolicy::KeepGoing`]: `Ok` with a warning, because
///   fork mode reports the last child's code and the run itself completed;
/// * an infrastructure failure: an error.
///
/// `findings` is only used to tell the user where to look for a finding.
fn run_fuzz_binary(
    sandbox: Sandbox,
    policy: CrashPolicy,
    binary: &Path,
    args: &[OsString],
    writable: &[PathBuf],
    findings: FindingsLocation<'_>,
) -> Result<()> {
    ensure!(
        binary.is_file(),
        "fuzz binary not found: {} (did `cargo fuzz build` succeed?)",
        binary.display()
    );
    let cmdline = sandboxed_argv(sandbox, binary, args, writable)?;
    let (program, rest) = cmdline
        .split_first()
        .ok_or_else(|| eyre!("empty fuzz command line"))?;
    tracing::info!("running command: {cmdline:?}");
    let output = cmd(program.clone(), rest.iter().cloned())
        .env(
            "ASAN_OPTIONS",
            asan_options(std::env::var_os("ASAN_OPTIONS").as_deref()),
        )
        .unchecked()
        .run()
        .wrap_err("failed to spawn the fuzz process")?;
    match classify_exit(output.status.code()) {
        RunEnd::Clean => Ok(()),
        RunEnd::Findings(code) => {
            let what = finding_description(code);
            match policy {
                CrashPolicy::Stop => bail!(
                    "found a crash/oom/timeout ({what}, exit {code}); {}",
                    findings.hint()
                ),
                CrashPolicy::KeepGoing => {
                    tracing::warn!(
                        "the run completed and found at least one crash/oom/timeout (last \
                         process exit {code}: {what}); {}",
                        findings.hint()
                    );
                    Ok(())
                }
            }
        }
        RunEnd::InfraFailure(why) => bail!("{why} ({})", output.status),
    }
}

/// Resolved, existing, canonical directories for one fuzz session.
struct Session {
    sandbox: Sandbox,
    corpus_root: PathBuf,
    artifacts_root: PathBuf,
}

impl Session {
    /// Resolve the roots against the workspace and create them.
    fn prepare(ctx: &FuzzContext, env: &FuzzEnvArgs) -> Result<Self> {
        Ok(Self {
            sandbox: env.sandbox,
            corpus_root: prepare_dir(&resolve_against(&ctx.root, &env.corpus_root))?,
            artifacts_root: prepare_dir(&resolve_against(&ctx.root, &env.artifacts_root))?,
        })
    }
}

/// Fuzz one already-built target.
fn fuzz_one(
    ctx: &FuzzContext,
    host_triple: &str,
    session: &Session,
    target: &str,
    time_secs: u64,
    policy: CrashPolicy,
    jobs: u32,
) -> Result<()> {
    let corpus = prepare_dir(&session.corpus_root.join(target))?;
    let artifacts = prepare_dir(&session.artifacts_root.join(target))?;
    let seeds = ctx.seeds_dir(target);
    ensure!(
        seeds.is_dir(),
        "seed directory missing: {}",
        seeds.display()
    );
    let binary = binary_path(&ctx.root, host_triple, target);
    ensure_inputs_visible(session.sandbox, &[&binary, &seeds])?;
    let args = fuzz_args(&FuzzArgsPlan {
        corpus: &corpus,
        seeds: &seeds,
        artifacts: &artifacts,
        time_secs,
        policy,
        jobs,
    });
    tracing::info!("fuzzing `{target}` for {time_secs}s");
    run_fuzz_binary(
        session.sandbox,
        policy,
        &binary,
        &args,
        &[corpus, artifacts.clone()],
        FindingsLocation::Artifacts(&artifacts),
    )
}

/// Replay seeds and regressions of one already-built target.
fn replay_one(ctx: &FuzzContext, host_triple: &str, session: &Session, target: &str) -> Result<()> {
    let seeds = ctx.seeds_dir(target);
    ensure!(
        seeds.is_dir(),
        "seed directory missing: {}",
        seeds.display()
    );
    let mut inputs = vec![seeds];
    let regressions = ctx.regressions_dir(target);
    if regressions.is_dir() {
        inputs.push(regressions);
    }
    let binary = binary_path(&ctx.root, host_triple, target);
    let visible: Vec<&Path> = std::iter::once(binary.as_path())
        .chain(inputs.iter().map(PathBuf::as_path))
        .collect();
    ensure_inputs_visible(session.sandbox, &visible)?;

    // One disposable directory under the artifacts root serves as libFuzzer's
    // output corpus and artifact prefix, so the committed inputs are never
    // written to.
    let scratch = tempfile::Builder::new()
        .prefix(&format!("replay-{target}-"))
        .tempdir_in(&session.artifacts_root)
        .wrap_err("failed to create replay scratch directory")?;
    let scratch_path = scratch.path().canonicalize()?;
    let args = replay_args(&scratch_path, &inputs, &scratch_path);
    tracing::info!("replaying `{target}`");
    run_fuzz_binary(
        session.sandbox,
        CrashPolicy::Stop,
        &binary,
        &args,
        std::slice::from_ref(&scratch_path),
        FindingsLocation::ReplayedInput,
    )
}

/// Minimise one target's working corpus.
fn cmin_one(ctx: &FuzzContext, host_triple: &str, session: &Session, target: &str) -> Result<()> {
    let plan = plan_cmin(&session.corpus_root, target);
    ensure!(
        plan.live.is_dir(),
        "no working corpus for `{target}` at {}",
        plan.live.display()
    );
    let binary = binary_path(&ctx.root, host_triple, target);
    ensure_inputs_visible(session.sandbox, &[&binary, &plan.live])?;
    let artifacts = prepare_dir(&session.artifacts_root.join(target))?;

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
        &[fresh, artifacts.clone()],
        FindingsLocation::Artifacts(&artifacts),
    )?;

    swap_cmin(&plan)?;
    let after = count_files(&plan.live)?;
    tracing::info!("corpus of `{target}`: {before} -> {after} files");
    Ok(())
}

/// Atomically (per rename) replace the live corpus with the merged one.
fn swap_cmin(plan: &CminPlan) -> Result<()> {
    remove_dir_if_exists(&plan.old)?;
    fs::rename(&plan.live, &plan.old)
        .wrap_err_with(|| format!("failed to move {} aside", plan.live.display()))?;
    if let Err(err) = fs::rename(&plan.fresh, &plan.live) {
        // Put the original corpus back; the merged one stays in `.cmin`.
        let restored = fs::rename(&plan.old, &plan.live);
        return Err(eyre!(
            "failed to install merged corpus {}: {err} (original restored: {})",
            plan.fresh.display(),
            restored.is_ok()
        ));
    }
    remove_dir_if_exists(&plan.old)
}

fn remove_dir_if_exists(dir: &Path) -> Result<()> {
    if dir.exists() {
        fs::remove_dir_all(dir).wrap_err_with(|| format!("failed to remove {}", dir.display()))?;
    }
    Ok(())
}

fn count_files(dir: &Path) -> Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir(dir).wrap_err_with(|| format!("failed to read {}", dir.display()))? {
        if entry?.file_type()?.is_file() {
            count += 1;
        }
    }
    Ok(count)
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
    let session = Session::prepare(&ctx, &args.env)?;
    build_targets(&ctx, Some(&args.target))?;
    let triple = host_triple()?;
    fuzz_one(
        &ctx,
        &triple,
        &session,
        &args.target,
        args.time,
        args.on_crash,
        args.jobs,
    )
}

/// `cargo xtask fuzz-smoke`
pub fn fuzz_smoke(args: &FuzzSmokeArgs) -> Result<()> {
    let ctx = FuzzContext::discover()?;
    require_tools(args.env.sandbox)?;
    let session = Session::prepare(&ctx, &args.env)?;
    build_targets(&ctx, None)?;
    let triple = host_triple()?;
    let results: Vec<(String, Outcome)> = ctx
        .targets
        .iter()
        .map(|target| {
            let outcome = match fuzz_one(
                &ctx,
                &triple,
                &session,
                target,
                args.time,
                CrashPolicy::Stop,
                1,
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

/// `cargo xtask fuzz-replay`
pub fn fuzz_replay(env: &FuzzEnvArgs) -> Result<()> {
    let ctx = FuzzContext::discover()?;
    require_tools(env.sandbox)?;
    let session = Session::prepare(&ctx, env)?;
    build_targets(&ctx, None)?;
    let triple = host_triple()?;
    let results: Vec<(String, Outcome)> = ctx
        .targets
        .iter()
        .map(|target| {
            let outcome = match replay_one(&ctx, &triple, &session, target) {
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
    require_tools(args.env.sandbox)?;
    let session = Session::prepare(&ctx, &args.env)?;
    let single = targets.len() == 1;
    for target in &targets {
        build_targets(&ctx, Some(target))?;
    }
    let triple = host_triple()?;
    let mut results: Vec<(String, Outcome)> = Vec::new();
    for target in targets {
        if !single && !session.corpus_root.join(&target).is_dir() {
            tracing::info!("skipping `{target}`: no working corpus");
            continue;
        }
        let outcome = match cmin_one(&ctx, &triple, &session, &target) {
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

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

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
    fn bwrap_argv_has_exact_order_with_writable_binds_after_tmpfs() {
        let cmdline = bwrap_argv(
            Path::new("/repo/fuzz/target/t/release/x"),
            &os(&["/c", "-runs=0"]),
            &[PathBuf::from("/w1"), PathBuf::from("/w2")],
        );
        let expected = os(&[
            "bwrap",
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
            "--tmpfs",
            "/dev/shm",
            "--bind",
            "/w1",
            "/w1",
            "--bind",
            "/w2",
            "/w2",
            "--setenv",
            "TMPDIR",
            "/tmp",
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--",
            "/repo/fuzz/target/t/release/x",
            "/c",
            "-runs=0",
        ]);
        assert_eq!(cmdline, expected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sandboxed_argv_bwrap_matches_bwrap_argv_on_linux() {
        let binary = Path::new("/repo/x");
        let args = os(&["a"]);
        let writable = [PathBuf::from("/w")];
        assert_eq!(
            sandboxed_argv(Sandbox::Bwrap, binary, &args, &writable).unwrap(),
            bwrap_argv(binary, &args, &writable)
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn sandboxed_argv_bwrap_errors_off_linux() {
        let err = sandboxed_argv(Sandbox::Bwrap, Path::new("/repo/x"), &[], &[]).unwrap_err();
        assert!(err.to_string().contains("Linux-only"));
    }

    #[test]
    fn sandboxed_argv_none_is_binary_plus_args() {
        let cmdline = sandboxed_argv(
            Sandbox::None,
            Path::new("/repo/x"),
            &os(&["-runs=0"]),
            &[PathBuf::from("/w")],
        )
        .unwrap();
        assert_eq!(cmdline, os(&["/repo/x", "-runs=0"]));
    }

    #[test]
    fn sandboxed_argv_rejects_relative_paths() {
        assert!(sandboxed_argv(Sandbox::None, Path::new("x"), &[], &[]).is_err());
        #[cfg(target_os = "linux")]
        assert!(sandboxed_argv(
            Sandbox::Bwrap,
            Path::new("/x"),
            &[],
            &[PathBuf::from("relative")]
        )
        .is_err());
    }

    #[test]
    fn tmp_and_shm_inputs_are_rejected_only_under_bwrap() {
        let hidden = Path::new("/tmp/repo/fuzz/seeds/x");
        let shm = Path::new("/dev/shm/seeds");
        let fine = Path::new("/home/u/repo/fuzz/seeds/x");
        assert!(ensure_inputs_visible(Sandbox::Bwrap, &[hidden]).is_err());
        assert!(ensure_inputs_visible(Sandbox::Bwrap, &[shm]).is_err());
        assert!(ensure_inputs_visible(Sandbox::Bwrap, &[fine]).is_ok());
        assert!(ensure_inputs_visible(Sandbox::None, &[hidden]).is_ok());
        assert!(!hidden_by_private_tmpfs(Path::new("/tmpfoo")));
    }

    #[test]
    fn stop_args_have_corpus_first_and_no_fork() {
        let args = fuzz_args(&FuzzArgsPlan {
            corpus: Path::new("/c/t"),
            seeds: Path::new("/s/t"),
            artifacts: Path::new("/a/t"),
            time_secs: 10,
            policy: CrashPolicy::Stop,
            jobs: 4,
        });
        assert_eq!(
            args,
            os(&[
                "/c/t",
                "/s/t",
                "-artifact_prefix=/a/t/",
                "-max_total_time=10",
                "-rss_limit_mb=2048",
                "-malloc_limit_mb=2048",
                "-timeout=25",
                "-timeout_exitcode=70",
                "-error_exitcode=77",
            ])
        );
    }

    #[test]
    fn keep_going_args_add_fork_and_ignore_flags() {
        let args = fuzz_args(&FuzzArgsPlan {
            corpus: Path::new("/c/t"),
            seeds: Path::new("/s/t"),
            artifacts: Path::new("/a/t"),
            time_secs: 60,
            policy: CrashPolicy::KeepGoing,
            jobs: 3,
        });
        assert_eq!(args[0], OsString::from("/c/t"));
        assert_eq!(args[1], OsString::from("/s/t"));
        assert_eq!(
            args[9..],
            os(&[
                "-fork=3",
                "-ignore_crashes=1",
                "-ignore_ooms=1",
                "-ignore_timeouts=1"
            ])[..]
        );
    }

    #[test]
    fn replay_args_put_scratch_first_and_do_not_fuzz() {
        let args = replay_args(
            Path::new("/a/replay"),
            &[PathBuf::from("/s/t"), PathBuf::from("/r/t")],
            Path::new("/a/replay"),
        );
        assert_eq!(args[..3], os(&["/a/replay", "/s/t", "/r/t"])[..]);
        assert!(args.contains(&OsString::from("-runs=0")));
        assert!(args.contains(&OsString::from("-artifact_prefix=/a/replay/")));
        assert!(!args
            .iter()
            .any(|a| a.to_string_lossy().contains("max_total_time")));
    }

    #[test]
    fn cmin_args_merge_into_fresh_dir_first() {
        let args = cmin_args(Path::new("/c/t.cmin"), Path::new("/c/t"), Path::new("/a/t"));
        assert_eq!(
            args[..4],
            os(&["-merge=1", "/c/t.cmin", "/c/t", "-artifact_prefix=/a/t/"])[..]
        );
    }

    #[test]
    fn cmin_plan_uses_sibling_directories() {
        assert_eq!(
            plan_cmin(Path::new("/c"), "osc99_parse"),
            CminPlan {
                live: PathBuf::from("/c/osc99_parse"),
                fresh: PathBuf::from("/c/osc99_parse.cmin"),
                old: PathBuf::from("/c/osc99_parse.old"),
            }
        );
    }

    #[test]
    fn swap_cmin_replaces_live_with_fresh_and_removes_old() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_cmin(dir.path(), "t");
        fs::create_dir_all(&plan.live).unwrap();
        fs::create_dir_all(&plan.fresh).unwrap();
        fs::create_dir_all(&plan.old).unwrap(); // stale leftover
        fs::write(plan.live.join("old_input"), b"o").unwrap();
        fs::write(plan.fresh.join("new_input"), b"n").unwrap();

        swap_cmin(&plan).unwrap();

        assert!(plan.live.join("new_input").is_file());
        assert!(!plan.live.join("old_input").exists());
        assert!(!plan.fresh.exists());
        assert!(!plan.old.exists());
    }

    #[test]
    fn swap_cmin_restores_original_when_install_fails() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_cmin(dir.path(), "t");
        fs::create_dir_all(&plan.live).unwrap();
        fs::write(plan.live.join("keep"), b"k").unwrap();
        // `fresh` does not exist, so the install rename fails.
        assert!(swap_cmin(&plan).is_err());
        assert!(plan.live.join("keep").is_file());
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
    fn limit_args_pin_libfuzzer_exit_codes() {
        let args = limit_args();
        assert!(args.contains(&OsString::from("-timeout_exitcode=70")));
        assert!(args.contains(&OsString::from("-error_exitcode=77")));
    }

    #[test]
    fn asan_options_unset_is_just_our_exitcode() {
        assert_eq!(asan_options(None), OsString::from("exitcode=77"));
    }

    #[test]
    fn asan_options_empty_has_no_leading_separator() {
        assert_eq!(
            asan_options(Some(OsStr::new(""))),
            OsString::from("exitcode=77")
        );
    }

    #[test]
    fn asan_options_existing_is_kept_and_ours_comes_last() {
        assert_eq!(
            asan_options(Some(OsStr::new("detect_leaks=0:exitcode=1"))),
            OsString::from("detect_leaks=0:exitcode=1:exitcode=77")
        );
    }

    #[test]
    fn classify_exit_zero_is_clean() {
        assert_eq!(classify_exit(Some(0)), RunEnd::Clean);
    }

    #[test]
    fn classify_exit_finding_codes_are_findings() {
        for code in [70, 71, 77] {
            assert_eq!(classify_exit(Some(code)), RunEnd::Findings(code), "{code}");
        }
    }

    #[test]
    fn classify_exit_interrupt_is_infra_failure() {
        assert!(
            matches!(classify_exit(Some(72)), RunEnd::InfraFailure(m) if m.contains("interrupted"))
        );
    }

    #[test]
    fn classify_exit_signal_is_infra_failure() {
        assert!(matches!(classify_exit(None), RunEnd::InfraFailure(m) if m.contains("signal")));
    }

    #[test]
    fn classify_exit_unknown_codes_are_infra_failures() {
        for code in [1, 2, 69, 73, 76, 78, 134, 255, -1] {
            assert!(
                matches!(classify_exit(Some(code)), RunEnd::InfraFailure(m) if m.contains("unexpected")),
                "{code}"
            );
        }
    }

    #[test]
    fn finding_description_names_each_code() {
        assert_eq!(finding_description(70), "timeout");
        assert_eq!(finding_description(71), "out-of-memory");
        assert_eq!(finding_description(77), "crash");
    }
}
