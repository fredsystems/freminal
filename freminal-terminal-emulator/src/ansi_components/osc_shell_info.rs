// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! OSC 1338 — freminal-private shell-information sub-protocol parser.
//!
//! Emitted by the bundled shell-integration scripts at session start to
//! convey out-of-band metadata that freminal cannot reliably observe from
//! the parent process environment.  The first (and currently only)
//! sub-command is `HISTFILE=<path>`, which lets the command-history
//! palette read the *correct* history file when `$HISTFILE` was set as a
//! non-exported shell variable in `.zshrc` / `.bashrc` and therefore
//! invisible to freminal at PTY spawn time.
//!
//! Wire format:
//!
//! ```text
//! OSC 1338 ; HISTFILE = <path> ST
//! ```
//!
//! The path is sent raw (no base64).  Paths containing `;` / BEL / ESC
//! are exceedingly rare; the parser bails silently on malformed input.
//! The OSC parser upstream has already validated byte ranges and stripped
//! the terminator, so `raw_params` contains only the printable
//! `1338;HISTFILE=<path>` bytes.

use crate::ansi_components::tracer::SequenceTracer;
use freminal_common::buffer_states::osc::AnsiOscType;
use freminal_common::buffer_states::terminal_output::TerminalOutput;
use std::path::PathBuf;

/// Handle OSC 1338 (freminal shell-info sub-protocol).
///
/// `raw_params` is the full, un-split OSC parameter bytes (before `;`
/// splitting upstream).  We parse from the raw bytes because the sub-
/// command may contain its own `=` (and could one day contain encoded
/// `;`), so the upstream semicolon split is too aggressive.
pub(super) fn handle_osc_shell_info(
    raw_params: &[u8],
    seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    // raw_params looks like: b"1338;HISTFILE=/home/u/.zsh_history"
    //
    // Find the first ';' to skip past "1338".
    let Some(first_semi) = raw_params.iter().position(|&b| b == b';') else {
        tracing::warn!(
            "OSC 1338: missing sub-command: recent='{}'",
            seq_trace.as_str()
        );
        return;
    };

    let rest = &raw_params[first_semi + 1..];

    // HISTFILE= sub-command (case-sensitive).
    if let Some(after_key) = strip_ascii_prefix(rest, b"HISTFILE=") {
        handle_shell_info_histfile(after_key, seq_trace, output);
        return;
    }

    // Unknown sub-command — silently consume.  Forwards-compatible:
    // future sub-commands added to newer shell-integration scripts will
    // be ignored by older freminal binaries without producing a parse
    // error.
    tracing::debug!(
        "OSC 1338: unrecognised sub-command (ignored): recent='{}'",
        seq_trace.as_str()
    );
}

/// Parse the value bytes of `HISTFILE=<path>` and emit the corresponding
/// `AnsiOscType::ShellInfoHistFile`.
fn handle_shell_info_histfile(
    value: &[u8],
    seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    if value.is_empty() {
        tracing::warn!(
            "OSC 1338 HISTFILE=: empty path: recent='{}'",
            seq_trace.as_str()
        );
        return;
    }

    // Paths from the shell are typically UTF-8 (Linux/macOS).  On
    // exotic filesystems they may be arbitrary bytes; we drop the
    // message in that case rather than lossy-decoding, because the
    // U+FFFD replacement bytes would produce a PathBuf that no longer
    // round-trips to an openable file on disk.  The env-derived
    // fallback path in the GUI loader already covers the unhappy path.
    let Ok(s) = std::str::from_utf8(value) else {
        tracing::warn!(
            "OSC 1338 HISTFILE=: non-UTF-8 path: recent='{}'",
            seq_trace.as_str()
        );
        return;
    };

    output.push(TerminalOutput::OscResponse(AnsiOscType::ShellInfoHistFile(
        PathBuf::from(s),
    )));
}

/// Strip an ASCII prefix from a byte slice, returning the remainder.
fn strip_ascii_prefix<'a>(haystack: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    if haystack.len() >= prefix.len() && &haystack[..prefix.len()] == prefix {
        Some(&haystack[prefix.len()..])
    } else {
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::osc::AnsiOscParser;
    use super::super::tracer::SequenceTracer;
    use freminal_common::buffer_states::osc::AnsiOscType;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;
    use std::path::PathBuf;

    fn feed_osc(payload: &[u8]) -> Vec<TerminalOutput> {
        let mut parser = AnsiOscParser::new();
        let mut output = Vec::new();
        for &b in payload {
            parser.ansiparser_inner_osc(b, &mut output);
        }
        output
    }

    fn tracer() -> SequenceTracer {
        SequenceTracer::new()
    }

    // ── End-to-end OSC 1338 HISTFILE=... parsing ─────────────────────────

    #[test]
    fn osc1338_histfile_basic_bel() {
        let payload = b"1338;HISTFILE=/home/user/.zsh_history\x07";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ShellInfoHistFile(path)) => {
                assert_eq!(path, &PathBuf::from("/home/user/.zsh_history"));
            }
            other => panic!("Expected ShellInfoHistFile, got: {other:?}"),
        }
    }

    #[test]
    fn osc1338_histfile_basic_st_terminator() {
        let payload = b"1338;HISTFILE=/var/log/hist\x1b\\";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ShellInfoHistFile(path)) => {
                assert_eq!(path, &PathBuf::from("/var/log/hist"));
            }
            other => panic!("Expected ShellInfoHistFile, got: {other:?}"),
        }
    }

    #[test]
    fn osc1338_histfile_path_with_spaces() {
        let payload = b"1338;HISTFILE=/home/My Documents/.bash_history\x07";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ShellInfoHistFile(path)) => {
                assert_eq!(path, &PathBuf::from("/home/My Documents/.bash_history"));
            }
            other => panic!("Expected ShellInfoHistFile, got: {other:?}"),
        }
    }

    #[test]
    fn osc1338_histfile_empty_path_no_output() {
        let payload = b"1338;HISTFILE=\x07";
        let output = feed_osc(payload);
        assert!(output.is_empty(), "got: {output:?}");
    }

    #[test]
    fn osc1338_missing_sub_command_no_output() {
        // No `;` after the 1338 → no sub-command to parse.
        // The upstream parser will split on `;` and produce a single
        // `1338` token; `OscTarget::ShellInfo` then dispatches but our
        // raw_params arm finds no semicolon and bails.
        let payload = b"1338\x07";
        let output = feed_osc(payload);
        // Empty output is the expected silent-consume behaviour.
        assert!(output.is_empty(), "got: {output:?}");
    }

    #[test]
    fn osc1338_unrecognised_subcommand_silent_consume() {
        // Future sub-command we don't yet support — must not produce
        // ShellInfoHistFile and must not error.
        let payload = b"1338;FUTURE_KEY=somevalue\x07";
        let output = feed_osc(payload);
        // No ShellInfoHistFile emitted; the OSC is silently consumed.
        for item in &output {
            assert!(
                !matches!(
                    item,
                    TerminalOutput::OscResponse(AnsiOscType::ShellInfoHistFile(_))
                ),
                "unexpected ShellInfoHistFile: {item:?}"
            );
        }
    }

    // ── Direct-call coverage for error branches ─────────────────────────

    #[test]
    fn shell_info_missing_semicolon_direct_call() {
        let mut output = Vec::new();
        super::handle_osc_shell_info(b"1338HISTFILE=/x", &tracer(), &mut output);
        assert!(output.is_empty());
    }

    #[test]
    fn shell_info_unrecognised_subcommand_direct_call() {
        let mut output = Vec::new();
        super::handle_osc_shell_info(b"1338;UNKNOWN=bar", &tracer(), &mut output);
        assert!(output.is_empty());
    }

    #[test]
    fn shell_info_histfile_non_utf8_direct_call() {
        let mut output = Vec::new();
        // Build "1338;HISTFILE=" + invalid UTF-8 bytes.
        let mut raw = b"1338;HISTFILE=".to_vec();
        raw.extend_from_slice(&[0xFF, 0xFE, 0xFD]);
        super::handle_osc_shell_info(&raw, &tracer(), &mut output);
        assert!(output.is_empty());
    }

    #[test]
    fn shell_info_histfile_empty_value_direct_call() {
        let mut output = Vec::new();
        super::handle_shell_info_histfile(b"", &tracer(), &mut output);
        assert!(output.is_empty());
    }
}
