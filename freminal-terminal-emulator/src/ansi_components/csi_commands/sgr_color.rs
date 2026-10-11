// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Shared extractor for SGR-style colour specifications.
//!
//! SGR `38` (foreground), `48` (background) and `58` (underline colour) are
//! followed by a colour type and its values, either as further
//! semicolon-separated parameters (`38;2;R;G;B`, `38;5;N`) or as colon
//! sub-parameters (`38:2::R:G:B`, `38:5:N`). [`parse_sgr_color`] turns the
//! values after the `38`/`48`/`58` into a [`TerminalColor`].
//!
//! This is the single extractor shared by SGR and by every later consumer of
//! the same wire syntax (DECCARA, Task 143; multiple cursors, Task 103), so
//! that all of them agree on edge cases.
//!
//! The edge-case semantics follow kitty (`vt-parser.c` `_parse_sgr` and
//! `cursor.c` `parse_color`), because ECMA-48 and ITU T.416 are silent on
//! them:
//!
//! - indexed colours (`5`) take one value, masked to 8 bits;
//! - direct colours (`2`) take `R;G;B` in semicolon form; in colon form, if
//!   four or more values follow the `2` the first is a colour-space id and is
//!   ignored; components are masked to 8 bits;
//! - missing or unparsable values count as 0;
//! - an incomplete colour is an error and produces no colour;
//! - a colour type other than `2`/`5` is an error the caller treats as fatal
//!   for the rest of the sequence.

use freminal_common::colors::TerminalColor;

/// Which wire syntax the colour values were written in.
///
/// The two forms differ for direct colour: only the colon form may carry a
/// colour-space id between the `2` and the `R:G:B` components.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SgrColorForm {
    /// Values were separate semicolon-delimited parameters (`38;2;R;G;B`).
    Semicolon,
    /// Values were colon sub-parameters of one parameter (`38:2:CS:R:G:B`).
    Colon,
}

/// A successfully extracted colour and how much input it used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SgrColor {
    /// The colour: always [`TerminalColor::PaletteIndex`] or
    /// [`TerminalColor::Custom`].
    pub color: TerminalColor,
    /// Number of elements of the input slice used, including the leading
    /// colour-type element.
    pub consumed: usize,
}

/// Why a colour specification could not be extracted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SgrColorError {
    /// The colour type is known but fewer values than it needs were present.
    Incomplete,
    /// The colour type is neither `2` (direct) nor `5` (indexed). Carries the
    /// type value (an unparsable type is reported as `0`).
    UnknownType(usize),
}

/// Narrow a colour value to its low 8 bits. Missing values count as 0.
fn low_byte(value: Option<usize>) -> u8 {
    u8::try_from(value.unwrap_or(0) & 0xFF).unwrap_or(0)
}

/// Extract a colour from the values following an SGR `38`, `48` or `58`.
///
/// `params` starts at the colour-type element (the element after the
/// `38`/`48`/`58`). `None` entries are empty or unparsable values and count
/// as 0. Elements past the ones the colour needs are ignored and are not
/// counted in [`SgrColor::consumed`].
///
/// This is the shared extractor for SGR and for future consumers of the same
/// syntax (DECCARA, Task 143; multiple cursors, Task 103).
///
/// # Errors
///
/// - [`SgrColorError::Incomplete`] if `params` is empty or has fewer values
///   than the colour type needs.
/// - [`SgrColorError::UnknownType`] if the colour type is not `2` or `5`.
pub(crate) fn parse_sgr_color(
    params: &[Option<usize>],
    form: SgrColorForm,
) -> Result<SgrColor, SgrColorError> {
    let Some(kind) = params.first().copied() else {
        return Err(SgrColorError::Incomplete);
    };
    let values = params.get(1..).unwrap_or_default();

    match kind.unwrap_or(0) {
        5 => {
            let index = values.first().ok_or(SgrColorError::Incomplete)?;
            Ok(SgrColor {
                color: TerminalColor::PaletteIndex(low_byte(*index)),
                consumed: 2,
            })
        }
        2 => {
            // Colon form with four or more values: the first is a colour-space
            // id, which is skipped. Semicolon form has no colour-space id.
            let skip = usize::from(form == SgrColorForm::Colon && values.len() >= 4);
            let (r, g, b) = match values.get(skip..skip + 3) {
                Some([r, g, b]) => (*r, *g, *b),
                _ => return Err(SgrColorError::Incomplete),
            };
            Ok(SgrColor {
                color: TerminalColor::Custom(low_byte(r), low_byte(g), low_byte(b)),
                consumed: 1 + skip + 3,
            })
        }
        other => Err(SgrColorError::UnknownType(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected successful extraction.
    macro_rules! ok {
        ($color:expr, $consumed:expr) => {
            Ok(SgrColor {
                color: $color,
                consumed: $consumed,
            })
        };
    }

    fn semi(params: &[Option<usize>]) -> Result<SgrColor, SgrColorError> {
        parse_sgr_color(params, SgrColorForm::Semicolon)
    }

    fn colon(params: &[Option<usize>]) -> Result<SgrColor, SgrColorError> {
        parse_sgr_color(params, SgrColorForm::Colon)
    }

    #[test]
    fn indexed_colour() {
        assert_eq!(
            semi(&[Some(5), Some(42)]),
            ok!(TerminalColor::PaletteIndex(42), 2)
        );
        assert_eq!(
            colon(&[Some(5), Some(42)]),
            ok!(TerminalColor::PaletteIndex(42), 2)
        );
    }

    #[test]
    fn indexed_colour_is_masked_to_8_bits() {
        assert_eq!(
            semi(&[Some(5), Some(256)]),
            ok!(TerminalColor::PaletteIndex(0), 2)
        );
        assert_eq!(
            semi(&[Some(5), Some(300)]),
            ok!(TerminalColor::PaletteIndex(44), 2)
        );
        assert_eq!(
            colon(&[Some(5), Some(300)]),
            ok!(TerminalColor::PaletteIndex(44), 2)
        );
    }

    #[test]
    fn indexed_colour_missing_value_is_incomplete() {
        assert_eq!(semi(&[Some(5)]), Err(SgrColorError::Incomplete));
        assert_eq!(colon(&[Some(5)]), Err(SgrColorError::Incomplete));
    }

    #[test]
    fn indexed_colour_unparsable_value_counts_as_zero() {
        assert_eq!(
            semi(&[Some(5), None]),
            ok!(TerminalColor::PaletteIndex(0), 2)
        );
        assert_eq!(
            colon(&[Some(5), None]),
            ok!(TerminalColor::PaletteIndex(0), 2)
        );
    }

    #[test]
    fn empty_input_is_incomplete() {
        assert_eq!(semi(&[]), Err(SgrColorError::Incomplete));
        assert_eq!(colon(&[]), Err(SgrColorError::Incomplete));
    }

    #[test]
    fn direct_colour_semicolon_form() {
        assert_eq!(
            semi(&[Some(2), Some(1), Some(2), Some(3)]),
            ok!(TerminalColor::Custom(1, 2, 3), 4)
        );
    }

    #[test]
    fn direct_colour_semicolon_form_has_no_colourspace_id() {
        // A fourth value after r;g;b is not a colour-space id and is left
        // for the caller.
        assert_eq!(
            semi(&[Some(2), Some(1), Some(2), Some(3), Some(4)]),
            ok!(TerminalColor::Custom(1, 2, 3), 4)
        );
    }

    #[test]
    fn direct_colour_too_few_values_is_incomplete() {
        assert_eq!(
            semi(&[Some(2), Some(1), Some(2)]),
            Err(SgrColorError::Incomplete)
        );
        assert_eq!(semi(&[Some(2)]), Err(SgrColorError::Incomplete));
        assert_eq!(
            colon(&[Some(2), Some(1), Some(2)]),
            Err(SgrColorError::Incomplete)
        );
        assert_eq!(colon(&[Some(2)]), Err(SgrColorError::Incomplete));
    }

    #[test]
    fn direct_colour_components_are_masked_to_8_bits() {
        assert_eq!(
            semi(&[Some(2), Some(300), Some(0), Some(0)]),
            ok!(TerminalColor::Custom(44, 0, 0), 4)
        );
        assert_eq!(
            colon(&[Some(2), Some(300), Some(256), Some(511)]),
            ok!(TerminalColor::Custom(44, 0, 255), 4)
        );
    }

    #[test]
    fn direct_colour_unparsable_components_count_as_zero() {
        assert_eq!(
            semi(&[Some(2), None, Some(5), None]),
            ok!(TerminalColor::Custom(0, 5, 0), 4)
        );
    }

    #[test]
    fn direct_colour_colon_form_without_colourspace() {
        assert_eq!(
            colon(&[Some(2), Some(1), Some(2), Some(3)]),
            ok!(TerminalColor::Custom(1, 2, 3), 4)
        );
    }

    #[test]
    fn direct_colour_colon_form_with_empty_colourspace() {
        assert_eq!(
            colon(&[Some(2), None, Some(1), Some(2), Some(3)]),
            ok!(TerminalColor::Custom(1, 2, 3), 5)
        );
    }

    #[test]
    fn direct_colour_colon_form_with_colourspace_id() {
        assert_eq!(
            colon(&[Some(2), Some(9), Some(1), Some(2), Some(3)]),
            ok!(TerminalColor::Custom(1, 2, 3), 5)
        );
    }

    #[test]
    fn direct_colour_colon_form_with_four_values_skips_first() {
        // Four values after the `2`: the first is taken as a colour-space id.
        assert_eq!(
            colon(&[Some(2), Some(1), Some(2), Some(3), Some(4)]),
            ok!(TerminalColor::Custom(2, 3, 4), 5)
        );
    }

    #[test]
    fn unknown_type_is_reported() {
        assert_eq!(
            semi(&[Some(4), Some(1)]),
            Err(SgrColorError::UnknownType(4))
        );
        assert_eq!(
            colon(&[Some(4), Some(1)]),
            Err(SgrColorError::UnknownType(4))
        );
        assert_eq!(semi(&[Some(0)]), Err(SgrColorError::UnknownType(0)));
    }

    #[test]
    fn unparsable_type_is_unknown_type_zero() {
        assert_eq!(semi(&[None]), Err(SgrColorError::UnknownType(0)));
        assert_eq!(colon(&[None]), Err(SgrColorError::UnknownType(0)));
    }
}
