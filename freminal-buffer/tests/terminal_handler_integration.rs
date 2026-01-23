// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_buffer::terminal_handler::TerminalHandler;

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

    let visible = handler.buffer().visible_rows();
    assert_eq!(visible.len(), 24);
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

    let visible = handler.buffer().visible_rows();
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

    let visible = handler.buffer().visible_rows();
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

    // User scrolls back
    handler.handle_scroll_back(10);

    // User scrolls forward
    handler.handle_scroll_forward(5);

    // Return to bottom
    handler.handle_scroll_to_bottom();
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
        TerminalOutput::<(), (), (), ()>::ClearDisplay,
        TerminalOutput::<(), (), (), ()>::SetCursorPos {
            x: Some(1),
            y: Some(1),
        },
        // Write prompt
        TerminalOutput::<(), (), (), ()>::Data(b"$ ".to_vec()),
        // User types command
        TerminalOutput::<(), (), (), ()>::Data(b"ls -la".to_vec()),
        // Enter pressed
        TerminalOutput::<(), (), (), ()>::Newline,
        TerminalOutput::<(), (), (), ()>::CarriageReturn,
        // Output some file listings
        TerminalOutput::<(), (), (), ()>::Data(b"total 48".to_vec()),
        TerminalOutput::<(), (), (), ()>::Newline,
        TerminalOutput::<(), (), (), ()>::CarriageReturn,
        TerminalOutput::<(), (), (), ()>::Data(b"-rw-r--r-- 1 user user 1234 file.txt".to_vec()),
        TerminalOutput::<(), (), (), ()>::Newline,
        TerminalOutput::<(), (), (), ()>::CarriageReturn,
        // New prompt
        TerminalOutput::<(), (), (), ()>::Data(b"$ ".to_vec()),
    ];

    handler.process_outputs(&outputs);

    // Verify final cursor position
    assert_eq!(handler.buffer().get_cursor().pos.x, 2);
    assert_eq!(handler.buffer().get_cursor().pos.y, 3);

    // Verify we have content
    let visible = handler.buffer().visible_rows();
    assert_eq!(visible.len(), 24);
}

#[test]
fn test_process_outputs_with_cursor_positioning() {
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(80, 24);

    let outputs = vec![
        // Write header at top
        TerminalOutput::<(), (), (), ()>::SetCursorPos {
            x: Some(1),
            y: Some(1),
        },
        TerminalOutput::<(), (), (), ()>::Data(b"=== Terminal Test ===".to_vec()),
        // Write content in middle
        TerminalOutput::<(), (), (), ()>::SetCursorPos {
            x: Some(1),
            y: Some(10),
        },
        TerminalOutput::<(), (), (), ()>::Data(b"Middle content".to_vec()),
        // Write footer at bottom
        TerminalOutput::<(), (), (), ()>::SetCursorPos {
            x: Some(1),
            y: Some(24),
        },
        TerminalOutput::<(), (), (), ()>::Data(b"Footer".to_vec()),
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
        TerminalOutput::<(), (), (), ()>::SetTopAndBottomMargins {
            top_margin: 5,
            bottom_margin: 20,
        },
        // Write some lines
        TerminalOutput::<(), (), (), ()>::Data(b"Line 1".to_vec()),
        TerminalOutput::<(), (), (), ()>::Newline,
        TerminalOutput::<(), (), (), ()>::CarriageReturn,
        TerminalOutput::<(), (), (), ()>::Data(b"Line 2".to_vec()),
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
        TerminalOutput::<(), (), (), ()>::Data(b"Line 1".to_vec()),
        TerminalOutput::<(), (), (), ()>::Newline,
        TerminalOutput::<(), (), (), ()>::CarriageReturn,
        TerminalOutput::<(), (), (), ()>::Data(b"Line 2".to_vec()),
        TerminalOutput::<(), (), (), ()>::Newline,
        TerminalOutput::<(), (), (), ()>::CarriageReturn,
        TerminalOutput::<(), (), (), ()>::Data(b"Line 3".to_vec()),
        // Move to middle of screen
        TerminalOutput::<(), (), (), ()>::SetCursorPos {
            x: Some(1),
            y: Some(2),
        },
        // Erase from cursor to end of display
        TerminalOutput::<(), (), (), ()>::ClearDisplayfromCursortoEndofDisplay,
        // Write new content
        TerminalOutput::<(), (), (), ()>::Data(b"New Line 2".to_vec()),
    ];

    handler.process_outputs(&outputs);

    assert_eq!(handler.buffer().get_cursor().pos.x, 10);
}

#[test]
fn test_process_outputs_insert_delete_operations() {
    use freminal_common::buffer_states::terminal_output::TerminalOutput;

    let mut handler = TerminalHandler::new(80, 24);

    let outputs = vec![
        TerminalOutput::<(), (), (), ()>::Data(b"Line 1".to_vec()),
        TerminalOutput::<(), (), (), ()>::Newline,
        TerminalOutput::<(), (), (), ()>::CarriageReturn,
        TerminalOutput::<(), (), (), ()>::Data(b"Line 2".to_vec()),
        TerminalOutput::<(), (), (), ()>::Newline,
        TerminalOutput::<(), (), (), ()>::CarriageReturn,
        TerminalOutput::<(), (), (), ()>::Data(b"Line 3".to_vec()),
        // Go back to line 2
        TerminalOutput::<(), (), (), ()>::SetCursorPos {
            x: Some(1),
            y: Some(2),
        },
        // Insert a blank line
        TerminalOutput::<(), (), (), ()>::InsertLines(1),
        // Write on the new line
        TerminalOutput::<(), (), (), ()>::Data(b"Inserted".to_vec()),
    ];

    handler.process_outputs(&outputs);

    assert_eq!(handler.buffer().get_cursor().pos.x, 8);
}
