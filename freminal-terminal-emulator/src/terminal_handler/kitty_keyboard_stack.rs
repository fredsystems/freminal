// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The kitty keyboard protocol mode stack for one screen.
//!
//! Programs push flags on entry (`CSI > flags u`), pop on exit
//! (`CSI < n u`), and modify the top entry in place (`CSI = flags ; mode u`).
//! The active flags are the top entry, or `0` when the stack is empty.  The
//! stack is bounded to [`KittyKeyboardFlags::MAX_STACK_DEPTH`] entries.
//!
//! The terminal keeps one of these per screen via
//! [`ScreenScoped`](super::screen_scoped::ScreenScoped).

use conv2::ValueFrom;
use freminal_common::buffer_states::modes::kitty_keyboard::KittyKeyboardFlags;

/// A bounded stack of kitty keyboard flag bitmasks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KittyKeyboardStack {
    entries: Vec<u32>,
}

impl KittyKeyboardStack {
    /// The active flags: the top entry, or `0` when the stack is empty.
    #[must_use]
    pub fn current(&self) -> u32 {
        self.entries.last().copied().unwrap_or(0)
    }

    /// Push `flags`.  At [`KittyKeyboardFlags::MAX_STACK_DEPTH`] entries the
    /// oldest (bottom) entry is evicted first, per the spec.
    pub fn push(&mut self, flags: u32) {
        if self.entries.len() >= KittyKeyboardFlags::MAX_STACK_DEPTH {
            self.entries.remove(0);
        }
        self.entries.push(flags);
    }

    /// Pop `n` entries, saturating at the stack's depth.  Popping every entry
    /// leaves the stack empty, so the active flags revert to `0`.
    pub fn pop(&mut self, n: u32) {
        // u32 → usize is lossless on 32/64-bit Freminal targets.
        let n = usize::value_from(n).unwrap_or(0).min(self.entries.len());
        let new_len = self.entries.len() - n;
        self.entries.truncate(new_len);
    }

    /// Modify the top entry in place (pushing one if the stack is empty).
    ///
    /// `mode` 1 replaces the flags, 2 sets the given bits, 3 clears them.  Any
    /// other mode leaves the active flags unchanged.
    pub fn set(&mut self, flags: u32, mode: u32) {
        let current = self.current();
        let new_flags = match mode {
            1 => flags,
            2 => current | flags,
            3 => current & !flags,
            _ => current,
        };
        if let Some(top) = self.entries.last_mut() {
            *top = new_flags;
        } else {
            self.entries.push(new_flags);
        }
    }

    /// Discard every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const MAX: usize = KittyKeyboardFlags::MAX_STACK_DEPTH;

    #[test]
    fn empty_stack_has_zero_flags() {
        assert_eq!(KittyKeyboardStack::default().current(), 0);
    }

    #[test]
    fn push_makes_flags_current() {
        let mut s = KittyKeyboardStack::default();
        s.push(3);
        assert_eq!(s.current(), 3);
        s.push(5);
        assert_eq!(s.current(), 5);
    }

    #[test]
    fn push_at_cap_evicts_the_bottom_entry() {
        let mut s = KittyKeyboardStack::default();
        for i in 0..=u32::try_from(MAX).unwrap() {
            s.push(i);
        }
        assert_eq!(s.entries.len(), MAX);
        assert_eq!(s.current(), u32::try_from(MAX).unwrap());
        // Entry 0 was evicted; the bottom is now 1.
        assert_eq!(s.entries[0], 1);
    }

    #[test]
    fn pop_removes_entries_and_restores_previous() {
        let mut s = KittyKeyboardStack::default();
        s.push(1);
        s.push(2);
        s.push(4);
        s.pop(2);
        assert_eq!(s.current(), 1);
    }

    #[test]
    fn pop_more_than_depth_empties_the_stack() {
        let mut s = KittyKeyboardStack::default();
        s.push(1);
        s.pop(5);
        assert_eq!(s.current(), 0);
        assert_eq!(s.entries, Vec::<u32>::new());
    }

    #[test]
    fn pop_on_empty_is_a_no_op() {
        let mut s = KittyKeyboardStack::default();
        s.pop(1);
        assert_eq!(s, KittyKeyboardStack::default());
    }

    #[test]
    fn pop_zero_changes_nothing() {
        let mut s = KittyKeyboardStack::default();
        s.push(7);
        s.pop(0);
        assert_eq!(s.current(), 7);
    }

    #[test]
    fn set_mode_1_replaces() {
        let mut s = KittyKeyboardStack::default();
        s.push(3);
        s.set(5, 1);
        assert_eq!(s.current(), 5);
        assert_eq!(s.entries.len(), 1);
    }

    #[test]
    fn set_mode_2_ors() {
        let mut s = KittyKeyboardStack::default();
        s.push(1);
        s.set(2, 2);
        assert_eq!(s.current(), 3);
    }

    #[test]
    fn set_mode_3_clears_bits() {
        let mut s = KittyKeyboardStack::default();
        s.push(7);
        s.set(2, 3);
        assert_eq!(s.current(), 5);
    }

    #[test]
    fn set_unknown_mode_keeps_current() {
        let mut s = KittyKeyboardStack::default();
        s.push(5);
        s.set(0xFF, 99);
        assert_eq!(s.current(), 5);
    }

    #[test]
    fn set_on_empty_stack_pushes() {
        let mut s = KittyKeyboardStack::default();
        s.set(5, 1);
        assert_eq!(s.current(), 5);
        assert_eq!(s.entries.len(), 1);
    }

    #[test]
    fn set_mode_2_on_empty_stack_pushes_the_flags() {
        let mut s = KittyKeyboardStack::default();
        s.set(6, 2);
        assert_eq!(s.current(), 6);
    }

    #[test]
    fn clear_empties_the_stack() {
        let mut s = KittyKeyboardStack::default();
        s.push(1);
        s.push(2);
        s.clear();
        assert_eq!(s.current(), 0);
        assert_eq!(s.entries, Vec::<u32>::new());
    }
}
