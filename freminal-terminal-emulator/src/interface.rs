// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::collections::HashMap;
use std::sync::Arc;

/// Cached flat representation of the visible window stored between snapshots.
///
/// Two separate `Arc<Vec<T>>` fields match the types in `TerminalSnapshot`
/// directly, so the clean path (no dirty rows) is a pair of refcount bumps
/// with no `Vec` allocation.
type VisibleSnap = Option<(Arc<Vec<TChar>>, Arc<Vec<FormatTag>>)>;

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
    decarm::Decarm, decbkm::Decbkm, decckm::Decckm, keypad::KeypadMode, mouse::MouseEncoding,
    mouse::MouseTrack, rl_bracket::RlBracket,
};

use freminal_common::{
    args::Args, buffer_states::tchar::TChar, terminal_size::DEFAULT_HEIGHT,
    terminal_size::DEFAULT_WIDTH,
};

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
        }
    }

    /// Creates a headless terminal emulator for playback mode.
    ///
    /// No PTY is spawned.  The returned `Receiver<PtyWrite>` drains any
    /// escape-sequence responses that the emulator's handler sends (DA, CPR,
    /// etc.) so channels never block.  The caller feeds recorded data via
    /// `handle_incoming_data`.
    #[must_use]
    pub fn new_for_playback(scrollback_limit: Option<usize>) -> (Self, Receiver<PtyWrite>) {
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
    pub fn new(args: &Args, scrollback_limit: Option<usize>) -> Result<(Self, Receiver<PtyRead>)> {
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

        let io =
            FreminalPtyInputOutput::new(read_rx, pty_tx, args.recording.clone(), command, shell)?;

        if let Err(e) = write_tx.send(PtyWrite::Resize(FreminalTerminalSize {
            width: DEFAULT_WIDTH as usize,
            height: DEFAULT_HEIGHT as usize,
            pixel_width: 0,
            pixel_height: 0,
        })) {
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
    /// Delegates to `Buffer::extract_text`.
    #[must_use]
    pub fn extract_selection_text(
        &self,
        start_row: usize,
        start_col: usize,
        end_row: usize,
        end_col: usize,
    ) -> String {
        self.internal
            .handler
            .buffer()
            .extract_text(start_row, start_col, end_row, end_col)
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

    /// Set the window title
    ///
    /// # Errors
    /// Will error if the terminal cannot be locked
    pub fn set_win_size(
        &mut self,
        width_chars: usize,
        height_chars: usize,
        font_pixel_width: usize,
        font_pixel_height: usize,
    ) -> Result<()> {
        let (old_width, old_height) = self.internal.get_win_size();
        #[allow(clippy::cast_possible_truncation)]
        self.internal.set_win_size(
            width_chars,
            height_chars,
            font_pixel_width as u32,
            font_pixel_height as u32,
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
        #[allow(clippy::cast_possible_truncation)]
        self.internal.set_win_size(
            width_chars,
            height_chars,
            font_pixel_width as u32,
            font_pixel_height as u32,
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

        // ── Determine whether any visible row changed since last snapshot ────
        let any_dirty = self.internal.handler.any_visible_dirty(scroll_offset);

        // ── Produce (visible_chars, visible_tags, content_changed) ───────
        let (visible_chars, visible_tags, content_changed) = if any_dirty {
            // At least one visible row is dirty — re-flatten via the cache.
            // `data_and_format_data_for_gui` calls `visible_as_tchars_and_tags`
            // which updates the per-row cache and clears dirty flags in one pass.
            let (chars, tags) = self.internal.data_and_format_data_for_gui(scroll_offset);
            let vc = Arc::new(chars.visible);
            let vt = Arc::new(tags.visible);

            // `content_changed` is true when the flat content actually differs
            // from the previous snapshot (guards against spurious redraws from
            // dirty flags set on rows that were ultimately written with the same
            // bytes, e.g. cursor-blink redraws).
            let changed = self
                .previous_visible_snap
                .as_ref()
                .is_none_or(|(prev_chars, _)| prev_chars.as_ref() != vc.as_ref());

            self.previous_visible_snap = Some((Arc::clone(&vc), Arc::clone(&vt)));
            (vc, vt, changed)
        } else if let Some((prev_chars, prev_tags)) = &self.previous_visible_snap {
            // No visible row is dirty — reuse cached Arcs.
            // This is a refcount bump only: no Vec allocation, no memcpy.
            (Arc::clone(prev_chars), Arc::clone(prev_tags), false)
        } else {
            // First-ever snapshot and nothing is marked dirty yet (e.g. the
            // buffer was just created).  Flatten once to populate the cache.
            let (chars, tags) = self.internal.data_and_format_data_for_gui(scroll_offset);
            let vc = Arc::new(chars.visible);
            let vt = Arc::new(tags.visible);
            self.previous_visible_snap = Some((Arc::clone(&vc), Arc::clone(&vt)));
            (vc, vt, true)
        };

        // ── Remaining cheap reads ────────────────────────────────────────────
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

        let cwd = self
            .internal
            .handler
            .current_working_directory()
            .map(String::from);

        let ftcs_state = self.internal.handler.ftcs_state();
        let last_exit_code = self.internal.handler.last_exit_code();
        let theme = self.internal.handler.theme();

        // ── Inline image data ────────────────────────────────────────────────
        let (images, visible_image_placements) = self.collect_visible_images(scroll_offset);

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
            cwd,
            ftcs_state,
            last_exit_code,
            theme,
            images,
            visible_image_placements,
            playback_info: None,
            cursor_color_override: self.internal.handler.cursor_color_override(),
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
