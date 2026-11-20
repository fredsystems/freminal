// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Terminal cell state types shared across the workspace.
//!
//! This module re-exports submodules for every category of terminal state:
//! character format, cursor, colors, SGR, DEC modes, OSC handlers, and output
//! command enums.

/// Which of the two terminal buffers is currently active.
pub mod buffer_type;
pub mod charset;
/// `CommandBlock` — a single shell command's full lifecycle from OSC 133 markers.
pub mod command_block;
/// Cursor state types: position, colors, decorations, and reverse-video.
pub mod cursor;
/// Error types for terminal cell and character operations.
pub mod error;
/// Font weight, decoration, and blink-state types.
pub mod fonts;
/// `FormatTag` — a half-open `[start, end)` range with its associated format.
pub mod format_tag;
pub mod ftcs;
/// Kitty graphics protocol types and helpers.
pub mod kitty_graphics;
/// Soft-wrap line join metadata.
pub mod line_wrap;
/// `Mode` and `SetMode` — generic set/reset mode command types.
pub mod mode;
/// Typed DEC private mode enums (one module per mode number).
pub mod modes;
/// OSC parameter types and inline-image data.
pub mod osc;
pub mod osc99_notification;
/// OSC 4 / 104 / 10-19 / 110-119 colour-control request types.
pub mod osc_color;
pub mod osc_notify_99;
pub mod pointer_shape;
pub mod progress;
pub mod row_number;
/// Sixel graphics types.
pub mod sixel;
/// `TChar` — a single terminal character with optional wide-character metadata.
pub mod tchar;
/// `TerminalOutput` — the parsed command enum produced by the ANSI parser.
pub mod terminal_output;
/// `TerminalSections` — a scrollback/visible pair of slices.
pub mod terminal_sections;
/// Unicode virtual placement helpers for the Kitty graphics protocol.
pub mod unicode_placeholder;
/// `Url` — an OSC 8 hyperlink URL with optional ID.
pub mod url;
/// `WindowManipulation` — window-level commands (resize, title, report queries).
pub mod window_manipulation;
/// XTSMGRAPHICS (`CSI ? Pi ; Pa ; Pv S`) item and action types.
pub mod xtsmgraphics;
