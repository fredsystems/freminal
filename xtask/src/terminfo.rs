// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `cargo xtask terminfo`: regenerate the committed compiled terminfo tree
//! (`res/terminfo/`) and its archive (`res/terminfo.tar`) from `res/freminal.ti`.
//!
//! This used to run inside `freminal-terminal-emulator/build.rs`.  A build script
//! may write only to `OUT_DIR`, so generation is an explicit step instead: the
//! committed tree and tar are the single generation that the binary embeds and the
//! packages copy, and the `source_compiles_from_stdin_to_the_committed_entry` test
//! fails when `res/freminal.ti` changes without a regeneration.

use std::path::{Path, PathBuf};
use std::process::Command;

use color_eyre::{
    eyre::{bail, eyre, Context},
    Result,
};

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
fn tree_paths() -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for name in TERMINFO_NAMES {
        let first = name
            .bytes()
            .next()
            .ok_or_else(|| eyre!("empty terminfo entry name"))?;
        paths.push(PathBuf::from(char::from(first).to_string()).join(name));
        paths.push(PathBuf::from(format!("{first:x}")).join(name));
    }
    paths.sort();
    Ok(paths)
}

/// Compile `res/freminal.ti` into the committed `res/terminfo/` tree and pack
/// that tree into `res/terminfo.tar`.
///
/// The files are rewritten only when their bytes differ, so a run with nothing
/// to change leaves the git working tree untouched.
///
/// The committed bytes are whatever the host's `tic` produced, and they can
/// differ between ncurses versions.  The tests check tree/tar byte consistency, and
/// compare `infocmp -x` of the committed tree with `infocmp -x` of a fresh
/// stdin compile of `res/freminal.ti` on the same host, not a fixed byte image.
///
/// `tic -x` compiles into a temporary directory; the compiled bytes of each
/// entry are then written to the four paths of [`tree_paths`] (identical bytes
/// in the letter and hex copies).  The archive is packed from the committed tree
/// using the Rust `tar` crate for portability (no GNU/BSD `tar` binary
/// required).
///
/// `tic` is invoked with `-x` because user-defined (extended) capabilities such
/// as `Su`, `Smulx` and `Setulc` are otherwise dropped with an "unknown
/// capability" warning and never reach the compiled entry.
///
/// Requires `tic` (from ncurses) on `$PATH`; fails with a clear error otherwise.
pub fn terminfo() -> Result<()> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| eyre!("could not determine workspace root"))?;
    let ti_src = workspace_root.join("res").join("freminal.ti");
    let tree_dst = workspace_root.join("res").join("terminfo");
    let tar_dst = workspace_root.join("res").join("terminfo.tar");

    if Command::new("tic").arg("-V").output().is_err() {
        bail!(
            "`tic` (from ncurses) was not found on PATH; it is required to regenerate \
             res/terminfo/ and res/terminfo.tar (run inside `nix develop`)"
        );
    }

    let staging = tempfile::tempdir().wrap_err("creating a staging directory")?;
    let compiled_dir = staging.path().join("terminfo_build");
    std::fs::create_dir_all(&compiled_dir)?;

    let tic_status = Command::new("tic")
        .arg("-x")
        .arg("-o")
        .arg(&compiled_dir)
        .arg(&ti_src)
        .status()
        .wrap_err("running `tic`")?;
    if !tic_status.success() {
        bail!("`tic` exited with status {tic_status}");
    }

    // `tic` writes either the letter layout (Linux) or the hex layout (Apple's
    // ncurses); locate each entry in whichever it produced and install the same
    // bytes at both layouts' paths.
    for name in TERMINFO_NAMES {
        let compiled = read_compiled_entry(&compiled_dir, name)?;
        for rel in tree_paths()?
            .into_iter()
            .filter(|p| p.file_name().is_some_and(|f| f == name))
        {
            write_if_changed(&tree_dst.join(rel), &compiled)?;
        }
    }

    let tar_staging = staging.path().join("terminfo.tar");
    create_deterministic_tar(&tree_dst, &tar_staging)?;
    let new_bytes = std::fs::read(&tar_staging)?;
    write_if_changed(&tar_dst, &new_bytes)?;

    Ok(())
}

/// Read the compiled bytes of entry `name` from a `tic -o` output directory,
/// looking in the letter layout first and then the hex layout.
fn read_compiled_entry(build_dir: &Path, name: &str) -> Result<Vec<u8>> {
    let first = name
        .bytes()
        .next()
        .ok_or_else(|| eyre!("empty terminfo entry name"))?;
    let letter = build_dir.join(char::from(first).to_string()).join(name);
    let hex = build_dir.join(format!("{first:x}")).join(name);
    for candidate in [letter, hex] {
        if candidate.is_file() {
            return Ok(std::fs::read(candidate)?);
        }
    }
    bail!("`tic` produced no compiled entry for `{name}`")
}

/// Write `bytes` to `path` (creating parent directories) unless the file
/// already holds exactly those bytes.  A missing or unreadable file is written.
fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
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
fn create_deterministic_tar(src_dir: &Path, dst_path: &Path) -> Result<()> {
    let file = std::fs::File::create(dst_path)?;
    let mut builder = tar::Builder::new(file);

    for rel_path in &tree_paths()? {
        let full_path = src_dir.join(rel_path);
        let mut header = tar::Header::new_gnu();
        let data = std::fs::read(&full_path)?;

        header.set_size(u64::try_from(data.len())?);
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
