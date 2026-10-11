// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! OSC colour control for [`TerminalHandler`].
//!
//! This module applies the parsed OSC 4 / 104 (palette) and OSC 10-19 /
//! 110-119 (dynamic colour) requests to the handler's colour state and answers
//! the queries:
//!
//! - [`TerminalHandler::handle_osc_colors`] — entry point for the
//!   [`AnsiOscType::PaletteColors`], [`AnsiOscType::ResetPaletteColors`],
//!   [`AnsiOscType::DynamicColors`] and [`AnsiOscType::ResetDynamicColor`]
//!   variants.
//!
//! Every colour reply carries 16 bits per channel (`rgb:RRRR/GGGG/BBBB`, each
//! 8-bit value scaled by 257), as xterm, kitty and Ghostty do.

use freminal_common::buffer_states::{
    osc::AnsiOscType,
    osc_color::{ColorRequest, PaletteReset},
};

use super::TerminalHandler;

/// Format an 8-bit colour as the 16-bit-per-channel `rgb:RRRR/GGGG/BBBB` spec
/// used in OSC colour replies. Each channel is the 8-bit value times 257, so
/// `0xff` becomes `0xffff`.
fn rgb_spec_16bit((r, g, b): (u8, u8, u8)) -> String {
    format!(
        "rgb:{:04x}/{:04x}/{:04x}",
        u16::from(r) * 257,
        u16::from(g) * 257,
        u16::from(b) * 257,
    )
}

impl TerminalHandler {
    /// Apply an OSC colour-control request and answer its queries.
    ///
    /// Operations are applied in wire order, and each query is answered at the
    /// point it is reached, so `4;1;red;1;?` reports the colour it just set.
    ///
    /// - `PaletteColors`: `Set` overrides the palette entry; `Query` replies
    ///   `4;{index};rgb:RRRR/GGGG/BBBB`.
    /// - `ResetPaletteColors`: clears every override, or the listed entries.
    /// - `DynamicColors`: `Set` overrides the slot; `Query` replies
    ///   `{code};rgb:RRRR/GGGG/BBBB` from the effective colour (override or
    ///   theme default), where `code` is the slot's OSC number.
    /// - `ResetDynamicColor`: clears the slot's override.
    ///
    /// Any other variant is ignored.
    pub(super) fn handle_osc_colors(&mut self, osc: &AnsiOscType) {
        match osc {
            AnsiOscType::PaletteColors(ops) => {
                for op in ops {
                    match op.request {
                        ColorRequest::Set(rgb) => self.color_state.set_palette(op.index, rgb),
                        ColorRequest::Query => {
                            let rgb = self.color_state.palette_rgb(op.index);
                            self.write_osc_response(&format!(
                                "4;{};{}",
                                op.index,
                                rgb_spec_16bit(rgb)
                            ));
                        }
                    }
                }
            }
            AnsiOscType::ResetPaletteColors(PaletteReset::All) => {
                self.color_state.reset_palette_all();
            }
            AnsiOscType::ResetPaletteColors(PaletteReset::Indices(indices)) => {
                for index in indices {
                    self.color_state.reset_palette(*index);
                }
            }
            AnsiOscType::DynamicColors(ops) => {
                for op in ops {
                    match op.request {
                        ColorRequest::Set(rgb) => self.color_state.set_dynamic(op.slot, rgb),
                        ColorRequest::Query => {
                            let rgb = self.color_state.effective_dynamic(op.slot);
                            self.write_osc_response(&format!(
                                "{};{}",
                                op.slot.osc_code(),
                                rgb_spec_16bit(rgb)
                            ));
                        }
                    }
                }
            }
            AnsiOscType::ResetDynamicColor(slot) => {
                self.color_state.reset_dynamic(*slot);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use freminal_common::{
        buffer_states::modes::s8c1t::S8c1t, colors::DynamicColorSlot, pty_write::PtyWrite,
        themes::CATPPUCCIN_MOCHA,
    };

    use super::super::TerminalHandler;
    use super::rgb_spec_16bit;
    use crate::ansi::FreminalAnsiParser;

    type Rx = crossbeam_channel::Receiver<PtyWrite>;

    fn handler_with_rx() -> (TerminalHandler, Rx) {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);
        (handler, rx)
    }

    /// Feed `bytes` through the full ANSI parser and into the handler.
    fn feed(handler: &mut TerminalHandler, bytes: &[u8]) {
        let outputs = FreminalAnsiParser::new().push(bytes);
        handler.process_outputs(&outputs);
    }

    /// Every PTY write queued so far, in order.
    fn replies(rx: &Rx) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Ok(PtyWrite::Write(bytes)) = rx.try_recv() {
            out.push(bytes);
        }
        out
    }

    /// The exact 7-bit reply for `body`: `ESC ] body ESC \`.
    fn reply(body: &str) -> Vec<u8> {
        let mut v = b"\x1b]".to_vec();
        v.extend_from_slice(body.as_bytes());
        v.extend_from_slice(b"\x1b\\");
        v
    }

    fn fg(handler: &TerminalHandler) -> (u8, u8, u8) {
        handler
            .color_state
            .effective_dynamic(DynamicColorSlot::Foreground)
    }

    fn bg(handler: &TerminalHandler) -> (u8, u8, u8) {
        handler
            .color_state
            .effective_dynamic(DynamicColorSlot::Background)
    }

    // ------------------------------------------------------------------
    // 16-bit reply formatting
    // ------------------------------------------------------------------

    #[test]
    fn rgb_spec_16bit_scales_each_channel_by_257() {
        assert_eq!(rgb_spec_16bit((0, 0, 0)), "rgb:0000/0000/0000");
        assert_eq!(rgb_spec_16bit((255, 255, 255)), "rgb:ffff/ffff/ffff");
        assert_eq!(rgb_spec_16bit((0x12, 0x34, 0x56)), "rgb:1212/3434/5656");
        assert_eq!(rgb_spec_16bit((1, 0x80, 0xfe)), "rgb:0101/8080/fefe");
    }

    // ------------------------------------------------------------------
    // OSC 10 / 11 / 12 / 17 / 19 queries
    // ------------------------------------------------------------------

    #[test]
    fn osc_fg_query_returns_theme_default_in_16_bit() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]10;?\x1b\\");
        let expected = format!("10;{}", rgb_spec_16bit(CATPPUCCIN_MOCHA.foreground));
        assert_eq!(replies(&rx), vec![reply(&expected)]);
    }

    #[test]
    fn osc_bg_query_returns_theme_default_in_16_bit() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]11;?\x1b\\");
        let expected = format!("11;{}", rgb_spec_16bit(CATPPUCCIN_MOCHA.background));
        assert_eq!(replies(&rx), vec![reply(&expected)]);
    }

    #[test]
    fn osc_cursor_query_returns_theme_default_in_16_bit() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]12;?\x1b\\");
        let expected = format!("12;{}", rgb_spec_16bit(CATPPUCCIN_MOCHA.cursor));
        assert_eq!(replies(&rx), vec![reply(&expected)]);
    }

    #[test]
    fn osc17_and_osc19_queries_return_theme_selection_colours() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]17;?\x1b\\\x1b]19;?\x1b\\");
        assert_eq!(
            replies(&rx),
            vec![
                reply(&format!(
                    "17;{}",
                    rgb_spec_16bit(CATPPUCCIN_MOCHA.selection_bg)
                )),
                reply(&format!(
                    "19;{}",
                    rgb_spec_16bit(CATPPUCCIN_MOCHA.selection_fg)
                )),
            ]
        );
    }

    #[test]
    fn osc19_query_reply_is_theme_selection_fg_times_257() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]19;?\x1b\\");
        let (r, g, b) = CATPPUCCIN_MOCHA.selection_fg;
        let expected = format!(
            "19;rgb:{:04x}/{:04x}/{:04x}",
            u16::from(r) * 257,
            u16::from(g) * 257,
            u16::from(b) * 257
        );
        assert_eq!(replies(&rx), vec![reply(&expected)]);
    }

    #[test]
    fn osc10_two_queries_reply_in_order() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]10;?;?\x1b\\");
        assert_eq!(
            replies(&rx),
            vec![
                reply(&format!(
                    "10;{}",
                    rgb_spec_16bit(CATPPUCCIN_MOCHA.foreground)
                )),
                reply(&format!(
                    "11;{}",
                    rgb_spec_16bit(CATPPUCCIN_MOCHA.background)
                )),
            ]
        );
    }

    // ------------------------------------------------------------------
    // OSC 10 / 11 / 12 / 17 / 19 set
    // ------------------------------------------------------------------

    #[test]
    fn osc_fg_set_stores_override_and_query_returns_it() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]10;rgb:ff/00/00\x1b\\");
        feed(&mut handler, b"\x1b]10;?\x1b\\");
        assert_eq!(replies(&rx), vec![reply("10;rgb:ffff/0000/0000")]);
    }

    #[test]
    fn osc_bg_set_stores_override_and_query_returns_it() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]11;rgb:00/ff/00\x1b\\");
        feed(&mut handler, b"\x1b]11;?\x1b\\");
        assert_eq!(replies(&rx), vec![reply("11;rgb:0000/ffff/0000")]);
    }

    #[test]
    fn osc_cursor_set_stores_override_and_query_returns_it() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]12;rgb:00/00/ff\x1b\\");
        feed(&mut handler, b"\x1b]12;?\x1b\\");
        assert_eq!(replies(&rx), vec![reply("12;rgb:0000/0000/ffff")]);
    }

    #[test]
    fn osc12_query_reply_is_16_bit_not_8_bit() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]12;rgb:12/34/56\x1b\\");
        feed(&mut handler, b"\x1b]12;?\x1b\\");
        assert_eq!(replies(&rx), vec![reply("12;rgb:1212/3434/5656")]);
    }

    #[test]
    fn osc10_two_values_set_foreground_and_background() {
        let (mut handler, _rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]10;red;blue\x1b\\");
        assert_eq!(fg(&handler), (255, 0, 0));
        assert_eq!(bg(&handler), (0, 0, 255));
    }

    #[test]
    fn osc11_colour_name_with_spaces_sets_background() {
        let (mut handler, _rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]11;dark slate gray\x1b\\");
        assert_eq!(bg(&handler), (47, 79, 79));
    }

    #[test]
    fn osc13_values_set_nothing() {
        let (mut handler, rx) = handler_with_rx();
        let (fg0, bg0) = (fg(&handler), bg(&handler));
        feed(&mut handler, b"\x1b]13;red;blue\x1b\\");
        assert_eq!((fg(&handler), bg(&handler)), (fg0, bg0));
        assert_eq!(replies(&rx), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn osc15_ignored_codes_precede_a_set_selection_background() {
        let (mut handler, rx) = handler_with_rx();
        let (fg0, bg0) = (fg(&handler), bg(&handler));
        // 15 and 16 consume `x` and `y`; 17 takes the colour.
        feed(&mut handler, b"\x1b]15;x;y;#ff0000\x1b\\");
        assert_eq!(
            handler
                .color_state
                .effective_dynamic(DynamicColorSlot::SelectionBackground),
            (255, 0, 0)
        );
        assert_eq!((fg(&handler), bg(&handler)), (fg0, bg0));

        feed(&mut handler, b"\x1b]17;?\x1b\\");
        assert_eq!(replies(&rx), vec![reply("17;rgb:ffff/0000/0000")]);
    }

    #[test]
    fn osc17_set_then_query_and_osc19_set() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]17;#ff0000\x1b\\");
        feed(&mut handler, b"\x1b]17;?\x1b\\");
        assert_eq!(replies(&rx), vec![reply("17;rgb:ffff/0000/0000")]);

        feed(&mut handler, b"\x1b]19;#00ff00\x1b\\");
        feed(&mut handler, b"\x1b]19;?\x1b\\");
        assert_eq!(replies(&rx), vec![reply("19;rgb:0000/ffff/0000")]);
    }

    #[test]
    fn osc_fg_set_invalid_spec_is_ignored() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]10;rgb:ff/00/00\x1b\\");
        feed(&mut handler, b"\x1b]10;not-a-color\x1b\\");
        feed(&mut handler, b"\x1b]10;?\x1b\\");
        assert_eq!(replies(&rx), vec![reply("10;rgb:ffff/0000/0000")]);
    }

    // ------------------------------------------------------------------
    // OSC 110-119
    // ------------------------------------------------------------------

    #[test]
    fn osc_110_resets_fg_override() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]10;rgb:ff/00/00\x1b\\");
        feed(&mut handler, b"\x1b]110\x1b\\");
        feed(&mut handler, b"\x1b]10;?\x1b\\");
        let expected = format!("10;{}", rgb_spec_16bit(CATPPUCCIN_MOCHA.foreground));
        assert_eq!(replies(&rx), vec![reply(&expected)]);
    }

    #[test]
    fn osc_111_resets_bg_override() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]11;rgb:00/ff/00\x1b\\");
        feed(&mut handler, b"\x1b]111\x1b\\");
        feed(&mut handler, b"\x1b]11;?\x1b\\");
        let expected = format!("11;{}", rgb_spec_16bit(CATPPUCCIN_MOCHA.background));
        assert_eq!(replies(&rx), vec![reply(&expected)]);
    }

    #[test]
    fn osc_112_resets_cursor_override() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]12;rgb:00/00/ff\x1b\\");
        feed(&mut handler, b"\x1b]112\x1b\\");
        feed(&mut handler, b"\x1b]12;?\x1b\\");
        let expected = format!("12;{}", rgb_spec_16bit(CATPPUCCIN_MOCHA.cursor));
        assert_eq!(replies(&rx), vec![reply(&expected)]);
    }

    #[test]
    fn osc_117_and_119_reset_the_selection_overrides() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]17;#ff0000\x1b\\\x1b]19;#00ff00\x1b\\");
        feed(&mut handler, b"\x1b]117\x1b\\");
        feed(&mut handler, b"\x1b]17;?\x1b\\\x1b]19;?\x1b\\");
        assert_eq!(
            replies(&rx),
            vec![
                reply(&format!(
                    "17;{}",
                    rgb_spec_16bit(CATPPUCCIN_MOCHA.selection_bg)
                )),
                reply("19;rgb:0000/ffff/0000"),
            ]
        );

        feed(&mut handler, b"\x1b]119\x1b\\");
        feed(&mut handler, b"\x1b]19;?\x1b\\");
        assert_eq!(
            replies(&rx),
            vec![reply(&format!(
                "19;{}",
                rgb_spec_16bit(CATPPUCCIN_MOCHA.selection_fg)
            ))]
        );
    }

    #[test]
    fn osc_113_to_116_and_118_change_nothing() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]10;red\x1b\\\x1b]11;blue\x1b\\");
        for code in [113, 114, 115, 116, 118] {
            feed(&mut handler, format!("\x1b]{code}\x1b\\").as_bytes());
        }
        assert_eq!(fg(&handler), (255, 0, 0));
        assert_eq!(bg(&handler), (0, 0, 255));
        assert_eq!(replies(&rx), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn full_reset_clears_every_dynamic_override() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]10;rgb:ff/00/00\x1b\\");
        feed(&mut handler, b"\x1b]11;rgb:00/ff/00\x1b\\");
        feed(&mut handler, b"\x1b]12;rgb:00/00/ff\x1b\\");

        handler.full_reset();

        feed(
            &mut handler,
            b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b]12;?\x1b\\",
        );
        assert_eq!(
            replies(&rx),
            vec![
                reply(&format!(
                    "10;{}",
                    rgb_spec_16bit(CATPPUCCIN_MOCHA.foreground)
                )),
                reply(&format!(
                    "11;{}",
                    rgb_spec_16bit(CATPPUCCIN_MOCHA.background)
                )),
                reply(&format!("12;{}", rgb_spec_16bit(CATPPUCCIN_MOCHA.cursor))),
            ]
        );
    }

    #[test]
    fn cursor_color_override_accessor_reflects_set_value() {
        let (mut handler, _rx) = handler_with_rx();
        assert_eq!(handler.cursor_color_override(), None);

        feed(&mut handler, b"\x1b]12;rgb:12/34/56\x1b\\");
        assert_eq!(handler.cursor_color_override(), Some((0x12, 0x34, 0x56)));

        feed(&mut handler, b"\x1b]112\x1b\\");
        assert_eq!(handler.cursor_color_override(), None);
    }

    // ------------------------------------------------------------------
    // Reply framing
    // ------------------------------------------------------------------

    #[test]
    fn query_reply_is_exact_seven_bit_bytes_with_st() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]11;rgb:12/34/56\x07");
        feed(&mut handler, b"\x1b]11;?\x07");
        assert_eq!(
            replies(&rx),
            vec![b"\x1b]11;rgb:1212/3434/5656\x1b\\".to_vec()]
        );
    }

    #[test]
    fn query_reply_uses_eight_bit_c1_framing_under_s8c1t() {
        let (mut handler, rx) = handler_with_rx();
        handler.set_s8c1t_mode(S8c1t::EightBit);
        feed(&mut handler, b"\x1b]11;rgb:12/34/56\x07");
        feed(&mut handler, b"\x1b]11;?\x07");
        assert_eq!(
            replies(&rx),
            vec![b"\x9d11;rgb:1212/3434/5656\x9c".to_vec()]
        );
    }

    // ------------------------------------------------------------------
    // OSC 4
    // ------------------------------------------------------------------

    #[test]
    fn osc4_set_and_query_in_one_sequence() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]4;1;red;2;?\x1b\\");
        assert_eq!(handler.color_state.palette_rgb(1), (255, 0, 0));
        let (r, g, b) = CATPPUCCIN_MOCHA.ansi[2];
        let expected = format!(
            "4;2;rgb:{:04x}/{:04x}/{:04x}",
            u16::from(r) * 257,
            u16::from(g) * 257,
            u16::from(b) * 257
        );
        assert_eq!(replies(&rx), vec![reply(&expected)]);
    }

    #[test]
    fn osc4_applies_pairs_in_order_so_a_query_sees_the_earlier_set() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]4;1;red;1;?\x1b\\");
        assert_eq!(replies(&rx), vec![reply("4;1;rgb:ffff/0000/0000")]);
    }

    #[test]
    fn osc4_multiple_queries_reply_in_order() {
        let (mut handler, rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]4;3;#0a0b0c;5;#ffffff;3;?;5;?\x1b\\");
        assert_eq!(
            replies(&rx),
            vec![
                reply("4;3;rgb:0a0a/0b0b/0c0c"),
                reply("4;5;rgb:ffff/ffff/ffff"),
            ]
        );
    }

    #[test]
    fn osc4_odd_field_count_drops_the_whole_sequence() {
        let (mut handler, _rx) = handler_with_rx();
        let before = handler.color_state.palette_rgb(1);
        feed(&mut handler, b"\x1b]4;1;red;2\x1b\\");
        assert_eq!(handler.color_state.palette_rgb(1), before);
    }

    #[test]
    fn osc4_out_of_range_index_skips_only_that_pair() {
        let (mut handler, _rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]4;300;red;2;blue\x1b\\");
        assert_eq!(handler.color_state.palette_rgb(2), (0, 0, 255));
    }

    #[test]
    fn osc4_invalid_spec_skips_only_that_pair() {
        let (mut handler, _rx) = handler_with_rx();
        let before = handler.color_state.palette_rgb(1);
        feed(&mut handler, b"\x1b]4;1;notacolor;2;blue\x1b\\");
        assert_eq!(handler.color_state.palette_rgb(1), before);
        assert_eq!(handler.color_state.palette_rgb(2), (0, 0, 255));
    }

    #[test]
    fn osc4_accepts_colour_names_with_spaces_and_wide_gamut_specs() {
        let (mut handler, _rx) = handler_with_rx();
        feed(&mut handler, b"\x1b]4;1;dark slate gray\x1b\\");
        assert_eq!(handler.color_state.palette_rgb(1), (47, 79, 79));
        feed(&mut handler, b"\x1b]4;1;oklch(0.5 0.1 180)\x1b\\");
        assert_eq!(handler.color_state.palette_rgb(1), (0, 117, 101));
    }

    // ------------------------------------------------------------------
    // OSC 104
    // ------------------------------------------------------------------

    fn set_three_entries(handler: &mut TerminalHandler) -> [(u8, u8, u8); 3] {
        let defaults = [1u8, 2, 3].map(|i| handler.color_state.palette_rgb(i));
        feed(handler, b"\x1b]4;1;#010101;2;#020202;3;#030303\x1b\\");
        defaults
    }

    #[test]
    fn osc104_with_indices_resets_each() {
        let (mut handler, _rx) = handler_with_rx();
        let defaults = set_three_entries(&mut handler);
        feed(&mut handler, b"\x1b]104;1;2\x1b\\");
        assert_eq!(handler.color_state.palette_rgb(1), defaults[0]);
        assert_eq!(handler.color_state.palette_rgb(2), defaults[1]);
        assert_eq!(handler.color_state.palette_rgb(3), (3, 3, 3));
    }

    #[test]
    fn osc104_without_index_resets_all() {
        let (mut handler, _rx) = handler_with_rx();
        let defaults = set_three_entries(&mut handler);
        feed(&mut handler, b"\x1b]104\x1b\\");
        for (i, d) in [1u8, 2, 3].into_iter().zip(defaults) {
            assert_eq!(handler.color_state.palette_rgb(i), d);
        }
    }

    #[test]
    fn osc104_with_trailing_semicolon_resets_all() {
        let (mut handler, _rx) = handler_with_rx();
        let defaults = set_three_entries(&mut handler);
        feed(&mut handler, b"\x1b]104;\x1b\\");
        for (i, d) in [1u8, 2, 3].into_iter().zip(defaults) {
            assert_eq!(handler.color_state.palette_rgb(i), d);
        }
    }

    #[test]
    fn osc104_skips_invalid_fields_and_resets_the_rest() {
        let (mut handler, _rx) = handler_with_rx();
        let defaults = set_three_entries(&mut handler);
        feed(&mut handler, b"\x1b]104;x;3\x1b\\");
        assert_eq!(handler.color_state.palette_rgb(3), defaults[2]);
        assert_eq!(handler.color_state.palette_rgb(1), (1, 1, 1));
        assert_eq!(handler.color_state.palette_rgb(2), (2, 2, 2));
    }

    #[test]
    fn osc104_with_no_valid_index_resets_nothing() {
        let (mut handler, _rx) = handler_with_rx();
        let _ = set_three_entries(&mut handler);
        feed(&mut handler, b"\x1b]104;x;300\x1b\\");
        assert_eq!(handler.color_state.palette_rgb(1), (1, 1, 1));
        assert_eq!(handler.color_state.palette_rgb(3), (3, 3, 3));
    }
}
