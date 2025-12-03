// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::{
    buffer_states::{
        fonts::{FontDecorations, FontWeight},
        line_wrap::LineWrap,
        url::Url,
    },
    colors::TerminalColor,
};

#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub enum ReverseVideo {
    On,
    #[default]
    Off,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StateColors {
    pub color: TerminalColor,
    pub background_color: TerminalColor,
    pub underline_color: TerminalColor,
    pub reverse_video: ReverseVideo,
}

impl Default for StateColors {
    fn default() -> Self {
        Self {
            color: TerminalColor::Default,
            background_color: TerminalColor::DefaultBackground,
            underline_color: TerminalColor::DefaultUnderlineColor,
            reverse_video: ReverseVideo::default(),
        }
    }
}

impl StateColors {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn set_default(&mut self) {
        self.color = TerminalColor::Default;
        self.background_color = TerminalColor::DefaultBackground;
        self.underline_color = TerminalColor::DefaultUnderlineColor;
        self.reverse_video = ReverseVideo::Off;
    }

    #[must_use]
    pub const fn with_background_color(mut self, background_color: TerminalColor) -> Self {
        self.background_color = background_color;
        self
    }

    #[must_use]
    pub const fn with_color(mut self, color: TerminalColor) -> Self {
        self.color = color;
        self
    }

    #[must_use]
    pub const fn with_underline_color(mut self, underline_color: TerminalColor) -> Self {
        self.underline_color = underline_color;
        self
    }

    #[must_use]
    pub const fn with_reverse_video(mut self, reverse_video: ReverseVideo) -> Self {
        self.reverse_video = reverse_video;
        self
    }

    pub const fn set_color(&mut self, color: TerminalColor) {
        self.color = color;
    }

    pub const fn set_background_color(&mut self, background_color: TerminalColor) {
        self.background_color = background_color;
    }

    pub const fn set_underline_color(&mut self, underline_color: TerminalColor) {
        self.underline_color = underline_color;
    }

    pub const fn set_reverse_video(&mut self, reverse_video: ReverseVideo) {
        self.reverse_video = reverse_video;
    }

    #[must_use]
    pub const fn get_color(&self) -> TerminalColor {
        match self.reverse_video {
            ReverseVideo::On => self.background_color.default_to_regular(),
            ReverseVideo::Off => self.color,
        }
    }

    #[must_use]
    pub const fn get_background_color(&self) -> TerminalColor {
        match self.reverse_video {
            ReverseVideo::On => self.color.default_to_regular(),
            ReverseVideo::Off => self.background_color,
        }
    }

    // FIXME: How does this work if an underline color is set but reverse video is on?
    // Probably should also check if underline color is set to default
    #[must_use]
    pub const fn get_underline_color(&self) -> TerminalColor {
        match self.reverse_video {
            ReverseVideo::On => self.background_color.default_to_regular(),
            ReverseVideo::Off => self.underline_color,
        }
    }

    pub const fn flip_reverse_video(&mut self) {
        self.reverse_video = match self.reverse_video {
            ReverseVideo::On => ReverseVideo::Off,
            ReverseVideo::Off => ReverseVideo::On,
        };
    }
}

#[allow(clippy::module_name_repetitions)]
#[derive(Eq, PartialEq, Debug, Clone, Default)]
pub struct CursorState {
    pub pos: CursorPos,
    pub font_weight: FontWeight,
    pub font_decorations: Vec<FontDecorations>,
    pub colors: StateColors,
    pub line_wrap_mode: LineWrap,
    pub url: Option<Url>,
}

impl CursorState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn with_background_color(mut self, background_color: TerminalColor) -> Self {
        self.colors.set_background_color(background_color);
        self
    }

    #[must_use]
    pub const fn with_color(mut self, color: TerminalColor) -> Self {
        self.colors.set_color(color);
        self
    }

    #[must_use]
    pub const fn with_font_weight(mut self, font_weight: FontWeight) -> Self {
        self.font_weight = font_weight;
        self
    }

    #[must_use]
    pub fn with_font_decorations(mut self, font_decorations: Vec<FontDecorations>) -> Self {
        self.font_decorations = font_decorations;
        self
    }

    #[must_use]
    pub const fn with_pos(mut self, pos: CursorPos) -> Self {
        self.pos = pos;
        self
    }
}

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, Eq, PartialEq, Default, Copy)]
pub struct CursorPos {
    pub x: usize,
    pub y: usize,
    // pub x_as_characters: usize,
}

impl fmt::Display for CursorPos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CursorPos {{ x: {}, y: {} }}", self.x, self.y)
    }
}
