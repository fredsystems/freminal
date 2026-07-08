// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! OSC 9 / OSC 777 / OSC 99 — desktop notification parsers (Tasks 76, 99).
//!
//! OSC 9 and OSC 777 are one-way, fire-and-forget notification requests. They
//! produce an [`AnsiOscType::Notify`] which the GUI routes to an in-app
//! toast and/or the system notification daemon per the `[notifications]`
//! config.
//!
//! OSC 99 is the kitty stateful notification protocol.  It produces an
//! [`AnsiOscType::Notify99`] carrying a fully-parsed
//! [`freminal_common::buffer_states::osc_notify_99::Osc99Command`].
//! Chunk reassembly, transport to the GUI, and reverse-write are handled
//! downstream (Tasks 99.3+).
//!
//! Wire formats:
//!
//! ```text
//! OSC 9 ; <body>                                        ST   (iTerm2 / WezTerm)
//! OSC 777 ; notify ; <title> ; <body>                   ST   (urxvt)
//! OSC 99  ; <colon-sep key=value metadata> ; <payload>  ST   (kitty)
//! ```
//!
//! We parse all three from the raw (un-split) parameter bytes because
//! notification bodies and payloads may legitimately contain `;` characters,
//! and the upstream semicolon split is too aggressive for free-form text. The
//! OSC parser upstream has already validated byte ranges and stripped the
//! terminator, so `raw_params` contains only the printable payload bytes (e.g.
//! `9;Build finished`, `777;notify;Title;Body`, or
//! `99;i=x:p=title;Hello world`).

use crate::ansi_components::tracer::SequenceTracer;
use freminal_common::buffer_states::osc::AnsiOscType;
use freminal_common::buffer_states::osc_notify_99::parse_osc_99;
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

/// Handle OSC 99 (kitty stateful desktop notification).
///
/// Wire form: `ESC ] 99 ; <metadata> ; <payload> ST`.
///
/// `raw_params` is the full printable byte sequence with the OSC framing and
/// terminator already stripped, e.g. `b"99;i=x:p=title;Hello world"`.
///
/// Splitting rules (parse-path only — no reassembly):
///
/// 1. Find the **first** `;`.  Everything before it is the leading number
///    (`99`) which is discarded.  If there is no `;` at all the sequence is
///    malformed and is silently consumed (like `handle_osc_notify_9`).
/// 2. In the remainder, find the **next** `;` (the second overall).  Bytes
///    before it are the `<metadata>` region; bytes after it are the
///    `<payload>` region.  Only the second `;` is used as a split point — a
///    payload may legitimately contain further `;` characters.
/// 3. If the remainder has **no** second `;`, the entire remainder is treated
///    as `<metadata>` and the payload is empty (`&[]`).
///
/// The metadata and payload byte slices are then forwarded to
/// [`parse_osc_99`] which returns a typed [`Osc99Command`].  On success an
/// [`AnsiOscType::Notify99`] is appended to `output`; on error the failure
/// is logged at `debug!` level and the function returns without output.
pub(super) fn handle_osc_notify_99(
    raw_params: &[u8],
    // Intentionally unused: OSC 99 payloads can carry notification text,
    // icon data, and other application metadata, so the raw sequence trace
    // must never be copied into logs (even at `debug`). Diagnostics below
    // describe the failure without echoing the payload.
    _seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    // Step 1: find the first `;`, separating "99" from the rest.
    let Some(first_semi) = raw_params.iter().position(|&b| b == b';') else {
        tracing::debug!("OSC 99: missing first `;` (malformed sequence)");
        return;
    };

    // The remainder after "99;".
    let remainder = &raw_params[first_semi + 1..];

    // Step 2: split the remainder on the SECOND overall `;` (first in remainder).
    // Everything before it is <metadata>; everything after is <payload>.
    // If there is no second `;`, <metadata> = entire remainder, <payload> = &[].
    let (metadata, payload) = remainder
        .iter()
        .position(|&b| b == b';')
        .map_or((remainder, &[][..]), |second_semi| {
            (&remainder[..second_semi], &remainder[second_semi + 1..])
        });

    match parse_osc_99(metadata, payload) {
        Ok(cmd) => {
            output.push(TerminalOutput::OscResponse(AnsiOscType::Notify99(cmd)));
        }
        Err(e) => {
            tracing::debug!("OSC 99: parse error (ignored): {e}");
        }
    }
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
    use freminal_common::buffer_states::osc_notify_99::{Osc99Command, Osc99PayloadType};
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

    // ── OSC 99 ───────────────────────────────────────────────────────────

    /// Extract the `Osc99Command` from a single-element output, panicking on mismatch.
    fn expect_notify99(output: &[TerminalOutput]) -> &Osc99Command {
        assert_eq!(output.len(), 1, "expected one output, got: {output:?}");
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::Notify99(cmd)) => cmd,
            other => panic!("expected Notify99, got: {other:?}"),
        }
    }

    #[test]
    fn osc99_title_payload_bel() {
        // 99 ; p=title ; Hello  BEL
        let output = feed_osc(b"99;p=title;Hello\x07");
        let cmd = expect_notify99(&output);
        assert_eq!(cmd.payload_type, Osc99PayloadType::Title);
        assert_eq!(cmd.payload, b"Hello");
    }

    #[test]
    fn osc99_empty_metadata_uses_defaults() {
        // 99 ;; Just a title  BEL  — second `;` immediately → metadata empty
        let output = feed_osc(b"99;;Just a title\x07");
        let cmd = expect_notify99(&output);
        // Default payload type is Title.
        assert_eq!(cmd.payload_type, Osc99PayloadType::Title);
        assert_eq!(cmd.payload, b"Just a title");
    }

    #[test]
    fn osc99_payload_with_semicolons_preserved() {
        // Only the SECOND `;` splits — further `;` in the payload survive.
        // 99 ; p=body ; a;b;c  BEL
        let output = feed_osc(b"99;p=body;a;b;c\x07");
        let cmd = expect_notify99(&output);
        assert_eq!(cmd.payload_type, Osc99PayloadType::Body);
        assert_eq!(cmd.payload, b"a;b;c");
    }

    #[test]
    fn osc99_query_form_no_second_semicolon() {
        // 99 ; i=abc:p=?  BEL  — no second `;`: entire remainder is metadata, payload empty.
        let output = feed_osc(b"99;i=abc:p=?\x07");
        let cmd = expect_notify99(&output);
        assert_eq!(cmd.payload_type, Osc99PayloadType::Query);
        assert_eq!(cmd.id, Some("abc".to_owned()));
        assert_eq!(cmd.payload, b"");
    }

    #[test]
    fn osc99_base64_payload_decoded() {
        // "Hello" in base64 is "SGVsbG8="
        // 99 ; e=1 ; SGVsbG8=  BEL
        let output = feed_osc(b"99;e=1;SGVsbG8=\x07");
        let cmd = expect_notify99(&output);
        assert_eq!(cmd.payload, b"Hello");
    }

    #[test]
    fn osc99_missing_first_semicolon_no_output() {
        // "99" with no `;` at all — malformed, silently consumed.
        let output = feed_osc(b"99\x07");
        assert!(output.is_empty(), "got: {output:?}");
    }

    #[test]
    fn osc99_invalid_metadata_no_output() {
        // "foo" has no `=` → `parse_osc_99` returns `InvalidMetadata` → handler swallows.
        let output = feed_osc(b"99;foo;body\x07");
        assert!(output.is_empty(), "got: {output:?}");
    }

    #[test]
    fn osc99_direct_call_invalid_metadata_no_output() {
        // Direct-call test: invalid metadata bytes → empty output, no panic.
        let mut output = Vec::new();
        // "foo=bar" has a multi-char key → InvalidMetadata
        super::handle_osc_notify_99(b"99;foo=bar;body", &tracer(), &mut output);
        assert!(output.is_empty());
    }
}
