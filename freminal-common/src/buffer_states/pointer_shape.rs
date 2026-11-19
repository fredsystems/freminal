// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `PointerShape` — typed representation of the X11/CSS pointer cursor shape
//! requested through `OSC 22`, and the types of the OSC 22 operations.
//!
//! The variants map onto `egui::CursorIcon` so the GUI can convert without
//! carrying an egui dependency into the common crate.  A [`PointerShape`] is
//! one shape; the protocol state is a per-screen stack of
//! [`PointerShapeEntry`] values, which an OSC 22 request sets, pushes or pops
//! (see [`PointerShapeOp`]).
//!
//! [`PointerShape::from_name`] is the one name table: the 30 CSS cursor names
//! from the kitty pointer-shapes specification, the CSS aliases `col-resize` /
//! `row-resize`, and kitty's legacy X11 aliases.  [`PointerShape::css_name`]
//! gives the canonical name of a shape, which is also what `Display` prints.

use std::fmt;

/// The mouse pointer (cursor) shape requested by the running application via
/// OSC 22.
///
/// An application sends `OSC 22 ; <name> ST` to request a specific X11 /
/// CSS pointer shape.  The type is exactly one shape: OSC 22 sets, pushes and
/// pops it on a per-screen stack of [`PointerShapeEntry`] values.  An empty set
/// name stores an unset entry (which shows [`PointerShape::Default`]);
/// `"default"` is an ordinary shape, not a reset.
///
/// The variants are named after their `egui::CursorIcon` counterparts so that
/// the GUI layer can convert with a simple `match` — no string comparison at
/// render time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointerShape {
    /// Default OS pointer (arrow).  Also what an empty stack or an unset top
    /// entry shows.
    #[default]
    Default,
    /// I-beam for text selection.
    Text,
    /// Vertical I-beam.
    VerticalText,
    /// Pointing-hand (links / clickable items).
    Pointer,
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

/// One entry of an OSC 22 pointer-shape stack.
///
/// Every entry is either a requested shape or [`PointerShapeEntry::Unset`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerShapeEntry {
    /// A shape requested by the application.
    Shape(PointerShape),
    /// The entry an empty set name stores (kitty's `INVALID_POINTER`).
    ///
    /// It shows the default shape and reports `0` to `__current__`, while the
    /// entries below it survive: a later pop reveals them again.
    Unset,
}

/// The most names one OSC 22 request may carry.
///
/// Names past this cap are ignored, for every operation.  This is a freminal
/// policy (kitty has no cap): it bounds the reply a single query can produce.
pub const MAX_OSC22_NAMES: usize = 64;

/// One item of an OSC 22 query (`OSC 22 ; ? name , name ST`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerShapeQuery {
    /// A name freminal accepts for set and push.
    Supported,
    /// A name freminal does not accept (including an empty name).
    Unsupported,
    /// `__current__`: the active screen's current shape.
    Current,
    /// `__default__`: the shape shown when none is set.
    Default,
    /// `__grabbed__`: the shape shown while the application has grabbed the
    /// mouse (mouse reporting is on).
    Grabbed,
}

/// A parsed OSC 22 request.
///
/// Names that freminal does not know are already dropped by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointerShapeOp {
    /// `OSC 22 ; [=] names ST`: replace the top entry, applied in order.
    Set(Vec<PointerShapeEntry>),
    /// `OSC 22 ; > names ST`: push each shape in order.  Empty and unknown
    /// names are already dropped.
    Push(Vec<PointerShape>),
    /// `OSC 22 ; < ST`: pop one entry.
    Pop,
    /// `OSC 22 ; ? names ST`: ask about names; always answered.
    Query(Vec<PointerShapeQuery>),
}

impl PointerShape {
    /// Look up a pointer shape by its OSC 22 name.
    ///
    /// This is the single name table.  It accepts the 30 CSS cursor names from
    /// the kitty pointer-shapes specification, the CSS names `col-resize` /
    /// `row-resize` (aliases of `ew-resize` / `ns-resize`), and kitty's legacy
    /// X11 aliases.
    ///
    /// Matching is case-sensitive and the name is not trimmed.  Anything not in
    /// the table, including the empty string, returns `None`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let shape = match name {
            "default" | "left_ptr" | "arrow" => Self::Default,
            "text" | "xterm" | "ibeam" | "beam" => Self::Text,
            "vertical-text" => Self::VerticalText,
            "pointer" | "pointing_hand" | "hand2" | "hand" => Self::Pointer,
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
            _ => return None,
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
            Self::Text => "text",
            Self::VerticalText => "vertical-text",
            Self::Pointer => "pointer",
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

#[cfg(test)]
mod tests {
    use super::PointerShape;

    #[test]
    fn explicit_default_names() {
        assert_eq!(
            PointerShape::from_name("default"),
            Some(PointerShape::Default)
        );
        assert_eq!(
            PointerShape::from_name("arrow"),
            Some(PointerShape::Default)
        );
        assert_eq!(
            PointerShape::from_name("left_ptr"),
            Some(PointerShape::Default)
        );
    }

    #[test]
    fn text_cursor_variants() {
        assert_eq!(PointerShape::from_name("text"), Some(PointerShape::Text));
        assert_eq!(PointerShape::from_name("xterm"), Some(PointerShape::Text));
    }

    #[test]
    fn vertical_text() {
        assert_eq!(
            PointerShape::from_name("vertical-text"),
            Some(PointerShape::VerticalText)
        );
    }

    #[test]
    fn pointer_variants() {
        assert_eq!(
            PointerShape::from_name("pointer"),
            Some(PointerShape::Pointer)
        );
        assert_eq!(PointerShape::from_name("hand"), Some(PointerShape::Pointer));
        assert_eq!(
            PointerShape::from_name("hand2"),
            Some(PointerShape::Pointer)
        );
    }

    #[test]
    fn help_cursor() {
        assert_eq!(PointerShape::from_name("help"), Some(PointerShape::Help));
    }

    #[test]
    fn progress_variants() {
        assert_eq!(
            PointerShape::from_name("progress"),
            Some(PointerShape::Progress)
        );
        assert_eq!(
            PointerShape::from_name("left_ptr_watch"),
            Some(PointerShape::Progress)
        );
    }

    #[test]
    fn wait_variants() {
        assert_eq!(PointerShape::from_name("wait"), Some(PointerShape::Wait));
        assert_eq!(PointerShape::from_name("watch"), Some(PointerShape::Wait));
    }

    #[test]
    fn crosshair() {
        assert_eq!(
            PointerShape::from_name("crosshair"),
            Some(PointerShape::Crosshair)
        );
    }

    #[test]
    fn cell_cursor() {
        assert_eq!(PointerShape::from_name("cell"), Some(PointerShape::Cell));
    }

    #[test]
    fn move_variants() {
        assert_eq!(PointerShape::from_name("move"), Some(PointerShape::Move));
        assert_eq!(PointerShape::from_name("fleur"), Some(PointerShape::Move));
    }

    #[test]
    fn no_drop() {
        assert_eq!(
            PointerShape::from_name("no-drop"),
            Some(PointerShape::NoDrop)
        );
    }

    #[test]
    fn not_allowed() {
        assert_eq!(
            PointerShape::from_name("not-allowed"),
            Some(PointerShape::NotAllowed)
        );
    }

    #[test]
    fn grab_and_grabbing() {
        assert_eq!(PointerShape::from_name("grab"), Some(PointerShape::Grab));
        assert_eq!(
            PointerShape::from_name("grabbing"),
            Some(PointerShape::Grabbing)
        );
    }

    #[test]
    fn alias_and_copy() {
        assert_eq!(PointerShape::from_name("alias"), Some(PointerShape::Alias));
        assert_eq!(PointerShape::from_name("copy"), Some(PointerShape::Copy));
    }

    #[test]
    fn horizontal_resize_variants() {
        assert_eq!(
            PointerShape::from_name("col-resize"),
            Some(PointerShape::ResizeHorizontal)
        );
        assert_eq!(
            PointerShape::from_name("ew-resize"),
            Some(PointerShape::ResizeHorizontal)
        );
    }

    #[test]
    fn vertical_resize_variants() {
        assert_eq!(
            PointerShape::from_name("row-resize"),
            Some(PointerShape::ResizeVertical)
        );
        assert_eq!(
            PointerShape::from_name("ns-resize"),
            Some(PointerShape::ResizeVertical)
        );
    }

    #[test]
    fn diagonal_resizes() {
        assert_eq!(
            PointerShape::from_name("nesw-resize"),
            Some(PointerShape::ResizeNeSw)
        );
        assert_eq!(
            PointerShape::from_name("nwse-resize"),
            Some(PointerShape::ResizeNwSe)
        );
    }

    #[test]
    fn directional_resizes() {
        assert_eq!(
            PointerShape::from_name("e-resize"),
            Some(PointerShape::ResizeEast)
        );
        assert_eq!(
            PointerShape::from_name("se-resize"),
            Some(PointerShape::ResizeSouthEast)
        );
        assert_eq!(
            PointerShape::from_name("s-resize"),
            Some(PointerShape::ResizeSouth)
        );
        assert_eq!(
            PointerShape::from_name("sw-resize"),
            Some(PointerShape::ResizeSouthWest)
        );
        assert_eq!(
            PointerShape::from_name("w-resize"),
            Some(PointerShape::ResizeWest)
        );
        assert_eq!(
            PointerShape::from_name("nw-resize"),
            Some(PointerShape::ResizeNorthWest)
        );
        assert_eq!(
            PointerShape::from_name("n-resize"),
            Some(PointerShape::ResizeNorth)
        );
        assert_eq!(
            PointerShape::from_name("ne-resize"),
            Some(PointerShape::ResizeNorthEast)
        );
    }

    #[test]
    fn zoom_in_and_out() {
        assert_eq!(
            PointerShape::from_name("zoom-in"),
            Some(PointerShape::ZoomIn)
        );
        assert_eq!(
            PointerShape::from_name("zoom-out"),
            Some(PointerShape::ZoomOut)
        );
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

    /// Declares [`ALL_VARIANTS`] and [`variant_name`] from one list.
    ///
    /// `variant_name` is an exhaustive `match` with no wildcard, so a
    /// [`PointerShape`] variant missing from this list fails to compile.  The
    /// array is built from the same list, so it cannot omit a listed variant.
    macro_rules! declare_variants {
        ($($variant:ident),+ $(,)?) => {
            const ALL_VARIANTS: [PointerShape; [$(stringify!($variant)),+].len()] =
                [$(PointerShape::$variant),+];

            const fn variant_name(variant: PointerShape) -> &'static str {
                match variant {
                    $(PointerShape::$variant => stringify!($variant)),+
                }
            }
        };
    }

    declare_variants!(
        Default,
        Text,
        VerticalText,
        Pointer,
        Help,
        Progress,
        Wait,
        Cell,
        Crosshair,
        Move,
        NoDrop,
        NotAllowed,
        Grab,
        Grabbing,
        Alias,
        Copy,
        ResizeHorizontal,
        ResizeVertical,
        ResizeNeSw,
        ResizeNwSe,
        ResizeEast,
        ResizeSouthEast,
        ResizeSouth,
        ResizeSouthWest,
        ResizeWest,
        ResizeNorthWest,
        ResizeNorth,
        ResizeNorthEast,
        ZoomIn,
        ZoomOut,
    );

    #[test]
    fn all_variants_lists_every_variant_exactly_once() {
        for (index, variant) in ALL_VARIANTS.iter().enumerate() {
            assert_eq!(
                variant_name(*variant),
                format!("{variant:?}"),
                "variant {variant:?}"
            );
            assert!(
                !ALL_VARIANTS[..index].contains(variant),
                "variant {variant:?} is listed twice"
            );
        }
    }

    #[test]
    fn removed_extras_are_not_names() {
        for name in ["none", "context-menu", "all-scroll"] {
            assert_eq!(PointerShape::from_name(name), None, "name {name:?}");
        }
    }

    #[test]
    fn from_name_maps_every_table_entry() {
        let tables: [&[(&str, PointerShape)]; 3] = [&SPEC_NAMES, &CSS_ALIASES, &KITTY_ALIASES];
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
    fn css_name_round_trips_spec_names() {
        for (name, _) in SPEC_NAMES {
            let shape = PointerShape::from_name(name).expect("table name");
            assert_eq!(shape.css_name(), name);
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
        assert_eq!(PointerShape::VerticalText.to_string(), "vertical-text");
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
