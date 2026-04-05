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

use std::{collections::HashMap, sync::Arc};

use freminal_buffer::image_store::{ImagePlacement, InlineImage};
use freminal_common::{
    buffer_states::{
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
        tchar::TChar,
    },
    cursor::CursorVisualStyle,
    themes::ThemePalette,
};

use crate::io::PlaybackMode;

/// Playback status information carried in each snapshot.
///
/// When the terminal is running in playback mode, the consumer thread
/// populates this struct after building the emulator snapshot so the GUI
/// can display progress and controls.
#[derive(Debug, Clone)]
pub struct PlaybackInfo {
    /// Index of the last processed frame (0-indexed).
    pub current_frame: usize,
    /// Total number of frames in the recording.
    pub total_frames: usize,
    /// Currently selected playback mode.
    pub mode: PlaybackMode,
    /// `true` when real-time playback is actively running (not paused).
    pub playing: bool,
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
#[allow(clippy::struct_excessive_bools)] // Seven independent rendering/bookkeeping bools; enums would add noise
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
    /// between screen-relative row indices and buffer-absolute row indices
    /// used by `SelectionState`.
    pub total_rows: usize,

    /// Set to `true` when the visible content changed since the previous
    /// snapshot.
    ///
    /// The GUI uses this flag to reset `ViewState::scroll_offset` to 0 when
    /// the user is scrolled back and new output arrives.
    pub content_changed: bool,

    /// `true` when at least one visible format tag has a non-`None` blink state.
    ///
    /// The GUI uses this to drive the blink timer — when no visible text is
    /// blinking, the timer is not ticked and no blink repaints are scheduled,
    /// saving power.
    pub has_blinking_text: bool,

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
    pub backarrow_sends_bs: Decbkm,

    /// Line Feed / New Line mode (`LNM` / mode 20).
    ///
    /// When set to `Lnm::NewLine`, the Enter key sends CR+LF instead of bare
    /// CR.  Needed by the GUI to encode the Enter key correctly without
    /// consulting the emulator.
    pub line_feed_mode: Lnm,

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

    /// Current FTCS (OSC 133) shell integration state.
    ///
    /// Indicates whether the terminal is currently inside a prompt, command
    /// input, or command output region.
    pub ftcs_state: FtcsState,

    /// Exit code from the most recent `OSC 133 ; D` marker, if any.
    ///
    /// The GUI can use this to display command success/failure indicators.
    pub last_exit_code: Option<i32>,

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

    /// Playback status, present only when the application is in playback mode.
    ///
    /// The GUI uses this to render playback controls and frame progress.
    /// `None` in normal (live PTY) mode.
    pub playback_info: Option<PlaybackInfo>,
}

impl TerminalSnapshot {
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
            height: 0,
            cursor_pos: CursorPos { x: 0, y: 0 },
            show_cursor: false,
            cursor_visual_style: CursorVisualStyle::default(),
            is_alternate_screen: false,
            is_normal_display: true,
            term_width: 0,
            term_height: 0,
            total_rows: 0,
            content_changed: false,
            has_blinking_text: false,
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
            backarrow_sends_bs: Decbkm::BackarrowSendsBs,
            line_feed_mode: Lnm::LineFeed,
            alternate_scroll: AlternateScroll::Disabled,
            cwd: None,
            ftcs_state: FtcsState::default(),
            last_exit_code: None,
            theme: &freminal_common::themes::CATPPUCCIN_MOCHA,
            images: Arc::new(HashMap::new()),
            visible_image_placements: Arc::new(Vec::new()),
            playback_info: None,
            cursor_color_override: None,
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
        assert!(TerminalSnapshot::empty().application_escape_key == ApplicationEscapeKey::Reset);
    }

    #[test]
    fn empty_cursor_color_override_is_none() {
        assert!(TerminalSnapshot::empty().cursor_color_override.is_none());
    }

    #[test]
    fn empty_has_blinking_text_is_false() {
        assert!(!TerminalSnapshot::empty().has_blinking_text);
    }
}
