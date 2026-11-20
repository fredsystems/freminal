// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::ansi::ParserOutcome;
use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_common::buffer_states::xtsmgraphics::{
    GraphicsAttributeAction, GraphicsAttributeItem,
};

/// Parse a run of ASCII digits into a `u32`. Empty input, a non-digit, or an
/// overflow is `None`.
fn parse_digits(field: &[u8]) -> Option<u32> {
    if field.is_empty() || !field.iter().all(u8::is_ascii_digit) {
        return None;
    }
    field.iter().try_fold(0u32, |acc, d| {
        acc.checked_mul(10)?.checked_add(u32::from(*d - b'0'))
    })
}

/// XTSMGRAPHICS — Set or Request Graphics Attribute (`CSI ? Pi ; Pa ; Pv S`)
///
/// Follows xterm (`charproc.c`, `CASE_GRAPHICS_ATTRIBUTES`): exactly three
/// `;`-separated parameters are required, otherwise the sequence is malformed
/// and gets no reply. `Pi` and `Pa` must be decimal numbers (there is nothing
/// meaningful to echo back otherwise). `Pv` is ignored, as in xterm except for
/// a set, which Freminal answers with failure regardless of the value.
///
/// The parser only classifies the request. The reply is built by the handler,
/// which knows the cell pixel size and grid size.
pub fn ansi_parser_inner_csi_finished_xtsmgraphics(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    let Some(body) = params.strip_prefix(b"?") else {
        output.push(TerminalOutput::Invalid);
        return ParserOutcome::Finished;
    };

    let mut fields = body.split(|&b| b == b';');
    let (Some(pi), Some(pa), Some(_pv), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        output.push(TerminalOutput::Invalid);
        return ParserOutcome::Finished;
    };

    let (Some(pi), Some(pa)) = (parse_digits(pi), parse_digits(pa)) else {
        output.push(TerminalOutput::Invalid);
        return ParserOutcome::Finished;
    };

    output.push(TerminalOutput::GraphicsAttributeRequest {
        item: GraphicsAttributeItem::from(pi),
        action: GraphicsAttributeAction::from(pa),
    });
    ParserOutcome::Finished
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(params: &[u8]) -> Vec<TerminalOutput> {
        let mut output = Vec::new();
        let outcome = ansi_parser_inner_csi_finished_xtsmgraphics(params, &mut output);
        assert_eq!(outcome, ParserOutcome::Finished);
        output
    }

    #[test]
    fn well_formed_request_is_classified() {
        assert_eq!(
            run(b"?1;1;0"),
            [TerminalOutput::GraphicsAttributeRequest {
                item: GraphicsAttributeItem::ColorRegisters,
                action: GraphicsAttributeAction::Read,
            }]
        );
        assert_eq!(
            run(b"?2;4;0"),
            [TerminalOutput::GraphicsAttributeRequest {
                item: GraphicsAttributeItem::SixelGeometry,
                action: GraphicsAttributeAction::ReadMaximum,
            }]
        );
    }

    #[test]
    fn unknown_item_and_action_are_carried_through() {
        assert_eq!(
            run(b"?9;7;0"),
            [TerminalOutput::GraphicsAttributeRequest {
                item: GraphicsAttributeItem::Unknown(9),
                action: GraphicsAttributeAction::Unknown(7),
            }]
        );
    }

    #[test]
    fn pv_is_ignored_even_when_empty_or_odd() {
        for params in [&b"?1;1;"[..], b"?1;1;999999999999", b"?1;1;x"] {
            assert_eq!(
                run(params),
                [TerminalOutput::GraphicsAttributeRequest {
                    item: GraphicsAttributeItem::ColorRegisters,
                    action: GraphicsAttributeAction::Read,
                }],
                "params {params:?}"
            );
        }
    }

    #[test]
    fn wrong_parameter_count_is_invalid() {
        for params in [&b"?"[..], b"?1", b"?1;1", b"?1;1;0;0", b"?;;;"] {
            assert_eq!(run(params), [TerminalOutput::Invalid], "params {params:?}");
        }
    }

    #[test]
    fn missing_or_non_numeric_pi_pa_is_invalid() {
        for params in [&b"?;1;0"[..], b"?1;;0", b"?a;1;0", b"?1;b;0", b"?1:2;1;0"] {
            assert_eq!(run(params), [TerminalOutput::Invalid], "params {params:?}");
        }
    }

    #[test]
    fn overflowing_pi_is_invalid() {
        assert_eq!(run(b"?99999999999;1;0"), [TerminalOutput::Invalid]);
    }

    #[test]
    fn missing_private_marker_is_invalid() {
        assert_eq!(run(b"1;1;0"), [TerminalOutput::Invalid]);
    }
}
