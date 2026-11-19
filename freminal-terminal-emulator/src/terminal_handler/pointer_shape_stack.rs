// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The kitty pointer-shape stack for one screen (OSC 22).
//!
//! See <https://sw.kovidgoyal.net/kitty/pointer-shapes/>.  A program sets the
//! top entry (`OSC 22 ; = name ST`, or no prefix), pushes a shape on entry
//! (`>`) and pops it on exit (`<`).  The active shape is the top entry; an
//! empty stack, or a [`PointerShapeEntry::Unset`] top, means the default
//! shape.
//!
//! Following kitty's reference implementation:
//!
//! - a set replaces the top entry (an empty stack first grows to one entry)
//!   and never pushes or clears, so entries below it survive;
//! - a push onto a full stack ([`POINTER_SHAPE_STACK_DEPTH`] entries, the
//!   spec minimum) evicts the bottom entry;
//! - a pop removes exactly one entry and does nothing on an empty stack.
//!
//! The terminal keeps one of these per screen via
//! [`ScreenScoped`](super::screen_scoped::ScreenScoped).

use freminal_common::buffer_states::pointer_shape::{PointerShape, PointerShapeEntry};

/// The capacity of a [`PointerShapeStack`]: the spec minimum and kitty's value.
pub const POINTER_SHAPE_STACK_DEPTH: usize = 16;

/// A bounded stack of pointer-shape entries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PointerShapeStack {
    entries: Vec<PointerShapeEntry>,
}

impl PointerShapeStack {
    /// The active shape: the top entry's shape, or `None` for an empty stack
    /// or a [`PointerShapeEntry::Unset`] top.
    #[must_use]
    pub fn current(&self) -> Option<PointerShape> {
        match self.entries.last() {
            Some(PointerShapeEntry::Shape(shape)) => Some(*shape),
            Some(PointerShapeEntry::Unset) | None => None,
        }
    }

    /// Replace the top entry with `entry`, pushing it if the stack is empty.
    /// Lower entries are never touched.
    pub fn set(&mut self, entry: PointerShapeEntry) {
        if let Some(top) = self.entries.last_mut() {
            *top = entry;
        } else {
            self.entries.push(entry);
        }
    }

    /// Push `shape`.  At [`POINTER_SHAPE_STACK_DEPTH`] entries the oldest
    /// (bottom) entry is evicted first.
    pub fn push(&mut self, shape: PointerShape) {
        if self.entries.len() >= POINTER_SHAPE_STACK_DEPTH {
            self.entries.remove(0);
        }
        self.entries.push(PointerShapeEntry::Shape(shape));
    }

    /// Remove the top entry.  Does nothing on an empty stack.
    pub fn pop(&mut self) {
        self.entries.pop();
    }

    /// Discard every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn shape(shape: PointerShape) -> PointerShapeEntry {
        PointerShapeEntry::Shape(shape)
    }

    #[test]
    fn empty_stack_has_no_current_shape() {
        assert_eq!(PointerShapeStack::default().current(), None);
    }

    #[test]
    fn set_on_empty_stack_grows_it_to_one_entry() {
        let mut s = PointerShapeStack::default();
        s.set(shape(PointerShape::Text));
        assert_eq!(s.current(), Some(PointerShape::Text));
        assert_eq!(s.entries, vec![shape(PointerShape::Text)]);
    }

    #[test]
    fn set_replaces_the_top_and_keeps_lower_entries() {
        let mut s = PointerShapeStack::default();
        s.push(PointerShape::Help);
        s.push(PointerShape::Wait);
        s.set(shape(PointerShape::Crosshair));
        assert_eq!(s.current(), Some(PointerShape::Crosshair));
        assert_eq!(
            s.entries,
            vec![shape(PointerShape::Help), shape(PointerShape::Crosshair)]
        );
    }

    #[test]
    fn push_order_makes_the_last_push_current() {
        let mut s = PointerShapeStack::default();
        s.push(PointerShape::Text);
        assert_eq!(s.current(), Some(PointerShape::Text));
        s.push(PointerShape::Pointer);
        assert_eq!(s.current(), Some(PointerShape::Pointer));
        assert_eq!(
            s.entries,
            vec![shape(PointerShape::Text), shape(PointerShape::Pointer)]
        );
    }

    #[test]
    fn push_at_capacity_evicts_the_bottom_entry() {
        const SHAPES: [PointerShape; 17] = [
            PointerShape::Text,
            PointerShape::Pointer,
            PointerShape::Help,
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
            PointerShape::ZoomIn,
            PointerShape::ZoomOut,
            PointerShape::ResizeEast,
            PointerShape::ResizeWest,
        ];
        let mut s = PointerShapeStack::default();
        for pushed in SHAPES {
            s.push(pushed);
        }
        assert_eq!(POINTER_SHAPE_STACK_DEPTH, 16);
        assert_eq!(s.entries.len(), POINTER_SHAPE_STACK_DEPTH);
        let expected: Vec<PointerShapeEntry> = SHAPES[1..].iter().copied().map(shape).collect();
        assert_eq!(s.entries, expected);
        assert_eq!(s.current(), Some(PointerShape::ResizeWest));
    }

    #[test]
    fn sixteen_pushes_evict_nothing() {
        let mut s = PointerShapeStack::default();
        s.push(PointerShape::Help);
        for _ in 1..POINTER_SHAPE_STACK_DEPTH {
            s.push(PointerShape::Text);
        }
        assert_eq!(s.entries.len(), POINTER_SHAPE_STACK_DEPTH);
        assert_eq!(s.entries[0], shape(PointerShape::Help));
    }

    #[test]
    fn pop_restores_the_previous_shape_and_empties_the_stack() {
        let mut s = PointerShapeStack::default();
        s.push(PointerShape::Text);
        s.push(PointerShape::Pointer);
        s.pop();
        assert_eq!(s.current(), Some(PointerShape::Text));
        s.pop();
        assert_eq!(s.current(), None);
        assert_eq!(s.entries, Vec::<PointerShapeEntry>::new());
    }

    #[test]
    fn pop_on_empty_is_a_no_op() {
        let mut s = PointerShapeStack::default();
        s.pop();
        assert_eq!(s, PointerShapeStack::default());
    }

    #[test]
    fn unset_top_hides_the_lower_shape_until_popped() {
        let mut s = PointerShapeStack::default();
        s.push(PointerShape::Help);
        s.push(PointerShape::Wait);
        s.set(PointerShapeEntry::Unset);
        assert_eq!(s.current(), None);
        assert_eq!(
            s.entries,
            vec![shape(PointerShape::Help), PointerShapeEntry::Unset]
        );
        s.pop();
        assert_eq!(s.current(), Some(PointerShape::Help));
    }

    #[test]
    fn unset_on_empty_stack_is_stored() {
        let mut s = PointerShapeStack::default();
        s.set(PointerShapeEntry::Unset);
        assert_eq!(s.current(), None);
        assert_eq!(s.entries, vec![PointerShapeEntry::Unset]);
    }

    #[test]
    fn clear_empties_the_stack() {
        let mut s = PointerShapeStack::default();
        s.push(PointerShape::Text);
        s.push(PointerShape::Pointer);
        s.clear();
        assert_eq!(s.current(), None);
        assert_eq!(s.entries, Vec::<PointerShapeEntry>::new());
    }
}
