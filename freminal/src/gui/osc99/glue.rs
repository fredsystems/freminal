//! The GUI glue for OSC 99: how `FreminalGui` routes a frame's requests,
//! expires and delivers events, and performs focus requests.
//!
//! The module has two layers, kept apart on purpose:
//!
//! - **Decision functions** ([`route_requests`], [`expire_due`],
//!   [`deliver_toast_events`], [`deliver_system_events`],
//!   [`drain_focus_requests`] and the small helpers around them) work on plain
//!   inputs: `&mut Osc99Registry`, an [`Osc99EffectSink`] (toast stack and
//!   backends), the icon cache, the notification config. They hold every
//!   decision and none of the `RefCell` / window plumbing, so they run
//!   headlessly under test.
//! - **The `impl FreminalGui` block** owns the plumbing: it borrows the shared
//!   cells, deferring work when one is busy rather than losing it, builds the
//!   sink out of the live backends, calls a decision function, and hands the
//!   focus requests it produced to the queue the next frame drains.
//!
//! Every method here borrows its cells with `try_borrow_mut` and never holds a
//! borrow across a call back into the GUI, so a busy cell can only defer
//! work, never panic.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use freminal_common::config::NotificationsConfig;
use freminal_windowing::WindowId;
use tracing::trace;

use super::{
    NoSystemBackend, NoUntrackedBackend, Osc99Effect, Osc99EffectSink, Osc99IconCache,
    Osc99LinuxDaemon, Osc99Origin, Osc99Registry, Osc99Request, Osc99SystemBackend,
    Osc99UntrackedBackend, apply_osc99_effects,
};
use crate::gui::FreminalGui;
use crate::gui::actions::{PaneFocusOutcome, focus_pane_in_tabs};
use crate::gui::frame_drain::WindowFocus;
use crate::gui::notifications::NotificationRouter;
use crate::gui::toast::ToastStack;
use crate::gui::window::PerWindowState;

/// How many OSC 99 requests may wait in `FreminalGui::deferred_osc99_requests`
/// for a busy cell to free up. A cell is only ever busy transiently, so
/// reaching this means something is badly wrong; the newest requests beyond it
/// are dropped rather than letting the queue grow without bound.
const MAX_DEFERRED_OSC99_REQUESTS: usize = 256;

// ── Decision functions (plain inputs) ──────────────────────────────────────

/// Execute `effects` against `sink`, then end the system leg of every show or
/// close the system backend could not queue.
///
/// A show or close that could not even be queued will never produce a service
/// event, so a failed show is fed back to `registry` as `ShowFailed` and a
/// failed close as `Closed`, and what the registry returns is executed in one
/// more pass (Linux/BSD only: elsewhere the executor never produces those
/// effects). On return the sink's `failed_system_*` lists are empty.
///
/// The extra pass cannot recurse: a failed show or close only closes the Linux
/// leg, so the registry answers with at most a close report, never another
/// `ShowLinuxSystem` or `CloseLinuxSystem`. The follow-up effects therefore run
/// against backends that fail every request, and a failure there (which cannot
/// happen) is only logged.
#[cfg(all(unix, not(target_os = "macos")))]
fn apply_and_feed_back(
    effects: Vec<Osc99Effect>,
    registry: &mut Osc99Registry,
    sink: &mut Osc99EffectSink<'_>,
) {
    apply_osc99_effects(effects, sink);
    feed_back_failed_legs(registry, sink);
}

/// [`apply_and_feed_back`] where there is no desktop-notification service
/// (macOS and Windows): only that service is ever asked to show or close, so
/// nothing can have failed and there is nothing to feed back. The registry is
/// read-only here, which is why the signature differs.
#[cfg(not(all(unix, not(target_os = "macos"))))]
fn apply_and_feed_back(
    effects: Vec<Osc99Effect>,
    _registry: &Osc99Registry,
    sink: &mut Osc99EffectSink<'_>,
) {
    apply_osc99_effects(effects, sink);
    debug_assert!(sink.failed_system_shows.is_empty() && sink.failed_system_closes.is_empty());
}

/// The feedback pass of `apply_and_feed_back`: report the sink's failed
/// system shows and closes to `registry` and execute what it returns, once.
#[cfg(all(unix, not(target_os = "macos")))]
fn feed_back_failed_legs(registry: &mut Osc99Registry, sink: &mut Osc99EffectSink<'_>) {
    use super::Osc99SystemLegEvent;

    let shows = std::mem::take(&mut sink.failed_system_shows);
    let closes = std::mem::take(&mut sink.failed_system_closes);
    if shows.is_empty() && closes.is_empty() {
        return;
    }

    let events = shows
        .into_iter()
        .map(|instance| (instance, Osc99SystemLegEvent::ShowFailed))
        .chain(
            closes
                .into_iter()
                .map(|instance| (instance, Osc99SystemLegEvent::Closed)),
        );
    let effects: Vec<Osc99Effect> = events
        .flat_map(|(instance, event)| registry.on_system_event(instance, event))
        .collect();

    let (mut no_system, mut no_untracked) = (NoSystemBackend, NoUntrackedBackend);
    let mut follow_up = Osc99EffectSink::new(&mut *sink.toasts, &mut no_system, &mut no_untracked);
    apply_osc99_effects(effects, &mut follow_up);
    let Osc99EffectSink {
        mut focus_requests,
        failed_system_shows,
        failed_system_closes,
        ..
    } = follow_up;
    sink.focus_requests.append(&mut focus_requests);
    if !failed_system_shows.is_empty() || !failed_system_closes.is_empty() {
        tracing::debug!(
            ?failed_system_shows,
            ?failed_system_closes,
            "OSC 99: a failed show or close produced another system request; ignored (cannot happen)"
        );
    }
}

/// Route `requests` strictly in order (Task 138.C10, G1) and apply each one's
/// effects before the next is routed, so a later request sees the registry
/// state an earlier one left.
///
/// The router decides (display) or answers (control, icon priming) from the
/// registry and the icon cache, and the effects it returns are applied to
/// `sink` with a failed desktop-notification leg fed back to `registry`
/// ([`apply_and_feed_back`]). Each request's `reply_tx` (a `Weak` handle to
/// the originating pane's reply sender) reaches the registry with it, which
/// upgrades it per send, so a pane is never kept alive by a notification. The
/// pane state gates `o=`; OS-window focus alone drives the toast/system
/// routing choice, and `daemon` turns a system leg nobody can show into the
/// toast leg.
///
/// The panes to focus are left in `sink.focus_requests`.
fn route_requests<'r>(
    requests: impl IntoIterator<Item = &'r Osc99Request>,
    notifications: &NotificationsConfig,
    window_focus: WindowFocus,
    daemon: Osc99LinuxDaemon,
    icon_cache: &mut Osc99IconCache,
    registry: &mut Osc99Registry,
    sink: &mut Osc99EffectSink<'_>,
) {
    for request in requests {
        let effects = NotificationRouter::route_osc99_request(
            request,
            notifications,
            window_focus,
            daemon,
            icon_cache,
            registry,
        );
        apply_and_feed_back(effects, registry, sink);
    }
}

/// The requests to keep for the next frame when a cell was busy: `requests`
/// in order, capped at [`MAX_DEFERRED_OSC99_REQUESTS`]. The newest requests
/// beyond the cap are dropped, with a warning.
fn cap_deferred<'r>(requests: impl IntoIterator<Item = &'r Osc99Request>) -> Vec<Osc99Request> {
    let mut kept: Vec<Osc99Request> = requests.into_iter().cloned().collect();
    if kept.len() > MAX_DEFERRED_OSC99_REQUESTS {
        tracing::warn!(
            dropped = kept.len() - MAX_DEFERRED_OSC99_REQUESTS,
            "OSC 99: deferred request queue full; newest requests dropped"
        );
        kept.truncate(MAX_DEFERRED_OSC99_REQUESTS);
    }
    kept
}

/// Close the OSC 99 notifications whose `w=` deadline has passed at `now`,
/// execute the resulting effects, and return how long until the next
/// deadline, or `None` when nothing will expire.
///
/// The delay is measured before the effects run, and is what the caller folds
/// into the window's repaint scheduling.
fn expire_due(
    registry: &mut Osc99Registry,
    sink: &mut Osc99EffectSink<'_>,
    now: Instant,
) -> Option<Duration> {
    let effects = registry.tick(now);
    let next = registry
        .next_deadline()
        .map(|deadline| deadline.saturating_duration_since(now));
    apply_and_feed_back(effects, registry, sink);
    next
}

/// Hand the events the keyed toasts recorded (activation, close) to the
/// registry and execute the effects it returns.
///
/// Takes the events out of `sink.toasts`, so a second call finds none.
fn deliver_toast_events(registry: &mut Osc99Registry, sink: &mut Osc99EffectSink<'_>) {
    let events = sink.toasts.take_events();
    if events.is_empty() {
        return;
    }
    let effects: Vec<Osc99Effect> = events
        .into_iter()
        .flat_map(|event| registry.on_toast_event(event))
        .collect();
    apply_and_feed_back(effects, registry, sink);
}

/// Record what is now known about the notification daemon and return the
/// capabilities to re-send to every pane when that changes them.
///
/// A daemon that did not change is ignored, so the one `warn!` for an
/// unreachable bus is logged once even if the event repeats. With the daemon
/// unavailable, `System` routing becomes `Unsupported`, while `Both` and
/// `SystemWhenUnfocused` fall back to the toast leg (routing substitutes it
/// for the system leg, see
/// [`crate::gui::notifications::Osc99DisplayContext::daemon`]).
#[cfg(all(unix, not(target_os = "macos")))]
fn record_daemon_state(
    daemon: &std::cell::Cell<Osc99LinuxDaemon>,
    new: Osc99LinuxDaemon,
    config: &freminal_common::config::Config,
) -> Option<freminal_common::host_capabilities::HostCapabilities> {
    let old = daemon.replace(new);
    if old == new {
        return None;
    }
    if matches!(new, Osc99LinuxDaemon::Unavailable) {
        tracing::warn!(
            "OSC 99: the desktop-notification service is unavailable; System routing \
             stops advertising OSC 99, and Both and SystemWhenUnfocused fall back to \
             the toast leg"
        );
    }
    crate::gui::host_capabilities::daemon_state_change(config, old, new)
}

/// Hand the events the Linux/BSD desktop-notification service queued to the
/// registry and execute the effects it returns.
///
/// The daemon's capability reply and a bus failure update `daemon`
/// ([`record_daemon_state`]); when that changes the resolved capabilities,
/// `broadcast` is called with them, immediately and before any of the
/// batch's effects run. `broadcast` must therefore touch none of the cells
/// the caller holds.
#[cfg(all(unix, not(target_os = "macos")))]
fn deliver_system_events(
    events: Vec<super::Osc99DbusEvent>,
    daemon: &std::cell::Cell<Osc99LinuxDaemon>,
    config: &freminal_common::config::Config,
    registry: &mut Osc99Registry,
    sink: &mut Osc99EffectSink<'_>,
    mut broadcast: impl FnMut(freminal_common::host_capabilities::HostCapabilities),
) {
    use super::{Osc99DbusEvent, Osc99SystemLegEvent};

    let mut effects = Vec::new();
    for event in events {
        let (instance, leg_event) = match event {
            Osc99DbusEvent::Shown { instance } => {
                tracing::trace!(instance = ?instance, "OSC 99: desktop notification shown");
                continue;
            }
            Osc99DbusEvent::ShowFailed { instance } => (instance, Osc99SystemLegEvent::ShowFailed),
            Osc99DbusEvent::Activated { instance, action } => (
                instance,
                Osc99SystemLegEvent::Activated(action.to_leg_activation()),
            ),
            Osc99DbusEvent::Closed { instance } => (instance, Osc99SystemLegEvent::Closed),
            Osc99DbusEvent::Capabilities(capabilities) => {
                tracing::debug!(
                    body = ?capabilities.body,
                    actions = ?capabilities.actions,
                    body_markup = ?capabilities.body_markup,
                    sound = ?capabilities.sound,
                    "OSC 99: notification daemon capabilities"
                );
                if let Some(changed) =
                    record_daemon_state(daemon, Osc99LinuxDaemon::Available(capabilities), config)
                {
                    broadcast(changed);
                }
                continue;
            }
            Osc99DbusEvent::Unavailable => {
                // The service already logged why; `record_daemon_state` logs
                // the consequence.
                if let Some(changed) =
                    record_daemon_state(daemon, Osc99LinuxDaemon::Unavailable, config)
                {
                    broadcast(changed);
                }
                continue;
            }
        };
        effects.extend(registry.on_system_event(instance, leg_event));
    }
    if !effects.is_empty() {
        apply_and_feed_back(effects, registry, sink);
    }
}

/// Whether `queue` holds work for the next frame. A busy queue counts as
/// empty: its holder is mid-update and will have its own frame.
fn has_queued_work<T>(queue: &RefCell<Vec<T>>) -> bool {
    queue.try_borrow().is_ok_and(|queue| !queue.is_empty())
}

/// The windows OSC 99 focus requests are performed on.
///
/// A trait so [`drain_focus_requests`] can be tested without a windowing
/// layer; [`LiveWindows`] is the implementation the app uses.
trait Osc99FocusTargets {
    /// Switch to `origin`'s tab, unzoom, and make its pane the active pane.
    /// `None` when the window is gone.
    fn focus_pane(&mut self, origin: Osc99Origin) -> Option<PaneFocusOutcome>;

    /// Un-minimise `window_id`, raise it, and repaint it.
    fn raise_window(&mut self, window_id: WindowId);
}

/// Perform the focus requests queued by activated OSC 99 notifications, in
/// order.
///
/// A request whose window is gone, or whose pane is gone, is skipped. A
/// focused pane's window is raised.
fn drain_focus_requests(requests: Vec<Osc99Origin>, targets: &mut impl Osc99FocusTargets) {
    for origin in requests {
        match targets.focus_pane(origin) {
            None => trace!(?origin, "OSC 99 focus request: window is gone; skipped"),
            Some(PaneFocusOutcome::PaneNotFound) => {
                trace!(?origin, "OSC 99 focus request: pane is gone; skipped");
            }
            Some(PaneFocusOutcome::Focused) => targets.raise_window(origin.window_id),
        }
    }
}

/// [`Osc99FocusTargets`] over the app's real windows and the windowing handle
/// of the frame being rendered.
struct LiveWindows<'a, 'h> {
    windows: &'a mut HashMap<WindowId, PerWindowState>,
    handle: &'a freminal_windowing::WindowHandle<'h>,
}

impl Osc99FocusTargets for LiveWindows<'_, '_> {
    fn focus_pane(&mut self, origin: Osc99Origin) -> Option<PaneFocusOutcome> {
        let win = self.windows.get_mut(&origin.window_id)?;
        Some(focus_pane_in_tabs(&mut win.tabs, origin.pane_id))
    }

    fn raise_window(&mut self, window_id: WindowId) {
        // Un-minimise first: a minimised window ignores a focus request on
        // some platforms. The window's minimised state is only known to its
        // own egui context, which is not the one rendering now, so the request
        // is always made; winit makes it a no-op for a window that is not
        // minimised (Windows and macOS compare the real state; X11 sends the
        // same activation `focus_window` does; Wayland cannot un-minimise and
        // logs that it ignored the request).
        self.handle.set_minimized(window_id, false);
        self.handle.focus_window(window_id);
        self.handle.request_repaint(window_id);
    }
}

// ── FreminalGui plumbing ───────────────────────────────────────────────────

impl FreminalGui {
    /// Route the OSC 99 requests a frame's drain collected, strictly in
    /// arrival order (Task 138.C10, G1), preceded by the requests an earlier
    /// frame had to defer.
    ///
    /// The toast stack, the icon cache and the registry are all borrowed
    /// before anything changes, so a busy cell leaves every request untouched:
    /// they are all deferred, in order, to the next frame (up to
    /// [`MAX_DEFERRED_OSC99_REQUESTS`]); that frame is requested by
    /// [`Self::wake_for_osc99_pending_work`]. Nothing else can borrow a cell
    /// between two requests of a frame, so one set of borrows serves the whole
    /// frame.
    ///
    /// A deferred request is routed with the window focus of the frame that
    /// finally routes it, not the frame that collected it.
    pub(in crate::gui) fn route_osc99_requests(
        &self,
        window_focus: WindowFocus,
        fresh: &[Osc99Request],
    ) {
        let deferred = self.deferred_osc99_requests.try_borrow_mut().map_or_else(
            |_| {
                tracing::warn!("OSC 99: deferred request queue busy; routing only this frame's");
                Vec::new()
            },
            |mut queue| std::mem::take(&mut *queue),
        );
        if deferred.is_empty() && fresh.is_empty() {
            return;
        }

        let (Ok(mut toasts), Ok(mut icon_cache), Ok(mut registry)) = (
            self.toasts.try_borrow_mut(),
            self.osc99_icon_cache.try_borrow_mut(),
            self.osc99_registry.try_borrow_mut(),
        ) else {
            tracing::warn!("OSC 99: notification state busy; requests deferred to the next frame");
            self.store_deferred_osc99_requests(cap_deferred(deferred.iter().chain(fresh)));
            return;
        };

        self.with_osc99_sink(&mut toasts, |sink| {
            route_requests(
                deferred.iter().chain(fresh),
                &self.config.notifications,
                window_focus,
                self.osc99_daemon.get(),
                &mut icon_cache,
                &mut registry,
                sink,
            );
        });
    }

    /// Keep `requests` for the next frame's routing pass.
    fn store_deferred_osc99_requests(&self, requests: Vec<Osc99Request>) {
        if requests.is_empty() {
            return;
        }
        if let Ok(mut queue) = self.deferred_osc99_requests.try_borrow_mut() {
            *queue = requests;
        } else {
            tracing::warn!(
                dropped = requests.len(),
                "OSC 99: deferred request queue busy; requests dropped"
            );
        }
    }

    /// Close the OSC 99 notifications whose `w=` deadline has passed, execute
    /// the resulting effects, and return how long until the next deadline.
    ///
    /// The returned delay is what the caller folds into the window's repaint
    /// scheduling, the same way the toast stack's own delay is. `None` means
    /// no instance will expire. A busy cell defers the tick and asks for a
    /// prompt retry rather than waiting for the next event. Both cells are
    /// borrowed before the registry changes, so a busy cell loses nothing.
    pub(in crate::gui) fn expire_osc99_notifications(&self) -> Option<Duration> {
        let (Ok(mut toasts), Ok(mut registry)) = (
            self.toasts.try_borrow_mut(),
            self.osc99_registry.try_borrow_mut(),
        ) else {
            tracing::warn!("OSC 99: toast stack or registry busy; expiry deferred");
            return Some(Duration::from_millis(16));
        };

        self.with_osc99_sink(&mut toasts, |sink| {
            expire_due(&mut registry, sink, Instant::now())
        })
    }

    /// Hand the events recorded by keyed toasts (activation, close) to the
    /// OSC 99 registry and execute the effects it returns.
    ///
    /// Runs once per window frame after `ToastStack::show`. Both cells are
    /// borrowed before the events are taken, so a busy cell defers delivery
    /// to the next frame instead of losing the events.
    pub(in crate::gui) fn route_osc99_toast_events(&self) {
        let (Ok(mut toasts), Ok(mut registry)) = (
            self.toasts.try_borrow_mut(),
            self.osc99_registry.try_borrow_mut(),
        ) else {
            tracing::warn!("OSC 99: toast stack or registry busy; toast events deferred");
            return;
        };

        self.with_osc99_sink(&mut toasts, |sink| {
            deliver_toast_events(&mut registry, sink);
        });
    }

    /// Hand the events the Linux/BSD desktop-notification service queued to
    /// the OSC 99 registry and execute the effects it returns.
    ///
    /// Runs once per window frame, with `current` the window being rendered
    /// (out of `self.windows` for the frame, see
    /// [`Self::broadcast_host_capabilities`]). Does nothing until the service
    /// has been started. Both cells are borrowed before the events are
    /// drained, so a busy cell defers delivery to the next frame instead of
    /// losing them. The service handle is only borrowed to drain, and
    /// released before the effects are applied (which borrow it again to
    /// show).
    ///
    /// The daemon's capability reply and a bus failure update
    /// `self.osc99_daemon`; when that changes the resolved `HostCapabilities`
    /// they are re-sent to every pane (including `current`'s). That touches
    /// no `RefCell`, so it is safe with the toast and registry borrows held.
    #[cfg(all(unix, not(target_os = "macos")))]
    pub(in crate::gui) fn route_osc99_system_events(&self, current: &PerWindowState) {
        if !self
            .osc99_dbus
            .try_borrow()
            .is_ok_and(|slot| slot.is_some())
        {
            return;
        }
        let (Ok(mut toasts), Ok(mut registry)) = (
            self.toasts.try_borrow_mut(),
            self.osc99_registry.try_borrow_mut(),
        ) else {
            tracing::warn!("OSC 99: toast stack or registry busy; system events deferred");
            return;
        };
        let events: Vec<super::Osc99DbusEvent> = match self.osc99_dbus.try_borrow() {
            Ok(slot) => slot
                .as_ref()
                .map_or_else(Vec::new, |handle| handle.try_events().collect()),
            Err(_) => return,
        };

        self.with_osc99_sink(&mut toasts, |sink| {
            deliver_system_events(
                events,
                &self.osc99_daemon,
                &self.config,
                &mut registry,
                sink,
                |capabilities| self.broadcast_host_capabilities(capabilities, Some(current)),
            );
        });
    }

    /// Start the Linux/BSD desktop-notification service if the config can
    /// route OSC 99 to the system leg
    /// ([`osc99_may_use_system_leg`](crate::gui::host_capabilities::osc99_may_use_system_leg)).
    /// It then lives for the rest of the process; calling this again is a
    /// no-op.
    ///
    /// A busy handle cell leaves the service unstarted; the next call retries,
    /// and until then effects are applied against [`NoSystemBackend`].
    #[cfg(all(unix, not(target_os = "macos")))]
    pub(in crate::gui) fn ensure_osc99_dbus_started(&self) {
        if !crate::gui::host_capabilities::osc99_may_use_system_leg(&self.config) {
            return;
        }
        let Ok(mut slot) = self.osc99_dbus.try_borrow_mut() else {
            return;
        };
        slot.get_or_insert_with(|| {
            tracing::debug!("OSC 99: starting the desktop-notification service");
            super::Osc99DbusService::spawn(self.osc99_wake.waker())
        });
    }

    /// Run `f` with an [`Osc99EffectSink`] over `toasts` and the live
    /// desktop-notification backends, then queue the panes the sink was asked
    /// to focus on `self.osc99_focus_requests`.
    ///
    /// The service is started by [`Self::ensure_osc99_dbus_started`], not
    /// here. If it is not running (the slot is empty or busy), a
    /// [`Osc99Effect::ShowLinuxSystem`] is applied to a backend that fails
    /// every show, which ends the system leg. Platforms without the service
    /// apply that effect to the same backend; it is never produced there.
    ///
    /// The effect executor has no window access, so focus is performed later,
    /// by [`Self::drain_osc99_focus_requests`] at the top of the next
    /// `update()`. A busy queue drops the requests with a warning.
    fn with_osc99_sink<R>(
        &self,
        toasts: &mut ToastStack,
        f: impl FnOnce(&mut Osc99EffectSink<'_>) -> R,
    ) -> R {
        let mut no_system = NoSystemBackend;
        let mut no_untracked = NoUntrackedBackend;
        // A busy cell cannot happen (showing a notification calls nothing back
        // into the GUI), but if it did the show would be lost, not deferred:
        // the untracked leg has no failure to feed back.
        let mut untracked_slot = self.osc99_untracked.try_borrow_mut().ok();
        let untracked: &mut dyn Osc99UntrackedBackend = match untracked_slot.as_deref_mut() {
            Some(backend) => backend.as_mut(),
            None => &mut no_untracked,
        };
        #[cfg(all(unix, not(target_os = "macos")))]
        let mut dbus = self.osc99_dbus.try_borrow_mut().ok();
        #[cfg(all(unix, not(target_os = "macos")))]
        let system: &mut dyn Osc99SystemBackend = match dbus.as_mut().and_then(|slot| slot.as_mut())
        {
            Some(handle) => handle,
            None => &mut no_system,
        };
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let system: &mut dyn Osc99SystemBackend = &mut no_system;

        let mut sink = Osc99EffectSink::new(toasts, system, untracked);
        let result = f(&mut sink);
        let focus_requests = std::mem::take(&mut sink.focus_requests);
        self.queue_osc99_focus_requests(focus_requests);
        result
    }

    /// Append `requests` to the focus queue the next `update()` drains.
    fn queue_osc99_focus_requests(&self, mut requests: Vec<Osc99Origin>) {
        if requests.is_empty() {
            return;
        }
        if let Ok(mut queue) = self.osc99_focus_requests.try_borrow_mut() {
            queue.append(&mut requests);
        } else {
            tracing::warn!(
                dropped = requests.len(),
                "OSC 99: focus request queue busy; focus requests dropped"
            );
        }
    }

    /// Ask for one more frame when OSC 99 work is waiting for one, so the
    /// next `update()` does it even on an otherwise idle window: focus
    /// requests to drain, or requests a busy cell deferred.
    pub(in crate::gui) fn wake_for_osc99_pending_work(&self, ctx: &egui::Context) {
        if has_queued_work(&self.osc99_focus_requests)
            || has_queued_work(&self.deferred_osc99_requests)
        {
            ctx.request_repaint();
        }
    }

    /// Perform the focus requests queued by activated OSC 99 notifications.
    ///
    /// For each requested pane: find its window, switch to the pane's tab,
    /// unzoom, make it the active pane ([`focus_pane_in_tabs`]), then
    /// un-minimise and raise the OS window and repaint it. A missing window
    /// or pane is skipped.
    pub(in crate::gui) fn drain_osc99_focus_requests(
        &mut self,
        handle: &freminal_windowing::WindowHandle<'_>,
    ) {
        let Ok(mut queue) = self.osc99_focus_requests.try_borrow_mut() else {
            tracing::warn!("OSC 99: focus request queue busy; draining deferred");
            return;
        };
        let requests = std::mem::take(&mut *queue);
        drop(queue);
        drain_focus_requests(
            requests,
            &mut LiveWindows {
                windows: &mut self.windows,
                handle,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use crossbeam_channel::{Receiver, Sender};
    use freminal_common::buffer_states::osc99_notification::{
        Notification99Data, NotificationOccasion, NotificationUrgency, Osc99ActivationFocus,
        Osc99ActivationReporting, Osc99CloseReporting, Osc99ControlKind, Osc99Expiry, Osc99Sound,
    };
    use freminal_common::config::NotificationRouting;
    use freminal_terminal_emulator::io::{GuiReply, InputEvent, Osc99CloseTracking};

    use super::*;
    use crate::gui::notifications::Osc99Control;
    use crate::gui::osc99::{
        Osc99ControlRequest, Osc99DisplayRequest, Osc99IconPrime, Osc99InstanceId, Osc99PaneState,
        PaneKeyboardFocus, PaneVisibility, RecordingSystemBackend, RecordingUntrackedBackend,
    };
    use crate::gui::panes::{PaneId, PaneIdGenerator};

    // ── Fixtures ────────────────────────────────────────────────────────

    /// A live pane: its origin, the strong reply handle that keeps it alive,
    /// and the receiving end the replies land on.
    struct LivePane {
        origin: Osc99Origin,
        tx: Arc<Sender<InputEvent>>,
        rx: Receiver<InputEvent>,
    }

    impl LivePane {
        fn new(window: u64, pane_id: PaneId) -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            Self {
                origin: Osc99Origin {
                    window_id: WindowId::for_test(window),
                    pane_id,
                },
                tx: Arc::new(tx),
                rx,
            }
        }

        fn first() -> Self {
            Self::new(1, PaneId::first())
        }

        fn weak(&self) -> Weak<Sender<InputEvent>> {
            Arc::downgrade(&self.tx)
        }

        /// Every reply the pane has been sent since the last call.
        fn replies(&self) -> Vec<GuiReply> {
            self.rx
                .try_iter()
                .filter_map(|event| match event {
                    InputEvent::Reply(reply) => Some(reply),
                    _ => None,
                })
                .collect()
        }
    }

    fn notification(id: &str) -> Notification99Data {
        Notification99Data {
            id: Some(id.to_owned()),
            title: "Title".to_owned(),
            body: Some("Body".to_owned()),
            icon_data: None,
            icon_names: Vec::new(),
            icon_cache_key: None,
            buttons: Vec::new(),
            activation_reporting: Osc99ActivationReporting::Silent,
            activation_focus: Osc99ActivationFocus::NoFocus,
            close_reporting: Osc99CloseReporting::Silent,
            urgency: NotificationUrgency::Normal,
            occasion: NotificationOccasion::Always,
            sound: Osc99Sound::System,
            app_name: None,
            notification_types: Vec::new(),
            expiry: Osc99Expiry::OsDefault,
        }
    }

    /// A notification that reports its close (`c=1`).
    fn reporting(id: &str) -> Notification99Data {
        Notification99Data {
            close_reporting: Osc99CloseReporting::Report,
            ..notification(id)
        }
    }

    fn display(pane: &LivePane, data: Notification99Data) -> Osc99Request {
        Osc99Request::Display(Box::new(Osc99DisplayRequest {
            data,
            reply_tx: pane.weak(),
            origin: pane.origin,
            pane_state: Osc99PaneState {
                focus: PaneKeyboardFocus::Focused,
                visibility: PaneVisibility::Visible,
            },
        }))
    }

    fn control(pane: &LivePane, id: &str, kind: Osc99ControlKind) -> Osc99Request {
        Osc99Request::Control(Osc99ControlRequest {
            control: Osc99Control {
                id: id.to_owned(),
                kind,
            },
            reply_tx: pane.weak(),
            origin: pane.origin,
        })
    }

    fn config(routing: NotificationRouting) -> NotificationsConfig {
        NotificationsConfig {
            enabled: true,
            routing_osc99: routing,
            ..NotificationsConfig::default()
        }
    }

    /// Everything one decision function acts on, owned in one place.
    struct Rig {
        toasts: ToastStack,
        icons: Osc99IconCache,
        registry: Osc99Registry,
        system: RecordingSystemBackend,
        untracked: RecordingUntrackedBackend,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                toasts: ToastStack::default(),
                icons: Osc99IconCache::default(),
                registry: Osc99Registry::default(),
                system: RecordingSystemBackend::default(),
                untracked: RecordingUntrackedBackend::default(),
            }
        }

        /// Run `f` over the registry, the icon cache and a sink on the rig's
        /// stack and backends; return its result and the focus requests the
        /// sink collected.
        fn run<R>(
            &mut self,
            f: impl FnOnce(&mut Osc99Registry, &mut Osc99IconCache, &mut Osc99EffectSink<'_>) -> R,
        ) -> (R, Vec<Osc99Origin>) {
            let mut sink =
                Osc99EffectSink::new(&mut self.toasts, &mut self.system, &mut self.untracked);
            let result = f(&mut self.registry, &mut self.icons, &mut sink);
            (result, sink.focus_requests)
        }

        /// Route `requests` as one frame, in a focused window.
        fn route(
            &mut self,
            requests: &[Osc99Request],
            notifications: &NotificationsConfig,
        ) -> Vec<Osc99Origin> {
            self.run(|registry, icons, sink| {
                route_requests(
                    requests,
                    notifications,
                    WindowFocus::Focused,
                    Osc99LinuxDaemon::assumed(),
                    icons,
                    registry,
                    sink,
                );
            })
            .1
        }

        /// Run the toast-event delivery.
        fn deliver_toast_events(&mut self) {
            self.run(|registry, _, sink| deliver_toast_events(registry, sink));
        }
    }

    /// The instance the router's `ShowToast` effect names for `request`, with
    /// the effects applied to `rig`.
    fn route_and_show(
        rig: &mut Rig,
        request: &Osc99Request,
        notifications: &NotificationsConfig,
    ) -> Osc99InstanceId {
        let effects = NotificationRouter::route_osc99_request(
            request,
            notifications,
            WindowFocus::Focused,
            Osc99LinuxDaemon::assumed(),
            &mut rig.icons,
            &mut rig.registry,
        );
        let shown = effects.iter().find_map(|effect| match effect {
            Osc99Effect::ShowToast { instance, .. }
            | Osc99Effect::ShowLinuxSystem { instance, .. } => Some(*instance),
            _ => None,
        });
        let Some(instance) = shown else {
            panic!("the request must show a notification, got {effects:?}");
        };
        rig.run(|registry, _, sink| apply_and_feed_back(effects, registry, sink));
        instance
    }

    // ── route_requests ──────────────────────────────────────────────────

    #[test]
    fn a_displayed_request_is_registered_and_shown_as_a_toast() {
        let pane = LivePane::first();
        let mut rig = Rig::new();

        let focus = rig.route(
            &[display(&pane, notification("a"))],
            &config(NotificationRouting::Toast),
        );

        assert_eq!(rig.toasts.len(), 1, "the toast leg shows the notification");
        assert_eq!(rig.registry.instances().len(), 1, "and it is registered");
        assert_eq!(focus, Vec::new());
    }

    #[test]
    fn alive_answers_from_what_the_frame_displayed_before_it() {
        let pane = LivePane::first();
        let mut rig = Rig::new();
        let notifications = config(NotificationRouting::Toast);

        rig.route(
            &[
                control(&pane, "poll-1", Osc99ControlKind::Alive),
                display(&pane, notification("a")),
                control(&pane, "poll-2", Osc99ControlKind::Alive),
            ],
            &notifications,
        );

        let lists: Vec<(Option<String>, Vec<String>)> = pane
            .replies()
            .into_iter()
            .filter_map(|reply| match reply {
                GuiReply::Osc99Alive {
                    request_id,
                    live_ids,
                } => Some((request_id, live_ids)),
                _ => None,
            })
            .collect();
        assert_eq!(
            lists,
            [
                (Some("poll-1".to_owned()), Vec::new()),
                (Some("poll-2".to_owned()), vec!["a".to_owned()]),
            ],
            "each poll sees only the displays that arrived before it"
        );
    }

    #[test]
    fn a_close_control_closes_the_toast_of_an_earlier_display() {
        let pane = LivePane::first();
        let mut rig = Rig::new();

        rig.route(
            &[
                display(&pane, notification("a")),
                control(&pane, "a", Osc99ControlKind::Close),
            ],
            &config(NotificationRouting::Toast),
        );

        assert_eq!(rig.toasts.len(), 0, "the close removed the toast");
    }

    #[test]
    fn an_icon_priming_is_stored_in_the_icon_cache() {
        let mut rig = Rig::new();

        rig.route(
            &[Osc99Request::PrimeIcon(Osc99IconPrime {
                key: "k".to_owned(),
                data: vec![1, 2, 3],
            })],
            &config(NotificationRouting::Toast),
        );

        assert_eq!(rig.icons.get("k"), Some(&[1u8, 2, 3][..]));
        assert_eq!(rig.toasts.len(), 0, "a priming shows nothing");
    }

    #[test]
    fn an_activation_that_asks_for_focus_is_queued_for_the_pane() {
        let pane = LivePane::first();
        let mut rig = Rig::new();
        let notifications = config(NotificationRouting::Toast);
        let focusing = Notification99Data {
            activation_focus: Osc99ActivationFocus::Focus,
            ..notification("a")
        };
        let instance = route_and_show(&mut rig, &display(&pane, focusing), &notifications);

        // The user clicks the toast: the stack records `Activated`, then
        // `Closed`, which the next delivery hands to the registry.
        let toast_events = vec![
            crate::gui::toast::ToastEvent {
                key: instance.to_toast_key(),
                kind: crate::gui::toast::ToastEventKind::Activated,
            },
            crate::gui::toast::ToastEvent {
                key: instance.to_toast_key(),
                kind: crate::gui::toast::ToastEventKind::Closed,
            },
        ];
        let effects: Vec<Osc99Effect> = toast_events
            .into_iter()
            .flat_map(|event| rig.registry.on_toast_event(event))
            .collect();
        let ((), focus) = rig.run(|registry, _, sink| apply_and_feed_back(effects, registry, sink));

        assert_eq!(focus, [pane.origin]);
    }

    // ── cap_deferred ────────────────────────────────────────────────────

    fn ids_of(requests: &[Osc99Request]) -> Vec<String> {
        requests
            .iter()
            .filter_map(|request| match request {
                Osc99Request::Display(display) => display.data.id.clone(),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn cap_deferred_keeps_every_request_under_the_cap_in_order() {
        let pane = LivePane::first();
        let requests: Vec<Osc99Request> = ["a", "b", "c"]
            .into_iter()
            .map(|id| display(&pane, notification(id)))
            .collect();

        assert_eq!(ids_of(&cap_deferred(&requests)), ["a", "b", "c"]);
    }

    #[test]
    fn cap_deferred_drops_the_newest_requests_beyond_the_cap() {
        let pane = LivePane::first();
        let requests: Vec<Osc99Request> = (0..MAX_DEFERRED_OSC99_REQUESTS + 44)
            .map(|n| display(&pane, notification(&n.to_string())))
            .collect();

        let kept = ids_of(&cap_deferred(&requests));

        assert_eq!(kept.len(), MAX_DEFERRED_OSC99_REQUESTS);
        assert_eq!(kept.first().map(String::as_str), Some("0"));
        let last = (MAX_DEFERRED_OSC99_REQUESTS - 1).to_string();
        assert_eq!(kept.last(), Some(&last), "the oldest requests are kept");
    }

    // ── expire_due ──────────────────────────────────────────────────────

    fn expiring(id: &str, millis: u32) -> Notification99Data {
        Notification99Data {
            expiry: std::num::NonZeroU32::new(millis)
                .map_or(Osc99Expiry::Never, Osc99Expiry::After),
            ..reporting(id)
        }
    }

    #[test]
    fn expire_due_before_the_deadline_changes_nothing_and_returns_the_wait() {
        let pane = LivePane::first();
        let mut rig = Rig::new();
        rig.route(
            &[display(&pane, expiring("a", 60_000))],
            &config(NotificationRouting::Toast),
        );

        let (next, _) = rig.run(|registry, _, sink| expire_due(registry, sink, Instant::now()));

        assert_eq!(rig.toasts.len(), 1, "not expired yet");
        assert!(
            next.is_some_and(|wait| wait > Duration::ZERO && wait <= Duration::from_secs(60)),
            "the wake-up is for the 60 s deadline, got {next:?}"
        );
    }

    #[test]
    fn expire_due_after_the_deadline_closes_the_toast_and_the_close_is_reported() {
        let pane = LivePane::first();
        let mut rig = Rig::new();
        rig.route(
            &[display(&pane, expiring("a", 50))],
            &config(NotificationRouting::Toast),
        );

        let later = Instant::now() + Duration::from_secs(5);
        let (next, _) = rig.run(|registry, _, sink| expire_due(registry, sink, later));
        assert_eq!(rig.toasts.len(), 0, "the expired toast is closed");
        assert_eq!(next, None, "nothing is left to expire");

        // The close the expiry caused reaches the registry as the toast's
        // `Closed` event, which sends the `c=1` report.
        rig.deliver_toast_events();
        assert_eq!(
            pane.replies(),
            [GuiReply::Osc99Closed {
                id: Some("a".to_owned()),
                tracking: Osc99CloseTracking::Tracked,
            }]
        );
        assert!(rig.registry.instances().is_empty());
    }

    #[test]
    fn expire_due_with_nothing_registered_returns_no_deadline() {
        let mut rig = Rig::new();

        let (next, _) = rig.run(|registry, _, sink| expire_due(registry, sink, Instant::now()));

        assert_eq!(next, None);
    }

    // ── deliver_toast_events ────────────────────────────────────────────

    #[test]
    fn a_toast_closing_is_delivered_as_a_close_report() {
        let pane = LivePane::first();
        let mut rig = Rig::new();
        let notifications = config(NotificationRouting::Toast);
        let instance = route_and_show(&mut rig, &display(&pane, reporting("a")), &notifications);

        rig.toasts.close_keyed(instance.to_toast_key());
        rig.deliver_toast_events();

        assert_eq!(
            pane.replies(),
            [GuiReply::Osc99Closed {
                id: Some("a".to_owned()),
                tracking: Osc99CloseTracking::Tracked,
            }]
        );
        assert!(
            rig.registry.instances().is_empty(),
            "no leg is left, so the instance is forgotten"
        );
    }

    #[test]
    fn toast_events_are_delivered_once() {
        let pane = LivePane::first();
        let mut rig = Rig::new();
        let notifications = config(NotificationRouting::Toast);
        let instance = route_and_show(&mut rig, &display(&pane, reporting("a")), &notifications);
        rig.toasts.close_keyed(instance.to_toast_key());

        rig.deliver_toast_events();
        rig.deliver_toast_events();

        assert_eq!(pane.replies().len(), 1, "the second pass finds no events");
    }

    #[test]
    fn a_toast_that_is_still_open_delivers_nothing() {
        let pane = LivePane::first();
        let mut rig = Rig::new();
        rig.route(
            &[display(&pane, reporting("a"))],
            &config(NotificationRouting::Toast),
        );

        rig.deliver_toast_events();

        assert_eq!(pane.replies(), Vec::new());
        assert_eq!(rig.registry.instances().len(), 1);
    }

    // ── Linux/BSD: failed legs and system events ────────────────────────

    #[cfg(all(unix, not(target_os = "macos")))]
    mod system_leg {
        use std::cell::Cell;

        use freminal_common::config::Config;

        use super::*;
        use crate::gui::osc99::{
            Osc99DaemonCapabilities, Osc99DaemonSupport, Osc99DbusEvent, Osc99Leg,
            dbus_service::Osc99SystemAction,
        };
        use freminal_common::host_capabilities::HostCapabilities;

        fn system_config() -> NotificationsConfig {
            config(NotificationRouting::System)
        }

        fn config_with(notifications: NotificationsConfig) -> Config {
            Config {
                notifications,
                ..Config::default()
            }
        }

        /// A rig with `data` shown on the system leg only, its show queued.
        fn shown_on_system(pane: &LivePane, data: Notification99Data) -> (Rig, Osc99InstanceId) {
            let mut rig = Rig::new();
            let instance = route_and_show(&mut rig, &display(pane, data), &system_config());
            assert_eq!(rig.system.shown.len(), 1, "the show reached the backend");
            assert_eq!(rig.toasts.len(), 0, "System routing shows no toast");
            (rig, instance)
        }

        fn closed_report(id: &str) -> GuiReply {
            GuiReply::Osc99Closed {
                id: Some(id.to_owned()),
                tracking: Osc99CloseTracking::Tracked,
            }
        }

        // ── apply_and_feed_back ─────────────────────────────────────────

        #[test]
        fn a_failed_system_show_is_fed_back_once_and_ends_the_leg() {
            let pane = LivePane::first();
            let mut rig = Rig::new();
            rig.system.fail_shows = true;

            let (leftover, _) = rig.run(|registry, icons, sink| {
                route_requests(
                    &[display(&pane, reporting("a"))],
                    &system_config(),
                    WindowFocus::Focused,
                    Osc99LinuxDaemon::assumed(),
                    icons,
                    registry,
                    sink,
                );
                (
                    sink.failed_system_shows.clone(),
                    sink.failed_system_closes.clone(),
                )
            });

            assert_eq!(
                leftover,
                (Vec::new(), Vec::new()),
                "the failures were consumed by the feedback pass"
            );
            assert_eq!(rig.system.shown.len(), 1, "the show was tried once");
            assert!(
                rig.system.closed.is_empty(),
                "the feedback pass does not ask the service for anything"
            );
            assert_eq!(pane.replies(), [closed_report("a")]);
            assert!(
                rig.registry.instances().is_empty(),
                "with its only leg ended the instance is forgotten"
            );
        }

        #[test]
        fn a_queued_system_show_is_not_fed_back() {
            let pane = LivePane::first();
            let (rig, _) = shown_on_system(&pane, reporting("a"));

            assert_eq!(pane.replies(), Vec::new());
            let [entry] = rig.registry.instances() else {
                panic!("the instance stays registered while its leg is open");
            };
            assert_eq!(entry.open_legs(), [Osc99Leg::LinuxSystem]);
        }

        #[test]
        fn a_failed_system_close_is_fed_back_as_closed() {
            let pane = LivePane::first();
            let (mut rig, _) = shown_on_system(&pane, reporting("a"));

            // The registry asks the service to close; the service cannot even
            // queue it, so no `Closed` event will ever come.
            let effects = rig.registry.close(pane.origin, "a");
            let mut no_system = NoSystemBackend;
            let mut sink =
                Osc99EffectSink::new(&mut rig.toasts, &mut no_system, &mut rig.untracked);
            apply_and_feed_back(effects, &mut rig.registry, &mut sink);

            assert!(
                sink.failed_system_closes.is_empty(),
                "the failure is consumed"
            );
            assert_eq!(pane.replies(), [closed_report("a")]);
            assert!(rig.registry.instances().is_empty());
        }

        // ── deliver_system_events ───────────────────────────────────────

        /// Deliver `events`, returning the capabilities broadcasts.
        fn deliver(
            rig: &mut Rig,
            daemon: &Cell<Osc99LinuxDaemon>,
            config: &Config,
            events: Vec<Osc99DbusEvent>,
        ) -> (Vec<HostCapabilities>, Vec<Osc99Origin>) {
            let mut broadcasts = Vec::new();
            let ((), focus) = rig.run(|registry, _, sink| {
                deliver_system_events(events, daemon, config, registry, sink, |capabilities| {
                    broadcasts.push(capabilities);
                });
            });
            (broadcasts, focus)
        }

        fn assumed_daemon() -> Cell<Osc99LinuxDaemon> {
            Cell::new(Osc99LinuxDaemon::assumed())
        }

        #[test]
        fn a_system_closed_event_ends_the_leg_and_reports_the_close() {
            let pane = LivePane::first();
            let (mut rig, instance) = shown_on_system(&pane, reporting("a"));

            let (broadcasts, _) = deliver(
                &mut rig,
                &assumed_daemon(),
                &config_with(system_config()),
                vec![Osc99DbusEvent::Closed { instance }],
            );

            assert_eq!(pane.replies(), [closed_report("a")]);
            assert!(rig.registry.instances().is_empty());
            assert_eq!(broadcasts, Vec::new(), "no daemon change");
        }

        #[test]
        fn a_system_show_failed_event_ends_the_leg() {
            let pane = LivePane::first();
            let (mut rig, instance) = shown_on_system(&pane, reporting("a"));

            deliver(
                &mut rig,
                &assumed_daemon(),
                &config_with(system_config()),
                vec![Osc99DbusEvent::ShowFailed { instance }],
            );

            assert_eq!(pane.replies(), [closed_report("a")]);
            assert!(rig.registry.instances().is_empty());
        }

        #[test]
        fn a_system_activation_that_asks_for_focus_queues_the_pane() {
            let pane = LivePane::first();
            let focusing = Notification99Data {
                activation_focus: Osc99ActivationFocus::Focus,
                ..reporting("a")
            };
            let (mut rig, instance) = shown_on_system(&pane, focusing);

            let (_, focus) = deliver(
                &mut rig,
                &assumed_daemon(),
                &config_with(system_config()),
                vec![Osc99DbusEvent::Activated {
                    instance,
                    action: Osc99SystemAction::Default,
                }],
            );

            assert_eq!(focus, [pane.origin]);
        }

        #[test]
        fn a_shown_event_changes_nothing() {
            let pane = LivePane::first();
            let (mut rig, instance) = shown_on_system(&pane, reporting("a"));

            let (broadcasts, focus) = deliver(
                &mut rig,
                &assumed_daemon(),
                &config_with(system_config()),
                vec![Osc99DbusEvent::Shown { instance }],
            );

            assert_eq!((broadcasts, focus), (Vec::new(), Vec::new()));
            assert_eq!(pane.replies(), Vec::new());
            assert_eq!(rig.registry.instances().len(), 1);
        }

        #[test]
        fn a_capabilities_event_that_changes_what_is_advertised_is_broadcast() {
            let mut rig = Rig::new();
            let daemon = assumed_daemon();
            let no_actions = Osc99DaemonCapabilities {
                actions: Osc99DaemonSupport::Unsupported,
                ..Osc99DaemonCapabilities::ASSUMED
            };
            let config = config_with(system_config());

            let (broadcasts, _) = deliver(
                &mut rig,
                &daemon,
                &config,
                vec![Osc99DbusEvent::Capabilities(no_actions)],
            );

            assert_eq!(daemon.get(), Osc99LinuxDaemon::Available(no_actions));
            assert_eq!(
                broadcasts,
                [crate::gui::host_capabilities::host_capabilities(
                    &config,
                    Osc99LinuxDaemon::Available(no_actions)
                )]
            );
        }

        #[test]
        fn a_repeated_capabilities_event_is_not_broadcast_again() {
            let mut rig = Rig::new();
            let daemon = assumed_daemon();
            let no_actions = Osc99DaemonCapabilities {
                actions: Osc99DaemonSupport::Unsupported,
                ..Osc99DaemonCapabilities::ASSUMED
            };
            let config = config_with(system_config());

            let (broadcasts, _) = deliver(
                &mut rig,
                &daemon,
                &config,
                vec![
                    Osc99DbusEvent::Capabilities(no_actions),
                    Osc99DbusEvent::Capabilities(no_actions),
                ],
            );

            assert_eq!(broadcasts.len(), 1, "the unchanged daemon is ignored");
        }

        #[test]
        fn an_unavailable_event_marks_the_daemon_and_broadcasts_when_system_routing_reads_it() {
            let mut rig = Rig::new();
            let daemon = assumed_daemon();

            let (broadcasts, _) = deliver(
                &mut rig,
                &daemon,
                &config_with(system_config()),
                vec![Osc99DbusEvent::Unavailable],
            );

            assert_eq!(daemon.get(), Osc99LinuxDaemon::Unavailable);
            assert_eq!(
                broadcasts.len(),
                1,
                "System routing stops advertising OSC 99"
            );
        }

        #[test]
        fn an_unavailable_event_under_toast_routing_updates_the_daemon_without_a_broadcast() {
            let mut rig = Rig::new();
            let daemon = assumed_daemon();

            let (broadcasts, _) = deliver(
                &mut rig,
                &daemon,
                &config_with(config(NotificationRouting::Toast)),
                vec![Osc99DbusEvent::Unavailable],
            );

            assert_eq!(daemon.get(), Osc99LinuxDaemon::Unavailable);
            assert_eq!(broadcasts, Vec::new(), "the toast leg does not read it");
        }
    }

    // ── drain_focus_requests ────────────────────────────────────────────

    /// A fake set of windows: which panes exist in which window, and what was
    /// raised.
    #[derive(Default)]
    struct FakeWindows {
        panes: HashMap<WindowId, Vec<PaneId>>,
        focused: Vec<Osc99Origin>,
        raised: Vec<WindowId>,
    }

    impl Osc99FocusTargets for FakeWindows {
        fn focus_pane(&mut self, origin: Osc99Origin) -> Option<PaneFocusOutcome> {
            let panes = self.panes.get(&origin.window_id)?;
            if panes.contains(&origin.pane_id) {
                self.focused.push(origin);
                Some(PaneFocusOutcome::Focused)
            } else {
                Some(PaneFocusOutcome::PaneNotFound)
            }
        }

        fn raise_window(&mut self, window_id: WindowId) {
            self.raised.push(window_id);
        }
    }

    fn origin(window: u64, pane_id: PaneId) -> Osc99Origin {
        Osc99Origin {
            window_id: WindowId::for_test(window),
            pane_id,
        }
    }

    #[test]
    fn a_focus_request_focuses_the_pane_and_raises_its_window() {
        let pane = PaneIdGenerator::new(0).next_id();
        let mut windows = FakeWindows::default();
        windows.panes.insert(WindowId::for_test(1), vec![pane]);

        drain_focus_requests(vec![origin(1, pane)], &mut windows);

        assert_eq!(windows.focused, [origin(1, pane)]);
        assert_eq!(windows.raised, [WindowId::for_test(1)]);
    }

    #[test]
    fn a_focus_request_for_a_missing_window_is_skipped() {
        let pane = PaneIdGenerator::new(0).next_id();
        let mut windows = FakeWindows::default();
        windows.panes.insert(WindowId::for_test(1), vec![pane]);

        drain_focus_requests(vec![origin(9, pane)], &mut windows);

        assert_eq!(windows.focused, Vec::new());
        assert_eq!(windows.raised, Vec::new(), "no window to raise");
    }

    #[test]
    fn a_focus_request_for_a_missing_pane_does_not_raise_the_window() {
        let mut ids = PaneIdGenerator::new(0);
        let (present, gone) = (ids.next_id(), ids.next_id());
        let mut windows = FakeWindows::default();
        windows.panes.insert(WindowId::for_test(1), vec![present]);

        drain_focus_requests(vec![origin(1, gone)], &mut windows);

        assert_eq!(windows.focused, Vec::new());
        assert_eq!(windows.raised, Vec::new());
    }

    #[test]
    fn focus_requests_are_performed_in_order_and_a_skipped_one_does_not_stop_the_rest() {
        let mut ids = PaneIdGenerator::new(0);
        let (first, second) = (ids.next_id(), ids.next_id());
        let mut windows = FakeWindows::default();
        windows.panes.insert(WindowId::for_test(1), vec![first]);
        windows.panes.insert(WindowId::for_test(2), vec![second]);

        drain_focus_requests(
            vec![origin(7, first), origin(2, second), origin(1, first)],
            &mut windows,
        );

        assert_eq!(windows.focused, [origin(2, second), origin(1, first)]);
        assert_eq!(
            windows.raised,
            [WindowId::for_test(2), WindowId::for_test(1)]
        );
    }
}
