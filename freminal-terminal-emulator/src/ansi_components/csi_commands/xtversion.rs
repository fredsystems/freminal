// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::ParserOutcome;
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// XTVERSION — Report xterm Version (`CSI > Ps q`)
///
/// Respond with `DCS > | version_string ST` containing the terminal name
/// and version. The leading `>` in params distinguishes this from DECSCUSR.
///
/// The whole parameter string is validated: only exactly `>` or `>0` is
/// XTVERSION. Anything else (e.g. `>0;4`, `>1`, `>00`) is rejected so that
/// `>`-prefixed sequences that merely share the final byte do not trigger a
/// version reply.
pub fn ansi_parser_inner_csi_finished_xtversion(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    if params != b">" && params != b">0" {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledXTVERSIONCommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    }

    output.push(TerminalOutput::RequestDeviceNameAndVersion);

    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    #[test]
    fn xtversion_bare_gt_q_emits_request_device_name_and_version() {
        // The dispatcher passes params without the `q` terminator: just `b">"`.
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_xtversion(b">", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::RequestDeviceNameAndVersion]);
    }

    #[test]
    fn xtversion_gt0_emits_request_device_name_and_version() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_xtversion(b">0", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::RequestDeviceNameAndVersion]);
    }

    #[test]
    fn xtversion_gt1_is_invalid() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_xtversion(b">1", &mut output);
        assert!(matches!(result, ParserOutcome::InvalidParserFailure(_)));
        assert_eq!(output, []);
    }

    #[test]
    fn xtversion_rejects_non_exact_params() {
        for params in [&b">0;4"[..], b">00", b">1;2:3:4", b">100", b">0;", b">;"] {
            let mut output = Vec::new();
            let result = ansi_parser_inner_csi_finished_xtversion(params, &mut output);
            assert!(
                matches!(
                    result,
                    ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledXTVERSIONCommand(
                        _
                    ))
                ),
                "params {params:?} should be rejected"
            );
            assert_eq!(output, []);
        }
    }
}
