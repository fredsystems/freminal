// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parsing of the OSC colour-control sequences: OSC 4 / 104 (palette) and
//! OSC 10-19 / 110-119 (dynamic colours).
//!
//! All four parsers work on the raw OSC body (`number[;field...]`, terminator
//! already stripped) split on `;` as **bytes**, not through the generic
//! tokenizer: a colour name may contain spaces and a value may be any text, so
//! each field is checked for UTF-8 only where it is used as a colour spec.
//!
//! Semantics follow xterm's `ctlseqs` and kitty:
//!
//! - **OSC 4** takes `index ; spec` pairs. An odd field count drops the whole
//!   sequence; an invalid pair is skipped and the rest are kept.
//! - **OSC 104** takes zero or more indices; none (or only empty fields) means
//!   the whole palette.
//! - **OSC 10-19** takes one value per consecutive code, starting at the OSC
//!   number: `OSC 10 ; fg ; bg ST` sets the foreground and the background.
//! - **OSC 110-119** reset one dynamic colour and ignore any parameters.
//!
//! Logging is payload-safe: invalid fields are reported by a count in a single
//! payload-free `warn!` per sequence, and the bounded raw sequence goes to an
//! adjacent `debug!`.

use crate::ansi_components::tracer::escape_sequence_for_log_bounded;
use freminal_common::buffer_states::osc::AnsiOscType;
use freminal_common::buffer_states::osc_color::{
    ColorRequest, DynamicColorOp, PaletteColorOp, PaletteReset,
};
use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_common::colors::{DynamicColorSlot, parse_color_spec};

/// The highest OSC code of the dynamic-colour range (OSC 10-19).
const LAST_DYNAMIC_CODE: u16 = 19;

/// The distance between a dynamic-colour set code and its reset code
/// (OSC 110 resets what OSC 10 sets).
const RESET_CODE_OFFSET: u16 = 100;

/// The `;`-separated fields of an OSC body after the OSC number.
fn fields_after_number(raw_params: &[u8]) -> impl Iterator<Item = &[u8]> {
    raw_params.split(|b| *b == b';').skip(1)
}

/// Parse a palette index: a non-empty run of ASCII decimal digits with a value
/// of 0..=255. Signs, whitespace and values over 255 are rejected.
fn parse_palette_index(field: &[u8]) -> Option<u8> {
    if field.is_empty() || !field.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(field).ok()?.parse::<u8>().ok()
}

/// Parse one colour value: `?` is a query, anything else must be a UTF-8 colour
/// spec accepted by [`parse_color_spec`]. `None` means the value is invalid;
/// the caller counts it and reports once per sequence (see [`log_skipped`]).
fn parse_color_request(field: &[u8]) -> Option<ColorRequest> {
    if field == b"?" {
        return Some(ColorRequest::Query);
    }
    let spec = std::str::from_utf8(field).ok()?;
    parse_color_spec(spec).ok().map(ColorRequest::Set)
}

/// Report the invalid fields skipped in one sequence.
///
/// One payload-free `warn!` per sequence (never one per field, so a long
/// malformed body cannot flood the log) and one `debug!` with the bounded raw
/// sequence for auditing. Nothing is logged when `skipped` is zero.
fn log_skipped(osc: &str, skipped: usize, raw_params: &[u8]) {
    if skipped == 0 {
        return;
    }
    tracing::warn!("{osc}: {skipped} invalid value(s) skipped");
    tracing::debug!(
        "{osc}: invalid value(s) skipped; raw sequence: \"{}\"",
        escape_sequence_for_log_bounded(raw_params)
    );
}

/// Handle OSC 4 (palette colour set / query).
///
/// Format: `OSC 4 ; c1 ; spec1 ; c2 ; spec2 ... ST`. Each `spec` is `?` (query)
/// or any colour spec [`parse_color_spec`] accepts. An odd number of fields
/// after the `4` drops the whole sequence; an invalid index or spec skips just
/// that pair. Nothing is emitted when no pair is valid.
pub(super) fn handle_osc_palette_color(raw_params: &[u8], output: &mut Vec<TerminalOutput>) {
    let fields: Vec<&[u8]> = fields_after_number(raw_params).collect();
    if !fields.len().is_multiple_of(2) {
        tracing::warn!("OSC 4: odd number of fields; whole sequence dropped");
        tracing::debug!(
            "OSC 4: odd number of fields ({}); raw sequence: \"{}\"",
            fields.len(),
            escape_sequence_for_log_bounded(raw_params)
        );
        return;
    }

    let mut ops = Vec::new();
    let mut skipped = 0usize;
    let (pairs, _) = fields.as_chunks::<2>();
    for [index, spec] in pairs {
        match (parse_palette_index(index), parse_color_request(spec)) {
            (Some(index), Some(request)) => ops.push(PaletteColorOp { index, request }),
            _ => skipped += 1,
        }
    }
    log_skipped("OSC 4", skipped, raw_params);

    if !ops.is_empty() {
        output.push(TerminalOutput::OscResponse(AnsiOscType::PaletteColors(ops)));
    }
}

/// Handle OSC 104 (palette colour reset).
///
/// Format: `OSC 104 ST` or `OSC 104 ; ST` (every field empty) resets the whole
/// palette; otherwise each field is a decimal index 0..=255 and the valid ones
/// are reset in order. Invalid or empty fields are skipped, and nothing is
/// emitted when none is valid.
pub(super) fn handle_osc_reset_palette(raw_params: &[u8], output: &mut Vec<TerminalOutput>) {
    let fields: Vec<&[u8]> = fields_after_number(raw_params).collect();

    if fields.iter().all(|field| field.is_empty()) {
        output.push(TerminalOutput::OscResponse(
            AnsiOscType::ResetPaletteColors(PaletteReset::All),
        ));
        return;
    }

    let mut indices = Vec::new();
    let mut skipped = 0usize;
    for field in fields {
        match parse_palette_index(field) {
            Some(index) => indices.push(index),
            None if field.is_empty() => {}
            None => skipped += 1,
        }
    }
    log_skipped("OSC 104", skipped, raw_params);

    if !indices.is_empty() {
        output.push(TerminalOutput::OscResponse(
            AnsiOscType::ResetPaletteColors(PaletteReset::Indices(indices)),
        ));
    }
}

/// Handle OSC 10-19 (dynamic colour set / query).
///
/// `start_code` is the OSC number (`10..=19`). Value `k` (counting from 0)
/// applies to code `start_code + k`, and values past code 19 are ignored. Codes
/// 10, 11, 12, 17 and 19 map to a [`DynamicColorSlot`]; 13, 14, 15, 16 and 18
/// (pointer colours, Tektronix colours) consume their value and are ignored.
/// An empty value or an invalid spec skips just that code. Nothing is emitted
/// when no value targets a supported slot.
pub(super) fn handle_osc_dynamic_colors(
    start_code: u16,
    raw_params: &[u8],
    output: &mut Vec<TerminalOutput>,
) {
    let mut ops = Vec::new();
    let mut skipped = 0usize;

    for (code, field) in (start_code..=LAST_DYNAMIC_CODE).zip(fields_after_number(raw_params)) {
        let Some(slot) = DynamicColorSlot::from_osc_code(code) else {
            tracing::debug!("OSC {code}: unsupported dynamic colour; value ignored");
            continue;
        };
        if field.is_empty() {
            continue;
        }
        match parse_color_request(field) {
            Some(request) => ops.push(DynamicColorOp { slot, request }),
            None => skipped += 1,
        }
    }
    log_skipped("OSC 10-19", skipped, raw_params);

    if !ops.is_empty() {
        output.push(TerminalOutput::OscResponse(AnsiOscType::DynamicColors(ops)));
    }
}

/// Handle OSC 110-119 (dynamic colour reset).
///
/// `reset_code` is the OSC number (`110..=119`). Codes 110, 111, 112, 117 and
/// 119 reset their slot; 113, 114, 115, 116 and 118 are recognised and ignored.
/// Any parameters after the number are ignored.
pub(super) fn handle_osc_reset_dynamic_color(reset_code: u16, output: &mut Vec<TerminalOutput>) {
    let slot = DynamicColorSlot::from_osc_code(reset_code.saturating_sub(RESET_CODE_OFFSET));
    if let Some(slot) = slot {
        output.push(TerminalOutput::OscResponse(AnsiOscType::ResetDynamicColor(
            slot,
        )));
    } else {
        tracing::debug!("OSC {reset_code}: unsupported dynamic colour reset; ignored");
    }
}

#[cfg(test)]
mod tests {
    use super::super::osc::AnsiOscParser;
    use freminal_common::buffer_states::osc::AnsiOscType;
    use freminal_common::buffer_states::osc_color::{
        ColorRequest, DynamicColorOp, PaletteColorOp, PaletteReset,
    };
    use freminal_common::buffer_states::terminal_output::TerminalOutput;
    use freminal_common::colors::DynamicColorSlot;

    fn feed_osc(payload: &[u8]) -> Vec<TerminalOutput> {
        let mut parser = AnsiOscParser::new();
        let mut output = Vec::new();
        for &b in payload {
            parser.ansiparser_inner_osc(b, &mut output);
        }
        output
    }

    fn palette(ops: &[(u8, ColorRequest)]) -> Vec<TerminalOutput> {
        vec![TerminalOutput::OscResponse(AnsiOscType::PaletteColors(
            ops.iter()
                .map(|&(index, request)| PaletteColorOp { index, request })
                .collect(),
        ))]
    }

    fn dynamic(ops: &[(DynamicColorSlot, ColorRequest)]) -> Vec<TerminalOutput> {
        vec![TerminalOutput::OscResponse(AnsiOscType::DynamicColors(
            ops.iter()
                .map(|&(slot, request)| DynamicColorOp { slot, request })
                .collect(),
        ))]
    }

    fn reset_palette(reset: PaletteReset) -> Vec<TerminalOutput> {
        vec![TerminalOutput::OscResponse(
            AnsiOscType::ResetPaletteColors(reset),
        )]
    }

    fn reset_dynamic(slot: DynamicColorSlot) -> Vec<TerminalOutput> {
        vec![TerminalOutput::OscResponse(AnsiOscType::ResetDynamicColor(
            slot,
        ))]
    }

    use ColorRequest::{Query, Set};
    use DynamicColorSlot::{
        Background, Cursor, Foreground, SelectionBackground, SelectionForeground,
    };

    // ------------------------------------------------------------------
    // OSC 4
    // ------------------------------------------------------------------

    #[test]
    fn osc4_set_palette_color_rgb_format() {
        assert_eq!(
            feed_osc(b"4;10;rgb:ff/00/80\x07"),
            palette(&[(10, Set((0xff, 0x00, 0x80)))])
        );
    }

    #[test]
    fn osc4_set_palette_color_hash_format() {
        assert_eq!(
            feed_osc(b"4;42;#aabbcc\x1b\\"),
            palette(&[(42, Set((0xaa, 0xbb, 0xcc)))])
        );
    }

    #[test]
    fn osc4_query_palette_color() {
        assert_eq!(feed_osc(b"4;5;?\x07"), palette(&[(5, Query)]));
    }

    #[test]
    fn osc4_multiple_pairs_keep_wire_order() {
        assert_eq!(
            feed_osc(b"4;1;red;2;?\x07"),
            palette(&[(1, Set((255, 0, 0))), (2, Query)])
        );
    }

    #[test]
    fn osc4_odd_field_count_drops_the_whole_sequence() {
        assert_eq!(feed_osc(b"4;1;red;2\x07"), []);
        // A lone index, and a trailing separator, are odd too.
        assert_eq!(feed_osc(b"4;10\x07"), []);
        assert_eq!(feed_osc(b"4;\x07"), []);
    }

    #[test]
    fn osc4_bare_number_emits_nothing() {
        assert_eq!(feed_osc(b"4\x07"), []);
    }

    #[test]
    fn osc4_out_of_range_index_skips_only_that_pair() {
        assert_eq!(
            feed_osc(b"4;300;red;2;blue\x07"),
            palette(&[(2, Set((0, 0, 255)))])
        );
        assert_eq!(feed_osc(b"4;300;rgb:ff/ff/ff\x07"), []);
    }

    #[test]
    fn osc4_non_decimal_index_skips_only_that_pair() {
        assert_eq!(
            feed_osc(b"4;+1;red;0x2;red;3;blue\x07"),
            palette(&[(3, Set((0, 0, 255)))])
        );
        assert_eq!(feed_osc(b"4;;red;-1;red\x07"), []);
    }

    #[test]
    fn osc4_invalid_spec_skips_only_that_pair() {
        assert_eq!(
            feed_osc(b"4;1;notacolor;2;blue\x07"),
            palette(&[(2, Set((0, 0, 255)))])
        );
        assert_eq!(feed_osc(b"4;7;notacolor\x07"), []);
    }

    #[test]
    fn osc4_empty_spec_skips_only_that_pair() {
        assert_eq!(
            feed_osc(b"4;1;;2;blue\x07"),
            palette(&[(2, Set((0, 0, 255)))])
        );
    }

    #[test]
    fn osc4_non_utf8_spec_skips_only_that_pair() {
        assert_eq!(
            feed_osc(b"4;1;\xff\xfe;2;blue\x07"),
            palette(&[(2, Set((0, 0, 255)))])
        );
    }

    #[test]
    fn osc4_colour_name_with_spaces() {
        assert_eq!(
            feed_osc(b"4;1;dark slate gray\x07"),
            palette(&[(1, Set((47, 79, 79)))])
        );
    }

    #[test]
    fn osc4_wide_gamut_spec() {
        assert_eq!(
            feed_osc(b"4;1;oklch(0.5 0.1 180)\x07"),
            palette(&[(1, Set((0, 117, 101)))])
        );
    }

    #[test]
    fn osc4_leading_zero_index_is_decimal() {
        assert_eq!(
            feed_osc(b"4;007;red\x07"),
            palette(&[(7, Set((255, 0, 0)))])
        );
    }

    #[test]
    fn osc4_highest_index() {
        assert_eq!(feed_osc(b"4;255;?\x07"), palette(&[(255, Query)]));
    }

    // ------------------------------------------------------------------
    // OSC 104
    // ------------------------------------------------------------------

    #[test]
    fn osc104_without_index_resets_all() {
        assert_eq!(feed_osc(b"104\x07"), reset_palette(PaletteReset::All));
    }

    #[test]
    fn osc104_with_only_empty_fields_resets_all() {
        assert_eq!(feed_osc(b"104;\x07"), reset_palette(PaletteReset::All));
        assert_eq!(feed_osc(b"104;;\x07"), reset_palette(PaletteReset::All));
    }

    #[test]
    fn osc104_single_index() {
        assert_eq!(
            feed_osc(b"104;42\x07"),
            reset_palette(PaletteReset::Indices(vec![42]))
        );
    }

    #[test]
    fn osc104_multiple_indices_keep_wire_order() {
        assert_eq!(
            feed_osc(b"104;1;2\x07"),
            reset_palette(PaletteReset::Indices(vec![1, 2]))
        );
        assert_eq!(
            feed_osc(b"104;9;1\x07"),
            reset_palette(PaletteReset::Indices(vec![9, 1]))
        );
    }

    #[test]
    fn osc104_skips_invalid_and_empty_fields() {
        assert_eq!(
            feed_osc(b"104;x;3\x07"),
            reset_palette(PaletteReset::Indices(vec![3]))
        );
        assert_eq!(
            feed_osc(b"104;1;;2\x07"),
            reset_palette(PaletteReset::Indices(vec![1, 2]))
        );
        assert_eq!(
            feed_osc(b"104;300;4\x07"),
            reset_palette(PaletteReset::Indices(vec![4]))
        );
    }

    #[test]
    fn osc104_no_valid_index_emits_nothing() {
        assert_eq!(feed_osc(b"104;300\x07"), []);
        assert_eq!(feed_osc(b"104;notanumber\x07"), []);
        assert_eq!(feed_osc(b"104;x;;y\x07"), []);
    }

    // ------------------------------------------------------------------
    // OSC 10-19
    // ------------------------------------------------------------------

    #[test]
    fn osc10_single_query() {
        assert_eq!(feed_osc(b"10;?\x07"), dynamic(&[(Foreground, Query)]));
    }

    #[test]
    fn osc11_single_query() {
        assert_eq!(feed_osc(b"11;?\x07"), dynamic(&[(Background, Query)]));
    }

    #[test]
    fn osc12_single_query() {
        assert_eq!(feed_osc(b"12;?\x07"), dynamic(&[(Cursor, Query)]));
    }

    #[test]
    fn osc10_two_values_set_foreground_then_background() {
        assert_eq!(
            feed_osc(b"10;red;blue\x07"),
            dynamic(&[
                (Foreground, Set((255, 0, 0))),
                (Background, Set((0, 0, 255)))
            ])
        );
    }

    #[test]
    fn osc10_two_queries() {
        assert_eq!(
            feed_osc(b"10;?;?\x07"),
            dynamic(&[(Foreground, Query), (Background, Query)])
        );
    }

    #[test]
    fn osc11_colour_name_with_spaces() {
        assert_eq!(
            feed_osc(b"11;dark slate gray\x07"),
            dynamic(&[(Background, Set((47, 79, 79)))])
        );
    }

    #[test]
    fn osc13_and_osc14_values_are_ignored() {
        assert_eq!(feed_osc(b"13;red;blue\x07"), []);
    }

    #[test]
    fn osc15_ignored_codes_are_consumed_before_a_mapped_slot() {
        // 15 and 16 consume `x` and `y`; 17 takes the colour.
        assert_eq!(
            feed_osc(b"15;x;y;#ff0000\x07"),
            dynamic(&[(SelectionBackground, Set((255, 0, 0)))])
        );
    }

    #[test]
    fn osc16_chain_skips_invalid_value_and_ignores_slot_18() {
        // 16 ignored, 17 gets the invalid `b` (skipped), 18 ignored.
        assert_eq!(feed_osc(b"16;a;b;red\x07"), []);
        // 16 ignored, 17 set, 18 ignored, 19 set.
        assert_eq!(
            feed_osc(b"16;a;#00ff00;c;#0000ff\x07"),
            dynamic(&[
                (SelectionBackground, Set((0, 255, 0))),
                (SelectionForeground, Set((0, 0, 255)))
            ])
        );
    }

    #[test]
    fn osc17_and_osc19_map_to_selection_slots() {
        assert_eq!(
            feed_osc(b"17;#ff0000\x07"),
            dynamic(&[(SelectionBackground, Set((255, 0, 0)))])
        );
        assert_eq!(
            feed_osc(b"19;?\x07"),
            dynamic(&[(SelectionForeground, Query)])
        );
    }

    #[test]
    fn osc18_alone_is_ignored() {
        assert_eq!(feed_osc(b"18;red\x07"), []);
    }

    #[test]
    fn osc_values_past_code_19_are_ignored() {
        // 19 takes `red`; the third value would be code 20.
        assert_eq!(
            feed_osc(b"19;red;blue\x07"),
            dynamic(&[(SelectionForeground, Set((255, 0, 0)))])
        );
    }

    #[test]
    fn osc10_empty_value_skips_only_that_code() {
        assert_eq!(
            feed_osc(b"10;;blue\x07"),
            dynamic(&[(Background, Set((0, 0, 255)))])
        );
        assert_eq!(feed_osc(b"10;\x07"), []);
    }

    #[test]
    fn osc10_invalid_value_skips_only_that_code() {
        assert_eq!(
            feed_osc(b"10;notacolor;blue\x07"),
            dynamic(&[(Background, Set((0, 0, 255)))])
        );
    }

    #[test]
    fn osc10_non_utf8_value_skips_only_that_code() {
        assert_eq!(
            feed_osc(b"10;\xff;blue\x07"),
            dynamic(&[(Background, Set((0, 0, 255)))])
        );
    }

    #[test]
    fn osc10_without_value_emits_nothing() {
        assert_eq!(feed_osc(b"10\x07"), []);
    }

    // ------------------------------------------------------------------
    // OSC 110-119
    // ------------------------------------------------------------------

    #[test]
    fn osc110_to_osc112_reset_their_slots() {
        assert_eq!(feed_osc(b"110\x07"), reset_dynamic(Foreground));
        assert_eq!(feed_osc(b"111\x07"), reset_dynamic(Background));
        assert_eq!(feed_osc(b"112\x07"), reset_dynamic(Cursor));
    }

    #[test]
    fn osc117_and_osc119_reset_the_selection_slots() {
        assert_eq!(feed_osc(b"117\x07"), reset_dynamic(SelectionBackground));
        assert_eq!(feed_osc(b"119\x07"), reset_dynamic(SelectionForeground));
    }

    #[test]
    fn osc113_to_osc116_and_osc118_are_ignored() {
        for code in [113, 114, 115, 116, 118] {
            assert_eq!(feed_osc(format!("{code}\x07").as_bytes()), [], "OSC {code}");
        }
    }

    #[test]
    fn reset_dynamic_ignores_parameters() {
        assert_eq!(feed_osc(b"110;ignored;also\x07"), reset_dynamic(Foreground));
        assert_eq!(feed_osc(b"113;ignored\x07"), []);
    }

    // ------------------------------------------------------------------
    // Neighbours stay unknown
    // ------------------------------------------------------------------

    #[test]
    fn osc5_and_osc105_remain_unknown() {
        assert_eq!(feed_osc(b"5;0;red\x07"), []);
        assert_eq!(feed_osc(b"105;0\x07"), []);
    }
}
