// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Comprehensive edge case tests for DECSTBM (scroll region) functionality.
//!
//! These tests verify boundary conditions, invalid inputs, and interactions
//! between scroll regions and other buffer operations.

use freminal_buffer::buffer::Buffer;
use freminal_common::buffer_states::tchar::TChar;

fn ascii(c: char) -> TChar {
    TChar::Ascii(c as u8)
}

fn visible_lengths(buf: &Buffer) -> Vec<usize> {
    buf.visible_rows()
        .iter()
        .map(|row| row.get_characters().len())
        .collect()
}

fn tag_rows(buf: &mut Buffer, height: usize) {
    for i in 0..height {
        let count = (i % 9) + 1; // Unique lengths: 1,2,3,4,5,6,7,8,9,1,2...
        let chars: Vec<TChar> = (0..count).map(|_| ascii('X')).collect();
        buf.insert_text(&chars);
        if i + 1 < height {
            buf.handle_lf();
            buf.handle_cr();
        }
    }
}

// ============================================================================
// Invalid / Boundary Parameter Tests
// ============================================================================

#[test]
fn scroll_region_zero_top_resets_to_full() {
    let mut buf = Buffer::new(10, 5);
    tag_rows(&mut buf, 5);

    // Set valid region first
    buf.set_scroll_region(2, 4);
    assert_eq!(buf.get_cursor().pos.y, 1); // Moved to region top

    // Set with top=0 should reset to full screen
    buf.set_scroll_region(0, 3);

    // Cursor should be at row 0 (full screen region top)
    assert_eq!(buf.get_cursor().pos.y, 0);
}

#[test]
fn scroll_region_zero_bottom_resets_to_full() {
    let mut buf = Buffer::new(10, 5);
    tag_rows(&mut buf, 5);

    buf.set_scroll_region(2, 4);
    assert_eq!(buf.get_cursor().pos.y, 1);

    buf.set_scroll_region(2, 0);
    assert_eq!(buf.get_cursor().pos.y, 0);
}

#[test]
fn scroll_region_inverted_bounds_resets_to_full() {
    let mut buf = Buffer::new(10, 5);
    tag_rows(&mut buf, 5);

    // Bottom before top
    buf.set_scroll_region(4, 2);

    // Should reset to full screen and move cursor to top
    assert_eq!(buf.get_cursor().pos.y, 0);
}

#[test]
fn scroll_region_bottom_beyond_screen_resets_to_full() {
    let mut buf = Buffer::new(10, 5);
    tag_rows(&mut buf, 5);

    // Bottom row 10 doesn't exist (screen is 5 high)
    buf.set_scroll_region(2, 10);

    // Should reset to full screen
    assert_eq!(buf.get_cursor().pos.y, 0);
}

#[test]
fn scroll_region_single_row_not_supported() {
    // Per the current implementation, top >= bottom resets to full screen.
    // This tests current behavior. If single-row regions should be supported,
    // this test documents the change needed.
    let mut buf = Buffer::new(10, 5);
    tag_rows(&mut buf, 5);

    // Request single row region: row 3 only (1-based)
    buf.set_scroll_region(3, 3);

    // Current behavior: resets to full screen because top == bottom after conversion
    // (top=2, bottom=2, and 2 >= 2 is true)
    assert_eq!(buf.get_cursor().pos.y, 0);

    // If single-row regions are needed, change condition in set_scroll_region
    // from `top >= bottom` to `top > bottom`
}

#[test]
fn scroll_region_full_screen_explicit() {
    let mut buf = Buffer::new(10, 5);
    tag_rows(&mut buf, 5);

    // Explicitly request full screen region
    buf.set_scroll_region(1, 5);

    // Should set region to full screen
    assert_eq!(buf.get_cursor().pos.y, 0);
    assert_eq!(buf.get_cursor().pos.x, 0);

    // Verify scrolling works as full-screen mode
    for _ in 0..10 {
        buf.handle_lf();
    }

    // Should have accumulated scrollback
    assert!(buf.visible_rows().len() == 5);
}

// ============================================================================
// Cursor Positioning Tests
// ============================================================================

#[test]
fn scroll_region_moves_cursor_to_region_top() {
    let mut buf = Buffer::new(10, 10);
    tag_rows(&mut buf, 10);

    // Cursor starts at last row
    assert_eq!(buf.get_cursor().pos.y, 9);

    buf.set_scroll_region(5, 8);

    // Should move to row 4 (0-based index of row 5)
    assert_eq!(buf.get_cursor().pos.y, 4);
    assert_eq!(buf.get_cursor().pos.x, 0);
}

#[test]
fn scroll_region_resets_cursor_x() {
    let mut buf = Buffer::new(10, 5);

    // Position cursor at middle of row
    buf.insert_text(&[ascii('A'), ascii('B'), ascii('C')]);
    assert_eq!(buf.get_cursor().pos.x, 3);

    buf.set_scroll_region(2, 4);

    // X should be reset to 0
    assert_eq!(buf.get_cursor().pos.x, 0);
}

// ============================================================================
// Operations Outside Scroll Region
// ============================================================================

#[test]
fn lf_outside_region_below_in_primary_creates_scrollback() {
    let mut buf = Buffer::new(10, 10);
    tag_rows(&mut buf, 10);

    buf.set_scroll_region(2, 5); // Region: rows 1..4 (0-based)

    // Move cursor outside region (to row 8, 0-based)
    buf.set_cursor_pos(Some(1), Some(9)); // 1-based: col 1, row 9

    let rows_before = buf.get_rows().len();

    // LF outside region in primary buffer should behave normally:
    // at bottom of screen, it adds a new row and scrollback
    buf.handle_lf();

    let rows_after = buf.get_rows().len();

    // Should have added a row (scrollback behavior)
    assert!(rows_after > rows_before);
}

#[test]
fn lf_outside_region_above_moves_cursor() {
    let mut buf = Buffer::new(10, 10);
    tag_rows(&mut buf, 10);

    buf.set_scroll_region(5, 8); // Region: rows 4..7 (0-based)

    // Move cursor to row 0 (above region)
    buf.set_cursor_pos(Some(1), Some(1));

    let cursor_before = buf.get_cursor().pos.y;

    buf.handle_lf();

    let cursor_after = buf.get_cursor().pos.y;

    // Cursor should have moved down
    assert_eq!(cursor_after, cursor_before + 1);
}

#[test]
fn ri_outside_region_moves_cursor_up() {
    let mut buf = Buffer::new(10, 10);
    tag_rows(&mut buf, 10);

    buf.set_scroll_region(5, 8);

    // Move cursor below region to row 9 (0-based)
    buf.set_cursor_pos(Some(1), Some(10));

    let cursor_before = buf.get_cursor().pos.y;

    buf.handle_ri();

    let cursor_after = buf.get_cursor().pos.y;

    // Cursor should have moved up
    assert_eq!(cursor_after, cursor_before - 1);
}

// ============================================================================
// Insert/Delete Lines Outside Region
// ============================================================================

#[test]
fn insert_lines_outside_region_is_noop() {
    let mut buf = Buffer::new(10, 10);
    tag_rows(&mut buf, 10);

    buf.set_scroll_region(3, 6);

    // Move cursor outside region
    buf.set_cursor_pos(Some(1), Some(8));

    let before = visible_lengths(&buf);

    buf.insert_lines(2);

    let after = visible_lengths(&buf);

    // No change because cursor is outside region
    assert_eq!(before, after);
}

#[test]
fn delete_lines_outside_region_is_noop() {
    let mut buf = Buffer::new(10, 10);
    tag_rows(&mut buf, 10);

    buf.set_scroll_region(3, 6);

    buf.set_cursor_pos(Some(1), Some(1)); // Above region

    let before = visible_lengths(&buf);

    buf.delete_lines(1);

    let after = visible_lengths(&buf);

    assert_eq!(before, after);
}

// ============================================================================
// Scrollback Interaction (Primary Buffer)
// ============================================================================

#[test]
fn scroll_region_operations_blocked_when_scrolled_back() {
    let mut buf = Buffer::new(10, 5);

    // Generate scrollback by filling screen and scrolling
    for i in 0..10 {
        let chars: Vec<TChar> = (0..(i % 9) + 1).map(|_| ascii('X')).collect();
        buf.insert_text(&chars);
        buf.handle_lf();
        buf.handle_cr();
    }

    // Scroll back to view history
    buf.scroll_back(3);
    // We're now scrolled back (viewing history)

    buf.set_scroll_region(2, 4);

    // Position cursor in region
    buf.set_cursor_pos(Some(1), Some(3));

    let before = visible_lengths(&buf);

    // These operations should be no-ops while scrolled back
    buf.insert_lines(1);
    buf.delete_lines(1);

    let after = visible_lengths(&buf);

    assert_eq!(before, after);
}

#[test]
fn lf_in_scroll_region_resets_scrollback_offset() {
    let mut buf = Buffer::new(10, 5);

    // Generate scrollback
    for i in 0..10 {
        let chars: Vec<TChar> = (0..(i % 9) + 1).map(|_| ascii('X')).collect();
        buf.insert_text(&chars);
        buf.handle_lf();
        buf.handle_cr();
    }

    buf.scroll_back(2);
    // We're now scrolled back

    buf.set_scroll_region(2, 4);

    // LF should reset scroll offset and return to live view
    buf.handle_lf();

    // Verify we're back at live view by checking visible_rows reflects current state
    // (indirect verification that scroll offset was reset)
    let visible = buf.visible_rows();
    assert_eq!(visible.len(), 5);
}

// ============================================================================
// Alternate Buffer
// ============================================================================

#[test]
fn scroll_region_in_alternate_buffer() {
    let mut buf = Buffer::new(10, 5);
    tag_rows(&mut buf, 5);

    buf.enter_alternate();

    // Tag alternate buffer rows
    for i in 0..5 {
        let chars: Vec<TChar> = vec![ascii((b'A' + i as u8) as char)];
        buf.insert_text(&chars);
        if i + 1 < 5 {
            buf.handle_lf();
            buf.handle_cr();
        }
    }

    buf.set_scroll_region(2, 4);

    // Should work in alternate buffer
    assert_eq!(buf.get_cursor().pos.y, 1);

    // Move to bottom and scroll
    buf.handle_lf();
    buf.handle_lf();

    let before_y = buf.get_cursor().pos.y;
    buf.handle_lf(); // Should scroll region up

    // Cursor should stay at bottom of region
    assert_eq!(buf.get_cursor().pos.y, before_y);
}

#[test]
fn scroll_region_state_not_restored_from_alternate() {
    let mut buf = Buffer::new(10, 8);
    tag_rows(&mut buf, 8);

    // Set scroll region in primary
    buf.set_scroll_region(3, 6);
    let primary_cursor_y = buf.get_cursor().pos.y;

    buf.enter_alternate();

    // Set different scroll region in alternate
    buf.set_scroll_region(2, 5);

    buf.leave_alternate();

    // Primary cursor should be restored, but does scroll region get restored?
    // Based on the code, scroll_region is NOT saved/restored
    assert_eq!(buf.get_cursor().pos.y, primary_cursor_y);
}

// ============================================================================
// Resize Interaction
// ============================================================================

#[test]
fn resize_clamps_scroll_region_and_cursor() {
    let mut buf = Buffer::new(10, 10);
    tag_rows(&mut buf, 10);

    // Set region near bottom
    buf.set_scroll_region(7, 10);
    assert_eq!(buf.get_cursor().pos.y, 6); // 0-based row 6

    // Shrink height to 5
    buf.set_size(10, 5);

    // Scroll region would be invalid (rows 6-9 don't exist)
    // Cursor should be clamped
    assert!(buf.get_cursor().pos.y < 5);
}

#[test]
fn scroll_region_persists_through_width_resize() {
    let mut buf = Buffer::new(80, 10);
    tag_rows(&mut buf, 10);

    buf.set_scroll_region(3, 7);
    let region_cursor_y = buf.get_cursor().pos.y;

    // Resize width only
    buf.set_size(100, 10);

    // Cursor position should be preserved (or at least reasonable)
    // This tests that width resize doesn't break scroll region state
    assert_eq!(buf.get_cursor().pos.y, region_cursor_y);
}

// ============================================================================
// Multiple Scroll Operations
// ============================================================================

#[test]
fn multiple_scroll_up_operations() {
    let mut buf = Buffer::new(10, 8);
    tag_rows(&mut buf, 8);

    buf.set_scroll_region(3, 6); // Rows 2,3,4,5 (0-based)

    // Move to bottom of region
    buf.handle_lf();
    buf.handle_lf();
    buf.handle_lf();

    // Multiple scrolls
    for _ in 0..5 {
        buf.handle_lf();
    }

    // Should still be at bottom of region
    assert_eq!(buf.get_cursor().pos.y, 5);

    // Verify buffer didn't crash and visible rows are sane
    assert_eq!(buf.visible_rows().len(), 8);
}

#[test]
fn multiple_scroll_down_operations() {
    let mut buf = Buffer::new(10, 8);
    tag_rows(&mut buf, 8);

    buf.set_scroll_region(3, 6);

    // Cursor at top of region (y=2)
    assert_eq!(buf.get_cursor().pos.y, 2);

    // Multiple RI at top should scroll region down multiple times
    for _ in 0..5 {
        buf.handle_ri();
    }

    // Should still be at top of region
    assert_eq!(buf.get_cursor().pos.y, 2);

    assert_eq!(buf.visible_rows().len(), 8);
}

// ============================================================================
// Insert/Delete with Large Counts
// ============================================================================

#[test]
fn insert_lines_count_larger_than_region() {
    let mut buf = Buffer::new(10, 10);
    tag_rows(&mut buf, 10);

    buf.set_scroll_region(4, 7); // 4 rows

    // Insert 100 lines (way more than region size)
    buf.insert_lines(100);

    // Should clamp to region size and not crash
    // All rows in region should now be blank
    let lengths = visible_lengths(&buf);

    // Rows 3,4,5,6 (0-based) should all be blank
    assert_eq!(lengths[3], 0);
    assert_eq!(lengths[4], 0);
    assert_eq!(lengths[5], 0);
    assert_eq!(lengths[6], 0);

    // Rows outside region should be unchanged
    assert!(lengths[0] > 0);
    assert!(lengths[7] > 0);
}

#[test]
fn delete_lines_count_larger_than_region() {
    let mut buf = Buffer::new(10, 10);
    tag_rows(&mut buf, 10);

    buf.set_scroll_region(4, 7);

    buf.delete_lines(100);

    // Should clamp and not crash
    let lengths = visible_lengths(&buf);

    // All region rows should be blank after massive delete
    assert_eq!(lengths[3], 0);
    assert_eq!(lengths[4], 0);
    assert_eq!(lengths[5], 0);
    assert_eq!(lengths[6], 0);

    // Outside region unchanged
    assert!(lengths[0] > 0);
    assert!(lengths[7] > 0);
}

// ============================================================================
// Reset Scroll Region
// ============================================================================

#[test]
fn reset_scroll_region_to_full_screen() {
    let mut buf = Buffer::new(10, 5);
    tag_rows(&mut buf, 5);

    buf.set_scroll_region(2, 4);
    assert_eq!(buf.get_cursor().pos.y, 1);

    // Reset using DECSTBM with bounds 0,0
    buf.set_scroll_region(0, 0);

    // Should be back to full screen
    assert_eq!(buf.get_cursor().pos.y, 0);

    // Verify LF behaves as full-screen again (accumulates scrollback)
    let initial_rows = buf.get_rows().len();

    for _ in 0..10 {
        buf.handle_lf();
    }

    // Should have added rows (scrollback)
    assert!(buf.get_rows().len() > initial_rows);
}

// ============================================================================
// Edge Cases with Wrapping
// ============================================================================

#[test]
fn text_wrapping_ignores_scroll_region() {
    let mut buf = Buffer::new(5, 10);

    buf.set_scroll_region(3, 5);

    // Insert text longer than width while cursor is in region
    let long_text: Vec<TChar> = (0..20).map(|_| ascii('X')).collect();
    buf.insert_text(&long_text);

    // Text should wrap normally, creating soft-wrap rows
    // Scroll region shouldn't affect wrapping during insert_text
    assert!(buf.get_cursor().pos.y > 2);
}

#[test]
fn wide_character_at_scroll_boundary() {
    let mut buf = Buffer::new(10, 5);

    buf.set_scroll_region(2, 4);

    // Insert wide character
    let wide = TChar::Utf8("🙂".as_bytes().to_vec());
    buf.insert_text(&[wide.clone()]);

    // Should handle wide character correctly even with scroll region active
    assert_eq!(buf.get_cursor().pos.x, 2); // Wide char takes 2 columns
}
