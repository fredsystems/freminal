// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The G0-G3 character-set model: designation, locking and single shifts.
//!
//! Four slots (G0-G3) each hold a [`CharacterSet`]. GL (the left half, codes
//! `0x20..=0x7E`) is mapped through one slot at a time, and so is GR (the
//! right half, `0xA0..=0xFF`). A single shift (`ESC N` / `ESC O`) maps just the
//! next printable character through G2 or G3.
//!
//! GR is tracked, and saved and restored with the rest of the state, but it is
//! never applied: bytes `0x80` and above are UTF-8, so remapping them would
//! corrupt every multi-byte character.
//!
//! The 96-character designators (`ESC - . /`) are not modelled.

use std::borrow::Cow;

/// A character set that can be designated into a G slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CharacterSet {
    /// US-ASCII (`B`): every code maps to itself.
    #[default]
    Ascii,
    /// The UK national set (`A`): `#` (`0x23`) is the pound sign.
    UnitedKingdom,
    /// DEC Special Graphics (`0`): `0x5F..=0x7E` become line-drawing and
    /// symbol glyphs.
    DecSpecialGraphics,
}

impl CharacterSet {
    /// The set a designator final byte selects (`ESC ( B`, `ESC ) 0`, ...), or
    /// `None` for a final naming a set freminal does not implement.
    #[must_use]
    pub const fn from_designator_final(final_byte: u8) -> Option<Self> {
        match final_byte {
            b'B' => Some(Self::Ascii),
            b'A' => Some(Self::UnitedKingdom),
            b'0' => Some(Self::DecSpecialGraphics),
            _ => None,
        }
    }

    /// The character an ASCII byte becomes in this set, or `None` when the set
    /// leaves the byte unchanged.
    #[must_use]
    pub const fn translate(self, byte: u8) -> Option<char> {
        match self {
            Self::Ascii => None,
            Self::UnitedKingdom => match byte {
                b'#' => Some('\u{00A3}'), // POUND SIGN
                _ => None,
            },
            Self::DecSpecialGraphics => dec_special_graphics(byte),
        }
    }
}

/// The DEC Special Graphics table (`0x5F..=0x7E`).
///
/// Reference: <https://en.wikipedia.org/wiki/DEC_Special_Graphics>
const fn dec_special_graphics(byte: u8) -> Option<char> {
    Some(match byte {
        0x5f => '\u{00A0}', // NO-BREAK SPACE
        0x60 => '\u{25C6}', // BLACK DIAMOND
        0x61 => '\u{2592}', // MEDIUM SHADE
        0x62 => '\u{2409}', // SYMBOL FOR HT
        0x63 => '\u{240C}', // SYMBOL FOR FF
        0x64 => '\u{240D}', // SYMBOL FOR CR
        0x65 => '\u{240A}', // SYMBOL FOR LF
        0x66 => '\u{00B0}', // DEGREE SIGN
        0x67 => '\u{00B1}', // PLUS-MINUS SIGN
        0x68 => '\u{2424}', // SYMBOL FOR NEWLINE
        0x69 => '\u{240B}', // SYMBOL FOR VT
        0x6a => '\u{2518}', // BOX LIGHT UP AND LEFT
        0x6b => '\u{2510}', // BOX LIGHT DOWN AND LEFT
        0x6c => '\u{250C}', // BOX LIGHT DOWN AND RIGHT
        0x6d => '\u{2514}', // BOX LIGHT UP AND RIGHT
        0x6e => '\u{253C}', // BOX LIGHT VERTICAL AND HORIZONTAL
        0x6f => '\u{23BA}', // HORIZONTAL SCAN LINE-1
        0x70 => '\u{23BB}', // HORIZONTAL SCAN LINE-3
        0x71 => '\u{2500}', // BOX LIGHT HORIZONTAL
        0x72 => '\u{23BC}', // HORIZONTAL SCAN LINE-7
        0x73 => '\u{23BD}', // HORIZONTAL SCAN LINE-9
        0x74 => '\u{251C}', // BOX LIGHT VERTICAL AND RIGHT
        0x75 => '\u{2524}', // BOX LIGHT VERTICAL AND LEFT
        0x76 => '\u{2534}', // BOX LIGHT UP AND HORIZONTAL
        0x77 => '\u{252C}', // BOX LIGHT DOWN AND HORIZONTAL
        0x78 => '\u{2502}', // BOX LIGHT VERTICAL
        0x79 => '\u{2264}', // LESS-THAN OR EQUAL TO
        0x7a => '\u{2265}', // GREATER-THAN OR EQUAL TO
        0x7b => '\u{03C0}', // GREEK SMALL LETTER PI
        0x7c => '\u{2260}', // NOT EQUAL TO
        0x7d => '\u{00A3}', // POUND SIGN
        0x7e => '\u{00B7}', // MIDDLE DOT
        _ => return None,
    })
}

/// One of the four character-set slots, G0 to G3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharsetSlot {
    G0,
    G1,
    G2,
    G3,
}

impl CharsetSlot {
    const fn index(self) -> usize {
        match self {
            Self::G0 => 0,
            Self::G1 => 1,
            Self::G2 => 2,
            Self::G3 => 3,
        }
    }
}

/// The complete character-set state: the four slots, what GL and GR invoke,
/// and a pending single shift.
///
/// The default is the power-on state (and what RIS and DECSTR restore): G0-G3
/// all ASCII, GL = G0, GR = G2, no single shift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharsetState {
    sets: [CharacterSet; 4],
    gl: CharsetSlot,
    gr: CharsetSlot,
    single_shift: Option<CharsetSlot>,
}

impl Default for CharsetState {
    fn default() -> Self {
        Self {
            sets: [CharacterSet::Ascii; 4],
            gl: CharsetSlot::G0,
            gr: CharsetSlot::G2,
            single_shift: None,
        }
    }
}

impl CharsetState {
    /// The set designated into `slot`.
    #[must_use]
    pub const fn set(&self, slot: CharsetSlot) -> CharacterSet {
        self.sets[slot.index()]
    }

    /// The slot GL currently invokes.
    #[must_use]
    pub const fn gl(&self) -> CharsetSlot {
        self.gl
    }

    /// The slot GR currently invokes. Tracked only: see the module documentation.
    #[must_use]
    pub const fn gr(&self) -> CharsetSlot {
        self.gr
    }

    /// The pending single shift, if any.
    #[must_use]
    pub const fn single_shift(&self) -> Option<CharsetSlot> {
        self.single_shift
    }

    /// `ESC ( F`, `ESC ) F`, `ESC * F`, `ESC + F`: designate `set` into `slot`.
    pub const fn designate(&mut self, slot: CharsetSlot, set: CharacterSet) {
        self.sets[slot.index()] = set;
    }

    /// SI, SO, LS2 and LS3: invoke `slot` into GL.
    pub const fn invoke_gl(&mut self, slot: CharsetSlot) {
        self.gl = slot;
    }

    /// LS1R, LS2R and LS3R: invoke `slot` into GR.
    pub const fn invoke_gr(&mut self, slot: CharsetSlot) {
        self.gr = slot;
    }

    /// SS2 and SS3: map the next printable character through `slot`.
    pub const fn set_single_shift(&mut self, slot: CharsetSlot) {
        self.single_shift = Some(slot);
    }

    /// Map printable text through GL (or a pending single shift).
    ///
    /// Returns the input untouched, with no allocation, in the overwhelmingly
    /// common case: GL is ASCII and no single shift is pending.
    ///
    /// Otherwise each printable ASCII byte (`0x20..=0x7E`) goes through the
    /// invoked set. A pending single shift applies to the first printable
    /// character only and is then cleared; a non-ASCII character consumes it
    /// and passes through unmapped. Control bytes and DEL are not printable:
    /// they pass through and leave a pending shift alone. Bytes `0x80` and
    /// above are UTF-8 and are never mapped.
    pub fn translate<'a>(&mut self, data: &'a [u8]) -> Cow<'a, [u8]> {
        let gl_set = self.set(self.gl);
        if self.single_shift.is_none() && gl_set == CharacterSet::Ascii {
            return Cow::Borrowed(data);
        }

        let mut out = Vec::with_capacity(data.len() + data.len() / 2);
        let mut utf8 = [0_u8; 4];
        for &byte in data {
            if byte < 0x20 || byte == 0x7f {
                out.push(byte);
                continue;
            }
            let set = self
                .single_shift
                .take()
                .map_or(gl_set, |slot| self.set(slot));
            match set.translate(byte) {
                Some(mapped) => out.extend_from_slice(mapped.encode_utf8(&mut utf8).as_bytes()),
                None => out.push(byte),
            }
        }
        Cow::Owned(out)
    }
}

#[cfg(test)]
mod tests {
    use super::{CharacterSet, CharsetSlot, CharsetState};

    fn translated(state: &mut CharsetState, data: &[u8]) -> String {
        String::from_utf8(state.translate(data).into_owned()).expect("valid UTF-8")
    }

    #[test]
    fn designator_finals_select_the_three_sets() {
        assert_eq!(
            CharacterSet::from_designator_final(b'B'),
            Some(CharacterSet::Ascii)
        );
        assert_eq!(
            CharacterSet::from_designator_final(b'A'),
            Some(CharacterSet::UnitedKingdom)
        );
        assert_eq!(
            CharacterSet::from_designator_final(b'0'),
            Some(CharacterSet::DecSpecialGraphics)
        );
    }

    #[test]
    fn unimplemented_designator_finals_are_unknown() {
        for byte in [b'C', b'5', b'K', b'R', b'<', b'1', b'@', 0x7e] {
            assert_eq!(CharacterSet::from_designator_final(byte), None);
        }
    }

    #[test]
    fn default_is_the_power_on_state() {
        let state = CharsetState::default();
        for slot in [
            CharsetSlot::G0,
            CharsetSlot::G1,
            CharsetSlot::G2,
            CharsetSlot::G3,
        ] {
            assert_eq!(state.set(slot), CharacterSet::Ascii);
        }
        assert_eq!(state.gl(), CharsetSlot::G0);
        assert_eq!(state.gr(), CharsetSlot::G2);
        assert_eq!(state.single_shift(), None);
    }

    #[test]
    fn ascii_translation_borrows_the_input() {
        let mut state = CharsetState::default();
        let data = b"hello";
        assert!(matches!(
            state.translate(data),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn uk_maps_only_the_hash() {
        let mut state = CharsetState::default();
        state.designate(CharsetSlot::G0, CharacterSet::UnitedKingdom);
        assert_eq!(translated(&mut state, b"a#b$"), "a\u{a3}b$");
    }

    #[test]
    fn dec_special_maps_the_whole_table_range_only() {
        let mut state = CharsetState::default();
        state.designate(CharsetSlot::G0, CharacterSet::DecSpecialGraphics);
        assert_eq!(translated(&mut state, b"^"), "^");
        assert_eq!(translated(&mut state, b"_"), "\u{a0}");
        assert_eq!(translated(&mut state, b"lqk"), "\u{250c}\u{2500}\u{2510}");
        assert_eq!(translated(&mut state, b"~"), "\u{b7}");
        assert_eq!(translated(&mut state, b"\x7f"), "\x7f");
    }

    #[test]
    fn invoking_a_slot_into_gl_selects_its_set() {
        let mut state = CharsetState::default();
        state.designate(CharsetSlot::G1, CharacterSet::DecSpecialGraphics);
        assert_eq!(translated(&mut state, b"q"), "q");
        state.invoke_gl(CharsetSlot::G1);
        assert_eq!(translated(&mut state, b"q"), "\u{2500}");
        state.invoke_gl(CharsetSlot::G0);
        assert_eq!(translated(&mut state, b"q"), "q");
    }

    #[test]
    fn gr_is_tracked_but_not_applied() {
        let mut state = CharsetState::default();
        state.designate(CharsetSlot::G1, CharacterSet::DecSpecialGraphics);
        state.invoke_gr(CharsetSlot::G1);
        assert_eq!(state.gr(), CharsetSlot::G1);
        let utf8 = "\u{e9}q";
        assert_eq!(translated(&mut state, utf8.as_bytes()), utf8);
    }

    #[test]
    fn single_shift_maps_exactly_one_printable_character() {
        let mut state = CharsetState::default();
        state.designate(CharsetSlot::G2, CharacterSet::DecSpecialGraphics);
        state.set_single_shift(CharsetSlot::G2);
        assert_eq!(translated(&mut state, b"qqq"), "\u{2500}qq");
        assert_eq!(state.single_shift(), None);
    }

    #[test]
    fn single_shift_survives_a_control_byte() {
        let mut state = CharsetState::default();
        state.designate(CharsetSlot::G3, CharacterSet::DecSpecialGraphics);
        state.set_single_shift(CharsetSlot::G3);
        assert_eq!(translated(&mut state, b"\x01q"), "\x01\u{2500}");
    }

    #[test]
    fn non_ascii_character_consumes_the_single_shift_unmapped() {
        let mut state = CharsetState::default();
        state.designate(CharsetSlot::G2, CharacterSet::DecSpecialGraphics);
        state.set_single_shift(CharsetSlot::G2);
        let data = "\u{e9}q".as_bytes();
        assert_eq!(translated(&mut state, data), "\u{e9}q");
        assert_eq!(state.single_shift(), None);
    }

    #[test]
    fn single_shift_overrides_gl_for_one_character() {
        let mut state = CharsetState::default();
        state.designate(CharsetSlot::G0, CharacterSet::DecSpecialGraphics);
        state.set_single_shift(CharsetSlot::G2);
        assert_eq!(translated(&mut state, b"qq"), "q\u{2500}");
    }
}
