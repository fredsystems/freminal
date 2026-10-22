//! Content-addressed temp files for OSC 99 icons (macOS and Windows).
//!
//! The macOS and Windows notification APIs take an icon as a file path, so an
//! icon has to be written to disk first. The file must outlive the `show()`
//! call: Windows reads the image when the toast is rendered, which can be
//! after `show()` returns. Deleting it right after `show()` (what the old
//! per-notification code did) is therefore wrong.
//!
//! # Where the files go
//!
//! Each process gets its own directory,
//! `std::env::temp_dir()/freminal-osc99-icons-<pid>` ([`default_icon_dir`]),
//! so two freminal processes never share, delete or collide on each other's
//! files. The directory is created with `create_dir` (never `create_dir_all`,
//! which would follow an existing path), and:
//!
//! - **Unix:** with mode `0700`, and its files with `0600`. A directory that
//!   already exists is used only if it is a real directory (not a symlink)
//!   with no group or other permission bits **and** owned by the current user;
//!   anything else is refused. The current user's uid is read from a probe
//!   file created (and deleted) in the parent temp directory, then compared
//!   with the directory's ([`owned_by`]); this needs no unsafe code and no
//!   `libc`. An existing safe directory is one a crashed process with the same
//!   pid left behind, and the store empties it on first use.
//! - **Windows:** the default ACL of the user's temp directory applies, which
//!   is already private to the user. An existing path that is not a real
//!   directory is refused.
//!
//! A directory that cannot be made safe means no icon; the notification is
//! still shown.
//!
//! # Lifetime
//!
//! Files are written once per distinct content, named by a hash of the bytes,
//! so a program that sends the same icon with every notification costs one
//! file. A file is deleted when it has not been used for [`MAX_ICON_AGE`]
//! (ten minutes), or when more than [`MAX_ICON_TEMP_FILES`] are kept (the
//! least recently used goes first). Reusing a file counts as using it. Age is
//! checked whenever a file is requested, by the store's own timestamps, never
//! from the file system's.
//!
//! The owner removes the rest at shutdown with [`IconTempFiles::remove_all`].
//!
//! **Limitation.** The OS may show a notification long after it was posted: the
//! Windows Action Center and the macOS Notification Center keep a delivered
//! notification until the user clears it, and read its image file again when
//! they redraw it. Once the file is deleted (by age, by the cap, or at exit)
//! that redraw shows no image. The notification itself is unaffected. Keeping
//! files forever to avoid this would leak disk space for every distinct icon a
//! program sends, which is the worse trade.
//!
//! # Stale directories
//!
//! A process that is killed cannot remove its directory, so on first use the
//! store also sweeps its siblings ([`sweep_stale_dirs`]): a
//! `freminal-osc99-icons-*` directory in the same parent that passes the same
//! checks as above (a real private directory owned by the current user) and
//! has not been modified for [`STALE_DIR_AGE`] (a day) has its files deleted
//! and is removed. Symlinks are never followed: a symlink in a directory is
//! removed as a link, and a symlink at a sibling's name is not a real
//! directory, so it is left alone.
//!
//! Apart from that sweep and the first-use clean-up of a stale directory left
//! by an earlier process with the same pid, only files this store wrote are
//! ever deleted.

use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// The most icon files kept on disk. The least recently used is deleted when
/// a new one would exceed this.
pub const MAX_ICON_TEMP_FILES: usize = 64;

/// How long a file may go unused before it is deleted.
pub const MAX_ICON_AGE: Duration = Duration::from_mins(10);

/// How long a sibling icon directory may go unmodified before the first-use
/// sweep ([`sweep_stale_dirs`]) removes it.
pub const STALE_DIR_AGE: Duration = Duration::from_hours(24);

/// The start of every icon directory's name; the process id follows.
const ICON_DIR_PREFIX: &str = "freminal-osc99-icons-";

/// The directory the icons of process `pid` live in, under `base`:
/// `<base>/freminal-osc99-icons-<pid>`.
fn icon_dir_in(base: &Path, pid: u32) -> PathBuf {
    base.join(format!("{ICON_DIR_PREFIX}{pid}"))
}

/// The directory icons are written to by default, one per process:
/// `std::env::temp_dir()/freminal-osc99-icons-<pid>`.
#[must_use]
pub fn default_icon_dir() -> PathBuf {
    icon_dir_in(&std::env::temp_dir(), std::process::id())
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

/// One file the store wrote, and when it was last asked for.
#[derive(Debug)]
struct Entry {
    path: PathBuf,
    last_used: Instant,
}

/// Whether the first-use sweep of stale sibling directories has run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SweepState {
    /// Not yet: the next successful [`IconTempFiles::ensure_dir`] runs it.
    Pending,
    /// Done; it never runs again for this store.
    Done,
}

/// The icon files one process has written, least recently used first.
#[derive(Debug)]
pub struct IconTempFiles {
    /// The directory the files are written to.
    dir: PathBuf,
    /// The files written (or reused) so far, least recently used first. The
    /// timestamps are non-decreasing front to back, so age eviction only ever
    /// looks at the front.
    order: VecDeque<Entry>,
    /// Whether stale sibling directories have been swept yet.
    sweep: SweepState,
}

impl IconTempFiles {
    /// A store that writes into `dir`. The directory is created on first use.
    #[must_use]
    pub const fn with_dir(dir: PathBuf) -> Self {
        Self {
            dir,
            order: VecDeque::new(),
            sweep: SweepState::Pending,
        }
    }

    /// The path of the file holding `bytes`, writing it if it is not already
    /// there, and marking it the most recently used.
    ///
    /// Identical bytes give the identical path and one file. Files unused for
    /// [`MAX_ICON_AGE`], and the least recently used ones past
    /// [`MAX_ICON_TEMP_FILES`], are deleted from disk; the file returned is
    /// never one of them.
    ///
    /// # Errors
    ///
    /// Returns the I/O error if the directory cannot be created or is not
    /// safe to use (see the module docs), or the file cannot be written. A
    /// partly written file is removed.
    pub fn path_for(&mut self, bytes: &[u8]) -> io::Result<PathBuf> {
        self.path_for_at(bytes, Instant::now())
    }

    /// [`Self::path_for`] with the current time given, so tests drive the age
    /// limit without sleeping. `now` must not be earlier than a previous call's.
    fn path_for_at(&mut self, bytes: &[u8], now: Instant) -> io::Result<PathBuf> {
        let path = self.dir.join(icon_file_name(bytes));

        if let Some(pos) = self.order.iter().position(|known| known.path == path) {
            self.order.remove(pos);
            // Ours already, and written by this process. If something outside
            // has deleted it since, writing it again is the repair.
            if path.is_file() {
                self.order.push_back(Entry {
                    path: path.clone(),
                    last_used: now,
                });
                self.evict(now);
                return Ok(path);
            }
        }

        // The directory may have been removed by something outside since the
        // last call (a temp cleaner), so it is ensured every time.
        self.ensure_dir()?;
        write_exclusive(&path, bytes)?;

        self.order.push_back(Entry {
            path: path.clone(),
            last_used: now,
        });
        self.evict(now);
        Ok(path)
    }

    /// Delete the files that have gone unused for more than [`MAX_ICON_AGE`]
    /// at `now`, then the least recently used ones beyond the cap.
    fn evict(&mut self, now: Instant) {
        while let Some(oldest) = self.order.front() {
            let aged = now.saturating_duration_since(oldest.last_used) > MAX_ICON_AGE;
            if !aged && self.order.len() <= MAX_ICON_TEMP_FILES {
                break;
            }
            if let Some(oldest) = self.order.pop_front() {
                remove_file_best_effort(&oldest.path);
            }
        }
    }

    /// Make sure the private directory exists.
    ///
    /// A directory that is already there is accepted only if
    /// [`verify_existing_dir`] approves it. When the store has written nothing
    /// yet, such a directory is a stale one from an earlier process with the
    /// same pid, and its files are deleted. The first time a directory is
    /// secured, stale sibling directories are swept ([`sweep_stale_dirs`]).
    fn ensure_dir(&mut self) -> io::Result<()> {
        match create_private_dir(&self.dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                verify_existing_dir(&self.dir)?;
                if self.order.is_empty() {
                    clear_dir_files(&self.dir);
                }
            }
            Err(e) => return Err(e),
        }
        if self.sweep == SweepState::Pending {
            self.sweep = SweepState::Done;
            sweep_stale_dirs(&self.dir, SystemTime::now(), STALE_DIR_AGE);
        }
        Ok(())
    }

    /// Delete every file this store wrote, then the directory if it is empty.
    pub fn remove_all(&mut self) {
        for entry in self.order.drain(..) {
            remove_file_best_effort(&entry.path);
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

/// Create `dir` (its parent must exist) with mode `0700`.
#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new().mode(0o700).create(dir)
}

/// Create `dir` (its parent must exist); the user's default ACLs apply.
#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new().create(dir)
}

/// Check that an already existing `dir` is safe to put icons in.
///
/// It must be a real directory: `symlink_metadata` does not follow a final
/// symlink, so a symlink planted at the name is refused. On Unix it must also
/// have no group or other permission bits, so nobody else can add, replace or
/// read files in it.
fn verify_private_dir(dir: &Path) -> io::Result<()> {
    let unsafe_dir = |why: &str| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("icon directory {} is not safe to use: {why}", dir.display()),
        )
    };
    let metadata = std::fs::symlink_metadata(dir)?;
    if !metadata.file_type().is_dir() {
        return Err(unsafe_dir("it is not a real directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(unsafe_dir("it is accessible to other users"));
        }
    }
    Ok(())
}

/// Who owns a directory, relative to the current user.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ownership {
    /// The directory belongs to the current user.
    Ours,
    /// Someone else owns it; it is not ours to put icons in or delete.
    Foreign,
}

/// Whether a directory owned by `dir_uid` is owned by the user `our_uid`.
#[cfg(unix)]
const fn owned_by(dir_uid: u32, our_uid: u32) -> Ownership {
    if dir_uid == our_uid {
        Ownership::Ours
    } else {
        Ownership::Foreign
    }
}

/// The current user's uid, found without unsafe code or `libc`: create a
/// private probe file in `parent`, read the owner the file system gave it,
/// and delete it.
#[cfg(unix)]
fn current_uid(parent: &Path) -> io::Result<u32> {
    use std::os::unix::fs::MetadataExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static PROBES: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    let probe = parent.join(format!(
        "freminal-osc99-probe-{}-{}-{nanos}",
        std::process::id(),
        PROBES.fetch_add(1, Ordering::Relaxed)
    ));
    // `create_new`: a name someone else made is an error, never reused.
    let file = create_new_file(&probe)?;
    let uid = file.metadata().map(|metadata| metadata.uid());
    drop(file);
    remove_file_best_effort(&probe);
    uid
}

/// The process's uid, probed once (see [`current_uid`]) and then remembered:
/// it cannot change while the process runs, so later checks of an existing
/// directory cost no extra file round trip.
#[cfg(unix)]
fn process_uid(parent: &Path) -> io::Result<u32> {
    static UID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    if let Some(uid) = UID.get() {
        return Ok(*uid);
    }
    let uid = current_uid(parent)?;
    Ok(*UID.get_or_init(|| uid))
}

/// Check that the existing directory `dir` belongs to the user `our_uid`.
/// `symlink_metadata` does not follow a final symlink.
#[cfg(unix)]
fn verify_owned_dir(dir: &Path, our_uid: u32) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::symlink_metadata(dir)?;
    match owned_by(metadata.uid(), our_uid) {
        Ownership::Ours => Ok(()),
        Ownership::Foreign => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "icon directory {} is not safe to use: it is owned by another user",
                dir.display()
            ),
        )),
    }
}

/// [`verify_private_dir`], and on Unix also that the directory is owned by the
/// current user (see [`current_uid`]).
fn verify_existing_dir(dir: &Path) -> io::Result<()> {
    verify_private_dir(dir)?;
    #[cfg(unix)]
    {
        let parent = dir.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("icon directory {} has no parent directory", dir.display()),
            )
        })?;
        verify_owned_dir(dir, process_uid(parent)?)?;
    }
    Ok(())
}

/// Remove the icon directories next to `own` that an earlier process left.
///
/// A sibling is removed only if its name starts with [`ICON_DIR_PREFIX`], it
/// is not `own`, it passes [`verify_private_dir`] (so a symlink or a directory
/// open to others is left alone) and, on Unix, is owned by the current user,
/// and its modification time is more than `max_age` before `now`. Its
/// non-directory entries are deleted (a symlink as a link, never followed),
/// then the directory itself; one that still holds a subdirectory stays.
/// Failures are logged at debug and skipped: the sweep is best effort and
/// never affects the store.
fn sweep_stale_dirs(own: &Path, now: SystemTime, max_age: Duration) {
    let Some(parent) = own.parent() else {
        return;
    };
    #[cfg(unix)]
    let our_uid = match process_uid(parent) {
        Ok(uid) => uid,
        Err(e) => {
            tracing::debug!("OSC 99 stale icon directories not swept: {e}");
            return;
        }
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_sibling = path != own
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(ICON_DIR_PREFIX));
        if !is_sibling || verify_private_dir(&path).is_err() {
            continue;
        }
        #[cfg(unix)]
        if verify_owned_dir(&path, our_uid).is_err() {
            continue;
        }
        let stale = std::fs::symlink_metadata(&path)
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|age| age > max_age));
        if !stale {
            continue;
        }
        clear_dir_files(&path);
        if let Err(e) = std::fs::remove_dir(&path) {
            tracing::debug!(
                "stale OSC 99 icon directory {} not removed: {e}",
                path.display()
            );
        }
    }
}

/// Delete the non-directory entries of `dir`, which [`verify_private_dir`]
/// has approved. A symlink is removed as a link; what it points at is not
/// touched.
fn clear_dir_files(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| !kind.is_dir()) {
            remove_file_best_effort(&entry.path());
        }
    }
}

/// Create a new file at `path`, mode `0600` on Unix, failing if it exists.
fn create_new_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

/// Write `bytes` to a new file at `path`.
///
/// The file is created with `create_new` (`O_CREAT | O_EXCL`), which refuses
/// to follow a symlink planted at a guessable name and refuses to truncate an
/// existing file. A file already at `path` that is not ours (a stale one from a
/// crashed run, a symlink, or a hash collision) is removed (a symlink is
/// removed as a link, never followed) and the create retried once, so the
/// content on disk is always `bytes`.
fn write_exclusive(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = match create_new_file(path) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(path)?;
            create_new_file(path)?
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

    /// `base` plus `minutes` minutes: a clock the tests move by hand.
    fn at(base: Instant, minutes: u64) -> Instant {
        base + Duration::from_secs(minutes * 60)
    }

    #[test]
    fn the_directory_is_per_process_under_the_system_temp_dir() {
        assert_eq!(
            default_icon_dir(),
            std::env::temp_dir().join(format!("freminal-osc99-icons-{}", std::process::id()))
        );
        assert_eq!(
            icon_dir_in(Path::new("base"), 4242),
            Path::new("base").join("freminal-osc99-icons-4242")
        );
        assert_ne!(
            icon_dir_in(Path::new("base"), 1),
            icon_dir_in(Path::new("base"), 2)
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
    fn a_stale_file_in_an_existing_safe_directory_is_cleared_on_first_use() {
        let (root, mut store) = store();
        let dir = root.path().join("icons");
        // What a crashed earlier process with our pid left: a safe directory
        // holding a file at exactly the name we will use, and another.
        create_private_dir(&dir).unwrap();
        std::fs::write(dir.join(icon_file_name(b"fresh")), b"stale").unwrap();
        std::fs::write(dir.join("other.png"), b"stale too").unwrap();

        let path = store.path_for(b"fresh").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"fresh");
        assert_eq!(files_in(&dir), 1, "the other stale file is gone");

        // Only the first use clears: what the store itself wrote stays.
        let second = store.path_for(b"second").unwrap();
        assert!(path.exists());
        assert!(second.exists());
    }

    #[test]
    fn write_exclusive_creates_a_new_file_with_the_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("new.png");

        write_exclusive(&path, b"fresh bytes").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"fresh bytes");
    }

    #[test]
    fn write_exclusive_replaces_an_existing_file_rather_than_appending() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("old.png");
        std::fs::write(&path, b"a much longer stale file").unwrap();

        write_exclusive(&path, b"new").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn create_new_file_refuses_an_existing_path() {
        // The `create_new` (`O_EXCL`) semantics `write_exclusive` builds on.
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("keep.png");
        std::fs::write(&path, b"keep").unwrap();

        let err = create_new_file(&path).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn write_exclusive_does_not_follow_a_symlink_at_the_target() {
        let root = tempfile::tempdir().unwrap();
        let victim = root.path().join("victim.txt");
        std::fs::write(&victim, b"precious").unwrap();
        let path = root.path().join("icon.png");
        std::os::unix::fs::symlink(&victim, &path).unwrap();

        write_exclusive(&path, b"icon bytes").unwrap();

        assert_eq!(
            std::fs::read(&victim).unwrap(),
            b"precious",
            "the link's target is untouched"
        );
        assert!(
            !std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link was replaced by a regular file"
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"icon bytes");
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

    // ── age eviction, with an injected clock ────────────────────────────

    #[test]
    fn a_file_unused_for_over_ten_minutes_is_deleted_on_the_next_insert() {
        let (_root, mut store) = store();
        let t0 = Instant::now();
        let old = store.path_for_at(b"old", t0).unwrap();
        let recent = store.path_for_at(b"recent", at(t0, 9)).unwrap();
        assert!(old.exists() && recent.exists());

        let newest = store.path_for_at(b"newest", at(t0, 11)).unwrap();

        assert!(!old.exists(), "11 minutes unused");
        assert!(recent.exists(), "2 minutes unused");
        assert!(newest.exists());
    }

    #[test]
    fn exactly_ten_minutes_is_not_yet_too_old() {
        let (_root, mut store) = store();
        let t0 = Instant::now();
        let first = store.path_for_at(b"first", t0).unwrap();

        let second = store.path_for_at(b"second", at(t0, 10)).unwrap();
        assert!(first.exists() && second.exists());

        let third = store
            .path_for_at(b"third", at(t0, 10) + Duration::from_secs(1))
            .unwrap();
        assert!(!first.exists());
        assert!(second.exists() && third.exists());
    }

    #[test]
    fn reusing_a_file_resets_its_age() {
        let (_root, mut store) = store();
        let t0 = Instant::now();
        let icon = store.path_for_at(b"icon", t0).unwrap();

        assert_eq!(store.path_for_at(b"icon", at(t0, 9)).unwrap(), icon);
        let other = store.path_for_at(b"other", at(t0, 18)).unwrap();

        assert!(icon.exists(), "used 9 minutes ago, not 18");
        assert!(other.exists());
    }

    #[test]
    fn the_file_being_returned_is_never_aged_out() {
        let (_root, mut store) = store();
        let t0 = Instant::now();
        let icon = store.path_for_at(b"icon", t0).unwrap();

        // An hour later the same icon is asked for again: it is returned,
        // and it exists.
        let again = store.path_for_at(b"icon", at(t0, 60)).unwrap();

        assert_eq!(icon, again);
        assert!(again.exists());
    }

    #[test]
    fn age_and_the_cap_both_apply() {
        let (root, mut store) = store();
        let dir = root.path().join("icons");
        let t0 = Instant::now();
        for n in 0..MAX_ICON_TEMP_FILES {
            let _ = store
                .path_for_at(format!("icon-{n}").as_bytes(), t0)
                .unwrap();
        }
        assert_eq!(files_in(&dir), MAX_ICON_TEMP_FILES);

        // One new file much later: every old one is aged out at once.
        let fresh = store.path_for_at(b"fresh", at(t0, 30)).unwrap();

        assert_eq!(files_in(&dir), 1);
        assert!(fresh.exists());
    }

    // ── two stores ──────────────────────────────────────────────────────

    #[test]
    fn two_stores_with_the_same_content_in_different_directories_do_not_interfere() {
        let root = tempfile::tempdir().unwrap();
        let mut first = IconTempFiles::with_dir(icon_dir_in(root.path(), 1));
        let mut second = IconTempFiles::with_dir(icon_dir_in(root.path(), 2));

        let in_first = first.path_for(PNG).unwrap();
        let in_second = second.path_for(PNG).unwrap();

        assert_ne!(in_first, in_second);
        assert_eq!(std::fs::read(&in_first).unwrap(), PNG);
        assert_eq!(std::fs::read(&in_second).unwrap(), PNG);

        // One process exiting touches only its own directory.
        first.remove_all();
        assert!(!in_first.exists());
        assert!(!icon_dir_in(root.path(), 1).exists());
        assert!(in_second.exists(), "the other store's file is untouched");
        assert_eq!(second.path_for(PNG).unwrap(), in_second);
    }

    // ── directory safety ────────────────────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn the_directory_is_created_0700_and_files_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let (root, mut store) = store();

        let file = store.path_for(PNG).unwrap();

        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&root.path().join("icons")), 0o700);
        assert_eq!(mode(&file), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_directory_open_to_others_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;
        let (root, mut store) = store();
        let dir = root.path().join("icons");
        std::fs::create_dir(&dir).unwrap();

        for mode in [0o755, 0o750, 0o705, 0o777] {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode)).unwrap();
            let err = store.path_for(PNG).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{mode:o}");
            assert_eq!(files_in(&dir), 0, "nothing written at {mode:o}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_directory_name_is_refused_and_not_written_through() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = root.path().join("elsewhere");
        create_private_dir(&elsewhere).unwrap();
        let dir = root.path().join("icons");
        std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();
        let mut store = IconTempFiles::with_dir(dir);

        let err = store.path_for(PNG).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(files_in(&elsewhere), 0, "nothing written through the link");
    }

    #[test]
    fn a_regular_file_at_the_directory_name_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("icons");
        std::fs::write(&dir, b"not a directory").unwrap();
        let mut store = IconTempFiles::with_dir(dir.clone());

        let err = store.path_for(PNG).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read(&dir).unwrap(), b"not a directory");
    }

    #[test]
    fn a_missing_parent_directory_is_an_error_not_a_recursive_create() {
        let root = tempfile::tempdir().unwrap();
        let mut store = IconTempFiles::with_dir(root.path().join("no").join("such").join("icons"));

        assert!(store.path_for(PNG).is_err());
        assert!(!root.path().join("no").exists());
    }

    // ── ownership ───────────────────────────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn owned_by_compares_the_uids() {
        assert_eq!(owned_by(1000, 1000), Ownership::Ours);
        assert_eq!(owned_by(0, 1000), Ownership::Foreign);
        assert_eq!(owned_by(1000, 0), Ownership::Foreign);
    }

    #[cfg(unix)]
    #[test]
    fn the_probe_reads_our_uid_and_leaves_nothing_behind() {
        use std::os::unix::fs::MetadataExt as _;
        let root = tempfile::tempdir().unwrap();
        let ours = std::fs::metadata(root.path()).unwrap().uid();

        assert_eq!(current_uid(root.path()).unwrap(), ours);
        assert_eq!(
            current_uid(root.path()).unwrap(),
            ours,
            "names do not clash"
        );
        assert_eq!(files_in(root.path()), 0, "the probe file is deleted");
    }

    #[cfg(unix)]
    #[test]
    fn the_probe_fails_in_a_directory_that_does_not_exist() {
        let root = tempfile::tempdir().unwrap();

        assert!(current_uid(&root.path().join("missing")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_we_created_passes_the_ownership_check() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("icons");
        create_private_dir(&dir).unwrap();

        verify_existing_dir(&dir).unwrap();
        verify_owned_dir(&dir, current_uid(root.path()).unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_owned_by_another_uid_is_refused() {
        // A foreign-owned directory cannot be made without privilege, so the
        // check is driven with a uid that is not ours.
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("icons");
        create_private_dir(&dir).unwrap();
        let ours = current_uid(root.path()).unwrap();

        let err = verify_owned_dir(&dir, ours.wrapping_add(1)).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[cfg(unix)]
    #[test]
    fn a_store_uses_an_existing_directory_it_owns() {
        let (root, mut store) = store();
        create_private_dir(&root.path().join("icons")).unwrap();

        assert!(store.path_for(PNG).unwrap().exists());
    }

    // ── stale directory sweep ───────────────────────────────────────────

    /// A private icon directory named for process `pid` under `root`, holding
    /// one file.
    fn sibling(root: &Path, pid: u32) -> PathBuf {
        let dir = icon_dir_in(root, pid);
        create_private_dir(&dir).unwrap();
        std::fs::write(dir.join("a.png"), b"icon").unwrap();
        dir
    }

    /// Set `dir`'s modification time to `hours` hours before now.
    #[cfg(unix)]
    fn age(dir: &Path, hours: u64) {
        let when = SystemTime::now() - Duration::from_secs(hours * 3600);
        std::fs::File::open(dir)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    #[test]
    fn the_sweep_uses_the_injected_clock_and_threshold() {
        let root = tempfile::tempdir().unwrap();
        let own = icon_dir_in(root.path(), 1);
        let other = sibling(root.path(), 2);
        let now = SystemTime::now();

        sweep_stale_dirs(&own, now, STALE_DIR_AGE);
        assert!(other.exists(), "a directory made just now is not stale");

        sweep_stale_dirs(&own, now + Duration::from_hours(23), STALE_DIR_AGE);
        assert!(other.exists(), "23 hours is under the day");

        sweep_stale_dirs(&own, now + Duration::from_hours(25), STALE_DIR_AGE);
        assert!(!other.exists(), "25 hours is stale");
    }

    #[test]
    fn the_sweep_keeps_a_directory_a_shorter_threshold_would_remove() {
        let root = tempfile::tempdir().unwrap();
        let own = icon_dir_in(root.path(), 1);
        let other = sibling(root.path(), 2);
        let later = SystemTime::now() + Duration::from_hours(2);

        sweep_stale_dirs(&own, later, Duration::from_hours(3));
        assert!(other.exists());

        sweep_stale_dirs(&own, later, Duration::from_hours(1));
        assert!(!other.exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_sweep_removes_an_old_private_sibling_and_keeps_the_rest() {
        let root = tempfile::tempdir().unwrap();
        let own = sibling(root.path(), 1);
        let old = sibling(root.path(), 2);
        let fresh = sibling(root.path(), 3);
        let unrelated = root.path().join("not-ours");
        std::fs::create_dir(&unrelated).unwrap();
        for dir in [&own, &old, &unrelated] {
            age(dir, 48);
        }

        sweep_stale_dirs(&own, SystemTime::now(), STALE_DIR_AGE);

        assert!(!old.exists(), "old and private: removed with its files");
        assert!(fresh.exists(), "fresh: kept");
        assert!(
            own.join("a.png").exists(),
            "our own directory is never swept"
        );
        assert!(unrelated.exists(), "a name without the prefix is not ours");
    }

    #[cfg(unix)]
    #[test]
    fn the_sweep_keeps_an_old_sibling_open_to_others() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let own = icon_dir_in(root.path(), 1);
        let open = sibling(root.path(), 2);
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        age(&open, 48);

        sweep_stale_dirs(&own, SystemTime::now(), STALE_DIR_AGE);

        assert!(open.join("a.png").exists(), "not private: left alone");
    }

    #[cfg(unix)]
    #[test]
    fn the_sweep_leaves_a_symlink_sibling_and_its_target_untouched() {
        let root = tempfile::tempdir().unwrap();
        let own = icon_dir_in(root.path(), 1);
        // The target is private and old, but its own name is not an icon
        // directory's, so only the link could lead the sweep to it.
        let target = root.path().join("target");
        create_private_dir(&target).unwrap();
        std::fs::write(target.join("precious.txt"), b"keep").unwrap();
        age(&target, 48);
        let link = icon_dir_in(root.path(), 2);
        std::os::unix::fs::symlink(&target, &link).unwrap();

        sweep_stale_dirs(&own, SystemTime::now(), STALE_DIR_AGE);

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link is still there"
        );
        assert_eq!(std::fs::read(target.join("precious.txt")).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn the_sweep_removes_a_symlink_inside_a_stale_directory_without_following_it() {
        let root = tempfile::tempdir().unwrap();
        let own = icon_dir_in(root.path(), 1);
        let victim = root.path().join("victim.txt");
        std::fs::write(&victim, b"precious").unwrap();
        let old = sibling(root.path(), 2);
        std::os::unix::fs::symlink(&victim, old.join("link.png")).unwrap();
        age(&old, 48);

        sweep_stale_dirs(&own, SystemTime::now(), STALE_DIR_AGE);

        assert!(!old.exists());
        assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
    }

    #[cfg(unix)]
    #[test]
    fn the_first_use_of_a_store_sweeps_once() {
        let root = tempfile::tempdir().unwrap();
        let mut store = IconTempFiles::with_dir(icon_dir_in(root.path(), 1));
        let old = sibling(root.path(), 2);
        age(&old, 48);

        let _ = store.path_for(b"first").unwrap();
        assert!(!old.exists(), "the stale sibling went on first use");

        let later = sibling(root.path(), 3);
        age(&later, 48);
        let _ = store.path_for(b"second").unwrap();
        assert!(later.exists(), "the sweep does not run again");
    }

    // ── removal ─────────────────────────────────────────────────────────

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
        std::fs::write(&theirs, b"someone else").unwrap();

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
