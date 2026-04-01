// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi_components::tracer::SequenceTracer;
use freminal_common::buffer_states::osc::{AnsiOscType, ITerm2InlineImageData, ImageDimension};
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Handle OSC 1337 (iTerm2 extensions).
///
/// The primary sub-command we support is `File=`, which carries an inline
/// image.  Format:
///
/// ```text
/// 1337 ; File = [key=value[;key=value]...] : <base64 data>
/// ```
///
/// `raw_params` is the full, un-split OSC parameter bytes (before `;` splitting).
/// We parse from the raw bytes because the `;` delimiter inside the `File=` args
/// must be handled together with the `:` that separates args from the base64 payload.
pub(super) fn handle_osc_iterm2(
    raw_params: &[u8],
    seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    // raw_params looks like: b"1337;File=inline=1;width=auto:BASE64DATA"
    // or: b"1337;MultipartFile=inline=1;width=auto"
    // or: b"1337;FilePart=BASE64DATA"
    // or: b"1337;FileEnd"
    // or: b"1337;SomeOtherCommand=..."
    //
    // Find the first ';' to skip past "1337".
    let Some(first_semi) = raw_params.iter().position(|&b| b == b';') else {
        tracing::warn!(
            "OSC 1337: missing sub-command: recent='{}'",
            seq_trace.as_str()
        );
        return;
    };

    let rest = &raw_params[first_semi + 1..];

    // Check for "File=" prefix (case-sensitive, per iTerm2 spec).
    if let Some(after_file) = strip_ascii_prefix(rest, b"File=") {
        handle_osc_iterm2_file(after_file, seq_trace, output);
        return;
    }

    // Check for "MultipartFile=" prefix.
    if let Some(after_mp) = strip_ascii_prefix(rest, b"MultipartFile=") {
        handle_osc_iterm2_multipart_begin(after_mp, seq_trace, output);
        return;
    }

    // Check for "FilePart=" prefix.
    if let Some(after_part) = strip_ascii_prefix(rest, b"FilePart=") {
        handle_osc_iterm2_file_part(after_part, seq_trace, output);
        return;
    }

    // Check for "FileEnd" (no '=' — it's a bare command).
    if rest == b"FileEnd" {
        output.push(TerminalOutput::OscResponse(AnsiOscType::ITerm2FileEnd));
        return;
    }

    // Not a recognised sub-command — silently consume, like xterm/VTE.
    tracing::warn!(
        "OSC 1337: unrecognised sub-command: recent='{}'",
        seq_trace.as_str()
    );
    output.push(TerminalOutput::OscResponse(AnsiOscType::ITerm2Unknown));
}

/// Parse the key=value args common to `File=` and `MultipartFile=`.
///
/// `args_str` is the `;`-delimited key=value portion (e.g. `"inline=1;width=auto"`).
fn parse_iterm2_file_args(args_str: &str) -> ITerm2InlineImageData {
    let mut name: Option<String> = None;
    let mut size: Option<usize> = None;
    let mut width: Option<ImageDimension> = None;
    let mut height: Option<ImageDimension> = None;
    let mut preserve_aspect_ratio = true;
    let mut inline = false;
    let mut do_not_move_cursor = false;

    for pair in args_str.split(';') {
        if let Some((key, value)) = pair.split_once('=') {
            match key {
                "name" => {
                    // Name is base64-encoded.
                    if let Ok(decoded) = freminal_common::base64::decode(value) {
                        name = Some(String::from_utf8_lossy(&decoded).into_owned());
                    }
                }
                "size" => {
                    size = value.parse().ok();
                }
                "width" => {
                    width = ImageDimension::parse(value);
                }
                "height" => {
                    height = ImageDimension::parse(value);
                }
                "preserveAspectRatio" => {
                    preserve_aspect_ratio = value != "0";
                }
                "inline" => {
                    inline = value == "1";
                }
                "doNotMoveCursor" => {
                    do_not_move_cursor = value == "1";
                }
                _ => {
                    tracing::debug!("OSC 1337 File args: unknown arg: {key}={value}");
                }
            }
        }
    }

    ITerm2InlineImageData {
        name,
        size,
        width,
        height,
        preserve_aspect_ratio,
        inline,
        do_not_move_cursor,
        data: Vec::new(),
    }
}

/// Handle `OSC 1337 ; File = [args] : [base64] BEL` — single-sequence inline image.
fn handle_osc_iterm2_file(
    after_file: &[u8],
    seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    // `after_file` is: b"inline=1;width=auto:BASE64DATA"
    // Split on ':' to separate key=value args from the base64 payload.
    let Some(colon_pos) = after_file.iter().position(|&b| b == b':') else {
        tracing::warn!(
            "OSC 1337 File=: missing ':' separator: recent='{}'",
            seq_trace.as_str()
        );
        return;
    };

    let args_bytes = &after_file[..colon_pos];
    let b64_bytes = &after_file[colon_pos + 1..];

    let Ok(args_str) = std::str::from_utf8(args_bytes) else {
        tracing::warn!("OSC 1337 File=: non-UTF-8 args");
        return;
    };

    let mut image_data = parse_iterm2_file_args(args_str);

    // Decode base64 payload.
    let Ok(b64_str) = std::str::from_utf8(b64_bytes) else {
        tracing::warn!("OSC 1337 File=: non-UTF-8 base64 payload");
        return;
    };

    let data = match freminal_common::base64::decode(b64_str) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!("OSC 1337 File=: base64 decode failed: {e}");
            return;
        }
    };

    if data.is_empty() {
        tracing::warn!("OSC 1337 File=: empty payload after base64 decode");
        return;
    }

    image_data.data = data;

    output.push(TerminalOutput::OscResponse(AnsiOscType::ITerm2FileInline(
        image_data,
    )));
}

/// Handle `OSC 1337 ; MultipartFile = [args] BEL` — begin multipart transfer.
///
/// `MultipartFile=` has the same key=value args as `File=` but **no** `:base64` payload.
fn handle_osc_iterm2_multipart_begin(
    after_mp: &[u8],
    seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    let Ok(args_str) = std::str::from_utf8(after_mp) else {
        tracing::warn!("OSC 1337 MultipartFile=: non-UTF-8 args");
        return;
    };

    if args_str.is_empty() {
        tracing::warn!(
            "OSC 1337 MultipartFile=: empty args: recent='{}'",
            seq_trace.as_str()
        );
        return;
    }

    let image_data = parse_iterm2_file_args(args_str);

    output.push(TerminalOutput::OscResponse(
        AnsiOscType::ITerm2MultipartBegin(image_data),
    ));
}

/// Handle `OSC 1337 ; FilePart = [base64] BEL` — one chunk of multipart data.
fn handle_osc_iterm2_file_part(
    after_part: &[u8],
    seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    let Ok(b64_str) = std::str::from_utf8(after_part) else {
        tracing::warn!("OSC 1337 FilePart=: non-UTF-8 base64 payload");
        return;
    };

    let data = match freminal_common::base64::decode(b64_str) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(
                "OSC 1337 FilePart=: base64 decode failed: {e}: recent='{}'",
                seq_trace.as_str()
            );
            return;
        }
    };

    output.push(TerminalOutput::OscResponse(AnsiOscType::ITerm2FilePart(
        data,
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
mod tests {
    use super::super::osc::AnsiOscParser;
    use freminal_common::buffer_states::osc::{AnsiOscType, ImageDimension};
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    fn feed_osc(payload: &[u8]) -> Vec<TerminalOutput> {
        let mut parser = AnsiOscParser::new();
        let mut output = Vec::new();
        for &b in payload {
            parser.ansiparser_inner_osc(b, &mut output);
        }
        output
    }

    /// Build a minimal valid OSC 1337 File= payload with base64-encoded data.
    fn build_iterm2_file_payload(args: &str, raw_data: &[u8]) -> Vec<u8> {
        let b64 = freminal_common::base64::encode(raw_data);
        let mut payload = format!("1337;File={args}:{b64}").into_bytes();
        payload.push(0x07); // BEL terminator
        payload
    }

    #[test]
    fn osc1337_file_inline_basic() {
        // Minimal: inline=1 with a small fake payload.
        let payload = build_iterm2_file_payload("inline=1", b"FAKEIMAGE");
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ITerm2FileInline(data)) => {
                assert!(data.inline);
                assert!(data.preserve_aspect_ratio); // default
                assert_eq!(data.name, None);
                assert_eq!(data.size, None);
                assert_eq!(data.width, None);
                assert_eq!(data.height, None);
                assert_eq!(data.data, b"FAKEIMAGE");
            }
            other => panic!("Expected ITerm2FileInline, got: {other:?}"),
        }
    }

    #[test]
    fn osc1337_file_all_args() {
        // name is base64-encoded "test.png"
        let name_b64 = freminal_common::base64::encode(b"test.png");
        let args = format!(
            "name={name_b64};size=12345;width=10;height=50%;preserveAspectRatio=0;inline=1"
        );
        let payload = build_iterm2_file_payload(&args, b"DATA");
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ITerm2FileInline(data)) => {
                assert_eq!(data.name, Some("test.png".to_string()));
                assert_eq!(data.size, Some(12345));
                assert_eq!(data.width, Some(ImageDimension::Cells(10)));
                assert_eq!(data.height, Some(ImageDimension::Percent(50)));
                assert!(!data.preserve_aspect_ratio);
                assert!(data.inline);
                assert_eq!(data.data, b"DATA");
            }
            other => panic!("Expected ITerm2FileInline, got: {other:?}"),
        }
    }

    #[test]
    fn osc1337_file_width_pixels_height_auto() {
        let args = "inline=1;width=100px;height=auto";
        let payload = build_iterm2_file_payload(args, b"PX");
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ITerm2FileInline(data)) => {
                assert_eq!(data.width, Some(ImageDimension::Pixels(100)));
                assert_eq!(data.height, Some(ImageDimension::Auto));
            }
            other => panic!("Expected ITerm2FileInline, got: {other:?}"),
        }
    }

    #[test]
    fn osc1337_file_inline_false_by_default() {
        // No inline= arg → inline defaults to false
        let payload = build_iterm2_file_payload("size=10", b"DATA");
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ITerm2FileInline(data)) => {
                assert!(!data.inline);
            }
            other => panic!("Expected ITerm2FileInline, got: {other:?}"),
        }
    }

    #[test]
    fn osc1337_non_file_subcommand_returns_unknown() {
        // OSC 1337 ; SetUserVar=foo=bar BEL
        let mut payload = b"1337;SetUserVar=foo=bar\x07".to_vec();
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::ITerm2Unknown)
        ));

        // Also test with ST terminator
        payload = b"1337;SetUserVar=foo=bar\x1b\\".to_vec();
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::ITerm2Unknown)
        ));
    }

    #[test]
    fn osc1337_missing_colon_no_output() {
        // File= args without ':' separator before base64 data
        let payload = b"1337;File=inline=1\x07";
        let output = feed_osc(payload);
        assert!(output.is_empty());
    }

    #[test]
    fn osc1337_empty_base64_no_output() {
        // File= with colon but empty base64 payload → empty after decode → no output
        let payload = b"1337;File=inline=1:\x07";
        let output = feed_osc(payload);
        assert!(output.is_empty());
    }

    #[test]
    fn osc1337_missing_semicolon_no_output() {
        // "1337File=inline=1:..." — missing ';' after 1337
        let payload = b"1337File=inline=1:QUFB\x07";
        let output = feed_osc(payload);
        // The parser splits on ';' first — "1337File" won't parse as a valid
        // OscValue, so this becomes an Invalid sequence.
        // Either no output or an invalid output is acceptable.
        for item in &output {
            assert!(!matches!(
                item,
                TerminalOutput::OscResponse(AnsiOscType::ITerm2FileInline(_))
            ));
        }
    }

    #[test]
    fn osc1337_file_st_terminator() {
        // Same as basic test but with ESC \ (ST) terminator instead of BEL
        let b64 = freminal_common::base64::encode(b"HELLO");
        let mut payload = format!("1337;File=inline=1:{b64}").into_bytes();
        payload.push(0x1b);
        payload.push(0x5c);
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ITerm2FileInline(data)) => {
                assert!(data.inline);
                assert_eq!(data.data, b"HELLO");
            }
            other => panic!("Expected ITerm2FileInline, got: {other:?}"),
        }
    }

    #[test]
    fn osc1337_file_unknown_args_ignored() {
        // Unknown key=value pairs should be silently ignored.
        let args = "inline=1;unknown_key=some_value;another=42";
        let payload = build_iterm2_file_payload(args, b"OK");
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ITerm2FileInline(data)) => {
                assert!(data.inline);
                assert_eq!(data.data, b"OK");
            }
            other => panic!("Expected ITerm2FileInline, got: {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // OSC 1337 MultipartFile / FilePart / FileEnd parser tests
    // ------------------------------------------------------------------

    #[test]
    fn osc1337_multipart_begin_basic() {
        let payload = b"1337;MultipartFile=inline=1\x07";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ITerm2MultipartBegin(data)) => {
                assert!(data.inline);
                assert!(data.preserve_aspect_ratio); // default
                assert_eq!(data.name, None);
                assert_eq!(data.size, None);
                assert_eq!(data.width, None);
                assert_eq!(data.height, None);
                assert!(data.data.is_empty()); // no payload for begin
            }
            other => panic!("Expected ITerm2MultipartBegin, got: {other:?}"),
        }
    }

    #[test]
    fn osc1337_multipart_begin_all_args() {
        let name_b64 = freminal_common::base64::encode(b"photo.jpg");
        let args =
            format!("1337;MultipartFile=name={name_b64};size=9999;width=20;height=10;inline=1");
        let mut payload = args.into_bytes();
        payload.push(0x07);
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ITerm2MultipartBegin(data)) => {
                assert_eq!(data.name, Some("photo.jpg".to_string()));
                assert_eq!(data.size, Some(9999));
                assert_eq!(data.width, Some(ImageDimension::Cells(20)));
                assert_eq!(data.height, Some(ImageDimension::Cells(10)));
                assert!(data.inline);
            }
            other => panic!("Expected ITerm2MultipartBegin, got: {other:?}"),
        }
    }

    #[test]
    fn osc1337_multipart_begin_empty_args_no_output() {
        // MultipartFile= with nothing after '=' → empty args → no output
        let payload = b"1337;MultipartFile=\x07";
        let output = feed_osc(payload);
        assert!(output.is_empty());
    }

    #[test]
    fn osc1337_file_part_basic() {
        let b64 = freminal_common::base64::encode(b"chunk data here");
        let mut payload = format!("1337;FilePart={b64}").into_bytes();
        payload.push(0x07);
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ITerm2FilePart(bytes)) => {
                assert_eq!(bytes, b"chunk data here");
            }
            other => panic!("Expected ITerm2FilePart, got: {other:?}"),
        }
    }

    #[test]
    fn osc1337_file_part_invalid_base64_no_output() {
        // Invalid base64 → decode fails → no output
        let payload = b"1337;FilePart=!!!invalid!!!\x07";
        let output = feed_osc(payload);
        assert!(output.is_empty());
    }

    #[test]
    fn osc1337_file_end() {
        let payload = b"1337;FileEnd\x07";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::ITerm2FileEnd)
        ));
    }

    #[test]
    fn osc1337_file_end_st_terminator() {
        // FileEnd with ESC \ (ST) terminator
        let payload = b"1337;FileEnd\x1b\\";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::ITerm2FileEnd)
        ));
    }

    #[test]
    fn osc1337_multipart_begin_st_terminator() {
        // MultipartFile with ST terminator
        let mut payload = b"1337;MultipartFile=inline=1\x1b\\".to_vec();
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ITerm2MultipartBegin(data)) => {
                assert!(data.inline);
            }
            other => panic!("Expected ITerm2MultipartBegin, got: {other:?}"),
        }

        // FilePart with ST terminator
        let b64 = freminal_common::base64::encode(b"TEST");
        payload = format!("1337;FilePart={b64}\x1b\\").into_bytes();
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::ITerm2FilePart(_))
        ));
    }

    #[test]
    fn osc1337_file_parses_do_not_move_cursor() {
        use freminal_common::base64;

        // Build a minimal valid PNG-like payload (doesn't matter for parsing).
        let b64_payload = base64::encode(b"\x89PNG\r\n\x1a\ntest");

        // With doNotMoveCursor=1
        let payload =
            format!("1337;File=inline=1;doNotMoveCursor=1:{b64_payload}\x07").into_bytes();
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ITerm2FileInline(data)) => {
                assert!(data.inline);
                assert!(
                    data.do_not_move_cursor,
                    "doNotMoveCursor=1 should set do_not_move_cursor to true"
                );
            }
            other => panic!("Expected ITerm2FileInline, got: {other:?}"),
        }

        // With doNotMoveCursor=0
        let payload =
            format!("1337;File=inline=1;doNotMoveCursor=0:{b64_payload}\x07").into_bytes();
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ITerm2FileInline(data)) => {
                assert!(
                    !data.do_not_move_cursor,
                    "doNotMoveCursor=0 should set do_not_move_cursor to false"
                );
            }
            other => panic!("Expected ITerm2FileInline, got: {other:?}"),
        }

        // Without doNotMoveCursor (default = false)
        let payload = format!("1337;File=inline=1:{b64_payload}\x07").into_bytes();
        let output = feed_osc(&payload);
        assert_eq!(output.len(), 1);
        match &output[0] {
            TerminalOutput::OscResponse(AnsiOscType::ITerm2FileInline(data)) => {
                assert!(
                    !data.do_not_move_cursor,
                    "Missing doNotMoveCursor should default to false"
                );
            }
            other => panic!("Expected ITerm2FileInline, got: {other:?}"),
        }
    }
}
