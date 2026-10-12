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
//! - does an icon called `name` exist anywhere in the icon directories?
//!   ([`Osc99IconLookup::icon_exists`]), and
//! - which icon does the application called `f=` use?
//!   ([`Osc99IconLookup::desktop_icon`], through its `.desktop` file).
//!
//! Each scan runs once, lazily, on the first question that needs it, which is
//! on the notification service's dispatcher thread, so the GUI thread never
//! walks the filesystem. The directories are injected
//! ([`XdgIconLookup::with_roots`]) so tests use a temporary directory.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Read;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use super::presentation::Osc99IconLookup;

/// The deepest a scan descends: a file `MAX_SCAN_DEPTH` path components below
/// a root is found, one further down is not. `hicolor/48x48/apps/x.png` is
/// four deep.
const MAX_SCAN_DEPTH: usize = 6;

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

/// The icon and application directories, scanned lazily.
pub struct XdgIconLookup {
    /// Directories searched for icon files, highest priority first.
    icon_roots: Vec<PathBuf>,
    /// Directories searched for `.desktop` files, highest priority first.
    app_roots: Vec<PathBuf>,
    /// The lowercased stem of every icon file, once scanned.
    icons: OnceCell<HashSet<String>>,
    /// The `Icon=` of every `.desktop` file by lowercased stem and by
    /// lowercased `Name=`, once scanned.
    desktop: OnceCell<HashMap<String, String>>,
}

impl XdgIconLookup {
    /// The lookup over the standard directories of this process's
    /// environment.
    #[must_use]
    pub fn from_env() -> Self {
        let (icon_roots, app_roots) = roots_from_env(
            std::env::var_os("XDG_DATA_HOME"),
            std::env::var_os("XDG_DATA_DIRS"),
            std::env::var_os("HOME"),
        );
        Self::with_roots(icon_roots, app_roots)
    }

    /// The lookup over the given directories, highest priority first.
    #[must_use]
    pub const fn with_roots(icon_roots: Vec<PathBuf>, app_roots: Vec<PathBuf>) -> Self {
        Self {
            icon_roots,
            app_roots,
            icons: OnceCell::new(),
            desktop: OnceCell::new(),
        }
    }

    fn icon_names(&self) -> &HashSet<String> {
        self.icons.get_or_init(|| scan_icons(&self.icon_roots))
    }

    fn desktop_entries(&self) -> &HashMap<String, String> {
        self.desktop
            .get_or_init(|| scan_desktop_files(&self.app_roots))
    }
}

impl Osc99IconLookup for XdgIconLookup {
    fn icon_exists(&self, name: &str) -> bool {
        self.icon_names().contains(&name.to_lowercase())
    }

    fn desktop_icon(&self, app_name: &str) -> Option<String> {
        self.desktop_entries()
            .get(&app_name.to_lowercase())
            .cloned()
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

/// Visit every file under `roots`, bounded by [`MAX_SCAN_DEPTH`].
///
/// Symbolic links are followed (a Nix profile is a tree of them), but a
/// directory is entered only once per scan, judged by its canonical path, so a
/// link back up the tree cannot loop. I/O errors are ignored: a directory that
/// cannot be read is skipped. `on_file` returns [`ControlFlow::Break`] to stop
/// the whole scan.
fn walk_files(roots: &[PathBuf], on_file: &mut dyn FnMut(&Path) -> ControlFlow<()>) {
    let mut entered: HashSet<PathBuf> = HashSet::new();
    for root in roots {
        if walk_dir(root, 1, &mut entered, on_file).is_break() {
            return;
        }
    }
}

/// Visit the files in `dir`, whose direct children are `depth` components
/// below their root, and recurse into its subdirectories.
fn walk_dir(
    dir: &Path,
    depth: usize,
    entered: &mut HashSet<PathBuf>,
    on_file: &mut dyn FnMut(&Path) -> ControlFlow<()>,
) -> ControlFlow<()> {
    if depth > MAX_SCAN_DEPTH {
        return ControlFlow::Continue(());
    }
    let Ok(canonical) = dir.canonicalize() else {
        return ControlFlow::Continue(());
    };
    if !entered.insert(canonical) {
        return ControlFlow::Continue(());
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return ControlFlow::Continue(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // `metadata` follows a symbolic link to what it points at.
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            walk_dir(&path, depth + 1, entered, on_file)?;
        } else if metadata.is_file() {
            on_file(&path)?;
        }
    }
    ControlFlow::Continue(())
}

/// The lowercased stems of the icon files under `roots`.
fn scan_icons(roots: &[PathBuf]) -> HashSet<String> {
    let mut names = HashSet::new();
    walk_files(roots, &mut |path| {
        if has_extension(path, &ICON_EXTENSIONS)
            && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
        {
            names.insert(stem.to_lowercase());
            if names.len() >= MAX_ICON_NAMES {
                return ControlFlow::Break(());
            }
        }
        ControlFlow::Continue(())
    });
    names
}

/// `Icon=` by lowercased file stem and by lowercased `Name=` for the
/// `.desktop` files under `roots`. The first file to claim a key keeps it, so
/// the directories earlier in `roots` take priority.
fn scan_desktop_files(roots: &[PathBuf]) -> HashMap<String, String> {
    let mut entries: HashMap<String, String> = HashMap::new();
    let mut files_read = 0usize;
    walk_files(roots, &mut |path| {
        if !has_extension(path, &["desktop"]) {
            return ControlFlow::Continue(());
        }
        files_read += 1;
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            return ControlFlow::Continue(());
        };
        if let Some(entry) = read_desktop_entry(path) {
            if !stem.is_empty() {
                entries
                    .entry(stem.to_lowercase())
                    .or_insert_with(|| entry.icon.clone());
            }
            if let Some(name) = entry.name.filter(|name| !name.is_empty()) {
                entries.entry(name.to_lowercase()).or_insert(entry.icon);
            }
        }
        if files_read >= MAX_DESKTOP_FILES {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    entries
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
/// Localised names (`Name[de]=`) are ignored. `None` when the group has no
/// non-empty `Icon=`: such a file cannot answer an icon question.
fn parse_desktop_entry(contents: &str) -> Option<DesktopEntry> {
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
        XdgIconLookup::with_roots(vec![icons.to_path_buf()], vec![apps.to_path_buf()])
    }

    #[test]
    fn an_icon_in_a_theme_tree_exists_case_insensitively() -> TestResult {
        let dir = tempfile::tempdir()?;
        touch(dir.path(), "hicolor/48x48/apps/foo.png")?;
        touch(dir.path(), "hicolor/scalable/apps/Scal.svg")?;
        touch(dir.path(), "hicolor/48x48/apps/readme.txt")?;
        let lookup = lookup_over(dir.path(), dir.path());

        assert!(lookup.icon_exists("Foo"));
        assert!(lookup.icon_exists("foo"));
        assert!(lookup.icon_exists("scal"));
        assert!(!lookup.icon_exists("bar"));
        assert!(!lookup.icon_exists("readme"));
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
        let lookup = XdgIconLookup::with_roots(
            Vec::new(),
            vec![first.path().to_path_buf(), second.path().to_path_buf()],
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

        assert!(lookup.icon_exists("shallow"));
        assert!(!lookup.icon_exists("deep"));
        Ok(())
    }

    #[test]
    fn a_symlink_loop_does_not_hang_the_scan() -> TestResult {
        let dir = tempfile::tempdir()?;
        touch(dir.path(), "a/x.png")?;
        std::os::unix::fs::symlink(dir.path().join("a"), dir.path().join("a/loop"))?;
        std::os::unix::fs::symlink(dir.path(), dir.path().join("a/up"))?;
        let lookup = lookup_over(dir.path(), dir.path());

        assert!(lookup.icon_exists("x"));
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

        assert!(lookup.icon_exists("linked"));
        assert!(lookup.icon_exists("single"));
        Ok(())
    }

    #[test]
    fn missing_roots_are_ignored() {
        let lookup = XdgIconLookup::with_roots(
            vec![PathBuf::from("/nonexistent/freminal/icons")],
            vec![PathBuf::from("/nonexistent/freminal/applications")],
        );

        assert!(!lookup.icon_exists("anything"));
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
}
