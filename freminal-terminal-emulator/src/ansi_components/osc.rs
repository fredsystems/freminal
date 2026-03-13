// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//use eframe::egui::Color32;

use crate::ansi::ParserOutcome;
use crate::ansi_components::tracer::{SequenceTraceable, SequenceTracer};
use anyhow::Result;
use freminal_common::buffer_states::ftcs::parse_ftcs_params;
use freminal_common::buffer_states::osc::{
    AnsiOscInternalType, AnsiOscToken, AnsiOscType, OscTarget, UrlResponse,
};
use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_common::colors::parse_color_spec;

#[derive(Eq, PartialEq, Debug)]
pub enum AnsiOscParserState {
    Params,
    //Intermediates,
    Finished,
    Invalid,
    InvalidFinished,
}

#[derive(Eq, PartialEq, Debug)]
pub struct AnsiOscParser {
    pub(crate) state: AnsiOscParserState,
    pub(crate) params: Vec<u8>,
    pub(crate) intermediates: Vec<u8>,
    pub(crate) seq_trace: SequenceTracer,
}

impl SequenceTraceable for AnsiOscParser {
    #[inline]
    fn seq_tracer(&mut self) -> &mut SequenceTracer {
        &mut self.seq_trace
    }
    #[inline]
    fn seq_tracer_ref(&self) -> &SequenceTracer {
        &self.seq_trace
    }
}

// OSC Sequence looks like this:
// 1b]11;?1b\

impl Default for AnsiOscParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AnsiOscParser {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: AnsiOscParserState::Params,
            params: Vec::new(),
            intermediates: Vec::new(),
            seq_trace: SequenceTracer::new(),
        }
    }

    /// Expose current sequence trace for testing and diagnostics.
    #[must_use]
    pub fn trace_str(&self) -> String {
        info!("current buffer trace: {}", self.seq_trace.as_str());
        self.seq_trace.as_str()
    }

    /// Push a byte into the parser
    ///
    /// # Errors
    /// Will return an error if the parser is in the `Finished` or `InvalidFinished` state
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn push(&mut self, b: u8) -> ParserOutcome {
        self.append_trace(b);
        if let AnsiOscParserState::Finished | AnsiOscParserState::InvalidFinished = &self.state {
            return ParserOutcome::Invalid("Parsed Pushed To Once Finished".to_string());
        }

        match self.state {
            AnsiOscParserState::Params => {
                if is_valid_osc_param(b) {
                    self.params.push(b);
                } else {
                    debug!("Invalid OSC param: {:x}", b);
                    {
                        self.state = AnsiOscParserState::Invalid;

                        self.params.clear();
                        self.intermediates.clear();

                        return ParserOutcome::Invalid("Invalid OSC param encountered".to_string());
                    };
                }

                if is_osc_terminator(&self.params) {
                    self.state = AnsiOscParserState::Finished;

                    self.seq_trace.trim_control_tail();

                    if !self.params.is_empty() {
                        while let Some(&last) = self.params.last() {
                            if is_final_character_osc_terminator(last) {
                                self.params.pop();
                            } else {
                                break;
                            }
                        }
                    }

                    return ParserOutcome::Finished;
                }

                ParserOutcome::Continue
            }
            // OscParserState::Intermediates => {
            //     panic!("OscParser should not be in intermediates state");
            // }
            AnsiOscParserState::Finished | AnsiOscParserState::InvalidFinished => {
                unreachable!()
            }
            AnsiOscParserState::Invalid => {
                if is_osc_terminator(&self.params) {
                    self.state = AnsiOscParserState::InvalidFinished;
                }

                ParserOutcome::Invalid("Invalid OSC sequence terminated".to_string())
            }
        }
    }

    /// Parse the OSC sequence
    ///
    /// # Errors
    /// Will return an error if the parser is in the `Finished` or `InvalidFinished` state
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn ansiparser_inner_osc(
        &mut self,
        b: u8,
        output: &mut Vec<TerminalOutput>,
    ) -> ParserOutcome {
        let push_result = self.push(b);

        // if we failed the push result with ParserOutcome::Invalid, return push_result
        if let ParserOutcome::Invalid(_) = push_result {
            return push_result;
        }

        match self.state {
            AnsiOscParserState::Finished => {
                if let Ok(params) = split_params_into_semicolon_delimited_usize(&self.params) {
                    let Some(type_number) = extract_param(0, &params) else {
                        output.push(TerminalOutput::Invalid);
                        return ParserOutcome::Invalid(format!(
                            "Invalid OSC params: recent='{}'",
                            self.seq_trace.as_str()
                        ));
                    };

                    // Only clone what’s actually reused later.
                    let osc_target = OscTarget::from(&type_number);
                    let osc_internal_type = AnsiOscInternalType::from(&params);

                    dispatch_osc_target(
                        &osc_target,
                        osc_internal_type,
                        params,
                        &self.seq_trace,
                        output,
                    );
                } else {
                    output.push(TerminalOutput::Invalid);

                    return ParserOutcome::Invalid(format!(
                        "Invalid OSC params: recent='{}'",
                        self.seq_trace.as_str()
                    ));
                }

                ParserOutcome::Finished
            }
            AnsiOscParserState::Invalid => ParserOutcome::Invalid("Invalid OSC State".to_string()),
            _ => ParserOutcome::Continue,
        }
    }
}

fn dispatch_osc_target(
    osc_target: &OscTarget,
    osc_internal_type: AnsiOscInternalType,
    params: Vec<Option<AnsiOscToken>>,
    seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    match *osc_target {
        OscTarget::Background => {
            output.push(TerminalOutput::OscResponse(
                AnsiOscType::RequestColorQueryBackground(osc_internal_type),
            ));
        }
        OscTarget::Foreground => {
            output.push(TerminalOutput::OscResponse(
                AnsiOscType::RequestColorQueryForeground(osc_internal_type),
            ));
        }
        OscTarget::TitleBar | OscTarget::IconName => {
            output.push(TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(
                osc_internal_type.to_string(),
            )));
        }
        OscTarget::Ftcs => {
            // Extract the string tokens after "133" and pass
            // them to the FTCS parser.  E.g. for
            // `OSC 133 ; D ; 0 ST` → params_strs = ["D", "0"]
            let ftcs_strs: Vec<&str> = params
                .iter()
                .skip(1) // skip the "133" token
                .filter_map(|t| match t {
                    Some(AnsiOscToken::String(s)) => Some(s.as_str()),
                    _ => None,
                })
                .collect();

            if let Some(marker) = parse_ftcs_params(&ftcs_strs) {
                output.push(TerminalOutput::OscResponse(AnsiOscType::Ftcs(marker)));
            } else {
                tracing::debug!(
                    "OSC 133: unrecognised FTCS params: recent='{}'",
                    seq_trace.as_str()
                );
            }
        }
        OscTarget::Clipboard => {
            handle_osc_clipboard(&params, seq_trace, output);
        }
        OscTarget::PaletteColor => {
            handle_osc_palette_color(&params, seq_trace, output);
        }
        OscTarget::ResetPaletteColor => {
            handle_osc_reset_palette(&params, output);
        }
        OscTarget::RemoteHost => {
            output.push(TerminalOutput::OscResponse(AnsiOscType::RemoteHost(
                osc_internal_type.to_string(),
            )));
        }
        OscTarget::Url => {
            let url_response = UrlResponse::from(params);
            output.push(TerminalOutput::OscResponse(AnsiOscType::Url(url_response)));
        }
        OscTarget::ResetCursorColor => {
            output.push(TerminalOutput::OscResponse(AnsiOscType::ResetCursorColor));
        }
        OscTarget::ResetForeground => {
            output.push(TerminalOutput::OscResponse(
                AnsiOscType::ResetForegroundColor,
            ));
        }
        OscTarget::ResetBackground => {
            output.push(TerminalOutput::OscResponse(
                AnsiOscType::ResetBackgroundColor,
            ));
        }
        OscTarget::ITerm2 => {
            output.push(TerminalOutput::OscResponse(AnsiOscType::ITerm2));
        }
        OscTarget::Unknown => {
            // Unknown OSC sequences are silently consumed (like
            // xterm/VTE).  Downgraded from error!/Invalid to debug!
            // so they don't spam logs during normal usage.
            tracing::debug!(
                "Unknown OSC Target (silently consumed): type_number={osc_internal_type:?}, recent='{}'",
                seq_trace.as_str()
            );
        }
    }
}

/// Handle OSC 52 clipboard set/query.
///
/// `params[0]` = `OscValue(52)`, `params[1]` = selection string, `params[2]` = base64 or `?`.
fn handle_osc_clipboard(
    params: &[Option<AnsiOscToken>],
    seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    let selection = match params.get(1) {
        Some(Some(AnsiOscToken::String(s))) => s.clone(),
        _ => "c".to_string(), // default to clipboard
    };

    match params.get(2) {
        Some(Some(AnsiOscToken::String(data))) if data == "?" => {
            output.push(TerminalOutput::OscResponse(AnsiOscType::QueryClipboard(
                selection,
            )));
        }
        Some(Some(AnsiOscToken::String(data))) => match freminal_common::base64::decode(data) {
            Ok(decoded_bytes) => {
                let content = String::from_utf8_lossy(&decoded_bytes).into_owned();
                output.push(TerminalOutput::OscResponse(AnsiOscType::SetClipboard(
                    selection, content,
                )));
            }
            Err(e) => {
                tracing::debug!("OSC 52: invalid base64 payload: {e}");
            }
        },
        _ => {
            tracing::debug!(
                "OSC 52: missing or invalid payload: recent='{}'",
                seq_trace.as_str()
            );
        }
    }
}

/// Handle OSC 4 (palette color set/query).
///
/// Format: `OSC 4 ; index ; spec ST`
/// - `spec` = `?` → query palette entry
/// - `spec` = `rgb:RR/GG/BB` (1-4 hex digits per channel) → set palette entry
/// - `spec` = `#RRGGBB` (6 hex digits) → set palette entry
fn handle_osc_palette_color(
    params: &[Option<AnsiOscToken>],
    seq_trace: &SequenceTracer,
    output: &mut Vec<TerminalOutput>,
) {
    // params[0] = OscValue(4), params[1] = index string, params[2] = color spec
    let index = match params.get(1) {
        Some(Some(AnsiOscToken::OscValue(v))) => {
            if *v > 255 {
                tracing::debug!("OSC 4: index out of range: {v}");
                return;
            }
            #[allow(clippy::cast_possible_truncation)]
            {
                *v as u8
            }
        }
        Some(Some(AnsiOscToken::String(s))) => {
            let Ok(v) = s.parse::<u16>() else {
                tracing::debug!("OSC 4: invalid index string: {s}");
                return;
            };
            if v > 255 {
                tracing::debug!("OSC 4: index out of range: {v}");
                return;
            }
            #[allow(clippy::cast_possible_truncation)]
            {
                v as u8
            }
        }
        _ => {
            tracing::debug!("OSC 4: missing index: recent='{}'", seq_trace.as_str());
            return;
        }
    };

    let spec = if let Some(Some(AnsiOscToken::String(s))) = params.get(2) {
        s.as_str()
    } else {
        tracing::debug!("OSC 4: missing color spec: recent='{}'", seq_trace.as_str());
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
        tracing::debug!("OSC 4: invalid color spec: {spec}");
    }
}

/// Handle OSC 104 (reset palette color).
///
/// Format: `OSC 104 ST` (reset all) or `OSC 104 ; index ST` (reset one).
fn handle_osc_reset_palette(params: &[Option<AnsiOscToken>], output: &mut Vec<TerminalOutput>) {
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
                #[allow(clippy::cast_possible_truncation)]
                output.push(TerminalOutput::OscResponse(AnsiOscType::ResetPaletteColor(
                    Some(*v as u8),
                )));
            } else {
                tracing::debug!("OSC 104: index out of range: {v}");
            }
        }
        Some(Some(AnsiOscToken::String(s))) => {
            if let Ok(v) = s.parse::<u16>() {
                if v <= 255 {
                    #[allow(clippy::cast_possible_truncation)]
                    output.push(TerminalOutput::OscResponse(AnsiOscType::ResetPaletteColor(
                        Some(v as u8),
                    )));
                } else {
                    tracing::debug!("OSC 104: index out of range: {v}");
                }
            } else {
                tracing::debug!("OSC 104: invalid index: {s}");
            }
        }
    }
}

// parse_color_spec is now provided by freminal_common::colors::parse_color_spec.
// The import above (`use freminal_common::colors::parse_color_spec`) brings it into scope
// for the palette-color handler below that still uses it locally.
const fn is_osc_terminator(b: &[u8]) -> bool {
    matches!(b, [.., 0x07] | [.., 0x1b, 0x5c])
}

// FIXME: Support ST (0x1b)\ as a terminator
const fn is_final_character_osc_terminator(b: u8) -> bool {
    b == 0x5c || b == 0x07 || b == 0x1b
}

fn is_valid_osc_param(b: u8) -> bool {
    // if the character is a printable character, or is 0x1b or 0x5c then it is valid
    (0x20..=0x7E).contains(&b) || (0x80..=0xff).contains(&b) || b == 0x1b || b == 0x07
}

/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn split_params_into_semicolon_delimited_usize(
    params: &[u8],
) -> Result<Vec<Option<AnsiOscToken>>> {
    params
        .split(|b| *b == b';')
        .map(parse_param_as::<AnsiOscToken>)
        .collect::<Result<Vec<Option<AnsiOscToken>>>>()
}

/// # Errors
///
/// Will return an error if the parameter is not a valid number
pub fn parse_param_as<T: std::str::FromStr>(param_bytes: &[u8]) -> Result<Option<T>> {
    let param_str = std::str::from_utf8(param_bytes)?;
    if param_str.is_empty() {
        return Ok(None);
    }
    param_str.parse().map_err(|_| ()).map_or_else(
        |()| {
            debug!(
                "Failed to parse parameter ({:?}) as {:?}",
                param_bytes,
                std::any::type_name::<T>()
            );
            Err(anyhow::anyhow!("Failed to parse parameter"))
        },
        |value| Ok(Some(value)),
    )
}

pub fn extract_param(idx: usize, params: &[Option<AnsiOscToken>]) -> Option<AnsiOscToken> {
    // get the parameter at the index
    params.get(idx).and_then(std::clone::Clone::clone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::colors::scale_hex_channel;

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

    /// Feed an OSC sequence byte-by-byte and collect the output.
    fn feed_osc(payload: &[u8]) -> Vec<TerminalOutput> {
        let mut parser = AnsiOscParser::new();
        let mut output = Vec::new();
        for &b in payload {
            parser.ansiparser_inner_osc(b, &mut output);
        }
        output
    }

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
}
