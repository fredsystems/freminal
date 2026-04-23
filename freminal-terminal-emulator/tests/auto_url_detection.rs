// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Integration tests for auto URL detection in terminal output.
//!
//! Covers Task 71.7b: plain URLs emitted by programs that do not use OSC 8
//! hyperlinks (cat, git log, lazygit, etc.) must be surfaced in the
//! `FormatTag.url` field so the renderer can make them clickable.
//!
//! Detection runs inside the per-row flatten cache pass in
//! `freminal-buffer::buffer::flatten`.  Detected URLs are spliced into the
//! tag sequence, splitting any covering base tag into pre/overlap/post with
//! the overlap piece inheriting visual attributes and gaining
//! `url = Some(Arc<Url>)`.  OSC 8 hyperlinks always take precedence: a base
//! tag that already carries `url.is_some()` is never overridden.

#![allow(clippy::unwrap_used)]

mod vttest_common;

use freminal_common::buffer_states::fonts::FontWeight;
use vttest_common::VtTestHelper;

/// Helper: collect all tag URL strings (deduplicated in order of first
/// appearance across visible + scrollback tag runs).
fn detected_urls(h: &mut VtTestHelper) -> Vec<String> {
    let (_chars, tags) = h.state.handler.data_and_format_data_for_gui(0);
    let mut out: Vec<String> = Vec::new();
    for tag in tags.visible.iter().chain(tags.scrollback.iter()) {
        if let Some(url) = tag.url.as_ref() {
            let s = url.url.clone();
            if !out.iter().any(|existing| existing == &s) {
                out.push(s);
            }
        }
    }
    out
}

/// A plain https URL surrounded by text is surfaced via `FormatTag.url`.
#[test]
fn https_url_in_plain_text_is_detected() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("visit https://example.com today");
    let urls = detected_urls(&mut h);
    assert_eq!(
        urls,
        vec!["https://example.com".to_owned()],
        "expected a single detected https URL"
    );
}

/// A URL followed by a sentence-terminating period must strip the period.
#[test]
fn trailing_period_is_stripped() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("See https://example.com.");
    let urls = detected_urls(&mut h);
    assert_eq!(
        urls,
        vec!["https://example.com".to_owned()],
        "trailing '.' must be stripped from the detected URL"
    );
}

/// A URL with a query string and fragment is detected in full, minus any
/// trailing sentence punctuation.
#[test]
fn url_with_query_and_fragment_is_detected() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("docs at https://example.com/path?q=1&r=2#frag now");
    let urls = detected_urls(&mut h);
    assert_eq!(
        urls,
        vec!["https://example.com/path?q=1&r=2#frag".to_owned()],
        "query + fragment must remain part of the URL"
    );
}

/// Two URLs on the same row are both detected.
#[test]
fn two_urls_on_one_row_are_both_detected() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("a https://one.example b http://two.example c");
    let urls = detected_urls(&mut h);
    assert_eq!(
        urls,
        vec![
            "https://one.example".to_owned(),
            "http://two.example".to_owned(),
        ],
        "both URLs on a single row must be detected"
    );
}

/// A mailto: URI is detected.
#[test]
fn mailto_url_is_detected() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("contact mailto:user@example.com please");
    let urls = detected_urls(&mut h);
    assert_eq!(
        urls,
        vec!["mailto:user@example.com".to_owned()],
        "mailto: scheme must be detected"
    );
}

/// Detection auto-overlay preserves the base visual attributes of the
/// formatting run (e.g., SGR bold).
#[test]
fn detected_url_preserves_underlying_formatting() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[1m"); // bold
    h.feed_str("go to https://example.com now");
    let (_chars, tags) = h.state.handler.data_and_format_data_for_gui(0);
    let overlap = tags
        .visible
        .iter()
        .find(|t| t.url.is_some())
        .expect("at least one tag must carry a detected URL");
    assert_eq!(
        overlap.font_weight,
        FontWeight::Bold,
        "auto URL overlay must inherit bold from the base tag"
    );
    assert_eq!(
        overlap
            .url
            .as_ref()
            .map(|u| u.url.as_str())
            .unwrap_or_default(),
        "https://example.com",
        "auto URL overlay must carry the detected URL string"
    );
}

/// OSC 8 explicit hyperlinks take precedence: auto-detection does not
/// override a tag that already has `url.is_some()`.
#[test]
fn osc8_hyperlink_is_not_overridden() {
    let mut h = VtTestHelper::new_default();
    // OSC 8 ; ; <uri> ST  text  OSC 8 ; ; ST
    // The visible text happens to contain an auto-detectable URL string.
    h.feed(b"\x1b]8;;https://explicit.example\x1b\\");
    h.feed_str("https://auto.example");
    h.feed(b"\x1b]8;;\x1b\\");
    let urls = detected_urls(&mut h);
    // The explicit OSC 8 URL must win; the auto-detect string must NOT
    // appear as a second detected URL over the same cells.
    assert!(
        urls.iter().any(|u| u == "https://explicit.example"),
        "explicit OSC 8 URL must be present (got {urls:?})"
    );
    assert!(
        !urls.iter().any(|u| u == "https://auto.example"),
        "auto-detect must NOT override the OSC 8 hyperlink (got {urls:?})"
    );
}

/// Disabling auto URL detection on the buffer suppresses detection for
/// subsequent flatten cache rebuilds.
#[test]
fn disabling_auto_detect_suppresses_detection() {
    let mut h = VtTestHelper::new_default();
    h.state.handler.buffer_mut().set_auto_detect_urls(false);
    h.feed_str("see https://example.com here");
    let urls = detected_urls(&mut h);
    assert!(
        urls.is_empty(),
        "auto-detect disabled must yield no detected URLs (got {urls:?})"
    );
}
