// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crossbeam_channel::Sender;
use freminal_common::{
    buffer_states::{
        cursor::{CursorPos, ReverseVideo},
        fonts::{FontDecorations, FontWeight},
        format_tag::FormatTag,
        ftcs::{FtcsMarker, FtcsState},
        kitty_graphics::{
            KittyAction, KittyControlData, KittyGraphicsCommand, KittyParseError,
            format_kitty_response, parse_kitty_graphics,
        },
        line_draw::DecSpecialGraphics,
        mode::{Mode, SetMode},
        modes::ReportMode,
        modes::allow_column_mode_switch::AllowColumnModeSwitch,
        modes::decawm::Decawm,
        modes::deccolm::Deccolm,
        modes::decom::Decom,
        modes::dectcem::Dectcem,
        modes::lnm::Lnm,
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
    sgr::SelectGraphicRendition,
    themes::ThemePalette,
};
use std::borrow::Cow;
use std::collections::HashMap;

use crate::buffer::Buffer;
use crate::image_store::{ImagePlacement, InlineImage, next_image_id};

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

/// High-level handler that processes terminal output commands and applies them to a buffer.
///
/// This is the main entry point for integrating the buffer with a terminal emulator.
/// It receives parsed terminal sequences (via a TerminalOutput-like enum) and updates
/// the buffer state accordingly.
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
    allow_column_mode_switch: bool,
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
            write_tx: None,
            window_commands: Vec::new(),
            last_graphic_char: None,
            current_working_directory: None,
            ftcs_state: FtcsState::default(),
            last_exit_code: None,
            palette: ColorPalette::default(),
            allow_column_mode_switch: true,
            pre_deccolm_width: None,
            theme: &freminal_common::themes::CATPPUCCIN_MOCHA,
            fg_color_override: None,
            bg_color_override: None,
            multipart_state: None,
            kitty_state: None,
            virtual_placements: HashMap::new(),
            prev_placeholder: None,
            cell_pixel_width: 8,
            cell_pixel_height: 16,
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
        self.window_commands.clear();
        self.last_graphic_char = None;
        self.current_working_directory = None;
        self.ftcs_state = FtcsState::default();
        self.last_exit_code = None;
        self.palette.reset_all();
        self.fg_color_override = None;
        self.bg_color_override = None;
        self.allow_column_mode_switch = true;
        self.virtual_placements.clear();
        self.prev_placeholder = None;
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
                self.last_graphic_char = Some(last.clone());
            }
            self.prev_placeholder = None;
            self.buffer.insert_text(&text);
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
            let is_ph = matches!(tch, TChar::Utf8(b) if is_placeholder(b));

            if is_ph {
                // Flush any pending normal text batch.
                if batch_start < i {
                    let batch = &text[batch_start..i];
                    if let Some(last) = batch.last() {
                        self.last_graphic_char = Some(last.clone());
                    }
                    self.prev_placeholder = None;
                    self.buffer.insert_text(batch);
                }
                batch_start = i + 1;

                // Process the placeholder character.
                if let TChar::Utf8(bytes) = tch {
                    self.handle_placeholder_char(bytes);
                }
            }
        }

        // Flush remaining normal text.
        if batch_start < text.len() {
            let batch = &text[batch_start..];
            if let Some(last) = batch.last() {
                self.last_graphic_char = Some(last.clone());
            }
            self.prev_placeholder = None;
            self.buffer.insert_text(batch);
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
            tracing::debug!(
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
            let repeated = vec![ch.clone(); count];
            self.buffer.insert_text(&repeated);
        }
    }

    /// Handle newline (LF)
    pub fn handle_newline(&mut self) {
        self.buffer.handle_lf();
    }

    /// Handle carriage return (CR)
    pub const fn handle_carriage_return(&mut self) {
        self.buffer.handle_cr();
    }

    /// Handle backspace
    pub fn handle_backspace(&mut self) {
        self.buffer.handle_backspace();
    }

    /// Handle horizontal tab (HT / 0x09)
    pub fn handle_tab(&mut self) {
        self.buffer.advance_to_next_tab_stop();
    }

    /// Handle cursor position (CUP, HVP)
    /// x and y are typically 1-indexed from the parser, so we subtract 1
    pub fn handle_cursor_pos(&mut self, x: Option<usize>, y: Option<usize>) {
        let x_zero = x.map(|v| v.saturating_sub(1));
        let y_zero = y.map(|v| v.saturating_sub(1));
        self.buffer.set_cursor_pos(x_zero, y_zero);
    }

    /// Handle relative cursor movement
    pub fn handle_cursor_relative(&mut self, dx: i32, dy: i32) {
        self.buffer.move_cursor_relative(dx, dy);
    }

    /// Handle cursor up (CUU)
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    pub fn handle_cursor_up(&mut self, n: usize) {
        self.buffer.move_cursor_relative(0, -(n as i32));
    }

    /// Handle cursor down (CUD)
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    pub fn handle_cursor_down(&mut self, n: usize) {
        self.buffer.move_cursor_relative(0, n as i32);
    }

    /// Handle cursor forward (CUF)
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    pub fn handle_cursor_forward(&mut self, n: usize) {
        self.buffer.move_cursor_relative(n as i32, 0);
    }

    /// Handle cursor backward (CUB)
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    pub fn handle_cursor_backward(&mut self, n: usize) {
        self.buffer.move_cursor_relative(-(n as i32), 0);
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

    /// Handle insert lines (IL)
    pub fn handle_insert_lines(&mut self, n: usize) {
        self.buffer.insert_lines(n);
    }

    /// Handle delete lines (DL)
    pub fn handle_delete_lines(&mut self, n: usize) {
        self.buffer.delete_lines(n);
    }

    /// Handle erase characters (ECH)
    pub fn handle_erase_chars(&mut self, n: usize) {
        self.buffer.erase_chars(n);
    }

    /// Handle delete characters (DCH)
    pub fn handle_delete_chars(&mut self, n: usize) {
        self.buffer.delete_chars(n);
    }

    /// Handle save cursor (DECSC)
    pub fn handle_save_cursor(&mut self) {
        self.buffer.save_cursor();
    }

    /// Handle restore cursor (DECRC)
    pub fn handle_restore_cursor(&mut self) {
        self.buffer.restore_cursor();
    }

    /// Handle insert spaces (ICH)
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

    /// Handle index (IND)
    pub fn handle_index(&mut self) {
        self.buffer.handle_ind();
    }

    /// Handle reverse index (RI)
    pub fn handle_reverse_index(&mut self) {
        self.buffer.handle_ri();
    }

    /// Handle next line (NEL)
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

    /// Handle SGR (Select Graphic Rendition) — update `current_format` and propagate to buffer.
    pub fn handle_sgr(&mut self, sgr: &SelectGraphicRendition) {
        // Resolve PaletteIndex colors against the mutable palette before applying.
        let resolved = match sgr {
            SelectGraphicRendition::Foreground(TerminalColor::PaletteIndex(idx)) => {
                SelectGraphicRendition::Foreground(
                    self.palette.lookup(usize::from(*idx), self.theme),
                )
            }
            SelectGraphicRendition::Background(TerminalColor::PaletteIndex(idx)) => {
                SelectGraphicRendition::Background(
                    self.palette.lookup(usize::from(*idx), self.theme),
                )
            }
            SelectGraphicRendition::UnderlineColor(TerminalColor::PaletteIndex(idx)) => {
                SelectGraphicRendition::UnderlineColor(
                    self.palette.lookup(usize::from(*idx), self.theme),
                )
            }
            _ => *sgr,
        };
        apply_sgr(&mut self.current_format, &resolved);
        self.buffer.set_format(self.current_format.clone());
    }

    /// Update format tag directly
    pub fn set_format(&mut self, format: FormatTag) {
        self.current_format = format.clone();
        self.buffer.set_format(format);
    }

    /// Handle entering alternate screen
    pub fn handle_enter_alternate(&mut self) {
        // scroll_offset lives in ViewState (Task 4). Pass 0 temporarily;
        // correct wiring happens in Task 7/8.
        self.buffer.enter_alternate(0);
    }

    /// Handle leaving alternate screen
    pub fn handle_leave_alternate(&mut self) {
        // Returns the saved scroll_offset; discarded here until ViewState is wired (Task 7/8).
        let _restored_offset = self.buffer.leave_alternate();
    }

    /// Handle DECAWM — enable or disable soft-wrapping.
    pub const fn handle_set_wrap(&mut self, enabled: bool) {
        self.buffer.set_wrap(enabled);
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
    pub const fn handle_set_lnm(&mut self, enabled: bool) {
        self.buffer.set_lnm(enabled);
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

    /// Send a raw string response to the PTY.  Silently drops if no channel is set.
    fn write_to_pty(&self, text: &str) {
        if let Some(tx) = &self.write_tx
            && let Err(e) = tx.send(PtyWrite::Write(text.as_bytes().to_vec()))
        {
            tracing::error!("Failed to write to PTY: {e}");
        }
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
        self.write_to_pty("\x1b[>65;0;0c");
    }

    /// Handle `RequestDeviceNameAndVersion` — respond with Freminal's name and version.
    /// Responds with `DCS > | Freminal <version> ST`.
    pub fn handle_device_name_and_version(&mut self) {
        let version = env!("CARGO_PKG_VERSION");
        self.write_to_pty(&format!("\x1bP>|Freminal {version}\x1b\\"));
    }

    /// Handle a DCS (Device Control String) sequence.
    ///
    /// The raw `dcs` payload includes the leading `P` byte and the trailing `ESC \`
    /// string terminator.  We strip those to get the inner content, then dispatch on
    /// known DCS sub-commands:
    ///
    /// - **DECRQSS** (`$ q <Pt> ST`): Request Selection or Setting.
    /// - **XTGETTCAP** (`+ q <hex> ST`): xterm termcap/terminfo query.
    ///
    /// Unknown or unsupported DCS sub-commands are logged at debug level and ignored.
    pub fn handle_device_control_string(&mut self, dcs: &[u8]) {
        // Strip leading 'P' and trailing ESC '\' to get inner content.
        let inner = Self::strip_dcs_envelope(dcs);

        if let Some(pt) = inner.strip_prefix(b"$q") {
            self.handle_decrqss(pt);
        } else if let Some(hex_payload) = inner.strip_prefix(b"+q") {
            self.handle_xtgettcap(hex_payload);
        } else if Self::is_sixel_sequence(inner) {
            self.handle_sixel(inner);
        } else {
            tracing::debug!(
                "DCS sub-command not recognized (ignored): {}",
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

    /// Check whether a DCS inner payload is a Sixel sequence.
    ///
    /// The format is `<optional digits and semicolons>q<sixel data>`.
    /// This returns `true` when the data up to the first `q` consists only
    /// of digits and semicolons (the P1;P2;P3 parameters).
    fn is_sixel_sequence(inner: &[u8]) -> bool {
        let Some(q_pos) = inner.iter().position(|&b| b == b'q') else {
            return false;
        };
        // Everything before `q` must be digits or semicolons (DCS params).
        inner[..q_pos]
            .iter()
            .all(|&b| b.is_ascii_digit() || b == b';')
    }

    /// Handle a Sixel graphics DCS sequence.
    ///
    /// `inner` is the stripped DCS payload: `<P1;P2;P3>q<sixel-data>`.
    fn handle_sixel(&mut self, inner: &[u8]) {
        use freminal_common::buffer_states::sixel::parse_sixel;

        let Some(sixel_image) = parse_sixel(inner) else {
            tracing::debug!("Sixel: failed to decode image from DCS payload");
            return;
        };

        if sixel_image.width == 0 || sixel_image.height == 0 {
            tracing::debug!("Sixel: decoded image has zero dimensions");
            return;
        }

        let (term_width, term_height) = self.get_win_size();

        // Compute display size in terminal cells using actual cell pixel dimensions.
        #[allow(clippy::cast_possible_truncation)]
        let display_cols = {
            let cols = sixel_image.width.div_ceil(self.cell_pixel_width) as usize;
            cols.min(term_width).max(1)
        };
        #[allow(clippy::cast_possible_truncation)]
        let display_rows = {
            let rows = sixel_image.height.div_ceil(self.cell_pixel_height) as usize;
            rows.min(term_height).max(1)
        };

        let image_id = next_image_id();

        let inline_image = InlineImage {
            id: image_id,
            pixels: std::sync::Arc::new(sixel_image.pixels),
            width_px: sixel_image.width,
            height_px: sixel_image.height,
            display_cols,
            display_rows,
        };

        let _new_offset = self.buffer.place_image(inline_image, 0);
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
                self.write_to_pty(&format!("\x1bP1$r{sgr}m\x1b\\"));
            }
            b"r" => {
                let (top, bottom) = self.buffer.scroll_region();
                // Respond with 1-based row numbers.
                let top_1 = top + 1;
                let bottom_1 = bottom + 1;
                self.write_to_pty(&format!("\x1bP1$r{top_1};{bottom_1}r\x1b\\"));
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
                self.write_to_pty(&format!("\x1bP1$r{style_num} q\x1b\\"));
            }
            _ => {
                // Invalid / unrecognized query → DCS 0 $ r ST
                self.write_to_pty("\x1bP0$r\x1b\\");
                tracing::debug!(
                    "DECRQSS: unrecognized setting query: {}",
                    String::from_utf8_lossy(pt)
                );
            }
        }
    }

    /// Build the SGR parameter string for the current format state.
    ///
    /// Returns a string like `0;1;4;38;2;255;0;0` representing the active SGR
    /// attributes.  The leading `0` (reset) is always included; individual
    /// attributes are appended only when they differ from the default.
    fn build_sgr_response(&self) -> String {
        let fmt = self.current_format();
        let mut parts: Vec<String> = vec!["0".to_string()];

        // Font weight
        if fmt.font_weight == FontWeight::Bold {
            parts.push("1".to_string());
        }

        // Font decorations
        for dec in &fmt.font_decorations {
            match dec {
                FontDecorations::Faint => parts.push("2".to_string()),
                FontDecorations::Italic => parts.push("3".to_string()),
                FontDecorations::Underline => parts.push("4".to_string()),
                FontDecorations::Strikethrough => parts.push("9".to_string()),
            }
        }

        // Reverse video
        if fmt.colors.reverse_video == ReverseVideo::On {
            parts.push("7".to_string());
        }

        // Foreground color
        Self::append_color_sgr(&mut parts, fmt.colors.color, true);

        // Background color
        Self::append_color_sgr(&mut parts, fmt.colors.background_color, false);

        // Underline color (SGR 58)
        if fmt.colors.underline_color != TerminalColor::DefaultUnderlineColor {
            Self::append_underline_color_sgr(&mut parts, fmt.colors.underline_color);
        }

        parts.join(";")
    }

    /// Append SGR parameters for a foreground (`is_fg = true`) or background color.
    fn append_color_sgr(parts: &mut Vec<String>, color: TerminalColor, is_fg: bool) {
        let (base, idx_code, rgb_code) = if is_fg { (30, 38, 38) } else { (40, 48, 48) };

        match color {
            TerminalColor::Black => parts.push(format!("{base}")),
            TerminalColor::Red => parts.push(format!("{}", base + 1)),
            TerminalColor::Green => parts.push(format!("{}", base + 2)),
            TerminalColor::Yellow => parts.push(format!("{}", base + 3)),
            TerminalColor::Blue => parts.push(format!("{}", base + 4)),
            TerminalColor::Magenta => parts.push(format!("{}", base + 5)),
            TerminalColor::Cyan => parts.push(format!("{}", base + 6)),
            TerminalColor::White => parts.push(format!("{}", base + 7)),
            TerminalColor::BrightBlack => parts.push(format!("{}", base + 60)),
            TerminalColor::BrightRed => parts.push(format!("{}", base + 61)),
            TerminalColor::BrightGreen => parts.push(format!("{}", base + 62)),
            TerminalColor::BrightYellow => parts.push(format!("{}", base + 63)),
            TerminalColor::BrightBlue => parts.push(format!("{}", base + 64)),
            TerminalColor::BrightMagenta => parts.push(format!("{}", base + 65)),
            TerminalColor::BrightCyan => parts.push(format!("{}", base + 66)),
            TerminalColor::BrightWhite => parts.push(format!("{}", base + 67)),
            TerminalColor::PaletteIndex(idx) => {
                parts.push(format!("{idx_code};5;{idx}"));
            }
            TerminalColor::Custom(r, g, b) => {
                parts.push(format!("{rgb_code};2;{r};{g};{b}"));
            }
            // Default, DefaultBackground, DefaultUnderlineColor, DefaultCursorColor — no SGR needed
            _ => {}
        }
    }

    /// Append SGR 58 (underline color) parameters.
    fn append_underline_color_sgr(parts: &mut Vec<String>, color: TerminalColor) {
        match color {
            TerminalColor::PaletteIndex(idx) => {
                parts.push(format!("58;5;{idx}"));
            }
            TerminalColor::Custom(r, g, b) => {
                parts.push(format!("58;2;{r};{g};{b}"));
            }
            // Named colors as underline color: encode as palette index 0-15
            TerminalColor::Black => parts.push("58;5;0".to_string()),
            TerminalColor::Red => parts.push("58;5;1".to_string()),
            TerminalColor::Green => parts.push("58;5;2".to_string()),
            TerminalColor::Yellow => parts.push("58;5;3".to_string()),
            TerminalColor::Blue => parts.push("58;5;4".to_string()),
            TerminalColor::Magenta => parts.push("58;5;5".to_string()),
            TerminalColor::Cyan => parts.push("58;5;6".to_string()),
            TerminalColor::White => parts.push("58;5;7".to_string()),
            TerminalColor::BrightBlack => parts.push("58;5;8".to_string()),
            TerminalColor::BrightRed => parts.push("58;5;9".to_string()),
            TerminalColor::BrightGreen => parts.push("58;5;10".to_string()),
            TerminalColor::BrightYellow => parts.push("58;5;11".to_string()),
            TerminalColor::BrightBlue => parts.push("58;5;12".to_string()),
            TerminalColor::BrightMagenta => parts.push("58;5;13".to_string()),
            TerminalColor::BrightCyan => parts.push("58;5;14".to_string()),
            TerminalColor::BrightWhite => parts.push("58;5;15".to_string()),
            _ => {}
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
        let payload_str = String::from_utf8_lossy(hex_payload);

        // Split on ';' to support multiple capability queries in a single DCS.
        for hex_name in payload_str.split(';') {
            if hex_name.is_empty() {
                continue;
            }

            let Some(cap_name) = Self::hex_decode(hex_name) else {
                tracing::debug!("XTGETTCAP: invalid hex encoding: {hex_name}");
                self.write_to_pty(&format!("\x1bP0+r{hex_name}\x1b\\"));
                continue;
            };

            if let Some(value) = Self::lookup_termcap(&cap_name) {
                let hex_value = Self::hex_encode(value);
                self.write_to_pty(&format!("\x1bP1+r{hex_name}={hex_value}\x1b\\"));
            } else {
                tracing::debug!("XTGETTCAP: unknown capability: {cap_name}");
                self.write_to_pty(&format!("\x1bP0+r{hex_name}\x1b\\"));
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
                tracing::debug!(
                    "APC received (not Kitty graphics, ignored): {}",
                    String::from_utf8_lossy(apc)
                );
            }
            Err(e) => {
                tracing::debug!("Kitty graphics parse error: {e}");
            }
        }
    }

    /// Dispatch a parsed Kitty graphics command.
    fn handle_kitty_graphics(&mut self, cmd: KittyGraphicsCommand) {
        let action = cmd.control.action.unwrap_or(KittyAction::Transmit);

        // If this is a continuation chunk for an in-progress chunked transfer,
        // append the payload regardless of the action field.
        if self.kitty_state.is_some() && action != KittyAction::Query {
            self.handle_kitty_chunk(&cmd);
            return;
        }

        match action {
            KittyAction::Query => self.handle_kitty_query(&cmd),
            KittyAction::Transmit | KittyAction::TransmitAndDisplay | KittyAction::Put => {
                if cmd.control.more_data {
                    // First chunk of a chunked transfer — start accumulating.
                    self.handle_kitty_chunk_start(cmd);
                } else {
                    // Single-chunk command (or final chunk with no prior state).
                    self.handle_kitty_single(&cmd, action);
                }
            }
            KittyAction::Delete => {
                self.handle_kitty_delete(&cmd);
            }
            KittyAction::AnimationFrame
            | KittyAction::AnimationControl
            | KittyAction::AnimationCompose => {
                tracing::debug!(
                    "Kitty graphics: animation commands not yet supported (a={action:?})"
                );
            }
        }
    }

    /// Handle `a=q` — query whether the terminal supports the Kitty graphics protocol.
    ///
    /// Responds with OK for formats 24 (RGB), 32 (RGBA), and 100 (PNG).
    /// Other formats get an error response.
    fn handle_kitty_query(&self, cmd: &KittyGraphicsCommand) {
        let image_id = cmd.control.image_id.unwrap_or(0);
        let quiet = cmd.control.quiet;

        // We support RGB (f=24), RGBA (f=32), and PNG (f=100).
        let supported = cmd.control.format.is_none_or(|f| {
            matches!(
                f,
                freminal_common::buffer_states::kitty_graphics::KittyFormat::Rgb
                    | freminal_common::buffer_states::kitty_graphics::KittyFormat::Rgba
                    | freminal_common::buffer_states::kitty_graphics::KittyFormat::Png
            )
        });

        // quiet=1 suppresses OK responses; quiet=2 suppresses all responses.
        if quiet >= 2 || (quiet >= 1 && supported) {
            return;
        }

        let response = if supported {
            format_kitty_response(image_id, true, "")
        } else {
            format_kitty_response(image_id, false, "ENOTSUP:unsupported format")
        };

        self.write_to_pty(&response);
    }

    /// Start a chunked Kitty graphics transfer (first chunk, `m=1`).
    fn handle_kitty_chunk_start(&mut self, cmd: KittyGraphicsCommand) {
        if self.kitty_state.is_some() {
            tracing::warn!(
                "Kitty graphics: new chunked transfer started while previous was in progress; \
                 discarding incomplete transfer"
            );
        }

        let capacity = cmd.control.data_size.unwrap_or(0) as usize;
        let mut accumulated_data = Vec::with_capacity(capacity);
        accumulated_data.extend_from_slice(&cmd.payload);

        self.kitty_state = Some(KittyImageState {
            control: cmd.control,
            accumulated_data,
        });

        tracing::debug!(
            "Kitty graphics: started chunked transfer (id={:?})",
            self.kitty_state.as_ref().map(|s| s.control.image_id),
        );
    }

    /// Append a chunk to the in-progress Kitty graphics transfer.
    ///
    /// If `m=0` (final chunk), finalise the transfer and dispatch.
    fn handle_kitty_chunk(&mut self, cmd: &KittyGraphicsCommand) {
        let Some(state) = &mut self.kitty_state else {
            tracing::warn!("Kitty graphics: chunk received with no active transfer; ignoring");
            return;
        };

        state.accumulated_data.extend_from_slice(&cmd.payload);

        if cmd.control.more_data {
            // More chunks to come.
            tracing::debug!(
                "Kitty graphics: appended chunk ({} bytes total so far)",
                state.accumulated_data.len(),
            );
        } else {
            // Final chunk — take ownership and dispatch.
            let final_state = self.kitty_state.take().unwrap_or_else(|| {
                // This branch is unreachable because we checked `is_some` above,
                // but we need to avoid `unwrap()` in production code.
                KittyImageState {
                    control: KittyControlData::default(),
                    accumulated_data: Vec::new(),
                }
            });

            tracing::debug!(
                "Kitty graphics: chunked transfer complete ({} bytes)",
                final_state.accumulated_data.len(),
            );

            let action = final_state.control.action.unwrap_or(KittyAction::Transmit);
            let final_cmd = KittyGraphicsCommand {
                control: final_state.control,
                payload: final_state.accumulated_data,
            };
            self.handle_kitty_single(&final_cmd, action);
        }
    }

    /// Handle a single (non-chunked) Kitty graphics command.
    ///
    /// Decodes the image payload according to the format (`f=24` RGB, `f=32`
    /// RGBA, `f=100` PNG), computes display dimensions, stores as an
    /// `InlineImage`, and places into the buffer.
    fn handle_kitty_single(&mut self, cmd: &KittyGraphicsCommand, action: KittyAction) {
        let image_id_hint = cmd.control.image_id.unwrap_or(0);
        let quiet = cmd.control.quiet;

        if cmd.payload.is_empty() && action != KittyAction::Put {
            tracing::debug!("Kitty graphics: empty payload for a={action:?}; ignoring");
            self.send_kitty_error(image_id_hint, quiet, "ENODATA:no payload");
            return;
        }

        // Decode payload into RGBA pixels + dimensions.
        let Some((rgba_pixels, img_width_px, img_height_px)) =
            self.decode_kitty_payload(cmd, image_id_hint, quiet)
        else {
            return; // Error already sent by decode_kitty_payload.
        };

        // Compute display size in cells and place the image.
        self.place_kitty_image(
            &cmd.control,
            action,
            rgba_pixels,
            img_width_px,
            img_height_px,
            image_id_hint,
            quiet,
        );
    }

    /// Decode a Kitty graphics payload into RGBA pixel data.
    ///
    /// Returns `None` if decoding fails (an error response is sent to the PTY).
    fn decode_kitty_payload(
        &self,
        cmd: &KittyGraphicsCommand,
        image_id_hint: u32,
        quiet: u8,
    ) -> Option<(Vec<u8>, u32, u32)> {
        use freminal_common::buffer_states::kitty_graphics::KittyFormat;

        let format = cmd.control.format.unwrap_or(KittyFormat::Rgba);

        match format {
            KittyFormat::Rgba => {
                let (w, h) = self.require_kitty_dimensions(cmd, image_id_hint, quiet)?;
                let expected = (w as usize) * (h as usize) * 4;
                if cmd.payload.len() != expected {
                    tracing::debug!(
                        "Kitty RGBA: expected {expected} bytes, got {}",
                        cmd.payload.len()
                    );
                    self.send_kitty_error(image_id_hint, quiet, "EINVAL:payload size mismatch");
                    return None;
                }
                Some((cmd.payload.clone(), w, h))
            }
            KittyFormat::Rgb => {
                let (w, h) = self.require_kitty_dimensions(cmd, image_id_hint, quiet)?;
                let expected = (w as usize) * (h as usize) * 3;
                if cmd.payload.len() != expected {
                    tracing::debug!(
                        "Kitty RGB: expected {expected} bytes, got {}",
                        cmd.payload.len()
                    );
                    self.send_kitty_error(image_id_hint, quiet, "EINVAL:payload size mismatch");
                    return None;
                }
                let pixel_count = (w as usize) * (h as usize);
                let mut rgba = Vec::with_capacity(pixel_count * 4);
                for chunk in cmd.payload.chunks_exact(3) {
                    rgba.extend_from_slice(chunk);
                    rgba.push(255);
                }
                Some((rgba, w, h))
            }
            KittyFormat::Png => match image::load_from_memory(&cmd.payload) {
                Ok(img) => {
                    let rgba_img = img.to_rgba8();
                    let w = rgba_img.width();
                    let h = rgba_img.height();
                    if w == 0 || h == 0 {
                        self.send_kitty_error(
                            image_id_hint,
                            quiet,
                            "EINVAL:decoded image has zero dimensions",
                        );
                        return None;
                    }
                    Some((rgba_img.into_raw(), w, h))
                }
                Err(e) => {
                    tracing::debug!("Kitty PNG decode failed: {e}");
                    self.send_kitty_error(image_id_hint, quiet, "EINVAL:PNG decode failed");
                    None
                }
            },
        }
    }

    /// Extract required `s` (width) and `v` (height) from Kitty control data.
    ///
    /// Returns `None` and sends an error if either is missing.
    fn require_kitty_dimensions(
        &self,
        cmd: &KittyGraphicsCommand,
        image_id_hint: u32,
        quiet: u8,
    ) -> Option<(u32, u32)> {
        let Some(w) = cmd.control.src_width else {
            self.send_kitty_error(image_id_hint, quiet, "EINVAL:missing width (s)");
            return None;
        };
        let Some(h) = cmd.control.src_height else {
            self.send_kitty_error(image_id_hint, quiet, "EINVAL:missing height (v)");
            return None;
        };
        Some((w, h))
    }

    /// Compute display dimensions, store an `InlineImage`, and optionally place
    /// it into the buffer.
    #[allow(clippy::too_many_arguments)]
    fn place_kitty_image(
        &mut self,
        control: &KittyControlData,
        action: KittyAction,
        rgba_pixels: Vec<u8>,
        img_width_px: u32,
        img_height_px: u32,
        image_id_hint: u32,
        quiet: u8,
    ) {
        if img_width_px == 0 || img_height_px == 0 {
            self.send_kitty_error(image_id_hint, quiet, "EINVAL:zero dimension");
            return;
        }

        let (term_width, term_height) = self.get_win_size();

        let display_cols = control.display_cols.map_or_else(
            || {
                #[allow(clippy::cast_possible_truncation)]
                let cols = img_width_px.div_ceil(self.cell_pixel_width) as usize;
                cols.min(term_width).max(1)
            },
            |c| {
                #[allow(clippy::cast_possible_truncation)]
                let cols = c as usize;
                cols.min(term_width).max(1)
            },
        );

        let display_rows = control.display_rows.map_or_else(
            || {
                #[allow(clippy::cast_possible_truncation)]
                let rows = img_height_px.div_ceil(self.cell_pixel_height) as usize;
                rows.min(term_height).max(1)
            },
            |r| {
                #[allow(clippy::cast_possible_truncation)]
                let rows = r as usize;
                rows.min(term_height).max(1)
            },
        );

        let assigned_id = if image_id_hint > 0 {
            u64::from(image_id_hint)
        } else {
            next_image_id()
        };

        let inline_image = InlineImage {
            id: assigned_id,
            pixels: std::sync::Arc::new(rgba_pixels),
            width_px: img_width_px,
            height_px: img_height_px,
            display_cols,
            display_rows,
        };

        self.buffer.image_store_mut().insert(inline_image.clone());

        // If this is a virtual (Unicode placeholder) placement, store it in
        // the virtual_placements table instead of placing cells in the buffer.
        if control.unicode_placeholder {
            let pid = control.placement_id.unwrap_or(0);
            let vp = VirtualPlacement {
                image_id: assigned_id,
                placement_id: pid,
                cols: u32::try_from(display_cols).unwrap_or(u32::MAX),
                rows: u32::try_from(display_rows).unwrap_or(u32::MAX),
            };
            self.virtual_placements.insert((assigned_id, pid), vp);
        } else {
            let should_display =
                matches!(action, KittyAction::TransmitAndDisplay | KittyAction::Put);
            if should_display {
                let _new_offset = self.buffer.place_image(inline_image, 0);
            }
        }

        // Send OK response unless suppressed.
        if quiet < 1 && assigned_id > 0 {
            #[allow(clippy::cast_possible_truncation)]
            let response_id = assigned_id as u32;
            let response = format_kitty_response(response_id, true, "");
            self.write_to_pty(&response);
        }
    }

    /// Send a Kitty graphics error response, respecting quiet mode.
    fn send_kitty_error(&self, image_id: u32, quiet: u8, message: &str) {
        // quiet=2 suppresses all responses (including errors).
        if quiet >= 2 {
            return;
        }
        let response = format_kitty_response(image_id, false, message);
        self.write_to_pty(&response);
    }

    /// Handle `a=d` — delete images.
    ///
    /// Supports the most common delete targets: delete all (`d=a`/`d=A`),
    /// delete by ID (`d=i`/`d=I`), and delete at cursor (`d=c`/`d=C`).
    /// Unsupported targets are logged and ignored.
    fn handle_kitty_delete(&mut self, cmd: &KittyGraphicsCommand) {
        use freminal_common::buffer_states::kitty_graphics::KittyDeleteTarget;

        let target = cmd.control.delete_target.unwrap_or(KittyDeleteTarget::All);

        match target {
            KittyDeleteTarget::All | KittyDeleteTarget::AllIncludingNonVisible => {
                tracing::debug!("Kitty graphics: deleting all images");
                // Clear all image placements from cells.
                self.buffer.clear_all_image_placements();
                // Clear the image store.
                self.buffer.image_store_mut().clear();
                // Clear all virtual placements.
                self.virtual_placements.clear();
            }
            KittyDeleteTarget::ById | KittyDeleteTarget::ByIdCursorOrAfter => {
                if let Some(image_id) = cmd.control.image_id {
                    let id = u64::from(image_id);
                    tracing::debug!("Kitty graphics: deleting image id={id}");
                    self.buffer.clear_image_placements_by_id(id);
                    self.buffer.image_store_mut().remove(id);
                    // Remove virtual placements with matching image ID.
                    self.virtual_placements
                        .retain(|&(img_id, _), _| img_id != id);
                }
            }
            other => {
                tracing::debug!(
                    "Kitty graphics: delete target {other:?} not yet implemented; ignoring"
                );
            }
        }
    }

    /// Handle CPR — Cursor Position Report.
    /// Responds with `ESC [ <row> ; <col> R` (1-indexed).
    pub fn handle_cursor_report(&mut self) {
        let pos = self.buffer.get_cursor().pos;
        let x = pos.x + 1;
        let y = pos.y + 1;
        self.write_to_pty(&format!("\x1b[{y};{x}R"));
    }

    /// Handle DSR — Device Status Report (Ps=5).
    /// Responds with `ESC [ 0 n` (device OK).
    pub fn handle_device_status_report(&mut self) {
        self.write_to_pty("\x1b[0n");
    }

    /// Handle DSR ?996 — Color Theme Report.
    /// Responds with `ESC [ ? 997 ; Ps n` where Ps = 1 (light) or 2 (dark).
    /// Freminal's default background is dark (#45475a), so we report dark (2).
    pub fn handle_color_theme_report(&mut self) {
        // 1 = light, 2 = dark
        self.write_to_pty("\x1b[?997;2n");
    }

    /// Handle DA1 — Primary Device Attributes.
    /// Responds with the capability string used by the old buffer (iTerm2 DA set).
    pub fn handle_request_device_attributes(&mut self) {
        self.write_to_pty("\x1b[?65;1;2;4;6;17;18;22c");
    }

    /// Handle an OSC (Operating System Command) sequence.
    ///
    /// Ports the logic from `TerminalState::osc_response` in the old buffer.
    pub fn handle_osc(&mut self, osc: &AnsiOscType) {
        match osc {
            // Hyperlink: OSC 8 ; params ; url ST  (start) / OSC 8 ; ; ST  (end)
            AnsiOscType::Url(UrlResponse::Url(url)) => {
                self.current_format.url = Some(Url {
                    id: url.id.clone(),
                    url: url.url.clone(),
                });
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

            // OSC 10/11 foreground/background color query, set, and reset.
            AnsiOscType::RequestColorQueryBackground(_)
            | AnsiOscType::RequestColorQueryForeground(_)
            | AnsiOscType::ResetForegroundColor
            | AnsiOscType::ResetBackgroundColor => {
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
                tracing::debug!("OSC 1337: unrecognised sub-command (ignored)");
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
                let response = format!(
                    "\x1b]4;{idx};rgb:{:04x}/{:04x}/{:04x}\x1b\\",
                    u16::from(r) * 257,
                    u16::from(g) * 257,
                    u16::from(b) * 257,
                );
                self.write_to_pty(&response);
            }
            AnsiOscType::ResetPaletteColor(Some(idx)) => {
                self.palette.reset(*idx);
            }
            AnsiOscType::ResetPaletteColor(None) => {
                self.palette.reset_all();
            }

            AnsiOscType::NoOp | AnsiOscType::ResetCursorColor => {}
        }
    }

    /// Handle OSC 10/11 foreground/background color query, set, and reset.
    ///
    /// Extracted from `handle_osc` to keep that function within the 100-line clippy limit.
    ///
    /// - `RequestColorQueryBackground(Query)` / `RequestColorQueryForeground(Query)`:
    ///   respond with the effective color (override or theme default).
    /// - `RequestColorQueryBackground(String(spec))` / `RequestColorQueryForeground(String(spec))`:
    ///   parse the X11 color spec and store as an override.
    /// - `ResetForegroundColor` / `ResetBackgroundColor`:
    ///   clear the corresponding override so subsequent queries return the theme color.
    fn handle_osc_fg_bg_color(&mut self, osc: &AnsiOscType) {
        match osc {
            // OSC 11 query: respond with the effective background color.
            AnsiOscType::RequestColorQueryBackground(AnsiOscInternalType::Query) => {
                let (r, g, b) = self.bg_color_override.unwrap_or(self.theme.background);
                self.write_to_pty(&format!("\x1b]11;rgb:{r:02x}/{g:02x}/{b:02x}\x1b\\"));
            }
            // OSC 10 query: respond with the effective foreground color.
            AnsiOscType::RequestColorQueryForeground(AnsiOscInternalType::Query) => {
                let (r, g, b) = self.fg_color_override.unwrap_or(self.theme.foreground);
                self.write_to_pty(&format!("\x1b]10;rgb:{r:02x}/{g:02x}/{b:02x}\x1b\\"));
            }
            // OSC 11 set: store a dynamic background color override.
            AnsiOscType::RequestColorQueryBackground(AnsiOscInternalType::String(spec)) => {
                if let Some(rgb) = parse_color_spec(spec) {
                    self.bg_color_override = Some(rgb);
                } else {
                    tracing::debug!("OSC 11: unrecognised color spec: {spec:?}");
                }
            }
            // OSC 10 set: store a dynamic foreground color override.
            AnsiOscType::RequestColorQueryForeground(AnsiOscInternalType::String(spec)) => {
                if let Some(rgb) = parse_color_spec(spec) {
                    self.fg_color_override = Some(rgb);
                } else {
                    tracing::debug!("OSC 10: unrecognised color spec: {spec:?}");
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
                tracing::debug!("OSC 1337 File=: image decode failed: {e}");
                return;
            }
        };

        let img_width_px = img.width();
        let img_height_px = img.height();

        if img_width_px == 0 || img_height_px == 0 {
            tracing::debug!("OSC 1337 File=: decoded image has zero dimensions");
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
            )
        } else {
            (display_cols, display_rows)
        };

        if display_cols == 0 || display_rows == 0 {
            tracing::debug!("OSC 1337 File=: computed display size is 0x0");
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

        // Place the image into the buffer. Pass 0 for scroll_offset — the
        // PTY thread always operates at the live bottom.
        let _new_offset = self.buffer.place_image(inline_image, 0);
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
            tracing::debug!("OSC 1337 FileEnd: transfer completed with empty payload; ignoring");
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
                #[allow(clippy::cast_possible_truncation)]
                let result = (cells as usize).min(term_cells).max(1);
                result
            }
            Some(ImageDimension::Cells(n)) => {
                #[allow(clippy::cast_possible_truncation)]
                let result = (*n as usize).min(term_cells).max(1);
                result
            }
            Some(ImageDimension::Pixels(px)) => {
                let cells = px.saturating_add(cell_pixels - 1) / cell_pixels;
                #[allow(clippy::cast_possible_truncation)]
                let result = (cells as usize).min(term_cells).max(1);
                result
            }
            Some(ImageDimension::Percent(pct)) => {
                let cells = (u64::from(*pct) * term_cells as u64) / 100;
                // Safe: cells is bounded by term_cells which fits in usize.
                #[allow(clippy::cast_possible_truncation)]
                let result = (cells as usize).min(term_cells).max(1);
                result
            }
        }
    }

    /// Adjust display dimensions to preserve aspect ratio.
    ///
    /// When `preserve_aspect_ratio` is true and one dimension is auto/unspecified,
    /// scale the auto dimension to match the other.
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
    ) -> (usize, usize) {
        let width_is_auto = matches!(width_spec, None | Some(ImageDimension::Auto));
        let height_is_auto = matches!(height_spec, None | Some(ImageDimension::Auto));

        if width_is_auto && !height_is_auto {
            // Height was explicitly set; scale width to preserve aspect ratio.
            // Approximate: each cell is ~8px wide, ~16px tall (2:1 aspect).
            // scaled_cols = display_rows * (img_w / img_h) * (cell_h / cell_w)
            //             = display_rows * img_w * 16 / (img_h * 8)
            //             = display_rows * img_w * 2 / img_h
            let scaled = (u64::from(img_width_px) * display_rows as u64 * 2)
                / u64::from(img_height_px).max(1);
            // Safe: scaled is bounded by term_width which fits in usize.
            #[allow(clippy::cast_possible_truncation)]
            let cols = (scaled as usize).min(term_width).max(1);
            (cols, display_rows)
        } else if !width_is_auto && height_is_auto {
            // Width was explicitly set; scale height to preserve aspect ratio.
            // scaled_rows = display_cols * (img_h / img_w) * (cell_w / cell_h)
            //             = display_cols * img_h / (img_w * 2)
            let scaled = (u64::from(img_height_px) * display_cols as u64)
                / (u64::from(img_width_px).max(1) * 2);
            // Safe: scaled is bounded by term_height which fits in usize.
            #[allow(clippy::cast_possible_truncation)]
            let rows = (scaled as usize).min(term_height).max(1);
            (display_cols, rows)
        } else {
            // Both auto or both explicit — no adjustment needed.
            (display_cols, display_rows)
        }
    }

    /// Handle resize
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
        // scroll_offset lives in `ViewState` (Task 4). Pass 0 temporarily;
        // correct wiring happens in Task 7/8.
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

            // === Unimplemented Operations - TODO ===
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
                3 => self.buffer.clear_all_tab_stops(),
                _ => {
                    tracing::debug!("TBC with unsupported Ps={ps} (ignored)");
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
                | Mode::AltScreen47(AltScreen47::Alternate) => self.handle_enter_alternate(),
                Mode::XtExtscrn(XtExtscrn::Primary) | Mode::AltScreen47(AltScreen47::Primary) => {
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
                    let mode = if self.buffer.is_wrap_enabled() {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&Decawm::AutoWrap.report(Some(mode)));
                }
                Mode::LineFeedMode(Lnm::Query) => {
                    let mode = if self.buffer.is_lnm_enabled() {
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
                    tracing::debug!("DECRQM: unknown mode ?{digits}, responding not recognized");
                    self.write_to_pty(&format!("\x1b[?{digits};0$y"));
                }
                Mode::Decawm(Decawm::AutoWrap) => self.handle_set_wrap(true),
                Mode::Decawm(Decawm::NoAutoWrap) => self.handle_set_wrap(false),
                Mode::LineFeedMode(Lnm::NewLine) => self.handle_set_lnm(true),
                Mode::LineFeedMode(Lnm::LineFeed) => self.handle_set_lnm(false),
                Mode::Dectem(Dectcem::Show) => self.show_cursor = Dectcem::Show,
                Mode::Dectem(Dectcem::Hide) => self.show_cursor = Dectcem::Hide,
                Mode::XtCBlink(blink) => self.apply_xtcblink(blink),
                Mode::Decom(Decom::OriginMode) => self.buffer.set_decom(true),
                Mode::Decom(Decom::NormalCursor) => self.buffer.set_decom(false),
                Mode::Decom(Decom::Query) => {
                    let mode = if self.buffer.is_decom_enabled() {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(&Decom::OriginMode.report(Some(mode)));
                }
                Mode::Deccolm(Deccolm::Column132) => {
                    if self.allow_column_mode_switch {
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
                    if self.allow_column_mode_switch {
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
                Mode::AllowColumnModeSwitch(AllowColumnModeSwitch::AllowColumnModeSwitch) => {
                    self.allow_column_mode_switch = true;
                }
                Mode::AllowColumnModeSwitch(AllowColumnModeSwitch::NoAllowColumnModeSwitch) => {
                    self.allow_column_mode_switch = false;
                }
                Mode::AllowColumnModeSwitch(AllowColumnModeSwitch::Query) => {
                    let mode = if self.allow_column_mode_switch {
                        SetMode::DecSet
                    } else {
                        SetMode::DecRst
                    };
                    self.write_to_pty(
                        &AllowColumnModeSwitch::AllowColumnModeSwitch.report(Some(mode)),
                    );
                }
                other => {
                    // Modes handled by TerminalState's mode-sync loop, or
                    // modes not yet acted on.  Log for diagnostic visibility.
                    tracing::debug!("Mode not handled by TerminalHandler: {other}");
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
                self.window_commands.push(wm.clone());
            }
            TerminalOutput::RequestDeviceAttributes => {
                self.handle_request_device_attributes();
            }
            TerminalOutput::EightBitControl => {
                tracing::debug!("EightBitControl not yet implemented (ignored)");
            }
            TerminalOutput::SevenBitControl => {
                tracing::debug!("SevenBitControl not yet implemented (ignored)");
            }
            TerminalOutput::AnsiConformanceLevelOne => {
                tracing::debug!("AnsiConformanceLevelOne not yet implemented (ignored)");
            }
            TerminalOutput::AnsiConformanceLevelTwo => {
                tracing::debug!("AnsiConformanceLevelTwo not yet implemented (ignored)");
            }
            TerminalOutput::AnsiConformanceLevelThree => {
                tracing::debug!("AnsiConformanceLevelThree not yet implemented (ignored)");
            }
            TerminalOutput::DoubleLineHeightTop => {
                tracing::debug!("DoubleLineHeightTop not yet implemented (ignored)");
            }
            TerminalOutput::DoubleLineHeightBottom => {
                tracing::debug!("DoubleLineHeightBottom not yet implemented (ignored)");
            }
            TerminalOutput::SingleWidthLine => {
                tracing::debug!("SingleWidthLine not yet implemented (ignored)");
            }
            TerminalOutput::DoubleWidthLine => {
                tracing::debug!("DoubleWidthLine not yet implemented (ignored)");
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
                tracing::debug!(
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
                tracing::debug!("CursorToLowerLeftCorner not yet implemented (ignored)");
            }
            TerminalOutput::ResetDevice => {
                self.full_reset();
            }
            TerminalOutput::MemoryLock => {
                tracing::debug!("MemoryLock not yet implemented (ignored)");
            }
            TerminalOutput::MemoryUnlock => {
                tracing::debug!("MemoryUnlock not yet implemented (ignored)");
            }
            TerminalOutput::DeviceControlString(dcs) => {
                self.handle_device_control_string(dcs);
            }
            TerminalOutput::ApplicationProgramCommand(apc) => {
                self.handle_application_program_command(apc);
            }
            TerminalOutput::RequestDeviceNameAndVersion => {
                self.handle_device_name_and_version();
            }
            TerminalOutput::RequestSecondaryDeviceAttributes { param: _param } => {
                self.handle_secondary_device_attributes();
            }
            // Silently ignore invalid, skipped, and any future variants
            TerminalOutput::Invalid | TerminalOutput::Skipped | _ => {
                // Silently ignore for forward compatibility
            }
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

/// Apply a single `SelectGraphicRendition` value to a `FormatTag`, mutating it in-place.
///
/// This is the central mapping between the parser's SGR enum and the buffer's format
/// representation.  It is a pure function — it has no side effects beyond mutating `tag`.
#[allow(clippy::too_many_lines)]
fn apply_sgr(tag: &mut FormatTag, sgr: &SelectGraphicRendition) {
    match sgr {
        // Reset: restore every field to its default value
        SelectGraphicRendition::Reset => {
            *tag = FormatTag::default();
        }

        // Font weight
        SelectGraphicRendition::Bold => {
            tag.font_weight = FontWeight::Bold;
        }
        SelectGraphicRendition::ResetBold => {
            tag.font_weight = FontWeight::Normal;
        }
        // NormalIntensity resets both bold AND faint
        SelectGraphicRendition::NormalIntensity => {
            tag.font_weight = FontWeight::Normal;
            tag.font_decorations
                .retain(|d| *d != FontDecorations::Faint);
        }

        // Italic
        SelectGraphicRendition::Italic => {
            if !tag.font_decorations.contains(&FontDecorations::Italic) {
                tag.font_decorations.push(FontDecorations::Italic);
            }
        }
        SelectGraphicRendition::NotItalic => {
            tag.font_decorations
                .retain(|d| *d != FontDecorations::Italic);
        }

        // Faint
        SelectGraphicRendition::Faint => {
            if !tag.font_decorations.contains(&FontDecorations::Faint) {
                tag.font_decorations.push(FontDecorations::Faint);
            }
        }

        // Underline
        SelectGraphicRendition::Underline => {
            if !tag.font_decorations.contains(&FontDecorations::Underline) {
                tag.font_decorations.push(FontDecorations::Underline);
            }
        }
        SelectGraphicRendition::NotUnderlined => {
            tag.font_decorations
                .retain(|d| *d != FontDecorations::Underline);
        }

        // Strikethrough
        SelectGraphicRendition::Strikethrough => {
            if !tag
                .font_decorations
                .contains(&FontDecorations::Strikethrough)
            {
                tag.font_decorations.push(FontDecorations::Strikethrough);
            }
        }
        SelectGraphicRendition::NotStrikethrough => {
            tag.font_decorations
                .retain(|d| *d != FontDecorations::Strikethrough);
        }

        // Reverse video
        SelectGraphicRendition::ReverseVideo => {
            tag.colors.set_reverse_video(ReverseVideo::On);
        }
        SelectGraphicRendition::ResetReverseVideo => {
            tag.colors.set_reverse_video(ReverseVideo::Off);
        }

        // Colors
        SelectGraphicRendition::Foreground(color) => {
            tag.colors.set_color(*color);
        }
        SelectGraphicRendition::Background(color) => {
            tag.colors.set_background_color(*color);
        }
        SelectGraphicRendition::UnderlineColor(color) => {
            tag.colors.set_underline_color(*color);
        }

        // Intentionally ignored attributes and unknown codes — these have no FormatTag
        // equivalent.  Silently ignore for forward compatibility.
        SelectGraphicRendition::NoOp
        | SelectGraphicRendition::FastBlink
        | SelectGraphicRendition::SlowBlink
        | SelectGraphicRendition::NotBlinking
        | SelectGraphicRendition::Conceal
        | SelectGraphicRendition::Revealed
        | SelectGraphicRendition::PrimaryFont
        | SelectGraphicRendition::AlternativeFont1
        | SelectGraphicRendition::AlternativeFont2
        | SelectGraphicRendition::AlternativeFont3
        | SelectGraphicRendition::AlternativeFont4
        | SelectGraphicRendition::AlternativeFont5
        | SelectGraphicRendition::AlternativeFont6
        | SelectGraphicRendition::AlternativeFont7
        | SelectGraphicRendition::AlternativeFont8
        | SelectGraphicRendition::AlternativeFont9
        | SelectGraphicRendition::FontFranktur
        | SelectGraphicRendition::ProportionalSpacing
        | SelectGraphicRendition::DisableProportionalSpacing
        | SelectGraphicRendition::Framed
        | SelectGraphicRendition::Encircled
        | SelectGraphicRendition::Overlined
        | SelectGraphicRendition::NotOverlined
        | SelectGraphicRendition::NotFramedOrEncircled
        | SelectGraphicRendition::IdeogramUnderline
        | SelectGraphicRendition::IdeogramDoubleUnderline
        | SelectGraphicRendition::IdeogramOverline
        | SelectGraphicRendition::IdeogramDoubleOverline
        | SelectGraphicRendition::IdeogramStress
        | SelectGraphicRendition::IdeogramAttributes
        | SelectGraphicRendition::Superscript
        | SelectGraphicRendition::Subscript
        | SelectGraphicRendition::NeitherSuperscriptNorSubscript
        | SelectGraphicRendition::Unknown(_) => {}
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
            cursor::ReverseVideo,
            fonts::{FontDecorations, FontWeight},
            terminal_output::TerminalOutput,
        },
        colors::TerminalColor,
        sgr::SelectGraphicRendition,
    };

    use super::*;

    // ------------------------------------------------------------------
    // apply_sgr unit tests (pure function, no buffer involved)
    // ------------------------------------------------------------------

    #[test]
    fn sgr_bold_sets_font_weight() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Bold);
        assert_eq!(tag.font_weight, FontWeight::Bold);
    }

    #[test]
    fn sgr_reset_bold_clears_font_weight() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Bold);
        apply_sgr(&mut tag, &SelectGraphicRendition::ResetBold);
        assert_eq!(tag.font_weight, FontWeight::Normal);
    }

    #[test]
    fn sgr_normal_intensity_clears_bold_and_faint() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Bold);
        apply_sgr(&mut tag, &SelectGraphicRendition::Faint);
        apply_sgr(&mut tag, &SelectGraphicRendition::NormalIntensity);
        assert_eq!(tag.font_weight, FontWeight::Normal);
        assert!(!tag.font_decorations.contains(&FontDecorations::Faint));
    }

    #[test]
    fn sgr_italic_toggle() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Italic);
        assert!(tag.font_decorations.contains(&FontDecorations::Italic));
        apply_sgr(&mut tag, &SelectGraphicRendition::NotItalic);
        assert!(!tag.font_decorations.contains(&FontDecorations::Italic));
    }

    #[test]
    fn sgr_italic_not_duplicated() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Italic);
        apply_sgr(&mut tag, &SelectGraphicRendition::Italic);
        assert_eq!(
            tag.font_decorations
                .iter()
                .filter(|d| **d == FontDecorations::Italic)
                .count(),
            1
        );
    }

    #[test]
    fn sgr_underline_toggle() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Underline);
        assert!(tag.font_decorations.contains(&FontDecorations::Underline));
        apply_sgr(&mut tag, &SelectGraphicRendition::NotUnderlined);
        assert!(!tag.font_decorations.contains(&FontDecorations::Underline));
    }

    #[test]
    fn sgr_strikethrough_toggle() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Strikethrough);
        assert!(
            tag.font_decorations
                .contains(&FontDecorations::Strikethrough)
        );
        apply_sgr(&mut tag, &SelectGraphicRendition::NotStrikethrough);
        assert!(
            !tag.font_decorations
                .contains(&FontDecorations::Strikethrough)
        );
    }

    #[test]
    fn sgr_faint_adds_decoration() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Faint);
        assert!(tag.font_decorations.contains(&FontDecorations::Faint));
    }

    #[test]
    fn sgr_fg_color() {
        let mut tag = FormatTag::default();
        apply_sgr(
            &mut tag,
            &SelectGraphicRendition::Foreground(TerminalColor::Red),
        );
        assert_eq!(tag.colors.color, TerminalColor::Red);
    }

    #[test]
    fn sgr_bg_color() {
        let mut tag = FormatTag::default();
        apply_sgr(
            &mut tag,
            &SelectGraphicRendition::Background(TerminalColor::Blue),
        );
        assert_eq!(tag.colors.background_color, TerminalColor::Blue);
    }

    #[test]
    fn sgr_custom_rgb_fg() {
        let mut tag = FormatTag::default();
        apply_sgr(
            &mut tag,
            &SelectGraphicRendition::Foreground(TerminalColor::Custom(255, 128, 0)),
        );
        assert_eq!(tag.colors.color, TerminalColor::Custom(255, 128, 0));
    }

    #[test]
    fn sgr_underline_color() {
        let mut tag = FormatTag::default();
        apply_sgr(
            &mut tag,
            &SelectGraphicRendition::UnderlineColor(TerminalColor::Green),
        );
        assert_eq!(tag.colors.underline_color, TerminalColor::Green);
    }

    #[test]
    fn sgr_reverse_video_on_off() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::ReverseVideo);
        assert_eq!(tag.colors.reverse_video, ReverseVideo::On);
        apply_sgr(&mut tag, &SelectGraphicRendition::ResetReverseVideo);
        assert_eq!(tag.colors.reverse_video, ReverseVideo::Off);
    }

    #[test]
    fn sgr_reset_clears_all() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Bold);
        apply_sgr(
            &mut tag,
            &SelectGraphicRendition::Foreground(TerminalColor::Red),
        );
        apply_sgr(&mut tag, &SelectGraphicRendition::Italic);
        apply_sgr(&mut tag, &SelectGraphicRendition::Reset);
        assert_eq!(tag, FormatTag::default());
    }

    #[test]
    fn sgr_multiple_accumulate() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::Bold);
        apply_sgr(&mut tag, &SelectGraphicRendition::Underline);
        apply_sgr(
            &mut tag,
            &SelectGraphicRendition::Foreground(TerminalColor::Red),
        );
        assert_eq!(tag.font_weight, FontWeight::Bold);
        assert!(tag.font_decorations.contains(&FontDecorations::Underline));
        assert_eq!(tag.colors.color, TerminalColor::Red);
    }

    #[test]
    fn sgr_noop_does_nothing() {
        let mut tag = FormatTag::default();
        apply_sgr(&mut tag, &SelectGraphicRendition::NoOp);
        assert_eq!(tag, FormatTag::default());
    }

    // ------------------------------------------------------------------
    // handle_sgr integration tests (via TerminalHandler)
    // ------------------------------------------------------------------

    #[test]
    fn handle_sgr_bold_propagates_to_buffer_format() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_sgr(&SelectGraphicRendition::Bold);
        assert_eq!(handler.current_format.font_weight, FontWeight::Bold);
    }

    #[test]
    fn handle_sgr_reset_propagates_to_buffer_format() {
        let mut handler = TerminalHandler::new(80, 24);
        handler.handle_sgr(&SelectGraphicRendition::Bold);
        handler.handle_sgr(&SelectGraphicRendition::Reset);
        assert_eq!(handler.current_format, FormatTag::default());
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
                freminal_common::buffer_states::tchar::TChar::Utf8(v) => {
                    String::from_utf8_lossy(v).to_string()
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
    fn handle_sgr_palette_index_resolves_against_palette() {
        let mut handler = TerminalHandler::new(80, 24);

        // Set index 42 to a custom colour.
        handler.handle_osc(&AnsiOscType::SetPaletteColor(42, 0xDE, 0xAD, 0x00));

        // Apply SGR foreground with PaletteIndex(42).
        handler.handle_sgr(&SelectGraphicRendition::Foreground(
            TerminalColor::PaletteIndex(42),
        ));

        // The resolved colour should be Custom(0xDE, 0xAD, 0x00), not PaletteIndex(42).
        let fmt = handler.current_format();
        assert_eq!(
            fmt.colors.color,
            TerminalColor::Custom(0xDE, 0xAD, 0x00),
            "PaletteIndex should be resolved to Custom via palette lookup"
        );
    }

    #[test]
    fn handle_sgr_palette_index_background_and_underline() {
        let mut handler = TerminalHandler::new(80, 24);

        handler.handle_osc(&AnsiOscType::SetPaletteColor(200, 0xAA, 0xBB, 0xCC));

        // Background
        handler.handle_sgr(&SelectGraphicRendition::Background(
            TerminalColor::PaletteIndex(200),
        ));
        assert_eq!(
            handler.current_format().colors.background_color,
            TerminalColor::Custom(0xAA, 0xBB, 0xCC),
        );

        // Underline colour
        handler.handle_sgr(&SelectGraphicRendition::UnderlineColor(
            TerminalColor::PaletteIndex(200),
        ));
        assert_eq!(
            handler.current_format().colors.underline_color,
            TerminalColor::Custom(0xAA, 0xBB, 0xCC),
        );
    }

    #[test]
    fn handle_sgr_palette_index_uses_default_when_no_override() {
        let mut handler = TerminalHandler::new(80, 24);

        // PaletteIndex(1) with no override → should resolve to the default for index 1.
        handler.handle_sgr(&SelectGraphicRendition::Foreground(
            TerminalColor::PaletteIndex(1),
        ));

        let expected = handler.palette().lookup(1, handler.theme());
        assert_eq!(
            handler.current_format().colors.color,
            expected,
            "PaletteIndex without override should resolve to default colour"
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
            TerminalHandler::apply_aspect_ratio(None, None, 20, 10, 160, 160, 80, 24);
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
        );
        assert_eq!(cols, 20);
        assert_eq!(rows, 10);
    }

    #[test]
    fn aspect_ratio_width_auto_height_explicit() {
        // height=10 rows, image is square (100x100), cell aspect ~2:1 (8w x 16h)
        // scaled_cols = 10 * 100 * 2 / 100 = 20
        let (cols, rows) = TerminalHandler::apply_aspect_ratio(
            None,
            Some(&ImageDimension::Cells(10)),
            1, // initial cols (ignored, will be recomputed)
            10,
            100,
            100,
            80,
            24,
        );
        assert_eq!(rows, 10);
        assert_eq!(cols, 20);
    }

    #[test]
    fn aspect_ratio_height_auto_width_explicit() {
        // width=20 cols, image is square (100x100), cell aspect ~2:1
        // scaled_rows = 20 * 100 / (100 * 2) = 10
        let (cols, rows) = TerminalHandler::apply_aspect_ratio(
            Some(&ImageDimension::Cells(20)),
            None,
            20,
            1, // initial rows (ignored, will be recomputed)
            100,
            100,
            80,
            24,
        );
        assert_eq!(cols, 20);
        assert_eq!(rows, 10);
    }

    #[test]
    fn aspect_ratio_clamped_to_term_size() {
        // width=80 cols, image is very tall (100w x 10000h)
        // scaled_rows = 80 * 10000 / (100 * 2) = 4000, clamped to 24
        let (cols, rows) = TerminalHandler::apply_aspect_ratio(
            Some(&ImageDimension::Cells(80)),
            None,
            80,
            1,
            100,
            10000,
            80,
            24,
        );
        assert_eq!(cols, 80);
        assert_eq!(rows, 24);
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
            data: png_buf,
        };

        handler.handle_iterm2_inline_image(&image_data);

        // After placement, cursor should have moved down by display_rows (2).
        // The buffer should contain image cells.
        let buf = handler.buffer();
        let rows = buf.get_rows();

        // Check that at least one cell has an image placement.
        let has_image_cell = rows.iter().any(|row| {
            let cells = row.cells();
            cells.iter().any(crate::cell::Cell::has_image)
        });
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
            data: vec![0xFF; 100],
        };

        // Cursor should not move.
        let cursor_before = handler.cursor_pos();
        handler.handle_iterm2_inline_image(&image_data);
        let cursor_after = handler.cursor_pos();
        assert_eq!(cursor_before, cursor_after);
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
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin_data));

        // Verify no image placed yet
        let has_image_before = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
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
        let has_image_after = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
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
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin_data));
        handler.handle_osc(&AnsiOscType::ITerm2FilePart(png_data));
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);

        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
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

        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
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
            data: Vec::new(),
        };
        handler.handle_osc(&AnsiOscType::ITerm2MultipartBegin(begin_data));
        handler.handle_osc(&AnsiOscType::ITerm2FileEnd);

        // No image should be placed (empty payload)
        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
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

        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
        assert!(
            !has_image,
            "Non-inline multipart transfer should not place an image"
        );
    }

    // ------------------------------------------------------------------
    // Kitty graphics direct transfer tests
    // ------------------------------------------------------------------

    /// Helper: create a `TerminalHandler` with a write channel and return
    /// `(handler, rx)` so tests can inspect PTY responses.
    fn kitty_handler() -> (TerminalHandler, crossbeam_channel::Receiver<PtyWrite>) {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);
        (handler, rx)
    }

    /// Helper: build a `KittyGraphicsCommand` with the given control data and
    /// raw RGBA payload for a 2x2 image.
    fn kitty_rgba_2x2_cmd(action: KittyAction) -> KittyGraphicsCommand {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyFormat};

        // 2x2 RGBA = 16 bytes.
        let rgba_data: Vec<u8> = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 0, 255, // yellow
        ];

        KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(action),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: rgba_data,
        }
    }

    #[test]
    fn kitty_single_rgba_transmit_and_display_places_image() {
        use freminal_common::buffer_states::kitty_graphics::KittyAction;

        let (mut handler, _rx) = kitty_handler();
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);

        handler.handle_kitty_graphics(cmd);

        // Image should be placed in the buffer.
        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
        assert!(
            has_image,
            "Expected image cells after Kitty TransmitAndDisplay"
        );

        // Image should be in the store.
        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "Expected image id=42 in the image store"
        );
    }

    #[test]
    fn kitty_single_rgba_transmit_only_stores_but_does_not_place() {
        use freminal_common::buffer_states::kitty_graphics::KittyAction;

        let (mut handler, _rx) = kitty_handler();
        let cmd = kitty_rgba_2x2_cmd(KittyAction::Transmit);

        handler.handle_kitty_graphics(cmd);

        // Image should be in the store but NOT placed in cells.
        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "Expected image id=42 in the store after Transmit"
        );

        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
        assert!(!has_image, "Transmit-only should not place image cells");
    }

    #[test]
    fn kitty_single_rgb_converts_to_rgba() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // 2x2 RGB = 12 bytes (no alpha channel).
        let rgb_data: Vec<u8> = vec![
            255, 0, 0, // red
            0, 255, 0, // green
            0, 0, 255, // blue
            255, 255, 0, // yellow
        ];

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgb),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(99),
                ..KittyControlData::default()
            },
            payload: rgb_data,
        };

        handler.handle_kitty_graphics(cmd);

        // The stored image should have RGBA pixels (16 bytes for 2x2).
        let img = handler.buffer().image_store().get(99).unwrap();
        assert_eq!(
            img.pixels.len(),
            16,
            "RGB should be converted to RGBA (4 bytes/pixel)"
        );
        // Verify alpha was inserted: first pixel should be [255, 0, 0, 255].
        assert_eq!(&img.pixels[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn kitty_single_png_decodes_and_places() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        let png_data = make_test_png(); // 2x2 red PNG.

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Png),
                image_id: Some(77),
                // PNG does not require s/v — dimensions come from the image.
                ..KittyControlData::default()
            },
            payload: png_data,
        };

        handler.handle_kitty_graphics(cmd);

        let img = handler.buffer().image_store().get(77).unwrap();
        assert_eq!(img.width_px, 2);
        assert_eq!(img.height_px, 2);
        // PNG decoded to RGBA: 2*2*4 = 16 bytes.
        assert_eq!(img.pixels.len(), 16);
    }

    #[test]
    fn kitty_single_rgba_missing_dimensions_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // RGBA payload but no s/v dimensions → should send error.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                image_id: Some(10),
                // No src_width or src_height!
                ..KittyControlData::default()
            },
            payload: vec![0; 16],
        };

        handler.handle_kitty_graphics(cmd);

        // Should NOT be stored.
        assert!(
            handler.buffer().image_store().get(10).is_none(),
            "Image should not be stored when dimensions are missing"
        );

        // Should have sent an error response.
        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("EINVAL"), "Expected EINVAL error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_single_rgba_payload_size_mismatch_sends_error() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // Says 2x2 RGBA (expects 16 bytes) but payload is only 8 bytes.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(11),
                ..KittyControlData::default()
            },
            payload: vec![0; 8], // too small
        };

        handler.handle_kitty_graphics(cmd);

        assert!(handler.buffer().image_store().get(11).is_none());

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("EINVAL"), "Expected EINVAL error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_single_empty_payload_sends_enodata() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(12),
                ..KittyControlData::default()
            },
            payload: Vec::new(), // empty
        };

        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("ENODATA"), "Expected ENODATA error, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_single_ok_response_sent_for_verbose_mode() {
        use freminal_common::buffer_states::kitty_graphics::KittyAction;

        let (mut handler, rx) = kitty_handler();

        // quiet=0 (default) → should send OK.
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(s.contains("OK"), "Expected OK response, got: {s}");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_single_quiet_1_suppresses_ok() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(50),
                quiet: 1, // suppress OK but not errors
                ..KittyControlData::default()
            },
            payload: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        };

        handler.handle_kitty_graphics(cmd);

        // Image should still be placed.
        assert!(handler.buffer().image_store().get(50).is_some());

        // But no OK response should be sent.
        assert!(
            rx.try_recv().is_err(),
            "quiet=1 should suppress OK response"
        );
    }

    #[test]
    fn kitty_single_quiet_2_suppresses_all_responses() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        // quiet=2 with a bad payload → error should be suppressed.
        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(51),
                quiet: 2,
                ..KittyControlData::default()
            },
            payload: vec![0; 8], // wrong size → EINVAL
        };

        handler.handle_kitty_graphics(cmd);

        // No response at all.
        assert!(
            rx.try_recv().is_err(),
            "quiet=2 should suppress all responses including errors"
        );
    }

    #[test]
    fn kitty_delete_all_clears_images() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget,
        };

        let (mut handler, _rx) = kitty_handler();

        // First, place an image.
        let cmd = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd);
        assert!(handler.buffer().image_store().get(42).is_some());

        // Now delete all.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::All),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        // Image should be gone from the store.
        assert!(
            handler.buffer().image_store().get(42).is_none(),
            "Delete all should remove all images"
        );

        // No image cells should remain.
        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
        assert!(!has_image, "Delete all should clear image cells");
    }

    #[test]
    fn kitty_delete_by_id_removes_only_target() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyDeleteTarget, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Place image id=42.
        let cmd1 = kitty_rgba_2x2_cmd(KittyAction::TransmitAndDisplay);
        handler.handle_kitty_graphics(cmd1);

        // Place image id=99.
        let cmd2 = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Transmit),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(99),
                ..KittyControlData::default()
            },
            payload: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        };
        handler.handle_kitty_graphics(cmd2);

        assert!(handler.buffer().image_store().get(42).is_some());
        assert!(handler.buffer().image_store().get(99).is_some());

        // Delete only id=42.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ById),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.buffer().image_store().get(42).is_none(),
            "id=42 should be removed"
        );
        assert!(
            handler.buffer().image_store().get(99).is_some(),
            "id=99 should survive delete-by-id of 42"
        );
    }

    #[test]
    fn kitty_display_cols_and_rows_override() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(55),
                display_cols: Some(10),
                display_rows: Some(5),
                ..KittyControlData::default()
            },
            payload: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        };

        handler.handle_kitty_graphics(cmd);

        let img = handler.buffer().image_store().get(55).unwrap();
        assert_eq!(img.display_cols, 10);
        assert_eq!(img.display_rows, 5);
    }

    #[test]
    fn kitty_query_rgb_format_responds_ok() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, rx) = kitty_handler();

        let cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Query),
                format: Some(KittyFormat::Rgb),
                image_id: Some(31),
                ..KittyControlData::default()
            },
            payload: vec![0, 0, 0], // minimal payload for query
        };

        handler.handle_kitty_graphics(cmd);

        let response = rx.try_recv().unwrap();
        match response {
            PtyWrite::Write(bytes) => {
                let s = String::from_utf8_lossy(&bytes);
                assert!(
                    s.contains("OK"),
                    "f=24 (RGB) query should succeed, got: {s}"
                );
                assert!(!s.contains("ENOTSUP"), "f=24 should NOT be unsupported");
            }
            PtyWrite::Resize(_) => panic!("Expected PtyWrite::Write"),
        }
    }

    #[test]
    fn kitty_chunked_transfer_assembles_and_places() {
        use freminal_common::buffer_states::kitty_graphics::{
            KittyAction, KittyControlData, KittyFormat,
        };

        let (mut handler, _rx) = kitty_handler();

        // Full RGBA payload for 2x2 = 16 bytes, split into two 8-byte chunks.
        let full_payload: Vec<u8> = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
        ];

        // First chunk (more_data=true).
        let chunk1 = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(88),
                more_data: true,
                ..KittyControlData::default()
            },
            payload: full_payload[..8].to_vec(),
        };

        // Last chunk (more_data=false / default).
        let chunk2 = KittyGraphicsCommand {
            control: KittyControlData {
                more_data: false,
                ..KittyControlData::default()
            },
            payload: full_payload[8..].to_vec(),
        };

        handler.handle_kitty_graphics(chunk1);
        // After first chunk, image should NOT be in the store yet.
        assert!(
            handler.buffer().image_store().get(88).is_none(),
            "Image should not appear until final chunk"
        );

        handler.handle_kitty_graphics(chunk2);
        // After final chunk, image should be stored and placed.
        assert!(
            handler.buffer().image_store().get(88).is_some(),
            "Image should appear after final chunk"
        );

        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
        assert!(has_image, "Chunked TransmitAndDisplay should place image");
    }

    // ------------------------------------------------------------------
    // Kitty Unicode placeholder tests
    // ------------------------------------------------------------------

    /// Helper: create a Kitty command with `unicode_placeholder = true` for a 2×2 RGBA image.
    fn kitty_virtual_2x2_cmd() -> KittyGraphicsCommand {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyFormat};

        let rgba_data: Vec<u8> = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 0, 255, // yellow
        ];

        KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::TransmitAndDisplay),
                format: Some(KittyFormat::Rgba),
                src_width: Some(2),
                src_height: Some(2),
                image_id: Some(42),
                placement_id: Some(0),
                unicode_placeholder: true,
                ..KittyControlData::default()
            },
            payload: rgba_data,
        }
    }

    /// Helper: build a `FormatTag` whose foreground encodes an image ID and
    /// whose underline color encodes a placement ID using 24-bit RGB.
    fn format_for_placeholder(image_id: u32, placement_id: u32) -> FormatTag {
        use freminal_common::buffer_states::cursor::StateColors;

        #[allow(clippy::cast_possible_truncation)]
        let fg = TerminalColor::Custom(
            (image_id >> 16) as u8,
            (image_id >> 8) as u8,
            image_id as u8,
        );

        #[allow(clippy::cast_possible_truncation)]
        let ul = TerminalColor::Custom(
            (placement_id >> 16) as u8,
            (placement_id >> 8) as u8,
            placement_id as u8,
        );

        let mut tag = FormatTag::default();
        tag.colors = StateColors {
            color: fg,
            underline_color: ul,
            ..tag.colors
        };
        tag
    }

    /// U+10EEEE without any diacritics (bare placeholder char).
    const PLACEHOLDER_BYTES: &[u8] = &[0xF4, 0x8E, 0xBB, 0xAE];

    /// U+10EEEE followed by one diacritic (row=0 → U+0305).
    fn placeholder_with_row(row_idx: usize) -> Vec<u8> {
        let mut bytes = PLACEHOLDER_BYTES.to_vec();
        let diacritic =
            char::from_u32(DIACRITICS_FOR_TESTS[row_idx]).expect("valid diacritic codepoint");
        let mut buf = [0u8; 4];
        let encoded = diacritic.encode_utf8(&mut buf);
        bytes.extend_from_slice(encoded.as_bytes());
        bytes
    }

    /// U+10EEEE followed by two diacritics (row + col).
    fn placeholder_with_row_col(row_idx: usize, col_idx: usize) -> Vec<u8> {
        let mut bytes = PLACEHOLDER_BYTES.to_vec();
        for &idx in &[row_idx, col_idx] {
            let diacritic =
                char::from_u32(DIACRITICS_FOR_TESTS[idx]).expect("valid diacritic codepoint");
            let mut buf = [0u8; 4];
            let encoded = diacritic.encode_utf8(&mut buf);
            bytes.extend_from_slice(encoded.as_bytes());
        }
        bytes
    }

    /// A few diacritics from the table for test convenience.
    /// Index 0 = U+0305, index 1 = U+030D, index 2 = U+030E, etc.
    const DIACRITICS_FOR_TESTS: &[u32] = &[0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D];

    #[test]
    fn kitty_virtual_placement_stores_but_does_not_place_cells() {
        let (mut handler, _rx) = kitty_handler();
        let cmd = kitty_virtual_2x2_cmd();

        handler.handle_kitty_graphics(cmd);

        // Image should be in the store.
        assert!(
            handler.buffer().image_store().get(42).is_some(),
            "Image id=42 should be in the image store"
        );

        // But NO image cells should be placed in the buffer.
        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
        assert!(
            !has_image,
            "Virtual placement should NOT place image cells directly"
        );

        // Virtual placement should be stored.
        assert!(
            !handler.virtual_placements.is_empty(),
            "Virtual placements table should have an entry"
        );
        assert!(
            handler.virtual_placements.contains_key(&(42, 0)),
            "Should have virtual placement for (image_id=42, placement_id=0)"
        );
    }

    #[test]
    fn kitty_placeholder_chars_create_image_cells() {
        let (mut handler, _rx) = kitty_handler();

        // Step 1: Create a virtual placement with U=1.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);

        // Step 2: Set foreground to encode image_id=42, underline to placement_id=0.
        let fmt = format_for_placeholder(42, 0);
        handler.set_format(fmt);

        // Step 3: Send two placeholder chars (row=0, col=0) and (row=0, col=1)
        // with explicit row+col diacritics.
        let mut data = placeholder_with_row_col(0, 0);
        data.extend_from_slice(&placeholder_with_row_col(0, 1));
        handler.handle_data(&data);

        // Step 4: Verify image cells were placed.
        let row0 = &handler.buffer().get_rows()[0];
        assert!(
            row0.cells()[0].has_image(),
            "Cell (0,0) should have an image placement"
        );
        assert!(
            row0.cells()[1].has_image(),
            "Cell (0,1) should have an image placement"
        );

        // Verify the image placements reference the correct image and positions.
        let p0 = row0.cells()[0].image_placement().unwrap();
        assert_eq!(p0.image_id, 42, "Cell (0,0) should reference image_id=42");
        assert_eq!(p0.row_in_image, 0);
        assert_eq!(p0.col_in_image, 0);

        let p1 = row0.cells()[1].image_placement().unwrap();
        assert_eq!(p1.image_id, 42, "Cell (0,1) should reference image_id=42");
        assert_eq!(p1.row_in_image, 0);
        assert_eq!(p1.col_in_image, 1);
    }

    #[test]
    fn kitty_placeholder_inheritance_no_diacritics() {
        let (mut handler, _rx) = kitty_handler();

        // Create virtual placement.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);

        // Set foreground = image_id=42, underline = placement_id=0.
        let fmt = format_for_placeholder(42, 0);
        handler.set_format(fmt);

        // Send first placeholder with explicit row=0, col=0.
        let first = placeholder_with_row_col(0, 0);
        handler.handle_data(&first);

        // Send second placeholder with NO diacritics — should inherit row=0, col=1.
        handler.handle_data(PLACEHOLDER_BYTES);

        let row0 = &handler.buffer().get_rows()[0];
        let p0 = row0.cells()[0].image_placement().unwrap();
        assert_eq!(p0.row_in_image, 0);
        assert_eq!(p0.col_in_image, 0);

        let p1 = row0.cells()[1].image_placement().unwrap();
        assert_eq!(p1.row_in_image, 0);
        assert_eq!(p1.col_in_image, 1, "Inherited col should be prev_col + 1");
    }

    #[test]
    fn kitty_placeholder_inheritance_row_only_diacritic() {
        let (mut handler, _rx) = kitty_handler();

        // Create virtual placement.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);

        let fmt = format_for_placeholder(42, 0);
        handler.set_format(fmt);

        // First cell: row=0, col=0 (explicit).
        let first = placeholder_with_row_col(0, 0);
        handler.handle_data(&first);

        // Second cell: row=0 only (one diacritic) — should inherit col=1 from prev.
        let second = placeholder_with_row(0);
        handler.handle_data(&second);

        let row0 = &handler.buffer().get_rows()[0];
        let p1 = row0.cells()[1].image_placement().unwrap();
        assert_eq!(p1.row_in_image, 0);
        assert_eq!(p1.col_in_image, 1, "Should inherit col+1 from previous");
    }

    #[test]
    fn kitty_placeholder_new_row_resets_col() {
        let (mut handler, _rx) = kitty_handler();

        // Create virtual placement.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);

        let fmt = format_for_placeholder(42, 0);
        handler.set_format(fmt);

        // Row 0, col 0 (explicit).
        handler.handle_data(&placeholder_with_row_col(0, 0));

        // Row 0, col 1 (inherited via bare placeholder).
        handler.handle_data(PLACEHOLDER_BYTES);

        // Now move to next line (simulate \r\n or newline).
        handler.handle_newline();
        handler.handle_carriage_return();

        // Row 1, col 0 with row-only diacritic — different row, so col resets to 0.
        handler.handle_data(&placeholder_with_row(1));

        let row1 = &handler.buffer().get_rows()[1];
        let p = row1.cells()[0].image_placement().unwrap();
        assert_eq!(p.row_in_image, 1, "Should be row 1");
        assert_eq!(p.col_in_image, 0, "New row should start at col 0");
    }

    #[test]
    fn kitty_placeholder_no_virtual_placement_inserts_space() {
        let (mut handler, _rx) = kitty_handler();
        // Do NOT create any virtual placement.

        // Set foreground to encode image_id=99.
        let fmt = format_for_placeholder(99, 0);
        handler.set_format(fmt);

        // Send a placeholder char with row=0, col=0.
        let data = placeholder_with_row_col(0, 0);

        // But wait — the fast path skips placeholder processing when
        // virtual_placements is empty. The char goes through as normal text.
        handler.handle_data(&data);

        // The cell should NOT have an image placement.
        let row0 = &handler.buffer().get_rows()[0];
        assert!(
            !row0.cells()[0].has_image(),
            "Without virtual placements, placeholder should not create image cells"
        );
    }

    #[test]
    fn kitty_delete_all_clears_virtual_placements() {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyDeleteTarget};

        let (mut handler, _rx) = kitty_handler();

        // Create a virtual placement.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);
        assert!(!handler.virtual_placements.is_empty());

        // Delete all.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::All),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.virtual_placements.is_empty(),
            "Delete all should clear virtual placements"
        );
    }

    #[test]
    fn kitty_delete_by_id_clears_matching_virtual_placements() {
        use freminal_common::buffer_states::kitty_graphics::{KittyControlData, KittyDeleteTarget};

        let (mut handler, _rx) = kitty_handler();

        // Create a virtual placement for image_id=42.
        let cmd = kitty_virtual_2x2_cmd();
        handler.handle_kitty_graphics(cmd);
        assert!(handler.virtual_placements.contains_key(&(42, 0)));

        // Delete by ID = 42.
        let delete_cmd = KittyGraphicsCommand {
            control: KittyControlData {
                action: Some(KittyAction::Delete),
                delete_target: Some(KittyDeleteTarget::ById),
                image_id: Some(42),
                ..KittyControlData::default()
            },
            payload: Vec::new(),
        };
        handler.handle_kitty_graphics(delete_cmd);

        assert!(
            handler.virtual_placements.is_empty(),
            "Delete by ID=42 should clear the virtual placement"
        );
    }

    #[test]
    fn kitty_placeholder_fast_path_with_no_virtual_placements() {
        let (mut handler, _rx) = kitty_handler();
        // No virtual placements — fast path should just insert text normally.

        handler.handle_data(b"Hello World");

        // Cursor should have advanced.
        let cursor = handler.buffer().get_cursor();
        assert_eq!(
            cursor.pos.x, 11,
            "Cursor should be at column 11 after 'Hello World'"
        );
    }

    // -----------------------------------------------------------------------
    // Sixel integration tests
    // -----------------------------------------------------------------------

    /// Build a DCS Sixel payload with `P` prefix and `ESC \` suffix.
    /// `params` is the "P1;P2;P3" part (may be empty), `sixel_body` is
    /// everything after the `q` introducer.
    fn build_sixel_dcs(params: &[u8], sixel_body: &[u8]) -> Vec<u8> {
        // Format: P <params> q <sixel_body> ESC '\'
        let mut v = vec![b'P'];
        v.extend_from_slice(params);
        v.push(b'q');
        v.extend_from_slice(sixel_body);
        v.extend_from_slice(b"\x1b\\");
        v
    }

    #[test]
    fn sixel_simple_red_pixel_places_image() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Define color 1 as red (RGB 100,0,0) and paint one sixel column.
        // '#1;2;100;0;0' defines color 1, '#1' selects it, '~' = 0x7E = 0b111111
        // encodes 6 pixels all set (1 column x 6 rows of red).
        let sixel_body = b"#1;2;100;0;0#1~";
        let dcs = build_sixel_dcs(b"0;0;0", sixel_body);
        handler.handle_device_control_string(&dcs);

        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
        assert!(
            has_image,
            "Sixel DCS should place image cells in the buffer"
        );
    }

    #[test]
    fn sixel_image_stored_in_image_store() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        let initial_len = handler.buffer().image_store().len();

        let sixel_body = b"#1;2;100;0;0#1~";
        let dcs = build_sixel_dcs(b"0;0;0", sixel_body);
        handler.handle_device_control_string(&dcs);

        assert_eq!(
            handler.buffer().image_store().len(),
            initial_len + 1,
            "Image store should contain one more image after Sixel"
        );
    }

    #[test]
    fn sixel_empty_body_no_image() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Just the `q` introducer with no sixel data at all → zero-dimension image.
        let dcs = build_sixel_dcs(b"", b"");
        handler.handle_device_control_string(&dcs);

        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
        assert!(
            !has_image,
            "Empty Sixel body should not produce image cells"
        );
    }

    #[test]
    fn sixel_with_repeat_expands_width() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Color 0 = red, then repeat '~' 16 times → 16 columns, 6 rows.
        // At 8 px/col that is 2 cells wide; at 16 px/row that is 1 cell tall.
        let sixel_body = b"#0;2;100;0;0#0!16~";
        let dcs = build_sixel_dcs(b"", sixel_body);
        handler.handle_device_control_string(&dcs);

        let has_image = handler
            .buffer()
            .get_rows()
            .iter()
            .any(|row| row.cells().iter().any(crate::cell::Cell::has_image));
        assert!(
            has_image,
            "Sixel with repeat operator should place image cells"
        );

        // The image should be 16 px wide, 6 px tall.
        // Find it via the image store iterator (there should be exactly one).
        let store = handler.buffer().image_store();
        let (_, img) = store
            .iter()
            .next()
            .expect("image store should contain an image");
        assert_eq!(img.width_px, 16, "Image width should be 16 pixels");
        assert_eq!(img.height_px, 6, "Image height should be 6 pixels");
    }

    #[test]
    fn sixel_multicolor_two_bands() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Define red and blue. Paint one column of red, newline, one column of blue.
        // Total: 1 px wide, 12 px tall (2 bands of 6).
        let sixel_body = b"#1;2;100;0;0#2;2;0;0;100#1~-#2~";
        let dcs = build_sixel_dcs(b"", sixel_body);
        handler.handle_device_control_string(&dcs);

        let store = handler.buffer().image_store();
        let (_, img) = store
            .iter()
            .next()
            .expect("image store should contain an image");
        assert_eq!(img.width_px, 1, "Image width should be 1 pixel");
        assert_eq!(
            img.height_px, 12,
            "Image height should be 12 pixels (2 bands)"
        );
    }

    #[test]
    fn sixel_with_raster_attributes() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Raster attributes: "1;1;8;12" = aspect 1:1, declared 8 wide x 12 tall.
        // Then paint 2 bands of 8 columns each.
        let sixel_body = b"\"1;1;8;12#1;2;0;100;0#1!8~-#1!8~";
        let dcs = build_sixel_dcs(b"", sixel_body);
        handler.handle_device_control_string(&dcs);

        let store = handler.buffer().image_store();
        let (_, img) = store
            .iter()
            .next()
            .expect("image store should contain an image");
        assert_eq!(img.width_px, 8, "Raster-declared width should be 8 pixels");
        assert_eq!(
            img.height_px, 12,
            "Raster-declared height should be 12 pixels"
        );
    }

    #[test]
    fn sixel_transparent_background() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // P2=1 means transparent background.
        // Paint only the top pixel of a single column (bit 0 set = '?' + 1 = '@').
        // The remaining 5 pixels in the column should be transparent (alpha=0).
        let sixel_body = b"#1;2;100;0;0#1@";
        let dcs = build_sixel_dcs(b"0;1;0", sixel_body);
        handler.handle_device_control_string(&dcs);

        let store = handler.buffer().image_store();
        let (_, img) = store
            .iter()
            .next()
            .expect("image store should contain an image");

        // 1 pixel wide, 6 pixels tall → 6 * 4 = 24 bytes RGBA.
        assert_eq!(img.pixels.len(), 24);

        // First pixel (row 0): red, fully opaque — '@' = 0x40 - 0x3F = 1 = bit 0 set.
        assert_eq!(img.pixels[0], 255, "R channel of top pixel");
        assert_eq!(img.pixels[1], 0, "G channel of top pixel");
        assert_eq!(img.pixels[2], 0, "B channel of top pixel");
        assert_eq!(img.pixels[3], 255, "A channel of top pixel (opaque)");

        // Second pixel (row 1): transparent — bit 1 not set.
        assert_eq!(img.pixels[7], 0, "A channel of second pixel (transparent)");
    }

    #[test]
    fn sixel_carriage_return_overlays_same_band() {
        let mut handler = TerminalHandler::new(80, 24);
        let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
        handler.set_write_tx(tx);

        // Paint red in column 0, then CR ('$'), then paint blue in column 0.
        // Blue should overwrite red in the overlapping pixels.
        // '~' = all 6 bits set.
        let sixel_body = b"#1;2;100;0;0#1~$#2;2;0;0;100#2~";
        let dcs = build_sixel_dcs(b"0;1;0", sixel_body);
        handler.handle_device_control_string(&dcs);

        let store = handler.buffer().image_store();
        let (_, img) = store
            .iter()
            .next()
            .expect("image store should contain an image");

        // Column 0, all 6 pixels should be blue (last writer wins).
        // First pixel: RGBA at offset 0..4.
        assert_eq!(img.pixels[0], 0, "R of overwritten pixel");
        assert_eq!(img.pixels[1], 0, "G of overwritten pixel");
        assert_eq!(img.pixels[2], 255, "B of overwritten pixel");
        assert_eq!(
            img.pixels[3], 255,
            "A of overwritten pixel (opaque, set by blue)"
        );
    }

    #[test]
    fn is_sixel_sequence_detection() {
        // Valid sixel sequences.
        assert!(
            TerminalHandler::is_sixel_sequence(b"q~"),
            "bare 'q' + data is sixel"
        );
        assert!(
            TerminalHandler::is_sixel_sequence(b"0;0;0q~"),
            "params before q is sixel"
        );
        assert!(
            TerminalHandler::is_sixel_sequence(b"0q"),
            "single param before q is sixel"
        );

        // Not sixel.
        assert!(
            !TerminalHandler::is_sixel_sequence(b"$qm"),
            "DECRQSS ($q) is not sixel"
        );
        assert!(
            !TerminalHandler::is_sixel_sequence(b"+qHEX"),
            "XTGETTCAP (+q) is not sixel"
        );
        assert!(
            !TerminalHandler::is_sixel_sequence(b"no_q_here"),
            "no q at all is not sixel"
        );
        assert!(
            !TerminalHandler::is_sixel_sequence(b"abcq~"),
            "letters before q is not sixel"
        );
    }
}
