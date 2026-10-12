//! Content-addressed temp files for OSC 99 icons (macOS and Windows).
//!
//! The macOS and Windows notification APIs take an icon as a file path, so an
//! icon transmitted as bytes has to be written to disk first. The file must
//! outlive the `show()` call: Windows reads the image when the toast is
//! rendered, which can be after `show()` returns. Deleting it right after
//! `show()` (what the old per-notification code did) is therefore wrong.
//!
//! Files are written once per distinct content into
//! `std::env::temp_dir()/freminal-osc99-icons/`, named by a hash of the bytes,
//! so a program that sends the same icon with every notification costs one
//! file. At most [`MAX_ICON_TEMP_FILES`] are kept (the least recently used is
//! deleted first), and the owner removes the rest at shutdown with
//! [`IconTempFiles::remove_all`].
//!
//! Only files this store wrote are ever deleted: another running freminal
//! shares the directory, and its files are not ours to touch.

use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// The most icon files kept on disk. The least recently used is deleted when
/// a new one would exceed this.
pub const MAX_ICON_TEMP_FILES: usize = 64;

/// The directory name, under the system temp directory, the icons live in.
const ICON_DIR_NAME: &str = "freminal-osc99-icons";

/// The directory icons are written to by default:
/// `std::env::temp_dir()/freminal-osc99-icons`.
#[must_use]
pub fn default_icon_dir() -> PathBuf {
    std::env::temp_dir().join(ICON_DIR_NAME)
}

/// The file extension for `bytes`, from the image magic number: `png`, `jpg`
/// or `gif`, else `img`.
fn icon_extension(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "jpg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "gif"
    } else {
        "img"
    }
}

/// The file name for `bytes`: the hex of a 64-bit content hash plus the
/// sniffed extension.
///
/// The hash is `DefaultHasher`'s, which is unkeyed (`DefaultHasher::new()` is
/// always the same function), so one icon gets one name for the life of the
/// process. Names need not be stable across Rust versions: files do not
/// outlive the process that wrote them.
fn icon_file_name(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    format!("{:016x}.{}", hasher.finish(), icon_extension(bytes))
}

/// The icon files one process has written, least recently used first.
#[derive(Debug)]
pub struct IconTempFiles {
    /// The directory the files are written to.
    dir: PathBuf,
    /// The files written (or reused) so far, least recently used first.
    order: VecDeque<PathBuf>,
}

impl IconTempFiles {
    /// A store that writes into `dir`. The directory is created on first use.
    #[must_use]
    pub const fn with_dir(dir: PathBuf) -> Self {
        Self {
            dir,
            order: VecDeque::new(),
        }
    }

    /// The path of the file holding `bytes`, writing it if it is not already
    /// there, and marking it the most recently used.
    ///
    /// Identical bytes give the identical path and one file. Once more than
    /// [`MAX_ICON_TEMP_FILES`] files are tracked, the least recently used
    /// ones are deleted from disk; the file returned is never one of them.
    ///
    /// # Errors
    ///
    /// Returns the I/O error if the directory cannot be created or the file
    /// cannot be written. A partly written file is removed.
    pub fn path_for(&mut self, bytes: &[u8]) -> io::Result<PathBuf> {
        let path = self.dir.join(icon_file_name(bytes));

        if let Some(pos) = self.order.iter().position(|known| *known == path) {
            // Ours already, and written by this process. If something outside
            // has deleted it since, writing it again is the repair.
            if path.is_file() {
                self.order.remove(pos);
                self.order.push_back(path.clone());
                return Ok(path);
            }
            self.order.remove(pos);
        }

        // The directory may have been removed by another instance's shutdown
        // since the last call, so it is ensured every time.
        std::fs::create_dir_all(&self.dir)?;
        write_exclusive(&path, bytes)?;

        self.order.push_back(path.clone());
        while self.order.len() > MAX_ICON_TEMP_FILES {
            if let Some(oldest) = self.order.pop_front() {
                remove_file_best_effort(&oldest);
            }
        }
        Ok(path)
    }

    /// Delete every file this store wrote, then the directory if it is empty
    /// (it is not when another freminal instance still has files in it).
    pub fn remove_all(&mut self) {
        for path in self.order.drain(..) {
            remove_file_best_effort(&path);
        }
        if let Err(e) = std::fs::remove_dir(&self.dir)
            && e.kind() != io::ErrorKind::NotFound
        {
            tracing::debug!(
                "OSC 99 icon directory {} not removed: {e}",
                self.dir.display()
            );
        }
    }
}

/// Write `bytes` to a new file at `path`.
///
/// The file is created with `create_new` (`O_CREAT | O_EXCL`), which refuses
/// to follow a symlink planted at a guessable name in a shared temp directory
/// and refuses to truncate an existing file. A file already at `path` that is
/// not ours (a stale one from a crashed run, or a hash collision) is removed
/// and the create retried once, so the content on disk is always `bytes`.
fn write_exclusive(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let open = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
    };
    let mut file = match open() {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(path)?;
            open()?
        }
        other => other?,
    };
    if let Err(e) = file.write_all(bytes) {
        drop(file);
        remove_file_best_effort(path);
        return Err(e);
    }
    Ok(())
}

/// Remove `path`, logging (at `debug`) any failure other than "already gone".
fn remove_file_best_effort(path: &Path) {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != io::ErrorKind::NotFound
    {
        tracing::debug!(
            "failed to remove OSC 99 icon temp file {}: {e}",
            path.display()
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest-of-the-png";

    fn store() -> (tempfile::TempDir, IconTempFiles) {
        let root = tempfile::tempdir().unwrap();
        let store = IconTempFiles::with_dir(root.path().join("icons"));
        (root, store)
    }

    fn files_in(dir: &Path) -> usize {
        std::fs::read_dir(dir).unwrap().count()
    }

    #[test]
    fn default_dir_is_under_the_system_temp_dir() {
        assert_eq!(
            default_icon_dir(),
            std::env::temp_dir().join("freminal-osc99-icons")
        );
    }

    #[test]
    fn extension_is_sniffed_from_the_magic_number() {
        assert_eq!(icon_extension(PNG), "png");
        assert_eq!(icon_extension(b"\xff\xd8\xff\xe0jfif"), "jpg");
        assert_eq!(icon_extension(b"GIF89a...."), "gif");
        assert_eq!(icon_extension(b"GIF87a...."), "gif");
        assert_eq!(icon_extension(b"not an image"), "img");
        assert_eq!(icon_extension(b""), "img");
    }

    #[test]
    fn file_name_is_sixteen_hex_digits_and_the_extension() {
        let name = icon_file_name(PNG);
        let (stem, ext) = name.split_once('.').unwrap();
        assert_eq!(ext, "png");
        assert_eq!(stem.len(), 16);
        assert!(stem.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(name, icon_file_name(PNG), "the name is stable");
    }

    #[test]
    fn same_bytes_give_the_same_path_and_one_file() {
        let (root, mut store) = store();

        let first = store.path_for(PNG).unwrap();
        let second = store.path_for(PNG).unwrap();

        assert_eq!(first, second);
        assert_eq!(std::fs::read(&first).unwrap(), PNG);
        assert_eq!(files_in(&root.path().join("icons")), 1);
        assert_eq!(first.extension().unwrap(), "png");
    }

    #[test]
    fn different_bytes_give_different_paths() {
        let (root, mut store) = store();

        let a = store.path_for(b"aaaa").unwrap();
        let b = store.path_for(b"bbbb").unwrap();

        assert_ne!(a, b);
        assert_eq!(std::fs::read(&a).unwrap(), b"aaaa");
        assert_eq!(std::fs::read(&b).unwrap(), b"bbbb");
        assert_eq!(files_in(&root.path().join("icons")), 2);
    }

    #[test]
    fn a_stale_file_at_the_name_is_replaced_not_followed() {
        let (root, mut store) = store();
        let dir = root.path().join("icons");
        std::fs::create_dir_all(&dir).unwrap();
        // A leftover (or colliding) file at exactly the name we will use.
        std::fs::write(dir.join(icon_file_name(b"fresh")), b"stale").unwrap();

        let path = store.path_for(b"fresh").unwrap();

        assert_eq!(std::fs::read(path).unwrap(), b"fresh");
    }

    #[test]
    fn write_exclusive_refuses_to_truncate_an_existing_path_directly() {
        // `create_new` semantics the store relies on: opening an existing
        // file exclusively fails instead of following or truncating it.
        let (_root, mut store) = store();
        let path = store.path_for(b"keep").unwrap();

        let err = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(path).unwrap(), b"keep");
    }

    #[test]
    fn a_deleted_file_is_written_again() {
        let (_root, mut store) = store();
        let path = store.path_for(b"gone").unwrap();
        std::fs::remove_file(&path).unwrap();

        let again = store.path_for(b"gone").unwrap();

        assert_eq!(path, again);
        assert_eq!(std::fs::read(again).unwrap(), b"gone");
    }

    #[test]
    fn exceeding_the_cap_deletes_the_oldest_file_from_disk() {
        let (root, mut store) = store();
        let dir = root.path().join("icons");
        let paths: Vec<PathBuf> = (0..=MAX_ICON_TEMP_FILES)
            .map(|n| store.path_for(format!("icon-{n}").as_bytes()).unwrap())
            .collect();

        assert_eq!(files_in(&dir), MAX_ICON_TEMP_FILES);
        assert!(!paths[0].exists(), "the oldest was evicted");
        assert!(paths[1].exists());
        assert!(paths[MAX_ICON_TEMP_FILES].exists());
    }

    #[test]
    fn reuse_marks_a_file_most_recent_so_it_survives_eviction() {
        let (_root, mut store) = store();
        let first = store.path_for(b"first").unwrap();
        let second = store.path_for(b"second").unwrap();
        for n in 0..MAX_ICON_TEMP_FILES - 2 {
            let _ = store.path_for(format!("filler-{n}").as_bytes()).unwrap();
        }
        // The store is exactly full. Touch `first`, then add one more: the
        // victim is `second`, not `first`.
        assert_eq!(store.path_for(b"first").unwrap(), first);
        let _ = store.path_for(b"one-too-many").unwrap();

        assert!(first.exists(), "a reused file is recent");
        assert!(!second.exists(), "the least recently used file went");
    }

    #[test]
    fn remove_all_deletes_the_files_and_the_directory() {
        let (root, mut store) = store();
        let dir = root.path().join("icons");
        let a = store.path_for(b"a").unwrap();
        let b = store.path_for(b"b").unwrap();

        store.remove_all();

        assert!(!a.exists());
        assert!(!b.exists());
        assert!(!dir.exists(), "the empty directory is removed too");
    }

    #[test]
    fn remove_all_leaves_files_it_did_not_write_and_so_the_directory() {
        let (root, mut store) = store();
        let dir = root.path().join("icons");
        let ours = store.path_for(b"ours").unwrap();
        let theirs = dir.join("theirs.png");
        std::fs::write(&theirs, b"another instance").unwrap();

        store.remove_all();

        assert!(!ours.exists());
        assert!(theirs.exists(), "not ours to delete");
        assert!(dir.exists(), "a non-empty directory stays");
    }

    #[test]
    fn remove_all_on_an_unused_store_is_harmless() {
        let (_root, mut store) = store();
        store.remove_all();
    }
}
