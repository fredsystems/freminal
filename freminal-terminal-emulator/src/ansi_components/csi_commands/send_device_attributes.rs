// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Request Device Attributes (DA1 / DA2 / `XTVersion`)
///
/// Supported formats:
/// - ESC [ c          → Primary Device Attributes (DA1)
/// - ESC [ > c        → `XTVersion` query (xterm)
/// - ESC [ > Ps c     → Secondary Device Attributes (DA2)
///
/// # Errors
/// Returns `InvalidParserFailure` if parameters are malformed.
pub fn ansi_parser_inner_csi_finished_send_da(
    params: &[u8],
    intermediates: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let is_gt_prefix = intermediates.contains(&b'>') || (!params.is_empty() && params[0] == b'>');

    if is_gt_prefix {
        // Strip any leading '>' from params for numeric parsing
        let clean_params = if !params.is_empty() && params[0] == b'>' {
            &params[1..]
        } else {
            params
        };

        // case 1: pure '>' only → XTVersion
        if clean_params.is_empty() {
            output.push(TerminalOutput::RequestXtVersion);
            return ParserOutcome::Finished;
        }

        // case 2: single numeric param → Secondary DA
        if let Ok(Some(v)) = parse_param_as::<usize>(clean_params) {
            output.push(TerminalOutput::RequestSecondaryDeviceAttributes { param: v });
            return ParserOutcome::Finished;
        }

        // case 3: anything else (multiple params like "1;2") → invalid
        return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDACommand(
            String::from_utf8_lossy(params).to_string(),
        ));
    }

    // Primary DA (ESC[c)
    if intermediates.is_empty() {
        let Ok(param) = parse_param_as::<usize>(params) else {
            return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDACommand(
                String::from_utf8_lossy(params).to_string(),
            ));
        };
        let param = param.unwrap_or(0);
        if param != 0 {
            return ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDACommand(
                format!("Invalid parameters for Send DA: {params:?}"),
            ));
        }
        output.push(TerminalOutput::RequestDeviceAttributes);
        return ParserOutcome::Finished;
    }

    ParserOutcome::InvalidParserFailure(ParserFailures::UnhandledDACommand(format!(
        "Invalid intermediates for Send DA: {params:?}, intermediates={intermediates:?}",
    )))
}
