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
/// The router calls this handler only for a key whose single intermediate is
/// `$` and whose final byte is `p`; any other intermediates (`$!`, `$$`, ` $`)
/// are unrouted and never arrive here, so the handler takes none.
pub fn ansi_parser_inner_csi_finished_decrqm(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
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
        let result = ansi_parser_inner_csi_finished_decrqm(b"?1", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        // Should push at least one Mode output
        assert_ne!(output, []);
        assert!(matches!(output[0], TerminalOutput::Mode(_)));
    }

    #[test]
    fn decrqm_without_mode_number_is_rejected() {
        for params in [&b""[..], b"?"] {
            let mut output = Vec::new();
            let result = ansi_parser_inner_csi_finished_decrqm(params, &mut output);
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
