// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The Linux/BSD OSC 99 desktop-notification service (Task 138.12).
//!
//! One *dispatcher* thread (`freminal-osc99-dbus`) owns one D-Bus connection
//! and calls `org.freedesktop.Notifications` on it (`GetCapabilities` once at
//! start, `Notify` per show). One *signal pump* thread
//! (`freminal-osc99-signals`) iterates a single match rule over that
//! connection and forwards `ActionInvoked` / `NotificationClosed` **into the
//! dispatcher's input channel**, not to the GUI. The dispatcher owns the
//! D-Bus-id to instance map, so a signal that arrives while a `Notify` call is
//! in flight is only processed after the call has recorded its id. That
//! removes the ordering race between the two threads.
//!
//! The GUI talks to the service through an [`Osc99DbusHandle`]: commands go in
//! over one crossbeam channel and [`Osc99DbusEvent`]s come back over another,
//! each event followed by a repaint wake so an idle window still drains it.
//! The dispatcher exits when the handle (and with it the command sender) is
//! dropped; the pump exits on its next send after that.
//!
//! Signals are filtered by sender (Task 138.C2). On a bus connection the
//! dispatcher resolves the unique name that owns `org.freedesktop.Notifications`
//! (`GetNameOwner`) and ignores `ActionInvoked` / `NotificationClosed` from any
//! other sender, so a session-bus peer cannot forge them. A second pump
//! (`freminal-osc99-owner`) follows `NameOwnerChanged` for that one name so a
//! daemon restart re-targets the filter. zbus's client-side match cannot
//! resolve a well-known sender, so the sender is compared in the dispatcher
//! rather than put in the match rule. A peer-to-peer connection (tests) has no
//! bus, no owner and no sender to check, so it is [`Osc99SignalSender::Unfiltered`].
//! When the owner changes to a different name (or to nobody) the old daemon's
//! notifications are gone with it, so every mapped instance is closed.
//!
//! Every state transition lives in the pure [`DispatcherState`], which has no
//! bus and no threads and is unit-tested directly.
//!
//! Update in place sends the replaced instance's D-Bus id as `replaces_id`.
//! Close calls `CloseNotification` and lets the daemon's `NotificationClosed`
//! signal produce the [`Osc99DbusEvent::Closed`]; an instance with no D-Bus id
//! (its show failed, or it was replaced) is reported closed at once.
//!
//! Presentation (icons, sounds, category, markup) is Task 138.14.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, select};
use freminal_common::buffer_states::osc_notify_99::NotificationUrgency;
use freminal_common::buffer_states::osc99_notification::Osc99Expiry;
use freminal_common::buffer_states::window_manipulation::Notification99Data;
use freminal_common::send_or_log;
use freminal_windowing::{RepaintProxy, WindowId};
use zbus::MatchRule;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::{Message, Type as MessageType};
use zbus::zvariant::{Array, Structure, Value};

use super::icon_lookup::XdgIconLookup;
use super::presentation::{
    Osc99IconChoice, Osc99IconImage, Osc99IconLookup, Osc99SoundHint, category_hint, choose_icon,
    decode_icon_image, escape_body_markup, sound_hint,
};
use super::{
    Osc99DaemonCapabilities, Osc99DaemonSupport, Osc99InstanceId, Osc99LegActivation,
    Osc99SystemBackend, Osc99SystemDispatch,
};

/// The well-known bus name of the notification daemon.
const NOTIFICATIONS_SERVICE: &str = "org.freedesktop.Notifications";
/// The object path the notification daemon serves.
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
/// The notification interface.
const NOTIFICATIONS_INTERFACE: &str = "org.freedesktop.Notifications";
/// The bus driver's name, which is also the sender of its signals.
const BUS_DRIVER_NAME: &str = "org.freedesktop.DBus";
/// The object path of the bus driver.
const BUS_DRIVER_PATH: &str = "/org/freedesktop/DBus";
/// The bus driver's interface.
const BUS_DRIVER_INTERFACE: &str = "org.freedesktop.DBus";
/// The `app_name` sent when the notification names no application (`f=`).
const DEFAULT_APP_NAME: &str = "freminal";
/// How long a D-Bus method call waits for its reply before it fails. This is
/// the libdbus default reply timeout. zbus's blocking calls have no timeout of
/// their own, so without it a daemon that never answers would block the
/// dispatcher thread forever and silently end the Linux leg. A timed-out
/// `Notify` becomes [`Osc99DbusEvent::ShowFailed`], and a timed-out
/// `CloseNotification` falls back to [`Osc99DbusEvent::Closed`].
const OSC99_DBUS_METHOD_TIMEOUT: Duration = Duration::from_secs(25);
/// The most button actions sent: OSC 99 keeps at most eight buttons, and
/// action keys `"1"`-`"8"` are the only ones mapped back.
const MAX_BUTTON_ACTIONS: usize = 8;

// ── Public vocabulary ──────────────────────────────────────────────────────

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

/// The action a user invoked on a desktop notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99SystemAction {
    /// The notification itself (action key `"default"`).
    Default,
    /// The 1-based button (action key `"1"` to `"8"`).
    Button(u8),
}

impl Osc99SystemAction {
    /// The registry's vocabulary for the same activation.
    #[must_use]
    pub const fn to_leg_activation(self) -> Osc99LegActivation {
        match self {
            Self::Default => Osc99LegActivation::Body,
            Self::Button(n) => Osc99LegActivation::Button(n),
        }
    }

    /// The action an `ActionInvoked` key stands for, or `None` for a key
    /// freminal never registered.
    fn from_key(key: &str) -> Option<Self> {
        match key.as_bytes() {
            b"default" => Some(Self::Default),
            [digit @ b'1'..=b'8'] => Some(Self::Button(digit - b'0')),
            _ => None,
        }
    }
}

/// What the service reports to the GUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Osc99DbusEvent {
    /// The daemon accepted `instance`.
    Shown {
        /// The instance that was displayed.
        instance: Osc99InstanceId,
    },
    /// The daemon did not display `instance`.
    ShowFailed {
        /// The instance that was not displayed.
        instance: Osc99InstanceId,
    },
    /// The user invoked an action on `instance`.
    Activated {
        /// The instance that was activated.
        instance: Osc99InstanceId,
        /// What the user invoked.
        action: Osc99SystemAction,
    },
    /// The daemon closed `instance`.
    Closed {
        /// The instance that was closed.
        instance: Osc99InstanceId,
    },
    /// The daemon answered `GetCapabilities`.
    Capabilities(Osc99DaemonCapabilities),
    /// The session bus (or the signal subscription) could not be set up: the
    /// system leg cannot display or observe anything.
    Unavailable,
}

// ── Repaint wake ───────────────────────────────────────────────────────────

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

    /// A waker for [`Osc99DbusService::spawn`].
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

// ── Pure dispatcher state ──────────────────────────────────────────────────

/// The `urgency` hint's value (`0` low, `1` normal, `2` critical).
const fn urgency_byte(urgency: NotificationUrgency) -> u8 {
    match urgency {
        NotificationUrgency::Low => 0,
        NotificationUrgency::Normal => 1,
        NotificationUrgency::Critical => 2,
    }
}

/// The icon-theme name of freminal's own icon, as `assets/freminal.desktop`
/// names it. The last resort of icon resolution.
const FREMINAL_ICON_NAME: &str = "freminal";

/// The wire value of the `suppress-sound` hint when it is sent: the hint's
/// presence with `true` is the request, so there is no `false` to send.
const SUPPRESS_SOUND_WIRE: bool = true;

/// The `has_alpha` field of the `image-data` structure: the pixels are always
/// RGBA.
const IMAGE_HAS_ALPHA_WIRE: bool = true;

/// The `bits_per_sample` field of the `image-data` structure.
const IMAGE_BITS_PER_SAMPLE: i32 = 8;

/// The `channels` field of the `image-data` structure (R, G, B, A).
const IMAGE_CHANNELS: i32 = 4;

/// The `hints` argument of `Notify`, before it becomes an `a{sv}` dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NotifyHints {
    /// The `urgency` hint.
    urgency: u8,
    /// The `category` hint.
    category: Option<String>,
    /// The `suppress-sound` / `sound-name` hint.
    sound: Osc99SoundHint,
    /// The `image-data` hint, for transmitted icon data.
    image: Option<Osc99IconImage>,
}

impl NotifyHints {
    /// The wire form: an `a{sv}` dictionary.
    fn to_map(&self) -> HashMap<&'static str, Value<'static>> {
        let mut map = HashMap::from([("urgency", Value::from(self.urgency))]);
        if let Some(category) = &self.category {
            map.insert("category", Value::from(category.clone()));
        }
        match &self.sound {
            Osc99SoundHint::None => {}
            Osc99SoundHint::Suppress => {
                map.insert("suppress-sound", Value::from(SUPPRESS_SOUND_WIRE));
            }
            Osc99SoundHint::Named(name) => {
                map.insert("sound-name", Value::from(name.clone()));
            }
        }
        if let Some(image) = &self.image {
            // `(iiibiiay)`: width, height, rowstride, has_alpha,
            // bits_per_sample, channels, data.
            let structure = Structure::from((
                image.width,
                image.height,
                image.rowstride,
                IMAGE_HAS_ALPHA_WIRE,
                IMAGE_BITS_PER_SAMPLE,
                IMAGE_CHANNELS,
                Array::from(image.rgba.as_slice()),
            ));
            map.insert("image-data", Value::Structure(structure));
        }
        map
    }
}

/// The arguments of one `Notify` call, in the order the method takes them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NotifyArgs {
    app_name: String,
    replaces_id: u32,
    app_icon: String,
    summary: String,
    body: String,
    actions: Vec<String>,
    hints: NotifyHints,
    expire_timeout: i32,
}

impl NotifyArgs {
    /// The `app_icon` and `image-data` for `notification`.
    fn icon(
        lookup: &dyn Osc99IconLookup,
        notification: &Notification99Data,
    ) -> (String, Option<Osc99IconImage>) {
        let choice = choose_icon(
            &notification.icon_names,
            notification.icon_data.as_deref(),
            notification.app_name.as_deref(),
            lookup,
        );
        let freminal_icon = || (FREMINAL_ICON_NAME.to_owned(), None);
        match choice {
            Some(Osc99IconChoice::Named(name)) => (name, None),
            Some(Osc99IconChoice::Data(bytes)) => decode_icon_image(&bytes).map_or_else(
                || {
                    tracing::debug!("OSC 99: the transmitted icon did not decode");
                    notification
                        .icon_names
                        .first()
                        .map_or_else(freminal_icon, |name| (name.clone(), None))
                },
                |image| (String::new(), Some(image)),
            ),
            // No rule found an icon: the last one is freminal's own.
            None => freminal_icon(),
        }
    }
}

/// What a `Close` command has to do, given the dispatcher's bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseTarget {
    /// The daemon displays the instance as this id: call `CloseNotification`.
    CallDaemon(u32),
    /// The instance has no D-Bus id (never shown, show failed, replaced, or
    /// already closed), so there is nothing to call and no signal will come.
    AlreadyGone,
}

/// A signal a pump forwards to the dispatcher.
///
/// The notification signals carry the unique name of their sender (`None` when
/// the message has no sender header, as over a peer-to-peer connection) so the
/// dispatcher can drop forgeries.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PumpSignal {
    /// `ActionInvoked(id, action_key)`.
    ActionInvoked {
        sender: Option<String>,
        dbus_id: u32,
        action_key: String,
    },
    /// `NotificationClosed(id, reason)`. The reason does not matter to the
    /// protocol, so it is not carried.
    NotificationClosed {
        sender: Option<String>,
        dbus_id: u32,
    },
    /// `NameOwnerChanged` for `org.freedesktop.Notifications`: the unique name
    /// that owns it now, or `None` when nobody does.
    OwnerChanged { new_owner: Option<String> },
}

/// Whose notification signals the dispatcher believes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Osc99SignalSender {
    /// There is no bus (a peer-to-peer connection), so there is no sender to
    /// check: every signal is believed.
    #[default]
    Unfiltered,
    /// Only signals from this unique name are believed.
    Owner(String),
    /// Nobody owns `org.freedesktop.Notifications`: no signal is believed.
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
}

/// The dispatcher's bookkeeping, free of any bus or thread.
struct DispatcherState {
    /// Whose notification signals are believed.
    daemon_owner: Osc99SignalSender,
    /// The daemon's id for each displayed instance.
    by_instance: HashMap<Osc99InstanceId, u32>,
    /// The instance behind each of the daemon's ids.
    by_dbus_id: HashMap<u32, Osc99InstanceId>,
    /// What the daemon can do; assumed until it answers.
    capabilities: Osc99DaemonCapabilities,
    /// What the icon and `.desktop` scans know; scanned on first use.
    icon_lookup: Box<dyn Osc99IconLookup + Send>,
}

impl DispatcherState {
    /// A state with no instances, the assumed capabilities, and `icon_lookup`
    /// to resolve icons with.
    fn new(daemon_owner: Osc99SignalSender, icon_lookup: Box<dyn Osc99IconLookup + Send>) -> Self {
        Self {
            daemon_owner,
            by_instance: HashMap::new(),
            by_dbus_id: HashMap::new(),
            capabilities: Osc99DaemonCapabilities::default(),
            icon_lookup,
        }
    }

    /// Record the daemon's `GetCapabilities` answer.
    fn set_capabilities(&mut self, advertised: &[String]) {
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
    fn notify_args(
        &self,
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
        let (app_icon, image) = NotifyArgs::icon(self.icon_lookup.as_ref(), notification);
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
            replaces_id: replaces
                .and_then(|old| self.by_instance.get(&old).copied())
                .unwrap_or(0),
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

    /// Record that the daemon displayed `instance` as `dbus_id`.
    ///
    /// When `replaces` is given the replaced instance's mappings are dropped
    /// first, so the replaced notification's later `NotificationClosed` is
    /// ignored (and a daemon that reuses the id for the update still maps it
    /// to the new instance).
    fn record_shown(
        &mut self,
        instance: Osc99InstanceId,
        replaces: Option<Osc99InstanceId>,
        dbus_id: u32,
    ) {
        if let Some(old) = replaces {
            self.forget_instance(old);
        }
        self.by_instance.insert(instance, dbus_id);
        self.by_dbus_id.insert(dbus_id, instance);
    }

    /// Drop `instance`'s mappings, if it has any.
    fn forget_instance(&mut self, instance: Osc99InstanceId) {
        if let Some(dbus_id) = self.by_instance.remove(&instance) {
            self.by_dbus_id.remove(&dbus_id);
        }
    }

    /// What closing `instance` calls for: the daemon's id when it displays
    /// the instance, otherwise [`CloseTarget::AlreadyGone`].
    fn close_target(&self, instance: Osc99InstanceId) -> CloseTarget {
        self.by_instance
            .get(&instance)
            .map_or(CloseTarget::AlreadyGone, |dbus_id| {
                CloseTarget::CallDaemon(*dbus_id)
            })
    }

    /// The event for an `ActionInvoked` signal. The mappings stay: a
    /// `NotificationClosed` normally follows. `None` for an id with no live
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
    fn on_signal(&mut self, signal: &PumpSignal) -> Vec<Osc99DbusEvent> {
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
    /// The same owner changes nothing. When a *known* owner is replaced by a
    /// different name, or by nobody, the old daemon (and every notification it
    /// displayed) is gone and will never send the signals that would end
    /// those legs, so every mapped instance is reported
    /// [`Osc99DbusEvent::Closed`] and the maps are cleared. This is freminal's
    /// choice, so the registry can finish those legs and send the owed close
    /// reports instead of leaving them open forever. A change *from* no owner
    /// closes nothing: a `Notify` that auto-started the daemon can be recorded
    /// before the change is processed, and those notifications belong to the
    /// new owner.
    fn on_owner_changed(&mut self, new_owner: Option<String>) -> Vec<Osc99DbusEvent> {
        let new = Osc99SignalSender::from_owner(new_owner);
        if self.daemon_owner == new {
            return Vec::new();
        }
        let previous = std::mem::replace(&mut self.daemon_owner, new);
        if !matches!(previous, Osc99SignalSender::Owner(_)) {
            return Vec::new();
        }
        self.by_dbus_id.clear();
        let mut instances: Vec<Osc99InstanceId> = self
            .by_instance
            .drain()
            .map(|(instance, _)| instance)
            .collect();
        instances.sort_by_key(|instance| instance.0);
        instances
            .into_iter()
            .map(|instance| Osc99DbusEvent::Closed { instance })
            .collect()
    }
}

// ── Handle ─────────────────────────────────────────────────────────────────

/// A request from the GUI to the dispatcher.
#[derive(Debug)]
enum DbusCommand {
    /// Display `notification` for `instance`, replacing `replaces`'.
    Show {
        instance: Osc99InstanceId,
        replaces: Option<Osc99InstanceId>,
        notification: Box<Notification99Data>,
    },
    /// Close `instance`'s notification. The dispatcher answers with
    /// [`Osc99DbusEvent::Closed`] (from the daemon's signal, or at once when
    /// there is nothing to close).
    Close { instance: Osc99InstanceId },
}

/// The GUI's end of the service.
///
/// Dropping it stops the dispatcher (and, on its next send, the pump).
pub struct Osc99DbusHandle {
    /// Commands for the dispatcher.
    commands: Sender<DbusCommand>,
    /// Events from the dispatcher.
    events: Receiver<Osc99DbusEvent>,
}

impl Osc99DbusHandle {
    /// The events that have arrived since the last call, oldest first. Never
    /// blocks.
    pub fn try_events(&self) -> impl Iterator<Item = Osc99DbusEvent> + '_ {
        self.events.try_iter()
    }
}

impl Osc99DbusHandle {
    /// Queue `command`. A dispatcher that is gone will never answer, so the
    /// caller must end the instance's system leg itself.
    fn dispatch(&self, command: DbusCommand, what: &'static str) -> Osc99SystemDispatch {
        match self.commands.send(command) {
            Ok(()) => Osc99SystemDispatch::Queued,
            Err(e) => {
                tracing::warn!("OSC 99: notification service is gone; {what} failed: {e}");
                Osc99SystemDispatch::Failed
            }
        }
    }
}

impl Osc99SystemBackend for Osc99DbusHandle {
    fn show(
        &mut self,
        instance: Osc99InstanceId,
        replaces: Option<Osc99InstanceId>,
        notification: &Notification99Data,
    ) -> Osc99SystemDispatch {
        let command = DbusCommand::Show {
            instance,
            replaces,
            notification: Box::new(notification.clone()),
        };
        self.dispatch(command, "show")
    }

    fn close(&mut self, instance: Osc99InstanceId) -> Osc99SystemDispatch {
        self.dispatch(DbusCommand::Close { instance }, "close")
    }
}

// ── Service ────────────────────────────────────────────────────────────────

/// Why the service could not start.
#[derive(Debug, thiserror::Error)]
enum StartError {
    /// The bus connection or the signal subscription failed.
    #[error("session bus unavailable: {0}")]
    Bus(#[from] zbus::Error),
    /// The signal pump thread could not be spawned.
    #[error("could not spawn the signal pump thread: {0}")]
    PumpThread(#[source] std::io::Error),
}

/// The dispatcher's way back to the GUI: an event channel plus the repaint
/// wake that follows every event.
struct EventSink {
    events: Sender<Osc99DbusEvent>,
    waker: Box<dyn Fn() + Send>,
}

impl EventSink {
    /// Send `event` to the GUI, then wake it.
    fn emit(&self, event: Osc99DbusEvent) {
        if let Err(e) = self.events.send(event) {
            tracing::debug!("OSC 99: GUI is gone; dropping a notification event: {e}");
            return;
        }
        (self.waker)();
    }
}

/// Namespace for starting the notification service.
pub struct Osc99DbusService;

impl Osc99DbusService {
    /// Start the service on the session bus. `waker` is called after every
    /// event so an idle GUI drains it.
    ///
    /// Never blocks: connecting happens on the dispatcher thread. If the bus
    /// cannot be reached the handle reports [`Osc99DbusEvent::Unavailable`]
    /// once, a single warning is logged, every later show answers
    /// [`Osc99DbusEvent::ShowFailed`], and every later close answers
    /// [`Osc99DbusEvent::Closed`].
    #[must_use]
    pub fn spawn(waker: Box<dyn Fn() + Send>) -> Osc99DbusHandle {
        start(connect_session, waker, Box::new(XdgIconLookup::from_env()))
    }

    /// Start the service on an existing connection (a peer-to-peer one, in
    /// tests).
    #[cfg(test)]
    fn spawn_with_connection(
        connection: Connection,
        waker: Box<dyn Fn() + Send>,
    ) -> Osc99DbusHandle {
        start(move || Ok(connection), waker, Box::new(NoIconLookup))
    }
}

/// An icon lookup that knows nothing, so a test touches no filesystem.
#[cfg(test)]
struct NoIconLookup;

#[cfg(test)]
impl Osc99IconLookup for NoIconLookup {
    fn icon_exists(&self, _name: &str) -> bool {
        false
    }

    fn desktop_icon(&self, _app_name: &str) -> Option<String> {
        None
    }
}

#[cfg(test)]
impl Default for DispatcherState {
    fn default() -> Self {
        Self::new(Osc99SignalSender::default(), Box::new(NoIconLookup))
    }
}

/// Connect to the session bus with [`OSC99_DBUS_METHOD_TIMEOUT`] on every
/// method call.
fn connect_session() -> zbus::Result<Connection> {
    zbus::blocking::connection::Builder::session()?
        .method_timeout(OSC99_DBUS_METHOD_TIMEOUT)
        .build()
}

/// Start the dispatcher thread, which obtains its connection from `connect`
/// and resolves icons with `icon_lookup` (scanning lazily, on that thread).
fn start<C>(
    connect: C,
    waker: Box<dyn Fn() + Send>,
    icon_lookup: Box<dyn Osc99IconLookup + Send>,
) -> Osc99DbusHandle
where
    C: FnOnce() -> zbus::Result<Connection> + Send + 'static,
{
    let (command_tx, command_rx) = crossbeam_channel::unbounded();
    let (event_tx, event_rx) = crossbeam_channel::unbounded();
    let sink = EventSink {
        events: event_tx.clone(),
        waker,
    };

    let spawned = std::thread::Builder::new()
        .name("freminal-osc99-dbus".to_owned())
        .spawn(move || run_dispatcher(connect, &command_rx, &sink, icon_lookup));
    if let Err(e) = spawned {
        // The closure (and the command receiver in it) is dropped, so every
        // later show fails at the handle.
        tracing::warn!("OSC 99: could not spawn the notification service thread: {e}");
        send_or_log!(
            event_tx,
            Osc99DbusEvent::Unavailable,
            "Failed to report the OSC 99 service as unavailable"
        );
    }

    Osc99DbusHandle {
        commands: command_tx,
        events: event_rx,
    }
}

/// The dispatcher thread: connect, then serve commands until the GUI drops its
/// command sender.
fn run_dispatcher<C>(
    connect: C,
    commands: &Receiver<DbusCommand>,
    sink: &EventSink,
    icon_lookup: Box<dyn Osc99IconLookup + Send>,
) where
    C: FnOnce() -> zbus::Result<Connection>,
{
    match connect_and_subscribe(connect) {
        Ok((connection, signals)) => serve(&connection, commands, signals, sink, icon_lookup),
        Err(e) => {
            tracing::warn!("OSC 99: system notifications are unavailable: {e}");
            sink.emit(Osc99DbusEvent::Unavailable);
            fail_every_command(commands, sink);
        }
    }
}

/// Connect, subscribe to the notification signals, and start the pump(s).
///
/// The subscriptions are made here, before any pump thread starts and before
/// the first `Notify` or `GetNameOwner`, so no signal can be missed. On a bus
/// connection there is a second subscription, to `NameOwnerChanged` for the
/// daemon's name, with its own pump: a blocking iterator reads one stream, and
/// match rules cannot be OR-ed, so two streams need two threads. Both pumps
/// feed the one signal channel. A peer-to-peer connection has no bus driver to
/// ask, so it gets only the first.
fn connect_and_subscribe<C>(connect: C) -> Result<(Connection, Receiver<PumpSignal>), StartError>
where
    C: FnOnce() -> zbus::Result<Connection>,
{
    let connection = connect()?;
    let notifications =
        MessageIterator::for_match_rule(notification_signals_rule()?, &connection, None)?;
    let owner_changes = if connection.is_bus() {
        Some(MessageIterator::for_match_rule(
            owner_changed_rule()?,
            &connection,
            None,
        )?)
    } else {
        None
    };

    let (signal_tx, signal_rx) = crossbeam_channel::unbounded();
    if let Some(owner_changes) = owner_changes {
        let owner_tx = signal_tx.clone();
        std::thread::Builder::new()
            .name("freminal-osc99-owner".to_owned())
            .spawn(move || run_pump(owner_changes, &owner_tx, parse_owner_changed))
            .map_err(StartError::PumpThread)?;
    }
    std::thread::Builder::new()
        .name("freminal-osc99-signals".to_owned())
        .spawn(move || run_pump(notifications, &signal_tx, parse_signal))
        .map_err(StartError::PumpThread)?;
    Ok((connection, signal_rx))
}

/// The match rule for the daemon's `ActionInvoked` / `NotificationClosed`
/// signals. It names no sender: see [`Osc99SignalSender`].
fn notification_signals_rule() -> zbus::Result<MatchRule<'static>> {
    Ok(MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(NOTIFICATIONS_INTERFACE)?
        .path(NOTIFICATIONS_PATH)?
        .build())
}

/// The match rule for the bus driver's `NameOwnerChanged` signal about the
/// notification daemon's name.
fn owner_changed_rule() -> zbus::Result<MatchRule<'static>> {
    Ok(MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(BUS_DRIVER_NAME)?
        .interface(BUS_DRIVER_INTERFACE)?
        .member("NameOwnerChanged")?
        .arg(0, NOTIFICATIONS_SERVICE)?
        .build())
}

/// Answer every show with `ShowFailed` and every close with `Closed` until
/// the GUI drops its sender: with no bus, nothing is displayed, so nothing
/// needs closing.
fn fail_every_command(commands: &Receiver<DbusCommand>, sink: &EventSink) {
    for command in commands {
        match command {
            DbusCommand::Show { instance, .. } => {
                sink.emit(Osc99DbusEvent::ShowFailed { instance });
            }
            DbusCommand::Close { instance } => sink.emit(Osc99DbusEvent::Closed { instance }),
        }
    }
}

/// Serve `commands` and the pump's `signals` until the GUI drops its command
/// sender.
fn serve(
    connection: &Connection,
    commands: &Receiver<DbusCommand>,
    signals: Receiver<PumpSignal>,
    sink: &EventSink,
    icon_lookup: Box<dyn Osc99IconLookup + Send>,
) {
    let mut state = DispatcherState::new(resolve_daemon_owner(connection), icon_lookup);
    match fetch_capabilities(connection) {
        Ok(advertised) => {
            state.set_capabilities(&advertised);
            sink.emit(Osc99DbusEvent::Capabilities(state.capabilities));
        }
        Err(e) => tracing::warn!("OSC 99: GetCapabilities failed; assuming defaults: {e}"),
    }

    // A pump that ended leaves a receiver that is ready forever; once that is
    // seen the arm is parked on a channel that never delivers.
    let parked = crossbeam_channel::never::<PumpSignal>();
    let mut pump = Some(signals);
    loop {
        let signal_rx = pump.as_ref().unwrap_or(&parked);
        let pump_ended = select! {
            recv(commands) -> command => match command {
                Ok(command) => {
                    handle_command(connection, &mut state, sink, command);
                    false
                }
                Err(_) => return,
            },
            recv(signal_rx) -> signal => signal.map_or(true, |signal| {
                for event in state.on_signal(&signal) {
                    sink.emit(event);
                }
                false
            }),
        };
        if pump_ended {
            tracing::warn!("OSC 99: the notification signal pump ended; no more signals");
            pump = None;
        }
    }
}

/// Carry out one GUI command.
fn handle_command(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
    command: DbusCommand,
) {
    match command {
        DbusCommand::Show {
            instance,
            replaces,
            notification,
        } => show_notification(connection, state, sink, instance, replaces, &notification),
        DbusCommand::Close { instance } => close_instance(connection, state, sink, instance),
    }
}

/// Carry out a `Show`: call `Notify` and record or report the outcome.
fn show_notification(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
    instance: Osc99InstanceId,
    replaces: Option<Osc99InstanceId>,
    notification: &Notification99Data,
) {
    let args = state.notify_args(replaces, notification);
    match notify(connection, &args) {
        Ok(dbus_id) => {
            state.record_shown(instance, replaces, dbus_id);
            sink.emit(Osc99DbusEvent::Shown { instance });
        }
        Err(e) => {
            tracing::warn!("OSC 99: Notify failed: {e}");
            if let Some(old) = replaces {
                // The registry already forgot the replaced instance, so its
                // signals are of no interest either.
                state.forget_instance(old);
            }
            sink.emit(Osc99DbusEvent::ShowFailed { instance });
        }
    }
}

/// Carry out a `Close`.
///
/// An instance the daemon displays is closed with `CloseNotification`, and the
/// daemon's `NotificationClosed` signal then produces the `Closed` event. If
/// the call fails the signal may never come, so the mappings are dropped and
/// `Closed` is emitted here (kitty's fallback). An instance with no D-Bus id
/// is reported closed at once.
fn close_instance(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
    instance: Osc99InstanceId,
) {
    match state.close_target(instance) {
        CloseTarget::CallDaemon(dbus_id) => {
            if let Err(e) = close_notification(connection, dbus_id) {
                tracing::warn!("OSC 99: CloseNotification failed: {e}");
                state.forget_instance(instance);
                sink.emit(Osc99DbusEvent::Closed { instance });
            }
        }
        CloseTarget::AlreadyGone => sink.emit(Osc99DbusEvent::Closed { instance }),
    }
}

/// Who owns `org.freedesktop.Notifications` at start.
///
/// A peer-to-peer connection has no bus driver and no sender to check, so it
/// is [`Osc99SignalSender::Unfiltered`]. On a bus, an error (most likely that
/// no daemon is running yet) is [`Osc99SignalSender::NoOwner`]: signals are
/// ignored until a `NameOwnerChanged` names an owner. The `NameOwnerChanged`
/// subscription was made before this call, so an owner appearing meanwhile is
/// queued for the dispatcher, and processing it after a matching answer here
/// is a no-op.
fn resolve_daemon_owner(connection: &Connection) -> Osc99SignalSender {
    if !connection.is_bus() {
        return Osc99SignalSender::Unfiltered;
    }
    match fetch_daemon_owner(connection) {
        Ok(owner) => Osc99SignalSender::Owner(owner),
        Err(e) => {
            tracing::debug!("OSC 99: no notification daemon owns its name yet: {e}");
            Osc99SignalSender::NoOwner
        }
    }
}

/// Call the bus driver's `GetNameOwner` for the notification daemon's name.
fn fetch_daemon_owner(connection: &Connection) -> zbus::Result<String> {
    let reply = connection.call_method(
        Some(BUS_DRIVER_NAME),
        BUS_DRIVER_PATH,
        Some(BUS_DRIVER_INTERFACE),
        "GetNameOwner",
        &NOTIFICATIONS_SERVICE,
    )?;
    reply.body().deserialize::<String>()
}

/// Call `GetCapabilities`.
fn fetch_capabilities(connection: &Connection) -> zbus::Result<Vec<String>> {
    let reply = connection.call_method(
        Some(NOTIFICATIONS_SERVICE),
        NOTIFICATIONS_PATH,
        Some(NOTIFICATIONS_INTERFACE),
        "GetCapabilities",
        &(),
    )?;
    reply.body().deserialize::<Vec<String>>()
}

/// Call `CloseNotification` for the daemon's `dbus_id`.
fn close_notification(connection: &Connection, dbus_id: u32) -> zbus::Result<()> {
    connection.call_method(
        Some(NOTIFICATIONS_SERVICE),
        NOTIFICATIONS_PATH,
        Some(NOTIFICATIONS_INTERFACE),
        "CloseNotification",
        &dbus_id,
    )?;
    Ok(())
}

/// Call `Notify`, returning the daemon's id for the notification.
fn notify(connection: &Connection, args: &NotifyArgs) -> zbus::Result<u32> {
    let reply = connection.call_method(
        Some(NOTIFICATIONS_SERVICE),
        NOTIFICATIONS_PATH,
        Some(NOTIFICATIONS_INTERFACE),
        "Notify",
        &(
            args.app_name.as_str(),
            args.replaces_id,
            args.app_icon.as_str(),
            args.summary.as_str(),
            args.body.as_str(),
            args.actions.as_slice(),
            args.hints.to_map(),
            args.expire_timeout,
        ),
    )?;
    reply.body().deserialize::<u32>()
}

// ── Signal pump ────────────────────────────────────────────────────────────

/// A pump thread: forward the signals `parse` recognises to the dispatcher
/// until the bus closes or the dispatcher is gone.
fn run_pump(
    messages: MessageIterator,
    signals: &Sender<PumpSignal>,
    parse: fn(&Message) -> Option<PumpSignal>,
) {
    for message in messages {
        let message = match message {
            Ok(message) => message,
            Err(e) => {
                tracing::warn!("OSC 99: notification signal stream failed: {e}");
                return;
            }
        };
        if let Some(signal) = parse(&message)
            && signals.send(signal).is_err()
        {
            // The dispatcher dropped its receiver: the GUI is gone.
            return;
        }
    }
}

/// The signal `message` carries, or `None` for any member other than
/// `ActionInvoked` and `NotificationClosed` and for a malformed body.
fn parse_signal(message: &Message) -> Option<PumpSignal> {
    let header = message.header();
    let member = header.member()?;
    let sender = header.sender().map(ToString::to_string);
    let body = message.body();
    match member.as_str() {
        "ActionInvoked" => {
            let (dbus_id, action_key) = body.deserialize::<(u32, String)>().ok()?;
            Some(PumpSignal::ActionInvoked {
                sender,
                dbus_id,
                action_key,
            })
        }
        "NotificationClosed" => {
            let (dbus_id, _reason) = body.deserialize::<(u32, u32)>().ok()?;
            Some(PumpSignal::NotificationClosed { sender, dbus_id })
        }
        _ => None,
    }
}

/// The [`PumpSignal::OwnerChanged`] `message` carries, or `None` for anything
/// else: another member, another name, a malformed body, or a sender that is
/// not the bus driver. The match rule names the bus driver as sender, but
/// zbus's client-side match cannot check a well-known sender, so it is checked
/// here: otherwise a peer could emit a forged `NameOwnerChanged` and name
/// itself the daemon.
fn parse_owner_changed(message: &Message) -> Option<PumpSignal> {
    let header = message.header();
    if header.member()?.as_str() != "NameOwnerChanged"
        || header.sender()?.as_str() != BUS_DRIVER_NAME
    {
        return None;
    }
    let (name, _old_owner, new_owner) = message
        .body()
        .deserialize::<(String, String, String)>()
        .ok()?;
    if name != NOTIFICATIONS_SERVICE {
        return None;
    }
    Some(PumpSignal::OwnerChanged {
        new_owner: (!new_owner.is_empty()).then_some(new_owner),
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::num::NonZeroU32;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use freminal_common::buffer_states::osc_notify_99::NotificationOccasion;
    use freminal_common::buffer_states::osc99_notification::{
        Osc99ActivationFocus, Osc99ActivationReporting, Osc99CloseReporting, Osc99Sound,
    };
    use std::collections::HashSet;

    use zbus::zvariant::OwnedValue;

    use super::*;

    /// How long a test waits for something that must happen. Generous so a
    /// loaded CI machine never trips it; a healthy run never waits it out.
    const EVENT_TIMEOUT: Duration = Duration::from_secs(20);

    type TestResult = Result<(), Box<dyn Error>>;

    fn instance(n: u64) -> Osc99InstanceId {
        Osc99InstanceId(n)
    }

    fn notification() -> Notification99Data {
        Notification99Data {
            id: None,
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

    fn with_buttons(labels: &[&str]) -> Notification99Data {
        Notification99Data {
            buttons: labels.iter().map(|label| (*label).to_owned()).collect(),
            ..notification()
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    // ── DispatcherState ─────────────────────────────────────────────────

    #[test]
    fn notify_args_carry_title_body_app_name_and_urgency() {
        let state = DispatcherState::default();
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
                image: None,
            }
        );
    }

    // ── Presentation: icon, sound, category, markup ─────────────────────

    /// An icon lookup over fixed sets, touching no filesystem.
    struct FakeLookup {
        icons: HashSet<String>,
        desktop: HashMap<String, String>,
    }

    impl Osc99IconLookup for FakeLookup {
        fn icon_exists(&self, name: &str) -> bool {
            self.icons.contains(name)
        }

        fn desktop_icon(&self, app_name: &str) -> Option<String> {
            self.desktop.get(app_name).cloned()
        }
    }

    fn state_with_icons(icons: &[&str], desktop: &[(&str, &str)]) -> DispatcherState {
        DispatcherState::new(
            Osc99SignalSender::default(),
            Box::new(FakeLookup {
                icons: icons.iter().map(|icon| (*icon).to_owned()).collect(),
                desktop: desktop
                    .iter()
                    .map(|(app, icon)| ((*app).to_owned(), (*icon).to_owned()))
                    .collect(),
            }),
        )
    }

    /// A `width` x `height` PNG of one opaque colour.
    fn png(width: u32, height: u32) -> Result<Vec<u8>, image::ImageError> {
        let pixels = image::RgbaImage::from_pixel(width, height, image::Rgba([1, 2, 3, 255]));
        let mut bytes = Vec::new();
        pixels.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )?;
        Ok(bytes)
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
        let state = DispatcherState::default();
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
    fn hints_become_the_wire_dictionary() {
        let hints = NotifyHints {
            urgency: 1,
            category: Some("im".to_owned()),
            sound: Osc99SoundHint::Suppress,
            image: None,
        };
        let map = hints.to_map();
        let mut keys: Vec<_> = map.keys().copied().collect();
        keys.sort_unstable();

        assert_eq!(keys, ["category", "suppress-sound", "urgency"]);
        assert_eq!(map.get("suppress-sound"), Some(&Value::from(true)));
        assert_eq!(map.get("urgency"), Some(&Value::from(1u8)));

        let named = NotifyHints {
            sound: Osc99SoundHint::Named("dialog-error".to_owned()),
            ..hints
        };
        let map = named.to_map();
        assert_eq!(
            map.get("sound-name"),
            Some(&Value::from("dialog-error".to_owned()))
        );
        assert!(!map.contains_key("suppress-sound"));
    }

    #[test]
    fn notify_args_use_an_existing_icon_name_over_data() -> TestResult {
        let state = state_with_icons(&["mail-unread"], &[]);
        let args = state.notify_args(
            None,
            &Notification99Data {
                icon_names: strings(&["mail-unread"]),
                icon_data: Some(png(2, 2)?),
                ..notification()
            },
        );

        assert_eq!(args.app_icon, "mail-unread");
        assert_eq!(args.hints.image, None);
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
        let image = args.hints.image.ok_or("no image hint")?;
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

        let image = args.hints.image.ok_or("no image hint")?;
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
        assert_eq!(args.hints.image, None);
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
        assert_eq!(args.hints.image, None);
    }

    #[test]
    fn notify_args_take_the_icon_of_the_desktop_file_matching_the_app() {
        let state = state_with_icons(&[], &[("my-app", "my-app-icon")]);
        let args = state.notify_args(
            None,
            &Notification99Data {
                app_name: Some("my-app".to_owned()),
                ..notification()
            },
        );

        assert_eq!(args.app_icon, "my-app-icon");
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
        let state = DispatcherState::default();
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
        let state = DispatcherState::default();
        let timeout = |expiry| {
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
        state.record_shown(instance(1), None, 42);

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
        state.record_shown(instance(1), None, 7);

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
        state.record_shown(instance(1), None, 7);

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
        state.record_shown(instance(1), None, 7);

        for key in ["0", "9", "x", "", "10", "01", "+1", "Default", "default "] {
            assert_eq!(state.on_action(7, key), None, "key {key:?}");
        }
    }

    #[test]
    fn a_close_after_an_activation_maps_both() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, 7);

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
        state.record_shown(instance(1), None, 7);

        state.record_shown(instance(2), Some(instance(1)), 8);

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
        state.record_shown(instance(1), None, 7);

        state.record_shown(instance(2), Some(instance(1)), 7);

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
        state.record_shown(instance(1), None, 7);

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

    fn action_from(sender: Option<&str>, dbus_id: u32, key: &str) -> PumpSignal {
        PumpSignal::ActionInvoked {
            sender: sender.map(str::to_owned),
            dbus_id,
            action_key: key.to_owned(),
        }
    }

    fn closed_from(sender: Option<&str>, dbus_id: u32) -> PumpSignal {
        PumpSignal::NotificationClosed {
            sender: sender.map(str::to_owned),
            dbus_id,
        }
    }

    fn owner_changed(new_owner: Option<&str>) -> PumpSignal {
        PumpSignal::OwnerChanged {
            new_owner: new_owner.map(str::to_owned),
        }
    }

    /// A state that believes `owner`, with instances 1 and 2 shown as ids 7
    /// and 8.
    fn owned_by(owner: Osc99SignalSender) -> DispatcherState {
        let mut state = DispatcherState {
            daemon_owner: owner,
            ..DispatcherState::default()
        };
        state.record_shown(instance(1), None, 7);
        state.record_shown(instance(2), None, 8);
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
        state.record_shown(instance(3), None, 1);
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
    fn system_actions_map_to_the_registrys_activations() {
        assert_eq!(
            Osc99SystemAction::Default.to_leg_activation(),
            Osc99LegActivation::Body
        );
        assert_eq!(
            Osc99SystemAction::Button(3).to_leg_activation(),
            Osc99LegActivation::Button(3)
        );
    }

    #[test]
    fn close_target_of_a_shown_instance_is_the_daemons_id() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, 7);

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
        state.record_shown(instance(1), None, 7);
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
        state.record_shown(instance(1), None, 7);

        state.record_shown(instance(2), Some(instance(1)), 8);

        assert_eq!(state.close_target(instance(1)), CloseTarget::AlreadyGone);
        assert_eq!(state.close_target(instance(2)), CloseTarget::CallDaemon(8));
    }

    #[test]
    fn close_target_after_the_daemon_closed_the_instance_is_already_gone() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, 7);
        let _ = state.on_closed(7);

        assert_eq!(state.close_target(instance(1)), CloseTarget::AlreadyGone);
    }

    #[test]
    fn close_target_follows_the_instance_after_an_activation() {
        let mut state = DispatcherState::default();
        state.record_shown(instance(1), None, 7);
        let _ = state.on_action(7, "default");

        assert_eq!(
            state.close_target(instance(1)),
            CloseTarget::CallDaemon(7),
            "an activation does not close the notification"
        );
    }

    // ── Signal parsing ──────────────────────────────────────────────────

    /// A signal message from the notification daemon, awaiting its body.
    fn signal(member: &str) -> zbus::Result<zbus::message::Builder<'_>> {
        Message::signal(NOTIFICATIONS_PATH, NOTIFICATIONS_INTERFACE, member)
    }

    #[test]
    fn parse_signal_reads_action_invoked() -> TestResult {
        let message = signal("ActionInvoked")?.build(&(7u32, "2"))?;

        assert_eq!(parse_signal(&message), Some(action_from(None, 7, "2")));
        Ok(())
    }

    #[test]
    fn parse_signal_carries_the_sender() -> TestResult {
        let message = signal("NotificationClosed")?
            .sender(":1.5")?
            .build(&(7u32, 3u32))?;

        assert_eq!(parse_signal(&message), Some(closed_from(Some(":1.5"), 7)));
        Ok(())
    }

    #[test]
    fn parse_signal_reads_notification_closed_whatever_the_reason() -> TestResult {
        let message = signal("NotificationClosed")?.build(&(7u32, 3u32))?;

        assert_eq!(parse_signal(&message), Some(closed_from(None, 7)));
        Ok(())
    }

    #[test]
    fn parse_signal_ignores_other_members() -> TestResult {
        let message = signal("ActivationToken")?.build(&(7u32, "token"))?;

        assert_eq!(parse_signal(&message), None);
        Ok(())
    }

    #[test]
    fn parse_signal_ignores_a_malformed_body() -> TestResult {
        let message = signal("ActionInvoked")?.build(&"not a tuple")?;

        assert_eq!(parse_signal(&message), None);
        Ok(())
    }

    /// A `NameOwnerChanged` signal as the bus driver would send it.
    fn name_owner_changed(sender: &str, name: &str, new_owner: &str) -> zbus::Result<Message> {
        Message::signal(BUS_DRIVER_PATH, BUS_DRIVER_INTERFACE, "NameOwnerChanged")?
            .sender(sender)?
            .build(&(name, "", new_owner))
    }

    #[test]
    fn parse_owner_changed_reads_the_new_owner() -> TestResult {
        let message = name_owner_changed(BUS_DRIVER_NAME, NOTIFICATIONS_SERVICE, ":1.6")?;

        assert_eq!(
            parse_owner_changed(&message),
            Some(owner_changed(Some(":1.6")))
        );
        Ok(())
    }

    #[test]
    fn parse_owner_changed_reads_an_empty_new_owner_as_no_owner() -> TestResult {
        let message = name_owner_changed(BUS_DRIVER_NAME, NOTIFICATIONS_SERVICE, "")?;

        assert_eq!(parse_owner_changed(&message), Some(owner_changed(None)));
        Ok(())
    }

    #[test]
    fn parse_owner_changed_ignores_other_names() -> TestResult {
        let message = name_owner_changed(BUS_DRIVER_NAME, "org.example.Other", ":1.6")?;

        assert_eq!(parse_owner_changed(&message), None);
        Ok(())
    }

    #[test]
    fn parse_owner_changed_ignores_a_sender_that_is_not_the_bus_driver() -> TestResult {
        let forged = name_owner_changed(":1.9", NOTIFICATIONS_SERVICE, ":1.9")?;
        let senderless =
            Message::signal(BUS_DRIVER_PATH, BUS_DRIVER_INTERFACE, "NameOwnerChanged")?
                .build(&(NOTIFICATIONS_SERVICE, "", ":1.9"))?;

        assert_eq!(parse_owner_changed(&forged), None);
        assert_eq!(parse_owner_changed(&senderless), None);
        Ok(())
    }

    #[test]
    fn parse_owner_changed_ignores_other_members_and_malformed_bodies() -> TestResult {
        let other_member = Message::signal(BUS_DRIVER_PATH, BUS_DRIVER_INTERFACE, "NameLost")?
            .sender(BUS_DRIVER_NAME)?
            .build(&(NOTIFICATIONS_SERVICE, "", ":1.6"))?;
        let malformed = Message::signal(BUS_DRIVER_PATH, BUS_DRIVER_INTERFACE, "NameOwnerChanged")?
            .sender(BUS_DRIVER_NAME)?
            .build(&"not a tuple")?;

        assert_eq!(parse_owner_changed(&other_member), None);
        assert_eq!(parse_owner_changed(&malformed), None);
        Ok(())
    }

    #[test]
    fn the_match_rules_build() -> TestResult {
        let rule = owner_changed_rule()?.to_string();

        assert_eq!(
            rule,
            "type='signal',sender='org.freedesktop.DBus',interface='org.freedesktop.DBus',\
             member='NameOwnerChanged',arg0='org.freedesktop.Notifications'"
        );
        assert!(
            notification_signals_rule()?
                .to_string()
                .contains("path='/org/freedesktop/Notifications'")
        );
        Ok(())
    }

    // ── Service over a peer-to-peer connection ──────────────────────────

    /// What the fake daemon saw in one `Notify` call.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedNotify {
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        urgency: Option<u8>,
        /// The key of every hint, sorted.
        hint_keys: Vec<String>,
        category: Option<String>,
        sound_name: Option<String>,
        /// `(width, height, rowstride, has_alpha, bits_per_sample, channels,
        /// data length)` of the `image-data` hint.
        image: Option<RecordedImage>,
        expire_timeout: i32,
        assigned_id: u32,
    }

    /// The `image-data` hint's `(iiibiiay)` structure, summarised.
    type RecordedImage = (i32, i32, i32, bool, i32, i32, usize);

    fn recorded_image(value: &OwnedValue) -> Option<RecordedImage> {
        let structure = Structure::try_from(Value::from(value.try_clone().ok()?)).ok()?;
        let (w, h, stride, alpha, bits, channels, data) =
            <(i32, i32, i32, bool, i32, i32, Vec<u8>)>::try_from(structure).ok()?;
        Some((w, h, stride, alpha, bits, channels, data.len()))
    }

    /// The `Notify` arguments, as the fake daemon deserialises them.
    type NotifyBody = (
        String,
        u32,
        String,
        String,
        String,
        Vec<String>,
        HashMap<String, OwnedValue>,
        i32,
    );

    /// The summary the fake daemon refuses to display (`Notify` errors).
    const REJECTED_SUMMARY: &str = "rejected";
    /// The summary whose `Notify` the fake daemon never answers.
    const UNANSWERED_SUMMARY: &str = "unanswered";
    /// The method timeout of the client in tests that need a call to time out.
    /// The timeout itself is deterministic because the fake never replies. It
    /// is not shorter because the same timeout also bounds the calls the fake
    /// *does* answer, and a loaded machine can delay those by a few hundred
    /// milliseconds.
    const SHORT_METHOD_TIMEOUT: Duration = Duration::from_secs(2);

    /// What the fake daemon saw and how it should behave.
    #[derive(Default)]
    struct DaemonLog {
        /// Every `Notify` it displayed, in call order.
        notifies: Mutex<Vec<RecordedNotify>>,
        /// The id of every `CloseNotification` call, in call order.
        closes: Mutex<Vec<u32>>,
        /// Answer `CloseNotification` with an error instead of closing.
        fail_close: AtomicBool,
        /// Never answer `CloseNotification`.
        ignore_close: AtomicBool,
    }

    /// Serve `org.freedesktop.Notifications` on `daemon` until its connection
    /// closes.
    ///
    /// - `Notify` records the call and returns a fresh, incrementing id (even
    ///   for a replacement), errors for [`REJECTED_SUMMARY`], and never
    ///   answers [`UNANSWERED_SUMMARY`].
    /// - `CloseNotification` records the id, then replies and emits
    ///   `NotificationClosed(id, 3)` as a real daemon does, errors when
    ///   `fail_close` is set, and never answers when `ignore_close` is set.
    /// - `GetCapabilities` answers `capabilities`.
    ///
    /// `messages` must have been created before the client could send its
    /// first call: a message stream only sees messages that arrive after it
    /// exists.
    ///
    /// Hand-written rather than a `#[zbus::interface]`: the macro's expansion
    /// of the eight-argument `Notify` trips pedantic lints that cannot be
    /// silenced without an `allow`.
    fn serve_fake_daemon(
        daemon: &Connection,
        messages: MessageIterator,
        log: &DaemonLog,
        capabilities: &[String],
    ) -> zbus::Result<()> {
        for message in messages {
            let message = message?;
            if message.message_type() != MessageType::MethodCall {
                continue;
            }
            let header = message.header();
            let Some(member) = header.member() else {
                continue;
            };
            match member.as_str() {
                "Notify" => {
                    let (app_name, replaces_id, app_icon, summary, body, actions, hints, timeout) =
                        message.body().deserialize::<NotifyBody>()?;
                    if summary == UNANSWERED_SUMMARY {
                        continue;
                    }
                    if summary == REJECTED_SUMMARY {
                        daemon.reply_error(
                            &header,
                            "org.freedesktop.DBus.Error.Failed",
                            &"rejected",
                        )?;
                        continue;
                    }
                    let mut notifies = log.notifies.lock().unwrap_or_else(PoisonError::into_inner);
                    let assigned_id = u32::try_from(notifies.len()).map_or(u32::MAX, |n| n + 1);
                    let mut hint_keys: Vec<String> = hints.keys().cloned().collect();
                    hint_keys.sort();
                    notifies.push(RecordedNotify {
                        app_name,
                        replaces_id,
                        app_icon,
                        summary,
                        body,
                        actions,
                        urgency: hints
                            .get("urgency")
                            .and_then(|value| u8::try_from(value).ok()),
                        hint_keys,
                        category: hints
                            .get("category")
                            .and_then(|value| String::try_from(value.try_clone().ok()?).ok()),
                        sound_name: hints
                            .get("sound-name")
                            .and_then(|value| String::try_from(value.try_clone().ok()?).ok()),
                        image: hints.get("image-data").and_then(recorded_image),
                        expire_timeout: timeout,
                        assigned_id,
                    });
                    drop(notifies);
                    daemon.reply(&header, &assigned_id)?;
                }
                "GetCapabilities" => daemon.reply(&header, &capabilities)?,
                "CloseNotification" => {
                    let dbus_id = message.body().deserialize::<u32>()?;
                    log.closes
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(dbus_id);
                    if log.ignore_close.load(Ordering::SeqCst) {
                        continue;
                    }
                    if log.fail_close.load(Ordering::SeqCst) {
                        daemon.reply_error(
                            &header,
                            "org.freedesktop.DBus.Error.Failed",
                            &"cannot close",
                        )?;
                    } else {
                        daemon.reply(&header, &())?;
                        daemon.emit_signal(
                            None::<&str>,
                            NOTIFICATIONS_PATH,
                            NOTIFICATIONS_INTERFACE,
                            "NotificationClosed",
                            &(dbus_id, 3u32),
                        )?;
                    }
                }
                "GetServerInformation" => {
                    daemon.reply(&header, &("fake", "freminal-tests", "0", "1.2"))?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// A connected client and fake-daemon pair over a socket pair, plus the
    /// daemon's record of the calls it served.
    struct Rig {
        client: Connection,
        daemon: Connection,
        log: Arc<DaemonLog>,
    }

    fn rig(advertised: &[&str]) -> Result<Rig, Box<dyn Error>> {
        rig_with_timeout(advertised, None)
    }

    /// A rig whose client gives up on a method call after `method_timeout`.
    fn rig_with_timeout(
        advertised: &[&str],
        method_timeout: Option<Duration>,
    ) -> Result<Rig, Box<dyn Error>> {
        let (server_stream, client_stream) = UnixStream::pair()?;
        let guid = zbus::Guid::generate();

        // The handshake needs both ends running at once.
        let server = std::thread::spawn(move || {
            zbus::blocking::connection::Builder::async_io_unix_stream(server_stream)
                .server(guid)?
                .p2p()
                .build()
        });
        let mut client_builder =
            zbus::blocking::connection::Builder::async_io_unix_stream(client_stream).p2p();
        if let Some(timeout) = method_timeout {
            client_builder = client_builder.method_timeout(timeout);
        }
        let client = client_builder.build()?;
        let daemon = server
            .join()
            .map_err(|_| "the fake daemon thread panicked")??;
        // Subscribe before anything can call: the client only starts talking
        // once the service is spawned, after this function returns, and a
        // stream created later (say, on the daemon thread) would miss a call
        // that arrived first, leaving the client waiting forever.
        let messages = MessageIterator::from(&daemon);

        let log = Arc::new(DaemonLog::default());
        let capabilities = strings(advertised);
        let daemon_side = daemon.clone();
        let record = Arc::clone(&log);
        std::thread::Builder::new()
            .name("fake-notification-daemon".to_owned())
            .spawn(move || {
                if let Err(e) = serve_fake_daemon(&daemon_side, messages, &record, &capabilities) {
                    tracing::debug!("fake notification daemon stopped: {e}");
                }
            })?;
        Ok(Rig {
            client,
            daemon,
            log,
        })
    }

    impl Rig {
        fn recorded(&self) -> Vec<RecordedNotify> {
            self.log
                .notifies
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        /// The id of every `CloseNotification` call the daemon served.
        fn closes(&self) -> Vec<u32> {
            self.log
                .closes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn emit_action(&self, dbus_id: u32, key: &str) -> zbus::Result<()> {
            self.daemon.emit_signal(
                None::<&str>,
                NOTIFICATIONS_PATH,
                NOTIFICATIONS_INTERFACE,
                "ActionInvoked",
                &(dbus_id, key),
            )
        }

        fn emit_closed(&self, dbus_id: u32, reason: u32) -> zbus::Result<()> {
            self.daemon.emit_signal(
                None::<&str>,
                NOTIFICATIONS_PATH,
                NOTIFICATIONS_INTERFACE,
                "NotificationClosed",
                &(dbus_id, reason),
            )
        }
    }

    /// The next event, failing the test if none arrives in [`EVENT_TIMEOUT`].
    fn next_event(handle: &Osc99DbusHandle) -> Result<Osc99DbusEvent, Box<dyn Error>> {
        Ok(handle.events.recv_timeout(EVENT_TIMEOUT)?)
    }

    /// A waker that counts how often it was called.
    fn counting_waker() -> (Box<dyn Fn() + Send>, Receiver<()>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (
            Box::new(move || {
                let _ = tx.send(());
            }),
            rx,
        )
    }

    #[test]
    fn show_calls_notify_then_activation_and_close_signals_become_events() -> TestResult {
        let rig = rig(&["body", "actions"])?;
        let (waker, woken) = counting_waker();
        let mut handle = Osc99DbusService::spawn_with_connection(rig.client.clone(), waker);

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(Osc99DaemonCapabilities {
                body: Osc99DaemonSupport::Supported,
                actions: Osc99DaemonSupport::Supported,
                body_markup: Osc99DaemonSupport::Unsupported,
                sound: Osc99DaemonSupport::Unsupported,
            })
        );

        assert_eq!(
            handle.show(
                instance(1),
                None,
                &Notification99Data {
                    body: Some("Body".to_owned()),
                    urgency: NotificationUrgency::Critical,
                    expiry: Osc99Expiry::After(NonZeroU32::new(5000).ok_or("zero")?),
                    ..with_buttons(&["A", "B"])
                },
            ),
            Osc99SystemDispatch::Queued
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(1)
            }
        );

        let recorded = rig.recorded();
        assert_eq!(
            recorded,
            [RecordedNotify {
                app_name: "freminal".to_owned(),
                replaces_id: 0,
                app_icon: "freminal".to_owned(),
                summary: "Title".to_owned(),
                body: "Body".to_owned(),
                actions: strings(&["default", "", "1", "A", "2", "B"]),
                urgency: Some(2),
                hint_keys: strings(&["urgency"]),
                category: None,
                sound_name: None,
                image: None,
                expire_timeout: 5000,
                assigned_id: 1,
            }]
        );
        let dbus_id = recorded[0].assigned_id;

        rig.emit_action(dbus_id, "2")?;
        rig.emit_closed(dbus_id, 2)?;
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Activated {
                instance: instance(1),
                action: Osc99SystemAction::Button(2),
            }
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );

        // Every event was followed by a wake.
        for _ in 0..4 {
            woken.recv_timeout(EVENT_TIMEOUT)?;
        }
        Ok(())
    }

    #[test]
    fn presentation_hints_arrive_over_the_wire() -> TestResult {
        let rig = rig(&["body", "actions", "sound"])?;
        let (waker, _woken) = counting_waker();
        let mut handle = Osc99DbusService::spawn_with_connection(rig.client.clone(), waker);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));

        assert_eq!(
            handle.show(
                instance(1),
                None,
                &Notification99Data {
                    icon_data: Some(png(4, 3)?),
                    notification_types: strings(&["email.arrived"]),
                    sound: Osc99Sound::Named("info".to_owned()),
                    ..notification()
                },
            ),
            Osc99SystemDispatch::Queued
        );
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        let recorded = rig.recorded();
        assert_eq!(recorded.len(), 1);
        let notify = &recorded[0];
        assert_eq!(
            notify.hint_keys,
            strings(&["category", "image-data", "sound-name", "urgency"])
        );
        assert_eq!(notify.category.as_deref(), Some("email.arrived"));
        assert_eq!(notify.sound_name.as_deref(), Some("dialog-information"));
        assert_eq!(notify.image, Some((4, 3, 16, true, 8, 4, 4 * 3 * 4)));
        assert_eq!(notify.app_icon, "");
        Ok(())
    }

    #[test]
    fn a_silent_notification_sends_suppress_sound() -> TestResult {
        let rig = rig(&["body", "actions"])?;
        let (waker, _woken) = counting_waker();
        let mut handle = Osc99DbusService::spawn_with_connection(rig.client.clone(), waker);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));

        assert_eq!(
            handle.show(
                instance(1),
                None,
                &Notification99Data {
                    sound: Osc99Sound::Silent,
                    ..notification()
                },
            ),
            Osc99SystemDispatch::Queued
        );
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        assert_eq!(
            rig.recorded()[0].hint_keys,
            strings(&["suppress-sound", "urgency"])
        );
        Ok(())
    }

    #[test]
    fn a_daemon_without_actions_gets_no_action_list() -> TestResult {
        let rig = rig(&["body"])?;
        let (waker, _woken) = counting_waker();
        let mut handle = Osc99DbusService::spawn_with_connection(rig.client.clone(), waker);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));

        assert_eq!(
            handle.show(instance(1), None, &with_buttons(&["A"])),
            Osc99SystemDispatch::Queued
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(1)
            }
        );

        assert_eq!(rig.recorded()[0].actions, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn signals_for_unknown_ids_and_other_members_produce_no_event() -> TestResult {
        let rig = rig(&["body", "actions"])?;
        let (waker, _woken) = counting_waker();
        let mut handle = Osc99DbusService::spawn_with_connection(rig.client.clone(), waker);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));
        assert_eq!(
            handle.show(instance(1), None, &notification()),
            Osc99SystemDispatch::Queued
        );
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        // Signals for ids nobody shown, and an unrelated member, then the
        // real close: the close is the first (and only) event, so the
        // earlier signals were ignored, in order, by the same dispatcher.
        rig.emit_action(999, "default")?;
        rig.emit_closed(999, 1)?;
        rig.daemon.emit_signal(
            None::<&str>,
            NOTIFICATIONS_PATH,
            NOTIFICATIONS_INTERFACE,
            "ActivationToken",
            &(1u32, "token"),
        )?;
        rig.emit_closed(rig.recorded()[0].assigned_id, 1)?;

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert!(handle.try_events().next().is_none());
        Ok(())
    }

    /// Start the service against a fresh rig and consume the `Capabilities`
    /// event, so a test begins with an empty event queue.
    fn started(
        advertised: &[&str],
    ) -> Result<(Rig, Osc99DbusHandle, Receiver<()>), Box<dyn Error>> {
        started_with(rig(advertised)?)
    }

    /// [`started`] for an existing rig.
    fn started_with(rig: Rig) -> Result<(Rig, Osc99DbusHandle, Receiver<()>), Box<dyn Error>> {
        let (waker, woken) = counting_waker();
        let handle = Osc99DbusService::spawn_with_connection(rig.client.clone(), waker);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));
        Ok((rig, handle, woken))
    }

    #[test]
    fn close_calls_close_notification_once_and_the_daemons_signal_becomes_closed() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        assert_eq!(
            handle.show(instance(1), None, &notification()),
            Osc99SystemDispatch::Queued
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(1)
            }
        );
        let dbus_id = rig.recorded()[0].assigned_id;

        assert_eq!(handle.close(instance(1)), Osc99SystemDispatch::Queued);

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(rig.closes(), [dbus_id], "exactly one call, with the id");
        assert!(
            handle.try_events().next().is_none(),
            "one Closed: the signal, not also a local one"
        );
        Ok(())
    }

    #[test]
    fn an_update_replaces_the_old_id_and_the_old_ids_close_produces_no_event() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        let _ = handle.show(instance(2), Some(instance(1)), &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(2)
            }
        );

        let recorded = rig.recorded();
        assert_eq!(recorded[1].replaces_id, recorded[0].assigned_id);
        assert_ne!(
            recorded[1].assigned_id, recorded[0].assigned_id,
            "the fake daemon hands out a fresh id, so the two are distinguishable"
        );

        // The old id closes first, then the new one: the new one's `Closed`
        // is the first event, so the old id's was ignored by the same
        // dispatcher, in order.
        rig.emit_closed(recorded[0].assigned_id, 2)?;
        rig.emit_closed(recorded[1].assigned_id, 2)?;
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(2)
            }
        );
        assert!(handle.try_events().next().is_none());
        Ok(())
    }

    #[test]
    fn closing_a_replaced_instance_reports_closed_without_calling_the_daemon() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));
        let _ = handle.show(instance(2), Some(instance(1)), &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        let _ = handle.close(instance(1));

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(rig.closes(), Vec::<u32>::new());
        Ok(())
    }

    #[test]
    fn close_after_a_failed_show_emits_closed_at_once_without_calling_the_daemon() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        let _ = handle.show(
            instance(1),
            None,
            &Notification99Data {
                title: REJECTED_SUMMARY.to_owned(),
                ..notification()
            },
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::ShowFailed {
                instance: instance(1)
            }
        );

        assert_eq!(handle.close(instance(1)), Osc99SystemDispatch::Queued);
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );

        // A later round trip proves no `CloseNotification` was in flight: the
        // dispatcher handles commands in order.
        let _ = handle.show(instance(2), None, &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(2)
            }
        );
        assert_eq!(rig.closes(), Vec::<u32>::new());
        Ok(())
    }

    #[test]
    fn close_of_a_never_shown_instance_emits_closed_at_once() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body"])?;

        let _ = handle.close(instance(9));

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(9)
            }
        );
        assert_eq!(rig.closes(), Vec::<u32>::new());
        Ok(())
    }

    #[test]
    fn a_failing_close_notification_call_falls_back_to_closed() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        rig.log.fail_close.store(true, Ordering::SeqCst);
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));
        let dbus_id = rig.recorded()[0].assigned_id;

        let _ = handle.close(instance(1));

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(rig.closes(), [dbus_id], "the call was attempted once");

        // The mappings were dropped, so a late signal for the id is ignored.
        let _ = handle.show(instance(2), None, &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(2)
            }
        );
        rig.emit_closed(dbus_id, 1)?;
        rig.emit_closed(rig.recorded()[1].assigned_id, 1)?;
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(2)
            }
        );
        assert!(handle.try_events().next().is_none());
        Ok(())
    }

    #[test]
    fn a_notify_the_daemon_never_answers_times_out_into_show_failed() -> TestResult {
        let (rig, mut handle, _woken) =
            started_with(rig_with_timeout(&["body"], Some(SHORT_METHOD_TIMEOUT))?)?;

        let _ = handle.show(
            instance(1),
            None,
            &Notification99Data {
                title: UNANSWERED_SUMMARY.to_owned(),
                ..notification()
            },
        );

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::ShowFailed {
                instance: instance(1)
            }
        );
        // The dispatcher survived the timeout and still serves commands.
        let _ = handle.show(instance(2), None, &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(2)
            }
        );
        assert_eq!(rig.recorded().len(), 1, "only the answered Notify counts");
        Ok(())
    }

    #[test]
    fn a_close_notification_the_daemon_never_answers_times_out_into_closed() -> TestResult {
        let (rig, mut handle, _woken) =
            started_with(rig_with_timeout(&["body"], Some(SHORT_METHOD_TIMEOUT))?)?;
        rig.log.ignore_close.store(true, Ordering::SeqCst);
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));
        let dbus_id = rig.recorded()[0].assigned_id;

        let _ = handle.close(instance(1));

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(rig.closes(), [dbus_id], "the call was attempted once");
        Ok(())
    }

    #[test]
    fn an_unreachable_bus_reports_unavailable_once_and_fails_every_show() -> TestResult {
        let (waker, woken) = counting_waker();
        let mut handle = start(
            || Err(zbus::Error::Failure("no bus".to_owned())),
            waker,
            Box::new(NoIconLookup),
        );

        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        assert_eq!(
            handle.show(instance(1), None, &notification()),
            Osc99SystemDispatch::Queued,
            "queued; the dispatcher answers ShowFailed itself"
        );
        assert_eq!(
            handle.show(instance(2), None, &notification()),
            Osc99SystemDispatch::Queued
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::ShowFailed {
                instance: instance(1)
            }
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::ShowFailed {
                instance: instance(2)
            }
        );
        assert_eq!(
            handle.close(instance(3)),
            Osc99SystemDispatch::Queued,
            "queued; the dispatcher answers Closed itself"
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(3)
            }
        );
        for _ in 0..4 {
            woken.recv_timeout(EVENT_TIMEOUT)?;
        }
        Ok(())
    }

    #[test]
    fn a_request_after_the_dispatcher_is_gone_returns_failed_and_queues_no_event() -> TestResult {
        let (waker, _woken) = counting_waker();
        let mut handle = start(
            || Err(zbus::Error::Failure("no bus".to_owned())),
            waker,
            Box::new(NoIconLookup),
        );
        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        // Replace the command sender with one whose receiver is already gone,
        // as after a dispatcher exit.
        let (dead_tx, dead_rx) = crossbeam_channel::unbounded();
        drop(dead_rx);
        handle.commands = dead_tx;

        assert_eq!(
            handle.show(instance(5), None, &notification()),
            Osc99SystemDispatch::Failed
        );
        assert_eq!(
            handle.try_events().next(),
            None,
            "no local event: the caller ends the leg"
        );
        assert_eq!(
            handle.close(instance(5)),
            Osc99SystemDispatch::Failed,
            "a close fails the same way"
        );
        assert_eq!(handle.try_events().next(), None);
        Ok(())
    }
}
