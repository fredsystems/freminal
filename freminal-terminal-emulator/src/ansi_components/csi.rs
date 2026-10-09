// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;

use super::csi_dispatch::{dispatch_csi, warn_unhandled};
use super::csi_key::CsiKey;
use crate::ansi_components::tracer::{SequenceTracer, escape_sequence_for_log};
use crate::{ansi::ParserOutcome, ansi_components::tracer::SequenceTraceable};

#[derive(Eq, PartialEq, Debug, Default)]
pub(crate) enum AnsiCsiParserState {
    #[default]
    Params,
    Intermediates,
    Finished(u8),
    Invalid,
    InvalidFinished,
}
/// Which Control Sequence Introducer began this CSI sequence.
///
/// The 7-bit `ESC [` form and the 8-bit `0x9B` (C1) form are equivalent
/// grammatically, but for lossless diagnostic logging they must be
/// reconstructed distinctly: the sub-parser never receives the introducer
/// bytes, so it records which one was used at construction time.
#[derive(Eq, PartialEq, Debug, Default, Clone, Copy)]
pub enum CsiIntroducer {
    /// 7-bit CSI: `ESC [` (`0x1B 0x5B`).
    #[default]
    SevenBit,
    /// 8-bit CSI: `0x9B` (C1).
    EightBit,
}

impl CsiIntroducer {
    /// The raw introducer bytes as they appear on the wire.
    const fn bytes(self) -> &'static [u8] {
        match self {
            Self::SevenBit => b"\x1b[",
            Self::EightBit => b"\x9b",
        }
    }
}

#[derive(Eq, PartialEq, Debug, Default)]
pub struct AnsiCsiParser {
    pub(crate) state: AnsiCsiParserState,
    pub params: Vec<u8>,
    pub intermediates: Vec<u8>,
    pub sequence: Vec<u8>,
    /// The introducer that began this sequence, for lossless diagnostics.
    introducer: CsiIntroducer,
    /// Internal trace of recent bytes for diagnostics.
    seq_trace: SequenceTracer,
}

impl SequenceTraceable for AnsiCsiParser {
    #[inline]
    fn seq_tracer(&mut self) -> &mut SequenceTracer {
        &mut self.seq_trace
    }
    #[inline]
    fn seq_tracer_ref(&self) -> &SequenceTracer {
        &self.seq_trace
    }
}

impl AnsiCsiParser {
    /// Construct a parser for a 7-bit (`ESC [`) CSI sequence.
    #[must_use]
    pub fn new() -> Self {
        Self::with_introducer(CsiIntroducer::SevenBit)
    }

    /// Construct a parser for an 8-bit (`0x9B`, C1) CSI sequence.
    #[must_use]
    pub fn new_c1() -> Self {
        Self::with_introducer(CsiIntroducer::EightBit)
    }

    #[must_use]
    fn with_introducer(introducer: CsiIntroducer) -> Self {
        Self {
            state: AnsiCsiParserState::Params,
            params: Vec::with_capacity(8),
            intermediates: Vec::with_capacity(4),
            sequence: Vec::with_capacity(16),
            introducer,
            seq_trace: SequenceTracer::new(),
        }
    }

    /// Expose current sequence trace for testing and diagnostics.
    #[must_use]
    pub fn trace_str(&self) -> String {
        self.seq_trace.as_str()
    }

    /// Render the full raw CSI sequence — including the reconstructed CSI
    /// introducer that the sub-parser never receives — as a
    /// reconstruction-faithful escaped string for diagnostics.
    ///
    /// The body is [`Self::sequence`], the accumulated CSI body bytes
    /// (parameters, intermediates, and the final byte). The actual introducer
    /// (`ESC [` for 7-bit, `0x9B` for 8-bit C1) is prepended so the logged form
    /// is the complete sequence a terminal would send.
    #[must_use]
    fn format_raw_csi(&self) -> String {
        let introducer = self.introducer.bytes();
        let mut full = Vec::with_capacity(self.sequence.len().saturating_add(introducer.len()));
        full.extend_from_slice(introducer);
        full.extend_from_slice(&self.sequence);
        escape_sequence_for_log(&full)
    }

    /// Push a byte into the parser
    ///
    /// # Errors
    /// Will return an error if the parser is in a finished state
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn push(&mut self, b: u8) -> ParserOutcome {
        self.append_trace(b);

        if let AnsiCsiParserState::Finished(_) | AnsiCsiParserState::InvalidFinished = &self.state {
            return ParserOutcome::Invalid("Parser pushed to once finished".to_string());
        }

        self.sequence.push(b);

        match &mut self.state {
            AnsiCsiParserState::Params => {
                if is_csi_param(b) {
                    self.params.push(b);
                    return ParserOutcome::Continue;
                } else if is_csi_intermediate(b) {
                    self.intermediates.push(b);
                    self.state = AnsiCsiParserState::Intermediates;
                    return ParserOutcome::Continue;
                } else if is_csi_terminator(b) {
                    self.state = AnsiCsiParserState::Finished(b);
                    self.seq_trace.trim_control_tail();
                    return ParserOutcome::Finished;
                }

                self.state = AnsiCsiParserState::Invalid;

                ParserOutcome::Invalid("Invalid CSI parameter".to_string())
            }
            AnsiCsiParserState::Intermediates => {
                if is_csi_param(b) {
                    self.state = AnsiCsiParserState::Invalid;

                    return ParserOutcome::Invalid("Invalid CSI intermediate".to_string());
                } else if is_csi_intermediate(b) {
                    self.intermediates.push(b);
                    return ParserOutcome::Continue;
                } else if is_csi_terminator(b) {
                    self.state = AnsiCsiParserState::Finished(b);
                    self.seq_trace.trim_control_tail();
                    return ParserOutcome::Finished;
                }

                self.state = AnsiCsiParserState::Invalid;

                ParserOutcome::Invalid("Invalid CSI intermediate".to_string())
            }
            AnsiCsiParserState::Invalid => {
                if is_csi_terminator(b) {
                    self.state = AnsiCsiParserState::InvalidFinished;
                }

                ParserOutcome::Invalid("Invalid CSI sequence".to_string())
            }
            AnsiCsiParserState::Finished(_) | AnsiCsiParserState::InvalidFinished => {
                // Guarded by the caller, but surface explicitly as an invalid
                // outcome rather than panicking if the invariant ever breaks.
                ParserOutcome::Invalid("CSI parser received byte after termination".to_string())
            }
        }
    }

    /// Push a byte into the parser and return the next state
    ///
    /// When the byte completes the sequence, the sequence is classified into a
    /// `CsiKey` and routed by `csi_dispatch::dispatch_csi`. A sequence whose private
    /// marker is misplaced cannot be classified and is treated like any other
    /// recognised-but-unhandled sequence: logged, no output.
    ///
    /// # Errors
    /// Will return an error if the parser encounters an invalid state
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn ansiparser_inner_csi(
        &mut self,
        b: u8,
        output: &mut Vec<TerminalOutput>,
    ) -> ParserOutcome {
        let push_result = self.push(b);

        // Anything that is not a finished sequence (Continue, Invalid, ...)
        // is reported exactly as `push` reported it.
        let AnsiCsiParserState::Finished(final_byte) = self.state else {
            return push_result;
        };

        let raw = || self.format_raw_csi();

        let Ok(key) = CsiKey::classify(&self.params, &self.intermediates, final_byte) else {
            warn_unhandled(&raw);
            return ParserOutcome::Finished;
        };

        dispatch_csi(key, &self.params, &raw, output)
    }
}

fn is_csi_param(b: u8) -> bool {
    (0x30..=0x3f).contains(&b)
}

fn is_csi_terminator(b: u8) -> bool {
    (0x40..=0x7e).contains(&b)
}

fn is_csi_intermediate(b: u8) -> bool {
    (0x20..=0x2f).contains(&b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::mode::Mode;
    use freminal_common::buffer_states::modes::{
        decckm::Decckm,
        mouse::{MouseEncoding, MouseTrack},
        rl_bracket::RlBracket,
        xtextscrn::XtExtscrn,
    };

    /// Helper: feed a full CSI sequence (everything after ESC[) into the parser
    /// and return the collected `TerminalOutput` vec.
    fn parse_csi_sequence(bytes: &[u8]) -> Vec<TerminalOutput> {
        let mut parser = AnsiCsiParser::new();
        let mut output = Vec::new();
        for &b in bytes {
            parser.ansiparser_inner_csi(b, &mut output);
        }
        output
    }

    /// Extract `Mode` variants from a `Vec<TerminalOutput>`.
    fn extract_modes(outputs: &[TerminalOutput]) -> Vec<&Mode> {
        outputs
            .iter()
            .filter_map(|o| {
                if let TerminalOutput::Mode(m) = o {
                    Some(m)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn test_compound_mode_set_alternate_screen_and_bracketed_paste() {
        // ESC[?1049;2004h — set alternate screen AND bracketed paste
        let output = parse_csi_sequence(b"?1049;2004h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 2, "expected two modes, got {modes:?}");
        assert_eq!(
            *modes[0],
            Mode::XtExtscrn(XtExtscrn::Alternate),
            "first mode should be alternate screen"
        );
        assert_eq!(
            *modes[1],
            Mode::BracketedPaste(RlBracket::Enabled),
            "second mode should be bracketed paste enabled"
        );
    }

    #[test]
    fn test_compound_mode_set_alternate_screen_and_decckm() {
        // ESC[?1049;1h — set alternate screen AND DECCKM application mode
        let output = parse_csi_sequence(b"?1049;1h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 2, "expected two modes, got {modes:?}");
        assert_eq!(*modes[0], Mode::XtExtscrn(XtExtscrn::Alternate));
        assert_eq!(*modes[1], Mode::Decckm(Decckm::Application));
    }

    #[test]
    fn test_compound_mode_set_mouse_x11_and_sgr() {
        // ESC[?1000;1006h — set X11 mouse tracking AND SGR mouse encoding
        let output = parse_csi_sequence(b"?1000;1006h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 2, "expected two modes, got {modes:?}");
        assert_eq!(*modes[0], Mode::MouseMode(MouseTrack::XtMseX11));
        assert_eq!(*modes[1], Mode::MouseEncodingMode(MouseEncoding::Sgr));
    }

    #[test]
    fn test_compound_mode_reset() {
        // ESC[?1049;2004l — reset alternate screen AND bracketed paste
        let output = parse_csi_sequence(b"?1049;2004l");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 2, "expected two modes, got {modes:?}");
        assert_eq!(*modes[0], Mode::XtExtscrn(XtExtscrn::Primary));
        assert_eq!(*modes[1], Mode::BracketedPaste(RlBracket::Disabled));
    }

    #[test]
    fn test_single_param_mode_set_unchanged() {
        // ESC[?1049h — single param, must still work
        let output = parse_csi_sequence(b"?1049h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 1);
        assert_eq!(*modes[0], Mode::XtExtscrn(XtExtscrn::Alternate));
    }

    #[test]
    fn test_single_param_mode_reset_unchanged() {
        // ESC[?2004l — single param reset
        let output = parse_csi_sequence(b"?2004l");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 1);
        assert_eq!(*modes[0], Mode::BracketedPaste(RlBracket::Disabled));
    }

    #[test]
    fn test_non_dec_single_param_unchanged() {
        // ESC[20h — non-DEC single param (LNM)
        let output = parse_csi_sequence(b"20h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 1);
        assert_eq!(
            *modes[0],
            Mode::LineFeedMode(freminal_common::buffer_states::modes::lnm::Lnm::NewLine)
        );
    }

    #[test]
    fn test_three_params_compound() {
        // ESC[?1049;1;2004h — three params: alternate screen + DECCKM + bracketed paste
        let output = parse_csi_sequence(b"?1049;1;2004h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 3, "expected three modes, got {modes:?}");
        assert_eq!(*modes[0], Mode::XtExtscrn(XtExtscrn::Alternate));
        assert_eq!(*modes[1], Mode::Decckm(Decckm::Application));
        assert_eq!(*modes[2], Mode::BracketedPaste(RlBracket::Enabled));
    }

    // ── CSI s routing: SCOSC vs DECSLRM ────────────────────────────────

    #[test]
    fn csi_s_no_params_is_save_cursor() {
        // CSI s with no params → SCOSC (save cursor)
        let out = parse_csi_sequence(b"s");
        assert_eq!(out, vec![TerminalOutput::SaveCursor]);
    }

    #[test]
    fn csi_s_with_params_is_decslrm() {
        // CSI 5;10 s → DECSLRM
        let out = parse_csi_sequence(b"5;10s");
        assert_eq!(
            out,
            vec![TerminalOutput::SetLeftAndRightMargins {
                left_margin: 5,
                right_margin: 10,
            }]
        );
    }

    #[test]
    fn csi_s_with_single_param_is_decslrm() {
        // CSI 3 s → DECSLRM with right=MAX
        let out = parse_csi_sequence(b"3s");
        assert_eq!(
            out,
            vec![TerminalOutput::SetLeftAndRightMargins {
                left_margin: 3,
                right_margin: usize::MAX,
            }]
        );
    }

    #[test]
    fn test_empty_sub_params_skipped() {
        // ESC[?1049;;2004h — empty sub-param between semicolons should be skipped
        let output = parse_csi_sequence(b"?1049;;2004h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 2, "empty sub-params should be skipped");
        assert_eq!(*modes[0], Mode::XtExtscrn(XtExtscrn::Alternate));
        assert_eq!(*modes[1], Mode::BracketedPaste(RlBracket::Enabled));
    }

    // ── push() state-machine edge cases ─────────────────────────────────────

    #[test]
    fn csi_push_to_finished_parser_returns_invalid() {
        // Feed a complete sequence, then push another byte → should return Invalid.
        let mut parser = AnsiCsiParser::new();
        let mut output = Vec::new();
        // Complete with 'H' (CUP)
        parser.ansiparser_inner_csi(b'H', &mut output);
        // Now push again to a finished parser
        let result = parser.push(b'A');
        assert!(matches!(result, ParserOutcome::Invalid(_)));
    }

    #[test]
    fn csi_intermediate_then_param_byte_is_invalid() {
        // Transition: Params → Intermediates (on `!`) → then a param byte (digit `5`)
        // The second byte `5` is a CSI param byte (0x30–0x3f) arriving in Intermediates state
        // → should set state to Invalid.
        let mut parser = AnsiCsiParser::new();
        // Push an intermediate byte to enter Intermediates state
        let r1 = parser.push(b' '); // space (0x20) is a valid CSI intermediate
        assert_eq!(r1, ParserOutcome::Continue);
        assert_eq!(parser.state, AnsiCsiParserState::Intermediates);
        // Now push a param byte (digit) which is invalid in Intermediates state
        let r2 = parser.push(b'5');
        assert!(matches!(r2, ParserOutcome::Invalid(_)));
        assert_eq!(parser.state, AnsiCsiParserState::Invalid);
    }

    #[test]
    fn csi_invalid_state_terminator_sets_invalid_finished() {
        // Enter Invalid state then receive a terminator → InvalidFinished.
        let mut parser = AnsiCsiParser::new();
        // Push a non-param, non-intermediate, non-terminator byte (0x01) → Invalid
        let r1 = parser.push(0x01);
        assert!(matches!(r1, ParserOutcome::Invalid(_)));
        assert_eq!(parser.state, AnsiCsiParserState::Invalid);
        // Push a terminator → InvalidFinished
        let r2 = parser.push(b'H');
        assert!(matches!(r2, ParserOutcome::Invalid(_)));
        assert_eq!(parser.state, AnsiCsiParserState::InvalidFinished);
        // Push yet another byte to the finished-invalid parser → Invalid
        let r3 = parser.push(b'A');
        assert!(matches!(r3, ParserOutcome::Invalid(_)));
    }

    // ── Non-DEC multi-param mode split ──────────────────────────────────────

    #[test]
    fn csi_non_dec_multi_param_mode_set() {
        // ESC[20;4h — non-DEC private, multiple params (splits to two Mode outputs).
        // Each sub-param is dispatched without the `?` prefix.
        let output = parse_csi_sequence(b"20;4h");
        // Both should be Mode outputs (even if NoOp/Unknown)
        assert!(
            output.len() >= 2,
            "expected at least 2 outputs, got {output:?}"
        );
        assert!(output.iter().all(|o| matches!(o, TerminalOutput::Mode(_))));
    }

    // ── CSI > ... q: XTVERSION vs. `>`-prefixed sequences with intermediates ─

    #[test]
    fn xtversion_bare_gt_q_through_parser() {
        let output = parse_csi_sequence(b">q");
        assert_eq!(output, vec![TerminalOutput::RequestDeviceNameAndVersion]);
    }

    #[test]
    fn xtversion_gt0_q_through_parser() {
        let output = parse_csi_sequence(b">0q");
        assert_eq!(output, vec![TerminalOutput::RequestDeviceNameAndVersion]);
    }

    #[test]
    fn gt_prefix_with_intermediate_q_emits_nothing() {
        // `>`-prefixed + intermediate is not XTVERSION and not DECSCUSR; it is
        // recognised (kitty multiple-cursors, Task 103) but unimplemented, so
        // the parser must emit no output at all.
        for seq in [&b">0;4 q"[..], b"> q", b">100 q", b">1;2:3:4 q", b">0;4$q"] {
            let mut parser = AnsiCsiParser::new();
            let mut output = Vec::new();
            let mut last = ParserOutcome::Continue;
            for &b in seq {
                last = parser.ansiparser_inner_csi(b, &mut output);
            }
            assert_eq!(last, ParserOutcome::Finished, "seq {seq:?}");
            assert_eq!(output, [], "seq {seq:?} must emit nothing");
        }
    }

    #[test]
    fn gt_prefix_without_intermediate_non_xtversion_params_is_rejected() {
        // `CSI > 0;4 q` (no space) is not XTVERSION; xtversion rejects it, so
        // no RequestDeviceNameAndVersion is emitted.
        let output = parse_csi_sequence(b">0;4q");
        assert!(
            !output.contains(&TerminalOutput::RequestDeviceNameAndVersion),
            "got {output:?}"
        );
    }

    #[test]
    fn decscusr_with_intermediate_still_dispatches() {
        // Unchanged path: no `>` prefix, `CSI 2 SP q` is DECSCUSR.
        let output = parse_csi_sequence(b"2 q");
        assert_eq!(output.len(), 1);
        assert!(matches!(output[0], TerminalOutput::CursorVisualStyle(_)));
    }

    // ── Unrecognized CSI final byte ──────────────────────────────────────────

    #[test]
    fn csi_unrecognized_final_byte_returns_push_result() {
        // A final byte that is not in the dispatch table (e.g. `w`) → falls through
        // to the `Finished(_esc) => push_result` arm and returns Finished.
        let mut parser = AnsiCsiParser::new();
        let mut output = Vec::new();
        // `w` (0x77) is a valid CSI terminator but not dispatched to any handler
        let result = parser.ansiparser_inner_csi(b'w', &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, []);
    }

    #[test]
    fn format_raw_csi_reconstructs_full_sequence() {
        // The CSI body accumulated by the parser excludes the `ESC [`
        // introducer; format_raw_csi must prepend it so the logged form is the
        // complete sequence a terminal would send.
        let mut parser = AnsiCsiParser::new();
        parser.sequence = b"38;5;9w".to_vec();
        assert_eq!(parser.format_raw_csi(), "\\x1b[38;5;9w");
        // Empty body still shows the introducer.
        let empty = AnsiCsiParser::new();
        assert_eq!(empty.format_raw_csi(), "\\x1b[");
    }

    #[test]
    fn format_raw_csi_preserves_c1_introducer() {
        // A sequence begun by the 8-bit C1 introducer (`0x9B`) must be logged
        // as `\x9b...`, not the 7-bit `\x1b[...` form, to stay lossless.
        let mut parser = AnsiCsiParser::new_c1();
        parser.sequence = b"38;5;9w".to_vec();
        assert_eq!(parser.format_raw_csi(), "\\x9b38;5;9w");
        // Empty body still shows the 8-bit introducer.
        let empty = AnsiCsiParser::new_c1();
        assert_eq!(empty.format_raw_csi(), "\\x9b");
    }

    #[test]
    fn unhandled_final_byte_accumulates_full_body_for_logging() {
        // Feed an unhandled-but-valid CSI (`ESC [ 1 ; 2 W`) one byte at a time
        // and confirm the parser's `sequence` field holds the complete body so
        // the diagnostic log can render it.
        let mut parser = AnsiCsiParser::new();
        let mut output = Vec::new();
        for &b in b"1;2" {
            let _ = parser.ansiparser_inner_csi(b, &mut output);
        }
        let result = parser.ansiparser_inner_csi(b'W', &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, []);
        assert_eq!(parser.sequence, b"1;2W");
        assert_eq!(parser.format_raw_csi(), "\\x1b[1;2W");
    }
}
