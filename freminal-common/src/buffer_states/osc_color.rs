// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Request types for the OSC colour-control sequences: OSC 4 / 104 (palette)
//! and OSC 10-19 / 110-119 (dynamic colours).
//!
//! One OSC 4 or OSC 10-19 sequence can carry several set / query operations.
//! The parser turns that into an ordered list of the operations below; the
//! handler applies them in order and answers each query in order.

use crate::colors::DynamicColorSlot;

/// What one colour operation asks for: set a colour, or report the current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorRequest {
    /// Set the colour to this `(r, g, b)` value (8 bits per channel).
    Set((u8, u8, u8)),
    /// Report the current colour (the `?` value on the wire).
    Query,
}

/// One `index ; spec` pair of an OSC 4 sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteColorOp {
    /// The palette entry (0-255) the operation applies to.
    pub index: u8,
    /// Whether to set that entry or report it.
    pub request: ColorRequest,
}

/// Which palette entries an OSC 104 sequence resets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteReset {
    /// Reset the whole palette (`OSC 104` with no index).
    All,
    /// Reset exactly these entries, in the order given.
    Indices(Vec<u8>),
}

/// One value of an OSC 10-19 sequence that applies to a supported slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DynamicColorOp {
    /// The dynamic colour slot the operation applies to.
    pub slot: DynamicColorSlot,
    /// Whether to set that slot or report it.
    pub request: ColorRequest,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_request_is_copy_and_comparable() {
        let set = ColorRequest::Set((1, 2, 3));
        let copy = set;
        assert_eq!(set, copy);
        assert_ne!(set, ColorRequest::Query);
    }

    #[test]
    fn palette_reset_variants_are_distinct() {
        assert_ne!(PaletteReset::All, PaletteReset::Indices(vec![]));
        assert_eq!(
            PaletteReset::Indices(vec![1, 2]),
            PaletteReset::Indices(vec![1, 2])
        );
    }

    #[test]
    fn ops_compare_by_value() {
        let a = PaletteColorOp {
            index: 4,
            request: ColorRequest::Query,
        };
        assert_eq!(a, a);
        let d = DynamicColorOp {
            slot: DynamicColorSlot::Cursor,
            request: ColorRequest::Query,
        };
        assert_ne!(
            d,
            DynamicColorOp {
                slot: DynamicColorSlot::Foreground,
                request: ColorRequest::Query,
            }
        );
    }
}
