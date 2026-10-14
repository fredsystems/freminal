// Copyright (C) 2024-2026 Fred Clausen and the ratatui project contributors
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Finding artifacts: which files libFuzzer writes into `-artifact_prefix`,
//! how to tell new ones from old, and how a run's findings are summarised.

use std::{collections::BTreeSet, fs, path::Path};

use color_eyre::{eyre::Context, Result};

/// The kinds of finding artifact libFuzzer writes into `-artifact_prefix`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum FindingKind {
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
pub(super) fn finding_artifacts(dir: &Path) -> Result<BTreeSet<String>> {
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
pub(super) fn new_findings(
    before: &BTreeSet<String>,
    after: &BTreeSet<String>,
) -> Vec<FindingKind> {
    after
        .difference(before)
        .filter_map(|name| finding_kind(name))
        .collect()
}

/// One line describing `found`, the new findings of a run, in `dir`.
///
/// `existing` is how many finding files `dir` holds in total. It only matters
/// when `found` is empty: an artifact name is a content hash, so a run that
/// re-finds a known input writes no new file, and "no new findings" alone
/// would hide that the directory still holds findings.
pub(super) fn findings_summary(dir: &Path, found: &[FindingKind], existing: usize) -> String {
    if found.is_empty() {
        if existing == 0 {
            return format!("no new findings in {}", dir.display());
        }
        return format!(
            "no new findings ({existing} existing finding file(s) in {}; re-found inputs keep \
             their name)",
            dir.display()
        );
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(findings_summary(dir, &[], 0), "no new findings in /a/t");
        assert_eq!(
            findings_summary(
                dir,
                &[
                    FindingKind::Oom,
                    FindingKind::Crash,
                    FindingKind::Crash,
                    FindingKind::SlowUnit
                ],
                9
            ),
            "4 new finding(s) (crash: 2, oom: 1, slow-unit: 1) in /a/t"
        );
    }

    #[test]
    fn findings_summary_mentions_existing_files_when_nothing_is_new() {
        assert_eq!(
            findings_summary(Path::new("/a/t"), &[], 3),
            "no new findings (3 existing finding file(s) in /a/t; re-found inputs keep their \
             name)"
        );
    }
}
