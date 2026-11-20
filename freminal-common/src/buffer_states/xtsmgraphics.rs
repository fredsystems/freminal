// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! XTSMGRAPHICS (`CSI ? Pi ; Pa ; Pv S`) request types.
//!
//! The parser classifies the item (`Pi`) and action (`Pa`) of a request; the
//! terminal handler decides the reply, because the answer depends on terminal
//! state (cell pixel size, grid size).  The value (`Pv`) is not carried: xterm
//! ignores it except when setting, and Freminal answers every set with
//! "failure".
//!
//! Reference: xterm `ctlseqs.txt` ("CSI ? Pi ; Pa ; Pv S") and `charproc.c`
//! (`CASE_GRAPHICS_ATTRIBUTES`).

/// The graphics attribute a request is about (`Pi`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsAttributeItem {
    /// `Pi = 1` — the number of sixel colour registers.
    ColorRegisters,
    /// `Pi = 2` — the sixel graphics geometry, in pixels.
    SixelGeometry,
    /// `Pi = 3` — the `ReGIS` graphics geometry, in pixels.
    ReGisGeometry,
    /// Any other `Pi`; answered with status 1 (error in `Pi`).
    Unknown(u32),
}

impl GraphicsAttributeItem {
    /// The numeric `Pi` this item was parsed from, echoed back in the reply.
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::ColorRegisters => 1,
            Self::SixelGeometry => 2,
            Self::ReGisGeometry => 3,
            Self::Unknown(pi) => pi,
        }
    }
}

impl From<u32> for GraphicsAttributeItem {
    fn from(pi: u32) -> Self {
        match pi {
            1 => Self::ColorRegisters,
            2 => Self::SixelGeometry,
            3 => Self::ReGisGeometry,
            other => Self::Unknown(other),
        }
    }
}

/// What a request does to the attribute (`Pa`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsAttributeAction {
    /// `Pa = 1` — read the current value.
    Read,
    /// `Pa = 2` — reset to the default.
    Reset,
    /// `Pa = 3` — set to `Pv`.
    Set,
    /// `Pa = 4` — read the maximum allowed value.
    ReadMaximum,
    /// Any other `Pa`; answered with status 2 (error in `Pa`).
    Unknown(u32),
}

impl From<u32> for GraphicsAttributeAction {
    fn from(pa: u32) -> Self {
        match pa {
            1 => Self::Read,
            2 => Self::Reset,
            3 => Self::Set,
            4 => Self::ReadMaximum,
            other => Self::Unknown(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_round_trips_its_code() {
        for pi in [0u32, 1, 2, 3, 4, 99, u32::MAX] {
            assert_eq!(GraphicsAttributeItem::from(pi).code(), pi);
        }
    }

    #[test]
    fn item_classification() {
        assert_eq!(
            GraphicsAttributeItem::from(1),
            GraphicsAttributeItem::ColorRegisters
        );
        assert_eq!(
            GraphicsAttributeItem::from(2),
            GraphicsAttributeItem::SixelGeometry
        );
        assert_eq!(
            GraphicsAttributeItem::from(3),
            GraphicsAttributeItem::ReGisGeometry
        );
        assert_eq!(
            GraphicsAttributeItem::from(0),
            GraphicsAttributeItem::Unknown(0)
        );
    }

    #[test]
    fn action_classification() {
        assert_eq!(
            GraphicsAttributeAction::from(1),
            GraphicsAttributeAction::Read
        );
        assert_eq!(
            GraphicsAttributeAction::from(2),
            GraphicsAttributeAction::Reset
        );
        assert_eq!(
            GraphicsAttributeAction::from(3),
            GraphicsAttributeAction::Set
        );
        assert_eq!(
            GraphicsAttributeAction::from(4),
            GraphicsAttributeAction::ReadMaximum
        );
        assert_eq!(
            GraphicsAttributeAction::from(5),
            GraphicsAttributeAction::Unknown(5)
        );
    }
}
