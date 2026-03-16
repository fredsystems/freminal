// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Tests for the UTF-8 tail-scan split logic in `TerminalState::handle_incoming_data`.
//!
//! The logic scans at most the last 3 bytes of the incoming buffer looking for a
//! leading byte of a multi-byte UTF-8 sequence whose declared length extends past
//! the end of the buffer.  When found, those bytes are split off into
//! `leftover_data` and prepended on the next call.
//!
//! Each test constructs its own `TerminalState` — tests are hermetic and
//! order-independent.

#![allow(clippy::unwrap_used)]

use crossbeam_channel::unbounded;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::state::internal::TerminalState;

// ─── helpers ────────────────────────────────────────────────────────────────

/// Construct a headless `TerminalState` together with the PTY-write receiver.
///
/// Keep `_rx` alive so the channel is never disconnected (a disconnected channel
/// causes the handler to log errors on write-back attempts, which could mask real
/// test failures).
fn make_state() -> (TerminalState, crossbeam_channel::Receiver<PtyWrite>) {
    let (tx, rx) = unbounded::<PtyWrite>();
    let state = TerminalState::new(tx, None);
    (state, rx)
}

/// Extract the visible buffer content as a plain `String` by walking every row
/// and every cell.  Trailing whitespace on each row is stripped; rows are joined
/// with a single space so that multi-row content can be matched with a simple
/// `contains()`.
fn visible_text(state: &mut TerminalState) -> String {
    let rows = state.handler.buffer().get_rows();
    let mut parts: Vec<String> = Vec::new();
    for row in rows {
        let mut row_str = String::new();
        for cell in row.cells() {
            row_str.push_str(&cell.into_utf8());
        }
        let trimmed = row_str.trim_end().to_string();
        if !trimmed.is_empty() {
            parts.push(trimmed);
        }
    }
    parts.join(" ")
}

// ─── 1. Pure ASCII — no leftover ─────────────────────────────────────────────

#[test]
fn test_pure_ascii_no_leftover() {
    let (mut state, _rx) = make_state();

    state.handle_incoming_data(b"hello world");

    assert!(
        state.leftover_data.is_none(),
        "pure ASCII must leave no leftover"
    );
    let text = visible_text(&mut state);
    assert!(
        text.contains("hello world"),
        "buffer must contain 'hello world', got: {text:?}"
    );
}

// ─── 2. Complete 2-byte sequence — no leftover ───────────────────────────────

#[test]
fn test_complete_2byte_no_leftover() {
    let (mut state, _rx) = make_state();

    // 'é' = U+00E9 → 0xC3 0xA9
    state.handle_incoming_data("é".as_bytes());

    assert!(
        state.leftover_data.is_none(),
        "complete 2-byte sequence must leave no leftover"
    );
    let text = visible_text(&mut state);
    assert!(text.contains('é'), "buffer must contain 'é', got: {text:?}");
}

// ─── 3. 2-byte sequence split after first byte ───────────────────────────────

#[test]
fn test_2byte_split_first_byte() {
    let (mut state, _rx) = make_state();

    // Feed only the leading byte of 'é' (0xC3).
    state.handle_incoming_data(&[0xC3]);

    assert_eq!(
        state.leftover_data,
        Some(vec![0xC3]),
        "leading byte 0xC3 alone must be held as leftover"
    );

    // Feed the continuation byte (0xA9).
    state.handle_incoming_data(&[0xA9]);

    assert!(
        state.leftover_data.is_none(),
        "leftover must be cleared after completing the sequence"
    );
    let text = visible_text(&mut state);
    assert!(
        text.contains('é'),
        "buffer must contain 'é' after reassembly, got: {text:?}"
    );
}

// ─── 4. 3-byte sequence split after first byte ───────────────────────────────

#[test]
fn test_3byte_split_after_first() {
    let (mut state, _rx) = make_state();

    // '€' = U+20AC → 0xE2 0x82 0xAC
    state.handle_incoming_data(&[0xE2]);

    assert_eq!(
        state.leftover_data,
        Some(vec![0xE2]),
        "leading byte 0xE2 alone must be held as leftover"
    );

    state.handle_incoming_data(&[0x82, 0xAC]);

    assert!(
        state.leftover_data.is_none(),
        "leftover must be cleared after completing the 3-byte sequence"
    );
    let text = visible_text(&mut state);
    assert!(
        text.contains('€'),
        "buffer must contain '€' after reassembly, got: {text:?}"
    );
}

// ─── 5. 3-byte sequence split after second byte ──────────────────────────────

#[test]
fn test_3byte_split_after_second() {
    let (mut state, _rx) = make_state();

    // '€' = 0xE2 0x82 0xAC — send first two bytes
    state.handle_incoming_data(&[0xE2, 0x82]);

    assert_eq!(
        state.leftover_data,
        Some(vec![0xE2, 0x82]),
        "first two bytes of '€' must be held as leftover"
    );

    state.handle_incoming_data(&[0xAC]);

    assert!(
        state.leftover_data.is_none(),
        "leftover must be cleared after the final byte"
    );
    let text = visible_text(&mut state);
    assert!(
        text.contains('€'),
        "buffer must contain '€' after reassembly, got: {text:?}"
    );
}

// ─── 6. 4-byte sequence split after first byte ───────────────────────────────

#[test]
fn test_4byte_split_after_first() {
    let (mut state, _rx) = make_state();

    // '😀' = U+1F600 → 0xF0 0x9F 0x98 0x80
    state.handle_incoming_data(&[0xF0]);

    assert_eq!(
        state.leftover_data,
        Some(vec![0xF0]),
        "leading byte 0xF0 alone must be held as leftover"
    );

    state.handle_incoming_data(&[0x9F, 0x98, 0x80]);

    assert!(
        state.leftover_data.is_none(),
        "leftover must be cleared after completing the 4-byte sequence"
    );
    let text = visible_text(&mut state);
    assert!(
        text.contains('😀'),
        "buffer must contain '😀' after reassembly, got: {text:?}"
    );
}

// ─── 7. 4-byte sequence split after second byte ──────────────────────────────

#[test]
fn test_4byte_split_after_second() {
    let (mut state, _rx) = make_state();

    // '😀' = 0xF0 0x9F 0x98 0x80
    state.handle_incoming_data(&[0xF0, 0x9F]);

    assert_eq!(
        state.leftover_data,
        Some(vec![0xF0, 0x9F]),
        "first two bytes of '😀' must be held as leftover"
    );

    state.handle_incoming_data(&[0x98, 0x80]);

    assert!(
        state.leftover_data.is_none(),
        "leftover must be cleared after completing the 4-byte sequence"
    );
    let text = visible_text(&mut state);
    assert!(
        text.contains('😀'),
        "buffer must contain '😀' after reassembly, got: {text:?}"
    );
}

// ─── 8. 4-byte sequence split after third byte ───────────────────────────────

#[test]
fn test_4byte_split_after_third() {
    let (mut state, _rx) = make_state();

    // '😀' = 0xF0 0x9F 0x98 0x80
    state.handle_incoming_data(&[0xF0, 0x9F, 0x98]);

    assert_eq!(
        state.leftover_data,
        Some(vec![0xF0, 0x9F, 0x98]),
        "first three bytes of '😀' must be held as leftover"
    );

    state.handle_incoming_data(&[0x80]);

    assert!(
        state.leftover_data.is_none(),
        "leftover must be cleared after the final byte"
    );
    let text = visible_text(&mut state);
    assert!(
        text.contains('😀'),
        "buffer must contain '😀' after reassembly, got: {text:?}"
    );
}

// ─── 9. 4-byte sequence fed one byte at a time ───────────────────────────────

#[test]
fn test_4byte_one_byte_at_a_time() {
    let (mut state, _rx) = make_state();

    // '😀' = 0xF0 0x9F 0x98 0x80 — feed each byte separately
    state.handle_incoming_data(&[0xF0]);
    assert!(
        state.leftover_data.is_some(),
        "after byte 1/4 leftover must be Some"
    );

    state.handle_incoming_data(&[0x9F]);
    assert!(
        state.leftover_data.is_some(),
        "after byte 2/4 leftover must be Some"
    );

    state.handle_incoming_data(&[0x98]);
    assert!(
        state.leftover_data.is_some(),
        "after byte 3/4 leftover must be Some"
    );

    state.handle_incoming_data(&[0x80]);
    assert!(
        state.leftover_data.is_none(),
        "after byte 4/4 leftover must be None (sequence complete)"
    );

    let text = visible_text(&mut state);
    assert!(
        text.contains('😀'),
        "buffer must contain '😀' after one-byte-at-a-time delivery, got: {text:?}"
    );
}

// ─── 10. Mixed ASCII + split at buffer boundary ──────────────────────────────

#[test]
fn test_mixed_ascii_plus_split() {
    let (mut state, _rx) = make_state();

    // Feed "hello" followed by only the leading byte of 'é'.
    // The buffer is: b"hello\xC3"
    let mut buf = b"hello".to_vec();
    buf.push(0xC3);
    state.handle_incoming_data(&buf);

    assert_eq!(
        state.leftover_data,
        Some(vec![0xC3]),
        "0xC3 at end of mixed buffer must be held as leftover"
    );

    // "hello" must already be visible.
    let text_after_first = visible_text(&mut state);
    assert!(
        text_after_first.contains("hello"),
        "buffer must contain 'hello' after first feed, got: {text_after_first:?}"
    );

    // Feed continuation + rest of the string.
    let mut rest = vec![0xA9u8];
    rest.extend_from_slice(b" world");
    state.handle_incoming_data(&rest);

    assert!(
        state.leftover_data.is_none(),
        "leftover must be cleared after completing 'é'"
    );

    let text = visible_text(&mut state);
    assert!(
        text.contains('é'),
        "buffer must contain 'é' after second feed, got: {text:?}"
    );
    assert!(
        text.contains("world"),
        "buffer must contain 'world' after second feed, got: {text:?}"
    );
}

// ─── 11. Complete multi-byte string — no split ───────────────────────────────

#[test]
fn test_complete_multibyte_no_split() {
    let (mut state, _rx) = make_state();

    // "héllo" — the 'é' is a complete 2-byte sequence in the middle.
    state.handle_incoming_data("héllo".as_bytes());

    assert!(
        state.leftover_data.is_none(),
        "complete UTF-8 string must leave no leftover"
    );
    let text = visible_text(&mut state);
    assert!(
        text.contains('h') && text.contains('é') && text.contains("llo"),
        "buffer must contain 'héllo', got: {text:?}"
    );
}

// ─── 12. Empty input ─────────────────────────────────────────────────────────

#[test]
fn test_empty_input() {
    let (mut state, _rx) = make_state();

    // Record a baseline of the visible content (empty terminal).
    let before = visible_text(&mut state);

    state.handle_incoming_data(b"");

    assert!(
        state.leftover_data.is_none(),
        "empty input must leave no leftover"
    );

    let after = visible_text(&mut state);
    assert_eq!(before, after, "empty input must not change buffer contents");
}
