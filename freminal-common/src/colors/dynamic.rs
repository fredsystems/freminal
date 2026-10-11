// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The xterm dynamic-colour slots ([`DynamicColorSlot`]) and the per-slot
//! runtime overrides ([`DynamicColorOverrides`]) set by OSC 10/11/12/17/19.

/// Number of dynamic-colour slots.
const SLOT_COUNT: usize = 5;

/// The xterm dynamic-colour slots freminal supports (OSC 10, 11, 12, 17, 19).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DynamicColorSlot {
    /// Default foreground colour (OSC 10).
    Foreground,
    /// Default background colour (OSC 11).
    Background,
    /// Text cursor colour (OSC 12).
    Cursor,
    /// Selection (highlight) background colour (OSC 17).
    SelectionBackground,
    /// Selection (highlight) foreground colour (OSC 19).
    SelectionForeground,
}

impl DynamicColorSlot {
    /// Every slot, in OSC-code order.
    pub const ALL: [Self; SLOT_COUNT] = [
        Self::Foreground,
        Self::Background,
        Self::Cursor,
        Self::SelectionBackground,
        Self::SelectionForeground,
    ];

    /// The OSC number that *sets* this slot (10, 11, 12, 17 or 19).
    ///
    /// The matching reset codes (110, 111, 112, 117, 119) are not
    /// represented here.
    #[must_use]
    pub const fn osc_code(self) -> u16 {
        match self {
            Self::Foreground => 10,
            Self::Background => 11,
            Self::Cursor => 12,
            Self::SelectionBackground => 17,
            Self::SelectionForeground => 19,
        }
    }

    /// The slot set by OSC `code`, or `None` if `code` is not one of the
    /// set codes 10, 11, 12, 17, 19.
    ///
    /// The reset codes (110, 111, 112, 117, 119) are deliberately rejected.
    #[must_use]
    pub const fn from_osc_code(code: u16) -> Option<Self> {
        match code {
            10 => Some(Self::Foreground),
            11 => Some(Self::Background),
            12 => Some(Self::Cursor),
            17 => Some(Self::SelectionBackground),
            19 => Some(Self::SelectionForeground),
            _ => None,
        }
    }

    /// Position of this slot in [`DynamicColorOverrides`]'s storage.
    const fn index(self) -> usize {
        match self {
            Self::Foreground => 0,
            Self::Background => 1,
            Self::Cursor => 2,
            Self::SelectionBackground => 3,
            Self::SelectionForeground => 4,
        }
    }
}

/// Per-slot runtime overrides set by OSC 10/11/12/17/19.
///
/// `None` for a slot means "use the theme's colour".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DynamicColorOverrides {
    slots: [Option<(u8, u8, u8)>; SLOT_COUNT],
}

impl DynamicColorOverrides {
    /// The current override for `slot`, or `None` if it follows the theme.
    #[must_use]
    pub const fn get(&self, slot: DynamicColorSlot) -> Option<(u8, u8, u8)> {
        self.slots[slot.index()]
    }

    /// Override `slot` with the given RGB value.
    pub const fn set(&mut self, slot: DynamicColorSlot, rgb: (u8, u8, u8)) {
        self.slots[slot.index()] = Some(rgb);
    }

    /// Clear the override for `slot`, returning it to the theme colour.
    pub const fn reset(&mut self, slot: DynamicColorSlot) {
        self.slots[slot.index()] = None;
    }

    /// Clear every override.
    pub const fn reset_all(&mut self) {
        self.slots = [None; SLOT_COUNT];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_no_overrides() {
        let o = DynamicColorOverrides::default();
        for slot in DynamicColorSlot::ALL {
            assert_eq!(o.get(slot), None, "{slot:?} should start unset");
        }
    }

    #[test]
    fn set_and_get_are_independent_per_slot() {
        let mut o = DynamicColorOverrides::default();
        for (i, slot) in DynamicColorSlot::ALL.into_iter().enumerate() {
            let v = u8::try_from(i).unwrap_or(0) + 1;
            o.set(slot, (v, v, v));
        }
        for (i, slot) in DynamicColorSlot::ALL.into_iter().enumerate() {
            let v = u8::try_from(i).unwrap_or(0) + 1;
            assert_eq!(o.get(slot), Some((v, v, v)));
        }
    }

    #[test]
    fn set_overwrites_previous_value() {
        let mut o = DynamicColorOverrides::default();
        o.set(DynamicColorSlot::Cursor, (1, 2, 3));
        o.set(DynamicColorSlot::Cursor, (4, 5, 6));
        assert_eq!(o.get(DynamicColorSlot::Cursor), Some((4, 5, 6)));
    }

    #[test]
    fn reset_clears_only_one_slot() {
        let mut o = DynamicColorOverrides::default();
        for slot in DynamicColorSlot::ALL {
            o.set(slot, (9, 9, 9));
        }
        o.reset(DynamicColorSlot::Background);
        for slot in DynamicColorSlot::ALL {
            if slot == DynamicColorSlot::Background {
                assert_eq!(o.get(slot), None);
            } else {
                assert_eq!(o.get(slot), Some((9, 9, 9)), "{slot:?} must be untouched");
            }
        }
    }

    #[test]
    fn reset_all_clears_every_slot() {
        let mut o = DynamicColorOverrides::default();
        for slot in DynamicColorSlot::ALL {
            o.set(slot, (7, 7, 7));
        }
        o.reset_all();
        assert_eq!(o, DynamicColorOverrides::default());
    }

    #[test]
    fn osc_codes_are_the_documented_set_codes() {
        let codes: Vec<u16> = DynamicColorSlot::ALL
            .into_iter()
            .map(DynamicColorSlot::osc_code)
            .collect();
        assert_eq!(codes, vec![10, 11, 12, 17, 19]);
    }

    #[test]
    fn osc_code_round_trips() {
        for slot in DynamicColorSlot::ALL {
            assert_eq!(DynamicColorSlot::from_osc_code(slot.osc_code()), Some(slot));
        }
    }

    #[test]
    fn from_osc_code_rejects_other_codes() {
        for code in [0, 13, 18, 20, 110, 111, 112, 117, 119] {
            assert_eq!(DynamicColorSlot::from_osc_code(code), None, "code {code}");
        }
    }
}
