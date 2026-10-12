// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The `org.freedesktop.Notifications` wire format and the method calls that
//! carry it: the `Notify` arguments and hints, the typed replies, and the
//! bounded calls to the daemon and the bus driver.

use crossbeam_channel::RecvTimeoutError;
use freminal_common::buffer_states::osc99_notification::{Notification99Data, NotificationUrgency};
use serde::{Serialize, Serializer};
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::MethodFlags;
use zbus::zvariant::{Signature, Type, as_value};

use crate::gui::osc99::presentation::{
    Osc99DecodedIconCache, Osc99IconChoice, Osc99IconImage, Osc99IconLookup, Osc99SoundHint,
    choose_icon,
};

use super::{
    BUS_DRIVER_INTERFACE, BUS_DRIVER_NAME, BUS_DRIVER_PATH, ERROR_NAME_HAS_NO_OWNER,
    ERROR_SERVICE_UNKNOWN, NOTIFICATIONS_INTERFACE, NOTIFICATIONS_PATH, NOTIFICATIONS_SERVICE,
    OSC99_DBUS_METHOD_TIMEOUT,
};

/// The `urgency` hint's value (`0` low, `1` normal, `2` critical).
pub(super) const fn urgency_byte(urgency: NotificationUrgency) -> u8 {
    match urgency {
        NotificationUrgency::Low => 0,
        NotificationUrgency::Normal => 1,
        NotificationUrgency::Critical => 2,
    }
}

/// The icon-theme name of freminal's own icon, as `assets/freminal.desktop`
/// names it. The last resort of icon resolution.
const FREMINAL_ICON_NAME: &str = "freminal";

/// The `bits_per_sample` field of the `image-data` structure.
const IMAGE_BITS_PER_SAMPLE: i32 = 8;

/// The `channels` field of the `image-data` structure (R, G, B, A).
const IMAGE_CHANNELS: i32 = 4;

/// A boolean the wire format asks for and freminal only ever sends as `true`:
/// `suppress-sound` is a request by being present, and the pixels of
/// `image-data` always have an alpha channel. There is no `false` to carry, so
/// the type has no value to get wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AlwaysTrue;

impl Serialize for AlwaysTrue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl Type for AlwaysTrue {
    const SIGNATURE: &'static Signature = <bool as Type>::SIGNATURE;
}

/// The `desktop-entry` hint: the name of freminal's `.desktop` file without
/// its extension, which lets the daemon group and attribute the notification.
const DESKTOP_ENTRY_HINT: &str = "freminal";

/// The `hints` argument of `Notify`, as presentation decided it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NotifyHints {
    /// The `urgency` hint.
    pub(super) urgency: u8,
    /// The `category` hint.
    pub(super) category: Option<String>,
    /// The `suppress-sound` / `sound-name` hint.
    pub(super) sound: Osc99SoundHint,
    /// The `image-data` hint, for transmitted icon data.
    pub(super) image: Option<Osc99IconImage>,
}

impl NotifyHints {
    /// The wire form: an `a{sv}` dictionary with the always-present hints and
    /// those that are set. It borrows from `self`; nothing is copied.
    fn to_wire(&self) -> WireHints<'_> {
        let (suppress_sound, sound_name) = match &self.sound {
            Osc99SoundHint::None => (None, None),
            Osc99SoundHint::Suppress => (Some(AlwaysTrue), None),
            Osc99SoundHint::Named(name) => (None, Some(name.clone())),
        };
        WireHints {
            urgency: self.urgency,
            category: self.category.clone(),
            suppress_sound,
            sound_name,
            image_data: self.image.as_ref().map(WireImage::from),
            desktop_entry: DESKTOP_ENTRY_HINT,
        }
    }
}

/// The `image-data` hint: the `(iiibiiay)` structure of width, height,
/// rowstride, `has_alpha`, `bits_per_sample`, channels and data. The data is one
/// byte array on the wire, never one variant per byte.
#[derive(Debug, PartialEq, Eq, Serialize, Type)]
struct WireImage<'a> {
    width: i32,
    height: i32,
    rowstride: i32,
    has_alpha: AlwaysTrue,
    bits_per_sample: i32,
    channels: i32,
    data: &'a [u8],
}

impl<'a> From<&'a Osc99IconImage> for WireImage<'a> {
    fn from(image: &'a Osc99IconImage) -> Self {
        Self {
            width: image.width,
            height: image.height,
            rowstride: image.rowstride,
            has_alpha: AlwaysTrue,
            bits_per_sample: IMAGE_BITS_PER_SAMPLE,
            channels: IMAGE_CHANNELS,
            data: &image.rgba,
        }
    }
}

/// [`NotifyHints`] as the `a{sv}` dictionary `Notify` takes. A hint that is
/// not set is left out of the dictionary; each present value is a variant
/// holding its own natural type.
#[derive(Debug, PartialEq, Eq, Serialize, Type)]
#[zvariant(signature = "a{sv}")]
#[serde(rename_all = "kebab-case")]
struct WireHints<'a> {
    #[serde(with = "as_value")]
    urgency: u8,
    #[serde(with = "as_value::optional", skip_serializing_if = "Option::is_none")]
    category: Option<String>,
    #[serde(with = "as_value::optional", skip_serializing_if = "Option::is_none")]
    suppress_sound: Option<AlwaysTrue>,
    #[serde(with = "as_value::optional", skip_serializing_if = "Option::is_none")]
    sound_name: Option<String>,
    #[serde(with = "as_value::optional", skip_serializing_if = "Option::is_none")]
    image_data: Option<WireImage<'a>>,
    #[serde(with = "as_value")]
    desktop_entry: &'static str,
}

/// The arguments of one `Notify` call, in the order the method takes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NotifyArgs {
    pub(super) app_name: String,
    pub(super) replaces_id: u32,
    pub(super) app_icon: String,
    pub(super) summary: String,
    pub(super) body: String,
    pub(super) actions: Vec<String>,
    pub(super) hints: NotifyHints,
    pub(super) expire_timeout: i32,
}

impl NotifyArgs {
    /// The `app_icon` and `image-data` for `notification`.
    ///
    /// Transmitted icon data that does not decode counts as no data: the
    /// icon is then the first name verbatim, else the `.desktop` icon of the
    /// application, else freminal's own, as in [`choose_icon`].
    pub(super) fn icon(
        lookup: &dyn Osc99IconLookup,
        cache: &mut Osc99DecodedIconCache,
        notification: &Notification99Data,
    ) -> (String, Option<Osc99IconImage>) {
        let choose = |data: Option<&[u8]>| {
            choose_icon(
                &notification.icon_names,
                data,
                notification.app_name.as_deref(),
                lookup,
            )
        };
        let freminal_icon = || (FREMINAL_ICON_NAME.to_owned(), None);
        let named = |choice| match choice {
            Some(Osc99IconChoice::Named(name)) => (name, None),
            // No rule found an icon: the last one is freminal's own.
            Some(Osc99IconChoice::Data(_)) | None => freminal_icon(),
        };
        match choose(notification.icon_data.as_deref()) {
            Some(Osc99IconChoice::Data(bytes)) => cache.decode(&bytes).map_or_else(
                || {
                    tracing::debug!("OSC 99: the transmitted icon did not decode");
                    named(choose(None))
                },
                |image| (String::new(), Some(image)),
            ),
            choice => named(choice),
        }
    }
}

/// What a successful `Notify` reply says: the daemon's id for the
/// notification, and the unique name that sent the reply.
///
/// The sender is `None` over a peer-to-peer connection, where nothing stamps
/// replies. On a bus it is the daemon that answered, which is how an
/// instance's owner is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NotifyReply {
    pub(super) dbus_id: u32,
    pub(super) sender: Option<String>,
}

/// Whether `error` is the D-Bus error reply named `name`.
pub(super) fn is_method_error(error: &zbus::Error, name: &str) -> bool {
    matches!(error, zbus::Error::MethodError(error_name, _, _) if error_name.as_str() == name)
}

/// Whether `error` says that there is no daemon to call: nobody owns the
/// name and (for a `NO_AUTO_START` call) nothing may start one.
pub(super) fn is_no_daemon(error: &zbus::Error) -> bool {
    is_method_error(error, ERROR_NAME_HAS_NO_OWNER) || is_method_error(error, ERROR_SERVICE_UNKNOWN)
}

/// Whether `error` is a method-call timeout.
///
/// zbus has no variant for it: its `method_timeout` (and [`call_with_timeout`])
/// fail with `Error::InputOutput` carrying an [`std::io::ErrorKind::TimedOut`]
/// `io::Error` (zbus 5.19, `abstractions/timeout.rs`).
pub(super) fn is_method_timeout(error: &zbus::Error) -> bool {
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
/// (`state::OSC99_DAEMON_COOLDOWN`) bounds how many such threads can pile up.
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
pub(super) fn fetch_daemon_owner(connection: &Connection) -> zbus::Result<String> {
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
pub(super) fn fetch_capabilities(connection: &Connection) -> zbus::Result<Vec<String>> {
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
pub(super) fn close_notification(connection: &Connection, dbus_id: u32) -> zbus::Result<()> {
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
pub(super) fn notify(connection: &Connection, args: &NotifyArgs) -> zbus::Result<NotifyReply> {
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
            args.hints.to_wire(),
            args.expire_timeout,
        ),
    )?;
    let dbus_id = reply.body().deserialize::<u32>()?;
    Ok(NotifyReply {
        dbus_id,
        sender: reply.header().sender().map(ToString::to_string),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::error::Error;

    use zbus::zvariant::{OwnedValue, Structure, Value};

    use super::super::test_rig::*;
    use super::*;

    /// `hints` serialised as a D-Bus message body would carry them, and read
    /// back as the receiving daemon reads them.
    fn round_trip(hints: &WireHints<'_>) -> Result<HashMap<String, OwnedValue>, Box<dyn Error>> {
        let context = zbus::zvariant::serialized::Context::new_dbus(zbus::zvariant::LE, 0);
        let data = zbus::zvariant::to_bytes(context, hints)?;
        Ok(data.deserialize::<HashMap<String, OwnedValue>>()?.0)
    }

    /// The hint `key` as a `T`, when it is present and is one.
    fn hint<T: TryFrom<OwnedValue>>(map: &HashMap<String, OwnedValue>, key: &str) -> Option<T> {
        T::try_from(map.get(key)?.try_clone().ok()?).ok()
    }

    fn sorted_keys(map: &HashMap<String, OwnedValue>) -> Vec<&str> {
        let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
        keys.sort_unstable();
        keys
    }

    #[test]
    fn hints_become_the_wire_dictionary() -> TestResult {
        let hints = NotifyHints {
            urgency: 1,
            category: Some("im".to_owned()),
            sound: Osc99SoundHint::Suppress,
            image: None,
        };
        assert_eq!(<WireHints<'_> as Type>::SIGNATURE.to_string(), "a{sv}");
        let map = round_trip(&hints.to_wire())?;

        assert_eq!(
            sorted_keys(&map),
            ["category", "desktop-entry", "suppress-sound", "urgency"]
        );
        assert_eq!(hint::<bool>(&map, "suppress-sound"), Some(true));
        assert_eq!(hint::<u8>(&map, "urgency"), Some(1));
        assert_eq!(hint::<String>(&map, "category").as_deref(), Some("im"));

        let named = NotifyHints {
            sound: Osc99SoundHint::Named("dialog-error".to_owned()),
            ..hints
        };
        let map = round_trip(&named.to_wire())?;
        assert_eq!(
            hint::<String>(&map, "sound-name").as_deref(),
            Some("dialog-error")
        );
        assert!(!map.contains_key("suppress-sound"));
        Ok(())
    }

    #[test]
    fn unset_hints_are_left_out_and_freminal_is_the_desktop_entry() -> TestResult {
        let hints = NotifyHints {
            urgency: 0,
            category: None,
            sound: Osc99SoundHint::None,
            image: None,
        };
        let map = round_trip(&hints.to_wire())?;

        assert_eq!(sorted_keys(&map), ["desktop-entry", "urgency"]);
        assert_eq!(
            hint::<String>(&map, "desktop-entry").as_deref(),
            Some("freminal")
        );
        Ok(())
    }

    #[test]
    fn the_image_hint_travels_as_one_byte_array_not_a_value_per_byte() -> TestResult {
        let image = Osc99IconImage {
            width: 256,
            height: 256,
            rowstride: 1024,
            rgba: (0..256 * 256 * 4)
                .map(|i| u8::try_from(i % 251).unwrap_or(0))
                .collect(),
        };
        let hints = NotifyHints {
            urgency: 1,
            category: None,
            sound: Osc99SoundHint::None,
            image: Some(image.clone()),
        };

        // Structurally: the wire hints borrow the pixel buffer. They hold no
        // `Value` (and no copy) for it, so nothing per byte is built.
        let wire = hints.to_wire();
        let wire_image = wire.image_data.as_ref().ok_or("no image on the wire")?;
        let held = hints.image.as_ref().ok_or("no image in the hints")?;
        assert!(std::ptr::eq(wire_image.data.as_ptr(), held.rgba.as_ptr()));
        assert_eq!(wire_image.data.len(), 256 * 256 * 4);

        // On the wire: the same pixels, in the `(iiibiiay)` structure.
        let map = round_trip(&wire)?;
        let value = map.get("image-data").ok_or("no image-data hint")?;
        let structure = Structure::try_from(Value::from(value.try_clone()?))?;
        let (w, h, stride, alpha, bits, channels, data) =
            <(i32, i32, i32, bool, i32, i32, Vec<u8>)>::try_from(structure)?;
        assert_eq!(
            (w, h, stride, alpha, bits, channels),
            (256, 256, 1024, true, 8, 4)
        );
        assert_eq!(data, image.rgba);
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
}
