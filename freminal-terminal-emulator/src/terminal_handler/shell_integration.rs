// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Shell integration helpers for [`TerminalHandler`].
//!
//! This module contains the OSC 7 CWD-tracking URI parser and its
//! supporting utilities:
//!
//! - [`parse_osc7_uri`] — validate and parse `file://hostname/path` URIs
//!   emitted by OSC 7
//! - [`Osc7Rejection`] — why an OSC 7 URI was refused
//! - [`Osc7PathStyle`] — which path syntax the decoded URI path must follow
//! - [`local_hostname`] — the machine's own hostname, resolved once
//! - [`percent_decode`] — strict URL percent-decode of a string
//! - [`hex_val`] — convert a single ASCII hex digit to its numeric value

use std::sync::OnceLock;

use freminal_common::windows_path::{WindowsPathKind, classify_windows_path};

/// Why an OSC 7 URI was refused by [`parse_osc7_uri`].
///
/// OSC 7 is unauthenticated: any program that can write to the terminal (a
/// `cat`-ed file, a remote shell, an `ssh` session) can emit it. The reported
/// directory is later used to start new tabs and splits and is written into
/// saved layouts, so everything that is not a plain absolute local path is
/// rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Osc7Rejection {
    /// The URI does not start with `file://`.
    #[error("not a file:// URI")]
    NotFileScheme,
    /// The URI has no path component after the host.
    #[error("URI has no path")]
    NoPath,
    /// The URI names a host other than this machine; the path refers to a
    /// directory on another computer.
    #[error("URI names a host other than this machine")]
    ForeignHost,
    /// The percent-decoded path is not valid UTF-8, is not absolute, or
    /// contains a control character (including NUL).
    #[error("path is not a valid absolute path")]
    InvalidPath,
}

/// The local machine's hostname, resolved once per process.
///
/// The value is immutable after first initialisation, so the [`OnceLock`]
/// involves no locking concern between threads. `None` when the OS cannot
/// report a hostname (or reports a non-UTF-8 / empty one); in that case only
/// the empty host and `localhost` are accepted by [`parse_osc7_uri`].
pub(super) fn local_hostname() -> Option<&'static str> {
    static HOSTNAME: OnceLock<Option<String>> = OnceLock::new();
    HOSTNAME
        .get_or_init(|| {
            let name = read_local_hostname()?;
            if name.is_empty() { None } else { Some(name) }
        })
        .as_deref()
}

#[cfg(unix)]
fn read_local_hostname() -> Option<String> {
    nix::unistd::gethostname()
        .ok()
        .and_then(|h| h.into_string().ok())
}

#[cfg(windows)]
fn read_local_hostname() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}

#[cfg(not(any(unix, windows)))]
fn read_local_hostname() -> Option<String> {
    None
}

/// The first DNS label of `host` (everything before the first `.`).
fn first_label(host: &str) -> &str {
    host.split('.').next().unwrap_or(host)
}

/// Whether the host component of a `file://` URI designates this machine.
///
/// RFC 8089 §2 and §3: the host of a file URI is either empty or the string
/// `localhost` for the local machine, and any other host names a different
/// machine. Host names compare ASCII case-insensitively (RFC 4343).
///
/// A shell commonly reports the short name (`mac`) while the OS hostname is
/// fully qualified (`mac.local`), so a reported host equal to the first DNS
/// label of the local name is also accepted. The reverse is **not**: a
/// reported FQDN whose first label equals the local name (`myhost.attacker.example`
/// against a local `myhost`) names a different machine and is rejected, so a
/// remote party cannot claim to be this host by choosing a suffix. The price
/// is that a shell reporting a FQDN while `gethostname` returns the short name
/// is rejected too, and the pane falls back to the process's working directory.
fn host_is_local(host: &str, local_hostname: Option<&str>) -> bool {
    if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let Some(local) = local_hostname.filter(|l| !l.is_empty()) else {
        return false;
    };
    host.eq_ignore_ascii_case(local) || host.eq_ignore_ascii_case(first_label(local))
}

/// Path syntax an OSC 7 URI path must follow for the host platform.
///
/// A `file://` URI path is always `/`-rooted, but what follows depends on the
/// platform: on POSIX it *is* the filesystem path, while on Windows a local
/// drive path appears as `/C:/Users/x` and the leading `/` is URI syntax, not
/// part of the path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Osc7PathStyle {
    /// POSIX: the decoded URI path is used verbatim.
    Posix,
    /// Windows: the decoded URI path must be `/` followed by a local
    /// drive-letter path; the `/` is stripped.
    Windows,
}

impl Osc7PathStyle {
    /// The style of the platform freminal is running on.
    const fn native() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Posix
        }
    }
}

/// Parse an OSC 7 URI of the form `file://hostname/path` and return the path
/// component, using the running platform's path syntax.
///
/// See [`parse_osc7_uri_styled`] for the rules.
///
/// # Errors
///
/// Returns an [`Osc7Rejection`] describing why the URI was refused.
pub(super) fn parse_osc7_uri(
    uri: &str,
    local_hostname: Option<&str>,
) -> Result<String, Osc7Rejection> {
    parse_osc7_uri_styled(uri, local_hostname, Osc7PathStyle::native())
}

/// Parse an OSC 7 URI with an explicit [`Osc7PathStyle`].
///
/// The host must designate this machine: empty, `localhost`
/// (RFC 8089 §2/§3), or `local_hostname` (RFC 4343 case-insensitive match,
/// including the local name's own first DNS label; see [`host_is_local`]). When
/// `local_hostname` is `None` only the empty host and `localhost` are
/// accepted. Any other host yields [`Osc7Rejection::ForeignHost`].
///
/// Percent-encoded bytes (e.g. `%20` for space) are decoded so the returned
/// path is a normal filesystem path string. The decoded path must be valid
/// UTF-8 (no lossy replacement), absolute, and free of control characters
/// (NUL, C0 and C1); otherwise [`Osc7Rejection::InvalidPath`] is returned.
///
/// With [`Osc7PathStyle::Windows`] the decoded path must additionally be `/`
/// followed by a local drive path (`/C:/Users/x`); the `/` is stripped, and
/// anything else (UNC `//host/share`, device paths, drive-relative or
/// drive-less paths) is [`Osc7Rejection::InvalidPath`].
fn parse_osc7_uri_styled(
    uri: &str,
    local_hostname: Option<&str>,
    style: Osc7PathStyle,
) -> Result<String, Osc7Rejection> {
    let rest = uri
        .strip_prefix("file://")
        .ok_or(Osc7Rejection::NotFileScheme)?;

    // The path starts at the first '/' after the hostname.
    // `file:///path` (empty hostname) → host = "",         path = "/path"
    // `file://hostname/path`          → host = "hostname", path = "/path"
    let slash_pos = rest.find('/').ok_or(Osc7Rejection::NoPath)?;
    let (host, path) = rest.split_at(slash_pos);

    if !host_is_local(host, local_hostname) {
        return Err(Osc7Rejection::ForeignHost);
    }

    let decoded = percent_decode(path).ok_or(Osc7Rejection::InvalidPath)?;
    if !decoded.starts_with('/') || decoded.chars().any(char::is_control) {
        return Err(Osc7Rejection::InvalidPath);
    }

    match style {
        Osc7PathStyle::Posix => Ok(decoded),
        Osc7PathStyle::Windows => {
            let drive_path = decoded.strip_prefix('/').unwrap_or(&decoded);
            if classify_windows_path(drive_path) == WindowsPathKind::LocalDrive {
                Ok(drive_path.to_owned())
            } else {
                Err(Osc7Rejection::InvalidPath)
            }
        }
    }
}

/// Decode percent-encoded bytes (`%XX`) in a string.
///
/// Only valid two-hex-digit sequences are decoded; malformed sequences are
/// passed through verbatim. Returns `None` when the decoded bytes are not
/// valid UTF-8 (they are never lossily replaced).
fn percent_decode(input: &str) -> Option<String> {
    let mut output = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            output.push(hi << 4 | lo);
            i += 3;
            continue;
        }
        output.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(output).ok()
}

/// Convert an ASCII hex digit to its numeric value.
const fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use freminal_common::buffer_states::{
        ftcs::{FtcsMarker, FtcsState, PromptKind},
        osc::AnsiOscType,
    };

    use super::*;
    use crate::terminal_handler::TerminalHandler;

    // ------------------------------------------------------------------
    // OSC 7 CWD tracking tests
    // ------------------------------------------------------------------

    const HOST: Option<&str> = Some("myhost.example.org");

    /// A per-platform OSC 7 sample: `(uri, expected_cwd)` for a directory
    /// `sub` below the sample root, reported by the `localhost` host.
    ///
    /// The native path style decides what a local directory looks like in a
    /// `file://` URI, so handler-level tests that run through the native
    /// parser cannot hard-code a POSIX path.
    fn sample_osc7(sub: &str) -> (String, String) {
        if cfg!(windows) {
            (
                format!("file://localhost/C:/Users/user{sub}"),
                format!("C:/Users/user{sub}"),
            )
        } else {
            (
                format!("file://localhost/home/user{sub}"),
                format!("/home/user{sub}"),
            )
        }
    }

    #[test]
    fn parse_osc7_uri_table() {
        let cases: &[(&str, Option<&str>, Result<&str, Osc7Rejection>)] = &[
            // Host handling.
            ("file:///home/user", HOST, Ok("/home/user")),
            ("file://localhost/tmp", HOST, Ok("/tmp")),
            ("file://LOCALHOST/tmp", HOST, Ok("/tmp")),
            ("file://LocalHost/tmp", None, Ok("/tmp")),
            ("file://myhost.example.org/tmp", HOST, Ok("/tmp")),
            ("file://MYHOST.Example.ORG/tmp", HOST, Ok("/tmp")),
            // Reported short name matches the first label of the local name.
            (
                "file://myhost/home/user/projects",
                HOST,
                Ok("/home/user/projects"),
            ),
            // A reported FQDN whose first label is the local (short) name is a
            // different machine as far as we can tell, so it is rejected.
            (
                "file://mac.local/tmp",
                Some("mac"),
                Err(Osc7Rejection::ForeignHost),
            ),
            (
                "file://myhost.attacker.example/tmp",
                Some("myhost"),
                Err(Osc7Rejection::ForeignHost),
            ),
            // The local FQDN itself still matches.
            ("file://mac.local/tmp", Some("mac.local"), Ok("/tmp")),
            ("file://other/tmp", HOST, Err(Osc7Rejection::ForeignHost)),
            ("file://x/tmp", HOST, Err(Osc7Rejection::ForeignHost)),
            ("file://myhost/tmp", None, Err(Osc7Rejection::ForeignHost)),
            (
                "file://myhost/tmp",
                Some(""),
                Err(Osc7Rejection::ForeignHost),
            ),
            (
                "file://.evil/tmp",
                Some(""),
                Err(Osc7Rejection::ForeignHost),
            ),
            (
                "file://127.0.0.1/tmp",
                HOST,
                Err(Osc7Rejection::ForeignHost),
            ),
            // Scheme / structure.
            (
                "http://example.com/path",
                HOST,
                Err(Osc7Rejection::NotFileScheme),
            ),
            ("ftp://host/path", HOST, Err(Osc7Rejection::NotFileScheme)),
            ("", HOST, Err(Osc7Rejection::NotFileScheme)),
            ("file://", HOST, Err(Osc7Rejection::NoPath)),
            ("file://myhost", HOST, Err(Osc7Rejection::NoPath)),
            // Path validity after decoding.
            ("file:///a%00b", HOST, Err(Osc7Rejection::InvalidPath)),
            ("file:///a%0ab", HOST, Err(Osc7Rejection::InvalidPath)),
            ("file:///a%1bb", HOST, Err(Osc7Rejection::InvalidPath)),
            ("file:///a%c2%9bb", HOST, Err(Osc7Rejection::InvalidPath)),
            ("file:///a\u{7}b", HOST, Err(Osc7Rejection::InvalidPath)),
            ("file:///a%ffb", HOST, Err(Osc7Rejection::InvalidPath)),
            // Escaped slashes in the host are not path separators.
            (
                "file://localhost%2Ftmp/x",
                HOST,
                Err(Osc7Rejection::ForeignHost),
            ),
            // Valid decoding.
            ("file:///tmp/%41", HOST, Ok("/tmp/A")),
            (
                "file:///home/user/my%20project",
                HOST,
                Ok("/home/user/my project"),
            ),
            (
                "file:///home/user/dir%20with%20spaces/sub%2Fdir",
                HOST,
                Ok("/home/user/dir with spaces/sub/dir"),
            ),
            ("file:///Espa%C3%B1a", HOST, Ok("/España")),
        ];
        for (uri, host, expected) in cases {
            assert_eq!(
                parse_osc7_uri_styled(uri, *host, Osc7PathStyle::Posix),
                expected.map(str::to_owned),
                "uri={uri:?} host={host:?}"
            );
        }
    }

    #[test]
    fn parse_osc7_uri_uses_the_native_path_style() {
        let (uri, expected) = sample_osc7("/project");
        assert_eq!(parse_osc7_uri(&uri, None), Ok(expected));
    }

    #[test]
    fn parse_osc7_uri_windows_style_table() {
        let cases: &[(&str, Result<&str, Osc7Rejection>)] = &[
            ("file:///C:/Users/x", Ok("C:/Users/x")),
            ("file:///c:/x", Ok("c:/x")),
            ("file://localhost/D:/a%20b", Ok("D:/a b")),
            ("file:///C:/", Ok("C:/")),
            ("file:////host/share", Err(Osc7Rejection::InvalidPath)),
            (
                "file://localhost//host/share",
                Err(Osc7Rejection::InvalidPath),
            ),
            ("file:///tmp", Err(Osc7Rejection::InvalidPath)),
            ("file:///C:", Err(Osc7Rejection::InvalidPath)),
            ("file:///C:x", Err(Osc7Rejection::InvalidPath)),
            ("file:////%3F/C:/x", Err(Osc7Rejection::InvalidPath)),
            ("file:///C:/a%0ab", Err(Osc7Rejection::InvalidPath)),
            ("file://other/C:/x", Err(Osc7Rejection::ForeignHost)),
        ];
        for (uri, expected) in cases {
            assert_eq!(
                parse_osc7_uri_styled(uri, None, Osc7PathStyle::Windows),
                expected.map(str::to_owned),
                "uri={uri:?}"
            );
        }
    }

    #[test]
    fn parse_osc7_uri_posix_style_keeps_drive_like_paths_verbatim() {
        assert_eq!(
            parse_osc7_uri_styled("file:///C:/Users/x", None, Osc7PathStyle::Posix),
            Ok("/C:/Users/x".to_owned())
        );
        assert_eq!(
            parse_osc7_uri_styled("file:///tmp", None, Osc7PathStyle::Posix),
            Ok("/tmp".to_owned())
        );
    }

    #[test]
    fn native_style_matches_platform() {
        let expected = if cfg!(windows) {
            Osc7PathStyle::Windows
        } else {
            Osc7PathStyle::Posix
        };
        assert_eq!(Osc7PathStyle::native(), expected);
    }

    #[test]
    fn osc7_rejection_display_is_static_and_payload_free() {
        assert_eq!(
            Osc7Rejection::NotFileScheme.to_string(),
            "not a file:// URI"
        );
        assert_eq!(Osc7Rejection::NoPath.to_string(), "URI has no path");
        assert_eq!(
            Osc7Rejection::ForeignHost.to_string(),
            "URI names a host other than this machine"
        );
        assert_eq!(
            Osc7Rejection::InvalidPath.to_string(),
            "path is not a valid absolute path"
        );
    }

    #[test]
    fn local_hostname_is_stable_and_non_empty_when_present() {
        let first = local_hostname();
        assert_eq!(first, local_hostname());
        if let Some(h) = first {
            assert_ne!(h, "");
        }
    }

    #[test]
    fn percent_decode_no_encoding() {
        assert_eq!(percent_decode("/home/user").as_deref(), Some("/home/user"));
    }

    #[test]
    fn percent_decode_malformed_sequence() {
        // %ZZ is not valid hex — pass through verbatim
        assert_eq!(
            percent_decode("/path%ZZfoo").as_deref(),
            Some("/path%ZZfoo")
        );
    }

    #[test]
    fn percent_decode_truncated_at_end() {
        // % at end of string with not enough chars
        assert_eq!(percent_decode("/path%2").as_deref(), Some("/path%2"));
        assert_eq!(percent_decode("/path%").as_deref(), Some("/path%"));
    }

    #[test]
    fn percent_decode_trailing_complete_escape() {
        // A complete `%XX` as the very last three bytes must be decoded.
        assert_eq!(percent_decode("/path%41").as_deref(), Some("/pathA"));
        assert_eq!(percent_decode("%41").as_deref(), Some("A"));
    }

    #[test]
    fn percent_decode_invalid_utf8_is_rejected() {
        assert_eq!(percent_decode("/a%ffb"), None);
        assert_eq!(percent_decode("/a%c3"), None);
    }

    #[test]
    fn percent_decode_multibyte_utf8() {
        // € is U+20AC, encoded as UTF-8 bytes E2 82 AC
        assert_eq!(
            percent_decode("/cost%E2%82%AC100").as_deref(),
            Some("/cost€100")
        );
        // ñ is U+00F1, encoded as UTF-8 bytes C3 B1
        assert_eq!(percent_decode("/Espa%C3%B1a").as_deref(), Some("/España"));
        // 日 is U+65E5, encoded as UTF-8 bytes E6 97 A5
        assert_eq!(percent_decode("/%E6%97%A5").as_deref(), Some("/日"));
    }

    #[test]
    fn handle_osc_foreign_host_clears_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        let (uri, expected) = sample_osc7("");
        handler.handle_osc(&AnsiOscType::RemoteHost(uri));
        assert_eq!(handler.current_working_directory(), Some(expected.as_str()));
        handler.handle_osc(&AnsiOscType::RemoteHost(
            "file://definitely-not-this-host.invalid/home/attacker".to_string(),
        ));
        assert_eq!(handler.current_working_directory(), None);
    }

    #[test]
    fn handle_osc_nul_in_path_clears_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        let (uri, _) = sample_osc7("");
        handler.handle_osc(&AnsiOscType::RemoteHost(uri));
        let (bad_uri, _) = sample_osc7("%00/evil");
        handler.handle_osc(&AnsiOscType::RemoteHost(bad_uri));
        assert_eq!(handler.current_working_directory(), None);
    }

    #[test]
    fn handle_osc_remote_host_sets_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        let (uri, expected) = sample_osc7("");
        handler.handle_osc(&AnsiOscType::RemoteHost(uri));
        assert_eq!(handler.current_working_directory(), Some(expected.as_str()));
    }

    #[test]
    fn handle_osc_remote_host_updates_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        let (uri_a, expected_a) = sample_osc7("/a");
        handler.handle_osc(&AnsiOscType::RemoteHost(uri_a));
        assert_eq!(
            handler.current_working_directory(),
            Some(expected_a.as_str())
        );

        let (uri_b, expected_b) = sample_osc7("/b");
        handler.handle_osc(&AnsiOscType::RemoteHost(uri_b));
        assert_eq!(
            handler.current_working_directory(),
            Some(expected_b.as_str())
        );
    }

    #[test]
    fn handle_osc_remote_host_invalid_uri_clears_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        // First set a valid CWD
        let (uri, _) = sample_osc7("");
        handler.handle_osc(&AnsiOscType::RemoteHost(uri));
        assert!(handler.current_working_directory().is_some());

        // Now send an invalid URI — CWD should be cleared (set to None)
        handler.handle_osc(&AnsiOscType::RemoteHost("not-a-file-uri".to_string()));
        assert_eq!(handler.current_working_directory(), None);
    }

    #[test]
    fn full_reset_keeps_cwd() {
        // The OSC 7 working directory describes the process, not the screen
        // (maintainer decision, Task 131 reset table), so RIS keeps it.
        let mut handler = TerminalHandler::new(80, 24);
        let (uri, _) = sample_osc7("");
        handler.handle_osc(&AnsiOscType::RemoteHost(uri));
        let before = handler.current_working_directory().map(str::to_owned);
        assert!(before.is_some());

        handler.full_reset();
        assert_eq!(
            handler.current_working_directory().map(str::to_owned),
            before
        );
    }

    #[test]
    fn cwd_is_none_by_default() {
        let handler = TerminalHandler::new(80, 24);
        assert_eq!(handler.current_working_directory(), None);
    }

    // ── FTCS / OSC 133 tests ────────────────────────────────────────────

    #[test]
    fn ftcs_state_default_is_none() {
        let handler = TerminalHandler::new(80, 24);
        assert_eq!(handler.ftcs_state(), FtcsState::None);
        assert_eq!(handler.last_exit_code(), None);
    }

    #[test]
    fn ftcs_prompt_start_sets_in_prompt() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "test".to_owned(),
        }));
        assert_eq!(handler.ftcs_state(), FtcsState::InPrompt);
    }

    #[test]
    fn ftcs_command_start_sets_in_command() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandStart {
            fid: "test".to_owned(),
        }));
        assert_eq!(handler.ftcs_state(), FtcsState::InCommand);
    }

    #[test]
    fn ftcs_output_start_sets_in_output() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart {
            fid: "test".to_owned(),
        }));
        assert_eq!(handler.ftcs_state(), FtcsState::InOutput);
    }

    #[test]
    fn ftcs_command_finished_resets_to_none() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "test".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart {
            fid: "test".to_owned(),
        }));
        assert_eq!(handler.ftcs_state(), FtcsState::InOutput);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
            exit_code: Some(0),
            fid: "test".to_owned(),
        }));
        assert_eq!(handler.ftcs_state(), FtcsState::None);
    }

    #[test]
    fn ftcs_command_finished_captures_exit_code() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "test".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
            exit_code: Some(42),
            fid: "test".to_owned(),
        }));
        assert_eq!(handler.last_exit_code(), Some(42));
    }

    #[test]
    fn ftcs_command_finished_no_exit_code() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "test".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
            exit_code: None,
            fid: "test".to_owned(),
        }));
        assert_eq!(handler.last_exit_code(), None);
    }

    #[test]
    fn ftcs_full_cycle() {
        let mut handler = TerminalHandler::new(80, 24);

        // A → prompt start
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "cycle1".to_owned(),
        }));
        assert_eq!(handler.ftcs_state(), FtcsState::InPrompt);

        // B → command start
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandStart {
            fid: "cycle1".to_owned(),
        }));
        assert_eq!(handler.ftcs_state(), FtcsState::InCommand);

        // C → output start
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart {
            fid: "cycle1".to_owned(),
        }));
        assert_eq!(handler.ftcs_state(), FtcsState::InOutput);

        // D;0 → command finished with exit code 0
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
            exit_code: Some(0),
            fid: "cycle1".to_owned(),
        }));
        assert_eq!(handler.ftcs_state(), FtcsState::None);
        assert_eq!(handler.last_exit_code(), Some(0));
    }

    #[test]
    fn ftcs_exit_code_updated_on_each_d_marker() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "c1".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
            exit_code: Some(0),
            fid: "c1".to_owned(),
        }));
        assert_eq!(handler.last_exit_code(), Some(0));

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "c2".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
            exit_code: Some(127),
            fid: "c2".to_owned(),
        }));
        assert_eq!(handler.last_exit_code(), Some(127));

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "c3".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
            exit_code: None,
            fid: "c3".to_owned(),
        }));
        assert_eq!(handler.last_exit_code(), None);
    }

    #[test]
    fn full_reset_clears_ftcs_state() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "test".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart {
            fid: "test".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
            exit_code: Some(1),
            fid: "test".to_owned(),
        }));
        assert_eq!(handler.last_exit_code(), Some(1));

        handler.full_reset();
        assert_eq!(handler.ftcs_state(), FtcsState::None);
        assert_eq!(handler.last_exit_code(), None);
    }

    // -----------------------------------------------------------------------
    // hex_val — unit tests for the ASCII hex-digit converter
    // -----------------------------------------------------------------------

    #[test]
    fn hex_val_decimal_digits() {
        for (byte, expected) in (b'0'..=b'9').zip(0u8..=9u8) {
            assert_eq!(
                hex_val(byte),
                Some(expected),
                "hex_val({}) should be Some({})",
                byte as char,
                expected
            );
        }
    }

    #[test]
    fn hex_val_lowercase_hex_digits() {
        for (byte, expected) in (b'a'..=b'f').zip(10u8..=15u8) {
            assert_eq!(
                hex_val(byte),
                Some(expected),
                "hex_val({}) should be Some({})",
                byte as char,
                expected
            );
        }
    }

    #[test]
    fn hex_val_uppercase_hex_digits() {
        for (byte, expected) in (b'A'..=b'F').zip(10u8..=15u8) {
            assert_eq!(
                hex_val(byte),
                Some(expected),
                "hex_val({}) should be Some({})",
                byte as char,
                expected
            );
        }
    }

    #[test]
    fn hex_val_non_hex_chars_return_none() {
        for &byte in b"GZgz !/:@[`{" {
            assert_eq!(
                hex_val(byte),
                None,
                "hex_val({}) should be None",
                byte as char
            );
        }
    }

    #[test]
    fn hex_val_boundary_chars_just_outside_hex_range() {
        // b'0' - 1 = b'/' and b'9' + 1 = b':' should both be None
        assert_eq!(hex_val(b'/'), None);
        assert_eq!(hex_val(b':'), None);
        // b'A' - 1 = b'@' and b'F' + 1 = b'G' should both be None
        assert_eq!(hex_val(b'@'), None);
        assert_eq!(hex_val(b'G'), None);
        // b'a' - 1 = b'`' and b'f' + 1 = b'g' should both be None
        assert_eq!(hex_val(b'`'), None);
        assert_eq!(hex_val(b'g'), None);
    }

    // -----------------------------------------------------------------------
    // CommandBlock side-effect tests (OSC 133 → Buffer command-block API)
    // -----------------------------------------------------------------------

    /// Emit OSC 133 A and verify one Running block is created.
    #[test]
    fn prompt_start_creates_command_block() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "test".to_owned(),
        }));
        assert_eq!(handler.buffer().command_blocks().len(), 1);
        assert_eq!(
            handler.buffer().command_blocks()[0].status(),
            freminal_common::buffer_states::command_block::CommandStatus::Running
        );
    }

    /// OSC 7 before OSC 133 A should be captured in the new block's cwd.
    #[test]
    fn prompt_start_captures_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        let (uri, expected) = sample_osc7("");
        handler.handle_osc(&AnsiOscType::RemoteHost(uri));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "test".to_owned(),
        }));
        assert_eq!(
            handler.buffer().command_blocks()[0].cwd.as_deref(),
            Some(expected.as_str())
        );
    }

    /// OSC 133 A without prior OSC 7 should produce a block with no cwd.
    #[test]
    fn prompt_start_without_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "test".to_owned(),
        }));
        assert!(handler.buffer().command_blocks()[0].cwd.is_none());
    }

    /// Full A → B → C → D(0) cycle must produce one fully populated Success block.
    #[test]
    fn full_cycle_records_block() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "c1".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandStart {
            fid: "c1".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart {
            fid: "c1".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
            exit_code: Some(0),
            fid: "c1".to_owned(),
        }));

        assert_eq!(handler.buffer().command_blocks().len(), 1);
        let block = &handler.buffer().command_blocks()[0];
        assert_eq!(
            block.status(),
            freminal_common::buffer_states::command_block::CommandStatus::Success
        );
        assert_eq!(block.exit_code, Some(0));
        assert!(block.end_row.is_some(), "end_row should be set after D");
        assert!(
            block.output_start_row.is_some(),
            "output_start_row should be set after C"
        );
        assert!(
            block.command_start_row.is_some(),
            "command_start_row should be set after B"
        );

        // drain_command_events should return one block
        let events = handler.drain_command_events();
        assert_eq!(events.len(), 1);

        // Second drain returns empty
        let events2 = handler.drain_command_events();
        assert!(events2.is_empty());
    }

    /// Full cycle with non-zero exit code records Failure status.
    #[test]
    fn full_cycle_non_zero_exit_code() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "c1".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandStart {
            fid: "c1".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart {
            fid: "c1".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
            exit_code: Some(127),
            fid: "c1".to_owned(),
        }));

        let events = handler.drain_command_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].exit_code, Some(127));
        assert_eq!(
            events[0].status(),
            freminal_common::buffer_states::command_block::CommandStatus::Failure(127)
        );
    }

    /// A → D (no B or C): `exit_code` is None → `status()` is Unknown.
    #[test]
    fn command_finished_without_code() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "c1".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
            exit_code: None,
            fid: "c1".to_owned(),
        }));

        let events = handler.drain_command_events();
        assert_eq!(events.len(), 1);
        assert!(events[0].exit_code.is_none());
        assert_eq!(
            events[0].status(),
            freminal_common::buffer_states::command_block::CommandStatus::Unknown
        );
    }

    /// A("a") → A("b") → D("b"): two blocks; only "b" is finished; "a" still Running.
    #[test]
    fn interrupted_prompt_pushes_two_blocks() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "fid-a".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "fid-b".to_owned(),
        }));
        // Finish only the second block (fid-b)
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
            exit_code: Some(0),
            fid: "fid-b".to_owned(),
        }));

        assert_eq!(
            handler.buffer().command_blocks().len(),
            2,
            "two A markers should produce two blocks"
        );

        // Only one drained event (fid-b was finished)
        let events = handler.drain_command_events();
        assert_eq!(events.len(), 1, "only one D marker was received");

        // First block (fid-a) is still Running
        assert_eq!(
            handler.buffer().command_blocks()[0].status(),
            freminal_common::buffer_states::command_block::CommandStatus::Running
        );
    }

    /// OSC 133 P (`PromptProperty`) alone must not create a command block.
    #[test]
    fn prompt_property_does_not_create_block() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptProperty(
            PromptKind::Initial,
        )));
        assert!(
            handler.buffer().command_blocks().is_empty(),
            "PromptProperty alone must not create blocks"
        );
        assert!(
            handler.drain_command_events().is_empty(),
            "PromptProperty alone must not queue events"
        );
    }

    /// A → C → D(0) (skipping B): `command_start_row` is None but `output_start_row` and `end_row` are set.
    #[test]
    fn output_start_without_command_start_marks_only_output() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "c1".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart {
            fid: "c1".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
            exit_code: Some(0),
            fid: "c1".to_owned(),
        }));

        assert_eq!(handler.buffer().command_blocks().len(), 1);
        let block = &handler.buffer().command_blocks()[0];
        assert!(
            block.command_start_row.is_none(),
            "B was never sent so command_start_row must be None"
        );
        assert!(
            block.output_start_row.is_some(),
            "C was sent so output_start_row must be set"
        );
        assert!(block.end_row.is_some(), "D was sent so end_row must be set");

        let events = handler.drain_command_events();
        assert_eq!(events.len(), 1, "one finished event expected");
    }

    /// Three A→D cycles with distinct fids; drain returns exit codes in order [0, 1, 2].
    #[test]
    fn drain_command_events_returns_oldest_first() {
        let mut handler = TerminalHandler::new(80, 24);

        for code in [0i32, 1, 2] {
            let fid = format!("cycle-{code}");
            handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
                fid: fid.clone(),
            }));
            handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
                exit_code: Some(code),
                fid,
            }));
        }

        let events = handler.drain_command_events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].exit_code, Some(0));
        assert_eq!(events[1].exit_code, Some(1));
        assert_eq!(events[2].exit_code, Some(2));
    }

    // ── New tests: foreign-marker rejection ─────────────────────────────

    /// Markers without `freminal=1` must produce no state change and no blocks.
    /// The handler only receives markers that passed [`parse_ftcs_params`], so
    /// this tests that [`parse_ftcs_params`] correctly rejects them.
    #[test]
    fn foreign_marker_rejection_is_handled_at_parse_layer() {
        // WezTerm-style: A without freminal=1 → parse returns None → handler never called
        use freminal_common::buffer_states::ftcs::parse_ftcs_params;
        assert_eq!(parse_ftcs_params(b"A;aid=12345"), None);
        assert_eq!(parse_ftcs_params(b"D;0;aid=12345"), None);
        // Plain markers (old style)
        assert_eq!(parse_ftcs_params(b"A"), None);
        assert_eq!(parse_ftcs_params(b"D;0"), None);
    }

    /// Completing a block by fid only closes the correct block.
    #[test]
    fn finish_by_correct_fid_only() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "first".to_owned(),
        }));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart {
            fid: "second".to_owned(),
        }));

        // Finish "first" only.
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished {
            exit_code: Some(0),
            fid: "first".to_owned(),
        }));

        let events = handler.drain_command_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].fid, "first");

        // "second" is still running.
        assert_eq!(
            handler.buffer().command_blocks()[1].status(),
            freminal_common::buffer_states::command_block::CommandStatus::Running
        );
    }
}
