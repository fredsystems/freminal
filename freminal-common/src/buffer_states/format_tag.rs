// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::buffer_states::{
    cursor::StateColors,
    fonts::{BlinkState, FontDecorations, FontWeight},
    url::Url,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormatTag {
    // FIXME: The start and end are irrelevant once we move to the line buffer
    pub start: usize,
    pub end: usize,
    pub colors: StateColors,
    pub font_weight: FontWeight,
    pub font_decorations: Vec<FontDecorations>,
    pub url: Option<Url>,
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
