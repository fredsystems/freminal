// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub enum FontWeight {
    #[default]
    Normal,
    Bold,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum FontDecorations {
    Italic,
    Underline,
    Faint,
    Strikethrough,
}

/// Blink state for text rendered with SGR 5 (slow blink) or SGR 6 (fast blink).
///
/// - `None` — no blink (default).
/// - `Slow` — SGR 5: ~1 Hz (500 ms on, 500 ms off).
/// - `Fast` — SGR 6: ~3 Hz (~167 ms on, ~167 ms off).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum BlinkState {
    #[default]
    None,
    Slow,
    Fast,
}
