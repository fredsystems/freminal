// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `PointerShape` — typed representation of the X11/CSS pointer cursor shape
//! set by `OSC 22 ; <name> ST`.
//!
//! The variants map 1-to-1 onto `egui::CursorIcon` so the GUI can convert
//! without carrying an egui dependency into the common crate.
//!
//! [`PointerShape::from_name`] is the one name table: the CSS cursor names from
//! the kitty pointer-shapes specification, freminal's extras, and kitty's
//! legacy X11 aliases.  [`PointerShape::css_name`] gives the canonical name of
//! a shape, which is also what `Display` prints.

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

impl PointerShape {
    /// Look up a pointer shape by its OSC 22 name.
    ///
    /// This is the single name table.  It accepts the 30 CSS cursor names from
    /// the kitty pointer-shapes specification, freminal's extras (`none`,
    /// `context-menu`, `all-scroll`), the CSS names `col-resize` / `row-resize`
    /// (aliases of `ew-resize` / `ns-resize`), and kitty's legacy X11 aliases.
    ///
    /// Matching is case-sensitive and the name is not trimmed.  Anything not in
    /// the table, including the empty string, returns `None`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let shape = match name {
            "default" | "left_ptr" | "arrow" => Self::Default,
            "none" => Self::None,
            "text" | "xterm" | "ibeam" | "beam" => Self::Text,
            "vertical-text" => Self::VerticalText,
            "pointer" | "pointing_hand" | "hand2" | "hand" => Self::Pointer,
            "context-menu" => Self::ContextMenu,
            "help" | "question_arrow" | "whats_this" => Self::Help,
            "progress" | "half-busy" | "left_ptr_watch" => Self::Progress,
            "wait" | "clock" | "watch" => Self::Wait,
            "cell" | "plus" | "cross" => Self::Cell,
            "crosshair" | "tcross" => Self::Crosshair,
            "move" | "fleur" | "pointer-move" => Self::Move,
            "no-drop" | "dnd-no-drop" => Self::NoDrop,
            "not-allowed" | "forbidden" | "crossed_circle" => Self::NotAllowed,
            "grab" | "openhand" | "hand1" => Self::Grab,
            "grabbing" | "closedhand" | "dnd-none" => Self::Grabbing,
            "alias" | "dnd-link" => Self::Alias,
            "copy" | "dnd-copy" => Self::Copy,
            "all-scroll" => Self::AllScroll,
            "ew-resize" | "col-resize" | "sb_h_double_arrow" | "split_h" => Self::ResizeHorizontal,
            "ns-resize" | "row-resize" | "sb_v_double_arrow" | "split_v" => Self::ResizeVertical,
            "nesw-resize" | "size_bdiag" | "size-bdiag" => Self::ResizeNeSw,
            "nwse-resize" | "size_fdiag" | "size-fdiag" => Self::ResizeNwSe,
            "e-resize" | "right_side" => Self::ResizeEast,
            "se-resize" | "bottom_right_corner" => Self::ResizeSouthEast,
            "s-resize" | "bottom_side" => Self::ResizeSouth,
            "sw-resize" | "bottom_left_corner" => Self::ResizeSouthWest,
            "w-resize" | "left_side" => Self::ResizeWest,
            "nw-resize" | "top_left_corner" => Self::ResizeNorthWest,
            "n-resize" | "top_side" => Self::ResizeNorth,
            "ne-resize" | "top_right_corner" => Self::ResizeNorthEast,
            "zoom-in" | "zoom_in" => Self::ZoomIn,
            "zoom-out" | "zoom_out" => Self::ZoomOut,
            _ => return Option::None,
        };
        Some(shape)
    }

    /// The canonical name of this shape.
    ///
    /// A shape set through an alias is reported by this name.  Every
    /// canonical name is accepted by [`PointerShape::from_name`] and maps back
    /// to the same variant.
    #[must_use]
    pub const fn css_name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::None => "none",
            Self::Text => "text",
            Self::VerticalText => "vertical-text",
            Self::Pointer => "pointer",
            Self::ContextMenu => "context-menu",
            Self::Help => "help",
            Self::Progress => "progress",
            Self::Wait => "wait",
            Self::Cell => "cell",
            Self::Crosshair => "crosshair",
            Self::Move => "move",
            Self::NoDrop => "no-drop",
            Self::NotAllowed => "not-allowed",
            Self::Grab => "grab",
            Self::Grabbing => "grabbing",
            Self::Alias => "alias",
            Self::Copy => "copy",
            Self::AllScroll => "all-scroll",
            Self::ResizeHorizontal => "ew-resize",
            Self::ResizeVertical => "ns-resize",
            Self::ResizeNeSw => "nesw-resize",
            Self::ResizeNwSe => "nwse-resize",
            Self::ResizeEast => "e-resize",
            Self::ResizeSouthEast => "se-resize",
            Self::ResizeSouth => "s-resize",
            Self::ResizeSouthWest => "sw-resize",
            Self::ResizeWest => "w-resize",
            Self::ResizeNorthWest => "nw-resize",
            Self::ResizeNorth => "n-resize",
            Self::ResizeNorthEast => "ne-resize",
            Self::ZoomIn => "zoom-in",
            Self::ZoomOut => "zoom-out",
        }
    }
}

impl fmt::Display for PointerShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.css_name())
    }
}

impl From<&str> for PointerShape {
    /// Convert an xcursor / CSS cursor name to a `PointerShape`, trimming the
    /// name first.  Unknown names and empty strings map to
    /// `PointerShape::Default`.
    ///
    /// TODO(139.3): temporary shim so the OSC 22 parser keeps compiling.  Task
    /// 139.3 removes it in favour of [`PointerShape::from_name`], which is
    /// case-sensitive, does not trim, and reports unknown names.
    fn from(name: &str) -> Self {
        Self::from_name(name.trim()).unwrap_or_default()
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
    fn shim_trims_and_defaults_unknown() {
        assert_eq!(PointerShape::from(" text "), PointerShape::Text);
        assert_eq!(PointerShape::from("\tpointer"), PointerShape::Pointer);
        assert_eq!(PointerShape::from("Pointer"), PointerShape::Default);
        assert_eq!(PointerShape::from("TEXT"), PointerShape::Default);
    }

    // ── from_name / css_name ────────────────────────────────────────

    const SPEC_NAMES: [(&str, PointerShape); 30] = [
        ("alias", PointerShape::Alias),
        ("cell", PointerShape::Cell),
        ("copy", PointerShape::Copy),
        ("crosshair", PointerShape::Crosshair),
        ("default", PointerShape::Default),
        ("e-resize", PointerShape::ResizeEast),
        ("ew-resize", PointerShape::ResizeHorizontal),
        ("grab", PointerShape::Grab),
        ("grabbing", PointerShape::Grabbing),
        ("help", PointerShape::Help),
        ("move", PointerShape::Move),
        ("n-resize", PointerShape::ResizeNorth),
        ("ne-resize", PointerShape::ResizeNorthEast),
        ("nesw-resize", PointerShape::ResizeNeSw),
        ("no-drop", PointerShape::NoDrop),
        ("not-allowed", PointerShape::NotAllowed),
        ("ns-resize", PointerShape::ResizeVertical),
        ("nw-resize", PointerShape::ResizeNorthWest),
        ("nwse-resize", PointerShape::ResizeNwSe),
        ("pointer", PointerShape::Pointer),
        ("progress", PointerShape::Progress),
        ("s-resize", PointerShape::ResizeSouth),
        ("se-resize", PointerShape::ResizeSouthEast),
        ("sw-resize", PointerShape::ResizeSouthWest),
        ("text", PointerShape::Text),
        ("vertical-text", PointerShape::VerticalText),
        ("w-resize", PointerShape::ResizeWest),
        ("wait", PointerShape::Wait),
        ("zoom-in", PointerShape::ZoomIn),
        ("zoom-out", PointerShape::ZoomOut),
    ];

    const EXTRA_NAMES: [(&str, PointerShape); 3] = [
        ("none", PointerShape::None),
        ("context-menu", PointerShape::ContextMenu),
        ("all-scroll", PointerShape::AllScroll),
    ];

    const CSS_ALIASES: [(&str, PointerShape); 2] = [
        ("col-resize", PointerShape::ResizeHorizontal),
        ("row-resize", PointerShape::ResizeVertical),
    ];

    const KITTY_ALIASES: [(&str, PointerShape); 46] = [
        ("left_ptr", PointerShape::Default),
        ("arrow", PointerShape::Default),
        ("xterm", PointerShape::Text),
        ("ibeam", PointerShape::Text),
        ("beam", PointerShape::Text),
        ("pointing_hand", PointerShape::Pointer),
        ("hand2", PointerShape::Pointer),
        ("hand", PointerShape::Pointer),
        ("question_arrow", PointerShape::Help),
        ("whats_this", PointerShape::Help),
        ("clock", PointerShape::Wait),
        ("watch", PointerShape::Wait),
        ("half-busy", PointerShape::Progress),
        ("left_ptr_watch", PointerShape::Progress),
        ("tcross", PointerShape::Crosshair),
        ("plus", PointerShape::Cell),
        ("cross", PointerShape::Cell),
        ("fleur", PointerShape::Move),
        ("pointer-move", PointerShape::Move),
        ("right_side", PointerShape::ResizeEast),
        ("top_right_corner", PointerShape::ResizeNorthEast),
        ("top_left_corner", PointerShape::ResizeNorthWest),
        ("top_side", PointerShape::ResizeNorth),
        ("bottom_right_corner", PointerShape::ResizeSouthEast),
        ("bottom_left_corner", PointerShape::ResizeSouthWest),
        ("bottom_side", PointerShape::ResizeSouth),
        ("left_side", PointerShape::ResizeWest),
        ("sb_h_double_arrow", PointerShape::ResizeHorizontal),
        ("split_h", PointerShape::ResizeHorizontal),
        ("sb_v_double_arrow", PointerShape::ResizeVertical),
        ("split_v", PointerShape::ResizeVertical),
        ("size_bdiag", PointerShape::ResizeNeSw),
        ("size-bdiag", PointerShape::ResizeNeSw),
        ("size_fdiag", PointerShape::ResizeNwSe),
        ("size-fdiag", PointerShape::ResizeNwSe),
        ("zoom_in", PointerShape::ZoomIn),
        ("zoom_out", PointerShape::ZoomOut),
        ("dnd-link", PointerShape::Alias),
        ("dnd-copy", PointerShape::Copy),
        ("forbidden", PointerShape::NotAllowed),
        ("crossed_circle", PointerShape::NotAllowed),
        ("dnd-no-drop", PointerShape::NoDrop),
        ("openhand", PointerShape::Grab),
        ("hand1", PointerShape::Grab),
        ("closedhand", PointerShape::Grabbing),
        ("dnd-none", PointerShape::Grabbing),
    ];

    const ALL_VARIANTS: [PointerShape; 33] = [
        PointerShape::Default,
        PointerShape::None,
        PointerShape::Text,
        PointerShape::VerticalText,
        PointerShape::Pointer,
        PointerShape::ContextMenu,
        PointerShape::Help,
        PointerShape::Progress,
        PointerShape::Wait,
        PointerShape::Cell,
        PointerShape::Crosshair,
        PointerShape::Move,
        PointerShape::NoDrop,
        PointerShape::NotAllowed,
        PointerShape::Grab,
        PointerShape::Grabbing,
        PointerShape::Alias,
        PointerShape::Copy,
        PointerShape::AllScroll,
        PointerShape::ResizeHorizontal,
        PointerShape::ResizeVertical,
        PointerShape::ResizeNeSw,
        PointerShape::ResizeNwSe,
        PointerShape::ResizeEast,
        PointerShape::ResizeSouthEast,
        PointerShape::ResizeSouth,
        PointerShape::ResizeSouthWest,
        PointerShape::ResizeWest,
        PointerShape::ResizeNorthWest,
        PointerShape::ResizeNorth,
        PointerShape::ResizeNorthEast,
        PointerShape::ZoomIn,
        PointerShape::ZoomOut,
    ];

    #[test]
    fn from_name_maps_every_table_entry() {
        let tables: [&[(&str, PointerShape)]; 4] =
            [&SPEC_NAMES, &EXTRA_NAMES, &CSS_ALIASES, &KITTY_ALIASES];
        for table in tables {
            for (name, expected) in table {
                assert_eq!(
                    PointerShape::from_name(name),
                    Some(*expected),
                    "name {name:?}"
                );
            }
        }
    }

    #[test]
    fn css_name_round_trips_spec_and_extra_names() {
        for (name, _) in SPEC_NAMES.iter().chain(EXTRA_NAMES.iter()) {
            let shape = PointerShape::from_name(name).expect("table name");
            assert_eq!(shape.css_name(), *name);
        }
    }

    #[test]
    fn from_name_of_css_name_is_identity_for_every_variant() {
        for variant in ALL_VARIANTS {
            assert_eq!(
                PointerShape::from_name(variant.css_name()),
                Some(variant),
                "variant {variant:?}"
            );
        }
    }

    #[test]
    fn css_name_of_aliases_is_canonical() {
        assert_eq!(PointerShape::ResizeHorizontal.css_name(), "ew-resize");
        assert_eq!(PointerShape::ResizeVertical.css_name(), "ns-resize");
        for (alias, shape) in CSS_ALIASES.iter().chain(KITTY_ALIASES.iter()) {
            let canonical = shape.css_name();
            assert_ne!(canonical, *alias, "{alias:?} is not canonical");
            assert_eq!(PointerShape::from_name(canonical), Some(*shape));
        }
    }

    #[test]
    fn display_delegates_to_css_name() {
        for variant in ALL_VARIANTS {
            assert_eq!(variant.to_string(), variant.css_name());
        }
    }

    #[test]
    fn from_name_is_case_sensitive() {
        assert_eq!(PointerShape::from_name("Pointer"), None);
        assert_eq!(PointerShape::from_name("TEXT"), None);
        assert_eq!(PointerShape::from_name("Default"), None);
        assert_eq!(PointerShape::from_name("Left_Ptr"), None);
    }

    #[test]
    fn from_name_does_not_trim() {
        assert_eq!(PointerShape::from_name(" text"), None);
        assert_eq!(PointerShape::from_name("text "), None);
        assert_eq!(PointerShape::from_name("\ttext"), None);
        assert_eq!(PointerShape::from_name("   "), None);
    }

    #[test]
    fn from_name_rejects_unknown_and_empty() {
        assert_eq!(PointerShape::from_name(""), None);
        assert_eq!(PointerShape::from_name("banana"), None);
        assert_eq!(PointerShape::from_name("not-a-cursor"), None);
        assert_eq!(PointerShape::from_name("text,pointer"), None);
        assert_eq!(PointerShape::from_name("__current__"), None);
    }

    #[test]
    fn shim_agrees_with_from_name_for_every_table_entry() {
        let tables: [&[(&str, PointerShape)]; 4] =
            [&SPEC_NAMES, &EXTRA_NAMES, &CSS_ALIASES, &KITTY_ALIASES];
        for table in tables {
            for (name, expected) in table {
                assert_eq!(PointerShape::from(*name), *expected, "name {name:?}");
            }
        }
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

    // ── Display for all variants ────────────────────────────────────

    #[test]
    fn display_all_pointer_shape_variants() {
        assert_eq!(PointerShape::None.to_string(), "none");
        assert_eq!(PointerShape::VerticalText.to_string(), "vertical-text");
        assert_eq!(PointerShape::ContextMenu.to_string(), "context-menu");
        assert_eq!(PointerShape::Help.to_string(), "help");
        assert_eq!(PointerShape::Progress.to_string(), "progress");
        assert_eq!(PointerShape::Wait.to_string(), "wait");
        assert_eq!(PointerShape::Cell.to_string(), "cell");
        assert_eq!(PointerShape::Crosshair.to_string(), "crosshair");
        assert_eq!(PointerShape::Move.to_string(), "move");
        assert_eq!(PointerShape::NoDrop.to_string(), "no-drop");
        assert_eq!(PointerShape::NotAllowed.to_string(), "not-allowed");
        assert_eq!(PointerShape::Grab.to_string(), "grab");
        assert_eq!(PointerShape::Grabbing.to_string(), "grabbing");
        assert_eq!(PointerShape::Alias.to_string(), "alias");
        assert_eq!(PointerShape::Copy.to_string(), "copy");
        assert_eq!(PointerShape::AllScroll.to_string(), "all-scroll");
        assert_eq!(PointerShape::ResizeHorizontal.to_string(), "ew-resize");
        assert_eq!(PointerShape::ResizeVertical.to_string(), "ns-resize");
        assert_eq!(PointerShape::ResizeNeSw.to_string(), "nesw-resize");
        assert_eq!(PointerShape::ResizeNwSe.to_string(), "nwse-resize");
        assert_eq!(PointerShape::ResizeEast.to_string(), "e-resize");
        assert_eq!(PointerShape::ResizeSouthEast.to_string(), "se-resize");
        assert_eq!(PointerShape::ResizeSouth.to_string(), "s-resize");
        assert_eq!(PointerShape::ResizeSouthWest.to_string(), "sw-resize");
        assert_eq!(PointerShape::ResizeWest.to_string(), "w-resize");
        assert_eq!(PointerShape::ResizeNorthWest.to_string(), "nw-resize");
        assert_eq!(PointerShape::ResizeNorth.to_string(), "n-resize");
        assert_eq!(PointerShape::ResizeNorthEast.to_string(), "ne-resize");
        assert_eq!(PointerShape::ZoomIn.to_string(), "zoom-in");
        assert_eq!(PointerShape::ZoomOut.to_string(), "zoom-out");
    }
}
