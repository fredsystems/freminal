// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `PointerShape` — typed representation of the X11/CSS pointer cursor shape
//! set by `OSC 22 ; <name> ST`.
//!
//! The variants map 1-to-1 onto `egui::CursorIcon` so the GUI can convert
//! without carrying an egui dependency into the common crate.

use std::fmt;

/// The mouse pointer (cursor) shape requested by the running application via
/// OSC 22.
///
/// An application sends `OSC 22 ; <name> ST` to request a specific X11 /
/// CSS pointer shape.  An empty name or `"default"` resets to the OS default.
///
/// The variants are named after their `egui::CursorIcon` counterparts so that
/// the GUI layer can convert with a simple `match` — no string comparison at
/// render time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointerShape {
    /// Default OS pointer (arrow).  Also the reset / "no override" state.
    #[default]
    Default,
    /// Hide the cursor entirely.
    None,
    /// I-beam for text selection.
    Text,
    /// Vertical I-beam.
    VerticalText,
    /// Pointing-hand (links / clickable items).
    Pointer,
    /// Context-menu cursor.
    ContextMenu,
    /// Help / question-mark cursor.
    Help,
    /// Spinning progress indicator (cursor + busy).
    Progress,
    /// Hourglass / busy wait — no interaction.
    Wait,
    /// Cell / plus cross (spreadsheet cell).
    Cell,
    /// Crosshair / precision cursor.
    Crosshair,
    /// Move (four-direction).
    Move,
    /// No-drop indicator.
    NoDrop,
    /// Not-allowed / forbidden.
    NotAllowed,
    /// Grab (open hand).
    Grab,
    /// Grabbing (closed hand).
    Grabbing,
    /// Alias / shortcut arrow.
    Alias,
    /// Copy arrow.
    Copy,
    /// All-scroll (four arrows).
    AllScroll,
    /// Horizontal resize (east–west).
    ResizeHorizontal,
    /// Vertical resize (north–south).
    ResizeVertical,
    /// North-east / south-west resize diagonal.
    ResizeNeSw,
    /// North-west / south-east resize diagonal.
    ResizeNwSe,
    /// Resize east.
    ResizeEast,
    /// Resize south-east.
    ResizeSouthEast,
    /// Resize south.
    ResizeSouth,
    /// Resize south-west.
    ResizeSouthWest,
    /// Resize west.
    ResizeWest,
    /// Resize north-west.
    ResizeNorthWest,
    /// Resize north.
    ResizeNorth,
    /// Resize north-east.
    ResizeNorthEast,
    /// Zoom in.
    ZoomIn,
    /// Zoom out.
    ZoomOut,
}

impl fmt::Display for PointerShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => write!(f, "default"),
            Self::None => write!(f, "none"),
            Self::Text => write!(f, "text"),
            Self::VerticalText => write!(f, "vertical-text"),
            Self::Pointer => write!(f, "pointer"),
            Self::ContextMenu => write!(f, "context-menu"),
            Self::Help => write!(f, "help"),
            Self::Progress => write!(f, "progress"),
            Self::Wait => write!(f, "wait"),
            Self::Cell => write!(f, "cell"),
            Self::Crosshair => write!(f, "crosshair"),
            Self::Move => write!(f, "move"),
            Self::NoDrop => write!(f, "no-drop"),
            Self::NotAllowed => write!(f, "not-allowed"),
            Self::Grab => write!(f, "grab"),
            Self::Grabbing => write!(f, "grabbing"),
            Self::Alias => write!(f, "alias"),
            Self::Copy => write!(f, "copy"),
            Self::AllScroll => write!(f, "all-scroll"),
            Self::ResizeHorizontal => write!(f, "col-resize"),
            Self::ResizeVertical => write!(f, "row-resize"),
            Self::ResizeNeSw => write!(f, "nesw-resize"),
            Self::ResizeNwSe => write!(f, "nwse-resize"),
            Self::ResizeEast => write!(f, "e-resize"),
            Self::ResizeSouthEast => write!(f, "se-resize"),
            Self::ResizeSouth => write!(f, "s-resize"),
            Self::ResizeSouthWest => write!(f, "sw-resize"),
            Self::ResizeWest => write!(f, "w-resize"),
            Self::ResizeNorthWest => write!(f, "nw-resize"),
            Self::ResizeNorth => write!(f, "n-resize"),
            Self::ResizeNorthEast => write!(f, "ne-resize"),
            Self::ZoomIn => write!(f, "zoom-in"),
            Self::ZoomOut => write!(f, "zoom-out"),
        }
    }
}

impl From<&str> for PointerShape {
    /// Convert an xcursor / CSS cursor name to a `PointerShape`.
    ///
    /// Unknown names and empty strings map to `PointerShape::Default`.
    fn from(name: &str) -> Self {
        match name.trim() {
            // Hide cursor
            "none" => Self::None,
            // Text
            "text" | "xterm" => Self::Text,
            "vertical-text" => Self::VerticalText,
            // Pointer
            "pointer" | "hand" | "hand2" => Self::Pointer,
            // Help
            "help" => Self::Help,
            // Context menu
            "context-menu" => Self::ContextMenu,
            // Progress
            "progress" | "left_ptr_watch" => Self::Progress,
            // Wait
            "wait" | "watch" => Self::Wait,
            // Crosshair
            "crosshair" => Self::Crosshair,
            // Cell
            "cell" => Self::Cell,
            // Move
            "move" | "fleur" => Self::Move,
            // No-drop
            "no-drop" => Self::NoDrop,
            // Not-allowed
            "not-allowed" => Self::NotAllowed,
            // Grab
            "grab" => Self::Grab,
            // Grabbing
            "grabbing" => Self::Grabbing,
            // Alias
            "alias" => Self::Alias,
            // Copy
            "copy" => Self::Copy,
            // All-scroll
            "all-scroll" => Self::AllScroll,
            // Horizontal resize
            "col-resize" | "ew-resize" => Self::ResizeHorizontal,
            // Vertical resize
            "row-resize" | "ns-resize" => Self::ResizeVertical,
            // Diagonal resize NE-SW
            "nesw-resize" => Self::ResizeNeSw,
            // Diagonal resize NW-SE
            "nwse-resize" => Self::ResizeNwSe,
            // Directional resizes
            "e-resize" => Self::ResizeEast,
            "se-resize" => Self::ResizeSouthEast,
            "s-resize" => Self::ResizeSouth,
            "sw-resize" => Self::ResizeSouthWest,
            "w-resize" => Self::ResizeWest,
            "nw-resize" => Self::ResizeNorthWest,
            "n-resize" => Self::ResizeNorth,
            "ne-resize" => Self::ResizeNorthEast,
            // Zoom
            "zoom-in" => Self::ZoomIn,
            "zoom-out" => Self::ZoomOut,
            // Unknown names, empty string, and explicit "default"/"arrow"/"left_ptr" all map here
            _ => Self::Default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PointerShape;

    #[test]
    fn empty_string_maps_to_default() {
        assert_eq!(PointerShape::from(""), PointerShape::Default);
    }

    #[test]
    fn whitespace_only_maps_to_default() {
        assert_eq!(PointerShape::from("   "), PointerShape::Default);
    }

    #[test]
    fn explicit_default_names() {
        assert_eq!(PointerShape::from("default"), PointerShape::Default);
        assert_eq!(PointerShape::from("arrow"), PointerShape::Default);
        assert_eq!(PointerShape::from("left_ptr"), PointerShape::Default);
    }

    #[test]
    fn none_cursor() {
        assert_eq!(PointerShape::from("none"), PointerShape::None);
    }

    #[test]
    fn text_cursor_variants() {
        assert_eq!(PointerShape::from("text"), PointerShape::Text);
        assert_eq!(PointerShape::from("xterm"), PointerShape::Text);
    }

    #[test]
    fn vertical_text() {
        assert_eq!(
            PointerShape::from("vertical-text"),
            PointerShape::VerticalText
        );
    }

    #[test]
    fn pointer_variants() {
        assert_eq!(PointerShape::from("pointer"), PointerShape::Pointer);
        assert_eq!(PointerShape::from("hand"), PointerShape::Pointer);
        assert_eq!(PointerShape::from("hand2"), PointerShape::Pointer);
    }

    #[test]
    fn help_cursor() {
        assert_eq!(PointerShape::from("help"), PointerShape::Help);
    }

    #[test]
    fn context_menu_cursor() {
        assert_eq!(
            PointerShape::from("context-menu"),
            PointerShape::ContextMenu
        );
    }

    #[test]
    fn progress_variants() {
        assert_eq!(PointerShape::from("progress"), PointerShape::Progress);
        assert_eq!(PointerShape::from("left_ptr_watch"), PointerShape::Progress);
    }

    #[test]
    fn wait_variants() {
        assert_eq!(PointerShape::from("wait"), PointerShape::Wait);
        assert_eq!(PointerShape::from("watch"), PointerShape::Wait);
    }

    #[test]
    fn crosshair() {
        assert_eq!(PointerShape::from("crosshair"), PointerShape::Crosshair);
    }

    #[test]
    fn cell_cursor() {
        assert_eq!(PointerShape::from("cell"), PointerShape::Cell);
    }

    #[test]
    fn move_variants() {
        assert_eq!(PointerShape::from("move"), PointerShape::Move);
        assert_eq!(PointerShape::from("fleur"), PointerShape::Move);
    }

    #[test]
    fn no_drop() {
        assert_eq!(PointerShape::from("no-drop"), PointerShape::NoDrop);
    }

    #[test]
    fn not_allowed() {
        assert_eq!(PointerShape::from("not-allowed"), PointerShape::NotAllowed);
    }

    #[test]
    fn grab_and_grabbing() {
        assert_eq!(PointerShape::from("grab"), PointerShape::Grab);
        assert_eq!(PointerShape::from("grabbing"), PointerShape::Grabbing);
    }

    #[test]
    fn alias_and_copy() {
        assert_eq!(PointerShape::from("alias"), PointerShape::Alias);
        assert_eq!(PointerShape::from("copy"), PointerShape::Copy);
    }

    #[test]
    fn all_scroll() {
        assert_eq!(PointerShape::from("all-scroll"), PointerShape::AllScroll);
    }

    #[test]
    fn horizontal_resize_variants() {
        assert_eq!(
            PointerShape::from("col-resize"),
            PointerShape::ResizeHorizontal
        );
        assert_eq!(
            PointerShape::from("ew-resize"),
            PointerShape::ResizeHorizontal
        );
    }

    #[test]
    fn vertical_resize_variants() {
        assert_eq!(
            PointerShape::from("row-resize"),
            PointerShape::ResizeVertical
        );
        assert_eq!(
            PointerShape::from("ns-resize"),
            PointerShape::ResizeVertical
        );
    }

    #[test]
    fn diagonal_resizes() {
        assert_eq!(PointerShape::from("nesw-resize"), PointerShape::ResizeNeSw);
        assert_eq!(PointerShape::from("nwse-resize"), PointerShape::ResizeNwSe);
    }

    #[test]
    fn directional_resizes() {
        assert_eq!(PointerShape::from("e-resize"), PointerShape::ResizeEast);
        assert_eq!(
            PointerShape::from("se-resize"),
            PointerShape::ResizeSouthEast
        );
        assert_eq!(PointerShape::from("s-resize"), PointerShape::ResizeSouth);
        assert_eq!(
            PointerShape::from("sw-resize"),
            PointerShape::ResizeSouthWest
        );
        assert_eq!(PointerShape::from("w-resize"), PointerShape::ResizeWest);
        assert_eq!(
            PointerShape::from("nw-resize"),
            PointerShape::ResizeNorthWest
        );
        assert_eq!(PointerShape::from("n-resize"), PointerShape::ResizeNorth);
        assert_eq!(
            PointerShape::from("ne-resize"),
            PointerShape::ResizeNorthEast
        );
    }

    #[test]
    fn zoom_in_and_out() {
        assert_eq!(PointerShape::from("zoom-in"), PointerShape::ZoomIn);
        assert_eq!(PointerShape::from("zoom-out"), PointerShape::ZoomOut);
    }

    #[test]
    fn unknown_name_maps_to_default() {
        assert_eq!(PointerShape::from("banana"), PointerShape::Default);
        assert_eq!(PointerShape::from("not-a-cursor"), PointerShape::Default);
    }

    #[test]
    fn default_display_roundtrip() {
        assert_eq!(PointerShape::Default.to_string(), "default");
    }

    #[test]
    fn text_display_roundtrip() {
        assert_eq!(PointerShape::Text.to_string(), "text");
    }

    #[test]
    fn pointer_display_roundtrip() {
        assert_eq!(PointerShape::Pointer.to_string(), "pointer");
    }

    #[test]
    fn default_trait_is_default_variant() {
        assert_eq!(PointerShape::default(), PointerShape::Default);
    }
}
