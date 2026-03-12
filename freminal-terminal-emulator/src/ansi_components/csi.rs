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
    cbt::ansi_parser_inner_csi_finished_cbt,
    cha::ansi_parser_inner_csi_finished_set_cursor_position_g,
    cht::ansi_parser_inner_csi_finished_cht, cnl::ansi_parser_inner_csi_finished_cnl,
    cpl::ansi_parser_inner_csi_finished_cpl, cub::ansi_parser_inner_csi_finished_move_cursor_left,
    cud::ansi_parser_inner_csi_finished_move_down, cuf::ansi_parser_inner_csi_finished_move_right,
    cup::ansi_parser_inner_csi_finished_set_position_h,
    cuu::ansi_parser_inner_csi_finished_move_up,
    dch::ansi_parser_inner_csi_finished_set_position_p,
    decrqm::ansi_parser_inner_csi_finished_decrqm,
    decscusr::ansi_parser_inner_csi_finished_set_position_q,
    decslpp::ansi_parser_inner_csi_finished_set_position_t,
    decstbm::ansi_parser_inner_csi_set_top_and_bottom_margins,
    dl::ansi_parser_inner_csi_finished_dl, dsr::ansi_parser_inner_csi_finished_dsr,
    ech::ansi_parser_inner_csi_finished_set_position_x,
    ed::ansi_parser_inner_csi_finished_set_position_j,
    el::ansi_parser_inner_csi_finished_set_position_k, ict::ansi_parser_inner_csi_finished_ich,
    il::ansi_parser_inner_csi_finished_set_position_l, rep::ansi_parser_inner_csi_finished_rep,
    report_xt_version::ansi_parser_inner_csi_finished_report_version_q,
    sd::ansi_parser_inner_csi_finished_sd,
    send_device_attributes::ansi_parser_inner_csi_finished_send_da,
    sgr::ansi_parser_inner_csi_finished_sgr_ansi, su::ansi_parser_inner_csi_finished_su,
    tbc::ansi_parser_inner_csi_finished_tbc, vpa::ansi_parser_inner_csi_finished_vpa,
};
use crate::ansi_components::tracer::SequenceTracer;
use crate::{ansi::ParserOutcome, ansi_components::tracer::SequenceTraceable};

#[derive(Eq, PartialEq, Debug, Default)]
pub enum AnsiCsiParserState {
    #[default]
    Params,
    Intermediates,
    Finished(u8),
    Invalid,
    InvalidFinished,
}
#[derive(Eq, PartialEq, Debug, Default)]
pub struct AnsiCsiParser {
    pub state: AnsiCsiParserState,
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
                unreachable!();
            }
        }
    }

    /// Push a byte into the parser and return the next state
    ///
    /// # Errors
    /// Will return an error if the parser encounters an invalid state
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
                ansi_parser_inner_csi_finished_move_up(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'B') => {
                ansi_parser_inner_csi_finished_move_down(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'C') => {
                ansi_parser_inner_csi_finished_move_right(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'D') => {
                ansi_parser_inner_csi_finished_move_cursor_left(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'E') => {
                ansi_parser_inner_csi_finished_cnl(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'F') => {
                ansi_parser_inner_csi_finished_cpl(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'H' | b'f') => {
                ansi_parser_inner_csi_finished_set_position_h(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'I') => {
                // CHT — Cursor Forward Tabulation
                ansi_parser_inner_csi_finished_cht(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'G' | b'`') => {
                // CHA (CSI G) and HPA (CSI `) — cursor horizontal absolute
                ansi_parser_inner_csi_finished_set_cursor_position_g(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'J') => {
                ansi_parser_inner_csi_finished_set_position_j(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'K') => {
                ansi_parser_inner_csi_finished_set_position_k(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'L') => {
                ansi_parser_inner_csi_finished_set_position_l(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'M') => {
                ansi_parser_inner_csi_finished_dl(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'P') => {
                ansi_parser_inner_csi_finished_set_position_p(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'S') => {
                ansi_parser_inner_csi_finished_su(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'T') => {
                ansi_parser_inner_csi_finished_sd(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'X') => {
                ansi_parser_inner_csi_finished_set_position_x(&self.params, output)
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
                ansi_parser_inner_csi_finished_sgr_ansi(&self.params, output)
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
                ansi_parser_inner_csi_finished_set_position_t(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'p') => {
                ansi_parser_inner_csi_finished_decrqm(&self.params, &self.intermediates, b, output)
            }
            AnsiCsiParserState::Finished(b'q') => {
                if self.params.is_empty() || self.params.first().unwrap_or(&b'0') != &b'>' {
                    return ansi_parser_inner_csi_finished_set_position_q(&self.params, output);
                }
                ansi_parser_inner_csi_finished_report_version_q(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'd') => {
                ansi_parser_inner_csi_finished_vpa(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'r') => {
                ansi_parser_inner_csi_set_top_and_bottom_margins(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'c') => {
                ansi_parser_inner_csi_finished_send_da(&self.params, &self.intermediates, output)
            }
            AnsiCsiParserState::Finished(b's') => {
                // SCOSC — Save Cursor Position
                output.push(TerminalOutput::SaveCursor);
                push_result
            }
            AnsiCsiParserState::Finished(b'u') => {
                // SCORC — Restore Cursor Position
                // Note: Kitty keyboard protocol also uses CSI u, but with
                // different parameter patterns (CSI > flags u). Plain CSI u
                // (no intermediates/params or numeric params) is SCORC.
                output.push(TerminalOutput::RestoreCursor);
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
        decckm::Decckm, mouse::MouseTrack, rl_bracket::RlBracket, xtextscrn::XtExtscrn,
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
        assert_eq!(*modes[1], Mode::MouseMode(MouseTrack::XtMseSgr));
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

    #[test]
    fn test_empty_sub_params_skipped() {
        // ESC[?1049;;2004h — empty sub-param between semicolons should be skipped
        let output = parse_csi_sequence(b"?1049;;2004h");
        let modes = extract_modes(&output);
        assert_eq!(modes.len(), 2, "empty sub-params should be skipped");
        assert_eq!(*modes[0], Mode::XtExtscrn(XtExtscrn::Alternate));
        assert_eq!(*modes[1], Mode::BracketedPaste(RlBracket::Enabled));
    }
}
