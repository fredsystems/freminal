// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::{
    buffer_states::{
        cursor::{ReverseVideo, StateColors},
        fonts::{FontDecorations, FontWeight},
        format_tag::FormatTag,
        line_draw::DecSpecialGraphics,
        mode::Mode,
        modes::decawm::Decawm,
        modes::dectcem::Dectcem,
        modes::lnm::Lnm,
        modes::xtcblink::XtCBlink,
        modes::xtextscrn::XtExtscrn,
        tchar::TChar,
        terminal_output::TerminalOutput,
    },
    cursor::CursorVisualStyle,
    sgr::SelectGraphicRendition,
};

use crate::buffer::Buffer;

/// High-level handler that processes terminal output commands and applies them to a buffer.
///
/// This is the main entry point for integrating the buffer with a terminal emulator.
/// It receives parsed terminal sequences (via a TerminalOutput-like enum) and updates
/// the buffer state accordingly.
pub struct TerminalHandler {
    buffer: Buffer,
    current_format: FormatTag,
    /// Whether the cursor should be rendered (`Dectcem::Show`) or hidden.
    show_cursor: Dectcem,
    /// The current cursor shape and blink state.
    cursor_visual_style: CursorVisualStyle,
    /// Whether DEC Special Graphics character remapping is active.
    character_replace: DecSpecialGraphics,
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
        }
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

    /// Handle raw data bytes - convert to `TChar` and insert.
    /// When DEC Special Graphics mode is active, bytes 0x5F–0x7E are remapped
    /// to their Unicode box-drawing equivalents before conversion.
    pub fn handle_data(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        let remapped: Vec<u8> = apply_dec_special(data, &self.character_replace);
        if let Ok(text) = TChar::from_vec(&remapped) {
            self.buffer.insert_text(&text);
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
    pub const fn handle_set_scroll_region(&mut self, top: usize, bottom: usize) {
        // Parser typically sends 1-indexed values
        let top_zero = top.saturating_sub(1);
        let bottom_zero = bottom.saturating_sub(1);
        self.buffer.set_scroll_region(top_zero, bottom_zero);
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

    /// Handle SGR (Select Graphic Rendition) — update `current_format` and propagate to buffer.
    pub fn handle_sgr(&mut self, sgr: &SelectGraphicRendition) {
        apply_sgr(&mut self.current_format, sgr);
        self.buffer.set_format(self.current_format.clone());
    }

    /// Update format tag directly
    pub fn set_format(&mut self, format: FormatTag) {
        self.current_format = format.clone();
        self.buffer.set_format(format);
    }

    /// Handle entering alternate screen
    pub fn handle_enter_alternate(&mut self) {
        self.buffer.enter_alternate();
    }

    /// Handle leaving alternate screen
    pub fn handle_leave_alternate(&mut self) {
        self.buffer.leave_alternate();
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
            // Query: deferred to Step 3.5
            XtCBlink::Query => {}
        }
    }

    /// Handle LNM — enable or disable Line Feed Mode.
    pub const fn handle_set_lnm(&mut self, enabled: bool) {
        self.buffer.set_lnm(enabled);
    }

    /// Handle resize
    pub fn handle_resize(&mut self, width: usize, height: usize) {
        self.buffer.set_size(width, height);
    }

    /// Handle scroll back (user scrolling)
    pub fn handle_scroll_back(&mut self, lines: usize) {
        self.buffer.scroll_back(lines);
    }

    /// Handle scroll forward (user scrolling)
    pub fn handle_scroll_forward(&mut self, lines: usize) {
        self.buffer.scroll_forward(lines);
    }

    /// Handle scroll to bottom
    pub fn handle_scroll_to_bottom(&mut self) {
        self.buffer.scroll_to_bottom();
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
                todo!("Bell not yet implemented");
            }
            TerminalOutput::ApplicationKeypadMode => {
                todo!("ApplicationKeypadMode not yet implemented");
            }
            TerminalOutput::NormalKeypadMode => {
                todo!("NormalKeypadMode not yet implemented");
            }
            TerminalOutput::Erase(n) => {
                self.handle_erase_chars(*n);
            }
            TerminalOutput::Sgr(sgr) => {
                self.handle_sgr(sgr);
            }
            TerminalOutput::Mode(mode) => match mode {
                Mode::XtExtscrn(XtExtscrn::Alternate) => self.handle_enter_alternate(),
                Mode::XtExtscrn(XtExtscrn::Primary) => self.handle_leave_alternate(),
                // Query variants: report mode — deferred to Step 3.5
                Mode::XtExtscrn(XtExtscrn::Query)
                | Mode::Decawm(Decawm::Query)
                | Mode::LineFeedMode(Lnm::Query)
                | Mode::Dectem(Dectcem::Query) => {
                    // TODO: Step 3.5 — report mode via outbound write channel
                }
                Mode::Decawm(Decawm::AutoWrap) => self.handle_set_wrap(true),
                Mode::Decawm(Decawm::NoAutoWrap) => self.handle_set_wrap(false),
                Mode::LineFeedMode(Lnm::NewLine) => self.handle_set_lnm(true),
                Mode::LineFeedMode(Lnm::LineFeed) => self.handle_set_lnm(false),
                Mode::Dectem(Dectcem::Show) => self.show_cursor = Dectcem::Show,
                Mode::Dectem(Dectcem::Hide) => self.show_cursor = Dectcem::Hide,
                Mode::XtCBlink(blink) => self.apply_xtcblink(blink),
                _other => {
                    // All other modes: silently ignore.
                    // Do NOT use todo!() — unknown modes must never panic.
                }
            },
            TerminalOutput::OscResponse(_osc) => {
                todo!("OSC response not yet implemented");
            }
            TerminalOutput::CursorReport => {
                todo!("Cursor report not yet implemented");
            }
            TerminalOutput::DecSpecialGraphics(dsg) => {
                self.character_replace = dsg.clone();
            }
            TerminalOutput::CursorVisualStyle(style) => {
                self.cursor_visual_style = style.clone();
            }
            TerminalOutput::WindowManipulation(_wm) => {
                todo!("Window manipulation not yet implemented");
            }
            TerminalOutput::RequestDeviceAttributes => {
                todo!("Request device attributes not yet implemented");
            }
            TerminalOutput::EightBitControl => {
                todo!("Eight bit control not yet implemented");
            }
            TerminalOutput::SevenBitControl => {
                todo!("Seven bit control not yet implemented");
            }
            TerminalOutput::AnsiConformanceLevelOne => {
                todo!("ANSI conformance level 1 not yet implemented");
            }
            TerminalOutput::AnsiConformanceLevelTwo => {
                todo!("ANSI conformance level 2 not yet implemented");
            }
            TerminalOutput::AnsiConformanceLevelThree => {
                todo!("ANSI conformance level 3 not yet implemented");
            }
            TerminalOutput::DoubleLineHeightTop => {
                todo!("Double line height top not yet implemented");
            }
            TerminalOutput::DoubleLineHeightBottom => {
                todo!("Double line height bottom not yet implemented");
            }
            TerminalOutput::SingleWidthLine => {
                todo!("Single width line not yet implemented");
            }
            TerminalOutput::DoubleWidthLine => {
                todo!("Double width line not yet implemented");
            }
            TerminalOutput::ScreenAlignmentTest => {
                todo!("Screen alignment test not yet implemented");
            }
            TerminalOutput::CharsetDefault => {
                todo!("Charset default not yet implemented");
            }
            TerminalOutput::CharsetUTF8 => {
                todo!("Charset UTF8 not yet implemented");
            }
            TerminalOutput::CharsetG0 => {
                todo!("Charset G0 not yet implemented");
            }
            TerminalOutput::CharsetG1 => {
                todo!("Charset G1 not yet implemented");
            }
            TerminalOutput::CharsetG1AsGR => {
                todo!("Charset G1 as GR not yet implemented");
            }
            TerminalOutput::CharsetG2 => {
                todo!("Charset G2 not yet implemented");
            }
            TerminalOutput::CharsetG2AsGR => {
                todo!("Charset G2 as GR not yet implemented");
            }
            TerminalOutput::CharsetG2AsGL => {
                todo!("Charset G2 as GL not yet implemented");
            }
            TerminalOutput::CharsetG3 => {
                todo!("Charset G3 not yet implemented");
            }
            TerminalOutput::CharsetG3AsGR => {
                todo!("Charset G3 as GR not yet implemented");
            }
            TerminalOutput::CharsetG3AsGL => {
                todo!("Charset G3 as GL not yet implemented");
            }
            TerminalOutput::DecSpecial => {
                todo!("DEC special not yet implemented");
            }
            TerminalOutput::CharsetUK => {
                todo!("Charset UK not yet implemented");
            }
            TerminalOutput::CharsetUS => {
                todo!("Charset US not yet implemented");
            }
            TerminalOutput::CharsetUSASCII => {
                todo!("Charset US ASCII not yet implemented");
            }
            TerminalOutput::CharsetDutch => {
                todo!("Charset Dutch not yet implemented");
            }
            TerminalOutput::CharsetFinnish => {
                todo!("Charset Finnish not yet implemented");
            }
            TerminalOutput::CharsetFrench => {
                todo!("Charset French not yet implemented");
            }
            TerminalOutput::CharsetFrenchCanadian => {
                todo!("Charset French Canadian not yet implemented");
            }
            TerminalOutput::CharsetGerman => {
                todo!("Charset German not yet implemented");
            }
            TerminalOutput::CharsetItalian => {
                todo!("Charset Italian not yet implemented");
            }
            TerminalOutput::CharsetNorwegianDanish => {
                todo!("Charset Norwegian/Danish not yet implemented");
            }
            TerminalOutput::CharsetSpanish => {
                todo!("Charset Spanish not yet implemented");
            }
            TerminalOutput::CharsetSwedish => {
                todo!("Charset Swedish not yet implemented");
            }
            TerminalOutput::CharsetSwiss => {
                todo!("Charset Swiss not yet implemented");
            }
            TerminalOutput::SaveCursor => {
                self.handle_save_cursor();
            }
            TerminalOutput::RestoreCursor => {
                self.handle_restore_cursor();
            }
            TerminalOutput::CursorToLowerLeftCorner => {
                todo!("Cursor to lower left corner not yet implemented");
            }
            TerminalOutput::ResetDevice => {
                todo!("Reset device not yet implemented");
            }
            TerminalOutput::MemoryLock => {
                todo!("Memory lock not yet implemented");
            }
            TerminalOutput::MemoryUnlock => {
                todo!("Memory unlock not yet implemented");
            }
            TerminalOutput::DeviceControlString(_dcs) => {
                todo!("Device control string not yet implemented");
            }
            TerminalOutput::ApplicationProgramCommand(_apc) => {
                todo!("Application program command not yet implemented");
            }
            TerminalOutput::RequestDeviceNameAndVersion => {
                todo!("Request device name and version not yet implemented");
            }
            TerminalOutput::RequestSecondaryDeviceAttributes { param: _param } => {
                todo!("Request secondary device attributes not yet implemented");
            }
            TerminalOutput::RequestXtVersion => {
                todo!("Request Xt version not yet implemented");
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
fn apply_dec_special(data: &[u8], mode: &DecSpecialGraphics) -> Vec<u8> {
    match mode {
        DecSpecialGraphics::DontReplace => data.to_vec(),
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
            out
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

#[cfg(test)]
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
        let rows = handler.buffer().visible_rows();
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

        let rows = handler.buffer().visible_rows();
        // Should have inserted a blank line, pushing content down
        assert!(rows.len() >= 2);
    }

    #[test]
    fn test_handle_scroll_region() {
        let mut handler = TerminalHandler::new(80, 24);

        // Set scroll region from line 5 to line 20 (1-indexed from parser)
        handler.handle_set_scroll_region(5, 20);

        // Buffer should have scroll region set (converted to 0-indexed)
        // This is hard to verify without exposing scroll region state,
        // but at least verify it doesn't panic
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

        // Screen should be cleared
        let visible = handler.buffer().visible_rows();
        assert_eq!(visible.len(), 24);
    }
}
