// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::ParserOutcome;
use crate::ansi_components::csi_commands::dec_modes::push_split_mode_params;
use crate::error::ParserFailures;
use freminal_common::buffer_states::mode::SetMode;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

/// DECRQM — Request Mode (`CSI Ps $ p` / `CSI ? Ps $ p`)
///
/// Query an ANSI or DEC private mode: respond with a mode status report.
/// DECSET / DECRST (`CSI ? Ps h` / `l`) are routed straight to
/// `push_split_mode_params` by `csi_dispatch.rs` and never reach this handler.
///
/// `_terminator` is the CSI final byte. The router only calls this handler for
/// `p`, so the value is no longer consulted; it is kept so the handler
/// signature is unchanged.
pub fn ansi_parser_inner_csi_finished_decrqm(
    params: &[u8],
    intermediates: &[u8],
    _terminator: u8,
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    // `CSI ... p` is DECRQM only with exactly one `$` intermediate
    // (`CSI ? Ps $ p` / `CSI Ps $ p`). Any other intermediates (`$!`, `$$`,
    // ` $`) make it an unrecognised sequence, which must be ignored: a
    // reply would be injected into the application's input unasked.
    if intermediates != b"$" {
        return ParserOutcome::InvalidParserFailure(ParserFailures::MalformedDECRQMIntermediates(
            intermediates.to_vec(),
        ));
    }
    // A query with no mode number (`CSI $ p`, `CSI ? $ p`) names no
    // mode, so there is nothing to report on.
    let mode_number = params.strip_prefix(b"?").unwrap_or(params);
    if mode_number.is_empty() {
        return ParserOutcome::InvalidParserFailure(ParserFailures::MissingDECRQMMode(
            params.to_vec(),
        ));
    }
    push_split_mode_params(params, SetMode::DecQuery, output);

    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    #[test]
    fn decrqm_dollar_intermediate_emits_dec_query() {
        // `$` intermediate → DecQuery
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_decrqm(b"?1", b"$", b'p', &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        // Should push at least one Mode output
        assert_ne!(output, []);
        assert!(matches!(output[0], TerminalOutput::Mode(_)));
    }

    #[test]
    fn decrqm_extra_intermediate_is_rejected() {
        for intermediates in [&b"$!"[..], b"!$", b"$$", b" $"] {
            let mut output = Vec::new();
            let result =
                ansi_parser_inner_csi_finished_decrqm(b"?1", intermediates, b'p', &mut output);
            assert!(
                matches!(
                    result,
                    ParserOutcome::InvalidParserFailure(
                        ParserFailures::MalformedDECRQMIntermediates(_)
                    )
                ),
                "intermediates {intermediates:?} gave {result:?}"
            );
            assert_eq!(output, [], "intermediates {intermediates:?}");
        }
    }

    #[test]
    fn decrqm_without_dollar_is_rejected() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_decrqm(b"?1", &[], b'p', &mut output);
        assert!(matches!(
            result,
            ParserOutcome::InvalidParserFailure(ParserFailures::MalformedDECRQMIntermediates(_))
        ));
        assert_eq!(output, []);
    }

    #[test]
    fn decrqm_without_mode_number_is_rejected() {
        for params in [&b""[..], b"?"] {
            let mut output = Vec::new();
            let result = ansi_parser_inner_csi_finished_decrqm(params, b"$", b'p', &mut output);
            assert!(
                matches!(
                    result,
                    ParserOutcome::InvalidParserFailure(ParserFailures::MissingDECRQMMode(_))
                ),
                "params {params:?} gave {result:?}"
            );
            assert_eq!(output, [], "params {params:?}");
        }
    }
}
