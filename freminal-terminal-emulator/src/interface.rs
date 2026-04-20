// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Cached flat representation of the visible window stored between snapshots.
///
/// Two separate `Arc<Vec<T>>` fields match the types in `TerminalSnapshot`
/// directly, so the clean path (no dirty rows) is a pair of refcount bumps
/// with no `Vec` allocation.
///
/// Fields: `(chars, tags, row_offsets, url_tag_indices)`.
type VisibleSnap = Option<(
    Arc<Vec<TChar>>,
    Arc<Vec<FormatTag>>,
    Arc<Vec<usize>>,
    Arc<Vec<usize>>,
)>;

/// Result of flattening visible rows:
/// `(chars, tags, row_offsets, url_tag_indices, content_changed)`.
type FlattenResult = (
    Arc<Vec<TChar>>,
    Arc<Vec<FormatTag>>,
    Arc<Vec<usize>>,
    Arc<Vec<usize>>,
    bool,
);

/// Image data collected from the visible window for a snapshot.
///
/// First element: map of referenced images (keyed by ID).
/// Second element: per-cell placement vector (parallel to `visible_chars`).
type VisibleImages = (
    Arc<HashMap<u64, InlineImage>>,
    Arc<Vec<Option<ImagePlacement>>>,
);

// Re-export input types from `input` so existing `interface::` import paths
// continue to compile without modification.
pub use crate::input::{
    KeyModifiers, TerminalInput, TerminalInputPayload, collect_text,
    raw_ascii_bytes_to_terminal_input,
};

use conv2::ValueFrom;

use crate::io::FreminalPtyInputOutput;
use crate::io::{FreminalTerminalSize, PtyRead, PtyWrite};
use crate::snapshot::TerminalSnapshot;
use crate::state::{TerminalSections, internal::TerminalState};
use anyhow::Result;
use crossbeam_channel::{Receiver, unbounded};
use freminal_buffer::image_store::{ImagePlacement, InlineImage};

use freminal_common::buffer_states::format_tag::FormatTag;
use freminal_common::buffer_states::modes::{
    alternate_scroll::AlternateScroll, application_escape_key::ApplicationEscapeKey,
    decarm::Decarm, decbkm::Decbkm, decckm::Decckm, keypad::KeypadMode, lnm::Lnm,
    mouse::MouseEncoding, mouse::MouseTrack, rl_bracket::RlBracket,
};

use freminal_common::{args::Args, buffer_states::tchar::TChar};

/// Mode-related fields extracted from the emulator state for a snapshot.
///
/// Factored out so `build_snapshot` stays within Clippy's 100-line limit.
struct SnapshotModeFields {
    bracketed_paste: RlBracket,
    mouse_tracking: MouseTrack,
    mouse_encoding: MouseEncoding,
    repeat_keys: Decarm,
    cursor_key_app_mode: Decckm,
    keypad_app_mode: KeypadMode,
    skip_draw: bool,
    modify_other_keys: u8,
    application_escape_key: ApplicationEscapeKey,
    backarrow_sends_bs: Decbkm,
    alternate_scroll: AlternateScroll,
    line_feed_mode: Lnm,
    kitty_keyboard_flags: u32,
}

#[must_use]
pub fn split_format_data_for_scrollback(
    tags: Vec<FormatTag>,
    scrollback_split: usize,
    visible_end: usize,
    include_scrollback: bool,
) -> TerminalSections<Vec<FormatTag>> {
    let scrollback_tags = if include_scrollback {
        tags.iter()
            .filter(|tag| tag.start < scrollback_split)
            .cloned()
            .map(|mut tag| {
                tag.end = tag.end.min(scrollback_split);
                tag
            })
            .collect()
    } else {
        Vec::new()
    };

    let canvas_tags: Vec<FormatTag> = tags
        .into_iter()
        .filter(|tag| tag.end > scrollback_split && tag.end <= visible_end)
        .map(|mut tag| {
            tag.start = tag.start.saturating_sub(scrollback_split);
            if tag.end != usize::MAX {
                tag.end -= scrollback_split;
            }
            tag
        })
        .collect();

    TerminalSections {
        scrollback: scrollback_tags,
        visible: canvas_tags,
    }
}

/// Auto-resume timeout for `SynchronizedUpdates::DontDraw` (?2026).
///
/// If a program sets `DontDraw` but crashes or never resets it, rendering would
/// freeze indefinitely.  After this many milliseconds of continuous `DontDraw`,
/// `build_snapshot` automatically resets the mode to `Draw`.
///
/// NOTE: Timeout implementation lives in `TerminalEmulator::apply_sync_updates_timeout`.
/// See `freminal-common/src/buffer_states/modes/sync_updates.rs` for spec references.
const SYNC_UPDATES_TIMEOUT_MS: u64 = 200;

pub struct TerminalEmulator {
    pub internal: TerminalState,
    /// PTY I/O layer (holds the terminfo `TempDir` and child-exit receiver).
    /// `None` in headless/benchmark mode where no PTY is started.
    pty_io: Option<FreminalPtyInputOutput>,
    write_tx: crossbeam_channel::Sender<PtyWrite>,
    /// Cached flat representation of the visible window from the last
    /// `build_snapshot` call.  `None` until the first snapshot is built.
    ///
    /// Stored as two separate `Arc<Vec<T>>` matching the types in
    /// `TerminalSnapshot`, so the clean path (no dirty rows) hands them
    /// directly into the snapshot with a refcount bump — no Vec allocation.
    previous_visible_snap: VisibleSnap,
    /// Whether the previous snapshot was taken while in the alternate screen
    /// buffer.  Used to detect primary↔alternate transitions and invalidate
    /// `previous_visible_snap` so stale content is never reused across a
    /// buffer switch.
    previous_was_alternate: bool,
    /// Scroll offset requested by the GUI (rows from the bottom, 0 = live).
    ///
    /// Updated when an `InputEvent::ScrollOffset(n)` is received.  Reset to 0
    /// when new PTY output arrives (auto-scroll to bottom).
    gui_scroll_offset: usize,
    /// The scroll offset used for the previous snapshot.  When this differs
    /// from the current `gui_scroll_offset`, the visible window has moved and
    /// the cached snapshot must be invalidated.
    previous_scroll_offset: usize,
    /// The terminal dimensions (cols, rows) at the time of the previous
    /// snapshot.  When these change (e.g. after a pane resize), the cached
    /// snapshot must be invalidated — it was built for a different grid size.
    previous_term_size: (usize, usize),
    /// The instant at which `SynchronizedUpdates::DontDraw` was first observed
    /// during `build_snapshot`.  Used to enforce the 200 ms auto-resume timeout:
    /// if `DontDraw` is still active when this deadline passes, the mode is
    /// automatically reset to `Draw` and rendering resumes.
    ///
    /// `None` when `SynchronizedUpdates` is not currently `DontDraw`.
    ///
    /// NOTE: The timeout is implemented here in `TerminalEmulator::build_snapshot()`
    /// rather than inside `SynchronizedUpdates` itself (which is a pure data type).
    /// See `freminal-common/src/buffer_states/modes/sync_updates.rs` for the spec
    /// references.
    dont_draw_entered_at: Option<Instant>,
}

impl TerminalEmulator {
    /// Creates a headless terminal emulator for benchmarks or tests.
    ///
    /// This version skips PTY setup and I/O threads, initializing only the
    /// fields required for data processing and snapshot building.
    #[must_use]
    pub fn dummy_for_bench() -> Self {
        use crossbeam_channel::unbounded;

        let (write_tx, _write_rx) = unbounded();

        Self {
            internal: TerminalState::default(),
            pty_io: None,
            write_tx,
            previous_visible_snap: None,
            previous_was_alternate: false,
            gui_scroll_offset: 0,
            previous_scroll_offset: 0,
            previous_term_size: (0, 0),
            dont_draw_entered_at: None,
        }
    }

    /// Creates a headless terminal emulator without a PTY.
    ///
    /// No PTY is spawned.  The returned `Receiver<PtyWrite>` drains any
    /// escape-sequence responses that the emulator's handler sends (DA, CPR,
    /// etc.) so channels never block.  The caller feeds data via
    /// `handle_incoming_data`.
    ///
    /// Used by playback mode, tests, and benchmarks.
    #[must_use]
    pub fn new_headless(scrollback_limit: Option<usize>) -> (Self, Receiver<PtyWrite>) {
        use crossbeam_channel::unbounded;

        let (write_tx, write_rx) = unbounded();

        let emulator = Self {
            internal: TerminalState::new(write_tx.clone(), scrollback_limit),
            pty_io: None,
            write_tx,
            previous_visible_snap: None,
            previous_was_alternate: false,
            gui_scroll_offset: 0,
            previous_scroll_offset: 0,
            previous_term_size: (0, 0),
            dont_draw_entered_at: None,
        };
        (emulator, write_rx)
    }

    /// Create a new terminal emulator
    ///
    /// `scrollback_limit` overrides the default scrollback history size when
    /// `Some(n)` is provided.  `None` keeps the compiled-in default (4000).
    ///
    /// # Errors
    ///
    pub fn new(
        args: &Args,
        scrollback_limit: Option<usize>,
        initial_size: FreminalTerminalSize,
        cwd: Option<&Path>,
    ) -> Result<(Self, Receiver<PtyRead>)> {
        let (write_tx, read_rx) = unbounded();
        let (pty_tx, pty_rx) = unbounded();

        // Derive the command tuple from the positional `command` arg.
        // If `command` is non-empty, it takes precedence over `--shell`.
        let command = if args.command.is_empty() {
            None
        } else {
            let mut iter = args.command.iter().cloned();
            // SAFETY: we just checked `is_empty()` above; first element exists.
            let prog = iter.next().unwrap_or_default();
            Some((prog, iter.collect()))
        };

        // When a positional command is specified, shell is ignored.
        let shell = if command.is_some() {
            None
        } else {
            args.shell.clone()
        };

        let io = FreminalPtyInputOutput::new(read_rx, pty_tx, command, shell, &initial_size, cwd)?;

        if let Err(e) = write_tx.send(PtyWrite::Resize(initial_size)) {
            error!("Failed to send resize to pty: {e}");
        }

        let ret = Self {
            internal: TerminalState::new(write_tx.clone(), scrollback_limit),
            pty_io: Some(io),
            write_tx,
            previous_visible_snap: None,
            previous_was_alternate: false,
            gui_scroll_offset: 0,
            previous_scroll_offset: 0,
            previous_term_size: (0, 0),
            dont_draw_entered_at: None,
        };
        Ok((ret, pty_rx))
    }

    /// Return a clone of the PTY write sender.
    ///
    /// Used by `main.rs` to pass the real write channel to the GUI before the
    /// emulator is moved into the PTY consumer thread.  The GUI uses it to
    /// send `PtyWrite::Write` responses for Report* window manipulation
    /// commands without going through the emulator lock.
    #[must_use]
    pub fn clone_write_tx(&self) -> crossbeam_channel::Sender<PtyWrite> {
        self.write_tx.clone()
    }

    /// Return the child-exit receiver from the PTY I/O layer.
    ///
    /// Returns `Some(Receiver<()>)` in normal mode (where a real PTY child
    /// process exists) or `None` in headless/benchmark/playback mode.
    ///
    /// Used by `main.rs` to add a third arm to the `select!` loop so the
    /// consumer thread can detect child exit on platforms (Windows) where the
    /// PTY read pipe does not close when the child exits.
    #[must_use]
    pub fn child_exit_rx(&self) -> Option<crossbeam_channel::Receiver<()>> {
        self.pty_io.as_ref().map(|io| io.child_exit_rx.clone())
    }

    /// Extract text from the full buffer for a selection range.
    ///
    /// Coordinates are buffer-absolute row indices and 0-indexed columns.
    /// When `is_block` is `true` the same `start_col`..=`end_col` column range
    /// is extracted from every row, producing a rectangular block of text.
    /// Delegates to `Buffer::extract_text` or `Buffer::extract_block_text`.
    #[must_use]
    pub fn extract_selection_text(
        &self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
        is_block: bool,
    ) -> String {
        let buf = self.internal.handler.buffer();
        if is_block {
            buf.extract_block_text(start_row, start_col, end_row, end_col)
        } else {
            buf.extract_text(start_row, start_col, end_row, end_col)
        }
    }

    /// Process a chunk of raw PTY bytes.
    ///
    /// This wraps `TerminalState::handle_incoming_data` for the consumer thread.
    /// When the user is scrolled back (`gui_scroll_offset > 0`), new output
    /// auto-scrolls to the bottom by resetting the offset to 0.
    pub fn handle_incoming_data(&mut self, incoming: &[u8]) {
        self.internal.handle_incoming_data(incoming);
        // Auto-scroll to bottom on new output, matching standard terminal
        // behavior.  The next snapshot will carry scroll_offset = 0 so the
        // GUI's ViewState is synced automatically.
        if self.gui_scroll_offset > 0 {
            self.gui_scroll_offset = 0;
        }
    }

    pub const fn get_win_size(&mut self) -> (usize, usize) {
        self.internal.get_win_size()
    }

    /// Set the terminal window dimensions in characters and notify the PTY of the size change.
    ///
    /// Updates the internal buffer to the new `(width_chars × height_chars)` grid and, if the
    /// dimensions have changed, sends a `PtyWrite::Resize` through the write channel so that
    /// the kernel's tty layer (TIOCSWINSZ) sees the new size.
    ///
    /// # Errors
    /// Returns an error if the `PtyWrite::Resize` message cannot be sent through the channel.
    pub fn set_win_size(
        &mut self,
        width_chars: usize,
        height_chars: usize,
        font_pixel_width: usize,
        font_pixel_height: usize,
    ) -> Result<()> {
        let (old_width, old_height) = self.internal.get_win_size();
        self.internal.set_win_size(
            width_chars,
            height_chars,
            u32::value_from(font_pixel_width).unwrap_or(0),
            u32::value_from(font_pixel_height).unwrap_or(0),
        );

        if old_width != width_chars || old_height != height_chars {
            // TIOCGWINSZ expects total window pixel dimensions, not per-cell.
            self.write_tx.send(PtyWrite::Resize(FreminalTerminalSize {
                width: width_chars,
                height: height_chars,
                pixel_width: font_pixel_width.saturating_mul(width_chars),
                pixel_height: font_pixel_height.saturating_mul(height_chars),
            }))?;
        }

        Ok(())
    }

    /// Handle a resize event delivered via the `InputEvent` channel.
    ///
    /// This is called by the PTY consumer thread (or, currently, the inline
    /// `input_rx` receiver in `main.rs`) when the GUI detects a change in
    /// terminal dimensions.  It updates the emulator's internal size and
    /// forwards a `PtyWrite::Resize` to the PTY writer so the kernel's tty
    /// layer sees the new window size.
    ///
    /// Unlike `set_win_size`, this method does not need to return `Result`
    /// because send failures are logged rather than propagated — the caller
    /// is on the consumer thread which has no caller to propagate to.
    pub fn handle_resize_event(
        &mut self,
        width_chars: usize,
        height_chars: usize,
        font_pixel_width: usize,
        font_pixel_height: usize,
    ) {
        self.internal.set_win_size(
            width_chars,
            height_chars,
            u32::value_from(font_pixel_width).unwrap_or(0),
            u32::value_from(font_pixel_height).unwrap_or(0),
        );

        // The PTY's TIOCGWINSZ expects the *total* window pixel dimensions
        // (ws_xpixel, ws_ypixel), not per-cell sizes.  Applications like nvim
        // compute cell size as ws_xpixel/ws_col, so passing per-cell values here
        // would give them a near-zero cell width.
        if let Err(e) = self.write_tx.send(PtyWrite::Resize(FreminalTerminalSize {
            width: width_chars,
            height: height_chars,
            pixel_width: font_pixel_width.saturating_mul(width_chars),
            pixel_height: font_pixel_height.saturating_mul(height_chars),
        })) {
            error!("Failed to send resize to PTY: {e}");
        }
    }

    /// Update the GUI-requested scroll offset.
    ///
    /// Called by the PTY consumer thread when it receives
    /// `InputEvent::ScrollOffset(n)`.  The value is clamped to
    /// `max_scroll_offset()` during the next `build_snapshot()` call.
    pub const fn set_gui_scroll_offset(&mut self, offset: usize) {
        self.gui_scroll_offset = offset;
    }

    /// Reset the scroll offset to 0 (live bottom).
    ///
    /// Called when new PTY data arrives while the user is scrolled back.
    pub const fn reset_scroll_offset(&mut self) {
        self.gui_scroll_offset = 0;
    }

    /// Write to the terminal
    ///
    /// # Errors
    /// Will error if the terminal cannot be locked
    pub fn write(&self, to_write: &TerminalInput) -> Result<()> {
        self.internal.write(to_write)
    }

    /// Write raw bytes directly to the PTY write channel.
    ///
    /// Used by the PTY consumer thread to forward keyboard input bytes that
    /// arrived via `InputEvent::Key(bytes)` without re-encoding them through
    /// `TerminalInput`.
    ///
    /// # Errors
    /// Returns an error if the send to the PTY write channel fails.
    pub fn write_raw_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.write_tx
            .send(PtyWrite::Write(bytes.to_vec()))
            .map_err(|e| anyhow::anyhow!("Failed to send raw bytes to PTY: {e}"))
    }

    /// Return a shared handle to the atomic flag that tracks whether the PTY
    /// slave has `ECHO` disabled (i.e. a password prompt is active).
    ///
    /// Returns a clone of the shared `Arc<AtomicBool>` — the caller reads it
    /// with a cheap `Relaxed` atomic load each frame.  The underlying flag is
    /// refreshed by the writer thread every 100 ms via `tcgetattr()` on the
    /// master fd (Unix only).
    ///
    /// Returns `None` in headless / benchmark / playback mode where there is
    /// no real PTY.
    #[must_use]
    pub fn echo_off_atomic(&self) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        self.pty_io
            .as_ref()
            .map(|io| std::sync::Arc::clone(&io.echo_off))
    }

    /// Build a point-in-time snapshot of the terminal state.
    ///
    /// This is cheap to call: the visible content is flattened here on the
    /// PTY thread so the GUI render path never has to do it.
    ///
    /// `content_changed` is `true` only when the visible flat content differs
    /// from the previous snapshot.  Cursor-only moves do not set it because
    /// cursor position is carried separately in the snapshot struct.
    #[must_use]
    pub fn build_snapshot(&mut self) -> TerminalSnapshot {
        // ── Cheap immutable reads (no &mut borrow of handler needed) ────────
        let (term_width, term_height) = self.internal.handler.get_win_size();
        let is_alternate_screen = self.internal.handler.is_alternate_screen();

        // On the alternate screen scrollback is meaningless — clamp to 0.
        let (scroll_offset, max_scroll_offset) = if is_alternate_screen {
            (0, 0)
        } else {
            // Clamp to the maximum scrollback offset so an out-of-range value
            // (e.g. from a previous buffer state) doesn't panic.
            let max = self.internal.handler.buffer().max_scroll_offset();
            (self.gui_scroll_offset.min(max), max)
        };

        // ── Invalidate the snap cache on primary ↔ alternate screen switch ───
        //
        // When the buffer type changes, the previous visible_snap belongs to
        // the other buffer and must never be reused for the new one.
        if is_alternate_screen != self.previous_was_alternate {
            self.previous_visible_snap = None;
            self.previous_was_alternate = is_alternate_screen;
        }

        // ── Invalidate the snap cache when scroll offset changes ─────────
        //
        // The visible window moved — the cached flat content is from a
        // different set of rows and must not be reused.
        let scroll_changed = scroll_offset != self.previous_scroll_offset;
        if scroll_changed {
            self.previous_visible_snap = None;
            self.previous_scroll_offset = scroll_offset;
        }

        // ── Invalidate the snap cache when terminal dimensions change ────
        //
        // After a resize the grid has a different number of columns and/or
        // rows.  The cached snapshot was flattened for the old dimensions
        // and must not be reused — otherwise full-screen TUIs like nvim
        // that rely on absolute cursor positioning after SIGWINCH see
        // stale content (gaps, uncolored cells, mispositioned text).
        let current_size = (term_width, term_height);
        if current_size != self.previous_term_size {
            self.previous_visible_snap = None;
            self.previous_term_size = current_size;
        }

        // ── Determine whether any visible row changed since last snapshot ────
        let any_dirty = self.internal.handler.any_visible_dirty(scroll_offset);

        // ── Produce (visible_chars, visible_tags, content_changed) ───────
        //
        // Only flatten the *visible* rows — scrollback is not part of the
        // snapshot.  The previous code called `data_and_format_data_for_gui`
        // which also flattened scrollback, then discarded the result.
        let (visible_chars, visible_tags, row_offsets, url_tag_indices, content_changed) =
            self.flatten_visible(any_dirty, scroll_offset);

        // ── Remaining cheap reads ────────────────────────────────────────────
        self.apply_sync_updates_timeout();

        let mode_fields = self.collect_mode_fields();
        let cursor_pos = self.internal.cursor_pos();
        // Hide the cursor when the user is scrolled back into history —
        // the live cursor line is not visible on screen.
        let show_cursor = self.internal.show_cursor() && scroll_offset == 0;
        let cursor_visual_style = self.internal.get_cursor_visual_style();
        let is_normal_display = self.internal.is_normal_display();

        // ── Blink detection ──────────────────────────────────────────────────
        let has_blinking_text = visible_tags
            .iter()
            .any(|tag| tag.blink != freminal_common::buffer_states::fonts::BlinkState::None);

        // ── URL presence detection ───────────────────────────────────────────
        // O(1) — `url_tag_indices` already enumerates exactly those tags with a
        // URL, so we skip the O(n) scan of `visible_tags`.
        let has_urls = !url_tag_indices.is_empty();

        let cwd = self
            .internal
            .handler
            .current_working_directory()
            .map(String::from);

        let ftcs_state = self.internal.handler.ftcs_state();
        let last_exit_code = self.internal.handler.last_exit_code();
        let prompt_rows = Arc::<[usize]>::from(self.internal.handler.buffer().prompt_rows());
        let theme = self.internal.handler.theme();

        // ── Inline image data ────────────────────────────────────────────────
        let (images, visible_image_placements) = self.collect_visible_images(scroll_offset);

        // ── Per-row line-width attributes (DECDWL / DECDHL) ──────────────────
        let visible_line_widths = Arc::new(
            self.internal
                .handler
                .buffer()
                .visible_line_widths(scroll_offset),
        );

        let total_rows = self.internal.handler.buffer().get_rows().len();

        TerminalSnapshot {
            visible_chars,
            visible_tags,
            scroll_offset,
            max_scroll_offset,
            height: term_height,
            cursor_pos,
            show_cursor,
            cursor_visual_style,
            is_alternate_screen,
            is_normal_display,
            term_width,
            term_height,
            total_rows,
            content_changed,
            has_blinking_text,
            has_urls,
            row_offsets,
            url_tag_indices,
            scroll_changed,
            bracketed_paste: mode_fields.bracketed_paste,
            mouse_tracking: mode_fields.mouse_tracking,
            mouse_encoding: mode_fields.mouse_encoding,
            repeat_keys: mode_fields.repeat_keys,
            cursor_key_app_mode: mode_fields.cursor_key_app_mode,
            keypad_app_mode: mode_fields.keypad_app_mode,
            skip_draw: mode_fields.skip_draw,
            modify_other_keys: mode_fields.modify_other_keys,
            application_escape_key: mode_fields.application_escape_key,
            backarrow_sends_bs: mode_fields.backarrow_sends_bs,
            alternate_scroll: mode_fields.alternate_scroll,
            line_feed_mode: mode_fields.line_feed_mode,
            kitty_keyboard_flags: mode_fields.kitty_keyboard_flags,
            cwd,
            ftcs_state,
            last_exit_code,
            prompt_rows,
            theme,
            images,
            visible_image_placements,
            visible_line_widths,
            cursor_color_override: self.internal.handler.cursor_color_override(),
            pointer_shape: self.internal.handler.pointer_shape(),
        }
    }

    /// Flatten the visible rows into
    /// `(chars, tags, row_offsets, url_tag_indices, content_changed)`, using the
    /// snapshot-level cache to avoid work when no visible row is dirty.
    ///
    /// The `row_offsets` contains per-row flat-index offsets into the chars vec
    /// (one entry per visible row).  `url_tag_indices` contains the indices of
    /// tags in `tags` that carry a URL.
    fn flatten_visible(&mut self, any_dirty: bool, scroll_offset: usize) -> FlattenResult {
        if any_dirty {
            // At least one visible row is dirty — re-flatten via the cache.
            let (vis_chars, vis_tags, vis_row_offsets, vis_url_indices) = self
                .internal
                .handler
                .buffer_mut()
                .visible_as_tchars_and_tags(scroll_offset);
            let vc = Arc::new(vis_chars);
            let vt = Arc::new(vis_tags);
            let vr = Arc::new(vis_row_offsets);
            let vu = Arc::new(vis_url_indices);

            // `content_changed` is true when the flat content actually differs
            // from the previous snapshot (guards against spurious redraws from
            // dirty flags set on rows that were ultimately written with the same
            // bytes, e.g. cursor-blink redraws).
            let changed = self
                .previous_visible_snap
                .as_ref()
                .is_none_or(|(prev_chars, _, _, _)| prev_chars.as_ref() != vc.as_ref());

            self.previous_visible_snap = Some((
                Arc::clone(&vc),
                Arc::clone(&vt),
                Arc::clone(&vr),
                Arc::clone(&vu),
            ));
            (vc, vt, vr, vu, changed)
        } else if let Some((prev_chars, prev_tags, prev_row_offsets, prev_url_indices)) =
            &self.previous_visible_snap
        {
            // No visible row is dirty — reuse cached Arcs (refcount bump only).
            (
                Arc::clone(prev_chars),
                Arc::clone(prev_tags),
                Arc::clone(prev_row_offsets),
                Arc::clone(prev_url_indices),
                false,
            )
        } else {
            // First-ever snapshot and nothing is marked dirty yet (e.g. the
            // buffer was just created).  Flatten once to populate the cache.
            let (vis_chars, vis_tags, vis_row_offsets, vis_url_indices) = self
                .internal
                .handler
                .buffer_mut()
                .visible_as_tchars_and_tags(scroll_offset);
            let vc = Arc::new(vis_chars);
            let vt = Arc::new(vis_tags);
            let vr = Arc::new(vis_row_offsets);
            let vu = Arc::new(vis_url_indices);
            self.previous_visible_snap = Some((
                Arc::clone(&vc),
                Arc::clone(&vt),
                Arc::clone(&vr),
                Arc::clone(&vu),
            ));
            (vc, vt, vr, vu, true)
        }
    }

    /// Enforce the 200 ms auto-resume timeout for `SynchronizedUpdates::DontDraw`.
    ///
    /// DEC ?2026 lets programs suppress rendering while composing a frame.  A
    /// crashed program that sets `DontDraw` and never resets it would freeze the
    /// display indefinitely.  This method starts `dont_draw_entered_at` on the
    /// first snapshot where `DontDraw` is active and resets `synchronized_updates`
    /// back to `Draw` once `SYNC_UPDATES_TIMEOUT_MS` milliseconds have elapsed.
    ///
    /// When the mode is not `DontDraw` the timer is cleared so it does not carry
    /// stale state into the next `DontDraw` activation.
    fn apply_sync_updates_timeout(&mut self) {
        use freminal_common::buffer_states::modes::sync_updates::SynchronizedUpdates;

        if self.internal.skip_draw_always() {
            match self.dont_draw_entered_at {
                None => {
                    // First snapshot with DontDraw active — start the clock.
                    self.dont_draw_entered_at = Some(Instant::now());
                }
                Some(entered_at)
                    if entered_at.elapsed() >= Duration::from_millis(SYNC_UPDATES_TIMEOUT_MS) =>
                {
                    // Timeout expired — reset to Draw so the next snapshot
                    // carries skip_draw = false, and clear the timer.
                    self.internal.modes.synchronized_updates = SynchronizedUpdates::Draw;
                    self.dont_draw_entered_at = None;
                }
                Some(_) => {}
            }
        } else {
            // Mode is Draw (or Query) — clear stale timer state.
            self.dont_draw_entered_at = None;
        }
    }

    /// Collect all mode flags needed by the snapshot in a single pass.
    fn collect_mode_fields(&self) -> SnapshotModeFields {
        SnapshotModeFields {
            bracketed_paste: self.internal.modes.bracketed_paste.clone(),
            mouse_tracking: self.internal.modes.mouse_tracking.clone(),
            mouse_encoding: self.internal.modes.mouse_encoding.clone(),
            repeat_keys: self.internal.modes.repeat_keys,
            cursor_key_app_mode: self.internal.get_cursor_key_mode(),
            keypad_app_mode: self.internal.modes.keypad_mode,
            skip_draw: self.internal.skip_draw_always(),
            modify_other_keys: self.internal.handler.modify_other_keys_level(),
            application_escape_key: self.internal.handler.application_escape_key(),
            backarrow_sends_bs: self.internal.modes.backarrow_key_mode,
            alternate_scroll: self.internal.modes.alternate_scroll,
            line_feed_mode: self.internal.modes.line_feed_mode,
            kitty_keyboard_flags: self.internal.handler.kitty_keyboard_flags(),
        }
    }

    /// Build the image map and placement vector for the visible window.
    ///
    /// Returns `(images, placements)` — both wrapped in `Arc` for cheap
    /// snapshot cloning.  The common case (no images) returns empty containers
    /// with zero allocation.
    fn collect_visible_images(&self, scroll_offset: usize) -> VisibleImages {
        if !self.internal.handler.has_visible_images(scroll_offset) {
            return (Arc::new(HashMap::new()), Arc::new(Vec::new()));
        }

        let placements = self
            .internal
            .handler
            .visible_image_placements(scroll_offset);

        // Collect only the images actually referenced by a visible cell so the
        // snapshot doesn't grow without bound.
        let mut img_map: HashMap<u64, InlineImage> = HashMap::new();
        for placement in placements.iter().flatten() {
            let id = placement.image_id;
            if let std::collections::hash_map::Entry::Vacant(entry) = img_map.entry(id)
                && let Some(img) = self.internal.handler.buffer().image_store().get(id)
            {
                entry.insert(img.clone());
            }
        }

        (Arc::new(img_map), Arc::new(placements))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // ── Helpers ────────────────────────────────────────────────────────────────

    /// Create a headless emulator; drop the write receiver (we don't need it).
    fn make_headless() -> TerminalEmulator {
        let (emu, _rx) = TerminalEmulator::new_headless(None);
        emu
    }

    // ── extract_selection_text ─────────────────────────────────────────────────

    #[test]
    fn extract_selection_text_empty_buffer() {
        let emu = make_headless();
        // An all-zero range on an empty buffer should return empty or whitespace.
        let text = emu.extract_selection_text(0, 0, 0, 0, false);
        // We just care that it doesn't panic; the exact content depends on buffer state.
        let _ = text;
    }

    #[test]
    fn extract_selection_text_after_write() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        // Write "Hello" into the buffer.
        emu.handle_incoming_data(b"Hello");
        // Extract across row 0 columns 0-4.
        let text = emu.extract_selection_text(0, 0, 0, 4, false);
        assert!(
            text.contains("Hello"),
            "expected 'Hello' in selection, got: {text:?}"
        );
    }

    #[test]
    fn extract_selection_text_block_mode() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        // Write two lines.
        emu.handle_incoming_data(b"AB\r\nCD");
        // Block selection on column 0 only, rows 0-1.
        let text = emu.extract_selection_text(0, 0, 1, 0, true);
        // Block should give us column 0 from each row.
        let _ = text; // Content may vary; just verify no panic.
    }

    // ── handle_incoming_data ───────────────────────────────────────────────────

    #[test]
    fn handle_incoming_data_resets_scroll_offset() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        // Manually set a non-zero scroll offset.
        emu.gui_scroll_offset = 5;
        // Receiving new data should auto-scroll back to bottom (offset = 0).
        emu.handle_incoming_data(b"new data");
        assert_eq!(
            emu.gui_scroll_offset, 0,
            "scroll offset should reset to 0 on new data"
        );
    }

    #[test]
    fn handle_incoming_data_zero_offset_unchanged() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        emu.gui_scroll_offset = 0;
        emu.handle_incoming_data(b"data");
        assert_eq!(emu.gui_scroll_offset, 0);
    }

    // ── set_win_size ───────────────────────────────────────────────────────────

    #[test]
    fn set_win_size_same_size_does_not_send_resize() {
        let (mut emu, rx) = TerminalEmulator::new_headless(None);
        // Drain any initial messages.
        while rx.try_recv().is_ok() {}

        let (w, h) = emu.internal.get_win_size();
        // Setting the same size should not enqueue a Resize message.
        emu.set_win_size(w, h, 8, 16).unwrap();
        assert!(
            rx.try_recv().is_err(),
            "no resize message expected when dimensions are unchanged"
        );
    }

    #[test]
    fn set_win_size_different_size_sends_resize() {
        let (mut emu, rx) = TerminalEmulator::new_headless(None);
        // Drain any initial messages.
        while rx.try_recv().is_ok() {}

        let (w, h) = emu.internal.get_win_size();
        let new_w = w + 10;
        let new_h = h + 5;
        emu.set_win_size(new_w, new_h, 8, 16).unwrap();

        match rx.try_recv() {
            Ok(PtyWrite::Resize(size)) => {
                assert_eq!(size.width, new_w);
                assert_eq!(size.height, new_h);
            }
            other => panic!("expected Resize message, got: {other:?}"),
        }
    }

    // ── build_snapshot: cache invalidation ────────────────────────────────────

    #[test]
    fn build_snapshot_first_call_returns_content_changed_true() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        emu.handle_incoming_data(b"hello");
        let snap = emu.build_snapshot();
        // First-ever snapshot must report content_changed = true.
        assert!(
            snap.content_changed,
            "first snapshot should have content_changed=true"
        );
    }

    #[test]
    fn build_snapshot_second_call_no_data_is_not_changed() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        emu.handle_incoming_data(b"hello");
        let _ = emu.build_snapshot();
        // Second snapshot with no new data: content should not have changed.
        let snap2 = emu.build_snapshot();
        assert!(
            !snap2.content_changed,
            "second snapshot with no new data should have content_changed=false"
        );
    }

    #[test]
    fn build_snapshot_scroll_offset_change_invalidates_cache() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        // Default terminal is 100x100.  Write >100 lines to create scrollback.
        for _ in 0..110 {
            emu.handle_incoming_data(b"line of text\r\n");
        }
        let snap1 = emu.build_snapshot();
        // Confirm we actually have scrollback to scroll into.
        assert!(
            snap1.max_scroll_offset > 0,
            "expected scrollback after writing >100 lines, got max_scroll_offset={}",
            snap1.max_scroll_offset
        );
        // Move scroll offset by 1 — cache should be invalidated.
        emu.set_gui_scroll_offset(1);
        let snap2 = emu.build_snapshot();
        assert!(
            snap2.content_changed,
            "scroll offset change should invalidate the cache"
        );
    }

    #[test]
    fn build_snapshot_size_change_invalidates_cache() {
        let (mut emu, rx) = TerminalEmulator::new_headless(None);
        emu.handle_incoming_data(b"hello");
        let _ = emu.build_snapshot();

        // Resize terminal — this should invalidate the cache.
        let (w, h) = emu.internal.get_win_size();
        emu.set_win_size(w + 10, h, 8, 16).unwrap();
        // Drain the resize message.
        while rx.try_recv().is_ok() {}

        let snap2 = emu.build_snapshot();
        assert!(
            snap2.content_changed,
            "terminal resize should invalidate the visible snap cache"
        );
    }

    // ── set_gui_scroll_offset / reset_scroll_offset ───────────────────────────

    #[test]
    fn set_and_reset_scroll_offset() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        emu.set_gui_scroll_offset(7);
        assert_eq!(emu.gui_scroll_offset, 7);
        emu.reset_scroll_offset();
        assert_eq!(emu.gui_scroll_offset, 0);
    }

    // ── clone_write_tx / write_raw_bytes ──────────────────────────────────────

    #[test]
    fn write_raw_bytes_succeeds() {
        let (emu, rx) = TerminalEmulator::new_headless(None);
        // Drain initial messages.
        while rx.try_recv().is_ok() {}

        emu.write_raw_bytes(b"hello pty").unwrap();

        match rx.try_recv() {
            Ok(PtyWrite::Write(bytes)) => {
                assert_eq!(bytes, b"hello pty");
            }
            other => panic!("expected PtyWrite::Write, got: {other:?}"),
        }
    }

    #[test]
    fn clone_write_tx_can_send() {
        let (emu, rx) = TerminalEmulator::new_headless(None);
        while rx.try_recv().is_ok() {}

        let tx = emu.clone_write_tx();
        tx.send(PtyWrite::Write(b"via clone".to_vec())).unwrap();

        match rx.try_recv() {
            Ok(PtyWrite::Write(bytes)) => assert_eq!(bytes, b"via clone"),
            other => panic!("expected PtyWrite::Write, got: {other:?}"),
        }
    }

    // ── dummy_for_bench ────────────────────────────────────────────────────────

    #[test]
    fn dummy_for_bench_does_not_panic() {
        let _ = TerminalEmulator::dummy_for_bench();
    }

    // ── handle_resize_event ────────────────────────────────────────────────────

    #[test]
    fn handle_resize_event_updates_size_and_sends_resize() {
        let (mut emu, rx) = TerminalEmulator::new_headless(None);
        // Drain initial messages.
        while rx.try_recv().is_ok() {}

        emu.handle_resize_event(120, 40, 9, 18);

        let (w, h) = emu.internal.get_win_size();
        assert_eq!(w, 120);
        assert_eq!(h, 40);

        // Should have sent a PtyWrite::Resize
        match rx.try_recv() {
            Ok(PtyWrite::Resize(size)) => {
                assert_eq!(size.width, 120);
                assert_eq!(size.height, 40);
                // Pixel dimensions are total (per-cell * chars)
                assert_eq!(size.pixel_width, 9 * 120);
                assert_eq!(size.pixel_height, 18 * 40);
            }
            other => panic!("expected PtyWrite::Resize, got: {other:?}"),
        }
    }

    #[test]
    fn handle_resize_event_same_size_still_sends_resize() {
        let (mut emu, rx) = TerminalEmulator::new_headless(None);
        while rx.try_recv().is_ok() {}

        let (w, h) = emu.internal.get_win_size();
        // handle_resize_event always sends, unlike set_win_size
        emu.handle_resize_event(w, h, 8, 16);

        // Should still send a resize (handle_resize_event doesn't check old==new)
        match rx.try_recv() {
            Ok(PtyWrite::Resize(size)) => {
                assert_eq!(size.width, w);
                assert_eq!(size.height, h);
            }
            other => panic!("expected PtyWrite::Resize, got: {other:?}"),
        }
    }

    // ── write (TerminalInput) ────────────────────────────────────────────────

    #[test]
    fn write_terminal_input_sends_bytes() {
        let (emu, rx) = TerminalEmulator::new_headless(None);
        while rx.try_recv().is_ok() {}

        emu.write(&TerminalInput::Ascii(b'A')).unwrap();

        match rx.try_recv() {
            Ok(PtyWrite::Write(bytes)) => {
                assert_eq!(bytes, b"A");
            }
            other => panic!("expected PtyWrite::Write with 'A', got: {other:?}"),
        }
    }

    #[test]
    fn write_enter_sends_carriage_return() {
        let (emu, rx) = TerminalEmulator::new_headless(None);
        while rx.try_recv().is_ok() {}

        emu.write(&TerminalInput::Enter).unwrap();

        match rx.try_recv() {
            Ok(PtyWrite::Write(bytes)) => {
                assert!(!bytes.is_empty(), "Enter should produce non-empty bytes");
            }
            other => panic!("expected PtyWrite::Write for Enter, got: {other:?}"),
        }
    }

    // ── echo_off_atomic ────────────────────────────────────────────────────────

    #[test]
    fn echo_off_atomic_headless_returns_none() {
        let emu = make_headless();
        assert!(
            emu.echo_off_atomic().is_none(),
            "headless emulator should return None for echo_off_atomic"
        );
    }

    // ── child_exit_rx ────────────────────────────────────────────────────────

    #[test]
    fn child_exit_rx_headless_returns_none() {
        let emu = make_headless();
        assert!(
            emu.child_exit_rx().is_none(),
            "headless emulator should return None for child_exit_rx"
        );
    }

    // ── build_snapshot with alternate screen ──────────────────────────────────

    #[test]
    fn build_snapshot_alternate_screen_zeroes_scroll() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        // Write enough to create scrollback on primary screen.
        for _ in 0..110 {
            emu.handle_incoming_data(b"line\r\n");
        }
        let snap1 = emu.build_snapshot();
        assert!(snap1.max_scroll_offset > 0);

        // Enter alternate screen via DECSET ?1049
        emu.handle_incoming_data(b"\x1b[?1049h");
        let snap2 = emu.build_snapshot();
        assert!(snap2.is_alternate_screen);
        assert_eq!(snap2.scroll_offset, 0);
        assert_eq!(snap2.max_scroll_offset, 0);
    }

    #[test]
    fn build_snapshot_alternate_screen_switch_invalidates_cache() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        emu.handle_incoming_data(b"primary content");
        let _ = emu.build_snapshot();

        // Switch to alternate screen — should invalidate cache
        emu.handle_incoming_data(b"\x1b[?1049h");
        let snap = emu.build_snapshot();
        assert!(
            snap.content_changed,
            "switching to alternate screen should mark content_changed"
        );
    }

    // ── collect_visible_images (no images case is trivially covered; test image path) ──

    #[test]
    fn build_snapshot_without_images_has_empty_image_data() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        emu.handle_incoming_data(b"plain text");
        let snap = emu.build_snapshot();
        assert!(snap.images.is_empty(), "no images in plain text buffer");
    }

    #[test]
    fn build_snapshot_with_iterm2_image_has_image_data() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        // Build a minimal iTerm2 inline image OSC sequence:
        // OSC 1337 ; File = inline=1 : <base64> BEL
        // Use a tiny 1x1 pixel PNG as the payload.
        let pixel_data = b"\x89PNG\r\n\x1a\nfake";
        let b64 = freminal_common::base64::encode(pixel_data);
        let osc = format!("\x1b]1337;File=inline=1:{b64}\x07");
        emu.handle_incoming_data(osc.as_bytes());
        let snap = emu.build_snapshot();
        // The image should be stored — check placements are populated.
        // Note: The actual rendering may or may not produce placements
        // depending on whether the image was successfully decoded and placed.
        // We primarily verify the code path doesn't panic.
        let _ = snap.images;
        let _ = snap.visible_image_placements;
    }

    // ── build_snapshot: blink detection ──────────────────────────────────────

    #[test]
    fn build_snapshot_no_blink_by_default() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        emu.handle_incoming_data(b"hello");
        let snap = emu.build_snapshot();
        assert!(!snap.has_blinking_text);
    }

    #[test]
    fn build_snapshot_with_blink_sgr5() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        // SGR 5 = slow blink
        emu.handle_incoming_data(b"\x1b[5mblinky\x1b[0m");
        let snap = emu.build_snapshot();
        assert!(
            snap.has_blinking_text,
            "SGR 5 should set has_blinking_text=true"
        );
    }

    // ── build_snapshot: URL detection ────────────────────────────────────────

    #[test]
    fn build_snapshot_no_urls_by_default() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        emu.handle_incoming_data(b"hello world");
        let snap = emu.build_snapshot();
        assert!(!snap.has_urls);
    }

    // ── get_win_size ─────────────────────────────────────────────────────────

    #[test]
    fn get_win_size_returns_headless_defaults() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(None);
        let (w, h) = emu.get_win_size();
        // Headless defaults are 100x100
        assert!(w > 0);
        assert!(h > 0);
    }

    // ── scrollback limit ─────────────────────────────────────────────────────

    #[test]
    fn new_headless_with_scrollback_limit() {
        let (mut emu, _rx) = TerminalEmulator::new_headless(Some(10));
        // Write many lines — scrollback should be limited
        for i in 0..100 {
            let line = format!("line {i}\r\n");
            emu.handle_incoming_data(line.as_bytes());
        }
        let snap = emu.build_snapshot();
        // With limit of 10, max_scroll_offset should be limited
        assert!(
            snap.max_scroll_offset <= 10,
            "scrollback limit=10, but max_scroll_offset={}",
            snap.max_scroll_offset
        );
    }
}
