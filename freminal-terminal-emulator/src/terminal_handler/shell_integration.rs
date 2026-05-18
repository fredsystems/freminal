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
    String::from_utf8_lossy(&output).into_owned()
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
        ftcs::{FtcsMarker, FtcsState, PromptKind},
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
    fn percent_decode_multibyte_utf8() {
        // € is U+20AC, encoded as UTF-8 bytes E2 82 AC
        assert_eq!(percent_decode("/cost%E2%82%AC100"), "/cost€100");
        // ñ is U+00F1, encoded as UTF-8 bytes C3 B1
        assert_eq!(percent_decode("/Espa%C3%B1a"), "/España");
        // 日 is U+65E5, encoded as UTF-8 bytes E6 97 A5
        assert_eq!(percent_decode("/%E6%97%A5"), "/日");
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
        for byte in [
            b'G', b'Z', b'g', b'z', b' ', b'!', b'/', b':', b'@', b'[', b'`', b'{',
        ] {
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
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
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
        handler.handle_osc(&AnsiOscType::RemoteHost("file:///home/user".to_string()));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
        assert_eq!(
            handler.buffer().command_blocks()[0].cwd.as_deref(),
            Some("/home/user")
        );
    }

    /// OSC 133 A without prior OSC 7 should produce a block with no cwd.
    #[test]
    fn prompt_start_without_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
        assert!(handler.buffer().command_blocks()[0].cwd.is_none());
    }

    /// Full A → B → C → D(0) cycle must produce one fully populated Success block.
    #[test]
    fn full_cycle_records_block() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandStart));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(0))));

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

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandStart));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(127))));

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

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(None)));

        let events = handler.drain_command_events();
        assert_eq!(events.len(), 1);
        assert!(events[0].exit_code.is_none());
        assert_eq!(
            events[0].status(),
            freminal_common::buffer_states::command_block::CommandStatus::Unknown
        );
    }

    /// A → A → D(0): two blocks exist; only the second is finished; first is still Running.
    #[test]
    fn interrupted_prompt_pushes_two_blocks() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(0))));

        assert_eq!(
            handler.buffer().command_blocks().len(),
            2,
            "two A markers should produce two blocks"
        );

        // Only one drained event (the second block was finished)
        let events = handler.drain_command_events();
        assert_eq!(events.len(), 1, "only one D marker was received");

        // First block is still Running
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

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(0))));

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

    /// Three A→D cycles; drain returns exit codes in order [0, 1, 2].
    #[test]
    fn drain_command_events_returns_oldest_first() {
        let mut handler = TerminalHandler::new(80, 24);

        for code in [0i32, 1, 2] {
            handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
            handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(code))));
        }

        let events = handler.drain_command_events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].exit_code, Some(0));
        assert_eq!(events[1].exit_code, Some(1));
        assert_eq!(events[2].exit_code, Some(2));
    }

    // -----------------------------------------------------------------------
    // parse_osc7_uri additional edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn parse_osc7_uri_with_percent_encoded_file_url() {
        // Explicit test for the case mentioned in the task description
        let result = parse_osc7_uri("file:///home/user/my%20file.txt");
        assert_eq!(result, Some("/home/user/my file.txt".to_string()));
    }

    #[test]
    fn parse_osc7_uri_file_scheme_with_empty_rest() {
        // "file://" → rest is empty → no slash found for hostname path
        // rest.starts_with('/') = false, find('/') = None → returns None
        assert_eq!(parse_osc7_uri("file://"), None);
    }
}
