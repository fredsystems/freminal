// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The repaint wake the notification service uses to reach the GUI.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use freminal_windowing::{RepaintProxy, WindowId};

/// Where the service's repaint wake-ups go.
///
/// The service runs on its own threads and must wake the GUI so an idle
/// window drains the events it just queued. Every terminal window's
/// `update()` drains the one shared event channel, so any live one will do.
/// The GUI registers each terminal window when it is created and unregisters
/// it when it closes ([`Self::register`], [`Self::unregister`]; the Settings
/// window has no terminal state to drain with, so it is never registered), and
/// re-points the preferred window at the one it is rendering
/// ([`Self::retarget`]). A wake goes to the preferred window while it is live,
/// else to any live window, else nowhere ([`select_wake_target`]): closing the
/// preferred window never leaves the service unable to wake an idle GUI.
#[derive(Clone, Default)]
pub struct NotifyWake {
    shared: Arc<WakeShared>,
}

/// The state [`NotifyWake`] shares with the service's waker.
#[derive(Default)]
struct WakeShared {
    /// The event-loop proxy, set once by the first frame.
    proxy: OnceLock<RepaintProxy>,
    /// The windows a wake may go to.
    targets: Mutex<WakeTargets>,
}

/// The windows a wake may go to.
#[derive(Default)]
struct WakeTargets {
    /// Every live terminal window, in registration order.
    live: Vec<WindowId>,
    /// The window that rendered most recently.
    preferred: Option<WindowId>,
}

impl WakeTargets {
    /// Add `window` to the live set (once).
    fn register(&mut self, window: WindowId) {
        if !self.live.contains(&window) {
            self.live.push(window);
        }
    }

    /// Remove `window` from the live set, and forget it as the preferred one.
    fn unregister(&mut self, window: WindowId) {
        self.live.retain(|live| *live != window);
        if self.preferred == Some(window) {
            self.preferred = None;
        }
    }

    /// The window a wake goes to now.
    fn target(&self) -> Option<WindowId> {
        select_wake_target(&self.live, self.preferred)
    }
}

/// The window a wake goes to: `preferred` if it is in `live`, else the first
/// live window, else none.
fn select_wake_target(live: &[WindowId], preferred: Option<WindowId>) -> Option<WindowId> {
    preferred
        .filter(|window| live.contains(window))
        .or_else(|| live.first().copied())
}

impl NotifyWake {
    fn lock_targets(&self) -> std::sync::MutexGuard<'_, WakeTargets> {
        self.shared
            .targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// A terminal window now exists: wakes may go to it.
    pub fn register(&self, window: WindowId) {
        self.lock_targets().register(window);
    }

    /// A terminal window is gone: wakes no longer go to it.
    pub fn unregister(&self, window: WindowId) {
        self.lock_targets().unregister(window);
    }

    /// Prefer `window` for wakes (it is rendering right now). `proxy` is only
    /// called the first time, to obtain the cross-thread repaint handle.
    pub fn retarget(&self, window: WindowId, proxy: impl FnOnce() -> RepaintProxy) {
        self.shared.proxy.get_or_init(proxy);
        self.lock_targets().preferred = Some(window);
    }

    /// A waker for the notification service: the one
    /// [`DesktopNotifier::new`](crate::gui::desktop_notify::DesktopNotifier::new)'s factory makes.
    pub fn waker(&self) -> Box<dyn Fn() + Send> {
        let shared = Arc::clone(&self.shared);
        Box::new(move || shared.wake())
    }
}

impl WakeShared {
    /// Ask the event loop to repaint the target window, if there is one.
    fn wake(&self) {
        let window = self
            .targets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .target();
        if let (Some(proxy), Some(window)) = (self.proxy.get(), window) {
            proxy.request_repaint(window);
        }
    }
}

#[cfg(test)]
mod tests {
    use freminal_windowing::WindowId;

    use super::{WakeTargets, select_wake_target};

    fn id(n: u64) -> WindowId {
        WindowId::for_test(n)
    }

    #[test]
    fn the_preferred_window_is_chosen_while_it_is_live() {
        assert_eq!(
            select_wake_target(&[id(1), id(2), id(3)], Some(id(2))),
            Some(id(2))
        );
    }

    #[test]
    fn a_dead_preferred_window_falls_back_to_any_live_one() {
        assert_eq!(
            select_wake_target(&[id(1), id(3)], Some(id(2))),
            Some(id(1))
        );
    }

    #[test]
    fn no_preference_falls_back_to_a_live_window() {
        assert_eq!(select_wake_target(&[id(4)], None), Some(id(4)));
    }

    #[test]
    fn no_live_window_means_no_target() {
        assert_eq!(select_wake_target(&[], Some(id(1))), None);
        assert_eq!(select_wake_target(&[], None), None);
    }

    #[test]
    fn unregistering_the_preferred_window_retargets_to_a_survivor() {
        let mut targets = WakeTargets::default();
        targets.register(id(1));
        targets.register(id(2));
        targets.preferred = Some(id(2));
        assert_eq!(targets.target(), Some(id(2)));

        targets.unregister(id(2));
        assert_eq!(targets.preferred, None);
        assert_eq!(targets.target(), Some(id(1)));

        targets.unregister(id(1));
        assert_eq!(targets.target(), None);
    }

    #[test]
    fn registering_twice_lists_the_window_once() {
        let mut targets = WakeTargets::default();
        targets.register(id(1));
        targets.register(id(1));
        assert_eq!(targets.live, [id(1)]);
    }

    #[test]
    fn a_window_that_renders_after_closing_is_not_a_target() {
        // `retarget` sets only the preference; a window that was never
        // registered (the Settings window) or is already unregistered is not
        // live, so it is never chosen.
        let mut targets = WakeTargets::default();
        targets.register(id(1));
        targets.preferred = Some(id(9));
        assert_eq!(targets.target(), Some(id(1)));
    }
}
