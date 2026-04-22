// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::mode::{Mode, SetMode};
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Split CSI mode parameters on `;` and emit one `TerminalOutput::Mode` per sub-parameter.
///
/// When the parameter string starts with `?` (DEC private indicator), the `?` prefix is
/// re-applied to each sub-parameter so that `terminal_mode_from_params` matches correctly.
///
/// For example, `?1049;2004` is split into `?1049` and `?2004`, each producing its own
/// `TerminalOutput::Mode`.
pub(crate) fn push_split_mode_params(
    params: &[u8],
    mode: SetMode,
    output: &mut Vec<TerminalOutput>,
) {
    let (is_dec_private, param_body) = if params.first() == Some(&b'?') {
        (true, &params[1..])
    } else {
        (false, params)
    };

    // Fast path: no semicolons means a single parameter — avoid allocation.
    if !param_body.contains(&b';') {
        output.push(TerminalOutput::Mode(Mode::terminal_mode_from_params(
            params, mode,
        )));
        return;
    }

    for sub_param in param_body.split(|&b| b == b';') {
        if sub_param.is_empty() {
            continue;
        }
        if is_dec_private {
            let mut prefixed = Vec::with_capacity(1 + sub_param.len());
            prefixed.push(b'?');
            prefixed.extend_from_slice(sub_param);
            output.push(TerminalOutput::Mode(Mode::terminal_mode_from_params(
                &prefixed, mode,
            )));
        } else {
            output.push(TerminalOutput::Mode(Mode::terminal_mode_from_params(
                sub_param, mode,
            )));
        }
    }
}

use super::csi_commands::{
    cbt::ansi_parser_inner_csi_finished_cbt, cha::ansi_parser_inner_csi_finished_cha,
    cht::ansi_parser_inner_csi_finished_cht, cnl::ansi_parser_inner_csi_finished_cnl,
    cpl::ansi_parser_inner_csi_finished_cpl, cub::ansi_parser_inner_csi_finished_cub,
    cud::ansi_parser_inner_csi_finished_cud, cuf::ansi_parser_inner_csi_finished_cuf,
    cup::ansi_parser_inner_csi_finished_cup, cuu::ansi_parser_inner_csi_finished_cuu,
    da::ansi_parser_inner_csi_finished_da, dch::ansi_parser_inner_csi_finished_dch,
    decrqm::ansi_parser_inner_csi_finished_decrqm,
    decscusr::ansi_parser_inner_csi_finished_decscusr,
    decslpp::ansi_parser_inner_csi_finished_decslpp,
    decslrm::ansi_parser_inner_csi_finished_decslrm,
    decstbm::ansi_parser_inner_csi_finished_decstbm, dl::ansi_parser_inner_csi_finished_dl,
    dsr::ansi_parser_inner_csi_finished_dsr, ech::ansi_parser_inner_csi_finished_ech,
    ed::ansi_parser_inner_csi_finished_ed, el::ansi_parser_inner_csi_finished_el,
    ich::ansi_parser_inner_csi_finished_ich, il::ansi_parser_inner_csi_finished_il,
    rep::ansi_parser_inner_csi_finished_rep, scorc::ansi_parser_inner_csi_finished_scorc,
    sd::ansi_parser_inner_csi_finished_sd, sgr::ansi_parser_inner_csi_finished_sgr,
    su::ansi_parser_inner_csi_finished_su, tbc::ansi_parser_inner_csi_finished_tbc,
    vpa::ansi_parser_inner_csi_finished_vpa, xtversion::ansi_parser_inner_csi_finished_xtversion,
};
use crate::ansi_components::tracer::SequenceTracer;
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
#[derive(Eq, PartialEq, Debug, Default)]
pub struct AnsiCsiParser {
    pub(crate) state: AnsiCsiParserState,
    pub params: Vec<u8>,
    pub intermediates: Vec<u8>,
    pub sequence: Vec<u8>,
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
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: AnsiCsiParserState::Params,
            params: Vec::with_capacity(8),
            intermediates: Vec::with_capacity(4),
            sequence: Vec::with_capacity(16),
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
    /// # Errors
    /// Will return an error if the parser encounters an invalid state
    // Inherently large: CSI final-byte dispatch table (ECMA-48 §8.3). Each arm handles a
    // distinct CSI sequence. Splitting would scatter a single coherent dispatch table.
    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn ansiparser_inner_csi(
        &mut self,
        b: u8,
        output: &mut Vec<TerminalOutput>,
    ) -> ParserOutcome {
        let push_result = self.push(b);

        match self.state {
            AnsiCsiParserState::Finished(b'A') => {
                ansi_parser_inner_csi_finished_cuu(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'B') => {
                ansi_parser_inner_csi_finished_cud(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'C') => {
                ansi_parser_inner_csi_finished_cuf(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'D') => {
                ansi_parser_inner_csi_finished_cub(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'E') => {
                ansi_parser_inner_csi_finished_cnl(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'F') => {
                ansi_parser_inner_csi_finished_cpl(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'H' | b'f') => {
                ansi_parser_inner_csi_finished_cup(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'I') => {
                // CHT — Cursor Forward Tabulation
                ansi_parser_inner_csi_finished_cht(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'G' | b'`') => {
                // CHA (CSI G) and HPA (CSI `) — cursor horizontal absolute
                ansi_parser_inner_csi_finished_cha(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'J') => {
                ansi_parser_inner_csi_finished_ed(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'K') => {
                ansi_parser_inner_csi_finished_el(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'L') => {
                ansi_parser_inner_csi_finished_il(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'M') => {
                ansi_parser_inner_csi_finished_dl(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'P') => {
                ansi_parser_inner_csi_finished_dch(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'S') => {
                ansi_parser_inner_csi_finished_su(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'T') => {
                ansi_parser_inner_csi_finished_sd(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'X') => {
                ansi_parser_inner_csi_finished_ech(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'Z') => {
                // CBT — Cursor Backward Tabulation
                ansi_parser_inner_csi_finished_cbt(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'b') => {
                // REP — Repeat preceding graphic character
                ansi_parser_inner_csi_finished_rep(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'g') => {
                // TBC — Tab Clear
                ansi_parser_inner_csi_finished_tbc(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'm') => {
                ansi_parser_inner_csi_finished_sgr(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'h') => {
                push_split_mode_params(&self.params, SetMode::DecSet, output);
                push_result
            }
            AnsiCsiParserState::Finished(b'l') => {
                push_split_mode_params(&self.params, SetMode::DecRst, output);
                push_result
            }
            AnsiCsiParserState::Finished(b'@') => {
                ansi_parser_inner_csi_finished_ich(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'n') => {
                ansi_parser_inner_csi_finished_dsr(&self.params, output)
            }
            AnsiCsiParserState::Finished(b't') => {
                ansi_parser_inner_csi_finished_decslpp(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'p') => {
                ansi_parser_inner_csi_finished_decrqm(&self.params, &self.intermediates, b, output)
            }
            AnsiCsiParserState::Finished(b'q') => {
                if self.params.is_empty() || self.params.first().unwrap_or(&b'0') != &b'>' {
                    return ansi_parser_inner_csi_finished_decscusr(&self.params, output);
                }
                ansi_parser_inner_csi_finished_xtversion(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'd') => {
                ansi_parser_inner_csi_finished_vpa(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'r') => {
                ansi_parser_inner_csi_finished_decstbm(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'c') => {
                ansi_parser_inner_csi_finished_da(&self.params, &self.intermediates, output)
            }
            AnsiCsiParserState::Finished(b's') => {
                // When params are present this is DECSLRM (set left/right margins);
                // when empty it is SCOSC (save cursor).  The handler
                // (`process_outputs`) ignores SetLeftAndRightMargins when
                // DECLRMM is not active, so the parse is always safe.
                if self.params.is_empty() {
                    output.push(TerminalOutput::SaveCursor);
                    push_result
                } else {
                    ansi_parser_inner_csi_finished_decslrm(&self.params, output)
                }
            }
            AnsiCsiParserState::Finished(b'u') => {
                ansi_parser_inner_csi_finished_scorc(&self.params, output);
                push_result
            }
            AnsiCsiParserState::Finished(b'x') => {
                // DECREQTPARM — Request Terminal Parameters.
                // Only plain CSI Ps x is valid (Ps=0 or Ps=1).
                // Reject if '>' intermediate is present (that would be a
                // malformed DA2/xtversion sequence, not DECREQTPARM).
                let has_gt = self.intermediates.contains(&b'>')
                    || self.params.first().copied() == Some(b'>');
                if has_gt {
                    output.push(TerminalOutput::Invalid);
                    return push_result;
                }
                // Parse first `;`-separated parameter only.
                // DECREQTPARM accepts Ps=0 (default) or Ps=1; reject anything else.
                let mut params = self.params.split(|&b| b == b';');
                let first_param = params.next().unwrap_or_default();
                let has_extra_params = params.any(|param| !param.is_empty());

                if has_extra_params {
                    output.push(TerminalOutput::Invalid);
                    return push_result;
                }

                let parsed_ps = if first_param.is_empty() {
                    Some(0u8)
                } else if first_param.iter().all(u8::is_ascii_digit) {
                    first_param
                        .iter()
                        .try_fold(0u8, |acc, d| acc.checked_mul(10)?.checked_add(*d - b'0'))
                } else {
                    None
                };

                match parsed_ps {
                    Some(ps @ 0..=1) => {
                        output.push(TerminalOutput::RequestTerminalParameters(ps));
                    }
                    _ => output.push(TerminalOutput::Invalid),
                }
                push_result
            }
            AnsiCsiParserState::Finished(_esc) => push_result,

            // Below should cover the invalid state(AnsiCsiParserState::Invalid) as well as any other finished states
            _ => push_result,
        }
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

    // ── DECREQTPARM CSI x ───────────────────────────────────────────────────

    #[test]
    fn decreqtparm_ps0_emits_request_terminal_parameters() {
        // ESC[0x → RequestTerminalParameters(0)
        let output = parse_csi_sequence(b"0x");
        assert_eq!(output, vec![TerminalOutput::RequestTerminalParameters(0)]);
    }

    #[test]
    fn decreqtparm_ps1_emits_request_terminal_parameters_1() {
        // ESC[1x → RequestTerminalParameters(1)
        let output = parse_csi_sequence(b"1x");
        assert_eq!(output, vec![TerminalOutput::RequestTerminalParameters(1)]);
    }

    #[test]
    fn decreqtparm_ps2_is_invalid() {
        // ESC[2x → ps=2 is out of range → Invalid
        let output = parse_csi_sequence(b"2x");
        assert_eq!(output, vec![TerminalOutput::Invalid]);
    }

    #[test]
    fn decreqtparm_with_gt_prefix_is_invalid() {
        // ESC[>0x → has_gt=true → Invalid
        let output = parse_csi_sequence(b">0x");
        assert_eq!(output, vec![TerminalOutput::Invalid]);
    }

    #[test]
    fn decreqtparm_extra_params_is_invalid() {
        // ESC[1;2x → has_extra_params=true → Invalid
        let output = parse_csi_sequence(b"1;2x");
        assert_eq!(output, vec![TerminalOutput::Invalid]);
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
        assert!(output.is_empty());
    }
}
