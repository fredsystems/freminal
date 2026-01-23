// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::mode::{Mode, SetMode};
use freminal_common::buffer_states::terminal_output::TerminalOutput;

use super::csi_commands::{
    cha::ansi_parser_inner_csi_finished_set_cursor_position_g,
    cub::ansi_parser_inner_csi_finished_move_cursor_left,
    cud::ansi_parser_inner_csi_finished_move_down, cuf::ansi_parser_inner_csi_finished_move_right,
    cup::ansi_parser_inner_csi_finished_set_position_h,
    cuu::ansi_parser_inner_csi_finished_move_up,
    dch::ansi_parser_inner_csi_finished_set_position_p,
    decrqm::ansi_parser_inner_csi_finished_decrqm,
    decscusr::ansi_parser_inner_csi_finished_set_position_q,
    decslpp::ansi_parser_inner_csi_finished_set_position_t,
    decstbm::ansi_parser_inner_csi_set_top_and_bottom_margins,
    ech::ansi_parser_inner_csi_finished_set_position_x,
    ed::ansi_parser_inner_csi_finished_set_position_j,
    el::ansi_parser_inner_csi_finished_set_position_k, ict::ansi_parser_inner_csi_finished_ich,
    il::ansi_parser_inner_csi_finished_set_position_l,
    report_xt_version::ansi_parser_inner_csi_finished_report_version_q,
    send_device_attributes::ansi_parser_inner_csi_finished_send_da,
    sgr::ansi_parser_inner_csi_finished_sgr_ansi,
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
            AnsiCsiParserState::Finished(b'H' | b'f') => {
                ansi_parser_inner_csi_finished_set_position_h(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'G') => {
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
            AnsiCsiParserState::Finished(b'P') => {
                ansi_parser_inner_csi_finished_set_position_p(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'X') => {
                ansi_parser_inner_csi_finished_set_position_x(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'm') => {
                ansi_parser_inner_csi_finished_sgr_ansi(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'h') => {
                output.push(TerminalOutput::Mode(Mode::terminal_mode_from_params(
                    &self.params,
                    &SetMode::DecSet,
                )));
                push_result
            }
            AnsiCsiParserState::Finished(b'l') => {
                output.push(TerminalOutput::Mode(Mode::terminal_mode_from_params(
                    &self.params,
                    &SetMode::DecRst,
                )));
                push_result
            }
            AnsiCsiParserState::Finished(b'@') => {
                ansi_parser_inner_csi_finished_ich(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'n') => {
                output.push(TerminalOutput::CursorReport);
                push_result
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
            AnsiCsiParserState::Finished(b'r') => {
                ansi_parser_inner_csi_set_top_and_bottom_margins(&self.params, output)
            }
            AnsiCsiParserState::Finished(b'c') => {
                ansi_parser_inner_csi_finished_send_da(&self.params, &self.intermediates, output)
            }
            AnsiCsiParserState::Finished(b'u') => {
                // https://sw.kovidgoyal.net/kitty/keyboard-protocol/
                output.push(TerminalOutput::Skipped);
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
