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
//! * `fuzz/regressions/<target>/` -- crash reproducers, committed, never written.
//! * `fuzz/corpus/<target>/`      -- working corpus, gitignored.
//! * `fuzz/artifacts/<target>/`   -- crash artifacts, gitignored.
//!
//! Everything that can be decided without touching the filesystem or running a
//! process (argument construction, sandbox cmdline, path planning, parsing) is a
//! pure function so it can be unit-tested.

use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::OnceLock,
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
/// `-max_len` handed to libFuzzer when fuzzing, in bytes.
///
/// Without it libFuzzer clamps the length of generated inputs to the largest
/// corpus file, but to at least 4096 and at most 1 MiB. A corpus of small
/// seeds therefore caps generation near 4 KiB, and the targets that need long
/// inputs to reach their deep paths (sixel images, FREC streams, whole
/// terminal-state sequences) would never see one. 64 KiB is large enough for
/// those and small enough to keep executions fast.
///
/// Deliberately **not** passed for replay (individual-file mode runs each file
/// whole, so a limit would silently truncate a committed reproducer) and not
/// for `-merge` (a merge must not truncate corpus files; without the flag
/// libFuzzer's merge step reads up to 1 MiB, which covers anything this limit
/// let a fuzz run produce).
const FUZZ_MAX_LEN: u32 = 65_536;
/// File git needs to track an otherwise-empty directory; never a replay input.
const GITKEEP: &str = ".gitkeep";
/// The line libFuzzer prints before it runs each file given as an individual
/// input (`RunIndividualFiles` in `FuzzerDriver.cpp`). It is the only place the
/// failing file of a replay is ever named.
const RUNNING_PREFIX: &str = "Running: ";

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

/// Arguments of `cargo xtask fuzz-replay`.
///
/// Replay reads only committed inputs and writes nothing, so unlike the other
/// commands it takes no corpus or artifacts directory.
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
        OsString::from(format!("-max_len={FUZZ_MAX_LEN}")),
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

/// libFuzzer arguments for a deterministic replay of `inputs`.
///
/// Every input is a *file*, so libFuzzer takes its individual-file path
/// (`AllInputsAreFiles` -> `RunIndividualFiles` in `FuzzerDriver.cpp`): it runs
/// each file once, in the order given, prints `Running: <path>` before each,
/// and writes nothing -- no output corpus, no artifacts. A directory argument
/// would instead start a corpus-loading run that never names the file that
/// crashed. `-runs=1` is the explicit "once". No `-max_len`: whole files run.
fn replay_args(inputs: &[PathBuf]) -> Vec<OsString> {
    let mut args: Vec<OsString> = inputs
        .iter()
        .map(|file| file.as_os_str().to_owned())
        .collect();
    args.push(OsString::from("-runs=1"));
    args.extend(limit_args());
    args
}

/// Every regular file directly inside each of `dirs`, ready to be passed to
/// [`replay_args`].
///
/// Directories are visited in the order given and the files of each are sorted
/// by path, so the order (and therefore which input a failure is attributed
/// to) is deterministic. Subdirectories are not descended into and
/// [`GITKEEP`] is skipped.
fn collect_replay_inputs(dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut inputs = Vec::new();
    for dir in dirs {
        let mut files = Vec::new();
        for entry in
            fs::read_dir(dir).wrap_err_with(|| format!("failed to read {}", dir.display()))?
        {
            let entry = entry.wrap_err_with(|| format!("failed to read {}", dir.display()))?;
            if entry.file_name() == GITKEEP {
                continue;
            }
            let path = entry.path();
            // `is_file` follows symlinks and is false for directories.
            if path.is_file() {
                files.push(path);
            }
        }
        files.sort();
        inputs.extend(files);
    }
    Ok(inputs)
}

/// The path in a libFuzzer `Running: <path>` line, if `line` is one.
fn running_input(line: &str) -> Option<&str> {
    line.strip_prefix(RUNNING_PREFIX).map(str::trim_end)
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

/// What to do about a possibly-interrupted earlier minimisation, decided
/// before anything is merged. See [`cmin_preflight`].
#[derive(Debug, PartialEq, Eq)]
enum CminPreflight {
    /// `live` exists and there is no stray `.old`: the normal case.
    Proceed,
    /// `live` is missing but `.old` exists: a previous swap was interrupted
    /// between parking the corpus and installing the merged one. `.old` is the
    /// original corpus; move it back, then proceed.
    RestoreOld,
    /// Both `live` and `.old` exist. Which one is the good corpus cannot be
    /// known, so nothing is touched and the user must resolve it.
    AbortBothExist,
    /// Neither exists: there is nothing to minimise.
    NothingToMinimise,
}

/// Whether a corpus directory is on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirPresence {
    /// The directory exists.
    Present,
    /// The directory does not exist.
    Absent,
}

impl DirPresence {
    /// Probe `path` (anything that exists counts as present).
    fn of(path: &Path) -> Self {
        if path.exists() {
            Self::Present
        } else {
            Self::Absent
        }
    }
}

/// Decide how to start a minimisation from which directories exist.
const fn cmin_preflight(live: DirPresence, old: DirPresence) -> CminPreflight {
    match (live, old) {
        (DirPresence::Present, DirPresence::Absent) => CminPreflight::Proceed,
        (DirPresence::Absent, DirPresence::Present) => CminPreflight::RestoreOld,
        (DirPresence::Present, DirPresence::Present) => CminPreflight::AbortBothExist,
        (DirPresence::Absent, DirPresence::Absent) => CminPreflight::NothingToMinimise,
    }
}

/// What to do once the merge has produced the fresh corpus. See
/// [`decide_cmin_swap`].
#[derive(Debug, PartialEq, Eq)]
enum SwapDecision {
    /// Replace the live corpus with the merged one.
    Swap,
    /// The merge produced no files from a non-empty corpus. That is a failed
    /// or truncated merge, not a minimisation: keep the live corpus.
    RefuseEmptyFresh,
    /// A `.old` appeared while the merge ran; swapping would clobber it.
    RefuseStrayOld,
}

/// Decide whether the merged corpus may replace the live one.
const fn decide_cmin_swap(live_files: usize, old: DirPresence, fresh_files: usize) -> SwapDecision {
    if matches!(old, DirPresence::Present) {
        SwapDecision::RefuseStrayOld
    } else if fresh_files == 0 && live_files > 0 {
        SwapDecision::RefuseEmptyFresh
    } else {
        SwapDecision::Swap
    }
}

/// The kinds of finding artifact libFuzzer writes into `-artifact_prefix`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum FindingKind {
    /// `crash-<sha1>`
    Crash,
    /// `oom-<sha1>`
    Oom,
    /// `timeout-<sha1>`
    Timeout,
    /// `leak-<sha1>`
    Leak,
    /// `slow-unit-<sha1>`
    SlowUnit,
}

impl FindingKind {
    /// Every kind, in the order a summary lists them.
    const ALL: [Self; 5] = [
        Self::Crash,
        Self::Oom,
        Self::Timeout,
        Self::Leak,
        Self::SlowUnit,
    ];

    /// The file-name prefix libFuzzer gives this kind of artifact.
    const fn prefix(self) -> &'static str {
        match self {
            Self::Crash => "crash-",
            Self::Oom => "oom-",
            Self::Timeout => "timeout-",
            Self::Leak => "leak-",
            Self::SlowUnit => "slow-unit-",
        }
    }

    /// The name used in a summary.
    const fn label(self) -> &'static str {
        match self {
            Self::Crash => "crash",
            Self::Oom => "oom",
            Self::Timeout => "timeout",
            Self::Leak => "leak",
            Self::SlowUnit => "slow-unit",
        }
    }
}

/// Which kind of finding artifact a file name is, if any.
fn finding_kind(file_name: &str) -> Option<FindingKind> {
    FindingKind::ALL
        .into_iter()
        .find(|kind| file_name.starts_with(kind.prefix()))
}

/// The finding-artifact file names currently in `dir`.
fn finding_artifacts(dir: &Path) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(dir).wrap_err_with(|| format!("failed to read {}", dir.display()))? {
        let entry = entry.wrap_err_with(|| format!("failed to read {}", dir.display()))?;
        let file_name = entry.file_name();
        let is_finding = file_name
            .to_str()
            .is_some_and(|n| finding_kind(n).is_some());
        if is_finding {
            names.insert(file_name.to_string_lossy().into_owned());
        }
    }
    Ok(names)
}

/// The kinds of the artifacts in `after` that are not in `before`.
///
/// Artifact names embed a content hash, so re-finding an existing crash writes
/// the same file and is (correctly) not counted as new.
fn new_findings(before: &BTreeSet<String>, after: &BTreeSet<String>) -> Vec<FindingKind> {
    after
        .difference(before)
        .filter_map(|name| finding_kind(name))
        .collect()
}

/// One line describing `found`, the new findings of a run, in `dir`.
fn findings_summary(dir: &Path, found: &[FindingKind]) -> String {
    if found.is_empty() {
        return format!("no new findings in {}", dir.display());
    }
    let breakdown: Vec<String> = FindingKind::ALL
        .into_iter()
        .filter_map(|kind| {
            let count = found.iter().filter(|&&k| k == kind).count();
            (count > 0).then(|| format!("{}: {count}", kind.label()))
        })
        .collect();
    format!(
        "{} new finding(s) ({}) in {}",
        found.len(),
        breakdown.join(", "),
        dir.display()
    )
}

/// Whether bubblewrap's private tmpfs mounts hide `path` from the sandbox.
fn hidden_by_private_tmpfs(path: &Path) -> bool {
    path.starts_with("/tmp") || path.starts_with("/dev/shm")
}

/// Fail if a read-only input would be masked by the sandbox's private tmpfs.
///
/// A path that is not itself under `/tmp` can still resolve there through a
/// symlink, so both the path as given and its canonical form are checked. When
/// the path cannot be canonicalised (it does not exist yet) only the path as
/// given is checked.
fn ensure_inputs_visible(sandbox: Sandbox, inputs: &[&Path]) -> Result<()> {
    if sandbox != Sandbox::Bwrap {
        return Ok(());
    }
    for input in inputs {
        let canonical = fs::canonicalize(input).ok();
        let hidden = hidden_by_private_tmpfs(input)
            || canonical.as_deref().is_some_and(hidden_by_private_tmpfs);
        ensure!(
            !hidden,
            "{} lives under /tmp or /dev/shm (resolves to {}), which the bwrap sandbox replaces \
             with an empty tmpfs; move it elsewhere or pass `--sandbox none` knowingly",
            input.display(),
            canonical.as_deref().unwrap_or(input).display()
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
    cmd("cargo", args)
        .dir(&fuzz_dir)
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

/// Where a finding from a fuzz process can be looked for afterwards.
#[derive(Debug, Clone, Copy)]
enum FindingsLocation<'a> {
    /// libFuzzer wrote crash artifacts into this directory.
    Artifacts(&'a Path),
    /// A replay of individual files. libFuzzer deliberately saves **no**
    /// artifact in that mode (`SaveArtifacts` is forced off in
    /// `FuzzerDriver.cpp`); the input file itself is the reproducer, and it is
    /// named by the last `Running:` line ([`RUNNING_PREFIX`]).
    ReplayedInput,
}

impl FindingsLocation<'_> {
    /// A user-facing sentence fragment saying where to look.
    fn hint(&self) -> String {
        match self {
            Self::Artifacts(dir) => format!("see artifacts in {}", dir.display()),
            Self::ReplayedInput => "libFuzzer saves no artifact when replaying files; the failing \
                 input is the last `Running:` path printed above and is itself the reproducer"
                .to_owned(),
        }
    }
}

/// A finished fuzz process.
struct ProcessEnd {
    /// The exit status.
    status: ExitStatus,
    /// The last path libFuzzer announced with a `Running:` line, if any.
    last_input: Option<String>,
}

/// Run `program` with `rest`, mirroring its combined output to our stderr as it
/// arrives, and remember the last `Running: <path>` line.
///
/// libFuzzer's individual-file mode is the only thing that ever names the file
/// being executed, and does so only on its output, so the output has to be
/// read here to be able to say which file failed.
fn run_streaming(program: &OsString, rest: &[OsString]) -> Result<ProcessEnd> {
    let reader = cmd(program.clone(), rest.iter().cloned())
        .env(
            "ASAN_OPTIONS",
            asan_options(std::env::var_os("ASAN_OPTIONS").as_deref()),
        )
        .stderr_to_stdout()
        .unchecked()
        .reader()
        .wrap_err("failed to spawn the fuzz process")?;
    let mut lines = BufReader::new(&reader);
    let mut stderr = io::stderr();
    let mut last_input = None;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = lines
            .read_until(b'\n', &mut line)
            .wrap_err("failed to read the fuzz process output")?;
        if read == 0 {
            break;
        }
        // Mirroring is best-effort: a closed stderr must not fail the run.
        let _ = stderr.write_all(&line);
        if let Some(path) = running_input(&String::from_utf8_lossy(&line)) {
            last_input = Some(path.to_owned());
        }
    }
    let output = reader
        .try_wait()
        .wrap_err("failed to wait for the fuzz process")?
        .ok_or_else(|| eyre!("the fuzz process closed its output but is still running"))?;
    Ok(ProcessEnd {
        status: output.status,
        last_input,
    })
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
    let end = run_streaming(program, rest)?;
    let while_running = end
        .last_input
        .as_deref()
        .map(|path| format!(" while running input {path}"))
        .unwrap_or_default();
    match classify_exit(end.status.code()) {
        RunEnd::Clean => Ok(()),
        RunEnd::Findings(code) => {
            let what = finding_description(code);
            match policy {
                CrashPolicy::Stop => bail!(
                    "found a crash/oom/timeout ({what}, exit {code}){while_running}; {}",
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
        RunEnd::InfraFailure(why) => bail!("{why} ({}){while_running}", end.status),
    }
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
    ensure_inputs_visible(session.sandbox, &[&binary, &seeds])?;
    let args = fuzz_args(&FuzzArgsPlan {
        corpus: &corpus,
        seeds: &seeds,
        artifacts: &artifacts,
        time_secs: request.time_secs,
        policy: request.policy,
        jobs: request.jobs,
    });
    let before = match request.policy {
        CrashPolicy::Stop => None,
        CrashPolicy::KeepGoing => Some(finding_artifacts(&artifacts)?),
    };
    tracing::info!("fuzzing `{target}` for {}s", request.time_secs);
    let result = run_fuzz_binary(
        session.sandbox,
        request.policy,
        &binary,
        &args,
        &[corpus, artifacts.clone()],
        FindingsLocation::Artifacts(&artifacts),
    );
    if let Some(before) = before {
        let found = new_findings(&before, &finding_artifacts(&artifacts)?);
        let summary = findings_summary(&artifacts, &found);
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
/// failure is attributed to a specific file and nothing is written anywhere.
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
    // Nothing is written, so nothing needs to be writable.
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

/// Atomically (per rename) replace the live corpus with the merged one.
///
/// Refuses to start when `.old` exists: parking the live corpus there would
/// have to delete whatever it holds first, and an `.old` that is not ours is
/// somebody's only copy of a corpus (see [`cmin_preflight`]).
fn swap_cmin(plan: &CminPlan) -> Result<()> {
    ensure!(
        !plan.old.exists(),
        "{} already exists; refusing to overwrite it",
        plan.old.display()
    );
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

/// Remove `dir` and everything in it if it exists.
fn remove_dir_if_exists(dir: &Path) -> Result<()> {
    if dir.exists() {
        fs::remove_dir_all(dir).wrap_err_with(|| format!("failed to remove {}", dir.display()))?;
    }
    Ok(())
}

/// The number of regular files directly inside `dir`.
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
    require_tools(args.env.sandbox)?;
    let session = Session::resolve(&ctx, &args.env);
    let corpus_root = session.prepare_corpus_root()?;
    let single = targets.len() == 1;
    for target in &targets {
        build_targets(&ctx, Some(target))?;
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
        let outcome = match cmin_one(&ctx, &session, &target) {
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

    #[cfg(target_os = "linux")]
    #[test]
    fn a_symlink_into_tmp_is_rejected_under_bwrap() {
        // The link lives next to this crate (not under /tmp) but resolves to
        // a directory under /tmp, which the sandbox would mask.
        let target = tempfile::tempdir_in("/tmp").unwrap();
        let links = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let link = links.path().join("seeds");
        std::os::unix::fs::symlink(target.path(), &link).unwrap();
        assert!(!hidden_by_private_tmpfs(&link));
        assert!(ensure_inputs_visible(Sandbox::Bwrap, &[link.as_path()]).is_err());
        assert!(ensure_inputs_visible(Sandbox::None, &[link.as_path()]).is_ok());
    }

    #[test]
    fn a_missing_path_is_checked_as_given() {
        let missing = Path::new("/definitely/not/here/seeds");
        assert!(ensure_inputs_visible(Sandbox::Bwrap, &[missing]).is_ok());
        assert!(ensure_inputs_visible(Sandbox::Bwrap, &[Path::new("/tmp/not/here")]).is_err());
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
                "-max_len=65536",
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
            args[10..],
            os(&[
                "-fork=3",
                "-ignore_crashes=1",
                "-ignore_ooms=1",
                "-ignore_timeouts=1"
            ])[..]
        );
    }

    #[test]
    fn replay_args_are_only_files_then_runs_one_and_limits() {
        let args = replay_args(&[PathBuf::from("/s/t/a"), PathBuf::from("/r/t/b")]);
        assert_eq!(
            args,
            os(&[
                "/s/t/a",
                "/r/t/b",
                "-runs=1",
                "-rss_limit_mb=2048",
                "-malloc_limit_mb=2048",
                "-timeout=25",
                "-timeout_exitcode=70",
                "-error_exitcode=77",
            ])
        );
    }

    #[test]
    fn replay_args_never_truncate_fuzz_or_write() {
        let args = replay_args(&[PathBuf::from("/s/t/a")]);
        for forbidden in [
            "max_len",
            "max_total_time",
            "artifact_prefix",
            "-fork",
            "-merge",
        ] {
            assert!(
                !args.iter().any(|a| a.to_string_lossy().contains(forbidden)),
                "{forbidden}"
            );
        }
    }

    #[test]
    fn replay_inputs_are_sorted_per_dir_in_dir_order_without_gitkeep_or_subdirs() {
        let root = tempfile::tempdir().unwrap();
        let seeds = root.path().join("seeds");
        let regressions = root.path().join("regressions");
        fs::create_dir_all(seeds.join("nested")).unwrap();
        fs::create_dir_all(&regressions).unwrap();
        for name in ["b", "a", ".gitkeep", "c"] {
            fs::write(seeds.join(name), b"x").unwrap();
        }
        fs::write(seeds.join("nested").join("deep"), b"x").unwrap();
        fs::write(regressions.join(".gitkeep"), b"").unwrap();
        fs::write(regressions.join("crash-1"), b"x").unwrap();

        let inputs = collect_replay_inputs(&[seeds.clone(), regressions.clone()]).unwrap();
        assert_eq!(
            inputs,
            vec![
                seeds.join("a"),
                seeds.join("b"),
                seeds.join("c"),
                regressions.join("crash-1"),
            ]
        );
    }

    #[test]
    fn replay_inputs_of_a_gitkeep_only_dir_are_empty_and_missing_dir_errors() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join(".gitkeep"), b"").unwrap();
        assert_eq!(
            collect_replay_inputs(&[root.path().to_path_buf()]).unwrap(),
            Vec::<PathBuf>::new()
        );
        assert!(collect_replay_inputs(&[root.path().join("missing")]).is_err());
    }

    #[test]
    fn running_input_extracts_the_path_of_a_libfuzzer_running_line() {
        assert_eq!(
            running_input("Running: /repo/fuzz/seeds/t/a\n"),
            Some("/repo/fuzz/seeds/t/a")
        );
        assert_eq!(running_input("Running: /x y/z"), Some("/x y/z"));
        assert_eq!(running_input("Executed /x in 3 ms"), None);
        assert_eq!(running_input("INFO: Running: nope"), None);
    }

    #[test]
    fn fuzz_args_carry_max_len_and_cmin_and_replay_do_not() {
        assert_eq!(FUZZ_MAX_LEN, 65_536);
        let stop = fuzz_args(&FuzzArgsPlan {
            corpus: Path::new("/c"),
            seeds: Path::new("/s"),
            artifacts: Path::new("/a"),
            time_secs: 1,
            policy: CrashPolicy::Stop,
            jobs: 1,
        });
        let keep = fuzz_args(&FuzzArgsPlan {
            corpus: Path::new("/c"),
            seeds: Path::new("/s"),
            artifacts: Path::new("/a"),
            time_secs: 1,
            policy: CrashPolicy::KeepGoing,
            jobs: 2,
        });
        for args in [&stop, &keep] {
            assert!(args.contains(&OsString::from("-max_len=65536")));
        }
        let cmin = cmin_args(Path::new("/f"), Path::new("/c"), Path::new("/a"));
        assert!(!cmin.iter().any(|a| a.to_string_lossy().contains("max_len")));
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
        fs::write(plan.live.join("old_input"), b"o").unwrap();
        fs::write(plan.fresh.join("new_input"), b"n").unwrap();

        swap_cmin(&plan).unwrap();

        assert!(plan.live.join("new_input").is_file());
        assert!(!plan.live.join("old_input").exists());
        assert!(!plan.fresh.exists());
        assert!(!plan.old.exists());
    }

    #[test]
    fn swap_cmin_refuses_to_overwrite_an_existing_old_dir() {
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_cmin(dir.path(), "t");
        for d in [&plan.live, &plan.fresh, &plan.old] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(plan.live.join("live_input"), b"l").unwrap();
        fs::write(plan.fresh.join("new_input"), b"n").unwrap();
        fs::write(plan.old.join("only_copy"), b"o").unwrap();

        assert!(swap_cmin(&plan).is_err());

        assert!(plan.live.join("live_input").is_file());
        assert!(plan.fresh.join("new_input").is_file());
        assert!(plan.old.join("only_copy").is_file());
    }

    #[test]
    fn cmin_preflight_covers_every_combination() {
        use DirPresence::{Absent, Present};
        assert_eq!(cmin_preflight(Present, Absent), CminPreflight::Proceed);
        assert_eq!(cmin_preflight(Absent, Present), CminPreflight::RestoreOld);
        assert_eq!(
            cmin_preflight(Present, Present),
            CminPreflight::AbortBothExist
        );
        assert_eq!(
            cmin_preflight(Absent, Absent),
            CminPreflight::NothingToMinimise
        );
    }

    #[test]
    fn cmin_swap_decision_protects_the_live_corpus() {
        use DirPresence::{Absent, Present};
        assert_eq!(decide_cmin_swap(10, Absent, 4), SwapDecision::Swap);
        assert_eq!(decide_cmin_swap(10, Absent, 10), SwapDecision::Swap);
        assert_eq!(decide_cmin_swap(0, Absent, 0), SwapDecision::Swap);
        assert_eq!(
            decide_cmin_swap(10, Absent, 0),
            SwapDecision::RefuseEmptyFresh
        );
        assert_eq!(
            decide_cmin_swap(10, Present, 4),
            SwapDecision::RefuseStrayOld
        );
        assert_eq!(
            decide_cmin_swap(0, Present, 0),
            SwapDecision::RefuseStrayOld
        );
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

    #[test]
    fn finding_kinds_are_recognised_by_prefix() {
        for (name, kind) in [
            ("crash-0a1b", FindingKind::Crash),
            ("crash-minimized-5bytes", FindingKind::Crash),
            ("oom-0a1b", FindingKind::Oom),
            ("timeout-0a1b", FindingKind::Timeout),
            ("leak-0a1b", FindingKind::Leak),
            ("slow-unit-0a1b", FindingKind::SlowUnit),
        ] {
            assert_eq!(finding_kind(name), Some(kind), "{name}");
        }
        for name in [
            "0a1b2c",
            "my-crash-1",
            "crash",
            "slow-1",
            "replay-t-x",
            ".gitkeep",
        ] {
            assert_eq!(finding_kind(name), None, "{name}");
        }
    }

    #[test]
    fn finding_artifacts_lists_only_finding_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["crash-1", "oom-2", "other", "timeout-3"] {
            fs::write(dir.path().join(name), b"x").unwrap();
        }
        fs::create_dir(dir.path().join("crash-dir")).unwrap();
        let names = finding_artifacts(dir.path()).unwrap();
        // A directory with a finding-like name is still listed by name; only
        // libFuzzer writes here and it never creates directories.
        assert!(names.contains("crash-1"));
        assert!(names.contains("oom-2"));
        assert!(names.contains("timeout-3"));
        assert!(!names.contains("other"));
        assert!(finding_artifacts(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn only_artifacts_absent_before_are_new_findings() {
        let set = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect();
        let before: BTreeSet<String> = set(&["crash-old"]);
        let after: BTreeSet<String> = set(&["crash-old", "crash-new", "oom-new", "timeout-new"]);
        let mut found = new_findings(&before, &after);
        found.sort();
        assert_eq!(
            found,
            vec![FindingKind::Crash, FindingKind::Oom, FindingKind::Timeout]
        );
        assert_eq!(new_findings(&after, &after), Vec::<FindingKind>::new());
    }

    #[test]
    fn findings_summary_reports_counts_by_kind_or_none() {
        let dir = Path::new("/a/t");
        assert_eq!(findings_summary(dir, &[]), "no new findings in /a/t");
        assert_eq!(
            findings_summary(
                dir,
                &[
                    FindingKind::Oom,
                    FindingKind::Crash,
                    FindingKind::Crash,
                    FindingKind::SlowUnit
                ]
            ),
            "4 new finding(s) (crash: 2, oom: 1, slow-unit: 1) in /a/t"
        );
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
    fn count_files_counts_only_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(count_files(dir.path()).unwrap(), 0);
        fs::write(dir.path().join("a"), b"x").unwrap();
        fs::write(dir.path().join("b"), b"x").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub").join("c"), b"x").unwrap();
        assert_eq!(count_files(dir.path()).unwrap(), 2);
        assert!(count_files(&dir.path().join("missing")).is_err());
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

    /// Write an executable stub (`#!/usr/bin/env sh` + `body`) into `dir`.
    #[cfg(unix)]
    fn stub(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        fs::write(&path, format!("#!/usr/bin/env sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// Run a stub under `Sandbox::None` with the given policy.
    #[cfg(unix)]
    fn run_stub(binary: &Path, policy: CrashPolicy) -> Result<()> {
        run_fuzz_binary(
            Sandbox::None,
            policy,
            binary,
            &[],
            &[],
            FindingsLocation::ReplayedInput,
        )
    }

    // One test, run sequentially: writing and then executing a script from
    // several threads races with `fork` (ETXTBSY), so the scenarios share a
    // thread instead of being separate `#[test]`s.
    #[cfg(unix)]
    #[test]
    fn run_fuzz_binary_classifies_exit_codes_end_to_end() {
        use CrashPolicy::{KeepGoing, Stop};
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();

        let clean = stub(d, "clean", "exit 0");
        assert!(run_stub(&clean, Stop).is_ok());
        assert!(run_stub(&clean, KeepGoing).is_ok());

        let crash = stub(d, "crash", "exit 77");
        let err = run_stub(&crash, Stop).unwrap_err().to_string();
        assert!(err.contains("crash") && err.contains("exit 77"), "{err}");
        assert!(run_stub(&crash, KeepGoing).is_ok());

        let timeout = stub(d, "timeout", "exit 70");
        let err = run_stub(&timeout, Stop).unwrap_err().to_string();
        assert!(err.contains("timeout") && err.contains("exit 70"), "{err}");
        assert!(run_stub(&timeout, KeepGoing).is_ok());

        let oom = stub(d, "oom", "exit 71");
        let err = run_stub(&oom, Stop).unwrap_err().to_string();
        assert!(err.contains("out-of-memory"), "{err}");
        assert!(run_stub(&oom, KeepGoing).is_ok());

        // Infrastructure failures are errors under BOTH policies.
        let interrupted = stub(d, "interrupted", "exit 72");
        for policy in [Stop, KeepGoing] {
            let err = run_stub(&interrupted, policy).unwrap_err().to_string();
            assert!(err.contains("interrupted"), "{err}");
        }
        let usage = stub(d, "usage", "exit 1");
        for policy in [Stop, KeepGoing] {
            let err = run_stub(&usage, policy).unwrap_err().to_string();
            assert!(err.contains("unexpected code 1"), "{err}");
        }
        let killed = stub(d, "killed", "kill -9 $$");
        for policy in [Stop, KeepGoing] {
            let err = run_stub(&killed, policy).unwrap_err().to_string();
            assert!(err.contains("signal"), "{err}");
        }

        // The failing replay input is named from the `Running:` lines.
        let replay = stub(
            d,
            "replay",
            "echo 'Running: /seeds/t/a' >&2\necho 'Running: /seeds/t/b' >&2\nexit 77",
        );
        let err = run_stub(&replay, Stop).unwrap_err().to_string();
        assert!(err.contains("while running input /seeds/t/b"), "{err}");
        assert!(!err.contains("/seeds/t/a"), "{err}");
        assert!(err.contains("no artifact"), "{err}");

        // ASAN_OPTIONS reaches the process with our exit code appended.
        let asan = stub(
            d,
            "asan",
            "case \"$ASAN_OPTIONS\" in *exitcode=77) exit 0;; *) exit 5;; esac",
        );
        assert!(run_stub(&asan, Stop).is_ok());

        // Arguments are passed through.
        let args = stub(d, "args", "[ \"$1\" = \"-runs=1\" ] || exit 5");
        assert!(run_fuzz_binary(
            Sandbox::None,
            Stop,
            &args,
            &os(&["-runs=1"]),
            &[],
            FindingsLocation::ReplayedInput
        )
        .is_ok());

        // A missing binary is reported before anything is spawned.
        let err = run_stub(&d.join("missing"), Stop).unwrap_err().to_string();
        assert!(err.contains("not found"), "{err}");
    }
}
