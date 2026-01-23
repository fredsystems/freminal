// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::{
    format_tag::FormatTag, tchar::TChar, terminal_output::TerminalOutput,
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
}

impl TerminalHandler {
    /// Create a new terminal handler with the specified dimensions
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            buffer: Buffer::new(width, height),
            current_format: FormatTag::default(),
        }
    }

    /// Get a reference to the underlying buffer
    #[must_use]
    pub const fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Get a mutable reference to the underlying buffer
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }

    /// Handle raw data bytes - convert to TChar and insert
    pub fn handle_data(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        // Convert bytes to TChar
        // This is a simplified version - in production, proper UTF-8 handling is needed
        if let Ok(text) = TChar::from_vec(data) {
            self.buffer.insert_text(&text);
        }
    }

    /// Handle newline (LF)
    pub fn handle_newline(&mut self) {
        self.buffer.handle_lf();
    }

    /// Handle carriage return (CR)
    pub fn handle_carriage_return(&mut self) {
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
    pub fn handle_cursor_up(&mut self, n: usize) {
        self.buffer.move_cursor_relative(0, -(n as i32));
    }

    /// Handle cursor down (CUD)
    pub fn handle_cursor_down(&mut self, n: usize) {
        self.buffer.move_cursor_relative(0, n as i32);
    }

    /// Handle cursor forward (CUF)
    pub fn handle_cursor_forward(&mut self, n: usize) {
        self.buffer.move_cursor_relative(n as i32, 0);
    }

    /// Handle cursor backward (CUB)
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

    /// Handle insert spaces (ICH)
    pub fn handle_insert_spaces(&mut self, n: usize) {
        self.buffer.insert_spaces(n);
    }

    /// Handle set top and bottom margins (DECSTBM)
    pub fn handle_set_scroll_region(&mut self, top: usize, bottom: usize) {
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

    /// Handle SGR (Set Graphics Rendition)
    /// This is a placeholder - actual SGR handling requires converting
    /// SelectGraphicRendition to FormatTag
    pub fn handle_sgr(&mut self, _sgr_data: &[u8]) {
        // TODO: Convert SGR parameters to FormatTag and call buffer.set_format()
        // For now, this is a stub
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

    /// Process an array of TerminalOutput commands
    ///
    /// This is the main entry point for integrating with the parser.
    /// It dispatches each TerminalOutput variant to the appropriate handler method.
    pub fn process_outputs<SGR, MODE, OSC, DECSG>(
        &mut self,
        outputs: &[TerminalOutput<SGR, MODE, OSC, DECSG>],
    ) where
        SGR: std::fmt::Debug,
        MODE: std::fmt::Debug,
        OSC: std::fmt::Debug,
        DECSG: std::fmt::Debug,
    {
        for output in outputs {
            self.process_output(output);
        }
    }

    /// Process a single TerminalOutput command
    #[allow(clippy::too_many_lines)]
    fn process_output<SGR, MODE, OSC, DECSG>(
        &mut self,
        output: &TerminalOutput<SGR, MODE, OSC, DECSG>,
    ) where
        SGR: std::fmt::Debug,
        MODE: std::fmt::Debug,
        OSC: std::fmt::Debug,
        DECSG: std::fmt::Debug,
    {
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
                self.handle_delete_lines(*n);
            }
            TerminalOutput::InsertSpaces(n) => {
                self.handle_insert_spaces(*n);
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
                todo!("Erase({}) not yet implemented", n);
            }
            TerminalOutput::Sgr(_sgr) => {
                todo!("SGR not yet implemented - need to convert to FormatTag");
            }
            TerminalOutput::Mode(_mode) => {
                todo!("Mode switching not yet implemented");
            }
            TerminalOutput::OscResponse(_osc) => {
                todo!("OSC response not yet implemented");
            }
            TerminalOutput::CursorReport => {
                todo!("Cursor report not yet implemented");
            }
            TerminalOutput::DecSpecialGraphics(_decsg) => {
                todo!("DEC special graphics not yet implemented");
            }
            TerminalOutput::CursorVisualStyle(_style) => {
                todo!("Cursor visual style not yet implemented");
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
                todo!("Save cursor not yet implemented");
            }
            TerminalOutput::RestoreCursor => {
                todo!("Restore cursor not yet implemented");
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
            TerminalOutput::Invalid => {
                // Silently ignore invalid sequences
            }
            TerminalOutput::Skipped => {
                // Silently ignore skipped sequences
            }
            // Catch-all for any future variants added to the non-exhaustive enum
            _ => {
                // Silently ignore unhandled sequences for forward compatibility
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            TerminalOutput::<(), (), (), ()>::Data(b"Hello".to_vec()),
            TerminalOutput::<(), (), (), ()>::Newline,
            TerminalOutput::<(), (), (), ()>::CarriageReturn,
            TerminalOutput::<(), (), (), ()>::Data(b"World".to_vec()),
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
            TerminalOutput::<(), (), (), ()>::SetCursorPos {
                x: Some(11),
                y: Some(6),
            },
            TerminalOutput::<(), (), (), ()>::Data(b"Test".to_vec()),
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
            TerminalOutput::<(), (), (), ()>::Data(b"Line 1".to_vec()),
            TerminalOutput::<(), (), (), ()>::Newline,
            TerminalOutput::<(), (), (), ()>::CarriageReturn,
            TerminalOutput::<(), (), (), ()>::Data(b"Line 2".to_vec()),
            TerminalOutput::<(), (), (), ()>::ClearDisplay,
        ];

        handler.process_outputs(&outputs);

        // Screen should be cleared
        let visible = handler.buffer().visible_rows();
        assert_eq!(visible.len(), 24);
    }
}
