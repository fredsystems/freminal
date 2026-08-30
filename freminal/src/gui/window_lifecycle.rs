// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Window-lifecycle requests raised during a frame (issue #512).
//!
//! `PerWindowState` used to carry two independent `bool` fields,
//! `pending_new_window` and `pending_quit_all`, each written from two call
//! sites — the Freminal menu (`gui/menu.rs`) and the key-binding
//! deferred-action dispatcher (`gui/actions.rs`) — and drained from a third,
//! `FreminalGui::update()` (`gui/app_impl.rs`).
//!
//! Issue #512 was exactly this fan-in/fan-out shape going wrong: the drain
//! used to sit near the top of `update()`, upstream of both places that set
//! the flags for the *current* frame. A flag set later in the same
//! `update()` call could therefore never be observed until some *other*
//! event happened to schedule one more frame — which the menu path did
//! incidentally (closing the dropdown repaints), and the keyboard path did
//! not. That made `Ctrl+Shift+Q` (`QuitAll` via key binding) silently do
//! nothing while the equivalent menu item worked, and gave `NewWindow` the
//! identical latent defect, merely less visibly.
//!
//! The fix moved the drain to the very end of `update()`, downstream of
//! every writer for that frame. This type exists to make that ordering
//! requirement structural rather than a comment someone can miss while
//! editing nearby code: every request funnels through `request_new_window`
//! / `request_quit_all`, and the *only* way to observe them is `take`,
//! which is meant to be called exactly once, at the end of the frame, by
//! the single reader in `app_impl.rs`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct WindowLifecycleRequests {
    new_window: bool,
    quit_all: bool,
}

impl WindowLifecycleRequests {
    /// Record a request to spawn a new window, raised by the `NewWindow`
    /// menu item or key binding. Idempotent — requesting twice before the
    /// next `take()` has no additional effect.
    pub(super) const fn request_new_window(&mut self) {
        self.new_window = true;
    }

    /// Record a request to quit every open window, raised by the
    /// `QuitAll` menu item or key binding. Idempotent — requesting twice
    /// before the next `take()` has no additional effect.
    pub(super) const fn request_quit_all(&mut self) {
        self.quit_all = true;
    }

    /// Whether a new-window request is currently pending.
    ///
    /// Takes `self` by value rather than `&self`: `WindowLifecycleRequests`
    /// is a 2-byte `Copy` type, and `clippy::trivially_copy_pass_by_ref`
    /// (part of the workspace's `clippy::pedantic` gate) requires by-value
    /// here. Call sites are unaffected — `requests.wants_new_window()`
    /// reads identically either way.
    pub(super) const fn wants_new_window(self) -> bool {
        self.new_window
    }

    /// Whether a quit-all request is currently pending. See
    /// [`Self::wants_new_window`] for why this takes `self` by value.
    pub(super) const fn wants_quit_all(self) -> bool {
        self.quit_all
    }

    /// Return the currently-pending requests and reset `self` to
    /// "nothing requested". This is the sole read path: callers must not
    /// peek at `wants_new_window` / `wants_quit_all` and then separately
    /// clear the flags, because that reintroduces the two-step
    /// read-then-write race this type exists to prevent (see the module
    /// doc, issue #512).
    pub(super) fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        let requests = WindowLifecycleRequests::default();
        assert!(!requests.wants_new_window());
        assert!(!requests.wants_quit_all());
    }

    #[test]
    fn request_new_window_sets_only_that_flag() {
        let mut requests = WindowLifecycleRequests::default();
        requests.request_new_window();
        assert!(requests.wants_new_window());
        assert!(!requests.wants_quit_all());
    }

    #[test]
    fn request_quit_all_sets_only_that_flag() {
        let mut requests = WindowLifecycleRequests::default();
        requests.request_quit_all();
        assert!(!requests.wants_new_window());
        assert!(requests.wants_quit_all());
    }

    #[test]
    fn take_returns_requests_and_resets_self() {
        let mut requests = WindowLifecycleRequests::default();
        requests.request_new_window();
        requests.request_quit_all();

        let taken = requests.take();

        assert!(taken.wants_new_window());
        assert!(taken.wants_quit_all());
        assert!(!requests.wants_new_window());
        assert!(!requests.wants_quit_all());
    }

    #[test]
    fn requesting_twice_is_idempotent() {
        let mut requests = WindowLifecycleRequests::default();
        requests.request_new_window();
        requests.request_new_window();
        requests.request_quit_all();
        requests.request_quit_all();

        let taken = requests.take();

        assert!(taken.wants_new_window());
        assert!(taken.wants_quit_all());
    }
}
