// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::buffer_states::{
    cursor::StateColors,
    fonts::{BlinkState, FontDecorations, FontWeight},
    url::Url,
};

/// A half-open character-index range `[start, end)` with its associated
/// text format.
///
/// The `start` and `end` fields index into the flat `Vec<TChar>` produced by
/// `Buffer::visible_as_tchars_and_tags` (or its scrollback counterpart).
/// Multiple non-overlapping `FormatTag` values cover the entire flat vector;
/// together they describe all color, weight, decoration, URL, and blink state
/// changes across the visible terminal content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormatTag {
    /// Index of the first character covered by this tag (inclusive).
    pub start: usize,
    /// Index past the last character covered by this tag (exclusive).
    ///
    /// May be `usize::MAX` for an open-ended range that reaches the end of
    /// the flat character vector.
    pub end: usize,
    /// Foreground and background colors for this range.
    pub colors: StateColors,
    /// Font weight (normal or bold) for this range.
    pub font_weight: FontWeight,
    /// Active font decorations (underline, strikethrough, etc.) for this range.
    pub font_decorations: Vec<FontDecorations>,
    /// OSC 8 hyperlink URL active for this range, if any.
    pub url: Option<Url>,
    /// Text blink state (none, slow SGR 5, or fast SGR 6) for this range.
    pub blink: BlinkState,
}

impl Default for FormatTag {
    fn default() -> Self {
        Self {
            start: 0,
            end: usize::MAX,
            colors: StateColors::default(),
            font_weight: FontWeight::Normal,
            font_decorations: Vec::new(),
            url: None,
            blink: BlinkState::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_format_tag_has_no_blink() {
        let tag = FormatTag::default();
        assert_eq!(tag.blink, BlinkState::None);
    }

    #[test]
    fn format_tags_with_different_blink_are_not_equal() {
        let tag_slow = FormatTag {
            blink: BlinkState::Slow,
            ..FormatTag::default()
        };

        let tag_fast = FormatTag {
            blink: BlinkState::Fast,
            ..FormatTag::default()
        };

        let tag_none = FormatTag::default();

        assert_ne!(tag_none, tag_slow);
        assert_ne!(tag_none, tag_fast);
        assert_ne!(tag_slow, tag_fast);
    }

    #[test]
    fn format_tags_with_same_blink_are_equal() {
        let tag_a = FormatTag {
            blink: BlinkState::Slow,
            ..FormatTag::default()
        };

        let tag_b = FormatTag {
            blink: BlinkState::Slow,
            ..FormatTag::default()
        };

        assert_eq!(tag_a, tag_b);
    }

    #[test]
    fn blink_state_default_is_none() {
        assert_eq!(BlinkState::default(), BlinkState::None);
    }

    #[test]
    fn blink_state_is_copy() {
        let state = BlinkState::Slow;
        let copied = state;
        assert_eq!(state, copied);
    }
}
