// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

// build.rs
extern crate vergen;
use std::path::PathBuf;
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

    rebuild_terminfo()?;

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

/// Names of the terminfo entries `res/freminal.ti` defines: the primary entry
/// and its alias.  Each is installed in both the letter and the hex directory
/// layout (see [`tree_paths`]).
const TERMINFO_NAMES: [&str; 2] = ["xterm-freminal", "freminal"];

/// The four relative paths of the committed compiled tree, sorted.
///
/// ncurses on Linux stores an entry under a directory named for its first
/// character (`x/xterm-freminal`); Apple's ncurses uses the hex of that
/// character (`78/xterm-freminal`).  Shipping both removes the need to know
/// which layout a given ncurses reads.  The paths are explicit rather than
/// discovered by walking a directory, because `tic` links the alias to the
/// primary entry and a walk that follows links would see it twice.
fn tree_paths() -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut paths = Vec::new();
    for name in TERMINFO_NAMES {
        let first = name.bytes().next().ok_or("empty terminfo entry name")?;
        paths.push(PathBuf::from(char::from(first).to_string()).join(name));
        paths.push(PathBuf::from(format!("{first:x}")).join(name));
    }
    paths.sort();
    Ok(paths)
}

/// Compile `res/freminal.ti` into the committed `res/terminfo/` tree and pack
/// that tree into `res/terminfo.tar`.
///
/// Re-runs when `res/freminal.ti` or the committed tree / archive change.  The
/// build script only rewrites a committed file when its bytes differ, so the
/// directives settle after one extra run rather than re-running forever.
///
/// `tic -x` compiles into a temporary directory in `$OUT_DIR`; the compiled
/// bytes of each entry are then written to the four paths of [`tree_paths`]
/// (identical bytes in the letter and hex copies).  The archive is packed from
/// the committed tree using the Rust `tar` crate for portability (no GNU/BSD
/// `tar` binary required), staged in `$OUT_DIR/terminfo.tar`, and copied over
/// `res/terminfo.tar` only if the content differs.  This avoids dirtying the
/// git working tree when nothing changed.
///
/// `tic` is invoked with `-x` because user-defined (extended) capabilities
/// such as `Su`, `Smulx` and `Setulc` are otherwise dropped with an
/// "unknown capability" warning and never reach the compiled entry.
///
/// Requires `tic` (from ncurses) to be present on `$PATH`.  If `tic` is
/// missing the function prints a warning and returns `Ok(())`, keeping the
/// committed tree and archive, so that builds in environments without `tic`
/// (e.g. Windows CI) are not broken.
fn rebuild_terminfo() -> Result<(), Box<dyn std::error::Error>> {
    // Derive the workspace root from the manifest directory of this crate.
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let workspace_root = manifest_dir
        .parent()
        .ok_or("could not determine workspace root")?;

    let ti_src = workspace_root.join("res").join("freminal.ti");
    let tree_dst = workspace_root.join("res").join("terminfo");
    let tar_dst = workspace_root.join("res").join("terminfo.tar");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    let terminfo_build_dir = out_dir.join("terminfo_build");
    let tar_staging = out_dir.join("terminfo.tar");

    println!("cargo:rerun-if-changed={}", ti_src.display());
    println!("cargo:rerun-if-changed={}", tree_dst.display());
    println!("cargo:rerun-if-changed={}", tar_dst.display());

    // ── tic ──────────────────────────────────────────────────────────────────
    // Check that tic is available; if not, warn and skip.
    if which_tic().is_none() {
        println!("cargo:warning=`tic` not found on PATH; skipping terminfo recompile");
        return Ok(());
    }

    // Clean stale compiled files from a previous run so that removed or
    // renamed terminfo entries don't persist in the archive.
    if terminfo_build_dir.exists() {
        std::fs::remove_dir_all(&terminfo_build_dir)?;
    }
    std::fs::create_dir_all(&terminfo_build_dir)?;

    let tic_status = Command::new("tic")
        .args([
            "-x",
            "-o",
            terminfo_build_dir.to_str().ok_or("non-UTF-8 OUT_DIR")?,
            ti_src.to_str().ok_or("non-UTF-8 ti path")?,
        ])
        .status()?;

    if !tic_status.success() {
        return Err(format!("`tic` exited with status {tic_status}").into());
    }

    // ── committed tree ───────────────────────────────────────────────────────
    // `tic` writes either the letter layout (Linux) or the hex layout (Apple's
    // ncurses); locate each entry in whichever it produced and install the same
    // bytes at both layouts' paths.
    for name in TERMINFO_NAMES {
        let compiled = read_compiled_entry(&terminfo_build_dir, name)?;
        for rel in tree_paths()?
            .into_iter()
            .filter(|p| p.file_name().is_some_and(|f| f == name))
        {
            write_if_changed(&tree_dst.join(rel), &compiled)?;
        }
    }

    // ── tar (Rust crate) ─────────────────────────────────────────────────────
    create_deterministic_tar(&tree_dst, &tar_staging)?;

    // Only update the committed copy when the content actually changed.  This
    // prevents every build from dirtying the git working tree.
    let new_bytes = std::fs::read(&tar_staging)?;
    write_if_changed(&tar_dst, &new_bytes)?;

    Ok(())
}

/// Read the compiled bytes of entry `name` from a `tic -o` output directory,
/// looking in the letter layout first and then the hex layout.
fn read_compiled_entry(
    build_dir: &std::path::Path,
    name: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let first = name.bytes().next().ok_or("empty terminfo entry name")?;
    let letter = build_dir.join(char::from(first).to_string()).join(name);
    let hex = build_dir.join(format!("{first:x}")).join(name);
    for candidate in [letter, hex] {
        if candidate.is_file() {
            return Ok(std::fs::read(candidate)?);
        }
    }
    Err(format!("`tic` produced no compiled entry for `{name}`").into())
}

/// Write `bytes` to `path` (creating parent directories) unless the file
/// already holds exactly those bytes.  A missing or unreadable file is written.
fn write_if_changed(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    if std::fs::read(path).is_ok_and(|existing| existing == bytes) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Create a deterministic tar archive of the four [`tree_paths`] under
/// `src_dir` at `dst_path`.
///
/// Entries are sorted by relative path and written with zeroed mtime, uid,
/// gid, mode 0644, and no directory entries, so that the archive is
/// reproducible.
fn create_deterministic_tar(
    src_dir: &std::path::Path,
    dst_path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::fs;

    let file = fs::File::create(dst_path)?;
    let mut builder = tar::Builder::new(file);

    for rel_path in &tree_paths()? {
        let full_path = src_dir.join(rel_path);
        let mut header = tar::Header::new_gnu();
        let data = fs::read(&full_path)?;

        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_username("root")?;
        header.set_groupname("root")?;
        header.set_cksum();

        builder.append_data(&mut header, rel_path, data.as_slice())?;
    }

    builder.finish()?;
    Ok(())
}

/// Return `Some(())` if `tic` can be found on `$PATH`, `None` otherwise.
fn which_tic() -> Option<()> {
    Command::new("tic")
        .arg("--version")
        .output()
        .ok()
        .map(|_| ())
}
