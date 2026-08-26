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
use egui::{self, Align2, Area, Color32, Frame, Key, Order, Pos2, Rect, Shadow, Ui};
use freminal_common::buffer_states::tchar::TChar;
use freminal_terminal_emulator::{io::InputEvent, snapshot::TerminalSnapshot};
use regex::Regex;

use super::hover_cursor::HoverAffordance;
use super::{
    panes::PaneId,
    renderer::MatchHighlight,
    view_state::{MatchSpan, SearchState, ViewState},
};

// ---------------------------------------------------------------------------
//  Search result returned from the overlay widget
// ---------------------------------------------------------------------------

/// Action produced by the search overlay on a given frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Safety classification for a search bar's actual paint bounds this frame
/// (Task 124.14d).
///
/// [`Self::Bounded`] means every pixel [`show_search_bar`] painted this
/// frame stayed inside [`SearchBarFrame::paint_rect`] -- the caller may
/// treat the previous and current frame's rect as the search overlay's
/// complete damage, and does not need to force the whole window `Full`
/// while search is open. [`Self::TooltipMayEscape`] means a tooltip-bearing
/// control (Prev, Next, Close, or the match-case checkbox) is hovered this
/// frame, so egui's hover-delay tooltip may paint outside `paint_rect` once
/// it appears.
///
/// This may over-report during the tooltip delay (safe: a needless repaint
/// of a small region for a few frames); assuming a tooltip bound that does
/// not actually exist would under-report (a stale tooltip fragment left on
/// screen with no later event to correct it) -- the same asymmetry
/// `frame_dirty::ChangedRows` documents for row-level damage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOverlaySafety {
    /// The bar's chrome painted only inside `paint_rect` this frame.
    Bounded,
    /// A tooltip-bearing control is hovered; chrome may escape `paint_rect`.
    TooltipMayEscape,
}

impl SearchOverlaySafety {
    /// Conservatively combine independent search-overlay paint sources.
    /// Any source whose tooltip may escape makes the whole overlay
    /// unbounded for this frame.
    #[must_use]
    pub(super) const fn combine(self, other: Self) -> Self {
        if matches!(self, Self::TooltipMayEscape) || matches!(other, Self::TooltipMayEscape) {
            Self::TooltipMayEscape
        } else {
            Self::Bounded
        }
    }
}

/// What [`show_search_bar`] actually drew this frame (Task 124.14d).
///
/// Replaces the bare [`SearchBarAction`] the function used to return, so the
/// caller can build the search overlay's exact old/new damage instead of
/// treating every open-search frame as globally damaging.
#[derive(Debug, Clone, Copy)]
pub struct SearchBarFrame {
    /// The action the user triggered this frame, if any.
    pub action: SearchBarAction,
    /// The bar's actual paint bounds this frame, in the same logical-point
    /// coordinate space as the `terminal_rect` passed in -- the
    /// `egui::Area`'s own response rect, expanded by the popup frame's
    /// drop-shadow margin ([`Shadow::margin`]), not a guessed constant. See
    /// [`expand_by_shadow_margin`].
    pub paint_rect: Rect,
    /// Whether this frame's chrome is safely bounded to `paint_rect`.
    pub safety: SearchOverlaySafety,
}

/// Expand `area_rect` by a popup frame's drop-shadow margin (Task 124.14d).
///
/// Pure and free of any `Ui`/`Context`, so the shadow-margin arithmetic is
/// unit-testable directly against an asymmetric [`Shadow`] -- proving all
/// four margins independently -- rather than only reachable through a live
/// `show_search_bar` call, where a symmetric default shadow would not
/// distinguish this from a hand-rolled constant.
#[must_use]
fn expand_by_shadow_margin(area_rect: Rect, shadow: Shadow) -> Rect {
    area_rect + shadow.margin()
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
/// `case_sensitive` controls whether the substring match is compared
/// verbatim (`true`) or after ASCII-lowercase folding (`false`). In
/// regex mode, `case_sensitive = false` prepends `(?i)` to the pattern.
///
/// # Errors
///
/// When `regex_mode` is `true` and the query is not a valid regex, returns an
/// empty `Vec` (the caller displays the error via `SearchState`).
#[must_use]
pub fn run_search(
    query: &str,
    regex_mode: bool,
    case_sensitive: bool,
    visible_chars: &[TChar],
) -> (Vec<MatchSpan>, Option<String>) {
    if query.is_empty() {
        return (Vec::new(), None);
    }

    let compiled_regex = if regex_mode {
        // For case-insensitive regex, prepend the `(?i)` inline flag.
        // The regex engine's ASCII-only matching semantics still apply
        // to the non-literal parts of the pattern.
        let effective_pattern = if case_sensitive {
            query.to_string()
        } else {
            format!("(?i){query}")
        };
        match Regex::new(&effective_pattern) {
            Ok(re) => Some(re),
            Err(e) => return (Vec::new(), Some(e.to_string())),
        }
    } else {
        None
    };

    let needle_folded = if case_sensitive {
        query.to_string()
    } else {
        query.to_ascii_lowercase()
    };

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
            // Substring search. In case-insensitive mode both needle and
            // haystack are ASCII-lowercased before comparison; otherwise
            // the raw strings are compared directly.
            let haystack_owned;
            let haystack_ref: &str = if case_sensitive {
                &row_str
            } else {
                haystack_owned = row_str.to_ascii_lowercase();
                &haystack_owned
            };
            let mut search_from = 0usize;
            while let Some(byte_pos) = haystack_ref[search_from..].find(&needle_folded) {
                let abs_byte = search_from + byte_pos;
                let match_byte_end = abs_byte + needle_folded.len();
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
                if search_from > haystack_ref.len() {
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
        && let Err(e) = input_tx.send(crate::gui::terminal::input::scroll_event(
            snap,
            &view_state.folded_blocks,
            offset,
        ))
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

/// Row 1 of the search bar: the text-input field, match counter, and the
/// Prev/Next/Close buttons. Extracted from [`show_search_bar`] to keep that
/// function under clippy's `too_many_lines` threshold; not meaningfully
/// reusable or testable on its own (it needs a live `Ui`), so this is a
/// pure decomposition, not a new concept.
///
/// Returns the action triggered this frame (if any) and the paint-safety
/// classification of the tooltip-bearing controls (Prev, Next, or Close;
/// Task 124.14d).
fn show_search_bar_row_1(
    ui: &mut Ui,
    view_state: &mut ViewState,
    match_count: usize,
    current: usize,
) -> (SearchBarAction, SearchOverlaySafety) {
    let mut action = SearchBarAction::None;
    let mut safety = SearchOverlaySafety::Bounded;

    ui.horizontal(|ui| {
        // When the user has typed a query but no matches were found,
        // tint the text-edit background red so the empty-result state
        // is visible even without reading the match-count label.
        let no_matches = !view_state.search_state.query.is_empty() && match_count == 0;
        // Tint from the palette error color so the no-match
        // state follows the theme (no hard-coded red).
        let error_color = ui.visuals().error_fg_color;
        let no_match_bg =
            Color32::from_rgba_unmultiplied(error_color.r(), error_color.g(), error_color.b(), 48);
        let mut text_edit = egui::TextEdit::singleline(&mut view_state.search_state.query)
            .hint_text("Search…")
            .desired_width(180.0)
            .lock_focus(true);
        if no_matches {
            text_edit = text_edit.background_color(no_match_bg);
        }

        // Text input.
        let response = ui.add(text_edit);

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
        let prev_response = ui.button("<").on_hover_text("Previous match");
        if prev_response.clicked() {
            action = SearchBarAction::Prev;
        }
        // → Next button.
        let next_response = ui.button(">").on_hover_text("Next match");
        if next_response.clicked() {
            action = SearchBarAction::Next;
        }
        // Close button.
        let close_response = ui.button("X").on_hover_text("Close");
        if close_response.clicked() {
            action = SearchBarAction::Close;
        }
        if prev_response.hovered() || next_response.hovered() || close_response.hovered() {
            safety = SearchOverlaySafety::TooltipMayEscape;
        }
    });

    (action, safety)
}

/// Row 2 of the search bar: the Regex/match-case toggles and the error
/// label. Extracted from [`show_search_bar`] for the same reason as
/// [`show_search_bar_row_1`].
///
/// Returns the paint-safety classification of the match-case checkbox (the
/// only tooltip-bearing control in this row; Task 124.14d). `Regex` has no
/// tooltip and deliberately does not contribute.
fn show_search_bar_row_2(
    ui: &mut Ui,
    view_state: &mut ViewState,
    error_msg: Option<&str>,
) -> SearchOverlaySafety {
    let mut safety = SearchOverlaySafety::Bounded;

    ui.horizontal(|ui| {
        ui.checkbox(&mut view_state.search_state.regex_mode, "Regex")
            .clickable();
        let case_response = ui
            .checkbox(&mut view_state.search_state.case_sensitive, "Aa")
            .clickable()
            .on_hover_text("Match case");
        if case_response.hovered() {
            safety = SearchOverlaySafety::TooltipMayEscape;
        }
        if let Some(err) = error_msg {
            let error_color = ui.visuals().error_fg_color;
            ui.colored_label(error_color, err);
        }
    });

    safety
}

/// Show the search overlay bar and return what it drew this frame.
///
/// The overlay is rendered as a floating `egui::Area` at the top-right
/// corner of `terminal_rect`.  It handles its own keyboard input (Enter,
/// Shift+Enter, Escape) so the caller does not need to intercept those keys
/// separately.
///
/// The function also updates `view_state.search_state.query` in response to
/// text-field input, but does NOT run the actual search — that is handled by
/// the caller so it can be deferred or run on a changed-query signal.
///
/// Returns a [`SearchBarFrame`] (Task 124.14d) carrying the triggered
/// [`SearchBarAction`], the bar's actual paint bounds, and whether those
/// bounds are safe to treat as complete this frame -- see
/// [`SearchOverlaySafety`].
pub fn show_search_bar(
    ui: &mut Ui,
    view_state: &mut ViewState,
    terminal_rect: Rect,
    error_msg: Option<&str>,
    pane_id: PaneId,
) -> SearchBarFrame {
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
    // OR-accumulated across every tooltip-bearing control (Task 124.14d):
    // Prev, Next, Close, and the "Aa" match-case checkbox. `Regex` has no
    // tooltip and deliberately does not contribute.
    let mut safety = SearchOverlaySafety::Bounded;

    // Constructed once so the shadow margin used below to compute
    // `paint_rect` is the exact same `Shadow` value the frame paints with,
    // not a second, possibly-drifted copy (`Frame` is `Copy`, so capturing
    // it here and reusing it inside the closure costs nothing).
    let popup_frame = Frame::popup(ui.style());

    let area_response = Area::new(egui::Id::new("search_overlay").with(pane_id))
        .order(Order::Foreground)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(anchor_pos)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            popup_frame
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.set_min_width(260.0);
                    let (row_1_action, row_1_safety) =
                        show_search_bar_row_1(ui, view_state, match_count, current);
                    action = row_1_action;
                    safety = safety.combine(row_1_safety);
                    safety = safety.combine(show_search_bar_row_2(ui, view_state, error_msg));
                });
        });

    // Also allow Escape at the window level (in case the text field doesn't have focus).
    if action == SearchBarAction::None && ui.input(|i| i.key_pressed(Key::Escape)) {
        action = SearchBarAction::Close;
    }

    let paint_rect = expand_by_shadow_margin(area_response.response.rect, popup_frame.shadow);
    SearchBarFrame {
        action,
        paint_rect,
        safety,
    }
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

    // ── SearchOverlaySafety::combine ─────────────────────────────────────

    /// Compact truth table for `combine`'s two-source OR-toward-unbounded
    /// rule. `Bounded`/`Bounded` and the two mixed cases are already
    /// exercised indirectly wherever `combine` is called (`show_search_bar`,
    /// `SearchDamageState::finish_overlay_frame`'s settling test); this pins
    /// all four cells explicitly, including `TooltipMayEscape`/
    /// `TooltipMayEscape`, which no caller's test sequence happens to hit
    /// (both operands unbounded at once is otherwise untested).
    #[test]
    fn combine_truth_table() {
        use SearchOverlaySafety::{Bounded, TooltipMayEscape};

        assert_eq!(Bounded.combine(Bounded), Bounded);
        assert_eq!(Bounded.combine(TooltipMayEscape), TooltipMayEscape);
        assert_eq!(TooltipMayEscape.combine(Bounded), TooltipMayEscape);
        assert_eq!(TooltipMayEscape.combine(TooltipMayEscape), TooltipMayEscape);
    }

    // ── run_search: substring ──────────────────────────────────────────────

    #[test]
    fn search_empty_query_returns_no_matches() {
        let chars = make_chars(&["hello world"]);
        let (matches, err) = run_search("", false, false, &chars);
        assert!(matches.is_empty());
        assert!(err.is_none());
    }

    #[test]
    fn search_single_match_on_first_row() {
        let chars = make_chars(&["hello world"]);
        let (matches, err) = run_search("hello", false, false, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].row, 0);
        assert_eq!(matches[0].col_start, 0);
        assert_eq!(matches[0].col_end, 4); // "hello" = cols 0-4
    }

    #[test]
    fn search_match_in_middle_of_row() {
        let chars = make_chars(&["abc foo bar"]);
        let (matches, err) = run_search("foo", false, false, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].col_start, 4);
        assert_eq!(matches[0].col_end, 6);
    }

    #[test]
    fn search_multiple_matches_same_row() {
        let chars = make_chars(&["abcabc"]);
        let (matches, err) = run_search("abc", false, false, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].col_start, 0);
        assert_eq!(matches[1].col_start, 3);
    }

    #[test]
    fn search_matches_across_rows() {
        let chars = make_chars(&["foo bar", "baz foo"]);
        let (matches, err) = run_search("foo", false, false, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].row, 0);
        assert_eq!(matches[1].row, 1);
    }

    #[test]
    fn search_case_insensitive() {
        let chars = make_chars(&["Hello WORLD"]);
        let (matches, err) = run_search("hello", false, false, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].col_end, 4);
    }

    #[test]
    fn search_no_match_returns_empty() {
        let chars = make_chars(&["hello world"]);
        let (matches, err) = run_search("xyz", false, false, &chars);
        assert!(err.is_none());
        assert!(matches.is_empty());
    }

    #[test]
    fn search_after_wide_char_uses_display_columns() {
        // U+4E16 (世) and U+754C (界) are each 2 display columns wide.
        // "世界hi" → display columns: 世=0-1, 界=2-3, h=4, i=5
        let chars = make_chars(&["世界hi"]);
        let (matches, err) = run_search("hi", false, false, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].col_start, 4);
        assert_eq!(matches[0].col_end, 5);
    }

    // ── run_search: regex ──────────────────────────────────────────────────

    #[test]
    fn search_regex_basic_match() {
        let chars = make_chars(&["foo123bar"]);
        let (matches, err) = run_search(r"\d+", true, false, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].col_start, 3);
        assert_eq!(matches[0].col_end, 5); // "123" = cols 3-5
    }

    #[test]
    fn search_invalid_regex_returns_error() {
        let chars = make_chars(&["hello"]);
        let (matches, err) = run_search(r"[invalid", true, false, &chars);
        assert!(matches.is_empty());
        assert!(err.is_some());
    }

    #[test]
    fn search_regex_no_match_returns_empty() {
        let chars = make_chars(&["hello"]);
        let (matches, err) = run_search(r"\d+", true, false, &chars);
        assert!(err.is_none());
        assert!(matches.is_empty());
    }

    // ── run_search: case sensitivity ───────────────────────────────────────

    #[test]
    fn search_case_sensitive_substring_rejects_different_case() {
        let chars = make_chars(&["Hello WORLD"]);
        let (matches, err) = run_search("hello", false, true, &chars);
        assert!(err.is_none());
        assert!(
            matches.is_empty(),
            "case-sensitive search for 'hello' must not match 'Hello'"
        );
    }

    #[test]
    fn search_case_sensitive_substring_matches_exact_case() {
        let chars = make_chars(&["Hello hello HELLO"]);
        let (matches, err) = run_search("hello", false, true, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 1, "exactly one case-sensitive match");
        assert_eq!(matches[0].col_start, 6);
    }

    #[test]
    fn search_case_insensitive_regex_matches_mixed_case() {
        let chars = make_chars(&["FOO bar Baz"]);
        let (matches, err) = run_search("foo", true, false, &chars);
        assert!(err.is_none());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].col_start, 0);
    }

    #[test]
    fn search_case_sensitive_regex_rejects_different_case() {
        let chars = make_chars(&["FOO bar"]);
        let (matches, err) = run_search("foo", true, true, &chars);
        assert!(err.is_none());
        assert!(
            matches.is_empty(),
            "case-sensitive regex must not match uppercase"
        );
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
            case_sensitive: false,
            last_searched_query: "foo".to_string(),
            last_searched_regex: true,
            last_searched_case_sensitive: false,
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
        assert!(!state.last_searched_case_sensitive);
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

    // ── expand_by_shadow_margin (Task 124.14d) ──────────────────────────────

    /// An asymmetric shadow -- distinct `offset` on each axis, non-zero
    /// `blur` and `spread` -- so all four margins (`left`, `right`, `top`,
    /// `bottom`) are proven independently against `epaint::Shadow::margin`'s
    /// own formula, rather than only a symmetric default that a guessed
    /// constant could accidentally match.
    #[test]
    fn expand_by_shadow_margin_uses_all_four_asymmetric_margins() {
        let shadow = Shadow {
            offset: [3, -2],
            blur: 4,
            spread: 1,
            color: Color32::BLACK,
        };
        // left = spread + 0.5*blur - offset_x = 1 + 2 - 3 = 0
        // right = spread + 0.5*blur + offset_x = 1 + 2 + 3 = 6
        // top = spread + 0.5*blur - offset_y = 1 + 2 - (-2) = 5
        // bottom = spread + 0.5*blur + offset_y = 1 + 2 + (-2) = 1
        let area_rect = Rect::from_min_max(Pos2::new(100.0, 100.0), Pos2::new(200.0, 150.0));

        let expanded = expand_by_shadow_margin(area_rect, shadow);

        assert_eq!(expanded.min.x.to_bits(), 100.0_f32.to_bits(), "left margin");
        assert_eq!(
            expanded.max.x.to_bits(),
            206.0_f32.to_bits(),
            "right margin"
        );
        assert_eq!(expanded.min.y.to_bits(), 95.0_f32.to_bits(), "top margin");
        assert_eq!(
            expanded.max.y.to_bits(),
            151.0_f32.to_bits(),
            "bottom margin"
        );
    }

    /// A zero shadow (no offset, no blur, no spread) must expand by
    /// nothing on every side -- the degenerate case that would hide a sign
    /// error if it were the only case tested.
    #[test]
    fn expand_by_shadow_margin_zero_shadow_is_a_no_op() {
        let area_rect = Rect::from_min_max(Pos2::new(10.0, 20.0), Pos2::new(30.0, 40.0));

        let expanded = expand_by_shadow_margin(area_rect, Shadow::NONE);

        assert_eq!(expanded, area_rect);
    }
}
