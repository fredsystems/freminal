//! The OSC 99 notification registry.
//!
//! Every OSC 99 notification that is actually displayed (after the `o=`
//! occasion check and the routing decision) becomes an *instance* here. The
//! registry is a pure state machine: its methods take inputs, mutate only the
//! registry itself, and return [`Osc99Effect`]s for the caller to execute
//! (see [`apply_osc99_effects`](super::apply_osc99_effects)). It does no I/O.
//!
//! # Scoping
//!
//! Instances are scoped per pane. An id (`i=`) is only meaningful within the
//! pane that sent it: `p=alive` lists, and `p=close` and a re-registration
//! replace, only the requesting pane's instances. This differs from kitty,
//! whose `p=close` looks across all windows. [`PaneId`] is globally unique
//! across windows, so `origin.pane_id` alone identifies a pane.
//!
//! # Legs and close reports
//!
//! An instance tracks the legs still displayed (`open_legs`) and the legs it
//! was ever shown on (`shown_legs`). It is forgotten when its last open leg
//! closes, and a single `p=close` report is then sent if `c=1` was requested.
//! Only the toast leg is registry-managed so far. The Linux system leg is
//! still driven by its own per-notification thread, which sends its own
//! activation and close reports, so the registry never reports for it and
//! never sends a close report for an instance that had one. An instance shown
//! on an untracked system leg likewise never gets a registry close report:
//! the `untracked` report was already sent at display.
//!
//! # Pane lifetime
//!
//! An instance remembers the pane's `Weak` reply handle. Once the pane is
//! gone the handle no longer upgrades and [`Osc99Registry::prune_dead_panes`]
//! drops the instance. Notifications already on screen simply stay there and
//! produce no replies.
//!
//! # Capacity
//!
//! At most [`MAX_OSC99_INSTANCES`] instances are kept. When a new one would
//! exceed that, the oldest is forgotten without a reply (its toast, if open,
//! is closed), like kitty.

use std::sync::Weak;

use crossbeam_channel::Sender;
use freminal_common::buffer_states::osc99_notification::{
    Osc99ActivationFocus, Osc99ActivationReporting, Osc99CloseReporting,
};
use freminal_common::buffer_states::window_manipulation::Notification99Data;
use freminal_terminal_emulator::io::{GuiReply, InputEvent, Osc99CloseTracking};

use super::{Osc99Effect, Osc99InstanceId, Osc99Leg, Osc99Origin, osc99_toast_spec};
use crate::gui::panes::PaneId;
use crate::gui::toast::{ToastEvent, ToastEventKind};

/// The most instances the registry keeps. The oldest is forgotten, with no
/// reply, when a new registration would exceed this.
pub const MAX_OSC99_INSTANCES: usize = 128;

/// One displayed OSC 99 notification.
#[derive(Debug, Clone)]
pub struct Osc99Instance {
    /// The registry-assigned identity, unique for the process lifetime.
    pub instance: Osc99InstanceId,
    /// The application's `i=` id, if the notification carried one.
    pub id: Option<String>,
    /// The pane the notification came from.
    pub origin: Osc99Origin,
    /// A `Weak` handle to the pane's reply sender; also the pane's liveness.
    pub reply_tx: Weak<Sender<InputEvent>>,
    /// Whether `a=report` was requested.
    pub activation_reporting: Osc99ActivationReporting,
    /// Whether activation should focus the pane.
    pub activation_focus: Osc99ActivationFocus,
    /// Whether `c=1` was requested.
    pub close_reporting: Osc99CloseReporting,
    /// Every leg the notification was shown on, without duplicates. Never
    /// shrinks.
    pub shown_legs: Vec<Osc99Leg>,
    /// The legs still displayed and observable, without duplicates. Never
    /// holds [`Osc99Leg::UntrackedSystem`].
    pub open_legs: Vec<Osc99Leg>,
}

impl Osc99Instance {
    /// Whether this instance belongs to `pane` and carries exactly `id`.
    fn is_pane_id(&self, pane: PaneId, id: &str) -> bool {
        self.origin.pane_id == pane && self.id.as_deref() == Some(id)
    }

    /// Whether `leg` is still displayed.
    fn is_open(&self, leg: Osc99Leg) -> bool {
        self.open_legs.contains(&leg)
    }

    /// The reply for this instance's activation, if `a=report` was requested.
    fn activation_reply(&self) -> Option<Osc99Effect> {
        (self.activation_reporting == Osc99ActivationReporting::Report).then(|| {
            Osc99Effect::Reply {
                reply_tx: Weak::clone(&self.reply_tx),
                reply: GuiReply::Osc99Activation {
                    id: self.id.clone(),
                    button: None,
                },
            }
        })
    }

    /// The `p=close` report for this instance, if the registry owes one:
    /// `c=1` was requested and no system leg reports for itself (see the
    /// module docs).
    fn close_reply(&self) -> Option<Osc99Effect> {
        let externally_reported = self
            .shown_legs
            .iter()
            .any(|leg| matches!(leg, Osc99Leg::LinuxSystem | Osc99Leg::UntrackedSystem));
        (self.close_reporting == Osc99CloseReporting::Report && !externally_reported).then(|| {
            Osc99Effect::Reply {
                reply_tx: Weak::clone(&self.reply_tx),
                reply: GuiReply::Osc99Closed {
                    id: self.id.clone(),
                    tracking: Osc99CloseTracking::Tracked,
                },
            }
        })
    }

    /// The effect that closes this instance's toast, if it is open.
    fn close_toast_effect(&self) -> Option<Osc99Effect> {
        self.is_open(Osc99Leg::Toast)
            .then_some(Osc99Effect::CloseToast(self.instance))
    }

    /// Record in the trace log that this instance left the registry.
    fn log_forgotten(&self, why: &'static str) {
        tracing::trace!(
            instance = self.instance.0,
            id = ?self.id,
            origin = ?self.origin,
            activation_focus = ?self.activation_focus,
            shown_legs = ?self.shown_legs,
            open_legs = ?self.open_legs,
            why,
            "forgetting OSC 99 instance"
        );
    }
}

/// The set of displayed OSC 99 notifications, in registration order.
#[derive(Debug, Default)]
pub struct Osc99Registry {
    /// Instances, oldest first.
    instances: Vec<Osc99Instance>,
    /// The raw value the next [`Osc99InstanceId`] is built from.
    next_instance: u64,
}

impl Osc99Registry {
    /// Record a displayed notification and return the effects that show it.
    ///
    /// Callers register only when at least one leg displayed it; `legs`
    /// lists the legs that did. An instance with no legs is legal but never
    /// produced by the router. The toast leg is not shown by the caller: a
    /// `Toast` in `legs` yields a [`Osc99Effect::ShowToast`] here.
    ///
    /// If `data.id` is set and the same pane already has an instance with
    /// that id, the notification is an update: the old instance is forgotten
    /// without a close report, and its open toast is replaced in place (or
    /// closed, when `legs` has no toast). If the registry would then exceed
    /// [`MAX_OSC99_INSTANCES`], the oldest instance is forgotten, again
    /// without a reply, and its open toast is closed.
    pub fn register(
        &mut self,
        origin: Osc99Origin,
        reply_tx: Weak<Sender<InputEvent>>,
        data: &Notification99Data,
        legs: &[Osc99Leg],
    ) -> Vec<Osc99Effect> {
        let mut effects = Vec::new();
        let wants_toast = legs.contains(&Osc99Leg::Toast);

        let mut replaces = None;
        if let Some(id) = data.id.as_deref()
            && let Some(pos) = self
                .instances
                .iter()
                .position(|instance| instance.is_pane_id(origin.pane_id, id))
        {
            let old = self.instances.remove(pos);
            old.log_forgotten("replaced by a notification with the same id");
            if old.is_open(Osc99Leg::Toast) {
                if wants_toast {
                    replaces = Some(old.instance);
                } else {
                    // The old toast's `Closed` event arrives for a forgotten
                    // instance and is ignored, so no close is reported.
                    effects.push(Osc99Effect::CloseToast(old.instance));
                }
            }
        }

        let instance = Osc99InstanceId(self.next_instance);
        self.next_instance = self.next_instance.wrapping_add(1);

        let mut deduped: Vec<Osc99Leg> = Vec::with_capacity(legs.len());
        for leg in legs {
            if !deduped.contains(leg) {
                deduped.push(*leg);
            }
        }

        if wants_toast {
            effects.push(Osc99Effect::ShowToast {
                instance,
                replaces,
                spec: osc99_toast_spec(data),
            });
        }

        self.instances.push(Osc99Instance {
            instance,
            id: data.id.clone(),
            origin,
            reply_tx,
            activation_reporting: data.activation_reporting,
            activation_focus: data.activation_focus,
            close_reporting: data.close_reporting,
            // An untracked system notification can never be observed closing,
            // so it is never an open leg the instance could wait on.
            open_legs: deduped
                .iter()
                .copied()
                .filter(|leg| *leg != Osc99Leg::UntrackedSystem)
                .collect(),
            shown_legs: deduped,
        });

        while self.instances.len() > MAX_OSC99_INSTANCES {
            let oldest = self.instances.remove(0);
            oldest.log_forgotten("registry full; oldest forgotten");
            effects.extend(oldest.close_toast_effect());
        }

        effects
    }

    /// Answer a `p=alive` poll from `origin`'s pane.
    ///
    /// The reply lists the ids of that pane's instances that carry an id,
    /// sorted and without duplicates. Other panes' instances are never
    /// listed.
    #[must_use]
    pub fn alive(
        &self,
        origin: Osc99Origin,
        request_id: String,
        reply_tx: Weak<Sender<InputEvent>>,
    ) -> Vec<Osc99Effect> {
        let mut live_ids: Vec<String> = self
            .instances
            .iter()
            .filter(|instance| instance.origin.pane_id == origin.pane_id)
            .filter_map(|instance| instance.id.clone())
            .collect();
        live_ids.sort();
        live_ids.dedup();

        vec![Osc99Effect::Reply {
            reply_tx,
            reply: GuiReply::Osc99Alive {
                request_id: Some(request_id),
                live_ids,
            },
        }]
    }

    /// Handle a `p=close` for `id` from `origin`'s pane.
    ///
    /// An id the pane does not have is a no-op. An open toast is asked to
    /// close; the instance stays until the toast's `Closed` event arrives
    /// (see [`Self::on_toast_event`]), which is when a `c=1` report is sent.
    /// The Linux and untracked system legs are dropped silently (the Linux
    /// leg's own thread reports for it). With no toast to wait for, the
    /// instance is forgotten at once, without a report.
    pub fn close(&mut self, origin: Osc99Origin, id: &str) -> Vec<Osc99Effect> {
        let Some(pos) = self
            .instances
            .iter()
            .position(|instance| instance.is_pane_id(origin.pane_id, id))
        else {
            return Vec::new();
        };

        let effects: Vec<Osc99Effect> = self.instances[pos]
            .close_toast_effect()
            .into_iter()
            .collect();
        self.instances[pos]
            .open_legs
            .retain(|leg| *leg == Osc99Leg::Toast);
        if effects.is_empty() {
            self.instances
                .remove(pos)
                .log_forgotten("closed by the application");
        }
        effects
    }

    /// Apply an event from the toast stack.
    ///
    /// An event for an unknown instance (replaced, evicted, forgotten, or
    /// its pane pruned) is ignored.
    ///
    /// - `Activated` sends the activation report when `a=report` was
    ///   requested, and forgets the instance unless `c=1` was requested (the
    ///   toast's `Closed` event follows either way).
    /// - `Closed` closes the toast leg; once no leg is open the instance is
    ///   forgotten and a close report is sent when owed.
    pub fn on_toast_event(&mut self, event: ToastEvent) -> Vec<Osc99Effect> {
        let instance = Osc99InstanceId::from_toast_key(event.key);
        let Some(pos) = self
            .instances
            .iter()
            .position(|candidate| candidate.instance == instance)
        else {
            return Vec::new();
        };

        match event.kind {
            ToastEventKind::Activated => {
                let effects: Vec<Osc99Effect> =
                    self.instances[pos].activation_reply().into_iter().collect();
                if self.instances[pos].close_reporting == Osc99CloseReporting::Silent {
                    self.instances
                        .remove(pos)
                        .log_forgotten("activated without close reporting");
                }
                effects
            }
            ToastEventKind::Closed => {
                self.instances[pos]
                    .open_legs
                    .retain(|leg| *leg != Osc99Leg::Toast);
                self.finish_if_closed(pos)
            }
        }
    }

    /// Forget the instance at `pos` if no leg is open, returning its close
    /// report when one is owed.
    fn finish_if_closed(&mut self, pos: usize) -> Vec<Osc99Effect> {
        if !self.instances[pos].open_legs.is_empty() {
            return Vec::new();
        }
        let finished = self.instances.remove(pos);
        finished.log_forgotten("last leg closed");
        finished.close_reply().into_iter().collect()
    }

    /// Drop every instance whose pane is gone (its `Weak` reply handle no
    /// longer upgrades).
    pub fn prune_dead_panes(&mut self) {
        self.instances.retain(|instance| {
            let alive = instance.reply_tx.strong_count() > 0;
            if !alive {
                instance.log_forgotten("originating pane is gone");
            }
            alive
        });
    }

    /// The instances, oldest first.
    #[cfg(test)]
    pub fn instances(&self) -> &[Osc99Instance] {
        &self.instances
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::time::Duration;

    use freminal_common::buffer_states::osc_notify_99::{
        NotificationOccasion, NotificationUrgency,
    };
    use freminal_common::buffer_states::osc99_notification::{Osc99Expiry, Osc99Sound};
    use freminal_windowing::WindowId;

    use super::*;
    use crate::gui::osc99::{Osc99EffectSink, Osc99ToastSpec, apply_osc99_effects};
    use crate::gui::panes::PaneIdGenerator;
    use crate::gui::toast::{
        ToastEvent, ToastEventKind, ToastKey, ToastKind, ToastLifetime, ToastStack,
    };

    /// A live pane: its origin plus the strong reply handle that keeps it
    /// alive. Dropping the handle simulates the pane closing.
    struct TestPane {
        origin: Osc99Origin,
        tx: Arc<Sender<InputEvent>>,
        _rx: crossbeam_channel::Receiver<InputEvent>,
    }

    impl TestPane {
        fn new(pane_id: PaneId) -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            Self {
                origin: Osc99Origin {
                    window_id: WindowId::for_test(1),
                    pane_id,
                },
                tx: Arc::new(tx),
                _rx: rx,
            }
        }

        fn weak(&self) -> Weak<Sender<InputEvent>> {
            Arc::downgrade(&self.tx)
        }
    }

    fn two_panes() -> (TestPane, TestPane) {
        let mut ids = PaneIdGenerator::new(0);
        (TestPane::new(ids.next_id()), TestPane::new(ids.next_id()))
    }

    fn data(id: Option<&str>) -> Notification99Data {
        Notification99Data {
            id: id.map(str::to_owned),
            title: "Title".to_owned(),
            body: None,
            icon_data: None,
            icon_names: Vec::new(),
            icon_cache_key: None,
            buttons: Vec::new(),
            activation_reporting: Osc99ActivationReporting::Silent,
            activation_focus: Osc99ActivationFocus::Focus,
            close_reporting: Osc99CloseReporting::Silent,
            urgency: NotificationUrgency::Normal,
            occasion: NotificationOccasion::Always,
            sound: Osc99Sound::System,
            app_name: None,
            notification_types: Vec::new(),
            expiry: Osc99Expiry::OsDefault,
        }
    }

    /// Data with `a=report` and the given `c=` setting.
    fn reporting_data(id: &str, close: Osc99CloseReporting) -> Notification99Data {
        Notification99Data {
            activation_reporting: Osc99ActivationReporting::Report,
            close_reporting: close,
            ..data(Some(id))
        }
    }

    fn register(registry: &mut Osc99Registry, pane: &TestPane, id: Option<&str>) {
        let _ = registry.register(pane.origin, pane.weak(), &data(id), &[Osc99Leg::Toast]);
    }

    /// Register `data` on `legs`, returning the effects and the new
    /// instance's id (the registry's newest instance).
    fn register_with(
        registry: &mut Osc99Registry,
        pane: &TestPane,
        data: &Notification99Data,
        legs: &[Osc99Leg],
    ) -> (Vec<Osc99Effect>, Osc99InstanceId) {
        let effects = registry.register(pane.origin, pane.weak(), data, legs);
        let Some(instance) = registry.instances().last().map(|i| i.instance) else {
            panic!("registration adds an instance");
        };
        (effects, instance)
    }

    fn activated(instance: Osc99InstanceId) -> ToastEvent {
        ToastEvent {
            key: instance.to_toast_key(),
            kind: ToastEventKind::Activated,
        }
    }

    fn closed(instance: Osc99InstanceId) -> ToastEvent {
        ToastEvent {
            key: instance.to_toast_key(),
            kind: ToastEventKind::Closed,
        }
    }

    /// The `GuiReply` of every `Reply` effect, panicking on any other effect.
    fn replies(effects: Vec<Osc99Effect>) -> Vec<GuiReply> {
        effects
            .into_iter()
            .map(|effect| match effect {
                Osc99Effect::Reply { reply, .. } => reply,
                other => panic!("expected only replies, got {other:?}"),
            })
            .collect()
    }

    fn is_tracked_close(reply: &GuiReply, id: &str) -> bool {
        matches!(
            reply,
            GuiReply::Osc99Closed { id: Some(i), tracking: Osc99CloseTracking::Tracked } if i == id
        )
    }

    fn is_activation(reply: &GuiReply, id: &str) -> bool {
        matches!(
            reply,
            GuiReply::Osc99Activation { id: Some(i), button: None } if i == id
        )
    }

    /// The ids in the single `p=alive` reply the registry produces.
    fn alive_ids(registry: &Osc99Registry, pane: &TestPane) -> Vec<String> {
        let mut effects = registry.alive(pane.origin, "req".to_owned(), pane.weak());
        assert_eq!(effects.len(), 1);
        match effects.remove(0) {
            Osc99Effect::Reply {
                reply:
                    GuiReply::Osc99Alive {
                        request_id,
                        live_ids,
                    },
                ..
            } => {
                assert_eq!(request_id.as_deref(), Some("req"));
                live_ids
            }
            other => panic!("expected an alive reply, got {other:?}"),
        }
    }

    fn ids_in_registry(registry: &Osc99Registry, pane: &TestPane) -> Vec<Option<String>> {
        registry
            .instances()
            .iter()
            .filter(|i| i.origin.pane_id == pane.origin.pane_id)
            .map(|i| i.id.clone())
            .collect()
    }

    // ── toast spec ──────────────────────────────────────────────────────

    fn spec_for(urgency: NotificationUrgency, expiry: Osc99Expiry) -> Osc99ToastSpec {
        osc99_toast_spec(&Notification99Data {
            urgency,
            expiry,
            ..data(None)
        })
    }

    #[test]
    fn toast_spec_follows_urgency_when_the_os_default_is_requested() {
        let critical = spec_for(NotificationUrgency::Critical, Osc99Expiry::OsDefault);
        assert_eq!(critical.kind, ToastKind::Error);
        assert_eq!(critical.lifetime, ToastLifetime::UntilDismissed);

        let normal = spec_for(NotificationUrgency::Normal, Osc99Expiry::OsDefault);
        assert_eq!(normal.kind, ToastKind::Info);
        assert_eq!(normal.lifetime, ToastLifetime::KindDefault);

        let low = spec_for(NotificationUrgency::Low, Osc99Expiry::OsDefault);
        assert_eq!(low.kind, ToastKind::Info);
        assert_eq!(
            low.lifetime,
            ToastLifetime::Fixed(Duration::from_secs(3)),
            "Low is half the 6 s Info default"
        );
    }

    #[test]
    fn toast_spec_explicit_expiry_wins_over_urgency() {
        let Some(ms) = NonZeroU32::new(5000) else {
            panic!("5000 is non-zero");
        };
        let five_seconds = Osc99Expiry::After(ms);
        for urgency in [
            NotificationUrgency::Low,
            NotificationUrgency::Normal,
            NotificationUrgency::Critical,
        ] {
            assert_eq!(
                spec_for(urgency, five_seconds).lifetime,
                ToastLifetime::Fixed(Duration::from_secs(5)),
                "urgency {urgency:?}"
            );
            assert_eq!(
                spec_for(urgency, Osc99Expiry::Never).lifetime,
                ToastLifetime::UntilDismissed,
                "urgency {urgency:?}"
            );
        }
    }

    #[test]
    fn toast_spec_maps_title_and_body() {
        let with_body = osc99_toast_spec(&Notification99Data {
            title: "T".to_owned(),
            body: Some("B".to_owned()),
            ..data(None)
        });
        assert_eq!(with_body.title, "T");
        assert_eq!(with_body.detail.as_deref(), Some("B"));

        let without_body = osc99_toast_spec(&data(None));
        assert_eq!(without_body.detail, None);

        let empty_body = osc99_toast_spec(&Notification99Data {
            body: Some(String::new()),
            ..data(None)
        });
        assert_eq!(empty_body.detail, None, "an empty body is not a detail");
    }

    // ── registration and updates ────────────────────────────────────────

    #[test]
    fn a_new_toast_instance_shows_a_toast_replacing_nothing() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();

        let (effects, instance) =
            register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);

        let [
            Osc99Effect::ShowToast {
                instance: shown,
                replaces: None,
                spec,
            },
        ] = effects.as_slice()
        else {
            panic!("expected one ShowToast with no replacement, got {effects:?}");
        };
        assert_eq!(*shown, instance);
        assert_eq!(*spec, osc99_toast_spec(&data(Some("n"))));
    }

    #[test]
    fn a_system_only_instance_shows_no_toast() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();

        let (effects, _) = register_with(&mut registry, &a, &data(None), &[Osc99Leg::LinuxSystem]);

        assert!(effects.is_empty());
    }

    #[test]
    fn updating_in_place_replaces_the_toast_and_suppresses_the_old_close() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let closing = reporting_data("n", Osc99CloseReporting::Report);
        let (_, first) = register_with(&mut registry, &a, &closing, &[Osc99Leg::Toast]);

        let (effects, second) = register_with(&mut registry, &a, &closing, &[Osc99Leg::Toast]);

        let [
            Osc99Effect::ShowToast {
                instance,
                replaces: Some(replaced),
                ..
            },
        ] = effects.as_slice()
        else {
            panic!("expected one ShowToast replacing the first, got {effects:?}");
        };
        assert_eq!(*instance, second);
        assert_eq!(*replaced, first);
        assert_ne!(first, second);

        // The replaced toast's `Closed` (if one is still queued) is for a
        // forgotten instance: no close report.
        assert!(registry.on_toast_event(closed(first)).is_empty());
        assert_eq!(alive_ids(&registry, &a), ["n"], "the update is live");
        assert_eq!(registry.instances().len(), 1);
    }

    #[test]
    fn updating_to_a_routing_without_a_toast_closes_the_old_toast() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let closing = reporting_data("n", Osc99CloseReporting::Report);
        let (_, first) = register_with(&mut registry, &a, &closing, &[Osc99Leg::Toast]);

        let (effects, _) = register_with(&mut registry, &a, &closing, &[Osc99Leg::LinuxSystem]);

        let [Osc99Effect::CloseToast(old)] = effects.as_slice() else {
            panic!("expected one CloseToast, got {effects:?}");
        };
        assert_eq!(*old, first);
        assert!(
            registry.on_toast_event(closed(first)).is_empty(),
            "the old close is suppressed"
        );
    }

    #[test]
    fn updating_an_instance_whose_toast_is_gone_shows_a_fresh_toast() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, first) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem],
        );
        assert!(registry.on_toast_event(closed(first)).is_empty());

        let (effects, _) = register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);

        assert!(matches!(
            effects.as_slice(),
            [Osc99Effect::ShowToast { replaces: None, .. }]
        ));
    }

    #[test]
    fn instance_ids_are_fresh_for_every_registration() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, first) = register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);
        let (_, second) = register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);

        assert_ne!(first, second);
    }

    #[test]
    fn legs_are_recorded_without_duplicates() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();

        let _ = registry.register(
            a.origin,
            a.weak(),
            &data(None),
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem, Osc99Leg::Toast],
        );

        let instance = &registry.instances()[0];
        assert_eq!(instance.open_legs, [Osc99Leg::Toast, Osc99Leg::LinuxSystem]);
        assert_eq!(instance.shown_legs, instance.open_legs);

        let _ = registry.register(
            a.origin,
            a.weak(),
            &data(None),
            &[Osc99Leg::Toast, Osc99Leg::UntrackedSystem],
        );
        let instance = &registry.instances()[1];
        assert_eq!(
            instance.shown_legs,
            [Osc99Leg::Toast, Osc99Leg::UntrackedSystem]
        );
        assert_eq!(instance.open_legs, [Osc99Leg::Toast]);
    }

    #[test]
    fn re_registering_the_same_pane_and_id_replaces_it() {
        let (a, b) = two_panes();
        let mut registry = Osc99Registry::default();
        register(&mut registry, &a, Some("n"));
        register(&mut registry, &b, Some("n"));
        let first = registry.instances()[0].instance;

        register(&mut registry, &a, Some("n"));

        assert_eq!(ids_in_registry(&registry, &a), [Some("n".to_owned())]);
        assert_eq!(ids_in_registry(&registry, &b), [Some("n".to_owned())]);
        assert_eq!(registry.instances().len(), 2);
        assert!(
            registry.instances().iter().all(|i| i.instance != first),
            "the replaced instance must be gone"
        );
    }

    // ── activation and close reports ────────────────────────────────────

    #[test]
    fn activation_without_close_reporting_reports_and_forgets_the_instance() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Silent),
            &[Osc99Leg::Toast],
        );

        let reports = replies(registry.on_toast_event(activated(instance)));

        assert_eq!(reports.len(), 1);
        assert!(is_activation(&reports[0], "n"), "got {:?}", reports[0]);
        assert!(registry.instances().is_empty(), "forgotten without c=1");
        assert!(
            registry.on_toast_event(closed(instance)).is_empty(),
            "no close follows an activation without c=1"
        );
    }

    #[test]
    fn activation_with_close_reporting_reports_then_closes_exactly_once() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast],
        );

        let reports = replies(registry.on_toast_event(activated(instance)));
        assert_eq!(reports.len(), 1);
        assert!(is_activation(&reports[0], "n"));
        assert_eq!(registry.instances().len(), 1, "kept until the toast closes");

        let reports = replies(registry.on_toast_event(closed(instance)));
        assert_eq!(reports.len(), 1);
        assert!(is_tracked_close(&reports[0], "n"), "got {:?}", reports[0]);
        assert!(registry.instances().is_empty());

        assert!(registry.on_toast_event(closed(instance)).is_empty());
        assert!(registry.on_toast_event(activated(instance)).is_empty());
    }

    #[test]
    fn activation_without_a_report_request_sends_no_reply() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);

        assert!(registry.on_toast_event(activated(instance)).is_empty());
    }

    #[test]
    fn dismissing_a_toast_with_close_reporting_sends_one_close_report() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast],
        );

        let reports = replies(registry.on_toast_event(closed(instance)));

        assert_eq!(reports.len(), 1);
        assert!(is_tracked_close(&reports[0], "n"));
    }

    #[test]
    fn dismissing_a_toast_without_close_reporting_is_silent() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);

        assert!(registry.on_toast_event(closed(instance)).is_empty());
        assert!(registry.instances().is_empty());
    }

    #[test]
    fn an_event_for_an_unknown_instance_is_ignored() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);
        let unknown = Osc99InstanceId::from_toast_key(ToastKey(instance.to_toast_key().0 + 1000));

        assert!(registry.on_toast_event(closed(unknown)).is_empty());
        assert!(registry.on_toast_event(activated(unknown)).is_empty());
        assert_eq!(registry.instances().len(), 1);
    }

    // ── p=close ─────────────────────────────────────────────────────────

    #[test]
    fn close_with_close_reporting_closes_the_toast_then_reports_once() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast],
        );

        let effects = registry.close(a.origin, "n");

        let [Osc99Effect::CloseToast(closing)] = effects.as_slice() else {
            panic!("expected one CloseToast, got {effects:?}");
        };
        assert_eq!(*closing, instance);
        assert_eq!(
            registry.instances().len(),
            1,
            "waits for the toast's Closed"
        );

        let reports = replies(registry.on_toast_event(closed(instance)));
        assert_eq!(reports.len(), 1);
        assert!(is_tracked_close(&reports[0], "n"));
        assert!(registry.instances().is_empty());
    }

    #[test]
    fn close_without_close_reporting_closes_the_toast_and_stays_silent() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Silent),
            &[Osc99Leg::Toast],
        );

        let effects = registry.close(a.origin, "n");

        assert!(matches!(effects.as_slice(), [Osc99Effect::CloseToast(_)]));
        assert!(registry.on_toast_event(closed(instance)).is_empty());
        assert!(registry.instances().is_empty());
    }

    #[test]
    fn close_from_another_pane_cannot_close_or_forget_an_instance() {
        let (a, b) = two_panes();
        let mut registry = Osc99Registry::default();
        register(&mut registry, &a, Some("shared"));

        let effects = registry.close(b.origin, "shared");

        assert!(effects.is_empty());
        assert_eq!(alive_ids(&registry, &a), ["shared"]);
    }

    #[test]
    fn close_affects_only_the_requesting_panes_instance() {
        let (a, b) = two_panes();
        let mut registry = Osc99Registry::default();
        register(&mut registry, &a, Some("n"));
        register(&mut registry, &b, Some("n"));

        let effects = registry.close(a.origin, "n");

        assert_eq!(effects.len(), 1);
        assert_eq!(alive_ids(&registry, &b), ["n"]);
    }

    #[test]
    fn close_of_an_unknown_id_is_a_no_op() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        register(&mut registry, &a, Some("n"));

        assert!(registry.close(a.origin, "other").is_empty());

        assert_eq!(registry.instances().len(), 1);
    }

    // ── interim system-leg rules ────────────────────────────────────────

    #[test]
    fn an_open_linux_leg_keeps_the_instance_and_the_registry_never_reports_for_it() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem],
        );

        assert!(
            registry.on_toast_event(closed(instance)).is_empty(),
            "no close report while the Linux leg is open"
        );
        assert_eq!(alive_ids(&registry, &a), ["n"], "still live");
        assert_eq!(registry.instances()[0].open_legs, [Osc99Leg::LinuxSystem]);

        // `p=close` has no toast left to close: the Linux leg is dropped and
        // the instance forgotten, silently.
        assert!(registry.close(a.origin, "n").is_empty());
        assert!(registry.instances().is_empty());
    }

    #[test]
    fn close_drops_the_linux_leg_silently_and_never_reports_after_the_toast_closes() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem],
        );

        let effects = registry.close(a.origin, "n");

        assert!(matches!(effects.as_slice(), [Osc99Effect::CloseToast(_)]));
        assert!(
            registry.on_toast_event(closed(instance)).is_empty(),
            "the Linux leg reports for itself, so the registry does not"
        );
        assert!(registry.instances().is_empty());
    }

    #[test]
    fn an_untracked_system_leg_means_no_registry_close_report() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast, Osc99Leg::UntrackedSystem],
        );

        assert!(registry.on_toast_event(closed(instance)).is_empty());
        assert!(registry.instances().is_empty(), "no tracked leg remains");
    }

    // ── capacity and pane lifetime ──────────────────────────────────────

    #[test]
    fn the_cap_forgets_the_oldest_instance() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        for n in 0..=MAX_OSC99_INSTANCES {
            register(&mut registry, &a, Some(&format!("id{n}")));
        }

        assert_eq!(registry.instances().len(), MAX_OSC99_INSTANCES);
        let ids = alive_ids(&registry, &a);
        assert!(!ids.contains(&"id0".to_owned()), "the oldest must be gone");
        assert!(ids.contains(&"id1".to_owned()));
        assert!(ids.contains(&format!("id{MAX_OSC99_INSTANCES}")));
    }

    #[test]
    fn cap_eviction_closes_the_evicted_instances_toast() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, oldest) = register_with(
            &mut registry,
            &a,
            &reporting_data("id0", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast],
        );
        for n in 1..MAX_OSC99_INSTANCES {
            register(&mut registry, &a, Some(&format!("id{n}")));
        }
        assert_eq!(registry.instances().len(), MAX_OSC99_INSTANCES);

        let (effects, _) =
            register_with(&mut registry, &a, &data(Some("last")), &[Osc99Leg::Toast]);

        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Osc99Effect::CloseToast(i) if *i == oldest)),
            "the evicted toast must be closed: {effects:?}"
        );
        assert!(
            registry.on_toast_event(closed(oldest)).is_empty(),
            "the evicted instance reports nothing"
        );
    }

    #[test]
    fn prune_dead_panes_drops_only_instances_of_gone_panes() {
        let (a, b) = two_panes();
        let mut registry = Osc99Registry::default();
        register(&mut registry, &a, Some("a1"));
        register(&mut registry, &b, Some("b1"));
        register(&mut registry, &a, None);

        drop(a.tx);
        registry.prune_dead_panes();

        assert_eq!(registry.instances().len(), 1);
        assert_eq!(registry.instances()[0].origin.pane_id, b.origin.pane_id);
    }

    #[test]
    fn an_id_less_instance_is_never_listed_as_alive() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        register(&mut registry, &a, None);
        register(&mut registry, &a, Some("n"));

        assert_eq!(registry.instances().len(), 2);
        assert_eq!(alive_ids(&registry, &a), ["n"]);
    }

    #[test]
    fn alive_lists_only_the_requesting_panes_ids_sorted() {
        let (a, b) = two_panes();
        let mut registry = Osc99Registry::default();
        for id in ["z", "x"] {
            register(&mut registry, &a, Some(id));
        }
        for id in ["m", "c", "a"] {
            register(&mut registry, &b, Some(id));
        }

        assert_eq!(alive_ids(&registry, &a), ["x", "z"]);
        assert_eq!(alive_ids(&registry, &b), ["a", "c", "m"]);
    }

    #[test]
    fn alive_with_no_instances_replies_with_an_empty_list() {
        let (a, _b) = two_panes();
        let registry = Osc99Registry::default();

        assert_eq!(alive_ids(&registry, &a), Vec::<String>::new());
    }

    #[test]
    fn alive_reply_is_silent_once_the_pane_is_gone() {
        // The effect carries the weak handle; the executor drops the reply
        // when it cannot upgrade, without panicking.
        let (a, _b) = two_panes();
        let weak = a.weak();
        let registry = Osc99Registry::default();
        let effects = registry.alive(a.origin, "req".to_owned(), weak);
        drop(a);
        let mut toasts = ToastStack::default();

        apply_osc99_effects(
            effects,
            &mut Osc99EffectSink {
                toasts: &mut toasts,
            },
        );
    }

    // ── executor ────────────────────────────────────────────────────────

    #[test]
    fn the_executor_shows_replaces_and_closes_keyed_toasts() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let mut toasts = ToastStack::default();

        let (effects, first) =
            register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);
        apply_osc99_effects(
            effects,
            &mut Osc99EffectSink {
                toasts: &mut toasts,
            },
        );
        assert_eq!(toasts.len(), 1);

        let (effects, _) = register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);
        apply_osc99_effects(
            effects,
            &mut Osc99EffectSink {
                toasts: &mut toasts,
            },
        );
        assert_eq!(toasts.len(), 1, "updated in place, not stacked");
        assert!(
            toasts.take_events().is_empty(),
            "no event for the replaced toast"
        );

        let effects = registry.close(a.origin, "n");
        apply_osc99_effects(
            effects,
            &mut Osc99EffectSink {
                toasts: &mut toasts,
            },
        );
        assert_eq!(toasts.len(), 0);
        let events = toasts.take_events();
        assert_eq!(events.len(), 1);
        assert_ne!(
            events[0].key,
            first.to_toast_key(),
            "keyed by the new instance"
        );
        assert_eq!(events[0].kind, ToastEventKind::Closed);
        assert!(
            registry.on_toast_event(events[0]).is_empty(),
            "c=0: no close report"
        );
    }
}
