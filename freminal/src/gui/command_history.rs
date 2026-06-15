// Copyright (C) 2026 Fred Clausen
//
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Quick Command History Palette (Task 72.15).
//!
//! A modal overlay anchored inside the focused pane that presents a
//! filter-and-select list over the union of:
//!
//! - The **shell-history seed** (bash / zsh / fish) loaded once per pane
//!   spawn by [`crate::gui::shell_history`].  Capped at
//!   [`crate::gui::shell_history::HISTORY_SEED_CAP`] entries.
//! - The **live recent commands** captured via OSC 133 in Task 72.9
//!   (`Pane::recent_commands`), together with the GUI-side text cache
//!   (`Pane::command_texts`) populated at finish time.
//!
//! On Enter, the selected entry's command text is sent to the pane as
//! keyboard input via the existing `InputEvent::Key` channel.  The text
//! is sent **without** a trailing `\n` so the user reviews and presses
//! Enter themselves -- matching the wezterm / iTerm2 Recall convention.
//!
//! ## Filter
//!
//! Case-insensitive substring (`str::contains` after ASCII-lowercase
//! folding).  A previous design discussed using `nucleo-matcher` but
//! that crate is not in `Cargo.lock` and the handoff specifically
//! forbids adding a new dependency for this commit.  Polish (and any
//! upgrade to fuzzy matching) lands in 72.15 commit 3.
//!
//! ## Live-entry text extraction
//!
//! The PTY thread owns the buffer and only publishes a visible-window
//! snapshot to the GUI.  Live `CommandBlock`s reference buffer-absolute
//! rows that may have scrolled into scrollback, so the GUI cannot
//! synchronously read arbitrary command text on demand.
//!
//! The chosen approach (Option A in the handoff): at command-finish
//! time the GUI extracts the command text from the **then-current
//! snapshot** -- the block just finished, so its rows are still in the
//! visible window with overwhelming probability -- and caches the
//! result in `Pane::command_texts`.  Blocks whose text cannot be
//! extracted (rare race: the command produced enough output to scroll
//! its own input row out of the visible window before the GUI drained
//! the event) are simply absent from the palette's live half; seed
//! entries continue to surface normally.

use std::collections::HashMap;
use std::collections::VecDeque;

use crossbeam_channel::Sender;
use egui::{self, Align2, Area, Color32, Frame, Key, Order, Pos2, Rect, Ui};
use freminal_common::buffer_states::command_block::{CommandBlock, CommandBlockId, CommandStatus};
use freminal_common::buffer_states::tchar::TChar;
use freminal_terminal_emulator::io::InputEvent;
use freminal_terminal_emulator::snapshot::TerminalSnapshot;
use tracing::{debug, error, trace, warn};

use super::panes::PaneId;
use super::view_state::CommandHistoryState;

/// Cap on the number of entries actually rendered in the scrollable list.
///
/// Above this cap the filtered list is truncated; the user must refine
/// the query to see further matches.  Chosen to keep the modal
/// comfortably under one screenful at common font sizes without
/// requiring an internal scroll bar (left for commit 3 polish).
pub const MAX_VISIBLE_ENTRIES: usize = 200;

/// One row in the palette's filtered list.
///
/// Carried by value through `merge_entries` and `filter_entries`;
/// `text` is the canonical command string used both for display and
/// for the "send to pane" action on Enter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteEntry {
    /// The command text that will be sent to the pane on Enter.
    pub text: String,
    /// Provenance + per-entry metadata used for rendering.
    pub kind: EntryKind,
}

/// Where a `PaletteEntry` came from, plus render-time metadata.
///
/// Seed entries render plain (text only).  Live entries render with
/// the exit-code status badge so the user can tell at a glance whether
/// the previous invocation succeeded.  Timestamps are deferred to
/// commit 3 polish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    /// Entry came from the shell-history seed file.  No status / timestamp.
    Seed,
    /// Entry came from a live OSC 133 `CommandBlock`.  Carries the
    /// block's stable id (for future cross-frame correlation) and the
    /// derived [`CommandStatus`] for rendering.
    Live {
        /// Stable block id.  Useful for commit 3 polish (e.g. jump
        /// from a palette entry to its on-screen position).
        id: CommandBlockId,
        /// Derived exit-code status.
        status: CommandStatus,
    },
}

/// Action returned by [`show_command_history_palette`] each frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteAction {
    /// No user action this frame.
    None,
    /// The user pressed Escape, clicked the close button, or clicked
    /// outside the palette.  The caller should close the modal.
    Close,
    /// The user selected an entry (Enter or click).  The carried
    /// `String` is the command text to send to the pane.
    Submit(String),
}

// ---------------------------------------------------------------------------
//  Text extraction from a snapshot
// ---------------------------------------------------------------------------

/// Extract the command text for a finished `CommandBlock` from a
/// snapshot's visible window.
///
/// Returns `None` when:
///
/// - the block has no `command_start_row` (OSC 133 B never fired), or
/// - the block has no `output_start_row` (OSC 133 C never fired), or
/// - the row range `[command_start_row, output_start_row)` lies wholly
///   or partially outside the snapshot's visible window (i.e. the rows
///   have already scrolled into scrollback by the time this is called).
///
/// The returned string is trimmed of leading and trailing ASCII
/// whitespace.  Multi-row commands are joined with single spaces in
/// place of internal newlines so the palette displays them compactly.
#[must_use]
pub fn extract_command_text(snap: &TerminalSnapshot, block: &CommandBlock) -> Option<String> {
    let cmd_start_buf = block.command_start_row?;
    let cmd_end_buf = block.output_start_row?;
    if cmd_end_buf < cmd_start_buf {
        trace!(
            "extract_command_text: degenerate row range {cmd_start_buf}..{cmd_end_buf} -- skipping"
        );
        return None;
    }

    // Compute the buffer-absolute row of screen row 0.
    //
    // visible window covers
    //   [total_rows - height - scroll_offset, total_rows - scroll_offset)
    // in buffer-absolute coordinates.  `saturating_sub` keeps things
    // sensible before any scrollback has accumulated.
    let visible_top_buf = snap
        .total_rows
        .saturating_sub(snap.height)
        .saturating_sub(snap.scroll_offset);
    let visible_bottom_buf = visible_top_buf.saturating_add(snap.height);

    // Both endpoints must be inside the visible window.  We need at
    // least one row of text (cmd_end_buf may equal cmd_start_buf when
    // the user pressed Enter on an empty line; we still want to skip
    // those cleanly via the trim below rather than emit an empty entry).
    if cmd_start_buf < visible_top_buf || cmd_end_buf > visible_bottom_buf {
        return None;
    }

    let screen_start = cmd_start_buf - visible_top_buf;
    let screen_end = cmd_end_buf - visible_top_buf;
    // A zero-length row range yields no text -- skip rather than emit "".
    if screen_start == screen_end {
        return None;
    }

    let mut out = String::new();
    for screen_row in screen_start..screen_end {
        let row_start = *snap.row_offsets.get(screen_row)?;
        let row_end = snap
            .row_offsets
            .get(screen_row + 1)
            .copied()
            .unwrap_or(snap.visible_chars.len());
        if row_end <= row_start || row_end > snap.visible_chars.len() {
            continue;
        }
        // Per-row text first goes into a scratch buffer so we can trim
        // leading/trailing whitespace before joining.  Without this, a
        // continuation row indented for visual alignment would carry
        // its indent into the palette display as extra spaces after
        // the joiner.
        let mut row_text = String::new();
        append_row_text(&snap.visible_chars[row_start..row_end], &mut row_text);
        let trimmed_row = row_text.trim();
        if !trimmed_row.is_empty() {
            if !out.is_empty() {
                // Collapse internal newlines to a single space so
                // multi-line commands display compactly in the
                // palette list.
                out.push(' ');
            }
            out.push_str(trimmed_row);
        }
    }

    let trimmed = out.trim().to_owned();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Append the textual content of one snapshot row's `TChar` slice into
/// `out`.  Stops at the first `NewLine` (which terminates the row in
/// the flat buffer) and silently skips invalid UTF-8 cells.
fn append_row_text(row: &[TChar], out: &mut String) {
    for tc in row {
        if matches!(tc, TChar::NewLine) {
            break;
        }
        if let Ok(text) = std::str::from_utf8(tc.as_bytes()) {
            out.push_str(text);
        }
    }
}

// ---------------------------------------------------------------------------
//  Entry merging and filtering
// ---------------------------------------------------------------------------

/// Merge the live recent commands with the shell-history seed into a
/// single de-duplicated, **most-recent-first** entry list.
///
/// **Ordering (106.5):** the most recently run command appears first.
/// Live OSC 133 entries are emitted newest-first (this session's
/// commands), followed by the shell-history seed newest-first.  This
/// matters for more than aesthetics: the rendered list is capped at
/// [`MAX_VISIBLE_ENTRIES`], and the seed can hold up to
/// [`crate::gui::shell_history::HISTORY_SEED_CAP`] entries.  With the
/// old oldest-first ordering, the cap sliced off the tail where this
/// session's live commands lived, so a command just typed never
/// appeared in the unfiltered list (it surfaced only when searched for,
/// because filtering scans the whole list before the cap is applied).
/// Newest-first puts recent commands at the top, inside the cap, and
/// pairs with the palette's default selection of index `0`.
///
/// **De-duplication:** dedup is global by command text, first
/// occurrence wins.  Because live entries are walked before seed
/// entries, the most-recent (live) variant of a repeated command is the
/// one kept -- it carries the exit-code badge the seed lacks -- and a
/// command that was both run this session and present in the history
/// file appears exactly once, near the top, rather than twice.  This
/// also subsumes the previous `HISTCONTROL=ignoredups`-style collapse of
/// back-to-back duplicates.
///
/// `seed` is the optional shell-history seed (`None` = loader thread
/// has not finished yet, or the shell has no recognised history file);
/// it is stored oldest-first, so it is iterated in reverse here.
/// `recent` is the pane's live `recent_commands` ring buffer (stored
/// oldest-first, also iterated in reverse).  `texts` is the per-pane
/// text cache keyed by [`CommandBlockId`]; live entries without a cache
/// hit are dropped (their text could not be extracted from the snapshot
/// at finish time).
#[must_use]
pub fn merge_entries(
    seed: Option<&Vec<String>>,
    recent: &VecDeque<CommandBlock>,
    texts: &HashMap<CommandBlockId, String>,
) -> Vec<PaletteEntry> {
    let mut out: Vec<PaletteEntry> = Vec::new();
    let mut seen_texts: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Live entries first, newest-first (the ring is stored oldest-first).
    for block in recent.iter().rev() {
        let Some(text) = texts.get(&block.id) else {
            continue;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !seen_texts.insert(trimmed.to_owned()) {
            continue;
        }
        out.push(PaletteEntry {
            text: trimmed.to_owned(),
            kind: EntryKind::Live {
                id: block.id,
                status: block.status(),
            },
        });
    }

    // Then the shell-history seed, newest-first (stored oldest-first).
    if let Some(seed_vec) = seed {
        for cmd in seed_vec.iter().rev() {
            let trimmed = cmd.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !seen_texts.insert(trimmed.to_owned()) {
                continue;
            }
            out.push(PaletteEntry {
                text: trimmed.to_owned(),
                kind: EntryKind::Seed,
            });
        }
    }

    out
}

/// Case-insensitive substring filter over a merged entry list.
///
/// An empty query returns the full list unchanged (cloned).  The
/// result is capped at [`MAX_VISIBLE_ENTRIES`] entries; the user can
/// refine the query to surface further matches.
#[must_use]
pub fn filter_entries(entries: &[PaletteEntry], query: &str) -> Vec<PaletteEntry> {
    if query.is_empty() {
        return entries.iter().take(MAX_VISIBLE_ENTRIES).cloned().collect();
    }
    let needle = query.to_ascii_lowercase();
    entries
        .iter()
        .filter(|e| e.text.to_ascii_lowercase().contains(&needle))
        .take(MAX_VISIBLE_ENTRIES)
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
//  Send-to-pane helper
// ---------------------------------------------------------------------------

/// Send the selected command text to the pane as keyboard input.
///
/// **No trailing newline:** the user reviews the line and presses Enter
/// themselves.  Matches the wezterm / iTerm2 Recall convention and is a
/// hard locked-in design decision (see the handoff).
///
/// Returns `true` when the send succeeded.  Logs on failure; the caller
/// closes the modal regardless of outcome so the user is never trapped
/// in a broken UI state.
pub fn send_command_text(input_tx: &Sender<InputEvent>, text: &str) -> bool {
    match input_tx.send(InputEvent::Key(text.as_bytes().to_vec())) {
        Ok(()) => true,
        Err(e) => {
            error!(
                "command-history palette: failed to send command text to PTY ({} bytes): {e}",
                text.len()
            );
            false
        }
    }
}

// ---------------------------------------------------------------------------
//  Palette UI
// ---------------------------------------------------------------------------

/// Render the Quick Command History Palette modal and return the
/// user's action this frame.
///
/// The palette is anchored to the centre-top of `terminal_rect` so it
/// stays inside the focused pane regardless of where that pane sits in
/// the window.  It mirrors the structural pattern of
/// [`crate::gui::search::show_search_bar`] (an `egui::Area` overlay in
/// `Order::Foreground` with a `Frame::popup` body) so behaviour is
/// consistent across modal surfaces.
///
/// The render fn keeps `state.selected` clamped to the bounds of the
/// filtered list so the caller never has to clamp it itself.
#[allow(clippy::too_many_arguments)]
pub fn show_command_history_palette(
    ui: &Ui,
    state: &mut CommandHistoryState,
    terminal_rect: Rect,
    pane_id: PaneId,
    seed: Option<&Vec<String>>,
    recent: &VecDeque<CommandBlock>,
    texts: &HashMap<CommandBlockId, String>,
) -> PaletteAction {
    let merged = merge_entries(seed, recent, texts);
    let filtered = filter_entries(&merged, &state.query);

    if !filtered.is_empty() && state.selected >= filtered.len() {
        state.selected = filtered.len() - 1;
    }

    let anchor_pos = Pos2::new(terminal_rect.center().x, terminal_rect.top() + 24.0);

    let mut action = PaletteAction::None;

    Area::new(egui::Id::new("command_history_overlay").with(pane_id))
        .order(Order::Foreground)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(anchor_pos)
        .interactable(true)
        .show(ui.ctx(), |ui| {
            Frame::popup(ui.style())
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_min_width(420.0);
                    ui.set_max_width(720.0);

                    // ── Title row ────────────────────────────────────
                    ui.horizontal(|ui| {
                        ui.label("Command history");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("X").on_hover_text("Close (Esc)").clicked() {
                                action = PaletteAction::Close;
                            }
                        });
                    });

                    ui.separator();

                    // ── Query input ──────────────────────────────────
                    let text_edit = egui::TextEdit::singleline(&mut state.query)
                        .hint_text("Filter…")
                        .desired_width(f32::INFINITY)
                        .lock_focus(true);
                    let response = ui.add(text_edit);

                    // Always pull focus when the palette is open so the user
                    // can start typing immediately on open.
                    if !response.has_focus() {
                        response.request_focus();
                    }

                    // Keyboard navigation (handled before list render so the
                    // selected index reflects the current frame's input).
                    let (pressed_up, pressed_down, pressed_enter, pressed_escape) = ui.input(|i| {
                        (
                            i.key_pressed(Key::ArrowUp),
                            i.key_pressed(Key::ArrowDown),
                            i.key_pressed(Key::Enter),
                            i.key_pressed(Key::Escape),
                        )
                    });

                    if pressed_escape {
                        action = PaletteAction::Close;
                    }

                    if !filtered.is_empty() {
                        if pressed_up {
                            state.selected = if state.selected == 0 {
                                filtered.len() - 1
                            } else {
                                state.selected - 1
                            };
                        }
                        if pressed_down {
                            state.selected = (state.selected + 1) % filtered.len();
                        }
                        if pressed_enter && let Some(entry) = filtered.get(state.selected) {
                            action = PaletteAction::Submit(entry.text.clone());
                        }
                    }

                    ui.separator();

                    // ── Entry list ───────────────────────────────────
                    if filtered.is_empty() {
                        let msg = if merged.is_empty() {
                            "No command history available."
                        } else {
                            "No matches."
                        };
                        ui.label(msg);
                    } else {
                        egui::ScrollArea::vertical()
                            .max_height(360.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for (idx, entry) in filtered.iter().enumerate() {
                                    let selected = idx == state.selected;
                                    let row = render_entry(ui, entry, selected);
                                    if row.clicked() {
                                        action = PaletteAction::Submit(entry.text.clone());
                                    }
                                }
                            });
                    }

                    // ── Hint row ─────────────────────────────────────
                    ui.separator();
                    ui.small("Enter: insert (no Enter sent)   Esc: close   ↑/↓: navigate");
                });
        });

    action
}

/// Render one entry row.  Returns the row's `Response` so the caller
/// can detect clicks and forward them as a `Submit` action.
fn render_entry(ui: &mut Ui, entry: &PaletteEntry, selected: bool) -> egui::Response {
    let bg = if selected {
        ui.visuals().selection.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    Frame::NONE
        .fill(bg)
        .inner_margin(egui::Margin::symmetric(4, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Status badge -- live entries only.
                let badge = entry_badge(&entry.kind);
                ui.label(badge);
                // Truncate with ellipsis so a single very long history
                // entry (e.g. a one-line megabyte JSON payload from a
                // real `.zsh_history`) cannot expand the row's
                // horizontal layout past the popup's max width and push
                // every other entry off-screen to the right.
                ui.add(egui::Label::new(&entry.text).truncate());
            });
        })
        .response
        .interact(egui::Sense::click())
}

/// Short ASCII badge prefix for the entry kind.
///
/// Commit 3 polish replaces these with proper exit-code icons; the
/// MVP keeps them as plain text so the palette is fully functional
/// without any icon-font dependency.
const fn entry_badge(kind: &EntryKind) -> &'static str {
    match kind {
        EntryKind::Seed => "  ",
        EntryKind::Live {
            status: CommandStatus::Success,
            ..
        } => "OK",
        EntryKind::Live {
            status: CommandStatus::Failure(_),
            ..
        } => "ER",
        EntryKind::Live {
            status: CommandStatus::Running,
            ..
        } => "..",
        EntryKind::Live {
            status: CommandStatus::Unknown,
            ..
        } => "??",
    }
}

// Lightweight diagnostics for the open/close path so flakes are
// debuggable from logs alone.
#[doc(hidden)]
pub fn log_open(pane_id: PaneId, seed_loaded: bool, recent_len: usize, texts_len: usize) {
    debug!(
        "command-history palette opened (pane={pane_id}, seed_loaded={seed_loaded}, \
         recent_len={recent_len}, texts_cached={texts_len})"
    );
}

#[doc(hidden)]
pub fn log_close(pane_id: PaneId) {
    debug!("command-history palette closed (pane={pane_id})");
}

#[doc(hidden)]
pub fn log_submit_failure(pane_id: PaneId, len: usize) {
    warn!("command-history palette: submit failed for {len}-byte command (pane={pane_id})");
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use super::*;

    // ── Test helpers ────────────────────────────────────────────────

    /// Build a minimal `TerminalSnapshot` whose visible window contains
    /// the given rows (one row per `&str`).  Buffer rows start at
    /// `scrollback_rows` (i.e. there are that many rows above the
    /// visible window).
    fn make_snapshot(rows: &[&str], scrollback_rows: usize) -> TerminalSnapshot {
        let mut chars: Vec<TChar> = Vec::new();
        let mut row_offsets: Vec<usize> = Vec::new();
        for row in rows {
            row_offsets.push(chars.len());
            for b in row.bytes() {
                chars.push(TChar::Ascii(b));
            }
            chars.push(TChar::NewLine);
        }
        row_offsets.push(chars.len());

        let height = rows.len();
        let term_width = rows.iter().map(|r| r.len()).max().unwrap_or(0);

        // Start from `TerminalSnapshot::empty()` so future field
        // additions on `TerminalSnapshot` do not require touching this
        // test helper (every field already has a sensible default).
        let mut snap = TerminalSnapshot::empty();
        snap.visible_chars = Arc::new(chars);
        snap.row_offsets = Arc::new(row_offsets);
        snap.height = height;
        snap.term_width = term_width;
        snap.term_height = height;
        snap.total_rows = scrollback_rows + height;
        snap.max_scroll_offset = scrollback_rows;
        snap
    }

    fn finished_block(
        prompt_row: usize,
        cmd_start: Option<usize>,
        output_start: Option<usize>,
        exit_code: Option<i32>,
    ) -> CommandBlock {
        CommandBlock {
            id: CommandBlockId::next(),
            fid: "test".to_owned(),
            prompt_start_row: prompt_row,
            command_start_row: cmd_start,
            output_start_row: output_start,
            end_row: Some(output_start.unwrap_or(prompt_row) + 1),
            exit_code,
            cwd: None,
            started_at: SystemTime::UNIX_EPOCH,
            executed_at: Some(SystemTime::UNIX_EPOCH),
            finished_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        }
    }

    // ── extract_command_text ────────────────────────────────────────

    #[test]
    fn extract_command_text_single_row_returns_trimmed() {
        // Visible window: 4 rows, no scrollback.  Block's command row
        // is row 1 (buffer-absolute 1), output starts at row 2.
        let snap = make_snapshot(&["$ ", "ls -la   ", "file1", "file2"], 0);
        let block = finished_block(0, Some(1), Some(2), Some(0));
        assert_eq!(
            extract_command_text(&snap, &block),
            Some("ls -la".to_owned())
        );
    }

    #[test]
    fn extract_command_text_multi_row_joins_with_space() {
        // Two-row command (line-continuation).
        let snap = make_snapshot(&["$ ", "echo foo \\", "  bar", "foo bar"], 0);
        let block = finished_block(0, Some(1), Some(3), Some(0));
        assert_eq!(
            extract_command_text(&snap, &block),
            Some("echo foo \\ bar".to_owned())
        );
    }

    #[test]
    fn extract_command_text_returns_none_when_command_start_missing() {
        let snap = make_snapshot(&["$ ", "ls"], 0);
        let block = finished_block(0, None, Some(2), Some(0));
        assert_eq!(extract_command_text(&snap, &block), None);
    }

    #[test]
    fn extract_command_text_returns_none_when_output_start_missing() {
        let snap = make_snapshot(&["$ ", "ls"], 0);
        let block = finished_block(0, Some(1), None, Some(0));
        assert_eq!(extract_command_text(&snap, &block), None);
    }

    #[test]
    fn extract_command_text_returns_none_when_rows_below_visible_window() {
        // Scrollback of 10 rows; visible window is buffer rows 10..14.
        // Block's command row is buffer row 5 (scrolled past).
        let snap = make_snapshot(&["a", "b", "c", "d"], 10);
        let block = finished_block(5, Some(5), Some(6), Some(0));
        assert_eq!(extract_command_text(&snap, &block), None);
    }

    #[test]
    fn extract_command_text_handles_visible_window_with_scrollback() {
        // Scrollback of 10; visible window covers buffer rows 10..14.
        // Block at buffer rows 11 (command) -> 12 (output).
        let snap = make_snapshot(&["$ ", "make test", "running…", "ok"], 10);
        let block = finished_block(10, Some(11), Some(12), Some(0));
        assert_eq!(
            extract_command_text(&snap, &block),
            Some("make test".to_owned())
        );
    }

    #[test]
    fn extract_command_text_returns_none_for_empty_range() {
        let snap = make_snapshot(&["$ "], 0);
        let block = finished_block(0, Some(0), Some(0), Some(0));
        assert_eq!(extract_command_text(&snap, &block), None);
    }

    #[test]
    fn extract_command_text_returns_none_for_inverted_range() {
        let snap = make_snapshot(&["$ ", "x"], 0);
        let block = finished_block(0, Some(2), Some(1), Some(0));
        assert_eq!(extract_command_text(&snap, &block), None);
    }

    // ── merge_entries ───────────────────────────────────────────────

    #[test]
    fn merge_entries_no_seed_no_recent_returns_empty() {
        let recent = VecDeque::new();
        let texts = HashMap::new();
        assert!(merge_entries(None, &recent, &texts).is_empty());
    }

    #[test]
    fn merge_entries_seed_only_is_newest_first() {
        // Seed is stored oldest-first; the palette presents it
        // newest-first (106.5).
        let seed = vec!["ls".to_owned(), "pwd".to_owned(), "echo hi".to_owned()];
        let recent = VecDeque::new();
        let texts = HashMap::new();
        let merged = merge_entries(Some(&seed), &recent, &texts);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].text, "echo hi");
        assert_eq!(merged[1].text, "pwd");
        assert_eq!(merged[2].text, "ls");
        assert!(matches!(merged[0].kind, EntryKind::Seed));
    }

    #[test]
    fn merge_entries_collapses_duplicates_in_seed() {
        // Global text dedup keeps the first (newest) occurrence.
        let seed = vec!["ls".to_owned(), "ls".to_owned(), "pwd".to_owned()];
        let recent = VecDeque::new();
        let texts = HashMap::new();
        let merged = merge_entries(Some(&seed), &recent, &texts);
        assert_eq!(merged.len(), 2);
        // Newest-first: pwd (newest) then the single surviving ls.
        assert_eq!(merged[0].text, "pwd");
        assert_eq!(merged[1].text, "ls");
    }

    #[test]
    fn merge_entries_dedups_non_consecutive_duplicates_in_seed() {
        // Old behaviour only collapsed *consecutive* duplicates; the
        // 106.5 global dedup also collapses an entry repeated later in
        // history, keeping the most-recent (first-seen, newest-first)
        // occurrence.
        let seed = vec!["ls".to_owned(), "pwd".to_owned(), "ls".to_owned()];
        let recent = VecDeque::new();
        let texts = HashMap::new();
        let merged = merge_entries(Some(&seed), &recent, &texts);
        assert_eq!(merged.len(), 2);
        // Walking newest-first: the trailing "ls" is seen first and kept;
        // "pwd" next; the leading "ls" is a duplicate and dropped.
        assert_eq!(merged[0].text, "ls");
        assert_eq!(merged[1].text, "pwd");
    }

    #[test]
    fn merge_entries_skips_empty_seed_entries() {
        let seed = vec![String::new(), "   ".to_owned(), "ls".to_owned()];
        let recent = VecDeque::new();
        let texts = HashMap::new();
        let merged = merge_entries(Some(&seed), &recent, &texts);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "ls");
    }

    #[test]
    fn merge_entries_includes_live_entries_with_cached_text() {
        let seed: Vec<String> = Vec::new();
        let mut recent = VecDeque::new();
        let block = finished_block(0, Some(1), Some(2), Some(0));
        let id = block.id;
        recent.push_back(block);
        let mut texts = HashMap::new();
        texts.insert(id, "make test".to_owned());

        let merged = merge_entries(Some(&seed), &recent, &texts);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "make test");
        assert!(matches!(
            merged[0].kind,
            EntryKind::Live {
                status: CommandStatus::Success,
                ..
            }
        ));
    }

    #[test]
    fn merge_entries_drops_live_entries_without_cached_text() {
        let mut recent = VecDeque::new();
        recent.push_back(finished_block(0, Some(1), Some(2), Some(0)));
        let texts = HashMap::new();
        let merged = merge_entries(None, &recent, &texts);
        assert!(merged.is_empty());
    }

    #[test]
    fn merge_entries_live_newest_first_then_seed_newest_first() {
        // 106.5: live commands (this session) come first, newest-first;
        // then the seed, newest-first.
        let seed = vec!["ls".to_owned()];
        let mut recent = VecDeque::new();
        let mut texts = HashMap::new();
        // Pushed oldest-first into the ring: "pwd" then "make".
        for cmd in ["pwd", "make"] {
            let block = finished_block(0, Some(1), Some(2), Some(0));
            texts.insert(block.id, cmd.to_owned());
            recent.push_back(block);
        }

        let merged = merge_entries(Some(&seed), &recent, &texts);
        assert_eq!(merged.len(), 3);
        // Live newest-first: "make" (most recent), then "pwd".
        assert_eq!(merged[0].text, "make");
        assert!(matches!(merged[0].kind, EntryKind::Live { .. }));
        assert_eq!(merged[1].text, "pwd");
        assert!(matches!(merged[1].kind, EntryKind::Live { .. }));
        // Then the seed.
        assert_eq!(merged[2].text, "ls");
        assert!(matches!(merged[2].kind, EntryKind::Seed));
    }

    #[test]
    fn merge_entries_live_command_also_in_seed_appears_once_as_live() {
        // A command run this session that is also present in the history
        // file must appear exactly once, near the top, as the Live
        // variant (carrying the exit-code badge) -- not twice.
        let seed = vec!["git status".to_owned(), "ls".to_owned()];
        let mut recent = VecDeque::new();
        let block = finished_block(0, Some(1), Some(2), Some(0));
        let id = block.id;
        recent.push_back(block);
        let mut texts = HashMap::new();
        texts.insert(id, "git status".to_owned());

        let merged = merge_entries(Some(&seed), &recent, &texts);
        // "git status" (live, newest), then seed "ls". The seed copy of
        // "git status" is deduplicated away.
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].text, "git status");
        assert!(matches!(merged[0].kind, EntryKind::Live { .. }));
        assert_eq!(merged[1].text, "ls");
        assert!(matches!(merged[1].kind, EntryKind::Seed));
    }

    #[test]
    fn merge_entries_recent_command_is_visible_within_cap_regression() {
        // 106.5 regression guard: a command typed this session must be
        // visible in the *unfiltered* list even when the seed is larger
        // than MAX_VISIBLE_ENTRIES. Previously the oldest-first ordering
        // pushed live entries past the cap so they only surfaced when
        // explicitly searched for.
        let seed: Vec<String> = (0..(MAX_VISIBLE_ENTRIES * 3))
            .map(|i| format!("old-cmd-{i}"))
            .collect();
        let mut recent = VecDeque::new();
        let block = finished_block(0, Some(1), Some(2), Some(0));
        let id = block.id;
        recent.push_back(block);
        let mut texts = HashMap::new();
        texts.insert(id, "random command".to_owned());

        let merged = merge_entries(Some(&seed), &recent, &texts);
        // The live command is the very first entry...
        assert_eq!(merged[0].text, "random command");
        // ...and an empty-query filter (which applies the cap) still
        // includes it.
        let filtered = filter_entries(&merged, "");
        assert_eq!(filtered.len(), MAX_VISIBLE_ENTRIES);
        assert_eq!(filtered[0].text, "random command");
    }

    #[test]
    fn merge_entries_live_failure_status_propagates() {
        let mut recent = VecDeque::new();
        let block = finished_block(0, Some(1), Some(2), Some(127));
        let id = block.id;
        recent.push_back(block);
        let mut texts = HashMap::new();
        texts.insert(id, "missing-bin".to_owned());

        let merged = merge_entries(None, &recent, &texts);
        assert_eq!(merged.len(), 1);
        assert!(matches!(
            merged[0].kind,
            EntryKind::Live {
                status: CommandStatus::Failure(127),
                ..
            }
        ));
    }

    // ── filter_entries ──────────────────────────────────────────────

    fn seed_entry(text: &str) -> PaletteEntry {
        PaletteEntry {
            text: text.to_owned(),
            kind: EntryKind::Seed,
        }
    }

    #[test]
    fn filter_entries_empty_query_returns_full_list() {
        let entries = vec![seed_entry("ls"), seed_entry("pwd")];
        let filtered = filter_entries(&entries, "");
        assert_eq!(filtered, entries);
    }

    #[test]
    fn filter_entries_case_insensitive_substring() {
        let entries = vec![
            seed_entry("Make Test"),
            seed_entry("CARGO build"),
            seed_entry("ls"),
        ];
        let filtered = filter_entries(&entries, "cargo");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].text, "CARGO build");
    }

    #[test]
    fn filter_entries_no_matches_returns_empty() {
        let entries = vec![seed_entry("ls"), seed_entry("pwd")];
        assert!(filter_entries(&entries, "xyz-unmatched").is_empty());
    }

    #[test]
    fn filter_entries_caps_at_max_visible() {
        let entries: Vec<PaletteEntry> = (0..(MAX_VISIBLE_ENTRIES + 50))
            .map(|i| seed_entry(&format!("cmd-{i}")))
            .collect();
        // Empty query: full list, capped.
        let all = filter_entries(&entries, "");
        assert_eq!(all.len(), MAX_VISIBLE_ENTRIES);
        // Matching query: "cmd-" matches all entries; still capped.
        let many = filter_entries(&entries, "cmd-");
        assert_eq!(many.len(), MAX_VISIBLE_ENTRIES);
    }

    // ── CommandHistoryState ─────────────────────────────────────────

    #[test]
    fn state_open_resets_query_and_selection() {
        let mut state = CommandHistoryState {
            is_open: false,
            query: "stale".to_owned(),
            selected: 5,
        };
        state.open();
        assert!(state.is_open);
        assert!(state.query.is_empty());
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn state_close_clears_open_flag_and_resets() {
        let mut state = CommandHistoryState {
            is_open: true,
            query: "ls".to_owned(),
            selected: 3,
        };
        state.close();
        assert!(!state.is_open);
        assert!(state.query.is_empty());
        assert_eq!(state.selected, 0);
    }

    // ── send_command_text ───────────────────────────────────────────

    #[test]
    fn send_command_text_emits_key_event_without_newline() {
        let (tx, rx) = crossbeam_channel::unbounded::<InputEvent>();
        assert!(send_command_text(&tx, "ls -la"));
        let evt = rx.try_recv().unwrap();
        match evt {
            InputEvent::Key(bytes) => assert_eq!(bytes, b"ls -la".to_vec()),
            other => panic!("expected Key event, got {other:?}"),
        }
        // No trailing newline -- the user reviews and presses Enter.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn send_command_text_returns_false_on_closed_channel() {
        let (tx, rx) = crossbeam_channel::unbounded::<InputEvent>();
        drop(rx);
        assert!(!send_command_text(&tx, "ls"));
    }

    // ── EntryKind status badges (regression guard for commit 3 polish) ──

    #[test]
    fn entry_badge_distinguishes_seed_and_live_kinds() {
        assert_eq!(entry_badge(&EntryKind::Seed), "  ");
        assert_eq!(
            entry_badge(&EntryKind::Live {
                id: CommandBlockId::next(),
                status: CommandStatus::Success,
            }),
            "OK"
        );
        assert_eq!(
            entry_badge(&EntryKind::Live {
                id: CommandBlockId::next(),
                status: CommandStatus::Failure(1),
            }),
            "ER"
        );
        assert_eq!(
            entry_badge(&EntryKind::Live {
                id: CommandBlockId::next(),
                status: CommandStatus::Running,
            }),
            ".."
        );
        assert_eq!(
            entry_badge(&EntryKind::Live {
                id: CommandBlockId::next(),
                status: CommandStatus::Unknown,
            }),
            "??"
        );
    }
}
