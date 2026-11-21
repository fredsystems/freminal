// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::charset::{CharacterSet, CharsetSlot};
use freminal_common::buffer_states::terminal_output::TerminalOutput;

use crate::ansi::ParserOutcome;
use crate::ansi_components::tracer::lossy_sequence_for_log_bounded;

/// How many intermediate bytes a [`StandardParser`] keeps. The intermediates
/// are read for dispatch (the first) and the debug log, so a designator's run
/// of extra ones (`ESC ( % % % ...`) is swallowed past this count without
/// being stored: the sequence ends only at its final, so the run is otherwise
/// unbounded.
const MAX_STORED_INTERMEDIATES: usize = 8;

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
}

impl Default for StandardParser {
    fn default() -> Self {
        Self::new()
    }
}

impl StandardParser {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: StandardParserState::Intermediates,
            params: Vec::with_capacity(8),
            intermediates: Vec::with_capacity(8),
        }
    }

    /// Expose the current sequence for testing and diagnostics.
    ///
    /// Rendered from the accepted bytes the parser already holds: the
    /// intermediates followed by the parameter byte (which is the order they
    /// arrive in). Bytes rejected as invalid are not retained. Lossily decoded
    /// and bounded to
    /// [`crate::ansi_components::tracer::LOG_SEQUENCE_MAX_BYTES`].
    #[must_use]
    pub fn trace_str(&self) -> String {
        let mut bytes = Vec::with_capacity(self.intermediates.len() + self.params.len());
        bytes.extend_from_slice(&self.intermediates);
        bytes.extend_from_slice(&self.params);
        lossy_sequence_for_log_bounded(&bytes)
    }

    /// Push a byte into the parser
    ///
    /// # Errors
    /// Will return an error if the parser is in a finished state
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn push(&mut self, b: u8) -> ParserOutcome {
        if let StandardParserState::Finished | StandardParserState::Invalid = &self.state {
            return ParserOutcome::Invalid("Parser pushed to after finish".to_string());
        }

        match self.state {
            StandardParserState::Intermediates => {
                if is_standard_intermediate_final(b) {
                    self.state = StandardParserState::Finished;
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
                // An ESC abandons the sequence and starts a new one (xterm's
                // `CASE_ESC` in every escape-intermediate table).
                if b == 0x1b {
                    self.state = StandardParserState::Finished;

                    return ParserOutcome::FinishedAtEsc;
                }

                if is_standard_param(b) || self.is_consumed_final(b) {
                    self.params.push(b);
                    self.state = StandardParserState::Finished;

                    return ParserOutcome::Finished;
                }

                // A designator takes further intermediates (0x20-0x2F) before
                // its final, as in `ESC ( % 5` or `ESC ( " 4`. xterm acts on
                // these multi-byte designators (`CASE_SCS_PERCENT`,
                // `CASE_SCS_DQUOTE`, `CASE_SCS_AMPRSND`: DEC Supplemental
                // Graphics, Hebrew, Cyrillic, ...), but freminal has none of
                // those sets: it deliberately recognises and ignores the whole
                // sequence up to its final byte, leaving the slot unchanged.
                if self.consumes_through_final() && (0x20..=0x2f).contains(&b) {
                    if self.intermediates.len() < MAX_STORED_INTERMEDIATES {
                        self.intermediates.push(b);
                    }

                    return ParserOutcome::Continue;
                }

                self.state = StandardParserState::Invalid;

                ParserOutcome::Invalid("Invalid parameter byte".to_string())
            }

            _ => ParserOutcome::Continue,
        }
    }

    /// Whether the sequence so far is one freminal consumes whole, through a
    /// final byte after any number of further intermediates, instead of
    /// matching it against a table of two-byte sequences: a designator
    /// (`ESC ( ) * +`, whose supported sets are dispatched, the rest ignored),
    /// a 96-character designator (`ESC - . /`) or a multi-byte set (`ESC $`).
    /// None of the last two names a set freminal implements; xterm's tables
    /// likewise end them at the first byte in 0x30-0x7E (`scs96table`,
    /// `eigtable`) rather than treating the final as text.
    fn consumes_through_final(&self) -> bool {
        self.intermediates
            .first()
            .is_some_and(|i| designated_slot(*i).is_some() || is_ignored_designator(*i))
    }

    /// Whether the sequence so far is [`Self::consumes_through_final`] and `b`
    /// is in the range its final byte may take. The final need not name a set
    /// freminal implements: an unknown one is ignored, not invalid.
    fn is_consumed_final(&self, b: u8) -> bool {
        self.consumes_through_final() && (0x30..=0x7e).contains(&b)
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

        if let ParserOutcome::Invalid(_) | ParserOutcome::FinishedAtEsc = return_state {
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
                Some(&intermediate @ (b'(' | b')' | b'*' | b'+')) => {
                    let (Some(slot), Some(&final_byte)) =
                        (designated_slot(intermediate), self.params.first())
                    else {
                        return ParserOutcome::Invalid("No params".to_string());
                    };

                    // A second intermediate makes this a multi-byte designator
                    // (`ESC ( % 5`), which freminal has none of; an unknown
                    // final is likewise not a set it implements. Either way
                    // the slot is left as it was and nothing is emitted.
                    if self.intermediates.len() > 1 {
                        debug!(
                            "Ignoring multi-byte charset designator: {}",
                            self.trace_str()
                        );
                    } else if let Some(set) = CharacterSet::from_designator_final(final_byte) {
                        output.push(TerminalOutput::DesignateCharset { slot, set });
                    } else {
                        debug!("Ignoring unknown charset designator final 0x{final_byte:02x}");
                    }

                    ParserOutcome::Finished
                }
                // `ESC - A`, `ESC $ B`, `ESC $ ( B`: designators for sets
                // freminal does not have. Consumed through the final byte,
                // which is not text.
                Some(&intermediate) if is_ignored_designator(intermediate) => {
                    debug!(
                        "Ignoring charset designator for an unsupported set: {}",
                        self.trace_str()
                    );

                    ParserOutcome::Finished
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
                        'N' => output.push(TerminalOutput::SingleShift(CharsetSlot::G2)),
                        'O' => output.push(TerminalOutput::SingleShift(CharsetSlot::G3)),
                        'n' => output.push(TerminalOutput::InvokeGl(CharsetSlot::G2)),
                        'o' => output.push(TerminalOutput::InvokeGl(CharsetSlot::G3)),
                        '|' => output.push(TerminalOutput::InvokeGr(CharsetSlot::G3)),
                        '}' => output.push(TerminalOutput::InvokeGr(CharsetSlot::G2)),
                        '~' => output.push(TerminalOutput::InvokeGr(CharsetSlot::G1)),
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

/// The slot a designator intermediate (`(`, `)`, `*`, `+`) designates into.
const fn designated_slot(intermediate: u8) -> Option<CharsetSlot> {
    match intermediate {
        b'(' => Some(CharsetSlot::G0),
        b')' => Some(CharsetSlot::G1),
        b'*' => Some(CharsetSlot::G2),
        b'+' => Some(CharsetSlot::G3),
        _ => None,
    }
}

/// Whether `intermediate` starts a designator for a set freminal has none of:
/// the 96-character designators `-` `.` `/` (G1-G3) and the multi-byte-set
/// designator `$`.
const fn is_ignored_designator(intermediate: u8) -> bool {
    matches!(intermediate, b'-' | b'.' | b'/' | b'$')
}

#[must_use]
pub const fn is_standard_intermediate_final(b: u8) -> bool {
    // 7 8 = > F H N O c l m n o | } ~ are final and we want to enter the finished state
    // H (0x48) is HTS — Horizontal Tab Set
    // N (0x4E) and O (0x4F) are SS2 and SS3

    matches!(
        b,
        0x7 | 0x8
            | 0x3e
            | 0x46
            | 0x48
            | 0x4e
            | 0x4f
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
    // space # $ % ( ) * + - . / are states where we want to continue and get a Params

    matches!(
        b,
        0x20 | 0x23 | 0x24 | 0x25 | 0x28 | 0x29 | 0x2a | 0x2b | 0x2d | 0x2e | 0x2f
    )
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
    use super::{MAX_STORED_INTERMEDIATES, StandardParser};
    use crate::ansi::ParserOutcome;
    use freminal_common::buffer_states::charset::{CharacterSet, CharsetSlot};
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
    // ESC ( ) * + — G0-G3 designation
    // ------------------------------------------------------------------

    const DESIGNATORS: [(u8, CharsetSlot); 4] = [
        (b'(', CharsetSlot::G0),
        (b')', CharsetSlot::G1),
        (b'*', CharsetSlot::G2),
        (b'+', CharsetSlot::G3),
    ];

    #[test]
    fn every_designator_designates_every_known_set_into_its_own_slot() {
        for (intermediate, slot) in DESIGNATORS {
            for (final_byte, set) in [
                (b'B', CharacterSet::Ascii),
                (b'A', CharacterSet::UnitedKingdom),
                (b'0', CharacterSet::DecSpecialGraphics),
            ] {
                let (output, result) = feed_standard(intermediate, final_byte);
                assert!(
                    matches!(result, ParserOutcome::Finished),
                    "ESC {} {} must finish",
                    intermediate as char,
                    final_byte as char
                );
                assert_eq!(
                    output,
                    vec![TerminalOutput::DesignateCharset { slot, set }],
                    "ESC {} {}",
                    intermediate as char,
                    final_byte as char
                );
            }
        }
    }

    #[test]
    fn esc_plus_0_designates_only_g3() {
        let (output, result) = feed_standard(b'+', b'0');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(
            output,
            vec![TerminalOutput::DesignateCharset {
                slot: CharsetSlot::G3,
                set: CharacterSet::DecSpecialGraphics,
            }]
        );
    }

    #[test]
    fn unknown_designator_final_finishes_with_no_output() {
        // Finals for sets freminal does not implement (Finnish, Dutch, French,
        // ...), and finals that are not character sets at all, leave the slot
        // as it was: the sequence is consumed and emits nothing.
        for (intermediate, _) in DESIGNATORS {
            for &final_byte in b"C54RQKYEZH=Fx~" {
                let (output, result) = feed_standard(intermediate, final_byte);
                assert!(
                    matches!(result, ParserOutcome::Finished),
                    "ESC {} {} must finish",
                    intermediate as char,
                    final_byte as char
                );
                assert!(
                    output.is_empty(),
                    "ESC {} {} must emit nothing, got {output:?}",
                    intermediate as char,
                    final_byte as char
                );
            }
        }
    }

    #[test]
    fn designator_final_outside_the_final_range_is_invalid() {
        for (intermediate, _) in DESIGNATORS {
            for final_byte in [0x01, 0x1f, 0x7f] {
                let (output, result) = feed_standard(intermediate, final_byte);
                assert!(
                    matches!(result, ParserOutcome::Invalid(_)),
                    "ESC {} 0x{final_byte:02x} must be invalid",
                    intermediate as char
                );
                assert_eq!(output, Vec::<TerminalOutput>::new());
            }
        }
    }

    #[test]
    fn designator_takes_extra_intermediates_and_ignores_the_whole_sequence() {
        for (intermediate, _) in DESIGNATORS {
            for extra in [&b"%"[..], b"\"", b" ", b"/", b"!#"] {
                let mut parser = StandardParser::new();
                let mut output = Vec::new();
                let mut results = vec![parser.standard_parser_inner(intermediate, &mut output)];
                for &b in extra {
                    results.push(parser.standard_parser_inner(b, &mut output));
                }
                // The final: a known set's final too, since the designator is
                // a multi-byte one.
                results.push(parser.standard_parser_inner(b'B', &mut output));
                let (last, before) = results.split_last().unwrap_or_else(|| unreachable!());
                assert!(
                    before.iter().all(|r| matches!(r, ParserOutcome::Continue)),
                    "ESC {} {extra:?}: every intermediate continues: {before:?}",
                    intermediate as char
                );
                assert!(matches!(last, ParserOutcome::Finished), "{last:?}");
                assert!(
                    output.is_empty(),
                    "ESC {} {extra:?} B must emit nothing, got {output:?}",
                    intermediate as char
                );
            }
        }
    }

    #[test]
    fn a_long_run_of_designator_intermediates_is_not_stored() {
        const RUN: usize = 1024 * 1024;

        let mut parser = StandardParser::new();
        let mut output = Vec::new();
        assert!(matches!(
            parser.standard_parser_inner(b'(', &mut output),
            ParserOutcome::Continue
        ));
        for _ in 0..RUN {
            assert!(matches!(
                parser.standard_parser_inner(b'%', &mut output),
                ParserOutcome::Continue
            ));
        }
        assert_eq!(parser.intermediates.len(), MAX_STORED_INTERMEDIATES);
        assert!(parser.intermediates.capacity() < 1024);

        // The final still ends the sequence, which is ignored as a whole.
        assert!(matches!(
            parser.standard_parser_inner(b'B', &mut output),
            ParserOutcome::Finished
        ));
        assert_eq!(output, Vec::<TerminalOutput>::new());
    }

    // ------------------------------------------------------------------
    // ESC - . / $ — designators for sets freminal does not have:
    // consumed through the final byte, which is not text.
    // ------------------------------------------------------------------

    const IGNORED_DESIGNATORS: [u8; 4] = *b"-./$";

    #[test]
    fn an_ignored_designator_is_consumed_through_its_final_and_emits_nothing() {
        for intermediate in IGNORED_DESIGNATORS {
            for final_byte in *b"AB0<~" {
                let (output, result) = feed_standard(intermediate, final_byte);
                assert!(
                    matches!(result, ParserOutcome::Finished),
                    "ESC {} {}: {result:?}",
                    intermediate as char,
                    final_byte as char
                );
                assert_eq!(output, Vec::<TerminalOutput>::new());
            }
        }
    }

    #[test]
    fn an_ignored_designator_takes_extra_intermediates_before_its_final() {
        for intermediate in IGNORED_DESIGNATORS {
            for extra in [&b"("[..], b")", b"%", b"\"", b"!#", b" "] {
                let mut parser = StandardParser::new();
                let mut output = Vec::new();
                let mut results = vec![parser.standard_parser_inner(intermediate, &mut output)];
                for &b in extra {
                    results.push(parser.standard_parser_inner(b, &mut output));
                }
                results.push(parser.standard_parser_inner(b'B', &mut output));
                let (last, before) = results.split_last().unwrap_or_else(|| unreachable!());
                assert!(
                    before.iter().all(|r| matches!(r, ParserOutcome::Continue)),
                    "ESC {} {extra:?}: every intermediate continues: {before:?}",
                    intermediate as char
                );
                assert!(matches!(last, ParserOutcome::Finished), "{last:?}");
                assert_eq!(output, Vec::<TerminalOutput>::new());
            }
        }
    }

    #[test]
    fn an_ignored_designator_final_outside_the_final_range_is_invalid() {
        for intermediate in IGNORED_DESIGNATORS {
            for final_byte in [0x01, 0x1f, 0x7f] {
                let (_, result) = feed_standard(intermediate, final_byte);
                assert!(
                    matches!(result, ParserOutcome::Invalid(_)),
                    "ESC {} 0x{final_byte:02x}: {result:?}",
                    intermediate as char
                );
            }
        }
    }

    #[test]
    fn esc_abandons_an_ignored_designator() {
        for intermediate in IGNORED_DESIGNATORS {
            let (_, result) = feed_standard(intermediate, 0x1b);
            assert!(
                matches!(result, ParserOutcome::FinishedAtEsc),
                "ESC {} ESC: {result:?}",
                intermediate as char
            );
        }
    }

    #[test]
    fn a_long_run_of_intermediates_after_an_ignored_designator_is_not_stored() {
        for intermediate in IGNORED_DESIGNATORS {
            let mut parser = StandardParser::new();
            let mut output = Vec::new();
            parser.standard_parser_inner(intermediate, &mut output);
            for _ in 0..100_000 {
                parser.standard_parser_inner(b'(', &mut output);
            }
            assert_eq!(parser.intermediates.len(), MAX_STORED_INTERMEDIATES);
            assert!(matches!(
                parser.standard_parser_inner(b'B', &mut output),
                ParserOutcome::Finished
            ));
        }
    }

    #[test]
    fn only_a_designator_takes_extra_intermediates() {
        // `ESC # %` and `ESC SP %` have no meaning: still invalid.
        for first in *b"# %" {
            let (output, result) = feed_standard(first, b'%');
            assert!(matches!(result, ParserOutcome::Invalid(_)), "{result:?}");
            assert_eq!(output, Vec::<TerminalOutput>::new());
        }
    }

    #[test]
    fn esc_after_an_intermediate_abandons_the_sequence() {
        for first in *b"()*+ #%" {
            let (output, result) = feed_standard(first, 0x1b);
            assert!(
                matches!(result, ParserOutcome::FinishedAtEsc),
                "ESC {} ESC: {result:?}",
                first as char
            );
            assert_eq!(output, Vec::<TerminalOutput>::new());
        }
    }

    #[test]
    fn esc_after_a_designator_with_extra_intermediates_abandons_the_sequence() {
        let mut parser = StandardParser::new();
        let mut output = Vec::new();
        parser.standard_parser_inner(b'(', &mut output);
        parser.standard_parser_inner(b'%', &mut output);
        assert!(matches!(
            parser.standard_parser_inner(0x1b, &mut output),
            ParserOutcome::FinishedAtEsc
        ));
        assert_eq!(output, Vec::<TerminalOutput>::new());
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
    fn esc_lower_n_invokes_g2_into_gl() {
        let (output, result) = feed_standard_final(b'n');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::InvokeGl(CharsetSlot::G2)]);
    }

    #[test]
    fn esc_lower_o_invokes_g3_into_gl() {
        let (output, result) = feed_standard_final(b'o');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::InvokeGl(CharsetSlot::G3)]);
    }

    #[test]
    fn esc_pipe_invokes_g3_into_gr() {
        let (output, result) = feed_standard_final(b'|');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::InvokeGr(CharsetSlot::G3)]);
    }

    #[test]
    fn esc_close_brace_invokes_g2_into_gr() {
        let (output, result) = feed_standard_final(b'}');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::InvokeGr(CharsetSlot::G2)]);
    }

    #[test]
    fn esc_tilde_invokes_g1_into_gr() {
        let (output, result) = feed_standard_final(b'~');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::InvokeGr(CharsetSlot::G1)]);
    }

    #[test]
    fn esc_upper_n_is_single_shift_two() {
        let (output, result) = feed_standard_final(b'N');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::SingleShift(CharsetSlot::G2)]);
    }

    #[test]
    fn esc_upper_o_is_single_shift_three() {
        let (output, result) = feed_standard_final(b'O');
        assert!(matches!(result, ParserOutcome::Finished));
        assert_eq!(output, vec![TerminalOutput::SingleShift(CharsetSlot::G3)]);
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
