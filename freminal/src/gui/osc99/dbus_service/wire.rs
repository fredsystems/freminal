// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The `org.freedesktop.Notifications` wire format and the method calls that
//! carry it: the `Notify` arguments and hints, the typed replies, and the
//! bounded calls to the daemon and the bus driver.

use std::time::Duration;

use freminal_common::buffer_states::osc99_notification::{Notification99Data, NotificationUrgency};
use serde::{Serialize, Serializer};
use zbus::blocking::Connection;
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

/// The notification's own image hint: the per-notification icon, as opposed
/// to `app_icon`, which the specification describes as the calling
/// program's icon.
///
/// Daemons that draw both show the image as the notification's picture.
/// Some (wayle's default `icon-source = "automatic"`) ignore `app_icon`
/// entirely, so an icon the application asked for is sent here as well.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NotifyImage {
    /// No image hint: the notification asked for no icon of its own.
    None,
    /// The `image-path` hint with an icon-theme name (the specification
    /// allows a name or a `file://` URI; freminal sends names only).
    Path(String),
    /// The `image-data` hint, for transmitted icon data.
    Data(Osc99IconImage),
}

/// The `hints` argument of `Notify`, as presentation decided it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NotifyHints {
    /// The `urgency` hint.
    pub(super) urgency: u8,
    /// The `category` hint.
    pub(super) category: Option<String>,
    /// The `suppress-sound` / `sound-name` hint.
    pub(super) sound: Osc99SoundHint,
    /// The `image-path` / `image-data` hint.
    pub(super) image: NotifyImage,
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
        let (image_path, image_data) = match &self.image {
            NotifyImage::None => (None, None),
            NotifyImage::Path(name) => (Some(name.clone()), None),
            NotifyImage::Data(image) => (None, Some(WireImage::from(image))),
        };
        WireHints {
            urgency: self.urgency,
            category: self.category.clone(),
            suppress_sound,
            sound_name,
            image_path,
            image_data,
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
    image_path: Option<String>,
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
    /// The `app_icon` and the image hint for `notification`.
    ///
    /// Transmitted icon data that does not decode counts as no data: the
    /// icon is then the first name verbatim, else the `.desktop` icon of the
    /// application, else freminal's own, as in [`choose_icon`].
    ///
    /// An icon chosen from the notification's own `n=` names is sent both as
    /// `app_icon` (kitty's behaviour) and as the `image-path` hint, so a
    /// daemon that only draws the notification image shows it too. The
    /// `.desktop` icon and freminal's own are the application's icon, not the
    /// notification's, so they go in `app_icon` only. With names present,
    /// [`choose_icon`] never picks the `.desktop` icon, and it returns `None`
    /// only when there are no names, so a named choice with names present is
    /// always one of them.
    pub(super) fn icon(
        lookup: &dyn Osc99IconLookup,
        cache: &mut Osc99DecodedIconCache,
        notification: &Notification99Data,
    ) -> (String, NotifyImage) {
        let choose = |data: Option<&[u8]>| {
            choose_icon(
                &notification.icon_names,
                data,
                notification.app_name.as_deref(),
                lookup,
            )
        };
        let freminal_icon = || (FREMINAL_ICON_NAME.to_owned(), NotifyImage::None);
        let named = |choice| match choice {
            Some(Osc99IconChoice::Named(name)) if !notification.icon_names.is_empty() => {
                (name.clone(), NotifyImage::Path(name))
            }
            Some(Osc99IconChoice::Named(name)) => (name, NotifyImage::None),
            // No rule found an icon: the last one is freminal's own.
            Some(Osc99IconChoice::Data(_)) | None => freminal_icon(),
        };
        match choose(notification.icon_data.as_deref()) {
            Some(Osc99IconChoice::Data(bytes)) => cache.decode(&bytes).map_or_else(
                || {
                    tracing::debug!("OSC 99: the transmitted icon did not decode");
                    named(choose(None))
                },
                |image| (String::new(), NotifyImage::Data(image)),
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
/// zbus has no variant for it: its `method_timeout` (and [`call_no_auto_start`])
/// fail with `Error::InputOutput` carrying an [`std::io::ErrorKind::TimedOut`]
/// `io::Error` (zbus 5.19, `abstractions/timeout.rs`).
pub(super) fn is_method_timeout(error: &zbus::Error) -> bool {
    matches!(error, zbus::Error::InputOutput(io) if io.kind() == std::io::ErrorKind::TimedOut)
}

/// Call `method` with the `NO_AUTO_START` flag and wait at most `timeout`
/// (in production [`OSC99_DBUS_METHOD_TIMEOUT`]) for the reply.
///
/// A proxy is the only public way to send a call with message flags, and it
/// does not apply the connection's `method_timeout` the way
/// `Connection::call_method` does, so a daemon that never answers would block
/// the caller for good. The call is therefore made with the async proxy and
/// raced against a timer on the calling thread. On a timeout the pending call
/// is dropped, which ends the wait for its reply: nothing is left behind (an
/// earlier version waited on a detached helper thread, which leaked one thread
/// per timed-out probe while the daemon stayed hung; 138.C26). The error is
/// the one zbus itself uses for a timeout, see [`is_method_timeout`].
fn call_no_auto_start<B, R>(
    connection: &Connection,
    target: CallTarget,
    method: &'static str,
    body: &B,
    timeout: Duration,
) -> zbus::Result<R>
where
    B: Serialize + zbus::zvariant::DynamicType,
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    use smol::future::FutureExt;

    let call = async {
        zbus::Proxy::new(
            connection.inner(),
            target.destination,
            target.path,
            target.interface,
        )
        .await?
        .call_with_flags::<_, _, R>(method, MethodFlags::NoAutoStart.into(), body)
        .await?
        .ok_or(zbus::Error::InvalidReply)
    };
    let timer = async {
        smol::Timer::after(timeout).await;
        Err(zbus::Error::from(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timed out",
        )))
    };
    smol::block_on(call.or(timer))
}

/// Where a [`call_no_auto_start`] call goes.
#[derive(Debug, Clone, Copy)]
struct CallTarget {
    destination: &'static str,
    path: &'static str,
    interface: &'static str,
}

/// Call the bus driver's `GetNameOwner` for the notification daemon's name,
/// without starting anything (`NO_AUTO_START`).
pub(super) fn fetch_daemon_owner(connection: &Connection) -> zbus::Result<String> {
    call_no_auto_start(
        connection,
        CallTarget {
            destination: BUS_DRIVER_NAME,
            path: BUS_DRIVER_PATH,
            interface: BUS_DRIVER_INTERFACE,
        },
        "GetNameOwner",
        &NOTIFICATIONS_SERVICE,
        OSC99_DBUS_METHOD_TIMEOUT,
    )
}

/// Call `GetCapabilities`, without D-Bus-activating a daemon
/// (`NO_AUTO_START`): the first `Notify` does that.
pub(super) fn fetch_capabilities(connection: &Connection) -> zbus::Result<Vec<String>> {
    call_no_auto_start(
        connection,
        CallTarget {
            destination: NOTIFICATIONS_SERVICE,
            path: NOTIFICATIONS_PATH,
            interface: NOTIFICATIONS_INTERFACE,
        },
        "GetCapabilities",
        &(),
        OSC99_DBUS_METHOD_TIMEOUT,
    )
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
            image: NotifyImage::None,
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
            image: NotifyImage::None,
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
            image: NotifyImage::Data(image.clone()),
        };

        // Structurally: the wire hints borrow the pixel buffer. They hold no
        // `Value` (and no copy) for it, so nothing per byte is built.
        let wire = hints.to_wire();
        let wire_image = wire.image_data.as_ref().ok_or("no image on the wire")?;
        let NotifyImage::Data(held) = &hints.image else {
            return Err("no image in the hints".into());
        };
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
        assert!(!map.contains_key("image-path"));
        Ok(())
    }

    #[test]
    fn a_named_image_travels_as_the_image_path_hint() -> TestResult {
        let hints = NotifyHints {
            urgency: 1,
            category: None,
            sound: Osc99SoundHint::None,
            image: NotifyImage::Path("dialog-error".to_owned()),
        };
        let map = round_trip(&hints.to_wire())?;

        assert_eq!(
            sorted_keys(&map),
            ["desktop-entry", "image-path", "urgency"]
        );
        assert_eq!(
            hint::<String>(&map, "image-path").as_deref(),
            Some("dialog-error")
        );
        Ok(())
    }

    #[test]
    fn an_unanswered_call_times_out_and_leaves_the_connection_usable() -> TestResult {
        let rig = rig(&["body"])?;
        let notifications = CallTarget {
            destination: NOTIFICATIONS_SERVICE,
            path: NOTIFICATIONS_PATH,
            interface: NOTIFICATIONS_INTERFACE,
        };

        // The fake daemon never answers a method it does not know.
        let outcome = call_no_auto_start::<_, String>(
            &rig.client,
            notifications,
            "NeverAnswered",
            &(),
            Duration::from_millis(100),
        );
        let error = outcome.err().ok_or("the call did not time out")?;
        assert!(is_method_timeout(&error), "{error}");

        // The dropped call did not disturb the connection.
        let advertised = call_no_auto_start::<_, Vec<String>>(
            &rig.client,
            notifications,
            "GetCapabilities",
            &(),
            Duration::from_secs(5),
        )?;
        assert_eq!(advertised, ["body"]);
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
