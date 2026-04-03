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

/// Compile `res/freminal.ti` into `res/terminfo.tar` using `tic` and `tar`.
///
/// Only re-runs when `res/freminal.ti` changes (targeted `rerun-if-changed`
/// directive).  The compiled binary terminfo tree is produced in a temporary
/// directory inside `$OUT_DIR` and then archived into the workspace-root
/// `res/terminfo.tar` so it is committed alongside the source.
///
/// Requires `tic` (from ncurses) and `tar` to be present on `$PATH`.  If
/// either is missing the function prints a warning and returns `Ok(())` so
/// that builds in environments without `tic` (e.g. Windows CI) are not broken.
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

    // Tell Cargo to re-run this build script only when the source file changes.
    println!("cargo:rerun-if-changed={}", ti_src.display());

    // ── tic ──────────────────────────────────────────────────────────────────
    // Check that tic is available; if not, warn and skip.
    if which_tic().is_none() {
        println!("cargo:warning=`tic` not found on PATH; skipping terminfo recompile");
        return Ok(());
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

    // ── tar ──────────────────────────────────────────────────────────────────
    // Use --mtime and --sort=name for a deterministic archive so the committed
    // res/terminfo.tar does not change on every build-script invocation.
    let tar_status = Command::new("tar")
        .args([
            "--sort=name",
            "--mtime=1970-01-01 00:00:00 UTC",
            "--owner=0",
            "--group=0",
            "--numeric-owner",
            "-czf",
            tar_dst.to_str().ok_or("non-UTF-8 tar path")?,
            "-C",
            terminfo_build_dir.to_str().ok_or("non-UTF-8 OUT_DIR")?,
            ".",
        ])
        .status()?;

    if !tar_status.success() {
        return Err(format!("`tar` exited with status {tar_status}").into());
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
