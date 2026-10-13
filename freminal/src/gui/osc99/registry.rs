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
//! The toast leg and the Linux/BSD system leg are both registry-managed: the
//! registry emits their show effects and consumes their activation and close
//! events ([`Osc99Registry::on_toast_event`], [`Osc99Registry::on_system_event`]),
//! so both legs share one activation path and one close report.
//!
//! The untracked system leg (macOS and Windows) is shown and forgotten: it is
//! never an open leg, so it never holds an instance alive. Displaying it emits
//! [`Osc99Effect::ShowUntrackedSystem`] and, when `c=1` was requested, the
//! `untracked` close report straight away (as kitty does on macOS). An
//! instance that was ever shown on it never gets a later, tracked close
//! report, whatever its other legs do.
//!
//! Closing is requested per leg. `p=close`, expiry and capacity eviction emit
//! a close effect for every open leg ([`Osc99Effect::CloseToast`],
//! [`Osc99Effect::CloseLinuxSystem`]), and the instance finishes when the last
//! of those legs reports `Closed`. An update that moves an instance off a leg
//! closes the old notification's leg, and an activation on one leg closes the
//! others. The registry remembers which legs it has already asked to close, so
//! no leg is ever asked twice. Events that later arrive for a forgotten
//! instance are ignored.
//!
//! An instance with no open leg is never kept: a display on the untracked leg
//! alone is shown, answered and forgotten at once.
//!
//! # Expiry
//!
//! A positive `w=` ([`Osc99Expiry::After`]) gives the instance a deadline,
//! measured from the moment it was registered (so an update, which registers
//! a new instance, restarts it). [`Osc99Registry::tick`] closes every
//! instance past its deadline through the same path as `p=close`, whatever
//! the toast leg's own lifetime is, and [`Osc99Registry::next_deadline`]
//! tells the GUI when to wake for the nearest one. Time is injected, so the
//! registry never reads a clock. The deadline also applies to legs left open
//! after an activation (the activated leg closes itself, so only a leg that
//! did not is closed by expiry); a leg already asked to close is not asked
//! again.
//!
//! # Pane lifetime
//!
//! An instance remembers the pane's `Weak` reply handle. Once the pane is
//! gone the handle no longer upgrades and [`Osc99Registry::prune_dead_panes`]
//! drops the instance and returns close effects for its open legs, so nothing
//! outlives its pane. No replies are produced.
//!
//! # Capacity
//!
//! A pane keeps at most [`MAX_OSC99_INSTANCES_PER_PANE`] instances: when a new
//! one would exceed that, *that pane's* oldest is forgotten without a reply,
//! so a noisy pane cannot evict another pane's notifications. As a backstop,
//! at most [`MAX_OSC99_INSTANCES`] instances are kept overall, and the oldest
//! is forgotten the same way. A forgotten instance's open legs are closed
//! (kitty also forgets the oldest).

use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use freminal_common::buffer_states::osc99_notification::{
    Notification99Data, Osc99ActivationFocus, Osc99ActivationReporting, Osc99CloseReporting,
    Osc99Expiry,
};
use freminal_terminal_emulator::io::{GuiReply, InputEvent, Osc99CloseTracking};

#[cfg(all(unix, not(target_os = "macos")))]
use super::Osc99SystemLegEvent;
use super::{
    Osc99Effect, Osc99Filter, Osc99FilterVerdict, Osc99InstanceId, Osc99Leg, Osc99LegActivation,
    Osc99Origin, osc99_toast_spec,
};
use crate::gui::panes::PaneId;
use crate::gui::toast::ToastEvent;

/// The most instances the registry keeps overall, as a backstop behind
/// [`MAX_OSC99_INSTANCES_PER_PANE`]. The oldest is forgotten, with no reply,
/// when a new registration would exceed this.
pub const MAX_OSC99_INSTANCES: usize = 128;

/// The most instances one pane keeps. That pane's oldest is forgotten, with no
/// reply, when a new registration would exceed this.
pub const MAX_OSC99_INSTANCES_PER_PANE: usize = 32;

/// Whether the registry has already asked for an instance to be closed.
///
/// An instance whose legs are closing stays registered until every open leg's
/// `Closed` event arrives. Without this state a second `p=close` or activation
/// would repeat work already done. Which legs were actually asked to close is
/// tracked per leg (`Osc99Instance::close_requested`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99Closing {
    /// No close has been requested and the instance has not been activated.
    Open,
    /// The instance was closed (`p=close`, expiry) or activated; it waits for
    /// its open legs' `Closed` events. A further activation or `p=close` is
    /// ignored. `p=alive` lists an instance while it has an open leg that has
    /// not been asked to close, which is independent of this state: an
    /// activated `c=1` instance whose desktop notification is still displayed
    /// stays listed, while a closed or expired one, whose legs were all asked
    /// to close, is not.
    Requested,
}

/// One displayed OSC 99 notification.
///
/// The fields are private: only the registry mutates an instance, so its
/// invariants (such as `open_legs` never being empty) hold by construction.
#[derive(Debug, Clone)]
pub struct Osc99Instance {
    /// The registry-assigned identity, unique for the process lifetime.
    instance: Osc99InstanceId,
    /// The application's `i=` id, if the notification carried one.
    id: Option<String>,
    /// The pane the notification came from.
    origin: Osc99Origin,
    /// A `Weak` handle to the pane's reply sender; also the pane's liveness.
    reply_tx: Weak<Sender<InputEvent>>,
    /// Whether `a=report` was requested.
    activation_reporting: Osc99ActivationReporting,
    /// Whether activation should focus the pane.
    activation_focus: Osc99ActivationFocus,
    /// Whether `c=1` was requested.
    close_reporting: Osc99CloseReporting,
    /// Every leg the notification was shown on, without duplicates. Never
    /// shrinks.
    shown_legs: Vec<Osc99Leg>,
    /// The legs still displayed and observable, without duplicates. Never
    /// holds [`Osc99Leg::UntrackedSystem`], and never empty while the instance
    /// is registered.
    open_legs: Vec<Osc99Leg>,
    /// The legs the registry has already emitted a close effect for. A leg is
    /// asked to close at most once, whichever of `p=close`, expiry or an
    /// activation on another leg asks first.
    close_requested: Vec<Osc99Leg>,
    /// When the notification expires (`w=` > 0), measured from its display.
    /// `None` when it never expires on its own, or once expiry has acted.
    deadline: Option<Instant>,
    /// Whether the registry has already asked the instance to close.
    closing: Osc99Closing,
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
    /// `button` is the 1-based button that was pressed, or `None` for an
    /// activation of the notification body.
    fn activation_reply(&self, button: Option<u8>) -> Option<Osc99Effect> {
        (self.activation_reporting == Osc99ActivationReporting::Report).then(|| {
            Osc99Effect::Reply {
                reply_tx: Weak::clone(&self.reply_tx),
                reply: GuiReply::Osc99Activation {
                    id: self.id.clone(),
                    button: button.map(|n| n.to_string()),
                },
            }
        })
    }

    /// The effect that focuses this instance's pane, if activation should
    /// focus it (`a=focus`).
    fn focus_effect(&self) -> Option<Osc99Effect> {
        (self.activation_focus == Osc99ActivationFocus::Focus)
            .then_some(Osc99Effect::FocusPane(self.origin))
    }

    /// The `p=close` report for this instance, if the registry owes one:
    /// `c=1` was requested and the `untracked` form was not already sent at
    /// display (see the module docs). An instance ever shown on the untracked
    /// leg has had it, so it is owed nothing here.
    fn close_reply(&self) -> Option<Osc99Effect> {
        let reported_untracked = self.shown_legs.contains(&Osc99Leg::UntrackedSystem);
        (self.close_reporting == Osc99CloseReporting::Report && !reported_untracked).then(|| {
            Osc99Effect::Reply {
                reply_tx: Weak::clone(&self.reply_tx),
                reply: GuiReply::Osc99Closed {
                    id: self.id.clone(),
                    tracking: Osc99CloseTracking::Tracked,
                },
            }
        })
    }

    /// The effect that closes `leg`, if it is open, closable and not already
    /// asked to close.
    fn close_leg_effect(&self, leg: Osc99Leg) -> Option<Osc99Effect> {
        if !self.is_open(leg) || self.close_requested.contains(&leg) {
            return None;
        }
        match leg {
            Osc99Leg::Toast => Some(Osc99Effect::CloseToast(self.instance)),
            Osc99Leg::LinuxSystem => Some(Osc99Effect::CloseLinuxSystem(self.instance)),
            // Never open: it cannot be observed, so it cannot be closed.
            Osc99Leg::UntrackedSystem => None,
        }
    }

    /// Whether some open leg has not been asked to close yet, so a close (or
    /// expiry) would still do something.
    fn has_unrequested_leg(&self) -> bool {
        self.open_legs
            .iter()
            .any(|leg| !self.close_requested.contains(leg))
    }

    /// Ask every open leg that has not been asked yet to close, except `kept`
    /// (the leg that closes itself: the one a user just activated), and return
    /// the effects. The legs asked are remembered, so none is asked twice.
    fn request_close_except(&mut self, kept: Option<Osc99Leg>) -> Vec<Osc99Effect> {
        let asked: Vec<Osc99Leg> = self
            .open_legs
            .iter()
            .copied()
            .filter(|leg| Some(*leg) != kept)
            .collect();
        let effects = asked
            .iter()
            .filter_map(|leg| self.close_leg_effect(*leg))
            .collect();
        let newly_asked: Vec<Osc99Leg> = asked
            .into_iter()
            .filter(|leg| !self.close_requested.contains(leg))
            .collect();
        self.close_requested.extend(newly_asked);
        effects
    }

    /// Ask every open leg that has not been asked yet to close (see
    /// [`Self::request_close_except`]) and mark the instance closing.
    fn request_close(&mut self) -> Vec<Osc99Effect> {
        self.closing = Osc99Closing::Requested;
        self.request_close_except(None)
    }

    /// The effects that close every open leg, for an instance that is being
    /// forgotten (replaced or evicted) rather than closed.
    fn forget_effects(&self) -> Vec<Osc99Effect> {
        self.open_legs
            .iter()
            .filter_map(|leg| self.close_leg_effect(*leg))
            .collect()
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
    /// The compiled `osc99_filters`. The router consults it through
    /// [`Self::filter_verdict`] before it displays or registers anything.
    filter: Osc99Filter,
}

impl Osc99Registry {
    /// Replace the compiled user filter (`[notifications] osc99_filters`).
    ///
    /// Already-registered instances are unaffected: the filter only decides
    /// whether a *new* notification is displayed.
    pub fn set_filter(&mut self, filter: Osc99Filter) {
        self.filter = filter;
    }

    /// Whether the user filter drops `data`. A dropped notification is
    /// neither displayed, registered, nor answered.
    #[must_use]
    pub fn filter_verdict(&self, data: &Notification99Data) -> Osc99FilterVerdict {
        self.filter.matches(data)
    }

    /// Record a displayed notification and return the effects that show it.
    ///
    /// Callers register only when at least one leg displayed it; `legs`
    /// lists the legs that did. No leg is shown by the caller: a `Toast` in
    /// `legs` yields a [`Osc99Effect::ShowToast`] here, a `LinuxSystem` a
    /// [`Osc99Effect::ShowLinuxSystem`], and an `UntrackedSystem` a
    /// [`Osc99Effect::ShowUntrackedSystem`] plus, when `c=1` was requested, the
    /// `untracked` close report. The two system effects share one copy of
    /// `data`, which is taken by value so that allocation is built without
    /// copying it. An update of an instance whose new legs include the untracked
    /// one shows a new notification (nothing can be replaced in place there)
    /// and, with `c=1`, reports again.
    ///
    /// An instance left with no open leg (a display on the untracked leg
    /// alone) is not kept: it can never be closed or activated through the
    /// registry, so it would only occupy a slot.
    ///
    /// If `data.id` is set and the same pane already has an instance with
    /// that id, the notification is an update: the old instance is forgotten
    /// without a close report, and each of its open legs is replaced in place
    /// when `legs` still has it (a leg `legs` no longer has is closed), whether
    /// or not the new instance is kept. If the pane would then exceed
    /// [`MAX_OSC99_INSTANCES_PER_PANE`], that pane's oldest instance is
    /// forgotten, again without a reply, and its open legs are closed; the
    /// same applies to the oldest overall instance beyond
    /// [`MAX_OSC99_INSTANCES`].
    ///
    /// `now` is the display time: an `w=` expiry is measured from it, so an
    /// update restarts the deadline.
    pub fn register(
        &mut self,
        origin: Osc99Origin,
        reply_tx: Weak<Sender<InputEvent>>,
        data: Notification99Data,
        legs: &[Osc99Leg],
        now: Instant,
    ) -> Vec<Osc99Effect> {
        let mut effects = Vec::new();
        let wants_toast = legs.contains(&Osc99Leg::Toast);
        let wants_linux = legs.contains(&Osc99Leg::LinuxSystem);
        let wants_untracked = legs.contains(&Osc99Leg::UntrackedSystem);

        let (replaces_toast, replaces_linux) =
            self.forget_replaced(origin, data.id.as_deref(), legs, &mut effects);

        let instance = Osc99InstanceId(self.next_instance);
        self.next_instance = self.next_instance.wrapping_add(1);

        let mut deduped: Vec<Osc99Leg> = Vec::with_capacity(legs.len());
        for leg in legs {
            if !deduped.contains(leg) {
                deduped.push(*leg);
            }
        }

        // What the instance keeps of `data`, taken before `data` may move into
        // the shared system notification below.
        let Notification99Data {
            id,
            activation_reporting,
            activation_focus,
            close_reporting,
            expiry,
            ..
        } = &data;
        let (id, activation_reporting, activation_focus, close_reporting, expiry) = (
            id.clone(),
            *activation_reporting,
            *activation_focus,
            *close_reporting,
            *expiry,
        );

        if wants_toast {
            effects.push(Osc99Effect::ShowToast {
                instance,
                replaces: replaces_toast,
                spec: osc99_toast_spec(&data),
            });
        }
        if wants_linux || wants_untracked {
            let shared = Arc::new(data);
            if wants_linux {
                effects.push(Osc99Effect::ShowLinuxSystem {
                    instance,
                    replaces: replaces_linux,
                    notification: Arc::clone(&shared),
                });
            }
            if wants_untracked {
                effects.push(Osc99Effect::ShowUntrackedSystem(shared));
                // Nothing can observe this notification closing, so a
                // requested close report is answered now, in the `untracked`
                // form.
                if close_reporting == Osc99CloseReporting::Report {
                    effects.push(Osc99Effect::Reply {
                        reply_tx: Weak::clone(&reply_tx),
                        reply: GuiReply::Osc99Closed {
                            id: id.clone(),
                            tracking: Osc99CloseTracking::Untracked,
                        },
                    });
                }
            }
        }

        // An untracked system notification can never be observed closing, so
        // it is never an open leg the instance could wait on.
        let open_legs: Vec<Osc99Leg> = deduped
            .iter()
            .copied()
            .filter(|leg| *leg != Osc99Leg::UntrackedSystem)
            .collect();
        if open_legs.is_empty() {
            return effects;
        }

        self.instances.push(Osc99Instance {
            instance,
            id,
            origin,
            reply_tx,
            activation_reporting,
            activation_focus,
            close_reporting,
            open_legs,
            shown_legs: deduped,
            close_requested: Vec::new(),
            deadline: match expiry {
                Osc99Expiry::After(ms) => {
                    now.checked_add(Duration::from_millis(u64::from(ms.get())))
                }
                Osc99Expiry::Never | Osc99Expiry::OsDefault => None,
            },
            closing: Osc99Closing::Open,
        });

        effects.extend(self.evict_over_capacity(origin));

        effects
    }

    /// Forget the pane's existing instance with `id`, if there is one, because
    /// a new notification with the same id replaces it.
    ///
    /// Returns the instances whose toast and Linux notification the new one
    /// (shown on `legs`) takes over in place: the old instance has that leg
    /// open and `legs` still has it. A leg the new notification no longer uses
    /// gets its close effect appended to `effects`, unless it was already asked
    /// to close.
    fn forget_replaced(
        &mut self,
        origin: Osc99Origin,
        id: Option<&str>,
        legs: &[Osc99Leg],
        effects: &mut Vec<Osc99Effect>,
    ) -> (Option<Osc99InstanceId>, Option<Osc99InstanceId>) {
        let wants_toast = legs.contains(&Osc99Leg::Toast);
        let wants_linux = legs.contains(&Osc99Leg::LinuxSystem);
        let mut replaces_toast = None;
        let mut replaces_linux = None;
        if let Some(id) = id
            && let Some(pos) = self
                .instances
                .iter()
                .position(|instance| instance.is_pane_id(origin.pane_id, id))
        {
            let old = self.instances.remove(pos);
            old.log_forgotten("replaced by a notification with the same id");
            if old.is_open(Osc99Leg::Toast) {
                if wants_toast {
                    replaces_toast = Some(old.instance);
                } else {
                    // The old toast's `Closed` event arrives for a forgotten
                    // instance and is ignored, so no close is reported.
                    effects.extend(old.close_leg_effect(Osc99Leg::Toast));
                }
            }
            if old.is_open(Osc99Leg::LinuxSystem) {
                if wants_linux {
                    replaces_linux = Some(old.instance);
                } else {
                    // As for the toast: its `Closed` is for a forgotten
                    // instance, so no close is reported.
                    effects.extend(old.close_leg_effect(Osc99Leg::LinuxSystem));
                }
            }
        }
        (replaces_toast, replaces_linux)
    }

    /// Forget the oldest instances beyond the pane quota of `origin`'s pane
    /// and then beyond the global cap, returning the effects that close their
    /// open legs.
    fn evict_over_capacity(&mut self, origin: Osc99Origin) -> Vec<Osc99Effect> {
        let mut effects = Vec::new();
        // The pane's own quota first, so a noisy pane evicts itself and never
        // another pane's notifications; the global cap is a backstop.
        let mut to_evict = self
            .instances
            .iter()
            .filter(|kept| kept.origin.pane_id == origin.pane_id)
            .count()
            .saturating_sub(MAX_OSC99_INSTANCES_PER_PANE);
        if to_evict > 0 {
            // Registration order is oldest first, so the first of the pane's
            // instances are the oldest.
            let (evicted, kept): (Vec<Osc99Instance>, Vec<Osc99Instance>) =
                std::mem::take(&mut self.instances)
                    .into_iter()
                    .partition(|candidate| {
                        let evict = to_evict > 0 && candidate.origin.pane_id == origin.pane_id;
                        if evict {
                            to_evict -= 1;
                        }
                        evict
                    });
            self.instances = kept;
            for oldest in evicted {
                oldest.log_forgotten("pane quota full; pane's oldest forgotten");
                effects.extend(oldest.forget_effects());
            }
        }
        while self.instances.len() > MAX_OSC99_INSTANCES {
            let oldest = self.instances.remove(0);
            oldest.log_forgotten("registry full; oldest forgotten");
            effects.extend(oldest.forget_effects());
        }
        effects
    }

    /// Answer a `p=alive` poll from `origin`'s pane.
    ///
    /// The reply lists the ids of that pane's instances that carry an id,
    /// sorted and without duplicates. Other panes' instances are never
    /// listed. An instance is listed while one of its legs is still open and
    /// has not been asked to close: a `p=close` or an expiry removes it at
    /// once, while an activated `c=1` instance whose desktop notification is
    /// still displayed stays listed until the daemon closes it (as kitty
    /// does).
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
            .filter(|instance| {
                instance.origin.pane_id == origin.pane_id && instance.has_unrequested_leg()
            })
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
    /// An id the pane does not have is a no-op. Every open leg (toast and
    /// Linux system) is asked to close; the instance stays until the last
    /// leg's `Closed` event arrives (see [`Self::on_toast_event`] and
    /// [`Self::on_system_event`]), which is when a `c=1` report is sent.
    /// Closing an instance that is already closing, or that was activated,
    /// does nothing.
    pub fn close(&mut self, origin: Osc99Origin, id: &str) -> Vec<Osc99Effect> {
        let Some(instance) = self
            .instances
            .iter_mut()
            .find(|instance| instance.is_pane_id(origin.pane_id, id))
        else {
            return Vec::new();
        };
        if instance.closing == Osc99Closing::Requested {
            return Vec::new();
        }
        instance.request_close()
    }

    /// Close the legs of every instance whose deadline is at or before `now`
    /// and return the effects.
    ///
    /// An open instance closes through the same path as `p=close`
    /// ([`Self::close`]). An instance that was activated, or is otherwise
    /// already closing, still has its deadline: the legs that were never asked
    /// to close (an activated leg that did not close itself) are closed now,
    /// and a leg already asked is not asked again. The deadline is spent once
    /// acted on.
    pub fn tick(&mut self, now: Instant) -> Vec<Osc99Effect> {
        let mut effects = Vec::new();
        for instance in &mut self.instances {
            if instance.has_unrequested_leg()
                && instance.deadline.is_some_and(|deadline| deadline <= now)
            {
                instance.deadline = None;
                effects.extend(instance.request_close());
            }
        }
        effects
    }

    /// The earliest deadline among instances that still have a leg to close,
    /// or `None` when nothing will expire. The GUI schedules a repaint for it.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.instances
            .iter()
            .filter(|instance| instance.has_unrequested_leg())
            .filter_map(|instance| instance.deadline)
            .min()
    }

    /// Apply an event from the toast stack.
    ///
    /// An event for an unknown instance (replaced, evicted, forgotten, or
    /// its pane pruned) is ignored.
    ///
    /// - `Activated` and `Button(n)` are an activation (see
    ///   [`Self::activate`]).
    /// - `Closed` closes the toast leg; once no leg is open the instance is
    ///   forgotten and a close report is sent when owed.
    pub fn on_toast_event(&mut self, event: ToastEvent) -> Vec<Osc99Effect> {
        let instance = Osc99InstanceId::from_toast_key(event.key);
        match Osc99LegActivation::from_toast_event(event.kind) {
            Some(activation) => self.activate(instance, Osc99Leg::Toast, activation),
            None => self.close_leg(instance, Osc99Leg::Toast),
        }
    }

    /// Apply an event from the Linux/BSD system leg (which exists only on
    /// Linux/BSD).
    ///
    /// An event for an unknown instance (replaced, forgotten, or its pane
    /// pruned) is ignored. `Activated` is an activation exactly like a
    /// toast's; `Closed` and `ShowFailed` close the Linux leg (a failed show
    /// leaves nothing displayed, so it ends the leg the same way).
    #[cfg(all(unix, not(target_os = "macos")))]
    pub fn on_system_event(
        &mut self,
        instance: Osc99InstanceId,
        event: Osc99SystemLegEvent,
    ) -> Vec<Osc99Effect> {
        match event {
            Osc99SystemLegEvent::Activated(activation) => {
                self.activate(instance, Osc99Leg::LinuxSystem, activation)
            }
            Osc99SystemLegEvent::Closed | Osc99SystemLegEvent::ShowFailed => {
                self.close_leg(instance, Osc99Leg::LinuxSystem)
            }
        }
    }

    /// Activate `instance` through `activation` on `leg`, from any leg:
    /// returns the activation report when one was requested, then the focus
    /// request when `a=focus` is in effect, then a close for every *other*
    /// open leg (the user has acted on the notification; `leg` closes itself).
    ///
    /// The instance is forgotten unless `c=1` was requested, so the other
    /// legs' `Closed` events are then ignored. With `c=1` it stays, marked
    /// closing, until every open leg has closed, and then reports once. An
    /// activation of an instance that is already closing (activated before, or
    /// closed by `p=close` or expiry) is ignored: no second report, no second
    /// focus request. An unknown instance yields nothing.
    ///
    /// The activated leg is not marked as asked to close: the user's action
    /// normally closes it, but if it stays open, expiry still closes it.
    fn activate(
        &mut self,
        instance: Osc99InstanceId,
        leg: Osc99Leg,
        activation: Osc99LegActivation,
    ) -> Vec<Osc99Effect> {
        let Some(pos) = self.position_of(instance) else {
            return Vec::new();
        };
        if self.instances[pos].closing == Osc99Closing::Requested {
            return Vec::new();
        }
        let button = match activation {
            Osc99LegActivation::Body => None,
            Osc99LegActivation::Button(n) => Some(n),
        };
        let other_legs = self.instances[pos].request_close_except(Some(leg));
        let effects: Vec<Osc99Effect> = self.instances[pos]
            .activation_reply(button)
            .into_iter()
            .chain(self.instances[pos].focus_effect())
            .chain(other_legs)
            .collect();
        if self.instances[pos].close_reporting == Osc99CloseReporting::Silent {
            self.instances
                .remove(pos)
                .log_forgotten("activated without close reporting");
        } else {
            self.instances[pos].closing = Osc99Closing::Requested;
        }
        effects
    }

    /// Close `leg` of `instance`; once no leg is open the instance is
    /// forgotten and its close report is returned when owed. An unknown
    /// instance yields nothing.
    fn close_leg(&mut self, instance: Osc99InstanceId, leg: Osc99Leg) -> Vec<Osc99Effect> {
        let Some(pos) = self.position_of(instance) else {
            return Vec::new();
        };
        self.instances[pos].open_legs.retain(|open| *open != leg);
        self.finish_if_closed(pos)
    }

    /// The index of `instance`, if it is registered.
    fn position_of(&self, instance: Osc99InstanceId) -> Option<usize> {
        self.instances
            .iter()
            .position(|candidate| candidate.instance == instance)
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
    /// longer upgrades), and return the effects that close the open legs of
    /// the dropped instances, so a `w=` toast or a desktop notification does
    /// not outlive its pane.
    ///
    /// A leg already asked to close is not asked again. No reply is produced:
    /// the pane is gone. The legs' later `Closed` events arrive for a
    /// forgotten instance and are ignored.
    #[must_use]
    pub fn prune_dead_panes(&mut self) -> Vec<Osc99Effect> {
        let (alive, gone): (Vec<Osc99Instance>, Vec<Osc99Instance>) =
            std::mem::take(&mut self.instances)
                .into_iter()
                .partition(|instance| instance.reply_tx.strong_count() > 0);
        self.instances = alive;
        gone.iter()
            .flat_map(|instance| {
                instance.log_forgotten("originating pane is gone");
                instance.forget_effects()
            })
            .collect()
    }

    /// The instances, oldest first.
    #[cfg(test)]
    pub fn instances(&self) -> &[Osc99Instance] {
        &self.instances
    }
}

/// Read access for tests outside this module (the fields are private).
#[cfg(test)]
impl Osc99Instance {
    /// The application's `i=` id.
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// The registry-assigned identity. Only the Linux/BSD tests read it.
    #[cfg(all(unix, not(target_os = "macos")))]
    pub const fn instance(&self) -> Osc99InstanceId {
        self.instance
    }

    /// Whether `a=report` was requested.
    pub const fn activation_reporting(&self) -> Osc99ActivationReporting {
        self.activation_reporting
    }

    /// Whether `c=1` was requested.
    pub const fn close_reporting(&self) -> Osc99CloseReporting {
        self.close_reporting
    }

    /// The legs the notification was shown on.
    pub fn shown_legs(&self) -> &[Osc99Leg] {
        &self.shown_legs
    }

    /// The legs still displayed.
    pub fn open_legs(&self) -> &[Osc99Leg] {
        &self.open_legs
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::time::Duration;

    use freminal_common::buffer_states::osc99_notification::{
        NotificationOccasion, NotificationUrgency, Osc99Expiry, Osc99Sound,
    };
    use freminal_windowing::WindowId;

    use super::*;
    use crate::gui::icons::ChromeIcon;
    use crate::gui::osc99::{
        NoSystemBackend, NoUntrackedBackend, Osc99EffectSink, Osc99SystemBackend, Osc99ToastSpec,
        Osc99UntrackedBackend, RecordedShow, RecordingSystemBackend, RecordingUntrackedBackend,
        apply_osc99_effects,
    };
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

    /// Data with `a=report` and the given `c=` setting.
    fn reporting_data(id: &str, close: Osc99CloseReporting) -> Notification99Data {
        Notification99Data {
            activation_reporting: Osc99ActivationReporting::Report,
            close_reporting: close,
            ..data(Some(id))
        }
    }

    fn register(registry: &mut Osc99Registry, pane: &TestPane, id: Option<&str>) {
        let _ = registry.register(
            pane.origin,
            pane.weak(),
            data(id),
            &[Osc99Leg::Toast],
            Instant::now(),
        );
    }

    /// Register `data` on `legs`, returning the effects and the new
    /// instance's id (the registry's newest instance).
    fn register_with(
        registry: &mut Osc99Registry,
        pane: &TestPane,
        data: &Notification99Data,
        legs: &[Osc99Leg],
    ) -> (Vec<Osc99Effect>, Osc99InstanceId) {
        let effects =
            registry.register(pane.origin, pane.weak(), data.clone(), legs, Instant::now());
        // The registry mints ids in order, whether or not it keeps the
        // instance (a display on the untracked leg alone is not kept).
        let instance = Osc99InstanceId(registry.next_instance.wrapping_sub(1));
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

    /// The focus requests and failed system requests one executor run left in
    /// its sink: `(focus_requests, failed_system_shows, failed_system_closes)`.
    type Executed = (Vec<Osc99Origin>, Vec<Osc99InstanceId>, Vec<Osc99InstanceId>);

    /// Execute `effects` against `toasts` and `system`, returning what the
    /// sink collected.
    fn execute_all(
        effects: Vec<Osc99Effect>,
        toasts: &mut ToastStack,
        system: &mut dyn Osc99SystemBackend,
    ) -> Executed {
        execute_all_with_untracked(effects, toasts, system, &mut NoUntrackedBackend)
    }

    /// [`execute_all`] with a caller-supplied untracked-leg backend.
    fn execute_all_with_untracked(
        effects: Vec<Osc99Effect>,
        toasts: &mut ToastStack,
        system: &mut dyn Osc99SystemBackend,
        untracked: &mut dyn Osc99UntrackedBackend,
    ) -> Executed {
        let mut sink = Osc99EffectSink::new(toasts, system, untracked);
        apply_osc99_effects(effects, &mut sink);
        (
            sink.focus_requests,
            sink.failed_system_shows,
            sink.failed_system_closes,
        )
    }

    /// Execute `effects` against `toasts` and `system`, returning the focus
    /// requests and the failed system shows the executor collected.
    fn execute(
        effects: Vec<Osc99Effect>,
        toasts: &mut ToastStack,
        system: &mut dyn Osc99SystemBackend,
    ) -> (Vec<Osc99Origin>, Vec<Osc99InstanceId>) {
        let (focus_requests, failed_shows, _) = execute_all(effects, toasts, system);
        (focus_requests, failed_shows)
    }

    /// The close effect of every kind, as `(leg, instance)` pairs, panicking
    /// on any other effect.
    #[cfg(all(unix, not(target_os = "macos")))]
    fn close_targets(effects: &[Osc99Effect]) -> Vec<(Osc99Leg, Osc99InstanceId)> {
        effects
            .iter()
            .map(|effect| match effect {
                Osc99Effect::CloseToast(instance) => (Osc99Leg::Toast, *instance),
                Osc99Effect::CloseLinuxSystem(instance) => (Osc99Leg::LinuxSystem, *instance),
                other => panic!("expected only close effects, got {other:?}"),
            })
            .collect()
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
    fn toast_spec_explicit_expiry_leaves_the_deadline_to_the_registry() {
        // A `w=` toast stays until dismissed: the exact deadline is the
        // registry's (see `the_deadline_closes_a_toast_whose_own_lifetime_is_longer`),
        // so the toast must not carry a second clock that hovering could
        // extend past it. This holds whatever the urgency.
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
                ToastLifetime::UntilDismissed,
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

    #[test]
    fn toast_spec_carries_the_buttons_in_order() {
        let with_buttons = osc99_toast_spec(&Notification99Data {
            buttons: vec!["Yes".to_owned(), "No".to_owned()],
            ..data(None)
        });
        assert_eq!(with_buttons.buttons, ["Yes", "No"]);

        assert_eq!(osc99_toast_spec(&data(None)).buttons.len(), 0);
    }

    #[test]
    fn toast_spec_icon_is_the_first_standard_name_in_order() {
        let names = |names: &[&str]| Notification99Data {
            icon_names: names.iter().map(|n| (*n).to_owned()).collect(),
            ..data(None)
        };
        assert_eq!(osc99_toast_spec(&data(None)).icon, None);
        assert_eq!(
            osc99_toast_spec(&names(&["custom-app-icon", "info", "error"])).icon,
            Some(ChromeIcon::Info),
            "a non-standard name before a standard one is skipped"
        );
        assert_eq!(
            osc99_toast_spec(&names(&["error", "info"])).icon,
            Some(ChromeIcon::Error)
        );
        assert_eq!(
            osc99_toast_spec(&names(&["warn"])).icon,
            Some(ChromeIcon::Warning)
        );
        assert_eq!(osc99_toast_spec(&names(&["Error", "folder"])).icon, None);
    }

    // ── registration and updates ────────────────────────────────────────

    fn filter_of(rules: &[&str]) -> Osc99Filter {
        let owned: Vec<String> = rules.iter().map(|r| (*r).to_owned()).collect();
        let (filter, errors) = Osc99Filter::compile(&owned);
        assert_eq!(errors, Vec::new(), "unexpected filter errors");
        filter
    }

    #[test]
    fn a_fresh_registry_keeps_everything() {
        let registry = Osc99Registry::default();
        assert_eq!(
            registry.filter_verdict(&data(Some("a"))),
            Osc99FilterVerdict::Keep
        );
    }

    #[test]
    fn the_filter_verdict_follows_the_installed_filter() {
        let mut registry = Osc99Registry::default();
        registry.set_filter(filter_of(&["title:^Title$"]));
        assert_eq!(
            registry.filter_verdict(&data(Some("a"))),
            Osc99FilterVerdict::Drop
        );

        let other = Notification99Data {
            title: "Other".to_owned(),
            ..data(Some("b"))
        };
        assert_eq!(registry.filter_verdict(&other), Osc99FilterVerdict::Keep);

        registry.set_filter(Osc99Filter::default());
        assert_eq!(
            registry.filter_verdict(&data(Some("a"))),
            Osc99FilterVerdict::Keep
        );
    }

    #[test]
    fn replacing_the_filter_leaves_registered_instances_alone() {
        let (pane, _) = two_panes();
        let mut registry = Osc99Registry::default();
        register(&mut registry, &pane, Some("kept"));

        registry.set_filter(filter_of(&["all"]));

        assert_eq!(ids_in_registry(&registry, &pane), vec![Some("kept".into())]);
    }

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
    fn a_linux_only_instance_shows_on_the_linux_leg_and_shows_no_toast() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();

        let (effects, instance) =
            register_with(&mut registry, &a, &data(None), &[Osc99Leg::LinuxSystem]);

        let [
            Osc99Effect::ShowLinuxSystem {
                instance: shown,
                replaces: None,
                notification,
            },
        ] = effects.as_slice()
        else {
            panic!("expected one ShowLinuxSystem replacing nothing, got {effects:?}");
        };
        assert_eq!(*shown, instance);
        assert_eq!(**notification, data(None));
    }

    #[test]
    fn a_toast_plus_linux_instance_shows_on_both_legs() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();

        let (effects, _) = register_with(
            &mut registry,
            &a,
            &data(None),
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem],
        );

        assert!(matches!(
            effects.as_slice(),
            [
                Osc99Effect::ShowToast { .. },
                Osc99Effect::ShowLinuxSystem { .. }
            ]
        ));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn updating_a_linux_instance_replaces_the_old_desktop_notification() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, first) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::LinuxSystem],
        );

        let (effects, second) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::LinuxSystem],
        );

        let [
            Osc99Effect::ShowLinuxSystem {
                instance,
                replaces: Some(replaced),
                ..
            },
        ] = effects.as_slice()
        else {
            panic!("expected one ShowLinuxSystem replacing the first, got {effects:?}");
        };
        assert_eq!(*instance, second);
        assert_eq!(*replaced, first);
        assert!(
            registry
                .on_system_event(first, Osc99SystemLegEvent::Closed)
                .is_empty(),
            "the replaced notification's close is for a forgotten instance"
        );
        assert_eq!(registry.instances().len(), 1, "the update is live");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn updating_after_the_linux_leg_closed_replaces_nothing_on_it() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, first) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem],
        );
        let _ = registry.on_system_event(first, Osc99SystemLegEvent::Closed);

        let (effects, _) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem],
        );

        assert!(
            matches!(
                effects.as_slice(),
                [
                    Osc99Effect::ShowToast {
                        replaces: Some(_),
                        ..
                    },
                    Osc99Effect::ShowLinuxSystem { replaces: None, .. }
                ]
            ),
            "got {effects:?}"
        );
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

        let [
            Osc99Effect::CloseToast(old),
            Osc99Effect::ShowLinuxSystem { replaces: None, .. },
        ] = effects.as_slice()
        else {
            panic!("expected CloseToast then a fresh ShowLinuxSystem, got {effects:?}");
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

        // The toast is gone, so nothing is replaced; the still-open Linux leg
        // is not wanted any more and is closed.
        assert!(
            matches!(
                effects.as_slice(),
                [
                    Osc99Effect::CloseLinuxSystem(old),
                    Osc99Effect::ShowToast { replaces: None, .. }
                ] if *old == first
            ),
            "got {effects:?}"
        );
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
            data(None),
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem, Osc99Leg::Toast],
            Instant::now(),
        );

        let instance = &registry.instances()[0];
        assert_eq!(instance.open_legs, [Osc99Leg::Toast, Osc99Leg::LinuxSystem]);
        assert_eq!(instance.shown_legs, instance.open_legs);

        let _ = registry.register(
            a.origin,
            a.weak(),
            data(None),
            &[Osc99Leg::Toast, Osc99Leg::UntrackedSystem],
            Instant::now(),
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

    fn button_pressed(instance: Osc99InstanceId, n: u8) -> ToastEvent {
        ToastEvent {
            key: instance.to_toast_key(),
            kind: ToastEventKind::Button(n),
        }
    }

    #[test]
    fn button_press_with_a_report_request_reports_the_one_based_button() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Silent),
            &[Osc99Leg::Toast],
        );

        let reports = replies(registry.on_toast_event(button_pressed(instance, 2)));

        assert_eq!(reports.len(), 1);
        assert!(
            matches!(
                &reports[0],
                GuiReply::Osc99Activation { id: Some(i), button: Some(b) } if i == "n" && b == "2"
            ),
            "got {:?}",
            reports[0]
        );
        assert!(registry.instances().is_empty(), "forgotten without c=1");
    }

    #[test]
    fn button_press_without_a_report_request_sends_no_reply() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);

        assert!(
            registry
                .on_toast_event(button_pressed(instance, 2))
                .is_empty()
        );
    }

    #[test]
    fn button_press_with_close_reporting_is_followed_by_exactly_one_close() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast],
        );

        let reports = replies(registry.on_toast_event(button_pressed(instance, 1)));
        assert_eq!(reports.len(), 1);
        assert!(matches!(
            &reports[0],
            GuiReply::Osc99Activation { button: Some(b), .. } if b == "1"
        ));
        assert_eq!(registry.instances().len(), 1, "kept until the toast closes");

        let reports = replies(registry.on_toast_event(closed(instance)));
        assert_eq!(reports.len(), 1);
        assert!(is_tracked_close(&reports[0], "n"), "got {:?}", reports[0]);
        assert!(registry.on_toast_event(closed(instance)).is_empty());
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

    // ── focus on activation ─────────────────────────────────────────────

    /// Data with `a=focus` / `a=-focus` and the given reporting.
    fn focus_data(
        focus: Osc99ActivationFocus,
        reporting: Osc99ActivationReporting,
    ) -> Notification99Data {
        Notification99Data {
            activation_focus: focus,
            activation_reporting: reporting,
            ..data(Some("n"))
        }
    }

    #[test]
    fn activation_with_focus_and_report_replies_then_focuses_the_pane() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &focus_data(
                Osc99ActivationFocus::Focus,
                Osc99ActivationReporting::Report,
            ),
            &[Osc99Leg::Toast],
        );

        let effects = registry.on_toast_event(activated(instance));

        let [
            Osc99Effect::Reply { reply, .. },
            Osc99Effect::FocusPane(origin),
        ] = effects.as_slice()
        else {
            panic!("expected the report then FocusPane, got {effects:?}");
        };
        assert!(is_activation(reply, "n"), "got {reply:?}");
        assert_eq!(*origin, a.origin);
    }

    #[test]
    fn activation_with_focus_but_no_report_only_focuses_the_pane() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &focus_data(
                Osc99ActivationFocus::Focus,
                Osc99ActivationReporting::Silent,
            ),
            &[Osc99Leg::Toast],
        );

        let effects = registry.on_toast_event(activated(instance));

        assert!(
            matches!(effects.as_slice(), [Osc99Effect::FocusPane(o)] if *o == a.origin),
            "got {effects:?}"
        );
    }

    #[test]
    fn activation_without_focus_never_focuses_the_pane() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &focus_data(
                Osc99ActivationFocus::NoFocus,
                Osc99ActivationReporting::Report,
            ),
            &[Osc99Leg::Toast],
        );

        let effects = registry.on_toast_event(activated(instance));

        assert!(
            matches!(effects.as_slice(), [Osc99Effect::Reply { .. }]),
            "only the report, got {effects:?}"
        );
    }

    #[test]
    fn button_press_with_focus_replies_with_the_button_then_focuses_the_pane() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &focus_data(
                Osc99ActivationFocus::Focus,
                Osc99ActivationReporting::Report,
            ),
            &[Osc99Leg::Toast],
        );

        let effects = registry.on_toast_event(button_pressed(instance, 2));

        let [
            Osc99Effect::Reply {
                reply:
                    GuiReply::Osc99Activation {
                        button: Some(b), ..
                    },
                ..
            },
            Osc99Effect::FocusPane(origin),
        ] = effects.as_slice()
        else {
            panic!("expected the button report then FocusPane, got {effects:?}");
        };
        assert_eq!(b, "2");
        assert_eq!(*origin, a.origin);
    }

    #[test]
    fn dismissing_a_focus_instance_does_not_focus_the_pane() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &focus_data(
                Osc99ActivationFocus::Focus,
                Osc99ActivationReporting::Report,
            ),
            &[Osc99Leg::Toast],
        );

        assert!(registry.on_toast_event(closed(instance)).is_empty());
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

    // ── Linux system leg ────────────────────────────────────────────────

    #[cfg(all(unix, not(target_os = "macos")))]
    fn linux_activated(
        registry: &mut Osc99Registry,
        instance: Osc99InstanceId,
        activation: Osc99LegActivation,
    ) -> Vec<Osc99Effect> {
        registry.on_system_event(instance, Osc99SystemLegEvent::Activated(activation))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_linux_button_activation_reports_the_one_based_button_then_focuses() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &focus_data(
                Osc99ActivationFocus::Focus,
                Osc99ActivationReporting::Report,
            ),
            &[Osc99Leg::LinuxSystem],
        );

        let effects = linux_activated(&mut registry, instance, Osc99LegActivation::Button(2));

        let [
            Osc99Effect::Reply {
                reply:
                    GuiReply::Osc99Activation {
                        id: Some(id),
                        button: Some(button),
                    },
                ..
            },
            Osc99Effect::FocusPane(origin),
        ] = effects.as_slice()
        else {
            panic!("expected the button report then FocusPane, got {effects:?}");
        };
        assert_eq!(id, "n");
        assert_eq!(button, "2");
        assert_eq!(*origin, a.origin);
        assert!(registry.instances().is_empty(), "forgotten without c=1");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_linux_body_activation_reports_without_a_button() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Silent),
            &[Osc99Leg::LinuxSystem],
        );

        let reports = replies(linux_activated(
            &mut registry,
            instance,
            Osc99LegActivation::Body,
        ));

        assert_eq!(reports.len(), 1);
        assert!(is_activation(&reports[0], "n"), "got {:?}", reports[0]);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_linux_activation_without_a_report_request_or_focus_does_nothing() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::LinuxSystem],
        );

        assert!(linux_activated(&mut registry, instance, Osc99LegActivation::Body).is_empty());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_linux_activation_with_close_reporting_is_followed_by_exactly_one_close() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::LinuxSystem],
        );

        let reports = replies(linux_activated(
            &mut registry,
            instance,
            Osc99LegActivation::Button(1),
        ));
        assert_eq!(reports.len(), 1);
        assert_eq!(registry.instances().len(), 1, "kept until the leg closes");

        let reports = replies(registry.on_system_event(instance, Osc99SystemLegEvent::Closed));
        assert_eq!(reports.len(), 1);
        assert!(is_tracked_close(&reports[0], "n"), "got {:?}", reports[0]);
        assert!(registry.instances().is_empty());
        assert!(
            registry
                .on_system_event(instance, Osc99SystemLegEvent::Closed)
                .is_empty()
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_linux_close_with_close_reporting_sends_exactly_one_report() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::LinuxSystem],
        );

        let reports = replies(registry.on_system_event(instance, Osc99SystemLegEvent::Closed));

        assert_eq!(reports.len(), 1, "no longer suppressed");
        assert!(is_tracked_close(&reports[0], "n"));
        assert!(registry.instances().is_empty());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_linux_close_without_close_reporting_is_silent() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::LinuxSystem],
        );

        assert!(
            registry
                .on_system_event(instance, Osc99SystemLegEvent::Closed)
                .is_empty()
        );
        assert!(registry.instances().is_empty());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_failed_linux_show_closes_the_leg() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, reporting) = register_with(
            &mut registry,
            &a,
            &reporting_data("r", Osc99CloseReporting::Report),
            &[Osc99Leg::LinuxSystem],
        );
        let (_, silent) = register_with(
            &mut registry,
            &a,
            &data(Some("s")),
            &[Osc99Leg::LinuxSystem],
        );

        let reports = replies(registry.on_system_event(reporting, Osc99SystemLegEvent::ShowFailed));
        assert_eq!(reports.len(), 1);
        assert!(is_tracked_close(&reports[0], "r"));

        assert!(
            registry
                .on_system_event(silent, Osc99SystemLegEvent::ShowFailed)
                .is_empty()
        );
        assert!(registry.instances().is_empty());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_failed_linux_show_leaves_the_toast_leg_open() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem],
        );

        assert!(
            registry
                .on_system_event(instance, Osc99SystemLegEvent::ShowFailed)
                .is_empty(),
            "the toast is still displayed, so no report yet"
        );
        assert_eq!(registry.instances()[0].open_legs, [Osc99Leg::Toast]);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_toast_plus_linux_instance_reports_one_close_after_both_legs_close() {
        for linux_first in [false, true] {
            let (a, _b) = two_panes();
            let mut registry = Osc99Registry::default();
            let (_, instance) = register_with(
                &mut registry,
                &a,
                &reporting_data("n", Osc99CloseReporting::Report),
                &[Osc99Leg::Toast, Osc99Leg::LinuxSystem],
            );

            let (first, second) = if linux_first {
                (
                    registry.on_system_event(instance, Osc99SystemLegEvent::Closed),
                    registry.on_toast_event(closed(instance)),
                )
            } else {
                (
                    registry.on_toast_event(closed(instance)),
                    registry.on_system_event(instance, Osc99SystemLegEvent::Closed),
                )
            };

            assert!(first.is_empty(), "linux_first={linux_first}: one leg open");
            let reports = replies(second);
            assert_eq!(reports.len(), 1, "linux_first={linux_first}");
            assert!(is_tracked_close(&reports[0], "n"));
            assert!(registry.instances().is_empty());
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_system_event_for_an_unknown_instance_is_ignored() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::LinuxSystem],
        );
        let unknown = Osc99InstanceId(instance.0 + 1000);

        for event in [
            Osc99SystemLegEvent::Closed,
            Osc99SystemLegEvent::ShowFailed,
            Osc99SystemLegEvent::Activated(Osc99LegActivation::Body),
        ] {
            assert!(registry.on_system_event(unknown, event).is_empty());
        }
        assert_eq!(registry.instances().len(), 1);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn close_of_a_linux_leg_waits_for_the_service_then_reports_once() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::LinuxSystem],
        );

        let effects = registry.close(a.origin, "n");

        assert_eq!(
            close_targets(&effects),
            [(Osc99Leg::LinuxSystem, instance)],
            "the desktop notification is closed through the service"
        );
        assert_eq!(registry.instances().len(), 1, "waiting for the Closed");
        assert!(registry.close(a.origin, "n").is_empty(), "idempotent");

        let reports = replies(registry.on_system_event(instance, Osc99SystemLegEvent::Closed));
        assert_eq!(reports.len(), 1);
        assert!(is_tracked_close(&reports[0], "n"));
        assert!(registry.instances().is_empty());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn close_of_a_linux_leg_without_close_reporting_is_silent() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::LinuxSystem],
        );

        assert_eq!(
            close_targets(&registry.close(a.origin, "n")),
            [(Osc99Leg::LinuxSystem, instance)]
        );
        assert!(
            registry
                .on_system_event(instance, Osc99SystemLegEvent::Closed)
                .is_empty()
        );
        assert!(registry.instances().is_empty());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn close_of_a_toast_plus_linux_instance_reports_once_after_both_legs_close() {
        for linux_first in [false, true] {
            let (a, _b) = two_panes();
            let mut registry = Osc99Registry::default();
            let (_, instance) = register_with(
                &mut registry,
                &a,
                &reporting_data("n", Osc99CloseReporting::Report),
                &[Osc99Leg::Toast, Osc99Leg::LinuxSystem],
            );

            let effects = registry.close(a.origin, "n");

            assert_eq!(
                close_targets(&effects),
                [
                    (Osc99Leg::Toast, instance),
                    (Osc99Leg::LinuxSystem, instance)
                ]
            );
            let (first, second) = if linux_first {
                (
                    registry.on_system_event(instance, Osc99SystemLegEvent::Closed),
                    registry.on_toast_event(closed(instance)),
                )
            } else {
                (
                    registry.on_toast_event(closed(instance)),
                    registry.on_system_event(instance, Osc99SystemLegEvent::Closed),
                )
            };
            assert!(first.is_empty(), "linux_first={linux_first}: one leg open");
            let reports = replies(second);
            assert_eq!(reports.len(), 1, "linux_first={linux_first}");
            assert!(is_tracked_close(&reports[0], "n"));
            assert!(registry.instances().is_empty());
        }
    }

    #[test]
    fn an_open_linux_leg_keeps_the_instance_after_the_toast_closes() {
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

    // ── the untracked system leg (macOS and Windows) ────────────────────

    fn is_untracked_close(reply: &GuiReply, id: &str) -> bool {
        matches!(
            reply,
            GuiReply::Osc99Closed { id: Some(i), tracking: Osc99CloseTracking::Untracked } if i == id
        )
    }

    #[test]
    fn an_untracked_only_display_with_close_reporting_replies_untracked_at_once() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let wanted = reporting_data("n", Osc99CloseReporting::Report);

        let (effects, _) = register_with(&mut registry, &a, &wanted, &[Osc99Leg::UntrackedSystem]);

        let [
            Osc99Effect::ShowUntrackedSystem(shown),
            Osc99Effect::Reply { reply, .. },
        ] = effects.as_slice()
        else {
            panic!("expected ShowUntrackedSystem then the untracked close, got {effects:?}");
        };
        assert_eq!(**shown, wanted);
        assert!(is_untracked_close(reply, "n"), "got {reply:?}");
    }

    #[test]
    fn an_untracked_only_display_is_not_kept() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let short_lived = Notification99Data {
            expiry: ms(500),
            ..reporting_data("n", Osc99CloseReporting::Report)
        };
        let now = Instant::now();

        let effects = registry.register(
            a.origin,
            a.weak(),
            short_lived,
            &[Osc99Leg::UntrackedSystem],
            now,
        );

        assert!(
            matches!(
                effects.as_slice(),
                [
                    Osc99Effect::ShowUntrackedSystem(_),
                    Osc99Effect::Reply { .. }
                ]
            ),
            "it is still shown and answered: {effects:?}"
        );
        assert!(registry.instances().is_empty(), "nothing is registered");
        assert_eq!(alive_ids(&registry, &a), Vec::<String>::new());
        assert_eq!(registry.next_deadline(), None);
        assert!(registry.tick(now + Duration::from_secs(1)).is_empty());
        assert!(registry.close(a.origin, "n").is_empty());
    }

    #[test]
    fn an_untracked_only_update_still_closes_the_old_instance() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, first) = register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);

        let (effects, _) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::UntrackedSystem],
        );

        assert!(
            matches!(
                effects.as_slice(),
                [Osc99Effect::CloseToast(old), Osc99Effect::ShowUntrackedSystem(_)] if *old == first
            ),
            "the old toast closes and the new one shows: {effects:?}"
        );
        assert!(registry.instances().is_empty(), "the old one was forgotten");
        assert!(
            registry.on_toast_event(closed(first)).is_empty(),
            "its late close is ignored"
        );
    }

    #[test]
    fn toast_plus_untracked_with_close_reporting_replies_untracked_and_never_tracked() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();

        let (effects, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast, Osc99Leg::UntrackedSystem],
        );

        let [
            Osc99Effect::ShowToast { .. },
            Osc99Effect::ShowUntrackedSystem(_),
            Osc99Effect::Reply { reply, .. },
        ] = effects.as_slice()
        else {
            panic!("expected toast, untracked show, untracked close, got {effects:?}");
        };
        assert!(is_untracked_close(reply, "n"), "got {reply:?}");

        // The toast closing is the last open leg, yet owes no tracked report:
        // the untracked one was sent at display.
        assert!(registry.on_toast_event(closed(instance)).is_empty());
        assert!(registry.instances().is_empty());
    }

    #[test]
    fn toast_plus_untracked_activation_with_close_reporting_sends_no_tracked_close() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast, Osc99Leg::UntrackedSystem],
        );

        let activation = registry.on_toast_event(activated(instance));
        let later = registry.on_toast_event(closed(instance));

        let replies = replies(activation.into_iter().chain(later).collect());
        assert!(
            replies.iter().all(|reply| !is_tracked_close(reply, "n")),
            "got {replies:?}"
        );
    }

    #[test]
    fn an_untracked_only_display_without_close_reporting_sends_no_reply() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();

        let (effects, _) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::UntrackedSystem],
        );

        assert!(
            matches!(effects.as_slice(), [Osc99Effect::ShowUntrackedSystem(_)]),
            "got {effects:?}"
        );
    }

    #[test]
    fn updating_an_untracked_instance_shows_a_new_notification_and_reports_again() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let wanted = reporting_data("n", Osc99CloseReporting::Report);
        let (_, first) = register_with(&mut registry, &a, &wanted, &[Osc99Leg::UntrackedSystem]);

        let (effects, second) =
            register_with(&mut registry, &a, &wanted, &[Osc99Leg::UntrackedSystem]);

        let [
            Osc99Effect::ShowUntrackedSystem(_),
            Osc99Effect::Reply { reply, .. },
        ] = effects.as_slice()
        else {
            panic!("expected a fresh show and its untracked close, got {effects:?}");
        };
        assert!(is_untracked_close(reply, "n"), "got {reply:?}");
        assert_ne!(first, second);
        assert!(
            registry.instances().is_empty(),
            "an untracked display is not kept"
        );
    }

    #[test]
    fn updating_an_untracked_instance_without_close_reporting_shows_without_a_reply() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let _ = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::UntrackedSystem],
        );

        let (effects, _) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::UntrackedSystem],
        );

        assert!(
            matches!(effects.as_slice(), [Osc99Effect::ShowUntrackedSystem(_)]),
            "got {effects:?}"
        );
    }

    #[test]
    fn the_executor_hands_an_untracked_show_to_the_untracked_backend() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let wanted = reporting_data("n", Osc99CloseReporting::Report);
        let (effects, _) = register_with(
            &mut registry,
            &a,
            &wanted,
            &[Osc99Leg::Toast, Osc99Leg::UntrackedSystem],
        );
        let mut toasts = ToastStack::default();
        let mut untracked = RecordingUntrackedBackend::default();

        let (focus, failed_shows, failed_closes) = execute_all_with_untracked(
            effects,
            &mut toasts,
            &mut RecordingSystemBackend::default(),
            &mut untracked,
        );

        assert_eq!(untracked.shown, [wanted]);
        assert_eq!(toasts.len(), 1, "the toast leg still showed");
        assert_eq!(focus, Vec::new());
        assert!(
            failed_shows.is_empty() && failed_closes.is_empty(),
            "an untracked show cannot fail"
        );
    }

    // ── activation closes the other legs ────────────────────────────────

    /// Data for a toast-plus-Linux instance with `a=report` and the given
    /// `c=` setting, registered on both legs.
    #[cfg(all(unix, not(target_os = "macos")))]
    fn register_both_legs(
        registry: &mut Osc99Registry,
        pane: &TestPane,
        close: Osc99CloseReporting,
    ) -> Osc99InstanceId {
        register_with(
            registry,
            pane,
            &reporting_data("n", close),
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem],
        )
        .1
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn activating_the_toast_closes_the_linux_leg_and_forgets_the_instance() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let instance = register_both_legs(&mut registry, &a, Osc99CloseReporting::Silent);

        let effects = registry.on_toast_event(activated(instance));

        let [Osc99Effect::Reply { reply, .. }, close] = effects.as_slice() else {
            panic!("expected the activation report then one close, got {effects:?}");
        };
        assert!(is_activation(reply, "n"));
        assert_eq!(
            close_targets(std::slice::from_ref(close)),
            [(Osc99Leg::LinuxSystem, instance)],
            "the activated toast closes itself; only the Linux leg is closed"
        );
        assert!(registry.instances().is_empty(), "c=0: forgotten at once");
        assert!(
            registry
                .on_system_event(instance, Osc99SystemLegEvent::Closed)
                .is_empty(),
            "the other leg's Closed is for a forgotten instance"
        );
        assert!(registry.on_toast_event(closed(instance)).is_empty());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn activating_the_linux_leg_closes_the_toast_and_forgets_the_instance() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let instance = register_both_legs(&mut registry, &a, Osc99CloseReporting::Silent);

        let effects = linux_activated(&mut registry, instance, Osc99LegActivation::Button(1));

        let [Osc99Effect::Reply { .. }, close] = effects.as_slice() else {
            panic!("expected the activation report then one close, got {effects:?}");
        };
        assert_eq!(
            close_targets(std::slice::from_ref(close)),
            [(Osc99Leg::Toast, instance)],
            "the daemon closes the activated notification itself"
        );
        assert!(registry.instances().is_empty(), "c=0: forgotten at once");
        assert!(registry.on_toast_event(closed(instance)).is_empty());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn activating_with_close_reporting_reports_once_after_every_leg_closes() {
        for toast_activated in [true, false] {
            let (a, _b) = two_panes();
            let mut registry = Osc99Registry::default();
            let instance = register_both_legs(&mut registry, &a, Osc99CloseReporting::Report);

            let effects = if toast_activated {
                registry.on_toast_event(activated(instance))
            } else {
                linux_activated(&mut registry, instance, Osc99LegActivation::Body)
            };
            let [Osc99Effect::Reply { reply, .. }, close] = effects.as_slice() else {
                panic!("toast_activated={toast_activated}: got {effects:?}");
            };
            assert!(is_activation(reply, "n"));
            let other = if toast_activated {
                Osc99Leg::LinuxSystem
            } else {
                Osc99Leg::Toast
            };
            assert_eq!(
                close_targets(std::slice::from_ref(close)),
                [(other, instance)]
            );
            assert_eq!(registry.instances().len(), 1, "c=1: waits for both legs");
            assert!(
                registry.close(a.origin, "n").is_empty(),
                "every leg is already closing"
            );

            // Both legs now report `Closed`, in either order.
            let (first, second) = if toast_activated {
                (
                    registry.on_toast_event(closed(instance)),
                    registry.on_system_event(instance, Osc99SystemLegEvent::Closed),
                )
            } else {
                (
                    registry.on_system_event(instance, Osc99SystemLegEvent::Closed),
                    registry.on_toast_event(closed(instance)),
                )
            };
            assert!(first.is_empty(), "toast_activated={toast_activated}");
            let reports = replies(second);
            assert_eq!(reports.len(), 1, "exactly one close report");
            assert!(is_tracked_close(&reports[0], "n"));
            assert!(registry.instances().is_empty());
        }
    }

    #[test]
    fn a_repeated_activation_of_an_activated_instance_is_ignored() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let notification = Notification99Data {
            activation_focus: Osc99ActivationFocus::Focus,
            ..reporting_data("n", Osc99CloseReporting::Report)
        };
        let (_, instance) = register_with(&mut registry, &a, &notification, &[Osc99Leg::Toast]);

        let first = registry.on_toast_event(activated(instance));
        assert!(
            matches!(
                first.as_slice(),
                [Osc99Effect::Reply { .. }, Osc99Effect::FocusPane(_)]
            ),
            "the first activation reports and focuses: {first:?}"
        );

        assert!(
            registry.on_toast_event(activated(instance)).is_empty(),
            "no second reply and no second focus request"
        );
        assert!(
            registry
                .on_toast_event(button_pressed(instance, 1))
                .is_empty(),
            "a button press on the activated instance is ignored too"
        );

        let reports = replies(registry.on_toast_event(closed(instance)));
        assert_eq!(reports.len(), 1, "still exactly one close report");
        assert!(is_tracked_close(&reports[0], "n"));
    }

    #[test]
    fn an_activation_after_p_close_is_ignored() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast],
        );
        let _ = registry.close(a.origin, "n");

        assert!(registry.on_toast_event(activated(instance)).is_empty());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn activating_a_single_leg_instance_emits_no_close() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::LinuxSystem],
        );

        let effects = linux_activated(&mut registry, instance, Osc99LegActivation::Body);

        assert!(
            matches!(effects.as_slice(), [Osc99Effect::Reply { .. }]),
            "got {effects:?}"
        );
    }

    // ── update moves an instance off a leg ──────────────────────────────

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn updating_from_linux_only_to_toast_only_closes_the_old_desktop_notification() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, first) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::LinuxSystem],
        );

        let (effects, second) =
            register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);

        let [
            Osc99Effect::CloseLinuxSystem(old),
            Osc99Effect::ShowToast {
                instance,
                replaces: None,
                ..
            },
        ] = effects.as_slice()
        else {
            panic!("expected CloseLinuxSystem then a fresh ShowToast, got {effects:?}");
        };
        assert_eq!(*old, first);
        assert_eq!(*instance, second);
        assert!(
            registry
                .on_system_event(first, Osc99SystemLegEvent::Closed)
                .is_empty(),
            "the old desktop notification's Closed reports nothing"
        );
        assert_eq!(registry.instances().len(), 1, "the update is live");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn updating_from_both_legs_to_toast_only_replaces_the_toast_and_closes_the_linux_leg() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let first = register_both_legs(&mut registry, &a, Osc99CloseReporting::Silent);

        let (effects, _) = register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);

        let [
            Osc99Effect::CloseLinuxSystem(old),
            Osc99Effect::ShowToast {
                replaces: Some(replaced),
                ..
            },
        ] = effects.as_slice()
        else {
            panic!("expected CloseLinuxSystem then ShowToast replacing, got {effects:?}");
        };
        assert_eq!(*old, first);
        assert_eq!(*replaced, first);
    }

    // ── capacity and pane lifetime ──────────────────────────────────────

    /// `n` live panes, with distinct ids.
    fn panes(n: usize) -> Vec<TestPane> {
        let mut ids = PaneIdGenerator::new(0);
        (0..n).map(|_| TestPane::new(ids.next_id())).collect()
    }

    #[test]
    fn the_global_cap_forgets_the_oldest_instance() {
        // Four panes at their quota fill the global cap exactly; a fifth
        // pane's registration pushes the overall oldest out.
        let panes = panes(5);
        let mut registry = Osc99Registry::default();
        for pane in &panes[..4] {
            for n in 0..MAX_OSC99_INSTANCES_PER_PANE {
                register(&mut registry, pane, Some(&format!("id{n}")));
            }
        }
        assert_eq!(registry.instances().len(), MAX_OSC99_INSTANCES);

        register(&mut registry, &panes[4], Some("fresh"));

        assert_eq!(registry.instances().len(), MAX_OSC99_INSTANCES);
        let first = alive_ids(&registry, &panes[0]);
        assert!(
            !first.contains(&"id0".to_owned()),
            "the oldest must be gone"
        );
        assert!(first.contains(&"id1".to_owned()));
        assert_eq!(alive_ids(&registry, &panes[4]), ["fresh"]);
    }

    #[test]
    fn the_pane_quota_evicts_that_panes_oldest_and_leaves_other_panes_alone() {
        let (a, b) = two_panes();
        let mut registry = Osc99Registry::default();
        register(&mut registry, &b, Some("b-first"));
        for n in 0..MAX_OSC99_INSTANCES_PER_PANE {
            register(&mut registry, &a, Some(&format!("id{n}")));
        }
        register(&mut registry, &b, Some("b-second"));
        assert_eq!(
            ids_in_registry(&registry, &a).len(),
            MAX_OSC99_INSTANCES_PER_PANE
        );

        register(&mut registry, &a, Some("id-over"));

        let ids = alive_ids(&registry, &a);
        assert_eq!(ids.len(), MAX_OSC99_INSTANCES_PER_PANE);
        assert!(!ids.contains(&"id0".to_owned()), "A's oldest must be gone");
        assert!(ids.contains(&"id1".to_owned()));
        assert!(ids.contains(&"id-over".to_owned()));
        assert_eq!(
            alive_ids(&registry, &b),
            ["b-first", "b-second"],
            "pane B is unaffected, including its older instance"
        );
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
        for n in 1..MAX_OSC99_INSTANCES_PER_PANE {
            register(&mut registry, &a, Some(&format!("id{n}")));
        }
        assert_eq!(registry.instances().len(), MAX_OSC99_INSTANCES_PER_PANE);

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

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn cap_eviction_closes_every_open_leg_of_the_evicted_instance() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, oldest) = register_with(
            &mut registry,
            &a,
            &reporting_data("id0", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem],
        );
        for n in 1..MAX_OSC99_INSTANCES_PER_PANE {
            register(&mut registry, &a, Some(&format!("id{n}")));
        }

        let (effects, _) =
            register_with(&mut registry, &a, &data(Some("last")), &[Osc99Leg::Toast]);

        let evicted: Vec<_> = close_targets(
            &effects
                .into_iter()
                .filter(|e| {
                    matches!(
                        e,
                        Osc99Effect::CloseToast(_) | Osc99Effect::CloseLinuxSystem(_)
                    )
                })
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            evicted,
            [(Osc99Leg::Toast, oldest), (Osc99Leg::LinuxSystem, oldest)]
        );
        assert!(registry.on_toast_event(closed(oldest)).is_empty());
        assert!(
            registry
                .on_system_event(oldest, Osc99SystemLegEvent::Closed)
                .is_empty(),
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
        let _ = registry.prune_dead_panes();

        assert_eq!(registry.instances().len(), 1);
        assert_eq!(registry.instances()[0].origin.pane_id, b.origin.pane_id);
    }

    #[test]
    fn register_takes_the_notification_by_value_without_copying_its_icon() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let notification = Notification99Data {
            icon_data: Some(vec![1, 2, 3, 4]),
            ..data(Some("n"))
        };
        let original = notification.icon_data.as_ref().map(Vec::as_ptr);

        let effects = registry.register(
            a.origin,
            a.weak(),
            notification,
            &[Osc99Leg::LinuxSystem],
            Instant::now(),
        );

        let [Osc99Effect::ShowLinuxSystem { notification, .. }] = effects.as_slice() else {
            panic!("one system show expected, got {effects:?}");
        };
        assert_eq!(
            notification.icon_data.as_ref().map(Vec::as_ptr),
            original,
            "the shared notification is the caller's allocation, not a copy"
        );
    }

    #[test]
    fn prune_dead_panes_closes_the_open_legs_of_a_gone_pane_without_replies() {
        let (a, b) = two_panes();
        let mut registry = Osc99Registry::default();
        let reporting = reporting_data("a1", Osc99CloseReporting::Report);
        let (_, toast_only) = register_with(&mut registry, &a, &reporting, &[Osc99Leg::Toast]);
        let (_, both) = register_with(
            &mut registry,
            &a,
            &reporting_data("a2", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem],
        );
        register(&mut registry, &b, Some("b1"));

        drop(a.tx);
        let effects = registry.prune_dead_panes();

        let closes: Vec<(Osc99Leg, Osc99InstanceId)> = effects
            .iter()
            .map(|effect| match effect {
                Osc99Effect::CloseToast(instance) => (Osc99Leg::Toast, *instance),
                Osc99Effect::CloseLinuxSystem(instance) => (Osc99Leg::LinuxSystem, *instance),
                other => panic!("pruning only closes legs, got {other:?}"),
            })
            .collect();
        assert_eq!(
            closes,
            [
                (Osc99Leg::Toast, toast_only),
                (Osc99Leg::Toast, both),
                (Osc99Leg::LinuxSystem, both),
            ]
        );
        assert_eq!(registry.instances().len(), 1, "the live pane's stays");
        assert!(
            registry.prune_dead_panes().is_empty(),
            "nothing left to prune"
        );
        assert!(
            registry.on_toast_event(closed(toast_only)).is_empty(),
            "the later Closed is for a forgotten instance and is ignored"
        );
    }

    #[test]
    fn prune_dead_panes_does_not_close_a_leg_already_asked_to_close() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("a1", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast],
        );
        let asked = registry.close(a.origin, "a1");
        assert!(matches!(asked[..], [Osc99Effect::CloseToast(i)] if i == instance));

        drop(a.tx);

        assert!(
            registry.prune_dead_panes().is_empty(),
            "the toast was already asked to close"
        );
        assert!(registry.instances().is_empty());
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
    fn alive_leaves_out_instances_whose_close_was_requested() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        register(&mut registry, &a, Some("closing"));
        register(&mut registry, &a, Some("open"));
        let _ = registry.close(a.origin, "closing");

        assert_eq!(
            alive_ids(&registry, &a),
            ["open"],
            "the closing instance is still registered but not alive"
        );
        assert_eq!(ids_in_registry(&registry, &a).len(), 2);
    }

    #[test]
    fn alive_lists_an_activated_instance_until_its_leg_closes() {
        // kitty keeps an activated `c=1` notification listed until the
        // display closes it: the activated leg is still open, and nothing
        // asked it to close.
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let (_, activated_one) = register_with(
            &mut registry,
            &a,
            &reporting_data("done", Osc99CloseReporting::Report),
            &[Osc99Leg::Toast],
        );
        register(&mut registry, &a, Some("open"));
        let _ = registry.on_toast_event(activated(activated_one));
        assert_eq!(alive_ids(&registry, &a), ["done", "open"]);

        let _ = registry.on_toast_event(closed(activated_one));
        assert_eq!(alive_ids(&registry, &a), ["open"]);
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

        let _ = execute(effects, &mut toasts, &mut NoSystemBackend);
    }

    // ── executor ────────────────────────────────────────────────────────

    #[test]
    fn the_executor_records_focus_requests_in_order_without_touching_toasts() {
        let (a, b) = two_panes();
        let mut toasts = ToastStack::default();
        let mut system = RecordingSystemBackend::default();

        let (focus_requests, failed_shows) = execute(
            vec![
                Osc99Effect::FocusPane(a.origin),
                Osc99Effect::FocusPane(b.origin),
            ],
            &mut toasts,
            &mut system,
        );

        assert_eq!(focus_requests, [a.origin, b.origin]);
        assert_eq!(failed_shows, Vec::new());
        assert_eq!(toasts.len(), 0);
        assert_eq!(system.shown, Vec::new());
    }

    #[test]
    fn the_executor_collects_the_instances_whose_show_could_not_be_queued() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let mut toasts = ToastStack::default();
        let (effects, instance) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::LinuxSystem],
        );

        let (_, failed) = execute(effects, &mut toasts, &mut NoSystemBackend);
        assert_eq!(failed, [instance], "NoSystemBackend fails every show");

        let (effects, second) = register_with(
            &mut registry,
            &a,
            &data(Some("m")),
            &[Osc99Leg::LinuxSystem],
        );
        let mut failing = RecordingSystemBackend {
            fail_shows: true,
            ..RecordingSystemBackend::default()
        };
        let (_, failed) = execute(effects, &mut toasts, &mut failing);
        assert_eq!(failed, [second]);
        assert_eq!(failing.shown.len(), 1, "the show was still attempted");

        let (effects, _) = register_with(
            &mut registry,
            &a,
            &data(Some("k")),
            &[Osc99Leg::LinuxSystem],
        );
        let (_, failed) = execute(effects, &mut toasts, &mut RecordingSystemBackend::default());
        assert_eq!(failed, Vec::new(), "a queued show is not a failure");
    }

    /// The Linux/BSD feedback loop the GUI runs: a show the backend could not
    /// queue is fed back as `ShowFailed`, which ends the only leg and sends
    /// exactly one close report.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_failed_show_fed_back_closes_the_only_leg_with_one_close_report() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let mut toasts = ToastStack::default();
        let (effects, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::LinuxSystem],
        );

        let (_, failed) = execute(effects, &mut toasts, &mut NoSystemBackend);
        assert_eq!(failed, [instance]);
        let reports: Vec<GuiReply> = failed
            .into_iter()
            .flat_map(|failed| registry.on_system_event(failed, Osc99SystemLegEvent::ShowFailed))
            .map(|effect| match effect {
                Osc99Effect::Reply { reply, .. } => reply,
                other => panic!("expected only replies, got {other:?}"),
            })
            .collect();

        assert_eq!(reports.len(), 1);
        assert!(is_tracked_close(&reports[0], "n"), "got {:?}", reports[0]);
        assert!(registry.instances().is_empty());
        assert_eq!(alive_ids(&registry, &a), Vec::<String>::new());
    }

    #[test]
    fn the_executor_hands_linux_closes_to_the_system_backend() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let mut toasts = ToastStack::default();
        let mut system = RecordingSystemBackend::default();
        let (effects, instance) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::LinuxSystem],
        );
        let _ = execute(effects, &mut toasts, &mut system);

        let effects = registry.close(a.origin, "n");
        let (_, failed_shows, failed_closes) = execute_all(effects, &mut toasts, &mut system);

        assert_eq!(system.closed, [instance]);
        assert!(
            failed_shows.is_empty() && failed_closes.is_empty(),
            "a queued close is not a failure"
        );
    }

    #[test]
    fn the_executor_collects_the_instances_whose_close_could_not_be_queued() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let mut toasts = ToastStack::default();
        let (effects, instance) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::LinuxSystem],
        );
        let _ = execute(effects, &mut toasts, &mut RecordingSystemBackend::default());

        let effects = registry.close(a.origin, "n");
        let (_, failed_shows, failed_closes) =
            execute_all(effects, &mut toasts, &mut NoSystemBackend);

        assert_eq!(
            failed_closes,
            [instance],
            "NoSystemBackend fails every close"
        );
        assert_eq!(failed_shows, Vec::new());
    }

    /// The Linux/BSD feedback loop for a close the backend could not queue:
    /// fed back as `Closed`, it ends the leg with exactly one close report.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_failed_close_fed_back_closes_the_leg_with_one_close_report() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let mut toasts = ToastStack::default();
        let (effects, instance) = register_with(
            &mut registry,
            &a,
            &reporting_data("n", Osc99CloseReporting::Report),
            &[Osc99Leg::LinuxSystem],
        );
        let _ = execute(effects, &mut toasts, &mut RecordingSystemBackend::default());

        let effects = registry.close(a.origin, "n");
        let (_, _, failed_closes) = execute_all(effects, &mut toasts, &mut NoSystemBackend);
        assert_eq!(failed_closes, [instance]);
        assert_eq!(registry.instances().len(), 1, "still waiting for a Closed");

        let reports: Vec<GuiReply> = failed_closes
            .into_iter()
            .flat_map(|failed| registry.on_system_event(failed, Osc99SystemLegEvent::Closed))
            .map(|effect| match effect {
                Osc99Effect::Reply { reply, .. } => reply,
                other => panic!("expected only replies, got {other:?}"),
            })
            .collect();

        assert_eq!(reports.len(), 1);
        assert!(is_tracked_close(&reports[0], "n"), "got {:?}", reports[0]);
        assert!(registry.instances().is_empty());
    }

    #[test]
    fn the_executor_hands_linux_shows_to_the_system_backend() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let mut toasts = ToastStack::default();
        let mut system = RecordingSystemBackend::default();

        let (effects, first) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::LinuxSystem],
        );
        let _ = execute(effects, &mut toasts, &mut system);
        let (effects, second) = register_with(
            &mut registry,
            &a,
            &data(Some("n")),
            &[Osc99Leg::LinuxSystem],
        );
        let _ = execute(effects, &mut toasts, &mut system);

        assert_eq!(
            system.shown,
            [
                RecordedShow {
                    instance: first,
                    replaces: None,
                    notification: data(Some("n")),
                },
                RecordedShow {
                    instance: second,
                    replaces: Some(first),
                    notification: data(Some("n")),
                },
            ]
        );
        assert_eq!(toasts.len(), 0, "no toast for a Linux-only instance");
    }

    #[test]
    fn the_executor_shows_replaces_and_closes_keyed_toasts() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let mut toasts = ToastStack::default();

        let (effects, first) =
            register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);
        let _ = execute(effects, &mut toasts, &mut NoSystemBackend);
        assert_eq!(toasts.len(), 1);

        let (effects, _) = register_with(&mut registry, &a, &data(Some("n")), &[Osc99Leg::Toast]);
        let _ = execute(effects, &mut toasts, &mut NoSystemBackend);
        assert_eq!(toasts.len(), 1, "updated in place, not stacked");
        assert!(
            toasts.take_events().is_empty(),
            "no event for the replaced toast"
        );

        let effects = registry.close(a.origin, "n");
        let _ = execute(effects, &mut toasts, &mut NoSystemBackend);
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

    // ── expiry ──────────────────────────────────────────────────────────

    fn ms(n: u32) -> Osc99Expiry {
        let Some(n) = NonZeroU32::new(n) else {
            panic!("test durations are non-zero");
        };
        Osc99Expiry::After(n)
    }

    /// Data with the given `w=` expiry and `c=` setting.
    fn expiring(id: &str, expiry: Osc99Expiry, close: Osc99CloseReporting) -> Notification99Data {
        Notification99Data {
            expiry,
            close_reporting: close,
            ..data(Some(id))
        }
    }

    /// Register at the injected time `at`, returning the new instance's id.
    fn register_at(
        registry: &mut Osc99Registry,
        pane: &TestPane,
        data: &Notification99Data,
        legs: &[Osc99Leg],
        at: Instant,
    ) -> Osc99InstanceId {
        let _ = registry.register(pane.origin, pane.weak(), data.clone(), legs, at);
        let Some(instance) = registry.instances().last().map(|i| i.instance) else {
            panic!("registration adds an instance");
        };
        instance
    }

    fn close_toast_targets(effects: &[Osc99Effect]) -> Vec<Osc99InstanceId> {
        effects
            .iter()
            .map(|effect| match effect {
                Osc99Effect::CloseToast(instance) => *instance,
                other => panic!("expected only CloseToast, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_toast_closes_at_its_deadline_then_reports_exactly_once() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let t0 = Instant::now();
        let notification = expiring("n", ms(5000), Osc99CloseReporting::Report);
        let instance = register_at(&mut registry, &a, &notification, &[Osc99Leg::Toast], t0);

        assert!(registry.tick(t0 + Duration::from_millis(4999)).is_empty());

        let effects = registry.tick(t0 + Duration::from_millis(5000));
        assert_eq!(close_toast_targets(&effects), [instance]);

        assert!(
            registry.tick(t0 + Duration::from_millis(6000)).is_empty(),
            "a closing instance is not closed again"
        );

        let reports = replies(registry.on_toast_event(closed(instance)));
        assert_eq!(reports.len(), 1);
        assert!(is_tracked_close(&reports[0], "n"));
        assert!(registry.instances().is_empty());
    }

    #[test]
    fn the_deadline_closes_a_toast_whose_own_lifetime_is_longer() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let t0 = Instant::now();
        let notification = Notification99Data {
            urgency: NotificationUrgency::Critical,
            ..expiring("n", ms(1000), Osc99CloseReporting::Silent)
        };
        // Documented premise: the toast alone would stay until dismissed...
        let spec = osc99_toast_spec(&Notification99Data {
            expiry: Osc99Expiry::OsDefault,
            ..notification.clone()
        });
        assert_eq!(spec.lifetime, ToastLifetime::UntilDismissed);
        // ...and the registry closes it anyway.
        let instance = register_at(&mut registry, &a, &notification, &[Osc99Leg::Toast], t0);

        let effects = registry.tick(t0 + Duration::from_millis(1000));

        assert_eq!(close_toast_targets(&effects), [instance]);
    }

    #[test]
    fn an_update_restarts_the_deadline() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let t0 = Instant::now();
        let notification = expiring("n", ms(5000), Osc99CloseReporting::Silent);
        let first = register_at(&mut registry, &a, &notification, &[Osc99Leg::Toast], t0);
        let second = register_at(
            &mut registry,
            &a,
            &notification,
            &[Osc99Leg::Toast],
            t0 + Duration::from_millis(3000),
        );
        assert_ne!(first, second);

        assert!(
            registry.tick(t0 + Duration::from_millis(5000)).is_empty(),
            "the first deadline died with the replaced instance"
        );
        assert_eq!(
            registry.next_deadline(),
            Some(t0 + Duration::from_millis(8000))
        );

        let effects = registry.tick(t0 + Duration::from_millis(8000));
        assert_eq!(close_toast_targets(&effects), [second]);
    }

    #[test]
    fn never_and_the_os_default_do_not_expire() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let t0 = Instant::now();
        for (id, expiry) in [
            ("never", Osc99Expiry::Never),
            ("os", Osc99Expiry::OsDefault),
        ] {
            let _ = register_at(
                &mut registry,
                &a,
                &expiring(id, expiry, Osc99CloseReporting::Silent),
                &[Osc99Leg::Toast],
                t0,
            );
        }

        assert_eq!(registry.next_deadline(), None);
        assert!(registry.tick(t0 + Duration::from_hours(24)).is_empty());
        assert_eq!(registry.instances().len(), 2);
    }

    #[test]
    fn next_deadline_is_the_minimum_and_skips_closing_instances() {
        let (a, b) = two_panes();
        let mut registry = Osc99Registry::default();
        let t0 = Instant::now();
        let silent = Osc99CloseReporting::Silent;
        let _late = register_at(
            &mut registry,
            &a,
            &expiring("late", ms(9000), silent),
            &[Osc99Leg::Toast],
            t0,
        );
        let _early = register_at(
            &mut registry,
            &b,
            &expiring("early", ms(2000), silent),
            &[Osc99Leg::Toast],
            t0,
        );
        let _no_deadline = register_at(
            &mut registry,
            &a,
            &expiring("none", Osc99Expiry::Never, silent),
            &[Osc99Leg::Toast],
            t0,
        );

        assert_eq!(
            registry.next_deadline(),
            Some(t0 + Duration::from_millis(2000))
        );

        // Once the earliest is closing it no longer counts.
        let effects = registry.tick(t0 + Duration::from_millis(2000));
        assert_eq!(effects.len(), 1);
        assert_eq!(
            registry.next_deadline(),
            Some(t0 + Duration::from_millis(9000))
        );
    }

    #[test]
    fn p_close_then_a_tick_past_the_deadline_does_not_close_twice() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let t0 = Instant::now();
        let notification = expiring("n", ms(1000), Osc99CloseReporting::Report);
        let instance = register_at(&mut registry, &a, &notification, &[Osc99Leg::Toast], t0);

        let effects = registry.close(a.origin, "n");
        assert_eq!(close_toast_targets(&effects), [instance]);
        assert_eq!(
            registry.next_deadline(),
            None,
            "closing: nothing to wake for"
        );

        assert!(registry.tick(t0 + Duration::from_millis(2000)).is_empty());
        assert!(
            registry.close(a.origin, "n").is_empty(),
            "a second p=close is idempotent too"
        );

        let reports = replies(registry.on_toast_event(closed(instance)));
        assert_eq!(reports.len(), 1, "exactly one close report");
    }

    #[test]
    fn a_toast_closed_by_its_own_lifetime_finishes_an_expiring_instance_once() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let t0 = Instant::now();
        let notification = expiring("n", ms(1000), Osc99CloseReporting::Report);
        let instance = register_at(&mut registry, &a, &notification, &[Osc99Leg::Toast], t0);

        // The toast leg's own `Fixed` lifetime fires first.
        let reports = replies(registry.on_toast_event(closed(instance)));
        assert_eq!(reports.len(), 1);

        // The instance is gone, so the deadline has nothing left to do.
        assert_eq!(registry.next_deadline(), None);
        assert!(registry.tick(t0 + Duration::from_millis(1000)).is_empty());
        assert!(registry.on_toast_event(closed(instance)).is_empty());
    }

    #[test]
    fn a_toast_closed_event_in_the_requested_state_finishes_it_exactly_once() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let t0 = Instant::now();
        let notification = expiring("n", ms(1000), Osc99CloseReporting::Report);
        let instance = register_at(&mut registry, &a, &notification, &[Osc99Leg::Toast], t0);
        let _ = registry.tick(t0 + Duration::from_millis(1000));

        assert_eq!(replies(registry.on_toast_event(closed(instance))).len(), 1);
        assert!(
            registry.on_toast_event(closed(instance)).is_empty(),
            "a repeated Closed is for a forgotten instance"
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn expiry_closes_a_linux_leg_then_reports_once_when_it_closes() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let t0 = Instant::now();
        let notification = expiring("n", ms(1000), Osc99CloseReporting::Report);
        let instance = register_at(
            &mut registry,
            &a,
            &notification,
            &[Osc99Leg::LinuxSystem],
            t0,
        );

        let effects = registry.tick(t0 + Duration::from_millis(1000));

        assert_eq!(close_targets(&effects), [(Osc99Leg::LinuxSystem, instance)]);
        assert_eq!(
            registry.next_deadline(),
            None,
            "closing: nothing to wake for"
        );
        assert!(registry.tick(t0 + Duration::from_millis(2000)).is_empty());

        let reports = replies(registry.on_system_event(instance, Osc99SystemLegEvent::Closed));
        assert_eq!(reports.len(), 1);
        assert!(is_tracked_close(&reports[0], "n"));
        assert!(registry.instances().is_empty());
    }

    #[test]
    fn expiry_after_a_toast_activation_closes_the_still_open_toast_exactly_once() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let t0 = Instant::now();
        let notification = Notification99Data {
            activation_reporting: Osc99ActivationReporting::Report,
            ..expiring("n", ms(1000), Osc99CloseReporting::Report)
        };
        let instance = register_at(&mut registry, &a, &notification, &[Osc99Leg::Toast], t0);
        let _ = registry.on_toast_event(activated(instance));

        let effects = registry.tick(t0 + Duration::from_millis(1000));

        assert_eq!(close_toast_targets(&effects), [instance]);
        assert!(registry.tick(t0 + Duration::from_millis(2000)).is_empty());
        let reports = replies(registry.on_toast_event(closed(instance)));
        assert_eq!(reports.len(), 1);
        assert!(is_tracked_close(&reports[0], "n"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn expiry_after_a_linux_activation_closes_the_still_open_leg_exactly_once() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let t0 = Instant::now();
        let notification = Notification99Data {
            activation_reporting: Osc99ActivationReporting::Report,
            ..expiring("n", ms(1000), Osc99CloseReporting::Report)
        };
        let instance = register_at(
            &mut registry,
            &a,
            &notification,
            &[Osc99Leg::LinuxSystem],
            t0,
        );
        // The daemon reports the activation but leaves the notification up
        // (a resident notification): the leg is activated yet still open.
        let activation = linux_activated(&mut registry, instance, Osc99LegActivation::Body);
        assert_eq!(replies(activation).len(), 1, "the activation is reported");
        assert_eq!(
            registry.next_deadline(),
            Some(t0 + Duration::from_millis(1000)),
            "the open leg still expires"
        );

        let effects = registry.tick(t0 + Duration::from_millis(1000));

        assert_eq!(close_targets(&effects), [(Osc99Leg::LinuxSystem, instance)]);
        assert!(
            registry.tick(t0 + Duration::from_millis(2000)).is_empty(),
            "never a second close effect"
        );
        assert!(registry.close(a.origin, "n").is_empty());
        assert_eq!(registry.next_deadline(), None);

        let reports = replies(registry.on_system_event(instance, Osc99SystemLegEvent::Closed));
        assert_eq!(reports.len(), 1, "then exactly one close report");
        assert!(is_tracked_close(&reports[0], "n"));
        assert!(registry.instances().is_empty());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn expiry_after_an_activation_does_not_re_close_the_legs_activation_already_closed() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let t0 = Instant::now();
        let instance = register_at(
            &mut registry,
            &a,
            &expiring("n", ms(1000), Osc99CloseReporting::Report),
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem],
            t0,
        );
        // Activating the Linux leg closes the toast; only the activated Linux
        // leg stays open.
        let activation = registry.on_system_event(
            instance,
            Osc99SystemLegEvent::Activated(Osc99LegActivation::Body),
        );
        assert_eq!(close_targets(&activation), [(Osc99Leg::Toast, instance)]);

        let effects = registry.tick(t0 + Duration::from_millis(1000));

        assert_eq!(
            close_targets(&effects),
            [(Osc99Leg::LinuxSystem, instance)],
            "the toast was already asked to close, so only the Linux leg is"
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn expiry_of_a_toast_plus_linux_instance_reports_only_after_both_close() {
        let (a, _b) = two_panes();
        let mut registry = Osc99Registry::default();
        let t0 = Instant::now();
        let notification = expiring("n", ms(1000), Osc99CloseReporting::Report);
        let instance = register_at(
            &mut registry,
            &a,
            &notification,
            &[Osc99Leg::Toast, Osc99Leg::LinuxSystem],
            t0,
        );

        let effects = registry.tick(t0 + Duration::from_millis(1000));
        assert_eq!(
            close_targets(&effects),
            [
                (Osc99Leg::Toast, instance),
                (Osc99Leg::LinuxSystem, instance)
            ]
        );

        assert!(
            registry.on_toast_event(closed(instance)).is_empty(),
            "the Linux leg is still closing"
        );
        let reports = replies(registry.on_system_event(instance, Osc99SystemLegEvent::Closed));
        assert_eq!(reports.len(), 1);
        assert!(is_tracked_close(&reports[0], "n"));
        assert!(registry.instances().is_empty());
    }
}
