// Copyright (C) 2024-2026 Fred Clausen and the ratatui project contributors
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Corpus minimisation bookkeeping for `fuzz-cmin`: the directory plan, the
//! recovery decision for an interrupted earlier run, the decision whether a
//! merge result may replace the live corpus, and the swap itself.
//!
//! A minimisation merges `<target>` into `<target>.cmin`, then parks the
//! original as `<target>.old` while `.cmin` is renamed into place. The `.old`
//! directory is the only copy of the original for that moment, so the code
//! here never deletes one it did not park itself.

use std::{
    fs,
    path::{Path, PathBuf},
};

use color_eyre::{
    eyre::{ensure, eyre, Context},
    Result,
};

/// Directory names used while minimising one target's corpus.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct CminPlan {
    /// The live working corpus, `<root>/<target>`.
    pub(super) live: PathBuf,
    /// The freshly merged corpus, `<root>/<target>.cmin`.
    pub(super) fresh: PathBuf,
    /// Where the old corpus is parked during the swap, `<root>/<target>.old`.
    pub(super) old: PathBuf,
}

/// Plan the directory names for minimising `target` under `corpus_root`.
pub(super) fn plan_cmin(corpus_root: &Path, target: &str) -> CminPlan {
    CminPlan {
        live: corpus_root.join(target),
        fresh: corpus_root.join(format!("{target}.cmin")),
        old: corpus_root.join(format!("{target}.old")),
    }
}

/// What to do about a possibly-interrupted earlier minimisation, decided
/// before anything is merged. See [`cmin_preflight`].
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CminPreflight {
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
pub(super) enum DirPresence {
    /// The directory exists.
    Present,
    /// The directory does not exist.
    Absent,
}

impl DirPresence {
    /// Probe `path` (anything that exists counts as present).
    pub(super) fn of(path: &Path) -> Self {
        if path.exists() {
            Self::Present
        } else {
            Self::Absent
        }
    }
}

/// Decide how to start a minimisation from which directories exist.
pub(super) const fn cmin_preflight(live: DirPresence, old: DirPresence) -> CminPreflight {
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
pub(super) enum SwapDecision {
    /// Replace the live corpus with the merged one.
    Swap,
    /// The merge produced no files from a non-empty corpus. That is a failed
    /// or truncated merge, not a minimisation: keep the live corpus.
    RefuseEmptyFresh,
    /// A `.old` appeared while the merge ran; swapping would clobber it.
    RefuseStrayOld,
}

/// Decide whether the merged corpus may replace the live one.
pub(super) const fn decide_cmin_swap(
    live_files: usize,
    old: DirPresence,
    fresh_files: usize,
) -> SwapDecision {
    if matches!(old, DirPresence::Present) {
        SwapDecision::RefuseStrayOld
    } else if fresh_files == 0 && live_files > 0 {
        SwapDecision::RefuseEmptyFresh
    } else {
        SwapDecision::Swap
    }
}

/// Atomically (per rename) replace the live corpus with the merged one.
///
/// Refuses to start when `.old` exists: parking the live corpus there would
/// have to delete whatever it holds first, and an `.old` that is not ours is
/// somebody's only copy of a corpus (see [`cmin_preflight`]).
///
/// Once the merged corpus is installed the swap has succeeded, so a failure to
/// delete the parked `.old` afterwards is a warning, not an error: the
/// minimised corpus is in place and reporting failure would be wrong. The
/// leftover `.old` makes the next minimisation stop at its preflight
/// ([`CminPreflight::AbortBothExist`]) until it is deleted, which the warning
/// says.
pub(super) fn swap_cmin(plan: &CminPlan) -> Result<()> {
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
    if let Err(err) = remove_dir_if_exists(&plan.old) {
        tracing::warn!(
            "the minimised corpus is installed at {}, but the previous corpus left at {} could \
             not be deleted ({err:#}); delete {} manually, because the next `fuzz-cmin` \
             refuses to run while it exists",
            plan.live.display(),
            plan.old.display(),
            plan.old.display()
        );
    }
    Ok(())
}

/// Remove `dir` and everything in it if it exists.
pub(super) fn remove_dir_if_exists(dir: &Path) -> Result<()> {
    if dir.exists() {
        fs::remove_dir_all(dir).wrap_err_with(|| format!("failed to remove {}", dir.display()))?;
    }
    Ok(())
}

/// The number of regular files directly inside `dir`.
pub(super) fn count_files(dir: &Path) -> Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir(dir).wrap_err_with(|| format!("failed to read {}", dir.display()))? {
        if entry?.file_type()?.is_file() {
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // A directory without write permission cannot have its entries removed, so
    // deleting the parked corpus fails *after* the merged one is installed.
    // Root ignores permissions, in which case the deletion succeeds and the
    // assertions about the outcome still hold.
    #[cfg(unix)]
    #[test]
    fn swap_cmin_succeeds_when_only_the_old_dir_cannot_be_deleted() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let plan = plan_cmin(dir.path(), "t");
        let locked = plan.live.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::create_dir_all(&plan.fresh).unwrap();
        fs::write(locked.join("stuck"), b"s").unwrap();
        fs::write(plan.fresh.join("new_input"), b"n").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();

        let result = swap_cmin(&plan);

        // Restore permissions first so the temporary directory can be cleaned
        // up even if an assertion below fails.
        let parked = plan.old.join("locked");
        if parked.exists() {
            fs::set_permissions(&parked, fs::Permissions::from_mode(0o700)).unwrap();
        }
        result.unwrap();
        assert!(plan.live.join("new_input").is_file());
        assert!(!plan.fresh.exists());
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
}
