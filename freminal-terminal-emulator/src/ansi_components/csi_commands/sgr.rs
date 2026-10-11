// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::slice::Iter;

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::ansi_components::csi_commands::sgr_color::{
    SgrColorError, SgrColorForm, parse_sgr_color,
};
use crate::ansi_components::tracer::escape_sequence_for_log;
use freminal_common::buffer_states::fonts::UnderlineStyle;
use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_common::colors::TerminalColor;
use freminal_common::sgr::SelectGraphicRendition;

/// SGR — Select Graphic Rendition (`CSI Ps m`)
///
/// Set text attributes for subsequent characters. Multiple attributes can be
/// combined with semicolons (e.g., `CSI 1;31 m` for bold red).
///
/// Supports colon-delimited subparameters within a semicolon-separated segment
/// (e.g., `4:3` for curly underline, `38:2::255:0:0` for truecolor).
///
/// Only plain `CSI Ps m` reaches this handler. `CSI > Ps m` (XTMODKEYS) is
/// routed to the `modify_other_keys` handler by `csi_dispatch.rs`.
pub fn ansi_parser_inner_csi_finished_sgr(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    // Split by semicolons first — each segment is a top-level SGR parameter
    // that may itself contain colon-delimited subparameters.
    let segments: Vec<&[u8]> = params.split(|b| *b == b';').collect();

    // Empty SGR params means reset (0)
    if segments.is_empty() || (segments.len() == 1 && segments[0].is_empty()) {
        output.push(TerminalOutput::Sgr(SelectGraphicRendition::Reset));
        return ParserOutcome::Finished;
    }

    // Some SGR codes (38, 48, 58) consume multiple subsequent
    // semicolon-separated segments as their parameters.
    let parsed = parse_all_segments(&segments);
    let mut iter = parsed.iter();

    while let Some(val) = iter.next() {
        if dispatch_sgr_value(val, &mut iter, output) == SgrFlow::Stop {
            break;
        }
    }

    ParserOutcome::Finished
}

/// Parse all semicolon-separated segments into a flat stream of
/// `SegmentValue` tokens.  Colon-containing segments produce `Colon`
/// tokens (self-contained); plain segments produce `Simple` tokens.
fn parse_all_segments(segments: &[&[u8]]) -> Vec<SegmentValue> {
    let mut result = Vec::with_capacity(segments.len());
    for &seg in segments {
        if seg.contains(&b':') {
            result.push(SegmentValue::Colon(seg.to_vec()));
        } else {
            let val = match parse_param_as::<usize>(seg) {
                Ok(Some(v)) => Some(v),
                Ok(None) => Some(0), // empty/omitted → reset
                Err(_) => None,      // parse error
            };
            result.push(SegmentValue::Simple(val));
        }
    }
    result
}

/// A parsed semicolon segment: either a plain numeric value or a
/// colon-delimited subparameter group.
enum SegmentValue {
    /// A simple numeric SGR parameter (or `None` for parse errors).
    Simple(Option<usize>),
    /// A colon-delimited segment (e.g., `4:3` or `38:2::255:0:0`).
    Colon(Vec<u8>),
}

/// Whether the remaining parameters of an SGR sequence should still be
/// processed after handling one parameter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SgrFlow {
    /// Carry on with the next parameter.
    Continue,
    /// Drop every remaining parameter of this sequence. Parameters before the
    /// current one have already been emitted and still apply.
    Stop,
}

/// Dispatch a single SGR value, consuming additional iterator items for
/// multi-param codes (38, 48, 58 in semicolon form).
fn dispatch_sgr_value(
    value: &SegmentValue,
    iter: &mut Iter<'_, SegmentValue>,
    output: &mut Vec<TerminalOutput>,
) -> SgrFlow {
    match value {
        SegmentValue::Colon(raw) => process_colon_segment(raw, output),
        SegmentValue::Simple(None) => {
            output.push(TerminalOutput::Invalid);
            SgrFlow::Continue
        }
        SegmentValue::Simple(Some(param)) => dispatch_simple_param(*param, iter, output),
    }
}

/// Handle a plain numeric SGR parameter.  For 38/48/58, extract the colour
/// from the following semicolon-form segments.
fn dispatch_simple_param(
    param: usize,
    iter: &mut Iter<'_, SegmentValue>,
    output: &mut Vec<TerminalOutput>,
) -> SgrFlow {
    match param {
        38 | 48 | 58 => handle_semicolon_color(param, iter, output),
        _ => {
            output.push(TerminalOutput::Sgr(SelectGraphicRendition::from_usize(
                param,
            )));
            SgrFlow::Continue
        }
    }
}

/// The most `Simple` segments a semicolon-form colour can use after its
/// `38`/`48`/`58`: the colour type plus `R;G;B`.
const MAX_SEMICOLON_COLOR_VALUES: usize = 4;

/// Extract a semicolon-form colour (`38;5;N`, `38;2;R;G;B`, and the `48` /
/// `58` equivalents) following a `38`/`48`/`58` that was just read.
///
/// Peeks up to [`MAX_SEMICOLON_COLOR_VALUES`] following `Simple` segments
/// without consuming them, then consumes exactly the ones the colour used. An
/// incomplete colour consumes everything it peeked and emits nothing. An
/// unknown colour type emits nothing and stops the rest of the sequence.
fn handle_semicolon_color(
    primary: usize,
    iter: &mut Iter<'_, SegmentValue>,
    output: &mut Vec<TerminalOutput>,
) -> SgrFlow {
    let mut window = [None; MAX_SEMICOLON_COLOR_VALUES];
    let mut len = 0;
    for (slot, segment) in window.iter_mut().zip(iter.clone()) {
        let SegmentValue::Simple(value) = segment else {
            break;
        };
        *slot = *value;
        len += 1;
    }
    let window = window.get(..len).unwrap_or_default();

    match parse_sgr_color(window, SgrColorForm::Semicolon) {
        Ok(extracted) => {
            output.push(color_sgr(primary, extracted.color));
            advance(iter, extracted.consumed);
            SgrFlow::Continue
        }
        Err(SgrColorError::Incomplete) => {
            advance(iter, len);
            SgrFlow::Continue
        }
        Err(SgrColorError::UnknownType(kind)) => {
            warn!("Unhandled SGR color type: parsed SGR {primary};{kind}; dropping the rest");
            SgrFlow::Stop
        }
    }
}

/// Discard the next `count` segments from `iter`.
fn advance(iter: &mut Iter<'_, SegmentValue>, count: usize) {
    for _ in 0..count {
        let _ = iter.next();
    }
}

/// Build the SGR that applies `color` to the channel selected by `primary`
/// (`38` foreground, `48` background, anything else underline colour).
const fn color_sgr(primary: usize, color: TerminalColor) -> TerminalOutput {
    // Only ever called with 38, 48 or 58; matching 38/48 explicitly and
    // treating the remainder as 58 avoids an unreachable arm.
    TerminalOutput::Sgr(match primary {
        38 => SelectGraphicRendition::Foreground(color),
        48 => SelectGraphicRendition::Background(color),
        _ => SelectGraphicRendition::UnderlineColor(color),
    })
}

/// Process a single semicolon-delimited segment that contains colons.
///
/// Colons delimit subparameters within one SGR code.  The first value is the
/// primary SGR parameter; subsequent values are subparameters.
///
/// Supported colon forms:
/// - `4:N` — underline style (N=0 off, 1 single, 2 double, 3 curly, 4 dotted, 5 dashed)
/// - `38:2:...:R:G:B` — truecolor foreground
/// - `48:2:...:R:G:B` — truecolor background
/// - `58:2:...:R:G:B` — truecolor underline color
/// - `38:5:IDX` / `48:5:IDX` / `58:5:IDX` — 256-color palette
fn process_colon_segment(segment: &[u8], output: &mut Vec<TerminalOutput>) -> SgrFlow {
    let Ok(parts) = segment
        .split(|b| *b == b':')
        .map(parse_param_as::<usize>)
        .collect::<Result<Vec<Option<usize>>, _>>()
    else {
        output.push(TerminalOutput::Invalid);
        return SgrFlow::Continue;
    };

    let Some(primary) = parts.first().copied().flatten() else {
        output.push(TerminalOutput::Sgr(SelectGraphicRendition::Reset));
        return SgrFlow::Continue;
    };

    match primary {
        4 => handle_underline_subparam(&parts, output),
        38 | 48 | 58 => return handle_colon_color(primary, &parts, output),
        _ => {
            // Unknown colon form — emit the primary as a plain SGR and warn.
            warn!(
                "Unknown colon-form SGR: primary={primary}; raw segment: \"{}\"",
                escape_sequence_for_log(segment)
            );
            output.push(TerminalOutput::Sgr(SelectGraphicRendition::from_usize(
                primary,
            )));
        }
    }
    SgrFlow::Continue
}

/// Extract a colon-form colour (`38:5:N`, `38:2:[CS:]R:G:B`, and the `48` /
/// `58` equivalents). `parts` includes the leading `38`/`48`/`58`. An
/// incomplete colour emits nothing; an unknown colour type emits nothing and
/// stops the rest of the sequence.
fn handle_colon_color(
    primary: usize,
    parts: &[Option<usize>],
    output: &mut Vec<TerminalOutput>,
) -> SgrFlow {
    let values = parts.get(1..).unwrap_or_default();
    match parse_sgr_color(values, SgrColorForm::Colon) {
        Ok(extracted) => {
            output.push(color_sgr(primary, extracted.color));
            SgrFlow::Continue
        }
        Err(SgrColorError::Incomplete) => SgrFlow::Continue,
        Err(SgrColorError::UnknownType(kind)) => {
            warn!("Unhandled SGR color type: parsed SGR {primary}:{kind}; dropping the rest");
            SgrFlow::Stop
        }
    }
}

/// Handle `4:N` underline subparameter.
fn handle_underline_subparam(parts: &[Option<usize>], output: &mut Vec<TerminalOutput>) {
    let style_param = parts.get(1).copied().flatten().unwrap_or(0);
    let style = UnderlineStyle::from_sgr_param(style_param);

    if style.is_active() {
        output.push(TerminalOutput::Sgr(
            SelectGraphicRendition::UnderlineWithStyle(style),
        ));
    } else {
        // 4:0 means "no underline" — same as SGR 24
        output.push(TerminalOutput::Sgr(SelectGraphicRendition::NotUnderlined));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi_components::csi::AnsiCsiParser;
    use freminal_common::buffer_states::terminal_output::TerminalOutput;
    use freminal_common::colors::TerminalColor;
    use freminal_common::sgr::SelectGraphicRendition;

    #[test]
    fn sgr_empty_params_emits_reset() {
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::Sgr(SelectGraphicRendition::Reset)]
        );
    }

    /// Feed a full CSI sequence (everything after ESC[) through the parser.
    fn parse_csi_sequence(bytes: &[u8]) -> Vec<TerminalOutput> {
        let mut parser = AnsiCsiParser::new();
        let mut output = Vec::new();
        for &b in bytes {
            parser.ansiparser_inner_csi(b, &mut output);
        }
        output
    }

    #[test]
    fn sgr_gt_prefix_is_routed_to_modify_other_keys() {
        // `CSI > 4 ; 1 m` is XTMODKEYS, routed past the SGR handler.
        assert_eq!(
            parse_csi_sequence(b">4;1m"),
            vec![TerminalOutput::ModifyOtherKeys(1)]
        );
    }

    #[test]
    fn sgr_gt4_no_level_is_routed_to_modify_other_keys_reset() {
        // `CSI > 4 m` (no level) → modifyOtherKeys level 0 (reset)
        assert_eq!(
            parse_csi_sequence(b">4m"),
            vec![TerminalOutput::ModifyOtherKeys(0)]
        );
    }

    #[test]
    fn sgr_semicolon_form_truecolor_fg() {
        // `38;2;255;128;0` → truecolor foreground
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"38;2;255;128;0", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::Sgr(SelectGraphicRendition::Foreground(
                TerminalColor::Custom(255, 128, 0)
            ))]
        );
    }

    #[test]
    fn sgr_semicolon_form_256_color_bg() {
        // `48;5;200` → 256-color background palette index 200
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"48;5;200", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::Sgr(SelectGraphicRendition::Background(
                TerminalColor::PaletteIndex(200)
            ))]
        );
    }

    #[test]
    fn sgr_semicolon_form_truecolor_underline_color() {
        // `58;2;0;255;0` → truecolor underline color
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"58;2;0;255;0", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::Sgr(SelectGraphicRendition::UnderlineColor(
                TerminalColor::Custom(0, 255, 0)
            ))]
        );
    }

    #[test]
    fn sgr_bare_38_48_58_emit_nothing() {
        // A bare colour introducer is incomplete: nothing is emitted (kitty),
        // in particular no reset to the default colour.
        for seq in [&b"38"[..], b"48", b"58"] {
            let mut output = Vec::new();
            let result = ansi_parser_inner_csi_finished_sgr(seq, &mut output);
            assert_eq!(result, ParserOutcome::Finished);
            assert_eq!(output, vec![], "bare {seq:?} must emit nothing");
        }
    }

    #[test]
    fn sgr_colon_form_truecolor_fg() {
        // `38:2::255:0:0` → truecolor foreground via colon form
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"38:2::255:0:0", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::Sgr(SelectGraphicRendition::Foreground(
                TerminalColor::Custom(255, 0, 0)
            ))]
        );
    }

    #[test]
    fn sgr_colon_form_256_color_bg() {
        // `48:5:200` → 256-color background via colon form
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"48:5:200", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::Sgr(SelectGraphicRendition::Background(
                TerminalColor::PaletteIndex(200)
            ))]
        );
    }

    #[test]
    fn sgr_colon_form_curly_underline() {
        // `4:3` → curly underline
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"4:3", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert!(matches!(
            output[0],
            TerminalOutput::Sgr(SelectGraphicRendition::UnderlineWithStyle(_))
        ));
    }

    #[test]
    fn sgr_colon_form_underline_off() {
        // `4:0` → no underline (SGR 24)
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"4:0", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::Sgr(SelectGraphicRendition::NotUnderlined)]
        );
    }

    #[test]
    fn sgr_unknown_color_type_drops_rest_of_sequence() {
        // `38;9;1;2;3` → type 9 is unknown: nothing is emitted for the colour
        // and the remaining params (1, 2, 3) are dropped too.
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"38;9;1;2;3", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![]);
    }

    #[test]
    fn sgr_non_numeric_param_emits_invalid() {
        // non-numeric param (parse error → Simple(None)) → Invalid
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"abc", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::Invalid]);
    }

    #[test]
    fn sgr_256_color_fg_semicolon() {
        // `38;5;100` → Foreground(PaletteIndex(100))
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"38;5;100", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::Sgr(SelectGraphicRendition::Foreground(
                TerminalColor::PaletteIndex(100)
            ))]
        );
    }

    #[test]
    fn sgr_256_color_underline_semicolon() {
        // `58;5;42` → UnderlineColor(PaletteIndex(42))
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"58;5;42", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::Sgr(SelectGraphicRendition::UnderlineColor(
                TerminalColor::PaletteIndex(42)
            ))]
        );
    }

    // --- Parser-level colour extraction (full `CSI ... m`) ---

    fn fg(color: TerminalColor) -> TerminalOutput {
        TerminalOutput::Sgr(SelectGraphicRendition::Foreground(color))
    }

    fn sgr(value: SelectGraphicRendition) -> TerminalOutput {
        TerminalOutput::Sgr(value)
    }

    #[test]
    fn csi_indexed_colour_is_masked_to_8_bits() {
        assert_eq!(
            parse_csi_sequence(b"38;5;256m"),
            vec![fg(TerminalColor::PaletteIndex(0))]
        );
        assert_eq!(
            parse_csi_sequence(b"38;5;300m"),
            vec![fg(TerminalColor::PaletteIndex(44))]
        );
    }

    #[test]
    fn csi_incomplete_colours_emit_nothing() {
        for seq in [
            &b"38;5m"[..],
            b"38m",
            b"48m",
            b"58m",
            b"38;2;1;2m",
            b"38:5m",
            b"38:2:1:2m",
        ] {
            assert_eq!(
                parse_csi_sequence(seq),
                vec![],
                "incomplete colour {:?} must emit nothing",
                String::from_utf8_lossy(seq)
            );
        }
    }

    #[test]
    fn csi_direct_colour_components_are_masked_to_8_bits() {
        assert_eq!(
            parse_csi_sequence(b"38;2;300;0;0m"),
            vec![fg(TerminalColor::Custom(44, 0, 0))]
        );
    }

    #[test]
    fn csi_direct_colour_colon_forms() {
        for seq in [
            &b"38:2:1:2:3m"[..],
            b"38:2::1:2:3m",
            b"38:2:9:1:2:3m",
            // Extra trailing sub-params after a complete colour are ignored.
            b"38:2:9:1:2:3:4m",
        ] {
            assert_eq!(
                parse_csi_sequence(seq),
                vec![fg(TerminalColor::Custom(1, 2, 3))],
                "colon colour {:?}",
                String::from_utf8_lossy(seq)
            );
        }
    }

    #[test]
    fn csi_colon_indexed_colour_ignores_extra_subparams() {
        assert_eq!(
            parse_csi_sequence(b"38:5:1:2m"),
            vec![fg(TerminalColor::PaletteIndex(1))]
        );
    }

    #[test]
    fn csi_unknown_colour_type_emits_nothing_and_drops_rest() {
        assert_eq!(parse_csi_sequence(b"38;4;1;1m"), vec![]);
        assert_eq!(parse_csi_sequence(b"38;0;1m"), vec![]);
        assert_eq!(parse_csi_sequence(b"38:4:1;1m"), vec![]);
    }

    #[test]
    fn csi_params_before_unknown_colour_type_are_kept() {
        assert_eq!(
            parse_csi_sequence(b"1;38;4;1m"),
            vec![sgr(SelectGraphicRendition::Bold)]
        );
    }

    #[test]
    fn csi_incomplete_colour_does_not_drop_earlier_params() {
        assert_eq!(
            parse_csi_sequence(b"1;38;5m"),
            vec![sgr(SelectGraphicRendition::Bold)]
        );
    }

    #[test]
    fn csi_params_after_complete_colour_are_processed() {
        assert_eq!(
            parse_csi_sequence(b"38;5;1;1m"),
            vec![
                fg(TerminalColor::PaletteIndex(1)),
                sgr(SelectGraphicRendition::Bold),
            ]
        );
    }

    #[test]
    fn csi_background_and_underline_indexed_colours() {
        assert_eq!(
            parse_csi_sequence(b"48;5;7m"),
            vec![sgr(SelectGraphicRendition::Background(
                TerminalColor::PaletteIndex(7)
            ))]
        );
        assert_eq!(
            parse_csi_sequence(b"58;5;7m"),
            vec![sgr(SelectGraphicRendition::UnderlineColor(
                TerminalColor::PaletteIndex(7)
            ))]
        );
    }
}
