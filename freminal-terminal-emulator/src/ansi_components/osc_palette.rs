// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi_components::tracer::SequenceTracer;
use freminal_common::buffer_states::osc::{AnsiOscToken, AnsiOscType};
use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_common::colors::parse_color_spec;

/// Handle OSC 4 (palette color set/query).
///
/// Format: `OSC 4 ; index ; spec ST`
/// - `spec` = `?` → query palette entry
/// - `spec` = `rgb:RR/GG/BB` (1-4 hex digits per channel) → set palette entry
/// - `spec` = `#RRGGBB` (6 hex digits) → set palette entry
pub(super) fn handle_osc_palette_color(
    params: &[Option<AnsiOscToken>],
    seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    // params[0] = OscValue(4), params[1] = index string, params[2] = color spec
    let index = match params.get(1) {
        Some(Some(AnsiOscToken::OscValue(v))) => {
            if *v > 255 {
                tracing::warn!("OSC 4: index out of range: {v}");
                return;
            }
            u8::try_from(*v).unwrap_or(0)
        }
        Some(Some(AnsiOscToken::String(s))) => {
            let Ok(v) = s.parse::<u16>() else {
                tracing::warn!("OSC 4: invalid index string: {s}");
                return;
            };
            if v > 255 {
                tracing::warn!("OSC 4: index out of range: {v}");
                return;
            }
            u8::try_from(v).unwrap_or(0)
        }
        _ => {
            tracing::warn!("OSC 4: missing index: recent='{}'", seq_trace.as_str());
            return;
        }
    };

    let spec = if let Some(Some(AnsiOscToken::String(s))) = params.get(2) {
        s.as_str()
    } else {
        tracing::warn!("OSC 4: missing color spec: recent='{}'", seq_trace.as_str());
        return;
    };

    if spec == "?" {
        output.push(TerminalOutput::OscResponse(AnsiOscType::QueryPaletteColor(
            index,
        )));
        return;
    }

    if let Some(rgb) = parse_color_spec(spec) {
        output.push(TerminalOutput::OscResponse(AnsiOscType::SetPaletteColor(
            index, rgb.0, rgb.1, rgb.2,
        )));
    } else {
        tracing::warn!("OSC 4: invalid color spec: {spec}");
    }
}

/// Handle OSC 104 (reset palette color).
///
/// Format: `OSC 104 ST` (reset all) or `OSC 104 ; index ST` (reset one).
pub(super) fn handle_osc_reset_palette(
    params: &[Option<AnsiOscToken>],
    output: &mut Vec<TerminalOutput>,
) {
    // params[0] = OscValue(104), params[1..] = optional index(es)
    match params.get(1) {
        None | Some(None) => {
            // No index → reset all
            output.push(TerminalOutput::OscResponse(AnsiOscType::ResetPaletteColor(
                None,
            )));
        }
        Some(Some(AnsiOscToken::OscValue(v))) => {
            if *v <= 255 {
                output.push(TerminalOutput::OscResponse(AnsiOscType::ResetPaletteColor(
                    Some(u8::try_from(*v).unwrap_or(0)),
                )));
            } else {
                tracing::warn!("OSC 104: index out of range: {v}");
            }
        }
        Some(Some(AnsiOscToken::String(s))) => {
            if let Ok(v) = s.parse::<u16>() {
                if v <= 255 {
                    output.push(TerminalOutput::OscResponse(AnsiOscType::ResetPaletteColor(
                        Some(u8::try_from(v).unwrap_or(0)),
                    )));
                } else {
                    tracing::warn!("OSC 104: index out of range: {v}");
                }
            } else {
                tracing::warn!("OSC 104: invalid index: {s}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::osc::AnsiOscParser;
    use freminal_common::buffer_states::osc::AnsiOscType;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;
    use freminal_common::colors::{parse_color_spec, scale_hex_channel};

    fn feed_osc(payload: &[u8]) -> Vec<TerminalOutput> {
        let mut parser = AnsiOscParser::new();
        let mut output = Vec::new();
        for &b in payload {
            parser.ansiparser_inner_osc(b, &mut output);
        }
        output
    }

    // ------------------------------------------------------------------
    // scale_hex_channel tests
    // ------------------------------------------------------------------

    #[test]
    fn scale_hex_channel_1_digit() {
        // 0xa → (0xa << 4) | 0xa = 0xaa
        assert_eq!(scale_hex_channel("a"), Some(0xaa));
        assert_eq!(scale_hex_channel("0"), Some(0x00));
        assert_eq!(scale_hex_channel("f"), Some(0xff));
    }

    #[test]
    fn scale_hex_channel_2_digits() {
        assert_eq!(scale_hex_channel("ff"), Some(0xff));
        assert_eq!(scale_hex_channel("00"), Some(0x00));
        assert_eq!(scale_hex_channel("7f"), Some(0x7f));
        assert_eq!(scale_hex_channel("ab"), Some(0xab));
    }

    #[test]
    fn scale_hex_channel_3_digits() {
        // 0xfff → 0xfff >> 4 = 0xff
        assert_eq!(scale_hex_channel("fff"), Some(0xff));
        // 0x800 → 0x800 >> 4 = 0x80
        assert_eq!(scale_hex_channel("800"), Some(0x80));
        assert_eq!(scale_hex_channel("000"), Some(0x00));
    }

    #[test]
    fn scale_hex_channel_4_digits() {
        // 0xffff → 0xffff >> 8 = 0xff
        assert_eq!(scale_hex_channel("ffff"), Some(0xff));
        // 0x8000 → 0x8000 >> 8 = 0x80
        assert_eq!(scale_hex_channel("8000"), Some(0x80));
        assert_eq!(scale_hex_channel("0000"), Some(0x00));
    }

    #[test]
    fn scale_hex_channel_empty_returns_none() {
        assert_eq!(scale_hex_channel(""), None);
    }

    #[test]
    fn scale_hex_channel_5_digits_returns_none() {
        assert_eq!(scale_hex_channel("fffff"), None);
    }

    #[test]
    fn scale_hex_channel_invalid_hex_returns_none() {
        assert_eq!(scale_hex_channel("zz"), None);
        assert_eq!(scale_hex_channel("gg"), None);
    }

    // ------------------------------------------------------------------
    // parse_color_spec tests
    // ------------------------------------------------------------------

    #[test]
    fn parse_color_spec_rgb_2digit() {
        assert_eq!(parse_color_spec("rgb:ff/00/80"), Some((0xff, 0x00, 0x80)));
    }

    #[test]
    fn parse_color_spec_rgb_1digit() {
        // 1-digit: a → 0xaa
        assert_eq!(parse_color_spec("rgb:a/b/c"), Some((0xaa, 0xbb, 0xcc)));
    }

    #[test]
    fn parse_color_spec_rgb_4digit() {
        // 4-digit: ffff → 0xff, 0000 → 0x00
        assert_eq!(
            parse_color_spec("rgb:ffff/0000/8000"),
            Some((0xff, 0x00, 0x80))
        );
    }

    #[test]
    fn parse_color_spec_rgb_mixed_lengths() {
        // Mixed: 1/2/4 digits
        assert_eq!(parse_color_spec("rgb:f/ff/ffff"), Some((0xff, 0xff, 0xff)));
    }

    #[test]
    fn parse_color_spec_hash_6digit() {
        assert_eq!(parse_color_spec("#ff0080"), Some((0xff, 0x00, 0x80)));
        assert_eq!(parse_color_spec("#000000"), Some((0x00, 0x00, 0x00)));
        assert_eq!(parse_color_spec("#ffffff"), Some((0xff, 0xff, 0xff)));
    }

    #[test]
    fn parse_color_spec_hash_3digit() {
        // #RGB → each expanded by *17: f→ff, 0→00, 8→88
        assert_eq!(parse_color_spec("#f08"), Some((0xff, 0x00, 0x88)));
        assert_eq!(parse_color_spec("#abc"), Some((0xaa, 0xbb, 0xcc)));
    }

    #[test]
    fn parse_color_spec_invalid_formats() {
        assert_eq!(parse_color_spec(""), None);
        assert_eq!(parse_color_spec("notacolor"), None);
        assert_eq!(parse_color_spec("#12"), None); // wrong length
        assert_eq!(parse_color_spec("#1234567"), None); // wrong length
        assert_eq!(parse_color_spec("rgb:"), None); // no channels
        assert_eq!(parse_color_spec("rgb:ff/00"), None); // only 2 channels
        assert_eq!(parse_color_spec("rgb:ff/00/80/aa"), None); // 4 channels
        assert_eq!(parse_color_spec("#zzzzzz"), None); // invalid hex
    }

    #[test]
    fn parse_color_spec_rgb_invalid_hex() {
        assert_eq!(parse_color_spec("rgb:zz/00/00"), None);
    }

    // ------------------------------------------------------------------
    // OSC 4 / OSC 104 parser integration tests
    // ------------------------------------------------------------------

    #[test]
    fn osc4_set_palette_color_rgb_format() {
        // OSC 4 ; 10 ; rgb:ff/00/80 BEL
        let payload = b"4;10;rgb:ff/00/80\x07";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::SetPaletteColor(10, 0xff, 0x00, 0x80))
        ));
    }

    #[test]
    fn osc4_set_palette_color_hash_format() {
        // OSC 4 ; 42 ; #aabbcc ST
        let payload = b"4;42;#aabbcc\x1b\\";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::SetPaletteColor(42, 0xaa, 0xbb, 0xcc))
        ));
    }

    #[test]
    fn osc4_query_palette_color() {
        // OSC 4 ; 5 ; ? BEL
        let payload = b"4;5;?\x07";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::QueryPaletteColor(5))
        ));
    }

    #[test]
    fn osc4_invalid_index_out_of_range_no_output() {
        // Index 300 is > 255, should produce no output
        let payload = b"4;300;rgb:ff/ff/ff\x07";
        let output = feed_osc(payload);
        assert!(output.is_empty());
    }

    #[test]
    fn osc4_missing_color_spec_no_output() {
        // OSC 4 ; 10 BEL (missing color spec)
        let payload = b"4;10\x07";
        let output = feed_osc(payload);
        assert!(output.is_empty());
    }

    #[test]
    fn osc104_reset_all() {
        // OSC 104 BEL
        let payload = b"104\x07";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::ResetPaletteColor(None))
        ));
    }

    #[test]
    fn osc104_reset_single_index() {
        // OSC 104 ; 42 BEL
        let payload = b"104;42\x07";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::ResetPaletteColor(Some(42)))
        ));
    }

    #[test]
    fn osc104_index_out_of_range_no_output() {
        // OSC 104 ; 300 BEL — index > 255
        let payload = b"104;300\x07";
        let output = feed_osc(payload);
        assert!(output.is_empty());
    }

    #[test]
    fn osc4_invalid_color_spec_no_output() {
        // OSC 4 ; 7 ; notacolor BEL — parse_color_spec fails → warn, no output
        let payload = b"4;7;notacolor\x07";
        let output = feed_osc(payload);
        assert!(output.is_empty());
    }

    #[test]
    fn osc4_valid_palette_color_index_7() {
        // OSC 4 ; 7 ; rgb:ff/00/00 BEL
        let payload = b"4;7;rgb:ff/00/00\x07";
        let output = feed_osc(payload);
        assert_eq!(output.len(), 1);
        assert!(matches!(
            &output[0],
            TerminalOutput::OscResponse(AnsiOscType::SetPaletteColor(7, 0xff, 0x00, 0x00))
        ));
    }

    #[test]
    fn osc104_invalid_string_index_no_output() {
        // OSC 104 ; notanumber BEL — fails to parse as u16 → warn, no output
        // We need to produce a String token rather than OscValue.
        // A non-numeric string after "104;" will be parsed as an AnsiOscToken::String.
        let payload = b"104;notanumber\x07";
        let output = feed_osc(payload);
        assert!(output.is_empty());
    }
}
