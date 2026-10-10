// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Bounded, allocation-free tokenizer for kitty-style `key=value` metadata.
//!
//! Several escape-sequence protocols (OSC 99, the kitty graphics control data,
//! iTerm2 `File=` arguments, FTCS, OSC 8 parameters) carry a list of
//! `key=value` items joined by a single separator byte. This module owns that
//! one concept: splitting such a list into items, with a cap on how many items
//! are accepted.
//!
//! The tokenizer **reports, consumers decide**. It splits each non-empty
//! segment at the first `=` into a [`KeyValueItem::Pair`] (so a value may
//! itself contain `=`, and either side may be empty), or yields a segment with
//! no `=` as [`KeyValueItem::Bare`]. It applies no policy of its own: whether a
//! bare item is an error, whether an empty key or empty value is acceptable,
//! and how wide a key may be are all decisions left to each protocol parser.
//!
//! Empty segments (leading, trailing or doubled separators) are skipped and do
//! not count toward the item cap. Once the cap is reached, the next non-empty
//! segment yields a single [`KeyValueError::TooManyItems`] and the iterator is
//! then exhausted.
//!
//! Input is treated as raw bytes and is never validated as UTF-8.

use std::iter::FusedIterator;

use thiserror::Error;

/// The byte that separates items in a `key=value` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyValueSeparator {
    /// `:` (OSC 99 metadata).
    Colon,
    /// `,` (kitty graphics control data).
    Comma,
    /// `;` (iTerm2, FTCS, OSC 8 parameters).
    Semicolon,
}

impl KeyValueSeparator {
    /// The ASCII byte this separator stands for.
    #[must_use]
    pub const fn byte(self) -> u8 {
        match self {
            Self::Colon => b':',
            Self::Comma => b',',
            Self::Semicolon => b';',
        }
    }
}

/// One non-empty segment of a `key=value` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyValueItem<'a> {
    /// A segment containing `=`, split at the first one. Either side may be
    /// empty and `value` may contain further `=` bytes.
    Pair {
        /// Bytes before the first `=`.
        key: &'a [u8],
        /// Bytes after the first `=`.
        value: &'a [u8],
    },
    /// A segment with no `=`.
    Bare(&'a [u8]),
}

/// Failure while tokenizing a `key=value` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum KeyValueError {
    /// More than `max` non-empty items were present.
    #[error("more than {max} key=value items")]
    TooManyItems {
        /// The item cap that was exceeded.
        max: usize,
    },
}

/// Iterator over the items of a `key=value` list. Created by [`tokenize`].
#[derive(Debug, Clone)]
pub struct KeyValueTokens<'a> {
    /// Unconsumed input, or `None` once the iterator is exhausted.
    rest: Option<&'a [u8]>,
    separator: u8,
    max_items: usize,
    emitted: usize,
}

/// Split `input` on `separator` into at most `max_items` items.
///
/// See the [module documentation](self) for the exact rules.
#[must_use]
pub const fn tokenize(
    input: &[u8],
    separator: KeyValueSeparator,
    max_items: usize,
) -> KeyValueTokens<'_> {
    KeyValueTokens {
        rest: Some(input),
        separator: separator.byte(),
        max_items,
        emitted: 0,
    }
}

impl<'a> Iterator for KeyValueTokens<'a> {
    type Item = Result<KeyValueItem<'a>, KeyValueError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let rest = self.rest?;
            let (segment, tail) = split_first_segment(rest, self.separator);
            self.rest = tail;

            if segment.is_empty() {
                continue;
            }

            if self.emitted >= self.max_items {
                self.rest = None;
                return Some(Err(KeyValueError::TooManyItems {
                    max: self.max_items,
                }));
            }
            self.emitted += 1;

            return Some(Ok(classify(segment)));
        }
    }
}

/// Split `input` at the first `separator`, returning the segment before it and
/// the remainder after it (`None` when no separator was found).
fn split_first_segment(input: &[u8], separator: u8) -> (&[u8], Option<&[u8]>) {
    input
        .iter()
        .position(|&b| b == separator)
        .map_or((input, None), |index| {
            let (segment, tail) = input.split_at(index);
            (segment, tail.split_first().map(|(_, after)| after))
        })
}

/// Classify a non-empty segment as a [`KeyValueItem::Pair`] (split at the
/// first `=`) or a [`KeyValueItem::Bare`].
fn classify(segment: &[u8]) -> KeyValueItem<'_> {
    segment
        .iter()
        .position(|&b| b == b'=')
        .map_or(KeyValueItem::Bare(segment), |index| {
            let (key, after) = segment.split_at(index);
            KeyValueItem::Pair {
                key,
                value: after.get(1..).unwrap_or_default(),
            }
        })
}

impl FusedIterator for KeyValueTokens<'_> {}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    type Collected<'a> = Vec<Result<KeyValueItem<'a>, KeyValueError>>;

    fn collect(input: &[u8], sep: KeyValueSeparator, max: usize) -> Collected<'_> {
        tokenize(input, sep, max).collect()
    }

    const fn pair<'a>(key: &'a [u8], value: &'a [u8]) -> KeyValueItem<'a> {
        KeyValueItem::Pair { key, value }
    }

    #[test]
    fn separator_bytes() {
        assert_eq!(KeyValueSeparator::Colon.byte(), b':');
        assert_eq!(KeyValueSeparator::Comma.byte(), b',');
        assert_eq!(KeyValueSeparator::Semicolon.byte(), b';');
    }

    #[test]
    fn each_separator_splits() {
        for (sep, input) in [
            (KeyValueSeparator::Colon, &b"a=1:b=2"[..]),
            (KeyValueSeparator::Comma, &b"a=1,b=2"[..]),
            (KeyValueSeparator::Semicolon, &b"a=1;b=2"[..]),
        ] {
            assert_eq!(
                collect(input, sep, 8),
                vec![Ok(pair(b"a", b"1")), Ok(pair(b"b", b"2"))],
                "separator {sep:?}"
            );
        }
    }

    #[test]
    fn other_separators_are_not_split_points() {
        assert_eq!(
            collect(b"a=1,b=2;c", KeyValueSeparator::Colon, 8),
            vec![Ok(pair(b"a", b"1,b=2;c"))]
        );
    }

    #[test]
    fn empty_input_yields_nothing() {
        assert_eq!(tokenize(b"", KeyValueSeparator::Colon, 8).next(), None);
    }

    #[test]
    fn only_separators_yield_nothing() {
        assert_eq!(tokenize(b":::", KeyValueSeparator::Colon, 8).next(), None);
    }

    #[test]
    fn leading_trailing_and_doubled_separators_are_skipped() {
        assert_eq!(
            collect(b";;a=1;;;b=2;;", KeyValueSeparator::Semicolon, 8),
            vec![Ok(pair(b"a", b"1")), Ok(pair(b"b", b"2"))]
        );
    }

    #[test]
    fn empty_segments_do_not_count_toward_cap() {
        assert_eq!(
            collect(b";;a=1;;;;b=2;;", KeyValueSeparator::Semicolon, 2),
            vec![Ok(pair(b"a", b"1")), Ok(pair(b"b", b"2"))]
        );
    }

    #[test]
    fn equals_inside_value_is_kept() {
        assert_eq!(
            collect(b"k=a=b==", KeyValueSeparator::Comma, 8),
            vec![Ok(pair(b"k", b"a=b=="))]
        );
    }

    #[test]
    fn empty_key() {
        assert_eq!(
            collect(b"=x", KeyValueSeparator::Comma, 8),
            vec![Ok(pair(b"", b"x"))]
        );
    }

    #[test]
    fn empty_value() {
        assert_eq!(
            collect(b"k=", KeyValueSeparator::Comma, 8),
            vec![Ok(pair(b"k", b""))]
        );
    }

    #[test]
    fn lone_equals_is_empty_pair() {
        assert_eq!(
            collect(b"=", KeyValueSeparator::Comma, 8),
            vec![Ok(pair(b"", b""))]
        );
    }

    #[test]
    fn bare_items() {
        assert_eq!(
            collect(b"flag:k=v:other", KeyValueSeparator::Colon, 8),
            vec![
                Ok(KeyValueItem::Bare(b"flag")),
                Ok(pair(b"k", b"v")),
                Ok(KeyValueItem::Bare(b"other")),
            ]
        );
    }

    #[test]
    fn multi_byte_key_is_passed_through() {
        assert_eq!(
            collect(b"abc=1", KeyValueSeparator::Comma, 8),
            vec![Ok(pair(b"abc", b"1"))]
        );
    }

    #[test]
    fn exactly_max_items_is_not_an_error() {
        assert_eq!(
            collect(b"a=1,b=2,c=3", KeyValueSeparator::Comma, 3),
            vec![
                Ok(pair(b"a", b"1")),
                Ok(pair(b"b", b"2")),
                Ok(pair(b"c", b"3")),
            ]
        );
    }

    #[test]
    fn max_plus_one_yields_one_error_then_none() {
        let mut tokens = tokenize(b"a=1,b=2,c=3,d=4,e=5", KeyValueSeparator::Comma, 3);
        assert_eq!(tokens.next(), Some(Ok(pair(b"a", b"1"))));
        assert_eq!(tokens.next(), Some(Ok(pair(b"b", b"2"))));
        assert_eq!(tokens.next(), Some(Ok(pair(b"c", b"3"))));
        assert_eq!(
            tokens.next(),
            Some(Err(KeyValueError::TooManyItems { max: 3 }))
        );
        assert_eq!(tokens.next(), None);
        assert_eq!(tokens.next(), None);
    }

    #[test]
    fn trailing_separator_after_max_is_not_an_error() {
        assert_eq!(
            collect(b"a=1,b=2,,", KeyValueSeparator::Comma, 2),
            vec![Ok(pair(b"a", b"1")), Ok(pair(b"b", b"2"))]
        );
    }

    #[test]
    fn max_items_zero_errors_on_first_item() {
        let mut tokens = tokenize(b"a=1", KeyValueSeparator::Comma, 0);
        assert_eq!(
            tokens.next(),
            Some(Err(KeyValueError::TooManyItems { max: 0 }))
        );
        assert_eq!(tokens.next(), None);
    }

    #[test]
    fn max_items_zero_with_no_items_is_empty() {
        assert_eq!(tokenize(b",,", KeyValueSeparator::Comma, 0).next(), None);
        assert_eq!(tokenize(b"", KeyValueSeparator::Comma, 0).next(), None);
    }

    #[test]
    fn non_utf8_bytes_pass_through_untouched() {
        assert_eq!(
            collect(b"\xff\xfe=\x80\x81:\xc3", KeyValueSeparator::Colon, 8),
            vec![
                Ok(pair(b"\xff\xfe", b"\x80\x81")),
                Ok(KeyValueItem::Bare(b"\xc3")),
            ]
        );
    }

    #[test]
    fn error_display() {
        assert_eq!(
            KeyValueError::TooManyItems { max: 64 }.to_string(),
            "more than 64 key=value items"
        );
    }

    fn any_separator() -> impl Strategy<Value = KeyValueSeparator> {
        prop_oneof![
            Just(KeyValueSeparator::Colon),
            Just(KeyValueSeparator::Comma),
            Just(KeyValueSeparator::Semicolon),
        ]
    }

    proptest! {
        #[test]
        fn never_panics_and_respects_bounds(
            input in proptest::collection::vec(any::<u8>(), 0..256),
            sep in any_separator(),
            max in 0usize..16,
        ) {
            let sep_byte = sep.byte();
            let items: Vec<_> = tokenize(&input, sep, max).collect();
            prop_assert!(items.len() <= max + 1);

            for (index, item) in items.iter().enumerate() {
                match item {
                    Ok(KeyValueItem::Pair { key, value }) => {
                        prop_assert!(!key.contains(&b'='));
                        prop_assert!(!key.contains(&sep_byte));
                        prop_assert!(!value.contains(&sep_byte));
                    }
                    Ok(KeyValueItem::Bare(segment)) => {
                        prop_assert!(!segment.is_empty());
                        prop_assert!(!segment.contains(&b'='));
                        prop_assert!(!segment.contains(&sep_byte));
                    }
                    Err(_) => {
                        // An error is always the final item.
                        prop_assert_eq!(index, items.len() - 1);
                        prop_assert_eq!(items.len(), max + 1);
                    }
                }
            }
        }
    }
}
