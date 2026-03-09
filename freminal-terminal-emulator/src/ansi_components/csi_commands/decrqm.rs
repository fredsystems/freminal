// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::ParserOutcome;
use crate::error::ParserFailures;
use freminal_common::buffer_states::mode::{Mode, SetMode};
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// DEC Private Mode Set
///
/// Supported formats:
/// - Set ESC [ ? Pn h
/// - Reset ESC [ ? Pn l
/// - Query ESC [ ? Pn $ h
/// # Errors
/// Will return an error if the parameter is not a valid number
pub fn ansi_parser_inner_csi_finished_decrqm(
    params: &[u8],
    intermediates: &[u8],
    terminator: u8,
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    // if intermediates contains '$' then we are querying
    if intermediates.contains(&b'$') {
        output.push(TerminalOutput::Mode(Mode::terminal_mode_from_params(
            params,
            &SetMode::DecQuery,
        )));
    } else if terminator == b'h' {
        output.push(TerminalOutput::Mode(Mode::terminal_mode_from_params(
            params,
            &SetMode::DecSet,
        )));
    } else if terminator == b'l' {
        output.push(TerminalOutput::Mode(Mode::terminal_mode_from_params(
            params,
            &SetMode::DecRst,
        )));
    } else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDECRQMCommand(
            params.to_vec(),
        ));
    }

    ParserOutcome::Finished
}
