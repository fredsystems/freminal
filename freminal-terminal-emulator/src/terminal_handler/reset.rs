// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The reset table: which [`TerminalHandler`] state RIS and DECSTR reset.
//!
//! [`TerminalHandler::reset`] destructures the handler with no `..`, so a new
//! field cannot be added without being classified here -- either reset per
//! kind, or bound to `_` with the reason it survives.  The authoritative row
//! list, with the xterm / kitty / Ghostty / `WezTerm` sources, is the "131 Reset
//! table" in `Documents/PLAN_VERSION_130.md`.
//!
//! `TerminalState` owns a further slice of reset state (DECCKM, DECNKM, the
//! parser) and resets it itself.

use std::collections::HashMap;

use freminal_buffer::buffer::Buffer;
use freminal_common::buffer_states::{
    format_tag::FormatTag,
    ftcs::FtcsState,
    line_draw::DecSpecialGraphics,
    modes::{
        allow_column_mode_switch::AllowColumnModeSwitch,
        application_escape_key::ApplicationEscapeKey, decanm::Decanm, decawm::Decawm,
        declrmm::Declrmm, decnrcm::Decnrcm, decom::Decom, decsdm::Decsdm, dectcem::Dectcem,
        in_band_resize_mode::InBandResizeMode, irm::Irm,
        private_color_registers::PrivateColorRegisters, reverse_wrap_around::ReverseWrapAround,
        s8c1t::S8c1t, xt_rev_wrap2::XtRevWrap2,
    },
    pointer_shape::PointerShape,
    progress::ProgressReport,
    row_number::RowNumber,
    unicode_placeholder::VirtualPlacement,
};

use super::{KittyTransfer, RealPlacement, ScreenScoped, TerminalHandler};

/// Which reset [`TerminalHandler::reset`] performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetKind {
    /// RIS (`ESC c`): back to the initial state, screen content included.
    Hard,
    /// DECSTR (`CSI ! p`): the Table 5-9 subset of modes and attributes; the
    /// screen content and the live cursor position are left alone.
    Soft,
}

/// Drop every Kitty placement of both screens and re-base the prune cursors.
fn clear_kitty_placements(
    virtual_placements: &mut ScreenScoped<HashMap<(u64, u32), VirtualPlacement>>,
    real_placements: &mut ScreenScoped<HashMap<(u64, u32), RealPlacement>>,
    placement_prune_base: &mut ScreenScoped<RowNumber>,
) {
    for placements in virtual_placements.both_mut() {
        placements.clear();
    }
    for placements in real_placements.both_mut() {
        placements.clear();
    }
    *placement_prune_base = ScreenScoped::new(RowNumber::ZERO, RowNumber::ALTERNATE_BASE);
}

/// The buffer half of DECSTR.  Resets origin mode, the scroll region and the
/// left/right margins, and records "home, default charset" as the DECSC state
/// of the **active** screen only, all without moving the live cursor.
///
/// Every step that touches DECOM or a margin homes the cursor as a side
/// effect, so the live position is captured first and restored last.
fn soft_reset_buffer(
    buffer: &mut Buffer,
    saved_character_replace: &mut ScreenScoped<Option<DecSpecialGraphics>>,
) {
    let live_pos = buffer.cursor_screen_pos();

    // DECOM -> Absolute (off).  `Buffer::set_decom` homes the cursor; that
    // homed position is captured into the *saved* cursor (DECSC state) via
    // `save_cursor()` -- Table 5-9 wants DECSC at home position after DECSTR,
    // and the saved cursor's attributes are already default.
    buffer.set_decom(Decom::NormalCursor);
    buffer.save_cursor();
    *saved_character_replace.get_mut(buffer.kind()) = Some(DecSpecialGraphics::default());

    // DECSTBM -> top = 1, bottom = page length.
    buffer.reset_scroll_region_to_full();

    // Deliberate deviation from Table 5-9: disabling DECLRMM also resets the
    // left/right margins to full width, for consistency with the DECSTBM reset
    // just above.
    buffer.set_declrmm(Declrmm::Disabled);

    buffer.set_cursor_pos(Some(live_pos.x), Some(live_pos.y));

    // SGR -> normal rendition, for subsequently-written characters.
    buffer.set_format(FormatTag::default());
}

impl TerminalHandler {
    /// Reset the handler per the reset table.  See the module documentation.
    ///
    /// `Hard` does not restore the DECCOLM width; [`Self::full_reset`] does,
    /// because that needs the buffer's pre-reset width.
    pub(crate) fn reset(&mut self, kind: ResetKind) {
        // No `..`: adding a field without classifying it is a compile error.
        let Self {
            buffer,
            current_format,
            show_cursor,
            cursor_visual_style,
            configured_cursor_visual_style,
            character_replace,
            saved_character_replace,
            write_tx: _,               // transport: survives both
            window_commands: _,        // already-emitted events in transit: survives both
            pending_command_events: _, // already-emitted events in transit: survives both
            last_graphic_char,
            current_working_directory: _, // describes the process, not the screen: survives both
            shell_histfile: _,            // describes the process, not the screen: survives both
            ftcs_state,
            last_exit_code,
            palette,
            allow_column_mode_switch,
            allow_alt_screen: _, // xterm does not reset it: survives both
            pre_deccolm_width,
            theme: _, // configuration: survives both
            fg_color_override,
            bg_color_override,
            cursor_color_override,
            pointer_shape,
            progress,
            progress_updated_at,
            multipart_state,
            kitty_transfer,
            virtual_placements,
            real_placements,
            placement_prune_base,
            prev_placeholder,
            cell_pixel_width: _, // font metrics, not terminal state: survives both
            cell_pixel_height: _, // font metrics, not terminal state: survives both
            tmux_passthrough_queue: _, // payloads awaiting the parser: survives both
            modify_other_keys_level,
            application_escape_key,
            in_band_resize_enabled,
            sixel_display_mode,
            private_color_registers,
            nrc_mode,
            reverse_wrap,
            xt_rev_wrap2,
            vt52_mode,
            insert_mode,
            sixel_shared_palette,
            s8c1t_mode,
            kitty_keyboard_stack,
            pending_notifications,
        } = self;

        // ── Reset by both kinds ──────────────────────────────────────────
        *show_cursor = Dectcem::Show;
        *cursor_visual_style = configured_cursor_visual_style.clone();
        *character_replace = DecSpecialGraphics::default();
        *current_format = FormatTag::default();
        palette.reset_all();
        *pointer_shape = PointerShape::Default;
        *progress = ProgressReport::default();
        *progress_updated_at = None;
        *modify_other_keys_level = 0;
        *insert_mode = Irm::Replace;
        *nrc_mode = Decnrcm::NrcDisabled;
        *reverse_wrap = ReverseWrapAround::default();
        *xt_rev_wrap2 = XtRevWrap2::Disabled;
        for stack in kitty_keyboard_stack.both_mut() {
            stack.clear();
        }

        // ── Reset per kind ───────────────────────────────────────────────
        match kind {
            ResetKind::Hard => {
                buffer.full_reset();
                for slot in saved_character_replace.both_mut() {
                    *slot = None;
                }
                *last_graphic_char = None;
                *ftcs_state = FtcsState::default();
                *last_exit_code = None;
                *allow_column_mode_switch = AllowColumnModeSwitch::AllowColumnModeSwitch;
                *pre_deccolm_width = None;
                *fg_color_override = None;
                *bg_color_override = None;
                *cursor_color_override = None;
                *multipart_state = None;
                *kitty_transfer = KittyTransfer::Idle;
                clear_kitty_placements(virtual_placements, real_placements, placement_prune_base);
                *prev_placeholder = None;
                *application_escape_key = ApplicationEscapeKey::Reset;
                *in_band_resize_enabled = InBandResizeMode::Reset;
                *sixel_display_mode = Decsdm::ScrollingMode;
                *private_color_registers = PrivateColorRegisters::Private;
                *sixel_shared_palette = None;
                *vt52_mode = Decanm::Ansi;
                *s8c1t_mode = S8c1t::SevenBit;
                pending_notifications.clear();
            }
            ResetKind::Soft => {
                // DECAWM -> on: xterm's own resource default, and what kitty,
                // Ghostty and WezTerm do, against the literal VT510 Table 5-9.
                buffer.set_wrap(Decawm::AutoWrap);
                soft_reset_buffer(buffer, saved_character_replace);
                // Soft keeps (today's behaviour, no table row): the REP
                // character, FTCS state, the colour overrides, ?40, the
                // sixel and S8C1T/DECANM state, ?7727, ?2048, OSC 99
                // accumulators, in-flight image transfers and placements.
            }
        }
    }
}
