// Copyright (C) 2026 Fred Clausen
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Bounded, type-checked reading of a shell history file (Task 148.9).
//!
//! The path handed to the history loader can come from the terminal's own
//! output (`OSC 1338 ; HISTFILE=<path>`), so it is attacker-influenced: a
//! plain `std::fs::read` on it would allocate without bound on `/dev/zero`,
//! hang the loader thread on a FIFO, and happily read any regular file the
//! user can read.  [`read_history_file`] closes the I/O half of that:
//!
//! - the file is opened non-blocking, so opening a FIFO never waits for a
//!   writer;
//! - the type check runs on the *opened handle* (`fstat`), not on the path,
//!   so there is no check-then-open race;
//! - only regular files are read -- devices, FIFOs, sockets and directories
//!   are refused;
//! - at most [`MAX_HISTORY_FILE_BYTES`] bytes are ever read, taken from the
//!   end of the file, because the newest history entries are the ones used;
//! - a symlink in the final path component can be refused outright
//!   ([`SymlinkPolicy::Refuse`]) for untrusted paths.
//!
//! Deciding *whether a path is a plausible history file at all* (the
//! basename allow-list) is the caller's concern and lives in
//! [`super::shell_history`].

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use conv2::ValueFrom;

/// Upper bound on the number of bytes read from a history file.
///
/// Generous for real histories (a million-line bash history is a few tens of
/// MiB at the extreme; the typical 10k-line file is well under 1 MiB), while
/// bounding memory for hostile input.  Only the newest entries are used --
/// [`super::shell_history::HISTORY_SEED_CAP`] is 1000 -- so when a file is
/// larger than this the *tail* is read.
pub const MAX_HISTORY_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Whether a symlink in the final path component may be followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymlinkPolicy {
    /// Follow symlinks.  For paths the user supplied through their own
    /// environment, where symlinked dotfiles are common.
    Follow,
    /// Refuse a symlink in the final path component.  For paths that came
    /// from terminal output and so are not trusted.
    Refuse,
}

/// Why a history file could not be read.
#[derive(Debug, thiserror::Error)]
pub enum HistoryReadError {
    /// The path does not exist.  Expected for a first-time shell.
    #[error("history file does not exist")]
    NotFound,
    /// The path exists but is not a regular file (device, FIFO, socket,
    /// directory, ...).
    #[error("history path is not a regular file")]
    NotRegularFile,
    /// The final path component is a symlink and the policy refuses those.
    #[error("history path is a symlink and symlinks are refused")]
    SymlinkRefused,
    /// Any other I/O failure.
    #[error("I/O error reading history file: {0}")]
    Io(#[source] std::io::Error),
}

impl HistoryReadError {
    /// Map an error from opening the file, taking the policy into account.
    fn from_open(e: std::io::Error, symlinks: SymlinkPolicy) -> Self {
        if e.kind() == std::io::ErrorKind::NotFound {
            return Self::NotFound;
        }
        if symlinks == SymlinkPolicy::Refuse && is_nofollow_symlink_error(&e) {
            return Self::SymlinkRefused;
        }
        Self::Io(e)
    }
}

/// Whether `e` is what `open(2)` with `O_NOFOLLOW` reports for a symlink.
#[cfg(unix)]
fn is_nofollow_symlink_error(e: &std::io::Error) -> bool {
    let Some(code) = e.raw_os_error() else {
        return false;
    };
    if code == libc::ELOOP {
        return true;
    }
    // The BSDs report EMLINK instead of ELOOP.
    #[cfg(any(
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    if code == libc::EMLINK {
        return true;
    }
    false
}

#[cfg(not(unix))]
const fn is_nofollow_symlink_error(_e: &std::io::Error) -> bool {
    false
}

/// Open `path` read-only without ever blocking and, for
/// [`SymlinkPolicy::Refuse`], without following a final-component symlink.
#[cfg(unix)]
fn open_history_file(path: &Path, symlinks: SymlinkPolicy) -> Result<File, HistoryReadError> {
    use std::os::unix::fs::OpenOptionsExt;

    // O_NONBLOCK: opening a FIFO for reading would otherwise wait for a
    // writer.  It has no effect on regular files, which are the only thing
    // we go on to read.
    let mut flags = libc::O_NONBLOCK;
    if symlinks == SymlinkPolicy::Refuse {
        flags |= libc::O_NOFOLLOW;
    }
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(path)
        .map_err(|e| HistoryReadError::from_open(e, symlinks))
}

/// Open `path` read-only; for [`SymlinkPolicy::Refuse`], first reject a
/// final-component symlink.
#[cfg(not(unix))]
fn open_history_file(path: &Path, symlinks: SymlinkPolicy) -> Result<File, HistoryReadError> {
    if symlinks == SymlinkPolicy::Refuse {
        let meta = std::fs::symlink_metadata(path)
            .map_err(|e| HistoryReadError::from_open(e, symlinks))?;
        if meta.file_type().is_symlink() {
            return Err(HistoryReadError::SymlinkRefused);
        }
    }
    File::open(path).map_err(|e| HistoryReadError::from_open(e, symlinks))
}

/// Read the newest [`MAX_HISTORY_FILE_BYTES`] of the history file at `path`.
///
/// If the file is larger than the cap, the tail is returned and the partial
/// first line (everything up to and including the first `\n`) is dropped, so
/// the result always starts on a line boundary.  The result never exceeds
/// [`MAX_HISTORY_FILE_BYTES`].
///
/// # Errors
///
/// Returns [`HistoryReadError::NotFound`] if the path does not exist,
/// [`HistoryReadError::NotRegularFile`] if it is not a regular file,
/// [`HistoryReadError::SymlinkRefused`] if `symlinks` is
/// [`SymlinkPolicy::Refuse`] and the final component is a symlink, and
/// [`HistoryReadError::Io`] for any other I/O failure.
pub fn read_history_file(
    path: &Path,
    symlinks: SymlinkPolicy,
) -> Result<Vec<u8>, HistoryReadError> {
    let mut file = open_history_file(path, symlinks)?;
    // Type-check the opened handle, not the path: no TOCTOU window.
    let meta = file.metadata().map_err(HistoryReadError::Io)?;
    if !meta.is_file() {
        return Err(HistoryReadError::NotRegularFile);
    }
    read_tail(&mut file, meta.len())
}

/// Read at most [`MAX_HISTORY_FILE_BYTES`] from the end of `file`, whose
/// length was observed as `len`.
fn read_tail(file: &mut File, len: u64) -> Result<Vec<u8>, HistoryReadError> {
    if len <= MAX_HISTORY_FILE_BYTES {
        let mut buf = Vec::with_capacity(usize::value_from(len).unwrap_or(0));
        // `take` bounds the read even if the file grows after the fstat.
        file.take(MAX_HISTORY_FILE_BYTES)
            .read_to_end(&mut buf)
            .map_err(HistoryReadError::Io)?;
        return Ok(buf);
    }

    // Read one byte more than the cap, starting one byte early, so we can
    // tell whether the window begins exactly on a line boundary: after
    // dropping through the first `\n` of this window, what remains is at
    // most `MAX_HISTORY_FILE_BYTES` and begins on a line start either way.
    let window = MAX_HISTORY_FILE_BYTES + 1;
    file.seek(SeekFrom::Start(len - window))
        .map_err(HistoryReadError::Io)?;
    let mut buf = Vec::with_capacity(usize::value_from(window).unwrap_or(0));
    file.take(window)
        .read_to_end(&mut buf)
        .map_err(HistoryReadError::Io)?;
    match buf.iter().position(|&b| b == b'\n') {
        Some(newline) => {
            buf.drain(..=newline);
        }
        // A single line longer than the whole window: nothing usable.
        None => buf.clear(),
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::TempDir::new().expect("tempdir")
    }

    /// Run `f` on a thread and fail (rather than hang) if it does not finish.
    #[cfg(unix)]
    fn within_timeout<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        use std::sync::mpsc;
        use std::time::Duration;

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("history read did not return within 5s (blocked?)")
    }

    #[test]
    fn small_regular_file_reads_fully() {
        let dir = tmp();
        let p = dir.path().join("h");
        std::fs::write(&p, b"ls\necho hi\n").expect("write");
        let got = read_history_file(&p, SymlinkPolicy::Refuse).expect("read");
        assert_eq!(got, b"ls\necho hi\n");
    }

    #[test]
    fn oversize_file_returns_newest_tail_on_a_line_boundary() {
        let dir = tmp();
        let p = dir.path().join("h");
        let mut content = String::new();
        let mut n: u32 = 0;
        // ~9 MiB of lines, comfortably above the 8 MiB cap.
        while u64::value_from(content.len()).expect("len") < MAX_HISTORY_FILE_BYTES + 1024 * 1024 {
            use std::fmt::Write as _;
            writeln!(content, "cmd {n}").expect("write to String");
            n += 1;
        }
        std::fs::write(&p, &content).expect("write");

        let got = read_history_file(&p, SymlinkPolicy::Refuse).expect("read");
        assert!(
            u64::value_from(got.len()).expect("len") <= MAX_HISTORY_FILE_BYTES,
            "read {} bytes, cap is {MAX_HISTORY_FILE_BYTES}",
            got.len()
        );
        let text = String::from_utf8(got).expect("utf8");
        assert!(
            text.starts_with("cmd "),
            "must start on a line boundary, got {:?}",
            text.chars().take(20).collect::<String>()
        );
        let last = n - 1;
        assert!(text.ends_with(&format!("cmd {last}\n")));
        // Every line is a complete `cmd N` line.
        assert!(text.lines().all(|l| l.starts_with("cmd ")
            && l[4..].chars().all(|c| c.is_ascii_digit())
            && l.len() > 4));
    }

    #[test]
    fn file_exactly_at_cap_is_read_whole() {
        let dir = tmp();
        let p = dir.path().join("h");
        let line = b"x\n";
        let count = usize::value_from(MAX_HISTORY_FILE_BYTES).expect("cap") / line.len();
        let content = line.repeat(count);
        std::fs::write(&p, &content).expect("write");
        let got = read_history_file(&p, SymlinkPolicy::Refuse).expect("read");
        assert_eq!(got.len(), content.len());
    }

    #[test]
    fn oversize_window_starting_on_a_line_start_loses_only_one_line() {
        let dir = tmp();
        let p = dir.path().join("h");
        // 2-byte lines: the window start (len - cap - 1) lands on the
        // newline that ends the preceding line, so only that '\n' is dropped.
        let line = b"x\n";
        let count = usize::value_from(MAX_HISTORY_FILE_BYTES).expect("cap") / line.len() + 10;
        let content = line.repeat(count);
        std::fs::write(&p, &content).expect("write");
        let got = read_history_file(&p, SymlinkPolicy::Refuse).expect("read");
        assert!(u64::value_from(got.len()).expect("len") <= MAX_HISTORY_FILE_BYTES);
        assert!(got.starts_with(b"x\n"));
        assert!(got.ends_with(b"x\n"));
    }

    #[test]
    fn missing_file_is_not_found() {
        let dir = tmp();
        let err = read_history_file(&dir.path().join("nope"), SymlinkPolicy::Follow)
            .expect_err("must fail");
        assert!(matches!(err, HistoryReadError::NotFound), "got {err:?}");
        let err = read_history_file(&dir.path().join("nope"), SymlinkPolicy::Refuse)
            .expect_err("must fail");
        assert!(matches!(err, HistoryReadError::NotFound), "got {err:?}");
    }

    #[test]
    fn directory_is_not_a_regular_file() {
        let dir = tmp();
        let err = read_history_file(dir.path(), SymlinkPolicy::Follow).expect_err("must fail");
        assert!(
            matches!(
                err,
                HistoryReadError::NotRegularFile | HistoryReadError::Io(_)
            ),
            "got {err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dev_zero_is_not_a_regular_file_and_returns_immediately() {
        let err = within_timeout(|| {
            read_history_file(Path::new("/dev/zero"), SymlinkPolicy::Follow).expect_err("fail")
        });
        assert!(
            matches!(err, HistoryReadError::NotRegularFile),
            "got {err:?}"
        );
        let err = within_timeout(|| {
            read_history_file(Path::new("/dev/zero"), SymlinkPolicy::Refuse).expect_err("fail")
        });
        assert!(
            matches!(err, HistoryReadError::NotRegularFile),
            "got {err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_not_a_regular_file_and_does_not_block() {
        let dir = tmp();
        let fifo = dir.path().join("fifo");
        match std::process::Command::new("mkfifo").arg(&fifo).status() {
            Ok(s) if s.success() => {}
            _ => return, // mkfifo unavailable; nothing to test.
        }
        let err = within_timeout(move || {
            read_history_file(&fifo, SymlinkPolicy::Follow).expect_err("must fail")
        });
        assert!(
            matches!(err, HistoryReadError::NotRegularFile),
            "got {err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_refused_under_refuse_and_followed_under_follow() {
        let dir = tmp();
        let target = dir.path().join("target");
        std::fs::write(&target, b"secret-ish\n").expect("write");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        let err = read_history_file(&link, SymlinkPolicy::Refuse).expect_err("must fail");
        assert!(
            matches!(err, HistoryReadError::SymlinkRefused),
            "got {err:?}"
        );

        let got = read_history_file(&link, SymlinkPolicy::Follow).expect("follow");
        assert_eq!(got, b"secret-ish\n");
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_refused_under_refuse() {
        let dir = tmp();
        let link = dir.path().join("dangling");
        std::os::unix::fs::symlink(dir.path().join("gone"), &link).expect("symlink");
        let err = read_history_file(&link, SymlinkPolicy::Refuse).expect_err("must fail");
        assert!(
            matches!(err, HistoryReadError::SymlinkRefused),
            "got {err:?}"
        );
    }
}
