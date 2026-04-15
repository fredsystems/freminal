// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// XTVERSION — Report xterm Version (`CSI > Ps q`)
///
/// Respond with `DCS > | version_string ST` containing the terminal name
/// and version. The leading `>` in params distinguishes this from DECSCUSR.
pub fn ansi_parser_inner_csi_finished_xtversion(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Ok(param) = parse_param_as::<usize>(&[*params.get(1).unwrap_or(&b'0')]) else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledXTVERSIONCommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    };

    let request = param.unwrap_or(0);

    if request == 0 {
        output.push(TerminalOutput::RequestDeviceNameAndVersion);
    } else {
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledXTVERSIONCommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    }

    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    #[test]
    fn xtversion_bare_gt_q_emits_request_device_name_and_version() {
        // params = b">q": first byte is `>`, second is `q` → param index 1 is b'q'
        // parse_param_as on b'q' fails → but wait, the function reads params[1].
        // When called via the CSI dispatcher, params = b">" (the `q` is the terminator).
        // Simulate the params slice the dispatcher passes: just `b">"`.
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_xtversion(b">", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::RequestDeviceNameAndVersion]);
    }

    #[test]
    fn xtversion_gt0_emits_request_device_name_and_version() {
        // params = b">0": second byte is b'0' → parse_param_as(b"0") = Ok(Some(0)) → request
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_xtversion(b">0", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::RequestDeviceNameAndVersion]);
    }

    #[test]
    fn xtversion_gt1_is_invalid() {
        // params = b">1": second byte is b'1' → parse_param_as(b"1") = Ok(Some(1)) → invalid
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_xtversion(b">1", &mut output);
        assert!(matches!(result, ParserOutcome::InvalidParserFailure(_)));
        assert!(output.is_empty());
    }
}
