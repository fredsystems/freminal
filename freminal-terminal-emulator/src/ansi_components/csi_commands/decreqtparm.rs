// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::ParserOutcome;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// DECREQTPARM — Request Terminal Parameters (`CSI Ps x`)
///
/// Only plain `CSI Ps x` is valid (`Ps` = 0 or 1). A `>` prefix is rejected
/// (that would be a malformed DA2/xtversion sequence, not DECREQTPARM), as are
/// extra non-empty `;`-separated parameters and any `Ps` other than 0 or 1.
pub fn ansi_parser_inner_csi_finished_decreqtparm(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    if params.first().copied() == Some(b'>') {
        output.push(TerminalOutput::Invalid);
        return ParserOutcome::Finished;
    }
    // Parse first `;`-separated parameter only.
    // DECREQTPARM accepts Ps=0 (default) or Ps=1; reject anything else.
    let mut params = params.split(|&b| b == b';');
    let first_param = params.next().unwrap_or_default();
    let has_extra_params = params.any(|param| !param.is_empty());

    if has_extra_params {
        output.push(TerminalOutput::Invalid);
        return ParserOutcome::Finished;
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
    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi_components::csi::AnsiCsiParser;

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
    fn decreqtparm_with_gt_prefix_is_unrouted() {
        // ESC[>0x has no route (`>` prefix, no intermediate, final `x`), so the
        // router recognises it as unhandled and emits nothing.
        let output = parse_csi_sequence(b">0x");
        assert_eq!(output, []);
    }

    #[test]
    fn decreqtparm_extra_params_is_invalid() {
        // ESC[1;2x → has_extra_params=true → Invalid
        let output = parse_csi_sequence(b"1;2x");
        assert_eq!(output, vec![TerminalOutput::Invalid]);
    }
}
