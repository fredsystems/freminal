// Copyright (C) 2024-2026 Fred Clausen and the ratatui project contributors
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Process isolation for fuzz binaries: the [`Sandbox`] choice, the
//! bubblewrap command line, and the check that read-only inputs are visible
//! inside it.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use clap::ValueEnum;
use color_eyre::{eyre::ensure, Result};

/// How a fuzz process is isolated from the host.
///
/// # Why a sandbox exists
///
/// The `terminal_state` target drives the real terminal handler end to end,
/// including kitty graphics file and shared-memory transmission. Those paths
/// read arbitrary files and **delete** files that are marked as kitty
/// temporaries under `/tmp` and `/dev/shm`. A fuzzer will happily produce such
/// a payload, so an unsandboxed local run can delete real files.
///
/// [`Sandbox::Bwrap`] runs the fuzz binary under bubblewrap with a read-only
/// root and private tmpfs mounts over `/tmp` and `/dev/shm`, so nothing outside
/// the explicitly writable directories can be modified or removed. CI runs in
/// disposable VMs (or a systemd-hardened runner unit) and passes
/// `--sandbox none`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Sandbox {
    /// Run under bubblewrap (Linux only). The default.
    #[default]
    Bwrap,
    /// Run directly on the host. Only for disposable environments.
    None,
}

/// Whether bubblewrap's private tmpfs mounts hide `path` from the sandbox.
fn hidden_by_private_tmpfs(path: &Path) -> bool {
    path.starts_with("/tmp") || path.starts_with("/dev/shm")
}

/// Fail if a read-only input would be masked by the sandbox's private tmpfs.
///
/// A path that is not itself under `/tmp` can still resolve there through a
/// symlink, so both the path as given and its canonical form are checked. When
/// the path cannot be canonicalised (it does not exist yet) only the path as
/// given is checked.
pub(super) fn ensure_inputs_visible(sandbox: Sandbox, inputs: &[&Path]) -> Result<()> {
    if sandbox != Sandbox::Bwrap {
        return Ok(());
    }
    for input in inputs {
        let canonical = fs::canonicalize(input).ok();
        let hidden = hidden_by_private_tmpfs(input)
            || canonical.as_deref().is_some_and(hidden_by_private_tmpfs);
        ensure!(
            !hidden,
            "{} lives under /tmp or /dev/shm (resolves to {}), which the bwrap sandbox replaces \
             with an empty tmpfs; move it elsewhere or pass `--sandbox none` knowingly",
            input.display(),
            canonical.as_deref().unwrap_or(input).display()
        );
    }
    Ok(())
}

/// Build the full cmdline (program first) for running `binary` with `args`.
///
/// For [`Sandbox::Bwrap`] the binary runs under a read-only root with private
/// tmpfs mounts over `/tmp` and `/dev/shm` (see [`Sandbox`] for why). The
/// `writable` directories are bound read-write **after** the tmpfs mounts so
/// they are not masked by them. All paths must be absolute.
pub(super) fn sandboxed_argv(
    sandbox: Sandbox,
    binary: &Path,
    args: &[OsString],
    writable: &[PathBuf],
) -> Result<Vec<OsString>> {
    ensure!(
        binary.is_absolute(),
        "fuzz binary path must be absolute: {}",
        binary.display()
    );
    match sandbox {
        Sandbox::None => {
            let mut cmdline = vec![binary.as_os_str().to_owned()];
            cmdline.extend(args.iter().cloned());
            Ok(cmdline)
        }
        Sandbox::Bwrap => {
            ensure!(
                cfg!(target_os = "linux"),
                "the bwrap sandbox is Linux-only (bubblewrap does not exist on this platform); \
                 pass `--sandbox none` knowingly if you accept that fuzzing can delete files"
            );
            for dir in writable {
                ensure!(
                    dir.is_absolute(),
                    "writable sandbox path must be absolute: {}",
                    dir.display()
                );
            }
            Ok(bwrap_argv(binary, args, writable))
        }
    }
}

/// The bubblewrap cmdline; see [`sandboxed_argv`].
fn bwrap_argv(binary: &Path, args: &[OsString], writable: &[PathBuf]) -> Vec<OsString> {
    let mut cmdline: Vec<OsString> = [
        "bwrap",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--tmpfs",
        "/tmp",
        "--tmpfs",
        "/dev/shm",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    for dir in writable {
        cmdline.push(OsString::from("--bind"));
        cmdline.push(dir.as_os_str().to_owned());
        cmdline.push(dir.as_os_str().to_owned());
    }
    // The host `TMPDIR` (e.g. `/tmp/nix-shell.XXXX` in a dev shell) does not
    // exist in the private tmpfs, and libFuzzer writes its merge control file
    // and fork-mode logs there.
    cmdline.extend(["--setenv", "TMPDIR", "/tmp"].iter().map(OsString::from));
    cmdline.extend(
        ["--unshare-all", "--die-with-parent", "--new-session", "--"]
            .iter()
            .map(OsString::from),
    );
    cmdline.push(binary.as_os_str().to_owned());
    cmdline.extend(args.iter().cloned());
    cmdline
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn bwrap_argv_has_exact_order_with_writable_binds_after_tmpfs() {
        let cmdline = bwrap_argv(
            Path::new("/repo/fuzz/target/t/release/x"),
            &os(&["/c", "-runs=1"]),
            &[PathBuf::from("/w1"), PathBuf::from("/w2")],
        );
        let expected = os(&[
            "bwrap",
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
            "--tmpfs",
            "/dev/shm",
            "--bind",
            "/w1",
            "/w1",
            "--bind",
            "/w2",
            "/w2",
            "--setenv",
            "TMPDIR",
            "/tmp",
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--",
            "/repo/fuzz/target/t/release/x",
            "/c",
            "-runs=1",
        ]);
        assert_eq!(cmdline, expected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sandboxed_argv_bwrap_matches_bwrap_argv_on_linux() {
        let binary = Path::new("/repo/x");
        let args = os(&["a"]);
        let writable = [PathBuf::from("/w")];
        assert_eq!(
            sandboxed_argv(Sandbox::Bwrap, binary, &args, &writable).unwrap(),
            bwrap_argv(binary, &args, &writable)
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn sandboxed_argv_bwrap_errors_off_linux() {
        let err = sandboxed_argv(Sandbox::Bwrap, Path::new("/repo/x"), &[], &[]).unwrap_err();
        assert!(err.to_string().contains("Linux-only"));
    }

    #[test]
    fn sandboxed_argv_none_is_binary_plus_args() {
        let cmdline = sandboxed_argv(
            Sandbox::None,
            Path::new("/repo/x"),
            &os(&["-runs=1"]),
            &[PathBuf::from("/w")],
        )
        .unwrap();
        assert_eq!(cmdline, os(&["/repo/x", "-runs=1"]));
    }

    #[test]
    fn sandboxed_argv_rejects_relative_paths() {
        assert!(sandboxed_argv(Sandbox::None, Path::new("x"), &[], &[]).is_err());
        #[cfg(target_os = "linux")]
        assert!(sandboxed_argv(
            Sandbox::Bwrap,
            Path::new("/x"),
            &[],
            &[PathBuf::from("relative")]
        )
        .is_err());
    }

    #[test]
    fn tmp_and_shm_inputs_are_rejected_only_under_bwrap() {
        let hidden = Path::new("/tmp/repo/fuzz/seeds/x");
        let shm = Path::new("/dev/shm/seeds");
        let fine = Path::new("/home/u/repo/fuzz/seeds/x");
        assert!(ensure_inputs_visible(Sandbox::Bwrap, &[hidden]).is_err());
        assert!(ensure_inputs_visible(Sandbox::Bwrap, &[shm]).is_err());
        assert!(ensure_inputs_visible(Sandbox::Bwrap, &[fine]).is_ok());
        assert!(ensure_inputs_visible(Sandbox::None, &[hidden]).is_ok());
        assert!(!hidden_by_private_tmpfs(Path::new("/tmpfoo")));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_symlink_into_tmp_is_rejected_under_bwrap() {
        // The link lives next to this crate (not under /tmp) but resolves to
        // a directory under /tmp, which the sandbox would mask.
        let target = tempfile::tempdir_in("/tmp").unwrap();
        let links = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        let link = links.path().join("seeds");
        std::os::unix::fs::symlink(target.path(), &link).unwrap();
        assert!(!hidden_by_private_tmpfs(&link));
        assert!(ensure_inputs_visible(Sandbox::Bwrap, &[link.as_path()]).is_err());
        assert!(ensure_inputs_visible(Sandbox::None, &[link.as_path()]).is_ok());
    }

    #[test]
    fn a_missing_path_is_checked_as_given() {
        let missing = Path::new("/definitely/not/here/seeds");
        assert!(ensure_inputs_visible(Sandbox::Bwrap, &[missing]).is_ok());
        assert!(ensure_inputs_visible(Sandbox::Bwrap, &[Path::new("/tmp/not/here")]).is_err());
    }
}
