// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

// build.rs
extern crate vergen;
use std::process::Command;
use vergen::{Build, Cargo, Emitter, Rustc, Sysinfo};

// https://github.com/sagiegurari/cargo-make
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Building....");
    println!("cargo:rerun-if-changed=build.rs");
    let build = Build::all_build();
    let cargo = Cargo::all_cargo();
    let rustc = Rustc::all_rustc();
    let si = Sysinfo::all_sysinfo();

    Emitter::default()
        .add_instructions(&build)?
        .add_instructions(&cargo)?
        .add_instructions(&rustc)?
        .add_instructions(&si)?
        .emit()?;

    // Re-run this build script whenever the git state changes (a new commit,
    // a new tag, a branch switch) so `VERGEN_GIT_DESCRIBE` never goes stale.
    // Without these directives Cargo caches the build-script output and the
    // About modal / `TERM_PROGRAM_VERSION` keep reporting whatever the git
    // state was on the very first build (often "unknown" if tags were not yet
    // present).  Resolving the paths via `git rev-parse --git-path` makes this
    // correct in linked worktrees and submodules, not just plain clones.
    emit_git_rerun_directives();

    // Resolve VERGEN_GIT_DESCRIBE.
    //
    // Override layer: if the build environment already provides a non-empty
    // `VERGEN_GIT_DESCRIBE`, honour it verbatim and skip the git shell-out.
    // This is the escape hatch for build systems that strip `.git` and run
    // git-less in a sandbox — the Nix flake passes the value in from
    // `self.shortRev` because `git describe` inside the Nix sandbox can only
    // ever produce "unknown".  Re-run if that variable changes.
    println!("cargo:rerun-if-env-changed=VERGEN_GIT_DESCRIBE");

    // Fallback layer: emit VERGEN_GIT_DESCRIBE via the `git` CLI.  This avoids
    // pulling in a heavy git-library dependency (vergen v9 requires
    // vergen-gix/vergen-git2 for git info).  The describe output looks like
    // "v0.6.0-42-gabc1234" or just "v0.6.0" when exactly on a tag.  Falls back
    // to "unknown" if `git describe` fails (e.g. shallow clone with no tags).
    let git_describe = std::env::var("VERGEN_GIT_DESCRIBE")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            Command::new("git")
                .args(["describe", "--tags", "--always", "--dirty"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map_or_else(|| "unknown".to_owned(), |s| s.trim().to_owned())
        });
    println!("cargo:rustc-env=VERGEN_GIT_DESCRIBE={git_describe}");

    Ok(())
}

/// Emit `cargo:rerun-if-changed` directives for the git ref files that govern
/// `git describe` output, so the build script re-runs when the commit or tag
/// set changes.
///
/// The relevant files are `HEAD` (current commit / branch), `packed-refs`
/// (packed tags and branches), and the loose `refs/` tree (unpacked branches
/// and tags).  Their real on-disk locations are resolved through
/// `git rev-parse --git-path`, which yields the correct paths even inside a
/// linked worktree (where `HEAD` lives in the worktree but `packed-refs` lives
/// in the common git dir) or a submodule.
///
/// Best-effort: if `git` is unavailable or any path cannot be resolved (e.g. a
/// tarball build with no `.git`), the directive for that path is skipped.  A
/// missing trigger only costs a stale `VERGEN_GIT_DESCRIBE`, never a failed
/// build.
fn emit_git_rerun_directives() {
    for relative in ["HEAD", "packed-refs", "refs"] {
        if let Some(path) = git_path(relative) {
            // `rerun-if-changed` on a non-existent path is harmless and is
            // re-evaluated each build, so unpacked-refs dirs that appear later
            // still trigger a rebuild.
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

/// Resolve a git internal path (e.g. `HEAD`, `packed-refs`, `refs`) to its real
/// on-disk location via `git rev-parse --git-path`, returning `None` if `git`
/// is unavailable, the command fails, or the output is not valid UTF-8.
fn git_path(relative: &str) -> Option<String> {
    Command::new("git")
        .args(["rev-parse", "--git-path", relative])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}
