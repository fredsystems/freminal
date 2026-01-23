// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::buffer_states::{
    cursor::StateColors,
    fonts::{FontDecorations, FontWeight},
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
        }
    }
}
