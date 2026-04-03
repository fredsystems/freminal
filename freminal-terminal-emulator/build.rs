// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

// build.rs
extern crate vergen;
use std::path::PathBuf;
use std::process::Command;
use vergen::{BuildBuilder, CargoBuilder, Emitter, RustcBuilder, SysinfoBuilder};

// https://github.com/sagiegurari/cargo-make
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Building....");
    println!("cargo:rerun-if-changed=build.rs");
    let build = BuildBuilder::all_build()?;
    let cargo = CargoBuilder::all_cargo()?;
    let rustc = RustcBuilder::all_rustc()?;
    let si = SysinfoBuilder::all_sysinfo()?;

    Emitter::default()
        .add_instructions(&build)?
        .add_instructions(&cargo)?
        .add_instructions(&rustc)?
        .add_instructions(&si)?
        .emit()?;

    rebuild_terminfo()?;

    Ok(())
}

/// Compile `res/freminal.ti` into `res/terminfo.tar` using `tic` and the Rust
/// `tar` crate.
///
/// Only re-runs when `res/freminal.ti` changes (targeted `rerun-if-changed`
/// directive).  The compiled binary terminfo tree is produced in a temporary
/// directory inside `$OUT_DIR`, then archived using the Rust `tar` crate for
/// portability (no GNU/BSD `tar` binary required).
///
/// The archive is first written to `$OUT_DIR/terminfo.tar`.  If the content
/// differs from the committed `res/terminfo.tar`, the committed copy is
/// updated.  This avoids dirtying the git working tree when nothing changed.
///
/// Requires `tic` (from ncurses) to be present on `$PATH`.  If `tic` is
/// missing the function prints a warning and returns `Ok(())` so that builds
/// in environments without `tic` (e.g. Windows CI) are not broken.
fn rebuild_terminfo() -> Result<(), Box<dyn std::error::Error>> {
    // Derive the workspace root from the manifest directory of this crate.
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let workspace_root = manifest_dir
        .parent()
        .ok_or("could not determine workspace root")?;

    let ti_src = workspace_root.join("res").join("freminal.ti");
    let tar_dst = workspace_root.join("res").join("terminfo.tar");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    let terminfo_build_dir = out_dir.join("terminfo_build");
    let tar_staging = out_dir.join("terminfo.tar");

    // Tell Cargo to re-run this build script only when the source file changes.
    println!("cargo:rerun-if-changed={}", ti_src.display());

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
            "-o",
            terminfo_build_dir.to_str().ok_or("non-UTF-8 OUT_DIR")?,
            ti_src.to_str().ok_or("non-UTF-8 ti path")?,
        ])
        .status()?;

    if !tic_status.success() {
        return Err(format!("`tic` exited with status {tic_status}").into());
    }

    // ── tar (Rust crate) ─────────────────────────────────────────────────────
    // Build the archive using the Rust `tar` crate for full portability — no
    // dependency on GNU or BSD `tar` binary.  Entries are collected, sorted by
    // path, and written with zeroed timestamps / ownership so the archive is
    // deterministic (identical content ⇒ identical bytes).
    create_deterministic_tar(&terminfo_build_dir, &tar_staging)?;

    // Only update the committed copy when the content actually changed.  This
    // prevents every build from dirtying the git working tree.
    let new_bytes = std::fs::read(&tar_staging)?;
    let update_needed = match std::fs::read(&tar_dst) {
        Ok(existing) => existing != new_bytes,
        Err(_) => true, // file missing or unreadable — always write
    };

    if update_needed {
        std::fs::copy(&tar_staging, &tar_dst)?;
    }

    Ok(())
}

/// Create a deterministic tar archive of `src_dir` at `dst_path`.
///
/// All entries are sorted by relative path and written with zeroed mtime,
/// uid, gid, and username/groupname so that the archive is reproducible.
fn create_deterministic_tar(
    src_dir: &std::path::Path,
    dst_path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::fs;

    // Collect all files under src_dir with their relative paths.
    let mut entries: Vec<PathBuf> = Vec::new();
    collect_files(src_dir, src_dir, &mut entries)?;
    entries.sort();

    let file = fs::File::create(dst_path)?;
    let mut builder = tar::Builder::new(file);

    for rel_path in &entries {
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

/// Recursively collect all file paths under `base` relative to `root`.
fn collect_files(
    root: &std::path::Path,
    current: &std::path::Path,
    out: &mut Vec<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| format!("strip_prefix failed: {e}"))?;
            out.push(rel.to_path_buf());
        }
    }
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
