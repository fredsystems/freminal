// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Markup escaping for notification text.
//!
//! Notification daemons on Linux/BSD (KDE, dunst, mako) interpret a subset of
//! HTML in the body, and sometimes the summary. Text that comes from a
//! terminal program is untrusted, so every notification freminal hands to a
//! daemon — OSC 99, OSC 9, OSC 777 — is escaped with [`escape_body_markup`].
//! This lives in its own module, unconditionally compiled, because the OSC 9 /
//! OSC 777 path runs on every platform while the rest of OSC 99 presentation
//! is Linux/BSD only.

/// Zero width non-joiner, inserted after the characters kitty escapes in a
/// body for daemons that interpret markup.
const ZERO_WIDTH_NON_JOINER: char = '\u{200c}';

/// Escape `body` for a daemon that interprets markup, as kitty does: a zero
/// width non-joiner after every `<` and `&` keeps them from forming a tag or
/// an entity without visibly changing the text.
#[must_use]
pub fn escape_body_markup(body: &str) -> String {
    let mut escaped = String::with_capacity(body.len());
    for c in body.chars() {
        escaped.push(c);
        if c == '<' || c == '&' {
            escaped.push(ZERO_WIDTH_NON_JOINER);
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_escaping_inserts_a_joiner_only_after_angle_and_ampersand() {
        assert_eq!(
            escape_body_markup("a<b>&amp; c"),
            "a<\u{200c}b>&\u{200c}amp; c"
        );
        assert_eq!(escape_body_markup("plain text > 3"), "plain text > 3");
        assert_eq!(escape_body_markup(""), "");
        assert_eq!(escape_body_markup("<<"), "<\u{200c}<\u{200c}");
    }
}
