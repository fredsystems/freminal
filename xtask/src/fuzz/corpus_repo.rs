// Copyright (C) 2024-2026 Fred Clausen and the ratatui project contributors
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `fuzz-corpus pull|push`: persist the working fuzz corpus in a separate
//! (private) git repository.
//!
//! The corpus root directory **is** the git working tree of that repository,
//! laid out as `<target>/<file>` at the repository root. libFuzzer names each
//! corpus file by the SHA-1 of its content, so one name always means the same
//! bytes: corpora produced on different machines merge without conflicts, and
//! a rebase can never hit a real content conflict.
//!
//! The decisions (what state the root is in, whether two URLs name the same
//! repository, how to word a commit, when to retry a rejected push) are pure
//! functions, unit-tested without git. The git plumbing lives behind [`Git`],
//! which tests drive against a bare repository in a temporary directory.
//!
//! # Authentication
//!
//! With `FREMINAL_FUZZ_CORPUS_TOKEN` set (CI), every git command that talks to
//! the remote gets an inline credential helper. The helper is a shell snippet
//! that reads the token from the environment when git runs it, so the token is
//! never written to disk and never appears in a command line. Without the
//! token (local use) git's normal credential setup is used and may prompt.

use std::{
    ffi::OsString,
    fs,
    io::ErrorKind,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::{Args, ValueEnum};
use color_eyre::{
    eyre::{bail, Context},
    Result,
};
use duct::cmd;

use super::{cmin::count_files, cmin_targets, FuzzContext, FuzzEnvArgs, Session};

/// The corpus repository used when `--repo-url` is not given.
pub const DEFAULT_REPO_URL: &str = "https://github.com/fredsystems/freminal-fuzz-corpus.git";

/// The only branch of the corpus repository.
const BRANCH: &str = "main";

/// Environment variable holding the access token used in CI.
const TOKEN_ENV: &str = "FREMINAL_FUZZ_CORPUS_TOKEN";

/// Most pushes attempted before giving up on a rejected push.
const MAX_PUSH_ATTEMPTS: u32 = 3;

/// Identity used for commits when git has no `user.email` configured.
const FALLBACK_IDENTITY: [&str; 4] = [
    "-c",
    "user.name=freminal-fuzz",
    "-c",
    "user.email=freminal-fuzz@users.noreply.github.com",
];

/// Environment variables that tell git which repository, index or work tree
/// to use. git sets several of them while running hooks (`GIT_DIR`,
/// `GIT_INDEX_FILE`, ...), and they override `git -C <dir>`: inherited from a
/// hook, or from a user's shell, they would make every command here act on
/// the *freminal* repository instead of the corpus checkout. They are removed
/// from every git command this module runs.
const INHERITED_REPO_ENV: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// The `-c` value installing the inline credential helper.
///
/// The `${...}` is deliberately **not** expanded by Rust: the literal text is
/// passed to git, and the shell git starts to run the helper expands it from
/// its environment. The token itself is therefore never in an argument list.
const CREDENTIAL_HELPER: &str = concat!(
    "credential.helper=!f() { echo username=x-access-token; echo \"password=$",
    "{FREMINAL_FUZZ_CORPUS_TOKEN}",
    "\"; }; f"
);

/// What `fuzz-corpus` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum CorpusAction {
    /// Make the corpus root a checkout of the corpus repository and update it.
    Pull,
    /// Pull, minimise every target's corpus, then commit and push the result.
    Push,
}

/// Arguments of `cargo xtask fuzz-corpus`.
#[derive(Clone, Debug, Args)]
pub struct FuzzCorpusArgs {
    /// Whether to pull the corpus repository or publish the local corpus.
    #[arg(value_enum)]
    pub action: CorpusAction,

    /// URL of the corpus repository.
    #[arg(
        long,
        value_name = "URL",
        env = "FREMINAL_FUZZ_CORPUS_REPO",
        default_value = DEFAULT_REPO_URL
    )]
    pub repo_url: String,

    /// Sandbox and directory options (the sandbox applies to the
    /// minimisation `push` runs).
    #[command(flatten)]
    pub env: FuzzEnvArgs,
}

// ---------------------------------------------------------------------------
// Pure decisions
// ---------------------------------------------------------------------------

/// Whether remote git commands carry the CI token.
///
/// Decided once from the environment, so nothing downstream re-reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RemoteAuth {
    /// `FREMINAL_FUZZ_CORPUS_TOKEN` is set: use the inline credential helper
    /// and never prompt.
    Token,
    /// No token: use the user's normal git credentials; prompting is allowed.
    Ambient,
}

impl RemoteAuth {
    /// Decide from the value of the token variable (`None` when unset).
    /// An empty or all-whitespace value counts as unset.
    fn from_token_value(value: Option<&str>) -> Self {
        match value {
            Some(token) if !token.trim().is_empty() => Self::Token,
            _ => Self::Ambient,
        }
    }

    /// Decide from the process environment.
    fn from_env() -> Self {
        Self::from_token_value(std::env::var(TOKEN_ENV).ok().as_deref())
    }

    /// Global `-c` options placed before a remote git command. The empty
    /// `credential.helper=` first clears every inherited helper.
    const fn config_args(self) -> &'static [&'static str] {
        match self {
            Self::Token => &["-c", "credential.helper=", "-c", CREDENTIAL_HELPER],
            Self::Ambient => &[],
        }
    }
}

/// Where a pulled or pushed root stands, from what is on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RootProbe {
    /// The corpus root does not exist.
    Missing,
    /// The corpus root is a directory.
    Present {
        /// Names of its entries (including `.git` when it is a checkout).
        entries: Vec<String>,
        /// Whether it is a git checkout.
        checkout: CheckoutProbe,
    },
}

/// Whether a directory is a git checkout and, if so, where it points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CheckoutProbe {
    /// No `.git` in the directory.
    NotCheckout,
    /// A `.git` is present; `origin` is the URL of the `origin` remote.
    Checkout {
        /// `None` when there is no `origin` remote.
        origin: Option<String>,
    },
}

/// How the corpus root relates to the corpus repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RootState {
    /// Nothing there yet.
    Missing,
    /// An empty directory.
    Empty,
    /// A checkout whose `origin` is the corpus repository.
    Checkout,
    /// A checkout of something else (or with no `origin`); never touched.
    ForeignCheckout {
        /// The checkout's `origin` URL, if it has one.
        origin: Option<String>,
    },
    /// A non-git directory holding only target and scratch directories: a
    /// corpus from before the repository existed.
    Adoptable,
    /// A non-git, non-empty directory holding something else.
    NotAdoptable {
        /// The entries that are not target or scratch directories.
        unexpected: Vec<String>,
    },
}

/// What `pull` does about a root in a given state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PullAction {
    /// `git clone` into the (created, if needed) root.
    Clone,
    /// Fetch and integrate `origin/main` into the existing checkout.
    Update,
    /// Turn the existing local corpus into a checkout of the repository.
    Adopt,
}

/// Whether two URLs name the same repository.
///
/// Trailing `.git` and `/`, credentials, case, and the https / `ssh://` /
/// `git@host:path` spellings of one GitHub repository are all ignored.
pub(super) fn urls_equal(a: &str, b: &str) -> bool {
    normalize_repo_url(a) == normalize_repo_url(b)
}

/// Reduce a repository URL to a canonical `host/path` form (or the path
/// itself for a local repository). See [`urls_equal`].
fn normalize_repo_url(url: &str) -> String {
    let url = url.trim();
    let host_and_path = if let Some((_, after_scheme)) = url.split_once("://") {
        // `scheme://[user[:password]@]host[:port]/path`; `file:///p` has an
        // empty authority and keeps just `/p`.
        let (authority, path) = after_scheme
            .find('/')
            .map_or((after_scheme, ""), |idx| after_scheme.split_at(idx));
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        format!("{host}{path}")
    } else if let Some((left, path)) = url.split_once(':') {
        // scp-like `[user@]host:path`. A single letter before the colon is a
        // Windows drive, and a `/` before it means a plain path.
        if left.len() > 1 && !left.contains('/') {
            let host = left.rsplit_once('@').map_or(left, |(_, h)| h);
            format!("{host}/{path}")
        } else {
            url.to_owned()
        }
    } else {
        url.to_owned()
    };
    let trimmed = host_and_path.trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    trimmed.trim_end_matches('/').to_lowercase()
}

/// Whether `entry` is a target directory or one of its minimisation scratch
/// directories (`<target>.cmin`, `<target>.old`).
fn is_target_or_scratch(entry: &str, targets: &[String]) -> bool {
    let base = entry
        .strip_suffix(".cmin")
        .or_else(|| entry.strip_suffix(".old"))
        .unwrap_or(entry);
    targets.iter().any(|target| target == base)
}

/// Classify the corpus root from a [`RootProbe`].
pub(super) fn classify_root(probe: &RootProbe, targets: &[String], repo_url: &str) -> RootState {
    let (entries, checkout) = match probe {
        RootProbe::Missing => return RootState::Missing,
        RootProbe::Present { entries, checkout } => (entries, checkout),
    };
    match checkout {
        CheckoutProbe::Checkout {
            origin: Some(origin),
        } if urls_equal(origin, repo_url) => RootState::Checkout,
        CheckoutProbe::Checkout { origin } => RootState::ForeignCheckout {
            origin: origin.clone(),
        },
        CheckoutProbe::NotCheckout if entries.is_empty() => RootState::Empty,
        CheckoutProbe::NotCheckout => {
            let unexpected: Vec<String> = entries
                .iter()
                .filter(|entry| !is_target_or_scratch(entry, targets))
                .cloned()
                .collect();
            if unexpected.is_empty() {
                RootState::Adoptable
            } else {
                RootState::NotAdoptable { unexpected }
            }
        }
    }
}

/// Decide what `pull` does, or refuse with an explanation.
pub(super) fn plan_pull(state: &RootState, root: &Path, repo_url: &str) -> Result<PullAction> {
    match state {
        RootState::Missing | RootState::Empty => Ok(PullAction::Clone),
        RootState::Checkout => Ok(PullAction::Update),
        RootState::Adoptable => Ok(PullAction::Adopt),
        RootState::ForeignCheckout { origin } => bail!(
            "{} is a git checkout of {}, not of {repo_url}; refusing to touch it. Point \
             --corpus-root at another directory",
            root.display(),
            origin
                .as_deref()
                .unwrap_or("a repository with no `origin` remote")
        ),
        RootState::NotAdoptable { unexpected } => bail!(
            "{} is not empty and not a checkout of {repo_url}, and it holds entries that are \
             not fuzz target directories ({}); refusing to adopt it. Move it away or point \
             --corpus-root at another directory",
            root.display(),
            unexpected.join(", ")
        ),
    }
}

/// Require the root to already be a checkout of the corpus repository (the
/// precondition of `push`).
pub(super) fn require_checkout(state: &RootState, root: &Path, repo_url: &str) -> Result<()> {
    match state {
        RootState::Checkout => Ok(()),
        RootState::ForeignCheckout { .. } => plan_pull(state, root, repo_url).map(|_| ()),
        _ => bail!(
            "{} is not a checkout of {repo_url}; run `cargo xtask fuzz-corpus pull` first",
            root.display()
        ),
    }
}

/// Keep only the characters of a hostname that are safe in a commit subject;
/// `unknown` when nothing is left.
fn sanitize_host(raw: &str) -> String {
    let clean: String = raw
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    if clean.is_empty() {
        "unknown".to_owned()
    } else {
        clean
    }
}

/// Format `secs` since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ` (UTC).
///
/// Civil-date conversion on unsigned integers (days from 0000-03-01), so it
/// needs neither a date crate nor a numeric cast.
fn format_utc(secs: u64) -> String {
    const SECS_PER_DAY: u64 = 86_400;
    // Days from 0000-03-01 to 1970-01-01.
    const EPOCH_SHIFT: u64 = 719_468;
    let days = secs / SECS_PER_DAY + EPOCH_SHIFT;
    let secs_of_day = secs % SECS_PER_DAY;
    let era = days / 146_097;
    let day_of_era = days % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let (month, year_bump) = if shifted_month < 10 {
        (shifted_month + 3, 0)
    } else {
        (shifted_month - 9, 1)
    };
    let year = year_of_era + era * 400 + year_bump;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3_600,
        secs_of_day % 3_600 / 60,
        secs_of_day % 60
    )
}

/// Who is committing and when; named in every commit this command makes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    /// Sanitised hostname (`unknown` when it cannot be determined).
    host: String,
    /// UTC time, `YYYY-MM-DDTHH:MM:SSZ`.
    timestamp: String,
}

impl Stamp {
    /// The current host and time.
    fn now() -> Self {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let raw_host = cmd!("hostname")
            .stderr_null()
            .read()
            .ok()
            .or_else(|| std::env::var("HOSTNAME").ok())
            .or_else(|| std::env::var("COMPUTERNAME").ok())
            .unwrap_or_default();
        Self {
            host: sanitize_host(&raw_host),
            timestamp: format_utc(secs),
        }
    }
}

/// A commit's subject and optional body.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CommitMessage {
    /// The first line.
    subject: String,
    /// The paragraph after it, if any.
    body: Option<String>,
}

impl CommitMessage {
    /// The message of a published corpus: `corpus: <host> <timestamp>` and a
    /// per-target file count.
    fn publish(stamp: &Stamp, counts: &[(String, usize)]) -> Self {
        let body = counts
            .iter()
            .map(|(target, files)| format!("{target}: {files} files"))
            .collect::<Vec<_>>()
            .join("\n");
        Self {
            subject: format!("corpus: {} {}", stamp.host, stamp.timestamp),
            body: (!body.is_empty()).then_some(body),
        }
    }

    /// The message of the commit that sets local, unpublished files aside
    /// before a pull integrates the remote.
    fn snapshot(stamp: &Stamp) -> Self {
        Self {
            subject: format!(
                "corpus: {} {} (local files before pull)",
                stamp.host, stamp.timestamp
            ),
            body: None,
        }
    }
}

/// Whether a commit needs the fallback identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommitIdentity {
    /// git has a `user.email`; use it.
    Configured,
    /// No `user.email`; pass [`FALLBACK_IDENTITY`] on the commit command.
    Fallback,
}

impl CommitIdentity {
    /// Decide from the result of `git config user.email`.
    fn detect(config_email: &GitOutput) -> Self {
        if config_email.success() && !config_email.stdout.trim().is_empty() {
            Self::Configured
        } else {
            Self::Fallback
        }
    }

    /// The `-c` options for the commit command.
    const fn config_args(self) -> &'static [&'static str] {
        match self {
            Self::Configured => &[],
            Self::Fallback => &FALLBACK_IDENTITY,
        }
    }
}

/// Why a `git push` failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PushFailure {
    /// The remote has commits the local branch lacks (non-fast-forward).
    Rejected,
    /// Anything else (authentication, network, permissions).
    Other,
}

/// Classify a failed push from git's stderr.
fn classify_push_failure(stderr: &str) -> PushFailure {
    const MARKERS: [&str; 3] = ["non-fast-forward", "fetch first", "[rejected]"];
    if MARKERS.iter().any(|marker| stderr.contains(marker)) {
        PushFailure::Rejected
    } else {
        PushFailure::Other
    }
}

/// What to do after a failed push.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PushStep {
    /// Fetch, rebase onto `origin/main`, push again.
    Retry,
    /// Stop and report.
    GiveUp,
}

/// Decide the next step after push number `attempts_made` failed.
const fn next_push_step(attempts_made: u32, failure: PushFailure) -> PushStep {
    match failure {
        PushFailure::Rejected if attempts_made < MAX_PUSH_ATTEMPTS => PushStep::Retry,
        PushFailure::Rejected | PushFailure::Other => PushStep::GiveUp,
    }
}

// ---------------------------------------------------------------------------
// Running git
// ---------------------------------------------------------------------------

/// Whether a git command talks to the remote (and so carries [`RemoteAuth`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GitScope {
    /// Touches only the local repository.
    Local,
    /// Clones, fetches or pushes.
    Remote,
}

/// What a finished git command produced.
#[derive(Debug)]
struct GitOutput {
    /// Exit code; `None` when the process was killed by a signal.
    code: Option<i32>,
    /// Captured standard output.
    stdout: String,
    /// Captured standard error.
    stderr: String,
}

impl GitOutput {
    /// Whether the command exited with status 0.
    const fn success(&self) -> bool {
        matches!(self.code, Some(0))
    }
}

/// Runs git commands for the corpus repository.
struct Git {
    /// How remote commands authenticate.
    auth: RemoteAuth,
    /// Extra environment for every command (tests pin git's configuration).
    env: Vec<(OsString, OsString)>,
    /// Variables removed from every command's environment on top of
    /// [`INHERITED_REPO_ENV`] (tests drop the identity a git hook exports).
    unset: Vec<OsString>,
}

impl Git {
    /// A runner that authenticates remote commands with `auth`.
    const fn new(auth: RemoteAuth) -> Self {
        Self {
            auth,
            env: Vec::new(),
            unset: Vec::new(),
        }
    }

    /// Also set `key=value` on every command.
    #[cfg(all(test, unix))]
    fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Also remove `key` from every command's environment.
    #[cfg(all(test, unix))]
    fn without_env(mut self, key: &str) -> Self {
        self.unset.push(key.into());
        self
    }

    /// Run `git [-C dir] args...` and capture its output; a non-zero exit is
    /// **not** an error here.
    fn run(&self, scope: GitScope, dir: Option<&Path>, args: &[&str]) -> Result<GitOutput> {
        let mut full: Vec<OsString> = Vec::new();
        if scope == GitScope::Remote {
            full.extend(self.auth.config_args().iter().map(OsString::from));
        }
        if let Some(dir) = dir {
            full.push("-C".into());
            full.push(dir.as_os_str().to_owned());
        }
        full.extend(args.iter().map(OsString::from));

        // The credential helper is left out of the log on purpose.
        tracing::info!(
            "running command: git {}{}",
            dir.map_or_else(String::new, |dir| format!("-C {} ", dir.display())),
            args.join(" ")
        );

        let mut expr = cmd("git", &full)
            .stdout_capture()
            .stderr_capture()
            .unchecked();
        // The child's whole environment, built explicitly: the inherited one
        // plus any injected variables (tests), minus the variables that would
        // redirect git to another repository. Injected variables go through the
        // same filter, so a test can prove the removal by injecting one.
        let mut env: std::collections::HashMap<OsString, OsString> = std::env::vars_os().collect();
        for (key, value) in &self.env {
            env.insert(key.into(), value.into());
        }
        for key in INHERITED_REPO_ENV {
            env.remove(std::ffi::OsStr::new(key));
        }
        for key in &self.unset {
            env.remove(key);
        }
        expr = expr.full_env(env);
        if scope == GitScope::Remote && self.auth == RemoteAuth::Token {
            // CI must fail, not hang, when the token is rejected.
            expr = expr.env("GIT_TERMINAL_PROMPT", "0");
        }
        let output = expr
            .run()
            .wrap_err_with(|| format!("failed to run `git {}`", args.join(" ")))?;
        Ok(GitOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// Like [`Git::run`], but a non-zero exit is an error carrying git's
    /// stderr. Returns the trimmed stdout.
    fn run_ok(&self, scope: GitScope, dir: Option<&Path>, args: &[&str]) -> Result<String> {
        let output = self.run(scope, dir, args)?;
        if output.success() {
            Ok(output.stdout.trim().to_owned())
        } else {
            bail!(
                "`git {}` failed ({}): {}",
                args.join(" "),
                output
                    .code
                    .map_or_else(|| "killed by a signal".to_owned(), |c| format!("exit {c}")),
                output.stderr.trim()
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Probing and pulling
// ---------------------------------------------------------------------------

/// Inspect `root` on disk.
fn probe_root(git: &Git, root: &Path) -> Result<RootProbe> {
    let read = match fs::read_dir(root) {
        Ok(read) => read,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(RootProbe::Missing),
        Err(err) => {
            return Err(err).wrap_err_with(|| format!("failed to read {}", root.display()));
        }
    };
    let mut entries = Vec::new();
    for entry in read {
        let entry = entry.wrap_err_with(|| format!("failed to read {}", root.display()))?;
        entries.push(entry.file_name().to_string_lossy().into_owned());
    }
    entries.sort();
    let checkout = if root.join(".git").exists() {
        let origin = git.run(
            GitScope::Local,
            Some(root),
            &["remote", "get-url", "origin"],
        )?;
        CheckoutProbe::Checkout {
            origin: origin
                .success()
                .then(|| origin.stdout.trim().to_owned())
                .filter(|url| !url.is_empty()),
        }
    } else {
        CheckoutProbe::NotCheckout
    };
    Ok(RootProbe::Present { entries, checkout })
}

/// `git clone` the repository into `root` (created if missing).
fn clone_into(git: &Git, root: &Path, repo_url: &str) -> Result<()> {
    fs::create_dir_all(root).wrap_err_with(|| format!("failed to create {}", root.display()))?;
    git.run_ok(
        GitScope::Remote,
        Some(root),
        &["clone", "--branch", BRANCH, repo_url, "."],
    )?;
    tracing::info!("cloned the corpus repository into {}", root.display());
    Ok(())
}

/// Turn a pre-existing local corpus into a checkout of the repository.
///
/// Local files that share a name with a tracked file are byte-identical by
/// construction (names are content hashes), so the forced checkout loses
/// nothing; every other local file stays untracked for `push` to commit.
/// If anything fails after `git init`, the `.git` created here is removed
/// again so the corpus is left exactly as it was found.
fn adopt(git: &Git, root: &Path, repo_url: &str) -> Result<()> {
    git.run_ok(GitScope::Local, Some(root), &["init", "-b", BRANCH])?;
    if let Err(err) = attach_to_remote(git, root, repo_url) {
        if let Err(cleanup) = fs::remove_dir_all(root.join(".git")) {
            tracing::warn!(
                "could not remove the partial {}/.git after a failed adoption: {cleanup}",
                root.display()
            );
        }
        return Err(err);
    }
    let untracked = git.run_ok(
        GitScope::Local,
        Some(root),
        &["ls-files", "--others", "--exclude-standard"],
    )?;
    tracing::info!(
        "adopted the existing corpus at {}: {} local files are untracked and will be \
         committed by `fuzz-corpus push`",
        root.display(),
        untracked.lines().count()
    );
    Ok(())
}

/// The part of [`adopt`] that follows `git init`.
fn attach_to_remote(git: &Git, root: &Path, repo_url: &str) -> Result<()> {
    git.run_ok(
        GitScope::Local,
        Some(root),
        &["remote", "add", "origin", repo_url],
    )?;
    git.run_ok(GitScope::Remote, Some(root), &["fetch", "origin", BRANCH])?;
    git.run_ok(
        GitScope::Local,
        Some(root),
        &["checkout", "-f", "-B", BRANCH, &format!("origin/{BRANCH}")],
    )?;
    Ok(())
}

/// Which identity commands that create commits (commit, rebase) must use in
/// `root`.
fn commit_identity(git: &Git, root: &Path) -> Result<CommitIdentity> {
    let email = git.run(GitScope::Local, Some(root), &["config", "user.email"])?;
    Ok(CommitIdentity::detect(&email))
}

/// Whether [`stage_and_commit`] made a commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommitOutcome {
    /// A commit was created.
    Committed,
    /// The working tree matches `HEAD`; nothing was committed.
    NothingToCommit,
}

/// `git add -A`, then commit if anything is staged.
fn stage_and_commit(git: &Git, root: &Path, message: &CommitMessage) -> Result<CommitOutcome> {
    git.run_ok(GitScope::Local, Some(root), &["add", "-A"])?;
    let diff = git.run(
        GitScope::Local,
        Some(root),
        &["diff", "--cached", "--quiet"],
    )?;
    match diff.code {
        Some(0) => return Ok(CommitOutcome::NothingToCommit),
        Some(1) => {}
        _ => bail!("`git diff --cached --quiet` failed: {}", diff.stderr.trim()),
    }
    let identity = commit_identity(git, root)?;
    let mut args: Vec<&str> = identity.config_args().to_vec();
    args.extend(["commit", "--quiet", "-m", &message.subject]);
    if let Some(body) = &message.body {
        args.extend(["-m", body]);
    }
    git.run_ok(GitScope::Local, Some(root), &args)?;
    Ok(CommitOutcome::Committed)
}

/// Rebase the checked-out branch onto `origin/main`, aborting the rebase and
/// explaining if it fails (which content-addressed files should never cause).
fn rebase_onto_origin(git: &Git, root: &Path) -> Result<()> {
    let onto = format!("origin/{BRANCH}");
    // A rebase writes commits, so it needs a committer identity as well.
    let mut args: Vec<&str> = commit_identity(git, root)?.config_args().to_vec();
    args.extend(["rebase", &onto]);
    let rebase = git.run(GitScope::Local, Some(root), &args)?;
    if rebase.success() {
        return Ok(());
    }
    if let Err(err) = git.run_ok(GitScope::Local, Some(root), &["rebase", "--abort"]) {
        tracing::warn!("`git rebase --abort` failed: {err:#}");
    }
    bail!(
        "rebasing {} onto {onto} failed ({}); the rebase was aborted and nothing was lost. \
         Corpus files are named by content hash, so this is unexpected: inspect it with \
         `git -C {} status`, resolve it by hand, then re-run",
        root.display(),
        rebase.stderr.trim(),
        root.display()
    )
}

/// Bring an existing checkout up to date with `origin/main`.
///
/// Local files not yet committed are first set aside in a local commit: a
/// merge would otherwise refuse when an untracked local file has the name of
/// an incoming one, which happens whenever two machines independently find
/// the same input (same bytes, same name). The commit then rebases away
/// cleanly, since both sides added identical content.
fn update_checkout(git: &Git, root: &Path, stamp: &Stamp) -> Result<()> {
    let branch = git.run_ok(
        GitScope::Local,
        Some(root),
        &["symbolic-ref", "--short", "HEAD"],
    )?;
    if branch != BRANCH {
        bail!(
            "{} is on `{branch}`, not `{BRANCH}`; check out `{BRANCH}` first",
            root.display()
        );
    }
    if stage_and_commit(git, root, &CommitMessage::snapshot(stamp))? == CommitOutcome::Committed {
        tracing::info!("committed local corpus files that were not yet in the repository");
    }
    git.run_ok(GitScope::Remote, Some(root), &["fetch", "origin", BRANCH])?;
    let onto = format!("origin/{BRANCH}");
    let merge = git.run(
        GitScope::Local,
        Some(root),
        &["merge", "--ff-only", "--quiet", &onto],
    )?;
    if !merge.success() {
        tracing::info!("local commits are not yet pushed; rebasing them onto {onto}");
        rebase_onto_origin(git, root)?;
    }
    Ok(())
}

/// Probe the root, decide, and make it an up-to-date checkout.
fn pull(git: &Git, root: &Path, targets: &[String], repo_url: &str, stamp: &Stamp) -> Result<()> {
    let state = classify_root(&probe_root(git, root)?, targets, repo_url);
    match plan_pull(&state, root, repo_url)? {
        PullAction::Clone => clone_into(git, root, repo_url),
        PullAction::Update => update_checkout(git, root, stamp),
        PullAction::Adopt => adopt(git, root, repo_url),
    }
    .wrap_err_with(|| format!("failed to pull the corpus into {}", root.display()))
}

// ---------------------------------------------------------------------------
// Pushing
// ---------------------------------------------------------------------------

/// What [`commit_and_push`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PushOutcome {
    /// The working tree matched `HEAD` and nothing was ahead of the remote.
    NothingToPublish,
    /// The commit was pushed on attempt number `attempts` (1-based).
    Published {
        /// How many pushes it took.
        attempts: u32,
    },
}

/// Push `main`, and when the remote moved meanwhile, rebase onto it and try
/// again, up to [`MAX_PUSH_ATTEMPTS`] pushes in all.
fn push_with_retry(git: &Git, root: &Path) -> Result<u32> {
    let mut attempt = 1;
    loop {
        let push = git.run(GitScope::Remote, Some(root), &["push", "origin", BRANCH])?;
        if push.success() {
            return Ok(attempt);
        }
        match next_push_step(attempt, classify_push_failure(&push.stderr)) {
            PushStep::GiveUp => bail!(
                "`git push origin {BRANCH}` failed after {attempt} attempt(s): {}",
                push.stderr.trim()
            ),
            PushStep::Retry => {
                tracing::warn!("push rejected (attempt {attempt}); rebasing onto the remote");
                git.run_ok(GitScope::Remote, Some(root), &["fetch", "origin", BRANCH])?;
                rebase_onto_origin(git, root)?;
                attempt += 1;
            }
        }
    }
}

/// Commit whatever changed in the working tree and push it.
///
/// This is the git-level half of `push` (minimisation happens before it), so
/// it can be exercised without a fuzz binary.
fn commit_and_push(git: &Git, root: &Path, message: &CommitMessage) -> Result<PushOutcome> {
    stage_and_commit(git, root, message)?;
    // Commits made earlier (a pull's snapshot, a push that failed to reach the
    // remote) also need publishing, so ask git rather than trusting the outcome.
    let ahead = git.run_ok(
        GitScope::Local,
        Some(root),
        &["rev-list", "--count", &format!("origin/{BRANCH}..HEAD")],
    )?;
    if ahead == "0" {
        return Ok(PushOutcome::NothingToPublish);
    }
    let attempts = push_with_retry(git, root)?;
    Ok(PushOutcome::Published { attempts })
}

/// Files directly inside each target's corpus directory under `root`
/// (targets without a directory are omitted).
fn target_file_counts(root: &Path, targets: &[String]) -> Result<Vec<(String, usize)>> {
    let mut counts = Vec::new();
    for target in targets {
        let dir = root.join(target);
        if dir.is_dir() {
            counts.push((target.clone(), count_files(&dir)?));
        }
    }
    Ok(counts)
}

/// `fuzz-corpus push`.
fn push(ctx: &FuzzContext, session: &Session, git: &Git, repo_url: &str) -> Result<()> {
    let root = &session.corpus_root;
    let stamp = Stamp::now();
    let state = classify_root(&probe_root(git, root)?, &ctx.targets, repo_url);
    require_checkout(&state, root, repo_url)?;
    update_checkout(git, root, &stamp)
        .wrap_err_with(|| format!("failed to update {} before publishing", root.display()))?;
    cmin_targets(ctx, session, ctx.targets.clone())
        .wrap_err("corpus minimisation failed; nothing was published")?;
    let counts = target_file_counts(root, &ctx.targets)?;
    match commit_and_push(git, root, &CommitMessage::publish(&stamp, &counts))? {
        PushOutcome::NothingToPublish => {
            tracing::info!("nothing to publish: the corpus matches the repository");
        }
        PushOutcome::Published { attempts } => {
            tracing::info!("published the corpus ({attempts} push attempt(s))");
        }
    }
    Ok(())
}

/// `cargo xtask fuzz-corpus <pull|push>`
pub(super) fn run(args: &FuzzCorpusArgs) -> Result<()> {
    let ctx = FuzzContext::discover()?;
    let session = Session::resolve(&ctx, &args.env);
    let git = Git::new(RemoteAuth::from_env());
    match args.action {
        CorpusAction::Pull => pull(
            &git,
            &session.corpus_root,
            &ctx.targets,
            &args.repo_url,
            &Stamp::now(),
        ),
        CorpusAction::Push => push(&ctx, &session, &git, &args.repo_url),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://github.com/fredsystems/freminal-fuzz-corpus.git";

    fn targets() -> Vec<String> {
        vec!["t1".to_owned(), "t2".to_owned()]
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn present(entries: &[&str], checkout: CheckoutProbe) -> RootProbe {
        RootProbe::Present {
            entries: strings(entries),
            checkout,
        }
    }

    fn stamp() -> Stamp {
        Stamp {
            host: "box".to_owned(),
            timestamp: "2026-10-18T12:00:00Z".to_owned(),
        }
    }

    #[test]
    fn default_repo_url_is_the_private_corpus_repository() {
        assert_eq!(DEFAULT_REPO_URL, URL);
    }

    #[test]
    fn spellings_of_one_repository_compare_equal() {
        for other in [
            "https://github.com/fredsystems/freminal-fuzz-corpus",
            "https://github.com/fredsystems/freminal-fuzz-corpus/",
            "https://github.com/fredsystems/freminal-fuzz-corpus.git/",
            "git@github.com:fredsystems/freminal-fuzz-corpus.git",
            "git@github.com:fredsystems/freminal-fuzz-corpus",
            "ssh://git@github.com/fredsystems/freminal-fuzz-corpus.git",
            "https://x-access-token:s3cret@github.com/fredsystems/freminal-fuzz-corpus.git",
            "HTTPS://GitHub.com/FredSystems/Freminal-Fuzz-Corpus.git",
            "  https://github.com/fredsystems/freminal-fuzz-corpus.git\n",
        ] {
            assert!(urls_equal(URL, other), "{other}");
        }
    }

    #[test]
    fn different_repositories_compare_unequal() {
        for other in [
            "https://github.com/fredsystems/freminal.git",
            "https://github.com/someoneelse/freminal-fuzz-corpus.git",
            "https://gitlab.com/fredsystems/freminal-fuzz-corpus.git",
            "git@github.com:fredsystems/freminal.git",
            "",
        ] {
            assert!(!urls_equal(URL, other), "{other}");
        }
    }

    #[test]
    fn local_paths_normalise_without_mangling() {
        assert!(urls_equal("/tmp/x/remote.git", "/tmp/x/remote"));
        assert!(urls_equal("/tmp/x/remote.git", "file:///tmp/x/remote.git"));
        assert!(!urls_equal("/tmp/x/remote.git", "/tmp/x/other.git"));
        assert!(urls_equal("C:/repos/corpus.git", "C:/repos/corpus"));
    }

    #[test]
    fn missing_and_empty_roots_classify_as_such() {
        assert_eq!(
            classify_root(&RootProbe::Missing, &targets(), URL),
            RootState::Missing
        );
        assert_eq!(
            classify_root(&present(&[], CheckoutProbe::NotCheckout), &targets(), URL),
            RootState::Empty
        );
    }

    #[test]
    fn checkout_with_matching_origin_is_a_checkout() {
        let probe = present(
            &[".git", "t1"],
            CheckoutProbe::Checkout {
                origin: Some("git@github.com:fredsystems/freminal-fuzz-corpus.git".to_owned()),
            },
        );
        assert_eq!(classify_root(&probe, &targets(), URL), RootState::Checkout);
    }

    #[test]
    fn checkout_with_other_or_no_origin_is_foreign() {
        for origin in [
            Some("https://github.com/fredsystems/freminal.git".to_owned()),
            None,
        ] {
            let probe = present(
                &[".git"],
                CheckoutProbe::Checkout {
                    origin: origin.clone(),
                },
            );
            assert_eq!(
                classify_root(&probe, &targets(), URL),
                RootState::ForeignCheckout { origin }
            );
        }
    }

    #[test]
    fn non_git_root_of_targets_and_scratch_dirs_is_adoptable() {
        let probe = present(
            &["t1", "t2", "t1.cmin", "t2.old"],
            CheckoutProbe::NotCheckout,
        );
        assert_eq!(classify_root(&probe, &targets(), URL), RootState::Adoptable);
    }

    #[test]
    fn non_git_root_with_other_entries_is_not_adoptable() {
        let probe = present(
            &["t1", "notes.txt", "t3", "t3.cmin"],
            CheckoutProbe::NotCheckout,
        );
        assert_eq!(
            classify_root(&probe, &targets(), URL),
            RootState::NotAdoptable {
                unexpected: strings(&["notes.txt", "t3", "t3.cmin"])
            }
        );
    }

    #[test]
    fn scratch_suffix_alone_is_not_a_target() {
        assert!(is_target_or_scratch("t1.cmin", &targets()));
        assert!(is_target_or_scratch("t1.old", &targets()));
        assert!(!is_target_or_scratch(".cmin", &targets()));
        assert!(!is_target_or_scratch("t1.cmin.old", &targets()));
        assert!(!is_target_or_scratch("t1.bak", &targets()));
    }

    #[test]
    fn pull_plan_covers_every_state() {
        let root = Path::new("/c");
        let plan = |state| plan_pull(&state, root, URL);
        assert_eq!(plan(RootState::Missing).unwrap(), PullAction::Clone);
        assert_eq!(plan(RootState::Empty).unwrap(), PullAction::Clone);
        assert_eq!(plan(RootState::Checkout).unwrap(), PullAction::Update);
        assert_eq!(plan(RootState::Adoptable).unwrap(), PullAction::Adopt);

        let foreign = plan(RootState::ForeignCheckout {
            origin: Some("https://example.com/other.git".to_owned()),
        })
        .unwrap_err()
        .to_string();
        assert!(
            foreign.contains("https://example.com/other.git"),
            "{foreign}"
        );
        assert!(foreign.contains("refusing to touch"), "{foreign}");

        let no_origin = plan(RootState::ForeignCheckout { origin: None })
            .unwrap_err()
            .to_string();
        assert!(no_origin.contains("no `origin`"), "{no_origin}");

        let unexpected = plan(RootState::NotAdoptable {
            unexpected: strings(&["a", "b"]),
        })
        .unwrap_err()
        .to_string();
        assert!(unexpected.contains("a, b"), "{unexpected}");
    }

    #[test]
    fn push_requires_a_checkout_and_says_to_pull() {
        let root = Path::new("/c");
        assert!(require_checkout(&RootState::Checkout, root, URL).is_ok());
        for state in [
            RootState::Missing,
            RootState::Empty,
            RootState::Adoptable,
            RootState::NotAdoptable {
                unexpected: strings(&["x"]),
            },
        ] {
            let err = require_checkout(&state, root, URL).unwrap_err().to_string();
            assert!(err.contains("fuzz-corpus pull"), "{err}");
        }
        let foreign = require_checkout(&RootState::ForeignCheckout { origin: None }, root, URL)
            .unwrap_err()
            .to_string();
        assert!(foreign.contains("refusing to touch"), "{foreign}");
    }

    #[test]
    fn publish_message_names_host_time_and_counts() {
        let message =
            CommitMessage::publish(&stamp(), &[("t1".to_owned(), 12), ("t2".to_owned(), 0)]);
        assert_eq!(message.subject, "corpus: box 2026-10-18T12:00:00Z");
        assert_eq!(message.body.as_deref(), Some("t1: 12 files\nt2: 0 files"));
        assert_eq!(CommitMessage::publish(&stamp(), &[]).body, None);
    }

    #[test]
    fn snapshot_message_is_distinguishable_from_a_publish() {
        let message = CommitMessage::snapshot(&stamp());
        assert!(message
            .subject
            .starts_with("corpus: box 2026-10-18T12:00:00Z"));
        assert!(message.subject.contains("before pull"));
        assert_eq!(message.body, None);
    }

    #[test]
    fn hostnames_are_sanitised_and_default_to_unknown() {
        assert_eq!(sanitize_host("my-box.local\n"), "my-box.local");
        assert_eq!(sanitize_host("a b\tc"), "abc");
        assert_eq!(sanitize_host(""), "unknown");
        assert_eq!(sanitize_host("  \n"), "unknown");
        assert_eq!(sanitize_host("!!"), "unknown");
    }

    #[test]
    fn utc_formatting_matches_known_instants() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_utc(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(format_utc(1_078_099_200), "2004-03-01T00:00:00Z");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        // The last second of a year, and the first of the next.
        assert_eq!(format_utc(1_735_689_599), "2024-12-31T23:59:59Z");
        assert_eq!(format_utc(1_735_689_600), "2025-01-01T00:00:00Z");
    }

    #[test]
    fn push_retries_only_rejections_and_only_within_the_attempt_limit() {
        assert_eq!(next_push_step(1, PushFailure::Rejected), PushStep::Retry);
        assert_eq!(next_push_step(2, PushFailure::Rejected), PushStep::Retry);
        assert_eq!(next_push_step(3, PushFailure::Rejected), PushStep::GiveUp);
        assert_eq!(next_push_step(1, PushFailure::Other), PushStep::GiveUp);
    }

    #[test]
    fn push_failures_are_classified_from_git_stderr() {
        let rejected = " ! [rejected]        main -> main (non-fast-forward)\n\
                        error: failed to push some refs";
        assert_eq!(classify_push_failure(rejected), PushFailure::Rejected);
        assert_eq!(
            classify_push_failure("hint: Updates were rejected ... (fetch first)"),
            PushFailure::Rejected
        );
        assert_eq!(
            classify_push_failure("fatal: Authentication failed for 'https://github.com/x'"),
            PushFailure::Other
        );
        assert_eq!(classify_push_failure(""), PushFailure::Other);
    }

    #[test]
    fn token_presence_is_decided_from_the_value() {
        assert_eq!(RemoteAuth::from_token_value(None), RemoteAuth::Ambient);
        assert_eq!(RemoteAuth::from_token_value(Some("")), RemoteAuth::Ambient);
        assert_eq!(
            RemoteAuth::from_token_value(Some("  \n")),
            RemoteAuth::Ambient
        );
        assert_eq!(
            RemoteAuth::from_token_value(Some("ghp_x")),
            RemoteAuth::Token
        );
    }

    #[test]
    fn token_auth_passes_the_variable_name_not_its_value() {
        let args = RemoteAuth::Token.config_args();
        assert_eq!(args[..3], ["-c", "credential.helper=", "-c"]);
        assert_eq!(args.len(), 4);
        assert!(
            args[3].contains("${FREMINAL_FUZZ_CORPUS_TOKEN}"),
            "{}",
            args[3]
        );
        assert!(args[3].starts_with("credential.helper=!"));
        assert_eq!(RemoteAuth::Ambient.config_args(), <&[&str]>::default());
    }

    #[test]
    fn fallback_identity_is_used_only_without_a_configured_email() {
        let output = |code, stdout: &str| GitOutput {
            code,
            stdout: stdout.to_owned(),
            stderr: String::new(),
        };
        assert_eq!(
            CommitIdentity::detect(&output(Some(0), "me@example.com\n")),
            CommitIdentity::Configured
        );
        for unset in [output(Some(1), ""), output(Some(0), "\n"), output(None, "")] {
            assert_eq!(CommitIdentity::detect(&unset), CommitIdentity::Fallback);
        }
        assert_eq!(
            CommitIdentity::Configured.config_args(),
            <&[&str]>::default()
        );
        assert_eq!(CommitIdentity::Fallback.config_args(), FALLBACK_IDENTITY);
    }

    #[test]
    fn corpus_args_parse_with_defaults() {
        use clap::Parser;
        #[derive(Parser)]
        struct Wrapper {
            #[command(flatten)]
            args: FuzzCorpusArgs,
        }
        let parsed = Wrapper::try_parse_from(["x", "pull"]).unwrap().args;
        assert_eq!(parsed.action, CorpusAction::Pull);
        assert_eq!(parsed.repo_url, DEFAULT_REPO_URL);
        assert!(Wrapper::try_parse_from(["x"]).is_err());
        assert!(Wrapper::try_parse_from(["x", "sync"]).is_err());
    }

    // -- git-backed tests ---------------------------------------------------

    #[cfg(unix)]
    mod git_backed {
        use super::*;
        use std::path::PathBuf;

        /// Git with the user's and the system's configuration ignored.
        fn hermetic() -> Git {
            Git::new(RemoteAuth::Ambient)
                .with_env("GIT_CONFIG_GLOBAL", "/dev/null")
                .with_env("GIT_CONFIG_NOSYSTEM", "1")
                // A git hook (pre-commit runs this suite) exports the
                // committing user's identity, which would override the
                // fallback identity the tests assert on.
                .without_env("GIT_AUTHOR_NAME")
                .without_env("GIT_AUTHOR_EMAIL")
                .without_env("GIT_AUTHOR_DATE")
                .without_env("GIT_COMMITTER_NAME")
                .without_env("GIT_COMMITTER_EMAIL")
                .without_env("GIT_COMMITTER_DATE")
        }

        const SEED_IDENTITY: [&str; 4] = [
            "-c",
            "user.name=seed",
            "-c",
            "user.email=seed@example.invalid",
        ];

        /// Commit everything in `dir` with a fixed identity.
        fn seed_commit(git: &Git, dir: &Path, message: &str) {
            git.run_ok(GitScope::Local, Some(dir), &["add", "-A"])
                .unwrap();
            let mut args: Vec<&str> = SEED_IDENTITY.to_vec();
            args.extend(["commit", "-q", "-m", message]);
            git.run_ok(GitScope::Local, Some(dir), &args).unwrap();
        }

        /// Write `files` (`path`, `content`) under `dir`.
        fn write_files(dir: &Path, files: &[(&str, &str)]) {
            for (path, content) in files {
                let full = dir.join(path);
                fs::create_dir_all(full.parent().unwrap()).unwrap();
                fs::write(full, content).unwrap();
            }
        }

        /// A bare repository `<base>/<name>.git` seeded with a README, a
        /// `.gitignore` and `t1/dup`; returns its path (also its URL).
        fn seeded_remote(git: &Git, base: &Path, name: &str) -> String {
            let remote = base.join(format!("{name}.git"));
            let remote_str = remote.to_str().unwrap().to_owned();
            git.run_ok(
                GitScope::Local,
                Some(base),
                &["init", "-q", "--bare", "-b", "main", &remote_str],
            )
            .unwrap();
            let seed = base.join(format!("{name}-seed"));
            fs::create_dir_all(&seed).unwrap();
            git.run_ok(GitScope::Local, Some(&seed), &["init", "-q", "-b", "main"])
                .unwrap();
            write_files(
                &seed,
                &[
                    ("README.md", "corpus\n"),
                    (".gitignore", "*.cmin/\n*.old/\n"),
                    ("t1/dup", "same bytes"),
                ],
            );
            seed_commit(git, &seed, "seed");
            git.run_ok(
                GitScope::Local,
                Some(&seed),
                &["push", "-q", &remote_str, "HEAD:main"],
            )
            .unwrap();
            remote_str
        }

        /// A second working clone of `url`, as another machine would have.
        fn clone_of(git: &Git, base: &Path, url: &str, name: &str) -> PathBuf {
            let dir = base.join(name);
            git.run_ok(
                GitScope::Local,
                Some(base),
                &["clone", "-q", url, dir.to_str().unwrap()],
            )
            .unwrap();
            dir
        }

        fn log_subjects(git: &Git, dir: &Path) -> Vec<String> {
            git.run_ok(GitScope::Local, Some(dir), &["log", "--format=%s"])
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        fn pull_into(git: &Git, root: &Path, url: &str) -> Result<()> {
            pull(git, root, &targets(), url, &stamp())
        }

        fn publish(git: &Git, root: &Path) -> PushOutcome {
            commit_and_push(git, root, &CommitMessage::publish(&stamp(), &[])).unwrap()
        }

        #[test]
        fn pull_clones_into_a_missing_root_and_creates_parents() {
            let base = tempfile::tempdir().unwrap();
            let git = hermetic();
            let url = seeded_remote(&git, base.path(), "remote");
            let root = base.path().join("deep").join("corpus");

            pull_into(&git, &root, &url).unwrap();

            assert!(root.join("README.md").is_file());
            assert!(root.join("t1").join("dup").is_file());
            assert!(root.join(".git").exists());
        }

        #[test]
        fn pull_clones_into_an_empty_root() {
            let base = tempfile::tempdir().unwrap();
            let git = hermetic();
            let url = seeded_remote(&git, base.path(), "remote");
            let root = base.path().join("corpus");
            fs::create_dir_all(&root).unwrap();

            pull_into(&git, &root, &url).unwrap();

            assert!(root.join("t1").join("dup").is_file());
        }

        #[test]
        fn second_pull_is_a_no_op_and_a_later_pull_fetches_new_files() {
            let base = tempfile::tempdir().unwrap();
            let git = hermetic();
            let url = seeded_remote(&git, base.path(), "remote");
            let root = base.path().join("corpus");
            pull_into(&git, &root, &url).unwrap();
            let before = log_subjects(&git, &root);

            pull_into(&git, &root, &url).unwrap();
            assert_eq!(log_subjects(&git, &root), before);

            let other = clone_of(&git, base.path(), &url, "other");
            write_files(&other, &[("t2/from_other", "o")]);
            seed_commit(&git, &other, "other machine");
            git.run_ok(
                GitScope::Remote,
                Some(&other),
                &["push", "-q", "origin", "main"],
            )
            .unwrap();

            pull_into(&git, &root, &url).unwrap();
            assert!(root.join("t2").join("from_other").is_file());
        }

        #[test]
        fn pull_adopts_a_non_git_corpus_and_push_publishes_its_new_files() {
            let base = tempfile::tempdir().unwrap();
            let git = hermetic();
            let url = seeded_remote(&git, base.path(), "remote");
            let root = base.path().join("corpus");
            // `t1/dup` has the remote's name and bytes; `t1/local_only` is new;
            // `t2.cmin` is scratch.
            write_files(
                &root,
                &[
                    ("t1/dup", "same bytes"),
                    ("t1/local_only", "mine"),
                    ("t2.cmin/x", "scratch"),
                ],
            );

            pull_into(&git, &root, &url).unwrap();

            assert!(root.join("README.md").is_file(), "tracked files arrive");
            assert_eq!(
                fs::read(root.join("t1").join("dup")).unwrap(),
                b"same bytes"
            );
            let untracked = git
                .run_ok(
                    GitScope::Local,
                    Some(&root),
                    &["ls-files", "--others", "--exclude-standard"],
                )
                .unwrap();
            assert_eq!(untracked, "t1/local_only", "scratch is ignored");

            assert_eq!(publish(&git, &root), PushOutcome::Published { attempts: 1 });

            let verify = clone_of(&git, base.path(), &url, "verify");
            assert_eq!(
                fs::read(verify.join("t1").join("local_only")).unwrap(),
                b"mine"
            );
            assert!(!verify.join("t2.cmin").exists());
        }

        #[test]
        fn failed_adoption_leaves_no_git_directory_behind() {
            let base = tempfile::tempdir().unwrap();
            let git = hermetic();
            let root = base.path().join("corpus");
            write_files(&root, &[("t1/a", "a")]);
            let missing_remote = base.path().join("no-such-remote.git");

            let err = pull_into(&git, &root, missing_remote.to_str().unwrap()).unwrap_err();

            assert!(format!("{err:#}").contains("fetch"), "{err:#}");
            assert!(!root.join(".git").exists());
            assert!(root.join("t1").join("a").is_file());
        }

        #[test]
        fn pull_refuses_a_non_corpus_directory_and_leaves_it_alone() {
            let base = tempfile::tempdir().unwrap();
            let git = hermetic();
            let url = seeded_remote(&git, base.path(), "remote");
            let root = base.path().join("stuff");
            write_files(&root, &[("t1/a", "a"), ("documents/b", "b")]);

            let err = pull_into(&git, &root, &url).unwrap_err();

            assert!(format!("{err:#}").contains("documents"), "{err:#}");
            assert!(!root.join(".git").exists());
            assert!(!root.join("README.md").exists());
        }

        #[test]
        fn pull_refuses_a_checkout_of_another_repository() {
            let base = tempfile::tempdir().unwrap();
            let git = hermetic();
            let url = seeded_remote(&git, base.path(), "remote");
            let other_url = seeded_remote(&git, base.path(), "other");
            let root = clone_of(&git, base.path(), &other_url, "corpus");
            write_files(&root, &[("t1/untracked", "u")]);
            let head_before = git
                .run_ok(GitScope::Local, Some(&root), &["rev-parse", "HEAD"])
                .unwrap();

            let err = pull_into(&git, &root, &url).unwrap_err();

            assert!(format!("{err:#}").contains("refusing to touch"), "{err:#}");
            assert_eq!(
                git.run_ok(GitScope::Local, Some(&root), &["rev-parse", "HEAD"])
                    .unwrap(),
                head_before
            );
            assert!(root.join("t1").join("untracked").is_file());
            assert_eq!(
                git.run_ok(GitScope::Local, Some(&root), &["status", "--short"])
                    .unwrap(),
                "?? t1/untracked"
            );
        }

        #[test]
        fn pull_survives_an_untracked_file_that_the_remote_also_added() {
            let base = tempfile::tempdir().unwrap();
            let git = hermetic();
            let url = seeded_remote(&git, base.path(), "remote");
            let root = base.path().join("corpus");
            pull_into(&git, &root, &url).unwrap();
            // Another machine finds the same input and publishes it first.
            let other = clone_of(&git, base.path(), &url, "other");
            write_files(&other, &[("t2/found_twice", "same"), ("t2/theirs", "t")]);
            seed_commit(&git, &other, "other machine");
            git.run_ok(
                GitScope::Remote,
                Some(&other),
                &["push", "-q", "origin", "main"],
            )
            .unwrap();
            // This machine found it too, plus one input of its own.
            write_files(&root, &[("t2/found_twice", "same"), ("t2/mine", "m")]);

            pull_into(&git, &root, &url).unwrap();

            assert!(root.join("t2").join("theirs").is_file());
            assert_eq!(fs::read(root.join("t2").join("mine")).unwrap(), b"m");
            assert_eq!(publish(&git, &root), PushOutcome::Published { attempts: 1 });
            let verify = clone_of(&git, base.path(), &url, "verify");
            assert!(verify.join("t2").join("mine").is_file());
        }

        #[test]
        fn pull_rebases_unpushed_local_commits_onto_a_moved_remote() {
            let base = tempfile::tempdir().unwrap();
            let git = hermetic();
            let url = seeded_remote(&git, base.path(), "remote");
            let root = base.path().join("corpus");
            pull_into(&git, &root, &url).unwrap();
            write_files(&root, &[("t1/local", "l")]);
            seed_commit(&git, &root, "local, unpushed");
            let other = clone_of(&git, base.path(), &url, "other");
            write_files(&other, &[("t1/remote", "r")]);
            seed_commit(&git, &other, "remote moved");
            git.run_ok(
                GitScope::Remote,
                Some(&other),
                &["push", "-q", "origin", "main"],
            )
            .unwrap();

            pull_into(&git, &root, &url).unwrap();

            assert!(root.join("t1").join("remote").is_file());
            assert!(root.join("t1").join("local").is_file());
            assert_eq!(
                log_subjects(&git, &root)[..2],
                ["local, unpushed".to_owned(), "remote moved".to_owned()]
            );
        }

        /// Regression: git sets `GIT_DIR` / `GIT_INDEX_FILE` while running
        /// hooks, and those override `git -C`. Inherited, they made pull and
        /// push act on the *calling* repository (a pre-commit hook running
        /// these tests committed into the freminal checkout). A decoy
        /// repository named by them must stay untouched.
        #[test]
        fn inherited_repository_variables_are_ignored() {
            let base = tempfile::tempdir().unwrap();
            let clean = hermetic();
            let decoy = base.path().join("decoy");
            fs::create_dir_all(&decoy).unwrap();
            clean
                .run_ok(GitScope::Local, Some(&decoy), &["init", "-q", "-b", "main"])
                .unwrap();
            write_files(&decoy, &[("keep", "k")]);
            seed_commit(&clean, &decoy, "decoy");
            let decoy_head = || {
                clean
                    .run_ok(GitScope::Local, Some(&decoy), &["rev-parse", "HEAD"])
                    .unwrap()
            };
            let before = decoy_head();

            let decoy_git = decoy.join(".git");
            let poisoned = hermetic()
                .with_env("GIT_DIR", decoy_git.to_str().unwrap())
                .with_env("GIT_INDEX_FILE", decoy_git.join("index").to_str().unwrap())
                .with_env("GIT_WORK_TREE", decoy.to_str().unwrap());
            let url = seeded_remote(&clean, base.path(), "remote");
            let root = base.path().join("corpus");
            pull_into(&poisoned, &root, &url).unwrap();
            write_files(&root, &[("t1/new", "n")]);
            assert_eq!(
                publish(&poisoned, &root),
                PushOutcome::Published { attempts: 1 }
            );

            assert_eq!(decoy_head(), before, "the decoy repository was written to");
            assert!(root.join(".git").is_dir());
            assert_eq!(log_subjects(&clean, &root).len(), 2);
        }

        #[test]
        fn pushing_an_unchanged_corpus_publishes_nothing() {
            let base = tempfile::tempdir().unwrap();
            let git = hermetic();
            let url = seeded_remote(&git, base.path(), "remote");
            let root = base.path().join("corpus");
            pull_into(&git, &root, &url).unwrap();

            assert_eq!(publish(&git, &root), PushOutcome::NothingToPublish);
            assert_eq!(
                git.run_ok(
                    GitScope::Local,
                    Some(&root),
                    &["rev-list", "--count", "HEAD"]
                )
                .unwrap(),
                "1"
            );
        }

        #[test]
        fn publish_commit_uses_the_fallback_identity_and_the_message() {
            let base = tempfile::tempdir().unwrap();
            let git = hermetic();
            let url = seeded_remote(&git, base.path(), "remote");
            let root = base.path().join("corpus");
            pull_into(&git, &root, &url).unwrap();
            write_files(&root, &[("t1/new", "n")]);

            let message = CommitMessage::publish(&stamp(), &[("t1".to_owned(), 2)]);
            assert_eq!(
                commit_and_push(&git, &root, &message).unwrap(),
                PushOutcome::Published { attempts: 1 }
            );

            let show = git
                .run_ok(
                    GitScope::Local,
                    Some(&root),
                    &["log", "-1", "--format=%an <%ae>%n%s%n%b"],
                )
                .unwrap();
            assert_eq!(
                show,
                "freminal-fuzz <freminal-fuzz@users.noreply.github.com>\n\
                 corpus: box 2026-10-18T12:00:00Z\n\
                 t1: 2 files"
            );
            // Nothing was written to the repository's configuration.
            let config = git
                .run(
                    GitScope::Local,
                    Some(&root),
                    &["config", "--local", "--get", "user.email"],
                )
                .unwrap();
            assert!(!config.success());
        }

        #[test]
        fn rejected_push_is_rebased_and_retried() {
            let base = tempfile::tempdir().unwrap();
            let git = hermetic();
            let url = seeded_remote(&git, base.path(), "remote");
            let first = clone_of(&git, base.path(), &url, "first");
            let second = clone_of(&git, base.path(), &url, "second");

            write_files(&first, &[("t1/from_first", "1")]);
            assert_eq!(
                publish(&git, &first),
                PushOutcome::Published { attempts: 1 }
            );

            // `second` still has the old tip, so its first push is rejected.
            write_files(&second, &[("t1/from_second", "2")]);
            assert_eq!(
                publish(&git, &second),
                PushOutcome::Published { attempts: 2 }
            );

            let verify = clone_of(&git, base.path(), &url, "verify");
            assert!(verify.join("t1").join("from_first").is_file());
            assert!(verify.join("t1").join("from_second").is_file());
        }

        #[test]
        fn push_gives_up_on_a_non_rejection_failure() {
            let base = tempfile::tempdir().unwrap();
            let git = hermetic();
            let url = seeded_remote(&git, base.path(), "remote");
            let root = base.path().join("corpus");
            pull_into(&git, &root, &url).unwrap();
            // Break the remote so the push fails for a reason other than a
            // rejection.
            git.run_ok(
                GitScope::Local,
                Some(&root),
                &["remote", "set-url", "origin", "/no/such/remote.git"],
            )
            .unwrap();
            write_files(&root, &[("t1/new", "n")]);

            let err =
                commit_and_push(&git, &root, &CommitMessage::publish(&stamp(), &[])).unwrap_err();

            assert!(format!("{err:#}").contains("after 1 attempt"), "{err:#}");
        }

        #[test]
        fn update_refuses_a_checkout_on_another_branch() {
            let base = tempfile::tempdir().unwrap();
            let git = hermetic();
            let url = seeded_remote(&git, base.path(), "remote");
            let root = base.path().join("corpus");
            pull_into(&git, &root, &url).unwrap();
            git.run_ok(
                GitScope::Local,
                Some(&root),
                &["checkout", "-q", "-b", "wip"],
            )
            .unwrap();

            let err = pull_into(&git, &root, &url).unwrap_err();

            assert!(format!("{err:#}").contains("`wip`"), "{err:#}");
        }

        #[test]
        fn target_counts_skip_missing_directories() {
            let base = tempfile::tempdir().unwrap();
            write_files(base.path(), &[("t1/a", "a"), ("t1/b", "b")]);
            assert_eq!(
                target_file_counts(base.path(), &targets()).unwrap(),
                vec![("t1".to_owned(), 2)]
            );
        }

        /// The inline helper must hand git the token from the environment
        /// that git's shell sees, with the token nowhere in the arguments.
        #[test]
        fn credential_helper_reads_the_token_from_the_environment() {
            let token = "s3cret-token-value";
            let args = RemoteAuth::Token.config_args();
            assert!(args.iter().all(|arg| !arg.contains(token)));

            let output = cmd("git", args.iter().copied().chain(["credential", "fill"]))
                .env(TOKEN_ENV, token)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_TERMINAL_PROMPT", "0")
                .stdin_bytes("protocol=https\nhost=example.invalid\n\n")
                .read()
                .unwrap();

            assert!(output.contains("username=x-access-token"), "{output}");
            assert!(output.contains(&format!("password={token}")), "{output}");
        }
    }
}
