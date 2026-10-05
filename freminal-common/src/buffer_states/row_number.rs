// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! [`RowNumber`] — a stable logical row identifier (Task 125.14).
//!
//! A buffer row's *index* is its position in the retained row storage, and it
//! changes whenever rows are evicted from the front. A [`RowNumber`] is the
//! row's identity: assigned when the row is created (`base + index` at that
//! moment) and never changed for the row's lifetime. Anything that must keep
//! pointing at a particular row across eviction -- prompt marks, command-block
//! boundaries, image placements -- stores a
//! `RowNumber` instead of an index, so eviction never has to rewrite it.
//!
//! # Namespaces
//!
//! The primary screen's rows are numbered from `0` upward. The alternate
//! screen uses a disjoint namespace starting at [`RowNumber::ALTERNATE_BASE`]
//! (`1 << 63`), so a mark taken on the primary screen can never alias a row on
//! the alternate screen and vice versa. [`RowNumber::is_alternate`] tells the
//! two apart.
//!
//! # Arithmetic
//!
//! Deliberately explicit: there are **no operator impls**. A row number is an
//! identity, not a quantity, and `a - b` / `a + n` on identities hides which
//! of "distance", "offset" and "index" is meant. Use [`RowNumber::rows_after`]
//! to ask how far a row is above a base, and the named `*_add` / `offset`
//! methods to move.

use std::fmt;

use conv2::ValueFrom;

/// A stable logical row number. See the [module docs](self).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RowNumber(u64);

impl RowNumber {
    /// Row number zero: the first row of the primary screen's namespace.
    pub const ZERO: Self = Self(0);

    /// First row number of the alternate-screen namespace (`1 << 63`).
    pub const ALTERNATE_BASE: Self = Self(1 << 63);

    /// Wrap a raw row number.
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw row number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// `true` if this number lies in the alternate-screen namespace.
    #[must_use]
    pub const fn is_alternate(self) -> bool {
        self.0 >= Self::ALTERNATE_BASE.0
    }

    /// Move by a signed number of rows, or `None` if the result would not be a
    /// row of the **same namespace**.
    ///
    /// Unlike [`Self::offset`] this never saturates: a move that would run
    /// past `0` (primary), below [`Self::ALTERNATE_BASE`] (alternate), past
    /// `u64::MAX`, or from one namespace into the other has no row to land on,
    /// and clamping it would silently land on a different row (row `0`, or a
    /// row of the other screen). Use it where the offset comes from outside
    /// the buffer.
    #[must_use]
    pub fn checked_offset(self, delta: i64) -> Option<Self> {
        let magnitude = delta.unsigned_abs();
        let raw = if delta >= 0 {
            self.0.checked_add(magnitude)?
        } else {
            self.0.checked_sub(magnitude)?
        };
        let moved = Self(raw);
        (moved.is_alternate() == self.is_alternate()).then_some(moved)
    }

    /// The row `rows` after this one, saturating at `u64::MAX`.
    #[must_use]
    pub fn saturating_add(self, rows: usize) -> Self {
        let rows = u64::value_from(rows).unwrap_or(u64::MAX);
        Self(self.0.saturating_add(rows))
    }

    /// How many rows this row lies at or after `base`: `self - base`.
    ///
    /// `None` when the two numbers are in different namespaces (exactly one
    /// of them [`is_alternate`](Self::is_alternate): there is no meaningful
    /// distance between a primary-screen row and an alternate-screen row), when
    /// this row is *before* `base` (it has been evicted), or when the distance
    /// does not fit in a `usize`. This is the single conversion from a number
    /// to a retained index: `number.rows_after(store_base)`.
    #[must_use]
    pub fn rows_after(self, base: Self) -> Option<usize> {
        if self.is_alternate() != base.is_alternate() {
            return None;
        }
        let distance = self.0.checked_sub(base.0)?;
        usize::value_from(distance).ok()
    }

    /// Move by a signed number of rows, saturating at the edges of **this
    /// row's namespace**.
    ///
    /// A primary number saturates within `[0, ALTERNATE_BASE - 1]` and an
    /// alternate number within `[ALTERNATE_BASE, u64::MAX]`, so the result is
    /// always in the same namespace as `self` and the "primary can never alias
    /// alternate" guarantee of the [module docs](self) holds for any `delta`
    /// (an `i64` magnitude reaches `1 << 63`, exactly the namespace width).
    /// Use [`Self::checked_offset`] to learn that a move ran off the edge
    /// instead of silently stopping there.
    #[must_use]
    pub const fn offset(self, delta: i64) -> Self {
        let magnitude = delta.unsigned_abs();
        let (floor, ceiling) = if self.is_alternate() {
            (Self::ALTERNATE_BASE.0, u64::MAX)
        } else {
            (0, Self::ALTERNATE_BASE.0 - 1)
        };
        let raw = if delta >= 0 {
            self.0.saturating_add(magnitude)
        } else {
            self.0.saturating_sub(magnitude)
        };
        // `Ord::clamp` is not `const`.
        if raw < floor {
            Self(floor)
        } else if raw > ceiling {
            Self(ceiling)
        } else {
            Self(raw)
        }
    }
}

impl fmt::Display for RowNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_alternate() {
            write!(f, "alt:{}", self.0 - Self::ALTERNATE_BASE.0)
        } else {
            write!(f, "{}", self.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_and_get_round_trip() {
        assert_eq!(RowNumber::new(0).get(), 0);
        assert_eq!(RowNumber::new(12_345).get(), 12_345);
        assert_eq!(RowNumber::new(u64::MAX).get(), u64::MAX);
    }

    #[test]
    fn zero_constant_is_zero() {
        assert_eq!(RowNumber::ZERO, RowNumber::new(0));
    }

    #[test]
    fn alternate_base_is_one_shl_63() {
        assert_eq!(RowNumber::ALTERNATE_BASE.get(), 1u64 << 63);
    }

    #[test]
    fn ordering_follows_the_raw_number() {
        assert!(RowNumber::new(1) < RowNumber::new(2));
        assert!(RowNumber::new(5) > RowNumber::new(4));
        assert_eq!(RowNumber::new(7), RowNumber::new(7));
        assert!(RowNumber::new(u64::MAX) > RowNumber::ALTERNATE_BASE);
    }

    #[test]
    fn every_primary_number_sorts_below_every_alternate_number() {
        let top_primary = RowNumber::new((1u64 << 63) - 1);
        assert!(top_primary < RowNumber::ALTERNATE_BASE);
        assert!(!top_primary.is_alternate());
    }

    #[test]
    fn is_alternate_boundaries() {
        assert!(!RowNumber::ZERO.is_alternate());
        assert!(!RowNumber::new(1).is_alternate());
        assert!(!RowNumber::new((1u64 << 63) - 1).is_alternate());
        assert!(RowNumber::ALTERNATE_BASE.is_alternate());
        assert!(RowNumber::new((1u64 << 63) + 1).is_alternate());
        assert!(RowNumber::new(u64::MAX).is_alternate());
    }

    #[test]
    fn hash_agrees_with_eq() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(RowNumber::new(3));
        set.insert(RowNumber::new(3));
        set.insert(RowNumber::new(4));
        assert_eq!(set.len(), 2);
        assert!(set.contains(&RowNumber::new(4)));
    }

    #[test]
    fn checked_offset_moves_both_ways() {
        let n = RowNumber::new(10);
        assert_eq!(n.checked_offset(0), Some(n));
        assert_eq!(n.checked_offset(5), Some(RowNumber::new(15)));
        assert_eq!(n.checked_offset(-4), Some(RowNumber::new(6)));
        assert_eq!(n.checked_offset(-10), Some(RowNumber::ZERO));
    }

    #[test]
    fn checked_offset_refuses_to_run_past_row_zero() {
        assert_eq!(RowNumber::new(10).checked_offset(-11), None);
        assert_eq!(RowNumber::ZERO.checked_offset(-1), None);
        assert_eq!(RowNumber::ZERO.checked_offset(i64::MIN), None);
    }

    #[test]
    fn checked_offset_refuses_to_leave_the_alternate_namespace() {
        let alt = RowNumber::ALTERNATE_BASE.saturating_add(2);
        assert_eq!(
            alt.checked_offset(-2),
            Some(RowNumber::ALTERNATE_BASE),
            "the first alternate row is still alternate"
        );
        assert_eq!(
            alt.checked_offset(-3),
            None,
            "one row further would be the last primary number"
        );
        assert_eq!(RowNumber::ALTERNATE_BASE.checked_offset(-1), None);
    }

    #[test]
    fn checked_offset_refuses_to_enter_the_alternate_namespace() {
        let top_primary = RowNumber::new((1u64 << 63) - 1);
        assert_eq!(top_primary.checked_offset(0), Some(top_primary));
        assert_eq!(top_primary.checked_offset(1), None);
        assert_eq!(RowNumber::new(5).checked_offset(i64::MAX), None);
    }

    #[test]
    fn checked_offset_reports_u64_overflow() {
        assert_eq!(RowNumber::new(u64::MAX).checked_offset(1), None);
        assert_eq!(
            RowNumber::new(u64::MAX - 1).checked_offset(1),
            Some(RowNumber::new(u64::MAX))
        );
    }

    #[test]
    fn saturating_add_clamps_at_max() {
        assert_eq!(
            RowNumber::new(10).saturating_add(5),
            RowNumber::new(15),
            "ordinary addition"
        );
        assert_eq!(
            RowNumber::new(u64::MAX).saturating_add(1),
            RowNumber::new(u64::MAX)
        );
        assert_eq!(
            RowNumber::new(u64::MAX - 1).saturating_add(100),
            RowNumber::new(u64::MAX)
        );
    }

    #[test]
    fn rows_after_is_distance_above_base() {
        let base = RowNumber::new(100);
        assert_eq!(RowNumber::new(100).rows_after(base), Some(0));
        assert_eq!(RowNumber::new(101).rows_after(base), Some(1));
        assert_eq!(RowNumber::new(150).rows_after(base), Some(50));
    }

    #[test]
    fn rows_after_is_none_below_base() {
        let base = RowNumber::new(100);
        assert_eq!(RowNumber::new(99).rows_after(base), None);
        assert_eq!(RowNumber::new(0).rows_after(base), None);
    }

    #[test]
    fn rows_after_is_none_across_namespaces_in_both_directions() {
        // A primary number measured against an alternate base.
        assert_eq!(
            RowNumber::new(5).rows_after(RowNumber::ALTERNATE_BASE),
            None
        );
        // An alternate number measured against a primary base: not a huge
        // distance, no distance at all.
        let alt = RowNumber::ALTERNATE_BASE.saturating_add(3);
        assert_eq!(alt.rows_after(RowNumber::ZERO), None);
        assert_eq!(alt.rows_after(RowNumber::new(1_000)), None);
        // Within the alternate namespace it still measures.
        assert_eq!(alt.rows_after(RowNumber::ALTERNATE_BASE), Some(3));
    }

    #[test]
    fn rows_after_round_trips_saturating_add() {
        let base = RowNumber::new(42);
        let n = base.saturating_add(17);
        assert_eq!(n.rows_after(base), Some(17));
    }

    #[test]
    fn offset_positive_and_negative() {
        let n = RowNumber::new(10);
        assert_eq!(n.offset(0), n);
        assert_eq!(n.offset(5), RowNumber::new(15));
        assert_eq!(n.offset(-4), RowNumber::new(6));
        assert_eq!(n.offset(-10), RowNumber::ZERO);
    }

    #[test]
    fn offset_saturates_at_both_ends_of_the_primary_namespace() {
        let top_primary = RowNumber::new((1u64 << 63) - 1);
        assert_eq!(RowNumber::new(3).offset(-100), RowNumber::ZERO);
        assert_eq!(RowNumber::ZERO.offset(i64::MIN), RowNumber::ZERO);
        assert_eq!(top_primary.offset(1), top_primary);
        assert_eq!(RowNumber::new(5).offset(i64::MAX), top_primary);
        assert!(!RowNumber::new(5).offset(i64::MAX).is_alternate());
    }

    #[test]
    fn offset_saturates_at_both_ends_of_the_alternate_namespace() {
        let alt = RowNumber::ALTERNATE_BASE;
        assert_eq!(alt.offset(-1), alt);
        assert_eq!(alt.saturating_add(3).offset(-100), alt);
        assert_eq!(
            RowNumber::new(u64::MAX - 1).offset(100),
            RowNumber::new(u64::MAX)
        );
        assert_eq!(
            RowNumber::new(u64::MAX).offset(i64::MAX),
            RowNumber::new(u64::MAX)
        );
    }

    #[test]
    fn offset_handles_i64_min_without_overflow() {
        // `i64::MIN.unsigned_abs()` is 2^63; a plain `-delta` would overflow.
        // From the top of the alternate namespace that magnitude would land on
        // `u64::MAX - 2^63`, a PRIMARY number; it must clamp to the
        // alternate namespace's first row instead.
        let n = RowNumber::new(u64::MAX);
        assert_eq!(n.offset(i64::MIN), RowNumber::ALTERNATE_BASE);
        assert!(n.offset(i64::MIN).is_alternate());
    }

    #[test]
    fn offset_never_changes_namespace() {
        for start in [
            RowNumber::ZERO,
            RowNumber::new(7),
            RowNumber::new((1u64 << 63) - 1),
            RowNumber::ALTERNATE_BASE,
            RowNumber::ALTERNATE_BASE.saturating_add(7),
            RowNumber::new(u64::MAX),
        ] {
            for delta in [i64::MIN, -1, 0, 1, i64::MAX] {
                assert_eq!(
                    start.offset(delta).is_alternate(),
                    start.is_alternate(),
                    "{start}.offset({delta}) crossed namespaces"
                );
            }
        }
    }

    #[test]
    fn offset_agrees_with_checked_offset_whenever_that_succeeds() {
        for start in [
            RowNumber::new(10),
            RowNumber::ALTERNATE_BASE.saturating_add(10),
        ] {
            for delta in [-5, 0, 5] {
                assert_eq!(start.checked_offset(delta), Some(start.offset(delta)));
            }
        }
    }

    #[test]
    fn display_marks_alternate_numbers() {
        assert_eq!(RowNumber::new(42).to_string(), "42");
        assert_eq!(RowNumber::ALTERNATE_BASE.to_string(), "alt:0");
        assert_eq!(
            RowNumber::ALTERNATE_BASE.saturating_add(9).to_string(),
            "alt:9"
        );
    }
}
