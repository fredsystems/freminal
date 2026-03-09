// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

// WIP for replaying terminal data

#![allow(dead_code)]
#![allow(unused_variables)]

use freminal_common::colors::TerminalColor;

use super::ansi_components::{mode::BracketedPaste, sgr::SelectGraphicRendition};
use super::state::buffer::TerminalBufferHolder;
use super::state::cursor::CursorState;
use super::state::fonts::{FontDecorations, FontWeight};
use super::state::term_char::TChar;
use super::{
    ansi::{FreminalAnsiParser, TerminalOutput},
    format_tracker::FormatTracker,
};
use crate::ansi_components::line_draw::DecSpecialGraphics;
use crate::ansi_components::mode::TerminalModes;
use crate::ansi_components::mode::{Decawm, Decckm};
use crate::ansi_components::mode::{Dectem, Mode};
use crate::format_tracker::FormatTag;
use crate::interface::split_format_data_for_scrollback;
use crate::state::cursor::{CursorPos, StateColors};
use crate::state::data::TerminalSections;

pub const TERMINAL_WIDTH: usize = 112;
pub const TERMINAL_HEIGHT: usize = 38;

pub struct ReplayIo {
    parser: FreminalAnsiParser,
    terminal_buffer: TerminalBufferHolder,
    format_tracker: FormatTracker,
    cursor_state: CursorState,
    modes: TerminalModes,
    show_cursor: Dectem,
    replace_characters: DecSpecialGraphics,
}

impl Default for ReplayIo {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayIo {
    #[must_use]
    pub fn new() -> Self {
        Self {
            parser: FreminalAnsiParser::new(),
            terminal_buffer: TerminalBufferHolder::new(TERMINAL_WIDTH, TERMINAL_HEIGHT),
            format_tracker: FormatTracker::new(),
            cursor_state: CursorState {
                pos: CursorPos::default(),
                font_weight: FontWeight::Normal,
                font_decorations: Vec::new(),
                colors: StateColors::default(),
                line_wrap_mode: Decawm::default(),
                url: None,
            },
            modes: TerminalModes {
                cursor_key: Decckm::default(),
                bracketed_paste: BracketedPaste::default(),
            },
            show_cursor: Dectem::Show,
            replace_characters: DecSpecialGraphics::DontReplace,
        }
    }

    #[must_use]
    pub const fn get_win_size(&self) -> (usize, usize) {
        self.terminal_buffer.get_win_size()
    }

    pub fn set_win_size(&mut self, width_chars: usize, height_chars: usize) {
        let response =
            self.terminal_buffer
                .set_win_size(width_chars, height_chars, &self.cursor_state.pos);
        self.cursor_state.pos = response.new_cursor_pos;
    }

    fn handle_data(&mut self, data: &[u8]) {
        let response = match self
            .terminal_buffer
            .insert_data(&self.cursor_state.pos, data)
        {
            Ok(response) => response,
            Err(e) => {
                error!("Failed to insert data: {e}");
                return;
            }
        };
        self.format_tracker
            .push_range_adjustment(response.insertion_range);
        self.format_tracker
            .push_range(&self.cursor_state, response.written_range);
        self.cursor_state.pos = response.new_cursor_pos;
    }

    fn set_cursor_pos(&mut self, x: Option<usize>, y: Option<usize>) {
        if let Some(x) = x {
            self.cursor_state.pos.x = x;
        }
        if let Some(y) = y {
            self.cursor_state.pos.y = y;
        }
    }

    fn set_cursor_pos_rel(&mut self, x: Option<i32>, y: Option<i32>) {
        if let Some(x) = x {
            let x: i64 = x.into();
            let current_x: i64 = self
                .cursor_state
                .pos
                .x
                .try_into()
                .expect("x position larger than i64 can handle");
            self.cursor_state.pos.x = usize::try_from((current_x + x).max(0)).unwrap_or(0);
        }
        if let Some(y) = y {
            let y: i64 = y.into();
            let current_y: i64 = self
                .cursor_state
                .pos
                .y
                .try_into()
                .expect("y position larger than i64 can handle");
            // ensure y is not negative, and throw an error if it is
            self.cursor_state.pos.y = usize::try_from((current_y + y).max(0)).unwrap_or(0);
        }
    }

    pub(crate) fn clear_forwards(&mut self) {
        match self.terminal_buffer.clear_forwards(&self.cursor_state.pos) {
            Ok(Some(buf_pos)) => {
                self.format_tracker
                    .push_range(&self.cursor_state, buf_pos..usize::MAX);
            }
            // FIXME: why on god's green earth are we having an option type here?
            Ok(None) => (),
            Err(e) => {
                error!("Failed to clear forwards: {e}");
            }
        }
    }

    pub(crate) fn clear_backwards(&mut self) {
        match self.terminal_buffer.clear_backwards(&self.cursor_state.pos) {
            Ok(Some(buf_pos)) => {
                self.format_tracker.push_range(&self.cursor_state, buf_pos);
            }
            Ok(None) => (),
            Err(e) => {
                error!("Failed to clear backwards: {e}");
            }
        }
    }

    fn clear_all(&mut self) {
        self.format_tracker
            .push_range(&self.cursor_state, 0..usize::MAX);
        self.terminal_buffer.clear_all();
    }

    pub(crate) fn clear_visible(&mut self) {
        let Some(range) = self.terminal_buffer.clear_visible() else {
            return;
        };

        if range.end > 0 {
            self.format_tracker.push_range(&self.cursor_state, range);
        }
    }

    fn clear_line_forwards(&mut self) {
        if let Some(range) = self
            .terminal_buffer
            .clear_line_forwards(&self.cursor_state.pos)
        {
            match self.format_tracker.delete_range(range) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to delete range: {e}");
                }
            }
        }
    }

    pub fn clear_line_backwards(&mut self) {
        if let Some(range) = self
            .terminal_buffer
            .clear_line_backwards(&self.cursor_state.pos)
        {
            match self.format_tracker.delete_range(range) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to delete range: {e}");
                }
            }
        }
    }

    pub fn clear_line(&mut self) {
        if let Some(range) = self.terminal_buffer.clear_line(&self.cursor_state.pos) {
            match self.format_tracker.delete_range(range) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to delete range: {e}");
                }
            }
        }
    }

    fn carriage_return(&mut self) {
        self.cursor_state.pos.x = 0;
    }

    fn new_line(&mut self) {
        self.cursor_state.pos.y += 1;
    }

    fn backspace(&mut self) {
        if self.cursor_state.pos.x >= 1 {
            self.cursor_state.pos.x -= 1;
        }
    }

    fn insert_lines(&mut self, num_lines: usize) {
        let response = self
            .terminal_buffer
            .insert_lines(&self.cursor_state.pos, num_lines);
        match self.format_tracker.delete_range(response.deleted_range) {
            Ok(()) => (),
            Err(e) => {
                error!("Failed to delete range: {e}");
            }
        }
        self.format_tracker
            .push_range_adjustment(response.inserted_range);
    }

    fn delete(&mut self, num_chars: usize) {
        let deleted_buf_range = self
            .terminal_buffer
            .delete_forwards(&self.cursor_state.pos, num_chars);
        if let Some(range) = deleted_buf_range {
            match self.format_tracker.delete_range(range) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to delete range: {e}");
                }
            }
        }
    }

    fn erase(&mut self, num_chars: usize) {
        let deleted_buf_range = self
            .terminal_buffer
            .erase_forwards(&self.cursor_state.pos, num_chars);
        if let Some(range) = deleted_buf_range {
            match self.format_tracker.delete_range(range) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to delete range: {e}");
                }
            }
        }
    }

    fn reset(&mut self) {
        self.cursor_state.colors = StateColors::default();
        self.cursor_state.font_weight = FontWeight::Normal;
        self.cursor_state.font_decorations.clear();
    }

    fn font_decordations_add_if_not_contains(&mut self, decoration: FontDecorations) {
        if !self.cursor_state.font_decorations.contains(&decoration) {
            self.cursor_state.font_decorations.push(decoration);
        }
    }

    fn font_decorations_remove_if_contains(&mut self, decoration: &FontDecorations) {
        self.cursor_state
            .font_decorations
            .retain(|d| *d != *decoration);
    }

    fn set_foreground(&mut self, color: TerminalColor) {
        self.cursor_state.colors.set_color(color);
    }

    fn set_background(&mut self, color: TerminalColor) {
        self.cursor_state.colors.set_background_color(color);
    }

    fn set_underline_color(&mut self, color: TerminalColor) {
        self.cursor_state.colors.set_underline_color(color);
    }

    fn sgr(&mut self, sgr: SelectGraphicRendition) {
        // if let Some(color) = TerminalColor::from_sgr(sgr) {
        //     self.cursor_state.color = color;
        //     return;
        // }

        match sgr {
            SelectGraphicRendition::Reset => self.reset(),
            SelectGraphicRendition::Bold => {
                self.cursor_state.font_weight = FontWeight::Bold;
            }
            SelectGraphicRendition::Italic => {
                self.font_decordations_add_if_not_contains(FontDecorations::Italic);
            }
            SelectGraphicRendition::NotItalic => {
                self.font_decorations_remove_if_contains(&FontDecorations::Italic);
            }
            SelectGraphicRendition::Faint => {
                self.font_decordations_add_if_not_contains(FontDecorations::Faint);
            }
            SelectGraphicRendition::Underline => {
                self.font_decordations_add_if_not_contains(FontDecorations::Underline);
            }
            SelectGraphicRendition::ResetBold => {
                self.cursor_state.font_weight = FontWeight::Normal;
            }
            SelectGraphicRendition::NormalIntensity => {
                self.font_decorations_remove_if_contains(&FontDecorations::Faint);
            }
            SelectGraphicRendition::NotUnderlined => {
                self.font_decorations_remove_if_contains(&FontDecorations::Underline);
            }
            SelectGraphicRendition::Strikethrough => {
                self.font_decordations_add_if_not_contains(FontDecorations::Strikethrough);
            }
            SelectGraphicRendition::NotStrikethrough => {
                self.font_decorations_remove_if_contains(&FontDecorations::Strikethrough);
            }
            SelectGraphicRendition::ReverseVideo => {
                self.cursor_state
                    .colors
                    .set_reverse_video(crate::state::cursor::ReverseVideo::On);
            }
            SelectGraphicRendition::ResetReverseVideo => {
                self.cursor_state
                    .colors
                    .set_reverse_video(crate::state::cursor::ReverseVideo::Off);
            }
            SelectGraphicRendition::Foreground(color) => self.set_foreground(color),
            SelectGraphicRendition::Background(color) => self.set_background(color),
            SelectGraphicRendition::UnderlineColor(color) => self.set_underline_color(color),
            SelectGraphicRendition::FastBlink
            | SelectGraphicRendition::SlowBlink
            | SelectGraphicRendition::NotBlinking
            | SelectGraphicRendition::Conceal
            | SelectGraphicRendition::PrimaryFont
            | SelectGraphicRendition::AlternativeFont1
            | SelectGraphicRendition::AlternativeFont2
            | SelectGraphicRendition::AlternativeFont3
            | SelectGraphicRendition::AlternativeFont4
            | SelectGraphicRendition::AlternativeFont5
            | SelectGraphicRendition::AlternativeFont6
            | SelectGraphicRendition::AlternativeFont7
            | SelectGraphicRendition::AlternativeFont8
            | SelectGraphicRendition::AlternativeFont9
            | SelectGraphicRendition::FontFranktur
            | SelectGraphicRendition::ProportionalSpacing
            | SelectGraphicRendition::DisableProportionalSpacing
            | SelectGraphicRendition::Framed
            | SelectGraphicRendition::Encircled
            | SelectGraphicRendition::Overlined
            | SelectGraphicRendition::NotOverlined
            | SelectGraphicRendition::NotFramedOrEncircled
            | SelectGraphicRendition::IdeogramUnderline
            | SelectGraphicRendition::IdeogramDoubleUnderline
            | SelectGraphicRendition::IdeogramOverline
            | SelectGraphicRendition::IdeogramDoubleOverline
            | SelectGraphicRendition::IdeogramStress
            | SelectGraphicRendition::IdeogramAttributes
            | SelectGraphicRendition::Superscript
            | SelectGraphicRendition::Subscript
            | SelectGraphicRendition::NeitherSuperscriptNorSubscript
            | SelectGraphicRendition::Revealed => {
                warn!("Unhandled sgr: {:?}", sgr);
            }
            SelectGraphicRendition::Unknown(_) => {
                warn!("Unknown sgr: {:?}", sgr);
            }
        }
    }

    fn set_mode(&mut self, mode: &Mode) {
        match mode {
            Mode::Decckm => {
                self.modes.cursor_key = Decckm::Application;
            }
            Mode::Decawm => {
                self.cursor_state.line_wrap_mode = Decawm::AutoWrap;
            }
            Mode::Dectem => {
                self.show_cursor = Dectem::Show;
            }
            Mode::BracketedPaste => {
                self.modes.bracketed_paste = BracketedPaste::Enabled;
            }
            Mode::XtExtscrn => {
                warn!("Unhandled set mode: {mode:?}");
            }
            Mode::Unknown(_) => {
                warn!("unhandled set mode: {mode:?}");
            }
        }
    }

    fn insert_spaces(&mut self, num_spaces: usize) {
        let response = self
            .terminal_buffer
            .insert_spaces(&self.cursor_state.pos, num_spaces);
        self.format_tracker
            .push_range_adjustment(response.insertion_range);
    }

    fn reset_mode(&mut self, mode: &Mode) {
        match mode {
            Mode::Decckm => {
                self.modes.cursor_key = Decckm::Ansi;
            }
            Mode::Decawm => {
                self.cursor_state.line_wrap_mode = Decawm::NoAutoWrap;
            }
            Mode::Dectem => {
                self.show_cursor = Dectem::Hide;
            }
            Mode::BracketedPaste => {
                self.modes.bracketed_paste = BracketedPaste::Disabled;
            }
            Mode::XtExtscrn => {
                warn!("Unhandled reset mode: {mode:?}");
            }
            Mode::Unknown(_) => {}
        }
    }

    pub fn handle_incoming_data(&mut self, incoming: &[u8]) {
        let parsed = self.parser.push(incoming);
        for segment in parsed {
            match segment {
                TerminalOutput::Data(data) => self.handle_data(&data),
                TerminalOutput::SetCursorPos { x, y } => self.set_cursor_pos(x, y),
                TerminalOutput::SetCursorPosRel { x, y } => self.set_cursor_pos_rel(x, y),
                TerminalOutput::ClearDisplayfromCursortoEndofDisplay => self.clear_forwards(),
                TerminalOutput::ClearDiplayfromStartofDisplaytoCursor => self.clear_backwards(),
                TerminalOutput::ClearScrollbackandDisplay => self.clear_all(),
                TerminalOutput::ClearDisplay => self.clear_visible(),
                TerminalOutput::ClearLineForwards => self.clear_line_forwards(),
                TerminalOutput::ClearLineBackwards => self.clear_line_backwards(),
                TerminalOutput::ClearLine => self.clear_line(),
                TerminalOutput::CarriageReturn => self.carriage_return(),
                TerminalOutput::Newline => self.new_line(),
                TerminalOutput::Backspace => self.backspace(),
                TerminalOutput::InsertLines(num_lines) => self.insert_lines(num_lines),
                TerminalOutput::Delete(num_chars) => self.delete(num_chars),
                TerminalOutput::Erase(num_chars) => self.erase(num_chars),
                TerminalOutput::Sgr(sgr) => self.sgr(sgr),
                TerminalOutput::SetMode(mode) => self.set_mode(&mode),
                TerminalOutput::InsertSpaces(num_spaces) => self.insert_spaces(num_spaces),
                TerminalOutput::ResetMode(mode) => self.reset_mode(&mode),
                TerminalOutput::DecSpecialGraphics(dec_special_graphics) => {
                    self.replace_characters = dec_special_graphics;
                }
                TerminalOutput::Bell
                | TerminalOutput::Invalid
                | TerminalOutput::OscResponse(_)
                | TerminalOutput::CursorReport
                | TerminalOutput::Skipped
                | TerminalOutput::ApplicationKeypadMode
                | TerminalOutput::NormalKeypadMode => (),
            }
        }
    }

    #[must_use]
    pub fn data(&self) -> TerminalSections<Vec<TChar>> {
        self.terminal_buffer.data()
    }

    #[must_use]
    pub fn format_data(&self) -> TerminalSections<Vec<FormatTag>> {
        let offset = self.terminal_buffer.data().scrollback.len();
        split_format_data_for_scrollback(self.format_tracker.tags(), offset)
    }

    #[must_use]
    pub fn cursor_pos(&self) -> CursorPos {
        self.cursor_state.pos.clone()
    }
}
