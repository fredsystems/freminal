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
//! exceed that, the oldest is forgotten silently (no reply), like kitty.

use std::sync::Weak;

use crossbeam_channel::Sender;
use freminal_common::buffer_states::osc99_notification::{
    Osc99ActivationFocus, Osc99ActivationReporting, Osc99CloseReporting,
};
use freminal_common::buffer_states::window_manipulation::Notification99Data;
use freminal_terminal_emulator::io::{GuiReply, InputEvent};

use super::{Osc99Effect, Osc99InstanceId, Osc99Leg, Osc99Origin};
use crate::gui::panes::PaneId;

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
    /// The legs the notification was shown on, without duplicates.
    pub legs: Vec<Osc99Leg>,
}

impl Osc99Instance {
    /// Whether this instance belongs to `pane` and carries exactly `id`.
    fn is_pane_id(&self, pane: PaneId, id: &str) -> bool {
        self.origin.pane_id == pane && self.id.as_deref() == Some(id)
    }

    /// Record in the trace log that this instance left the registry.
    fn log_forgotten(&self, why: &'static str) {
        tracing::trace!(
            instance = self.instance.0,
            id = ?self.id,
            origin = ?self.origin,
            activation_reporting = ?self.activation_reporting,
            activation_focus = ?self.activation_focus,
            close_reporting = ?self.close_reporting,
            legs = ?self.legs,
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
    /// Record a displayed notification and return its fresh instance id.
    ///
    /// Callers register only when at least one leg displayed it; `legs`
    /// lists the legs that did. An instance with no legs is legal but never
    /// produced by the router.
    ///
    /// If `data.id` is set and the same pane already has an instance with
    /// that id, the old instance is forgotten without a reply (in-place
    /// update semantics arrive with the toast leg). If the registry would
    /// then exceed [`MAX_OSC99_INSTANCES`], the oldest instance is forgotten,
    /// again without a reply.
    pub fn register(
        &mut self,
        origin: Osc99Origin,
        reply_tx: Weak<Sender<InputEvent>>,
        data: &Notification99Data,
        legs: &[Osc99Leg],
    ) -> Osc99InstanceId {
        if let Some(id) = data.id.as_deref() {
            self.forget_matching(
                origin.pane_id,
                id,
                "replaced by a notification with the same id",
            );
        }

        let instance = Osc99InstanceId(self.next_instance);
        self.next_instance = self.next_instance.wrapping_add(1);

        let mut deduped: Vec<Osc99Leg> = Vec::with_capacity(legs.len());
        for leg in legs {
            if !deduped.contains(leg) {
                deduped.push(*leg);
            }
        }

        self.instances.push(Osc99Instance {
            instance,
            id: data.id.clone(),
            origin,
            reply_tx,
            activation_reporting: data.activation_reporting,
            activation_focus: data.activation_focus,
            close_reporting: data.close_reporting,
            legs: deduped,
        });

        while self.instances.len() > MAX_OSC99_INSTANCES {
            let oldest = self.instances.remove(0);
            oldest.log_forgotten("registry full; oldest forgotten");
        }

        instance
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
    /// Forgets that pane's instance with that id. An id the pane does not
    /// have is a no-op. Closing the displayed legs and the close report are
    /// not yet produced, so no effects are returned.
    pub fn close(&mut self, origin: Osc99Origin, id: &str) -> Vec<Osc99Effect> {
        self.forget_matching(origin.pane_id, id, "closed by the application");
        Vec::new()
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

    /// Forget `pane`'s instance with `id`, if any.
    fn forget_matching(&mut self, pane: PaneId, id: &str, why: &'static str) {
        self.instances.retain(|instance| {
            let matches = instance.is_pane_id(pane, id);
            if matches {
                instance.log_forgotten(why);
            }
            !matches
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
    use std::sync::Arc;

    use freminal_common::buffer_states::osc_notify_99::{
        NotificationOccasion, NotificationUrgency,
    };
    use freminal_common::buffer_states::osc99_notification::{Osc99Expiry, Osc99Sound};
    use freminal_windowing::WindowId;

    use super::*;
    use crate::gui::panes::PaneIdGenerator;

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

    fn register(registry: &mut Osc99Registry, pane: &TestPane, id: Option<&str>) {
        registry.register(pane.origin, pane.weak(), &data(id), &[Osc99Leg::Toast]);
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
            other @ Osc99Effect::Reply { .. } => panic!("expected an alive reply, got {other:?}"),
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
    fn close_from_another_pane_cannot_forget_an_instance() {
        let (a, b) = two_panes();
        let mut registry = Osc99Registry::default();
        register(&mut registry, &a, Some("shared"));

        let effects = registry.close(b.origin, "shared");

        assert!(effects.is_empty());
        assert_eq!(alive_ids(&registry, &a), ["shared"]);
    }

    #[test]
    fn close_forgets_the_requesting_panes_instance() {
        let (a, b) = two_panes();
        let mut registry = Osc99Registry::default();
        register(&mut registry, &a, Some("n"));
        register(&mut registry, &b, Some("n"));

        let effects = registry.close(a.origin, "n");

        assert!(effects.is_empty());
        assert_eq!(alive_ids(&registry, &a), Vec::<String>::new());
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

    #[test]
    fn instance_ids_are_fresh_for_every_registration() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let first = registry.register(a.origin, a.weak(), &data(Some("n")), &[Osc99Leg::Toast]);
        let second = registry.register(a.origin, a.weak(), &data(Some("n")), &[Osc99Leg::Toast]);

        assert_ne!(first, second);
    }

    #[test]
    fn legs_are_recorded_without_duplicates() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();

        registry.register(
            a.origin,
            a.weak(),
            &data(None),
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem, Osc99Leg::Toast],
        );

        assert_eq!(
            registry.instances()[0].legs,
            [Osc99Leg::Toast, Osc99Leg::LinuxSystem]
        );
    }

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

        super::super::apply_osc99_effects(effects);
    }
}
