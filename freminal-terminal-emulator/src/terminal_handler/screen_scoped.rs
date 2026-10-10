// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Per-screen state indexed by the buffer's active screen.
//!
//! [`ScreenScoped`] holds one value for the primary screen and one for the
//! alternate screen.  It deliberately has no notion of an "active" screen: the
//! caller supplies the screen on every access (normally `buffer.kind()`).  The
//! buffer is therefore the single source of truth for which screen is active,
//! and this state cannot drift from it.  That also makes switching screens
//! idempotent by construction -- there is nothing to swap, so entering the
//! alternate screen twice, or leaving it twice, cannot corrupt anything.

use freminal_common::buffer_states::buffer_type::BufferType;

/// One value of `T` per screen, indexed by [`BufferType`].
// TODO(131.3): consumed by the kitty keyboard stack
#[cfg_attr(not(test), expect(dead_code))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenScoped<T> {
    primary: T,
    alternate: T,
}

// TODO(131.3): consumed by the kitty keyboard stack
#[cfg_attr(not(test), expect(dead_code))]
impl<T> ScreenScoped<T> {
    /// Build from the primary-screen and alternate-screen values.
    #[must_use]
    pub const fn new(primary: T, alternate: T) -> Self {
        Self { primary, alternate }
    }

    /// Borrow the value for `screen`.
    #[must_use]
    pub const fn get(&self, screen: BufferType) -> &T {
        match screen {
            BufferType::Primary => &self.primary,
            BufferType::Alternate => &self.alternate,
        }
    }

    /// Mutably borrow the value for `screen`.
    pub const fn get_mut(&mut self, screen: BufferType) -> &mut T {
        match screen {
            BufferType::Primary => &mut self.primary,
            BufferType::Alternate => &mut self.alternate,
        }
    }

    /// Mutably borrow both values, primary first.  Used by resets that apply
    /// to every screen.
    pub const fn both_mut(&mut self) -> [&mut T; 2] {
        [&mut self.primary, &mut self.alternate]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_indexes_the_right_slot() {
        let s = ScreenScoped::new(1, 2);
        assert_eq!(*s.get(BufferType::Primary), 1);
        assert_eq!(*s.get(BufferType::Alternate), 2);
    }

    #[test]
    fn get_mut_indexes_the_right_slot() {
        let mut s = ScreenScoped::new(1, 2);
        *s.get_mut(BufferType::Primary) = 10;
        assert_eq!(*s.get(BufferType::Primary), 10);
        assert_eq!(*s.get(BufferType::Alternate), 2);
        *s.get_mut(BufferType::Alternate) = 20;
        assert_eq!(*s.get(BufferType::Primary), 10);
        assert_eq!(*s.get(BufferType::Alternate), 20);
    }

    #[test]
    fn mutating_one_slot_does_not_affect_the_other() {
        let mut s = ScreenScoped::new(vec![1], vec![2]);
        s.get_mut(BufferType::Alternate).push(3);
        assert_eq!(s.get(BufferType::Primary), &vec![1]);
        assert_eq!(s.get(BufferType::Alternate), &vec![2, 3]);
    }

    #[test]
    fn both_mut_returns_primary_first() {
        let mut s = ScreenScoped::new(1, 2);
        let [primary, alternate] = s.both_mut();
        assert_eq!(*primary, 1);
        assert_eq!(*alternate, 2);
        *primary = 100;
        *alternate = 200;
        assert_eq!(*s.get(BufferType::Primary), 100);
        assert_eq!(*s.get(BufferType::Alternate), 200);
    }

    #[test]
    fn default_gives_defaults_in_both() {
        let s: ScreenScoped<u32> = ScreenScoped::default();
        assert_eq!(*s.get(BufferType::Primary), 0);
        assert_eq!(*s.get(BufferType::Alternate), 0);
        let s: ScreenScoped<String> = ScreenScoped::default();
        assert_eq!(s.get(BufferType::Primary), "");
        assert_eq!(s.get(BufferType::Alternate), "");
    }
}
