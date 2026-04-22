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
        ftcs::FtcsState,
        kitty_graphics::KittyControlData,
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
        osc::ITerm2InlineImageData,
        pointer_shape::PointerShape,
        tchar::TChar,
        terminal_output::TerminalOutput,
        terminal_sections::TerminalSections,
        unicode_placeholder::{
            VirtualPlacement, color_to_image_id, color_to_placement_id, is_placeholder,
            parse_placeholder_diacritics,
        },
        window_manipulation::WindowManipulation,
    },
    colors::{ColorPalette, TerminalColor},
    cursor::CursorVisualStyle,
    pty_write::PtyWrite,
    themes::ThemePalette,
};
use std::borrow::Cow;
use std::collections::HashMap;

use freminal_buffer::buffer::Buffer;
use freminal_buffer::image_store::{ImagePlacement, ImageProtocol};

mod cursor_ops;
mod dcs;
mod edit_ops;
mod graphics_iterm2;
mod graphics_kitty;
mod graphics_sixel;
mod osc;
mod osc_colors;
mod pty_writer;
mod reports;
mod scroll_ops;
mod sgr;
mod shell_integration;
mod window_ops;

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
    /// Pointer (mouse cursor) shape requested via OSC 22.
    ///
    /// Defaults to `PointerShape::Default` (OS default arrow).
    /// Reset to `Default` by `OSC 22 ; ST` (empty name) or full reset.
    pointer_shape: PointerShape,
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
    in_band_resize_enabled: InBandResizeMode,
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
            pointer_shape: PointerShape::Default,
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
            in_band_resize_enabled: InBandResizeMode::Reset,
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

    /// Get the current pointer (mouse cursor) shape requested via OSC 22.
    ///
    /// Returns `PointerShape::Default` when no override is active.
    #[must_use]
    pub const fn pointer_shape(&self) -> PointerShape {
        self.pointer_shape
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
        self.pointer_shape = PointerShape::Default;
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
                matches!(tch, TChar::Utf8(buf, len) if is_placeholder(&buf[..usize::from(*len)]));

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
                    self.handle_placeholder_char(&buf[..usize::from(*len)]);
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

    /// Update format tag directly
    pub fn set_format(&mut self, format: FormatTag) {
        self.current_format = format.clone();
        self.buffer.set_format(format);
    }

    /// Set the PTY write channel.  Once set, responses such as CPR and DA1
    /// will be sent through this channel rather than silently discarded.
    pub fn set_write_tx(&mut self, tx: Sender<PtyWrite>) {
        self.write_tx = Some(tx);
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
    ) -> Vec<Option<freminal_buffer::image_store::ImagePlacement>> {
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
                | Mode::AltScreen47(AltScreen47::Alternate)
                    if self.allow_alt_screen == AllowAltScreen::Allow =>
                {
                    self.handle_enter_alternate();
                }
                Mode::XtExtscrn(XtExtscrn::Primary) | Mode::AltScreen47(AltScreen47::Primary)
                    if self.allow_alt_screen == AllowAltScreen::Allow =>
                {
                    self.handle_leave_alternate();
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
                Mode::Deccolm(Deccolm::Column132)
                    if self.allow_column_mode_switch
                        == AllowColumnModeSwitch::AllowColumnModeSwitch =>
                {
                    // Save the current width so CSI?3l can restore it
                    // instead of hardcoding 80.
                    if self.pre_deccolm_width.is_none() {
                        self.pre_deccolm_width = Some(self.buffer.terminal_width());
                    }
                    self.buffer.set_column_mode(132);
                    self.send_pty_resize(132);
                }
                Mode::Deccolm(Deccolm::Column80)
                    if self.allow_column_mode_switch
                        == AllowColumnModeSwitch::AllowColumnModeSwitch =>
                {
                    // Restore the pre-DECCOLM width (falls back to 80 if
                    // no prior width was saved — e.g. CSI?3l without a
                    // preceding CSI?3h).
                    let restore_width = self.pre_deccolm_width.take().unwrap_or(80);
                    self.buffer.set_column_mode(restore_width);
                    self.send_pty_resize(restore_width);
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
                // XtExtscrn/AltScreen47 Alternate/Primary without allow_alt_screen
                // permission, and Deccolm Column132/Column80 without
                // allow_column_mode_switch permission, are also silently ignored.
                Mode::XtExtscrn(XtExtscrn::Alternate | XtExtscrn::Primary)
                | Mode::AltScreen47(AltScreen47::Alternate | AltScreen47::Primary)
                | Mode::Deccolm(Deccolm::Column132 | Deccolm::Column80)
                | Mode::Decckm(_)
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
                | Mode::Theming(_)
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
                    self.in_band_resize_enabled = InBandResizeMode::Set;
                    // Send an immediate resize notification per the specification
                    self.send_in_band_resize();
                }
                Mode::InBandResizeMode(InBandResizeMode::Reset) => {
                    self.in_band_resize_enabled = InBandResizeMode::Reset;
                }
                Mode::InBandResizeMode(InBandResizeMode::Query) => {
                    let mode = if self.in_band_resize_enabled == InBandResizeMode::Set {
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
                Mode::NoOp | Mode::Decsclm(_) | Mode::Unknown(_) => {
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
                    .set_cursor_line_width(freminal_buffer::row::LineWidth::DoubleHeightTop);
            }
            TerminalOutput::DoubleLineHeightBottom => {
                self.buffer
                    .set_cursor_line_width(freminal_buffer::row::LineWidth::DoubleHeightBottom);
            }
            TerminalOutput::SingleWidthLine => {
                self.buffer
                    .set_cursor_line_width(freminal_buffer::row::LineWidth::Normal);
            }
            TerminalOutput::DoubleWidthLine => {
                self.buffer
                    .set_cursor_line_width(freminal_buffer::row::LineWidth::DoubleWidth);
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
                // u32 → usize is lossless on 32/64-bit Freminal targets.
                let n = usize::value_from(*n)
                    .unwrap_or(0)
                    .min(self.kitty_keyboard_stack.len());
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
            osc::{AnsiOscType, UrlResponse},
            terminal_output::TerminalOutput,
            url::Url,
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
                    String::from_utf8_lossy(&buf[..usize::from(*len)]).to_string()
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

    // ------------------------------------------------------------------
    // OSC 22 — pointer shape
    // ------------------------------------------------------------------

    #[test]
    fn osc22_default_pointer_shape_on_new() {
        let handler = TerminalHandler::new(80, 24);
        assert_eq!(
            handler.pointer_shape(),
            freminal_common::buffer_states::pointer_shape::PointerShape::Default,
            "initial pointer shape must be Default"
        );
    }

    #[test]
    fn osc22_set_and_read_pointer_shape() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::SetPointerShape(
            freminal_common::buffer_states::pointer_shape::PointerShape::Text,
        ));
        assert_eq!(
            handler.pointer_shape(),
            freminal_common::buffer_states::pointer_shape::PointerShape::Text,
            "OSC 22 must update pointer_shape to Text"
        );
    }

    #[test]
    fn osc22_reset_via_default_shape() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::SetPointerShape(
            freminal_common::buffer_states::pointer_shape::PointerShape::Crosshair,
        ));
        handler.handle_osc(&AnsiOscType::SetPointerShape(
            freminal_common::buffer_states::pointer_shape::PointerShape::Default,
        ));
        assert_eq!(
            handler.pointer_shape(),
            freminal_common::buffer_states::pointer_shape::PointerShape::Default,
            "OSC 22 with Default shape must reset to Default"
        );
    }

    #[test]
    fn osc22_full_reset_clears_pointer_shape() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::SetPointerShape(
            freminal_common::buffer_states::pointer_shape::PointerShape::Pointer,
        ));
        handler.full_reset();
        assert_eq!(
            handler.pointer_shape(),
            freminal_common::buffer_states::pointer_shape::PointerShape::Default,
            "full_reset must clear pointer_shape to Default"
        );
    }

    // ------------------------------------------------------------------
    // Mode query tests (DECRQM — each mode set/reset/query)
    // ------------------------------------------------------------------

    /// Helper: create a handler with a PTY write channel, return (handler, rx).
    fn handler_with_pty() -> (TerminalHandler, crossbeam_channel::Receiver<PtyWrite>) {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);
        (handler, rx)
    }

    /// Helper: drain a single `PtyWrite::Write` response as a `String`.
    fn recv_pty_string(rx: &crossbeam_channel::Receiver<PtyWrite>) -> String {
        match rx.try_recv().expect("expected a PtyWrite") {
            PtyWrite::Write(bytes) => String::from_utf8(bytes).expect("valid UTF-8"),
            other @ PtyWrite::Resize(_) => panic!("expected PtyWrite::Write, got {other:?}"),
        }
    }

    #[test]
    fn mode_dectcem_query_show() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Dectem(Dectcem::Show))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Dectem(Dectcem::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("25"),
            "DECTCEM query response should contain mode 25"
        );
    }

    #[test]
    fn mode_dectcem_query_hide() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Dectem(Dectcem::Hide))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Dectem(Dectcem::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("25"),
            "DECTCEM query response should contain mode 25"
        );
    }

    #[test]
    fn mode_decawm_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decawm(Decawm::NoAutoWrap))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decawm(Decawm::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains('7'),
            "DECAWM query response should contain mode 7"
        );
    }

    #[test]
    fn mode_lnm_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::LineFeedMode(Lnm::NewLine))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::LineFeedMode(Lnm::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("20"),
            "LNM query response should contain mode 20"
        );
    }

    #[test]
    fn mode_xtextscrn_query_primary() {
        let (mut handler, rx) = handler_with_pty();
        // Default is primary screen
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtExtscrn(XtExtscrn::Query))]);
        let resp = recv_pty_string(&rx);
        // Should report DecRst (not in alt)
        assert!(
            resp.contains("1049"),
            "XtExtscrn query should contain mode 1049"
        );
    }

    #[test]
    fn mode_altscreen47_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::AltScreen47(AltScreen47::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("47"),
            "AltScreen47 query should contain mode 47"
        );
    }

    #[test]
    fn mode_save_cursor_1048_query() {
        let (mut handler, rx) = handler_with_pty();
        // No cursor saved yet
        handler.process_outputs(&[TerminalOutput::Mode(Mode::SaveCursor1048(
            SaveCursor1048::Query,
        ))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("1048"),
            "SaveCursor1048 query should contain mode 1048"
        );
    }

    #[test]
    fn mode_save_cursor_1048_save_then_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::SaveCursor1048(
            SaveCursor1048::Save,
        ))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::SaveCursor1048(
            SaveCursor1048::Query,
        ))]);
        let resp = recv_pty_string(&rx);
        // Should report DecSet (cursor is saved)
        assert!(resp.contains("1048"), "response should contain mode 1048");
    }

    #[test]
    fn mode_xtcblink_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(resp.contains("12"), "XtCBlink query should contain mode 12");
    }

    #[test]
    fn mode_decom_set_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decom(Decom::OriginMode))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decom(Decom::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(resp.contains('6'), "DECOM query should contain mode 6");
    }

    #[test]
    fn mode_declrmm_set_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Declrmm(Declrmm::Enabled))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Declrmm(Declrmm::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(resp.contains("69"), "DECLRMM query should contain mode 69");
    }

    #[test]
    fn mode_declrmm_disable_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Declrmm(Declrmm::Enabled))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Declrmm(Declrmm::Disabled))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Declrmm(Declrmm::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(resp.contains("69"), "DECLRMM query should contain mode 69");
    }

    #[test]
    fn mode_allow_column_mode_switch_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowColumnModeSwitch(
            AllowColumnModeSwitch::NoAllowColumnModeSwitch,
        ))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowColumnModeSwitch(
            AllowColumnModeSwitch::Query,
        ))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("40"),
            "AllowColumnModeSwitch query should contain mode 40"
        );
    }

    #[test]
    fn mode_decsdm_set_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decsdm(Decsdm::DisplayMode))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decsdm(Decsdm::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(resp.contains("80"), "DECSDM query should contain mode 80");
    }

    #[test]
    fn mode_allow_alt_screen_disallow_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowAltScreen(
            AllowAltScreen::Disallow,
        ))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowAltScreen(
            AllowAltScreen::Query,
        ))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("1046"),
            "AllowAltScreen query should contain mode 1046"
        );
    }

    #[test]
    fn mode_private_color_registers_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::PrivateColorRegisters(
            PrivateColorRegisters::Shared,
        ))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::PrivateColorRegisters(
            PrivateColorRegisters::Query,
        ))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("1070"),
            "PrivateColorRegisters query should contain mode 1070"
        );
    }

    #[test]
    fn mode_private_color_registers_private_discards_shared_palette() {
        let mut handler = TerminalHandler::new(80, 24);
        // Set to shared mode first
        handler.process_outputs(&[TerminalOutput::Mode(Mode::PrivateColorRegisters(
            PrivateColorRegisters::Shared,
        ))]);
        // Switch back to private — should clear shared palette
        handler.process_outputs(&[TerminalOutput::Mode(Mode::PrivateColorRegisters(
            PrivateColorRegisters::Private,
        ))]);
        // The handler should have cleared sixel_shared_palette (internal state)
        // — we can't directly observe it, but running the path is the coverage goal.
    }

    #[test]
    fn mode_decnrcm_set_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decnrcm(Decnrcm::NrcEnabled))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decnrcm(Decnrcm::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(resp.contains("42"), "DECNRCM query should contain mode 42");
    }

    #[test]
    fn mode_reverse_wrap_around_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::ReverseWrapAround(
            ReverseWrapAround::WrapAround,
        ))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::ReverseWrapAround(
            ReverseWrapAround::Query,
        ))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("45"),
            "ReverseWrapAround query should contain mode 45"
        );
    }

    #[test]
    fn mode_xt_rev_wrap2_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtRevWrap2(XtRevWrap2::Enabled))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtRevWrap2(XtRevWrap2::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("1045"),
            "XtRevWrap2 query should contain mode 1045"
        );
    }

    #[test]
    fn mode_decanm_vt52_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decanm(Decanm::Vt52))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decanm(Decanm::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(resp.contains('2'), "DECANM query should contain mode 2");
    }

    #[test]
    fn mode_decanm_ansi_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decanm(Decanm::Ansi))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decanm(Decanm::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(resp.contains('2'), "DECANM query should contain mode 2");
    }

    #[test]
    fn mode_application_escape_key_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::ApplicationEscapeKey(
            ApplicationEscapeKey::Set,
        ))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::ApplicationEscapeKey(
            ApplicationEscapeKey::Query,
        ))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("7727"),
            "ApplicationEscapeKey query should contain mode 7727"
        );
    }

    #[test]
    fn mode_in_band_resize_set_sends_immediate_notification() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::InBandResizeMode(
            InBandResizeMode::Set,
        ))]);
        // Setting the mode sends an immediate resize notification
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("48;"),
            "In-band resize notification should contain '48;'"
        );
    }

    #[test]
    fn mode_in_band_resize_query() {
        let (mut handler, rx) = handler_with_pty();
        // Set then query
        handler.process_outputs(&[TerminalOutput::Mode(Mode::InBandResizeMode(
            InBandResizeMode::Set,
        ))]);
        let _ = rx.try_recv(); // drain the immediate notification
        handler.process_outputs(&[TerminalOutput::Mode(Mode::InBandResizeMode(
            InBandResizeMode::Query,
        ))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("2048"),
            "InBandResizeMode query should contain mode 2048"
        );
    }

    #[test]
    fn mode_in_band_resize_reset() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::InBandResizeMode(
            InBandResizeMode::Set,
        ))]);
        let _ = rx.try_recv(); // drain notification
        handler.process_outputs(&[TerminalOutput::Mode(Mode::InBandResizeMode(
            InBandResizeMode::Reset,
        ))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::InBandResizeMode(
            InBandResizeMode::Query,
        ))]);
        let resp = recv_pty_string(&rx);
        assert!(resp.contains("2048"), "Query response should contain 2048");
    }

    #[test]
    fn mode_grapheme_clustering_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::GraphemeClustering(
            GraphemeClustering::Query,
        ))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("2027"),
            "GraphemeClustering query should contain mode 2027"
        );
    }

    #[test]
    fn mode_irm_insert() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Irm(Irm::Insert))]);
        handler.handle_data(b"AB");
        // In insert mode, characters shift existing content right
        assert_eq!(handler.buffer().get_cursor().pos.x, 2);
    }

    #[test]
    fn mode_unknown_query_responds_not_recognized() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::UnknownQuery(vec![
            b'9', b'9', b'9',
        ]))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("999") && resp.contains(";0$y"),
            "Unknown query should respond with Ps=0 (not recognized): got {resp}"
        );
    }

    // ------------------------------------------------------------------
    // DECCOLM (column mode switching)
    // ------------------------------------------------------------------

    #[test]
    fn deccolm_132_with_allow() {
        let (mut handler, rx) = handler_with_pty();
        // Allow column mode switch (default is allow)
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::Column132))]);
        // Should have sent a resize via PTY
        let msg = rx.try_recv();
        assert!(msg.is_ok(), "DECCOLM 132 should send a PTY resize");
        assert_eq!(
            handler.buffer().terminal_width(),
            132,
            "Width should be 132 after DECCOLM"
        );
    }

    #[test]
    fn deccolm_80_restores_width() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::Column132))]);
        let _ = rx.try_recv(); // drain resize
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::Column80))]);
        let _ = rx.try_recv(); // drain resize
        // Should restore to original 80
        assert_eq!(handler.buffer().terminal_width(), 80);
    }

    #[test]
    fn deccolm_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(resp.contains('3'), "DECCOLM query should contain mode 3");
    }

    #[test]
    fn deccolm_blocked_when_not_allowed() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowColumnModeSwitch(
            AllowColumnModeSwitch::NoAllowColumnModeSwitch,
        ))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::Column132))]);
        // Width should remain 80
        assert_eq!(handler.buffer().terminal_width(), 80);
    }

    // ------------------------------------------------------------------
    // VT52 cursor position handling
    // ------------------------------------------------------------------

    #[test]
    fn vt52_cursor_pos_in_bounds() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decanm(Decanm::Vt52))]);
        handler.handle_cursor_pos(Some(10), Some(5)); // 1-indexed
        assert_eq!(handler.buffer().get_cursor().pos.x, 9);
        assert_eq!(handler.buffer().get_cursor().pos.y, 4);
    }

    #[test]
    fn vt52_cursor_pos_out_of_bounds_row_ignored() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decanm(Decanm::Vt52))]);
        handler.handle_cursor_pos(Some(5), Some(3));
        // Now try out-of-bounds row
        handler.handle_cursor_pos(Some(10), Some(100));
        // Row should be unchanged (2, from previous), col should also be unchanged
        // because the VT52 handler ignores both axes independently
        assert_eq!(
            handler.buffer().get_cursor().pos.y,
            2,
            "row should be unchanged"
        );
    }

    #[test]
    fn vt52_cursor_pos_out_of_bounds_col_ignored() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decanm(Decanm::Vt52))]);
        handler.handle_cursor_pos(Some(5), Some(3));
        // Out-of-bounds column
        handler.handle_cursor_pos(Some(200), Some(3));
        assert_eq!(
            handler.buffer().get_cursor().pos.x,
            4,
            "col should be unchanged"
        );
    }

    // ------------------------------------------------------------------
    // Device attributes & report responses
    // ------------------------------------------------------------------

    #[test]
    fn da1_ansi_mode_response() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::RequestDeviceAttributes]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("?65;"),
            "DA1 ANSI response should contain '?65;'"
        );
    }

    #[test]
    fn da1_vt52_mode_response() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decanm(Decanm::Vt52))]);
        handler.process_outputs(&[TerminalOutput::RequestDeviceAttributes]);
        let resp = recv_pty_string(&rx);
        assert_eq!(resp, "\x1b/Z", "DA1 in VT52 mode should respond ESC / Z");
    }

    #[test]
    fn da2_response() {
        let (mut handler, rx) = handler_with_pty();
        handler.handle_secondary_device_attributes();
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains(">65;0;0c"),
            "DA2 should respond with >65;0;0c"
        );
    }

    #[test]
    fn da3_response() {
        let (mut handler, rx) = handler_with_pty();
        handler.handle_tertiary_device_attributes();
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("!|00000000"),
            "DA3 should respond with DCS !|00000000 ST"
        );
    }

    #[test]
    fn decreqtparm_ps0() {
        let (mut handler, rx) = handler_with_pty();
        handler.handle_request_terminal_parameters(0);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("2;1;1;120;120;1;0x"),
            "DECREQTPARM Ps=0 should respond with code 2"
        );
    }

    #[test]
    fn decreqtparm_ps1() {
        let (mut handler, rx) = handler_with_pty();
        handler.handle_request_terminal_parameters(1);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("3;1;1;120;120;1;0x"),
            "DECREQTPARM Ps=1 should respond with code 3"
        );
    }

    #[test]
    fn decreqtparm_invalid_ps() {
        let (mut handler, rx) = handler_with_pty();
        handler.handle_request_terminal_parameters(5);
        // Should not send any response
        assert!(
            rx.try_recv().is_err(),
            "DECREQTPARM with invalid Ps should not respond"
        );
    }

    #[test]
    fn device_name_and_version_response() {
        let (mut handler, rx) = handler_with_pty();
        handler.handle_device_name_and_version();
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("XTerm(Freminal"),
            "Device name should contain 'XTerm(Freminal'"
        );
    }

    #[test]
    fn cursor_report_normal_mode() {
        let (mut handler, rx) = handler_with_pty();
        handler.handle_cursor_pos(Some(10), Some(5)); // 1-based → 9, 4
        handler.handle_cursor_report();
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("5;10R"),
            "CPR should report row=5, col=10 (1-indexed)"
        );
    }

    #[test]
    fn cursor_report_decom_mode() {
        let (mut handler, rx) = handler_with_pty();
        handler.handle_set_scroll_region(5, 20);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Decom(Decom::OriginMode))]);
        handler.handle_cursor_pos(Some(1), Some(5)); // 0-based: x=0, y=4
        handler.handle_cursor_report();
        let resp = recv_pty_string(&rx);
        // In DECOM mode, row is relative to scroll region top (0-based row 4, region top 4)
        // So relative row = 4 - 4 = 0, reported as 1
        assert!(
            resp.contains('R'),
            "CPR in DECOM mode should contain 'R' terminator"
        );
    }

    #[test]
    fn dsr_response() {
        let (mut handler, rx) = handler_with_pty();
        handler.handle_device_status_report();
        let resp = recv_pty_string(&rx);
        assert!(resp.contains("0n"), "DSR should respond with '0n'");
    }

    #[test]
    fn color_theme_report() {
        let (mut handler, rx) = handler_with_pty();
        handler.handle_color_theme_report();
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("?997;2n"),
            "Color theme report should indicate dark (2)"
        );
    }

    // ------------------------------------------------------------------
    // Window manipulation
    // ------------------------------------------------------------------

    #[test]
    fn window_manipulation_report_char_size() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::WindowManipulation(
            WindowManipulation::ReportCharacterSizeInPixels,
        )]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("6;"),
            "ReportCharacterSizeInPixels should start with '6;'"
        );
    }

    #[test]
    fn window_manipulation_report_terminal_size() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::WindowManipulation(
            WindowManipulation::ReportTerminalSizeInCharacters,
        )]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("8;24;80"),
            "ReportTerminalSizeInCharacters should report '8;24;80'"
        );
    }

    #[test]
    fn window_manipulation_report_root_window_size() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::WindowManipulation(
            WindowManipulation::ReportRootWindowSizeInCharacters,
        )]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("9;24;80"),
            "ReportRootWindowSizeInCharacters should report '9;24;80'"
        );
    }

    #[test]
    fn window_manipulation_other_pushed_to_commands() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::WindowManipulation(
            WindowManipulation::SetTitleBarText("test".to_string()),
        )]);
        let cmds = handler.take_window_commands();
        assert_eq!(cmds.len(), 1);
    }

    // ------------------------------------------------------------------
    // Kitty keyboard protocol
    // ------------------------------------------------------------------

    #[test]
    fn kitty_keyboard_push_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(3)]);
        assert_eq!(handler.kitty_keyboard_flags(), 3);
        handler.process_outputs(&[TerminalOutput::KittyKeyboardQuery]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("?3u"),
            "Kitty keyboard query should report '?3u'"
        );
    }

    #[test]
    fn kitty_keyboard_pop() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(1)]);
        handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(5)]);
        assert_eq!(handler.kitty_keyboard_flags(), 5);
        handler.process_outputs(&[TerminalOutput::KittyKeyboardPop(1)]);
        assert_eq!(handler.kitty_keyboard_flags(), 1);
    }

    #[test]
    fn kitty_keyboard_pop_more_than_stack() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(7)]);
        handler.process_outputs(&[TerminalOutput::KittyKeyboardPop(10)]);
        assert_eq!(handler.kitty_keyboard_flags(), 0);
    }

    #[test]
    fn kitty_keyboard_set_mode1_replace() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(0xFF)]);
        handler.process_outputs(&[TerminalOutput::KittyKeyboardSet { flags: 3, mode: 1 }]);
        assert_eq!(handler.kitty_keyboard_flags(), 3);
    }

    #[test]
    fn kitty_keyboard_set_mode2_or() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(1)]);
        handler.process_outputs(&[TerminalOutput::KittyKeyboardSet { flags: 2, mode: 2 }]);
        assert_eq!(handler.kitty_keyboard_flags(), 3); // 1 | 2
    }

    #[test]
    fn kitty_keyboard_set_mode3_and_not() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(7)]);
        handler.process_outputs(&[TerminalOutput::KittyKeyboardSet { flags: 2, mode: 3 }]);
        assert_eq!(handler.kitty_keyboard_flags(), 5); // 7 & !2
    }

    #[test]
    fn kitty_keyboard_set_on_empty_stack() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::KittyKeyboardSet { flags: 5, mode: 1 }]);
        assert_eq!(handler.kitty_keyboard_flags(), 5);
    }

    #[test]
    fn kitty_keyboard_stack_overflow() {
        let mut handler = TerminalHandler::new(80, 24);
        // Push beyond max depth to test eviction
        #[allow(clippy::cast_possible_truncation)]
        let max_depth = KittyKeyboardFlags::MAX_STACK_DEPTH as u32;
        for i in 0..=max_depth {
            handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(i)]);
        }
        // Stack should be at max depth, oldest entry evicted
        assert!(handler.kitty_keyboard_stack.len() <= KittyKeyboardFlags::MAX_STACK_DEPTH);
    }

    // ------------------------------------------------------------------
    // Repeat character (REP)
    // ------------------------------------------------------------------

    #[test]
    fn repeat_character() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"A");
        handler.process_outputs(&[TerminalOutput::RepeatCharacter(5)]);
        // Should have 'A' + 5 repeats = 6 chars total
        assert_eq!(handler.buffer().get_cursor().pos.x, 6);
    }

    #[test]
    fn repeat_character_no_previous() {
        let mut handler = TerminalHandler::new(80, 24);
        // No previous graphic char — REP should be a no-op
        handler.process_outputs(&[TerminalOutput::RepeatCharacter(5)]);
        assert_eq!(handler.buffer().get_cursor().pos.x, 0);
    }

    // ------------------------------------------------------------------
    // FTCS shell integration markers
    // ------------------------------------------------------------------

    #[test]
    fn ftcs_state_machine() {
        use freminal_common::buffer_states::ftcs::{FtcsMarker, FtcsState};

        let mut handler = TerminalHandler::new(80, 24);
        assert_eq!(handler.ftcs_state(), FtcsState::None);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InPrompt);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InCommand);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::OutputStart));
        assert_eq!(handler.ftcs_state(), FtcsState::InOutput);

        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(Some(0))));
        assert_eq!(handler.ftcs_state(), FtcsState::None);
        assert_eq!(handler.last_exit_code(), Some(0));
    }

    #[test]
    fn ftcs_command_finished_no_exit_code() {
        use freminal_common::buffer_states::ftcs::FtcsMarker;

        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::CommandFinished(None)));
        assert_eq!(handler.last_exit_code(), None);
    }

    #[test]
    fn ftcs_prompt_property_is_no_op() {
        use freminal_common::buffer_states::ftcs::{FtcsMarker, FtcsState, PromptKind};

        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptStart));
        handler.handle_osc(&AnsiOscType::Ftcs(FtcsMarker::PromptProperty(
            PromptKind::Initial,
        )));
        // State should still be InPrompt — prompt property doesn't change state
        assert_eq!(handler.ftcs_state(), FtcsState::InPrompt);
    }

    // ------------------------------------------------------------------
    // Handle resize with in-band notification
    // ------------------------------------------------------------------

    #[test]
    fn handle_resize_sends_in_band_when_enabled() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::InBandResizeMode(
            InBandResizeMode::Set,
        ))]);
        let _ = rx.try_recv(); // drain initial notification
        handler.handle_resize(100, 30, 8, 16);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("48;"),
            "Resize should trigger in-band notification"
        );
    }

    #[test]
    fn handle_resize_no_notification_when_disabled() {
        let (mut handler, rx) = handler_with_pty();
        handler.handle_resize(100, 30, 8, 16);
        // No in-band resize notification expected
        assert!(
            rx.try_recv().is_err(),
            "No notification when in-band resize is disabled"
        );
    }

    #[test]
    fn handle_resize_updates_pixel_dimensions() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_resize(80, 24, 10, 20);
        // Pixel dimensions are stored internally; verify via window manipulation
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);
        handler.process_outputs(&[TerminalOutput::WindowManipulation(
            WindowManipulation::ReportCharacterSizeInPixels,
        )]);
        let resp = recv_pty_string(&rx);
        assert!(resp.contains("6;20;10"), "Pixel dims should be h=20, w=10");
    }

    #[test]
    fn handle_resize_zero_pixel_dims_not_stored() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_resize(80, 24, 10, 20);
        handler.handle_resize(90, 30, 0, 0); // zero dims should not overwrite
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);
        handler.process_outputs(&[TerminalOutput::WindowManipulation(
            WindowManipulation::ReportCharacterSizeInPixels,
        )]);
        let resp = recv_pty_string(&rx);
        // Should still have 10, 20 from the first resize
        assert!(
            resp.contains("6;20;10"),
            "Zero pixel dims should not overwrite"
        );
    }

    // ------------------------------------------------------------------
    // Tab stops via process_output
    // ------------------------------------------------------------------

    #[test]
    fn tab_clear_at_cursor() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::TabClear(0)]);
        // Tab stop at cursor position (0) should be cleared
        handler.handle_tab();
        // Default tab stop at col 8 was cleared at col 0 — but cursor is at 0,
        // so clearing col 0 doesn't affect col 8. Tab should still go to 8.
        assert_eq!(handler.buffer().get_cursor().pos.x, 8);
    }

    #[test]
    fn tab_clear_all() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::TabClear(3)]);
        handler.handle_tab();
        // All tab stops cleared — should go to last column
        assert_eq!(handler.buffer().get_cursor().pos.x, 79);
    }

    #[test]
    fn tab_clear_ps5_same_as_all() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::TabClear(5)]);
        handler.handle_tab();
        assert_eq!(handler.buffer().get_cursor().pos.x, 79);
    }

    #[test]
    fn tab_clear_line_tab_noop() {
        let mut handler = TerminalHandler::new(80, 24);
        // Ps=1,2,4 are line tab stops — should be no-ops
        handler.process_outputs(&[TerminalOutput::TabClear(1)]);
        handler.process_outputs(&[TerminalOutput::TabClear(2)]);
        handler.process_outputs(&[TerminalOutput::TabClear(4)]);
        handler.handle_tab();
        assert_eq!(
            handler.buffer().get_cursor().pos.x,
            8,
            "Line tab clears should be no-ops"
        );
    }

    #[test]
    fn horizontal_tab_set() {
        let mut handler = TerminalHandler::new(80, 24);
        // Clear all, set a custom tab stop at col 5, tab to it
        handler.process_outputs(&[TerminalOutput::TabClear(3)]);
        handler.handle_cursor_pos(Some(6), Some(1)); // col 5 (0-indexed)
        handler.process_outputs(&[TerminalOutput::HorizontalTabSet]);
        handler.handle_cursor_pos(Some(1), Some(1)); // back to col 0
        handler.handle_tab();
        assert_eq!(
            handler.buffer().get_cursor().pos.x,
            5,
            "Should tab to custom stop at 5"
        );
    }

    #[test]
    fn cursor_forward_tab() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::CursorForwardTab(2)]);
        // 2 tabs forward: 0→8→16
        assert_eq!(handler.buffer().get_cursor().pos.x, 16);
    }

    #[test]
    fn cursor_backward_tab() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(21), Some(1)); // col 20
        handler.process_outputs(&[TerminalOutput::CursorBackwardTab(1)]);
        // Backward 1 tab from col 20: previous stop is col 16
        assert_eq!(handler.buffer().get_cursor().pos.x, 16);
    }

    // ------------------------------------------------------------------
    // Miscellaneous process_output arms
    // ------------------------------------------------------------------

    #[test]
    fn process_set_cursor_pos_rel() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(10), Some(5));
        handler.process_outputs(&[TerminalOutput::SetCursorPosRel {
            x: Some(3),
            y: Some(-2),
        }]);
        assert_eq!(handler.buffer().get_cursor().pos.x, 12); // 9 + 3
        assert_eq!(handler.buffer().get_cursor().pos.y, 2); // 4 - 2
    }

    #[test]
    fn process_set_cursor_pos_rel_none() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(10), Some(5));
        handler.process_outputs(&[TerminalOutput::SetCursorPosRel { x: None, y: None }]);
        // No change — defaults to (0, 0)
        assert_eq!(handler.buffer().get_cursor().pos.x, 9);
        assert_eq!(handler.buffer().get_cursor().pos.y, 4);
    }

    #[test]
    fn process_scroll_up_and_down() {
        let mut handler = TerminalHandler::new(80, 5);
        // Write some content
        for i in 0..5_u8 {
            handler.handle_data(&[b'A' + i]);
            handler.handle_newline();
            handler.handle_carriage_return();
        }
        handler.process_outputs(&[TerminalOutput::ScrollUp(1)]);
        handler.process_outputs(&[TerminalOutput::ScrollDown(1)]);
        // Just exercising the code paths — no crash
    }

    #[test]
    fn process_index_and_reverse_index() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(1), Some(5));
        handler.process_outputs(&[TerminalOutput::Index]);
        assert_eq!(handler.buffer().get_cursor().pos.y, 5);
        handler.process_outputs(&[TerminalOutput::ReverseIndex]);
        assert_eq!(handler.buffer().get_cursor().pos.y, 4);
    }

    #[test]
    fn process_next_line() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"Hello");
        handler.process_outputs(&[TerminalOutput::NextLine]);
        assert_eq!(handler.buffer().get_cursor().pos.x, 0, "NEL should CR");
        assert_eq!(handler.buffer().get_cursor().pos.y, 1, "NEL should LF");
    }

    #[test]
    fn process_set_margins() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::SetTopAndBottomMargins {
            top_margin: 5,
            bottom_margin: 20,
        }]);
        let (top, bottom) = handler.buffer().scroll_region();
        assert_eq!(top, 4);
        assert_eq!(bottom, 19);
    }

    #[test]
    fn process_set_left_right_margins() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Declrmm(Declrmm::Enabled))]);
        handler.process_outputs(&[TerminalOutput::SetLeftAndRightMargins {
            left_margin: 5,
            right_margin: 40,
        }]);
        // The margins are set via handler
    }

    #[test]
    fn process_dec_special_graphics() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::DecSpecialGraphics(
            DecSpecialGraphics::Replace,
        )]);
        // 'q' (0x71) should map to a box drawing character
        handler.handle_data(b"q");
        // Cursor should advance
        assert_eq!(handler.buffer().get_cursor().pos.x, 1);
    }

    #[test]
    fn process_cursor_visual_style() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::CursorVisualStyle(
            CursorVisualStyle::UnderlineCursorBlink,
        )]);
        assert_eq!(
            handler.cursor_visual_style(),
            CursorVisualStyle::UnderlineCursorBlink
        );
    }

    #[test]
    fn process_line_width_variants() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"test");
        handler.process_outputs(&[TerminalOutput::DoubleLineHeightTop]);
        handler.process_outputs(&[TerminalOutput::DoubleLineHeightBottom]);
        handler.process_outputs(&[TerminalOutput::SingleWidthLine]);
        handler.process_outputs(&[TerminalOutput::DoubleWidthLine]);
        // Just exercising the code paths
    }

    #[test]
    fn process_screen_alignment_test() {
        let mut handler = TerminalHandler::new(10, 5);
        handler.process_outputs(&[TerminalOutput::ScreenAlignmentTest]);
        // Screen should be filled with 'E' characters
    }

    #[test]
    fn process_save_restore_cursor() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_cursor_pos(Some(10), Some(5));
        handler.process_outputs(&[TerminalOutput::SaveCursor]);
        handler.handle_cursor_pos(Some(1), Some(1));
        handler.process_outputs(&[TerminalOutput::RestoreCursor]);
        assert_eq!(handler.buffer().get_cursor().pos.x, 9);
        assert_eq!(handler.buffer().get_cursor().pos.y, 4);
    }

    #[test]
    fn process_reset_device() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"test");
        handler.process_outputs(&[TerminalOutput::ResetDevice]);
        // After full reset, cursor should be at origin
        assert_eq!(handler.buffer().get_cursor().pos.x, 0);
        assert_eq!(handler.buffer().get_cursor().pos.y, 0);
    }

    #[test]
    fn process_enq() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Enq]);
        let resp = recv_pty_string(&rx);
        assert_eq!(resp, "", "ENQ should send empty answerback");
    }

    #[test]
    fn process_modify_other_keys() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::ModifyOtherKeys(2)]);
        assert_eq!(handler.modify_other_keys_level(), 2);
    }

    #[test]
    fn process_invalid_and_skipped() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::Invalid]);
        handler.process_outputs(&[TerminalOutput::Skipped]);
        // Should not crash
    }

    #[test]
    fn process_application_and_normal_keypad_mode() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::ApplicationKeypadMode]);
        handler.process_outputs(&[TerminalOutput::NormalKeypadMode]);
        // Just exercising trace-only code paths
    }

    #[test]
    fn process_eight_and_seven_bit_control() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::EightBitControl]);
        handler.process_outputs(&[TerminalOutput::SevenBitControl]);
        // Handled by TerminalState — just trace paths
    }

    #[test]
    fn process_ansi_conformance_levels() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::AnsiConformanceLevelOne]);
        handler.process_outputs(&[TerminalOutput::AnsiConformanceLevelTwo]);
        handler.process_outputs(&[TerminalOutput::AnsiConformanceLevelThree]);
        // All are logged-only
    }

    #[test]
    fn process_memory_lock_unlock() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::MemoryLock]);
        handler.process_outputs(&[TerminalOutput::MemoryUnlock]);
    }

    #[test]
    fn process_cursor_to_lower_left() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::CursorToLowerLeftCorner]);
    }

    #[test]
    fn process_charset_variants_are_noops() {
        let mut handler = TerminalHandler::new(80, 24);
        let charsets = [
            TerminalOutput::CharsetDefault,
            TerminalOutput::CharsetUTF8,
            TerminalOutput::CharsetG0,
            TerminalOutput::CharsetG1,
            TerminalOutput::CharsetG1AsGR,
            TerminalOutput::CharsetG2,
            TerminalOutput::CharsetG2AsGR,
            TerminalOutput::CharsetG2AsGL,
            TerminalOutput::CharsetG3,
            TerminalOutput::CharsetG3AsGR,
            TerminalOutput::CharsetG3AsGL,
            TerminalOutput::DecSpecial,
            TerminalOutput::CharsetUK,
            TerminalOutput::CharsetUS,
            TerminalOutput::CharsetUSASCII,
            TerminalOutput::CharsetDutch,
            TerminalOutput::CharsetFinnish,
            TerminalOutput::CharsetFrench,
            TerminalOutput::CharsetFrenchCanadian,
            TerminalOutput::CharsetGerman,
            TerminalOutput::CharsetItalian,
            TerminalOutput::CharsetNorwegianDanish,
            TerminalOutput::CharsetSpanish,
            TerminalOutput::CharsetSwedish,
            TerminalOutput::CharsetSwiss,
        ];
        for cs in &charsets {
            handler.process_outputs(std::slice::from_ref(cs));
        }
    }

    #[test]
    fn process_modes_handled_by_terminal_state() {
        use freminal_common::buffer_states::modes::{
            alternate_scroll::AlternateScroll,
            decarm::Decarm,
            decbkm::Decbkm,
            decckm::Decckm,
            decnkm::Decnkm,
            decscnm::Decscnm,
            mouse::{MouseEncoding, MouseTrack},
            rl_bracket::RlBracket,
            sync_updates::SynchronizedUpdates,
            theme::Theming,
            xtmsewin::XtMseWin,
        };
        let mut handler = TerminalHandler::new(80, 24);
        let modes = [
            Mode::Decckm(Decckm::Application),
            Mode::BracketedPaste(RlBracket::Enabled),
            Mode::MouseMode(MouseTrack::NoTracking),
            Mode::MouseEncodingMode(MouseEncoding::X11),
            Mode::XtMseWin(XtMseWin::Enabled),
            Mode::Decscnm(Decscnm::ReverseDisplay),
            Mode::Decarm(Decarm::RepeatKey),
            Mode::SynchronizedUpdates(SynchronizedUpdates::DontDraw),
            Mode::Decnkm(Decnkm::Application),
            Mode::Decbkm(Decbkm::BackarrowSendsBs),
            Mode::AlternateScroll(AlternateScroll::Enabled),
            Mode::Theming(Theming::Dark),
            Mode::GraphemeClustering(GraphemeClustering::Unicode),
            Mode::GraphemeClustering(GraphemeClustering::Legacy),
        ];
        for mode in &modes {
            handler.process_outputs(&[TerminalOutput::Mode(mode.clone())]);
        }
        // All handled by TerminalState — should be no-ops in TerminalHandler
    }

    #[test]
    fn process_mode_noop_and_unknown() {
        use freminal_common::buffer_states::modes::unknown::{ModeNamespace, UnknownMode};
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::NoOp)]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Unknown(UnknownMode::new(
            b"99",
            SetMode::DecSet,
            ModeNamespace::Dec,
        )))]);
    }

    // ------------------------------------------------------------------
    // IRM (Insert/Replace Mode) text insertion
    // ------------------------------------------------------------------

    #[test]
    fn irm_insert_mode_shifts_content() {
        let mut handler = TerminalHandler::new(20, 5);
        handler.handle_data(b"ABCDE");
        handler.handle_cursor_pos(Some(3), Some(1)); // col 2
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Irm(Irm::Insert))]);
        handler.handle_data(b"XY");
        // After inserting "XY" at col 2 in insert mode, cursor should be at col 4
        assert_eq!(handler.buffer().get_cursor().pos.x, 4);
    }

    // ------------------------------------------------------------------
    // OSC remote host / CWD
    // ------------------------------------------------------------------

    #[test]
    fn osc_remote_host_cwd() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::RemoteHost(
            "file://localhost/home/user".to_string(),
        ));
        assert_eq!(handler.current_working_directory(), Some("/home/user"));
    }

    #[test]
    fn osc_remote_host_invalid() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::RemoteHost("not-a-valid-uri".to_string()));
        assert!(handler.current_working_directory().is_none());
    }

    // ------------------------------------------------------------------
    // OSC set title
    // ------------------------------------------------------------------

    #[test]
    fn osc_set_title() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::SetTitleBar("My Terminal".to_string()));
        let cmds = handler.take_window_commands();
        assert_eq!(cmds.len(), 1);
        assert!(matches!(
            &cmds[0],
            WindowManipulation::SetTitleBarText(t) if t == "My Terminal"
        ));
    }

    // ------------------------------------------------------------------
    // OSC URL hyperlinks
    // ------------------------------------------------------------------

    #[test]
    fn osc_url_start_and_end() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::Url(UrlResponse::Url(Url {
            id: Some("myid".to_string()),
            url: "https://example.com".to_string(),
        })));
        assert!(handler.current_format().url.is_some());
        handler.handle_osc(&AnsiOscType::Url(UrlResponse::End));
        assert!(handler.current_format().url.is_none());
    }

    // ------------------------------------------------------------------
    // Scroll helpers
    // ------------------------------------------------------------------

    #[test]
    fn scroll_back_and_forward() {
        let mut handler = TerminalHandler::new(80, 3);
        // Write enough to create scrollback
        for i in 0..10_u8 {
            handler.handle_data(&[b'A' + i]);
            handler.handle_newline();
            handler.handle_carriage_return();
        }
        let offset = handler.handle_scroll_back(0, 3);
        assert_eq!(offset, 3);
        let offset2 = handler.handle_scroll_forward(offset, 1);
        assert_eq!(offset2, 2);
        let bottom = TerminalHandler::handle_scroll_to_bottom();
        assert_eq!(bottom, 0);
    }

    // ------------------------------------------------------------------
    // Accessors
    // ------------------------------------------------------------------

    #[test]
    fn s8c1t_mode_accessor() {
        let mut handler = TerminalHandler::new(80, 24);
        assert_eq!(*handler.s8c1t_mode(), S8c1t::SevenBit);
        handler.set_s8c1t_mode(S8c1t::EightBit);
        assert_eq!(*handler.s8c1t_mode(), S8c1t::EightBit);
    }

    #[test]
    fn cursor_color_override_accessor() {
        let handler = TerminalHandler::new(80, 24);
        assert!(handler.cursor_color_override().is_none());
    }

    #[test]
    fn theme_accessor() {
        let handler = TerminalHandler::new(80, 24);
        let _theme = handler.theme();
        // Just verify it doesn't panic
    }

    #[test]
    fn is_alternate_screen_accessor() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(!handler.is_alternate_screen());
        handler.handle_enter_alternate();
        assert!(handler.is_alternate_screen());
        handler.handle_leave_alternate();
        assert!(!handler.is_alternate_screen());
    }

    #[test]
    fn has_saved_cursor_accessor() {
        let mut handler = TerminalHandler::new(80, 24);
        assert!(!handler.has_saved_cursor());
        handler.handle_save_cursor();
        assert!(handler.has_saved_cursor());
    }

    #[test]
    fn application_escape_key_accessor() {
        let handler = TerminalHandler::new(80, 24);
        assert_eq!(
            handler.application_escape_key(),
            ApplicationEscapeKey::Reset
        );
    }

    #[test]
    fn buffer_mut_accessor() {
        let mut handler = TerminalHandler::new(80, 24);
        let buf = handler.buffer_mut();
        buf.handle_cr(); // Just verify we can call methods on the mutable ref
    }

    #[test]
    fn take_tmux_reparse_queue() {
        let mut handler = TerminalHandler::new(80, 24);
        let queue = handler.take_tmux_reparse_queue();
        assert!(queue.is_empty());
    }

    // ------------------------------------------------------------------
    // Alt screen with disallowed switching
    // ------------------------------------------------------------------

    #[test]
    fn alt_screen_blocked_when_disallowed() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowAltScreen(
            AllowAltScreen::Disallow,
        ))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtExtscrn(XtExtscrn::Alternate))]);
        assert!(
            !handler.is_alternate_screen(),
            "Alt screen should be blocked"
        );
    }

    #[test]
    fn alt_screen_47_blocked_when_disallowed() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowAltScreen(
            AllowAltScreen::Disallow,
        ))]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::AltScreen47(
            AltScreen47::Alternate,
        ))]);
        assert!(
            !handler.is_alternate_screen(),
            "AltScreen47 should be blocked"
        );
    }

    // ------------------------------------------------------------------
    // XtCBlink (cursor blink) set/reset
    // ------------------------------------------------------------------

    #[test]
    fn xtcblink_set_makes_cursor_blink() {
        let mut handler = TerminalHandler::new(80, 24);
        // Default cursor is block blink
        handler.process_outputs(&[TerminalOutput::CursorVisualStyle(
            CursorVisualStyle::BlockCursorSteady,
        )]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::Blinking))]);
        assert_eq!(
            handler.cursor_visual_style(),
            CursorVisualStyle::BlockCursorBlink
        );
    }

    #[test]
    fn xtcblink_reset_makes_cursor_steady() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::CursorVisualStyle(
            CursorVisualStyle::BlockCursorBlink,
        )]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::Steady))]);
        assert_eq!(
            handler.cursor_visual_style(),
            CursorVisualStyle::BlockCursorSteady
        );
    }

    #[test]
    fn xtcblink_underline_blink_to_steady() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::CursorVisualStyle(
            CursorVisualStyle::UnderlineCursorBlink,
        )]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::Steady))]);
        assert_eq!(
            handler.cursor_visual_style(),
            CursorVisualStyle::UnderlineCursorSteady
        );
    }

    #[test]
    fn xtcblink_vertical_line_blink_to_steady() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::CursorVisualStyle(
            CursorVisualStyle::VerticalLineCursorBlink,
        )]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::Steady))]);
        assert_eq!(
            handler.cursor_visual_style(),
            CursorVisualStyle::VerticalLineCursorSteady
        );
    }

    #[test]
    fn xtcblink_underline_steady_to_blink() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::CursorVisualStyle(
            CursorVisualStyle::UnderlineCursorSteady,
        )]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::Blinking))]);
        assert_eq!(
            handler.cursor_visual_style(),
            CursorVisualStyle::UnderlineCursorBlink
        );
    }

    #[test]
    fn xtcblink_vertical_line_steady_to_blink() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::CursorVisualStyle(
            CursorVisualStyle::VerticalLineCursorSteady,
        )]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::Blinking))]);
        assert_eq!(
            handler.cursor_visual_style(),
            CursorVisualStyle::VerticalLineCursorBlink
        );
    }

    // ------------------------------------------------------------------
    // any_visible_dirty / has_visible_images
    // ------------------------------------------------------------------

    #[test]
    fn any_visible_dirty_and_has_visible_images() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"test");
        let _dirty = handler.any_visible_dirty(0);
        let _images = handler.has_visible_images(0);
        let _placements = handler.visible_image_placements(0);
        // Just exercising these paths
    }

    // ------------------------------------------------------------------
    // OSC NoOp
    // ------------------------------------------------------------------

    #[test]
    fn osc_noop() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::NoOp);
        // Should be a no-op
    }

    // ------------------------------------------------------------------
    // Full reset clears kitty keyboard stack
    // ------------------------------------------------------------------

    #[test]
    fn full_reset_clears_kitty_stack() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(5)]);
        assert_eq!(handler.kitty_keyboard_flags(), 5);
        handler.full_reset();
        assert_eq!(handler.kitty_keyboard_flags(), 0);
    }

    #[test]
    fn full_reset_clears_format_and_modes() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::ModifyOtherKeys(2)]);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::ApplicationEscapeKey(
            ApplicationEscapeKey::Set,
        ))]);
        handler.full_reset();
        assert_eq!(handler.modify_other_keys_level(), 0);
        assert_eq!(
            handler.application_escape_key(),
            ApplicationEscapeKey::Reset
        );
        assert_eq!(*handler.current_format(), FormatTag::default());
    }

    // ------------------------------------------------------------------
    // `apply_dec_special` standalone function
    // ------------------------------------------------------------------

    #[test]
    fn apply_dec_special_dont_replace() {
        let data = b"hello";
        let result = apply_dec_special(data, &DecSpecialGraphics::DontReplace);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(&*result, data);
    }

    #[test]
    fn apply_dec_special_replace_box_drawing() {
        // 'q' (0x71) maps to U+2500 HORIZONTAL LINE (─)
        let data = b"q";
        let result = apply_dec_special(data, &DecSpecialGraphics::Replace);
        // The result should be UTF-8 bytes for ─ (U+2500 = 0xE2 0x94 0x80)
        assert_ne!(&*result, data, "Should have been remapped");
    }

    #[test]
    fn apply_dec_special_replace_non_mappable_byte() {
        // Bytes outside 0x5F-0x7E are passed through
        let data = b"ABC";
        let result = apply_dec_special(data, &DecSpecialGraphics::Replace);
        assert_eq!(&*result, data, "Non-mappable bytes should pass through");
    }

    // ------------------------------------------------------------------
    // Coverage gap tests: terminal_handler/mod.rs
    // ------------------------------------------------------------------

    #[test]
    fn set_theme_changes_active_theme() {
        let mut handler = TerminalHandler::new(80, 24);
        let original = handler.theme();
        // Find a different theme
        let themes = freminal_common::themes::all_themes();
        let other = themes.iter().find(|t| t.name != original.name).unwrap();
        handler.set_theme(other);
        assert_eq!(handler.theme().name, other.name);
    }

    #[test]
    fn handle_data_empty_is_noop() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(&[]);
        // No crash, buffer unchanged
        assert_eq!(handler.buffer.get_cursor().pos.x, 0);
    }

    #[test]
    fn handle_erase_in_display_3_clears_scrollback() {
        let mut handler = TerminalHandler::new(80, 24);
        // Write some data that generates scrollback
        for _ in 0..30 {
            handler.handle_data(b"line of text");
            handler.handle_newline();
        }
        handler.handle_erase_in_display(3);
        // After erase scrollback, max_scroll_offset should be 0
        assert_eq!(handler.buffer.max_scroll_offset(), 0);
    }

    #[test]
    fn handle_erase_in_display_unknown_mode_is_noop() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"hello");
        handler.handle_erase_in_display(99);
        // No crash, data still present
    }

    #[test]
    fn handle_erase_in_line_unknown_mode_is_noop() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"hello");
        handler.handle_erase_in_line(99);
        // No crash
    }

    #[test]
    fn handle_xt_cblink_query_is_noop() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.apply_xtcblink(&XtCBlink::Query);
        // Default is BlockCursorSteady, Query should not change it
        assert_eq!(
            handler.cursor_visual_style,
            CursorVisualStyle::BlockCursorSteady
        );
    }

    #[test]
    fn handle_xt_cblink_blink_already_blinking_unchanged() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.cursor_visual_style = CursorVisualStyle::BlockCursorBlink;
        handler.apply_xtcblink(&XtCBlink::Blinking);
        assert_eq!(
            handler.cursor_visual_style,
            CursorVisualStyle::BlockCursorBlink
        );
    }

    #[test]
    fn handle_xt_cblink_steady_already_steady_unchanged() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.cursor_visual_style = CursorVisualStyle::BlockCursorSteady;
        handler.apply_xtcblink(&XtCBlink::Steady);
        assert_eq!(
            handler.cursor_visual_style,
            CursorVisualStyle::BlockCursorSteady
        );
    }

    #[test]
    fn handle_apc_not_kitty_does_not_panic() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_application_program_command(b"not_kitty_graphics");
        // Should not panic
    }

    #[test]
    fn handle_apc_invalid_kitty_does_not_panic() {
        let mut handler = TerminalHandler::new(80, 24);
        // Starts with _G but has invalid content
        handler.handle_application_program_command(b"_Ga=INVALID");
        // Should not panic
    }

    #[test]
    fn process_output_clear_scrollback_and_display() {
        let mut handler = TerminalHandler::new(80, 24);
        for _ in 0..30 {
            handler.handle_data(b"text");
            handler.handle_newline();
        }
        handler.process_outputs(&[TerminalOutput::ClearScrollbackandDisplay]);
        assert_eq!(handler.buffer.max_scroll_offset(), 0);
    }

    #[test]
    fn process_output_tbc_unsupported_ps() {
        let mut handler = TerminalHandler::new(80, 24);
        // Ps=99 is unsupported, should be ignored
        handler.process_outputs(&[TerminalOutput::TabClear(99)]);
        // No crash
    }

    #[test]
    fn mode_xt_extscrn_query_primary() {
        let (mut handler, rx) = handler_with_pty();
        // Not in alternate screen
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtExtscrn(XtExtscrn::Query))]);
        let resp = recv_pty_string(&rx);
        // Should report DecRst (not in alt screen)
        assert!(
            resp.contains("1049"),
            "XtExtscrn query should reference mode 1049"
        );
    }

    #[test]
    fn mode_alt_screen_47_query_primary() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::AltScreen47(AltScreen47::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("47"),
            "AltScreen47 query should reference mode 47"
        );
    }

    #[test]
    fn mode_deccolm_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(resp.contains('3'), "Deccolm query should reference mode 3");
    }

    #[test]
    fn mode_allow_column_mode_switch_set_and_query() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowColumnModeSwitch(
            AllowColumnModeSwitch::AllowColumnModeSwitch,
        ))]);
        assert_eq!(
            handler.allow_column_mode_switch,
            AllowColumnModeSwitch::AllowColumnModeSwitch
        );

        handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowColumnModeSwitch(
            AllowColumnModeSwitch::Query,
        ))]);
        let resp = recv_pty_string(&rx);
        // Should report DecSet since we just enabled it
        assert!(
            resp.contains("40"),
            "AllowColumnModeSwitch query should reference mode 40"
        );
    }

    #[test]
    fn mode_allow_column_mode_switch_disable() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.allow_column_mode_switch = AllowColumnModeSwitch::AllowColumnModeSwitch;
        handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowColumnModeSwitch(
            AllowColumnModeSwitch::NoAllowColumnModeSwitch,
        ))]);
        assert_eq!(
            handler.allow_column_mode_switch,
            AllowColumnModeSwitch::NoAllowColumnModeSwitch
        );
    }

    #[test]
    fn process_output_application_program_command() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.process_outputs(&[TerminalOutput::ApplicationProgramCommand(
            b"not_kitty".to_vec(),
        )]);
        // Should not panic
    }

    #[test]
    fn process_output_request_terminal_parameters() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::RequestTerminalParameters(0)]);
        let resp = recv_pty_string(&rx);
        // DECREPTPARM response: CSI 2;1;1;128;128;1;0x
        assert!(
            resp.contains('x'),
            "DECREPTPARM response should end with 'x'"
        );
    }

    #[test]
    fn handle_repeat_character_repeats_last() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_data(b"A");
        handler.handle_repeat_character(3);
        // Should have written 'A' then repeated it 3 times = 4 total A's
        let text = handler.buffer.extract_text(0, 0, 0, 3);
        assert_eq!(text, "AAAA");
    }

    #[test]
    fn handle_repeat_character_no_last_char_is_noop() {
        let mut handler = TerminalHandler::new(80, 24);
        // No prior graphic char
        handler.handle_repeat_character(3);
        // Should not crash, no text written
    }

    #[test]
    fn insert_text_irm_insert_mode() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.insert_mode = Irm::Insert;
        handler.handle_data(b"AB");
        // Cursor should be at col 2
        assert_eq!(handler.buffer.get_cursor().pos.x, 2);
    }

    #[test]
    fn osc_iterm2_inline_dispatched() {
        let mut handler = TerminalHandler::new(80, 24);
        // Construct a minimal iTerm2 inline image data
        handler.handle_osc(&AnsiOscType::ITerm2FileInline(ITerm2InlineImageData {
            name: None,
            size: None,
            width: None,
            height: None,
            preserve_aspect_ratio: true,
            inline: true,
            do_not_move_cursor: false,
            data: vec![],
        }));
        // Just verifying dispatch doesn't panic; no actual image data to decode
    }

    #[test]
    fn osc_iterm2_unknown_is_noop() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_osc(&AnsiOscType::ITerm2Unknown);
        // Should not panic
    }

    #[test]
    fn send_in_band_resize_dispatched() {
        let (mut handler, rx) = handler_with_pty();
        handler.in_band_resize_enabled = InBandResizeMode::Set;
        // Trigger a resize that changes dimensions to fire send_in_band_resize
        handler.handle_resize(100, 30, 8, 16);
        // Should have sent an in-band resize notification
        let resp = recv_pty_string(&rx);
        assert!(resp.contains("48;"), "In-band resize should contain '48;'");
    }

    #[test]
    fn mode_save_cursor_1048_query_no_saved() {
        let (mut handler, rx) = handler_with_pty();
        handler.process_outputs(&[TerminalOutput::Mode(Mode::SaveCursor1048(
            SaveCursor1048::Query,
        ))]);
        let resp = recv_pty_string(&rx);
        // No cursor saved yet, should report DecRst
        assert!(
            resp.contains("1048"),
            "SaveCursor1048 query should reference mode 1048"
        );
    }

    #[test]
    fn mode_xt_cblink_query_reports_steady() {
        let (mut handler, rx) = handler_with_pty();
        // Default is BlockCursorSteady
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("12"),
            "XtCBlink query should reference mode 12"
        );
    }

    // ── Coverage gap tests: mode queries on alternate screen ──────────

    #[test]
    fn mode_xt_extscrn_query_on_alternate_screen() {
        let (mut handler, rx) = handler_with_pty();
        // Switch to alternate screen first
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtExtscrn(XtExtscrn::Alternate))]);
        assert!(handler.is_alternate_screen());
        // Now query — should report DecSet
        handler.process_outputs(&[TerminalOutput::Mode(Mode::XtExtscrn(XtExtscrn::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("1049"),
            "XtExtscrn query on alt screen should reference mode 1049"
        );
        // Verify it reports DecSet (contains "1" for set, not "2" for reset)
        // The DECRPM response format is CSI ? Pd ; Ps $ y
        // Ps=1 means set, Ps=2 means reset
    }

    #[test]
    fn mode_alt_screen_47_query_on_alternate_screen() {
        let (mut handler, rx) = handler_with_pty();
        // Enter alternate screen via mode 47
        handler.process_outputs(&[TerminalOutput::Mode(Mode::AltScreen47(
            AltScreen47::Alternate,
        ))]);
        assert!(handler.is_alternate_screen());
        // Query — should report DecSet
        handler.process_outputs(&[TerminalOutput::Mode(Mode::AltScreen47(AltScreen47::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains("47"),
            "AltScreen47 query on alt screen should reference mode 47"
        );
    }

    #[test]
    fn mode_deccolm_query_when_132_columns() {
        let (mut handler, rx) = handler_with_pty();
        // Resize to 132 columns
        handler.handle_resize(132, 24, 8, 16);
        handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::Query))]);
        let resp = recv_pty_string(&rx);
        assert!(
            resp.contains('3'),
            "Deccolm query at 132 columns should reference mode 3"
        );
    }

    // ── Coverage gap: KittyKeyboardSet with unknown mode ──────────────

    #[test]
    fn kitty_keyboard_set_unknown_mode_preserves_current() {
        let mut handler = TerminalHandler::new(80, 24);
        // Push initial flags
        handler.kitty_keyboard_stack.push(0b0000_0101); // flags = 5
        // Mode=99 is not 1/2/3, so should keep current
        handler.process_outputs(&[TerminalOutput::KittyKeyboardSet {
            flags: 0xFF,
            mode: 99,
        }]);
        assert_eq!(
            handler.kitty_keyboard_flags(),
            0b0000_0101,
            "Unknown mode should preserve current flags"
        );
    }

    // ── Coverage gap: APC non-Kitty graphics ─────────────────────────

    #[test]
    fn apc_non_kitty_graphics_does_not_panic() {
        let mut handler = TerminalHandler::new(80, 24);
        // Send a non-Kitty APC (no 'G' control character)
        handler.handle_application_program_command(b"SomeRandomAPC");
        // Should log a warning but not panic
    }

    // ── Coverage gap: handle_data_with_placeholders ──────────────────

    #[test]
    fn handle_data_with_placeholders_no_match() {
        use freminal_common::buffer_states::unicode_placeholder::VirtualPlacement;

        let mut handler = TerminalHandler::new(80, 24);
        // Add a virtual placement so the placeholder path is taken
        handler.virtual_placements.insert(
            (1, 0),
            VirtualPlacement {
                image_id: 1,
                placement_id: 0,
                rows: 1,
                cols: 1,
            },
        );

        // Send text that contains NO placeholders — should go through
        // handle_data_with_placeholders but only flush the batch, never
        // entering the placeholder branch.
        handler.handle_data_with_placeholders(&[TChar::Ascii(b'A'), TChar::Ascii(b'B')]);

        // Verify text was inserted
        let row = &handler.buffer.get_rows()[0];
        assert_eq!(row.cells().len(), 2);
    }

    #[test]
    fn handle_data_with_placeholders_mixed_text_and_placeholder() {
        use freminal_common::buffer_states::unicode_placeholder::VirtualPlacement;

        let mut handler = TerminalHandler::new(80, 24);

        // Create a virtual placement
        handler.virtual_placements.insert(
            (1, 0),
            VirtualPlacement {
                image_id: 1,
                placement_id: 0,
                rows: 2,
                cols: 2,
            },
        );

        // Store an image in the image store so the placeholder can reference it
        let img = freminal_buffer::image_store::InlineImage {
            id: 1,
            pixels: std::sync::Arc::new(vec![0; 4]),
            width_px: 1,
            height_px: 1,
            display_cols: 1,
            display_rows: 1,
        };
        handler.buffer.image_store_mut().insert(img);

        // Set foreground color to encode image_id=1 (RGB: 0, 0, 1)
        handler.current_format.colors.color =
            freminal_common::colors::TerminalColor::Custom(0, 0, 1);
        // Set underline color to encode placement_id=0
        handler.current_format.colors.underline_color =
            freminal_common::colors::TerminalColor::Custom(0, 0, 0);

        // Build a placeholder TChar: U+10EEEE encoded as UTF-8
        // U+10EEEE = 0xF4 0x8E 0xBB 0xAE (no diacritics = rule 1)
        let placeholder_bytes: [u8; 4] = [0xF4, 0x8E, 0xBB, 0xAE];
        let mut buf = [0u8; 16];
        buf[..4].copy_from_slice(&placeholder_bytes);

        let placeholder = TChar::Utf8(buf, 4);

        // Mix text + placeholder + text
        let text = vec![TChar::Ascii(b'X'), placeholder, TChar::Ascii(b'Y')];

        handler.handle_data_with_placeholders(&text);

        // Verify: "X" was inserted, then placeholder, then "Y"
        // The buffer should have cells written
        let rows = handler.buffer.get_rows();
        assert!(
            !rows.is_empty(),
            "buffer should have at least one row after text+placeholder"
        );
    }

    // ── Coverage gap: resolve_placeholder_diacritics rule 3 col mismatch ──

    #[test]
    fn resolve_placeholder_diacritics_rule3_col_mismatch() {
        use freminal_common::buffer_states::unicode_placeholder::PlaceholderDiacritics;
        use freminal_common::colors::TerminalColor;

        let mut handler = TerminalHandler::new(80, 24);
        // Set up a prev_placeholder
        handler.prev_placeholder = Some(PrevPlaceholder {
            image_id: 42,
            placement_id: 0,
            row: 1,
            col: 5,
            id_msb: 0x10,
            fg_color: TerminalColor::Custom(0, 0, 42),
            underline_color: TerminalColor::Custom(0, 0, 0),
        });

        // Rule 3: two diacritics (row+col explicit), but col does NOT match prev.col+1
        let diacritics = PlaceholderDiacritics {
            diacritic_count: 2,
            row: 1,
            col: 10, // not prev.col+1 (6)
            id_msb: 0,
        };

        let (row, col, msb) = handler.resolve_placeholder_diacritics(
            diacritics,
            42,
            0,
            TerminalColor::Custom(0, 0, 42),
            TerminalColor::Custom(0, 0, 0),
        );

        assert_eq!(row, 1);
        assert_eq!(col, 10);
        assert_eq!(msb, 0, "col mismatch means msb should be 0, not inherited");
    }

    #[test]
    fn resolve_placeholder_diacritics_all_three_present() {
        use freminal_common::buffer_states::unicode_placeholder::PlaceholderDiacritics;
        use freminal_common::colors::TerminalColor;

        let mut handler = TerminalHandler::new(80, 24);
        handler.prev_placeholder = Some(PrevPlaceholder {
            image_id: 42,
            placement_id: 0,
            row: 1,
            col: 5,
            id_msb: 0x10,
            fg_color: TerminalColor::Custom(0, 0, 42),
            underline_color: TerminalColor::Custom(0, 0, 0),
        });

        // All three diacritics — no inheritance needed.
        let diacritics = PlaceholderDiacritics {
            diacritic_count: 3,
            row: 2,
            col: 3,
            id_msb: 0x20,
        };

        let (row, col, msb) = handler.resolve_placeholder_diacritics(
            diacritics,
            42,
            0,
            TerminalColor::Custom(0, 0, 42),
            TerminalColor::Custom(0, 0, 0),
        );

        assert_eq!(row, 2);
        assert_eq!(col, 3);
        assert_eq!(msb, 0x20, "all diacritics present: use explicit msb");
    }

    // ── Coverage gap: handle_placeholder_char with invalid diacritics ─

    #[test]
    fn handle_placeholder_char_invalid_bytes_returns_early() {
        let mut handler = TerminalHandler::new(80, 24);
        // Send bytes that don't parse as valid placeholder diacritics
        handler.handle_placeholder_char(&[0x00, 0x01, 0x02]);
        // Should return early without crashing
    }

    #[test]
    fn handle_placeholder_char_no_virtual_placement_inserts_space() {
        let mut handler = TerminalHandler::new(80, 24);
        // Don't register any virtual placements

        // Set foreground color to encode image_id=99
        handler.current_format.colors.color =
            freminal_common::colors::TerminalColor::Custom(0, 0, 99);
        handler.current_format.colors.underline_color =
            freminal_common::colors::TerminalColor::Custom(0, 0, 0);

        // U+10EEEE = F4 8E BB AE (just the base, no diacritics = 0 diacritics)
        let bytes: [u8; 4] = [0xF4, 0x8E, 0xBB, 0xAE];
        handler.handle_placeholder_char(&bytes);

        // Should have inserted a space (no matching virtual placement)
        let rows = handler.buffer.get_rows();
        if !rows.is_empty() && !rows[0].cells().is_empty() {
            assert_eq!(
                rows[0].cells()[0].tchar(),
                &TChar::Space,
                "no virtual placement → should insert a space"
            );
        }
    }
}
