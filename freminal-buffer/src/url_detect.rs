// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Auto-detection of plain URLs (http, https, file, ftp, mailto) in terminal
//! output.
//!
//! This module is consumed by [`crate::buffer::Buffer`]'s flatten cache. For
//! each row's byte buffer (built from cell `TChar::as_bytes()` in the same
//! pass that produces `chars` and `tags`), [`find_urls_bytes`] returns the
//! byte ranges where URLs were detected. Callers translate those byte ranges
//! into character ranges via a parallel `byte_to_char` map, and splice the
//! resulting ranges into the per-row `FormatTag` vec as `FormatTag.url =
//! Some(Arc<Url>)`.
//!
//! Design decisions:
//!
//! - Byte-based regex (`regex::bytes::Regex`) avoids a `String` allocation
//!   per row. The input is already UTF-8 bytes from `TChar::as_bytes()`.
//! - A single shared `LazyLock<Regex>` keeps compilation cost off the hot
//!   path.
//! - Termination uses the common GitHub-Flavored-Markdown heuristic: match
//!   runs until a whitespace or control byte. Trailing punctuation that is
//!   almost always sentence punctuation (`.,;:!?`) and unbalanced closers
//!   (`)`, `]`, `}`, `>`) is stripped.
//! - OSC 8 precedence is enforced by the caller (flatten merge step), not
//!   here.

use regex::bytes::Regex;
use std::sync::LazyLock;

/// A detected URL range within a row's byte buffer.
///
/// Offsets are half-open: `[byte_start, byte_end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UrlMatch {
    /// Inclusive start byte offset into the row's byte buffer.
    pub byte_start: usize,
    /// Exclusive end byte offset into the row's byte buffer.
    pub byte_end: usize,
    /// `true` when the **raw regex match**, before trailing-punctuation
    /// trimming, reached the very end of `bytes`.
    ///
    /// This is the precise signal that a match *might* be a DECAWM-wrapped
    /// URL continuing onto the next physical row: wrapping only ever occurs
    /// at the row's exact last column, so a URL that got cut off by wrapping
    /// always has its raw match extend to the row's last byte — regardless
    /// of whether `trim_trailing` then stripped a few trailing punctuation
    /// bytes from the reported `byte_end`. A URL that ends naturally mid-row
    /// (followed by whitespace, a control byte, or simply the end of typed
    /// text) does not reach the buffer end and this is `false`.
    pub touches_buffer_end: bool,
}

/// Regex matching plain URLs for any of the supported schemes.
///
/// Matches as much non-whitespace/non-control text as possible after the
/// scheme. Trailing punctuation stripping is done in a post-pass so the
/// regex itself can stay simple and fast.
///
/// The pattern is a compile-time constant and is exercised by this
/// module's unit tests (`tests::plain_https` et al.). If the pattern
/// somehow fails to compile we fall back to an empty regex — effectively
/// disabling URL detection rather than panicking.
static URL_REGEX: LazyLock<Regex> = LazyLock::new(build_url_regex);

/// Build the URL-detection regex. Split out of the `LazyLock` closure so
/// its local `#[allow]` has a clear scope.
#[allow(
    clippy::expect_used,
    reason = "PATTERN is a compile-time constant exercised by unit tests; \
              fallback path is unreachable in practice but still avoids \
              panicking by returning a never-matching empty regex"
)]
fn build_url_regex() -> Regex {
    // `\S` in the bytes regex = any byte that is not ASCII whitespace. The
    // `(?-u)` flag disables Unicode-aware character classes for consistent
    // byte behaviour.
    //
    // The `[^\s\x00-\x1f\x7f]` class also excludes control characters, which
    // we never want inside a URL.
    //
    // Schemes:
    //   - http://, https://, file://, ftp://  → require `://`
    //   - mailto:                              → requires only `:`
    const PATTERN: &str = r"(?-u)(?:https?://|file://|ftp://|mailto:)[^\s\x00-\x1f\x7f]+";
    Regex::new(PATTERN).unwrap_or_else(|_| {
        // An empty regex compiles on every platform and matches nothing of
        // interest; this keeps the static initialiser infallible without a
        // runtime panic in the astronomically unlikely case the constant
        // regex fails to build.
        Regex::new("$^").expect("$^ is a trivially valid regex")
    })
}

/// Find all URL matches in `bytes`.
///
/// Returns ranges relative to `bytes`. Trailing sentence punctuation and
/// unbalanced brackets are stripped so that common patterns like
/// `see https://example.com.` yield `https://example.com` and
/// `(see https://example.com)` yields `https://example.com` (without the
/// closing paren).
#[must_use]
pub fn find_urls_bytes(bytes: &[u8]) -> Vec<UrlMatch> {
    let mut out = Vec::new();
    for m in URL_REGEX.find_iter(bytes) {
        let start = m.start();
        let raw_end = m.end();
        let end = trim_trailing(bytes, start, raw_end);
        if end > start {
            out.push(UrlMatch {
                byte_start: start,
                byte_end: end,
                touches_buffer_end: raw_end == bytes.len(),
            });
        }
    }
    out
}

/// Strip trailing characters that are almost always sentence punctuation or
/// unbalanced closing brackets from a matched URL range.
///
/// Runs right-to-left and stops as soon as a character is kept. For `)`, `]`,
/// `}`, and `>`, the character is only stripped when there is no matching
/// opener earlier in the URL (heuristic: if an URL contains more closers of
/// that kind than openers, the trailing one is presumed extraneous).
fn trim_trailing(bytes: &[u8], start: usize, mut end: usize) -> usize {
    while end > start {
        let b = bytes[end - 1];
        let strip = match b {
            // Unconditional sentence punctuation and quote/backtick
            // characters that commonly wrap URLs in prose.
            b'.' | b',' | b';' | b':' | b'!' | b'?' | b'\'' | b'"' | b'`' => true,
            // Conditional closers: strip only when unbalanced.
            b')' => unbalanced(&bytes[start..end], b'(', b')'),
            b']' => unbalanced(&bytes[start..end], b'[', b']'),
            b'}' => unbalanced(&bytes[start..end], b'{', b'}'),
            b'>' => unbalanced(&bytes[start..end], b'<', b'>'),
            _ => false,
        };
        if strip {
            end -= 1;
        } else {
            break;
        }
    }
    end
}

/// Return `true` when `slice` contains more `close` bytes than `open` bytes.
fn unbalanced(slice: &[u8], open: u8, close: u8) -> bool {
    let mut opens: usize = 0;
    let mut closes: usize = 0;
    for &b in slice {
        if b == open {
            opens += 1;
        } else if b == close {
            closes += 1;
        }
    }
    closes > opens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(s: &str) -> Vec<&str> {
        find_urls_bytes(s.as_bytes())
            .into_iter()
            .map(|m| &s[m.byte_start..m.byte_end])
            .collect()
    }

    #[test]
    fn plain_https() {
        assert_eq!(detect("https://example.com"), vec!["https://example.com"]);
    }

    #[test]
    fn plain_http() {
        assert_eq!(detect("http://example.com"), vec!["http://example.com"]);
    }

    #[test]
    fn mailto() {
        assert_eq!(detect("mailto:foo@bar.com"), vec!["mailto:foo@bar.com"]);
    }

    #[test]
    fn file_url() {
        assert_eq!(detect("file:///etc/hosts"), vec!["file:///etc/hosts"]);
    }

    #[test]
    fn ftp_url() {
        assert_eq!(
            detect("ftp://ftp.example.com/x"),
            vec!["ftp://ftp.example.com/x"]
        );
    }

    #[test]
    fn strips_trailing_period() {
        assert_eq!(
            detect("see https://example.com."),
            vec!["https://example.com"]
        );
    }

    #[test]
    fn strips_trailing_comma_and_semicolon() {
        assert_eq!(detect("a https://x.com, b"), vec!["https://x.com"]);
        assert_eq!(detect("a https://x.com; b"), vec!["https://x.com"]);
    }

    #[test]
    fn strips_unbalanced_closing_paren() {
        assert_eq!(
            detect("(see https://example.com)"),
            vec!["https://example.com"]
        );
    }

    #[test]
    fn keeps_balanced_closing_paren() {
        // Wikipedia-style URL with parens in the path.
        assert_eq!(
            detect("https://en.wikipedia.org/wiki/Foo_(bar)"),
            vec!["https://en.wikipedia.org/wiki/Foo_(bar)"]
        );
    }

    #[test]
    fn terminates_on_whitespace() {
        assert_eq!(
            detect("https://example.com and more"),
            vec!["https://example.com"]
        );
    }

    #[test]
    fn terminates_on_control_char() {
        let s = b"https://example.com\x1b[0m trailing";
        let ranges = find_urls_bytes(s);
        assert_eq!(ranges.len(), 1);
        assert_eq!(
            &s[ranges[0].byte_start..ranges[0].byte_end],
            b"https://example.com"
        );
    }

    #[test]
    fn two_urls_on_one_line() {
        assert_eq!(
            detect("see https://a.com and https://b.com for info"),
            vec!["https://a.com", "https://b.com"]
        );
    }

    #[test]
    fn url_with_query_and_fragment() {
        assert_eq!(
            detect("https://example.com/path?q=1&x=2#frag"),
            vec!["https://example.com/path?q=1&x=2#frag"]
        );
    }

    #[test]
    fn empty_input() {
        assert!(detect("").is_empty());
    }

    #[test]
    fn no_url_input() {
        assert!(detect("just a plain sentence").is_empty());
    }

    #[test]
    fn strips_multiple_trailing() {
        assert_eq!(detect("see https://x.com.,;"), vec!["https://x.com"]);
    }

    #[test]
    fn scheme_only_is_dropped() {
        // `https://` alone has nothing after the slashes, regex requires at
        // least one non-whitespace byte → no match.
        assert!(detect("https:// is not a url").is_empty());
    }

    #[test]
    fn touches_buffer_end_true_when_match_reaches_end_of_bytes() {
        let s = "https://example.com";
        let ranges = find_urls_bytes(s.as_bytes());
        assert_eq!(ranges.len(), 1);
        assert!(
            ranges[0].touches_buffer_end,
            "the match is the last thing in the buffer, so it must touch the end"
        );
    }

    #[test]
    fn touches_buffer_end_false_when_followed_by_more_text() {
        let s = "https://example.com and more";
        let ranges = find_urls_bytes(s.as_bytes());
        assert_eq!(ranges.len(), 1);
        assert!(
            !ranges[0].touches_buffer_end,
            "the match ends before trailing prose, so it must not touch the end"
        );
    }

    #[test]
    fn touches_buffer_end_true_even_when_trailing_punctuation_is_trimmed() {
        // The raw regex match ("https://example.com.") reaches the buffer
        // end even though the reported (trimmed) range does not include the
        // trailing '.'. `touches_buffer_end` must reflect the *raw* match.
        let s = "see https://example.com.";
        let ranges = find_urls_bytes(s.as_bytes());
        assert_eq!(ranges.len(), 1);
        assert_eq!(
            &s[ranges[0].byte_start..ranges[0].byte_end],
            "https://example.com"
        );
        assert!(
            ranges[0].touches_buffer_end,
            "trailing-punctuation trimming must not affect touches_buffer_end"
        );
    }
}
