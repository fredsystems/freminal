// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::line_draw::DecSpecialGraphics;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

use crate::ansi::ParserOutcome;
use crate::ansi_components::tracer::{SequenceTraceable, SequenceTracer};

#[derive(Eq, PartialEq, Debug)]
pub(crate) enum StandardParserState {
    Params,
    Intermediates,
    Finished,
    Invalid,
}

#[derive(Eq, PartialEq, Debug)]
pub struct StandardParser {
    pub(crate) state: StandardParserState,
    pub params: Vec<u8>,
    pub intermediates: Vec<u8>,

    // Internal trace of recent bytes for diagnostics.
    seq_trace: SequenceTracer,
}

impl Default for StandardParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SequenceTraceable for StandardParser {
    #[inline]
    fn seq_tracer(&mut self) -> &mut SequenceTracer {
        &mut self.seq_trace
    }
    #[inline]
    fn seq_tracer_ref(&self) -> &SequenceTracer {
        &self.seq_trace
    }
}

impl StandardParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: StandardParserState::Intermediates,
            params: Vec::with_capacity(8),
            intermediates: Vec::with_capacity(8),
            seq_trace: SequenceTracer::new(),
        }
    }

    /// Expose current sequence trace for testing and diagnostics.
    #[must_use]
    pub fn trace_str(&self) -> String {
        self.seq_trace.as_str()
    }

    /// Push a byte into the parser
    ///
    /// # Errors
    /// Will return an error if the parser is in a finished state
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn push(&mut self, b: u8) -> ParserOutcome {
        self.append_trace(b);

        if let StandardParserState::Finished | StandardParserState::Invalid = &self.state {
            return ParserOutcome::Invalid("Parser pushed to after finish".to_string());
        }

        match self.state {
            StandardParserState::Intermediates => {
                if is_standard_intermediate_final(b) {
                    self.state = StandardParserState::Finished;

                    self.seq_trace.trim_control_tail();
                    self.intermediates.push(b);

                    return ParserOutcome::Finished;
                } else if is_standard_intermediate_continue(b) {
                    self.state = StandardParserState::Params;
                    self.intermediates.push(b);

                    return ParserOutcome::Continue;
                }

                self.state = StandardParserState::Invalid;

                ParserOutcome::Invalid("Invalid intermediate byte".to_string())
            }
            StandardParserState::Params => {
                if is_standard_param(b) {
                    self.params.push(b);
                    self.state = StandardParserState::Finished;

                    self.seq_trace.trim_control_tail();

                    return ParserOutcome::Finished;
                }

                self.state = StandardParserState::Invalid;

                ParserOutcome::Invalid("Invalid parameter byte".to_string())
            }

            _ => ParserOutcome::Continue,
        }
    }

    /// Push a byte into the parser and return the next state
    ///
    /// # Errors
    /// Will return an error if the parser encounters an invalid state
    // Inherently large: standard (non-CSI) escape sequence dispatch. Each arm handles a
    // distinct two-character escape code. Splitting would break a coherent dispatch table.
    #[allow(clippy::too_many_lines)]
    pub fn standard_parser_inner(
        &mut self,
        b: u8,
        output: &mut Vec<TerminalOutput>,
    ) -> ParserOutcome {
        let return_state = self.push(b);

        if let ParserOutcome::Invalid(_) = return_state {
            return return_state;
        }

        match self.state {
            StandardParserState::Finished => match self.intermediates.first() {
                None => ParserOutcome::Invalid("No intermediates".to_string()),
                Some(b' ') => {
                    let value = self.params.first();

                    match value {
                        None => ParserOutcome::Invalid("No params".to_string()),
                        Some(value) => {
                            let value = *value as char;
                            match value {
                                'F' => output.push(TerminalOutput::SevenBitControl),
                                'G' => output.push(TerminalOutput::EightBitControl),
                                'L' => output.push(TerminalOutput::AnsiConformanceLevelOne),
                                'M' => output.push(TerminalOutput::AnsiConformanceLevelTwo),
                                'N' => output.push(TerminalOutput::AnsiConformanceLevelThree),
                                _ => {
                                    output.push(TerminalOutput::Invalid);
                                    return ParserOutcome::Invalid(
                                        "Invalid param value".to_string(),
                                    );
                                }
                            }

                            ParserOutcome::Finished
                        }
                    }
                }
                Some(b'#') => {
                    let value = self.params.first();

                    match value {
                        None => ParserOutcome::Invalid("No params".to_string()),
                        Some(value) => {
                            let value = *value as char;
                            match value {
                                '3' => output.push(TerminalOutput::DoubleLineHeightTop),
                                '4' => output.push(TerminalOutput::DoubleLineHeightBottom),
                                '5' => output.push(TerminalOutput::SingleWidthLine),
                                '6' => output.push(TerminalOutput::DoubleWidthLine),
                                '8' => output.push(TerminalOutput::ScreenAlignmentTest),
                                _ => {
                                    output.push(TerminalOutput::Invalid);
                                    return ParserOutcome::Invalid(
                                        "Invalid param value".to_string(),
                                    );
                                }
                            }

                            ParserOutcome::Finished
                        }
                    }
                }
                Some(b'%') => {
                    let value = self.params.first();

                    match value {
                        None => ParserOutcome::Invalid("No params".to_string()),
                        Some(value) => {
                            let value = *value as char;
                            match value {
                                '@' => output.push(TerminalOutput::CharsetDefault),
                                'G' => output.push(TerminalOutput::CharsetUTF8),
                                _ => {
                                    output.push(TerminalOutput::Invalid);
                                    return ParserOutcome::Invalid(
                                        "Invalid param value".to_string(),
                                    );
                                }
                            }

                            ParserOutcome::Finished
                        }
                    }
                }
                Some(b'(') => {
                    let value = self.params.first();

                    match value {
                        None => ParserOutcome::Invalid("No params".to_string()),
                        Some(value) => {
                            let value = *value as char;

                            match value {
                                '0' => output.push(TerminalOutput::DecSpecialGraphics(
                                    DecSpecialGraphics::Replace,
                                )),
                                'B' => output.push(TerminalOutput::DecSpecialGraphics(
                                    DecSpecialGraphics::DontReplace,
                                )),
                                'C' => output.push(TerminalOutput::CharsetG0),
                                _ => {
                                    output.push(TerminalOutput::Invalid);
                                    return ParserOutcome::Invalid(
                                        "Invalid param value".to_string(),
                                    );
                                }
                            }
                            ParserOutcome::Finished
                        }
                    }
                }
                Some(b')') => {
                    let value = self.params.first();

                    match value {
                        None => ParserOutcome::Invalid("No params".to_string()),
                        Some(value) => {
                            match *value {
                                b'0' | b'B' | b'C' => {
                                    // G1 designation: '0' = DEC Special, 'B' = US-ASCII,
                                    // 'C' = Finnish NRC.  Freminal uses a simplified
                                    // single-slot charset model — G1 designations are
                                    // silently accepted but do not change state.
                                    output.push(TerminalOutput::CharsetG1);
                                }
                                _ => {
                                    output.push(TerminalOutput::Invalid);
                                    return ParserOutcome::Invalid(
                                        "Invalid param value".to_string(),
                                    );
                                }
                            }

                            ParserOutcome::Finished
                        }
                    }
                }
                Some(b'*') => {
                    let value = self.params.first();

                    match value {
                        None => ParserOutcome::Invalid("No params".to_string()),
                        Some(value) => {
                            if *value == b'C' {
                                output.push(TerminalOutput::CharsetG2);
                            } else {
                                output.push(TerminalOutput::Invalid);
                                return ParserOutcome::Invalid("Invalid param value".to_string());
                            }
                            ParserOutcome::Finished
                        }
                    }
                }
                Some(b'+') => {
                    let value = self.params.first();

                    match value {
                        None => ParserOutcome::Invalid("No params".to_string()),
                        Some(value) => {
                            match value {
                                b'0' => output.push(TerminalOutput::DecSpecialGraphics(
                                    DecSpecialGraphics::Replace,
                                )),
                                b'A' => output.push(TerminalOutput::CharsetUK),
                                b'B' => output.push(TerminalOutput::CharsetUSASCII),
                                b'4' => output.push(TerminalOutput::CharsetDutch),
                                b'5' | b'C' => output.push(TerminalOutput::CharsetFinnish),
                                b'R' => output.push(TerminalOutput::CharsetFrench),
                                b'Q' => output.push(TerminalOutput::CharsetFrenchCanadian),
                                b'K' => output.push(TerminalOutput::CharsetGerman),
                                b'Y' => output.push(TerminalOutput::CharsetItalian),
                                b'E' | b'6' => output.push(TerminalOutput::CharsetNorwegianDanish),
                                b'Z' => output.push(TerminalOutput::CharsetSpanish),
                                b'H' | b'7' => output.push(TerminalOutput::CharsetSwedish),
                                b'=' => output.push(TerminalOutput::CharsetSwiss),
                                _ => {
                                    output.push(TerminalOutput::Invalid);
                                    return ParserOutcome::Invalid(
                                        "Invalid param value".to_string(),
                                    );
                                }
                            }

                            ParserOutcome::Finished
                        }
                    }
                }
                Some(value) => {
                    let value = *value as char;
                    match value {
                        '7' => output.push(TerminalOutput::SaveCursor),
                        '8' => output.push(TerminalOutput::RestoreCursor),
                        '=' => output.push(TerminalOutput::ApplicationKeypadMode),
                        '>' => output.push(TerminalOutput::NormalKeypadMode),
                        'F' => output.push(TerminalOutput::CursorToLowerLeftCorner),
                        'c' => output.push(TerminalOutput::ResetDevice),
                        'l' => output.push(TerminalOutput::MemoryLock),
                        'm' => output.push(TerminalOutput::MemoryUnlock),
                        'n' => output.push(TerminalOutput::CharsetG2AsGL),
                        'o' => output.push(TerminalOutput::CharsetG3AsGL),
                        '|' => output.push(TerminalOutput::CharsetG3AsGR),
                        '}' => output.push(TerminalOutput::CharsetG2AsGR),
                        '~' => output.push(TerminalOutput::CharsetG1AsGR),
                        'M' => output.push(TerminalOutput::ReverseIndex),
                        'D' => output.push(TerminalOutput::Index),
                        'E' => output.push(TerminalOutput::NextLine),
                        'H' => output.push(TerminalOutput::HorizontalTabSet),
                        _ => {
                            output.push(TerminalOutput::Invalid);
                            return ParserOutcome::Invalid(
                                "Invalid intermediate value".to_string(),
                            );
                        }
                    }

                    ParserOutcome::Finished
                }
            },
            StandardParserState::Invalid => {
                ParserOutcome::Invalid("Invalid parser state".to_string())
            }
            _ => ParserOutcome::Continue,
        }
    }
}

#[must_use]
pub const fn is_standard_intermediate_final(b: u8) -> bool {
    // 7 8 = > F H c l m n o | } ~ are final and we want to enter the finished state
    // H (0x48) is HTS — Horizontal Tab Set

    matches!(
        b,
        0x7 | 0x8
            | 0x3e
            | 0x46
            | 0x48
            | 0x63
            | 0x6c
            | 0x6d
            | 0x6e
            | 0x6f
            | 0x7c
            | 0x7d
            | 0x7e
            | 0x3d
            | 0x37
            | 0x38
            | 0x4d
            | 0x44
            | 0x45
    )
}

#[must_use]
pub const fn is_standard_intermediate_continue(b: u8) -> bool {
    // space # % ( ) * + are states where we want to continue and get a Params

    matches!(b, 0x20 | 0x23 | 0x25 | 0x28 | 0x29 | 0x2a | 0x2b)
}

#[must_use]
pub const fn is_standard_param(b: u8) -> bool {
    // F G L M N 3 4 5 6 8 @ G C 0 A B 4 5 R Q K Y E Z H 7 = are valid params

    matches!(
        b,
        0x46 | 0x47
            | 0x4c
            | 0x4d
            | 0x4e
            | 0x33
            | 0x34
            | 0x35
            | 0x36
            | 0x38
            | 0x40
            | 0x43
            | 0x30
            | 0x41
            | 0x42
            | 0x52
            | 0x51
            | 0x4b
            | 0x59
            | 0x45
            | 0x5a
            | 0x48
            | 0x37
            | 0x3d
    )
}

#[cfg(test)]
mod tests {
    use super::StandardParser;
    use crate::ansi::ParserOutcome;
    use freminal_common::buffer_states::line_draw::DecSpecialGraphics;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    /// Feed a two-byte standard escape sequence through the parser.
    /// The first byte is the intermediate (e.g. `b'#'`), the second is the param/final.
    fn feed_standard(intermediate: u8, param: u8) -> (Vec<TerminalOutput>, ParserOutcome) {
        let mut parser = StandardParser::new();
        let mut output = Vec::new();
        parser.standard_parser_inner(intermediate, &mut output);
        let result = parser.standard_parser_inner(param, &mut output);
        (output, result)
    }

    /// Feed a single-byte standard escape final (no intermediate continuation).
    fn feed_standard_final(b: u8) -> (Vec<TerminalOutput>, ParserOutcome) {
        let mut parser = StandardParser::new();
        let mut output = Vec::new();
        let result = parser.standard_parser_inner(b, &mut output);
        (output, result)
    }

    // ------------------------------------------------------------------
    // State machine — invalid transitions
    // ------------------------------------------------------------------

    #[test]
    fn invalid_first_byte_transitions_to_invalid() {
        let mut parser = StandardParser::new();
        let mut output = Vec::new();
        // 0x01 is not a valid intermediate or final byte
        let result = parser.standard_parser_inner(0x01, &mut output);
        assert!(matches!(result, ParserOutcome::Invalid(_)));
    }

    #[test]
    fn invalid_param_byte_in_waiting_for_final_state() {
        // Feed a valid intermediate first ('#'), then an invalid param byte (0x01)
        let mut parser = StandardParser::new();
        let mut output = Vec::new();
        parser.standard_parser_inner(b'#', &mut output);
        // Now in Params state; 0x01 is not a valid param byte
        let result = parser.standard_parser_inner(0x01, &mut output);
        assert!(matches!(result, ParserOutcome::Invalid(_)));
    }

    #[test]
    fn push_after_finish_returns_invalid() {
        let mut parser = StandardParser::new();
        let mut output = Vec::new();
        // Feed a valid two-byte sequence to completion
        parser.standard_parser_inner(b'#', &mut output);
        parser.standard_parser_inner(b'8', &mut output);
        // Pushing again after finish
        let result = parser.standard_parser_inner(b'x', &mut output);
        assert!(matches!(result, ParserOutcome::Invalid(_)));
    }

    // ------------------------------------------------------------------
    // ESC # — DEC double/single height/width + DECALN
    // ------------------------------------------------------------------

    #[test]
    fn esc_hash_3_double_line_height_top() {
        let (output, result) = feed_standard(b'#', b'3');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::DoubleLineHeightTop]);
    }

    #[test]
    fn esc_hash_4_double_line_height_bottom() {
        let (output, result) = feed_standard(b'#', b'4');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::DoubleLineHeightBottom]);
    }

    #[test]
    fn esc_hash_5_single_width_line() {
        let (output, result) = feed_standard(b'#', b'5');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::SingleWidthLine]);
    }

    #[test]
    fn esc_hash_6_double_width_line() {
        let (output, result) = feed_standard(b'#', b'6');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::DoubleWidthLine]);
    }

    #[test]
    fn esc_hash_8_screen_alignment_test() {
        let (output, result) = feed_standard(b'#', b'8');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::ScreenAlignmentTest]);
    }

    #[test]
    fn esc_hash_invalid_param() {
        // b'0' passes is_standard_param() but is not valid for the '#' dispatch.
        // So push() succeeds (Finished), then dispatch pushes TerminalOutput::Invalid.
        let (output, result) = feed_standard(b'#', b'0');
        assert!(matches!(result, ParserOutcome::Invalid(_)));
        assert!(output.contains(&TerminalOutput::Invalid));
    }

    #[test]
    fn esc_hash_no_param_returns_invalid() {
        // intermediate '#' with no valid params state → no intermediates match when None
        let mut parser = StandardParser::new();
        let mut output = Vec::new();
        // Only feed intermediate, never a param
        let result = parser.standard_parser_inner(b'#', &mut output);
        // Should continue (waiting for param)
        assert!(matches!(result, ParserOutcome::Continue));
    }

    // ------------------------------------------------------------------
    // ESC % — charset default / UTF-8
    // ------------------------------------------------------------------

    #[test]
    fn esc_percent_at_charset_default() {
        let (output, result) = feed_standard(b'%', b'@');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetDefault]);
    }

    #[test]
    fn esc_percent_g_charset_utf8() {
        let (output, result) = feed_standard(b'%', b'G');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetUTF8]);
    }

    #[test]
    fn esc_percent_invalid_param() {
        // b'0' passes is_standard_param() but is not valid for the '%' dispatch.
        let (output, result) = feed_standard(b'%', b'0');
        assert!(matches!(result, ParserOutcome::Invalid(_)));
        assert!(output.contains(&TerminalOutput::Invalid));
    }

    // ------------------------------------------------------------------
    // ESC ( — G0 charset
    // ------------------------------------------------------------------

    #[test]
    fn esc_paren_0_dec_special_graphics_replace() {
        let (output, result) = feed_standard(b'(', b'0');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(
            output,
            vec![TerminalOutput::DecSpecialGraphics(
                DecSpecialGraphics::Replace
            )]
        );
    }

    #[test]
    fn esc_paren_b_dec_special_graphics_dont_replace() {
        let (output, result) = feed_standard(b'(', b'B');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(
            output,
            vec![TerminalOutput::DecSpecialGraphics(
                DecSpecialGraphics::DontReplace
            )]
        );
    }

    #[test]
    fn esc_paren_c_charset_g0() {
        let (output, result) = feed_standard(b'(', b'C');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetG0]);
    }

    #[test]
    fn esc_paren_invalid_param() {
        let (output, result) = feed_standard(b'(', b'Z');
        assert!(matches!(result, ParserOutcome::Invalid(_)));
        assert!(output.contains(&TerminalOutput::Invalid));
    }

    // ------------------------------------------------------------------
    // ESC ) — G1 charset designators
    // ------------------------------------------------------------------

    #[test]
    fn esc_close_paren_0_charset_g1() {
        let (output, result) = feed_standard(b')', b'0');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetG1]);
    }

    #[test]
    fn esc_close_paren_b_charset_g1() {
        let (output, result) = feed_standard(b')', b'B');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetG1]);
    }

    #[test]
    fn esc_close_paren_c_charset_g1() {
        let (output, result) = feed_standard(b')', b'C');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetG1]);
    }

    #[test]
    fn esc_close_paren_invalid_param() {
        let (output, result) = feed_standard(b')', b'Z');
        assert!(matches!(result, ParserOutcome::Invalid(_)));
        assert!(output.contains(&TerminalOutput::Invalid));
    }

    // ------------------------------------------------------------------
    // ESC * — G2 charset
    // ------------------------------------------------------------------

    #[test]
    fn esc_star_c_charset_g2() {
        let (output, result) = feed_standard(b'*', b'C');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetG2]);
    }

    #[test]
    fn esc_star_invalid_param() {
        let (output, result) = feed_standard(b'*', b'Z');
        assert!(matches!(result, ParserOutcome::Invalid(_)));
        assert!(output.contains(&TerminalOutput::Invalid));
    }

    // ------------------------------------------------------------------
    // ESC + — national character set designations
    // ------------------------------------------------------------------

    #[test]
    fn esc_plus_0_dec_special_graphics() {
        let (output, result) = feed_standard(b'+', b'0');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(
            output,
            vec![TerminalOutput::DecSpecialGraphics(
                DecSpecialGraphics::Replace
            )]
        );
    }

    #[test]
    fn esc_plus_a_charset_uk() {
        let (output, result) = feed_standard(b'+', b'A');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetUK]);
    }

    #[test]
    fn esc_plus_b_charset_us_ascii() {
        let (output, result) = feed_standard(b'+', b'B');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetUSASCII]);
    }

    #[test]
    fn esc_plus_4_charset_dutch() {
        let (output, result) = feed_standard(b'+', b'4');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetDutch]);
    }

    #[test]
    fn esc_plus_5_charset_finnish() {
        let (output, result) = feed_standard(b'+', b'5');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetFinnish]);
    }

    #[test]
    fn esc_plus_r_charset_french() {
        let (output, result) = feed_standard(b'+', b'R');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetFrench]);
    }

    #[test]
    fn esc_plus_q_charset_french_canadian() {
        let (output, result) = feed_standard(b'+', b'Q');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetFrenchCanadian]);
    }

    #[test]
    fn esc_plus_k_charset_german() {
        let (output, result) = feed_standard(b'+', b'K');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetGerman]);
    }

    #[test]
    fn esc_plus_y_charset_italian() {
        let (output, result) = feed_standard(b'+', b'Y');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetItalian]);
    }

    #[test]
    fn esc_plus_e_charset_norwegian_danish() {
        let (output, result) = feed_standard(b'+', b'E');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetNorwegianDanish]);
    }

    #[test]
    fn esc_plus_z_charset_spanish() {
        let (output, result) = feed_standard(b'+', b'Z');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetSpanish]);
    }

    #[test]
    fn esc_plus_h_charset_swedish() {
        let (output, result) = feed_standard(b'+', b'H');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetSwedish]);
    }

    #[test]
    fn esc_plus_eq_charset_swiss() {
        let (output, result) = feed_standard(b'+', b'=');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetSwiss]);
    }

    #[test]
    fn esc_plus_c_charset_finnish_alternate() {
        let (output, result) = feed_standard(b'+', b'C');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetFinnish]);
    }

    #[test]
    fn esc_plus_6_charset_norwegian_danish_alternate() {
        let (output, result) = feed_standard(b'+', b'6');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetNorwegianDanish]);
    }

    #[test]
    fn esc_plus_7_charset_swedish_alternate() {
        let (output, result) = feed_standard(b'+', b'7');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetSwedish]);
    }

    #[test]
    fn esc_plus_invalid_param() {
        // b'F' passes is_standard_param() but is not valid for the '+' dispatch.
        let (output, result) = feed_standard(b'+', b'F');
        assert!(matches!(result, ParserOutcome::Invalid(_)));
        assert!(output.contains(&TerminalOutput::Invalid));
    }

    // ------------------------------------------------------------------
    // ESC   (space) — 7-bit/8-bit control & ANSI conformance
    // ------------------------------------------------------------------

    #[test]
    fn esc_space_f_seven_bit_control() {
        let (output, result) = feed_standard(b' ', b'F');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::SevenBitControl]);
    }

    #[test]
    fn esc_space_g_eight_bit_control() {
        let (output, result) = feed_standard(b' ', b'G');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::EightBitControl]);
    }

    #[test]
    fn esc_space_l_ansi_conformance_level_one() {
        let (output, result) = feed_standard(b' ', b'L');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::AnsiConformanceLevelOne]);
    }

    #[test]
    fn esc_space_m_ansi_conformance_level_two() {
        let (output, result) = feed_standard(b' ', b'M');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::AnsiConformanceLevelTwo]);
    }

    #[test]
    fn esc_space_n_ansi_conformance_level_three() {
        let (output, result) = feed_standard(b' ', b'N');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::AnsiConformanceLevelThree]);
    }

    #[test]
    fn esc_space_invalid_param() {
        // b'0' passes is_standard_param() but is not valid for the ' ' dispatch.
        let (output, result) = feed_standard(b' ', b'0');
        assert!(matches!(result, ParserOutcome::Invalid(_)));
        assert!(output.contains(&TerminalOutput::Invalid));
    }

    // ------------------------------------------------------------------
    // Single-byte finals (no intermediate)
    // ------------------------------------------------------------------

    #[test]
    fn esc_7_save_cursor() {
        let (output, result) = feed_standard_final(b'7');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::SaveCursor]);
    }

    #[test]
    fn esc_8_restore_cursor() {
        let (output, result) = feed_standard_final(b'8');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::RestoreCursor]);
    }

    #[test]
    fn esc_eq_application_keypad() {
        let (output, result) = feed_standard_final(b'=');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::ApplicationKeypadMode]);
    }

    #[test]
    fn esc_gt_normal_keypad() {
        let (output, result) = feed_standard_final(b'>');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::NormalKeypadMode]);
    }

    #[test]
    fn esc_upper_m_reverse_index() {
        let (output, result) = feed_standard_final(b'M');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::ReverseIndex]);
    }

    #[test]
    fn esc_upper_d_index() {
        let (output, result) = feed_standard_final(b'D');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::Index]);
    }

    #[test]
    fn esc_upper_e_next_line() {
        let (output, result) = feed_standard_final(b'E');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::NextLine]);
    }

    #[test]
    fn esc_upper_h_horizontal_tab_set() {
        let (output, result) = feed_standard_final(b'H');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::HorizontalTabSet]);
    }

    #[test]
    fn esc_upper_f_cursor_to_lower_left() {
        let (output, result) = feed_standard_final(b'F');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CursorToLowerLeftCorner]);
    }

    #[test]
    fn esc_lower_c_reset_device() {
        let (output, result) = feed_standard_final(b'c');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::ResetDevice]);
    }

    #[test]
    fn esc_lower_n_charset_g2_as_gl() {
        let (output, result) = feed_standard_final(b'n');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetG2AsGL]);
    }

    #[test]
    fn esc_lower_o_charset_g3_as_gl() {
        let (output, result) = feed_standard_final(b'o');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetG3AsGL]);
    }

    #[test]
    fn esc_pipe_charset_g3_as_gr() {
        let (output, result) = feed_standard_final(b'|');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetG3AsGR]);
    }

    #[test]
    fn esc_close_brace_charset_g2_as_gr() {
        let (output, result) = feed_standard_final(b'}');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetG2AsGR]);
    }

    #[test]
    fn esc_tilde_charset_g1_as_gr() {
        let (output, result) = feed_standard_final(b'~');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::CharsetG1AsGR]);
    }

    #[test]
    fn esc_unknown_final_returns_invalid() {
        // 0x07 (BEL) passes is_standard_intermediate_final() but hits the '_' arm
        // in the dispatch (not a valid ESC sequence final), producing TerminalOutput::Invalid.
        let (output, result) = feed_standard_final(0x07);
        assert!(matches!(result, ParserOutcome::Invalid(_)));
        assert!(output.contains(&TerminalOutput::Invalid));
    }

    // ------------------------------------------------------------------
    // trace_str coverage
    // ------------------------------------------------------------------

    #[test]
    fn trace_str_returns_string() {
        let mut parser = StandardParser::new();
        let mut output = Vec::new();
        parser.standard_parser_inner(b'#', &mut output);
        parser.standard_parser_inner(b'8', &mut output);
        let _ = parser.trace_str();
    }
}
