// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Search-in-scrollback: text search logic and overlay UI.
//!
//! The search runs on the GUI thread against a full-buffer `TChar` corpus
//! (scrollback + visible) fetched on demand from the PTY thread via
//! `InputEvent::RequestSearchBuffer`.  The cached corpus is stored in
//! `SearchState::cached_full_buffer` and refreshed whenever `total_rows`
//! changes (indicating new PTY output).
//!
//! # Data flow
//!
//! 1. The user opens the search overlay (`Ctrl+Shift+F` → `KeyAction::OpenSearch`).
//! 2. The overlay is rendered as an `egui::Area` on top of the terminal area.
//! 3. The widget sends `InputEvent::RequestSearchBuffer` to the PTY thread,
//!    which responds with the concatenated scrollback + visible `TChar` data.
//! 4. On each frame where `SearchState::needs_refresh()` is true, `run_search()`
//!    is called against the cached full buffer and results stored in
//!    `SearchState::matches`.  Match rows are buffer-absolute.
//! 5. `matches_to_highlights()` filters to the visible window and converts
//!    rows to screen-relative for the renderer vertex builder.
//! 6. The current match scroll offset is updated by `scroll_to_match()`.

use crossbeam_channel::Sender;
use egui::{self, Align2, Area, Color32, Frame, Key, Order, Pos2, Rect, Ui};
use freminal_common::buffer_states::tchar::TChar;
use freminal_terminal_emulator::{io::InputEvent, snapshot::TerminalSnapshot};
use regex::Regex;

use super::{
    panes::PaneId,
    renderer::MatchHighlight,
    view_state::{MatchSpan, SearchState, ViewState},
};

// ---------------------------------------------------------------------------
//  Search result returned from the overlay widget
// ---------------------------------------------------------------------------

/// Action produced by the search overlay on a given frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchBarAction {
    /// No action this frame.
    None,
    /// The user pressed the close button or Escape while the search bar was focused.
    Close,
    /// Navigate to the next match.
    Next,
    /// Navigate to the previous match.
    Prev,
}

// ---------------------------------------------------------------------------
//  Core text search
// ---------------------------------------------------------------------------

/// Extract a plain `String` row from `visible_chars`, stopping at a `NewLine`
/// or the end of the slice.  Returns the string, the number of `TChar`
/// elements consumed (including the trailing `NewLine` if present), and a
/// byte-offset-to-display-column map.
///
/// The map has one entry per byte in the returned string.  `byte_to_col[i]`
/// gives the 0-indexed display column at which byte `i` starts.
fn extract_row_string(chars: &[TChar]) -> (String, usize, Vec<usize>) {
    let mut s = String::new();
    let mut byte_to_col: Vec<usize> = Vec::new();
    let mut display_col = 0usize;
    let mut consumed = 0;
    for tc in chars {
        consumed += 1;
        if matches!(tc, TChar::NewLine) {
            break;
        }
        if let Ok(text) = std::str::from_utf8(tc.as_bytes()) {
            let width = tc.display_width();
            for _ in 0..text.len() {
                byte_to_col.push(display_col);
            }
            s.push_str(text);
            display_col += width;
        }
    }
    (s, consumed, byte_to_col)
}

/// Compute the display width of a substring `s[start..end]` using the
/// byte-to-display-column map returned by `extract_row_string`.
///
/// Returns `(col_start, display_width)`.
fn byte_range_to_display_cols(
    byte_to_col: &[usize],
    row_str: &str,
    byte_start: usize,
    byte_end: usize,
) -> (usize, usize) {
    let col_start = byte_to_col.get(byte_start).copied().unwrap_or(0);
    // The display width of the match is the sum of UnicodeWidthChar widths
    // of the characters in the matched substring.
    let display_width: usize = row_str[byte_start..byte_end]
        .chars()
        .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
        .sum();
    (col_start, display_width)
}

/// Run a substring search over all rows in the provided `TChar` buffer.
///
/// Returns a `Vec<MatchSpan>` in document order (top row first, left-to-right
/// within each row).  Each span's `row` is the 0-indexed row within the input
/// buffer and `col_start`/`col_end` are display-column indices within that row
/// (wide characters such as CJK ideographs occupy two columns).
///
/// When the input is the full scrollback + visible corpus, `row` values are
/// buffer-absolute (0 = first scrollback row).
///
/// When the query is empty the result is always empty.
///
/// # Errors
///
/// When `regex_mode` is `true` and the query is not a valid regex, returns an
/// empty `Vec` (the caller displays the error via `SearchState`).
#[must_use]
pub fn run_search(
    query: &str,
    regex_mode: bool,
    visible_chars: &[TChar],
) -> (Vec<MatchSpan>, Option<String>) {
    if query.is_empty() {
        return (Vec::new(), None);
    }

    let compiled_regex = if regex_mode {
        match Regex::new(query) {
            Ok(re) => Some(re),
            Err(e) => return (Vec::new(), Some(e.to_string())),
        }
    } else {
        None
    };

    let needle_lower = query.to_ascii_lowercase();

    let mut matches = Vec::new();
    let mut row = 0usize;
    let mut remaining = visible_chars;

    while !remaining.is_empty() {
        let (row_str, consumed, byte_to_col) = extract_row_string(remaining);
        remaining = &remaining[consumed..];

        if regex_mode {
            if let Some(re) = &compiled_regex {
                for m in re.find_iter(&row_str) {
                    let (col_start, display_width) =
                        byte_range_to_display_cols(&byte_to_col, &row_str, m.start(), m.end());
                    if display_width == 0 {
                        continue;
                    }
                    matches.push(MatchSpan {
                        row,
                        col_start,
                        col_end: col_start + display_width - 1,
                    });
                }
            }
        } else {
            // Case-insensitive substring search.
            let haystack_lower = row_str.to_ascii_lowercase();
            let mut search_from = 0usize;
            while let Some(byte_pos) = haystack_lower[search_from..].find(&needle_lower) {
                let abs_byte = search_from + byte_pos;
                let match_byte_end = abs_byte + needle_lower.len();
                let (col_start, display_width) =
                    byte_range_to_display_cols(&byte_to_col, &row_str, abs_byte, match_byte_end);
                if display_width == 0 {
                    break;
                }
                matches.push(MatchSpan {
                    row,
                    col_start,
                    col_end: col_start + display_width - 1,
                });
                // Advance past this match (at least 1 byte to avoid infinite loop).
                search_from = match_byte_end.max(abs_byte + 1);
                if search_from > haystack_lower.len() {
                    break;
                }
            }
        }

        row += 1;
    }

    (matches, None)
}

/// Convert `SearchState::matches` into `MatchHighlight` instances suitable
/// for the renderer vertex builder.
///
/// Only matches whose row falls within the visible window
/// `[visible_window_start, visible_window_start + term_height)` are included.
/// Buffer-absolute rows are converted to screen-relative rows for rendering.
///
/// The current match uses `is_current = true`; all others use `is_current = false`.
#[must_use]
pub fn matches_to_highlights(
    state: &SearchState,
    visible_window_start: usize,
    term_height: usize,
) -> Vec<MatchHighlight> {
    let win_end = visible_window_start + term_height;
    state
        .matches
        .iter()
        .enumerate()
        .filter(|(_, span)| span.row >= visible_window_start && span.row < win_end)
        .map(|(i, span)| MatchHighlight {
            row: span.row - visible_window_start,
            col_start: span.col_start,
            col_end: span.col_end,
            is_current: i == state.current_match,
        })
        .collect()
}

// ---------------------------------------------------------------------------
//  Scroll-to-match
// ---------------------------------------------------------------------------

/// Adjust `view_state.scroll_offset` so that the current match row is
/// centred (or at least visible) in the viewport.
///
/// Returns `Some(new_offset)` when the scroll offset was updated (the caller
/// should send `InputEvent::ScrollOffset` to the PTY thread), or `None` when
/// no change was needed (no matches, or the offset did not change).
pub fn scroll_to_match(view_state: &mut ViewState, snap: &TerminalSnapshot) -> Option<usize> {
    let span = view_state.search_state.current()?;
    // `span.row` is buffer-absolute (0 = first scrollback row).
    let abs_row = span.row;

    // We want abs_row to be visible. Compute the scroll_offset that centres it.
    let half_height = snap.term_height / 2;
    let ideal_start = abs_row.saturating_sub(half_height);
    // The maximum valid start puts the last `term_height` rows on screen.
    let max_start = snap.total_rows.saturating_sub(snap.term_height);
    let clamped_start = ideal_start.min(max_start);
    let new_scroll_offset = max_start
        .saturating_sub(clamped_start)
        .min(snap.max_scroll_offset);

    let old = view_state.scroll_offset;
    view_state.scroll_offset = new_scroll_offset;
    if new_scroll_offset == old {
        None
    } else {
        Some(new_scroll_offset)
    }
}

/// Scroll to the current search match and, if the scroll offset changed,
/// send the new offset to the PTY thread.
///
/// This is a convenience wrapper around [`scroll_to_match`] that eliminates
/// the repeated `if let Some(offset) … send(ScrollOffset)` pattern at every
/// call-site.
pub fn scroll_to_match_and_send(
    view_state: &mut ViewState,
    snap: &TerminalSnapshot,
    input_tx: &Sender<InputEvent>,
) {
    if let Some(offset) = scroll_to_match(view_state, snap)
        && let Err(e) = input_tx.send(InputEvent::ScrollOffset(offset))
    {
        error!("Failed to send scroll offset to PTY: {e}");
    }
}

// ---------------------------------------------------------------------------
//  Command-boundary jump
// ---------------------------------------------------------------------------

/// Jump to the previous command boundary (OSC 133 prompt start).
///
/// Searches `snap.prompt_rows` for the highest prompt row that is above the
/// current visible window top, then scrolls to place that row near the top
/// of the viewport.
///
/// Returns `Some(new_scroll_offset)` if the scroll offset changed, `None`
/// otherwise.
pub fn jump_to_prev_command(view_state: &mut ViewState, snap: &TerminalSnapshot) -> Option<usize> {
    if snap.prompt_rows.is_empty() || snap.total_rows <= snap.term_height {
        return None;
    }

    let max_start = snap.total_rows.saturating_sub(snap.term_height);
    // Use snap.scroll_offset (source of truth matching snap.total_rows).
    let window_start = max_start.saturating_sub(snap.scroll_offset);

    // Find the last prompt row strictly above the current window start.
    let target = snap.prompt_rows.iter().rev().find(|&&r| r < window_start)?;

    let new_start = (*target).min(max_start);
    let new_scroll_offset = max_start
        .saturating_sub(new_start)
        .min(snap.max_scroll_offset);

    let old = snap.scroll_offset;
    view_state.scroll_offset = new_scroll_offset;
    if new_scroll_offset == old {
        None
    } else {
        Some(new_scroll_offset)
    }
}

/// Jump to the next command boundary (OSC 133 prompt start).
///
/// Searches `snap.prompt_rows` for the lowest prompt row that is below the
/// current visible window top, then scrolls to place that row near the top
/// of the viewport.
///
/// Returns `Some(new_scroll_offset)` if the scroll offset changed, `None`
/// otherwise.
pub fn jump_to_next_command(view_state: &mut ViewState, snap: &TerminalSnapshot) -> Option<usize> {
    if snap.prompt_rows.is_empty() || snap.total_rows <= snap.term_height {
        return None;
    }

    let max_start = snap.total_rows.saturating_sub(snap.term_height);
    let window_start = max_start.saturating_sub(snap.scroll_offset);

    // Find the first prompt row strictly after the current window start.
    let target = snap.prompt_rows.iter().find(|&&r| r > window_start)?;

    let new_start = (*target).min(max_start);
    let new_scroll_offset = max_start
        .saturating_sub(new_start)
        .min(snap.max_scroll_offset);

    let old = snap.scroll_offset;
    view_state.scroll_offset = new_scroll_offset;
    if new_scroll_offset == old {
        None
    } else {
        Some(new_scroll_offset)
    }
}

// ---------------------------------------------------------------------------
//  Overlay UI
// ---------------------------------------------------------------------------

/// Show the search overlay bar and return the action the user triggered.
///
/// The overlay is rendered as a floating `egui::Area` at the top-right
/// corner of `terminal_rect`.  It handles its own keyboard input (Enter,
/// Shift+Enter, Escape) so the caller does not need to intercept those keys
/// separately.
///
/// The function also updates `view_state.search_state.query` in response to
/// text-field input, but does NOT run the actual search — that is handled by
/// the caller so it can be deferred or run on a changed-query signal.
pub fn show_search_bar(
    ui: &mut Ui,
    view_state: &mut ViewState,
    terminal_rect: Rect,
    error_msg: Option<&str>,
    pane_id: PaneId,
) -> SearchBarAction {
    let match_count = view_state.search_state.matches.len();
    let current = if match_count > 0 {
        view_state.search_state.current_match + 1
    } else {
        0
    };

    // Anchor the search bar to the top-right corner of the pane's terminal area.
    // Use pivot(RIGHT_TOP) so that fixed_pos refers to the Area's right-top corner,
    // not its top-left.  Do NOT use .anchor() — it overrides fixed_pos and positions
    // relative to the full window rect, ignoring pane boundaries.
    let anchor_pos = Pos2::new(terminal_rect.right() - 4.0, terminal_rect.top() + 4.0);

    let mut action = SearchBarAction::None;

    Area::new(egui::Id::new("search_overlay").with(pane_id))
        .order(Order::Foreground)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(anchor_pos)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            Frame::popup(ui.style())
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.set_min_width(260.0);

                    // ── Row 1: text input + control buttons ──────────────
                    ui.horizontal(|ui| {
                        // Text input.
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut view_state.search_state.query)
                                .hint_text("Search…")
                                .desired_width(180.0)
                                .lock_focus(true),
                        );

                        // Handle Enter / Shift+Enter inside the text field.
                        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            if ui.input(|i| i.modifiers.shift) {
                                action = SearchBarAction::Prev;
                            } else {
                                action = SearchBarAction::Next;
                            }
                        }

                        // Handle Escape inside the text field.
                        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                            action = SearchBarAction::Close;
                        }

                        // Always request focus when the search bar is open so the
                        // user can start typing immediately.
                        if !response.has_focus() {
                            response.request_focus();
                        }

                        // Match counter.
                        ui.label(if match_count == 0 {
                            if view_state.search_state.query.is_empty() {
                                String::new()
                            } else {
                                "No matches".to_string()
                            }
                        } else {
                            format!("{current}/{match_count}")
                        });

                        // ← Prev button.
                        if ui.button("<").clicked() {
                            action = SearchBarAction::Prev;
                        }
                        // → Next button.
                        if ui.button(">").clicked() {
                            action = SearchBarAction::Next;
                        }
                        // Close button.
                        if ui.button("X").clicked() {
                            action = SearchBarAction::Close;
                        }
                    });

                    // ── Row 2: regex toggle + error ───────────────────────
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut view_state.search_state.regex_mode, "Regex");
                        if let Some(err) = error_msg {
                            ui.colored_label(Color32::from_rgb(255, 80, 80), err);
                        }
                    });
                });
        });

    // Also allow Escape at the window level (in case the text field doesn't have focus).
    if action == SearchBarAction::None && ui.input(|i| i.key_pressed(Key::Escape)) {
        action = SearchBarAction::Close;
    }

    action
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use freminal_common::buffer_states::tchar::TChar;

    /// Build a `Vec<TChar>` from a slice of row strings.
    fn make_chars(rows: &[&str]) -> Vec<TChar> {
        let mut chars = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            for c in row.chars() {
                chars.push(TChar::from(c));
            }
            if i + 1 < rows.len() {
                chars.push(TChar::NewLine);
            }
        }
        chars
    }

    // ── run_search: substring ──────────────────────────────────────────────

    #[test]
    fn search_empty_query_returns_no_matches() {
        let chars = make_chars(&["hello world"]);
        let (matches, err) = run_search("", false, &chars);
        assert!(matches.is_empty());
        assert!(err.is_none());
    }

    #[test]
    fn search_single_match_on_first_row() {
        let chars = make_chars(&["hello world"]);
        let (matches, err) = run_search("hello", false, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].row, 0);
        assert_eq!(matches[0].col_start, 0);
        assert_eq!(matches[0].col_end, 4); // "hello" = cols 0-4
    }

    #[test]
    fn search_match_in_middle_of_row() {
        let chars = make_chars(&["abc foo bar"]);
        let (matches, err) = run_search("foo", false, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].col_start, 4);
        assert_eq!(matches[0].col_end, 6);
    }

    #[test]
    fn search_multiple_matches_same_row() {
        let chars = make_chars(&["abcabc"]);
        let (matches, err) = run_search("abc", false, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].col_start, 0);
        assert_eq!(matches[1].col_start, 3);
    }

    #[test]
    fn search_matches_across_rows() {
        let chars = make_chars(&["foo bar", "baz foo"]);
        let (matches, err) = run_search("foo", false, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].row, 0);
        assert_eq!(matches[1].row, 1);
    }

    #[test]
    fn search_case_insensitive() {
        let chars = make_chars(&["Hello WORLD"]);
        let (matches, err) = run_search("hello", false, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].col_end, 4);
    }

    #[test]
    fn search_no_match_returns_empty() {
        let chars = make_chars(&["hello world"]);
        let (matches, err) = run_search("xyz", false, &chars);
        assert!(err.is_none());
        assert!(matches.is_empty());
    }

    #[test]
    fn search_after_wide_char_uses_display_columns() {
        // U+4E16 (世) and U+754C (界) are each 2 display columns wide.
        // "世界hi" → display columns: 世=0-1, 界=2-3, h=4, i=5
        let chars = make_chars(&["世界hi"]);
        let (matches, err) = run_search("hi", false, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].col_start, 4);
        assert_eq!(matches[0].col_end, 5);
    }

    // ── run_search: regex ──────────────────────────────────────────────────

    #[test]
    fn search_regex_basic_match() {
        let chars = make_chars(&["foo123bar"]);
        let (matches, err) = run_search(r"\d+", true, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].col_start, 3);
        assert_eq!(matches[0].col_end, 5); // "123" = cols 3-5
    }

    #[test]
    fn search_invalid_regex_returns_error() {
        let chars = make_chars(&["hello"]);
        let (matches, err) = run_search(r"[invalid", true, &chars);
        assert!(matches.is_empty());
        assert!(err.is_some());
    }

    #[test]
    fn search_regex_no_match_returns_empty() {
        let chars = make_chars(&["hello"]);
        let (matches, err) = run_search(r"\d+", true, &chars);
        assert!(err.is_none());
        assert!(matches.is_empty());
    }

    // ── SearchState navigation ─────────────────────────────────────────────

    #[test]
    fn next_match_wraps_around() {
        let mut state = SearchState {
            matches: vec![
                MatchSpan {
                    row: 0,
                    col_start: 0,
                    col_end: 2,
                },
                MatchSpan {
                    row: 1,
                    col_start: 0,
                    col_end: 2,
                },
            ],
            current_match: 1,
            ..SearchState::default()
        };
        state.next_match();
        assert_eq!(state.current_match, 0, "should wrap to 0");
    }

    #[test]
    fn prev_match_wraps_around() {
        let mut state = SearchState {
            matches: vec![
                MatchSpan {
                    row: 0,
                    col_start: 0,
                    col_end: 2,
                },
                MatchSpan {
                    row: 1,
                    col_start: 0,
                    col_end: 2,
                },
            ],
            ..SearchState::default()
        };
        state.prev_match();
        assert_eq!(state.current_match, 1, "should wrap to last");
    }

    #[test]
    fn next_match_no_op_when_empty() {
        let mut state = SearchState::default();
        state.next_match();
        assert_eq!(state.current_match, 0);
    }

    #[test]
    fn prev_match_no_op_when_empty() {
        let mut state = SearchState::default();
        state.prev_match();
        assert_eq!(state.current_match, 0);
    }

    #[test]
    fn needs_refresh_true_when_query_changed() {
        let visible = Arc::new(make_chars(&["hello"]));
        let state = SearchState {
            query: "foo".to_string(),
            cached_full_buffer: Some(visible),
            ..SearchState::default()
        };
        assert!(state.needs_refresh());
    }

    #[test]
    fn needs_refresh_false_after_mark_fresh() {
        let visible = Arc::new(make_chars(&["hello"]));
        let mut state = SearchState {
            query: "foo".to_string(),
            cached_full_buffer: Some(visible),
            ..SearchState::default()
        };
        state.mark_fresh();
        assert!(!state.needs_refresh());
    }

    #[test]
    fn needs_refresh_true_when_no_cached_buffer() {
        let state = SearchState {
            query: "foo".to_string(),
            last_searched_query: "foo".to_string(),
            ..SearchState::default()
        };
        // No cached buffer → always needs refresh.
        assert!(state.needs_refresh());
    }

    #[test]
    fn close_resets_state() {
        let visible = Arc::new(make_chars(&["foo"]));
        let mut state = SearchState {
            is_open: true,
            query: "foo".to_string(),
            matches: vec![MatchSpan {
                row: 0,
                col_start: 0,
                col_end: 2,
            }],
            current_match: 0,
            regex_mode: true,
            last_searched_query: "foo".to_string(),
            last_searched_regex: true,
            cached_full_buffer: Some(visible),
            last_known_total_rows: 10,
            buffer_request_state: crate::gui::view_state::BufferRequestState::Idle,
        };
        state.close();
        assert!(!state.is_open);
        assert!(state.matches.is_empty());
        assert_eq!(state.current_match, 0);
        assert!(state.last_searched_query.is_empty());
        assert!(!state.last_searched_regex);
        assert!(state.cached_full_buffer.is_none());
        assert_eq!(state.last_known_total_rows, 0);
        assert_eq!(
            state.buffer_request_state,
            crate::gui::view_state::BufferRequestState::Idle
        );
    }

    // ── matches_to_highlights ──────────────────────────────────────────────

    #[test]
    fn highlights_marks_current_match() {
        let state = SearchState {
            matches: vec![
                MatchSpan {
                    row: 0,
                    col_start: 0,
                    col_end: 2,
                },
                MatchSpan {
                    row: 1,
                    col_start: 0,
                    col_end: 2,
                },
            ],
            current_match: 1,
            ..SearchState::default()
        };
        // Both matches are within the visible window [0, 10).
        let highlights = matches_to_highlights(&state, 0, 10);
        assert_eq!(highlights.len(), 2);
        assert!(!highlights[0].is_current);
        assert!(highlights[1].is_current);
    }

    #[test]
    fn highlights_filters_to_visible_window() {
        let state = SearchState {
            matches: vec![
                MatchSpan {
                    row: 5,
                    col_start: 0,
                    col_end: 2,
                },
                MatchSpan {
                    row: 15,
                    col_start: 0,
                    col_end: 3,
                },
                MatchSpan {
                    row: 25,
                    col_start: 1,
                    col_end: 4,
                },
            ],
            current_match: 1,
            ..SearchState::default()
        };
        // Visible window: rows [10, 20). Only match at row 15 is visible.
        let highlights = matches_to_highlights(&state, 10, 10);
        assert_eq!(highlights.len(), 1);
        assert_eq!(highlights[0].row, 5); // 15 - 10 = screen row 5
        assert!(highlights[0].is_current); // match index 1 is current
    }

    #[test]
    fn highlights_converts_absolute_to_screen_relative() {
        let state = SearchState {
            matches: vec![MatchSpan {
                row: 100,
                col_start: 3,
                col_end: 7,
            }],
            current_match: 0,
            ..SearchState::default()
        };
        // Visible window starts at row 90, height 24.
        let highlights = matches_to_highlights(&state, 90, 24);
        assert_eq!(highlights.len(), 1);
        assert_eq!(highlights[0].row, 10); // 100 - 90 = screen row 10
        assert_eq!(highlights[0].col_start, 3);
        assert_eq!(highlights[0].col_end, 7);
    }
}
