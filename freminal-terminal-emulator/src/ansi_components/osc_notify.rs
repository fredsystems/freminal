// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! OSC 9 / OSC 777 — desktop notification parsers (Task 76).
//!
//! Both sequences are one-way, fire-and-forget notification requests. They
//! produce an [`AnsiOscType::Notify`] which the GUI routes to an in-app
//! toast and/or the system notification daemon per the `[notifications]`
//! config.
//!
//! Wire formats:
//!
//! ```text
//! OSC 9 ; <body>                       ST   (iTerm2 / WezTerm)
//! OSC 777 ; notify ; <title> ; <body>  ST   (urxvt)
//! ```
//!
//! We parse from the raw (un-split) parameter bytes because a notification
//! body may legitimately contain `;` characters, and the upstream
//! semicolon split is too aggressive for free-form text. The OSC parser
//! upstream has already validated byte ranges and stripped the terminator,
//! so `raw_params` contains only the printable payload bytes (e.g.
//! `9;Build finished` or `777;notify;Title;Body`).

use crate::ansi_components::tracer::SequenceTracer;
use freminal_common::buffer_states::osc::AnsiOscType;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Handle OSC 9 (`iTerm2` / `WezTerm` notification).
///
/// The entire payload after the leading `9;` is the notification body;
/// there is no separate title. An empty body is silently consumed (nothing
/// useful to display).
pub(super) fn handle_osc_notify_9(
    raw_params: &[u8],
    seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    // raw_params looks like: b"9;Build finished"
    let Some(first_semi) = raw_params.iter().position(|&b| b == b';') else {
        // `OSC 9 ST` with no body — nothing to notify.
        tracing::debug!(
            "OSC 9: missing notification body: recent='{}'",
            seq_trace.as_str()
        );
        return;
    };

    let body_bytes = &raw_params[first_semi + 1..];
    let Some(body) = decode_utf8(body_bytes, seq_trace) else {
        return;
    };

    if body.is_empty() {
        tracing::debug!(
            "OSC 9: empty notification body: recent='{}'",
            seq_trace.as_str()
        );
        return;
    }

    output.push(TerminalOutput::OscResponse(AnsiOscType::Notify {
        title: None,
        body,
    }));
}

/// Handle OSC 777 (urxvt notification).
///
/// Canonical form is `777;notify;TITLE;BODY`. The `notify;` prefix is
/// consumed when present. The remainder is split on the first `;` into
/// title and body:
///
/// - `notify;TITLE;BODY` → title = `TITLE`, body = `BODY`.
/// - `notify;TITLE`      → title = `TITLE`, body = `""` (empty).
/// - payload without the `notify;` prefix → the entire payload (after the
///   leading `777;`) is the body, with no title.
///
/// An entirely empty payload is silently consumed.
pub(super) fn handle_osc_notify_777(
    raw_params: &[u8],
    seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    // raw_params looks like: b"777;notify;Title;Body"
    let Some(first_semi) = raw_params.iter().position(|&b| b == b';') else {
        tracing::debug!("OSC 777: missing payload: recent='{}'", seq_trace.as_str());
        return;
    };

    let rest = &raw_params[first_semi + 1..];
    let Some(rest) = decode_utf8(rest, seq_trace) else {
        return;
    };

    let (title, body) = parse_777_payload(&rest);

    // Nothing to show if both fields are empty.
    if title.as_deref().is_none_or(str::is_empty) && body.is_empty() {
        tracing::debug!(
            "OSC 777: empty notification: recent='{}'",
            seq_trace.as_str()
        );
        return;
    }

    output.push(TerminalOutput::OscResponse(AnsiOscType::Notify {
        title,
        body,
    }));
}

/// Split an OSC 777 payload (already stripped of the leading `777;`) into an
/// optional title and a body.
fn parse_777_payload(payload: &str) -> (Option<String>, String) {
    // Strip the urxvt `notify;` sub-command prefix when present.
    payload.strip_prefix("notify;").map_or_else(
        || {
            // No `notify;` prefix: treat the whole payload as the body.
            (None, payload.to_owned())
        },
        |after| {
            // `after` is `TITLE` or `TITLE;BODY`.  Split on the first `;`
            // only, so a body containing further semicolons is preserved.
            after.split_once(';').map_or_else(
                || (Some(after.to_owned()), String::new()),
                |(title, body)| (Some(title.to_owned()), body.to_owned()),
            )
        },
    )
}

/// Decode notification payload bytes as UTF-8, logging and bailing on
/// invalid input rather than lossy-decoding (which would corrupt the
/// displayed text).
fn decode_utf8(bytes: &[u8], seq_trace: &SequenceTracer) -> Option<String> {
    std::str::from_utf8(bytes).map_or_else(
        |_| {
            tracing::debug!(
                "OSC notification: non-UTF-8 payload (ignored): recent='{}'",
                seq_trace.as_str()
            );
            None
        },
        |s| Some(s.to_owned()),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::super::osc::AnsiOscParser;
    use super::super::tracer::SequenceTracer;
    use freminal_common::buffer_states::osc::AnsiOscType;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

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

    fn expect_notify(output: &[TerminalOutput]) -> (&Option<String>, &str) {
        assert_eq!(output.len(), 1, "expected one output, got: {output:?}");
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::Notify { title, body }) => {
                (title, body.as_str())
            }
            other => panic!("expected Notify, got: {other:?}"),
        }
    }

    // ── OSC 9 ────────────────────────────────────────────────────────────

    #[test]
    fn osc9_basic_body_bel() {
        let output = feed_osc(b"9;Build finished\x07");
        let (title, body) = expect_notify(&output);
        assert_eq!(*title, None);
        assert_eq!(body, "Build finished");
    }

    #[test]
    fn osc9_basic_body_st_terminator() {
        let output = feed_osc(b"9;Done\x1b\\");
        let (title, body) = expect_notify(&output);
        assert_eq!(*title, None);
        assert_eq!(body, "Done");
    }

    #[test]
    fn osc9_body_with_semicolons_preserved() {
        // A body containing semicolons must survive intact (no token split).
        let output = feed_osc(b"9;a;b;c\x07");
        let (title, body) = expect_notify(&output);
        assert_eq!(*title, None);
        assert_eq!(body, "a;b;c");
    }

    #[test]
    fn osc9_empty_body_no_output() {
        let output = feed_osc(b"9;\x07");
        assert!(output.is_empty(), "got: {output:?}");
    }

    #[test]
    fn osc9_missing_body_no_output() {
        // `OSC 9 ST` with no semicolon — the upstream parser produces a single
        // `9` token; dispatch reaches us but raw_params has no `;`.
        let output = feed_osc(b"9\x07");
        assert!(output.is_empty(), "got: {output:?}");
    }

    // ── OSC 777 ──────────────────────────────────────────────────────────

    #[test]
    fn osc777_notify_title_body() {
        let output = feed_osc(b"777;notify;Title;Body text\x07");
        let (title, body) = expect_notify(&output);
        assert_eq!(title.as_deref(), Some("Title"));
        assert_eq!(body, "Body text");
    }

    #[test]
    fn osc777_notify_title_only() {
        let output = feed_osc(b"777;notify;Just a title\x07");
        let (title, body) = expect_notify(&output);
        assert_eq!(title.as_deref(), Some("Just a title"));
        assert_eq!(body, "");
    }

    #[test]
    fn osc777_body_with_semicolons_preserved() {
        let output = feed_osc(b"777;notify;T;a;b;c\x07");
        let (title, body) = expect_notify(&output);
        assert_eq!(title.as_deref(), Some("T"));
        assert_eq!(body, "a;b;c");
    }

    #[test]
    fn osc777_without_notify_prefix_is_all_body() {
        // No `notify;` prefix → entire payload is the body, no title.
        let output = feed_osc(b"777;raw message\x07");
        let (title, body) = expect_notify(&output);
        assert_eq!(*title, None);
        assert_eq!(body, "raw message");
    }

    #[test]
    fn osc777_st_terminator() {
        let output = feed_osc(b"777;notify;T;B\x1b\\");
        let (title, body) = expect_notify(&output);
        assert_eq!(title.as_deref(), Some("T"));
        assert_eq!(body, "B");
    }

    #[test]
    fn osc777_empty_notify_no_output() {
        // `notify;` with nothing after it → empty title, empty body → consumed.
        let output = feed_osc(b"777;notify;\x07");
        assert!(output.is_empty(), "got: {output:?}");
    }

    #[test]
    fn osc777_missing_payload_no_output() {
        let output = feed_osc(b"777\x07");
        assert!(output.is_empty(), "got: {output:?}");
    }

    // ── Direct-call coverage for error branches ──────────────────────────

    #[test]
    fn notify9_non_utf8_direct_call() {
        let mut output = Vec::new();
        let mut raw = b"9;".to_vec();
        raw.extend_from_slice(&[0xFF, 0xFE, 0xFD]);
        super::handle_osc_notify_9(&raw, &tracer(), &mut output);
        assert!(output.is_empty());
    }

    #[test]
    fn notify9_missing_semicolon_direct_call() {
        let mut output = Vec::new();
        super::handle_osc_notify_9(b"9", &tracer(), &mut output);
        assert!(output.is_empty());
    }

    #[test]
    fn notify777_non_utf8_direct_call() {
        let mut output = Vec::new();
        let mut raw = b"777;".to_vec();
        raw.extend_from_slice(&[0xFF, 0xFE, 0xFD]);
        super::handle_osc_notify_777(&raw, &tracer(), &mut output);
        assert!(output.is_empty());
    }

    #[test]
    fn notify777_missing_semicolon_direct_call() {
        let mut output = Vec::new();
        super::handle_osc_notify_777(b"777", &tracer(), &mut output);
        assert!(output.is_empty());
    }

    #[test]
    fn parse_777_payload_variants() {
        assert_eq!(
            super::parse_777_payload("notify;T;B"),
            (Some("T".to_owned()), "B".to_owned())
        );
        assert_eq!(
            super::parse_777_payload("notify;T"),
            (Some("T".to_owned()), String::new())
        );
        assert_eq!(
            super::parse_777_payload("raw body"),
            (None, "raw body".to_owned())
        );
    }
}
