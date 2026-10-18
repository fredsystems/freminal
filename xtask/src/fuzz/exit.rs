// Copyright (C) 2024-2026 Fred Clausen and the ratatui project contributors
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Running a fuzz process and judging how it ended: libFuzzer's exit codes,
//! [`classify_exit`], output streaming ([`run_streaming`]) and the
//! [`run_fuzz_binary`] entry point.

use std::{
    ffi::{OsStr, OsString},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::ExitStatus,
};

use color_eyre::{
    eyre::{bail, ensure, eyre, Context},
    Result,
};
use duct::cmd;

use super::{
    sandbox::{sandboxed_argv, Sandbox},
    CrashPolicy,
};

/// libFuzzer's `TimeoutExitCode` (`FuzzerOptions.h`): a child hit `-timeout`.
///
/// Passed explicitly as `-timeout_exitcode` so the mapping never depends on
/// libFuzzer's default.
pub(super) const LIBFUZZER_TIMEOUT_EXIT: i32 = 70;
/// libFuzzer's `OOMExitCode` (`FuzzerOptions.h`): a child exceeded
/// `-rss_limit_mb` or `-malloc_limit_mb`. Not configurable by a flag.
const LIBFUZZER_OOM_EXIT: i32 = 71;
/// libFuzzer's `InterruptExitCode` (`FuzzerOptions.h`): the process was
/// interrupted (SIGINT). That is an operator action, not a finding.
const LIBFUZZER_INTERRUPT_EXIT: i32 = 72;
/// libFuzzer's `ErrorExitCode` (`FuzzerOptions.h`): a child crashed or
/// libFuzzer reported a bug. Passed explicitly as `-error_exitcode`, and
/// also the code `AddressSanitizer` is told to exit with (see [`asan_options`]).
pub(super) const LIBFUZZER_ERROR_EXIT: i32 = 77;

/// The line libFuzzer prints before it runs each file given as an individual
/// input (`RunIndividualFiles` in `FuzzerDriver.cpp`). It is the only place the
/// failing file of a replay is ever named.
const RUNNING_PREFIX: &str = "Running: ";

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

/// The path in a libFuzzer `Running: <path>` line, if `line` is one.
fn running_input(line: &str) -> Option<&str> {
    line.strip_prefix(RUNNING_PREFIX).map(str::trim_end)
}

/// Where a finding from a fuzz process can be looked for afterwards.
#[derive(Debug, Clone, Copy)]
pub(super) enum FindingsLocation<'a> {
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
///
/// EOF on the output pipe arrives only once **every** holder of the pipe's
/// write end has closed it, not just the direct child. Under bubblewrap
/// (`--unshare-all` gives a private pid namespace, `--die-with-parent` tears it
/// down) and libFuzzer's fork mode (the parent reaps its workers) no stray
/// process outlives the run in practice, so the read ends when the run does.
///
/// duct's `ReaderHandle` has no blocking `wait`: reading EOF already waits for
/// the child (see its `Read` impl), after which `try_wait` is documented to
/// return the exit status.
fn run_streaming(program: &OsString, rest: &[OsString]) -> Result<ProcessEnd> {
    let reader = cmd(program.clone(), rest.iter().cloned())
        .env(
            "ASAN_OPTIONS",
            asan_options(std::env::var_os("ASAN_OPTIONS").as_deref()),
        )
        // The fuzz binary may run unsandboxed (CI) with network access; the
        // corpus token is for the git credential helper only.
        .env_remove(super::corpus_repo::TOKEN_ENV)
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
        .ok_or_else(|| {
            eyre!("duct returned no exit status after the output pipe reached EOF (a duct bug)")
        })?;
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
pub(super) fn run_fuzz_binary(
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::fs;

    #[cfg(unix)]
    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
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
