//! [`DesktopNotifier`]: the one owner of the operating-system notification
//! backends.
//!
//! A caller holds one [`DesktopNotifier`] and never names a platform. It owns:
//!
//! - the Linux/BSD D-Bus service, started on demand and kept for the rest of
//!   the process (dropping the notifier stops it);
//! - the macOS and Windows untracked backend, whose delivery worker joins and
//!   removes its icon temp files when the notifier is dropped.
//!
//! Dropping the notifier drops the service first and the untracked backend
//! second, which is the order the GUI's two separate fields had.
//!
//! Every method exists on every platform. Where a platform has no such
//! backend the method does what a missing backend does: nothing is started,
//! the tracked backend is [`NoTrackedBackend`], and
//! plain notifications go to the platform worker. The platform split lives in
//! the private `platform` module below, as free functions: a method that read
//! nothing from `self` on one platform would only exist to make call sites
//! uniform.
//!
//! The policy that decides whether the service is wanted stays with the
//! caller, who passes it in as a [`ServiceDemand`]: the notifier only obeys.
//!
//! A cell that is borrowed elsewhere never panics. It makes the notifier fall
//! back (retry on the next call, or the no-op backend), because showing a
//! notification calls nothing back into the GUI and a busy cell is therefore
//! transient.

use std::cell::RefCell;

use super::assets::NotifyAssets;
use super::backend::{NoTrackedBackend, TrackedBackend};
#[cfg(all(unix, not(target_os = "macos")))]
use super::linux::{LinuxNotifyHandle, LinuxNotifyService, SystemNotifyEvent};
use super::plain_sink::PlainSink;
use super::untracked::{NoUntrackedBackend, UntrackedBackend, platform_untracked_backend};

/// Whether the caller's configuration wants the notification service running.
///
/// The notifier does not decide this; it obeys. Off Linux/BSD there is no
/// service, so the demand is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceDemand {
    /// The configuration can route a notification to the service: start it.
    Wanted,
    /// Nothing can reach the service: do not start it.
    NotWanted,
}

/// Makes a waker: the callback the service calls after every event so an idle
/// GUI drains it. A factory rather than one waker so the service can be
/// started at the moment it is first wanted, not when the notifier is built.
type WakerFactory = Box<dyn Fn() -> Box<dyn Fn() + Send>>;

/// Owns the OS notification backends. See the module docs.
pub struct DesktopNotifier {
    // Declared first, so it drops first.
    service: platform::Service,
    untracked: RefCell<Box<dyn UntrackedBackend>>,
}

impl DesktopNotifier {
    /// A notifier with no service started. `assets` are the bytes the
    /// backends draw with. `waker` makes the callback the service uses to
    /// wake an idle GUI; it is called when the service starts, and never off
    /// Linux/BSD.
    #[must_use]
    pub fn new(assets: NotifyAssets, waker: impl Fn() -> Box<dyn Fn() + Send> + 'static) -> Self {
        Self {
            service: platform::Service::new(assets, Box::new(waker)),
            untracked: RefCell::new(platform_untracked_backend(assets)),
        }
    }

    /// Start the service if `demand` is [`ServiceDemand::Wanted`] and it is
    /// not running. Calling this again once it runs is a no-op, and so is
    /// calling it off Linux/BSD.
    ///
    /// A busy handle cell leaves the service unstarted; the next call
    /// retries, and until then tracked requests fail.
    pub fn ensure_started(&self, demand: ServiceDemand) {
        platform::ensure_started(&self.service, demand);
    }

    /// Where a plain notification (OSC 9 / OSC 777 / command-finished / the
    /// Settings "Test Notification" button) goes to be shown.
    ///
    /// On Linux/BSD, a running service gives a sink that reaches it, without
    /// asking `demand`. Otherwise `demand` is asked once: if the service is
    /// [`Wanted`](ServiceDemand::Wanted) it starts (once) and the sink
    /// reaches it; if not, the sink drops what it is given. A busy handle
    /// cell, whether to look or to store the started service, gives that same
    /// empty sink (logged at debug) and starts nothing. Elsewhere the sink is
    /// the platform worker and `demand` is never asked.
    ///
    /// A running service costs a shared borrow of the handle cell and one
    /// `Sender` clone; the cell is borrowed mutably only to start the
    /// service.
    #[must_use]
    pub fn plain_sink(&self, demand: impl FnOnce() -> ServiceDemand) -> PlainSink {
        platform::plain_sink(&self.service, demand)
    }

    /// Run `f` with the tracked and the untracked backend.
    ///
    /// The tracked backend is the running service. It is
    /// [`NoTrackedBackend`], which fails every request, when the service is
    /// not running, its cell is busy, or the platform has no service. The
    /// untracked backend is the platform's, or
    /// [`NoUntrackedBackend`] when its cell is busy. A busy cell cannot
    /// happen in practice, but if it did the untracked show would be lost,
    /// not deferred: that leg has no failure to feed back.
    ///
    /// The cells stay borrowed while `f` runs, so `f` must not call back into
    /// this notifier.
    pub fn with_backends<R>(
        &self,
        f: impl FnOnce(&mut dyn TrackedBackend, &mut dyn UntrackedBackend) -> R,
    ) -> R {
        let mut no_untracked = NoUntrackedBackend;
        let mut untracked_slot = self.untracked.try_borrow_mut().ok();
        let untracked: &mut dyn UntrackedBackend = match untracked_slot.as_deref_mut() {
            Some(backend) => backend.as_mut(),
            None => &mut no_untracked,
        };
        platform::with_tracked(&self.service, |system| f(system, untracked))
    }

    /// Whether the service has been started. A busy handle cell counts as not
    /// started.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[must_use]
    pub fn is_started(&self) -> bool {
        platform::is_started(&self.service)
    }

    /// The events the service queued since the last call, oldest first. Empty
    /// when it is not running or its handle cell is busy; nothing is taken
    /// from a busy cell, so nothing is lost. Never blocks.
    ///
    /// The events exist only where the service does, so this method does too.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[must_use]
    pub fn drain_events(&self) -> Vec<SystemNotifyEvent> {
        platform::drain_events(&self.service)
    }
}

/// The Linux/BSD side: the D-Bus service, in a cell.
#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use std::cell::RefCell;

    use super::{
        LinuxNotifyHandle, LinuxNotifyService, NoTrackedBackend, NotifyAssets, PlainSink,
        ServiceDemand, SystemNotifyEvent, TrackedBackend, WakerFactory,
    };

    /// The service's handle cell and what is needed to start it.
    ///
    /// Single-threaded GUI, so `RefCell` suffices.
    pub struct Service {
        /// `None` until the service is started; kept for the rest of the
        /// process.
        pub(super) slot: RefCell<Option<LinuxNotifyHandle>>,
        assets: NotifyAssets,
        waker: WakerFactory,
    }

    impl Service {
        pub fn new(assets: NotifyAssets, waker: WakerFactory) -> Self {
            Self {
                slot: RefCell::new(None),
                assets,
                waker,
            }
        }

        /// Start the service, whose events are drained by the frame loop and
        /// whose every event wakes an idle window.
        fn spawn(&self) -> LinuxNotifyHandle {
            tracing::debug!("starting the desktop-notification service");
            LinuxNotifyService::spawn((self.waker)(), self.assets)
        }
    }

    pub fn ensure_started(service: &Service, demand: ServiceDemand) {
        ensure_started_with(&service.slot, demand, || service.spawn());
    }

    pub(super) fn ensure_started_with(
        slot: &RefCell<Option<LinuxNotifyHandle>>,
        demand: ServiceDemand,
        spawn: impl FnOnce() -> LinuxNotifyHandle,
    ) {
        if demand == ServiceDemand::NotWanted {
            return;
        }
        let Ok(mut running) = slot.try_borrow_mut() else {
            return;
        };
        running.get_or_insert_with(spawn);
    }

    pub fn plain_sink(service: &Service, demand: impl FnOnce() -> ServiceDemand) -> PlainSink {
        resolve_plain_sink(&service.slot, demand, || service.spawn())
    }

    /// The decision behind `DesktopNotifier::plain_sink`, over the handle
    /// cell.
    pub(super) fn resolve_plain_sink(
        slot: &RefCell<Option<LinuxNotifyHandle>>,
        demand: impl FnOnce() -> ServiceDemand,
        spawn: impl FnOnce() -> LinuxNotifyHandle,
    ) -> PlainSink {
        let busy = || {
            tracing::debug!(
                "desktop-notification service handle busy; system notifications dropped"
            );
            PlainSink::nowhere()
        };
        match slot.try_borrow() {
            Ok(running) => {
                if let Some(handle) = running.as_ref() {
                    return PlainSink::through_service(handle.plain_sender());
                }
            }
            Err(_) => return busy(),
        }
        if demand() == ServiceDemand::NotWanted {
            return PlainSink::nowhere();
        }
        let Ok(mut running) = slot.try_borrow_mut() else {
            return busy();
        };
        let handle = running.get_or_insert_with(spawn);
        PlainSink::through_service(handle.plain_sender())
    }

    pub fn with_tracked<R>(service: &Service, f: impl FnOnce(&mut dyn TrackedBackend) -> R) -> R {
        let mut no_system = NoTrackedBackend;
        let mut running = service.slot.try_borrow_mut().ok();
        let system: &mut dyn TrackedBackend = match running.as_mut().and_then(|slot| slot.as_mut())
        {
            Some(handle) => handle,
            None => &mut no_system,
        };
        f(system)
    }

    pub fn is_started(service: &Service) -> bool {
        service.slot.try_borrow().is_ok_and(|slot| slot.is_some())
    }

    pub fn drain_events(service: &Service) -> Vec<SystemNotifyEvent> {
        service.slot.try_borrow().map_or_else(
            |_| Vec::new(),
            |slot| {
                slot.as_ref()
                    .map_or_else(Vec::new, |handle| handle.try_events().collect())
            },
        )
    }
}

/// The macOS and Windows side: no service. The untracked backend is the
/// notifier's own field, and plain notifications go to the process-wide
/// worker.
#[cfg(not(all(unix, not(target_os = "macos"))))]
mod platform {
    use super::{
        NoTrackedBackend, NotifyAssets, PlainSink, ServiceDemand, TrackedBackend, WakerFactory,
    };

    /// There is no service to hold.
    pub struct Service;

    impl Service {
        pub fn new(_assets: NotifyAssets, _waker: WakerFactory) -> Self {
            Self
        }
    }

    pub fn ensure_started(_service: &Service, demand: ServiceDemand) {
        tracing::trace!(?demand, "no desktop-notification service on this platform");
    }

    pub fn plain_sink(_service: &Service, _demand: impl FnOnce() -> ServiceDemand) -> PlainSink {
        PlainSink::platform_worker()
    }

    pub fn with_tracked<R>(_service: &Service, f: impl FnOnce(&mut dyn TrackedBackend) -> R) -> R {
        f(&mut NoTrackedBackend)
    }
}

#[cfg(test)]
mod tests {
    use freminal_common::buffer_states::osc99_notification::{
        Notification99Data, NotificationOccasion, NotificationUrgency, Osc99ActivationFocus,
        Osc99ActivationReporting, Osc99CloseReporting, Osc99Expiry, Osc99Sound,
    };

    use super::super::assets::test_assets;
    use super::super::backend::{NotificationInstanceId, SystemDispatch};
    use super::super::untracked::RecordingUntrackedBackend;
    use super::*;

    fn notification() -> Notification99Data {
        Notification99Data {
            id: Some("n".to_owned()),
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

    fn instance(n: u64) -> NotificationInstanceId {
        NotificationInstanceId::new(n)
    }

    /// A recording backend that can sit behind the notifier's `Box<dyn>` cell
    /// while the test reads what it recorded.
    #[derive(Clone, Default)]
    struct Shared(std::rc::Rc<RefCell<RecordingUntrackedBackend>>);

    impl UntrackedBackend for Shared {
        fn show(&mut self, notification: &Notification99Data) {
            self.0.borrow_mut().show(notification);
        }
    }

    /// A notifier whose untracked backend is `untracked`, with no service.
    fn notifier_with(untracked: Box<dyn UntrackedBackend>) -> DesktopNotifier {
        let notifier = DesktopNotifier::new(test_assets(), || Box::new(|| {}));
        *notifier.untracked.borrow_mut() = untracked;
        notifier
    }

    // ── with_backends: the untracked backend ────────────────────────────

    #[test]
    fn with_backends_hands_out_the_untracked_backend() {
        let shared = Shared::default();
        let notifier = notifier_with(Box::new(shared.clone()));

        notifier.with_backends(|_, untracked| untracked.show(&notification()));

        assert_eq!(shared.0.borrow().shown, [notification()]);
    }

    #[test]
    fn with_backends_falls_back_to_the_no_op_untracked_backend_when_its_cell_is_busy() {
        let shared = Shared::default();
        let notifier = notifier_with(Box::new(shared.clone()));
        let held = notifier.untracked.borrow_mut();

        let ran = notifier.with_backends(|_, untracked| {
            untracked.show(&notification());
            true
        });
        drop(held);

        assert!(ran, "the closure still runs");
        assert_eq!(shared.0.borrow().shown, [], "the busy backend was not used");
    }

    // ── with_backends: the tracked backend ──────────────────────────────

    #[test]
    fn with_backends_without_a_started_service_fails_tracked_requests() {
        let notifier = notifier_with(Box::new(NoUntrackedBackend));

        let dispatch =
            notifier.with_backends(|system, _| system.show(instance(1), None, &notification()));

        assert_eq!(dispatch, SystemDispatch::Failed);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    mod service {
        use std::cell::Cell;

        use super::super::super::linux::{DetachedQueue, detached_handle};
        use super::super::super::plain_sink::PlainNotification;
        use super::*;

        /// A notifier whose service cell holds `handle`.
        fn running(handle: LinuxNotifyHandle) -> DesktopNotifier {
            let notifier = notifier_with(Box::new(NoUntrackedBackend));
            *notifier.service.slot.borrow_mut() = Some(handle);
            notifier
        }

        /// Show one plain notification through `sink` and report how many
        /// reached the service behind `queue`.
        fn show_one(sink: &PlainSink, queue: &DetachedQueue) -> usize {
            sink.show(PlainNotification {
                summary: "Title".to_owned(),
                body: "Body".to_owned(),
                urgency: NotificationUrgency::Normal,
            });
            queue.take_plain().len()
        }

        #[test]
        fn with_backends_hands_out_the_running_service() {
            let (handle, _queue) = detached_handle();
            let notifier = running(handle);

            let dispatch =
                notifier.with_backends(|system, _| system.show(instance(1), None, &notification()));

            assert_eq!(dispatch, SystemDispatch::Queued);
        }

        #[test]
        fn with_backends_falls_back_to_the_no_op_tracked_backend_when_the_handle_cell_is_busy() {
            let (handle, _queue) = detached_handle();
            let notifier = running(handle);
            let held = notifier.service.slot.borrow_mut();

            let dispatch =
                notifier.with_backends(|system, _| system.show(instance(1), None, &notification()));
            drop(held);

            assert_eq!(dispatch, SystemDispatch::Failed);
        }

        // ── ensure_started ──────────────────────────────────────────────

        #[test]
        fn a_wanted_service_is_started_once_and_kept() {
            let slot = RefCell::new(None);
            let spawned = Cell::new(0_u32);
            let spawn = || {
                spawned.set(spawned.get() + 1);
                detached_handle().0
            };

            platform::ensure_started_with(&slot, ServiceDemand::Wanted, spawn);
            platform::ensure_started_with(&slot, ServiceDemand::Wanted, spawn);

            assert_eq!(spawned.get(), 1);
            assert!(slot.borrow().is_some(), "the cell now holds the handle");
        }

        #[test]
        fn a_service_that_is_not_wanted_is_not_started() {
            let slot = RefCell::new(None);
            let spawned = Cell::new(0_u32);

            platform::ensure_started_with(&slot, ServiceDemand::NotWanted, || {
                spawned.set(spawned.get() + 1);
                detached_handle().0
            });

            assert_eq!(spawned.get(), 0);
            assert!(slot.borrow().is_none(), "nothing was stored");
        }

        #[test]
        fn a_busy_handle_cell_starts_nothing_and_the_next_call_retries() {
            let slot = RefCell::new(None);
            let spawned = Cell::new(0_u32);
            let spawn = || {
                spawned.set(spawned.get() + 1);
                detached_handle().0
            };

            let held = slot.borrow_mut();
            platform::ensure_started_with(&slot, ServiceDemand::Wanted, spawn);
            assert_eq!(spawned.get(), 0);
            drop(held);

            platform::ensure_started_with(&slot, ServiceDemand::Wanted, spawn);
            assert_eq!(spawned.get(), 1, "the retry started it");
        }

        // ── plain_sink ──────────────────────────────────────────────────

        #[test]
        fn a_busy_handle_cell_gives_an_empty_sink_and_starts_nothing() {
            let (handle, queue) = detached_handle();
            let slot = RefCell::new(Some(handle));
            let spawned = Cell::new(0_u32);
            let _held = slot.borrow_mut();

            let sink = platform::resolve_plain_sink(
                &slot,
                || ServiceDemand::Wanted,
                || {
                    spawned.set(spawned.get() + 1);
                    detached_handle().0
                },
            );

            assert_eq!(show_one(&sink, &queue), 0);
            assert_eq!(spawned.get(), 0);
        }

        #[test]
        fn a_running_service_gives_a_carrying_sink_without_asking_or_spawning() {
            let (handle, queue) = detached_handle();
            let slot = RefCell::new(Some(handle));
            let (asked, spawned) = (Cell::new(0_u32), Cell::new(0_u32));

            let sink = platform::resolve_plain_sink(
                &slot,
                || {
                    asked.set(asked.get() + 1);
                    ServiceDemand::NotWanted
                },
                || {
                    spawned.set(spawned.get() + 1);
                    detached_handle().0
                },
            );

            assert_eq!(show_one(&sink, &queue), 1);
            assert_eq!((asked.get(), spawned.get()), (0, 0));
        }

        #[test]
        fn a_service_that_is_not_running_and_not_wanted_is_not_started() {
            let slot = RefCell::new(None);
            let (_unused, queue) = detached_handle();
            let spawned = Cell::new(0_u32);

            let sink = platform::resolve_plain_sink(
                &slot,
                || ServiceDemand::NotWanted,
                || {
                    spawned.set(spawned.get() + 1);
                    detached_handle().0
                },
            );

            assert_eq!(show_one(&sink, &queue), 0);
            assert_eq!(spawned.get(), 0);
            assert!(slot.borrow().is_none(), "nothing was stored");
        }

        #[test]
        fn a_wanted_service_is_started_once_and_kept_for_plain_notifications() {
            let slot = RefCell::new(None);
            let (spawned_handle, queue) = detached_handle();
            let spawned = Cell::new(0_u32);

            let sink = platform::resolve_plain_sink(
                &slot,
                || ServiceDemand::Wanted,
                || {
                    spawned.set(spawned.get() + 1);
                    spawned_handle
                },
            );

            assert_eq!(spawned.get(), 1);
            assert!(slot.borrow().is_some(), "the cell now holds the handle");
            assert_eq!(show_one(&sink, &queue), 1, "the sink reaches it");

            // A second call finds it running and does not spawn another.
            let again = platform::resolve_plain_sink(
                &slot,
                || ServiceDemand::Wanted,
                || {
                    spawned.set(spawned.get() + 1);
                    detached_handle().0
                },
            );
            assert_eq!(spawned.get(), 1);
            assert_eq!(show_one(&again, &queue), 1);
        }

        #[test]
        fn plain_sink_on_a_notifier_with_a_running_service_reaches_it() {
            let (handle, queue) = detached_handle();
            let notifier = running(handle);

            let sink = notifier.plain_sink(|| ServiceDemand::NotWanted);

            assert_eq!(show_one(&sink, &queue), 1);
        }

        #[test]
        fn plain_sink_on_a_notifier_that_is_not_wanted_drops_notifications() {
            let notifier = notifier_with(Box::new(NoUntrackedBackend));

            let sink = notifier.plain_sink(|| ServiceDemand::NotWanted);

            // Reaches no service and starts none.
            sink.show(PlainNotification {
                summary: "Title".to_owned(),
                body: "Body".to_owned(),
                urgency: NotificationUrgency::Normal,
            });
            assert!(!notifier.is_started());
        }

        // ── is_started / drain_events ───────────────────────────────────

        #[test]
        fn a_service_that_is_not_started_has_no_events() {
            let notifier = notifier_with(Box::new(NoUntrackedBackend));

            assert!(!notifier.is_started());
            assert_eq!(notifier.drain_events(), []);
        }

        #[test]
        fn a_running_service_with_nothing_queued_has_no_events() {
            let (handle, _queue) = detached_handle();
            let notifier = running(handle);

            assert!(notifier.is_started());
            assert_eq!(notifier.drain_events(), []);
        }

        #[test]
        fn a_busy_handle_cell_counts_as_not_started_and_yields_no_events() {
            let (handle, _queue) = detached_handle();
            let notifier = running(handle);
            let _held = notifier.service.slot.borrow_mut();

            assert!(!notifier.is_started());
            assert_eq!(notifier.drain_events(), []);
        }
    }
}
