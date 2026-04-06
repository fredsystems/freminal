// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Shell integration helpers for [`TerminalHandler`].
//!
//! This module contains the OSC 7 CWD-tracking URI parser and its
//! supporting utilities:
//!
//! - [`parse_osc7_uri`] — parse `file://hostname/path` URIs emitted by OSC 7
//! - [`percent_decode`] — URL percent-decode a string
//! - [`hex_val`] — convert a single ASCII hex digit to its numeric value

/// Parse an OSC 7 URI of the form `file://hostname/path` and return the path
/// component.
///
/// The hostname is intentionally ignored — it is only meaningful for network
/// file-systems and most shells send `localhost` or the local hostname.
///
/// Percent-encoded bytes (e.g. `%20` for space) are decoded so the returned
/// path is a normal filesystem path string.
///
/// Returns `None` when the URI does not start with `file://` or has no path.
pub(super) fn parse_osc7_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;

    // The path starts at the first '/' after the hostname.
    // `file:///path` (empty hostname) → rest = "/path"
    // `file://hostname/path`          → rest = "hostname/path"
    let path = if rest.starts_with('/') {
        rest
    } else {
        let slash_pos = rest.find('/')?;
        &rest[slash_pos..]
    };

    if path.is_empty() {
        return None;
    }

    Some(percent_decode(path))
}

/// Decode percent-encoded bytes (`%XX`) in a string.
///
/// Only valid two-hex-digit sequences are decoded; malformed sequences are
/// passed through verbatim.
fn percent_decode(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            output.push(char::from(hi << 4 | lo));
            i += 3;
            continue;
        }
        output.push(char::from(bytes[i]));
        i += 1;
    }
    output
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use freminal_common::buffer_states::{
        ftcs::{FtcsMarker, FtcsState},
        osc::AnsiOscType,
    };

    use super::*;
    use crate::terminal_handler::TerminalHandler;

    // ------------------------------------------------------------------
    // OSC 7 CWD tracking tests
    // ------------------------------------------------------------------

    #[test]
    fn parse_osc7_uri_with_hostname() {
        let result = parse_osc7_uri("file://myhost/home/user/projects");
        assert_eq!(result, Some("/home/user/projects".to_string()));
    }

    #[test]
    fn parse_osc7_uri_empty_hostname() {
        // file:///path — empty hostname (common on macOS)
        let result = parse_osc7_uri("file:///home/user/projects");
        assert_eq!(result, Some("/home/user/projects".to_string()));
    }

    #[test]
    fn parse_osc7_uri_localhost() {
        let result = parse_osc7_uri("file://localhost/tmp");
        assert_eq!(result, Some("/tmp".to_string()));
    }

    #[test]
    fn parse_osc7_uri_percent_encoded_space() {
        let result = parse_osc7_uri("file:///home/user/my%20project");
        assert_eq!(result, Some("/home/user/my project".to_string()));
    }

    #[test]
    fn parse_osc7_uri_multiple_percent_encodings() {
        let result = parse_osc7_uri("file:///home/user/dir%20with%20spaces/sub%2Fdir");
        assert_eq!(
            result,
            Some("/home/user/dir with spaces/sub/dir".to_string())
        );
    }

    #[test]
    fn parse_osc7_uri_not_file_scheme() {
        assert_eq!(parse_osc7_uri("http://example.com/path"), None);
        assert_eq!(parse_osc7_uri("https://example.com/path"), None);
        assert_eq!(parse_osc7_uri("ftp://host/path"), None);
    }

    #[test]
    fn parse_osc7_uri_no_path_after_hostname() {
        // "file://hostname" with no trailing slash — no path
        assert_eq!(parse_osc7_uri("file://hostname"), None);
    }

    #[test]
    fn parse_osc7_uri_empty_string() {
        assert_eq!(parse_osc7_uri(""), None);
    }

    #[test]
    fn parse_osc7_uri_just_file_scheme() {
        assert_eq!(parse_osc7_uri("file://"), None);
    }

    #[test]
    fn percent_decode_no_encoding() {
        assert_eq!(percent_decode("/home/user"), "/home/user");
    }

    #[test]
    fn percent_decode_malformed_sequence() {
        // %ZZ is not valid hex — pass through verbatim
        assert_eq!(percent_decode("/path%ZZfoo"), "/path%ZZfoo");
    }

    #[test]
    fn percent_decode_truncated_at_end() {
        // % at end of string with not enough chars
        assert_eq!(percent_decode("/path%2"), "/path%2");
        assert_eq!(percent_decode("/path%"), "/path%");
    }

    #[test]
    fn handle_osc_remote_host_sets_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::RemoteHost(
            "file://localhost/home/user".to_string(),
        ));
        assert_eq!(handler.current_working_directory(), Some("/home/user"));
    }

    #[test]
    fn handle_osc_remote_host_updates_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::RemoteHost(
            "file://localhost/home/user/a".to_string(),
        ));
        assert_eq!(handler.current_working_directory(), Some("/home/user/a"));

        handler.handle_osc(&AnsiOscType::RemoteHost(
            "file://localhost/home/user/b".to_string(),
        ));
        assert_eq!(handler.current_working_directory(), Some("/home/user/b"));
    }

    #[test]
    fn handle_osc_remote_host_invalid_uri_clears_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        // First set a valid CWD
        handler.handle_osc(&AnsiOscType::RemoteHost(
            "file://localhost/home/user".to_string(),
        ));
        assert!(handler.current_working_directory().is_some());

        // Now send an invalid URI — CWD should be cleared (set to None)
        handler.handle_osc(&AnsiOscType::RemoteHost("not-a-file-uri".to_string()));
        assert_eq!(handler.current_working_directory(), None);
    }

    #[test]
    fn full_reset_clears_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::RemoteHost(
            "file://localhost/home/user".to_string(),
        ));
        assert!(handler.current_working_directory().is_some());

        handler.full_reset();
        assert_eq!(handler.current_working_directory(), None);
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
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InPrompt);
    }

    #[test]
    fn ftcs_command_start_sets_in_command() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InCommand);
    }

    #[test]
    fn ftcs_output_start_sets_in_output() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InOutput);
    }

    #[test]
    fn ftcs_command_finished_resets_to_none() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InOutput);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(0))));
        assert_eq!(handler.ftcs_state(), FtcsState::None);
    }

    #[test]
    fn ftcs_command_finished_captures_exit_code() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(42))));
        assert_eq!(handler.last_exit_code(), Some(42));
    }

    #[test]
    fn ftcs_command_finished_no_exit_code() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(None)));
        assert_eq!(handler.last_exit_code(), None);
    }

    #[test]
    fn ftcs_full_cycle() {
        let mut handler = TerminalHandler::new(80, 24);

        // A → prompt start
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InPrompt);

        // B → command start
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InCommand);

        // C → output start
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InOutput);

        // D;0 → command finished with exit code 0
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(0))));
        assert_eq!(handler.ftcs_state(), FtcsState::None);
        assert_eq!(handler.last_exit_code(), Some(0));
    }

    #[test]
    fn ftcs_exit_code_updated_on_each_d_marker() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(0))));
        assert_eq!(handler.last_exit_code(), Some(0));

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(127))));
        assert_eq!(handler.last_exit_code(), Some(127));

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(None)));
        assert_eq!(handler.last_exit_code(), None);
    }

    #[test]
    fn full_reset_clears_ftcs_state() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(1))));
        assert_eq!(handler.last_exit_code(), Some(1));

        handler.full_reset();
        assert_eq!(handler.ftcs_state(), FtcsState::None);
        assert_eq!(handler.last_exit_code(), None);
    }
}
