// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::iter::Peekable;
use std::vec::IntoIter;

use crate::ansi::{ParserOutcome, parse_param_as};
use crate::ansi_components::csi_commands::modify_other_keys::ansi_parser_inner_csi_finished_modify_other_keys;
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
/// When `params` starts with `>`, dispatches to the xterm `modifyOtherKeys`
/// handler instead.
pub fn ansi_parser_inner_csi_finished_sgr(
    params: &[u8],
    output: &mut Vec<TerminalOutput>,
) -> ParserOutcome {
    if params.first() == Some(&b'>') {
        return ansi_parser_inner_csi_finished_modify_other_keys(params, output);
    }

    // Split by semicolons first — each segment is a top-level SGR parameter
    // that may itself contain colon-delimited subparameters.
    let segments: Vec<&[u8]> = params.split(|b| *b == b';').collect();

    // Empty SGR params means reset (0)
    if segments.is_empty() || (segments.len() == 1 && segments[0].is_empty()) {
        output.push(TerminalOutput::Sgr(SelectGraphicRendition::Reset));
        return ParserOutcome::Finished;
    }

    // Use an index-based loop because some SGR codes (38, 48, 58) consume
    // multiple subsequent semicolon-separated segments as their parameters.
    let parsed = parse_all_segments(&segments);
    let mut iter = parsed.into_iter().peekable();

    while let Some(val) = iter.next() {
        dispatch_sgr_value(val, &mut iter, output);
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

/// Dispatch a single SGR value, consuming additional iterator items for
/// multi-param codes (38, 48, 58 in semicolon form).
fn dispatch_sgr_value(
    value: SegmentValue,
    iter: &mut Peekable<IntoIter<SegmentValue>>,
    output: &mut Vec<TerminalOutput>,
) {
    match value {
        SegmentValue::Colon(raw) => process_colon_segment(&raw, output),
        SegmentValue::Simple(None) => output.push(TerminalOutput::Invalid),
        SegmentValue::Simple(Some(param)) => {
            dispatch_simple_param(param, iter, output);
        }
    }
}

/// Handle a plain numeric SGR parameter.  For 38/48/58, consume subsequent
/// segments from the iterator as color subparameters (semicolon form).
fn dispatch_simple_param(
    param: usize,
    iter: &mut Peekable<IntoIter<SegmentValue>>,
    output: &mut Vec<TerminalOutput>,
) {
    match param {
        38 | 48 | 58 => {
            // Collect remaining simple values for the color handler
            let mut color_params: Vec<Option<usize>> = Vec::new();
            consume_color_params(iter, &mut color_params);
            let mut color_iter = color_params.into_iter();
            handle_custom_color(output, &mut color_iter, param, false);
        }
        _ => {
            output.push(TerminalOutput::Sgr(SelectGraphicRendition::from_usize(
                param,
            )));
        }
    }
}

/// Consume subsequent `Simple` segments from the iterator that belong to a
/// semicolon-form color specification (mode + up to 4 values).
fn consume_color_params(iter: &mut Peekable<IntoIter<SegmentValue>>, out: &mut Vec<Option<usize>>) {
    // Peek at the next segment to get the color mode.  If it's not a simple
    // value, bail without consuming — the segment will be dispatched normally
    // by the caller's main loop.
    let mode = match iter.peek() {
        Some(SegmentValue::Simple(v)) => *v,
        _ => return,
    };
    // Safe to consume after peeking.
    iter.next();
    out.push(mode);

    // Determine how many more values to consume based on the mode
    let count = match mode {
        Some(2) => 3, // r, g, b
        Some(5) => 1, // palette index
        _ => return,  // unknown mode, stop
    };

    for _ in 0..count {
        match iter.peek() {
            Some(SegmentValue::Simple(_)) => {
                if let Some(SegmentValue::Simple(v)) = iter.next() {
                    out.push(v);
                }
            }
            _ => break, // colon segment or end — stop without consuming
        }
    }
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
fn process_colon_segment(segment: &[u8], output: &mut Vec<TerminalOutput>) {
    let Ok(parts) = segment
        .split(|b| *b == b':')
        .map(parse_param_as::<usize>)
        .collect::<Result<Vec<Option<usize>>, _>>()
    else {
        output.push(TerminalOutput::Invalid);
        return;
    };

    let Some(primary) = parts.first().copied().flatten() else {
        output.push(TerminalOutput::Sgr(SelectGraphicRendition::Reset));
        return;
    };

    match primary {
        4 => handle_underline_subparam(&parts, output),
        38 | 48 | 58 => {
            let mut iter: IntoIter<Option<usize>> = parts.into_iter();
            let _ = iter.next(); // skip the primary (38/48/58)
            handle_custom_color(output, &mut iter, primary, true);
        }
        _ => {
            // Unknown colon form — emit the primary as a plain SGR and warn.
            debug!("Unknown colon-form SGR: primary={primary}");
            output.push(TerminalOutput::Sgr(SelectGraphicRendition::from_usize(
                primary,
            )));
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

fn default_color(output: &mut Vec<TerminalOutput>, custom_color_control_code: usize) {
    // NOTE: Per xterm/VTE convention, bare 38/48/58 with no subparam resets the respective
    // color channel. This is not explicitly specified in ECMA-48 but is de facto standard
    // across all major terminal emulators.

    output.push(match custom_color_control_code {
        38 => TerminalOutput::Sgr(SelectGraphicRendition::Foreground(TerminalColor::Default)),
        48 => TerminalOutput::Sgr(SelectGraphicRendition::Background(
            TerminalColor::DefaultBackground,
        )),
        // instead of matching directly on 58, we'll match on a wildcard. This helps with codecov because it thought
        // we were testing `_` in the match statement when it's impossible to end up here with a value other than 58
        _ => TerminalOutput::Sgr(SelectGraphicRendition::UnderlineColor(
            TerminalColor::DefaultUnderlineColor,
        )),
    });
}

pub fn handle_custom_color(
    output: &mut Vec<TerminalOutput>,
    param_iter: &mut IntoIter<Option<usize>>,
    param: usize,
    split_by_colon: bool,
) {
    // if control code is 38, 48 or 58 we need to read the next param
    // otherwise, store the param as is
    let custom_color_control_code = param;

    let next = param_iter.next();
    let Some(param) = next.flatten() else {
        // No mode parameter after 38/48/58 → treat as a reset for that channel
        default_color(output, custom_color_control_code);
        return;
    };

    match param {
        // Truecolor: 2;r;g;b  or  2:r:g:b
        2 => {
            // Some colon-splitters may leave an extra token after the 2; skip it if present.
            if param_iter.len() > 3 && split_by_colon {
                let _ = param_iter.next();
            }

            let r = param_iter.next().flatten().unwrap_or(0);
            let g = param_iter.next().flatten().unwrap_or(0);
            let b = param_iter.next().flatten().unwrap_or(0);

            match SelectGraphicRendition::from_usize_color(custom_color_control_code, r, g, b) {
                Ok(sgr) => output.push(TerminalOutput::Sgr(sgr)),
                Err(e) => {
                    warn!("Invalid RGB SGR sequence: {}", e);
                    output.push(TerminalOutput::Invalid);
                }
            }
        }

        // 256-color: 5;idx
        5 => {
            let lookup = param_iter.next().flatten().unwrap_or(0);

            // Clamp to 0–255 and emit a PaletteIndex.  The handler
            // resolves it against the mutable ColorPalette.
            let idx = u8::try_from(lookup & 0xFF).unwrap_or(0);
            let color = TerminalColor::PaletteIndex(idx);

            match custom_color_control_code {
                38 => output.push(TerminalOutput::Sgr(SelectGraphicRendition::Foreground(
                    color,
                ))),
                48 => output.push(TerminalOutput::Sgr(SelectGraphicRendition::Background(
                    color,
                ))),
                58 => output.push(TerminalOutput::Sgr(SelectGraphicRendition::UnderlineColor(
                    color,
                ))),
                _ => output.push(TerminalOutput::Invalid),
            }
        }

        _ => {
            warn!("Invalid SGR sequence: {}", param);
            output.push(TerminalOutput::Invalid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn sgr_gt_prefix_delegates_to_modify_other_keys() {
        // `>4;1` → modifyOtherKeys level 1
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b">4;1", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::ModifyOtherKeys(1)]);
    }

    #[test]
    fn sgr_gt4_no_level_resets_modify_other_keys() {
        // `>4` (no level) → modifyOtherKeys level 0 (reset)
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b">4", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(output, vec![TerminalOutput::ModifyOtherKeys(0)]);
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
    fn sgr_bare_38_no_subparams_resets_fg_to_default() {
        // bare `38` (no mode after) → Foreground(Default)
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"38", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::Sgr(SelectGraphicRendition::Foreground(
                TerminalColor::Default
            ))]
        );
    }

    #[test]
    fn sgr_bare_48_no_subparams_resets_bg_to_default() {
        // bare `48` → Background(DefaultBackground)
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"48", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::Sgr(SelectGraphicRendition::Background(
                TerminalColor::DefaultBackground
            ))]
        );
    }

    #[test]
    fn sgr_bare_58_no_subparams_resets_underline_color_to_default() {
        // bare `58` → UnderlineColor(DefaultUnderlineColor)
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"58", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![TerminalOutput::Sgr(SelectGraphicRendition::UnderlineColor(
                TerminalColor::DefaultUnderlineColor
            ))]
        );
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
    fn sgr_unknown_color_mode_emits_invalid() {
        // `38;9;1;2;3` → mode=9 is unknown → Invalid for the color, then remaining
        // params (1=Bold, 2=Faint, 3=Italic) are processed normally.
        let mut output = Vec::new();
        let result = ansi_parser_inner_csi_finished_sgr(b"38;9;1;2;3", &mut output);
        assert_eq!(result, ParserOutcome::Finished);
        assert_eq!(
            output,
            vec![
                TerminalOutput::Invalid,
                TerminalOutput::Sgr(
                    crate::ansi_components::csi_commands::sgr::SelectGraphicRendition::Bold
                ),
                TerminalOutput::Sgr(
                    crate::ansi_components::csi_commands::sgr::SelectGraphicRendition::Faint
                ),
                TerminalOutput::Sgr(
                    crate::ansi_components::csi_commands::sgr::SelectGraphicRendition::Italic
                ),
            ]
        );
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
}
