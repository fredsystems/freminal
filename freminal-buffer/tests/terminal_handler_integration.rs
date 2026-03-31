// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_buffer::terminal_handler::TerminalHandler;
use freminal_common::buffer_states::mode::Mode;
use freminal_common::buffer_states::modes::application_escape_key::ApplicationEscapeKey;
use freminal_common::buffer_states::modes::modify_other_keys_mode::ModifyOtherKeysMode;
use freminal_common::buffer_states::terminal_output::TerminalOutput;
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

    // Set scroll region to lines 5-20 (1-based from parser)
    handler.handle_set_scroll_region(5, 20);

    // Verify scroll region was set correctly (0-based: 4, 19)
    let (top, bottom) = handler.buffer().scroll_region();
    assert_eq!(top, 4, "scroll region top should be 0-based 4");
    assert_eq!(bottom, 19, "scroll region bottom should be 0-based 19");

    // Fill some content — should scroll within the region
    for i in 0..25 {
        handler.handle_data(&text_to_bytes(&format!("Line {}", i)));
        handler.handle_newline();
    }

    // Scroll region should still be the same after writing content
    let (top, bottom) = handler.buffer().scroll_region();
    assert_eq!(top, 4);
    assert_eq!(bottom, 19);
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
    handler.handle_resize(120, 30, 8, 16);

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
fn test_process_outputs_delete_lines() {
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(10, 5);
    handler.handle_enter_alternate();

    // Fill 5 visible rows
    let outputs = vec![
        TerminalOutput::Data(b"AAAAAAAAAA".to_vec()),
        TerminalOutput::Newline,
        TerminalOutput::CarriageReturn,
        TerminalOutput::Data(b"BBBBBBBBBB".to_vec()),
        TerminalOutput::Newline,
        TerminalOutput::CarriageReturn,
        TerminalOutput::Data(b"CCCCCCCCCC".to_vec()),
        TerminalOutput::Newline,
        TerminalOutput::CarriageReturn,
        TerminalOutput::Data(b"DDDDDDDDDD".to_vec()),
        TerminalOutput::Newline,
        TerminalOutput::CarriageReturn,
        TerminalOutput::Data(b"EEEEEEEEEE".to_vec()),
        // Move cursor to row 2 (1-based)
        TerminalOutput::SetCursorPos {
            x: Some(1),
            y: Some(2),
        },
        // Delete 1 line at cursor → row B is removed, C/D/E shift up,
        // bottom row becomes blank
        TerminalOutput::DeleteLines(1),
    ];
    handler.process_outputs(&outputs);

    let visible = handler.buffer().visible_rows(0);
    // Row 0 should still be "A..."
    let row0_text: String = visible[0]
        .get_characters()
        .iter()
        .map(|c| c.into_utf8())
        .collect();
    assert!(row0_text.starts_with("AAAAAAAAAA"), "row 0: {row0_text}");

    // Row 1 should now be "C..." (was row 2 before delete)
    let row1_text: String = visible[1]
        .get_characters()
        .iter()
        .map(|c| c.into_utf8())
        .collect();
    assert!(row1_text.starts_with("CCCCCCCCCC"), "row 1: {row1_text}");
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
fn report_character_size_in_pixels_is_synchronous() {
    // ReportCharacterSizeInPixels must be handled synchronously via write_to_pty,
    // not deferred to window_commands.  This ensures the response arrives in the
    // same batch as DA1 so that applications using DA1 as a fence (e.g. yazi)
    // receive it before the fence response.
    use freminal_common::buffer_states::{
        terminal_output::TerminalOutput, window_manipulation::WindowManipulation,
    };

    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let mut handler = TerminalHandler::new(80, 24);
    handler.set_write_tx(tx);

    // Set non-default cell pixel dimensions to verify they appear in the response.
    handler.handle_resize(80, 24, 9, 17);

    handler.process_outputs(&[TerminalOutput::WindowManipulation(
        WindowManipulation::ReportCharacterSizeInPixels,
    )]);

    // Must NOT be queued as a window command.
    let commands = handler.take_window_commands();
    assert!(
        commands.is_empty(),
        "ReportCharacterSizeInPixels must not be deferred to window_commands"
    );

    // Must appear immediately on the PTY write channel.
    let msg = rx
        .try_recv()
        .expect("ReportCharacterSizeInPixels must produce a PTY response");
    let bytes = match msg {
        PtyWrite::Write(b) => b,
        other => panic!("expected PtyWrite::Write, got {other:?}"),
    };
    let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
    assert_eq!(
        response, "\x1b[6;17;9t",
        "CSI 16t response must be ESC[6;height;widtht"
    );
}

#[test]
fn report_terminal_size_in_characters_is_synchronous() {
    use freminal_common::buffer_states::{
        terminal_output::TerminalOutput, window_manipulation::WindowManipulation,
    };

    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let mut handler = TerminalHandler::new(80, 24);
    handler.set_write_tx(tx);

    handler.process_outputs(&[TerminalOutput::WindowManipulation(
        WindowManipulation::ReportTerminalSizeInCharacters,
    )]);

    let commands = handler.take_window_commands();
    assert!(
        commands.is_empty(),
        "ReportTerminalSizeInCharacters must not be deferred to window_commands"
    );

    let msg = rx
        .try_recv()
        .expect("ReportTerminalSizeInCharacters must produce a PTY response");
    let bytes = match msg {
        PtyWrite::Write(b) => b,
        other => panic!("expected PtyWrite::Write, got {other:?}"),
    };
    let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
    assert_eq!(
        response, "\x1b[8;24;80t",
        "CSI 18t response must be ESC[8;height;widtht"
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

// =============================================================================
// Subtask 7.6: CNL (Cursor Next Line) and CPL (Cursor Previous Line)
// =============================================================================

#[test]
fn test_cnl_moves_cursor_down_and_to_column_one() {
    let mut handler = TerminalHandler::new(80, 24);

    // Position cursor at column 10, row 0
    handler.handle_data(&text_to_bytes("0123456789"));
    assert_eq!(handler.cursor_pos().x, 10);
    assert_eq!(handler.cursor_pos().y, 0);

    // CNL with default param (1) — move down 1 line, cursor to column 0
    handler.process_outputs(&[
        TerminalOutput::SetCursorPosRel {
            x: None,
            y: Some(1),
        },
        TerminalOutput::SetCursorPos {
            x: Some(1),
            y: None,
        },
    ]);

    assert_eq!(
        handler.cursor_pos().x,
        0,
        "CNL should move cursor to column 0"
    );
    assert_eq!(
        handler.cursor_pos().y,
        1,
        "CNL should move cursor down 1 line"
    );
}

#[test]
fn test_cnl_with_explicit_count() {
    let mut handler = TerminalHandler::new(80, 24);

    // Position cursor at column 5, row 2
    handler.handle_data(&text_to_bytes("Hello"));
    handler.process_outputs(&[TerminalOutput::SetCursorPosRel {
        x: None,
        y: Some(2),
    }]);
    assert_eq!(handler.cursor_pos().x, 5);
    assert_eq!(handler.cursor_pos().y, 2);

    // CNL 3 — move down 3 lines, cursor to column 0
    handler.process_outputs(&[
        TerminalOutput::SetCursorPosRel {
            x: None,
            y: Some(3),
        },
        TerminalOutput::SetCursorPos {
            x: Some(1),
            y: None,
        },
    ]);

    assert_eq!(
        handler.cursor_pos().x,
        0,
        "CNL 3 should move cursor to column 0"
    );
    assert_eq!(
        handler.cursor_pos().y,
        5,
        "CNL 3 should move cursor down 3 lines from row 2 to row 5"
    );
}

#[test]
fn test_cpl_moves_cursor_up_and_to_column_one() {
    let mut handler = TerminalHandler::new(80, 24);

    // Position cursor at column 10, row 5
    handler.handle_data(&text_to_bytes("0123456789"));
    handler.process_outputs(&[TerminalOutput::SetCursorPosRel {
        x: None,
        y: Some(5),
    }]);
    assert_eq!(handler.cursor_pos().x, 10);
    assert_eq!(handler.cursor_pos().y, 5);

    // CPL with default param (1) — move up 1 line, cursor to column 0
    handler.process_outputs(&[
        TerminalOutput::SetCursorPosRel {
            x: None,
            y: Some(-1),
        },
        TerminalOutput::SetCursorPos {
            x: Some(1),
            y: None,
        },
    ]);

    assert_eq!(
        handler.cursor_pos().x,
        0,
        "CPL should move cursor to column 0"
    );
    assert_eq!(
        handler.cursor_pos().y,
        4,
        "CPL should move cursor up 1 line"
    );
}

#[test]
fn test_cpl_with_explicit_count() {
    let mut handler = TerminalHandler::new(80, 24);

    // Position cursor at column 15, row 10
    handler.handle_data(&text_to_bytes("0123456789ABCDE"));
    handler.process_outputs(&[TerminalOutput::SetCursorPosRel {
        x: None,
        y: Some(10),
    }]);
    assert_eq!(handler.cursor_pos().x, 15);
    assert_eq!(handler.cursor_pos().y, 10);

    // CPL 4 — move up 4 lines, cursor to column 0
    handler.process_outputs(&[
        TerminalOutput::SetCursorPosRel {
            x: None,
            y: Some(-4),
        },
        TerminalOutput::SetCursorPos {
            x: Some(1),
            y: None,
        },
    ]);

    assert_eq!(
        handler.cursor_pos().x,
        0,
        "CPL 4 should move cursor to column 0"
    );
    assert_eq!(
        handler.cursor_pos().y,
        6,
        "CPL 4 should move cursor up 4 lines from row 10 to row 6"
    );
}

// ── SU / SD (Scroll Up / Scroll Down) integration tests ──────────────

/// Helper: extract the text content from a row, trimming trailing spaces.
fn row_text(row: &freminal_buffer::row::Row) -> String {
    let s: String = row.get_characters().iter().map(|c| c.into_utf8()).collect();
    s.trim_end().to_string()
}

/// Fill lines 0..n with "Line 0", "Line 1", etc, moving with newline+CR.
fn fill_lines(handler: &mut TerminalHandler, n: usize) {
    for i in 0..n {
        handler.handle_data(&text_to_bytes(&format!("Line {i}")));
        if i + 1 < n {
            handler.handle_newline();
            handler.handle_carriage_return();
        }
    }
}

#[test]
fn test_su_default_count_no_scroll_region() {
    // SU with default count (1) on whole screen.
    let mut handler = TerminalHandler::new(40, 10);
    fill_lines(&mut handler, 10);

    // Cursor should be on the last line (row 9) after filling 10 lines.
    assert_eq!(handler.cursor_pos().y, 9);

    // ScrollUp(1): top line scrolls off, blank line appears at bottom.
    handler.handle_scroll_up(1);

    let rows = handler.buffer().visible_rows(0);
    // Row 0 should now hold what was "Line 1" (old row 1).
    assert_eq!(row_text(&rows[0]), "Line 1", "first visible row after SU 1");
    // Last visible row should be blank (newly inserted).
    assert_eq!(
        row_text(&rows[9]),
        "",
        "last visible row should be blank after SU 1"
    );
}

#[test]
fn test_su_explicit_count_no_scroll_region() {
    let mut handler = TerminalHandler::new(40, 10);
    fill_lines(&mut handler, 10);

    handler.handle_scroll_up(3);

    let rows = handler.buffer().visible_rows(0);
    // After scrolling up by 3, old rows 3..9 become visible rows 0..6.
    assert_eq!(row_text(&rows[0]), "Line 3", "first visible row after SU 3");
    assert_eq!(
        row_text(&rows[6]),
        "Line 9",
        "row 6 should hold old Line 9 after SU 3"
    );
    // Rows 7, 8, 9 should be blank.
    for (i, row) in rows.iter().enumerate().take(10).skip(7) {
        assert_eq!(row_text(row), "", "row {i} should be blank after SU 3");
    }
}

#[test]
fn test_sd_default_count_no_scroll_region() {
    // SD with default count (1) on whole screen.
    let mut handler = TerminalHandler::new(40, 10);
    fill_lines(&mut handler, 10);

    handler.handle_scroll_down(1);

    let rows = handler.buffer().visible_rows(0);
    // Row 0 should now be blank (newly inserted at top).
    assert_eq!(
        row_text(&rows[0]),
        "",
        "first visible row should be blank after SD 1"
    );
    // Row 1 should hold what was Line 0.
    assert_eq!(
        row_text(&rows[1]),
        "Line 0",
        "row 1 should hold old Line 0 after SD 1"
    );
    // The old last line ("Line 9") should have scrolled off the bottom.
    assert_eq!(
        row_text(&rows[9]),
        "Line 8",
        "last visible row should hold old Line 8 after SD 1"
    );
}

#[test]
fn test_sd_explicit_count_no_scroll_region() {
    let mut handler = TerminalHandler::new(40, 10);
    fill_lines(&mut handler, 10);

    handler.handle_scroll_down(3);

    let rows = handler.buffer().visible_rows(0);
    // First 3 rows should be blank.
    for (i, row) in rows.iter().enumerate().take(3) {
        assert_eq!(row_text(row), "", "row {i} should be blank after SD 3");
    }
    // Row 3 should hold "Line 0".
    assert_eq!(
        row_text(&rows[3]),
        "Line 0",
        "row 3 should hold Line 0 after SD 3"
    );
    // Row 9 should hold "Line 6" (old lines 7, 8, 9 scrolled off bottom).
    assert_eq!(
        row_text(&rows[9]),
        "Line 6",
        "row 9 should hold Line 6 after SD 3"
    );
}

#[test]
fn test_su_with_scroll_region() {
    // Set scroll region to rows 3..7 (1-based: 4..8), fill the screen,
    // then SU 2 should only affect that region.
    let mut handler = TerminalHandler::new(40, 10);
    fill_lines(&mut handler, 10);

    // Set scroll region (1-based top=4, bottom=8 → 0-based 3..7)
    handler.handle_set_scroll_region(4, 8);

    handler.handle_scroll_up(2);

    let rows = handler.buffer().visible_rows(0);
    // Rows above the region (0..2) should be untouched.
    assert_eq!(row_text(&rows[0]), "Line 0", "row above region untouched");
    assert_eq!(row_text(&rows[1]), "Line 1", "row above region untouched");
    assert_eq!(row_text(&rows[2]), "Line 2", "row above region untouched");

    // Inside the region: old rows 5,6,7 shift up to rows 3,4,5; rows 6,7 become blank.
    assert_eq!(
        row_text(&rows[3]),
        "Line 5",
        "region row 0 after SU 2 within region"
    );
    assert_eq!(
        row_text(&rows[4]),
        "Line 6",
        "region row 1 after SU 2 within region"
    );
    assert_eq!(
        row_text(&rows[5]),
        "Line 7",
        "region row 2 after SU 2 within region"
    );
    // Bottom 2 rows of the region become blank.
    assert_eq!(
        row_text(&rows[6]),
        "",
        "region row 3 (blank) after SU 2 within region"
    );
    assert_eq!(
        row_text(&rows[7]),
        "",
        "region row 4 (blank) after SU 2 within region"
    );

    // Rows below the region should be untouched.
    assert_eq!(row_text(&rows[8]), "Line 8", "row below region untouched");
    assert_eq!(row_text(&rows[9]), "Line 9", "row below region untouched");
}

#[test]
fn test_sd_with_scroll_region() {
    let mut handler = TerminalHandler::new(40, 10);
    fill_lines(&mut handler, 10);

    // Set scroll region (1-based top=4, bottom=8 → 0-based 3..7)
    handler.handle_set_scroll_region(4, 8);

    handler.handle_scroll_down(2);

    let rows = handler.buffer().visible_rows(0);
    // Rows above the region untouched.
    assert_eq!(row_text(&rows[0]), "Line 0");
    assert_eq!(row_text(&rows[1]), "Line 1");
    assert_eq!(row_text(&rows[2]), "Line 2");

    // Top 2 rows of the region become blank.
    assert_eq!(
        row_text(&rows[3]),
        "",
        "region top (blank) after SD 2 within region"
    );
    assert_eq!(
        row_text(&rows[4]),
        "",
        "region top+1 (blank) after SD 2 within region"
    );
    // Old rows 3,4,5 shift down to rows 5,6,7.
    assert_eq!(
        row_text(&rows[5]),
        "Line 3",
        "region row shifted down after SD 2"
    );
    assert_eq!(
        row_text(&rows[6]),
        "Line 4",
        "region row shifted down after SD 2"
    );
    assert_eq!(
        row_text(&rows[7]),
        "Line 5",
        "region row shifted down after SD 2"
    );

    // Rows below the region untouched.
    assert_eq!(row_text(&rows[8]), "Line 8");
    assert_eq!(row_text(&rows[9]), "Line 9");
}

#[test]
fn test_su_count_exceeds_region_size_clamped() {
    // SU with count larger than region size should clear the entire region.
    let mut handler = TerminalHandler::new(40, 10);
    fill_lines(&mut handler, 10);

    // Region is 5 rows (1-based 4..8 → 0-based 3..7).
    handler.handle_set_scroll_region(4, 8);

    // Scroll up by 100 — should clamp to region size (5).
    handler.handle_scroll_up(100);

    let rows = handler.buffer().visible_rows(0);
    // Rows above region untouched.
    assert_eq!(row_text(&rows[0]), "Line 0");
    assert_eq!(row_text(&rows[1]), "Line 1");
    assert_eq!(row_text(&rows[2]), "Line 2");

    // Entire region should be blank.
    for (i, row) in rows.iter().enumerate().take(8).skip(3) {
        assert_eq!(
            row_text(row),
            "",
            "row {i} in region should be blank after SU(100)"
        );
    }

    // Rows below region untouched.
    assert_eq!(row_text(&rows[8]), "Line 8");
    assert_eq!(row_text(&rows[9]), "Line 9");
}

#[test]
fn test_sd_count_exceeds_region_size_clamped() {
    let mut handler = TerminalHandler::new(40, 10);
    fill_lines(&mut handler, 10);

    handler.handle_set_scroll_region(4, 8);

    // Scroll down by 100 — should clamp to region size (5).
    handler.handle_scroll_down(100);

    let rows = handler.buffer().visible_rows(0);
    // Rows above region untouched.
    assert_eq!(row_text(&rows[0]), "Line 0");
    assert_eq!(row_text(&rows[1]), "Line 1");
    assert_eq!(row_text(&rows[2]), "Line 2");

    // Entire region should be blank.
    for (i, row) in rows.iter().enumerate().take(8).skip(3) {
        assert_eq!(
            row_text(row),
            "",
            "row {i} in region should be blank after SD(100)"
        );
    }

    // Rows below region untouched.
    assert_eq!(row_text(&rows[8]), "Line 8");
    assert_eq!(row_text(&rows[9]), "Line 9");
}

#[test]
fn test_su_via_process_outputs() {
    // Verify that ScrollUp works through the process_outputs dispatch path.
    let mut handler = TerminalHandler::new(40, 10);
    fill_lines(&mut handler, 10);

    handler.process_outputs(&[TerminalOutput::ScrollUp(2)]);

    let rows = handler.buffer().visible_rows(0);
    assert_eq!(
        row_text(&rows[0]),
        "Line 2",
        "first row after SU(2) via process_outputs"
    );
    // Last 2 rows blank.
    assert_eq!(row_text(&rows[8]), "", "row 8 blank after SU(2)");
    assert_eq!(row_text(&rows[9]), "", "row 9 blank after SU(2)");
}

#[test]
fn test_sd_via_process_outputs() {
    let mut handler = TerminalHandler::new(40, 10);
    fill_lines(&mut handler, 10);

    handler.process_outputs(&[TerminalOutput::ScrollDown(2)]);

    let rows = handler.buffer().visible_rows(0);
    // First 2 rows blank.
    assert_eq!(row_text(&rows[0]), "", "row 0 blank after SD(2)");
    assert_eq!(row_text(&rows[1]), "", "row 1 blank after SD(2)");
    assert_eq!(
        row_text(&rows[2]),
        "Line 0",
        "row 2 = old Line 0 after SD(2)"
    );
}

#[test]
fn test_su_cursor_position_unchanged() {
    // SU should NOT move the cursor.
    let mut handler = TerminalHandler::new(40, 10);
    fill_lines(&mut handler, 10);

    // Move cursor to a specific position.
    handler.handle_cursor_pos(Some(5), Some(5));
    let before_x = handler.cursor_pos().x;
    let before_y = handler.cursor_pos().y;

    handler.handle_scroll_up(3);

    assert_eq!(
        handler.cursor_pos().x,
        before_x,
        "SU should not change cursor X"
    );
    assert_eq!(
        handler.cursor_pos().y,
        before_y,
        "SU should not change cursor Y"
    );
}

#[test]
fn test_sd_cursor_position_unchanged() {
    // SD should NOT move the cursor.
    let mut handler = TerminalHandler::new(40, 10);
    fill_lines(&mut handler, 10);

    handler.handle_cursor_pos(Some(5), Some(5));
    let before_x = handler.cursor_pos().x;
    let before_y = handler.cursor_pos().y;

    handler.handle_scroll_down(3);

    assert_eq!(
        handler.cursor_pos().x,
        before_x,
        "SD should not change cursor X"
    );
    assert_eq!(
        handler.cursor_pos().y,
        before_y,
        "SD should not change cursor Y"
    );
}

// ── DSR (Device Status Report) integration tests ─────────────────────

#[test]
fn test_dsr_ps5_device_status_report() {
    // CSI 5 n should respond with CSI 0 n (device OK).
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();

    let mut handler = TerminalHandler::new(80, 24);
    handler.set_write_tx(tx);

    handler.process_outputs(&[TerminalOutput::DeviceStatusReport]);

    let msg = rx
        .try_recv()
        .expect("DeviceStatusReport must send a message to the channel");
    let bytes = match msg {
        PtyWrite::Write(b) => b,
        other => panic!("expected PtyWrite::Write, got {other:?}"),
    };
    let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
    assert_eq!(
        response, "\x1b[0n",
        "DSR Ps=5 must respond with ESC[0n (device OK)"
    );
}

#[test]
fn test_dsr_ps6_cursor_position_report() {
    // CSI 6 n should respond with cursor position report CSI row ; col R.
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();

    let mut handler = TerminalHandler::new(80, 24);
    handler.set_write_tx(tx);

    // Move cursor to col 9, row 4 (0-indexed) via 1-indexed SetCursorPos.
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: Some(10), // 1-indexed → col 9
        y: Some(5),  // 1-indexed → row 4
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
        response, "\x1b[5;10R",
        "DSR Ps=6 must respond with ESC[row;colR (1-indexed)"
    );
}

#[test]
fn test_dsr_ps6_cursor_at_origin() {
    // CSI 6 n at origin should respond with ESC [ 1 ; 1 R.
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();

    let mut handler = TerminalHandler::new(80, 24);
    handler.set_write_tx(tx);

    handler.process_outputs(&[TerminalOutput::CursorReport]);

    let msg = rx
        .try_recv()
        .expect("CursorReport must send a message to the channel");
    let bytes = match msg {
        PtyWrite::Write(b) => b,
        other => panic!("expected PtyWrite::Write, got {other:?}"),
    };
    let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
    assert_eq!(response, "\x1b[1;1R", "Cursor at origin should report 1;1");
}

#[test]
fn test_dsr_996_color_theme_report() {
    // DSR ?996 should respond with CSI ? 997 ; 2 n (dark mode).
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();

    let mut handler = TerminalHandler::new(80, 24);
    handler.set_write_tx(tx);

    handler.process_outputs(&[TerminalOutput::ColorThemeReport]);

    let msg = rx
        .try_recv()
        .expect("ColorThemeReport must send a message to the channel");
    let bytes = match msg {
        PtyWrite::Write(b) => b,
        other => panic!("expected PtyWrite::Write, got {other:?}"),
    };
    let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
    assert_eq!(
        response, "\x1b[?997;2n",
        "DSR ?996 must respond with ESC[?997;2n (dark mode)"
    );
}

// ── RIS (Reset to Initial State, ESC c) integration tests ────────────

#[test]
fn test_ris_clears_screen_and_resets_cursor() {
    let mut handler = TerminalHandler::new(40, 10);

    // Fill the screen with content and move cursor.
    fill_lines(&mut handler, 10);
    assert!(handler.cursor_pos().x > 0 || handler.cursor_pos().y > 0);

    // Full reset.
    handler.process_outputs(&[TerminalOutput::ResetDevice]);

    // Cursor should be at home (0,0).
    assert_eq!(handler.cursor_pos().x, 0, "RIS must reset cursor X to 0");
    assert_eq!(handler.cursor_pos().y, 0, "RIS must reset cursor Y to 0");

    // Visible area should have only 1 row (the initial blank row).
    let rows = handler.buffer().visible_rows(0);
    assert_eq!(rows.len(), 1, "RIS must clear all rows back to 1");
    assert_eq!(row_text(&rows[0]), "", "RIS row should be blank");
}

#[test]
fn test_ris_resets_scroll_region() {
    let mut handler = TerminalHandler::new(40, 10);

    // Set a non-default scroll region.
    handler.handle_set_scroll_region(3, 8);
    let (top, bottom) = handler.buffer().scroll_region();
    assert_eq!(top, 2);
    assert_eq!(bottom, 7);

    // Full reset.
    handler.process_outputs(&[TerminalOutput::ResetDevice]);

    // Scroll region should be full screen (0, height-1).
    let (top, bottom) = handler.buffer().scroll_region();
    assert_eq!(top, 0, "RIS must reset scroll region top to 0");
    assert_eq!(bottom, 9, "RIS must reset scroll region bottom to height-1");
}

#[test]
fn test_ris_resets_character_attributes() {
    use freminal_common::buffer_states::format_tag::FormatTag;

    let mut handler = TerminalHandler::new(40, 10);

    // Apply bold SGR.
    handler.process_outputs(&[TerminalOutput::Sgr(
        freminal_common::sgr::SelectGraphicRendition::Bold,
    )]);

    // Verify format changed from default.
    assert_ne!(
        *handler.current_format(),
        FormatTag::default(),
        "format should not be default after SGR bold"
    );

    // Full reset.
    handler.process_outputs(&[TerminalOutput::ResetDevice]);

    assert_eq!(
        *handler.current_format(),
        FormatTag::default(),
        "RIS must reset character attributes to default"
    );
}

#[test]
fn test_ris_via_process_outputs_does_not_panic() {
    // Smoke test: ensure RIS through process_outputs does not panic.
    let mut handler = TerminalHandler::new(80, 24);

    // Set up various state.
    fill_lines(&mut handler, 24);
    handler.handle_set_scroll_region(5, 20);
    handler.handle_enter_alternate();
    handler.handle_data(&text_to_bytes("Alternate content"));

    // Full reset — should return to primary, clean state.
    handler.process_outputs(&[TerminalOutput::ResetDevice]);

    assert_eq!(handler.cursor_pos().x, 0);
    assert_eq!(handler.cursor_pos().y, 0);
}

#[test]
fn test_ris_resets_deccolm_back_to_80() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::deccolm::Deccolm,
    };

    let mut handler = TerminalHandler::new(80, 24);

    // Switch to 132 columns.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::new(
        &SetMode::DecSet,
    )))]);
    assert_eq!(handler.buffer().terminal_width(), 132);

    // Full reset.
    handler.process_outputs(&[TerminalOutput::ResetDevice]);

    // Width should be back to 80.
    assert_eq!(
        handler.buffer().terminal_width(),
        80,
        "RIS must reset DECCOLM back to 80 columns"
    );
    // Cursor at home.
    assert_eq!(handler.cursor_pos().x, 0);
    assert_eq!(handler.cursor_pos().y, 0);
}

// ── 7.21 — HTS, TBC, CHT, CBT (Tab Stop Control) ────────────────────

#[test]
fn test_hts_sets_tab_stop_at_cursor() {
    let mut handler = TerminalHandler::new(80, 24);

    // Move to column 5 and set a tab stop there
    handler.handle_cursor_pos(Some(6), Some(1)); // 1-indexed → col 5
    handler.process_outputs(&[TerminalOutput::HorizontalTabSet]);

    // Move to column 0 and tab forward — should land on col 5
    // (assuming default tab at 8 was cleared or col 5 comes first)
    // Actually default tabs are at 8, 16, 24, ...
    // Tab from col 0 should hit col 5 (custom stop) before col 8
    handler.handle_cursor_pos(Some(1), Some(1)); // col 0
    handler.handle_tab();
    assert_eq!(handler.buffer().get_cursor().pos.x, 5);
}

#[test]
fn test_tbc_clears_tab_stop_at_cursor() {
    let mut handler = TerminalHandler::new(80, 24);

    // Default tab stops are at every 8th column: 0, 8, 16, 24, ...
    // Move to column 8 and clear the tab stop there
    handler.handle_cursor_pos(Some(9), Some(1)); // col 8
    handler.process_outputs(&[TerminalOutput::TabClear(0)]);

    // Tab from col 0 should now skip col 8 and land on col 16
    handler.handle_cursor_pos(Some(1), Some(1)); // col 0
    handler.handle_tab();
    assert_eq!(handler.buffer().get_cursor().pos.x, 16);
}

#[test]
fn test_tbc_clear_all_tab_stops() {
    let mut handler = TerminalHandler::new(80, 24);

    // Clear ALL tab stops
    handler.process_outputs(&[TerminalOutput::TabClear(3)]);

    // Tab from col 0 should go to the last column (no stops to land on)
    handler.handle_cursor_pos(Some(1), Some(1)); // col 0
    handler.handle_tab();
    assert_eq!(handler.buffer().get_cursor().pos.x, 79);
}

#[test]
fn test_cht_cursor_forward_tabulation() {
    let mut handler = TerminalHandler::new(80, 24);

    // Default tabs at 8, 16, 24, ...
    handler.handle_cursor_pos(Some(1), Some(1)); // col 0

    // CHT 2 = advance 2 tab stops → col 16
    handler.process_outputs(&[TerminalOutput::CursorForwardTab(2)]);
    assert_eq!(handler.buffer().get_cursor().pos.x, 16);
}

#[test]
fn test_cht_default_one() {
    let mut handler = TerminalHandler::new(80, 24);

    handler.handle_cursor_pos(Some(1), Some(1)); // col 0

    // CHT 1 = advance 1 tab stop → col 8
    handler.process_outputs(&[TerminalOutput::CursorForwardTab(1)]);
    assert_eq!(handler.buffer().get_cursor().pos.x, 8);
}

#[test]
fn test_cbt_cursor_backward_tabulation() {
    let mut handler = TerminalHandler::new(80, 24);

    // Move to column 20
    handler.handle_cursor_pos(Some(21), Some(1)); // col 20

    // CBT 1 = back 1 tab stop → col 16
    handler.process_outputs(&[TerminalOutput::CursorBackwardTab(1)]);
    assert_eq!(handler.buffer().get_cursor().pos.x, 16);
}

#[test]
fn test_cbt_multiple_stops() {
    let mut handler = TerminalHandler::new(80, 24);

    // Move to column 25
    handler.handle_cursor_pos(Some(26), Some(1)); // col 25

    // CBT 2 = back 2 tab stops
    // Default tabs at 0, 8, 16, 24, 32, ...
    // From col 25: 1st back → 24, 2nd back → 16
    handler.process_outputs(&[TerminalOutput::CursorBackwardTab(2)]);
    assert_eq!(handler.buffer().get_cursor().pos.x, 16);
}

#[test]
fn test_cbt_at_start_stays_at_zero() {
    let mut handler = TerminalHandler::new(80, 24);

    handler.handle_cursor_pos(Some(1), Some(1)); // col 0

    // CBT when already at col 0 should stay at 0
    handler.process_outputs(&[TerminalOutput::CursorBackwardTab(1)]);
    assert_eq!(handler.buffer().get_cursor().pos.x, 0);
}

// ── 7.24 — CSI s / CSI u (Save / Restore Cursor) ────────────────────

#[test]
fn test_save_restore_cursor_via_process_outputs() {
    let mut handler = TerminalHandler::new(80, 24);

    // Move cursor to (10, 5)
    handler.handle_cursor_pos(Some(11), Some(6)); // col 10, row 5
    assert_eq!(handler.buffer().get_cursor().pos.x, 10);
    assert_eq!(handler.buffer().get_cursor().pos.y, 5);

    // Save cursor
    handler.process_outputs(&[TerminalOutput::SaveCursor]);

    // Move cursor somewhere else
    handler.handle_cursor_pos(Some(1), Some(1));
    assert_eq!(handler.buffer().get_cursor().pos.x, 0);
    assert_eq!(handler.buffer().get_cursor().pos.y, 0);

    // Restore cursor
    handler.process_outputs(&[TerminalOutput::RestoreCursor]);
    assert_eq!(handler.buffer().get_cursor().pos.x, 10);
    assert_eq!(handler.buffer().get_cursor().pos.y, 5);
}

#[test]
fn test_save_restore_cursor_direct() {
    let mut handler = TerminalHandler::new(80, 24);

    handler.handle_cursor_pos(Some(16), Some(8)); // col 15, row 7
    handler.handle_save_cursor();

    handler.handle_cursor_pos(Some(1), Some(1));
    handler.handle_restore_cursor();

    assert_eq!(handler.buffer().get_cursor().pos.x, 15);
    assert_eq!(handler.buffer().get_cursor().pos.y, 7);
}

// ── 7.25 — REP (CSI b) — Repeat Preceding Graphic Character ─────────

#[test]
fn test_rep_repeats_last_graphic_char() {
    let mut handler = TerminalHandler::new(80, 24);

    // Write 'A' then repeat it 4 more times
    handler.handle_data(&text_to_bytes("A"));
    handler.process_outputs(&[TerminalOutput::RepeatCharacter(4)]);

    // Cursor should be at column 5 (1 original + 4 repeated)
    assert_eq!(handler.buffer().get_cursor().pos.x, 5);

    let rows = handler.buffer().visible_rows(0);
    let text = row_text(&rows[0]);
    assert_eq!(text, "AAAAA");
}

#[test]
fn test_rep_with_no_preceding_char_is_noop() {
    let mut handler = TerminalHandler::new(80, 24);

    // No data written yet — REP should be a no-op
    handler.process_outputs(&[TerminalOutput::RepeatCharacter(5)]);

    assert_eq!(handler.buffer().get_cursor().pos.x, 0);
}

#[test]
fn test_rep_repeats_last_char_of_string() {
    let mut handler = TerminalHandler::new(80, 24);

    handler.handle_data(&text_to_bytes("XY"));
    // Last graphic char is 'Y'
    handler.process_outputs(&[TerminalOutput::RepeatCharacter(3)]);

    assert_eq!(handler.buffer().get_cursor().pos.x, 5); // 2 + 3
    let rows = handler.buffer().visible_rows(0);
    let text = row_text(&rows[0]);
    assert_eq!(text, "XYYYY");
}

#[test]
fn test_rep_default_count_one() {
    let mut handler = TerminalHandler::new(80, 24);

    handler.handle_data(&text_to_bytes("Z"));
    handler.process_outputs(&[TerminalOutput::RepeatCharacter(1)]);

    assert_eq!(handler.buffer().get_cursor().pos.x, 2);
    let rows = handler.buffer().visible_rows(0);
    let text = row_text(&rows[0]);
    assert_eq!(text, "ZZ");
}

// ── 7.26 — HPA (CSI `) alias for CHA ────────────────────────────────

#[test]
fn test_hpa_via_process_outputs() {
    // HPA and CHA both produce SetCursorPos with only x set
    let mut handler = TerminalHandler::new(80, 24);

    handler.handle_data(&text_to_bytes("Hello"));
    assert_eq!(handler.buffer().get_cursor().pos.x, 5);

    // CHA/HPA to column 3 (1-indexed)
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: Some(3),
        y: None,
    }]);
    assert_eq!(handler.buffer().get_cursor().pos.x, 2); // 0-indexed
}

// ── 7.28 — DECALN (ESC # 8) — Screen Alignment Test ─────────────────

#[test]
fn test_screen_alignment_fills_with_e() {
    let mut handler = TerminalHandler::new(10, 5);

    // Write some content first
    fill_lines(&mut handler, 5);

    // Perform screen alignment test
    handler.process_outputs(&[TerminalOutput::ScreenAlignmentTest]);

    let rows = handler.buffer().visible_rows(0);
    assert_eq!(rows.len(), 5);

    for (i, row) in rows.iter().enumerate() {
        let text = row_text(row);
        assert_eq!(
            text, "EEEEEEEEEE",
            "row {i} should be filled with 'E' characters"
        );
    }
}

#[test]
fn test_screen_alignment_homes_cursor() {
    let mut handler = TerminalHandler::new(10, 5);

    handler.handle_cursor_pos(Some(5), Some(3)); // somewhere away from home
    handler.process_outputs(&[TerminalOutput::ScreenAlignmentTest]);

    assert_eq!(handler.buffer().get_cursor().pos.x, 0);
    assert_eq!(handler.buffer().get_cursor().pos.y, 0);
}

#[test]
fn test_screen_alignment_resets_scroll_region() {
    let mut handler = TerminalHandler::new(10, 5);

    // Set a custom scroll region
    handler.handle_set_scroll_region(2, 4);
    let (top, bottom) = handler.buffer().scroll_region();
    assert_eq!(top, 1);
    assert_eq!(bottom, 3);

    // DECALN should reset it to full screen
    handler.process_outputs(&[TerminalOutput::ScreenAlignmentTest]);

    let (top, bottom) = handler.buffer().scroll_region();
    assert_eq!(top, 0);
    assert_eq!(bottom, 4);
}

// ── 7.30 — OSC Unknown no longer emits Invalid ──────────────────────
// (This is a parser-level test, best tested by verifying the code path
//  doesn't generate TerminalOutput::Invalid. We test via process_outputs
//  that no crash occurs when handling well-known outputs.)

// ── 7.29 — Legacy alternate screen ?47/?1047 and ?1048 ──────────────

#[test]
fn alt_screen_47_enter_clears_and_leave_restores() {
    // ?47 should behave like ?1049 for buffer switching: enter alternate
    // clears the screen, leave restores primary content.
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::xtextscrn::AltScreen47,
        terminal_output::TerminalOutput,
    };

    let mut handler = TerminalHandler::new(20, 5);

    // Write to primary.
    handler.process_outputs(&[TerminalOutput::Data(b"primary text".to_vec())]);
    let primary_rows = handler.buffer().visible_rows(0);
    let primary_text = row_text(&primary_rows[0]);
    assert_eq!(primary_text, "primary text");

    // Enter alternate via ?47.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::AltScreen47(AltScreen47::new(
        &SetMode::DecSet,
    )))]);

    // Alternate screen should be blank.
    let alt_rows = handler.buffer().visible_rows(0);
    let alt_text = row_text(&alt_rows[0]);
    assert!(
        alt_text.is_empty(),
        "alternate screen via ?47 should be blank, got: {alt_text:?}"
    );

    // Write something in alternate.
    handler.process_outputs(&[TerminalOutput::Data(b"alt stuff".to_vec())]);

    // Leave alternate via ?47.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::AltScreen47(AltScreen47::new(
        &SetMode::DecRst,
    )))]);

    // Primary content must be restored.
    let restored_rows = handler.buffer().visible_rows(0);
    let restored_text = row_text(&restored_rows[0]);
    assert_eq!(
        restored_text, "primary text",
        "leaving ?47 alt screen must restore primary content"
    );
}

#[test]
fn alt_screen_1047_enter_clears_and_leave_restores() {
    // ?1047 is an alias for ?47 — same behavior.
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::xtextscrn::AltScreen47,
    };

    let mut handler = TerminalHandler::new(20, 5);
    handler.handle_data(b"hello");

    // Enter alternate — via terminal_mode_from_params, ?1047 maps to AltScreen47.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::AltScreen47(AltScreen47::new(
        &SetMode::DecSet,
    )))]);

    let alt_rows = handler.buffer().visible_rows(0);
    let alt_text = row_text(&alt_rows[0]);
    assert!(
        alt_text.is_empty(),
        "alternate screen via ?1047 should be blank"
    );

    handler.process_outputs(&[TerminalOutput::Mode(Mode::AltScreen47(AltScreen47::new(
        &SetMode::DecRst,
    )))]);

    let restored = handler.buffer().visible_rows(0);
    let text = row_text(&restored[0]);
    assert_eq!(text, "hello", "leaving ?1047 must restore primary");
}

#[test]
fn save_cursor_1048_saves_and_restores() {
    // ?1048 set = save cursor, ?1048 reset = restore cursor.
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::xtextscrn::SaveCursor1048,
    };

    let mut handler = TerminalHandler::new(20, 10);

    // Move cursor to (7, 4).
    handler.handle_cursor_pos(Some(8), Some(5)); // 1-based
    assert_eq!(handler.buffer().get_cursor().pos.x, 7);
    assert_eq!(handler.buffer().get_cursor().pos.y, 4);

    // Save via ?1048 set.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::SaveCursor1048(
        SaveCursor1048::new(&SetMode::DecSet),
    ))]);

    // Move cursor somewhere else.
    handler.handle_cursor_pos(Some(1), Some(1));
    assert_eq!(handler.buffer().get_cursor().pos.x, 0);
    assert_eq!(handler.buffer().get_cursor().pos.y, 0);

    // Restore via ?1048 reset.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::SaveCursor1048(
        SaveCursor1048::new(&SetMode::DecRst),
    ))]);

    assert_eq!(
        handler.buffer().get_cursor().pos.x,
        7,
        "?1048 restore must bring cursor back to saved x"
    );
    assert_eq!(
        handler.buffer().get_cursor().pos.y,
        4,
        "?1048 restore must bring cursor back to saved y"
    );
}

#[test]
fn mode_from_params_maps_47_and_1047_to_alt_screen_47() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::xtextscrn::AltScreen47,
    };

    let mode_47 = Mode::terminal_mode_from_params(b"?47", SetMode::DecSet);
    assert_eq!(mode_47, Mode::AltScreen47(AltScreen47::Alternate));

    let mode_1047 = Mode::terminal_mode_from_params(b"?1047", SetMode::DecSet);
    assert_eq!(mode_1047, Mode::AltScreen47(AltScreen47::Alternate));

    let rst_47 = Mode::terminal_mode_from_params(b"?47", SetMode::DecRst);
    assert_eq!(rst_47, Mode::AltScreen47(AltScreen47::Primary));

    let rst_1047 = Mode::terminal_mode_from_params(b"?1047", SetMode::DecRst);
    assert_eq!(rst_1047, Mode::AltScreen47(AltScreen47::Primary));

    let query_47 = Mode::terminal_mode_from_params(b"?47", SetMode::DecQuery);
    assert_eq!(query_47, Mode::AltScreen47(AltScreen47::Query));
}

#[test]
fn mode_from_params_maps_1048_to_save_cursor() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::xtextscrn::SaveCursor1048,
    };

    let set = Mode::terminal_mode_from_params(b"?1048", SetMode::DecSet);
    assert_eq!(set, Mode::SaveCursor1048(SaveCursor1048::Save));

    let rst = Mode::terminal_mode_from_params(b"?1048", SetMode::DecRst);
    assert_eq!(rst, Mode::SaveCursor1048(SaveCursor1048::Restore));

    let query = Mode::terminal_mode_from_params(b"?1048", SetMode::DecQuery);
    assert_eq!(query, Mode::SaveCursor1048(SaveCursor1048::Query));
}

// ═══════════════════════════════════════════════════════════════════════════
// DECRPM (Mode Query) response tests — handler-owned modes
// ═══════════════════════════════════════════════════════════════════════════

/// Helper: create a handler with a PTY write channel, send a Mode Query
/// output, and return the response string.
fn query_handler_mode(handler: &mut TerminalHandler, mode_output: TerminalOutput) -> String {
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
    handler.set_write_tx(tx);
    handler.process_outputs(&[mode_output]);
    let msg = rx
        .try_recv()
        .expect("Mode query must produce a DECRPM response on the PTY channel");
    let bytes = match msg {
        PtyWrite::Write(b) => b,
        other => panic!("expected PtyWrite::Write, got {other:?}"),
    };
    String::from_utf8(bytes).expect("DECRPM response must be valid UTF-8")
}

#[test]
fn decrpm_dectcem_default_is_show() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::dectcem::Dectcem,
    };

    let mut handler = TerminalHandler::new(80, 24);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::Dectem(Dectcem::new(&SetMode::DecQuery))),
    );
    // Default DECTCEM is Show → Ps=1 (set)
    assert_eq!(resp, "\x1b[?25;1$y", "DECTCEM default (Show) → Ps=1");
}

#[test]
fn decrpm_dectcem_after_hide() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::dectcem::Dectcem,
    };

    let mut handler = TerminalHandler::new(80, 24);
    // Hide cursor first
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Dectem(Dectcem::new(
        &SetMode::DecRst,
    )))]);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::Dectem(Dectcem::new(&SetMode::DecQuery))),
    );
    // After Hide → Ps=2 (reset)
    assert_eq!(resp, "\x1b[?25;2$y", "DECTCEM after Hide → Ps=2");
}

#[test]
fn decrpm_decawm_default_is_set() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decawm::Decawm,
    };

    let mut handler = TerminalHandler::new(80, 24);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::Decawm(Decawm::new(&SetMode::DecQuery))),
    );
    // Default DECAWM is AutoWrap (enabled) → Ps=1 (set)
    assert_eq!(resp, "\x1b[?7;1$y", "DECAWM default (AutoWrap) → Ps=1");
}

#[test]
fn decrpm_decawm_after_disable() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decawm::Decawm,
    };

    let mut handler = TerminalHandler::new(80, 24);
    // Disable auto-wrap
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decawm(Decawm::new(
        &SetMode::DecRst,
    )))]);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::Decawm(Decawm::new(&SetMode::DecQuery))),
    );
    // After disable → Ps=2 (reset)
    assert_eq!(resp, "\x1b[?7;2$y", "DECAWM after disable → Ps=2");
}

#[test]
fn decrpm_xtextscrn_default_is_primary() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::xtextscrn::XtExtscrn,
    };

    let mut handler = TerminalHandler::new(80, 24);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::XtExtscrn(XtExtscrn::new(&SetMode::DecQuery))),
    );
    // Default is primary screen → Ps=2 (reset)
    assert_eq!(resp, "\x1b[?1049;2$y", "XtExtscrn default (primary) → Ps=2");
}

#[test]
fn decrpm_xtcblink_default_is_steady() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::xtcblink::XtCBlink,
    };

    let mut handler = TerminalHandler::new(80, 24);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::new(&SetMode::DecQuery))),
    );
    // Default cursor is steady → Ps=2 (reset)
    assert_eq!(resp, "\x1b[?12;2$y", "XtCBlink default (steady) → Ps=2");
}

#[test]
fn decrpm_xtcblink_after_enable_blink() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::xtcblink::XtCBlink,
    };

    let mut handler = TerminalHandler::new(80, 24);
    // Enable cursor blink
    handler.process_outputs(&[TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::new(
        &SetMode::DecSet,
    )))]);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::XtCBlink(XtCBlink::new(&SetMode::DecQuery))),
    );
    // After enable → Ps=1 (set)
    assert_eq!(resp, "\x1b[?12;1$y", "XtCBlink after blink enable → Ps=1");
}

#[test]
fn decrpm_unknown_mode_returns_not_recognized() {
    use freminal_common::buffer_states::mode::Mode;

    let mut handler = TerminalHandler::new(80, 24);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::UnknownQuery(b"9999".to_vec())),
    );
    // Unknown mode → Ps=0 (not recognized)
    assert_eq!(
        resp, "\x1b[?9999;0$y",
        "Unknown mode query must return Ps=0"
    );
}

#[test]
fn decrpm_lnm_default_is_line_feed() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::lnm::Lnm,
    };

    let mut handler = TerminalHandler::new(80, 24);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::LineFeedMode(Lnm::new(&SetMode::DecQuery))),
    );
    // Default LNM is LineFeed (disabled) → Ps=2 (reset)
    assert_eq!(resp, "\x1b[?20;2$y", "LNM default (LineFeed mode) → Ps=2");
}

// ── DECOM (Origin Mode ?6) ────────────────────────────────────────────────────

/// Helper: fill the handler's buffer so that rows.len() >= height by writing
/// enough newlines.  This ensures `visible_window_start(0) == 0` for simple
/// position math in tests.
fn fill_buffer(handler: &mut TerminalHandler) {
    let (_, h) = handler.get_win_size();
    for _ in 0..h {
        handler.handle_newline();
    }
    // Home cursor
    handler.handle_cursor_pos(Some(1), Some(1));
}

#[test]
fn decom_mode_dispatch() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decom::Decom,
    };

    let mut handler = TerminalHandler::new(80, 24);

    // Default is DECOM off.
    assert!(
        !handler.buffer().is_decom_enabled(),
        "DECOM must be disabled by default"
    );

    // Enable DECOM.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decom(Decom::new(
        &SetMode::DecSet,
    )))]);
    assert!(
        handler.buffer().is_decom_enabled(),
        "DECOM must be enabled after DecSet"
    );

    // Disable DECOM.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decom(Decom::new(
        &SetMode::DecRst,
    )))]);
    assert!(
        !handler.buffer().is_decom_enabled(),
        "DECOM must be disabled after DecRst"
    );

    // Query variant must not panic and must leave state unchanged.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decom(Decom::Query))]);
    assert!(
        !handler.buffer().is_decom_enabled(),
        "DECOM state must not change after a Query"
    );
}

#[test]
fn decom_enable_homes_cursor_to_region_top() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decom::Decom,
    };

    let mut handler = TerminalHandler::new(80, 24);
    fill_buffer(&mut handler);

    // Set a scroll region lines 5–20 (0-based: rows 4–19).
    handler.handle_set_scroll_region(5, 20);

    // Move cursor somewhere random.
    handler.handle_cursor_pos(Some(10), Some(15));
    assert_ne!(handler.buffer().get_cursor().pos.x, 0);

    // Enable DECOM — cursor should home to (0, region_top) in buffer coords.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decom(Decom::new(
        &SetMode::DecSet,
    )))]);

    let cursor = handler.buffer().get_cursor().pos;
    // In DECOM mode, set_cursor_pos(0,0) offsets y by scroll_region_top (4).
    // After fill_buffer, visible_window_start(0) is a known offset; cursor.pos.y
    // relative to the start of the visible window should equal scroll_region_top.
    let rows_len = handler.buffer().get_rows().len();
    let vis_start = rows_len.saturating_sub(24);
    assert_eq!(cursor.x, 0, "DECOM enable must home x to 0");
    assert_eq!(
        cursor.y,
        vis_start + 4,
        "DECOM enable must home cursor to scroll region top"
    );
}

#[test]
fn decom_cursor_pos_is_relative_to_scroll_region() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decom::Decom,
    };

    let mut handler = TerminalHandler::new(80, 24);
    fill_buffer(&mut handler);

    // Set scroll region 5–15 (0-based: 4–14).
    handler.handle_set_scroll_region(5, 15);

    // Enable DECOM.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decom(Decom::new(
        &SetMode::DecSet,
    )))]);

    // CUP to row 3, col 5 (1-based) → within-region row 2, col 4 (0-based).
    // With DECOM, row 3 maps to screen row scroll_region_top + 2 = 4 + 2 = 6.
    // handle_cursor_pos(x=col, y=row)
    handler.handle_cursor_pos(Some(5), Some(3));

    let cursor = handler.buffer().get_cursor().pos;
    let rows_len = handler.buffer().get_rows().len();
    let vis_start = rows_len.saturating_sub(24);
    // handle_cursor_pos converts 1-based to 0-based, then set_cursor_pos adds
    // scroll_region_top (4).
    assert_eq!(
        cursor.x, 4,
        "DECOM cursor x should be col 4 (0-based from 5)"
    );
    assert_eq!(
        cursor.y,
        vis_start + 6,
        "DECOM cursor y should be vis_start + region_top(4) + row(2) = vis_start + 6"
    );
}

#[test]
fn decom_cursor_clamped_to_scroll_region() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decom::Decom,
    };

    let mut handler = TerminalHandler::new(80, 24);
    fill_buffer(&mut handler);

    // Set scroll region 5–10 (0-based: 4–9, region height = 5).
    handler.handle_set_scroll_region(5, 10);

    // Enable DECOM.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decom(Decom::new(
        &SetMode::DecSet,
    )))]);

    // CUP to row 100 (way beyond region) — should clamp to region bottom.
    // handle_cursor_pos(x=col, y=row)
    handler.handle_cursor_pos(Some(1), Some(100));

    let cursor = handler.buffer().get_cursor().pos;
    let rows_len = handler.buffer().get_rows().len();
    let vis_start = rows_len.saturating_sub(24);
    // Region height = bottom - top = 9 - 4 = 5 rows (rows 4..=9).
    // Clamped row = min(99, 5) = 5 → screen row = 4 + 5 = 9.
    assert_eq!(
        cursor.y,
        vis_start + 9,
        "DECOM cursor y should clamp to bottom of scroll region"
    );
}

#[test]
fn decom_disable_homes_cursor_to_screen_top() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decom::Decom,
    };

    let mut handler = TerminalHandler::new(80, 24);
    fill_buffer(&mut handler);

    // Set scroll region, enable DECOM, move cursor somewhere.
    handler.handle_set_scroll_region(5, 20);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decom(Decom::new(
        &SetMode::DecSet,
    )))]);
    handler.handle_cursor_pos(Some(5), Some(10));

    // Disable DECOM — cursor should home to (0, 0) screen-relative.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decom(Decom::new(
        &SetMode::DecRst,
    )))]);

    let cursor = handler.buffer().get_cursor().pos;
    let rows_len = handler.buffer().get_rows().len();
    let vis_start = rows_len.saturating_sub(24);
    assert_eq!(cursor.x, 0, "DECOM disable must home x to 0");
    assert_eq!(
        cursor.y, vis_start,
        "DECOM disable must home cursor to screen row 0"
    );
}

#[test]
fn decrpm_decom_default_is_reset() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decom::Decom,
    };

    let mut handler = TerminalHandler::new(80, 24);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::Decom(Decom::new(&SetMode::DecQuery))),
    );
    // Default DECOM is NormalCursor (disabled) → Ps=2 (reset)
    assert_eq!(resp, "\x1b[?6;2$y", "DECOM default (NormalCursor) → Ps=2");
}

#[test]
fn decrpm_decom_after_enable() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decom::Decom,
    };

    let mut handler = TerminalHandler::new(80, 24);
    // Enable DECOM
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decom(Decom::new(
        &SetMode::DecSet,
    )))]);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::Decom(Decom::new(&SetMode::DecQuery))),
    );
    // After enable → Ps=1 (set)
    assert_eq!(resp, "\x1b[?6;1$y", "DECOM after enable → Ps=1");
}

// ---------------------------------------------------------------------------
// DECCOLM (132-column mode) tests
// ---------------------------------------------------------------------------

#[test]
fn deccolm_set_switches_to_132_columns() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::deccolm::Deccolm,
    };

    let mut handler = TerminalHandler::new(80, 24);

    // Write some data so the buffer is not empty.
    handler.process_outputs(&[TerminalOutput::Data(b"Hello, world!".to_vec())]);

    // Switch to 132-column mode.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::new(
        &SetMode::DecSet,
    )))]);

    // Buffer width should be 132.
    assert_eq!(
        handler.buffer().terminal_width(),
        132,
        "DECCOLM set → 132 cols"
    );
    // Cursor should be at home (0, 0 screen-relative).
    let cursor = handler.buffer().get_cursor().pos;
    let vis_start = handler.buffer().get_rows().len().saturating_sub(24);
    assert_eq!(cursor.x, 0, "DECCOLM set → cursor x = 0");
    assert_eq!(cursor.y, vis_start, "DECCOLM set → cursor y = vis_start");
}

#[test]
fn deccolm_reset_switches_to_80_columns() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::deccolm::Deccolm,
    };

    let mut handler = TerminalHandler::new(80, 24);

    // First switch to 132.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::new(
        &SetMode::DecSet,
    )))]);
    assert_eq!(handler.buffer().terminal_width(), 132);

    // Write some data.
    handler.process_outputs(&[TerminalOutput::Data(b"Test data".to_vec())]);

    // Switch back to 80.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::new(
        &SetMode::DecRst,
    )))]);

    assert_eq!(
        handler.buffer().terminal_width(),
        80,
        "DECCOLM reset → 80 cols"
    );
    let cursor = handler.buffer().get_cursor().pos;
    let vis_start = handler.buffer().get_rows().len().saturating_sub(24);
    assert_eq!(cursor.x, 0, "DECCOLM reset → cursor x = 0");
    assert_eq!(cursor.y, vis_start, "DECCOLM reset → cursor y = vis_start");
}

#[test]
fn deccolm_blocked_by_allow_column_mode_switch() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::allow_column_mode_switch::AllowColumnModeSwitch,
        modes::deccolm::Deccolm,
    };

    let mut handler = TerminalHandler::new(80, 24);

    // Disable column mode switching.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowColumnModeSwitch(
        AllowColumnModeSwitch::new(&SetMode::DecRst),
    ))]);

    // Attempt to switch to 132 columns — should be blocked.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::new(
        &SetMode::DecSet,
    )))]);

    // Width should remain 80.
    assert_eq!(
        handler.buffer().terminal_width(),
        80,
        "DECCOLM blocked → width unchanged"
    );
}

#[test]
fn deccolm_resets_decom() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::deccolm::Deccolm,
        modes::decom::Decom,
    };

    let mut handler = TerminalHandler::new(80, 24);

    // Enable DECOM.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decom(Decom::new(
        &SetMode::DecSet,
    )))]);
    assert!(handler.buffer().is_decom_enabled());

    // Switch to 132-column mode — should reset DECOM.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::new(
        &SetMode::DecSet,
    )))]);

    assert!(
        !handler.buffer().is_decom_enabled(),
        "DECCOLM set must reset DECOM"
    );
}

#[test]
fn deccolm_resets_scroll_region() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::deccolm::Deccolm,
    };

    let mut handler = TerminalHandler::new(80, 24);

    // Set a non-default scroll region.
    handler.handle_set_scroll_region(5, 20);
    assert_eq!(handler.buffer().scroll_region(), (4, 19));

    // Switch to 132 columns — scroll region should reset to full screen.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::new(
        &SetMode::DecSet,
    )))]);

    assert_eq!(
        handler.buffer().scroll_region(),
        (0, 23),
        "DECCOLM set must reset scroll region to full screen"
    );
}

#[test]
fn decrpm_deccolm_default_is_80() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::deccolm::Deccolm,
    };

    let mut handler = TerminalHandler::new(80, 24);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::Deccolm(Deccolm::new(&SetMode::DecQuery))),
    );
    // Default is 80 columns → Ps=2 (reset)
    assert_eq!(resp, "\x1b[?3;2$y", "DECCOLM default → Ps=2 (80-col)");
}

#[test]
fn deccolm_reset_restores_original_non_80_width() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::deccolm::Deccolm,
    };

    // Start with a 96-column terminal (not 80).
    let mut handler = TerminalHandler::new(96, 24);
    assert_eq!(handler.buffer().terminal_width(), 96);

    // Switch to 132-column mode.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::new(
        &SetMode::DecSet,
    )))]);
    assert_eq!(handler.buffer().terminal_width(), 132);

    // Switch back via CSI?3l — should restore 96, not hardcode 80.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::new(
        &SetMode::DecRst,
    )))]);

    assert_eq!(
        handler.buffer().terminal_width(),
        96,
        "DECCOLM reset must restore original width (96), not hardcode 80"
    );
}

#[test]
fn ris_restores_original_non_80_width_after_deccolm() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::deccolm::Deccolm,
    };

    // Start with a 96-column terminal (not 80).
    let mut handler = TerminalHandler::new(96, 24);
    assert_eq!(handler.buffer().terminal_width(), 96);

    // Switch to 132-column mode.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Deccolm(Deccolm::new(
        &SetMode::DecSet,
    )))]);
    assert_eq!(handler.buffer().terminal_width(), 132);

    // Full reset (RIS) — should restore 96, not hardcode 80.
    handler.process_outputs(&[TerminalOutput::ResetDevice]);

    assert_eq!(
        handler.buffer().terminal_width(),
        96,
        "RIS after DECCOLM must restore original width (96), not hardcode 80"
    );
}

// ============================================================================
// Regression tests for tmux scroll region / CPR fixes
// ============================================================================

/// Bug 1 regression: CPR must report screen-relative row, not absolute buffer row.
///
/// When scrollback exists, `cursor.pos.y` is an absolute index into the row
/// vector.  CPR must subtract `visible_window_start` to report the screen row.
#[test]
fn cursor_report_with_scrollback_reports_screen_row() {
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();

    let mut handler = TerminalHandler::new(80, 24);
    handler.set_write_tx(tx);

    // Generate scrollback by outputting 30 lines (more than 24-row height)
    for i in 0..30 {
        handler.process_outputs(&[TerminalOutput::Data(format!("line {i}\n").into_bytes())]);
    }

    // Cursor should be on the last visible screen row (or near it).
    // Move cursor to a known screen position.
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: Some(5),  // 1-indexed → col 4
        y: Some(10), // 1-indexed → screen row 9
    }]);

    handler.process_outputs(&[TerminalOutput::CursorReport]);

    // Drain to the last message (SetCursorPos doesn't write, but Data does write newlines)
    let mut last_msg = None;
    while let Ok(msg) = rx.try_recv() {
        last_msg = Some(msg);
    }
    let bytes = match last_msg.expect("CursorReport must send a message") {
        PtyWrite::Write(b) => b,
        other => panic!("expected PtyWrite::Write, got {other:?}"),
    };
    let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
    assert_eq!(
        response, "\x1b[10;5R",
        "CPR must report screen-relative row (10), not absolute buffer row"
    );
}

/// Bug 1 regression: CPR with DECOM reports region-relative row.
#[test]
fn cursor_report_with_decom_reports_region_relative_row() {
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();

    let mut handler = TerminalHandler::new(80, 24);
    handler.set_write_tx(tx);

    // Set scroll region rows 5..20 (1-based), then enable DECOM
    handler.process_outputs(&[TerminalOutput::SetTopAndBottomMargins {
        top_margin: 5,
        bottom_margin: 20,
    }]);
    handler.process_outputs(&[TerminalOutput::Mode(
        freminal_common::buffer_states::mode::Mode::Decom(
            freminal_common::buffer_states::modes::decom::Decom::OriginMode,
        ),
    )]);

    // Move cursor to row 3 within the region (1-indexed, region-relative)
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: Some(1),
        y: Some(3), // region-relative row 3
    }]);

    handler.process_outputs(&[TerminalOutput::CursorReport]);

    let mut last_msg = None;
    while let Ok(msg) = rx.try_recv() {
        last_msg = Some(msg);
    }
    let bytes = match last_msg.expect("CursorReport must send a message") {
        PtyWrite::Write(b) => b,
        other => panic!("expected PtyWrite::Write, got {other:?}"),
    };
    let response = String::from_utf8(bytes).expect("response must be valid UTF-8");
    assert_eq!(
        response, "\x1b[3;1R",
        "CPR with DECOM must report region-relative row (3)"
    );
}

/// Bug 2 regression: DECSTBM always homes cursor to screen origin (0,0),
/// not to the region top.
#[test]
fn decstbm_homes_cursor_to_screen_origin() {
    let mut handler = TerminalHandler::new(80, 24);

    // Move cursor to row 12
    handler.process_outputs(&[TerminalOutput::SetCursorPos {
        x: Some(10),
        y: Some(12),
    }]);

    // Set scroll region
    handler.process_outputs(&[TerminalOutput::SetTopAndBottomMargins {
        top_margin: 5,
        bottom_margin: 20,
    }]);

    // Cursor must be at screen row 0, col 0
    let screen_pos = handler.buffer().get_cursor_screen_pos();
    assert_eq!(screen_pos.y, 0, "DECSTBM must home cursor to screen row 0");
    assert_eq!(screen_pos.x, 0, "DECSTBM must home cursor to column 0");
}

/// Bug 4 regression: RI at screen top with full-screen region must scroll down.
#[test]
fn ri_at_screen_top_full_region_scrolls_down() {
    use freminal_buffer::buffer::Buffer;
    use freminal_common::buffer_states::tchar::TChar;

    let mut buf = Buffer::new(10, 5);

    // Fill all 5 rows with identifiable content
    for i in 0..5 {
        let count = i + 1;
        let chars: Vec<TChar> = (0..count).map(|_| TChar::Ascii(b'X')).collect();
        buf.insert_text(&chars);
        if i < 4 {
            buf.handle_lf();
            buf.handle_cr();
        }
    }

    // Verify initial state: lengths [1,2,3,4,5]
    let lengths: Vec<usize> = buf
        .visible_rows(0)
        .iter()
        .map(|r| r.get_characters().len())
        .collect();
    assert_eq!(lengths, vec![1, 2, 3, 4, 5]);

    // Move cursor to screen top (row 0)
    buf.set_cursor_pos(Some(0), Some(0));

    // Full-screen region (default). RI at top should scroll screen down.
    buf.handle_ri();

    let lengths_after: Vec<usize> = buf
        .visible_rows(0)
        .iter()
        .map(|r| r.get_characters().len())
        .collect();

    // Row 0 should be blank (scrolled in), rows shift down, row 4 (length 5) falls off
    assert_eq!(
        lengths_after,
        vec![0, 1, 2, 3, 4],
        "RI at screen top with full-screen region must scroll content down"
    );
}

/// Bug 5 regression: Scroll region operations work during early buffer fill.
#[test]
fn scroll_region_works_during_early_buffer_fill() {
    use freminal_buffer::buffer::Buffer;
    use freminal_common::buffer_states::tchar::TChar;

    // Create a 10x10 buffer but only fill 3 rows
    let mut buf = Buffer::new(10, 10);
    for i in 0..3 {
        let chars: Vec<TChar> = (0..=i).map(|_| TChar::Ascii(b'X')).collect();
        buf.insert_text(&chars);
        if i < 2 {
            buf.handle_lf();
            buf.handle_cr();
        }
    }

    // Set a scroll region that extends beyond current rows
    buf.set_scroll_region(2, 8);

    // Move cursor to bottom of region
    buf.set_cursor_pos(Some(0), Some(7));

    // LF at bottom of region should scroll the region up without panicking
    buf.handle_lf();

    // Verify buffer is sane
    assert!(
        buf.get_rows().len() >= 8,
        "Buffer should have grown to accommodate scroll region"
    );
}

// ============================================================================
// Alternate buffer resize regression tests
//
// Root cause: `resize_height` did not truncate rows in the alternate buffer
// when shrinking, leaving `rows.len() > height`.  This broke the invariant
// that alternate buffer coordinates (`cursor.pos.y`) equal screen coordinates,
// causing `handle_lf`, `handle_ri`, `insert_lines`, and `delete_lines` to
// silently malfunction (cursor.pos.y would fall outside the scroll region
// range even when the cursor was visually at the bottom of the screen).
// ============================================================================

/// After shrinking the alternate buffer, `rows.len()` must equal `new_height`.
#[test]
fn alt_buffer_resize_shrink_maintains_row_count_invariant() {
    use freminal_buffer::buffer::Buffer;

    let mut buf = Buffer::new(80, 24);
    buf.enter_alternate(0);

    assert_eq!(buf.get_rows().len(), 24, "pre-condition");

    // Shrink from 24 → 20
    let _ = buf.set_size(80, 20, 0);

    assert_eq!(
        buf.get_rows().len(),
        20,
        "alternate buffer must have exactly `height` rows after shrink"
    );
}

/// After growing the alternate buffer, `rows.len()` must equal `new_height`.
#[test]
fn alt_buffer_resize_grow_maintains_row_count_invariant() {
    use freminal_buffer::buffer::Buffer;

    let mut buf = Buffer::new(80, 24);
    buf.enter_alternate(0);

    assert_eq!(buf.get_rows().len(), 24, "pre-condition");

    // Grow from 24 → 30
    let _ = buf.set_size(80, 30, 0);

    assert_eq!(
        buf.get_rows().len(),
        30,
        "alternate buffer must have exactly `height` rows after grow"
    );
}

/// LF at the bottom of the scroll region must trigger scrolling after a
/// height shrink.  This was the exact symptom reported: tmux pane content
/// overwrote itself instead of scrolling after a window resize.
#[test]
fn alt_buffer_lf_scrolls_after_resize_shrink() {
    use freminal_buffer::buffer::Buffer;
    use freminal_common::buffer_states::tchar::TChar;

    let mut buf = Buffer::new(80, 24);
    buf.enter_alternate(0);

    // Set scroll region to full screen (1-based inclusive: 1..24)
    buf.set_scroll_region(1, 24);

    // Fill every row with a distinct marker character
    for row in 0..24 {
        let ch = b'A' + (row as u8);
        let chars: Vec<TChar> = vec![TChar::Ascii(ch); 80];
        buf.set_cursor_pos(Some(0), Some(row));
        buf.insert_text(&chars);
    }

    // Shrink from 24 → 20
    let _ = buf.set_size(80, 20, 0);

    // After resize, scroll region is reset/clamped to full screen (0..19).
    // Place cursor at the bottom of the visible screen.
    buf.set_cursor_pos(Some(0), Some(19));

    // LF at scroll_region_bottom should scroll the region up.
    // Before the fix, this was a no-op because cursor.pos.y (e.g. 23)
    // was > scroll_region_bottom (19), so the scroll_region check failed.
    let bottom_tchar_before = buf.get_rows()[19].resolve_cell(0).tchar().clone();

    buf.handle_lf();

    // The old bottom row should have been replaced with a blank row
    let bottom_tchar_after = buf.get_rows()[19].resolve_cell(0).tchar().clone();

    assert_ne!(
        bottom_tchar_before, bottom_tchar_after,
        "LF at scroll_region_bottom must scroll the region (bottom row should change)"
    );

    // The new bottom row should be blank (Space)
    assert_eq!(
        bottom_tchar_after,
        TChar::Space,
        "new bottom row after LF scroll should be blank"
    );
}

/// DL (delete lines) must work correctly in the alternate buffer after a
/// height shrink.  tmux uses DL extensively for pane scrolling.
#[test]
fn alt_buffer_delete_lines_works_after_resize_shrink() {
    use freminal_buffer::buffer::Buffer;
    use freminal_common::buffer_states::tchar::TChar;

    let mut buf = Buffer::new(80, 24);
    buf.enter_alternate(0);

    // Fill screen
    for row in 0..24 {
        let ch = b'A' + (row as u8);
        let chars: Vec<TChar> = vec![TChar::Ascii(ch); 80];
        buf.set_cursor_pos(Some(0), Some(row));
        buf.insert_text(&chars);
    }

    // Shrink from 24 → 20
    let _ = buf.set_size(80, 20, 0);

    // Place cursor at row 5 (inside scroll region)
    buf.set_cursor_pos(Some(0), Some(5));

    // Read the character at row 6 before DL
    let row_6_tchar_before = buf.get_rows()[6].resolve_cell(0).tchar().clone();

    // Delete 1 line at cursor position
    buf.delete_lines(1);

    // Row 6's content should now be at row 5 (shifted up by DL)
    let row_5_tchar_after = buf.get_rows()[5].resolve_cell(0).tchar().clone();

    assert_eq!(
        row_5_tchar_after, row_6_tchar_before,
        "DL should shift row 6 content up to row 5"
    );

    // Bottom of scroll region should be blank after DL
    let bottom_tchar = buf.get_rows()[19].resolve_cell(0).tchar().clone();
    assert_eq!(
        bottom_tchar,
        TChar::Space,
        "bottom row should be blank after DL"
    );
}

/// IL (insert lines) must work correctly in the alternate buffer after a
/// height shrink.  tmux uses IL for pane scrolling.
#[test]
fn alt_buffer_insert_lines_works_after_resize_shrink() {
    use freminal_buffer::buffer::Buffer;
    use freminal_common::buffer_states::tchar::TChar;

    let mut buf = Buffer::new(80, 24);
    buf.enter_alternate(0);

    // Fill screen
    for row in 0..24 {
        let ch = b'A' + (row as u8);
        let chars: Vec<TChar> = vec![TChar::Ascii(ch); 80];
        buf.set_cursor_pos(Some(0), Some(row));
        buf.insert_text(&chars);
    }

    // Shrink from 24 → 20
    let _ = buf.set_size(80, 20, 0);

    // Place cursor at row 5
    buf.set_cursor_pos(Some(0), Some(5));

    // Read the character at row 5 before IL
    let row_5_tchar_before = buf.get_rows()[5].resolve_cell(0).tchar().clone();

    // Insert 1 line at cursor position
    buf.insert_lines(1);

    // Row 5's content should have shifted down to row 6
    let row_6_tchar_after = buf.get_rows()[6].resolve_cell(0).tchar().clone();
    assert_eq!(
        row_6_tchar_after, row_5_tchar_before,
        "IL should shift row 5 content down to row 6"
    );

    // Row 5 itself should now be blank (the inserted line)
    let row_5_tchar_after = buf.get_rows()[5].resolve_cell(0).tchar().clone();
    assert_eq!(
        row_5_tchar_after,
        TChar::Space,
        "inserted row should be blank"
    );
}

/// RI (reverse index) at scroll_region_top must scroll down after a height
/// shrink.
#[test]
fn alt_buffer_ri_scrolls_after_resize_shrink() {
    use freminal_buffer::buffer::Buffer;
    use freminal_common::buffer_states::tchar::TChar;

    let mut buf = Buffer::new(80, 24);
    buf.enter_alternate(0);

    // Fill screen
    for row in 0..24 {
        let ch = b'A' + (row as u8);
        let chars: Vec<TChar> = vec![TChar::Ascii(ch); 80];
        buf.set_cursor_pos(Some(0), Some(row));
        buf.insert_text(&chars);
    }

    // Shrink from 24 → 20
    let _ = buf.set_size(80, 20, 0);

    // Place cursor at row 0 (top of scroll region)
    buf.set_cursor_pos(Some(0), Some(0));

    // Read the character at row 0 before RI
    let row_0_tchar_before = buf.get_rows()[0].resolve_cell(0).tchar().clone();

    // RI at scroll_region_top should scroll the region down
    buf.handle_ri();

    // Row 0's content should have moved to row 1
    let row_1_tchar_after = buf.get_rows()[1].resolve_cell(0).tchar().clone();
    assert_eq!(
        row_1_tchar_after, row_0_tchar_before,
        "RI should shift row 0 content down to row 1"
    );

    // Row 0 should now be blank
    let row_0_tchar_after = buf.get_rows()[0].resolve_cell(0).tchar().clone();
    assert_eq!(
        row_0_tchar_after,
        TChar::Space,
        "top row should be blank after RI"
    );
}

/// Multiple shrink-grow cycles must maintain the invariant.
#[test]
fn alt_buffer_resize_multiple_cycles_maintain_invariant() {
    use freminal_buffer::buffer::Buffer;

    let mut buf = Buffer::new(80, 24);
    buf.enter_alternate(0);

    // Shrink → grow → shrink → grow
    for &new_height in &[20_usize, 30, 15, 24] {
        let _ = buf.set_size(80, new_height, 0);
        assert_eq!(
            buf.get_rows().len(),
            new_height,
            "alternate buffer rows.len() must equal height={new_height} after resize"
        );
    }
}

/// Simulates the exact tmux failure scenario: alternate buffer is active,
/// DECSTBM is set, content is written, then the window is resized.  After
/// resize, new output must scroll correctly.
#[test]
fn alt_buffer_tmux_resize_scenario() {
    use freminal_buffer::buffer::Buffer;
    use freminal_common::buffer_states::tchar::TChar;

    let mut buf = Buffer::new(80, 24);
    buf.enter_alternate(0);

    // tmux sets DECSTBM to (1, height-1) for the top pane, leaving the
    // bottom row for the status bar.  set_scroll_region takes 1-based
    // inclusive params matching DECSTBM.
    buf.set_scroll_region(1, 23); // 0-based rows 0..22 scroll, row 23 is status

    // Fill the scroll region with content
    for row in 0..23 {
        let ch = b'A' + (row as u8);
        let chars: Vec<TChar> = vec![TChar::Ascii(ch); 80];
        buf.set_cursor_pos(Some(0), Some(row));
        buf.insert_text(&chars);
    }

    // Write status bar
    let status: Vec<TChar> = b"[tmux] 0:bash".iter().map(|&b| TChar::Ascii(b)).collect();
    buf.set_cursor_pos(Some(0), Some(23));
    buf.insert_text(&status);

    // Now resize the window from 24 → 18 rows
    let _ = buf.set_size(80, 18, 0);

    assert_eq!(buf.get_rows().len(), 18, "post-resize row count");

    // After resize, scroll region is clamped/reset.
    // tmux would re-send DECSTBM for the new size (1-based inclusive):
    buf.set_scroll_region(1, 17); // 0-based rows 0..16 scroll, row 17 is status

    // Place cursor at the bottom of the scroll region
    buf.set_cursor_pos(Some(0), Some(16));

    // Write new content and LF — this must scroll, not overwrite
    let new_content: Vec<TChar> = b"New output line"
        .iter()
        .map(|&b| TChar::Ascii(b))
        .collect();
    buf.insert_text(&new_content);
    buf.handle_lf();

    // Cursor should still be at row 16 (bottom of region, after scroll)
    assert_eq!(
        buf.get_cursor().pos.y,
        16,
        "cursor should stay at scroll region bottom after LF-triggered scroll"
    );

    // The new bottom row (16) should be blank (LF scrolled the region)
    let bottom_tchar = buf.get_rows()[16].resolve_cell(0).tchar().clone();
    assert_eq!(
        bottom_tchar,
        TChar::Space,
        "bottom of scroll region should be blank after LF scroll"
    );
}

// ============================================================================
// Alternate buffer width-change resize regression tests
//
// Root cause: `reflow_to_width` was called unconditionally for all buffer
// types.  For the alternate buffer, reflow re-wraps logical lines which can
// produce more or fewer rows than `height`, breaking the invariant that
// alternate buffers always have exactly `height` rows.  This caused the
// exact same class of symptom as the height-shrink bug: coordinates
// desynchronised, operations silently malfunctioned, and tmux content
// disappeared or overwrote itself after making the window narrower.
// ============================================================================

/// Shrinking the width of the alternate buffer must NOT change the row count.
/// Before the fix, `reflow_to_width` would re-wrap long lines and produce
/// extra rows.
#[test]
fn alt_buffer_width_shrink_maintains_row_count_invariant() {
    use freminal_buffer::buffer::Buffer;
    use freminal_common::buffer_states::tchar::TChar;

    let mut buf = Buffer::new(80, 24);
    buf.enter_alternate(0);

    // Fill every row to full width so reflow would have split them.
    for row in 0..24 {
        let ch = b'A' + (row as u8);
        let chars: Vec<TChar> = vec![TChar::Ascii(ch); 80];
        buf.set_cursor_pos(Some(0), Some(row));
        buf.insert_text(&chars);
    }

    // Shrink width only: 80 → 40 (height stays 24).
    let _ = buf.set_size(40, 24, 0);

    assert_eq!(
        buf.get_rows().len(),
        24,
        "alternate buffer must still have exactly `height` rows after width shrink"
    );
}

/// Growing the width of the alternate buffer must NOT change the row count.
#[test]
fn alt_buffer_width_grow_maintains_row_count_invariant() {
    use freminal_buffer::buffer::Buffer;
    use freminal_common::buffer_states::tchar::TChar;

    let mut buf = Buffer::new(80, 24);
    buf.enter_alternate(0);

    // Fill rows with content
    for row in 0..24 {
        let ch = b'A' + (row as u8);
        let chars: Vec<TChar> = vec![TChar::Ascii(ch); 80];
        buf.set_cursor_pos(Some(0), Some(row));
        buf.insert_text(&chars);
    }

    // Grow width only: 80 → 120 (height stays 24).
    let _ = buf.set_size(120, 24, 0);

    assert_eq!(
        buf.get_rows().len(),
        24,
        "alternate buffer must still have exactly `height` rows after width grow"
    );
}

/// Simultaneous width shrink + height shrink must maintain the invariant.
/// This is the exact scenario from the bug report: making the window smaller
/// in both dimensions while tmux is running.
#[test]
fn alt_buffer_width_and_height_shrink_maintains_invariant() {
    use freminal_buffer::buffer::Buffer;
    use freminal_common::buffer_states::tchar::TChar;

    let mut buf = Buffer::new(80, 24);
    buf.enter_alternate(0);

    // Fill rows with full-width content
    for row in 0..24 {
        let ch = b'A' + (row as u8);
        let chars: Vec<TChar> = vec![TChar::Ascii(ch); 80];
        buf.set_cursor_pos(Some(0), Some(row));
        buf.insert_text(&chars);
    }

    // Shrink both: 80x24 → 40x18
    let _ = buf.set_size(40, 18, 0);

    assert_eq!(
        buf.get_rows().len(),
        18,
        "alternate buffer must have exactly `new_height` rows after combined shrink"
    );
}

/// After a width-only shrink, LF at the bottom of the scroll region must
/// still trigger scrolling.
#[test]
fn alt_buffer_lf_works_after_width_shrink() {
    use freminal_buffer::buffer::Buffer;
    use freminal_common::buffer_states::tchar::TChar;

    let mut buf = Buffer::new(80, 24);
    buf.enter_alternate(0);

    // Fill rows
    for row in 0..24 {
        let ch = b'A' + (row as u8);
        let chars: Vec<TChar> = vec![TChar::Ascii(ch); 80];
        buf.set_cursor_pos(Some(0), Some(row));
        buf.insert_text(&chars);
    }

    // Width-only shrink: 80 → 40
    let _ = buf.set_size(40, 24, 0);

    // Cursor at bottom of screen
    buf.set_cursor_pos(Some(0), Some(23));
    let bottom_before = buf.get_rows()[23].resolve_cell(0).tchar().clone();

    buf.handle_lf();

    // Bottom row should be blank (scroll happened)
    let bottom_after = buf.get_rows()[23].resolve_cell(0).tchar().clone();
    assert_ne!(
        bottom_before, bottom_after,
        "LF at bottom must scroll after width-only shrink"
    );
    assert_eq!(
        bottom_after,
        TChar::Space,
        "new bottom row should be blank after LF scroll"
    );
}

// ===========================================================================
// modifyOtherKeys + Application Escape Key handler dispatch tests
// ===========================================================================

// ── Default state ────────────────────────────────────────────────────────

#[test]
fn handler_default_modify_other_keys_level_is_zero() {
    let handler = TerminalHandler::new(80, 24);
    assert_eq!(handler.modify_other_keys_level(), 0);
}

#[test]
fn handler_default_application_escape_key_is_false() {
    let handler = TerminalHandler::new(80, 24);
    assert!(!handler.application_escape_key());
}

// ── ModifyOtherKeys via CSI > 4 ; Pv m (TerminalOutput variant) ─────────

#[test]
fn handler_modify_other_keys_set_level_2() {
    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[TerminalOutput::ModifyOtherKeys(2)]);
    assert_eq!(handler.modify_other_keys_level(), 2);
}

#[test]
fn handler_modify_other_keys_set_level_1() {
    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[TerminalOutput::ModifyOtherKeys(1)]);
    assert_eq!(handler.modify_other_keys_level(), 1);
}

#[test]
fn handler_modify_other_keys_set_level_0() {
    let mut handler = TerminalHandler::new(80, 24);
    // First set to 2, then reset to 0
    handler.process_outputs(&[TerminalOutput::ModifyOtherKeys(2)]);
    handler.process_outputs(&[TerminalOutput::ModifyOtherKeys(0)]);
    assert_eq!(handler.modify_other_keys_level(), 0);
}

// ── Application Escape Key (?7727) via Mode dispatch ────────────────────

#[test]
fn handler_application_escape_key_set() {
    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::ApplicationEscapeKey(
        ApplicationEscapeKey::Set,
    ))]);
    assert!(handler.application_escape_key());
}

#[test]
fn handler_application_escape_key_reset() {
    let mut handler = TerminalHandler::new(80, 24);
    // Set then reset
    handler.process_outputs(&[TerminalOutput::Mode(Mode::ApplicationEscapeKey(
        ApplicationEscapeKey::Set,
    ))]);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::ApplicationEscapeKey(
        ApplicationEscapeKey::Reset,
    ))]);
    assert!(!handler.application_escape_key());
}

#[test]
fn handler_application_escape_key_query_when_false() {
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let mut handler = TerminalHandler::new(80, 24);
    handler.set_write_tx(tx);

    handler.process_outputs(&[TerminalOutput::Mode(Mode::ApplicationEscapeKey(
        ApplicationEscapeKey::Query,
    ))]);

    let msg = rx.try_recv().expect("expected DECRQM response");
    match msg {
        PtyWrite::Write(bytes) => {
            let s = String::from_utf8(bytes).expect("valid UTF-8");
            assert_eq!(
                s, "\x1b[?7727;2$y",
                "query when false should report mode 2 (reset)"
            );
        }
        other => panic!("expected PtyWrite::Write, got {other:?}"),
    }
}

#[test]
fn handler_application_escape_key_query_when_true() {
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let mut handler = TerminalHandler::new(80, 24);
    handler.set_write_tx(tx);

    // Set first
    handler.process_outputs(&[TerminalOutput::Mode(Mode::ApplicationEscapeKey(
        ApplicationEscapeKey::Set,
    ))]);

    handler.process_outputs(&[TerminalOutput::Mode(Mode::ApplicationEscapeKey(
        ApplicationEscapeKey::Query,
    ))]);

    let msg = rx.try_recv().expect("expected DECRQM response");
    match msg {
        PtyWrite::Write(bytes) => {
            let s = String::from_utf8(bytes).expect("valid UTF-8");
            assert_eq!(
                s, "\x1b[?7727;1$y",
                "query when true should report mode 1 (set)"
            );
        }
        other => panic!("expected PtyWrite::Write, got {other:?}"),
    }
}

// ── ModifyOtherKeysMode (?2048) via Mode dispatch ───────────────────────

#[test]
fn handler_modify_other_keys_mode_dec_set() {
    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::ModifyOtherKeysMode(
        ModifyOtherKeysMode::Set,
    ))]);
    assert_eq!(handler.modify_other_keys_level(), 1);
}

#[test]
fn handler_modify_other_keys_mode_dec_rst() {
    let mut handler = TerminalHandler::new(80, 24);
    // Set via CSI then reset via DEC mode
    handler.process_outputs(&[TerminalOutput::ModifyOtherKeys(2)]);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::ModifyOtherKeysMode(
        ModifyOtherKeysMode::Reset,
    ))]);
    assert_eq!(handler.modify_other_keys_level(), 0);
}

#[test]
fn handler_modify_other_keys_mode_query_when_level_zero() {
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let mut handler = TerminalHandler::new(80, 24);
    handler.set_write_tx(tx);

    handler.process_outputs(&[TerminalOutput::Mode(Mode::ModifyOtherKeysMode(
        ModifyOtherKeysMode::Query,
    ))]);

    let msg = rx.try_recv().expect("expected DECRQM response");
    match msg {
        PtyWrite::Write(bytes) => {
            let s = String::from_utf8(bytes).expect("valid UTF-8");
            assert_eq!(
                s, "\x1b[?2048;2$y",
                "query when level=0 should report mode 2 (reset)"
            );
        }
        other => panic!("expected PtyWrite::Write, got {other:?}"),
    }
}

#[test]
fn handler_modify_other_keys_mode_query_when_level_nonzero() {
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let mut handler = TerminalHandler::new(80, 24);
    handler.set_write_tx(tx);

    // Set level to 2 via CSI > 4 ; 2 m
    handler.process_outputs(&[TerminalOutput::ModifyOtherKeys(2)]);

    handler.process_outputs(&[TerminalOutput::Mode(Mode::ModifyOtherKeysMode(
        ModifyOtherKeysMode::Query,
    ))]);

    let msg = rx.try_recv().expect("expected DECRQM response");
    match msg {
        PtyWrite::Write(bytes) => {
            let s = String::from_utf8(bytes).expect("valid UTF-8");
            assert_eq!(
                s, "\x1b[?2048;1$y",
                "query when level>0 should report mode 1 (set)"
            );
        }
        other => panic!("expected PtyWrite::Write, got {other:?}"),
    }
}

// ── Cross-path interaction ──────────────────────────────────────────────

#[test]
fn handler_csi_set_then_dec_mode_reset_interaction() {
    let mut handler = TerminalHandler::new(80, 24);
    // Set via CSI > 4 ; 2 m
    handler.process_outputs(&[TerminalOutput::ModifyOtherKeys(2)]);
    assert_eq!(handler.modify_other_keys_level(), 2);

    // Reset via DECRST ?2048
    handler.process_outputs(&[TerminalOutput::Mode(Mode::ModifyOtherKeysMode(
        ModifyOtherKeysMode::Reset,
    ))]);
    assert_eq!(handler.modify_other_keys_level(), 0);
}

// ── full_reset clears both fields ───────────────────────────────────────

#[test]
fn handler_full_reset_clears_modify_other_keys() {
    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[TerminalOutput::ModifyOtherKeys(2)]);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::ApplicationEscapeKey(
        ApplicationEscapeKey::Set,
    ))]);
    assert_eq!(handler.modify_other_keys_level(), 2);
    assert!(handler.application_escape_key());

    handler.full_reset();

    assert_eq!(handler.modify_other_keys_level(), 0);
    assert!(!handler.application_escape_key());
}

// ── Query with no write_tx does not panic ───────────────────────────────

#[test]
fn handler_query_without_write_tx_does_not_panic() {
    let mut handler = TerminalHandler::new(80, 24);
    // No set_write_tx — query should be silently dropped
    handler.process_outputs(&[TerminalOutput::Mode(Mode::ApplicationEscapeKey(
        ApplicationEscapeKey::Query,
    ))]);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::ModifyOtherKeysMode(
        ModifyOtherKeysMode::Query,
    ))]);
    // If we get here without panicking, the test passes
}

// ── Grapheme Clustering (?2027) — permanently set ───────────────────────

#[test]
fn decrpm_grapheme_clustering_always_permanently_set() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::grapheme::GraphemeClustering,
    };

    let mut handler = TerminalHandler::new(80, 24);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::GraphemeClustering(GraphemeClustering::new(
            &SetMode::DecQuery,
        ))),
    );
    assert_eq!(
        resp, "\x1b[?2027;3$y",
        "Grapheme clustering is permanently set → Ps=3"
    );
}

#[test]
fn decrpm_grapheme_clustering_after_reset_still_permanently_set() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::grapheme::GraphemeClustering,
    };

    let mut handler = TerminalHandler::new(80, 24);
    // Send DECRST — should be silently accepted
    handler.process_outputs(&[TerminalOutput::Mode(Mode::GraphemeClustering(
        GraphemeClustering::new(&SetMode::DecRst),
    ))]);
    // Query should still report permanently set
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::GraphemeClustering(GraphemeClustering::new(
            &SetMode::DecQuery,
        ))),
    );
    assert_eq!(
        resp, "\x1b[?2027;3$y",
        "Grapheme clustering remains permanently set even after DECRST"
    );
}

// ── DECSDM (?80) — Sixel Display Mode ──────────────────────────────────

#[test]
fn decrpm_decsdm_default_is_scrolling() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decsdm::Decsdm,
    };

    let mut handler = TerminalHandler::new(80, 24);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::Decsdm(Decsdm::new(&SetMode::DecQuery))),
    );
    assert_eq!(
        resp, "\x1b[?80;2$y",
        "DECSDM default (ScrollingMode) → Ps=2"
    );
}

#[test]
fn decrpm_decsdm_after_set() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decsdm::Decsdm,
    };

    let mut handler = TerminalHandler::new(80, 24);
    // Set DECSDM (Display Mode)
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decsdm(Decsdm::new(
        &SetMode::DecSet,
    )))]);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::Decsdm(Decsdm::new(&SetMode::DecQuery))),
    );
    assert_eq!(
        resp, "\x1b[?80;1$y",
        "DECSDM after DECSET (DisplayMode) → Ps=1"
    );
}

#[test]
fn decrpm_decsdm_after_reset() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decsdm::Decsdm,
    };

    let mut handler = TerminalHandler::new(80, 24);
    // Set then reset
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decsdm(Decsdm::new(
        &SetMode::DecSet,
    )))]);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decsdm(Decsdm::new(
        &SetMode::DecRst,
    )))]);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::Decsdm(Decsdm::new(&SetMode::DecQuery))),
    );
    assert_eq!(
        resp, "\x1b[?80;2$y",
        "DECSDM after DECRST (ScrollingMode) → Ps=2"
    );
}

// ── AllowAltScreen (?1046) — Allow Alternate Screen Switching ──────────

#[test]
fn decrpm_allow_alt_screen_default_is_allow() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::allow_alt_screen::AllowAltScreen,
    };

    let mut handler = TerminalHandler::new(80, 24);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::AllowAltScreen(AllowAltScreen::new(
            &SetMode::DecQuery,
        ))),
    );
    assert_eq!(
        resp, "\x1b[?1046;1$y",
        "AllowAltScreen default (Allow) → Ps=1"
    );
}

#[test]
fn decrpm_allow_alt_screen_after_disallow() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::allow_alt_screen::AllowAltScreen,
    };

    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowAltScreen(
        AllowAltScreen::new(&SetMode::DecRst),
    ))]);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::AllowAltScreen(AllowAltScreen::new(
            &SetMode::DecQuery,
        ))),
    );
    assert_eq!(
        resp, "\x1b[?1046;2$y",
        "AllowAltScreen after DECRST (Disallow) → Ps=2"
    );
}

#[test]
fn decrpm_allow_alt_screen_after_re_enable() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::allow_alt_screen::AllowAltScreen,
    };

    let mut handler = TerminalHandler::new(80, 24);
    // Disallow then re-allow
    handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowAltScreen(
        AllowAltScreen::new(&SetMode::DecRst),
    ))]);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowAltScreen(
        AllowAltScreen::new(&SetMode::DecSet),
    ))]);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::AllowAltScreen(AllowAltScreen::new(
            &SetMode::DecQuery,
        ))),
    );
    assert_eq!(
        resp, "\x1b[?1046;1$y",
        "AllowAltScreen after re-enable → Ps=1"
    );
}

#[test]
fn allow_alt_screen_blocks_enter_alternate_when_disallowed() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::{allow_alt_screen::AllowAltScreen, xtextscrn::XtExtscrn},
    };

    let mut handler = TerminalHandler::new(80, 24);
    // Disallow alternate screen
    handler.process_outputs(&[TerminalOutput::Mode(Mode::AllowAltScreen(
        AllowAltScreen::new(&SetMode::DecRst),
    ))]);
    // Try to enter alternate screen — should be a no-op
    handler.process_outputs(&[TerminalOutput::Mode(Mode::XtExtscrn(XtExtscrn::new(
        &SetMode::DecSet,
    )))]);
    assert!(
        !handler.is_alternate_screen(),
        "Alternate screen should not be entered when AllowAltScreen is Disallow"
    );
}

#[test]
fn allow_alt_screen_permits_enter_alternate_when_allowed() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::xtextscrn::XtExtscrn,
    };

    let mut handler = TerminalHandler::new(80, 24);
    // Default is Allow — entering alternate screen should succeed
    handler.process_outputs(&[TerminalOutput::Mode(Mode::XtExtscrn(XtExtscrn::new(
        &SetMode::DecSet,
    )))]);
    assert!(
        handler.is_alternate_screen(),
        "Alternate screen should be entered when AllowAltScreen is Allow (default)"
    );
}

// ── PrivateColorRegisters (?1070) — Private Color Registers for Sixel ─────

#[test]
fn decrpm_private_color_registers_default_is_private() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::private_color_registers::PrivateColorRegisters,
    };

    let mut handler = TerminalHandler::new(80, 24);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::PrivateColorRegisters(PrivateColorRegisters::new(
            &SetMode::DecQuery,
        ))),
    );
    assert_eq!(
        resp, "\x1b[?1070;1$y",
        "PrivateColorRegisters default (Private) → Ps=1"
    );
}

#[test]
fn decrpm_private_color_registers_after_reset_to_shared() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::private_color_registers::PrivateColorRegisters,
    };

    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::PrivateColorRegisters(
        PrivateColorRegisters::new(&SetMode::DecRst),
    ))]);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::PrivateColorRegisters(PrivateColorRegisters::new(
            &SetMode::DecQuery,
        ))),
    );
    assert_eq!(
        resp, "\x1b[?1070;2$y",
        "PrivateColorRegisters after DECRST (Shared) → Ps=2"
    );
}

#[test]
fn decrpm_private_color_registers_after_reset_then_set() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::private_color_registers::PrivateColorRegisters,
    };

    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::PrivateColorRegisters(
        PrivateColorRegisters::new(&SetMode::DecRst),
    ))]);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::PrivateColorRegisters(
        PrivateColorRegisters::new(&SetMode::DecSet),
    ))]);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::PrivateColorRegisters(PrivateColorRegisters::new(
            &SetMode::DecQuery,
        ))),
    );
    assert_eq!(
        resp, "\x1b[?1070;1$y",
        "PrivateColorRegisters after re-enable (Private) → Ps=1"
    );
}

/// Build a raw DCS sixel sequence: `P<params>q<sixel_body>ESC\`
///
/// Mirrors the private `build_sixel_dcs` helper in the unit tests of
/// `terminal_handler.rs`; duplicated here so integration tests can call
/// `handler.handle_device_control_string()` directly.
fn build_sixel_dcs_bytes(params: &[u8], sixel_body: &[u8]) -> Vec<u8> {
    let mut v = vec![b'P'];
    v.extend_from_slice(params);
    v.push(b'q');
    v.extend_from_slice(sixel_body);
    v.extend_from_slice(b"\x1b\\");
    v
}

#[test]
fn sixel_shared_palette_persists_color_definition_across_images() {
    // In shared-register mode (?1070 l), a colour defined in one Sixel image
    // should be available under the same index in the next image.
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::private_color_registers::PrivateColorRegisters,
    };

    let mut handler = TerminalHandler::new(80, 24);
    handler.handle_resize(80, 24, 8, 16);
    let (tx, _rx) = crossbeam_channel::unbounded::<freminal_common::pty_write::PtyWrite>();
    handler.set_write_tx(tx);

    // Switch to shared palette mode.
    handler.process_outputs(&[TerminalOutput::Mode(Mode::PrivateColorRegisters(
        PrivateColorRegisters::new(&SetMode::DecRst),
    ))]);

    // Image 1: define colour index 10 as pure red (RGB 100,0,0) and paint one pixel.
    let body1 = b"#10;2;100;0;0#10~";
    let dcs1 = build_sixel_dcs_bytes(b"0;0;0", body1);
    handler.handle_device_control_string(&dcs1);

    // Image 2: select colour index 10 without redefining it, paint one pixel.
    let body2 = b"#10~";
    let dcs2 = build_sixel_dcs_bytes(b"0;0;0", body2);
    handler.handle_device_control_string(&dcs2);

    // Two images should now be in the image store.
    let mut images: Vec<_> = handler.buffer().image_store().iter().collect();
    assert_eq!(images.len(), 2, "expected two images in the store");
    // Sort by image id (monotonically increasing) so images[0] is image 1 and
    // images[1] is image 2, regardless of HashMap iteration order.
    images.sort_by_key(|(id, _)| *id);

    // The second image's top-left pixel should be red because colour 10 was
    // inherited from the first image's palette definition.
    let (_, img2) = images[1];
    assert_eq!(
        img2.pixels[0], 255,
        "R of pixel (0,0) in image 2 should be 255 (red)"
    );
    assert_eq!(img2.pixels[1], 0, "G of pixel (0,0) in image 2 should be 0");
    assert_eq!(img2.pixels[2], 0, "B of pixel (0,0) in image 2 should be 0");
}

#[test]
fn sixel_private_palette_does_not_persist_color_definition() {
    // In private-register mode (?1070 h, default), each image gets a fresh
    // palette.  A colour defined in image 1 must NOT appear in image 2.
    let mut handler = TerminalHandler::new(80, 24);
    handler.handle_resize(80, 24, 8, 16);
    let (tx, _rx) = crossbeam_channel::unbounded::<freminal_common::pty_write::PtyWrite>();
    handler.set_write_tx(tx);

    // Private mode is the default — no need to set it explicitly.

    // Image 1: define colour index 10 as pure red and paint.
    let body1 = b"#10;2;100;0;0#10~";
    let dcs1 = build_sixel_dcs_bytes(b"0;0;0", body1);
    handler.handle_device_control_string(&dcs1);

    // Image 2: select colour index 10 without redefining; should use the
    // default VT340 palette entry for index 10 (light red ≈ (255,85,85)).
    let body2 = b"#10~";
    let dcs2 = build_sixel_dcs_bytes(b"0;0;0", body2);
    handler.handle_device_control_string(&dcs2);

    let mut images: Vec<_> = handler.buffer().image_store().iter().collect();
    assert_eq!(images.len(), 2, "expected two images in the store");
    // Sort by image id (monotonically increasing) so images[0]/[1] are stable.
    images.sort_by_key(|(id, _)| *id);

    // The second image should NOT be pure red — it should be the default
    // VT340 palette index 10 (≈ light red, R=255 G=85 B=85).
    let (_, img2) = images[1];
    // The green channel distinguishes pure red (G=0) from VT340 index 10 (G=85).
    assert_ne!(
        img2.pixels[1], 0,
        "G channel should not be 0 in private mode (pure red should not persist)"
    );
}

// ── DECNRCM (?42) — National Replacement Character Set Mode ───────────

#[test]
fn decrpm_decnrcm_default_is_disabled() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decnrcm::Decnrcm,
    };

    let mut handler = TerminalHandler::new(80, 24);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::Decnrcm(Decnrcm::new(&SetMode::DecQuery))),
    );
    assert_eq!(resp, "\x1b[?42;2$y", "DECNRCM default (NrcDisabled) → Ps=2");
}

#[test]
fn decrpm_decnrcm_after_set() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decnrcm::Decnrcm,
    };

    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decnrcm(Decnrcm::new(
        &SetMode::DecSet,
    )))]);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::Decnrcm(Decnrcm::new(&SetMode::DecQuery))),
    );
    assert_eq!(
        resp, "\x1b[?42;1$y",
        "DECNRCM after DECSET (NrcEnabled) → Ps=1"
    );
}

#[test]
fn decrpm_decnrcm_after_set_then_reset() {
    use freminal_common::buffer_states::{
        mode::{Mode, SetMode},
        modes::decnrcm::Decnrcm,
    };

    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decnrcm(Decnrcm::new(
        &SetMode::DecSet,
    )))]);
    handler.process_outputs(&[TerminalOutput::Mode(Mode::Decnrcm(Decnrcm::new(
        &SetMode::DecRst,
    )))]);
    let resp = query_handler_mode(
        &mut handler,
        TerminalOutput::Mode(Mode::Decnrcm(Decnrcm::new(&SetMode::DecQuery))),
    );
    assert_eq!(
        resp, "\x1b[?42;2$y",
        "DECNRCM after DECRST (NrcDisabled) → Ps=2"
    );
}
