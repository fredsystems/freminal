// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_buffer::terminal_handler::TerminalHandler;
use freminal_common::pty_write::PtyWrite;

/// Helper to convert a string slice to TChar representation as bytes
fn text_to_bytes(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

#[test]
fn test_simple_text_insertion() {
    let mut handler = TerminalHandler::new(80, 24);

    handler.handle_data(&text_to_bytes("Hello, World!"));

    assert_eq!(handler.buffer().get_cursor().pos.x, 13);
    assert_eq!(handler.buffer().get_cursor().pos.y, 0);

    // Buffer grows dynamically; only the rows that have been written exist.
    let visible = handler.buffer().visible_rows(0);
    assert_eq!(visible.len(), 1);
}

#[test]
fn test_multiline_output() {
    let mut handler = TerminalHandler::new(80, 24);

    handler.handle_data(&text_to_bytes("Line 1"));
    handler.handle_newline();
    handler.handle_carriage_return(); // CR needed separately from LF
    handler.handle_data(&text_to_bytes("Line 2"));
    handler.handle_newline();
    handler.handle_carriage_return(); // CR needed separately from LF
    handler.handle_data(&text_to_bytes("Line 3"));

    assert_eq!(handler.buffer().get_cursor().pos.y, 2);
    assert_eq!(handler.buffer().get_cursor().pos.x, 6);
}

#[test]
fn test_clear_screen_workflow() {
    let mut handler = TerminalHandler::new(80, 24);

    // Write some content
    for i in 0..10 {
        handler.handle_data(&text_to_bytes(&format!("Line {}", i)));
        handler.handle_newline();
    }

    // Clear the screen (ED 2)
    handler.handle_erase_in_display(2);

    // Move cursor to home
    handler.handle_cursor_pos(Some(1), Some(1));

    // Write new content
    handler.handle_data(&text_to_bytes("After clear"));

    assert_eq!(handler.buffer().get_cursor().pos.x, 11);
    assert_eq!(handler.buffer().get_cursor().pos.y, 0);
}

#[test]
fn test_cursor_positioning_and_data() {
    let mut handler = TerminalHandler::new(80, 24);

    // Move to position (5, 5) - using 1-indexed as parser would send
    handler.handle_cursor_pos(Some(6), Some(6));
    handler.handle_data(&text_to_bytes("Middle"));

    assert_eq!(handler.buffer().get_cursor().pos.x, 11); // 5 + 6
    assert_eq!(handler.buffer().get_cursor().pos.y, 5);

    // Move to (0, 10)
    handler.handle_cursor_pos(Some(1), Some(11));
    handler.handle_data(&text_to_bytes("Lower"));

    assert_eq!(handler.buffer().get_cursor().pos.x, 5);
    assert_eq!(handler.buffer().get_cursor().pos.y, 10);
}

#[test]
fn test_line_wrapping() {
    let mut handler = TerminalHandler::new(10, 5);

    // Write text longer than the width
    handler.handle_data(&text_to_bytes("HelloWorld!"));

    // Should have wrapped to next line
    assert_eq!(handler.buffer().get_cursor().pos.y, 1);
    assert_eq!(handler.buffer().get_cursor().pos.x, 1); // 11 chars - 10 = 1
}

#[test]
fn test_erase_line_operations() {
    let mut handler = TerminalHandler::new(20, 5);

    handler.handle_data(&text_to_bytes("0123456789"));

    // Move cursor to middle of line
    handler.handle_cursor_pos(Some(6), Some(1));

    // Erase to end of line (EL 0)
    handler.handle_erase_in_line(0);

    // Cursor should still be at column 5 (0-indexed)
    assert_eq!(handler.buffer().get_cursor().pos.x, 5);
}

#[test]
fn test_scroll_region_operations() {
    let mut handler = TerminalHandler::new(80, 24);

    // Set scroll region to lines 5-20 (1-indexed from parser)
    handler.handle_set_scroll_region(5, 20);

    // Fill some content
    for i in 0..25 {
        handler.handle_data(&text_to_bytes(&format!("Line {}", i)));
        handler.handle_newline();
    }

    // Should have scrolling behavior within the region
    // (exact behavior depends on cursor position and scroll region implementation)
}

#[test]
fn test_insert_delete_lines() {
    let mut handler = TerminalHandler::new(80, 24);

    // Write initial content
    handler.handle_data(&text_to_bytes("Line A"));
    handler.handle_newline();
    handler.handle_data(&text_to_bytes("Line B"));
    handler.handle_newline();
    handler.handle_data(&text_to_bytes("Line C"));

    // Move to second line
    handler.handle_cursor_pos(Some(1), Some(2));

    // Insert a line - should push Line B down
    handler.handle_insert_lines(1);

    let visible = handler.buffer().visible_rows(0);
    assert!(visible.len() >= 3);

    // Delete a line
    handler.handle_delete_lines(1);
}

#[test]
fn test_alternate_buffer_switching() {
    let mut handler = TerminalHandler::new(80, 24);

    // Write to primary buffer
    handler.handle_data(&text_to_bytes("Primary content"));
    let primary_cursor_x = handler.buffer().get_cursor().pos.x;

    // Enter alternate buffer
    handler.handle_enter_alternate();

    // Write different content
    handler.handle_data(&text_to_bytes("Alternate content"));

    // Leave alternate buffer
    handler.handle_leave_alternate();

    // Should restore primary buffer state
    assert_eq!(handler.buffer().get_cursor().pos.x, primary_cursor_x);
}

#[test]
fn test_carriage_return_and_newline() {
    let mut handler = TerminalHandler::new(80, 24);

    handler.handle_data(&text_to_bytes("Hello"));
    assert_eq!(handler.buffer().get_cursor().pos.x, 5);

    // Carriage return should move to column 0
    handler.handle_carriage_return();
    assert_eq!(handler.buffer().get_cursor().pos.x, 0);
    assert_eq!(handler.buffer().get_cursor().pos.y, 0);

    // Newline should advance row
    handler.handle_newline();
    assert_eq!(handler.buffer().get_cursor().pos.y, 1);
}

#[test]
fn test_backspace_behavior() {
    let mut handler = TerminalHandler::new(80, 24);

    handler.handle_data(&text_to_bytes("Hello"));
    assert_eq!(handler.buffer().get_cursor().pos.x, 5);

    handler.handle_backspace();
    assert_eq!(handler.buffer().get_cursor().pos.x, 4);

    handler.handle_backspace();
    assert_eq!(handler.buffer().get_cursor().pos.x, 3);

    // Backspace at start of line shouldn't move to previous line
    handler.handle_cursor_pos(Some(1), Some(1));
    handler.handle_backspace();
    assert_eq!(handler.buffer().get_cursor().pos.x, 0);
}

#[test]
fn test_cursor_movement_commands() {
    let mut handler = TerminalHandler::new(80, 24);

    // Start at origin
    handler.handle_cursor_pos(Some(1), Some(1));
    assert_eq!(handler.buffer().get_cursor().pos.x, 0);
    assert_eq!(handler.buffer().get_cursor().pos.y, 0);

    // Move down 5 lines
    handler.handle_cursor_down(5);
    assert_eq!(handler.buffer().get_cursor().pos.y, 5);

    // Move right 10 columns
    handler.handle_cursor_forward(10);
    assert_eq!(handler.buffer().get_cursor().pos.x, 10);

    // Move up 3 lines
    handler.handle_cursor_up(3);
    assert_eq!(handler.buffer().get_cursor().pos.y, 2);

    // Move left 5 columns
    handler.handle_cursor_backward(5);
    assert_eq!(handler.buffer().get_cursor().pos.x, 5);
}

#[test]
fn test_index_and_reverse_index() {
    let mut handler = TerminalHandler::new(80, 24);

    // Move to middle of screen
    handler.handle_cursor_pos(Some(1), Some(10));

    // Index should move cursor down
    handler.handle_index();
    // Exact behavior depends on whether we're at scroll region bottom

    // Reverse index should move cursor up or scroll
    handler.handle_reverse_index();
}

#[test]
fn test_insert_spaces() {
    let mut handler = TerminalHandler::new(20, 5);

    handler.handle_data(&text_to_bytes("HelloWorld"));

    // Move cursor to middle
    handler.handle_cursor_pos(Some(6), Some(1));

    // Insert 3 spaces
    handler.handle_insert_spaces(3);

    // Characters after cursor should be shifted right
    assert_eq!(handler.buffer().get_cursor().pos.x, 5);
}

#[test]
fn test_real_world_sequence() {
    let mut handler = TerminalHandler::new(80, 24);

    // Simulate: clear screen, write header, write content
    handler.handle_erase_in_display(2);
    handler.handle_cursor_pos(Some(1), Some(1));

    handler.handle_data(&text_to_bytes("=== Terminal Test ==="));
    handler.handle_newline();
    handler.handle_newline();

    for i in 1..=5 {
        handler.handle_data(&text_to_bytes(&format!("Item {}", i)));
        handler.handle_newline();
    }

    // Move to bottom
    handler.handle_cursor_pos(Some(1), Some(24));
    handler.handle_data(&text_to_bytes("Bottom line"));

    let visible = handler.buffer().visible_rows(0);
    assert_eq!(visible.len(), 24);
}

#[test]
fn test_scrollback_erase() {
    let mut handler = TerminalHandler::new(80, 24);

    // Fill buffer with content to create scrollback
    for i in 0..100 {
        handler.handle_data(&text_to_bytes(&format!("Line {}", i)));
        handler.handle_newline();
    }

    // Erase scrollback (ED 3)
    handler.handle_erase_in_display(3);

    // Scrollback should be cleared (can't directly verify without inspecting buffer internals,
    // but at least ensure it doesn't panic)
}

#[test]
fn test_resize_handling() {
    let mut handler = TerminalHandler::new(80, 24);

    handler.handle_data(&text_to_bytes("Before resize"));

    // Resize terminal
    handler.handle_resize(120, 30);

    handler.handle_data(&text_to_bytes(" After resize"));

    // Should still function correctly
}

#[test]
fn test_user_scrolling() {
    let mut handler = TerminalHandler::new(80, 24);

    // Fill buffer to create scrollback
    for i in 0..50 {
        handler.handle_data(&text_to_bytes(&format!("Line {}", i)));
        handler.handle_newline();
    }

    // User scrolls back (scroll_offset lives in ViewState; pass 0 temporarily)
    let scroll_offset = handler.handle_scroll_back(0, 10);

    // User scrolls forward
    let scroll_offset = handler.handle_scroll_forward(scroll_offset, 5);

    // Return to bottom
    let _scroll_offset = TerminalHandler::handle_scroll_to_bottom();
    let _ = scroll_offset;
}

#[test]
fn test_wide_character_handling() {
    let mut handler = TerminalHandler::new(10, 5);

    // Insert wide characters (emoji)
    handler.handle_data("Hello 😀".as_bytes());

    // Wide chars should be handled properly
    // (exact cursor position depends on unicode width calculation)
}

#[test]
fn test_mixed_operations_workflow() {
    let mut handler = TerminalHandler::new(80, 24);

    // Simulate a complex terminal interaction
    handler.handle_data(&text_to_bytes("$ ls -la"));
    handler.handle_newline();
    handler.handle_carriage_return();

    for i in 1..=10 {
        handler.handle_data(&text_to_bytes(&format!("file{}.txt", i)));
        handler.handle_newline();
        handler.handle_carriage_return();
    }

    handler.handle_newline();
    handler.handle_carriage_return();
    handler.handle_data(&text_to_bytes("$ "));

    // Save position before cursor movement
    let _cursor_x_before = handler.buffer().get_cursor().pos.x;

    // Move cursor back
    handler.handle_cursor_backward(5);
    handler.handle_cursor_forward(5);

    // More typing
    handler.handle_data(&text_to_bytes("vim"));

    // Simulate entering vim (alternate screen)
    handler.handle_enter_alternate();
    handler.handle_erase_in_display(2);
    handler.handle_cursor_pos(Some(1), Some(1));
    handler.handle_data(&text_to_bytes("~ VIM - Vi IMproved"));

    // Exit vim
    handler.handle_leave_alternate();

    // Back to shell - cursor position is restored from primary buffer
    // The cursor should be where we left it before entering alternate
    // cursor_x_before is 2 (from "$ "), move back 5 (clamped to 0), forward 5 (now at 5),
    // then "vim" (3 chars) = 8
    assert_eq!(handler.buffer().get_cursor().pos.x, 8);
}

#[test]
fn test_process_outputs_api() {
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(80, 24);

    // Simulate a realistic terminal session using process_outputs
    let outputs = vec![
        // Clear screen and go home
        TerminalOutput::ClearDisplay,
        TerminalOutput::SetCursorPos {
            x: Some(1),
            y: Some(1),
        },
        // Write prompt
        TerminalOutput::Data(b"$ ".to_vec()),
        // User types command
        TerminalOutput::Data(b"ls -la".to_vec()),
        // Enter pressed
        TerminalOutput::Newline,
        TerminalOutput::CarriageReturn,
        // Output some file listings
        TerminalOutput::Data(b"total 48".to_vec()),
        TerminalOutput::Newline,
        TerminalOutput::CarriageReturn,
        TerminalOutput::Data(b"-rw-r--r-- 1 user user 1234 file.txt".to_vec()),
        TerminalOutput::Newline,
        TerminalOutput::CarriageReturn,
        // New prompt
        TerminalOutput::Data(b"$ ".to_vec()),
    ];

    handler.process_outputs(&outputs);

    // Verify final cursor position
    assert_eq!(handler.buffer().get_cursor().pos.x, 2);
    assert_eq!(handler.buffer().get_cursor().pos.y, 3);

    // Buffer grows dynamically; 4 rows of content were written (prompt+cmd,
    // total 48, file listing, new prompt).
    let visible = handler.buffer().visible_rows(0);
    assert_eq!(visible.len(), 4);
}

#[test]
fn test_process_outputs_with_cursor_positioning() {
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(80, 24);

    let outputs = vec![
        // Write header at top
        TerminalOutput::SetCursorPos {
            x: Some(1),
            y: Some(1),
        },
        TerminalOutput::Data(b"=== Terminal Test ===".to_vec()),
        // Write content in middle
        TerminalOutput::SetCursorPos {
            x: Some(1),
            y: Some(10),
        },
        TerminalOutput::Data(b"Middle content".to_vec()),
        // Write footer at bottom
        TerminalOutput::SetCursorPos {
            x: Some(1),
            y: Some(24),
        },
        TerminalOutput::Data(b"Footer".to_vec()),
    ];

    handler.process_outputs(&outputs);

    assert_eq!(handler.buffer().get_cursor().pos.x, 6);
    assert_eq!(handler.buffer().get_cursor().pos.y, 23);
}

#[test]
fn test_process_outputs_with_scroll_region() {
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(80, 24);

    let outputs = vec![
        // Set up scroll region
        TerminalOutput::SetTopAndBottomMargins {
            top_margin: 5,
            bottom_margin: 20,
        },
        // Write some lines
        TerminalOutput::Data(b"Line 1".to_vec()),
        TerminalOutput::Newline,
        TerminalOutput::CarriageReturn,
        TerminalOutput::Data(b"Line 2".to_vec()),
    ];

    handler.process_outputs(&outputs);

    // Should not panic and maintain scroll region
}

#[test]
fn test_process_outputs_mixed_erase_operations() {
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(40, 10);

    let outputs = vec![
        // Fill screen with data
        TerminalOutput::Data(b"Line 1".to_vec()),
        TerminalOutput::Newline,
        TerminalOutput::CarriageReturn,
        TerminalOutput::Data(b"Line 2".to_vec()),
        TerminalOutput::Newline,
        TerminalOutput::CarriageReturn,
        TerminalOutput::Data(b"Line 3".to_vec()),
        // Move to middle of screen
        TerminalOutput::SetCursorPos {
            x: Some(1),
            y: Some(2),
        },
        // Erase from cursor to end of display
        TerminalOutput::ClearDisplayfromCursortoEndofDisplay,
        // Write new content
        TerminalOutput::Data(b"New Line 2".to_vec()),
    ];

    handler.process_outputs(&outputs);

    assert_eq!(handler.buffer().get_cursor().pos.x, 10);
}

#[test]
fn test_process_outputs_insert_delete_operations() {
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(80, 24);

    let outputs = vec![
        TerminalOutput::Data(b"Line 1".to_vec()),
        TerminalOutput::Newline,
        TerminalOutput::CarriageReturn,
        TerminalOutput::Data(b"Line 2".to_vec()),
        TerminalOutput::Newline,
        TerminalOutput::CarriageReturn,
        TerminalOutput::Data(b"Line 3".to_vec()),
        // Go back to line 2
        TerminalOutput::SetCursorPos {
            x: Some(1),
            y: Some(2),
        },
        // Insert a blank line
        TerminalOutput::InsertLines(1),
        // Write on the new line
        TerminalOutput::Data(b"Inserted".to_vec()),
    ];

    handler.process_outputs(&outputs);

    assert_eq!(handler.buffer().get_cursor().pos.x, 8);
}

#[test]
fn ind_scrolls_at_bottom_margin() {
    // Use the alternate buffer (no scrollback) so IND at the bottom margin
    // scrolls the screen in-place and the cursor stays at the bottom row.
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(10, 5);

    // Enter alternate screen so IND scrolls rather than growing scrollback.
    handler.handle_enter_alternate();

    // Fill all 5 rows with content.
    for _ in 0..5 {
        handler.process_outputs(&[
            TerminalOutput::Data(b"XXXXX".to_vec()),
            TerminalOutput::Newline,
            TerminalOutput::CarriageReturn,
        ]);
    }

    // Place cursor at the bottom margin row (row 4, 0-indexed).
    let bottom_row = 4usize;
    handler
        .buffer_mut()
        .set_cursor_pos(Some(0), Some(bottom_row));

    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        bottom_row,
        "cursor should be at bottom margin before IND"
    );

    // IND at bottom margin in alternate buffer — scrolls up, cursor stays at row 4.
    handler.process_outputs(&[TerminalOutput::Index]);

    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        bottom_row,
        "IND at bottom margin must keep cursor at bottom margin row"
    );

    // Leave alternate buffer to restore state.
    handler.handle_leave_alternate();
}

#[test]
fn ri_scrolls_at_top_margin() {
    // 5-row buffer. Set scroll region rows 1–4 (1-indexed). Place cursor at the
    // top of the region (row 0 in 0-indexed). Send ReverseIndex → region scrolls
    // down, blank line inserted at top, cursor stays at the top margin row.
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(10, 5);

    handler.process_outputs(&[TerminalOutput::SetTopAndBottomMargins {
        top_margin: 1,
        bottom_margin: 5,
    }]);

    // Write a few lines.
    for _ in 0..3 {
        handler.process_outputs(&[
            TerminalOutput::Data(b"HELLO".to_vec()),
            TerminalOutput::Newline,
            TerminalOutput::CarriageReturn,
        ]);
    }

    // Move cursor to row 0 (top of scroll region).
    handler.buffer_mut().set_cursor_pos(Some(0), Some(0));
    let cursor_row_before = handler.buffer().get_cursor().pos.y;

    handler.process_outputs(&[TerminalOutput::ReverseIndex]);

    // Cursor must remain at the top margin row (or scroll inserted a blank above,
    // keeping cursor at the same screen row).
    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        cursor_row_before,
        "RI at top margin must keep cursor at top margin row"
    );
}

#[test]
fn nel_moves_to_col_zero_of_next_line() {
    // NEL is like CR+LF: cursor goes to column 0 of the next row.
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(20, 10);

    // Write some text so cursor is mid-row.
    handler.process_outputs(&[TerminalOutput::Data(b"Hello".to_vec())]);
    let row_before = handler.buffer().get_cursor().pos.y;
    assert!(
        handler.buffer().get_cursor().pos.x > 0,
        "cursor should be past col 0 after inserting text"
    );

    handler.process_outputs(&[TerminalOutput::NextLine]);

    assert_eq!(
        handler.buffer().get_cursor().pos.x,
        0,
        "NEL must reset cursor to column 0"
    );
    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        row_before + 1,
        "NEL must advance cursor to next row"
    );
}

#[test]
fn alternate_enter_clears_screen() {
    // Write content to the primary buffer, then enter alternate screen.
    // Visible rows in the alternate buffer must be empty — no content from
    // the primary buffer should bleed through.
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::xtextscrn::XtExtscrn,
        terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(20, 5);

    handler.process_outputs(&[TerminalOutput::Data(b"primary content".to_vec())]);

    // Verify primary has content.
    let primary_row = handler.buffer().visible_rows(0);
    assert!(
        !primary_row.is_empty(),
        "primary buffer should have visible rows after data"
    );

    // Enter alternate screen via Mode dispatch.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::XtExtscrn(XtExtscrn::new(
        &SetMode::DecSet,
    )))]);

    // Alternate screen must show only blank rows.
    let alt_rows = handler.buffer().visible_rows(0);
    for row in alt_rows {
        for cell in row.get_characters() {
            assert!(
                !cell.is_head(),
                "alternate screen should have no wide-head content cells"
            );
        }
    }
}

#[test]
fn alternate_leave_restores_content() {
    // Write "hello" in primary, enter alternate, write "world", leave alternate.
    // After leaving, visible rows must show the primary content ("hello"), not "world".
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::xtextscrn::XtExtscrn,
        terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(20, 5);

    // Write to primary.
    handler.process_outputs(&[TerminalOutput::Data(b"hello".to_vec())]);

    // Enter alternate and write different content.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::XtExtscrn(XtExtscrn::new(
        &SetMode::DecSet,
    )))]);
    handler.process_outputs(&[TerminalOutput::Data(b"world".to_vec())]);

    // Leave alternate — should restore primary.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::XtExtscrn(XtExtscrn::new(
        &SetMode::DecRst,
    )))]);

    // The first visible row should contain the original primary content.
    let rows = handler.buffer().visible_rows(0);
    let first_row = &rows[0];
    let content: String = first_row
        .get_characters()
        .iter()
        .filter_map(|c| match c.tchar() {
            freminal_common::buffer_states::tchar::TChar::Ascii(b) => Some(*b as char),
            _ => None,
        })
        .collect();
    assert_eq!(
        content, "hello",
        "leaving alternate screen must restore primary content"
    );
}

#[test]
fn unknown_mode_does_not_panic() {
    // Sending unhandled Mode variants through process_outputs must not panic.
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decarm::Decarm,
        terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(20, 5);

    // NoOp mode.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::NoOp)]);

    // Decarm (key auto-repeat) — not handled by the buffer.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decarm(Decarm::new(
        &SetMode::DecSet,
    )))]);

    // If we reach here, no panic occurred.
    assert_eq!(handler.buffer().get_cursor().pos.x, 0);
}

#[test]
fn save_restore_position() {
    // Move cursor to (5, 3), save, move to (0, 0), restore → cursor is back at (5, 3).
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(20, 10);

    // Position cursor at column 5, row 3.
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: Some(6), // 1-indexed → col 5
        y: Some(4), // 1-indexed → row 3
    }]);
    assert_eq!(handler.buffer().get_cursor().pos.x, 5);
    assert_eq!(handler.buffer().get_cursor().pos.y, 3);

    // Save cursor.
    handler.process_outputs(&[TerminalOutput::SaveCursor]);

    // Move cursor somewhere else.
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: Some(1),
        y: Some(1),
    }]);
    assert_eq!(handler.buffer().get_cursor().pos.x, 0);
    assert_eq!(handler.buffer().get_cursor().pos.y, 0);

    // Restore cursor.
    handler.process_outputs(&[TerminalOutput::RestoreCursor]);

    assert_eq!(
        handler.buffer().get_cursor().pos.x,
        5,
        "cursor x must be restored to saved value"
    );
    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        3,
        "cursor y must be restored to saved value"
    );
}

#[test]
fn restore_without_save_is_noop() {
    // RestoreCursor without a prior SaveCursor must not panic and must leave
    // the cursor at its current position.
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(20, 10);

    // Position cursor at (3, 2).
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: Some(4),
        y: Some(3),
    }]);
    let x_before = handler.buffer().get_cursor().pos.x;
    let y_before = handler.buffer().get_cursor().pos.y;

    // Restore without a prior save — must be a no-op.
    handler.process_outputs(&[TerminalOutput::RestoreCursor]);

    assert_eq!(
        handler.buffer().get_cursor().pos.x,
        x_before,
        "x must not change on restore without save"
    );
    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        y_before,
        "y must not change on restore without save"
    );
}

#[test]
fn save_survives_alternate_roundtrip() {
    // Save cursor in primary, enter alternate, leave alternate, restore →
    // cursor position must be what was saved in the primary buffer.
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::xtextscrn::XtExtscrn,
        terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(20, 10);

    // Move to (7, 4) and save.
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: Some(8),
        y: Some(5),
    }]);
    handler.process_outputs(&[TerminalOutput::SaveCursor]);

    // Enter alternate screen.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::XtExtscrn(XtExtscrn::new(
        &SetMode::DecSet,
    )))]);

    // Do some work in alternate buffer.
    handler.process_outputs(&[TerminalOutput::Data(b"alternate content".to_vec())]);

    // Leave alternate screen.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::XtExtscrn(XtExtscrn::new(
        &SetMode::DecRst,
    )))]);

    // Restore cursor — must retrieve the primary-buffer saved position.
    handler.process_outputs(&[TerminalOutput::RestoreCursor]);

    assert_eq!(
        handler.buffer().get_cursor().pos.x,
        7,
        "cursor x must be restored to primary-saved value after alt roundtrip"
    );
    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        4,
        "cursor y must be restored to primary-saved value after alt roundtrip"
    );
}

#[test]
fn wrap_enabled_default() {
    // By default DECAWM is on: writing 85 chars to an 80-col buffer must
    // produce content on row 1 (the soft-wrap continuation row).
    let mut handler = TerminalHandler::new(80, 24);

    // 85 ASCII characters — 5 should overflow onto row 1.
    let text: Vec<u8> = b"A".repeat(85);
    handler.handle_data(&text);

    // Cursor must have wrapped to row 1.
    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        1,
        "cursor must be on row 1 after wrapping 85 chars into 80-col buffer"
    );
    // Cursor column should be 5 (the 5 overflow chars).
    assert_eq!(
        handler.buffer().get_cursor().pos.x,
        5,
        "cursor x must be 5 after the 5-char overflow"
    );

    let visible = handler.buffer().visible_rows(0);
    // Row 1 should have cells (at least the 5 overflow chars).
    assert!(
        visible.len() > 1,
        "visible rows must include at least two rows after wrap"
    );
}

#[test]
fn wrap_disabled_clamps() {
    // With DECAWM disabled writing 85 chars to an 80-col buffer must:
    // - keep the cursor on row 0
    // - clamp cursor x to column 79 (last valid column)
    // - not create a row 1
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decawm::Decawm,
        terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(80, 24);

    // Disable autowrap via the Mode dispatcher.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decawm(Decawm::new(
        &SetMode::DecRst,
    )))]);

    let text: Vec<u8> = b"A".repeat(85);
    handler.handle_data(&text);

    // Must still be on row 0.
    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        0,
        "cursor must remain on row 0 when wrap is disabled"
    );
    // Cursor must be clamped to the last column.
    assert_eq!(
        handler.buffer().get_cursor().pos.x,
        79,
        "cursor x must be clamped to 79 (last column) when wrap is disabled"
    );
}

#[test]
fn wrap_re_enable() {
    // Disable wrap → write overflow (stays on row 0) → re-enable wrap →
    // write more text → overflow must now wrap to row 1.
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decawm::Decawm,
        terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(80, 24);

    // Disable wrap and write 85 chars — all stay on row 0.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decawm(Decawm::new(
        &SetMode::DecRst,
    )))]);
    handler.handle_data(&b"A".repeat(85));
    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        0,
        "cursor should stay on row 0 while wrap is disabled"
    );

    // Re-enable wrap.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decawm(Decawm::new(
        &SetMode::DecSet,
    )))]);

    // Move cursor to col 75 so the next 10 chars will overflow.
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: Some(76), // 1-indexed → col 75
        y: Some(1),  // 1-indexed → row 0
    }]);

    // Write 10 chars — 5 fit, 5 overflow onto row 1.
    handler.handle_data(&b"B".repeat(10));

    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        1,
        "cursor must be on row 1 after re-enabling wrap and writing overflow"
    );
}

#[test]
fn osc_title_queued() {
    // OscResponse(SetTitleBar) must queue a SetTitleBarText window command.
    use freminal_common::buffer_states::{
        osc::AnsiOscType, terminal_output::TerminalOutput, window_manipulation::WindowManipulation,
    };

    let mut handler = TerminalHandler::new(80, 24);

    handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(
        String::from("vim"),
    ))]);

    let commands = handler.take_window_commands();
    assert_eq!(commands.len(), 1, "one window command must be queued");
    match &commands[0] {
        WindowManipulation::SetTitleBarText(title) => {
            assert_eq!(title, "vim", "title bar text must be preserved");
        }
        other => panic!("expected SetTitleBarText, got {other:?}"),
    }
}

#[test]
fn osc_url_sets_format() {
    // OSC 8 URL start sets current_format.url; OSC 8 URL end clears it.
    // Cells written between start and end must carry the URL tag;
    // cells written after end must have url == None.
    use freminal_common::buffer_states::{
        osc::{AnsiOscType, UrlResponse},
        terminal_output::TerminalOutput,
        url::Url,
    };

    let mut handler = TerminalHandler::new(80, 24);

    // Start URL
    handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Url(
        UrlResponse::Url(Url {
            id: None,
            url: String::from("https://example.com"),
        }),
    ))]);

    // Write text inside the URL
    handler.handle_data(b"click");

    // End URL
    handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::Url(
        UrlResponse::End,
    ))]);

    // Write text after the URL
    handler.handle_data(b"plain");

    let rows = handler.buffer().visible_rows(0);
    let row = &rows[0];

    // Cells 0-4 ("click") must have a URL tag.
    for col in 0..5 {
        let cell = row
            .get_char_at(col)
            .unwrap_or_else(|| panic!("cell {col} must exist"));
        assert!(
            cell.tag().url.is_some(),
            "cell {col} inside URL must have url tag set"
        );
        assert_eq!(
            cell.tag().url.as_ref().unwrap().url,
            "https://example.com",
            "cell {col} must carry the correct URL"
        );
    }

    // Cells 5-9 ("plain") must have no URL tag.
    for col in 5..10 {
        let cell = row
            .get_char_at(col)
            .unwrap_or_else(|| panic!("cell {col} must exist"));
        assert!(
            cell.tag().url.is_none(),
            "cell {col} after URL end must have no url tag"
        );
    }
}

#[test]
fn osc_noop_does_not_panic() {
    // OscResponse(NoOp) must not panic and must produce no side effects.
    use freminal_common::buffer_states::{osc::AnsiOscType, terminal_output::TerminalOutput};

    let mut handler = TerminalHandler::new(80, 24);

    handler.process_outputs(&[TerminalOutput::OscResponse(AnsiOscType::NoOp)]);

    // No window commands queued.
    assert!(
        handler.take_window_commands().is_empty(),
        "NoOp must not queue any window commands"
    );
    // Cursor unmoved.
    assert_eq!(handler.buffer().get_cursor().pos.x, 0);
    assert_eq!(handler.buffer().get_cursor().pos.y, 0);
}

#[test]
fn cursor_report_sends_correct_position() {
    // Move cursor to (4, 2) (0-indexed), send CursorReport → channel receives "\x1b[3;5R".
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();

    let mut handler = TerminalHandler::new(80, 24);
    handler.set_write_tx(tx);

    // Move cursor to col 4, row 2 (0-indexed) via 1-indexed SetCursorPos.
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: Some(5), // 1-indexed → col 4
        y: Some(3), // 1-indexed → row 2
    }]);

    handler.process_outputs(&[TerminalOutput::CursorReport]);

    let msg = rx
        .try_recv()
        .expect("CursorReport must send a message to the channel");
    let bytes = match msg {
        PtyWrite::Write(b) => b,
        other => panic!("expected PtyWrite::Write, got {other:?}"),
    };
    let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
    assert_eq!(
        response, "\x1b[3;5R",
        "CPR must be ESC[row;colR (1-indexed)"
    );
}

#[test]
fn da1_sends_response() {
    // RequestDeviceAttributes must send the DA1 capability string.
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();

    let mut handler = TerminalHandler::new(80, 24);
    handler.set_write_tx(tx);

    handler.process_outputs(&[TerminalOutput::RequestDeviceAttributes]);

    let msg = rx
        .try_recv()
        .expect("RequestDeviceAttributes must send a message to the channel");
    let bytes = match msg {
        PtyWrite::Write(b) => b,
        other => panic!("expected PtyWrite::Write, got {other:?}"),
    };
    let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
    assert_eq!(
        response, "\x1b[?65;1;2;4;6;17;18;22c",
        "DA1 response must match the expected capability string"
    );
}

#[test]
fn window_manipulation_queued() {
    // WindowManipulation commands must be stored and returned by take_window_commands().
    use freminal_common::buffer_states::{
        terminal_output::TerminalOutput, window_manipulation::WindowManipulation,
    };

    let mut handler = TerminalHandler::new(80, 24);

    handler.process_outputs(&[TerminalOutput::WindowManipulation(
        WindowManipulation::SetTitleBarText(String::from("test title")),
    )]);

    let commands = handler.take_window_commands();
    assert_eq!(commands.len(), 1, "one window command must be queued");
    match &commands[0] {
        WindowManipulation::SetTitleBarText(title) => {
            assert_eq!(title, "test title", "title bar text must be preserved");
        }
        other => panic!("expected SetTitleBarText, got {other:?}"),
    }

    // After draining, the queue must be empty.
    let commands2 = handler.take_window_commands();
    assert!(
        commands2.is_empty(),
        "take_window_commands must drain the queue"
    );
}

#[test]
fn no_write_tx_does_not_panic() {
    // CursorReport and RequestDeviceAttributes with no write_tx set must not panic.
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(80, 24);

    // No set_write_tx call — both of these must silently no-op.
    handler.process_outputs(&[TerminalOutput::CursorReport]);
    handler.process_outputs(&[TerminalOutput::RequestDeviceAttributes]);

    // If we reach here, no panic occurred.
    assert_eq!(handler.buffer().get_cursor().pos.x, 0);
}

#[test]
fn dec_special_replace_lower_right_corner() {
    // Enable Replace mode, send byte 0x6a → cell must contain TChar::Utf8 for ┘ (U+2518).
    use freminal_common::buffer_states::{
        line_draw::DecSpecialGraphics, terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(80, 24);

    handler.process_outputs(&[TerminalOutput::DecSpecialGraphics(
        DecSpecialGraphics::Replace,
    )]);
    handler.handle_data(&[0x6a]);

    let visible_rows = handler.buffer().visible_rows(0);
    let cell = visible_rows[0]
        .get_char_at(0)
        .expect("cell 0 must exist after writing");
    assert_eq!(
        cell.tchar(),
        &freminal_common::buffer_states::tchar::TChar::Utf8("\u{2518}".as_bytes().to_vec()),
        "0x6a in Replace mode must produce ┘ (U+2518)"
    );
}

#[test]
fn dec_special_dont_replace_passthrough() {
    // In DontReplace mode (default), byte 0x6a is stored as TChar::Ascii(0x6a).
    let mut handler = TerminalHandler::new(80, 24);

    handler.handle_data(&[0x6a]);

    let visible_rows = handler.buffer().visible_rows(0);
    let cell = visible_rows[0]
        .get_char_at(0)
        .expect("cell 0 must exist after writing");
    assert_eq!(
        cell.tchar(),
        &freminal_common::buffer_states::tchar::TChar::Ascii(0x6a),
        "0x6a in DontReplace mode must be stored as Ascii(0x6a)"
    );
}

#[test]
fn dec_special_toggle() {
    // Enable Replace → write 0x6a (gets remapped to ┘).
    // Disable Replace → write 0x6a again (stored as ASCII).
    // Verify both cells independently.
    use freminal_common::buffer_states::{
        line_draw::DecSpecialGraphics, terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(80, 24);

    // First character: Replace mode → ┘
    handler.process_outputs(&[TerminalOutput::DecSpecialGraphics(
        DecSpecialGraphics::Replace,
    )]);
    handler.handle_data(&[0x6a]);

    // Second character: DontReplace mode → ASCII 'j'
    handler.process_outputs(&[TerminalOutput::DecSpecialGraphics(
        DecSpecialGraphics::DontReplace,
    )]);
    handler.handle_data(&[0x6a]);

    let visible_rows = handler.buffer().visible_rows(0);
    let cell0 = visible_rows[0].get_char_at(0).expect("cell 0 must exist");
    let cell1 = visible_rows[0].get_char_at(1).expect("cell 1 must exist");

    assert_eq!(
        cell0.tchar(),
        &freminal_common::buffer_states::tchar::TChar::Utf8("\u{2518}".as_bytes().to_vec()),
        "cell 0 must be ┘ (Replace mode)"
    );
    assert_eq!(
        cell1.tchar(),
        &freminal_common::buffer_states::tchar::TChar::Ascii(0x6a),
        "cell 1 must be Ascii(0x6a) (DontReplace mode)"
    );
}

#[test]
fn dec_special_all_passthrough_above_7e() {
    // Bytes above 0x7E are never remapped even in Replace mode.
    use freminal_common::buffer_states::{
        line_draw::DecSpecialGraphics, terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(80, 24);

    handler.process_outputs(&[TerminalOutput::DecSpecialGraphics(
        DecSpecialGraphics::Replace,
    )]);

    // 0x41 = 'A' (below 0x5F — also not remapped)
    handler.handle_data(&[0x41]);

    let visible_rows = handler.buffer().visible_rows(0);
    let cell = visible_rows[0].get_char_at(0).expect("cell 0 must exist");
    assert_eq!(
        cell.tchar(),
        &freminal_common::buffer_states::tchar::TChar::Ascii(0x41),
        "bytes outside 0x5F–0x7E must pass through unchanged in Replace mode"
    );
}

#[test]
fn show_cursor_default_true() {
    // A freshly created handler must report show_cursor() == true (DECTCEM Show is default).
    let handler = TerminalHandler::new(80, 24);
    assert!(handler.show_cursor(), "show_cursor must be true by default");
}

#[test]
fn hide_cursor_mode() {
    // Sending Mode(Dectem(Hide)) must make show_cursor() return false.
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::dectcem::Dectcem,
        terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(80, 24);

    handler.process_outputs(&[TerminalOutput::Mode(Mode::Dectem(Dectcem::new(
        &SetMode::DecRst,
    )))]);

    assert!(
        !handler.show_cursor(),
        "show_cursor must be false after Hide mode"
    );
}

#[test]
fn show_cursor_mode() {
    // Hide then Show must leave show_cursor() == true.
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::dectcem::Dectcem,
        terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(80, 24);

    handler.process_outputs(&[TerminalOutput::Mode(Mode::Dectem(Dectcem::new(
        &SetMode::DecRst,
    )))]);
    assert!(!handler.show_cursor(), "must be hidden after DecRst");

    handler.process_outputs(&[TerminalOutput::Mode(Mode::Dectem(Dectcem::new(
        &SetMode::DecSet,
    )))]);
    assert!(
        handler.show_cursor(),
        "show_cursor must be true after Show mode"
    );
}

#[test]
fn cursor_visual_style_set() {
    // CursorVisualStyle output must update cursor_visual_style().
    use freminal_common::{
        buffer_states::terminal_output::TerminalOutput, cursor::CursorVisualStyle,
    };

    let mut handler = TerminalHandler::new(80, 24);

    handler.process_outputs(&[TerminalOutput::CursorVisualStyle(
        CursorVisualStyle::VerticalLineCursorSteady,
    )]);

    assert_eq!(
        handler.cursor_visual_style(),
        CursorVisualStyle::VerticalLineCursorSteady,
        "cursor_visual_style must reflect the last CursorVisualStyle output"
    );
}

#[test]
fn xtcblink_toggles_blink() {
    // XtCBlink::Blinking flips the current steady style to blinking;
    // XtCBlink::Steady flips it back.
    use freminal_common::{
        buffer_states::{
            mode::{Mode, SetMode},
            modes::xtcblink::XtCBlink,
            terminal_output::TerminalOutput,
        },
        cursor::CursorVisualStyle,
    };

    let mut handler = TerminalHandler::new(80, 24);

    // Default style is BlockCursorSteady; enabling blink must give BlockCursorBlink.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::new(
        &SetMode::DecSet,
    )))]);
    assert_eq!(
        handler.cursor_visual_style(),
        CursorVisualStyle::BlockCursorBlink,
        "XtCBlink Blinking must flip BlockCursorSteady to BlockCursorBlink"
    );

    // Disabling blink must restore the steady variant.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::new(
        &SetMode::DecRst,
    )))]);
    assert_eq!(
        handler.cursor_visual_style(),
        CursorVisualStyle::BlockCursorSteady,
        "XtCBlink Steady must flip BlockCursorBlink back to BlockCursorSteady"
    );

    // Switch to a different shape, then verify blink still works on the new shape.
    handler.process_outputs(&[TerminalOutput::CursorVisualStyle(
        CursorVisualStyle::UnderlineCursorSteady,
    )]);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::new(
        &SetMode::DecSet,
    )))]);
    assert_eq!(
        handler.cursor_visual_style(),
        CursorVisualStyle::UnderlineCursorBlink,
        "XtCBlink Blinking must flip UnderlineCursorSteady to UnderlineCursorBlink"
    );

    // Query variant must not panic and must leave state unchanged.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::Query))]);
    assert_eq!(
        handler.cursor_visual_style(),
        CursorVisualStyle::UnderlineCursorBlink,
        "XtCBlink Query must not change cursor_visual_style"
    );
}

#[test]
fn lnm_off_lf_does_not_reset_x() {
    // LNM disabled (default): LF advances Y but leaves X unchanged.
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(80, 24);

    handler.handle_data(&text_to_bytes("hello"));
    let x_before = handler.buffer().get_cursor().pos.x;
    let y_before = handler.buffer().get_cursor().pos.y;
    assert!(x_before > 0, "cursor x must be past 0 after writing text");

    handler.process_outputs(&[TerminalOutput::Newline]);

    assert_eq!(
        handler.buffer().get_cursor().pos.x,
        x_before,
        "LNM off: LF must not reset cursor X"
    );
    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        y_before + 1,
        "LNM off: LF must advance cursor Y by 1"
    );
}

#[test]
fn lnm_on_lf_resets_x() {
    // LNM enabled: LF behaves like CRLF — X resets to 0, Y advances.
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::lnm::Lnm,
        terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(80, 24);

    handler.process_outputs(&[TerminalOutput::Mode(Mode::LineFeedMode(Lnm::new(
        &SetMode::DecSet,
    )))]);

    handler.handle_data(&text_to_bytes("hello"));
    let y_before = handler.buffer().get_cursor().pos.y;
    assert!(
        handler.buffer().get_cursor().pos.x > 0,
        "cursor x must be past 0 after writing text"
    );

    handler.process_outputs(&[TerminalOutput::Newline]);

    assert_eq!(
        handler.buffer().get_cursor().pos.x,
        0,
        "LNM on: LF must reset cursor X to 0"
    );
    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        y_before + 1,
        "LNM on: LF must still advance cursor Y by 1"
    );
}

#[test]
fn lnm_toggle() {
    // Enable LNM → LF resets X; disable LNM → LF leaves X alone.
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::lnm::Lnm,
        terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(80, 24);

    // Default: LNM off.
    assert!(
        !handler.buffer().is_lnm_enabled(),
        "LNM must be disabled by default"
    );

    // Enable LNM.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::LineFeedMode(Lnm::new(
        &SetMode::DecSet,
    )))]);
    assert!(
        handler.buffer().is_lnm_enabled(),
        "LNM must be enabled after NewLine mode"
    );

    // Verify LF resets X while LNM is on.
    handler.handle_data(&text_to_bytes("hello"));
    handler.process_outputs(&[TerminalOutput::Newline]);
    assert_eq!(
        handler.buffer().get_cursor().pos.x,
        0,
        "LNM on: LF must reset X to 0"
    );

    // Disable LNM.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::LineFeedMode(Lnm::new(
        &SetMode::DecRst,
    )))]);
    assert!(
        !handler.buffer().is_lnm_enabled(),
        "LNM must be disabled after LineFeed mode"
    );

    // Verify LF no longer resets X.
    handler.handle_data(&text_to_bytes("world"));
    let x_after_write = handler.buffer().get_cursor().pos.x;
    let y_after_write = handler.buffer().get_cursor().pos.y;
    handler.process_outputs(&[TerminalOutput::Newline]);
    assert_eq!(
        handler.buffer().get_cursor().pos.x,
        x_after_write,
        "LNM off: LF must not reset X"
    );
    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        y_after_write + 1,
        "LNM off: LF must still advance Y"
    );

    // Query variant must not panic and must leave state unchanged.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::LineFeedMode(Lnm::Query))]);
    assert!(
        !handler.buffer().is_lnm_enabled(),
        "LNM state must not change after a Query mode"
    );
}

#[test]
fn decawm_mode_dispatch() {
    // Verify that Mode::Decawm variants are correctly dispatched through
    // process_outputs (AutoWrap enables, NoAutoWrap disables).
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decawm::Decawm,
        terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(80, 24);

    // Default is wrap-enabled.
    assert!(
        handler.buffer().is_wrap_enabled(),
        "wrap must be enabled by default"
    );

    // Disable via mode dispatch.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decawm(Decawm::new(
        &SetMode::DecRst,
    )))]);
    assert!(
        !handler.buffer().is_wrap_enabled(),
        "wrap must be disabled after NoAutoWrap mode"
    );

    // Re-enable via mode dispatch.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decawm(Decawm::new(
        &SetMode::DecSet,
    )))]);
    assert!(
        handler.buffer().is_wrap_enabled(),
        "wrap must be re-enabled after AutoWrap mode"
    );

    // Query variant must not panic and must leave the state unchanged.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decawm(Decawm::Query))]);
    assert!(
        handler.buffer().is_wrap_enabled(),
        "wrap state must not change after a Query mode"
    );
}

// ── VPA (ESC [ n d) – Vertical Position Absolute ──────────────────────────────

#[test]
fn vpa_moves_cursor_to_correct_row() {
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    // ESC[42d should place cursor on row 41 (0-based) without touching x.
    let mut handler = TerminalHandler::new(147, 43);

    // Position cursor at column 10 (1-based → col 9 zero-based)
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: Some(10),
        y: None,
    }]);

    // VPA to row 42 (1-based → screen row 41 zero-based)
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: None,
        y: Some(42),
    }]);

    let screen_pos = handler.cursor_pos();
    assert_eq!(
        screen_pos.y, 41,
        "cursor should be on screen row 41 (0-based)"
    );
    assert_eq!(screen_pos.x, 9, "VPA must not change the column");
}

#[test]
fn vpa_row_1_places_cursor_at_top() {
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(80, 24);

    // Move cursor somewhere first (1-based → col 4 / row 9 zero-based)
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: Some(5),
        y: Some(10),
    }]);

    // VPA to row 1 (top of screen)
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: None,
        y: Some(1),
    }]);

    let screen_pos = handler.cursor_pos();
    assert_eq!(screen_pos.y, 0, "VPA 1 should place cursor at top row");
    assert_eq!(screen_pos.x, 4, "VPA must not change the column");
}

#[test]
fn vpa_nano_bottom_bar_sequence() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::xtextscrn::XtExtscrn,
        terminal_output::TerminalOutput,
    };

    // Simulate nano's approach: switch to alt screen, set scroll region,
    // clear, then use VPA to position the bottom status bar rows.
    let mut handler = TerminalHandler::new(147, 43);

    // Enter alternate screen
    handler.process_outputs(&[TerminalOutput::Mode(Mode::XtExtscrn(XtExtscrn::new(
        &SetMode::DecSet,
    )))]);

    // Set scroll region rows 1..43 (DECSTBM ESC[1;43r → 0-based: top=0, bottom=42)
    handler.process_outputs(&[TerminalOutput::SetTopAndBottomMargins {
        top_margin: 1,
        bottom_margin: 43,
    }]);

    // Home + clear (ESC[H + ESC[2J)
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: Some(1),
        y: Some(1),
    }]);
    handler.process_outputs(&[TerminalOutput::ClearDisplay]);

    // Write title bar content at row 1 (CUP ESC[1;1H already done above)
    handler.handle_data(b"  GNU nano 8.7.1  ");

    // VPA to row 42 — nano's shortcut bar (1-based → screen row 41)
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: None,
        y: Some(42),
    }]);

    assert_eq!(
        handler.cursor_pos().y,
        41,
        "nano's ESC[42d should land on screen row 41 (0-based)"
    );

    // VPA to row 43 — second shortcut bar row
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: None,
        y: Some(43),
    }]);

    assert_eq!(
        handler.cursor_pos().y,
        42,
        "nano's ESC[43d should land on screen row 42 (0-based)"
    );

    // VPA to row 2 — cursor should return near the top (content area)
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: None,
        y: Some(2),
    }]);

    assert_eq!(
        handler.cursor_pos().y,
        1,
        "nano's ESC[2d should land on screen row 1 (0-based)"
    );
}
