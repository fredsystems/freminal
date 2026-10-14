// Copyright (C) 2024-2026 Fred Clausen and the ratatui project contributors
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! libFuzzer command-line construction for the three ways a fuzz binary is
//! run (fuzz, replay, corpus merge), plus discovery of the replay inputs.
//!
//! Everything here is a pure function of its inputs (replay discovery reads a
//! directory listing), so it is unit-tested without running a fuzzer.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use color_eyre::{eyre::Context, Result};

use super::{
    exit::{LIBFUZZER_ERROR_EXIT, LIBFUZZER_TIMEOUT_EXIT},
    CrashPolicy,
};

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

/// Inputs for [`fuzz_args`].
pub(super) struct FuzzArgsPlan<'a> {
    /// Working corpus (libFuzzer writes new inputs to the *first* directory).
    pub(super) corpus: &'a Path,
    /// Curated seeds (read-only).
    pub(super) seeds: &'a Path,
    /// Directory receiving crash artifacts.
    pub(super) artifacts: &'a Path,
    /// Wall-clock budget in seconds.
    pub(super) time_secs: u64,
    /// What to do on a finding.
    pub(super) policy: CrashPolicy,
    /// Fork-mode workers (used only for [`CrashPolicy::KeepGoing`]).
    pub(super) jobs: u32,
}

/// libFuzzer arguments for a normal fuzzing run.
pub(super) fn fuzz_args(plan: &FuzzArgsPlan<'_>) -> Vec<OsString> {
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
pub(super) fn replay_args(inputs: &[PathBuf]) -> Vec<OsString> {
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
pub(super) fn collect_replay_inputs(dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
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

/// libFuzzer arguments for a corpus merge (`-merge=1 <fresh> <corpus>`).
pub(super) fn cmin_args(fresh: &Path, corpus: &Path, artifacts: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        OsString::from("-merge=1"),
        fresh.as_os_str().to_owned(),
        corpus.as_os_str().to_owned(),
        artifact_prefix_arg(artifacts),
    ];
    args.extend(limit_args());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
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
    fn limit_args_pin_libfuzzer_exit_codes() {
        let args = limit_args();
        assert!(args.contains(&OsString::from("-timeout_exitcode=70")));
        assert!(args.contains(&OsString::from("-error_exitcode=77")));
    }
}
