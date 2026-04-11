// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use conv2::ValueFrom;
use crossbeam_channel::Sender;
use freminal_common::{
    buffer_states::{
        cursor::CursorPos,
        format_tag::FormatTag,
        ftcs::{FtcsMarker, FtcsState},
        kitty_graphics::{KittyControlData, KittyParseError, parse_kitty_graphics},
        line_draw::DecSpecialGraphics,
        mode::{Mode, SetMode},
        modes::ReportMode,
        modes::allow_alt_screen::AllowAltScreen,
        modes::allow_column_mode_switch::AllowColumnModeSwitch,
        modes::application_escape_key::ApplicationEscapeKey,
        modes::decanm::Decanm,
        modes::decawm::Decawm,
        modes::deccolm::Deccolm,
        modes::declrmm::Declrmm,
        modes::decnrcm::Decnrcm,
        modes::decom::Decom,
        modes::decsdm::Decsdm,
        modes::dectcem::Dectcem,
        modes::grapheme::GraphemeClustering,
        modes::in_band_resize_mode::InBandResizeMode,
        modes::irm::Irm,
        modes::kitty_keyboard::KittyKeyboardFlags,
        modes::lnm::Lnm,
        modes::private_color_registers::PrivateColorRegisters,
        modes::reverse_wrap_around::ReverseWrapAround,
        modes::s8c1t::S8c1t,
        modes::xt_rev_wrap2::XtRevWrap2,
        modes::xtcblink::XtCBlink,
        modes::xtextscrn::{AltScreen47, SaveCursor1048, XtExtscrn},
        osc::{AnsiOscType, ITerm2InlineImageData, UrlResponse},
        tchar::TChar,
        terminal_output::TerminalOutput,
        terminal_sections::TerminalSections,
        unicode_placeholder::{
            VirtualPlacement, color_to_image_id, color_to_placement_id, is_placeholder,
            parse_placeholder_diacritics,
        },
        url::Url,
        window_manipulation::WindowManipulation,
    },
    colors::{ColorPalette, TerminalColor},
    cursor::CursorVisualStyle,
    pty_write::{FreminalTerminalSize, PtyWrite},
    themes::ThemePalette,
};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use crate::buffer::Buffer;
use crate::image_store::{ImagePlacement, ImageProtocol};

mod dcs;
mod graphics_iterm2;
mod graphics_kitty;
mod graphics_sixel;
mod osc_colors;
mod pty_writer;
mod sgr;
mod shell_integration;

/// In-progress state for an iTerm2 multipart file transfer.
///
/// Accumulates metadata from `MultipartFile=` and decoded byte chunks from `FilePart=`
/// until `FileEnd` signals completion.
#[derive(Debug)]
struct MultipartImageState {
    /// Metadata parsed from the `MultipartFile=` begin sequence.
    metadata: ITerm2InlineImageData,
    /// Accumulated decoded bytes from all `FilePart=` chunks so far.
    accumulated_data: Vec<u8>,
}

/// In-progress state for a Kitty graphics chunked transfer.
///
/// Accumulates control data from the first chunk and decoded payload bytes from
/// subsequent `m=1` chunks until a `m=0` final chunk arrives.
#[derive(Debug)]
struct KittyImageState {
    /// Control data from the first chunk of the transfer.
    control: KittyControlData,
    /// Accumulated decoded payload bytes from all chunks so far.
    accumulated_data: Vec<u8>,
}

/// Tracked state for diacritic inheritance between consecutive placeholder cells.
///
/// When a U+10EEEE placeholder character omits some or all diacritics, the
/// missing values are inherited from the previous placeholder cell if the
/// foreground and underline colors match.
#[derive(Debug, Clone)]
struct PrevPlaceholder {
    /// Image ID extracted from the foreground color.
    image_id: u32,
    /// Placement ID extracted from the underline color.
    placement_id: u32,
    /// Row index within the image.
    row: u16,
    /// Column index within the image.
    col: u16,
    /// MSB of the image ID (from 3rd diacritic).
    id_msb: u16,
    /// The foreground color used for comparison.
    fg_color: TerminalColor,
    /// The underline color used for comparison.
    underline_color: TerminalColor,
}

/// Processes parsed terminal output sequences and drives mutations on the underlying [`Buffer`].
///
/// `TerminalHandler` owns the buffer plus all terminal mode state (cursor style, color palette,
/// SGR attributes, image transfer state, etc.).  It is driven by
/// [`TerminalHandler::process_outputs`], which dispatches each [`TerminalOutput`] variant to the
/// appropriate handler method.  Write-back responses (CPR, DA1, etc.) are sent through the
/// optional `write_tx` PTY channel.
#[derive(Debug)]
pub struct TerminalHandler {
    buffer: Buffer,
    current_format: FormatTag,
    /// Whether the cursor should be rendered (`Dectcem::Show`) or hidden.
    show_cursor: Dectcem,
    /// The current cursor shape and blink state.
    cursor_visual_style: CursorVisualStyle,
    /// Whether DEC Special Graphics character remapping is active.
    character_replace: DecSpecialGraphics,
    /// Saved `character_replace` state from the most recent DECSC.
    ///
    /// The VT100 spec requires DECSC to save the character set designators
    /// (G0/G1) and GL invocation.  Freminal uses a simplified single-flag
    /// model, so we save just `character_replace` here.
    saved_character_replace: Option<DecSpecialGraphics>,
    /// Optional channel for writing responses back to the PTY.
    write_tx: Option<Sender<PtyWrite>>,
    /// Queued window-manipulation commands waiting to be consumed by the GUI.
    window_commands: Vec<WindowManipulation>,
    /// Last graphic character written (for REP — CSI b).
    last_graphic_char: Option<TChar>,
    /// Current working directory reported by the shell via OSC 7.
    ///
    /// Stores the decoded path component from `file://hostname/path`.
    current_working_directory: Option<String>,
    /// Current FTCS (OSC 133) shell integration state.
    ftcs_state: FtcsState,
    /// Exit code from the most recent `OSC 133 ; D [; exitcode]` marker.
    last_exit_code: Option<i32>,
    /// Mutable 256-color palette with optional per-index overrides.
    palette: ColorPalette,
    /// Whether DECCOLM (132-column mode switching) is allowed.
    /// Controlled by `CSI?40h` / `CSI?40l` (`AllowColumnModeSwitch`).
    allow_column_mode_switch: AllowColumnModeSwitch,
    /// Whether alternate screen switching is allowed (`?1046`).
    ///
    /// When `Disallow`, `?47`/`?1047`/`?1049` Set/Reset are silently ignored.
    /// Default is `Allow` (allowed).
    allow_alt_screen: AllowAltScreen,
    /// The terminal width before DECCOLM was activated.
    ///
    /// Saved when `CSI?3h` (132-column mode) is received so that `CSI?3l`
    /// (80-column reset) restores the actual GUI window width rather than
    /// hardcoding 80.  `None` means DECCOLM has not changed the width.
    pre_deccolm_width: Option<usize>,
    /// Active color theme for default palette lookups.
    theme: &'static ThemePalette,
    /// Dynamic foreground color override (set via OSC 10; reset via OSC 110).
    ///
    /// When `Some`, responses to OSC 10 queries use this value instead of the
    /// theme's `foreground` field.
    fg_color_override: Option<(u8, u8, u8)>,
    /// Dynamic background color override (set via OSC 11; reset via OSC 111).
    ///
    /// When `Some`, responses to OSC 11 queries use this value instead of the
    /// theme's `background` field.
    bg_color_override: Option<(u8, u8, u8)>,
    /// Dynamic cursor color override (set via OSC 12; reset via OSC 112).
    ///
    /// When `Some`, the cursor is rendered in this color instead of the
    /// theme's `cursor` field.
    cursor_color_override: Option<(u8, u8, u8)>,
    /// In-progress iTerm2 multipart file transfer, if any.
    ///
    /// Set by `ITerm2MultipartBegin`, appended by `ITerm2FilePart`, consumed
    /// and cleared by `ITerm2FileEnd`.
    multipart_state: Option<MultipartImageState>,
    /// In-progress Kitty graphics chunked transfer, if any.
    ///
    /// Set by a Kitty graphics command with `m=1`, appended by subsequent
    /// `m=1` chunks, consumed and cleared by a final `m=0` chunk.
    kitty_state: Option<KittyImageState>,
    /// Virtual placements created by Kitty `a=p,U=1` or `a=T,U=1` commands.
    ///
    /// Keyed by `(image_id, placement_id)`.  When U+10EEEE placeholder characters
    /// appear in the text stream, these are looked up to determine image tile
    /// dimensions.
    virtual_placements: HashMap<(u64, u32), VirtualPlacement>,
    /// State of the most recent placeholder cell, for diacritic inheritance.
    ///
    /// Reset to `None` on any non-placeholder text insertion, newline, or
    /// cursor movement.
    prev_placeholder: Option<PrevPlaceholder>,
    /// Width of a single terminal cell in pixels (updated on resize).
    ///
    /// Used by image handlers (iTerm2, Kitty, Sixel) to convert pixel
    /// dimensions to cell counts.  Defaults to 8 until the first resize
    /// event provides real font metrics.
    cell_pixel_width: u32,
    /// Height of a single terminal cell in pixels (updated on resize).
    ///
    /// Used by image handlers (iTerm2, Kitty, Sixel) to convert pixel
    /// dimensions to cell counts.  Defaults to 16 until the first resize
    /// event provides real font metrics.
    cell_pixel_height: u32,
    /// Raw bytes queued for re-parsing by the emulator layer.
    ///
    /// tmux DCS passthrough can contain inner CSI or OSC sequences that
    /// `TerminalHandler` cannot parse directly (the ANSI parser lives in
    /// the `freminal-terminal-emulator` crate).  These bytes are queued here
    /// and drained by `TerminalState::handle_incoming_data()` after
    /// `process_outputs()` returns, fed back through the parser, and
    /// processed as normal terminal output.
    tmux_reparse_queue: Vec<Vec<u8>>,
    /// True while dispatching an inner sequence from a tmux DCS passthrough.
    ///
    /// When set, [`write_to_pty`] wraps the outgoing response in a DCS tmux
    /// passthrough envelope (`ESC P tmux; <doubled-ESC payload> ESC \`) so
    /// that tmux can relay it back to the requesting client.
    in_tmux_passthrough: bool,
    /// Current xterm `modifyOtherKeys` level (0, 1, or 2).
    ///
    /// Set by `CSI > 4 ; Pv m`.  Level 0 is the default (disabled).
    /// Level 1: currently behaves like level 0 in this implementation
    /// (Ctrl-key combinations still send C0 control bytes; no extended format).
    /// Level 2: ALL modified keys use the extended-format encoding as implemented
    /// by `TerminalInput::to_payload`.
    modify_other_keys_level: u8,
    /// Whether Application Escape Key mode (`?7727`) is active.
    ///
    /// When set, pressing Escape sends `CSI 27 ; 1 ; 27 ~` instead of bare
    /// `ESC` (`0x1b`), allowing tmux to instantly distinguish the Escape key
    /// from the start of an escape sequence.
    application_escape_key: ApplicationEscapeKey,
    /// Whether In-Band Resize Notifications are enabled (`?2048 h`).
    ///
    /// When set, the terminal sends `CSI 48 ; height ; width t` upon window
    /// resize, allowing the application to receive resize events in the input
    /// stream instead of relying on `SIGWINCH`.
    in_band_resize_enabled: bool,
    /// Whether Sixel Display Mode (DECSDM `?80`) is active.
    ///
    /// When set (`CSI ? 80 h`), Sixel images are placed at the cursor position
    /// but the cursor does NOT advance past the image.
    /// When reset (`CSI ? 80 l`, the default), the cursor advances below the
    /// image after placement (scrolling mode).
    sixel_display_mode: Decsdm,
    /// Whether each Sixel image uses a private (independent) color register
    /// set (`?1070 h`, default) or all images share a single persistent
    /// palette (`?1070 l`).
    private_color_registers: PrivateColorRegisters,
    /// Whether DECNRCM (National Replacement Character Set Mode, `?42`) is
    /// active. When `NrcEnabled`, character set designations map specific ASCII
    /// positions to national characters. Default is `NrcDisabled`.
    nrc_mode: Decnrcm,
    /// Whether reverse-wraparound (`?45`) is active.
    ///
    /// When `WrapAround`, the cursor can wrap backwards from column 0 to the end
    /// of the previous line within the visible screen.
    /// Default is `WrapAround` (enabled) — matches xterm's default.
    reverse_wrap: ReverseWrapAround,
    /// Whether extended reverse-wraparound (`?1045`) is active.
    ///
    /// When `Enabled` (and `?45` is also set), the cursor can wrap backwards
    /// past row 0 of the visible screen into the scrollback buffer.
    /// Default is `Disabled`.
    xt_rev_wrap2: XtRevWrap2,
    /// Whether the terminal is in VT52 compatibility mode (`?2 reset`).
    ///
    /// When `Vt52`, the parser uses the reduced VT52 escape set.
    /// Default is `Ansi` mode.
    vt52_mode: Decanm,
    /// Whether Insert Mode (IRM, ANSI mode 4) is active.
    ///
    /// When `Insert`, writing a character first shifts existing content one
    /// cell to the right.  When `Replace` (the default), the character
    /// overwrites the cell at the cursor.
    insert_mode: Irm,
    /// The persistent Sixel palette used when `private_color_registers` is
    /// `false` (shared mode, `?1070 l`).  Palette changes in one image carry
    /// over to the next.  `None` when private mode is active; populated the
    /// first time shared mode is used.
    sixel_shared_palette:
        Option<Box<[(u8, u8, u8); freminal_common::buffer_states::sixel::MAX_PALETTE]>>,
    /// Whether 8-bit C1 controls should be used in PTY responses.
    ///
    /// When `EightBit`, CSI responses use `0x9B` instead of `ESC [`, DCS uses
    /// `0x90` instead of `ESC P`, OSC uses `0x9D` instead of `ESC ]`, and ST
    /// uses `0x9C` instead of `ESC \`.  Default is `SevenBit`.
    s8c1t_mode: S8c1t,
    /// Kitty keyboard protocol mode stack.
    ///
    /// Each entry is a `u32` bitmask.  Programs push on entry via `CSI > flags u`
    /// and pop on exit via `CSI < number u`.  The active flags are
    /// `kitty_keyboard_stack.last().copied().unwrap_or(0)`.
    /// Bounded to [`KittyKeyboardFlags::MAX_STACK_DEPTH`] (256) entries.
    kitty_keyboard_stack: Vec<u32>,
    /// Saved main-screen KKP stack when alternate screen is active.
    ///
    /// The spec requires main and alternate screens to maintain independent
    /// keyboard mode stacks.
    saved_kitty_keyboard_stack: Option<Vec<u32>>,
}

impl TerminalHandler {
    /// Create a new terminal handler with the specified dimensions
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            buffer: Buffer::new(width, height),
            current_format: FormatTag::default(),
            show_cursor: Dectcem::default(),
            cursor_visual_style: CursorVisualStyle::default(),
            character_replace: DecSpecialGraphics::default(),
            saved_character_replace: None,
            write_tx: None,
            window_commands: Vec::new(),
            last_graphic_char: None,
            current_working_directory: None,
            ftcs_state: FtcsState::default(),
            last_exit_code: None,
            palette: ColorPalette::default(),
            allow_column_mode_switch: AllowColumnModeSwitch::AllowColumnModeSwitch,
            allow_alt_screen: AllowAltScreen::Allow,
            pre_deccolm_width: None,
            theme: &freminal_common::themes::CATPPUCCIN_MOCHA,
            fg_color_override: None,
            bg_color_override: None,
            cursor_color_override: None,
            multipart_state: None,
            kitty_state: None,
            virtual_placements: HashMap::new(),
            prev_placeholder: None,
            cell_pixel_width: 8,
            cell_pixel_height: 16,
            tmux_reparse_queue: Vec::new(),
            in_tmux_passthrough: false,
            modify_other_keys_level: 0,
            application_escape_key: ApplicationEscapeKey::Reset,
            in_band_resize_enabled: false,
            sixel_display_mode: Decsdm::ScrollingMode,
            private_color_registers: PrivateColorRegisters::Private,
            nrc_mode: Decnrcm::NrcDisabled,
            reverse_wrap: ReverseWrapAround::WrapAround,
            xt_rev_wrap2: XtRevWrap2::Disabled,
            vt52_mode: Decanm::Ansi,
            insert_mode: Irm::Replace,
            sixel_shared_palette: None,
            s8c1t_mode: S8c1t::SevenBit,
            kitty_keyboard_stack: Vec::new(),
            saved_kitty_keyboard_stack: None,
        }
    }

    /// Return a new handler with the given scrollback limit instead of the
    /// default (4000).  Builder-style chaining method.
    #[must_use]
    pub fn with_scrollback_limit(mut self, limit: usize) -> Self {
        self.buffer = self.buffer.with_scrollback_limit(limit);
        self
    }

    /// Get the active theme palette.
    #[must_use]
    pub const fn theme(&self) -> &'static ThemePalette {
        self.theme
    }

    /// Set the active theme palette.
    pub const fn set_theme(&mut self, theme: &'static ThemePalette) {
        self.theme = theme;
    }

    /// Get the current S8C1T mode.
    #[must_use]
    pub const fn s8c1t_mode(&self) -> &S8c1t {
        &self.s8c1t_mode
    }

    /// Set the S8C1T mode (8-bit vs 7-bit C1 controls in responses).
    pub const fn set_s8c1t_mode(&mut self, mode: S8c1t) {
        self.s8c1t_mode = mode;
    }

    /// Get the dynamic cursor color override (set via OSC 12).
    ///
    /// Returns `None` when the theme default should be used.
    #[must_use]
    pub const fn cursor_color_override(&self) -> Option<(u8, u8, u8)> {
        self.cursor_color_override
    }

    /// Full terminal reset (RIS — Reset to Initial State).
    ///
    /// Restores the handler and buffer to initial startup state.
    /// Preserves the PTY write channel and terminal geometry/scrollback config.
    ///
    /// If DECCOLM had changed the column width to 132, this resets it back
    /// to 80 columns and sends a PTY resize notification.
    pub fn full_reset(&mut self) {
        // If DECCOLM switched us to a different width, restore the pre-DECCOLM
        // width.  Fall back to 80 if no prior width was saved.
        let prev_width = self.buffer.terminal_width();
        let restore_width = self.pre_deccolm_width.take().unwrap_or(80);
        self.buffer.full_reset();
        if prev_width != restore_width {
            self.buffer.set_column_mode(restore_width);
            self.send_pty_resize(restore_width);
        }
        self.current_format = FormatTag::default();
        self.show_cursor = Dectcem::default();
        self.cursor_visual_style = CursorVisualStyle::default();
        self.character_replace = DecSpecialGraphics::default();
        self.saved_character_replace = None;
        self.window_commands.clear();
        self.last_graphic_char = None;
        self.current_working_directory = None;
        self.ftcs_state = FtcsState::default();
        self.last_exit_code = None;
        self.palette.reset_all();
        self.fg_color_override = None;
        self.bg_color_override = None;
        self.cursor_color_override = None;
        self.allow_column_mode_switch = AllowColumnModeSwitch::AllowColumnModeSwitch;
        self.virtual_placements.clear();
        self.prev_placeholder = None;
        self.modify_other_keys_level = 0;
        self.application_escape_key = ApplicationEscapeKey::Reset;
        self.kitty_keyboard_stack.clear();
        self.saved_kitty_keyboard_stack = None;
    }

    /// Get a reference to the underlying buffer
    #[must_use]
    pub const fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Get a mutable reference to the underlying buffer
    pub const fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }

    /// Get a reference to the 256-color palette.
    #[must_use]
    pub const fn palette(&self) -> &ColorPalette {
        &self.palette
    }

    /// Get a reference to the current character format (SGR state).
    #[must_use]
    pub const fn current_format(&self) -> &FormatTag {
        &self.current_format
    }

    /// Handle raw data bytes - convert to `TChar` and insert.
    /// When DEC Special Graphics mode is active, bytes 0x5F–0x7E are remapped
    /// to their Unicode box-drawing equivalents before conversion.
    ///
    /// If any grapheme cluster begins with U+10EEEE (the Kitty Unicode
    /// placeholder character), it is intercepted and converted into an image
    /// cell referencing the appropriate virtual placement.
    pub fn handle_data(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        let remapped: Cow<[u8]> = apply_dec_special(data, &self.character_replace);
        let Ok(text) = TChar::from_vec(&remapped) else {
            return;
        };

        // Fast path: if no virtual placements exist, no placeholder can resolve.
        if self.virtual_placements.is_empty() {
            if let Some(last) = text.last() {
                self.last_graphic_char = Some(*last);
            }
            self.prev_placeholder = None;
            self.insert_text_irm_aware(&text);
            return;
        }

        self.handle_data_with_placeholders(&text);
    }

    /// Slow path for `handle_data` when virtual placements exist.
    ///
    /// Scans the parsed `TChar` sequence for U+10EEEE placeholder graphemes,
    /// batching normal text for bulk insertion and converting placeholders
    /// into image cells individually.
    fn handle_data_with_placeholders(&mut self, text: &[TChar]) {
        let mut batch_start: usize = 0;

        for (i, tch) in text.iter().enumerate() {
            let is_ph =
                matches!(tch, TChar::Utf8(buf, len) if is_placeholder(&buf[..*len as usize]));

            if is_ph {
                // Flush any pending normal text batch.
                if batch_start < i {
                    let batch = &text[batch_start..i];
                    if let Some(last) = batch.last() {
                        self.last_graphic_char = Some(*last);
                    }
                    self.prev_placeholder = None;
                    self.insert_text_irm_aware(batch);
                }
                batch_start = i + 1;

                // Process the placeholder character.
                if let TChar::Utf8(buf, len) = tch {
                    self.handle_placeholder_char(&buf[..*len as usize]);
                }
            }
        }

        // Flush remaining normal text.
        if batch_start < text.len() {
            let batch = &text[batch_start..];
            if let Some(last) = batch.last() {
                self.last_graphic_char = Some(*last);
            }
            self.prev_placeholder = None;
            self.insert_text_irm_aware(batch);
        }
    }

    /// Insert text into the buffer, honouring the current IRM state.
    ///
    /// In replace mode (the default) the characters are written in bulk via
    /// `self.buffer.insert_text(text)`.  In insert mode each character is
    /// preceded by a `self.buffer.insert_spaces(1)` call that shifts existing
    /// content right.
    fn insert_text_irm_aware(&mut self, text: &[TChar]) {
        if self.insert_mode.is_insert() {
            for ch in text {
                self.buffer.insert_spaces(1);
                self.buffer.insert_text(std::slice::from_ref(ch));
            }
        } else {
            self.buffer.insert_text(text);
        }
    }

    /// Process a single U+10EEEE placeholder character and insert an image cell.
    ///
    /// Extracts the image ID from the current foreground color, the placement
    /// ID from the underline color, and row/col indices from combining
    /// diacritics.  Applies the Kitty diacritic inheritance rules when
    /// diacritics are omitted.
    fn handle_placeholder_char(&mut self, bytes: &[u8]) {
        let Some(diacritics) = parse_placeholder_diacritics(bytes) else {
            return;
        };

        let fg = self.current_format.colors.color;
        let ul = self.current_format.colors.underline_color;
        let image_id_24 = color_to_image_id(&fg);
        let placement_id = color_to_placement_id(&ul);

        // Apply diacritic inheritance rules from the Kitty spec.
        let (row, col, id_msb) =
            self.resolve_placeholder_diacritics(diacritics, image_id_24, placement_id, fg, ul);

        // Combine image ID with MSB from the 3rd diacritic.
        let full_image_id = u64::from(image_id_24) | (u64::from(id_msb) << 24);

        // Look up a matching virtual placement.
        let vp = self
            .virtual_placements
            .get(&(full_image_id, placement_id))
            .or_else(|| {
                // Fall back to placement_id=0 (any virtual placement for this image).
                if placement_id != 0 {
                    self.virtual_placements.get(&(full_image_id, 0))
                } else {
                    None
                }
            });

        let Some(_vp) = vp else {
            tracing::warn!(
                "Kitty placeholder: no virtual placement for image_id={full_image_id}, \
                 placement_id={placement_id}; inserting space"
            );
            // No matching virtual placement — insert a space and move on.
            self.buffer.insert_text(&[TChar::Space]);
            self.prev_placeholder = None;
            return;
        };

        // Insert an image cell at the current cursor position.
        let placement = ImagePlacement {
            image_id: full_image_id,
            col_in_image: usize::from(col),
            row_in_image: usize::from(row),
            protocol: ImageProtocol::Kitty,
            image_number: None,
            placement_id: Some(placement_id),
            z_index: 0,
        };

        let cursor_pos = self.buffer.get_cursor().pos;
        let row_idx = cursor_pos.y;
        let col_idx = cursor_pos.x;

        // Ensure the row exists.
        while row_idx >= self.buffer.get_rows().len() {
            self.buffer.handle_lf();
        }

        self.buffer
            .set_image_cell_at(row_idx, col_idx, placement, self.current_format.clone());

        // Advance cursor by one column (placeholder occupies one cell).
        self.buffer.advance_cursor_one();

        // Update the inheritance tracker.
        self.prev_placeholder = Some(PrevPlaceholder {
            image_id: image_id_24,
            placement_id,
            row,
            col,
            id_msb,
            fg_color: fg,
            underline_color: ul,
        });
    }

    /// Resolve diacritics for a placeholder, applying inheritance from the
    /// previous placeholder cell when diacritics are omitted.
    ///
    /// Returns `(row, col, id_msb)` after applying the Kitty spec rules:
    ///
    /// 1. No diacritics + same fg/underline as previous → inherit row, col+1, msb
    /// 2. Only row diacritic + same row/fg/underline as previous → inherit col+1, msb
    /// 3. Row+col diacritics + previous has same row, fg, underline and col=current-1 → inherit msb
    fn resolve_placeholder_diacritics(
        &self,
        diacritics: freminal_common::buffer_states::unicode_placeholder::PlaceholderDiacritics,
        image_id: u32,
        placement_id: u32,
        fg: TerminalColor,
        ul: TerminalColor,
    ) -> (u16, u16, u16) {
        let prev = match &self.prev_placeholder {
            Some(p)
                if p.image_id == image_id
                    && p.placement_id == placement_id
                    && p.fg_color == fg
                    && p.underline_color == ul =>
            {
                Some(p)
            }
            _ => None,
        };

        match diacritics.diacritic_count {
            0 => {
                // Rule 1: inherit everything, col increments by 1.
                prev.map_or((0, 0, 0), |p| (p.row, p.col.saturating_add(1), p.id_msb))
            }
            1 => {
                // Rule 2: row is explicit; if same row as prev, col = prev.col+1.
                prev.map_or((diacritics.row, 0, 0), |p| {
                    if p.row == diacritics.row {
                        (diacritics.row, p.col.saturating_add(1), p.id_msb)
                    } else {
                        (diacritics.row, 0, p.id_msb)
                    }
                })
            }
            2 => {
                // Rule 3: row+col explicit; inherit msb if prev matches.
                prev.map_or((diacritics.row, diacritics.col, 0), |p| {
                    if p.row == diacritics.row && p.col.saturating_add(1) == diacritics.col {
                        (diacritics.row, diacritics.col, p.id_msb)
                    } else {
                        (diacritics.row, diacritics.col, 0)
                    }
                })
            }
            _ => {
                // All three diacritics present — no inheritance needed.
                (diacritics.row, diacritics.col, diacritics.id_msb)
            }
        }
    }

    /// Handle REP (CSI Ps b) — repeat the last graphic character Ps times.
    fn handle_repeat_character(&mut self, count: usize) {
        if let Some(ref ch) = self.last_graphic_char {
            let repeated = vec![*ch; count];
            self.buffer.insert_text(&repeated);
        }
    }

    /// Handle LF (Line Feed) — advance cursor to the next line, scrolling if needed.
    pub fn handle_newline(&mut self) {
        self.buffer.handle_lf();
    }

    /// Handle CR (Carriage Return) — move cursor to column 0 of the current row.
    pub const fn handle_carriage_return(&mut self) {
        self.buffer.handle_cr();
    }

    /// Handle BS (Backspace) — move cursor one column to the left, respecting reverse-wrap modes.
    pub fn handle_backspace(&mut self) {
        self.buffer
            .handle_backspace(self.reverse_wrap, self.xt_rev_wrap2);
    }

    /// Handle HT (Horizontal Tab) — advance cursor to the next tab stop.
    pub fn handle_tab(&mut self) {
        self.buffer.advance_to_next_tab_stop();
    }

    /// Handle cursor position (CUP, HVP).
    ///
    /// `x` and `y` are 1-indexed (from the parser).  `None` means "leave this
    /// axis unchanged" (e.g. CHA supplies only `x`).
    ///
    /// **VT52 out-of-bounds row rule** — When the terminal is in VT52
    /// compatibility mode (`Decanm::Vt52`) and the supplied row index exceeds
    /// the screen height, the row coordinate is silently ignored and only the
    /// column is updated.  This matches the behaviour documented in the vttest
    /// source (`vt52.c`, lines 94-107): `vt52cup(max_lines+3, i-1)` is used
    /// deliberately to update only the column.
    pub fn handle_cursor_pos(&mut self, x: Option<usize>, y: Option<usize>) {
        // In VT52 mode, out-of-bounds coordinates are ignored (the axis is
        // left unchanged) rather than clamped.  This matches VT100-emulating-
        // VT52 behaviour and is relied upon by vttest's box-drawing test.
        let (x_zero, y_zero) = if self.vt52_mode == Decanm::Vt52 {
            let x_z = x.and_then(|col_1indexed| {
                if col_1indexed > self.buffer.terminal_width() {
                    None // out-of-bounds — ignore column, keep current position
                } else {
                    Some(col_1indexed.saturating_sub(1))
                }
            });
            let y_z = y.and_then(|row_1indexed| {
                if row_1indexed > self.buffer.terminal_height() {
                    None // out-of-bounds — ignore row, keep current position
                } else {
                    Some(row_1indexed.saturating_sub(1))
                }
            });
            (x_z, y_z)
        } else {
            (
                x.map(|v| v.saturating_sub(1)),
                y.map(|v| v.saturating_sub(1)),
            )
        };

        self.buffer.set_cursor_pos(x_zero, y_zero);
    }

    /// Move the cursor by `(dx, dy)` cells relative to its current position.
    pub fn handle_cursor_relative(&mut self, dx: i32, dy: i32) {
        self.buffer.move_cursor_relative(dx, dy);
    }

    /// Handle CUU (Cursor Up) — move cursor up `n` rows.
    pub fn handle_cursor_up(&mut self, n: usize) {
        let dy = i32::value_from(n).unwrap_or(i32::MAX);
        self.buffer.move_cursor_relative(0, -dy);
    }

    /// Handle CUD (Cursor Down) — move cursor down `n` rows.
    pub fn handle_cursor_down(&mut self, n: usize) {
        let dy = i32::value_from(n).unwrap_or(i32::MAX);
        self.buffer.move_cursor_relative(0, dy);
    }

    /// Handle CUF (Cursor Forward) — move cursor forward `n` columns.
    pub fn handle_cursor_forward(&mut self, n: usize) {
        let dx = i32::value_from(n).unwrap_or(i32::MAX);
        self.buffer.move_cursor_relative(dx, 0);
    }

    /// Handle CUB (Cursor Backward) — move cursor backward `n` columns.
    pub fn handle_cursor_backward(&mut self, n: usize) {
        let dx = i32::value_from(n).unwrap_or(i32::MAX);
        self.buffer.move_cursor_relative(-dx, 0);
    }

    /// Handle erase in display (ED)
    pub fn handle_erase_in_display(&mut self, mode: usize) {
        match mode {
            0 => self.buffer.erase_to_end_of_display(),
            1 => self.buffer.erase_to_beginning_of_display(),
            2 => self.buffer.erase_display(),
            3 => self.buffer.erase_scrollback(),
            _ => {} // Unknown mode, ignore
        }
    }

    /// Handle erase in line (EL)
    pub fn handle_erase_in_line(&mut self, mode: usize) {
        match mode {
            0 => self.buffer.erase_line_to_end(),
            1 => self.buffer.erase_line_to_beginning(),
            2 => self.buffer.erase_line(),
            _ => {} // Unknown mode, ignore
        }
    }

    /// Handle IL — insert `n` blank lines at the cursor row, pushing existing lines down (Insert Lines).
    pub fn handle_insert_lines(&mut self, n: usize) {
        self.buffer.insert_lines(n);
    }

    /// Handle DL — delete `n` lines starting at the cursor row, pulling lines below up (Delete Lines).
    pub fn handle_delete_lines(&mut self, n: usize) {
        self.buffer.delete_lines(n);
    }

    /// Handle ECH (Erase Characters) — erase `n` characters starting at the cursor column.
    pub fn handle_erase_chars(&mut self, n: usize) {
        self.buffer.erase_chars(n);
    }

    /// Handle DCH (Delete Characters) — delete `n` characters at the cursor column, shifting remaining characters left.
    pub fn handle_delete_chars(&mut self, n: usize) {
        self.buffer.delete_chars(n);
    }

    /// Handle DECSC — save the current cursor position, SGR state, and character set.
    pub fn handle_save_cursor(&mut self) {
        self.buffer.save_cursor();
        self.saved_character_replace = Some(self.character_replace.clone());
    }

    /// Handle DECRC — restore the cursor position, SGR state, and character set saved by the most recent DECSC.
    pub fn handle_restore_cursor(&mut self) {
        self.buffer.restore_cursor();
        if let Some(saved) = &self.saved_character_replace {
            self.character_replace = saved.clone();
        }
    }

    /// Handle ICH (Insert Characters) — insert `n` blank spaces at the cursor column, shifting existing characters right.
    pub fn handle_insert_spaces(&mut self, n: usize) {
        self.buffer.insert_spaces(n);
    }

    /// Handle set top and bottom margins (DECSTBM)
    ///
    /// `top` and `bottom` are **1-based inclusive** row numbers, exactly as the
    /// ANSI parser delivers them.  `Buffer::set_scroll_region` already converts
    /// 1-based → 0-based internally, so we must NOT subtract here.
    pub fn handle_set_scroll_region(&mut self, top: usize, bottom: usize) {
        self.buffer.set_scroll_region(top, bottom);
    }

    /// Set DECSLRM left/right margins.
    ///
    /// `left` and `right` are **1-based inclusive** column numbers as delivered
    /// by the parser.  Only effective when DECLRMM (`?69`) is active.
    pub fn handle_set_left_right_margins(&mut self, left: usize, right: usize) {
        if self.buffer.is_declrmm_enabled() == Declrmm::Enabled {
            self.buffer.set_left_right_margins(left, right);
        }
    }

    /// Handle IND — Index: move cursor down one row, scrolling the scroll region up if at the bottom margin.
    pub fn handle_index(&mut self) {
        self.buffer.handle_ind();
    }

    /// Handle RI — Reverse Index: move cursor up one row, scrolling the scroll region down if at the top margin.
    pub fn handle_reverse_index(&mut self) {
        self.buffer.handle_ri();
    }

    /// Handle NEL — Next Line: perform a carriage return followed by an index (move to start of next line).
    pub fn handle_next_line(&mut self) {
        self.buffer.handle_nel();
    }

    /// Handle SU — Scroll Up `n` lines within the scroll region.
    /// Content moves up; blank lines appear at the bottom of the region.
    pub fn handle_scroll_up(&mut self, n: usize) {
        self.buffer.scroll_region_up_n(n);
    }

    /// Handle SD — Scroll Down `n` lines within the scroll region.
    /// Content moves down; blank lines appear at the top of the region.
    pub fn handle_scroll_down(&mut self, n: usize) {
        self.buffer.scroll_region_down_n(n);
    }

    /// Update format tag directly
    pub fn set_format(&mut self, format: FormatTag) {
        self.current_format = format.clone();
        self.buffer.set_format(format);
    }

    /// Handle entering alternate screen
    pub fn handle_enter_alternate(&mut self) {
        // scroll_offset is owned by ViewState on the GUI side; the PTY thread
        // always passes 0 when entering the alternate screen.
        self.buffer.enter_alternate(0);
        // Save and reset the KKP stack — the spec requires main and alternate
        // screens to maintain independent keyboard mode stacks.
        self.saved_kitty_keyboard_stack = Some(std::mem::take(&mut self.kitty_keyboard_stack));
    }

    /// Handle leaving alternate screen
    pub fn handle_leave_alternate(&mut self) {
        // Returns the saved scroll_offset from the primary screen; discarded here
        // because scroll_offset is owned by ViewState on the GUI side.
        let _restored_offset = self.buffer.leave_alternate();
        // Restore the main-screen KKP stack.
        if let Some(saved) = self.saved_kitty_keyboard_stack.take() {
            self.kitty_keyboard_stack = saved;
        }
    }

    /// Handle DECAWM — enable or disable soft-wrapping.
    pub const fn handle_set_wrap(&mut self, mode: Decawm) {
        self.buffer.set_wrap(mode);
    }

    /// Return `true` when the cursor should be painted.
    #[must_use]
    pub const fn show_cursor(&self) -> bool {
        matches!(self.show_cursor, Dectcem::Show)
    }

    /// Return the current cursor shape / blink style.
    #[must_use]
    pub fn cursor_visual_style(&self) -> CursorVisualStyle {
        self.cursor_visual_style.clone()
    }

    /// Apply an `XtCBlink` blink-mode change to the current `cursor_visual_style`.
    ///
    /// Flips between the blinking and steady variants of whichever shape is active,
    /// matching the behaviour of the old buffer's `set_mode` handler.
    fn apply_xtcblink(&mut self, blink: &XtCBlink) {
        match blink {
            XtCBlink::Blinking => {
                self.cursor_visual_style = match self.cursor_visual_style {
                    CursorVisualStyle::BlockCursorSteady => CursorVisualStyle::BlockCursorBlink,
                    CursorVisualStyle::UnderlineCursorSteady => {
                        CursorVisualStyle::UnderlineCursorBlink
                    }
                    CursorVisualStyle::VerticalLineCursorSteady => {
                        CursorVisualStyle::VerticalLineCursorBlink
                    }
                    // Already blinking — leave unchanged.
                    ref other => other.clone(),
                };
            }
            XtCBlink::Steady => {
                self.cursor_visual_style = match self.cursor_visual_style {
                    CursorVisualStyle::BlockCursorBlink => CursorVisualStyle::BlockCursorSteady,
                    CursorVisualStyle::UnderlineCursorBlink => {
                        CursorVisualStyle::UnderlineCursorSteady
                    }
                    CursorVisualStyle::VerticalLineCursorBlink => {
                        CursorVisualStyle::VerticalLineCursorSteady
                    }
                    // Already steady — leave unchanged.
                    ref other => other.clone(),
                };
            }
            // Query is handled at the Mode dispatch level, not here.
            XtCBlink::Query => {}
        }
    }

    /// Handle LNM — enable or disable Line Feed Mode.
    pub const fn handle_set_lnm(&mut self, mode: Lnm) {
        self.buffer.set_lnm(mode);
    }

    /// Set the PTY write channel.  Once set, responses such as CPR and DA1
    /// will be sent through this channel rather than silently discarded.
    pub fn set_write_tx(&mut self, tx: Sender<PtyWrite>) {
        self.write_tx = Some(tx);
    }

    /// Drain and return all queued `WindowManipulation` commands.
    pub fn take_window_commands(&mut self) -> Vec<WindowManipulation> {
        std::mem::take(&mut self.window_commands)
    }

    /// Drain and return all queued raw-byte sequences from tmux passthrough
    /// that need to be re-parsed by the ANSI parser.
    pub fn take_tmux_reparse_queue(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.tmux_reparse_queue)
    }

    /// Return the current working directory reported by the shell via OSC 7, if any.
    #[must_use]
    pub fn current_working_directory(&self) -> Option<&str> {
        self.current_working_directory.as_deref()
    }

    /// Return the current FTCS (OSC 133) shell integration state.
    #[must_use]
    pub const fn ftcs_state(&self) -> FtcsState {
        self.ftcs_state
    }

    /// Return the exit code from the most recent `OSC 133 ; D` marker, if any.
    #[must_use]
    pub const fn last_exit_code(&self) -> Option<i32> {
        self.last_exit_code
    }

    /// Return the current xterm `modifyOtherKeys` level (0, 1, or 2).
    #[must_use]
    pub const fn modify_other_keys_level(&self) -> u8 {
        self.modify_other_keys_level
    }

    /// Return the Application Escape Key mode (`?7727`) state.
    #[must_use]
    pub const fn application_escape_key(&self) -> ApplicationEscapeKey {
        self.application_escape_key
    }

    /// Returns the currently active Kitty keyboard protocol flags.
    ///
    /// Returns `0` when the stack is empty (protocol not active).
    #[must_use]
    pub fn kitty_keyboard_flags(&self) -> u32 {
        self.kitty_keyboard_stack.last().copied().unwrap_or(0)
    }

    /// Notify the PTY of a column-mode resize (DECCOLM).
    ///
    /// Sends a `PtyWrite::Resize` with the new width and the current height.
    /// Pixel dimensions are set to 0 — the PTY thread will use the character
    /// dimensions to compute the actual pixel size.
    fn send_pty_resize(&self, new_width: usize) {
        let height = self.buffer.terminal_height();
        if let Some(tx) = &self.write_tx {
            let size = FreminalTerminalSize {
                width: new_width,
                height,
                pixel_width: 0,
                pixel_height: 0,
            };
            if let Err(e) = tx.send(PtyWrite::Resize(size)) {
                tracing::error!("Failed to send PTY resize: {e}");
            }
        }
    }

    /// Handle DA2 — Secondary Device Attributes.
    /// Responds with `ESC [ > 65 ; 0 ; 0 c` (VT525, firmware 0, ROM 0).
    pub fn handle_secondary_device_attributes(&mut self) {
        tracing::debug!("DA2 query received");
        self.write_csi_response(">65;0;0c");
    }

    /// Handle DA3 — Tertiary Device Attributes.
    /// Responds with `DCS ! | 00000000 ST`.
    /// This identifies Freminal with a fixed 8-digit hexadecimal unit ID.
    pub fn handle_tertiary_device_attributes(&mut self) {
        self.write_dcs_response("!|00000000");
    }

    /// Handle DECREQTPARM — Request Terminal Parameters.
    ///
    /// Sends `CSI <code> ; 1 ; 1 ; 120 ; 120 ; 1 ; 0 x` where `<code>` is
    /// `2` for Ps=0 and `3` for Ps=1. Values chosen to represent:
    /// - Parity: 1 (NONE)
    /// - Bits: 1 (8-bit)
    /// - Transmit speed: 120 (38400 baud)
    /// - Receive speed: 120 (38400 baud)
    /// - Clock multiplier: 1
    /// - Flags: 0
    pub fn handle_request_terminal_parameters(&mut self, ps: u8) {
        // DECREQTPARM only defines Ps=0 and Ps=1.  The parser should have
        // already validated this, but we defend against unexpected values.
        let code = match ps {
            0 => 2u8,
            1 => 3u8,
            _ => return,
        };
        self.write_csi_response(&format!("{code};1;1;120;120;1;0x"));
    }

    /// Handle `RequestDeviceNameAndVersion` — respond with Freminal's name and version.
    ///
    /// Responds with `DCS >|XTerm(Freminal <version>) ST` (7-bit) or the 8-bit
    /// equivalent when S8C1T is active.
    ///
    /// The `XTerm(` prefix is intentional: tmux's XDA handler
    /// (`tty_keys_extended_device_attributes` in `tty-keys.c`) matches the
    /// payload against a small set of known prefixes to decide which terminal
    /// feature sets to enable.  Without a recognised prefix tmux skips
    /// `extkeys`, which means `modifyOtherKeys` (`\033[>4;2m`) is never sent
    /// to Freminal and extended key sequences are not forwarded to programs
    /// running inside tmux.  Prefixing with `XTerm(` causes tmux to apply the
    /// `XTerm` feature set (which includes `extkeys`), fixing the issue.
    pub fn handle_device_name_and_version(&mut self) {
        let version = env!("CARGO_PKG_VERSION");
        self.write_dcs_response(&format!(">|XTerm(Freminal {version})"));
    }

    /// Handle an APC (Application Program Command) sequence.
    ///
    /// Attempts to parse the data as a Kitty graphics command (`_G...`).
    /// If it is not a Kitty graphics command, logs and ignores.
    pub fn handle_application_program_command(&mut self, apc: &[u8]) {
        match parse_kitty_graphics(apc) {
            Ok(cmd) => self.handle_kitty_graphics(cmd),
            Err(KittyParseError::NotKittyGraphics) => {
                tracing::warn!(
                    "APC received (not Kitty graphics, ignored): {}",
                    String::from_utf8_lossy(apc)
                );
            }
            Err(e) => {
                tracing::warn!("Kitty graphics parse error: {e}");
            }
        }
    }

    /// Handle CPR — Cursor Position Report.
    /// Responds with `CSI <row> ; <col> R` (1-indexed).
    ///
    /// Per DEC VT510: when DECOM is enabled, the reported row is relative to the
    /// scroll region top margin.  When DECOM is disabled, it is relative to the
    /// screen origin.
    pub fn handle_cursor_report(&mut self) {
        let screen_pos = self.buffer.get_cursor_screen_pos();
        let x = screen_pos.x + 1;
        let y = if self.buffer.is_decom_enabled() == Decom::OriginMode {
            let (region_top, _) = self.buffer.scroll_region();
            screen_pos.y.saturating_sub(region_top) + 1
        } else {
            screen_pos.y + 1
        };
        let body = format!("{y};{x}R");
        self.write_csi_response(&body);
    }

    /// Handle DSR — Device Status Report (Ps=5).
    /// Responds with `CSI 0 n` (device OK).
    pub fn handle_device_status_report(&mut self) {
        self.write_csi_response("0n");
    }

    /// Handle DSR ?996 — Color Theme Report.
    /// Responds with `CSI ? 997 ; Ps n` where Ps = 1 (light) or 2 (dark).
    /// Freminal's default background is dark (#45475a), so we report dark (2).
    pub fn handle_color_theme_report(&mut self) {
        // 1 = light, 2 = dark
        self.write_csi_response("?997;2n");
    }

    /// Handle DA1 — Primary Device Attributes.
    /// Responds with the capability string used by the old buffer (iTerm2 DA set).
    pub fn handle_request_device_attributes(&mut self) {
        tracing::debug!("DA1 query received");
        if self.vt52_mode == Decanm::Vt52 {
            // VT52 identify response: ESC / Z — not affected by S8C1T
            self.write_to_pty("\x1b/Z");
        } else {
            self.write_csi_response("?65;1;2;4;6;17;18;22c");
        }
    }

    /// Handle a `WindowManipulation` command.
    ///
    /// Report variants that can be answered from terminal state are handled
    /// synchronously here via `write_to_pty` so the response reaches the PTY
    /// in the same processing batch as DA1 and other inline responses.  This
    /// is critical for applications (e.g. yazi) that use DA1 as a "fence" to
    /// detect when all prior query responses have arrived.
    ///
    /// Variants that require GUI-side data (viewport position, window title,
    /// clipboard, etc.) are deferred to `self.window_commands` for the GUI
    /// thread to handle asynchronously.
    fn handle_window_manipulation(&mut self, wm: &WindowManipulation) {
        match wm {
            WindowManipulation::ReportCharacterSizeInPixels => {
                let w = self.cell_pixel_width;
                let h = self.cell_pixel_height;
                self.write_csi_response(&format!("6;{h};{w}t"));
            }
            WindowManipulation::ReportTerminalSizeInCharacters => {
                let (width, height) = self.get_win_size();
                self.write_csi_response(&format!("8;{height};{width}t"));
            }
            WindowManipulation::ReportRootWindowSizeInCharacters => {
                let (width, height) = self.get_win_size();
                self.write_csi_response(&format!("9;{height};{width}t"));
            }
            other => {
                self.window_commands.push(other.clone());
            }
        }
    }

    /// Handle an OSC (Operating System Command) sequence.
    ///
    /// Ports the logic from `TerminalState::osc_response` in the old buffer.
    pub fn handle_osc(&mut self, osc: &AnsiOscType) {
        match osc {
            // Hyperlink: OSC 8 ; params ; url ST  (start) / OSC 8 ; ; ST  (end)
            AnsiOscType::Url(UrlResponse::Url(url)) => {
                self.current_format.url = Some(Arc::new(Url {
                    id: url.id.clone(),
                    url: url.url.clone(),
                }));
                self.buffer.set_format(self.current_format.clone());
            }
            AnsiOscType::Url(UrlResponse::End) => {
                self.current_format.url = None;
                self.buffer.set_format(self.current_format.clone());
            }

            // Window title
            AnsiOscType::SetTitleBar(title) => {
                self.window_commands
                    .push(WindowManipulation::SetTitleBarText(title.clone()));
            }

            // OSC 10/11/12 foreground/background/cursor color query, set, and reset.
            AnsiOscType::RequestColorQueryBackground(_)
            | AnsiOscType::RequestColorQueryForeground(_)
            | AnsiOscType::RequestColorQueryCursor(_)
            | AnsiOscType::ResetForegroundColor
            | AnsiOscType::ResetBackgroundColor
            | AnsiOscType::ResetCursorColor => {
                self.handle_osc_fg_bg_color(osc);
            }

            // Remote host / CWD: OSC 7 ; file://hostname/path ST
            AnsiOscType::RemoteHost(value) => {
                self.current_working_directory = shell_integration::parse_osc7_uri(value);
                if self.current_working_directory.is_none() {
                    tracing::warn!("OSC 7: failed to parse URI: {value}");
                } else {
                    tracing::debug!("OSC 7: CWD set to {:?}", self.current_working_directory);
                }
            }
            AnsiOscType::Ftcs(marker) => {
                tracing::debug!("OSC 133 FTCS marker: {marker}");
                match &marker {
                    FtcsMarker::PromptStart => {
                        self.ftcs_state = FtcsState::InPrompt;
                    }
                    FtcsMarker::CommandStart => {
                        self.ftcs_state = FtcsState::InCommand;
                    }
                    FtcsMarker::OutputStart => {
                        self.ftcs_state = FtcsState::InOutput;
                    }
                    FtcsMarker::CommandFinished(exit_code) => {
                        self.last_exit_code = *exit_code;
                        self.ftcs_state = FtcsState::None;
                    }
                    FtcsMarker::PromptProperty(_kind) => {
                        // Prompt property is informational metadata — it annotates
                        // the type of the next prompt (initial, continuation, right)
                        // but does not change the FTCS state machine.
                    }
                }
            }
            AnsiOscType::ITerm2FileInline(data) => {
                self.handle_iterm2_inline_image(data);
            }
            AnsiOscType::ITerm2MultipartBegin(data) => {
                self.handle_iterm2_multipart_begin(data);
            }
            AnsiOscType::ITerm2FilePart(bytes) => {
                self.handle_iterm2_file_part(bytes);
            }
            AnsiOscType::ITerm2FileEnd => {
                self.handle_iterm2_file_end();
            }
            AnsiOscType::ITerm2Unknown => {
                tracing::warn!("OSC 1337: unrecognised sub-command (ignored)");
            }

            // Clipboard: forward to GUI via window_commands
            AnsiOscType::SetClipboard(sel, content) => {
                self.window_commands.push(WindowManipulation::SetClipboard(
                    sel.clone(),
                    content.clone(),
                ));
            }
            AnsiOscType::QueryClipboard(sel) => {
                self.window_commands
                    .push(WindowManipulation::QueryClipboard(sel.clone()));
            }

            // Palette manipulation: OSC 4 (set/query) and OSC 104 (reset)
            AnsiOscType::SetPaletteColor(idx, r, g, b) => {
                self.palette.set(*idx, *r, *g, *b);
            }
            AnsiOscType::QueryPaletteColor(idx) => {
                let (r, g, b) = self.palette.get_rgb(*idx, self.theme);
                let body = format!(
                    "4;{idx};rgb:{:04x}/{:04x}/{:04x}",
                    u16::from(r) * 257,
                    u16::from(g) * 257,
                    u16::from(b) * 257,
                );
                self.write_osc_response(&body);
            }
            AnsiOscType::ResetPaletteColor(Some(idx)) => {
                self.palette.reset(*idx);
            }
            AnsiOscType::ResetPaletteColor(None) => {
                self.palette.reset_all();
            }

            AnsiOscType::NoOp => {}
        }
    }

    /// Resize the terminal grid to `width` × `height` characters.
    ///
    /// Also updates the stored pixel-per-cell dimensions used for building
    /// `PtyWrite::Resize` payloads.  Zero values for the pixel dimensions are
    /// ignored (the stored value is not overwritten).
    ///
    /// `scroll_offset` is **always `0`** here — it is owned by `ViewState` on
    /// the GUI side.  The PTY thread never holds a scroll offset.  `set_size`
    /// returns the post-reflow offset (which may differ when scrollback rows
    /// are removed), but we discard it because the GUI's `ViewState` will
    /// clamp its own offset the next time it sends a snapshot request.
    ///
    /// The underlying `Buffer::set_size` call triggers `reflow_to_width` when
    /// the column count changes, and adjusts the row count by appending blank
    /// rows or truncating from the live bottom when the height changes.
    pub fn handle_resize(
        &mut self,
        width: usize,
        height: usize,
        cell_pixel_width: u32,
        cell_pixel_height: u32,
    ) {
        let (old_width, old_height) = self.get_win_size();

        if cell_pixel_width > 0 {
            self.cell_pixel_width = cell_pixel_width;
        }
        if cell_pixel_height > 0 {
            self.cell_pixel_height = cell_pixel_height;
        }
        // scroll_offset is owned by ViewState on the GUI side; the PTY thread
        // always passes 0 when resizing.
        let _new_offset = self.buffer.set_size(width, height, 0);

        if self.in_band_resize_enabled && (old_width != width || old_height != height) {
            self.send_in_band_resize();
        }
    }

    /// Send an in-band resize notification to the PTY.
    /// Format: `CSI 48 ; height_chars ; width_chars ; height_pixels ; width_pixels t`
    fn send_in_band_resize(&self) {
        let (width, height) = self.get_win_size();
        let Ok(width_u32) = u32::value_from(width) else {
            return;
        };
        let Ok(height_u32) = u32::value_from(height) else {
            return;
        };
        let px_w = width_u32 * self.cell_pixel_width;
        let px_h = height_u32 * self.cell_pixel_height;
        self.write_csi_response(&format!("48;{height_u32};{width_u32};{px_h};{px_w}t"));
    }

    /// Compute new `scroll_offset` after scrolling back by `lines`.
    ///
    /// The caller must pass the current offset and store the returned value
    /// into `ViewState::scroll_offset`.
    #[must_use]
    pub fn handle_scroll_back(&self, scroll_offset: usize, lines: usize) -> usize {
        self.buffer.scroll_back(scroll_offset, lines)
    }

    /// Compute new `scroll_offset` after scrolling forward by `lines`.
    ///
    /// The caller must pass the current offset and store the returned value
    /// into `ViewState::scroll_offset`.
    #[must_use]
    pub fn handle_scroll_forward(&self, scroll_offset: usize, lines: usize) -> usize {
        self.buffer.scroll_forward(scroll_offset, lines)
    }

    /// Returns 0 — the scroll offset for the live bottom view.
    ///
    /// The caller should store this into `ViewState::scroll_offset`.
    #[must_use]
    pub const fn handle_scroll_to_bottom() -> usize {
        Buffer::scroll_to_bottom()
    }

    /// Return the complete GUI data set: visible and scrollback content as
    /// `(TerminalSections<Vec<TChar>>, TerminalSections<Vec<FormatTag>>)`.
    ///
    /// `scroll_offset` is the number of rows scrolled back from the live
    /// bottom (0 = live view).  The visible window is shifted upward by this
    /// many rows so the user sees historical output.
    ///
    /// This is the primary method the GUI layer should call to obtain all data
    /// needed to render the terminal.
    #[must_use]
    pub fn data_and_format_data_for_gui(
        &mut self,
        scroll_offset: usize,
    ) -> (
        TerminalSections<Vec<TChar>>,
        TerminalSections<Vec<FormatTag>>,
    ) {
        let (visible_chars, visible_tags, _visible_row_offsets, _visible_url_indices) =
            self.buffer.visible_as_tchars_and_tags(scroll_offset);
        let (scrollback_chars, scrollback_tags, _scrollback_row_offsets, _scrollback_url_indices) =
            self.buffer.scrollback_as_tchars_and_tags(scroll_offset);
        (
            TerminalSections {
                scrollback: scrollback_chars,
                visible: visible_chars,
            },
            TerminalSections {
                scrollback: scrollback_tags,
                visible: visible_tags,
            },
        )
    }

    /// Return the current cursor position in screen coordinates (0-indexed).
    ///
    /// This returns coordinates relative to the top of the visible window, so
    /// the GUI painter can use them directly without any offset adjustment.
    #[must_use]
    pub fn cursor_pos(&self) -> CursorPos {
        self.buffer.get_cursor_screen_pos()
    }

    /// Return the current terminal dimensions as `(width, height)`.
    #[must_use]
    pub const fn get_win_size(&self) -> (usize, usize) {
        (self.buffer.terminal_width(), self.buffer.terminal_height())
    }

    /// Return `true` when the alternate screen buffer is currently active.
    #[must_use]
    pub const fn is_alternate_screen(&self) -> bool {
        self.buffer.is_alternate_screen()
    }

    /// Return `true` when a cursor has been saved via DECSC (ESC 7 / `\x1b[?1048h`).
    #[must_use]
    pub const fn has_saved_cursor(&self) -> bool {
        self.buffer.has_saved_cursor()
    }

    /// Return `true` if any row in the visible window (at the given scroll
    /// offset) has been mutated since it was last flattened into the cache.
    ///
    /// The PTY thread always passes `scroll_offset = 0`.  When this returns
    /// `false`, `build_snapshot` can skip flattening and reuse the previous
    /// `visible_chars` / `visible_tags` vectors.
    #[must_use]
    pub fn any_visible_dirty(&self, scroll_offset: usize) -> bool {
        self.buffer.any_visible_dirty(scroll_offset)
    }

    /// Extract image placements for all cells in the visible window.
    ///
    /// Returns a flat `Vec` of `Option<ImagePlacement>`, one entry per cell
    /// in row-major order, matching the layout of `visible_chars`.
    #[must_use]
    pub fn visible_image_placements(
        &self,
        scroll_offset: usize,
    ) -> Vec<Option<crate::image_store::ImagePlacement>> {
        self.buffer.visible_image_placements(scroll_offset)
    }

    /// Returns `true` if any cell in the visible window carries an image placement.
    #[must_use]
    pub fn has_visible_images(&self, scroll_offset: usize) -> bool {
        self.buffer.has_visible_images(scroll_offset)
    }

    /// Process an array of `TerminalOutput` commands
    ///
    /// This is the main entry point for integrating with the parser.
    /// It dispatches each `TerminalOutput` variant to the appropriate handler method.
    pub fn process_outputs(&mut self, outputs: &[TerminalOutput]) {
        for output in outputs {
            self.process_output(output);
        }
    }

    /// Process a single `TerminalOutput` command
    // Inherently large: exhaustive match over all `TerminalOutput` variants. Each arm is
    // tightly coupled to buffer state. Splitting would require passing the full handler context
    // to sub-functions without any reduction in complexity.
    #[allow(clippy::too_many_lines)]
    fn process_output(&mut self, output: &TerminalOutput) {
        match output {
            // === Implemented Operations ===
            TerminalOutput::Data(bytes) => {
                self.handle_data(bytes);
            }
            TerminalOutput::Newline => {
                self.handle_newline();
            }
            TerminalOutput::CarriageReturn => {
                self.handle_carriage_return();
            }
            TerminalOutput::Backspace => {
                self.handle_backspace();
            }
            TerminalOutput::SetCursorPos { x, y } => {
                self.handle_cursor_pos(*x, *y);
            }
            TerminalOutput::SetCursorPosRel { x, y } => {
                let dx = x.unwrap_or(0);
                let dy = y.unwrap_or(0);
                self.handle_cursor_relative(dx, dy);
            }
            TerminalOutput::ClearDisplayfromCursortoEndofDisplay => {
                self.handle_erase_in_display(0);
            }
            TerminalOutput::ClearDisplayfromStartofDisplaytoCursor => {
                self.handle_erase_in_display(1);
            }
            TerminalOutput::ClearDisplay => {
                self.handle_erase_in_display(2);
            }
            TerminalOutput::ClearScrollbackandDisplay => {
                self.handle_erase_in_display(3);
            }
            TerminalOutput::ClearLineForwards => {
                self.handle_erase_in_line(0);
            }
            TerminalOutput::ClearLineBackwards => {
                self.handle_erase_in_line(1);
            }
            TerminalOutput::ClearLine => {
                self.handle_erase_in_line(2);
            }
            TerminalOutput::InsertLines(n) => {
                self.handle_insert_lines(*n);
            }
            TerminalOutput::DeleteLines(n) => {
                self.handle_delete_lines(*n);
            }
            TerminalOutput::ScrollUp(n) => {
                self.handle_scroll_up(*n);
            }
            TerminalOutput::ScrollDown(n) => {
                self.handle_scroll_down(*n);
            }
            TerminalOutput::Delete(n) => {
                self.handle_delete_chars(*n);
            }
            TerminalOutput::InsertSpaces(n) => {
                self.handle_insert_spaces(*n);
            }
            TerminalOutput::Index => {
                self.handle_index();
            }
            TerminalOutput::ReverseIndex => {
                self.handle_reverse_index();
            }
            TerminalOutput::NextLine => {
                self.handle_next_line();
            }
            TerminalOutput::SetTopAndBottomMargins {
                top_margin,
                bottom_margin,
            } => {
                self.handle_set_scroll_region(*top_margin, *bottom_margin);
            }
            TerminalOutput::SetLeftAndRightMargins {
                left_margin,
                right_margin,
            } => {
                self.handle_set_left_right_margins(*left_margin, *right_margin);
            }

            // === Bell, Tab Stops, and Miscellaneous ===
            TerminalOutput::Bell => {
                self.window_commands.push(WindowManipulation::Bell);
            }
            TerminalOutput::Tab => {
                self.buffer.advance_to_next_tab_stop();
            }
            TerminalOutput::HorizontalTabSet => {
                self.buffer.set_tab_stop();
            }
            TerminalOutput::TabClear(ps) => match ps {
                0 => self.buffer.clear_tab_stop_at_cursor(),
                3 | 5 => self.buffer.clear_all_tab_stops(),
                1 | 2 | 4 => {
                    // Line tab stops (Ps=1: at cursor line, Ps=2: at cursor line,
                    // Ps=4: all). No modern terminal implements line tabulation —
                    // silently accept as no-ops.
                }
                _ => {
                    tracing::warn!("TBC with unsupported Ps={ps} (ignored)");
                }
            },
            TerminalOutput::CursorForwardTab(n) => {
                self.buffer.tab_forward(*n);
            }
            TerminalOutput::CursorBackwardTab(n) => {
                self.buffer.tab_backward(*n);
            }
            TerminalOutput::RepeatCharacter(n) => {
                self.handle_repeat_character(*n);
            }
            TerminalOutput::ApplicationKeypadMode => {
                tracing::debug!("ApplicationKeypadMode (DECPAM) — tracked in TerminalState");
            }
            TerminalOutput::NormalKeypadMode => {
                tracing::debug!("NormalKeypadMode (DECPNM) — tracked in TerminalState");
            }
            TerminalOutput::Erase(n) => {
                self.handle_erase_chars(*n);
            }
            TerminalOutput::Sgr(sgr) => {
                self.handle_sgr(sgr);
            }
            TerminalOutput::Mode(mode) => match mode {
                Mode::XtExtscrn(XtExtscrn::Alternate)
                | Mode::AltScreen47(AltScreen47::Alternate) => {
                    if self.allow_alt_screen == AllowAltScreen::Allow {
                        self.handle_enter_alternate();
                    }
                }
                Mode::XtExtscrn(XtExtscrn::Primary) | Mode::AltScreen47(AltScreen47::Primary) => {
                    if self.allow_alt_screen == AllowAltScreen::Allow {
                        self.handle_leave_alternate();
                    }
                }
                Mode::SaveCursor1048(SaveCursor1048::Save) => self.handle_save_cursor(),
                Mode::SaveCursor1048(SaveCursor1048::Restore) => self.handle_restore_cursor(),
                // Query variants: report current mode state via DECRPM response
                Mode::Dectem(Dectcem::Query) => {
                    let current = &self.show_cursor;
                    self.write_to_pty(&current.report(None));
                }
                Mode::Decawm(Decawm::Query) => {
                    let mode = if self.buffer.is_wrap_enabled() == Decawm::AutoWrap {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&Decawm::AutoWrap.report(Some(mode)));
                }
                Mode::LineFeedMode(Lnm::Query) => {
                    let mode = if self.buffer.is_lnm_enabled() == Lnm::NewLine {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&Lnm::NewLine.report(Some(mode)));
                }
                Mode::XtExtscrn(XtExtscrn::Query) => {
                    let mode = if self.is_alternate_screen() {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&XtExtscrn::Alternate.report(Some(mode)));
                }
                Mode::AltScreen47(AltScreen47::Query) => {
                    let mode = if self.is_alternate_screen() {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&AltScreen47::Alternate.report(Some(mode)));
                }
                Mode::SaveCursor1048(SaveCursor1048::Query) => {
                    // Report based on whether a cursor has actually been saved
                    // via DECSC/DECRC, not on alternate-screen state as a proxy.
                    let mode = if self.buffer.has_saved_cursor() {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&SaveCursor1048::Save.report(Some(mode)));
                }
                Mode::XtCBlink(XtCBlink::Query) => {
                    let is_blinking = matches!(
                        self.cursor_visual_style,
                        CursorVisualStyle::BlockCursorBlink
                            | CursorVisualStyle::UnderlineCursorBlink
                            | CursorVisualStyle::VerticalLineCursorBlink
                    );
                    let mode = if is_blinking {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&XtCBlink::Blinking.report(Some(mode)));
                }
                Mode::UnknownQuery(params) => {
                    // Unknown mode — respond with Ps=0 (not recognized)
                    let digits: String = params.iter().map(|&x| x as char).collect();
                    tracing::warn!("DECRQM: unknown mode ?{digits}, responding not recognized");
                    self.write_csi_response(&format!("?{digits};0$y"));
                }
                Mode::Decawm(Decawm::AutoWrap) => self.handle_set_wrap(Decawm::AutoWrap),
                Mode::Decawm(Decawm::NoAutoWrap) => self.handle_set_wrap(Decawm::NoAutoWrap),
                Mode::LineFeedMode(Lnm::NewLine) => self.handle_set_lnm(Lnm::NewLine),
                Mode::LineFeedMode(Lnm::LineFeed) => self.handle_set_lnm(Lnm::LineFeed),
                Mode::Dectem(Dectcem::Show) => self.show_cursor = Dectcem::Show,
                Mode::Dectem(Dectcem::Hide) => self.show_cursor = Dectcem::Hide,
                Mode::XtCBlink(blink) => self.apply_xtcblink(blink),
                Mode::Decom(Decom::OriginMode) => self.buffer.set_decom(Decom::OriginMode),
                Mode::Decom(Decom::NormalCursor) => self.buffer.set_decom(Decom::NormalCursor),
                Mode::Decom(Decom::Query) => {
                    let mode = if self.buffer.is_decom_enabled() == Decom::OriginMode {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&Decom::OriginMode.report(Some(mode)));
                }
                Mode::Deccolm(Deccolm::Column132) => {
                    if self.allow_column_mode_switch == AllowColumnModeSwitch::AllowColumnModeSwitch
                    {
                        // Save the current width so CSI?3l can restore it
                        // instead of hardcoding 80.
                        if self.pre_deccolm_width.is_none() {
                            self.pre_deccolm_width = Some(self.buffer.terminal_width());
                        }
                        self.buffer.set_column_mode(132);
                        self.send_pty_resize(132);
                    }
                }
                Mode::Deccolm(Deccolm::Column80) => {
                    if self.allow_column_mode_switch == AllowColumnModeSwitch::AllowColumnModeSwitch
                    {
                        // Restore the pre-DECCOLM width (falls back to 80 if
                        // no prior width was saved — e.g. CSI?3l without a
                        // preceding CSI?3h).
                        let restore_width = self.pre_deccolm_width.take().unwrap_or(80);
                        self.buffer.set_column_mode(restore_width);
                        self.send_pty_resize(restore_width);
                    }
                }
                Mode::Deccolm(Deccolm::Query) => {
                    // Report current column mode: 132 if width == 132, else 80.
                    let mode = if self.buffer.terminal_width() == 132 {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&Deccolm::Column132.report(Some(mode)));
                }
                // ── DECLRMM — Left/Right Margin Mode (?69) ───────
                Mode::Declrmm(Declrmm::Enabled) => {
                    self.buffer.set_declrmm(Declrmm::Enabled);
                }
                Mode::Declrmm(Declrmm::Disabled) => {
                    // set_declrmm(Disabled) resets margins as a side effect.
                    self.buffer.set_declrmm(Declrmm::Disabled);
                }
                Mode::Declrmm(Declrmm::Query) => {
                    let mode = if self.buffer.is_declrmm_enabled() == Declrmm::Enabled {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&Declrmm::Enabled.report(Some(mode)));
                }
                Mode::AllowColumnModeSwitch(AllowColumnModeSwitch::AllowColumnModeSwitch) => {
                    self.allow_column_mode_switch = AllowColumnModeSwitch::AllowColumnModeSwitch;
                }
                Mode::AllowColumnModeSwitch(AllowColumnModeSwitch::NoAllowColumnModeSwitch) => {
                    self.allow_column_mode_switch = AllowColumnModeSwitch::NoAllowColumnModeSwitch;
                }
                Mode::AllowColumnModeSwitch(AllowColumnModeSwitch::Query) => {
                    let mode = if self.allow_column_mode_switch
                        == AllowColumnModeSwitch::AllowColumnModeSwitch
                    {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(
                        &AllowColumnModeSwitch::AllowColumnModeSwitch.report(Some(mode)),
                    );
                }
                // ── Sixel Display Mode (?80) ──────────────────────────
                Mode::Decsdm(Decsdm::DisplayMode) => {
                    self.sixel_display_mode = Decsdm::DisplayMode;
                }
                Mode::Decsdm(Decsdm::ScrollingMode) => {
                    self.sixel_display_mode = Decsdm::ScrollingMode;
                }
                Mode::Decsdm(Decsdm::Query) => {
                    let mode = if self.sixel_display_mode == Decsdm::DisplayMode {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&Decsdm::DisplayMode.report(Some(mode)));
                }
                // ── Allow Alternate Screen Switching (?1046) ──────────
                Mode::AllowAltScreen(AllowAltScreen::Allow) => {
                    self.allow_alt_screen = AllowAltScreen::Allow;
                }
                Mode::AllowAltScreen(AllowAltScreen::Disallow) => {
                    self.allow_alt_screen = AllowAltScreen::Disallow;
                }
                Mode::AllowAltScreen(AllowAltScreen::Query) => {
                    let mode = if self.allow_alt_screen == AllowAltScreen::Allow {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&AllowAltScreen::Allow.report(Some(mode)));
                }
                // ── Private Color Registers for Sixel (?1070) ────────
                Mode::PrivateColorRegisters(PrivateColorRegisters::Private) => {
                    self.private_color_registers = PrivateColorRegisters::Private;
                    // Switching back to private mode discards the shared palette.
                    self.sixel_shared_palette = None;
                }
                Mode::PrivateColorRegisters(PrivateColorRegisters::Shared) => {
                    self.private_color_registers = PrivateColorRegisters::Shared;
                }
                Mode::PrivateColorRegisters(PrivateColorRegisters::Query) => {
                    let mode = if self.private_color_registers == PrivateColorRegisters::Private {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&PrivateColorRegisters::Private.report(Some(mode)));
                }
                // ── DECNRCM — National Replacement Character Set (?42) ─
                Mode::Decnrcm(Decnrcm::NrcEnabled) => {
                    self.nrc_mode = Decnrcm::NrcEnabled;
                }
                Mode::Decnrcm(Decnrcm::NrcDisabled) => {
                    self.nrc_mode = Decnrcm::NrcDisabled;
                }
                Mode::Decnrcm(Decnrcm::Query) => {
                    let mode = if self.nrc_mode == Decnrcm::NrcEnabled {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&Decnrcm::NrcEnabled.report(Some(mode)));
                }
                // ── Reverse Wrap Around (?45) ─────────────────────
                Mode::ReverseWrapAround(ReverseWrapAround::WrapAround) => {
                    self.reverse_wrap = ReverseWrapAround::WrapAround;
                }
                Mode::ReverseWrapAround(ReverseWrapAround::DontWrap) => {
                    self.reverse_wrap = ReverseWrapAround::DontWrap;
                }
                Mode::ReverseWrapAround(ReverseWrapAround::Query) => {
                    let mode = if self.reverse_wrap == ReverseWrapAround::WrapAround {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&ReverseWrapAround::WrapAround.report(Some(mode)));
                }
                // ── Extended Reverse Wrap (?1045) ─────────────────
                Mode::XtRevWrap2(XtRevWrap2::Enabled) => {
                    self.xt_rev_wrap2 = XtRevWrap2::Enabled;
                }
                Mode::XtRevWrap2(XtRevWrap2::Disabled) => {
                    self.xt_rev_wrap2 = XtRevWrap2::Disabled;
                }
                Mode::XtRevWrap2(XtRevWrap2::Query) => {
                    let mode = if self.xt_rev_wrap2 == XtRevWrap2::Enabled {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&XtRevWrap2::Enabled.report(Some(mode)));
                }
                // ── DECANM — ANSI/VT52 Mode (?2) ─────────────────
                Mode::Decanm(Decanm::Vt52) => {
                    self.vt52_mode = Decanm::Vt52;
                }
                Mode::Decanm(Decanm::Ansi) => {
                    self.vt52_mode = Decanm::Ansi;
                }
                Mode::Decanm(Decanm::Query) => {
                    let mode = if self.vt52_mode == Decanm::Vt52 {
                        SetMode::DecRst
                    } else {
                        SetMode::DecSet
                    };
                    self.write_to_pty(&Decanm::Ansi.report(Some(mode)));
                }
                // ── Modes handled by TerminalState's mode-sync loop ──
                // These are GUI/input-concern modes tracked in
                // TerminalState::modes.  TerminalHandler does not act on
                // them; listing them explicitly silences spurious debug
                // noise from the catch-all.
                // GraphemeClustering Set/Reset are also silently accepted
                // here — Freminal does grapheme clustering unconditionally.
                Mode::Decckm(_)
                | Mode::BracketedPaste(_)
                | Mode::MouseMode(_)
                | Mode::MouseEncodingMode(_)
                | Mode::XtMseWin(_)
                | Mode::Decscnm(_)
                | Mode::Decarm(_)
                | Mode::SynchronizedUpdates(_)
                | Mode::Decnkm(_)
                | Mode::Decbkm(_)
                | Mode::AlternateScroll(_)
                | Mode::GraphemeClustering(
                    GraphemeClustering::Unicode | GraphemeClustering::Legacy,
                ) => {}

                // ── Application Escape Key (?7727) ────────────────────
                Mode::ApplicationEscapeKey(ApplicationEscapeKey::Set) => {
                    self.application_escape_key = ApplicationEscapeKey::Set;
                }
                Mode::ApplicationEscapeKey(ApplicationEscapeKey::Reset) => {
                    self.application_escape_key = ApplicationEscapeKey::Reset;
                }
                Mode::ApplicationEscapeKey(ApplicationEscapeKey::Query) => {
                    let mode = if self.application_escape_key == ApplicationEscapeKey::Set {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&ApplicationEscapeKey::Set.report(Some(mode)));
                }

                // ── In-Band Resize Notifications (?2048) ──────────────
                Mode::InBandResizeMode(InBandResizeMode::Set) => {
                    self.in_band_resize_enabled = true;
                    // Send an immediate resize notification per the specification
                    self.send_in_band_resize();
                }
                Mode::InBandResizeMode(InBandResizeMode::Reset) => {
                    self.in_band_resize_enabled = false;
                }
                Mode::InBandResizeMode(InBandResizeMode::Query) => {
                    let mode = if self.in_band_resize_enabled {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&InBandResizeMode::Set.report(Some(mode)));
                }

                // ── Grapheme Clustering (?2027) — permanently on ────
                // Freminal unconditionally uses unicode-segmentation's
                // graphemes(true), so Query always reports ";3$y"
                // (permanently set). Set/Reset are in the catch-all above.
                Mode::GraphemeClustering(GraphemeClustering::Query) => {
                    self.write_to_pty(&GraphemeClustering::Unicode.report(None));
                }

                // ── Insert/Replace Mode (IRM, ANSI mode 4) ───────────
                Mode::Irm(irm) => {
                    self.insert_mode = *irm;
                }

                // ── Modes parsed but not yet acted on ─────────────────
                Mode::NoOp | Mode::Decsclm(_) | Mode::Theming(_) | Mode::Unknown(_) => {
                    tracing::warn!("Mode not acted on by TerminalHandler: {mode}");
                }
            },
            TerminalOutput::OscResponse(osc) => {
                self.handle_osc(osc);
            }
            TerminalOutput::CursorReport => {
                self.handle_cursor_report();
            }
            TerminalOutput::ColorThemeReport => {
                self.handle_color_theme_report();
            }
            TerminalOutput::DeviceStatusReport => {
                self.handle_device_status_report();
            }
            TerminalOutput::DecSpecialGraphics(dsg) => {
                self.character_replace = dsg.clone();
            }
            TerminalOutput::CursorVisualStyle(style) => {
                self.cursor_visual_style = style.clone();
            }
            TerminalOutput::WindowManipulation(wm) => {
                self.handle_window_manipulation(wm);
            }
            TerminalOutput::RequestDeviceAttributes => {
                self.handle_request_device_attributes();
            }
            TerminalOutput::EightBitControl => {
                tracing::debug!("EightBitControl (S8C1T) — mode sync handled by TerminalState");
            }
            TerminalOutput::SevenBitControl => {
                tracing::debug!("SevenBitControl (S7C1T) — mode sync handled by TerminalState");
            }
            TerminalOutput::AnsiConformanceLevelOne => {
                tracing::warn!("AnsiConformanceLevelOne not yet implemented (ignored)");
            }
            TerminalOutput::AnsiConformanceLevelTwo => {
                tracing::warn!("AnsiConformanceLevelTwo not yet implemented (ignored)");
            }
            TerminalOutput::AnsiConformanceLevelThree => {
                tracing::warn!("AnsiConformanceLevelThree not yet implemented (ignored)");
            }
            TerminalOutput::DoubleLineHeightTop => {
                self.buffer
                    .set_cursor_line_width(crate::row::LineWidth::DoubleHeightTop);
            }
            TerminalOutput::DoubleLineHeightBottom => {
                self.buffer
                    .set_cursor_line_width(crate::row::LineWidth::DoubleHeightBottom);
            }
            TerminalOutput::SingleWidthLine => {
                self.buffer
                    .set_cursor_line_width(crate::row::LineWidth::Normal);
            }
            TerminalOutput::DoubleWidthLine => {
                self.buffer
                    .set_cursor_line_width(crate::row::LineWidth::DoubleWidth);
            }
            TerminalOutput::ScreenAlignmentTest => {
                self.buffer.screen_alignment_test();
            }
            TerminalOutput::CharsetDefault
            | TerminalOutput::CharsetUTF8
            | TerminalOutput::CharsetG0
            | TerminalOutput::CharsetG1
            | TerminalOutput::CharsetG1AsGR
            | TerminalOutput::CharsetG2
            | TerminalOutput::CharsetG2AsGR
            | TerminalOutput::CharsetG2AsGL
            | TerminalOutput::CharsetG3
            | TerminalOutput::CharsetG3AsGR
            | TerminalOutput::CharsetG3AsGL
            | TerminalOutput::DecSpecial
            | TerminalOutput::CharsetUK
            | TerminalOutput::CharsetUS
            | TerminalOutput::CharsetUSASCII
            | TerminalOutput::CharsetDutch
            | TerminalOutput::CharsetFinnish
            | TerminalOutput::CharsetFrench
            | TerminalOutput::CharsetFrenchCanadian
            | TerminalOutput::CharsetGerman
            | TerminalOutput::CharsetItalian
            | TerminalOutput::CharsetNorwegianDanish
            | TerminalOutput::CharsetSpanish
            | TerminalOutput::CharsetSwedish
            | TerminalOutput::CharsetSwiss => {
                tracing::warn!(
                    "Charset/line-drawing designation not yet implemented (ignored): {output}"
                );
            }
            TerminalOutput::SaveCursor => {
                self.handle_save_cursor();
            }
            TerminalOutput::RestoreCursor => {
                self.handle_restore_cursor();
            }
            TerminalOutput::CursorToLowerLeftCorner => {
                tracing::warn!("CursorToLowerLeftCorner not yet implemented (ignored)");
            }
            TerminalOutput::ResetDevice => {
                self.full_reset();
            }
            TerminalOutput::MemoryLock => {
                tracing::warn!("MemoryLock not yet implemented (ignored)");
            }
            TerminalOutput::MemoryUnlock => {
                tracing::warn!("MemoryUnlock not yet implemented (ignored)");
            }
            TerminalOutput::DeviceControlString(dcs) => {
                self.handle_device_control_string(dcs);
            }
            TerminalOutput::ApplicationProgramCommand(apc) => {
                self.handle_application_program_command(apc);
            }
            TerminalOutput::RequestTertiaryDeviceAttributes => {
                self.handle_tertiary_device_attributes();
            }
            TerminalOutput::RequestTerminalParameters(ps) => {
                self.handle_request_terminal_parameters(*ps);
            }
            TerminalOutput::RequestDeviceNameAndVersion => {
                self.handle_device_name_and_version();
            }
            TerminalOutput::RequestSecondaryDeviceAttributes { param: _param } => {
                self.handle_secondary_device_attributes();
            }
            TerminalOutput::KittyKeyboardQuery => {
                let flags = self.kitty_keyboard_flags();
                tracing::debug!("KittyKeyboardQuery received, flags={flags}");
                self.write_to_pty(&format!("\x1b[?{flags}u"));
            }
            TerminalOutput::KittyKeyboardPush(flags) => {
                if self.kitty_keyboard_stack.len() >= KittyKeyboardFlags::MAX_STACK_DEPTH {
                    // Evict the oldest entry (bottom of the stack) per the spec.
                    self.kitty_keyboard_stack.remove(0);
                }
                self.kitty_keyboard_stack.push(*flags);
            }
            TerminalOutput::KittyKeyboardPop(n) => {
                let n = (*n as usize).min(self.kitty_keyboard_stack.len());
                let new_len = self.kitty_keyboard_stack.len() - n;
                self.kitty_keyboard_stack.truncate(new_len);
            }
            TerminalOutput::KittyKeyboardSet { flags, mode } => {
                let current = self.kitty_keyboard_flags();
                let new_flags = match mode {
                    1 => *flags,
                    2 => current | *flags,
                    3 => current & !*flags,
                    _ => current,
                };
                if self.kitty_keyboard_stack.is_empty() {
                    self.kitty_keyboard_stack.push(new_flags);
                } else {
                    let top = self.kitty_keyboard_stack.len() - 1;
                    self.kitty_keyboard_stack[top] = new_flags;
                }
            }
            TerminalOutput::ModifyOtherKeys(level) => {
                self.modify_other_keys_level = *level;
            }
            TerminalOutput::Enq => {
                // ENQ — transmit answerback message.
                // Most modern terminals send an empty string; we do the same.
                self.write_to_pty("");
            }
            // Silently ignore `Invalid`, `Skipped`, and any future variants.
            //
            // `Invalid` — a sequence the parser recognised as malformed; the
            //   error was already logged by the parser.  There is nothing the
            //   buffer can do with it.
            //
            // `Skipped` — a sequence the parser intentionally dropped (e.g.
            //   an OSC that was too long or a DCS string with an unknown
            //   introducer).  Ignored for the same reason.
            //
            // `_` (catch-all) — forward compatibility: new `TerminalOutput`
            //   variants added in future will not cause a compile error or
            //   panic here.  Any variant that needs buffer-level handling must
            //   be added as an explicit arm above.
            TerminalOutput::Invalid | TerminalOutput::Skipped | _ => {}
        }
    }
}

/// Remap bytes in `data` according to the DEC Special Graphics character set.
///
/// When `mode` is `DecSpecialGraphics::Replace`, bytes in the range `0x5F–0x7E`
/// are expanded to their Unicode UTF-8 equivalents (box-drawing, Greek, etc.).
/// All other bytes are passed through unchanged. When `mode` is `DontReplace`
/// the slice is copied verbatim.
///
/// Reference: <https://en.wikipedia.org/wiki/DEC_Special_Graphics>
///
/// Returns `Cow::Borrowed(data)` when no remapping is needed (`DontReplace` mode),
/// avoiding any heap allocation in the overwhelmingly common case.
fn apply_dec_special<'a>(data: &'a [u8], mode: &DecSpecialGraphics) -> Cow<'a, [u8]> {
    match mode {
        DecSpecialGraphics::DontReplace => Cow::Borrowed(data),
        DecSpecialGraphics::Replace => {
            let mut out = Vec::with_capacity(data.len() * 3);
            for &b in data {
                match b {
                    0x5f => out.extend_from_slice("\u{00A0}".as_bytes()), // NO-BREAK SPACE
                    0x60 => out.extend_from_slice("\u{25C6}".as_bytes()), // BLACK DIAMOND
                    0x61 => out.extend_from_slice("\u{2592}".as_bytes()), // MEDIUM SHADE
                    0x62 => out.extend_from_slice("\u{2409}".as_bytes()), // SYMBOL FOR HT
                    0x63 => out.extend_from_slice("\u{240C}".as_bytes()), // SYMBOL FOR FF
                    0x64 => out.extend_from_slice("\u{240D}".as_bytes()), // SYMBOL FOR CR
                    0x65 => out.extend_from_slice("\u{240A}".as_bytes()), // SYMBOL FOR LF
                    0x66 => out.extend_from_slice("\u{00B0}".as_bytes()), // DEGREE SIGN
                    0x67 => out.extend_from_slice("\u{00B1}".as_bytes()), // PLUS-MINUS SIGN
                    0x68 => out.extend_from_slice("\u{2424}".as_bytes()), // SYMBOL FOR NEWLINE
                    0x69 => out.extend_from_slice("\u{240B}".as_bytes()), // SYMBOL FOR VT
                    0x6a => out.extend_from_slice("\u{2518}".as_bytes()), // BOX LIGHT UP AND LEFT
                    0x6b => out.extend_from_slice("\u{2510}".as_bytes()), // BOX LIGHT DOWN AND LEFT
                    0x6c => out.extend_from_slice("\u{250C}".as_bytes()), // BOX LIGHT DOWN AND RIGHT
                    0x6d => out.extend_from_slice("\u{2514}".as_bytes()), // BOX LIGHT UP AND RIGHT
                    0x6e => out.extend_from_slice("\u{253C}".as_bytes()), // BOX LIGHT VERTICAL AND HORIZONTAL
                    0x6f => out.extend_from_slice("\u{23BA}".as_bytes()), // HORIZONTAL SCAN LINE-1
                    0x70 => out.extend_from_slice("\u{23BB}".as_bytes()), // HORIZONTAL SCAN LINE-3
                    0x71 => out.extend_from_slice("\u{2500}".as_bytes()), // BOX LIGHT HORIZONTAL
                    0x72 => out.extend_from_slice("\u{23BC}".as_bytes()), // HORIZONTAL SCAN LINE-7
                    0x73 => out.extend_from_slice("\u{23BD}".as_bytes()), // HORIZONTAL SCAN LINE-9
                    0x74 => out.extend_from_slice("\u{251C}".as_bytes()), // BOX LIGHT VERTICAL AND RIGHT
                    0x75 => out.extend_from_slice("\u{2524}".as_bytes()), // BOX LIGHT VERTICAL AND LEFT
                    0x76 => out.extend_from_slice("\u{2534}".as_bytes()), // BOX LIGHT UP AND HORIZONTAL
                    0x77 => out.extend_from_slice("\u{252C}".as_bytes()), // BOX LIGHT DOWN AND HORIZONTAL
                    0x78 => out.extend_from_slice("\u{2502}".as_bytes()), // BOX LIGHT VERTICAL
                    0x79 => out.extend_from_slice("\u{2264}".as_bytes()), // LESS-THAN OR EQUAL TO
                    0x7a => out.extend_from_slice("\u{2265}".as_bytes()), // GREATER-THAN OR EQUAL TO
                    0x7b => out.extend_from_slice("\u{03C0}".as_bytes()), // GREEK SMALL LETTER PI
                    0x7c => out.extend_from_slice("\u{2260}".as_bytes()), // NOT EQUAL TO
                    0x7d => out.extend_from_slice("\u{00A3}".as_bytes()), // POUND SIGN
                    0x7e => out.extend_from_slice("\u{00B7}".as_bytes()), // MIDDLE DOT
                    _ => out.push(b),
                }
            }
            Cow::Owned(out)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use freminal_common::{
        buffer_states::{
            fonts::{BlinkState, FontWeight},
            terminal_output::TerminalOutput,
        },
        colors::TerminalColor,
        sgr::SelectGraphicRendition,
    };

    use super::*;

    #[test]
    fn process_output_blink_then_data() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[
            TerminalOutput::Sgr(SelectGraphicRendition::SlowBlink),
            TerminalOutput::Data(b"Hello".to_vec()),
        ]);
        assert_eq!(handler.current_format.blink, BlinkState::Slow);
    }

    #[test]
    fn process_output_bold_and_blink_then_data() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[
            TerminalOutput::Sgr(SelectGraphicRendition::Bold),
            TerminalOutput::Sgr(SelectGraphicRendition::SlowBlink),
            TerminalOutput::Data(b"BoldBlink".to_vec()),
        ]);
        assert_eq!(handler.current_format.font_weight, FontWeight::Bold);
        assert_eq!(handler.current_format.blink, BlinkState::Slow);
    }

    #[test]
    fn process_output_sgr_bold_then_data() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[
            TerminalOutput::Sgr(SelectGraphicRendition::Bold),
            TerminalOutput::Data(b"A".to_vec()),
        ]);
        // After writing, current format should still be bold
        assert_eq!(handler.current_format.font_weight, FontWeight::Bold);
    }

    #[test]
    fn process_output_sgr_fg_color_then_reset() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[
            TerminalOutput::Sgr(SelectGraphicRendition::Foreground(TerminalColor::Green)),
            TerminalOutput::Sgr(SelectGraphicRendition::Reset),
        ]);
        assert_eq!(handler.current_format, FormatTag::default());
    }

    #[test]
    fn test_handler_creation() {
        let handler = TerminalHandler::new(80, 24);
        assert_eq!(handler.buffer().get_cursor().pos.x, 0);
        assert_eq!(handler.buffer().get_cursor().pos.y, 0);
    }

    #[test]
    fn test_handle_data() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"Hello");

        assert_eq!(handler.buffer().get_cursor().pos.x, 5);
        assert_eq!(handler.buffer().get_cursor().pos.y, 0);
    }

    #[test]
    fn test_handle_newline() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"Hello");
        handler.handle_newline();

        assert_eq!(handler.buffer().get_cursor().pos.y, 1);
    }

    #[test]
    fn test_handle_cursor_movement() {
        let mut handler = TerminalHandler::new(80, 24);

        // Move to position (10, 5) - parser sends 1-indexed
        handler.handle_cursor_pos(Some(11), Some(6));
        assert_eq!(handler.buffer().get_cursor().pos.x, 10);
        assert_eq!(handler.buffer().get_cursor().pos.y, 5);

        // Move right 5
        handler.handle_cursor_forward(5);
        assert_eq!(handler.buffer().get_cursor().pos.x, 15);

        // Move up 2
        handler.handle_cursor_up(2);
        assert_eq!(handler.buffer().get_cursor().pos.y, 3);
    }

    #[test]
    fn test_handle_erase_operations() {
        let mut handler = TerminalHandler::new(10, 5);

        // Fill with data
        handler.handle_data(b"Line1");
        handler.handle_newline();
        handler.handle_data(b"Line2");
        handler.handle_newline();
        handler.handle_data(b"Line3");

        // Move cursor to middle
        handler.handle_cursor_pos(Some(1), Some(2));

        // Erase to end of line
        handler.handle_erase_in_line(0);

        // The line should be partially cleared
        let rows = handler.buffer().visible_rows(0);
        assert!(rows.len() >= 2);
    }

    #[test]
    fn test_handle_insert_delete_lines() {
        let mut handler = TerminalHandler::new(10, 5);

        handler.handle_data(b"Line1");
        handler.handle_newline();
        handler.handle_data(b"Line2");

        // Go back to first line
        handler.handle_cursor_pos(Some(1), Some(1));

        // Insert a line
        handler.handle_insert_lines(1);

        let rows = handler.buffer().visible_rows(0);
        // Should have inserted a blank line, pushing content down
        assert!(rows.len() >= 2);
    }

    #[test]
    fn test_handle_scroll_region() {
        let mut handler = TerminalHandler::new(80, 24);

        // Set scroll region from line 5 to line 20 (1-based from parser)
        handler.handle_set_scroll_region(5, 20);

        // Buffer stores 0-based inclusive: (4, 19)
        let (top, bottom) = handler.buffer().scroll_region();
        assert_eq!(top, 4, "top should be 0-based (5-1=4)");
        assert_eq!(bottom, 19, "bottom should be 0-based (20-1=19)");
    }

    #[test]
    fn test_handle_scroll_region_full_screen() {
        let mut handler = TerminalHandler::new(80, 24);

        // CSI r with no params: parser sends (1, usize::MAX)
        handler.handle_set_scroll_region(1, usize::MAX);

        // usize::MAX >= height → invalid → resets to full screen [0, 23]
        let (top, bottom) = handler.buffer().scroll_region();
        assert_eq!(top, 0);
        assert_eq!(bottom, 23);
    }

    #[test]
    fn test_handle_scroll_region_single_row() {
        let mut handler = TerminalHandler::new(80, 24);

        // top == bottom (1-based) → 0-based top >= bottom → resets to full screen
        handler.handle_set_scroll_region(5, 5);
        let (top, bottom) = handler.buffer().scroll_region();
        assert_eq!(top, 0);
        assert_eq!(bottom, 23);
    }

    #[test]
    fn test_handle_scroll_region_inverted() {
        let mut handler = TerminalHandler::new(80, 24);

        // Inverted range → invalid → resets to full screen
        handler.handle_set_scroll_region(20, 5);
        let (top, bottom) = handler.buffer().scroll_region();
        assert_eq!(top, 0);
        assert_eq!(bottom, 23);
    }

    #[test]
    fn test_handle_scroll_region_bottom_beyond_screen() {
        let mut handler = TerminalHandler::new(80, 24);

        // bottom (1-based 30) exceeds height (24) → 0-based 29 >= 24 → resets
        handler.handle_set_scroll_region(1, 30);
        let (top, bottom) = handler.buffer().scroll_region();
        assert_eq!(top, 0);
        assert_eq!(bottom, 23);
    }

    #[test]
    fn test_handle_scroll_region_exact_full_screen() {
        let mut handler = TerminalHandler::new(80, 24);

        // (1, 24) in 1-based → (0, 23) in 0-based → valid, full screen
        handler.handle_set_scroll_region(1, 24);
        let (top, bottom) = handler.buffer().scroll_region();
        assert_eq!(top, 0);
        assert_eq!(bottom, 23);
    }

    #[test]
    fn test_tab_from_column_0() {
        let mut handler = TerminalHandler::new(80, 24);
        // Cursor starts at col 0
        assert_eq!(handler.buffer().get_cursor().pos.x, 0);
        handler.handle_tab();
        // Should advance to column 8 (first default tab stop)
        assert_eq!(handler.buffer().get_cursor().pos.x, 8);
    }

    #[test]
    fn test_tab_from_column_7() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"1234567"); // 7 chars → cursor at col 7
        handler.handle_tab();
        // Column 7 → next tab stop is column 8
        assert_eq!(handler.buffer().get_cursor().pos.x, 8);
    }

    #[test]
    fn test_tab_from_tab_stop() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"12345678"); // 8 chars → cursor at col 8
        handler.handle_tab();
        // Column 8 is a tab stop → next is column 16
        assert_eq!(handler.buffer().get_cursor().pos.x, 16);
    }

    #[test]
    fn test_tab_multiple() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_tab(); // 0 → 8
        handler.handle_tab(); // 8 → 16
        handler.handle_tab(); // 16 → 24
        assert_eq!(handler.buffer().get_cursor().pos.x, 24);
    }

    #[test]
    fn test_tab_near_end_of_line() {
        let mut handler = TerminalHandler::new(80, 24);
        // Move cursor to column 75
        handler.handle_cursor_pos(Some(76), Some(1)); // 1-based
        handler.handle_tab();
        // Last tab stop in 80-col terminal is col 72 (8*9=72).
        // At col 75, no more tab stops → goes to col 79 (rightmost)
        assert_eq!(handler.buffer().get_cursor().pos.x, 79);
    }

    #[test]
    fn test_tab_does_not_wrap() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(80), Some(1)); // 1-based → col 79
        let y_before = handler.buffer().get_cursor().pos.y;
        handler.handle_tab();
        // Should stay at col 79, not wrap
        assert_eq!(handler.buffer().get_cursor().pos.x, 79);
        assert_eq!(handler.buffer().get_cursor().pos.y, y_before);
    }

    #[test]
    fn test_alternate_buffer() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_data(b"Primary");
        handler.handle_enter_alternate();
        handler.handle_data(b"Alternate");

        // Verify we're in alternate buffer
        handler.handle_leave_alternate();

        // Should restore primary buffer
        // (exact verification requires exposing buffer state)
    }

    #[test]
    fn test_backspace() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_data(b"Hello");
        assert_eq!(handler.buffer().get_cursor().pos.x, 5);

        handler.handle_backspace();
        assert_eq!(handler.buffer().get_cursor().pos.x, 4);
    }

    #[test]
    fn test_process_outputs() {
        use freminal_common::buffer_states::terminal_output::TerminalOutput;

        let mut handler = TerminalHandler::new(80, 24);

        let outputs = vec![
            TerminalOutput::Data(b"Hello".to_vec()),
            TerminalOutput::Newline,
            TerminalOutput::CarriageReturn,
            TerminalOutput::Data(b"World".to_vec()),
        ];

        handler.process_outputs(&outputs);

        assert_eq!(handler.buffer().get_cursor().pos.y, 1);
        assert_eq!(handler.buffer().get_cursor().pos.x, 5);
    }

    #[test]
    fn test_process_cursor_movements() {
        use freminal_common::buffer_states::terminal_output::TerminalOutput;

        let mut handler = TerminalHandler::new(80, 24);

        let outputs = vec![
            TerminalOutput::SetCursorPos {
                x: Some(11),
                y: Some(6),
            },
            TerminalOutput::Data(b"Test".to_vec()),
        ];

        handler.process_outputs(&outputs);

        assert_eq!(handler.buffer().get_cursor().pos.x, 14); // 10 + 4
        assert_eq!(handler.buffer().get_cursor().pos.y, 5); // 5 (0-indexed)
    }

    #[test]
    fn test_process_erase_operations() {
        use freminal_common::buffer_states::terminal_output::TerminalOutput;

        let mut handler = TerminalHandler::new(80, 24);

        let outputs = vec![
            TerminalOutput::Data(b"Line 1".to_vec()),
            TerminalOutput::Newline,
            TerminalOutput::CarriageReturn,
            TerminalOutput::Data(b"Line 2".to_vec()),
            TerminalOutput::ClearDisplay,
        ];

        handler.process_outputs(&outputs);

        // Screen should be cleared — buffer only grew to 2 rows (one per Data+Newline),
        // so visible_rows() returns those 2 rows (both now empty after ClearDisplay).
        let visible = handler.buffer().visible_rows(0);
        assert_eq!(visible.len(), 2);
        // Both rows must be empty after the clear.
        for row in visible {
            assert!(
                row.get_characters().is_empty(),
                "all visible rows must be empty after ClearDisplay"
            );
        }
    }

    #[test]
    fn gui_data_visible_only() {
        // Write two rows — scrollback must be empty, visible must have content.
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_data(b"row one");
        handler.handle_newline();
        handler.handle_carriage_return();
        handler.handle_data(b"row two");

        let (chars, tags) = handler.data_and_format_data_for_gui(0);

        // Nothing has scrolled off yet — scrollback must be empty.
        assert!(
            chars.scrollback.is_empty(),
            "scrollback chars must be empty when nothing has scrolled off"
        );
        assert!(
            tags.scrollback.is_empty(),
            "scrollback tags must be empty when nothing has scrolled off"
        );

        // Visible section must contain the written text.
        let visible_str: String = chars
            .visible
            .iter()
            .map(|c| match c {
                freminal_common::buffer_states::tchar::TChar::Ascii(b) => (*b as char).to_string(),
                freminal_common::buffer_states::tchar::TChar::Space => " ".to_string(),
                freminal_common::buffer_states::tchar::TChar::NewLine => "\n".to_string(),
                freminal_common::buffer_states::tchar::TChar::Utf8(buf, len) => {
                    String::from_utf8_lossy(&buf[..*len as usize]).to_string()
                }
            })
            .collect();

        assert!(
            visible_str.contains("row one"),
            "visible must contain 'row one'"
        );
        assert!(
            visible_str.contains("row two"),
            "visible must contain 'row two'"
        );
        assert!(!tags.visible.is_empty(), "visible tags must not be empty");
    }

    #[test]
    fn gui_data_scrollback_present() {
        // Use a very small terminal so lines scroll off quickly.
        let mut handler = TerminalHandler::new(80, 3);

        // Write more lines than the visible height so some content is pushed into scrollback.
        for i in 0..10_u8 {
            handler.handle_data(&[b'A' + i]);
            handler.handle_newline();
            handler.handle_carriage_return();
        }

        let (chars, tags) = handler.data_and_format_data_for_gui(0);

        // Some content must have scrolled off.
        assert!(
            !chars.scrollback.is_empty(),
            "scrollback chars must be non-empty after many lines"
        );
        assert!(
            !tags.scrollback.is_empty(),
            "scrollback tags must be non-empty after many lines"
        );

        // Visible section is still populated.
        assert!(!chars.visible.is_empty(), "visible chars must not be empty");
    }

    #[test]
    fn cursor_pos_accessor() {
        let mut handler = TerminalHandler::new(80, 24);

        // Move the cursor to a known position then verify the accessor.
        handler.handle_cursor_pos(Some(5), Some(3));

        let pos = handler.cursor_pos();
        assert_eq!(
            pos.x, 4,
            "cursor x should be 4 (0-indexed from 1-indexed 5)"
        );
        assert_eq!(
            pos.y, 2,
            "cursor y should be 2 (0-indexed from 1-indexed 3)"
        );
    }

    #[test]
    fn win_size_accessor() {
        let handler = TerminalHandler::new(132, 48);

        let (w, h) = handler.get_win_size();
        assert_eq!(w, 132, "width must match constructor argument");
        assert_eq!(h, 48, "height must match constructor argument");
    }

    #[test]
    fn default_scrollback_limit_is_4000() {
        let handler = TerminalHandler::new(80, 24);
        assert_eq!(handler.buffer().scrollback_limit(), 4000);
    }

    #[test]
    fn with_scrollback_limit_overrides_default() {
        let handler = TerminalHandler::new(80, 24).with_scrollback_limit(200);
        assert_eq!(handler.buffer().scrollback_limit(), 200);
    }

    // ------------------------------------------------------------------
    // OSC 52 clipboard tests
    // ------------------------------------------------------------------

    #[test]
    fn handle_osc_set_clipboard_pushes_window_command() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::SetClipboard(
            "c".to_string(),
            "hello world".to_string(),
        ));
        let cmds = handler.take_window_commands();
        assert_eq!(cmds.len(), 1);
        assert!(
            matches!(&cmds[0], WindowManipulation::SetClipboard(sel, content) if sel == "c" && content == "hello world")
        );
    }

    #[test]
    fn handle_osc_query_clipboard_pushes_window_command() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::QueryClipboard("c".to_string()));
        let cmds = handler.take_window_commands();
        assert_eq!(cmds.len(), 1);
        assert!(matches!(
            &cmds[0],
            WindowManipulation::QueryClipboard(sel) if sel == "c"
        ));
    }

    #[test]
    fn handle_osc_set_clipboard_primary_selection() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::SetClipboard(
            "p".to_string(),
            "primary text".to_string(),
        ));
        let cmds = handler.take_window_commands();
        assert_eq!(cmds.len(), 1);
        assert!(
            matches!(&cmds[0], WindowManipulation::SetClipboard(sel, content) if sel == "p" && content == "primary text")
        );
    }

    #[test]
    fn handle_osc_clipboard_commands_are_drained_by_take() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::SetClipboard(
            "c".to_string(),
            "first".to_string(),
        ));
        handler.handle_osc(&AnsiOscType::QueryClipboard("s".to_string()));
        let cmds = handler.take_window_commands();
        assert_eq!(cmds.len(), 2);

        // After take, window_commands should be empty
        let cmds2 = handler.take_window_commands();
        assert!(cmds2.is_empty());
    }

    // ── Palette (OSC 4 / OSC 104) tests ─────────────────────────────────

    #[test]
    fn palette_default_is_empty_overrides() {
        let handler = TerminalHandler::new(80, 24);
        // By default the palette has no overrides — all lookups hit defaults.
        assert_eq!(
            handler.palette(),
            &freminal_common::colors::ColorPalette::default()
        );
    }

    #[test]
    fn handle_osc_set_palette_color() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::SetPaletteColor(42, 0xAA, 0xBB, 0xCC));

        let (r, g, b) = handler.palette().get_rgb(42, handler.theme());
        assert_eq!((r, g, b), (0xAA, 0xBB, 0xCC));
    }

    #[test]
    fn handle_osc_query_palette_color_sends_response() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set index 10 to a known value first.
        handler.handle_osc(&AnsiOscType::SetPaletteColor(10, 0xFF, 0x80, 0x00));

        // Query it.
        handler.handle_osc(&AnsiOscType::QueryPaletteColor(10));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response from query");
        };
        let Ok(response) = String::from_utf8(bytes) else {
            panic!("response should be valid UTF-8");
        };
        // 0xFF * 257 = 0xFFFF (65535), 0x80 * 257 = 0x8080 (32896), 0x00 * 257 = 0x0000
        assert_eq!(response, "\x1b]4;10;rgb:ffff/8080/0000\x1b\\");
    }

    #[test]
    fn handle_osc_query_palette_default_index() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query index 0 (Black in Catppuccin Mocha) without setting an override.
        handler.handle_osc(&AnsiOscType::QueryPaletteColor(0));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response from query");
        };
        let Ok(response) = String::from_utf8(bytes) else {
            panic!("response should be valid UTF-8");
        };
        // Index 0 default is (69, 71, 90) -> 69*257=17733=0x4545, 71*257=18247=0x4747,
        // 90*257=23130=0x5a5a
        assert_eq!(response, "\x1b]4;0;rgb:4545/4747/5a5a\x1b\\");
    }

    #[test]
    fn handle_osc_reset_palette_single_index() {
        let mut handler = TerminalHandler::new(80, 24);

        // Set index 5 to a custom value.
        handler.handle_osc(&AnsiOscType::SetPaletteColor(5, 0x11, 0x22, 0x33));
        assert_eq!(
            handler.palette().get_rgb(5, handler.theme()),
            (0x11, 0x22, 0x33)
        );

        // Reset just index 5.
        handler.handle_osc(&AnsiOscType::ResetPaletteColor(Some(5)));

        // Should revert to the default for index 5.
        let default_rgb = freminal_common::colors::default_index_to_rgb(5, handler.theme());
        assert_eq!(handler.palette().get_rgb(5, handler.theme()), default_rgb);
    }

    #[test]
    fn handle_osc_reset_palette_all() {
        let mut handler = TerminalHandler::new(80, 24);

        // Set a few indices.
        handler.handle_osc(&AnsiOscType::SetPaletteColor(0, 0xFF, 0x00, 0x00));
        handler.handle_osc(&AnsiOscType::SetPaletteColor(100, 0x00, 0xFF, 0x00));
        handler.handle_osc(&AnsiOscType::SetPaletteColor(255, 0x00, 0x00, 0xFF));

        // Reset all.
        handler.handle_osc(&AnsiOscType::ResetPaletteColor(None));

        // All should revert to defaults.
        assert_eq!(
            handler.palette(),
            &freminal_common::colors::ColorPalette::default()
        );
    }

    #[test]
    fn full_reset_clears_palette() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::SetPaletteColor(42, 0xFF, 0x00, 0xFF));
        assert_ne!(
            handler.palette(),
            &freminal_common::colors::ColorPalette::default()
        );

        handler.full_reset();

        assert_eq!(
            handler.palette(),
            &freminal_common::colors::ColorPalette::default(),
            "full_reset must clear all palette overrides"
        );
    }

    #[test]
    fn bell_pushes_window_command() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(
            handler.window_commands.is_empty(),
            "no window commands initially"
        );

        handler.process_outputs(&[TerminalOutput::Bell]);

        assert_eq!(handler.window_commands.len(), 1);
        assert!(
            matches!(
                handler.window_commands[0],
                freminal_common::buffer_states::window_manipulation::WindowManipulation::Bell
            ),
            "Bell output should produce WindowManipulation::Bell"
        );
    }

    #[test]
    fn multiple_bells_push_multiple_commands() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.process_outputs(&[TerminalOutput::Bell, TerminalOutput::Bell]);

        assert_eq!(
            handler.window_commands.len(),
            2,
            "each Bell output should produce one WindowManipulation::Bell"
        );
    }
}
