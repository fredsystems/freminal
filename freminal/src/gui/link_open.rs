// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Deciding whether a hyperlink target may be handed to the OS opener, and
//! opening it (GHSA-jhw2-xjg6-mwhq).
//!
//! OSC 8 lets terminal output attach an arbitrary URI to text, and a single
//! Ctrl/Cmd-click then passes it to the platform opener. Left unchecked that
//! launches `.desktop` / `.command` files, `ms-msdt:` / `search-ms:` handlers,
//! or `\\host\share` paths. [`classify_link`] is the allow-list that sits in
//! front of the opener; [`open_link`] is the only function in the GUI that
//! hands a terminal-supplied target to `open::that`.
//!
//! The allowed schemes are exactly the set auto-detection produces
//! (`freminal-buffer/src/url_detect.rs`), so one rule covers both the
//! auto-detected and the OSC 8 entry points. Confirmation of any other scheme
//! waits for the shared consent prompt (Task 133); refusing is the strict
//! subset of that behaviour.
//!
//! Two allowed schemes can still launch things, so they are narrowed further:
//! a `file:` target is refused when it names a launcher / executable type (by
//! extension, see [`LAUNCHER_EXTENSIONS`], or on unix by an execute bit at
//! click time), and a `mailto:` target is refused when it asks the mail client
//! to attach a local file (`attach=` / `attachment=`).

use freminal_terminal_emulator::ansi_components::tracer::escape_sequence_for_log_bounded;
use tracing::{debug, error, warn};

/// File extensions (ASCII case-insensitive, no leading dot) that mean "this
/// file runs something when opened": desktop-entry and launcher formats
/// (`desktop`, `command`, `tool`, `app`, `lnk`, `url`, `appimage`), Windows
/// executables and scripts (`exe`, `com`, `bat`, `cmd`, `scr`, `pif`, `msi`,
/// `msc`, `cpl`, `ps1`, `psm1`, `vbs`, `vbe`, `js`, `jse`, `wsf`, `wsh`,
/// `hta`), `jar`, and unix shell / installer types (`sh`, `bash`, `zsh`,
/// `fish`, `csh`, `run`, `bin`). A `file:` link to one of these is refused
/// because the platform opener may execute it instead of displaying it.
const LAUNCHER_EXTENSIONS: &[&str] = &[
    "desktop", "command", "tool", "app", "exe", "com", "bat", "cmd", "scr", "pif", "msi", "msc",
    "lnk", "url", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "hta", "cpl", "jar",
    "appimage", "sh", "bash", "zsh", "fish", "csh", "run", "bin",
];

/// Outcome of [`classify_link`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkOpenDecision {
    /// The target may be passed to the OS opener.
    Open,
    /// The target must not be opened, for the given reason.
    Refused(LinkRefusal),
}

/// Why a link target was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRefusal {
    /// The target is not a well-formed absolute URL: no valid RFC 3986
    /// scheme (bare path, drive-letter path, empty string), a scheme that
    /// requires `//` without it, a relative `file:` path, or it contains
    /// whitespace or control characters.
    NotAUrl,
    /// A well-formed URL whose scheme is not on the allow-list.
    SchemeNotAllowed,
    /// A `file:` URL naming a host other than the local machine.
    RemoteFileHost,
    /// A UNC-style target (`\\host\share`, `//host/share`) or a `file:` path
    /// that encodes one.
    UncPath,
    /// A `file:` URL naming a launcher / executable type (see
    /// [`LAUNCHER_EXTENSIONS`]), or on unix an existing regular file with an
    /// execute bit set. Opening it could run it rather than display it.
    ExecutableTarget,
    /// A `mailto:` URL with an `attach` / `attachment` parameter, which asks
    /// the mail client to attach a local file to the draft.
    MailtoAttachment,
}

/// Classify `target` against the link allow-list.
///
/// Allowed: `http`, `https`, `ftp`, `mailto` (without an attachment
/// parameter), and `file` only when it is local (empty authority or
/// `localhost`, absolute path, not a UNC form, not a launcher extension).
/// Scheme comparison is case-insensitive.
///
/// Pure: it never touches the filesystem. The unix execute-bit check lives in
/// [`decide_open`], which [`open_link`] uses.
#[must_use]
pub fn classify_link(target: &str) -> LinkOpenDecision {
    use LinkOpenDecision::{Open, Refused};
    use LinkRefusal::{NotAUrl, SchemeNotAllowed};

    if target.is_empty() || target.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Refused(NotAUrl);
    }
    if target.starts_with("\\\\") || target.starts_with("//") {
        return Refused(LinkRefusal::UncPath);
    }

    let Some((scheme, rest)) = split_scheme(target) else {
        return Refused(NotAUrl);
    };

    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" | "ftp" => {
            if rest.starts_with("//") {
                Open
            } else {
                Refused(NotAUrl)
            }
        }
        "mailto" => {
            if mailto_has_attachment(rest) {
                Refused(LinkRefusal::MailtoAttachment)
            } else {
                Open
            }
        }
        "file" => classify_file(rest),
        _ => Refused(SchemeNotAllowed),
    }
}

/// Split `target` into its RFC 3986 §3.1 scheme and the text after the colon.
///
/// Returns `None` when there is no valid scheme. A single-letter "scheme" is
/// treated as a Windows drive letter (`C:\x`), not a URL.
fn split_scheme(target: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = target.split_once(':')?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    if scheme.len() == 1 {
        return None;
    }
    Some((scheme, rest))
}

/// Classify the part of a `file:` URI after the colon (RFC 8089).
fn classify_file(rest: &str) -> LinkOpenDecision {
    use LinkOpenDecision::{Open, Refused};
    use LinkRefusal::{NotAUrl, RemoteFileHost, UncPath};

    let (authority, path) = split_file_authority(rest);

    if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
        return Refused(RemoteFileHost);
    }
    if !path.starts_with('/') {
        return Refused(NotAUrl);
    }
    let lowered = path.to_ascii_lowercase();
    // `file:////host/share` is a UNC path in disguise, also when the second
    // slash is percent-encoded (`/%2F...`), which an opener may decode.
    if path.starts_with("//") || lowered.starts_with("/%2f") {
        return Refused(RemoteFileHost);
    }
    // A backslash or encoded separator can re-introduce a UNC form once the
    // platform opener decodes the path.
    if path.contains('\\') || lowered.contains("%5c") {
        return Refused(UncPath);
    }
    if has_launcher_extension(path) {
        return Refused(LinkRefusal::ExecutableTarget);
    }
    Open
}

/// Split the part of a `file:` URI after the colon into `(authority, path)`.
///
/// `file:/path` has no authority (empty authority string); `file:relative`
/// yields a path that is not absolute, which the caller rejects.
fn split_file_authority(rest: &str) -> (&str, &str) {
    rest.strip_prefix("//").map_or(("", rest), |after| {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        after.split_at(end)
    })
}

/// The path of a `file:` URI without any query or fragment.
fn path_without_query(path: &str) -> &str {
    path.find(['?', '#']).map_or(path, |i| &path[..i])
}

/// Decode `%XX` escapes. Malformed escapes pass through verbatim; the result
/// is raw bytes (a path need not be UTF-8).
fn percent_decode(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hi) = bytes.get(i + 1).copied().and_then(hex_value)
            && let Some(lo) = bytes.get(i + 2).copied().and_then(hex_value)
        {
            out.push(hi << 4 | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Numeric value of an ASCII hex digit.
const fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Whether the final path component of a `file:` path ends in one of
/// [`LAUNCHER_EXTENSIONS`], after percent-decoding.
///
/// Both the full path and the path without its query/fragment are examined,
/// since an opener may treat `?` as part of the file name. Trailing path
/// separators are ignored (a trailing slash does not hide `Foo.app/`), as are
/// trailing dots and spaces and an NTFS `:stream` suffix, which Windows strips
/// before resolving the file.
fn has_launcher_extension(path: &str) -> bool {
    [path, path_without_query(path)]
        .into_iter()
        .any(|candidate| file_name_has_launcher_extension(&percent_decode(candidate)))
}

/// [`has_launcher_extension`] on one decoded path.
fn file_name_has_launcher_extension(decoded: &[u8]) -> bool {
    let is_separator = |b: &u8| matches!(b, b'/' | b'\\');
    let trimmed = decoded.trim_ascii_end();
    // Drop trailing separators, then everything up to the last one.
    let end = trimmed
        .iter()
        .rposition(|b| !is_separator(b))
        .map_or(0, |i| i + 1);
    let trimmed = &trimmed[..end];
    let start = trimmed.iter().rposition(is_separator).map_or(0, |i| i + 1);
    // An NTFS `name:stream` suffix, then trailing dots and spaces, are
    // dropped by Windows before it resolves the file.
    let mut name = trimmed[start..]
        .split(|b| *b == b':')
        .next()
        .unwrap_or_default();
    while let Some(shorter) = name.strip_suffix(b".").or_else(|| name.strip_suffix(b" ")) {
        name = shorter;
    }
    name.iter().rposition(|b| *b == b'.').is_some_and(|dot| {
        let extension = &name[dot + 1..];
        LAUNCHER_EXTENSIONS
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known.as_bytes()))
    })
}

/// Whether the part of a `mailto:` URI after the colon carries an `attach` or
/// `attachment` header, which makes common mail clients attach a local file.
///
/// The whole text is percent-decoded first so an encoded key (`%61ttach`) or
/// an encoded `?` separator cannot hide the parameter; the query is then split
/// on `&` and `;`, and each key is compared ASCII case-insensitively.
fn mailto_has_attachment(rest: &str) -> bool {
    let decoded = String::from_utf8_lossy(&percent_decode(rest)).into_owned();
    let Some((_, query)) = decoded.split_once('?') else {
        return false;
    };
    query.split(['&', ';']).any(|pair| {
        let key = pair.split_once('=').map_or(pair, |(key, _)| key).trim();
        key.eq_ignore_ascii_case("attach") || key.eq_ignore_ascii_case("attachment")
    })
}

/// [`classify_link`] plus the one check that needs the filesystem: on unix, a
/// `file:` target naming an existing regular file with any execute bit set is
/// refused as [`LinkRefusal::ExecutableTarget`] (directories are allowed; the
/// opener shows them in the file manager).
///
/// Called once per click, never per frame, so the `stat` is a user action.
#[must_use]
pub fn decide_open(target: &str) -> LinkOpenDecision {
    match classify_link(target) {
        LinkOpenDecision::Open if local_file_is_executable(target) => {
            LinkOpenDecision::Refused(LinkRefusal::ExecutableTarget)
        }
        decision => decision,
    }
}

/// The percent-decoded filesystem path named by a `file:` target, if it is
/// one.
#[cfg(unix)]
fn local_file_path(target: &str) -> Option<Vec<u8>> {
    let (scheme, rest) = split_scheme(target)?;
    if !scheme.eq_ignore_ascii_case("file") {
        return None;
    }
    let (_, path) = split_file_authority(rest);
    Some(percent_decode(path_without_query(path)))
}

/// Whether `target` is a `file:` link to an existing regular file with an
/// execute bit set. Always `false` off unix, where the extension list is the
/// only available signal.
#[cfg(unix)]
fn local_file_is_executable(target: &str) -> bool {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;

    let Some(path) = local_file_path(target) else {
        return false;
    };
    std::fs::metadata(OsStr::from_bytes(&path))
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
const fn local_file_is_executable(_target: &str) -> bool {
    false
}

/// Open `target` with the platform opener if [`decide_open`] allows it.
///
/// A refused target is logged (reason only at `warn`; a bounded, escaped copy
/// at `debug`) and otherwise ignored. The opener runs on its own thread so a
/// slow handler cannot stall the GUI.
pub fn open_link(target: String) {
    if let LinkOpenDecision::Refused(reason) = decide_open(&target) {
        warn!("Refusing to open hyperlink: {reason:?}");
        debug!(
            "Refused hyperlink target: {}",
            escape_sequence_for_log_bounded(target.as_bytes())
        );
        return;
    }

    if let Err(e) = std::thread::Builder::new()
        .name("freminal-open-url".to_string())
        .spawn(move || {
            if let Err(e) = open::that(&target) {
                error!("Failed to open hyperlink: {e}");
                debug!(
                    "Failed hyperlink target: {}",
                    escape_sequence_for_log_bounded(target.as_bytes())
                );
            }
        })
    {
        error!("Failed to spawn URL-open thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::LinkOpenDecision::{Open, Refused};
    use super::LinkRefusal::{
        ExecutableTarget, MailtoAttachment, NotAUrl, RemoteFileHost, SchemeNotAllowed, UncPath,
    };
    use super::*;

    #[test]
    fn allowed_targets_open() {
        for target in [
            "https://example.com/x",
            "HTTP://EXAMPLE.COM",
            "ftp://h/x",
            "mailto:a@b.c",
            "file:///home/u/x.txt",
            "file://localhost/tmp/x",
            "file:/home/u/x.txt",
            "file:///C:/Users/u/x.txt",
            // Directories and extension-less names are opened in the file
            // manager / default handler.
            "file:///home/u/projects/",
            "file:///home/u/README",
            "file:///home/u/archive.tar.gz",
            // An extension that merely contains a launcher name is not one.
            "file:///home/u/x.shtml",
            "file:///home/u/notes.txt?x=1#frag",
            "mailto:a@b.c?subject=hi&body=attached%20below",
            "mailto:a@b.c?cc=d@e.f",
        ] {
            assert_eq!(classify_link(target), Open, "{target}");
        }
    }

    #[test]
    fn refused_targets_are_refused() {
        let cases: &[(&str, LinkRefusal)] = &[
            ("javascript:alert(1)", SchemeNotAllowed),
            ("ms-msdt:/id", SchemeNotAllowed),
            ("search-ms:query=x", SchemeNotAllowed),
            ("smb://h/s", SchemeNotAllowed),
            ("file://attacker/share/x", RemoteFileHost),
            ("file:////attacker/share/x", RemoteFileHost),
            ("file://localhost//attacker/share", RemoteFileHost),
            ("file:///%2F%2Fattacker/share", RemoteFileHost),
            ("file:///%2fattacker/share", RemoteFileHost),
            ("file:///\\\\attacker\\share", UncPath),
            ("file:///%5C%5Cattacker/share", UncPath),
            ("file:relative.txt", NotAUrl),
            ("file://", NotAUrl),
            ("\\\\attacker\\share\\x", UncPath),
            ("//attacker/share", UncPath),
            ("/home/u/x.desktop", NotAUrl),
            ("C:\\x.exe", NotAUrl),
            ("x.command", NotAUrl),
            ("http:example.com", NotAUrl),
            ("https://exa mple.com", NotAUrl),
            ("https://example.com/\x07", NotAUrl),
            ("", NotAUrl),
            (":nope", NotAUrl),
            ("1http://x", NotAUrl),
            ("file:///home/u/x.desktop", ExecutableTarget),
            ("mailto:a@b.c?attach=/etc/passwd", MailtoAttachment),
        ];
        for (target, reason) in cases {
            assert_eq!(classify_link(target), Refused(*reason), "{target:?}");
        }
    }

    #[test]
    fn every_launcher_extension_is_refused_for_file_targets() {
        for extension in LAUNCHER_EXTENSIONS {
            for target in [
                format!("file:///home/u/x.{extension}"),
                format!("file:///home/u/x.{}", extension.to_ascii_uppercase()),
                format!("file://localhost/tmp/dir.v1/run.{extension}"),
            ] {
                assert_eq!(
                    classify_link(&target),
                    Refused(ExecutableTarget),
                    "{target}"
                );
            }
        }
    }

    #[test]
    fn launcher_extensions_are_found_through_encoding_and_decoration() {
        for target in [
            // Percent-encoded dot, letters and separators.
            "file:///home/u/x%2Edesktop",
            "file:///home/u/x.%64esktop",
            "file:///home/u/x.DeSkToP",
            "file:///home/u/dir%2Fx.command",
            // Query / fragment after the launcher name, or after a decoy.
            "file:///home/u/x.desktop?a=b",
            "file:///home/u/x.desktop#frag",
            "file:///home/u/x.txt?y.desktop",
            // Trailing separators, dots, spaces and NTFS streams.
            "file:///Applications/Foo.app/",
            "file:///home/u/x.exe.",
            "file:///home/u/x.exe%20",
            "file:///home/u/x.exe:Zone.Identifier",
            "file:///C:/Windows/System32/calc.exe",
            // Backslash separator (encoded) before the name.
            "file:///home/u/dir%5Cx.bat",
        ] {
            assert!(
                matches!(classify_link(target), Refused(_)),
                "{target} must be refused"
            );
        }
        // The encoded-backslash and encoded-slash-leading forms are refused
        // as UNC; the rest are refused as executable targets.
        assert_eq!(
            classify_link("file:///home/u/x%2Edesktop"),
            Refused(ExecutableTarget)
        );
        assert_eq!(
            classify_link("file:///Applications/Foo.app/"),
            Refused(ExecutableTarget)
        );
        assert_eq!(
            classify_link("file:///home/u/x.exe:Zone.Identifier"),
            Refused(ExecutableTarget)
        );
    }

    #[test]
    fn mailto_attachment_parameters_are_refused() {
        for target in [
            "mailto:a@b.c?attach=/etc/passwd",
            "mailto:a@b.c?ATTACH=/etc/passwd",
            "mailto:a@b.c?attachment=/etc/passwd",
            "mailto:a@b.c?subject=hi&attach=/etc/passwd",
            "mailto:a@b.c?subject=hi;attachment=/etc/passwd",
            // Percent-encoded key, and an encoded `?` separator.
            "mailto:a@b.c?%61ttach=/etc/passwd",
            "mailto:a@b.c?subject=hi&%41TTACHMENT=/etc/passwd",
            "mailto:a@b.c%3Fattach=/etc/passwd",
        ] {
            assert_eq!(classify_link(target), Refused(MailtoAttachment), "{target}");
        }
    }

    #[test]
    fn mailto_with_attachment_word_only_in_a_value_is_allowed() {
        for target in [
            "mailto:attach@b.c",
            "mailto:a@b.c?subject=attach",
            "mailto:a@b.c?body=please%20attach%20the%20file",
            "mailto:a@b.c?subject=attachment=x",
        ] {
            assert_eq!(classify_link(target), Open, "{target}");
        }
    }

    #[test]
    fn percent_decode_passes_malformed_escapes_through() {
        assert_eq!(percent_decode("a%2Fb"), b"a/b");
        assert_eq!(percent_decode("100%"), b"100%");
        assert_eq!(percent_decode("%zz%4"), b"%zz%4");
        assert_eq!(percent_decode("%e9"), [0xe9]);
    }

    /// Percent-encode everything but unreserved characters and `/`.
    #[cfg(unix)]
    fn file_url(path: &std::path::Path) -> String {
        use std::fmt::Write as _;
        use std::os::unix::ffi::OsStrExt;

        let mut url = String::from("file://");
        for &b in path.as_os_str().as_bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-') {
                url.push(char::from(b));
            } else {
                let _ = write!(url, "%{b:02X}");
            }
        }
        url
    }

    #[cfg(unix)]
    #[test]
    fn unix_execute_bit_makes_a_file_target_refused() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("payload.txt");
        std::fs::write(&file, b"data").expect("write");
        let url = file_url(&file);

        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert_eq!(decide_open(&url), Open, "non-executable file opens");

        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(
            decide_open(&url),
            Refused(ExecutableTarget),
            "executable file is refused despite a harmless extension"
        );
        // The pure classifier does not stat.
        assert_eq!(classify_link(&url), Open);

        // Any single execute bit counts.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o601)).expect("chmod");
        assert_eq!(decide_open(&url), Refused(ExecutableTarget));
    }

    #[cfg(unix)]
    #[test]
    fn unix_directories_and_missing_paths_are_not_treated_as_executables() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        // Directories always carry execute bits; they open in the file manager.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
        assert_eq!(decide_open(&file_url(dir.path())), Open);
        assert_eq!(
            decide_open(&file_url(&dir.path().join("missing.txt"))),
            Open
        );
        // Non-file schemes never reach the stat.
        assert_eq!(decide_open("https://example.com/x"), Open);
    }

    #[test]
    fn every_auto_detected_scheme_is_allowed() {
        // A scheme added to auto-detection without a matching allow-list entry
        // would produce links that refuse to open; this fails instead.
        for scheme in freminal_terminal_emulator::AUTO_DETECT_SCHEMES {
            let scheme = std::str::from_utf8(scheme).expect("scheme literals are ASCII");
            // The smallest valid target for the scheme's shape: `file:` needs
            // an absolute path, `mailto:` an address, the rest a host.
            let rest = match scheme {
                "file://" => "/tmp/x",
                "mailto:" => "a@b.c",
                _ => "example.com",
            };
            let target = format!("{scheme}{rest}");
            assert_eq!(classify_link(&target), Open, "{target}");
        }
    }
}
