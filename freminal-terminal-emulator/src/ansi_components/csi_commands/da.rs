// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::error::ParserFailures;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// Request Device Attributes (DA1 / DA2)
///
/// Supported formats:
/// - ESC [ c          → Primary Device Attributes (DA1)
/// - ESC [ > c        → Secondary Device Attributes (DA2, implicit param 0)
/// - ESC [ > Ps c     → Secondary Device Attributes (DA2, explicit param)
///
/// ## Disambiguation logic
///
/// `>` and `=` are private-marker *parameter* bytes (`0x3C..=0x3F`), never
/// intermediates: they arrive as the first byte of `params`
/// (`params = [b'>', ...]`). The router (`csi_dispatch.rs`) calls this handler
/// with empty `intermediates`.
///
/// A leading `>` sets `is_gt_prefix` before the three sub-cases are
/// evaluated:
/// - **Case 1** — `>` alone, no numeric params → DA2 with `param = 0`.
/// - **Case 2** — `>` followed by a single numeric value → DA2 with that
///   param (only `0` is meaningful; other values are rarely used).
/// - **Case 3** — `>` followed by anything unparsable (e.g. `"1;2"`) →
///   error (malformed).
///
/// A leading `=` is DA3 (`ESC [ = c`).
///
/// Without a `>` or `=` prefix the only valid form is a bare `ESC [ c` or
/// `ESC [ 0 c` (DA1 with `param = 0`).  Any non-zero param or stray
/// intermediates are rejected.
///
/// Note: the XTVERSION query (`ESC [ > q`) uses terminator `q` and is
/// dispatched through `report_xt_version.rs`, not this function.
///
/// # Errors
/// Returns `InvalidParserFailure` if parameters are malformed.
pub fn ansi_parser_inner_csi_finished_da(
    params: &[u8],
    intermediates: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    // DA3: CSI = c — Tertiary Device Attributes
    if params.first() == Some(&b'=') {
        output.push(TerminalOutput::RequestTertiaryDeviceAttributes);
        return ParserOutcome::Finished;
    }

    let is_gt_prefix = params.first() == Some(&b'>');

    if is_gt_prefix {
        // Strip any leading '>' from params for numeric parsing
        let clean_params = if !params.is_empty() && params[0] == b'>' {
            &params[1..]
        } else {
            params
        };

        // case 1: pure '>' only (ESC[>c) → DA2 with implicit param 0
        if clean_params.is_empty() {
            output.push(TerminalOutput::RequestSecondaryDeviceAttributes { param: 0 });
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

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    #[test]
    fn da_primary_empty_params_emits_request_device_attributes() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_da(b"", &[], &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::RequestDeviceAttributes]);
    }

    #[test]
    fn da_primary_zero_param_emits_request_device_attributes() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_da(b"0", &[], &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::RequestDeviceAttributes]);
    }

    #[test]
    fn da_primary_non_zero_param_is_invalid() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_da(b"1", &[], &mut output);
        assert!(matches!(result, ParserOutcome::InvalidParserFailure(_)));
        assert_eq!(output, []);
    }

    #[test]
    fn da_secondary_gt_param_byte_emits_secondary_param0() {
        // `>` as first param byte, nothing else → DA2 with param=0
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_da(b">", &[], &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::RequestSecondaryDeviceAttributes { param: 0 }]
        );
    }

    #[test]
    fn da_secondary_gt0_emits_secondary_param0() {
        // `>0` → DA2 with param=0
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_da(b">0", &[], &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::RequestSecondaryDeviceAttributes { param: 0 }]
        );
    }

    #[test]
    fn da_secondary_multi_params_is_invalid() {
        // `>1;2` → invalid (multiple numeric params after `>`)
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_da(b">1;2", &[], &mut output);
        assert!(matches!(result, ParserOutcome::InvalidParserFailure(_)));
        assert_eq!(output, []);
    }

    #[test]
    fn da_tertiary_eq_param_byte_emits_tertiary() {
        // `=` as first param byte → Tertiary DA
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_da(b"=", &[], &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::RequestTertiaryDeviceAttributes]
        );
    }

    #[test]
    fn da_invalid_intermediates_is_invalid() {
        // some unknown intermediate → last fallthrough
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_da(b"0", b"!", &mut output);
        assert!(matches!(result, ParserOutcome::InvalidParserFailure(_)));
        assert_eq!(output, []);
    }
}
