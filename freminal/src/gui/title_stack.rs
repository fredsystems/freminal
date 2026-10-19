// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Bounded window-title stack for `CSI 22 ; Ps t` / `CSI 23 ; Ps t`.
//!
//! A terminal application may push the current title any number of times,
//! and each title can be up to the OSC payload limit.  An unbounded stack is
//! therefore a memory-exhaustion vector (GHSA-jh4m-7w5g-j4qg), so the stack
//! has a fixed maximum depth and drops the **oldest** entry when full.
//!
//! The depth and overflow policy follow the reference implementations:
//!
//! - xterm: `ptyx.h` defines `MAX_SAVED_TITLES 10`, and `misc.c`
//!   `xtermPushTitle` stores titles in a ring buffer -- once 10 are stored,
//!   a push overwrites the oldest slot.
//! - kitty: `window.py` uses `self.title_stack = deque(maxlen=10)`, where
//!   `append` on a full deque drops the oldest entry.

use std::collections::VecDeque;

/// Maximum number of titles retained on the stack (xterm `MAX_SAVED_TITLES`,
/// kitty `deque(maxlen=10)`).
pub const MAX_TITLE_STACK_DEPTH: usize = 10;

/// A LIFO stack of saved window titles, bounded to
/// [`MAX_TITLE_STACK_DEPTH`] entries.
///
/// Pushing onto a full stack discards the oldest entry, so the stack always
/// holds the most recent [`MAX_TITLE_STACK_DEPTH`] titles.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TitleStack {
    titles: VecDeque<String>,
}

impl TitleStack {
    /// Create an empty stack.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            titles: VecDeque::new(),
        }
    }

    /// Push `title`, dropping the oldest entry if the stack is full.
    pub fn push(&mut self, title: String) {
        if self.titles.len() >= MAX_TITLE_STACK_DEPTH {
            self.titles.pop_front();
        }
        self.titles.push_back(title);
    }

    /// Pop the most recently pushed title, if any.
    pub fn pop(&mut self) -> Option<String> {
        self.titles.pop_back()
    }

    /// Number of titles currently stored (never exceeds
    /// [`MAX_TITLE_STACK_DEPTH`]).
    #[must_use]
    pub fn len(&self) -> usize {
        self.titles.len()
    }

    /// `true` if no titles are stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.titles.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_stack_is_empty() {
        let mut stack = TitleStack::new();
        assert!(stack.is_empty());
        assert_eq!(stack.len(), 0);
        assert_eq!(stack.pop(), None);
    }

    #[test]
    fn push_pop_is_lifo_below_the_limit() {
        let mut stack = TitleStack::default();
        stack.push("a".to_owned());
        stack.push("b".to_owned());
        assert_eq!(stack.len(), 2);
        assert!(!stack.is_empty());
        assert_eq!(stack.pop().as_deref(), Some("b"));
        assert_eq!(stack.pop().as_deref(), Some("a"));
        assert_eq!(stack.pop(), None);
    }

    #[test]
    fn thousands_of_pushes_stay_at_the_depth_limit() {
        let mut stack = TitleStack::new();
        for i in 0..5000 {
            stack.push(format!("title-{i}"));
            assert!(stack.len() <= MAX_TITLE_STACK_DEPTH);
        }
        assert_eq!(stack.len(), MAX_TITLE_STACK_DEPTH);
    }

    #[test]
    fn overflow_keeps_the_newest_titles_in_lifo_order() {
        let mut stack = TitleStack::new();
        for i in 0..5000 {
            stack.push(format!("title-{i}"));
        }
        for i in (4990..5000).rev() {
            assert_eq!(stack.pop(), Some(format!("title-{i}")));
        }
        assert_eq!(stack.pop(), None);
        assert!(stack.is_empty());
    }
}
