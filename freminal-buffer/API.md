# freminal-buffer API Documentation

A high-performance terminal buffer implementation for the Freminal terminal emulator.

## Overview

`freminal-buffer` provides a clean, well-tested buffer system that handles terminal state, text rendering, scrollback, and all standard VT100/ANSI escape sequence operations.

## Quick Start

```rust
use freminal_buffer::terminal_handler::TerminalHandler;
use freminal_common::buffer_states::terminal_output::TerminalOutput;

// Create a terminal buffer (80 columns x 24 rows)
let mut handler = TerminalHandler::new(80, 24);

// Process terminal output commands
let outputs = vec![
    TerminalOutput::<(), (), (), ()>::Data(b"Hello, World!".to_vec()),
    TerminalOutput::<(), (), (), ()>::Newline,
    TerminalOutput::<(), (), (), ()>::CarriageReturn,
];

handler.process_outputs(&outputs);

// Access the buffer state
let cursor = handler.buffer().get_cursor();
let visible_rows = handler.buffer().visible_rows();
```

## Architecture

### Three-Layer Design

1. **Buffer** (`buffer.rs`) - Core state management
   - Maintains rows, cursor position, scrollback
   - Handles primary/alternate screen buffers
   - Manages scroll regions (DECSTBM)
   - Implements reflow on resize

2. **TerminalHandler** (`terminal_handler.rs`) - Command dispatcher
   - Processes `TerminalOutput` commands
   - Routes to appropriate buffer operations
   - Handles coordinate conversions (1-based → 0-based)

3. **TerminalOutput** (`freminal-common`) - Command definitions
   - Enum of all terminal operations
   - Shared between parser and buffer
   - Generic design for parser-specific types

## Main API: TerminalHandler

### Creating a Handler

```rust
use freminal_buffer::terminal_handler::TerminalHandler;

// Standard terminal size
let mut handler = TerminalHandler::new(80, 24);

// Wide terminal
let mut handler = TerminalHandler::new(120, 40);
```

### Processing Commands

#### Batch Processing (Recommended)

```rust
use freminal_common::buffer_states::terminal_output::TerminalOutput;

let outputs = vec![
    TerminalOutput::<(), (), (), ()>::ClearDisplay,
    TerminalOutput::<(), (), (), ()>::SetCursorPos {
        x: Some(1),
        y: Some(1)
    },
    TerminalOutput::<(), (), (), ()>::Data(b"Welcome!".to_vec()),
];

handler.process_outputs(&outputs);
```

#### Individual Operations

```rust
// Insert text
handler.handle_data(b"Hello");

// Cursor movement
handler.handle_cursor_pos(Some(10), Some(5)); // Move to (10, 5)
handler.handle_cursor_forward(3);              // Move right 3
handler.handle_cursor_up(2);                   // Move up 2

// Erase operations
handler.handle_erase_in_display(2);  // Clear entire screen
handler.handle_erase_in_line(0);     // Clear from cursor to EOL

// Line operations
handler.handle_insert_lines(2);      // Insert 2 blank lines
handler.handle_delete_lines(1);      // Delete 1 line

// Newline/carriage return
handler.handle_newline();
handler.handle_carriage_return();
```

### Accessing Buffer State

```rust
// Get cursor position
let cursor = handler.buffer().get_cursor();
println!("Cursor at ({}, {})", cursor.pos.x, cursor.pos.y);

// Get visible rows (what should be displayed)
let visible = handler.buffer().visible_rows();
for (i, row) in visible.iter().enumerate() {
    println!("Row {}: {} characters", i, row.get_characters().len());
}

// Get all rows (including scrollback)
let all_rows = handler.buffer().get_rows();
```

## Buffer Operations

### Text Insertion

```rust
use freminal_common::buffer_states::tchar::TChar;

let text = vec![
    TChar::Ascii(b'H'),
    TChar::Ascii(b'i'),
];

handler.buffer_mut().insert_text(&text);
```

Text automatically wraps at the terminal width, creating soft-wrapped rows.

### Cursor Movement

```rust
// Absolute positioning (0-indexed internally)
handler.buffer_mut().set_cursor_pos(Some(0), Some(0)); // Top-left

// Relative movement
handler.buffer_mut().move_cursor_relative(5, -2); // Right 5, up 2
```

### Erase Operations

```rust
// Erase from cursor to end of display (ED 0)
handler.buffer_mut().erase_to_end_of_display();

// Erase entire display (ED 2)
handler.buffer_mut().erase_display();

// Erase from cursor to end of line (EL 0)
handler.buffer_mut().erase_line_to_end();

// Erase entire line (EL 2)
handler.buffer_mut().erase_line();

// Clear scrollback (ED 3)
handler.buffer_mut().erase_scrollback();
```

### Line Operations

```rust
// Insert 3 blank lines at cursor position
handler.buffer_mut().insert_lines(3);

// Delete 2 lines at cursor position
handler.buffer_mut().delete_lines(2);

// Insert 5 spaces at cursor position (ICH)
handler.buffer_mut().insert_spaces(5);
```

### Scroll Regions (DECSTBM)

```rust
// Set scroll region from line 5 to line 20 (1-indexed)
handler.buffer_mut().set_scroll_region(5, 20);

// Operations within region will scroll only that region
handler.buffer_mut().handle_lf(); // Scrolls region, not whole screen
```

### Alternate Screen Buffer

```rust
// Enter alternate screen (like vim, less, etc.)
handler.buffer_mut().enter_alternate();

// Do work in alternate screen...
handler.handle_data(b"Alternate screen content");

// Return to primary screen (restores previous state)
handler.buffer_mut().leave_alternate();
```

### Scrollback Navigation (User Scrolling)

```rust
// Scroll back 10 lines
handler.buffer_mut().scroll_back(10);

// Scroll forward 5 lines
handler.buffer_mut().scroll_forward(5);

// Jump to bottom (live view)
handler.buffer_mut().scroll_to_bottom();
```

### Resize Handling

```rust
// Resize to 120x30
handler.buffer_mut().set_size(120, 30);

// The buffer automatically:
// - Reflows wrapped lines to new width
// - Adjusts scrollback
// - Clamps cursor position
// - Updates scroll regions
```

## Working with Rows

### Row Structure

```rust
use freminal_buffer::row::{Row, RowOrigin, RowJoin};

// Rows track their origin
pub enum RowOrigin {
    HardBreak,    // Ended with CR/LF
    SoftWrap,     // Line wrapped due to width
    ScrollFill,   // Empty row from scrolling
}

// And their relationship to the logical line
pub enum RowJoin {
    NewLogicalLine,      // Start of a logical line
    ContinueLogicalLine, // Continuation of previous row
}
```

### Accessing Row Data

```rust
let visible = handler.buffer().visible_rows();

for row in visible {
    // Get cells
    let cells = row.get_characters();

    // Get specific character
    if let Some(cell) = row.get_char_at(5) {
        let text = cell.into_utf8();
        let tag = cell.tag(); // Format tag (colors, styles)
    }

    // Check row properties
    let width = row.max_width();
    let used = row.get_row_width();

    match row.origin {
        RowOrigin::HardBreak => println!("Hard line break"),
        RowOrigin::SoftWrap => println!("Soft-wrapped"),
        RowOrigin::ScrollFill => println!("Empty from scroll"),
    }
}
```

## Working with Cells

### Cell Structure

Cells represent individual character positions:

```rust
use freminal_buffer::cell::Cell;
use freminal_common::buffer_states::{tchar::TChar, format_tag::FormatTag};

// Create a cell
let cell = Cell::new(TChar::Ascii(b'A'), FormatTag::default());

// Wide character (emoji, CJK)
let wide = Cell::new(
    TChar::Utf8("😀".as_bytes().to_vec()),
    FormatTag::default()
);

// Check properties
cell.is_head();         // Is this the head of a wide char?
cell.is_continuation(); // Is this a continuation cell?
cell.display_width();   // How many columns (1 or 2)
cell.into_utf8();       // Convert to String
```

### Wide Character Handling

Wide characters (display width = 2) automatically create:

1. **Head cell** - Contains the character and format
2. **Continuation cell** - Placeholder for the second column

The buffer manages this automatically during text insertion.

## Format Tags

```rust
use freminal_common::buffer_states::format_tag::FormatTag;

// Create format tag (currently placeholder for future SGR support)
let tag = FormatTag::default();

// Set current format for text insertion
handler.buffer_mut().set_format(tag);

// Get current format
let current = handler.buffer().get_format();
```

**Note:** Full SGR (colors, bold, italic, etc.) support is TODO.

## Implemented Operations

### ✅ Fully Implemented (19 operations)

- **Text Operations:**
  - `Data` - Insert text
  - `Newline` - Line feed (LF)
  - `CarriageReturn` - Move to column 0 (CR)
  - `Backspace` - Move cursor left

- **Cursor Movement:**
  - `SetCursorPos` - Absolute positioning (CUP, HVP)
  - `SetCursorPosRel` - Relative movement

- **Erase Operations:**
  - `ClearDisplayfromCursortoEndofDisplay` (ED 0)
  - `ClearDisplayfromStartofDisplaytoCursor` (ED 1)
  - `ClearDisplay` (ED 2)
  - `ClearScrollbackandDisplay` (ED 3)
  - `ClearLineForwards` (EL 0)
  - `ClearLineBackwards` (EL 1)
  - `ClearLine` (EL 2)

- **Line Operations:**
  - `InsertLines` (IL)
  - `Delete` (DL - delete lines)
  - `InsertSpaces` (ICH)

- **Scroll Regions:**
  - `SetTopAndBottomMargins` (DECSTBM)

- **Special:**
  - `Invalid` - Silently ignored
  - `Skipped` - Silently ignored

### ⚠️ TODO Operations (77 remaining)

All marked with explicit `todo!()` macros:

- SGR (colors, bold, italic) - needs FormatTag conversion
- Mode switching (DECAWM, etc.)
- Charset selection
- Save/Restore cursor
- Device attributes
- OSC sequences
- Special graphics
- And more...

See `PROGRESS.md` for complete list.

## Testing

### Unit Tests

```rust
#[test]
fn my_terminal_test() {
    let mut handler = TerminalHandler::new(80, 24);

    handler.handle_data(b"Test");
    assert_eq!(handler.buffer().get_cursor().pos.x, 4);
}
```

### Integration Tests

```rust
use freminal_common::buffer_states::terminal_output::TerminalOutput;

#[test]
fn realistic_session() {
    let mut handler = TerminalHandler::new(80, 24);

    let outputs = vec![
        TerminalOutput::<(), (), (), ()>::ClearDisplay,
        TerminalOutput::<(), (), (), ()>::SetCursorPos {
            x: Some(1),
            y: Some(1)
        },
        TerminalOutput::<(), (), (), ()>::Data(b"$ ls".to_vec()),
        TerminalOutput::<(), (), (), ()>::Newline,
        TerminalOutput::<(), (), (), ()>::CarriageReturn,
        TerminalOutput::<(), (), (), ()>::Data(b"file.txt".to_vec()),
    ];

    handler.process_outputs(&outputs);

    // Verify final state
    assert_eq!(handler.buffer().get_cursor().pos.y, 1);
}
```

## Performance Characteristics

- **Text Insertion:** O(n) where n = text length
- **Cursor Movement:** O(1)
- **Erase Operations:** O(h) where h = height affected
- **Scrollback:** Limited by `scrollback_limit` (default: 4000 lines)
- **Resize/Reflow:** O(rows × width) but only on resize

## Key Features

### ✅ Implemented

- Primary and alternate screen buffers
- Scrollback with configurable limit
- Scroll regions (DECSTBM)
- Wide character support (emoji, CJK)
- Soft wrapping with line origin tracking
- Automatic reflow on resize
- Sparse row storage (memory efficient)
- Comprehensive test suite

### 🚧 In Progress

- SGR (colors and text attributes)
- Tab stops
- Save/restore cursor
- Full mode support

### 📋 Planned

- Optimized rendering hints
- Selection support
- Hyperlink support
- Sixel graphics

## Common Patterns

### Clear Screen and Write Header

```rust
handler.handle_erase_in_display(2);
handler.handle_cursor_pos(Some(1), Some(1));
handler.handle_data(b"=== My App ===");
```

### Create a Scrolling Region

```rust
// Leave top 2 and bottom 1 lines fixed
handler.buffer_mut().set_scroll_region(3, 23);
// Now only lines 3-23 will scroll
```

### Handle Terminal Resize

```rust
// Called when window size changes
handler.handle_resize(new_width, new_height);
// Content automatically reflows
```

### Implement Progress Bar

```rust
handler.handle_cursor_pos(Some(1), Some(20));
handler.handle_data(b"Progress: [");
for i in 0..50 {
    handler.handle_data(if i < progress { b"#" } else { b" " });
}
handler.handle_data(b"]");
```

## Error Handling

The current implementation uses panic-free error handling:

- Invalid operations are silently ignored or clamped
- Cursor positions are clamped to valid ranges
- Out-of-bounds access returns default values
- No `unwrap()` or `expect()` calls

## Thread Safety

`TerminalHandler` and `Buffer` are **not** thread-safe by design:

- Designed for single-threaded terminal processing
- Wrap in `Mutex` if needed across threads
- Consider message passing instead of shared state

## Dependencies

- `freminal-common` - Shared types (TChar, FormatTag, etc.)
- `tracing` - Logging (optional, feature-gated)

## License

MIT License - See LICENSE file for details

## Contributing

See `PROGRESS.md` for list of TODO operations that need implementation.

Priority areas:

1. SGR (colors/styles) implementation
2. Save/restore cursor
3. Tab stops and tab handling
4. Mode switching support

## Further Reading

- `PROGRESS.md` - Detailed implementation status
- `tests/` - Comprehensive test suite examples
- VT100 documentation - For terminal behavior reference
