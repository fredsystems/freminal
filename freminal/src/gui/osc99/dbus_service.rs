// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The Linux/BSD OSC 99 desktop-notification service (Task 138.12).
//!
//! # Threads
//!
//! One *dispatcher* thread (`freminal-osc99-dbus`) owns one D-Bus connection
//! and calls `org.freedesktop.Notifications` on it (`GetCapabilities` at start
//! and after every owner change, `Notify` per show, `CloseNotification` per
//! close). Up to two *signal pump* threads iterate one match rule each over
//! that connection and forward what they see **into the dispatcher's input
//! channel**, not to the GUI:
//!
//! - `freminal-osc99-signals` forwards `ActionInvoked` / `NotificationClosed`;
//! - `freminal-osc99-owner` (bus connections only) forwards `NameOwnerChanged`
//!   for the daemon's name.
//!
//! The dispatcher owns the D-Bus-id to instance map, so a signal that arrives
//! while a `Notify` call is in flight is only processed after the call has
//! recorded its id. That removes the ordering race between the threads.
//! `GetNameOwner` and `GetCapabilities` run on a short-lived helper thread
//! (`freminal-osc99-call`) so that they can carry the `NO_AUTO_START` flag
//! *and* be bounded by [`OSC99_DBUS_METHOD_TIMEOUT`]; see [`call_with_timeout`].
//!
//! The GUI talks to the service through an [`Osc99DbusHandle`]: commands go in
//! over one crossbeam channel and [`Osc99DbusEvent`]s come back over another,
//! each event followed by a repaint wake so an idle window still drains it.
//!
//! # Shutdown
//!
//! Dropping the handle closes the command channel, and the dispatcher thread
//! returns the next time it looks at it: at once when idle, or after the call
//! in flight (at most [`OSC99_DBUS_METHOD_TIMEOUT`]). Returning drops the
//! event sender and the signal receiver. A signal pump is blocked reading the
//! bus and notices only when it next has something to forward: it exits when
//! that send fails, or when the connection ends. Until then it is idle and
//! holds a clone of the connection. A helper call thread exits when its call
//! is answered or the connection ends. None of them does any work after the
//! dispatcher is gone, and all of them end with the process.
//!
//! # Owner tracking
//!
//! Signals are filtered by sender (Task 138.C2). On a bus connection the
//! dispatcher resolves the unique name that owns `org.freedesktop.Notifications`
//! (`GetNameOwner`) and ignores `ActionInvoked` / `NotificationClosed` from any
//! other sender, so a session-bus peer cannot forge them. The owner pump
//! follows `NameOwnerChanged` so a daemon restart re-targets the filter.
//! zbus's client-side match cannot resolve a well-known sender, so the sender
//! is compared in the dispatcher rather than put in the match rule. A
//! peer-to-peer connection has no bus, no owner and no sender to check, so it
//! is [`Osc99SignalSender::Unfiltered`]. Which of the two applies is the
//! [`OwnerTracking`] mode, chosen by whoever supplies the connection.
//!
//! Each mapping records the unique name that answered its `Notify`. When the
//! owner changes, only instances recorded under a *different* owner are closed
//! (their daemon is gone and will never send the signals that would end
//! them). An instance with no recorded owner (a peer-to-peer reply has no
//! sender) is never closed by an owner change. If `GetNameOwner` fails or finds
//! nobody, the owner is unknown and the sender of the next successful `Notify`
//! reply is adopted as the owner, which also covers a daemon that D-Bus
//! activated for that very call. Every change to a new owner fetches
//! `GetCapabilities` again.
//!
//! # Failure handling
//!
//! - **Pump loss.** If either pump ends, every mapped instance is reported
//!   closed, [`Osc99DbusEvent::Unavailable`] is emitted, and the service
//!   answers every later command as if the bus were down.
//! - **Hung daemon.** A `Notify`, `CloseNotification` or `GetCapabilities`
//!   that times out puts the daemon in [`Osc99DaemonHealth::Unresponsive`]:
//!   for [`OSC99_DAEMON_COOLDOWN`] shows fail and closes complete without
//!   calling it, then the next command tries again.
//! - **No daemon at start.** `GetNameOwner` and `GetCapabilities` never
//!   D-Bus-activate a daemon (`NO_AUTO_START`); the first `Notify` does. A
//!   missing daemon is a debug log, not a warning. After a failed connect the
//!   next show reconnects, at most once per [`OSC99_RECONNECT_INTERVAL`].
//!
//! Every state transition lives in the pure [`DispatcherState`], which has no
//! bus and no threads and is unit-tested directly.
//!
//! Update in place sends the replaced instance's D-Bus id as `replaces_id`.
//! If that `Notify` fails, a best-effort `CloseNotification` for the old id
//! follows, so the old notification does not stay on screen unmanaged. Close
//! calls `CloseNotification` and lets the daemon's `NotificationClosed`
//! signal produce the [`Osc99DbusEvent::Closed`]; an instance with no D-Bus id
//! (its show failed, or it was replaced) is reported closed at once.
//!
//! Presentation (icons, sounds, category, markup) is Task 138.14.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, select};
use freminal_common::buffer_states::osc99_notification::{
    Notification99Data, NotificationUrgency, Osc99Expiry,
};
use freminal_common::send_or_log;
use freminal_windowing::{RepaintProxy, WindowId};
use zbus::MatchRule;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::message::{Message, Type as MessageType};
use zbus::proxy::MethodFlags;
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
/// The error the bus driver answers `GetNameOwner` with when nobody owns the
/// name.
const ERROR_NAME_HAS_NO_OWNER: &str = "org.freedesktop.DBus.Error.NameHasNoOwner";
/// The error the bus answers a `NO_AUTO_START` call to a name that nobody owns
/// and that no service file can start.
const ERROR_SERVICE_UNKNOWN: &str = "org.freedesktop.DBus.Error.ServiceUnknown";
/// How long the dispatcher leaves a daemon alone after one of its calls timed
/// out. Every show fails and every close completes without calling it until
/// this has passed; then the next command tries once more.
const OSC99_DAEMON_COOLDOWN: Duration = Duration::from_secs(30);
/// The least time between two attempts to reconnect after a failed connect.
const OSC99_RECONNECT_INTERVAL: Duration = Duration::from_secs(30);
/// The `app_name` sent when the notification names no application (`f=`).
const DEFAULT_APP_NAME: &str = "freminal";
/// How long a D-Bus method call waits for its reply before it fails. This is
/// the libdbus default reply timeout. zbus's blocking calls have no timeout of
/// their own, so without it a daemon that never answers would block the
/// dispatcher thread forever and silently end the Linux leg. The bus
/// connection carries it as its `method_timeout` (which bounds `Notify` and
/// `CloseNotification`), and [`call_with_timeout`] applies it to the calls zbus
/// cannot bound itself. A timed-out `Notify` becomes
/// [`Osc99DbusEvent::ShowFailed`], a timed-out `CloseNotification` falls back
/// to [`Osc99DbusEvent::Closed`], and any timeout starts the
/// [`OSC99_DAEMON_COOLDOWN`].
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

/// Which of the two signal pumps a [`PumpSignal::Ended`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PumpKind {
    /// `freminal-osc99-signals`: `ActionInvoked` / `NotificationClosed`.
    Notifications,
    /// `freminal-osc99-owner`: `NameOwnerChanged`.
    OwnerChanges,
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
    /// The pump of this kind has ended (the connection closed or the stream
    /// failed): its signals will not arrive any more. Sent by the pump itself
    /// just before its thread exits.
    Ended(PumpKind),
}

/// Whose notification signals the dispatcher believes.
///
/// There is deliberately no `Default`: the one value that believes everyone
/// ([`Self::Unfiltered`]) must be chosen, never fallen into.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Osc99SignalSender {
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
enum OwnerTracking {
    /// A message bus: ask the bus driver (`GetNameOwner`), follow
    /// `NameOwnerChanged`, and believe only the owner's signals.
    Bus,
    /// A peer-to-peer connection (tests): there is no bus driver, so there is
    /// no owner to learn and every signal is believed.
    Peer,
}

impl OwnerTracking {
    /// The mode that fits `connection`.
    fn of(connection: &Connection) -> Self {
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
enum DaemonGate {
    /// Call the daemon.
    Call,
    /// The daemon is in its cooldown: do not call it, fail the command.
    Skip,
}

/// What a successful `Notify` reply says: the daemon's id for the
/// notification, and the unique name that sent the reply.
///
/// The sender is `None` over a peer-to-peer connection, where nothing stamps
/// replies. On a bus it is the daemon that answered, which is how an
/// instance's owner is known.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NotifyReply {
    dbus_id: u32,
    sender: Option<String>,
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
struct DispatcherState {
    /// Whose notification signals are believed.
    daemon_owner: Osc99SignalSender,
    /// The daemon's id (and answering owner) for each displayed instance.
    by_instance: HashMap<Osc99InstanceId, Mapping>,
    /// The instance behind each of the daemon's ids.
    by_dbus_id: HashMap<u32, Osc99InstanceId>,
    /// What the daemon can do; assumed until it answers.
    capabilities: Osc99DaemonCapabilities,
    /// The owner `GetCapabilities` was last attempted against, so a new owner
    /// is known to need a fresh answer.
    capabilities_asked_of: Option<String>,
    /// Whether the daemon answers.
    health: Osc99DaemonHealth,
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
            capabilities_asked_of: None,
            health: Osc99DaemonHealth::Healthy,
            icon_lookup,
        }
    }

    /// Start a new connection's session: `daemon_owner` is who owns the name
    /// now. Nothing displayed through an earlier connection is still tracked
    /// (a connection only ends by being lost, which already closed it), and a
    /// fresh connection has no history of timeouts.
    fn begin_session(&mut self, daemon_owner: Osc99SignalSender) {
        self.daemon_owner = daemon_owner;
        self.by_instance.clear();
        self.by_dbus_id.clear();
        self.capabilities_asked_of = None;
        self.health = Osc99DaemonHealth::Healthy;
    }

    /// Whether the daemon may be called at `now`. After a timeout the answer
    /// is [`DaemonGate::Skip`] until [`OSC99_DAEMON_COOLDOWN`] has passed; the
    /// first call after that is a probe, and its outcome (see
    /// [`Self::note_call`]) decides whether the daemon is trusted again.
    fn daemon_gate(&self, now: Instant) -> DaemonGate {
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
    fn note_call<T>(&mut self, outcome: &zbus::Result<T>, now: Instant) {
        self.health = match outcome {
            Err(e) if is_method_timeout(e) => Osc99DaemonHealth::Unresponsive { since: now },
            Ok(_) | Err(_) => Osc99DaemonHealth::Healthy,
        };
    }

    /// Whether the owner is one `GetCapabilities` has not been tried against.
    /// A state without a known owner never is: with no owner there is no one
    /// to ask, and a peer-to-peer connection is asked once, at the start.
    fn capabilities_stale(&self) -> bool {
        self.daemon_owner
            .owner_name()
            .is_some_and(|owner| self.capabilities_asked_of.as_deref() != Some(owner))
    }

    /// Note that `GetCapabilities` was attempted against the current owner.
    fn note_capabilities_asked(&mut self) {
        self.capabilities_asked_of = self.daemon_owner.owner_name().map(str::to_owned);
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
    fn dbus_id_of(&self, instance: Osc99InstanceId) -> Option<u32> {
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
    fn record_shown(
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
    fn forget_instance(&mut self, instance: Osc99InstanceId) {
        if let Some(mapping) = self.by_instance.remove(&instance) {
            self.by_dbus_id.remove(&mapping.dbus_id);
        }
    }

    /// What closing `instance` calls for: the daemon's id when it displays
    /// the instance, otherwise [`CloseTarget::AlreadyGone`].
    fn close_target(&self, instance: Osc99InstanceId) -> CloseTarget {
        self.dbus_id_of(instance)
            .map_or(CloseTarget::AlreadyGone, CloseTarget::CallDaemon)
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
    fn on_signals_lost(&mut self) -> Vec<Osc99DbusEvent> {
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
/// Dropping it stops the dispatcher; see the module docs for when each thread
/// exits.
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
    /// A signal pump thread could not be spawned.
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
    /// once, a single warning is logged, and every show answers
    /// [`Osc99DbusEvent::ShowFailed`] and every close
    /// [`Osc99DbusEvent::Closed`]. The next show after
    /// [`OSC99_RECONNECT_INTERVAL`] tries to connect again.
    #[must_use]
    pub fn spawn(waker: Box<dyn Fn() + Send>) -> Osc99DbusHandle {
        start(
            connect_session,
            waker,
            Box::new(XdgIconLookup::from_env()),
            OSC99_RECONNECT_INTERVAL,
        )
    }

    /// Start the service on an existing connection (a peer-to-peer one, in
    /// tests), tracking the daemon's owner as `owner_tracking` says.
    #[cfg(test)]
    fn spawn_with_connection(
        connection: Connection,
        owner_tracking: OwnerTracking,
        waker: Box<dyn Fn() + Send>,
    ) -> Osc99DbusHandle {
        start(
            move || {
                Ok(Link {
                    connection: connection.clone(),
                    owner_tracking,
                })
            },
            waker,
            Box::new(NoIconLookup),
            OSC99_RECONNECT_INTERVAL,
        )
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
        Self::new(Osc99SignalSender::Unfiltered, Box::new(NoIconLookup))
    }
}

/// A connection and how the daemon's owner is tracked on it.
struct Link {
    connection: Connection,
    owner_tracking: OwnerTracking,
}

/// Connect to the session bus with [`OSC99_DBUS_METHOD_TIMEOUT`] on every
/// method call.
fn connect_session() -> zbus::Result<Link> {
    let connection = zbus::blocking::connection::Builder::session()?
        .method_timeout(OSC99_DBUS_METHOD_TIMEOUT)
        .build()?;
    Ok(Link {
        owner_tracking: OwnerTracking::of(&connection),
        connection,
    })
}

/// Start the dispatcher thread, which obtains its connection from `connect`
/// (again, at most once per `reconnect_interval`, after it fails) and resolves
/// icons with `icon_lookup` (scanning lazily, on that thread).
fn start<C>(
    connect: C,
    waker: Box<dyn Fn() + Send>,
    icon_lookup: Box<dyn Osc99IconLookup + Send>,
    reconnect_interval: Duration,
) -> Osc99DbusHandle
where
    C: FnMut() -> zbus::Result<Link> + Send + 'static,
{
    let (command_tx, command_rx) = crossbeam_channel::unbounded();
    let (event_tx, event_rx) = crossbeam_channel::unbounded();
    let sink = EventSink {
        events: event_tx.clone(),
        waker,
    };

    let spawned = std::thread::Builder::new()
        .name("freminal-osc99-dbus".to_owned())
        .spawn(move || {
            run_dispatcher(connect, &command_rx, &sink, icon_lookup, reconnect_interval);
        });
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

/// Whether a failed connect may be retried yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reconnect {
    /// Try to connect now.
    Now,
    /// Too soon after the last attempt.
    Later,
}

/// Rate-limits reconnect attempts after a failed connect.
struct ReconnectGate {
    /// The least time between two attempts.
    interval: Duration,
    /// When the last attempt was made.
    last_attempt: Instant,
}

impl ReconnectGate {
    /// A gate whose last attempt was at `last_attempt`.
    const fn new(interval: Duration, last_attempt: Instant) -> Self {
        Self {
            interval,
            last_attempt,
        }
    }

    /// Whether an attempt may be made at `now`. When it may, the attempt is
    /// recorded, so the next one waits a full interval.
    fn decide(&mut self, now: Instant) -> Reconnect {
        if now.saturating_duration_since(self.last_attempt) >= self.interval {
            self.last_attempt = now;
            Reconnect::Now
        } else {
            Reconnect::Later
        }
    }
}

/// A live connection: the dispatcher's calls go out over `connection`, and the
/// pumps' signals come back on `signals`.
struct Session {
    connection: Connection,
    owner_tracking: OwnerTracking,
    signals: Receiver<PumpSignal>,
}

/// How [`serve`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServeEnd {
    /// The GUI dropped its command sender.
    GuiGone,
    /// A signal pump ended: nothing can be tracked any more.
    SignalsLost,
}

/// The dispatcher thread: connect, then serve commands until the GUI drops its
/// command sender.
fn run_dispatcher<C>(
    mut connect: C,
    commands: &Receiver<DbusCommand>,
    sink: &EventSink,
    icon_lookup: Box<dyn Osc99IconLookup + Send>,
    reconnect_interval: Duration,
) where
    C: FnMut() -> zbus::Result<Link>,
{
    let mut state = DispatcherState::new(Osc99SignalSender::NoOwner, icon_lookup);
    let (session, first) = match connect_and_subscribe(&mut connect) {
        Ok(session) => (session, None),
        Err(e) => {
            tracing::warn!("OSC 99: system notifications are unavailable: {e}");
            sink.emit(Osc99DbusEvent::Unavailable);
            match reconnect(&mut connect, commands, sink, reconnect_interval) {
                Some((session, command)) => (session, Some(command)),
                None => return,
            }
        }
    };
    match serve(session, &mut state, commands, sink, first) {
        ServeEnd::GuiGone => {}
        // The dead pump's connection is not trusted again: answer every later
        // command as if the bus were down.
        ServeEnd::SignalsLost => fail_every_command(commands, sink),
    }
}

/// After a failed connect: fail every command until a show comes in that is
/// due a new attempt, and make the attempt. Returns the new session together
/// with the show that asked for it, or `None` when the GUI is gone.
///
/// A failed attempt is not reported again ([`Osc99DbusEvent::Unavailable`]
/// was sent when the service first failed): the show that triggered it is
/// answered with `ShowFailed` and the wait starts over.
fn reconnect<C>(
    connect: &mut C,
    commands: &Receiver<DbusCommand>,
    sink: &EventSink,
    interval: Duration,
) -> Option<(Session, DbusCommand)>
where
    C: FnMut() -> zbus::Result<Link>,
{
    let mut gate = ReconnectGate::new(interval, Instant::now());
    loop {
        let command = fail_until_reconnect_due(commands, sink, &mut gate)?;
        match connect_and_subscribe(connect) {
            Ok(session) => return Some((session, command)),
            Err(e) => {
                tracing::debug!("OSC 99: reconnecting to the notification service failed: {e}");
                report_unserved(sink, &command);
            }
        }
    }
}

/// Answer commands as [`fail_every_command`] does until a `Show` arrives that
/// `gate` allows a reconnect for, and return it. `None` once the GUI is gone.
fn fail_until_reconnect_due(
    commands: &Receiver<DbusCommand>,
    sink: &EventSink,
    gate: &mut ReconnectGate,
) -> Option<DbusCommand> {
    for command in commands {
        let due = matches!(command, DbusCommand::Show { .. })
            && gate.decide(Instant::now()) == Reconnect::Now;
        if due {
            return Some(command);
        }
        report_unserved(sink, &command);
    }
    None
}

/// Connect, subscribe to the notification signals, and start the pump(s).
///
/// The subscriptions are made here, before any pump thread starts and before
/// the first `Notify` or `GetNameOwner`, so no signal can be missed. With
/// [`OwnerTracking::Bus`] there is a second subscription, to `NameOwnerChanged`
/// for the daemon's name, with its own pump: a blocking iterator reads one
/// stream, and match rules cannot be OR-ed, so two streams need two threads.
/// Both pumps feed the one signal channel. [`OwnerTracking::Peer`] has no bus
/// driver to ask, so it gets only the first.
///
/// If a pump thread cannot be spawned, the connection is closed before the
/// error is returned. Nothing will serve it, and a pump that did start would
/// otherwise stay blocked on its stream (deaf, but alive and holding the
/// connection) until its next message, which on a quiet bus may never come.
/// Closing ends the stream, so that pump exits now; the signal receiver is
/// dropped with the error as well, so the `Ended` it sends goes nowhere.
fn connect_and_subscribe<C>(connect: &mut C) -> Result<Session, StartError>
where
    C: FnMut() -> zbus::Result<Link>,
{
    let Link {
        connection,
        owner_tracking,
    } = connect()?;
    let notifications =
        MessageIterator::for_match_rule(notification_signals_rule()?, &connection, None)?;
    let owner_changes = match owner_tracking {
        OwnerTracking::Bus => Some(MessageIterator::for_match_rule(
            owner_changed_rule()?,
            &connection,
            None,
        )?),
        OwnerTracking::Peer => None,
    };

    let (signal_tx, signals) = crossbeam_channel::unbounded();
    let spawned = owner_changes
        .map_or(Ok(()), |messages| {
            spawn_pump(
                "freminal-osc99-owner",
                PumpKind::OwnerChanges,
                messages,
                signal_tx.clone(),
                parse_owner_changed,
            )
        })
        .and_then(|()| {
            spawn_pump(
                "freminal-osc99-signals",
                PumpKind::Notifications,
                notifications,
                signal_tx,
                parse_signal,
            )
        });
    if let Err(e) = spawned {
        if let Err(close_error) = connection.close() {
            tracing::debug!("OSC 99: closing the unused connection failed: {close_error}");
        }
        return Err(e);
    }
    Ok(Session {
        connection,
        owner_tracking,
        signals,
    })
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

/// Answer a command that nothing will serve: a show fails and a close
/// completes (with nothing displayed, there is nothing to close).
fn report_unserved(sink: &EventSink, command: &DbusCommand) {
    match command {
        DbusCommand::Show { instance, .. } => {
            sink.emit(Osc99DbusEvent::ShowFailed {
                instance: *instance,
            });
        }
        DbusCommand::Close { instance } => sink.emit(Osc99DbusEvent::Closed {
            instance: *instance,
        }),
    }
}

/// Answer every show with `ShowFailed` and every close with `Closed` until
/// the GUI drops its sender: with no bus, nothing is displayed, so nothing
/// needs closing.
fn fail_every_command(commands: &Receiver<DbusCommand>, sink: &EventSink) {
    for command in commands {
        report_unserved(sink, &command);
    }
}

/// Serve `first` (if any), `commands` and the pumps' signals until the GUI
/// drops its command sender or a pump ends.
///
/// The capabilities are fetched when serving starts (unless nobody owns the
/// name) and again whenever the owner changes to a name they were not
/// fetched from, which also covers an owner adopted from a `Notify` reply.
fn serve(
    session: Session,
    state: &mut DispatcherState,
    commands: &Receiver<DbusCommand>,
    sink: &EventSink,
    first: Option<DbusCommand>,
) -> ServeEnd {
    let Session {
        connection,
        owner_tracking,
        signals,
    } = session;
    state.begin_session(resolve_daemon_owner(&connection, owner_tracking));
    if !matches!(state.daemon_owner, Osc99SignalSender::NoOwner) {
        refresh_capabilities(&connection, state, sink);
    }
    if let Some(command) = first {
        handle_command(&connection, state, sink, command);
        refresh_stale_capabilities(&connection, state, sink);
    }
    loop {
        select! {
            recv(commands) -> command => match command {
                Ok(command) => handle_command(&connection, state, sink, command),
                Err(_) => return ServeEnd::GuiGone,
            },
            recv(signals) -> signal => {
                if let Some(end) = handle_signal(state, sink, signal.ok()) {
                    return end;
                }
            },
        }
        refresh_stale_capabilities(&connection, state, sink);
    }
}

/// Apply a pump `signal` (`None`: every pump is gone and the channel closed
/// without a word). Returns [`ServeEnd::SignalsLost`] once no more signals can
/// come, so the caller stops serving.
fn handle_signal(
    state: &mut DispatcherState,
    sink: &EventSink,
    signal: Option<PumpSignal>,
) -> Option<ServeEnd> {
    let (events, end) = match signal {
        Some(signal @ PumpSignal::Ended(_)) => {
            (state.on_signal(&signal), Some(ServeEnd::SignalsLost))
        }
        Some(signal) => (state.on_signal(&signal), None),
        None => {
            tracing::warn!("OSC 99: every signal pump is gone; no more signals");
            (state.on_signals_lost(), Some(ServeEnd::SignalsLost))
        }
    };
    for event in events {
        sink.emit(event);
    }
    end
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
///
/// While the daemon is in its cooldown nothing is called and the show fails.
fn show_notification(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
    instance: Osc99InstanceId,
    replaces: Option<Osc99InstanceId>,
    notification: &Notification99Data,
) {
    if state.daemon_gate(Instant::now()) == DaemonGate::Skip {
        tracing::debug!("OSC 99: the notification daemon is unresponsive; failing a show");
        fail_show(connection, state, sink, instance, replaces);
        return;
    }
    let args = state.notify_args(replaces, notification);
    let outcome = notify(connection, &args);
    state.note_call(&outcome, Instant::now());
    match outcome {
        Ok(reply) => {
            if let Some(evicted) = state.record_shown(instance, replaces, reply) {
                sink.emit(Osc99DbusEvent::Closed { instance: evicted });
            }
            sink.emit(Osc99DbusEvent::Shown { instance });
        }
        Err(e) => {
            tracing::warn!("OSC 99: Notify failed: {e}");
            fail_show(connection, state, sink, instance, replaces);
        }
    }
}

/// Report a show as failed. The registry already forgot the replaced
/// instance, so its signals are of no interest either; but the daemon may
/// still display it, so it gets a best-effort `CloseNotification` rather than
/// staying on screen with nothing tracking it.
fn fail_show(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
    instance: Osc99InstanceId,
    replaces: Option<Osc99InstanceId>,
) {
    if let Some(old) = replaces {
        let old_id = state.dbus_id_of(old);
        state.forget_instance(old);
        if let Some(old_id) = old_id {
            close_replaced(connection, state, old_id);
        }
    }
    sink.emit(Osc99DbusEvent::ShowFailed { instance });
}

/// Ask the daemon to close a notification nobody tracks any more. The outcome
/// matters to nothing but the daemon's health, so a failure is only logged.
fn close_replaced(connection: &Connection, state: &mut DispatcherState, dbus_id: u32) {
    if state.daemon_gate(Instant::now()) == DaemonGate::Skip {
        return;
    }
    let outcome = close_notification(connection, dbus_id);
    state.note_call(&outcome, Instant::now());
    if let Err(e) = outcome {
        tracing::debug!("OSC 99: closing the replaced notification failed: {e}");
    }
}

/// Carry out a `Close`.
///
/// An instance the daemon displays is closed with `CloseNotification`, and the
/// daemon's `NotificationClosed` signal then produces the `Closed` event. If
/// the call fails, or the daemon is in its cooldown and is not called, the
/// signal may never come, so the mappings are dropped and `Closed` is emitted
/// here (kitty's fallback). An instance with no D-Bus id is reported closed at
/// once.
fn close_instance(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
    instance: Osc99InstanceId,
) {
    let CloseTarget::CallDaemon(dbus_id) = state.close_target(instance) else {
        sink.emit(Osc99DbusEvent::Closed { instance });
        return;
    };
    if state.daemon_gate(Instant::now()) == DaemonGate::Skip {
        tracing::debug!("OSC 99: the notification daemon is unresponsive; closing locally");
    } else {
        let outcome = close_notification(connection, dbus_id);
        state.note_call(&outcome, Instant::now());
        match outcome {
            Ok(()) => return,
            Err(e) => tracing::warn!("OSC 99: CloseNotification failed: {e}"),
        }
    }
    state.forget_instance(instance);
    sink.emit(Osc99DbusEvent::Closed { instance });
}

/// Fetch the capabilities again if the owner changed since they were fetched.
fn refresh_stale_capabilities(
    connection: &Connection,
    state: &mut DispatcherState,
    sink: &EventSink,
) {
    if state.capabilities_stale() {
        refresh_capabilities(connection, state, sink);
    }
}

/// Call `GetCapabilities` and, if it answers, record and report the result.
/// While the daemon is in its cooldown the call is not made and the
/// capabilities stay stale, so the first command after the cooldown asks.
fn refresh_capabilities(connection: &Connection, state: &mut DispatcherState, sink: &EventSink) {
    if state.daemon_gate(Instant::now()) == DaemonGate::Skip {
        tracing::debug!("OSC 99: the notification daemon is unresponsive; not asking it");
        return;
    }
    state.note_capabilities_asked();
    let outcome = fetch_capabilities(connection);
    state.note_call(&outcome, Instant::now());
    match outcome {
        Ok(advertised) => {
            state.set_capabilities(&advertised);
            sink.emit(Osc99DbusEvent::Capabilities(state.capabilities));
        }
        Err(e) if is_no_daemon(&e) => {
            tracing::debug!("OSC 99: no notification daemon to ask for its capabilities: {e}");
        }
        Err(e) => tracing::warn!("OSC 99: GetCapabilities failed; assuming defaults: {e}"),
    }
}

/// Who owns `org.freedesktop.Notifications` at start.
///
/// A peer-to-peer connection has no bus driver and no sender to check, so it
/// is [`Osc99SignalSender::Unfiltered`]. On a bus, only `NameHasNoOwner` means
/// that nobody owns the name; that and every other failure leave the owner
/// unknown ([`Osc99SignalSender::NoOwner`]): signals are ignored until a
/// `NameOwnerChanged` names an owner, or the sender of a `Notify` reply is
/// adopted (see [`DispatcherState::record_shown`]), so a lookup that merely
/// failed cannot leave the service deaf for good. Only an unexpected failure
/// is worth a warning: a missing daemon is the normal state of a machine
/// without one, and the call is sent with `NO_AUTO_START`, so it does not
/// start one. The `NameOwnerChanged` subscription was made before this call, so
/// an owner appearing meanwhile is queued for the dispatcher, and processing
/// it after a matching answer here is a no-op.
fn resolve_daemon_owner(connection: &Connection, tracking: OwnerTracking) -> Osc99SignalSender {
    match tracking {
        OwnerTracking::Peer => Osc99SignalSender::Unfiltered,
        OwnerTracking::Bus => match fetch_daemon_owner(connection) {
            Ok(owner) => Osc99SignalSender::Owner(owner),
            Err(e) if is_method_error(&e, ERROR_NAME_HAS_NO_OWNER) => {
                tracing::debug!("OSC 99: no notification daemon owns its name yet");
                Osc99SignalSender::NoOwner
            }
            Err(e) => {
                tracing::warn!(
                    "OSC 99: GetNameOwner failed ({e}); the next Notify reply names the daemon"
                );
                Osc99SignalSender::NoOwner
            }
        },
    }
}

/// Whether `error` is the D-Bus error reply named `name`.
fn is_method_error(error: &zbus::Error, name: &str) -> bool {
    matches!(error, zbus::Error::MethodError(error_name, _, _) if error_name.as_str() == name)
}

/// Whether `error` says that there is no daemon to call: nobody owns the
/// name and (for a `NO_AUTO_START` call) nothing may start one.
fn is_no_daemon(error: &zbus::Error) -> bool {
    is_method_error(error, ERROR_NAME_HAS_NO_OWNER) || is_method_error(error, ERROR_SERVICE_UNKNOWN)
}

/// Whether `error` is a method-call timeout.
///
/// zbus has no variant for it: its `method_timeout` (and [`call_with_timeout`])
/// fail with `Error::InputOutput` carrying an [`std::io::ErrorKind::TimedOut`]
/// `io::Error` (zbus 5.19, `abstractions/timeout.rs`).
fn is_method_timeout(error: &zbus::Error) -> bool {
    matches!(error, zbus::Error::InputOutput(io) if io.kind() == std::io::ErrorKind::TimedOut)
}

/// Run `call` on a short-lived helper thread and wait at most
/// [`OSC99_DBUS_METHOD_TIMEOUT`] for it.
///
/// A [`zbus::blocking::Proxy`] is the only public way to send a call with
/// message flags (`NO_AUTO_START`), and it does not apply the connection's
/// `method_timeout` the way `Connection::call_method` does, so a daemon that
/// never answers would block the caller for good. The helper thread keeps the
/// blocking call off the dispatcher: if it times out, the thread is left
/// blocked until the reply or the end of the connection (it holds no state
/// and does nothing after), and the caller gets the same error zbus itself
/// uses for a timeout, see [`is_method_timeout`]. The cooldown
/// ([`OSC99_DAEMON_COOLDOWN`]) bounds how many such threads can pile up.
fn call_with_timeout<R, F>(connection: &Connection, call: F) -> zbus::Result<R>
where
    R: Send + 'static,
    F: FnOnce(&Connection) -> zbus::Result<R> + Send + 'static,
{
    let (result_tx, result_rx) = crossbeam_channel::bounded(1);
    let connection = connection.clone();
    std::thread::Builder::new()
        .name("freminal-osc99-call".to_owned())
        .spawn(move || {
            if result_tx.send(call(&connection)).is_err() {
                tracing::debug!("OSC 99: a call finished after its caller gave up");
            }
        })
        .map_err(zbus::Error::from)?;
    match result_rx.recv_timeout(OSC99_DBUS_METHOD_TIMEOUT) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err(zbus::Error::from(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timed out",
        ))),
        Err(RecvTimeoutError::Disconnected) => Err(zbus::Error::Failure(
            "the call thread ended without an answer".to_owned(),
        )),
    }
}

/// Call the bus driver's `GetNameOwner` for the notification daemon's name,
/// without starting anything (`NO_AUTO_START`).
fn fetch_daemon_owner(connection: &Connection) -> zbus::Result<String> {
    call_with_timeout(connection, |connection| {
        Proxy::new(
            connection,
            BUS_DRIVER_NAME,
            BUS_DRIVER_PATH,
            BUS_DRIVER_INTERFACE,
        )?
        .call_with_flags::<_, _, String>(
            "GetNameOwner",
            MethodFlags::NoAutoStart.into(),
            &NOTIFICATIONS_SERVICE,
        )?
        .ok_or(zbus::Error::InvalidReply)
    })
}

/// Call `GetCapabilities`, without D-Bus-activating a daemon
/// (`NO_AUTO_START`): the first `Notify` does that.
fn fetch_capabilities(connection: &Connection) -> zbus::Result<Vec<String>> {
    call_with_timeout(connection, |connection| {
        Proxy::new(
            connection,
            NOTIFICATIONS_SERVICE,
            NOTIFICATIONS_PATH,
            NOTIFICATIONS_INTERFACE,
        )?
        .call_with_flags::<_, _, Vec<String>>(
            "GetCapabilities",
            MethodFlags::NoAutoStart.into(),
            &(),
        )?
        .ok_or(zbus::Error::InvalidReply)
    })
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

/// Call `Notify`, returning the daemon's id for the notification and the
/// unique name that sent the reply.
fn notify(connection: &Connection, args: &NotifyArgs) -> zbus::Result<NotifyReply> {
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
    let dbus_id = reply.body().deserialize::<u32>()?;
    Ok(NotifyReply {
        dbus_id,
        sender: reply.header().sender().map(ToString::to_string),
    })
}

// ── Signal pump ────────────────────────────────────────────────────────────

/// Start a pump thread: forward the signals `parse` recognises from `messages`
/// to the dispatcher.
fn spawn_pump(
    name: &str,
    kind: PumpKind,
    messages: MessageIterator,
    signals: Sender<PumpSignal>,
    parse: fn(&Message) -> Option<PumpSignal>,
) -> Result<(), StartError> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || run_pump(kind, messages, &signals, parse))
        .map(|_| ())
        .map_err(StartError::PumpThread)
}

/// A pump thread: forward until the stream ends or the dispatcher is gone,
/// then tell the dispatcher (if it is still there) that this pump is over.
fn run_pump(
    kind: PumpKind,
    messages: MessageIterator,
    signals: &Sender<PumpSignal>,
    parse: fn(&Message) -> Option<PumpSignal>,
) {
    forward_signals(messages, signals, parse);
    if signals.send(PumpSignal::Ended(kind)).is_err() {
        tracing::debug!("OSC 99: the dispatcher is gone; the {kind:?} pump exits quietly");
    }
}

/// Forward the signals `parse` recognises until the bus closes or the
/// dispatcher is gone.
fn forward_signals(
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use freminal_common::buffer_states::osc99_notification::{
        NotificationOccasion, Osc99ActivationFocus, Osc99ActivationReporting, Osc99CloseReporting,
        Osc99Sound,
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

    /// A `Notify` reply with no sender, as over a peer-to-peer connection.
    fn peer_reply(dbus_id: u32) -> NotifyReply {
        NotifyReply {
            dbus_id,
            sender: None,
        }
    }

    /// A `Notify` reply sent by `sender`, as a bus stamps it.
    fn reply_from(dbus_id: u32, sender: &str) -> NotifyReply {
        NotifyReply {
            dbus_id,
            sender: Some(sender.to_owned()),
        }
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
            Osc99SignalSender::Unfiltered,
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

    // ── Circuit breaker (Z6) ────────────────────────────────────────────

    fn timeout_error() -> zbus::Error {
        zbus::Error::from(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timed out",
        ))
    }

    /// An error reply named `name`, as a daemon or the bus sends it.
    fn error_reply(name: &str) -> Result<zbus::Error, Box<dyn Error>> {
        let call = Message::method_call("/", "Ping")?.build(&())?;
        let reply = Message::error(&call.header(), name.to_owned())?.build(&"details")?;
        Ok(zbus::Error::from(reply))
    }

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

    #[test]
    fn only_zbuss_timeout_error_is_a_method_timeout() -> TestResult {
        assert!(is_method_timeout(&timeout_error()));
        assert!(!is_method_timeout(&zbus::Error::from(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "gone",
        ))));
        assert!(!is_method_timeout(&zbus::Error::Failure(
            "timed out".to_owned()
        )));
        assert!(!is_method_timeout(&error_reply(
            "org.freedesktop.DBus.Error.NoReply"
        )?));
        Ok(())
    }

    // ── Error classification (Z2, Z18) ──────────────────────────────────

    #[test]
    fn a_missing_daemon_is_told_from_other_failures_by_the_error_name() -> TestResult {
        let no_owner = error_reply(ERROR_NAME_HAS_NO_OWNER)?;
        let unknown = error_reply(ERROR_SERVICE_UNKNOWN)?;
        let failed = error_reply("org.freedesktop.DBus.Error.Failed")?;

        assert!(is_method_error(&no_owner, ERROR_NAME_HAS_NO_OWNER));
        assert!(!is_method_error(&failed, ERROR_NAME_HAS_NO_OWNER));
        assert!(!is_method_error(&timeout_error(), ERROR_NAME_HAS_NO_OWNER));

        assert!(is_no_daemon(&no_owner));
        assert!(is_no_daemon(&unknown));
        assert!(!is_no_daemon(&failed));
        assert!(!is_no_daemon(&timeout_error()));
        Ok(())
    }

    // ── Reconnect gate (Z18) ────────────────────────────────────────────

    #[test]
    fn a_reconnect_waits_one_interval_after_the_last_attempt() {
        let start = Instant::now();
        let interval = Duration::from_secs(30);
        let mut gate = ReconnectGate::new(interval, start);

        assert_eq!(gate.decide(start), Reconnect::Later);
        assert_eq!(
            gate.decide(start + interval.saturating_sub(Duration::from_millis(1))),
            Reconnect::Later
        );
        assert_eq!(gate.decide(start + interval), Reconnect::Now);
        assert_eq!(
            gate.decide(start + interval + Duration::from_secs(1)),
            Reconnect::Later,
            "the attempt just made restarted the wait"
        );
        assert_eq!(gate.decide(start + interval * 2), Reconnect::Now);
    }

    #[test]
    fn a_zero_interval_always_allows_a_reconnect() {
        let start = Instant::now();
        let mut gate = ReconnectGate::new(Duration::ZERO, start);

        assert_eq!(gate.decide(start), Reconnect::Now);
        assert_eq!(gate.decide(start), Reconnect::Now);
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

    /// How the fake bus driver answers `GetNameOwner`.
    #[derive(Debug, Clone, Default)]
    enum NameOwnerAnswer {
        /// This unique name owns the name.
        Owner(String),
        /// Nobody does (`NameHasNoOwner`).
        #[default]
        Nobody,
        /// The lookup fails with an unrelated error.
        Broken,
    }

    /// Whether a call carried the `NO_AUTO_START` flag.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Activation {
        Allowed,
        Suppressed,
    }

    /// What the fake daemon saw and how it should behave.
    #[derive(Default)]
    struct DaemonLog {
        /// Every `Notify` it displayed, in call order.
        notifies: Mutex<Vec<RecordedNotify>>,
        /// The id of every `CloseNotification` call, in call order.
        closes: Mutex<Vec<u32>>,
        /// Every method call, in call order, with its `NO_AUTO_START` flag.
        calls: Mutex<Vec<(String, Activation)>>,
        /// Answer `CloseNotification` with an error instead of closing.
        fail_close: AtomicBool,
        /// What `GetCapabilities` answers.
        capabilities: Mutex<Vec<String>>,
        /// The sender stamped on replies, as a bus stamps them. Over a
        /// peer-to-peer connection nothing does, hence `None`.
        reply_sender: Mutex<Option<String>>,
        /// What `GetNameOwner` answers.
        name_owner: Mutex<NameOwnerAnswer>,
    }

    /// Reply to the call `header` belongs to, with the body `build` makes,
    /// stamped with `sender` (as a bus would) when there is one.
    fn reply_with<F>(
        daemon: &Connection,
        header: &zbus::message::Header<'_>,
        sender: Option<&str>,
        build: F,
    ) -> zbus::Result<()>
    where
        F: FnOnce(zbus::message::Builder<'static>) -> zbus::Result<Message>,
    {
        let mut builder = Message::method_return(header)?;
        if let Some(sender) = sender {
            builder = builder.sender(sender.to_owned())?;
        }
        daemon.send(&build(builder)?)
    }

    /// Serve one `Notify` call: record it and answer with the next id, or
    /// refuse it when its summary is [`REJECTED_SUMMARY`].
    fn fake_notify(
        daemon: &Connection,
        log: &DaemonLog,
        message: &Message,
        stamp: Option<&str>,
    ) -> zbus::Result<()> {
        let header = message.header();
        let (app_name, replaces_id, app_icon, summary, body, actions, hints, timeout) =
            message.body().deserialize::<NotifyBody>()?;
        if summary == REJECTED_SUMMARY {
            return daemon.reply_error(&header, "org.freedesktop.DBus.Error.Failed", &"rejected");
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
        reply_with(daemon, &header, stamp, |reply| reply.build(&assigned_id))
    }

    /// Serve `org.freedesktop.Notifications` (and the bus driver's
    /// `GetNameOwner`) on `daemon` until its connection closes.
    ///
    /// - `Notify` records the call and returns a fresh, incrementing id (even
    ///   for a replacement), and errors for [`REJECTED_SUMMARY`].
    /// - `CloseNotification` records the id, then replies and emits
    ///   `NotificationClosed(id, 3)` as a real daemon does, or errors when
    ///   `fail_close` is set.
    /// - `GetCapabilities` answers the log's capabilities.
    /// - `GetNameOwner` answers as the log's [`NameOwnerAnswer`] says.
    ///
    /// Replies carry the log's `reply_sender`. `messages` must have been
    /// created before the client could send its first call: a message stream
    /// only sees messages that arrive after it exists.
    ///
    /// Hand-written rather than a `#[zbus::interface]`: the macro's expansion
    /// of the eight-argument `Notify` trips pedantic lints that cannot be
    /// silenced without an `allow`.
    fn serve_fake_daemon(
        daemon: &Connection,
        messages: MessageIterator,
        log: &DaemonLog,
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
            let activation = if message
                .primary_header()
                .flags()
                .contains(zbus::message::Flags::NoAutoStart)
            {
                Activation::Suppressed
            } else {
                Activation::Allowed
            };
            log.calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((member.to_string(), activation));
            let stamp = log
                .reply_sender
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            match member.as_str() {
                "Notify" => fake_notify(daemon, log, &message, stamp.as_deref())?,
                "GetCapabilities" => {
                    let capabilities = log
                        .capabilities
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .clone();
                    reply_with(daemon, &header, stamp.as_deref(), |reply| {
                        reply.build(&capabilities)
                    })?;
                }
                "GetNameOwner" => {
                    let answer = log
                        .name_owner
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .clone();
                    match answer {
                        NameOwnerAnswer::Owner(owner) => {
                            reply_with(daemon, &header, None, |reply| reply.build(&owner))?;
                        }
                        NameOwnerAnswer::Nobody => daemon.reply_error(
                            &header,
                            ERROR_NAME_HAS_NO_OWNER,
                            &"the name has no owner",
                        )?,
                        NameOwnerAnswer::Broken => daemon.reply_error(
                            &header,
                            "org.freedesktop.DBus.Error.Failed",
                            &"lookup failed",
                        )?,
                    }
                }
                "CloseNotification" => {
                    let dbus_id = message.body().deserialize::<u32>()?;
                    log.closes
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(dbus_id);
                    if log.fail_close.load(Ordering::SeqCst) {
                        daemon.reply_error(
                            &header,
                            "org.freedesktop.DBus.Error.Failed",
                            &"cannot close",
                        )?;
                    } else {
                        reply_with(daemon, &header, stamp.as_deref(), |reply| reply.build(&()))?;
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
        let (server_stream, client_stream) = UnixStream::pair()?;
        let guid = zbus::Guid::generate();

        // The handshake needs both ends running at once.
        let server = std::thread::spawn(move || {
            zbus::blocking::connection::Builder::async_io_unix_stream(server_stream)
                .server(guid)?
                .p2p()
                .build()
        });
        let client = zbus::blocking::connection::Builder::async_io_unix_stream(client_stream)
            .p2p()
            .build()?;
        let daemon = server
            .join()
            .map_err(|_| "the fake daemon thread panicked")??;
        // Subscribe before anything can call: the client only starts talking
        // once the service is spawned, after this function returns, and a
        // stream created later (say, on the daemon thread) would miss a call
        // that arrived first, leaving the client waiting forever.
        let messages = MessageIterator::from(&daemon);

        let log = Arc::new(DaemonLog {
            capabilities: Mutex::new(strings(advertised)),
            ..DaemonLog::default()
        });
        let daemon_side = daemon.clone();
        let record = Arc::clone(&log);
        std::thread::Builder::new()
            .name("fake-notification-daemon".to_owned())
            .spawn(move || {
                if let Err(e) = serve_fake_daemon(&daemon_side, messages, &record) {
                    tracing::debug!("fake notification daemon stopped: {e}");
                }
            })?;
        Ok(Rig {
            client,
            daemon,
            log,
        })
    }

    /// A rig for a service that tracks owners as on a bus: `GetNameOwner`
    /// answers `owner` and every reply is stamped as sent by `replies_from`.
    fn bus_rig(
        advertised: &[&str],
        owner: NameOwnerAnswer,
        replies_from: Option<&str>,
    ) -> Result<Rig, Box<dyn Error>> {
        let rig = rig(advertised)?;
        *rig.log
            .name_owner
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = owner;
        *rig.log
            .reply_sender
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = replies_from.map(str::to_owned);
        Ok(rig)
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

        /// Every method call the daemon served, with its flag.
        fn calls(&self) -> Vec<(String, Activation)> {
            self.log
                .calls
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        /// What the daemon will answer `GetCapabilities` with from now on.
        fn advertise(&self, capabilities: &[&str]) {
            *self
                .log
                .capabilities
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = strings(capabilities);
        }

        /// Stamp every reply from now on as sent by `sender`.
        fn reply_as(&self, sender: &str) {
            *self
                .log
                .reply_sender
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(sender.to_owned());
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

        /// A `NotificationClosed` signal stamped as sent by `sender`: what a
        /// bus delivers, which `emit_signal` over a peer-to-peer connection
        /// cannot produce.
        fn emit_closed_from(&self, sender: &str, dbus_id: u32) -> zbus::Result<()> {
            let message = signal("NotificationClosed")?
                .sender(sender.to_owned())?
                .build(&(dbus_id, 1u32))?;
            self.daemon.send(&message)
        }

        /// An `ActionInvoked` signal stamped as sent by `sender`.
        fn emit_action_from(&self, sender: &str, dbus_id: u32, key: &str) -> zbus::Result<()> {
            let message = signal("ActionInvoked")?
                .sender(sender.to_owned())?
                .build(&(dbus_id, key))?;
            self.daemon.send(&message)
        }

        /// The bus driver's `NameOwnerChanged` for the daemon's name.
        fn emit_owner_changed(&self, new_owner: &str) -> zbus::Result<()> {
            let message = name_owner_changed(BUS_DRIVER_NAME, NOTIFICATIONS_SERVICE, new_owner)?;
            self.daemon.send(&message)
        }
    }

    /// The next event, failing the test if none arrives in [`EVENT_TIMEOUT`].
    fn next_event(handle: &Osc99DbusHandle) -> Result<Osc99DbusEvent, Box<dyn Error>> {
        Ok(handle.events.recv_timeout(EVENT_TIMEOUT)?)
    }

    /// The instance a test closes to prove nothing else is queued.
    const SENTINEL: Osc99InstanceId = Osc99InstanceId(u64::MAX);

    /// Prove that no event is queued behind the last one received.
    ///
    /// A close of an instance that was never shown is answered at once, from
    /// the dispatcher itself, so its event comes after every event the
    /// dispatcher produced for the commands and signals it had already taken:
    /// if the next event is the sentinel's, nothing was queued before it. A
    /// signal still travelling through a pump is not covered, so a test that
    /// ignores signals orders a later, observable signal behind them instead.
    fn assert_no_queued_events(handle: &mut Osc99DbusHandle) -> TestResult {
        assert_eq!(handle.close(SENTINEL), Osc99SystemDispatch::Queued);
        assert_eq!(
            next_event(handle)?,
            Osc99DbusEvent::Closed { instance: SENTINEL }
        );
        Ok(())
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

    /// Start the service on `rig` with `owner_tracking`, without consuming
    /// any event.
    fn spawn_on(rig: &Rig, owner_tracking: OwnerTracking) -> (Osc99DbusHandle, Receiver<()>) {
        let (waker, woken) = counting_waker();
        let handle =
            Osc99DbusService::spawn_with_connection(rig.client.clone(), owner_tracking, waker);
        (handle, woken)
    }

    /// Start the service against a fresh peer-to-peer rig and consume the
    /// `Capabilities` event, so a test begins with an empty event queue.
    fn started(
        advertised: &[&str],
    ) -> Result<(Rig, Osc99DbusHandle, Receiver<()>), Box<dyn Error>> {
        started_with(rig(advertised)?)
    }

    /// [`started`] for an existing rig.
    fn started_with(rig: Rig) -> Result<(Rig, Osc99DbusHandle, Receiver<()>), Box<dyn Error>> {
        let (handle, woken) = spawn_on(&rig, OwnerTracking::Peer);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));
        Ok((rig, handle, woken))
    }

    #[test]
    fn show_calls_notify_then_activation_and_close_signals_become_events() -> TestResult {
        let rig = rig(&["body", "actions"])?;
        let (mut handle, woken) = spawn_on(&rig, OwnerTracking::Peer);

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
        let (rig, mut handle, _woken) = started(&["body", "actions", "sound"])?;

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
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;

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
        let (rig, mut handle, _woken) = started(&["body"])?;

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
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
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
        assert_no_queued_events(&mut handle)
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
        // One `Closed`: the signal, not also a local one. A local one would
        // be emitted by the close command itself, so it would precede the
        // sentinel's event.
        assert_no_queued_events(&mut handle)
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
        assert_no_queued_events(&mut handle)
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
        assert_no_queued_events(&mut handle)
    }

    #[test]
    fn a_failed_replacing_show_closes_the_old_notification() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));
        let old_id = rig.recorded()[0].assigned_id;

        let _ = handle.show(
            instance(2),
            Some(instance(1)),
            &Notification99Data {
                title: REJECTED_SUMMARY.to_owned(),
                ..notification()
            },
        );

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::ShowFailed {
                instance: instance(2)
            }
        );
        // The best-effort close is made before `ShowFailed` is reported.
        assert_eq!(rig.closes(), [old_id]);

        // The old instance is forgotten: closing it calls nothing.
        let _ = handle.close(instance(1));
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(rig.closes(), [old_id]);
        Ok(())
    }

    #[test]
    fn the_capability_and_owner_calls_are_sent_without_auto_start() -> TestResult {
        let rig = bus_rig(&["body"], NameOwnerAnswer::Owner(":1.5".to_owned()), None)?;
        let (mut handle, _woken) = spawn_on(&rig, OwnerTracking::Bus);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));

        // `Notify` is the call that is allowed to start a daemon.
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));

        assert_eq!(
            rig.calls(),
            [
                ("GetNameOwner".to_owned(), Activation::Suppressed),
                ("GetCapabilities".to_owned(), Activation::Suppressed),
                ("Notify".to_owned(), Activation::Allowed),
            ]
        );
        Ok(())
    }

    #[test]
    fn dropping_the_handle_ends_the_dispatcher() -> TestResult {
        let (_rig, handle, _woken) = started(&["body"])?;
        let Osc99DbusHandle { commands, events } = handle;

        drop(commands);

        // The event sender lives on the dispatcher thread (and nowhere else),
        // so the channel disconnects exactly when that thread has returned.
        assert!(matches!(
            events.recv_timeout(EVENT_TIMEOUT),
            Err(RecvTimeoutError::Disconnected)
        ));
        Ok(())
    }

    #[test]
    fn losing_the_connection_closes_every_instance_and_reports_unavailable() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions"])?;
        for n in [1, 2] {
            let _ = handle.show(instance(n), None, &notification());
            assert_eq!(
                next_event(&handle)?,
                Osc99DbusEvent::Shown {
                    instance: instance(n)
                }
            );
        }

        // The daemon's end of the socket goes away: the client's pump sees the
        // stream end.
        rig.daemon.close()?;

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(2)
            }
        );
        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        // From now on the service answers as if the bus were down.
        assert_eq!(
            handle.show(instance(3), None, &notification()),
            Osc99SystemDispatch::Queued
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::ShowFailed {
                instance: instance(3)
            }
        );
        assert_eq!(
            handle.close(instance(2)),
            Osc99SystemDispatch::Queued,
            "queued; the dispatcher answers Closed itself"
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(2)
            }
        );
        Ok(())
    }

    // ── Owner tracking over the rig (bus mode on a peer-to-peer link) ──

    #[test]
    fn signals_from_anyone_but_the_owner_are_ignored() -> TestResult {
        let rig = bus_rig(
            &["body", "actions"],
            NameOwnerAnswer::Owner(":1.5".to_owned()),
            Some(":1.5"),
        )?;
        let (mut handle, _woken) = spawn_on(&rig, OwnerTracking::Bus);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));
        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));
        let dbus_id = rig.recorded()[0].assigned_id;

        // Forged signals first, then the real one: the real one is the first
        // event, so the forgeries (one pump, in order) were ignored.
        rig.emit_action_from(":1.9", dbus_id, "default")?;
        rig.emit_closed_from(":1.9", dbus_id)?;
        rig.emit_action_from(":1.5", dbus_id, "1")?;

        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Activated {
                instance: instance(1),
                action: Osc99SystemAction::Button(1),
            }
        );
        assert_no_queued_events(&mut handle)
    }

    #[test]
    fn a_new_owner_closes_only_the_old_daemons_instances_and_refreshes_the_capabilities()
    -> TestResult {
        let rig = bus_rig(
            &["body", "actions"],
            NameOwnerAnswer::Owner(":1.5".to_owned()),
            Some(":1.5"),
        )?;
        let (mut handle, _woken) = spawn_on(&rig, OwnerTracking::Bus);
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));
        let _ = handle.show(instance(1), None, &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(1)
            }
        );

        // The daemon restarts as `:1.6`, and a show is answered by it before
        // the bus driver's `NameOwnerChanged` has been processed.
        rig.reply_as(":1.6");
        rig.advertise(&["body", "sound"]);
        let _ = handle.show(instance(2), None, &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(2)
            }
        );
        rig.emit_owner_changed(":1.6")?;

        // Only the old daemon's instance is closed, and the new daemon is
        // asked what it can do.
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(1)
            }
        );
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(Osc99DaemonCapabilities {
                body: Osc99DaemonSupport::Supported,
                actions: Osc99DaemonSupport::Unsupported,
                body_markup: Osc99DaemonSupport::Unsupported,
                sound: Osc99DaemonSupport::Supported,
            })
        );

        // The new daemon's instance survived, and its signals are believed.
        let new_id = rig.recorded()[1].assigned_id;
        rig.emit_closed_from(":1.5", new_id)?;
        rig.emit_closed_from(":1.6", new_id)?;
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Closed {
                instance: instance(2)
            }
        );
        assert_no_queued_events(&mut handle)
    }

    #[test]
    fn a_failed_owner_lookup_adopts_the_notify_senders_name() -> TestResult {
        let rig = bus_rig(&["body", "actions"], NameOwnerAnswer::Broken, Some(":1.7"))?;
        let (mut handle, _woken) = spawn_on(&rig, OwnerTracking::Bus);

        // Nobody is known to own the name, so nobody is asked what it can do.
        assert_no_queued_events(&mut handle)?;
        assert_eq!(
            rig.calls(),
            [("GetNameOwner".to_owned(), Activation::Suppressed)]
        );

        // The reply to the first `Notify` names the daemon; that is also a
        // new owner, so the capabilities are fetched.
        let _ = handle.show(instance(1), None, &notification());
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(1)
            }
        );
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));

        // The adopted owner's signals are believed; others' are not.
        let dbus_id = rig.recorded()[0].assigned_id;
        rig.emit_action_from(":1.9", dbus_id, "default")?;
        rig.emit_action_from(":1.7", dbus_id, "default")?;
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Activated {
                instance: instance(1),
                action: Osc99SystemAction::Default,
            }
        );
        assert_no_queued_events(&mut handle)
    }

    #[test]
    fn no_daemon_at_start_means_no_capabilities_call_until_one_answers_a_notify() -> TestResult {
        let rig = bus_rig(&["body"], NameOwnerAnswer::Nobody, Some(":1.8"))?;
        let (mut handle, _woken) = spawn_on(&rig, OwnerTracking::Bus);

        assert_no_queued_events(&mut handle)?;
        assert_eq!(
            rig.calls(),
            [("GetNameOwner".to_owned(), Activation::Suppressed)],
            "a missing daemon is not asked, so it cannot be started by asking"
        );

        let _ = handle.show(instance(1), None, &notification());
        assert!(matches!(next_event(&handle)?, Osc99DbusEvent::Shown { .. }));
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));
        assert_eq!(
            rig.calls(),
            [
                ("GetNameOwner".to_owned(), Activation::Suppressed),
                ("Notify".to_owned(), Activation::Allowed),
                ("GetCapabilities".to_owned(), Activation::Suppressed),
            ]
        );
        Ok(())
    }

    // ── Reconnecting after a failed connect ─────────────────────────────

    /// A connect that fails on its first call and succeeds on the rest over
    /// `rig`, plus how often it was called.
    fn connect_after_one_failure(
        rig: &Rig,
    ) -> (
        impl FnMut() -> zbus::Result<Link> + Send + 'static,
        Arc<AtomicUsize>,
    ) {
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&attempts);
        let client = rig.client.clone();
        (
            move || {
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(zbus::Error::Failure("no bus".to_owned()))
                } else {
                    Ok(Link {
                        connection: client.clone(),
                        owner_tracking: OwnerTracking::Peer,
                    })
                }
            },
            attempts,
        )
    }

    #[test]
    fn a_show_after_a_failed_connect_reconnects_when_the_interval_has_passed() -> TestResult {
        let rig = rig(&["body", "actions"])?;
        let (connect, attempts) = connect_after_one_failure(&rig);
        let (waker, _woken) = counting_waker();
        let mut handle = start(connect, waker, Box::new(NoIconLookup), Duration::ZERO);
        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        let _ = handle.show(instance(1), None, &notification());

        // The new session reports the daemon again, then serves the show that
        // asked for it.
        assert!(matches!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(_)
        ));
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Shown {
                instance: instance(1)
            }
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(rig.recorded().len(), 1);
        Ok(())
    }

    #[test]
    fn a_show_soon_after_a_failed_connect_does_not_reconnect() -> TestResult {
        let rig = rig(&["body"])?;
        let (connect, attempts) = connect_after_one_failure(&rig);
        let (waker, _woken) = counting_waker();
        let mut handle = start(
            connect,
            waker,
            Box::new(NoIconLookup),
            Duration::from_secs(3600),
        );
        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        let _ = handle.show(instance(1), None, &notification());
        let _ = handle.show(instance(2), None, &notification());

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
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(rig.recorded(), []);
        Ok(())
    }

    #[test]
    fn an_unreachable_bus_reports_unavailable_once_and_fails_every_show() -> TestResult {
        let (waker, woken) = counting_waker();
        let mut handle = start(
            || Err(zbus::Error::Failure("no bus".to_owned())),
            waker,
            Box::new(NoIconLookup),
            Duration::from_secs(3600),
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
            Duration::from_secs(3600),
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
