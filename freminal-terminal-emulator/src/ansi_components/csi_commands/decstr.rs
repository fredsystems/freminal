// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::ParserOutcome;
use crate::ansi_components::tracer::escape_sequence_for_log;
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// DECSTR — Soft Terminal Reset (`CSI ! p`)
///
/// Resets the subset of terminal modes, margins, and attributes listed in
/// Table 5-9 of the VT510 Programmer Reference
/// (<https://vt100.net/docs/vt510-rm/DECSTR.html>) to their power-on
/// defaults. Unlike RIS (`ESC c`), DECSTR does not clear screen content,
/// scrollback, images, the palette, the window title, tab stops, or the
/// alternate-screen flag, and it does not move the live cursor.
///
/// `CSI ! p` takes no parameters; a parameter string present alongside the
/// `!` intermediate is rejected as invalid, mirroring how sibling CSI
/// handlers reject unexpected parameters.
pub fn ansi_parser_inner_csi_finished_decstr(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    if !params.is_empty() {
        warn!(
            "Invalid DECSTR command (CSI ! p); raw params: \"{}\"",
            escape_sequence_for_log(params)
        );
        output.push(TerminalOutput::Invalid);
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDECSTRCommand(
            format!("{params:?}"),
        ));
    }

    output.push(TerminalOutput::SoftReset);
    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi_components::csi::AnsiCsiParser;

    /// Helper: feed a full CSI sequence (everything after `ESC [`) into the
    /// parser and return the collected `TerminalOutput` vec.
    fn parse_csi_sequence(bytes: &[u8]) -> Vec<TerminalOutput> {
        let mut parser = AnsiCsiParser::new();
        let mut output = Vec::new();
        for &b in bytes {
            parser.ansiparser_inner_csi(b, &mut output);
        }
        output
    }

    #[test]
    fn decstr_bare_produces_soft_reset() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_decstr(b"", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::SoftReset]);
    }

    #[test]
    fn decstr_with_unexpected_params_is_rejected() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_decstr(b"1", &mut output);
        assert!(matches!(result, ParserOutcome::InvalidParserFailure(_)));
        assert_eq!(output, vec![TerminalOutput::Invalid]);
    }

    #[test]
    fn csi_bang_p_reaches_decstr_via_full_parser() {
        // `CSI ! p` — full round trip through the CSI parser's dispatch table.
        let output = parse_csi_sequence(b"!p");
        assert_eq!(output, vec![TerminalOutput::SoftReset]);
    }

    #[test]
    fn csi_dollar_p_still_reaches_decrqm_unchanged() {
        // Regression guard: `CSI $ p` (DECSLPP-adjacent DECRQM query without a
        // DEC private prefix) must still route to DECRQM, not DECSTR, since
        // the two share the final byte `p`.
        let output = parse_csi_sequence(b"$p");
        assert_ne!(output.as_slice(), []);
        assert!(matches!(output[0], TerminalOutput::Mode(_)));
    }

    #[test]
    fn csi_question_1_dollar_p_still_reaches_decrqm_unchanged() {
        // Regression guard: `CSI ? 1 $ p` (DECRQM query for DECCKM) must
        // still route to DECRQM, not DECSTR.
        let output = parse_csi_sequence(b"?1$p");
        assert_ne!(output.as_slice(), []);
        assert!(matches!(output[0], TerminalOutput::Mode(_)));
    }
}
