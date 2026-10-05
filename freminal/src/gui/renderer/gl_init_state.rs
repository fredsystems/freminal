// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Where a renderer is in its GL-object lifecycle (Task 125.C16).
//!
//! One concept: whether a renderer's `init` has run, succeeded, or failed.
//! A failed `init` is **latched** so the paint callbacks that lazily
//! initialise a renderer do not retry -- and re-log -- a persistently failing
//! `init` on every frame.

/// The initialisation state of a GL renderer.
///
/// This is the *attempt* history, not the object inventory: whether a
/// renderer currently owns GL objects is answered by its own
/// `holds_gl_objects()`, because a failed `init` releases what it created and
/// a retired renderer must be deleted by what it holds, not by what it once
/// reported.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GlInitState {
    /// `init` has not run (or `destroy` reset the renderer): the next paint
    /// should attempt it.
    #[default]
    Uninitialized,
    /// `init` succeeded: every pass's GL objects exist.
    Ready,
    /// `init` failed and released what it had created. Callers do not retry
    /// automatically; an explicit `init` still can, and `destroy` clears the
    /// latch.
    Failed,
}

impl GlInitState {
    /// Whether drawing is possible.
    #[must_use]
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// Whether a lazy caller should attempt `init` now: only before the first
    /// attempt, never after a latched failure.
    #[must_use]
    pub const fn should_attempt_init(self) -> bool {
        matches!(self, Self::Uninitialized)
    }
}

#[cfg(test)]
mod tests {
    use super::GlInitState;

    #[test]
    fn default_is_uninitialized_and_attempts_init() {
        let state = GlInitState::default();
        assert_eq!(state, GlInitState::Uninitialized);
        assert!(state.should_attempt_init());
        assert!(!state.is_ready());
    }

    #[test]
    fn ready_draws_and_does_not_reinit() {
        assert!(GlInitState::Ready.is_ready());
        assert!(!GlInitState::Ready.should_attempt_init());
    }

    #[test]
    fn failed_is_latched_neither_drawing_nor_retrying() {
        assert!(!GlInitState::Failed.is_ready());
        assert!(!GlInitState::Failed.should_attempt_init());
    }
}
