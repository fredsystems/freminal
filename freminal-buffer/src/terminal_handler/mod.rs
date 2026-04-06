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
        modes::irm::Irm,
        modes::kitty_keyboard::KittyKeyboardFlags,
        modes::lnm::Lnm,
        modes::modify_other_keys_mode::ModifyOtherKeysMode,
        modes::private_color_registers::PrivateColorRegisters,
        modes::reverse_wrap_around::ReverseWrapAround,
        modes::s8c1t::S8c1t,
        modes::xt_rev_wrap2::XtRevWrap2,
        modes::xtcblink::XtCBlink,
        modes::xtextscrn::{AltScreen47, SaveCursor1048, XtExtscrn},
        osc::{
            AnsiOscInternalType, AnsiOscType, ITerm2InlineImageData, ImageDimension, UrlResponse,
        },
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
    colors::{ColorPalette, TerminalColor, parse_color_spec},
    cursor::CursorVisualStyle,
    pty_write::{FreminalTerminalSize, PtyWrite},
    themes::ThemePalette,
};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use crate::buffer::Buffer;
use crate::image_store::{ImagePlacement, ImageProtocol, InlineImage, next_image_id};

mod graphics_kitty;
mod graphics_sixel;
mod pty_writer;
mod sgr;

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

    /// Handle a DCS (Device Control String) sequence.
    ///
    /// The raw `dcs` payload includes the leading `P` byte and the trailing `ESC \`
    /// string terminator.  We strip those to get the inner content, then dispatch on
    /// known DCS sub-commands:
    ///
    /// - **DECRQSS** (`$ q <Pt> ST`): Request Selection or Setting.
    /// - **XTGETTCAP** (`+ q <hex> ST`): xterm termcap/terminfo query.
    /// - **tmux passthrough** (`tmux; <inner> ST`): un-doubles ESC bytes and
    ///   dispatches the inner escape sequence to the appropriate handler.
    ///
    /// Unknown or unsupported DCS sub-commands are logged at warn level.
    pub fn handle_device_control_string(&mut self, dcs: &[u8]) {
        tracing::debug!("DCS received: {:?}", String::from_utf8_lossy(dcs));
        // Strip leading 'P' and trailing ESC '\' to get inner content.
        let inner = Self::strip_dcs_envelope(dcs);

        if let Some(pt) = inner.strip_prefix(b"$q") {
            self.handle_decrqss(pt);
        } else if let Some(hex_payload) = inner.strip_prefix(b"+q") {
            self.handle_xtgettcap(hex_payload);
        } else if Self::is_sixel_sequence(inner) {
            self.handle_sixel(inner);
        } else if let Some(payload) = inner.strip_prefix(b"tmux;") {
            self.handle_tmux_passthrough(payload);
        } else {
            tracing::warn!(
                "DCS sub-command not recognized: {}",
                String::from_utf8_lossy(dcs)
            );
        }
    }

    /// Strip the DCS envelope: leading `P` byte and trailing `ESC \` (if present).
    fn strip_dcs_envelope(dcs: &[u8]) -> &[u8] {
        let start = usize::from(dcs.first() == Some(&b'P'));
        let end = if dcs.len() >= 2 && dcs[dcs.len() - 2] == 0x1b && dcs[dcs.len() - 1] == b'\\' {
            dcs.len() - 2
        } else {
            dcs.len()
        };
        if start <= end { &dcs[start..end] } else { &[] }
    }

    /// Un-double ESC bytes in a tmux passthrough payload.
    ///
    /// tmux DCS passthrough encodes the inner escape sequence with every `ESC`
    /// (`0x1b`) byte doubled to `ESC ESC`.  This function reverses that
    /// encoding: consecutive pairs of `0x1b` are collapsed to a single `0x1b`.
    fn undouble_esc(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == 0x1b {
                out.push(0x1b);
                i += 2;
            } else {
                out.push(data[i]);
                i += 1;
            }
        }
        out
    }

    /// Double every ESC byte in `data`.
    ///
    /// This is the inverse of [`undouble_esc`]: each `0x1b` in the input
    /// becomes `0x1b 0x1b` in the output.  Used when wrapping a response
    /// in a DCS tmux passthrough envelope.
    fn double_esc(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + data.len() / 4);
        for &b in data {
            if b == 0x1b {
                out.push(0x1b);
            }
            out.push(b);
        }
        out
    }

    /// Wrap raw response bytes in a DCS tmux passthrough envelope.
    ///
    /// Format: `ESC P tmux; <payload-with-doubled-ESCs> ESC \`
    fn wrap_tmux_passthrough(data: &[u8]) -> Vec<u8> {
        let doubled = Self::double_esc(data);
        // \x1bPtmux; ... \x1b\\
        let mut out = Vec::with_capacity(8 + doubled.len() + 2);
        out.extend_from_slice(b"\x1bPtmux;");
        out.extend_from_slice(&doubled);
        out.extend_from_slice(b"\x1b\\");
        out
    }

    /// Handle a tmux DCS passthrough payload.
    ///
    /// The `payload` is the content after the `tmux;` prefix, with ESC bytes
    /// still doubled.  This method un-doubles the ESC bytes, identifies the
    /// inner escape sequence type from its introducer byte, and dispatches to
    /// the appropriate handler.
    ///
    /// Supported inner sequence types:
    /// - **APC** (`ESC _`): dispatched to [`Self::handle_application_program_command`]
    ///   (e.g. Kitty graphics protocol).
    /// - **DCS** (`ESC P`): dispatched to [`Self::handle_device_control_string`]
    ///   (recursive — the inner DCS is itself unwrapped).
    /// - **OSC** (`ESC ]`): not yet supported (logged at warn level).
    /// - **CSI** (`ESC [`): not yet supported (logged at warn level).
    ///
    /// Any other introducer byte is logged at warn level.
    fn handle_tmux_passthrough(&mut self, payload: &[u8]) {
        if payload.is_empty() {
            tracing::warn!("DCS tmux passthrough: empty payload");
            return;
        }

        let inner = Self::undouble_esc(payload);

        if inner.len() < 2 || inner[0] != 0x1b {
            tracing::warn!(
                "DCS tmux passthrough: inner sequence does not start with ESC: {}",
                String::from_utf8_lossy(&inner)
            );
            return;
        }

        // Set the flag so write_to_pty wraps responses in DCS tmux passthrough.
        self.in_tmux_passthrough = true;

        // The byte after ESC determines the sequence type.
        match inner[1] {
            // APC: ESC _ <content> ESC \   →  pass `_<content>ESC \` to APC handler
            b'_' => {
                tracing::debug!(
                    "DCS tmux passthrough: dispatching APC ({} bytes)",
                    inner.len()
                );
                // The APC handler expects the raw sequence starting with `_`
                // (strip_apc_envelope will remove the `_` prefix and `ESC \` suffix).
                self.handle_application_program_command(&inner[1..]);
            }
            // DCS: ESC P <content> ESC \   →  pass `P<content>ESC \` to DCS handler
            b'P' => {
                tracing::debug!(
                    "DCS tmux passthrough: dispatching DCS ({} bytes)",
                    inner.len()
                );
                // The DCS handler expects the raw sequence starting with `P`
                // (strip_dcs_envelope will remove the `P` prefix and `ESC \` suffix).
                self.handle_device_control_string(&inner[1..]);
            }
            // OSC: ESC ] <content> ESC \   →  queue for re-parsing
            b']' => {
                tracing::debug!(
                    "DCS tmux passthrough: queuing OSC for re-parse ({} bytes)",
                    inner.len()
                );
                self.tmux_reparse_queue.push(inner);
            }
            // CSI: ESC [ <params> <terminator>
            //
            // We dispatch common CSI commands (cursor movement, erase, etc.)
            // directly to avoid ordering issues.  When a DCS-wrapped CUP and
            // a DCS-wrapped APC Kitty Put arrive in the same PTY frame, the
            // CUP must execute before the Put so the cursor is at the correct
            // position.  If the CUP were queued to the reparse queue it would
            // only run *after* all DCS items in the current batch, which is
            // too late.
            //
            // Mode-setting commands (CSI ? ... h/l) and SGR (CSI ... m) are
            // still queued because they need the full parser or
            // TerminalState-level sync.
            b'[' => {
                // inner[0] = ESC, inner[1] = '[', CSI body starts at [2].
                if !self.dispatch_tmux_csi(&inner[2..]) {
                    // Unhandled CSI — fall back to the reparse queue.
                    self.tmux_reparse_queue.push(inner);
                }
            }
            other => {
                tracing::warn!(
                    "DCS tmux passthrough: unknown inner sequence type 0x{other:02x}: {}",
                    String::from_utf8_lossy(&inner)
                );
            }
        }

        // Clear the flag after dispatch so subsequent direct writes are not wrapped.
        self.in_tmux_passthrough = false;
    }

    /// Directly dispatch a CSI sequence from inside a tmux DCS passthrough.
    ///
    /// `csi_body` is the bytes *after* `ESC [` — i.e. the parameter bytes and
    /// the terminator.  Returns `true` if the command was handled directly,
    /// `false` if the caller should fall back to the reparse queue.
    ///
    /// This handles the subset of CSI commands that are purely buffer-level
    /// (cursor movement, erase) so they execute immediately — critical for
    /// correct ordering when a CUP precedes a Kitty Put in the same frame.
    // Inherently large: tmux-passthrough CSI dispatch table. Each arm handles a distinct CSI
    // sequence. Splitting would scatter related escape-sequence handling.
    #[allow(clippy::too_many_lines)]
    fn dispatch_tmux_csi(&mut self, csi_body: &[u8]) -> bool {
        if csi_body.is_empty() {
            return false;
        }

        // CSI parameters that start with '?' are DEC private modes (h/l).
        // These need TerminalState-level sync, so fall back to the reparse queue.
        if csi_body.first() == Some(&b'?') {
            tracing::debug!("DCS tmux passthrough: queuing DEC private CSI for re-parse");
            return false;
        }

        // Find the terminator: the last byte in 0x40..=0x7E range.
        let Some(&terminator) = csi_body.last() else {
            return false;
        };
        if !(0x40..=0x7e).contains(&terminator) {
            return false;
        }

        // Param bytes are everything before the terminator.
        let params = &csi_body[..csi_body.len() - 1];

        // Check for intermediate bytes (0x20..=0x2F) — these indicate
        // extended CSI commands that we don't handle directly.
        if params.iter().any(|&b| (0x20..=0x2f).contains(&b)) {
            tracing::debug!("DCS tmux passthrough: queuing CSI with intermediates for re-parse");
            return false;
        }

        // Parse semicolon-delimited numeric parameters.
        let numeric_params = Self::parse_csi_params(params);

        match terminator {
            // CUP — Cursor Position: ESC [ row ; col H  (or f)
            b'H' | b'f' => {
                let row = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                let col = numeric_params
                    .get(1)
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                tracing::debug!(
                    "DCS tmux passthrough: CSI CUP row={row} col={col} (direct dispatch)"
                );
                self.handle_cursor_pos(Some(col), Some(row));
                true
            }
            // CUU — Cursor Up: ESC [ n A
            b'A' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_cursor_up(n);
                true
            }
            // CUD — Cursor Down: ESC [ n B
            b'B' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_cursor_down(n);
                true
            }
            // CUF — Cursor Forward: ESC [ n C
            b'C' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_cursor_forward(n);
                true
            }
            // CUB — Cursor Backward: ESC [ n D
            b'D' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_cursor_backward(n);
                true
            }
            // CNL — Cursor Next Line: ESC [ n E
            b'E' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                let n_i32 = i32::value_from(n).unwrap_or(i32::MAX);
                self.handle_cursor_relative(0, n_i32);
                self.handle_cursor_pos(Some(1), None);
                true
            }
            // CPL — Cursor Previous Line: ESC [ n F
            b'F' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                let n_i32 = i32::value_from(n).unwrap_or(i32::MAX);
                self.handle_cursor_relative(0, -n_i32);
                self.handle_cursor_pos(Some(1), None);
                true
            }
            // CHA/HPA — Cursor Horizontal Absolute: ESC [ n G  (or `)
            b'G' | b'`' => {
                let col = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_cursor_pos(Some(col), None);
                true
            }
            // VPA — Vertical Position Absolute: ESC [ n d
            b'd' => {
                let row = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_cursor_pos(None, Some(row));
                true
            }
            // ED — Erase in Display: ESC [ n J
            b'J' => {
                let mode = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(0))
                    .unwrap_or(0);
                self.handle_erase_in_display(mode);
                true
            }
            // EL — Erase in Line: ESC [ n K
            b'K' => {
                let mode = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(0))
                    .unwrap_or(0);
                self.handle_erase_in_line(mode);
                true
            }
            // IL — Insert Lines: ESC [ n L
            b'L' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_insert_lines(n);
                true
            }
            // DL — Delete Lines: ESC [ n M
            b'M' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_delete_lines(n);
                true
            }
            // DCH — Delete Characters: ESC [ n P
            b'P' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_delete_chars(n);
                true
            }
            // ECH — Erase Characters: ESC [ n X
            b'X' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_erase_chars(n);
                true
            }
            // ICH — Insert Characters: ESC [ n @
            b'@' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_insert_spaces(n);
                true
            }
            // SU — Scroll Up: ESC [ n S
            b'S' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_scroll_up(n);
                true
            }
            // SD — Scroll Down: ESC [ n T
            b'T' => {
                let n = numeric_params
                    .first()
                    .copied()
                    .unwrap_or(Some(1))
                    .unwrap_or(1)
                    .max(1);
                self.handle_scroll_down(n);
                true
            }
            // DECSTBM — Set Top and Bottom Margins: ESC [ top ; bottom r
            b'r' => {
                let top = numeric_params
                    .first()
                    .copied()
                    .flatten()
                    .unwrap_or(1)
                    .max(1);
                let bottom = numeric_params
                    .get(1)
                    .copied()
                    .flatten()
                    .unwrap_or(usize::MAX);
                self.handle_set_scroll_region(top, bottom);
                true
            }
            // SCOSC — Save Cursor: ESC [ s
            b's' if params.is_empty() => {
                self.buffer.save_cursor();
                true
            }
            // SCORC — Restore Cursor: ESC [ u
            b'u' if params.is_empty() => {
                self.buffer.restore_cursor();
                true
            }
            // SGR and mode-setting (h/l) fall through to the reparse queue.
            // SGR (m) needs the full SGR parser; mode set/reset (h/l) needs
            // TerminalState-level sync.
            _ => {
                tracing::debug!(
                    "DCS tmux passthrough: queuing unhandled CSI '{}'(0x{terminator:02x}) for re-parse",
                    terminator as char,
                );
                false
            }
        }
    }

    /// Parse CSI parameter bytes into a list of `Option<usize>` values.
    ///
    /// Parameters are separated by `;`.  An empty field yields `None`.
    /// For example, `b"1;42"` → `[Some(1), Some(42)]`,
    /// `b""` → `[]`, `b";"` → `[None, None]`.
    fn parse_csi_params(params: &[u8]) -> Vec<Option<usize>> {
        if params.is_empty() {
            return Vec::new();
        }

        let param_str = std::str::from_utf8(params).unwrap_or("");
        param_str
            .split(';')
            .map(|s| {
                if s.is_empty() {
                    None
                } else {
                    s.parse::<usize>().ok()
                }
            })
            .collect()
    }

    /// Handle DECRQSS — Request Selection or Setting.
    ///
    /// `pt` is the setting identifier after stripping the `$q` prefix:
    /// - `m`     → current SGR attributes
    /// - `r`     → current scroll region (DECSTBM)
    /// - `SP q`  → current cursor style (DECSCUSR)  (note: space + q)
    ///
    /// Response format: `DCS Ps $ r Pt ST`
    /// - `Ps = 1` for valid request, `Ps = 0` for invalid.
    fn handle_decrqss(&self, pt: &[u8]) {
        match pt {
            b"m" => {
                let sgr = self.build_sgr_response();
                self.write_dcs_response(&format!("1$r{sgr}m"));
            }
            b"r" => {
                let (top, bottom) = self.buffer.scroll_region();
                // Respond with 1-based row numbers.
                let top_1 = top + 1;
                let bottom_1 = bottom + 1;
                self.write_dcs_response(&format!("1$r{top_1};{bottom_1}r"));
            }
            // SP q = space (0x20) followed by 'q' (0x71)
            b" q" => {
                let style_num = match self.cursor_visual_style() {
                    CursorVisualStyle::BlockCursorBlink => 1,
                    CursorVisualStyle::BlockCursorSteady => 2,
                    CursorVisualStyle::UnderlineCursorBlink => 3,
                    CursorVisualStyle::UnderlineCursorSteady => 4,
                    CursorVisualStyle::VerticalLineCursorBlink => 5,
                    CursorVisualStyle::VerticalLineCursorSteady => 6,
                };
                self.write_dcs_response(&format!("1$r{style_num} q"));
            }
            // "p = DECSCL (Set Conformance Level) query.
            //
            // Response format: DCS 1 $ r Ps1 ; Ps2 " p ST
            //   Ps1 = 6x where x is the conformance level (1–5)
            //   Ps2 = C1 control mode (0 or 2 = 8-bit, 1 = 7-bit)
            //
            // Freminal advertises VT525 (DA1 first param = 65) and uses 7-bit
            // controls by default; when S8C1T is active, report 8-bit.
            b"\"p" => {
                let c1_mode = match self.s8c1t_mode {
                    S8c1t::EightBit => 0,
                    S8c1t::SevenBit => 1,
                };
                self.write_dcs_response(&format!("1$r65;{c1_mode}\"p"));
            }
            _ => {
                // Invalid / unrecognized query → DCS 0 $ r ST
                self.write_dcs_response("0$r");
                tracing::warn!(
                    "DECRQSS: unrecognized setting query: {}",
                    String::from_utf8_lossy(pt)
                );
            }
        }
    }

    /// Handle XTGETTCAP — xterm termcap/terminfo capability query.
    ///
    /// `hex_payload` is the hex-encoded capability name(s) after stripping the `+q`
    /// prefix.  Multiple capability names may be separated by `;` in the hex payload.
    ///
    /// Response: `DCS 1 + r <hex-name> = <hex-value> ST` for known capabilities,
    ///           `DCS 0 + r <hex-name> ST` for unknown ones.
    fn handle_xtgettcap(&self, hex_payload: &[u8]) {
        tracing::debug!(
            "XTGETTCAP query: {:?}",
            String::from_utf8_lossy(hex_payload)
        );
        let payload_str = String::from_utf8_lossy(hex_payload);

        // Split on ';' to support multiple capability queries in a single DCS.
        for hex_name in payload_str.split(';') {
            if hex_name.is_empty() {
                continue;
            }

            let Some(cap_name) = Self::hex_decode(hex_name) else {
                tracing::warn!("XTGETTCAP: invalid hex encoding: {hex_name}");
                self.write_dcs_response(&format!("0+r{hex_name}"));
                continue;
            };

            // "u" — Kitty keyboard protocol flags.  This is instance state
            // (not a static value), so handle it before the static lookup.
            if cap_name == "u" {
                let flags = self.kitty_keyboard_flags();
                let hex_value = Self::hex_encode(&flags.to_string());
                self.write_dcs_response(&format!("1+r{hex_name}={hex_value}"));
                continue;
            }

            if let Some(value) = Self::lookup_termcap(&cap_name) {
                let hex_value = Self::hex_encode(value);
                self.write_dcs_response(&format!("1+r{hex_name}={hex_value}"));
            } else {
                tracing::warn!("XTGETTCAP: unknown capability: {cap_name}");
                self.write_dcs_response(&format!("0+r{hex_name}"));
            }
        }
    }

    /// Decode a hex-encoded ASCII string (e.g., "524742" → "RGB").
    fn hex_decode(hex: &str) -> Option<String> {
        let bytes = hex.as_bytes();
        if !bytes.len().is_multiple_of(2) {
            return None;
        }
        let mut result = Vec::with_capacity(bytes.len() / 2);
        let mut i = 0;
        while i < bytes.len() {
            let hi = Self::hex_nibble(bytes[i])?;
            let lo = Self::hex_nibble(bytes[i + 1])?;
            result.push((hi << 4) | lo);
            i += 2;
        }
        String::from_utf8(result).ok()
    }

    /// Encode an ASCII string as hex (e.g., "1" → "31").
    fn hex_encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len() * 2);
        for b in s.bytes() {
            result.push(Self::nibble_to_hex(b >> 4));
            result.push(Self::nibble_to_hex(b & 0x0F));
        }
        result
    }

    /// Convert a single ASCII hex character to its numeric value.
    const fn hex_nibble(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }

    /// Convert a 4-bit nibble to an uppercase hex character.
    const fn nibble_to_hex(n: u8) -> char {
        match n {
            0..=9 => (b'0' + n) as char,
            _ => (b'A' + n - 10) as char,
        }
    }

    /// Look up a termcap/terminfo capability by decoded name.
    ///
    /// Returns `Some(value_str)` for known capabilities, `None` for unknown ones.
    /// The returned string is the raw value (not yet hex-encoded).
    fn lookup_termcap(name: &str) -> Option<&'static str> {
        match name {
            // RGB — terminal supports direct-color (24-bit) via SGR 38/48;2;R;G;B
            "RGB" => Some("8/8/8"),
            // Tc — tmux extension: true color support
            "Tc" => Some(""),
            // setrgbf — SGR sequence to set RGB foreground
            "setrgbf" => Some("\x1b[38;2;%p1%d;%p2%d;%p3%dm"),
            // setrgbb — SGR sequence to set RGB background
            "setrgbb" => Some("\x1b[48;2;%p1%d;%p2%d;%p3%dm"),
            // colors — number of colors supported
            "colors" | "Co" => Some("256"),
            // TN — terminal name
            "TN" => Some("xterm-256color"),
            // Ms — set selection (clipboard) via OSC 52
            "Ms" => Some("\x1b]52;%p1%s;%p2%s\x1b\\"),
            // Se — reset cursor to default style (DECSCUSR 0)
            "Se" => Some("\x1b[2 q"),
            // Ss — set cursor style (DECSCUSR)
            "Ss" => Some("\x1b[%p1%d q"),
            // Smulx — extended underline (SGR 4:N for curly, dotted, etc.)
            "Smulx" => Some("\x1b[4:%p1%dm"),
            // Setulc — set underline color
            "Setulc" => Some("\x1b[58;2;%p1%d;%p2%d;%p3%dm"),
            // khome — Home key
            "khome" => Some("\x1bOH"),
            // kend — End key
            "kend" => Some("\x1bOF"),
            // kHOM — Shift+Home
            "kHOM" => Some("\x1b[1;2H"),
            // kEND — Shift+End
            "kEND" => Some("\x1b[1;2F"),
            // smkx — enter keypad transmit (application) mode
            "smkx" => Some("\x1b[?1h\x1b="),
            // rmkx — exit keypad transmit mode (back to numeric)
            "rmkx" => Some("\x1b[?1l\x1b>"),
            _ => None,
        }
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
                self.current_working_directory = parse_osc7_uri(value);
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

    /// Handle OSC 10/11/12 foreground/background/cursor color query, set,
    /// and reset (OSC 110/111/112).
    ///
    /// Extracted from `handle_osc` to keep that function within the 100-line clippy limit.
    ///
    /// - `RequestColorQuery*(Query)`: respond with the effective color
    ///   (override or theme default).
    /// - `RequestColorQuery*(String(spec))`: parse the X11 color spec and
    ///   store as an override.
    /// - `ResetForegroundColor` / `ResetBackgroundColor` / `ResetCursorColor`:
    ///   clear the corresponding override so subsequent queries return the
    ///   theme color.
    fn handle_osc_fg_bg_color(&mut self, osc: &AnsiOscType) {
        match osc {
            // OSC 11 query: respond with the effective background color.
            AnsiOscType::RequestColorQueryBackground(AnsiOscInternalType::Query) => {
                let (r, g, b) = self.bg_color_override.unwrap_or(self.theme.background);
                self.write_osc_response(&format!("11;rgb:{r:02x}/{g:02x}/{b:02x}"));
            }
            // OSC 10 query: respond with the effective foreground color.
            AnsiOscType::RequestColorQueryForeground(AnsiOscInternalType::Query) => {
                let (r, g, b) = self.fg_color_override.unwrap_or(self.theme.foreground);
                self.write_osc_response(&format!("10;rgb:{r:02x}/{g:02x}/{b:02x}"));
            }
            // OSC 12 query: respond with the effective cursor color.
            AnsiOscType::RequestColorQueryCursor(AnsiOscInternalType::Query) => {
                let (r, g, b) = self.cursor_color_override.unwrap_or(self.theme.cursor);
                self.write_osc_response(&format!("12;rgb:{r:02x}/{g:02x}/{b:02x}"));
            }
            // OSC 11 set: store a dynamic background color override.
            AnsiOscType::RequestColorQueryBackground(AnsiOscInternalType::String(spec)) => {
                if let Some(rgb) = parse_color_spec(spec) {
                    self.bg_color_override = Some(rgb);
                } else {
                    tracing::warn!("OSC 11: unrecognised color spec: {spec:?}");
                }
            }
            // OSC 10 set: store a dynamic foreground color override.
            AnsiOscType::RequestColorQueryForeground(AnsiOscInternalType::String(spec)) => {
                if let Some(rgb) = parse_color_spec(spec) {
                    self.fg_color_override = Some(rgb);
                } else {
                    tracing::warn!("OSC 10: unrecognised color spec: {spec:?}");
                }
            }
            // OSC 12 set: store a dynamic cursor color override.
            AnsiOscType::RequestColorQueryCursor(AnsiOscInternalType::String(spec)) => {
                if let Some(rgb) = parse_color_spec(spec) {
                    self.cursor_color_override = Some(rgb);
                } else {
                    tracing::warn!("OSC 12: unrecognised color spec: {spec:?}");
                }
            }
            // OSC 110: reset dynamic foreground color override.
            AnsiOscType::ResetForegroundColor => {
                self.fg_color_override = None;
            }
            // OSC 111: reset dynamic background color override.
            AnsiOscType::ResetBackgroundColor => {
                self.bg_color_override = None;
            }
            // OSC 112: reset dynamic cursor color override.
            AnsiOscType::ResetCursorColor => {
                self.cursor_color_override = None;
            }
            // Unknown internal-type variants and unreachable arms — silently ignore.
            _ => {}
        }
    }

    /// Handle an iTerm2 `OSC 1337 ; File=` inline image.
    ///
    /// Decodes the raw image bytes into RGBA pixels, computes the display size
    /// in terminal cells from the dimension specs, and places the image into
    /// the buffer at the cursor position.
    fn handle_iterm2_inline_image(&mut self, data: &ITerm2InlineImageData) {
        if !data.inline {
            tracing::debug!("OSC 1337 File=: inline=0 (download), ignoring");
            return;
        }

        // Decode the raw image bytes into RGBA pixels.
        let img = match image::load_from_memory(&data.data) {
            Ok(img) => img.to_rgba8(),
            Err(e) => {
                tracing::warn!("OSC 1337 File=: image decode failed: {e}");
                return;
            }
        };

        let img_width_px = img.width();
        let img_height_px = img.height();

        if img_width_px == 0 || img_height_px == 0 {
            tracing::warn!("OSC 1337 File=: decoded image has zero dimensions");
            return;
        }

        let (term_width, term_height) = self.get_win_size();

        // Compute the display size in cells from the iTerm2 dimension specs.
        let display_cols = Self::resolve_image_dimension(
            data.width.as_ref(),
            img_width_px,
            term_width,
            self.cell_pixel_width,
        );
        let display_rows = Self::resolve_image_dimension(
            data.height.as_ref(),
            img_height_px,
            term_height,
            self.cell_pixel_height,
        );

        // Apply aspect-ratio preservation when only one dimension was
        // explicitly specified by the user.
        let (display_cols, display_rows) = if data.preserve_aspect_ratio {
            Self::apply_aspect_ratio(
                data.width.as_ref(),
                data.height.as_ref(),
                display_cols,
                display_rows,
                img_width_px,
                img_height_px,
                term_width,
                term_height,
                self.cell_pixel_width,
                self.cell_pixel_height,
            )
        } else {
            (display_cols, display_rows)
        };

        if display_cols == 0 || display_rows == 0 {
            tracing::warn!("OSC 1337 File=: computed display size is 0x0");
            return;
        }

        let pixels = img.into_raw();
        let inline_image = InlineImage {
            id: next_image_id(),
            pixels: std::sync::Arc::new(pixels),
            width_px: img_width_px,
            height_px: img_height_px,
            display_cols,
            display_rows,
        };

        // Save cursor position if doNotMoveCursor is set — iTerm2 protocol
        // specifies that the cursor should remain at its pre-image position.
        let saved_cursor = if data.do_not_move_cursor {
            Some(self.buffer.get_cursor().pos)
        } else {
            None
        };

        // Place the image into the buffer. Pass 0 for scroll_offset — the
        // PTY thread always operates at the live bottom.
        let _new_offset =
            self.buffer
                .place_image(inline_image, 0, ImageProtocol::ITerm2, None, None, 0);

        // Restore cursor position if doNotMoveCursor was requested.
        if let Some(pos) = saved_cursor {
            self.buffer.set_cursor_pos(Some(pos.x), Some(pos.y));
        }
    }

    /// Handle `OSC 1337 ; MultipartFile = [args]` — begin a multipart transfer.
    ///
    /// Stores the metadata and initialises an accumulator for incoming `FilePart`
    /// chunks.  If a previous multipart transfer was in progress, it is discarded
    /// with a warning.
    fn handle_iterm2_multipart_begin(&mut self, data: &ITerm2InlineImageData) {
        if self.multipart_state.is_some() {
            tracing::warn!(
                "OSC 1337 MultipartFile=: new transfer started while previous was in progress; \
                 discarding incomplete transfer"
            );
        }

        // Pre-allocate the accumulator to the declared size if available,
        // otherwise start with an empty vec.
        let capacity = data.size.unwrap_or(0);

        self.multipart_state = Some(MultipartImageState {
            metadata: data.clone(),
            accumulated_data: Vec::with_capacity(capacity),
        });

        tracing::debug!(
            "OSC 1337 MultipartFile=: started transfer (name={:?}, size={:?})",
            data.name,
            data.size,
        );
    }

    /// Handle `OSC 1337 ; FilePart = [base64]` — append a chunk to the active
    /// multipart transfer.
    fn handle_iterm2_file_part(&mut self, bytes: &[u8]) {
        let Some(state) = &mut self.multipart_state else {
            tracing::warn!("OSC 1337 FilePart=: no active multipart transfer; ignoring chunk");
            return;
        };

        state.accumulated_data.extend_from_slice(bytes);
        tracing::debug!(
            "OSC 1337 FilePart=: appended {} bytes (total so far: {})",
            bytes.len(),
            state.accumulated_data.len(),
        );
    }

    /// Handle `OSC 1337 ; FileEnd` — complete the active multipart transfer.
    ///
    /// Assembles the final `ITerm2InlineImageData` from the accumulated chunks
    /// and delegates to `handle_iterm2_inline_image` for decoding and placement.
    fn handle_iterm2_file_end(&mut self) {
        let Some(state) = self.multipart_state.take() else {
            tracing::warn!("OSC 1337 FileEnd: no active multipart transfer; ignoring");
            return;
        };

        if state.accumulated_data.is_empty() {
            tracing::warn!("OSC 1337 FileEnd: transfer completed with empty payload; ignoring");
            return;
        }

        tracing::debug!(
            "OSC 1337 FileEnd: transfer complete ({} bytes)",
            state.accumulated_data.len(),
        );

        // Assemble the final image data from metadata + accumulated bytes.
        let final_data = ITerm2InlineImageData {
            data: state.accumulated_data,
            ..state.metadata
        };

        self.handle_iterm2_inline_image(&final_data);
    }

    /// Resolve an iTerm2 image dimension spec to a cell count.
    ///
    /// `is_width` indicates whether we are computing columns (true) or rows (false).
    /// When the spec is `Auto` or `None`, the full image pixel size is used,
    /// divided by the terminal dimension to get a proportional cell count.
    fn resolve_image_dimension(
        spec: Option<&ImageDimension>,
        image_pixels: u32,
        term_cells: usize,
        cell_pixels: u32,
    ) -> usize {
        match spec {
            None | Some(ImageDimension::Auto) => {
                // image_pixels / cell_pixels, rounded up, clamped to term size.
                let cells = image_pixels.saturating_add(cell_pixels - 1) / cell_pixels;
                usize::value_from(cells).unwrap_or(0).min(term_cells).max(1)
            }
            Some(ImageDimension::Cells(n)) => {
                usize::value_from(*n).unwrap_or(0).min(term_cells).max(1)
            }
            Some(ImageDimension::Pixels(px)) => {
                let cells = px.saturating_add(cell_pixels - 1) / cell_pixels;
                usize::value_from(cells).unwrap_or(0).min(term_cells).max(1)
            }
            Some(ImageDimension::Percent(pct)) => {
                let cells = (u64::from(*pct) * u64::value_from(term_cells).unwrap_or(0)) / 100;
                // Safe: cells is bounded by term_cells which fits in usize.
                usize::value_from(cells).unwrap_or(0).min(term_cells).max(1)
            }
        }
    }

    /// Adjust display dimensions to preserve aspect ratio.
    ///
    /// When `preserve_aspect_ratio` is true and one dimension is auto/unspecified,
    /// scale the auto dimension to match the other using real cell pixel
    /// dimensions for correct terminal-cell aspect-ratio compensation.
    // All parameters represent independent geometric inputs. A struct would not improve clarity
    // for this pure geometric helper.
    #[allow(clippy::too_many_arguments)]
    fn apply_aspect_ratio(
        width_spec: Option<&ImageDimension>,
        height_spec: Option<&ImageDimension>,
        display_cols: usize,
        display_rows: usize,
        img_width_px: u32,
        img_height_px: u32,
        term_width: usize,
        term_height: usize,
        cell_pixel_width: u32,
        cell_pixel_height: u32,
    ) -> (usize, usize) {
        let width_is_auto = matches!(width_spec, None | Some(ImageDimension::Auto));
        let height_is_auto = matches!(height_spec, None | Some(ImageDimension::Auto));

        // Use actual cell pixel dimensions for aspect-ratio compensation.
        let cpw = u64::from(cell_pixel_width.max(1));
        let cph = u64::from(cell_pixel_height.max(1));

        if width_is_auto && !height_is_auto {
            // Height was explicitly set; scale width to preserve aspect ratio.
            // scaled_cols = display_rows * (img_w / img_h) * (cell_h / cell_w)
            let scaled =
                (u64::from(img_width_px) * u64::value_from(display_rows).unwrap_or(0) * cph)
                    / (u64::from(img_height_px).max(1) * cpw);
            // Safe: scaled is bounded by term_width which fits in usize.
            let cols = usize::value_from(scaled)
                .unwrap_or(0)
                .min(term_width)
                .max(1);
            (cols, display_rows)
        } else if !width_is_auto && height_is_auto {
            // Width was explicitly set; scale height to preserve aspect ratio.
            // scaled_rows = display_cols * (img_h / img_w) * (cell_w / cell_h)
            let scaled =
                (u64::from(img_height_px) * u64::value_from(display_cols).unwrap_or(0) * cpw)
                    / (u64::from(img_width_px).max(1) * cph);
            // Safe: scaled is bounded by term_height which fits in usize.
            let rows = usize::value_from(scaled)
                .unwrap_or(0)
                .min(term_height)
                .max(1);
            (display_cols, rows)
        } else {
            // Both auto or both explicit — no adjustment needed.
            (display_cols, display_rows)
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
        if cell_pixel_width > 0 {
            self.cell_pixel_width = cell_pixel_width;
        }
        if cell_pixel_height > 0 {
            self.cell_pixel_height = cell_pixel_height;
        }
        // scroll_offset is owned by ViewState on the GUI side; the PTY thread
        // always passes 0 when resizing.
        let _new_offset = self.buffer.set_size(width, height, 0);
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
        let (visible_chars, visible_tags) = self.buffer.visible_as_tchars_and_tags(scroll_offset);
        let (scrollback_chars, scrollback_tags) =
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
                tracing::debug!("Bell (ignored)");
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

                // ── ModifyOtherKeys via DEC mode (?2048) ──────────────
                Mode::ModifyOtherKeysMode(ModifyOtherKeysMode::Set) => {
                    // DECSET ?2048 → enable modifyOtherKeys level 1
                    self.modify_other_keys_level = 1;
                }
                Mode::ModifyOtherKeysMode(ModifyOtherKeysMode::Reset) => {
                    // DECRST ?2048 → disable modifyOtherKeys (level 0)
                    self.modify_other_keys_level = 0;
                }
                Mode::ModifyOtherKeysMode(ModifyOtherKeysMode::Query) => {
                    let mode = if self.modify_other_keys_level > 0 {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&ModifyOtherKeysMode::Set.report(Some(mode)));
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
                tracing::warn!("DoubleLineHeightTop not yet implemented (ignored)");
            }
            TerminalOutput::DoubleLineHeightBottom => {
                tracing::warn!("DoubleLineHeightBottom not yet implemented (ignored)");
            }
            TerminalOutput::SingleWidthLine => {
                tracing::warn!("SingleWidthLine not yet implemented (ignored)");
            }
            TerminalOutput::DoubleWidthLine => {
                tracing::warn!("DoubleWidthLine not yet implemented (ignored)");
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

/// Parse an OSC 7 URI of the form `file://hostname/path` and return the path
/// component.
///
/// The hostname is intentionally ignored — it is only meaningful for network
/// file-systems and most shells send `localhost` or the local hostname.
///
/// Percent-encoded bytes (e.g. `%20` for space) are decoded so the returned
/// path is a normal filesystem path string.
///
/// Returns `None` when the URI does not start with `file://` or has no path.
fn parse_osc7_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;

    // The path starts at the first '/' after the hostname.
    // `file:///path` (empty hostname) → rest = "/path"
    // `file://hostname/path`          → rest = "hostname/path"
    let path = if rest.starts_with('/') {
        rest
    } else {
        let slash_pos = rest.find('/')?;
        &rest[slash_pos..]
    };

    if path.is_empty() {
        return None;
    }

    Some(percent_decode(path))
}

/// Decode percent-encoded bytes (`%XX`) in a string.
///
/// Only valid two-hex-digit sequences are decoded; malformed sequences are
/// passed through verbatim.
fn percent_decode(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            output.push(char::from(hi << 4 | lo));
            i += 3;
            continue;
        }
        output.push(char::from(bytes[i]));
        i += 1;
    }
    output
}

/// Convert an ASCII hex digit to its numeric value.
const fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
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
    // OSC 7 CWD tracking tests
    // ------------------------------------------------------------------

    #[test]
    fn parse_osc7_uri_with_hostname() {
        let result = super::parse_osc7_uri("file://myhost/home/user/projects");
        assert_eq!(result, Some("/home/user/projects".to_string()));
    }

    #[test]
    fn parse_osc7_uri_empty_hostname() {
        // file:///path — empty hostname (common on macOS)
        let result = super::parse_osc7_uri("file:///home/user/projects");
        assert_eq!(result, Some("/home/user/projects".to_string()));
    }

    #[test]
    fn parse_osc7_uri_localhost() {
        let result = super::parse_osc7_uri("file://localhost/tmp");
        assert_eq!(result, Some("/tmp".to_string()));
    }

    #[test]
    fn parse_osc7_uri_percent_encoded_space() {
        let result = super::parse_osc7_uri("file:///home/user/my%20project");
        assert_eq!(result, Some("/home/user/my project".to_string()));
    }

    #[test]
    fn parse_osc7_uri_multiple_percent_encodings() {
        let result = super::parse_osc7_uri("file:///home/user/dir%20with%20spaces/sub%2Fdir");
        assert_eq!(
            result,
            Some("/home/user/dir with spaces/sub/dir".to_string())
        );
    }

    #[test]
    fn parse_osc7_uri_not_file_scheme() {
        assert_eq!(super::parse_osc7_uri("http://example.com/path"), None);
        assert_eq!(super::parse_osc7_uri("https://example.com/path"), None);
        assert_eq!(super::parse_osc7_uri("ftp://host/path"), None);
    }

    #[test]
    fn parse_osc7_uri_no_path_after_hostname() {
        // "file://hostname" with no trailing slash — no path
        assert_eq!(super::parse_osc7_uri("file://hostname"), None);
    }

    #[test]
    fn parse_osc7_uri_empty_string() {
        assert_eq!(super::parse_osc7_uri(""), None);
    }

    #[test]
    fn parse_osc7_uri_just_file_scheme() {
        assert_eq!(super::parse_osc7_uri("file://"), None);
    }

    #[test]
    fn percent_decode_no_encoding() {
        assert_eq!(super::percent_decode("/home/user"), "/home/user");
    }

    #[test]
    fn percent_decode_malformed_sequence() {
        // %ZZ is not valid hex — pass through verbatim
        assert_eq!(super::percent_decode("/path%ZZfoo"), "/path%ZZfoo");
    }

    #[test]
    fn percent_decode_truncated_at_end() {
        // % at end of string with not enough chars
        assert_eq!(super::percent_decode("/path%2"), "/path%2");
        assert_eq!(super::percent_decode("/path%"), "/path%");
    }

    #[test]
    fn handle_osc_remote_host_sets_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::RemoteHost(
            "file://localhost/home/user".to_string(),
        ));
        assert_eq!(handler.current_working_directory(), Some("/home/user"));
    }

    #[test]
    fn handle_osc_remote_host_updates_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::RemoteHost(
            "file://localhost/home/user/a".to_string(),
        ));
        assert_eq!(handler.current_working_directory(), Some("/home/user/a"));

        handler.handle_osc(&AnsiOscType::RemoteHost(
            "file://localhost/home/user/b".to_string(),
        ));
        assert_eq!(handler.current_working_directory(), Some("/home/user/b"));
    }

    #[test]
    fn handle_osc_remote_host_invalid_uri_clears_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        // First set a valid CWD
        handler.handle_osc(&AnsiOscType::RemoteHost(
            "file://localhost/home/user".to_string(),
        ));
        assert!(handler.current_working_directory().is_some());

        // Now send an invalid URI — CWD should be cleared (set to None)
        handler.handle_osc(&AnsiOscType::RemoteHost("not-a-file-uri".to_string()));
        assert_eq!(handler.current_working_directory(), None);
    }

    #[test]
    fn full_reset_clears_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::RemoteHost(
            "file://localhost/home/user".to_string(),
        ));
        assert!(handler.current_working_directory().is_some());

        handler.full_reset();
        assert_eq!(handler.current_working_directory(), None);
    }

    #[test]
    fn cwd_is_none_by_default() {
        let handler = TerminalHandler::new(80, 24);
        assert_eq!(handler.current_working_directory(), None);
    }

    // ── FTCS / OSC 133 tests ────────────────────────────────────────────

    #[test]
    fn ftcs_state_default_is_none() {
        let handler = TerminalHandler::new(80, 24);
        assert_eq!(handler.ftcs_state(), FtcsState::None);
        assert_eq!(handler.last_exit_code(), None);
    }

    #[test]
    fn ftcs_prompt_start_sets_in_prompt() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InPrompt);
    }

    #[test]
    fn ftcs_command_start_sets_in_command() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InCommand);
    }

    #[test]
    fn ftcs_output_start_sets_in_output() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InOutput);
    }

    #[test]
    fn ftcs_command_finished_resets_to_none() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InOutput);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(0))));
        assert_eq!(handler.ftcs_state(), FtcsState::None);
    }

    #[test]
    fn ftcs_command_finished_captures_exit_code() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(42))));
        assert_eq!(handler.last_exit_code(), Some(42));
    }

    #[test]
    fn ftcs_command_finished_no_exit_code() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(None)));
        assert_eq!(handler.last_exit_code(), None);
    }

    #[test]
    fn ftcs_full_cycle() {
        let mut handler = TerminalHandler::new(80, 24);

        // A → prompt start
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InPrompt);

        // B → command start
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InCommand);

        // C → output start
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InOutput);

        // D;0 → command finished with exit code 0
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(0))));
        assert_eq!(handler.ftcs_state(), FtcsState::None);
        assert_eq!(handler.last_exit_code(), Some(0));
    }

    #[test]
    fn ftcs_exit_code_updated_on_each_d_marker() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(0))));
        assert_eq!(handler.last_exit_code(), Some(0));

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(127))));
        assert_eq!(handler.last_exit_code(), Some(127));

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(None)));
        assert_eq!(handler.last_exit_code(), None);
    }

    #[test]
    fn full_reset_clears_ftcs_state() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(1))));
        assert_eq!(handler.last_exit_code(), Some(1));

        handler.full_reset();
        assert_eq!(handler.ftcs_state(), FtcsState::None);
        assert_eq!(handler.last_exit_code(), None);
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

    // ------------------------------------------------------------------
    // DECRQSS tests (DCS $ q ... ST)
    // ------------------------------------------------------------------

    /// Helper: build a raw DCS payload as the standard parser would produce.
    /// Format: `P` + content + `ESC \`
    fn build_dcs_payload(content: &[u8]) -> Vec<u8> {
        let mut v = vec![b'P'];
        v.extend_from_slice(content);
        v.extend_from_slice(b"\x1b\\");
        v
    }

    /// Helper: receive the PTY write-back response from a DECRQSS query.
    fn recv_pty_response(rx: &crossbeam_channel::Receiver<PtyWrite>) -> String {
        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response from DCS query");
        };
        let Ok(s) = String::from_utf8(bytes) else {
            panic!("DCS response should be valid UTF-8");
        };
        s
    }

    #[test]
    fn decrqss_sgr_default_attributes() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let dcs = build_dcs_payload(b"$qm");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Default state: just "0" (reset)
        assert_eq!(response, "\x1bP1$r0m\x1b\\");
    }

    #[test]
    fn decrqss_sgr_bold_and_italic() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Apply bold + italic
        handler.process_output(&TerminalOutput::Sgr(SelectGraphicRendition::Bold));
        handler.process_output(&TerminalOutput::Sgr(SelectGraphicRendition::Italic));

        let dcs = build_dcs_payload(b"$qm");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r0;1;3m\x1b\\");
    }

    #[test]
    fn decrqss_sgr_with_fg_color() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.process_output(&TerminalOutput::Sgr(SelectGraphicRendition::Foreground(
            TerminalColor::Red,
        )));

        let dcs = build_dcs_payload(b"$qm");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r0;31m\x1b\\");
    }

    #[test]
    fn decrqss_sgr_with_truecolor() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.process_output(&TerminalOutput::Sgr(SelectGraphicRendition::Foreground(
            TerminalColor::Custom(255, 128, 0),
        )));

        let dcs = build_dcs_payload(b"$qm");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r0;38;2;255;128;0m\x1b\\");
    }

    #[test]
    fn decrqss_sgr_reverse_video() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.process_output(&TerminalOutput::Sgr(SelectGraphicRendition::ReverseVideo));

        let dcs = build_dcs_payload(b"$qm");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r0;7m\x1b\\");
    }

    #[test]
    fn decrqss_decstbm_default_scroll_region() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let dcs = build_dcs_payload(b"$qr");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Default scroll region: full screen [0, 23] → 1-based [1, 24]
        assert_eq!(response, "\x1bP1$r1;24r\x1b\\");
    }

    #[test]
    fn decrqss_decstbm_custom_scroll_region() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set scroll region to 1-based rows 5-20
        handler.handle_set_scroll_region(5, 20);

        let dcs = build_dcs_payload(b"$qr");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r5;20r\x1b\\");
    }

    #[test]
    fn decrqss_decscusr_default_cursor_style() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Note: space + q = DECSCUSR query
        let dcs = build_dcs_payload(b"$q q");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Default is BlockCursorSteady = 2
        assert_eq!(response, "\x1bP1$r2 q\x1b\\");
    }

    #[test]
    fn decrqss_decscusr_after_style_change() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        handler.process_output(&TerminalOutput::CursorVisualStyle(
            CursorVisualStyle::UnderlineCursorBlink,
        ));

        let dcs = build_dcs_payload(b"$q q");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP1$r3 q\x1b\\");
    }

    #[test]
    fn decrqss_decscl_conformance_level() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // "p = DECSCL (Set Conformance Level) query
        let dcs = build_dcs_payload(b"$q\"p");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Freminal claims VT525 (level 5) with 7-bit C1 controls (Ps2=1)
        // Response format: DCS 1 $ r 65 ; 1 " p ST
        assert_eq!(response, "\x1bP1$r65;1\"p\x1b\\");
    }

    #[test]
    fn decrqss_invalid_query() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let dcs = build_dcs_payload(b"$qZ");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // Invalid query → DCS 0 $ r ST
        assert_eq!(response, "\x1bP0$r\x1b\\");
    }

    #[test]
    fn dcs_unknown_subcommand_does_not_panic() {
        let mut handler = TerminalHandler::new(80, 24);

        // No write_tx set — should not panic even on unknown DCS
        let dcs = build_dcs_payload(b"!zsome_data");
        handler.handle_device_control_string(&dcs);
        // Success = no panic
    }

    #[test]
    fn strip_dcs_envelope_handles_minimal_payload() {
        // Just "P" + ESC '\' — inner content is empty
        let dcs = b"P\x1b\\";
        let inner = TerminalHandler::strip_dcs_envelope(dcs);
        assert!(inner.is_empty());
    }

    #[test]
    fn strip_dcs_envelope_preserves_content() {
        let dcs = b"P$qm\x1b\\";
        let inner = TerminalHandler::strip_dcs_envelope(dcs);
        assert_eq!(inner, b"$qm");
    }

    // ── undouble_esc tests ────────────────────────────────────────────────

    #[test]
    fn undouble_esc_no_esc_bytes() {
        let data = b"hello world";
        let result = TerminalHandler::undouble_esc(data);
        assert_eq!(result, b"hello world");
    }

    #[test]
    fn undouble_esc_single_pair() {
        // ESC ESC → ESC
        let data = b"\x1b\x1b";
        let result = TerminalHandler::undouble_esc(data);
        assert_eq!(result, b"\x1b");
    }

    #[test]
    fn undouble_esc_multiple_pairs() {
        // Two doubled pairs with content between
        let data = b"\x1b\x1b_G\x1b\x1b\\";
        let result = TerminalHandler::undouble_esc(data);
        assert_eq!(result, b"\x1b_G\x1b\\");
    }

    #[test]
    fn undouble_esc_lone_esc_at_end() {
        // A single ESC at the end (not doubled) stays as-is
        let data = b"abc\x1b";
        let result = TerminalHandler::undouble_esc(data);
        assert_eq!(result, b"abc\x1b");
    }

    #[test]
    fn undouble_esc_empty() {
        let result = TerminalHandler::undouble_esc(b"");
        assert!(result.is_empty());
    }

    #[test]
    fn undouble_esc_triple_esc() {
        // Three consecutive ESC bytes: first two form a pair → ESC, the third
        // remains as a lone ESC.
        let data = b"\x1b\x1b\x1b";
        let result = TerminalHandler::undouble_esc(data);
        assert_eq!(result, b"\x1b\x1b");
    }

    // ── double_esc tests ──────────────────────────────────────────────────

    #[test]
    fn double_esc_no_esc_bytes() {
        let data = b"hello world";
        let result = TerminalHandler::double_esc(data);
        assert_eq!(result, b"hello world");
    }

    #[test]
    fn double_esc_single_esc() {
        let data = b"\x1b";
        let result = TerminalHandler::double_esc(data);
        assert_eq!(result, b"\x1b\x1b");
    }

    #[test]
    fn double_esc_apc_sequence() {
        // ESC _ G i=1;OK ESC \  → ESC ESC _ G i=1;OK ESC ESC backslash
        let data = b"\x1b_Gi=1;OK\x1b\\";
        let result = TerminalHandler::double_esc(data);
        assert_eq!(result, b"\x1b\x1b_Gi=1;OK\x1b\x1b\\");
    }

    #[test]
    fn double_esc_empty() {
        let result = TerminalHandler::double_esc(b"");
        assert!(result.is_empty());
    }

    #[test]
    fn double_esc_roundtrip() {
        // undouble(double(x)) == x for any input
        let original = b"\x1b_Ga=q,i=1;\x1b\\";
        let doubled = TerminalHandler::double_esc(original);
        let undoubled = TerminalHandler::undouble_esc(&doubled);
        assert_eq!(undoubled, original.to_vec());
    }

    // ── wrap_tmux_passthrough tests ───────────────────────────────────────

    #[test]
    fn wrap_tmux_passthrough_kitty_response() {
        // A Kitty OK response should be wrapped correctly
        let response = b"\x1b_Gi=1;OK\x1b\\";
        let wrapped = TerminalHandler::wrap_tmux_passthrough(response);
        // Expected: ESC P tmux; ESC ESC _ G i=1;OK ESC ESC \ ESC \
        let expected = b"\x1bPtmux;\x1b\x1b_Gi=1;OK\x1b\x1b\\\x1b\\";
        assert_eq!(wrapped, expected.to_vec());
    }

    #[test]
    fn wrap_tmux_passthrough_plain_text() {
        // Plain text (no ESC) should pass through with just the envelope
        let data = b"hello";
        let wrapped = TerminalHandler::wrap_tmux_passthrough(data);
        assert_eq!(wrapped, b"\x1bPtmux;hello\x1b\\".to_vec());
    }

    // ── tmux passthrough dispatch tests ───────────────────────────────────

    #[test]
    fn tmux_passthrough_empty_payload_does_not_panic() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_tmux_passthrough(b"");
        // Success = no panic
    }

    #[test]
    fn tmux_passthrough_no_esc_prefix_does_not_panic() {
        let mut handler = TerminalHandler::new(80, 24);
        // Payload that does not start with doubled ESC
        handler.handle_tmux_passthrough(b"junk data");
        // Success = no panic
    }

    #[test]
    fn tmux_passthrough_too_short_does_not_panic() {
        let mut handler = TerminalHandler::new(80, 24);
        // Payload is just a doubled ESC with no type byte
        handler.handle_tmux_passthrough(b"\x1b\x1b");
        // After un-doubling: [0x1b] — length < 2 → warn and return
    }

    #[test]
    fn tmux_passthrough_dispatches_apc_kitty_query() {
        // Build a tmux-wrapped Kitty graphics query:
        //   Inner (un-doubled): ESC _ G a=q,i=1; ESC \
        //   Doubled for tmux:   ESC ESC _ G a=q,i=1; ESC ESC \
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // The tmux payload (after "tmux;" prefix has been stripped):
        // doubled-ESC _ G a=q,i=1; doubled-ESC backslash
        let mut payload = Vec::new();
        payload.extend_from_slice(b"\x1b\x1b_Ga=q,i=1;\x1b\x1b\\");

        handler.handle_tmux_passthrough(&payload);

        // The Kitty query handler should respond with a tmux-wrapped APC response
        let response = rx.try_recv();
        assert!(
            response.is_ok(),
            "Expected a Kitty graphics query response via PTY write"
        );
        let PtyWrite::Write(bytes) = response.unwrap() else {
            panic!("expected PtyWrite::Write");
        };
        let resp_str = String::from_utf8_lossy(&bytes);
        // Response should be wrapped in DCS tmux passthrough
        assert!(
            resp_str.starts_with("\x1bPtmux;"),
            "Expected tmux-wrapped response, got: {resp_str}"
        );
        // The inner content (after un-doubling) should be a Kitty APC response
        let inner = resp_str
            .strip_prefix("\x1bPtmux;")
            .and_then(|s| s.strip_suffix("\x1b\\"))
            .expect("Expected DCS tmux envelope");
        let inner_bytes = TerminalHandler::undouble_esc(inner.as_bytes());
        let inner_str = String::from_utf8_lossy(&inner_bytes);
        assert!(
            inner_str.starts_with("\x1b_G"),
            "Expected inner Kitty APC response, got: {inner_str}"
        );

        // The passthrough flag should be cleared after dispatch
        assert!(
            !handler.in_tmux_passthrough,
            "in_tmux_passthrough should be false after dispatch"
        );
    }

    #[test]
    fn tmux_passthrough_dispatches_nested_dcs() {
        // Build a tmux-wrapped DCS DECRQSS query for SGR:
        //   Inner (un-doubled): ESC P $ q m ESC \
        //   Doubled for tmux:   ESC ESC P $ q m ESC ESC \
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let mut payload = Vec::new();
        payload.extend_from_slice(b"\x1b\x1bP$qm\x1b\x1b\\");

        handler.handle_tmux_passthrough(&payload);

        // The DECRQSS handler should respond with a tmux-wrapped DCS response
        let response = rx.try_recv();
        assert!(
            response.is_ok(),
            "Expected a DECRQSS response via PTY write"
        );
        let PtyWrite::Write(bytes) = response.unwrap() else {
            panic!("expected PtyWrite::Write");
        };
        let resp_str = String::from_utf8_lossy(&bytes);
        // Response should be wrapped in DCS tmux passthrough
        assert!(
            resp_str.starts_with("\x1bPtmux;"),
            "Expected tmux-wrapped response, got: {resp_str}"
        );
        // The inner content should be a DECRQSS response
        let inner = resp_str
            .strip_prefix("\x1bPtmux;")
            .and_then(|s| s.strip_suffix("\x1b\\"))
            .expect("Expected DCS tmux envelope");
        let inner_bytes = TerminalHandler::undouble_esc(inner.as_bytes());
        let inner_str = String::from_utf8_lossy(&inner_bytes);
        assert!(
            inner_str.contains("$r"),
            "Expected DECRQSS response, got: {inner_str}"
        );
    }

    #[test]
    fn tmux_passthrough_unknown_type_does_not_panic() {
        let mut handler = TerminalHandler::new(80, 24);
        // Inner: ESC Z (unknown type)
        let payload = b"\x1b\x1bZ";
        handler.handle_tmux_passthrough(payload);
        // Success = no panic; flag should be cleared
        assert!(!handler.in_tmux_passthrough);
    }

    #[test]
    fn tmux_passthrough_via_full_dcs_handler() {
        // End-to-end: feed a complete DCS tmux passthrough through
        // handle_device_control_string (the normal entry point).
        //
        // Format: P tmux; <doubled-payload> ESC \
        // Payload: Kitty graphics query: ESC ESC _ G a=q,i=1; ESC ESC \
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let mut dcs = vec![b'P'];
        dcs.extend_from_slice(b"tmux;");
        dcs.extend_from_slice(b"\x1b\x1b_Ga=q,i=1;\x1b\x1b\\");
        dcs.extend_from_slice(b"\x1b\\");

        handler.handle_device_control_string(&dcs);

        // Should have dispatched to the Kitty query handler with tmux wrapping
        let response = rx.try_recv();
        assert!(
            response.is_ok(),
            "Expected a Kitty graphics query response from full DCS tmux passthrough"
        );
        let PtyWrite::Write(bytes) = response.unwrap() else {
            panic!("expected PtyWrite::Write");
        };
        let resp_str = String::from_utf8_lossy(&bytes);
        // Response should be wrapped in DCS tmux passthrough
        assert!(
            resp_str.starts_with("\x1bPtmux;"),
            "Expected tmux-wrapped response, got: {resp_str}"
        );
    }

    #[test]
    fn tmux_passthrough_flag_cleared_after_early_return() {
        // Even when the payload is invalid and we return early,
        // the flag should not be left set.
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_tmux_passthrough(b"");
        assert!(!handler.in_tmux_passthrough);
        handler.handle_tmux_passthrough(b"junk");
        assert!(!handler.in_tmux_passthrough);
    }

    // ── XTGETTCAP tests ──────────────────────────────────────────────────

    #[test]
    fn xtgettcap_hex_decode_rgb() {
        // "RGB" = 0x52 0x47 0x42 → "524742"
        let decoded = TerminalHandler::hex_decode("524742");
        assert_eq!(decoded.as_deref(), Some("RGB"));
    }

    #[test]
    fn xtgettcap_hex_decode_lowercase() {
        // "Ms" = 0x4D 0x73 → uppercase hex "4D73", lowercase "4d73"
        // 'd' is a hex letter that differs between cases — a good test for
        // case-insensitive parsing.
        let decoded_upper = TerminalHandler::hex_decode("4D73");
        assert_eq!(decoded_upper.as_deref(), Some("Ms"));

        let decoded_lower = TerminalHandler::hex_decode("4d73");
        assert_eq!(decoded_lower.as_deref(), Some("Ms"));
    }

    #[test]
    fn xtgettcap_hex_decode_odd_length_fails() {
        // Odd-length hex string is invalid
        assert!(TerminalHandler::hex_decode("52474").is_none());
    }

    #[test]
    fn xtgettcap_hex_encode_roundtrip() {
        let original = "RGB";
        let encoded = TerminalHandler::hex_encode(original);
        assert_eq!(encoded, "524742");
        let decoded = TerminalHandler::hex_decode(&encoded);
        assert_eq!(decoded.as_deref(), Some(original));
    }

    #[test]
    fn xtgettcap_known_capability_rgb() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "RGB" → hex "524742"
        let dcs = build_dcs_payload(b"+q524742");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // "8/8/8" → hex "382F382F38"
        assert_eq!(response, "\x1bP1+r524742=382F382F38\x1b\\");
    }

    #[test]
    fn xtgettcap_known_capability_colors() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "colors" → hex "636F6C6F7273"
        let dcs = build_dcs_payload(b"+q636F6C6F7273");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // "256" → hex "323536"
        assert_eq!(response, "\x1bP1+r636F6C6F7273=323536\x1b\\");
    }

    #[test]
    fn xtgettcap_known_capability_tn() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "TN" → hex "544E"
        let dcs = build_dcs_payload(b"+q544E");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // "xterm-256color" → hex
        let expected_hex = TerminalHandler::hex_encode("xterm-256color");
        assert_eq!(response, format!("\x1bP1+r544E={expected_hex}\x1b\\"));
    }

    #[test]
    fn xtgettcap_unknown_capability() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "UNKN" → hex "554E4B4E"
        let dcs = build_dcs_payload(b"+q554E4B4E");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        assert_eq!(response, "\x1bP0+r554E4B4E\x1b\\");
    }

    #[test]
    fn xtgettcap_multiple_capabilities() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "RGB" and "TN" separated by ';'
        // "RGB" = 524742, "TN" = 544E
        let dcs = build_dcs_payload(b"+q524742;544E");
        handler.handle_device_control_string(&dcs);

        // Should get two separate responses
        let response1 = recv_pty_response(&rx);
        assert_eq!(response1, "\x1bP1+r524742=382F382F38\x1b\\");

        let response2 = recv_pty_response(&rx);
        let tn_hex = TerminalHandler::hex_encode("xterm-256color");
        assert_eq!(response2, format!("\x1bP1+r544E={tn_hex}\x1b\\"));
    }

    #[test]
    fn xtgettcap_known_capability_tc() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "Tc" → hex "5463"
        let dcs = build_dcs_payload(b"+q5463");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // "Tc" has empty value, so hex-encoded value is ""
        assert_eq!(response, "\x1bP1+r5463=\x1b\\");
    }

    #[test]
    fn xtgettcap_known_capability_se() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "Se" → hex "5365"
        let dcs = build_dcs_payload(b"+q5365");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // "\x1b[2 q" → hex "1B5B322071"
        let expected_hex = TerminalHandler::hex_encode("\x1b[2 q");
        assert_eq!(response, format!("\x1bP1+r5365={expected_hex}\x1b\\"));
    }

    #[test]
    fn xtgettcap_known_capability_setrgbf() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Query "setrgbf" → hex encode each byte
        let hex_name = TerminalHandler::hex_encode("setrgbf");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[38;2;%p1%d;%p2%d;%p3%dm");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_setrgbb() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let hex_name = TerminalHandler::hex_encode("setrgbb");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[48;2;%p1%d;%p2%d;%p3%dm");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_co_alias() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // "Co" is an alias for "colors"; both should return "256"
        // "Co" = 0x43 0x6F → hex "436F"
        let dcs = build_dcs_payload(b"+q436F");
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // "256" → hex "323536"
        assert_eq!(response, "\x1bP1+r436F=323536\x1b\\");
    }

    #[test]
    fn xtgettcap_known_capability_ms() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let hex_name = TerminalHandler::hex_encode("Ms");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b]52;%p1%s;%p2%s\x1b\\");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_ss() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let hex_name = TerminalHandler::hex_encode("Ss");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[%p1%d q");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_smulx() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let hex_name = TerminalHandler::hex_encode("Smulx");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[4:%p1%dm");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_setulc() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let hex_name = TerminalHandler::hex_encode("Setulc");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[58;2;%p1%d;%p2%d;%p3%dm");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_khome() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // "khome" → hex encode name, expect CSI H response (\x1b[H)
        let hex_name = TerminalHandler::hex_encode("khome");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // SS3 H = \x1bOH — the sequence Freminal sends for Home in DECCKM Application mode
        let expected_val_hex = TerminalHandler::hex_encode("\x1bOH");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_kend() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // "kend" → hex encode name, expect SS3 F response (\x1bOF)
        let hex_name = TerminalHandler::hex_encode("kend");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        // SS3 F = \x1bOF — the sequence Freminal sends for End in DECCKM Application mode
        let expected_val_hex = TerminalHandler::hex_encode("\x1bOF");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_khom_shift() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // "kHOM" (Shift+Home) → expect \x1b[1;2H
        let hex_name = TerminalHandler::hex_encode("kHOM");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[1;2H");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    #[test]
    fn xtgettcap_known_capability_kend_shift() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // "kEND" (Shift+End) → expect \x1b[1;2F
        let hex_name = TerminalHandler::hex_encode("kEND");
        let mut payload = Vec::new();
        payload.extend_from_slice(b"+q");
        payload.extend_from_slice(hex_name.as_bytes());
        let dcs = build_dcs_payload(&payload);
        handler.handle_device_control_string(&dcs);

        let response = recv_pty_response(&rx);
        let expected_val_hex = TerminalHandler::hex_encode("\x1b[1;2F");
        assert_eq!(
            response,
            format!("\x1bP1+r{hex_name}={expected_val_hex}\x1b\\")
        );
    }

    // ------------------------------------------------------------------
    // OSC 10/11/110/111 foreground/background color tests
    // ------------------------------------------------------------------

    #[test]
    fn osc11_query_returns_theme_background_by_default() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // OSC 11 query — no override set, should return CATPPUCCIN_MOCHA background.
        handler.handle_osc(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response for OSC 11 query");
        };
        let Ok(response) = String::from_utf8(bytes) else {
            panic!("OSC response should be valid UTF-8");
        };
        // CATPPUCCIN_MOCHA background = (0x1e, 0x1e, 0x2e)
        assert_eq!(response, "\x1b]11;rgb:1e/1e/2e\x1b\\");
    }

    #[test]
    fn osc10_query_returns_theme_foreground_by_default() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // OSC 10 query — no override set, should return CATPPUCCIN_MOCHA foreground.
        handler.handle_osc(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response for OSC 10 query");
        };
        let Ok(response) = String::from_utf8(bytes) else {
            panic!("OSC response should be valid UTF-8");
        };
        // CATPPUCCIN_MOCHA foreground = (0xcd, 0xd6, 0xf4)
        assert_eq!(response, "\x1b]10;rgb:cd/d6/f4\x1b\\");
    }

    #[test]
    fn osc11_set_stores_override_and_query_returns_it() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set background override to #ff0080.
        handler.handle_osc(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::String("#ff0080".to_string()),
        ));

        // Query — should return the override, not the theme default.
        handler.handle_osc(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response after OSC 11 set + query");
        };
        let Ok(response) = String::from_utf8(bytes) else {
            panic!("OSC response should be valid UTF-8");
        };
        assert_eq!(response, "\x1b]11;rgb:ff/00/80\x1b\\");
    }

    #[test]
    fn osc10_set_stores_override_and_query_returns_it() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set foreground override via rgb: format.
        handler.handle_osc(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::String("rgb:aa/bb/cc".to_string()),
        ));

        // Query — should return the override.
        handler.handle_osc(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response after OSC 10 set + query");
        };
        let Ok(response) = String::from_utf8(bytes) else {
            panic!("OSC response should be valid UTF-8");
        };
        assert_eq!(response, "\x1b]10;rgb:aa/bb/cc\x1b\\");
    }

    #[test]
    fn osc111_resets_bg_override_and_query_returns_theme() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set override first.
        handler.handle_osc(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::String("#112233".to_string()),
        ));

        // Reset via OSC 111.
        handler.handle_osc(&AnsiOscType::ResetBackgroundColor);

        // Query — should return theme background again.
        handler.handle_osc(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response after OSC 111 reset + query");
        };
        let Ok(response) = String::from_utf8(bytes) else {
            panic!("OSC response should be valid UTF-8");
        };
        // CATPPUCCIN_MOCHA background = (0x1e, 0x1e, 0x2e)
        assert_eq!(response, "\x1b]11;rgb:1e/1e/2e\x1b\\");
    }

    #[test]
    fn osc110_resets_fg_override_and_query_returns_theme() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set override first.
        handler.handle_osc(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::String("rgb:ff/00/00".to_string()),
        ));

        // Reset via OSC 110.
        handler.handle_osc(&AnsiOscType::ResetForegroundColor);

        // Query — should return theme foreground again.
        handler.handle_osc(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response after OSC 110 reset + query");
        };
        let Ok(response) = String::from_utf8(bytes) else {
            panic!("OSC response should be valid UTF-8");
        };
        // CATPPUCCIN_MOCHA foreground = (0xcd, 0xd6, 0xf4)
        assert_eq!(response, "\x1b]10;rgb:cd/d6/f4\x1b\\");
    }

    #[test]
    fn full_reset_clears_fg_bg_color_overrides() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set both overrides.
        handler.handle_osc(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::String("#ff0000".to_string()),
        ));
        handler.handle_osc(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::String("#0000ff".to_string()),
        ));

        // full_reset should clear both.
        handler.full_reset();

        // Foreground query should return theme default.
        handler.handle_osc(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::Query,
        ));
        let Ok(PtyWrite::Write(fg_bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write for fg query after full_reset");
        };
        let Ok(fg_response) = String::from_utf8(fg_bytes) else {
            panic!("fg OSC response should be valid UTF-8");
        };
        assert_eq!(fg_response, "\x1b]10;rgb:cd/d6/f4\x1b\\");

        // Background query should return theme default.
        handler.handle_osc(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::Query,
        ));
        let Ok(PtyWrite::Write(bg_bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write for bg query after full_reset");
        };
        let Ok(bg_response) = String::from_utf8(bg_bytes) else {
            panic!("bg OSC response should be valid UTF-8");
        };
        assert_eq!(bg_response, "\x1b]11;rgb:1e/1e/2e\x1b\\");
    }

    #[test]
    fn osc12_query_returns_theme_cursor_by_default() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // OSC 12 query — no override set, should return CATPPUCCIN_MOCHA cursor.
        handler.handle_osc(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response for OSC 12 query");
        };
        let Ok(response) = String::from_utf8(bytes) else {
            panic!("OSC response should be valid UTF-8");
        };
        // CATPPUCCIN_MOCHA cursor = (0xf5, 0xe0, 0xdc) — Rosewater
        assert_eq!(response, "\x1b]12;rgb:f5/e0/dc\x1b\\");
    }

    #[test]
    fn osc12_set_stores_override_and_query_returns_it() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set cursor color override to #ff8000.
        handler.handle_osc(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::String("#ff8000".to_string()),
        ));

        // Query — should return the override, not the theme default.
        handler.handle_osc(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response after OSC 12 set + query");
        };
        let Ok(response) = String::from_utf8(bytes) else {
            panic!("OSC response should be valid UTF-8");
        };
        assert_eq!(response, "\x1b]12;rgb:ff/80/00\x1b\\");
    }

    #[test]
    fn osc112_resets_cursor_override_and_query_returns_theme() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set override first.
        handler.handle_osc(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::String("rgb:aa/bb/cc".to_string()),
        ));

        // Reset via OSC 112.
        handler.handle_osc(&AnsiOscType::ResetCursorColor);

        // Query — should return theme cursor again.
        handler.handle_osc(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::Query,
        ));

        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response after OSC 112 reset + query");
        };
        let Ok(response) = String::from_utf8(bytes) else {
            panic!("OSC response should be valid UTF-8");
        };
        // CATPPUCCIN_MOCHA cursor = (0xf5, 0xe0, 0xdc)
        assert_eq!(response, "\x1b]12;rgb:f5/e0/dc\x1b\\");
    }

    #[test]
    fn full_reset_clears_cursor_color_override() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set cursor color override.
        handler.handle_osc(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::String("#112233".to_string()),
        ));
        assert!(handler.cursor_color_override().is_some());

        // full_reset should clear the override.
        handler.full_reset();
        assert!(handler.cursor_color_override().is_none());

        // Query should return theme default.
        handler.handle_osc(&AnsiOscType::RequestColorQueryCursor(
            AnsiOscInternalType::Query,
        ));
        let Ok(PtyWrite::Write(bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write for cursor query after full_reset");
        };
        let Ok(response) = String::from_utf8(bytes) else {
            panic!("cursor OSC response should be valid UTF-8");
        };
        assert_eq!(response, "\x1b]12;rgb:f5/e0/dc\x1b\\");
    }

    #[test]
    fn theme_switch_changes_osc10_osc11_responses() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Switch from default CATPPUCCIN_MOCHA to DRACULA.
        handler.set_theme(&freminal_common::themes::DRACULA);

        // OSC 10 query — should return DRACULA foreground (0xf8, 0xf8, 0xf2).
        handler.handle_osc(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::Query,
        ));
        let Ok(PtyWrite::Write(fg_bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response for OSC 10 query after theme switch");
        };
        let Ok(fg_response) = String::from_utf8(fg_bytes) else {
            panic!("OSC 10 response should be valid UTF-8");
        };
        assert_eq!(fg_response, "\x1b]10;rgb:f8/f8/f2\x1b\\");

        // OSC 11 query — should return DRACULA background (0x28, 0x2a, 0x36).
        handler.handle_osc(&AnsiOscType::RequestColorQueryBackground(
            AnsiOscInternalType::Query,
        ));
        let Ok(PtyWrite::Write(bg_bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write response for OSC 11 query after theme switch");
        };
        let Ok(bg_response) = String::from_utf8(bg_bytes) else {
            panic!("OSC 11 response should be valid UTF-8");
        };
        assert_eq!(bg_response, "\x1b]11;rgb:28/2a/36\x1b\\");
    }

    #[test]
    fn osc10_override_persists_across_theme_switch() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Set foreground override to #ff0000.
        handler.handle_osc(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::String("#ff0000".to_string()),
        ));

        // Switch theme to DRACULA — override should survive.
        handler.set_theme(&freminal_common::themes::DRACULA);

        // OSC 10 query — override (#ff0000) takes precedence over the new theme.
        handler.handle_osc(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::Query,
        ));
        let Ok(PtyWrite::Write(override_bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write for OSC 10 query while override is active");
        };
        let Ok(override_response) = String::from_utf8(override_bytes) else {
            panic!("OSC 10 response should be valid UTF-8");
        };
        assert_eq!(override_response, "\x1b]10;rgb:ff/00/00\x1b\\");

        // Reset the override via OSC 110.
        handler.handle_osc(&AnsiOscType::ResetForegroundColor);

        // OSC 10 query — should now return DRACULA foreground (0xf8, 0xf8, 0xf2).
        handler.handle_osc(&AnsiOscType::RequestColorQueryForeground(
            AnsiOscInternalType::Query,
        ));
        let Ok(PtyWrite::Write(theme_bytes)) = rx.try_recv() else {
            panic!("expected PtyWrite::Write for OSC 10 query after OSC 110 reset");
        };
        let Ok(theme_response) = String::from_utf8(theme_bytes) else {
            panic!("OSC 10 response should be valid UTF-8");
        };
        assert_eq!(theme_response, "\x1b]10;rgb:f8/f8/f2\x1b\\");
    }

    // ------------------------------------------------------------------
    // resolve_image_dimension tests
    // ------------------------------------------------------------------

    #[test]
    fn resolve_auto_uses_image_pixels() {
        // 160px wide image, 8px per cell → 20 cells
        let result = TerminalHandler::resolve_image_dimension(None, 160, 80, 8);
        assert_eq!(result, 20);
    }

    #[test]
    fn resolve_auto_explicit() {
        let result =
            TerminalHandler::resolve_image_dimension(Some(&ImageDimension::Auto), 160, 80, 8);
        assert_eq!(result, 20);
    }

    #[test]
    fn resolve_auto_height() {
        // 320px tall image, 16px per cell → 20 rows
        let result = TerminalHandler::resolve_image_dimension(None, 320, 24, 16);
        assert_eq!(result, 20);
    }

    #[test]
    fn resolve_auto_clamps_to_term_size() {
        // 10000px wide, 8px/cell = 1250 cells, but term is only 80
        let result = TerminalHandler::resolve_image_dimension(None, 10000, 80, 8);
        assert_eq!(result, 80);
    }

    #[test]
    fn resolve_auto_minimum_is_1() {
        // 0px image → would be 0 cells, but minimum is 1
        let result = TerminalHandler::resolve_image_dimension(None, 0, 80, 8);
        assert_eq!(result, 1);
    }

    #[test]
    fn resolve_cells_direct() {
        let result =
            TerminalHandler::resolve_image_dimension(Some(&ImageDimension::Cells(10)), 999, 80, 8);
        assert_eq!(result, 10);
    }

    #[test]
    fn resolve_cells_clamped_to_term() {
        let result =
            TerminalHandler::resolve_image_dimension(Some(&ImageDimension::Cells(200)), 999, 80, 8);
        assert_eq!(result, 80);
    }

    #[test]
    fn resolve_pixels() {
        // 80px wide, 8px/cell → 10 cells
        let result =
            TerminalHandler::resolve_image_dimension(Some(&ImageDimension::Pixels(80)), 999, 80, 8);
        assert_eq!(result, 10);
    }

    #[test]
    fn resolve_pixels_rounds_up() {
        // 81px wide, 8px/cell → ceil(81/8) = 11 cells
        let result =
            TerminalHandler::resolve_image_dimension(Some(&ImageDimension::Pixels(81)), 999, 80, 8);
        assert_eq!(result, 11);
    }

    #[test]
    fn resolve_percent() {
        // 50% of 80 cols = 40
        let result = TerminalHandler::resolve_image_dimension(
            Some(&ImageDimension::Percent(50)),
            999,
            80,
            8,
        );
        assert_eq!(result, 40);
    }

    #[test]
    fn resolve_percent_100() {
        let result = TerminalHandler::resolve_image_dimension(
            Some(&ImageDimension::Percent(100)),
            999,
            80,
            8,
        );
        assert_eq!(result, 80);
    }

    // ------------------------------------------------------------------
    // apply_aspect_ratio tests
    // ------------------------------------------------------------------

    #[test]
    fn aspect_ratio_both_auto_no_adjustment() {
        let (cols, rows) =
            TerminalHandler::apply_aspect_ratio(None, None, 20, 10, 160, 160, 80, 24, 8, 16);
        // Both auto → no change
        assert_eq!(cols, 20);
        assert_eq!(rows, 10);
    }

    #[test]
    fn aspect_ratio_both_explicit_no_adjustment() {
        let (cols, rows) = TerminalHandler::apply_aspect_ratio(
            Some(&ImageDimension::Cells(20)),
            Some(&ImageDimension::Cells(10)),
            20,
            10,
            160,
            160,
            80,
            24,
            8,
            16,
        );
        assert_eq!(cols, 20);
        assert_eq!(rows, 10);
    }

    #[test]
    fn aspect_ratio_width_auto_height_explicit() {
        // height=10 rows, image is square (100x100), cell aspect 2:1 (8w x 16h)
        // scaled_cols = 10 * 100 * 16 / (100 * 8) = 20
        let (cols, rows) = TerminalHandler::apply_aspect_ratio(
            None,
            Some(&ImageDimension::Cells(10)),
            1, // initial cols (ignored, will be recomputed)
            10,
            100,
            100,
            80,
            24,
            8,
            16,
        );
        assert_eq!(rows, 10);
        assert_eq!(cols, 20);
    }

    #[test]
    fn aspect_ratio_height_auto_width_explicit() {
        // width=20 cols, image is square (100x100), cell aspect 2:1 (8w x 16h)
        // scaled_rows = 20 * 100 * 8 / (100 * 16) = 10
        let (cols, rows) = TerminalHandler::apply_aspect_ratio(
            Some(&ImageDimension::Cells(20)),
            None,
            20,
            1, // initial rows (ignored, will be recomputed)
            100,
            100,
            80,
            24,
            8,
            16,
        );
        assert_eq!(cols, 20);
        assert_eq!(rows, 10);
    }

    #[test]
    fn aspect_ratio_clamped_to_term_size() {
        // width=80 cols, image is very tall (100w x 10000h)
        // scaled_rows = 80 * 10000 * 8 / (100 * 16) = 4000, clamped to 24
        let (cols, rows) = TerminalHandler::apply_aspect_ratio(
            Some(&ImageDimension::Cells(80)),
            None,
            80,
            1,
            100,
            10000,
            80,
            24,
            8,
            16,
        );
        assert_eq!(cols, 80);
        assert_eq!(rows, 24);
    }

    #[test]
    fn aspect_ratio_non_square_cells() {
        // Verify the fix: non-2:1 cell aspect (e.g. 10w x 20h).
        // height=10 rows, square image (200x200)
        // scaled_cols = 10 * 200 * 20 / (200 * 10) = 20
        let (cols, rows) = TerminalHandler::apply_aspect_ratio(
            None,
            Some(&ImageDimension::Cells(10)),
            1,
            10,
            200,
            200,
            80,
            24,
            10,
            20,
        );
        assert_eq!(rows, 10);
        assert_eq!(cols, 20);

        // With 12w x 18h cells (1.5:1 ratio), square image
        // scaled_cols = 10 * 200 * 18 / (200 * 12) = 15
        let (cols2, rows2) = TerminalHandler::apply_aspect_ratio(
            None,
            Some(&ImageDimension::Cells(10)),
            1,
            10,
            200,
            200,
            80,
            24,
            12,
            18,
        );
        assert_eq!(rows2, 10);
        assert_eq!(cols2, 15);
    }

    // ------------------------------------------------------------------
    // handle_iterm2_inline_image integration test
    // ------------------------------------------------------------------

    #[test]
    fn handle_iterm2_inline_image_places_image_in_buffer() {
        use freminal_common::buffer_states::osc::ITerm2InlineImageData;

        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Create a minimal 2x2 red PNG image in memory.
        let mut png_buf = Vec::new();
        {
            use image::ImageEncoder;
            let encoder = image::codecs::png::PngEncoder::new(&mut png_buf);
            // 2x2 RGBA image: all red
            let rgba_data: [u8; 16] = [
                255, 0, 0, 255, // pixel (0,0)
                255, 0, 0, 255, // pixel (1,0)
                255, 0, 0, 255, // pixel (0,1)
                255, 0, 0, 255, // pixel (1,1)
            ];
            encoder
                .write_image(&rgba_data, 2, 2, image::ExtendedColorType::Rgba8)
                .unwrap();
        }

        let image_data = ITerm2InlineImageData {
            name: Some("red.png".to_string()),
            size: Some(png_buf.len()),
            width: Some(ImageDimension::Cells(4)),
            height: Some(ImageDimension::Cells(2)),
            preserve_aspect_ratio: false,
            inline: true,
            do_not_move_cursor: false,
            data: png_buf,
        };

        handler.handle_iterm2_inline_image(&image_data);

        // After placement, cursor should have moved down by display_rows (2).
        // The buffer should contain image cells.

        // Check that at least one image overlay has been placed.
        let has_image_cell = handler.buffer().has_any_image_cell();
        assert!(has_image_cell, "Expected at least one image cell in buffer");
    }

    #[test]
    fn handle_iterm2_inline_image_non_inline_ignored() {
        use freminal_common::buffer_states::osc::ITerm2InlineImageData;

        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let image_data = ITerm2InlineImageData {
            name: None,
            size: None,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            inline: false, // not inline → should be ignored
            do_not_move_cursor: false,
            data: vec![0xFF; 100],
        };

        // Cursor should not move.
        let cursor_before = handler.cursor_pos();
        handler.handle_iterm2_inline_image(&image_data);
        let cursor_after = handler.cursor_pos();
        assert_eq!(cursor_before, cursor_after);
    }

    #[test]
    fn handle_iterm2_inline_image_do_not_move_cursor() {
        use freminal_common::buffer_states::osc::{ITerm2InlineImageData, ImageDimension};
        use image::ImageEncoder;

        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Create a minimal test PNG.
        let mut png_buf = Vec::new();
        {
            let encoder = image::codecs::png::PngEncoder::new(&mut png_buf);
            let rgba_data: [u8; 16] = [
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ];
            encoder
                .write_image(&rgba_data, 2, 2, image::ExtendedColorType::Rgba8)
                .unwrap();
        }

        // Position cursor at a known location.
        handler.buffer_mut().set_cursor_pos(Some(5), Some(3));
        let cursor_before = handler.cursor_pos();

        let image_data = ITerm2InlineImageData {
            name: None,
            size: Some(png_buf.len()),
            width: Some(ImageDimension::Cells(4)),
            height: Some(ImageDimension::Cells(2)),
            preserve_aspect_ratio: false,
            inline: true,
            do_not_move_cursor: true,
            data: png_buf,
        };

        handler.handle_iterm2_inline_image(&image_data);

        // Cursor should NOT have moved because doNotMoveCursor=1.
        let cursor_after = handler.cursor_pos();
        assert_eq!(
            cursor_before, cursor_after,
            "Cursor should be preserved when doNotMoveCursor=1"
        );

        // But the image should still have been placed.
        let has_image = handler.buffer().has_any_image_cell();
        assert!(has_image, "Image should still be placed");
    }

    // ------------------------------------------------------------------
    // iTerm2 multipart file transfer integration tests
    // ------------------------------------------------------------------

    /// Create a minimal 2x2 red PNG image as raw bytes.
    fn make_test_png() -> Vec<u8> {
        use image::ImageEncoder;
        let mut png_buf = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut png_buf);
        // 2x2 RGBA image: all red
        let rgba_data: [u8; 16] = [
            255, 0, 0, 255, // pixel (0,0)
            255, 0, 0, 255, // pixel (1,0)
            255, 0, 0, 255, // pixel (0,1)
            255, 0, 0, 255, // pixel (1,1)
        ];
        encoder
            .write_image(&rgba_data, 2, 2, image::ExtendedColorType::Rgba8)
            .unwrap();
        png_buf
    }

    #[test]
    fn multipart_begin_part_end_places_image() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let png_data = make_test_png();

        // Begin: metadata with no payload
        let begin_data = ITerm2InlineImageData {
            name: Some("red.png".to_string()),
            size: Some(png_data.len()),
            width: Some(ImageDimension::Cells(4)),
            height: Some(ImageDimension::Cells(2)),
            preserve_aspect_ratio: false,
            inline: true,
            do_not_move_cursor: false,
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin_data));

        // Verify no image placed yet
        let has_image_before = handler.buffer().has_any_image_cell();
        assert!(
            !has_image_before,
            "No image should be placed before FileEnd"
        );

        // Send data in two chunks
        let mid = png_data.len() / 2;
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(png_data[..mid].to_vec()));
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(png_data[mid..].to_vec()));

        // End: assemble and place
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);

        // Verify image was placed
        let has_image_after = handler.buffer().has_any_image_cell();
        assert!(has_image_after, "Image should be placed after FileEnd");

        // Verify multipart state was cleared
        assert!(
            handler.multipart_state.is_none(),
            "multipart_state should be None after FileEnd"
        );
    }

    #[test]
    fn multipart_single_chunk_places_image() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let png_data = make_test_png();

        let begin_data = ITerm2InlineImageData {
            name: None,
            size: None,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            inline: true,
            do_not_move_cursor: false,
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin_data));
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(png_data));
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);

        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            has_image,
            "Image should be placed after single-chunk transfer"
        );
    }

    #[test]
    fn multipart_file_part_without_begin_ignored() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // FilePart with no active transfer — should be silently ignored
        let cursor_before = handler.cursor_pos();
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(vec![1, 2, 3]));
        let cursor_after = handler.cursor_pos();
        assert_eq!(cursor_before, cursor_after);

        // Verify no multipart state was created
        assert!(handler.multipart_state.is_none());
    }

    #[test]
    fn multipart_file_end_without_begin_ignored() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // FileEnd with no active transfer — should be silently ignored
        let cursor_before = handler.cursor_pos();
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);
        let cursor_after = handler.cursor_pos();
        assert_eq!(cursor_before, cursor_after);
    }

    #[test]
    fn multipart_begin_resets_previous_incomplete_transfer() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let png_data = make_test_png();

        // Start first transfer (will be abandoned)
        let begin1 = ITerm2InlineImageData {
            name: Some("first.png".to_string()),
            size: None,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            inline: true,
            do_not_move_cursor: false,
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin1));
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(vec![0xDE, 0xAD]));

        // Start second transfer — should discard the first
        let begin2 = ITerm2InlineImageData {
            name: Some("second.png".to_string()),
            size: Some(png_data.len()),
            width: Some(ImageDimension::Cells(4)),
            height: Some(ImageDimension::Cells(2)),
            preserve_aspect_ratio: false,
            inline: true,
            do_not_move_cursor: false,
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin2));

        // Verify state was replaced (accumulated data from first transfer is gone)
        let state = handler.multipart_state.as_ref().unwrap();
        assert_eq!(state.metadata.name, Some("second.png".to_string()));
        assert!(
            state.accumulated_data.is_empty(),
            "accumulated data should be empty after new begin"
        );

        // Complete second transfer with real image data
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(png_data));
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);

        let has_image = handler.buffer().has_any_image_cell();
        assert!(has_image, "Second transfer should produce an image");
    }

    #[test]
    fn multipart_empty_payload_ignored_on_file_end() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Begin a transfer but send no FilePart chunks
        let begin_data = ITerm2InlineImageData {
            name: None,
            size: None,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            inline: true,
            do_not_move_cursor: false,
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin_data));
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);

        // No image should be placed (empty payload)
        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            !has_image,
            "Empty multipart transfer should not place an image"
        );

        // State should be cleared
        assert!(handler.multipart_state.is_none());
    }

    #[test]
    fn multipart_non_inline_ignored() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let png_data = make_test_png();

        // Begin with inline=false (download, not display)
        let begin_data = ITerm2InlineImageData {
            name: None,
            size: None,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            inline: false, // not inline
            do_not_move_cursor: false,
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin_data));
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(png_data));

        let cursor_before = handler.cursor_pos();
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);
        let cursor_after = handler.cursor_pos();

        // inline=false means the image is a download, not an inline display.
        // handle_iterm2_inline_image returns early for non-inline, so no image placed.
        assert_eq!(
            cursor_before, cursor_after,
            "Cursor should not move for non-inline"
        );

        let has_image = handler.buffer().has_any_image_cell();
        assert!(
            !has_image,
            "Non-inline multipart transfer should not place an image"
        );
    }

    // ------------------------------------------------------------------
    // dispatch_tmux_csi unit tests
    // ------------------------------------------------------------------

    #[test]
    fn tmux_csi_cup_dispatches_directly() {
        let mut handler = TerminalHandler::new(80, 24);
        // CSI 5;10H → move cursor to row 5, col 10 (1-based)
        let dispatched = handler.dispatch_tmux_csi(b"5;10H");
        assert!(dispatched, "CUP should be handled directly");
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 9, "col should be 10 - 1 = 9 (0-based)");
        assert_eq!(cursor.y, 4, "row should be 5 - 1 = 4 (0-based)");
    }

    #[test]
    fn tmux_csi_cup_default_params() {
        let mut handler = TerminalHandler::new(80, 24);
        // CSI H → move cursor to row 1, col 1 (default)
        let dispatched = handler.dispatch_tmux_csi(b"H");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 0);
        assert_eq!(cursor.y, 0);
    }

    #[test]
    fn tmux_csi_cup_with_f_terminator() {
        let mut handler = TerminalHandler::new(80, 24);
        // CSI 3;7f → same as H
        let dispatched = handler.dispatch_tmux_csi(b"3;7f");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 6);
        assert_eq!(cursor.y, 2);
    }

    #[test]
    fn tmux_csi_cursor_up() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(5), Some(10));
        let dispatched = handler.dispatch_tmux_csi(b"3A");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.y, 6, "should move up 3 from row 9 → row 6");
    }

    #[test]
    fn tmux_csi_cursor_down() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(1), Some(5));
        let dispatched = handler.dispatch_tmux_csi(b"2B");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.y, 6, "should move down 2 from row 4 → row 6");
    }

    #[test]
    fn tmux_csi_cursor_forward() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(10), Some(1));
        let dispatched = handler.dispatch_tmux_csi(b"5C");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 14, "should move forward 5 from col 9 → col 14");
    }

    #[test]
    fn tmux_csi_cursor_backward() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(10), Some(1));
        let dispatched = handler.dispatch_tmux_csi(b"3D");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 6, "should move backward 3 from col 9 → col 6");
    }

    #[test]
    fn tmux_csi_cha() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(1), Some(5));
        let dispatched = handler.dispatch_tmux_csi(b"20G");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 19, "CHA should set col to 20 - 1 = 19");
        assert_eq!(cursor.y, 4, "CHA should not change row");
    }

    #[test]
    fn tmux_csi_vpa() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(10), Some(1));
        let dispatched = handler.dispatch_tmux_csi(b"15d");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.y, 14, "VPA should set row to 15 - 1 = 14");
    }

    #[test]
    fn tmux_csi_dec_private_falls_through() {
        let mut handler = TerminalHandler::new(80, 24);
        // CSI ? 1049 h — DEC private mode, should not be handled directly
        let dispatched = handler.dispatch_tmux_csi(b"?1049h");
        assert!(
            !dispatched,
            "DEC private modes should fall through to reparse queue"
        );
    }

    #[test]
    fn tmux_csi_sgr_falls_through() {
        let mut handler = TerminalHandler::new(80, 24);
        // CSI 1;32 m — SGR bold green, should fall through
        let dispatched = handler.dispatch_tmux_csi(b"1;32m");
        assert!(!dispatched, "SGR should fall through to reparse queue");
    }

    #[test]
    fn tmux_csi_erase_in_display() {
        let mut handler = TerminalHandler::new(80, 24);
        // Write some text first
        handler.handle_data(b"Hello");
        // CSI 2 J — erase display
        let dispatched = handler.dispatch_tmux_csi(b"2J");
        assert!(dispatched, "ED should be handled directly");
    }

    #[test]
    fn tmux_csi_erase_in_line() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"Hello");
        let dispatched = handler.dispatch_tmux_csi(b"0K");
        assert!(dispatched, "EL should be handled directly");
    }

    #[test]
    fn tmux_csi_empty_body_returns_false() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(!handler.dispatch_tmux_csi(b""));
    }

    #[test]
    fn tmux_csi_intermediates_fall_through() {
        let mut handler = TerminalHandler::new(80, 24);
        // CSI with intermediate byte (space + p = DECRQM)
        assert!(!handler.dispatch_tmux_csi(b"?1049$p"));
    }

    #[test]
    fn parse_csi_params_basic() {
        assert_eq!(
            TerminalHandler::parse_csi_params(b"1;42"),
            vec![Some(1), Some(42)]
        );
    }

    #[test]
    fn parse_csi_params_empty() {
        assert_eq!(
            TerminalHandler::parse_csi_params(b""),
            Vec::<Option<usize>>::new()
        );
    }

    #[test]
    fn parse_csi_params_missing_field() {
        assert_eq!(
            TerminalHandler::parse_csi_params(b";42"),
            vec![None, Some(42)]
        );
    }

    #[test]
    fn parse_csi_params_single() {
        assert_eq!(TerminalHandler::parse_csi_params(b"5"), vec![Some(5)]);
    }

    /// Integration test: simulate the exact nvim tmux passthrough scenario
    /// where CUP and Kitty Put arrive as separate DCS tmux items in the same
    /// `process_outputs()` batch.  With the direct CSI dispatch, the CUP
    /// should execute immediately so the Put reads the correct cursor position.
    #[test]
    fn tmux_passthrough_cup_then_kitty_put_ordering() {
        let mut handler = TerminalHandler::new(80, 24);

        // Start cursor at 0,0
        assert_eq!(handler.buffer.get_cursor().pos.x, 0);
        assert_eq!(handler.buffer.get_cursor().pos.y, 0);

        // Simulate: DCS tmux passthrough containing CSI 1;42H (CUP row 1, col 42)
        // The tmux DCS wrapper has already been stripped; the inner content is
        // ESC [ 1 ; 4 2 H with doubled ESC bytes.
        // undouble_esc would produce: ESC [ 1 ; 4 2 H
        // handle_tmux_passthrough would match inner[1] == '[' and call dispatch_tmux_csi
        // with "1;42H".
        let dispatched = handler.dispatch_tmux_csi(b"1;42H");
        assert!(dispatched);
        let cursor = handler.buffer.get_cursor().pos;
        assert_eq!(cursor.x, 41, "col should be 42 - 1 = 41 (0-based)");
        assert_eq!(cursor.y, 0, "row should be 1 - 1 = 0 (0-based)");

        // Now the APC Kitty Put would execute, reading cursor.pos correctly.
    }
}
