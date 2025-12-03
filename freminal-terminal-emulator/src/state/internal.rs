// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use anyhow::Result;
use conv2::ConvUtil;
use core::str;
use eframe::egui::{self, Color32, Context};
use freminal_common::{
    buffer_states::{
        buffer_type::BufferType,
        cursor::{CursorPos, CursorState, ReverseVideo},
        fonts::{FontDecorations, FontWeight},
        format_tag::FormatTag,
        line_wrap::LineWrap,
        tchar::TChar,
    },
    colors::TerminalColor,
    cursor::CursorVisualStyle,
    scroll::ScrollDirection,
    terminal_size::{DEFAULT_HEIGHT, DEFAULT_WIDTH},
    window_manipulation::WindowManipulation,
};
#[cfg(debug_assertions)]
use std::time::Instant;

use crate::{
    ansi::{FreminalAnsiParser, TerminalOutput},
    ansi_components::{
        line_draw::DecSpecialGraphics,
        mode::{Mode, SetMode, TerminalModes},
        modes::{
            allow_column_mode_switch::AllowColumnModeSwitch, decarm::Decarm, decawm::Decawm,
            decckm::Decckm, deccolm::Deccolm, decom::Decom, decsclm::Decsclm, decscnm::Decscnm,
            dectcem::Dectcem, grapheme::GraphemeClustering, lnm::Lnm, mouse::MouseTrack,
            reverse_wrap_around::ReverseWrapAround, rl_bracket::RlBracket,
            sync_updates::SynchronizedUpdates, theme::Theming, xtcblink::XtCBlink,
            xtextscrn::XtExtscrn, xtmsewin::XtMseWin, MouseModeNumber, ReportMode,
        },
        osc::{AnsiOscInternalType, AnsiOscType, UrlResponse},
        sgr::SelectGraphicRendition,
    },
    format_tracker::FormatTracker,
    interface::{
        collect_text, split_format_data_for_scrollback, TerminalInput, TerminalInputPayload,
    },
    io::PtyWrite,
    //state::term_char::display_vec_tchar_as_string,
};

use super::{
    buffer::{TerminalBufferHolder, TerminalBufferSetWinSizeResponse},
    data::TerminalSections,
};

#[derive(Debug, PartialEq, Eq)]
pub struct Buffer {
    pub terminal_buffer: TerminalBufferHolder,
    pub format_tracker: FormatTracker,
    pub cursor_state: CursorState,
    pub show_cursor: Dectcem,
    pub saved_cursor_position: Option<CursorPos>,
    pub cursor_color: TerminalColor,
}

impl Default for Buffer {
    fn default() -> Self {
        Self {
            cursor_color: TerminalColor::DefaultCursorColor,
            cursor_state: CursorState::default(),
            format_tracker: FormatTracker::new(),
            saved_cursor_position: None,
            show_cursor: Dectcem::default(),
            terminal_buffer: TerminalBufferHolder::new(
                DEFAULT_WIDTH as usize,
                DEFAULT_HEIGHT as usize,
                BufferType::Primary,
            ),
        }
    }
}

impl Buffer {
    #[must_use]
    pub fn new(width: usize, height: usize, buffer_type: BufferType) -> Self {
        Self {
            terminal_buffer: TerminalBufferHolder::new(width, height, buffer_type),
            format_tracker: FormatTracker::new(),
            cursor_state: CursorState::default(),
            show_cursor: Dectcem::default(),
            saved_cursor_position: None,
            cursor_color: TerminalColor::DefaultCursorColor,
        }
    }

    #[must_use]
    pub const fn show_cursor(&self) -> bool {
        self.terminal_buffer.show_cursor(&self.cursor_state.pos)
    }
}

#[derive(Debug, Default)]
pub enum Theme {
    Light,
    #[default]
    Dark,
}

impl From<bool> for Theme {
    fn from(dark_mode: bool) -> Self {
        if dark_mode {
            Self::Dark
        } else {
            Self::Light
        }
    }
}

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct TerminalState {
    pub parser: FreminalAnsiParser,
    pub current_buffer: BufferType,
    pub primary_buffer: Buffer,
    pub alternate_buffer: Buffer,
    pub modes: TerminalModes,
    pub write_tx: crossbeam_channel::Sender<PtyWrite>,
    pub changed: bool,
    pub ctx: Option<Context>,
    pub leftover_data: Option<Vec<u8>>,
    pub character_replace: DecSpecialGraphics,
    pub mouse_position: Option<egui::Pos2>,
    pub window_focused: bool,
    pub window_commands: Vec<WindowManipulation>,
    pub saved_cursor: Option<CursorState>,
    pub theme: Theme,
    pub cursor_visual_style: CursorVisualStyle,
}

impl Default for TerminalState {
    /// This method should never really be used. It was added to allow the test suite to pass
    /// The problem here is that you most likely really really want a rx channel to go with the tx channel
    fn default() -> Self {
        Self::new(crossbeam_channel::unbounded().0)
    }
}

impl PartialEq for TerminalState {
    fn eq(&self, other: &Self) -> bool {
        self.parser == other.parser
            && self.primary_buffer == other.primary_buffer
            && self.alternate_buffer == other.alternate_buffer
            && self.modes == other.modes
            && self.changed == other.changed
            && self.ctx == other.ctx
            && self.leftover_data == other.leftover_data
            && self.character_replace == other.character_replace
    }
}

impl TerminalState {
    #[must_use]
    pub fn new(write_tx: crossbeam_channel::Sender<PtyWrite>) -> Self {
        Self {
            parser: FreminalAnsiParser::new(),
            current_buffer: BufferType::Primary,
            primary_buffer: Buffer::new(
                DEFAULT_WIDTH as usize,
                DEFAULT_HEIGHT as usize,
                BufferType::Primary,
            ),
            alternate_buffer: Buffer::new(
                DEFAULT_WIDTH as usize,
                DEFAULT_HEIGHT as usize,
                BufferType::Alternate,
            ),
            modes: TerminalModes::default(),
            write_tx,
            changed: false,
            ctx: None,
            leftover_data: None,
            character_replace: DecSpecialGraphics::DontReplace,
            mouse_position: None,
            window_focused: true,
            window_commands: Vec::new(),
            saved_cursor: None,
            theme: Theme::default(),
            cursor_visual_style: CursorVisualStyle::default(),
        }
    }

    #[must_use]
    pub fn get_cursor_visual_style(&self) -> CursorVisualStyle {
        self.cursor_visual_style.clone()
    }

    pub const fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    #[must_use]
    pub const fn is_normal_display(&self) -> bool {
        self.modes.invert_screen.is_normal_display()
    }

    #[must_use]
    pub fn should_repeat_keys(&self) -> bool {
        self.modes.repeat_keys == Decarm::RepeatKey
    }

    #[must_use]
    pub const fn show_cursor(&mut self) -> bool {
        self.get_current_buffer().show_cursor()
    }

    #[must_use]
    pub fn skip_draw_always(&self) -> bool {
        self.modes.synchronized_updates == SynchronizedUpdates::DontDraw
    }

    #[must_use]
    pub fn is_changed(&self) -> bool {
        if self.modes.synchronized_updates == SynchronizedUpdates::DontDraw {
            debug!("Internal State: Synchronized updates is set to DontDraw, returning false");
            return false;
        }

        self.changed
    }

    pub const fn set_state_changed(&mut self) {
        self.changed = true;
    }

    pub const fn clear_changed(&mut self) {
        self.changed = false;
    }

    pub fn set_ctx(&mut self, ctx: Context) {
        if self.ctx.is_some() {
            return;
        }

        self.ctx = Some(ctx);
    }

    fn request_redraw(&mut self) {
        self.changed = true;
        if let Some(ctx) = &self.ctx {
            debug!("Internal State: Requesting repaint");
            ctx.request_repaint();
        }
    }

    pub const fn get_current_buffer(&mut self) -> &mut Buffer {
        match self.current_buffer {
            BufferType::Primary => &mut self.primary_buffer,
            BufferType::Alternate => &mut self.alternate_buffer,
        }
    }

    #[must_use]
    pub const fn get_win_size(&mut self) -> (usize, usize) {
        self.get_current_buffer().terminal_buffer.get_win_size()
    }

    pub(crate) fn data(&mut self, include_scrollback: bool) -> TerminalSections<Vec<TChar>> {
        self.get_current_buffer()
            .terminal_buffer
            .data(include_scrollback)
    }

    pub fn is_mouse_hovered_on_url(&mut self, pos: &CursorPos) -> Option<String> {
        let current_buffer = self.get_current_buffer();
        let buf_pos = current_buffer.terminal_buffer.cursor_pos_to_buf_pos(pos)?;

        let tags = self.get_current_buffer().format_tracker.tags();

        for tag in tags {
            if tag.url.is_none() {
                continue;
            }

            // check if the cursor pos is within the range of the tag
            if tag.start <= buf_pos && buf_pos < tag.end {
                if let Some(url) = &tag.url {
                    return Some(url.url.clone());
                }
            }
        }

        None
    }

    pub(crate) fn data_and_format_data_for_gui(
        &mut self,
    ) -> (
        TerminalSections<Vec<TChar>>,
        TerminalSections<Vec<FormatTag>>,
    ) {
        let (data, offset, end) = self.get_current_buffer().terminal_buffer.data_for_gui();

        let format_data = split_format_data_for_scrollback(
            self.get_current_buffer().format_tracker.tags(),
            offset,
            end,
            false,
        );

        (data, format_data)
    }

    #[must_use]
    pub const fn cursor_pos(&mut self) -> CursorPos {
        self.get_current_buffer().cursor_state.pos
    }

    pub fn set_win_size(
        &mut self,
        width: usize,
        height: usize,
    ) -> TerminalBufferSetWinSizeResponse {
        let current_buffer = self.get_current_buffer();
        let response = current_buffer.terminal_buffer.set_win_size(
            width,
            height,
            &current_buffer.cursor_state.pos,
        );
        self.get_current_buffer().cursor_state.pos = response.new_cursor_pos;

        response
    }

    #[must_use]
    pub fn get_cursor_key_mode(&self) -> Decckm {
        self.modes.cursor_key.clone()
    }

    pub fn set_window_focused(&mut self, focused: bool) {
        self.window_focused = focused;

        if self.modes.focus_reporting == XtMseWin::Disabled {
            return;
        }

        let to_write = if focused {
            TerminalInput::InFocus
        } else {
            TerminalInput::LostFocus
        };

        if let Err(e) = self.write(&to_write) {
            error!("Failed to write focus change: {e}");
        }

        debug!("Reported focus change to terminal");
    }

    pub(crate) fn handle_data(&mut self, data: &[u8]) {
        let data = match self.character_replace {
            //  Code page 1090
            // https://en.wikipedia.org/wiki/DEC_Special_Graphics / http://fileformats.archiveteam.org/wiki/DEC_Special_Graphics_Character_Set
            // 0x5f Blank	 	U+00A0 NO-BREAK SPACE
            // 0x60 Diamond	◆	U+25C6 BLACK DIAMOND
            // 0x61 Checkerboard	▒	U+2592 MEDIUM SHADE
            // 0x62 HT	␉	U+2409 SYMBOL FOR HORIZONTAL TABULATION
            // 0x63 FF	␌	U+240C SYMBOL FOR FORM FEED
            // 0x64 CR	␍	U+240D SYMBOL FOR CARRIAGE RETURN
            // 0x65 LF	␊	U+240A SYMBOL FOR LINE FEED
            // 0x66 Degree symbol	°	U+00B0 DEGREE SIGN
            // 0x67 Plus/minus	±	U+00B1 PLUS-MINUS SIGN
            // 0x68 NL	␤	U+2424 SYMBOL FOR NEWLINE
            // 0x69 VT	␋	U+240B SYMBOL FOR VERTICAL TABULATION
            // 0x6a Lower-right corner	┘	U+2518 BOX DRAWINGS LIGHT UP AND LEFT
            // 0x6b Upper-right corner	┐	U+2510 BOX DRAWINGS LIGHT DOWN AND LEFT
            // 0x6c Upper-left corner	┌	U+250C BOX DRAWINGS LIGHT DOWN AND RIGHT
            // 0x6d Lower-left corner	└	U+2514 BOX DRAWINGS LIGHT UP AND RIGHT
            // 0x6e Crossing Lines	┼	U+253C BOX DRAWINGS LIGHT VERTICAL AND HORIZONTAL
            // 0x6f Horizontal line - scan 1	⎺	U+23BA HORIZONTAL SCAN LINE-1
            // 0x70 Horizontal line - scan 3	⎻	U+23BB HORIZONTAL SCAN LINE-3
            // 0x71 Horizontal line - scan 5	─	U+2500 BOX DRAWINGS LIGHT HORIZONTAL
            // 0x72 Horizontal line - scan 7	⎼	U+23BC HORIZONTAL SCAN LINE-7
            // 0x73 Horizontal line - scan 9	⎽	U+23BD HORIZONTAL SCAN LINE-9
            // 0x74 Left "T"	├	U+251C BOX DRAWINGS LIGHT VERTICAL AND RIGHT
            // 0x75 Right "T"	┤	U+2524 BOX DRAWINGS LIGHT VERTICAL AND LEFT
            // 0x76 Bottom "T"	┴	U+2534 BOX DRAWINGS LIGHT UP AND HORIZONTAL
            // 0x77 Top "T"	┬	U+252C BOX DRAWINGS LIGHT DOWN AND HORIZONTAL
            // 0x78 Vertical bar	│	U+2502 BOX DRAWINGS LIGHT VERTICAL
            // 0x79 Less than or equal to	≤	U+2264 LESS-THAN OR EQUAL TO
            // 0x7a Greater than or equal to	≥	U+2265 GREATER-THAN OR EQUAL TO
            // 0x7b Pi	π	U+03C0 GREEK SMALL LETTER PI
            // 0x7c Not equal to	≠	U+2260 NOT EQUAL TO
            // 0x7d UK pound symbol	£	U+00A3 POUND SIGN
            // 0x7e Centered dot	·	U+00B7 MIDDLE DOT
            DecSpecialGraphics::Replace => {
                debug!("Replacing special graphics characters");
                // iterate through the characters and replace them with the appropriate unicode character
                let mut new_data = Vec::new();
                for c in data {
                    match c {
                        0x5f => new_data.extend_from_slice("\u{00A0}".as_bytes()),
                        0x60 => new_data.extend_from_slice("\u{25C6}".as_bytes()),
                        0x61 => new_data.extend_from_slice("\u{2592}".as_bytes()),
                        0x62 => new_data.extend_from_slice("\u{2409}".as_bytes()),
                        0x63 => new_data.extend_from_slice("\u{240C}".as_bytes()),
                        0x64 => new_data.extend_from_slice("\u{240D}".as_bytes()),
                        0x65 => new_data.extend_from_slice("\u{240A}".as_bytes()),
                        0x66 => new_data.extend_from_slice("\u{00B0}".as_bytes()),
                        0x67 => new_data.extend_from_slice("\u{00B1}".as_bytes()),
                        0x68 => new_data.extend_from_slice("\u{2424}".as_bytes()),
                        0x69 => new_data.extend_from_slice("\u{240B}".as_bytes()),
                        0x6a => new_data.extend_from_slice("\u{2518}".as_bytes()),
                        0x6b => new_data.extend_from_slice("\u{2510}".as_bytes()),
                        0x6c => new_data.extend_from_slice("\u{250C}".as_bytes()),
                        0x6d => new_data.extend_from_slice("\u{2514}".as_bytes()),
                        0x6e => new_data.extend_from_slice("\u{253C}".as_bytes()),
                        0x6f => new_data.extend_from_slice("\u{23BA}".as_bytes()),
                        0x70 => new_data.extend_from_slice("\u{23BB}".as_bytes()),
                        0x71 => new_data.extend_from_slice("\u{2500}".as_bytes()),
                        0x72 => new_data.extend_from_slice("\u{23BC}".as_bytes()),
                        0x73 => new_data.extend_from_slice("\u{23BD}".as_bytes()),
                        0x74 => new_data.extend_from_slice("\u{251C}".as_bytes()),
                        0x75 => new_data.extend_from_slice("\u{2524}".as_bytes()),
                        0x76 => new_data.extend_from_slice("\u{2534}".as_bytes()),
                        0x77 => new_data.extend_from_slice("\u{252C}".as_bytes()),
                        0x78 => new_data.extend_from_slice("\u{2502}".as_bytes()),
                        0x79 => new_data.extend_from_slice("\u{2264}".as_bytes()),
                        0x7a => new_data.extend_from_slice("\u{2265}".as_bytes()),
                        0x7b => new_data.extend_from_slice("\u{03C0}".as_bytes()),
                        0x7c => new_data.extend_from_slice("\u{2260}".as_bytes()),
                        0x7d => new_data.extend_from_slice("\u{00A3}".as_bytes()),
                        0x7e => new_data.extend_from_slice("\u{00B7}".as_bytes()),
                        _ => new_data.push(*c),
                    }
                }

                new_data
            }
            DecSpecialGraphics::DontReplace => data.to_vec(),
        };

        let current_buffer = self.get_current_buffer();
        let response = match current_buffer.terminal_buffer.insert_data(
            &current_buffer.cursor_state.pos,
            &data,
            &Decawm::from(current_buffer.cursor_state.line_wrap_mode),
        ) {
            Ok(response) => response,
            Err(e) => {
                error!("Failed to insert data: {e}");
                return;
            }
        };

        current_buffer
            .format_tracker
            .push_range_adjustment(response.insertion_range);
        current_buffer
            .format_tracker
            .push_range(&current_buffer.cursor_state, response.written_range);
        current_buffer.cursor_state.pos = response.new_cursor_pos;
    }

    pub const fn set_cursor_pos(&mut self, x: Option<usize>, y: Option<usize>) {
        let current_buffer = self.get_current_buffer();
        if let Some(x) = x {
            current_buffer.cursor_state.pos.x = x.saturating_sub(1);

            if current_buffer.cursor_state.pos.x > current_buffer.terminal_buffer.width - 1 {
                current_buffer.cursor_state.pos.x = current_buffer.terminal_buffer.width - 1;
            }
        }
        if let Some(y) = y {
            current_buffer.cursor_state.pos.y = y.saturating_sub(1);

            if current_buffer.cursor_state.pos.y > current_buffer.terminal_buffer.height - 1 {
                current_buffer.cursor_state.pos.y = current_buffer.terminal_buffer.height - 1;
            }
        }
    }

    pub fn set_cursor_pos_rel(&mut self, x: Option<i32>, y: Option<i32>) {
        let current_buffer = self.get_current_buffer();
        if let Some(x) = x {
            let x: i64 = x.into();
            let current_x: i64 = match current_buffer.cursor_state.pos.x.try_into() {
                Ok(x) => x,
                Err(e) => {
                    error!("Failed to convert x position to i64: {e}");
                    return;
                }
            };

            current_buffer.cursor_state.pos.x =
                usize::try_from((current_x + x).max(0)).unwrap_or(0);

            if current_buffer.cursor_state.pos.x > current_buffer.terminal_buffer.width - 1 {
                current_buffer.cursor_state.pos.x = current_buffer.terminal_buffer.width - 1;
            }
        }
        if let Some(y) = y {
            let y: i64 = y.into();
            let current_y: i64 = match current_buffer.cursor_state.pos.y.try_into() {
                Ok(y) => y,
                Err(e) => {
                    error!("Failed to convert y position to i64: {e}");
                    return;
                }
            };
            // ensure y is not negative, and throw an error if it is
            current_buffer.cursor_state.pos.y =
                usize::try_from((current_y + y).max(0)).unwrap_or(0);

            if current_buffer.cursor_state.pos.y > current_buffer.terminal_buffer.height - 1 {
                current_buffer.cursor_state.pos.y = current_buffer.terminal_buffer.height - 1;
            }
        }
    }

    pub(crate) fn clear_forwards(&mut self) {
        let current_buffer = self.get_current_buffer();
        if let Some(buf_pos) = current_buffer
            .terminal_buffer
            .clear_forwards(&current_buffer.cursor_state.pos)
        {
            current_buffer
                .format_tracker
                .push_range(&current_buffer.cursor_state, buf_pos);
        }
    }

    pub(crate) fn clear_backwards(&mut self) {
        let current_buffer = self.get_current_buffer();
        if let Some(buf_pos) = current_buffer
            .terminal_buffer
            .clear_backwards(&current_buffer.cursor_state.pos)
        {
            current_buffer
                .format_tracker
                .push_range(&current_buffer.cursor_state, buf_pos);
        }
    }

    pub(crate) fn clear_all(&mut self) {
        let current_buffer = self.get_current_buffer();
        current_buffer
            .format_tracker
            .push_range(&current_buffer.cursor_state, 0..usize::MAX);
        current_buffer.terminal_buffer.clear_all();
    }

    pub(crate) fn clear_visible(&mut self) {
        let current_buffer = self.get_current_buffer();

        let Some(range) = current_buffer.terminal_buffer.clear_visible() else {
            return;
        };

        if range.end > 0 {
            current_buffer
                .format_tracker
                .push_range(&current_buffer.cursor_state, range);
        }
    }

    pub(crate) fn clear_line_forwards(&mut self) {
        let current_buffer = self.get_current_buffer();

        if let Some(range) = current_buffer
            .terminal_buffer
            .clear_line_forwards(&current_buffer.cursor_state.pos)
        {
            match current_buffer.format_tracker.delete_range(range.clone()) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to delete range: {e}");
                }
            }

            current_buffer.format_tracker.push_range_adjustment(range);
        }
    }

    pub(crate) fn clear_line_backwards(&mut self) {
        let current_buffer = self.get_current_buffer();

        if let Some(range) = current_buffer
            .terminal_buffer
            .clear_line_backwards(&current_buffer.cursor_state.pos)
        {
            match current_buffer.format_tracker.delete_range(range.clone()) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to delete range: {e}");
                }
            }

            current_buffer.format_tracker.push_range_adjustment(range);
        }
    }

    pub(crate) fn clear_line(&mut self) {
        let current_buffer = self.get_current_buffer();

        if let Some(range) = current_buffer
            .terminal_buffer
            .clear_line(&current_buffer.cursor_state.pos)
        {
            match current_buffer.format_tracker.delete_range(range.clone()) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to delete range: {e}");
                }
            }

            current_buffer.format_tracker.push_range_adjustment(range);
        }
    }

    pub(crate) const fn carriage_return(&mut self) {
        self.get_current_buffer().cursor_state.pos.x = 0;
    }

    pub(crate) fn new_line(&mut self) {
        self.get_current_buffer().cursor_state.pos.y += 1;

        if self.modes.line_feed_mode == Lnm::NewLine {
            self.get_current_buffer().cursor_state.pos.x = 0;
        }
    }

    pub(crate) fn backspace(&mut self) {
        let current_buffer = self.get_current_buffer();
        debug!("Backspace at {}", current_buffer.cursor_state.pos);

        if current_buffer.cursor_state.pos.x >= 1 {
            if current_buffer.cursor_state.pos.x == current_buffer.terminal_buffer.width {
                current_buffer.cursor_state.pos.x = current_buffer.terminal_buffer.width - 2;
            } else {
                current_buffer.cursor_state.pos.x -= 1;
            }
        } else {
            // FIXME: this is not correct, we should move to the end of the previous line
            warn!("FIXME: Backspace at the beginning of the line. Not wrapping");
        }

        debug!("Backspace moved to {}", current_buffer.cursor_state.pos);
    }

    pub(crate) fn insert_lines(&mut self, num_lines: usize) {
        let current_buffer = self.get_current_buffer();

        let response = current_buffer
            .terminal_buffer
            .insert_lines(&current_buffer.cursor_state.pos, num_lines);
        match current_buffer
            .format_tracker
            .delete_range(response.deleted_range)
        {
            Ok(()) => (),
            Err(e) => {
                error!("Failed to delete range: {e}");
                return;
            }
        }

        current_buffer
            .format_tracker
            .push_range_adjustment(response.inserted_range);
    }

    pub(crate) fn delete(&mut self, num_chars: usize) {
        let current_buffer = self.get_current_buffer();

        let deleted_buf_range = current_buffer
            .terminal_buffer
            .delete_forwards(&current_buffer.cursor_state.pos, num_chars);
        if let Some(range) = deleted_buf_range {
            match current_buffer.format_tracker.delete_range(range) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to delete range: {e}");
                }
            }
        }
    }

    pub(crate) fn erase_forwards(&mut self, num_chars: usize) {
        let current_buffer = self.get_current_buffer();

        let deleted_buf_range = current_buffer
            .terminal_buffer
            .erase_forwards(&current_buffer.cursor_state.pos, num_chars);
        if let Some(range) = deleted_buf_range {
            match current_buffer.format_tracker.delete_range(range.clone()) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to delete range: {e}");
                }
            }

            current_buffer.format_tracker.push_range_adjustment(range);
        }
    }

    pub(crate) fn reset(&mut self) {
        // FIXME: move these to the buffer struct
        let current_buffer = self.get_current_buffer();

        current_buffer.cursor_state.colors.set_default();
        current_buffer.cursor_state.font_weight = FontWeight::Normal;
        current_buffer.cursor_state.font_decorations.clear();
    }

    pub(crate) fn font_decordations_add_if_not_contains(&mut self, decoration: FontDecorations) {
        let current_buffer = self.get_current_buffer();

        if !current_buffer
            .cursor_state
            .font_decorations
            .contains(&decoration)
        {
            current_buffer
                .cursor_state
                .font_decorations
                .push(decoration);
        }
    }

    pub(crate) fn font_decorations_remove_if_contains(&mut self, decoration: &FontDecorations) {
        self.get_current_buffer()
            .cursor_state
            .font_decorations
            .retain(|d| *d != *decoration);
    }

    pub(crate) const fn set_foreground(&mut self, color: TerminalColor) {
        self.get_current_buffer()
            .cursor_state
            .colors
            .set_color(color);
    }

    pub(crate) const fn set_background(&mut self, color: TerminalColor) {
        self.get_current_buffer()
            .cursor_state
            .colors
            .set_background_color(color);
    }

    pub(crate) const fn set_underline_color(&mut self, color: TerminalColor) {
        self.get_current_buffer()
            .cursor_state
            .colors
            .set_underline_color(color);
    }

    pub(crate) const fn set_reverse_video(&mut self, reverse_video: ReverseVideo) {
        self.get_current_buffer()
            .cursor_state
            .colors
            .set_reverse_video(reverse_video);
    }

    pub(crate) fn sgr(&mut self, sgr: SelectGraphicRendition) {
        match sgr {
            SelectGraphicRendition::NoOp => (),
            SelectGraphicRendition::Reset => self.reset(),
            SelectGraphicRendition::Bold => {
                self.get_current_buffer().cursor_state.font_weight = FontWeight::Bold;
            }
            SelectGraphicRendition::Underline => {
                self.font_decordations_add_if_not_contains(FontDecorations::Underline);
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
            SelectGraphicRendition::ResetBold => {
                self.get_current_buffer().cursor_state.font_weight = FontWeight::Normal;
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
                self.set_reverse_video(ReverseVideo::On);
            }
            SelectGraphicRendition::ResetReverseVideo => {
                self.set_reverse_video(ReverseVideo::Off);
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

    pub(crate) fn insert_spaces(&mut self, num_spaces: usize) {
        let current_buffer = self.get_current_buffer();

        let response = current_buffer
            .terminal_buffer
            .insert_spaces(&current_buffer.cursor_state.pos, num_spaces);
        current_buffer
            .format_tracker
            .push_range_adjustment(response.insertion_range);
    }

    pub(crate) fn screen_alignment_test(&mut self) {
        self.reset();
        self.clear_all();
        let current_buffer = self.get_current_buffer();
        let response = current_buffer.terminal_buffer.screen_alignment_test();
        current_buffer
            .format_tracker
            .push_range_adjustment(response);

        // set the cursor to the top left corner
        current_buffer.cursor_state.pos = CursorPos::default();
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn set_mode(&mut self, mode: &Mode) {
        match mode {
            Mode::NoOp => {}
            Mode::Decckm(Decckm::Query) => {
                self.report_mode(&self.get_cursor_key_mode().report(None));
            }
            Mode::Decckm(decckm) => {
                self.modes.cursor_key = decckm.clone();
            }
            Mode::Decawm(Decawm::Query) => {
                let to_write = Decawm::from(self.get_current_buffer().cursor_state.line_wrap_mode);

                self.report_mode(&to_write.report(None));
            }
            Mode::Decawm(decawm) => {
                let assign = match decawm {
                    Decawm::AutoWrap => LineWrap::Wrap,
                    Decawm::NoAutoWrap => LineWrap::NoWrap,
                    Decawm::Query => unreachable!(),
                };
                self.get_current_buffer().cursor_state.line_wrap_mode = assign;
            }
            Mode::Dectem(Dectcem::Query) => {
                let to_write = self.get_current_buffer().show_cursor.report(None);
                self.report_mode(&to_write);
            }
            Mode::Dectem(dectem) => {
                self.get_current_buffer().show_cursor = dectem.clone();
            }
            Mode::BracketedPaste(RlBracket::Query) => {
                self.report_mode(&self.modes.bracketed_paste.report(None));
            }
            Mode::BracketedPaste(bracketed_paste) => {
                self.modes.bracketed_paste = bracketed_paste.clone();
            }
            Mode::XtCBlink(XtCBlink::Query) => {
                self.report_mode(&self.modes.cursor_blinking.report(None));
            }
            Mode::XtCBlink(xtcblink) => {
                self.modes.cursor_blinking = xtcblink.clone();

                // also set the cursor visual style so the UI gets the new state
                if self.modes.cursor_blinking == XtCBlink::Blinking {
                    match self.cursor_visual_style {
                        CursorVisualStyle::BlockCursorSteady => {
                            self.cursor_visual_style = CursorVisualStyle::BlockCursorBlink;
                        }
                        CursorVisualStyle::UnderlineCursorSteady => {
                            self.cursor_visual_style = CursorVisualStyle::UnderlineCursorBlink;
                        }
                        CursorVisualStyle::VerticalLineCursorSteady => {
                            self.cursor_visual_style = CursorVisualStyle::VerticalLineCursorBlink;
                        }
                        _ => (),
                    }
                } else {
                    match self.cursor_visual_style {
                        CursorVisualStyle::BlockCursorBlink => {
                            self.cursor_visual_style = CursorVisualStyle::BlockCursorSteady;
                        }
                        CursorVisualStyle::UnderlineCursorBlink => {
                            self.cursor_visual_style = CursorVisualStyle::UnderlineCursorSteady;
                        }
                        CursorVisualStyle::VerticalLineCursorBlink => {
                            self.cursor_visual_style = CursorVisualStyle::VerticalLineCursorSteady;
                        }
                        _ => (),
                    }
                }
            }
            Mode::XtExtscrn(XtExtscrn::Query) => {
                let to_write = match self.current_buffer {
                    BufferType::Primary => XtExtscrn::Primary,
                    BufferType::Alternate => XtExtscrn::Alternate,
                }
                .report(None);

                self.report_mode(&to_write);
            }
            Mode::XtExtscrn(XtExtscrn::Alternate) => {
                debug!("Switching to alternate screen buffer");
                // SPEC Steps:
                // 1. Save the cursor position
                // 2. Switch to the alternate screen buffer
                // 3. Clear the screen

                // TODO: We're supposed to save the cursor POS here. Do we assign the current cursor pos to the saved cursor pos?
                // I don't see why we need to explicitly do that, as the cursor pos is already saved in the buffer
                // Do we copy the cursor pos to the new buffer?
                // Also, the "clear screen" bit implies to me that the buffer we switch to is *always* new, but is that correct?
                // This is why we're making a "new" buffer here

                let (width, height) = self.get_current_buffer().terminal_buffer.get_win_size();
                self.alternate_buffer = Buffer::new(width, height, BufferType::Alternate);
                self.current_buffer = BufferType::Alternate;
            }
            Mode::XtExtscrn(XtExtscrn::Primary) => {
                debug!("Switching to primary screen buffer");
                // SPEC Steps:
                // 1. Restore the cursor position
                // 2. Switch to the primary screen buffer
                // 3. Clear the screen
                // See set mode for notes on the cursor pos

                self.current_buffer = BufferType::Primary;
                let (width, height) = self.get_current_buffer().terminal_buffer.get_win_size();
                self.alternate_buffer = Buffer::new(width, height, BufferType::Alternate);
            }
            Mode::XtMseWin(XtMseWin::Query) => {
                self.report_mode(&self.modes.focus_reporting.report(None));
            }
            Mode::XtMseWin(XtMseWin::Enabled) => {
                debug!("Setting focus reporting");
                self.modes.focus_reporting = XtMseWin::Enabled;

                let to_write = if self.window_focused {
                    TerminalInput::InFocus
                } else {
                    TerminalInput::LostFocus
                };

                if let Err(e) = self.write(&to_write) {
                    error!("Failed to write focus change: {e}");
                }

                debug!("Reported current focus {:?} to terminal", to_write);
            }
            Mode::XtMseWin(XtMseWin::Disabled) => {
                self.modes.focus_reporting = XtMseWin::Disabled;
            }
            Mode::MouseMode(MouseTrack::Query(v)) => {
                let is_set = if self.modes.mouse_tracking.mouse_mode_number() == *v {
                    SetMode::DecSet
                } else {
                    SetMode::DecRst
                };

                self.report_mode(&self.modes.mouse_tracking.report(Some(is_set)));
            }
            Mode::MouseMode(mode) => {
                if let MouseTrack::XtMsex10
                | MouseTrack::XtMseX11
                | MouseTrack::XtMseBtn
                | MouseTrack::NoTracking
                | MouseTrack::XtMseAny
                | MouseTrack::XtMseSgr = mode
                {
                    debug!("Setting mode to: {mode}");
                    self.modes.mouse_tracking = mode.clone();
                } else {
                    warn!("Unhandled mouse mode: {mode}");
                }
            }
            Mode::SynchronizedUpdates(SynchronizedUpdates::Query) => {
                self.report_mode(&self.modes.synchronized_updates.report(None));
            }
            Mode::SynchronizedUpdates(sync) => {
                self.modes.synchronized_updates = sync.clone();
            }
            Mode::UnknownQuery(m) => {
                let query = String::from_utf8(m.clone())
                    .unwrap_or_else(|_| String::from("Unable to convert to string"));
                warn!("Querying unknown mode: {query}");
                self.report_mode(&mode.report(None));
            }
            Mode::Unknown(_) => {
                warn!("unhandled mode: {mode}");
            }
            Mode::Deccolm(Deccolm::Query) => {
                self.report_mode(&Deccolm::Query.report(None));
            }
            Mode::Deccolm(deccolm) => {
                warn!("Received DECCOLM({deccolm}), but it's not supported");
            }
            Mode::Decsclm(Decsclm::Query) => {
                self.report_mode(&Decsclm::Query.report(None));
            }
            Mode::Decsclm(decsclm) => {
                warn!("Received DECSCLM({decsclm}), but it's not supported");
            }
            Mode::Decom(Decom::Query) => {
                self.report_mode(&Decom::Query.report(None));
            }
            Mode::Decom(decom) => {
                warn!("Received DECOM({decom}), but it's not supported");
            }
            Mode::Decscnm(Decscnm::Query) => {
                self.report_mode(&self.modes.invert_screen.report(None));
            }
            Mode::Decarm(Decarm::Query) => {
                self.report_mode(&self.modes.repeat_keys.report(None));
            }
            Mode::Decarm(decarm) => {
                self.modes.repeat_keys = decarm.clone();
            }
            Mode::Decscnm(decscnm) => {
                self.modes.invert_screen = decscnm.clone();
            }
            Mode::AllowColumnModeSwitch(AllowColumnModeSwitch::Query) => {
                self.report_mode(&AllowColumnModeSwitch::Query.report(None));
            }
            Mode::AllowColumnModeSwitch(allow_column_resize) => {
                warn!("Received AllowColumnResize({allow_column_resize}), but it's not supported");
            }
            Mode::ReverseWrapAround(ReverseWrapAround::Query) => {
                self.report_mode(&self.modes.reverse_wrap_around.report(None));
            }
            Mode::ReverseWrapAround(reverse_wrap_around) => {
                self.modes.reverse_wrap_around = reverse_wrap_around.clone();
            }
            Mode::LineFeedMode(Lnm::Query) => {
                self.report_mode(&self.modes.line_feed_mode.report(None));
            }
            Mode::LineFeedMode(line_feed_new_line) => {
                self.modes.line_feed_mode = line_feed_new_line.clone();
            }
            Mode::GraphemeClustering(GraphemeClustering::Query) => {
                self.report_mode(&GraphemeClustering::Query.report(None));
            }
            Mode::GraphemeClustering(grapheme_clustering) => {
                warn!("Received GraphemeClustering({grapheme_clustering}), but it's not supported");
            }
            Mode::Theming(Theming::Query) => {
                let theme = match self.theme {
                    Theme::Light => SetMode::DecSet,
                    Theme::Dark => SetMode::DecRst,
                };

                self.report_mode(&Theming::Query.report(Some(theme)));
            }
            Mode::Theming(theming) => {
                warn!("Received Theming({theming}), but it's not supported");
            }
        }
    }

    pub(crate) fn report_da(&self) {
        // FIXME: I don't know if we're covering all of the supported DA's we should be sending
        // for now, we're sending the following (borrowed from iTerm2):
        // 65: VT525
        // 1: 132 columns
        // 2: Printer port (should we send this?)
        // 4: Sixel graphics
        // 6: Selective erase
        // 17: Terminal State Interrogation
        // 18: User windows
        // 22: ANSI color, e.g., VT525

        let output = collect_text(&String::from("\x1b[?65;1;2;4;6;17;18;22c"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write da: {e}");
                }
            }
        }
    }

    pub(crate) fn osc_response(&mut self, osc: AnsiOscType) {
        match osc {
            AnsiOscType::NoOp => (),
            AnsiOscType::Url(url) => match url {
                UrlResponse::End => {
                    self.get_current_buffer().cursor_state.url = None;
                }
                UrlResponse::Url(url_value) => {
                    self.get_current_buffer().cursor_state.url = Some(url_value);
                }
            },
            AnsiOscType::RequestColorQueryBackground(color) => {
                match color {
                    // OscInternalType::SetColor(_) => {
                    //     warn!("RequestColorQueryBackground: Set is not supported");
                    // }
                    AnsiOscInternalType::Query => {
                        // lets get the color as a hex string

                        // FIXME: this is hard coded and should be from the active color scheme
                        let (r, g, b, a) =
                            Color32::from_hex("#45475a").unwrap_or_default().to_tuple();
                        let output = collect_text(&format!(
                            "\x1b]11;rgb:{r:02x}/{g:02x}/{b:02x}{a:02x}\x1b\\"
                        ));

                        for byte in output.iter() {
                            if let Err(e) = self.write(byte) {
                                error!("Failed to write osc color response: {e}");
                            }
                        }
                    }
                    AnsiOscInternalType::Unknown(_) => {
                        warn!("OSC Unknown is not supported");
                    }
                    AnsiOscInternalType::String(_) => {
                        warn!("OSC Type {color:?} Skipped");
                    }
                }
            }
            AnsiOscType::RequestColorQueryForeground(color) => {
                match color {
                    // OscInternalType::SetColor(_) => {
                    //     warn!("RequestColorQueryForeground: Set is not supported");
                    // }
                    AnsiOscInternalType::Query => {
                        // lets get the color as a hex string
                        let (r, g, b, a) = Color32::WHITE.to_tuple();

                        let output = collect_text(&format!(
                            "\x1b]10;rgb:{r:02x}/{g:02x}/{b:02x}{a:02x}\x1b\\"
                        ));

                        for byte in output.iter() {
                            if let Err(e) = self.write(byte) {
                                error!("Failed to write osc color response: {e}");
                            }
                        }
                    }
                    AnsiOscInternalType::Unknown(_) => {
                        warn!("OSC Unknown is not supported");
                    }
                    AnsiOscInternalType::String(_) => {
                        warn!("OSC Type {color:?} Skipped");
                    }
                }
            }
            AnsiOscType::SetTitleBar(title) => {
                self.window_commands
                    .push(WindowManipulation::SetTitleBarText(title));
            }
            AnsiOscType::Ftcs(value) => {
                debug!("Ftcs is not supported: {value}");
            }
            // FIXME: I think once we get in to muxxing we'll need to handle this
            // I think the idea here is that OSC 7 is emitted to inform the terminal of the current working directory
            // So that if you open a new tab in the terminal, it will start in the same directory as the current tab
            // https://github.com/jarun/nnn/issues/1147
            AnsiOscType::RemoteHost(value) => {
                debug!("Received for remote host: {value}");
            }
            AnsiOscType::ResetCursorColor => {
                self.get_current_buffer().cursor_color = TerminalColor::DefaultCursorColor;
            }
            AnsiOscType::ITerm2 => {
                debug!("iTerm2 OSC codes are not supported yet");
            }
        }
    }

    pub(crate) fn report_cursor_position(&mut self) {
        let current_buffer = self.get_current_buffer();

        let x = current_buffer.cursor_state.pos.x + 1;
        let y = current_buffer.cursor_state.pos.y + 1;
        debug!("Reporting cursor position: {y}, {x}");
        let output = collect_text(&format!("\x1b[{y};{x}R"));

        for input in output.iter() {
            if let Err(e) = self.write(input) {
                error!("Failed to write cursor position: {e}");
            }
        }
    }

    pub fn report_window_state(&mut self, minimized: bool) {
        let output = if minimized {
            collect_text(&String::from("\x1b[2t"))
        } else {
            collect_text(&String::from("\x1b[1t"))
        };
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write window state: {e}");
                }
            }
        }
    }

    pub fn report_window_position(&mut self, x: usize, y: usize) {
        let output = collect_text(&format!("\x1b[3;{x};{y}t"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write window position: {e}");
                }
            }
        }
    }

    pub fn report_window_size(&mut self, width: usize, height: usize) {
        let output = collect_text(&format!("\x1b[4;{height};{width}t"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write window size: {e}");
                }
            }
        }
    }

    pub fn report_root_window_size(&mut self, width: usize, height: usize) {
        let output = collect_text(&format!("\x1b[5;{height};{width}t"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write window size: {e}");
                }
            }
        }
    }

    pub fn report_character_size(&mut self, width: usize, height: usize) {
        let output = collect_text(&format!("\x1b[6;{height};{width}t"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write character size: {e}");
                }
            }
        }
    }

    pub fn report_terminal_size_in_characters(&mut self, width: usize, height: usize) {
        let output = collect_text(&format!("\x1b[8;{height};{width}t"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write terminal size in characters: {e}");
                }
            }
        }
    }

    pub fn report_root_terminal_size_in_characters(&mut self, width: usize, height: usize) {
        let output = collect_text(&format!("\x1b[9;{height};{width}t"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write terminal size in characters: {e}");
                }
            }
        }
    }

    pub fn report_icon_label(&mut self, title: &str) {
        let output = collect_text(&format!("\x1b]L{title}\x1b\\"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write title: {e}");
                }
            }
        }
    }

    pub fn report_device_name_and_version(&mut self) {
        let version = format!(
            "{}-{}",
            env!("CARGO_PKG_VERSION"),
            env!("VERGEN_BUILD_TIMESTAMP")
        );
        let output = collect_text(&format!("\x1bP>|Freminal {version}\x1b\\"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write device name and version: {e}");
                }
            }
        }
    }

    pub fn report_title(&mut self, title: &str) {
        let output = collect_text(&format!("\x1b]l{title}\x1b\\"));
        for input in output.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write title: {e}");
                }
            }
        }
    }

    pub fn report_mode(&mut self, report: &String) {
        let report = collect_text(report);
        for input in report.iter() {
            match self.write(input) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to write mode report: {e}");
                }
            }
        }
    }

    pub(crate) fn clip_buffer_lines(&mut self) {
        match self.current_buffer {
            BufferType::Primary => {
                let current_buffer = self.get_current_buffer();

                if let Some(range) = current_buffer
                    .terminal_buffer
                    .clip_lines_for_primary_buffer()
                {
                    match current_buffer.format_tracker.delete_range(range) {
                        Ok(()) => (),
                        Err(e) => {
                            error!("Failed to delete range: {e}");
                        }
                    }
                }
            }
            BufferType::Alternate => {
                let current_buffer = self.get_current_buffer();

                if let Some(range) = current_buffer
                    .terminal_buffer
                    .clip_lines_for_alternate_buffer()
                {
                    match current_buffer.format_tracker.delete_range(range) {
                        Ok(()) => (),
                        Err(e) => {
                            error!("Failed to delete range: {e}");
                        }
                    }
                }
            }
        }
    }

    pub const fn set_top_and_bottom_margins(&mut self, top: usize, bottom: usize) {
        let current_buffer = self.get_current_buffer();

        current_buffer
            .terminal_buffer
            .set_top_and_bottom_margins(top, bottom);
        self.set_cursor_pos(Some(1), Some(1));
    }

    #[allow(clippy::too_many_lines)]
    pub fn handle_incoming_data(&mut self, incoming: &[u8]) {
        debug!("Handling Incoming Data");
        #[cfg(debug_assertions)]
        let now = Instant::now();
        // if we have leftover data, prepend it to the incoming data
        let mut incoming = self.leftover_data.take().map_or_else(
            || incoming.to_vec(),
            |leftover_data| {
                debug!("We have leftover data: {:?}", leftover_data);
                let mut new_data = Vec::with_capacity(leftover_data.len() + incoming.len());
                new_data.extend_from_slice(&leftover_data);
                new_data.extend_from_slice(incoming);
                trace!("Reassembled buffer: {:?}", new_data);
                self.leftover_data = None;
                new_data
            },
        );

        let mut leftover_bytes = vec![];
        while let Err(_e) = String::from_utf8(incoming.clone()) {
            let Some(p) = incoming.pop() else { break };
            leftover_bytes.insert(0, p);
        }

        if !leftover_bytes.is_empty() {
            match self.leftover_data {
                Some(ref mut self_leftover) => {
                    // this should be at the start of the leftover data
                    self_leftover.splice(0..0, leftover_bytes);
                }
                None => self.leftover_data = Some(leftover_bytes),
            }
        }

        // verify that the incoming data is utf-8
        let parsed = self.parser.push(&incoming);

        for segment in parsed {
            // if segment is not data, we want to print out the segment
            if let TerminalOutput::Data(data) = &segment {
                debug!(
                    "Incoming segment (data): \"{}\"",
                    str::from_utf8(data).unwrap_or(&format!(
                        "Failed to parse data for display as string: {data:?}"
                    ))
                );
            } else {
                debug!("Incoming segment: {segment}");
            }

            match segment {
                TerminalOutput::Data(data) => self.handle_data(&data),
                TerminalOutput::SetCursorPos { x, y } => self.set_cursor_pos(x, y),
                TerminalOutput::SetCursorPosRel { x, y } => self.set_cursor_pos_rel(x, y),
                TerminalOutput::ClearDisplayfromCursortoEndofDisplay => self.clear_forwards(),
                TerminalOutput::ClearDisplayfromStartofDisplaytoCursor => self.clear_backwards(),
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
                TerminalOutput::Erase(num_chars) => self.erase_forwards(num_chars),
                TerminalOutput::Sgr(sgr) => self.sgr(sgr),
                TerminalOutput::Mode(mode) => self.set_mode(&mode),
                TerminalOutput::InsertSpaces(num_spaces) => self.insert_spaces(num_spaces),
                TerminalOutput::OscResponse(osc) => self.osc_response(osc),
                TerminalOutput::DecSpecialGraphics(dec_special_graphics) => {
                    self.character_replace = dec_special_graphics;
                }
                TerminalOutput::CursorReport => self.report_cursor_position(),
                TerminalOutput::ApplicationKeypadMode => {
                    self.modes.cursor_key = Decckm::Application;
                }
                TerminalOutput::NormalKeypadMode => self.modes.cursor_key = Decckm::Ansi,
                TerminalOutput::CursorVisualStyle(style) => {
                    self.cursor_visual_style = style;
                }
                TerminalOutput::WindowManipulation(manip) => self.window_commands.push(manip),
                TerminalOutput::SetTopAndBottomMargins {
                    top_margin,
                    bottom_margin,
                } => {
                    self.set_top_and_bottom_margins(top_margin, bottom_margin);
                }
                TerminalOutput::RequestDeviceAttributes => self.report_da(),
                TerminalOutput::ScreenAlignmentTest => self.screen_alignment_test(),
                TerminalOutput::SaveCursor => {
                    self.saved_cursor = Some(self.get_current_buffer().cursor_state.clone());
                }
                TerminalOutput::RestoreCursor => {
                    if let Some(saved_cursor) = &self.saved_cursor {
                        self.get_current_buffer().cursor_state = saved_cursor.clone();
                    }
                }
                TerminalOutput::RequestDeviceNameAndVersion => {
                    self.report_device_name_and_version();
                }
                TerminalOutput::Skipped | TerminalOutput::Bell | _ => (),
            }
        }

        // now ensure total lines in buffer isn't too big
        self.clip_buffer_lines();

        #[cfg(debug_assertions)]
        // log the frame time
        let elapsed = now.elapsed();
        // show either elapsed as micros or millis, depending on the duration
        #[cfg(debug_assertions)]
        if elapsed.as_millis() > 0 {
            debug!("Data processing time: {}ms", elapsed.as_millis());
        } else {
            debug!("Data processing time: {}μs", elapsed.as_micros());
        }

        self.set_state_changed();
        self.request_redraw();
        debug!("Finished handling incoming data");
    }

    /// Write data to the terminal
    ///
    /// # Errors
    /// Will return an error if the write fails
    pub fn write(&self, to_write: &TerminalInput) -> Result<()> {
        match to_write.to_payload(
            self.get_cursor_key_mode() == Decckm::Application,
            self.get_cursor_key_mode() == Decckm::Application,
        ) {
            TerminalInputPayload::Single(c) => {
                self.write_tx.send(PtyWrite::Write(vec![c]))?;
            }
            TerminalInputPayload::Many(to_write) => {
                self.write_tx.send(PtyWrite::Write(to_write.to_vec()))?;
            }
        }

        Ok(())
    }

    pub fn scroll(&mut self, scroll: f32) {
        if self.current_buffer == BufferType::Alternate {
            let key = if scroll < 0.0 {
                TerminalInput::ArrowDown
            } else {
                TerminalInput::ArrowUp
            };

            match self.write(&key) {
                Ok(()) => (),
                Err(e) => {
                    error!("Failed to scroll: {e}");
                }
            }

            return;
        }

        let current_buffer = &mut self.get_current_buffer().terminal_buffer;
        // convert the scroll to usize, with a minimum of 1
        let mut scroll = scroll.round();

        if scroll < 0.0 {
            scroll *= -1.0;
            let scroll_as_usize = match scroll.max(1.0).approx_as::<usize>() {
                Ok(scroll) => scroll,
                Err(e) => {
                    error!("Failed to convert scroll to usize: {e}\nUsing default of 1");
                    1
                }
            };

            let scoller = ScrollDirection::Down(scroll_as_usize);
            current_buffer.scroll(&scoller);
        } else {
            let scroll_as_usize = match scroll.max(1.0).approx_as::<usize>() {
                Ok(scroll) => scroll,
                Err(e) => {
                    error!("Failed to convert scroll to usize: {e}\nUsing default of 1");
                    1
                }
            };

            let scroller = ScrollDirection::Up(scroll_as_usize);
            current_buffer.scroll(&scroller);
        }
    }
}
