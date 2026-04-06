// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;
use std::sync::Arc;

use crate::{
    buffer_states::{
        fonts::{FontDecorationFlags, FontWeight},
        line_wrap::LineWrap,
        url::Url,
    },
    colors::TerminalColor,
};

/// Whether reverse-video mode (DECSCNM / SGR 7) is currently active.
///
/// When `On`, foreground and background colors are swapped when drawing text.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default, Hash)]
pub enum ReverseVideo {
    /// Reverse-video is enabled — foreground and background are swapped.
    On,
    /// Normal display (default).
    #[default]
    Off,
}

/// The active foreground, background, and underline colors for a cursor position,
/// together with the current reverse-video state.
///
/// All color lookups respect `reverse_video`: when `On`, `get_color` returns
/// the background and `get_background_color` returns the foreground.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct StateColors {
    /// Active text foreground color.
    pub color: TerminalColor,
    /// Active cell background color.
    pub background_color: TerminalColor,
    /// Active underline decoration color (independent of fg/bg inversion).
    pub underline_color: TerminalColor,
    /// Whether reverse-video mode is currently active for this cell.
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
    /// Create a new `StateColors` with default terminal colors.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset all colors to their terminal defaults.
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

    /// Return a copy of `self` with `color` (foreground) replaced.
    #[must_use]
    pub const fn with_color(mut self, color: TerminalColor) -> Self {
        self.color = color;
        self
    }

    /// Return a copy of `self` with `underline_color` replaced.
    #[must_use]
    pub const fn with_underline_color(mut self, underline_color: TerminalColor) -> Self {
        self.underline_color = underline_color;
        self
    }

    /// Return a copy of `self` with `reverse_video` replaced.
    #[must_use]
    pub const fn with_reverse_video(mut self, reverse_video: ReverseVideo) -> Self {
        self.reverse_video = reverse_video;
        self
    }

    /// Set the foreground color in-place.
    pub const fn set_color(&mut self, color: TerminalColor) {
        self.color = color;
    }

    /// Set the background color in-place.
    pub const fn set_background_color(&mut self, background_color: TerminalColor) {
        self.background_color = background_color;
    }

    /// Set the underline decoration color in-place.
    pub const fn set_underline_color(&mut self, underline_color: TerminalColor) {
        self.underline_color = underline_color;
    }

    /// Set the reverse-video state in-place.
    pub const fn set_reverse_video(&mut self, reverse_video: ReverseVideo) {
        self.reverse_video = reverse_video;
    }

    /// Return the effective foreground color, accounting for reverse-video.
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

    #[must_use]
    pub const fn get_underline_color(&self) -> TerminalColor {
        match self.reverse_video {
            ReverseVideo::On => {
                // An explicitly-set underline colour is independent of fg/bg inversion.
                // Only fall back to the inverted background when no underline colour was set.
                if matches!(self.underline_color, TerminalColor::DefaultUnderlineColor) {
                    self.background_color.default_to_regular()
                } else {
                    self.underline_color
                }
            }
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
    pub font_decorations: FontDecorationFlags,
    pub colors: StateColors,
    pub line_wrap_mode: LineWrap,
    pub url: Option<Arc<Url>>,
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
    pub const fn with_font_decorations(mut self, font_decorations: FontDecorationFlags) -> Self {
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
