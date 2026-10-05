// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `TerminalSnapshot` — the lock-free data contract between the PTY processing
//! thread and the GUI thread.
//!
//! The PTY thread produces a fresh snapshot after every batch of processed data
//! and publishes it atomically via an `ArcSwap<TerminalSnapshot>`.  The GUI
//! thread loads the snapshot with a single atomic pointer load — no lock, no
//! blocking.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use freminal_buffer::image_store::{ImagePlacement, InlineImage};
use freminal_common::{
    buffer_states::{
        command_block::CommandBlock,
        cursor::CursorPos,
        format_tag::FormatTag,
        ftcs::FtcsState,
        modes::{
            alternate_scroll::AlternateScroll,
            application_escape_key::ApplicationEscapeKey,
            decarm::Decarm,
            decbkm::Decbkm,
            decckm::Decckm,
            keypad::KeypadMode,
            lnm::Lnm,
            mouse::{MouseEncoding, MouseTrack},
            rl_bracket::RlBracket,
        },
        pointer_shape::PointerShape,
        row_number::RowNumber,
        tchar::TChar,
    },
    cursor::CursorVisualStyle,
    themes::ThemePalette,
};

/// The stretch of the active screen's buffer that a snapshot, or a search
/// corpus cut from the same buffer, covers: which row is the oldest retained
/// one and how many rows are retained.
///
/// Two extents are equal only if they describe the same retained rows.
/// `total_rows` alone is **not** enough to tell: at scrollback capacity it
/// freezes while `row_base` keeps advancing by one per evicted row, so a
/// corpus fetched a moment ago has the same `total_rows` as the live buffer
/// but is made of different rows. Comparing the pair (Task 125.17) is what
/// lets staleness checks notice that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferExtent {
    /// Logical number of the oldest retained row (retained index `0`).
    pub row_base: RowNumber,
    /// Number of retained rows (scrollback + visible).
    pub total_rows: usize,
}

/// A point-in-time snapshot of the terminal state, ready for the GUI to render.
///
/// All expensive work (flattening rows → `Vec<TChar>` / `Vec<FormatTag>`) is
/// performed on the PTY thread so the GUI render path is allocation-free.
///
/// The snapshot is always immutable once constructed.  The GUI must never
/// mutate any field.
///
/// `visible_chars` and `visible_tags` are wrapped in `Arc` so that cloning a
/// snapshot (or handing the same content to a second snapshot on the clean
/// path) is a cheap atomic refcount increment rather than a full `Vec` copy.
#[allow(clippy::struct_excessive_bools)] // Eight independent rendering/bookkeeping bools; enums would add noise
#[derive(Debug, Clone)]
pub struct TerminalSnapshot {
    /// Flattened visible character content, already converted from `Row`/`Cell`.
    ///
    /// Produced once on the PTY side; the GUI reads it directly.
    ///
    /// Wrapped in `Arc` so passing the same content to a new snapshot (clean
    /// path — no visible rows changed) is a refcount bump, not a Vec copy.
    pub visible_chars: Arc<Vec<TChar>>,

    /// Format tags corresponding to `visible_chars`.
    ///
    /// Wrapped in `Arc` for the same reason as `visible_chars`.
    pub visible_tags: Arc<Vec<FormatTag>>,

    /// Current scroll offset (rows from the bottom, 0 = live view).
    ///
    /// The GUI reads this to stay in sync with the PTY thread's view.  When
    /// the PTY thread auto-scrolls to bottom on new output, this will be 0
    /// even if the GUI previously sent a non-zero offset.
    pub scroll_offset: usize,

    /// Maximum valid scroll offset (total scrollback rows above the visible
    /// window).  Used by the GUI to compute the scrollbar thumb position and
    /// size.  When `max_scroll_offset == 0` there is no scrollback history.
    pub max_scroll_offset: usize,

    /// Number of extra rows flattened **above** the normal visible window for
    /// command-block fold support.
    ///
    /// `visible_chars` / `visible_tags` / `row_offsets` / `visible_line_widths`
    /// / `visible_image_placements` describe `term_height + window_extra_rows`
    /// rows. The first `window_extra_rows` rows sit above the normal visible
    /// window (i.e. the buffer-absolute window start is
    /// `total_rows - term_height - scroll_offset - window_extra_rows`).
    ///
    /// `0` in the common case (no folds in view). The GUI uses these extra
    /// rows to fill the screen after collapsing folded blocks, keeping the
    /// live bottom pinned. It never affects `scroll_offset`, `show_cursor`, or
    /// `scroll_changed` — only the top of the flatten window moves.
    pub window_extra_rows: usize,

    /// Height of the visible window in rows.
    pub height: usize,

    /// Cursor position in screen coordinates (0-indexed, relative to the top
    /// of the visible window).
    pub cursor_pos: CursorPos,

    /// Whether the cursor should be painted.
    pub show_cursor: bool,

    /// Current cursor shape / blink style.
    pub cursor_visual_style: CursorVisualStyle,

    /// `true` when the alternate screen buffer is currently active.
    pub is_alternate_screen: bool,

    /// `true` when the display is in normal (non-inverted) mode.
    pub is_normal_display: bool,

    /// Terminal width in character columns.
    pub term_width: usize,

    /// Terminal height in character rows.
    pub term_height: usize,

    /// Total number of rows in the buffer (scrollback + visible).
    ///
    /// The GUI uses this together with `term_height` and `scroll_offset` to
    /// compute the *visible window start* index, which is needed to convert
    /// between screen-relative row indices and retained buffer indices.
    /// Anything the GUI stores across frames (selection, search matches) is
    /// numbered with logical [`RowNumber`]s instead; see [`Self::row_base`].
    pub total_rows: usize,

    /// Logical row number of the oldest retained row (retained index `0`).
    ///
    /// Row `i` of the buffer is numbered `row_base + i`. Stable row numbers
    /// (Task 125.14) are what `prompt_rows` and `command_blocks` carry: a
    /// number never changes while its row lives, and falls below `row_base`
    /// once the row has been evicted. Convert between numbers and the
    /// retained indices the GUI's row math uses with [`Self::row_number_at`]
    /// and [`Self::retained_index_of`].
    pub row_base: RowNumber,

    /// `true` when at least one visible format tag has a non-`None` blink state.
    ///
    /// The GUI uses this to drive the blink timer — when no visible text is
    /// blinking, the timer is not ticked and no blink repaints are scheduled,
    /// saving power.
    pub has_blinking_text: bool,

    /// `true` when at least one visible format tag carries a URL (`url.is_some()`).
    ///
    /// The GUI uses this to skip the entire URL hover detection code path when
    /// no URLs exist in the visible window — the common case (~99% of terminal
    /// usage).  This avoids the `O(visible_chars)` + `O(tags)` scan that would
    /// otherwise run on every mouse-move pixel.
    pub has_urls: bool,

    /// Per-row flat-index offsets into `visible_chars`.
    ///
    /// `row_offsets[r]` is the index in `visible_chars` where row `r` begins.
    /// This enables O(1) row lookup in `flat_index_for_cell` instead of the
    /// `O(visible_chars)` linear scan for `NewLine` separators.
    ///
    /// Wrapped in `Arc` so the clean-path snapshot reuse is a refcount bump.
    pub row_offsets: Arc<Vec<usize>>,

    /// Per-row content epoch for the visible window, parallel to `row_offsets`.
    ///
    /// One entry per visible window row, top to bottom — `row_epochs[r]`
    /// addresses the same row as `row_offsets[r]`. The value changes exactly
    /// when that row's rendered content changed since the previous flatten of
    /// the window (merged characters, merged format tags, and the row's
    /// [`freminal_buffer::row::LineWidth`]); a row merely *written to* with
    /// identical bytes does not bump. Stamps are globally monotonic and never
    /// reused, so the consumer detects change by comparing each entry against
    /// the epoch it last rendered for that row, not by comparing against a
    /// fixed baseline.
    ///
    /// Unlike a `bool` edge, a monotonic stamp is level-triggered rather than
    /// edge-triggered, so it survives the many snapshots the GUI never
    /// renders: a change made between two rendered frames still shows up as a
    /// differing epoch on the next one the GUI actually consumes, instead of
    /// being silently lost. This replaced a sticky `content_changed: bool`
    /// field (deleted by Task 124.12) that went stale across exactly those
    /// unrendered snapshots.
    pub row_epochs: Arc<[u64]>,

    /// Indices into `visible_tags` of tags that carry a URL (`url.is_some()`).
    ///
    /// The GUI uses this to iterate only URL-bearing tags during hover
    /// detection instead of scanning all tags — reducing the cost from
    /// `O(all_tags)` to `O(url_tags)` (typically O(0)).
    ///
    /// Wrapped in `Arc` so the clean-path snapshot reuse is a refcount bump.
    pub url_tag_indices: Arc<Vec<usize>>,

    /// Set to `true` when the scroll offset changed since the previous
    /// snapshot (the visible window moved, but the underlying text may not
    /// have changed).
    ///
    /// The GUI uses this to distinguish a pure scroll event from actual
    /// content mutation so that text selections are not spuriously cleared
    /// when the user scrolls through history.
    pub scroll_changed: bool,

    /// Current bracketed-paste mode setting.
    ///
    /// Carried in the snapshot so the GUI can wrap pasted text in the correct
    /// escape sequences without holding the emulator lock.
    pub bracketed_paste: RlBracket,

    /// Current mouse-tracking mode setting.
    ///
    /// Carried in the snapshot so the GUI can decide which mouse events to
    /// encode and send to the PTY without holding the emulator lock.
    pub mouse_tracking: MouseTrack,

    /// Current mouse-encoding format setting.
    ///
    /// Orthogonal to `mouse_tracking` — the tracking level determines *which*
    /// events are reported, while the encoding determines *how* they are
    /// formatted (X11 binary vs SGR text vs UTF-8 extended).
    ///
    /// Set by `?1005` (Utf8), `?1006` (Sgr), `?1016` (`SgrPixels`).
    /// Defaults to `X11` when no encoding mode has been explicitly set.
    pub mouse_encoding: MouseEncoding,

    /// Whether the terminal should repeat key-press events while a key is held.
    pub repeat_keys: Decarm,

    /// Cursor key mode (`DECCKM`).
    ///
    /// Needed by the GUI to encode arrow / home / end keys correctly without
    /// consulting the emulator.
    pub cursor_key_app_mode: Decckm,

    /// Keypad mode (`DECPAM` / `DECPNM`).
    ///
    /// Needed by the GUI to encode keypad key presses correctly: application
    /// mode sends escape sequences (`ESC O …`) while numeric mode sends the
    /// literal digit/operator character.
    pub keypad_app_mode: KeypadMode,

    /// Whether the terminal has requested that rendering be suppressed
    /// (Synchronized Output / `DEC 2026`).
    ///
    /// When `true` the GUI skips the render pass entirely for this frame.
    pub skip_draw: bool,

    /// Current xterm `modifyOtherKeys` level (0, 1, or 2).
    ///
    /// Carried in the snapshot so the GUI can encode modified character keys.
    /// At present, level 2 uses the xterm `CSI 27 ; MOD ; CODE ~` format for
    /// modified keys, while levels 0 and 1 both emit the usual C0 control bytes.
    pub modify_other_keys: u8,

    /// Whether Application Escape Key mode (`?7727`) is active.
    ///
    /// When set, pressing the Escape key should send `CSI 27 ; 1 ; 27 ~`
    /// (unambiguous CSI format) instead of bare `ESC` (`0x1b`), allowing
    /// tmux to instantly distinguish the Escape key from the start of an
    /// escape sequence.
    pub application_escape_key: ApplicationEscapeKey,

    /// Backarrow key mode (`DECBKM` / `?67`).
    ///
    /// Controls whether the Backspace key sends BS (0x08) or DEL (0x7F).
    /// Default is DEL (reset), matching xterm and modern terminals.
    pub backarrow_sends_bs: Decbkm,

    /// Line Feed / New Line mode (`LNM` / mode 20).
    ///
    /// When set to `Lnm::NewLine`, the Enter key sends CR+LF instead of bare
    /// CR.  Needed by the GUI to encode the Enter key correctly without
    /// consulting the emulator.
    pub line_feed_mode: Lnm,

    /// Currently active Kitty keyboard protocol flags (stack top, 0 if empty).
    ///
    /// When non-zero, the GUI must encode key events using KKP format instead
    /// of the legacy xterm encoding.  The specific flags determine which
    /// extensions are active (disambiguation, event types, etc.).
    pub kitty_keyboard_flags: u32,

    /// Alternate scroll mode (`?1007`).
    ///
    /// When enabled and the alternate screen is active, mouse scroll-wheel
    /// events are translated into arrow-key sequences sent to the PTY.
    /// When disabled, scroll events on the alternate screen are ignored
    /// (unless mouse tracking is active).
    pub alternate_scroll: AlternateScroll,

    /// Current working directory reported by the shell via OSC 7, if any.
    ///
    /// The GUI can use this for tab titles, file-open dialogs, or spawning
    /// new terminals in the same directory.
    pub cwd: Option<String>,

    /// Shell history file path reported by the shell-integration scripts via
    /// `OSC 1338 ; HISTFILE=<path> ST`, if any.
    ///
    /// The GUI uses this to seed the command-history palette with the
    /// shell-evaluated `$HISTFILE` (which may differ from the parent-environment
    /// value when shell configs override it).
    pub shell_histfile: Option<PathBuf>,

    /// Current FTCS (OSC 133) shell integration state.
    ///
    /// Indicates whether the terminal is currently inside a prompt, command
    /// input, or command output region.
    pub ftcs_state: FtcsState,

    /// Exit code from the most recent `OSC 133 ; D` marker, if any.
    ///
    /// The GUI can use this to display command success/failure indicators.
    pub last_exit_code: Option<i32>,

    /// Logical row numbers where OSC 133 prompt-start markers fired.
    ///
    /// Used by the GUI for command-boundary jumping (Ctrl+Shift+Up/Down).
    /// Ordering is not guaranteed; consumers must not assume this list is
    /// sorted. A mark whose row has been evicted can linger (eviction only
    /// trims the leading run of such marks), so consumers must filter with
    /// [`Self::retained_index_of`], which yields `None` for one.
    pub prompt_rows: Arc<[RowNumber]>,

    /// OSC 133 command blocks captured by the buffer.
    ///
    /// Each block records one shell command's full lifecycle — prompt start
    /// (`A`), command input start (`B`), output start (`C`), command finished
    /// (`D`) — along with optional exit code, captured CWD, and start/finish
    /// timestamps.  Surfaced through the snapshot so the GUI can render
    /// command gutters (Task 73), copy-output actions (Task 72.11), and fold
    /// state (Task 72.10).
    ///
    /// Ordering: oldest-first.  Capped at the buffer's `scrollback_limit`.
    ///
    /// The row fields are logical [`RowNumber`]s; see [`Self::row_base`].
    pub command_blocks: Arc<[CommandBlock]>,

    /// The active color theme palette.
    ///
    /// Carried in the snapshot so the GUI can render with the user's chosen
    /// theme without holding any lock.
    pub theme: &'static ThemePalette,

    /// Dynamic cursor color override (set via OSC 12; reset via OSC 112).
    ///
    /// When `Some`, the cursor should be rendered in this color instead of
    /// the theme's `cursor` field.
    pub cursor_color_override: Option<(u8, u8, u8)>,

    /// Pointer (mouse cursor) shape requested by the application via OSC 22.
    ///
    /// The GUI maps this to `egui::CursorIcon` during the render pass.
    /// `PointerShape::Default` means no override — use the OS default arrow.
    pub pointer_shape: PointerShape,

    /// All inline images referenced by the visible window.
    ///
    /// The map contains only the images that appear in `visible_image_placements`
    /// — images that have scrolled completely out of the visible window are not
    /// included.  Wrapped in `Arc` so cloning a snapshot is a refcount bump, not
    /// a deep copy of the pixel data.
    pub images: Arc<HashMap<u64, InlineImage>>,

    /// Per-cell image placement data for the visible window.
    ///
    /// One entry per cell, in row-major order (row 0 col 0, row 0 col 1, …,
    /// row N-1 col W-1).  `None` means the cell carries no image; `Some`
    /// means the cell is part of an inline image and identifies which portion.
    ///
    /// Parallel to `visible_chars` — the same cell index addresses both vectors.
    ///
    /// Wrapped in `Arc` so the clean-path snapshot reuse is cheap.
    pub visible_image_placements: Arc<Vec<Option<ImagePlacement>>>,

    /// Per-row line-width attribute for the visible window.
    ///
    /// One entry per visible row, in top-to-bottom order.  The renderer uses
    /// this to apply 2× horizontal scaling for DECDWL rows and 2× scaling in
    /// both dimensions (with top/bottom clipping) for DECDHL rows.
    pub visible_line_widths: Arc<Vec<freminal_buffer::row::LineWidth>>,
}

impl TerminalSnapshot {
    /// The extent of the buffer this snapshot was built from.
    #[must_use]
    pub const fn extent(&self) -> BufferExtent {
        BufferExtent {
            row_base: self.row_base,
            total_rows: self.total_rows,
        }
    }

    /// The logical row number of the buffer row at retained index
    /// `retained_index` (`row_base + retained_index`).
    ///
    /// Plain arithmetic: an index at or past [`Self::total_rows`] yields the
    /// number such a row *would* have.
    #[must_use]
    pub fn row_number_at(&self, retained_index: usize) -> RowNumber {
        self.row_base.saturating_add(retained_index)
    }

    /// The retained buffer index of the row numbered `row`, or `None` if that
    /// row is not in this snapshot's buffer: evicted (below
    /// [`Self::row_base`]), not yet created (at or past `row_base +
    /// total_rows`), or numbered in the other screen's namespace.
    #[must_use]
    pub fn retained_index_of(&self, row: RowNumber) -> Option<usize> {
        row.rows_after(self.row_base)
            .filter(|&index| index < self.total_rows)
    }

    /// Construct a blank snapshot suitable as the initial value for an
    /// `ArcSwap<TerminalSnapshot>` before the PTY thread has produced any
    /// real data.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            visible_chars: Arc::new(Vec::new()),
            visible_tags: Arc::new(Vec::new()),
            scroll_offset: 0,
            max_scroll_offset: 0,
            window_extra_rows: 0,
            height: 0,
            cursor_pos: CursorPos { x: 0, y: 0 },
            show_cursor: false,
            cursor_visual_style: CursorVisualStyle::default(),
            is_alternate_screen: false,
            is_normal_display: true,
            term_width: 0,
            term_height: 0,
            total_rows: 0,
            row_base: RowNumber::ZERO,
            has_blinking_text: false,
            has_urls: false,
            row_offsets: Arc::new(Vec::new()),
            row_epochs: Arc::from([]),
            url_tag_indices: Arc::new(Vec::new()),
            scroll_changed: false,
            bracketed_paste: RlBracket::default(),
            mouse_tracking: MouseTrack::default(),
            mouse_encoding: MouseEncoding::default(),
            repeat_keys: Decarm::RepeatKey,
            cursor_key_app_mode: Decckm::Ansi,
            keypad_app_mode: KeypadMode::Numeric,
            skip_draw: false,
            modify_other_keys: 0,
            application_escape_key: ApplicationEscapeKey::Reset,
            backarrow_sends_bs: Decbkm::BackarrowSendsDel,
            line_feed_mode: Lnm::LineFeed,
            kitty_keyboard_flags: 0,
            alternate_scroll: AlternateScroll::Disabled,
            cwd: None,
            shell_histfile: None,
            ftcs_state: FtcsState::default(),
            last_exit_code: None,
            prompt_rows: Arc::from([]),
            command_blocks: Arc::from(Vec::<CommandBlock>::new()),
            theme: &freminal_common::themes::CATPPUCCIN_MOCHA,
            images: Arc::new(HashMap::new()),
            visible_image_placements: Arc::new(Vec::new()),
            visible_line_widths: Arc::new(Vec::new()),
            cursor_color_override: None,
            pointer_shape: PointerShape::Default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_modify_other_keys_is_zero() {
        assert_eq!(TerminalSnapshot::empty().modify_other_keys, 0);
    }

    #[test]
    fn empty_application_escape_key_is_reset() {
        assert_eq!(
            TerminalSnapshot::empty().application_escape_key,
            ApplicationEscapeKey::Reset
        );
    }

    #[test]
    fn empty_cursor_color_override_is_none() {
        assert!(TerminalSnapshot::empty().cursor_color_override.is_none());
    }

    #[test]
    fn empty_has_blinking_text_is_false() {
        assert!(!TerminalSnapshot::empty().has_blinking_text);
    }

    #[test]
    fn empty_has_urls_is_false() {
        assert!(!TerminalSnapshot::empty().has_urls);
    }

    #[test]
    fn empty_kitty_keyboard_flags_is_zero() {
        assert_eq!(TerminalSnapshot::empty().kitty_keyboard_flags, 0);
    }

    #[test]
    fn empty_visible_line_widths_is_empty() {
        assert!(TerminalSnapshot::empty().visible_line_widths.is_empty());
    }

    #[test]
    fn empty_pointer_shape_is_default() {
        assert_eq!(
            TerminalSnapshot::empty().pointer_shape,
            PointerShape::Default
        );
    }

    // ── logical row numbers (Task 125.14) ───────────────────────────────

    /// A snapshot of `total_rows` rows whose oldest row is numbered `base`.
    fn snap_at(base: u64, total_rows: usize, term_height: usize) -> TerminalSnapshot {
        let mut snap = TerminalSnapshot::empty();
        snap.row_base = RowNumber::new(base);
        snap.total_rows = total_rows;
        snap.term_height = term_height;
        snap
    }

    #[test]
    fn extent_pairs_row_base_with_total_rows() {
        let snap = snap_at(100, 10, 3);
        assert_eq!(
            snap.extent(),
            BufferExtent {
                row_base: RowNumber::new(100),
                total_rows: 10,
            }
        );
    }

    #[test]
    fn extent_differs_when_only_the_base_advances() {
        // At scrollback capacity `total_rows` is frozen while the base
        // advances; the two extents must still compare unequal.
        let before = snap_at(100, 10, 3);
        let after = snap_at(101, 10, 3);
        assert_ne!(before.extent(), after.extent());
    }

    #[test]
    fn empty_snapshot_has_zero_row_base() {
        assert_eq!(TerminalSnapshot::empty().row_base, RowNumber::ZERO);
        assert!(TerminalSnapshot::empty().prompt_rows.is_empty());
    }

    #[test]
    fn row_number_at_is_base_plus_index() {
        let snap = snap_at(100, 10, 4);
        assert_eq!(snap.row_number_at(0), RowNumber::new(100));
        assert_eq!(snap.row_number_at(7), RowNumber::new(107));
    }

    #[test]
    fn retained_index_of_inverts_row_number_at() {
        let snap = snap_at(100, 10, 4);
        for i in 0..10 {
            assert_eq!(snap.retained_index_of(snap.row_number_at(i)), Some(i));
        }
    }

    #[test]
    fn retained_index_of_rejects_evicted_and_not_yet_created_rows() {
        let snap = snap_at(100, 10, 4);
        assert_eq!(snap.retained_index_of(RowNumber::new(99)), None, "evicted");
        assert_eq!(
            snap.retained_index_of(RowNumber::new(110)),
            None,
            "past the last retained row"
        );
        assert_eq!(snap.retained_index_of(RowNumber::new(0)), None);
    }

    #[test]
    fn retained_index_of_rejects_the_other_namespace() {
        let snap = snap_at(100, 10, 4);
        assert_eq!(snap.retained_index_of(RowNumber::ALTERNATE_BASE), None);
        let alt = snap_at(1 << 63, 4, 4);
        assert_eq!(alt.retained_index_of(RowNumber::new(5)), None);
        assert_eq!(alt.retained_index_of(RowNumber::ALTERNATE_BASE), Some(0));
    }
}
