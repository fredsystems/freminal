// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The XDG icon and `.desktop` scans behind OSC 99's icon resolution
//! (Linux/BSD).
//!
//! Neither scan is a full implementation of the icon theme spec. They answer
//! two questions kitty asks:
//!
//! - does an icon called `name` exist anywhere in the icon directories, and
//!   under which on-disk name? ([`Osc99IconLookup::matched_icon`]), and
//! - which icon does the application called `f=` use?
//!   ([`Osc99IconLookup::desktop_icon`], through its `.desktop` file).
//!
//! Both scans run once, together, on a background thread started when the
//! lookup is created ([`XdgIconLookup::from_env`]), so neither the GUI thread
//! nor the notification service's dispatcher ever waits for the filesystem.
//! Until the scan has finished every question is answered "no match", which
//! makes the caller fall through to its next icon rule. The directories are
//! injected so tests use a temporary directory and scan synchronously
//! (`XdgIconLookup::with_roots_sync`).

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Read;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;

use super::presentation::Osc99IconLookup;

/// The name of the thread that runs the scans.
const SCAN_THREAD_NAME: &str = "freminal-osc99-icons";

/// The deepest a scan descends: a file `MAX_SCAN_DEPTH` path components below
/// a root is found, one further down is not. `hicolor/48x48/apps/x.png` is
/// four deep.
const MAX_SCAN_DEPTH: usize = 6;

/// The most directory entries (files and directories) one scan visits before
/// it stops, bounding the time spent on a machine with an enormous icon
/// collection or a pathological symlink farm.
const MAX_SCAN_ENTRIES: usize = 200_000;

/// The most icon names the icon scan keeps, bounding its memory on a machine
/// with an enormous icon collection.
const MAX_ICON_NAMES: usize = 100_000;

/// The most `.desktop` files the application scan reads.
const MAX_DESKTOP_FILES: usize = 20_000;

/// The most bytes read from one `.desktop` file. The keys wanted are at the
/// top of the `[Desktop Entry]` group.
const MAX_DESKTOP_FILE_BYTES: u64 = 64 * 1024;

/// The file extensions the icon scan counts as icons.
const ICON_EXTENSIONS: [&str; 3] = ["png", "svg", "xpm"];

/// The group of a `.desktop` file that holds `Name=` and `Icon=`.
const DESKTOP_ENTRY_GROUP: &str = "[Desktop Entry]";

/// What the scans found.
#[derive(Debug, Default)]
struct Scans {
    /// The on-disk stem of every icon file, by lowercased stem. The first icon
    /// to claim a key keeps it, so earlier directories take priority.
    icons: HashMap<String, String>,
    /// The `Icon=` of every `.desktop` file by lowercased desktop-file ID.
    desktop_by_id: HashMap<String, String>,
    /// The `Icon=` of every `.desktop` file by lowercased `Name=`.
    desktop_by_name: HashMap<String, String>,
}

/// The icon and application directories, scanned in the background.
pub struct XdgIconLookup {
    /// The scan's result; empty until the scan thread has finished.
    scans: Arc<OnceLock<Scans>>,
}

impl XdgIconLookup {
    /// The lookup over the standard directories of this process's
    /// environment. The scan starts immediately on its own thread.
    #[must_use]
    pub fn from_env() -> Self {
        let (icon_roots, app_roots) = roots_from_env(
            std::env::var_os("XDG_DATA_HOME"),
            std::env::var_os("XDG_DATA_DIRS"),
            std::env::var_os("HOME"),
        );
        // The thread finishes on its own; nothing ever needs to wait for it.
        let (lookup, _detached) = Self::start_scan(icon_roots, app_roots, MAX_SCAN_ENTRIES);
        lookup
    }

    /// Start scanning `icon_roots` and `app_roots` (highest priority first) on
    /// a background thread, visiting at most `max_entries` entries per scan.
    ///
    /// Returns the lookup, which answers "no match" until the scan finishes,
    /// and the scan thread's handle (`None` if the thread could not be
    /// started, in which case the lookup never matches).
    fn start_scan(
        icon_roots: Vec<PathBuf>,
        app_roots: Vec<PathBuf>,
        max_entries: usize,
    ) -> (Self, Option<JoinHandle<()>>) {
        let scans = Arc::new(OnceLock::new());
        let for_thread = Arc::clone(&scans);
        let handle = std::thread::Builder::new()
            .name(SCAN_THREAD_NAME.to_owned())
            .spawn(move || {
                for_thread.get_or_init(|| scan_all(&icon_roots, &app_roots, max_entries));
            })
            .map_err(|e| tracing::warn!("OSC 99: could not start the icon scan thread: {e}"))
            .ok();
        (Self { scans }, handle)
    }

    /// The lookup over the given directories, highest priority first, scanned
    /// before this returns.
    #[cfg(test)]
    #[must_use]
    pub fn with_roots_sync(icon_roots: &[PathBuf], app_roots: &[PathBuf]) -> Self {
        let scans = Arc::new(OnceLock::new());
        scans.get_or_init(|| scan_all(icon_roots, app_roots, MAX_SCAN_ENTRIES));
        Self { scans }
    }
}

impl Osc99IconLookup for XdgIconLookup {
    fn matched_icon(&self, name: &str) -> Option<String> {
        self.scans.get()?.icons.get(&name.to_lowercase()).cloned()
    }

    fn desktop_icon(&self, app_name: &str) -> Option<String> {
        let scans = self.scans.get()?;
        let key = app_name.to_lowercase();
        // A file's ID outranks another file's `Name=`.
        scans
            .desktop_by_id
            .get(&key)
            .or_else(|| scans.desktop_by_name.get(&key))
            .cloned()
    }
}

/// Run both scans.
fn scan_all(icon_roots: &[PathBuf], app_roots: &[PathBuf], max_entries: usize) -> Scans {
    let icons = scan_icons(icon_roots, max_entries);
    let (desktop_by_id, desktop_by_name) = scan_desktop_files(app_roots, max_entries);
    Scans {
        icons,
        desktop_by_id,
        desktop_by_name,
    }
}

/// The icon and application directories for the given environment values.
///
/// Icons: `$XDG_DATA_HOME/icons` (default `~/.local/share/icons`), each
/// `$XDG_DATA_DIRS` entry plus `/icons` (default `/usr/local/share:/usr/share`),
/// `~/.icons`, `/usr/share/pixmaps`. Applications: `$XDG_DATA_HOME/applications`
/// and each `$XDG_DATA_DIRS` entry plus `/applications`. As the XDG spec says,
/// an empty value counts as unset and a relative path is ignored.
fn roots_from_env(
    xdg_data_home: Option<OsString>,
    xdg_data_dirs: Option<OsString>,
    home: Option<OsString>,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let non_empty = |value: Option<OsString>| value.filter(|v| !v.is_empty());
    let home = non_empty(home)
        .map(PathBuf::from)
        .filter(|h| h.is_absolute());

    let data_home = non_empty(xdg_data_home)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    let data_dirs: Vec<PathBuf> = non_empty(xdg_data_dirs).map_or_else(
        || {
            vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ]
        },
        |dirs| {
            std::env::split_paths(&dirs)
                .filter(|path| path.is_absolute())
                .collect()
        },
    );

    let mut icon_roots = Vec::new();
    let mut app_roots = Vec::new();
    for base in data_home.iter().chain(data_dirs.iter()) {
        icon_roots.push(base.join("icons"));
        app_roots.push(base.join("applications"));
    }
    if let Some(home) = &home {
        icon_roots.push(home.join(".icons"));
    }
    icon_roots.push(PathBuf::from("/usr/share/pixmaps"));
    (icon_roots, app_roots)
}

/// A directory walk's state: the directories already entered and the entries
/// visited so far.
struct Walker<'a> {
    /// The canonical paths of the directories entered, so a link back up the
    /// tree cannot loop.
    entered: HashSet<PathBuf>,
    /// The entries visited so far.
    visited: usize,
    /// The most entries to visit.
    max_entries: usize,
    /// Called with the root a file was found under and the file's path.
    on_file: &'a mut dyn FnMut(&Path, &Path) -> ControlFlow<()>,
}

/// Visit every file under `roots`, bounded by [`MAX_SCAN_DEPTH`] and
/// `max_entries` visited entries.
///
/// Symbolic links are followed (a Nix profile is a tree of them), but a
/// directory is entered only once per scan, judged by its canonical path, so a
/// link back up the tree cannot loop. I/O errors are ignored: a directory that
/// cannot be read is skipped. `on_file` is given the root and the file's path;
/// it returns [`ControlFlow::Break`] to stop the whole scan. Running out of
/// `max_entries` also stops the scan, and is logged once.
fn walk_files(
    roots: &[PathBuf],
    max_entries: usize,
    on_file: &mut dyn FnMut(&Path, &Path) -> ControlFlow<()>,
) {
    let mut walker = Walker {
        entered: HashSet::new(),
        visited: 0,
        max_entries,
        on_file,
    };
    for root in roots {
        if walker.walk_dir(root, root, 1).is_break() {
            return;
        }
    }
}

impl Walker<'_> {
    /// Visit the files in `dir`, whose direct children are `depth` components
    /// below `root`, and recurse into its subdirectories.
    fn walk_dir(&mut self, root: &Path, dir: &Path, depth: usize) -> ControlFlow<()> {
        if depth > MAX_SCAN_DEPTH {
            return ControlFlow::Continue(());
        }
        let Ok(canonical) = dir.canonicalize() else {
            return ControlFlow::Continue(());
        };
        if !self.entered.insert(canonical) {
            return ControlFlow::Continue(());
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return ControlFlow::Continue(());
        };
        for entry in entries.flatten() {
            self.visited += 1;
            if self.visited > self.max_entries {
                tracing::debug!(
                    "OSC 99: the XDG scan stopped after {} entries",
                    self.max_entries
                );
                return ControlFlow::Break(());
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            // `file_type` does not follow a symbolic link; only a link needs a
            // second system call, to learn what it points at.
            let file_type = if file_type.is_symlink() {
                let Ok(target) = std::fs::metadata(&path) else {
                    continue;
                };
                target.file_type()
            } else {
                file_type
            };
            if file_type.is_dir() {
                self.walk_dir(root, &path, depth + 1)?;
            } else if file_type.is_file() {
                (self.on_file)(root, &path)?;
            }
        }
        ControlFlow::Continue(())
    }
}

/// The on-disk stems of the icon files under `roots`, by lowercased stem.
fn scan_icons(roots: &[PathBuf], max_entries: usize) -> HashMap<String, String> {
    let mut names: HashMap<String, String> = HashMap::new();
    walk_files(roots, max_entries, &mut |_root, path| {
        if has_extension(path, &ICON_EXTENSIONS)
            && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
        {
            names
                .entry(stem.to_lowercase())
                .or_insert_with(|| stem.to_owned());
            if names.len() >= MAX_ICON_NAMES {
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
    });
    names
}

/// `Icon=` by lowercased desktop-file ID and by lowercased `Name=` for the
/// `.desktop` files under `roots`. The first file to claim a key in a map
/// keeps it, so the directories earlier in `roots` take priority.
fn scan_desktop_files(
    roots: &[PathBuf],
    max_entries: usize,
) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut by_id: HashMap<String, String> = HashMap::new();
    let mut by_name: HashMap<String, String> = HashMap::new();
    let mut files_read = 0usize;
    walk_files(roots, max_entries, &mut |root, path| {
        if !has_extension(path, &["desktop"]) {
            return ControlFlow::Continue(());
        }
        files_read += 1;
        if let Some(entry) = read_desktop_entry(path) {
            if let Some(id) = desktop_file_id(root, path) {
                by_id
                    .entry(id.to_lowercase())
                    .or_insert_with(|| entry.icon.clone());
            }
            if let Some(name) = entry.name.filter(|name| !name.is_empty()) {
                by_name.entry(name.to_lowercase()).or_insert(entry.icon);
            }
        }
        if files_read >= MAX_DESKTOP_FILES {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    (by_id, by_name)
}

/// The desktop-file ID of the `.desktop` file at `path` under `root`: its path
/// relative to `root` without the extension, with the directory separators
/// replaced by `-` (the XDG desktop entry spec: `sub/x.desktop` is `sub-x`).
/// `None` for a path that is not valid UTF-8 or is not under `root`.
fn desktop_file_id(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?.with_extension("");
    let parts: Vec<&str> = relative
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<_>>()?;
    Some(parts.join("-"))
}

/// Whether `path`'s extension is one of `extensions`, ignoring case.
fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|wanted| extension.eq_ignore_ascii_case(wanted))
        })
}

/// What a `.desktop` file says about its application's icon.
#[derive(Debug, PartialEq, Eq)]
struct DesktopEntry {
    /// The unlocalised `Name=`.
    name: Option<String>,
    /// The non-empty `Icon=`.
    icon: String,
}

/// Read the first [`MAX_DESKTOP_FILE_BYTES`] of the `.desktop` file at `path`.
fn read_desktop_entry(path: &Path) -> Option<DesktopEntry> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_DESKTOP_FILE_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    parse_desktop_entry(&String::from_utf8_lossy(&bytes))
}

/// The `Name=` and `Icon=` of the `[Desktop Entry]` group of `contents`.
///
/// A leading byte order mark is ignored, and so are localised names
/// (`Name[de]=`). `None` when the group has no
/// non-empty `Icon=`: such a file cannot answer an icon question.
fn parse_desktop_entry(contents: &str) -> Option<DesktopEntry> {
    // A byte order mark ahead of `[Desktop Entry]` would hide the group header.
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    let mut in_entry_group = false;
    let mut name = None;
    let mut icon = None;
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry_group = line == DESKTOP_ENTRY_GROUP;
            continue;
        }
        if !in_entry_group {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            match key.trim_end() {
                "Name" if name.is_none() => name = Some(value.trim().to_owned()),
                "Icon" if icon.is_none() => icon = Some(value.trim().to_owned()),
                _ => {}
            }
        }
    }
    icon.filter(|icon| !icon.is_empty())
        .map(|icon| DesktopEntry { name, icon })
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;

    use super::*;

    type TestResult = Result<(), Box<dyn Error>>;

    fn touch(root: &Path, relative: &str) -> TestResult {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, b"x")?;
        Ok(())
    }

    fn lookup_over(icons: &Path, apps: &Path) -> XdgIconLookup {
        XdgIconLookup::with_roots_sync(&[icons.to_path_buf()], &[apps.to_path_buf()])
    }

    #[test]
    fn an_icon_in_a_theme_tree_exists_case_insensitively() -> TestResult {
        let dir = tempfile::tempdir()?;
        touch(dir.path(), "hicolor/48x48/apps/foo.png")?;
        touch(dir.path(), "hicolor/scalable/apps/Scal.svg")?;
        touch(dir.path(), "hicolor/48x48/apps/readme.txt")?;
        let lookup = lookup_over(dir.path(), dir.path());

        assert!(lookup.matched_icon("Foo").is_some());
        assert!(lookup.matched_icon("foo").is_some());
        assert!(lookup.matched_icon("scal").is_some());
        assert_eq!(lookup.matched_icon("bar"), None);
        assert_eq!(lookup.matched_icon("readme"), None);
        Ok(())
    }

    #[test]
    fn a_desktop_file_resolves_by_stem_and_by_name() -> TestResult {
        let dir = tempfile::tempdir()?;
        fs::write(
            dir.path().join("myapp.desktop"),
            "[Desktop Entry]\nType=Application\nName=My App\nName[de]=Meine App\nIcon=myicon\n\
             \n[Desktop Action new]\nName=New Window\nIcon=other\n",
        )?;
        let lookup = lookup_over(dir.path(), dir.path());

        assert_eq!(lookup.desktop_icon("myapp").as_deref(), Some("myicon"));
        assert_eq!(lookup.desktop_icon("MyApp").as_deref(), Some("myicon"));
        assert_eq!(lookup.desktop_icon("my app").as_deref(), Some("myicon"));
        assert_eq!(lookup.desktop_icon("Meine App"), None);
        assert_eq!(lookup.desktop_icon("New Window"), None);
        assert_eq!(lookup.desktop_icon("unknown"), None);
        Ok(())
    }

    #[test]
    fn a_desktop_file_without_an_icon_resolves_nothing() -> TestResult {
        let dir = tempfile::tempdir()?;
        fs::write(
            dir.path().join("noicon.desktop"),
            "[Desktop Entry]\nName=No Icon\n",
        )?;
        let lookup = lookup_over(dir.path(), dir.path());

        assert_eq!(lookup.desktop_icon("noicon"), None);
        assert_eq!(lookup.desktop_icon("no icon"), None);
        Ok(())
    }

    #[test]
    fn the_first_root_wins_a_desktop_entry() -> TestResult {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        fs::write(
            first.path().join("app.desktop"),
            "[Desktop Entry]\nName=App\nIcon=from-first\n",
        )?;
        fs::write(
            second.path().join("app.desktop"),
            "[Desktop Entry]\nName=App\nIcon=from-second\n",
        )?;
        let lookup = XdgIconLookup::with_roots_sync(
            &[],
            &[first.path().to_path_buf(), second.path().to_path_buf()],
        );

        assert_eq!(lookup.desktop_icon("app").as_deref(), Some("from-first"));
        Ok(())
    }

    #[test]
    fn the_scan_respects_the_depth_bound() -> TestResult {
        let dir = tempfile::tempdir()?;
        // Six components below the root: found.
        touch(dir.path(), "d1/d2/d3/d4/d5/shallow.png")?;
        // Eight components below the root: not found.
        touch(dir.path(), "d1/d2/d3/d4/d5/d6/d7/deep.png")?;
        let lookup = lookup_over(dir.path(), dir.path());

        assert!(lookup.matched_icon("shallow").is_some());
        assert_eq!(lookup.matched_icon("deep"), None);
        Ok(())
    }

    #[test]
    fn a_symlink_loop_does_not_hang_the_scan() -> TestResult {
        let dir = tempfile::tempdir()?;
        touch(dir.path(), "a/x.png")?;
        std::os::unix::fs::symlink(dir.path().join("a"), dir.path().join("a/loop"))?;
        std::os::unix::fs::symlink(dir.path(), dir.path().join("a/up"))?;
        let lookup = lookup_over(dir.path(), dir.path());

        assert!(lookup.matched_icon("x").is_some());
        Ok(())
    }

    #[test]
    fn a_symlinked_directory_and_file_are_followed() -> TestResult {
        let store = tempfile::tempdir()?;
        let profile = tempfile::tempdir()?;
        touch(store.path(), "theme/apps/linked.png")?;
        touch(store.path(), "single.png")?;
        std::os::unix::fs::symlink(store.path().join("theme"), profile.path().join("theme"))?;
        std::os::unix::fs::symlink(
            store.path().join("single.png"),
            profile.path().join("single.png"),
        )?;
        let lookup = lookup_over(profile.path(), profile.path());

        assert!(lookup.matched_icon("linked").is_some());
        assert!(lookup.matched_icon("single").is_some());
        Ok(())
    }

    #[test]
    fn missing_roots_are_ignored() {
        let lookup = XdgIconLookup::with_roots_sync(
            &[PathBuf::from("/nonexistent/freminal/icons")],
            &[PathBuf::from("/nonexistent/freminal/applications")],
        );

        assert_eq!(lookup.matched_icon("anything"), None);
        assert_eq!(lookup.desktop_icon("anything"), None);
    }

    #[test]
    fn default_roots_follow_the_xdg_defaults() {
        let (icons, apps) = roots_from_env(None, None, Some(OsString::from("/home/u")));

        assert_eq!(
            icons,
            [
                "/home/u/.local/share/icons",
                "/usr/local/share/icons",
                "/usr/share/icons",
                "/home/u/.icons",
                "/usr/share/pixmaps",
            ]
            .map(PathBuf::from)
        );
        assert_eq!(
            apps,
            [
                "/home/u/.local/share/applications",
                "/usr/local/share/applications",
                "/usr/share/applications",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn explicit_roots_replace_the_defaults_and_relative_ones_are_ignored() {
        let (icons, apps) = roots_from_env(
            Some(OsString::from("/data")),
            Some(OsString::from("/a:relative/b:/c")),
            Some(OsString::from("/home/u")),
        );

        assert_eq!(
            icons,
            [
                "/data/icons",
                "/a/icons",
                "/c/icons",
                "/home/u/.icons",
                "/usr/share/pixmaps",
            ]
            .map(PathBuf::from)
        );
        assert_eq!(
            apps,
            ["/data/applications", "/a/applications", "/c/applications"].map(PathBuf::from)
        );
    }

    #[test]
    fn empty_environment_values_count_as_unset() {
        let (icons, _) = roots_from_env(
            Some(OsString::new()),
            Some(OsString::new()),
            Some(OsString::from("/home/u")),
        );

        assert_eq!(icons[0], PathBuf::from("/home/u/.local/share/icons"));
        assert_eq!(icons[1], PathBuf::from("/usr/local/share/icons"));
    }

    #[test]
    fn parsing_ignores_other_groups_and_localised_keys() {
        let entry = parse_desktop_entry(
            "[Other]\nIcon=nope\n[Desktop Entry]\nName[fr]=Fr\nName = Real\nIcon=real\n",
        );

        assert_eq!(
            entry,
            Some(DesktopEntry {
                name: Some("Real".to_owned()),
                icon: "real".to_owned(),
            })
        );
    }

    // ── Z8: the matched on-disk stem ────────────────────────────────────

    #[test]
    fn the_matched_icon_is_the_on_disk_stem_in_its_original_case() -> TestResult {
        let dir = tempfile::tempdir()?;
        touch(dir.path(), "hicolor/48x48/apps/MixedCase.png")?;
        let lookup = lookup_over(dir.path(), dir.path());

        assert_eq!(
            lookup.matched_icon("mixedcase").as_deref(),
            Some("MixedCase")
        );
        assert_eq!(
            lookup.matched_icon("MIXEDCASE").as_deref(),
            Some("MixedCase")
        );
        Ok(())
    }

    #[test]
    fn the_first_root_wins_an_icon_stem() -> TestResult {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        touch(first.path(), "Icon.png")?;
        touch(second.path(), "icon.png")?;
        let lookup = XdgIconLookup::with_roots_sync(
            &[first.path().to_path_buf(), second.path().to_path_buf()],
            &[],
        );

        assert_eq!(lookup.matched_icon("icon").as_deref(), Some("Icon"));
        Ok(())
    }

    // ── Z11: the background scan ────────────────────────────────────────

    #[test]
    fn a_lookup_whose_scan_has_not_finished_matches_nothing() {
        let lookup = XdgIconLookup {
            scans: Arc::new(OnceLock::new()),
        };

        assert_eq!(lookup.matched_icon("anything"), None);
        assert_eq!(lookup.desktop_icon("anything"), None);
    }

    #[test]
    fn the_background_scan_fills_the_lookup_when_it_finishes() -> TestResult {
        let dir = tempfile::tempdir()?;
        touch(dir.path(), "Found.png")?;
        fs::write(
            dir.path().join("app.desktop"),
            "[Desktop Entry]\nIcon=app-icon\n",
        )?;

        let (lookup, handle) = XdgIconLookup::start_scan(
            vec![dir.path().to_path_buf()],
            vec![dir.path().to_path_buf()],
            MAX_SCAN_ENTRIES,
        );
        handle
            .ok_or("the scan thread did not start")?
            .join()
            .map_err(|_| "the scan panicked")?;

        assert_eq!(lookup.matched_icon("found").as_deref(), Some("Found"));
        assert_eq!(lookup.desktop_icon("app").as_deref(), Some("app-icon"));
        Ok(())
    }

    #[test]
    fn the_scan_stops_at_the_entry_cap() -> TestResult {
        let dir = tempfile::tempdir()?;
        for index in 0..10 {
            touch(dir.path(), &format!("icon{index}.png"))?;
        }

        assert_eq!(scan_icons(&[dir.path().to_path_buf()], 10).len(), 10);
        assert_eq!(scan_icons(&[dir.path().to_path_buf()], 4).len(), 4);
        assert!(scan_icons(&[dir.path().to_path_buf()], 0).is_empty());
        Ok(())
    }

    #[test]
    fn the_entry_cap_counts_directories_too() -> TestResult {
        let dir = tempfile::tempdir()?;
        for index in 0..5 {
            touch(dir.path(), &format!("d{index}/icon{index}.png"))?;
        }

        // Five directories and their five files are ten entries.
        assert_eq!(scan_icons(&[dir.path().to_path_buf()], 10).len(), 5);
        assert!(scan_icons(&[dir.path().to_path_buf()], 5).len() < 5);
        Ok(())
    }

    // ── Z19: `.desktop` details ─────────────────────────────────────────

    #[test]
    fn a_byte_order_mark_does_not_hide_the_entry_group() {
        let entry = parse_desktop_entry("\u{feff}[Desktop Entry]\nName=Bom\nIcon=bom-icon\n");

        assert_eq!(
            entry,
            Some(DesktopEntry {
                name: Some("Bom".to_owned()),
                icon: "bom-icon".to_owned(),
            })
        );
    }

    #[test]
    fn a_byte_order_mark_in_a_file_on_disk_is_stripped() -> TestResult {
        let dir = tempfile::tempdir()?;
        fs::write(
            dir.path().join("bom.desktop"),
            b"\xEF\xBB\xBF[Desktop Entry]\nIcon=bom-icon\n",
        )?;
        let lookup = lookup_over(dir.path(), dir.path());

        assert_eq!(lookup.desktop_icon("bom").as_deref(), Some("bom-icon"));
        Ok(())
    }

    #[test]
    fn a_file_stem_outranks_another_files_name() -> TestResult {
        let dir = tempfile::tempdir()?;
        // Scanned first (and in an earlier root), but only *named* `target`.
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        fs::create_dir_all(&first)?;
        fs::create_dir_all(&second)?;
        fs::write(
            first.join("other.desktop"),
            "[Desktop Entry]\nName=Target\nIcon=by-name\n",
        )?;
        fs::write(
            second.join("target.desktop"),
            "[Desktop Entry]\nName=Something Else\nIcon=by-stem\n",
        )?;
        let lookup = XdgIconLookup::with_roots_sync(&[], &[first, second]);

        assert_eq!(lookup.desktop_icon("target").as_deref(), Some("by-stem"));
        assert_eq!(
            lookup.desktop_icon("something else").as_deref(),
            Some("by-stem")
        );
        Ok(())
    }

    #[test]
    fn a_desktop_file_in_a_subdirectory_has_a_dash_joined_id() -> TestResult {
        let dir = tempfile::tempdir()?;
        fs::create_dir_all(dir.path().join("kde"))?;
        fs::create_dir_all(dir.path().join("a/b"))?;
        fs::write(
            dir.path().join("kde/konsole.desktop"),
            "[Desktop Entry]\nIcon=konsole-icon\n",
        )?;
        fs::write(
            dir.path().join("a/b/deep.desktop"),
            "[Desktop Entry]\nIcon=deep-icon\n",
        )?;
        let lookup = lookup_over(dir.path(), dir.path());

        assert_eq!(
            lookup.desktop_icon("kde-konsole").as_deref(),
            Some("konsole-icon")
        );
        assert_eq!(
            lookup.desktop_icon("A-B-Deep").as_deref(),
            Some("deep-icon")
        );
        // The bare stem is not the ID of a file in a subdirectory.
        assert_eq!(lookup.desktop_icon("konsole"), None);
        Ok(())
    }

    #[test]
    fn the_desktop_file_id_joins_the_relative_path_with_dashes() {
        let root = Path::new("/usr/share/applications");

        assert_eq!(
            desktop_file_id(root, &root.join("x.desktop")).as_deref(),
            Some("x")
        );
        assert_eq!(
            desktop_file_id(root, &root.join("sub/x.desktop")).as_deref(),
            Some("sub-x")
        );
        assert_eq!(
            desktop_file_id(root, Path::new("/elsewhere/x.desktop")),
            None
        );
    }
}
