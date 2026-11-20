// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The terminfo source (`res/freminal.ti`) and a test-only parser for it.
//!
//! The table-of-truth tests compare what the entry declares with what freminal
//! sends. The encoders live in different crates (the emulator's
//! `TerminalInput::to_payload`, the GUI's keypad path), so the parsed source
//! has to be reachable from both. This module exists only under `cfg(test)` or
//! the `test-utils` feature; a production build never enables it.

// Test infrastructure: a malformed `.ti` source is a broken fixture, and
// failing the test loudly with a message is the intent, as in the other
// `test-utils` modules.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

/// The source of the entries freminal ships.
pub const FREMINAL_TI: &str = include_str!("../../res/freminal.ti");

/// Decode the escapes of one terminfo string value: `\E`, `\\`, `\,`,
/// `\:`, `\^`, `\NNN` octal and `^X` control notation.
///
/// # Panics
///
/// On a dangling `\` or `^`, an octal escape above `\377`, or non-ASCII
/// control notation.
#[must_use]
pub fn decode_terminfo_value(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let escaped = chars
                    .next()
                    .unwrap_or_else(|| panic!("dangling backslash in {raw:?}"));
                match escaped {
                    'E' | 'e' => out.push('\x1b'),
                    'n' | 'l' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    's' => out.push(' '),
                    '0'..='7' => {
                        let mut value = escaped.to_digit(8).unwrap_or_default();
                        for _ in 0..2 {
                            let Some(digit) = chars.peek().and_then(|d| d.to_digit(8)) else {
                                break;
                            };
                            value = value * 8 + digit;
                            chars.next();
                        }
                        let byte = u8::try_from(value)
                            .unwrap_or_else(|_| panic!("octal escape out of range in {raw:?}"));
                        out.push(char::from(byte));
                    }
                    other => out.push(other),
                }
            }
            '^' => {
                let control = chars
                    .next()
                    .unwrap_or_else(|| panic!("dangling caret in {raw:?}"));
                let byte = u8::try_from(control)
                    .unwrap_or_else(|_| panic!("non-ASCII control notation in {raw:?}"));
                out.push(char::from(if byte == b'?' { 0x7f } else { byte & 0x1f }));
            }
            other => out.push(other),
        }
    }
    out
}

/// Parse the entry named `term` in a terminfo source file into its name
/// field and a name to value map. Booleans map to `""`, numbers to their
/// digits and strings to their decoded value.
///
/// # Panics
///
/// When the entry is missing, a capability uses syntax this parser does not
/// understand, a capability is declared twice, or the entry has text after its
/// last comma.
#[must_use]
pub fn parse_terminfo_entry(source: &str, term: &str) -> (String, BTreeMap<String, String>) {
    let mut lines = source
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .skip_while(|line| !line.starts_with(&format!("{term}|")));
    let header = lines
        .next()
        .unwrap_or_else(|| panic!("entry {term} not found"));
    let names = header.trim_end().trim_end_matches(',').to_string();
    let body = lines.collect::<Vec<_>>().join("\n");

    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            ',' => tokens.push(std::mem::take(&mut token)),
            '\\' => {
                token.push(c);
                if let Some(escaped) = chars.next() {
                    token.push(escaped);
                }
            }
            other => token.push(other),
        }
    }
    assert!(token.trim().is_empty(), "text after the last comma");

    let mut caps = BTreeMap::new();
    for token in tokens {
        let token = token.trim_start();
        let name_len = token
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(token.len());
        let (name, rest) = token.split_at(name_len);
        let value = if rest.is_empty() {
            String::new()
        } else if let Some(number) = rest.strip_prefix('#') {
            number.to_string()
        } else if let Some(string) = rest.strip_prefix('=') {
            decode_terminfo_value(string)
        } else {
            panic!("unsupported capability syntax: {token:?}");
        };
        assert!(
            caps.insert(name.to_string(), value).is_none(),
            "capability {name} is declared twice"
        );
    }
    (names, caps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminfo_parser_decodes_escapes_and_continuations() {
        let source = "# a comment, with a comma\nfoo|f|A phrase,\n\tam, cols#80,\n\
                      \tbel=^G, kbs=\\177, del=^?, esc=\\E[0 q,\n\
                      \tcomma=a\\,b\\:c\\\\d, nul=\\007\\0338,\n# trailing comment\n\tlast=x,\n";
        let (names, caps) = parse_terminfo_entry(source, "foo");
        assert_eq!(names, "foo|f|A phrase");
        let expected = [
            ("am", ""),
            ("cols", "80"),
            ("bel", "\x07"),
            ("kbs", "\x7f"),
            ("del", "\x7f"),
            ("esc", "\x1b[0 q"),
            ("comma", "a,b:c\\d"),
            ("nul", "\x07\x1b8"),
            ("last", "x"),
        ];
        assert_eq!(caps.len(), expected.len(), "{caps:?}");
        for (name, value) in expected {
            assert_eq!(caps.get(name).map(String::as_str), Some(value), "{name}");
        }
    }

    #[test]
    fn the_shipped_source_declares_the_primary_entry_with_kent() {
        let (_, caps) = parse_terminfo_entry(FREMINAL_TI, "xterm-freminal");
        assert_eq!(caps.get("kent").map(String::as_str), Some("\x1bOM"));
    }

    #[test]
    #[should_panic(expected = "entry nope not found")]
    fn a_missing_entry_fails_loudly() {
        let _ = parse_terminfo_entry(FREMINAL_TI, "nope");
    }
}
