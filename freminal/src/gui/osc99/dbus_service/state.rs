// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The dispatcher's pure bookkeeping: which instance the daemon displays as
//! which id, whose signals are believed, the daemon-health breaker and the
//! reconnect gate. Nothing here touches a bus or a thread.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use freminal_common::buffer_states::osc99_notification::{Notification99Data, Osc99Expiry};
use zbus::blocking::Connection;

use crate::gui::osc99::presentation::{
    Osc99DecodedIconCache, Osc99IconLookup, category_hint, escape_body_markup, sound_hint,
};
use crate::gui::osc99::{Osc99DaemonCapabilities, Osc99DaemonSupport, Osc99InstanceId};

use super::pump::PumpSignal;
use super::wire::{NotifyArgs, NotifyHints, NotifyReply, is_method_timeout, urgency_byte};
use super::{Osc99DbusEvent, Osc99SystemAction};

/// How long the dispatcher leaves a daemon alone after one of its calls timed
/// out. Every show fails and every close completes without calling it until
/// this has passed; then the next command tries once more.
const OSC99_DAEMON_COOLDOWN: Duration = Duration::from_secs(30);

/// The `app_name` sent when the notification names no application (`f=`).
const DEFAULT_APP_NAME: &str = "freminal";

/// The most button actions sent: OSC 99 keeps at most eight buttons, and
/// action keys `"1"`-`"8"` are the only ones mapped back.
const MAX_BUTTON_ACTIONS: usize = 8;

impl Osc99DaemonCapabilities {
    /// The capabilities a daemon advertised in its `GetCapabilities` reply.
    fn from_advertised(advertised: &[String]) -> Self {
        let support = |name: &str| {
            if advertised.iter().any(|capability| capability == name) {
                Osc99DaemonSupport::Supported
            } else {
                Osc99DaemonSupport::Unsupported
            }
        };
        Self {
            body: support("body"),
            actions: support("actions"),
            body_markup: support("body-markup"),
            sound: support("sound"),
        }
    }
}

/// What a `Close` command has to do, given the dispatcher's bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CloseTarget {
    /// The daemon displays the instance as this id: call `CloseNotification`.
    CallDaemon(u32),
    /// The instance has no D-Bus id (never shown, show failed, replaced, or
    /// already closed), so there is nothing to call and no signal will come.
    AlreadyGone,
}

/// Whose notification signals the dispatcher believes.
///
/// There is deliberately no `Default`: the one value that believes everyone
/// ([`Self::Unfiltered`]) must be chosen, never fallen into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Osc99SignalSender {
    /// There is no bus (a peer-to-peer connection), so there is no sender to
    /// check: every signal is believed.
    Unfiltered,
    /// Only signals from this unique name are believed.
    Owner(String),
    /// Nobody is known to own `org.freedesktop.Notifications`: no signal is
    /// believed until an owner is learnt from `NameOwnerChanged` or adopted
    /// from a `Notify` reply.
    NoOwner,
}

impl Osc99SignalSender {
    /// Whether a signal from `sender` is believed.
    fn accepts(&self, sender: Option<&str>) -> bool {
        match self {
            Self::Unfiltered => true,
            Self::Owner(owner) => sender == Some(owner.as_str()),
            Self::NoOwner => false,
        }
    }

    /// The owner for a `NameOwnerChanged` / `GetNameOwner` answer.
    fn from_owner(owner: Option<String>) -> Self {
        owner.map_or(Self::NoOwner, Self::Owner)
    }

    /// The unique name of the known owner, if there is one.
    const fn owner_name(&self) -> Option<&str> {
        match self {
            Self::Owner(owner) => Some(owner.as_str()),
            Self::Unfiltered | Self::NoOwner => None,
        }
    }
}

/// How the dispatcher learns who the notification daemon is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OwnerTracking {
    /// A message bus: ask the bus driver (`GetNameOwner`), follow
    /// `NameOwnerChanged`, and believe only the owner's signals.
    Bus,
    /// A peer-to-peer connection (tests): there is no bus driver, so there is
    /// no owner to learn and every signal is believed.
    Peer,
}

impl OwnerTracking {
    /// The mode that fits `connection`.
    pub(super) fn of(connection: &Connection) -> Self {
        if connection.is_bus() {
            Self::Bus
        } else {
            Self::Peer
        }
    }
}

/// Whether the daemon answers its calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Osc99DaemonHealth {
    /// The last call was answered (or none was made yet).
    Healthy,
    /// A call timed out at `since`: the daemon is left alone until
    /// [`OSC99_DAEMON_COOLDOWN`] has passed.
    Unresponsive { since: Instant },
}

/// Whether a call to the daemon may be made now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DaemonGate {
    /// Call the daemon.
    Call,
    /// The daemon is in its cooldown: do not call it, fail the command.
    Skip,
}

/// The daemon's id for one displayed instance, and who displayed it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Mapping {
    /// The daemon's id for the notification.
    dbus_id: u32,
    /// The unique name that answered the `Notify`; `None` when the reply had
    /// no sender (peer-to-peer).
    owner: Option<String>,
}

/// The dispatcher's bookkeeping, free of any bus or thread.
pub(super) struct DispatcherState {
    /// Whose notification signals are believed.
    pub(super) daemon_owner: Osc99SignalSender,
    /// The daemon's id (and answering owner) for each displayed instance.
    by_instance: HashMap<Osc99InstanceId, Mapping>,
    /// The instance behind each of the daemon's ids.
    by_dbus_id: HashMap<u32, Osc99InstanceId>,
    /// What the daemon can do; assumed until it answers.
    pub(super) capabilities: Osc99DaemonCapabilities,
    /// The owner `GetCapabilities` was last attempted against, so a new owner
    /// is known to need a fresh answer.
    capabilities_asked_of: Option<String>,
    /// Whether the daemon answers.
    health: Osc99DaemonHealth,
    /// What the icon and `.desktop` scans know.
    icon_lookup: Box<dyn Osc99IconLookup + Send>,
    /// The icons transmitted recently, already decoded.
    icon_cache: Osc99DecodedIconCache,
}

impl DispatcherState {
    /// A state with no instances, the assumed capabilities, and `icon_lookup`
    /// to resolve icons with.
    pub(super) fn new(
        daemon_owner: Osc99SignalSender,
        icon_lookup: Box<dyn Osc99IconLookup + Send>,
    ) -> Self {
        Self {
            daemon_owner,
            by_instance: HashMap::new(),
            by_dbus_id: HashMap::new(),
            capabilities: Osc99DaemonCapabilities::default(),
            capabilities_asked_of: None,
            health: Osc99DaemonHealth::Healthy,
            icon_lookup,
            icon_cache: Osc99DecodedIconCache::default(),
        }
    }

    /// Start a new connection's session: `daemon_owner` is who owns the name
    /// now. Nothing displayed through an earlier connection is still tracked
    /// (a connection only ends by being lost, which already closed it), a
    /// fresh connection has no history of timeouts, and the capabilities of
    /// an earlier daemon are forgotten: they are assumed again until the
    /// daemon on this connection answers.
    pub(super) fn begin_session(&mut self, daemon_owner: Osc99SignalSender) {
        self.daemon_owner = daemon_owner;
        self.by_instance.clear();
        self.by_dbus_id.clear();
        self.capabilities = Osc99DaemonCapabilities::default();
        self.capabilities_asked_of = None;
        self.health = Osc99DaemonHealth::Healthy;
    }

    /// Whether the daemon may be called at `now`. After a timeout the answer
    /// is [`DaemonGate::Skip`] until [`OSC99_DAEMON_COOLDOWN`] has passed; the
    /// first call after that is a probe, and its outcome (see
    /// [`Self::note_call`]) decides whether the daemon is trusted again.
    pub(super) fn daemon_gate(&self, now: Instant) -> DaemonGate {
        match self.health {
            Osc99DaemonHealth::Healthy => DaemonGate::Call,
            Osc99DaemonHealth::Unresponsive { since }
                if now.saturating_duration_since(since) >= OSC99_DAEMON_COOLDOWN =>
            {
                DaemonGate::Call
            }
            Osc99DaemonHealth::Unresponsive { .. } => DaemonGate::Skip,
        }
    }

    /// Record the outcome of a call to the daemon, made and finished by `now`.
    /// A timeout makes the daemon [`Osc99DaemonHealth::Unresponsive`]; any
    /// other outcome, an error reply included, proves it answers.
    pub(super) fn note_call<T>(&mut self, outcome: &zbus::Result<T>, now: Instant) {
        self.health = match outcome {
            Err(e) if is_method_timeout(e) => Osc99DaemonHealth::Unresponsive { since: now },
            Ok(_) | Err(_) => Osc99DaemonHealth::Healthy,
        };
    }

    /// Whether the owner is one `GetCapabilities` has not been tried against.
    /// A state without a known owner never is: with no owner there is no one
    /// to ask, and a peer-to-peer connection is asked once, at the start.
    pub(super) fn capabilities_stale(&self) -> bool {
        self.daemon_owner
            .owner_name()
            .is_some_and(|owner| self.capabilities_asked_of.as_deref() != Some(owner))
    }

    /// Note that `GetCapabilities` was attempted against the current owner.
    pub(super) fn note_capabilities_asked(&mut self) {
        self.capabilities_asked_of = self.daemon_owner.owner_name().map(str::to_owned);
    }

    /// Record the daemon's `GetCapabilities` answer.
    pub(super) fn set_capabilities(&mut self, advertised: &[String]) {
        self.capabilities = Osc99DaemonCapabilities::from_advertised(advertised);
    }

    /// Build the `Notify` arguments for `notification`.
    ///
    /// `replaces_id` is the replaced instance's D-Bus id, or `0` when there is
    /// none (or it is not known). The `actions` array is the `default` action
    /// followed by one 1-based action per button, and is sent only when the
    /// daemon has the `actions` capability. The icon, sound, category and
    /// body follow `presentation`; the body is escaped only when the daemon
    /// has the `body-markup` capability.
    pub(super) fn notify_args(
        &mut self,
        replaces: Option<Osc99InstanceId>,
        notification: &Notification99Data,
    ) -> NotifyArgs {
        let actions = match self.capabilities.actions {
            Osc99DaemonSupport::Supported => {
                let mut actions = vec!["default".to_owned(), String::new()];
                for (index, label) in notification
                    .buttons
                    .iter()
                    .take(MAX_BUTTON_ACTIONS)
                    .enumerate()
                {
                    actions.push((index + 1).to_string());
                    actions.push(label.clone());
                }
                actions
            }
            Osc99DaemonSupport::Unsupported => Vec::new(),
        };
        let expire_timeout = match notification.expiry {
            Osc99Expiry::OsDefault => -1,
            Osc99Expiry::Never => 0,
            Osc99Expiry::After(ms) => i32::try_from(ms.get()).unwrap_or(i32::MAX),
        };
        let (app_icon, image) = NotifyArgs::icon(
            self.icon_lookup.as_ref(),
            &mut self.icon_cache,
            notification,
        );
        let body = notification.body.clone().unwrap_or_default();
        let body = match self.capabilities.body_markup {
            Osc99DaemonSupport::Supported => escape_body_markup(&body),
            Osc99DaemonSupport::Unsupported => body,
        };
        NotifyArgs {
            app_name: notification
                .app_name
                .clone()
                .unwrap_or_else(|| DEFAULT_APP_NAME.to_owned()),
            replaces_id: replaces.and_then(|old| self.dbus_id_of(old)).unwrap_or(0),
            app_icon,
            summary: notification.title.clone(),
            body,
            actions,
            hints: NotifyHints {
                urgency: urgency_byte(notification.urgency),
                category: category_hint(&notification.notification_types).map(str::to_owned),
                sound: sound_hint(&notification.sound),
                image,
            },
            expire_timeout,
        }
    }

    /// The daemon's id for `instance`, if the daemon displays it.
    pub(super) fn dbus_id_of(&self, instance: Osc99InstanceId) -> Option<u32> {
        self.by_instance
            .get(&instance)
            .map(|mapping| mapping.dbus_id)
    }

    /// Record that the daemon displayed `instance` under `reply`, returning
    /// the other instance this evicted, if any.
    ///
    /// When `replaces` is given the replaced instance's mappings are dropped
    /// first, so the replaced notification's later `NotificationClosed` is
    /// ignored (and a daemon that reuses the id for the update still maps it
    /// to the new instance). A *different* instance still mapped to the same
    /// D-Bus id cannot be displayed any more (a daemon never gives one id to
    /// two live notifications), so its mappings are dropped and it is
    /// returned for the caller to report closed; leaving it would orphan its
    /// leg. An `instance` that was already mapped under another id keeps only
    /// the new one.
    ///
    /// With no known owner (the lookup failed, or nobody owned the name) the
    /// sender of the reply is adopted as the owner: it is whoever the bus
    /// routed the call to, and only the owner of the name can have answered
    /// it.
    pub(super) fn record_shown(
        &mut self,
        instance: Osc99InstanceId,
        replaces: Option<Osc99InstanceId>,
        reply: NotifyReply,
    ) -> Option<Osc99InstanceId> {
        if let Some(old) = replaces {
            self.forget_instance(old);
        }
        self.forget_instance(instance);
        let evicted = self.by_dbus_id.insert(reply.dbus_id, instance);
        if let Some(previous) = evicted {
            self.by_instance.remove(&previous);
        }
        if matches!(self.daemon_owner, Osc99SignalSender::NoOwner)
            && let Some(sender) = &reply.sender
        {
            tracing::debug!("OSC 99: adopting the sender of a Notify reply as the daemon's owner");
            self.daemon_owner = Osc99SignalSender::Owner(sender.clone());
        }
        self.by_instance.insert(
            instance,
            Mapping {
                dbus_id: reply.dbus_id,
                owner: reply.sender,
            },
        );
        evicted
    }

    /// Drop `instance`'s mappings, if it has any.
    pub(super) fn forget_instance(&mut self, instance: Osc99InstanceId) {
        if let Some(mapping) = self.by_instance.remove(&instance) {
            self.by_dbus_id.remove(&mapping.dbus_id);
        }
    }

    /// What closing `instance` calls for: the daemon's id when it displays
    /// the instance, otherwise [`CloseTarget::AlreadyGone`].
    pub(super) fn close_target(&self, instance: Osc99InstanceId) -> CloseTarget {
        self.dbus_id_of(instance)
            .map_or(CloseTarget::AlreadyGone, CloseTarget::CallDaemon)
    }

    /// The event for an `ActionInvoked` signal. The mappings stay: the
    /// service closes the activated notification next, which drops them
    /// (138.C23). `None` for an id with no live
    /// instance or an action freminal never registered.
    fn on_action(&self, dbus_id: u32, action_key: &str) -> Option<Osc99DbusEvent> {
        let instance = *self.by_dbus_id.get(&dbus_id)?;
        let action = Osc99SystemAction::from_key(action_key)?;
        Some(Osc99DbusEvent::Activated { instance, action })
    }

    /// The event for a `NotificationClosed` signal, dropping the mappings.
    /// `None` for an id with no live instance (this covers replaced
    /// notifications).
    fn on_closed(&mut self, dbus_id: u32) -> Option<Osc99DbusEvent> {
        let instance = self.by_dbus_id.remove(&dbus_id)?;
        self.by_instance.remove(&instance);
        Some(Osc99DbusEvent::Closed { instance })
    }

    /// The events, if any, a pump signal stands for.
    ///
    /// A notification signal from a sender that is not the daemon's owner is
    /// ignored (logged at debug level, without its payload).
    pub(super) fn on_signal(&mut self, signal: &PumpSignal) -> Vec<Osc99DbusEvent> {
        match signal {
            PumpSignal::ActionInvoked {
                sender,
                dbus_id,
                action_key,
            } => {
                if self.believes(sender.as_deref(), "ActionInvoked") {
                    self.on_action(*dbus_id, action_key).into_iter().collect()
                } else {
                    Vec::new()
                }
            }
            PumpSignal::NotificationClosed { sender, dbus_id } => {
                if self.believes(sender.as_deref(), "NotificationClosed") {
                    self.on_closed(*dbus_id).into_iter().collect()
                } else {
                    Vec::new()
                }
            }
            PumpSignal::OwnerChanged { new_owner } => self.on_owner_changed(new_owner.clone()),
            PumpSignal::Ended(kind) => {
                tracing::warn!("OSC 99: the {kind:?} signal pump ended; no more signals");
                self.on_signals_lost()
            }
        }
    }

    /// Whether a `member` signal from `sender` comes from the daemon's owner.
    fn believes(&self, sender: Option<&str>, member: &str) -> bool {
        let believed = self.daemon_owner.accepts(sender);
        if !believed {
            tracing::debug!(
                "OSC 99: ignoring a {member} signal from a sender that is not the daemon"
            );
        }
        believed
    }

    /// Record who owns `org.freedesktop.Notifications` now.
    ///
    /// The same owner changes nothing. Otherwise every instance recorded
    /// under a *different* owner is reported [`Osc99DbusEvent::Closed`] and
    /// forgotten: the daemon that displayed it is gone and will never send
    /// the signals that would end its leg. This is freminal's choice, so the
    /// registry can finish those legs and send the owed close reports instead
    /// of leaving them open forever. An instance recorded under the new owner
    /// stays (a `Notify` that auto-started the daemon can be recorded before
    /// the change is processed), and so does one with no recorded owner,
    /// which is a peer-to-peer instance that owner changes do not concern.
    fn on_owner_changed(&mut self, new_owner: Option<String>) -> Vec<Osc99DbusEvent> {
        let new = Osc99SignalSender::from_owner(new_owner);
        if self.daemon_owner == new {
            return Vec::new();
        }
        self.daemon_owner = new;
        let current = self.daemon_owner.owner_name();
        let mut gone: Vec<Osc99InstanceId> = self
            .by_instance
            .iter()
            .filter(|(_, mapping)| {
                mapping
                    .owner
                    .as_deref()
                    .is_some_and(|owner| Some(owner) != current)
            })
            .map(|(instance, _)| *instance)
            .collect();
        gone.sort_by_key(|instance| instance.0);
        gone.into_iter()
            .map(|instance| {
                self.forget_instance(instance);
                Osc99DbusEvent::Closed { instance }
            })
            .collect()
    }

    /// The signals will not arrive any more (a pump ended), so nothing
    /// displayed can be tracked: every mapped instance is reported
    /// [`Osc99DbusEvent::Closed`], the maps are cleared, and the service
    /// reports [`Osc99DbusEvent::Unavailable`] last.
    pub(super) fn on_signals_lost(&mut self) -> Vec<Osc99DbusEvent> {
        self.by_dbus_id.clear();
        let mut instances: Vec<Osc99InstanceId> =
            self.by_instance.drain().map(|(i, _)| i).collect();
        instances.sort_by_key(|instance| instance.0);
        instances
            .into_iter()
            .map(|instance| Osc99DbusEvent::Closed { instance })
            .chain(std::iter::once(Osc99DbusEvent::Unavailable))
            .collect()
    }
}

#[cfg(test)]
impl Default for DispatcherState {
    fn default() -> Self {
        Self::new(
            Osc99SignalSender::Unfiltered,
            Box::new(super::test_rig::NoIconLookup),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;
    use std::time::{Duration, Instant};

    use freminal_common::buffer_states::osc99_notification::{
        Notification99Data, NotificationUrgency, Osc99Sound,
    };

    use super::super::pump::PumpKind;
    use super::super::test_rig::*;
    use super::super::wire::NotifyImage;
    use super::*;
    use crate::gui::osc99::presentation::Osc99SoundHint;

    // ── DispatcherState ─────────────────────────────────────────────────

    #[test]
    fn notify_args_carry_title_body_app_name_and_urgency() {
        let mut state = DispatcherState::default();
        let args = state.notify_args(
            None,
            &Notification99Data {
                body: Some("Body".to_owned()),
                app_name: Some("my-app".to_owned()),
                urgency: NotificationUrgency::Critical,
                ..notification()
            },
        );

        assert_eq!(args.app_name, "my-app");
        assert_eq!(args.summary, "Title");
        assert_eq!(args.body, "Body");
        // `f=` matched no icon, so the last rule applies: freminal's own.
        assert_eq!(args.app_icon, "freminal");
        assert_eq!(
            args.hints,
            NotifyHints {
                urgency: 2,
                category: None,
                sound: Osc99SoundHint::None,
                image: NotifyImage::None,
            }
        );
    }

    // ── Presentation: icon, sound, category, markup ─────────────────────

    /// An icon lookup over fixed sets, touching no filesystem.
    struct FakeLookup {
        /// The name asked for to the on-disk stem that matches it.
        icons: HashMap<String, String>,
        desktop: HashMap<String, String>,
    }

    impl Osc99IconLookup for FakeLookup {
        fn matched_icon(&self, name: &str) -> Option<String> {
            self.icons.get(name).cloned()
        }

        fn desktop_icon(&self, app_name: &str) -> Option<String> {
            self.desktop.get(app_name).cloned()
        }
    }

    fn state_with_icons(icons: &[&str], desktop: &[(&str, &str)]) -> DispatcherState {
        DispatcherState::new(
            Osc99SignalSender::Unfiltered,
            Box::new(FakeLookup {
                icons: icons
                    .iter()
                    .map(|icon| ((*icon).to_owned(), (*icon).to_owned()))
                    .collect(),
                desktop: desktop
                    .iter()
                    .map(|(app, icon)| ((*app).to_owned(), (*icon).to_owned()))
                    .collect(),
            }),
        )
    }

    #[test]
    fn notify_args_escape_the_body_only_with_body_markup() {
        let body = Notification99Data {
            body: Some("a<b>&c".to_owned()),
            ..notification()
        };

        let mut state = DispatcherState::default();
        state.set_capabilities(&strings(&["body", "body-markup"]));
        assert_eq!(
            state.notify_args(None, &body).body,
            "a<\u{200c}b>&\u{200c}c"
        );

        state.set_capabilities(&strings(&["body"]));
        assert_eq!(state.notify_args(None, &body).body, "a<b>&c");
    }

    #[test]
    fn notify_args_assume_body_markup_until_the_daemon_answers() {
        let args = DispatcherState::default().notify_args(
            None,
            &Notification99Data {
                body: Some("<".to_owned()),
                ..notification()
            },
        );

        assert_eq!(args.body, "<\u{200c}");
    }

    #[test]
    fn notify_args_hint_the_first_non_empty_category() {
        let args = DispatcherState::default().notify_args(
            None,
            &Notification99Data {
                notification_types: strings(&["", "email.arrived", "im"]),
                ..notification()
            },
        );

        assert_eq!(args.hints.category.as_deref(), Some("email.arrived"));
    }

    #[test]
    fn notify_args_map_sounds_to_hints() {
        let mut state = DispatcherState::default();
        for (sound, expected) in [
            (Osc99Sound::System, Osc99SoundHint::None),
            (Osc99Sound::Silent, Osc99SoundHint::Suppress),
            (
                Osc99Sound::Named("warn".to_owned()),
                Osc99SoundHint::Named("dialog-warning".to_owned()),
            ),
            (
                Osc99Sound::Named("bell".to_owned()),
                Osc99SoundHint::Named("bell".to_owned()),
            ),
        ] {
            let args = state.notify_args(
                None,
                &Notification99Data {
                    sound: sound.clone(),
                    ..notification()
                },
            );
            assert_eq!(args.hints.sound, expected, "{sound:?}");
        }
    }

    #[test]
    fn notify_args_use_an_existing_icon_name_over_data() -> TestResult {
        let mut state = state_with_icons(&["mail-unread"], &[]);
        let args = state.notify_args(
            None,
            &Notification99Data {
                icon_names: strings(&["mail-unread"]),
                icon_data: Some(png(2, 2)?),
                ..notification()
            },
        );

        assert_eq!(args.app_icon, "mail-unread");
        assert_eq!(
            args.hints.image,
            NotifyImage::Path("mail-unread".to_owned())
        );
        Ok(())
    }

    #[test]
    fn notify_args_send_decoded_data_as_image_data() -> TestResult {
        let args = DispatcherState::default().notify_args(
            None,
            &Notification99Data {
                icon_names: strings(&["unknown"]),
                icon_data: Some(png(3, 2)?),
                ..notification()
            },
        );

        assert_eq!(args.app_icon, "");
        let NotifyImage::Data(image) = args.hints.image else {
            return Err("no image hint".into());
        };
        assert_eq!((image.width, image.height, image.rowstride), (3, 2, 12));
        assert_eq!(image.rgba.len(), 3 * 2 * 4);
        Ok(())
    }

    #[test]
    fn notify_args_downscale_a_large_image() -> TestResult {
        let args = DispatcherState::default().notify_args(
            None,
            &Notification99Data {
                icon_data: Some(png(2000, 1000)?),
                ..notification()
            },
        );

        let NotifyImage::Data(image) = args.hints.image else {
            return Err("no image hint".into());
        };
        assert_eq!(
            (image.width, image.height, image.rowstride),
            (256, 128, 1024)
        );
        assert_eq!(image.rgba.len(), 256 * 128 * 4);
        Ok(())
    }

    #[test]
    fn notify_args_fall_back_to_the_first_name_when_the_data_does_not_decode() {
        let args = DispatcherState::default().notify_args(
            None,
            &Notification99Data {
                icon_names: strings(&["unknown", "other"]),
                icon_data: Some(b"not an image".to_vec()),
                ..notification()
            },
        );

        assert_eq!(args.app_icon, "unknown");
        assert_eq!(args.hints.image, NotifyImage::Path("unknown".to_owned()));
    }

    #[test]
    fn notify_args_send_the_matched_on_disk_stem() {
        let mut state = DispatcherState::new(
            Osc99SignalSender::Unfiltered,
            Box::new(FakeLookup {
                icons: HashMap::from([("mailicon".to_owned(), "MailIcon".to_owned())]),
                desktop: HashMap::new(),
            }),
        );
        let args = state.notify_args(
            None,
            &Notification99Data {
                icon_names: strings(&["mailicon"]),
                ..notification()
            },
        );

        assert_eq!(args.app_icon, "MailIcon");
        assert_eq!(args.hints.image, NotifyImage::Path("MailIcon".to_owned()));
    }

    #[test]
    fn notify_args_use_the_desktop_icon_when_the_data_does_not_decode() {
        let mut state = state_with_icons(&[], &[("my-app", "my-app-icon")]);
        let args = state.notify_args(
            None,
            &Notification99Data {
                app_name: Some("my-app".to_owned()),
                icon_data: Some(b"not an image".to_vec()),
                ..notification()
            },
        );

        assert_eq!(args.app_icon, "my-app-icon");
        assert_eq!(args.hints.image, NotifyImage::None);
    }

    #[test]
    fn notify_args_prefer_a_name_to_the_desktop_icon_when_the_data_does_not_decode() {
        let mut state = state_with_icons(&[], &[("my-app", "my-app-icon")]);
        let args = state.notify_args(
            None,
            &Notification99Data {
                app_name: Some("my-app".to_owned()),
                icon_names: strings(&["unknown"]),
                icon_data: Some(b"not an image".to_vec()),
                ..notification()
            },
        );

        assert_eq!(args.app_icon, "unknown");
    }

    #[test]
    fn notify_args_decode_the_same_icon_the_same_way_every_time() -> TestResult {
        let mut state = DispatcherState::default();
        let with_icon = Notification99Data {
            icon_data: Some(png(3, 2)?),
            ..notification()
        };

        let first = state.notify_args(None, &with_icon).hints.image;
        let second = state.notify_args(None, &with_icon).hints.image;

        assert!(matches!(first, NotifyImage::Data(_)));
        assert_eq!(first, second);
        Ok(())
    }

    #[test]
    fn notify_args_without_names_and_with_undecodable_data_use_the_freminal_icon() {
        let args = DispatcherState::default().notify_args(
            None,
            &Notification99Data {
                app_name: Some("my-app".to_owned()),
                icon_data: Some(b"not an image".to_vec()),
                ..notification()
            },
        );

        assert_eq!(args.app_icon, "freminal");
        assert_eq!(args.hints.image, NotifyImage::None);
    }

    #[test]
    fn notify_args_take_the_icon_of_the_desktop_file_matching_the_app() {
        let mut state = state_with_icons(&[], &[("my-app", "my-app-icon")]);
        let args = state.notify_args(
            None,
            &Notification99Data {
                app_name: Some("my-app".to_owned()),
                ..notification()
            },
        );

        assert_eq!(args.app_icon, "my-app-icon");
        assert_eq!(args.hints.image, NotifyImage::None);
    }

    #[test]
    fn notify_args_send_a_standard_name_as_app_icon_and_image_path() {
        let args = DispatcherState::default().notify_args(
            None,
            &Notification99Data {
                icon_names: strings(&["error"]),
                ..notification()
            },
        );

        assert_eq!(args.app_icon, "dialog-error");
        assert_eq!(
            args.hints.image,
            NotifyImage::Path("dialog-error".to_owned())
        );
    }

    #[test]
    fn notify_args_without_names_send_no_image_hint() {
        let args = DispatcherState::default().notify_args(None, &notification());

        assert_eq!(args.hints.image, NotifyImage::None);
    }

    #[test]
    fn notify_args_default_to_the_freminal_icon_without_an_app_name() {
        let args = DispatcherState::default().notify_args(None, &notification());

        assert_eq!(args.app_icon, "freminal");
    }

    #[test]
    fn notify_args_use_the_freminal_icon_when_the_app_matches_no_desktop_file() {
        let args = DispatcherState::default().notify_args(
            None,
            &Notification99Data {
                app_name: Some("unknown-app".to_owned()),
                ..notification()
            },
        );

        assert_eq!(args.app_icon, "freminal");
    }

    #[test]
    fn notify_args_default_the_app_name_and_body() {
        let args = DispatcherState::default().notify_args(None, &notification());

        assert_eq!(args.app_name, "freminal");
        assert_eq!(args.body, "");
    }

    #[test]
    fn notify_args_map_each_urgency_to_its_byte() {
        let mut state = DispatcherState::default();
        for (urgency, byte) in [
            (NotificationUrgency::Low, 0),
            (NotificationUrgency::Normal, 1),
            (NotificationUrgency::Critical, 2),
        ] {
            let args = state.notify_args(
                None,
                &Notification99Data {
                    urgency,
                    ..notification()
                },
            );
            assert_eq!(args.hints.urgency, byte, "{urgency:?}");
        }
    }

    #[test]
    fn notify_args_send_the_default_action_and_one_based_buttons() {
        let args = DispatcherState::default().notify_args(None, &with_buttons(&["A", "B"]));

        assert_eq!(args.actions, ["default", "", "1", "A", "2", "B"]);
    }

    #[test]
    fn notify_args_send_only_the_default_action_without_buttons() {
        let args = DispatcherState::default().notify_args(None, &notification());

        assert_eq!(args.actions, ["default", ""]);
    }

    #[test]
    fn notify_args_send_no_actions_when_the_daemon_lacks_the_capability() {
        let mut state = DispatcherState::default();
        state.set_capabilities(&strings(&["body"]));

        let args = state.notify_args(None, &with_buttons(&["A", "B"]));

        assert_eq!(args.actions, Vec::<String>::new());
    }

    #[test]
    fn notify_args_keep_at_most_eight_buttons() {
        let labels: Vec<String> = (0..12).map(|n| format!("b{n}")).collect();
        let args = DispatcherState::default().notify_args(
            None,
            &Notification99Data {
                buttons: labels,
                ..notification()
            },
        );

        assert_eq!(args.actions.len(), 2 + 8 * 2);
        assert_eq!(args.actions[args.actions.len() - 2], "8");
    }

    #[test]
    fn notify_args_map_expiry_to_the_timeout() -> TestResult {
        let mut state = DispatcherState::default();
        let mut timeout = |expiry| {
            state
                .notify_args(
                    None,
                    &Notification99Data {
                        expiry,
                        ..notification()
                    },
                )
                .expire_timeout
        };

        assert_eq!(timeout(Osc99Expiry::OsDefault), -1);
        assert_eq!(timeout(Osc99Expiry::Never), 0);
        assert_eq!(
            timeout(Osc99Expiry::After(NonZeroU32::new(5000).ok_or("zero")?)),
            5000
        );
        assert_eq!(
            timeout(Osc99Expiry::After(NonZeroU32::MAX)),
            i32::MAX,
            "a duration beyond i32 clamps"
        );
        Ok(())
    }

    #[test]
    fn notify_args_replace_id_is_the_replaced_instances_dbus_id() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(42));

        assert_eq!(
            state
                .notify_args(Some(instance(1)), &notification())
                .replaces_id,
            42
        );
        assert_eq!(
            state.notify_args(None, &notification()).replaces_id,
            0,
            "no replacement"
        );
        assert_eq!(
            state
                .notify_args(Some(instance(9)), &notification())
                .replaces_id,
            0,
            "an instance with no D-Bus id"
        );
    }

    #[test]
    fn capabilities_default_to_what_kitty_assumes() {
        let state = DispatcherState::default();

        assert_eq!(
            state.capabilities,
            Osc99DaemonCapabilities {
                body: Osc99DaemonSupport::Supported,
                actions: Osc99DaemonSupport::Supported,
                body_markup: Osc99DaemonSupport::Supported,
                sound: Osc99DaemonSupport::Unsupported,
            }
        );
    }

    #[test]
    fn capabilities_follow_the_advertised_list() {
        let mut state = DispatcherState::default();

        state.set_capabilities(&strings(&["body", "sound", "persistence"]));

        assert_eq!(
            state.capabilities,
            Osc99DaemonCapabilities {
                body: Osc99DaemonSupport::Supported,
                actions: Osc99DaemonSupport::Unsupported,
                body_markup: Osc99DaemonSupport::Unsupported,
                sound: Osc99DaemonSupport::Supported,
            }
        );
    }

    #[test]
    fn an_action_for_an_unknown_id_is_ignored() {
        let state = DispatcherState::default();

        assert_eq!(state.on_action(7, "default"), None);
    }

    #[test]
    fn the_default_action_activates_the_instance() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));

        assert_eq!(
            state.on_action(7, "default"),
            Some(Osc99DbusEvent::Activated {
                instance: instance(1),
                action: Osc99SystemAction::Default,
            })
        );
    }

    #[test]
    fn button_keys_one_to_eight_map_to_their_button() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));

        for n in 1..=8u8 {
            assert_eq!(
                state.on_action(7, &n.to_string()),
                Some(Osc99DbusEvent::Activated {
                    instance: instance(1),
                    action: Osc99SystemAction::Button(n),
                })
            );
        }
    }

    #[test]
    fn other_action_keys_are_ignored() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));

        for key in ["0", "9", "x", "", "10", "01", "+1", "Default", "default "] {
            assert_eq!(state.on_action(7, key), None, "key {key:?}");
        }
    }

    #[test]
    fn a_close_after_an_activation_maps_both() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));

        assert!(matches!(
            state.on_action(7, "2"),
            Some(Osc99DbusEvent::Activated { .. })
        ));
        assert_eq!(
            state.on_closed(7),
            Some(Osc99DbusEvent::Closed {
                instance: instance(1)
            })
        );
        assert_eq!(state.on_closed(7), None, "the close dropped the mapping");
        assert_eq!(state.on_action(7, "default"), None);
    }

    #[test]
    fn a_close_for_an_unknown_id_is_ignored() {
        let mut state = DispatcherState::default();

        assert_eq!(state.on_closed(7), None);
    }

    #[test]
    fn replacing_drops_the_old_mapping_so_its_close_is_ignored() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));

        state.record_shown(instance(2), Some(instance(1)), peer_reply(8));

        assert_eq!(state.on_closed(7), None, "the replaced id is forgotten");
        assert_eq!(
            state.on_closed(8),
            Some(Osc99DbusEvent::Closed {
                instance: instance(2)
            })
        );
    }

    #[test]
    fn replacing_with_a_reused_dbus_id_maps_it_to_the_new_instance() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));

        state.record_shown(instance(2), Some(instance(1)), peer_reply(7));

        assert_eq!(
            state.on_closed(7),
            Some(Osc99DbusEvent::Closed {
                instance: instance(2)
            })
        );
        assert!(state.by_instance.is_empty());
        assert!(state.by_dbus_id.is_empty());
    }

    #[test]
    fn signals_map_through_on_signal() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));

        assert_eq!(
            state.on_signal(&action_from(None, 7, "1")),
            [Osc99DbusEvent::Activated {
                instance: instance(1),
                action: Osc99SystemAction::Button(1),
            }]
        );
        assert_eq!(
            state.on_signal(&closed_from(None, 7)),
            [Osc99DbusEvent::Closed {
                instance: instance(1)
            }]
        );
    }

    // ── Sender filtering (138.C2) ───────────────────────────────────────

    /// A state that believes `owner`, with instances 1 and 2 shown as ids 7
    /// and 8 by that owner (or with no recorded owner, when there is none).
    fn owned_by(owner: Osc99SignalSender) -> DispatcherState {
        let answered_by = owner.owner_name().map(str::to_owned);
        let reply = |dbus_id| NotifyReply {
            dbus_id,
            sender: answered_by.clone(),
        };
        let mut state = DispatcherState {
            daemon_owner: owner,
            ..DispatcherState::default()
        };
        state.record_shown(instance(1), None, reply(7));
        state.record_shown(instance(2), None, reply(8));
        state
    }

    #[test]
    fn a_state_believes_everyone_until_it_learns_an_owner() {
        assert_eq!(
            DispatcherState::default().daemon_owner,
            Osc99SignalSender::Unfiltered
        );
    }

    #[test]
    fn an_owned_state_ignores_signals_from_other_senders() {
        let mut state = owned_by(Osc99SignalSender::Owner(":1.5".to_owned()));

        for sender in [Some(":1.9"), None, Some(""), Some(":1.55")] {
            assert_eq!(
                state.on_signal(&action_from(sender, 7, "default")),
                [],
                "action from {sender:?}"
            );
            assert_eq!(
                state.on_signal(&closed_from(sender, 7)),
                [],
                "close from {sender:?}"
            );
        }
        assert_eq!(
            state.close_target(instance(1)),
            CloseTarget::CallDaemon(7),
            "a forged close did not drop the mapping"
        );
    }

    #[test]
    fn an_owned_state_believes_the_owner() {
        let mut state = owned_by(Osc99SignalSender::Owner(":1.5".to_owned()));

        assert_eq!(
            state.on_signal(&action_from(Some(":1.5"), 7, "default")),
            [Osc99DbusEvent::Activated {
                instance: instance(1),
                action: Osc99SystemAction::Default,
            }]
        );
        assert_eq!(
            state.on_signal(&closed_from(Some(":1.5"), 7)),
            [Osc99DbusEvent::Closed {
                instance: instance(1)
            }]
        );
    }

    #[test]
    fn a_state_with_no_owner_ignores_every_signal() {
        let mut state = owned_by(Osc99SignalSender::NoOwner);

        for sender in [Some(":1.5"), None] {
            assert_eq!(state.on_signal(&action_from(sender, 7, "default")), []);
            assert_eq!(state.on_signal(&closed_from(sender, 7)), []);
        }
    }

    #[test]
    fn an_unfiltered_state_believes_every_sender() {
        let mut state = owned_by(Osc99SignalSender::Unfiltered);

        assert_eq!(state.on_signal(&action_from(Some(":1.9"), 7, "1")).len(), 1);
        assert_eq!(state.on_signal(&action_from(None, 7, "1")).len(), 1);
        assert_eq!(state.on_signal(&closed_from(Some(":1.9"), 7)).len(), 1);
        assert_eq!(state.on_signal(&closed_from(None, 8)).len(), 1);
    }

    #[test]
    fn a_new_owner_retargets_the_filter_and_closes_the_old_daemons_instances() {
        let mut state = owned_by(Osc99SignalSender::Owner(":1.5".to_owned()));

        assert_eq!(
            state.on_signal(&owner_changed(Some(":1.6"))),
            [
                Osc99DbusEvent::Closed {
                    instance: instance(1)
                },
                Osc99DbusEvent::Closed {
                    instance: instance(2)
                },
            ]
        );

        assert_eq!(
            state.daemon_owner,
            Osc99SignalSender::Owner(":1.6".to_owned())
        );
        assert!(state.by_instance.is_empty());
        assert!(state.by_dbus_id.is_empty());
        // The new owner is believed, the old one no longer is.
        state.record_shown(instance(3), None, peer_reply(1));
        assert_eq!(state.on_signal(&closed_from(Some(":1.5"), 1)), []);
        assert_eq!(
            state.on_signal(&closed_from(Some(":1.6"), 1)),
            [Osc99DbusEvent::Closed {
                instance: instance(3)
            }]
        );
    }

    #[test]
    fn losing_the_owner_closes_the_old_daemons_instances_and_ignores_signals() {
        let mut state = owned_by(Osc99SignalSender::Owner(":1.5".to_owned()));

        assert_eq!(state.on_signal(&owner_changed(None)).len(), 2);

        assert_eq!(state.daemon_owner, Osc99SignalSender::NoOwner);
        assert!(state.by_instance.is_empty());
        assert!(state.by_dbus_id.is_empty());
        assert_eq!(state.on_signal(&closed_from(Some(":1.5"), 7)), []);
    }

    #[test]
    fn the_same_owner_changes_nothing() {
        let mut state = owned_by(Osc99SignalSender::Owner(":1.5".to_owned()));

        assert_eq!(state.on_signal(&owner_changed(Some(":1.5"))), []);

        assert_eq!(
            state.daemon_owner,
            Osc99SignalSender::Owner(":1.5".to_owned())
        );
        assert_eq!(state.by_instance.len(), 2);
        assert_eq!(state.by_dbus_id.len(), 2);
    }

    #[test]
    fn gaining_an_owner_from_none_keeps_what_was_recorded() {
        // A `Notify` that auto-started the daemon can be recorded before the
        // `NameOwnerChanged` that names the daemon is processed.
        let mut state = owned_by(Osc99SignalSender::NoOwner);

        assert_eq!(state.on_signal(&owner_changed(Some(":1.5"))), []);

        assert_eq!(
            state.daemon_owner,
            Osc99SignalSender::Owner(":1.5".to_owned())
        );
        assert_eq!(state.by_instance.len(), 2);
        assert_eq!(
            state.on_signal(&closed_from(Some(":1.5"), 7)),
            [Osc99DbusEvent::Closed {
                instance: instance(1)
            }]
        );
    }

    #[test]
    fn losing_the_owner_then_gaining_one_again_ends_up_following_the_new_one() {
        let mut state = owned_by(Osc99SignalSender::Owner(":1.5".to_owned()));

        let _ = state.on_signal(&owner_changed(None));

        assert_eq!(state.on_signal(&owner_changed(Some(":1.8"))), []);
        assert_eq!(
            state.daemon_owner,
            Osc99SignalSender::Owner(":1.8".to_owned())
        );
    }

    #[test]
    fn close_target_of_a_shown_instance_is_the_daemons_id() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));

        assert_eq!(state.close_target(instance(1)), CloseTarget::CallDaemon(7));
    }

    #[test]
    fn close_target_of_a_never_shown_instance_is_already_gone() {
        let state = DispatcherState::default();

        assert_eq!(state.close_target(instance(1)), CloseTarget::AlreadyGone);
    }

    #[test]
    fn close_target_after_a_failed_show_is_already_gone() {
        // A failed show records nothing; a failed replacing show also drops
        // the replaced instance's mappings (see `show_notification`).
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));
        state.forget_instance(instance(1));

        assert_eq!(state.close_target(instance(1)), CloseTarget::AlreadyGone);
        assert_eq!(
            state.close_target(instance(2)),
            CloseTarget::AlreadyGone,
            "the failed instance itself was never recorded"
        );
    }

    #[test]
    fn close_target_of_a_replaced_instance_is_already_gone() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));

        state.record_shown(instance(2), Some(instance(1)), peer_reply(8));

        assert_eq!(state.close_target(instance(1)), CloseTarget::AlreadyGone);
        assert_eq!(state.close_target(instance(2)), CloseTarget::CallDaemon(8));
    }

    #[test]
    fn close_target_after_the_daemon_closed_the_instance_is_already_gone() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));
        let _ = state.on_closed(7);

        assert_eq!(state.close_target(instance(1)), CloseTarget::AlreadyGone);
    }

    #[test]
    fn close_target_follows_the_instance_after_an_activation() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));
        let _ = state.on_action(7, "default");

        assert_eq!(
            state.close_target(instance(1)),
            CloseTarget::CallDaemon(7),
            "an activation does not close the notification"
        );
    }

    // ── Per-mapping owners (Z1), adoption (Z2), reused ids (Z12) ────────

    #[test]
    fn only_instances_answered_by_a_different_owner_are_closed_on_an_owner_change() {
        let mut state = DispatcherState::new(
            Osc99SignalSender::Owner(":1.5".to_owned()),
            Box::new(NoIconLookup),
        );
        state.record_shown(instance(1), None, reply_from(7, ":1.5"));
        // Answered by the new daemon before its `NameOwnerChanged` is handled.
        state.record_shown(instance(2), None, reply_from(8, ":1.6"));
        state.record_shown(instance(3), None, reply_from(9, ":1.5"));

        assert_eq!(
            state.on_signal(&owner_changed(Some(":1.6"))),
            [
                Osc99DbusEvent::Closed {
                    instance: instance(1)
                },
                Osc99DbusEvent::Closed {
                    instance: instance(3)
                },
            ]
        );

        assert_eq!(state.dbus_id_of(instance(2)), Some(8));
        assert_eq!(state.dbus_id_of(instance(1)), None);
        assert_eq!(state.dbus_id_of(instance(3)), None);
        assert_eq!(
            state.on_signal(&closed_from(Some(":1.6"), 8)),
            [Osc99DbusEvent::Closed {
                instance: instance(2)
            }]
        );
    }

    #[test]
    fn an_instance_with_no_recorded_owner_is_never_closed_by_an_owner_change() {
        let mut state = DispatcherState::new(
            Osc99SignalSender::Owner(":1.5".to_owned()),
            Box::new(NoIconLookup),
        );
        state.record_shown(instance(1), None, peer_reply(7));

        assert_eq!(state.on_signal(&owner_changed(Some(":1.6"))), []);
        assert_eq!(state.on_signal(&owner_changed(None)), []);

        assert_eq!(state.dbus_id_of(instance(1)), Some(7));
    }

    #[test]
    fn losing_the_owner_closes_every_instance_that_had_one() {
        let mut state = DispatcherState::new(
            Osc99SignalSender::Owner(":1.5".to_owned()),
            Box::new(NoIconLookup),
        );
        state.record_shown(instance(2), None, reply_from(8, ":1.5"));
        state.record_shown(instance(1), None, reply_from(7, ":1.5"));

        assert_eq!(
            state.on_signal(&owner_changed(None)),
            [
                Osc99DbusEvent::Closed {
                    instance: instance(1)
                },
                Osc99DbusEvent::Closed {
                    instance: instance(2)
                },
            ],
            "in instance order"
        );
        assert_eq!(state.daemon_owner, Osc99SignalSender::NoOwner);
    }

    #[test]
    fn a_notify_reply_names_the_owner_when_none_is_known() {
        let mut state = DispatcherState::new(Osc99SignalSender::NoOwner, Box::new(NoIconLookup));

        state.record_shown(instance(1), None, reply_from(7, ":1.7"));

        assert_eq!(
            state.daemon_owner,
            Osc99SignalSender::Owner(":1.7".to_owned())
        );
        assert_eq!(
            state.on_signal(&closed_from(Some(":1.7"), 7)),
            [Osc99DbusEvent::Closed {
                instance: instance(1)
            }]
        );
    }

    #[test]
    fn a_notify_reply_does_not_replace_a_known_owner_or_a_missing_sender() {
        let mut owned = DispatcherState::new(
            Osc99SignalSender::Owner(":1.5".to_owned()),
            Box::new(NoIconLookup),
        );
        owned.record_shown(instance(1), None, reply_from(7, ":1.6"));
        assert_eq!(
            owned.daemon_owner,
            Osc99SignalSender::Owner(":1.5".to_owned()),
            "only a bus-driver answer or NameOwnerChanged moves a known owner"
        );

        let mut unfiltered = DispatcherState::default();
        unfiltered.record_shown(instance(1), None, reply_from(7, ":1.6"));
        assert_eq!(unfiltered.daemon_owner, Osc99SignalSender::Unfiltered);

        let mut senderless =
            DispatcherState::new(Osc99SignalSender::NoOwner, Box::new(NoIconLookup));
        senderless.record_shown(instance(1), None, peer_reply(7));
        assert_eq!(senderless.daemon_owner, Osc99SignalSender::NoOwner);
    }

    #[test]
    fn an_instance_adopted_with_its_owner_survives_the_owner_change_that_names_it() {
        // The adoption and the bus driver's later `NameOwnerChanged` agree.
        let mut state = DispatcherState::new(Osc99SignalSender::NoOwner, Box::new(NoIconLookup));
        state.record_shown(instance(1), None, reply_from(7, ":1.7"));

        assert_eq!(state.on_signal(&owner_changed(Some(":1.7"))), []);

        assert_eq!(state.dbus_id_of(instance(1)), Some(7));
    }

    #[test]
    fn a_reused_dbus_id_evicts_the_instance_that_held_it() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));

        let evicted = state.record_shown(instance(2), None, peer_reply(7));

        assert_eq!(evicted, Some(instance(1)));
        assert_eq!(state.dbus_id_of(instance(1)), None);
        assert_eq!(state.dbus_id_of(instance(2)), Some(7));
        assert_eq!(state.by_instance.len(), 1);
        assert_eq!(state.by_dbus_id.len(), 1);
        assert_eq!(
            state.on_closed(7),
            Some(Osc99DbusEvent::Closed {
                instance: instance(2)
            })
        );
    }

    #[test]
    fn showing_an_instance_again_under_a_new_id_leaves_one_mapping() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));

        assert_eq!(
            state.record_shown(instance(1), None, peer_reply(8)),
            None,
            "nobody else held the new id"
        );

        assert_eq!(state.dbus_id_of(instance(1)), Some(8));
        assert_eq!(state.by_dbus_id.len(), 1);
        assert_eq!(state.on_closed(7), None);
    }

    #[test]
    fn a_distinct_dbus_id_evicts_nobody() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, peer_reply(7));

        assert_eq!(state.record_shown(instance(2), None, peer_reply(8)), None);
        assert_eq!(state.by_instance.len(), 2);
    }

    // ── Lost pumps (Z4) ─────────────────────────────────────────────────

    #[test]
    fn an_ended_pump_closes_every_instance_then_reports_unavailable() {
        for kind in [PumpKind::Notifications, PumpKind::OwnerChanges] {
            let mut state = owned_by(Osc99SignalSender::Owner(":1.5".to_owned()));

            assert_eq!(
                state.on_signal(&PumpSignal::Ended(kind)),
                [
                    Osc99DbusEvent::Closed {
                        instance: instance(1)
                    },
                    Osc99DbusEvent::Closed {
                        instance: instance(2)
                    },
                    Osc99DbusEvent::Unavailable,
                ],
                "{kind:?}"
            );
            assert!(state.by_instance.is_empty());
            assert!(state.by_dbus_id.is_empty());
        }
    }

    #[test]
    fn an_ended_pump_with_nothing_shown_only_reports_unavailable() {
        let mut state = DispatcherState::default();

        assert_eq!(
            state.on_signal(&PumpSignal::Ended(PumpKind::Notifications)),
            [Osc99DbusEvent::Unavailable]
        );
    }

    // ── Capability refresh (Z5) ─────────────────────────────────────────

    #[test]
    fn capabilities_are_stale_for_an_owner_they_were_not_asked_of() {
        let mut state = DispatcherState::new(
            Osc99SignalSender::Owner(":1.5".to_owned()),
            Box::new(NoIconLookup),
        );
        assert!(state.capabilities_stale(), "never asked");

        state.note_capabilities_asked();
        assert!(!state.capabilities_stale(), "asked of this owner");

        let _ = state.on_signal(&owner_changed(Some(":1.6")));
        assert!(state.capabilities_stale(), "a new owner");

        state.note_capabilities_asked();
        assert!(!state.capabilities_stale());
    }

    #[test]
    fn a_state_with_no_owner_or_an_unfiltered_one_never_wants_a_refresh() {
        for owner in [Osc99SignalSender::NoOwner, Osc99SignalSender::Unfiltered] {
            let state = DispatcherState::new(owner.clone(), Box::new(NoIconLookup));
            assert!(!state.capabilities_stale(), "{owner:?}");
        }
    }

    #[test]
    fn an_adopted_owner_makes_the_capabilities_stale() {
        let mut state = DispatcherState::new(Osc99SignalSender::NoOwner, Box::new(NoIconLookup));

        state.record_shown(instance(1), None, reply_from(7, ":1.7"));

        assert!(state.capabilities_stale());
    }

    #[test]
    fn a_new_session_starts_clean() {
        let mut state = owned_by(Osc99SignalSender::Owner(":1.5".to_owned()));
        state.note_capabilities_asked();
        state.note_call::<()>(&Err(timeout_error()), Instant::now());

        state.begin_session(Osc99SignalSender::Owner(":1.8".to_owned()));

        assert_eq!(
            state.daemon_owner,
            Osc99SignalSender::Owner(":1.8".to_owned())
        );
        assert!(state.by_instance.is_empty());
        assert!(state.by_dbus_id.is_empty());
        assert!(state.capabilities_stale());
        assert_eq!(state.health, Osc99DaemonHealth::Healthy);
    }

    #[test]
    fn a_new_session_assumes_the_capabilities_again() {
        let mut state = DispatcherState::default();
        state.set_capabilities(&strings(&["body", "sound"]));
        assert_ne!(state.capabilities, Osc99DaemonCapabilities::ASSUMED);

        state.begin_session(Osc99SignalSender::Unfiltered);

        assert_eq!(state.capabilities, Osc99DaemonCapabilities::ASSUMED);
    }

    // ── Circuit breaker (Z6) ────────────────────────────────────────────

    #[test]
    fn a_healthy_daemon_is_always_called() {
        let state = DispatcherState::default();

        assert_eq!(state.daemon_gate(Instant::now()), DaemonGate::Call);
    }

    #[test]
    fn a_timeout_keeps_the_daemon_from_being_called_until_the_cooldown_has_passed() {
        let start = Instant::now();
        let mut state = DispatcherState::default();

        state.note_call::<()>(&Err(timeout_error()), start);

        assert_eq!(
            state.health,
            Osc99DaemonHealth::Unresponsive { since: start }
        );
        assert_eq!(state.daemon_gate(start), DaemonGate::Skip);
        assert_eq!(
            state.daemon_gate(
                start + OSC99_DAEMON_COOLDOWN.saturating_sub(Duration::from_millis(1))
            ),
            DaemonGate::Skip
        );
        assert_eq!(
            state.daemon_gate(start + OSC99_DAEMON_COOLDOWN),
            DaemonGate::Call,
            "the cooldown is over"
        );
        assert_eq!(
            state.daemon_gate(start + OSC99_DAEMON_COOLDOWN * 3),
            DaemonGate::Call
        );
    }

    #[test]
    fn a_probe_that_is_answered_makes_the_daemon_healthy_again() {
        let start = Instant::now();
        let probe = start + OSC99_DAEMON_COOLDOWN;
        let mut state = DispatcherState::default();
        state.note_call::<()>(&Err(timeout_error()), start);

        state.note_call(&Ok(()), probe);

        assert_eq!(state.health, Osc99DaemonHealth::Healthy);
        assert_eq!(state.daemon_gate(probe), DaemonGate::Call);
    }

    #[test]
    fn a_probe_that_times_out_again_restarts_the_cooldown() {
        let start = Instant::now();
        let probe = start + OSC99_DAEMON_COOLDOWN;
        let mut state = DispatcherState::default();
        state.note_call::<()>(&Err(timeout_error()), start);

        state.note_call::<()>(&Err(timeout_error()), probe);

        assert_eq!(
            state.health,
            Osc99DaemonHealth::Unresponsive { since: probe }
        );
        assert_eq!(
            state.daemon_gate(
                probe + OSC99_DAEMON_COOLDOWN.saturating_sub(Duration::from_millis(1))
            ),
            DaemonGate::Skip
        );
        assert_eq!(
            state.daemon_gate(probe + OSC99_DAEMON_COOLDOWN),
            DaemonGate::Call
        );
    }

    #[test]
    fn an_error_reply_proves_the_daemon_answers() -> TestResult {
        let start = Instant::now();
        let mut state = DispatcherState::default();
        state.note_call::<()>(&Err(timeout_error()), start);

        state.note_call::<()>(
            &Err(error_reply("org.freedesktop.DBus.Error.Failed")?),
            start,
        );

        assert_eq!(state.health, Osc99DaemonHealth::Healthy);
        Ok(())
    }
}
