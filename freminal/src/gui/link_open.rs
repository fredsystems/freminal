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

use freminal_terminal_emulator::ansi_components::tracer::escape_sequence_for_log_bounded;
use tracing::{debug, error, warn};

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
}

/// Classify `target` against the link allow-list.
///
/// Allowed: `http`, `https`, `ftp`, `mailto`, and `file` only when it is local
/// (empty authority or `localhost`, absolute path, not a UNC form). Scheme
/// comparison is case-insensitive.
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
        "mailto" => Open,
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

    // `file:/path` has no authority; `file:relative` is not absolute.
    let (authority, path) = rest.strip_prefix("//").map_or(("", rest), |after| {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        after.split_at(end)
    });

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
    Open
}

/// Open `target` with the platform opener if [`classify_link`] allows it.
///
/// A refused target is logged (reason only at `warn`; a bounded, escaped copy
/// at `debug`) and otherwise ignored. The opener runs on its own thread so a
/// slow handler cannot stall the GUI.
pub fn open_link(target: String) {
    if let LinkOpenDecision::Refused(reason) = classify_link(&target) {
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
    use super::LinkRefusal::{NotAUrl, RemoteFileHost, SchemeNotAllowed, UncPath};
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
        ];
        for (target, reason) in cases {
            assert_eq!(classify_link(target), Refused(*reason), "{target:?}");
        }
    }

    #[test]
    fn every_auto_detected_scheme_is_allowed() {
        // Mirrors the scheme list in `freminal-buffer/src/url_detect.rs`
        // (not exported there); keep the two in step.
        for target in [
            "http://example.com",
            "https://example.com",
            "file:///tmp/x",
            "ftp://example.com",
            "mailto:a@b.c",
        ] {
            assert_eq!(classify_link(target), Open, "{target}");
        }
    }
}
