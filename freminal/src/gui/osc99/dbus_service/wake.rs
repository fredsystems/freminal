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
/// window drains the events it just queued. Every window's `update()` drains
/// the one shared event channel, so any live window will do: the GUI
/// re-points this at the window it is rendering each frame
/// ([`Self::retarget`]), which keeps the target a window that exists. If that
/// window closes before another frame retargets, a wake is simply ignored and
/// the events wait for the next frame of any window.
#[derive(Clone, Default)]
pub struct Osc99Wake {
    shared: Arc<WakeShared>,
}

/// The state [`Osc99Wake`] shares with the service's waker.
#[derive(Default)]
struct WakeShared {
    /// The event-loop proxy, set once by the first frame.
    proxy: OnceLock<RepaintProxy>,
    /// The window to wake: the one that rendered most recently.
    window: Mutex<Option<WindowId>>,
}

impl Osc99Wake {
    /// Point the wake at `window`. `proxy` is only called the first time, to
    /// obtain the cross-thread repaint handle.
    pub fn retarget(&self, window: WindowId, proxy: impl FnOnce() -> RepaintProxy) {
        self.shared.proxy.get_or_init(proxy);
        *self
            .shared
            .window
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(window);
    }

    /// A waker for [`Osc99DbusService::spawn`](super::Osc99DbusService::spawn).
    pub fn waker(&self) -> Box<dyn Fn() + Send> {
        let shared = Arc::clone(&self.shared);
        Box::new(move || shared.wake())
    }
}

impl WakeShared {
    /// Ask the event loop to repaint the target window, if there is one.
    fn wake(&self) {
        let window = *self.window.lock().unwrap_or_else(PoisonError::into_inner);
        if let (Some(proxy), Some(window)) = (self.proxy.get(), window) {
            proxy.request_repaint(window);
        }
    }
}
